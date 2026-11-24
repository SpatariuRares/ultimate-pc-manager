//! The types the performance data is carried in: the live figures of a
//! sample ([`PerfLive`], its disks and GPUs), the keys of the counters the
//! sensor providers count ([`counter`]), the machine's fixed facts
//! ([`StaticInfo`]) and the helpers that turn GPU engine instances into
//! adapters and per-process shares. The providers under `sensors/` read
//! them; the Sampler puts them together.


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

/// The GPU adapters from what the "GPU Engine" and "GPU Adapter Memory"
/// counters list. Engine instances
/// ("pid_1234_luid_0x00000000_0x0001670C_phys_0_eng_3_engtype_Copy") are
/// summed over processes per engine, then the busiest engine of each type is
/// kept; every engine is also its own cell, repeated types numbered (see
/// [`engine_cell_names`]). Memory instances name only the adapter. Adapters
/// come in luid order.
pub fn gpu_adapters(engines: &[(String, f64)], dedicated: &[(String, f64)], shared: &[(String, f64)]) -> Vec<GpuLive> {
    use std::collections::BTreeMap;
    let mut eng: BTreeMap<(u64, u32), (String, f64)> = BTreeMap::new();
    for (n, v) in engines {
        let Some((luid, rest)) = parse_luid(n) else { continue };
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
    for g in gpus.values_mut() {
        let types: Vec<String> = g.cells.iter().map(|c| c.0.clone()).collect();
        for (cell, name) in g.cells.iter_mut().zip(engine_cell_names(&types)) {
            cell.0 = name;
        }
    }
    for (memory, is_dedicated) in [(dedicated, true), (shared, false)] {
        for (n, v) in memory {
            let Some((luid, _)) = parse_luid(n) else { continue };
            let g = gpus.entry(luid).or_insert_with(|| GpuLive { luid, ..Default::default() });
            if is_dedicated {
                g.dedicated = v.max(0.0) as u64
            } else {
                g.shared = v.max(0.0) as u64
            }
        }
    }
    gpus.into_values().collect()
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
    /// Linux pressure stall information: the share of the last 10 seconds
    /// some task waited for a processor; `None` when the kernel has none.
    pub cpu_pressure: Option<f32>,
    /// The same for memory (some task waited for a page) and for disk I/O.
    pub memory_pressure: Option<f32>,
    pub io_pressure: Option<f32>,
    /// Working set of the "Memory Compression" process (it is protected, so
    /// it is read through the performance counters).
    pub compressed: u64,
    /// Percent of the page file in use.
    pub pagefile_pct: f32,
    /// The counters (keys of [`COUNTERS`]) that gave no value this sample:
    /// their figures are gaps, not 0. Empty when every counter answered.
    pub unanswered: std::collections::BTreeSet<&'static str>,
}

/// The performance counters' keys: in `PerfLive::unanswered` and
/// [`COUNTERS`]. Each key is read by exactly one sensor provider.
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
    /// Linux pressure stall information (in no health list: older kernels have none).
    pub const PRESSURE: &str = "pressure";
}

/// The performance counters every Windows PC answers, by key: the provider
/// state is healthy when all of them answered, degraded when some did not
/// and unavailable when none did. The GPU and energy-meter counters are left
/// out, since a PC without a GPU driver or a power meter never answers them.
pub const COUNTERS: &[&str] = &[counter::KERNEL, counter::PAGEIN, counter::IDLE, counter::DREAD, counter::DWRITE, counter::DRESP];

/// Counters a healthy PC may never answer (VMs, no page file): gaps, but not counted in the provider state.
pub const OPTIONAL: &[&str] = &[counter::PERF, counter::COMPR, counter::PAGEFILE];

/// Not a counter but the GetPerformanceInfo call (threads, handles, commit,
/// cached): in `unanswered` when the call failed, so its figures are gaps.
/// The provider state counts it with [`COUNTERS`] (see [`sources`]).
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
