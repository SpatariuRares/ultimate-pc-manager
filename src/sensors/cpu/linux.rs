//! Linux: the processor's utilisation from `/proc/stat` and its clocks from
//! cpufreq (`/sys/devices/system/cpu/cpuN/cpufreq`), read through an
//! [`FsRoot`]. A processor without cpufreq shows no clock, never a zero.

use crate::availability::{unavailable_error, Feature};
use crate::fs_root::FsRoot;
use crate::sensors::cpu::{CoreSample, CpuCounters, CpuSample};
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::proc_stat::{self, Stat};

/// The processor query: the previous tallies, to say what happened since.
pub struct Counters {
    root: FsRoot,
    previous: Option<Stat>,
}

impl Counters {
    pub fn open() -> Self {
        Self::at(FsRoot::system())
    }

    pub fn at(root: FsRoot) -> Self {
        Counters { root, previous: None }
    }

    pub fn availability(&self) -> Result<(), String> {
        if self.root.probe("proc/stat") {
            Ok(())
        } else {
            Err(unavailable_error(Feature::HardwareSensors))
        }
    }

    /// A clock in MHz of the cpufreq file `name` of processor `n`.
    fn mhz(&self, n: usize, name: &str) -> Option<f64> {
        let khz: f64 = self.root.read(&format!("sys/devices/system/cpu/cpu{n}/cpufreq/{name}"))?.parse().ok()?;
        (khz > 0.0).then_some(khz / 1000.0)
    }

    /// What the processor does now: utilisation since the last sample (none
    /// on the first), every logical processor's clock and top clock, and the
    /// performance level against the base clock (also counted, once).
    pub fn sample(&mut self, name: &str, machine_mhz: u32, counted: &mut Vec<CounterSet>) -> CpuSample {
        let mut s = CpuSample { name: name.to_string(), ..Default::default() };
        let Some(now) = proc_stat::read(&self.root) else { return s };
        let before = self.previous.replace(now.clone());
        let usage = |was: Option<&proc_stat::Times>, is: &proc_stat::Times| was.and_then(|w| is.since(w)).map(|u| u.busy);
        s.utilization_percent = usage(before.as_ref().map(|b| &b.total), &now.total);
        for (n, times) in &now.cores {
            let was = before.as_ref().and_then(|b| b.cores.iter().find(|c| c.0 == *n)).map(|c| &c.1);
            if s.cores.len() <= *n {
                s.cores.resize(*n + 1, CoreSample::default());
            }
            s.cores[*n] = CoreSample { current_mhz: self.mhz(*n, "scaling_cur_freq"), max_mhz: self.mhz(*n, "cpuinfo_max_freq"), utilization_percent: usage(was, times), ..Default::default() };
        }
        let top = s.cores.iter().filter_map(|c| c.max_mhz).reduce(f64::max);
        s.base_mhz = CpuCounters::base_clock(machine_mhz, None, top);
        let current: Vec<f64> = s.cores.iter().filter_map(|c| c.current_mhz).collect();
        if let (Some(base), false) = (s.base_mhz, current.is_empty()) {
            let level = current.iter().sum::<f64>() / current.len() as f64 / base * 100.0;
            s.performance_percent = Some(level);
            *counted = vec![CounterSet::new(crate::perfdata::counter::PERF, vec![Instance::new("_Total", level)])];
        }
        s
    }
}

/// The processor's marketing name, from `/proc/cpuinfo`.
pub fn cpu_name() -> String {
    FsRoot::system().read("proc/cpuinfo").map(|t| crate::cpu_topology::cpu_name_from(&t)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    /// The tallies an instant before the fixture's.
    fn earlier() -> Stat {
        proc_stat::parse("cpu  100 0 50 800 50 0 0 0\ncpu0 30 0 15 200 10 0 0 0\ncpu1 30 0 15 200 10 0 0 0\ncpu2 20 0 10 200 15 0 0 0\ncpu3 20 0 10 200 15 0 0 0\n").unwrap()
    }

    #[test]
    fn the_first_sample_has_clocks_but_no_utilisation() {
        let mut c = Counters::at(fixture("cpu-modern"));
        let mut counted = Vec::new();
        let s = c.sample("CPU", 2400, &mut counted);
        assert_eq!(s.utilization_percent, None);
        assert_eq!(s.cores.len(), 4);
        assert_eq!((s.cores[0].current_mhz, s.cores[0].max_mhz), (Some(3000.0), Some(4800.0)));
        assert_eq!((s.cores[3].current_mhz, s.cores[3].max_mhz), (Some(1800.0), Some(3600.0)));
    }

    #[test]
    fn a_later_sample_says_how_busy_each_processor_was() {
        let mut c = Counters::at(fixture("cpu-modern"));
        c.previous = Some(earlier());
        let s = c.sample("CPU", 2400, &mut Vec::new());
        // 180 ticks passed in total, 120 of them user or kernel work: 67 %.
        assert_eq!(s.utilization_percent.map(f64::round), Some(67.0));
        assert!(s.cores.iter().all(|c| c.utilization_percent.is_some()));
    }

    #[test]
    fn the_performance_level_is_the_average_clock_over_the_base_clock() {
        let mut counted = Vec::new();
        let s = Counters::at(fixture("cpu-modern")).sample("CPU", 2400, &mut counted);
        assert_eq!(s.base_mhz, Some(2400.0));
        assert_eq!(counted.len(), 1);
        assert_eq!(counted[0].key, crate::perfdata::counter::PERF);
        assert_eq!(counted[0].instance("_Total").map(|i| i.value), Some(100.0));
    }

    #[test]
    fn a_processor_without_cpufreq_shows_no_clock_and_no_performance_level() {
        let mut counted = Vec::new();
        let s = Counters::at(fixture("cpu-old")).sample("CPU", 0, &mut counted);
        assert_eq!((s.cores[1].current_mhz, s.cores[1].max_mhz), (None, None));
        assert_eq!(s.cores[0].current_mhz, Some(2800.0));
        assert_eq!(s.base_mhz, Some(2800.0));
    }

    #[test]
    fn a_pc_without_proc_stat_is_not_available_and_gives_an_empty_sample() {
        let mut c = Counters::at(fixture("desktop"));
        assert!(c.availability().is_err());
        let mut counted = Vec::new();
        assert_eq!(c.sample("CPU", 0, &mut counted).cores.len(), 0);
        assert!(counted.is_empty());
        assert!(Counters::at(fixture("cpu-old")).availability().is_ok());
    }
}
