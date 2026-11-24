//! Linux: the per-process facts, read from `/proc/<pid>/` through an
//! [`FsRoot`] (so tests read a fixture tree, never the running system).
//!
//! [`ProcFiles`] is the per-process reader: one method per file of the
//! process (`status`, `stat`, `io`, `cgroup`, `fd`, `cmdline`, `environ`,
//! `cwd`, `exe`), each returning `None` when the file is missing or the
//! process may not be asked for it. The facts that need permission (the I/O
//! counters, the open files, the executable) are simply missing for another
//! user's process, and the row keeps its defaults: they are hidden, not
//! guessed. The functions below it build what the process view asks for
//! (`counts`, `identity`, `windows`, ...) out of those reads; a later fact
//! is one more method on [`ProcFiles`] and one more line there.
//!
//! # Apps and Background processes
//!
//! A process is an App when it lives in a desktop application unit of the
//! user's service manager: a cgroup component named `app-*.scope` or
//! `app-*.service`. When the cgroup says nothing about a user session (no
//! `.slice` in it: a minimal system, a container), the fallback is: the
//! process belongs to a user (not root, not a system account), has no
//! controlling terminal, and its environment names a display
//! (`DISPLAY` or `WAYLAND_DISPLAY`).

use super::{CommandAndEnvironment, Counts, Missing, Resource, ResourceKind, Windows};
use crate::fs_root::FsRoot;
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::Duration;

/// The kernel's clock tick (`getconf CLK_TCK`): 100 on every mainstream
/// build, and the unit of the CPU times and of the start time in `stat`.
const TICKS_PER_SECOND: u64 = 100;
/// The start time of a process in the 100 ns units [`Counts::created`] uses.
const UNITS_PER_TICK: i64 = 10_000_000 / TICKS_PER_SECOND as i64;
/// The first user ID of a person; below it are the system's accounts.
const FIRST_USER_UID: u32 = 1000;

/// The facts of `/proc/<pid>/status` that the process view uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    /// The real user ID.
    pub uid: u32,
    /// The resident memory, in bytes.
    pub resident: u64,
    /// The peak resident memory, in bytes.
    pub resident_peak: u64,
    /// The resident memory that is not backed by a file, in bytes.
    pub anonymous: u64,
    /// The size of the data segment (private writable memory), in bytes.
    pub data: u64,
}

/// The facts of `/proc/<pid>/stat that the process view uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub state: char,
    /// The session ID.
    pub session: u32,
    /// The controlling terminal's device number; 0 when there is none.
    pub tty: u32,
    pub minor_faults: u64,
    pub major_faults: u64,
    /// CPU time in user mode, in ticks.
    pub user_ticks: u64,
    /// CPU time in kernel mode, in ticks.
    pub kernel_ticks: u64,
    /// The nice value, -20 (most favourable) to 19.
    pub nice: i32,
    pub threads: u32,
    /// When the process started, in ticks after boot.
    pub start_ticks: u64,
    /// The scheduling policy: 0 normal, 1 FIFO and 2 round-robin (the
    /// real-time ones), 3 batch, 5 idle...
    pub policy: u32,
}

/// The facts of `/proc/<pid>/io`, which only the process's owner may read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Io {
    pub read_bytes: u64,
    pub write_bytes: u64,
    pub read_ops: u64,
    pub write_ops: u64,
}

/// The reader of one process's `/proc/<pid>/` folder.
pub struct ProcFiles<'a> {
    root: &'a FsRoot,
    pid: u32,
}

impl<'a> ProcFiles<'a> {
    pub fn new(root: &'a FsRoot, pid: u32) -> Self {
        Self { root, pid }
    }

    fn rel(&self, file: &str) -> String {
        format!("proc/{}/{file}", self.pid)
    }

    /// Whether the process is there at all.
    pub fn exists(&self) -> bool {
        self.root.probe(&format!("proc/{}", self.pid))
    }

    /// The whole file, as bytes (the kernel's lists end in NUL bytes that
    /// `FsRoot::read` would trim).
    fn bytes(&self, file: &str) -> std::io::Result<Vec<u8>> {
        std::fs::read(self.root.path(&self.rel(file)))
    }

    pub fn status(&self) -> Option<Status> {
        parse_status(&self.root.read(&self.rel("status"))?)
    }

    pub fn stat(&self) -> Option<Stat> {
        parse_stat(&self.root.read(&self.rel("stat"))?)
    }

    pub fn io(&self) -> Option<Io> {
        parse_io(&self.root.read(&self.rel("io"))?)
    }

    /// The process's control group path (like
    /// `/user.slice/user-1000.slice/.../app-firefox-1.scope`).
    pub fn cgroup(&self) -> Option<String> {
        cgroup_path(&self.root.read(&self.rel("cgroup"))?)
    }

    /// How many files and sockets the process holds open (`fd`).
    pub fn open_files(&self) -> Option<u32> {
        let entries = std::fs::read_dir(self.root.path(&self.rel("fd"))).ok()?;
        Some(entries.filter(|e| e.is_ok()).count() as u32)
    }

    /// The command line, one word per argument.
    pub fn command_line(&self) -> Result<Vec<String>, Missing> {
        let words = split_words(&self.bytes("cmdline").map_err(|e| self.missing(&e))?);
        if words.is_empty() { Err(Missing::Unavailable) } else { Ok(words) }
    }

    /// The environment variables, in the order they were given.
    pub fn environment(&self) -> Result<Vec<(String, String)>, Missing> {
        let words = split_words(&self.bytes("environ").map_err(|e| self.missing(&e))?);
        Ok(words
            .into_iter()
            .filter_map(|w| w.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
            .collect())
    }

    /// The files, sockets and pipes the process holds open (`fd`), in
    /// descriptor order. Sockets are named from the system's socket tables.
    pub fn open_resources(&self) -> Result<Vec<Resource>, Missing> {
        let entries = std::fs::read_dir(self.root.path(&self.rel("fd"))).map_err(|e| self.missing(&e))?;
        let mut links: Vec<(u32, String)> = entries
            .filter_map(|e| {
                let entry = e.ok()?;
                let number = entry.file_name().to_str()?.parse().ok()?;
                let target = std::fs::read_link(entry.path()).ok()?;
                Some((number, target.to_string_lossy().into_owned()))
            })
            .collect();
        links.sort();
        let sockets = SocketNames::read(self.root);
        let mut list: Vec<Resource> = links.into_iter().filter_map(|(_, target)| resource_of(&target, &sockets)).collect();
        list.sort();
        list.dedup();
        Ok(list)
    }

    /// The process's working folder.
    pub fn cwd(&self) -> Option<PathBuf> {
        std::fs::read_link(self.root.path(&self.rel("cwd"))).ok()
    }

    /// The path of the process's program file.
    // Read by the Inspector and the publisher/description columns.
    #[allow(dead_code)]
    pub fn exe(&self) -> Option<PathBuf> {
        std::fs::read_link(self.root.path(&self.rel("exe"))).ok()
    }

    /// The architecture the program file was built for, from its ELF header.
    pub fn architecture(&self) -> &'static str {
        let Ok(mut file) = std::fs::File::open(self.root.path(&self.rel("exe"))) else { return "" };
        let mut header = [0u8; 20];
        if file.read_exact(&mut header).is_err() {
            return "";
        }
        architecture_of(&header)
    }

    /// Why a read failed: permission, the process having ended, or neither.
    fn missing(&self, error: &std::io::Error) -> Missing {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => Missing::Denied,
            std::io::ErrorKind::NotFound if !self.exists() => Missing::Exited,
            _ => Missing::Unavailable,
        }
    }

    /// Whether the process is an App (see the module docs).
    pub fn is_app(&self) -> bool {
        match self.cgroup() {
            Some(path) if path.split('/').any(|c| c.ends_with(".slice")) => path.split('/').any(is_app_unit),
            _ => self.looks_like_desktop_app(),
        }
    }

    /// The fallback of [`Self::is_app`] for a system without a user session
    /// in the cgroups.
    fn looks_like_desktop_app(&self) -> bool {
        let user = self.status().is_some_and(|s| s.uid >= FIRST_USER_UID);
        let no_terminal = self.stat().is_some_and(|s| s.tty == 0);
        user && no_terminal
            && self
                .environment()
                .is_ok_and(|e| e.iter().any(|(k, v)| (k == "DISPLAY" || k == "WAYLAND_DISPLAY") && !v.is_empty()))
    }
}

/// A desktop application's unit of the service manager (`app-gnome-x-12.scope`).
fn is_app_unit(component: &str) -> bool {
    component.starts_with("app-") && (component.ends_with(".scope") || component.ends_with(".service"))
}

/// The NUL-separated words of `cmdline` and `environ`.
fn split_words(bytes: &[u8]) -> Vec<String> {
    bytes
        .split(|&b| b == 0)
        .filter(|w| !w.is_empty())
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect()
}

/// The path of the unified (or, failing that, the systemd) hierarchy in a
/// `cgroup` file.
pub fn cgroup_path(text: &str) -> Option<String> {
    let mut systemd = None;
    for line in text.lines() {
        let mut parts = line.splitn(3, ':');
        let (_, controllers, path) = (parts.next()?, parts.next()?, parts.next()?);
        if controllers.is_empty() {
            return Some(path.to_string());
        }
        if controllers.contains("name=systemd") {
            systemd = Some(path.to_string());
        }
    }
    systemd
}

fn kib(value: &str) -> u64 {
    value.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) * 1024
}

/// The parts of `/proc/<pid>/status` the view uses; `None` without a state.
pub fn parse_status(text: &str) -> Option<Status> {
    let mut status = Status { uid: 0, resident: 0, resident_peak: 0, anonymous: 0, data: 0 };
    let mut has_state = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key {
            "State" => has_state = !value.is_empty(),
            "Uid" => status.uid = value.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0),
            "VmRSS" => status.resident = kib(value),
            "VmHWM" => status.resident_peak = kib(value),
            "RssAnon" => status.anonymous = kib(value),
            "VmData" => status.data = kib(value),
            _ => {}
        }
    }
    has_state.then_some(status)
}

/// The parts of `/proc/<pid>/stat` the view uses. The name (field 2) may hold
/// spaces and parentheses, so the rest is read after the last `)`.
pub fn parse_stat(text: &str) -> Option<Stat> {
    let rest = text.rsplit_once(')')?.1;
    let f: Vec<&str> = rest.split_whitespace().collect();
    // f[0] is field 3 (the state); field n is f[n - 3].
    let num = |n: usize| f.get(n - 3).and_then(|v| v.parse::<i64>().ok());
    Some(Stat {
        state: f.first()?.chars().next()?,
        session: num(6)? as u32,
        tty: num(7)? as u32,
        minor_faults: num(10)? as u64,
        major_faults: num(12)? as u64,
        user_ticks: num(14)? as u64,
        kernel_ticks: num(15)? as u64,
        nice: num(19)? as i32,
        threads: num(20)? as u32,
        start_ticks: num(22)? as u64,
        policy: num(41).unwrap_or(0) as u32,
    })
}

/// The counters of `/proc/<pid>/io`.
pub fn parse_io(text: &str) -> Option<Io> {
    let mut io = Io { read_bytes: 0, write_bytes: 0, read_ops: 0, write_ops: 0 };
    let mut seen = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let Ok(value) = value.trim().parse::<u64>() else { continue };
        match key {
            "rchar" => io.read_bytes = value,
            "wchar" => io.write_bytes = value,
            "syscr" => io.read_ops = value,
            "syscw" => io.write_ops = value,
            _ => continue,
        }
        seen = true;
    }
    seen.then_some(io)
}

/// The architecture named by the first 20 bytes of an ELF file: `""` for a
/// file that is not ELF or built for a machine the view has no word for.
pub fn architecture_of(header: &[u8; 20]) -> &'static str {
    if header[..4] != *b"\x7fELF" {
        return "";
    }
    let machine = match header[5] {
        1 => u16::from_le_bytes([header[18], header[19]]),
        2 => u16::from_be_bytes([header[18], header[19]]),
        _ => return "",
    };
    match machine {
        3 => "x86",
        62 => "x64",
        40 => "ARM",
        183 => "ARM64",
        243 => "RISC-V",
        _ => "",
    }
}

/// The priority class (the words of the process view) of a nice value and
/// scheduling policy: a real-time policy is Realtime, then the nice value
/// decides.
pub fn priority_class_of(policy: u32, nice: i32) -> &'static str {
    match (policy, nice) {
        (1 | 2, _) => "Realtime",
        (_, ..=-10) => "High",
        (_, -9..=-1) => "Above normal",
        (_, 0) => "Normal",
        (_, 1..=9) => "Below normal",
        _ => "Low",
    }
}

/// The base priority (on the scale the view's fallback reads) of a class.
fn base_priority_of(class: &str) -> i32 {
    match class {
        "Realtime" => 24,
        "High" => 13,
        "Above normal" => 10,
        "Normal" => 8,
        "Below normal" => 6,
        _ => 4,
    }
}

/// Everything the view takes from one process; `None` when it has ended
/// (or its `stat` cannot be read).
fn counts_of(files: &ProcFiles) -> Option<Counts> {
    let stat = files.stat()?;
    let status = files.status();
    let io = files.io();
    let tick = |t: u64| Duration::from_millis(t * 1000 / TICKS_PER_SECOND);
    let class = priority_class_of(stat.policy, stat.nice);
    let mut c = Counts {
        threads: stat.threads,
        handles: files.open_files().unwrap_or(0),
        page_faults: (stat.minor_faults + stat.major_faults).min(u32::MAX as u64) as u32,
        cpu_time: tick(stat.user_ticks + stat.kernel_ticks),
        kernel_time: tick(stat.kernel_ticks),
        user_time: tick(stat.user_ticks),
        session: stat.session,
        base_priority: base_priority_of(class),
        created: stat.start_ticks as i64 * UNITS_PER_TICK,
        suspended: stat.state == 'T',
        ..Counts::default()
    };
    if let Some(s) = status {
        c.working_set = s.resident;
        c.peak_mem = s.resident_peak;
        c.commit = s.data;
        c.private_ws = s.anonymous;
    }
    if let Some(io) = io {
        c.io_read = io.read_bytes;
        c.io_write = io.write_bytes;
        c.io_read_ops = io.read_ops;
        c.io_write_ops = io.write_ops;
    }
    Some(c)
}

/// The IDs of the processes under the root.
fn pids(root: &FsRoot) -> Vec<u32> {
    let Ok(entries) = std::fs::read_dir(root.path("proc")) else { return Vec::new() };
    entries.filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect()
}

/// One read of every process.
pub fn counts_in(root: &FsRoot) -> HashMap<u32, Counts> {
    pids(root).into_iter().filter_map(|pid| Some((pid, counts_of(&ProcFiles::new(root, pid))?))).collect()
}

/// The processes listed as Apps; Linux has no hung-window check and the
/// view's "Bring to front" has no window walk to ask yet.
pub fn windows_in(root: &FsRoot) -> Windows {
    let apps = pids(root).into_iter().filter(|&pid| ProcFiles::new(root, pid).is_app()).collect();
    Windows { apps, ..Windows::default() }
}

/// The command line (words joined by spaces) and the environment of one process.
pub fn command_line_and_environment_in(root: &FsRoot, pid: u32) -> CommandAndEnvironment {
    let files = ProcFiles::new(root, pid);
    (files.command_line().map(|words| words.join(" ")), files.environment())
}

/// The priority class of one process.
pub fn priority_class_in(root: &FsRoot, pid: u32) -> Option<&'static str> {
    let stat = ProcFiles::new(root, pid).stat()?;
    Some(priority_class_of(stat.policy, stat.nice))
}

/// One read of every process.
pub fn counts(_buf: &mut Vec<u64>) -> HashMap<u32, Counts> {
    counts_in(&FsRoot::system())
}

/// Not available yet.
pub fn thread_ids(_pid: u32) -> Option<Vec<u32>> {
    None
}

/// The architecture of the process's program, and processor group 0 (Linux
/// has no processor groups).
pub fn identity(pid: u32) -> (&'static str, u16) {
    (ProcFiles::new(&FsRoot::system(), pid).architecture(), 0)
}

/// The priority class of the process, from its nice value and policy.
pub fn priority_class(pid: u32) -> Option<&'static str> {
    priority_class_in(&FsRoot::system(), pid)
}

/// The Apps among the processes.
pub fn windows() -> super::Windows {
    windows_in(&FsRoot::system())
}

/// Not available yet.
pub fn bring_to_front(_pid: u32) -> bool {
    false
}

/// The command line and the environment of the process, from `/proc`.
pub fn command_line_and_environment(pid: u32) -> super::CommandAndEnvironment {
    command_line_and_environment_in(&FsRoot::system(), pid)
}

/// The open files, sockets and pipes of one process.
pub fn open_resources_in(root: &FsRoot, pid: u32) -> Result<Vec<Resource>, Missing> {
    ProcFiles::new(root, pid).open_resources()
}

/// The open files, sockets and pipes of the process, from `/proc`.
pub fn open_resources(pid: u32, _each: std::time::Duration) -> Result<Vec<Resource>, Missing> {
    open_resources_in(&FsRoot::system(), pid)
}

/// The working folder of the process, from `/proc`.
pub fn working_folder(pid: u32) -> Option<String> {
    ProcFiles::new(&FsRoot::system(), pid).cwd().map(|p| p.to_string_lossy().into_owned())
}

/// How one descriptor's link target shows in the Inspector: a path is a
/// file, a socket or pipe is a socket; anonymous kernel objects (events,
/// timers) are left out.
fn resource_of(target: &str, sockets: &SocketNames) -> Option<Resource> {
    if let Some(inode) = target.strip_prefix("socket:[").and_then(|t| t.strip_suffix(']')) {
        let name = sockets.0.get(inode).cloned().unwrap_or_else(|| target.to_string());
        return Some(Resource { kind: ResourceKind::Socket, name });
    }
    if target.starts_with("pipe:[") {
        return Some(Resource { kind: ResourceKind::Socket, name: target.to_string() });
    }
    target.starts_with('/').then(|| Resource { kind: ResourceKind::File, name: target.to_string() })
}

/// The names of the system's sockets by inode, from `/proc/net`.
struct SocketNames(HashMap<String, String>);

impl SocketNames {
    fn read(root: &FsRoot) -> Self {
        let mut names = HashMap::new();
        for (file, protocol) in [("tcp", "TCP"), ("tcp6", "TCP"), ("udp", "UDP"), ("udp6", "UDP")] {
            if let Some(text) = root.read(&format!("proc/net/{file}")) {
                parse_inet_sockets(&text, protocol, &mut names);
            }
        }
        if let Some(text) = root.read("proc/net/unix") {
            parse_unix_sockets(&text, &mut names);
        }
        SocketNames(names)
    }
}

/// `address:port` from the kernel's hex form (little-endian words).
fn inet_address(hex: &str) -> Option<String> {
    let (addr, port) = hex.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let word = |i: usize| u32::from_str_radix(addr.get(i * 8..i * 8 + 8)?, 16).ok().map(u32::swap_bytes);
    match addr.len() {
        8 => Some(format!("{}:{port}", std::net::Ipv4Addr::from(word(0)?))),
        32 => {
            let ip = std::net::Ipv6Addr::from(u128::from(word(0)?) << 96 | u128::from(word(1)?) << 64 | u128::from(word(2)?) << 32 | u128::from(word(3)?));
            Some(format!("[{ip}]:{port}"))
        }
        _ => None,
    }
}

fn tcp_state(code: &str) -> &'static str {
    match code {
        "01" => "established",
        "02" => "syn sent",
        "03" => "syn received",
        "04" => "fin wait 1",
        "05" => "fin wait 2",
        "06" => "time wait",
        "07" => "closed",
        "08" => "close wait",
        "09" => "last ack",
        "0A" => "listening",
        "0B" => "closing",
        _ => "",
    }
}

/// Reads one of `/proc/net/tcp`, `tcp6`, `udp`, `udp6` into inode -> name.
pub fn parse_inet_sockets(text: &str, protocol: &str, names: &mut HashMap<String, String>) {
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (Some(local), Some(remote), Some(state), Some(inode)) = (f.get(1), f.get(2), f.get(3), f.get(9)) else { continue };
        let (Some(local), Some(remote)) = (inet_address(local), inet_address(remote)) else { continue };
        let state = if protocol == "TCP" { tcp_state(state) } else { "" };
        let name = match (state, protocol) {
            ("listening", _) => format!("{protocol} {local} listening"),
            (_, "UDP") => format!("{protocol} {local}"),
            ("", _) => format!("{protocol} {local} -> {remote}"),
            (state, _) => format!("{protocol} {local} -> {remote} ({state})"),
        };
        names.insert((*inode).to_string(), name);
    }
}

/// Reads `/proc/net/unix` into inode -> name.
pub fn parse_unix_sockets(text: &str, names: &mut HashMap<String, String>) {
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        let Some(inode) = f.get(6) else { continue };
        let name = match f.get(7) {
            Some(path) => format!("Unix socket {path}"),
            None => "Unix socket".to_string(),
        };
        names.insert((*inode).to_string(), name);
    }
}

/// The NT names of the drives: none, not available yet.
#[allow(dead_code)] // read by the Windows trace
pub fn drive_devices() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn counts_of_fixture(pid: u32) -> Counts {
        counts_in(&fixture("desktop")).remove(&pid).unwrap_or_else(|| panic!("no process {pid}"))
    }

    #[test]
    fn the_facts_of_the_users_process_are_filled() {
        let c = counts_of_fixture(1000);
        assert_eq!(c.threads, 5);
        assert_eq!(c.handles, 3, "the open files");
        assert_eq!(c.session, 1000);
        assert_eq!(c.page_faults, 1230, "minor and major faults");
        assert_eq!((c.user_time, c.kernel_time, c.cpu_time), (Duration::from_millis(1500), Duration::from_millis(500), Duration::from_secs(2)));
        assert_eq!((c.working_set, c.peak_mem), (160_000 * 1024, 200_000 * 1024));
        assert_eq!((c.private_ws, c.commit), (90_000 * 1024, 300_000 * 1024));
        assert_eq!((c.io_read, c.io_write, c.io_read_ops, c.io_write_ops), (5000, 3000, 40, 20), "the I/O counters");
        assert_eq!(c.created, 123_456 * 100_000, "the start time, in 100 ns units");
        assert!(!c.suspended);
    }

    #[test]
    fn the_facts_that_need_permission_are_hidden_for_another_users_process() {
        let c = counts_of_fixture(1003);
        // Public facts are still there...
        assert_eq!((c.threads, c.working_set), (1, 5000 * 1024));
        // ...the ones the kernel keeps for the owner are not.
        assert_eq!(c.handles, 0);
        assert_eq!((c.io_read, c.io_write, c.io_read_ops, c.io_write_ops), (0, 0, 0, 0));
        assert_eq!(ProcFiles::new(&fixture("desktop"), 1003).architecture(), "");
        assert_eq!(ProcFiles::new(&fixture("desktop"), 1003).open_files(), None);
    }

    #[test]
    fn the_architecture_comes_from_the_executables_elf_header() {
        let root = fixture("desktop");
        assert_eq!(identity_in(&root, 1000), ("x64", 0));
        assert_eq!(identity_in(&root, 1001), ("ARM", 0), "a 32-bit ARM program");
        assert_eq!(identity_in(&root, 1002), ("ARM64", 0));
        assert_eq!(identity_in(&root, 4_000_000), ("", 0), "a process that is not there");
        let mut header = [0u8; 20];
        assert_eq!(architecture_of(&header), "", "not an ELF file");
        header[..4].copy_from_slice(b"\x7fELF");
        header[5] = 1;
        header[18] = 3;
        assert_eq!(architecture_of(&header), "x86");
        header[5] = 2;
        (header[18], header[19]) = (0, 62);
        assert_eq!(architecture_of(&header), "x64", "a big-endian header is read big-endian");
    }

    fn identity_in(root: &FsRoot, pid: u32) -> (&'static str, u16) {
        (ProcFiles::new(root, pid).architecture(), 0)
    }

    #[test]
    fn a_process_in_a_desktop_app_scope_is_an_app_and_a_service_is_background() {
        let w = windows_in(&fixture("desktop"));
        assert!(w.apps.contains(&1000), "app-*.scope");
        for background in [1001, 1002, 1003] {
            assert!(!w.apps.contains(&background), "{background}");
        }
        assert!(w.hung.is_empty(), "no hung-window check on Linux: Not responding stays hidden");
    }

    #[test]
    fn without_a_user_session_in_the_cgroups_a_display_without_a_terminal_makes_an_app() {
        let w = windows_in(&fixture("desktop"));
        assert!(w.apps.contains(&1004), "a user, no terminal, a display");
        assert!(!w.apps.contains(&1005), "a terminal makes it a command, not an app");
    }

    #[test]
    fn the_cgroup_of_either_hierarchy_is_found() {
        assert_eq!(cgroup_path("0::/a/b.slice/c.scope\n").as_deref(), Some("/a/b.slice/c.scope"));
        let v1 = "12:cpu,cpuacct:/user\n1:name=systemd:/user.slice/app-x-1.scope\n";
        assert_eq!(cgroup_path(v1).as_deref(), Some("/user.slice/app-x-1.scope"));
        assert_eq!(cgroup_path("12:cpu:/user\n"), None);
    }

    #[test]
    fn a_stopped_process_is_suspended_and_others_are_not() {
        assert!(counts_of_fixture(1002).suspended, "state T");
        assert!(!counts_of_fixture(1000).suspended);
        let mut rows = [crate::sampler::ProcRow { pid: 1002, ..Default::default() }, crate::sampler::ProcRow { pid: 1000, ..Default::default() }];
        let counts = counts_in(&fixture("desktop"));
        let windows = windows_in(&fixture("desktop"));
        for row in rows.iter_mut() {
            let stopped = counts.get(&row.pid).is_some_and(|c| c.suspended);
            row.status = super::super::status_of(stopped, windows.hung.contains(&row.pid)).into();
        }
        assert_eq!((rows[0].status.as_str(), rows[1].status.as_str()), ("Suspended", "Running"));
    }

    #[test]
    fn the_priority_class_comes_from_the_nice_value_and_policy() {
        let root = fixture("desktop");
        assert_eq!(priority_class_in(&root, 1000), Some("Normal"));
        assert_eq!(priority_class_in(&root, 1007), Some("High"), "nice -12");
        assert_eq!(priority_class_in(&root, 1008), Some("Below normal"), "nice 5");
        assert_eq!(priority_class_in(&root, 1002), Some("Low"), "nice 19");
        assert_eq!(priority_class_in(&root, 1001), Some("Above normal"), "nice -5");
        assert_eq!(priority_class_in(&root, 1006), Some("Realtime"), "a real-time policy, whatever the name");
        assert_eq!(priority_class_in(&root, 4_000_000), None);
        assert_eq!(counts_of_fixture(1000).base_priority, 8);
    }

    #[test]
    fn a_name_with_a_parenthesis_does_not_shift_the_fields() {
        let c = counts_of_fixture(1006);
        assert_eq!((c.session, c.threads), (1006, 1));
    }

    #[test]
    fn the_command_line_and_the_environment_are_read_from_proc() {
        let root = fixture("desktop");
        let (cmd, env) = command_line_and_environment_in(&root, 1000);
        assert_eq!(cmd.as_deref(), Ok("/usr/lib/firefox/firefox --new-window https://example.org"));
        assert_eq!(env.unwrap()[1], ("WAYLAND_DISPLAY".to_string(), "wayland-0".to_string()));
        let (cmd, env) = command_line_and_environment_in(&root, 1003);
        assert_eq!(cmd.as_deref(), Ok("sshd: /usr/sbin/sshd -D"), "the command line is public");
        assert_eq!(env, Err(Missing::Unavailable), "the environment is not there to read");
        assert_eq!(command_line_and_environment_in(&root, 4_000_000), (Err(Missing::Exited), Err(Missing::Exited)));
    }

    #[test]
    fn only_numbered_folders_are_processes() {
        let counts = counts_in(&fixture("desktop"));
        let mut pids: Vec<u32> = counts.keys().copied().collect();
        pids.sort();
        assert_eq!(pids, [1000, 1001, 1002, 1003, 1004, 1005, 1006, 1007, 1008]);
    }

    #[test]
    fn the_working_folder_and_the_program_are_read_through_their_links() {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("upm-proc-links-{}", std::process::id()));
        let process = dir.join("proc/42");
        std::fs::create_dir_all(&process).unwrap();
        symlink("/home/user/project", process.join("cwd")).unwrap();
        symlink("/usr/bin/tool", process.join("exe")).unwrap();
        let root = FsRoot::at(&dir);
        let files = ProcFiles::new(&root, 42);
        assert_eq!(files.cwd(), Some(PathBuf::from("/home/user/project")));
        assert_eq!(files.exe(), Some(PathBuf::from("/usr/bin/tool")));
        assert_eq!(ProcFiles::new(&root, 43).cwd(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `/proc` of process 42 with descriptors and the socket tables.
    fn resources_root(name: &str) -> PathBuf {
        use std::os::unix::fs::symlink;
        let dir = std::env::temp_dir().join(format!("upm-proc-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let fd = dir.join("proc/42/fd");
        std::fs::create_dir_all(&fd).unwrap();
        std::fs::create_dir_all(dir.join("proc/net")).unwrap();
        for (n, target) in [
            ("0", "/dev/null"),
            ("1", "/home/user/notes.txt"),
            ("2", "socket:[111]"),
            ("3", "socket:[222]"),
            ("4", "pipe:[333]"),
            ("5", "anon_inode:[eventpoll]"),
            ("6", "socket:[999]"),
            ("7", "socket:[444]"),
            ("10", "/var/log/app.log"),
        ] {
            symlink(target, fd.join(n)).unwrap();
        }
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n   0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 111 1 0000 100 0 0 10 0\n   1: 0100007F:D2A4 0100007F:0050 01 00000000:00000000 00:00000000 00000000  1000        0 222 1 0000 100 0 0 10 0\n";
        std::fs::write(dir.join("proc/net/tcp"), tcp).unwrap();
        std::fs::write(dir.join("proc/net/udp"), "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode ref pointer drops\n").unwrap();
        let tcp6 = "  sl  local_address rem_address st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode\n   0: 00000000000000000000000001000000:0016 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000 0 0 555 1 0 100 0 0 10 0\n";
        std::fs::write(dir.join("proc/net/tcp6"), tcp6).unwrap();
        let unix = "Num       RefCount Protocol Flags    Type St Inode Path\n0000000000000000: 00000002 00000000 00010000 0001 01 444 /run/user/1000/bus\n";
        std::fs::write(dir.join("proc/net/unix"), unix).unwrap();
        dir
    }

    #[test]
    fn the_open_files_sockets_and_pipes_are_listed_with_sockets_named_from_the_tables() {
        let dir = resources_root("resources");
        let list = open_resources_in(&FsRoot::at(&dir), 42).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let shown: Vec<(ResourceKind, &str)> = list.iter().map(|r| (r.kind, r.name.as_str())).collect();
        assert_eq!(
            shown,
            [
                (ResourceKind::File, "/dev/null"),
                (ResourceKind::File, "/home/user/notes.txt"),
                (ResourceKind::File, "/var/log/app.log"),
                (ResourceKind::Socket, "TCP 127.0.0.1:53924 -> 127.0.0.1:80 (established)"),
                (ResourceKind::Socket, "TCP 127.0.0.1:8080 listening"),
                (ResourceKind::Socket, "Unix socket /run/user/1000/bus"),
                (ResourceKind::Socket, "pipe:[333]"),
                (ResourceKind::Socket, "socket:[999]"),
            ],
            "sorted by kind and name; anonymous kernel objects left out; an unknown socket keeps the kernel's name"
        );
    }

    #[test]
    fn an_ipv6_socket_is_named_with_its_address_and_port() {
        let mut names = HashMap::new();
        parse_inet_sockets(
            "h\n   0: 00000000000000000000000001000000:0016 00000000000000000000000000000000:0000 0A 0:0 0:0 0 0 0 555 1\n",
            "TCP",
            &mut names,
        );
        assert_eq!(names["555"], "TCP [::1]:22 listening");
    }

    #[test]
    fn the_open_resources_of_a_gone_process_say_exited() {
        assert_eq!(open_resources_in(&fixture("desktop"), 4_000_000), Err(Missing::Exited));
    }

    #[test]
    #[ignore = "reads the running system's /proc"]
    fn the_view_fills_its_rows_from_the_system_read() {
        // On the running system: the test's own process is listed, its facts
        // are filled and its program is a known architecture.
        let mut rows = [crate::sampler::ProcRow { pid: std::process::id(), ..Default::default() }];
        super::super::Reader::default().fill(&mut rows);
        assert!(rows[0].threads >= 1 && rows[0].working_set > 0, "{:?}", rows[0].threads);
        assert_eq!(rows[0].status, "Running");
        assert!(!rows[0].arch.is_empty());
        assert!(!rows[0].priority.is_empty());
    }
}
