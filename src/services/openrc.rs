//! The OpenRC source: every service and its state from `rc-status`, and
//! which ones start at boot from `rc-update`, through the process runner.
//!
//! `rc-status --servicelist` names every service OpenRC knows with its state
//! in brackets (`sshd   [  started  ]`), also those in no runlevel;
//! `rc-update show` names the ones added to a runlevel. OpenRC does not say
//! a service's process id, so rows carry none. This source lists only;
//! start, stop and restart for it come with the control ticket.
use std::collections::HashSet;

use super::sources::{run_text, Output, ServiceSource, SourceError};
use crate::platform::ProcessRunner;
use crate::system_lists::ServiceRow;

/// The name this source puts on its rows.
pub const SOURCE: &str = "OpenRC";

pub struct OpenRc;

impl ServiceSource for OpenRc {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn list(&self, runner: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError> {
        let states = match run_text(runner, "rc-status", &["--servicelist", "--nocolor"]) {
            Output::NotStarted => return Err(SourceError::Absent),
            Output::Exited(0, text) => parse_states(&text),
            Output::Exited(code, text) => {
                let said = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
                return Err(SourceError::Failed(if said.is_empty() { format!("rc-status ended with code {code}") } else { said.chars().take(200).collect() }));
            }
        };
        // Without the runlevels the start types are empty; the list still shows.
        let enabled: Option<HashSet<String>> = match run_text(runner, "rc-update", &["show"]) {
            Output::Exited(0, text) => Some(parse_enabled(&text)),
            _ => None,
        };
        Ok(states
            .into_iter()
            .map(|(name, state)| ServiceRow {
                display: name.clone(),
                status: status_word(&state),
                start_type: match &enabled {
                    Some(set) if set.contains(&name) => "Enabled".into(),
                    Some(_) => "Disabled".into(),
                    None => String::new(),
                },
                name,
                pid: 0,
                source: SOURCE.into(),
                scope: "System".into(),
            })
            .collect())
    }
}

/// `(service, state)` for every `name [ state ]` line; section headers
/// (`Runlevel: default`) have no brackets and are skipped. A service listed
/// under several runlevels counts once.
fn parse_states(text: &str) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((name, rest)) = line.split_once('[') else { continue };
        let name = name.trim();
        let state = rest.split(']').next().unwrap_or("").trim();
        if name.is_empty() || name.contains(char::is_whitespace) || name.ends_with(':') || !seen.insert(name.to_string()) {
            continue;
        }
        out.push((name.to_string(), state.to_string()));
    }
    out
}

/// The services `rc-update show` lists with at least one runlevel
/// (`sshd | default boot`).
fn parse_enabled(text: &str) -> HashSet<String> {
    text.lines()
        .filter_map(|l| l.split_once('|'))
        .filter(|(_, levels)| !levels.trim().is_empty())
        .map(|(name, _)| name.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// OpenRC's state word as the page says it.
fn status_word(state: &str) -> String {
    match state.split_whitespace().next().unwrap_or("") {
        "started" => "Running".into(),
        "stopped" => "Stopped".into(),
        "crashed" => "Failed".into(),
        "" => String::new(),
        other => {
            let mut c = other.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Launch, RecordingRunner, When};

    const STATUS: &str = "Runlevel: default\n sshd                                                              [  started  ]\n cronie                                                            [  started  ]\n local                                                             [  stopped  ]\nRunlevel: boot\n hostname                                                          [  started  ]\nDynamic Runlevel: hotplugged\nDynamic Runlevel: needed/wanted\n netmount                                                          [  crashed  ]\n sshd                                                              [  started  ]\nDynamic Runlevel: manual\n cupsd                                                             [  inactive ]\n";
    const UPDATE: &str = "      sshd | default\n    cronie | default\n  hostname | boot\n     local |\n";

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
    fn services_are_listed_with_state_start_type_and_source() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, STATUS);
        runner.prepare(When::Nth(1), 0, UPDATE);
        let rows = OpenRc.list(&runner).unwrap();
        assert_eq!(asked(&runner), ["rc-status --servicelist --nocolor", "rc-update show"]);
        let ssh = by_name(&rows, "sshd");
        assert_eq!((ssh.status.as_str(), ssh.start_type.as_str(), ssh.source.as_str(), ssh.scope.as_str(), ssh.pid), ("Running", "Enabled", "OpenRC", "System", 0));
        assert_eq!(rows.iter().filter(|r| r.name == "sshd").count(), 1, "a service in two lists counts once");
        let local = by_name(&rows, "local");
        assert_eq!((local.status.as_str(), local.start_type.as_str()), ("Stopped", "Disabled"), "added to no runlevel");
        assert_eq!(by_name(&rows, "netmount").status, "Failed");
        let cups = by_name(&rows, "cupsd");
        assert_eq!((cups.status.as_str(), cups.start_type.as_str()), ("Inactive", "Disabled"));
        assert!(rows.iter().all(|r| !r.name.contains(':')), "runlevel headers are not services: {rows:?}");
    }

    #[test]
    fn a_failing_rc_update_leaves_the_start_types_empty() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, STATUS);
        runner.prepare(When::Nth(1), 1, "denied");
        let rows = OpenRc.list(&runner).unwrap();
        assert_eq!(by_name(&rows, "sshd").start_type, "");
    }

    #[test]
    fn no_rc_status_means_the_source_is_absent() {
        assert_eq!(OpenRc.list(&RecordingRunner::default()).err(), Some(SourceError::Absent));
    }

    #[test]
    fn an_rc_status_that_fails_says_why() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, "rc-status: cannot open runlevel\nmore");
        assert_eq!(OpenRc.list(&runner).err(), Some(SourceError::Failed("rc-status: cannot open runlevel".into())));
    }
}
