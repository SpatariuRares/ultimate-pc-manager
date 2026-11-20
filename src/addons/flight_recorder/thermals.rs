//! The Thermals panel (pure): the temperature sensors the shared sensor
//! service reports (the add-on reads none of its own), the hottest of them,
//! what a recording keeps of them, and the panel live or at a replay
//! position, in °C or °F as the app's setting says.

use super::layout::{PanelView, Series, Slot, Line, NO_VALUE};
use super::replay::Replay;
use super::trace::Panel;
use crate::sensors::model::{Group, Unit};
use crate::sensors::Frame;

/// Under the panel's cells (the approved prototype's text).
pub const NOTE: &str = "One cell per sensor; the hottest is ringed. Follows \u{00b0}C / \u{00b0}F.";

/// After the hottest cell's name (the approved prototype's "GPU (hottest)").
pub const HOTTEST: &str = " (hottest)";

/// The temperatures a trace may hold, in °C (as Replay checks them).
pub const BOUNDS: std::ops::RangeInclusive<f32> = -100.0..=250.0;

/// A temperature sensor with a reading: its metric id (the trace's key),
/// the name its cell shows and its value in °C.
#[derive(Clone, Debug, PartialEq)]
pub struct Sensor {
    pub key: String,
    pub name: String,
    pub celsius: f32,
}

/// The temperature sensors of `frame` that have a value, in the tree's
/// order; each named after its device and its reading.
pub fn sensors(frame: &Frame) -> Vec<Sensor> {
    frame
        .devices
        .iter()
        .flat_map(|d| d.groups.iter().filter(|g| g.group == Group::Temperatures).flat_map(move |g| g.readings.iter().map(move |r| (d, r))))
        .filter(|(_, r)| r.unit == Unit::Celsius)
        .filter_map(|(d, r)| {
            let celsius = r.value? as f32;
            BOUNDS.contains(&celsius).then(|| Sensor { key: r.id.clone(), name: format!("{}: {}", d.device.name, r.name), celsius })
        })
        .collect()
}

/// The hottest of `celsius` (the first one on a tie); None when empty.
pub fn hottest(celsius: &[f32]) -> Option<usize> {
    celsius.iter().enumerate().fold(None, |best: Option<(usize, f32)>, (i, &v)| match best {
        Some((_, b)) if b >= v => best,
        _ => Some((i, v)),
    })
    .map(|(i, _)| i)
}

/// What else a frame of the shared sensors gives the cores and GPU panels
/// (#35: dashed GPU clock, dotted temperature where it exists), and the
/// hottest of its temperatures; None where it reads nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Extras {
    pub cpu_temp_c: Option<f32>,
    pub gpu_temp_c: Option<f32>,
    pub gpu_clock_ghz: Option<f32>,
    pub hottest_c: Option<f32>,
}

/// The GPU temperature's metric, after `gpu.<key>.` (see [`crate::sensors::gpu`]).
const GPU_TEMPERATURE: &str = "temperature.gpu_c";

/// The [`Extras`] of `frame`: the CPU's first temperature, and the
/// temperature and graphics clock of the card named `gpu` (the one whose
/// engines the page shows), else of the first card that has them.
pub fn extras(frame: &Frame, gpu: Option<&str>) -> Extras {
    use crate::sensors::model::DeviceKind;
    let value = |r: &crate::sensors::model::Reading| r.value.map(|v| v as f32);
    let celsius = |r: &crate::sensors::model::Reading| value(r).filter(|v| r.unit == Unit::Celsius && BOUNDS.contains(v));
    let cpu_temp_c = frame
        .devices
        .iter()
        .filter(|d| d.device.kind == DeviceKind::Cpu)
        .flat_map(|d| d.groups.iter().filter(|g| g.group == Group::Temperatures).flat_map(|g| &g.readings))
        .find_map(celsius);
    let card = |d: &&crate::sensors::model::DeviceNode| {
        let of = |what: &str| {
            let id = format!("gpu.{}.{what}", d.device.key);
            d.groups.iter().flat_map(|g| &g.readings).find(|r| r.id == id)
        };
        (of(GPU_TEMPERATURE).and_then(celsius), of(crate::sensors::gpu::GRAPHICS_CLOCK).and_then(value).map(|hz| hz / 1e9))
    };
    let cards: Vec<_> = frame.devices.iter().filter(|d| d.device.kind == DeviceKind::Gpu).collect();
    let named = cards.iter().find(|d| gpu.is_some_and(|g| d.device.name == g)).map(card);
    let (gpu_temp_c, gpu_clock_ghz) = named.unwrap_or_else(|| cards.iter().map(card).find(|(t, c)| t.is_some() || c.is_some()).unwrap_or_default());
    let hottest_c = sensors(frame).iter().map(|s| s.celsius).reduce(f32::max);
    Extras { cpu_temp_c, gpu_temp_c, gpu_clock_ghz, hottest_c }
}

/// What a recording keeps of the sensors: the ones read at Record, and
/// each one's last value (kept when a later frame lacks it).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Feed {
    keys: Vec<String>,
    last: Vec<f32>,
    seq: Option<u64>,
}

impl Feed {
    /// A feed of `sensors`, starting from their values.
    pub fn new(sensors: &[Sensor]) -> Self {
        Feed { keys: sensors.iter().map(|s| s.key.clone()).collect(), last: sensors.iter().map(|s| s.celsius).collect(), seq: None }
    }

    /// Takes the values of a newer `frame` (a sensor it lacks, or out of
    /// bounds, keeps its last value).
    pub fn update(&mut self, frame: &Frame) {
        if self.seq == Some(frame.seq) {
            return;
        }
        self.seq = Some(frame.seq);
        for (key, last) in self.keys.iter().zip(&mut self.last) {
            if let Some(v) = frame.reading(key).and_then(|r| r.value).map(|v| v as f32).filter(|v| BOUNDS.contains(v)) {
                *last = v;
            }
        }
    }

    /// Each sensor's last value, in the order of the keys.
    pub fn values(&self) -> &[f32] {
        &self.last
    }
}

/// "1 sensor", "3 sensors".
pub fn sub(n: usize) -> String {
    if n == 1 {
        "1 sensor".into()
    } else {
        format!("{n} sensors")
    }
}

/// The panel for `names` at `celsius`, its line the hottest over time
/// (`history`, °C): the hottest's value at the top right, one cell per
/// sensor (the hottest named so). Empty with no sensor.
pub fn panel(names: &[String], celsius: &[f32], history: Vec<f32>, fahrenheit: bool) -> PanelView {
    let Some(top) = hottest(celsius) else {
        return PanelView { slot: Slot::Thermals, value: NO_VALUE.into(), series: Vec::new(), cells: Vec::new(), names: Vec::new(), sub: String::new(), note: String::new() };
    };
    let names = names.iter().enumerate().map(|(i, n)| if i == top { format!("{n}{HOTTEST}") } else { n.clone() }).collect();
    let peak = history.iter().copied().fold(celsius[top], f32::max);
    let max = if peak > 100.0 { peak * 1.1 } else { 100.0 };
    PanelView {
        slot: Slot::Thermals,
        value: Unit::Celsius.format_in(f64::from(celsius[top]), fahrenheit),
        series: vec![Series { line: Line::Solid, values: history, max }],
        cells: celsius.to_vec(),
        names,
        sub: sub(celsius.len()),
        note: NOTE.into(),
    }
}

/// The panel at `at_ms` of a replay, from the trace alone: its sensors'
/// names, their values at the position, and the hottest from 00:00 to
/// there (each sample's recorded hottest, else its hottest cell). None
/// when the trace holds no temperatures.
pub fn replay_panel(replay: &Replay, at_ms: u64, fahrenheit: bool) -> Option<PanelView> {
    let trace = replay.trace();
    let index = trace.header.panels.iter().position(|h| h.panel == Panel::Thermals)?;
    let now = replay.sample_at(at_ms).t_ms;
    let values = |s: &super::trace::Sample| s.panels.get(index).and_then(|p| p.series.first()).cloned().unwrap_or_default();
    let history = trace.samples.iter().take_while(|s| s.t_ms <= now).map(|s| s.hottest_c.unwrap_or_else(|| values(s).into_iter().fold(f32::MIN, f32::max))).collect();
    Some(panel(&trace.header.panels[index].cells, &values(replay.sample_at(at_ms)), history, fahrenheit))
}

#[cfg(test)]
mod tests {
    use super::super::trace::{Header, PanelHeader, PanelSample, Sample, Trace};
    use super::*;
    use crate::sensors::model::{Device, DeviceKind, Reading};
    use std::time::Instant;

    fn temperature(id: &str, device: &Device, name: &str, c: Option<f64>) -> Reading {
        Reading::measure(id, device.clone(), Group::Temperatures, name, Unit::Celsius, "test", Instant::now()).with(c, "not read")
    }

    /// A PC whose CPU package is at 61 °C, GPU at 49 °C, a disk without a
    /// value and a CPU clock (not a temperature).
    fn frame(seq: u64, cpu: Option<f64>, gpu: Option<f64>) -> Frame {
        let (c, g, d) = (Device::cpu("Ryzen 7"), Device::gpu("gpu0", "RTX 4080"), Device::new(DeviceKind::Disk, "disk0", "SSD"));
        let clock = Reading::measure("cpu.clock", c.clone(), Group::Clocks, "Core Clock", Unit::Celsius, "test", Instant::now()).with(Some(4.8), "");
        let readings = vec![
            temperature("cpu.temp", &c, "Package", cpu),
            clock,
            temperature("gpu0.temp", &g, "GPU Temperature", gpu),
            temperature("disk0.temp", &d, "Temperature", None),
        ];
        Frame::build(seq, &[readings], Vec::new())
    }

    #[test]
    fn the_sensors_are_the_temperatures_the_service_reports_with_a_value() {
        let found = sensors(&frame(1, Some(61.0), Some(49.0)));
        assert_eq!(
            found,
            [
                Sensor { key: "cpu.temp".into(), name: "Ryzen 7: Package".into(), celsius: 61.0 },
                Sensor { key: "gpu0.temp".into(), name: "RTX 4080: GPU Temperature".into(), celsius: 49.0 },
            ]
        );
        assert_eq!(sensors(&frame(1, Some(400.0), None)), [], "out of bounds is no reading");
    }

    #[test]
    fn the_hottest_is_the_highest_and_the_first_on_a_tie() {
        assert_eq!(hottest(&[49.0, 61.0, 30.0]), Some(1));
        assert_eq!(hottest(&[50.0, 50.0]), Some(0));
        assert_eq!(hottest(&[]), None);
    }

    /// The Thermals panel live after `frames`, one a sample.
    fn live(frames: &[Option<Frame>], fahrenheit: bool) -> PanelView {
        let mut history = super::super::layout::LiveHistory::default();
        for f in frames {
            history.push_sensors(f.as_ref());
        }
        let sample = crate::sampler::Snapshot { cpu_per_core: vec![1.0], ..Default::default() };
        super::super::layout::live_panels(&history, &sample, fahrenheit).into_iter().find(|p| p.slot == Slot::Thermals).expect("a Thermals panel")
    }

    #[test]
    fn live_there_is_a_cell_per_sensor_and_the_hottest_is_named_so() {
        let p = live(&[Some(frame(1, Some(55.0), Some(49.0))), Some(frame(2, Some(61.0), Some(49.0)))], false);
        assert_eq!(p.slot, Slot::Thermals);
        assert_eq!(p.cells, [61.0, 49.0]);
        assert_eq!(p.names, ["Ryzen 7: Package (hottest)", "RTX 4080: GPU Temperature"]);
        assert_eq!(p.value, "61.0 \u{00b0}C");
        assert_eq!(p.sub, "2 sensors");
        assert_eq!(p.note, "One cell per sensor; the hottest is ringed. Follows \u{00b0}C / \u{00b0}F.");
        assert_eq!(p.series, [Series { line: Line::Solid, values: vec![55.0, 61.0], max: 100.0 }], "the hottest over the last minute");
    }

    #[test]
    fn the_value_follows_the_fahrenheit_setting() {
        let p = live(&[Some(frame(1, Some(61.0), None))], true);
        assert_eq!(p.value, "141.8 \u{00b0}F");
        assert_eq!(p.sub, "1 sensor");
    }

    #[test]
    fn without_sensors_the_panel_stays_empty() {
        for p in [live(&[None], false), live(&[Some(frame(1, None, None))], false)] {
            assert_eq!(p.value, "\u{2013}");
            assert!(p.cells.is_empty() && p.series.is_empty() && p.sub.is_empty());
        }
    }

    #[test]
    fn a_recording_keeps_each_sensors_last_value() {
        let mut feed = Feed::new(&sensors(&frame(1, Some(61.0), Some(49.0))));
        assert_eq!(feed.values(), [61.0, 49.0]);
        feed.update(&frame(2, Some(70.0), None));
        assert_eq!(feed.values(), [70.0, 49.0], "the GPU's last value is kept");
        feed.update(&frame(3, Some(999.0), Some(52.0)));
        assert_eq!(feed.values(), [70.0, 52.0], "out of bounds keeps the last value");
    }

    /// A PC with a CPU at 58 °C and two cards: an integrated one first, the
    /// RTX 4080 at 2.52 GHz and 47 °C.
    fn two_card_frame() -> Frame {
        let (c, igpu, big) = (Device::cpu("Ryzen 7"), Device::gpu("igpu", "Radeon Graphics"), Device::gpu("rtx", "NVIDIA GeForce RTX 4080"));
        let hz = |id: &str, d: &Device, v: f64| Reading::measure(id, d.clone(), Group::Clocks, "Graphics Clock", Unit::Hz, "test", Instant::now()).with(Some(v), "");
        let readings = vec![
            temperature("cpu.package.temperature_c", &c, "Package", Some(58.0)),
            temperature("gpu.igpu.temperature.gpu_c", &igpu, "GPU Temperature", Some(40.0)),
            hz("gpu.igpu.clock.graphics_hz", &igpu, 0.4e9),
            temperature("gpu.rtx.temperature.gpu_c", &big, "GPU Temperature", Some(47.0)),
            hz("gpu.rtx.clock.graphics_hz", &big, 2.52e9),
        ];
        Frame::build(1, &[readings], Vec::new())
    }

    #[test]
    fn the_frame_gives_the_cpu_and_gpu_temperature_and_the_gpu_clock_of_the_card_shown() {
        let e = extras(&two_card_frame(), Some("NVIDIA GeForce RTX 4080"));
        assert_eq!(e, Extras { cpu_temp_c: Some(58.0), gpu_temp_c: Some(47.0), gpu_clock_ghz: Some(2.52), hottest_c: Some(58.0) });
        let first = extras(&two_card_frame(), None);
        assert_eq!((first.gpu_temp_c, first.gpu_clock_ghz), (Some(40.0), Some(0.4)), "without a GPU named, the first card");
    }

    #[test]
    fn a_frame_without_them_gives_none() {
        let e = extras(&frame(1, None, None), Some("RTX 4080"));
        assert_eq!(e, Extras::default());
        assert_eq!(extras(&frame(1, Some(61.0), Some(49.0)), None).hottest_c, Some(61.0));
    }

    /// A trace with two sensors, the second the hottest from 1000 ms on.
    fn trace() -> Trace {
        let mut t = Trace::new(Header::new("OTHER-PC", 0, 1000, 100, 1));
        t.header.panels.push(PanelHeader { panel: Panel::Thermals, cells: vec!["CPU".into(), "GPU".into()] });
        for (t_ms, cpu, gpu) in [(0, 50.0, 40.0), (1000, 52.0, 60.0), (2000, 55.0, 80.0)] {
            t.samples.push(Sample {
                t_ms,
                panels: vec![PanelSample { panel: Panel::PerformanceCores, series: vec![vec![1.0]] }, PanelSample { panel: Panel::Thermals, series: vec![vec![cpu, gpu]] }],
                memory_used_bytes: 1,
                hottest_c: None,
            });
        }
        t
    }

    #[test]
    fn a_replay_shows_the_traces_sensors_at_the_position() {
        let r = Replay::new(trace());
        let p = replay_panel(&r, 1000, false).expect("the trace holds temperatures");
        assert_eq!(p.names, ["CPU", "GPU (hottest)"]);
        assert_eq!(p.cells, [52.0, 60.0]);
        assert_eq!(p.value, "60.0 \u{00b0}C");
        assert_eq!(p.series[0].values, [50.0, 60.0], "the hottest from 00:00 to the position");
        assert_eq!(replay_panel(&r, 0, true).unwrap().value, "122.0 \u{00b0}F");
        let mut none = trace();
        none.header.panels.pop();
        assert_eq!(replay_panel(&Replay::new(none), 0, false), None);
    }

    #[test]
    fn a_replays_line_is_each_samples_hottest_as_recorded_when_the_trace_keeps_it() {
        let mut t = trace();
        // A sensor outside the Thermals cells ran hotter at 1000 ms.
        t.samples[1].hottest_c = Some(90.0);
        let p = replay_panel(&Replay::new(t), 2000, false).unwrap();
        assert_eq!(p.series[0].values, [50.0, 90.0, 80.0], "recorded where kept, else the hottest cell");
    }
}
