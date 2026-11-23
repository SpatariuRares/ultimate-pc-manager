//! Linux: no uninstall registry yet.

/// The uninstall registry is not available on this system yet. `Uninstaller::uninstall`
/// reports that before it gets here, so "still listed" is never asked.
pub fn still_listed(_: &str) -> bool {
    true
}

/// The uninstall registry is not available on this system yet: no entry is there.
pub fn entry(_: &str) -> Option<(String, bool)> {
    None
}
