//! NPUs (pure): the readings of each NPU DXCore lists, or the single
//! "No NPU in this PC" row when it lists none.

use std::time::{Duration, Instant};

use super::model::{Device, DeviceKind, Frame, Group, Reading, Unit};

/// The NPU device's only row when DXCore lists no NPU.
pub const NO_NPU: &str = "No NPU in this PC";

/// One NPU as read. `None` = not reported.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NpuSample {
    pub name: String,
    pub utilization_percent: Option<f64>,
    pub power_w: Option<f64>,
    /// Energy used over the last interval (joules).
    pub energy_j: Option<f64>,
    pub temperature_c: Option<f64>,
    pub engine_hz: Option<f64>,
    pub memory_hz: Option<f64>,
}

/// The NPU's metric id: `npu.<key>.<what>` (keys `npu0`, `npu1`...).
pub fn metric(device: &Device, what: &str) -> String {
    format!("npu.{}.{what}", device.key)
}

/// Every NPU's readings, from `source`; a missing value is grey with `why`.
/// No NPU: one NPU device with the single row [`NO_NPU`].
pub fn readings(npus: &[NpuSample], source: &'static str, why: &str, at: Instant) -> Vec<Reading> {
    if npus.is_empty() {
        let device = Device::new(DeviceKind::Npu, "npu", "NPU");
        let none = Unit::YesNo { yes: "Present", no: "None" };
        return vec![Reading::measure("npu.present", device, Group::General, "NPU", none, source, at).with(None, NO_NPU).counter()];
    }
    npus.iter()
        .enumerate()
        .flat_map(|(i, s)| {
            let device = Device::new(DeviceKind::Npu, &format!("npu{i}"), &s.name);
            let rows: [(&str, Group, &str, Unit, Option<f64>); 6] = [
                ("temperature_c", Group::Temperatures, "NPU Temperature", Unit::Celsius, s.temperature_c),
                ("utilization_percent", Group::Utilization, "NPU Load", Unit::Percent, s.utilization_percent),
                ("clock.engine_hz", Group::Clocks, "Engine Clock", Unit::Hz, s.engine_hz),
                ("clock.memory_hz", Group::Clocks, "Memory Clock", Unit::Hz, s.memory_hz),
                ("power_w", Group::Powers, "NPU Power", Unit::Watt, s.power_w),
                ("energy_j", Group::Powers, "Energy (interval)", Unit::Joule, s.energy_j),
            ];
            rows.into_iter()
                .map(|(what, group, name, unit, value)| {
                    Reading::measure(&metric(&device, what), device.clone(), group, name, unit, source, at).with(value, why)
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

/// The NPUs a frame holds, read back from their readings in the order
/// DXCore listed them (`npu0`, `npu1`...); empty when it lists none.
pub fn in_frame(frame: &Frame) -> Vec<NpuSample> {
    let value = |device: &Device, what: &str| frame.reading(&metric(device, what)).and_then(|r| r.value);
    let mut npus: Vec<(usize, NpuSample)> = frame
        .devices
        .iter()
        .filter(|d| d.device.kind == DeviceKind::Npu)
        .filter_map(|d| {
            let index = d.device.key.strip_prefix("npu")?.parse::<usize>().ok()?;
            let device = &d.device;
            let sample = NpuSample {
                name: device.name.clone(),
                utilization_percent: value(device, "utilization_percent"),
                power_w: value(device, "power_w"),
                energy_j: value(device, "energy_j"),
                temperature_c: value(device, "temperature_c"),
                engine_hz: value(device, "clock.engine_hz"),
                memory_hz: value(device, "clock.memory_hz"),
            };
            Some((index, sample))
        })
        .collect();
    npus.sort_by_key(|(i, _)| *i);
    npus.into_iter().map(|(_, s)| s).collect()
}

/// How busy an engine was, from its running time (microseconds) at two
/// reads `elapsed` apart: 0..=100 %. `None` without a previous read.
pub fn busy_percent(before_us: Option<u64>, now_us: u64, elapsed: Duration) -> Option<f64> {
    let ran = now_us.checked_sub(before_us?)?;
    let span = elapsed.as_micros();
    (span > 0).then(|| (ran as f64 / span as f64 * 100.0).min(100.0))
}

/// Energy used over an interval at `power_w` (joules).
pub fn interval_energy_j(power_w: Option<f64>, elapsed: Duration) -> Option<f64> {
    (!elapsed.is_zero()).then(|| power_w.map(|w| w * elapsed.as_secs_f64())).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(npus: &[NpuSample]) -> Frame {
        Frame::build(1, &[readings(npus, "DXCore", "DXCore does not report it", Instant::now())], vec![])
    }

    fn text(f: &Frame, id: &str) -> String {
        f.reading(id).unwrap_or_else(|| panic!("no reading {id}")).text()
    }

    #[test]
    fn no_npu_shows_a_single_row_saying_so() {
        let f = frame(&[]);
        let npu = f.devices.iter().find(|d| d.device.kind == DeviceKind::Npu).expect("the NPU device is listed");
        let rows: Vec<String> = npu.groups.iter().flat_map(|g| &g.readings).map(|r| r.text()).collect();
        assert_eq!(rows, vec![NO_NPU.to_string()]);
    }

    #[test]
    fn an_npu_shows_utilization_power_energy_temperature_and_clocks() {
        let npu = NpuSample {
            name: "Intel(R) AI Boost".into(),
            utilization_percent: Some(37.0),
            power_w: Some(2.5),
            energy_j: Some(5.0),
            temperature_c: Some(48.0),
            engine_hz: Some(1_400e6),
            memory_hz: Some(800e6),
        };
        let f = frame(&[npu]);
        let device = &f.devices.iter().find(|d| d.device.kind == DeviceKind::Npu).unwrap().device;
        assert_eq!((device.key.as_str(), device.name.as_str()), ("npu0", "Intel(R) AI Boost"));
        assert_eq!(text(&f, "npu.npu0.utilization_percent"), "37 %");
        assert_eq!(text(&f, "npu.npu0.power_w"), "2.5 W");
        assert_eq!(text(&f, "npu.npu0.energy_j"), "5.0 J");
        assert_eq!(text(&f, "npu.npu0.temperature_c"), "48.0 °C");
        assert_eq!(text(&f, "npu.npu0.clock.engine_hz"), "1.40 GHz");
        assert_eq!(text(&f, "npu.npu0.clock.memory_hz"), "800 MHz");
    }

    #[test]
    fn what_the_npu_does_not_report_is_grey_with_the_reason() {
        let f = frame(&[NpuSample { name: "NPU".into(), utilization_percent: Some(0.0), ..Default::default() }]);
        let power = f.reading("npu.npu0.power_w").unwrap();
        assert_eq!(power.value, None);
        assert_eq!(power.unavailable.as_deref(), Some("DXCore does not report it"));
        assert_eq!(text(&f, "npu.npu0.utilization_percent"), "0 %");
    }

    #[test]
    fn the_npus_come_back_from_a_frame_in_order() {
        let a = NpuSample { name: "Intel(R) AI Boost".into(), utilization_percent: Some(37.0), temperature_c: Some(48.0), engine_hz: Some(1_400e6), ..Default::default() };
        let b = NpuSample { name: "Second NPU".into(), utilization_percent: Some(5.0), ..Default::default() };
        let back = in_frame(&frame(&[a.clone(), b.clone()]));
        assert_eq!(back.len(), 2);
        assert_eq!((back[0].name.as_str(), back[0].utilization_percent, back[0].temperature_c, back[0].engine_hz), ("Intel(R) AI Boost", Some(37.0), Some(48.0), Some(1_400e6)));
        assert_eq!((back[1].name.as_str(), back[1].utilization_percent, back[1].engine_hz), ("Second NPU", Some(5.0), None));
    }

    #[test]
    fn a_frame_without_an_npu_gives_none() {
        assert!(in_frame(&frame(&[])).is_empty());
        assert!(in_frame(&Frame::build(1, &[], vec![])).is_empty());
    }

    #[test]
    fn busy_percent_is_running_time_over_elapsed_time() {
        let second = Duration::from_secs(1);
        assert_eq!(busy_percent(None, 500_000, second), None);
        assert_eq!(busy_percent(Some(1_000_000), 1_250_000, second), Some(25.0));
        assert_eq!(busy_percent(Some(0), 3_000_000, second), Some(100.0), "never above 100 %");
        assert_eq!(busy_percent(Some(900), 100, second), None, "a counter that went back (driver reset)");
        assert_eq!(busy_percent(Some(0), 0, Duration::ZERO), None);
    }

    #[test]
    fn interval_energy_is_power_times_the_interval() {
        assert_eq!(interval_energy_j(Some(2.5), Duration::from_secs(2)), Some(5.0));
        assert_eq!(interval_energy_j(None, Duration::from_secs(2)), None);
        assert_eq!(interval_energy_j(Some(2.5), Duration::ZERO), None);
    }
}
