//! The facts System Info shows beyond what every system reports (the
//! snapshot and the OS name): each system's own, read by its submodule.
//! The page asks [`fields`] and merges the answer into its own fields.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as system;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as system;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as system;

/// The running system's own facts as System Info fields, in page order
/// (a fact the system does not expose is left out).
pub fn fields() -> Vec<(&'static str, String)> {
    system::fields()
}
