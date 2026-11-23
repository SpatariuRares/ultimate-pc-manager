//! The machine's fixed facts (processor topology, memory modules, disks,
//! video cards) and the firmware table, read from Windows. They belong to
//! the sensor providers: [`MachineFacts`] reports them through the frame,
//! and the disk, firmware and processor providers take what they need from
//! here at construction, so nothing reads them from a global.

use std::sync::Arc;
use std::time::Instant;

use super::model::Reading;
use super::provider::Provider;
use crate::availability::{is_available, Feature};
use crate::perfdata::{CpuInfo, DiskInfo, GpuInfo, MemInfo, StaticInfo};

/// Everything this PC's fixed facts say, read now.
pub fn read() -> StaticInfo {
    imp::static_info()
}

/// The processor's rated clock in MHz (0 if Windows does not say).
pub fn base_mhz() -> u32 {
    imp::base_mhz()
}

/// The physical disks (model, bus, drive letters), read now.
pub fn disks() -> Vec<DiskInfo> {
    imp::disk_info()
}

/// The raw SMBIOS table (`RawSMBIOSData`, from
/// `GetSystemFirmwareTable('RSMB')`), if the system gives it.
pub fn smbios_table() -> Option<Vec<u8>> {
    imp::smbios_table()
}

/// Its name in "Sources on this PC".
const SOURCE: &str = "Machine information";

/// Reports the machine's fixed facts, read once (they do not change while
/// Windows runs), when the service asks for them after its first read. A
/// counter provider, so the core pages have them without anything turning
/// the hardware sensors on.
pub struct MachineFacts {
    facts: Option<Arc<StaticInfo>>,
}

impl MachineFacts {
    pub fn new() -> Self {
        MachineFacts { facts: None }
    }
}

impl Provider for MachineFacts {
    fn name(&self) -> &'static str {
        SOURCE
    }

    /// It has no readings, so "Sources on this PC" does not list it.
    fn listed(&self) -> bool {
        false
    }

    fn read(&mut self, _at: Instant) -> Vec<Reading> {
        Vec::new()
    }

    fn machine(&mut self) -> Option<Arc<StaticInfo>> {
        // Blank facts would read as a machine with nothing in it.
        if !is_available(Feature::MachineInformation) {
            return None;
        }
        Some(self.facts.get_or_insert_with(|| Arc::new(read())).clone())
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
    use crate::availability::unavailable_in_test;

    #[test]
    fn the_machine_facts_provider_is_not_a_source_of_any_figure() {
        assert!(!MachineFacts::new().listed(), "it reports no readings, so \"Sources on this PC\" does not list it");
    }

    #[test]
    fn an_unavailable_static_information_is_not_reported_as_facts() {
        let _unavailable = unavailable_in_test(&[Feature::MachineInformation]);
        assert!(MachineFacts::new().machine().is_none(), "no blank facts: the page leaves them out");
    }

    /// Every source file, with its path under `src/`.
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    #[test]
    fn nothing_reads_the_global_machine_information() {
        for (file, src) in sources() {
            if file.ends_with("machine.rs") {
                continue; // this test names what it forbids
            }
            for word in [concat!("perfdata::", "info()"), concat!("perfdata::", "smbios_table"), concat!("perfdata::", "power_plan")] {
                assert!(!src.contains(word), "{file} still reads {word}");
            }
        }
    }
}
