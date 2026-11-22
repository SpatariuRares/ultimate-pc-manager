//! The adapter that really acts on the system: each method is today's
//! `winsys` call, unchanged (on other systems `winsys` keeps its own
//! fallbacks).

use super::SystemActions;
use crate::winsys;

/// The system-actions adapter of the normal app.
pub struct WindowsActions;

impl SystemActions for WindowsActions {
    // ---- Process actions (ticket 96) ----
    fn end_process(&self, pid: u32, force: bool, tree: bool) -> Result<(), String> {
        winsys::end_process(pid, force, tree)
    }
    fn suspend_process(&self, pid: u32) -> Result<(), String> {
        winsys::suspend_process(pid)
    }
    fn resume_process(&self, pid: u32) -> Result<(), String> {
        winsys::resume_process(pid)
    }
    fn set_efficiency(&self, pid: u32) -> Result<(), String> {
        winsys::set_efficiency(pid)
    }
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String> {
        winsys::set_affinity(pid, mask)
    }

    // ---- Dump and Explorer restart (ticket 98) ----
    fn write_dump(&self, pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        winsys::write_dump(pid, name, dir)
    }
    fn restart_explorer(&self, pid: u32) -> Result<(), String> {
        winsys::restart_explorer(pid)
    }

    // ---- (add the next group of actions below, in its own section) ----
}
