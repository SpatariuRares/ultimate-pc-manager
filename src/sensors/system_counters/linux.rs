//! Linux: the system counters, read through an [`FsRoot`]. Today: kernel
//! time per processor and in total (`/proc/stat`) and the processor pressure
//! stall information (`/proc/pressure/cpu`, when the kernel has it).
//!
//! Each further reader adds one `CounterSet` in [`Counters::read`], from its
//! own function below:
//! - memory (`/proc/meminfo`, `/proc/vmstat`): the pages read, the cached
//!   memory, the commit and the swap in use (the memory ticket);
//!
//! Disks (`/proc/diskstats`, only the physical ones of `linux_disks`) are
//! counted from [`disk_counters`]: the idle time, the read and write speed
//! with the bytes since boot, and the seconds per transfer, as the four
//! disk counters the Sampler consumes.

use std::time::Instant;

use crate::availability::{unavailable_error, Feature};
use crate::fs_root::FsRoot;
use crate::linux_disks::{self, Tally};
use crate::perfdata::counter;
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::proc_stat::{self, Stat};

/// The system counters query: the previous tallies, to say what happened since.
pub struct Counters {
    root: FsRoot,
    previous: Option<Stat>,
    /// The disks' tallies of the last read, and when.
    previous_disks: Option<(Instant, Vec<(String, Tally)>)>,
}

impl Counters {
    pub fn open() -> Self {
        Self::at(FsRoot::system())
    }

    pub fn at(root: FsRoot) -> Self {
        Counters { root, previous: None, previous_disks: None }
    }

    /// Counters that already counted `before` (the text of `/proc/diskstats`) at `at` (tests).
    #[cfg(test)]
    pub(crate) fn seeded(root: FsRoot, before: &str, at: Instant) -> Self {
        let mut c = Self::at(root);
        c.previous_disks = Some((at, linux_disks::parse_diskstats(before)));
        c
    }

    pub fn availability(&self) -> Result<(), String> {
        if self.root.probe("proc/stat") {
            Ok(())
        } else {
            Err(unavailable_error(Feature::HardwareSensors))
        }
    }

    /// What the counters count now; `None` when the kernel's tallies cannot be read.
    pub fn read(&mut self) -> Option<Vec<CounterSet>> {
        self.read_at(Instant::now())
    }

    pub(crate) fn read_at(&mut self, at: Instant) -> Option<Vec<CounterSet>> {
        let now = proc_stat::read(&self.root)?;
        let before = self.previous.replace(now.clone());
        let mut sets = vec![kernel_time(before.as_ref(), &now)];
        sets.extend(cpu_pressure(&self.root));
        sets.extend(disk_counters(&self.root, &mut self.previous_disks, at));
        Some(sets)
    }
}

/// Kernel time in percent, per processor (named by its number) and in total
/// ("_Total"). Nothing is counted on the first read: a rate needs a span.
fn kernel_time(before: Option<&Stat>, now: &Stat) -> CounterSet {
    let mut instances = Vec::new();
    if let Some(before) = before {
        if let Some(u) = now.total.since(&before.total) {
            instances.push(Instance::new("_Total", u.kernel));
        }
        for (n, times) in &now.cores {
            let was = before.cores.iter().find(|c| c.0 == *n).map(|c| &c.1);
            if let Some(u) = was.and_then(|w| times.since(w)) {
                instances.push(Instance::new(&n.to_string(), u.kernel));
            }
        }
    }
    CounterSet::new(counter::KERNEL, instances)
}

/// The four disk counters, one instance per physical disk named by its
/// number ("1 sda"): `IDLE` (percent idle), `DREAD` and `DWRITE` (bytes per
/// second, with the bytes since boot) and `DRESP` (seconds per transfer).
/// Nothing is counted on the first read: a rate needs a span. Empty when the
/// kernel gives no disk tallies.
fn disk_counters(root: &FsRoot, previous: &mut Option<(Instant, Vec<(String, Tally)>)>, at: Instant) -> Vec<CounterSet> {
    let Some(tallies) = linux_disks::read_tallies(root) else { return Vec::new() };
    let before = previous.replace((at, tallies.clone()));
    let (mut idle, mut read, mut write, mut resp) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    if let Some((was_at, was)) = before {
        let secs = at.saturating_duration_since(was_at).as_secs_f64();
        for disk in linux_disks::physical_disks(root) {
            let find = |all: &[(String, Tally)]| all.iter().find(|t| t.0 == disk.name).map(|t| t.1);
            let (Some(b), Some(n)) = (find(&was), find(&tallies)) else { continue };
            let Some(r) = linux_disks::rates(&b, &n, secs) else { continue };
            let name = format!("{} {}", disk.number, disk.name);
            idle.push(Instance::new(&name, 100.0 - r.active));
            read.push(Instance::new(&name, r.read_bps).with_raw(n.bytes_read as i64));
            write.push(Instance::new(&name, r.write_bps).with_raw(n.bytes_written as i64));
            resp.push(Instance::new(&name, r.response));
        }
    }
    vec![CounterSet::new(counter::IDLE, idle), CounterSet::new(counter::DREAD, read), CounterSet::new(counter::DWRITE, write), CounterSet::new(counter::DRESP, resp)]
}

/// The share of the last 10 seconds some task waited for a processor, when
/// the kernel has pressure stall information ("cpu"); nothing otherwise.
fn cpu_pressure(root: &FsRoot) -> Option<CounterSet> {
    let avg10 = pressure_avg10(&root.read("proc/pressure/cpu")?)?;
    Some(CounterSet::new(counter::PRESSURE, vec![Instance::new("cpu", avg10)]))
}

/// The `some` line's 10-second average of a pressure file's text.
fn pressure_avg10(text: &str) -> Option<f64> {
    let some = text.lines().find(|l| l.starts_with("some "))?;
    some.split_whitespace().find_map(|w| w.strip_prefix("avg10=")?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn stat(text: &str) -> Stat {
        proc_stat::parse(text).unwrap()
    }

    #[test]
    fn the_first_read_counts_no_kernel_time_and_a_later_one_does() {
        let mut c = Counters::at(fixture("cpu-modern"));
        let first = c.read().unwrap();
        assert!(!first[0].answered(), "a rate needs a span");
        c.previous = Some(stat("cpu  100 0 50 800 50 0 0 0\ncpu0 30 0 15 200 10 0 0 0\ncpu1 30 0 15 200 10 0 0 0\ncpu2 20 0 10 200 15 0 0 0\ncpu3 20 0 10 200 15 0 0 0\n"));
        let set = &c.read().unwrap()[0];
        assert_eq!(set.key, counter::KERNEL);
        // Of 180 ticks, 50 were system, irq or soft-interrupt time.
        assert_eq!(set.instance("_Total").map(|i| i.value.round()), Some(28.0));
        assert!(["0", "1", "2", "3"].iter().all(|n| set.instance(n).is_some()));
    }

    #[test]
    fn pressure_is_read_when_the_kernel_has_it() {
        let sets = Counters::at(fixture("cpu-modern")).read().unwrap();
        let p = sets.iter().find(|s| s.key == counter::PRESSURE).unwrap();
        assert_eq!(p.instance("cpu").map(|i| i.value), Some(1.5));
    }

    #[test]
    fn an_old_kernel_without_pressure_hides_nothing_else() {
        let mut c = Counters::at(fixture("cpu-old"));
        c.previous = Some(stat("cpu  5 0 2 40 3 0 0 0\ncpu0 2 0 1 20 1 0 0 0\ncpu1 3 0 1 20 2 0 0 0\n"));
        let sets = c.read().unwrap();
        assert!(sets.iter().all(|s| s.key != counter::PRESSURE));
        assert!(sets[0].answered());
    }

    #[test]
    fn a_pc_without_proc_stat_reads_nothing_and_is_not_available() {
        let mut c = Counters::at(fixture("desktop"));
        assert!(c.availability().is_err());
        assert!(c.read().is_none());
        assert!(Counters::at(fixture("cpu-modern")).availability().is_ok());
    }

    #[test]
    fn disks_are_counted_per_physical_disk_after_a_span() {
        let t0 = Instant::now();
        let root = fixture("disks");
        let mut prev = None;
        let first = disk_counters(&root, &mut prev, t0);
        assert!(first.iter().all(|s| !s.answered()), "a rate needs a span");
        // Everything counted since boot was counted 10 s before: the same tallies again, 10 s on.
        prev.as_mut().unwrap().1.iter_mut().for_each(|t| {
            t.1 = Tally { reads: t.1.reads.saturating_sub(100), bytes_read: t.1.bytes_read.saturating_sub(10_240_000), ms_reading: t.1.ms_reading.saturating_sub(100), ms_busy: t.1.ms_busy.saturating_sub(500), ..t.1 }
        });
        let sets = disk_counters(&root, &mut prev, t0 + std::time::Duration::from_secs(10));
        let get = |k: &str| sets.iter().find(|s| s.key == k).unwrap();
        // sda: 500 ms busy of 10 s; sdb and nvme0n1 only had that much left to subtract or less.
        assert_eq!(get(counter::IDLE).instance("1 sda").map(|i| i.value), Some(95.0));
        assert_eq!(get(counter::DREAD).instance("1 sda").map(|i| (i.value, i.raw)), Some((1_024_000.0, Some(20_000 * 512))));
        assert!(get(counter::DWRITE).instance("1 sda").is_some() && get(counter::DRESP).instance("1 sda").is_some());
        let names: Vec<&str> = get(counter::DREAD).instances.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["0 nvme0n1", "1 sda", "2 sdb"], "SSD, HDD and NVMe; no partitions or virtual devices");
    }

    #[test]
    fn pressure_text_without_a_some_line_is_nothing() {
        assert_eq!(pressure_avg10("full avg10=3.00 avg60=0 avg300=0 total=1"), None);
        assert_eq!(pressure_avg10("some avg10=0.25 avg60=0 avg300=0 total=1"), Some(0.25));
    }
}
