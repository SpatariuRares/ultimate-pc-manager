//! File activity: one window per process, opened from its menu. Without
//! administrator rights it says so and offers Restart as Administrator;
//! with them a real-time ETW session on the Microsoft-Windows-Kernel-File
//! provider lists the process's file operations. The
//! session is stopped when the window closes, when the app exits and when
//! it unwinds from a panic, so none is left running.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SendError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{vec2, Align, Layout, RichText, Ui};

use crate::app::Action;
use crate::availability::{is_available, Feature};
use crate::platform::lock_state;
use crate::sampler::ProcRow;
use crate::theme::{self, Theme};
use crate::widgets::fmt_bytes;
use crate::widgets::parts::{admin_button, outline_button};

/// The window's title, before the process's name and PID.
pub const TITLE: &str = "File activity";
/// Said instead of tracing when the app is not elevated.
pub const NEEDS_ADMIN: &str = "Administrator privileges required";
/// The button that starts this app again elevated.
pub const RESTART: &str = "Restart as Administrator";
pub const STARTING: &str = "Starting the trace";
pub const TRACING_SINCE: &str = "Tracing since %1";
pub const STOPPED: &str = "The trace has stopped.";
pub const FAILED: &str = "The trace could not be started (error %1).";
pub const NONE_YET: &str = "No file activity seen yet.";
pub const NONE_MATCH: &str = "No file activity matches the filter.";
pub const LOST: &str = "%1 events lost by the trace";
pub const UNNAMED: &str = "%1 on files the trace never saw named";
/// Said when Totals per file reached its cap.
pub const UNLISTED: &str = "%1 more files not listed";
/// Said once more files were left out than [`UNLISTED_CAP`].
pub const UNLISTED_AT_LEAST: &str = "at least %1 more files not listed";
/// What tells this window apart from the Inspector's open resources.
pub const NOTE: &str = "Operations seen since the trace started, not the process's open handles; Inspect lists those.";

/// How many operations Recent activity keeps (the oldest go first).
pub const RECENT_CAP: usize = 5000;
/// How many files Totals per file lists (owner decision 2026-11-19); the
/// files seen after are only counted.
pub const TOTALS_CAP: usize = 50_000;
/// How many of the files past [`TOTALS_CAP`] are told apart (by name, so
/// one seen twice counts once); past this the footer says "at least".
pub const UNLISTED_CAP: usize = 50_000;

/// How many messages wait for the window at most. The window takes them
/// in only while it is drawn (not while minimized), so a busy process
/// cannot fill the memory: past this, events are dropped and counted.
const FEED_CAP: usize = 50_000;

/// The worker's side of the window's feed. Events never block the trace's
/// callback: when the window has not taken the feed in, they are dropped
/// and counted (the footer adds them to the lost events).
pub struct Feed {
    tx: SyncSender<TraceMsg>,
    dropped: Arc<AtomicU64>,
}

impl Clone for Feed {
    fn clone(&self) -> Self {
        Feed { tx: self.tx.clone(), dropped: Arc::clone(&self.dropped) }
    }
}

impl Feed {
    /// A feed holding at most `capacity` messages; the window's receiver;
    /// the count of events dropped.
    pub fn new(capacity: usize) -> (Feed, Receiver<TraceMsg>, Arc<AtomicU64>) {
        let (tx, rx) = sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        (Feed { tx, dropped: Arc::clone(&dropped) }, rx, dropped)
    }

    /// Hands an event over without waiting; when the feed is full it is
    /// dropped and counted. False once the window is gone.
    pub fn event(&self, e: FileEvent) -> bool {
        match self.tx.try_send(TraceMsg::Event(e)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Disconnected(_)) => false,
        }
    }

    /// Hands a count over without waiting (a later one carries it when
    /// the feed is full). False once the window is gone.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn count(&self, m: TraceMsg) -> bool {
        !matches!(self.tx.try_send(m), Err(TrySendError::Disconnected(_)))
    }

    /// Hands over how the trace stands, waiting for room: the worker's
    /// few status messages are never dropped.
    pub fn send(&self, m: TraceMsg) -> Result<(), SendError<TraceMsg>> {
        self.tx.send(m)
    }
}

/// How many messages of the trace one frame takes in (the rest wait for
/// the next frame, so a busy process cannot stall the UI).
const PER_FRAME: usize = 20_000;

/// The egui id of the File activity window of `pid`.
pub fn window_id(pid: u32) -> egui::Id {
    egui::Id::new(("file-activity", pid))
}

/// A file operation, as the window names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Open,
    Read,
    Write,
    Delete,
    Rename,
}

impl Op {
    pub fn label(self) -> &'static str {
        match self {
            Op::Open => "Open",
            Op::Read => "Read",
            Op::Write => "Write",
            Op::Delete => "Delete",
            Op::Rename => "Rename",
        }
    }

    /// Whether the operation moves bytes (the Bytes column is blank otherwise).
    fn moves_bytes(self) -> bool {
        matches!(self, Op::Read | Op::Write)
    }
}

/// One operation the trace saw. `ms` is local time in milliseconds since
/// 1970; `path` is None for a file the trace never saw named (it was
/// opened before the trace started).
#[derive(Clone, Debug, PartialEq)]
pub struct FileEvent {
    pub ms: i64,
    pub op: Op,
    pub bytes: u64,
    pub path: Option<String>,
}

/// A row of Totals per file.
#[derive(Clone, Debug, PartialEq)]
pub struct Total {
    pub path: String,
    pub read: u64,
    pub written: u64,
    pub operations: u64,
    pub last_ms: i64,
}

/// What the trace saw, as the two lists and the footer show it.
#[derive(Clone, Debug, Default)]
pub struct Activity {
    /// Named operations, oldest first, at most [`RECENT_CAP`].
    recent: VecDeque<FileEvent>,
    /// One row per file, in the order first seen.
    totals: Vec<Total>,
    /// Where each file's row is, by its path in lower case (Windows paths
    /// ignore case).
    index: HashMap<String, usize>,
    /// Operations on files the trace never saw named.
    unnamed: u64,
    /// The session's count of lost events, and that count at the last Clear.
    lost: u64,
    lost_before: u64,
    /// Events dropped on a full feed, and that count at the last Clear.
    dropped: u64,
    dropped_before: u64,
    /// The files past [`TOTALS_CAP`], by their path in lower case: only
    /// counted, and at most [`UNLISTED_CAP`] of them kept.
    unlisted: HashSet<String>,
    /// Whether a file was left out of `unlisted` because it was full.
    unlisted_more: bool,
    /// Bumped on every change to the lists, so a filtered view knows
    /// when to build its rows again; kept across Clear.
    changes: u64,
}

impl Activity {
    /// Takes in one operation: a named one goes to both lists, an unnamed
    /// one is only counted.
    pub fn add(&mut self, e: FileEvent) {
        let Some(path) = &e.path else {
            self.unnamed += 1;
            return;
        };
        self.changes += 1;
        let key = path.to_lowercase();
        let at = match self.index.get(&key) {
            Some(&at) => Some(at),
            None if self.totals.len() < TOTALS_CAP => {
                self.totals.push(Total { path: path.clone(), read: 0, written: 0, operations: 0, last_ms: e.ms });
                self.index.insert(key, self.totals.len() - 1);
                Some(self.totals.len() - 1)
            }
            None => {
                if self.unlisted.len() < UNLISTED_CAP {
                    self.unlisted.insert(key);
                } else if !self.unlisted.contains(&key) {
                    self.unlisted_more = true;
                }
                None
            }
        };
        if let Some(at) = at {
            let t = &mut self.totals[at];
            match e.op {
                Op::Read => t.read += e.bytes,
                Op::Write => t.written += e.bytes,
                _ => {}
            }
            t.operations += 1;
            t.last_ms = t.last_ms.max(e.ms);
        }
        if self.recent.len() == RECENT_CAP {
            self.recent.pop_front();
        }
        self.recent.push_back(e);
    }

    /// The session's count of lost events so far.
    pub fn set_lost(&mut self, total: u64) {
        self.lost = total;
    }

    /// The count of events dropped on a full feed so far.
    pub fn set_dropped(&mut self, total: u64) {
        self.dropped = total;
    }

    /// Events lost since the trace started or the last Clear: those the
    /// session lost and those dropped on a full feed.
    pub fn lost(&self) -> u64 {
        self.lost.saturating_sub(self.lost_before) + self.dropped.saturating_sub(self.dropped_before)
    }

    /// Files seen past [`TOTALS_CAP`], not listed (told apart up to
    /// [`UNLISTED_CAP`]; there may be more when that is reached).
    pub fn unlisted(&self) -> u64 {
        self.unlisted.len() as u64
    }

    /// The footer's counts, in order; the files not listed only when
    /// there are some.
    pub fn footer(&self) -> Vec<String> {
        let mut out = vec![LOST.replace("%1", &self.lost().to_string()), UNNAMED.replace("%1", &self.unnamed().to_string())];
        if self.unlisted() > 0 {
            let text = if self.unlisted_more { UNLISTED_AT_LEAST } else { UNLISTED };
            out.push(text.replace("%1", &self.unlisted().to_string()));
        }
        out
    }

    /// Operations on files the trace never saw named.
    pub fn unnamed(&self) -> u64 {
        self.unnamed
    }

    /// Clear: empties both lists and the footer's counts.
    pub fn clear(&mut self) {
        *self = Activity { lost: self.lost, lost_before: self.lost, dropped: self.dropped, dropped_before: self.dropped, changes: self.changes + 1, ..Default::default() };
    }

    /// Whether nothing was seen since the start or the last Clear.
    pub fn is_empty(&self) -> bool {
        self.recent.is_empty() && self.unnamed == 0
    }

    /// Recent activity, oldest first: the operations whose path or
    /// operation contains `filter` (any case).
    pub fn recent(&self, filter: &str) -> Vec<&FileEvent> {
        self.recent_matching(filter).into_iter().map(|i| &self.recent[i]).collect()
    }

    /// Totals per file, first seen first: the files whose path contains
    /// `filter` (any case).
    #[cfg(test)]
    pub fn totals(&self, filter: &str) -> Vec<&Total> {
        self.totals_matching(filter).into_iter().map(|i| &self.totals[i]).collect()
    }

    /// Where the operations [`Activity::recent`] keeps are, in order.
    fn recent_matching(&self, filter: &str) -> Vec<usize> {
        let q = filter.trim().to_lowercase();
        (0..self.recent.len()).filter(|&i| q.is_empty() || { let e = &self.recent[i]; e.op.label().to_lowercase().contains(&q) || e.path.as_deref().is_some_and(|p| p.to_lowercase().contains(&q)) }).collect()
    }

    /// Where the files [`Activity::totals`] keeps are, in order.
    fn totals_matching(&self, filter: &str) -> Vec<usize> {
        let q = filter.trim().to_lowercase();
        (0..self.totals.len()).filter(|&i| q.is_empty() || self.totals[i].path.to_lowercase().contains(&q)).collect()
    }

    /// The `i`th operation of Recent activity, oldest first.
    fn recent_at(&self, i: usize) -> &FileEvent {
        &self.recent[i]
    }

    /// The `i`th row of Totals per file, first seen first.
    fn total_at(&self, i: usize) -> &Total {
        &self.totals[i]
    }

    /// Whether Recent activity has any operation (filter aside).
    fn has_recent(&self) -> bool {
        !self.recent.is_empty()
    }

    /// Bumped on every change to the lists, Clear included.
    fn changes(&self) -> u64 {
        self.changes
    }

    /// Copy: Recent activity as shown, under its titles, tab-separated.
    pub fn copy_text(&self, filter: &str) -> String {
        let mut lines = vec!["Time\tOperation\tBytes\tPath".to_string()];
        lines.extend(self.recent(filter).iter().map(|e| format!("{}\t{}\t{}\t{}", time_text(e.ms), e.op.label(), bytes_text(e), e.path.as_deref().unwrap_or_default())));
        lines.join("\r\n")
    }
}

/// The rows the filter keeps, kept between frames and built again only
/// when the filter or the lists changed.
#[derive(Default)]
struct View {
    /// The filter and [`Activity::changes`] the rows were built for.
    built_for: Option<(String, u64)>,
    /// Where the rows of Recent activity and Totals per file are.
    recent: Vec<usize>,
    totals: Vec<usize>,
    /// How many times the rows were built.
    builds: u64,
}

impl View {
    /// Builds the rows again if `filter` or `a` changed since the last time.
    fn refresh(&mut self, a: &Activity, filter: &str) {
        if self.built_for.as_ref().is_some_and(|(f, c)| f == filter && *c == a.changes()) {
            return;
        }
        self.recent = a.recent_matching(filter);
        self.totals = a.totals_matching(filter);
        self.built_for = Some((filter.to_string(), a.changes()));
        self.builds += 1;
    }
}

/// The Bytes cell: the size of a read or write, blank for the rest.
fn bytes_text(e: &FileEvent) -> String {
    if e.op.moves_bytes() {
        fmt_bytes(e.bytes)
    } else {
        String::new()
    }
}

/// "22:04:11.208": a local time in milliseconds, as Recent activity shows it.
pub fn time_text(ms: i64) -> String {
    format!("{}.{:03}", clock_text(ms), ms.rem_euclid(1000))
}

/// "22:04:11": Last seen and Tracing since.
pub fn clock_text(ms: i64) -> String {
    let d = ms.div_euclid(1000).rem_euclid(86_400);
    format!("{:02}:{:02}:{:02}", d / 3600, d % 3600 / 60, d % 60)
}

/// Now, in local milliseconds since 1970.
pub fn now_local_ms() -> i64 {
    let utc = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
    utc + local_offset_ms()
}

/// How far local time is ahead of UTC, in milliseconds.
fn local_offset_ms() -> i64 {
    let utc_secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    (crate::pro::local_secs() - utc_secs) * 1000
}

/// Where the trace stands.
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    NeedsAdmin,
    Starting,
    Tracing { since_ms: i64 },
    Stopped,
    Failed(u32),
}

/// The status line's words.
pub fn status_text(s: &Status) -> String {
    match s {
        Status::NeedsAdmin => NEEDS_ADMIN.to_string(),
        Status::Starting => STARTING.to_string(),
        Status::Tracing { since_ms } => TRACING_SINCE.replace("%1", &clock_text(*since_ms)),
        Status::Stopped => STOPPED.to_string(),
        Status::Failed(code) => FAILED.replace("%1", &code.to_string()),
    }
}

/// What the trace's worker sends the window.
#[derive(Clone, Debug, PartialEq)]
pub enum TraceMsg {
    Started { since_ms: i64 },
    Event(FileEvent),
    /// The session's count of lost events so far.
    Lost(u64),
    /// The session ended without the window asking.
    Stopped,
    Failed(u32),
}

/// Ends a trace session exactly once, whoever asks first: the window (its
/// [`Trace`] dropped), the worker (done, failed or unwinding) or a stop
/// that comes before the session is even started.
pub struct Stopper {
    /// (a stop was asked, the session's handle while it runs)
    state: Mutex<(bool, Option<u64>)>,
    /// Stops the session of a handle.
    end: fn(u64),
}

impl Stopper {
    pub fn new(end: fn(u64)) -> Self {
        Stopper { state: Mutex::new((false, None)), end }
    }

    /// Stops the session if one runs; any session registered later is
    /// stopped at once.
    pub fn stop(&self) {
        let mut s = lock_state(&self.state);
        s.0 = true;
        if let Some(handle) = s.1.take() {
            (self.end)(handle);
        }
    }

    /// Hands over a session just started; false (and the session stopped)
    /// when a stop came first.
    pub fn register(&self, handle: u64) -> bool {
        let mut s = lock_state(&self.state);
        if s.0 {
            (self.end)(handle);
            return false;
        }
        s.1 = Some(handle);
        true
    }

    /// Whether a stop was asked.
    pub fn requested(&self) -> bool {
        lock_state(&self.state).0
    }

    /// Runs `f` on the running session's handle (under the lock, so the
    /// session cannot be stopped meanwhile).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn with_session<T>(&self, f: impl FnOnce(u64) -> Option<T>) -> Option<T> {
        let s = lock_state(&self.state);
        s.1.and_then(f)
    }
}

/// A running trace; dropping it stops the session.
pub struct Trace {
    stopper: Arc<Stopper>,
}

impl Drop for Trace {
    fn drop(&mut self) {
        self.stopper.stop();
    }
}

impl Trace {
    /// Starts tracing `pid` on a worker thread; what it sees comes on the
    /// receiver, the count of events dropped on a full feed in the last.
    pub fn start(pid: u32) -> (Trace, Receiver<TraceMsg>, Arc<AtomicU64>) {
        let (feed, rx, dropped) = Feed::new(FEED_CAP);
        let stopper = Arc::new(Stopper::new(etw::stop_session));
        let (worker_stopper, worker_feed) = (stopper.clone(), feed.clone());
        let spawned = std::thread::Builder::new().name("file-activity".into()).spawn(move || etw::run(pid, worker_stopper, worker_feed));
        if spawned.is_err() {
            // ERROR_NOT_ENOUGH_MEMORY: no thread, no session.
            let _ = feed.send(TraceMsg::Failed(8));
        }
        (Trace { stopper }, rx, dropped)
    }
}

/// One File activity window.
pub struct FileActivity {
    pub pid: u32,
    /// "NordVPN.exe (26920)".
    name: String,
    status: Status,
    activity: Activity,
    filter: String,
    /// The rows the filter keeps, built again only when needed.
    view: View,
    /// The session; dropped (and so stopped) with the window.
    trace: Option<Trace>,
    /// What the trace's worker sends.
    feed: Option<Receiver<TraceMsg>>,
    /// The events the worker dropped on a full feed.
    dropped: Arc<AtomicU64>,
}

impl FileActivity {
    /// The window of `pid`. With `admin` the trace starts on a worker
    /// thread; without, the window says administrator rights are needed.
    pub fn open(pid: u32, procs: &[ProcRow], admin: bool) -> Self {
        if !admin {
            return Self::idle(pid, procs, Status::NeedsAdmin);
        }
        let mut w = Self::idle(pid, procs, Status::Starting);
        let (trace, feed, dropped) = Trace::start(pid);
        w.trace = Some(trace);
        w.feed = Some(feed);
        w.dropped = dropped;
        w
    }

    fn idle(pid: u32, procs: &[ProcRow], status: Status) -> Self {
        let name = procs.iter().find(|p| p.pid == pid).map_or_else(|| pid.to_string(), |p| format!("{} ({})", p.name, p.pid));
        FileActivity { pid, name, status, activity: Activity::default(), filter: String::new(), view: View::default(), trace: None, feed: None, dropped: Arc::default() }
    }

    /// A window fed by the test instead of a trace.
    #[cfg(test)]
    pub fn with_feed(pid: u32, procs: &[ProcRow]) -> (Self, Feed) {
        let (feed, rx, dropped) = Feed::new(FEED_CAP);
        let mut w = Self::idle(pid, procs, Status::Starting);
        w.feed = Some(rx);
        w.dropped = dropped;
        (w, feed)
    }

    /// Whether a trace session is held for this window.
    #[cfg(test)]
    pub fn tracing(&self) -> bool {
        self.trace.is_some()
    }

    /// Ends the trace (if any) and says it stopped.
    fn stopped(&mut self, status: Status) {
        self.trace = None;
        self.feed = None;
        self.status = status;
    }

    /// Takes in what the worker sent since the last frame.
    fn poll(&mut self) {
        self.activity.set_dropped(self.dropped.load(Ordering::Relaxed));
        for _ in 0..PER_FRAME {
            let Some(rx) = &self.feed else { return };
            match rx.try_recv() {
                Ok(TraceMsg::Started { since_ms }) => self.status = Status::Tracing { since_ms },
                Ok(TraceMsg::Event(e)) => self.activity.add(e),
                Ok(TraceMsg::Lost(n)) => self.activity.set_lost(n),
                Ok(TraceMsg::Stopped) => self.stopped(Status::Stopped),
                Ok(TraceMsg::Failed(code)) => self.stopped(Status::Failed(code)),
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    let status = if matches!(self.status, Status::Starting | Status::Tracing { .. }) { Status::Stopped } else { self.status.clone() };
                    self.stopped(status);
                }
            }
        }
    }

    /// Draws the window; false once the user closed it. Its content width
    /// opens at `width` and a resize is written back to it (saved with the
    /// Processes table's settings). The trace stops when the process is
    /// gone from `procs`.
    pub fn show(&mut self, ctx: &egui::Context, procs: &[ProcRow], width: &mut f32, actions: &mut Vec<Action>) -> bool {
        self.poll();
        let live = matches!(self.status, Status::Starting | Status::Tracing { .. });
        if live && !procs.is_empty() && !procs.iter().any(|p| p.pid == self.pid) {
            self.stopped(Status::Stopped);
        }
        if self.feed.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let mut open = true;
        let mut drawn = None;
        egui::Window::new(format!("{TITLE} \u{b7} {}", self.name))
            .id(window_id(self.pid))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(*width)
            .default_height(700.0)
            .show(ctx, |ui| {
                drawn = Some(ui.max_rect().width());
                self.body(ui, actions);
            });
        if let Some(w) = drawn {
            if (w - *width).abs() > 0.5 {
                *width = w;
            }
        }
        open
    }

    /// The window's content.
    fn body(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        self.view.refresh(&self.activity, &self.filter);
        let shown = self.view.recent.len();
        let mut clear = false;
        ui.horizontal(|ui| {
            soft(ui, &t, &status_text(&self.status));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if outline_button(ui, "Copy", shown > 0).clicked() {
                    ui.ctx().copy_text(self.activity.copy_text(&self.filter));
                    actions.push(Action::Say(format!("Copied {shown} file operations")));
                }
                clear = outline_button(ui, "Clear", !self.activity.is_empty()).clicked();
            });
        });
        if self.status == Status::NeedsAdmin && is_available(Feature::Elevation) && admin_button(ui, RESTART, false, true).clicked() {
            actions.push(Action::RestartAsAdministrator);
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Recent activity").font(t.fonts.panel_heading.clone()).color(t.text));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter").desired_width(220.0));
            });
        });
        let avail = ui.available_width();
        let recent_cols = [96.0, 76.0, 72.0, (avail - 96.0 - 76.0 - 72.0 - 3.0 * GAP).max(80.0)];
        cells(ui, &t, &recent_cols, &["Time", "Operation", "Bytes", "Path"], &[Align::Min, Align::Min, Align::Max, Align::Min], true);
        if shown == 0 {
            soft(ui, &t, if self.activity.has_recent() { NONE_MATCH } else { NONE_YET });
        } else {
            let row_h = t.fonts.row.size + 6.0;
            egui::ScrollArea::vertical().id_salt("file-activity-recent").max_height(240.0).stick_to_bottom(true).auto_shrink([false, true]).show_rows(ui, row_h, shown, |ui, range| {
                for e in self.view.recent[range].iter().map(|&i| self.activity.recent_at(i)) {
                    let texts = [time_text(e.ms), e.op.label().to_string(), bytes_text(e), e.path.clone().unwrap_or_default()];
                    cells(ui, &t, &recent_cols, &texts.each_ref().map(String::as_str), &[Align::Min, Align::Min, Align::Max, Align::Min], false);
                }
            });
        }
        ui.add_space(10.0);
        ui.label(RichText::new("Totals per file").font(t.fonts.panel_heading.clone()).color(t.text));
        let total_cols = [(avail - 4.0 * 80.0 - 4.0 * GAP).max(120.0), 80.0, 80.0, 80.0, 80.0];
        let right = [Align::Min, Align::Max, Align::Max, Align::Max, Align::Max];
        cells(ui, &t, &total_cols, &["Path", "Read", "Written", "Operations", "Last seen"], &right, true);
        let row_h = t.fonts.row.size + 6.0;
        egui::ScrollArea::vertical().id_salt("file-activity-totals").max_height(200.0).auto_shrink([false, true]).show_rows(ui, row_h, self.view.totals.len(), |ui, range| {
            for f in self.view.totals[range].iter().map(|&i| self.activity.total_at(i)) {
                let texts = [f.path.clone(), fmt_bytes(f.read), fmt_bytes(f.written), f.operations.to_string(), clock_text(f.last_ms)];
                cells(ui, &t, &total_cols, &texts.each_ref().map(String::as_str), &right, false);
            }
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            for (i, text) in self.activity.footer().iter().enumerate() {
                if i > 0 {
                    soft(ui, &t, "\u{b7}");
                }
                soft(ui, &t, text);
            }
        });
        soft(ui, &t, NOTE);
        if clear {
            self.activity.clear();
        }
    }
}

/// Room between two cells of a row.
const GAP: f32 = 10.0;

/// One row of a list: each text in its column's width, cut to it, on the
/// left or the right; titles in the label colour.
fn cells(ui: &mut Ui, t: &Theme, widths: &[f32], texts: &[&str], aligns: &[Align], titles: bool) {
    let (font, color) = if titles { (t.fonts.label.clone(), t.label) } else { (t.fonts.row.clone(), t.text) };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = GAP;
        for ((w, text), align) in widths.iter().zip(texts).zip(aligns) {
            let layout = if *align == Align::Max { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
            ui.allocate_ui_with_layout(vec2(*w, font.size + 4.0), layout, |ui| {
                ui.set_min_width(*w);
                ui.add(egui::Label::new(RichText::new(*text).font(font.clone()).color(color)).truncate());
            });
        }
    });
}

/// A line of quiet text.
fn soft(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text).font(t.fonts.row.clone()).color(t.soft));
}

/// The ETW session, Windows only: a real-time session of its own (a
/// unique name per window) on Microsoft-Windows-Kernel-File, consumed on
/// the worker thread, its lost-event count read every half second.
#[cfg(windows)]
mod etw {
    use super::{now_local_ms, Feed, FileEvent, Op, Stopper, TraceMsg};
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Diagnostics::Etw::*;
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// Every session this app starts is named this, then "<app PID> <n>".
    pub const PREFIX: &str = "UltimatePCManager File activity ";
    /// Microsoft-Windows-Kernel-File.
    const KERNEL_FILE: GUID = GUID::from_u128(0xedd08927_9cc4_4e65_b970_c2560fb5c289);
    /// KERNEL_FILE_KEYWORD_CREATE, _READ, _WRITE, _DELETE_PATH and
    /// _RENAME_SETLINK_PATH.
    const KEYWORDS: u64 = 0x80 | 0x100 | 0x200 | 0x400 | 0x800;
    /// The provider's events this window reads: Create, Read, Write,
    /// DeletePath, RenamePath.
    const CREATE: u16 = 12;
    const READ: u16 = 15;
    const WRITE: u16 = 16;
    const DELETE_PATH: u16 = 26;
    const RENAME_PATH: u16 = 27;
    /// OpenTraceW's failure.
    const INVALID_TRACE: u64 = u64::MAX;
    const ERROR_INVALID_PARAMETER: u32 = 87;
    const ERROR_MORE_DATA: u32 = 234;
    const STILL_ACTIVE: u32 = 259;
    /// File objects remembered by name before the map starts over.
    const NAMES_CAP: usize = 200_000;

    static NEXT: AtomicU32 = AtomicU32::new(0);

    use crate::winsys::wide;

    /// An EVENT_TRACE_PROPERTIES with room for a session and a file name
    /// after it, aligned for the structure.
    struct Props(Vec<u64>);

    impl Props {
        const NAME_BYTES: usize = 1024 * 2;

        fn new() -> Props {
            let head = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
            let mut buf = vec![0u64; (head + 2 * Self::NAME_BYTES).div_ceil(8)];
            let bytes = (buf.len() * 8) as u32;
            let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
            // SAFETY: `buf` is zeroed, 8-aligned and larger than the
            // structure.
            unsafe {
                (*p).Wnode.BufferSize = bytes;
                (*p).LoggerNameOffset = head as u32;
                (*p).LogFileNameOffset = (head + Self::NAME_BYTES) as u32;
            }
            Props(buf)
        }

        fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
            self.0.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES
        }

        fn get(&self) -> &EVENT_TRACE_PROPERTIES {
            // SAFETY: as in `new`.
            unsafe { &*(self.0.as_ptr() as *const EVENT_TRACE_PROPERTIES) }
        }

        /// The session name Windows wrote after the structure.
        fn logger_name(&self) -> String {
            let at = self.get().LoggerNameOffset as usize;
            let bytes: &[u8] = &as_bytes(&self.0)[at.min(self.0.len() * 8)..];
            let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
            String::from_utf16_lossy(&units)
        }
    }

    fn as_bytes(v: &[u64]) -> &[u8] {
        // SAFETY: any u64 slice is also a valid byte slice of 8 times its length.
        unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 8) }
    }

    /// Starts a real-time session; its handle, or the Win32 error (5 when
    /// not elevated).
    pub fn start_session(name: &str) -> Result<u64, u32> {
        let mut props = Props::new();
        let p = props.ptr();
        // SAFETY: `p` points into `props`, alive for the call; the name is
        // NUL-terminated and fits the room after the structure.
        unsafe {
            (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            (*p).Wnode.ClientContext = 1; // QPC timestamps
            (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            (*p).BufferSize = 64;
            (*p).FlushTimer = 1;
            let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
            let w = wide(name);
            match StartTraceW(&mut handle, w.as_ptr(), p) {
                0 => Ok(handle.Value),
                e => Err(e),
            }
        }
    }

    /// Stops the session of `handle` (nothing when it already ended).
    pub fn stop_session(handle: u64) {
        let mut props = Props::new();
        // SAFETY: `props` outlives the call; with a handle no name is needed.
        unsafe {
            ControlTraceW(CONTROLTRACE_HANDLE { Value: handle }, std::ptr::null(), props.ptr(), EVENT_TRACE_CONTROL_STOP);
        }
    }

    fn stop_by_name(name: &str) {
        let mut props = Props::new();
        let w = wide(name);
        // SAFETY: `props` and the NUL-terminated name outlive the call.
        unsafe {
            ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, w.as_ptr(), props.ptr(), EVENT_TRACE_CONTROL_STOP);
        }
    }

    /// The session's count of lost events.
    fn lost(handle: u64) -> Option<u64> {
        let mut props = Props::new();
        // SAFETY: `props` outlives the call.
        let e = unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: handle }, std::ptr::null(), props.ptr(), EVENT_TRACE_CONTROL_QUERY) };
        (e == 0).then(|| u64::from(props.get().EventsLost))
    }

    /// The running sessions of this window's kind: (name, the PID of the
    /// app that started it).
    pub fn sessions() -> Vec<(String, u32)> {
        const SLOTS: usize = 64;
        let mut all: Vec<Props> = (0..SLOTS).map(|_| Props::new()).collect();
        let mut ptrs: Vec<*mut EVENT_TRACE_PROPERTIES> = all.iter_mut().map(Props::ptr).collect();
        let mut count = 0u32;
        // SAFETY: SLOTS pointers, each to a Props sized as its BufferSize says.
        let e = unsafe { QueryAllTracesW(ptrs.as_mut_ptr(), SLOTS as u32, &mut count) };
        if e != 0 && e != ERROR_MORE_DATA {
            return Vec::new();
        }
        all.iter()
            .take(count as usize)
            .filter_map(|props| {
                let name = props.logger_name();
                let owner = name.strip_prefix(PREFIX).and_then(|rest| rest.split(' ').next()).and_then(|s| s.parse::<u32>().ok())?;
                Some((name, owner))
            })
            .collect()
    }

    /// Stops the sessions of this window's kind left by an app that is gone
    /// (killed before it could stop them).
    fn stop_stale_sessions() {
        for (name, owner) in sessions() {
            if owner != std::process::id() && !alive(owner) {
                stop_by_name(&name);
            }
        }
    }

    /// Whether a process of this PID still runs (a process that may not be
    /// opened counts as running).
    fn alive(pid: u32) -> bool {
        // SAFETY: the handle is checked and closed once.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return GetLastError() != ERROR_INVALID_PARAMETER;
            }
            let mut code = 0u32;
            let read = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            !read || code == STILL_ACTIVE
        }
    }

    /// Stops the session when the worker ends, however it ends.
    struct StopOnDrop(Arc<Stopper>);

    impl Drop for StopOnDrop {
        fn drop(&mut self) {
            self.0.stop();
        }
    }

    /// The worker: starts the session, enables the provider, consumes the
    /// events until the session stops, then says why it ended.
    pub fn run(pid: u32, stopper: Arc<Stopper>, tx: Feed) {
        let _stop = StopOnDrop(stopper.clone());
        stop_stale_sessions();
        let name = format!("{PREFIX}{} {}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
        let handle = match start_session(&name) {
            Ok(h) => h,
            Err(e) => {
                let _ = tx.send(TraceMsg::Failed(e));
                return;
            }
        };
        if !stopper.register(handle) {
            return;
        }
        // SAFETY: the GUID outlives the call; no filter parameters.
        let e = unsafe {
            EnableTraceEx2(CONTROLTRACE_HANDLE { Value: handle }, &KERNEL_FILE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, TRACE_LEVEL_VERBOSE as u8, KEYWORDS, 0, 0, std::ptr::null())
        };
        if e != 0 {
            stopper.stop();
            let _ = tx.send(TraceMsg::Failed(e));
            return;
        }
        let consumer = Box::into_raw(Box::new(Consumer::new(pid, tx.clone())));
        let mut logger = wide(&name);
        // SAFETY: a zeroed EVENT_TRACE_LOGFILEW is valid; the name lives
        // until OpenTraceW returns; `consumer` lives until ProcessTrace and
        // CloseTrace have returned (below), so every callback finds it.
        let trace = unsafe {
            let mut log: EVENT_TRACE_LOGFILEW = std::mem::zeroed();
            log.LoggerName = logger.as_mut_ptr();
            log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
            log.Anonymous2.EventRecordCallback = Some(on_event);
            log.Context = consumer as *mut c_void;
            OpenTraceW(&mut log).Value
        };
        if trace == INVALID_TRACE {
            // SAFETY: read right after the failed call; `consumer` was never handed out.
            let e = unsafe { GetLastError() };
            // SAFETY: `consumer` came from Box::into_raw above and was never
            // handed to Windows (OpenTraceW failed), so this is its only owner.
            drop(unsafe { Box::from_raw(consumer) });
            stopper.stop();
            let _ = tx.send(TraceMsg::Failed(e));
            return;
        }
        let _ = tx.send(TraceMsg::Started { since_ms: now_local_ms() });
        let poll_stopper = stopper.clone();
        let poll_tx = tx.clone();
        let poller = std::thread::Builder::new().name("file-activity-lost".into()).spawn(move || {
            while !poll_stopper.requested() {
                if let Some(n) = poll_stopper.with_session(lost) {
                    if !poll_tx.count(TraceMsg::Lost(n)) {
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        });
        // SAFETY: `trace` is the handle OpenTraceW gave; ProcessTrace
        // returns once the session stops, then the handle is closed once.
        unsafe {
            let handles = [PROCESSTRACE_HANDLE { Value: trace }];
            ProcessTrace(handles.as_ptr(), 1, std::ptr::null(), std::ptr::null());
            CloseTrace(PROCESSTRACE_HANDLE { Value: trace });
            drop(Box::from_raw(consumer));
        }
        let asked = stopper.requested();
        stopper.stop();
        if let Ok(p) = poller {
            let _ = p.join();
        }
        if !asked {
            let _ = tx.send(TraceMsg::Stopped);
        }
    }

    /// The callback ProcessTrace calls per event.
    unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
        // A panic must not unwind into Windows.
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // SAFETY: ProcessTrace passes a valid record; its UserContext
            // is the Consumer `run` keeps alive until ProcessTrace returns.
            unsafe {
                let record = &*record;
                if let Some(consumer) = (record.UserContext as *mut Consumer).as_mut() {
                    consumer.take(record);
                }
            }
        }));
    }

    fn same_guid(a: &GUID, b: &GUID) -> bool {
        (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
    }

    /// Turns the provider's events of one process into [`FileEvent`]s.
    struct Consumer {
        pid: u32,
        tx: Feed,
        /// File object → its path, from the Create events seen.
        names: HashMap<u64, String>,
        /// The NT names of the drives ("\Device\HarddiskVolume3" → "C:").
        drives: Vec<(String, String)>,
        /// Local time ahead of UTC, in milliseconds.
        offset_ms: i64,
        file_object: Vec<u16>,
        file_name: Vec<u16>,
        file_path: Vec<u16>,
        io_size: Vec<u16>,
    }

    impl Consumer {
        fn new(pid: u32, tx: Feed) -> Consumer {
            Consumer {
                pid,
                tx,
                names: HashMap::new(),
                drives: crate::proc_info::drive_devices(),
                offset_ms: super::local_offset_ms(),
                file_object: wide("FileObject"),
                file_name: wide("FileName"),
                file_path: wide("FilePath"),
                io_size: wide("IOSize"),
            }
        }

        fn take(&mut self, record: &EVENT_RECORD) {
            let header = &record.EventHeader;
            if !same_guid(&header.ProviderId, &KERNEL_FILE) || header.ProcessId != self.pid {
                return;
            }
            let id = header.EventDescriptor.Id;
            if ![CREATE, READ, WRITE, DELETE_PATH, RENAME_PATH].contains(&id) {
                return;
            }
            let object = property(record, &self.file_object).and_then(|b| number(&b));
            let named = |names: &HashMap<u64, String>| object.and_then(|o| names.get(&o).cloned());
            let (op, bytes, path) = match id {
                CREATE => {
                    let name = property(record, &self.file_name).map(|b| text(&b)).filter(|n| !n.is_empty()).map(|n| crate::proc_info::dos_path(&n, &self.drives));
                    if let (Some(o), Some(n)) = (object, &name) {
                        if self.names.len() >= NAMES_CAP {
                            self.names.clear();
                        }
                        self.names.insert(o, n.clone());
                    }
                    (Op::Open, 0, name)
                }
                READ | WRITE => {
                    let bytes = property(record, &self.io_size).and_then(|b| number(&b)).unwrap_or(0);
                    (if id == READ { Op::Read } else { Op::Write }, bytes, named(&self.names))
                }
                _ => {
                    let given = || property(record, &self.file_path).map(|b| text(&b)).filter(|n| !n.is_empty()).map(|n| crate::proc_info::dos_path(&n, &self.drives));
                    (if id == DELETE_PATH { Op::Delete } else { Op::Rename }, 0, named(&self.names).or_else(given))
                }
            };
            let ms = filetime_ms(header.TimeStamp) + self.offset_ms;
            // Never blocks the callback: on a full feed the event is
            // dropped and counted.
            self.tx.event(FileEvent { ms, op, bytes, path });
        }
    }

    /// A property of the event by name, as raw bytes (TDH decodes the
    /// provider's manifest, whatever the event's version).
    fn property(record: &EVENT_RECORD, name: &[u16]) -> Option<Vec<u8>> {
        let desc = PROPERTY_DATA_DESCRIPTOR { PropertyName: name.as_ptr() as u64, ArrayIndex: u32::MAX, Reserved: 0 };
        let mut size = 0u32;
        // SAFETY: `record` is the callback's record; `name` is
        // NUL-terminated; the buffer holds the size TDH asked for.
        unsafe {
            if TdhGetPropertySize(record, 0, std::ptr::null(), 1, &desc, &mut size) != 0 || size == 0 || size > 65_536 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            (TdhGetProperty(record, 0, std::ptr::null(), 1, &desc, size, buf.as_mut_ptr()) == 0).then_some(buf)
        }
    }

    /// A 4- or 8-byte little-endian number (pointers and sizes).
    pub fn number(b: &[u8]) -> Option<u64> {
        match b.len() {
            8 => Some(u64::from_le_bytes(b.try_into().ok()?)),
            4 => Some(u64::from(u32::from_le_bytes(b.try_into().ok()?))),
            _ => None,
        }
    }

    /// A UTF-16 string up to its NUL.
    pub fn text(b: &[u8]) -> String {
        let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        String::from_utf16_lossy(&units)
    }

    /// An event's FILETIME (100 ns since 1601, UTC) as milliseconds since 1970.
    pub fn filetime_ms(t: i64) -> i64 {
        t / 10_000 - 11_644_473_600_000
    }

}

/// Where ETW is missing: a trace cannot be started.
#[cfg(not(windows))]
mod etw {
    use super::{Feed, Stopper, TraceMsg};
    use std::sync::Arc;

    pub fn stop_session(_: u64) {}

    pub fn run(_: u32, _: Arc<Stopper>, tx: Feed) {
        // ERROR_NOT_SUPPORTED
        let _ = tx.send(TraceMsg::Failed(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Mode;
    use std::sync::atomic::Ordering as AtomicOrdering;

    /// 2026-11-18 22:04:09 local, in milliseconds.
    const T0: i64 = 1_795_039_449_000;

    fn ev(ms: i64, op: Op, bytes: u64, path: Option<&str>) -> FileEvent {
        FileEvent { ms: T0 + ms, op, bytes, path: path.map(str::to_string) }
    }

    fn sample() -> Vec<FileEvent> {
        vec![
            ev(2_208, Op::Open, 0, Some(r"C:\Users\spata\nordvpn.log")),
            ev(2_214, Op::Read, 4096, Some(r"C:\Users\spata\nordvpn.log")),
            ev(2_530, Op::Write, 512, Some(r"C:\Temp\state.json")),
            ev(2_600, Op::Read, 100, None),
            ev(3_002, Op::Rename, 0, Some(r"C:\Temp\old.tmp")),
            // The same file in other letters: one row.
            ev(3_410, Op::Write, 1024, Some(r"c:\users\SPATA\nordvpn.log")),
            ev(3_500, Op::Delete, 0, Some(r"C:\Temp\gone.tmp")),
            ev(3_600, Op::Write, 7, None),
        ]
    }

    fn fed(events: Vec<FileEvent>) -> Activity {
        let mut a = Activity::default();
        for e in events {
            a.add(e);
        }
        a
    }

    #[test]
    fn the_totals_add_up_each_files_reads_writes_and_operations_and_keep_its_last_time() {
        let a = fed(sample());
        let totals: Vec<(&str, u64, u64, u64, i64)> = a.totals("").iter().map(|t| (t.path.as_str(), t.read, t.written, t.operations, t.last_ms - T0)).collect();
        assert_eq!(
            totals,
            [
                (r"C:\Users\spata\nordvpn.log", 4096, 1024, 3, 3_410),
                (r"C:\Temp\state.json", 0, 512, 1, 2_530),
                (r"C:\Temp\old.tmp", 0, 0, 1, 3_002),
                (r"C:\Temp\gone.tmp", 0, 0, 1, 3_500),
            ],
            "first seen first; a path in other letters is the same file"
        );
        let ops: Vec<Op> = a.recent("").iter().map(|e| e.op).collect();
        assert_eq!(ops, [Op::Open, Op::Read, Op::Write, Op::Rename, Op::Write, Op::Delete], "oldest first; unnamed ones are only counted");
        assert_eq!(a.unnamed(), 2);
    }

    #[test]
    fn lost_events_count_from_the_last_clear_and_clear_empties_both_lists() {
        let mut a = fed(sample());
        a.set_lost(3);
        assert_eq!(a.lost(), 3);
        a.clear();
        assert!(a.recent("").is_empty() && a.totals("").is_empty());
        assert_eq!((a.lost(), a.unnamed()), (0, 0));
        a.set_lost(5);
        assert_eq!(a.lost(), 2, "only what was lost since Clear");
    }

    #[test]
    fn recent_activity_keeps_the_newest_operations_up_to_its_cap() {
        let mut a = Activity::default();
        for i in 0..RECENT_CAP as i64 + 10 {
            a.add(ev(i, Op::Read, 1, Some(r"C:\a.bin")));
        }
        let recent = a.recent("");
        assert_eq!(recent.len(), RECENT_CAP);
        assert_eq!(recent[0].ms - T0, 10, "the oldest went first");
        assert_eq!(a.totals("")[0].operations, RECENT_CAP as u64 + 10, "the totals keep counting");
    }

    #[test]
    fn totals_per_file_list_at_most_their_cap_and_the_footer_counts_the_files_left_out() {
        let mut a = Activity::default();
        let file = |i: usize| format!(r"C:\f\{i}.bin");
        for i in 0..TOTALS_CAP + 3 {
            a.add(ev(i as i64, Op::Write, 1, Some(&file(i))));
        }
        // Files beyond the cap seen again count once.
        a.add(ev(0, Op::Read, 1, Some(&file(TOTALS_CAP + 1).to_uppercase())));
        // A listed file goes on counting.
        a.add(ev(0, Op::Read, 9, Some(&file(0))));
        let totals = a.totals("");
        assert_eq!(totals.len(), TOTALS_CAP);
        assert_eq!((totals[0].read, totals[0].operations), (9, 2));
        assert!(!totals.iter().any(|t| t.path == file(TOTALS_CAP)), "the first file past the cap is not listed");
        assert_eq!(a.unlisted(), 3);
        assert!(a.footer().contains(&"3 more files not listed".to_string()), "{:?}", a.footer());
        assert_eq!(a.recent("").len(), RECENT_CAP, "Recent activity still shows them");
        a.clear();
        assert_eq!(a.unlisted(), 0);
        assert!(!a.footer().iter().any(|t| t.contains("not listed")), "said only when some are left out");
    }

    #[test]
    fn events_dropped_on_a_full_feed_add_to_the_lost_count() {
        let mut a = fed(sample());
        a.set_lost(3);
        a.set_dropped(4);
        assert_eq!(a.lost(), 7);
        assert!(a.footer().contains(&"7 events lost by the trace".to_string()), "{:?}", a.footer());
        a.clear();
        a.set_dropped(6);
        assert_eq!(a.lost(), 2, "only what was dropped since Clear");
    }

    #[test]
    fn a_full_feed_drops_events_without_blocking_and_counts_them() {
        let (feed, rx, dropped) = Feed::new(2);
        for i in 0..5 {
            assert!(feed.event(ev(i, Op::Read, 1, Some(r"C:\a.bin"))), "the window is still there");
        }
        assert_eq!(dropped.load(AtomicOrdering::Relaxed), 3);
        assert_eq!(rx.try_iter().count(), 2, "the first two wait for the window");
        assert!(feed.event(ev(9, Op::Read, 1, None)), "room again");
        drop(rx);
        assert!(!feed.event(ev(10, Op::Read, 1, None)), "the window is gone");
    }

    #[test]
    fn the_filter_keeps_the_operations_and_files_whose_path_or_operation_match() {
        let a = fed(sample());
        let paths = |f: &str| a.recent(f).iter().map(|e| e.path.clone().unwrap()).collect::<Vec<_>>();
        assert_eq!(paths("TEMP"), [r"C:\Temp\state.json", r"C:\Temp\old.tmp", r"C:\Temp\gone.tmp"]);
        assert_eq!(a.recent("rename").len(), 1, "by operation too");
        assert_eq!(a.totals("state").iter().map(|t| t.path.as_str()).collect::<Vec<_>>(), [r"C:\Temp\state.json"]);
        assert!(a.recent("nothing like it").is_empty());
    }

    #[test]
    fn copy_gives_the_rows_shown_as_tab_separated_lines() {
        let a = fed(sample());
        assert_eq!(
            a.copy_text("state"),
            "Time\tOperation\tBytes\tPath\r\n22:04:11.530\tWrite\t512 B\tC:\\Temp\\state.json"
        );
    }

    #[test]
    fn memory_stays_bounded_however_many_files_the_trace_sees() {
        let mut a = Activity::default();
        for i in 0..TOTALS_CAP + UNLISTED_CAP + 25_000 {
            a.add(ev(i as i64, Op::Write, 1, Some(&format!(r"C:\f\{i}.bin"))));
        }
        assert_eq!((a.totals.len(), a.index.len()), (50_000, 50_000), "Totals per file");
        assert_eq!(a.recent.len(), 5000, "Recent activity");
        assert!(a.unlisted.len() <= 50_000, "the files not listed are not all kept: {}", a.unlisted.len());
        assert!(a.footer().contains(&"at least 50000 more files not listed".to_string()), "{:?}", a.footer());
    }

    #[test]
    fn the_filtered_view_is_built_again_only_when_the_filter_or_the_data_changed() {
        let mut a = fed(sample());
        let mut v = View::default();
        let paths = |v: &View, a: &Activity| v.recent.iter().map(|&i| a.recent_at(i).path.clone().unwrap()).collect::<Vec<_>>();
        v.refresh(&a, "temp");
        v.refresh(&a, "temp");
        assert_eq!(v.builds, 1, "same filter, same data");
        assert_eq!(paths(&v, &a), [r"C:\Temp\state.json", r"C:\Temp\old.tmp", r"C:\Temp\gone.tmp"]);
        assert_eq!(v.totals.len(), 3);
        v.refresh(&a, "state");
        assert_eq!((v.builds, v.recent.len(), v.totals.len()), (2, 1, 1), "the filter changed");
        a.set_lost(4);
        a.set_dropped(1);
        v.refresh(&a, "state");
        assert_eq!(v.builds, 2, "the footer's counts do not change the rows");
        a.add(ev(4_000, Op::Read, 8, Some(r"C:\Temp\STATE.json")));
        v.refresh(&a, "state");
        assert_eq!((v.builds, v.recent.len(), v.totals.len()), (3, 2, 1), "an operation came in");
        a.clear();
        v.refresh(&a, "state");
        assert_eq!((v.builds, v.recent.len(), v.totals.len()), (4, 0, 0), "Clear");
    }

    #[test]
    fn frames_without_new_activity_reuse_the_filtered_view() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        for e in sample() {
            feed.send(TraceMsg::Event(e)).unwrap();
        }
        texts(&ctx, &mut w);
        let built = w.view.builds;
        let shown = texts(&ctx, &mut w);
        assert_eq!(w.view.builds, built, "nothing new, nothing built");
        assert!(shown.iter().any(|t| t == r"C:\Temp\state.json"), "{shown:?}");
        feed.send(TraceMsg::Event(ev(5_000, Op::Open, 0, Some(r"C:\new.txt")))).unwrap();
        let shown = texts(&ctx, &mut w);
        assert_eq!(w.view.builds, built + 1);
        assert!(shown.iter().any(|t| t == r"C:\new.txt"), "{shown:?}");
    }

    #[test]
    fn times_read_as_the_local_clock() {
        assert_eq!(time_text(T0 + 2_208), "22:04:11.208");
        assert_eq!(clock_text(T0 + 2_208), "22:04:11");
    }

    fn nordvpn() -> ProcRow {
        ProcRow { pid: 26920, name: "NordVPN.exe".into(), status: "Running".into(), ..Default::default() }
    }

    /// The texts one frame of the window drew (a window is only sized on
    /// its first frame, so a first one is drawn unseen), and the actions.
    fn frame(ctx: &egui::Context, w: &mut FileActivity, events: Vec<egui::Event>) -> (Vec<(String, egui::Rect)>, Vec<Action>) {
        let procs = [nordvpn()];
        let mut width = 760.0;
        let mut actions = Vec::new();
        if ctx.memory(|m| m.area_rect(window_id(w.pid)).is_none()) {
            crate::test_ui::run(ctx, (1400.0, 1400.0), vec![], |ui| {
                w.show(ui.ctx(), &procs, &mut width, &mut Vec::new());
            });
        }
        let out = crate::test_ui::run(ctx, (1400.0, 1400.0), events, |ui| {
            w.show(ui.ctx(), &procs, &mut width, &mut actions);
        });
        let texts = crate::test_ui::flat_shapes(&out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(g) => Some((g.galley.text().to_string(), g.visual_bounding_rect())),
                _ => None,
            })
            .collect();
        (texts, actions)
    }

    fn texts(ctx: &egui::Context, w: &mut FileActivity) -> Vec<String> {
        frame(ctx, w, vec![]).0.into_iter().map(|x| x.0).collect()
    }

    fn click(rect: egui::Rect) -> Vec<egui::Event> {
        let b = |pressed| egui::Event::PointerButton { pos: rect.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        vec![egui::Event::PointerMoved(rect.center()), b(true), b(false)]
    }

    #[test]
    fn without_administrator_rights_the_window_says_so_starts_no_trace_and_offers_a_restart() {
        // The test process is not elevated: this is the real check.
        if crate::platform::is_admin() {
            eprintln!("skipped: the tests run elevated");
            return;
        }
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut w = FileActivity::open(26920, &[nordvpn()], crate::platform::is_admin());
        assert!(!w.tracing(), "no trace without the rights");
        let (spots, _) = frame(&ctx, &mut w, vec![]);
        let all: Vec<&str> = spots.iter().map(|s| s.0.as_str()).collect();
        assert!(all.contains(&NEEDS_ADMIN), "{all:?}");
        assert!(all.contains(&NOTE), "{all:?}");
        assert!(all.iter().any(|t| t.starts_with("File activity") && t.contains("NordVPN.exe (26920)")), "the title names the process: {all:?}");
        let button = spots.iter().find(|s| s.0.trim() == RESTART).unwrap_or_else(|| panic!("{RESTART} in {all:?}")).1;
        let (_, actions) = frame(&ctx, &mut w, click(button));
        assert_eq!(actions, [Action::RestartAsAdministrator]);
    }

    #[test]
    fn a_system_that_cannot_restart_elevated_hides_the_restart_button_and_the_window_stays() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let words = |w: &mut FileActivity| frame(&ctx, w, vec![]).0.into_iter().map(|s| s.0).collect::<Vec<_>>();
        let mut shown = FileActivity::open(26920, &[nordvpn()], false);
        let on_this_system = words(&mut shown);
        assert_eq!(on_this_system.iter().any(|t| t.trim() == RESTART), is_available(Feature::Elevation), "{on_this_system:?}");
        let _unavailable = crate::availability::unavailable_in_test(&[Feature::Elevation]);
        let hidden = words(&mut shown);
        assert!(!hidden.iter().any(|t| t.trim() == RESTART), "{hidden:?}");
        assert!(hidden.iter().any(|t| t == NEEDS_ADMIN), "the window still says why it cannot trace: {hidden:?}");
    }

    #[test]
    fn a_traced_process_fills_recent_activity_totals_per_file_and_the_footer() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        feed.send(TraceMsg::Started { since_ms: T0 }).unwrap();
        for e in sample() {
            feed.send(TraceMsg::Event(e)).unwrap();
        }
        feed.send(TraceMsg::Lost(3)).unwrap();
        let (spots, _) = frame(&ctx, &mut w, vec![]);
        let all: Vec<&str> = spots.iter().map(|s| s.0.as_str()).collect();
        let top = |s: &str| spots.iter().find(|(t, _)| t == s).unwrap_or_else(|| panic!("{s} in {all:?}")).1.top();
        for shown in ["Tracing since 22:04:09", "Clear", "Copy", "22:04:11.208", "4 KB", "Rename", "Delete", r"C:\Temp\state.json", "3 events lost by the trace", "2 on files the trace never saw named", NOTE] {
            top(shown);
        }
        let order = ["Recent activity", "Time", "Totals per file", "Written", "Last seen"];
        let tops: Vec<f32> = order.iter().map(|s| top(s)).collect();
        assert!(tops.windows(2).all(|p| p[0] <= p[1]), "{order:?} top to bottom: {tops:?}");
        for title in ["Operation", "Bytes", "Path", "Read", "Operations"] {
            top(title);
        }
        assert!(!all.contains(&NEEDS_ADMIN));
    }

    #[test]
    fn every_status_line_is_shown_in_its_state() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        let shown = texts(&ctx, &mut w);
        assert!(shown.iter().any(|t| t == STARTING), "{shown:?}");
        assert!(shown.iter().any(|t| t == NONE_YET), "{shown:?}");
        feed.send(TraceMsg::Started { since_ms: T0 }).unwrap();
        feed.send(TraceMsg::Event(ev(10, Op::Open, 0, Some(r"C:\a.txt")))).unwrap();
        w.filter = "zzz".into();
        let shown = texts(&ctx, &mut w);
        assert!(shown.iter().any(|t| t == NONE_MATCH), "{shown:?}");
        feed.send(TraceMsg::Stopped).unwrap();
        let shown = texts(&ctx, &mut w);
        assert!(shown.iter().any(|t| t == STOPPED), "{shown:?}");
        let (mut failed, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        feed.send(TraceMsg::Failed(5)).unwrap();
        let shown = texts(&ctx, &mut failed);
        assert!(shown.iter().any(|t| t == "The trace could not be started (error 5)."), "{shown:?}");
    }

    #[test]
    fn clear_empties_the_lists() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        feed.send(TraceMsg::Started { since_ms: T0 }).unwrap();
        feed.send(TraceMsg::Event(ev(10, Op::Open, 0, Some(r"C:\a.txt")))).unwrap();
        let (spots, _) = frame(&ctx, &mut w, vec![]);
        let clear = spots.iter().find(|s| s.0 == "Clear").expect("Clear").1;
        frame(&ctx, &mut w, click(clear));
        let shown = texts(&ctx, &mut w);
        assert!(shown.iter().any(|t| t == NONE_YET), "{shown:?}");
        assert!(!shown.iter().any(|t| t == r"C:\a.txt"));
    }

    #[test]
    fn the_window_opens_at_the_saved_width() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, _feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        let procs = [nordvpn()];
        let mut width = 700.0;
        for _ in 0..2 {
            crate::test_ui::run(&ctx, (1400.0, 1400.0), vec![], |ui| {
                w.show(ui.ctx(), &procs, &mut width, &mut Vec::new());
            });
        }
        assert!((width - 700.0).abs() < 1.0, "{width}");
    }

    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn a_session_is_stopped_once_whoever_asks_and_one_registered_after_a_stop_at_once() {
        static ENDED: AtomicUsize = AtomicUsize::new(0);
        fn end(handle: u64) {
            assert_eq!(handle, 42);
            ENDED.fetch_add(1, Ordering::SeqCst);
        }
        // The window's handle and the worker both stop it: one stop.
        let stopper = Arc::new(Stopper::new(end));
        assert!(stopper.register(42));
        let trace = Trace { stopper: stopper.clone() };
        drop(trace);
        stopper.stop();
        assert_eq!(ENDED.load(Ordering::SeqCst), 1);
        assert!(stopper.requested());
        // The window closed before the worker had its session: it is
        // stopped as soon as it is handed over.
        let early = Stopper::new(end);
        early.stop();
        assert!(!early.register(42));
        assert_eq!(ENDED.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn closing_a_window_drops_its_trace() {
        static ENDED: AtomicUsize = AtomicUsize::new(0);
        fn end(_: u64) {
            ENDED.fetch_add(1, Ordering::SeqCst);
        }
        let stopper = Arc::new(Stopper::new(end));
        assert!(stopper.register(7));
        let (mut w, _feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        w.trace = Some(Trace { stopper });
        assert!(w.tracing());
        drop(w);
        assert_eq!(ENDED.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_trace_stops_when_the_process_is_gone() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (mut w, feed) = FileActivity::with_feed(26920, &[nordvpn()]);
        feed.send(TraceMsg::Started { since_ms: T0 }).unwrap();
        let other = ProcRow { pid: 1, name: "other.exe".into(), ..Default::default() };
        crate::test_ui::run(&ctx, (1400.0, 1400.0), vec![], |ui| {
            w.show(ui.ctx(), std::slice::from_ref(&other), &mut 760.0, &mut Vec::new());
        });
        assert_eq!(w.status, Status::Stopped);
    }

    /// The real ETW path on the test's own process: the test process is
    /// not elevated, so Windows refuses the trace (a session may be created
    /// without rights, but the kernel provider is not enabled), and the
    /// worker stops what it started: no session of this app is left.
    #[cfg(windows)]
    #[test]
    fn without_administrator_rights_windows_refuses_the_trace_and_no_session_is_left() {
        if crate::platform::is_admin() {
            eprintln!("skipped: the tests run elevated");
            return;
        }
        let ours = || etw::sessions().into_iter().filter(|(_, owner)| *owner == std::process::id()).count();
        let before = ours();
        let (trace, rx, _) = Trace::start(std::process::id());
        let got = rx.recv_timeout(Duration::from_secs(10));
        drop(trace);
        assert!(matches!(got, Ok(TraceMsg::Failed(code)) if code != 0), "{got:?}");
        assert_eq!(ours(), before, "the refused trace's session was stopped");
    }

    #[cfg(windows)]
    #[test]
    fn event_fields_decode_into_paths_numbers_and_times() {
        let drives = vec![(r"\Device\HarddiskVolume3".to_string(), "C:".to_string())];
        assert_eq!(crate::proc_info::dos_path(r"\Device\HarddiskVolume3\Users\a.txt", &drives), r"C:\Users\a.txt");
        assert_eq!(crate::proc_info::dos_path(r"\Device\HarddiskVolume30\a.txt", &drives), r"\Device\HarddiskVolume30\a.txt", "another volume");
        assert_eq!(etw::number(&4096u32.to_le_bytes()), Some(4096));
        assert_eq!(etw::number(&0xffff_8000_0000_1234u64.to_le_bytes()), Some(0xffff_8000_0000_1234));
        let name: Vec<u8> = "a.txt\0junk".encode_utf16().flat_map(u16::to_le_bytes).collect();
        assert_eq!(etw::text(&name), "a.txt");
        // 1970-01-01 as a FILETIME.
        assert_eq!(etw::filetime_ms(116_444_736_000_000_000), 0);
    }
}
