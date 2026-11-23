//! The machine's fixed facts (processor topology, memory modules, disks,
//! video cards) and the firmware table, read from Windows. They belong to
//! the sensor providers: [`MachineFacts`] reports them through the frame,
//! and the disk, firmware and processor providers take what they need from
//! here at construction, so nothing reads them from a global.

use std::sync::Arc;
use std::time::Instant;

use super::model::Reading;
use super::provider::Provider;
use crate::perfdata::{CpuInfo, DiskInfo, GpuInfo, MemInfo, StaticInfo};

/// Everything this PC's fixed facts say, read now.
pub fn read() -> StaticInfo {
    #[cfg(windows)]
    return imp::static_info();
    #[cfg(not(windows))]
    StaticInfo::default()
}

/// The processor's rated clock in MHz (0 if Windows does not say).
pub fn base_mhz() -> u32 {
    #[cfg(windows)]
    return imp::base_mhz();
    #[cfg(not(windows))]
    0
}

/// The physical disks (model, bus, drive letters), read now.
pub fn disks() -> Vec<DiskInfo> {
    #[cfg(windows)]
    return imp::disk_info();
    #[cfg(not(windows))]
    Vec::new()
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

/// Its name in "Sources on this PC".
const SOURCE: &str = "Machine information";

/// Reports the machine's fixed facts, read once (they do not change while
/// Windows runs), when the service asks for them after its first read. A
/// counter provider, so the core pages have them without anything turning
/// the hardware sensors on.
pub struct MachineFacts {
    facts: Option<Arc<StaticInfo>>,
}

impl MachineFacts {
    pub fn new() -> Self {
        MachineFacts { facts: None }
    }
}

impl Provider for MachineFacts {
    fn name(&self) -> &'static str {
        SOURCE
    }

    /// It has no readings, so "Sources on this PC" does not list it.
    fn listed(&self) -> bool {
        false
    }

    fn read(&mut self, _at: Instant) -> Vec<Reading> {
        Vec::new()
    }

    fn machine(&mut self) -> Option<Arc<StaticInfo>> {
        // Blank facts would read as a machine with nothing in it.
        if !crate::availability::is_available(crate::availability::Feature::PerformanceInfo) {
            return None;
        }
        Some(self.facts.get_or_insert_with(|| Arc::new(read())).clone())
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
    use windows_sys::Win32::System::SystemInformation::{
        GetLogicalProcessorInformationEx, GetSystemWindowsDirectoryW,
        RelationAll, RelationCache, RelationProcessorCore, RelationProcessorPackage,
        SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX,
    };
    use windows_sys::Win32::System::Threading::{IsProcessorFeaturePresent, PF_VIRT_FIRMWARE_ENABLED};
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;

    use crate::winsys::wide;

    pub fn static_info() -> StaticInfo {
        StaticInfo { cpu: cpu_info(), mem: mem_info(), disks: disk_info(), gpus: gpu_info() }
    }

    /// The processor's rated clock from the registry (`~MHz`), 0 if unknown.
    pub fn base_mhz() -> u32 {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
            .and_then(|k| k.get_value::<u32, _>("~MHz"))
            .unwrap_or(0)
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
        c.plan = crate::sensors::power_plan::active_plan_name().unwrap_or_default();
        c.overlay = crate::sensors::power_plan::current_power_mode().unwrap_or_default();
        c
    }

    /// Memory modules from the SMBIOS table (type 17 entries).
    fn mem_info() -> MemInfo {
        let mut m = MemInfo::default();
        let Some(data) = super::smbios_table() else { return m };
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

    pub fn disk_info() -> Vec<DiskInfo> {
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
    fn the_machine_facts_provider_is_not_a_source_of_any_figure() {
        assert!(!MachineFacts::new().listed(), "it reports no readings, so \"Sources on this PC\" does not list it");
    }

    #[test]
    fn an_unavailable_static_information_is_not_reported_as_facts() {
        assert!(MachineFacts::new().machine().is_some() == cfg!(windows), "the system's answer without a test one");
        let _unavailable = crate::availability::unavailable_in_test(&[crate::availability::Feature::PerformanceInfo]);
        assert!(MachineFacts::new().machine().is_none(), "no blank facts: the page leaves them out");
    }

    /// Every source file, with its path under `src/`.
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    #[test]
    fn nothing_reads_the_global_machine_information() {
        for (file, src) in sources() {
            if file.ends_with("machine.rs") {
                continue; // this test names what it forbids
            }
            for word in [concat!("perfdata::", "info()"), concat!("perfdata::", "smbios_table"), concat!("perfdata::", "power_plan")] {
                assert!(!src.contains(word), "{file} still reads {word}");
            }
        }
    }
}
