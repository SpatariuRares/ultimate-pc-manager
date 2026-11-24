//! The programs that start with the system: the entries, and turning one
//! on or off.
//!
//! The module holds the shared entry type; the `windows`, `linux` and
//! `macos` submodules read and change them (macOS reports that they are not
//! available yet). Where several sources of entries coexist (Linux), each is
//! a [`StartupSource`] and the lists are shown together, every entry marked
//! with its source.

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

mod sources;
#[cfg_attr(not(target_os = "linux"), allow(unused_imports))]
pub use sources::{list_all, set_enabled_in, StartupSource};

pub use system::{set_startup_enabled, sources_note};
#[cfg(not(target_os = "linux"))]
pub use system::list_startup;
#[cfg(target_os = "linux")]
pub use system::list_startup_noticed;

/// The entries, with a notice for each source that could not be read (only
/// the system's several sources can fail apart).
#[cfg(not(target_os = "linux"))]
pub fn list_startup_noticed() -> (Vec<StartupEntry>, Vec<String>) {
    (list_startup(), Vec::new())
}

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
    /// The source that read this entry (empty on Windows, which has one
    /// reader), so a change goes back to it.
    pub source: String,
    /// The entry's name inside its source (a file name); empty on Windows.
    pub key: String,
    /// The file the entry lives in, when the user can be shown one.
    pub file: String,
}
