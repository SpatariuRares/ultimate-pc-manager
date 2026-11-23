//! The runit source: the service folders read through [`FsRoot`], the state
//! of the enabled ones asked with `sv status` through the process runner.
//!
//! runit keeps every service in a folder of the service directory
//! (`/etc/sv/<name>`, with a `run` program); a service is enabled when the
//! supervised folder (`/var/service`, `/etc/service` or
//! `/etc/runit/runsvdir/current`, depending on the distribution) holds a
//! link to it. The folders say which services exist and which are enabled;
//! only `sv status` says whether one runs and with which process id, so it
//! is asked once for all the enabled ones. This source lists only; start,
//! stop and restart for it come with the control ticket.
use super::sources::{run_text, Output, ServiceSource, SourceError};
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
                }
            })
            .collect())
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
