//! The alerts' watcher: one thread, alive only while at least one alert
//! exists, that holds an interest in the sensor service (page open or
//! not, window minimized or not), evaluates every new frame with
//! [`alerts::evaluate`] and raises a Windows notification on each
//! transition.

use super::alerts::{self, Rule, Tracked};
use crate::sensors::Frame;
use crate::platform::AddonContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// How often the watcher looks for a new frame.
const POLL: Duration = Duration::from_millis(250);

/// What the watcher thread and the add-on share.
#[derive(Default)]
struct Shared {
    rules: Mutex<Vec<Rule>>,
    tracked: Mutex<Vec<Tracked>>,
    /// The last frame it evaluated (Settings lists its sensors).
    frame: Mutex<Option<Arc<Frame>>>,
    fahrenheit: AtomicBool,
    stop: Mutex<bool>,
    wake: Condvar,
}

/// The running watcher; dropping it ends its thread (and its interest).
pub struct Watcher {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Why its thread could not start (the alerts are then not watched).
    problem: Option<String>,
}

impl Watcher {
    /// Starts watching `rules` (temperatures told in °F when `fahrenheit`).
    pub fn start(context: &AddonContext, rules: Vec<Rule>, fahrenheit: bool) -> Self {
        let shared = Arc::new(Shared { rules: Mutex::new(rules), fahrenheit: AtomicBool::new(fahrenheit), ..Shared::default() });
        let (mine, context) = (shared.clone(), context.clone());
        match std::thread::Builder::new().name("power-freq alerts".into()).spawn(move || watch(&mine, &context)) {
            Ok(thread) => Watcher { shared, thread: Some(thread), problem: None },
            Err(e) => {
                let why = format!("{NOT_STARTED} ({e})");
                eprintln!("power-freq: {why}");
                Watcher { shared, thread: None, problem: Some(why) }
            }
        }
    }

    /// Why the alerts are not watched, when its thread could not start.
    pub fn problem(&self) -> Option<&str> {
        self.problem.as_deref()
    }

    /// The alerts it watches.
    pub fn rules(&self) -> Vec<Rule> {
        self.shared.rules.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Watches these alerts from the next frame on. An edited (or deleted)
    /// alert starts afresh at once: its status is dropped; the alerts left
    /// as they were keep theirs.
    pub fn set_rules(&self, rules: Vec<Rule>) {
        // Locked in the thread's order: rules, then statuses.
        let mut current = self.shared.rules.lock().unwrap_or_else(|e| e.into_inner());
        let mut tracked = self.shared.tracked.lock().unwrap_or_else(|e| e.into_inner());
        let mut kept = Vec::with_capacity(rules.len());
        for rule in &rules {
            if let Some(i) = tracked.iter().position(|t| t.rule == *rule) {
                kept.push(tracked.remove(i));
            }
        }
        *tracked = kept;
        *current = rules;
    }

    /// The temperature unit of the notifications.
    pub fn set_fahrenheit(&self, fahrenheit: bool) {
        self.shared.fahrenheit.store(fahrenheit, Ordering::Relaxed);
    }

    /// Where each alert stands after the last frame.
    pub fn tracked(&self) -> Vec<Tracked> {
        self.shared.tracked.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The last frame it evaluated, if any yet.
    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.shared.frame.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Its thread is alive.
    pub fn running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        *self.shared.stop.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Said when the watcher thread cannot start.
pub const NOT_STARTED: &str = "The alerts could not be watched: their thread did not start";

/// The alerts' statuses and transitions after `frame`, timed by when the
/// frame was read (its newest reading), not by when it is looked at.
fn step(rules: &[Rule], before: &[Tracked], frame: &Frame) -> (Vec<Tracked>, Vec<alerts::Transition>) {
    alerts::evaluate(rules, before, frame, frame.read_at().unwrap_or_else(Instant::now))
}

/// The thread: reads the latest frame every [`POLL`], evaluates the
/// alerts on each new one and notifies their transitions, until stopped.
fn watch(shared: &Shared, context: &AddonContext) {
    let sensors = context.sensors().interest();
    let mut seen = None;
    loop {
        if let Some(frame) = sensors.latest().filter(|f| seen != Some(f.seq)) {
            seen = Some(frame.seq);
            // Both held, so an edit (`set_rules`) lands before or after a
            // frame, never in the middle.
            let rules = shared.rules.lock().unwrap_or_else(|e| e.into_inner());
            let mut tracked = shared.tracked.lock().unwrap_or_else(|e| e.into_inner());
            let (next, changes) = step(&rules, &tracked, &frame);
            *tracked = next;
            drop((tracked, rules));
            *shared.frame.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
            let fahrenheit = shared.fahrenheit.load(Ordering::Relaxed);
            for change in changes {
                let (title, body) = change.message(fahrenheit);
                // A notification that cannot be shown is not retried.
                let _ = context.notify(&title, &body);
            }
        }
        let stop = shared.stop.lock().unwrap_or_else(|e| e.into_inner());
        let (stop, _) = shared.wake.wait_timeout_while(stop, POLL, |s| !*s).unwrap_or_else(|e| e.into_inner());
        if *stop {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::cpu::CORE_AVERAGE;
    use crate::sensors::model::{Device, Group, Reading, Unit};
    use alerts::{Change, Comparison};

    fn until(what: &str, mut cond: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn above(metric: &str, threshold: f64, duration_s: u64) -> Rule {
        Rule { metric: metric.into(), comparison: Comparison::Above, threshold, duration_s }
    }

    #[test]
    fn edited_alerts_start_afresh_at_once() {
        let (ctx, _) = AddonContext::recording();
        let kept = above(CORE_AVERAGE, 0.0, 0);
        let edited = above(CORE_AVERAGE, -1.0, 0);
        let watcher = Watcher::start(&ctx, vec![kept.clone(), edited.clone()], false);
        until("both triggered", || watcher.tracked().iter().filter(|t| t.status.active).count() == 2);
        let slow = above(CORE_AVERAGE, -1.0, 3600);
        watcher.set_rules(vec![kept.clone(), slow.clone()]);
        let tracked = watcher.tracked();
        assert!(tracked.iter().all(|t| t.rule != edited), "the edited alert's old status is gone: {tracked:?}");
        assert!(tracked.iter().any(|t| t.rule == kept && t.status.active), "an untouched alert keeps its status");
        assert!(!tracked.iter().any(|t| t.rule == slow && t.status.active), "the edited alert waits again");
    }

    #[test]
    fn watcher_drop_ends_thread_and_interest_promptly() {
        let (ctx, _) = AddonContext::recording();
        let watcher = Watcher::start(&ctx, vec![above(CORE_AVERAGE, 0.0, 0)], false);
        until("watching with an interest", || watcher.running() && ctx.sensors().threads() > 0 && watcher.frame().is_some());
        let dropped = Instant::now();
        drop(watcher);
        assert!(dropped.elapsed() < 2 * POLL, "the thread ended in {:?}", dropped.elapsed());
        while ctx.sensors().threads() != 0 {
            assert!(dropped.elapsed() < 2 * POLL, "the sensor service still runs {} threads", ctx.sensors().threads());
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn set_rules_during_evaluation_never_deadlocks() {
        let (ctx, _) = AddonContext::recording();
        let interest = ctx.sensors().interest();
        interest.set_interval_ms(20);
        let watcher = Arc::new(Watcher::start(&ctx, vec![above(CORE_AVERAGE, 0.0, 0)], false));
        let (done, finished) = std::sync::mpsc::channel();
        let editor = watcher.clone();
        std::thread::spawn(move || {
            let first = loop {
                if let Some(f) = editor.frame() {
                    break f.seq;
                }
                std::thread::yield_now();
            };
            let mut i = 0u64;
            while editor.frame().is_none_or(|f| f.seq < first + 3) {
                editor.set_rules(vec![above(CORE_AVERAGE, (i % 7) as f64, i % 3)]);
                let _ = editor.tracked();
                i += 1;
            }
            let _ = done.send(i);
        });
        let edits = finished.recv_timeout(Duration::from_secs(5)).expect("set_rules and the evaluations finished within 5 s");
        assert!(edits > 0);
        drop(interest);
    }

    /// A frame with one reading of `metric` worth `v`, read at `at`.
    fn frame(seq: u64, metric: &str, v: f64, at: Instant) -> Frame {
        Frame::build(seq, &[vec![Reading::measure(metric, Device::cpu("CPU"), Group::Clocks, "Clock", Unit::Hz, "test", at).with(Some(v), "")]], Vec::new())
    }

    #[test]
    fn alerts_are_timed_by_when_the_frame_was_read() {
        let rules = [above("clock", 1.0, 10)];
        let t0 = Instant::now();
        let (tracked, changes) = step(&rules, &[], &frame(1, "clock", 2.0, t0));
        assert!(changes.is_empty());
        let (_, changes) = step(&rules, &tracked, &frame(2, "clock", 2.0, t0 + Duration::from_secs(10)));
        assert_eq!(changes.iter().map(|c| c.change).collect::<Vec<_>>(), [Change::Triggered], "10 s apart by their readings, at once by the clock");
    }
}
