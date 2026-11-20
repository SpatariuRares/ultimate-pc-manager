//! The recorder: its own thread, started by Record, that turns every
//! reading it is fed (one per app sample, page open or not) into a trace
//! sample, until Stop or the safe capture limit.

use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use super::layout::npu_series;
use super::thermals::Extras;
use super::trace::{Header, Panel, PanelSample, Sample, Trace};
use crate::platform::lock_state;
use crate::sensors::npu::NpuSample;

/// The safe capture limit, in samples (decided in #35): one hour at Fast
/// speed (4 a second), about 64 MB of trace at most.
pub const SAFE_CAPTURE_LIMIT: usize = 14_400;

/// Shown once the limit is reached.
pub const LIMIT_REACHED: &str = "Safe capture limit reached; stop to save the samples collected.";

/// What the recorder is fed: one app sample's values.
#[derive(Clone, Debug)]
pub struct Reading {
    /// When the sample was taken.
    pub at: Instant,
    /// Load of every performance core (every logical processor without
    /// hybrid cores), in percent.
    pub cores: Vec<f32>,
    /// Load of every efficiency core (none without hybrid cores).
    pub efficiency_cores: Vec<f32>,
    pub memory_used_bytes: u64,
    /// The NPUs the sensors read; None when they have not answered (the
    /// last values are kept then).
    pub npus: Option<Vec<NpuSample>>,
    /// The Thermals sensors' temperatures (°C), in the header's order;
    /// empty when the trace records none.
    pub thermals: Vec<f32>,
    /// Every GPU engine's name and utilisation (empty without a GPU).
    pub gpu_engines: Vec<(String, f32)>,
    /// The performance and efficiency cores' clocks (GHz), in the order of
    /// their loads; empty when the sample has no clock per core.
    pub core_clocks_ghz: Vec<f32>,
    pub efficiency_clocks_ghz: Vec<f32>,
    /// The CPU and GPU temperatures, the GPU clock and the hottest
    /// temperature the sensors gave (None where they read nothing).
    pub extras: Extras,
}

/// The RECORDING card's numbers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Status {
    pub samples: usize,
    /// Share of the safe capture limit used (0..=1).
    pub limit_fraction: f32,
    /// The limit was reached: nothing more is collected.
    pub limit_reached: bool,
}

/// The trace being collected, shared with the recorder's thread.
struct Capture {
    trace: Trace,
    limit: usize,
    /// When the first reading was taken (the trace's 00:00).
    origin: Option<Instant>,
    limit_reached: bool,
    /// The NPUs' last values (load, clock, temperature), for a sample the
    /// sensors did not answer.
    last_npu: Option<[Vec<f32>; 3]>,
    /// The last CPU and GPU temperatures and GPU clock read, for a sample
    /// whose sensors gave none.
    last: Extras,
}

/// A panel's series: `load`, then `clock` (0 where not read) when there
/// is a clock or a temperature, then `temp` for every cell when read.
fn with_clock_and_temp(load: Vec<f32>, clock: Vec<f32>, temp: Option<f32>) -> Vec<Vec<f32>> {
    let n = load.len();
    let clock = (!clock.is_empty() && clock.len() == n).then_some(clock);
    let mut series = vec![load];
    if clock.is_some() || temp.is_some() {
        series.push(clock.unwrap_or_else(|| vec![0.0; n]));
    }
    if let Some(t) = temp {
        series.push(vec![t; n]);
    }
    series
}

impl Capture {
    /// The Neural Engine panel of a sample when the header has one: its
    /// NPUs' load, engine clock (GHz) and temperature (°C), one value per
    /// cell; the last values (or 0) when the reading has none for them.
    fn npu_panel(&mut self, npus: Option<&[NpuSample]>) -> Option<PanelSample> {
        let cells = self.trace.header.panels.iter().find(|p| p.panel == Panel::NeuralEngine)?.cells.len();
        if cells == 0 {
            return Some(PanelSample { panel: Panel::NeuralEngine, series: vec![Vec::new()] });
        }
        if let Some(npus) = npus.filter(|n| n.len() == cells) {
            self.last_npu = Some(npu_series(npus));
        }
        let series = self.last_npu.clone().unwrap_or_else(|| [vec![0.0; cells], vec![0.0; cells], vec![0.0; cells]]);
        Some(PanelSample { panel: Panel::NeuralEngine, series: series.into() })
    }

    /// Adds `reading` as a sample, unless the limit is reached: then the
    /// trace keeps its start and nothing more is added.
    fn push(&mut self, reading: Reading) {
        if self.trace.samples.len() >= self.limit {
            self.limit_reached = true;
            return;
        }
        let origin = *self.origin.get_or_insert(reading.at);
        let t_ms = reading.at.saturating_duration_since(origin).as_millis() as u64;
        let npu = self.npu_panel(reading.npus.as_deref());
        let e = reading.extras;
        self.last = Extras {
            cpu_temp_c: e.cpu_temp_c.or(self.last.cpu_temp_c),
            gpu_temp_c: e.gpu_temp_c.or(self.last.gpu_temp_c),
            gpu_clock_ghz: e.gpu_clock_ghz.or(self.last.gpu_clock_ghz),
            hottest_c: None,
        };
        let last = self.last;
        let header = &self.trace.header.panels;
        let mut panels = vec![PanelSample { panel: Panel::PerformanceCores, series: with_clock_and_temp(reading.cores, reading.core_clocks_ghz, last.cpu_temp_c) }];
        if header.iter().any(|h| h.panel == Panel::EfficiencyCores) {
            let series = with_clock_and_temp(reading.efficiency_cores, reading.efficiency_clocks_ghz, last.cpu_temp_c);
            panels.push(PanelSample { panel: Panel::EfficiencyCores, series });
        }
        panels.extend(npu);
        if header.iter().any(|h| h.panel == Panel::Thermals) {
            panels.push(PanelSample { panel: Panel::Thermals, series: vec![reading.thermals] });
        }
        // The GPU's engines and its video engines, matched to the header's cells.
        // The GPU cores with the GPU's clock and temperature.
        for h in header.iter().filter(|h| matches!(h.panel, Panel::GpuCores | Panel::MediaEngine)) {
            let values = engine_values(&h.cells, &reading.gpu_engines);
            let series = match h.panel {
                Panel::GpuCores => with_clock_and_temp(values, last.gpu_clock_ghz.map_or_else(Vec::new, |c| vec![c; h.cells.len()]), last.gpu_temp_c),
                _ => vec![values],
            };
            panels.push(PanelSample { panel: h.panel, series });
        }
        // In the header's order, whatever order they were built in.
        panels.sort_by_key(|p| header.iter().position(|h| h.panel == p.panel));
        self.trace.samples.push(Sample { t_ms, panels, memory_used_bytes: reading.memory_used_bytes, hottest_c: e.hottest_c });
        if self.trace.samples.len() >= self.limit {
            self.limit_reached = true;
        }
    }

    fn status(&self) -> Status {
        let samples = self.trace.samples.len();
        let limit_fraction = if self.limit == 0 { 1.0 } else { (samples as f32 / self.limit as f32).min(1.0) };
        Status { samples, limit_fraction, limit_reached: self.limit_reached || self.limit == 0 }
    }
}

/// The engines' values in the order of the header's `cells`: an engine
/// missing from the reading reads as idle (the counters list an engine
/// only while a process uses it).
fn engine_values(cells: &[String], engines: &[(String, f32)]) -> Vec<f32> {
    cells.iter().map(|c| engines.iter().find(|e| &e.0 == c).map_or(0.0, |e| e.1.clamp(0.0, 100.0))).collect()
}

/// A running recording: its thread, fed through a channel.
pub struct Recorder {
    feed: Sender<Reading>,
    capture: Arc<Mutex<Capture>>,
    thread: JoinHandle<()>,
    started: Instant,
}

impl Recorder {
    /// Starts the recorder's thread, collecting at most `limit` samples
    /// under `header`.
    #[cfg(test)]
    pub fn start(header: Header, limit: usize) -> Recorder {
        Recorder::start_counted(header, limit, Arc::new(()))
    }

    /// Like [`Recorder::start`]; the thread holds a clone of `alive` until
    /// it ends, so its owner counts the threads still running.
    pub fn start_counted(header: Header, limit: usize, alive: Arc<()>) -> Recorder {
        let capture = Arc::new(Mutex::new(Capture { trace: Trace::new(header), limit, origin: None, limit_reached: false, last_npu: None, last: Extras::default() }));
        let (feed, readings) = channel::<Reading>();
        let shared = capture.clone();
        let thread = std::thread::Builder::new()
            .name("flight-recorder".into())
            .spawn(move || {
                let _alive = alive;
                // Ends when the feed is dropped (Stop).
                for reading in readings {
                    lock_state(&shared).push(reading);
                }
            })
            .expect("start the Flight Recorder thread");
        Recorder { feed, capture, thread, started: Instant::now() }
    }

    /// Hands one reading to the thread (cheap: the UI thread calls it).
    pub fn feed(&self, reading: Reading) {
        let _ = self.feed.send(reading);
    }

    /// The samples so far and the share of the limit used.
    pub fn status(&self) -> Status {
        lock_state(&self.capture).status()
    }

    /// Milliseconds since Record.
    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Stops the thread (after the readings already fed) and returns the trace.
    pub fn stop(self) -> Trace {
        let Recorder { feed, capture, thread, .. } = self;
        drop(feed);
        let _ = thread.join();
        // The thread has ended, so its handle on the capture is gone.
        match Arc::try_unwrap(capture) {
            Ok(alone) => alone.into_inner().unwrap_or_else(|e| e.into_inner()).trace,
            Err(shared) => lock_state(&shared).trace.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::flight_recorder::trace::{Header, Panel, PanelHeader};
    use std::time::{Duration, Instant};

    /// A fake source: readings of a two-core machine, `step_ms` apart, the
    /// first core's load counting 0, 1, 2, ...
    fn readings(n: usize, step_ms: u64) -> Vec<Reading> {
        let t0 = Instant::now();
        (0..n)
            .map(|i| Reading { at: t0 + Duration::from_millis(step_ms * i as u64), cores: vec![i as f32, 50.0], efficiency_cores: Vec::new(), memory_used_bytes: 1000 + i as u64, npus: None, thermals: Vec::new(), gpu_engines: Vec::new(), core_clocks_ghz: Vec::new(), efficiency_clocks_ghz: Vec::new(), extras: Extras::default() })
            .collect()
    }

    fn header() -> Header {
        Header::new("DESK-01", 0, 250, 4096, 2)
    }

    /// Waits (a few seconds at most) until the recorder holds `n` samples.
    fn wait_for(recorder: &Recorder, n: usize) -> Status {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let status = recorder.status();
            if status.samples >= n || Instant::now() > until {
                return status;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn every_reading_becomes_a_sample_from_zero() {
        let recorder = Recorder::start(header(), 100);
        for r in readings(3, 250) {
            recorder.feed(r);
        }
        let trace = recorder.stop();
        assert_eq!(trace.header, header());
        let times: Vec<u64> = trace.samples.iter().map(|s| s.t_ms).collect();
        assert_eq!(times, [0, 250, 500]);
        assert_eq!(trace.samples[2].panels[0].panel, Panel::PerformanceCores);
        assert_eq!(trace.samples[2].panels[0].series, vec![vec![2.0, 50.0]]);
        assert_eq!(trace.samples[2].memory_used_bytes, 1002);
    }

    #[test]
    fn the_status_counts_samples_and_the_share_of_the_limit_used() {
        let recorder = Recorder::start(header(), 4);
        assert_eq!(recorder.status(), Status { samples: 0, limit_fraction: 0.0, limit_reached: false });
        for r in readings(1, 250) {
            recorder.feed(r);
        }
        assert_eq!(wait_for(&recorder, 1), Status { samples: 1, limit_fraction: 0.25, limit_reached: false });
        recorder.stop();
    }

    #[test]
    fn at_the_limit_collection_stops_and_the_start_is_kept() {
        let recorder = Recorder::start(header(), 3);
        for r in readings(5, 250) {
            recorder.feed(r);
        }
        assert_eq!(wait_for(&recorder, 3), Status { samples: 3, limit_fraction: 1.0, limit_reached: true });
        let trace = recorder.stop();
        let first_core: Vec<f32> = trace.samples.iter().map(|s| s.panels[0].series[0][0]).collect();
        assert_eq!(first_core, [0.0, 1.0, 2.0], "the first samples are kept, the later ones dropped");
    }

    fn boost(load: f64) -> NpuSample {
        NpuSample { name: "Intel(R) AI Boost".into(), utilization_percent: Some(load), engine_hz: Some(1.4e9), temperature_c: Some(45.0), ..Default::default() }
    }

    #[test]
    fn an_npu_in_the_header_is_recorded_in_every_sample() {
        let mut h = header();
        h.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: vec!["Intel(R) AI Boost".into()] });
        let recorder = Recorder::start(h, 100);
        let mut rs = readings(3, 250);
        rs[0].npus = Some(vec![boost(10.0)]);
        rs[1].npus = None; // the sensors did not answer: the last values are kept
        rs[2].npus = Some(vec![boost(30.0)]);
        for r in rs {
            recorder.feed(r);
        }
        let trace = recorder.stop();
        let npu: Vec<&PanelSample> = trace.samples.iter().map(|s| &s.panels[1]).collect();
        assert!(npu.iter().all(|p| p.panel == Panel::NeuralEngine));
        assert_eq!(npu[0].series, vec![vec![10.0], vec![1.4], vec![45.0]], "load, engine clock GHz, temperature");
        assert_eq!(npu[1].series[0], vec![10.0]);
        assert_eq!(npu[2].series[0], vec![30.0]);
        assert_eq!(crate::addons::flight_recorder::replay::check(&trace), Ok(()), "a valid trace");
    }

    #[test]
    fn a_pc_without_an_npu_records_an_empty_neural_engine() {
        let mut h = header();
        h.panels.push(PanelHeader { panel: Panel::NeuralEngine, cells: vec![] });
        let recorder = Recorder::start(h, 100);
        for mut r in readings(2, 250) {
            r.npus = Some(vec![]);
            recorder.feed(r);
        }
        let trace = recorder.stop();
        assert_eq!(trace.samples[1].panels[1].series, vec![Vec::<f32>::new()]);
        assert_eq!(crate::addons::flight_recorder::replay::check(&trace), Ok(()));
    }

    #[test]
    fn gpu_and_video_engines_are_recorded_in_the_headers_cell_order() {
        let mut header = header();
        header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into(), "Copy 1".into(), "Video decode".into()] });
        header.panels.push(PanelHeader { panel: Panel::MediaEngine, cells: vec!["Video decode".into()] });
        let recorder = Recorder::start(header, 10);
        let mut reading = readings(1, 250).remove(0);
        // An engine missing from a sample reads as idle; one the header does not name is left out.
        reading.gpu_engines = vec![("Video decode".into(), 7.0), ("3D".into(), 50.0), ("New engine".into(), 9.0)];
        recorder.feed(reading);
        let trace = recorder.stop();
        let panels = &trace.samples[0].panels;
        assert_eq!(panels[1], PanelSample { panel: Panel::GpuCores, series: vec![vec![50.0, 0.0, 7.0]] });
        assert_eq!(panels[2], PanelSample { panel: Panel::MediaEngine, series: vec![vec![7.0]] });
        assert_eq!(crate::addons::flight_recorder::replay::check(&trace), Ok(()), "a valid trace");
    }

    #[test]
    fn a_hybrid_processors_efficiency_cores_are_recorded_in_their_own_panel() {
        let hybrid = crate::cpu_topology::CoreTopology::from_efficiency_classes(&[1, 1, 0]);
        let header = Header::new("LAPTOP", 0, 250, 4096, 3).split_cores(&hybrid);
        let recorder = Recorder::start(header, 10);
        recorder.feed(Reading { at: Instant::now(), cores: vec![10.0, 20.0], efficiency_cores: vec![70.0], memory_used_bytes: 1, npus: None, thermals: Vec::new(), gpu_engines: Vec::new(), core_clocks_ghz: Vec::new(), efficiency_clocks_ghz: Vec::new(), extras: Extras::default() });
        let trace = recorder.stop();
        let panels: Vec<(Panel, Vec<Vec<f32>>)> = trace.samples[0].panels.iter().map(|p| (p.panel, p.series.clone())).collect();
        assert_eq!(panels, [(Panel::PerformanceCores, vec![vec![10.0, 20.0]]), (Panel::EfficiencyCores, vec![vec![70.0]])], "in the header's order");
    }

    #[test]
    fn without_efficiency_cores_no_efficiency_panel_is_recorded() {
        let recorder = Recorder::start(header(), 10);
        recorder.feed(readings(1, 250).remove(0));
        let trace = recorder.stop();
        assert!(trace.samples[0].panels.iter().all(|p| p.panel != Panel::EfficiencyCores));
    }

    #[test]
    fn temperatures_are_recorded_in_the_headers_panel_order() {
        let mut h = header();
        h.panels.push(crate::addons::flight_recorder::trace::PanelHeader { panel: Panel::Thermals, cells: vec!["CPU: Package".into()] });
        h.sensor_keys = vec!["cpu.temp".into()];
        let recorder = Recorder::start(h.clone(), 100);
        for (i, mut r) in readings(2, 250).into_iter().enumerate() {
            r.thermals = vec![60.0 + i as f32];
            recorder.feed(r);
        }
        let trace = recorder.stop();
        assert_eq!(trace.header, h);
        let panels: Vec<Panel> = trace.samples[1].panels.iter().map(|p| p.panel).collect();
        assert_eq!(panels, [Panel::PerformanceCores, Panel::Thermals]);
        assert_eq!(trace.samples[1].panels[1].series, vec![vec![61.0]]);
        assert_eq!(crate::addons::flight_recorder::replay::check(&trace), Ok(()), "a trace Replay opens");
    }

    #[test]
    fn the_clocks_temperatures_and_hottest_are_recorded_where_the_sensors_read_them() {
        let mut h = header();
        h.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into(), "Copy".into()] });
        let recorder = Recorder::start(h, 10);
        let mut rs = readings(2, 250);
        for r in &mut rs {
            r.core_clocks_ghz = vec![4.0, 3.0];
            r.gpu_engines = vec![("3D".into(), 40.0)];
        }
        rs[0].extras = Extras { cpu_temp_c: Some(60.0), gpu_temp_c: Some(47.0), gpu_clock_ghz: Some(2.5), hottest_c: Some(66.0) };
        // The sensors did not answer: the last temperatures and clock are kept, the hottest is not made up.
        rs[1].extras = Extras::default();
        for r in rs {
            recorder.feed(r);
        }
        let trace = recorder.stop();
        let [first, second] = [&trace.samples[0], &trace.samples[1]];
        assert_eq!(first.panels[0].series, vec![vec![0.0, 50.0], vec![4.0, 3.0], vec![60.0, 60.0]], "load, clock (GHz) and the CPU temperature per core");
        assert_eq!(first.panels[1].series, vec![vec![40.0, 0.0], vec![2.5, 2.5], vec![47.0, 47.0]], "engines, GPU clock and temperature");
        assert_eq!(first.hottest_c, Some(66.0));
        assert_eq!(second.panels[0].series[2], vec![60.0, 60.0]);
        assert_eq!(second.panels[1].series[1], vec![2.5, 2.5]);
        assert_eq!(second.hottest_c, None);
        assert_eq!(crate::addons::flight_recorder::replay::check(&trace), Ok(()));
    }

    #[test]
    fn without_clocks_or_sensors_the_cores_record_their_load_alone() {
        let recorder = Recorder::start(header(), 10);
        recorder.feed(readings(1, 250).remove(0));
        let trace = recorder.stop();
        assert_eq!(trace.samples[0].panels[0].series, vec![vec![0.0, 50.0]]);
        assert_eq!(trace.samples[0].hottest_c, None);
    }

    #[test]
    fn the_limit_message_tells_to_stop_and_save() {
        assert_eq!(LIMIT_REACHED, "Safe capture limit reached; stop to save the samples collected.");
    }
}
