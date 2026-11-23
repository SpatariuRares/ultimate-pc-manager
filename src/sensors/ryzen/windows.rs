//! Windows: AMD's Ryzen Master Monitoring SDK, called through its
//! `Platform.dll`, and what the registry says about the SDK, the BIOS and
//! the driver.

use crate::sensors::ryzen::{looks_virtual, platform_dll, Probe};
use std::path::PathBuf;

/// What this PC says about the SDK.
pub fn probe(amd: bool) -> Probe {
    if !amd {
        return Probe { amd, ..Probe::default() };
    }
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    let sdk = hklm
        .open_subkey(r"SOFTWARE\AMD\RyzenMasterMonitoringSDK")
        .and_then(|k| k.get_value::<String, _>("InstallationPath"))
        .ok()
        .map(PathBuf::from)
        .filter(|dir| platform_dll(dir).is_file());
    let version = sdk.as_deref().and_then(|dir| sdk::file_version(&platform_dll(dir)));
    let bios = hklm.open_subkey(r"HARDWARE\DESCRIPTION\System\BIOS").ok();
    let field = |name: &str| bios.as_ref().and_then(|k| k.get_value::<String, _>(name).ok()).unwrap_or_default();
    let virtual_machine = looks_virtual(&format!("{} {}", field("SystemManufacturer"), field("SystemProductName")));
    Probe { amd, sdk, version, virtual_machine, admin: crate::platform::is_admin(), driver_running: driver_running() }
}

/// Whether one of AMD's Ryzen Master drivers (`AMDRyzenMasterDriverV*`) runs.
fn driver_running() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let Ok(services) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SYSTEM\CurrentControlSet\Services") else {
        return false;
    };
    let names: Vec<String> = services.enum_keys().flatten().filter(|k| k.to_ascii_lowercase().starts_with("amdryzenmasterdriver")).collect();
    names.iter().any(|name| crate::winsys::service_running(name) == Some(true))
}

pub use sdk::load;
/// Whether a DLL of the SDK is mapped in this process (tests).
#[cfg(test)]
pub use sdk::loaded;

/// The calls into `Platform.dll` (see the module comment for the interface).
/// Its virtual methods are called as `extern "C"` functions taking `this`
/// first, which matches the C++ (`thiscall`) convention only on x86_64:
/// other targets get the stub below and report the SDK unsupported.
#[cfg(target_arch = "x86_64")]
mod sdk {
    use crate::sensors::ryzen::{answer, failure_reason, find_cpu, platform_dll, CpuParameters, Devices, Failure, RawCpuParameters, Sdk, Session, DT_CPU};
    use std::ffi::c_void;
    use std::path::Path;
    use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR};

    const PLATFORM_INIT: usize = 0;
    const PLATFORM_UNINIT: usize = 1;
    const PLATFORM_DEVICE_MANAGER: usize = 2;
    const DEVICE_GET_TYPE: usize = 7;
    const CPU_GET_CORE_COUNT: usize = 14;
    const CPU_GET_PARAMETERS: usize = 17;

    /// The SDK's one session in this process (see [`Session`]).
    static SESSION: Session = Session::new();

    type GetPlatform = unsafe extern "C" fn() -> *mut c_void;
    type PlatformInit = unsafe extern "C" fn(*mut c_void, *const u8, u8) -> u8;
    type UnInit = unsafe extern "C" fn(*mut c_void) -> u8;
    type GetManager = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
    type GetDevice = unsafe extern "C" fn(*mut c_void, i32, u32) -> *mut c_void;
    type GetType = unsafe extern "C" fn(*mut c_void) -> i32;
    type GetCoreCount = unsafe extern "C" fn(*mut c_void, *mut u32) -> i32;
    type GetParameters = unsafe extern "C" fn(*mut c_void, *mut RawCpuParameters) -> i32;

    /// The struct the SDK fills, with room behind it: a newer SDK that
    /// appends fields writes into `_spare`, not past the buffer.
    #[repr(C)]
    struct Buffer {
        raw: RawCpuParameters,
        _spare: [u8; 512],
    }

    /// Entry `index` of a C++ object's vtable.
    ///
    /// # Safety
    /// `obj` is a live C++ object (vtable pointer first) whose vtable has
    /// more than `index` entries.
    unsafe fn slot(obj: *mut c_void, index: usize) -> *const c_void {
        let vtable = *(obj as *const *const *const c_void);
        *vtable.add(index)
    }

    fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    }

    /// `Platform.dll`, started, with its first CPU device.
    struct Loaded {
        lib: HMODULE,
        platform: *mut c_void,
        cpu: *mut c_void,
    }

    impl Sdk for Loaded {
        fn cpu_parameters(&mut self) -> Result<CpuParameters, String> {
            let mut buf = Buffer { raw: RawCpuParameters::default(), _spare: [0; 512] };
            // SAFETY: `cpu` was checked to be the SDK's ICPUEx (GetType,
            // GetCoreCount); slot 17 is GetCPUParameters(CPUParameters&),
            // given a zeroed struct (no arrays) with spare room behind it.
            let status = unsafe {
                let f: GetParameters = std::mem::transmute(slot(self.cpu, CPU_GET_PARAMETERS));
                f(self.cpu, &mut buf.raw)
            };
            answer(status, &buf.raw).map_err(String::from)
        }
    }

    // SAFETY: the SDK's objects are process-wide (one `IPlatform` per
    // process), not tied to the thread that started them; a `Loaded` is
    // reached only through the session's mutex, one call at a time.
    unsafe impl Send for Loaded {}

    impl Drop for Loaded {
        fn drop(&mut self) {
            // The platform Init succeeded: UnInit then unload, once (the
            // session drops its `Loaded` when its last lease ends).
            let mut started = Platform { lib: self.lib, platform: self.platform, manager: std::ptr::null_mut() };
            started.uninit();
            started.unload();
        }
    }

    /// `Platform.dll` loaded and its `IPlatform` started (`Init` succeeded):
    /// the device calls of [`find_cpu`]. Built only in [`start`].
    struct Platform {
        lib: HMODULE,
        platform: *mut c_void,
        /// Set by `manager`.
        manager: *mut c_void,
    }

    // SAFETY (every method): `platform` is the started `IPlatform`,
    // `manager` its `IDeviceManager` once `manager` gave one (`find_cpu`
    // asks for devices only then), and a device is one `GetDevice` returned
    // non-null; each slot follows the interface in the module comment.
    impl Devices for Platform {
        type Device = *mut c_void;

        fn manager(&mut self) -> bool {
            unsafe {
                let get: GetManager = std::mem::transmute(slot(self.platform, PLATFORM_DEVICE_MANAGER));
                self.manager = get(self.platform);
            }
            !self.manager.is_null()
        }

        fn device(&mut self, index: usize) -> Option<*mut c_void> {
            let device = unsafe {
                let get: GetDevice = std::mem::transmute(slot(self.manager, index));
                get(self.manager, DT_CPU, 0)
            };
            (!device.is_null()).then_some(device)
        }

        fn device_type(&mut self, device: *mut c_void) -> i32 {
            unsafe {
                let get_type: GetType = std::mem::transmute(slot(device, DEVICE_GET_TYPE));
                get_type(device)
            }
        }

        fn core_count(&mut self, device: *mut c_void) -> (i32, u32) {
            let mut n = 0u32;
            let status = unsafe {
                let cores: GetCoreCount = std::mem::transmute(slot(device, CPU_GET_CORE_COUNT));
                cores(device, &mut n)
            };
            (status, n)
        }

        fn uninit(&mut self) {
            unsafe {
                let uninit: UnInit = std::mem::transmute(slot(self.platform, PLATFORM_UNINIT));
                uninit(self.platform);
            }
        }

        fn unload(&mut self) {
            unsafe { FreeLibrary(self.lib) };
        }
    }

    /// A lease on the SDK's session, started from its folder when nobody
    /// holds one, or the reason it cannot start.
    pub fn load(folder: &Path) -> Result<Box<dyn Sdk>, String> {
        SESSION.acquire(|| start(folder)).map(|lease| Box::new(lease) as Box<dyn Sdk>)
    }

    /// Loads and starts the SDK from its folder, or the reason it cannot.
    fn start(folder: &Path) -> Result<Box<dyn Sdk + Send>, String> {
        let path = wide(platform_dll(folder).as_os_str());
        // SAFETY: a NUL-terminated path; Device.dll resolves from its folder.
        let lib = unsafe { LoadLibraryExW(path.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS) };
        if lib.is_null() {
            return Err(failure_reason(Failure::NotLoaded).into());
        }
        // SAFETY: `lib` is loaded; every call follows the interface above
        // and every failure unloads it before returning.
        let platform = unsafe {
            let Some(get_platform) = GetProcAddress(lib, c"GetPlatform".as_ptr() as *const u8) else {
                FreeLibrary(lib);
                return Err(failure_reason(Failure::NoEntryPoint).into());
            };
            let get_platform: GetPlatform = std::mem::transmute(get_platform);
            let platform = get_platform();
            if platform.is_null() {
                FreeLibrary(lib);
                return Err(failure_reason(Failure::InitRefused).into());
            }
            let init: PlatformInit = std::mem::transmute(slot(platform, PLATFORM_INIT));
            if init(platform, std::ptr::null(), 0) == 0 {
                FreeLibrary(lib);
                return Err(failure_reason(Failure::InitRefused).into());
            }
            platform
        };
        let mut started = Platform { lib, platform, manager: std::ptr::null_mut() };
        let cpu = find_cpu(&mut started).map_err(|f| failure_reason(f).to_string())?;
        Ok(Box::new(Loaded { lib, platform, cpu }))
    }

    /// A file's version resource (a.b.c.d).
    pub fn file_version(path: &Path) -> Option<[u16; 4]> {
        use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
        let w = wide(path.as_os_str());
        // SAFETY: NUL-terminated strings; the buffer is sized by the API and
        // the fixed info is read only when at least 16 bytes long.
        unsafe {
            let size = GetFileVersionInfoSizeW(w.as_ptr(), std::ptr::null_mut());
            if size == 0 {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
                return None;
            }
            let root: Vec<u16> = "\\\0".encode_utf16().collect();
            let (mut p, mut len): (*mut c_void, u32) = (std::ptr::null_mut(), 0);
            if VerQueryValueW(data.as_ptr() as *const c_void, root.as_ptr(), &mut p, &mut len) == 0 || len < 16 {
                return None;
            }
            let f = p as *const u32;
            let (ms, ls) = (*f.add(2), *f.add(3));
            Some([(ms >> 16) as u16, ms as u16, (ls >> 16) as u16, ls as u16])
        }
    }

    /// Whether a DLL of the SDK is mapped in this process (tests).
    #[cfg(test)]
    pub fn loaded() -> bool {
        ["Platform.dll", "Device.dll"].iter().any(|name| {
            let w: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated name; no reference is taken.
            !unsafe { windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(w.as_ptr()) }.is_null()
        })
    }
}

/// No calls into `Platform.dll` on a Windows target other than x86_64,
/// where the `extern "C"` calls of its C++ virtual methods would not match
/// the SDK's calling convention.
#[cfg(not(target_arch = "x86_64"))]
mod sdk {
    use crate::sensors::ryzen::{Sdk, SDK_UNSUPPORTED};
    pub fn load(_folder: &std::path::Path) -> Result<Box<dyn Sdk>, String> {
        Err(SDK_UNSUPPORTED.into())
    }
    pub fn file_version(_path: &std::path::Path) -> Option<[u16; 4]> {
        None
    }
    #[cfg(test)]
    pub fn loaded() -> bool {
        false
    }
}
