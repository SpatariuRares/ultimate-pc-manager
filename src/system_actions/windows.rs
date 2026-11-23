//! The adapter that really acts on the system: each method is today's
//! `process_actions` or `run_open` call, unchanged (on other systems those
//! concerns keep their own fallbacks).

use super::SystemActions;
use crate::{process_actions, run_open};

/// The system-actions adapter of the normal app.
pub struct WindowsActions;

impl SystemActions for WindowsActions {
    // ---- Process control ----
    fn end_process(&self, pid: u32, force: bool, tree: bool) -> Result<(), String> {
        process_actions::end_process(pid, force, tree)
    }
    fn suspend_process(&self, pid: u32) -> Result<(), String> {
        process_actions::suspend_process(pid)
    }
    fn resume_process(&self, pid: u32) -> Result<(), String> {
        process_actions::resume_process(pid)
    }
    fn set_efficiency(&self, pid: u32) -> Result<(), String> {
        process_actions::set_efficiency(pid)
    }
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String> {
        process_actions::set_affinity(pid, mask)
    }

    // ---- Dumps and Explorer ----
    fn write_dump(&self, pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        process_actions::write_dump(pid, name, dir)
    }
    fn restart_explorer(&self, pid: u32) -> Result<(), String> {
        process_actions::restart_explorer(pid)
    }

    // ---- Elevation and shell ----
    fn restart_elevated(&self, page: &str) -> Result<(), String> {
        crate::platform::restart_elevated(page)
    }
    fn is_admin(&self) -> bool {
        crate::platform::is_admin()
    }
    fn toggle_uac_virtualization(&self, pid: u32) -> Result<(), String> {
        process_actions::toggle_uac_virtualization(pid)
    }
    fn bring_to_front(&self, pid: u32) -> Result<(), String> {
        crate::proc_info::bring_to_front(pid)
    }
    fn open_file_location(&self, path: &str) -> Result<(), String> {
        run_open::open_file_location(path)
    }
    fn reveal_in_explorer(&self, path: &std::path::Path) -> Result<(), String> {
        run_open::reveal_in_explorer(path)
    }
    fn run_new_task(&self, cmdline: &str) -> Result<(), String> {
        run_open::run_new_task(cmdline)
    }
    fn open_power_options(&self) -> Result<(), String> {
        run_open::open_power_options()
    }


    // ---- Priority and reads ----
    fn set_priority(&self, pid: u32, class: &str) -> Result<(), String> {
        process_actions::set_priority(pid, class)
    }
    fn priority_of(&self, pid: u32) -> Option<&'static str> {
        process_actions::get_priority(pid)
    }
    fn affinity_of(&self, pid: u32) -> Option<process_actions::Affinity> {
        process_actions::get_affinity(pid)
    }
    fn has_window(&self, pid: u32) -> bool {
        crate::proc_info::has_window(pid)
    }
    fn shell_pid(&self) -> Option<u32> {
        process_actions::shell_pid()
    }
}
