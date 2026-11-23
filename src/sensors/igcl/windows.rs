//! Windows: IGCL, loaded from `ControlLib.dll` only while the service runs.

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

/// The library, if it loads.
pub fn library() -> Result<Box<dyn crate::sensors::vendor_gpu::Library>, String> {
    Igcl::open().map(|l| Box::new(l) as Box<dyn crate::sensors::vendor_gpu::Library>)
}
