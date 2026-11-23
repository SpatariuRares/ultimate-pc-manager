//! Background sampler: reads processes, CPU, memory, disks and network at the
//! chosen update speed and hands snapshots to the UI.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crate::perfdata::{counter, PerfLive};
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::service::{Interest, ProviderGroup, Ticket};
use crate::sensors::SensorService;
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
    /// A priority class from `process_actions::PRIORITIES` ("Normal", "High", ...).
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
    /// The machine's fixed facts, as the providers reported them (none
    /// until one did).
    pub machine: Option<Arc<crate::perfdata::StaticInfo>>,
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

/// What fills a snapshot with everything the sensor service does not give
/// it: processes, memory, networks, disks. It runs on the sampler's thread.
pub type Fill = Box<dyn FnMut(&mut Snapshot)>;

/// The longest the sampler waits for the counter providers' frame. A
/// provider that does not answer ends the wait at its own read timeout (it
/// is "not responding"); this is only the backstop.
const FRAME_WAIT: Duration = Duration::from_secs(15);

pub struct Sampler {
    pub rx: Receiver<Snapshot>,
    /// Milliseconds between samples; 0 means paused.
    pub interval_ms: Arc<AtomicU64>,
    /// Set to take a sample at once ("Refresh", also while paused); the
    /// sampler clears it when it takes one.
    pub refresh_requested: Arc<AtomicBool>,
}

impl Sampler {
    /// Starts sampling the PC; the counters the sensor service's providers
    /// read come from `service` (the one every page shares).
    pub fn start(interval_ms: u64, service: SensorService) -> Self {
        Self::start_with(interval_ms, service, system_fill)
    }

    /// Like [`Sampler::start`], filling a snapshot with `make_fill()` (built
    /// on the sampler's thread) instead of reading the system.
    pub fn start_with(interval_ms: u64, service: SensorService, make_fill: impl FnOnce() -> Fill + Send + 'static) -> Self {
        let (tx, rx) = channel();
        let interval = Arc::new(AtomicU64::new(interval_ms));
        let refresh_requested = Arc::new(AtomicBool::new(true));
        // Taken now, so the counters start reading while the rest warms up;
        // held by the sampler's thread for as long as the app runs.
        let interest = service.interest_in(ProviderGroup::Counters);
        let (i2, r2) = (interval.clone(), refresh_requested.clone());
        thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(tx, i2, r2, interest, make_fill()))
            .expect("start sampler thread");
        Sampler { rx, interval_ms: interval, refresh_requested }
    }

    /// A sampler that never samples (tests feed `rx` themselves).
    #[cfg(test)]
    pub fn detached(rx: Receiver<Snapshot>) -> Self {
        Sampler { rx, interval_ms: Default::default(), refresh_requested: Default::default() }
    }

    /// "Refresh": takes a sample now (also while paused). Like every sample
    /// it is made from counters read after it began.
    pub fn refresh(&self) {
        self.refresh_requested.store(true, Ordering::Relaxed);
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
fn services_by_host(running: &[crate::services::RunningService], procs: &[ProcRow], descriptions: &HashMap<String, String>) -> HashMap<u32, Vec<ServiceInfo>> {
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

/// What fills `perf` from one counter that answered.
type Fill1 = fn(&mut PerfLive, &CounterSet);

/// Every counter the provider state counts ([`crate::perfdata::COUNTERS`],
/// [`crate::perfdata::OPTIONAL`] and the GetPerformanceInfo call), with what
/// it fills. A counter that gave nothing (no frame yet, a provider with a
/// problem or not responding) is in `perf.unanswered` and its figures are
/// gaps, not zeros: this table is the one place a reading becomes an answer
/// or a gap, and [`crate::app::ProviderState::of`] the one place the gaps
/// become a state. The GPU and energy counters are in no health list (a PC
/// without a GPU driver or an energy meter never answers them), so they are
/// filled apart.
const FILLS: &[(&str, Fill1)] = &[
    (counter::PERF, performance_level),
    (counter::IDLE, disk_activity),
    (counter::DREAD, disk_reads),
    (counter::DWRITE, disk_writes),
    (counter::DRESP, disk_response),
    (counter::KERNEL, kernel_time),
    (counter::PAGEIN, pages_read),
    (counter::COMPR, compressed_memory),
    (counter::PAGEFILE, page_file),
    (crate::perfdata::PERF_INFO, performance_info),
];

/// The counter `key` of `frame`, when it answered.
fn answered<'a>(frame: Option<&'a crate::sensors::Frame>, key: &str) -> Option<&'a CounterSet> {
    frame.and_then(|f| f.counter(key)).filter(|c| c.answered())
}

/// Puts what the sensor service's providers counted into `perf`: the CPU
/// performance level, every disk's activity and rates (with the totals since
/// boot), the memory counters, the GPU adapters with each process's GPU
/// share and the package power.
fn counters_into(perf: &mut PerfLive, frame: Option<&crate::sensors::Frame>) {
    for &(key, fill) in FILLS {
        match answered(frame, key) {
            Some(counted) => fill(perf, counted),
            None => {
                perf.unanswered.insert(key);
            }
        }
    }
    gpus_into(perf, frame);
    power_into(perf, answered(frame, counter::POWER));
}

fn performance_level(perf: &mut PerfLive, counted: &CounterSet) {
    perf.cpu_perf_pct = counted.instance("_Total").map_or(0.0, |i| i.value as f32);
}

/// The disk numbered `number`, added in order when `perf` has none yet.
fn disk_of(perf: &mut PerfLive, number: u32) -> &mut crate::perfdata::DiskLive {
    let at = perf.disks.binary_search_by_key(&number, |d| d.number).unwrap_or_else(|at| {
        perf.disks.insert(at, crate::perfdata::DiskLive { number, ..Default::default() });
        at
    });
    &mut perf.disks[at]
}

/// Gives every disk instance ("0 C:", not "_Total") of `counted` to `fill`.
fn for_each_disk(perf: &mut PerfLive, counted: &CounterSet, fill: impl Fn(&mut crate::perfdata::DiskLive, &Instance)) {
    for instance in &counted.instances {
        if let Some(number) = crate::sensors::disk::disk_number(&instance.name) {
            fill(disk_of(perf, number), instance);
        }
    }
}

/// The bytes since boot an instance counted.
fn since_boot(instance: &Instance) -> u64 {
    instance.raw.map_or(0, |r| r.max(0) as u64)
}

fn disk_activity(perf: &mut PerfLive, counted: &CounterSet) {
    for_each_disk(perf, counted, |disk, i| disk.active = (100.0 - i.value as f32).clamp(0.0, 100.0));
}

fn disk_reads(perf: &mut PerfLive, counted: &CounterSet) {
    for_each_disk(perf, counted, |disk, i| (disk.read_bps, disk.total_read) = (i.value.max(0.0), since_boot(i)));
}

fn disk_writes(perf: &mut PerfLive, counted: &CounterSet) {
    for_each_disk(perf, counted, |disk, i| (disk.write_bps, disk.total_written) = (i.value.max(0.0), since_boot(i)));
}

/// Seconds per transfer, shown in milliseconds.
fn disk_response(perf: &mut PerfLive, counted: &CounterSet) {
    for_each_disk(perf, counted, |disk, i| disk.resp_ms = (i.value * 1000.0) as f32);
}

/// Kernel time per processor (in processor order) and in total.
fn kernel_time(perf: &mut PerfLive, counted: &CounterSet) {
    let mut cores: Vec<(usize, f32)> = Vec::new();
    for i in &counted.instances {
        if i.name == "_Total" {
            perf.kernel_total = (i.value as f32).clamp(0.0, 100.0);
        } else if let Ok(n) = i.name.parse::<usize>() {
            cores.push((n, (i.value as f32).clamp(0.0, 100.0)));
        }
    }
    cores.sort_by_key(|c| c.0);
    perf.kernel_per_core = cores.into_iter().map(|c| c.1).collect();
}

/// The pages read from disk, per second and since boot.
fn pages_read(perf: &mut PerfLive, counted: &CounterSet) {
    let Some(first) = counted.instances.first() else { return };
    perf.hard_faults = first.value as f32;
    perf.page_ins = since_boot(first);
}

/// The memory the compression process holds.
fn compressed_memory(perf: &mut PerfLive, counted: &CounterSet) {
    perf.compressed = counted.instances.first().map_or(0, |i| i.value.max(0.0) as u64);
}

/// The page file in use, in percent.
fn page_file(perf: &mut PerfLive, counted: &CounterSet) {
    perf.pagefile_pct = counted.instances.first().map_or(0.0, |i| i.value as f32);
}

/// GetPerformanceInfo: threads, handles, commit, its limit and the cache.
fn performance_info(perf: &mut PerfLive, counted: &CounterSet) {
    let named = |name: &str| counted.instance(name).map_or(0.0, |i| i.value.max(0.0));
    (perf.threads, perf.handles) = (named("threads") as u32, named("handles") as u32);
    (perf.commit, perf.commit_limit, perf.cached) = (named("commit") as u64, named("commit_limit") as u64, named("cached") as u64);
}

/// The GPU adapters and each process's share. No GPU counters (no driver) is
/// no adapter, not a gap.
fn gpus_into(perf: &mut PerfLive, frame: Option<&crate::sensors::Frame>) {
    let instances = |key: &str| answered(frame, key).map(|c| c.instances.iter().map(|i| (i.name.clone(), i.value)).collect::<Vec<_>>()).unwrap_or_default();
    let engines = instances(counter::GPU);
    perf.gpu_by_pid = crate::perfdata::gpu_by_pid(&engines);
    perf.gpus = crate::perfdata::gpu_adapters(&engines, &instances(counter::GDED), &instances(counter::GSH));
}

/// Windows' energy meter reports the RAPL package rail in milliwatts.
fn power_into(perf: &mut PerfLive, counted: Option<&CounterSet>) {
    perf.cpu_power_w = counted
        .and_then(|c| {
            c.instances.iter().find(|i| {
                let name = i.name.to_ascii_lowercase();
                name.contains("package0") && name.ends_with("_pkg")
            })
        })
        .map(|i| i.value as f32 / 1000.0);
}

/// Fills a snapshot from the system (see [`Fill`]); built on the sampler's
/// thread, where it warms up.
fn system_fill() -> Fill {
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
    let mut disk_tick = 0u32;
    let mut facts = crate::proc_info::Reader::default();
    let mut descriptions: HashMap<String, String> = HashMap::new();
    // A short first wait so CPU usage has two points to compare.
    thread::sleep(Duration::from_millis(250));
    Box::new(move |snap: &mut Snapshot| {
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
            let running = crate::services::running_services();
            for s in &running {
                if !descriptions.contains_key(&s.name) {
                    descriptions.insert(s.name.clone(), crate::services::service_description(&s.name));
                }
            }
            snap.services = Arc::new(services_by_host(&running, &snap.procs, &descriptions));
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
    })
}

/// The memory figures that need the counters and the system's own reading
/// together: the compressed memory is the compression process's working set
/// when the counter did not give it, and the page file in use is its
/// percentage of the total when the system reported none.
fn memory_from_counters(snap: &mut Snapshot) {
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
}

/// The frame the sampler builds a snapshot from: of the providers' reads
/// after `ticket` when it has one, else the latest; waits for the providers'
/// answers (a provider that stops answering ends the wait at its read
/// timeout), never for the hardware ones.
fn counted_frame(interest: &Interest, ticket: Option<&Ticket>) -> Option<Arc<crate::sensors::Frame>> {
    let start = Instant::now();
    loop {
        let frame = match ticket {
            Some(ticket) => interest.latest_after(ticket),
            None => interest.latest(),
        };
        if frame.is_some() || start.elapsed() >= FRAME_WAIT {
            return frame;
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The sampler's thread: one snapshot per tick, at the app's update speed
/// (and on Refresh, also while paused), until the app drops the receiver.
/// `counters` is its interest in the counter providers, held for as long as
/// it runs.
fn run(tx: Sender<Snapshot>, interval: Arc<AtomicU64>, refresh_requested: Arc<AtomicBool>, counters: Interest, mut fill: Fill) {
    let mut seq = 0u64;
    let mut machine = None;
    loop {
        let ms = interval.load(Ordering::Relaxed);
        let forced = refresh_requested.swap(false, Ordering::Relaxed);
        if ms == 0 && !forced {
            thread::sleep(Duration::from_millis(100));
            continue;
        }
        // The counters are read once per tick, whatever any add-on asks of
        // the sensors, and the snapshot is built from a read that began
        // after the tick did: ask now, so they read while `fill` works. The
        // first tick takes the read the providers began with at start.
        let ticket = (seq > 0).then(|| counters.read_now());
        let mut snap = Snapshot::default();
        fill(&mut snap);
        snap.forced = forced;
        let frame = counted_frame(&counters, ticket.as_ref());
        counters_into(&mut snap.perf, frame.as_deref());
        merge_gpu(&mut snap.procs, &snap.perf.gpu_by_pid);
        memory_from_counters(&mut snap);
        // The facts do not change: the last ones stand while a frame has none.
        machine = frame.and_then(|f| f.machine.clone()).or(machine);
        snap.machine = machine.clone();
        seq += 1;
        snap.seq = seq;
        if tx.send(snap).is_err() {
            return;
        }
        // Sleep in small steps so a speed change or "Refresh now" acts quickly.
        let target = Duration::from_millis(if ms == 0 { 100 } else { ms });
        let start = Instant::now();
        while start.elapsed() < target {
            if refresh_requested.load(Ordering::Relaxed) {
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
mod through_the_app;

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
        let running = |pid, name: &str, display: &str, shared| crate::services::RunningService { pid, name: name.into(), display: display.into(), shared };
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

    #[test]
    fn every_source_the_provider_state_counts_has_a_way_into_the_snapshot() {
        let filled: Vec<&str> = FILLS.iter().map(|f| f.0).collect();
        for key in crate::perfdata::COUNTERS.iter().chain(crate::perfdata::OPTIONAL).copied().chain([crate::perfdata::PERF_INFO]) {
            assert!(filled.contains(&key), "{key} is in FILLS");
        }
        assert_eq!(filled.len(), crate::perfdata::COUNTERS.len() + crate::perfdata::OPTIONAL.len() + 1, "and nothing else");
    }

    fn frame_of(sets: Vec<CounterSet>) -> crate::sensors::Frame {
        let mut frame = crate::sensors::Frame::build(1, &[], Vec::new());
        frame.counters = sets;
        frame
    }

    /// What a PC with two disks and an energy meter counts.
    fn counted() -> Vec<CounterSet> {
        vec![
            CounterSet::new(counter::PERF, vec![Instance::new("_Total", 108.5), Instance::new("0,_Total", 108.5), Instance::new("0,0", 90.0)]),
            CounterSet::new(counter::IDLE, vec![Instance::new("0 C:", 75.0), Instance::new("1 D:", 100.0), Instance::new("_Total", 80.0)]),
            CounterSet::new(counter::DREAD, vec![Instance::new("0 C:", 2_000.0).with_raw(9_000_000), Instance::new("1 D:", 0.0).with_raw(5), Instance::new("_Total", 2_000.0).with_raw(9_000_005)]),
            CounterSet::new(counter::DWRITE, vec![Instance::new("0 C:", 500.0).with_raw(7_000_000), Instance::new("1 D:", 0.0).with_raw(0), Instance::new("_Total", 500.0).with_raw(7_000_000)]),
            CounterSet::new(counter::POWER, vec![Instance::new("rapl_package0_core0_core", 3_000.0), Instance::new("rapl_package0_pkg", 47_500.0)]),
            CounterSet::new(counter::DRESP, vec![Instance::new("0 C:", 0.0035), Instance::new("_Total", 0.0035)]),
            CounterSet::new(counter::KERNEL, vec![Instance::new("1", 4.0), Instance::new("0", 8.0), Instance::new("_Total", 6.0)]),
            CounterSet::new(counter::PAGEIN, vec![Instance::new("", 120.0).with_raw(9_000)]),
            CounterSet::new(counter::COMPR, vec![Instance::new("memory compression", 4096.0)]),
            CounterSet::new(counter::PAGEFILE, vec![Instance::new("_total", 40.0)]),
            CounterSet::new(crate::perfdata::PERF_INFO, vec![Instance::new("threads", 2_000.0), Instance::new("handles", 90_000.0), Instance::new("commit", 8e9), Instance::new("commit_limit", 3e10), Instance::new("cached", 4e9)]),
        ]
    }

    #[test]
    fn the_performance_level_the_disks_and_the_package_power_come_from_the_frame() {
        let mut perf = PerfLive::default();
        counters_into(&mut perf, Some(&frame_of(counted())));
        assert_eq!(perf.cpu_perf_pct, 108.5);
        let disks: Vec<(u32, f32, f64, f64, u64, u64)> = perf.disks.iter().map(|d| (d.number, d.active, d.read_bps, d.write_bps, d.total_read, d.total_written)).collect();
        assert_eq!(disks, [(0, 25.0, 2_000.0, 500.0, 9_000_000, 7_000_000), (1, 0.0, 0.0, 0.0, 5, 0)], "_Total is not a disk");
        assert_eq!(perf.cpu_power_w, Some(47.5));
        assert!(perf.unanswered.is_empty(), "every counter answered: {:?}", perf.unanswered);
    }

    #[test]
    fn the_system_counters_come_from_the_frame_with_the_kernel_cores_in_order() {
        let mut perf = PerfLive::default();
        counters_into(&mut perf, Some(&frame_of(counted())));
        assert_eq!(perf.disks[0].resp_ms, 3.5, "seconds per transfer, shown in ms");
        assert_eq!((perf.kernel_total, perf.kernel_per_core.clone()), (6.0, vec![8.0, 4.0]));
        assert_eq!((perf.hard_faults, perf.page_ins), (120.0, 9_000));
        assert_eq!((perf.compressed, perf.pagefile_pct), (4096, 40.0));
        assert_eq!((perf.threads, perf.handles, perf.commit, perf.commit_limit, perf.cached), (2_000, 90_000, 8_000_000_000, 30_000_000_000, 4_000_000_000));
    }

    #[test]
    fn a_counter_nobody_answered_is_a_gap_and_the_others_are_not() {
        let mut perf = PerfLive::default();
        let only_idle = counted().into_iter().filter(|c| c.key == counter::IDLE).collect();
        counters_into(&mut perf, Some(&frame_of(only_idle)));
        let gaps: Vec<&str> = perf.unanswered.iter().copied().collect();
        assert_eq!(gaps, [counter::COMPR, counter::DREAD, counter::DRESP, counter::DWRITE, counter::KERNEL, counter::PAGEFILE, counter::PAGEIN, counter::PERF, crate::perfdata::PERF_INFO]);
        assert!(perf.answered(counter::IDLE));
        assert_eq!(perf.cpu_power_w, None, "no energy meter: no figure");
    }

    #[test]
    fn without_a_frame_no_provided_counter_answered() {
        let mut perf = PerfLive::default();
        counters_into(&mut perf, None);
        let gaps: Vec<&str> = perf.unanswered.iter().copied().collect();
        assert_eq!(gaps, [counter::COMPR, counter::DREAD, counter::DRESP, counter::DWRITE, counter::IDLE, counter::KERNEL, counter::PAGEFILE, counter::PAGEIN, counter::PERF, crate::perfdata::PERF_INFO]);
        assert!(perf.disks.is_empty());
    }

    #[test]
    fn a_counter_that_gave_no_instance_has_not_answered() {
        let mut perf = PerfLive::default();
        let mut sets = counted();
        sets[2] = CounterSet::new(counter::DREAD, Vec::new());
        counters_into(&mut perf, Some(&frame_of(sets)));
        assert!(!perf.answered(counter::DREAD) && perf.answered(counter::DWRITE));
    }
}
