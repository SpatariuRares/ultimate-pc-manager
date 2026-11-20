//! "Export as CSV" (our addition): a trace as a table, one row per sample,
//! one column for the time and one per series and header cell. The trace
//! file stays the saved format; CSV is only an export.

use super::trace::{Panel, Trace};
use crate::platform::csv_field;

/// A panel's title, as the page shows it.
fn title(panel: Panel) -> &'static str {
    match panel {
        Panel::PerformanceCores => "Performance cores",
        Panel::EfficiencyCores => "Efficiency cores",
        Panel::GpuCores => "GPU cores",
        Panel::NeuralEngine => "Neural Engine",
        Panel::MediaEngine => "Media Engine",
        Panel::Thermals => "Thermals",
    }
}

/// The suffix of a panel's series after the first.
const SERIES_SUFFIX: [&str; 3] = ["", " (secondary)", " (tertiary)"];

/// The trace as CSV text: "Time (ms)", then for every panel of the header
/// each of its series (as many as its samples carry) with one column per
/// cell, then "Memory used (bytes)". A value a sample lacks stays empty.
pub fn csv(trace: &Trace) -> String {
    // (panel, series index, cell index) of every value column, in order.
    let mut columns = Vec::new();
    let mut names = vec!["Time (ms)".to_string()];
    for head in &trace.header.panels {
        let series = trace
            .samples
            .iter()
            .flat_map(|s| s.panels.iter().filter(|p| p.panel == head.panel))
            .map(|p| p.series.len())
            .max()
            .unwrap_or(0);
        for k in 0..series {
            let suffix = SERIES_SUFFIX.get(k).map_or_else(|| format!(" (series {})", k + 1), |s| s.to_string());
            for (i, cell) in head.cells.iter().enumerate() {
                columns.push((head.panel, k, i));
                names.push(format!("{}: {cell}{suffix}", title(head.panel)));
            }
        }
    }
    names.push("Memory used (bytes)".into());
    let mut out = String::new();
    push_row(&mut out, names);
    for sample in &trace.samples {
        let mut row = vec![sample.t_ms.to_string()];
        for &(panel, k, i) in &columns {
            let value = sample.panels.iter().find(|p| p.panel == panel).and_then(|p| p.series.get(k)).and_then(|v| v.get(i));
            row.push(value.map_or_else(String::new, |v| v.to_string()));
        }
        row.push(sample.memory_used_bytes.to_string());
        push_row(&mut out, row);
    }
    out
}

/// The CSV's default name, from the recording date in the header:
/// `flight-recorder_YYYYMMDD_HHMM.csv`.
pub fn file_name(trace: &Trace) -> String {
    let digits: String = trace.header.recorded.chars().filter(char::is_ascii_digit).collect();
    match (digits.get(..8), digits.get(8..12)) {
        (Some(date), Some(time)) => format!("flight-recorder_{date}_{time}.csv"),
        _ => "flight-recorder.csv".into(),
    }
}

/// Appends one CSV line.
fn push_row(out: &mut String, fields: Vec<String>) {
    out.push_str(&fields.iter().map(|f| csv_field(f)).collect::<Vec<_>>().join(","));
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::flight_recorder::trace::{Header, Panel, PanelHeader, PanelSample, Sample};

    /// A two-core machine, two samples half a second apart.
    fn small_trace() -> Trace {
        let mut trace = Trace::new(Header::new("DESK-01", 0, 500, 16_000_000_000, 2));
        trace.samples.push(Sample { t_ms: 0, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![12.5, 3.25]] }], memory_used_bytes: 8_000_000_000, hottest_c: None });
        trace.samples.push(Sample { t_ms: 500, panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![99.0, 0.1]] }], memory_used_bytes: 8_100_000_000, hottest_c: None });
        trace
    }

    #[test]
    fn one_row_per_sample_and_one_column_per_cell() {
        assert_eq!(
            csv(&small_trace()),
            "Time (ms),Performance cores: CPU 0,Performance cores: CPU 1,Memory used (bytes)\n\
             0,12.5,3.25,8000000000\n\
             500,99,0.1,8100000000\n"
        );
    }

    #[test]
    fn every_series_of_every_panel_gets_its_columns() {
        let mut trace = small_trace();
        trace.header.panels.push(PanelHeader { panel: Panel::GpuCores, cells: vec!["3D".into(), "Copy, 1".into()] });
        trace.samples[0].panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![40.0, 2.0], vec![1800.0, 1800.0]] });
        trace.samples[1].panels.push(PanelSample { panel: Panel::GpuCores, series: vec![vec![41.0, 0.0], vec![2100.0, 2100.0]] });
        let text = csv(&trace);
        let mut lines = text.lines();
        assert_eq!(
            lines.next().unwrap(),
            "Time (ms),Performance cores: CPU 0,Performance cores: CPU 1,\
             GPU cores: 3D,\"GPU cores: Copy, 1\",GPU cores: 3D (secondary),\"GPU cores: Copy, 1 (secondary)\",\
             Memory used (bytes)"
        );
        assert_eq!(lines.next().unwrap(), "0,12.5,3.25,40,2,1800,1800,8000000000");
        assert_eq!(lines.next().unwrap(), "500,99,0.1,41,0,2100,2100,8100000000");
        assert_eq!(lines.next(), None);
    }

    #[test]
    fn the_csv_is_named_after_the_recording_date() {
        let trace = Trace::new(Header::new("PC", 1_794_776_670, 500, 1, 1));
        assert_eq!(file_name(&trace), "flight-recorder_20261115_2104.csv");
    }

    #[test]
    fn a_value_a_sample_lacks_is_an_empty_field() {
        let mut trace = small_trace();
        trace.samples[1].panels.clear();
        assert_eq!(csv(&trace).lines().nth(2).unwrap(), "500,,,8100000000");
    }
}
