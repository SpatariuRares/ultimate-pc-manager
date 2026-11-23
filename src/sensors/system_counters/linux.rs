//! Linux: the system counters, read through an [`FsRoot`]. Today: kernel
//! time per processor and in total (`/proc/stat`) and the processor pressure
//! stall information (`/proc/pressure/cpu`, when the kernel has it).
//!
//! The memory counters keep the shapes the Windows provider gives, so the
//! sampler fills the same figures from them:
//! - `pagein`: the pages read from disk per second, from the major page
//!   faults (`pgmajfault` in `/proc/vmstat`). A major fault is a page the
//!   kernel had to read from disk to resolve a fault, which is what the
//!   Windows "Pages Input/sec" counts. `pgpgin` is not used: it counts every
//!   read from a block device, file reads that never faulted included;
//! - `pagefile`: the swap in use in percent (`/proc/meminfo`);
//! - `perfinfo`: the commit and its limit, the page cache, the threads
//!   and the open file handles.
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
    /// The major faults counted and when, to say how many happened per second.
    faults: Option<(u64, Instant)>,
}

impl Counters {
    pub fn open() -> Self {
        Self::at(FsRoot::system())
    }

    pub fn at(root: FsRoot) -> Self {
        Counters { root, previous: None, previous_disks: None, faults: None }
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
        sets.push(pages_read(&self.root, &mut self.faults, at));
        sets.push(swap_in_use(&self.root));
        sets.push(memory_info(&self.root));
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

/// The figure `key` of a `/proc/meminfo` text, in bytes (the file counts kB).
fn meminfo_bytes(text: &str, key: &str) -> Option<f64> {
    let line = text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))?;
    let kb: f64 = line.split_whitespace().next()?.parse().ok()?;
    Some(kb * 1024.0)
}

/// The major page faults since boot (`pgmajfault` of `/proc/vmstat`).
fn major_faults(root: &FsRoot) -> Option<u64> {
    root.read("proc/vmstat")?.lines().find_map(|l| l.strip_prefix("pgmajfault ")?.trim().parse().ok())
}

/// The pages read from disk per second since the previous read (with the
/// tally since boot as the raw value). Nothing on the first read, which has
/// no span, nor when the tally went back (a reset) or cannot be read.
fn pages_read(root: &FsRoot, previous: &mut Option<(u64, Instant)>, at: Instant) -> CounterSet {
    let mut instances = Vec::new();
    if let Some(now) = major_faults(root) {
        if let Some((before, then)) = previous.replace((now, at)) {
            let secs = at.saturating_duration_since(then).as_secs_f64();
            if now >= before && secs > 0.0 {
                instances.push(Instance::new("", (now - before) as f64 / secs).with_raw(now as i64));
            }
        }
    }
    CounterSet::new(counter::PAGEIN, instances)
}

/// The swap in use, in percent; nothing when the PC has no swap (a gap in
/// an optional counter, as for a Windows PC without a page file).
fn swap_in_use(root: &FsRoot) -> CounterSet {
    let used = root.read("proc/meminfo").and_then(|text| {
        let total = meminfo_bytes(&text, "SwapTotal").filter(|t| *t > 0.0)?;
        let free = meminfo_bytes(&text, "SwapFree")?;
        Some(Instance::new("_total", ((total - free) / total * 100.0).clamp(0.0, 100.0)))
    });
    CounterSet::new(counter::PAGEFILE, used.into_iter().collect())
}

/// The commit (`Committed_AS`) and its limit (`CommitLimit`), the page cache
/// (`Cached`), the threads (the scheduler's count in `/proc/loadavg`) and the
/// open file handles (`/proc/sys/fs/file-nr`), in bytes or units. The commit
/// figures are required (no `/proc/meminfo`, no answer); the other two are
/// left out when they cannot be read.
fn memory_info(root: &FsRoot) -> CounterSet {
    let mut instances = Vec::new();
    if let Some(text) = root.read("proc/meminfo") {
        let figures = [("commit", "Committed_AS"), ("commit_limit", "CommitLimit"), ("cached", "Cached")];
        let found: Vec<_> = figures.iter().filter_map(|(name, key)| Some(Instance::new(name, meminfo_bytes(&text, key)?))).collect();
        if found.len() == figures.len() {
            instances = found;
        }
    }
    if !instances.is_empty() {
        let threads = root.read("proc/loadavg").and_then(|t| t.split_whitespace().nth(3)?.split('/').nth(1)?.parse::<f64>().ok());
        let handles = root.read("proc/sys/fs/file-nr").and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok());
        instances.extend(threads.map(|v| Instance::new("threads", v)));
        instances.extend(handles.map(|v| Instance::new("handles", v)));
    }
    CounterSet::new(crate::perfdata::PERF_INFO, instances)
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

    /// The set of `key` among `sets`.
    fn set<'a>(sets: &'a [CounterSet], key: &str) -> &'a CounterSet {
        sets.iter().find(|s| s.key == key).unwrap()
    }

    #[test]
    fn the_pages_read_are_the_major_faults_per_second_with_the_tally_since_boot() {
        let root = fixture("memory");
        let t0 = Instant::now();
        let mut previous = None;
        assert!(!pages_read(&root, &mut previous, t0).answered(), "the first read has no span");
        // 12000 faults now, 11000 two seconds ago.
        previous = Some((11_000, t0));
        let set = pages_read(&root, &mut previous, t0 + std::time::Duration::from_secs(2));
        assert_eq!(set.key, counter::PAGEIN);
        let i = set.instances.first().unwrap();
        assert_eq!((i.value, i.raw), (500.0, Some(12_000)));
        // A tally that went back (a reset) is not a negative rate.
        previous = Some((50_000, t0));
        assert!(!pages_read(&root, &mut previous, t0 + std::time::Duration::from_secs(2)).answered());
    }

    #[test]
    fn a_kernel_without_vmstat_answers_no_pages_read() {
        let mut c = Counters::at(fixture("memory-bare"));
        c.read().unwrap();
        let sets = c.read().unwrap();
        assert!(!set(&sets, counter::PAGEIN).answered());
    }

    #[test]
    fn the_commit_the_cache_the_threads_and_the_handles_come_from_the_kernel_tallies() {
        let sets = Counters::at(fixture("memory")).read().unwrap();
        let info = set(&sets, crate::perfdata::PERF_INFO);
        let value = |n: &str| info.instance(n).map(|i| i.value);
        assert_eq!(value("commit"), Some(6_000_000.0 * 1024.0));
        assert_eq!(value("commit_limit"), Some(10_240_000.0 * 1024.0));
        assert_eq!(value("cached"), Some(4_000_000.0 * 1024.0));
        assert_eq!(value("threads"), Some(1234.0));
        assert_eq!(value("handles"), Some(9120.0));
    }

    #[test]
    fn the_threads_and_handles_missing_leave_the_commit_figures_answered() {
        let sets = Counters::at(fixture("memory-bare")).read().unwrap();
        let info = set(&sets, crate::perfdata::PERF_INFO);
        assert!(info.instance("commit").is_some() && info.instance("cached").is_some());
        assert!(info.instance("threads").is_none() && info.instance("handles").is_none());
    }

    #[test]
    fn the_swap_in_use_is_a_percentage_and_a_pc_without_swap_has_none() {
        let sets = Counters::at(fixture("memory")).read().unwrap();
        assert_eq!(set(&sets, counter::PAGEFILE).instance("_total").map(|i| i.value), Some(50.0));
        let bare = Counters::at(fixture("memory-bare")).read().unwrap();
        assert!(!set(&bare, counter::PAGEFILE).answered(), "no swap is a gap in an optional counter");
    }

    #[test]
    fn without_meminfo_the_commit_figures_are_a_gap() {
        let sets = Counters::at(fixture("cpu-modern")).read().unwrap();
        assert!(!set(&sets, crate::perfdata::PERF_INFO).answered());
        assert!(!set(&sets, counter::PAGEFILE).answered());
    }

    #[test]
    fn meminfo_figures_are_read_in_bytes_and_a_prefix_is_not_a_match() {
        let text = "Cached:  10 kB\nSwapCached:  99 kB\n";
        assert_eq!(meminfo_bytes(text, "Cached"), Some(10.0 * 1024.0));
        assert_eq!(meminfo_bytes(text, "Swap"), None);
    }

    /// Copies the files of `from` into `to`, replacing the ones already there.
    fn overlay(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                overlay(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn with_the_memory_and_the_disk_files_every_health_source_is_answered() {
        // A PC that has both: the disks' files, then the memory's over them.
        let dir = std::env::temp_dir().join(format!("upm-counters-both-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux");
        overlay(&fixtures.join("disks"), &dir);
        overlay(&fixtures.join("memory"), &dir);
        let diskstats = std::fs::read_to_string(dir.join("proc/diskstats")).unwrap();
        let t0 = Instant::now();
        let mut c = Counters::seeded(FsRoot::at(dir.clone()), &diskstats, t0);
        c.read_at(t0).unwrap();
        // The fixture's processors did not tick between the two reads.
        c.previous = Some(stat("cpu  90 0 40 790 40 0 0 0
cpu0 45 0 20 395 20 0 0 0
cpu1 45 0 20 395 20 0 0 0
"));
        let sets = c.read_at(t0 + std::time::Duration::from_secs(2)).unwrap();
        let missing: Vec<&str> = crate::perfdata::sources().filter(|k| !sets.iter().any(|s| s.key == *k && s.answered())).collect();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(missing.is_empty(), "sources still unanswered: {missing:?}");
    }
}
