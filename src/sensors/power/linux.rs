//! Linux: the power status is not read yet.

use crate::availability::{unavailable_error, Feature};
use crate::sensors::power::PowerSample;

/// The power provider's reads: none, not available yet.
pub struct Reader;

impl Reader {
    pub fn open() -> Self {
        Reader
    }

    pub fn availability(&self) -> Result<(), String> {
        Err(unavailable_error(Feature::HardwareSensors))
    }

    /// A sample with no figures.
    pub fn sample(&mut self) -> PowerSample {
        PowerSample::default()
    }
}
