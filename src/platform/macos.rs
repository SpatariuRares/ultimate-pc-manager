//! macOS: stand-ins until this system has its own implementation: every
//! launch fails, no dialog.

use super::*;
use crate::availability::{unavailable_error, Feature};
/// PowerShell is not available on this system yet.
pub fn run_script(_: &str, _: &[(&str, &str)], _: bool) -> Result<serde_json::Value, String> {
    Err(unavailable_error(Feature::PowerShell))
}
/// PowerShell is not available on this system yet.
pub fn run_script_until(_: &str, _: &[(&str, &str)], _: &AtomicBool) -> Result<serde_json::Value, String> {
    Err(unavailable_error(Feature::PowerShell))
}
/// winget is not available on this system yet.
pub fn winget(_: &[&str]) -> Result<(i32, String), String> {
    Err(unavailable_error(Feature::Winget))
}
/// Running programs is not available on this system yet.
pub fn run(_: &str, _: &[&str], _: &std::path::Path, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Ran {
    Ran::NotStarted(unavailable_error(Feature::ProgramLaunch))
}
/// Starting programs this way is not available on this system yet.
pub fn run_and_wait(_: &str, _: &str, _: &AtomicBool) -> Result<Waited, String> {
    Err(unavailable_error(Feature::ProgramLaunch))
}
/// Elevation is not available on this system yet.
pub fn restart_elevated(_: &[&str]) -> Result<(), String> {
    Err(unavailable_error(Feature::Elevation))
}
/// No file dialog: always cancelled.
pub fn file_dialog(_: bool, _: &str, _: &str, _: &str, _: &str) -> Option<String> {
    None
}
/// No folder dialog: always cancelled.
pub fn folder_dialog(_: &str, _: &str) -> Option<String> {
    None
}
/// No open dialog: it cannot be shown.
pub fn open_dialog(_: &str, _: &str, _: &str) -> Result<Option<String>, ()> {
    Err(())
}

/// The local time's offset from UTC in seconds: UTC until this system reads its zone.
pub fn local_offset_secs() -> i64 {
    0
}

/// Whether this process runs with administrator rights: not known on this system yet.
pub fn is_admin() -> bool {
    false
}
