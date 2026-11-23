//! The systemd source: the system services and the user's own, read with
//! `systemctl` through the process runner.
//!
//! Per scope three questions are asked: the loaded units (`list-units`),
//! their unit files (`list-unit-files`, for the start type) and the main
//! process ids (`show`). The two lists use `--output=json` when this
//! systemd has it. A systemd that does not (such as 219) fails that call, and
//! the plain-text form of the same list is read instead; the choice is made
//! by asking, never by comparing version numbers.

use std::collections::{HashMap, HashSet};

use super::sources::{check_name, run_text, ControlHost, ALL_ACTIONS, Output, ServiceSource, SourceError};
use crate::action_elevation::{self, Spawned};
use crate::platform::ProcessRunner;
use crate::system::System;
use crate::system_lists::ServiceRow;

/// The name this source puts on its rows.
pub const SOURCE: &str = "systemd";

pub struct Systemd;

/// Which services of systemd: the machine's, or the logged-in user's.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    System,
    User,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::System => "System",
            Scope::User => "User",
        }
    }

    /// `systemctl`'s arguments for this scope followed by `rest`.
    fn args<'a>(self, rest: &[&'a str]) -> Vec<&'a str> {
        let mut args = Vec::new();
        if self == Scope::User {
            args.push("--user");
        }
        args.extend_from_slice(rest);
        args
    }
}

impl ServiceSource for Systemd {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn list(&self, runner: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError> {
        let mut rows = scope_rows(runner, Scope::System)?;
        // The user's manager may not run (no session): the system's services
        // are listed all the same.
        if let Ok(user) = scope_rows(runner, Scope::User) {
            rows.extend(user);
        }
        Ok(rows)
    }

    fn control(&self, host: &ControlHost, scope: &str, name: &str, action: &str) -> Result<(), String> {
        let scope = if scope == Scope::User.label() { Scope::User } else { Scope::System };
        control(host, scope, name, action)
    }

    fn actions(&self, _scope: &str) -> &'static [&'static str] {
        ALL_ACTIONS
    }
}

/// What `systemctl` is asked to do for each action of the page.
const VERBS: [&str; 5] = ["start", "stop", "restart", "enable", "disable"];

/// `systemctl [--user] <verb> <unit>`. The system's units need administrator
/// rights, asked through `pkexec`; the user's own do not.
fn control(host: &ControlHost, scope: Scope, name: &str, action: &str) -> Result<(), String> {
    if !VERBS.contains(&action) {
        return Err(format!("\"{action}\" is not something a service can be asked to do."));
    }
    // The name goes to systemctl as one argument: a name that reads as an
    // option, or that holds a space, a quote or a control character, is not
    // a unit name.
    check_name(name)?;
    let argv: Vec<String> = ["systemctl"].into_iter().chain(scope.args(&[action, name])).map(String::from).collect();
    let elevate = scope == Scope::System;
    // The elevated command line is split into words again, where a backslash
    // (instance units hold some) would be read as an escape: the name goes
    // in single quotes, which keep every character.
    let launched = if elevate { action_elevation::elevated_argv(System::Linux, &format!("systemctl {action} '{name}'"), host.env)? } else { argv };
    match (host.launch)(&launched) {
        // pkexec's own refusals keep their plain wording.
        Spawned::Exited { code, stderr } if code != 0 && !(elevate && (code == 126 || code == 127)) => {
            let stderr = stderr.trim();
            Err(if stderr.is_empty() { format!("systemctl failed (exit code {code}).") } else { stderr.to_string() })
        }
        Spawned::NotFound if !elevate => Err("systemctl was not found on this system.".into()),
        Spawned::Failed(e) if !elevate => Err(format!("systemctl could not be started: {e}")),
        other => action_elevation::interpret(System::Linux, other),
    }
}

/// A loaded unit as `list-units` tells it.
#[derive(Debug, PartialEq, Eq)]
struct Unit {
    name: String,
    load: String,
    active: String,
    sub: String,
    description: String,
}

/// A unit file as `list-unit-files` tells it: its name and state.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UnitFile {
    pub(crate) name: String,
    pub(crate) state: String,
}

fn scope_rows(runner: &dyn ProcessRunner, scope: Scope) -> Result<Vec<ServiceRow>, SourceError> {
    let units = ask(
        runner,
        &scope.args(&["list-units", "--type=service", "--all", "--no-pager", "--output=json"]),
        &scope.args(&["list-units", "--type=service", "--all", "--no-pager", "--no-legend", "--plain"]),
        parse_units_json,
        parse_units_text,
    )?;
    // Without unit files the start types are empty; the list still shows.
    let files = ask(
        runner,
        &scope.args(&["list-unit-files", "--type=service", "--no-pager", "--output=json"]),
        &scope.args(&["list-unit-files", "--type=service", "--no-pager", "--no-legend"]),
        parse_files_json,
        parse_files_text,
    )
    .unwrap_or_default();
    let pids = match run_text(runner, "systemctl", &scope.args(&["show", "--no-pager", "--property=Id,MainPID", "*.service"])) {
        Output::Exited(0, text) => parse_show(&text),
        _ => HashMap::new(),
    };
    Ok(build_rows(scope, units, files, &pids))
}

/// One list, from the JSON call, else from the plain-text call.
pub(crate) fn ask<T>(runner: &dyn ProcessRunner, json_args: &[&str], text_args: &[&str], from_json: fn(&str) -> Option<Vec<T>>, from_text: fn(&str) -> Vec<T>) -> Result<Vec<T>, SourceError> {
    match run_text(runner, "systemctl", json_args) {
        Output::NotStarted => return Err(SourceError::Absent),
        Output::Exited(0, text) => {
            if let Some(list) = from_json(&text) {
                return Ok(list);
            }
        }
        Output::Exited(..) => {}
    }
    match run_text(runner, "systemctl", text_args) {
        Output::NotStarted => Err(SourceError::Absent),
        Output::Exited(0, text) => Ok(from_text(&text)),
        Output::Exited(code, text) => Err(failure(code, &text)),
    }
}

/// A systemctl that ran but cannot talk to a systemd that runs the system
/// (a container, another init) means systemd is not here; anything else is
/// a failure with what systemctl said.
pub(crate) fn failure(code: i32, text: &str) -> SourceError {
    let lower = text.to_lowercase();
    if lower.contains("not been booted with systemd") || lower.contains("failed to connect to bus") {
        return SourceError::Absent;
    }
    let said = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    SourceError::Failed(if said.is_empty() { format!("systemctl ended with code {code}") } else { said.chars().take(200).collect() })
}

fn parse_units_json(text: &str) -> Option<Vec<Unit>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = |v: &serde_json::Value, key: &str| v.get(key).and_then(|s| s.as_str()).unwrap_or("").to_string();
    Some(
        value
            .as_array()?
            .iter()
            .map(|v| Unit { name: field(v, "unit"), load: field(v, "load"), active: field(v, "active"), sub: field(v, "sub"), description: field(v, "description") })
            .collect(),
    )
}

pub(crate) fn parse_files_json(text: &str) -> Option<Vec<UnitFile>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let field = |v: &serde_json::Value, key: &str| v.get(key).and_then(|s| s.as_str()).unwrap_or("").to_string();
    Some(value.as_array()?.iter().map(|v| UnitFile { name: field(v, "unit_file"), state: field(v, "state") }).collect())
}

/// `UNIT LOAD ACTIVE SUB DESCRIPTION` lines; an old systemd marks some
/// units with a leading bullet.
fn parse_units_text(text: &str) -> Vec<Unit> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim_start_matches(|c: char| c.is_whitespace() || !c.is_ascii());
            let line = line.strip_prefix('*').unwrap_or(line).trim_start();
            let mut words = line.split_whitespace();
            let name = words.next()?;
            if !name.ends_with(".service") {
                return None;
            }
            let (load, active, sub) = (words.next()?, words.next()?, words.next()?);
            let description = words.collect::<Vec<_>>().join(" ");
            Some(Unit { name: name.into(), load: load.into(), active: active.into(), sub: sub.into(), description })
        })
        .collect()
}

/// `UNIT FILE STATE [PRESET]` lines.
pub(crate) fn parse_files_text(text: &str) -> Vec<UnitFile> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let state = words.next()?;
            name.ends_with(".service").then(|| UnitFile { name: name.into(), state: state.into() })
        })
        .collect()
}

/// `systemctl show` blocks of `Id=` and `MainPID=`: the unit's main process,
/// for the units that have one.
fn parse_show(text: &str) -> HashMap<String, u32> {
    let mut pids = HashMap::new();
    let mut id = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("Id=") {
            id = Some(name.trim().to_string());
        } else if let Some(pid) = line.strip_prefix("MainPID=") {
            if let (Some(name), Ok(pid)) = (id.take(), pid.trim().parse::<u32>()) {
                if pid > 0 {
                    pids.insert(name, pid);
                }
            }
        }
    }
    pids
}

/// The state words the page shows, from systemd's active and sub states.
fn status(active: &str, sub: &str) -> String {
    match (active, sub) {
        (_, "running") => "Running".into(),
        ("inactive", _) => "Stopped".into(),
        ("active", other) => capital(other),
        (other, _) => capital(other),
    }
}

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

fn build_rows(scope: Scope, units: Vec<Unit>, files: Vec<UnitFile>, pids: &HashMap<String, u32>) -> Vec<ServiceRow> {
    let states: HashMap<&str, &str> = files.iter().map(|f| (f.name.as_str(), f.state.as_str())).collect();
    let mut seen = HashSet::new();
    let mut rows = Vec::new();
    for unit in units.iter().filter(|u| u.load != "not-found") {
        let status = status(&unit.active, &unit.sub);
        let pid = if status == "Running" { pids.get(&unit.name).copied().unwrap_or(0) } else { 0 };
        seen.insert(unit.name.as_str());
        rows.push(ServiceRow {
            name: unit.name.clone(),
            display: unit.description.clone(),
            pid,
            status,
            start_type: states.get(unit.name.as_str()).map(|s| capital(s)).unwrap_or_default(),
            source: SOURCE.into(),
            scope: scope.label().into(),
            actions: None,
        });
    }
    // Installed but never loaded (templates are patterns, not services).
    for file in files.iter().filter(|f| !seen.contains(f.name.as_str()) && !f.name.contains("@.")) {
        rows.push(ServiceRow {
            name: file.name.clone(),
            status: "Stopped".into(),
            start_type: capital(&file.state),
            source: SOURCE.into(),
            scope: scope.label().into(),
            ..Default::default()
        });
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Launch, RecordingRunner, When};

    const UNITS_JSON: &str = r#"[
      {"unit":"ssh.service","load":"loaded","active":"active","sub":"running","description":"OpenBSD Secure Shell server"},
      {"unit":"cron.service","load":"loaded","active":"inactive","sub":"dead","description":"Regular background program processing daemon"},
      {"unit":"backup.service","load":"loaded","active":"active","sub":"exited","description":"Nightly backup"},
      {"unit":"ghost.service","load":"not-found","active":"inactive","sub":"dead","description":"ghost.service"}
    ]"#;
    const FILES_JSON: &str = r#"[
      {"unit_file":"ssh.service","state":"enabled","preset":"enabled"},
      {"unit_file":"cron.service","state":"disabled","preset":"enabled"},
      {"unit_file":"backup.service","state":"static","preset":null},
      {"unit_file":"idle.service","state":"masked","preset":null},
      {"unit_file":"getty@.service","state":"enabled","preset":"enabled"}
    ]"#;
    const SHOW: &str = "Id=ssh.service\nMainPID=812\n\nId=cron.service\nMainPID=0\n\nId=backup.service\nMainPID=0\n";

    const UNITS_OLD: &str = "  ssh.service    loaded active running OpenBSD Secure Shell server\n  cron.service   loaded inactive dead  Regular background program processing daemon\n\u{25cf} broken.service loaded failed failed A broken one\n";
    const FILES_OLD: &str = "ssh.service      enabled\ncron.service     disabled\nbroken.service   enabled\n";

    /// The program and arguments of each request, in order.
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

    fn list(runner: &RecordingRunner) -> Result<Vec<ServiceRow>, SourceError> {
        Systemd.list(runner)
    }

    fn by_name<'a>(rows: &'a [ServiceRow], scope: &str, name: &str) -> &'a ServiceRow {
        rows.iter().find(|r| r.scope == scope && r.name == name).unwrap_or_else(|| panic!("{scope} {name} in {rows:?}"))
    }

    #[test]
    fn current_systemd_lists_system_services_with_state_start_type_and_pid() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, UNITS_JSON);
        runner.prepare(When::Nth(1), 0, FILES_JSON);
        runner.prepare(When::Nth(2), 0, SHOW);
        // The user's manager does not answer: nothing prepared for it.
        let rows = list(&runner).unwrap();

        assert_eq!(
            asked(&runner)[..3],
            [
                "systemctl list-units --type=service --all --no-pager --output=json",
                "systemctl list-unit-files --type=service --no-pager --output=json",
                "systemctl show --no-pager --property=Id,MainPID *.service",
            ]
        );
        let ssh = by_name(&rows, "System", "ssh.service");
        assert_eq!((ssh.display.as_str(), ssh.status.as_str(), ssh.start_type.as_str(), ssh.pid, ssh.source.as_str()), ("OpenBSD Secure Shell server", "Running", "Enabled", 812, "systemd"));
        let cron = by_name(&rows, "System", "cron.service");
        assert_eq!((cron.status.as_str(), cron.start_type.as_str(), cron.pid), ("Stopped", "Disabled", 0));
        let backup = by_name(&rows, "System", "backup.service");
        assert_eq!((backup.status.as_str(), backup.start_type.as_str(), backup.pid), ("Exited", "Static", 0));
        let idle = by_name(&rows, "System", "idle.service");
        assert_eq!((idle.status.as_str(), idle.start_type.as_str()), ("Stopped", "Masked"), "an unloaded unit file is listed too");
        assert!(rows.iter().all(|r| r.name != "ghost.service" && r.name != "getty@.service"), "dangling references and templates are not services: {rows:?}");
    }

    #[test]
    fn user_services_are_listed_with_their_scope() {
        let runner = RecordingRunner::default();
        for (n, text) in [UNITS_JSON, FILES_JSON, SHOW].into_iter().enumerate() {
            runner.prepare(When::Nth(n), 0, text);
        }
        runner.prepare(When::Nth(3), 0, r#"[{"unit":"pipewire.service","load":"loaded","active":"active","sub":"running","description":"PipeWire Multimedia Service"}]"#);
        runner.prepare(When::Nth(4), 0, r#"[{"unit_file":"pipewire.service","state":"enabled","preset":"enabled"}]"#);
        runner.prepare(When::Nth(5), 0, "Id=pipewire.service\nMainPID=2210\n");
        let rows = list(&runner).unwrap();

        assert_eq!(
            asked(&runner)[3..],
            [
                "systemctl --user list-units --type=service --all --no-pager --output=json",
                "systemctl --user list-unit-files --type=service --no-pager --output=json",
                "systemctl --user show --no-pager --property=Id,MainPID *.service",
            ]
        );
        let pipewire = by_name(&rows, "User", "pipewire.service");
        assert_eq!((pipewire.pid, pipewire.status.as_str(), pipewire.start_type.as_str()), (2210, "Running", "Enabled"));
        assert_eq!(by_name(&rows, "System", "ssh.service").scope, "System");
    }

    #[test]
    fn old_systemd_without_json_output_is_read_as_plain_text() {
        let runner = RecordingRunner::default();
        // The JSON calls fail as systemd 219 does ("Unknown output 'json'").
        runner.prepare(When::Nth(0), 1, "Unknown output 'json'.");
        runner.prepare(When::Nth(1), 0, UNITS_OLD);
        runner.prepare(When::Nth(2), 1, "Unknown output 'json'.");
        runner.prepare(When::Nth(3), 0, FILES_OLD);
        runner.prepare(When::Nth(4), 0, "Id=ssh.service\nMainPID=77\n");
        let rows = list(&runner).unwrap();

        assert_eq!(
            asked(&runner)[..5],
            [
                "systemctl list-units --type=service --all --no-pager --output=json",
                "systemctl list-units --type=service --all --no-pager --no-legend --plain",
                "systemctl list-unit-files --type=service --no-pager --output=json",
                "systemctl list-unit-files --type=service --no-pager --no-legend",
                "systemctl show --no-pager --property=Id,MainPID *.service",
            ]
        );
        let ssh = by_name(&rows, "System", "ssh.service");
        assert_eq!((ssh.display.as_str(), ssh.status.as_str(), ssh.start_type.as_str(), ssh.pid), ("OpenBSD Secure Shell server", "Running", "Enabled", 77));
        assert_eq!(by_name(&rows, "System", "cron.service").status, "Stopped");
        let broken = by_name(&rows, "System", "broken.service");
        assert_eq!((broken.status.as_str(), broken.display.as_str()), ("Failed", "A broken one"), "the bullet an old systemctl puts on failed units is skipped");
    }

    #[test]
    fn json_that_does_not_parse_falls_back_to_plain_text() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, "not json at all");
        runner.prepare(When::Nth(1), 0, UNITS_OLD);
        let rows = list(&runner).unwrap();
        assert!(rows.iter().any(|r| r.name == "ssh.service"));
    }

    #[test]
    fn no_systemctl_means_the_source_is_absent() {
        // Nothing prepared: the runner reports the program could not start.
        assert_eq!(list(&RecordingRunner::default()).err(), Some(SourceError::Absent));
    }

    #[test]
    fn a_system_that_is_not_booted_with_systemd_means_the_source_is_absent() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, "System has not been booted with systemd as init system (PID 1). Can't operate.");
        runner.prepare(When::Nth(1), 1, "System has not been booted with systemd as init system (PID 1). Can't operate.");
        assert_eq!(list(&runner).err(), Some(SourceError::Absent));
    }

    #[test]
    fn a_systemctl_that_fails_says_why() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, "Unknown output 'json'.");
        runner.prepare(When::Nth(1), 5, "Access denied\nmore");
        assert_eq!(list(&runner).err(), Some(SourceError::Failed("Access denied".into())));
    }

    /// Runs `control` with a launcher that records its argv and answers `answer`.
    fn controlled(scope: &str, name: &str, action: &str, answer: Spawned) -> (Result<(), String>, Vec<Vec<String>>) {
        let launched = std::cell::RefCell::new(Vec::new());
        let launch = |argv: &[String]| {
            launched.borrow_mut().push(argv.to_vec());
            answer.clone()
        };
        let env = |_: &str| None;
        let result = Systemd.control(&ControlHost { launch: &launch, env: &env }, scope, name, action);
        (result, launched.into_inner())
    }

    fn ok() -> Spawned {
        Spawned::Exited { code: 0, stderr: String::new() }
    }

    fn words(line: &str) -> Vec<String> {
        line.split(' ').map(String::from).collect()
    }

    #[test]
    fn a_system_service_is_controlled_through_pkexec() {
        for action in ["start", "stop", "restart", "enable", "disable"] {
            let (result, launched) = controlled("System", "ssh.service", action, ok());
            assert_eq!(result, Ok(()));
            assert_eq!(launched, [words(&format!("pkexec systemctl {action} ssh.service"))]);
        }
    }

    #[test]
    fn an_instance_unit_with_escapes_reaches_systemctl_unchanged() {
        let name = "systemd-fsck@dev-disk-by\\x2dlabel-DATA.service";
        let (result, launched) = controlled("System", name, "restart", ok());
        assert_eq!(result, Ok(()));
        assert_eq!(launched, [vec!["pkexec".to_string(), "systemctl".into(), "restart".into(), name.into()]]);
    }

    #[test]
    fn a_user_service_is_controlled_without_pkexec() {
        let (result, launched) = controlled("User", "pipewire.service", "restart", ok());
        assert_eq!(result, Ok(()));
        assert_eq!(launched, [words("systemctl --user restart pipewire.service")]);
    }

    #[test]
    fn a_system_and_a_user_service_of_one_name_are_told_apart_by_scope() {
        assert_eq!(controlled("System", "sync.service", "stop", ok()).1, [words("pkexec systemctl stop sync.service")]);
        assert_eq!(controlled("User", "sync.service", "stop", ok()).1, [words("systemctl --user stop sync.service")]);
    }

    #[test]
    fn refusal_dismissal_and_a_missing_pkexec_are_worded_plainly() {
        let (refused, _) = controlled("System", "ssh.service", "stop", Spawned::Exited { code: 127, stderr: String::new() });
        assert_eq!(refused, Err(action_elevation::PERMISSION_REFUSED.to_string()));
        let (dismissed, _) = controlled("System", "ssh.service", "stop", Spawned::Exited { code: 126, stderr: String::new() });
        assert_eq!(dismissed, Err(action_elevation::PERMISSION_DISMISSED.to_string()));
        let (missing, _) = controlled("System", "ssh.service", "stop", Spawned::NotFound);
        assert_eq!(missing, Err(action_elevation::PKEXEC_MISSING.to_string()));
    }

    #[test]
    fn what_systemctl_says_is_the_error_and_a_missing_user_systemctl_is_named() {
        let (failed, _) = controlled("System", "ssh.service", "start", Spawned::Exited { code: 1, stderr: "Failed to start ssh.service: Unit ssh.service not found.\n".into() });
        assert_eq!(failed, Err("Failed to start ssh.service: Unit ssh.service not found.".to_string()));
        let (silent, _) = controlled("User", "x.service", "start", Spawned::Exited { code: 5, stderr: String::new() });
        assert_eq!(silent, Err("systemctl failed (exit code 5).".to_string()));
        let (missing, _) = controlled("User", "x.service", "start", Spawned::NotFound);
        assert_eq!(missing, Err("systemctl was not found on this system.".to_string()));
    }

    #[test]
    fn an_unknown_action_or_a_name_that_is_not_a_unit_starts_nothing() {
        for (name, action) in [("ssh.service", "mask"), ("--now", "start"), ("a b.service", "start"), ("", "start"), ("it's.service", "stop")] {
            let (result, launched) = controlled("System", name, action, ok());
            assert!(result.is_err(), "{name} {action}");
            assert!(launched.is_empty(), "nothing is launched for {name} {action}");
        }
    }

    #[test]
    fn a_pid_is_kept_only_for_a_running_service() {
        let pids = parse_show("Id=a.service\nMainPID=10\n\nId=b.service\nMainPID=0\n");
        assert_eq!(pids, HashMap::from([("a.service".to_string(), 10)]));
    }
}
