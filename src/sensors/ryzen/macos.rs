//! macOS: AMD's Ryzen Master Monitoring SDK is a Windows library, so none is
//! probed or loaded.

use crate::sensors::ryzen::{failure_reason, Failure, Probe, Sdk};

/// What this PC says about the SDK: only whether the processor is AMD's.
pub fn probe(amd: bool) -> Probe {
    Probe { amd, ..Probe::default() }
}

/// The SDK cannot be loaded.
pub fn load(_folder: &std::path::Path) -> Result<Box<dyn Sdk>, String> {
    Err(failure_reason(Failure::NotLoaded).into())
}

/// Whether a DLL of the SDK is mapped in this process (tests).
#[cfg(test)]
pub fn loaded() -> bool {
    false
}
