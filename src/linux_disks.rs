//! The physical disks of a Linux system and their I/O tallies, read through
//! an [`FsRoot`] so tests run on any host against fixture trees.
//!
//! A physical disk is a whole block device of `/sys/block` that has a
//! `device` entry (hardware or a virtual disk of a hypervisor). Partitions
//! (they are folders inside their disk's, and rows of `/proc/diskstats`),
//! loop, RAM, compressed-RAM and device-mapper devices, and optical or
//! floppy drives are not disks here.
//!
//! The disks are numbered in the order of their names, which gives the
//! number the pages and the counters share (a disk that is unplugged
//! renumbers the ones after it, as the kernel's names do not say better).

use crate::fs_root::FsRoot;

/// What a disk's kernel counters count: the bytes of a sector in `/proc/diskstats`.
const SECTOR_BYTES: u64 = 512;

/// What kind of disk it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiskKind {
    Nvme,
    Ssd,
    Hdd,
}

impl DiskKind {
    /// The word the pages show: "NVMe", "SSD" or "HDD".
    pub fn label(self) -> &'static str {
        match self {
            DiskKind::Nvme => "NVMe",
            DiskKind::Ssd => "SSD",
            DiskKind::Hdd => "HDD",
        }
    }
}

/// One physical disk.
#[derive(Clone, Debug, PartialEq)]
pub struct PhysicalDisk {
    pub number: u32,
    /// The kernel's name ("sda", "nvme0n1").
    pub name: String,
    /// The model the device reports; empty when it does not.
    pub model: String,
    pub kind: DiskKind,
    pub capacity: u64,
}

/// Block devices that are never disks, by their name's start.
const NOT_DISKS: &[&str] = &["loop", "ram", "zram", "dm-", "md", "sr", "fd", "nbd"];

/// The physical disks, in number order.
pub fn physical_disks(root: &FsRoot) -> Vec<PhysicalDisk> {
    let mut disks = Vec::new();
    for name in root.list_dir("sys/block") {
        let base = format!("sys/block/{name}");
        let is_disk = !NOT_DISKS.iter().any(|p| name.starts_with(p)) && root.probe(&format!("{base}/device")) && !root.probe(&format!("{base}/partition"));
        if !is_disk {
            continue;
        }
        let rotational = root.read(&format!("{base}/queue/rotational")).is_some_and(|t| t == "1");
        let kind = if name.starts_with("nvme") {
            DiskKind::Nvme
        } else if rotational {
            DiskKind::Hdd
        } else {
            DiskKind::Ssd
        };
        let sectors: u64 = root.read(&format!("{base}/size")).and_then(|t| t.parse().ok()).unwrap_or(0);
        disks.push(PhysicalDisk {
            number: disks.len() as u32,
            model: root.read(&format!("{base}/device/model")).unwrap_or_default(),
            name,
            kind,
            capacity: sectors * SECTOR_BYTES,
        });
    }
    disks
}

/// What the kernel has counted for one disk since boot.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Tally {
    pub reads: u64,
    pub bytes_read: u64,
    pub ms_reading: u64,
    pub writes: u64,
    pub bytes_written: u64,
    pub ms_writing: u64,
    /// Milliseconds the disk had requests in flight.
    pub ms_busy: u64,
}

/// The tallies of `/proc/diskstats`, by device name; `None` when it cannot be read.
pub fn read_tallies(root: &FsRoot) -> Option<Vec<(String, Tally)>> {
    Some(parse_diskstats(&root.read("proc/diskstats")?))
}

pub(crate) fn parse_diskstats(text: &str) -> Vec<(String, Tally)> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.nth(2)?.to_string();
            let f: Vec<u64> = words.map_while(|w| w.parse().ok()).collect();
            // reads, merged, sectors, ms, writes, merged, sectors, ms, in flight, ms busy.
            (f.len() >= 10).then(|| {
                let tally = Tally { reads: f[0], bytes_read: f[2] * SECTOR_BYTES, ms_reading: f[3], writes: f[4], bytes_written: f[6] * SECTOR_BYTES, ms_writing: f[7], ms_busy: f[9] };
                (name, tally)
            })
        })
        .collect()
}

/// A disk's figures over a span.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rates {
    /// Percent of the span the disk had requests in flight.
    pub active: f64,
    pub read_bps: f64,
    pub write_bps: f64,
    /// Seconds per transfer; 0 when none finished.
    pub response: f64,
}

/// What `now` says about the span since `before`, `secs` long; `None` for no span
/// or a tally that went back (the device was replaced).
pub fn rates(before: &Tally, now: &Tally, secs: f64) -> Option<Rates> {
    if secs <= 0.0 {
        return None;
    }
    let d = |a: u64, b: u64| b.checked_sub(a);
    let transfers = d(before.reads, now.reads)? + d(before.writes, now.writes)?;
    let ms = d(before.ms_reading, now.ms_reading)? + d(before.ms_writing, now.ms_writing)?;
    Some(Rates {
        active: (d(before.ms_busy, now.ms_busy)? as f64 / (secs * 1000.0) * 100.0).clamp(0.0, 100.0),
        read_bps: d(before.bytes_read, now.bytes_read)? as f64 / secs,
        write_bps: d(before.bytes_written, now.bytes_written)? as f64 / secs,
        response: if transfers == 0 { 0.0 } else { ms as f64 / transfers as f64 / 1000.0 },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    #[test]
    fn a_disk_kind_is_shown_by_its_label() {
        assert_eq!([DiskKind::Nvme, DiskKind::Ssd, DiskKind::Hdd].map(DiskKind::label), ["NVMe", "SSD", "HDD"]);
        let kinds: Vec<DiskKind> = physical_disks(&fixture("disks")).iter().map(|d| d.kind).collect();
        assert_eq!(kinds, [DiskKind::Nvme, DiskKind::Ssd, DiskKind::Hdd]);
    }

    #[test]
    fn only_whole_physical_disks_are_listed_with_their_type_model_and_size() {
        let disks = physical_disks(&fixture("disks"));
        let shown: Vec<(u32, &str, &str, &str, u64)> = disks.iter().map(|d| (d.number, d.name.as_str(), d.kind.label(), d.model.as_str(), d.capacity)).collect();
        assert_eq!(
            shown,
            [
                (0, "nvme0n1", "NVMe", "Samsung SSD 990 PRO 1TB", 1_000_215_216 * 512),
                (1, "sda", "SSD", "Samsung SSD 870 EVO 1TB", 1_953_525_168 * 512),
                (2, "sdb", "HDD", "WDC WD40EFRX-68N32N0", 7_814_037_168 * 512),
            ],
            "no partitions, loop, ram, zram, dm or optical devices"
        );
    }

    #[test]
    fn a_pc_without_block_devices_has_no_disks() {
        assert!(physical_disks(&fixture("desktop")).is_empty());
        assert!(read_tallies(&fixture("desktop")).is_none());
    }

    #[test]
    fn the_tallies_are_read_in_bytes() {
        let tallies = read_tallies(&fixture("disks")).unwrap();
        let sda = tallies.iter().find(|t| t.0 == "sda").unwrap().1;
        assert_eq!((sda.bytes_read, sda.bytes_written, sda.ms_busy, sda.reads, sda.writes), (20_000 * 512, 40_000 * 512, 1000, 1000, 2000));
    }

    #[test]
    fn rates_over_a_span_give_activity_speed_and_response_time() {
        let before = Tally { reads: 100, bytes_read: 1_000_000, ms_reading: 50, writes: 100, bytes_written: 0, ms_writing: 50, ms_busy: 100 };
        let now = Tally { reads: 200, bytes_read: 3_000_000, ms_reading: 150, writes: 150, bytes_written: 1_000_000, ms_writing: 100, ms_busy: 600 };
        let r = rates(&before, &now, 2.0).unwrap();
        assert_eq!(r.active, 25.0, "500 ms busy of 2 s");
        assert_eq!((r.read_bps, r.write_bps), (1_000_000.0, 500_000.0));
        // 150 ms over 150 transfers.
        assert!((r.response - 0.001).abs() < 1e-12);
        assert!(rates(&before, &now, 0.0).is_none(), "no span");
        assert!(rates(&now, &before, 2.0).is_none(), "a tally that went back");
    }

    #[test]
    fn an_idle_disk_has_no_response_time_to_divide() {
        let t = Tally { reads: 5, ..Tally::default() };
        assert_eq!(rates(&t, &t, 1.0).map(|r| (r.active, r.response)), Some((0.0, 0.0)));
    }
}
