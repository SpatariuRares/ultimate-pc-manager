//! Linux: the machine's fixed facts. The processor's, the memory modules' and
//! the disks' are read.
//!
//! The memory modules come from the firmware's per-entry files under
//! `/sys/firmware/dmi/entries/17-*/raw` (one SMBIOS "memory device" each).
//! The kernel makes them readable by root only on most systems, so without
//! root nothing is read and the page shows the facts as unknown, never as
//! empty fields.

use crate::fs_root::FsRoot;
use crate::perfdata::{DiskInfo, MemInfo, StaticInfo};

/// Everything this PC's fixed facts say.
pub fn static_info() -> StaticInfo {
    static_info_at(&FsRoot::system())
}

fn static_info_at(root: &FsRoot) -> StaticInfo {
    StaticInfo { cpu: crate::cpu_topology::cpu_facts(root), mem: memory_modules(root), disks: disks_of(root), ..StaticInfo::default() }
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

/// The physical disks, with their model, type (NVMe, SSD or HDD) and size.
pub fn disk_info() -> Vec<DiskInfo> {
    disks_of(&FsRoot::system())
}

fn disks_of(root: &FsRoot) -> Vec<DiskInfo> {
    crate::linux_disks::physical_disks(root).into_iter().map(|d| DiskInfo { number: d.number, model: d.model, bus: d.kind, capacity: d.capacity, ..DiskInfo::default() }).collect()
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
    fn the_disk_facts_are_the_models_types_and_sizes_of_the_physical_disks() {
        let disks = disks_of(&fixture("disks"));
        let shown: Vec<(u32, &str, &str, u64)> = disks.iter().map(|d| (d.number, d.model.as_str(), d.bus, d.capacity)).collect();
        assert_eq!(shown[0], (0, "Samsung SSD 990 PRO 1TB", "NVMe", 1_000_215_216 * 512));
        assert_eq!(shown[1].2, "SSD");
        assert_eq!(shown[2].2, "HDD");
        assert_eq!(shown.len(), 3);
        assert!(disks_of(&fixture("desktop")).is_empty());
    }

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

    #[test]
    fn one_read_fills_the_processor_the_memory_and_the_disks() {
        let info = static_info_at(&fixture("disks"));
        assert_eq!(info.disks.len(), 3);
        assert_eq!(info.mem, MemInfo::default(), "no firmware entries in that root");
        assert!(static_info_at(&fixture("memory")).disks.is_empty());
    }
}
