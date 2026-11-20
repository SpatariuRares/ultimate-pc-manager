//! Observed: how long the add-on has seen each connection. A map from the
//! connection's key (protocol, local end, remote end, PID) to the first
//! reading it was in; a connection gone from a reading is forgotten.

use std::time::{Duration, Instant};

use std::collections::HashMap;

use super::groups::{key, Key};
use super::inventory::{Inventory, Row};

/// When each connection of the last reading was first seen.
#[derive(Default, Debug)]
pub struct Observed {
    first: HashMap<Key, Instant>,
}

impl Observed {
    /// Takes in a reading made at `at`: a new connection is seen from `at`,
    /// a known one keeps its first sight, a missing one is forgotten.
    #[cfg(test)]
    pub fn update(&mut self, rows: &[Row], at: Instant) {
        self.update_with(rows, at, |_| None);
    }

    /// Takes in an inventory: the creation time Windows reports for a
    /// socket, else the first reading it was in.
    pub fn take(&mut self, inventory: &Inventory) {
        self.update_with(&inventory.rows, inventory.at, |row| inventory.ages.get(&key(row)).copied());
    }

    /// As [`Observed::update`], but a row whose age at `at` Windows reports
    /// (`windows_age`) is seen from when Windows created it.
    pub fn update_with(&mut self, rows: &[Row], at: Instant, windows_age: impl Fn(&Row) -> Option<Duration>) {
        let mut first = HashMap::with_capacity(rows.len());
        for row in rows {
            let k = key(row);
            let created = windows_age(row).and_then(|age| at.checked_sub(age));
            let since = created.or_else(|| self.first.get(&k).copied()).unwrap_or(at);
            first.insert(k, since);
        }
        self.first = first;
    }

    /// How many connections it knows.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.first.len()
    }

    /// How long `row` has been seen, at `now`; None when it is not known.
    pub fn age(&self, row: &Row, now: Instant) -> Option<Duration> {
        self.first.get(&key(row)).map(|since| now.saturating_duration_since(*since))
    }
}

/// The Observed field: "325s" under an hour, else "10 h 52 min".
pub fn duration_text(d: Duration) -> String {
    let s = d.as_secs();
    if s < 3600 {
        format!("{s}s")
    } else {
        format!("{} h {} min", s / 3600, s % 3600 / 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{Proto, TcpState};

    fn conn(local: &str, remote: &str, pid: u32) -> Row {
        Row {
            proto: Proto::Tcp,
            local: local.parse().unwrap(),
            remote: Some(remote.parse().unwrap()),
            state: Some(TcpState::Established),
            pid,
        }
    }

    #[test]
    fn the_first_sight_is_kept_across_readings() {
        let t0 = Instant::now();
        let a = conn("10.0.0.2:50000", "1.2.3.4:443", 9);
        let mut seen = Observed::default();
        seen.update(&[a.clone()], t0);
        seen.update(&[a.clone()], t0 + Duration::from_secs(5));
        assert_eq!(seen.age(&a, t0 + Duration::from_secs(325)), Some(Duration::from_secs(325)));
    }

    #[test]
    fn a_connection_gone_from_a_reading_is_forgotten_and_starts_over_when_back() {
        let t0 = Instant::now();
        let a = conn("10.0.0.2:50000", "1.2.3.4:443", 9);
        let b = conn("10.0.0.2:50001", "1.2.3.4:443", 9);
        let mut seen = Observed::default();
        seen.update(&[a.clone(), b.clone()], t0);
        seen.update(&[b.clone()], t0 + Duration::from_secs(10));
        assert_eq!(seen.age(&a, t0 + Duration::from_secs(20)), None, "pruned");
        assert_eq!(seen.len(), 1);
        seen.update(&[a.clone(), b.clone()], t0 + Duration::from_secs(20));
        assert_eq!(seen.age(&a, t0 + Duration::from_secs(25)), Some(Duration::from_secs(5)));
        assert_eq!(seen.age(&b, t0 + Duration::from_secs(25)), Some(Duration::from_secs(25)));
    }

    #[test]
    fn a_state_change_is_the_same_connection_but_another_pid_is_not() {
        let t0 = Instant::now();
        let a = conn("10.0.0.2:50000", "1.2.3.4:443", 9);
        let mut closing = a.clone();
        closing.state = Some(TcpState::CloseWait);
        let mut other = a.clone();
        other.pid = 10;
        let mut seen = Observed::default();
        seen.update(&[a], t0);
        seen.update(&[closing.clone(), other.clone()], t0 + Duration::from_secs(4));
        let at = t0 + Duration::from_secs(6);
        assert_eq!(seen.age(&closing, at), Some(Duration::from_secs(6)));
        assert_eq!(seen.age(&other, at), Some(Duration::from_secs(2)));
    }

    #[test]
    fn the_creation_time_windows_reports_wins_over_the_first_sight() {
        let t0 = Instant::now() + Duration::from_secs(100_000);
        let a = conn("10.0.0.2:50000", "1.2.3.4:443", 9);
        let mut seen = Observed::default();
        // Windows says the socket was 39,120 s old (10 h 52 min) when read.
        let windows_age = Duration::from_secs(39_120);
        seen.update_with(&[a.clone()], t0, |_| Some(windows_age));
        assert_eq!(seen.age(&a, t0 + Duration::from_secs(5)), Some(Duration::from_secs(39_125)));
    }

    #[test]
    fn durations_read_in_seconds_then_hours_and_minutes() {
        let text = |s| duration_text(Duration::from_secs(s));
        assert_eq!(text(0), "0s");
        assert_eq!(text(325), "325s");
        assert_eq!(text(3_599), "3599s");
        assert_eq!(text(3_600), "1 h 0 min");
        assert_eq!(text(39_120), "10 h 52 min");
        assert_eq!(text(39_179), "10 h 52 min", "seconds are dropped");
        assert_eq!(text(200_000), "55 h 33 min");
        assert_eq!(duration_text(Duration::from_millis(1_999)), "1s");
    }
}
