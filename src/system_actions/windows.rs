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

    // ---- Elevation, administrator check and the remaining actions (ticket 99) ----
    fn restart_elevated(&self, page: &str) -> Result<(), String> {
        crate::platform::restart_elevated(page)
    }
    fn is_admin(&self) -> bool {
        crate::platform::is_admin()
    }
    fn toggle_uac_virtualization(&self, pid: u32) -> Result<(), String> {
        winsys::toggle_uac_virtualization(pid)
    }
    fn bring_to_front(&self, pid: u32) -> Result<(), String> {
        crate::proc_info::bring_to_front(pid)
    }
    fn open_file_location(&self, path: &str) -> Result<(), String> {
        winsys::open_file_location(path)
    }
    fn reveal_in_explorer(&self, path: &std::path::Path) -> Result<(), String> {
        winsys::reveal_in_explorer(path)
    }
    fn run_new_task(&self, cmdline: &str) -> Result<(), String> {
        winsys::run_new_task(cmdline)
    }
    fn open_power_options(&self) -> Result<(), String> {
        winsys::open_power_options()
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
