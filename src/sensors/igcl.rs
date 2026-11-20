//! The IGCL provider: Intel cards read through `ControlLib.dll` (Intel
//! Graphics Control Library, installed with the Intel graphics driver),
//! loaded only while the sensor service runs ([`super::vendor_gpu`]).
//! IGCL's power telemetry gives energy and activity as running counters,
//! so power and load come from two reads ([`sample`]).

use super::gpu::GpuSample;
use super::vendor_gpu::{Vendor, VendorGpu};

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "IGCL";

/// What the provider shows of Intel cards.
pub fn vendor() -> Vendor {
    Vendor { source: SOURCE, vendor_id: super::gpu::INTEL, not_reported: "IGCL does not report it", prefix: "intel", no_card: "no Intel GPU" }
}

/// One read of IGCL's power telemetry (`None` = not supported). Energy in
/// joules and activity in seconds are running counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Telemetry {
    pub time_s: Option<f64>,
    pub gpu_energy_j: Option<f64>,
    pub card_energy_j: Option<f64>,
    pub activity_s: Option<f64>,
    pub gpu_mhz: Option<f64>,
    pub vram_mhz: Option<f64>,
    pub temperature_c: Option<f64>,
    pub vram_temperature_c: Option<f64>,
}

/// A card's values from its telemetry `now` and the read before (`prev`):
/// power = energy used / time, load = busy time / time (none on the first
/// read or when the counters went back).
pub fn sample(prev: Option<&Telemetry>, now: &Telemetry) -> GpuSample {
    let rate = |counter: fn(&Telemetry) -> Option<f64>| {
        let prev = prev?;
        let dt = now.time_s? - prev.time_s?;
        let used = counter(now)? - counter(prev)?;
        (dt > 0.0 && used >= 0.0).then(|| used / dt)
    };
    let power = rate(|t| t.card_energy_j).or_else(|| rate(|t| t.gpu_energy_j));
    GpuSample {
        graphics_mhz: now.gpu_mhz,
        memory_mhz: now.vram_mhz,
        power_w: power,
        temperature_c: now.temperature_c,
        load_percent: rate(|t| t.activity_s).map(|busy| (busy * 100.0).clamp(0.0, 100.0)),
        ..Default::default()
    }
}

/// The provider: `ControlLib.dll` loaded if present, the Intel cards it
/// sees matched to DXCore's.
pub fn provider() -> VendorGpu {
    #[cfg(windows)]
    let library = imp::Igcl::open().map(|l| Box::new(l) as Box<dyn super::vendor_gpu::Library>);
    #[cfg(not(windows))]
    let library = Err("IGCL is a Windows library".to_string());
    VendorGpu::new(vendor(), library, &super::dxcore::adapters().unwrap_or_default())
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::mem::{size_of, transmute, zeroed};
    use std::ptr::null_mut;

    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    use super::{sample, Telemetry};
    use crate::sensors::gpu::{Adapter, GpuSample};
    use crate::sensors::vendor_gpu::Library;

    type Handle = *mut c_void;
    type Init = unsafe extern "C" fn(*mut InitArgs, *mut Handle) -> u32;
    type Close = unsafe extern "C" fn(Handle) -> u32;
    type Enumerate = unsafe extern "C" fn(Handle, *mut u32, *mut Handle) -> u32;
    type Properties = unsafe extern "C" fn(Handle, *mut AdapterProperties) -> u32;
    type PowerTelemetry = unsafe extern "C" fn(Handle, *mut PowerTelemetryT) -> u32;

    const CTL_RESULT_SUCCESS: u32 = 0;
    /// CTL_MAKE_VERSION(1, 1).
    const CTL_IMPL_VERSION: u32 = (1 << 16) | 1;
    const CTL_INIT_FLAG_USE_LEVEL_ZERO: u32 = 1;
    const CTL_DEVICE_TYPE_GRAPHICS: u32 = 1;

    // The structures of igcl_api.h (Intel drivers.gpu.control-library).
    #[repr(C)]
    struct InitArgs {
        size: u32,
        version: u8,
        app_version: u32,
        flags: u32,
        supported_version: u32,
        application_uid: [u8; 16],
    }

    #[repr(C)]
    struct AdapterProperties {
        size: u32,
        version: u8,
        device_id: *mut c_void,
        device_id_size: u32,
        device_type: u32,
        supported_subfunction_flags: u32,
        driver_version: u64,
        firmware_version: [u64; 3],
        pci_vendor_id: u32,
        pci_device_id: u32,
        rev_id: u32,
        num_eus_per_sub_slice: u32,
        num_sub_slices_per_slice: u32,
        num_slices: u32,
        name: [u8; 100],
        graphics_adapter_properties: u32,
        frequency: u32,
        pci_subsys_id: u16,
        pci_subsys_vendor_id: u16,
        adapter_bdf: [u8; 3],
        num_xe_cores: u32,
        reserved: [u8; 108],
    }

    /// ctl_oc_telemetry_item_t.
    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Item {
        supported: bool,
        units: u32,
        data_type: u32,
        /// ctl_data_value_t (a union of 8 bytes).
        value: u64,
    }

    impl Item {
        fn get(&self) -> Option<f64> {
            if !self.supported {
                return None;
            }
            let v = self.value;
            let x = match self.data_type {
                0 => v as u8 as i8 as f64,
                1 => v as u8 as f64,
                2 => v as u16 as i16 as f64,
                3 => v as u16 as f64,
                4 => v as u32 as i32 as f64,
                5 => v as u32 as f64,
                6 => v as i64 as f64,
                7 => v as f64,
                8 => f64::from(f32::from_bits(v as u32)),
                9 => f64::from_bits(v),
                _ => return None,
            };
            x.is_finite().then_some(x)
        }
    }

    #[repr(C)]
    struct PsuInfo {
        supported: bool,
        psu_type: u32,
        energy_counter: Item,
        voltage: Item,
    }

    /// ctl_power_telemetry_t.
    #[repr(C)]
    struct PowerTelemetryT {
        size: u32,
        version: u8,
        time_stamp: Item,
        gpu_energy_counter: Item,
        gpu_voltage: Item,
        gpu_current_clock_frequency: Item,
        gpu_current_temperature: Item,
        global_activity_counter: Item,
        render_compute_activity_counter: Item,
        media_activity_counter: Item,
        gpu_limited: [bool; 5],
        vram_energy_counter: Item,
        vram_voltage: Item,
        vram_current_clock_frequency: Item,
        vram_current_effective_frequency: Item,
        vram_read_bandwidth_counter: Item,
        vram_write_bandwidth_counter: Item,
        vram_current_temperature: Item,
        vram_limited: [bool; 5],
        total_card_energy_counter: Item,
        psu: [PsuInfo; 5],
        fan_speed: [Item; 5],
        gpu_vr_temp: Item,
        vram_vr_temp: Item,
        sa_vr_temp: Item,
        gpu_effective_clock: Item,
        gpu_over_voltage_percent: Item,
        gpu_power_percent: Item,
        gpu_temperature_percent: Item,
        vram_read_bandwidth: Item,
        vram_write_bandwidth: Item,
    }

    /// A started IGCL: the library, its API handle and the Intel cards
    /// (with their last telemetry). Dropped = IGCL closed, library freed.
    pub struct Igcl {
        lib: HMODULE,
        api: Handle,
        close: Option<Close>,
        telemetry: Option<PowerTelemetry>,
        cards: Vec<(Handle, Adapter, Option<Telemetry>)>,
    }

    impl Igcl {
        /// Loads `ControlLib.dll`, starts IGCL and finds the Intel cards.
        pub fn open() -> Result<Igcl, String> {
            let name: Vec<u16> = "ControlLib.dll\0".encode_utf16().collect();
            // SAFETY: signatures and layouts are igcl_api.h's; `s` closes
            // IGCL and frees the library on every early return.
            unsafe {
                let lib = LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
                if lib.is_null() {
                    return Err("ControlLib.dll not found (no Intel graphics driver)".into());
                }
                let mut s = Igcl { lib, api: null_mut(), close: None, telemetry: None, cards: Vec::new() };
                let sym = |n: &std::ffi::CStr| GetProcAddress(lib, n.as_ptr().cast());
                let (Some(init), Some(close), Some(enumerate), Some(properties)) =
                    (sym(c"ctlInit"), sym(c"ctlClose"), sym(c"ctlEnumerateDevices"), sym(c"ctlGetDeviceProperties"))
                else {
                    return Err("ControlLib.dll is too old".into());
                };
                let (init, enumerate, properties): (Init, Enumerate, Properties) = (transmute(init), transmute(enumerate), transmute(properties));
                let mut args: InitArgs = zeroed();
                args.size = size_of::<InitArgs>() as u32;
                args.app_version = CTL_IMPL_VERSION;
                args.flags = CTL_INIT_FLAG_USE_LEVEL_ZERO;
                args.application_uid = *b"UltimatePCManagr";
                let code = init(&mut args, &mut s.api);
                if code != CTL_RESULT_SUCCESS || s.api.is_null() {
                    return Err(format!("IGCL did not start (error {code:#x})"));
                }
                s.close = Some(transmute(close));
                s.telemetry = sym(c"ctlPowerTelemetryGet").map(|f| transmute(f));
                let mut n = 0u32;
                if enumerate(s.api, &mut n, null_mut()) != CTL_RESULT_SUCCESS || n == 0 {
                    return Ok(s);
                }
                let mut handles = vec![null_mut(); n as usize];
                if enumerate(s.api, &mut n, handles.as_mut_ptr()) != CTL_RESULT_SUCCESS {
                    return Ok(s);
                }
                for h in handles.into_iter().take(n as usize) {
                    let mut luid = [0u8; 8];
                    let mut p: AdapterProperties = zeroed();
                    p.size = size_of::<AdapterProperties>() as u32;
                    p.version = 2;
                    p.device_id = luid.as_mut_ptr().cast();
                    p.device_id_size = luid.len() as u32;
                    if properties(h, &mut p) != CTL_RESULT_SUCCESS || p.device_type != CTL_DEVICE_TYPE_GRAPHICS {
                        continue;
                    }
                    let end = p.name.iter().position(|&b| b == 0).unwrap_or(p.name.len());
                    let name = String::from_utf8_lossy(&p.name[..end]).into_owned();
                    s.cards.push((h, Adapter { name, vendor_id: p.pci_vendor_id, device_id: p.pci_device_id }, None));
                }
                Ok(s)
            }
        }
    }

    impl Library for Igcl {
        fn cards(&self) -> Vec<Adapter> {
            self.cards.iter().map(|(_, a, _)| a.clone()).collect()
        }

        fn sample(&mut self, card: usize) -> GpuSample {
            let (Some(get), Some((h, _, prev))) = (self.telemetry, self.cards.get_mut(card)) else { return GpuSample::default() };
            // SAFETY: `h` is a device of this IGCL session; the structure
            // is sized and versioned as the API asks.
            let t = unsafe {
                let mut t: PowerTelemetryT = zeroed();
                t.size = size_of::<PowerTelemetryT>() as u32;
                t.version = 1;
                if get(*h, &mut t) != CTL_RESULT_SUCCESS {
                    return GpuSample::default();
                }
                t
            };
            let now = Telemetry {
                time_s: t.time_stamp.get(),
                gpu_energy_j: t.gpu_energy_counter.get(),
                card_energy_j: t.total_card_energy_counter.get(),
                activity_s: t.global_activity_counter.get(),
                gpu_mhz: t.gpu_current_clock_frequency.get(),
                vram_mhz: t.vram_current_clock_frequency.get(),
                temperature_c: t.gpu_current_temperature.get(),
                vram_temperature_c: t.vram_current_temperature.get(),
            };
            let out = sample(prev.as_ref(), &now);
            *prev = Some(now);
            out
        }
    }

    impl Drop for Igcl {
        fn drop(&mut self) {
            // SAFETY: close only after a successful init; nothing of the
            // library is used afterwards.
            unsafe {
                if let Some(close) = self.close {
                    close(self.api);
                }
                FreeLibrary(self.lib);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_structures_have_igcls_sizes() {
            // Item: bool + pad, two enums, pad, an 8-byte union.
            assert_eq!(size_of::<Item>(), 24);
            assert_eq!(size_of::<PsuInfo>(), 56);
            assert_eq!(std::mem::offset_of!(PowerTelemetryT, time_stamp), 8);
            assert_eq!(std::mem::offset_of!(AdapterProperties, name), 88);
        }

        #[test]
        fn a_telemetry_item_reads_the_type_it_says() {
            let item = |data_type, value| Item { supported: true, units: 0, data_type, value };
            assert_eq!(item(9, 41.5f64.to_bits()).get(), Some(41.5));
            assert_eq!(item(8, u64::from(2.5f32.to_bits())).get(), Some(2.5));
            assert_eq!(item(5, 2100).get(), Some(2100.0));
            assert_eq!(item(4, u64::from((-3i32) as u32)).get(), Some(-3.0));
            assert_eq!(Item { supported: false, ..item(9, 1.0f64.to_bits()) }.get(), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(time_s: f64, energy_j: f64, activity_s: f64) -> Telemetry {
        Telemetry { time_s: Some(time_s), gpu_energy_j: Some(energy_j), activity_s: Some(activity_s), gpu_mhz: Some(2100.0), temperature_c: Some(55.0), ..Default::default() }
    }

    #[test]
    fn power_and_load_come_from_two_reads_of_the_counters() {
        let s = sample(Some(&at(10.0, 1000.0, 4.0)), &at(12.0, 1070.0, 5.5));
        assert_eq!(s.power_w, Some(35.0), "70 J in 2 s");
        assert_eq!(s.load_percent, Some(75.0), "1.5 s busy in 2 s");
        assert_eq!(s.graphics_mhz, Some(2100.0));
        assert_eq!(s.temperature_c, Some(55.0));
        assert_eq!(s.hotspot_c, None);
    }

    #[test]
    fn the_first_read_has_clocks_but_no_power_or_load() {
        let s = sample(None, &at(10.0, 1000.0, 4.0));
        assert_eq!((s.power_w, s.load_percent), (None, None), "never shown as zero");
        assert_eq!(s.graphics_mhz, Some(2100.0));
    }

    #[test]
    fn the_whole_card_power_is_preferred_and_a_counter_that_went_back_gives_nothing() {
        let mut a = at(0.0, 0.0, 0.0);
        let mut b = at(1.0, 20.0, 0.5);
        a.card_energy_j = Some(100.0);
        b.card_energy_j = Some(150.0);
        assert_eq!(sample(Some(&a), &b).power_w, Some(50.0), "card, not GPU only");
        assert_eq!(sample(Some(&b), &a).power_w, None, "time went back");
        let restarted = at(2.0, 5.0, 0.6);
        assert_eq!(sample(Some(&b), &restarted).power_w, None, "energy went back (driver restarted): no value");
    }

    #[test]
    #[ignore = "reads this PC (no Intel GPU, no IGCL)"]
    fn on_this_pc_igcl_is_unavailable_and_nothing_stays_loaded() {
        use crate::sensors::provider::Provider;
        let mut p = provider();
        let why = p.availability().expect_err("no Intel GPU on this PC");
        println!("IGCL: {why}");
        assert!(why.contains("ControlLib.dll") || why == "no Intel GPU");
        assert!(p.read(std::time::Instant::now()).is_empty());
    }
}
