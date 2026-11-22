//! Extra Windows data for the Performance page:
//! PDH counters (kernel time, CPU frequency, physical disks, GPU engines,
//! CPU package power), GetPerformanceInfo, processor topology, SMBIOS memory
//! details, physical disks and GPU adapters.

use std::sync::OnceLock;

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
pub const PROVIDED: &[&str] = &[counter::PERF, counter::IDLE, counter::DREAD, counter::DWRITE, counter::POWER, counter::GPU, counter::GDED, counter::GSH, counter::KERNEL, counter::PAGEIN, counter::DRESP, counter::COMPR, counter::PAGEFILE, PERF_INFO];

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

#[derive(Clone, Debug, Default)]
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

#[derive(Clone, Debug, Default)]
pub struct MemInfo {
    pub speed_mts: Option<u32>,
    pub kind: Option<&'static str>,
    pub form: Option<&'static str>,
    pub used_slots: u32,
    pub slots: u32,
}

#[derive(Clone, Debug, Default)]
pub struct DiskInfo {
    pub number: u32,
    pub model: String,
    pub bus: &'static str,
    pub capacity: u64,
    pub letters: Vec<char>,
    pub formatted: u64,
    pub system: bool,
}

#[derive(Clone, Debug, Default)]
pub struct GpuInfo {
    pub luid: u64,
    pub name: String,
    pub dedicated: u64,
    pub shared: u64,
    pub driver: String,
    pub location: String,
}

#[derive(Clone, Debug, Default)]
pub struct StaticInfo {
    pub cpu: CpuInfo,
    pub mem: MemInfo,
    pub disks: Vec<DiskInfo>,
    pub gpus: Vec<GpuInfo>,
}

/// Facts that do not change while the program runs, read once.
/// The active power plan's name, from the power plan API (empty if unknown).
pub fn power_plan() -> String {
    crate::sensors::power_plan::active_plan_name().unwrap_or_default()
}

/// The raw SMBIOS table (`RawSMBIOSData`, from
/// `GetSystemFirmwareTable('RSMB')`), if Windows gives it.
pub fn smbios_table() -> Option<Vec<u8>> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::SystemInformation::{GetSystemFirmwareTable, RSMB};
        // SAFETY: the first call only asks the size (null buffer, size 0);
        // the second fills a buffer of that size, passed with its size.
        unsafe {
            let size = GetSystemFirmwareTable(RSMB, 0, std::ptr::null_mut(), 0);
            if size == 0 {
                return None;
            }
            let mut buf = vec![0u8; size as usize];
            let got = GetSystemFirmwareTable(RSMB, 0, buf.as_mut_ptr().cast(), size);
            if got == 0 || got > size {
                return None;
            }
            buf.truncate(got as usize);
            Some(buf)
        }
    }
    #[cfg(not(windows))]
    None
}

pub fn info() -> &'static StaticInfo {
    static I: OnceLock<StaticInfo> = OnceLock::new();
    I.get_or_init(imp::static_info)
}

/// What the performance-data collector still reads: nothing. Every counter
/// comes from the sensor service's providers (see `sampler::counters_into`);
/// the collector is removed with the rest of the old path.
pub struct Collector;

impl Collector {
    pub fn new() -> Self {
        Collector
    }
    pub fn sample(&mut self) -> PerfLive {
        PerfLive { unanswered: collected().collect(), ..Default::default() }
    }
}
#[cfg(not(windows))]
mod imp {
    pub fn static_info() -> super::StaticInfo {
        Default::default()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::BTreeMap;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::ptr::{null, null_mut};

    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDriveStringsW, FILE_SHARE_READ,
        FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Ioctl::{
        PropertyStandardQuery, StorageDeviceProperty, DISK_EXTENT, DISK_GEOMETRY_EX,
        IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_STORAGE_QUERY_PROPERTY, STORAGE_DEVICE_DESCRIPTOR,
        STORAGE_PROPERTY_QUERY,
    };
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Performance::*;
    use windows_sys::Win32::System::ProcessStatus::{GetPerformanceInfo, PERFORMANCE_INFORMATION};
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, GetSystemWindowsDirectoryW,
        RelationAll, RelationCache, RelationProcessorCore, RelationProcessorPackage,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    use windows_sys::Win32::System::Threading::{IsProcessorFeaturePresent, PF_VIRT_FIRMWARE_ENABLED};
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

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

    // ---------------------------------------------------------- static facts

    pub fn static_info() -> StaticInfo {
        StaticInfo { cpu: cpu_info(), mem: mem_info(), disks: disk_info(), gpus: gpu_info() }
    }

    fn cpu_info() -> CpuInfo {
        let mut c = CpuInfo::default();
        unsafe {
            let mut len = 0u32;
            GetLogicalProcessorInformationEx(RelationAll, null_mut(), &mut len);
            if len > 0 {
                let mut buf = vec![0u64; len as usize / 8 + 1];
                let base = buf.as_mut_ptr() as *mut u8;
                if GetLogicalProcessorInformationEx(RelationAll, base as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX, &mut len) != 0 {
                    let mut off = 0usize;
                    while off < len as usize {
                        let e = &*(base.add(off) as *const SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX);
                        match e.Relationship {
                            r if r == RelationProcessorCore => c.cores += 1,
                            r if r == RelationProcessorPackage => c.sockets += 1,
                            r if r == RelationCache => {
                                let k = &e.Anonymous.Cache;
                                match k.Level {
                                    1 => c.l1 += k.CacheSize as u64,
                                    2 => c.l2 += k.CacheSize as u64,
                                    3 => c.l3 += k.CacheSize as u64,
                                    _ => {}
                                }
                            }
                            _ => {}
                        }
                        if e.Size == 0 {
                            break;
                        }
                        off += e.Size as usize;
                    }
                }
            }
        }
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        if let Ok(k) = hklm.open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") {
            c.base_mhz = k.get_value::<u32, _>("~MHz").unwrap_or(0);
        }
        #[cfg(target_arch = "x86_64")]
        let hypervisor = std::arch::x86_64::__cpuid(1).ecx >> 31 & 1 == 1;
        #[cfg(not(target_arch = "x86_64"))]
        let hypervisor = false;
        let firmware = unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) != 0 };
        c.virtualization = Some(firmware || hypervisor);
        if let Ok(k) = hklm.open_subkey(r"HARDWARE\DESCRIPTION\System\BIOS") {
            let s = format!(
                "{} {}",
                k.get_value::<String, _>("SystemManufacturer").unwrap_or_default(),
                k.get_value::<String, _>("SystemProductName").unwrap_or_default()
            )
            .to_lowercase();
            c.vm = ["virtual machine", "vmware", "virtualbox", "kvm", "qemu", "xen", "parallels", "hyper-v"]
                .iter()
                .any(|m| s.contains(m));
        }
        let hiberboot = hklm
            .open_subkey(r"SYSTEM\CurrentControlSet\Control\Session Manager\Power")
            .and_then(|k| k.get_value::<u32, _>("HiberbootEnabled"))
            .ok();
        let hibernate = hklm
            .open_subkey(r"SYSTEM\CurrentControlSet\Control\Power")
            .and_then(|k| k.get_value::<u32, _>("HibernateEnabled"))
            .unwrap_or(1);
        c.fast_startup = hiberboot.map(|h| h == 1 && hibernate == 1);
        c.plan = crate::perfdata::power_plan();
        c.overlay = crate::sensors::power_plan::current_power_mode().unwrap_or_default();
        c
    }

    /// Memory modules from the SMBIOS table (type 17 entries).
    fn mem_info() -> MemInfo {
        let mut m = MemInfo::default();
        let Some(data) = smbios_table() else { return m };
        if data.len() < 8 {
            return m;
        }
        let t = &data[8..];
        let u16at = |s: &[u8], o: usize| if o + 1 < s.len() { u16::from_le_bytes([s[o], s[o + 1]]) } else { 0 };
        let u32at = |s: &[u8], o: usize| if o + 3 < s.len() { u32::from_le_bytes([s[o], s[o + 1], s[o + 2], s[o + 3]]) } else { 0 };
        let mut off = 0usize;
        while off + 4 <= t.len() {
            let ty = t[off];
            let len = t[off + 1] as usize;
            if len < 4 || off + len > t.len() {
                break;
            }
            let s = &t[off..off + len];
            if ty == 17 {
                m.slots += 1;
                let size = u16at(s, 0x0C);
                if size != 0 && size != 0xFFFF {
                    m.used_slots += 1;
                    m.form = m.form.or(match s.get(0x0E) {
                        Some(0x09) => Some("DIMM"),
                        Some(0x0D) => Some("SODIMM"),
                        Some(0x0F) => Some("FB-DIMM"),
                        Some(0x10) => Some("Die"),
                        _ => None,
                    });
                    m.kind = m.kind.or(match s.get(0x12) {
                        Some(0x18) => Some("DDR3"),
                        Some(0x1A) => Some("DDR4"),
                        Some(0x1B) => Some("LPDDR"),
                        Some(0x1C) => Some("LPDDR2"),
                        Some(0x1D) => Some("LPDDR3"),
                        Some(0x1E) => Some("LPDDR4"),
                        Some(0x22) => Some("DDR5"),
                        Some(0x23) => Some("LPDDR5"),
                        _ => None,
                    });
                    let mut speed = if len > 0x21 { u16at(s, 0x20) as u32 } else { 0 };
                    if speed == 0xFFFF && len > 0x5B {
                        speed = u32at(s, 0x58);
                    }
                    if speed == 0 || speed == 0xFFFF {
                        speed = u16at(s, 0x15) as u32;
                        if speed == 0xFFFF && len > 0x57 {
                            speed = u32at(s, 0x54);
                        }
                    }
                    if speed != 0 && speed != 0xFFFF {
                        m.speed_mts = Some(m.speed_mts.unwrap_or(0).max(speed));
                    }
                }
            }
            if ty == 127 {
                break;
            }
            // Skip the formatted area, then the strings up to a double NUL.
            let mut p = off + len;
            while p + 1 < t.len() && !(t[p] == 0 && t[p + 1] == 0) {
                p += 1;
            }
            off = p + 2;
        }
        m
    }

    unsafe fn open_device(path: &str) -> Option<HANDLE> {
        let w = wide(path);
        let h = CreateFileW(w.as_ptr(), 0, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, 0, null_mut());
        (h != INVALID_HANDLE_VALUE).then_some(h)
    }

    unsafe fn ioctl(h: HANDLE, code: u32, input: *const c_void, in_len: u32, out: &mut [u8]) -> bool {
        let mut ret = 0u32;
        DeviceIoControl(h, code, input, in_len, out.as_mut_ptr() as *mut c_void, out.len() as u32, &mut ret, null_mut()) != 0
    }

    fn disk_info() -> Vec<DiskInfo> {
        let mut disks: Vec<DiskInfo> = Vec::new();
        unsafe {
            for n in 0..32u32 {
                let Some(h) = open_device(&format!(r"\\.\PhysicalDrive{n}")) else { continue };
                let mut d = DiskInfo { number: n, bus: "Unknown", ..Default::default() };
                let q = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceProperty, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
                let mut buf = vec![0u8; 1024];
                if ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, &q as *const _ as *const c_void, size_of::<STORAGE_PROPERTY_QUERY>() as u32, &mut buf) {
                    let desc = &*(buf.as_ptr() as *const STORAGE_DEVICE_DESCRIPTOR);
                    let text = |o: u32| -> String {
                        let o = o as usize;
                        if o == 0 || o >= buf.len() {
                            return String::new();
                        }
                        let end = buf[o..].iter().position(|&b| b == 0).map(|e| o + e).unwrap_or(buf.len());
                        String::from_utf8_lossy(&buf[o..end]).trim().to_string()
                    };
                    let vendor = text(desc.VendorIdOffset);
                    let product = text(desc.ProductIdOffset);
                    d.model = if vendor.is_empty() || product.starts_with(&vendor) { product } else { format!("{vendor} {product}") };
                    d.bus = match desc.BusType {
                        17 => "NVMe",
                        11 => "SATA",
                        7 => "USB",
                        10 => "SAS",
                        8 => "RAID",
                        3 => "ATA",
                        1 => "SCSI",
                        12 => "SD",
                        13 => "MMC",
                        14 | 15 => "Virtual",
                        _ => "Unknown",
                    };
                }
                let mut g = vec![0u8; 256];
                if ioctl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, null(), 0, &mut g) {
                    d.capacity = (*(g.as_ptr() as *const DISK_GEOMETRY_EX)).DiskSize.max(0) as u64;
                }
                CloseHandle(h);
                if d.model.is_empty() {
                    d.model = format!("Disk {n}");
                }
                disks.push(d);
            }

            // Which drive letters live on which disk.
            let mut windir = [0u16; 260];
            let wl = GetSystemWindowsDirectoryW(windir.as_mut_ptr(), 260);
            let sys_letter = (wl > 0).then(|| char::from_u32(windir[0] as u32)).flatten().map(|c| c.to_ascii_uppercase());
            let mut drives = [0u16; 512];
            let len = GetLogicalDriveStringsW(512, drives.as_mut_ptr()) as usize;
            for root in String::from_utf16_lossy(&drives[..len.min(512)]).split('\0').filter(|s| !s.is_empty()) {
                let Some(letter) = root.chars().next().map(|c| c.to_ascii_uppercase()) else { continue };
                let rw = wide(root);
                // 2 removable, 3 fixed; skip CD-ROMs and network drives.
                if !matches!(GetDriveTypeW(rw.as_ptr()), 2 | 3) {
                    continue;
                }
                let Some(h) = open_device(&format!(r"\\.\{letter}:")) else { continue };
                let mut out = vec![0u8; 512];
                let ok = ioctl(h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, null(), 0, &mut out);
                CloseHandle(h);
                if !ok {
                    continue;
                }
                let count = u32::from_le_bytes([out[0], out[1], out[2], out[3]]) as usize;
                let first = out.as_ptr().add(8) as *const DISK_EXTENT;
                let mut total = 0u64;
                GetDiskFreeSpaceExW(rw.as_ptr(), null_mut(), &mut total, null_mut());
                for i in 0..count.min(16) {
                    let ext = &*first.add(i);
                    if let Some(d) = disks.iter_mut().find(|d| d.number == ext.DiskNumber) {
                        if !d.letters.contains(&letter) {
                            d.letters.push(letter);
                            d.formatted += total;
                        }
                        if Some(letter) == sys_letter {
                            d.system = true;
                        }
                    }
                }
            }
        }
        disks
    }

    #[repr(C)]
    struct DxgiAdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        sub_sys_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        luid_low: u32,
        luid_high: i32,
        flags: u32,
    }

    #[link(name = "dxgi")]
    extern "system" {
        fn CreateDXGIFactory1(riid: *const GUID, factory: *mut *mut c_void) -> i32;
    }

    unsafe fn vtable_fn(obj: *mut c_void, index: usize) -> *const c_void {
        let vtbl = *(obj as *const *const *const c_void);
        *vtbl.add(index)
    }

    unsafe fn release(obj: *mut c_void) {
        let f: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(vtable_fn(obj, 2));
        f(obj);
    }

    /// GPU adapters through DXGI (IDXGIFactory1::EnumAdapters1 and
    /// IDXGIAdapter1::GetDesc1, called through their vtables).
    fn gpu_info() -> Vec<GpuInfo> {
        let mut out: Vec<GpuInfo> = Vec::new();
        unsafe {
            let iid = GUID::from_u128(0x770aae78_f26f_4dba_a829_253c83d1b387);
            let mut factory: *mut c_void = null_mut();
            if CreateDXGIFactory1(&iid, &mut factory) < 0 || factory.is_null() {
                return out;
            }
            let enum_adapters: unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32 =
                std::mem::transmute(vtable_fn(factory, 12));
            for i in 0..16u32 {
                let mut adapter: *mut c_void = null_mut();
                if enum_adapters(factory, i, &mut adapter) < 0 || adapter.is_null() {
                    break;
                }
                let get_desc: unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32 =
                    std::mem::transmute(vtable_fn(adapter, 10));
                let mut d: DxgiAdapterDesc1 = zeroed();
                let ok = get_desc(adapter, &mut d) >= 0;
                release(adapter);
                // Skip the software renderer (flag 2) and Microsoft's basic adapter.
                if !ok || d.flags & 2 != 0 || d.vendor_id == 0x1414 {
                    continue;
                }
                let luid = ((d.luid_high as u32 as u64) << 32) | d.luid_low as u64;
                if out.iter().any(|g| g.luid == luid) {
                    continue;
                }
                let end = d.description.iter().position(|&c| c == 0).unwrap_or(128);
                out.push(GpuInfo {
                    luid,
                    name: String::from_utf16_lossy(&d.description[..end]).trim().to_string(),
                    dedicated: d.dedicated_video_memory as u64,
                    shared: d.shared_system_memory as u64,
                    ..Default::default()
                });
            }
            release(factory);
        }
        // Adapters are listed by name.
        out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        add_driver_details(&mut out);
        out
    }

    /// Driver version, date and PCI location from the display-class registry keys.
    fn add_driver_details(gpus: &mut [GpuInfo]) {
        const CLASS: &str = "{4d36e968-e325-11ce-bfc1-08002be10318}";
        let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
        let Ok(class) = hklm.open_subkey(format!(r"SYSTEM\CurrentControlSet\Control\Class\{CLASS}")) else { return };
        // driver key ("0001") → PCI location text
        let mut locations: BTreeMap<String, String> = BTreeMap::new();
        if let Ok(pci) = hklm.open_subkey(r"SYSTEM\CurrentControlSet\Enum\PCI") {
            for dev in pci.enum_keys().flatten() {
                let Ok(dk) = pci.open_subkey(&dev) else { continue };
                for inst in dk.enum_keys().flatten() {
                    let Ok(ik) = dk.open_subkey(&inst) else { continue };
                    let driver: String = ik.get_value("Driver").unwrap_or_default();
                    if let Some(k) = driver.to_lowercase().strip_prefix(&format!("{CLASS}\\")) {
                        let loc: String = ik.get_value("LocationInformation").unwrap_or_default();
                        locations.insert(k.to_string(), pci_location(&loc));
                    }
                }
            }
        }
        for key in class.enum_keys().flatten() {
            let Ok(k) = class.open_subkey(&key) else { continue };
            let desc: String = k.get_value("DriverDesc").unwrap_or_default();
            let Some(g) = gpus.iter_mut().find(|g| g.name.eq_ignore_ascii_case(desc.trim())) else { continue };
            if !g.driver.is_empty() {
                continue;
            }
            let ver: String = k.get_value("DriverVersion").unwrap_or_default();
            let date: String = k.get_value("DriverDate").unwrap_or_default();
            let parts: Vec<&str> = date.split('-').collect();
            g.driver = if parts.len() == 3 {
                format!("{ver} ({}-{:0>2}-{:0>2})", parts[2], parts[0], parts[1])
            } else {
                ver
            };
            if let Some(l) = locations.get(&key.to_lowercase()) {
                g.location = l.clone();
            }
        }
    }

    /// "PCI bus 11, device 0, function 0" → "0b:00.0". Newer systems store
    /// "@System32\drivers\pci.sys,#65536;PCI bus %1, device %2, function %3;(11,0,0)":
    /// then the numbers are the ones in the last parentheses.
    fn pci_location(s: &str) -> String {
        let s = match (s.rfind('('), s.rfind(')')) {
            (Some(a), Some(b)) if a < b => &s[a + 1..b],
            _ => s,
        };
        let nums: Vec<u32> = s
            .split(|c: char| !c.is_ascii_digit())
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse().ok())
            .collect();
        if nums.len() >= 3 {
            format!("{:02x}:{:02x}.{}", nums[0], nums[1], nums[2])
        } else {
            String::new()
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
