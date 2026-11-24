//! Process actions on Linux: End task and End process tree (SIGTERM, or
//! SIGKILL when forced), Set priority (nice values), Set affinity,
//! Suspend and Resume (SIGSTOP and SIGCONT).
//!
//! Every external program (`kill`, `renice`, `taskset`) is started through
//! one launch function, so the tests see the command lines and start
//! nothing. Acting on another user's process, or raising a priority, asks
//! for administrator permission through `pkexec`, built by
//! [`crate::action_elevation`]. Process facts are read from `/proc` through
//! an [`FsRoot`].
//!
//! Signalling a process is one function, [`signal`]: the only place that
//! knows how a signal reaches a process.

use super::{Affinity, DUMP_FAILED, PRIORITIES, UAC_FAILED};
use crate::action_elevation::{self, Spawned};
use crate::availability::{unavailable_error, Feature};
use crate::fs_root::FsRoot;
use crate::proc_fast::{self, Capabilities, Kernel, SystemKernel};
use crate::system::System;

/// The nice value each entry of [`PRIORITIES`] sets, in the same order.
const NICE_OF: [i32; 6] = [-20, -10, -5, 0, 5, 19];

/// What the actions need from the system: where `/proc` is, how a program
/// is started (and waited for) and the environment.
struct Host<'a> {
    root: &'a FsRoot,
    launch: &'a dyn Fn(&[String]) -> Spawned,
    env: &'a dyn Fn(&str) -> Option<String>,
    /// The kernel and what the probe found in it: the process-descriptor
    /// signals are used only when the probe found them.
    kernel: &'a dyn Kernel,
    caps: Capabilities,
}

/// Runs `f` on the running system.
fn on_system<T>(f: impl FnOnce(&Host) -> T) -> T {
    let root = FsRoot::system();
    let env = |name: &str| std::env::var(name).ok();
    f(&Host { root: &root, launch: &action_elevation::spawn_and_wait, env: &env, kernel: &SystemKernel, caps: proc_fast::system_capabilities() })
}

/// Dumps cannot be written: an error.
pub fn write_dump_file(_: u32, _: &std::path::Path) -> Result<(), String> {
    Err(DUMP_FAILED.into())
}

/// UAC virtualization does not exist here: an error.
pub fn toggle_uac_virtualization(_: u32) -> Result<(), String> {
    Err(UAC_FAILED.into())
}

/// End a process: politely (SIGTERM) or forced (SIGKILL); with `tree`, the
/// processes it started go first, the deepest ones before their parents.
/// Every process is tried; the first failure is the answer.
pub fn end_process(pid: u32, force: bool, tree: bool) -> Result<(), String> {
    on_system(|h| end(h, pid, force, tree))
}

/// No shell process is known.
pub fn shell_pid() -> Option<u32> {
    None
}

/// Sets the priority class named `class` (one of [`PRIORITIES`]) as a nice value.
pub fn set_priority(pid: u32, class: &str) -> Result<(), String> {
    on_system(|h| renice(h, pid, class))
}

/// The priority class closest to the nice value of `pid`.
pub fn get_priority(pid: u32) -> Option<&'static str> {
    on_system(|h| priority_of(h, pid))
}

/// The processors `pid` may run on and the processors the system has.
pub fn get_affinity(pid: u32) -> Option<Affinity> {
    on_system(|h| affinity_of(h, pid))
}

/// Lets `pid` run only on the processors whose bit is set in `mask`.
pub fn set_affinity(pid: u32, mask: u64) -> Result<(), String> {
    on_system(|h| taskset(h, pid, mask))
}

/// Suspends a process (SIGSTOP).
pub fn suspend_process(pid: u32) -> Result<(), String> {
    on_system(|h| signal(h, pid, "STOP"))
}

/// Resumes a suspended process (SIGCONT).
pub fn resume_process(pid: u32) -> Result<(), String> {
    on_system(|h| signal(h, pid, "CONT"))
}

/// Efficiency mode cannot be set: an error.
pub fn set_efficiency(_: u32) -> Result<(), String> {
    Err(unavailable_error(Feature::ProcessEfficiency))
}

// ---- Facts from /proc ----

/// The user id on the `Uid:` line of a status file (the first one: real).
fn uid_in(status: &str) -> Option<u32> {
    status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().next()?.parse().ok())
}

/// The effective user id of the app itself (the second `Uid:` value).
fn own_uid(h: &Host) -> Option<u32> {
    let status = h.root.read("proc/self/status")?;
    status.lines().find_map(|l| l.strip_prefix("Uid:")).and_then(|v| v.split_whitespace().nth(1)?.parse().ok())
}

/// Whether acting on `pid` is another user's business: the app's user is
/// not the owner and is not root. Unknown owners count as the app's own,
/// so the program that acts says why it cannot.
fn is_foreign(h: &Host, pid: u32) -> bool {
    let (Some(own), Some(owner)) = (own_uid(h), h.root.read(&format!("proc/{pid}/status")).as_deref().and_then(uid_in)) else {
        return false;
    };
    own != 0 && own != owner
}

/// The parent and the nice value in `proc/<pid>/stat` (read after the
/// last `)`, which closes the name).
fn stat_of(h: &Host, pid: u32) -> Option<(u32, i32)> {
    let stat = h.root.read(&format!("proc/{pid}/stat"))?;
    let after = &stat[stat.rfind(')')? + 1..];
    let fields: Vec<&str> = after.split_whitespace().collect();
    Some((fields.get(1)?.parse().ok()?, fields.get(16)?.parse().ok()?))
}

/// The processes `pid` started, directly or not, deepest first.
fn descendants(h: &Host, pid: u32) -> Vec<u32> {
    let mut children: std::collections::HashMap<u32, Vec<u32>> = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(h.root.path("proc")) {
        for entry in entries.flatten() {
            if let Some(child) = entry.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) {
                if let Some((parent, _)) = stat_of(h, child) {
                    children.entry(parent).or_default().push(child);
                }
            }
        }
    }
    let mut found = vec![pid];
    let mut next = 0;
    while next < found.len() {
        let mut kids = children.get(&found[next]).cloned().unwrap_or_default();
        kids.sort_unstable();
        for kid in kids {
            if !found.contains(&kid) {
                found.push(kid);
            }
        }
        next += 1;
    }
    found.remove(0);
    found.reverse();
    found
}

/// The priority class a nice value falls in.
fn class_of_nice(nice: i32) -> &'static str {
    match nice {
        i32::MIN..=-15 => PRIORITIES[0],
        -14..=-8 => PRIORITIES[1],
        -7..=-1 => PRIORITIES[2],
        0 => PRIORITIES[3],
        1..=9 => PRIORITIES[4],
        _ => PRIORITIES[5],
    }
}

fn priority_of(h: &Host, pid: u32) -> Option<&'static str> {
    stat_of(h, pid).map(|(_, nice)| class_of_nice(nice))
}

/// The bits of a `Cpus_allowed` value (hex groups split by commas); None
/// when it needs more than 64 bits.
fn mask_of_hex(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(|c| *c != ',').collect();
    let digits = digits.trim_start_matches('0');
    if digits.len() > 16 {
        return None;
    }
    if digits.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(digits, 16).ok()
}

/// The bits of a CPU list such as `0-3,8`; None when a CPU is past bit 63.
fn mask_of_list(list: &str) -> Option<u64> {
    let mut mask = 0u64;
    for part in list.split(',').filter(|p| !p.is_empty()) {
        let (from, to) = match part.split_once('-') {
            Some((a, b)) => (a.parse::<u32>().ok()?, b.parse::<u32>().ok()?),
            None => (part.parse::<u32>().ok()?, part.parse::<u32>().ok()?),
        };
        if to >= 64 || from > to {
            return None;
        }
        for cpu in from..=to {
            mask |= 1 << cpu;
        }
    }
    Some(mask)
}

fn affinity_of(h: &Host, pid: u32) -> Option<Affinity> {
    let status = h.root.read(&format!("proc/{pid}/status"))?;
    let allowed = status.lines().find_map(|l| l.strip_prefix("Cpus_allowed:"))?.trim();
    let online = h.root.read("sys/devices/system/cpu/online")?;
    match (mask_of_hex(allowed), mask_of_list(&online)) {
        (Some(process), Some(system)) => Some(Affinity::Mask { process, system }),
        _ => Some(Affinity::ManyGroups),
    }
}

// ---- Running the programs ----

/// Runs `argv`, through `pkexec` when `elevate`; the words say how it failed.
fn run(h: &Host, argv: &[String], elevate: bool) -> Result<(), String> {
    let program = argv.first().cloned().unwrap_or_default();
    let launched = if elevate { action_elevation::elevated_argv(System::Linux, &argv.join(" "), h.env)? } else { argv.to_vec() };
    match (h.launch)(&launched) {
        // pkexec's own refusals keep their plain wording.
        Spawned::Exited { code, stderr } if code != 0 && !(elevate && (code == 126 || code == 127)) => {
            let stderr = stderr.trim();
            Err(if stderr.is_empty() { format!("{program} failed (exit code {code}).") } else { stderr.to_string() })
        }
        Spawned::NotFound if !elevate => Err(format!("{program} was not found on this system.")),
        Spawned::Failed(e) if !elevate => Err(format!("{program} could not be started: {e}")),
        other => action_elevation::interpret(System::Linux, other),
    }
}

/// Sends the signal named `name` (`TERM`, `KILL`, `STOP`, `CONT`) to `pid`:
/// the one place a signal reaches a process. Another user's process is
/// signalled through `pkexec`. Where the kernel has process descriptors the
/// signal goes through one; if that is not there, or fails, `kill` does it
/// and words the failure.
fn signal(h: &Host, pid: u32, name: &str) -> Result<(), String> {
    if !is_foreign(h, pid) && proc_fast::signal_fast(h.kernel, h.caps, pid, name).is_some() {
        return Ok(());
    }
    let argv = ["kill".to_string(), "-s".into(), name.into(), pid.to_string()];
    run(h, &argv, is_foreign(h, pid))
}

fn end(h: &Host, pid: u32, force: bool, tree: bool) -> Result<(), String> {
    let name = if force { "KILL" } else { "TERM" };
    let mut targets = if tree { descendants(h, pid) } else { Vec::new() };
    targets.push(pid);
    let mut first = Ok(());
    for target in targets {
        let result = signal(h, target, name);
        if first.is_ok() {
            first = result;
        }
    }
    first
}

fn renice(h: &Host, pid: u32, class: &str) -> Result<(), String> {
    let Some(index) = PRIORITIES.iter().position(|p| *p == class) else {
        return Err(format!("\"{class}\" is not a priority."));
    };
    let nice = NICE_OF[index];
    // Lowering one's own priority is free; raising it is not.
    let raises = stat_of(h, pid).is_some_and(|(_, now)| nice < now);
    let argv = ["renice".to_string(), "-n".into(), nice.to_string(), "-p".into(), pid.to_string()];
    run(h, &argv, raises || is_foreign(h, pid))
}

fn taskset(h: &Host, pid: u32, mask: u64) -> Result<(), String> {
    if mask == 0 {
        return Err("A process must be allowed to run on at least one processor.".into());
    }
    let argv = ["taskset".to_string(), "-a".into(), "-p".into(), format!("{mask:x}"), pid.to_string()];
    run(h, &argv, is_foreign(h, pid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_elevation::{PERMISSION_DISMISSED, PERMISSION_REFUSED, PKEXEC_MISSING};
    use std::cell::RefCell;

    /// Runs `f` over the `procs` fixture (the app is user 1000) with a
    /// launch that records every command line and answers `answer`.
    fn with_host<T>(answer: Spawned, f: impl FnOnce(&Host) -> T) -> (T, Vec<Vec<String>>) {
        let calls = RefCell::new(Vec::new());
        let launch = |argv: &[String]| {
            calls.borrow_mut().push(argv.to_vec());
            answer.clone()
        };
        let env = |_: &str| None;
        let root = crate::fs_root::fixture("procs");
        let out = f(&Host { root: &root, launch: &launch, env: &env, kernel: &NO_KERNEL, caps: Capabilities::default() });
        (out, calls.into_inner())
    }

    /// A kernel without any faster interface.
    static NO_KERNEL: crate::proc_fast::tests::FakeKernel = crate::proc_fast::tests::FakeKernel {
        pidfd: false,
        events: std::sync::Mutex::new(None),
        signals: std::sync::Mutex::new(Vec::new()),
        fail: false,
    };

    /// Like [`with_host`] with a kernel that has process descriptors.
    fn with_pidfd<T>(kernel: &crate::proc_fast::tests::FakeKernel, f: impl FnOnce(&Host) -> T) -> (T, Vec<Vec<String>>) {
        let calls = RefCell::new(Vec::new());
        let launch = |argv: &[String]| {
            calls.borrow_mut().push(argv.to_vec());
            ok()
        };
        let env = |_: &str| None;
        let root = crate::fs_root::fixture("procs");
        let out = f(&Host { root: &root, launch: &launch, env: &env, kernel, caps: Capabilities { pidfd: true, events: false } });
        (out, calls.into_inner())
    }

    #[test]
    fn with_process_descriptors_a_signal_starts_no_program_and_the_result_is_the_same() {
        let kernel = crate::proc_fast::tests::kernel(true, None);
        let (r, calls) = with_pidfd(&kernel, |h| end(h, 100, true, true));
        assert_eq!((r, calls.len()), (Ok(()), 0));
        assert_eq!(*kernel.signals.lock().unwrap(), vec![(102, 9), (101, 9), (100, 9)], "the same order as the fallback's kill commands");
        let (_, calls) = with_pidfd(&kernel, |h| signal(h, 100, "STOP"));
        assert!(calls.is_empty());
    }

    #[test]
    fn another_users_process_still_goes_through_pkexec_with_process_descriptors() {
        let kernel = crate::proc_fast::tests::kernel(true, None);
        let (_, calls) = with_pidfd(&kernel, |h| signal(h, 200, "TERM"));
        assert_eq!(lines(&calls), ["pkexec kill -s TERM 200"]);
        assert!(kernel.signals.lock().unwrap().is_empty());
    }

    #[test]
    fn a_failed_process_descriptor_signal_falls_back_to_kill() {
        let kernel = crate::proc_fast::tests::FakeKernel { pidfd: true, fail: true, ..Default::default() };
        let (r, calls) = with_pidfd(&kernel, |h| signal(h, 100, "TERM"));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["kill -s TERM 100".to_string()]));
    }

    fn ok() -> Spawned {
        Spawned::Exited { code: 0, stderr: String::new() }
    }

    fn lines(calls: &[Vec<String>]) -> Vec<String> {
        calls.iter().map(|c| c.join(" ")).collect()
    }

    #[test]
    fn end_task_sends_sigterm_and_force_quit_sends_sigkill() {
        let (r, calls) = with_host(ok(), |h| end(h, 100, false, false));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["kill -s TERM 100".to_string()]));
        let (r, calls) = with_host(ok(), |h| end(h, 100, true, false));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["kill -s KILL 100".to_string()]));
    }

    #[test]
    fn end_process_tree_ends_the_deepest_processes_first() {
        let (r, calls) = with_host(ok(), |h| end(h, 100, false, true));
        assert_eq!(r, Ok(()));
        assert_eq!(lines(&calls), ["kill -s TERM 102", "kill -s TERM 101", "kill -s TERM 100"]);
    }

    #[test]
    fn a_tree_end_tries_every_process_and_says_the_first_failure() {
        let (r, calls) = with_host(Spawned::Exited { code: 1, stderr: "kill: (102) - No such process\n".into() }, |h| end(h, 100, false, true));
        assert_eq!(r, Err("kill: (102) - No such process".to_string()));
        assert_eq!(calls.len(), 3);
    }

    #[test]
    fn another_users_process_is_signalled_through_pkexec() {
        let (r, calls) = with_host(ok(), |h| end(h, 200, false, false));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["pkexec kill -s TERM 200".to_string()]));
        let (_, calls) = with_host(ok(), |h| signal(h, 200, "STOP"));
        assert_eq!(lines(&calls), ["pkexec kill -s STOP 200"]);
    }

    #[test]
    fn suspend_and_resume_send_sigstop_and_sigcont() {
        let (_, calls) = with_host(ok(), |h| signal(h, 100, "STOP"));
        assert_eq!(lines(&calls), ["kill -s STOP 100"]);
        let (_, calls) = with_host(ok(), |h| signal(h, 100, "CONT"));
        assert_eq!(lines(&calls), ["kill -s CONT 100"]);
    }

    #[test]
    fn a_refusal_and_a_missing_pkexec_are_worded_plainly() {
        let (r, _) = with_host(Spawned::Exited { code: 127, stderr: String::new() }, |h| signal(h, 200, "TERM"));
        assert_eq!(r, Err(PERMISSION_REFUSED.into()));
        let (r, _) = with_host(Spawned::Exited { code: 126, stderr: String::new() }, |h| signal(h, 200, "TERM"));
        assert_eq!(r, Err(PERMISSION_DISMISSED.into()));
        let (r, _) = with_host(Spawned::NotFound, |h| signal(h, 200, "TERM"));
        assert_eq!(r, Err(PKEXEC_MISSING.into()));
        let (r, _) = with_host(Spawned::NotFound, |h| signal(h, 100, "TERM"));
        assert_eq!(r, Err("kill was not found on this system.".into()));
        let (r, _) = with_host(Spawned::Exited { code: 1, stderr: String::new() }, |h| signal(h, 100, "TERM"));
        assert_eq!(r, Err("kill failed (exit code 1).".into()));
    }

    #[test]
    fn set_priority_uses_nice_values_and_asks_only_to_raise() {
        let (r, calls) = with_host(ok(), |h| renice(h, 100, "Below normal"));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["renice -n 5 -p 100".to_string()]));
        let (_, calls) = with_host(ok(), |h| renice(h, 100, "Low"));
        assert_eq!(lines(&calls), ["renice -n 19 -p 100"]);
        // Raising: from nice 10 to 0, and to -5 and -10 from 0.
        let (_, calls) = with_host(ok(), |h| renice(h, 300, "Normal"));
        assert_eq!(lines(&calls), ["pkexec renice -n 0 -p 300"]);
        let (_, calls) = with_host(ok(), |h| renice(h, 100, "Above normal"));
        assert_eq!(lines(&calls), ["pkexec renice -n -5 -p 100"]);
        let (_, calls) = with_host(ok(), |h| renice(h, 100, "High"));
        assert_eq!(lines(&calls), ["pkexec renice -n -10 -p 100"]);
        // Another user's process: always through pkexec.
        let (_, calls) = with_host(ok(), |h| renice(h, 200, "Low"));
        assert_eq!(lines(&calls), ["pkexec renice -n 19 -p 200"]);
        let (r, calls) = with_host(ok(), |h| renice(h, 100, "Turbo"));
        assert_eq!((r.is_err(), calls.len()), (true, 0));
    }

    #[test]
    fn the_priority_is_the_class_of_the_nice_value() {
        let (p, _) = with_host(ok(), |h| (priority_of(h, 100), priority_of(h, 300), priority_of(h, 999)));
        assert_eq!(p, (Some("Normal"), Some("Low"), None));
        for (nice, class) in [(-20, "Realtime"), (-10, "High"), (-5, "Above normal"), (0, "Normal"), (5, "Below normal"), (19, "Low")] {
            assert_eq!(class_of_nice(nice), class);
        }
    }

    #[test]
    fn the_affinity_is_read_from_the_process_and_the_cpus_online() {
        let (a, _) = with_host(ok(), |h| (affinity_of(h, 100), affinity_of(h, 102), affinity_of(h, 400), affinity_of(h, 999)));
        assert_eq!(a.0, Some(Affinity::Mask { process: 0xf, system: 0xf }));
        assert_eq!(a.1, Some(Affinity::Mask { process: 0x3, system: 0xf }));
        assert_eq!(a.2, Some(Affinity::ManyGroups), "more than 64 processors cannot be said in one mask");
        assert_eq!(a.3, None);
    }

    #[test]
    fn set_affinity_runs_taskset_with_a_hex_mask_and_asks_for_another_users_process() {
        let (r, calls) = with_host(ok(), |h| taskset(h, 100, 0x5));
        assert_eq!((r, lines(&calls)), (Ok(()), vec!["taskset -a -p 5 100".to_string()]));
        let (_, calls) = with_host(ok(), |h| taskset(h, 200, 0xff));
        assert_eq!(lines(&calls), ["pkexec taskset -a -p ff 200"]);
        let (r, calls) = with_host(ok(), |h| taskset(h, 100, 0));
        assert_eq!((r.is_err(), calls.len()), (true, 0), "no processor at all is refused before any launch");
    }

    #[test]
    fn an_unknown_owner_is_tried_directly() {
        let root = crate::fs_root::FsRoot::at(std::env::temp_dir().join("upm-no-such-root"));
        let launch = |_: &[String]| ok();
        let env = |_: &str| None;
        assert!(!is_foreign(&Host { root: &root, launch: &launch, env: &env, kernel: &NO_KERNEL, caps: Capabilities::default() }, 5));
    }

    #[test]
    fn a_pid_in_the_command_is_only_ever_digits() {
        let (_, calls) = with_host(ok(), |h| signal(h, 4_000_000, "TERM"));
        assert_eq!(lines(&calls), ["kill -s TERM 4000000"]);
    }

    #[test]
    fn the_windows_only_actions_still_say_so() {
        assert_eq!(write_dump_file(1, std::path::Path::new("x")), Err(DUMP_FAILED.into()));
        assert_eq!(toggle_uac_virtualization(1), Err(UAC_FAILED.into()));
        assert_eq!(set_efficiency(1), Err("Not available on this system.".into()));
        assert_eq!(shell_pid(), None);
    }
}
