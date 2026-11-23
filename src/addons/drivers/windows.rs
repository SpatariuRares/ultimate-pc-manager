//! The Windows reader: today's SetupDi, service-manager and file-version
//! code, moved unchanged. Elsewhere it lists nothing.

use super::engine::{DeviceDriver, KernelDriver, SecurityReading};
use super::ListingReader;

/// The Drivers add-on's reader on a real PC.
#[derive(Clone, Copy, Debug, Default)]
pub struct WindowsListing;

impl ListingReader for WindowsListing {
    fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
        imp::list_devices()
    }
    fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
        imp::list_kernel()
    }
    fn security(&self) -> SecurityReading {
        imp::read_security()
    }
    fn signature(&self, path: &str) -> super::signature::Signature {
        super::signature::check(path)
    }
}

#[cfg(not(windows))]
mod imp {
    use super::super::engine::*;
    use crate::availability::{unavailable_error, Feature};
    pub fn list_devices() -> Result<Vec<DeviceDriver>, String> {
        Err(unavailable_error(Feature::DeviceSetupApi))
    }
    pub fn list_kernel() -> Result<Vec<KernelDriver>, String> {
        Err(unavailable_error(Feature::DeviceSetupApi))
    }
    pub fn read_security() -> SecurityReading {
        SecurityReading::unreadable(&unavailable_error(Feature::DeviceSetupApi))
    }
}

#[cfg(windows)]
mod imp {
    use super::super::engine::*;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows_sys::Win32::Devices::Properties::*;
    use windows_sys::Win32::Foundation::DEVPROPKEY;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::Services::*;

    /// One DWORD of HKLM, opened with read rights only (no write path).
    fn read_dword(subkey: &str, name: &str) -> Reading {
        use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
        let missing = |e: &std::io::Error| e.kind() == std::io::ErrorKind::NotFound;
        match winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(subkey, KEY_READ) {
            Err(e) if missing(&e) => Reading::NotSet,
            Err(e) => Reading::Failed(e.to_string()),
            Ok(key) => match key.get_value::<u32, _>(name) {
                Ok(v) => Reading::Value(v),
                Err(e) if missing(&e) => Reading::NotSet,
                Err(e) => Reading::Failed(e.to_string()),
            },
        }
    }

    /// Memory integrity and the vulnerable-driver blocklist.
    pub fn read_security() -> SecurityReading {
        SecurityReading {
            memory_integrity: read_dword(r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled"),
            blocklist: read_dword(r"SYSTEM\CurrentControlSet\Control\CI\Config", "VulnerableDriverBlocklistEnable"),
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

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

    // ------------------------------------------------------- devices

    unsafe fn prop(dn: u32, key: &DEVPROPKEY) -> Option<(u32, Vec<u8>)> {
        let (mut ty, mut size) = (0u32, 0u32);
        CM_Get_DevNode_PropertyW(dn, key, &mut ty, null_mut(), &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut b = vec![0u8; size as usize];
        (CM_Get_DevNode_PropertyW(dn, key, &mut ty, b.as_mut_ptr(), &mut size, 0) == CR_SUCCESS).then_some((ty, b))
    }

    fn utf16(b: &[u8]) -> Vec<u16> {
        b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }

    unsafe fn prop_str(dn: u32, key: &DEVPROPKEY) -> String {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_STRING, b)) => {
                let w = utf16(&b);
                let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
                String::from_utf16_lossy(&w[..end]).trim().to_string()
            }
            _ => String::new(),
        }
    }

    unsafe fn prop_list(dn: u32, key: &DEVPROPKEY) -> Vec<String> {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_STRING_LIST, b)) => utf16(&b)
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect(),
            _ => Vec::new(),
        }
    }

    unsafe fn prop_date(dn: u32, key: &DEVPROPKEY) -> String {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_FILETIME, b)) if b.len() >= 8 => {
                let ft = u64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8]));
                if ft == 0 {
                    return String::new();
                }
                // 100 ns ticks since 1601-01-01 → days since 1970-01-01.
                let days = (ft / 864_000_000_000) as i64 - 134_774;
                let (y, m, d) = civil(days);
                format!("{y:04}-{m:02}-{d:02}")
            }
            _ => String::new(),
        }
    }

    /// Days since 1970-01-01 → (year, month, day).
    fn civil(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
    }

    pub fn list_devices() -> Result<Vec<DeviceDriver>, String> {
        let mut out = Vec::new();
        unsafe {
            let mut len = 0u32;
            let cr = CM_Get_Device_ID_List_SizeW(&mut len, null(), CM_GETIDLIST_FILTER_NONE);
            if cr != CR_SUCCESS {
                return Err(format!("the configuration manager could not size the device list (CONFIGRET {cr:#x})"));
            }
            if len == 0 {
                return Ok(out);
            }
            let mut buf = vec![0u16; len as usize];
            let cr = CM_Get_Device_ID_ListW(null(), buf.as_mut_ptr(), len, CM_GETIDLIST_FILTER_NONE);
            if cr != CR_SUCCESS {
                return Err(format!("the configuration manager could not list the devices (CONFIGRET {cr:#x})"));
            }
            for id in buf.split(|&c| c == 0).filter(|s| !s.is_empty()) {
                let mut wid = id.to_vec();
                wid.push(0);
                let mut dn = 0u32;
                if CM_Locate_DevNodeW(&mut dn, wid.as_ptr(), CM_LOCATE_DEVNODE_PHANTOM) != CR_SUCCESS {
                    continue;
                }
                // Devices without a driver package have nothing to update.
                let inf = prop_str(dn, &DEVPKEY_Device_DriverInfPath);
                if inf.is_empty() {
                    continue;
                }
                let (mut status, mut problem) = (0u32, 0u32);
                let present = CM_Get_DevNode_Status(&mut status, &mut problem, dn, 0) == CR_SUCCESS;
                let mut name = prop_str(dn, &DEVPKEY_Device_FriendlyName);
                if name.is_empty() {
                    name = prop_str(dn, &DEVPKEY_Device_DeviceDesc);
                }
                out.push(DeviceDriver {
                    instance_id: String::from_utf16_lossy(id),
                    name,
                    class: prop_str(dn, &DEVPKEY_Device_Class),
                    manufacturer: prop_str(dn, &DEVPKEY_Device_Manufacturer),
                    provider: prop_str(dn, &DEVPKEY_Device_DriverProvider),
                    version: prop_str(dn, &DEVPKEY_Device_DriverVersion),
                    date: prop_date(dn, &DEVPKEY_Device_DriverDate),
                    inf,
                    service: prop_str(dn, &DEVPKEY_Device_Service),
                    hardware_ids: prop_list(dn, &DEVPKEY_Device_HardwareIds),
                    compatible_ids: prop_list(dn, &DEVPKEY_Device_CompatibleIds),
                    present,
                    started: present && status & DN_STARTED != 0,
                    problem: if present && status & DN_HAS_PROBLEM != 0 { problem } else { 0 },
                });
            }
        }
        out.sort_by(|a, b| (a.class.to_lowercase(), a.name.to_lowercase()).cmp(&(b.class.to_lowercase(), b.name.to_lowercase())));
        Ok(out)
    }

    // ------------------------------------------------ kernel drivers

    /// "@%SystemRoot%\System32\drivers\tcpipreg.sys,-10110" -> the text it
    /// points to. Some entries carry a stray trailing comma; when nothing
    /// can be loaded the service name stands in.
    fn indirect(s: &str, service: &str) -> String {
        if !s.starts_with('@') {
            return if s.is_empty() { service.to_string() } else { s.to_string() };
        }
        let clean = s.trim_end_matches([',', ';', ' ']);
        for candidate in [s, clean] {
            unsafe {
                let w = wide(candidate);
                let mut out = vec![0u16; 512];
                if windows_sys::Win32::UI::Shell::SHLoadIndirectString(w.as_ptr(), out.as_mut_ptr(), 512, null()) == 0 {
                    let t = from_wide(out.as_ptr());
                    if !t.is_empty() {
                        return t;
                    }
                }
            }
        }
        service.to_string()
    }

    /// "\SystemRoot\System32\drivers\ACPI.sys", "System32\drivers\x.sys",
    /// "\??\C:\…" or nothing → a normal path.
    fn resolve_image(path: &str, service: &str) -> String {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let p = path.trim().trim_matches('"');
        if p.is_empty() {
            return format!(r"{windir}\System32\drivers\{service}.sys");
        }
        let lower = p.to_lowercase();
        if let Some(rest) = lower.strip_prefix(r"\systemroot\") {
            return format!(r"{windir}\{}", &p[p.len() - rest.len()..]);
        }
        if let Some(rest) = p.strip_prefix(r"\??\") {
            return rest.to_string();
        }
        if lower.starts_with("system32\\") || lower.starts_with("syswow64\\") {
            return format!(r"{windir}\{p}");
        }
        p.to_string()
    }

    /// File version ("10.0.26100.1") and CompanyName from the version resource.
    pub fn file_version(path: &str) -> (String, String) {
        unsafe {
            let w = wide(path);
            let mut handle = 0u32;
            let size = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
            if size == 0 {
                return (String::new(), String::new());
            }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
                return (String::new(), String::new());
            }
            let mut version = String::new();
            let (mut p, mut len): (*mut c_void, u32) = (null_mut(), 0);
            let root = wide("\\");
            if VerQueryValueW(data.as_ptr() as *const c_void, root.as_ptr(), &mut p, &mut len) != 0 && len >= 16 {
                let f = p as *const u32;
                let (ms, ls) = (*f.add(2), *f.add(3));
                version = format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff);
            }
            let mut company = String::new();
            let tr = wide("\\VarFileInfo\\Translation");
            let mut langs: Vec<String> = Vec::new();
            if VerQueryValueW(data.as_ptr() as *const c_void, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
                let t = p as *const u16;
                langs.push(format!("{:04x}{:04x}", *t, *t.add(1)));
            }
            langs.extend(["040904b0".to_string(), "040904e4".to_string(), "000004b0".to_string()]);
            for l in langs {
                let key = wide(&format!("\\StringFileInfo\\{l}\\CompanyName"));
                if VerQueryValueW(data.as_ptr() as *const c_void, key.as_ptr(), &mut p, &mut len) != 0 && len > 0 {
                    company = from_wide(p as *const u16).trim().to_string();
                    if !company.is_empty() {
                        break;
                    }
                }
            }
            (version, company)
        }
    }

    /// Loaded kernel images with their full paths, from
    /// NtQuerySystemInformation(SystemModuleInformation); None when the
    /// list could not be read.
    fn kernel_modules() -> Option<Vec<LoadedModule>> {
        let mut out = Vec::new();
        unsafe {
            let lib = wide("ntdll.dll");
            let h = LoadLibraryW(lib.as_ptr());
            if h.is_null() {
                return None;
            }
            let f = GetProcAddress(h, b"NtQuerySystemInformation\0".as_ptr())?;
            let query: unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32 = std::mem::transmute(f);
            let mut len = 0u32;
            query(11, null_mut(), 0, &mut len);
            if len == 0 {
                return None;
            }
            let mut buf = vec![0u64; (len as usize + 65536) / 8];
            if query(11, buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32, &mut len) < 0 {
                return None;
            }
            let base = buf.as_ptr() as *const u8;
            let count = *(base as *const u32) as usize;
            // RTL_PROCESS_MODULE_INFORMATION is 296 bytes; the list starts at 8.
            for i in 0..count {
                let e = base.add(8 + i * 296);
                if 8 + (i + 1) * 296 > buf.len() * 8 {
                    break;
                }
                let size = *(e.add(24) as *const u32) as u64;
                let index = *(e.add(32) as *const u16) as u32;
                let path = std::slice::from_raw_parts(e.add(40), 256);
                let end = path.iter().position(|&b| b == 0).unwrap_or(256);
                out.push(LoadedModule { path: String::from_utf8_lossy(&path[..end]).into_owned(), size, index });
            }
        }
        Some(out)
    }

    pub fn list_kernel() -> Result<Vec<KernelDriver>, String> {
        let modules = kernel_modules();
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        let mut out = Vec::new();
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err(format!("the service control manager could not be opened: {}", std::io::Error::last_os_error()));
            }
            let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
            EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_DRIVER, SERVICE_STATE_ALL, null_mut(), 0, &mut needed, &mut count, &mut resume, null());
            let mut buf = vec![0u8; needed as usize + 4096];
            resume = 0;
            if EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_DRIVER, SERVICE_STATE_ALL, buf.as_mut_ptr(), buf.len() as u32, &mut needed, &mut count, &mut resume, null()) == 0 {
                let e = std::io::Error::last_os_error();
                CloseServiceHandle(scm);
                return Err(format!("the service control manager could not list the driver services: {e}"));
            }
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            let mut cfg = vec![0u64; 1024];
            for it in items {
                let service = from_wide(it.lpServiceName);
                let st = &it.ServiceStatusProcess;
                let mut d = KernelDriver {
                    display: indirect(&from_wide(it.lpDisplayName), &service),
                    running: st.dwCurrentState == SERVICE_RUNNING,
                    state_code: Some(st.dwCurrentState),
                    kind: kernel_kind_from_type(st.dwServiceType),
                    start_code: None,
                    ..Default::default()
                };
                let mut raw_path = String::new();
                let h = OpenServiceW(scm, it.lpServiceName, SERVICE_QUERY_CONFIG);
                if !h.is_null() {
                    let mut need = 0u32;
                    if QueryServiceConfigW(h, cfg.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, (cfg.len() * 8) as u32, &mut need) != 0 {
                        let c = &*(cfg.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                        d.start_code = Some(c.dwStartType);
                        raw_path = from_wide(c.lpBinaryPathName);
                        d.group = from_wide(c.lpLoadOrderGroup);
                        if !c.lpDependencies.is_null() {
                            // A list of nul-ended strings ended by an empty one: read up to the double nul.
                            let mut n = 0;
                            while !(*c.lpDependencies.add(n) == 0 && *c.lpDependencies.add(n + 1) == 0) {
                                n += 1;
                            }
                            (d.depends_services, d.depends_groups) = parse_dependencies(std::slice::from_raw_parts(c.lpDependencies, n + 2));
                        }
                    }
                    CloseServiceHandle(h);
                }
                d.start_type = start_type_name(d.start_code);
                d.path = resolve_image(&raw_path, &service);
                if std::path::Path::new(&d.path).exists() {
                    let (v, c) = file_version(&d.path);
                    d.version = v;
                    d.vendor = c;
                }
                if d.is_loaded() {
                    match &modules {
                        Some(list) => {
                            if let Some(m) = find_module(list, &d.path, &windir) {
                                d.image_size = Some(m.size);
                                d.load_index = Some(m.index);
                            }
                        }
                        None => d.modules_unread = true,
                    }
                }
                d.service = service;
                out.push(d);
            }
            CloseServiceHandle(scm);
        }
        out.sort_by_key(|d| d.display.to_lowercase());
        Ok(out)
    }
}

#[cfg(all(test, windows))]
mod pc_tests {
    use super::*;

    /// Reads this PC's kernel drivers: states, kinds and dependencies are plausible.
    #[test]
    #[ignore = "reads this PC's service control manager"]
    fn this_pc_reports_states_kinds_groups_and_dependencies() {
        let list = WindowsListing.kernel().expect("kernel drivers");
        assert!(list.iter().all(|d| d.state_code.is_some()), "every state was read");
        assert!(list.iter().any(|d| d.state_label() == "Running"));
        assert!(list.iter().any(|d| d.kind.label() == "File system"));
        assert!(list.iter().any(|d| !d.group.is_empty()), "some service has a load order group");
        let with_deps: Vec<_> = list.iter().filter(|d| !d.depends_on_text().is_empty()).collect();
        assert!(!with_deps.is_empty(), "some driver depends on a service or group");
        println!("{} drivers, {} with dependencies", list.len(), with_deps.len());
    }
}
