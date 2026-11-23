//! macOS: the GPU counters are not read yet.

use crate::availability::{unavailable_error, Feature};
use crate::sensors::model::CounterSet;

/// The GPU counters query: none, not available yet.
pub struct Counters;

impl Counters {
    pub fn open() -> Self {
        Counters
    }

    pub fn availability(&self) -> Result<(), String> {
        Err(unavailable_error(Feature::HardwareSensors))
    }

    /// Nothing is counted.
    pub fn read(&mut self) -> Option<Vec<CounterSet>> {
        None
    }
}
