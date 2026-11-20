//! Per-process facts that `sysinfo` does not give: threads, handles, peak
//! memory, page faults, CPU, kernel and user time, priority, base priority,
//! session, working set, commit size, I/O counters, architecture, processor
//! group, whether the process owns a visible window (Apps or Background
//! processes) and its status (Running, Suspended, Not responding). The
//! sampler fills them into its rows each sample.

use std::collections::HashMap;
use std::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::sampler::ProcRow;

/// How many samples a priority class is kept before it is read again: it
/// needs one open per process, so it is not read every sample.
const PRIORITY_EVERY: u32 = 10;

/// What one system-wide read gives for a process.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub threads: u32,
    pub handles: u32,
    pub peak_mem: u64,
    pub page_faults: u32,
    pub cpu_time: Duration,
    pub kernel_time: Duration,
    pub user_time: Duration,
    pub session: u32,
    pub working_set: u64,
    /// Private bytes (the pagefile usage), Task Manager's "Commit size".
    pub commit: u64,
    /// The private working set: the Process Inspector's "Private memory".
    pub private_ws: u64,
    pub io_read: u64,
    pub io_write: u64,
    pub io_other: u64,
    pub io_read_ops: u64,
    pub io_write_ops: u64,
    /// The process's base priority (4, 6, 8, 10, 13 or 24 for the classes).
    pub base_priority: i32,
    /// When the process started, in the system's 100 ns units; with the PID
    /// it names one process even after the PID is reused.
    pub created: i64,
    /// Every thread of the process is suspended.
    pub suspended: bool,
}

/// A live process's status: "Suspended" when every thread is suspended,
/// "Not responding" when one of its windows hangs, otherwise "Running".
/// (A process that has ended reads "Exited": the process view keeps it.)
pub fn status_of(suspended: bool, hung: bool) -> &'static str {
    if suspended {
        "Suspended"
    } else if hung {
        "Not responding"
    } else {
        "Running"
    }
}

/// The priority class a base priority belongs to, for processes that
/// cannot be opened to ask for the class itself.
fn class_of_base(base: i32) -> &'static str {
    match base {
        24.. => "Realtime",
        13..=23 => "High",
        10..=12 => "Above normal",
        8..=9 => "Normal",
        6..=7 => "Below normal",
        1..=5 => "Low",
        _ => "",
    }
}

/// Reads the extra facts and keeps what rarely changes between samples:
/// the architecture and group are read once per process, the priority class every
/// `PRIORITY_EVERY` samples.
#[derive(Default)]
pub struct Reader {
    /// The architecture and processor group, read once per process.
    once: HashMap<(u32, i64), (&'static str, u16)>,
    priority: HashMap<(u32, i64), &'static str>,
    tick: u32,
    /// The system read's buffer, kept so each sample does not allocate it.
    buf: Vec<u64>,
}

impl Reader {
    /// Fills the extra facts into `rows`, matched by PID. Rows the system
    /// does not report keep their defaults.
    pub fn fill(&mut self, rows: &mut [ProcRow]) {
        let windows = imp::windows();
        let counts = imp::counts(&mut self.buf);
        for row in rows.iter_mut() {
            row.app = windows.apps.contains(&row.pid);
            let suspended = counts.get(&row.pid).is_some_and(|c| c.suspended);
            row.status = status_of(suspended, windows.hung.contains(&row.pid)).into();
        }
        if counts.is_empty() {
            return;
        }
        let fresh_priority = self.tick.is_multiple_of(PRIORITY_EVERY);
        self.tick = self.tick.wrapping_add(1);
        for row in rows.iter_mut() {
            let Some(c) = counts.get(&row.pid) else { continue };
            row.threads = c.threads;
            row.handles = c.handles;
            row.peak_mem = c.peak_mem;
            row.page_faults = c.page_faults;
            row.cpu_time = c.cpu_time;
            row.kernel_time = c.kernel_time;
            row.user_time = c.user_time;
            row.session = c.session;
            row.working_set = c.working_set;
            row.commit = c.commit;
            row.io_read = c.io_read;
            row.io_write = c.io_write;
            row.io_other = c.io_other;
            row.io_read_ops = c.io_read_ops;
            row.io_write_ops = c.io_write_ops;
            row.base_priority = c.base_priority;
            let key = (row.pid, c.created);
            (row.arch, row.group) = *self.once.entry(key).or_insert_with(|| imp::identity(row.pid));
            if fresh_priority || !self.priority.contains_key(&key) {
                let class = imp::priority_class(row.pid).unwrap_or_else(|| class_of_base(c.base_priority));
                self.priority.insert(key, class);
            }
            row.priority = self.priority[&key];
        }
        // Forget processes that have gone.
        self.once.retain(|(pid, created), _| counts.get(pid).is_some_and(|c| c.created == *created));
        self.priority.retain(|(pid, created), _| counts.get(pid).is_some_and(|c| c.created == *created));
    }
}

/// What one walk of the top-level windows gives.
#[derive(Debug, Default)]
pub struct Windows {
    /// The processes listed as Apps (see `imp::windows`).
    pub apps: std::collections::HashSet<u32>,
    /// The processes with a window that does not respond (IsHungAppWindow).
    pub hung: std::collections::HashSet<u32>,
    /// The processes with any visible top-level window, Apps or not
    /// (Bring to front is greyed only without one).
    pub windowed: std::collections::HashSet<u32>,
}

/// Whether `pid` has any visible top-level window (one walk of the
/// windows, as [`Windows`] reads them).
pub fn has_window(pid: u32) -> bool {
    imp::windows().windowed.contains(&pid)
}

/// Why a part of a process's details could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Missing {
    /// The process may not be opened for this ("Access denied").
    Denied,
    /// Windows did not give it ("Unavailable").
    Unavailable,
    /// The process ended before it was read.
    Exited,
}

/// What an open resource of a process is, as the Process Inspector types
/// its rows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ResourceKind {
    File,
    Key,
    /// A socket or a named pipe (counted together).
    Socket,
}

/// One handle a process holds open on a file, a registry key or a socket.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resource {
    pub kind: ResourceKind,
    /// The file's path, the key's path or the device's name; "Unavailable"
    /// when its name could not be read in time.
    pub name: String,
}

/// What asking for one handle's name gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Named {
    Name(String),
    /// The handle has no name.
    Nameless,
    /// The ask did not answer in time (a named pipe can block it forever).
    Stuck,
}

/// After this many stuck asks in one [`name_each`] the rest are not asked:
/// each one leaves a thread blocked until Windows answers it.
const MAX_STUCK: usize = 8;
/// At most this many name helper threads live at once in the whole
/// process, stuck ones included, however many reads run or ran.
const MAX_HELPERS: usize = 16;

/// A count of live name helper threads with a cap: a helper takes a slot
/// before it starts and gives it back when its thread ends, so threads
/// left blocked by stuck asks cannot pile up across calls.
pub struct Helpers {
    live: AtomicUsize,
    cap: usize,
}

impl Helpers {
    pub const fn new(cap: usize) -> Self {
        Helpers { live: AtomicUsize::new(0), cap }
    }

    /// How many helper threads are alive now.
    #[cfg(test)]
    pub fn live(&self) -> usize {
        self.live.load(Ordering::SeqCst)
    }

    /// Takes a slot; false when all are taken.
    fn reserve(&self) -> bool {
        self.live.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| (n < self.cap).then_some(n + 1)).is_ok()
    }
}

/// A taken slot of [`Helpers`], given back when dropped (when its helper
/// thread ends, or at once when the thread could not start).
struct Slot(&'static Helpers);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.live.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The process-wide helper slots behind [`name_each`].
static HELPERS: Helpers = Helpers::new(MAX_HELPERS);

/// How [`name_each`] asks an item's name.
pub type NameQuery<T> = std::sync::Arc<dyn Fn(&T) -> Option<String> + Send + Sync>;

/// Asks `query` for each item's name on a helper thread, waiting at most
/// `each` per item, and gives the answers in the items' order. An item
/// whose ask does not answer in time is `Stuck`; its thread is left to
/// finish on its own (it drops the item then) and a new one asks the rest,
/// while the process has fewer than `MAX_HELPERS` helpers alive. So a
/// handle that blocks the name query cannot block the caller.
pub fn name_each<T: Send + 'static>(items: Vec<T>, query: NameQuery<T>, each: Duration) -> Vec<Named> {
    name_each_in(&HELPERS, items, query, each)
}

/// [`name_each`] with its helper threads counted in `helpers`.
fn name_each_in<T: Send + 'static>(helpers: &'static Helpers, items: Vec<T>, query: NameQuery<T>, each: Duration) -> Vec<Named> {
    use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
    let spawn = |query: NameQuery<T>| -> Option<(Sender<T>, std::sync::mpsc::Receiver<Option<String>>)> {
        if !helpers.reserve() {
            return None;
        }
        let slot = Slot(helpers);
        let (jobs, job_rx) = channel::<T>();
        let (answers, rx) = channel();
        std::thread::Builder::new()
            .name("handle-names".into())
            .spawn(move || {
                let _slot = slot;
                for item in job_rx {
                    if answers.send(query(&item)).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some((jobs, rx))
    };
    let mut out = Vec::with_capacity(items.len());
    let mut stuck = 0;
    let mut helper = spawn(query.clone());
    for item in items {
        let Some((jobs, answers)) = helper.as_ref() else {
            out.push(Named::Stuck);
            continue;
        };
        if jobs.send(item).is_err() {
            out.push(Named::Stuck);
            continue;
        }
        match answers.recv_timeout(each) {
            Ok(Some(name)) if !name.is_empty() => out.push(Named::Name(name)),
            Ok(_) => out.push(Named::Nameless),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => {
                out.push(Named::Stuck);
                stuck += 1;
                // The helper is blocked: leave it, ask the rest on a new one.
                helper = if stuck < MAX_STUCK { spawn(query.clone()) } else { None };
            }
        }
    }
    out
}

/// The variables of an environment block (UTF-16 "NAME=VALUE" strings,
/// each ending in a NUL, the block in two). Names may start with "="
/// (the per-drive current folders).
pub fn parse_environment(block: &[u16]) -> Vec<(String, String)> {
    block
        .split(|&c| c == 0)
        .take_while(|s| !s.is_empty())
        .map(|s| {
            let s = String::from_utf16_lossy(s);
            match s.char_indices().skip(1).find(|&(_, c)| c == '=') {
                Some((i, _)) => (s[..i].to_string(), s[i + 1..].to_string()),
                None => (s, String::new()),
            }
        })
        .collect()
}

/// The private working set of the process, from one system-wide read;
/// `None` when the process is not listed.
pub fn private_memory(pid: u32) -> Option<u64> {
    imp::counts(&mut Vec::new()).get(&pid).map(|c| c.private_ws)
}

/// The IDs of the threads of the process, from the system's process list
/// (SystemProcessInformation); `None` when the process is not listed.
pub fn thread_ids(pid: u32) -> Option<Vec<u32>> {
    imp::thread_ids(pid)
}

/// The command line and the environment variables, each read on its own.
pub type CommandAndEnvironment = (Result<String, Missing>, Result<Vec<(String, String)>, Missing>);

/// The command line and the environment variables of the process, read
/// from its PEB (see `imp::Params` for the layout).
pub fn command_line_and_environment(pid: u32) -> CommandAndEnvironment {
    imp::command_line_and_environment(pid)
}

/// The files, registry keys and sockets the process holds open, from the
/// system's handle list; each name is asked off the caller's thread with
/// at most `each` per handle (see [`name_each`]). Sorted by kind and name,
/// without repeats.
pub fn open_resources(pid: u32, each: Duration) -> Result<Vec<Resource>, Missing> {
    imp::open_resources(pid, each)
}

/// A kernel path as the user knows it: "\Device\HarddiskVolume3\x" →
/// "C:\x" with `drives` from [`drive_devices`]; other names as they are.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn dos_path(name: &str, drives: &[(String, String)]) -> String {
    let lower = name.to_ascii_lowercase();
    drives
        .iter()
        .find(|(device, _)| lower.starts_with(&device.to_ascii_lowercase()) && name[device.len()..].starts_with('\\'))
        .map_or_else(|| name.to_string(), |(device, drive)| format!("{drive}{}", &name[device.len()..]))
}

/// The NT names of the drives ("\Device\HarddiskVolume3" → "C:"), for
/// [`dos_path`].
#[cfg(windows)]
pub fn drive_devices() -> Vec<(String, String)> {
    use windows_sys::Win32::Storage::FileSystem::QueryDosDeviceW;
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let drive = format!("{}:", letter as char);
        let name = crate::winsys::wide(&drive);
        let mut target = vec![0u16; 1024];
        // SAFETY: `name` is NUL-terminated and `target` holds 1024 units.
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), target.as_mut_ptr(), target.len() as u32) } as usize;
        if n > 0 {
            let device = String::from_utf16_lossy(&target[..n.min(target.len())]);
            let device = device.split('\0').next().unwrap_or_default().to_string();
            if !device.is_empty() {
                out.push((device, drive));
            }
        }
    }
    out
}

/// Said when Bring to front finds no window to bring up or Windows refuses.
pub const FRONT_FAILED: &str = "Windows could not bring this process's window to the front.";

/// Bring to front: the process's first window of the kind that lists it
/// under Apps (the walk of [`Windows`]), else its first visible top-level
/// window, is restored if minimized and comes to the foreground;
/// [`FRONT_FAILED`] when it has none or Windows refuses.
pub fn bring_to_front(pid: u32) -> Result<(), String> {
    if imp::bring_to_front(pid) {
        Ok(())
    } else {
        Err(FRONT_FAILED.into())
    }
}

#[cfg(windows)]
mod imp {
    use super::Counts;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::time::Duration;
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::SystemInformation::*;
    use windows_sys::Win32::System::Threading::{GetProcessGroupAffinity, IsWow64Process2, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// SystemProcessInformation, for NtQuerySystemInformation.
    const SYSTEM_PROCESS_INFORMATION: u32 = 5;
    /// STATUS_INFO_LENGTH_MISMATCH: the buffer was too small.
    const LENGTH_MISMATCH: i32 = 0xC000_0004u32 as i32;

    /// SYSTEM_PROCESS_INFORMATION as ntdll lays it out, up to the I/O
    /// counters (the `Thread` array follows). Offsets on x64 / x86 are given
    /// per field and checked at compile time below; repr(C) gives the same
    /// padding as the C struct on both.
    #[repr(C)]
    #[allow(dead_code)]
    struct Entry {
        next: u32,                  // 0x00 / 0x00
        threads: u32,               // 0x04 / 0x04
        working_set_private: i64,   // 0x08 / 0x08
        hard_faults: u32,           // 0x10 / 0x10
        threads_high: u32,          // 0x14 / 0x14
        cycle_time: u64,            // 0x18 / 0x18
        create_time: i64,           // 0x20 / 0x20
        user_time: i64,             // 0x28 / 0x28
        kernel_time: i64,           // 0x30 / 0x30
        image_len: u16,             // 0x38 / 0x38 (UNICODE_STRING ImageName)
        image_max: u16,             // 0x3a / 0x3a
        image: *const u16,          // 0x40 / 0x3c
        base_priority: i32,         // 0x48 / 0x40
        pid: usize,                 // 0x50 / 0x44
        parent: usize,              // 0x58 / 0x48
        handles: u32,               // 0x60 / 0x4c
        session: u32,               // 0x64 / 0x50
        key: usize,                 // 0x68 / 0x54
        peak_virtual: usize,        // 0x70 / 0x58
        virtual_size: usize,        // 0x78 / 0x5c
        page_faults: u32,           // 0x80 / 0x60
        peak_working_set: usize,    // 0x88 / 0x64
        working_set: usize,         // 0x90 / 0x68
        quota_peak_paged: usize,    // 0x98 / 0x6c
        quota_paged: usize,         // 0xa0 / 0x70
        quota_peak_nonpaged: usize, // 0xa8 / 0x74
        quota_nonpaged: usize,      // 0xb0 / 0x78
        pagefile: usize,            // 0xb8 / 0x7c (PagefileUsage: the commit size)
        peak_pagefile: usize,       // 0xc0 / 0x80
        private_pages: usize,       // 0xc8 / 0x84
        read_ops: i64,              // 0xd0 / 0x88 (ReadOperationCount)
        write_ops: i64,             // 0xd8 / 0x90
        other_ops: i64,             // 0xe0 / 0x98
        read_bytes: i64,            // 0xe8 / 0xa0 (ReadTransferCount)
        write_bytes: i64,           // 0xf0 / 0xa8
        other_bytes: i64,           // 0xf8 / 0xb0
    }

    /// Checks every field's offset, and the size, against the layout above:
    /// (x64, x86) for each.
    macro_rules! layout {
        ($($field:ident: $x64:literal, $x86:literal;)* => size $s64:literal, $s86:literal) => {
            #[cfg(target_pointer_width = "64")]
            const _: () = {
                $(assert!(std::mem::offset_of!(Entry, $field) == $x64);)*
                assert!(std::mem::size_of::<Entry>() == $s64);
            };
            #[cfg(target_pointer_width = "32")]
            const _: () = {
                $(assert!(std::mem::offset_of!(Entry, $field) == $x86);)*
                assert!(std::mem::size_of::<Entry>() == $s86);
            };
        };
    }
    layout! {
        next: 0x00, 0x00; threads: 0x04, 0x04; working_set_private: 0x08, 0x08; hard_faults: 0x10, 0x10;
        threads_high: 0x14, 0x14; cycle_time: 0x18, 0x18; create_time: 0x20, 0x20; user_time: 0x28, 0x28;
        kernel_time: 0x30, 0x30; image_len: 0x38, 0x38; image_max: 0x3a, 0x3a; image: 0x40, 0x3c;
        base_priority: 0x48, 0x40; pid: 0x50, 0x44; parent: 0x58, 0x48; handles: 0x60, 0x4c;
        session: 0x64, 0x50; key: 0x68, 0x54; peak_virtual: 0x70, 0x58; virtual_size: 0x78, 0x5c;
        page_faults: 0x80, 0x60; peak_working_set: 0x88, 0x64; working_set: 0x90, 0x68;
        quota_peak_paged: 0x98, 0x6c; quota_paged: 0xa0, 0x70; quota_peak_nonpaged: 0xa8, 0x74;
        quota_nonpaged: 0xb0, 0x78; pagefile: 0xb8, 0x7c; peak_pagefile: 0xc0, 0x80; private_pages: 0xc8, 0x84;
        read_ops: 0xd0, 0x88; write_ops: 0xd8, 0x90; other_ops: 0xe0, 0x98; read_bytes: 0xe8, 0xa0;
        write_bytes: 0xf0, 0xa8; other_bytes: 0xf8, 0xb0;
        => size 0x100, 0xb8
    }

    /// SYSTEM_THREAD_INFORMATION, `Entry::threads` of them right after each
    /// entry; offsets on x64 / x86 checked at compile time below.
    #[repr(C)]
    #[allow(dead_code)]
    struct Thread {
        kernel_time: i64,      // 0x00 / 0x00
        user_time: i64,        // 0x08 / 0x08
        create_time: i64,      // 0x10 / 0x10
        wait_time: u32,        // 0x18 / 0x18
        start: usize,          // 0x20 / 0x1c
        process: usize,        // 0x28 / 0x20 (CLIENT_ID)
        thread: usize,         // 0x30 / 0x24
        priority: i32,         // 0x38 / 0x28
        base_priority: i32,    // 0x3c / 0x2c
        context_switches: u32, // 0x40 / 0x30
        state: u32,            // 0x44 / 0x34 (KTHREAD_STATE)
        wait_reason: u32,      // 0x48 / 0x38 (KWAIT_REASON)
    }
    const _: () = {
        #[cfg(target_pointer_width = "64")]
        let at = [0x00, 0x08, 0x10, 0x18, 0x20, 0x28, 0x30, 0x38, 0x3c, 0x40, 0x44, 0x48, 0x50];
        #[cfg(target_pointer_width = "32")]
        let at = [0x00, 0x08, 0x10, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x34, 0x38, 0x40];
        use std::mem::offset_of;
        assert!(offset_of!(Thread, kernel_time) == at[0] && offset_of!(Thread, user_time) == at[1]);
        assert!(offset_of!(Thread, create_time) == at[2] && offset_of!(Thread, wait_time) == at[3]);
        assert!(offset_of!(Thread, start) == at[4] && offset_of!(Thread, process) == at[5]);
        assert!(offset_of!(Thread, thread) == at[6] && offset_of!(Thread, priority) == at[7]);
        assert!(offset_of!(Thread, base_priority) == at[8] && offset_of!(Thread, context_switches) == at[9]);
        assert!(offset_of!(Thread, state) == at[10] && offset_of!(Thread, wait_reason) == at[11]);
        assert!(std::mem::size_of::<Thread>() == at[12]);
    };
    /// KTHREAD_STATE Waiting and KWAIT_REASON Suspended: a suspended thread.
    const WAITING: u32 = 5;
    const SUSPENDED: u32 = 5;

    type Query = unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32;

    fn query() -> Option<Query> {
        static QUERY: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
        let f = QUERY.get_or_init(|| {
            let lib = crate::winsys::wide("ntdll.dll");
            // SAFETY: `lib` is a NUL-terminated name; ntdll is loaded in every
            // process, so the module stays valid for the program's life.
            unsafe {
                let h = LoadLibraryW(lib.as_ptr());
                if h.is_null() {
                    return None;
                }
                GetProcAddress(h, c"NtQuerySystemInformation".as_ptr().cast()).map(|f| f as usize)
            }
        });
        // SAFETY: the address is ntdll's NtQuerySystemInformation, whose
        // signature is `Query`.
        f.map(|f| unsafe { std::mem::transmute::<usize, Query>(f) })
    }

    /// Fills `buf` with the system's process list (SystemProcessInformation),
    /// growing it as needed; false when Windows did not give it.
    fn read_all(buf: &mut Vec<u64>) -> bool {
        let Some(query) = query() else { return false };
        if buf.is_empty() {
            *buf = vec![0; 256 * 1024 / 8];
        }
        let mut len = 0u32;
        loop {
            let bytes = (buf.len() * 8) as u32;
            // SAFETY: the buffer is writable for `bytes` bytes and `len` is a
            // valid out-param.
            let status = unsafe { query(SYSTEM_PROCESS_INFORMATION, buf.as_mut_ptr() as *mut c_void, bytes, &mut len) };
            if status == LENGTH_MISMATCH && buf.len() < 64 * 1024 * 1024 / 8 {
                *buf = vec![0; (len as usize).max(bytes as usize * 2).div_ceil(8) + 8192];
                continue;
            }
            return status >= 0;
        }
    }

    /// The IDs of the threads of `pid`, from the thread array that follows
    /// its entry in one NtQuerySystemInformation call; `None` when the
    /// process is not listed.
    pub fn thread_ids(pid: u32) -> Option<Vec<u32>> {
        let mut buf = Vec::new();
        if !read_all(&mut buf) {
            return None;
        }
        let total = buf.len() * 8;
        let base = buf.as_ptr() as *const u8;
        let mut at = 0usize;
        while at + std::mem::size_of::<Entry>() <= total {
            // SAFETY: the whole entry lies inside the buffer (checked by the
            // loop condition) and read_unaligned needs no alignment.
            let e = unsafe { std::ptr::read_unaligned(base.add(at) as *const Entry) };
            if e.pid as u32 == pid {
                let first = at + std::mem::size_of::<Entry>();
                let end = first + e.threads as usize * std::mem::size_of::<Thread>();
                if end > total || (e.next != 0 && end > at + e.next as usize) {
                    return None;
                }
                let ids = (0..e.threads as usize)
                    .map(|i| {
                        // SAFETY: thread `i` lies inside the buffer (checked
                        // above) and read_unaligned needs no alignment.
                        let t = unsafe { std::ptr::read_unaligned(base.add(first + i * std::mem::size_of::<Thread>()) as *const Thread) };
                        t.thread as u32
                    })
                    .collect();
                return Some(ids);
            }
            if e.next == 0 {
                break;
            }
            at += e.next as usize;
        }
        None
    }

    /// Every process's counts by PID, from one NtQuerySystemInformation call.
    pub fn counts(buf: &mut Vec<u64>) -> HashMap<u32, Counts> {
        let mut out = HashMap::new();
        if !read_all(buf) {
            return out;
        }
        let total = buf.len() * 8;
        let base = buf.as_ptr() as *const u8;
        let mut at = 0usize;
        while at + std::mem::size_of::<Entry>() <= total {
            // SAFETY: the whole entry lies inside the buffer (checked by the
            // loop condition) and read_unaligned needs no alignment.
            let e = unsafe { std::ptr::read_unaligned(base.add(at) as *const Entry) };
            // Times are in 100 ns units.
            let time = |t: i64| Duration::from_nanos((t.max(0) as u64).saturating_mul(100));
            let ticks = (e.user_time.max(0) + e.kernel_time.max(0)) as u64;
            // The thread array, when it lies inside the entry and the buffer.
            let first = at + std::mem::size_of::<Entry>();
            let end = first + e.threads as usize * std::mem::size_of::<Thread>();
            let fits = end <= total && (e.next == 0 || end <= at + e.next as usize);
            let suspended = fits
                && e.threads > 0
                && (0..e.threads as usize).all(|i| {
                    // SAFETY: thread `i` lies inside the buffer (`fits`) and
                    // read_unaligned needs no alignment.
                    let t = unsafe { std::ptr::read_unaligned(base.add(first + i * std::mem::size_of::<Thread>()) as *const Thread) };
                    t.state == WAITING && t.wait_reason == SUSPENDED
                });
            out.insert(
                e.pid as u32,
                Counts {
                    threads: e.threads,
                    handles: e.handles,
                    peak_mem: e.peak_working_set as u64,
                    page_faults: e.page_faults,
                    cpu_time: Duration::from_nanos(ticks.saturating_mul(100)),
                    kernel_time: time(e.kernel_time),
                    user_time: time(e.user_time),
                    session: e.session,
                    working_set: e.working_set as u64,
                    commit: e.pagefile as u64,
                    private_ws: e.working_set_private.max(0) as u64,
                    io_read: e.read_bytes.max(0) as u64,
                    io_write: e.write_bytes.max(0) as u64,
                    io_other: e.other_bytes.max(0) as u64,
                    io_read_ops: e.read_ops.max(0) as u64,
                    io_write_ops: e.write_ops.max(0) as u64,
                    base_priority: e.base_priority,
                    created: e.create_time,
                    suspended,
                },
            );
            if e.next == 0 {
                break;
            }
            at += e.next as usize;
        }
        out
    }

    /// The architecture the process runs as ("x64", "x86", "ARM64" or "ARM",
    /// empty when unknown) and its processor group (0 when unknown), from
    /// one open of the process.
    pub fn identity(pid: u32) -> (&'static str, u16) {
        // SAFETY: the handle is checked before use and closed by `Handle`;
        // the machines, the count and the group array are out-params of the
        // right type, the array as long as the count says.
        unsafe {
            let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return ("", 0);
            }
            let h = crate::winsys::Handle(h);
            let (mut process, mut native) = (0u16, 0u16);
            let arch = if IsWow64Process2(h.0, &mut process, &mut native) == 0 {
                ""
            } else {
                // Not under WOW64: the process runs as the machine does.
                let machine = if process == IMAGE_FILE_MACHINE_UNKNOWN { native } else { process };
                match machine {
                    IMAGE_FILE_MACHINE_AMD64 => "x64",
                    IMAGE_FILE_MACHINE_I386 => "x86",
                    IMAGE_FILE_MACHINE_ARM64 => "ARM64",
                    IMAGE_FILE_MACHINE_ARMNT | IMAGE_FILE_MACHINE_ARM => "ARM",
                    _ => "",
                }
            };
            // The first of the process's groups (a process starts in one).
            let (mut count, mut groups) = (4u16, [0u16; 4]);
            let group = if GetProcessGroupAffinity(h.0, &mut count, groups.as_mut_ptr()) != 0 && count > 0 { groups[0] } else { 0 };
            (arch, group)
        }
    }

    /// The priority class from GetPriorityClass; `None` when the process
    /// cannot be opened.
    pub fn priority_class(pid: u32) -> Option<&'static str> {
        crate::winsys::get_priority(pid)
    }

    /// The PIDs that own a visible, titled top-level window that is not a
    /// tool window or owned by another window: the processes listed as
    /// Apps; and those of them with such a window that hangs
    /// (IsHungAppWindow: Not responding). One EnumWindows walk per sample.
    /// The PID that owns `hwnd` when it is a window of the kind that lists
    /// its process under Apps: visible, titled, not a tool window and not
    /// owned by another window.
    ///
    /// # Safety
    /// `hwnd` is a window handle Windows gave (it may have closed since).
    unsafe fn app_window_pid(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<u32> {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        // SAFETY: each call takes any window handle and fails cleanly for
        // one that has closed.
        unsafe {
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0;
            if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() || tool || GetWindowTextLengthW(hwnd) == 0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            (pid != 0).then_some(pid)
        }
    }

    /// The PID that owns `hwnd` when it is visible (any kind of top-level
    /// window).
    ///
    /// # Safety
    /// `hwnd` is a window handle Windows gave (it may have closed since).
    unsafe fn visible_window_pid(hwnd: windows_sys::Win32::Foundation::HWND) -> Option<u32> {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        // SAFETY: both calls take any window handle and fail cleanly for
        // one that has closed.
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return None;
            }
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, &mut pid);
            (pid != 0).then_some(pid)
        }
    }

    /// Brings up the first window of `pid` that [`app_window_pid`] counts,
    /// else its first visible one (restored when minimized); false when it
    /// has none or Windows refuses.
    pub fn bring_to_front(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        /// The PID looked for, then its first App window and its first
        /// visible window.
        struct Look(u32, HWND, HWND);
        unsafe extern "system" fn each(hwnd: HWND, look: LPARAM) -> windows_sys::core::BOOL {
            // SAFETY: `look` is the value passed to EnumWindows below,
            // alive for the whole walk; `hwnd` is a window EnumWindows gives.
            unsafe {
                let look = &mut *(look as *mut Look);
                if app_window_pid(hwnd) == Some(look.0) {
                    look.1 = hwnd;
                    return 0;
                }
                if look.2.is_null() && visible_window_pid(hwnd) == Some(look.0) {
                    look.2 = hwnd;
                }
            }
            1
        }
        let mut look = Look(pid, std::ptr::null_mut(), std::ptr::null_mut());
        // SAFETY: the callback only touches `look`, which outlives the
        // call; the window found is checked before use.
        unsafe {
            EnumWindows(Some(each), &mut look as *mut _ as LPARAM);
            let hwnd = if look.1.is_null() { look.2 } else { look.1 };
            if hwnd.is_null() {
                return false;
            }
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd) != 0
        }
    }

    pub fn windows() -> super::Windows {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        unsafe extern "system" fn each(hwnd: HWND, pids: LPARAM) -> windows_sys::core::BOOL {
            // SAFETY: `pids` is the set passed to EnumWindows below, alive
            // for the whole walk; `hwnd` is a window EnumWindows gives.
            unsafe {
                let found = &mut *(pids as *mut super::Windows);
                if let Some(pid) = visible_window_pid(hwnd) {
                    found.windowed.insert(pid);
                }
                if let Some(pid) = app_window_pid(hwnd) {
                    found.apps.insert(pid);
                    if IsHungAppWindow(hwnd) != 0 {
                        found.hung.insert(pid);
                    }
                }
            }
            1
        }
        let mut found = super::Windows::default();
        // SAFETY: the callback only touches `found`, which outlives the call.
        unsafe { EnumWindows(Some(each), &mut found as *mut _ as LPARAM) };
        found
    }

    use super::{Missing, Named, Resource, ResourceKind};
    use crate::winsys::Handle;
    use windows_sys::Win32::Foundation::{DuplicateHandle, GetLastError, DUPLICATE_SAME_ACCESS, HANDLE};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetExitCodeProcess, PROCESS_DUP_HANDLE, PROCESS_VM_READ};

    #[link(name = "kernel32")]
    extern "system" {
        // Declared here rather than through windows-sys, whose
        // Diagnostics_Debug feature this crate does not turn on.
        fn ReadProcessMemory(process: HANDLE, base: *const c_void, buf: *mut c_void, size: usize, read: *mut usize) -> i32;
    }

    /// NtQueryInformationProcess and NtQueryObject share this shape.
    type HandleQuery = unsafe extern "system" fn(HANDLE, u32, *mut c_void, u32, *mut u32) -> i32;

    /// An ntdll export with the `HandleQuery` shape, by name.
    fn ntdll_query(name: &std::ffi::CStr) -> Option<HandleQuery> {
        let lib = crate::winsys::wide("ntdll.dll");
        // SAFETY: `lib` is NUL-terminated and ntdll is loaded in every
        // process; the export named has the `HandleQuery` signature (the
        // callers name only NtQueryInformationProcess and NtQueryObject).
        unsafe {
            let h = GetModuleHandleW(lib.as_ptr());
            if h.is_null() {
                return None;
            }
            GetProcAddress(h, name.as_ptr().cast()).map(|f| std::mem::transmute::<usize, HandleQuery>(f as usize))
        }
    }

    /// ProcessBasicInformation, for NtQueryInformationProcess.
    const PROCESS_BASIC_INFORMATION: u32 = 0;
    /// SystemExtendedHandleInformation, for NtQuerySystemInformation.
    const SYSTEM_EXTENDED_HANDLE_INFORMATION: u32 = 64;
    /// ObjectNameInformation and ObjectTypeInformation, for NtQueryObject.
    const OBJECT_NAME_INFORMATION: u32 = 1;
    const OBJECT_TYPE_INFORMATION: u32 = 2;
    /// STATUS_BUFFER_OVERFLOW: the name did not fit.
    const BUFFER_OVERFLOW: i32 = 0x8000_0005u32 as i32;
    /// GetExitCodeProcess's code for a process still running.
    const STILL_ACTIVE: u32 = 259;
    /// The longest environment block read (4 MiB).
    const MAX_ENVIRONMENT: usize = 4 << 20;

    /// UNICODE_STRING: a length in bytes and an address.
    #[repr(C)]
    #[derive(Clone, Copy)]
    #[allow(dead_code)]
    struct UStr {
        len: u16,   // 0x00 / 0x00
        max: u16,   // 0x02 / 0x02
        buf: usize, // 0x08 / 0x04
    }

    /// PROCESS_BASIC_INFORMATION.
    #[repr(C)]
    #[allow(dead_code)]
    struct BasicInfo {
        exit_status: i32,   // 0x00 / 0x00
        peb: usize,         // 0x08 / 0x04
        affinity: usize,    // 0x10 / 0x08
        base_priority: i32, // 0x18 / 0x0c
        pid: usize,         // 0x20 / 0x10
        parent: usize,      // 0x28 / 0x14
    }

    /// The start of the PEB, up to ProcessParameters.
    #[repr(C)]
    #[allow(dead_code)]
    struct PebHead {
        inherited_address_space: u8, // 0x00 / 0x00
        read_image_file_exec_options: u8,
        being_debugged: u8,
        bit_field: u8,
        mutant: usize,             // 0x08 / 0x04
        image_base: usize,         // 0x10 / 0x08
        ldr: usize,                // 0x18 / 0x0c
        process_parameters: usize, // 0x20 / 0x10
    }

    /// RTL_DRIVE_LETTER_CURDIR.
    #[repr(C)]
    #[allow(dead_code)]
    struct DriveDir {
        flags: u16,
        length: u16,
        time_stamp: u32,
        dos_path: UStr,
    }

    /// RTL_USER_PROCESS_PARAMETERS up to EnvironmentSize (Vista and later).
    #[repr(C)]
    #[allow(dead_code)]
    struct Params {
        maximum_length: u32,       // 0x00 / 0x00
        length: u32,               // 0x04 / 0x04
        flags: u32,                // 0x08 / 0x08
        debug_flags: u32,          // 0x0c / 0x0c
        console_handle: usize,     // 0x10 / 0x10
        console_flags: u32,        // 0x18 / 0x14
        std_in: usize,             // 0x20 / 0x18
        std_out: usize,            // 0x28 / 0x1c
        std_err: usize,            // 0x30 / 0x20
        current_directory: UStr,   // 0x38 / 0x24 (CURDIR.DosPath)
        current_directory_handle: usize, // 0x48 / 0x2c
        dll_path: UStr,            // 0x50 / 0x30
        image_path: UStr,          // 0x60 / 0x38
        command_line: UStr,        // 0x70 / 0x40
        environment: usize,        // 0x80 / 0x48
        starting: [u32; 9],        // 0x88 / 0x4c (StartingX … ShowWindowFlags)
        window_title: UStr,        // 0xb0 / 0x70
        desktop_info: UStr,        // 0xc0 / 0x78
        shell_info: UStr,          // 0xd0 / 0x80
        runtime_data: UStr,        // 0xe0 / 0x88
        drive_dirs: [DriveDir; 32], // 0xf0 / 0x90
        environment_size: usize,   // 0x3f0 / 0x290
    }

    /// SYSTEM_HANDLE_TABLE_ENTRY_INFO_EX: one handle of the system's list
    /// (after a two-word head: the count and a reserved word).
    #[repr(C)]
    #[allow(dead_code)]
    struct HandleEntry {
        object: usize,      // 0x00 / 0x00
        pid: usize,         // 0x08 / 0x04
        value: usize,       // 0x10 / 0x08
        access: u32,        // 0x18 / 0x0c
        back_trace: u16,    // 0x1c / 0x10
        type_index: u16,    // 0x1e / 0x12
        attributes: u32,    // 0x20 / 0x14
        reserved: u32,      // 0x24 / 0x18
    }

    /// Checks each struct's field offsets and size: (x64, x86).
    macro_rules! layouts {
        ($($ty:ident { $($field:ident: $x64:literal, $x86:literal;)* } size $s64:literal, $s86:literal;)*) => {
            #[cfg(target_pointer_width = "64")]
            const _: () = {
                $($(assert!(std::mem::offset_of!($ty, $field) == $x64);)* assert!(std::mem::size_of::<$ty>() == $s64);)*
            };
            #[cfg(target_pointer_width = "32")]
            const _: () = {
                $($(assert!(std::mem::offset_of!($ty, $field) == $x86);)* assert!(std::mem::size_of::<$ty>() == $s86);)*
            };
        };
    }
    layouts! {
        UStr { len: 0x00, 0x00; max: 0x02, 0x02; buf: 0x08, 0x04; } size 0x10, 0x08;
        BasicInfo { exit_status: 0x00, 0x00; peb: 0x08, 0x04; affinity: 0x10, 0x08; base_priority: 0x18, 0x0c; pid: 0x20, 0x10; parent: 0x28, 0x14; } size 0x30, 0x18;
        PebHead { mutant: 0x08, 0x04; image_base: 0x10, 0x08; ldr: 0x18, 0x0c; process_parameters: 0x20, 0x10; } size 0x28, 0x14;
        DriveDir { flags: 0x00, 0x00; length: 0x02, 0x02; time_stamp: 0x04, 0x04; dos_path: 0x08, 0x08; } size 0x18, 0x10;
        Params {
            console_handle: 0x10, 0x10; console_flags: 0x18, 0x14; std_in: 0x20, 0x18; std_err: 0x30, 0x20;
            current_directory: 0x38, 0x24; current_directory_handle: 0x48, 0x2c; dll_path: 0x50, 0x30; image_path: 0x60, 0x38;
            command_line: 0x70, 0x40; environment: 0x80, 0x48; starting: 0x88, 0x4c; window_title: 0xb0, 0x70;
            runtime_data: 0xe0, 0x88; drive_dirs: 0xf0, 0x90; environment_size: 0x3f0, 0x290;
        } size 0x3f8, 0x294;
        HandleEntry {
            object: 0x00, 0x00; pid: 0x08, 0x04; value: 0x10, 0x08; access: 0x18, 0x0c; back_trace: 0x1c, 0x10;
            type_index: 0x1e, 0x12; attributes: 0x20, 0x14; reserved: 0x24, 0x18;
        } size 0x28, 0x1c;
    }

    /// Opens the process for `access`: a refusal is `Denied`, a PID no
    /// longer running is `Exited` (also when the process has ended but is
    /// still held open by someone).
    fn open(pid: u32, access: u32) -> Result<Handle, Missing> {
        // SAFETY: OpenProcess takes plain values; the error code is read
        // right after the failed call; the exit code is an out-param.
        unsafe {
            let h = OpenProcess(access | PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if h.is_null() {
                return Err(match GetLastError() {
                    5 => Missing::Denied,
                    87 => Missing::Exited,
                    _ => Missing::Unavailable,
                });
            }
            let h = Handle(h);
            let mut code = 0u32;
            if GetExitCodeProcess(h.0, &mut code) != 0 && code != STILL_ACTIVE {
                return Err(Missing::Exited);
            }
            Ok(h)
        }
    }

    /// Reads a `T` (plain integers only) from the process at `at`.
    fn read_struct<T>(h: &Handle, at: usize) -> Option<T> {
        // SAFETY: every `T` used here is a repr(C) struct of integers, so
        // all-zero and any bytes read are valid values; the buffer is
        // exactly `size_of::<T>()` writable bytes.
        unsafe {
            let mut v: T = std::mem::zeroed();
            let mut got = 0usize;
            let size = std::mem::size_of::<T>();
            let ok = ReadProcessMemory(h.0, at as *const c_void, (&mut v as *mut T).cast(), size, &mut got) != 0;
            (ok && got == size).then_some(v)
        }
    }

    /// Reads `count` UTF-16 units from the process at `at`.
    /// No units (an empty CommandLine, whose buffer may be null) read as
    /// an empty text without asking.
    fn read_units(h: &Handle, at: usize, count: usize) -> Option<Vec<u16>> {
        if count == 0 {
            return Some(Vec::new());
        }
        let mut out = vec![0u16; count];
        let mut got = 0usize;
        // SAFETY: `out` is writable for `count * 2` bytes.
        let ok = unsafe { ReadProcessMemory(h.0, at as *const c_void, out.as_mut_ptr().cast(), count * 2, &mut got) != 0 };
        (ok && got == count * 2).then_some(out)
    }

    /// The end of the page holding `from` and how many whole UTF-16 units
    /// lie before it; `None` at the top of the address space.
    fn page_step(from: usize) -> Option<(usize, usize)> {
        let page_end = (from | 0xfff).checked_add(1)?;
        Some((page_end, (page_end - from) / 2))
    }

    /// The environment block at `at`, read a page at a time up to its
    /// closing double NUL (its recorded size can lag behind a change).
    fn read_environment(h: &Handle, at: usize) -> Option<Vec<u16>> {
        let mut block: Vec<u16> = Vec::new();
        let mut from = at;
        loop {
            let Some((page_end, units)) = page_step(from) else {
                return (!block.is_empty()).then_some(block);
            };
            let Some(part) = read_units(h, from, units) else {
                return (!block.is_empty()).then_some(block);
            };
            let scan = block.len().saturating_sub(1);
            block.extend_from_slice(&part);
            if let Some(end) = block[scan..].windows(2).position(|w| w == [0, 0]) {
                block.truncate(scan + end + 2);
                return Some(block);
            }
            if block.len() * 2 >= MAX_ENVIRONMENT {
                return Some(block);
            }
            from = page_end;
        }
    }

    /// The command line and the environment from their raw units, each on
    /// its own: one that could not be read does not hide the other.
    fn parts(command: Option<Vec<u16>>, environment: Option<Vec<u16>>) -> super::CommandAndEnvironment {
        (
            command.map(|u| String::from_utf16_lossy(&u)).ok_or(Missing::Unavailable),
            environment.map(|e| super::parse_environment(&e)).ok_or(Missing::Unavailable),
        )
    }

    pub fn command_line_and_environment(pid: u32) -> super::CommandAndEnvironment {
        match process_parameters(pid) {
            Ok((h, params)) => {
                let cmd = &params.command_line;
                parts(read_units(&h, cmd.buf, cmd.len as usize / 2), read_environment(&h, params.environment))
            }
            Err(m) => (Err(m), Err(m)),
        }
    }

    /// The process opened for reading and its RTL_USER_PROCESS_PARAMETERS.
    fn process_parameters(pid: u32) -> Result<(Handle, Params), Missing> {
        let h = open(pid, PROCESS_VM_READ)?;
        let query = ntdll_query(c"NtQueryInformationProcess").ok_or(Missing::Unavailable)?;
        // SAFETY: all-zero is a valid BasicInfo (integers only).
        let mut basic: BasicInfo = unsafe { std::mem::zeroed() };
        let mut len = 0u32;
        // SAFETY: `basic` is writable for its size and `len` is an out-param.
        let status = unsafe { query(h.0, PROCESS_BASIC_INFORMATION, (&mut basic as *mut BasicInfo).cast(), std::mem::size_of::<BasicInfo>() as u32, &mut len) };
        if status < 0 || basic.peb == 0 {
            return Err(Missing::Unavailable);
        }
        let peb: PebHead = read_struct(&h, basic.peb).ok_or(Missing::Unavailable)?;
        let params: Params = read_struct(&h, peb.process_parameters).ok_or(Missing::Unavailable)?;
        Ok((h, params))
    }

    /// The system's handle list as raw words; `None` when it cannot be read.
    fn handle_list() -> Option<Vec<u64>> {
        let query = query()?;
        let mut buf = vec![0u64; 4 << 20 >> 3];
        loop {
            let bytes = (buf.len() * 8) as u32;
            let mut len = 0u32;
            // SAFETY: the buffer is writable for `bytes` bytes and `len` is a
            // valid out-param.
            let status = unsafe { query(SYSTEM_EXTENDED_HANDLE_INFORMATION, buf.as_mut_ptr() as *mut c_void, bytes, &mut len) };
            if status == LENGTH_MISMATCH && buf.len() < 512 << 20 >> 3 {
                buf = vec![0; (len as usize).max(bytes as usize * 2).div_ceil(8) + 8192];
                continue;
            }
            return (status >= 0).then_some(buf);
        }
    }

    /// Whether `s` holds text lying wholly in `start..end`, at an address a
    /// `u16` slice may start at.
    fn text_fits(s: &UStr, start: usize, end: usize) -> bool {
        s.len != 0 && s.buf.is_multiple_of(2) && s.buf >= start && s.buf.checked_add(s.len as usize).is_some_and(|e| e <= end)
    }

    /// What NtQueryObject says for `class` about `h`: the UNICODE_STRING
    /// that leads both the name and the type answers.
    fn object_text(h: HANDLE, class: u32) -> Option<String> {
        let query = ntdll_query(c"NtQueryObject")?;
        let mut buf = vec![0u64; 0x1000 >> 3];
        loop {
            let mut len = 0u32;
            let bytes = (buf.len() * 8) as u32;
            // SAFETY: the buffer is writable for `bytes` bytes and `len` is a
            // valid out-param.
            let status = unsafe { query(h, class, buf.as_mut_ptr().cast(), bytes, &mut len) };
            if (status == LENGTH_MISMATCH || status == BUFFER_OVERFLOW) && len > bytes && len < 1 << 20 {
                buf = vec![0; (len as usize).div_ceil(8) + 1];
                continue;
            }
            if status < 0 {
                return None;
            }
            // SAFETY: on success the buffer starts with a UNICODE_STRING
            // whose text lies in this same buffer, `len` bytes long.
            let s = unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const UStr) };
            let start = buf.as_ptr() as usize;
            let end = start + buf.len() * 8;
            if !text_fits(&s, start, end) {
                return Some(String::new());
            }
            // SAFETY: the text was checked to lie inside `buf`, aligned.
            let units = unsafe { std::slice::from_raw_parts(s.buf as *const u16, s.len as usize / 2) };
            return Some(String::from_utf16_lossy(units));
        }
    }

    /// A handle duplicated into this process, which the name helper may
    /// own on another thread.
    struct Dup(Handle);
    // SAFETY: a kernel handle is a process-wide value, usable from any
    // thread; `Dup` owns it alone and closes it once.
    unsafe impl Send for Dup {}

    /// A handle's NT name as the Inspector lists it: drive paths for
    /// files, HKLM / HKU for keys, sockets and pipes as named.
    fn resource(kind: ResourceKind, name: String, drives: &[(String, String)]) -> Resource {
        let lower = name.to_ascii_lowercase();
        match kind {
            ResourceKind::Key => {
                let short = [(r"\registry\machine", "HKLM"), (r"\registry\user", "HKU")]
                    .iter()
                    .find(|(nt, _)| lower.starts_with(nt))
                    .map(|(nt, hive)| format!("{hive}{}", &name[nt.len()..]));
                Resource { kind, name: short.unwrap_or(name) }
            }
            _ if lower.starts_with(r"\device\afd") || lower.starts_with(r"\device\namedpipe") => Resource { kind: ResourceKind::Socket, name },
            _ => Resource { kind, name: super::dos_path(&name, drives) },
        }
    }

    pub fn open_resources(pid: u32, each: std::time::Duration) -> Result<Vec<Resource>, Missing> {
        let target = open(pid, PROCESS_DUP_HANDLE)?;
        let list = handle_list().ok_or(Missing::Unavailable)?;
        let base = list.as_ptr() as *const u8;
        let total = list.len() * 8;
        // SAFETY: the list starts with its handle count.
        let count = unsafe { std::ptr::read_unaligned(base as *const usize) };
        let head = 2 * std::mem::size_of::<usize>();
        let size = std::mem::size_of::<HandleEntry>();
        let count = count.min(total.saturating_sub(head) / size);
        // Each type is asked once: "File" and "Key" are listed.
        let mut types: HashMap<u16, Option<ResourceKind>> = HashMap::new();
        let mut kinds = Vec::new();
        let mut dups = Vec::new();
        // SAFETY: GetCurrentProcess returns a pseudo-handle that needs no close.
        let me = unsafe { GetCurrentProcess() };
        for i in 0..count {
            // SAFETY: entry `i` lies inside the list (count was clamped to it).
            let e = unsafe { std::ptr::read_unaligned(base.add(head + i * size) as *const HandleEntry) };
            if e.pid != pid as usize || types.get(&e.type_index) == Some(&None) {
                continue;
            }
            let mut dup: HANDLE = std::ptr::null_mut();
            // SAFETY: `target` was opened with PROCESS_DUP_HANDLE and `dup`
            // is an out-param; a failure leaves it null.
            if unsafe { DuplicateHandle(target.0, e.value as HANDLE, me, &mut dup, 0, 0, DUPLICATE_SAME_ACCESS) } == 0 || dup.is_null() {
                continue;
            }
            let dup = Handle(dup);
            // A type query does not block, unlike a name query.
            let kind = *types.entry(e.type_index).or_insert_with(|| match object_text(dup.0, OBJECT_TYPE_INFORMATION).as_deref() {
                Some("File") => Some(ResourceKind::File),
                Some("Key") => Some(ResourceKind::Key),
                _ => None,
            });
            if let Some(kind) = kind {
                kinds.push(kind);
                dups.push(Dup(dup));
            }
        }
        let names = super::name_each(dups, std::sync::Arc::new(|d: &Dup| object_text(d.0 .0, OBJECT_NAME_INFORMATION)), each);
        let drives = super::drive_devices();
        let mut out: Vec<Resource> = kinds
            .into_iter()
            .zip(names)
            .filter_map(|(kind, named)| match named {
                Named::Name(name) => Some(resource(kind, name, &drives)),
                Named::Nameless => None,
                Named::Stuck => Some(Resource { kind, name: "Unavailable".into() }),
            })
            .collect();
        out.sort();
        out.dedup();
        Ok(out)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_page_step_stops_at_the_top_of_the_address_space() {
            assert_eq!(page_step(0x1000), Some((0x2000, 0x800)));
            assert_eq!(page_step(0x1ffe), Some((0x2000, 1)));
            assert_eq!(page_step(usize::MAX - 1), None, "the last page has no end to step to");
        }

        #[test]
        fn reading_no_units_gives_an_empty_text_even_at_a_null_buffer() {
            let h = open(std::process::id(), PROCESS_VM_READ).expect("the test's own process");
            assert_eq!(read_units(&h, 0, 0), Some(Vec::new()));
        }

        #[test]
        fn an_object_name_at_an_odd_address_is_not_read_as_text() {
            let s = |buf: usize, len: u16| UStr { len, max: len, buf };
            assert!(text_fits(&s(0x1000, 4), 0x1000, 0x1010));
            assert!(!text_fits(&s(0x1001, 4), 0x1000, 0x1010), "odd address");
            assert!(!text_fits(&s(0x100e, 4), 0x1000, 0x1010), "past the end");
            assert!(!text_fits(&s(0x1000, 0), 0x1000, 0x1010), "empty");
        }

        #[test]
        fn an_unreadable_environment_does_not_hide_the_command_line() {
            let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
            let (command, environment) = parts(Some(units("app.exe --x")), None);
            assert_eq!(command, Ok("app.exe --x".to_string()));
            assert_eq!(environment, Err(Missing::Unavailable));
            let (command, environment) = parts(None, Some(units("A=1\0\0")));
            assert_eq!(command, Err(Missing::Unavailable));
            assert_eq!(environment, Ok(vec![("A".to_string(), "1".to_string())]));
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::Counts;
    use std::collections::HashMap;

    pub fn counts(_buf: &mut Vec<u64>) -> HashMap<u32, Counts> {
        HashMap::new()
    }

    pub fn thread_ids(_pid: u32) -> Option<Vec<u32>> {
        None
    }

    pub fn identity(_pid: u32) -> (&'static str, u16) {
        ("", 0)
    }

    pub fn priority_class(_pid: u32) -> Option<&'static str> {
        None
    }

    pub fn windows() -> super::Windows {
        Default::default()
    }

    pub fn bring_to_front(_pid: u32) -> bool {
        false
    }

    pub fn command_line_and_environment(_pid: u32) -> super::CommandAndEnvironment {
        (Err(super::Missing::Unavailable), Err(super::Missing::Unavailable))
    }

    pub fn open_resources(_pid: u32, _each: std::time::Duration) -> Result<Vec<super::Resource>, super::Missing> {
        Err(super::Missing::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn reading_the_tests_own_process_gives_its_threads_handles_architecture_and_cpu_time() {
        // Spend some CPU so the process has a CPU time to show.
        let start = std::time::Instant::now();
        let mut x = 0u64;
        while start.elapsed() < std::time::Duration::from_millis(50) {
            x = x.wrapping_mul(31).wrapping_add(7);
        }
        std::hint::black_box(x);
        let mut rows = vec![ProcRow { pid: std::process::id(), ..Default::default() }];
        Reader::default().fill(&mut rows);
        let me = &rows[0];
        assert!(me.threads >= 1, "threads: {}", me.threads);
        assert!(me.handles > 0, "handles: {}", me.handles);
        assert!(me.peak_mem > 0, "peak memory: {}", me.peak_mem);
        assert!(me.page_faults > 0, "page faults: {}", me.page_faults);
        assert!(me.cpu_time > std::time::Duration::ZERO, "CPU time: {:?}", me.cpu_time);
        let arch = if cfg!(target_arch = "x86_64") {
            "x64"
        } else if cfg!(target_arch = "aarch64") {
            "ARM64"
        } else {
            "x86"
        };
        assert_eq!(me.arch, arch);
        // CI runners start tests below normal priority, so compare with the
        // class Windows reports for this process rather than a fixed name.
        let own = crate::winsys::get_priority(std::process::id()).expect("the test can read its own priority");
        assert_eq!(me.priority, own);
    }

    #[cfg(windows)]
    #[test]
    fn a_suspended_child_process_reads_suspended_and_the_tests_own_reads_running() {
        use std::os::windows::process::CommandExt;
        const CREATE_SUSPENDED: u32 = 0x4;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new("cmd.exe")
            .args(["/c", "exit"])
            .creation_flags(CREATE_SUSPENDED | CREATE_NO_WINDOW)
            .spawn()
            .expect("start a suspended cmd.exe");
        let mut rows = vec![ProcRow { pid: child.id(), ..Default::default() }, ProcRow { pid: std::process::id(), ..Default::default() }];
        // The new thread is Ready for a moment before it takes its suspend
        // and waits: a later sample sees it (seen on a loaded machine).
        let mut reader = Reader::default();
        let start = std::time::Instant::now();
        loop {
            reader.fill(&mut rows);
            if rows[0].status == "Suspended" || start.elapsed() > std::time::Duration::from_secs(5) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(rows[0].status, "Suspended", "threads: {}", rows[0].threads);
        assert_eq!(rows[1].status, "Running");
    }

    #[test]
    fn a_live_process_reads_running_suspended_or_not_responding_never_terminated() {
        assert_eq!(status_of(false, false), "Running");
        assert_eq!(status_of(true, false), "Suspended");
        assert_eq!(status_of(false, true), "Not responding");
        assert_eq!(status_of(true, true), "Suspended", "a suspended app's window hangs because it is suspended");
        // No source of the Processes page names the old sysinfo status.
        let gone = concat!("\"Termin", "ated\"");
        for (file, text) in [("sampler.rs", include_str!("sampler.rs")), ("proc_info.rs", include_str!("proc_info.rs")), ("process_view.rs", include_str!("process_view.rs")), ("pages.rs", include_str!("pages.rs"))] {
            assert!(!text.contains(gone), "{file} still names {gone}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn the_tests_own_window_less_process_is_a_background_process() {
        let mut rows = vec![ProcRow { pid: std::process::id(), app: true, ..Default::default() }];
        Reader::default().fill(&mut rows);
        assert!(!rows[0].app, "the test runner owns no window");
    }

    #[cfg(windows)]
    #[test]
    fn reading_the_tests_own_process_gives_the_details_panes_facts() {
        // Read a file so the process has I/O to count.
        let exe = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        std::hint::black_box(exe);
        let mut rows = vec![ProcRow { pid: std::process::id(), ..Default::default() }];
        Reader::default().fill(&mut rows);
        let me = &rows[0];
        assert_eq!(me.kernel_time + me.user_time, me.cpu_time, "kernel plus user time is the CPU time");
        assert!(me.user_time > std::time::Duration::ZERO);
        assert!(me.working_set > 0 && me.working_set <= me.peak_mem, "working set {} within peak {}", me.working_set, me.peak_mem);
        assert!(me.commit > 0, "commit size: {}", me.commit);
        assert!(me.io_read_ops > 0 && me.io_read > 0, "I/O read: {} ops, {} bytes", me.io_read_ops, me.io_read);
        // The base priority must match the process's own priority class
        // (CI runners start tests below normal, base 6, instead of 8).
        assert_eq!(class_of_base(me.base_priority), me.priority, "base priority {} matches the priority class", me.base_priority);
        let mut session = 0u32;
        // SAFETY: `session` is a valid out-param.
        assert_ne!(unsafe { ProcessIdToSessionId(std::process::id(), &mut session) }, 0);
        assert_eq!(me.session, session);
    }

    #[cfg(windows)]
    #[test]
    fn the_tests_own_environment_and_command_line_are_read_from_its_peb() {
        let name = format!("UPM_INSPECTOR_TEST_{}", std::process::id());
        std::env::set_var(&name, "seen by the inspector");
        let (command, environment) = command_line_and_environment(std::process::id());
        let (command, environment) = (command.expect("the test's own command line"), environment.expect("the test's own environment"));
        let exe = std::env::current_exe().unwrap();
        let stem = exe.file_stem().unwrap().to_string_lossy().to_lowercase();
        assert!(command.to_lowercase().contains(&stem), "{command:?} names {stem}");
        assert!(environment.contains(&(name.clone(), "seen by the inspector".to_string())), "{name} in {} variables", environment.len());
        assert!(environment.iter().any(|(k, _)| k.eq_ignore_ascii_case("PATH")), "PATH is listed");
    }

    #[cfg(windows)]
    #[test]
    fn the_tests_own_open_file_is_listed_among_its_open_resources() {
        let path = std::env::temp_dir().join(format!("upm-inspector-{}-{:?}.txt", std::process::id(), std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()));
        let file = std::fs::File::create(&path).unwrap();
        let wanted = path.file_name().unwrap().to_string_lossy().to_lowercase();
        let resources = open_resources(std::process::id(), Duration::from_millis(500)).expect("the test's own handles");
        drop(file);
        let _ = std::fs::remove_file(&path);
        let found = resources.iter().find(|r| r.name.to_lowercase().ends_with(&wanted)).unwrap_or_else(|| panic!("{wanted} in {resources:?}"));
        assert_eq!(found.kind, ResourceKind::File);
        assert!(found.name.chars().nth(1) == Some(':'), "a drive path: {}", found.name);
        assert!(resources.iter().any(|r| r.kind == ResourceKind::Key), "a registry key is open too");
    }

    #[test]
    fn a_name_query_that_never_answers_cannot_hold_up_the_rest() {
        // Item 1 blocks forever, as a named pipe's name query can.
        let query: NameQuery<u32> = std::sync::Arc::new(|i: &u32| match i {
            1 => loop {
                std::thread::park();
            },
            3 => None,
            i => Some(format!("name {i}")),
        });
        let start = std::time::Instant::now();
        let names = name_each(vec![0, 1, 2, 3], query, Duration::from_millis(100));
        assert!(start.elapsed() < Duration::from_secs(2), "took {:?}", start.elapsed());
        assert_eq!(names, [Named::Name("name 0".into()), Named::Stuck, Named::Name("name 2".into()), Named::Nameless]);
    }

    #[test]
    fn stuck_name_queries_keep_the_live_helpers_under_one_cap_across_calls() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, RwLock};
        static TEST_HELPERS: Helpers = Helpers::new(6);
        let gate = Arc::new(RwLock::new(()));
        let shut = gate.write().unwrap();
        let blocked = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let query: NameQuery<u32> = {
            let (gate, blocked, peak) = (gate.clone(), blocked.clone(), peak.clone());
            Arc::new(move |_: &u32| {
                peak.fetch_max(blocked.fetch_add(1, Ordering::SeqCst) + 1, Ordering::SeqCst);
                drop(gate.read());
                blocked.fetch_sub(1, Ordering::SeqCst);
                None
            })
        };
        for _ in 0..5 {
            let names = name_each_in(&TEST_HELPERS, (0..20).collect(), query.clone(), Duration::from_millis(1));
            assert!(names.iter().all(|n| *n == Named::Stuck), "{names:?}");
            assert!(TEST_HELPERS.live() <= 6, "{} helpers alive", TEST_HELPERS.live());
        }
        assert!(peak.load(Ordering::SeqCst) <= 6, "{} asks blocked at once", peak.load(Ordering::SeqCst));
        drop(shut);
        let start = std::time::Instant::now();
        while TEST_HELPERS.live() > 0 && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(TEST_HELPERS.live(), 0, "every released helper ended");
    }

    #[cfg(windows)]
    #[link(name = "kernel32")]
    extern "system" {
        fn ProcessIdToSessionId(pid: u32, session: *mut u32) -> i32;
    }
}
