//! The system-actions seam: every action the app takes on the system goes
//! through [`SystemActions`]. The app holds one adapter: [`WindowsActions`]
//! (what the normal app gets) or, in tests, `RecordingActions` (what the
//! headless app gets: it records each request and acts on nothing).
//!
//! To add an action: add a [`Request`] variant and a method to the trait
//! here, the real call in `windows.rs`, and the recording in `recording.rs`
//! (each file has one marked section per group of actions, so groups added
//! side by side do not touch the same lines).

mod windows;
pub use windows::WindowsActions;

#[cfg(test)]
mod recording;
#[cfg(test)]
pub use recording::RecordingActions;

/// What the app can ask of the system. Shared with background threads
/// (hence `Send + Sync`), like [`crate::platform::ProcessRunner`].
pub trait SystemActions: Send + Sync {
    // ---- Process actions (ticket 96) ----

    /// Ends `pid`; `force` kills it, `tree` ends the processes it started too.
    fn end_process(&self, pid: u32, force: bool, tree: bool) -> Result<(), String>;
    fn suspend_process(&self, pid: u32) -> Result<(), String>;
    fn resume_process(&self, pid: u32) -> Result<(), String>;
    fn set_efficiency(&self, pid: u32) -> Result<(), String>;
    /// Lets `pid` run only on the processors whose bit is set in `mask`.
    fn set_affinity(&self, pid: u32, mask: u64) -> Result<(), String>;

    // ---- Dump and Explorer restart (ticket 98) ----

    /// Writes a memory dump of `pid` (named after `name`) in `dir`; the file's
    /// path, or the text to say when it could not be written. Can take seconds.
    fn write_dump(&self, pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String>;
    /// Restarts the shell's Windows Explorer, whose process is `pid`. Can wait
    /// several seconds.
    fn restart_explorer(&self, pid: u32) -> Result<(), String>;

    // ---- (add the next group of actions below, in its own section) ----
}

/// One request the seam received (what the recording adapter notes).
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    // ---- Process actions (ticket 96) ----
    End { pid: u32, force: bool, tree: bool },
    Suspend(u32),
    Resume(u32),
    Efficiency(u32),
    Affinity(u32, u64),
    // ---- Dump and Explorer restart (ticket 98) ----
    Dump { pid: u32, name: String, dir: std::path::PathBuf },
    RestartExplorer(u32),
    // ---- (add the next group's requests below) ----
}
