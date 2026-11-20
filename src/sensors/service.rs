//! The shared sensor service: one worker thread per provider, alive only
//! while someone holds an [`Interest`] in it (refcounted). Each worker reads
//! its provider at the app's update speed (not at all while Paused) and
//! merges what it read into the shared [`Frame`].
//!
//! Lifecycle: the first [`SensorService::interest`] starts a run (one
//! worker per provider); dropping the last [`Interest`] ends it at once
//! without waiting: workers see the run number change and exit after their
//! current read, and nothing of an old run is published (checked under the
//! state lock). The frame, the histories and the statistics are dropped
//! with the run.

use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::model::{Frame, Reading, SourceStatus};
use super::stats::Stats;
pub use super::provider::{Factory, Provider};

/// The time the history of each metric covers unless told otherwise.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);

/// Timestamped values per metric over a fixed time window (pure): points
/// older than the window (measured from the newest point) are dropped, so
/// the time covered does not depend on the update speed.
#[derive(Clone, Debug)]
pub struct History {
    window: Duration,
    by_id: HashMap<String, Series>,
}

/// One metric's points, oldest first.
#[derive(Clone, Debug, Default)]
struct Series {
    points: VecDeque<(Instant, f64)>,
    /// A point fell out of the window: the points cover all of it.
    full: bool,
}

impl Series {
    /// Drops the points older than `window` before the newest.
    fn trim(&mut self, window: Duration) {
        let Some(&(newest, _)) = self.points.back() else { return };
        while self.points.front().is_some_and(|&(at, _)| newest.duration_since(at) > window) {
            self.points.pop_front();
            self.full = true;
        }
    }

    fn span(&self) -> Duration {
        match (self.points.front(), self.points.back()) {
            (Some(&(first, _)), Some(&(last, _))) => last.duration_since(first),
            _ => Duration::ZERO,
        }
    }
}

impl Default for History {
    fn default() -> Self {
        History::new(DEFAULT_WINDOW)
    }
}

impl History {
    /// An empty history covering `window`.
    pub fn new(window: Duration) -> Self {
        History { window, by_id: HashMap::new() }
    }

    /// The time it covers.
    pub fn window(&self) -> Duration {
        self.window
    }

    /// Covers `window` from now on (a shorter one drops the older points).
    pub fn set_window(&mut self, window: Duration) {
        if window < self.window {
            for series in self.by_id.values_mut() {
                series.trim(window);
            }
        } else if window > self.window {
            for series in self.by_id.values_mut() {
                series.full = false;
            }
        }
        self.window = window;
    }

    /// Adds the finite values of these readings at their read instant. A
    /// reading not newer than the metric's last point is ignored, so the
    /// readings of a frame shown again are counted once.
    pub fn record<'a>(&mut self, readings: impl IntoIterator<Item = &'a Reading>) {
        for r in readings {
            let Some(v) = r.value.filter(|v| v.is_finite()) else { continue };
            let series = self.by_id.entry(r.id.clone()).or_default();
            if series.points.back().is_some_and(|&(at, _)| r.at <= at) {
                continue;
            }
            series.points.push_back((r.at, v));
            series.trim(self.window);
        }
    }

    /// The values of a metric within the window, oldest first.
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn values(&self, id: &str) -> Vec<f64> {
        self.by_id.get(id).map(|s| s.points.iter().map(|&(_, v)| v).collect()).unwrap_or_default()
    }

    /// The values of a metric spread by time over a graph of `slots`
    /// points covering the window (linear between the points read). A
    /// window still filling covers only its share of the slots, ending at
    /// the newest point; nothing is returned with no point.
    pub fn graph(&self, id: &str, slots: usize) -> Vec<f64> {
        let Some(series) = self.by_id.get(id).filter(|s| !s.points.is_empty()) else { return Vec::new() };
        let newest = series.points[series.points.len() - 1];
        if slots < 2 || self.window.is_zero() {
            return vec![newest.1];
        }
        let step = self.window.as_secs_f64() / (slots - 1) as f64;
        let covered = if series.full { self.window } else { series.span().min(self.window) };
        let n = ((covered.as_secs_f64() / step).round() as usize + 1).min(slots);
        let mut out = Vec::with_capacity(n);
        let mut next = 0; // the first point not before the slot's instant
        for k in 0..n {
            let back = Duration::from_secs_f64(step * (n - 1 - k) as f64);
            let t = newest.0.checked_sub(back).unwrap_or(newest.0);
            while next < series.points.len() && series.points[next].0 < t {
                next += 1;
            }
            let before = next.checked_sub(1).map(|i| series.points[i]);
            let v = match (before, series.points.get(next)) {
                (Some((a, va)), Some(&(b, vb))) => {
                    let f = t.duration_since(a).as_secs_f64() / b.duration_since(a).as_secs_f64();
                    va + (vb - va) * f
                }
                (None, Some(&(_, v))) | (Some((_, v)), None) => v,
                (None, None) => newest.1,
            };
            out.push(v);
        }
        out
    }

    /// The time the graph of a metric shows: the window once it is full
    /// (or before anything was read), the real span of the points while
    /// it fills ("Last 40 seconds").
    pub fn caption(&self, id: &str) -> String {
        let filling = self.by_id.get(id).filter(|s| !s.points.is_empty() && !s.full).map(Series::span);
        caption_for(filling.unwrap_or(self.window).min(self.window))
    }
}

/// "Last minute", "Last 2 minutes", "Last 40 seconds", "Last second".
fn caption_for(shown: Duration) -> String {
    let secs = (shown.as_secs_f64().round() as u64).max(1);
    match secs {
        1 => "Last second".into(),
        60 => "Last minute".into(),
        s if s % 60 == 0 => format!("Last {} minutes", s / 60),
        s => format!("Last {s} seconds"),
    }
}

/// One provider's place in a run.
struct Slot {
    name: &'static str,
    /// Its last readings; `None` until it answered once.
    report: Option<Vec<Reading>>,
    /// Why it reads nothing (unavailable on this PC).
    problem: Option<String>,
    /// A read started then and has not ended yet.
    busy_since: Option<Instant>,
    timeout: Duration,
    /// When the run started (a provider that never answers times out too).
    started: Instant,
    /// For hardware this PC lacks: left out of the frame's sources.
    hidden: bool,
}

impl Slot {
    fn stuck(&self, now: Instant) -> bool {
        let since = self.busy_since.or(self.report.is_none().then_some(self.started));
        since.is_some_and(|t| now.duration_since(t) > self.timeout)
    }
}

struct State {
    interest: usize,
    /// Bumped at every start and stop: workers of another run exit.
    run: u64,
    interval_ms: u64,
    /// Bumped by "Refresh": every worker reads once more.
    kick: u64,
    slots: Vec<Slot>,
    frame: Option<Arc<Frame>>,
    /// Never reset, so frames of a later run are newer.
    seq: u64,
    history: History,
    /// Min / Avg / Max of every frame published in this run; readers get
    /// the snapshot (copied on the next publish only while one is held).
    stats: Arc<Stats>,
}

struct Shared {
    factories: Vec<Factory>,
    state: Mutex<State>,
    wake: Condvar,
    threads: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The shared sensor service (cheap to clone: one service per app, reached
/// through `AddonContext::sensors`).
#[derive(Clone)]
pub struct SensorService(Arc<Shared>);

impl SensorService {
    /// A service over these providers; nothing runs until someone asks.
    pub fn new(factories: Vec<Factory>) -> Self {
        let state = State { interest: 0, run: 0, interval_ms: 1000, kick: 0, slots: Vec::new(), frame: None, seq: 0, history: History::default(), stats: Arc::default() };
        SensorService(Arc::new(Shared { factories, state: Mutex::new(state), wake: Condvar::new(), threads: AtomicUsize::new(0) }))
    }

    /// Asks for sensors: the service runs while at least one [`Interest`]
    /// is alive.
    pub fn interest(&self) -> Interest {
        let mut st = self.0.lock();
        st.interest += 1;
        if st.interest == 1 {
            st.run += 1;
            st.stats = Arc::new(Stats::new(crate::platform::local_secs()));
            let now = Instant::now();
            st.slots = self
                .0
                .factories
                .iter()
                .map(|_| Slot { name: "", report: None, problem: None, busy_since: None, timeout: super::provider::DEFAULT_TIMEOUT, started: now, hidden: false })
                .collect();
            let born = st.run;
            for i in 0..self.0.factories.len() {
                let shared = self.0.clone();
                shared.threads.fetch_add(1, Ordering::SeqCst);
                std::thread::spawn(move || {
                    let _alive = Alive(&shared);
                    work(&shared, born, i);
                });
            }
        }
        Interest(self.0.clone())
    }

    /// Worker threads alive now (0 while nobody asks, once old reads end).
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn threads(&self) -> usize {
        self.0.threads.load(Ordering::SeqCst)
    }
}

/// Counts a worker thread while it lives.
struct Alive<'a>(&'a Shared);

impl Drop for Alive<'_> {
    fn drop(&mut self) {
        self.0.threads.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A worker: builds its provider, then reads it once at once and then at
/// every interval (or Refresh), until its run ends.
fn work(shared: &Shared, born: u64, i: usize) {
    // A provider that panics (a crashing vendor library) fails alone.
    let built = catch_unwind(AssertUnwindSafe(|| {
        let mut provider = (shared.factories[i])();
        let available = provider.availability();
        (provider, available)
    }));
    let Ok((mut provider, available)) = built else {
        fail(shared, born, i);
        return;
    };
    {
        let mut st = shared.lock();
        if st.run != born {
            return;
        }
        st.slots[i].name = provider.name();
        st.slots[i].timeout = provider.timeout();
        if let Err(why) = available {
            // Hardware this PC lacks (AMD's SDK on an Intel PC): no source
            // line, no rows.
            if !provider.for_this_pc() {
                st.slots[i].hidden = true;
                st.slots[i].report = Some(Vec::new());
                publish(&mut st);
                return;
            }
            st.slots[i].report = Some(provider.unavailable_readings(&why, Instant::now()));
            st.slots[i].problem = Some(why);
            publish(&mut st);
            return;
        }
    }
    let mut last_kick = shared.lock().kick;
    let mut last_read: Option<Instant> = None;
    loop {
        {
            let mut st = shared.lock();
            loop {
                if st.run != born {
                    return;
                }
                if st.kick != last_kick {
                    last_kick = st.kick;
                    break;
                }
                let wait = match (last_read, st.interval_ms) {
                    (None, _) => break,
                    (Some(_), 0) => Duration::from_secs(1),
                    (Some(t), ms) => match Duration::from_millis(ms).checked_sub(t.elapsed()) {
                        Some(left) if !left.is_zero() => left,
                        _ => break,
                    },
                };
                st = shared.wake.wait_timeout(st, wait).unwrap_or_else(|e| e.into_inner()).0;
            }
            st.slots[i].busy_since = Some(Instant::now());
        }
        let started = Instant::now();
        let Ok(readings) = catch_unwind(AssertUnwindSafe(|| provider.read(started))) else {
            fail(shared, born, i);
            return;
        };
        last_read = Some(started);
        let mut st = shared.lock();
        if st.run != born {
            return;
        }
        st.history.record(&readings);
        let slot = &mut st.slots[i];
        slot.busy_since = None;
        slot.report = Some(readings);
        publish(&mut st);
    }
}

/// Why a provider that panicked reads nothing more.
pub const FAILED: &str = "stopped with an error";
/// Why a provider stuck past its timeout shows no values.
pub const NOT_RESPONDING: &str = "not responding";

/// A provider panicked: it is dropped (its DLL unloaded) and its sensors
/// stay unavailable until the next run; the others go on.
fn fail(shared: &Shared, born: u64, i: usize) {
    let mut st = shared.lock();
    if st.run != born {
        return;
    }
    let slot = &mut st.slots[i];
    if slot.name.is_empty() {
        slot.name = "provider";
    }
    slot.busy_since = None;
    slot.problem = Some(FAILED.into());
    slot.report.get_or_insert_with(Vec::new);
    publish(&mut st);
}

impl Slot {
    /// Why its readings show no value now (unavailable, failed, stuck).
    fn why(&self, now: Instant) -> Option<String> {
        self.problem.clone().or_else(|| self.stuck(now).then(|| NOT_RESPONDING.to_string()))
    }

    /// Its last readings, greyed with the reason when it has a problem.
    fn readings(&self, now: Instant) -> Vec<Reading> {
        let mut readings = self.report.clone().unwrap_or_default();
        if let Some(why) = self.why(now) {
            for r in &mut readings {
                (r.value, r.words, r.unavailable) = (None, None, Some(why.clone()));
            }
        }
        readings
    }
}

/// Rebuilds the frame once every provider answered (or is stuck).
fn publish(st: &mut State) {
    let now = Instant::now();
    if st.frame.is_none() && !st.slots.iter().all(|s| s.report.is_some() || s.stuck(now)) {
        return;
    }
    st.seq += 1;
    let reports: Vec<Vec<Reading>> = st.slots.iter().map(|s| s.readings(now)).collect();
    let sources = st.slots.iter().filter(|s| !s.hidden).map(|s| SourceStatus { name: if s.name.is_empty() { "provider" } else { s.name }, problem: s.why(now) }).collect();
    let frame = Arc::new(Frame::build(st.seq, &reports, sources));
    Arc::make_mut(&mut st.stats).observe(&frame);
    st.frame = Some(frame);
}

/// A provider got stuck since the frame was built (nobody else published
/// meanwhile), so the frame still shows its old values.
fn newly_stuck(st: &State) -> bool {
    let now = Instant::now();
    // The frame lists only the slots that are not hidden, in slot order.
    st.frame.as_ref().is_some_and(|f| st.slots.iter().filter(|s| !s.hidden).zip(&f.sources).any(|(s, shown)| shown.problem.is_none() && s.stuck(now)))
}

/// Someone looking at the sensors. The service runs while one is alive.
pub struct Interest(Arc<Shared>);

impl Interest {
    /// The latest frame; `None` until every provider answered once
    /// ("Waiting for all sensors to report...").
    pub fn latest(&self) -> Option<Arc<Frame>> {
        let mut st = self.0.lock();
        if st.frame.is_none() || newly_stuck(&st) {
            publish(&mut st);
        }
        st.frame.clone()
    }

    /// The values of a metric over the history window, oldest first.
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn history(&self, id: &str) -> Vec<f64> {
        self.0.lock().history.values(id)
    }

    /// The values of a metric spread by time over `slots` graph points
    /// covering the window (see [`History::graph`]).
    pub fn history_graph(&self, id: &str, slots: usize) -> Vec<f64> {
        self.0.lock().history.graph(id, slots)
    }

    /// The time the graph of a metric shows ("Last minute",
    /// "Last 40 seconds" while it fills).
    pub fn history_caption(&self, id: &str) -> String {
        self.0.lock().history.caption(id)
    }

    /// The time every metric's history covers, shared by every interest
    /// ([`DEFAULT_WINDOW`] until set).
    pub fn set_history_window(&self, window: Duration) {
        self.0.lock().history.set_window(window);
    }

    /// The statistics of this run (shared by every interest): Min / Avg /
    /// Max of every read published since the epoch. A snapshot, cheap to
    /// take at every draw.
    pub fn stats(&self) -> Arc<Stats> {
        self.0.lock().stats.clone()
    }

    /// "Reset statistics": a new epoch, now, for every interest.
    pub fn reset_stats(&self) {
        Arc::make_mut(&mut self.0.lock().stats).reset(crate::platform::local_secs());
    }

    /// The app's update interval; 0 = Paused (no reads but on Refresh).
    pub fn set_interval_ms(&self, ms: u64) {
        let mut st = self.0.lock();
        if st.interval_ms != ms {
            st.interval_ms = ms;
            self.0.wake.notify_all();
        }
    }

    /// Reads every provider once more, now (Refresh).
    pub fn read_now(&self) {
        self.0.lock().kick += 1;
        self.0.wake.notify_all();
    }
}

impl Drop for Interest {
    fn drop(&mut self) {
        let mut st = self.0.lock();
        st.interest -= 1;
        if st.interest == 0 {
            st.run += 1;
            st.slots.clear();
            st.frame = None;
            st.history = History::new(st.history.window());
            st.stats = Arc::default();
            self.0.wake.notify_all();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Frame, Group, Reading, Unit};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    /// A provider that counts its reads and gives one reading.
    struct Fake {
        name: &'static str,
        reads: Arc<AtomicUsize>,
        delay: Duration,
        problem: Option<String>,
    }

    impl Provider for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn availability(&mut self) -> Result<(), String> {
            self.problem.clone().map_or(Ok(()), Err)
        }
        fn read(&mut self, at: Instant) -> Vec<Reading> {
            std::thread::sleep(self.delay);
            let n = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
            vec![Reading::measure(&format!("{}.reads", self.name), Device::cpu("CPU"), Group::Clocks, "Reads", Unit::Percent, self.name, at).with(Some(n as f64), "")]
        }
        fn timeout(&self) -> Duration {
            Duration::from_millis(300)
        }
    }

    fn fake(name: &'static str, delay_ms: u64, problem: Option<&str>) -> (Factory, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let r = reads.clone();
        let problem = problem.map(String::from);
        let f: Factory = Arc::new(move || Box::new(Fake { name, reads: r.clone(), delay: Duration::from_millis(delay_ms), problem: problem.clone() }));
        (f, reads)
    }

    fn until(what: &str, cond: impl Fn() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn no_thread_runs_while_nobody_asks() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(service.threads(), 0);
        assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing read");
    }

    #[test]
    fn the_service_runs_while_at_least_one_asks() {
        let (f, _) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let first = service.interest();
        let second = service.interest();
        until("a worker", || service.threads() == 1);
        drop(first);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(service.threads(), 1, "still asked by the second");
        drop(second);
        until("no worker", || service.threads() == 0);
    }

    /// A provider whose every read waits for a permit the test sends (so
    /// a test decides when a read ends instead of guessing with sleeps).
    struct Gated {
        name: &'static str,
        /// Reads begun, counted when a read starts (before its permit).
        started: Arc<AtomicUsize>,
        permits: std::sync::mpsc::Receiver<()>,
    }

    impl Provider for Gated {
        fn name(&self) -> &'static str {
            self.name
        }
        fn read(&mut self, at: Instant) -> Vec<Reading> {
            let n = self.started.fetch_add(1, Ordering::SeqCst) + 1;
            let _ = self.permits.recv();
            vec![Reading::measure(&format!("{}.reads", self.name), Device::cpu("CPU"), Group::Clocks, "Reads", Unit::Percent, self.name, at).with(Some(n as f64), "")]
        }
        fn timeout(&self) -> Duration {
            // Never "not responding" while a test holds its permit.
            Duration::from_secs(600)
        }
    }

    /// A gated provider, the count of its reads begun, and its permits.
    fn gated(name: &'static str) -> (Factory, Arc<AtomicUsize>, std::sync::mpsc::Sender<()>) {
        let started = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let rx = Mutex::new(Some(rx));
        let s = started.clone();
        let f: Factory = Arc::new(move || Box::new(Gated { name, started: s.clone(), permits: rx.lock().unwrap().take().expect("built once per test") }));
        (f, started, tx)
    }

    #[test]
    fn it_waits_for_every_provider_before_the_first_frame() {
        let (fast, fast_reads) = fake("fast", 0, None);
        let (slow, slow_reads, permit) = gated("slow");
        let service = SensorService::new(vec![fast, slow]);
        let interest = service.interest();
        until("the fast provider answered and the slow one is reading", || fast_reads.load(Ordering::SeqCst) >= 1 && slow_reads.load(Ordering::SeqCst) >= 1);
        until("the fast provider's report is in", || service.0.lock().slots.first().is_some_and(|s| s.report.is_some()));
        assert!(interest.latest().is_none(), "the slow provider has not answered yet");
        permit.send(()).unwrap();
        until("the first frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        assert!(frame.reading("fast.reads").is_some() && frame.reading("slow.reads").is_some());
    }

    #[test]
    fn a_provider_stuck_past_its_timeout_does_not_hold_the_others() {
        let (ok, _) = fake("ok", 0, None);
        let (stuck, _) = fake("stuck", 3_000, None);
        let service = SensorService::new(vec![ok, stuck]);
        let interest = service.interest();
        until("the frame without the stuck provider", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        assert!(frame.reading("ok.reads").is_some());
        let stuck = frame.sources.iter().find(|s| s.name == "stuck").unwrap();
        assert_eq!(stuck.problem.as_deref(), Some("not responding"));
    }

    /// A provider that answers its first read, then misbehaves: `hang`
    /// sleeps 3 s per read, otherwise it panics (a crashing vendor DLL).
    struct Bad {
        hang: bool,
        reads: usize,
        dropped: Arc<AtomicUsize>,
    }

    impl Provider for Bad {
        fn name(&self) -> &'static str {
            "bad"
        }
        fn read(&mut self, at: Instant) -> Vec<Reading> {
            self.reads += 1;
            if self.reads > 1 {
                if self.hang {
                    std::thread::sleep(Duration::from_secs(3));
                } else {
                    panic!("vendor library crashed");
                }
            }
            vec![Reading::measure("bad.reads", Device::cpu("CPU"), Group::Clocks, "Bad", Unit::Percent, "bad", at).with(Some(1.0), "")]
        }
        fn timeout(&self) -> Duration {
            Duration::from_millis(200)
        }
    }

    impl Drop for Bad {
        fn drop(&mut self) {
            self.dropped.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn bad(hang: bool) -> (Factory, Arc<AtomicUsize>) {
        let dropped = Arc::new(AtomicUsize::new(0));
        let d = dropped.clone();
        (Arc::new(move || Box::new(Bad { hang, reads: 0, dropped: d.clone() })), dropped)
    }

    /// After the bad provider answered once and then failed, only its
    /// reading is unavailable, for `why`, and the other one goes on.
    fn only_its_sensors_go_grey(hang: bool, why: &str) {
        let (ok, _) = fake("ok", 0, None);
        let (b, _) = bad(hang);
        let service = SensorService::new(vec![ok, b]);
        let interest = service.interest();
        interest.set_interval_ms(20);
        until("its sensors unavailable", || {
            interest.latest().and_then(|f| f.reading("bad.reads").map(|r| r.unavailable.clone())).flatten().is_some()
        });
        let frame = interest.latest().unwrap();
        assert_eq!(frame.reading("bad.reads").unwrap().unavailable.as_deref(), Some(why));
        assert_eq!(frame.sources.iter().find(|s| s.name == "bad").unwrap().problem.as_deref(), Some(why));
        let before = frame.reading("ok.reads").unwrap().value.unwrap();
        until("the other provider goes on", || interest.latest().unwrap().reading("ok.reads").unwrap().value.unwrap() > before);
    }

    #[test]
    fn a_provider_that_stops_answering_greys_only_its_own_sensors() {
        only_its_sensors_go_grey(true, "not responding");
    }

    #[test]
    fn a_provider_that_stops_answering_after_a_hidden_source_still_goes_grey() {
        // Nobody else publishes: the frame learns of the hang only by
        // matching each listed source to its own provider.
        let other: Factory = Arc::new(|| Box::new(OtherHardware));
        let (b, _) = bad(true);
        let service = SensorService::new(vec![other, b]);
        let interest = service.interest();
        interest.set_interval_ms(20);
        until("its sensors unavailable", || {
            interest.latest().and_then(|f| f.reading("bad.reads").map(|r| r.unavailable.clone())).flatten().is_some()
        });
        let frame = interest.latest().unwrap();
        assert_eq!(frame.sources.iter().find(|s| s.name == "bad").unwrap().problem.as_deref(), Some(NOT_RESPONDING));
    }

    #[test]
    fn a_provider_that_crashes_greys_only_its_own_sensors() {
        only_its_sensors_go_grey(false, "stopped with an error");
    }

    #[test]
    fn nobody_asking_drops_every_provider_so_their_libraries_unload() {
        let (b, dropped) = bad(false);
        let (ok, _) = fake("ok", 0, None);
        let service = SensorService::new(vec![ok, b]);
        let interest = service.interest();
        until("a frame", || interest.latest().is_some());
        drop(interest);
        until("no worker", || service.threads() == 0);
        assert_eq!(dropped.load(Ordering::SeqCst), 1, "the provider (and its DLL) is gone");
    }

    /// A provider that cannot work here but lists the rows it would fill.
    struct Missing;

    impl Provider for Missing {
        fn name(&self) -> &'static str {
            "sdk"
        }
        fn availability(&mut self) -> Result<(), String> {
            Err("needs AMD's add-on".into())
        }
        fn unavailable_readings(&mut self, why: &str, at: Instant) -> Vec<Reading> {
            vec![Reading::measure("cpu.soc.power_w", Device::cpu("CPU"), Group::Powers, "SoC", Unit::Watt, "sdk", at).with(None, why)]
        }
        fn read(&mut self, _: Instant) -> Vec<Reading> {
            panic!("never read when unavailable")
        }
    }

    #[test]
    fn a_provider_that_cannot_work_here_still_lists_its_rows_with_the_reason() {
        let f: Factory = Arc::new(|| Box::new(Missing));
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        until("the first frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        let soc = frame.reading("cpu.soc.power_w").expect("the row is listed");
        assert_eq!((soc.value, soc.unavailable.as_deref()), (None, Some("needs AMD's add-on")));
        assert_eq!(frame.sources[0].problem.as_deref(), Some("needs AMD's add-on"));
    }

    #[test]
    fn a_provider_that_cannot_work_here_says_why() {
        let (ok, _) = fake("ok", 0, None);
        let (missing, reads) = fake("sdk", 0, Some("needs AMD's add-on"));
        let service = SensorService::new(vec![ok, missing]);
        let interest = service.interest();
        until("the first frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        let sdk = frame.sources.iter().find(|s| s.name == "sdk").unwrap();
        assert_eq!(sdk.problem.as_deref(), Some("needs AMD's add-on"));
        assert_eq!(frame.sources.iter().find(|s| s.name == "ok").unwrap().problem, None);
        assert_eq!(reads.load(Ordering::SeqCst), 0, "never read");
    }

    /// A source for hardware this PC does not have (an AMD SDK on an Intel PC).
    struct OtherHardware;

    impl Provider for OtherHardware {
        fn name(&self) -> &'static str {
            "amd-sdk"
        }
        fn availability(&mut self) -> Result<(), String> {
            Err("AMD processors only".into())
        }
        fn for_this_pc(&mut self) -> bool {
            false
        }
        fn unavailable_readings(&mut self, why: &str, at: Instant) -> Vec<Reading> {
            vec![Reading::measure("amd.package_c", Device::cpu("CPU"), Group::Temperatures, "Package", Unit::Celsius, "amd-sdk", at).with(None, why)]
        }
        fn read(&mut self, _at: Instant) -> Vec<Reading> {
            unreachable!("never available")
        }
    }

    #[test]
    fn a_source_for_hardware_this_pc_lacks_is_not_listed_and_shows_no_rows() {
        let (ok, _) = fake("ok", 0, None);
        let (missing, _) = fake("sdk", 0, Some("needs AMD's add-on"));
        let other: Factory = Arc::new(|| Box::new(OtherHardware));
        let service = SensorService::new(vec![ok, missing, other]);
        let interest = service.interest();
        until("the first frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        let names: Vec<_> = frame.sources.iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["ok", "sdk"], "the AMD source is not listed on an Intel PC; one with a problem still is");
        assert!(frame.readings().all(|r| r.source != "amd-sdk"), "and none of its rows shows");
    }

    #[test]
    fn it_reads_at_the_update_speed_and_not_while_paused() {
        let (f, reads, permit) = gated("a");
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        interest.set_interval_ms(10);
        for _ in 0..4 {
            permit.send(()).unwrap();
        }
        // Four reads ended at the update speed; the fifth waits for its permit.
        until("several reads", || reads.load(Ordering::SeqCst) == 5);
        interest.set_interval_ms(0);
        // The read in progress ends; Paused is set, so no other begins.
        for _ in 0..10 {
            permit.send(()).unwrap();
        }
        until("the read in progress is shown", || interest.latest().and_then(|f| f.reading("a.reads").and_then(|r| r.value)) == Some(5.0));
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(reads.load(Ordering::SeqCst), 5, "Paused: no reads");
        interest.read_now();
        until("one read on Refresh", || reads.load(Ordering::SeqCst) == 6);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::SeqCst), 6, "only one");
    }

    #[test]
    fn each_new_frame_has_a_higher_number_even_after_a_restart() {
        let (f, _) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        until("a frame", || interest.latest().is_some());
        let first = interest.latest().unwrap().seq;
        drop(interest);
        let interest = service.interest();
        assert!(interest.latest().is_none(), "a new run waits again");
        until("a frame", || interest.latest().is_some());
        assert!(interest.latest().unwrap().seq > first);
    }

    /// A frame with one reading of `id` worth `v`, read `secs` after `t0`.
    fn point(id: &str, t0: Instant, secs: f64, v: f64) -> Frame {
        let at = t0 + Duration::from_secs_f64(secs);
        Frame::build(1, &[vec![Reading::measure(id, Device::cpu("CPU"), Group::Clocks, id, Unit::Hz, "t", at).with(Some(v), "")]], Vec::new())
    }

    /// Feeds `history` one reading every `step` seconds for `secs` seconds.
    fn feed(history: &mut History, t0: Instant, from: f64, secs: f64, step: f64) {
        let mut s = from;
        while s <= from + secs + 1e-9 {
            history.record(point("clock", t0, s, s).readings());
            s += step;
        }
    }

    #[test]
    fn history_drops_points_older_than_the_window_whatever_the_interval() {
        for step in [0.5, 1.0, 2.0] {
            let t0 = Instant::now();
            let mut history = History::new(Duration::from_secs(60));
            feed(&mut history, t0, 0.0, 300.0, step);
            let values = history.values("clock");
            assert_eq!((values.first().copied(), values.last().copied()), (Some(240.0), Some(300.0)), "at {step} s the last 60 s are kept");
            assert_eq!(history.caption("clock"), "Last minute", "at {step} s");
        }
    }

    #[test]
    fn the_caption_states_the_window_or_the_real_span_while_filling() {
        let t0 = Instant::now();
        let mut two = History::new(Duration::from_secs(120));
        assert_eq!(two.caption("clock"), "Last 2 minutes", "nothing read yet: the window");
        feed(&mut two, t0, 0.0, 40.0, 2.0);
        assert_eq!(two.caption("clock"), "Last 40 seconds", "filling");
        feed(&mut two, t0, 42.0, 200.0, 2.0);
        assert_eq!(two.caption("clock"), "Last 2 minutes");
        let mut one = History::new(Duration::from_secs(60));
        feed(&mut one, t0, 0.0, 0.0, 1.0);
        assert_eq!(one.caption("clock"), "Last second", "a single point");
    }

    #[test]
    fn changing_the_update_speed_does_not_shrink_the_history() {
        let t0 = Instant::now();
        let mut history = History::new(Duration::from_secs(60));
        feed(&mut history, t0, 0.0, 120.0, 2.0);
        feed(&mut history, t0, 120.5, 5.0, 0.5);
        let values = history.values("clock");
        assert_eq!(values.first().copied(), Some(66.0), "still a full minute after speeding up");
        assert_eq!(history.caption("clock"), "Last minute");
    }

    #[test]
    fn the_graph_spreads_the_points_over_the_window_by_time() {
        let t0 = Instant::now();
        let mut history = History::new(Duration::from_secs(60));
        feed(&mut history, t0, 0.0, 100.0, 2.0);
        let graph = history.graph("clock", 61);
        assert_eq!(graph.len(), 61, "a full window fills the graph at 2 s too");
        assert_eq!((graph[0], graph[1], graph[60]), (40.0, 41.0, 100.0), "one slot per second, in between interpolated");
        let mut filling = History::new(Duration::from_secs(60));
        feed(&mut filling, t0, 0.0, 30.0, 0.5);
        assert_eq!(filling.graph("clock", 61).len(), 31, "half a window fills half the graph");
        assert!(History::new(Duration::from_secs(60)).graph("clock", 61).is_empty());
    }

    #[test]
    fn it_keeps_the_last_window_of_each_metric_by_age() {
        let (f, _) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        interest.set_history_window(Duration::from_millis(20));
        interest.set_interval_ms(5);
        // Kept by count (60 points), the first read would stay for 60 reads.
        until("the first read left the window", || interest.history("a.reads").first().is_some_and(|&v| v > 1.0));
        let h = interest.history("a.reads");
        assert!(h.windows(2).all(|w| w[0] < w[1]), "oldest first");
        assert!(h.len() < 60, "trimmed by age: {} points kept of 20 ms read every 5 ms", h.len());
        assert!(interest.history_caption("a.reads").starts_with("Last "));
    }

    #[test]
    fn its_history_is_dropped_when_nobody_asks_any_more() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        interest.set_interval_ms(1);
        until("many reads", || reads.load(Ordering::SeqCst) > 10);
        // A read is counted as it begins and kept once it ends.
        until("a minute keeps every read", || interest.history("a.reads").len() > 10);
        interest.set_interval_ms(1000);
        drop(interest);
        let interest = service.interest();
        assert!(interest.history("a.reads").len() <= 1, "a new run starts a new history");
    }

    #[test]
    fn statistics_count_every_published_frame() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        interest.set_interval_ms(1);
        until("a few reads", || reads.load(Ordering::SeqCst) > 3);
        let stats = interest.stats();
        let (lo, hi) = stats.min_max("a.reads").expect("frames counted");
        assert_eq!(lo, 1.0, "the first frame counts");
        assert!(hi > lo);
        let avg = stats.avg("a.reads").unwrap();
        assert!(lo < avg && avg <= hi, "avg {avg} within [{lo}, {hi}]");
    }

    #[test]
    fn a_slow_provider_is_counted_once_per_read_not_once_per_frame() {
        let (fast, fast_reads) = fake("fast", 0, None);
        let (slow, slow_reads) = fake("slow", 40, None);
        let service = SensorService::new(vec![fast, slow]);
        let interest = service.interest();
        interest.set_interval_ms(1);
        until("a few slow reads", || slow_reads.load(Ordering::SeqCst) >= 4);
        let stats = interest.stats();
        let read = slow_reads.load(Ordering::SeqCst) as u64;
        assert!(fast_reads.load(Ordering::SeqCst) as u64 > read, "the fast provider published more frames");
        let counted = stats.samples("slow.reads");
        assert!((1..=read).contains(&counted), "the slow value counted {counted} times, read {read} times");
        assert!(stats.samples("fast.reads") <= fast_reads.load(Ordering::SeqCst) as u64);
    }

    #[test]
    fn statistics_survive_while_any_interest_is_held() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let recorder = service.interest();
        let page = service.interest();
        page.set_interval_ms(1);
        until("a few reads", || reads.load(Ordering::SeqCst) > 3);
        let since = page.stats().since();
        drop(page); // the page is left
        let page = service.interest(); // and reopened
        assert_eq!(page.stats().min_max("a.reads").map(|(lo, _)| lo), Some(1.0), "the first frame is still counted");
        assert_eq!(page.stats().since(), since, "same epoch");
        drop(recorder);
    }

    #[test]
    fn statistics_clear_when_the_last_interest_drops() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        interest.set_interval_ms(1);
        until("a few reads", || reads.load(Ordering::SeqCst) > 3);
        interest.set_interval_ms(1000);
        drop(interest);
        let interest = service.interest();
        let lo = interest.stats().min_max("a.reads").map(|(lo, _)| lo);
        assert!(lo.is_none_or(|lo| lo > 1.0), "a new run starts new statistics, got {lo:?}");
    }

    #[test]
    fn reset_statistics_starts_a_new_epoch_for_every_interest() {
        let (f, reads) = fake("a", 0, None);
        let service = SensorService::new(vec![f]);
        let page = service.interest();
        let tile = service.interest();
        page.set_interval_ms(1);
        until("a few reads", || reads.load(Ordering::SeqCst) > 3);
        // Only the read in progress (value `before`) can still publish.
        let before = reads.load(Ordering::SeqCst) as f64;
        page.reset_stats();
        let lo = tile.stats().min_max("a.reads").map(|(lo, _)| lo);
        assert!(lo.is_none_or(|lo| lo >= before), "only frames after the reset count, got {lo:?} (reads before the reset {before})");
        assert!(tile.stats().since() >= page.stats().since() && page.stats().since() > 0, "the epoch is the reset's local time");
        until("counted again", || tile.stats().min_max("a.reads").is_some());
    }
}
