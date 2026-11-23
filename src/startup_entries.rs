//! The programs that start with the system: the entries, and turning one
//! on or off.
//!
//! The module holds the shared entry type; the `windows`, `linux` and
//! `macos` submodules read and change them (Linux and macOS report that they
//! are not available yet).

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as system;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as system;

#[cfg(not(any(windows, target_os = "macos")))]
mod linux;
#[cfg(not(any(windows, target_os = "macos")))]
use linux as system;

pub use system::{list_startup, set_startup_enabled};

#[derive(Clone, Debug, Default)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct StartupEntry {
    pub name: String,
    pub command: String,
    pub location: String,
    pub enabled: bool,
    /// Where the enabled/disabled flag lives: (hive is HKCU, approved subkey).
    pub hkcu: bool,
    pub approved_key: String,
}
