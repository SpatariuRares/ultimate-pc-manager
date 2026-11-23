//! The Flight Recorder trace (pure): a header that names the machine and
//! the cells of every panel, then samples from 00:00. Saved as JSON.

use serde::{Deserialize, Serialize};

use crate::cpu_topology::CoreTopology;

/// The version of the trace format this build writes.
pub const FORMAT_VERSION: u32 = 1;

/// The platform written in (and later checked from) a trace's header.
pub const PLATFORM: &str = "windows";

/// A telemetry panel of the page. This slice records the cores (all as
/// performance cores) and memory (in the header and every sample); the
/// other panels join the header and samples as they are built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Panel {
    PerformanceCores,
    EfficiencyCores,
    GpuCores,
    NeuralEngine,
    MediaEngine,
    Thermals,
}

/// A panel in the header: its cells' names, in the order of the values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelHeader {
    pub panel: Panel,
    pub cells: Vec<String>,
}

/// What a trace says about its recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Header {
    pub format_version: u32,
    pub platform: String,
    /// The machine's name ("This machine" when it is unknown).
    pub machine: String,
    /// When the recording started, local ISO 8601.
    pub recorded: String,
    /// Milliseconds between samples (the app's update speed).
    pub interval_ms: u64,
    pub memory_total_bytes: u64,
    pub panels: Vec<PanelHeader>,
    /// The GPU whose engines the trace holds (empty without one).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gpu: String,
    /// The sensor service's keys (metric ids) of the Thermals panel's
    /// cells, in their order; absent when no temperature was recorded.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sensor_keys: Vec<String>,
}

impl Header {
    /// The header of a recording started at `local_secs` on a machine
    /// with `cores` logical processors ("CPU 0", "CPU 1", ...).
    pub fn new(machine: &str, local_secs: i64, interval_ms: u64, memory_total_bytes: u64, cores: usize) -> Self {
        Header {
            format_version: FORMAT_VERSION,
            platform: PLATFORM.into(),
            machine: machine.into(),
            recorded: iso_local(local_secs),
            interval_ms,
            memory_total_bytes,
            panels: vec![PanelHeader { panel: Panel::PerformanceCores, cells: core_names(cores) }],
            gpu: String::new(),
            sensor_keys: Vec::new(),
        }
    }

    /// The header with its cores divided by `topology`: the efficiency
    /// cores move to an Efficiency cores panel right after the performance
    /// cores (none without efficiency cores), keeping their names.
    pub fn split_cores(mut self, topology: &CoreTopology) -> Self {
        let Some(at) = self.panels.iter().position(|p| p.panel == Panel::PerformanceCores) else { return self };
        let (performance, efficiency) = topology.split(&self.panels[at].cells);
        if !efficiency.is_empty() {
            self.panels[at].cells = performance;
            self.panels.insert(at + 1, PanelHeader { panel: Panel::EfficiencyCores, cells: efficiency });
        }
        self
    }

    /// The trace's default file name, dated at the recording's start
    /// (`recorded`); now when that date cannot be read.
    pub fn file_name(&self) -> String {
        let r = self.recorded.as_bytes();
        let digits = |a: usize, b: usize| r.get(a..b).filter(|d| d.iter().all(u8::is_ascii_digit)).map(|d| String::from_utf8_lossy(d).into_owned());
        match (digits(0, 4), digits(5, 7), digits(8, 10), digits(11, 13), digits(14, 16)) {
            (Some(y), Some(mo), Some(d), Some(h), Some(mi)) => format!("{NAME_PREFIX}_{y}{mo}{d}_{h}{mi}.{TRACE_EXTENSION}"),
            _ => default_file_name(crate::platform::local_secs()),
        }
    }
}

/// The names of `cores` logical processors: "CPU 0", "CPU 1", ...
pub fn core_names(cores: usize) -> Vec<String> {
    (0..cores).map(|i| format!("CPU {i}")).collect()
}

/// A panel's values in one sample: its series (primary, then secondary
/// and tertiary when the panel has them), each one value per cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelSample {
    pub panel: Panel,
    pub series: Vec<Vec<f32>>,
}

/// One sample: when, every panel's values and the memory in use.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Milliseconds since the first sample (which is at 0).
    pub t_ms: u64,
    pub panels: Vec<PanelSample>,
    pub memory_used_bytes: u64,
    /// The hottest temperature (°C) the sensors reported at this sample,
    /// over every sensor, not only the Thermals cells (#44); absent in a
    /// trace without temperatures or written before it was kept.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hottest_c: Option<f32>,
}

/// A whole trace: the header, then the samples from 00:00.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Trace {
    pub header: Header,
    pub samples: Vec<Sample>,
}

impl Trace {
    /// A trace with no samples yet.
    pub fn new(header: Header) -> Self {
        Trace { header, samples: Vec::new() }
    }

    /// The trace as JSON text.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// Reads a trace back from JSON text (validation comes with Replay,
    /// its first caller outside the tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }
}

/// The trace file's extension (decided in #35; it follows the app's
/// final name, still open).
pub const TRACE_EXTENSION: &str = "upmtrace";

/// The default file name, before the date and time.
const NAME_PREFIX: &str = "flight-recorder";

/// A local time, split into its calendar fields.
struct Civil {
    year: i64,
    month: u32,
    day: u32,
    hour: i64,
    minute: i64,
    second: i64,
}

/// Splits `local_secs` (seconds since 1970-01-01 in local time, as
/// [`crate::platform::local_secs`] gives) into date and time (proleptic Gregorian).
fn civil(local_secs: i64) -> Civil {
    let days = local_secs.div_euclid(86_400);
    let secs = local_secs.rem_euclid(86_400);
    // Howard Hinnant's days-to-civil.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    Civil { year, month, day, hour: secs / 3600, minute: (secs / 60) % 60, second: secs % 60 }
}

/// The default file name for a trace recorded at `local_secs`:
/// `flight-recorder_YYYYMMDD_HHMM.<ext>`.
pub fn default_file_name(local_secs: i64) -> String {
    let c = civil(local_secs);
    format!("{NAME_PREFIX}_{:04}{:02}{:02}_{:02}{:02}.{TRACE_EXTENSION}", c.year, c.month, c.day, c.hour, c.minute)
}

/// `local_secs` as an ISO 8601 local time (`2026-11-15T21:04:30`), as the
/// header's recording date.
pub fn iso_local(local_secs: i64) -> String {
    let c = civil(local_secs);
    format!("{:04}-{:02}-{:02}T{:02}:{:02}:{:02}", c.year, c.month, c.day, c.hour, c.minute, c.second)
}

/// Elapsed time as the RECORDING card shows it: `HH:MM:SS`.
pub fn format_elapsed(ms: u64) -> String {
    let s = ms / 1000;
    format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

/// "%1 samples", its thousands grouped ("14,400 samples"),
/// the same everywhere a count of samples shows.
pub fn samples_label(n: usize) -> String {
    let digits = n.to_string();
    let mut grouped = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped} samples")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_time_reads_as_hours_minutes_seconds() {
        assert_eq!(format_elapsed(0), "00:00:00");
        assert_eq!(format_elapsed(59_999), "00:00:59");
        assert_eq!(format_elapsed(3_723_000), "01:02:03");
        assert_eq!(format_elapsed(100 * 3_600_000), "100:00:00");
    }

    /// A two-core machine, two samples half a second apart.
    fn small_trace() -> Trace {
        let header = Header::new("DESK-01", NOV_15_2104, 500, 16 * 1024 * 1024 * 1024, 2);
        let mut trace = Trace::new(header);
        trace.samples.push(Sample { t_ms: 0, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![12.5, 3.25]] }], memory_used_bytes: 8_000_000_000, hottest_c: None });
        trace.samples.push(Sample { t_ms: 500, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![99.0, 0.1]] }], memory_used_bytes: 8_100_000_000, hottest_c: None });
        trace
    }

    #[test]
    fn a_trace_comes_back_identical_from_json() {
        let trace = small_trace();
        assert_eq!(Trace::from_json(&trace.to_json()).unwrap(), trace);
    }

    #[test]
    fn each_samples_hottest_temperature_is_kept_and_older_traces_without_it_still_read() {
        let mut trace = small_trace();
        trace.samples[1].hottest_c = Some(71.5);
        let json: serde_json::Value = serde_json::from_str(&trace.to_json()).unwrap();
        assert_eq!(json["samples"][1]["hottestC"], 71.5);
        assert!(json["samples"][0].get("hottestC").is_none(), "absent, not null, when not read");
        assert_eq!(Trace::from_json(&trace.to_json()).unwrap(), trace);
        let older = r#"{"header":{"formatVersion":1,"platform":"windows","machine":"PC","recorded":"2026-11-15T21:04:30","intervalMs":1000,"memoryTotalBytes":10,"panels":[{"panel":"performance-cores","cells":["CPU 0"]}]},"samples":[{"tMs":0,"panels":[{"panel":"performance-cores","series":[[5.0]]}],"memoryUsedBytes":1}]}"#;
        assert_eq!(Trace::from_json(older).unwrap().samples[0].hottest_c, None);
    }

    #[test]
    fn the_json_has_a_header_and_samples_from_zero() {
        let json: serde_json::Value = serde_json::from_str(&small_trace().to_json()).unwrap();
        let h = &json["header"];
        assert_eq!(h["formatVersion"], 1);
        assert_eq!(h["platform"], "windows");
        assert_eq!(h["machine"], "DESK-01");
        assert_eq!(h["recorded"], "2026-11-15T21:04:30");
        assert_eq!(h["intervalMs"], 500);
        assert_eq!(h["memoryTotalBytes"], 17_179_869_184u64);
        assert_eq!(h["panels"][0]["panel"], "performance-cores");
        assert_eq!(h["panels"][0]["cells"], serde_json::json!(["CPU 0", "CPU 1"]));
        assert_eq!(json["samples"][0]["tMs"], 0);
        assert_eq!(json["samples"][1]["tMs"], 500);
        assert_eq!(json["samples"][1]["memoryUsedBytes"], 8_100_000_000u64);
    }

    #[test]
    fn a_hybrid_processors_header_names_its_performance_and_efficiency_cores() {
        let hybrid = CoreTopology::from_efficiency_classes(&[1, 1, 0, 1, 0]);
        let header = Header::new("LAPTOP", 0, 1000, 1, 5).split_cores(&hybrid);
        let panels: Vec<(Panel, Vec<String>)> = header.panels.iter().map(|p| (p.panel, p.cells.clone())).collect();
        assert_eq!(panels, [(Panel::PerformanceCores, core_list(&["CPU 0", "CPU 1", "CPU 3"])), (Panel::EfficiencyCores, core_list(&["CPU 2", "CPU 4"]))]);
    }

    #[test]
    fn without_efficiency_cores_the_header_has_no_efficiency_panel() {
        let uniform = CoreTopology::from_efficiency_classes(&[0, 0]);
        assert_eq!(Header::new("DESK", 0, 1000, 1, 2).split_cores(&uniform), Header::new("DESK", 0, 1000, 1, 2));
    }

    fn core_list(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// 2026-11-15 21:04:30, local time.
    const NOV_15_2104: i64 = 1_794_776_670;

    #[test]
    fn the_default_name_carries_the_date_and_time() {
        assert_eq!(default_file_name(NOV_15_2104), format!("flight-recorder_20261115_2104.{TRACE_EXTENSION}"));
        assert_eq!(default_file_name(1_709_164_800), format!("flight-recorder_20240229_0000.{TRACE_EXTENSION}"), "a leap day at midnight");
    }

    #[test]
    fn the_recording_date_is_an_iso_local_time() {
        assert_eq!(iso_local(NOV_15_2104), "2026-11-15T21:04:30");
    }

    #[test]
    fn a_traces_file_name_is_dated_at_its_start() {
        assert_eq!(Header::new("PC", NOV_15_2104, 1000, 1, 1).file_name(), format!("flight-recorder_20261115_2104.{TRACE_EXTENSION}"));
        let mut odd = Header::new("PC", NOV_15_2104, 1000, 1, 1);
        odd.recorded = "not a date".into();
        assert!(odd.file_name().starts_with("flight-recorder_"), "an unreadable date falls back to now");
    }

    #[test]
    fn the_sample_count_reads_as_n_samples_with_its_thousands_grouped() {
        assert_eq!(samples_label(0), "0 samples");
        assert_eq!(samples_label(999), "999 samples");
        assert_eq!(samples_label(14_400), "14,400 samples");
        assert_eq!(samples_label(1_234_567), "1,234,567 samples");
    }
}
