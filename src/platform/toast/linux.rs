//! Linux notifications: not available yet.

use crate::availability::{unavailable_error, Feature};

pub fn show(_: &str, _: &str) -> Result<(), String> {
    Err(unavailable_error(Feature::Notifications))
}

pub fn shutdown() {}
