//! The NVML provider: NVIDIA cards read through `nvml.dll` (installed with
//! the NVIDIA driver). The DLL is loaded when the provider is built (the
//! sensor service is running); when the provider is dropped (nobody asks
//! for sensors any more) NVML is shut down and the library freed. When the
//! DLL is missing nothing is loaded. Note: `nvmlInit` pins `nvml.dll`, so
//! after the first start its code stays mapped (idle, shut down) until the
//! app exits; nothing of it runs while the service is stopped.

use std::time::Instant;

use super::gpu;
use super::model::{Device, Group, Reading};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "NVML";
/// Why a value NVML does not give is missing (hot spot, fan on laptops).
#[cfg_attr(not(windows), allow(dead_code))]
const NOT_REPORTED: &str = "NVML does not report it";

/// The provider: a started NVML session, or why there is none.
pub struct Nvml {
    #[cfg(windows)]
    session: Result<imp::Session, String>,
}

impl Nvml {
    pub fn new() -> Self {
        Nvml {
            #[cfg(windows)]
            session: imp::Session::open(),
        }
    }
}

/// Whether a PC whose cards are `listed` (by DXCore) is one NVML is for: it
/// has an NVIDIA card, or its cards are unknown.
pub fn for_a_pc_with(listed: &Result<Vec<gpu::Adapter>, String>) -> bool {
    listed.as_ref().map_or(true, |cards| cards.iter().any(|a| a.vendor_id == gpu::NVIDIA))
}

impl Provider for Nvml {
    fn name(&self) -> &'static str {
        SOURCE
    }

    /// Only a PC with an NVIDIA card lists NVML.
    fn for_this_pc(&mut self) -> bool {
        for_a_pc_with(&super::dxcore::adapters())
    }

    #[cfg(windows)]
    fn availability(&mut self) -> Result<(), String> {
        self.session.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    #[cfg(not(windows))]
    fn availability(&mut self) -> Result<(), String> {
        Err("no NVIDIA GPU".into())
    }

    #[cfg(windows)]
    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let Ok(s) = &self.session else { return Vec::new() };
        s.cards
            .iter()
            .flat_map(|(handle, card)| {
                let mut readings = s.sample(*handle).readings(card, SOURCE, NOT_REPORTED, at);
                readings.extend(s.extras(*handle).readings(card, at));
                readings
            })
            .collect()
    }

    #[cfg(not(windows))]
    fn read(&mut self, _at: Instant) -> Vec<Reading> {
        Vec::new()
    }
}

/// The clock event reasons NVML reports, in its bit order (NVML API
/// Reference, "ClocksEventReasons", R535+:
/// <https://docs.nvidia.com/deploy/archive/R535/nvml-api/group__nvmlClocksEventReasons.html>;
/// the deprecated `nvmlClocksThrottleReason*` names have the same values).
/// NVML has no "reliability voltage" bit (that limit is NVAPI's), so it
/// cannot be shown from here.
const CLOCK_EVENT_REASONS: [(u64, &str); 9] = [
    (0x0000_0001, "Idle"),                  // nvmlClocksEventReasonGpuIdle
    (0x0000_0002, "Application setting"),   // nvmlClocksEventReasonApplicationsClocksSetting
    (0x0000_0004, "Power cap"),             // nvmlClocksEventReasonSwPowerCap
    (0x0000_0008, "Hardware slowdown"),     // nvmlClocksThrottleReasonHwSlowdown
    (0x0000_0010, "Sync boost"),            // nvmlClocksEventReasonSyncBoost
    (0x0000_0020, "Thermal"),               // nvmlClocksEventReasonSwThermalSlowdown
    (0x0000_0040, "Hardware thermal"),      // nvmlClocksThrottleReasonHwThermalSlowdown
    (0x0000_0080, "Power brake"),           // nvmlClocksThrottleReasonHwPowerBrakeSlowdown
    (0x0000_0100, "Display clock setting"), // nvmlClocksEventReasonDisplayClockSetting
];

/// The clock event reasons bitmask in words ("Power cap, Thermal"); no bit
/// set is "None" (clocks as high as possible), bits NVML may add later show
/// as "Unknown (0x...)".
pub fn clock_event_reasons(mask: u64) -> String {
    let known = CLOCK_EVENT_REASONS.iter().fold(0, |all, (bit, _)| all | bit);
    let mut words: Vec<String> = CLOCK_EVENT_REASONS.iter().filter(|(bit, _)| mask & bit != 0).map(|(_, w)| (*w).to_string()).collect();
    if mask & !known != 0 {
        words.push(format!("Unknown ({:#x})", mask & !known));
    }
    if words.is_empty() {
        return "None".into();
    }
    words.join(", ")
}

/// The live PCIe link next to the maximum: `(generation, width)` pairs as
/// nvmlDeviceGetCurrPcieLinkGeneration/Width and
/// nvmlDeviceGetMaxPcieLinkGeneration/Width give them ("Gen 4 x16 of
/// Gen 4 x16"); without a maximum, the live link alone.
pub fn pcie_link(current: (u32, u32), max: Option<(u32, u32)>) -> String {
    let link = |(generation, width): (u32, u32)| format!("Gen {generation} x{width}");
    match max {
        Some(max) => format!("{} of {}", link(current), link(max)),
        None => link(current),
    }
}

/// An `nvmlPstates_t` in words: NVML_PSTATE_0..NVML_PSTATE_15 are 0..15
/// ("P0" = maximum performance); NVML_PSTATE_UNKNOWN (32) and anything else
/// is no state.
pub fn performance_state(pstate: u32) -> Option<String> {
    (pstate <= 15).then(|| format!("P{pstate}"))
}

/// NVML_ERROR_NOT_SUPPORTED (nvmlReturn_t).
const NOT_SUPPORTED: u32 = 3;

/// Why a reading is missing when this driver's `nvml.dll` lacks `function`.
pub fn missing(function: &str) -> String {
    format!("this NVIDIA driver has no {function}")
}

/// Why a reading is missing when NVML answered the error `code`.
pub fn failed(code: u32) -> String {
    match code {
        NOT_SUPPORTED => "not supported on this card".into(),
        _ => format!("NVML could not read it (error {code})"),
    }
}

/// One read of the NVIDIA-only states of a card; `Err` = why not.
#[derive(Clone, Debug, PartialEq)]
pub struct Extras {
    /// The clock event reasons bitmask.
    pub clock_reasons: Result<u64, String>,
    /// The `nvmlPstates_t`.
    pub pstate: Result<u32, String>,
    /// The live `(generation, width)` and the maximum, if NVML gave it.
    pub pcie: Result<((u32, u32), Option<(u32, u32)>), String>,
}

impl Extras {
    /// Clock Limit Reasons (Clocks), Performance State and PCIe Link
    /// (General): states in words, read from NVML, grey with the reason.
    pub fn readings(&self, device: &Device, at: Instant) -> Vec<Reading> {
        let row = |what: &str, group: Group, name: &str, words: &Result<String, String>| {
            let why = words.as_ref().err().map_or("", String::as_str);
            Reading::state(&gpu::metric(device, what), device.clone(), group, name, SOURCE, at).with_text(words.clone().ok(), why)
        };
        let pstate = self.pstate.clone().and_then(|p| performance_state(p).ok_or_else(|| "NVML reports no performance state".to_string()));
        vec![
            row("clock.limit_reasons", Group::Clocks, "Clock Limit Reasons", &self.clock_reasons.clone().map(clock_event_reasons)),
            row("performance_state", Group::General, "Performance State", &pstate),
            row("pcie.link", Group::General, "PCIe Link", &self.pcie.clone().map(|(current, max)| pcie_link(current, max))),
        ]
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::mem::transmute_copy;

    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

    use crate::sensors::gpu::{self, Adapter, GpuSample};
    use crate::sensors::model::Device;

    type Dev = *mut c_void;
    type Plain = unsafe extern "C" fn() -> u32;
    type Count = unsafe extern "C" fn(*mut u32) -> u32;
    type Handle = unsafe extern "C" fn(u32, *mut Dev) -> u32;
    type Name = unsafe extern "C" fn(Dev, *mut u8, u32) -> u32;
    type Pci = unsafe extern "C" fn(Dev, *mut PciInfo) -> u32;
    /// GetClockInfo, GetMaxClockInfo, GetTemperature: (device, type, out).
    type Typed = unsafe extern "C" fn(Dev, u32, *mut u32) -> u32;
    type Value = unsafe extern "C" fn(Dev, *mut u32) -> u32;
    type Utilization = unsafe extern "C" fn(Dev, *mut [u32; 2]) -> u32;
    type Codec = unsafe extern "C" fn(Dev, *mut u32, *mut u32) -> u32;
    /// nvmlDeviceGetCurrentClocksEventReasons(nvmlDevice_t, unsigned long long*)
    /// (R535+) and the deprecated nvmlDeviceGetCurrentClocksThrottleReasons
    /// with the same signature (NVML API Reference, Device Queries:
    /// <https://docs.nvidia.com/deploy/archive/R535/nvml-api/group__nvmlDeviceQueries.html>).
    type Mask = unsafe extern "C" fn(Dev, *mut u64) -> u32;
    // nvmlDeviceGetPerformanceState(nvmlDevice_t, nvmlPstates_t*) and
    // nvmlDeviceGetCurr/MaxPcieLinkGeneration/Width(nvmlDevice_t,
    // unsigned int*) are [`Value`]s (nvmlPstates_t is a C enum, 32 bits).

    const NVML_SUCCESS: u32 = 0;
    const CLOCK_GRAPHICS: u32 = 0;
    const CLOCK_MEM: u32 = 2;
    const TEMPERATURE_GPU: u32 = 0;
    const NVSMI: &str = r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll";

    /// nvmlPciInfo_t (only the PCI ids are read).
    #[repr(C)]
    #[allow(dead_code)]
    struct PciInfo {
        bus_id_legacy: [u8; 16],
        domain: u32,
        bus: u32,
        device: u32,
        /// (device id << 16) | vendor id.
        pci_device_id: u32,
        pci_sub_system_id: u32,
        bus_id: [u8; 32],
    }

    /// A started NVML: the library, its functions and the cards it sees.
    /// Dropped = NVML shut down and `nvml.dll` unloaded.
    pub struct Session {
        lib: HMODULE,
        shutdown: Option<Plain>,
        clock: Option<Typed>,
        max_clock: Option<Typed>,
        temperature: Option<Typed>,
        power: Option<Value>,
        power_limit: Option<Value>,
        fan: Option<Value>,
        utilization: Option<Utilization>,
        encoder: Option<Codec>,
        decoder: Option<Codec>,
        clock_reasons: Result<Mask, String>,
        pstate: Option<Value>,
        pcie_gen: Option<Value>,
        pcie_width: Option<Value>,
        pcie_max_gen: Option<Value>,
        pcie_max_width: Option<Value>,
        pub cards: Vec<(Dev, Device)>,
    }

    /// A function of the library, if it has it.
    unsafe fn sym<T: Copy>(lib: HMODULE, name: &std::ffi::CStr) -> Option<T> {
        GetProcAddress(lib, name.as_ptr().cast()).map(|f| transmute_copy(&f))
    }

    fn load() -> HMODULE {
        // SAFETY: plain loader calls with NUL-terminated wide strings.
        unsafe {
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
            let lib = LoadLibraryExW(wide("nvml.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if !lib.is_null() {
                return lib;
            }
            LoadLibraryExW(wide(NVSMI).as_ptr(), std::ptr::null_mut(), 0)
        }
    }

    impl Session {
        /// Loads `nvml.dll`, starts NVML and finds the NVIDIA cards.
        pub fn open() -> Result<Session, String> {
            let lib = load();
            if lib.is_null() {
                return Err("nvml.dll not found (no NVIDIA driver)".into());
            }
            // SAFETY: the signatures are NVML's (nvml.h); `s` frees the
            // library on every early return.
            unsafe {
                let mut s = Session {
                    lib,
                    shutdown: None,
                    clock: sym(lib, c"nvmlDeviceGetClockInfo"),
                    max_clock: sym(lib, c"nvmlDeviceGetMaxClockInfo"),
                    temperature: sym(lib, c"nvmlDeviceGetTemperature"),
                    power: sym(lib, c"nvmlDeviceGetPowerUsage"),
                    power_limit: sym(lib, c"nvmlDeviceGetEnforcedPowerLimit"),
                    fan: sym(lib, c"nvmlDeviceGetFanSpeed"),
                    utilization: sym(lib, c"nvmlDeviceGetUtilizationRates"),
                    encoder: sym(lib, c"nvmlDeviceGetEncoderUtilization"),
                    decoder: sym(lib, c"nvmlDeviceGetDecoderUtilization"),
                    clock_reasons: sym(lib, c"nvmlDeviceGetCurrentClocksEventReasons")
                        .or_else(|| sym(lib, c"nvmlDeviceGetCurrentClocksThrottleReasons"))
                        .ok_or_else(|| super::missing("nvmlDeviceGetCurrentClocksEventReasons")),
                    pstate: sym(lib, c"nvmlDeviceGetPerformanceState"),
                    pcie_gen: sym(lib, c"nvmlDeviceGetCurrPcieLinkGeneration"),
                    pcie_width: sym(lib, c"nvmlDeviceGetCurrPcieLinkWidth"),
                    pcie_max_gen: sym(lib, c"nvmlDeviceGetMaxPcieLinkGeneration"),
                    pcie_max_width: sym(lib, c"nvmlDeviceGetMaxPcieLinkWidth"),
                    cards: Vec::new(),
                };
                let (Some(init), Some(count), Some(handle)) = (
                    sym::<Plain>(lib, c"nvmlInit_v2"),
                    sym::<Count>(lib, c"nvmlDeviceGetCount_v2"),
                    sym::<Handle>(lib, c"nvmlDeviceGetHandleByIndex_v2"),
                ) else {
                    return Err("nvml.dll is too old".into());
                };
                let code = init();
                if code != NVML_SUCCESS {
                    return Err(format!("NVML did not start (error {code})"));
                }
                s.shutdown = sym(lib, c"nvmlShutdown");
                let (name, pci) = (sym::<Name>(lib, c"nvmlDeviceGetName"), sym::<Pci>(lib, c"nvmlDeviceGetPciInfo_v3"));
                let mut n = 0u32;
                if count(&mut n) != NVML_SUCCESS || n == 0 {
                    return Err("no NVIDIA GPU".into());
                }
                let mut seen = Vec::new();
                let mut handles = Vec::new();
                for i in 0..n {
                    let mut d: Dev = std::ptr::null_mut();
                    if handle(i, &mut d) != NVML_SUCCESS {
                        continue;
                    }
                    let mut buf = [0u8; 96];
                    let name = match name {
                        Some(f) if f(d, buf.as_mut_ptr(), buf.len() as u32) == NVML_SUCCESS => {
                            String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == 0).unwrap_or(buf.len())]).into_owned()
                        }
                        _ => "NVIDIA GPU".into(),
                    };
                    let mut info: PciInfo = std::mem::zeroed();
                    let ids = match pci {
                        Some(f) if f(d, &mut info) == NVML_SUCCESS => info.pci_device_id,
                        _ => 0,
                    };
                    seen.push(Adapter { name, vendor_id: ids & 0xFFFF, device_id: ids >> 16 });
                    handles.push(d);
                }
                let listed = super::super::dxcore::adapters().unwrap_or_default();
                s.cards = handles.into_iter().zip(gpu::match_cards(&listed, &seen, "nvidia")).collect();
                Ok(s)
            }
        }

        /// One read of a card; what NVML refuses stays `None`.
        pub fn sample(&self, d: Dev) -> GpuSample {
            // SAFETY: `d` is a handle of this session; out-pointers are valid.
            unsafe {
                let typed = |f: Option<Typed>, t: u32| {
                    let mut v = 0u32;
                    f.filter(|f| f(d, t, &mut v) == NVML_SUCCESS).map(|_| f64::from(v))
                };
                let value = |f: Option<Value>| {
                    let mut v = 0u32;
                    f.filter(|f| f(d, &mut v) == NVML_SUCCESS).map(|_| f64::from(v))
                };
                let codec = |f: Option<Codec>| {
                    let (mut v, mut period) = (0u32, 0u32);
                    f.filter(|f| f(d, &mut v, &mut period) == NVML_SUCCESS).map(|_| f64::from(v))
                };
                let mut util = [0u32; 2];
                let load = self.utilization.filter(|f| f(d, &mut util) == NVML_SUCCESS).map(|_| f64::from(util[0]));
                GpuSample {
                    graphics_mhz: typed(self.clock, CLOCK_GRAPHICS),
                    graphics_max_mhz: typed(self.max_clock, CLOCK_GRAPHICS),
                    memory_mhz: typed(self.clock, CLOCK_MEM),
                    memory_max_mhz: typed(self.max_clock, CLOCK_MEM),
                    power_w: value(self.power).map(|mw| mw / 1000.0),
                    power_limit_w: value(self.power_limit).map(|mw| mw / 1000.0),
                    temperature_c: typed(self.temperature, TEMPERATURE_GPU),
                    hotspot_c: None,
                    fan_percent: value(self.fan),
                    load_percent: load,
                    encoder_percent: codec(self.encoder),
                    decoder_percent: codec(self.decoder),
                }
            }
        }
    }

    impl Session {
        /// One read of a card's NVIDIA-only states; a function this
        /// driver lacks or a call NVML refuses gives only that reading's
        /// reason.
        pub fn extras(&self, d: Dev) -> super::Extras {
            // SAFETY: `d` is a handle of this session; out-pointers are valid.
            unsafe {
                let value = |f: Option<Value>, name: &str| {
                    let f = f.ok_or_else(|| super::missing(name))?;
                    let mut v = 0u32;
                    match f(d, &mut v) {
                        NVML_SUCCESS => Ok(v),
                        code => Err(super::failed(code)),
                    }
                };
                let clock_reasons = self.clock_reasons.clone().and_then(|f| {
                    let mut mask = 0u64;
                    match f(d, &mut mask) {
                        NVML_SUCCESS => Ok(mask),
                        code => Err(super::failed(code)),
                    }
                });
                let current = value(self.pcie_gen, "nvmlDeviceGetCurrPcieLinkGeneration")
                    .and_then(|g| value(self.pcie_width, "nvmlDeviceGetCurrPcieLinkWidth").map(|w| (g, w)));
                let max = value(self.pcie_max_gen, "nvmlDeviceGetMaxPcieLinkGeneration")
                    .and_then(|g| value(self.pcie_max_width, "nvmlDeviceGetMaxPcieLinkWidth").map(|w| (g, w)))
                    .ok();
                super::Extras {
                    clock_reasons,
                    pstate: value(self.pstate, "nvmlDeviceGetPerformanceState"),
                    pcie: current.map(|c| (c, max)),
                }
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            // SAFETY: shutdown only after a successful init; the library
            // was loaded by `load` and nothing of it is used afterwards.
            unsafe {
                if let Some(shutdown) = self.shutdown {
                    shutdown();
                }
                FreeLibrary(self.lib);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvml_is_listed_only_on_a_pc_with_an_nvidia_card() {
        let card = |vendor_id| gpu::Adapter { name: "GPU".into(), vendor_id, device_id: 1 };
        assert!(!for_a_pc_with(&Ok(vec![card(gpu::INTEL), card(gpu::AMD)])), "Intel and AMD only: no NVML line");
        assert!(for_a_pc_with(&Ok(vec![card(gpu::INTEL), card(gpu::NVIDIA)])));
        assert!(for_a_pc_with(&Err("DXCore failed".into())), "when the cards are unknown, NVML stays listed with its reason");
    }

    #[test]
    fn clock_event_reasons_read_in_words_from_the_bitmask() {
        assert_eq!(clock_event_reasons(0x0), "None");
        assert_eq!(clock_event_reasons(0x1), "Idle");
        assert_eq!(clock_event_reasons(0x4), "Power cap");
        assert_eq!(clock_event_reasons(0x4 | 0x20), "Power cap, Thermal");
        assert_eq!(clock_event_reasons(0x2 | 0x10), "Application setting, Sync boost");
        assert_eq!(clock_event_reasons(0x8 | 0x40 | 0x80 | 0x100), "Hardware slowdown, Hardware thermal, Power brake, Display clock setting");
        assert_eq!(clock_event_reasons(0x1 | 0x200), "Idle, Unknown (0x200)", "bits NVML adds later are not hidden");
    }

    #[test]
    fn the_pcie_link_reads_current_of_maximum() {
        assert_eq!(pcie_link((4, 16), Some((4, 16))), "Gen 4 x16 of Gen 4 x16");
        assert_eq!(pcie_link((1, 8), Some((4, 16))), "Gen 1 x8 of Gen 4 x16", "an idle card drops its link");
        assert_eq!(pcie_link((3, 4), None), "Gen 3 x4", "no maximum: the live link alone");
    }

    #[test]
    fn the_performance_state_reads_p0_to_p15_and_unknown_is_missing() {
        assert_eq!(performance_state(0).as_deref(), Some("P0"));
        assert_eq!(performance_state(8).as_deref(), Some("P8"));
        assert_eq!(performance_state(12).as_deref(), Some("P12"));
        assert_eq!(performance_state(15).as_deref(), Some("P15"));
        assert_eq!(performance_state(32), None, "NVML_PSTATE_UNKNOWN");
    }

    fn rtx() -> crate::sensors::model::Device {
        crate::sensors::model::Device::gpu("gpu0", "NVIDIA GeForce RTX 4080")
    }

    fn row<'a>(frame: &'a crate::sensors::model::Frame, name: &str) -> &'a Reading {
        frame.readings().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}"))
    }

    #[test]
    fn an_nvidia_card_shows_clock_limit_reasons_performance_state_and_pcie_link_read_from_nvml() {
        use crate::sensors::model::{Frame, Group};
        let extras = Extras { clock_reasons: Ok(0x4 | 0x20), pstate: Ok(2), pcie: Ok(((4, 16), Some((4, 16)))) };
        let frame = Frame::build(0, &[extras.readings(&rtx(), Instant::now())], Vec::new());
        let reasons = row(&frame, "Clock Limit Reasons");
        assert_eq!((reasons.text(), reasons.group, reasons.source), ("Power cap, Thermal".to_string(), Group::Clocks, SOURCE));
        assert_eq!(reasons.id, "gpu.gpu0.clock.limit_reasons");
        let pstate = row(&frame, "Performance State");
        assert_eq!((pstate.text(), pstate.group, pstate.source), ("P2".to_string(), Group::General, SOURCE));
        let pcie = row(&frame, "PCIe Link");
        assert_eq!((pcie.text(), pcie.group, pcie.source), ("Gen 4 x16 of Gen 4 x16".to_string(), Group::General, SOURCE));
        assert!(frame.readings().all(|r| r.kind == crate::sensors::model::Kind::Counter), "states: no Min/Max");
    }

    #[test]
    fn a_reading_the_driver_cannot_give_is_grey_with_its_reason_and_the_others_stay() {
        use crate::sensors::model::Frame;
        let extras = Extras {
            clock_reasons: Err(missing("nvmlDeviceGetCurrentClocksEventReasons")),
            pstate: Ok(32),
            pcie: Err(failed(3)),
        };
        let frame = Frame::build(0, &[extras.readings(&rtx(), Instant::now())], Vec::new());
        let reasons = row(&frame, "Clock Limit Reasons");
        assert_eq!((reasons.value, reasons.text()), (None, "this NVIDIA driver has no nvmlDeviceGetCurrentClocksEventReasons".to_string()));
        assert_eq!(row(&frame, "Performance State").text(), "NVML reports no performance state");
        assert_eq!(row(&frame, "PCIe Link").text(), "not supported on this card");
        assert_eq!(failed(15), "NVML could not read it (error 15)");
        assert!(frame.readings().all(|r| r.source == SOURCE), "grey rows still say where they were read from");
    }

    #[test]
    fn the_existing_nvml_readings_still_hold_next_to_the_new_ones() {
        use crate::sensors::gpu::{self, GpuSample};
        use crate::sensors::model::Frame;
        let at = Instant::now();
        let sample = GpuSample { graphics_mhz: Some(2730.0), memory_mhz: Some(11201.0), power_w: Some(285.26), temperature_c: Some(61.0), ..Default::default() };
        let extras = Extras { clock_reasons: Ok(0x1), pstate: Ok(8), pcie: Ok(((1, 16), Some((4, 16)))) };
        let mut readings = sample.readings(&rtx(), SOURCE, NOT_REPORTED, at);
        readings.extend(extras.readings(&rtx(), at));
        let frame = Frame::build(0, &[readings], Vec::new());
        assert_eq!(frame.devices.len(), 1, "one heading per card");
        assert_eq!(gpu::summary(&frame.devices[0]), "Clock 2.73 GHz   Power 285.3 W   Memory 11.20 GHz");
        assert_eq!(row(&frame, "Graphics Clock").text(), "2.73 GHz");
        assert_eq!(row(&frame, "GPU Temperature").text(), "61.0 °C");
        assert_eq!(row(&frame, "Hot Spot").text(), NOT_REPORTED);
        assert_eq!(row(&frame, "Clock Limit Reasons").text(), "Idle");
        let groups: Vec<_> = frame.devices[0].groups.iter().map(|g| g.group.name()).collect();
        assert_eq!(groups, vec!["Temperatures", "Utilization", "Clocks", "Powers", "General", "Fans"]);
    }

    #[test]
    #[ignore = "reads this PC's NVIDIA card"]
    #[cfg(windows)]
    fn reads_this_pc_and_shuts_nvml_down_when_dropped() {
        use super::*;
        use crate::sensors::gpu;
        use crate::sensors::model::Frame;
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let module = || {
            let name: Vec<u16> = "nvml.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated name; no reference is taken.
            unsafe { GetModuleHandleW(name.as_ptr()) }
        };
        let loaded = || !module().is_null();
        assert!(!loaded(), "nothing loads nvml.dll before the provider");
        let mut p = Nvml::new();
        assert_eq!(p.availability(), Ok(()));
        assert!(loaded());
        let frame = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        for d in &frame.devices {
            println!("{}   {}", d.device.name, gpu::summary(d));
            for g in &d.groups {
                println!("  {}", g.group.name());
                for r in &g.readings {
                    println!("    {:<22} {}", r.name, r.text());
                }
            }
        }
        let card = frame.devices.iter().find(|d| d.device.name.contains("RTX 4080")).expect("the RTX 4080");
        assert_eq!(card.device.key, "gpu0", "matched to DXCore's card");
        let value = |what: &str| frame.reading(&gpu::metric(&card.device, what)).and_then(|r| r.value);
        assert!(value(gpu::GRAPHICS_CLOCK).unwrap() > 0.0);
        assert!(value(gpu::MEMORY_CLOCK).unwrap() > 0.0);
        assert!(value(gpu::BOARD_POWER).unwrap() > 0.0);
        assert!(value("temperature.gpu_c").unwrap() > 0.0);
        for what in ["clock.limit_reasons", "performance_state", "pcie.link"] {
            let r = frame.reading(&gpu::metric(&card.device, what)).expect("the new NVML reading");
            println!("NEW {:<22} {}", r.name, r.text());
            assert!(r.value.is_some(), "{} read on the RTX 4080: {}", r.name, r.text());
        }
        drop(p);
        // nvmlInit pins nvml.dll in the process (FreeLibrary cannot unmap
        // it), so what dropping guarantees is that NVML is shut down: it
        // answers NVML_ERROR_UNINITIALIZED and runs no work of its own.
        if loaded() {
            // SAFETY: nvmlDeviceGetCount_v2(unsigned int*) of the pinned module.
            let count: unsafe extern "C" fn(*mut u32) -> u32 =
                unsafe { std::mem::transmute(GetProcAddress(module(), c"nvmlDeviceGetCount_v2".as_ptr().cast()).unwrap()) };
            let mut n = 0;
            assert_eq!(unsafe { count(&mut n) }, 1, "NVML shut down when the provider is dropped");
        }
    }
}
