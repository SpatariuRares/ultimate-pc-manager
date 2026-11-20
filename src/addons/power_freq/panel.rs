//! The side panel of Power & Freq: the selected sensor (name, device, group,
//! big value, Min/Avg/Max since when, "Read from", the graph's time window), "Sources on this PC", the
//! EcoQoS note and the actions "Copy reading" / "Open Windows power options".
//! The content is pure (tested here); drawing is functional until #62.

use crate::sensors::{cpu, disk};
use crate::sensors::model::{DeviceKind, Group, Kind};
use crate::sensors::{Frame, Stats};

/// Points EcoQoS (per-program slowdown) to where it lives.
pub const ECOQOS_NOTE: &str = "To slow down a single program (EcoQoS), use Processes › Efficiency mode.";

/// What the panel shows of the selected sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct Detail {
    pub id: String,
    pub name: String,
    pub device: String,
    pub group: &'static str,
    /// The value in its unit, or why it is missing.
    pub value: String,
    pub unavailable: bool,
    /// `None` for counters and states, and before the first value.
    pub stats: Option<Figures>,
    /// The provider it came from.
    pub source: &'static str,
    /// The base clock (Hz) to draw on a CPU clock's graph.
    pub base_hz: Option<f64>,
}

/// Min / Avg / Max of a sensor in its unit, and since when they run.
#[derive(Clone, Debug, PartialEq)]
pub struct Figures {
    pub min: String,
    pub avg: String,
    pub max: String,
    /// "since 14:32".
    pub since: String,
}

impl Detail {
    /// The selected sensor in this frame, `None` if the frame has no such
    /// sensor (e.g. its provider is gone on this PC).
    pub fn of(frame: &Frame, stats: &Stats, fahrenheit: bool, id: &str) -> Option<Self> {
        let r = frame.reading(id)?;
        let figures = match (r.kind, stats.min_max(id), stats.avg(id)) {
            (Kind::Measure, Some((lo, hi)), Some(avg)) => Some(Figures {
                min: r.unit.format_in(lo, fahrenheit),
                avg: r.unit.format_in(avg, fahrenheit),
                max: r.unit.format_in(hi, fahrenheit),
                since: stats.since_text(),
            }),
            _ => None,
        };
        let base_hz = (r.group == Group::Clocks && r.device.kind == DeviceKind::Cpu)
            .then(|| frame.reading(crate::sensors::cpu::BASE_HZ).and_then(|b| b.value))
            .flatten();
        Some(Detail {
            id: r.id.clone(),
            name: r.name.clone(),
            device: r.device.name.clone(),
            group: r.group.name(),
            value: r.text_in(fahrenheit),
            unavailable: r.value.is_none(),
            stats: figures,
            source: r.source,
            base_hz,
        })
    }

    /// The text "Copy reading" puts on the clipboard: one line to paste in
    /// a message.
    pub fn copy_text(&self) -> String {
        let mut text = format!("{} ({} › {}): {}", self.name, self.device, self.group, self.value);
        if let Some(s) = &self.stats {
            text += &format!(", Min {}, Avg {}, Max {}, {}", s.min, s.avg, s.max, s.since);
        }
        text + &format!(", read from {}", self.source)
    }

    /// The caption under its graph: the time shown (`shown`, e.g. "Last
    /// minute") and, on a CPU clock, what the dashed line is.
    pub fn graph_caption(&self, shown: &str) -> String {
        match self.base_hz {
            Some(b) => format!("{shown}; dashed line = {} base", crate::sensors::model::Unit::Hz.format(b)),
            None => shown.to_string(),
        }
    }
}

/// "Sources on this PC": every registered provider and whether it works.
/// S.M.A.R.T., read by the disk provider, follows it with its own state:
/// "Working" once a disk answered, else why not ("needs administrator").
/// CPU limits and idle states, read by the Windows counters, follow them
/// the same way.
pub fn sources(frame: &Frame) -> Vec<(&'static str, String)> {
    let mut out = Vec::new();
    for s in &frame.sources {
        out.push((s.name, s.problem.clone().unwrap_or_else(|| "Working".into())));
        let parts: &[&'static str] = match s.name {
            disk::SOURCE => &[disk::SMART_SOURCE],
            cpu::SOURCE => &[cpu::LIMITS_SOURCE, cpu::IDLE_SOURCE],
            _ => &[],
        };
        for &part in parts {
            out.extend(part_state(frame, part).map(|state| (part, state)));
        }
    }
    out
}

/// The state of the rows a provider reads under a source of their own
/// (S.M.A.R.T., CPU limits, idle states) in this frame; `None` when no row
/// has it.
fn part_state(frame: &Frame, source: &str) -> Option<String> {
    let rows: Vec<_> = frame.readings().filter(|r| r.source == source).collect();
    let first = rows.first()?;
    Some(if rows.iter().any(|r| r.value.is_some()) { "Working".into() } else { first.unavailable.clone().unwrap_or_else(|| "no value".into()) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Reading, SourceStatus, Unit};
    use std::time::Instant;

    fn frame(seq: u64, avg_ghz: f64) -> Frame {
        let at = Instant::now();
        let cpu = Device::cpu("Test CPU");
        let clock = |id: &str, name: &str, ghz: f64| Reading::measure(id, cpu.clone(), Group::Clocks, name, Unit::Hz, "Windows counters", at).with(Some(ghz * 1e9), "");
        let readings = vec![
            clock("cpu.package.frequency.average_effective_hz", "Core Average", avg_ghz),
            clock(crate::sensors::cpu::BASE_HZ, "Base Clock", 4.7),
            Reading::measure("cpu.core3.parked", cpu.clone(), Group::Clocks, "CPU 3 Parked", Unit::YesNo { yes: "Parked", no: "Active" }, "Windows counters", at)
                .with(Some(1.0), "")
                .counter(),
            Reading::measure("cpu.package.utilization_percent", cpu, Group::Utilization, "CPU Total", Unit::Percent, "Windows counters", at)
                .with(None, "needs administrator"),
        ];
        let sources = vec![
            SourceStatus { name: "Windows counters", problem: None },
            SourceStatus { name: "NVML", problem: Some("no NVIDIA GPU".into()) },
        ];
        Frame::build(seq, &[readings], sources)
    }

    fn observed() -> (Frame, Stats) {
        let mut stats = Stats::new(14 * 3600 + 32 * 60);
        stats.observe(&frame(1, 4.0));
        let last = frame(2, 5.2);
        stats.observe(&last);
        (last, stats)
    }

    #[test]
    fn copy_reading_gives_name_value_min_avg_max_the_statistics_start_and_source() {
        let (f, stats) = observed();
        let d = Detail::of(&f, &stats, false, "cpu.package.frequency.average_effective_hz").unwrap();
        assert_eq!(d.copy_text(), "Core Average (Test CPU › Clocks): 5.20 GHz, Min 4.00 GHz, Avg 4.60 GHz, Max 5.20 GHz, since 14:32, read from Windows counters");
    }

    #[test]
    fn the_panel_gives_min_avg_max_and_since_when() {
        let (f, stats) = observed();
        let d = Detail::of(&f, &stats, false, "cpu.package.frequency.average_effective_hz").unwrap();
        let s = d.stats.expect("a measure has statistics");
        assert_eq!((s.min.as_str(), s.avg.as_str(), s.max.as_str(), s.since.as_str()), ("4.00 GHz", "4.60 GHz", "5.20 GHz", "since 14:32"));
    }

    #[test]
    fn copy_reading_of_a_state_has_no_min_max() {
        let (f, stats) = observed();
        let d = Detail::of(&f, &stats, false, "cpu.core3.parked").unwrap();
        assert_eq!(d.copy_text(), "CPU 3 Parked (Test CPU › Clocks): Parked, read from Windows counters");
    }

    #[test]
    fn copy_reading_of_a_missing_value_says_why() {
        let (f, stats) = observed();
        let d = Detail::of(&f, &stats, false, "cpu.package.utilization_percent").unwrap();
        assert!(d.unavailable);
        assert_eq!(d.copy_text(), "CPU Total (Test CPU › Utilization): needs administrator, read from Windows counters");
    }

    #[test]
    fn cpu_clocks_carry_the_base_clock_for_their_graph() {
        let (f, stats) = observed();
        let clock = Detail::of(&f, &stats, false, "cpu.package.frequency.average_effective_hz").unwrap();
        assert_eq!(clock.base_hz, Some(4.7e9));
        let util = Detail::of(&f, &stats, false, "cpu.package.utilization_percent").unwrap();
        assert_eq!(util.base_hz, None);
    }

    #[test]
    fn the_graph_caption_states_the_time_shown_and_the_base_line_of_clocks() {
        let (f, stats) = observed();
        let clock = Detail::of(&f, &stats, false, "cpu.package.frequency.average_effective_hz").unwrap();
        assert_eq!(clock.graph_caption("Last 2 minutes"), "Last 2 minutes; dashed line = 4.70 GHz base");
        let util = Detail::of(&f, &stats, false, "cpu.package.utilization_percent").unwrap();
        assert_eq!(util.graph_caption("Last 40 seconds"), "Last 40 seconds");
    }

    #[test]
    fn a_sensor_this_pc_does_not_have_shows_nothing() {
        let (f, stats) = observed();
        assert_eq!(Detail::of(&f, &stats, false, "gpu0.temperature"), None);
    }

    #[test]
    fn sources_on_this_pc_say_which_providers_work() {
        let (f, _) = observed();
        assert_eq!(sources(&f), vec![("Windows counters", "Working".to_string()), ("NVML", "no NVIDIA GPU".to_string())]);
    }

    #[test]
    fn smart_is_listed_after_the_disk_counters_with_its_own_state() {
        use crate::sensors::disk::{self, DiskSample, DiskSmart};
        let disk_sources = || vec![SourceStatus { name: disk::SOURCE, problem: None }, SourceStatus { name: "NVML", problem: Some("no NVIDIA GPU".into()) }];
        let disk = |smart| DiskSample { number: 0, model: "Disk".into(), idle_percent: Some(90.0), smart: Some(smart), ..Default::default() }.readings(Instant::now());
        let not_elevated = Frame::build(1, &[disk(DiskSmart::Nvme(disk::nvme::Smart::NeedsAdmin))], disk_sources());
        assert_eq!(
            sources(&not_elevated),
            vec![("Disk counters", "Working".to_string()), ("S.M.A.R.T.", "needs administrator".to_string()), ("NVML", "no NVIDIA GPU".to_string())]
        );
        let health = disk::nvme::NvmeHealth { composite_kelvin: 310, ..Default::default() };
        let read = Frame::build(1, &[disk(DiskSmart::Nvme(disk::nvme::Smart::Read(health)))], disk_sources());
        assert_eq!(sources(&read)[1], ("S.M.A.R.T.", "Working".to_string()));
        let (no_disk, _) = observed();
        assert!(sources(&no_disk).iter().all(|(name, _)| *name != "S.M.A.R.T."), "no S.M.A.R.T. disk, no line");
    }

    #[test]
    fn cpu_limits_are_listed_after_the_windows_counters_with_their_own_state() {
        use crate::sensors::cpu::{CoreSample, CpuSample, PERFORMANCE_LIMIT};
        let cpu_sources = || vec![SourceStatus { name: "Windows counters", problem: None }, SourceStatus { name: "NVML", problem: Some("no NVIDIA GPU".into()) }];
        let cpu = |limit: Option<f64>, flags: Option<u32>| {
            let core = CoreSample { performance_limit_percent: limit, limit_flags: flags, ..Default::default() };
            CpuSample { name: "Test CPU".into(), cores: vec![core.clone(), core], ..Default::default() }.readings(Instant::now())
        };
        let read = Frame::build(1, &[cpu(Some(100.0), Some(0))], cpu_sources());
        assert_eq!(
            sources(&read),
            vec![
                ("Windows counters", "Working".to_string()),
                ("CPU limits (Windows counters)", "Working".to_string()),
                ("Idle states (Windows counters)", "no counter on this PC".to_string()),
                ("NVML", "no NVIDIA GPU".to_string())
            ]
        );
        let missing = Frame::build(1, &[cpu(None, None)], cpu_sources());
        assert_eq!(sources(&missing)[1], ("CPU limits (Windows counters)", "no counter on this PC".to_string()));
        let detail = Detail::of(&read, &Stats::default(), false, PERFORMANCE_LIMIT).unwrap();
        assert_eq!((detail.group, detail.source), ("Limits", "CPU limits (Windows counters)"), "Read from names the source");
    }

    #[test]
    fn idle_states_are_listed_after_the_cpu_limits_with_their_own_state() {
        use crate::sensors::cpu::{CoreSample, CpuSample, IDLE_C1};
        let cpu_sources = || vec![SourceStatus { name: "Windows counters", problem: None }];
        let cpu = |c1: Option<f64>| {
            let core = CoreSample { c1_percent: c1, ..Default::default() };
            CpuSample { name: "Test CPU".into(), cores: vec![core.clone(), core], ..Default::default() }.readings(Instant::now())
        };
        let read = Frame::build(1, &[cpu(Some(40.0))], cpu_sources());
        let listed = sources(&read);
        assert_eq!(listed.last(), Some(&("Idle states (Windows counters)", "Working".to_string())), "{listed:?}");
        let missing = Frame::build(1, &[cpu(None)], cpu_sources());
        assert_eq!(sources(&missing).last(), Some(&("Idle states (Windows counters)", "no counter on this PC".to_string())));
        let detail = Detail::of(&read, &Stats::default(), false, IDLE_C1).unwrap();
        assert_eq!((detail.group, detail.source), ("Idle states", "Idle states (Windows counters)"), "Read from names the source");
    }
}
