//! The runit source: the service folders read through [`FsRoot`], the state
//! of the enabled ones asked with `sv status` through the process runner.
//!
//! runit keeps every service in a folder of the service directory
//! (`/etc/sv/<name>`, with a `run` program); a service is enabled when the
//! supervised folder (`/var/service`, `/etc/service` or
//! `/etc/runit/runsvdir/current`, depending on the distribution) holds a
//! link to it. The folders say which services exist and which are enabled;
//! only `sv status` says whether one runs and with which process id, so it
//! is asked once for all the enabled ones. Start, stop and restart go to
//! `sv`; enabling a service at boot links its folder into the supervised
//! folder, disabling removes the link.
use super::sources::{check_name, run_elevated, run_text, ControlHost, Output, ServiceSource, SourceError, ALL_ACTIONS};
use crate::fs_root::FsRoot;
use crate::platform::ProcessRunner;
use crate::system_lists::ServiceRow;

/// The name this source puts on its rows.
pub const SOURCE: &str = "runit";

/// Where the available services are, and where the enabled ones are linked
/// (the first of each that exists is the one in use).
const AVAILABLE: &[&str] = &["etc/sv", "etc/runit/sv"];
const SUPERVISED: &[&str] = &["var/service", "etc/service", "etc/runit/runsvdir/current", "service"];

pub struct Runit {
    root: FsRoot,
}

impl Runit {
    pub fn new(root: FsRoot) -> Self {
        Self { root }
    }
}

impl ServiceSource for Runit {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn list(&self, runner: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError> {
        let supervised = SUPERVISED.iter().find(|d| self.root.probe(d));
        let available = AVAILABLE.iter().find(|d| self.root.probe(d));
        if supervised.is_none() && available.is_none() {
            return Err(SourceError::Absent);
        }
        let enabled: Vec<String> = supervised.map(|d| self.root.list_dir(d)).unwrap_or_default();
        let mut names: Vec<String> = available.map(|d| self.root.list_dir(d)).unwrap_or_default();
        for name in &enabled {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        names.sort();

        let mut states = Vec::new();
        if let (Some(dir), false) = (supervised, enabled.is_empty()) {
            let paths: Vec<String> = enabled.iter().map(|n| format!("/{dir}/{n}")).collect();
            let mut args = vec!["status"];
            args.extend(paths.iter().map(String::as_str));
            match run_text(runner, "sv", &args) {
                Output::NotStarted => return Err(SourceError::Absent),
                // `sv` ends with a failure code when any service is down;
                // its lines are read all the same.
                Output::Exited(_, text) => states = parse_status(&text),
            }
        }

        Ok(names
            .into_iter()
            .map(|name| {
                let is_enabled = enabled.contains(&name);
                let state = states.iter().find(|s| s.name == name);
                ServiceRow {
                    display: name.clone(),
                    pid: state.map_or(0, |s| s.pid),
                    status: match (is_enabled, state) {
                        (false, _) => "Stopped".into(),
                        (true, Some(s)) => s.word.into(),
                        (true, None) => String::new(),
                    },
                    start_type: if is_enabled { "Enabled" } else { "Disabled" }.into(),
                    name,
                    source: SOURCE.into(),
                    scope: "System".into(),
                    actions: None,
                }
            })
            .collect())
    }

    /// `sv up|down|restart <supervised folder>/<service>`; at boot, a link
    /// from the supervised folder to the service's folder is made (`ln -s`)
    /// or removed (`rm`). runit's services are the machine's, so every action
    /// goes through `pkexec`.
    fn control(&self, host: &ControlHost, _scope: &str, name: &str, action: &str) -> Result<(), String> {
        check_name(name)?;
        if name.contains('/') || name == "." || name == ".." {
            return Err(format!("\"{name}\" is not a service name."));
        }
        if !ALL_ACTIONS.contains(&action) {
            return Err(format!("\"{action}\" is not something a service can be asked to do."));
        }
        let Some(supervised) = SUPERVISED.iter().find(|d| self.root.probe(d)) else {
            return Err("The folder runit supervises its services from was not found.".into());
        };
        let link = format!("/{supervised}/{name}");
        match action {
            "start" => run_elevated(host, "sv", &["up", &link]),
            "stop" => run_elevated(host, "sv", &["down", &link]),
            "restart" => run_elevated(host, "sv", &["restart", &link]),
            "enable" => {
                let Some(available) = AVAILABLE.iter().find(|d| self.root.probe(&format!("{d}/{name}"))) else {
                    return Err(format!("\"{name}\" has no service folder to link to."));
                };
                run_elevated(host, "ln", &["-s", &format!("/{available}/{name}"), &link])
            }
            _ => run_elevated(host, "rm", &[&link]),
        }
    }

    fn actions(&self, _scope: &str) -> &'static [&'static str] {
        ALL_ACTIONS
    }
}

/// One line of `sv status`.
#[derive(Debug, PartialEq, Eq)]
struct State {
    name: String,
    word: &'static str,
    pid: u32,
}

/// `run: /var/service/sshd: (pid 812) 45s; run: log: (pid 813) 45s`,
/// `down: /var/service/cron: 3s, normally up`, `fail: /var/service/x: unable
/// to change to service directory`. The state is the first word; the log
/// service on the same line is not a service of its own.
fn parse_status(text: &str) -> Vec<State> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, ": ");
            let word = match parts.next()?.trim() {
                "run" => "Running",
                "down" | "finish" => "Stopped",
                "warning" | "fail" => "Failed",
                _ => return None,
            };
            let path = parts.next()?;
            let rest = parts.next().unwrap_or("");
            let pid = rest.split("(pid ").nth(1).and_then(|p| p.split(')').next()).and_then(|p| p.trim().parse().ok()).unwrap_or(0);
            Some(State { name: path.rsplit('/').next()?.to_string(), word, pid })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action_elevation::{self, Spawned};
    use crate::fs_root::fixture;
    use crate::platform::{Launch, RecordingRunner, When};

    const STATUS: &str = "run: /var/service/sshd: (pid 812) 3600s; run: log: (pid 813) 3600s\ndown: /var/service/socklog: 12s, normally up\n";

    fn asked(runner: &RecordingRunner) -> Vec<String> {
        runner
            .launches()
            .into_iter()
            .map(|l| match l {
                Launch::Run { program, args, .. } => format!("{program} {}", args.join(" ")),
                other => panic!("unexpected {other:?}"),
            })
            .collect()
    }

    fn by_name<'a>(rows: &'a [ServiceRow], name: &str) -> &'a ServiceRow {
        rows.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("{name} in {rows:?}"))
    }

    #[test]
    fn services_are_listed_with_state_start_type_pid_and_source() {
        let runner = RecordingRunner::default();
        // `sv` ends with 1 when a service is down.
        runner.prepare(When::Nth(0), 1, STATUS);
        let rows = Runit::new(fixture("runit")).list(&runner).unwrap();
        assert_eq!(asked(&runner), ["sv status /var/service/socklog /var/service/sshd"]);
        let ssh = by_name(&rows, "sshd");
        assert_eq!((ssh.status.as_str(), ssh.start_type.as_str(), ssh.pid, ssh.source.as_str(), ssh.scope.as_str()), ("Running", "Enabled", 812, "runit", "System"));
        let socklog = by_name(&rows, "socklog");
        assert_eq!((socklog.status.as_str(), socklog.start_type.as_str(), socklog.pid), ("Stopped", "Enabled", 0));
        let dhcpcd = by_name(&rows, "dhcpcd");
        assert_eq!((dhcpcd.status.as_str(), dhcpcd.start_type.as_str()), ("Stopped", "Disabled"), "available but not linked");
        assert_eq!(rows.len(), 3);
    }

    fn controlled(root: &str, name: &str, action: &str, answer: Spawned) -> (Result<(), String>, Vec<Vec<String>>) {
        let launched = std::cell::RefCell::new(Vec::new());
        let launch = |argv: &[String]| {
            launched.borrow_mut().push(argv.to_vec());
            answer.clone()
        };
        let env = |_: &str| None;
        let result = Runit::new(fixture(root)).control(&ControlHost { launch: &launch, env: &env }, "System", name, action);
        (result, launched.into_inner())
    }

    fn ok() -> Spawned {
        Spawned::Exited { code: 0, stderr: String::new() }
    }

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(String::from).collect()
    }

    #[test]
    fn each_action_runs_its_runit_command_through_pkexec() {
        for (name, action, line) in [
            ("sshd", "start", "pkexec sv up /var/service/sshd"),
            ("sshd", "stop", "pkexec sv down /var/service/sshd"),
            ("sshd", "restart", "pkexec sv restart /var/service/sshd"),
            ("dhcpcd", "enable", "pkexec ln -s /etc/sv/dhcpcd /var/service/dhcpcd"),
            ("sshd", "disable", "pkexec rm /var/service/sshd"),
        ] {
            let (result, launched) = controlled("runit", name, action, ok());
            assert_eq!(result, Ok(()), "{action}");
            assert_eq!(launched, [words(line)], "{action}");
        }
    }

    #[test]
    fn refusal_and_a_missing_pkexec_are_worded_plainly_and_a_silent_failure_names_the_exit_code() {
        let (refused, _) = controlled("runit", "sshd", "stop", Spawned::Exited { code: 127, stderr: String::new() });
        assert_eq!(refused, Err(action_elevation::PERMISSION_REFUSED.to_string()));
        let (dismissed, _) = controlled("runit", "sshd", "stop", Spawned::Exited { code: 126, stderr: String::new() });
        assert_eq!(dismissed, Err(action_elevation::PERMISSION_DISMISSED.to_string()));
        let (missing, _) = controlled("runit", "sshd", "stop", Spawned::NotFound);
        assert_eq!(missing, Err(action_elevation::PKEXEC_MISSING.to_string()));
        let (silent, _) = controlled("runit", "sshd", "start", Spawned::Exited { code: 1, stderr: String::new() });
        assert_eq!(silent, Err("sv failed (exit code 1).".to_string()));
    }

    #[test]
    fn a_bad_name_an_unknown_action_or_missing_folders_start_nothing() {
        for (root, name, action) in [
            ("runit", "sshd", "mask"),
            ("runit", "-x", "start"),
            ("runit", "../etc", "start"),
            ("runit", "a b", "stop"),
            ("runit", "ghost", "enable"),
            ("runit-none", "sshd", "start"),
        ] {
            let (result, launched) = controlled(root, name, action, ok());
            assert!(result.is_err(), "{root} {name} {action}");
            assert!(launched.is_empty(), "{root} {name} {action}");
        }
    }

    #[test]
    fn rows_of_runit_offer_every_action() {
        use crate::services::sources::list_all;
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, STATUS);
        let sources: Vec<Box<dyn ServiceSource>> = vec![Box::new(Runit::new(fixture("runit")))];
        let rows = list_all(&sources, &runner).unwrap();
        assert!(rows.iter().all(|r| ["start", "stop", "restart", "enable", "disable"].into_iter().all(|a| r.can(a))));
    }

    #[test]
    fn without_the_service_folders_the_source_is_absent() {
        assert_eq!(Runit::new(fixture("runit-none")).list(&RecordingRunner::default()).err(), Some(SourceError::Absent));
    }

    #[test]
    fn without_sv_the_source_is_absent() {
        assert_eq!(Runit::new(fixture("runit")).list(&RecordingRunner::default()).err(), Some(SourceError::Absent));
    }

    #[test]
    fn a_failing_openrc_does_not_hide_the_runit_services() {
        use crate::services::openrc::OpenRc;
        use crate::services::sources::list_all;
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, "rc-status: broken");
        runner.prepare(When::Nth(1), 0, STATUS);
        let sources: Vec<Box<dyn ServiceSource>> = vec![Box::new(OpenRc), Box::new(Runit::new(fixture("runit")))];
        let rows = list_all(&sources, &runner).unwrap();
        assert_eq!(rows.iter().map(|r| r.source.as_str()).collect::<Vec<_>>(), ["runit", "runit", "runit"]);
    }

    #[test]
    fn a_failed_supervision_is_worded_failed() {
        let states = parse_status("fail: /var/service/x: unable to change to service directory: file does not exist\n");
        assert_eq!(states, [State { name: "x".into(), word: "Failed", pid: 0 }]);
    }
}
