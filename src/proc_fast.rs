//! The faster process interfaces of Linux, used only where the kernel has
//! them: process start and exit notifications (the process-events
//! connector) and acting on a process through a process file descriptor
//! (`pidfd_open` and `pidfd_send_signal`).
//!
//! Which of them exist is found by probing ([`probe`]): a file that must
//! exist and a call that must succeed, never the kernel version. Where the
//! probe finds nothing, [`PidTracker`] keeps listing `/proc` and the
//! actions keep signalling by process ID, with the same results.
//!
//! Everything that touches the kernel is behind [`Kernel`], so tests drive
//! both paths with a fake one; [`SystemKernel`] is the real one.

use crate::fs_root::FsRoot;
use std::collections::BTreeSet;

/// A process started or ended (the process ID is the thread-group ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    Started(u32),
    Exited(u32),
}

/// What a read of the notification source brought.
#[derive(Debug, Default, PartialEq)]
pub struct Drained {
    pub events: Vec<Event>,
    /// The kernel dropped notifications: the list must be read again.
    pub lost: bool,
}

/// A source of process notifications.
pub trait EventSource: Send {
    /// Everything that arrived since the last call; never waits.
    fn drain(&mut self) -> Drained;
}

/// The kernel calls the faster interfaces need.
pub trait Kernel: Send + Sync {
    /// Whether a process file descriptor can be opened (tried on this process).
    fn pidfd_works(&self) -> bool;
    /// Sends signal number `signal` to `pid` through a process file descriptor.
    fn pidfd_signal(&self, pid: u32, signal: i32) -> Result<(), String>;
    /// Subscribes to process notifications, when the kernel lets this user.
    fn open_events(&self) -> Option<Box<dyn EventSource>>;
}

/// What the probe found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub pidfd: bool,
    pub events: bool,
}

/// Asks the system which faster interfaces it has. The notification
/// interface needs its file under `/proc/net` and a subscription that is
/// accepted (it is refused to ordinary users on some systems).
pub fn probe(kernel: &dyn Kernel, root: &FsRoot) -> Capabilities {
    Capabilities { pidfd: kernel.pidfd_works(), events: root.probe("proc/net/connector") && kernel.open_events().is_some() }
}

/// The number of the signal named `name` (`TERM`, `KILL`, `STOP`, `CONT`).
pub fn signal_number(name: &str) -> Option<i32> {
    Some(match name {
        "TERM" => 15,
        "KILL" => 9,
        "STOP" => 19,
        "CONT" => 18,
        _ => return None,
    })
}

/// Sends the signal named `name` to `pid` through a process file
/// descriptor when the probe found them; `None` means "use the fallback"
/// (not found, an unknown signal name, or the call failed: the fallback
/// then says why in its usual words).
pub fn signal_fast(kernel: &dyn Kernel, caps: Capabilities, pid: u32, name: &str) -> Option<()> {
    if !caps.pidfd {
        return None;
    }
    kernel.pidfd_signal(pid, signal_number(name)?).ok()
}

/// The IDs of the processes under the root, from a plain scan of `/proc`.
pub fn scan(root: &FsRoot) -> BTreeSet<u32> {
    root.list_dir("proc").iter().filter_map(|n| n.parse().ok()).collect()
}

/// The list of running processes: kept up to date by notifications where
/// there are some, read from `/proc` every time otherwise.
pub struct PidTracker {
    source: Option<Box<dyn EventSource>>,
    known: Option<BTreeSet<u32>>,
}

impl PidTracker {
    pub fn new(source: Option<Box<dyn EventSource>>) -> Self {
        PidTracker { source, known: None }
    }

    /// The processes now. The first answer and every answer after lost
    /// notifications come from a scan; the others from the events alone.
    pub fn pids(&mut self, root: &FsRoot) -> BTreeSet<u32> {
        let Some(source) = self.source.as_mut() else { return scan(root) };
        let drained = source.drain();
        let set = match (self.known.take(), drained.lost) {
            (Some(mut set), false) => {
                for event in drained.events {
                    match event {
                        Event::Started(pid) => set.insert(pid),
                        Event::Exited(pid) => set.remove(&pid),
                    };
                }
                set
            }
            _ => scan(root),
        };
        self.known = Some(set.clone());
        set
    }
}

/// Reads the datagram of the process-events connector: one netlink
/// message holding one connector message holding one event.
pub fn parse_datagram(bytes: &[u8]) -> Vec<Event> {
    const NLMSG_HDR: usize = 16;
    const CN_MSG: usize = 20;
    const EVENT_DATA: usize = NLMSG_HDR + CN_MSG + 16;
    const FORK: u32 = 0x1;
    const EXIT: u32 = 0x8000_0000;
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(what), Some(a), Some(b), Some(c), Some(d)) = (word(NLMSG_HDR + CN_MSG), word(EVENT_DATA), word(EVENT_DATA + 4), word(EVENT_DATA + 8), word(EVENT_DATA + 12)) else {
        return Vec::new();
    };
    match what {
        // parent pid, parent tgid, child pid, child tgid: a new process, not a thread.
        FORK if c == d => vec![Event::Started(d)],
        // process pid, process tgid: the whole process ends with its main thread.
        EXIT if a == b => vec![Event::Exited(b)],
        _ => Vec::new(),
    }
}

/// The real kernel.
pub struct SystemKernel;

impl Kernel for SystemKernel {
    fn pidfd_works(&self) -> bool {
        // SAFETY: plain system calls on this process's own ID and the
        // descriptor they return.
        unsafe {
            let fd = libc::syscall(libc::SYS_pidfd_open, libc::getpid(), 0);
            if fd < 0 {
                return false;
            }
            libc::close(fd as i32);
        }
        true
    }

    fn pidfd_signal(&self, pid: u32, signal: i32) -> Result<(), String> {
        // SAFETY: the descriptor is opened, used once and closed here.
        unsafe {
            let fd = libc::syscall(libc::SYS_pidfd_open, pid as libc::pid_t, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error().to_string());
            }
            let sent = libc::syscall(libc::SYS_pidfd_send_signal, fd as i32, signal, std::ptr::null::<libc::siginfo_t>(), 0);
            let error = std::io::Error::last_os_error();
            libc::close(fd as i32);
            if sent < 0 {
                return Err(error.to_string());
            }
        }
        Ok(())
    }

    fn open_events(&self) -> Option<Box<dyn EventSource>> {
        Connector::open().map(|c| Box::new(c) as Box<dyn EventSource>)
    }
}

/// A subscription to process notifications (a netlink connector socket).
struct Connector {
    fd: i32,
}

impl Connector {
    fn open() -> Option<Connector> {
        const NETLINK_CONNECTOR: i32 = 11;
        const CN_IDX_PROC: u32 = 1;
        // SAFETY: a socket created, bound and closed on failure here; the
        // structures passed are fully initialised and sized correctly.
        unsafe {
            let fd = libc::socket(libc::AF_NETLINK, libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC, NETLINK_CONNECTOR);
            if fd < 0 {
                return None;
            }
            let mut address: libc::sockaddr_nl = std::mem::zeroed();
            address.nl_family = libc::AF_NETLINK as u16;
            address.nl_groups = CN_IDX_PROC;
            address.nl_pid = libc::getpid() as u32;
            let bound = libc::bind(fd, &address as *const _ as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as u32);
            // nlmsghdr (16) + cn_msg (20) + the "listen" operation (4).
            let mut message = Vec::with_capacity(40);
            message.extend_from_slice(&40u32.to_ne_bytes());
            message.extend_from_slice(&3u16.to_ne_bytes()); // NLMSG_DONE
            message.extend_from_slice(&0u16.to_ne_bytes());
            message.extend_from_slice(&0u32.to_ne_bytes());
            message.extend_from_slice(&address.nl_pid.to_ne_bytes());
            message.extend_from_slice(&CN_IDX_PROC.to_ne_bytes()); // cn id idx
            message.extend_from_slice(&1u32.to_ne_bytes()); // cn id val
            message.extend_from_slice(&0u32.to_ne_bytes()); // seq
            message.extend_from_slice(&0u32.to_ne_bytes()); // ack
            message.extend_from_slice(&4u16.to_ne_bytes()); // len
            message.extend_from_slice(&0u16.to_ne_bytes()); // flags
            message.extend_from_slice(&1u32.to_ne_bytes()); // PROC_CN_MCAST_LISTEN
            let sent = if bound == 0 { libc::send(fd, message.as_ptr() as *const _, message.len(), 0) } else { -1 };
            if sent < 0 {
                libc::close(fd);
                return None;
            }
            Some(Connector { fd })
        }
    }
}

impl EventSource for Connector {
    fn drain(&mut self) -> Drained {
        let mut out = Drained::default();
        let mut buffer = [0u8; 1024];
        loop {
            // SAFETY: reads into a buffer of the size given.
            let n = unsafe { libc::recv(self.fd, buffer.as_mut_ptr() as *mut _, buffer.len(), 0) };
            if n < 0 {
                if std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOBUFS) {
                    out.lost = true;
                    continue;
                }
                return out;
            }
            out.events.extend(parse_datagram(&buffer[..n as usize]));
        }
    }
}

impl Drop for Connector {
    fn drop(&mut self) {
        // SAFETY: the descriptor is this value's own.
        unsafe { libc::close(self.fd) };
    }
}

/// The process list of the running system: notified where the probe found
/// the interface, scanned otherwise. Shared by every refresh.
pub fn system_pids() -> BTreeSet<u32> {
    use std::sync::{Mutex, OnceLock};
    static TRACKER: OnceLock<Mutex<PidTracker>> = OnceLock::new();
    let root = FsRoot::system();
    let tracker = TRACKER.get_or_init(|| {
        let source = if probe(&SystemKernel, &root).events { SystemKernel.open_events() } else { None };
        Mutex::new(PidTracker::new(source))
    });
    tracker.lock().map(|mut t| t.pids(&root)).unwrap_or_else(|_| scan(&root))
}

/// The capabilities of the running system, probed once.
pub fn system_capabilities() -> Capabilities {
    static CAPS: std::sync::OnceLock<Capabilities> = std::sync::OnceLock::new();
    *CAPS.get_or_init(|| probe(&SystemKernel, &FsRoot::system()))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A kernel whose answers the test sets and whose signals it reads.
    #[derive(Default)]
    pub struct FakeKernel {
        pub pidfd: bool,
        pub events: Mutex<Option<Vec<Drained>>>,
        pub signals: Mutex<Vec<(u32, i32)>>,
        pub fail: bool,
    }

    struct Scripted(Vec<Drained>);
    impl EventSource for Scripted {
        fn drain(&mut self) -> Drained {
            if self.0.is_empty() { Drained::default() } else { self.0.remove(0) }
        }
    }

    impl Kernel for FakeKernel {
        fn pidfd_works(&self) -> bool {
            self.pidfd
        }
        fn pidfd_signal(&self, pid: u32, signal: i32) -> Result<(), String> {
            self.signals.lock().unwrap().push((pid, signal));
            if self.fail { Err("no".into()) } else { Ok(()) }
        }
        fn open_events(&self) -> Option<Box<dyn EventSource>> {
            self.events.lock().unwrap().take().map(|s| Box::new(Scripted(s)) as Box<dyn EventSource>)
        }
    }

    pub fn kernel(pidfd: bool, events: Option<Vec<Drained>>) -> FakeKernel {
        FakeKernel { pidfd, events: Mutex::new(events), ..Default::default() }
    }

    const BOTH: Capabilities = Capabilities { pidfd: true, events: false };

    #[test]
    fn the_probe_finds_each_interface_by_trying_it_not_by_the_kernel_version() {
        let with_connector = crate::fs_root::fixture("procs-events");
        let without = crate::fs_root::fixture("procs");
        assert_eq!(probe(&kernel(true, Some(vec![])), &with_connector), Capabilities { pidfd: true, events: true });
        assert_eq!(probe(&kernel(false, Some(vec![])), &with_connector), Capabilities { pidfd: false, events: true });
        // The file is there but the subscription is refused (not root).
        assert_eq!(probe(&kernel(true, None), &with_connector), Capabilities { pidfd: true, events: false });
        // No connector file: the subscription is not even tried.
        let k = kernel(true, Some(vec![]));
        assert_eq!(probe(&k, &without), Capabilities { pidfd: true, events: false });
        assert!(k.events.lock().unwrap().is_some());
    }

    #[test]
    fn with_notifications_the_list_follows_starts_and_exits_after_one_scan() {
        let root = crate::fs_root::fixture("procs");
        let all = scan(&root);
        assert!(all.contains(&100));
        let script = vec![Drained::default(), Drained { events: vec![Event::Started(5000), Event::Exited(100)], lost: false }];
        let mut tracker = PidTracker::new(Some(Box::new(Scripted(script))));
        assert_eq!(tracker.pids(&root), all);
        let mut expected = all.clone();
        expected.insert(5000);
        expected.remove(&100);
        assert_eq!(tracker.pids(&root), expected, "5000 is not under the root: only the event can have added it");
    }

    #[test]
    fn lost_notifications_make_the_tracker_read_the_list_again() {
        let root = crate::fs_root::fixture("procs");
        let script = vec![Drained::default(), Drained { events: vec![Event::Started(5000)], lost: true }];
        let mut tracker = PidTracker::new(Some(Box::new(Scripted(script))));
        tracker.pids(&root);
        assert_eq!(tracker.pids(&root), scan(&root));
    }

    #[test]
    fn without_notifications_the_list_is_the_plain_scan_and_gives_the_same_answer() {
        let root = crate::fs_root::fixture("procs");
        let mut plain = PidTracker::new(None);
        let mut fast = PidTracker::new(Some(Box::new(Scripted(vec![]))));
        assert_eq!(plain.pids(&root), fast.pids(&root));
    }

    #[test]
    fn a_signal_goes_through_the_process_descriptor_only_when_the_probe_found_it() {
        let k = kernel(true, None);
        assert_eq!(signal_fast(&k, BOTH, 42, "STOP"), Some(()));
        assert_eq!(*k.signals.lock().unwrap(), vec![(42, 19)]);
        let k = kernel(false, None);
        assert_eq!(signal_fast(&k, Capabilities::default(), 42, "STOP"), None);
        assert!(k.signals.lock().unwrap().is_empty());
        // A failed call or an unknown name leaves the work to the fallback.
        let failing = FakeKernel { pidfd: true, fail: true, ..Default::default() };
        assert_eq!(signal_fast(&failing, BOTH, 42, "TERM"), None);
        assert_eq!(signal_fast(&kernel(true, None), BOTH, 42, "HUP"), None);
    }

    #[test]
    fn signal_names_have_their_linux_numbers() {
        assert_eq!(["TERM", "KILL", "STOP", "CONT"].map(signal_number), [Some(15), Some(9), Some(19), Some(18)]);
    }

    /// A connector datagram: the two headers, the event kind, the CPU and
    /// time fields, then the four words of the event.
    fn datagram(what: u32, words: [u32; 4]) -> Vec<u8> {
        let mut m = vec![0u8; 36];
        m.extend_from_slice(&what.to_ne_bytes());
        m.extend_from_slice(&[0u8; 12]);
        for w in words {
            m.extend_from_slice(&w.to_ne_bytes());
        }
        m
    }

    #[test]
    fn datagrams_say_which_processes_started_and_ended_and_ignore_threads() {
        assert_eq!(parse_datagram(&datagram(1, [1, 1, 700, 700])), vec![Event::Started(700)]);
        assert_eq!(parse_datagram(&datagram(1, [1, 1, 701, 700])), vec![]);
        assert_eq!(parse_datagram(&datagram(0x8000_0000, [700, 700, 0, 17])), vec![Event::Exited(700)]);
        assert_eq!(parse_datagram(&datagram(0x8000_0000, [701, 700, 0, 17])), vec![]);
        assert_eq!(parse_datagram(&datagram(2, [700, 700, 0, 0])), vec![]);
        assert_eq!(parse_datagram(&[1, 2, 3]), vec![]);
    }
}
