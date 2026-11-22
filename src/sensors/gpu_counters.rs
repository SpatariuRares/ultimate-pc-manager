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

/// The shortest span between two collects of the rate counters.
#[cfg(windows)]
const MIN_SPAN: std::time::Duration = std::time::Duration::from_millis(200);

/// The GPU counter provider. Built on its worker thread (holds a PDH query).
pub struct GpuCounters {
    #[cfg(windows)]
    query: Option<super::pdh::Query>,
    #[cfg(windows)]
    collected: Instant,
    /// What the last read counted: every engine, the adapters' dedicated and
    /// shared memory.
    counted: Vec<CounterSet>,
}

impl GpuCounters {
    pub fn new() -> Self {
        #[cfg(windows)]
        use crate::perfdata::counter;
        GpuCounters {
            #[cfg(windows)]
            query: super::pdh::Query::open(&[
                (counter::GPU, r"\GPU Engine(*)\Utilization Percentage"),
                (counter::GDED, r"\GPU Adapter Memory(*)\Dedicated Usage"),
                (counter::GSH, r"\GPU Adapter Memory(*)\Shared Usage"),
            ]),
            #[cfg(windows)]
            collected: Instant::now(),
            counted: Vec::new(),
        }
    }
}

impl Provider for GpuCounters {
    fn name(&self) -> &'static str {
        SOURCE
    }
    fn availability(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        return match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        };
        #[cfg(not(windows))]
        Err("Windows only".into())
    }
    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }
    fn read(&mut self, _at: Instant) -> Vec<Reading> {
        #[cfg(windows)]
        if let Some(q) = self.query.as_mut() {
            use crate::perfdata::counter;
            // A rate counter needs a span between two collects.
            let wait = MIN_SPAN.saturating_sub(self.collected.elapsed());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
            q.collect();
            self.collected = Instant::now();
            self.counted = [counter::GPU, counter::GDED, counter::GSH].into_iter().map(|key| q.counted(key, false)).collect();
        }
        Vec::new()
    }
}
