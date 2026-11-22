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

use super::model::{CounterSet, Frame, Reading, SourceStatus};
use super::stats::Stats;
use crate::perfdata::StaticInfo;
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

/// Which providers an [`Interest`] wakes. The service starts a group's
/// providers with the first interest that asks for the group and stops them
/// with the last, so something that is on for the whole app run (the
/// Sampler) can read the performance counters without keeping the hardware
/// sensors on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderGroup {
    /// Windows performance counters (processor information, disks, energy
    /// meter): what the core pages are built from. Read once when the group
    /// starts and then only when asked ([`Interest::read_now`]): the
    /// Sampler asks at every tick.
    Counters,
    /// Everything that talks to hardware or vendor libraries: read only
    /// while a page, tile, recording or alert looks at it.
    Hardware,
}

impl ProviderGroup {
    const ALL: [ProviderGroup; 2] = [ProviderGroup::Counters, ProviderGroup::Hardware];

    fn index(self) -> usize {
        self as usize
    }
}

/// The groups an interest in everything wakes, and in the counters only.
const EVERYTHING: &[ProviderGroup] = &[ProviderGroup::Counters, ProviderGroup::Hardware];
const COUNTERS_ONLY: &[ProviderGroup] = &[ProviderGroup::Counters];
const HARDWARE_ONLY: &[ProviderGroup] = &[ProviderGroup::Hardware];

/// A Refresh request: what [`Interest::latest_after`] waits for (the
/// Refresh count of each group, at the time of the request).
#[derive(Clone, Debug, Default)]
pub struct Ticket([u64; 2]);

/// One provider's place in a run.
struct Slot {
    group: ProviderGroup,
    /// Its group has an interest: its worker lives.
    running: bool,
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
    /// The group's Refresh count when the read in progress began.
    started_kick: u64,
    /// The same, for the read `report` comes from.
    report_kick: u64,
    /// What it counted in that read.
    counted: Vec<CounterSet>,
    /// The machine facts it reports, if it does.
    machine: Option<Arc<StaticInfo>>,
}

impl Slot {
    /// A slot of a group nobody asks for.
    fn idle(group: ProviderGroup) -> Self {
        Slot { group, running: false, name: "", report: None, problem: None, busy_since: None, timeout: super::provider::DEFAULT_TIMEOUT, started: Instant::now(), hidden: false, started_kick: 0, report_kick: 0, counted: Vec::new(), machine: None }
    }

    fn stuck(&self, now: Instant) -> bool {
        let since = self.busy_since.or(self.report.is_none().then_some(self.started));
        self.running && since.is_some_and(|t| now.duration_since(t) > self.timeout)
    }

    /// It has answered once (or never will: it is stuck).
    fn settled(&self, now: Instant) -> bool {
        self.report.is_some() || self.stuck(now)
    }
}

/// What the service keeps for one group of providers.
struct GroupState {
    interest: usize,
    /// Bumped at every start and stop of the group: its workers of another
    /// run exit.
    run: u64,
    interval_ms: u64,
    /// Bumped by "Refresh": the group's workers read once more.
    kick: u64,
}

struct State {
    groups: [GroupState; 2],
    /// One per factory, in order; the ones of a group nobody asks for idle.
    slots: Vec<Slot>,
    frame: Option<Arc<Frame>>,
    /// Per slot: it showed a problem in `frame` (so a newly stuck one is
    /// noticed).
    shown: Vec<bool>,
    /// Never reset, so frames of a later run are newer.
    seq: u64,
    history: History,
    /// Min / Avg / Max of every frame published in this run; readers get
    /// the snapshot (copied on the next publish only while one is held).
    stats: Arc<Stats>,
}

impl State {
    fn wanted(&self, group: ProviderGroup) -> bool {
        self.groups[group.index()].interest > 0
    }

    /// Every running provider of `groups` has answered (or is stuck).
    fn settled(&self, groups: &[ProviderGroup], now: Instant) -> bool {
        self.slots.iter().filter(|s| s.running && groups.contains(&s.group)).all(|s| s.settled(now))
    }
}

struct Shared {
    factories: Vec<(ProviderGroup, Factory)>,
    state: Mutex<State>,
    wake: Condvar,
    threads: [AtomicUsize; 2],
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
    /// A service over these providers, all of the [`ProviderGroup::Hardware`]
    /// group; nothing runs until someone asks.
    pub fn new(factories: Vec<Factory>) -> Self {
        Self::grouped(factories.into_iter().map(|f| (ProviderGroup::Hardware, f)).collect())
    }

    /// A service over providers in groups; nothing runs until someone asks.
    pub fn grouped(factories: Vec<(ProviderGroup, Factory)>) -> Self {
        let group = || GroupState { interest: 0, run: 0, interval_ms: 1000, kick: 0 };
        let slots = factories.iter().map(|(g, _)| Slot::idle(*g)).collect();
        let state = State { groups: [group(), group()], slots, frame: None, shown: Vec::new(), seq: 0, history: History::default(), stats: Arc::default() };
        SensorService(Arc::new(Shared { factories, state: Mutex::new(state), wake: Condvar::new(), threads: [AtomicUsize::new(0), AtomicUsize::new(0)] }))
    }

    /// Asks for every sensor: the groups run while at least one [`Interest`]
    /// in them is alive.
    pub fn interest(&self) -> Interest {
        self.ask(EVERYTHING)
    }

    /// Asks for the sensors of one group only.
    pub fn interest_in(&self, group: ProviderGroup) -> Interest {
        self.ask(match group {
            ProviderGroup::Counters => COUNTERS_ONLY,
            ProviderGroup::Hardware => HARDWARE_ONLY,
        })
    }

    fn ask(&self, scope: &'static [ProviderGroup]) -> Interest {
        let mut st = self.0.lock();
        for &group in scope {
            start_group(&self.0, &mut st, group);
        }
        Interest { shared: self.0.clone(), scope }
    }

    /// Worker threads of one group alive now.
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn threads_in(&self, group: ProviderGroup) -> usize {
        self.0.threads[group.index()].load(Ordering::SeqCst)
    }

    /// Worker threads alive now (0 while nobody asks, once old reads end).
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn threads(&self) -> usize {
        ProviderGroup::ALL.iter().map(|g| self.threads_in(*g)).sum()
    }
}

/// The first interest in a group starts its workers.
fn start_group(shared: &Arc<Shared>, st: &mut State, group: ProviderGroup) {
    let gs = &mut st.groups[group.index()];
    gs.interest += 1;
    if gs.interest > 1 {
        return;
    }
    gs.run += 1;
    let born = gs.run;
    let now = Instant::now();
    if group == ProviderGroup::Hardware {
        st.stats = Arc::new(Stats::new(crate::platform::local_secs()));
    }
    for i in 0..shared.factories.len() {
        if shared.factories[i].0 != group {
            continue;
        }
        st.slots[i] = Slot { running: true, started: now, ..Slot::idle(group) };
        let shared = shared.clone();
        shared.threads[group.index()].fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let _alive = Alive(&shared, group);
            work(&shared, born, i);
        });
    }
    if st.frame.is_some() {
        // The frame lists the sources of the running providers.
        publish(st, false);
    }
}

/// The last interest in a group ends its run at once, without waiting for
/// the reads in progress.
fn stop_group(shared: &Shared, st: &mut State, group: ProviderGroup) {
    let gs = &mut st.groups[group.index()];
    gs.interest -= 1;
    if gs.interest > 0 {
        return;
    }
    gs.run += 1;
    for slot in st.slots.iter_mut().filter(|s| s.group == group) {
        *slot = Slot::idle(group);
    }
    let window = st.history.window();
    if group == ProviderGroup::Hardware || !ProviderGroup::ALL.iter().any(|g| st.wanted(*g)) {
        st.history = History::new(window);
        st.stats = Arc::default();
    }
    st.frame = None;
    publish(st, false);
    shared.wake.notify_all();
}

/// Counts a worker thread while it lives.
struct Alive<'a>(&'a Shared, ProviderGroup);

impl Drop for Alive<'_> {
    fn drop(&mut self) {
        self.0.threads[self.1.index()].fetch_sub(1, Ordering::SeqCst);
    }
}

/// A worker: builds its provider, then reads it once at once and then at
/// every interval of its group (or Refresh), until its run ends.
fn work(shared: &Shared, born: u64, i: usize) {
    let group = shared.factories[i].0.index();
    let on_demand = shared.factories[i].0 == ProviderGroup::Counters;
    // A provider that panics (a crashing vendor library) fails alone.
    let built = catch_unwind(AssertUnwindSafe(|| {
        let mut provider = (shared.factories[i].1)();
        let available = provider.availability();
        (provider, available)
    }));
    let Ok((mut provider, available)) = built else {
        fail(shared, born, i);
        return;
    };
    {
        let mut st = shared.lock();
        if st.groups[group].run != born {
            return;
        }
        st.slots[i].name = provider.name();
        st.slots[i].timeout = provider.timeout();
        st.slots[i].hidden = !provider.listed();
        if let Err(why) = available {
            // Hardware this PC lacks (AMD's SDK on an Intel PC): no source
            // line, no rows.
            if !provider.for_this_pc() {
                st.slots[i].hidden = true;
                st.slots[i].report = Some(Vec::new());
                publish(&mut st, false);
                return;
            }
            st.slots[i].report = Some(provider.unavailable_readings(&why, Instant::now()));
            st.slots[i].problem = Some(why);
            publish(&mut st, false);
            return;
        }
    }
    let mut last_kick = shared.lock().groups[group].kick;
    let mut last_read: Option<Instant> = None;
    loop {
        let hardware_wanted;
        {
            let mut st = shared.lock();
            loop {
                if st.groups[group].run != born {
                    return;
                }
                if st.groups[group].kick != last_kick {
                    last_kick = st.groups[group].kick;
                    break;
                }
                let wait = match (last_read, st.groups[group].interval_ms) {
                    (None, _) => break,
                    // The counters keep no clock of their own: the Sampler
                    // asks for each read (see `Interest::set_interval_ms`).
                    (Some(_), _) if on_demand => Duration::from_secs(1),
                    (Some(_), 0) => Duration::from_secs(1),
                    (Some(t), ms) => match Duration::from_millis(ms).checked_sub(t.elapsed()) {
                        Some(left) if !left.is_zero() => left,
                        _ => break,
                    },
                };
                st = shared.wake.wait_timeout(st, wait).unwrap_or_else(|e| e.into_inner()).0;
            }
            st.slots[i].busy_since = Some(Instant::now());
            st.slots[i].started_kick = st.groups[group].kick;
            hardware_wanted = st.wanted(ProviderGroup::Hardware);
        }
        let started = Instant::now();
        let Ok((readings, counted)) = catch_unwind(AssertUnwindSafe(|| {
            provider.hardware_wanted(hardware_wanted);
            let readings = provider.read(started);
            (readings, provider.counted())
        })) else {
            fail(shared, born, i);
            return;
        };
        last_read = Some(started);
        let mut st = shared.lock();
        if st.groups[group].run != born {
            return;
        }
        if st.wanted(ProviderGroup::Hardware) {
            st.history.record(&readings);
        }
        let slot = &mut st.slots[i];
        slot.busy_since = None;
        slot.report = Some(readings);
        slot.counted = counted;
        slot.report_kick = slot.started_kick;
        let facts_known = slot.machine.is_some();
        publish(&mut st, false);
        drop(st);
        if facts_known {
            continue;
        }
        // The machine's facts are asked once the read is out, so a slow one
        // never holds up the first figures: they join the frame when read.
        let Ok(facts) = catch_unwind(AssertUnwindSafe(|| provider.machine())) else {
            fail(shared, born, i);
            return;
        };
        if let Some(facts) = facts {
            let mut st = shared.lock();
            if st.groups[group].run != born {
                return;
            }
            st.slots[i].machine = Some(facts);
            publish(&mut st, false);
        }
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
    if st.groups[shared.factories[i].0.index()].run != born {
        return;
    }
    let slot = &mut st.slots[i];
    if slot.name.is_empty() {
        slot.name = "provider";
    }
    slot.busy_since = None;
    slot.problem = Some(FAILED.into());
    slot.report.get_or_insert_with(Vec::new);
    publish(&mut st, false);
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

impl Slot {
    /// What it counted, nothing while it shows a problem (a gap, not an old
    /// figure).
    fn counters(&self, now: Instant) -> &[CounterSet] {
        if self.why(now).is_some() { &[] } else { &self.counted }
    }
}

/// The counted sets of the running providers, one per key: the first
/// provider that answered a counter gives it.
fn merge_counters(st: &State, now: Instant) -> Vec<CounterSet> {
    let mut merged: Vec<CounterSet> = Vec::new();
    for set in st.slots.iter().filter(|s| s.running).flat_map(|s| s.counters(now)) {
        match merged.iter_mut().find(|m| m.key == set.key) {
            Some(m) if !m.answered() && set.answered() => *m = set.clone(),
            Some(_) => {}
            None => merged.push(set.clone()),
        }
    }
    merged
}

/// Rebuilds the frame of the running providers once a group of them has
/// answered (or is stuck); `force` builds it even for no provider at all.
fn publish(st: &mut State, force: bool) {
    let now = Instant::now();
    let running = |g: &ProviderGroup| st.wanted(*g) && st.slots.iter().any(|s| s.running && s.group == *g);
    let answered = ProviderGroup::ALL.iter().any(|g| running(g) && st.settled(&[*g], now));
    if st.frame.is_none() && !answered && !force {
        return;
    }
    st.seq += 1;
    let live = || st.slots.iter().filter(|s| s.running);
    let reports: Vec<Vec<Reading>> = live().map(|s| s.readings(now)).collect();
    let sources = live().filter(|s| !s.hidden).map(|s| SourceStatus { name: if s.name.is_empty() { "provider" } else { s.name }, problem: s.why(now) }).collect();
    let mut frame = Frame::build(st.seq, &reports, sources);
    frame.counters = merge_counters(st, now);
    frame.machine = st.slots.iter().filter(|s| s.running).find_map(|s| s.machine.clone());
    let frame = Arc::new(frame);
    st.shown = st.slots.iter().map(|s| s.running && s.why(now).is_some()).collect();
    if st.settled(&ProviderGroup::ALL, now) {
        Arc::make_mut(&mut st.stats).observe(&frame);
    }
    st.frame = Some(frame);
}

/// A provider got stuck since the frame was built (nobody else published
/// meanwhile), so the frame still shows its old values.
fn newly_stuck(st: &State) -> bool {
    let now = Instant::now();
    st.frame.is_some() && st.slots.iter().enumerate().any(|(i, s)| s.stuck(now) && !st.shown.get(i).copied().unwrap_or(false))
}

/// Someone looking at the sensors of some groups. Their providers run while
/// one is alive.
pub struct Interest {
    shared: Arc<Shared>,
    scope: &'static [ProviderGroup],
}

impl Interest {
    /// The latest frame; `None` until every provider of this interest's
    /// groups answered once ("Waiting for all sensors to report...").
    pub fn latest(&self) -> Option<Arc<Frame>> {
        let mut st = self.shared.lock();
        self.frame(&mut st, |st, now| st.settled(self.scope, now))
    }

    /// The latest frame once every provider of this interest's groups has
    /// read after the Refresh `ticket` asked for (or cannot: it has a
    /// problem or is stuck); `None` until then.
    pub fn latest_after(&self, ticket: &Ticket) -> Option<Arc<Frame>> {
        let mut st = self.shared.lock();
        self.frame(&mut st, |st, now| {
            st.slots.iter().filter(|s| s.running && self.scope.contains(&s.group)).all(|s| s.problem.is_some() || s.stuck(now) || (s.report.is_some() && s.report_kick >= ticket.0[s.group.index()]))
        })
    }

    fn frame(&self, st: &mut State, ready: impl Fn(&State, Instant) -> bool) -> Option<Arc<Frame>> {
        if st.frame.is_none() || newly_stuck(st) {
            publish(st, false);
        }
        if !ready(st, Instant::now()) {
            return None;
        }
        if st.frame.is_none() {
            publish(st, true);
        }
        st.frame.clone()
    }

    /// The values of a metric over the history window, oldest first.
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn history(&self, id: &str) -> Vec<f64> {
        self.shared.lock().history.values(id)
    }

    /// The values of a metric spread by time over `slots` graph points
    /// covering the window (see [`History::graph`]).
    pub fn history_graph(&self, id: &str, slots: usize) -> Vec<f64> {
        self.shared.lock().history.graph(id, slots)
    }

    /// The time the graph of a metric shows ("Last minute",
    /// "Last 40 seconds" while it fills).
    pub fn history_caption(&self, id: &str) -> String {
        self.shared.lock().history.caption(id)
    }

    /// The time every metric's history covers, shared by every interest
    /// ([`DEFAULT_WINDOW`] until set).
    pub fn set_history_window(&self, window: Duration) {
        self.shared.lock().history.set_window(window);
    }

    /// The statistics of this run (shared by every interest): Min / Avg /
    /// Max of every read published since the epoch. A snapshot, cheap to
    /// take at every draw.
    pub fn stats(&self) -> Arc<Stats> {
        self.shared.lock().stats.clone()
    }

    /// "Reset statistics": a new epoch, now, for every interest.
    pub fn reset_stats(&self) {
        Arc::make_mut(&mut self.shared.lock().stats).reset(crate::platform::local_secs());
    }

    /// How often the hardware is read (an add-on's or a page's interval);
    /// 0 = Paused (no reads but on Refresh). It is the interval of the
    /// [`ProviderGroup::Hardware`] group when this interest asks for it (the
    /// last one to set it wins). The counters have no interval: the Sampler
    /// reads them once per tick, at the app's update speed
    /// ([`Interest::read_now`]), whatever any add-on asks.
    pub fn set_interval_ms(&self, ms: u64) {
        let mut st = self.shared.lock();
        let mut changed = false;
        for g in self.scope.iter().filter(|g| **g == ProviderGroup::Hardware) {
            let gs = &mut st.groups[g.index()];
            changed |= gs.interval_ms != ms;
            gs.interval_ms = ms;
        }
        if changed {
            self.shared.wake.notify_all();
        }
    }

    /// Reads every provider of this interest's groups once more, now
    /// (Refresh). The ticket says when to look for the result with
    /// [`Interest::latest_after`].
    pub fn read_now(&self) -> Ticket {
        let mut ticket = Ticket::default();
        {
            let mut st = self.shared.lock();
            for g in self.scope {
                let gs = &mut st.groups[g.index()];
                gs.kick += 1;
                ticket.0[g.index()] = gs.kick;
            }
        }
        self.shared.wake.notify_all();
        ticket
    }
}

impl Drop for Interest {
    fn drop(&mut self) {
        let mut st = self.shared.lock();
        for &group in self.scope {
            stop_group(&self.shared, &mut st, group);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Group, Instance, Unit};

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

    // ---- provider groups (spec #77, D1)

    fn two_groups() -> (SensorService, Arc<AtomicUsize>, Arc<AtomicUsize>) {
        let (counters, counter_reads) = fake("counters", 0, None);
        let (hardware, hardware_reads) = fake("hardware", 0, None);
        (SensorService::grouped(vec![(ProviderGroup::Counters, counters), (ProviderGroup::Hardware, hardware)]), counter_reads, hardware_reads)
    }

    #[test]
    fn an_interest_in_the_counters_wakes_only_the_counter_providers() {
        let (service, counter_reads, hardware_reads) = two_groups();
        let _interest = service.interest_in(ProviderGroup::Counters);
        until("the counters read", || counter_reads.load(Ordering::SeqCst) >= 1);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(service.threads_in(ProviderGroup::Hardware), 0, "no hardware thread");
        assert_eq!(service.threads(), 1);
        assert_eq!(hardware_reads.load(Ordering::SeqCst), 0, "the hardware is never read");
    }

    #[test]
    fn the_hardware_stops_with_the_last_surface_while_the_counters_go_on() {
        let (service, counter_reads, hardware_reads) = two_groups();
        let always = service.interest_in(ProviderGroup::Counters);
        let page = service.interest();
        until("both groups read", || counter_reads.load(Ordering::SeqCst) >= 1 && hardware_reads.load(Ordering::SeqCst) >= 1);
        assert_eq!(service.threads(), 2);
        drop(page);
        until("the hardware stopped", || service.threads_in(ProviderGroup::Hardware) == 0);
        assert_eq!(service.threads_in(ProviderGroup::Counters), 1, "the counters are still read");
        assert!(always.latest().is_some());
        drop(always);
        until("nothing runs", || service.threads() == 0);
    }

    #[test]
    fn the_counters_do_not_wait_for_slow_hardware() {
        let (counters, _) = fake("counters", 0, None);
        let (slow, slow_reads, permit) = gated("slow");
        let service = SensorService::grouped(vec![(ProviderGroup::Counters, counters), (ProviderGroup::Hardware, slow)]);
        let always = service.interest_in(ProviderGroup::Counters);
        let page = service.interest();
        until("the slow hardware is reading", || slow_reads.load(Ordering::SeqCst) >= 1);
        until("the counters' first frame", || always.latest().is_some());
        assert!(page.latest().is_none(), "a page still waits for every provider it asked for");
        let frame = always.latest().unwrap();
        assert!(frame.reading("counters.reads").is_some());
        permit.send(()).unwrap();
        until("the page's first frame", || page.latest().is_some());
        assert!(page.latest().unwrap().reading("slow.reads").is_some());
    }

    #[test]
    fn an_update_speed_sets_the_hardwares_and_never_the_counters_which_are_read_when_asked() {
        let (service, counter_reads, hardware_reads) = two_groups();
        let always = service.interest_in(ProviderGroup::Counters);
        let page = service.interest();
        until("one read each", || counter_reads.load(Ordering::SeqCst) >= 1 && hardware_reads.load(Ordering::SeqCst) >= 1);
        always.set_interval_ms(10);
        page.set_interval_ms(10);
        until("the hardware reads fast", || hardware_reads.load(Ordering::SeqCst) >= 5);
        assert_eq!(counter_reads.load(Ordering::SeqCst), 1, "no interval reads the counters");
        always.read_now();
        until("the counters read when asked", || counter_reads.load(Ordering::SeqCst) == 2);
    }

    #[test]
    fn a_refresh_ticket_is_met_only_by_a_read_that_began_after_it() {
        let (f, started, permit) = gated("a");
        let service = SensorService::grouped(vec![(ProviderGroup::Counters, f)]);
        let interest = service.interest_in(ProviderGroup::Counters);
        permit.send(()).unwrap();
        until("the first frame", || interest.latest().is_some());
        interest.set_interval_ms(0);
        let ticket = interest.read_now();
        until("the Refresh read began", || started.load(Ordering::SeqCst) == 2);
        assert!(interest.latest().is_some(), "the old frame is still the latest");
        assert!(interest.latest_after(&ticket).is_none(), "but it was read before the Refresh");
        permit.send(()).unwrap();
        until("the frame after the Refresh", || interest.latest_after(&ticket).is_some());
        assert_eq!(interest.latest_after(&ticket).unwrap().reading("a.reads").and_then(|r| r.value), Some(2.0));
    }

    // ---- counters: what is more than one plain number

    /// A provider that counts disks: "0 C:" with a raw total, and "_Total".
    struct Counting {
        name: &'static str,
        sets: Vec<CounterSet>,
        delay: Duration,
    }

    impl Provider for Counting {
        fn name(&self) -> &'static str {
            self.name
        }
        fn read(&mut self, _at: Instant) -> Vec<Reading> {
            std::thread::sleep(self.delay);
            Vec::new()
        }
        fn counted(&mut self) -> Vec<CounterSet> {
            self.sets.clone()
        }
        fn timeout(&self) -> Duration {
            Duration::from_millis(300)
        }
    }

    fn counting(name: &'static str, delay_ms: u64, sets: Vec<CounterSet>) -> Factory {
        Arc::new(move || Box::new(Counting { name, sets: sets.clone(), delay: Duration::from_millis(delay_ms) }))
    }

    fn disk_sets() -> Vec<CounterSet> {
        vec![CounterSet::new("dread", vec![Instance::new("0 C:", 2_000.0).with_raw(9_000_000), Instance::new("_Total", 2_000.0)])]
    }

    #[test]
    fn a_frame_carries_what_a_provider_counted_with_its_instances_and_raw_values() {
        let service = SensorService::new(vec![counting("disks", 0, disk_sets())]);
        let interest = service.interest();
        until("a frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        let set = frame.counter("dread").expect("the counter is in the frame");
        assert!(set.answered());
        assert_eq!(set.instance("0 C:").map(|i| (i.value, i.raw)), Some((2_000.0, Some(9_000_000))));
        assert_eq!(set.instance("_Total").map(|i| i.raw), Some(None), "a total has no raw value here");
        assert!(frame.counter("nothing").is_none());
    }

    #[test]
    fn what_a_stuck_provider_counted_is_not_shown() {
        let service = SensorService::new(vec![counting("stuck", 3_000, disk_sets())]);
        let interest = service.interest();
        until("the frame of a provider that is not responding", || interest.latest().is_some());
        assert!(interest.latest().unwrap().counter("dread").is_none_or(|c| !c.answered()), "a gap, not an old figure");
    }

    #[test]
    fn what_a_provider_with_a_problem_counted_is_not_shown() {
        let f: Factory = Arc::new(|| {
            struct Down;
            impl Provider for Down {
                fn name(&self) -> &'static str {
                    "down"
                }
                fn availability(&mut self) -> Result<(), String> {
                    Err("counters are off".into())
                }
                fn read(&mut self, _at: Instant) -> Vec<Reading> {
                    Vec::new()
                }
                fn counted(&mut self) -> Vec<CounterSet> {
                    disk_sets()
                }
            }
            Box::new(Down)
        });
        let service = SensorService::new(vec![f]);
        let interest = service.interest();
        until("a frame", || interest.latest().is_some());
        assert!(interest.latest().unwrap().counter("dread").is_none());
    }

    #[test]
    fn the_first_provider_that_answered_a_counter_gives_it() {
        let empty = vec![CounterSet::new("dread", Vec::new())];
        let other = vec![CounterSet::new("dread", vec![Instance::new("1", 7.0)])];
        let service = SensorService::new(vec![counting("first", 0, empty), counting("second", 0, other), counting("third", 0, disk_sets())]);
        let interest = service.interest();
        until("a frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        assert_eq!(frame.counters.len(), 1, "one set per key");
        assert!(frame.counter("dread").unwrap().instance("1").is_some());
    }

    #[test]
    fn a_provider_that_is_not_listed_is_left_out_of_the_sources_but_still_reports() {
        struct Quiet;
        impl Provider for Quiet {
            fn name(&self) -> &'static str {
                "quiet"
            }
            fn listed(&self) -> bool {
                false
            }
            fn read(&mut self, _at: Instant) -> Vec<Reading> {
                Vec::new()
            }
            fn machine(&mut self) -> Option<Arc<crate::perfdata::StaticInfo>> {
                Some(Arc::default())
            }
        }
        let quiet: Factory = Arc::new(|| Box::new(Quiet));
        let (shown, _) = fake("shown", 0, None);
        let service = SensorService::grouped(vec![(ProviderGroup::Counters, quiet), (ProviderGroup::Counters, shown)]);
        let interest = service.interest_in(ProviderGroup::Counters);
        until("the facts", || interest.latest().is_some_and(|f| f.machine.is_some()));
        let names: Vec<&str> = interest.latest().unwrap().sources.iter().map(|s| s.name).collect();
        assert_eq!(names, ["shown"]);
    }

    #[test]
    fn a_counter_provider_is_told_whether_the_hardware_is_looked_at() {
        struct Told(Arc<Mutex<Vec<bool>>>);
        impl Provider for Told {
            fn name(&self) -> &'static str {
                "told"
            }
            fn hardware_wanted(&mut self, wanted: bool) {
                self.0.lock().unwrap().push(wanted);
            }
            fn read(&mut self, _at: Instant) -> Vec<Reading> {
                Vec::new()
            }
        }
        let told = Arc::new(Mutex::new(Vec::new()));
        let t = told.clone();
        let f: Factory = Arc::new(move || Box::new(Told(t.clone())));
        let service = SensorService::grouped(vec![(ProviderGroup::Counters, f)]);
        let always = service.interest_in(ProviderGroup::Counters);
        until("the first read", || !told.lock().unwrap().is_empty());
        assert_eq!(told.lock().unwrap().last(), Some(&false), "only the counters are asked for");
        let page = service.interest();
        always.read_now();
        until("a read told about the page", || told.lock().unwrap().last() == Some(&true));
        drop(page);
        always.read_now();
        until("a read told the page is gone", || told.lock().unwrap().last() == Some(&false));
    }
}
