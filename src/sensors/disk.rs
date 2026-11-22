//! The disk provider ("Disk counters"): one device per physical disk, with
//! Utilization (activity) and Speed (read and write) from the PDH
//! `PhysicalDisk(*)` counters; NVMe and SATA disks add Temperatures and
//! S.M.A.R.T. (source [`SMART_SOURCE`]), read every [`SMART_PERIOD`]: the
//! NVMe health log whenever the app is elevated, the SATA attributes only
//! after the Manual SMART Probe consent too ([`blocked`]).

use std::time::{Duration, Instant};

#[cfg_attr(not(windows), allow(unused_imports))]
use super::model::Instance;
use super::model::{CounterSet, Device, DeviceKind, Group, Reading, Unit};
#[cfg_attr(not(windows), allow(unused_imports))]
use crate::perfdata::counter;
use super::provider::Provider;

/// NVMe S.M.A.R.T.: the health log and its readings.
pub mod nvme;

/// ATA (SATA) S.M.A.R.T.: the attributes and thresholds pages and their readings.
pub mod ata;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Disk counters";
/// "Read from" of the S.M.A.R.T. and drive temperature readings (the
/// disks' own health data, not the counters); "Sources on this PC" lists it
/// with its own state.
pub const SMART_SOURCE: &str = "S.M.A.R.T.";

/// Why the S.M.A.R.T. rows are empty when the app is not elevated.
pub const NEEDS_ADMIN: &str = "needs administrator";
/// Why the S.M.A.R.T. rows are empty before the user ran the Manual SMART Probe.
pub const PROBE_OFF: &str = "Manual SMART Probe not run";

/// What the last direct S.M.A.R.T. query of a disk gave.
#[derive(Clone, Debug, PartialEq)]
pub enum Probe<T> {
    /// The app is not elevated: the query is not even tried.
    NeedsAdmin,
    /// The user has not agreed to the Manual SMART Probe: not tried.
    Off,
    /// The query failed; why.
    Failed(String),
    Read(T),
}

impl<T> Probe<T> {
    /// What was read, and why nothing was ("" when read).
    pub fn value(&self) -> (Option<&T>, &str) {
        match self {
            Probe::Read(v) => (Some(v), ""),
            Probe::NeedsAdmin => (None, NEEDS_ADMIN),
            Probe::Off => (None, PROBE_OFF),
            Probe::Failed(why) => (None, why.as_str()),
        }
    }
}

/// Whether a direct disk query may run: never without administrator
/// rights. The ATA pass-through of SATA disks (`ata`) also waits for the
/// user to agree to the Manual SMART Probe; the NVMe health log, a standard
/// storage query, reads by itself when elevated.
pub fn blocked<T>(ata: bool, admin: bool, consent: bool) -> Option<Probe<T>> {
    match (admin, ata && !consent) {
        (false, _) => Some(Probe::NeedsAdmin),
        (true, true) => Some(Probe::Off),
        (true, false) => None,
    }
}

/// `\\.\PhysicalDrive{n}`, open for read and write (both S.M.A.R.T.
/// queries need it); closed when dropped.
#[cfg(windows)]
pub(super) struct Drive(crate::winsys::Handle);

#[cfg(windows)]
impl Drive {
    /// Opens physical drive `n` (administrator only).
    pub(super) fn open(n: u32) -> Result<Drive, String> {
        use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE};
        use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
        let path = crate::winsys::wide_path(std::path::Path::new(&format!(r"\\.\PhysicalDrive{n}")));
        // SAFETY: `path` is NUL-terminated and outlives the call; no
        // security attributes or template handle are passed.
        let h = unsafe { CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, std::ptr::null(), OPEN_EXISTING, 0, std::ptr::null_mut()) };
        if h == INVALID_HANDLE_VALUE {
            Err("the disk cannot be opened".into())
        } else {
            Ok(Drive(crate::winsys::Handle(h)))
        }
    }

    /// The open handle, for DeviceIoControl.
    pub(super) fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0 .0
    }
}

/// The user's consent to the Manual SMART Probe, given by Power & Freq
/// from its settings (the provider runs on its own thread).
static PROBE_CONSENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Records the user's consent to the Manual SMART Probe.
pub fn set_probe_consent(consent: bool) {
    PROBE_CONSENT.store(consent, std::sync::atomic::Ordering::Relaxed);
}

#[cfg_attr(not(windows), allow(dead_code))]
fn probe_consent() -> bool {
    PROBE_CONSENT.load(std::sync::atomic::Ordering::Relaxed)
}

/// Which disks have a direct S.M.A.R.T. query, by their bus: `Some(false)`
/// = NVMe health log, `Some(true)` = ATA attributes, `None` = none (USB,
/// RAID, virtual...).
pub fn smart_bus(bus: &str) -> Option<bool> {
    match bus {
        "NVMe" => Some(false),
        "SATA" | "ATA" => Some(true),
        _ => None,
    }
}

/// The S.M.A.R.T. query of a disk, by its kind.
#[derive(Clone, Debug, PartialEq)]
pub enum DiskSmart {
    Nvme(nvme::Smart),
    Ata(ata::Smart),
}
const NO_COUNTER: &str = "no counter on this PC";

/// The physical disk number of a `PhysicalDisk` instance ("0 C:", "1");
/// `None` for "_Total".
pub fn disk_number(instance: &str) -> Option<u32> {
    instance.split_whitespace().next()?.parse().ok()
}

/// One physical disk's raw values.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskSample {
    pub number: u32,
    /// The model ("Samsung SSD 990 PRO 2TB"); empty = unknown.
    pub model: String,
    /// `% Idle Time` (activity = 100 − idle).
    pub idle_percent: Option<f64>,
    pub read_bytes_per_sec: Option<f64>,
    pub write_bytes_per_sec: Option<f64>,
    /// The last S.M.A.R.T. query (NVMe and SATA disks only).
    pub smart: Option<DiskSmart>,
}

/// How often the NVMe health log is read (slower than the refresh).
pub const SMART_PERIOD: Duration = Duration::from_secs(10);

/// Whether the health log last read at `last` is due again at `now`.
pub fn smart_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= SMART_PERIOD)
}

impl DiskSample {
    /// The readings of this disk, in the order shown.
    pub fn readings(&self, at: Instant) -> Vec<Reading> {
        let n = self.number;
        let name = if self.model.is_empty() { format!("Disk {n}") } else { self.model.clone() };
        let disk = Device::new(DeviceKind::Disk, &format!("disk{n}"), &name);
        let row = |id: &str, group: Group, name: &str, unit: Unit, value: Option<f64>| {
            Reading::measure(&format!("disk{n}.{id}"), disk.clone(), group, name, unit, SOURCE, at).with(value, NO_COUNTER)
        };
        let activity = self.idle_percent.map(|idle| (100.0 - idle).clamp(0.0, 100.0));
        let mut out = vec![
            row("utilization.activity_percent", Group::Utilization, "Activity", Unit::Percent, activity),
            row("speed.read_bytes_per_sec", Group::Speed, "Read Speed", Unit::BytesPerSec, self.read_bytes_per_sec.map(|v| v.max(0.0))),
            row("speed.write_bytes_per_sec", Group::Speed, "Write Speed", Unit::BytesPerSec, self.write_bytes_per_sec.map(|v| v.max(0.0))),
        ];
        match &self.smart {
            Some(DiskSmart::Nvme(smart)) => out.extend(nvme::readings(n, &disk, smart, at)),
            Some(DiskSmart::Ata(smart)) => out.extend(ata::readings(n, &disk, smart, at)),
            None => {}
        }
        out
    }
}

/// Rate counters need this much time between two collections.
#[cfg_attr(not(windows), allow(dead_code))]
const MIN_SPAN: Duration = Duration::from_millis(200);

/// The disk provider. Built on its worker thread (holds a PDH query).
pub struct DiskCounters {
    #[cfg(windows)]
    query: Option<super::pdh::Query>,
    #[cfg(windows)]
    collected: Instant,
    /// (disk number, model) of every physical disk.
    #[cfg_attr(not(windows), allow(dead_code))]
    models: Vec<(u32, String)>,
    /// NVMe and SATA disks: number, whether ATA, last direct S.M.A.R.T.
    /// query and when it was made (`None` while the query is blocked).
    #[cfg_attr(not(windows), allow(dead_code))]
    smart: Vec<(u32, bool, Option<(DiskSmart, Instant)>)>,
    /// Whether the hardware sensors are looked at: the health of the disks
    /// (S.M.A.R.T.) is read only then (see [`Provider::hardware_wanted`]).
    #[cfg_attr(not(windows), allow(dead_code))]
    health: bool,
    /// What the last read counted: every disk's idle time, read and write
    /// rate, the last two with their raw totals (see [`Provider::counted`]).
    counted: Vec<CounterSet>,
}

impl DiskCounters {
    pub fn new() -> Self {
        Self::over(super::machine::disks())
    }

    /// A provider for these disks (their models and buses are its input).
    pub fn over(disks: Vec<crate::perfdata::DiskInfo>) -> Self {
        DiskCounters {
            #[cfg(windows)]
            query: super::pdh::Query::open(&[
                (counter::IDLE, r"\PhysicalDisk(*)\% Idle Time"),
                (counter::DREAD, r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
                (counter::DWRITE, r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
                (counter::DRESP, r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"),
            ]),
            #[cfg(windows)]
            collected: Instant::now(),
            models: disks.iter().map(|d| (d.number, d.model.clone())).collect(),
            smart: disks.iter().filter_map(|d| smart_bus(d.bus).map(|ata| (d.number, ata, None))).collect(),
            health: true,
            counted: Vec::new(),
        }
    }

    #[cfg(windows)]
    fn sample(&mut self) -> Vec<DiskSample> {
        let mut disks: Vec<DiskSample> = self.models.iter().map(|(n, m)| DiskSample { number: *n, model: m.clone(), ..Default::default() }).collect();
        let now = Instant::now();
        let admin = crate::platform::is_admin();
        let consent = probe_consent();
        for (n, ata, last) in self.smart.iter_mut().filter(|_| self.health) {
            let gate = if *ata { blocked(true, admin, consent).map(DiskSmart::Ata) } else { blocked(false, admin, consent).map(DiskSmart::Nvme) };
            let smart = match gate {
                Some(not_tried) => {
                    // Not cached: consenting (or elevating) reads at once.
                    *last = None;
                    not_tried
                }
                None => {
                    let (read, when) = match last.take() {
                        Some((cached, t)) if !smart_due(Some(t), now) => (cached, t),
                        _ => (if *ata { DiskSmart::Ata(ata::query(*n)) } else { DiskSmart::Nvme(nvme::query(*n)) }, now),
                    };
                    *last = Some((read.clone(), when));
                    read
                }
            };
            if let Some(d) = disks.iter_mut().find(|d| d.number == *n) {
                d.smart = Some(smart);
            }
        }
        let Some(q) = self.query.as_mut() else { return disks };
        let wait = MIN_SPAN.saturating_sub(self.collected.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        q.collect();
        self.collected = Instant::now();
        let counted = [q.counted(counter::IDLE, false), q.counted(counter::DREAD, true), q.counted(counter::DWRITE, true)];
        for (counted, set) in counted.iter().zip([
            (|d: &mut DiskSample, v: f64| d.idle_percent = Some(v)) as fn(&mut DiskSample, f64),
            |d, v| d.read_bytes_per_sec = Some(v),
            |d, v| d.write_bytes_per_sec = Some(v),
        ]) {
            for Instance { name: inst, value: v, .. } in &counted.instances {
                let v = *v;
                let Some(n) = disk_number(inst) else { continue };
                let i = match disks.iter().position(|d| d.number == n) {
                    Some(i) => i,
                    None => {
                        disks.push(DiskSample { number: n, ..Default::default() });
                        disks.len() - 1
                    }
                };
                set(&mut disks[i], v);
            }
        }
        disks.sort_by_key(|d| d.number);
        // The average response time is only reported (the readings have no row for it).
        self.counted = counted.into_iter().chain([q.counted(counter::DRESP, false)]).collect();
        disks
    }
}

impl Provider for DiskCounters {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        #[cfg(windows)]
        return match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        };
        #[cfg(not(windows))]
        Err("Windows only".into())
    }

    fn hardware_wanted(&mut self, wanted: bool) {
        self.health = wanted;
    }

    fn counted(&mut self) -> Vec<CounterSet> {
        std::mem::take(&mut self.counted)
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        #[cfg(windows)]
        return self.sample().iter().flat_map(|d| d.readings(at)).collect();
        #[cfg(not(windows))]
        {
            let _ = at;
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Frame, Kind};

    fn two_disks() -> Vec<Reading> {
        let at = Instant::now();
        let nvme = DiskSample { number: 0, model: "Samsung SSD 990 PRO 2TB".into(), idle_percent: Some(75.0), read_bytes_per_sec: Some(2.5e9), write_bytes_per_sec: Some(40e6), smart: None };
        let sata = DiskSample { number: 1, model: String::new(), idle_percent: None, read_bytes_per_sec: Some(0.0), write_bytes_per_sec: None, smart: None };
        nvme.readings(at).into_iter().chain(sata.readings(at)).collect()
    }

    #[test]
    fn each_physical_disk_is_a_device_with_utilization_then_speed() {
        let frame = Frame::build(1, &[two_disks()], Vec::new());
        let names: Vec<_> = frame.devices.iter().map(|d| (d.device.kind, d.device.name.as_str())).collect();
        assert_eq!(names, vec![(DeviceKind::Disk, "Samsung SSD 990 PRO 2TB"), (DeviceKind::Disk, "Disk 1")]);
        for d in &frame.devices {
            let groups: Vec<_> = d.groups.iter().map(|g| g.group.name()).collect();
            assert_eq!(groups, vec!["Utilization", "Speed"]);
            let rows: Vec<_> = d.groups.iter().flat_map(|g| &g.readings).map(|r| r.name.as_str()).collect();
            assert_eq!(rows, vec!["Activity", "Read Speed", "Write Speed"]);
        }
    }

    #[test]
    fn activity_is_the_time_the_disk_was_not_idle() {
        let frame = Frame::build(1, &[two_disks()], Vec::new());
        let activity = frame.reading("disk0.utilization.activity_percent").unwrap();
        assert_eq!(activity.text(), "25 %");
        assert_eq!(activity.kind, Kind::Measure, "has Min/Max");
        assert_eq!(frame.reading("disk0.speed.read_bytes_per_sec").unwrap().text(), "2.50 GB/s");
        assert_eq!(frame.reading("disk0.speed.write_bytes_per_sec").unwrap().text(), "40.0 MB/s");
    }

    #[test]
    fn a_missing_disk_counter_is_unavailable_not_zero() {
        let frame = Frame::build(1, &[two_disks()], Vec::new());
        assert_eq!(frame.reading("disk1.utilization.activity_percent").unwrap().value, None);
        assert_eq!(frame.reading("disk1.speed.write_bytes_per_sec").unwrap().text(), NO_COUNTER);
        assert_eq!(frame.reading("disk1.speed.read_bytes_per_sec").unwrap().value, Some(0.0), "an idle disk reads zero");
    }

    #[test]
    fn an_nvme_disk_adds_temperatures_first_and_smart_last() {
        let nvme = DiskSample { number: 0, model: "NVMe".into(), smart: Some(DiskSmart::Nvme(nvme::Smart::NeedsAdmin)), ..Default::default() };
        let frame = Frame::build(1, &[nvme.readings(Instant::now())], Vec::new());
        let groups: Vec<_> = frame.devices[0].groups.iter().map(|g| g.group.name()).collect();
        assert_eq!(groups, vec!["Temperatures", "Utilization", "Speed", "S.M.A.R.T."]);
        assert_eq!(frame.devices.len(), 1, "one device for the counters and the health log");
    }

    #[test]
    fn a_sata_disk_is_queried_only_elevated_and_after_the_users_consent() {
        let sata = true;
        assert_eq!(blocked::<()>(sata, false, true), Some(Probe::NeedsAdmin), "never without administrator");
        assert_eq!(blocked::<()>(sata, false, false), Some(Probe::NeedsAdmin));
        assert_eq!(blocked::<()>(sata, true, false), Some(Probe::Off), "never without the Manual SMART Probe");
        assert_eq!(blocked::<()>(sata, true, true), None);
        assert_eq!(Probe::<()>::Off.value(), (None, PROBE_OFF));
    }

    #[test]
    fn the_nvme_health_log_reads_by_itself_when_elevated() {
        let nvme = false;
        assert_eq!(blocked::<()>(nvme, true, false), None, "no Manual SMART Probe needed");
        assert_eq!(blocked::<()>(nvme, true, true), None);
        assert_eq!(blocked::<()>(nvme, false, true), Some(Probe::NeedsAdmin), "never without administrator");
        assert_eq!(blocked::<()>(nvme, false, false), Some(Probe::NeedsAdmin));
    }

    #[test]
    fn smart_readings_are_read_from_smart_not_the_disk_counters() {
        for smart in [DiskSmart::Nvme(nvme::Smart::NeedsAdmin), DiskSmart::Ata(ata::Smart::Off)] {
            let disk = DiskSample { number: 0, model: "Disk".into(), smart: Some(smart), ..Default::default() };
            let rows = disk.readings(Instant::now());
            for r in &rows {
                let from_smart = matches!(r.group, Group::Smart | Group::Temperatures);
                assert_eq!(r.source, if from_smart { SMART_SOURCE } else { SOURCE }, "{}", r.id);
            }
        }
        assert_eq!(SMART_SOURCE, "S.M.A.R.T.");
    }

    #[test]
    fn sata_disks_get_ata_counters_and_nvme_disks_the_health_log() {
        assert_eq!(smart_bus("NVMe"), Some(false));
        assert_eq!(smart_bus("SATA"), Some(true));
        assert_eq!(smart_bus("ATA"), Some(true));
        assert_eq!(smart_bus("USB"), None);
        assert_eq!(smart_bus("RAID"), None);
    }

    #[test]
    fn a_sata_disk_adds_temperatures_first_and_smart_last() {
        let sata = DiskSample { number: 1, model: "SATA".into(), smart: Some(DiskSmart::Ata(ata::Smart::Off)), ..Default::default() };
        let frame = Frame::build(1, &[sata.readings(Instant::now())], Vec::new());
        let groups: Vec<_> = frame.devices[0].groups.iter().map(|g| g.group.name()).collect();
        assert_eq!(groups, vec!["Temperatures", "Utilization", "Speed", "S.M.A.R.T."]);
        assert_eq!(frame.reading("disk1.smart.health").unwrap().text(), PROBE_OFF);
    }

    #[test]
    fn the_health_log_is_read_on_a_slower_rhythm() {
        let t0 = Instant::now();
        assert!(smart_due(None, t0), "first read at once");
        assert!(!smart_due(Some(t0), t0 + Duration::from_secs(1)));
        assert!(smart_due(Some(t0), t0 + SMART_PERIOD));
    }

    #[test]
    fn physical_disk_instances_name_the_disk_number() {
        assert_eq!(disk_number("0 C:"), Some(0));
        assert_eq!(disk_number("1 D: E:"), Some(1));
        assert_eq!(disk_number("2"), Some(2));
        assert_eq!(disk_number("_Total"), None);
    }

    /// Reads this PC's disks (S.M.A.R.T. when elevated) and prints them.
    #[test]
    #[ignore]
    fn reads_this_pcs_disks() {
        let mut p = DiskCounters::new();
        assert_eq!(p.availability(), Ok(()));
        let frame = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        assert!(!frame.devices.is_empty(), "at least one physical disk");
        for d in &frame.devices {
            println!("{}", d.device.name);
            for g in &d.groups {
                for r in &g.readings {
                    println!("  {:<12} {:<12} {}", g.group.name(), r.name, r.text());
                    // The health log needs administrator; the counters never do.
                    let from_health_log = matches!(g.group, Group::Smart | Group::Temperatures);
                    assert!(r.value.is_some() || from_health_log, "{} has a value", r.id);
                }
            }
        }
    }
}
