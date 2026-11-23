//! Linux: process facts are not available yet; every read says so.

use super::Counts;
use std::collections::HashMap;

/// Not available yet.
pub fn counts(_buf: &mut Vec<u64>) -> HashMap<u32, Counts> {
    HashMap::new()
}

/// Not available yet.
pub fn thread_ids(_pid: u32) -> Option<Vec<u32>> {
    None
}

/// Not available yet.
pub fn identity(_pid: u32) -> (&'static str, u16) {
    ("", 0)
}

/// Not available yet.
pub fn priority_class(_pid: u32) -> Option<&'static str> {
    None
}

/// Not available yet.
pub fn windows() -> super::Windows {
    Default::default()
}

/// Not available yet.
pub fn bring_to_front(_pid: u32) -> bool {
    false
}

/// Not available yet.
pub fn command_line_and_environment(_pid: u32) -> super::CommandAndEnvironment {
    (Err(super::Missing::Unavailable), Err(super::Missing::Unavailable))
}

/// Not available yet.
pub fn open_resources(_pid: u32, _each: std::time::Duration) -> Result<Vec<super::Resource>, super::Missing> {
    Err(super::Missing::Unavailable)
}

/// The NT names of the drives: none, not available yet.
#[allow(dead_code)] // read by the Windows trace
pub fn drive_devices() -> Vec<(String, String)> {
    Vec::new()
}
