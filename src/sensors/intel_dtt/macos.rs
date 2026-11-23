//! macOS: Intel DTT is not read yet.

use crate::availability::{unavailable_error, Feature};

/// Whether DTT runs here: not available yet.
pub fn provider_availability() -> Result<(), String> {
    Err(unavailable_error(Feature::HardwareSensors))
}

/// Whether this PC's processor is one DTT is for: not known yet.
pub fn for_this_pc() -> bool {
    false
}
