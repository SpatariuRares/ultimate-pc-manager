//! Linux: NVML is not read yet.

use crate::sensors::model::Reading;
use std::time::Instant;

/// The provider's NVML session: none, not available yet.
pub struct Reader;

impl Reader {
    pub fn open() -> Self {
        Reader
    }

    pub fn availability(&self) -> Result<(), String> {
        Err("no NVIDIA GPU".into())
    }

    pub fn read(&self, _at: Instant) -> Vec<Reading> {
        Vec::new()
    }
}
