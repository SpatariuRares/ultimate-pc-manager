//! A provider for tests whose figures a test sets and whose reads it counts:
//! it stands in for the processor, disk and energy providers, so an app can
//! run over a sensor service of fake providers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::cpu::CpuSample;
use super::model::{CounterSet, Reading};
use super::provider::{Factory, Provider};

/// What a fake provider reads now, and how often it was read.
#[derive(Default)]
pub struct Script {
    /// What it counts (see [`Provider::counted`]).
    pub sets: Mutex<Vec<CounterSet>>,
    /// The processor it reads (its readings are the real provider's).
    pub cpu: Mutex<Option<CpuSample>>,
    /// Reads begun.
    pub reads: AtomicUsize,
    /// How long a read takes.
    pub delay: Mutex<Duration>,
    /// What the last read was told about the hardware sensors.
    pub hardware_wanted: Mutex<Option<bool>>,
}

impl Script {
    pub fn new(sets: Vec<CounterSet>) -> Arc<Self> {
        Arc::new(Script { sets: Mutex::new(sets), ..Default::default() })
    }

    /// A fake processor: its readings and its performance level.
    pub fn cpu(sample: CpuSample) -> Arc<Self> {
        let performance = sample.performance_percent;
        let script = Script::new(Vec::new());
        if let Some(p) = performance {
            *script.sets.lock().unwrap() = vec![CounterSet::new(crate::perfdata::counter::PERF, vec![super::model::Instance::new("_Total", p)])];
        }
        *script.cpu.lock().unwrap() = Some(sample);
        script
    }

    pub fn reads(&self) -> usize {
        self.reads.load(Ordering::SeqCst)
    }

    pub fn set(&self, sets: Vec<CounterSet>) {
        *self.sets.lock().unwrap() = sets;
    }

    pub fn slow(&self, delay: Duration) {
        *self.delay.lock().unwrap() = delay;
    }
}

struct Fake {
    name: &'static str,
    script: Arc<Script>,
    counted: Vec<CounterSet>,
}

impl Provider for Fake {
    fn name(&self) -> &'static str {
        self.name
    }
    fn hardware_wanted(&mut self, wanted: bool) {
        *self.script.hardware_wanted.lock().unwrap() = Some(wanted);
    }
    fn read(&mut self, at: Instant) -> Vec<Reading> {
        self.script.reads.fetch_add(1, Ordering::SeqCst);
        let delay = *self.script.delay.lock().unwrap();
        std::thread::sleep(delay);
        self.counted = self.script.sets.lock().unwrap().clone();
        self.script.cpu.lock().unwrap().as_ref().map(|s| s.readings(at)).unwrap_or_default()
    }
    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }
    fn timeout(&self) -> Duration {
        Duration::from_millis(1_500)
    }
}

/// A provider named `name` that reads what `script` says.
pub fn provider(name: &'static str, script: &Arc<Script>) -> Factory {
    let script = script.clone();
    Arc::new(move || Box::new(Fake { name, script: script.clone(), counted: Vec::new() }))
}
