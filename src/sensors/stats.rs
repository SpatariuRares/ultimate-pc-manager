//! Min / Avg / Max per metric since an epoch (pure). The shared sensor
//! service owns one: it counts every reading of the frames it publishes
//! once per read (a slow provider's value, carried along the frames other
//! providers publish, counts once), starts a new epoch on "Reset
//! statistics", and is dropped with the run (when the last interest drops).

use std::collections::HashMap;
use std::time::Instant;

use super::model::{Frame, Kind};

/// One metric's values since the epoch.
#[derive(Clone, Copy, Debug)]
struct Entry {
    min: f64,
    max: f64,
    sum: f64,
    count: u64,
}

/// Min / Avg / Max per metric id since an epoch.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    by_id: HashMap<String, Entry>,
    /// The last frame counted (frames are counted once).
    last_seq: Option<u64>,
    /// When each metric's last counted value was read: a value read no
    /// later is the same read, carried along a newer frame.
    read_at: HashMap<String, Instant>,
    /// When the epoch began: local seconds since 1970 ([`crate::platform::local_secs`]).
    since: i64,
}

impl Stats {
    /// Empty statistics whose epoch began at `since` (local seconds).
    pub fn new(since: i64) -> Self {
        Stats { since, ..Self::default() }
    }

    /// Counts a frame's measures, each value once per read (by when it was
    /// read). Unavailable and not finite values, and counters and text
    /// states, are ignored; a frame not newer than the last is too.
    pub fn observe(&mut self, frame: &Frame) {
        if self.last_seq.is_some_and(|s| frame.seq <= s) {
            return;
        }
        self.last_seq = Some(frame.seq);
        for r in frame.readings().filter(|r| r.kind == Kind::Measure) {
            let Some(v) = r.value.filter(|v| v.is_finite()) else { continue };
            match self.read_at.get_mut(&r.id) {
                Some(last) if r.at <= *last => continue,
                Some(last) => *last = r.at,
                None => {
                    self.read_at.insert(r.id.clone(), r.at);
                }
            }
            let e = self.by_id.entry(r.id.clone()).or_insert(Entry { min: v, max: v, sum: 0.0, count: 0 });
            e.min = e.min.min(v);
            e.max = e.max.max(v);
            e.sum += v;
            e.count += 1;
        }
    }

    /// (min, max) of a metric, if a value was seen since the epoch.
    pub fn min_max(&self, id: &str) -> Option<(f64, f64)> {
        self.by_id.get(id).map(|e| (e.min, e.max))
    }

    /// The mean of a metric's values since the epoch (each frame once).
    pub fn avg(&self, id: &str) -> Option<f64> {
        self.by_id.get(id).filter(|e| e.count > 0).map(|e| e.sum / e.count as f64)
    }

    /// How many values of a metric were counted since the epoch.
    #[cfg_attr(not(test), allow(dead_code))] // contract tests
    pub fn samples(&self, id: &str) -> u64 {
        self.by_id.get(id).map_or(0, |e| e.count)
    }

    /// The last frame counted.
    pub fn seq(&self) -> Option<u64> {
        self.last_seq
    }

    /// When the epoch began (local seconds since 1970).
    pub fn since(&self) -> i64 {
        self.since
    }

    /// The epoch's start as the page shows it: "since 14:32".
    pub fn since_text(&self) -> String {
        let d = self.since.rem_euclid(86_400);
        format!("since {:02}:{:02}", d / 3600, (d % 3600) / 60)
    }

    /// "Reset statistics": starts a new epoch at `now` (local seconds) and
    /// keeps observing; frames and reads already counted are not counted
    /// again.
    pub fn reset(&mut self, now: i64) {
        self.by_id.clear();
        self.since = now;
    }

    /// Forgets everything: the next frame starts a new observation.
    #[cfg_attr(not(test), allow(dead_code))] // the service and the page drop their snapshot instead
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Frame, Group, Reading, Unit};
    use std::time::Instant;

    /// A frame numbered `seq` whose readings were read `seq` ms after a
    /// fixed start (a new read per frame).
    fn frame(seq: u64, values: &[(&str, Option<f64>)]) -> Frame {
        static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        let at = *START.get_or_init(Instant::now) + std::time::Duration::from_millis(seq);
        let readings = values
            .iter()
            .map(|(id, v)| Reading::measure(id, Device::cpu("CPU"), Group::Clocks, id, Unit::Hz, "test", at).with(*v, "unavailable"))
            .collect();
        Frame::build(seq, &[readings], Vec::new())
    }

    #[test]
    fn min_and_max_follow_the_values_seen() {
        let mut stats = Stats::default();
        for (seq, v) in [(1, 4.7e9), (2, 2.98e9), (3, 5.1e9), (4, 4.72e9)] {
            stats.observe(&frame(seq, &[("clock", Some(v))]));
        }
        assert_eq!(stats.min_max("clock"), Some((2.98e9, 5.1e9)));
        assert_eq!(stats.min_max("never-read"), None);
    }

    #[test]
    fn unavailable_and_not_finite_values_are_ignored() {
        let mut stats = Stats::default();
        stats.observe(&frame(1, &[("clock", None)]));
        assert_eq!(stats.min_max("clock"), None, "nothing seen yet");
        stats.observe(&frame(2, &[("clock", Some(3.0e9))]));
        stats.observe(&frame(3, &[("clock", None)]));
        let mut nan = frame(4, &[("clock", Some(1.0))]);
        nan.devices[0].groups[0].readings[0].value = Some(f64::NAN);
        stats.observe(&nan);
        let mut inf = frame(5, &[("clock", Some(1.0))]);
        inf.devices[0].groups[0].readings[0].value = Some(f64::INFINITY);
        stats.observe(&inf);
        assert_eq!(stats.min_max("clock"), Some((3.0e9, 3.0e9)));
    }

    #[test]
    fn counters_get_no_min_or_max() {
        let mut stats = Stats::default();
        let readings = vec![Reading::measure("parked", Device::cpu("CPU"), Group::Clocks, "Parked", Unit::Percent, "test", Instant::now()).with(Some(1.0), "").counter()];
        stats.observe(&Frame::build(1, &[readings], Vec::new()));
        assert_eq!(stats.min_max("parked"), None);
    }

    #[test]
    fn a_frame_is_counted_once() {
        let mut stats = Stats::default();
        stats.observe(&frame(1, &[("clock", Some(2.0))]));
        let mut same = frame(1, &[("clock", Some(1.0))]);
        same.seq = 1;
        stats.observe(&same);
        assert_eq!(stats.min_max("clock"), Some((2.0, 2.0)), "an older or repeated frame changes nothing");
    }

    #[test]
    fn a_reading_not_read_again_is_counted_once_across_frames() {
        let mut stats = Stats::new(0);
        let at = Instant::now();
        let slow = |v: f64, at: Instant| Reading::measure("slow", Device::cpu("CPU"), Group::Clocks, "Slow", Unit::Watt, "test", at).with(Some(v), "");
        // The slow reading (read once) rides along three frames published by others.
        for seq in 1..=3 {
            stats.observe(&Frame::build(seq, &[vec![slow(9.0, at)]], Vec::new()));
        }
        stats.observe(&Frame::build(4, &[vec![slow(3.0, at + std::time::Duration::from_millis(1))]], Vec::new()));
        assert_eq!(stats.samples("slow"), 2, "one sample per read");
        assert_eq!(stats.avg("slow"), Some(6.0), "each read weighs the same");
    }

    #[test]
    fn avg_is_the_mean_of_the_frames_seen_since_the_epoch() {
        let mut stats = Stats::new(0);
        for (seq, v) in [(1, 2.0), (2, 4.0), (3, 9.0)] {
            stats.observe(&frame(seq, &[("clock", Some(v))]));
        }
        assert_eq!(stats.avg("clock"), Some(5.0));
        assert_eq!(stats.avg("never-read"), None);
    }

    #[test]
    fn avg_counts_each_frame_once_and_ignores_missing_values() {
        let mut stats = Stats::new(0);
        stats.observe(&frame(1, &[("clock", Some(2.0))]));
        stats.observe(&frame(1, &[("clock", Some(100.0))]));
        stats.observe(&frame(2, &[("clock", None)]));
        let mut nan = frame(3, &[("clock", Some(1.0))]);
        nan.devices[0].groups[0].readings[0].value = Some(f64::NAN);
        stats.observe(&nan);
        let mut inf = frame(4, &[("clock", Some(1.0))]);
        inf.devices[0].groups[0].readings[0].value = Some(f64::INFINITY);
        stats.observe(&inf);
        stats.observe(&frame(5, &[("clock", Some(4.0))]));
        assert_eq!(stats.avg("clock"), Some(3.0), "a repeated frame, unavailable and not finite values do not count");
    }

    #[test]
    fn counters_and_text_states_get_no_avg() {
        let mut stats = Stats::new(0);
        let at = Instant::now();
        let readings = vec![
            Reading::measure("parked", Device::cpu("CPU"), Group::Clocks, "Parked", Unit::Percent, "test", at).with(Some(1.0), "").counter(),
            Reading::state("plan", Device::cpu("CPU"), Group::General, "Governor", "test", at).with_text(Some("Balanced".into()), ""),
        ];
        stats.observe(&Frame::build(1, &[readings], Vec::new()));
        assert_eq!((stats.avg("parked"), stats.min_max("parked")), (None, None));
        assert_eq!((stats.avg("plan"), stats.min_max("plan")), (None, None));
    }

    #[test]
    fn reset_sets_a_new_epoch_and_keeps_observing() {
        let start = 14 * 3600 + 32 * 60;
        let mut stats = Stats::new(start);
        assert_eq!(stats.since(), start);
        assert_eq!(stats.since_text(), "since 14:32");
        stats.observe(&frame(1, &[("clock", Some(2.0))]));
        stats.observe(&frame(2, &[("clock", Some(8.0))]));
        let later = 86_400 * 3 + 9 * 3600 + 5 * 60 + 59;
        stats.reset(later);
        assert_eq!(stats.since(), later);
        assert_eq!(stats.since_text(), "since 09:05");
        assert_eq!((stats.min_max("clock"), stats.avg("clock")), (None, None), "nothing seen since the new epoch");
        stats.observe(&frame(2, &[("clock", Some(50.0))]));
        assert_eq!(stats.avg("clock"), None, "a frame counted before the reset is not counted again");
        stats.observe(&frame(3, &[("clock", Some(3.0))]));
        stats.observe(&frame(4, &[("clock", Some(5.0))]));
        assert_eq!((stats.min_max("clock"), stats.avg("clock")), (Some((3.0, 5.0)), Some(4.0)));
    }

    #[test]
    fn min_and_max_still_follow_the_values_next_to_avg() {
        let mut stats = Stats::new(0);
        for (seq, v) in [(1, 4.7e9), (2, 2.98e9), (3, 5.1e9), (4, 4.72e9)] {
            stats.observe(&frame(seq, &[("clock", Some(v))]));
        }
        assert_eq!(stats.min_max("clock"), Some((2.98e9, 5.1e9)), "Min/Max behave as before");
        assert_eq!(stats.avg("clock"), Some(4.375e9));
    }

    #[test]
    fn clearing_starts_a_new_observation() {
        let mut stats = Stats::default();
        stats.observe(&frame(1, &[("clock", Some(2.0))]));
        stats.clear();
        assert_eq!(stats.min_max("clock"), None);
        stats.observe(&frame(1, &[("clock", Some(3.0))]));
        assert_eq!(stats.min_max("clock"), Some((3.0, 3.0)), "frames count again after a clear");
    }
}
