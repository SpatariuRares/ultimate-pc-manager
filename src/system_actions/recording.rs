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
    /// What the reads answer; an unscripted read finds nothing.
    reads: Mutex<Reads>,
    /// Scripted waits: a request equal to the first of a pair takes that long
    /// to answer (like a real dump or Explorer restart).
    delays: Mutex<Vec<(Request, std::time::Duration)>>,
    /// What `is_admin` answers (not elevated unless a test says so).
    admin: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct Reads {
    priority: Vec<(u32, &'static str)>,
    affinity: Vec<(u32, crate::process_actions::Affinity)>,
    windowed: Vec<u32>,
    shell: Option<u32>,
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

    /// From now on `pid` reads as running at `class`.
    pub fn script_priority(&self, pid: u32, class: &'static str) {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).priority.push((pid, class));
    }

    /// From now on `pid` reads as having `affinity`.
    pub fn script_affinity(&self, pid: u32, affinity: crate::process_actions::Affinity) {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).affinity.push((pid, affinity));
    }

    /// From now on `pid` reads as having a visible window.
    pub fn script_window(&self, pid: u32) {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).windowed.push(pid);
    }

    /// From now on the shell's Windows Explorer reads as the process `pid`.
    pub fn script_shell(&self, pid: u32) {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).shell = Some(pid);
    }

    /// From now on `request` takes `wait` to answer.
    pub fn delay(&self, request: Request, wait: std::time::Duration) {
        self.delays.lock().unwrap_or_else(|e| e.into_inner()).push((request, wait));
    }

    /// From now on the seam says the app does (or does not) run as administrator.
    pub fn set_admin(&self, admin: bool) {
        self.admin.store(admin, std::sync::atomic::Ordering::Relaxed);
    }

    /// Notes `request` and answers it: its scripted failure, else success.
    fn record(&self, request: Request) -> Result<(), String> {
        self.requests.lock().unwrap_or_else(|e| e.into_inner()).push(request.clone());
        let wait = self.delays.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(r, _)| *r == request).map(|(_, d)| *d);
        if let Some(wait) = wait {
            std::thread::sleep(wait);
        }
        let failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        match failures.iter().find(|(r, _)| *r == request) {
            Some((_, error)) => Err(error.clone()),
            None => Ok(()),
        }
    }
}

impl SystemActions for RecordingActions {
    // ---- Process control ----
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

    // ---- Dumps and Explorer ----
    fn write_dump(&self, pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
        self.record(Request::Dump { pid, name: name.into(), dir: dir.into() })?;
        Ok(dir.join(format!("{name}-{pid}.dmp")))
    }
    fn restart_explorer(&self, pid: u32) -> Result<(), String> {
        self.record(Request::RestartExplorer(pid))
    }

    // ---- Elevation and shell ----
    fn restart_elevated(&self, page: &str) -> Result<(), String> {
        self.record(Request::RestartElevated(page.into()))
    }
    fn is_admin(&self) -> bool {
        self.admin.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn toggle_uac_virtualization(&self, pid: u32) -> Result<(), String> {
        self.record(Request::UacVirtualization(pid))
    }
    fn bring_to_front(&self, pid: u32) -> Result<(), String> {
        self.record(Request::BringToFront(pid))
    }
    fn open_file_location(&self, path: &str) -> Result<(), String> {
        self.record(Request::OpenFileLocation(path.into()))
    }
    fn reveal_in_explorer(&self, path: &std::path::Path) -> Result<(), String> {
        self.record(Request::RevealInExplorer(path.into()))
    }
    fn run_new_task(&self, cmdline: &str) -> Result<(), String> {
        self.record(Request::RunNewTask(cmdline.into()))
    }
    fn run_new_task_as_admin(&self, cmdline: &str) -> Result<(), String> {
        self.record(Request::RunNewTaskAsAdmin(cmdline.into()))
    }
    fn offers_run_as_admin(&self) -> bool {
        true
    }
    fn open_power_options(&self) -> Result<(), String> {
        self.record(Request::OpenPowerOptions)
    }


    // ---- Priority and reads ----
    fn set_priority(&self, pid: u32, class: &str) -> Result<(), String> {
        self.record(Request::Priority(pid, class.into()))
    }
    fn priority_of(&self, pid: u32) -> Option<&'static str> {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).priority.iter().find(|(p, _)| *p == pid).map(|(_, c)| *c)
    }
    fn affinity_of(&self, pid: u32) -> Option<crate::process_actions::Affinity> {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).affinity.iter().find(|(p, _)| *p == pid).map(|(_, a)| *a)
    }
    fn has_window(&self, pid: u32) -> bool {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).windowed.contains(&pid)
    }
    fn shell_pid(&self) -> Option<u32> {
        self.reads.lock().unwrap_or_else(|e| e.into_inner()).shell
    }
}
