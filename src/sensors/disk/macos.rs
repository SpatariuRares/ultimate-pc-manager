//! macOS: the disks' counters and S.M.A.R.T. queries are not read yet.

use std::time::Instant;

use crate::availability::{unavailable_error, Feature};
use crate::sensors::disk::{DiskSample, DiskSmart};
use crate::sensors::model::CounterSet;

/// The disk counters query: none, not available yet.
pub struct Counters;

impl Counters {
    pub fn open() -> Self {
        Counters
    }

    pub fn availability(&self) -> Result<(), String> {
        Err(unavailable_error(Feature::HardwareSensors))
    }

    /// No disk is read.
    pub fn sample(
        &mut self,
        _models: &[(u32, String)],
        _smart_disks: &mut [(u32, bool, Option<(DiskSmart, Instant)>)],
        _health: bool,
        _counted: &mut Vec<CounterSet>,
    ) -> Vec<DiskSample> {
        Vec::new()
    }
}
