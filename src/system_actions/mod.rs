//! The system-actions seam: every action the app takes on the system goes
//! through [`SystemActions`]. The app holds one adapter: [`WindowsActions`]
//! (what the normal app gets) or, in tests, `RecordingActions` (what the
//! headless app gets: it records each request and acts on nothing).
//!
//! To add an action: add a [`Request`] variant and a method to the trait
//! here, the real call in `windows.rs`, and the recording in `recording.rs`
//! (each file keeps its actions in sections named after the concern).

mod windows;
pub use windows::WindowsActions;

#[cfg(test)]
mod recording;
#[cfg(test)]
pub use recording::RecordingActions;

/// What the app can ask of the system. Shared with background threads
/// (hence `Send + Sync`), like [`crate::platform::ProcessRunner`].
pub trait SystemActions: Send + Sync {
    // ---- Process control ----

    /// Ends `pid`; `force` kills it, `tree` ends the processes it started too.
    fn end_process(&self, pid: u32, force: bool, tree: bool) -> Result<(), String>;
    fn suspend_process(&self, pid: u32) -> Result<(), String>;
    fn resume_process(&self, pid: u32) -> Result<(), String>;
    fn set_efficiency(&self, pid: u32) -> Result<(), String>;
    /// Lets `pid` run only on the processors whose bit is set in `mask`.
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String>;

    // ---- Dumps and Explorer ----

    /// Writes a memory dump of `pid` (named after `name`) in `dir`; the file's
    /// path, or the text to say when it could not be written. Can take seconds.
    fn write_dump(&self, pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String>;
    /// Restarts the shell's Windows Explorer, whose process is `pid`. Can wait
    /// several seconds.
    fn restart_explorer(&self, pid: u32) -> Result<(), String>;

    // ---- Elevation and shell ----

    /// Starts the app again with administrator rights, opening on `page`.
    /// Ok means the elevated copy took over and this one should close.
    fn restart_elevated(&self, page: &str) -> Result<(), String>;
    /// Whether the app runs with administrator rights.
    fn is_admin(&self) -> bool;
    /// Turns UAC virtualization of `pid` on or off.
    fn toggle_uac_virtualization(&self, pid: u32) -> Result<(), String>;
    /// Brings the main window of `pid` to the front.
    fn bring_to_front(&self, pid: u32) -> Result<(), String>;
    /// Opens the folder of the program file `path` in Explorer.
    fn open_file_location(&self, path: &str) -> Result<(), String>;
    /// Shows `path` selected in Explorer.
    fn reveal_in_explorer(&self, path: &std::path::Path) -> Result<(), String>;
    /// Runs the command line `cmdline` (Run new task).
    fn run_new_task(&self, cmdline: &str) -> Result<(), String>;
    /// Runs `cmdline` with administrator rights for this one action
    /// (Run new task, "Create this task with administrator privileges").
    /// Waits for the permission to be answered: call it off the UI thread.
    fn run_new_task_as_admin(&self, cmdline: &str) -> Result<(), String>;
    /// Whether Run new task offers the administrator option here: not on
    /// Windows, where UAC asks for the actions that need it.
    fn offers_run_as_admin(&self) -> bool;
    fn open_power_options(&self) -> Result<(), String>;


    // ---- Priority and reads ----

    /// Sets the priority class of `pid` (one of the names Set priority offers).
    fn set_priority(&self, pid: u32, class: &str) -> Result<(), String>;
    /// The priority class of `pid`; None when it cannot be read.
    fn priority_of(&self, pid: u32) -> Option<&'static str>;
    /// The processor affinity of `pid`; None when it cannot be read.
    fn affinity_of(&self, pid: u32) -> Option<crate::process_actions::Affinity>;
    /// Whether `pid` has any visible top-level window.
    fn has_window(&self, pid: u32) -> bool;
    /// The process of the shell's Windows Explorer (the one that owns the
    /// shell window); None when there is none.
    fn shell_pid(&self) -> Option<u32>;
}

/// One request the seam received (what the recording adapter notes).
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    // ---- Process control ----
    End { pid: u32, force: bool, tree: bool },
    Suspend(u32),
    Resume(u32),
    Efficiency(u32),
    Affinity(u32, u64),
    // ---- Dumps and Explorer ----
    Dump { pid: u32, name: String, dir: std::path::PathBuf },
    RestartExplorer(u32),

    // ---- Elevation and shell ----
    RestartElevated(String),
    UacVirtualization(u32),
    BringToFront(u32),
    OpenFileLocation(String),
    RevealInExplorer(std::path::PathBuf),
    RunNewTask(String),
    RunNewTaskAsAdmin(String),
    OpenPowerOptions,

    // ---- Priority and reads ----
    Priority(u32, String),
}
