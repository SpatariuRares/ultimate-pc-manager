//! Services on Windows: the Service Control Manager.

use super::{RunningService, ServiceRow};
use crate::win_ffi::{from_wide, wide};
use std::ptr::{null, null_mut};
use windows_sys::Win32::System::Services::*;

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
