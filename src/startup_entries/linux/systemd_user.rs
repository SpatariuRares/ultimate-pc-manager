//! systemd user units: the services the user's service manager starts with
//! the session.
//!
//! The enabled and the disabled user services are read with `systemctl --user
//! list-unit-files --state=enabled,disabled` (the same reader the Services page
//! uses, JSON when this systemd has it, plain text when not). A disabled
//! unit is listed (shown off, so it can be turned on again) only when its
//! `[Install]` group says the session starts it (`WantedBy=default.target`
//! or `graphical-session.target`), read from the unit files; masked and
//! static units are never listed. That list is used rather than the
//! `default.target.wants` folders because a unit can be wanted by other
//! targets of the session (such as `graphical-session.target`), and only the
//! manager knows every place a unit is enabled from. Turning an entry on or
//! off is `systemctl --user enable|disable <unit>`: user scope, so no
//! elevation is involved.

use super::super::{StartupEntry, StartupSource};
use crate::app_folders::{xdg_config_home, Env};
use crate::fs_root::FsRoot;
use crate::platform::{ProcessRunner, SystemRunner};
use crate::services::sources::{run_text, Output, SourceError};
use crate::services::systemd::{ask, failure, parse_files_json, parse_files_text};
use std::sync::Arc;

/// Names this source in [`StartupEntry::source`].
pub const ID: &str = "systemd-user";
const LOCATION: &str = "User service";
const GONE: &str = "That startup app is no longer in the list";

/// The targets a session reaches: a unit wanted by one of them starts with it.
const SESSION_TARGETS: [&str; 2] = ["default.target", "graphical-session.target"];

pub struct SystemdUser {
    runner: Arc<dyn ProcessRunner>,
    /// Where the unit files are read from.
    root: FsRoot,
    /// The folders of user unit files under the root, the first that has a unit wins.
    unit_dirs: Vec<String>,
}

impl SystemdUser {
    /// The user services `runner` lists, their unit files read under `root`.
    pub fn over(runner: Arc<dyn ProcessRunner>, root: FsRoot, env: Env) -> Self {
        let mut unit_dirs = Vec::new();
        if let Some(config) = xdg_config_home(env) {
            unit_dirs.push(format!("{}/systemd/user", config.to_string_lossy().trim_start_matches('/')));
        }
        unit_dirs.extend(["etc/systemd/user", "usr/lib/systemd/user", "lib/systemd/user"].map(String::from));
        SystemdUser { runner, root, unit_dirs }
    }

    /// The running system's user services.
    pub fn system() -> Self {
        Self::over(Arc::new(SystemRunner), FsRoot::system(), &|name| std::env::var_os(name))
    }

    /// Does the session start `unit` (its `[Install]` group names a session target)?
    fn wanted_by_session(&self, unit: &str) -> bool {
        let Some(text) = self.unit_dirs.iter().find_map(|d| self.root.read(&format!("{d}/{unit}"))) else { return false };
        let mut install = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                install = line == "[Install]";
            } else if install {
                if let Some(wants) = line.strip_prefix("WantedBy=") {
                    if wants.split_whitespace().any(|t| SESSION_TARGETS.contains(&t)) {
                        return true;
                    }
                }
            }
        }
        false
    }
}

impl StartupSource for SystemdUser {
    fn id(&self) -> &'static str {
        ID
    }

    fn list(&self) -> Result<Vec<StartupEntry>, String> {
        let files = ask(
            &*self.runner,
            &["--user", "list-unit-files", "--type=service", "--state=enabled,disabled", "--no-pager", "--output=json"],
            &["--user", "list-unit-files", "--type=service", "--state=enabled,disabled", "--no-pager", "--no-legend"],
            parse_files_json,
            parse_files_text,
        );
        let files = match files {
            Ok(files) => files,
            // No systemd, or no user manager running: nothing to list, nothing wrong.
            Err(SourceError::Absent) => return Ok(Vec::new()),
            Err(SourceError::Failed(why)) => return Err(why),
        };
        let mut rows: Vec<StartupEntry> = files
            .into_iter()
            // Templates (`name@.service`) are started per instance, not by themselves.
            .filter(|f| !f.name.contains("@.") && (f.state == "enabled" || (f.state == "disabled" && self.wanted_by_session(&f.name))))
            .map(|f| StartupEntry {
                name: f.name.trim_end_matches(".service").to_string(),
                location: LOCATION.to_string(),
                enabled: f.state == "enabled",
                key: f.name,
                ..Default::default()
            })
            .collect();
        rows.sort_by_key(|r| r.name.to_lowercase());
        Ok(rows)
    }

    fn set_enabled(&self, entry: &StartupEntry, enable: bool) -> Result<(), String> {
        let unit = entry.key.as_str();
        // A name from a listing is a unit file name; refuse anything that could be an option or a path.
        if !unit.ends_with(".service") || unit.starts_with('-') || unit.contains(['/', ' ']) {
            return Err(GONE.to_string());
        }
        let verb = if enable { "enable" } else { "disable" };
        match run_text(&*self.runner, "systemctl", &["--user", verb, unit]) {
            Output::Exited(0, _) => Ok(()),
            Output::NotStarted => Err("systemctl is not available.".to_string()),
            Output::Exited(code, text) => match failure(code, &text) {
                SourceError::Failed(why) => Err(why),
                SourceError::Absent => Err("The user service manager is not running.".to_string()),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{Launch, RecordingRunner, When};

    const FILES_JSON: &str = r#"[
      {"unit_file":"syncthing.service","state":"enabled","preset":"enabled"},
      {"unit_file":"Pipewire.service","state":"enabled","preset":"enabled"},
      {"unit_file":"app@.service","state":"enabled","preset":null},
      {"unit_file":"idle.service","state":"disabled","preset":null},
      {"unit_file":"on-demand.service","state":"disabled","preset":null},
      {"unit_file":"vendor.service","state":"disabled","preset":null},
      {"unit_file":"masked.service","state":"masked","preset":null},
      {"unit_file":"static.service","state":"static","preset":null}
    ]"#;

    fn source() -> (Arc<RecordingRunner>, SystemdUser) {
        let runner = Arc::new(RecordingRunner::default());
        let env: Env = &|name| (name == "HOME").then(|| "/home/user".into());
        (runner.clone(), SystemdUser::over(runner, crate::fs_root::fixture("user-units"), env))
    }

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

    fn entry(unit: &str) -> StartupEntry {
        StartupEntry { key: unit.into(), source: ID.into(), ..Default::default() }
    }

    #[test]
    fn enabled_and_session_disabled_user_services_are_listed_through_the_user_manager() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 0, FILES_JSON);
        let rows = source.list().unwrap();
        let seen: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.key.as_str(), r.location.as_str(), r.enabled)).collect();
        assert_eq!(
            seen,
            [
                ("idle", "idle.service", LOCATION, false),
                ("Pipewire", "Pipewire.service", LOCATION, true),
                ("syncthing", "syncthing.service", LOCATION, true),
                ("vendor", "vendor.service", LOCATION, false),
            ],
            "a disabled unit the session starts stays listed (off); one a timer starts, masked and static ones are left out"
        );
        assert_eq!(asked(&runner), ["systemctl --user list-unit-files --type=service --state=enabled,disabled --no-pager --output=json"]);
    }

    #[test]
    fn a_systemd_without_json_output_is_read_as_plain_text() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 1, "unknown option --output");
        runner.prepare(When::Nth(1), 0, "syncthing.service  enabled\nbroken.service enabled\n");
        let rows = source.list().unwrap();
        assert_eq!(rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), ["broken.service", "syncthing.service"]);
        assert_eq!(asked(&runner)[1], "systemctl --user list-unit-files --type=service --state=enabled,disabled --no-pager --no-legend");
    }

    #[test]
    fn no_user_manager_lists_nothing_and_is_not_a_failure() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 1, "Failed to connect to bus: No medium found");
        runner.prepare(When::Nth(1), 1, "Failed to connect to bus: No medium found");
        assert!(source.list().unwrap().is_empty());
    }

    #[test]
    fn a_systemctl_that_fails_says_why() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 1, "boom");
        runner.prepare(When::Nth(1), 1, "Something broke\nmore");
        assert_eq!(source.list().unwrap_err(), "Something broke");
    }

    #[test]
    fn turning_an_entry_off_and_on_asks_the_user_manager() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 0, "");
        runner.prepare(When::Nth(1), 0, "");
        source.set_enabled(&entry("syncthing.service"), false).unwrap();
        source.set_enabled(&entry("syncthing.service"), true).unwrap();
        assert_eq!(asked(&runner), ["systemctl --user disable syncthing.service", "systemctl --user enable syncthing.service"]);
    }

    #[test]
    fn a_refused_change_says_why() {
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 1, "Failed to disable unit: Unit file syncthing.service does not exist.\n");
        let err = source.set_enabled(&entry("syncthing.service"), false).unwrap_err();
        assert_eq!(err, "Failed to disable unit: Unit file syncthing.service does not exist.");
    }

    #[test]
    fn a_name_that_is_not_a_unit_is_refused_without_running_anything() {
        let (runner, source) = source();
        for bad in ["", "--now", "-x.service", "../x.service", "a b.service", "x.timer"] {
            assert_eq!(source.set_enabled(&entry(bad), true), Err(GONE.to_string()), "{bad:?}");
        }
        assert!(runner.launches().is_empty());
    }

    #[test]
    fn a_failing_user_manager_does_not_hide_the_other_sources() {
        use super::super::super::sources::list_all;
        let (runner, source) = source();
        runner.prepare(When::Nth(0), 1, "boom");
        runner.prepare(When::Nth(1), 1, "boom");
        let sources: Vec<Box<dyn StartupSource>> = vec![Box::new(source), Box::new(Fixed)];
        let (entries, failures) = list_all(&sources);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].source, "fixed");
        assert_eq!(failures, [(ID, "boom".to_string())]);
    }

    struct Fixed;
    impl StartupSource for Fixed {
        fn id(&self) -> &'static str {
            "fixed"
        }
        fn list(&self) -> Result<Vec<StartupEntry>, String> {
            Ok(vec![StartupEntry { name: "x".into(), ..Default::default() }])
        }
        fn set_enabled(&self, _: &StartupEntry, _: bool) -> Result<(), String> {
            Ok(())
        }
    }
}
