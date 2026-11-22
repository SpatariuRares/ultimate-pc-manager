//! The test adapter: records every request and acts on nothing. A request
//! succeeds unless the test scripted a failure for it (see
//! [`RecordingActions::fail`]). Safe to share with background threads.

use std::sync::Mutex;

use super::{Request, SystemActions};

/// The system-actions adapter of the headless app.
#[derive(Default)]
pub struct RecordingActions {
    requests: Mutex<Vec<Request>>,
    /// Scripted failures: a request equal to the first of a pair fails with
    /// the second.
    failures: Mutex<Vec<(Request, String)>>,
    /// What `is_admin` answers (not elevated unless a test says so).
    admin: std::sync::atomic::AtomicBool,
}

impl RecordingActions {
    /// The requests so far, in order.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// From now on `request` (every time it is made) fails with `error`.
    pub fn fail(&self, request: Request, error: &str) {
        self.failures.lock().unwrap_or_else(|e| e.into_inner()).push((request, error.into()));
    }

    /// From now on the seam says the app does (or does not) run as administrator.
    pub fn set_admin(&self, admin: bool) {
        self.admin.store(admin, std::sync::atomic::Ordering::Relaxed);
    }

    /// Notes `request` and answers it: its scripted failure, else success.
    fn record(&self, request: Request) -> Result<(), String> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).push(request.clone());
        let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match failures.iter().find(|(r, _)| *r == request) {
            Some((_, error)) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl SystemActions for RecordingActions {
    // ---- Process actions (ticket 96) ----
    fn end_process(&self, pid: u32, force: bool, tree: bool) -> Result<(), String> {
        self.record(Request::End { pid, force, tree })
    }
    fn suspend_process(&self, pid: u32) -> Result<(), String> {
        self.record(Request::Suspend(pid))
    }
    fn resume_process(&self, pid: u32) -> Result<(), String> {
        self.record(Request::Resume(pid))
    }
    fn set_efficiency(&self, pid: u32) -> Result<(), String> {
        self.record(Request::Efficiency(pid))
    }
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String> {
        self.record(Request::Affinity(pid, mask))
    }

    // ---- Elevation, administrator check and the remaining actions (ticket 99) ----
    fn restart_elevated(&self, _page: &str) -> Result<(), String> {
        Ok(())
    }
    fn is_admin(&self) -> bool {
        false
    }
    fn toggle_uac_virtualization(&self, _pid: u32) -> Result<(), String> {
        Ok(())
    }
    fn bring_to_front(&self, _pid: u32) -> Result<(), String> {
        Ok(())
    }
    fn open_file_location(&self, _path: &str) -> Result<(), String> {
        Ok(())
    }
    fn reveal_in_explorer(&self, _path: &std::path::Path) -> Result<(), String> {
        Ok(())
    }
    fn run_new_task(&self, _cmdline: &str) -> Result<(), String> {
        Ok(())
    }
    fn open_power_options(&self) -> Result<(), String> {
        Ok(())
    }

    // ---- (add the next group of actions below, in its own section) ----
}
