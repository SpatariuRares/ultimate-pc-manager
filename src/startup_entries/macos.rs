//! Startup entries on macOS: not available yet, as the app already says.

use super::StartupEntry;
use crate::availability::{unavailable_error, Feature};

/// Where the list comes from, as the page's band says it.
pub fn sources_note() -> &'static str {
    "From the startup entries of this system"
}

/// No startup entries.
pub fn list_startup() -> Vec<StartupEntry> {
    Vec::new()
}

/// Startup entries cannot be changed: an error.
pub fn set_startup_enabled(_: &StartupEntry, _: bool) -> Result<(), String> {
    Err(unavailable_error(Feature::StartupEntries))
}
