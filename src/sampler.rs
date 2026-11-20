//! Background sampler: reads processes, CPU, memory, disks and network at the
//! chosen update speed and hands snapshots to the UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use sysinfo::{
    Disks, Networks, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind,
    Users,
};

#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct ProcRow {
    pub pid: u32,
    pub parent: Option<u32>,
    pub name: String,
    pub status: String,
    pub user: String,
    /// Percent of the whole machine (0..=100).
    pub cpu: f32,
    pub mem: u64,
    pub disk_read: f64,
    pub disk_write: f64,
    pub run_time: u64,
    pub exe: String,
    pub cmd: String,
    /// The facts below come from `proc_info` (0 or empty when unknown).
    pub threads: u32,
    /// The process's GPU engine utilisation summed over its engines, in
    /// percent (0 when it has none).
    pub gpu: f32,
    pub handles: u32,
    /// The largest working set the process has had, in bytes.
    pub peak_mem: u64,
    pub page_faults: u32,
    /// Kernel plus user time spent on the processors.
    pub cpu_time: Duration,
    /// A priority class from `winsys::PRIORITIES` ("Normal", "High", ...).
    pub priority: &'static str,
    /// "x64", "x86", "ARM64" or "ARM".
    pub arch: &'static str,
    /// The process owns a visible top-level window: it is listed under
    /// Apps, not Background processes.
    pub app: bool,
    /// The Windows session the process runs in.
    pub session: u32,
    /// The parts of `cpu_time` spent in the kernel and in user mode.
    pub kernel_time: Duration,
    pub user_time: Duration,
    /// The working set and the commit size (private bytes), in bytes.
    pub working_set: u64,
    pub commit: u64,
    /// Bytes read, written and moved by other I/O since the process started.
    pub io_read: u64,
    pub io_write: u64,
    pub io_other: u64,
    /// Read and write operations since the process started.
    pub io_read_ops: u64,
    pub io_write_ops: u64,
    /// The scheduler's base priority (8 for Normal).
    pub base_priority: i32,
    /// The processor group the process runs in.
    pub group: u16,
    /// Set on a service row under its host process (none until the rows
    /// list services).
    pub service: Option<ServiceInfo>,
}

/// What the details pane shows for a service row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ServiceInfo {
    /// The short name ("Winmgmt").
    pub name: String,
    pub display_name: String,
    pub description: String,
    /// The process the service runs in ("svchost.exe (2148)").
    pub host: String,
    /// The service group it is started with.
    pub group: String,
    /// How the service is registered ("Shared process", "Own process").
    pub registration: String,
}

#[derive(Clone, Debug, Default)]
pub struct NetRow {
    pub name: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
    pub total_rx: u64,
    pub total_tx: u64,
}

#[derive(Clone, Debug, Default)]
pub struct DiskRow {
    pub name: String,
    pub mount: String,
    pub fs: String,
    pub kind: String,
    pub total: u64,
    pub free: u64,
    pub removable: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub procs: Vec<ProcRow>,
    pub cpu_total: f32,
    pub cpu_per_core: Vec<f32>,
    pub cpu_freq_mhz: u64,
    pub cpu_freqs: Vec<u64>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub mem_avail: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    pub disk_read_bps: f64,
    pub disk_write_bps: f64,
    pub net: Vec<NetRow>,
    pub disks: Vec<DiskRow>,
    pub uptime: u64,
    pub seq: u64,
    /// Extra Windows counters for the Performance page.
    pub perf: crate::perfdata::PerfLive,
    /// Working set of the "Memory Compression" process.
    pub compressed: u64,
    /// The running services by the PID of the process that hosts them.
    pub services: crate::process_view::Services,
    /// Taken on Refresh (also while updates are paused), not on the clock.
    pub forced: bool,
}

impl Snapshot {
    /// The average clock of every logical processor, in GHz (0 if unknown).
    pub fn average_clock_ghz(&self) -> f32 {
        if self.cpu_freqs.is_empty() {
            return 0.0;
        }
        self.cpu_freqs.iter().sum::<u64>() as f32 / self.cpu_freqs.len() as f32 / 1000.0
    }
}

pub struct Sampler {
    pub rx: Receiver<Snapshot>,
    /// Milliseconds between samples; 0 means paused.
    pub interval_ms: Arc<AtomicU64>,
    pub refresh_now: Arc<AtomicBool>,
}

impl Sampler {
    pub fn start(interval_ms: u64) -> Self {
        let (tx, rx) = channel();
        let interval = Arc::new(AtomicU64::new(interval_ms));
        let refresh_now = Arc::new(AtomicBool::new(true));
        let i2 = interval.clone();
        let r2 = refresh_now.clone();
        thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(tx, i2, r2))
            .expect("start sampler thread");
        Sampler { rx, interval_ms: interval, refresh_now }
    }
}

/// Give each process its GPU utilisation from the per-process sums; a
/// process with no GPU engines gets 0.
fn merge_gpu(procs: &mut [ProcRow], by_pid: &std::collections::HashMap<u32, f32>) {
    for p in procs {
        p.gpu = by_pid.get(&p.pid).copied().unwrap_or(0.0);
    }
}

/// The running services by host PID, as the Processes tree lists them:
/// each names its host ("svchost.exe (2148)"), the svchost group from the
/// host's "-k" argument, its registration and its description from
/// `descriptions` (by service name; empty when unknown).
fn services_by_host(running: &[crate::winsys::RunningService], procs: &[ProcRow], descriptions: &HashMap<String, String>) -> HashMap<u32, Vec<ServiceInfo>> {
    let hosts: HashMap<u32, &ProcRow> = procs.iter().map(|p| (p.pid, p)).collect();
    let mut out: HashMap<u32, Vec<ServiceInfo>> = HashMap::new();
    for s in running {
        let Some(host) = hosts.get(&s.pid) else { continue };
        let mut args = host.cmd.split_whitespace();
        let group = args.by_ref().find(|a| a.eq_ignore_ascii_case("-k")).and_then(|_| args.next()).unwrap_or_default();
        out.entry(s.pid).or_default().push(ServiceInfo {
            name: s.name.clone(),
            display_name: s.display.clone(),
            description: descriptions.get(&s.name).cloned().unwrap_or_default(),
            host: format!("{} ({})", host.name, host.pid),
            group: group.to_string(),
            registration: if s.shared { "Shared process" } else { "Own process" }.to_string(),
        });
    }
    out
}

fn run(tx: Sender<Snapshot>, interval: Arc<AtomicU64>, refresh_now: Arc<AtomicBool>) {
    let mut sys = System::new();
    let users = Users::new_with_refreshed_list();
    let mut disks = Disks::new_with_refreshed_list();
    let mut networks = Networks::new_with_refreshed_list();
    let kind = ProcessRefreshKind::nothing()
        .with_cpu()
        .with_memory()
        .with_disk_usage()
        .with_exe(UpdateKind::OnlyIfNotSet)
        .with_user(UpdateKind::OnlyIfNotSet)
        .with_cmd(UpdateKind::OnlyIfNotSet);
    sys.refresh_cpu_all();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
    let mut last = Instant::now();
    let mut seq = 0u64;
    let mut disk_tick = 0u32;
    let mut perf = crate::perfdata::Collector::new();
    let mut facts = crate::proc_info::Reader::default();
    let mut descriptions: HashMap<String, String> = HashMap::new();
    // Read the fixed facts (disks, GPUs, memory modules) off the UI thread.
    let _ = crate::perfdata::info();
    // A short first wait so CPU usage has two points to compare.
    thread::sleep(Duration::from_millis(250));
    loop {
        let ms = interval.load(Ordering::Relaxed);
        let forced = refresh_now.swap(false, Ordering::Relaxed);
        if ms == 0 && !forced {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        sys.refresh_cpu_all();
        sys.refresh_memory();
        sys.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
        networks.refresh(true);
        disk_tick += 1;
        if disk_tick % 10 == 1 {
            disks.refresh(true);
        }
        let now = Instant::now();
        let secs = now.duration_since(last).as_secs_f64().max(0.05);
        last = now;
        let ncpu = sys.cpus().len().max(1) as f32;

        let mut snap = Snapshot::default();
        let mut dr = 0.0;
        let mut dw = 0.0;
        for (pid, p) in sys.processes() {
            let du = p.disk_usage();
            let r = du.read_bytes as f64 / secs;
            let w = du.written_bytes as f64 / secs;
            dr += r;
            dw += w;
            let user = p
                .user_id()
                .and_then(|u| users.get_user_by_id(u))
                .map(|u| u.name().to_string())
                .unwrap_or_default();
            let cmd = p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ");
            snap.procs.push(ProcRow {
                pid: pid.as_u32(),
                parent: p.parent().map(|x| x.as_u32()),
                name: p.name().to_string_lossy().into_owned(),
                // `proc_info` reads the real status below.
                status: "Running".into(),
                user,
                cpu: (p.cpu_usage() / ncpu).clamp(0.0, 100.0),
                mem: p.memory(),
                disk_read: r,
                disk_write: w,
                run_time: p.run_time(),
                exe: p.exe().map(|e| e.display().to_string()).unwrap_or_default(),
                cmd,
                ..Default::default()
            });
        }
        facts.fill(&mut snap.procs);
        // The services under their hosts; a description is read once per
        // service.
        let running = crate::winsys::running_services();
        for s in &running {
            if !descriptions.contains_key(&s.name) {
                descriptions.insert(s.name.clone(), crate::winsys::service_description(&s.name));
            }
        }
        snap.services = Arc::new(services_by_host(&running, &snap.procs, &descriptions));
        snap.forced = forced;
        snap.cpu_total = sys.global_cpu_usage();
        snap.cpu_per_core = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
        snap.cpu_freq_mhz = sys.cpus().first().map(|c| c.frequency()).unwrap_or(0);
        snap.cpu_freqs = sys.cpus().iter().map(|c| c.frequency()).collect();
        snap.mem_total = sys.total_memory();
        snap.mem_used = sys.used_memory();
        snap.mem_avail = sys.available_memory();
        snap.swap_total = sys.total_swap();
        snap.swap_used = sys.used_swap();
        snap.disk_read_bps = dr;
        snap.disk_write_bps = dw;
        for (name, data) in networks.iter() {
            snap.net.push(NetRow {
                name: name.clone(),
                rx_bps: data.received() as f64 / secs,
                tx_bps: data.transmitted() as f64 / secs,
                total_rx: data.total_received(),
                total_tx: data.total_transmitted(),
            });
        }
        snap.net.sort_by(|a, b| {
            (b.total_rx + b.total_tx).cmp(&(a.total_rx + a.total_tx))
        });
        for d in disks.list() {
            snap.disks.push(DiskRow {
                name: d.name().to_string_lossy().into_owned(),
                mount: d.mount_point().display().to_string(),
                fs: d.file_system().to_string_lossy().into_owned(),
                kind: format!("{:?}", d.kind()),
                total: d.total_space(),
                free: d.available_space(),
                removable: d.is_removable(),
            });
        }
        snap.uptime = System::uptime();
        snap.perf = perf.sample();
        merge_gpu(&mut snap.procs, &snap.perf.gpu_by_pid);
        snap.compressed = match snap.perf.compressed {
            0 => snap
                .procs
                .iter()
                .find(|p| p.name.eq_ignore_ascii_case("Memory Compression"))
                .map(|p| p.mem)
                .unwrap_or(0),
            c => c,
        };
        if snap.swap_used == 0 && snap.perf.pagefile_pct > 0.0 {
            snap.swap_used = (snap.swap_total as f64 * snap.perf.pagefile_pct as f64 / 100.0) as u64;
        }
        seq += 1;
        snap.seq = seq;
        if tx.send(snap).is_err() {
            return;
        }
        // Sleep in small steps so a speed change or "Refresh now" acts quickly.
        let target = Duration::from_millis(if ms == 0 { 100 } else { ms });
        let start = Instant::now();
        while start.elapsed() < target {
            if refresh_now.load(Ordering::Relaxed) {
                break;
            }
            let cur = interval.load(Ordering::Relaxed);
            if cur != ms && cur != 0 && start.elapsed() >= Duration::from_millis(cur) {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_process_gets_its_gpu_and_one_without_engines_gets_zero() {
        let row = |pid, gpu| ProcRow { pid, gpu, ..Default::default() };
        let mut procs = vec![row(1234, 0.0), row(77, 9.0)];
        merge_gpu(&mut procs, &[(1234, 13.0)].into_iter().collect());
        assert_eq!(procs.iter().map(|p| (p.pid, p.gpu)).collect::<Vec<_>>(), [(1234, 13.0), (77, 0.0)]);
    }

    #[test]
    fn each_service_names_its_host_group_registration_and_description() {
        let svchost = ProcRow { pid: 2148, name: "svchost.exe".into(), cmd: r"C:\Windows\system32\svchost.exe -k netsvcs -p".into(), ..Default::default() };
        let spooler = ProcRow { pid: 3000, name: "spoolsv.exe".into(), ..Default::default() };
        let running = |pid, name: &str, display: &str, shared| crate::winsys::RunningService { pid, name: name.into(), display: display.into(), shared };
        let services = [running(2148, "Winmgmt", "Windows Management Instrumentation", true), running(3000, "Spooler", "Print Spooler", false), running(9, "Gone", "No host", false)];
        let descriptions = [("Winmgmt".to_string(), "Provides a common interface.".to_string())].into_iter().collect();
        let by_host = services_by_host(&services, &[svchost, spooler], &descriptions);
        assert_eq!(
            by_host[&2148],
            [ServiceInfo {
                name: "Winmgmt".into(),
                display_name: "Windows Management Instrumentation".into(),
                description: "Provides a common interface.".into(),
                host: "svchost.exe (2148)".into(),
                group: "netsvcs".into(),
                registration: "Shared process".into(),
            }]
        );
        assert_eq!((by_host[&3000][0].group.as_str(), by_host[&3000][0].registration.as_str()), ("", "Own process"));
        assert!(!by_host.contains_key(&9), "a service whose host is not listed is left out");
    }
}
