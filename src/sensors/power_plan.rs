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
        imp::availability()
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        imp::sample(&self.cpu).readings(at)
    }
}

/// The active plan's name (`None` if Windows gives none). Core pages use it
/// too (Performance › CPU).
pub fn active_plan_name() -> Option<String> {
    imp::plan_name()
}

/// The current Windows power mode's name (`None` if unknown).
pub fn current_power_mode() -> Option<String> {
    imp::overlay().map(|g| power_mode(g).to_string())
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
