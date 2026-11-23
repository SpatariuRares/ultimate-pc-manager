//! Windows: the Energy Meter's `\Energy Meter(*)\Power` counters, read
//! through a PDH query.

use crate::perfdata::counter;
use crate::sensors::model::CounterSet;
use crate::sensors::pdh::Query;

/// The Energy Meter query, if the counters are on.
pub struct Counters {
    query: Option<Query>,
}

impl Counters {
    pub fn open() -> Self {
        Counters { query: Query::open(&[(counter::POWER, r"\Energy Meter(*)\Power")]) }
    }

    pub fn availability(&self) -> Result<(), String> {
        match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        }
    }

    /// The channels now, in milliwatts.
    pub fn read(&mut self) -> CounterSet {
        match self.query.as_mut() {
            Some(q) => {
                q.collect();
                q.counted(counter::POWER, false)
            }
            None => CounterSet::new(counter::POWER, Vec::new()),
        }
    }
}
