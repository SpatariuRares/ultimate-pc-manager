//! The power provider ("Windows power status"): power source, battery and
//! battery saver (`GetSystemPowerStatus` and the battery IOCTLs), ACPI
//! thermal zones (PDH `Thermal Zone Information`) and the thermal state
//! drawn from them.

use std::time::Instant;

use super::model::{Device, DeviceKind, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Windows power status";
/// Shown under ACPI Thermal Zones when the counter has no instance.
pub const NO_ZONES: &str = "firmware publishes no ACPI thermal zones";
/// The battery device's only row on a desktop.
pub const NO_BATTERY: &str = "No battery in this PC";
const NO_COUNTER: &str = "no counter on this PC";
const UNKNOWN: &str = "Windows does not know";
const NOT_REPORTED: &str = "the battery does not report it";

/// One ACPI thermal zone as read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zone {
    /// The zone's name ("TZ00", from the instance `\_TZ.TZ00`).
    pub name: String,
    /// `High Precision Temperature`: tenths of a kelvin.
    pub tenths_kelvin: Option<f64>,
    /// `% Passive Limit`: 100 = the zone does not slow the CPU down.
    pub passive_limit: Option<f64>,
}

/// The words of the thermal state, worst last.
pub const THERMAL_STATES: &[&str] = &["nominal", "elevated", "serious", "critical"];

/// "Read from" of the thermal state: it says the state is our inference.
pub const THERMAL_STATE_SOURCE: &str =
    "Windows power status, inferred from the ACPI zones' lowest % Passive Limit: 100 nominal, 80 or more elevated, 50 or more serious, below critical";

/// The thermal state (index into [`THERMAL_STATES`]) from the lowest
/// passive limit of the zones: 100 % nominal, 80 % or more elevated, 50 % or
/// more serious, below that critical.
///
/// This is an inference, not a value Windows reports: Windows publishes no
/// thermal state for desktop apps, only each zone's `% Passive Limit` (how
/// far the firmware lets the CPU run). The thresholds are our own choice,
/// shown as the reading's source ([`THERMAL_STATE_SOURCE`]).
pub fn thermal_state(lowest_passive_limit: f64) -> usize {
    match lowest_passive_limit {
        l if l >= 100.0 => 0,
        l if l >= 80.0 => 1,
        l if l >= 50.0 => 2,
        _ => 3,
    }
}

/// `SYSTEM_POWER_STATUS`, as `GetSystemPowerStatus` fills it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PowerStatus {
    /// 0 = offline (battery), 1 = online (AC), 255 = unknown.
    pub ac_line_status: u8,
    /// Bits: 1 high, 2 low, 4 critical, 8 charging, 128 no system
    /// battery; 255 = unknown.
    pub battery_flag: u8,
    /// 0-100, 255 = unknown.
    pub battery_life_percent: u8,
    /// 1 = battery saver on.
    pub system_status_flag: u8,
    /// Seconds left, `u32::MAX` = unknown (on AC, or still estimating).
    pub battery_life_time: u32,
}

/// What the battery's IOCTLs report (`BATTERY_INFORMATION`, device name,
/// `BATTERY_STATUS`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BatteryDetails {
    pub name: String,
    /// mWh (or relative units: only the ratio is used).
    pub designed_capacity: u32,
    pub full_charged_capacity: u32,
    /// 0 = not reported.
    pub cycle_count: u32,
    /// `BATTERY_INFORMATION.Capabilities` has `BATTERY_CAPACITY_RELATIVE`:
    /// capacities and the rate are in arbitrary units, not mWh / mW.
    pub relative: bool,
    /// `BATTERY_STATUS.Rate` as read (mW, may be [`BATTERY_UNKNOWN_RATE`]);
    /// `None` = `IOCTL_BATTERY_QUERY_STATUS` failed.
    pub rate: Option<i32>,
}

/// The battery's charge / discharge rate in signed watts (+ charging,
/// - draining).
pub const BATTERY_RATE: &str = "battery.rate_w";

/// `BATTERY_UNKNOWN_RATE` (batclass.h, 0x80000000) in the signed `Rate`.
pub const BATTERY_UNKNOWN_RATE: i32 = i32::MIN;
const NO_RATE: &str = "the battery does not report its rate right now";
const RELATIVE_RATE: &str = "the battery reports its rate in relative units, not watts";

/// The charge / discharge rate of `BATTERY_STATUS.Rate`: signed watts
/// (positive charging) and the words shown, or why it cannot be shown.
///
/// Per Microsoft's BATTERY_STATUS docs
/// (https://learn.microsoft.com/en-us/windows/win32/power/battery-status-str):
/// "This value will be in milliwatts unless the battery rate information is
/// relative, in which case it will be in arbitrary units per hour [...] A
/// nonzero, positive rate indicates charging; a negative rate indicates
/// discharging. [...] If the rate is unavailable, this member is
/// BATTERY_UNKNOWN_RATE" (0x80000000 in batclass.h).
pub fn decode_rate(rate: i32, relative: bool) -> Result<(f64, String), &'static str> {
    if rate == BATTERY_UNKNOWN_RATE {
        return Err(NO_RATE);
    }
    if relative {
        return Err(RELATIVE_RATE);
    }
    let watts = f64::from(rate) / 1000.0;
    let magnitude = watts.abs();
    let shown = if (magnitude * 10.0).round() % 10.0 == 0.0 { format!("{magnitude:.0} W") } else { format!("{magnitude:.1} W") };
    let words = match rate {
        r if r > 0 => format!("charging at {shown}"),
        r if r < 0 => format!("draining at {shown}"),
        _ => "neither charging nor draining".to_string(),
    };
    Ok((watts, words))
}

/// One read of the power and thermal raw values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerSample {
    /// `None` = `GetSystemPowerStatus` failed.
    pub status: Option<PowerStatus>,
    /// `None` = no battery answered its IOCTLs.
    pub battery: Option<BatteryDetails>,
    pub zones: Vec<Zone>,
}

/// Degrees Celsius from tenths of a kelvin.
pub fn celsius_from_tenths_kelvin(tenths: f64) -> f64 {
    tenths / 10.0 - 273.15
}

fn zones_device() -> Device {
    Device::new(DeviceKind::ThermalZone, "acpi-thermal-zones", "ACPI Thermal Zones")
}

impl PowerSample {
    /// The readings of this sample, in the order shown.
    pub fn readings(&self, at: Instant) -> Vec<Reading> {
        let mut out = Vec::new();
        let pc = Device::this_pc();
        let status = self.status;
        let ac = status.and_then(|s| match s.ac_line_status {
            0 => Some(0.0),
            1 => Some(1.0),
            _ => None,
        });
        let source = Unit::YesNo { yes: "AC Power", no: "Battery" };
        out.push(Reading::measure("system.power.source", pc.clone(), Group::General, "Power Source", source, SOURCE, at).with(ac, UNKNOWN).counter());
        let lowest = self.zones.iter().filter_map(|z| z.passive_limit).fold(None, |m: Option<f64>, l| Some(m.map_or(l, |m| m.min(l))));
        let state = lowest.map(|l| thermal_state(l) as f64);
        let r = Reading::measure("system.thermal_state", pc, Group::General, "Thermal State", Unit::State(THERMAL_STATES), THERMAL_STATE_SOURCE, at);
        out.push(r.with(state, "no ACPI thermal zone to read it from").counter());
        out.extend(self.battery_readings(at));
        if self.zones.is_empty() {
            let r = Reading::measure("acpi.zones", zones_device(), Group::General, "Thermal Zones", Unit::Celsius, SOURCE, at);
            out.push(r.with(None, NO_ZONES).counter());
        }
        for z in &self.zones {
            let id = format!("acpi.tz.{}.temperature_c", z.name);
            let c = z.tenths_kelvin.map(celsius_from_tenths_kelvin);
            out.push(Reading::measure(&id, zones_device(), Group::Temperatures, &z.name, Unit::Celsius, SOURCE, at).with(c, NO_COUNTER));
        }
        for z in &self.zones {
            let row = |what: &str, name: &str, unit: Unit, v: Option<f64>| {
                Reading::measure(&format!("acpi.tz.{}.{what}", z.name), zones_device(), Group::General, &format!("{} {name}", z.name), unit, SOURCE, at).with(v, NO_COUNTER)
            };
            out.push(row("passive_limit_pct", "Passive Limit", Unit::Percent, z.passive_limit));
            let cooling = Unit::YesNo { yes: "Slowing the CPU to cool down", no: "Not limiting" };
            out.push(row("cooling", "Cooling", cooling, z.passive_limit.map(|l| f64::from(l < 100.0))).counter());
        }
        out
    }

    fn battery_readings(&self, at: Instant) -> Vec<Reading> {
        let status = self.status.unwrap_or_default();
        let name = self.battery.as_ref().map(|b| b.name.as_str()).filter(|n| !n.is_empty()).unwrap_or("Battery");
        let device = Device::new(DeviceKind::Battery, "battery0", name);
        let row = |id: &str, name: &str, unit: Unit, value: Option<f64>, why: &str| {
            Reading::measure(id, device.clone(), Group::General, name, unit, SOURCE, at).with(value, why)
        };
        let known = |v: u8| (v != 255).then_some(v);
        let flag = known(status.battery_flag);
        if self.battery.is_none() && flag.is_some_and(|f| f & 128 != 0) {
            let r = row("battery.present", "Battery", Unit::YesNo { yes: "Present", no: "None" }, None, NO_BATTERY);
            return vec![r.counter()];
        }
        let details = self.battery.as_ref();
        let charge = known(status.battery_life_percent).map(f64::from);
        let charging = flag.map(|f| f64::from(f & 8 != 0));
        let left = (status.battery_life_time != u32::MAX).then(|| f64::from(status.battery_life_time));
        let left_why = if status.ac_line_status == 1 { "on AC power" } else { "Windows is still estimating" };
        let cycles = details.map(|b| f64::from(b.cycle_count)).filter(|c| *c > 0.0);
        let condition = details
            .filter(|b| b.designed_capacity > 0 && b.full_charged_capacity > 0)
            .map(|b| f64::from(b.full_charged_capacity) * 100.0 / f64::from(b.designed_capacity));
        let rate = details.and_then(|b| b.rate).ok_or(NO_RATE).and_then(|r| decode_rate(r, details.is_some_and(|b| b.relative)));
        // Signed watts (+ charging, - draining), with Min/Avg/Max and a
        // graph; its value is told in words ("draining at 12 W").
        let rate_row = Reading::measure(BATTERY_RATE, device.clone(), Group::General, "Charge Rate", Unit::Watt, SOURCE, at);
        let rate_row = match rate {
            Ok((watts, words)) => rate_row.with(Some(watts), NO_RATE).told_as(words),
            Err(why) => rate_row.with(None, why),
        };
        let saver = Unit::YesNo { yes: "On", no: "Off" };
        vec![
            row("battery.charge_pct", "Battery Charge", Unit::Percent, charge, UNKNOWN),
            row("battery.charging", "Charging", Unit::YesNo { yes: "Yes", no: "No" }, charging, UNKNOWN).counter(),
            row("battery.time_remaining_s", "Time Remaining", Unit::Seconds, left, left_why).counter(),
            rate_row,
            row("battery.cycle_count", "Cycle Count", Unit::Count, cycles, NOT_REPORTED).counter(),
            row("battery.condition_pct", "Condition", Unit::Percent, condition, NOT_REPORTED).counter(),
            row("battery.saver", "Battery Saver", saver, self.status.map(|s| f64::from(s.system_status_flag & 1)), UNKNOWN).counter(),
        ]
    }
}

/// The power provider. Built on its worker thread (holds a PDH query).
pub struct PowerStatusProvider {
    reader: imp::Reader,
}

impl PowerStatusProvider {
    pub fn new() -> Self {
        PowerStatusProvider { reader: imp::Reader::open() }
    }
}

impl Provider for PowerStatusProvider {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        self.reader.availability()
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        self.reader.sample().readings(at)
    }
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
    use crate::sensors::model::{Frame, Kind};

    fn frame(s: &PowerSample) -> Frame {
        Frame::build(1, &[s.readings(Instant::now())], Vec::new())
    }

    #[test]
    fn a_thermal_zone_temperature_comes_from_tenths_of_a_kelvin() {
        let s = PowerSample { zones: vec![Zone { name: "TZ00".into(), tenths_kelvin: Some(3_232.0), passive_limit: None }], ..Default::default() };
        let f = frame(&s);
        let t = f.reading("acpi.tz.TZ00.temperature_c").unwrap();
        assert!((t.value.unwrap() - 50.05).abs() < 1e-9, "3232 dK = 50.05 °C, got {:?}", t.value);
        assert_eq!(Unit::Celsius.format(50.0), "50.0 °C");
        assert_eq!(t.name, "TZ00");
        assert!(f.devices.iter().any(|d| d.device.kind == DeviceKind::ThermalZone && d.device.name == "ACPI Thermal Zones"));
    }

    #[test]
    fn without_thermal_zones_the_tree_says_the_firmware_publishes_none() {
        let f = frame(&PowerSample::default());
        let d = f.devices.iter().find(|d| d.device.name == "ACPI Thermal Zones").expect("the zones device is still listed");
        let r = &d.groups[0].readings[0];
        assert_eq!((r.value, r.text()), (None, "firmware publishes no ACPI thermal zones".to_string()));
    }

    fn laptop_on_battery() -> PowerSample {
        PowerSample {
            // GetSystemPowerStatus: offline, battery high, 76 %, saver on, 1 h 30 min left.
            status: Some(PowerStatus { ac_line_status: 0, battery_flag: 1, battery_life_percent: 76, system_status_flag: 1, battery_life_time: 5_400 }),
            // BATTERY_STATUS.Rate: -12 000 mW, draining.
            battery: Some(BatteryDetails { name: "DELL 7FHHV".into(), designed_capacity: 50_000, full_charged_capacity: 45_000, cycle_count: 312, rate: Some(-12_000), ..Default::default() }),
            zones: Vec::new(),
        }
    }

    fn text(f: &Frame, id: &str) -> String {
        f.reading(id).unwrap_or_else(|| panic!("no reading {id}")).text()
    }

    #[test]
    fn a_laptop_on_battery_shows_charge_time_cycles_condition_and_saver() {
        let f = frame(&laptop_on_battery());
        assert_eq!(text(&f, "system.power.source"), "Battery");
        let battery = f.devices.iter().find(|d| d.device.kind == DeviceKind::Battery).unwrap();
        assert_eq!(battery.device.name, "DELL 7FHHV");
        assert_eq!(text(&f, "battery.charge_pct"), "76 %");
        assert_eq!(text(&f, "battery.charging"), "No");
        assert_eq!(text(&f, "battery.time_remaining_s"), "1 h 30 min");
        assert_eq!(text(&f, "battery.rate_w"), "draining at 12 W");
        assert_eq!(text(&f, "battery.cycle_count"), "312");
        assert_eq!(text(&f, "battery.condition_pct"), "90 %");
        assert_eq!(text(&f, "battery.saver"), "On");
    }

    #[test]
    fn a_desktop_says_no_battery_in_this_pc_instead_of_empty_rows() {
        // GetSystemPowerStatus on a desktop: online, flag 128 "no system battery".
        let status = PowerStatus { ac_line_status: 1, battery_flag: 128, battery_life_percent: 255, system_status_flag: 0, battery_life_time: u32::MAX };
        let f = frame(&PowerSample { status: Some(status), ..Default::default() });
        assert_eq!(text(&f, "system.power.source"), "AC Power");
        let battery = f.devices.iter().find(|d| d.device.kind == DeviceKind::Battery).expect("the battery device is still listed");
        let rows: Vec<String> = battery.groups.iter().flat_map(|g| &g.readings).map(|r| r.text()).collect();
        assert_eq!(rows, vec!["No battery in this PC".to_string()]);
        assert_eq!(f.reading("battery.charge_pct"), None);
    }

    #[test]
    fn the_rate_sign_and_milliwatts_decode_into_watts_and_words() {
        // BATTERY_STATUS.Rate samples: positive charges, negative drains, mW.
        assert_eq!(decode_rate(25_000, false), Ok((25.0, "charging at 25 W".to_string())));
        assert_eq!(decode_rate(-12_000, false), Ok((-12.0, "draining at 12 W".to_string())));
        assert_eq!(decode_rate(-12_345, false), Ok((-12.345, "draining at 12.3 W".to_string())));
        assert_eq!(decode_rate(0, false), Ok((0.0, "neither charging nor draining".to_string())));
    }

    #[test]
    fn the_charge_rate_is_a_signed_measure_in_watts() {
        let mut s = laptop_on_battery();
        let r = frame(&s).reading("battery.rate_w").cloned().unwrap();
        assert_eq!((r.value, r.unit, r.kind), (Some(-12.0), Unit::Watt, Kind::Measure), "draining: negative watts with Min/Avg/Max");
        s.battery.as_mut().unwrap().rate = Some(25_000);
        let r = frame(&s).reading("battery.rate_w").cloned().unwrap();
        assert_eq!((r.value, r.text()), (Some(25.0), "charging at 25 W".to_string()), "charging: positive, still told in words");
        assert_eq!(r.unit.format(-12.0), "-12.0 W", "Min/Avg/Max keep the sign");
    }

    #[test]
    fn an_unknown_or_relative_rate_says_why_it_is_missing() {
        // BATTERY_UNKNOWN_RATE is 0x80000000 in the LONG Rate member.
        assert_eq!(decode_rate(i32::MIN, false), Err("the battery does not report its rate right now"));
        assert_eq!(decode_rate(-12_000, true), Err("the battery reports its rate in relative units, not watts"));
    }

    #[test]
    fn an_unknown_rate_greys_with_its_reason_next_to_the_time_remaining() {
        let mut s = laptop_on_battery();
        s.battery.as_mut().unwrap().rate = Some(i32::MIN);
        let f = frame(&s);
        let r = f.reading("battery.rate_w").unwrap();
        assert_eq!((r.value, r.text()), (None, "the battery does not report its rate right now".to_string()));
        let battery = f.devices.iter().find(|d| d.device.kind == DeviceKind::Battery).unwrap();
        let names: Vec<&str> = battery.groups[0].readings.iter().map(|r| r.name.as_str()).collect();
        let after = names.iter().position(|n| *n == "Time Remaining").unwrap() + 1;
        assert_eq!(names[after], "Charge Rate");

        s.battery.as_mut().unwrap().rate = None;
        let r = frame(&s).reading("battery.rate_w").cloned().unwrap();
        assert_eq!((r.value, r.text()), (None, "the battery does not report its rate right now".to_string()));
    }

    #[test]
    fn a_desktop_still_says_no_battery_and_shows_no_charge_rate() {
        let status = PowerStatus { ac_line_status: 1, battery_flag: 128, battery_life_percent: 255, system_status_flag: 0, battery_life_time: u32::MAX };
        let f = frame(&PowerSample { status: Some(status), ..Default::default() });
        assert_eq!(text(&f, "battery.present"), "No battery in this PC");
        assert_eq!(f.reading("battery.rate_w"), None);
    }

    fn zone(name: &str, passive_limit: f64) -> Zone {
        Zone { name: name.into(), tenths_kelvin: Some(3_232.0), passive_limit: Some(passive_limit) }
    }

    #[test]
    fn the_thermal_state_is_the_worst_zone_and_each_zone_says_whether_it_cools() {
        let state = |limits: &[f64]| {
            let zones = limits.iter().enumerate().map(|(i, l)| zone(&format!("TZ0{i}"), *l)).collect();
            text(&frame(&PowerSample { zones, ..Default::default() }), "system.thermal_state")
        };
        assert_eq!(state(&[100.0, 100.0]), "nominal");
        assert_eq!(state(&[100.0, 90.0]), "elevated");
        assert_eq!(state(&[100.0, 60.0]), "serious");
        assert_eq!(state(&[30.0]), "critical");

        let f = frame(&PowerSample { zones: vec![zone("TZ00", 100.0), zone("TZ01", 60.0)], ..Default::default() });
        assert_eq!(text(&f, "acpi.tz.TZ00.cooling"), "Not limiting");
        assert_eq!(text(&f, "acpi.tz.TZ01.cooling"), "Slowing the CPU to cool down");
        assert_eq!(text(&f, "acpi.tz.TZ01.passive_limit_pct"), "60 %");
        let pc = f.devices.iter().find(|d| d.device.kind == DeviceKind::ThisPc).unwrap();
        assert!(pc.groups[0].readings.iter().any(|r| r.name == "Thermal State"), "under This PC › General");
    }

    #[test]
    fn the_thermal_state_says_it_is_inferred_and_how() {
        let f = frame(&PowerSample { zones: vec![zone("TZ00", 90.0)], ..Default::default() });
        let r = f.reading("system.thermal_state").unwrap();
        assert_eq!(r.source, "Windows power status, inferred from the ACPI zones' lowest % Passive Limit: 100 nominal, 80 or more elevated, 50 or more serious, below critical");
        let d = crate::addons::power_freq::panel::Detail::of(&f, &Default::default(), false, "system.thermal_state").unwrap();
        assert!(d.copy_text().ends_with("read from Windows power status, inferred from the ACPI zones' lowest % Passive Limit: 100 nominal, 80 or more elevated, 50 or more serious, below critical"));
    }

    #[test]
    fn without_thermal_zones_the_thermal_state_says_why_it_is_missing() {
        let r = frame(&PowerSample::default()).reading("system.thermal_state").cloned().unwrap();
        assert_eq!((r.value, r.text()), (None, "no ACPI thermal zone to read it from".to_string()));
    }

    /// Reads this PC and prints the readings (here: AC Power, no battery,
    /// no ACPI thermal zone).
    #[test]
    #[ignore]
    fn reads_this_pcs_power() {
        let mut p = PowerStatusProvider::new();
        assert_eq!(p.availability(), Ok(()));
        let f = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        for r in f.readings() {
            println!("{:<20} {:<26} {}", r.device.name, r.name, r.text());
        }
        assert_eq!(text(&f, "system.power.source"), "AC Power");
        assert_eq!(text(&f, "battery.present"), NO_BATTERY);
        assert_eq!(text(&f, "acpi.zones"), NO_ZONES);
    }
}
