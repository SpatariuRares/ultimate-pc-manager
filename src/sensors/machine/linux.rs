//! Linux: the machine's fixed facts are not read yet; every read gives none.

use crate::perfdata::{DiskInfo, StaticInfo};

/// Everything this PC's fixed facts say: blank, not available yet.
pub fn static_info() -> StaticInfo {
    StaticInfo::default()
}

/// The processor's rated clock in MHz: 0, not available yet.
pub fn base_mhz() -> u32 {
    0
}

/// The physical disks: none, not available yet.
pub fn disk_info() -> Vec<DiskInfo> {
    Vec::new()
}

/// The raw SMBIOS table: none, not available yet.
pub fn smbios_table() -> Option<Vec<u8>> {
    None
}
