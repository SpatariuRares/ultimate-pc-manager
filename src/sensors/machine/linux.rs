//! Linux: the machine's fixed facts. The processor's are read; the memory
//! modules and the disks are not read here yet (their tickets add them).

use crate::fs_root::FsRoot;
use crate::perfdata::{DiskInfo, StaticInfo};

/// Everything this PC's fixed facts say. Memory and disks stay blank here.
pub fn static_info() -> StaticInfo {
    StaticInfo { cpu: crate::cpu_topology::cpu_facts(&FsRoot::system()), ..StaticInfo::default() }
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
