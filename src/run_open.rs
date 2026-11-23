//! Showing a file in the file manager, starting a program, folder or
//! document by name, and opening the power settings.
//!
//! The module holds the checks of what the user typed; the `windows`,
//! `linux` and `macos` submodules do the opening.

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

pub use system::{open_power_options, reveal_in_explorer};

pub fn open_file_location(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("The file location of this process can't be read.".into());
    }
    system::show_file_location(path)
}

pub fn run_new_task(cmdline: &str) -> Result<(), String> {
    let cmdline = cmdline.trim();
    if cmdline.is_empty() {
        return Err("Type the name of a program, folder or document.".into());
    }
    system::start_command(cmdline)
}
