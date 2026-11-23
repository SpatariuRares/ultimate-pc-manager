//! Linux: the Energy Meter is not read yet.

use crate::availability::{unavailable_error, Feature};
use crate::perfdata::counter;
use crate::sensors::model::CounterSet;

/// The Energy Meter query: none, not available yet.
pub struct Counters;

impl Counters {
    pub fn open() -> Self {
        Counters
    }

    pub fn availability(&self) -> Result<(), String> {
        Err(unavailable_error(Feature::HardwareSensors))
    }

    /// No channels.
    pub fn read(&mut self) -> CounterSet {
        CounterSet::new(counter::POWER, Vec::new())
    }
}
