//! Linux: the machine's fixed facts. The processor's and the disks' are read;
//! the memory modules are not read here yet (their ticket adds them).

use crate::fs_root::FsRoot;
use crate::perfdata::{DiskInfo, StaticInfo};

/// Everything this PC's fixed facts say. Memory stays blank here.
pub fn static_info() -> StaticInfo {
    StaticInfo { cpu: crate::cpu_topology::cpu_facts(&FsRoot::system()), disks: disk_info(), ..StaticInfo::default() }
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
}
