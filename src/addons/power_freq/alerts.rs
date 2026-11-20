//! Threshold alerts: the rules the user sets on sensors ("CPU package
//! above 90 °C for 10 s") and the pure evaluator that turns frames into
//! "triggered" and "cleared" transitions, one per crossing.

use crate::sensors::model::{fahrenheit_of, Frame, Reading, Unit};
use std::time::{Duration, Instant};

/// The id of the alerts' line in [`crate::addons::TIERS`].
pub const TIER_ID: &str = "power-freq.alerts";

/// What a new alert watches until the user picks another sensor.
pub const NEW_RULE_THRESHOLD_C: f64 = 90.0;
/// How long a new alert waits, in seconds.
pub const NEW_RULE_DURATION_S: u64 = 10;
/// The longest duration offered, in seconds.
pub const MAX_DURATION_S: u64 = 3600;

/// Which side of the threshold raises the alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Comparison {
    Above,
    Below,
}

impl Comparison {
    /// The word shown in Settings and in the notifications.
    pub fn word(self) -> &'static str {
        match self {
            Comparison::Above => "above",
            Comparison::Below => "below",
        }
    }
}

/// One alert, as saved under `addons.power-freq.settings.alerts`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Rule {
    /// The metric id of the sensor it watches.
    pub metric: String,
    pub comparison: Comparison,
    /// In the metric's base unit: °C for temperatures, Hz for clocks.
    pub threshold: f64,
    /// How long the value must stay past the threshold (and then back on
    /// the safe side) before it triggers (or clears), in seconds.
    pub duration_s: u64,
}

impl Rule {
    /// A new alert on `metric`: above 90 (°C for a temperature) for 10 s.
    pub fn new(metric: &str) -> Self {
        Rule { metric: metric.into(), comparison: Comparison::Above, threshold: NEW_RULE_THRESHOLD_C, duration_s: NEW_RULE_DURATION_S }
    }

    fn breached(&self, value: f64) -> bool {
        match self.comparison {
            Comparison::Above => value > self.threshold,
            Comparison::Below => value < self.threshold,
        }
    }

    fn duration(&self) -> Duration {
        Duration::from_secs(self.duration_s)
    }
}

/// The saved alerts, keeping every entry it can read (an unreadable one
/// is dropped alone, never the other settings).
pub fn lenient<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Rule>, D::Error> {
    let raw: serde_json::Value = serde::Deserialize::deserialize(d)?;
    let items = raw.as_array().cloned().unwrap_or_default();
    Ok(items.into_iter().filter_map(|v| serde_json::from_value::<Rule>(v).ok()).filter(|r| r.threshold.is_finite()).collect())
}

/// Where an alert stands after the last frame.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Status {
    /// Triggered and not cleared yet.
    pub active: bool,
    /// Since when the value is on the other side (past the threshold while
    /// not active, back on the safe side while active).
    pub since: Option<Instant>,
    /// Why the alert can neither trigger nor clear now (its sensor is
    /// unavailable); `None` while its value is read.
    pub quiet: Option<String>,
}

/// An alert with its status.
#[derive(Clone, Debug, PartialEq)]
pub struct Tracked {
    pub rule: Rule,
    pub status: Status,
}

/// What happened to an alert.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Triggered,
    Cleared,
}

/// One alert that triggered or cleared, with what the notification says.
#[derive(Clone, Debug, PartialEq)]
pub struct Transition {
    pub rule: Rule,
    pub change: Change,
    /// The sensor's name and device ("Package (CPU)").
    pub sensor: String,
    pub unit: Unit,
    /// The value that completed the crossing, in the base unit.
    pub value: f64,
}

/// Why a sensor that is not in the frame cannot be watched.
pub const NOT_READ: &str = "This sensor is not read on this PC.";

/// The alerts' new statuses and their transitions after `frame`, read at
/// `at`, from the statuses `before` (in any order; an edited rule starts
/// afresh). An alert triggers once its value stays past the threshold for
/// its duration, and clears once it stays back on the safe side for the
/// same duration. An unavailable value neither triggers nor clears: the
/// wait starts again and the status says why.
pub fn evaluate(rules: &[Rule], before: &[Tracked], frame: &Frame, at: Instant) -> (Vec<Tracked>, Vec<Transition>) {
    let mut used = vec![false; before.len()];
    let mut tracked = Vec::with_capacity(rules.len());
    let mut changes = Vec::new();
    for rule in rules {
        let mut status = match before.iter().enumerate().find(|(i, t)| !used[*i] && t.rule == *rule) {
            Some((i, t)) => {
                used[i] = true;
                t.status.clone()
            }
            None => Status::default(),
        };
        let reading = frame.reading(&rule.metric);
        match reading.and_then(|r| r.value.map(|v| (r, v))) {
            None => {
                status.since = None;
                status.quiet = Some(match reading {
                    Some(r) => r.unavailable.clone().unwrap_or_else(|| "Unavailable".into()),
                    None => NOT_READ.into(),
                });
            }
            Some((r, value)) => {
                status.quiet = None;
                // Past the threshold while waiting to trigger, or back on
                // the safe side while waiting to clear.
                let crossing = rule.breached(value) != status.active;
                if !crossing {
                    status.since = None;
                } else {
                    let since = *status.since.get_or_insert(at);
                    if at.saturating_duration_since(since) >= rule.duration() {
                        status.active = !status.active;
                        status.since = None;
                        let change = if status.active { Change::Triggered } else { Change::Cleared };
                        changes.push(Transition { rule: rule.clone(), change, sensor: sensor_name(r), unit: r.unit, value });
                    }
                }
            }
        }
        tracked.push(Tracked { rule: rule.clone(), status });
    }
    (tracked, changes)
}

/// A reading's name and device, as in the notifications ("Package (CPU)").
pub fn sensor_name(r: &Reading) -> String {
    format!("{} ({})", r.name, r.device.name)
}

impl Transition {
    /// The notification's title and text, temperatures in °F when
    /// `fahrenheit`.
    pub fn message(&self, fahrenheit: bool) -> (String, String) {
        let threshold = self.unit.format_in(self.rule.threshold, fahrenheit);
        let now = self.unit.format_in(self.value, fahrenheit);
        let word = self.rule.comparison.word();
        let wait = duration_text(self.rule.duration_s);
        match self.change {
            Change::Triggered => (format!("Alert: {}", self.sensor), format!("{} {threshold} for {wait} (now {now}).", capitalized(word))),
            Change::Cleared => (format!("Alert cleared: {}", self.sensor), format!("No longer {word} {threshold} for {wait} (now {now}).")),
        }
    }
}

/// `word` with its first letter in upper case ("Above").
fn capitalized(word: &str) -> String {
    let mut c = word.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// "10 s", "2 min", "1 min 30 s", or "0 s" (at once).
pub fn duration_text(s: u64) -> String {
    match (s / 60, s % 60) {
        (0, s) => format!("{s} s"),
        (m, 0) => format!("{m} min"),
        (m, s) => format!("{m} min {s} s"),
    }
}

/// Whether a reading of this unit can carry an alert (numbers only, not
/// words, yes/no or states).
pub fn alertable(unit: Unit) -> bool {
    !matches!(unit, Unit::Text | Unit::YesNo { .. } | Unit::State(_))
}

/// The unit a threshold of `unit` is typed in, and how many base units
/// one of it is (temperatures handled apart, see [`shown`]).
fn entry(unit: Unit, fahrenheit: bool) -> (&'static str, f64) {
    match unit {
        Unit::Celsius if fahrenheit => ("°F", 1.0),
        Unit::Celsius => ("°C", 1.0),
        Unit::Hz => ("GHz", 1e9),
        Unit::BytesPerSec => ("MB/s", 1e6),
        Unit::Bytes => ("GB", 1e9),
        Unit::Percent => ("%", 1.0),
        Unit::Watt => ("W", 1.0),
        Unit::Joule => ("J", 1.0),
        Unit::Volt => ("V", 1.0),
        Unit::Rpm => ("RPM", 1.0),
        Unit::Hours => ("h", 1.0),
        Unit::Seconds => ("s", 1.0),
        Unit::Count | Unit::Text | Unit::YesNo { .. } | Unit::State(_) => ("", 1.0),
    }
}

/// The unit word next to a threshold of `unit` in Settings ("°F", "GHz").
pub fn entry_unit(unit: Unit, fahrenheit: bool) -> &'static str {
    entry(unit, fahrenheit).0
}

/// A stored threshold (base unit, °C) as shown in Settings: in °F when
/// `fahrenheit`, clocks in GHz.
pub fn shown(unit: Unit, stored: f64, fahrenheit: bool) -> f64 {
    match unit {
        Unit::Celsius if fahrenheit => fahrenheit_of(stored),
        _ => stored / entry(unit, fahrenheit).1,
    }
}

/// A threshold typed in Settings, as stored: °F entries become °C,
/// GHz become Hz.
pub fn stored(unit: Unit, entered: f64, fahrenheit: bool) -> f64 {
    match unit {
        Unit::Celsius if fahrenheit => (entered - 32.0) * 5.0 / 9.0,
        _ => entered * entry(unit, fahrenheit).1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Group};

    const T: &str = "cpu.package.temperature";

    /// A frame where the package temperature reads `value` (None =
    /// unavailable, "needs administrator").
    fn frame(value: Option<f64>) -> Frame {
        let r = Reading::measure(T, Device::cpu("CPU"), Group::Temperatures, "Package", Unit::Celsius, "test", Instant::now()).with(value, "needs administrator");
        Frame::build(1, &[vec![r]], Vec::new())
    }

    fn above(threshold: f64, duration_s: u64) -> Rule {
        Rule { metric: T.into(), comparison: Comparison::Above, threshold, duration_s }
    }

    /// Feeds `values` one second apart from `t0`; the transitions, with the
    /// second each happened at.
    fn run(rule: &Rule, values: &[Option<f64>]) -> (Vec<(u64, Change)>, Vec<Tracked>) {
        let t0 = Instant::now();
        let mut state = Vec::new();
        let mut seen = Vec::new();
        for (s, v) in values.iter().enumerate() {
            let (next, changes) = evaluate(std::slice::from_ref(rule), &state, &frame(*v), t0 + Duration::from_secs(s as u64));
            state = next;
            seen.extend(changes.into_iter().map(|c| (s as u64, c.change)));
        }
        (seen, state)
    }

    #[test]
    fn an_alert_triggers_only_after_its_duration() {
        let hot = Some(95.0);
        let (seen, _) = run(&above(90.0, 3), &[hot, hot, hot]);
        assert!(seen.is_empty(), "2 s past the threshold: not yet");
        let (seen, _) = run(&above(90.0, 3), &[hot, hot, hot, hot]);
        assert_eq!(seen, vec![(3, Change::Triggered)], "3 s past the threshold");
        let (seen, _) = run(&above(90.0, 3), &[hot, hot, Some(80.0), hot, hot, hot]);
        assert!(seen.is_empty(), "a dip starts the wait again");
        let (seen, _) = run(&above(90.0, 0), &[hot]);
        assert_eq!(seen, vec![(0, Change::Triggered)], "no duration: at once");
    }

    #[test]
    fn an_alert_gives_one_transition_per_crossing() {
        let (hot, cool) = (Some(95.0), Some(80.0));
        let (seen, state) = run(&above(90.0, 1), &[hot, hot, hot, hot, hot, cool, cool, cool, hot, hot]);
        assert_eq!(seen, vec![(1, Change::Triggered), (6, Change::Cleared), (9, Change::Triggered)]);
        assert!(state[0].status.active);
    }

    #[test]
    fn an_alert_clears_only_after_the_same_duration_on_the_safe_side() {
        let (hot, cool) = (Some(95.0), Some(80.0));
        let (seen, _) = run(&above(90.0, 2), &[hot, hot, hot, cool, cool, hot, cool, cool]);
        assert_eq!(seen, vec![(2, Change::Triggered)], "back for 1 s, then hot again: still triggered");
        let (seen, _) = run(&above(90.0, 2), &[hot, hot, hot, cool, cool, cool]);
        assert_eq!(seen, vec![(2, Change::Triggered), (5, Change::Cleared)]);
        let below = Rule { comparison: Comparison::Below, ..above(50.0, 0) };
        let (seen, _) = run(&below, &[Some(60.0), Some(40.0), Some(45.0), Some(55.0)]);
        assert_eq!(seen, vec![(1, Change::Triggered), (3, Change::Cleared)], "below works the other way round");
    }

    #[test]
    fn unavailable_values_never_trigger_or_clear_and_say_why() {
        let (hot, cool) = (Some(95.0), Some(80.0));
        let (seen, state) = run(&above(90.0, 0), &[None, None]);
        assert!(seen.is_empty());
        assert_eq!(state[0].status.quiet.as_deref(), Some("needs administrator"));
        let (seen, state) = run(&above(90.0, 0), &[hot, None, None]);
        assert_eq!(seen, vec![(0, Change::Triggered)], "unavailable does not clear");
        assert!(state[0].status.active);
        let (seen, _) = run(&above(90.0, 2), &[hot, None, hot, hot]);
        assert!(seen.is_empty(), "a gap starts the wait again");
        let (seen, state) = run(&above(90.0, 0), &[hot, cool]);
        assert_eq!(seen.len(), 2);
        assert_eq!(state[0].status.quiet, None, "read again: no reason");
        let missing = Rule { metric: "gpu0.hotspot".into(), ..above(90.0, 0) };
        let (seen, state) = run(&missing, &[hot]);
        assert!(seen.is_empty());
        assert_eq!(state[0].status.quiet.as_deref(), Some(NOT_READ));
    }

    #[test]
    fn a_fahrenheit_entry_is_stored_in_celsius() {
        assert!((stored(Unit::Celsius, 194.0, true) - 90.0).abs() < 1e-9);
        assert!((shown(Unit::Celsius, 90.0, true) - 194.0).abs() < 1e-9);
        assert_eq!(entry_unit(Unit::Celsius, true), "°F");
        assert_eq!(stored(Unit::Celsius, 90.0, false), 90.0, "°C: as typed");
        assert_eq!(entry_unit(Unit::Celsius, false), "°C");
        assert_eq!(stored(Unit::Hz, 4.5, true), 4.5e9, "clocks are typed in GHz");
        assert_eq!(shown(Unit::Hz, 4.5e9, false), 4.5);
    }

    #[test]
    fn the_notifications_say_what_crossed_in_the_users_unit() {
        let t = Transition { rule: above(90.0, 10), change: Change::Triggered, sensor: "Package (CPU)".into(), unit: Unit::Celsius, value: 92.0 };
        assert_eq!(t.message(false), ("Alert: Package (CPU)".to_string(), "Above 90.0 °C for 10 s (now 92.0 °C).".to_string()));
        assert_eq!(t.message(true).1, "Above 194.0 °F for 10 s (now 197.6 °F).");
        let c = Transition { change: Change::Cleared, value: 80.0, ..t };
        assert_eq!(c.message(false), ("Alert cleared: Package (CPU)".to_string(), "No longer above 90.0 °C for 10 s (now 80.0 °C).".to_string()));
    }

    #[test]
    fn unreadable_saved_alerts_are_dropped_one_by_one() {
        #[derive(serde::Deserialize)]
        struct S {
            #[serde(deserialize_with = "lenient")]
            alerts: Vec<Rule>,
        }
        let s: S = serde_json::from_value(serde_json::json!({ "alerts": [
            { "metric": T, "comparison": "above", "threshold": 90.0, "duration_s": 10 },
            { "metric": T, "comparison": "sideways", "threshold": 90.0, "duration_s": 10 },
            3
        ] }))
        .unwrap();
        assert_eq!(s.alerts, vec![above(90.0, 10)]);
    }
}
