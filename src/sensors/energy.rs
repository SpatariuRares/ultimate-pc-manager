//! The Energy Meter provider: Windows' `\Energy Meter(*)\Power` counters
//! (milliwatts). RAPL channels: the package (`_pkg`), the rail of each core
//! (`_coreN_core`), all cores (`_pp0`), the integrated GPU (`_pp1`), DRAM
//! (`_dram`), the platform (`_psys`) and the SoC (`_soc`).
//!
//! This PC › Powers always lists System, CPU, GPU, DRAM and SoC: a channel
//! this PC lacks is an unavailable row with its reason (another provider,
//! e.g. NVML for the GPU, can fill the same metric id later).

use std::time::Instant;

use super::model::{CounterSet, Device, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Energy Meter";
/// The metric of the CPU package power.
pub const PACKAGE: &str = "cpu.package.power_w";
/// Why a This PC power is missing when Energy Meter has no such channel.
pub const NO_CHANNEL: &str = "no provider on this PC";
/// Why every This PC power is missing when there is no Energy Meter.
pub const NO_METER: &str = "no Energy Meter on this PC";

/// One Energy Meter channel, from its instance name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Channel {
    Package,
    Core(usize),
    Cores,
    Gpu,
    Dram,
    System,
    Soc,
}

/// The channel of an instance ("rapl_package0_core3_core" → core 3);
/// `None` for `_total` and channels this app does not show.
fn channel(instance: &str) -> Option<Channel> {
    let n = instance.to_ascii_lowercase();
    let rest = n.strip_prefix("rapl_package0_")?;
    Some(match rest {
        "pkg" => Channel::Package,
        "pp0" => Channel::Cores,
        "pp1" => Channel::Gpu,
        "dram" => Channel::Dram,
        "psys" => Channel::System,
        "soc" => Channel::Soc,
        _ => Channel::Core(rest.strip_prefix("core")?.strip_suffix("_core")?.parse().ok()?),
    })
}

/// The readings of one read: `channels` are `(instance, milliwatts)`,
/// `cpu_name` the CPU device's name (shared with the Windows counters).
pub fn readings(channels: &[(String, f64)], cpu_name: &str, at: Instant) -> Vec<Reading> {
    let watts = |c: Channel| channels.iter().find(|(i, _)| channel(i) == Some(c)).map(|(_, mw)| mw / 1000.0);
    let cpu = Device::cpu(cpu_name);
    let cpu_row = |id: &str, name: &str, w: f64| Reading::measure(id, cpu.clone(), Group::Powers, name, Unit::Watt, SOURCE, at).with(Some(w), "");
    let mut out = Vec::new();
    let why = if channels.iter().any(|(i, _)| channel(i).is_some()) { NO_CHANNEL } else { NO_METER };
    for (key, name, c) in [
        ("system", "System", Channel::System),
        ("cpu", "CPU", Channel::Package),
        ("gpu", "GPU", Channel::Gpu),
        ("dram", "DRAM", Channel::Dram),
        ("soc", "SoC", Channel::Soc),
    ] {
        let r = Reading::measure(&format!("pc.power.{key}_w"), Device::this_pc(), Group::Powers, name, Unit::Watt, SOURCE, at);
        out.push(r.with(watts(c), why));
    }
    if let Some(w) = watts(Channel::Package) {
        out.push(cpu_row(PACKAGE, "CPU Package", w));
    }
    if let Some(w) = watts(Channel::Cores) {
        out.push(cpu_row("cpu.package.cores_power_w", "Cores", w));
    }
    let mut cores: Vec<(usize, f64)> = channels.iter().filter_map(|(i, mw)| match channel(i) {
        Some(Channel::Core(n)) => Some((n, mw / 1000.0)),
        _ => None,
    }).collect();
    cores.sort_by_key(|c| c.0);
    for (n, w) in cores {
        out.push(cpu_row(&format!("cpu.core{n}.power_w"), &format!("Core {n}"), w));
    }
    out
}

/// The Energy Meter provider. Built on its worker thread (holds a PDH query).
pub struct EnergyMeter {
    #[cfg(windows)]
    query: Option<super::pdh::Query>,
    name: String,
    /// The channels of the last read, in milliwatts (see [`Provider::counted`]).
    counted: Vec<CounterSet>,
}

impl EnergyMeter {
    pub fn new() -> Self {
        EnergyMeter {
            #[cfg(windows)]
            query: super::pdh::Query::open(&[(crate::perfdata::counter::POWER, r"\Energy Meter(*)\Power")]),
            name: super::cpu::cpu_name(),
            counted: Vec::new(),
        }
    }
}

impl Provider for EnergyMeter {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        return match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        };
        #[cfg(not(windows))]
        Err("Windows only".into())
    }

    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        #[cfg(windows)]
        let set = match self.query.as_mut() {
            Some(q) => {
                q.collect();
                q.counted(crate::perfdata::counter::POWER, false)
            }
            None => CounterSet::new(crate::perfdata::counter::POWER, Vec::new()),
        };
        #[cfg(not(windows))]
        let set = CounterSet::new(crate::perfdata::counter::POWER, Vec::new());
        let channels: Vec<(String, f64)> = set.instances.iter().map(|i| (i.name.clone(), i.value)).collect();
        self.counted = vec![set];
        readings(&channels, &self.name, at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{DeviceKind, Frame, Group};
    use std::time::Instant;

    /// What this PC's Energy Meter gives (AMD Ryzen 5 7600X, milliwatts).
    fn ryzen() -> Vec<(String, f64)> {
        let mut v: Vec<(String, f64)> = (0..6).map(|i| (format!("rapl_package0_core{i}_core"), 1_000.0 * (i + 1) as f64)).collect();
        v.push(("rapl_package0_pkg".into(), 47_000.02));
        v.push(("_total".into(), 0.0));
        v
    }

    fn frame(channels: &[(String, f64)]) -> Frame {
        Frame::build(1, &[readings(channels, "AMD Ryzen 5 7600X", Instant::now())], Vec::new())
    }

    fn powers<'a>(frame: &'a Frame, kind: DeviceKind) -> Vec<(&'a str, String)> {
        let d = frame.devices.iter().find(|d| d.device.kind == kind).unwrap();
        let g = d.groups.iter().find(|g| g.group == Group::Powers).unwrap();
        g.readings.iter().map(|r| (r.name.as_str(), r.text())).collect()
    }

    #[test]
    fn the_cpu_gets_the_package_and_each_core_in_watts() {
        let f = frame(&ryzen());
        let cpu = powers(&f, DeviceKind::Cpu);
        assert_eq!(cpu[0], ("CPU Package", "47.0 W".to_string()));
        assert_eq!(cpu[1], ("Core 0", "1.0 W".to_string()));
        assert_eq!(cpu[6], ("Core 5", "6.0 W".to_string()));
        assert_eq!(cpu.len(), 7, "no row for _total");
        assert_eq!(f.reading(PACKAGE).unwrap().source, SOURCE);
    }

    #[test]
    fn the_cpu_rows_join_the_cpu_of_the_windows_counters() {
        let f = frame(&ryzen());
        let cpu = f.devices.iter().find(|d| d.device.kind == DeviceKind::Cpu).unwrap();
        assert_eq!(cpu.device, crate::sensors::model::Device::cpu("AMD Ryzen 5 7600X"));
    }

    #[test]
    fn this_pc_powers_list_system_cpu_gpu_dram_soc_and_explain_the_missing_ones() {
        let f = frame(&ryzen());
        assert_eq!(f.devices[0].device.kind, DeviceKind::ThisPc);
        let pc = powers(&f, DeviceKind::ThisPc);
        let names: Vec<_> = pc.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, vec!["System", "CPU", "GPU", "DRAM", "SoC"]);
        assert_eq!(pc[1].1, "47.0 W");
        for (name, text) in [&pc[0], &pc[2], &pc[3], &pc[4]] {
            assert_eq!(text, NO_CHANNEL, "{name}");
            assert_eq!(f.reading(&format!("pc.power.{}_w", name.to_lowercase())).unwrap().value, None, "{name} is never a zero");
        }
    }

    #[test]
    fn intel_channels_fill_cores_gpu_dram_and_system() {
        let mw = |n: &str, v: f64| (n.to_string(), v);
        let f = frame(&[
            mw("RAPL_Package0_PKG", 30_000.0),
            mw("RAPL_Package0_PP0", 20_000.0),
            mw("RAPL_Package0_PP1", 3_000.0),
            mw("RAPL_Package0_DRAM", 2_500.0),
            mw("RAPL_Package0_PSYS", 60_000.0),
        ]);
        let pc = powers(&f, DeviceKind::ThisPc);
        assert_eq!(pc[0], ("System", "60.0 W".to_string()));
        assert_eq!(pc[2], ("GPU", "3.0 W".to_string()));
        assert_eq!(pc[3], ("DRAM", "2.5 W".to_string()));
        assert_eq!(pc[4].1, NO_CHANNEL);
        let cpu = powers(&f, DeviceKind::Cpu);
        assert_eq!(cpu, vec![("CPU Package", "30.0 W".to_string()), ("Cores", "20.0 W".to_string())]);
    }

    #[test]
    fn a_pc_without_energy_meter_shows_this_pc_powers_unavailable_and_no_cpu_rows() {
        let f = frame(&[]);
        assert!(f.devices.iter().all(|d| d.device.kind == DeviceKind::ThisPc));
        let pc = powers(&f, DeviceKind::ThisPc);
        assert_eq!(pc.len(), 5);
        assert!(pc.iter().all(|(_, t)| t == NO_METER));
    }

    #[test]
    fn a_soc_channel_fills_the_soc_row() {
        let f = frame(&[("rapl_package0_soc".into(), 5_260.0)]);
        assert_eq!(f.reading("pc.power.soc_w").unwrap().text(), "5.3 W");
    }

    /// Reads this PC's Energy Meter and prints the Powers rows.
    #[test]
    #[ignore]
    fn reads_this_pc_energy_meter() {
        let mut p = EnergyMeter::new();
        assert_eq!(p.availability(), Ok(()));
        let f = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        for r in f.readings() {
            println!("{:<12} {:<12} {}", r.device.name, r.name, r.text());
        }
        assert!(f.reading(PACKAGE).and_then(|r| r.value).unwrap() > 0.0, "package power above zero");
        assert!(f.reading("cpu.core0.power_w").and_then(|r| r.value).is_some(), "core rails");
    }
}
