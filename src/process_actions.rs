//! Actions on a running process: End task, priority, processor affinity,
//! Suspend and Resume, Efficiency mode, memory dumps, UAC virtualization and
//! the shell's Windows Explorer.
//!
//! The module holds the shared texts and types and the rules that do not
//! depend on the system; the `windows`, `linux` and `macos` submodules do
//! the calls (Linux and macOS report that the action is not available yet).

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as system;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as system;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as system;

pub use system::{
    end_process, get_affinity, get_priority, resume_process, set_affinity, set_efficiency, set_priority, shell_pid, suspend_process,
    toggle_uac_virtualization,
};
#[cfg(all(test, windows))]
pub use system::efficiency_on;

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
#[cfg(windows)]
pub const AFFINITY_FAILED: &str = "Windows could not change the processor affinity.";

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
    system::write_dump_file(pid, &path).map(|()| path)
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
        use crate::process_menu::{menu_entries, MenuEntry, MenuState};
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
        // A child that has only just started cannot be dumped until its
        // start-up is done (Windows answers "partial copy"): under load that
        // takes longer, so the dump is asked for again until it is written.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let written = loop {
            let attempt = write_dump(pid, "PING.EXE", &dir);
            if attempt.is_ok() || std::time::Instant::now() > until {
                break attempt;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
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
