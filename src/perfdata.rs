//! Extra Windows data for the Performance page:
//! PDH counters (kernel time, CPU frequency, physical disks, GPU engines,
//! CPU package power), GetPerformanceInfo, processor topology, SMBIOS memory
//! details, physical disks and GPU adapters.


/// One physical disk, read every sample.
#[derive(Clone, Debug, Default)]
pub struct DiskLive {
    pub number: u32,
    /// Percent of time the disk was busy (100 - % Idle Time).
    pub active: f32,
    pub read_bps: f64,
    pub write_bps: f64,
    pub resp_ms: f32,
    /// Bytes since boot.
    pub total_read: u64,
    pub total_written: u64,
}

/// One GPU adapter, read every sample.
#[derive(Clone, Debug, Default)]
pub struct GpuLive {
    pub luid: u64,
    /// Busiest engine, as Windows Task Manager reports it.
    pub util: f32,
    /// Engine type name and its utilisation, in the adapter's engine order.
    pub engines: Vec<(String, f32)>,
    /// Every engine and its utilisation, in engine index order, named as
    /// [`engine_cell_names`] names them (one cell each on Flight Recorder).
    pub cells: Vec<(String, f32)>,
    pub dedicated: u64,
    pub shared: u64,
}

/// One name per engine from the engines' type names, in order: a type
/// that repeats is numbered from 1 ("Copy 1", "Copy 2"), a single one is not.
pub fn engine_cell_names(types: &[String]) -> Vec<String> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    types
        .iter()
        .map(|t| {
            if types.iter().filter(|o| *o == t).count() < 2 {
                return t.clone();
            }
            let n = seen.entry(t.as_str()).or_insert(0);
            *n += 1;
            format!("{t} {n}")
        })
        .collect()
}

/// Each process's GPU utilisation from the "GPU Engine" counter instances
/// ("pid_1234_luid_..._eng_3_engtype_Copy"): the sum of its engines, in
/// percent, at most 100. Instances that name no process are skipped.
pub fn gpu_by_pid(instances: &[(String, f64)]) -> std::collections::HashMap<u32, f32> {
    let mut sums: std::collections::HashMap<u32, f64> = std::collections::HashMap::new();
    for (name, v) in instances {
        let Some(rest) = name.strip_prefix("pid_") else { continue };
        let Some(pid) = rest.split('_').next().and_then(|p| p.parse().ok()) else { continue };
        *sums.entry(pid).or_insert(0.0) += v.max(0.0);
    }
    sums.into_iter().map(|(pid, v)| (pid, (v as f32).min(100.0))).collect()
}

#[derive(Clone, Debug, Default)]
pub struct PerfLive {
    pub kernel_per_core: Vec<f32>,
    pub kernel_total: f32,
    /// "% Processor Performance": current speed as a percent of base speed.
    pub cpu_perf_pct: f32,
    pub threads: u32,
    pub handles: u32,
    pub commit: u64,
    pub commit_limit: u64,
    pub cached: u64,
    /// Pages read from disk per second.
    pub hard_faults: f32,
    /// Pages read from disk since boot.
    pub page_ins: u64,
    pub disks: Vec<DiskLive>,
    pub gpus: Vec<GpuLive>,
    /// Each process's GPU engine utilisation, summed over its engines (see
    /// [`gpu_by_pid`]); processes without engines are absent.
    pub gpu_by_pid: std::collections::HashMap<u32, f32>,
    pub cpu_power_w: Option<f32>,
    /// Working set of the "Memory Compression" process (it is protected, so
    /// it is read through the performance counters).
    pub compressed: u64,
    /// Percent of the page file in use.
    pub pagefile_pct: f32,
    /// The counters (keys of [`COUNTERS`]) that gave no value this sample:
    /// their figures are gaps, not 0. Empty when every counter answered.
    pub unanswered: std::collections::BTreeSet<&'static str>,
}

/// The performance counters' keys: in `PerfLive::unanswered`, [`COUNTERS`]
/// and the collector's query.
pub mod counter {
    /// Kernel (privileged) time per processor and in total.
    pub const KERNEL: &str = "kernel";
    /// The live "% Processor Performance" (the CPU speed rule).
    pub const PERF: &str = "perf";
    /// Pages read from disk per second (hard faults).
    pub const PAGEIN: &str = "pagein";
    /// Each physical disk's idle time (its active time).
    pub const IDLE: &str = "idle";
    /// Each physical disk's read bytes per second.
    pub const DREAD: &str = "dread";
    /// Each physical disk's write bytes per second.
    pub const DWRITE: &str = "dwrite";
    /// Each physical disk's average response time.
    pub const DRESP: &str = "dresp";
    /// Every GPU engine's utilization.
    pub const GPU: &str = "gpu";
    /// Each GPU's dedicated memory in use.
    pub const GDED: &str = "gded";
    /// Each GPU's shared memory in use.
    pub const GSH: &str = "gsh";
    /// The Windows energy meter's rails.
    pub const POWER: &str = "power";
    /// The Memory Compression process's working set.
    pub const COMPR: &str = "compr";
    /// The page file in use, in percent.
    pub const PAGEFILE: &str = "pagefile";
}

/// The performance counters every Windows PC answers, by key: the provider
/// state is healthy when all of them answered, degraded when some did not
/// and unavailable when none did. The GPU and energy-meter counters are left
/// out, since a PC without a GPU driver or a power meter never answers them.
pub const COUNTERS: &[&str] = &[counter::KERNEL, counter::PAGEIN, counter::IDLE, counter::DREAD, counter::DWRITE, counter::DRESP];

/// Counters a healthy PC may never answer (VMs, no page file): gaps, but not counted in the provider state.
pub const OPTIONAL: &[&str] = &[counter::PERF, counter::COMPR, counter::PAGEFILE];

/// The counters the sensor service's providers read for the Sampler: they
/// are in no query of the [`Collector`], and the Sampler adds the ones that
/// did not answer to [`PerfLive::unanswered`] when it merges what the
/// providers counted (see `sampler::counters_into`).
pub const PROVIDED: &[&str] = &[counter::PERF, counter::IDLE, counter::DREAD, counter::DWRITE, counter::POWER];

/// [`COUNTERS`] and [`OPTIONAL`] the [`Collector`] itself reads.
fn collected() -> impl Iterator<Item = &'static str> {
    COUNTERS.iter().chain(OPTIONAL).copied().filter(|k| !PROVIDED.contains(k))
}

/// Not a counter but the GetPerformanceInfo call (threads, handles, commit,
/// cached): in `unanswered` when the call failed, so its figures are gaps.
/// The provider state counts it with [`COUNTERS`] (see [`SOURCES`]).
pub const PERF_INFO: &str = "perfinfo";

/// Every source the provider state counts: the [`COUNTERS`] and the
/// GetPerformanceInfo call ([`PERF_INFO`]). Healthy only when all answered.
pub fn sources() -> impl Iterator<Item = &'static str> {
    COUNTERS.iter().copied().chain([PERF_INFO])
}

impl PerfLive {
    /// Whether `counter` (a key of [`COUNTERS`]) gave a value this sample.
    pub fn answered(&self, counter: &str) -> bool {
        !self.unanswered.contains(counter)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CpuInfo {
    pub sockets: u32,
    pub cores: u32,
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    pub base_mhz: u32,
    pub virtualization: Option<bool>,
    pub vm: bool,
    pub fast_startup: Option<bool>,
    pub plan: String,
    pub overlay: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MemInfo {
    pub speed_mts: Option<u32>,
    pub kind: Option<&'static str>,
    pub form: Option<&'static str>,
    pub used_slots: u32,
    pub slots: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DiskInfo {
    pub number: u32,
    pub model: String,
    pub bus: &'static str,
    pub capacity: u64,
    pub letters: Vec<char>,
    pub formatted: u64,
    pub system: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuInfo {
    pub luid: u64,
    pub name: String,
    pub dedicated: u64,
    pub shared: u64,
    pub driver: String,
    pub location: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StaticInfo {
    pub cpu: CpuInfo,
    pub mem: MemInfo,
    pub disks: Vec<DiskInfo>,
    pub gpus: Vec<GpuInfo>,
}

#[cfg(windows)]
pub use imp::Collector;

#[cfg(not(windows))]
pub struct Collector;
#[cfg(not(windows))]
impl Collector {
    pub fn new() -> Self {
        Collector
    }
    /// No performance counters off Windows: none answered.
    pub fn sample(&mut self) -> PerfLive {
        PerfLive { unanswered: collected().chain([PERF_INFO]).collect(), ..Default::default() }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::BTreeMap;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::System::Performance::*;
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
    const PDH_FMT_NOCAP100: u32 = 0x8000;

    use crate::sensors::disk::disk_number;
    use crate::winsys::wide;

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    // ------------------------------------------------------------ live data

    const PATHS: &[(&str, &str)] = &[
        (counter::KERNEL, r"\Processor(*)\% Privileged Time"),
        (counter::PAGEIN, r"\Memory\Pages Input/sec"),
        (counter::DRESP, r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"),
        (counter::GPU, r"\GPU Engine(*)\Utilization Percentage"),
        (counter::GDED, r"\GPU Adapter Memory(*)\Dedicated Usage"),
        (counter::GSH, r"\GPU Adapter Memory(*)\Shared Usage"),
        (counter::COMPR, r"\Process(Memory Compression)\Working Set"),
        (counter::PAGEFILE, r"\Paging File(_Total)\% Usage"),
    ];

    /// A PDH query holding every counter the page needs.
    pub struct Collector {
        q: PDH_HQUERY,
        c: BTreeMap<&'static str, PDH_HCOUNTER>,
        /// The counters that gave at least one value since `sample` began.
        answered: std::cell::RefCell<std::collections::BTreeSet<&'static str>>,
    }

    impl Collector {
        pub fn new() -> Self {
            let mut q: PDH_HQUERY = null_mut();
            let mut c = BTreeMap::new();
            unsafe {
                if PdhOpenQueryW(null(), 0, &mut q) == 0 {
                    for (k, p) in PATHS {
                        let mut h: PDH_HCOUNTER = null_mut();
                        let w = wide(p);
                        if PdhAddEnglishCounterW(q, w.as_ptr(), 0, &mut h) == 0 {
                            c.insert(*k, h);
                        }
                    }
                    PdhCollectQueryData(q);
                } else {
                    q = null_mut();
                }
            }
            Collector { q, c, answered: Default::default() }
        }

        /// The counter's values; a counter that gives any is noted as
        /// answered for this sample.
        fn fmt(&self, k: &str) -> Vec<(String, f64)> {
            let Some((&key, &h)) = self.c.get_key_value(k) else { return Vec::new() };
            let values = self.fmt_values(h);
            if !values.is_empty() {
                self.answered.borrow_mut().insert(key);
            }
            values
        }

        /// The counter's current values by instance name, leaving out the
        /// instances without valid data (empty when the counter gave none).
        fn fmt_values(&self, h: PDH_HCOUNTER) -> Vec<(String, f64)> {
            unsafe {
                let flags = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
                let (mut size, mut count) = (0u32, 0u32);
                if PdhGetFormattedCounterArrayW(h, flags, &mut size, &mut count, null_mut()) != PDH_MORE_DATA || size == 0 {
                    return Vec::new();
                }
                let mut buf = vec![0u64; size as usize / 8 + 1];
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                if PdhGetFormattedCounterArrayW(h, flags, &mut size, &mut count, items) != 0 {
                    return Vec::new();
                }
                (0..count as usize)
                    .filter_map(|i| {
                        let it = &*items.add(i);
                        // 0 = valid data, 1 = new data.
                        (it.FmtValue.CStatus <= 1).then(|| (from_wide(it.szName), it.FmtValue.Anonymous.doubleValue))
                    })
                    .collect()
            }
        }

        /// Raw values: for "per second" counters the first value is the
        /// running total since boot.
        fn raw(&self, k: &str) -> Vec<(String, i64)> {
            let Some(&h) = self.c.get(k) else { return Vec::new() };
            unsafe {
                let (mut size, mut count) = (0u32, 0u32);
                if PdhGetRawCounterArrayW(h, &mut size, &mut count, null_mut()) != PDH_MORE_DATA || size == 0 {
                    return Vec::new();
                }
                let mut buf = vec![0u64; size as usize / 8 + 1];
                let items = buf.as_mut_ptr() as *mut PDH_RAW_COUNTER_ITEM_W;
                if PdhGetRawCounterArrayW(h, &mut size, &mut count, items) != 0 {
                    return Vec::new();
                }
                (0..count as usize)
                    .filter_map(|i| {
                        let it = &*items.add(i);
                        (it.RawValue.CStatus <= 1).then(|| (from_wide(it.szName), it.RawValue.FirstValue))
                    })
                    .collect()
            }
        }

        pub fn sample(&mut self) -> PerfLive {
            // Until a counter gives a value, it has not answered.
            let mut l = PerfLive { unanswered: collected().collect(), ..Default::default() };
            self.answered.borrow_mut().clear();
            if self.q.is_null() {
                l.unanswered.insert(PERF_INFO);
                return l;
            }
            unsafe {
                PdhCollectQueryData(self.q);
            }

            let mut cores: Vec<(usize, f32)> = Vec::new();
            for (n, v) in self.fmt(counter::KERNEL) {
                if n == "_Total" {
                    l.kernel_total = (v as f32).clamp(0.0, 100.0);
                } else if let Ok(i) = n.parse::<usize>() {
                    cores.push((i, (v as f32).clamp(0.0, 100.0)));
                }
            }
            cores.sort_by_key(|c| c.0);
            l.kernel_per_core = cores.into_iter().map(|c| c.1).collect();
            l.hard_faults = self.fmt(counter::PAGEIN).first().map(|x| x.1 as f32).unwrap_or(0.0);
            l.page_ins = self.raw(counter::PAGEIN).first().map(|x| x.1.max(0) as u64).unwrap_or(0);

            // Physical disks: instances look like "0 C: D:". The sensor
            // service's providers give the rest of a disk (see
            // `sampler::counters_into`).
            let mut disks: BTreeMap<u32, DiskLive> = BTreeMap::new();
            for (n, v) in self.fmt(counter::DRESP) {
                if let Some(d) = disk_number(&n) {
                    let e = disks.entry(d).or_default();
                    e.number = d;
                    e.resp_ms = (v * 1000.0) as f32;
                }
            }
            l.disks = disks.into_values().collect();

            // GPU engines: "pid_1234_luid_0x00000000_0x0001670C_phys_0_eng_3_engtype_Copy".
            // Sum over processes per engine, then keep the busiest engine of each type.
            let mut eng: BTreeMap<(u64, u32), (String, f64)> = BTreeMap::new();
            let engines = self.fmt(counter::GPU);
            l.gpu_by_pid = super::gpu_by_pid(&engines);
            for (n, v) in engines {
                let Some((luid, rest)) = parse_luid(&n) else { continue };
                let Some(i) = rest.find("eng_") else { continue };
                let Some((idx, ty)) = rest[i + 4..].split_once("_engtype_") else { continue };
                let e = eng.entry((luid, idx.parse().unwrap_or(0))).or_insert_with(|| (pretty_engine(ty), 0.0));
                e.1 += v;
            }
            let mut gpus: BTreeMap<u64, GpuLive> = BTreeMap::new();
            for ((luid, _), (name, v)) in eng {
                let g = gpus.entry(luid).or_insert_with(|| GpuLive { luid, ..Default::default() });
                let v = (v as f32).clamp(0.0, 100.0);
                g.cells.push((name.clone(), v));
                match g.engines.iter_mut().find(|e| e.0 == name) {
                    Some(e) => e.1 = e.1.max(v),
                    None => g.engines.push((name, v)),
                }
                g.util = g.util.max(v);
            }
            // Every engine its own cell: repeated types numbered.
            for g in gpus.values_mut() {
                let types: Vec<String> = g.cells.iter().map(|c| c.0.clone()).collect();
                for (cell, name) in g.cells.iter_mut().zip(super::engine_cell_names(&types)) {
                    cell.0 = name;
                }
            }
            for (key, f) in [(counter::GDED, 0), (counter::GSH, 1)] {
                for (n, v) in self.fmt(key) {
                    let Some((luid, _)) = parse_luid(&n) else { continue };
                    let g = gpus.entry(luid).or_insert_with(|| GpuLive { luid, ..Default::default() });
                    if f == 0 { g.dedicated = v.max(0.0) as u64 } else { g.shared = v.max(0.0) as u64 }
                }
            }
            l.gpus = gpus.into_values().collect();

            l.compressed = self.fmt(counter::COMPR).first().map(|x| x.1.max(0.0) as u64).unwrap_or(0);
            l.pagefile_pct = self.fmt(counter::PAGEFILE).first().map(|x| x.1 as f32).unwrap_or(0.0);

            unsafe {
                let mut pi: PERFORMANCE_INFORMATION = zeroed();
                pi.cb = size_of::<PERFORMANCE_INFORMATION>() as u32;
                if GetPerformanceInfo(&mut pi, pi.cb) != 0 {
                    let page = pi.PageSize as u64;
                    l.threads = pi.ThreadCount;
                    l.handles = pi.HandleCount;
                    l.commit = pi.CommitTotal as u64 * page;
                    l.commit_limit = pi.CommitLimit as u64 * page;
                    l.cached = pi.SystemCache as u64 * page;
                } else {
                    l.unanswered.insert(PERF_INFO);
                }
            }
            l.unanswered.retain(|k| !self.answered.borrow().contains(k));
            l
        }
    }

    impl Drop for Collector {
        fn drop(&mut self) {
            if !self.q.is_null() {
                unsafe {
                    PdhCloseQuery(self.q);
                }
            }
        }
    }

    /// "…luid_0x00000000_0x0001670C_phys_0…" → (luid, text after the luid).
    fn parse_luid(n: &str) -> Option<(u64, &str)> {
        let i = n.find("luid_0x")?;
        let rest = &n[i + 5..];
        let mut it = rest.splitn(3, '_');
        let hi = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
        let lo = u64::from_str_radix(it.next()?.trim_start_matches("0x"), 16).ok()?;
        Some(((hi << 32) | lo, it.next().unwrap_or("")))
    }

    /// "VideoDecode" → "Video decode", "High Priority 3D" → "High priority 3d",
    /// "3D" → "3D", "Compute_0" → "Compute 0" (as the page shows them).
    fn pretty_engine(ty: &str) -> String {
        let mut s = String::new();
        let mut prev_lower = false;
        for ch in ty.chars() {
            if ch.is_uppercase() && prev_lower {
                s.push(' ');
            }
            prev_lower = ch.is_lowercase();
            s.push(if ch == '_' { ' ' } else { ch });
        }
        let s = s.to_lowercase();
        if s == "3d" {
            return "3D".into();
        }
        let mut c = s.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engines_of_a_repeated_type_are_numbered_and_single_ones_are_not() {
        let types: Vec<String> = ["3D", "Copy", "Video decode", "Copy", "Video encode", "Video encode", "Ofa 0"].map(String::from).to_vec();
        assert_eq!(engine_cell_names(&types), ["3D", "Copy 1", "Video decode", "Copy 2", "Video encode 1", "Video encode 2", "Ofa 0"]);
        assert!(engine_cell_names(&[]).is_empty());
    }

    #[test]
    fn a_processs_gpu_is_the_sum_of_its_engines() {
        let i = |n: &str, v: f64| (n.to_string(), v);
        let by_pid = gpu_by_pid(&[
            i("pid_1234_luid_0x00000000_0x0001670C_phys_0_eng_0_engtype_3D", 10.0),
            i("pid_1234_luid_0x00000000_0x0001670C_phys_0_eng_3_engtype_Copy", 2.5),
            i("pid_1234_luid_0x00000000_0x0002AAAA_phys_0_eng_0_engtype_VideoDecode", 0.5),
            i("pid_88_luid_0x00000000_0x0001670C_phys_0_eng_0_engtype_3D", 70.0),
            i("pid_88_luid_0x00000000_0x0001670C_phys_0_eng_1_engtype_Compute_0", 60.0),
            i("luid_0x00000000_0x0001670C_phys_0_eng_0_engtype_3D", 5.0),
        ]);
        assert_eq!(by_pid.get(&1234), Some(&13.0), "3D + Copy on one adapter + Video decode on another");
        assert_eq!(by_pid.get(&88), Some(&100.0), "the sum stops at 100");
        assert_eq!(by_pid.len(), 2, "an instance without a pid names no process: {by_pid:?}");
    }
}
