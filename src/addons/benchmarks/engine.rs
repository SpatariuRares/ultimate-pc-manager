//! The benchmark engine: runs one test (or a sequence of tests) at a time
//! in its own thread. Each test is a unit with a name that reports its
//! phases, its progress (0 to 1) and its figures, and checks for a
//! cooperative cancel. The page reads the engine's state: the test and
//! phase running, the progress, the figures so far, the results of every
//! test that completed, and how the last run ended.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use crate::platform::lock_state;

/// The unit a figure is measured in, with the decimals it is shown with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measure {
    /// "Mops/sec".
    pub label: &'static str,
    /// The decimals of its values.
    pub decimals: usize,
}

impl std::fmt::Display for Measure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label)
    }
}

/// Million operations per second (CPU, one core).
pub const MOPS: Measure = Measure { label: "Mops/sec", decimals: 0 };
/// Billion operations per second (CPU, all cores).
pub const GOPS: Measure = Measure { label: "Gop/sec", decimals: 2 };
/// Gigabytes per second (memory).
pub const GBS: Measure = Measure { label: "GB/s", decimals: 1 };
/// Megabytes per second (disk).
pub const MBS: Measure = Measure { label: "MB/sec", decimals: 0 };
/// Megabits per second (Internet).
pub const MBITS: Measure = Measure { label: "Mbit/s", decimals: 1 };

/// One number a test measured, with its unit.
#[derive(Clone, Debug, PartialEq)]
pub struct Figure {
    pub name: &'static str,
    pub value: f64,
    pub unit: Measure,
}

impl Figure {
    /// The value with its unit, e.g. "1234 Mops/sec" or "12.35 Gop/sec".
    pub fn text(&self) -> String {
        format!("{:.*} {}", self.unit.decimals, self.value, self.unit.label)
    }
}

/// How the last run ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    Complete,
    Cancelled,
    Failed { test: &'static str, error: String },
}

/// What the page reads from the engine.
#[derive(Clone, Debug, Default)]
pub struct EngineState {
    /// The name of the test running, if any.
    pub running: Option<&'static str>,
    /// The phase of the running test, e.g. "Testing one CPU core".
    pub phase: Option<&'static str>,
    /// The running test's progress, 0 to 1.
    pub progress: f32,
    /// Cancel was asked and the test has not stopped yet.
    pub cancelling: bool,
    /// The figures the running test measured so far.
    pub partial: Vec<Figure>,
    /// How the last run ended (none while running or before any run): the
    /// first failure of a sequence, unless it was cancelled.
    pub outcome: Option<Outcome>,
    /// The tests of the current (or last) run that ended so far, in order,
    /// with how each ended.
    pub ended: Vec<(&'static str, Outcome)>,
    /// The figures of the last completed run of each test.
    results: HashMap<&'static str, Vec<Figure>>,
}

impl EngineState {
    /// The figures of the last completed run of `test` (empty if none).
    pub fn results(&self, test: &str) -> &[Figure] {
        self.results.get(test).map(Vec::as_slice).unwrap_or(&[])
    }

    /// The line that tells how the run is going or ended: "Cancelling",
    /// "Test complete", "Cancelled" or "%1 test failed: %2".
    pub fn status(&self) -> Option<String> {
        if self.running.is_some() {
            return self.cancelling.then(|| "Cancelling".to_string());
        }
        match self.outcome.as_ref()? {
            Outcome::Complete => Some("Test complete".into()),
            Outcome::Cancelled => Some("Cancelled".into()),
            Outcome::Failed { test, error } => Some(format!("{test} test failed: {error}")),
        }
    }
}

/// The work of a test: it reports through the probe; an `Err` is its
/// error message.
type Work = Box<dyn FnOnce(&Probe) -> Result<(), String> + Send>;

/// One test the engine can run.
pub struct Unit {
    name: &'static str,
    work: Work,
}

impl Unit {
    /// The test `name` doing `work`.
    pub fn new(name: &'static str, work: impl FnOnce(&Probe) -> Result<(), String> + Send + 'static) -> Unit {
        Unit { name, work: Box::new(work) }
    }
}

/// The state shared with the engine's thread.
#[derive(Default)]
struct Shared {
    state: Mutex<EngineState>,
    cancel: Cancel,
}

/// The engine's cancel flag, for work a test keeps outside the probe's
/// borrow (a device that waits for the GPU).
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// Cancel was asked: stop as soon as possible.
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// A running test's link to the engine: it reports its phase, progress
/// and figures, and checks whether it was cancelled. It can be shared with
/// the test's own worker threads.
pub struct Probe<'a> {
    shared: &'a Shared,
}

impl Probe<'_> {
    /// Cancel was asked: stop as soon as possible.
    pub fn cancelled(&self) -> bool {
        self.shared.cancel.cancelled()
    }

    /// The cancel flag, to keep with work that outlives this borrow.
    pub fn cancel_flag(&self) -> Cancel {
        self.shared.cancel.clone()
    }

    /// Enters `phase`.
    pub fn phase(&self, phase: &'static str) {
        lock_state(&self.shared.state).phase = Some(phase);
    }

    /// Shows the test's progress (0 to 1).
    pub fn progress(&self, p: f32) {
        lock_state(&self.shared.state).progress = p.clamp(0.0, 1.0);
    }

    /// Adds a measured figure.
    pub fn figure(&self, name: &'static str, value: f64, unit: Measure) {
        lock_state(&self.shared.state).partial.push(Figure { name, value, unit });
    }
}

/// The engine: one run (a test or a sequence) at a time, in its own thread.
#[derive(Default)]
pub struct Engine {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    /// A copy of the state now.
    pub fn state(&self) -> EngineState {
        lock_state(&self.shared.state).clone()
    }

    /// A test is running.
    pub fn busy(&self) -> bool {
        lock_state(&self.shared.state).running.is_some()
    }

    /// The engine's thread is alive.
    #[cfg(test)]
    pub fn has_thread(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Runs `units` in order in a new thread, unless a test is running
    /// (then nothing happens and it returns false). A test that fails does
    /// not stop the sequence; a cancel stops it.
    pub fn start(&mut self, units: Vec<Unit>) -> bool {
        let Some(first) = units.first().map(|u| u.name) else {
            return false;
        };
        {
            let mut s = lock_state(&self.shared.state);
            if s.running.is_some() {
                return false;
            }
            s.running = Some(first);
            s.outcome = None;
            s.ended.clear();
        }
        // The previous run has ended (it cleared `running`): reap its thread.
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.shared.cancel.0.store(false, Ordering::SeqCst);
        let shared = self.shared.clone();
        self.thread = Some(
            std::thread::Builder::new()
                .name("benchmark".into())
                .spawn(move || run(&shared, units))
                .expect("the benchmark thread starts"),
        );
        true
    }

    /// Asks the running test to stop; it says "Cancelling" until it does.
    pub fn cancel(&self) {
        let mut s = lock_state(&self.shared.state);
        if s.running.is_some() {
            self.shared.cancel.0.store(true, Ordering::SeqCst);
            s.cancelling = true;
        }
    }

    /// Cancels the running test and waits for its thread to end.
    pub fn shutdown(&mut self) {
        self.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The engine's thread: runs each unit, keeps the figures of those that
/// complete, goes on after a failure and stops at a cancel.
fn run(shared: &Shared, units: Vec<Unit>) {
    let probe = Probe { shared };
    let mut outcome = Outcome::Complete;
    for unit in units {
        if probe.cancelled() {
            outcome = Outcome::Cancelled;
            break;
        }
        {
            let mut s = lock_state(&shared.state);
            s.running = Some(unit.name);
            s.phase = None;
            s.progress = 0.0;
            s.partial.clear();
        }
        // A test that panics fails alone: the engine goes on and ends idle.
        let work = unit.work;
        let res = catch_unwind(AssertUnwindSafe(|| work(&probe))).unwrap_or_else(|panic| Err(panic_text(&*panic)));
        let mut s = lock_state(&shared.state);
        let figures = std::mem::take(&mut s.partial);
        if probe.cancelled() {
            outcome = Outcome::Cancelled;
            s.ended.push((unit.name, Outcome::Cancelled));
            break;
        }
        match res {
            Ok(()) => {
                s.results.insert(unit.name, figures);
                s.ended.push((unit.name, Outcome::Complete));
            }
            Err(error) => {
                let failed = Outcome::Failed { test: unit.name, error };
                if outcome == Outcome::Complete {
                    outcome = failed.clone();
                }
                s.ended.push((unit.name, failed));
            }
        }
    }
    let mut s = lock_state(&shared.state);
    s.running = None;
    s.phase = None;
    s.cancelling = false;
    s.partial.clear();
    if outcome == Outcome::Complete {
        s.progress = 1.0;
    }
    s.outcome = Some(outcome);
}

/// The message of a panic, or a generic one when it carries no text.
fn panic_text(panic: &(dyn Any + Send)) -> String {
    match (panic.downcast_ref::<&str>(), panic.downcast_ref::<String>()) {
        (Some(text), _) => (*text).to_string(),
        (_, Some(text)) => text.clone(),
        _ => "the test stopped unexpectedly".to_string(),
    }
}

/// Waits (10 s at most) until no test runs: the state then. For tests.
#[cfg(test)]
pub fn wait_idle(engine: &Engine) -> EngineState {
    let t = std::time::Instant::now();
    while engine.busy() && t.elapsed() < std::time::Duration::from_secs(10) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    engine.state()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    fn unit(name: &'static str, work: impl FnOnce(&Probe) -> Result<(), String> + Send + 'static) -> Unit {
        Unit::new(name, work)
    }

    /// A unit that waits until it is cancelled, after telling it started.
    fn blocking(name: &'static str) -> (Unit, std::sync::mpsc::Receiver<()>, std::sync::mpsc::Receiver<bool>) {
        let (started, wait_started) = channel();
        let (finished, wait_finished) = channel();
        let u = unit(name, move |probe| {
            probe.phase("Waiting");
            probe.figure("Partial", 7.0, GBS);
            started.send(()).unwrap();
            let t = Instant::now();
            while !probe.cancelled() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            finished.send(probe.cancelled()).unwrap();
            Ok(())
        });
        (u, wait_started, wait_finished)
    }

    #[test]
    fn a_completed_test_keeps_its_figures_and_says_test_complete() {
        let mut engine = Engine::default();
        assert!(engine.start(vec![unit("CPU", |probe| {
            probe.phase("Testing one CPU core");
            probe.progress(0.5);
            probe.figure("Single core", 1234.0, MOPS);
            Ok(())
        })]));
        let s = wait_idle(&engine);
        assert_eq!(s.running, None);
        assert_eq!(s.results("CPU"), [Figure { name: "Single core", value: 1234.0, unit: MOPS }]);
        assert_eq!(s.outcome, Some(Outcome::Complete));
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        assert_eq!(s.progress, 1.0);
    }

    #[test]
    fn the_state_shows_the_running_test_its_phase_progress_and_partial_figures() {
        let mut engine = Engine::default();
        let (u, started, finished) = blocking("Memory");
        engine.start(vec![u]);
        started.recv().unwrap();
        let s = engine.state();
        assert_eq!(s.running, Some("Memory"));
        assert_eq!(s.phase, Some("Waiting"));
        assert_eq!(s.partial, [Figure { name: "Partial", value: 7.0, unit: GBS }]);
        assert_eq!(s.status(), None);
        engine.cancel();
        assert!(finished.recv().unwrap());
    }

    #[test]
    fn only_one_test_runs_at_a_time() {
        let mut engine = Engine::default();
        let (u, started, finished) = blocking("CPU");
        assert!(engine.start(vec![u]));
        started.recv().unwrap();
        let (ran, saw_run) = channel();
        assert!(!engine.start(vec![unit("Memory", move |_| {
            ran.send(()).unwrap();
            Ok(())
        })]));
        assert_eq!(engine.state().running, Some("CPU"));
        engine.cancel();
        finished.recv().unwrap();
        wait_idle(&engine);
        assert!(saw_run.try_recv().is_err(), "the second test never ran");
    }

    #[test]
    fn cancel_says_cancelling_then_cancelled_and_keeps_previous_results() {
        let mut engine = Engine::default();
        engine.start(vec![unit("Memory", |probe| {
            probe.figure("Copy speed", 20.0, GBS);
            Ok(())
        })]);
        wait_idle(&engine);

        let (started, wait_started) = channel();
        let (release, wait_release) = channel::<()>();
        engine.start(vec![unit("Memory", move |probe| {
            probe.figure("Copy speed", 1.0, GBS);
            started.send(()).unwrap();
            while !probe.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            // A test may take a while to notice; it ends when released.
            let _ = wait_release.recv_timeout(Duration::from_secs(5));
            Ok(())
        })]);
        wait_started.recv().unwrap();
        engine.cancel();
        let s = engine.state();
        assert!(s.cancelling);
        assert_eq!(s.status().as_deref(), Some("Cancelling"));
        release.send(()).unwrap();

        let s = wait_idle(&engine);
        assert_eq!(s.outcome, Some(Outcome::Cancelled));
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert_eq!(s.results("Memory"), [Figure { name: "Copy speed", value: 20.0, unit: GBS }]);
        assert!(s.partial.is_empty());
    }

    #[test]
    fn a_failed_test_says_why_and_keeps_previous_results() {
        let mut engine = Engine::default();
        engine.start(vec![unit("CPU", |probe| {
            probe.figure("Single core", 100.0, MOPS);
            Ok(())
        })]);
        wait_idle(&engine);
        engine.start(vec![unit("CPU", |probe| {
            probe.figure("Single core", 1.0, MOPS);
            Err("out of memory".into())
        })]);
        let s = wait_idle(&engine);
        assert_eq!(s.outcome, Some(Outcome::Failed { test: "CPU", error: "out of memory".into() }));
        assert_eq!(s.status().as_deref(), Some("CPU test failed: out of memory"));
        assert_eq!(s.results("CPU"), [Figure { name: "Single core", value: 100.0, unit: MOPS }]);
    }

    #[test]
    fn a_sequence_runs_in_order_and_stops_on_cancel() {
        let mut engine = Engine::default();
        let (order, seen) = channel();
        let first = order.clone();
        let (blocker, started, finished) = blocking("Memory");
        engine.start(vec![
            unit("CPU", move |_| {
                first.send("CPU").unwrap();
                Ok(())
            }),
            blocker,
            unit("Disk", move |_| {
                order.send("Disk").unwrap();
                Ok(())
            }),
        ]);
        started.recv().unwrap();
        engine.cancel();
        finished.recv().unwrap();
        let s = wait_idle(&engine);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), ["CPU"]);
        assert_eq!(s.outcome, Some(Outcome::Cancelled));
        assert_eq!(s.ended, [("CPU", Outcome::Complete), ("Memory", Outcome::Cancelled)], "Disk never ran");
    }

    #[test]
    fn the_state_lists_the_tests_of_the_sequence_that_ended_so_far() {
        let mut engine = Engine::default();
        let (blocker, started, finished) = blocking("GPU");
        engine.start(vec![unit("CPU", |_| Ok(())), blocker]);
        started.recv().unwrap();
        let s = engine.state();
        assert_eq!(s.running, Some("GPU"));
        assert_eq!(s.ended, [("CPU", Outcome::Complete)]);
        engine.cancel();
        finished.recv().unwrap();
        wait_idle(&engine);
        engine.start(vec![unit("Disk", |_| Ok(()))]);
        assert_eq!(wait_idle(&engine).ended, [("Disk", Outcome::Complete)], "a new run starts a new list");
    }

    #[test]
    fn a_failing_test_does_not_stop_the_sequence_nor_touch_previous_results() {
        let mut engine = Engine::default();
        engine.start(vec![unit("GPU", |probe| {
            probe.figure("Compute", 50.0, GOPS);
            Ok(())
        })]);
        wait_idle(&engine);

        let (order, seen) = channel();
        let (o1, o2) = (order.clone(), order.clone());
        engine.start(vec![
            unit("CPU", move |probe| {
                o1.send("CPU").unwrap();
                probe.figure("Single core", 100.0, MOPS);
                Ok(())
            }),
            unit("GPU", move |probe| {
                o2.send("GPU").unwrap();
                probe.figure("Compute", 1.0, GOPS);
                Err("no card".into())
            }),
            unit("Disk", move |probe| {
                order.send("Disk").unwrap();
                probe.figure("Write speed", 500.0, MBS);
                Ok(())
            }),
        ]);
        let s = wait_idle(&engine);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), ["CPU", "GPU", "Disk"]);
        assert_eq!(s.results("CPU"), [Figure { name: "Single core", value: 100.0, unit: MOPS }]);
        assert_eq!(s.results("GPU"), [Figure { name: "Compute", value: 50.0, unit: GOPS }], "the previous GPU results");
        assert_eq!(s.results("Disk"), [Figure { name: "Write speed", value: 500.0, unit: MBS }]);
        let failed = Outcome::Failed { test: "GPU", error: "no card".into() };
        assert_eq!(s.ended, [("CPU", Outcome::Complete), ("GPU", failed.clone()), ("Disk", Outcome::Complete)]);
        assert_eq!(s.outcome, Some(failed));
        assert_eq!(s.status().as_deref(), Some("GPU test failed: no card"));
    }

    #[test]
    fn a_panicking_test_fails_without_leaving_the_engine_busy() {
        let mut engine = Engine::default();
        engine.start(vec![unit("CPU", |_| panic!("kernel crashed")), unit("Disk", |_| Ok(()))]);
        let s = wait_idle(&engine);
        assert!(!engine.busy());
        let failed = Outcome::Failed { test: "CPU", error: "kernel crashed".into() };
        assert_eq!(s.ended, [("CPU", failed.clone()), ("Disk", Outcome::Complete)]);
        assert_eq!(s.outcome, Some(failed));
        assert!(engine.start(vec![unit("CPU", |_| Ok(()))]), "a new run can start");
        assert_eq!(wait_idle(&engine).outcome, Some(Outcome::Complete));
    }

    #[test]
    fn shutdown_cancels_the_test_and_leaves_no_thread() {
        let mut engine = Engine::default();
        let (u, started, finished) = blocking("CPU");
        engine.start(vec![u]);
        started.recv().unwrap();
        assert!(engine.has_thread());
        engine.shutdown();
        assert!(finished.try_recv().unwrap(), "the test was cancelled and had ended");
        assert!(!engine.has_thread());
        assert!(!engine.busy());
    }

    #[test]
    fn no_thread_runs_when_no_test_is_running() {
        let mut engine = Engine::default();
        assert!(!engine.has_thread());
        engine.start(vec![unit("CPU", |_| Ok(()))]);
        wait_idle(&engine);
        let t = Instant::now();
        while engine.has_thread() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(!engine.has_thread());
    }

    #[test]
    fn figures_are_formatted_with_their_units() {
        assert_eq!(Figure { name: "Single core", value: 1234.4, unit: MOPS }.text(), "1234 Mops/sec");
        assert_eq!(Figure { name: "Multi core", value: 12.345, unit: GOPS }.text(), "12.35 Gop/sec");
        assert_eq!(Figure { name: "Copy speed", value: 21.06, unit: GBS }.text(), "21.1 GB/s");
        assert_eq!(Figure { name: "Write speed", value: 1500.4, unit: MBS }.text(), "1500 MB/sec");
    }
}
