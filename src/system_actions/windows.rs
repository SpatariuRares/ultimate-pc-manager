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

    // ---- (add the next group of actions below, in its own section) ----

    // ---- Process priority and the process menu's reads (ticket 97) ----
    fn set_priority(&self, pid: u32, class: &str) -> Result<(), String> {
        winsys::set_priority(pid, class)
    }
    fn priority_of(&self, pid: u32) -> Option<&'static str> {
        winsys::get_priority(pid)
    }
    fn affinity_of(&self, pid: u32) -> Option<winsys::Affinity> {
        winsys::get_affinity(pid)
    }
    fn has_window(&self, pid: u32) -> bool {
        crate::proc_info::has_window(pid)
    }
}
