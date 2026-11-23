//! Startup entries on macOS: not available yet, as the app already says.

use super::StartupEntry;
use crate::availability::{unavailable_error, Feature};

/// No startup entries.
pub fn list_startup() -> Vec<StartupEntry> {
    Vec::new()
}

/// Startup entries cannot be changed: an error.
pub fn set_startup_enabled(_: &StartupEntry, _: bool) -> Result<(), String> {
    Err(unavailable_error(Feature::StartupEntries))
}
