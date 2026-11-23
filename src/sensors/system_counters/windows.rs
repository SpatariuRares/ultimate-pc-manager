//! Windows: the kernel time, pages read, compression and page file counters
//! through a PDH query, and GetPerformanceInfo's own tally.

use std::time::{Duration, Instant};

use crate::perfdata::counter;
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::pdh::Query;

/// The shortest span between two collects of the rate counters.
const MIN_SPAN: Duration = Duration::from_millis(200);

/// The system counters query, if the counters are on.
pub struct Counters {
    query: Option<Query>,
    collected: Instant,
}

impl Counters {
    pub fn open() -> Self {
        Counters {
            query: Query::open(&[
                (counter::KERNEL, r"\Processor(*)\% Privileged Time"),
                (counter::PAGEIN, r"\Memory\Pages Input/sec"),
                (counter::COMPR, r"\Process(Memory Compression)\Working Set"),
                (counter::PAGEFILE, r"\Paging File(_Total)\% Usage"),
            ]),
            collected: Instant::now(),
        }
    }

    pub fn availability(&self) -> Result<(), String> {
        match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        }
    }

    /// What the counters count now; `None` when they are off.
    pub fn read(&mut self) -> Option<Vec<CounterSet>> {
        let q = self.query.as_mut()?;
        // A rate counter needs a span between two collects.
        let wait = MIN_SPAN.saturating_sub(self.collected.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        q.collect();
        self.collected = Instant::now();
        Some(vec![
            q.counted(counter::KERNEL, false),
            q.counted(counter::PAGEIN, true),
            q.counted(counter::COMPR, false),
            q.counted(counter::PAGEFILE, false),
            performance_info(),
        ])
    }
}

/// What GetPerformanceInfo returns, as a counted set of named figures in
/// bytes or units; no instance when the call failed.
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
