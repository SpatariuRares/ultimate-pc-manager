//! Process actions on Windows.

use super::{Affinity, AFFINITY_FAILED, DUMP_FAILED, UAC_FAILED};
use crate::win_ffi::{wide, Handle};
use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::*;

/// Writes the dump of the process `pid` to `path`; a failed dump leaves no
/// file.
pub fn write_dump_file(pid: u32, path: &std::path::Path) -> Result<(), String> {
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
    Ok(())
}

/// UAC virtualization: the process `pid`'s token has it switched the other
/// way (TokenVirtualizationEnabled); [`UAC_FAILED`] when Windows refuses.
pub fn toggle_uac_virtualization(pid: u32) -> Result<(), String> {
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

/// End a process: politely ("End task"), forced, and/or with its children.
pub fn end_process(pid: u32, force: bool, tree: bool) -> Result<(), String> {
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

/// The PID of the shell's Windows Explorer: the process that owns
/// `GetShellWindow` (none while no shell runs).
pub fn shell_pid() -> Option<u32> {
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
