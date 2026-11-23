//! Opening things on Linux: today's fallbacks (`xdg-open`, `sh -c`).

use crate::availability::{unavailable_error, Feature};

/// Opens the folder holding `path`.
pub fn show_file_location(path: &str) -> Result<(), String> {
    let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
    std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Opens the folder holding `path`.
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    super::open_file_location(&path.display().to_string())
}

/// There is no Power Options panel here: an error.
pub fn open_power_options() -> Result<(), String> {
    Err(unavailable_error(Feature::PowerOptionsPanel))
}

/// Starts `cmdline` through the shell.
pub fn start_command(cmdline: &str) -> Result<(), String> {
    std::process::Command::new("sh").args(["-c", cmdline]).spawn().map(|_| ()).map_err(|e| e.to_string())
}
