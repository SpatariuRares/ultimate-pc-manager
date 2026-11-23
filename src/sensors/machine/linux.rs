//! Linux: the machine's fixed facts. The processor's and the memory
//! modules' are read; the disks are not read here yet (their ticket adds them).
//!
//! The memory modules come from the firmware's per-entry files under
//! `/sys/firmware/dmi/entries/17-*/raw` (one SMBIOS "memory device" each).
//! The kernel makes them readable by root only on most systems, so without
//! root nothing is read and the page shows the facts as unknown, never as
//! empty fields.

use crate::fs_root::FsRoot;
use crate::perfdata::{DiskInfo, MemInfo, StaticInfo};

/// Everything this PC's fixed facts say. The disks stay blank here.
pub fn static_info() -> StaticInfo {
    static_info_at(&FsRoot::system())
}

fn static_info_at(root: &FsRoot) -> StaticInfo {
    StaticInfo { cpu: crate::cpu_topology::cpu_facts(root), mem: memory_modules(root), ..StaticInfo::default() }
}

/// The memory modules the firmware entries say (speed, type, form factor and
/// slots); unknown ([`MemInfo::default`]) when none can be read.
fn memory_modules(root: &FsRoot) -> MemInfo {
    let mut table = Vec::new();
    for entry in root.list_dir("sys/firmware/dmi/entries").iter().filter(|n| n.starts_with("17-")) {
        if let Ok(raw) = std::fs::read(root.path(&format!("sys/firmware/dmi/entries/{entry}/raw"))) {
            table.extend(raw);
        }
    }
    crate::sensors::smbios::memory_facts(&table)
}

/// The processor's rated clock in MHz; 0 when unknown.
pub fn base_mhz() -> u32 {
    crate::cpu_topology::cpu_facts(&FsRoot::system()).base_mhz
}

/// The physical disks: none, not available yet.
pub fn disk_info() -> Vec<DiskInfo> {
    Vec::new()
}

/// The raw SMBIOS table: none, not available yet.
pub fn smbios_table() -> Option<Vec<u8>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    #[test]
    fn readable_firmware_entries_give_the_module_details() {
        let m = static_info_at(&fixture("memory")).mem;
        assert_eq!(m.slots, 3, "every memory device entry is a slot");
        assert_eq!(m.used_slots, 2, "the entry of size 0 is an empty slot");
        assert_eq!((m.kind, m.form), (Some("DDR5"), Some("SODIMM")));
        assert_eq!(m.speed_mts, Some(5200), "the fastest speed the modules run at");
    }

    #[test]
    fn firmware_entries_that_cannot_be_read_leave_the_module_details_unknown() {
        // The entries are there, their `raw` files are not readable (without root).
        assert_eq!(static_info_at(&fixture("memory-bare")).mem, MemInfo::default());
        // No firmware entries at all.
        assert_eq!(static_info_at(&fixture("desktop")).mem, MemInfo::default());
    }

    #[test]
    fn the_processor_facts_are_still_read() {
        assert!(static_info_at(&fixture("cpu-modern")).cpu.cores > 0);
    }
}
