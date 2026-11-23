//! The Flight Recorder add-on. Record starts a recorder thread that turns
//! every app sample (page open or not) into a trace sample, up to the safe
//! capture limit; Stop opens the Save dialog and writes the trace as JSON.
//! The page shows the live last minute meanwhile. Nothing runs before
//! Record; turned off, it ends the recording and drops what it kept.

/// The recorder's thread and its safe capture limit.
pub mod recorder;
/// Replay: opening and checking a trace, and its position.
pub mod replay;
/// The trace model: header, samples, JSON and the default file name.
pub mod trace;
/// "Export as CSV": a trace as a table.
pub mod export;
/// What the page shows, apart from how it is drawn.
pub mod layout;
/// The page's drawing (functional draft).
mod page;
/// The Thermals panel: temperatures from the shared sensor service.
pub mod thermals;
/// Its settings: the trace folder, the last Replay folder, the Summary tile.
pub mod prefs;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use egui::{RichText, Ui};

use crate::addons::{read_settings, write_settings, Addon, Asked, Capability, Consumption, Recording, Requirements, SensorRecorder, UnsavedWork};
use crate::platform::{Action, AddonContext, PageContext};
use crate::sampler::Snapshot;
use crate::sensors::npu;
use crate::sensors::Interest;

use layout::LiveHistory;
use prefs::Prefs;
use recorder::{Reading, Recorder, SAFE_CAPTURE_LIMIT};
use replay::{Replay, CLOSE_REPLAY_FIRST, PICKER_FAILED, PICKER_TITLE};
use trace::{format_elapsed, samples_label, Header, Panel, PanelHeader, Trace, TRACE_EXTENSION};

/// Record pressed while a recording runs
pub const ALREADY_RECORDING: &str = "A trace is already being recorded.";

/// Why Record is off while the update speed is Paused.
pub const PAUSED: &str = "Updates are paused: choose an update speed to record.";

/// Why Record is off while a stopped trace waits unsaved (or its save
/// failed): the approved prototype turns it off then without a reason.
pub const SAVE_OR_DISCARD_FIRST: &str = "Save or discard the last trace before recording again.";

/// Why Replay… is off during a recording (the prototype turns it off).
pub const STOP_RECORDING_FIRST: &str = "Stop the recording before opening a replay.";

/// Asked when a save fails, with Discard / Cancel
pub const SAVE_FAILED: &str = "The Flight Recorder trace could not be saved. Close and discard the captured trace?";

/// The failed-save question's buttons.
pub const DISCARD: &str = "Discard";
pub const CANCEL: &str = "Cancel";

/// A recording that holds nothing
pub const NO_SAMPLES: &str = "The recording captured no samples.";

/// The chosen path is taken: it is never overwritten
pub const ALREADY_EXISTS: &str = "A recording already exists at that path.";

/// The trace is over [`MAX_TRACE_BYTES`]
pub const TOO_LARGE: &str = "The recording is too large to save safely.";

/// The chosen folder is missing and could not be made
pub const FOLDER_NOT_CREATED: &str = "The recording folder could not be created.";

/// The temporary file could not be written (worded like the folder
/// message).
pub const NOT_WRITTEN: &str = "The recording file could not be written.";

/// The written file could not take the chosen name (worded as above).
pub const NOT_PUBLISHED: &str = "The recording file could not be published.";

/// The largest trace saved: four times the ~64 MB a trace reaches at the
/// safe capture limit (#35), so only a runaway one is refused.
pub const MAX_TRACE_BYTES: usize = 256 * 1024 * 1024;

/// The line for a stopped trace whose Save dialog was cancelled.
pub const UNSAVED: &str = "Unsaved Flight Recorder trace";

/// Why "Export as CSV" is off.
pub const NOTHING_TO_EXPORT: &str = "Record or open a trace to export it.";

/// Its requirement in Settings › Add-ons: the traces it saves.
pub const DISK_NOTE: &str = "Needs disk space for the traces you save (up to about 64 MB each).";

/// Asked before turning it off or closing the app during a recording
/// (Save / Discard / Cancel).
pub const OFF_RECORDING: &str = "A recording is running. Save the trace first?";

/// Asked before turning it off or closing the app with a stopped trace
/// not saved yet.
pub const OFF_UNSAVED: &str = "The last trace is not saved. Save it first?";

/// What its Record can do for another add-on's page (see
/// [`SensorRecorder::sensor_recording`]): Running while it records; Locked while a
/// replay is open (`replaying`) or Record is `blocked`; Ready otherwise.
pub fn recording_state(recording: bool, replaying: bool, blocked: Option<&'static str>) -> Recording {
    if recording {
        Recording::Running
    } else if replaying {
        Recording::Locked(CLOSE_REPLAY_FIRST)
    } else {
        blocked.map_or(Recording::Ready, Recording::Locked)
    }
}

/// The add-on: the recording, a trace left unsaved, a replay, and the
/// live last minute.
#[derive(Default)]
pub struct FlightRecorder {
    /// The running recording and its thread; None before Record.
    recorder: Option<Recorder>,
    /// A stopped trace whose Save dialog was cancelled, or whose save failed.
    unsaved: Option<Trace>,
    /// The trace being replayed and the replay position; None when live.
    replay: Option<Replay>,
    position_ms: u64,
    /// Why the last trace chosen for Replay was refused.
    replay_error: Option<String>,
    /// Record pressed during the replay: the note on the REPLAY card.
    replay_note: Option<&'static str>,
    /// Why the last save failed, while its Discard / Cancel question shows.
    failure: Option<String>,
    /// The trace just recorded and saved, kept for "Export as CSV".
    recorded: Option<Trace>,
    /// For the Save dialog; set while the add-on is on.
    context: Option<AddonContext>,
    /// Its settings, kept while it is off.
    prefs: Prefs,
    /// Held by every recorder thread while it runs: its count of clones
    /// beyond this one is the threads alive.
    alive: Arc<()>,
    /// The live last minute the charts show.
    history: LiveHistory,
    /// The latest sensor frame (Neural Engine, Thermals, the CPU and GPU
    /// temperatures and the GPU clock) the host's lease gave the page at its
    /// last draw while it shows live data (see
    /// [`FlightRecorder::live_shown`]). The host owns the interest behind it
    /// and ends it when the page is not drawn; the add-on keeps only the frame.
    page_frame: Option<Arc<crate::sensors::Frame>>,
    /// The one recording interest (NPU and temperatures), held until Stop.
    recording_sensors: Option<Interest>,
    /// The temperatures of the sensors read at Record, kept up to date.
    feed: thermals::Feed,
    /// Its page was drawn and not hidden since.
    page_open: bool,
    /// Refresh was pressed: the GPU engines are taken again before the
    /// page is next drawn.
    reload_engines: bool,
}

/// The GPU's engines in `s` (None without GPU engine counters).
fn engines_now(s: &Snapshot) -> Option<layout::Engines> {
    if s.perf.gpus.is_empty() {
        return None;
    }
    layout::engines_of(&s.perf.gpus, s.machine.as_deref().map_or(&[], |m| &m.gpus[..]))
}

impl FlightRecorder {
    /// Whether a recording (and its thread) is running.
    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Record: starts the recorder thread at the update speed
    /// `interval_ms`, describing the machine from `sample`. Refused while
    /// paused (0) or while a recording runs.
    pub fn record(&mut self, interval_ms: u64, sample: &Snapshot) -> Result<(), &'static str> {
        // The one recording interest in the shared sensor service (NPU and
        // temperatures), held until Stop. The temperature sensors and NPUs
        // it reports now (none while it still waits for them) enter the header.
        let interest = self.context.as_ref().map(|c| c.sensors().interest());
        let read = interest.as_ref().and_then(|i| {
            i.set_interval_ms(interval_ms);
            i.latest()
        });
        let sensors = read.as_ref().map(|f| thermals::sensors(f)).unwrap_or_default();
        if let (Some(frame), None) = (&read, self.history.npus()) {
            self.history.push_npus(npu::in_frame(frame));
        }
        self.record_sensors(interval_ms, sample, &sensors)?;
        self.recording_sensors = interest;
        Ok(())
    }

    /// Record with the temperature `sensors` read at Record: their names
    /// and keys enter the header, their values every sample.
    pub fn record_sensors(&mut self, interval_ms: u64, sample: &Snapshot, sensors: &[thermals::Sensor]) -> Result<(), &'static str> {
        if self.is_replaying() {
            return Err(CLOSE_REPLAY_FIRST);
        }
        if self.recorder.is_some() {
            return Err(ALREADY_RECORDING);
        }
        if let Some(why) = self.record_blocked(interval_ms) {
            return Err(why);
        }
        let machine = sysinfo::System::host_name().unwrap_or_else(|| "Unknown machine".into());
        let mut header = Header::new(&machine, crate::pro::local_secs(), interval_ms, sample.mem_total, sample.cpu_per_core.len()).split_cores(self.history.topology());
        if let Some(engines) = engines_now(sample) {
            let names = |cells: &[(String, f32)]| cells.iter().map(|c| c.0.clone()).collect::<Vec<_>>();
            header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: names(&engines.cells) });
            let video = engines.video();
            if !video.is_empty() {
                header.panels.push(PanelHeader { panel: Panel::MediaEngine, cells: names(&video) });
            }
            header.gpu = engines.adapter;
        }
        // The NPUs as last read; not read yet, the trace has no Neural Engine.
        if let Some(npus) = self.history.npus() {
            header.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: npus.iter().map(|n| n.name.clone()).collect() });
        }
        if !sensors.is_empty() {
            header.panels.push(PanelHeader { panel: Panel::Thermals, cells: sensors.iter().map(|s| s.name.clone()).collect() });
            header.sensor_keys = sensors.iter().map(|s| s.key.clone()).collect();
        }
        self.feed = thermals::Feed::new(sensors);
        self.recorder = Some(Recorder::start_counted(header, SAFE_CAPTURE_LIMIT, self.alive.clone()));
        Ok(())
    }

    /// Why Record is off at the update speed `interval_ms` (None when it
    /// is on): updates paused, or a stopped trace not saved or discarded.
    pub fn record_blocked(&self, interval_ms: u64) -> Option<&'static str> {
        if interval_ms == 0 {
            Some(PAUSED)
        } else if self.has_unsaved() || self.failure.is_some() {
            Some(SAVE_OR_DISCARD_FIRST)
        } else {
            None
        }
    }

    /// Why Replay… is off (None when it is on): a recording runs.
    pub fn replay_blocked(&self) -> Option<&'static str> {
        self.is_recording().then_some(STOP_RECORDING_FIRST)
    }

    /// Stop: ends the thread and hands back the trace (None when nothing
    /// was being recorded).
    pub fn finish(&mut self) -> Option<Trace> {
        self.recording_sensors = None;
        self.recorder.take().map(Recorder::stop)
    }

    /// Whether the page shows live data: drawn, and no trace replayed. The
    /// one rule for what the page itself takes: the GPU engines (also
    /// taken while recording) and the page interest in the sensors.
    pub fn live_shown(&self) -> bool {
        self.page_open && !self.is_replaying()
    }

    /// Before the page is drawn: opens it (the first time) and, after
    /// Refresh, takes the GPU engines of `sample` again.
    pub fn prepare_page(&mut self, sample: &Snapshot) {
        if !self.page_open {
            self.page_shown(sample);
        }
        if std::mem::take(&mut self.reload_engines) && self.live_shown() {
            self.history.push_engines(engines_now(sample));
        }
    }

    /// The sensors' latest frame: the live page's, else the recording's;
    /// None while neither is held or before it answered.
    fn latest_frame(&self) -> Option<Arc<crate::sensors::Frame>> {
        let page = self.live_shown().then(|| self.page_frame.clone()).flatten();
        page.or_else(|| self.recording_sensors.as_ref()?.latest())
    }

    /// Stop (on the page or the Summary tile): ends the recording and
    /// opens the Save dialog on it.
    pub fn stop_recording(&mut self, actions: &mut Vec<Action>) {
        if let Some(trace) = self.finish() {
            self.offer_save(trace, actions);
        }
    }

    /// The page opens: from now on (while it shows live data) the GPU
    /// engines are taken, starting with the last sample's, and the sensors
    /// are read for it.
    pub fn page_shown(&mut self, sample: &Snapshot) {
        self.page_open = true;
        if self.live_shown() {
            self.history.push_engines(engines_now(sample));
        }
    }

    /// Whether its Summary tile shows: only while recording, unless
    /// turned off in Settings.
    pub fn tile_shown(&self) -> bool {
        self.is_recording() && self.prefs.summary_tile
    }

    /// Whether a trace is being replayed (the page is not live).
    pub fn is_replaying(&self) -> bool {
        self.replay.is_some()
    }

    /// Replay with what the picker gave: a failed picker or a refused
    /// trace returns its message and replays nothing; a cancelled one
    /// changes nothing; a good trace replays from 00:00.
    pub fn replay_picked(&mut self, picked: Result<Option<String>, ()>) -> Option<String> {
        if let Some(why) = self.replay_blocked() {
            return Some(why.into());
        }
        let path = match picked {
            Err(()) => return Some(PICKER_FAILED.into()),
            Ok(None) => return None,
            Ok(Some(path)) => path,
        };
        match replay::open_trace_file(Path::new(&path)) {
            Ok(trace) => {
                self.prefs.remember_replay(Path::new(&path));
                self.replay = Some(Replay::new(trace));
                self.position_ms = 0;
                self.replay_error = None;
                self.replay_note = None;
                // Nothing on the page is live now: the host's lease ends.
                None
            }
            Err(e) => {
                self.replay_error = Some(e.clone());
                Some(e)
            }
        }
    }

    /// Close Replay: back to live.
    pub fn close_replay(&mut self) {
        self.replay = None;
        self.position_ms = 0;
        self.replay_note = None;
    }

    /// Replay…: the picker, filtered on trace files.
    fn open_replay(&mut self, actions: &mut Vec<Action>) {
        let picked = match &self.context {
            Some(c) => c.pick_open(PICKER_TITLE, &replay::picker_filter(), &self.prefs.replay_folder().to_string_lossy()),
            None => Err(()),
        };
        if let Some(message) = self.replay_picked(picked) {
            actions.push(Action::Say(message));
        }
    }

    /// Opens the Save dialog in the trace folder on a dated name and
    /// writes `trace` there; cancelled or failed, the trace stays unsaved.
    fn offer_save(&mut self, trace: Trace, actions: &mut Vec<Action>) {
        let default = self.save_path(&trace);
        let filter = format!("Flight Recorder trace (*.{TRACE_EXTENSION})\0*.{TRACE_EXTENSION}\0\0");
        let chosen = self.context.as_ref().and_then(|c| c.pick_save(&filter, TRACE_EXTENSION, &default.to_string_lossy()));
        let Some(path) = chosen else {
            self.unsaved = Some(trace);
            return;
        };
        self.save_to(Path::new(&path), trace, actions);
    }

    /// Where the Save dialog opens for `trace`: the trace folder, on a
    /// name dated at the recording's start.
    fn save_path(&self, trace: &Trace) -> PathBuf {
        self.prefs.trace_folder().join(trace.header.file_name())
    }

    /// Writes `trace` to `path`. Saved, it is kept for "Export as CSV";
    /// failed, it stays unsaved and the failed-save question (Discard /
    /// Cancel) shows with the reason.
    fn save_to(&mut self, path: &Path, trace: Trace, actions: &mut Vec<Action>) {
        match save_trace(path, &trace) {
            Ok(()) => {
                actions.push(Action::Say(format!("Saved {}", path.display())));
                self.recorded = Some(trace);
            }
            Err(e) => {
                self.unsaved = Some(trace);
                self.failure = Some(e);
            }
        }
    }

    /// Whether a trace waits to be saved or discarded.
    pub fn has_unsaved(&self) -> bool {
        self.unsaved.is_some()
    }

    /// Why the last save failed, while its question is open.
    pub fn save_failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Discard on the failed-save question: the trace is dropped.
    pub fn discard_failed(&mut self) {
        self.failure = None;
        self.discard();
    }

    /// Cancel on the failed-save question: the trace stays unsaved, to
    /// save or discard later.
    pub fn cancel_failed(&mut self) {
        self.failure = None;
    }

    /// The trace "Export as CSV" exports: the one open in Replay, else
    /// the one just recorded, saved or not (None before a recording has
    /// stopped, or once discarded).
    pub fn exportable(&self) -> Option<&Trace> {
        self.replay.as_ref().map(Replay::trace).or(self.unsaved.as_ref()).or(self.recorded.as_ref())
    }

    /// Drops the trace just recorded (Discard on an unsaved trace).
    pub fn discard(&mut self) {
        self.unsaved = None;
        self.recorded = None;
    }

    /// "Export as CSV": opens the Save dialog in the trace folder on a
    /// dated `.csv` name and writes the exportable trace there.
    fn export_csv(&self, actions: &mut Vec<Action>) {
        let Some(trace) = self.exportable() else { return };
        let default = self.prefs.trace_folder().join(export::file_name(trace));
        let chosen = self.context.as_ref().and_then(|c| c.pick_save("CSV (*.csv)\0*.csv\0\0", "csv", &default.to_string_lossy()));
        let Some(path) = chosen else { return };
        match std::fs::write(&path, format!("\u{feff}{}", export::csv(trace))) {
            Ok(()) => actions.push(Action::Say(format!("Saved {path}"))),
            Err(e) => actions.push(Action::Say(format!("Could not save: {e}"))),
        }
    }
}

/// The Summary tile's line: the time recorded and the samples.
pub fn tile_line(elapsed_ms: u64, samples: usize) -> String {
    format!("{}  \u{2022}  {}", format_elapsed(elapsed_ms), samples_label(samples))
}

/// Writes `trace` as JSON to `path` (see [`save_trace_within`]), up to
/// [`MAX_TRACE_BYTES`].
pub fn save_trace(path: &Path, trace: &Trace) -> Result<(), String> {
    save_trace_within(path, trace, MAX_TRACE_BYTES)
}

/// Writes `trace` as JSON to `path`, atomically: a temporary file next to
/// it, then renamed, so a failure never leaves half a trace. Refused, with
/// a message, when the trace is empty, the path is taken, the
/// JSON is over `max_bytes` or the folder cannot be made.
pub fn save_trace_within(path: &Path, trace: &Trace, max_bytes: usize) -> Result<(), String> {
    if trace.samples.is_empty() {
        return Err(NO_SAMPLES.into());
    }
    if path.exists() {
        return Err(ALREADY_EXISTS.into());
    }
    let json = trace.to_json();
    if json.is_empty() || json.len() > max_bytes {
        return Err(TOO_LARGE.into());
    }
    if let Some(folder) = path.parent().filter(|f| !f.as_os_str().is_empty()) {
        std::fs::create_dir_all(folder).map_err(|_| FOLDER_NOT_CREATED.to_string())?;
    }
    let temp = temp_path(path);
    if write_synced(&temp, json.as_bytes()).is_err() {
        if temp.is_file() {
            let _ = std::fs::remove_file(&temp);
        }
        return Err(NOT_WRITTEN.into());
    }
    // Checked again: the path may have been taken while writing.
    if path.exists() {
        let _ = std::fs::remove_file(&temp);
        return Err(ALREADY_EXISTS.into());
    }
    std::fs::rename(&temp, path).map_err(|_| {
        let _ = std::fs::remove_file(&temp);
        NOT_PUBLISHED.to_string()
    })
}

/// The temporary file a trace is written to before it takes `path`'s
/// name: in the same folder, so the rename stays on one volume.
fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.{}.partial", std::process::id()))
}

/// Writes `bytes` to a new file at `path` and flushes it to the disk.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = std::fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

impl Addon for FlightRecorder {
    fn id(&self) -> &'static str {
        "flight-recorder"
    }
    fn name(&self) -> &'static str {
        "Flight Recorder"
    }
    fn description(&self) -> &'static str {
        "Records performance samples so you can look back at a slowdown."
    }
    fn requirements(&self) -> Requirements {
        Requirements { note: Some(DISK_NOTE), ..Requirements::default() }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Low", network: "None", disk: "Only when saving", when: "Every sample while recording" }
    }

    fn start(&mut self, context: &AddonContext) {
        self.context = Some(context.clone());
    }

    /// Ends the recorder thread (joined), closes the replay and frees
    /// every trace; only its settings stay.
    fn stop(&mut self) {
        let _ = self.finish();
        let (prefs, alive) = (std::mem::take(&mut self.prefs), self.alive.clone());
        *self = FlightRecorder { prefs, alive, ..FlightRecorder::default() };
    }

    fn threads(&self) -> usize {
        Arc::strong_count(&self.alive) - 1
    }

    /// It records sensors for other pages (Power & Freq's "Record with
    /// Flight Recorder") and holds the recording that turning off or closing
    /// would lose.
    fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
        match asked {
            Asked::SensorRecorder => Some(Capability::SensorRecorder(self)),
            Asked::UnsavedWork => Some(Capability::UnsavedWork(self)),
            _ => None,
        }
    }

    fn settings(&self) -> serde_json::Value {
        write_settings(&self.prefs)
    }

    fn load_settings(&mut self, settings: &serde_json::Value) {
        self.prefs = read_settings(settings);
    }

    /// The trace folder (Change…, back to Documents) and the Summary tile.
    fn settings_section(&mut self, ui: &mut Ui, page: &mut PageContext) {
        use crate::widgets::parts::{group_heading, setting_folder, setting_switch, FolderAct};
        let _ = page;
        group_heading(ui, "Storage");
        let folder = self.prefs.trace_folder().to_string_lossy().into_owned();
        let custom = !self.prefs.trace_folder.is_empty();
        match setting_folder(ui, "Save traces in", None, &folder, Some(("Use Documents", custom)), true) {
            FolderAct::Choose => {
                if let Some(dir) = self.context.as_ref().and_then(|c| c.pick_folder("Choose the folder for Flight Recorder traces", "Choose")) {
                    self.prefs.trace_folder = dir;
                }
            }
            FolderAct::Reset => self.prefs.trace_folder.clear(),
            FolderAct::None => {}
        }
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("While recording"), &mut self.prefs.summary_tile);
    }

    /// While recording: a red dot, the time, the samples and Stop.
    fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
        if !self.tile_shown() {
            return;
        }
        let Some(rec) = &self.recorder else { return };
        let line = tile_line(rec.elapsed_ms(), rec.status().samples);
        let t = crate::theme::of_ui(ui);
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            // The record dot; its word in the readable red.
            page::dot_word(ui, t.record, t.red, "Flight Recorder");
            ui.label(RichText::new(line).monospace().color(t.label));
            if crate::widgets::parts::destructive_button(ui, "Stop", true).on_hover_text("Stop recording").clicked() {
                self.stop_recording(&mut page.actions);
            }
        });
        // The time moves on: draw again each second.
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
    }

    /// The GPU engines are taken only while the page shows live data or a
    /// recording runs (the rule the page's sensor interest follows too).
    fn on_sample(&mut self, s: &Snapshot) {
        self.history.push(s);
        let engines = if self.live_shown() || self.is_recording() { engines_now(s) } else { None };
        // One frame per sample, for the NPU, the temperatures and the GPU clock alike.
        let frame = self.latest_frame();
        self.history.push_sensors(frame.as_deref());
        let npus = frame.as_deref().map(npu::in_frame);
        if let Some(npus) = &npus {
            self.history.push_npus(npus.clone());
        }
        if let Some(rec) = &self.recorder {
            if let Some(frame) = &frame {
                self.feed.update(frame);
            }
            let topology = self.history.topology();
            let (cores, efficiency_cores) = topology.split(&s.cpu_per_core);
            let (core_clocks_ghz, efficiency_clocks_ghz) = topology.split(&layout::core_clocks_ghz(s));
            let gpu_engines = engines.as_ref().map(|e| e.cells.clone()).unwrap_or_default();
            let thermals = self.feed.values().to_vec();
            let mut extras = self.history.extras();
            // The hottest of every sensor, those read at Record included.
            extras.hottest_c = extras.hottest_c.into_iter().chain(thermals.iter().copied()).reduce(f32::max);
            rec.feed(Reading { at: Instant::now(), cores, efficiency_cores, memory_used_bytes: s.mem_used, npus, thermals, gpu_engines, core_clocks_ghz, efficiency_clocks_ghz, extras });
        }
        if engines.is_some() {
            self.history.push_engines(engines);
        }
    }

    /// Refresh: the sensors are read again now (the host reads for the
    /// page's lease; the recording's own interest here) and the GPU engines
    /// taken again before the page is next drawn (#40).
    fn refresh(&mut self) {
        if let Some(interest) = &self.recording_sensors {
            interest.read_now();
        }
        self.reload_engines = true;
    }

    /// Stops taking the GPU engines; the host ends the page's sensor
    /// interest itself (a recording keeps its own until Stop).
    fn page_hidden(&mut self) {
        self.page_open = false;
        self.page_frame = None;
    }

    fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.prepare_page(page.sample);
        // Asking is what keeps the host's lease: only while live.
        self.page_frame = if self.live_shown() {
            page.sensors().and_then(|s| {
                s.set_interval_ms(page.settings.interval_ms());
                s.latest()
            })
        } else {
            None
        };
        self.page(ui, page);
    }
}

/// What other pages see of it and ask of it: see [`recording_state`]. Record
/// (with the sensors) or Stop (which opens the Save dialog), exactly as its
/// own buttons do.
impl UnsavedWork for FlightRecorder {
    fn unsaved_work(&self) -> Option<&'static str> {
        if self.is_recording() {
            Some(OFF_RECORDING)
        } else if self.has_unsaved() {
            Some(OFF_UNSAVED)
        } else {
            None
        }
    }

    /// Save before turning off: stops the recording and opens the Save
    /// dialog; true only when the trace was written.
    fn save_work(&mut self) -> bool {
        let Some(trace) = self.finish().or_else(|| self.unsaved.take()) else {
            return true;
        };
        self.offer_save(trace, &mut Vec::new());
        !self.has_unsaved()
    }
}

impl SensorRecorder for FlightRecorder {
    fn sensor_recording(&self, interval_ms: u64) -> Recording {
        recording_state(self.is_recording(), self.is_replaying(), self.record_blocked(interval_ms))
    }

    fn set_sensor_recording(&mut self, on: bool, page: &mut PageContext) -> Result<(), &'static str> {
        if on {
            self.record(page.settings.interval_ms(), page.sample)
        } else {
            self.stop_recording(&mut page.actions);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::AddonContext;
    use crate::sampler::Snapshot;

    /// A two-core sample at `load` percent, a quarter of 100 bytes in use.
    fn sample(load: f32) -> Snapshot {
        Snapshot { cpu_total: load, cpu_per_core: vec![load, load / 2.0], mem_total: 100, mem_used: 25, ..Default::default() }
    }

    const NORMAL: u64 = 1000;

    fn started() -> FlightRecorder {
        let mut fr = FlightRecorder::default();
        fr.start(&AddonContext::recording().0);
        fr
    }

    #[test]
    fn record_starts_the_recording_thread_and_nothing_runs_before() {
        let mut fr = started();
        fr.on_sample(&sample(10.0));
        assert!(!fr.is_recording(), "no thread before Record");
        assert_eq!(fr.record(NORMAL, &sample(10.0)), Ok(()));
        assert!(fr.is_recording());
        fr.stop();
    }

    #[test]
    fn stop_hands_back_every_sample_from_zero_with_its_header() {
        let mut fr = started();
        fr.on_sample(&sample(10.0));
        fr.record(NORMAL, &sample(10.0)).unwrap();
        fr.on_sample(&sample(20.0));
        fr.on_sample(&sample(30.0));
        let trace = fr.finish().expect("a trace");
        assert!(!fr.is_recording(), "the thread is gone");
        assert_eq!(trace.header.interval_ms, NORMAL);
        assert_eq!(trace.header.memory_total_bytes, 100);
        assert_eq!(trace.header.panels[0].cells, ["CPU 0", "CPU 1"]);
        assert_eq!(trace.samples.len(), 2, "only the samples taken while recording");
        assert_eq!(trace.samples[0].t_ms, 0);
        assert_eq!(trace.samples[1].panels[0].series, vec![vec![30.0, 15.0]]);
        assert_eq!(trace.samples[1].memory_used_bytes, 25);
    }

    #[test]
    fn a_hybrid_recording_keeps_its_efficiency_cores_through_save_and_replay() {
        let mut fr = started();
        // CPU 1 is the efficiency core.
        fr.history = LiveHistory::with_topology(crate::cpu_topology::CoreTopology::from_efficiency_classes(&[1, 0]));
        fr.record(NORMAL, &sample(10.0)).unwrap();
        fr.on_sample(&sample(40.0));
        let trace = fr.finish().expect("a trace");
        let saved = Trace::from_json(&trace.to_json()).unwrap();
        assert_eq!(layout::header_counts(&saved.header), (1, 1, 0));
        let replay = Replay::new(saved);
        let panels = layout::replay_panels(&replay, 0, false);
        let efficiency = panels.iter().find(|p| p.slot == layout::Slot::EfficiencyCores).expect("the Efficiency cores panel");
        assert_eq!((efficiency.cells.as_slice(), efficiency.names.as_slice()), ([20.0].as_slice(), ["CPU 1".to_string()].as_slice()));
        let performance = panels.iter().find(|p| p.slot == layout::Slot::PerformanceCores).unwrap();
        assert_eq!(performance.cells, [40.0]);
    }

    #[test]
    fn the_sensors_read_at_record_are_named_in_the_header_and_recorded_in_every_sample() {
        use thermals::Sensor;
        let mut fr = started();
        let read = [Sensor { key: "cpu.temp".into(), name: "CPU: Package".into(), celsius: 61.0 }, Sensor { key: "gpu0.temp".into(), name: "GPU: GPU Temperature".into(), celsius: 49.0 }];
        fr.record_sensors(NORMAL, &sample(1.0), &read).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        let thermals = trace.header.panels.iter().find(|p| p.panel == trace::Panel::Thermals).expect("a Thermals panel");
        assert_eq!(thermals.cells, ["CPU: Package", "GPU: GPU Temperature"]);
        assert_eq!(trace.header.sensor_keys, ["cpu.temp", "gpu0.temp"]);
        assert_eq!(trace.samples[0].panels[1].series, vec![vec![61.0, 49.0]]);
        assert_eq!(replay::check(&trace), Ok(()));
        assert_eq!(fr.threads(), 0, "no thread left");
    }

    #[test]
    fn without_temperature_sensors_the_trace_has_no_thermals_panel() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        assert!(trace.header.panels.iter().all(|p| p.panel != trace::Panel::Thermals), "the made-up CPU has no temperature");
        assert!(trace.header.sensor_keys.is_empty());
        assert_eq!(replay::check(&trace), Ok(()));
    }

    #[test]
    fn a_second_recording_is_refused_with_a_message() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Err("A trace is already being recorded."));
        fr.stop();
    }

    /// A sample of a PC whose one GPU has a 3D and a video decode engine.
    fn gpu_sample(load_3d: f32, decode: f32) -> Snapshot {
        let gpu = crate::perfdata::GpuLive { luid: 7, cells: vec![("3D".into(), load_3d), ("Video decode".into(), decode)], ..Default::default() };
        let mut s = sample(10.0);
        s.perf.gpus = vec![gpu];
        s
    }

    #[test]
    fn gpu_engines_are_taken_only_while_the_page_is_open_or_recording() {
        let mut fr = started();
        fr.on_sample(&gpu_sample(40.0, 5.0));
        assert!(fr.history.engines().is_none(), "page closed, not recording: no engines taken");
        fr.page_shown(&gpu_sample(40.0, 5.0));
        assert_eq!(fr.history.engines().map(|e| e.cells.clone()), Some(vec![("3D".into(), 40.0), ("Video decode".into(), 5.0)]), "taken as the page opens");
        fr.on_sample(&gpu_sample(60.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 60.0);
        fr.page_hidden();
        fr.on_sample(&gpu_sample(90.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 60.0, "closed again: no more taken");
    }

    #[test]
    fn a_replay_takes_neither_the_gpu_engines_nor_the_sensors_for_the_page() {
        let path = trace_file("gating", &recorded_trace().to_json());
        let mut fr = started();
        fr.prepare_page(&gpu_sample(40.0, 5.0));
        assert!(fr.live_shown(), "shown live");
        fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert!(!fr.live_shown());
        fr.on_sample(&gpu_sample(90.0, 5.0));
        fr.prepare_page(&gpu_sample(90.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 40.0, "no engines taken during the replay");
        fr.close_replay();
        assert!(fr.live_shown(), "back to live");
        fr.on_sample(&gpu_sample(70.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 70.0);
        fr.page_hidden();
        assert!(!fr.live_shown());
    }

    #[test]
    fn refresh_takes_the_gpu_engines_again() {
        let mut fr = started();
        fr.prepare_page(&gpu_sample(40.0, 5.0));
        fr.prepare_page(&gpu_sample(55.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 40.0, "drawn again, nothing new without a sample");
        fr.refresh();
        fr.prepare_page(&gpu_sample(55.0, 5.0));
        assert_eq!(fr.history.engines().unwrap().cells[0].1, 55.0, "Refresh reloads them");
    }

    #[test]
    fn a_recording_holds_the_gpu_and_video_engines_page_closed() {
        let mut fr = started();
        fr.record(NORMAL, &gpu_sample(40.0, 5.0)).unwrap();
        fr.on_sample(&gpu_sample(70.0, 25.0));
        let trace = fr.finish().unwrap();
        let cells = |panel| trace.header.panels.iter().find(|h| h.panel == panel).map(|h| h.cells.clone());
        assert_eq!(cells(trace::Panel::GpuCores), Some(vec!["3D".to_string(), "Video decode".to_string()]));
        assert_eq!(cells(trace::Panel::MediaEngine), Some(vec!["Video decode".to_string()]));
        assert!(!trace.header.gpu.is_empty(), "the adapter is named");
        let values = |panel| trace.samples[0].panels.iter().find(|p| p.panel == panel).map(|p| p.series.clone());
        assert_eq!(values(trace::Panel::GpuCores), Some(vec![vec![70.0, 25.0]]));
        assert_eq!(values(trace::Panel::MediaEngine), Some(vec![vec![25.0]]));
        let back = replay::open_trace(&trace.to_json()).expect("a valid trace");
        let panels = layout::replay_panels(&Replay::new(back), 0, false);
        assert_eq!(panels.iter().find(|p| p.slot == layout::Slot::GpuCores).unwrap().cells, [70.0, 25.0], "shown in replay");
    }

    #[test]
    fn a_pc_without_gpu_counters_records_no_engine_panels() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        assert_eq!(trace.header.panels.len(), 1, "cores only");
    }

    /// Lists this PC's GPU engines as the GPU cores and Media Engine
    /// panels show them (expected on the RTX 4080 here: 14 and 3).
    #[test]
    #[ignore]
    fn lists_this_pcs_gpu_engines() {
        use crate::perfdata::counter;
        let service = crate::sensors::SensorService::grouped(crate::sensors::provider::standard());
        let interest = service.interest_in(crate::sensors::service::ProviderGroup::Counters);
        interest.set_interval_ms(1000);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let frame = loop {
            if let Some(f) = interest.latest() {
                break f;
            }
            assert!(std::time::Instant::now() < end, "the counter providers gave no frame");
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        let of = |key: &str| frame.counter(key).map(|c| c.instances.iter().map(|i| (i.name.clone(), i.value)).collect::<Vec<_>>()).unwrap_or_default();
        let gpus = crate::perfdata::gpu_adapters(&of(counter::GPU), &of(counter::GDED), &of(counter::GSH));
        let engines = layout::engines_of(&gpus, &crate::sensors::machine::read().gpus).expect("a GPU with engine counters");
        println!("{}: {} engines", engines.adapter, engines.cells.len());
        for (name, v) in &engines.cells {
            println!("  {name}: {v:.1}%");
        }
        println!("Media Engine: {:?}", engines.video().iter().map(|c| &c.0).collect::<Vec<_>>());
        assert!(!engines.cells.is_empty());
    }

    #[test]
    fn record_is_off_while_a_trace_waits_unsaved_or_its_save_failed() {
        let mut fr = started();
        assert_eq!(fr.record_blocked(NORMAL), None);
        assert_eq!(fr.record_blocked(0), Some(PAUSED));
        fr.offer_save(one_sample_trace(), &mut Vec::new());
        assert_eq!(fr.record_blocked(NORMAL), Some(SAVE_OR_DISCARD_FIRST));
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Err(SAVE_OR_DISCARD_FIRST));
        assert!(!fr.is_recording());
        fr.failure = Some(ALREADY_EXISTS.into());
        assert_eq!(fr.record_blocked(NORMAL), Some(SAVE_OR_DISCARD_FIRST), "a failed save too");
        fr.discard_failed();
        assert_eq!(fr.record_blocked(NORMAL), None, "discarded, Record is back");
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Ok(()));
        fr.stop();
    }

    #[test]
    fn replay_is_off_during_a_recording() {
        let path = trace_file("busy", &recorded_trace().to_json());
        let mut fr = started();
        assert_eq!(fr.replay_blocked(), None);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        assert_eq!(fr.replay_blocked(), Some(STOP_RECORDING_FIRST));
        let said = fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(said, Some(STOP_RECORDING_FIRST.to_string()));
        assert!(!fr.is_replaying() && fr.is_recording());
        fr.stop();
    }

    #[test]
    fn record_is_refused_while_updates_are_paused() {
        let mut fr = started();
        assert_eq!(fr.record(0, &sample(1.0)), Err(PAUSED));
        assert!(!fr.is_recording());
    }

    #[test]
    fn turning_off_during_a_recording_ends_its_thread_and_drops_it() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(20.0));
        fr.stop();
        assert!(!fr.is_recording());
        assert!(fr.finish().is_none(), "its memory is freed");
        fr.start(&AddonContext::recording().0);
        fr.on_sample(&sample(30.0));
        assert!(!fr.is_recording(), "turned on again, it does not record by itself");
    }

    /// Feeds samples (page shown) until the sensors have answered once.
    fn until_npus_known(fr: &mut FlightRecorder) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while fr.history.npus().is_none() {
            draw_leased(fr);
            assert!(std::time::Instant::now() < end, "the sensors never answered");
            std::thread::sleep(std::time::Duration::from_millis(5));
            fr.on_sample(&sample(1.0));
        }
    }

    /// Draws the page as the host does: its sensors come from the leases of
    /// the add-on's context.
    fn draw_leased(fr: &mut FlightRecorder) {
        let (settings, snap) = (crate::settings::Settings::default(), sample(1.0));
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut page = PageContext::new(&settings, &snap);
                page.lease = fr.context.as_ref().map(|c| c.leases().lease(crate::addons::leases::Surface::Page("flight-recorder")));
                fr.ui(ui, &mut page);
            });
        });
    }

    #[test]
    fn the_neural_engine_is_recorded_from_the_shared_sensors_and_this_pc_has_none() {
        let mut fr = started();
        until_npus_known(&mut fr);
        let live = layout::live_panels(&fr.history, &sample(1.0), false);
        assert_eq!(live.iter().find(|p| p.slot == layout::Slot::NeuralEngine).unwrap().sub, "No NPU in this PC", "the test sensors have no NPU");
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.page_hidden();
        fr.on_sample(&sample(2.0));
        let trace = fr.finish().unwrap();
        let neural = trace.header.panels.iter().find(|p| p.panel == trace::Panel::NeuralEngine).expect("the Neural Engine is in the trace");
        assert!(neural.cells.is_empty());
        assert_eq!(trace.samples[0].panels.iter().filter(|p| p.panel == trace::Panel::NeuralEngine).count(), 1);
        assert_eq!(replay::check(&trace), Ok(()));
    }

    /// A host with Flight Recorder alone.
    fn recorder_host() -> crate::addons::AddonHost {
        let entries = vec![crate::addons::Entry::Addon(Box::<FlightRecorder>::default())];
        crate::addons::AddonHost::for_tests_with(entries, &serde_json::json!({})).0
    }

    /// One frame with the page drawn through the host, then the frame ends.
    fn draw_through(host: &mut crate::addons::AddonHost) {
        crate::addons::draw_page_with(host, "flight-recorder", &sample(1.0));
    }

    #[test]
    fn the_live_page_holds_no_sensor_interest_of_its_own() {
        let mut host = recorder_host();
        draw_through(&mut host);
        assert!(crate::addons::sensor_threads(&host) > 0, "the page reads the sensors while drawn");
        // Only the host's lease keeps the service running for the page.
        host.context.leases().release("flight-recorder");
        crate::addons::until("no sensor thread", || crate::addons::sensor_threads(&host) == 0);
    }

    #[test]
    fn leaving_the_page_while_nothing_records_stops_the_service() {
        let mut host = recorder_host();
        for _ in 0..3 {
            draw_through(&mut host);
        }
        assert!(crate::addons::sensor_threads(&host) > 0);
        host.end_frame(); // the first frame without the page
        crate::addons::until("no sensor thread", || crate::addons::sensor_threads(&host) == 0);
    }

    #[test]
    fn a_recording_keeps_the_service_running_after_the_page_is_hidden_and_stop_ends_it() {
        let mut host = recorder_host();
        draw_through(&mut host);
        let (settings, snap) = (crate::settings::Settings::default(), sample(1.0));
        let mut page = PageContext::new(&settings, &snap);
        assert_eq!(host.record_sensors(true, &mut page), Ok(()));
        draw_through(&mut host);
        host.end_frame(); // the page is hidden
        host.end_frame();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(crate::addons::sensor_threads(&host) > 0, "the recording keeps the service running");
        assert_eq!(host.record_sensors(false, &mut page), Ok(()));
        crate::addons::until("no sensor thread after Stop", || crate::addons::sensor_threads(&host) == 0);
    }

    #[test]
    fn refresh_on_the_page_reads_the_sensors_at_once() {
        let mut host = recorder_host();
        draw_through(&mut host);
        // Only the host's lease for the page exists; its update interval is far
        // away, so nothing reads by itself while this test waits.
        let page = host.context.leases().held_by("flight-recorder").expect("the host keeps the sensors for the page");
        page.set_interval_ms(600_000);
        crate::addons::until("a frame", || page.latest().is_some());
        let before = page.latest().unwrap().seq;
        assert!(host.refresh("flight-recorder"));
        crate::addons::until("a newer frame", || page.latest().is_some_and(|f| f.seq > before));
    }

    #[test]
    fn the_sensors_run_only_while_the_page_is_shown_or_a_recording_runs() {
        let mut fr = started();
        let service = fr.context.clone().unwrap();
        let idle = |why: &str| {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while service.sensors().threads() > 0 {
                assert!(std::time::Instant::now() < end, "sensor threads alive {why}");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        fr.on_sample(&sample(1.0));
        idle("before the page was shown");
        until_npus_known(&mut fr);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.page_hidden();
        service.leases().end_frame(); // the host ends the page's lease
        service.leases().end_frame();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(service.sensors().threads() > 0, "the recording keeps reading the NPU");
        fr.finish();
        idle("after Stop with the page hidden");
    }

    /// Real sensors (DXCore): this PC has no NPU, so the panel says so.
    /// `cargo test --release flight_recorder::tests::lists_this_pcs_npu -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn lists_this_pcs_npu() {
        let mut fr = FlightRecorder::default();
        fr.start(&AddonContext::system());
        let end = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while fr.history.npus().is_none() {
            draw_leased(&mut fr);
            assert!(std::time::Instant::now() < end, "the sensors never answered");
            std::thread::sleep(std::time::Duration::from_millis(50));
            fr.on_sample(&sample(1.0));
        }
        println!("NPUs: {:?}", fr.history.npus());
        let live = layout::live_panels(&fr.history, &sample(1.0), false);
        let neural = live.iter().find(|p| p.slot == layout::Slot::NeuralEngine).unwrap();
        println!("Neural Engine: {} | {} | {}", neural.sub, neural.value, neural.note);
        assert_eq!((neural.sub.as_str(), neural.value.as_str()), ("No NPU in this PC", "Unavailable"), "expected: no NPU on this PC");
        fr.stop();
    }

    #[test]
    fn reading_the_npu_while_recording_stays_within_the_sample_budget() {
        let mut fr = started();
        until_npus_known(&mut fr);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        let n = 200;
        let t = std::time::Instant::now();
        for i in 0..n {
            fr.on_sample(&sample(i as f32 % 100.0));
        }
        let each = t.elapsed() / n;
        fr.stop();
        assert!(each < crate::addons::SAMPLE_HOOK_BUDGET, "{each:?} per sample");
    }

    #[test]
    fn a_saved_trace_is_its_json() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        let dir = std::env::temp_dir().join(format!("fr-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.upmtrace");
        let saved = save_trace(&path, &trace);
        let back = std::fs::read_to_string(&path).map(|t| Trace::from_json(&t));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, Ok(()));
        assert_eq!(back.unwrap().unwrap(), trace);
    }

    #[test]
    fn a_trace_without_samples_is_not_saved() {
        let trace = Trace::new(Header::new("PC", 0, NORMAL, 1, 1));
        let path = std::env::temp_dir().join(format!("fr-empty-{}.upmtrace", std::process::id()));
        assert_eq!(save_trace(&path, &trace), Err("The recording captured no samples.".to_string()));
        assert!(!path.exists());
    }

    /// A trace of one sample, as Stop hands back.
    fn one_sample_trace() -> Trace {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        fr.finish().unwrap()
    }

    /// A fresh scratch folder for one test (removed by the test).
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("fr-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The names of the files in `dir`.
    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn a_path_that_already_exists_is_refused_and_left_alone() {
        let dir = scratch("exists");
        let path = dir.join("t.upmtrace");
        std::fs::write(&path, "earlier").unwrap();
        let saved = save_trace(&path, &one_sample_trace());
        let kept = std::fs::read_to_string(&path).unwrap();
        let names = files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, Err("A recording already exists at that path.".to_string()));
        assert_eq!(kept, "earlier");
        assert_eq!(names, ["t.upmtrace"], "no temporary file left");
    }

    #[test]
    fn a_trace_too_large_to_save_safely_is_refused_and_writes_nothing() {
        let dir = scratch("large");
        let path = dir.join("t.upmtrace");
        let trace = one_sample_trace();
        let saved = save_trace_within(&path, &trace, 10);
        let names = files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, Err("The recording is too large to save safely.".to_string()));
        assert!(names.is_empty(), "nothing written: {names:?}");
    }

    #[test]
    fn a_missing_folder_is_created_and_one_that_cannot_be_is_reported() {
        let dir = scratch("folder");
        let nested = dir.join("a").join("b").join("t.upmtrace");
        let made = save_trace(&nested, &one_sample_trace());
        std::fs::write(dir.join("file"), "x").unwrap();
        let blocked = save_trace(&dir.join("file").join("t.upmtrace"), &one_sample_trace());
        let nested_exists = nested.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(made, Ok(()));
        assert!(nested_exists);
        assert_eq!(blocked, Err("The recording folder could not be created.".to_string()));
    }

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let dir = scratch("partial");
        let path = dir.join("t.upmtrace");
        // The temporary file's place is taken by a folder: it cannot be written.
        std::fs::create_dir_all(temp_path(&path)).unwrap();
        let saved = save_trace(&path, &one_sample_trace());
        let final_exists = path.exists();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, Err("The recording file could not be written.".to_string()));
        assert!(!final_exists, "no half-written trace at the chosen path");
    }

    #[test]
    fn a_saved_trace_leaves_only_its_file() {
        let dir = scratch("only");
        let saved = save_trace(&dir.join("t.upmtrace"), &one_sample_trace());
        let names = files_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(saved, Ok(()));
        assert_eq!(names, ["t.upmtrace"]);
    }

    #[test]
    fn a_failed_save_asks_to_discard_and_discard_drops_the_trace() {
        let dir = scratch("discard");
        let path = dir.join("t.upmtrace");
        std::fs::write(&path, "earlier").unwrap();
        let mut fr = started();
        let mut actions = Vec::new();
        fr.save_to(&path, one_sample_trace(), &mut actions);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(fr.save_failure(), Some("A recording already exists at that path."));
        assert!(fr.has_unsaved(), "the trace is kept until the user decides");
        fr.discard_failed();
        assert_eq!(fr.save_failure(), None);
        assert!(!fr.has_unsaved(), "Discard drops the trace");
    }

    #[test]
    fn cancel_on_a_failed_save_keeps_the_trace_unsaved() {
        let dir = scratch("cancel");
        let path = dir.join("t.upmtrace");
        std::fs::write(&path, "earlier").unwrap();
        let mut fr = started();
        let mut actions = Vec::new();
        fr.save_to(&path, one_sample_trace(), &mut actions);
        fr.cancel_failed();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(fr.save_failure(), None);
        assert!(fr.has_unsaved(), "Cancel keeps it as an unsaved trace to save later");
    }

    #[test]
    fn a_successful_save_says_where_and_keeps_nothing_unsaved() {
        let dir = scratch("ok");
        let path = dir.join("t.upmtrace");
        let mut fr = started();
        let mut actions = Vec::new();
        fr.save_to(&path, one_sample_trace(), &mut actions);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(fr.save_failure(), None);
        assert!(!fr.has_unsaved());
        assert!(matches!(&actions[..], [Action::Say(s)] if s.starts_with("Saved ")));
    }

    #[test]
    fn the_save_messages_read_as_written() {
        assert_eq!(SAVE_FAILED, "The Flight Recorder trace could not be saved. Close and discard the captured trace?");
        assert_eq!(UNSAVED, "Unsaved Flight Recorder trace");
        assert_eq!((DISCARD, CANCEL), ("Discard", "Cancel"));
    }

    #[test]
    fn the_save_dialog_offers_a_name_dated_at_the_recordings_start() {
        let mut fr = started();
        fr.load_settings(&serde_json::json!({ "traceFolder": "D:\\Traces" }));
        let mut trace = one_sample_trace();
        trace.header.recorded = "2026-11-15T21:04:30".into();
        assert_eq!(fr.save_path(&trace), PathBuf::from("D:\\Traces").join(format!("flight-recorder_20261115_2104.{TRACE_EXTENSION}")));
    }

    #[test]
    fn stop_without_a_dialog_keeps_the_trace_unsaved() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        let mut actions = Vec::new();
        fr.offer_save(trace, &mut actions);
        assert!(fr.unsaved.is_some(), "a cancelled Save dialog keeps the trace");
        assert!(actions.is_empty());
    }

    /// `trace` saved to a temporary file, whose path comes back.
    fn trace_file(name: &str, text: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fr-replay-{name}-{}.upmtrace", std::process::id()));
        std::fs::write(&path, text).unwrap();
        path
    }

    fn recorded_trace() -> Trace {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        fr.on_sample(&sample(7.0));
        fr.finish().unwrap()
    }

    #[test]
    fn a_picker_that_cannot_open_says_so_and_replays_nothing() {
        let mut fr = started();
        assert_eq!(fr.replay_picked(Err(())), Some(replay::PICKER_FAILED.to_string()));
        assert!(!fr.is_replaying());
    }

    #[test]
    fn a_cancelled_picker_changes_nothing() {
        let mut fr = started();
        assert_eq!(fr.replay_picked(Ok(None)), None);
        assert!(!fr.is_replaying());
    }

    #[test]
    fn a_saved_trace_replays_until_closed() {
        let trace = recorded_trace();
        let path = trace_file("good", &trace.to_json());
        let mut fr = started();
        let said = fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(said, None);
        assert!(fr.is_replaying());
        fr.close_replay();
        assert!(!fr.is_replaying(), "back to live");
    }

    #[test]
    fn a_rejected_trace_shows_its_reason_and_none_of_its_data() {
        let mut trace = recorded_trace();
        trace.samples[0].t_ms = 7;
        let path = trace_file("late", &trace.to_json());
        let mut fr = started();
        let said = fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(said, Some("The trace must begin at timestamp zero.".to_string()));
        assert!(!fr.is_replaying());
    }

    #[test]
    fn the_trace_open_in_replay_is_the_one_exported() {
        let trace = recorded_trace();
        let path = trace_file("export", &trace.to_json());
        let mut fr = started();
        fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(fr.exportable(), Some(&trace));
        fr.close_replay();
        assert_eq!(fr.exportable(), None, "closed, there is nothing to export");
    }

    #[test]
    fn record_during_a_replay_is_refused_with_a_message() {
        let path = trace_file("rec", &recorded_trace().to_json());
        let mut fr = started();
        fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Err("Close the replay before starting a recording."));
        assert!(!fr.is_recording());
    }

    #[test]
    fn export_is_off_until_a_trace_has_been_recorded() {
        let mut fr = started();
        assert!(fr.exportable().is_none(), "nothing recorded yet");
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        assert!(fr.exportable().is_none(), "the recording is still running");
        fr.stop();
    }

    #[test]
    fn export_offers_the_trace_just_recorded_until_it_is_discarded() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        let mut actions = Vec::new();
        fr.offer_save(trace.clone(), &mut actions);
        assert_eq!(fr.exportable(), Some(&trace), "a stopped trace, even unsaved, can be exported");
        fr.discard();
        assert!(fr.exportable().is_none(), "a discarded trace is gone");
    }

    #[test]
    fn turning_off_drops_the_trace_to_export() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        fr.offer_save(trace, &mut Vec::new());
        fr.stop();
        assert!(fr.exportable().is_none());
    }

    #[test]
    fn it_needs_no_admin_internet_or_program_and_says_traces_take_disk_space() {
        let r = FlightRecorder::default().requirements();
        assert!(!r.admin && !r.network && r.programs.is_empty());
        assert_eq!(r.note, Some(DISK_NOTE));
    }

    #[test]
    fn its_thread_runs_only_while_recording() {
        let mut fr = started();
        fr.on_sample(&sample(1.0));
        assert_eq!(fr.threads(), 0, "on, page closed, not recording: no thread");
        fr.record(NORMAL, &sample(1.0)).unwrap();
        assert_eq!(fr.threads(), 1);
        fr.finish();
        assert_eq!(fr.threads(), 0, "Stop ends it");
    }

    #[test]
    fn a_recording_or_an_unsaved_trace_is_work_turning_off_would_lose() {
        let mut fr = started();
        assert_eq!(fr.unsaved_work(), None);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        assert_eq!(fr.unsaved_work(), Some(OFF_RECORDING));
        fr.on_sample(&sample(5.0));
        let trace = fr.finish().unwrap();
        fr.offer_save(trace, &mut Vec::new());
        assert_eq!(fr.unsaved_work(), Some(OFF_UNSAVED), "its Save dialog was cancelled");
        fr.discard();
        assert_eq!(fr.unsaved_work(), None);
    }

    #[test]
    fn save_before_turning_off_stops_the_recording_and_fails_when_the_dialog_is_cancelled() {
        let mut fr = started();
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        assert!(!fr.save_work(), "no dialog in tests: not saved, so it stays on");
        assert!(!fr.is_recording());
        assert_eq!(fr.threads(), 0);
        assert!(fr.has_unsaved(), "the trace is kept to save later");
    }

    /// A host with only Flight Recorder, on, recording.
    fn recording_host() -> crate::addons::AddonHost {
        let mut fr = FlightRecorder::default();
        fr.start(&AddonContext::recording().0);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        fr.on_sample(&sample(5.0));
        let entries = vec![crate::addons::Entry::Addon(Box::new(fr))];
        crate::addons::AddonHost::for_tests_with(entries, &serde_json::json!({})).0
    }

    #[test]
    fn turning_off_during_a_recording_asks_then_discard_leaves_no_thread() {
        use crate::addons::OffAnswer;
        let mut host = recording_host();
        assert_eq!(host.threads("flight-recorder"), 1);
        host.set_enabled("flight-recorder", false);
        assert_eq!(host.off_question().map(|q| q.question), Some(OFF_RECORDING));
        assert_eq!(host.threads("flight-recorder"), 1, "still recording while it asks");
        assert!(host.answer_off(OffAnswer::Discard));
        assert!(!host.is_enabled("flight-recorder"));
        assert!(!host.visible().contains(&"flight-recorder"));
        assert_eq!(host.threads("flight-recorder"), 0);
    }

    #[test]
    fn closing_the_app_during_a_recording_asks_and_discard_leaves_no_thread() {
        use crate::addons::OffAnswer;
        let mut host = recording_host();
        assert!(!host.request_close(), "the app stays open");
        assert_eq!(host.close_save_question().map(|q| q.question), Some(OFF_RECORDING));
        assert_eq!(host.threads("flight-recorder"), 1, "still recording while it asks");
        assert!(host.answer_close(OffAnswer::Discard), "the app closes");
        assert_eq!(host.threads("flight-recorder"), 0, "the recorder thread does not outlive closing");
    }

    #[test]
    fn closing_the_app_with_an_unsaved_trace_asks_the_same_question() {
        let mut host = recording_host();
        host.request_close();
        // Save stops the recording; no dialog in tests, so it stays unsaved.
        assert!(!host.answer_close(crate::addons::OffAnswer::Save), "a save that did not happen keeps the app open");
        assert_eq!(host.threads("flight-recorder"), 0);
        assert!(!host.request_close());
        assert_eq!(host.close_save_question().map(|q| q.question), Some(OFF_UNSAVED));
    }

    #[test]
    fn cancel_on_closing_the_app_keeps_the_recording_running() {
        let mut host = recording_host();
        host.request_close();
        assert!(!host.answer_close(crate::addons::OffAnswer::Cancel));
        assert_eq!(host.threads("flight-recorder"), 1);
        assert_eq!(host.close_save_question(), None);
        host.request_close();
        host.answer_close(crate::addons::OffAnswer::Discard);
    }

    #[test]
    fn cancel_keeps_the_recording_running() {
        use crate::addons::OffAnswer;
        let mut host = recording_host();
        host.set_enabled("flight-recorder", false);
        host.answer_off(OffAnswer::Cancel);
        assert!(host.is_enabled("flight-recorder"));
        assert_eq!(host.threads("flight-recorder"), 1);
        host.set_enabled("flight-recorder", false);
        host.answer_off(OffAnswer::Discard);
        assert_eq!(host.threads("flight-recorder"), 0);
    }

    #[test]
    fn turned_off_it_frees_the_trace_and_closes_the_replay_but_keeps_its_settings() {
        let path = trace_file("off", &recorded_trace().to_json());
        let mut fr = started();
        fr.load_settings(&serde_json::json!({ "traceFolder": "D:\\Traces", "summaryTile": false }));
        fr.replay_picked(Ok(Some(path.to_string_lossy().into())));
        let _ = std::fs::remove_file(&path);
        let before = fr.settings();
        fr.stop();
        assert!(!fr.is_replaying(), "the replay is closed");
        assert!(fr.exportable().is_none(), "no trace kept");
        fr.start(&AddonContext::recording().0);
        assert_eq!(fr.settings(), before, "its settings are back");
        assert_eq!(before["replayFolder"], serde_json::json!(std::env::temp_dir().to_string_lossy().trim_end_matches('\\')));
    }

    #[test]
    fn the_summary_tile_shows_only_while_recording_and_when_asked_for() {
        let mut fr = started();
        assert!(!fr.tile_shown(), "not recording");
        fr.record(NORMAL, &sample(1.0)).unwrap();
        assert!(fr.tile_shown());
        fr.load_settings(&serde_json::json!({ "summaryTile": false }));
        assert!(!fr.tile_shown(), "turned off in Settings");
        fr.load_settings(&serde_json::Value::Null);
        fr.stop_recording(&mut Vec::new());
        assert!(!fr.tile_shown(), "Stop on the tile ends the recording");
        assert_eq!(fr.threads(), 0);
        assert!(fr.has_unsaved(), "and offers to save it (cancelled here)");
    }

    #[test]
    fn another_page_can_record_only_while_record_is_on() {
        assert_eq!(recording_state(true, false, None), Recording::Running);
        assert_eq!(recording_state(false, false, None), Recording::Ready);
        assert_eq!(recording_state(false, false, Some(PAUSED)), Recording::Locked(PAUSED));
        assert_eq!(recording_state(false, false, Some(SAVE_OR_DISCARD_FIRST)), Recording::Locked(SAVE_OR_DISCARD_FIRST));
        assert_eq!(recording_state(false, true, None), Recording::Locked(CLOSE_REPLAY_FIRST), "a replay is open");
    }

    #[test]
    fn another_page_starts_a_recording_with_the_sensors_and_stops_it() {
        let mut fr = started();
        let (settings, s) = (crate::settings::Settings::default(), sample(1.0));
        let mut page = PageContext::new(&settings, &s);
        assert_eq!(fr.sensor_recording(NORMAL), Recording::Ready);
        assert_eq!(fr.set_sensor_recording(true, &mut page), Ok(()));
        assert!(fr.is_recording());
        assert!(fr.recording_sensors.is_some(), "the recording holds its sensor interest");
        assert_eq!(fr.threads(), 1);
        assert_eq!(fr.sensor_recording(NORMAL), Recording::Running);
        assert_eq!(fr.set_sensor_recording(true, &mut page), Err(ALREADY_RECORDING));
        assert_eq!(fr.set_sensor_recording(false, &mut page), Ok(()));
        assert!(!fr.is_recording());
        assert_eq!(fr.threads(), 0);
        assert!(fr.has_unsaved(), "Stop offers to save it (no dialog in tests)");
        assert_eq!(fr.sensor_recording(NORMAL), Recording::Locked(SAVE_OR_DISCARD_FIRST));
    }

    #[test]
    fn its_own_record_and_stop_still_behave_as_before() {
        let mut fr = started();
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Ok(()));
        assert_eq!(fr.record(NORMAL, &sample(1.0)), Err(ALREADY_RECORDING));
        assert!(fr.recording_sensors.is_some());
        assert_eq!(fr.sensor_recording(NORMAL), Recording::Running, "another page sees its own recording");
        fr.stop_recording(&mut Vec::new());
        assert!(!fr.is_recording());
        assert!(fr.recording_sensors.is_none());
        assert_eq!(fr.threads(), 0);
        assert!(fr.has_unsaved());
        assert_eq!(fr.record_blocked(NORMAL), Some(SAVE_OR_DISCARD_FIRST));
    }

    /// A page that notes what the host told it about the sensor recording.
    struct Watcher(std::sync::Arc<std::sync::Mutex<Option<Recording>>>);

    impl crate::addons::Addon for Watcher {
        fn id(&self) -> &'static str {
            "watcher"
        }
        fn name(&self) -> &'static str {
            "Watcher"
        }
        fn description(&self) -> &'static str {
            "Watches the recording."
        }
        fn requirements(&self) -> crate::addons::Requirements {
            crate::addons::Requirements::default()
        }
        fn consumption(&self) -> crate::addons::Consumption {
            crate::addons::Consumption { cpu: "None", network: "None", disk: "None", when: "Never" }
        }
        fn ui(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
            *self.0.lock().unwrap() = Some(page.recording);
        }
    }

    /// A host with a recording running and a page that watches it.
    fn host_with_a_watching_page() -> (crate::addons::AddonHost, std::sync::Arc<std::sync::Mutex<Option<Recording>>>) {
        let mut fr = FlightRecorder::default();
        fr.start(&AddonContext::recording().0);
        fr.record(NORMAL, &sample(1.0)).unwrap();
        let told = std::sync::Arc::<std::sync::Mutex<Option<Recording>>>::default();
        let entries = vec![crate::addons::Entry::Addon(Box::new(fr)), crate::addons::Entry::Addon(Box::new(Watcher(told.clone())))];
        (crate::addons::AddonHost::for_tests_with(entries, &serde_json::json!({})).0, told)
    }

    /// What the host tells the page "watcher" when it draws it.
    fn recording_told(host: &mut crate::addons::AddonHost, told: &std::sync::Mutex<Option<Recording>>) -> Option<Recording> {
        let (settings, s) = (crate::settings::Settings::default(), sample(1.0));
        let _ = egui::Context::default().run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                host.ui("watcher", ui, &mut PageContext::new(&settings, &s));
            });
        });
        told.lock().unwrap().take()
    }

    #[test]
    fn the_host_says_off_once_it_is_turned_off() {
        let (mut host, told) = host_with_a_watching_page();
        let (settings, s) = (crate::settings::Settings::default(), sample(1.0));
        let record = |host: &mut crate::addons::AddonHost| host.record_sensors(true, &mut PageContext::new(&settings, &s));
        assert_eq!(recording_told(&mut host, &told), Some(Recording::Running), "another page is told it records");
        assert_eq!(record(&mut host), Err(ALREADY_RECORDING), "it answers while it is on");
        host.set_enabled("flight-recorder", false);
        host.answer_off(crate::addons::OffAnswer::Discard);
        assert_eq!(recording_told(&mut host, &told), Some(Recording::Off), "another page is told it is off");
        assert_eq!(record(&mut host), Err(crate::addons::RECORDING_OFF), "the host says off");
    }

    #[test]
    fn the_summary_tile_line_has_the_time_and_samples() {
        assert_eq!(tile_line(3_723_000, 12), "01:02:03  \u{2022}  12 samples");
        assert_eq!(tile_line(0, 14_400), "00:00:00  \u{2022}  14,400 samples", "the page's number format");
    }

    #[test]
    fn its_page_has_no_status_bar_controls_busiest_moments_or_clear() {
        let code = include_str!("flight_recorder.rs").split("#[cfg(test)]").next().unwrap();
        for gone in ["fn status_bar(", "Busiest moments", "\"Clear\"", "RecSample", "to_csv"] {
            assert!(!code.contains(gone), "still has {gone}");
        }
    }

    #[test]
    fn the_platform_keeps_no_recorder_code() {
        for (file, src) in [("app.rs", include_str!("../app.rs")), ("pro.rs", include_str!("../pro.rs"))] {
            for word in ["Recorder", "recorder", "RecSample", "RECORD_LIMIT"] {
                let code = src.split("#[cfg(test)]").next().unwrap();
                let hits = code.lines().filter(|l| l.contains(word)).count();
                assert_eq!(hits, 0, "{file} still mentions {word}");
            }
        }
    }
}
