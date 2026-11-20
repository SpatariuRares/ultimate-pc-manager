//! Windows-only lists and actions: services, startup apps,
//! process priority. Other systems get empty lists and a clear message.

#[derive(Clone, Debug, Default)]
pub struct ServiceRow {
    pub name: String,
    pub display: String,
    pub pid: u32,
    pub status: String,
    pub start_type: String,
}

/// A running Win32 service and the process that hosts it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunningService {
    pub pid: u32,
    pub name: String,
    pub display: String,
    /// It shares its process with other services (else it has its own).
    pub shared: bool,
}

#[derive(Clone, Debug, Default)]
pub struct StartupRow {
    pub name: String,
    pub command: String,
    pub location: String,
    pub enabled: bool,
    /// Where the enabled/disabled flag lives: (hive is HKCU, approved subkey).
    pub hkcu: bool,
    pub approved_key: String,
}

/// Priority choices, in the order the "Set priority" menu lists them.
pub const PRIORITIES: [&str; 6] = ["Realtime", "High", "Above normal", "Normal", "Below normal", "Low"];

/// A process's processor affinity as Set affinity reads it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Affinity {
    /// The process's mask and the system's: one bit per logical processor.
    Mask { process: u64, system: u64 },
    /// The system has several processor groups: one mask cannot say it.
    ManyGroups,
}

/// Said when Windows refuses a new processor affinity.
pub const AFFINITY_FAILED: &str = "Windows could not change the processor affinity.";

pub fn open_file_location(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("The file location of this process can't be read.".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("explorer")
            .arg(format!("/select,{}", path))
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
        std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// `s` as a NUL-terminated UTF-16 string, for the wide Windows calls.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `path` as a NUL-terminated UTF-16 string, for the wide Windows calls.
#[cfg(windows)]
pub fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// `path` as handed to Windows: with the `\\?\` long-path prefix
/// (`\\?\UNC\` for a share) when it is too long for the plain form, else
/// unchanged. The prefix turns Windows' own normalisation off, so the path
/// is normalised first (`/` to `\`, `.` and `..` resolved); a relative
/// path cannot take it and is left as it is. The length is counted in
/// UTF-16 units, as Windows counts it. Paths are kept and shown
/// unprefixed; every place that hands one to Windows goes through this,
/// and nothing else builds the prefix.
pub fn long_path(path: &std::path::Path) -> std::path::PathBuf {
    /// Below MAX_PATH less room for an 8.3 name, as folders need.
    const PLAIN_MAX: usize = 240;
    let units = |p: &std::path::Path| p.as_os_str().to_string_lossy().encode_utf16().count();
    if !cfg!(windows) || !path.is_absolute() || units(path) < PLAIN_MAX || path.as_os_str().to_string_lossy().starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = full.as_os_str().to_string_lossy();
    if units(&full) < PLAIN_MAX || s.starts_with(r"\\?\") {
        full
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        std::path::PathBuf::from(format!(r"\\?\UNC\{unc}"))
    } else {
        std::path::PathBuf::from(format!(r"\\?\{s}"))
    }
}

/// A Windows handle from a successful open, closed once when dropped.
#[cfg(windows)]
pub struct Handle(pub windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open (never
        // INVALID_HANDLE_VALUE) and is owned by this value alone, so it is
        // closed exactly once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// Opens the folder holding `path` in File Explorer with the item
/// selected, through the shell (SHOpenFolderAndSelectItems).
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
        use windows_sys::Win32::UI::Shell::{ILFree, SHOpenFolderAndSelectItems};
        // SAFETY: the item list is freed once; COM is uninitialised only if
        // this call initialised it (a thread already in another apartment
        // keeps it).
        unsafe {
            let joined = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
            let item = shell_item(path);
            let hr = if item.is_null() { -1 } else { SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0) };
            if !item.is_null() {
                ILFree(item);
            }
            if joined {
                CoUninitialize();
            }
            if hr >= 0 {
                Ok(())
            } else {
                Err(format!("The shell could not show {} (0x{hr:08x}).", path.display()))
            }
        }
    }
    #[cfg(not(windows))]
    {
        open_file_location(&path.display().to_string())
    }
}

/// The shell's item list for `path` (null when the shell cannot resolve
/// it); the caller frees it with ILFree. The shell refuses the `\\?\`
/// form (E_INVALIDARG, measured), so it gets the plain path, which it
/// resolves past 260 characters when long paths are enabled; failing
/// that, the 8.3 short form, read through [`long_path`], which is shorter.
#[cfg(windows)]
fn shell_item(path: &std::path::Path) -> *mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows_sys::Win32::UI::Shell::ILCreateFromPathW;
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let item = unsafe { ILCreateFromPathW(wide.as_ptr()) };
    if !item.is_null() {
        return item;
    }
    let long = wide_path(&long_path(path));
    let mut short = vec![0u16; 32_768];
    // SAFETY: `long` is NUL-terminated; the system writes at most the
    // given length of UTF-16 units into `short`, which holds exactly that.
    let n = unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), short.len() as u32) } as usize;
    if n == 0 || n >= short.len() {
        return std::ptr::null_mut();
    }
    short.truncate(n);
    // The short form keeps the prefix it was asked with; the shell takes it plain.
    let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let (long_prefix, unc, share) = (units(r"\\?\"), units(r"UNC\"), units(r"\\"));
    let plain = match short.strip_prefix(&long_prefix[..]) {
        Some(rest) if rest.starts_with(&unc) => [&share[..], &rest[unc.len()..]].concat(),
        Some(rest) => rest.to_vec(),
        None => short,
    };
    let plain: Vec<u16> = plain.into_iter().chain(std::iter::once(0)).collect();
    // SAFETY: `plain` is NUL-terminated and outlives the call.
    unsafe { ILCreateFromPathW(plain.as_ptr()) }
}

/// Said when Windows refuses Create memory dump file.
pub const DUMP_FAILED: &str = "Windows could not write the memory dump file.";

/// Said when Windows refuses UAC virtualization.
pub const UAC_FAILED: &str = "Windows could not change UAC virtualization.";

/// The dump files being written now, so two dumps written at once never
/// share a file.
static DUMPS_WRITING: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// A dump file's path, held for one dump while it is written.
pub struct DumpClaim(pub std::path::PathBuf);

impl Drop for DumpClaim {
    fn drop(&mut self) {
        crate::platform::lock_state(&DUMPS_WRITING).retain(|p| *p != self.0);
    }
}

/// The file a dump of `pid` is written to: `TaskManager-<stem>-<pid>.dmp`
/// in `dir`, or, while another dump writes that one,
/// `TaskManager-<stem>-<pid>-2.dmp`, `-3` and so on.
pub fn claim_dump_path(dir: &std::path::Path, stem: &str, pid: u32) -> DumpClaim {
    let mut writing = crate::platform::lock_state(&DUMPS_WRITING);
    let path = (1..)
        .map(|n| dir.join(if n == 1 { format!("TaskManager-{stem}-{pid}.dmp") } else { format!("TaskManager-{stem}-{pid}-{n}.dmp") }))
        .find(|p| !writing.contains(p))
        .expect("a free name");
    writing.push(path.clone());
    DumpClaim(path)
}

/// Create memory dump file: a full dump of the process `pid` written to
/// `dir` as `TaskManager-<name>-<pid>.dmp` (`name` without its
/// extension; see [`claim_dump_path`] for a dump of the same process
/// still being written). The path written, or [`DUMP_FAILED`] with no file
/// left.
pub fn write_dump(pid: u32, name: &str, dir: &std::path::Path) -> Result<std::path::PathBuf, String> {
    let stem = std::path::Path::new(name).file_stem().map_or_else(|| name.to_string(), |s| s.to_string_lossy().into_owned());
    let claim = claim_dump_path(dir, &stem, pid);
    let path = claim.0.clone();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Foundation::HANDLE;
        use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_DUP_HANDLE, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
        #[link(name = "dbghelp")]
        extern "system" {
            // Declared here rather than through windows-sys, whose
            // Diagnostics_Debug feature this crate does not turn on. The
            // three null pointers are the optional exception, user stream
            // and callback information.
            fn MiniDumpWriteDump(process: HANDLE, pid: u32, file: HANDLE, kind: u32, exception: *const std::ffi::c_void, user: *const std::ffi::c_void, callback: *const std::ffi::c_void) -> i32;
        }
        // Full memory with handles, unloaded modules, memory regions and
        // threads, as a dump for a developer needs.
        const FULL: u32 = 0x2 | 0x4 | 0x20 | 0x800 | 0x1000;
        // SAFETY: the process handle is checked and closed by `Handle`;
        // the file handle belongs to `file`, alive for the whole call;
        // the optional pointers are null.
        let written = unsafe {
            let h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ | PROCESS_DUP_HANDLE, 0, pid);
            if h.is_null() {
                return Err(DUMP_FAILED.into());
            }
            let h = Handle(h);
            let file = std::fs::File::create(&path).map_err(|_| DUMP_FAILED.to_string())?;
            let ok = MiniDumpWriteDump(h.0, pid, file.as_raw_handle() as HANDLE, FULL, std::ptr::null(), std::ptr::null(), std::ptr::null()) != 0;
            drop(file);
            ok
        };
        if !written {
            let _ = std::fs::remove_file(&path);
            return Err(DUMP_FAILED.into());
        }
        Ok(path)
    }
    #[cfg(not(windows))]
    {
        let _ = path;
        Err(DUMP_FAILED.into())
    }
}

/// UAC virtualization: the process `pid`'s token has it switched the other
/// way (TokenVirtualizationEnabled); [`UAC_FAILED`] when Windows refuses.
pub fn toggle_uac_virtualization(pid: u32) -> Result<(), String> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Security::{GetTokenInformation, SetTokenInformation, TokenVirtualizationEnabled, TOKEN_ADJUST_DEFAULT, TOKEN_QUERY};
        use windows_sys::Win32::System::Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
        // SAFETY: both handles are checked and closed by `Handle`; the
        // value read and written is the u32 TokenVirtualizationEnabled
        // takes, passed with its own size.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return Err(UAC_FAILED.into());
            }
            let h = Handle(h);
            let mut token = std::ptr::null_mut();
            if OpenProcessToken(h.0, TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, &mut token) == 0 {
                return Err(UAC_FAILED.into());
            }
            let token = Handle(token);
            let size = std::mem::size_of::<u32>() as u32;
            let (mut on, mut got) = (0u32, 0u32);
            if GetTokenInformation(token.0, TokenVirtualizationEnabled, (&mut on as *mut u32).cast(), size, &mut got) == 0 {
                return Err(UAC_FAILED.into());
            }
            let flipped = u32::from(on == 0);
            if SetTokenInformation(token.0, TokenVirtualizationEnabled, (&flipped as *const u32).cast(), size) == 0 {
                return Err(UAC_FAILED.into());
            }
            Ok(())
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        Err(UAC_FAILED.into())
    }
}

/// Opens Windows' Power Options control panel.
pub fn open_power_options() -> Result<(), String> {
    #[cfg(windows)]
    {
        std::process::Command::new("control").arg("powercfg.cpl").spawn().map(|_| ()).map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        Err("Power options are a Windows panel.".into())
    }
}

/// End a process: politely ("End task"), forced, and/or with its children.
pub fn end_process(pid: u32, force: bool, tree: bool) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut c = std::process::Command::new("taskkill");
        c.arg("/PID").arg(pid.to_string());
        if force {
            c.arg("/F");
        }
        if tree {
            c.arg("/T");
        }
        let out = c.creation_flags(0x0800_0000).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(())
        } else {
            let msg = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if msg.is_empty() { "The process could not be ended.".into() } else { msg })
        }
    }
    #[cfg(not(windows))]
    {
        let _ = tree;
        let sig = if force { "-9" } else { "-15" };
        std::process::Command::new("kill").args([sig, &pid.to_string()]).status().map(|_| ()).map_err(|e| e.to_string())
    }
}

/// The PID of the shell's Windows Explorer: the process that owns
/// `GetShellWindow` (none while no shell runs).
pub fn shell_pid() -> Option<u32> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
        // SAFETY: both calls only read; a null window yields no PID.
        unsafe {
            let hwnd = GetShellWindow();
            if hwnd.is_null() {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            (pid != 0).then_some(pid)
        }
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Restart on the shell's Windows Explorer (`pid`): ends it, then starts
/// explorer.exe ([`explorer_path`]) unless Windows has already brought the
/// shell back. Waits up to a few seconds each way (so the app calls it off
/// the UI thread); the error is the status bar's words.
pub fn restart_explorer(pid: u32) -> Result<(), String> {
    use crate::platform::{EXPLORER_NOT_ENDED, EXPLORER_NOT_RESTARTED};
    end_process(pid, true, false).map_err(|e| format!("{EXPLORER_NOT_ENDED} {e}"))?;
    // A new shell window, from another process than the one ended.
    let back = |tries: u32| {
        (0..tries).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            shell_pid().is_some_and(|now| now != pid)
        })
    };
    // Windows restarts its shell by itself (AutoRestartShell); starting a
    // second explorer.exe then would only open a folder window.
    if back(30) {
        return Ok(());
    }
    std::process::Command::new(explorer_path(std::env::var_os("WINDIR").as_deref())).spawn().map_err(|_| EXPLORER_NOT_RESTARTED.to_string())?;
    if back(50) {
        Ok(())
    } else {
        Err(EXPLORER_NOT_RESTARTED.into())
    }
}

/// explorer.exe by its full path, in the Windows folder (%WINDIR%),
/// so no other explorer.exe found on the search path is started.
pub fn explorer_path(windir: Option<&std::ffi::OsStr>) -> std::path::PathBuf {
    let dir = windir.filter(|d| !d.is_empty()).map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from);
    dir.join("explorer.exe")
}

pub fn run_new_task(cmdline: &str) -> Result<(), String> {
    let cmdline = cmdline.trim();
    if cmdline.is_empty() {
        return Err("Type the name of a program, folder or document.".into());
    }
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", ""])
            .arg(cmdline)
            .spawn()
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
    #[cfg(not(windows))]
    {
        std::process::Command::new("sh").args(["-c", cmdline]).spawn().map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Services::*;
    use windows_sys::Win32::System::Threading::*;
    use winreg::enums::*;
    use winreg::RegKey;

    /// A NUL-terminated UTF-16 string from Windows (empty for null).
    ///
    /// # Safety
    /// `p` is null or points to a readable NUL-terminated UTF-16 string.
    pub unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        // SAFETY: the caller guarantees a NUL-terminated string, so every
        // unit up to and including the NUL is readable.
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }

    /// Whether the Windows service `name` runs; `None` when it is not
    /// installed (or the service manager cannot be opened).
    pub fn service_running(name: &str) -> Option<bool> {
        let w = wide(name);
        // SAFETY: both handles are checked before use and closed; `w` is a
        // NUL-terminated name that outlives the call; the status is an
        // out-param of the right type.
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return None;
            }
            let h = OpenServiceW(scm, w.as_ptr(), SERVICE_QUERY_STATUS);
            let out = if h.is_null() {
                None
            } else {
                let mut s: SERVICE_STATUS = std::mem::zeroed();
                let running = QueryServiceStatus(h, &mut s) != 0 && s.dwCurrentState == SERVICE_RUNNING;
                CloseServiceHandle(h);
                Some(running)
            };
            CloseServiceHandle(scm);
            out
        }
    }

    fn state_text(s: u32) -> &'static str {
        match s {
            SERVICE_RUNNING => "Running",
            SERVICE_STOPPED => "Stopped",
            SERVICE_START_PENDING => "Starting",
            SERVICE_STOP_PENDING => "Stopping",
            SERVICE_PAUSED => "Paused",
            SERVICE_PAUSE_PENDING => "Pausing",
            SERVICE_CONTINUE_PENDING => "Resuming",
            _ => "Unknown",
        }
    }

    fn start_text(t: u32) -> &'static str {
        match t {
            SERVICE_AUTO_START => "Automatic",
            SERVICE_DEMAND_START => "Manual",
            SERVICE_DISABLED => "Disabled",
            SERVICE_BOOT_START => "Boot",
            SERVICE_SYSTEM_START => "System",
            _ => "",
        }
    }

    /// (PID, service name) of every running Win32 service: one service
    /// manager call, without the per-service configuration `list_services`
    /// reads.
    pub fn running_service_pids() -> Vec<(u32, String)> {
        running_services().into_iter().map(|s| (s.pid, s.name)).collect()
    }

    /// Every running Win32 service with its host's PID, display name and
    /// whether it shares its process: one service manager call.
    pub fn running_services() -> Vec<RunningService> {
        // SAFETY: the manager handle is checked before use and closed once;
        // the buffer is u64s, so aligned for ENUM_SERVICE_STATUS_PROCESSW,
        // its byte length is passed, and it outlives the `count` entries the
        // call wrote at its start, whose name pointers point into it and are
        // NUL-terminated (read by `from_wide` before the buffer is freed).
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Vec::new();
            }
            let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
            EnumServicesStatusExW(
                scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE,
                null_mut(), 0, &mut needed, &mut count, &mut resume, null(),
            );
            let mut buf = vec![0u64; (needed as usize + 4096).div_ceil(8)];
            resume = 0;
            let ok = EnumServicesStatusExW(
                scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_ACTIVE,
                buf.as_mut_ptr().cast(), (buf.len() * 8) as u32, &mut needed, &mut count, &mut resume, null(),
            );
            let out = if ok == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize)
                    .iter()
                    .map(|it| RunningService {
                        pid: it.ServiceStatusProcess.dwProcessId,
                        name: from_wide(it.lpServiceName),
                        display: from_wide(it.lpDisplayName),
                        shared: it.ServiceStatusProcess.dwServiceType & SERVICE_WIN32_SHARE_PROCESS != 0,
                    })
                    .collect()
            };
            CloseServiceHandle(scm);
            out
        }
    }

    /// A service's description as the Services console shows it (empty
    /// when it has none or cannot be read).
    pub fn service_description(name: &str) -> String {
        let w = wide(name);
        // SAFETY: the handles are checked and closed; the buffer holds the
        // SERVICE_DESCRIPTIONW the call writes, whose text points into it.
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return String::new();
            }
            let h = OpenServiceW(scm, w.as_ptr(), SERVICE_QUERY_CONFIG);
            let mut text = String::new();
            if !h.is_null() {
                let mut buf = vec![0u8; 8192];
                let mut need = 0u32;
                if QueryServiceConfig2W(h, SERVICE_CONFIG_DESCRIPTION, buf.as_mut_ptr(), buf.len() as u32, &mut need) != 0 {
                    let d = &*(buf.as_ptr() as *const SERVICE_DESCRIPTIONW);
                    if !d.lpDescription.is_null() {
                        text = from_wide(d.lpDescription);
                    }
                }
                CloseServiceHandle(h);
            }
            CloseServiceHandle(scm);
            text
        }
    }

    /// The FileDescription of an executable's version resource (e.g.
    /// "Microsoft Edge"); None when it has none.
    pub fn file_description(path: &str) -> Option<String> {
        use std::ffi::c_void;
        use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
        let w = wide(path);
        // SAFETY: every pointer handed to the version API points into `data`
        // or to NUL-terminated wide strings that outlive the calls.
        unsafe {
            let mut handle = 0u32;
            let size = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
            if size == 0 {
                return None;
            }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
                return None;
            }
            let (mut p, mut len): (*mut c_void, u32) = (null_mut(), 0);
            let mut langs: Vec<String> = Vec::new();
            let tr = wide("\\VarFileInfo\\Translation");
            if VerQueryValueW(data.as_ptr() as *const c_void, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
                let t = p as *const u16;
                langs.push(format!("{:04x}{:04x}", *t, *t.add(1)));
            }
            langs.extend(["040904b0".to_string(), "040904e4".to_string(), "000004b0".to_string()]);
            for l in langs {
                let key = wide(&format!("\\StringFileInfo\\{l}\\FileDescription"));
                if VerQueryValueW(data.as_ptr() as *const c_void, key.as_ptr(), &mut p, &mut len) != 0 && len > 0 {
                    let text = from_wide(p as *const u16).trim().to_string();
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
            None
        }
    }

    pub fn list_services() -> Result<Vec<ServiceRow>, String> {
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err("Can't open the service list.".into());
            }
            let mut needed = 0u32;
            let mut count = 0u32;
            let mut resume = 0u32;
            EnumServicesStatusExW(
                scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL,
                null_mut(), 0, &mut needed, &mut count, &mut resume, null(),
            );
            let mut buf = vec![0u8; needed as usize + 4096];
            resume = 0;
            let ok = EnumServicesStatusExW(
                scm, SC_ENUM_PROCESS_INFO, SERVICE_WIN32, SERVICE_STATE_ALL,
                buf.as_mut_ptr(), buf.len() as u32, &mut needed, &mut count, &mut resume, null(),
            );
            if ok == 0 {
                CloseServiceHandle(scm);
                return Err("Can't read the service list.".into());
            }
            let items = std::slice::from_raw_parts(
                buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW,
                count as usize,
            );
            let mut out = Vec::with_capacity(items.len());
            let mut cfgbuf = vec![0u8; 8192];
            for it in items {
                let name = from_wide(it.lpServiceName);
                let mut start_type = String::new();
                let h = OpenServiceW(scm, it.lpServiceName, SERVICE_QUERY_CONFIG);
                if !h.is_null() {
                    let mut need = 0u32;
                    if QueryServiceConfigW(
                        h, cfgbuf.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW,
                        cfgbuf.len() as u32, &mut need,
                    ) != 0
                    {
                        let cfg = &*(cfgbuf.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                        start_type = start_text(cfg.dwStartType).to_string();
                    }
                    CloseServiceHandle(h);
                }
                out.push(ServiceRow {
                    name,
                    display: from_wide(it.lpDisplayName),
                    pid: it.ServiceStatusProcess.dwProcessId,
                    status: state_text(it.ServiceStatusProcess.dwCurrentState).to_string(),
                    start_type,
                });
            }
            CloseServiceHandle(scm);
            out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            Ok(out)
        }
    }

    pub fn control_service(name: &str, action: &str) -> Result<(), String> {
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err("Can't open the service list.".into());
            }
            let w = wide(name);
            let h = OpenServiceW(scm, w.as_ptr(), SERVICE_START | SERVICE_STOP | SERVICE_QUERY_STATUS);
            if h.is_null() {
                CloseServiceHandle(scm);
                return Err(format!("Access denied for \"{}\". Run the app as administrator to change services.", name));
            }
            let mut st: SERVICE_STATUS = std::mem::zeroed();
            let mut stop = |h| ControlService(h, SERVICE_CONTROL_STOP, &mut st) != 0;
            let ok = match action {
                "start" => StartServiceW(h, 0, null()) != 0,
                "stop" => stop(h),
                _ => {
                    let _ = stop(h);
                    // Wait a little for the stop to finish before starting again.
                    for _ in 0..40 {
                        let mut s2: SERVICE_STATUS = std::mem::zeroed();
                        if QueryServiceStatus(h, &mut s2) != 0 && s2.dwCurrentState == SERVICE_STOPPED {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(100));
                    }
                    StartServiceW(h, 0, null()) != 0
                }
            };
            let err = std::io::Error::last_os_error();
            CloseServiceHandle(h);
            CloseServiceHandle(scm);
            if ok { Ok(()) } else { Err(format!("\"{}\": {}", name, err)) }
        }
    }

    pub fn set_priority(pid: u32, which: &str) -> Result<(), String> {
        let class = match which {
            "Realtime" => REALTIME_PRIORITY_CLASS,
            "High" => HIGH_PRIORITY_CLASS,
            "Above normal" => ABOVE_NORMAL_PRIORITY_CLASS,
            "Normal" => NORMAL_PRIORITY_CLASS,
            "Below normal" => BELOW_NORMAL_PRIORITY_CLASS,
            _ => IDLE_PRIORITY_CLASS,
        };
        unsafe {
            let h = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
            if h.is_null() {
                return Err("Access denied: this process can't be changed.".into());
            }
            let ok = SetPriorityClass(h, class) != 0;
            CloseHandle(h);
            if ok { Ok(()) } else { Err(std::io::Error::last_os_error().to_string()) }
        }
    }

    pub fn get_priority(pid: u32) -> Option<&'static str> {
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let c = GetPriorityClass(h);
            CloseHandle(h);
            Some(match c {
                REALTIME_PRIORITY_CLASS => "Realtime",
                HIGH_PRIORITY_CLASS => "High",
                ABOVE_NORMAL_PRIORITY_CLASS => "Above normal",
                NORMAL_PRIORITY_CLASS => "Normal",
                BELOW_NORMAL_PRIORITY_CLASS => "Below normal",
                IDLE_PRIORITY_CLASS => "Low",
                _ => return None,
            })
        }
    }

    /// The processor affinity of the process `pid` (GetProcessAffinityMask),
    /// or [`Affinity::ManyGroups`] on a system with several processor
    /// groups; None when the process cannot be opened.
    pub fn get_affinity(pid: u32) -> Option<Affinity> {
        // SAFETY: the handle is checked and closed by `Handle`; both masks
        // are written by GetProcessAffinityMask.
        unsafe {
            if GetActiveProcessorGroupCount() > 1 {
                return Some(Affinity::ManyGroups);
            }
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let h = Handle(h);
            let (mut process, mut system) = (0usize, 0usize);
            if GetProcessAffinityMask(h.0, &mut process, &mut system) == 0 {
                return None;
            }
            Some(Affinity::Mask { process: process as u64, system: system as u64 })
        }
    }

    /// Set affinity: the process `pid` may run only on the logical
    /// processors of `mask` (SetProcessAffinityMask).
    pub fn set_affinity(pid: u32, mask: u64) -> Result<(), String> {
        // SAFETY: the handle is checked and closed by `Handle`.
        unsafe {
            let h = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
            if h.is_null() {
                return Err(AFFINITY_FAILED.into());
            }
            let h = Handle(h);
            if SetProcessAffinityMask(h.0, mask as usize) == 0 {
                return Err(AFFINITY_FAILED.into());
            }
            Ok(())
        }
    }

    /// The shape of ntdll's NtSuspendProcess and NtResumeProcess.
    type SuspendCall = unsafe extern "system" fn(windows_sys::Win32::Foundation::HANDLE) -> i32;

    /// Suspends (`suspend`) or resumes every thread of the process `pid`
    /// through ntdll's NtSuspendProcess / NtResumeProcess, looked up at
    /// run time (they are not in the import libraries).
    fn suspend_or_resume(pid: u32, suspend: bool) -> Result<(), String> {
        use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
        let name: &std::ffi::CStr = if suspend { c"NtSuspendProcess" } else { c"NtResumeProcess" };
        let lib = wide("ntdll.dll");
        // SAFETY: `lib` and `name` are NUL-terminated; ntdll is loaded in
        // every process and both exports have the `SuspendCall` shape; the
        // process handle is checked and closed by `Handle`.
        unsafe {
            let module = GetModuleHandleW(lib.as_ptr());
            if module.is_null() {
                return Err("Windows did not provide the call to suspend processes.".into());
            }
            let call: SuspendCall = match GetProcAddress(module, name.as_ptr().cast()) {
                Some(f) => std::mem::transmute::<unsafe extern "system" fn() -> isize, SuspendCall>(f),
                None => return Err("Windows did not provide the call to suspend processes.".into()),
            };
            let h = OpenProcess(PROCESS_SUSPEND_RESUME, 0, pid);
            if h.is_null() {
                return Err("Access denied: this process can't be changed.".into());
            }
            let h = Handle(h);
            let status = call(h.0);
            if status >= 0 { Ok(()) } else { Err(format!("The process could not be {} (0x{status:08x}).", if suspend { "suspended" } else { "resumed" })) }
        }
    }

    /// Suspend: every thread of the process `pid` stops.
    pub fn suspend_process(pid: u32) -> Result<(), String> {
        suspend_or_resume(pid, true)
    }

    /// Resume: the threads of the suspended process `pid` run again.
    pub fn resume_process(pid: u32) -> Result<(), String> {
        suspend_or_resume(pid, false)
    }

    /// Efficiency mode: Windows throttles the process `pid`'s execution
    /// speed (ProcessPowerThrottling, EXECUTION_SPEED) and it gets the Low
    /// (idle) priority class.
    pub fn set_efficiency(pid: u32) -> Result<(), String> {
        // SAFETY: the handle is checked and closed by `Handle`; the state
        // is a PROCESS_POWER_THROTTLING_STATE passed with its own size.
        unsafe {
            let h = OpenProcess(PROCESS_SET_INFORMATION, 0, pid);
            if h.is_null() {
                return Err("Access denied: this process can't be changed.".into());
            }
            let h = Handle(h);
            let state = PROCESS_POWER_THROTTLING_STATE {
                Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
            };
            let size = std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32;
            if SetProcessInformation(h.0, ProcessPowerThrottling, (&state as *const PROCESS_POWER_THROTTLING_STATE).cast(), size) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            if SetPriorityClass(h.0, IDLE_PRIORITY_CLASS) == 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            Ok(())
        }
    }

    /// Whether Windows throttles the execution speed of the process `pid`
    /// (Efficiency mode); None when it cannot be read.
    #[cfg(test)]
    pub fn efficiency_on(pid: u32) -> Option<bool> {
        // SAFETY: the handle is checked and closed by `Handle`; the state
        // is written with its own size.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return None;
            }
            let h = Handle(h);
            let mut state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: 0, StateMask: 0 };
            let size = std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32;
            if GetProcessInformation(h.0, ProcessPowerThrottling, (&mut state as *mut PROCESS_POWER_THROTTLING_STATE).cast(), size) == 0 {
                return None;
            }
            Some(state.StateMask & PROCESS_POWER_THROTTLING_EXECUTION_SPEED != 0)
        }
    }

    const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
    const APPROVED_FOLDER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";

    fn approved(hive: &RegKey, key: &str, name: &str) -> bool {
        hive.open_subkey(key)
            .ok()
            .and_then(|k| k.get_raw_value(name).ok())
            .map(|v| v.bytes.first().map(|b| b & 1 == 0).unwrap_or(true))
            .unwrap_or(true)
    }

    pub fn list_startup() -> Vec<StartupRow> {
        let mut out = Vec::new();
        for (hkcu, hive, label) in [
            (true, RegKey::predef(HKEY_CURRENT_USER), "Registry (current user)"),
            (false, RegKey::predef(HKEY_LOCAL_MACHINE), "Registry (all users)"),
        ] {
            if let Ok(k) = hive.open_subkey(RUN) {
                for (name, value) in k.enum_values().flatten() {
                    let command = value.to_string().trim_matches('"').to_string();
                    out.push(StartupRow {
                        enabled: approved(&hive, APPROVED_RUN, &name),
                        name,
                        command,
                        location: label.into(),
                        hkcu,
                        approved_key: APPROVED_RUN.into(),
                    });
                }
            }
        }
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        if let Ok(appdata) = std::env::var("APPDATA") {
            let dir = std::path::Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
            if let Ok(rd) = std::fs::read_dir(&dir) {
                for e in rd.flatten() {
                    let file = e.file_name().to_string_lossy().into_owned();
                    if file.eq_ignore_ascii_case("desktop.ini") {
                        continue;
                    }
                    out.push(StartupRow {
                        enabled: approved(&hkcu, APPROVED_FOLDER, &file),
                        name: file.trim_end_matches(".lnk").to_string(),
                        command: e.path().display().to_string(),
                        location: "Startup folder".into(),
                        hkcu: true,
                        approved_key: APPROVED_FOLDER.into(),
                    });
                }
            }
        }
        out
    }

    pub fn set_startup_enabled(row: &StartupRow, enable: bool) -> Result<(), String> {
        let hive = if row.hkcu { RegKey::predef(HKEY_CURRENT_USER) } else { RegKey::predef(HKEY_LOCAL_MACHINE) };
        let (key, _) = hive
            .create_subkey(&row.approved_key)
            .map_err(|_| "Access denied. Run the app as administrator to change this entry.".to_string())?;
        let value_name = if row.location == "Startup folder" {
            std::path::Path::new(&row.command).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
        } else {
            row.name.clone()
        };
        let mut bytes = vec![0u8; 12];
        bytes[0] = if enable { 0x02 } else { 0x03 };
        key.set_raw_value(&value_name, &winreg::RegValue { bytes: bytes.into(), vtype: REG_BINARY })
            .map_err(|e| e.to_string())
    }

}

/// Stand-ins off Windows, so the app builds and its tests run there: each
/// reports that nothing is available instead of reading the system.
#[cfg(not(windows))]
mod imp {
    use super::*;
    /// The error every call that would change the system returns.
    const NOT: &str = "Only available on Windows.";
    /// No Service Control Manager: an error.
    pub fn list_services() -> Result<Vec<ServiceRow>, String> { Err(NOT.into()) }
    /// No running services: no (PID, service) pair.
    pub fn running_service_pids() -> Vec<(u32, String)> { Vec::new() }
    /// No running services.
    pub fn running_services() -> Vec<RunningService> { Vec::new() }
    /// No service descriptions.
    pub fn service_description(_: &str) -> String { String::new() }
    /// No version resources: no file description.
    pub fn file_description(_: &str) -> Option<String> { None }
    /// Services cannot be started or stopped: an error.
    pub fn control_service(_: &str, _: &str) -> Result<(), String> { Err(NOT.into()) }
    /// Priorities cannot be set: an error.
    pub fn set_priority(_: u32, _: &str) -> Result<(), String> { Err(NOT.into()) }
    /// No priority is known.
    pub fn get_priority(_: u32) -> Option<&'static str> { None }
    /// No affinity is known.
    pub fn get_affinity(_: u32) -> Option<Affinity> { None }
    /// Affinities cannot be set: an error.
    pub fn set_affinity(_: u32, _: u64) -> Result<(), String> { Err(NOT.into()) }
    /// Processes cannot be suspended: an error.
    pub fn suspend_process(_: u32) -> Result<(), String> { Err(NOT.into()) }
    /// Processes cannot be resumed: an error.
    pub fn resume_process(_: u32) -> Result<(), String> { Err(NOT.into()) }
    /// Efficiency mode cannot be set: an error.
    pub fn set_efficiency(_: u32) -> Result<(), String> { Err(NOT.into()) }
    /// Efficiency mode is not known.
    #[cfg(test)]
    pub fn efficiency_on(_: u32) -> Option<bool> { None }
    /// No startup entries.
    pub fn list_startup() -> Vec<StartupRow> { Vec::new() }
    /// Startup entries cannot be changed: an error.
    pub fn set_startup_enabled(_: &StartupRow, _: bool) -> Result<(), String> { Err(NOT.into()) }
}

pub use imp::*;

#[cfg(all(test, windows))]
mod tests {
    use super::*;
    use crate::proc_info::Reader;
    use crate::sampler::ProcRow;

    #[test]
    fn restart_starts_explorer_by_its_full_path_in_the_windows_folder() {
        use std::path::PathBuf;
        assert_eq!(explorer_path(Some(std::ffi::OsStr::new(r"D:\Win"))), PathBuf::from(r"D:\Win\explorer.exe"));
        assert_eq!(explorer_path(None), PathBuf::from(r"C:\Windows\explorer.exe"), "WINDIR unset");
        assert_eq!(explorer_path(Some(std::ffi::OsStr::new(""))), PathBuf::from(r"C:\Windows\explorer.exe"), "WINDIR empty");
    }

    #[test]
    fn only_a_path_too_long_for_the_plain_form_gets_the_long_path_prefix() {
        use std::path::{Path, PathBuf};
        let deep = "d".repeat(250);
        assert_eq!(long_path(Path::new(r"C:\Users\a.txt")), PathBuf::from(r"C:\Users\a.txt"));
        assert_eq!(long_path(&Path::new(r"C:\").join(&deep)), PathBuf::from(format!(r"\\?\C:\{deep}")));
        assert_eq!(long_path(&Path::new(r"\\srv\share").join(&deep)), PathBuf::from(format!(r"\\?\UNC\srv\share\{deep}")));
        let prefixed = PathBuf::from(format!(r"\\?\C:\{deep}"));
        assert_eq!(long_path(&prefixed), prefixed, "already prefixed");
    }

    #[test]
    fn the_long_path_prefix_goes_only_on_a_normalised_absolute_path() {
        use std::path::{Path, PathBuf};
        let deep = "d".repeat(250);
        // The prefix turns normalisation off, so it is done first.
        assert_eq!(long_path(Path::new(&format!("C:/Data/{deep}"))), PathBuf::from(format!(r"\\?\C:\Data\{deep}")), "slashes");
        assert_eq!(long_path(Path::new(&format!(r"C:\Data\.\skip\..\{deep}"))), PathBuf::from(format!(r"\\?\C:\Data\{deep}")), "dot folders");
        assert_eq!(long_path(Path::new(&format!(r"\\srv\share\x\..\{deep}"))), PathBuf::from(format!(r"\\?\UNC\srv\share\{deep}")), "a share");
        // A relative path cannot take the prefix.
        let relative = PathBuf::from(format!(r"Data\{deep}"));
        assert_eq!(long_path(&relative), relative, "relative");
        // The limit is in UTF-16 units, as Windows counts: 200 "é" are 400
        // bytes but 200 units.
        let accented = PathBuf::from(format!(r"C:\{}", "é".repeat(200)));
        assert_eq!(long_path(&accented), accented, "short in UTF-16 units");
        let wide = PathBuf::from(format!(r"C:\{}", "é".repeat(250)));
        assert_eq!(long_path(&wide), PathBuf::from(format!(r"\\?\{}", wide.display())), "long in UTF-16 units");
    }

    #[test]
    fn the_shell_finds_an_item_whose_path_is_longer_than_260_characters() {
        let root = std::env::temp_dir().join(format!("mtm-winsys-long-{}", std::process::id()));
        let mut dir = root.clone();
        while dir.as_os_str().len() < 300 {
            dir.push("a-rather-long-folder-name-for-the-test");
        }
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("deep.txt");
        std::fs::write(&file, b"x").unwrap();
        let item = shell_item(&file);
        let found = !item.is_null();
        if found {
            // SAFETY: a non-null item list from ILCreateFromPathW, freed once.
            unsafe { windows_sys::Win32::UI::Shell::ILFree(item) };
        }
        let _ = std::fs::remove_dir_all(&root);
        assert!(found, "the shell resolves {} characters", file.as_os_str().len());
    }

    /// A child that waits a minute without a window; killed by the test.
    fn sleeping_child() -> std::process::Child {
        use std::os::windows::process::CommandExt;
        std::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .creation_flags(0x0800_0000)
            .spawn()
            .expect("start ping")
    }

    /// The child's status once it reads `want` (or after 5 s).
    fn status_until(pid: u32, want: &str) -> String {
        let mut rows = vec![ProcRow { pid, ..Default::default() }];
        let mut reader = Reader::default();
        let start = std::time::Instant::now();
        loop {
            reader.fill(&mut rows);
            if rows[0].status == want || start.elapsed() > std::time::Duration::from_secs(5) {
                return rows[0].status.clone();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn suspend_and_resume_round_trip_on_a_child_process() {
        let mut child = sleeping_child();
        let pid = child.id();
        let suspended = suspend_process(pid).map(|_| status_until(pid, "Suspended"));
        let resumed = resume_process(pid).map(|_| status_until(pid, "Running"));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(suspended.as_deref(), Ok("Suspended"));
        assert_eq!(resumed.as_deref(), Ok("Running"));
    }

    #[test]
    fn efficiency_mode_throttles_a_child_and_gives_it_low_priority() {
        let mut child = sleeping_child();
        let pid = child.id();
        let before = efficiency_on(pid);
        let set = set_efficiency(pid);
        let (after, priority) = (efficiency_on(pid), get_priority(pid));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(before, Some(false), "a new process is not throttled");
        assert_eq!(set, Ok(()));
        assert_eq!(after, Some(true));
        assert_eq!(priority, Some("Low"));
    }

    #[test]
    fn set_affinity_lists_the_logical_cpus_and_toggles_one_on_a_child_process() {
        use crate::widgets::{menu_entries, MenuEntry, MenuState};
        let mut child = sleeping_child();
        let pid = child.id();
        let read = get_affinity(pid);
        let row = ProcRow { pid, name: "PING.EXE".into(), ..Default::default() };
        let state = MenuState { links: &[], percents: false, view: &[], priority: Some("Normal"), affinity: read, shell: None, following: None, services: None, windowed: None };
        let entries = menu_entries(&row, &[&row], &state);
        let cpus: Vec<(String, Vec<crate::app::Action>)> = entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Sub { label: "Set affinity", items } => Some(items),
                _ => None,
            })
            .into_iter()
            .flatten()
            .filter_map(|e| match e {
                MenuEntry::Item { label, actions, .. } if label.starts_with("CPU ") => Some((label.clone(), actions.clone())),
                _ => None,
            })
            .collect();
        // Clear CPU 0 (or, on one processor, try to): the item's own action.
        let sent = cpus.first().map(|(_, a)| a.clone()).unwrap_or_default();
        let applied: Vec<Result<(), String>> = sent
            .iter()
            .filter_map(|a| match a {
                crate::app::Action::Affinity(pids, mask) => Some(set_affinity(pids[0], *mask)),
                _ => None,
            })
            .collect();
        let after = get_affinity(pid);
        let _ = child.kill();
        let _ = child.wait();
        let logical = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        let Some(Affinity::Mask { process, system }) = read else { panic!("a one-group system reads a mask: {read:?}") };
        assert_eq!(process, system, "a new child runs on every processor");
        assert_eq!(cpus.len(), logical.min(64), "one item per logical processor: {cpus:?}");
        assert_eq!(cpus.iter().map(|(l, _)| l.as_str()).collect::<Vec<_>>(), (0..cpus.len()).map(|n| format!("CPU {n}")).collect::<Vec<_>>());
        if logical > 1 {
            assert_eq!(applied, [Ok(())]);
            assert_eq!(after, Some(Affinity::Mask { process: system & !1, system }), "CPU 0 cleared");
        }
    }

    /// A new directory of its own in the temp folder, outside the worktree
    /// (kept after the test so its path can be checked).
    fn scratch_dir(label: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("upm-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch dir");
        dir
    }

    #[test]
    fn a_dump_of_a_child_writes_a_non_empty_file_in_the_folder_given() {
        let dir = scratch_dir("dump");
        eprintln!("dump scratch dir: {}", dir.display());
        let worktree = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        assert!(!dir.starts_with(worktree), "outside the worktree");
        let mut child = sleeping_child();
        let pid = child.id();
        let written = write_dump(pid, "PING.EXE", &dir);
        let _ = child.kill();
        let _ = child.wait();
        let path = written.expect("the dump is written");
        assert_eq!(path, dir.join(format!("TaskManager-PING-{pid}.dmp")));
        let size = std::fs::metadata(&path).map_or(0, |m| m.len());
        assert!(size > 0, "{} is empty", path.display());
    }

    #[test]
    fn two_dumps_of_one_process_at_once_write_two_files() {
        let dir = std::path::Path::new(r"C:\dumps");
        let first = claim_dump_path(dir, "PING", 42);
        let second = claim_dump_path(dir, "PING", 42);
        let third = claim_dump_path(dir, "PING", 42);
        assert_eq!(first.0, dir.join("TaskManager-PING-42.dmp"));
        assert_eq!(second.0, dir.join("TaskManager-PING-42-2.dmp"));
        assert_eq!(third.0, dir.join("TaskManager-PING-42-3.dmp"));
        assert_eq!(claim_dump_path(dir, "PING", 43).0, dir.join("TaskManager-PING-43.dmp"), "another process");
        drop(first);
        assert_eq!(claim_dump_path(dir, "PING", 42).0, dir.join("TaskManager-PING-42.dmp"), "once written, its name is free again");
    }

    #[test]
    fn a_dump_windows_refuses_says_so_and_leaves_no_file() {
        let dir = scratch_dir("dump-refused");
        eprintln!("dump scratch dir: {}", dir.display());
        // No process has this PID (PIDs are multiples of 4).
        let refused = write_dump(3, "gone.exe", &dir);
        assert_eq!(refused, Err(DUMP_FAILED.to_string()));
        assert!(!dir.join("TaskManager-gone-3.dmp").exists(), "no file left behind");
    }

    #[test]
    fn uac_virtualization_windows_refuses_is_reported() {
        assert_eq!(toggle_uac_virtualization(3), Err("Windows could not change UAC virtualization.".to_string()));
    }

    #[test]
    fn bring_to_front_of_a_process_without_a_window_fails() {
        let mut child = sleeping_child();
        let front = crate::proc_info::bring_to_front(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(front, Err(crate::proc_info::FRONT_FAILED.to_string()));
    }
}
