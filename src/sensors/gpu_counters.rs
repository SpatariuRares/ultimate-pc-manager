//! The GPU counters provider: Windows' `\GPU Engine(*)\Utilization Percentage`
//! (one instance per process and engine) and `\GPU Adapter Memory(*)`
//! dedicated and shared usage. It gives no readings of its own: it reports
//! what it counted (see [`Provider::counted`]) and the Sampler builds the
//! adapters, their engines and each process's share from it (see
//! `perfdata::gpu_adapters`). The vendor providers and DXCore, which know
//! the cards by their own index, stay in the hardware group and before the
//! generic one.
use std::time::Instant;

use super::model::{CounterSet, Reading};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "GPU counters";

/// The GPU counter provider. Built on its worker thread (holds a PDH query).
pub struct GpuCounters {
    counters: imp::Counters,
    /// What the last read counted: every engine, the adapters' dedicated and
    /// shared memory.
    counted: Vec<CounterSet>,
}

impl GpuCounters {
    pub fn new() -> Self {
        GpuCounters { counters: imp::Counters::open(), counted: Vec::new() }
    }
}

impl Provider for GpuCounters {
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
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;
