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
#[allow(dead_code)] // the Windows reader and the tests use it
pub fn dos_path(name: &str, drives: &[(String, String)]) -> String {
    let lower = name.to_ascii_lowercase();
    drives
        .iter()
        .find(|(device, _)| lower.starts_with(&device.to_ascii_lowercase()) && name[device.len()..].starts_with('\\'))
        .map_or_else(|| name.to_string(), |(device, drive)| format!("{drive}{}", &name[device.len()..]))
}

/// The NT names of the drives ("\Device\HarddiskVolume3" → "C:"), for
/// [`dos_path`].
#[allow(unused_imports)] // read by the Windows trace
pub use imp::drive_devices;

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
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

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
        let own = crate::process_actions::get_priority(std::process::id()).expect("the test can read its own priority");
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
        for (file, text) in [("sampler.rs", include_str!("sampler.rs")), ("proc_info.rs", include_str!("proc_info.rs")), ("process_view.rs", include_str!("process_view.rs"))] {
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
