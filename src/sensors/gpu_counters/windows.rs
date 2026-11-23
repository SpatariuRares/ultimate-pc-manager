//! Windows: `\GPU Engine(*)\Utilization Percentage` and `\GPU Adapter
//! Memory(*)`, read through a PDH query.

use std::time::{Duration, Instant};

use crate::perfdata::counter;
use crate::sensors::model::CounterSet;
use crate::sensors::pdh::Query;

/// The shortest span between two collects of the rate counters.
const MIN_SPAN: Duration = Duration::from_millis(200);

/// The GPU counters query, if the counters are on.
pub struct Counters {
    query: Option<Query>,
    collected: Instant,
}

impl Counters {
    pub fn open() -> Self {
        Counters {
            query: Query::open(&[
                (counter::GPU, r"\GPU Engine(*)\Utilization Percentage"),
                (counter::GDED, r"\GPU Adapter Memory(*)\Dedicated Usage"),
                (counter::GSH, r"\GPU Adapter Memory(*)\Shared Usage"),
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
        Some([counter::GPU, counter::GDED, counter::GSH].into_iter().map(|key| q.counted(key, false)).collect())
    }
}
