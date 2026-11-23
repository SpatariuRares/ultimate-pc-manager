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
#[allow(dead_code)] // the Windows reader and the tests use it
const NOT_REPORTED: &str = "NVML does not report it";

/// The provider: a started NVML session, or why there is none.
pub struct Nvml {
    reader: imp::Reader,
}

impl Nvml {
    pub fn new() -> Self {
        Nvml { reader: imp::Reader::open() }
    }
}

/// Whether a PC whose cards are `listed` (by DXCore) is one NVML is for: it
/// has an NVIDIA card, or its cards are unknown.
#[cfg_attr(target_os = "linux", allow(dead_code))] // Linux asks the kernel's card list instead
pub fn for_a_pc_with(listed: &Result<Vec<gpu::Adapter>, String>) -> bool {
    listed.as_ref().map_or(true, |cards| cards.iter().any(|a| a.vendor_id == gpu::NVIDIA))
}

impl Provider for Nvml {
    fn name(&self) -> &'static str {
        SOURCE
    }

    /// Only a PC with an NVIDIA card lists NVML.
    fn for_this_pc(&mut self) -> bool {
        // Linux has no DXCore: the kernel's list of video cards answers.
        #[cfg(target_os = "linux")]
        return imp::has_card(&crate::fs_root::FsRoot::system());
        #[cfg(not(target_os = "linux"))]
        for_a_pc_with(&super::dxcore::adapters())
    }

    fn availability(&mut self) -> Result<(), String> {
        self.reader.availability()
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        self.reader.read(at)
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
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

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
