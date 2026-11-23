//! The description a program file carries (the name its publisher gives it).
//!
//! The `windows`, `linux` and `macos` submodules read it (Linux and macOS
//! have no version resource: no description).

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

pub use system::file_description;
