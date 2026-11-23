//! Windows: PowerShell, winget, start-and-wait, elevation and the common
//! file dialogs.

use super::*;
use std::collections::{HashMap, HashSet};
use std::mem::zeroed;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject, INFINITE};
use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW};
use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

use crate::winsys::wide;

/// The text of a NUL-terminated UTF-16 buffer.
fn from_wide(buf: &[u16]) -> String {
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..n])
}

// --------------------------------------------------- PowerShell

/// Runs `script` with Windows PowerShell (elevated through UAC when
/// asked) and reads the JSON it writes to -Out.
pub fn run_script(script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String> {
    if elevated {
        // The script never reaches the elevated side through a file a
        // normal process could swap, and its result comes back through
        // a folder only administrators can write: see `elevated`.
        let results = std::env::var_os("windir").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Temp");
        return super::elevated::run(&jobs_dir()?, &results, script, args, false, run_elevated);
    }
    let never = AtomicBool::new(false);
    run_in(&jobs_dir()?, script, args, |params| run_hidden(params, &never))
}

/// Like [`run_script`] without elevation; a set `stop` ends PowerShell
/// at once (its files are deleted all the same).
pub fn run_script_until(script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String> {
    run_in(&jobs_dir()?, script, args, |params| run_hidden(params, stop))
}

/// Where scripts and their results are written while they run.
fn jobs_dir() -> Result<std::path::PathBuf, String> {
    let app = crate::app_folders::cache().unwrap_or_else(|| std::env::temp_dir().join(crate::settings::DATA_FOLDER));
    let dir = app.join("jobs");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Writes `script` into `dir`, lets `run` start PowerShell with the
/// parameters, reads the JSON result and deletes both files, however
/// the run ended.
pub(super) fn run_in(dir: &std::path::Path, script: &str, args: &[(&str, &str)], run: impl FnOnce(&str) -> Result<(), String>) -> Result<serde_json::Value, String> {
    let stamp = unique_stamp();
    let ps1 = dir.join(format!("job-{stamp}.ps1"));
    let out = dir.join(format!("job-{stamp}.json"));
    // A BOM so Windows PowerShell reads the file as UTF-8.
    std::fs::write(&ps1, format!("\u{feff}{script}")).map_err(|e| e.to_string())?;
    let mut params = format!(
        "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" -Out \"{}\"",
        ps1.display(),
        out.display()
    );
    for (k, v) in args {
        params.push_str(&format!(" -{k} \"{}\"", v.replace('"', "")));
    }
    let result = run(&params).and_then(|_| {
        let text = std::fs::read_to_string(&out).map_err(|_| "PowerShell finished without a result.".to_string())?;
        serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("Unreadable result: {e}"))
    });
    let _ = std::fs::remove_file(&ps1);
    let _ = std::fs::remove_file(&out);
    result
}

/// Runs Windows PowerShell with `params`, without a window, and waits;
/// a set `stop` ends it at once.
pub(super) fn run_hidden(params: &str, stop: &AtomicBool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    if stop.load(Ordering::SeqCst) {
        return Err(crate::platform::STOPPED.into());
    }
    let mut child = std::process::Command::new("powershell.exe")
        .raw_arg(params)
        .creation_flags(0x0800_0000)
        .spawn()
        .map_err(|e| format!("Could not start PowerShell: {e}"))?;
    loop {
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Ok(());
        }
        if stop.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(crate::platform::STOPPED.into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Runs Windows PowerShell with `params` as administrator (UAC asks)
/// and waits.
fn run_elevated(params: &str) -> Result<i32, String> {
    use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;
    unsafe {
        let verb = wide("runas");
        // The full path from the system call (not an environment
        // variable), so a program planted earlier on the search path
        // is never the one that gets administrator rights.
        let mut dir = [0u16; 260];
        let n = GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) as usize;
        if n == 0 || n >= dir.len() {
            return Err("Could not find the Windows system folder.".into());
        }
        let system = String::from_utf16_lossy(&dir[..n]);
        let file = wide(&format!("{system}\\WindowsPowerShell\\v1.0\\powershell.exe"));
        let p = wide(params);
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = p.as_ptr();
        info.nShow = 0;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(if GetLastError() == 1223 {
                ELEVATION_CANCELLED.into()
            } else {
                format!("Could not start an elevated PowerShell (error {}).", GetLastError())
            });
        }
        let mut code = 0u32;
        if !info.hProcess.is_null() {
            WaitForSingleObject(info.hProcess, INFINITE);
            GetExitCodeProcess(info.hProcess, &mut code);
            CloseHandle(info.hProcess);
        }
        Ok(code as i32)
    }
}

/// Starts this executable again as administrator (UAC asks) with
/// `args`, without waiting.
pub fn restart_elevated(args: &[&str]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let params: Vec<String> = args.iter().map(|a| format!("\"{}\"", a.replace('"', ""))).collect();
    // SAFETY: SHELLEXECUTEINFOW is zeroed (valid: all-null pointers)
    // with cbSize set; the verb, file and parameter strings are
    // NUL-terminated and live until ShellExecuteExW returns; no process
    // handle is asked for (no SEE_MASK_NOCLOSEPROCESS), so none leaks.
    unsafe {
        let verb = wide("runas");
        let file = wide(&exe.to_string_lossy());
        let p = wide(&params.join(" "));
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.lpVerb = verb.as_ptr();
        info.lpFile = file.as_ptr();
        info.lpParameters = p.as_ptr();
        info.nShow = 1; // SW_SHOWNORMAL
        if ShellExecuteExW(&mut info) == 0 {
            return Err(if GetLastError() == 1223 {
                ELEVATION_CANCELLED.into()
            } else {
                format!("Could not restart as administrator (error {}).", GetLastError())
            });
        }
    }
    Ok(())
}

// ------------------------------------------------------- winget

/// Runs winget without a window; (exit code, output).
pub fn winget(args: &[&str]) -> Result<(i32, String), String> {
    winget_via_run(&SystemRunner, &jobs_dir()?, args)
}

// ------------------------------------------------- generic run

/// Runs `program` without a window, its stdout and stderr written to
/// the file `output`; passes each new line to `on_line` while it runs
/// (and the last unfinished one at the end); a set `stop` ends it.
pub fn run(program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
    use std::os::windows::process::CommandExt;
    if stop.load(Ordering::SeqCst) {
        return Ran::Stopped;
    }
    let (out, err) = match std::fs::File::create(output).and_then(|f| f.try_clone().map(|g| (f, g))) {
        Ok(files) => files,
        Err(e) => return Ran::NotStarted(format!("Could not create the output file: {e}")),
    };
    let mut child = match std::process::Command::new(program)
        .args(args)
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(out)
        .stderr(err)
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Ran::NotStarted(format!("Could not start {program}: {e}")),
    };
    let mut lines = NewLines::new(output);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                lines.follow(on_line);
                lines.finish(on_line);
                return Ran::Exited(status.code().unwrap_or(-1));
            }
            Ok(None) => {}
            Err(e) => return Ran::NotStarted(e.to_string()),
        }
        if stop.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            return Ran::Stopped;
        }
        lines.follow(on_line);
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Reads the lines a running program adds to its output file.
struct NewLines {
    file: Option<std::fs::File>,
    /// Bytes read so far.
    read: u64,
    /// The start of a line not finished yet.
    pending: Vec<u8>,
}

impl NewLines {
    fn new(path: &std::path::Path) -> Self {
        NewLines { file: std::fs::File::open(path).ok(), read: 0, pending: Vec::new() }
    }

    /// Passes every line finished since the last call to `on_line`.
    fn follow(&mut self, on_line: &mut dyn FnMut(&str)) {
        use std::io::{Read, Seek, SeekFrom};
        let Some(file) = self.file.as_mut() else { return };
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(self.read)).is_err() || file.read_to_end(&mut buf).is_err() {
            return;
        }
        self.read += buf.len() as u64;
        self.pending.extend_from_slice(&buf);
        while let Some(at) = self.pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=at).collect();
            on_line(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']));
        }
    }

    /// Passes the last line, when the output does not end with one.
    fn finish(&mut self, on_line: &mut dyn FnMut(&str)) {
        if !self.pending.is_empty() {
            on_line(String::from_utf8_lossy(&self.pending).trim_end_matches('\r'));
            self.pending.clear();
        }
    }
}

// ---------------------------------------------- start and wait

/// Every running process id with its parent's.
fn process_parents() -> HashMap<u32, u32> {
    let mut map = HashMap::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return map;
        }
        let mut e: PROCESSENTRY32W = zeroed();
        e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        if Process32FirstW(snap, &mut e) != 0 {
            loop {
                map.insert(e.th32ProcessID, e.th32ParentProcessID);
                if Process32NextW(snap, &mut e) == 0 {
                    break;
                }
            }
        }
        CloseHandle(snap);
    }
    map
}

/// Starts a program (Windows asks for permission when it needs it) and
/// waits for it and every process it starts: uninstallers often copy
/// themselves to a temporary folder and exit at once. Errors may hold
/// "%1" for the caller to replace with the app's name.
pub fn run_and_wait(file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String> {
    let before: HashSet<u32> = process_parents().keys().copied().collect();
    let (wfile, wparams) = (wide(file), wide(params));
    unsafe {
        let mut info: SHELLEXECUTEINFOW = zeroed();
        info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
        info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
        info.lpFile = wfile.as_ptr();
        info.lpParameters = wparams.as_ptr();
        info.nShow = 1;
        if ShellExecuteExW(&mut info) == 0 {
            return Err(if GetLastError() == 1223 {
                "%1 was not uninstalled: the administrator prompt was dismissed.".into()
            } else {
                format!("Windows could not start the uninstaller for %1 (error {}).", GetLastError())
            });
        }
        if info.hProcess.is_null() {
            return Ok(Waited::Finished(None));
        }
        let pid = GetProcessId(info.hProcess);
        // The handle keeps the pid from being reused while we watch.
        let h = info.hProcess;
        let started = Instant::now();
        let mut tracked: HashSet<u32> = HashSet::from([pid]);
        let mut root_done = false;
        loop {
            if stop.load(Ordering::Relaxed) {
                CloseHandle(h);
                return Ok(Waited::Stopped);
            }
            if started.elapsed() > Duration::from_secs(3600) {
                CloseHandle(h);
                return Ok(Waited::TimedOut);
            }
            if !root_done && WaitForSingleObject(h, 0) == WAIT_OBJECT_0 {
                root_done = true;
            }
            let parents = process_parents();
            // Adopt new children of what we watch.
            let mut grew = true;
            while grew {
                grew = false;
                for (&p, &parent) in &parents {
                    if !tracked.contains(&p) && !before.contains(&p) && tracked.contains(&parent) {
                        tracked.insert(p);
                        grew = true;
                    }
                }
            }
            tracked.retain(|p| parents.contains_key(p) && !(root_done && *p == pid));
            if root_done && tracked.is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        Ok(Waited::Finished(read.then_some(code as i32)))
    }
}

// ------------------------------------------------------ dialogs

/// A Save (or Open) dialog with a Win32 filter, the extension a saved
/// name gets when it has none, a suggested name (or path) and the
/// folder it starts in ("" for Windows' choice); the chosen path, or
/// None when cancelled.
pub fn file_dialog(save: bool, filter: &str, extension: &str, default_name: &str, folder: &str) -> Option<String> {
    unsafe {
        let filter: Vec<u16> = filter.encode_utf16().collect();
        let mut file = vec![0u16; 1024];
        for (i, c) in default_name.encode_utf16().take(1000).enumerate() {
            file[i] = c;
        }
        let ext = wide(extension);
        let folder = wide(folder);
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        if folder.len() > 1 {
            ofn.lpstrInitialDir = folder.as_ptr();
        }
        // OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST, plus FILEMUSTEXIST or OVERWRITEPROMPT
        ofn.Flags = 0x8 | 0x800 | if save { 0x2 } else { 0x1000 };
        if save {
            ofn.lpstrDefExt = ext.as_ptr();
        }
        let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
        (ok != 0).then(|| from_wide(&file))
    }
}

/// The shell's folder picker titled `title`, its OK button saying `ok`;
/// the chosen folder, or None when cancelled.
pub fn folder_dialog(title: &str, ok: &str) -> Option<String> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::Shell::{
        ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, BFFM_INITIALIZED, BFFM_SETOKTEXT, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

    /// Sets the OK button's text once the dialog is up.
    unsafe extern "system" fn on_event(hwnd: HWND, msg: u32, _: LPARAM, data: LPARAM) -> i32 {
        if msg == BFFM_INITIALIZED {
            // SAFETY: `hwnd` is the live dialog calling us; `data` is the
            // NUL-terminated `ok` text, alive until the dialog returns.
            unsafe { SendMessageW(hwnd, BFFM_SETOKTEXT, 0, data) };
        }
        0
    }

    let title = wide(title);
    let ok = wide(ok);
    let mut name = vec![0u16; 260];
    // SAFETY: every pointer in `bi` (title, OK text, the 260-unit name
    // buffer MAX_PATH asks for) outlives the modal call; the item list
    // returned is freed once; `path` is far larger than the MAX_PATH
    // units SHGetPathFromIDListW may write.
    unsafe {
        let mut bi: BROWSEINFOW = zeroed();
        bi.pszDisplayName = name.as_mut_ptr();
        bi.lpszTitle = title.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        bi.lpfn = Some(on_event);
        bi.lParam = ok.as_ptr() as LPARAM;
        let pidl = SHBrowseForFolderW(&bi);
        if pidl.is_null() {
            return None;
        }
        let mut path = vec![0u16; 32_768];
        let got = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
        ILFree(pidl);
        (got != 0).then(|| from_wide(&path)).filter(|p| !p.is_empty())
    }
}

/// An Open dialog titled `title` with a Win32 filter, opened in
/// `initial_dir` (Windows' choice when empty): the chosen file, None
/// when cancelled, Err when the dialog itself failed.
pub fn open_dialog(title: &str, filter: &str, initial_dir: &str) -> Result<Option<String>, ()> {
    use windows_sys::Win32::UI::Controls::Dialogs::CommDlgExtendedError;
    let filter: Vec<u16> = filter.encode_utf16().collect();
    let title = wide(title);
    let dir = wide(initial_dir);
    let mut file = vec![0u16; 1024];
    // SAFETY: the filter, title, folder and file buffer outlive the
    // modal call, and nMaxFile is the buffer's length.
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrTitle = title.as_ptr();
        if !initial_dir.is_empty() {
            ofn.lpstrInitialDir = dir.as_ptr();
        }
        ofn.lpstrFile = file.as_mut_ptr();
        ofn.nMaxFile = file.len() as u32;
        // OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST
        ofn.Flags = 0x8 | 0x800 | 0x1000;
        if GetOpenFileNameW(&mut ofn) != 0 {
            return Ok(Some(from_wide(&file)));
        }
        // Zero means the user cancelled; anything else is a failure.
        if CommDlgExtendedError() == 0 {
            Ok(None)
        } else {
            Err(())
        }
    }
}

/// The local time's offset from UTC in seconds.
pub fn local_offset_secs() -> i64 {
    // SAFETY: GetTimeZoneInformation fills the plain-data struct it is given.
    unsafe {
        use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let mut bias = tz.Bias;
        if r == 2 {
            bias += tz.DaylightBias;
        } else if r == 1 {
            bias += tz.StandardBias;
        }
        -(bias as i64) * 60
    }
}

/// Whether this process runs with administrator rights.
pub fn is_admin() -> bool {
    // SAFETY: IsUserAnAdmin takes no argument and only reads this
    // process's token.
    unsafe { windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0 }
}
