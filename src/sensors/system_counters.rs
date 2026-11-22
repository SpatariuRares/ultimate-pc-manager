//! The system counters provider: kernel time per processor, the pages read
//! from disk, the memory the compression process holds, the page file in use
//! and Windows' own tally (GetPerformanceInfo: threads, handles, commit,
//! cached). It gives no readings of its own: it reports what it counted (see
//! [`Provider::counted`]) and the Sampler builds the core pages' figures
//! from it (see `sampler::counters_into`).
use std::time::Instant;

use super::model::{CounterSet, Reading};
#[cfg(windows)]
use super::model::Instance;
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "System counters";

/// The shortest span between two collects of the rate counters.
#[cfg(windows)]
const MIN_SPAN: std::time::Duration = std::time::Duration::from_millis(200);

/// The system counter provider. Built on its worker thread (holds a PDH query).
pub struct SystemCounters {
    #[cfg(windows)]
    query: Option<super::pdh::Query>,
    #[cfg(windows)]
    collected: Instant,
    /// What the last read counted.
    counted: Vec<CounterSet>,
}

impl SystemCounters {
    pub fn new() -> Self {
        #[cfg(windows)]
        use crate::perfdata::counter;
        SystemCounters {
            #[cfg(windows)]
            query: super::pdh::Query::open(&[
                (counter::KERNEL, r"\Processor(*)\% Privileged Time"),
                (counter::PAGEIN, r"\Memory\Pages Input/sec"),
                (counter::COMPR, r"\Process(Memory Compression)\Working Set"),
                (counter::PAGEFILE, r"\Paging File(_Total)\% Usage"),
            ]),
            #[cfg(windows)]
            collected: Instant::now(),
            counted: Vec::new(),
        }
    }
}

/// What GetPerformanceInfo returns, as a counted set of named figures in
/// bytes or units; no instance when the call failed.
#[cfg(windows)]
fn performance_info() -> CounterSet {
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
    // SAFETY: the structure is zeroed with its size set, as the call needs.
    let info = unsafe {
        let mut pi: PERFORMANCE_INFORMATION = std::mem::zeroed();
        pi.cb = std::mem::size_of::<PERFORMANCE_INFORMATION>() as u32;
        (GetPerformanceInfo(&mut pi, pi.cb) != 0).then_some(pi)
    };
    let instances = info.map_or_else(Vec::new, |pi| {
        let page = pi.PageSize as f64;
        vec![
            Instance::new("threads", pi.ThreadCount as f64),
            Instance::new("handles", pi.HandleCount as f64),
            Instance::new("commit", pi.CommitTotal as f64 * page),
            Instance::new("commit_limit", pi.CommitLimit as f64 * page),
            Instance::new("cached", pi.SystemCache as f64 * page),
        ]
    });
    CounterSet::new(crate::perfdata::PERF_INFO, instances)
}

impl Provider for SystemCounters {
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
            self.counted = vec![
                q.counted(counter::KERNEL, false),
                q.counted(counter::PAGEIN, true),
                q.counted(counter::COMPR, false),
                q.counted(counter::PAGEFILE, false),
                performance_info(),
            ];
        }
        Vec::new()
    }
}
