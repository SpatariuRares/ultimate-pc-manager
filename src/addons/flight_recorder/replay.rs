//! Replay (pure): reading a trace file back, checking it before anything
//! of it is shown, and finding the sample at a replay position.

use super::trace::{Panel, Sample, Trace};

/// A trace recorded on another platform.
pub const OTHER_PLATFORM: &str = "That trace was recorded on a different platform.";

/// A trace written by a newer version of the format.
pub const NEWER_VERSION: &str = "That trace was saved by a newer version of the app.";

/// A file that is not valid trace JSON (the parser's reason follows).
pub const NOT_A_TRACE: &str = "That file is not a valid Flight Recorder trace.";

/// More samples than any recording can hold
pub const TOO_MANY_SAMPLES: &str = "That trace contains too many samples.";

/// A trace with no samples.
pub const NO_SAMPLES: &str = "That trace contains no samples.";

/// A first sample that is not at 00:00
pub const NOT_AT_ZERO: &str = "The trace must begin at timestamp zero.";

/// A timestamp earlier than the one before it.
pub const OUT_OF_ORDER: &str = "The trace's timestamps are out of order.";

/// A value that is not a number or is out of its bounds.
pub const INVALID_VALUE: &str = "The trace contains an invalid value.";

/// Samples whose panels or cells differ from the header's.
pub const CELLS_MISMATCH: &str = "The trace's samples do not match its header.";

/// The most samples a trace may hold: the safe capture limit.
pub const MAX_SAMPLES: usize = super::recorder::SAFE_CAPTURE_LIMIT;

/// The largest file read at all (four times the ~64 MB a full trace
/// takes); a bigger one is refused with [`FILE_TOO_LARGE`].
pub const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

/// A file over [`MAX_FILE_BYTES`]: refused before it is read.
pub const FILE_TOO_LARGE: &str = "That trace file is too large to open.";

/// The open dialog's title
pub const PICKER_TITLE: &str = "Choose a Flight Recorder trace to replay";

/// The open dialog failed to show
pub const PICKER_FAILED: &str = "The Replay file picker could not be opened.";

/// Record pressed during a replay
pub const CLOSE_REPLAY_FIRST: &str = "Close the replay before starting a recording.";

/// Shown on the REPLAY card
pub const NOTHING_LIVE: &str = "Replaying a recorded run. Nothing on this page is live until the replay is closed.";

/// The machine of a trace that names none
pub const UNKNOWN_MACHINE: &str = "Unknown machine";

/// The open dialog's Win32 filter: trace files only.
pub fn picker_filter() -> String {
    let ext = super::trace::TRACE_EXTENSION;
    format!("Flight Recorder trace (*.{ext})\0*.{ext}\0\0")
}

/// Reads and checks the trace file at `path`.
pub fn open_trace_file(path: &std::path::Path) -> Result<Trace, String> {
    open_trace_file_within(path, MAX_FILE_BYTES)
}

/// Reads and checks the trace file at `path`, refusing one over `max_bytes`.
pub fn open_trace_file_within(path: &std::path::Path, max_bytes: u64) -> Result<Trace, String> {
    let size = std::fs::metadata(path).map_err(|e| format!("{NOT_A_TRACE} {e}"))?.len();
    if size > max_bytes {
        return Err(FILE_TOO_LARGE.into());
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("{NOT_A_TRACE} {e}"))?;
    open_trace(&text)
}

/// Reads a trace from JSON text and checks it; the first broken rule is
/// the error. The platform and version are checked before the rest, so a
/// newer trace is named as such even when this build cannot read it.
pub fn open_trace(text: &str) -> Result<Trace, String> {
    let json: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("{NOT_A_TRACE} {e}"))?;
    let header = &json["header"];
    if header["platform"].as_str().is_some_and(|p| p != super::trace::PLATFORM) {
        return Err(OTHER_PLATFORM.into());
    }
    if header["formatVersion"].as_u64().is_some_and(|v| v > u64::from(super::trace::FORMAT_VERSION)) {
        return Err(NEWER_VERSION.into());
    }
    let trace: Trace = serde_json::from_value(json).map_err(|e| format!("{NOT_A_TRACE} {e}"))?;
    check(&trace)?;
    Ok(trace)
}

/// Whether `v` fits a panel's primary series: a percentage, except the
/// Thermals panel's temperatures (degrees Celsius).
fn primary_fits(panel: Panel, v: f32) -> bool {
    match panel {
        Panel::Thermals => super::thermals::BOUNDS.contains(&v),
        _ => (0.0..=100.0).contains(&v),
    }
}

/// Checks a parsed trace: sample count, start at zero, order, values and
/// cells against the header.
pub fn check(trace: &Trace) -> Result<(), String> {
    let samples = &trace.samples;
    if samples.len() > MAX_SAMPLES {
        return Err(TOO_MANY_SAMPLES.into());
    }
    let Some(first) = samples.first() else {
        return Err(NO_SAMPLES.into());
    };
    if first.t_ms != 0 {
        return Err(NOT_AT_ZERO.into());
    }
    if samples.windows(2).any(|w| w[1].t_ms < w[0].t_ms) {
        return Err(OUT_OF_ORDER.into());
    }
    let header = &trace.header;
    let thermals = header.panels.iter().find(|p| p.panel == Panel::Thermals).map_or(0, |p| p.cells.len());
    if !header.sensor_keys.is_empty() && header.sensor_keys.len() != thermals {
        return Err(CELLS_MISMATCH.into());
    }
    for s in samples {
        if s.memory_used_bytes > header.memory_total_bytes || s.hottest_c.is_some_and(|c| !super::thermals::BOUNDS.contains(&c)) {
            return Err(INVALID_VALUE.into());
        }
        if s.panels.len() != header.panels.len() {
            return Err(CELLS_MISMATCH.into());
        }
        for (p, h) in s.panels.iter().zip(&header.panels) {
            if p.panel != h.panel || p.series.is_empty() || p.series.len() > 3 || p.series.iter().any(|v| v.len() != h.cells.len()) {
                return Err(CELLS_MISMATCH.into());
            }
            let finite = p.series.iter().flatten().all(|v| v.is_finite());
            if !finite || !p.series[0].iter().all(|&v| primary_fits(p.panel, v)) {
                return Err(INVALID_VALUE.into());
            }
        }
    }
    Ok(())
}

/// An open, checked trace being replayed.
pub struct Replay {
    trace: Trace,
}

impl Replay {
    /// Replays `trace`, which [`check`] accepted (it has a sample).
    pub fn new(trace: Trace) -> Self {
        Replay { trace }
    }

    /// The trace being replayed.
    pub fn trace(&self) -> &Trace {
        &self.trace
    }

    /// The time of the last sample: the replay's length.
    pub fn duration_ms(&self) -> u64 {
        self.trace.samples.last().map_or(0, |s| s.t_ms)
    }

    /// The sample nearest to `ms` (the earlier one on a tie).
    pub fn sample_at(&self, ms: u64) -> &Sample {
        let s = &self.trace.samples;
        let after = s.partition_point(|x| x.t_ms < ms);
        if after == 0 {
            return &s[0];
        }
        if after == s.len() {
            return &s[s.len() - 1];
        }
        let (before, next) = (&s[after - 1], &s[after]);
        if ms - before.t_ms <= next.t_ms - ms {
            before
        } else {
            next
        }
    }

    /// Every panel's cells at `ms`: the names from the trace's header
    /// (not this PC) with the primary values of the nearest sample.
    pub fn cells_at(&self, ms: u64) -> Vec<(Panel, Vec<(String, f32)>)> {
        let sample = self.sample_at(ms);
        self.trace
            .header
            .panels
            .iter()
            .zip(&sample.panels)
            .map(|(h, p)| (h.panel, h.cells.iter().cloned().zip(p.series[0].iter().copied()).collect()))
            .collect()
    }

    /// The trace's machine, or "Unknown machine" when it names none.
    pub fn machine(&self) -> &str {
        let m = self.trace.header.machine.trim();
        if m.is_empty() {
            UNKNOWN_MACHINE
        } else {
            m
        }
    }

    /// "Recorded %1", with the recording's date and time.
    pub fn recorded_label(&self) -> String {
        format!("Recorded {}", self.trace.header.recorded.replacen('T', " ", 1))
    }
}

#[cfg(test)]
mod tests {
    use super::super::trace::{Header, PanelHeader, PanelSample, FORMAT_VERSION};
    use super::*;

    /// A two-core trace: three samples half a second apart.
    fn good() -> Trace {
        let mut trace = Trace::new(Header::new("DESK-01", 1_794_776_670, 500, 1000, 2));
        for (t_ms, load) in [(0, 10.0), (500, 50.0), (1000, 90.0)] {
            trace.samples.push(Sample { t_ms, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![load, load / 2.0]] }], memory_used_bytes: 400, hottest_c: None });
        }
        trace
    }

    fn rejected(trace: &Trace) -> String {
        open_trace(&trace.to_json()).unwrap_err()
    }

    #[test]
    fn a_good_trace_opens_as_it_was_saved() {
        assert_eq!(open_trace(&good().to_json()), Ok(good()));
    }

    #[test]
    fn a_trace_from_another_platform_is_refused() {
        let mut t = good();
        t.header.platform = "macos".into();
        assert_eq!(rejected(&t), OTHER_PLATFORM);
    }

    #[test]
    fn a_trace_from_a_newer_version_is_refused() {
        let mut json: serde_json::Value = serde_json::from_str(&good().to_json()).unwrap();
        json["header"]["formatVersion"] = (FORMAT_VERSION + 1).into();
        json["header"]["somethingNew"] = "unknown to this build".into();
        json["samples"][0]["tMs"] = "a shape this build cannot read".into();
        assert_eq!(open_trace(&json.to_string()).unwrap_err(), NEWER_VERSION);
    }

    #[test]
    fn a_file_that_is_not_json_is_refused() {
        assert!(open_trace("{\"header\": [").unwrap_err().starts_with(NOT_A_TRACE));
        assert!(open_trace("hello").unwrap_err().starts_with(NOT_A_TRACE));
    }

    #[test]
    fn a_trace_with_too_many_samples_is_refused() {
        let mut t = good();
        let last = t.samples[2].clone();
        t.samples = (0..=MAX_SAMPLES as u64).map(|i| Sample { t_ms: i * 250, ..last.clone() }).collect();
        assert_eq!(rejected(&t), "That trace contains too many samples.");
    }

    #[test]
    fn a_file_over_the_size_cap_is_refused_as_too_large_not_as_too_many_samples() {
        let path = std::env::temp_dir().join(format!("fr-big-{}.upmtrace", std::process::id()));
        std::fs::write(&path, good().to_json()).unwrap();
        let small_cap = open_trace_file_within(&path, 10);
        let roomy = open_trace_file_within(&path, MAX_FILE_BYTES);
        let _ = std::fs::remove_file(&path);
        assert_eq!(small_cap, Err("That trace file is too large to open.".to_string()));
        assert_eq!(roomy, Ok(good()));
        assert_ne!(FILE_TOO_LARGE, TOO_MANY_SAMPLES);
    }

    #[test]
    fn the_size_cap_is_four_times_a_full_trace_and_the_disk_note_says_a_full_one_is_about_64_mb() {
        assert_eq!(MAX_FILE_BYTES, 4 * 64 * 1024 * 1024);
        assert!(super::super::DISK_NOTE.contains("about 64 MB"));
    }

    #[test]
    fn a_trace_without_samples_is_refused() {
        let mut t = good();
        t.samples.clear();
        assert_eq!(rejected(&t), NO_SAMPLES);
    }

    #[test]
    fn a_trace_must_begin_at_zero() {
        let mut t = good();
        t.samples.remove(0);
        assert_eq!(rejected(&t), "The trace must begin at timestamp zero.");
    }

    #[test]
    fn timestamps_out_of_order_are_refused() {
        let mut t = good();
        t.samples[2].t_ms = 400;
        assert_eq!(rejected(&t), OUT_OF_ORDER);
        let mut t = good();
        t.samples[2].t_ms = 500;
        assert_eq!(open_trace(&t.to_json()), Ok(t), "two samples in the same millisecond are in order");
    }

    #[test]
    fn a_nan_value_is_refused() {
        let mut t = good();
        t.samples[1].panels[0].series[0][1] = f32::NAN;
        assert_eq!(check(&t), Err(INVALID_VALUE.to_string()));
        // Written as JSON, a NaN becomes null: that is no trace either.
        assert!(rejected(&t).starts_with(NOT_A_TRACE));
    }

    #[test]
    fn values_out_of_bounds_are_refused() {
        let mut t = good();
        t.samples[1].panels[0].series[0][0] = 150.0;
        assert_eq!(rejected(&t), INVALID_VALUE, "a load over 100%");
        let mut t = good();
        t.samples[1].panels[0].series[0][0] = -1.0;
        assert_eq!(rejected(&t), INVALID_VALUE, "a negative load");
        let mut t = good();
        t.samples[1].memory_used_bytes = 1001;
        assert_eq!(rejected(&t), INVALID_VALUE, "more memory in use than the total");
        let mut t = good();
        t.header.panels.push(PanelHeader { panel: Panel::Thermals, cells: vec!["CPU".into()] });
        for s in &mut t.samples {
            s.panels.push(PanelSample { panel: Panel::Thermals, series: vec![vec![105.0]] });
        }
        assert_eq!(open_trace(&t.to_json()), Ok(t), "a temperature is not a percentage");
        let mut t = good();
        t.samples[1].hottest_c = Some(400.0);
        assert_eq!(rejected(&t), INVALID_VALUE, "a hottest temperature out of bounds");
        t.samples[1].hottest_c = Some(88.0);
        assert_eq!(open_trace(&t.to_json()), Ok(t), "one in bounds");
    }

    #[test]
    fn cells_that_do_not_match_the_header_are_refused() {
        let mut t = good();
        t.samples[1].panels[0].series[0].push(5.0);
        assert_eq!(rejected(&t), CELLS_MISMATCH, "a cell more than the header names");
        let mut t = good();
        t.samples[1].panels[0].series[0].pop();
        assert_eq!(rejected(&t), CELLS_MISMATCH, "a cell less");
        let mut t = good();
        t.samples[1].panels.clear();
        assert_eq!(rejected(&t), CELLS_MISMATCH, "a panel missing");
        let mut t = good();
        t.samples[1].panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![1.0]] });
        assert_eq!(rejected(&t), CELLS_MISMATCH, "a panel the header does not name");
        let mut t = good();
        t.samples[1].panels[0].series.clear();
        assert_eq!(rejected(&t), CELLS_MISMATCH, "no series");
    }

    #[test]
    fn sensor_keys_must_name_every_thermals_cell() {
        let mut t = good();
        t.header.panels.push(PanelHeader { panel: Panel::Thermals, cells: vec!["CPU: Package".into(), "GPU: Hot Spot".into()] });
        t.header.sensor_keys = vec!["cpu.temp".into(), "gpu0.hotspot".into()];
        for s in &mut t.samples {
            s.panels.push(PanelSample { panel: Panel::Thermals, series: vec![vec![60.0, 70.0]] });
        }
        assert_eq!(open_trace(&t.to_json()), Ok(t.clone()), "names and keys come back");
        t.header.sensor_keys.pop();
        assert_eq!(rejected(&t), CELLS_MISMATCH, "a key missing");
    }

    #[test]
    fn the_position_finds_the_nearest_sample() {
        let r = Replay::new(good());
        assert_eq!(r.duration_ms(), 1000);
        assert_eq!(r.sample_at(0).t_ms, 0);
        assert_eq!(r.sample_at(200).t_ms, 0);
        assert_eq!(r.sample_at(260).t_ms, 500);
        assert_eq!(r.sample_at(500).t_ms, 500);
        assert_eq!(r.sample_at(749).t_ms, 500);
        assert_eq!(r.sample_at(751).t_ms, 1000);
        assert_eq!(r.sample_at(5000).t_ms, 1000, "past the end: the last sample");
    }

    #[test]
    fn the_cells_come_from_the_traces_header_not_this_pc() {
        let mut t = Trace::new(Header::new("BIG-RIG", 0, 1000, 1000, 3));
        t.samples.push(Sample { t_ms: 0, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![1.0, 2.0, 3.0]] }], memory_used_bytes: 1, hottest_c: None });
        let r = Replay::new(t);
        let panels = r.cells_at(0);
        assert_eq!(panels.len(), 1);
        assert_eq!(panels[0].0, Panel::PerformanceCores);
        assert_eq!(panels[0].1, vec![("CPU 0".to_string(), 1.0), ("CPU 1".to_string(), 2.0), ("CPU 2".to_string(), 3.0)]);
    }

    #[test]
    fn the_replay_card_names_the_machine_and_the_date() {
        let r = Replay::new(good());
        assert_eq!(r.machine(), "DESK-01");
        assert_eq!(r.recorded_label(), "Recorded 2026-11-15 21:04:30");
        let mut t = good();
        t.header.machine = "  ".into();
        assert_eq!(Replay::new(t).machine(), "Unknown machine");
    }
}
