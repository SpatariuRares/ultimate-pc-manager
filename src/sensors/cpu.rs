//! The CPU provider ("Windows counters"): PDH `Processor Information`
//! counters (% Processor Performance, Processor Frequency, Parking Status,
//! % Processor Utility) and `CallNtPowerInformation` (CurrentMhz,
//! MaximumMhz per logical processor). The Limits group comes from the
//! same query: `% Performance Limit` and `Performance Limit Flags`, and so
//! does the Idle states group: `% C1 Time`, `% C2 Time`, `% C3 Time`.

use std::time::Instant;

use super::model::{CounterSet, Device, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Windows counters";
/// The metric of "Core Average": the package's effective clock.
pub const CORE_AVERAGE: &str = "cpu.package.frequency.average_effective_hz";
/// The metric of the base (nominal) clock, CPU › General.
pub const BASE_HZ: &str = "cpu.package.frequency.base_hz";
/// The CPU package temperature (a vendor SDK fills it; none does yet).
pub const PACKAGE_TEMPERATURE: &str = "cpu.package.temperature_c";
/// The source of the Limits rows in "Sources on this PC" / "Read from".
pub const LIMITS_SOURCE: &str = "CPU limits (Windows counters)";
/// The source of the Idle states rows in "Sources on this PC" / "Read from".
pub const IDLE_SOURCE: &str = "Idle states (Windows counters)";
/// The package's worst `% Performance Limit` (the lowest of every core).
pub const PERFORMANCE_LIMIT: &str = "cpu.limits.performance_pct";
/// Every core's limit reasons, decoded into words.
pub const LIMIT_REASONS: &str = "cpu.limits.reasons";
/// The words of a limit reasons row when nothing holds the CPU back.
pub const NO_LIMIT: &str = "none";
/// The package averages of the idle states (C1 / C2 / C3 residency).
pub const IDLE_C1: &str = "cpu.idle.c1_pct";
pub const IDLE_C2: &str = "cpu.idle.c2_pct";
pub const IDLE_C3: &str = "cpu.idle.c3_pct";
const NO_COUNTER: &str = "no counter on this PC";

/// The documented reasons of a performance limit, by bit. Source:
/// Microsoft Learn, `PEP_PPM_PERF_CONSTRAINTS` (pep_x.h), member
/// `LimitReasons`:
/// https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/pep_x/ns-pep_x-_pep_ppm_query_perf_constraints
/// (PERFORMANCE_LIMIT_THERMAL 0x1: out-of-band thermal condition;
/// PERFORMANCE_LIMIT_POWER 0x2: power within hardware safety limits;
/// PERFORMANCE_LIMIT_DOMAIN_DEPENDENCY 0x4: voltage or frequency dependency
/// on a non-processor device). The `Performance Limit Flags` counter of
/// `Processor Information` reports these reasons.
const LIMIT_BITS: [(u32, &str); 3] = [(0x1, "thermal"), (0x2, "power"), (0x4, "domain dependency")];

/// The words of a `Performance Limit Flags` bitmask, lowest bit first;
/// empty when nothing limits the processor. Undocumented bits say
/// "unknown reason (bit N)".
pub fn limit_reasons(flags: u32) -> Vec<String> {
    (0..32)
        .map(|bit| 1u32 << bit)
        .filter(|mask| flags & mask != 0)
        .map(|mask| match LIMIT_BITS.iter().find(|(m, _)| *m == mask) {
            Some((_, word)) => word.to_string(),
            None => format!("unknown reason (bit {})", mask.trailing_zeros()),
        })
        .collect()
}

/// A limit reasons row's words: the reasons joined, or [`NO_LIMIT`].
fn reasons_text(flags: u32) -> String {
    let words = limit_reasons(flags);
    if words.is_empty() { NO_LIMIT.into() } else { words.join(", ") }
}

/// The effective clock (Hz): base clock × % Processor Performance / 100,
/// by the rule every page shares; `None` when either input is unknown.
pub fn effective_hz(base_mhz: Option<f64>, performance_percent: Option<f64>) -> Option<f64> {
    crate::metrics::cpu_speed_mhz(base_mhz, performance_percent).map(|mhz| mhz * 1e6)
}

/// The logical processor of a `Processor Information` instance ("g,n"),
/// numbered across groups of 64; `None` for totals.
pub fn logical_index(instance: &str) -> Option<usize> {
    let (g, n) = instance.split_once(',')?;
    Some(g.parse::<usize>().ok()? * 64 + n.parse::<usize>().ok()?)
}

/// One logical processor's raw values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CoreSample {
    pub current_mhz: Option<f64>,
    pub max_mhz: Option<f64>,
    pub parked: Option<bool>,
    pub utilization_percent: Option<f64>,
    /// `% Performance Limit`: 100 when nothing holds the processor back.
    pub performance_limit_percent: Option<f64>,
    /// `Performance Limit Flags`, the raw bitmask.
    pub limit_flags: Option<u32>,
    /// `% C1 Time`, `% C2 Time`, `% C3 Time`: the share of time spent in
    /// each idle state.
    pub c1_percent: Option<f64>,
    pub c2_percent: Option<f64>,
    pub c3_percent: Option<f64>,
}

/// One read of the CPU's raw values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuSample {
    pub name: String,
    /// The base (nominal) clock: `Processor Frequency`.
    pub base_mhz: Option<f64>,
    /// `% Processor Performance` of the package.
    pub performance_percent: Option<f64>,
    pub utilization_percent: Option<f64>,
    pub cores: Vec<CoreSample>,
}

impl CpuSample {
    /// The readings of this sample, in the order shown.
    pub fn readings(&self, at: Instant) -> Vec<Reading> {
        let cpu = Device::cpu(&self.name);
        let row = |id: &str, group: Group, name: &str, unit: Unit, value: Option<f64>| {
            Reading::measure(id, cpu.clone(), group, name, unit, SOURCE, at).with(value, NO_COUNTER)
        };
        let mhz = |v: Option<f64>| v.map(|v| v * 1e6);
        let reported: Vec<f64> = self.cores.iter().filter_map(|c| c.current_mhz).collect();
        let reported_avg = (!reported.is_empty()).then(|| reported.iter().sum::<f64>() / reported.len() as f64);
        let mut out = vec![
            row("cpu.package.utilization_percent", Group::Utilization, "CPU Total", Unit::Percent, self.utilization_percent.map(|u| u.clamp(0.0, 100.0))),
        ];
        for (i, c) in self.cores.iter().enumerate() {
            out.push(row(&format!("cpu.core{i}.utilization_percent"), Group::Utilization, &format!("CPU {i}"), Unit::Percent, c.utilization_percent.map(|u| u.clamp(0.0, 100.0))));
        }
        let effective = effective_hz(self.base_mhz, self.performance_percent);
        out.push(row(CORE_AVERAGE, Group::Clocks, "Core Average", Unit::Hz, effective));
        out.push(row("cpu.package.frequency.average_reported_hz", Group::Clocks, "Reported Clock", Unit::Hz, mhz(reported_avg)));
        out.push(row(BASE_HZ, Group::General, "Base Frequency", Unit::Hz, mhz(self.base_mhz)));
        out.extend(self.limits(&cpu, at));
        out.extend(self.idle_states(&cpu, at));
        for (i, c) in self.cores.iter().enumerate() {
            out.push(row(&format!("cpu.core{i}.frequency.reported_hz"), Group::Clocks, &format!("CPU {i} Clock"), Unit::Hz, mhz(c.current_mhz)));
            out.push(row(&format!("cpu.core{i}.frequency.max_hz"), Group::Clocks, &format!("CPU {i} Max Clock"), Unit::Hz, mhz(c.max_mhz)));
            let parked = Unit::YesNo { yes: "Parked", no: "Active" };
            out.push(row(&format!("cpu.core{i}.parked"), Group::Clocks, &format!("CPU {i} Parked"), parked, c.parked.map(f64::from)).counter());
        }
        out
    }

    /// The Limits group: the package's worst performance limit and every
    /// core's reasons, then one pair of rows per logical processor.
    fn limits(&self, cpu: &Device, at: Instant) -> Vec<Reading> {
        let percent = |id: &str, name: &str, v: Option<f64>| Reading::measure(id, cpu.clone(), Group::Limits, name, Unit::Percent, LIMITS_SOURCE, at).with(v, NO_COUNTER);
        let reasons = |id: &str, name: &str, flags: Option<u32>| Reading::state(id, cpu.clone(), Group::Limits, name, LIMITS_SOURCE, at).with_text(flags.map(reasons_text), NO_COUNTER);
        let worst = self.cores.iter().filter_map(|c| c.performance_limit_percent).reduce(f64::min);
        let flags: Vec<u32> = self.cores.iter().filter_map(|c| c.limit_flags).collect();
        let all = (!flags.is_empty()).then(|| flags.iter().fold(0, |a, f| a | f));
        let mut out = vec![percent(PERFORMANCE_LIMIT, "Package Performance Limit", worst), reasons(LIMIT_REASONS, "Package Limit Reasons", all)];
        for (i, c) in self.cores.iter().enumerate() {
            out.push(percent(&format!("cpu.core{i}.limits.performance_pct"), &format!("CPU {i} Performance Limit"), c.performance_limit_percent));
            out.push(reasons(&format!("cpu.core{i}.limits.reasons"), &format!("CPU {i} Limit Reasons"), c.limit_flags));
        }
        out
    }

    /// The Idle states group: the package average of C1 / C2 / C3, then
    /// one row per logical processor per state.
    fn idle_states(&self, cpu: &Device, at: Instant) -> Vec<Reading> {
        let percent = |id: &str, name: &str, v: Option<f64>| {
            Reading::measure(id, cpu.clone(), Group::IdleStates, name, Unit::Percent, IDLE_SOURCE, at).with(v.map(|v| v.clamp(0.0, 100.0)), NO_COUNTER)
        };
        let states: [(&str, &str, fn(&CoreSample) -> Option<f64>); 3] = [
            ("c1", IDLE_C1, |c| c.c1_percent),
            ("c2", IDLE_C2, |c| c.c2_percent),
            ("c3", IDLE_C3, |c| c.c3_percent),
        ];
        let mut out = Vec::new();
        for (key, id, get) in states {
            let values: Vec<f64> = self.cores.iter().filter_map(get).collect();
            let avg = (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64);
            out.push(percent(id, &format!("Package {}", key.to_uppercase()), avg));
        }
        for (i, c) in self.cores.iter().enumerate() {
            for (key, _, get) in states {
                out.push(percent(&format!("cpu.core{i}.idle.{key}_pct"), &format!("CPU {i} {}", key.to_uppercase()), get(c)));
            }
        }
        out
    }
}


/// The CPU provider. Built on its worker thread (holds a PDH query).
pub struct CpuCounters {
    counters: imp::Counters,
    name: String,
    /// The processor's rated clock in MHz (0 if unknown): the first of the
    /// base clock's sources.
    machine_mhz: u32,
    /// What the last read counted (see [`Provider::counted`]).
    counted: Vec<CounterSet>,
}

impl CpuCounters {
    pub fn new() -> Self {
        Self::with_base_mhz(super::machine::base_mhz())
    }

    /// A provider of a processor rated at `machine_mhz`.
    pub fn with_base_mhz(machine_mhz: u32) -> Self {
        CpuCounters {
            machine_mhz,
            counters: imp::Counters::open(),
            name: imp::cpu_name(),
            counted: Vec::new(),
        }
    }

    /// The base clock in MHz: the machine's rated clock, else the counter's,
    /// else the power API's maximum (the one order, see `metrics`).
    #[allow(dead_code)] // the Windows reader and the tests use it
    fn base_clock(machine_mhz: u32, counter: Option<f64>, power_max: Option<f64>) -> Option<f64> {
        crate::metrics::base_clock_mhz(Some(machine_mhz as f64), counter, power_max)
    }
}

impl Provider for CpuCounters {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        self.counters.availability()
    }

    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        self.counters.sample(&self.name, self.machine_mhz, &mut self.counted).readings(at)
    }
}

/// A provider for tests: a made-up 4-core CPU ("Test CPU", base 4.70 GHz,
/// 110 % performance, core 3 parked, no limit, 80 % C1, 5 % C2, no C3),
/// never reading this PC.
#[cfg(test)]
pub fn made_up() -> super::provider::Factory {
    struct MadeUp;
    impl Provider for MadeUp {
        fn name(&self) -> &'static str {
            SOURCE
        }
        fn read(&mut self, at: Instant) -> Vec<Reading> {
            let core = |i: usize| CoreSample { current_mhz: Some(4_700.0), max_mhz: Some(5_100.0), parked: Some(i == 3), utilization_percent: Some(10.0), performance_limit_percent: Some(100.0), limit_flags: Some(0), c1_percent: Some(80.0), c2_percent: Some(5.0), c3_percent: Some(0.0) };
            let sample = CpuSample {
                name: "Test CPU".into(),
                base_mhz: Some(4_700.0),
                performance_percent: Some(110.0),
                utilization_percent: Some(10.0),
                cores: (0..4).map(core).collect(),
            };
            sample.readings(at)
        }
    }
    std::sync::Arc::new(|| Box::new(MadeUp))
}

/// The processor's marketing name, from the registry.
pub(super) fn cpu_name() -> String {
    imp::cpu_name()
}

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Frame, Group, Kind};

    #[test]
    fn the_processors_rated_clock_given_to_the_provider_is_the_first_base_clock_source() {
        let provider = CpuCounters::with_base_mhz(3_400);
        assert_eq!(CpuCounters::base_clock(provider.machine_mhz, Some(2_900.0), Some(3_000.0)), Some(3_400.0), "the machine's clock first");
        let unknown = CpuCounters::with_base_mhz(0);
        assert_eq!(CpuCounters::base_clock(unknown.machine_mhz, Some(2_900.0), Some(3_000.0)), Some(2_900.0), "then the counter");
        assert_eq!(CpuCounters::base_clock(unknown.machine_mhz, None, Some(3_000.0)), Some(3_000.0), "then the power API");
        assert_eq!(CpuCounters::base_clock(unknown.machine_mhz, None, None), None, "else unknown");
    }

    fn twelve_cores() -> CpuSample {
        CpuSample {
            name: "AMD Ryzen 5 7600X 6-Core Processor".into(),
            base_mhz: Some(4_700.0),
            performance_percent: Some(108.7),
            utilization_percent: Some(9.0),
            cores: (0..12)
                .map(|i| CoreSample { current_mhz: Some(4_700.0), max_mhz: Some(4_700.0), parked: Some(i >= 10), utilization_percent: Some(i as f64), ..Default::default() })
                .collect(),
        }
    }

    #[test]
    fn the_effective_clock_is_the_base_times_processor_performance() {
        let hz = |b, p| effective_hz(Some(b), Some(p)).unwrap();
        assert_eq!(hz(4_700.0, 100.0), 4.7e9);
        assert!((hz(4_700.0, 108.7) - 5.1089e9).abs() < 1e3, "boost above the base");
        assert_eq!(hz(4_700.0, 50.0), 2.35e9, "below the base");
    }

    #[test]
    fn core_average_is_the_effective_clock_of_the_package() {
        let frame = Frame::build(1, &[twelve_cores().readings(std::time::Instant::now())], Vec::new());
        let avg = frame.reading(CORE_AVERAGE).unwrap();
        assert_eq!(avg.name, "Core Average");
        assert!((avg.value.unwrap() - 5.1089e9).abs() < 1e3);
        assert_eq!(frame.reading(BASE_HZ).unwrap().value, Some(4.7e9));
    }

    #[test]
    fn each_logical_processor_gets_its_clock_max_clock_and_parked_rows() {
        let frame = Frame::build(1, &[twelve_cores().readings(std::time::Instant::now())], Vec::new());
        let cpu = &frame.devices[0];
        assert_eq!(cpu.device.name, "AMD Ryzen 5 7600X 6-Core Processor");
        let groups: Vec<_> = cpu.groups.iter().map(|g| g.group).collect();
        assert_eq!(groups, vec![Group::Utilization, Group::Clocks, Group::Limits, Group::IdleStates, Group::General]);
        let clocks = &cpu.groups[1].readings;
        for i in 0..12 {
            for row in [format!("CPU {i} Clock"), format!("CPU {i} Max Clock"), format!("CPU {i} Parked")] {
                assert!(clocks.iter().any(|r| r.name == row), "{row}");
            }
        }
        let parked = frame.reading("cpu.core11.parked").unwrap();
        assert_eq!((parked.text(), parked.kind), ("Parked".to_string(), Kind::Counter));
        assert_eq!(frame.reading("cpu.core0.parked").unwrap().text(), "Active");
        assert_eq!(frame.reading("cpu.core3.utilization_percent").unwrap().value, Some(3.0));
        assert_eq!(frame.reading("cpu.package.utilization_percent").unwrap().name, "CPU Total");
    }

    #[test]
    fn a_missing_counter_gives_an_unavailable_row_not_a_zero() {
        let mut s = twelve_cores();
        s.performance_percent = None;
        s.cores[2].parked = None;
        let frame = Frame::build(1, &[s.readings(std::time::Instant::now())], Vec::new());
        let avg = frame.reading(CORE_AVERAGE).unwrap();
        assert_eq!(avg.value, None);
        assert_eq!(avg.text(), "no counter on this PC");
        assert_eq!(frame.reading("cpu.core2.parked").unwrap().value, None);
    }

    fn limited(mut s: CpuSample) -> CpuSample {
        for (i, c) in s.cores.iter_mut().enumerate() {
            c.performance_limit_percent = Some(if i == 4 { 62.0 } else { 100.0 });
            c.limit_flags = Some(match i {
                4 => 0x2,
                7 => 0x1,
                _ => 0,
            });
        }
        s
    }

    #[test]
    fn performance_limit_flags_decode_into_words() {
        assert_eq!(limit_reasons(0), Vec::<String>::new());
        assert_eq!(limit_reasons(0x1), vec!["thermal"]);
        assert_eq!(limit_reasons(0x2), vec!["power"]);
        assert_eq!(limit_reasons(0x4), vec!["domain dependency"]);
        assert_eq!(limit_reasons(0x3), vec!["thermal", "power"]);
        assert_eq!(limit_reasons(0x7), vec!["thermal", "power", "domain dependency"]);
        assert_eq!(limit_reasons(0x10), vec!["unknown reason (bit 4)"]);
        assert_eq!(limit_reasons(0x8000_0002), vec!["power", "unknown reason (bit 31)"]);
    }

    #[test]
    fn the_limits_group_has_one_row_per_logical_processor_and_a_package_row() {
        let frame = Frame::build(1, &[limited(twelve_cores()).readings(std::time::Instant::now())], Vec::new());
        let cpu = &frame.devices[0];
        let limits = cpu.groups.iter().find(|g| g.group == Group::Limits).expect("a Limits group");
        assert_eq!(limits.group.name(), "Limits");
        for i in 0..12 {
            for row in [format!("CPU {i} Performance Limit"), format!("CPU {i} Limit Reasons")] {
                assert!(limits.readings.iter().any(|r| r.name == row), "{row}");
            }
        }
        assert_eq!(limits.readings.len(), 2 * 12 + 2);
        assert!(limits.readings.iter().all(|r| r.source == LIMITS_SOURCE), "Read from says CPU limits");
        let package = frame.reading(PERFORMANCE_LIMIT).unwrap();
        assert_eq!((package.name.as_str(), package.value), ("Package Performance Limit", Some(62.0)), "the worst value");
        assert_eq!(frame.reading(LIMIT_REASONS).unwrap().text(), "thermal, power", "every core's reasons");
        assert_eq!(frame.reading(LIMIT_REASONS).unwrap().kind, Kind::Counter, "a text state, no Min/Max");
        assert_eq!(frame.reading("cpu.core4.limits.performance_pct").unwrap().value, Some(62.0));
        assert_eq!(frame.reading("cpu.core4.limits.reasons").unwrap().text(), "power");
        assert_eq!(frame.reading("cpu.core0.limits.reasons").unwrap().text(), NO_LIMIT);
        let groups: Vec<_> = cpu.groups.iter().map(|g| g.group).collect();
        assert_eq!(groups, vec![Group::Utilization, Group::Clocks, Group::Limits, Group::IdleStates, Group::General]);
    }

    #[test]
    fn a_failing_limit_counter_greys_only_its_own_rows() {
        let mut s = limited(twelve_cores());
        s.cores.iter_mut().for_each(|c| c.limit_flags = None);
        let frame = Frame::build(1, &[s.readings(std::time::Instant::now())], Vec::new());
        assert_eq!(frame.reading(LIMIT_REASONS).unwrap().value, None);
        assert_eq!(frame.reading(LIMIT_REASONS).unwrap().text(), "no counter on this PC");
        assert_eq!(frame.reading("cpu.core3.limits.reasons").unwrap().value, None);
        assert_eq!(frame.reading(PERFORMANCE_LIMIT).unwrap().value, Some(62.0), "the other counter still reads");
        assert_eq!(frame.reading("cpu.core3.limits.performance_pct").unwrap().value, Some(100.0));
        assert!(frame.reading(CORE_AVERAGE).unwrap().value.is_some(), "the clocks still read");
    }

    #[test]
    fn the_limits_leave_the_existing_cpu_readings_unchanged() {
        let at = std::time::Instant::now();
        let shown = |s: CpuSample| -> Vec<(String, Option<f64>, String)> {
            s.readings(at).into_iter().filter(|r| r.group != Group::Limits && r.group != Group::IdleStates).map(|r| (r.id.clone(), r.value, r.text())).collect()
        };
        let before = shown(twelve_cores());
        assert_eq!(before.len(), 1 + 12 + 3 + 3 * 12);
        assert_eq!(shown(limited(twelve_cores())), before);
    }

    fn idling(mut s: CpuSample) -> CpuSample {
        for (i, c) in s.cores.iter_mut().enumerate() {
            c.c1_percent = Some(if i < 6 { 90.0 } else { 30.0 });
            c.c2_percent = Some(i as f64);
            c.c3_percent = Some(0.0);
        }
        s
    }

    #[test]
    fn the_idle_states_group_has_a_row_per_logical_processor_per_state_and_a_package_average() {
        let frame = Frame::build(1, &[idling(twelve_cores()).readings(std::time::Instant::now())], Vec::new());
        let cpu = &frame.devices[0];
        let idle = cpu.groups.iter().find(|g| g.group == Group::IdleStates).expect("an Idle states group");
        assert_eq!(idle.group.name(), "Idle states");
        for i in 0..12 {
            for state in ["C1", "C2", "C3"] {
                let row = format!("CPU {i} {state}");
                assert!(idle.readings.iter().any(|r| r.name == row), "{row}");
            }
        }
        assert_eq!(idle.readings.len(), 3 * 12 + 3);
        assert!(idle.readings.iter().all(|r| r.unit == Unit::Percent && r.kind == Kind::Measure), "percentages with statistics");
        let package = frame.reading(IDLE_C1).unwrap();
        assert_eq!((package.name.as_str(), package.value), ("Package C1", Some(60.0)), "the average of the cores");
        assert_eq!(frame.reading(IDLE_C2).unwrap().value, Some(5.5));
        assert_eq!(frame.reading(IDLE_C3).unwrap().value, Some(0.0));
        assert_eq!(frame.reading("cpu.core7.idle.c1_pct").unwrap().value, Some(30.0));
        assert_eq!(frame.reading("cpu.core7.idle.c2_pct").unwrap().value, Some(7.0));
        assert_eq!(frame.reading("cpu.core7.idle.c2_pct").unwrap().text(), "7 %");
        let groups: Vec<_> = cpu.groups.iter().map(|g| g.group).collect();
        assert_eq!(groups, vec![Group::Utilization, Group::Clocks, Group::Limits, Group::IdleStates, Group::General], "after Limits");
    }

    #[test]
    fn an_unavailable_idle_state_greys_with_a_reason() {
        let mut s = idling(twelve_cores());
        s.cores.iter_mut().for_each(|c| c.c3_percent = None);
        let frame = Frame::build(1, &[s.readings(std::time::Instant::now())], Vec::new());
        assert_eq!(frame.reading(IDLE_C3).unwrap().value, None);
        assert_eq!(frame.reading(IDLE_C3).unwrap().text(), "no counter on this PC");
        assert_eq!(frame.reading("cpu.core3.idle.c3_pct").unwrap().value, None);
        assert_eq!(frame.reading("cpu.core3.idle.c3_pct").unwrap().text(), "no counter on this PC");
        assert_eq!(frame.reading(IDLE_C1).unwrap().value, Some(60.0), "the other states still read");
    }

    #[test]
    fn the_idle_states_leave_the_existing_cpu_and_limits_readings_unchanged() {
        let at = std::time::Instant::now();
        let shown = |s: CpuSample| -> Vec<(String, Option<f64>, String)> {
            s.readings(at).into_iter().filter(|r| r.group != Group::IdleStates).map(|r| (r.id.clone(), r.value, r.text())).collect()
        };
        let before = shown(limited(twelve_cores()));
        assert_eq!(before.len(), 1 + 12 + 3 + 3 * 12 + 2 * 12 + 2);
        assert_eq!(shown(idling(limited(twelve_cores()))), before);
        assert_eq!(frame_value(&idling(limited(twelve_cores())), PERFORMANCE_LIMIT), Some(62.0));
        assert!((frame_value(&idling(limited(twelve_cores())), CORE_AVERAGE).unwrap() - 5.1089e9).abs() < 1e3);
    }

    fn frame_value(s: &CpuSample, id: &str) -> Option<f64> {
        Frame::build(1, &[s.readings(std::time::Instant::now())], Vec::new()).reading(id).and_then(|r| r.value)
    }

    #[test]
    fn processor_information_instances_name_group_and_number() {
        assert_eq!(logical_index("0,5"), Some(5));
        assert_eq!(logical_index("1,2"), Some(66));
        assert_eq!(logical_index("0,_Total"), None);
        assert_eq!(logical_index("_Total"), None);
    }

    /// Reads one frame of this PC and prints the tree.
    #[test]
    #[ignore]
    fn reads_this_pc() {
        let mut p = CpuCounters::new();
        assert_eq!(p.availability(), Ok(()));
        let readings = p.read(std::time::Instant::now());
        let frame = Frame::build(1, &[readings], Vec::new());
        for d in &frame.devices {
            println!("{}", d.device.name);
            for g in &d.groups {
                println!("  {}", g.group.name());
                for r in &g.readings {
                    println!("    {:<22} {}", r.name, r.text());
                }
            }
        }
        let logical = frame.readings().filter(|r| r.id.ends_with(".frequency.max_hz") && r.id.contains("core")).count();
        assert_eq!(logical, 12, "12 logical processors");
        let base = frame.reading(BASE_HZ).and_then(|r| r.value).unwrap();
        assert!((base - 4.7e9).abs() < 0.05e9, "base 4.70 GHz, got {base}");
        assert!(frame.reading(CORE_AVERAGE).and_then(|r| r.value).unwrap() > 0.0, "Core Average above zero");
        let limits = frame.readings().filter(|r| r.id.ends_with(".limits.performance_pct") && r.id.contains("core") && r.value.is_some()).count();
        assert_eq!(limits, 12, "a performance limit per logical processor");
        assert!(frame.reading(LIMIT_REASONS).and_then(|r| r.value).is_some(), "the package's limit reasons read");
        let idle = frame.readings().filter(|r| r.id.contains("core") && r.id.contains(".idle.")).count();
        assert_eq!(idle, 3 * 12, "C1 / C2 / C3 per logical processor");
        assert!(frame.reading(IDLE_C1).and_then(|r| r.value).is_some(), "the package's C1 reads");
    }
}
