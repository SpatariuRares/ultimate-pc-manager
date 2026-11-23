//! Linux: the processor counters are not read yet.

use crate::availability::{unavailable_error, Feature};
use crate::sensors::cpu::CpuSample;
use crate::sensors::model::CounterSet;

/// The processor query: none, not available yet.
pub struct Counters;

impl Counters {
    pub fn open() -> Self {
        Counters
    }

    pub fn availability(&self) -> Result<(), String> {
        Err(unavailable_error(Feature::HardwareSensors))
    }

    /// A sample with the processor's name and no figures.
    pub fn sample(&mut self, name: &str, _machine_mhz: u32, _counted: &mut Vec<CounterSet>) -> CpuSample {
        CpuSample { name: name.to_string(), ..Default::default() }
    }
}

/// The processor's marketing name: not available yet.
pub fn cpu_name() -> String {
    String::new()
}
