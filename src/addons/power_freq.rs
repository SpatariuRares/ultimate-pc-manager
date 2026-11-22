//! The Power & Freq add-on: the hardware sensor tree This PC › device ›
//! group › sensor with Sensor / Value / Min / Avg / Max, read from the shared
//! sensor service (`crate::sensors`) while its page is shown. Min/Max cover
//! the time since the add-on started and are forgotten when it stops.
//! Drawn in design A (#62): status band, AMD / administrator band, KPI
//! tiles, then the tree beside the side panel.

pub mod panel;
/// Threshold alerts and their evaluator.
pub mod alerts;
/// The alerts' watcher thread.
pub mod watch;
/// Alerts in Settings › Power & Freq and the add-on's alert commands.
mod alerts_ui;
/// Sensors pinned to the Summary tile.
mod pins;
#[cfg_attr(not(test), allow(unused_imports))] // the paths tests and other add-ons name
pub use pins::{pinned_lines, NOT_ON_THIS_PC, PIN, PINS_TIER_ID, UNPIN};

use egui::{vec2, Align, Color32, Layout, RichText, Ui};

use crate::addons::{read_settings, Addon, Consumption, Recording, Requirements, Tier};
use crate::platform::{Action, AddonContext, PageContext};
use crate::sensors::cpu::{BASE_HZ, CORE_AVERAGE, LIMIT_REASONS, NO_LIMIT};
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use crate::sensors::model::{Device, DeviceKind, DeviceNode, Group, Kind, Reading, Unit};
use crate::sensors::power_plan::GOVERNOR;

const REPORTED: &str = "cpu.package.frequency.average_reported_hz";
use crate::addons::leases::Sensors;
use crate::sensors::{Frame, Interest, Stats};
use crate::theme;
use crate::widgets::parts::{admin_button, kpi_tile, neutral_note, outline_button, primary_button, BandArea, Mark, StatusBand};
use crate::widgets::{display_font, dot, hue, meter_with_tick, search_field, sparkline, tree_header, tree_row, Glyph, Hue, RowLead, TreeRow};

/// The AMD SDK / administrator band.
pub mod notice;

/// Shown until every provider answered once.
pub const WAITING: &str = "Waiting for all sensors to report...";

/// A reading older than this many intervals is marked "(stale)".
pub const STALE_INTERVALS: u32 = 3;

/// One row of the tree, ready to draw.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// 0 = This PC, 1 = device, 2 = group, 3 = sensor.
    pub depth: u8,
    pub sensor: String,
    pub value: String,
    pub min: String,
    /// The mean since the statistics epoch; empty for counters and states.
    pub avg: String,
    pub max: String,
    /// The value is missing (drawn grey, never as zero).
    pub unavailable: bool,
    /// Key to open/close this row ([`View::toggle`]); empty for sensors.
    pub key: String,
    /// `Some(open)` for This PC, devices and groups; `None` for sensors.
    pub expanded: Option<bool>,
    /// The group of a group row (its icon).
    pub group: Option<Group>,
    /// Older than [`STALE_INTERVALS`] intervals.
    pub stale: bool,
    /// The sensor's metric id (selects it in the side panel); empty on
    /// headings.
    pub id: String,
    /// The device of This PC and device headings (their icon).
    pub kind: Option<DeviceKind>,
    /// A device with nothing to read: one row saying why, nothing under it.
    pub empty: bool,
}

impl Row {
    fn heading(depth: u8, name: &str, key: String, expanded: bool) -> Self {
        Row { depth, sensor: name.into(), value: String::new(), min: String::new(), avg: String::new(), max: String::new(), unavailable: false, key, expanded: Some(expanded), group: None, stale: false, id: String::new(), kind: None, empty: false }
    }
}

/// The metric of the power source ("AC power, no battery"), read by the
/// battery / power source provider (#52).
pub const POWER_SOURCE: &str = "system.power.source";
/// The metric of the thermal state (nominal / elevated / ...), #52.
pub const THERMAL_STATE: &str = "system.thermal_state";
/// Why a metric no provider gives yet is missing.
const NOT_READ: &str = "no provider on this PC yet";

/// The sentence of the status band: "Cores run at 5.11 GHz, 9% above the
/// 4.70 GHz base" (or "below", or "at the ... base" when within 0.5 %).
/// Below the base, a known limit reason ("power") adds the cause:
/// "... below the 4.70 GHz base: power limit".
pub fn band(core_average_hz: Option<f64>, base_hz: Option<f64>, limit_reason: Option<&str>) -> String {
    let Some(avg) = core_average_hz else {
        return "The cores' clock is not available".into();
    };
    let ghz = |v| Unit::Hz.format(v);
    match base_hz.filter(|b| *b > 0.0) {
        None => format!("Cores run at {}", ghz(avg)),
        Some(base) => {
            let pct = ((avg / base - 1.0) * 100.0).round();
            let how = if pct > 0.0 {
                format!("{pct:.0}% above the")
            } else if pct < 0.0 {
                format!("{:.0}% below the", -pct)
            } else {
                "at the".into()
            };
            let cause = match limit_reason {
                Some(reason) if pct < 0.0 => format!(": {reason} limit"),
                _ => String::new(),
            };
            format!("Cores run at {}, {how} {} base{cause}", ghz(avg), ghz(base))
        }
    }
}

/// The top of the page: the band, the line under it and the KPI tiles.
#[derive(Clone, Debug, PartialEq)]
pub struct Status {
    pub band: String,
    /// Reported clock, parked logical processors, power plan, power source,
    /// and the package temperature when known.
    pub details: Vec<String>,
    /// (name, value or why it is missing).
    pub tiles: Vec<(&'static str, String)>,
    /// Per tile: the value is missing (the tile says why, in grey).
    pub missing: Vec<bool>,
}

/// The metrics of the KPI tiles, in their order.
const TILE_METRICS: [&str; 4] = [CORE_AVERAGE, BASE_HZ, POWER_SOURCE, THERMAL_STATE];

/// The small line under each KPI tile: Core Average's Min/Max, how many
/// logical processors, "No battery in this PC".
pub fn tile_notes(frame: &Frame, stats: &Stats) -> [Option<String>; 4] {
    let min_max = stats.min_max(CORE_AVERAGE).map(|(lo, hi)| format!("Min {} · Max {}", Unit::Hz.format(lo), Unit::Hz.format(hi)));
    let cpus = frame.readings().filter(|r| r.id.ends_with(".parked")).count();
    let cpus = (cpus > 0).then(|| format!("{cpus} logical processor{}", if cpus == 1 { "" } else { "s" }));
    let battery = frame.reading("battery.present").filter(|r| r.value.is_none()).map(|r| r.text());
    [min_max, cpus, battery, None]
}

/// The reasons that hold the CPU back in this frame, from the Limits group;
/// `None` when unread or when nothing limits it.
fn limit_reason(frame: &Frame) -> Option<String> {
    frame.reading(LIMIT_REASONS).filter(|r| r.value.is_some()).map(|r| r.text()).filter(|t| t != NO_LIMIT)
}

/// The band, its details and the KPI tiles of a frame (temperatures in
/// °F when `fahrenheit`).
pub fn status(frame: &Frame, fahrenheit: bool) -> Status {
    let value = |id: &str| frame.reading(id).and_then(|r| r.value);
    let text = |id: &str| frame.reading(id).map_or_else(|| NOT_READ.to_string(), |r| r.text());
    let missing = TILE_METRICS.iter().map(|id| value(id).is_none()).collect();
    let parked = frame.readings().filter(|r| r.id.ends_with(".parked") && r.value == Some(1.0)).count();
    let source = frame.reading(POWER_SOURCE);
    let package = frame.reading(PACKAGE_TEMPERATURE).filter(|r| r.value.is_some()).map(|r| format!("Package {}", r.text_in(fahrenheit)));
    Status {
        band: band(value(CORE_AVERAGE), value(BASE_HZ), limit_reason(frame).as_deref()),
        details: [
            format!("Reported {}", text(REPORTED)),
            format!("{parked} logical processor{} parked", if parked == 1 { "" } else { "s" }),
            match frame.reading(GOVERNOR) {
                Some(r) if r.value.is_some() => r.text(),
                Some(r) => format!("Power plan: {}", r.text()),
                None => format!("Power plan: {NOT_READ}"),
            },
            match source {
                Some(r) if r.value.is_some() => r.text(),
                _ => format!("Power source: {}", text(POWER_SOURCE)),
            },
        ]
        .into_iter()
        .chain(package)
        .collect(),
        tiles: vec![
            ("Core Average", text(CORE_AVERAGE)),
            ("Base frequency", text(BASE_HZ)),
            ("Power source", text(POWER_SOURCE)),
            ("Thermal state", text(THERMAL_STATE)),
        ],
        missing,
    }
}

/// How the tree is shown: what the user opened or closed and searched,
/// plus the clock, interval and temperature unit of this frame.
#[derive(Clone, Debug, Default)]
pub struct View {
    /// When the tree is drawn; `None` = nothing is stale.
    pub now: Option<Instant>,
    /// The update interval; 0 (paused) = nothing is stale.
    pub interval_ms: u64,
    pub fahrenheit: bool,
    /// "Find": only sensors whose name holds this (any case).
    pub find: String,
    /// Keys of the closed rows (everything is open by default).
    pub collapsed: BTreeSet<String>,
    /// Leave out the readings without a value (and groups left empty).
    pub hide_unavailable: bool,
}

const ROOT: &str = "d:pc";

fn device_key(d: &Device) -> String {
    format!("d:{}", d.key)
}

fn group_key(d: &Device, g: Group) -> String {
    format!("g:{}/{}", d.key, g.name())
}

impl View {
    /// Opens a closed row, closes an open one.
    pub fn toggle(&mut self, key: &str) {
        if !self.collapsed.remove(key) {
            self.collapsed.insert(key.to_string());
        }
    }

    /// Opens every row ("Expand all").
    pub fn expand_all(&mut self) {
        self.collapsed.clear();
    }

    /// Closes This PC and every device and group of the frame.
    pub fn collapse_all(&mut self, frame: &Frame) {
        self.collapsed.insert(ROOT.into());
        for d in &frame.devices {
            self.collapsed.insert(device_key(&d.device));
            for g in &d.groups {
                self.collapsed.insert(group_key(&d.device, g.group));
            }
        }
    }

    fn is_open(&self, key: &str) -> bool {
        !self.collapsed.contains(key)
    }

    fn is_stale(&self, r: &Reading) -> bool {
        let limit = Duration::from_millis(self.interval_ms) * STALE_INTERVALS;
        self.interval_ms > 0 && self.now.is_some_and(|now| now.saturating_duration_since(r.at) > limit)
    }

    fn matches(&self, r: &Reading) -> bool {
        let found = self.find.is_empty() || r.name.to_lowercase().contains(&self.find.trim().to_lowercase());
        found && !(self.hide_unavailable && r.value.is_none())
    }

    /// The Value / Min / Avg / Max of a reading as shown.
    fn cells(&self, r: &Reading, stats: &Stats) -> (String, String, String, String, bool) {
        let stale = self.is_stale(r);
        let mut value = r.text_in(self.fahrenheit);
        if stale {
            value.push_str(" (stale)");
        }
        let (min, avg, max) = match (r.kind, stats.min_max(&r.id), stats.avg(&r.id)) {
            (Kind::Measure, Some((lo, hi)), Some(avg)) => (r.unit.format_in(lo, self.fahrenheit), r.unit.format_in(avg, self.fahrenheit), r.unit.format_in(hi, self.fahrenheit)),
            _ => (String::new(), String::new(), String::new()),
        };
        (value, min, avg, max, stale)
    }
}

/// The tree of a frame with the Min/Max seen so far and everything open;
/// `None` while waiting for the first frame.
#[cfg(test)]
pub fn tree(frame: Option<&Frame>, stats: &Stats) -> Option<Vec<Row>> {
    tree_view(frame, stats, &View::default())
}

/// The tree as `view` shows it; `None` while waiting for the first frame.
/// Devices other than This PC sit under a This PC row. A group row sums up
/// its first reading with a value (Clocks: Core Average). While "Find"
/// holds text, the matching sensors show through closed rows.
pub fn tree_view(frame: Option<&Frame>, stats: &Stats, view: &View) -> Option<Vec<Row>> {
    let frame = frame?;
    let finding = !view.find.trim().is_empty();
    let root_open = view.is_open(ROOT);
    let mut rows = vec![Row { kind: Some(DeviceKind::ThisPc), ..Row::heading(0, "This PC", ROOT.into(), root_open) }];
    if !root_open && !finding {
        return Some(rows);
    }
    for d in &frame.devices {
        let mut device_rows = Vec::new();
        let this_pc = d.device.kind == DeviceKind::ThisPc;
        let depth = if this_pc { 0 } else { 1 };
        let dkey = device_key(&d.device);
        let device_open = this_pc || view.is_open(&dkey);
        let why_empty = if this_pc { None } else { nothing_to_read(d) };
        if (device_open || finding) && why_empty.is_none() {
            for g in &d.groups {
                let gkey = group_key(&d.device, g.group);
                let shown: Vec<&Reading> = g.readings.iter().filter(|r| view.matches(r)).collect();
                if (finding || view.hide_unavailable) && shown.is_empty() {
                    continue;
                }
                let mut heading = Row::heading(depth + 1, g.group.name(), gkey.clone(), view.is_open(&gkey));
                heading.group = Some(g.group);
                if let Some(first) = g.readings.iter().find(|r| r.value.is_some()).or(g.readings.first()) {
                    let (value, min, avg, max, stale) = view.cells(first, stats);
                    (heading.value, heading.min, heading.avg, heading.max, heading.stale, heading.unavailable) = (value, min, avg, max, stale, first.value.is_none());
                }
                device_rows.push(heading);
                if view.is_open(&gkey) || finding {
                    for r in shown {
                        let (value, min, avg, max, stale) = view.cells(r, stats);
                        device_rows.push(Row { depth: depth + 2, sensor: r.name.clone(), value, min, avg, max, unavailable: r.value.is_none(), key: String::new(), expanded: None, group: None, stale, id: r.id.clone(), kind: None, empty: false });
                    }
                }
            }
        }
        let nothing_shown = !d.groups.iter().flat_map(|g| &g.readings).any(|r| view.matches(r));
        if (finding && device_rows.is_empty()) || (view.hide_unavailable && nothing_shown) {
            continue;
        }
        if !this_pc {
            let mut heading = Row::heading(1, &d.device.name, dkey, device_open);
            heading.kind = Some(d.device.kind);
            if d.device.kind == DeviceKind::Gpu {
                heading.value = crate::sensors::gpu::summary(d);
            }
            if let Some(why) = why_empty {
                (heading.value, heading.unavailable, heading.empty) = (why, true, true);
            }
            rows.push(heading);
        }
        rows.extend(device_rows);
    }
    Some(rows)
}

/// Why a device has nothing to read ("No battery in this PC"), when none
/// of its readings has a value and they all say the same.
fn nothing_to_read(d: &DeviceNode) -> Option<String> {
    let mut all = d.groups.iter().flat_map(|g| &g.readings);
    let first = all.next()?;
    let why = first.text();
    (first.value.is_none() && all.all(|r| r.value.is_none() && r.text() == why)).then_some(why)
}

/// The drawn icon and colour of a group row (and the dots of its sensors).
fn group_look(g: Group) -> (Glyph, Hue) {
    match g {
        Group::Temperatures => (Glyph::Thermometer, Hue::Orange),
        Group::Utilization => (Glyph::Gauge, Hue::Green),
        Group::Clocks => (Glyph::Clock, Hue::Red),
        Group::Limits => (Glyph::Sliders, Hue::Orange),
        Group::IdleStates => (Glyph::Gauge, Hue::Blue),
        Group::Speed => (Glyph::Trend, Hue::Blue),
        Group::Powers => (Glyph::Bolt, Hue::Yellow),
        Group::Voltages => (Glyph::Wave, Hue::Violet),
        Group::Levels => (Glyph::Sliders, Hue::Teal),
        Group::Smart => (Glyph::ShieldCheck, Hue::Green),
        Group::General => (Glyph::Info, Hue::Grey),
        Group::Fans => (Glyph::Fan, Hue::Teal),
    }
}

/// The drawn icon of a device row.
fn device_glyph(k: DeviceKind) -> Glyph {
    match k {
        DeviceKind::ThisPc => Glyph::Monitor,
        DeviceKind::Cpu => Glyph::Chip,
        DeviceKind::Gpu => Glyph::GraphicsCard,
        DeviceKind::Npu => Glyph::Hexagon,
        DeviceKind::Motherboard => Glyph::Board,
        DeviceKind::Memory => Glyph::MemoryStick,
        DeviceKind::Disk => Glyph::Drive,
        DeviceKind::Battery => Glyph::Battery,
        DeviceKind::ThermalZone => Glyph::Thermometer,
    }
}

/// The CPU package's power (Energy Meter, or a vendor SDK with the same id).
pub const PACKAGE_POWER: &str = crate::sensors::energy::PACKAGE;
/// The CPU package's temperature (a vendor SDK; none reads it yet).
pub const PACKAGE_TEMPERATURE: &str = crate::sensors::cpu::PACKAGE_TEMPERATURE;

/// The words of a disk's S.M.A.R.T. Health row when nothing is wrong (as
/// the ATA and NVMe readers write them).
const HEALTHY: &str = "Healthy";

/// The Summary tile's lines for the whole PC, each only when this PC reads
/// its value (no grey placeholder lines): Core Average, the CPU package's
/// temperature and power, one line per GPU with its Board Power and
/// temperature, then S.M.A.R.T. ("Healthy", or the first disk's problem).
/// Later lines (pinned sensors) are appended after these.
pub fn tile_lines(frame: &Frame, fahrenheit: bool) -> Vec<String> {
    let read = |id: &str| frame.reading(id).filter(|r| r.value.is_some()).map(|r| r.text_in(fahrenheit));
    let mut lines: Vec<String> = [("Core Average", CORE_AVERAGE), ("Package temperature", PACKAGE_TEMPERATURE), ("Package power", PACKAGE_POWER)]
        .into_iter()
        .filter_map(|(name, id)| read(id).map(|v| format!("{name}: {v}")))
        .collect();
    for card in frame.devices.iter().filter(|d| d.device.kind == DeviceKind::Gpu) {
        let values: Vec<String> = [crate::sensors::gpu::BOARD_POWER, crate::sensors::gpu::TEMPERATURE].into_iter().filter_map(|what| read(&crate::sensors::gpu::metric(&card.device, what))).collect();
        if !values.is_empty() {
            lines.push(format!("{}: {}", card.device.name, values.join(", ")));
        }
    }
    lines.extend(smart_line(frame));
    lines
}

/// "S.M.A.R.T.: Healthy" when every disk read says so, else the first
/// disk's problem; `None` when no disk's health is read.
fn smart_line(frame: &Frame) -> Option<String> {
    let mut health = frame.readings().filter(|r| r.group == Group::Smart && r.id.ends_with(".smart.health") && r.value.is_some()).map(|r| r.text());
    let first = health.next()?;
    let problem = std::iter::once(first).chain(health).find(|t| t != HEALTHY);
    Some(format!("{}: {}", Group::Smart.name(), problem.as_deref().unwrap_or(HEALTHY)))
}

/// The warning shown before the user agrees to the Manual SMART Probe.
pub const SMART_PROBE_WARNING: &str = "This manually sends read-only storage queries to physical-drive drivers and firmware. \
Some defective storage stacks have frozen Windows during these queries.";

/// The question under the warning.
pub const SMART_PROBE_QUESTION: &str = "Probe SMART data once now?";

/// Its choices in Settings (saved under its id, kept while it is off).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// Its tile on Summary (keeps the sensors running while Summary shows).
    pub summary_tile: bool,
    /// Show the grey rows of readings that are not available.
    pub show_unavailable: bool,
    /// The user agreed to the Manual SMART Probe's warning.
    pub smart_probe: bool,
    /// The time the side panel's graph covers, in seconds.
    pub history_window_s: u64,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { summary_tile: false, show_unavailable: true, smart_probe: false, history_window_s: DEFAULT_HISTORY_WINDOW_S }
    }
}

/// The graph's time window unless the user picks another.
pub const DEFAULT_HISTORY_WINDOW_S: u64 = 60;

/// The graph windows offered in Settings (seconds).
const HISTORY_WINDOWS_S: [u64; 4] = [60, 120, 300, 600];

impl Prefs {
    /// The time the sensor service keeps of each metric (at least 1 s).
    pub fn history_window(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.history_window_s.max(1))
    }
}

/// What it may load, shown in Settings › Add-ons ("Runs ...").
const VENDOR_LIBRARIES: &str = "the hardware makers' libraries (NVML, ADLX, IGCL, AMD's SDK, Intel DTT) if installed";

/// Everything it saves under `addons.power-freq.settings`: the selected
/// sensor, the closed rows and its [`Prefs`], under their field names.
/// Settings it cannot read give the defaults ([`read_settings`]).
#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Saved {
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<String>,
    collapsed: BTreeSet<String>,
    #[serde(flatten)]
    prefs: Prefs,
    /// Its alerts; the key is written only while one exists.
    #[serde(deserialize_with = "alerts::lenient", skip_serializing_if = "Vec::is_empty")]
    alerts: Vec<alerts::Rule>,
    /// Its pinned sensors in order; the key is written only while one exists.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pinned: Vec<String>,
}

/// The add-on. It holds an interest in the sensor service only while its
/// page or its Summary tile is shown.
#[derive(Default)]
pub struct PowerFreq {
    context: Option<AddonContext>,
    /// What the page reads sensors through, set only while it draws (the
    /// host owns the interest behind it).
    drawing: Option<Sensors>,
    /// Whether the page / the Summary tile were drawn and not yet hidden: the
    /// statistics reset when neither is.
    page_on: bool,
    tile_on: bool,
    /// The sensor service's statistics as of the last frame (the service
    /// owns them, so they survive this page while anyone holds interest).
    stats: std::sync::Arc<Stats>,
    /// Open rows and "Find": kept while the page is hidden and off.
    view: View,
    /// The sensor shown in the side panel (metric id), `None` = Core Average.
    selected: Option<String>,
    /// Its saved settings, kept as given (other fields survive).
    saved: serde_json::Value,
    prefs: Prefs,
    /// The Manual SMART Probe's warning is shown, waiting for an answer.
    probe_asking: bool,
    /// The alerts set in Settings (saved under `alerts`).
    alerts: Vec<alerts::Rule>,
    /// Runs while it is on and at least one alert exists, page open or
    /// not: it holds an interest in the sensor service.
    watcher: Option<watch::Watcher>,
    /// The sensors pinned to the Summary tile (saved under `pinned`).
    pinned: Vec<String>,
    /// Turned on (between [`Addon::start`] and [`Addon::stop`]).
    on: bool,
    /// The app's temperature unit as last drawn (for the notifications).
    fahrenheit: bool,
    /// What Flight Recorder can do now, told by the host before each draw.
    recording: Recording,
    /// Where its features' tiers are read ([`crate::addons::TIERS`]).
    tiers: Tiers,
}

/// Where the add-on reads its features' tiers: [`crate::addons::tier_of`]
/// (tests give another table).
#[derive(Clone, Copy)]
pub struct Tiers(pub fn(&str) -> Tier);

impl Default for Tiers {
    fn default() -> Self {
        Tiers(crate::addons::tier_of)
    }
}

/// Said where a feature whose [`crate::addons::TIERS`] line is not Free
/// would be: there is no way to buy one yet, so it is simply not offered.
pub const NOT_INCLUDED: &str = "Not included in this version.";

/// The page's "Record with Flight Recorder" button.
pub const RECORD_WITH_FLIGHT_RECORDER: &str = "Record with Flight Recorder";

/// Why the button is off while Flight Recorder is off.
pub const FLIGHT_RECORDER_OFF: &str = "Flight Recorder is off: turn it on in Settings \u{203a} Add-ons to record.";

/// Shown next to Stop while Flight Recorder records.
pub const RECORDING_NOW: &str = "Recording\u{2026}";

/// "Record with Flight Recorder" as the page shows it, from Flight
/// Recorder's state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecordAction {
    /// The button; off with the reason when it holds one.
    Record(Option<&'static str>),
    /// "Recording…" with Stop.
    Recording,
}

/// The action for Flight Recorder's state `recording`: off with the
/// reason while it is off or locked, Stop while it records.
pub fn record_action(recording: Recording) -> RecordAction {
    match recording {
        Recording::Off => RecordAction::Record(Some(FLIGHT_RECORDER_OFF)),
        Recording::Locked(why) => RecordAction::Record(Some(why)),
        Recording::Ready => RecordAction::Record(None),
        Recording::Running => RecordAction::Recording,
    }
}

impl PowerFreq {
    /// "Restart as administrator": this app again, elevated, on this page.
    pub fn restart_as_administrator(&self) -> Result<(), String> {
        let ctx = self.context.as_ref().ok_or("Power & Freq is off.")?;
        ctx.restart_elevated(self.id())
    }

    /// "Manual SMART Probe", after the user accepted the warning: saves the
    /// consent, then reads the disks at once when elevated, or reopens
    /// this page elevated (the elevated copy finds the consent saved).
    pub fn manual_smart_probe(&mut self) -> Result<(), String> {
        self.prefs.smart_probe = true;
        self.share_probe_consent();
        let ctx = self.context.as_ref().ok_or("Power & Freq is off.")?;
        if !ctx.is_admin() {
            return self.restart_as_administrator();
        }
        self.refresh();
        Ok(())
    }

    /// Gives the disk provider the saved Manual SMART Probe consent.
    fn share_probe_consent(&self) {
        crate::sensors::disk::set_probe_consent(self.prefs.smart_probe);
    }

    /// "Get AMD's add-on": AMD's page of the Ryzen Master Monitoring SDK.
    pub fn get_amd_addon(&self) -> Result<(), String> {
        let ctx = self.context.as_ref().ok_or("Power & Freq is off.")?;
        ctx.open(crate::sensors::ryzen::AMD_URL)
    }

    /// The metric id shown in the side panel.
    pub fn selected(&self) -> &str {
        self.selected.as_deref().unwrap_or(CORE_AVERAGE)
    }

    /// Shows this sensor in the side panel (remembered in its settings).
    pub fn select(&mut self, id: &str) {
        self.selected = Some(id.to_string());
    }

    /// What pressing "Record with Flight Recorder" (or its Stop) asks the
    /// app for now; None while the action is off.
    pub fn record_request(&self) -> Option<Action> {
        match record_action(self.recording) {
            RecordAction::Record(None) => Some(Action::RecordSensors(true)),
            RecordAction::Record(Some(_)) => None,
            RecordAction::Recording => Some(Action::RecordSensors(false)),
        }
    }

    /// "Copy reading" of the selected sensor, if this frame has it.
    pub fn copy_reading(&self, frame: &Frame) -> Option<Action> {
        panel::Detail::of(frame, &self.stats, self.view.fahrenheit, self.selected()).map(|d| Action::Copy(d.copy_text()))
    }

    /// "Reset statistics": the sensor service starts a new epoch now.
    pub fn reset_statistics(&mut self) {
        let held = self.context.as_ref().and_then(|c| c.leases().held_by(self.id()));
        if let Some(s) = self.drawing.as_ref().or(held.as_ref()) {
            s.reset_stats();
            self.stats = s.stats();
        }
    }

    /// The latest frame (with the service's statistics) through what the
    /// host gave this draw (page or tile); asking is what makes the host keep
    /// the service running while it is drawn.
    fn frame(&mut self, interval_ms: u64) -> Option<std::sync::Arc<Frame>> {
        let sensors = self.drawing.as_ref()?;
        let frame = latest(sensors, interval_ms, self.prefs.history_window())?;
        if self.stats.seq() != Some(frame.seq) {
            self.stats = sensors.stats();
        }
        Some(frame)
    }
}

/// The latest frame of `sensors`, read at the app's update speed, keeping
/// `window` of history.
fn latest(sensors: &Interest, interval_ms: u64, window: std::time::Duration) -> Option<std::sync::Arc<Frame>> {
    sensors.set_interval_ms(interval_ms);
    sensors.set_history_window(window);
    sensors.latest()
}

impl Addon for PowerFreq {
    fn id(&self) -> &'static str {
        "power-freq"
    }
    fn name(&self) -> &'static str {
        "Power & Freq"
    }
    fn aliases(&self) -> &'static [&'static str] {
        // A longer page id.
        &["power-and-freq"]
    }
    fn description(&self) -> &'static str {
        "Hardware sensors of the CPU, GPUs, NPU, disks, power and battery: clocks, load, temperatures, power and S.M.A.R.T."
    }
    /// Administrator only for some sensors (S.M.A.R.T., AMD's SDK); no
    /// internet; the hardware makers' libraries when they are installed.
    fn requirements(&self) -> Requirements {
        Requirements { admin: true, network: false, programs: &[VENDOR_LIBRARIES], note: None }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Low", network: "None", disk: "None", when: "While its page or its Summary tile is shown, or while an alert is set" }
    }

    fn start(&mut self, context: &AddonContext) {
        self.context = Some(context.clone());
        self.share_probe_consent();
        self.on = true;
        self.sync_alerts();
    }

    fn stop(&mut self) {
        self.on = false;
        self.watcher = None;
        self.drawing = None;
        (self.page_on, self.tile_on) = (false, false);
        self.stats = Default::default();
    }

    fn page_hidden(&mut self) {
        // The host drops the page's sensors itself.
        self.page_on = false;
        if !self.tile_on {
            // The service keeps them while anyone else holds interest; the
            // next frame brings them back.
            self.stats = Default::default();
        }
    }

    fn summary_hidden(&mut self) {
        self.tile_on = false;
        if !self.page_on {
            self.stats = Default::default();
        }
    }

    fn recording_shown(&mut self, recording: Recording) {
        self.recording = recording;
    }

    /// The values of the whole PC this PC reads ([`tile_lines`]).
    fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.tile_on = self.prefs.summary_tile;
        if !self.tile_on {
            return;
        }
        self.drawing = page.sensors();
        let frame = self.frame(page.settings.interval_ms());
        // Between frames the host alone holds the interest.
        self.drawing = None;
        let Some(frame) = frame else {
            crate::addons::summary_tile(ui, page, "Power & Freq", vec![WAITING.to_string()]);
            return;
        };
        crate::addons::summary_tile(ui, page, "Power & Freq", tile_lines(&frame, page.settings.fahrenheit));
        self.pinned_rows(ui, &frame, page);
    }

    fn settings_section(&mut self, ui: &mut Ui, page: &mut PageContext) {
        use crate::widgets::parts::{group_heading, setting_note, setting_switch};
        let _ = page;
        group_heading(ui, "Readings");
        setting_switch(ui, "Show readings that are not available", None, &mut self.prefs.show_unavailable);
        if setting_switch(ui, "Allow the Manual SMART Probe", None, &mut self.prefs.smart_probe) {
            self.share_probe_consent();
        }
        setting_note(ui, SMART_PROBE_WARNING);
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("Reads the sensors while Summary is open"), &mut self.prefs.summary_tile);
        group_heading(ui, "Sensor graph");
        ui.horizontal(|ui| {
            ui.label("The sensor graph shows the");
            let shown = |s: u64| crate::sensors::service::History::new(std::time::Duration::from_secs(s)).caption("").to_lowercase();
            egui::ComboBox::from_id_salt("power_history_window").selected_text(shown(self.prefs.history_window_s)).show_ui(ui, |ui| {
                for s in HISTORY_WINDOWS_S {
                    ui.selectable_value(&mut self.prefs.history_window_s, s, shown(s));
                }
            });
        });
        self.alerts_section(ui, page);
    }

    /// Its choices (selected sensor, closed rows, Settings), with the
    /// other fields it was given; `Null` while nothing was chosen or saved.
    fn settings(&self) -> serde_json::Value {
        let untouched = self.selected.is_none() && self.view.collapsed.is_empty() && self.prefs == Prefs::default() && self.alerts.is_empty() && self.pinned.is_empty();
        if self.saved.is_null() && untouched {
            return serde_json::Value::Null;
        }
        let mut settings = if self.saved.is_object() { self.saved.clone() } else { serde_json::json!({}) };
        let mine = Saved { selected: self.selected.clone(), collapsed: self.view.collapsed.clone(), prefs: self.prefs, alerts: self.alerts.clone(), pinned: self.pinned.clone() };
        if let (Some(all), Ok(serde_json::Value::Object(fields))) = (settings.as_object_mut(), serde_json::to_value(mine)) {
            all.extend(fields);
            if self.prefs.history_window_s == DEFAULT_HISTORY_WINDOW_S {
                // The default is not written, as before the setting existed.
                all.remove("history_window_s");
            }
            if self.alerts.is_empty() {
                // The last alert was deleted: no key, as before alerts existed.
                all.remove("alerts");
            }
            if self.pinned.is_empty() {
                // The last pin was removed: no key, as before pins existed.
                all.remove("pinned");
            }
        }
        settings
    }

    /// Reads its choices; settings it cannot read give the defaults.
    fn load_settings(&mut self, settings: &serde_json::Value) {
        let Saved { selected, collapsed, prefs, alerts, pinned } = read_settings::<Saved>(settings);
        (self.selected, self.view.collapsed, self.prefs, self.alerts, self.pinned) = (selected, collapsed, prefs, alerts, pinned);
        self.saved = settings.clone();
        self.share_probe_consent();
        self.sync_alerts();
    }

    /// The alerts' watcher, while it runs.
    fn threads(&self) -> usize {
        self.watcher.as_ref().map_or(0, |w| usize::from(w.running()))
    }

    /// Drawn on every page: keeps the alerts' unit in step with the app's
    /// and names the triggered alerts in red.
    fn status_bar(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.fahrenheit = page.settings.fahrenheit;
        self.sync_alerts();
        let triggered = self.watcher.as_ref().map(|w| w.tracked()).unwrap_or_default().into_iter().filter(|t| t.status.active).count();
        if triggered > 0 {
            let words = if triggered == 1 { "1 alert triggered".to_string() } else { format!("{triggered} alerts triggered") };
            ui.label(RichText::new(words).color(theme::of_ui(ui).red));
        }
    }

    fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.page_on = true;
        self.drawing = page.sensors();
        let frame = self.frame(page.settings.interval_ms());
        let user = page.settings.theme();
        let accent = user.graphs.value;
        (self.view.now, self.view.interval_ms, self.view.fahrenheit) = (Some(Instant::now()), page.settings.interval_ms(), page.settings.fahrenheit);
        self.fahrenheit = page.settings.fahrenheit;
        self.view.hide_unavailable = !self.prefs.show_unavailable;
        let rows = tree_view(frame.as_deref(), &self.stats, &self.view);
        let top = frame.as_deref().map(|f| status(f, page.settings.fahrenheit));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let width = ui.available_width() - 4.0;
            ui.spacing_mut().item_spacing.y = 12.0;
            let offer = ui.scope(|ui| {
                ui.set_width(width);
                self.status_band(ui, frame.as_deref().zip(top.as_ref()), accent)
            });
            if let Some(offer) = offer.inner {
                self.take_offer(ui, offer, page);
            }
            self.record_row(ui, page);
            if let (Some(f), Some(s)) = (frame.as_deref(), &top) {
                self.tiles(ui, f, s, width);
            }
            const PANEL: f32 = 300.0;
            const GAP: f32 = 18.0;
            if width >= 760.0 {
                ui.horizontal_top(|ui| {
                    let tree_w = width - PANEL - GAP;
                    ui.vertical(|ui| {
                        ui.set_width(tree_w);
                        self.tree_box(ui, rows.as_deref(), frame.as_deref(), tree_w, &user);
                    });
                    ui.add_space(GAP - ui.spacing().item_spacing.x * 2.0);
                    ui.vertical(|ui| {
                        ui.set_width(PANEL);
                        self.side_panel(ui, frame.as_deref(), page);
                    });
                });
            } else {
                self.tree_box(ui, rows.as_deref(), frame.as_deref(), width, &user);
                self.side_panel(ui, frame.as_deref(), page);
            }
        });
        // Between frames the host alone holds the interest.
        self.drawing = None;
    }
}

/// The tree's minimum width; narrower, it scrolls sideways in its box.
const TREE_MIN_WIDTH: f32 = 732.0;

/// Width of the status band's right-hand column (its buttons and the
/// clock scale).
const BAND_ACTIONS: f32 = 280.0;

impl PowerFreq {
    /// The status band (Design A): a dot (amber while AMD's add-on or
    /// administrator would unlock sensors), the sentence, the details
    /// under it and why sensors are locked; on the right, the buttons that
    /// unlock them and the clock against the base. Returns the button
    /// clicked.
    fn status_band(&self, ui: &mut Ui, now: Option<(&Frame, &Status)>, accent: Color32) -> Option<notice::Offer> {
        let t = theme::of_ui(ui);
        let Some((frame, top)) = now else {
            StatusBand::new(Mark::Busy, WAITING).actions_width(0.0).show(ui, |_, _| {});
            return None;
        };
        let admin = self.context.as_ref().is_some_and(|c| c.is_admin());
        let lines = notice::notice(frame, admin).map(|n| n.lines()).unwrap_or_default();
        let mark = Mark::Dot(if lines.is_empty() { t.green } else { t.amber });
        let sub = top.details.join("  ·  ");
        let mut clicked = None;
        StatusBand::new(mark, &top.band).sub(&sub).actions_width(BAND_ACTIONS).show(ui, |ui, area| match area {
            BandArea::Body if !lines.is_empty() => {
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(r, egui::CornerRadius::ZERO, t.line);
                for (_, text) in &lines {
                    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(t.warm_note_text)).wrap());
                }
            }
            BandArea::Actions => {
                ui.spacing_mut().item_spacing.y = 6.0;
                for (offer, _) in &lines {
                    let button = match offer {
                        notice::Offer::GetAddon => primary_button(ui, "Get AMD's add-on", true),
                        notice::Offer::Restart => admin_button(ui, "Restart as administrator", false, true),
                    };
                    if button.clicked() {
                        clicked = Some(*offer);
                    }
                }
                ui.vertical(|ui| self.clock_scale(ui, frame, BAND_ACTIONS, accent));
            }
            _ => {}
        });
        clicked
    }

    /// Core Average on a 0 to 6 GHz track (wider if it ran faster), with a
    /// tick at the base clock.
    fn clock_scale(&self, ui: &mut Ui, frame: &Frame, width: f32, accent: Color32) {
        let value = |id: &str| frame.reading(id).and_then(|r| r.value);
        let Some(avg) = value(CORE_AVERAGE) else { return };
        let base = value(BASE_HZ);
        let seen = self.stats.min_max(CORE_AVERAGE).map_or(avg, |(_, hi)| hi);
        let top = (seen.max(avg) / 1e9).ceil().max(6.0) * 1e9;
        let sub = theme::of_ui(ui).label;
        let small = |t: String| RichText::new(t).size(11.5).color(sub);
        ui.set_max_width(width);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.horizontal(|ui| {
            ui.label(small("Core Average".into()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(small(format!("max seen {}", Unit::Hz.format(seen)))));
        });
        meter_with_tick(ui, width, (avg / top) as f32, base.map(|b| (b / top) as f32), accent);
        ui.horizontal(|ui| {
            ui.label(small("0 GHz".into()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.label(small(Unit::Hz.format(top)));
                if let Some(b) = base {
                    ui.add_space((width / 2.0 - 110.0).max(4.0));
                    ui.label(small(format!("tick = {} base", Unit::Hz.format(b))));
                }
            });
        });
    }

    /// "Record with Flight Recorder" (off with the reason when Flight
    /// Recorder is off or locked), or "Recording…" with Stop while it
    /// records; pressed, it asks the app through the add-on host.
    fn record_row(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let sub = page.sub_text();
        ui.horizontal(|ui| {
            let pressed = match record_action(self.recording) {
                RecordAction::Record(reason) => {
                    let button = outline_button(ui, RECORD_WITH_FLIGHT_RECORDER, reason.is_none());
                    let pressed = button.on_disabled_hover_text(reason.unwrap_or_default()).clicked();
                    if let Some(why) = reason {
                        ui.label(RichText::new(why).size(12.0).color(sub));
                    }
                    pressed
                }
                RecordAction::Recording => {
                    ui.label(RichText::new(RECORDING_NOW).strong());
                    outline_button(ui, "Stop", true).on_hover_text("Stop the Flight Recorder recording").clicked()
                }
            };
            if pressed {
                page.actions.extend(self.record_request());
            }
        });
    }

    /// Does what a status band button offers: AMD's add-on page, or a
    /// restart as administrator.
    fn take_offer(&mut self, ui: &mut Ui, offer: notice::Offer, page: &mut PageContext) {
        let result = match offer {
            notice::Offer::GetAddon => self.get_amd_addon(),
            notice::Offer::Restart => {
                let r = self.restart_as_administrator();
                if r.is_ok() {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                }
                r
            }
        };
        if let Err(e) = result {
            page.actions.push(Action::Say(e));
        }
    }

    /// The four KPI tiles in a row (two rows of two when narrow).
    fn tiles(&self, ui: &mut Ui, frame: &Frame, top: &Status, width: f32) {
        let notes = tile_notes(frame, &self.stats);
        let cols = if width >= 640.0 { 4 } else { 2 };
        let gap = 10.0;
        let tile_w = (width - gap * (cols - 1) as f32) / cols as f32;
        let tiles: Vec<_> = top.tiles.iter().zip(&top.missing).zip(&notes).collect();
        for chunk in tiles.chunks(cols) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for (((label, value), missing), note) in chunk {
                    kpi_tile(ui, tile_w, label, value, note.as_deref().filter(|_| !**missing), **missing);
                }
            });
        }
    }

    /// Find, Expand all / Collapse all, then the tree in its box.
    fn tree_box(&mut self, ui: &mut Ui, rows: Option<&[Row]>, frame: Option<&Frame>, width: f32, user: &theme::Theme) {
        let t = theme::of_ui(ui);
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            search_field(ui, &mut self.view.find, "Find a sensor", (width - 260.0).clamp(160.0, 320.0));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if outline_button(ui, "Reset statistics", frame.is_some()).on_hover_text("Min, Avg and Max start again from now").clicked() {
                    self.reset_statistics();
                }
                if frame.is_some() {
                    ui.label(RichText::new(format!("Statistics {}", self.stats.since_text())).size(12.0).color(t.label));
                }
                if outline_button(ui, "Collapse all", true).clicked() {
                    if let Some(f) = frame {
                        self.view.collapse_all(f);
                    }
                }
                if outline_button(ui, "Expand all", true).clicked() {
                    self.view.expand_all();
                }
            });
        });
        let mut picked = None;
        egui::Frame::new().stroke(egui::Stroke::new(1.0, t.line)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
            let inner = width - 10.0;
            ui.set_width(inner);
            if inner >= TREE_MIN_WIDTH {
                picked = self.tree_rows(ui, rows, inner, user);
            } else {
                egui::ScrollArea::horizontal().id_salt("power_tree").show(ui, |ui| picked = self.tree_rows(ui, rows, TREE_MIN_WIDTH, user));
            }
        });
        match picked {
            Some(Pick::Toggle(key)) => self.view.toggle(&key),
            Some(Pick::Select(id)) => self.select(&id),
            None => {}
        }
    }

    /// The header and rows of the tree, `width` wide, in `user`'s graph
    /// colours; what was clicked.
    fn tree_rows(&self, ui: &mut Ui, rows: Option<&[Row]>, width: f32, user: &theme::Theme) -> Option<Pick> {
        ui.spacing_mut().item_spacing.y = 0.0;
        tree_header(ui, width, ["Sensor", "Value", "Min", "Avg", "Max"]);
        let t = theme::of_ui(ui);
        let Some(rows) = rows else {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.add(egui::Spinner::new().size(14.0));
                ui.label(RichText::new(WAITING).color(t.label));
            });
            ui.add_space(24.0);
            return None;
        };
        let mut picked = None;
        let mut dot_color = hue(Hue::Grey, user);
        let alerted = self.alerted_ids();
        for r in rows {
            let stale_free = r.value.strip_suffix(" (stale)").filter(|_| r.stale).unwrap_or(&r.value);
            let mut row = TreeRow {
                depth: r.depth,
                lead: RowLead::Dot(dot_color),
                name: &r.sensor,
                strong: false,
                grey: r.unavailable,
                note: "",
                value: stale_free,
                missing: r.unavailable,
                stale: r.stale,
                min: &r.min,
                avg: &r.avg,
                max: &r.max,
                selected: !r.id.is_empty() && r.id == self.selected(),
            };
            let open = r.expanded.unwrap_or(false);
            if let Some(g) = r.group {
                let (icon, h) = group_look(g);
                dot_color = hue(h, user);
                row.lead = RowLead::Caret { open, icon: Some(icon) };
            } else if let Some(kind) = r.kind {
                row.strong = !r.empty;
                row.lead = if r.empty { RowLead::Icon(device_glyph(kind)) } else { RowLead::Caret { open, icon: Some(device_glyph(kind)) } };
                if !r.empty {
                    (row.note, row.value) = (&r.value, "");
                }
            }
            let alert = r.expanded.is_none() && alerted.contains(&r.id);
            if alert {
                (row.note, row.lead) = ("Alert", RowLead::Dot(t.red));
            }
            let response = tree_row(ui, &row, width);
            if alert {
                ui.painter().rect_filled(response.rect, 0.0, t.red.gamma_multiply(0.12));
            }
            if response.clicked() {
                picked = match (&r.expanded, r.empty) {
                    (Some(_), false) => Some(Pick::Toggle(r.key.clone())),
                    (None, _) => Some(Pick::Select(r.id.clone())),
                    _ => None,
                };
            }
        }
        let finding = self.view.find.trim();
        if !finding.is_empty() && rows.iter().all(|r| r.expanded.is_some()) {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                ui.label(RichText::new(format!("No sensor called \u{201c}{finding}\u{201d}.")).color(t.label));
            });
            ui.add_space(12.0);
        }
        picked
    }
}

/// A click in the tree.
enum Pick {
    Toggle(String),
    Select(String),
}

impl PowerFreq {
    /// "Manual SMART Probe" on a disk: the warning and question first,
    /// unless the user already agreed (saved in Settings).
    fn smart_probe_action(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let t = theme::of_ui(ui);
        let mut probe = false;
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0, t.line))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.label(RichText::new("Manual SMART Probe").font(egui::FontId::new(13.0, theme::semibold())).color(t.text));
                if self.probe_asking {
                    ui.label(RichText::new(SMART_PROBE_WARNING).size(12.5).color(t.label));
                    ui.label(RichText::new(SMART_PROBE_QUESTION).size(12.5).color(t.text));
                    ui.horizontal(|ui| {
                        probe = outline_button(ui, "Probe", true).clicked();
                        if probe || outline_button(ui, "Cancel", true).clicked() {
                            self.probe_asking = false;
                        }
                    });
                } else if outline_button(ui, "Manual SMART Probe", true).clicked() {
                    probe = self.prefs.smart_probe;
                    self.probe_asking = !probe;
                }
            });
        if probe {
            let elevated = self.context.as_ref().is_some_and(|c| c.is_admin());
            match self.manual_smart_probe() {
                Ok(()) if !elevated => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                Ok(()) => {}
                Err(e) => page.actions.push(Action::Say(e)),
            }
        }
    }

    /// The side panel (Design A): selected sensor, Sources on this PC,
    /// EcoQoS note.
    fn side_panel(&mut self, ui: &mut Ui, frame: Option<&Frame>, page: &mut PageContext) {
        let t = theme::of_ui(ui);
        let value_ink = page.settings.theme().graphs.value;
        egui::Frame::new().fill(t.panel).stroke(egui::Stroke::new(1.0, t.line)).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(20)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 12.0;
            let Some(frame) = frame else {
                ui.label(RichText::new(WAITING).color(t.label));
                return;
            };
            match panel::Detail::of(frame, &self.stats, self.view.fahrenheit, self.selected()) {
                None => {
                    ui.add(egui::Label::new(RichText::new(crate::widgets::parts::EMPTY_DETAIL).color(t.label)).wrap());
                }
                Some(d) => {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        ui.add(egui::Label::new(RichText::new(&d.name).font(display_font(ui, 15.0)).color(t.text)).wrap());
                        ui.label(RichText::new(format!("{} › {}", d.device, d.group)).size(12.0).color(t.label));
                    });
                    if d.unavailable {
                        ui.add(egui::Label::new(RichText::new(&d.value).size(15.0).italics().color(t.faint)).wrap());
                    } else {
                        ui.label(RichText::new(&d.value).font(display_font(ui, 26.0)).color(value_ink));
                    }
                    let pairs = |ui: &mut Ui, id: &str, items: &[(&str, &str)]| {
                        egui::Grid::new(id).num_columns(2).spacing(vec2(12.0, 6.0)).show(ui, |ui| {
                            for (k, v) in items {
                                ui.label(RichText::new(*k).size(13.0).color(t.label));
                                ui.add(egui::Label::new(RichText::new(*v).size(13.0).color(t.text)).wrap());
                                ui.end_row();
                            }
                        });
                    };
                    if let Some(s) = &d.stats {
                        pairs(ui, "power_min_max", &[("Min", &s.min), ("Avg", &s.avg), ("Max", &s.max)]);
                        ui.label(RichText::new(format!("Statistics {}", s.since)).size(11.5).color(t.faint));
                    }
                    let scale = |v: f64| if d.base_hz.is_some() { (v / 1e9) as f32 } else { v as f32 };
                    let graph = self.drawing.as_ref().filter(|_| !d.unavailable).map(|s| s.history_graph(&d.id, crate::widgets::HISTORY)).unwrap_or_default();
                    let history: Vec<f32> = graph.into_iter().map(scale).collect();
                    let base = d.base_hz.map(scale);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        sparkline(ui, vec2(ui.available_width(), 70.0), &history, base, value_ink);
                        let shown = self.drawing.as_ref().map(|s| s.history_caption(&d.id)).unwrap_or_else(|| "Last minute".into());
                        let caption = d.graph_caption(&shown);
                        ui.label(RichText::new(caption).size(11.5).color(t.faint));
                    });
                    pairs(ui, "power_source", &[("Read from", d.source)]);
                }
            }
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 16.0;
                if frame.reading(self.selected()).is_some() && ui.link(RichText::new("Copy reading").size(12.5)).clicked() {
                    page.actions.extend(self.copy_reading(frame));
                }
                let id = self.selected().to_string();
                if self.pins_offered() && frame.reading(&id).is_some() && ui.link(RichText::new(self.pin_label(&id)).size(12.5)).clicked() {
                    self.toggle_pin(&id);
                }
                if ui.link(RichText::new("Open Windows power options").size(12.5)).clicked() {
                    page.actions.push(Action::OpenPowerOptions);
                }
            });
            let on_disk = frame.reading(self.selected()).is_some_and(|r| r.device.kind == DeviceKind::Disk);
            if on_disk {
                self.smart_probe_action(ui, page);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 7.0;
                let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(r, egui::CornerRadius::ZERO, t.line);
                ui.label(RichText::new("Sources on this PC").font(egui::FontId::new(13.0, theme::semibold())).color(t.text));
                for (name, status) in panel::sources(frame) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let working = status == "Working";
                        dot(ui, if working { t.green } else { t.faint });
                        ui.label(RichText::new(name).size(13.0).color(t.text));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add(egui::Label::new(RichText::new(&status).size(11.5).color(t.faint)).truncate()).on_hover_text(&status);
                        });
                    });
                }
            });
            neutral_note(ui, panel::ECOQOS_NOTE);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::AddonContext;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use std::time::{Duration, Instant};

    fn draw(p: &mut PowerFreq) {
        let (settings, snap) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut page = PageContext::new(&settings, &snap);
                // As the host draws the page: its sensors come from the host's leases.
                page.lease = p.context.as_ref().map(|c| c.leases().lease(crate::addons::leases::Surface::Page("power-freq")));
                p.ui(ui, &mut page)
            });
        });
    }

    /// Alerts and pins as if their [`crate::addons::TIERS`] lines said Paid.
    fn paid_features(id: &str) -> Tier {
        if id == alerts::TIER_ID || id == PINS_TIER_ID {
            Tier::Paid
        } else {
            crate::addons::tier_of(id)
        }
    }

    #[test]
    fn alerts_and_pins_follow_their_tier_lines() {
        let shipped = PowerFreq::default();
        assert_eq!(shipped.alerts_offered(), crate::addons::tier_of("power-freq.alerts") == Tier::Free);
        assert_eq!(shipped.pins_offered(), crate::addons::tier_of("power-freq.pins") == Tier::Free);
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq { tiers: Tiers(paid_features), ..PowerFreq::default() };
        p.load_settings(&serde_json::json!({ "alerts": [always_triggered()], "pinned": [BASE_HZ] }));
        p.start(&ctx);
        assert!(!p.alerts_offered() && !p.pins_offered());
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(p.threads(), 0, "a Paid alerts line: saved alerts are not watched");
        p.add_alert();
        assert_eq!(p.alerts().len(), 1, "nor added");
        p.toggle_pin(CORE_AVERAGE);
        assert!(!p.is_pinned(CORE_AVERAGE), "a Paid pins line: nothing is pinned");
        assert_eq!(p.settings()["pinned"], serde_json::json!([BASE_HZ]), "the saved choices are kept");
        p.stop();
    }

    fn until(what: &str, mut cond: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(5);
        while !cond() {
            assert!(Instant::now() < end, "timed out waiting: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Draws the page until the first frame arrived.
    fn shown(p: &mut PowerFreq) {
        until("the first frame", || {
            draw(p);
            p.stats.min_max(CORE_AVERAGE).is_some()
        });
    }

    #[test]
    fn record_with_flight_recorder_follows_flight_recorders_state() {
        use crate::addons::flight_recorder::PAUSED;
        assert_eq!(record_action(Recording::Ready), RecordAction::Record(None));
        assert_eq!(record_action(Recording::Off), RecordAction::Record(Some(FLIGHT_RECORDER_OFF)), "off, saying why");
        assert_eq!(record_action(Recording::Locked(PAUSED)), RecordAction::Record(Some(PAUSED)), "locked, saying why");
        assert_eq!(record_action(Recording::Running), RecordAction::Recording, "Recording… with Stop");
    }

    #[test]
    fn pressing_the_action_asks_the_host_to_start_or_stop_the_recording() {
        let mut p = PowerFreq::default();
        assert_eq!(p.record_request(), None, "Flight Recorder is off until the host says otherwise");
        p.recording_shown(Recording::Ready);
        assert_eq!(p.record_request(), Some(Action::RecordSensors(true)));
        p.recording_shown(Recording::Running);
        assert_eq!(p.record_request(), Some(Action::RecordSensors(false)));
        p.recording_shown(Recording::Locked(crate::addons::flight_recorder::SAVE_OR_DISCARD_FIRST));
        assert_eq!(p.record_request(), None);
    }

    #[test]
    fn restart_as_administrator_reopens_this_page_elevated() {
        let (ctx, runner) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        assert!(p.restart_as_administrator().is_err(), "the test executor launches nothing");
        let args = vec!["--page".to_string(), "power-freq".to_string()];
        assert_eq!(runner.launches(), vec![crate::platform::Launch::Elevated(args)]);
    }

    #[test]
    fn the_manual_smart_probe_remembers_the_consent_and_asks_for_administrator() {
        let (ctx, runner) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        assert!(runner.launches().is_empty(), "nothing probes until the user asks");
        assert!(p.manual_smart_probe().is_err(), "the test executor launches nothing");
        let args = vec!["--page".to_string(), "power-freq".to_string()];
        assert_eq!(runner.launches(), vec![crate::platform::Launch::Elevated(args)], "not elevated: reopens this page elevated");
        assert_eq!(p.settings()["smart_probe"], serde_json::Value::Bool(true), "consent saved for the elevated copy");
    }

    #[test]
    fn get_amds_add_on_opens_amds_page() {
        let (ctx, runner) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        let _ = p.get_amd_addon();
        assert_eq!(runner.launches(), vec![crate::platform::Launch::Open(crate::sensors::ryzen::AMD_URL.into())]);
    }

    #[test]
    fn turning_it_on_reads_no_sensor_until_its_page_is_shown() {
        let (ctx, runner) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ctx.sensors().threads(), 0);
        assert!(runner.launches().is_empty(), "starting launches nothing");
    }

    #[test]
    fn the_sensors_run_while_the_page_is_shown_and_stop_when_it_is_hidden() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        assert!(ctx.sensors().threads() > 0);
        ctx.leases().end_frame();
        ctx.leases().end_frame(); // the host drops the page's sensors one frame after
        p.page_hidden();
        until("no sensor thread", || ctx.sensors().threads() == 0);
        assert_eq!(p.stats.min_max(CORE_AVERAGE), None, "nobody else holds interest: the statistics are gone");
    }

    #[test]
    fn statistics_survive_leaving_and_reopening_the_page_while_someone_holds_interest() {
        let (ctx, _) = AddonContext::recording();
        let recorder = ctx.sensors().interest(); // e.g. the Flight Recorder
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        let since = p.stats.since();
        p.page_hidden();
        draw(&mut p);
        assert!(p.stats.min_max(CORE_AVERAGE).is_some(), "reopened: the statistics went on");
        assert_eq!(p.stats.since(), since, "same epoch");
        drop(recorder);
    }

    #[test]
    fn reset_statistics_starts_a_new_epoch_now() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        let before = crate::platform::local_secs();
        p.reset_statistics();
        assert!(p.stats.since() >= before, "the epoch is the reset's time");
        assert!(p.stats.since_text().starts_with("since "));
    }

    #[test]
    fn turning_it_off_stops_the_sensors_and_forgets_min_and_max() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        p.stop();
        ctx.leases().release("power-freq"); // as the host does when it turns an add-on off
        until("no sensor thread", || ctx.sensors().threads() == 0);
        assert_eq!(p.stats.min_max(CORE_AVERAGE), None);
    }

    #[test]
    fn the_page_waits_for_the_first_answer_of_every_sensor() {
        assert_eq!(tree(None, &Stats::default()), None);
    }

    #[test]
    fn the_tree_shows_this_pc_the_cpu_its_groups_and_min_max() {
        let (ctx, _) = AddonContext::recording();
        let interest = ctx.sensors().interest();
        until("a frame", || interest.latest().is_some());
        let frame = interest.latest().unwrap();
        let mut stats = Stats::default();
        stats.observe(&frame);
        let rows = tree(Some(&frame), &stats).unwrap();
        let at = |name: &str| rows.iter().find(|r| r.sensor == name).unwrap_or_else(|| panic!("no row {name}"));
        assert_eq!((rows[0].depth, rows[0].sensor.as_str()), (0, "This PC"));
        assert_eq!(at("Test CPU").depth, 1);
        let groups: Vec<_> = rows.iter().filter(|r| r.depth == 2).map(|r| r.sensor.as_str()).collect();
        assert_eq!(groups, vec!["Utilization", "Clocks", "Limits", "Idle states", "Levels (Energy Perf)", "General"]);
        assert_eq!(at("Package Limit Reasons").value, "none");
        assert_eq!(at("Package C1").value, "80 %");
        assert_eq!(at("Governor").value, "Ultimate Performance");
        assert_eq!(at("Energy Performance Preference").value, "0 %");
        assert_eq!(at("Base Frequency").value, "4.70 GHz");
        let avg = at("Core Average");
        assert_eq!((avg.value.as_str(), avg.min.as_str(), avg.max.as_str()), ("5.17 GHz", "5.17 GHz", "5.17 GHz"));
        let parked = at("CPU 3 Parked");
        assert_eq!((parked.value.as_str(), parked.min.as_str()), ("Parked", ""), "a state has no Min/Max");
        assert_eq!(at("CPU 0 Max Clock").value, "5.10 GHz");
    }

    mod view {
        use super::super::*;
        use crate::sensors::model::{Device, DeviceKind, Group, Reading, Unit};
        use std::time::{Duration, Instant};

        /// A CPU (temperature, clocks) and a disk (S.M.A.R.T.), read at `at`,
        /// with Core Average at `ghz`.
        fn frame(seq: u64, ghz: f64, at: Instant) -> Frame {
            let cpu = Device::cpu("CPU");
            let disk = Device::new(DeviceKind::Disk, "disk0", "SSD");
            let readings = vec![
                Reading::measure("cpu.t", cpu.clone(), Group::Temperatures, "Package", Unit::Celsius, "test", at).with(Some(45.0), ""),
                Reading::measure(CORE_AVERAGE, cpu.clone(), Group::Clocks, "Core Average", Unit::Hz, "test", at).with(Some(ghz * 1e9), ""),
                Reading::measure("cpu.p", cpu, Group::Clocks, "CPU 0 Parked", Unit::YesNo { yes: "Parked", no: "Active" }, "test", at)
                    .with(Some(0.0), "")
                    .counter(),
                Reading::measure("disk.spare", disk.clone(), Group::Smart, "Available Spare", Unit::Percent, "test", at).with(None, "needs administrator"),
                Reading::measure("disk.hours", disk, Group::Smart, "Power On Hours", Unit::Hours, "test", at).with(Some(12_345.0), "").counter(),
            ];
            Frame::build(seq, &[readings], Vec::new())
        }

        /// The rows of the frame after 2.98 and 5.10 GHz were seen, now at 4.72.
        fn rows(view: &View) -> Vec<Row> {
            let t0 = Instant::now();
            // Three reads (statistics count each read once).
            let at = |seq: u64| t0 + Duration::from_millis(seq);
            let mut stats = Stats::default();
            for (seq, ghz) in [(1, 2.98), (2, 5.10), (3, 4.72)] {
                stats.observe(&frame(seq, ghz, at(seq)));
            }
            tree_view(Some(&frame(3, 4.72, at(3))), &stats, view).unwrap()
        }

        fn at<'a>(rows: &'a [Row], name: &str) -> &'a Row {
            rows.iter().find(|r| r.sensor == name).unwrap_or_else(|| panic!("no row {name}"))
        }

        fn names(rows: &[Row]) -> Vec<&str> {
            rows.iter().map(|r| r.sensor.as_str()).collect()
        }

        #[test]
        fn a_group_row_sums_up_its_first_reading_with_min_and_max() {
            let rows = rows(&View::default());
            let clocks = at(&rows, "Clocks");
            assert_eq!((clocks.value.as_str(), clocks.min.as_str(), clocks.max.as_str()), ("4.72 GHz", "2.98 GHz", "5.10 GHz"));
            assert_eq!(clocks.group, Some(Group::Clocks), "the group row knows its icon");
            assert_eq!(clocks.expanded, Some(true));
        }

        #[test]
        fn counters_show_only_their_value() {
            let rows = rows(&View::default());
            let hours = at(&rows, "Power On Hours");
            assert_eq!((hours.value.as_str(), hours.min.as_str(), hours.avg.as_str(), hours.max.as_str()), ("12,345 h", "", "", ""));
            let parked = at(&rows, "CPU 0 Parked");
            assert_eq!((parked.value.as_str(), parked.avg.as_str()), ("Active", ""), "a state has no Avg");
        }

        #[test]
        fn a_sensor_row_shows_value_min_avg_and_max() {
            let rows = rows(&View::default());
            let avg = at(&rows, "Core Average");
            assert_eq!((avg.value.as_str(), avg.min.as_str(), avg.avg.as_str(), avg.max.as_str()), ("4.72 GHz", "2.98 GHz", "4.27 GHz", "5.10 GHz"));
        }

        #[test]
        fn an_unavailable_reading_shows_why_never_zero() {
            let rows = rows(&View::default());
            let spare = at(&rows, "Available Spare");
            assert_eq!((spare.value.as_str(), spare.unavailable), ("needs administrator", true));
        }

        #[test]
        fn temperatures_follow_the_fahrenheit_setting() {
            let rows = rows(&View { fahrenheit: true, ..View::default() });
            assert_eq!(at(&rows, "Package").value, "113.0 °F");
            assert_eq!(at(&rows, "Package").min, "113.0 °F", "Min/Max too");
            assert_eq!(at(&rows, "Temperatures").value, "113.0 °F");
        }

        #[test]
        fn a_reading_older_than_three_intervals_is_stale() {
            let read = Instant::now();
            let mut stats = Stats::default();
            let f = frame(1, 4.72, read);
            stats.observe(&f);
            let view = |after_ms: u64, interval_ms: u64| View { now: Some(read + Duration::from_millis(after_ms)), interval_ms, ..View::default() };
            let fresh = tree_view(Some(&f), &stats, &view(2_900, 1_000)).unwrap();
            assert_eq!(at(&fresh, "Core Average").value, "4.72 GHz");
            let old = tree_view(Some(&f), &stats, &view(3_100, 1_000)).unwrap();
            assert_eq!(at(&old, "Core Average").value, "4.72 GHz (stale)");
            assert!(at(&old, "Core Average").stale);
            let paused = tree_view(Some(&f), &stats, &view(60_000, 0)).unwrap();
            assert!(!at(&paused, "Core Average").stale, "paused readings are not stale");
        }

        #[test]
        fn a_collapsed_group_hides_its_sensors_but_keeps_its_summary() {
            let mut view = View::default();
            view.collapsed.insert(at(&rows(&view), "Clocks").key.clone());
            let rows = rows(&view);
            assert_eq!(at(&rows, "Clocks").expanded, Some(false));
            assert_eq!(at(&rows, "Clocks").value, "4.72 GHz");
            assert!(!names(&rows).contains(&"Core Average"));
            assert!(names(&rows).contains(&"Package"), "other groups stay open");
        }

        #[test]
        fn a_collapsed_device_hides_its_groups() {
            let mut view = View::default();
            view.collapsed.insert(at(&rows(&view), "CPU").key.clone());
            assert_eq!(names(&rows(&view)), vec!["This PC", "CPU", "SSD", "S.M.A.R.T.", "Available Spare", "Power On Hours"]);
        }

        #[test]
        fn collapse_all_then_expand_all() {
            let at_ = Instant::now();
            let mut view = View::default();
            view.collapse_all(&frame(1, 4.72, at_));
            assert_eq!(names(&rows(&view)), vec!["This PC"]);
            view.expand_all();
            assert_eq!(rows(&view).len(), rows(&View::default()).len());
        }

        #[test]
        fn toggling_a_row_opens_and_closes_it() {
            let mut view = View::default();
            let key = at(&rows(&view), "SSD").key.clone();
            view.toggle(&key);
            assert_eq!(at(&rows(&view), "SSD").expanded, Some(false));
            view.toggle(&key);
            assert_eq!(at(&rows(&view), "SSD").expanded, Some(true));
        }

        #[test]
        fn hiding_unavailable_readings_drops_the_grey_rows_and_emptied_groups() {
            let view = View { hide_unavailable: true, ..View::default() };
            assert_eq!(
                names(&rows(&view)),
                vec!["This PC", "CPU", "Temperatures", "Package", "Clocks", "Core Average", "CPU 0 Parked", "SSD", "S.M.A.R.T.", "Power On Hours"]
            );
            let at_ = Instant::now();
            let cpu = Device::cpu("CPU");
            let grey = Reading::measure("cpu.v", cpu, Group::Voltages, "Core", Unit::Percent, "test", at_).with(None, "no provider on this PC");
            let mut frame = frame(1, 4.72, at_);
            frame = Frame::build(2, &[frame.readings().cloned().chain([grey]).collect()], Vec::new());
            let rows = tree_view(Some(&frame), &Stats::default(), &view).unwrap();
            assert!(!names(&rows).contains(&"Voltages"), "a group with nothing to show goes too");
            let mut closed = View { hide_unavailable: true, ..View::default() };
            closed.collapsed.insert(at(&rows, "CPU").key.clone());
            assert!(names(&rows_of(&frame, &closed)).contains(&"CPU"), "a closed device keeps its row");
        }

        fn rows_of(frame: &Frame, view: &View) -> Vec<Row> {
            tree_view(Some(frame), &Stats::default(), view).unwrap()
        }

        #[test]
        fn a_device_with_nothing_to_read_is_one_row_saying_why() {
            let at_ = Instant::now();
            let npu = Device::new(DeviceKind::Npu, "npu", "NPU");
            let none = Reading::measure("npu.present", npu, Group::General, "NPU", Unit::YesNo { yes: "Present", no: "None" }, "test", at_)
                .with(None, "No NPU in this PC")
                .counter();
            let f = frame(1, 4.72, at_);
            let f = Frame::build(2, &[f.readings().cloned().chain([none]).collect()], Vec::new());
            let rows = rows_of(&f, &View::default());
            let npu = at(&rows, "NPU");
            assert_eq!((npu.value.as_str(), npu.unavailable, npu.empty, npu.kind), ("No NPU in this PC", true, true, Some(DeviceKind::Npu)));
            assert_eq!(rows.iter().filter(|r| r.sensor == "NPU").count(), 1, "nothing under it");
            let cpu = at(&rows, "CPU");
            assert_eq!((cpu.empty, cpu.kind), (false, Some(DeviceKind::Cpu)), "a device with values opens as before");
            assert_eq!(rows[0].kind, Some(DeviceKind::ThisPc));
        }

        #[test]
        fn find_keeps_the_sensors_whose_name_matches_and_their_headings() {
            let mut view = View { find: "hours".into(), ..View::default() };
            view.collapsed.insert(at(&rows(&View::default()), "SSD").key.clone());
            assert_eq!(names(&rows(&view)), vec!["This PC", "SSD", "S.M.A.R.T.", "Power On Hours"], "a match shows through collapsed rows");
            let none = View { find: "hotspot".into(), ..View::default() };
            assert_eq!(names(&rows(&none)), vec!["This PC"]);
        }
    }

    #[test]
    fn the_open_rows_are_remembered_when_the_page_comes_back() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        p.view.toggle("d:cpu");
        p.page_hidden();
        shown(&mut p);
        assert!(p.view.collapsed.contains("d:cpu"));
    }

    #[test]
    fn the_panel_shows_core_average_until_a_sensor_is_selected() {
        assert_eq!(PowerFreq::default().selected(), CORE_AVERAGE);
    }

    #[test]
    fn a_custom_cpu_colour_reaches_power_and_freq() {
        use crate::sensors::model::{Device, DeviceKind, Group, Reading, Unit};
        let (settings, cpu) = crate::pages::tests::custom_cpu();
        let disk = Device::new(DeviceKind::Disk, "disk0", "SSD");
        let speed = Reading::measure("disk.read", disk, Group::Speed, "Read Speed", Unit::BytesPerSec, "test", Instant::now()).with(Some(1e6), "");
        let frame = Frame::build(1, &[vec![speed]], Vec::new());
        let p = PowerFreq::default();
        let rows = tree_view(Some(&frame), &p.stats, &p.view).unwrap();
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1000.0, 400.0), vec![], |ui| {
            p.tree_rows(ui, Some(&rows), 900.0, &settings.theme());
        });
        let drawn = crate::test_ui::colours(&out);
        assert!(drawn.contains(&cpu), "the Speed group in the CPU colour");
        assert!(!drawn.contains(&crate::theme::GRAPHS.cpu), "no default CPU colour left");
    }

    #[test]
    fn with_no_sensor_selected_the_panel_shows_the_shared_empty_text() {
        let mut p = PowerFreq::default();
        p.select("no.such.sensor");
        let frame = made_up_frame();
        let (settings, snap) = (Settings::default(), Snapshot::default());
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (400.0, 600.0), vec![], |ui| {
            p.side_panel(ui, Some(&frame), &mut PageContext::new(&settings, &snap));
        });
        let texts: Vec<String> = crate::test_ui::flat_shapes(&out)
            .iter()
            .filter_map(|s| if let egui::Shape::Text(t) = s { Some(t.galley.text().to_string()) } else { None })
            .collect();
        assert!(texts.iter().any(|t| t == crate::widgets::parts::EMPTY_DETAIL), "{texts:?}");
    }

    #[test]
    fn the_selected_sensor_is_remembered() {
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "other": 1 }));
        p.select("cpu.core3.parked");
        let saved = p.settings();
        assert_eq!(saved["other"], 1, "other settings are kept");
        let mut again = PowerFreq::default();
        again.load_settings(&saved);
        assert_eq!(again.selected(), "cpu.core3.parked");
    }

    #[test]
    fn its_settings_come_back_after_turning_it_off_and_on() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "other": 1 }));
        p.start(&ctx);
        p.select("cpu.core3.parked");
        p.view.toggle("d:cpu");
        p.view.toggle("g:cpu/Clocks");
        p.prefs = Prefs { summary_tile: true, show_unavailable: false, smart_probe: true, ..Prefs::default() };
        p.stop();
        let saved = p.settings();
        assert_eq!(saved["other"], 1, "other settings are kept");
        let mut again = PowerFreq::default();
        again.load_settings(&saved);
        again.start(&ctx);
        assert_eq!(again.selected(), "cpu.core3.parked");
        assert_eq!(again.view.collapsed, p.view.collapsed, "the same rows are closed");
        assert_eq!(again.prefs, Prefs { summary_tile: true, show_unavailable: false, smart_probe: true, ..Prefs::default() });
    }

    fn draw_summary(p: &mut PowerFreq) {
        let (settings, snap) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut page = PageContext::new(&settings, &snap);
                // As the host draws the tile: its sensors come from the host's leases.
                page.lease = p.context.as_ref().map(|c| c.leases().lease(crate::addons::leases::Surface::Tile("power-freq")));
                p.summary(ui, &mut page)
            });
        });
    }

    #[test]
    fn settings_add_ons_tells_what_it_needs_before_it_is_turned_on() {
        let (host, _) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "power-freq": { "enabled": false } }));
        let d = crate::addons::overview::Overview::of(&host, Some("power-freq")).detail.unwrap();
        assert_eq!(
            d.requirements,
            [
                "Asks for administrator rights for some actions",
                "Runs the hardware makers' libraries (NVML, ADLX, IGCL, AMD's SDK, Intel DTT) if installed"
            ],
            "administrator for some sensors, no internet, the vendors' DLLs"
        );
    }

    #[test]
    fn without_its_tile_summary_reads_no_sensor() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        draw_summary(&mut p);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ctx.sensors().threads(), 0);
    }

    #[test]
    fn its_tile_keeps_the_sensors_running_with_the_page_closed_until_summary_is_left() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "summary_tile": true }));
        p.start(&ctx);
        shown(&mut p);
        draw_summary(&mut p);
        ctx.leases().end_frame();
        ctx.leases().end_frame(); // the host drops the page's sensors
        p.page_hidden();
        draw_summary(&mut p);
        std::thread::sleep(Duration::from_millis(30));
        assert!(ctx.sensors().threads() > 0, "the tile still looks at the sensors");
        ctx.leases().end_frame();
        ctx.leases().end_frame(); // Summary is left: the host drops the tile's sensors
        p.summary_hidden();
        until("no sensor thread", || ctx.sensors().threads() == 0);
    }

    #[test]
    fn turning_it_off_stops_the_sensors_of_its_tile_too() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "summary_tile": true }));
        p.start(&ctx);
        draw_summary(&mut p);
        assert!(ctx.sensors().threads() > 0);
        p.stop();
        ctx.leases().release("power-freq"); // as the host does when it turns an add-on off
        until("no sensor thread", || ctx.sensors().threads() == 0);
    }

    #[test]
    fn the_tile_still_shows_core_average_first() {
        assert_eq!(tile_lines(&made_up_frame(), false), vec!["Core Average: 5.17 GHz"], "nothing else read: only Core Average");
    }

    /// A CPU's Core Average and, when given, its package power and temperature.
    fn cpu_readings(power: Option<f64>, temperature: Option<f64>) -> Vec<Reading> {
        let (at, cpu) = (Instant::now(), Device::cpu("CPU"));
        vec![
            Reading::measure(CORE_AVERAGE, cpu.clone(), Group::Clocks, "Core Average", Unit::Hz, "t", at).with(Some(5.11e9), ""),
            Reading::measure(PACKAGE_POWER, cpu.clone(), Group::Powers, "CPU Package", Unit::Watt, "t", at).with(power, "not read"),
            Reading::measure(PACKAGE_TEMPERATURE, cpu, Group::Temperatures, "Package", Unit::Celsius, "t", at).with(temperature, "not read"),
        ]
    }

    /// A card's readings with these Board Power and temperature.
    fn gpu_readings(key: &str, name: &str, power_w: Option<f64>, temperature_c: Option<f64>) -> Vec<Reading> {
        let sample = crate::sensors::gpu::GpuSample { power_w, temperature_c, ..Default::default() };
        sample.readings(&Device::gpu(key, name), "t", "not read", Instant::now())
    }

    /// A disk's S.M.A.R.T. Health row: its words, or unread.
    fn health(n: u32, words: Option<&str>) -> Vec<Reading> {
        let disk = Device::new(DeviceKind::Disk, &format!("disk{n}"), &format!("Disk {n}"));
        vec![Reading::state(&format!("disk{n}.smart.health"), disk, Group::Smart, "Health", "t", Instant::now()).with_text(words.map(String::from), "needs administrator")]
    }

    #[test]
    fn the_tile_adds_the_package_power_and_temperature_only_when_read() {
        assert_eq!(
            tile_lines(&Frame::build(1, &[cpu_readings(Some(41.26), Some(50.0))], Vec::new()), true),
            vec!["Core Average: 5.11 GHz", "Package temperature: 122.0 °F", "Package power: 41.3 W"]
        );
        assert_eq!(tile_lines(&Frame::build(1, &[cpu_readings(Some(41.26), None)], Vec::new()), false), vec!["Core Average: 5.11 GHz", "Package power: 41.3 W"], "no temperature: no line");
        assert_eq!(tile_lines(&Frame::build(1, &[cpu_readings(None, None)], Vec::new()), false), vec!["Core Average: 5.11 GHz"]);
    }

    #[test]
    fn the_tile_has_a_line_per_gpu_with_its_board_power_and_temperature() {
        let one = Frame::build(1, &[cpu_readings(None, None), gpu_readings("0", "RTX 4080", Some(285.3), Some(64.0))], Vec::new());
        assert_eq!(tile_lines(&one, false), vec!["Core Average: 5.11 GHz", "RTX 4080: 285.3 W, 64.0 °C"]);
        let two = Frame::build(
            1,
            &[cpu_readings(None, None), gpu_readings("0", "RTX 4080", Some(285.3), Some(64.0)), gpu_readings("1", "Radeon 780M", None, Some(51.0)), gpu_readings("2", "Basic Display", None, None)],
            Vec::new(),
        );
        assert_eq!(tile_lines(&two, true), vec!["Core Average: 5.11 GHz", "RTX 4080: 285.3 W, 147.2 °F", "Radeon 780M: 123.8 °F"], "a card with nothing read has no line");
    }

    #[test]
    fn the_tile_says_healthy_or_the_first_smart_problem() {
        let healthy = Frame::build(1, &[cpu_readings(None, None), health(0, Some("Healthy")), health(1, None), health(2, Some("Healthy"))], Vec::new());
        assert_eq!(tile_lines(&healthy, false), vec!["Core Average: 5.11 GHz", "S.M.A.R.T.: Healthy"]);
        let problem = Frame::build(
            1,
            &[cpu_readings(None, None), health(0, Some("Healthy")), health(1, Some("Warning: SMART threshold exceeded (Reallocated Sectors)")), health(2, Some("Warning: spare below threshold"))],
            Vec::new(),
        );
        assert_eq!(tile_lines(&problem, false), vec!["Core Average: 5.11 GHz", "S.M.A.R.T.: Warning: SMART threshold exceeded (Reallocated Sectors)"]);
        let unread = Frame::build(1, &[cpu_readings(None, None), health(0, None)], Vec::new());
        assert_eq!(tile_lines(&unread, false), vec!["Core Average: 5.11 GHz"], "no disk read: no line");
    }

    #[test]
    fn the_tile_has_no_line_for_a_value_this_pc_does_not_read() {
        let (at, cpu) = (Instant::now(), Device::cpu("CPU"));
        let nothing = vec![Reading::measure(CORE_AVERAGE, cpu, Group::Clocks, "Core Average", Unit::Hz, "t", at).with(None, "not read")];
        assert!(tile_lines(&Frame::build(1, &[nothing], Vec::new()), false).is_empty());
        assert!(tile_lines(&Frame::build(1, &[], Vec::new()), false).is_empty());
    }

    #[test]
    fn pinned_sensors_follow_the_built_in_lines_in_their_order() {
        let frame = Frame::build(1, &[cpu_readings(Some(41.26), None), health(0, Some("Healthy"))], Vec::new());
        let pinned = vec![PACKAGE_POWER.to_string(), "disk7.smart.health".to_string(), PACKAGE_TEMPERATURE.to_string()];
        assert_eq!(
            pinned_lines(&frame, &pinned, false),
            vec!["CPU Package: 41.3 W", "disk7.smart.health: not on this PC", "Package: not read"],
            "in the pinned order; a missing id says so; a known but unread one gives its reason"
        );
        let mut tile = tile_lines(&frame, false);
        tile.extend(pinned_lines(&frame, &pinned, false));
        assert_eq!(tile[..3], ["Core Average: 5.11 GHz", "Package power: 41.3 W", "S.M.A.R.T.: Healthy"], "the #192 lines come first, unchanged");
        assert_eq!(tile.len(), 6);
        assert!(pinned_lines(&frame, &[], false).is_empty(), "nothing pinned: no extra line");
    }

    #[test]
    fn a_pinned_temperature_follows_the_app_unit() {
        let frame = Frame::build(1, &[cpu_readings(None, Some(50.0))], Vec::new());
        assert_eq!(pinned_lines(&frame, &[PACKAGE_TEMPERATURE.to_string()], true), vec!["Package: 122.0 °F"]);
    }

    #[test]
    fn pins_are_toggled_kept_in_order_and_saved_under_pinned() {
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "other": 1 }));
        p.toggle_pin(PACKAGE_POWER);
        p.toggle_pin(CORE_AVERAGE);
        p.toggle_pin(BASE_HZ);
        assert!(p.is_pinned(CORE_AVERAGE));
        assert_eq!(p.pin_label(CORE_AVERAGE), UNPIN);
        assert_eq!(p.pin_label(PACKAGE_TEMPERATURE), PIN);
        p.toggle_pin(CORE_AVERAGE);
        assert!(!p.is_pinned(CORE_AVERAGE), "pressed again: unpinned");
        let saved = p.settings();
        assert_eq!(saved["pinned"], serde_json::json!([PACKAGE_POWER, BASE_HZ]), "ordered as pinned");
        assert_eq!(saved["other"], 1, "other settings are kept");
        let mut again = PowerFreq::default();
        again.load_settings(&saved);
        assert_eq!(again.pinned(), [PACKAGE_POWER.to_string(), BASE_HZ.to_string()], "pins survive a restart");
        again.toggle_pin(PACKAGE_POWER);
        again.toggle_pin(BASE_HZ);
        let saved = again.settings();
        assert!(saved.get("pinned").is_none(), "nothing pinned: the key goes");
        assert_eq!(saved["other"], 1);
    }

    #[test]
    fn settings_saved_by_the_earlier_version_still_load() {
        let saved = serde_json::json!({
            "selected": "cpu.core3.parked",
            "collapsed": ["d:cpu", "g:cpu/Clocks"],
            "summary_tile": true,
            "show_unavailable": false,
            "smart_probe": true,
            "other": 1
        });
        let mut p = PowerFreq::default();
        p.load_settings(&saved);
        assert_eq!(p.selected(), "cpu.core3.parked");
        assert_eq!(p.view.collapsed, ["d:cpu".to_string(), "g:cpu/Clocks".to_string()].into_iter().collect());
        assert_eq!(p.prefs, Prefs { summary_tile: true, show_unavailable: false, smart_probe: true, ..Prefs::default() });
        assert_eq!(p.settings(), saved, "written back under the same keys");
    }

    /// An alert the made-up CPU always triggers: Core Average above 0 Hz,
    /// at once.
    fn always_triggered() -> serde_json::Value {
        serde_json::json!({ "metric": CORE_AVERAGE, "comparison": "above", "threshold": 0.0, "duration_s": 0 })
    }

    #[test]
    fn one_alert_keeps_the_sensors_running_with_the_page_closed_and_none_without() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ctx.sensors().threads(), 0, "no alert: no background work");
        assert_eq!(p.threads(), 0);
        p.add_alert();
        until("a sensor thread for the alert", || ctx.sensors().threads() > 0);
        assert_eq!(p.threads(), 1, "the watcher, page never shown");
        p.delete_alert(0);
        until("no sensor thread", || ctx.sensors().threads() == 0);
        assert_eq!(p.threads(), 0);
    }

    /// A host with Power & Freq alone (and `section` as saved settings).
    fn power_freq_host(section: &serde_json::Value) -> crate::addons::AddonHost {
        let entries = vec![crate::addons::Entry::Addon(Box::<PowerFreq>::default())];
        crate::addons::AddonHost::for_tests_with(entries, section).0
    }

    /// One frame through the host with the page and/or the tile drawn.
    fn draw_surfaces(host: &mut crate::addons::AddonHost, page_shown: bool, tile_shown: bool) {
        let (settings, snap) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if page_shown {
                    host.ui("power-freq", ui, &mut PageContext::new(&settings, &snap));
                }
                if tile_shown {
                    host.summary_tiles(ui, &mut PageContext::new(&settings, &snap));
                }
            });
        });
        host.end_frame();
    }

    fn tile_host() -> crate::addons::AddonHost {
        power_freq_host(&serde_json::json!({ "power-freq": { "settings": { "summary_tile": true } } }))
    }

    #[test]
    fn the_host_stops_the_service_one_frame_after_the_tile_is_no_longer_drawn() {
        let mut host = tile_host();
        for _ in 0..3 {
            draw_surfaces(&mut host, false, true);
            assert!(host.context.sensors().threads() > 0, "drawn in every frame");
        }
        host.end_frame(); // the first frame without the tile
        until("no sensor thread", || host.context.sensors().threads() == 0);
    }

    #[test]
    fn with_page_and_tile_shown_hiding_one_keeps_the_service_and_hiding_both_stops_it() {
        let mut host = tile_host();
        draw_surfaces(&mut host, true, true);
        draw_surfaces(&mut host, false, true); // page hidden
        draw_surfaces(&mut host, false, true);
        std::thread::sleep(Duration::from_millis(30));
        assert!(host.context.sensors().threads() > 0, "the tile still looks");
        draw_surfaces(&mut host, true, false); // the tile is hidden, the page is back
        draw_surfaces(&mut host, true, false);
        std::thread::sleep(Duration::from_millis(30));
        assert!(host.context.sensors().threads() > 0, "the page still looks");
        host.end_frame();
        until("no sensor thread", || host.context.sensors().threads() == 0);
    }

    /// Power & Freq inside a host, with a handle on it to look at what its
    /// statistics show.
    struct Hosted(std::sync::Arc<std::sync::Mutex<PowerFreq>>);

    impl Addon for Hosted {
        fn id(&self) -> &'static str {
            "power-freq"
        }
        fn name(&self) -> &'static str {
            "Power & Freq"
        }
        fn description(&self) -> &'static str {
            "Power & Freq, hosted for a test."
        }
        fn requirements(&self) -> Requirements {
            self.0.lock().unwrap().requirements()
        }
        fn consumption(&self) -> Consumption {
            self.0.lock().unwrap().consumption()
        }
        fn start(&mut self, context: &AddonContext) {
            self.0.lock().unwrap().start(context);
        }
        fn stop(&mut self) {
            self.0.lock().unwrap().stop();
        }
        fn load_settings(&mut self, saved: &serde_json::Value) {
            self.0.lock().unwrap().load_settings(saved);
        }
        fn page_hidden(&mut self) {
            self.0.lock().unwrap().page_hidden();
        }
        fn summary_hidden(&mut self) {
            self.0.lock().unwrap().summary_hidden();
        }
        fn recording_shown(&mut self, recording: Recording) {
            self.0.lock().unwrap().recording_shown(recording);
        }
        fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
            self.0.lock().unwrap().summary(ui, page);
        }
        fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
            self.0.lock().unwrap().ui(ui, page);
        }
    }

    /// A host with Power & Freq (its Summary tile on) and a handle on it.
    fn hosted_with_tile() -> (crate::addons::AddonHost, std::sync::Arc<std::sync::Mutex<PowerFreq>>) {
        let power_freq = std::sync::Arc::default();
        let entries = vec![crate::addons::Entry::Addon(Box::new(Hosted(std::sync::Arc::clone(&power_freq))))];
        let section = serde_json::json!({ "power-freq": { "settings": { "summary_tile": true } } });
        (crate::addons::AddonHost::for_tests_with(entries, &section).0, power_freq)
    }

    #[test]
    fn statistics_survive_the_page_to_the_tile_and_reset_when_neither_is_shown() {
        let (mut host, power_freq) = hosted_with_tile();
        let min_max = || power_freq.lock().unwrap().stats.min_max(CORE_AVERAGE);
        until("the page has statistics", || {
            draw_surfaces(&mut host, true, false);
            min_max().is_some()
        });
        let since = power_freq.lock().unwrap().stats.since();
        draw_surfaces(&mut host, true, true);
        draw_surfaces(&mut host, false, true); // the page is left
        draw_surfaces(&mut host, false, true);
        assert!(min_max().is_some(), "the tile went on with the page's statistics");
        assert_eq!(power_freq.lock().unwrap().stats.since(), since, "same epoch");
        draw_surfaces(&mut host, false, false); // Summary is left too
        until("no sensor thread", || host.context.sensors().threads() == 0);
        assert_eq!(min_max(), None, "neither is shown: the statistics are gone");
    }

    #[test]
    fn reset_statistics_works_with_only_the_tile_shown() {
        let (mut host, power_freq) = hosted_with_tile();
        until("the tile has statistics", || {
            draw_surfaces(&mut host, false, true);
            power_freq.lock().unwrap().stats.min_max(CORE_AVERAGE).is_some()
        });
        std::thread::sleep(Duration::from_millis(1100)); // the epoch is in whole seconds
        let before = crate::platform::local_secs();
        power_freq.lock().unwrap().reset_statistics();
        assert!(power_freq.lock().unwrap().stats.since() >= before, "the epoch is the reset's time");
    }

    #[test]
    fn the_host_runs_the_sensors_while_the_page_is_drawn_and_has_none_a_frame_after() {
        let mut host = power_freq_host(&serde_json::json!({}));
        for _ in 0..3 {
            draw_surfaces(&mut host, true, false);
            assert!(host.context.sensors().threads() > 0);
        }
        host.end_frame();
        until("no sensor thread", || host.context.sensors().threads() == 0);
    }

    #[test]
    fn turning_it_off_through_the_host_stops_a_service_only_its_page_kept() {
        let mut host = power_freq_host(&serde_json::json!({}));
        draw_surfaces(&mut host, true, false);
        assert!(host.context.sensors().threads() > 0);
        host.set_enabled("power-freq", false);
        until("no sensor thread", || host.context.sensors().threads() == 0);
    }

    #[test]
    fn an_alert_keeps_the_service_running_after_the_page_is_hidden() {
        let power_freq = std::sync::Arc::<std::sync::Mutex<PowerFreq>>::default();
        let entries = vec![crate::addons::Entry::Addon(Box::new(Hosted(std::sync::Arc::clone(&power_freq))))];
        let section = serde_json::json!({ "power-freq": { "settings": { "alerts": [always_triggered()] } } });
        let mut host = crate::addons::AddonHost::for_tests_with(entries, &section).0;
        draw_surfaces(&mut host, true, false);
        // The watcher's thread takes its own interest when it starts: wait
        // until it has (it evaluated a frame) before the page lets go of its.
        until("the alert's watcher has a frame", || power_freq.lock().unwrap().watcher.as_ref().is_some_and(|w| w.frame().is_some()));
        host.end_frame(); // the page is not drawn any more
        std::thread::sleep(Duration::from_millis(50));
        assert!(host.context.sensors().threads() > 0, "the alert still watches");
        host.set_enabled("power-freq", false);
        until("no sensor thread", || host.context.sensors().threads() == 0);
    }

    #[test]
    fn saved_alerts_watch_from_the_start_and_stop_when_it_is_turned_off() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "alerts": [always_triggered()] }));
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(ctx.sensors().threads(), 0, "off: nothing runs");
        p.start(&ctx);
        until("a sensor thread for the alert", || ctx.sensors().threads() > 0);
        p.stop();
        assert_eq!(p.threads(), 0, "off: its watcher ended");
        until("no sensor thread", || ctx.sensors().threads() == 0);
    }

    #[test]
    fn a_triggered_alert_shows_one_notification() {
        let (ctx, runner) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "alerts": [always_triggered()] }));
        p.start(&ctx);
        let notes = || runner.launches().into_iter().filter(|l| matches!(l, crate::platform::Launch::Notify { .. })).collect::<Vec<_>>();
        until("the notification", || !notes().is_empty());
        std::thread::sleep(Duration::from_millis(600));
        match &notes()[..] {
            [crate::platform::Launch::Notify { title, body }] => {
                assert!(title.starts_with("Alert: Core Average"), "{title}");
                assert!(body.starts_with("Above 0 MHz for 0 s (now "), "{body}");
            }
            other => panic!("one notification expected, got {other:?}"),
        }
        assert!(p.alerted(CORE_AVERAGE), "its row is highlighted");
        assert!(!p.alerted(BASE_HZ));
    }

    #[test]
    fn alerts_are_listed_edited_deleted_and_saved_under_alerts() {
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "other": 1 }));
        p.add_alert();
        p.add_alert();
        p.alerts[1].comparison = alerts::Comparison::Below;
        p.alerts[1].threshold = alerts::stored(Unit::Celsius, 50.0, true);
        p.alerts[1].duration_s = 30;
        let saved = p.settings();
        assert_eq!(saved["other"], 1, "other settings are kept");
        assert_eq!(saved["alerts"][0], serde_json::json!({ "metric": PACKAGE_TEMPERATURE, "comparison": "above", "threshold": 90.0, "duration_s": 10 }));
        assert_eq!(saved["alerts"][1]["comparison"], "below");
        assert!((saved["alerts"][1]["threshold"].as_f64().unwrap() - 10.0).abs() < 1e-9, "50 °F is stored as 10 °C");
        let mut again = PowerFreq::default();
        again.load_settings(&saved);
        assert_eq!(again.alerts(), p.alerts(), "listed again after a restart");
        again.delete_alert(0);
        again.delete_alert(0);
        let saved = again.settings();
        assert!(saved.get("alerts").is_none(), "no alert left: the key goes");
        assert_eq!(saved["other"], 1);
    }

    #[test]
    fn settings_with_alerts_still_round_trip_the_earlier_keys() {
        let saved = serde_json::json!({
            "selected": "cpu.core3.parked",
            "collapsed": ["d:cpu"],
            "summary_tile": true,
            "show_unavailable": false,
            "smart_probe": true,
            "history_window_s": 120,
            "alerts": [always_triggered()],
            "other": 1
        });
        let mut p = PowerFreq::default();
        p.load_settings(&saved);
        assert_eq!(p.prefs, Prefs { summary_tile: true, show_unavailable: false, smart_probe: true, history_window_s: 120 });
        assert_eq!(p.alerts().len(), 1);
        assert_eq!(p.settings(), saved, "written back under the same keys");
        assert!(PowerFreq::default().settings().is_null(), "untouched: still nothing saved");
    }

    #[test]
    fn settings_with_pins_still_round_trip_the_earlier_keys() {
        let saved = serde_json::json!({
            "selected": "cpu.core3.parked",
            "collapsed": ["d:cpu"],
            "summary_tile": true,
            "show_unavailable": false,
            "smart_probe": true,
            "history_window_s": 120,
            "alerts": [always_triggered()],
            "pinned": [PACKAGE_TEMPERATURE, CORE_AVERAGE],
            "other": 1
        });
        let mut p = PowerFreq::default();
        p.load_settings(&saved);
        assert_eq!(p.prefs, Prefs { summary_tile: true, show_unavailable: false, smart_probe: true, history_window_s: 120 });
        assert_eq!(p.alerts().len(), 1);
        assert_eq!(p.pinned(), [PACKAGE_TEMPERATURE.to_string(), CORE_AVERAGE.to_string()]);
        assert_eq!(p.settings(), saved, "written back under the same keys");
        let mut pinned_only = PowerFreq::default();
        pinned_only.toggle_pin(CORE_AVERAGE);
        assert_eq!(pinned_only.settings()["pinned"], serde_json::json!([CORE_AVERAGE]), "a pin alone is saved");
    }

    #[test]
    fn its_graph_window_is_saved_only_when_not_the_default_minute() {
        let mut p = PowerFreq::default();
        p.load_settings(&serde_json::json!({ "history_window_s": 120, "summary_tile": true }));
        assert_eq!(p.prefs.history_window(), Duration::from_secs(120));
        assert_eq!(p.settings()["history_window_s"], 120, "written back");
        p.prefs.history_window_s = DEFAULT_HISTORY_WINDOW_S;
        assert!(p.settings().get("history_window_s").is_none(), "back to a minute: not written");
        assert_eq!(PowerFreq::default().prefs.history_window(), Duration::from_secs(60), "a minute by default");
    }

    #[test]
    fn untouched_it_keeps_no_settings_and_shows_everything_without_a_tile() {
        let p = PowerFreq::default();
        assert!(p.settings().is_null());
        assert_eq!(p.prefs, Prefs { summary_tile: false, show_unavailable: true, smart_probe: false, history_window_s: 60 });
        let mut broken = PowerFreq::default();
        broken.load_settings(&serde_json::json!({ "collapsed": 3, "summary_tile": "yes" }));
        assert_eq!(broken.prefs, Prefs::default(), "unreadable fields give their defaults");
        assert!(broken.view.collapsed.is_empty());
    }

    #[test]
    fn copy_reading_copies_the_selected_sensor() {
        let (ctx, _) = AddonContext::recording();
        let mut p = PowerFreq::default();
        p.start(&ctx);
        shown(&mut p);
        let frame = ctx.leases().ask(crate::addons::leases::Surface::Page("power-freq")).latest().unwrap();
        let action = p.copy_reading(&frame);
        let want = format!("Core Average (Test CPU › Clocks): 5.17 GHz, Min 5.17 GHz, Avg 5.17 GHz, Max 5.17 GHz, {}, read from Windows counters", p.stats.since_text());
        assert!(
            matches!(&action, Some(crate::platform::Action::Copy(t)) if *t == want),
            "got {:?}",
            action.map(|_| ())
        );
    }

    #[test]
    fn the_band_says_how_far_above_or_below_the_base_the_cores_run() {
        assert_eq!(band(Some(5.11e9), Some(4.70e9), None), "Cores run at 5.11 GHz, 9% above the 4.70 GHz base");
        assert_eq!(band(Some(2.35e9), Some(4.70e9), None), "Cores run at 2.35 GHz, 50% below the 4.70 GHz base");
        assert_eq!(band(Some(4.71e9), Some(4.70e9), None), "Cores run at 4.71 GHz, at the 4.70 GHz base");
        assert_eq!(band(Some(800.0e6), Some(4.70e9), None), "Cores run at 800 MHz, 83% below the 4.70 GHz base");
        assert_eq!(band(Some(5.11e9), None, None), "Cores run at 5.11 GHz");
        assert_eq!(band(None, Some(4.70e9), None), "The cores' clock is not available");
    }

    #[test]
    fn below_the_base_the_band_says_the_limit_reason_when_one_is_known() {
        assert_eq!(band(Some(2.35e9), Some(4.70e9), Some("power")), "Cores run at 2.35 GHz, 50% below the 4.70 GHz base: power limit");
        assert_eq!(
            band(Some(2.35e9), Some(4.70e9), Some("thermal, power")),
            "Cores run at 2.35 GHz, 50% below the 4.70 GHz base: thermal, power limit"
        );
        assert_eq!(band(Some(2.35e9), Some(4.70e9), None), "Cores run at 2.35 GHz, 50% below the 4.70 GHz base", "no reason known");
        assert_eq!(band(Some(5.11e9), Some(4.70e9), Some("power")), "Cores run at 5.11 GHz, 9% above the 4.70 GHz base", "above the base: no cause");
        assert_eq!(band(Some(4.71e9), Some(4.70e9), Some("power")), "Cores run at 4.71 GHz, at the 4.70 GHz base", "at the base: no cause");
    }

    #[test]
    fn the_status_band_takes_the_limit_reason_from_the_limits_group() {
        use crate::sensors::cpu::{CoreSample, CpuSample};
        let sample = |flags: u32| CpuSample {
            name: "Test CPU".into(),
            base_mhz: Some(4_700.0),
            performance_percent: Some(50.0),
            utilization_percent: Some(10.0),
            cores: (0..2).map(|_| CoreSample { performance_limit_percent: Some(50.0), limit_flags: Some(flags), ..Default::default() }).collect(),
        };
        let band_of = |flags| status(&Frame::build(1, &[sample(flags).readings(std::time::Instant::now())], Vec::new()), false).band;
        assert_eq!(band_of(0x2), "Cores run at 2.35 GHz, 50% below the 4.70 GHz base: power limit");
        assert_eq!(band_of(0), "Cores run at 2.35 GHz, 50% below the 4.70 GHz base", "nothing limits the cores");
    }

    fn made_up_frame() -> std::sync::Arc<Frame> {
        let (ctx, _) = AddonContext::recording();
        let interest = ctx.sensors().interest();
        until("a frame", || interest.latest().is_some());
        interest.latest().unwrap()
    }

    #[test]
    fn under_the_band_come_the_reported_clock_parked_processors_plan_and_power_source() {
        let s = status(&made_up_frame(), false);
        assert_eq!(s.band, "Cores run at 5.17 GHz, 10% above the 4.70 GHz base");
        assert_eq!(
            s.details,
            vec!["Reported 4.70 GHz", "1 logical processor parked", "Ultimate Performance", "Power source: no provider on this PC yet"]
        );
    }

    #[test]
    fn the_package_temperature_joins_the_line_under_the_band_when_known() {
        let cpu = Device::cpu("AMD Ryzen 5 7600X");
        let at = Instant::now();
        let known = Frame::build(1, &[vec![Reading::measure(PACKAGE_TEMPERATURE, cpu.clone(), Group::Temperatures, "CPU Package", Unit::Celsius, "AMD Ryzen Master SDK", at).with(Some(71.5), "")]], Vec::new());
        assert_eq!(status(&known, false).details.last().map(String::as_str), Some("Package 71.5 °C"));
        assert_eq!(status(&known, true).details.last().map(String::as_str), Some("Package 160.7 °F"), "in the app's unit");
        let grey = Frame::build(1, &[crate::sensors::ryzen::readings(Err(crate::sensors::ryzen::NEEDS_SDK), "AMD Ryzen 5 7600X", at)], Vec::new());
        assert_eq!(status(&grey, false).details.len(), 4, "not known: not on the line");
        assert!(status(&grey, false).details.iter().all(|d| !d.starts_with("Package")));
    }

    #[test]
    fn the_kpi_tiles_show_core_average_base_power_source_and_thermal_state_or_why_not() {
        let tiles = status(&made_up_frame(), false).tiles;
        let names: Vec<_> = tiles.iter().map(|t| t.0).collect();
        assert_eq!(names, vec!["Core Average", "Base frequency", "Power source", "Thermal state"]);
        assert_eq!((tiles[0].1.as_str(), tiles[1].1.as_str()), ("5.17 GHz", "4.70 GHz"));
        assert_eq!(tiles[3].1, "no provider on this PC yet", "a missing tile says why");
        assert_eq!(status(&made_up_frame(), false).missing, vec![false, false, true, true], "and is drawn grey");
    }

    #[test]
    fn under_the_tiles_come_core_averages_min_max_and_the_logical_processors() {
        let frame = made_up_frame();
        let mut stats = Stats::default();
        stats.observe(&frame);
        let notes = tile_notes(&frame, &stats);
        assert_eq!(notes[0].as_deref(), Some("Min 5.17 GHz · Max 5.17 GHz"));
        assert!(notes[1].as_deref().is_some_and(|n| n.ends_with("logical processors")), "got {:?}", notes[1]);
        assert_eq!((notes[2].as_deref(), notes[3].as_deref()), (None, None), "no battery reading: nothing to add");
    }

    #[test]
    fn a_gpu_heading_carries_its_clock_power_and_memory_line() {
        use crate::sensors::gpu::GpuSample;
        let card = Device::gpu("gpu0", "NVIDIA GeForce RTX 4080");
        let s = GpuSample { graphics_mhz: Some(2730.0), memory_mhz: Some(11201.0), power_w: Some(285.26), ..Default::default() };
        let frame = Frame::build(1, &[s.readings(&card, "NVML", "-", Instant::now())], Vec::new());
        let rows = tree(Some(&frame), &Stats::default()).unwrap();
        let heading = rows.iter().find(|r| r.sensor == "NVIDIA GeForce RTX 4080").unwrap();
        assert_eq!((heading.depth, heading.value.as_str()), (1, "Clock 2.73 GHz   Power 285.3 W   Memory 11.20 GHz"));
    }

    #[test]
    fn page_requests_reach_it_by_its_alias() {
        assert!(PowerFreq::default().aliases().contains(&"power-and-freq"));
    }

    #[test]
    fn the_platform_keeps_no_power_and_freq_code() {
        for (file, src) in [("app.rs", include_str!("../app.rs")), ("page_perf.rs", include_str!("../page_perf.rs")), ("pro.rs", include_str!("../pro.rs"))] {
            for word in ["freq_hist", "power_info", concat!("Power", "Info"), "power_freq"] {
                assert!(!src.contains(word), "{file} still has {word}");
            }
        }
        let addon = include_str!("power_freq.rs");
        assert!(!addon.contains(concat!("Power", "Info")), "the power info block left the add-on");
        assert!(!addon.contains(concat!("fn on_", "sample")), "the clock history lives in the sensor service");
    }
}
