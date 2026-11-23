//! The Intel Dynamic Tuning provider ("Intel DTT"): on Intel processors
//! with Intel Dynamic Tuning installed and running, the CPU package
//! temperature DTT manages.
//!
//! Intel publishes no API for DTT to other programs, and the Windows
//! thermal-zone counters it drives are the ACPI zones the Windows power
//! status provider already lists. So this build does not read DTT: its row
//! is grey with [`NOT_SUPPORTED`], as the AMD SDK's are, and no ACPI zone
//! is shown again under another name. Nothing is opened.

use std::time::Instant;

use super::model::{Device, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Intel DTT";
/// The Windows service of Intel Dynamic Tuning (ESIF upper framework).
pub const SERVICE: &str = "esifsvc";
/// Why DTT's row has no value in this build.
pub const NOT_SUPPORTED: &str = "reading Intel DTT is not supported yet";

/// Whether DTT runs here: `vendor` is the CPU's vendor id
/// ("GenuineIntel"), `service_running` is `None` when DTT is not installed.
pub fn availability(vendor: &str, service_running: Option<bool>) -> Result<(), String> {
    match (vendor == "GenuineIntel", service_running) {
        (false, _) => Err("Intel processors only".into()),
        (true, None) => Err("Intel Dynamic Tuning is not installed".into()),
        (true, Some(false)) => Err("Intel Dynamic Tuning is not running".into()),
        (true, Some(true)) => Ok(()),
    }
}

/// The provider: [`Provider::availability`] checks that DTT runs on an
/// Intel processor; its one row stays grey ([`NOT_SUPPORTED`]).
pub struct IntelDtt {
    cpu_name: String,
}

impl IntelDtt {
    pub fn new() -> Self {
        IntelDtt { cpu_name: super::cpu::cpu_name() }
    }
}

impl Provider for IntelDtt {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        imp::provider_availability()
    }

    /// Only a PC with an Intel processor lists Intel DTT.
    fn for_this_pc(&mut self) -> bool {
        imp::for_this_pc()
    }

    /// CPU › Temperatures › Package, grey with [`NOT_SUPPORTED`].
    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let cpu = Device::cpu(&self.cpu_name);
        vec![Reading::measure(super::cpu::PACKAGE_TEMPERATURE, cpu, Group::Temperatures, "Package", Unit::Celsius, SOURCE, at).with(None, NOT_SUPPORTED)]
    }
}

/// Whether a processor of `vendor` ("GenuineIntel") is one DTT is for.
pub fn for_a_cpu_from(vendor: &str) -> bool {
    vendor == "GenuineIntel"
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
    use crate::sensors::cpu::PACKAGE_TEMPERATURE;
    use crate::sensors::model::{DeviceKind, Frame};

    #[test]
    fn intel_dtt_is_listed_only_on_an_intel_processor() {
        assert!(!for_a_cpu_from("AuthenticAMD"), "a Ryzen PC has no Intel DTT line");
        assert!(for_a_cpu_from("GenuineIntel"));
    }

    #[test]
    fn only_intel_processors_with_dynamic_tuning_running_are_read() {
        assert_eq!(availability("AuthenticAMD", Some(true)), Err("Intel processors only".into()));
        assert_eq!(availability("GenuineIntel", None), Err("Intel Dynamic Tuning is not installed".into()));
        assert_eq!(availability("GenuineIntel", Some(false)), Err("Intel Dynamic Tuning is not running".into()));
        assert_eq!(availability("GenuineIntel", Some(true)), Ok(()));
    }

    #[test]
    fn with_dynamic_tuning_running_its_row_is_grey_until_reading_it_is_supported() {
        let mut p = IntelDtt { cpu_name: "Intel Core i7-1360P".into() };
        let frame = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        let rows: Vec<_> = frame.readings().collect();
        assert_eq!(rows.len(), 1, "no copy of the ACPI thermal zones");
        let pkg = rows[0];
        assert_eq!((pkg.id.as_str(), pkg.name.as_str(), pkg.group), (PACKAGE_TEMPERATURE, "Package", Group::Temperatures));
        assert_eq!(pkg.device, Device::cpu("Intel Core i7-1360P"));
        assert_eq!((pkg.value, pkg.text()), (None, NOT_SUPPORTED.to_string()));
        assert_eq!(NOT_SUPPORTED, "reading Intel DTT is not supported yet");
        assert!(rows.iter().all(|r| r.device.kind != DeviceKind::ThisPc));
    }

    /// On this PC: says why DTT is not read (AMD here: "Intel processors only").
    #[test]
    #[ignore]
    fn reads_this_pc_intel_dtt() {
        let mut p = IntelDtt::new();
        println!("availability: {:?}", p.availability());
        for r in p.read(Instant::now()) {
            println!("{:<10} {:<14} {}", r.device.name, r.name, r.text());
        }
    }
}
