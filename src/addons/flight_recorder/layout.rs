//! What the Flight Recorder page shows, apart from how it is drawn: the
//! band's heading and hardware line, the mode, the panels in their order
//! and each panel's value, series and cells, live or at a replay position.

use std::collections::VecDeque;

use super::replay::Replay;
use super::thermals::{self, Extras, Sensor};
use super::trace::{core_names, samples_label, Header, Panel};
use crate::cpu_topology::{self, CoreTopology};
use crate::perfdata::{GpuInfo, GpuLive};
use crate::sampler::Snapshot;
use crate::sensors::npu::{NpuSample, NO_NPU};
use crate::sensors::Frame;
use crate::widgets::{fmt_bytes, push};

/// The band's heading while live.
pub const THIS_MACHINE: &str = "This machine";

/// What Record does, under the band (PRD story 4).
pub const EXPLANATION: &str = "Record captures every series on this page while you work elsewhere in the app. CPU cores are shown individually; GPU, Neural Engine, and Media Engine grids show the hardware engines Windows reports.";

/// The band's hardware line.
pub fn hardware_line(performance: usize, efficiency: usize, gpu_engines: usize) -> String {
    format!("{performance} performance cores  \u{2022}  {efficiency} efficiency cores  \u{2022}  {gpu_engines} GPU engines")
}

/// The "Recordings" panel's Limit row: the safe capture limit and how
/// long it lasts at one sample a second.
pub fn limit_line(limit: usize) -> String {
    let hours = limit as f64 / 3600.0;
    let unit = if hours == 1.0 { "hour" } else { "hours" };
    format!("{} ({hours} {unit} at 1 s)", samples_label(limit))
}

/// The band's heading: "This machine" live, the trace's machine in a
/// replay ("Unknown machine" when it names none).
pub fn machine_heading(replay: Option<&Replay>) -> &str {
    replay.map_or(THIS_MACHINE, Replay::machine)
}

/// The hardware line's counts in a trace: its performance and efficiency
/// cores and its GPU engines, from the header (not this PC).
pub fn header_counts(header: &Header) -> (usize, usize, usize) {
    let count = |panel| header.panels.iter().find(|p| p.panel == panel).map_or(0, |p| p.cells.len());
    (count(Panel::PerformanceCores), count(Panel::EfficiencyCores), count(Panel::GpuCores))
}

/// What the page shows: live data, a recording, or a replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Live,
    Recording,
    Replay,
}

impl Mode {
    /// The mode from the page's state; a replay wins (Record is refused
    /// during one).
    pub fn of(recording: bool, replaying: bool) -> Mode {
        match (recording, replaying) {
            (_, true) => Mode::Replay,
            (true, false) => Mode::Recording,
            (false, false) => Mode::Live,
        }
    }

    /// The word on the mode card.
    pub fn word(self) -> &'static str {
        match self {
            Mode::Live => "LIVE",
            Mode::Recording => "RECORDING",
            Mode::Replay => "REPLAY",
        }
    }

    /// Whether the control bar shows the "LIVE" indicator: whenever the
    /// page shows live data, recording or not.
    pub fn is_live(self) -> bool {
        self != Mode::Replay
    }
}

/// A panel of the page's grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    PerformanceCores,
    EfficiencyCores,
    GpuCores,
    NeuralEngine,
    MediaEngine,
    Thermals,
    Memory,
}

/// Every slot in the grid's order, with its title and the trace panel it
/// shows (None for Memory, kept apart in a trace).
const SLOTS: [(Slot, &str, Option<Panel>); 7] = [
    (Slot::PerformanceCores, "Performance cores", Some(Panel::PerformanceCores)),
    (Slot::EfficiencyCores, "Efficiency cores", Some(Panel::EfficiencyCores)),
    (Slot::GpuCores, "GPU cores", Some(Panel::GpuCores)),
    (Slot::NeuralEngine, "Neural Engine", Some(Panel::NeuralEngine)),
    (Slot::MediaEngine, "Media Engine", Some(Panel::MediaEngine)),
    (Slot::Thermals, "Thermals", Some(Panel::Thermals)),
    (Slot::Memory, "Memory", None),
];

impl Slot {
    /// Its row of [`SLOTS`].
    fn row(self) -> &'static (Slot, &'static str, Option<Panel>) {
        SLOTS.iter().find(|r| r.0 == self).expect("every slot is in the table")
    }

    /// Its title.
    pub fn title(self) -> &'static str {
        self.row().1
    }

    /// The trace panel it shows (None for Memory).
    pub fn panel(self) -> Option<Panel> {
        self.row().2
    }

    /// Whether its cells are engines, read as their busiest (as Task
    /// Manager) rather than their average.
    fn is_engines(self) -> bool {
        matches!(self, Slot::GpuCores | Slot::MediaEngine)
    }
}

/// The panels in the grid's order (two a row): Efficiency cores only when
/// there are some.
pub fn slots(efficiency_cores: usize) -> Vec<Slot> {
    SLOTS.iter().map(|r| r.0).filter(|&s| s != Slot::EfficiencyCores || efficiency_cores > 0).collect()
}

/// How a series is drawn in its panel's chart (#35): solid for the load,
/// dashed for the clock, the third for a temperature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Line {
    Solid,
    Dashed,
    Third,
}

/// One series of a panel's chart: its values, oldest first, drawn
/// against `max`.
#[derive(Clone, Debug, PartialEq)]
pub struct Series {
    pub line: Line,
    pub values: Vec<f32>,
    pub max: f32,
}

/// A panel as the page draws it: the value at the top right, up to three
/// series and one cell per core, engine or sensor (0..=100). A panel with
/// no data yet has no series and no cells.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelView {
    pub slot: Slot,
    pub value: String,
    pub series: Vec<Series>,
    pub cells: Vec<f32>,
    /// Each cell's name (a core, engine or sensor), in the cells' order.
    pub names: Vec<String>,
    /// What the panel covers, beside its title (empty without data).
    pub sub: String,
    /// What its cells and lines mean, under them (empty without data).
    pub note: String,
}

/// The cores panels' note: the effective clock and the CPU temperature
/// named only when drawn (with the clock and no temperature, the approved
/// prototype's text).
pub fn cores_note(clock: bool, temperature: bool) -> String {
    let mut note = String::from("One cell per logical core, brighter = busier.");
    if clock {
        note.push_str(" Dashed: effective clock.");
    }
    note.push_str(if temperature { " Dotted: CPU temperature." } else { " No CPU temperature sensor read here." });
    note
}

/// The Memory panel's note (the approved prototype's text).
pub const MEMORY_NOTE: &str = "In use as a share of installed memory.";

/// A cores panel's line beside its title.
fn cores_sub(cores: usize) -> String {
    format!("{cores} logical processors")
}

/// The value of a panel with no data yet.
pub const NO_VALUE: &str = "\u{2013}";

/// The GPU cores panel's note (the approved prototype's "All 14 engines
/// Windows reports. Dashed: GPU clock. Dotted: GPU temperature."): the
/// clock and temperature named only when drawn.
pub fn gpu_note(engines: usize, clock: bool, temperature: bool) -> String {
    let mut note = if engines == 1 { "The 1 engine Windows reports.".to_owned() } else { format!("All {engines} engines Windows reports.") };
    if clock {
        note.push_str(" Dashed: GPU clock.");
    }
    if temperature {
        note.push_str(" Dotted: GPU temperature.");
    }
    note
}

/// The Media Engine panel's note (the approved prototype's text).
pub const MEDIA_NOTE: &str = "The video engines, also counted among the GPU engines.";

/// The GPU the page shows and every engine Windows reports for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Engines {
    /// The adapter's name ("GPU" when DXGI does not list it).
    pub adapter: String,
    /// Each engine's name and utilisation (0..=100), in engine order.
    pub cells: Vec<(String, f32)>,
}

impl Engines {
    /// The video decode and encode engines (the Media Engine panel).
    pub fn video(&self) -> Vec<(String, f32)> {
        self.cells.iter().filter(|(n, _)| is_video_engine(n)).cloned().collect()
    }
}

/// Whether an engine decodes or encodes video ("Video decode", "Video
/// encode 1", "Video codec 0").
pub fn is_video_engine(name: &str) -> bool {
    let n = name.to_lowercase();
    n.starts_with("video") && (n.contains("decode") || n.contains("encode") || n.contains("codec"))
}

/// The GPU's engines: of the adapters with engine counters, the one with
/// the most dedicated memory (the discrete card), then the most engines.
pub fn engines_of(gpus: &[GpuLive], infos: &[GpuInfo]) -> Option<Engines> {
    let info = |g: &GpuLive| infos.iter().find(|i| i.luid == g.luid);
    let best = gpus.iter().filter(|g| !g.cells.is_empty()).max_by_key(|g| (info(g).map_or(0, |i| i.dedicated), g.cells.len()))?;
    let adapter = info(best).map_or_else(|| "GPU".to_owned(), |i| i.name.clone());
    Some(Engines { adapter, cells: best.cells.clone() })
}

/// The Media Engine panel's line beside its title: the video engines'
/// types, a repeated one counted ("Video decode, video encode ×2").
pub fn media_sub(video: &[String]) -> String {
    let mut kinds: Vec<(String, usize)> = Vec::new();
    for name in video {
        let base = name.trim_end_matches(|c: char| c.is_ascii_digit()).trim_end().to_lowercase();
        match kinds.iter_mut().find(|k| k.0 == base) {
            Some(k) => k.1 += 1,
            None => kinds.push((base, 1)),
        }
    }
    let text = kinds.iter().map(|(k, n)| if *n > 1 { format!("{k} \u{d7}{n}") } else { k.clone() }).collect::<Vec<_>>().join(", ");
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |f| f.to_uppercase().collect::<String>() + chars.as_str())
}

/// The Neural Engine panel's note with an NPU.
pub const NPU_NOTE: &str = "One cell per NPU DXCore reports. Dashed: engine clock. Dotted: NPU temperature, where reported.";

/// The Neural Engine panel's note without one (the approved prototype's text).
pub const NO_NPU_NOTE: &str = "No NPU adapter reported by DXCore: the panel stays empty, not at 0%.";

/// The Neural Engine panel's value without an NPU (the prototype's text).
pub const UNAVAILABLE: &str = "Unavailable";

/// The live last minute the charts show, fed one app sample at a time.
#[derive(Clone, Debug, Default)]
pub struct LiveHistory {
    load: VecDeque<f32>,
    clock_ghz: VecDeque<f32>,
    /// The efficiency cores' load and clock (empty without hybrid cores).
    efficiency_load: VecDeque<f32>,
    efficiency_clock_ghz: VecDeque<f32>,
    memory: VecDeque<f32>,
    /// The GPU's engines at the last sample taken while the page was open
    /// or recording (None before).
    engines: Option<Engines>,
    gpu: VecDeque<f32>,
    media: VecDeque<f32>,
    /// Which cores are efficiency cores; None reads this PC's.
    topology: Option<CoreTopology>,
    /// The NPUs last read; None until the sensors first answered.
    npus: Option<Vec<NpuSample>>,
    npu_load: VecDeque<f32>,
    npu_clock_ghz: VecDeque<f32>,
    npu_temp_c: VecDeque<f32>,
    /// The sensor frame last taken (its number), its temperature sensors
    /// (None until the sensors first answered) and what else it gave.
    frame_seq: Option<u64>,
    sensors: Option<Vec<Sensor>>,
    extras: Extras,
    cpu_temp_c: VecDeque<f32>,
    gpu_clock_ghz: VecDeque<f32>,
    gpu_temp_c: VecDeque<f32>,
    /// The hottest temperature, sample by sample.
    hottest_c: VecDeque<f32>,
}

/// A last-minute history as a chart's values, oldest first.
fn list(v: &VecDeque<f32>) -> Vec<f32> {
    v.iter().copied().collect()
}

impl LiveHistory {
    /// A history that divides the cores by `topology` instead of this PC's.
    #[cfg(test)]
    pub fn with_topology(topology: CoreTopology) -> Self {
        LiveHistory { topology: Some(topology), ..Default::default() }
    }

    /// Which cores are efficiency cores (this PC's unless set).
    pub fn topology(&self) -> &CoreTopology {
        self.topology.as_ref().unwrap_or_else(|| cpu_topology::this_pc())
    }

    /// Adds one app sample (the oldest beyond a minute is dropped).
    pub fn push(&mut self, s: &Snapshot) {
        match split_cores(self.topology(), s) {
            None => {
                push(&mut self.load, s.cpu_total);
                push(&mut self.clock_ghz, s.average_clock_ghz());
            }
            Some([performance, efficiency]) => {
                push(&mut self.load, average(&performance.load));
                push(&mut self.clock_ghz, average(&performance.ghz));
                push(&mut self.efficiency_load, average(&efficiency.load));
                push(&mut self.efficiency_clock_ghz, average(&efficiency.ghz));
            }
        }
        push(&mut self.memory, percent(s.mem_used, s.mem_total));
    }

    /// Adds the GPU's engines of one sample (None: no GPU counters).
    pub fn push_engines(&mut self, engines: Option<Engines>) {
        if let Some(e) = &engines {
            push(&mut self.gpu, busiest(e.cells.iter().map(|c| c.1)));
            push(&mut self.media, busiest(e.video().iter().map(|c| c.1)));
        }
        self.engines = engines;
    }

    /// The GPU's engines last added.
    pub fn engines(&self) -> Option<&Engines> {
        self.engines.as_ref()
    }

    /// Adds what the shared sensors give at one sample: `frame`, their
    /// latest (None while none is held or before they answered), read
    /// again only when it is a newer one; else the last values stay.
    pub fn push_sensors(&mut self, frame: Option<&Frame>) {
        if let Some(f) = frame.filter(|f| self.frame_seq != Some(f.seq)) {
            self.frame_seq = Some(f.seq);
            self.sensors = Some(thermals::sensors(f));
            self.extras = thermals::extras(f, self.engines.as_ref().map(|e| e.adapter.as_str()));
        }
        let e = self.extras;
        push(&mut self.cpu_temp_c, e.cpu_temp_c.unwrap_or(0.0));
        push(&mut self.gpu_clock_ghz, e.gpu_clock_ghz.unwrap_or(0.0));
        push(&mut self.gpu_temp_c, e.gpu_temp_c.unwrap_or(0.0));
        if let Some(h) = e.hottest_c {
            push(&mut self.hottest_c, h);
        }
    }

    /// What the sensors last gave beyond the Thermals cells.
    pub fn extras(&self) -> Extras {
        self.extras
    }

    /// The NPUs last read; None until the sensors first answered.
    pub fn npus(&self) -> Option<&[NpuSample]> {
        self.npus.as_deref()
    }

    /// Adds the NPUs the sensors read (none: this PC has no NPU).
    pub fn push_npus(&mut self, npus: Vec<NpuSample>) {
        if !npus.is_empty() {
            let [load, clock, temp] = npu_series(&npus);
            push(&mut self.npu_load, average(&load));
            push(&mut self.npu_clock_ghz, average(&clock));
            push(&mut self.npu_temp_c, average(&temp));
        }
        self.npus = Some(npus);
    }
}

/// The busiest of `values` (0 when there are none).
fn busiest(values: impl IntoIterator<Item = f32>) -> f32 {
    values.into_iter().fold(0.0, f32::max)
}

/// A panel's overall value: engines as their busiest (as Task Manager),
/// the rest as their average.
fn overall(slot: Slot, values: &[f32]) -> f32 {
    if slot.is_engines() {
        busiest(values.iter().copied())
    } else {
        average(values)
    }
}

/// A panel from its named cells and its series (the clock and
/// temperature already left out where not read), live or replayed: its
/// value, what it covers beside the title (`gpu`, the GPU's name) and its
/// note, by one rule per slot. A Neural Engine without cells says there
/// is no NPU; another panel without cells is empty.
fn panel_view(slot: Slot, cells: Vec<(String, f32)>, series: Vec<Series>, gpu: &str) -> PanelView {
    if cells.is_empty() {
        return if slot == Slot::NeuralEngine { no_npu_view() } else { empty(slot) };
    }
    let (names, values): (Vec<String>, Vec<f32>) = cells.into_iter().unzip();
    let drawn = |line| series.iter().any(|s| s.line == line);
    let (sub, note) = match slot {
        Slot::PerformanceCores | Slot::EfficiencyCores => (cores_sub(values.len()), cores_note(drawn(Line::Dashed), drawn(Line::Third))),
        Slot::GpuCores => (if gpu.is_empty() { "GPU".to_owned() } else { gpu.to_owned() }, gpu_note(values.len(), drawn(Line::Dashed), drawn(Line::Third))),
        Slot::MediaEngine => (media_sub(&names), MEDIA_NOTE.to_owned()),
        Slot::NeuralEngine => (names.join(", "), NPU_NOTE.to_owned()),
        Slot::Thermals | Slot::Memory => (String::new(), String::new()),
    };
    PanelView { slot, value: format!("{:.1}%", overall(slot, &values)), series, cells: values, names, sub, note }
}

/// The NPUs' values as a trace records them, one per NPU: load %, engine
/// clock in GHz and temperature in °C (0 where not reported).
pub fn npu_series(npus: &[NpuSample]) -> [Vec<f32>; 3] {
    let of = |f: fn(&NpuSample) -> Option<f64>| npus.iter().map(|n| f(n).unwrap_or(0.0) as f32).collect::<Vec<f32>>();
    [of(|n| n.utilization_percent.map(|v| v.clamp(0.0, 100.0))), of(|n| n.engine_hz.map(|hz| hz / 1e9)), of(|n| n.temperature_c)]
}

/// The Neural Engine panel of a machine without an NPU: empty, saying so.
fn no_npu_view() -> PanelView {
    PanelView { sub: NO_NPU.into(), value: UNAVAILABLE.into(), note: NO_NPU_NOTE.into(), ..empty(Slot::NeuralEngine) }
}

/// One class of cores in a sample: each core's name, load and clock (GHz).
struct ClassCores {
    names: Vec<String>,
    load: Vec<f32>,
    ghz: Vec<f32>,
}

/// The sample's cores divided into performance and efficiency cores; None
/// without efficiency cores.
fn split_cores(topology: &CoreTopology, s: &Snapshot) -> Option<[ClassCores; 2]> {
    let n = s.cpu_per_core.len();
    if topology.efficiency_count(n) == 0 {
        return None;
    }
    let (p_names, e_names) = topology.split(&core_names(n));
    let (p_load, e_load) = topology.split(&s.cpu_per_core);
    let (p_ghz, e_ghz) = topology.split(&core_clocks_ghz(s));
    Some([ClassCores { names: p_names, load: p_load, ghz: p_ghz }, ClassCores { names: e_names, load: e_load, ghz: e_ghz }])
}

/// Each logical processor's clock in GHz; empty unless the sample has one
/// per core.
pub fn core_clocks_ghz(s: &Snapshot) -> Vec<f32> {
    if s.cpu_freqs.len() == s.cpu_per_core.len() {
        s.cpu_freqs.iter().map(|&mhz| mhz as f32 / 1000.0).collect()
    } else {
        Vec::new()
    }
}

/// The hardware line's live counts: performance and efficiency cores.
pub fn live_counts(history: &LiveHistory, s: &Snapshot) -> (usize, usize) {
    let n = s.cpu_per_core.len();
    let efficiency = history.topology().efficiency_count(n);
    (n - efficiency, efficiency)
}

/// The panels live: the cores (divided into Performance and Efficiency
/// cores on a hybrid processor) and Memory from the app's sample and the
/// last minute, the GPU's engines, the NPUs and the temperatures (in °F
/// when `fahrenheit`) as last taken; the others empty until their data
/// comes. A clock or temperature the sensors never gave is left out.
pub fn live_panels(history: &LiveHistory, s: &Snapshot, fahrenheit: bool) -> Vec<PanelView> {
    let e = history.extras;
    let temp = |read: bool, v: &VecDeque<f32>| read.then(|| temp_series(list(v)));
    let cores = |slot, load: &VecDeque<f32>, clock: &VecDeque<f32>, cells: Vec<f32>, names: Vec<String>| {
        let series = [Some(load_series(list(load))), Some(clock_series(list(clock))), temp(e.cpu_temp_c.is_some(), &history.cpu_temp_c)];
        panel_view(slot, names.into_iter().zip(cells).collect(), series.into_iter().flatten().collect(), "")
    };
    let split = split_cores(history.topology(), s);
    let efficiency = split.as_ref().map_or(0, |[_, e]| e.load.len());
    slots(efficiency)
        .into_iter()
        .map(|slot| match (slot, &split) {
            // Every core one class: the value is the CPU's total, as the sample has it.
            (Slot::PerformanceCores, None) => PanelView {
                value: format!("{:.1}%", s.cpu_total),
                ..cores(slot, &history.load, &history.clock_ghz, s.cpu_per_core.clone(), core_names(s.cpu_per_core.len()))
            },
            (Slot::PerformanceCores, Some([p, _])) => cores(slot, &history.load, &history.clock_ghz, p.load.clone(), p.names.clone()),
            (Slot::EfficiencyCores, Some([_, ef])) => cores(slot, &history.efficiency_load, &history.efficiency_clock_ghz, ef.load.clone(), ef.names.clone()),
            (Slot::Memory, _) => memory_view(list(&history.memory), s.mem_used, s.mem_total),
            (Slot::GpuCores, _) => match &history.engines {
                Some(engines) => {
                    let clock = e.gpu_clock_ghz.is_some().then(|| clock_series(list(&history.gpu_clock_ghz)));
                    let series = [Some(load_series(list(&history.gpu))), clock, temp(e.gpu_temp_c.is_some(), &history.gpu_temp_c)];
                    panel_view(slot, engines.cells.clone(), series.into_iter().flatten().collect(), &engines.adapter)
                }
                None => empty(slot),
            },
            (Slot::MediaEngine, _) => match &history.engines {
                Some(engines) => panel_view(slot, engines.video(), vec![load_series(list(&history.media))], ""),
                None => empty(slot),
            },
            (Slot::NeuralEngine, _) => match &history.npus {
                Some(npus) => {
                    let (clock, read_temp) = (npus.iter().any(|n| n.engine_hz.is_some()), npus.iter().any(|n| n.temperature_c.is_some()));
                    let series = [Some(load_series(list(&history.npu_load))), clock.then(|| clock_series(list(&history.npu_clock_ghz))), temp(read_temp, &history.npu_temp_c)];
                    let [load, ..] = npu_series(npus);
                    panel_view(slot, npus.iter().map(|n| n.name.clone()).zip(load).collect(), series.into_iter().flatten().collect(), "")
                }
                None => empty(slot),
            },
            (Slot::Thermals, _) => match &history.sensors {
                Some(sensors) => {
                    let (names, celsius): (Vec<String>, Vec<f32>) = sensors.iter().map(|s| (s.name.clone(), s.celsius)).unzip();
                    thermals::panel(&names, &celsius, list(&history.hottest_c), fahrenheit)
                }
                None => empty(slot),
            },
            (Slot::EfficiencyCores, None) => empty(slot),
        })
        .collect()
}

/// The panels at `at_ms` of a replay, from the trace alone (its cells, not
/// this PC's): each chart runs from 00:00 to the position; temperatures in
/// °F when `fahrenheit`. A clock or temperature recorded as 0 throughout
/// was never read: it is left out, not drawn at 0 (the load always stays).
pub fn replay_panels(replay: &Replay, at_ms: u64, fahrenheit: bool) -> Vec<PanelView> {
    let trace = replay.trace();
    let now = replay.sample_at(at_ms).t_ms;
    let upto: Vec<_> = trace.samples.iter().take_while(|s| s.t_ms <= now).collect();
    let (_, efficiency, _) = header_counts(&trace.header);
    let total = trace.header.memory_total_bytes;
    let cells_now = replay.cells_at(at_ms);
    slots(efficiency)
        .into_iter()
        .map(|slot| {
            if slot == Slot::Memory {
                let history = upto.iter().map(|s| percent(s.memory_used_bytes, total)).collect();
                return memory_view(history, upto.last().map_or(0, |s| s.memory_used_bytes), total);
            }
            if slot == Slot::Thermals {
                return thermals::replay_panel(replay, at_ms, fahrenheit).unwrap_or_else(|| empty(slot));
            }
            let Some(index) = trace.header.panels.iter().position(|h| Some(h.panel) == slot.panel()) else {
                return empty(slot);
            };
            // Every series over the cells, sample by sample.
            let series_count = upto.last().map_or(0, |s| s.panels.get(index).map_or(0, |p| p.series.len()));
            let series = (0..series_count.min(3))
                .map(|k| {
                    let values: Vec<f32> = upto.iter().map(|s| s.panels.get(index).and_then(|p| p.series.get(k)).map_or(0.0, |v| overall(slot, v))).collect();
                    match k {
                        0 => load_series(values),
                        1 => clock_series(values),
                        _ => temp_series(values),
                    }
                })
                .enumerate()
                .filter(|(k, s)| *k == 0 || s.values.iter().any(|&v| v != 0.0))
                .map(|(_, s)| s)
                .collect();
            let cells = cells_now.get(index).map(|(_, c)| c.clone()).unwrap_or_default();
            panel_view(slot, cells, series, &trace.header.gpu)
        })
        .collect()
}

/// A panel with no data yet.
fn empty(slot: Slot) -> PanelView {
    PanelView { slot, value: NO_VALUE.into(), series: Vec::new(), cells: Vec::new(), names: Vec::new(), sub: String::new(), note: String::new() }
}

/// The Memory panel: used of total, its percent over time, one cell.
fn memory_view(history: Vec<f32>, used: u64, total: u64) -> PanelView {
    PanelView {
        slot: Slot::Memory,
        value: format!("{} of {}", fmt_bytes(used), fmt_bytes(total)),
        series: vec![load_series(history)],
        cells: vec![percent(used, total)],
        names: vec!["Memory in use".into()],
        sub: fmt_bytes(total),
        note: MEMORY_NOTE.into(),
    }
}

/// The solid series: a load in percent.
fn load_series(values: Vec<f32>) -> Series {
    Series { line: Line::Solid, values, max: 100.0 }
}

/// The dashed series: a clock, drawn against a fifth above its peak.
fn clock_series(values: Vec<f32>) -> Series {
    let max = values.iter().copied().fold(0.0f32, f32::max).max(0.1) * 1.2;
    Series { line: Line::Dashed, values, max }
}

/// The third series: a temperature in °C, drawn against 100.
fn temp_series(values: Vec<f32>) -> Series {
    Series { line: Line::Third, values, max: 100.0 }
}

/// The mean of `values` (0 when there are none).
fn average(values: &[f32]) -> f32 {
    values.iter().sum::<f32>() / values.len().max(1) as f32
}

/// `used` in percent of `total` (0 when the total is unknown).
fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        used as f32 * 100.0 / total as f32
    }
}

#[cfg(test)]
mod tests {
    use super::super::trace::{PanelHeader, PanelSample, Sample, Trace};
    use super::*;
    use crate::cpu_topology::CoreTopology;
    use crate::sensors::npu::NpuSample;

    #[test]
    fn the_recordings_limit_line_names_the_samples_and_the_hours_at_one_second() {
        assert_eq!(limit_line(14_400), "14,400 samples (4 hours at 1 s)");
        assert_eq!(limit_line(3_600), "3,600 samples (1 hour at 1 s)");
        assert_eq!(limit_line(900), "900 samples (0.25 hours at 1 s)");
    }

    #[test]
    fn the_hardware_line_counts_cores_and_gpu_engines() {
        assert_eq!(hardware_line(12, 0, 14), "12 performance cores  \u{2022}  0 efficiency cores  \u{2022}  14 GPU engines");
    }

    fn header(machine: &str, panels: &[(Panel, usize)]) -> Header {
        let mut h = Header::new(machine, 0, 1000, 100, 0);
        h.panels = panels.iter().map(|&(panel, n)| PanelHeader { panel, cells: (0..n).map(|i| format!("c{i}")).collect() }).collect();
        h
    }

    #[test]
    fn the_heading_is_this_machine_live_and_the_traces_machine_in_a_replay() {
        assert_eq!(machine_heading(None), "This machine");
        let named = Replay::new(Trace::new(header("RARES-PC", &[])));
        assert_eq!(machine_heading(Some(&named)), "RARES-PC");
        let nameless = Replay::new(Trace::new(header("  ", &[])));
        assert_eq!(machine_heading(Some(&nameless)), "Unknown machine");
    }

    #[test]
    fn a_traces_counts_come_from_its_header() {
        let h = header("PC", &[(Panel::PerformanceCores, 8), (Panel::EfficiencyCores, 4), (Panel::GpuCores, 14)]);
        assert_eq!(header_counts(&h), (8, 4, 14));
        assert_eq!(header_counts(&header("PC", &[(Panel::PerformanceCores, 12)])), (12, 0, 0));
    }

    #[test]
    fn the_mode_card_says_live_recording_or_replay() {
        assert_eq!(Mode::of(false, false).word(), "LIVE");
        assert_eq!(Mode::of(true, false).word(), "RECORDING");
        assert_eq!(Mode::of(false, true).word(), "REPLAY");
    }

    #[test]
    fn the_live_indicator_shows_unless_a_trace_is_replayed() {
        assert!(Mode::Live.is_live());
        assert!(Mode::Recording.is_live(), "a recording shows live data");
        assert!(!Mode::Replay.is_live());
    }

    #[test]
    fn the_grid_is_six_panels_in_order() {
        let titles: Vec<&str> = slots(0).into_iter().map(Slot::title).collect();
        assert_eq!(titles, ["Performance cores", "GPU cores", "Neural Engine", "Media Engine", "Thermals", "Memory"]);
    }

    #[test]
    fn efficiency_cores_show_only_on_a_hybrid_processor() {
        let titles: Vec<&str> = slots(4).into_iter().map(Slot::title).collect();
        assert_eq!(titles, ["Performance cores", "Efficiency cores", "GPU cores", "Neural Engine", "Media Engine", "Thermals", "Memory"]);
    }

    const GB: u64 = 1024 * 1024 * 1024;

    /// A sample of this PC: two cores, 2.0 and 4.0 GHz, 16 of 64 GB in use.
    fn snapshot(load: f32) -> Snapshot {
        Snapshot { cpu_total: load, cpu_per_core: vec![load, load / 2.0], cpu_freqs: vec![2000, 4000], mem_total: 64 * GB, mem_used: 16 * GB, ..Default::default() }
    }

    fn view(panels: &[PanelView], slot: Slot) -> &PanelView {
        panels.iter().find(|p| p.slot == slot).expect("the panel")
    }

    #[test]
    fn live_performance_cores_show_a_cell_per_logical_core_load_and_clock() {
        let mut history = LiveHistory::default();
        history.push(&snapshot(10.0));
        history.push(&snapshot(30.0));
        let panels = live_panels(&history, &snapshot(30.0), false);
        let cores = view(&panels, Slot::PerformanceCores);
        assert_eq!(cores.value, "30.0%");
        assert_eq!(cores.cells, [30.0, 15.0]);
        assert_eq!(cores.names, ["CPU 0", "CPU 1"], "named as a trace names them");
        assert_eq!(cores.series[0], Series { line: Line::Solid, values: vec![10.0, 30.0], max: 100.0 });
        assert_eq!(cores.series[1].line, Line::Dashed);
        assert_eq!(cores.series[1].values, [3.0, 3.0], "the average clock, GHz");
        assert_eq!(cores.series.len(), 2, "no temperature yet");
    }

    #[test]
    fn live_memory_shows_used_of_total() {
        let mut history = LiveHistory::default();
        history.push(&snapshot(10.0));
        let panels = live_panels(&history, &snapshot(10.0), false);
        let memory = view(&panels, Slot::Memory);
        assert_eq!(memory.value, "16.0 GB of 64.0 GB");
        assert_eq!(memory.series, [Series { line: Line::Solid, values: vec![25.0], max: 100.0 }]);
        assert_eq!(memory.cells, [25.0]);
    }

    #[test]
    fn panels_with_data_say_what_they_show_under_and_beside_the_title() {
        let panels = live_panels(&LiveHistory::default(), &snapshot(10.0), false);
        let cores = view(&panels, Slot::PerformanceCores);
        assert_eq!(cores.sub, "2 logical processors");
        assert_eq!(cores.note, "One cell per logical core, brighter = busier. Dashed: effective clock. No CPU temperature sensor read here.");
        let memory = view(&panels, Slot::Memory);
        assert_eq!(memory.sub, "64.0 GB");
        assert_eq!(memory.note, "In use as a share of installed memory.");
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!((gpu.sub.as_str(), gpu.note.as_str()), ("", ""), "an empty panel says nothing yet");
        let replay = replay_panels(&Replay::new(three_core_trace()), 0, false);
        assert_eq!(view(&replay, Slot::PerformanceCores).sub, "3 logical processors", "the trace's cores");
        assert_eq!(view(&replay, Slot::Memory).sub, "8.0 GB");
    }

    #[test]
    fn the_other_panels_are_there_but_empty_for_now() {
        let panels = live_panels(&LiveHistory::default(), &snapshot(10.0), false);
        assert_eq!(panels.iter().map(|p| p.slot).collect::<Vec<_>>(), slots(0));
        for slot in [Slot::GpuCores, Slot::NeuralEngine, Slot::MediaEngine, Slot::Thermals] {
            let p = view(&panels, slot);
            assert_eq!(p.value, "\u{2013}");
            assert!(p.series.is_empty() && p.cells.is_empty(), "{slot:?} has no data yet");
        }
    }

    /// An RTX 4080 (luid 1) with its 14 engines as Windows reports them,
    /// beside a smaller adapter (luid 2) with more engine counters.
    fn two_adapters() -> (Vec<GpuLive>, Vec<GpuInfo>) {
        let names = ["3D", "Legacy overlay", "Video decode", "Copy 1", "Copy 2", "Security", "Video encode 1", "Video encode 2", "Ofa 0", "Vr", "Copy 3", "Copy 4", "Copy 5", "Copy 6"];
        let cells = names.iter().enumerate().map(|(i, n)| (n.to_string(), if i == 0 { 45.0 } else if i == 2 { 12.0 } else { 0.0 })).collect();
        let big = GpuLive { luid: 1, cells, ..Default::default() };
        let small = GpuLive { luid: 2, cells: (0..16).map(|i| (format!("3D {i}"), 1.0)).collect(), ..Default::default() };
        let infos = vec![
            GpuInfo { luid: 2, name: "Integrated graphics".into(), dedicated: 512 << 20, ..Default::default() },
            GpuInfo { luid: 1, name: "NVIDIA GeForce RTX 4080".into(), dedicated: 16 << 30, ..Default::default() },
        ];
        (vec![small, big], infos)
    }

    #[test]
    fn the_gpu_shown_is_the_adapter_with_the_most_dedicated_memory() {
        let (live, infos) = two_adapters();
        let engines = engines_of(&live, &infos).expect("a GPU");
        assert_eq!(engines.adapter, "NVIDIA GeForce RTX 4080");
        assert_eq!(engines.cells.len(), 14, "every engine Windows reports");
        assert!(engines_of(&[], &infos).is_none(), "no GPU counters, no engines");
        let unknown = engines_of(&live[1..], &[]).expect("an adapter DXGI does not list");
        assert_eq!(unknown.adapter, "GPU");
    }

    #[test]
    fn the_video_engines_are_the_decode_and_encode_ones() {
        let (live, infos) = two_adapters();
        let video = engines_of(&live, &infos).unwrap().video();
        let names: Vec<&str> = video.iter().map(|c| c.0.as_str()).collect();
        assert_eq!(names, ["Video decode", "Video encode 1", "Video encode 2"]);
        assert_eq!(media_sub(&video.iter().map(|c| c.0.clone()).collect::<Vec<_>>()), "Video decode, video encode \u{d7}2");
    }

    #[test]
    fn live_gpu_cores_show_a_cell_per_engine_and_the_busiest_as_value() {
        let (live, infos) = two_adapters();
        let mut history = LiveHistory::default();
        history.push_engines(engines_of(&live, &infos));
        let panels = live_panels(&history, &snapshot(10.0), false);
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!(gpu.cells.len(), 14);
        assert_eq!(gpu.names[2], "Video decode");
        assert_eq!(gpu.cells[0], 45.0);
        assert_eq!(gpu.value, "45.0%", "the busiest engine, as Task Manager");
        assert_eq!(gpu.series, [Series { line: Line::Solid, values: vec![45.0], max: 100.0 }]);
        assert_eq!(gpu.sub, "NVIDIA GeForce RTX 4080");
        assert_eq!(gpu.note, "All 14 engines Windows reports.", "no clock or temperature read: not named");
        let media = view(&panels, Slot::MediaEngine);
        assert_eq!(media.names, ["Video decode", "Video encode 1", "Video encode 2"]);
        assert_eq!(media.cells, [12.0, 0.0, 0.0]);
        assert_eq!(media.value, "12.0%");
        assert_eq!(media.series[0].values, [12.0]);
        assert_eq!(media.sub, "Video decode, video encode \u{d7}2");
        assert_eq!(media.note, "The video engines, also counted among the GPU engines.");
    }

    /// A sensor frame: the CPU at `cpu` °C, the RTX 4080 at 2.5 GHz and 47 °C.
    fn sensor_frame(seq: u64, cpu: Option<f64>) -> crate::sensors::Frame {
        use crate::sensors::model::{Device, Group, Reading, Unit};
        let at = std::time::Instant::now();
        let (c, g) = (Device::cpu("Ryzen 7"), Device::gpu("rtx", "NVIDIA GeForce RTX 4080"));
        let readings = vec![
            Reading::measure("cpu.package.temperature_c", c, Group::Temperatures, "Package", Unit::Celsius, "test", at).with(cpu, "not read"),
            Reading::measure("gpu.rtx.temperature.gpu_c", g.clone(), Group::Temperatures, "GPU Temperature", Unit::Celsius, "test", at).with(Some(47.0), ""),
            Reading::measure("gpu.rtx.clock.graphics_hz", g, Group::Clocks, "Graphics Clock", Unit::Hz, "test", at).with(Some(2.5e9), ""),
        ];
        crate::sensors::Frame::build(seq, &[readings], Vec::new())
    }

    #[test]
    fn live_gpu_cores_draw_the_gpu_clock_and_temperature_where_the_sensors_give_them() {
        let (live, infos) = two_adapters();
        let mut history = LiveHistory::default();
        history.push_engines(engines_of(&live, &infos));
        history.push_sensors(Some(&sensor_frame(1, Some(60.0))));
        history.push_sensors(Some(&sensor_frame(1, Some(60.0))));
        let panels = live_panels(&history, &snapshot(10.0), false);
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!(gpu.series.iter().map(|s| s.line).collect::<Vec<_>>(), [Line::Solid, Line::Dashed, Line::Third]);
        assert_eq!(gpu.series[1].values, [2.5, 2.5], "the GPU clock, GHz");
        assert_eq!(gpu.series[2].values, [47.0, 47.0], "the GPU temperature");
        assert_eq!(gpu.note, "All 14 engines Windows reports. Dashed: GPU clock. Dotted: GPU temperature.", "the prototype's note");
        let cores = view(&panels, Slot::PerformanceCores);
        assert_eq!(cores.series[2], Series { line: Line::Third, values: vec![60.0, 60.0], max: 100.0 }, "the CPU temperature, dotted");
        assert_eq!(cores.note, "One cell per logical core, brighter = busier. Dashed: effective clock. Dotted: CPU temperature.");
    }

    #[test]
    fn without_a_cpu_temperature_the_cores_say_so_and_draw_none() {
        let mut history = LiveHistory::default();
        history.push(&snapshot(10.0));
        history.push_sensors(Some(&sensor_frame(1, None)));
        let cores = view(&live_panels(&history, &snapshot(10.0), false), Slot::PerformanceCores).clone();
        assert!(cores.series.iter().all(|s| s.line != Line::Third));
        assert!(cores.note.ends_with("No CPU temperature sensor read here."));
    }

    #[test]
    fn a_replay_draws_the_recorded_clock_and_temperature_and_leaves_out_one_never_read() {
        let mut trace = three_core_trace();
        trace.header.gpu = "NVIDIA GeForce RTX 4080".into();
        trace.header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into()] });
        for s in &mut trace.samples {
            // Load, then a clock never read (0), then the temperature.
            s.panels[0].series = vec![s.panels[0].series[0].clone(), vec![0.0; 3], vec![55.0; 3]];
            s.panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![30.0], vec![2.5], vec![47.0]] });
        }
        let panels = replay_panels(&Replay::new(trace), 2000, false);
        let cores = view(&panels, Slot::PerformanceCores);
        assert_eq!(cores.series.iter().map(|s| s.line).collect::<Vec<_>>(), [Line::Solid, Line::Third], "the temperature stays dotted");
        assert_eq!(cores.note, "One cell per logical core, brighter = busier. Dotted: CPU temperature.");
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!(gpu.series[1].values, [2.5, 2.5, 2.5]);
        assert_eq!(gpu.note, "The 1 engine Windows reports. Dashed: GPU clock. Dotted: GPU temperature.");
    }

    #[test]
    fn a_replay_shows_the_traces_temperatures_in_its_thermals_panel() {
        let mut trace = three_core_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::Thermals, cells: vec!["CPU".into()] });
        for s in &mut trace.samples {
            s.panels.push(PanelSample { panel: Panel::Thermals, series: vec![vec![61.0]] });
        }
        let panels = replay_panels(&Replay::new(trace), 0, true);
        assert_eq!(view(&panels, Slot::Thermals).value, "141.8 \u{00b0}F");
        assert_eq!(view(&replay_panels(&Replay::new(three_core_trace()), 0, false), Slot::Thermals).value, "\u{2013}", "none recorded");
    }

    #[test]
    fn a_gpu_without_video_engines_leaves_media_engine_empty() {
        let mut history = LiveHistory::default();
        history.push_engines(Some(Engines { adapter: "GPU".into(), cells: vec![("3D".into(), 5.0)] }));
        let panels = live_panels(&history, &snapshot(10.0), false);
        assert_eq!(view(&panels, Slot::GpuCores).cells, [5.0]);
        assert_eq!(view(&panels, Slot::MediaEngine).value, "\u{2013}");
    }

    fn npu(name: &str, load: f64, ghz: Option<f64>, temp: Option<f64>) -> NpuSample {
        NpuSample { name: name.into(), utilization_percent: Some(load), engine_hz: ghz.map(|g| g * 1e9), temperature_c: temp, ..Default::default() }
    }

    #[test]
    fn the_neural_engine_says_no_npu_in_this_pc_instead_of_an_empty_panel() {
        let mut history = LiveHistory::default();
        assert_eq!(view(&live_panels(&history, &snapshot(1.0), false), Slot::NeuralEngine).value, "\u{2013}", "the sensors have not answered yet");
        history.push_npus(Vec::new());
        let panels = live_panels(&history, &snapshot(1.0), false);
        let neural = view(&panels, Slot::NeuralEngine);
        assert_eq!(neural.sub, "No NPU in this PC");
        assert_eq!(neural.value, "Unavailable");
        assert_eq!(neural.note, "No NPU adapter reported by DXCore: the panel stays empty, not at 0%.");
        assert!(neural.series.is_empty() && neural.cells.is_empty(), "empty, not at 0%");
    }

    #[test]
    fn the_neural_engine_shows_each_npus_activity_clock_and_temperature() {
        let mut history = LiveHistory::default();
        history.push_npus(vec![npu("Intel(R) AI Boost", 20.0, Some(1.4), Some(48.0))]);
        history.push_npus(vec![npu("Intel(R) AI Boost", 60.0, Some(1.4), Some(50.0))]);
        let panels = live_panels(&history, &snapshot(1.0), false);
        let neural = view(&panels, Slot::NeuralEngine);
        assert_eq!(neural.sub, "Intel(R) AI Boost");
        assert_eq!(neural.value, "60.0%");
        assert_eq!(neural.cells, [60.0]);
        assert_eq!(neural.names, ["Intel(R) AI Boost"]);
        assert_eq!(neural.note, NPU_NOTE);
        assert_eq!(neural.series[0], Series { line: Line::Solid, values: vec![20.0, 60.0], max: 100.0 });
        assert_eq!((neural.series[1].line, neural.series[1].values.clone()), (Line::Dashed, vec![1.4, 1.4]), "the engine clock, GHz");
        assert_eq!(neural.series[2], Series { line: Line::Third, values: vec![48.0, 50.0], max: 100.0 });
    }

    #[test]
    fn an_npu_without_clock_or_temperature_draws_only_its_load() {
        let mut history = LiveHistory::default();
        history.push_npus(vec![npu("NPU", 10.0, None, None)]);
        let neural = view(&live_panels(&history, &snapshot(1.0), false), Slot::NeuralEngine).clone();
        assert_eq!(neural.series.len(), 1);
    }

    #[test]
    fn a_replay_shows_the_traces_npu_even_on_a_pc_without_one() {
        let mut trace = three_core_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: vec!["Intel(R) AI Boost".into()] });
        for (i, s) in trace.samples.iter_mut().enumerate() {
            s.panels.push(PanelSample { panel: Panel::NeuralEngine, series: vec![vec![10.0 * i as f32], vec![1.4], vec![45.0]] });
        }
        let panels = replay_panels(&Replay::new(trace), 2000, false);
        let neural = view(&panels, Slot::NeuralEngine);
        assert_eq!(neural.cells, [20.0]);
        assert_eq!(neural.names, ["Intel(R) AI Boost"]);
        assert_eq!((neural.sub.as_str(), neural.value.as_str(), neural.note.as_str()), ("Intel(R) AI Boost", "20.0%", NPU_NOTE));
        assert_eq!(neural.series[0].values, [0.0, 10.0, 20.0]);
        assert_eq!(neural.series[2].values, [45.0, 45.0, 45.0]);
    }

    #[test]
    fn a_replayed_npu_without_clock_or_temperature_draws_only_its_load() {
        let mut trace = three_core_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: vec!["NPU".into()] });
        for s in &mut trace.samples {
            s.panels.push(PanelSample { panel: Panel::NeuralEngine, series: vec![vec![5.0], vec![0.0], vec![0.0]] });
        }
        let panels = replay_panels(&Replay::new(trace), 2000, false);
        assert_eq!(view(&panels, Slot::NeuralEngine).series.len(), 1, "never reported: absent, not a line at 0");
    }

    #[test]
    fn a_replay_of_a_pc_without_an_npu_says_so() {
        let mut trace = three_core_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: vec![] });
        for s in &mut trace.samples {
            s.panels.push(PanelSample { panel: Panel::NeuralEngine, series: vec![vec![]] });
        }
        let panels = replay_panels(&Replay::new(trace), 0, false);
        let neural = view(&panels, Slot::NeuralEngine);
        assert_eq!((neural.sub.as_str(), neural.value.as_str()), ("No NPU in this PC", "Unavailable"));
        assert!(neural.series.is_empty());
    }

    /// A hybrid sample: CPU 0 and 1 performance cores at 40 and 20 %, 4.0
    /// GHz; CPU 2 an efficiency core at 90 %, 2.0 GHz.
    fn hybrid_sample() -> Snapshot {
        Snapshot { cpu_total: 50.0, cpu_per_core: vec![40.0, 20.0, 90.0], cpu_freqs: vec![4000, 4000, 2000], mem_total: 64 * GB, mem_used: 16 * GB, ..Default::default() }
    }

    fn hybrid_history() -> LiveHistory {
        LiveHistory::with_topology(CoreTopology::from_efficiency_classes(&[1, 1, 0]))
    }

    #[test]
    fn live_cores_of_a_hybrid_processor_split_into_performance_and_efficiency_panels() {
        let mut history = hybrid_history();
        history.push(&hybrid_sample());
        let panels = live_panels(&history, &hybrid_sample(), false);
        assert_eq!(panels.iter().map(|p| p.slot).collect::<Vec<_>>(), slots(1));
        let performance = view(&panels, Slot::PerformanceCores);
        assert_eq!((performance.cells.as_slice(), performance.names.as_slice()), ([40.0, 20.0].as_slice(), ["CPU 0", "CPU 1"].map(String::from).as_slice()));
        assert_eq!(performance.value, "30.0%");
        assert_eq!(performance.series[0].values, [30.0]);
        assert_eq!(performance.series[1].values, [4.0], "the performance cores' clock, GHz");
        assert_eq!(performance.sub, "2 logical processors");
        let efficiency = view(&panels, Slot::EfficiencyCores);
        assert_eq!((efficiency.cells.as_slice(), efficiency.names.as_slice()), ([90.0].as_slice(), ["CPU 2".to_string()].as_slice()), "named by its logical index");
        assert_eq!(efficiency.value, "90.0%");
        assert_eq!(efficiency.series[0].values, [90.0]);
        assert_eq!(efficiency.series[1].values, [2.0]);
        assert_eq!(efficiency.sub, "1 logical processors");
        assert_eq!(efficiency.note, "One cell per logical core, brighter = busier. Dashed: effective clock. No CPU temperature sensor read here.");
        assert_eq!(live_counts(&history, &hybrid_sample()), (2, 1));
    }

    #[test]
    fn with_zero_efficiency_cores_there_is_no_efficiency_panel() {
        let history = LiveHistory::with_topology(CoreTopology::from_efficiency_classes(&[0, 0]));
        let panels = live_panels(&history, &snapshot(10.0), false);
        assert!(panels.iter().all(|p| p.slot != Slot::EfficiencyCores));
        assert_eq!(view(&panels, Slot::PerformanceCores).cells, [10.0, 5.0]);
        assert_eq!(live_counts(&history, &snapshot(10.0)), (2, 0));
    }

    #[test]
    fn a_hybrid_trace_shows_its_efficiency_cores_in_a_replay() {
        let hybrid = CoreTopology::from_efficiency_classes(&[1, 1, 0]);
        let mut trace = Trace::new(Header::new("LAPTOP", 0, 1000, 8 * GB, 3).split_cores(&hybrid));
        trace.samples.push(Sample {
            t_ms: 0,
            panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![40.0, 20.0]] }, PanelSample { panel: Panel::EfficiencyCores, series: vec![vec![90.0]] }],
            memory_used_bytes: GB,
            hottest_c: None,
        });
        let replay = Replay::new(trace);
        assert_eq!(header_counts(&replay.trace().header), (2, 1, 0));
        let panels = replay_panels(&replay, 0, false);
        let efficiency = view(&panels, Slot::EfficiencyCores);
        assert_eq!(efficiency.cells, [90.0]);
        assert_eq!(efficiency.names, ["CPU 2"]);
        assert_eq!(efficiency.sub, "1 logical processors");
        assert_eq!(view(&panels, Slot::PerformanceCores).cells, [40.0, 20.0]);
    }

    /// A trace of another machine: three cores and 8 GB; samples at 0,
    /// 1000 and 2000 ms with the cores at n, n/2 and 0, and 1, 2, 3 GB in use.
    fn three_core_trace() -> Trace {
        let mut trace = Trace::new(header("OTHER-PC", &[(Panel::PerformanceCores, 3)]));
        trace.header.memory_total_bytes = 8 * GB;
        for (i, n) in [20.0f32, 40.0, 60.0].into_iter().enumerate() {
            trace.samples.push(Sample {
                t_ms: i as u64 * 1000,
                panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![n, n / 2.0, 0.0]] }],
                memory_used_bytes: (i as u64 + 1) * GB,
                hottest_c: None,
            });
        }
        trace
    }

    #[test]
    fn a_replay_shows_the_traces_cells_and_its_charts_up_to_the_position() {
        let replay = Replay::new(three_core_trace());
        let panels = replay_panels(&replay, 1000, false);
        let cores = view(&panels, Slot::PerformanceCores);
        assert_eq!(cores.cells, [40.0, 20.0, 0.0], "the trace's three cores, not this PC's");
        assert_eq!(cores.names, ["c0", "c1", "c2"]);
        assert_eq!(cores.value, "20.0%");
        assert_eq!(cores.series, [Series { line: Line::Solid, values: vec![10.0, 20.0], max: 100.0 }], "only the load was recorded");
        let memory = view(&panels, Slot::Memory);
        assert_eq!(memory.value, "2.0 GB of 8.0 GB");
        assert_eq!(memory.series[0].values, [12.5, 25.0]);
        assert_eq!(memory.cells, [25.0]);
        assert_eq!(view(&panels, Slot::GpuCores).value, "\u{2013}", "not in the trace");
    }

    #[test]
    fn a_replay_shows_other_panels_its_trace_holds() {
        let mut trace = three_core_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into(), "Copy".into()] });
        for s in &mut trace.samples {
            s.panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![50.0, 10.0], vec![1.0, 1.0]] });
        }
        let panels = replay_panels(&Replay::new(trace), 2000, false);
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!(gpu.cells, [50.0, 10.0]);
        assert_eq!(gpu.names, ["3D", "Copy"]);
        assert_eq!(gpu.series.len(), 2);
        assert_eq!(gpu.series[1].line, Line::Dashed);
    }

    #[test]
    fn a_replay_shows_the_traces_gpu_engines_and_video_engines() {
        let mut trace = three_core_trace();
        trace.header.gpu = "NVIDIA GeForce RTX 4080".into();
        trace.header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into(), "Video decode".into()] });
        trace.header.panels.push(PanelHeader { panel: Panel::MediaEngine, cells: vec!["Video decode".into()] });
        for (i, s) in trace.samples.iter_mut().enumerate() {
            let decode = 10.0 * i as f32;
            s.panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![60.0, decode]] });
            s.panels.push(PanelSample { panel: Panel::MediaEngine, series: vec![vec![decode]] });
        }
        let panels = replay_panels(&Replay::new(trace), 1000, false);
        let gpu = view(&panels, Slot::GpuCores);
        assert_eq!(gpu.cells, [60.0, 10.0]);
        assert_eq!(gpu.value, "60.0%", "the busiest engine");
        assert_eq!(gpu.series[0].values, [60.0, 60.0], "the busiest engine over time");
        assert_eq!((gpu.sub.as_str(), gpu.note.as_str()), ("NVIDIA GeForce RTX 4080", "All 2 engines Windows reports."));
        let media = view(&panels, Slot::MediaEngine);
        assert_eq!(media.names, ["Video decode"]);
        assert_eq!(media.value, "10.0%");
        assert_eq!((media.sub.as_str(), media.note.as_str()), ("Video decode", MEDIA_NOTE));
        let (_, _, engines) = header_counts(&Replay::new(three_core_trace()).trace().header);
        assert_eq!(engines, 0);
    }
}
