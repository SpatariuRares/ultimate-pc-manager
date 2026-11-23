//! Process actions on Linux: not available yet, as the app already says.

use super::{Affinity, DUMP_FAILED, UAC_FAILED};
use crate::availability::{unavailable_error, Feature};

/// Dumps cannot be written: an error.
pub fn write_dump_file(_: u32, _: &std::path::Path) -> Result<(), String> {
    Err(DUMP_FAILED.into())
}

/// UAC virtualization does not exist here: an error.
pub fn toggle_uac_virtualization(_: u32) -> Result<(), String> {
    Err(UAC_FAILED.into())
}

/// End a process: politely (SIGTERM) or forced (SIGKILL).
pub fn end_process(pid: u32, force: bool, tree: bool) -> Result<(), String> {
    let _ = tree;
    let sig = if force { "-9" } else { "-15" };
    std::process::Command::new("kill").args([sig, &pid.to_string()]).status().map(|_| ()).map_err(|e| e.to_string())
}

/// No shell process is known.
pub fn shell_pid() -> Option<u32> {
    None
}

/// Priorities cannot be set: an error.
pub fn set_priority(_: u32, _: &str) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessControl))
}

/// No priority is known.
pub fn get_priority(_: u32) -> Option<&'static str> {
    None
}

/// No affinity is known.
pub fn get_affinity(_: u32) -> Option<Affinity> {
    None
}

/// Affinities cannot be set: an error.
pub fn set_affinity(_: u32, _: u64) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessControl))
}

/// Processes cannot be suspended: an error.
pub fn suspend_process(_: u32) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessControl))
}

/// Processes cannot be resumed: an error.
pub fn resume_process(_: u32) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessControl))
}

/// Efficiency mode cannot be set: an error.
pub fn set_efficiency(_: u32) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessControl))
}
