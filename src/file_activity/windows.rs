//! Windows: the ETW session: a real-time session of its own (a unique name
//! per window) on Microsoft-Windows-Kernel-File, consumed on the worker
//! thread, its lost-event count read every half second.

use crate::file_activity::{now_local_ms, Feed, FileEvent, Op, Stopper, TraceMsg};
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
use windows_sys::Win32::System::Diagnostics::Etw::*;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

/// Every session this app starts is named this, then "<app PID> <n>".
pub const PREFIX: &str = "UltimatePCManager File activity ";
/// Microsoft-Windows-Kernel-File.
const KERNEL_FILE: GUID = GUID::from_u128(0xedd08927_9cc4_4e65_b970_c2560fb5c289);
/// KERNEL_FILE_KEYWORD_CREATE, _READ, _WRITE, _DELETE_PATH and
/// _RENAME_SETLINK_PATH.
const KEYWORDS: u64 = 0x80 | 0x100 | 0x200 | 0x400 | 0x800;
/// The provider's events this window reads: Create, Read, Write,
/// DeletePath, RenamePath.
const CREATE: u16 = 12;
const READ: u16 = 15;
const WRITE: u16 = 16;
const DELETE_PATH: u16 = 26;
const RENAME_PATH: u16 = 27;
/// OpenTraceW's failure.
const INVALID_TRACE: u64 = u64::MAX;
const ERROR_INVALID_PARAMETER: u32 = 87;
const ERROR_MORE_DATA: u32 = 234;
const STILL_ACTIVE: u32 = 259;
/// File objects remembered by name before the map starts over.
const NAMES_CAP: usize = 200_000;

static NEXT: AtomicU32 = AtomicU32::new(0);

use crate::win_ffi::wide;

/// An EVENT_TRACE_PROPERTIES with room for a session and a file name
/// after it, aligned for the structure.
struct Props(Vec<u64>);

impl Props {
    const NAME_BYTES: usize = 1024 * 2;

    fn new() -> Props {
        let head = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
        let mut buf = vec![0u64; (head + 2 * Self::NAME_BYTES).div_ceil(8)];
        let bytes = (buf.len() * 8) as u32;
        let p = buf.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES;
        // SAFETY: `buf` is zeroed, 8-aligned and larger than the
        // structure.
        unsafe {
            (*p).Wnode.BufferSize = bytes;
            (*p).LoggerNameOffset = head as u32;
            (*p).LogFileNameOffset = (head + Self::NAME_BYTES) as u32;
        }
        Props(buf)
    }

    fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.0.as_mut_ptr() as *mut EVENT_TRACE_PROPERTIES
    }

    fn get(&self) -> &EVENT_TRACE_PROPERTIES {
        // SAFETY: as in `new`.
        unsafe { &*(self.0.as_ptr() as *const EVENT_TRACE_PROPERTIES) }
    }

    /// The session name Windows wrote after the structure.
    fn logger_name(&self) -> String {
        let at = self.get().LoggerNameOffset as usize;
        let bytes: &[u8] = &as_bytes(&self.0)[at.min(self.0.len() * 8)..];
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
        String::from_utf16_lossy(&units)
    }
}

fn as_bytes(v: &[u64]) -> &[u8] {
    // SAFETY: any u64 slice is also a valid byte slice of 8 times its length.
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 8) }
}

/// Starts a real-time session; its handle, or the Win32 error (5 when
/// not elevated).
pub fn start_session(name: &str) -> Result<u64, u32> {
    let mut props = Props::new();
    let p = props.ptr();
    // SAFETY: `p` points into `props`, alive for the call; the name is
    // NUL-terminated and fits the room after the structure.
    unsafe {
        (*p).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
        (*p).Wnode.ClientContext = 1; // QPC timestamps
        (*p).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
        (*p).BufferSize = 64;
        (*p).FlushTimer = 1;
        let mut handle = CONTROLTRACE_HANDLE { Value: 0 };
        let w = wide(name);
        match StartTraceW(&mut handle, w.as_ptr(), p) {
            0 => Ok(handle.Value),
            e => Err(e),
        }
    }
}

/// Stops the session of `handle` (nothing when it already ended).
pub fn stop_session(handle: u64) {
    let mut props = Props::new();
    // SAFETY: `props` outlives the call; with a handle no name is needed.
    unsafe {
        ControlTraceW(CONTROLTRACE_HANDLE { Value: handle }, std::ptr::null(), props.ptr(), EVENT_TRACE_CONTROL_STOP);
    }
}

fn stop_by_name(name: &str) {
    let mut props = Props::new();
    let w = wide(name);
    // SAFETY: `props` and the NUL-terminated name outlive the call.
    unsafe {
        ControlTraceW(CONTROLTRACE_HANDLE { Value: 0 }, w.as_ptr(), props.ptr(), EVENT_TRACE_CONTROL_STOP);
    }
}

/// The session's count of lost events.
fn lost(handle: u64) -> Option<u64> {
    let mut props = Props::new();
    // SAFETY: `props` outlives the call.
    let e = unsafe { ControlTraceW(CONTROLTRACE_HANDLE { Value: handle }, std::ptr::null(), props.ptr(), EVENT_TRACE_CONTROL_QUERY) };
    (e == 0).then(|| u64::from(props.get().EventsLost))
}

/// The running sessions of this window's kind: (name, the PID of the
/// app that started it).
pub fn sessions() -> Vec<(String, u32)> {
    const SLOTS: usize = 64;
    let mut all: Vec<Props> = (0..SLOTS).map(|_| Props::new()).collect();
    let mut ptrs: Vec<*mut EVENT_TRACE_PROPERTIES> = all.iter_mut().map(Props::ptr).collect();
    let mut count = 0u32;
    // SAFETY: SLOTS pointers, each to a Props sized as its BufferSize says.
    let e = unsafe { QueryAllTracesW(ptrs.as_mut_ptr(), SLOTS as u32, &mut count) };
    if e != 0 && e != ERROR_MORE_DATA {
        return Vec::new();
    }
    all.iter()
        .take(count as usize)
        .filter_map(|props| {
            let name = props.logger_name();
            let owner = name.strip_prefix(PREFIX).and_then(|rest| rest.split(' ').next()).and_then(|s| s.parse::<u32>().ok())?;
            Some((name, owner))
        })
        .collect()
}

/// Stops the sessions of this window's kind left by an app that is gone
/// (killed before it could stop them).
fn stop_stale_sessions() {
    for (name, owner) in sessions() {
        if owner != std::process::id() && !alive(owner) {
            stop_by_name(&name);
        }
    }
}

/// Whether a process of this PID still runs (a process that may not be
/// opened counts as running).
fn alive(pid: u32) -> bool {
    // SAFETY: the handle is checked and closed once.
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return GetLastError() != ERROR_INVALID_PARAMETER;
        }
        let mut code = 0u32;
        let read = GetExitCodeProcess(h, &mut code) != 0;
        CloseHandle(h);
        !read || code == STILL_ACTIVE
    }
}

/// Stops the session when the worker ends, however it ends.
struct StopOnDrop(Arc<Stopper>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// The worker: starts the session, enables the provider, consumes the
/// events until the session stops, then says why it ended.
pub fn run(pid: u32, stopper: Arc<Stopper>, tx: Feed) {
    let _stop = StopOnDrop(stopper.clone());
    stop_stale_sessions();
    let name = format!("{PREFIX}{} {}", std::process::id(), NEXT.fetch_add(1, Ordering::SeqCst));
    let handle = match start_session(&name) {
        Ok(h) => h,
        Err(e) => {
            let _ = tx.send(TraceMsg::Failed(e));
            return;
        }
    };
    if !stopper.register(handle) {
        return;
    }
    // SAFETY: the GUID outlives the call; no filter parameters.
    let e = unsafe {
        EnableTraceEx2(CONTROLTRACE_HANDLE { Value: handle }, &KERNEL_FILE, EVENT_CONTROL_CODE_ENABLE_PROVIDER, TRACE_LEVEL_VERBOSE as u8, KEYWORDS, 0, 0, std::ptr::null())
    };
    if e != 0 {
        stopper.stop();
        let _ = tx.send(TraceMsg::Failed(e));
        return;
    }
    let consumer = Box::into_raw(Box::new(Consumer::new(pid, tx.clone())));
    let mut logger = wide(&name);
    // SAFETY: a zeroed EVENT_TRACE_LOGFILEW is valid; the name lives
    // until OpenTraceW returns; `consumer` lives until ProcessTrace and
    // CloseTrace have returned (below), so every callback finds it.
    let trace = unsafe {
        let mut log: EVENT_TRACE_LOGFILEW = std::mem::zeroed();
        log.LoggerName = logger.as_mut_ptr();
        log.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD;
        log.Anonymous2.EventRecordCallback = Some(on_event);
        log.Context = consumer as *mut c_void;
        OpenTraceW(&mut log).Value
    };
    if trace == INVALID_TRACE {
        // SAFETY: read right after the failed call; `consumer` was never handed out.
        let e = unsafe { GetLastError() };
        // SAFETY: `consumer` came from Box::into_raw above and was never
        // handed to Windows (OpenTraceW failed), so this is its only owner.
        drop(unsafe { Box::from_raw(consumer) });
        stopper.stop();
        let _ = tx.send(TraceMsg::Failed(e));
        return;
    }
    let _ = tx.send(TraceMsg::Started { since_ms: now_local_ms() });
    let poll_stopper = stopper.clone();
    let poll_tx = tx.clone();
    let poller = std::thread::Builder::new().name("file-activity-lost".into()).spawn(move || {
        while !poll_stopper.requested() {
            if let Some(n) = poll_stopper.with_session(lost) {
                if !poll_tx.count(TraceMsg::Lost(n)) {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
    // SAFETY: `trace` is the handle OpenTraceW gave; ProcessTrace
    // returns once the session stops, then the handle is closed once.
    unsafe {
        let handles = [PROCESSTRACE_HANDLE { Value: trace }];
        ProcessTrace(handles.as_ptr(), 1, std::ptr::null(), std::ptr::null());
        CloseTrace(PROCESSTRACE_HANDLE { Value: trace });
        drop(Box::from_raw(consumer));
    }
    let asked = stopper.requested();
    stopper.stop();
    if let Ok(p) = poller {
        let _ = p.join();
    }
    if !asked {
        let _ = tx.send(TraceMsg::Stopped);
    }
}

/// The callback ProcessTrace calls per event.
unsafe extern "system" fn on_event(record: *mut EVENT_RECORD) {
    // A panic must not unwind into Windows.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: ProcessTrace passes a valid record; its UserContext
        // is the Consumer `run` keeps alive until ProcessTrace returns.
        unsafe {
            let record = &*record;
            if let Some(consumer) = (record.UserContext as *mut Consumer).as_mut() {
                consumer.take(record);
            }
        }
    }));
}

fn same_guid(a: &GUID, b: &GUID) -> bool {
    (a.data1, a.data2, a.data3, a.data4) == (b.data1, b.data2, b.data3, b.data4)
}

/// Turns the provider's events of one process into [`FileEvent`]s.
struct Consumer {
    pid: u32,
    tx: Feed,
    /// File object → its path, from the Create events seen.
    names: HashMap<u64, String>,
    /// The NT names of the drives ("\Device\HarddiskVolume3" → "C:").
    drives: Vec<(String, String)>,
    /// Local time ahead of UTC, in milliseconds.
    offset_ms: i64,
    file_object: Vec<u16>,
    file_name: Vec<u16>,
    file_path: Vec<u16>,
    io_size: Vec<u16>,
}

impl Consumer {
    fn new(pid: u32, tx: Feed) -> Consumer {
        Consumer {
            pid,
            tx,
            names: HashMap::new(),
            drives: crate::proc_info::drive_devices(),
            offset_ms: crate::file_activity::local_offset_ms(),
            file_object: wide("FileObject"),
            file_name: wide("FileName"),
            file_path: wide("FilePath"),
            io_size: wide("IOSize"),
        }
    }

    fn take(&mut self, record: &EVENT_RECORD) {
        let header = &record.EventHeader;
        if !same_guid(&header.ProviderId, &KERNEL_FILE) || header.ProcessId != self.pid {
            return;
        }
        let id = header.EventDescriptor.Id;
        if ![CREATE, READ, WRITE, DELETE_PATH, RENAME_PATH].contains(&id) {
            return;
        }
        let object = property(record, &self.file_object).and_then(|b| number(&b));
        let named = |names: &HashMap<u64, String>| object.and_then(|o| names.get(&o).cloned());
        let (op, bytes, path) = match id {
            CREATE => {
                let name = property(record, &self.file_name).map(|b| text(&b)).filter(|n| !n.is_empty()).map(|n| crate::proc_info::dos_path(&n, &self.drives));
                if let (Some(o), Some(n)) = (object, &name) {
                    if self.names.len() >= NAMES_CAP {
                        self.names.clear();
                    }
                    self.names.insert(o, n.clone());
                }
                (Op::Open, 0, name)
            }
            READ | WRITE => {
                let bytes = property(record, &self.io_size).and_then(|b| number(&b)).unwrap_or(0);
                (if id == READ { Op::Read } else { Op::Write }, bytes, named(&self.names))
            }
            _ => {
                let given = || property(record, &self.file_path).map(|b| text(&b)).filter(|n| !n.is_empty()).map(|n| crate::proc_info::dos_path(&n, &self.drives));
                (if id == DELETE_PATH { Op::Delete } else { Op::Rename }, 0, named(&self.names).or_else(given))
            }
        };
        let ms = filetime_ms(header.TimeStamp) + self.offset_ms;
        // Never blocks the callback: on a full feed the event is
        // dropped and counted.
        self.tx.event(FileEvent { ms, op, bytes, path });
    }
}

/// A property of the event by name, as raw bytes (TDH decodes the
/// provider's manifest, whatever the event's version).
fn property(record: &EVENT_RECORD, name: &[u16]) -> Option<Vec<u8>> {
    let desc = PROPERTY_DATA_DESCRIPTOR { PropertyName: name.as_ptr() as u64, ArrayIndex: u32::MAX, Reserved: 0 };
    let mut size = 0u32;
    // SAFETY: `record` is the callback's record; `name` is
    // NUL-terminated; the buffer holds the size TDH asked for.
    unsafe {
        if TdhGetPropertySize(record, 0, std::ptr::null(), 1, &desc, &mut size) != 0 || size == 0 || size > 65_536 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        (TdhGetProperty(record, 0, std::ptr::null(), 1, &desc, size, buf.as_mut_ptr()) == 0).then_some(buf)
    }
}

/// A 4- or 8-byte little-endian number (pointers and sizes).
pub fn number(b: &[u8]) -> Option<u64> {
    match b.len() {
        8 => Some(u64::from_le_bytes(b.try_into().ok()?)),
        4 => Some(u64::from(u32::from_le_bytes(b.try_into().ok()?))),
        _ => None,
    }
}

/// A UTF-16 string up to its NUL.
pub fn text(b: &[u8]) -> String {
    let units: Vec<u16> = b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&u| u != 0).collect();
    String::from_utf16_lossy(&units)
}

/// An event's FILETIME (100 ns since 1601, UTC) as milliseconds since 1970.
pub fn filetime_ms(t: i64) -> i64 {
    t / 10_000 - 11_644_473_600_000
}
