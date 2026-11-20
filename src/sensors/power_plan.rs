//! The power plan provider ("Windows power plan"): the active plan, its
//! minimum / maximum processor state and energy performance preference
//! (EPP), and the Windows power mode, read with the power plan API
//! (`PowerGetActiveScheme`, `PowerRead*ValueIndex`, `PowerReadFriendlyName`,
//! `PowerGetEffectiveOverlayScheme`). Read only: nothing is ever changed,
//! and `powercfg` is never launched.

use std::time::Instant;

use super::model::{Device, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Windows power plan";
/// The metric of the active plan's name ("Governor").
pub const GOVERNOR: &str = "cpu.power.governor";
/// The metric of the Windows power mode ("Power Preference").
pub const PREFERENCE: &str = "cpu.power.preference";
/// What decides the clocks on Windows.
pub const FREQUENCY_DRIVER: &str = "Windows processor power management";
const NO_PLAN: &str = "no active power plan";

/// The name of a Windows power mode (Settings › Power › Power mode) from
/// its overlay scheme GUID, as one number.
pub fn power_mode(overlay: u128) -> &'static str {
    match overlay {
        0x961cc777_2547_4f9d_8174_7d86181b8a7a => "Best power efficiency",
        0 => "Balanced",
        0x3af9b8d9_7c97_431d_ad78_34a8bfea439f => "Better performance",
        0xded574b5_45a0_4f42_8737_46345c09c238 => "Best performance",
        _ => "Custom",
    }
}

/// One read of the plan's raw values (those of the current power source).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlanSample {
    /// The CPU's name, as the CPU provider shows it.
    pub cpu: String,
    /// The active plan's name ("Balanced"), shown as Governor.
    pub plan: Option<String>,
    /// The power mode overlay ("Best performance"), shown as Power Preference.
    pub mode: Option<String>,
    /// Minimum processor state, %.
    pub min_state: Option<u32>,
    /// Maximum processor state, %.
    pub max_state: Option<u32>,
    /// Energy Performance Preference, % (0 = performance, 100 = saving).
    pub epp: Option<u32>,
}

impl PlanSample {
    /// The readings of this sample: CPU › General and Levels (Energy Perf).
    pub fn readings(&self, at: Instant) -> Vec<Reading> {
        let cpu = Device::cpu(&self.cpu);
        let words = |id: &str, name: &str, value: Option<String>| Reading::state(id, cpu.clone(), Group::General, name, SOURCE, at).with_text(value, NO_PLAN);
        let level = |id: &str, name: &str, value: Option<u32>| {
            Reading::measure(id, cpu.clone(), Group::Levels, name, Unit::Percent, SOURCE, at).with(value.map(f64::from), NO_PLAN).counter()
        };
        vec![
            words("cpu.power.frequency_driver", "Frequency Driver", Some(FREQUENCY_DRIVER.into())),
            words(GOVERNOR, "Governor", self.plan.clone()),
            words(PREFERENCE, "Power Preference", self.mode.clone()),
            level("cpu.power.min_state_percent", "Minimum Processor State", self.min_state),
            level("cpu.power.max_state_percent", "Maximum Processor State", self.max_state),
            level("cpu.power.epp_percent", "Energy Performance Preference", self.epp),
        ]
    }
}

/// The power plan provider. Holds nothing: every read asks Windows again.
pub struct PowerPlan {
    cpu: String,
}

impl PowerPlan {
    pub fn new() -> Self {
        PowerPlan { cpu: super::cpu::cpu_name() }
    }
}

impl Provider for PowerPlan {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        return Ok(());
        #[cfg(not(windows))]
        Err("Windows only".into())
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        #[cfg(windows)]
        let s = imp::sample(&self.cpu);
        #[cfg(not(windows))]
        let s = PlanSample { cpu: self.cpu.clone(), ..Default::default() };
        s.readings(at)
    }
}

/// The active plan's name (`None` if Windows gives none). Core pages use it
/// too (Performance › CPU).
pub fn active_plan_name() -> Option<String> {
    #[cfg(windows)]
    return imp::plan_name();
    #[cfg(not(windows))]
    None
}

/// The current Windows power mode's name (`None` if unknown).
pub fn current_power_mode() -> Option<String> {
    #[cfg(windows)]
    return imp::overlay().map(|g| power_mode(g).to_string());
    #[cfg(not(windows))]
    None
}

/// A provider for tests: the "Ultimate Performance" plan, Best performance
/// mode, states 5 % – 100 %, EPP 0 %, on the made-up "Test CPU".
#[cfg(test)]
pub fn made_up() -> super::provider::Factory {
    struct MadeUp;
    impl Provider for MadeUp {
        fn name(&self) -> &'static str {
            SOURCE
        }
        fn read(&mut self, at: Instant) -> Vec<Reading> {
            let s = PlanSample {
                cpu: "Test CPU".into(),
                plan: Some("Ultimate Performance".into()),
                mode: Some("Best performance".into()),
                min_state: Some(5),
                max_state: Some(100),
                epp: Some(0),
            };
            s.readings(at)
        }
    }
    std::sync::Arc::new(|| Box::new(MadeUp))
}

#[cfg(windows)]
mod imp {
    use super::PlanSample;
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::System::Power::{GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex, PowerReadFriendlyName, SYSTEM_POWER_STATUS};

    const PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
    const MIN_STATE: GUID = GUID::from_u128(0x893dee8e_2bef_41e0_89c6_b55d0929964c);
    const MAX_STATE: GUID = GUID::from_u128(0xbc5038f7_23e0_4960_96da_33abaf5935ec);
    const EPP: GUID = GUID::from_u128(0x36687f9e_e3a5_4dbf_b1dc_15eb381c6863);

    /// The active scheme's GUID.
    fn active() -> Option<GUID> {
        // SAFETY: Windows allocates the GUID with LocalAlloc; it is copied
        // out and freed once.
        unsafe {
            let mut p: *mut GUID = null_mut();
            if PowerGetActiveScheme(null_mut(), &mut p) != 0 || p.is_null() {
                return None;
            }
            let g = *p;
            LocalFree(p.cast());
            Some(g)
        }
    }

    fn name_of(scheme: &GUID) -> Option<String> {
        // SAFETY: the first call asks the size, the second fills a buffer of it.
        unsafe {
            let mut size = 0u32;
            if PowerReadFriendlyName(null_mut(), scheme, null(), null(), null_mut(), &mut size) != 0 || size < 2 {
                return None;
            }
            let mut buf = vec![0u16; size as usize / 2];
            if PowerReadFriendlyName(null_mut(), scheme, null(), null(), buf.as_mut_ptr().cast(), &mut size) != 0 {
                return None;
            }
            let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
            Some(String::from_utf16_lossy(&buf[..end]).trim().to_string()).filter(|s| !s.is_empty())
        }
    }

    pub fn plan_name() -> Option<String> {
        name_of(&active()?)
    }

    /// On battery the plan's DC values apply.
    fn on_battery() -> bool {
        // SAFETY: plain out-parameter.
        unsafe {
            let mut s: SYSTEM_POWER_STATUS = zeroed();
            GetSystemPowerStatus(&mut s) != 0 && s.ACLineStatus == 0
        }
    }

    fn value(scheme: &GUID, setting: &GUID, battery: bool) -> Option<u32> {
        let mut v = 0u32;
        // SAFETY: GUID pointers outlive the call; plain out-parameter.
        let r = unsafe {
            if battery {
                PowerReadDCValueIndex(null_mut(), scheme, &PROCESSOR, setting, &mut v)
            } else {
                PowerReadACValueIndex(null_mut(), scheme, &PROCESSOR, setting, &mut v)
            }
        };
        (r == 0).then_some(v)
    }

    /// The effective power mode overlay, as one number (loaded at run time:
    /// not every Windows has it).
    pub fn overlay() -> Option<u128> {
        use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        // SAFETY: the function has this signature since Windows 10 1709.
        unsafe {
            let lib: Vec<u16> = "powrprof.dll\0".encode_utf16().collect();
            let h = LoadLibraryW(lib.as_ptr());
            if h.is_null() {
                return None;
            }
            let f = GetProcAddress(h, b"PowerGetEffectiveOverlayScheme\0".as_ptr())?;
            let f: unsafe extern "system" fn(*mut GUID) -> u32 = std::mem::transmute(f);
            let mut g: GUID = zeroed();
            if f(&mut g) != 0 {
                return None;
            }
            Some(((g.data1 as u128) << 96) | ((g.data2 as u128) << 80) | ((g.data3 as u128) << 64) | u64::from_be_bytes(g.data4) as u128)
        }
    }

    pub fn sample(cpu: &str) -> PlanSample {
        let mut s = PlanSample { cpu: cpu.into(), mode: super::current_power_mode(), ..Default::default() };
        let Some(scheme) = active() else { return s };
        let battery = on_battery();
        s.plan = name_of(&scheme);
        s.min_state = value(&scheme, &MIN_STATE, battery);
        s.max_state = value(&scheme, &MAX_STATE, battery);
        s.epp = value(&scheme, &EPP, battery);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Frame, Kind};

    fn ultimate() -> PlanSample {
        PlanSample { cpu: "CPU".into(), plan: Some("Ultimate Performance".into()), mode: Some("Balanced".into()), min_state: Some(5), max_state: Some(100), epp: Some(33) }
    }

    #[test]
    fn the_plan_fills_general_and_levels_of_the_cpu() {
        let frame = Frame::build(1, &[ultimate().readings(Instant::now())], Vec::new());
        let text = |id: &str| frame.reading(id).unwrap().text();
        assert_eq!(text(GOVERNOR), "Ultimate Performance");
        assert_eq!(text(PREFERENCE), "Balanced");
        assert_eq!(text("cpu.power.frequency_driver"), "Windows processor power management");
        assert_eq!(text("cpu.power.min_state_percent"), "5 %");
        assert_eq!(text("cpu.power.max_state_percent"), "100 %");
        assert_eq!(text("cpu.power.epp_percent"), "33 %");
        let groups: Vec<_> = frame.devices[0].groups.iter().map(|g| g.group.name()).collect();
        assert_eq!(groups, vec!["Levels (Energy Perf)", "General"]);
        assert!(frame.readings().all(|r| r.kind == Kind::Counter), "settings have no Min/Max");
    }

    #[test]
    fn without_an_active_plan_its_rows_say_why() {
        let s = PlanSample { cpu: "CPU".into(), ..Default::default() };
        let frame = Frame::build(1, &[s.readings(Instant::now())], Vec::new());
        assert_eq!(frame.reading(GOVERNOR).unwrap().value, None);
        assert_eq!(frame.reading("cpu.power.epp_percent").unwrap().text(), "no active power plan");
    }

    #[test]
    fn power_modes_are_named_like_windows_settings() {
        assert_eq!(power_mode(0), "Balanced");
        assert_eq!(power_mode(0xded574b5_45a0_4f42_8737_46345c09c238), "Best performance");
        assert_eq!(power_mode(0x961cc777_2547_4f9d_8174_7d86181b8a7a), "Best power efficiency");
        assert_eq!(power_mode(1), "Custom");
    }

    /// Reads the plan of this PC (expected here: Ultimate Performance).
    #[test]
    #[ignore]
    fn reads_the_plan_of_this_pc() {
        let readings = PowerPlan::new().read(Instant::now());
        for r in &readings {
            println!("{:<32} {}", r.name, r.text());
        }
        let frame = Frame::build(1, &[readings], Vec::new());
        assert!(frame.reading(GOVERNOR).and_then(|r| r.value).is_some(), "an active plan");
        assert!(frame.reading("cpu.power.max_state_percent").and_then(|r| r.value).is_some());
    }
}
