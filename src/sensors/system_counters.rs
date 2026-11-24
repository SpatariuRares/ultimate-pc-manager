//! The system counters provider: kernel time per processor, the pages read
//! from disk, the memory the compression process holds, the page file in use
//! and Windows' own tally (GetPerformanceInfo: threads, handles, commit,
//! cached). It gives no readings of its own: it reports what it counted (see
//! [`Provider::counted`]) and the Sampler builds the core pages' figures
//! from it (see `sampler::counters_into`).
use std::time::Instant;

use super::model::{CounterSet, Reading};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "System counters";

/// The system counter provider. Built on its worker thread (holds a PDH query).
pub struct SystemCounters {
    counters: imp::Counters,
    /// What the last read counted.
    counted: Vec<CounterSet>,
}

impl SystemCounters {
    pub fn new() -> Self {
        SystemCounters { counters: imp::Counters::open(), counted: Vec::new() }
    }
}

impl Provider for SystemCounters {
    fn name(&self) -> &'static str {
        SOURCE
    }
    fn availability(&mut self) -> Result<(), String> {
        self.counters.availability()
    }
    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }
    fn read(&mut self, _at: Instant) -> Vec<Reading> {
        if let Some(sets) = self.counters.read() {
            self.counted = sets;
        }
        Vec::new()
    }
}

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
