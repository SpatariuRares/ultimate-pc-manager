//! The telemetry frame (pure): stable identities for devices, groups and
//! readings, and the merge of what several providers read into one tree
//! This PC › device › group › reading.
//!
//! A provider adds a device kind, a group or a unit here only when it is
//! the first to need one: one enum line each.

use std::time::Instant;

/// What a device is; its place among the devices under This PC.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum DeviceKind {
    /// The PC itself (system power, power source): the root of the tree.
    ThisPc,
    Cpu,
    Gpu,
    Npu,
    Motherboard,
    Memory,
    Disk,
    Battery,
    ThermalZone,
}

/// A device: its kind, a key stable across frames (e.g. `disk0`) and the
/// name shown in the tree.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Device {
    pub kind: DeviceKind,
    pub key: String,
    pub name: String,
}

impl Device {
    pub fn new(kind: DeviceKind, key: &str, name: &str) -> Self {
        Device { kind, key: key.into(), name: name.into() }
    }

    /// The root of the tree (system power, power source, thermal state).
    pub fn this_pc() -> Self {
        Self::new(DeviceKind::ThisPc, "pc", "This PC")
    }

    /// The CPU package (one per PC here), named "CPU" when `name` is empty.
    /// Every provider builds it here so their rows merge under one device.
    pub fn cpu(name: &str) -> Self {
        Self::new(DeviceKind::Cpu, "cpu", if name.is_empty() { "CPU" } else { name })
    }

    /// A video card (`gpu0`, `gpu1`... in DXCore's order).
    pub fn gpu(key: &str, name: &str) -> Self {
        Self::new(DeviceKind::Gpu, key, name)
    }
}

/// A group of readings under a device. The order of the variants is the
/// display order (CPU: Temperatures, Utilization, Clocks, Powers;
/// disks: Temperatures, Utilization, Speed, S.M.A.R.T.).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Group {
    Temperatures,
    Utilization,
    Clocks,
    /// CPU performance limit and its reasons.
    Limits,
    /// CPU idle-state (C1 / C2 / C3) residency.
    IdleStates,
    Speed,
    Powers,
    Voltages,
    Levels,
    Smart,
    General,
    /// Fan speeds (motherboard).
    Fans,
}

impl Group {
    /// The name shown in the tree.
    pub fn name(self) -> &'static str {
        match self {
            Group::Temperatures => "Temperatures",
            Group::Utilization => "Utilization",
            Group::Clocks => "Clocks",
            Group::Limits => "Limits",
            Group::IdleStates => "Idle states",
            Group::Speed => "Speed",
            Group::Powers => "Powers",
            Group::Voltages => "Voltages",
            Group::Levels => "Levels (Energy Perf)",
            Group::Smart => "S.M.A.R.T.",
            Group::General => "General",
            Group::Fans => "Fans",
        }
    }
}

/// The unit of a reading's value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Hertz, shown as GHz.
    Hz,
    Percent,
    /// Watts.
    Watt,
    /// Joules (energy over an interval).
    Joule,
    /// Bytes per second, shown as MB/s below 1 GB/s, GB/s above (decimal).
    BytesPerSec,
    /// 1 = yes, 0 = no, shown with the reading's own words.
    YesNo { yes: &'static str, no: &'static str },
    /// Words (a plan's name, a mode): the reading's `words`, its value is 1.
    Text,
    /// Degrees Celsius, shown in °C or °F (the app's setting).
    Celsius,
    /// Hours (Power On Hours), with thousands separators.
    Hours,
    /// Bytes (Data Read), shown as GB below 1 TB, TB above (decimal).
    Bytes,
    /// A plain count (commands, power cycles), with thousands separators.
    Count,
    /// Seconds, shown as "1 h 30 min".
    Seconds,
    /// 0, 1, 2... shown as the word at that index.
    State(&'static [&'static str]),
    /// Volts (board voltages).
    Volt,
    /// Revolutions per minute (fans).
    Rpm,
}

impl Unit {
    /// A value in this unit, as shown in the tree (temperatures in °C).
    pub fn format(self, v: f64) -> String {
        self.format_in(v, false)
    }

    /// A value in this unit, temperatures in °F when `fahrenheit`.
    pub fn format_in(self, v: f64, fahrenheit: bool) -> String {
        match self {
            Unit::Hz if v >= 1e9 => format!("{:.2} GHz", v / 1e9),
            Unit::Hz => format!("{:.0} MHz", v / 1e6),
            Unit::Text => String::new(),
            Unit::Percent => format!("{v:.0} %"),
            Unit::Watt => format!("{v:.1} W"),
            Unit::Joule => format!("{v:.1} J"),
            Unit::BytesPerSec if v >= 1e9 => format!("{:.2} GB/s", v / 1e9),
            Unit::BytesPerSec => format!("{:.1} MB/s", v / 1e6),
            Unit::YesNo { yes, no } => (if v != 0.0 { yes } else { no }).to_string(),
            Unit::Celsius if fahrenheit => format!("{:.1} °F", fahrenheit_of(v)),
            Unit::Celsius => format!("{v:.1} °C"),
            Unit::Bytes if v >= 1e12 => format!("{:.2} TB", v / 1e12),
            Unit::Bytes => format!("{:.1} GB", v / 1e9),
            Unit::Hours => format!("{} h", thousands(v)),
            Unit::Count => thousands(v),
            Unit::Seconds => {
                let m = (v.max(0.0) / 60.0).round() as u64;
                if m >= 60 {
                    format!("{} h {} min", m / 60, m % 60)
                } else {
                    format!("{m} min")
                }
            }
            Unit::State(words) => words.get(v.max(0.0) as usize).copied().unwrap_or("Unknown").to_string(),
            Unit::Volt => format!("{v:.3} V"),
            Unit::Rpm => format!("{} RPM", thousands(v)),
        }
    }
}

/// A temperature in °C as °F.
pub fn fahrenheit_of(celsius: f64) -> f64 {
    celsius * 9.0 / 5.0 + 32.0
}

/// A whole number with comma thousands separators ("1,234,567").
pub fn thousands(v: f64) -> String {
    let digits = format!("{:.0}", v.abs());
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if v.round() < 0.0 {
        out.insert(0, '-');
    }
    out
}

/// A measure has Min/Max; a counter (or a state) shows only its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Measure,
    Counter,
}

/// One reading of one metric.
#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    /// Stable metric id, e.g. `cpu.package.frequency.average_effective_hz`.
    pub id: String,
    pub device: Device,
    pub group: Group,
    pub name: String,
    pub unit: Unit,
    /// `None` = unavailable (never shown as zero).
    pub value: Option<f64>,
    /// Why the value is missing ("needs administrator", ...).
    pub unavailable: Option<String>,
    /// The provider it came from ("Windows counters", ...).
    pub source: &'static str,
    pub kind: Kind,
    /// When it was last read.
    pub at: Instant,
    /// The words of a [`Unit::Text`] reading.
    pub words: Option<String>,
}

impl Reading {
    /// A measure (with Min/Max) and no value yet.
    pub fn measure(id: &str, device: Device, group: Group, name: &str, unit: Unit, source: &'static str, at: Instant) -> Self {
        Reading { id: id.into(), device, group, name: name.into(), unit, value: None, unavailable: None, source, kind: Kind::Measure, at, words: None }
    }

    /// A state shown in words (no Min/Max) and no words yet.
    pub fn state(id: &str, device: Device, group: Group, name: &str, source: &'static str, at: Instant) -> Self {
        Self::measure(id, device, group, name, Unit::Text, source, at).counter()
    }

    /// The same text reading with its words, or unavailable for `why`.
    pub fn with_text(mut self, words: Option<String>, why: &str) -> Self {
        let words = words.filter(|w| !w.is_empty());
        self = self.with(words.as_ref().map(|_| 1.0), why);
        self.words = words;
        self
    }

    /// The same reading with a value, or unavailable for `why` when none.
    pub fn with(mut self, value: Option<f64>, why: &str) -> Self {
        self.value = value.filter(|v| v.is_finite());
        self.unavailable = self.value.is_none().then(|| why.to_string());
        self
    }

    /// The same reading, its value told in `words` instead of its unit's
    /// format (a number with words, e.g. "draining at 12 W"); Min/Avg/Max
    /// and the graph still use the number.
    pub fn told_as(mut self, words: String) -> Self {
        self.words = self.value.is_some().then_some(words).filter(|w| !w.is_empty());
        self
    }

    /// The same reading shown without Min/Max.
    pub fn counter(mut self) -> Self {
        self.kind = Kind::Counter;
        self
    }

    /// Its value in its unit, or why it is missing.
    pub fn text(&self) -> String {
        self.text_in(false)
    }

    /// Like [`Reading::text`], temperatures in °F when `fahrenheit`.
    pub fn text_in(&self, fahrenheit: bool) -> String {
        match self.value {
            Some(_) if self.unit == Unit::Text => self.words.clone().unwrap_or_default(),
            Some(_) if self.words.is_some() => self.words.clone().unwrap_or_default(),
            Some(v) => self.unit.format_in(v, fahrenheit),
            None => self.unavailable.clone().unwrap_or_else(|| "Unavailable".into()),
        }
    }
}

/// Whether a provider works on this PC, and why not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStatus {
    pub name: &'static str,
    /// `None` = working; otherwise why it reads nothing.
    pub problem: Option<String>,
}

/// A group of a device and its readings.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupNode {
    pub group: Group,
    pub readings: Vec<Reading>,
}

/// A device and its groups, in display order.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceNode {
    pub device: Device,
    pub groups: Vec<GroupNode>,
}

/// Everything the providers read, merged into one tree.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Grows with every new frame.
    pub seq: u64,
    pub devices: Vec<DeviceNode>,
    pub sources: Vec<SourceStatus>,
}

impl Frame {
    /// Merges the readings of each provider (in provider order) into one
    /// tree. A metric read by several providers gives one row: the first
    /// value wins, and a value always replaces an unavailable reading.
    pub fn build(seq: u64, reports: &[Vec<Reading>], sources: Vec<SourceStatus>) -> Self {
        let mut merged: Vec<Reading> = Vec::new();
        for r in reports.iter().flatten() {
            match merged.iter_mut().find(|m| m.id == r.id) {
                Some(m) if m.value.is_none() && r.value.is_some() => *m = r.clone(),
                Some(_) => {}
                None => merged.push(r.clone()),
            }
        }
        super::gpu::fill_total_power(&mut merged);
        let mut devices: Vec<DeviceNode> = Vec::new();
        for r in merged {
            let d = match devices.iter().position(|d| d.device == r.device) {
                Some(i) => i,
                None => {
                    devices.push(DeviceNode { device: r.device.clone(), groups: Vec::new() });
                    devices.len() - 1
                }
            };
            let groups = &mut devices[d].groups;
            match groups.iter_mut().find(|g| g.group == r.group) {
                Some(g) => g.readings.push(r),
                None => groups.push(GroupNode { group: r.group, readings: vec![r] }),
            }
        }
        // Stable sorts: same-kind devices keep the providers' order.
        devices.sort_by_key(|d| d.device.kind);
        for d in &mut devices {
            d.groups.sort_by_key(|g| g.group);
        }
        Frame { seq, devices, sources }
    }

    /// The reading of a metric, if any provider gave it.
    pub fn reading(&self, id: &str) -> Option<&Reading> {
        self.readings().find(|r| r.id == id)
    }

    /// Every reading, in tree order.
    pub fn readings(&self) -> impl Iterator<Item = &Reading> {
        self.devices.iter().flat_map(|d| &d.groups).flat_map(|g| &g.readings)
    }

    /// When the frame was read: its newest reading's instant (`None` for a
    /// frame without readings).
    pub fn read_at(&self) -> Option<Instant> {
        self.readings().map(|r| r.at).max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn reading(id: &str, device: Device, group: Group, value: Option<f64>) -> Reading {
        let mut r = Reading::measure(id, device, group, id, Unit::Hz, "test", Instant::now());
        r.value = value;
        if value.is_none() {
            r.unavailable = Some("no provider on this PC".into());
        }
        r
    }

    fn group_names(frame: &Frame, device: &str) -> Vec<&'static str> {
        let d = frame.devices.iter().find(|d| d.device.name == device).unwrap();
        d.groups.iter().map(|g| g.group.name()).collect()
    }

    #[test]
    fn watts_show_with_one_decimal_and_a_w() {
        assert_eq!(Unit::Watt.format(47.000_02), "47.0 W");
        assert_eq!(Unit::Watt.format(1.528), "1.5 W");
        assert_eq!(Unit::Watt.format(0.0), "0.0 W");
        assert_eq!(Unit::Watt.format(312.46), "312.5 W");
    }

    #[test]
    fn counts_and_hours_carry_thousands_separators() {
        assert_eq!(Unit::Count.format(1_234_567.0), "1,234,567");
        assert_eq!(Unit::Count.format(999.0), "999");
        assert_eq!(Unit::Count.format(0.0), "0");
        assert_eq!(Unit::Hours.format(12_345.0), "12,345 h");
        assert_eq!(Unit::Percent.format(42.4), "42 %");
        assert_eq!(Unit::Hz.format(4.72e9), "4.72 GHz");
    }

    #[test]
    fn temperatures_follow_celsius_or_fahrenheit() {
        assert_eq!(Unit::Celsius.format(45.0), "45.0 °C");
        assert_eq!(Unit::Celsius.format_in(45.0, true), "113.0 °F");
        assert_eq!(Unit::Celsius.format_in(-40.0, true), "-40.0 °F");
        assert_eq!(Unit::Hz.format_in(4.72e9, true), "4.72 GHz", "only temperatures change");
        let r = Reading::measure("t", Device::cpu("CPU"), Group::Temperatures, "Package", Unit::Celsius, "test", Instant::now()).with(Some(100.0), "x");
        assert_eq!(r.text_in(true), "212.0 °F");
    }

    #[test]
    fn disk_speeds_show_in_mb_s_below_a_gigabyte_and_in_gb_s_above() {
        let speed = Unit::BytesPerSec;
        assert_eq!(speed.format(0.0), "0.0 MB/s");
        assert_eq!(speed.format(12_345_678.0), "12.3 MB/s");
        assert_eq!(speed.format(999_900_000.0), "999.9 MB/s");
        assert_eq!(speed.format(1_000_000_000.0), "1.00 GB/s");
        assert_eq!(speed.format(6_850_000_000.0), "6.85 GB/s");
    }

    #[test]
    fn groups_come_in_display_order() {
        let cpu = Device::cpu("AMD Ryzen 5 7600X");
        let disk = Device::new(DeviceKind::Disk, "disk0", "Samsung SSD 990 PRO");
        let mut readings = Vec::new();
        // Read in a scrambled order on purpose.
        for g in [Group::Powers, Group::Clocks, Group::Utilization, Group::Temperatures] {
            readings.push(reading(&format!("cpu.{}", g.name()), cpu.clone(), g, Some(1.0)));
        }
        for g in [Group::Smart, Group::Speed, Group::Temperatures, Group::Utilization] {
            readings.push(reading(&format!("disk.{}", g.name()), disk.clone(), g, Some(1.0)));
        }
        let frame = Frame::build(0, &[readings], Vec::new());
        assert_eq!(group_names(&frame, "AMD Ryzen 5 7600X"), vec!["Temperatures", "Utilization", "Clocks", "Powers"]);
        assert_eq!(group_names(&frame, "Samsung SSD 990 PRO"), vec!["Temperatures", "Utilization", "Speed", "S.M.A.R.T."]);
    }

    #[test]
    fn clocks_show_in_ghz_from_one_gigahertz_and_in_mhz_below() {
        assert_eq!(Unit::Hz.format(5.11e9), "5.11 GHz");
        assert_eq!(Unit::Hz.format(1.0e9), "1.00 GHz");
        assert_eq!(Unit::Hz.format(100.0e6), "100 MHz");
        assert_eq!(Unit::Hz.format(999.6e6), "1000 MHz");
    }

    #[test]
    fn a_text_reading_shows_its_words_and_has_no_min_max() {
        let r = Reading::state("cpu.power.plan", Device::cpu("CPU"), Group::General, "Governor", "test", Instant::now()).with_text(Some("Ultimate Performance".into()), "x");
        assert_eq!((r.text(), r.kind), ("Ultimate Performance".to_string(), Kind::Counter));
        let missing = Reading::state("cpu.power.plan", Device::cpu("CPU"), Group::General, "Governor", "test", Instant::now()).with_text(None, "no plan");
        assert_eq!((missing.value, missing.text()), (None, "no plan".to_string()));
    }

    #[test]
    fn this_pc_comes_first_then_the_cpu_then_the_disks() {
        let disk = Device::new(DeviceKind::Disk, "disk0", "SSD");
        let readings = vec![
            reading("disk.t", disk, Group::Temperatures, Some(40.0)),
            reading("cpu.c", Device::cpu("CPU"), Group::Clocks, Some(1.0)),
            reading("pc.p", Device::this_pc(), Group::Powers, Some(90.0)),
        ];
        let frame = Frame::build(0, &[readings], Vec::new());
        let names: Vec<_> = frame.devices.iter().map(|d| d.device.name.as_str()).collect();
        assert_eq!(names, vec!["This PC", "CPU", "SSD"]);
    }

    #[test]
    fn readings_keep_the_order_their_provider_gave_within_a_group() {
        let cpu = Device::cpu("CPU");
        let readings: Vec<_> = (0..12).map(|i| reading(&format!("cpu.core{i}.frequency.reported_hz"), cpu.clone(), Group::Clocks, Some(1.0))).collect();
        let frame = Frame::build(0, &[readings], Vec::new());
        let ids: Vec<_> = frame.devices[0].groups[0].readings.iter().map(|r| r.id.clone()).collect();
        assert_eq!(ids.first().map(String::as_str), Some("cpu.core0.frequency.reported_hz"));
        assert_eq!(ids.last().map(String::as_str), Some("cpu.core11.frequency.reported_hz"));
    }

    #[test]
    fn readings_of_different_providers_are_merged_by_metric_id() {
        let cpu = Device::cpu("CPU");
        let counters = vec![
            reading("cpu.package.power_w", cpu.clone(), Group::Powers, None),
            reading("cpu.package.frequency.average_effective_hz", cpu.clone(), Group::Clocks, Some(5.1e9)),
        ];
        let sdk = vec![
            reading("cpu.package.power_w", cpu.clone(), Group::Powers, Some(88.0)),
            reading("cpu.package.frequency.average_effective_hz", cpu.clone(), Group::Clocks, None),
        ];
        let frame = Frame::build(0, &[counters, sdk], Vec::new());
        assert_eq!(frame.reading("cpu.package.power_w").and_then(|r| r.value), Some(88.0), "a value fills an unavailable reading");
        assert_eq!(
            frame.reading("cpu.package.frequency.average_effective_hz").and_then(|r| r.value),
            Some(5.1e9),
            "an unavailable reading never hides a value"
        );
        let count: usize = frame.devices.iter().flat_map(|d| &d.groups).map(|g| g.readings.len()).sum();
        assert_eq!(count, 2, "one row per metric");
    }
}
