//! The service managers a system can run side by side (Linux: systemd now,
//! others later), each as one [`ServiceSource`].
//!
//! Several sources coexist at runtime, so a trait is the right shape here:
//! each source lists its own services, marks every row with itself and the
//! scope the service belongs to, and may fail without hiding the others.
//! Control (start, stop, restart, enable and disable at boot) is a method of
//! the same trait that a source implements when it can; the default says it
//! cannot. Every external program goes through the process runner (listing)
//! or the launcher (control, which may need administrator rights).

use std::sync::atomic::AtomicBool;

use crate::action_elevation::Spawned;
use crate::availability::{unavailable_error, Feature};
use crate::platform::{ProcessRunner, Ran};
use crate::system_lists::{ServiceKey, ServiceRow};

/// What control needs from the system: how a program is started and waited
/// for (the one place a launch happens, so tests see command lines and start
/// nothing) and the environment.
pub struct ControlHost<'a> {
    pub launch: &'a dyn Fn(&[String]) -> Spawned,
    pub env: &'a dyn Fn(&str) -> Option<String>,
}

/// Why a source gave no list.
#[derive(Debug, PartialEq, Eq)]
pub enum SourceError {
    /// This service manager is not what the system runs (its program is
    /// missing, or it is not the running init): not a failure.
    Absent,
    /// It is there but could not be read; why.
    Failed(String),
}

/// One service manager.
pub trait ServiceSource {
    /// The name rows carry in [`ServiceRow::source`].
    fn name(&self) -> &'static str;

    /// Every service of this source, in every scope it has.
    fn list(&self, runner: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError>;

    /// Starts, stops or restarts the service `name` of `scope`, or turns it
    /// on or off at boot (`action` is "start", "stop", "restart", "enable"
    /// or "disable"). A source that cannot control services keeps this
    /// default.
    fn control(&self, _host: &ControlHost, _scope: &str, _name: &str, _action: &str) -> Result<(), String> {
        Err(format!("Services of {} cannot be started or stopped from here.", self.name()))
    }
}

/// Asks the source that lists `service` to do `action` on it.
pub fn control(sources: &[Box<dyn ServiceSource>], host: &ControlHost, service: &ServiceKey, action: &str) -> Result<(), String> {
    match sources.iter().find(|s| s.name() == service.source) {
        Some(source) => source.control(host, &service.scope, &service.name, action),
        None => Err(format!("\"{}\" belongs to a service manager this app does not know.", service.name)),
    }
}

/// The services of every source, sorted by name. A source that is absent is
/// skipped and one that fails does not hide the others; the list is an error
/// only when no source is present at all, or every present one failed.
pub fn list_all(sources: &[Box<dyn ServiceSource>], runner: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, String> {
    let mut rows = Vec::new();
    let mut failures = Vec::new();
    let mut present = 0;
    for source in sources {
        match source.list(runner) {
            Ok(list) => {
                present += 1;
                rows.extend(list);
            }
            Err(SourceError::Absent) => {}
            Err(SourceError::Failed(why)) => {
                present += 1;
                failures.push(format!("{}: {why}", source.name()));
            }
        }
    }
    if present == 0 {
        return Err(unavailable_error(Feature::ServiceListing));
    }
    if present == failures.len() {
        return Err(failures.join("; "));
    }
    rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then_with(|| a.scope.cmp(&b.scope)));
    Ok(rows)
}

/// How a program ended: its exit code and its output, or that it could not
/// be started.
pub enum Output {
    NotStarted,
    Exited(i32, String),
}

/// Runs `program` through the runner, its output in a temporary file that
/// is deleted afterwards.
pub fn run_text(runner: &dyn ProcessRunner, program: &str, args: &[&str]) -> Output {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let file = std::env::temp_dir().join(format!("upcm-services-{}-{nanos}-{n}.txt", std::process::id()));
    let ran = runner.run(program, args, &file, &AtomicBool::new(false), &mut |_| {});
    let text = std::fs::read(&file).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    match ran {
        Ran::Exited(code) => Output::Exited(code, text),
        Ran::NotStarted(_) | Ran::Stopped => Output::NotStarted,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::RecordingRunner;

    struct Fixed(&'static str, Result<Vec<ServiceRow>, SourceError>);

    fn row(name: &str, source: &str) -> ServiceRow {
        ServiceRow { name: name.into(), source: source.into(), ..Default::default() }
    }

    impl ServiceSource for Fixed {
        fn name(&self) -> &'static str {
            self.0
        }
        fn list(&self, _: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError> {
            match &self.1 {
                Ok(rows) => Ok(rows.clone()),
                Err(SourceError::Absent) => Err(SourceError::Absent),
                Err(SourceError::Failed(why)) => Err(SourceError::Failed(why.clone())),
            }
        }
    }

    struct Controlling;

    impl ServiceSource for Controlling {
        fn name(&self) -> &'static str {
            "ctl"
        }
        fn list(&self, _: &dyn ProcessRunner) -> Result<Vec<ServiceRow>, SourceError> {
            Err(SourceError::Absent)
        }
        fn control(&self, _: &ControlHost, scope: &str, name: &str, action: &str) -> Result<(), String> {
            Err(format!("{action} {scope} {name}"))
        }
    }

    fn with_host<T>(f: impl FnOnce(&ControlHost) -> T) -> T {
        f(&ControlHost { launch: &|_| Spawned::Exited { code: 0, stderr: String::new() }, env: &|_| None })
    }

    fn key(name: &str, source: &str, scope: &str) -> ServiceKey {
        ServiceKey { name: name.into(), source: source.into(), scope: scope.into() }
    }

    #[test]
    fn control_goes_to_the_source_named_by_the_key_with_its_scope() {
        let sources: Vec<Box<dyn ServiceSource>> = vec![Box::new(Fixed("a", Ok(Vec::new()))), Box::new(Controlling)];
        let said = with_host(|h| control(&sources, h, &key("x.service", "ctl", "User"), "stop"));
        assert_eq!(said, Err("stop User x.service".to_string()));
    }

    #[test]
    fn a_source_without_control_says_so_and_an_unknown_source_is_refused() {
        let sources: Vec<Box<dyn ServiceSource>> = vec![Box::new(Fixed("a", Ok(Vec::new())))];
        let no_control = with_host(|h| control(&sources, h, &key("x", "a", ""), "start")).unwrap_err();
        assert_eq!(no_control, "Services of a cannot be started or stopped from here.");
        let unknown = with_host(|h| control(&sources, h, &key("x", "zzz", ""), "start")).unwrap_err();
        assert!(unknown.contains("does not know"), "{unknown}");
        let nameless = with_host(|h| control(&sources, h, &ServiceKey::from("x"), "start"));
        assert!(nameless.is_err(), "a key without a manager reaches no source");
    }

    fn boxed(list: Vec<Fixed>) -> Vec<Box<dyn ServiceSource>> {
        list.into_iter().map(|f| Box::new(f) as Box<dyn ServiceSource>).collect()
    }

    #[test]
    fn a_failing_source_does_not_hide_the_others() {
        let sources = boxed(vec![Fixed("a", Ok(vec![row("zeta", "a")])), Fixed("b", Err(SourceError::Failed("boom".into()))), Fixed("c", Ok(vec![row("Alpha", "c")]))]);
        let rows = list_all(&sources, &RecordingRunner::default()).unwrap();
        let names: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.source.as_str())).collect();
        assert_eq!(names, [("Alpha", "c"), ("zeta", "a")], "sorted by name, each row keeps its source");
    }

    #[test]
    fn when_every_present_source_fails_the_reasons_are_the_error() {
        let sources = boxed(vec![Fixed("a", Err(SourceError::Failed("boom".into()))), Fixed("b", Err(SourceError::Absent)), Fixed("c", Err(SourceError::Failed("bang".into())))]);
        assert_eq!(list_all(&sources, &RecordingRunner::default()).unwrap_err(), "a: boom; c: bang");
    }

    #[test]
    fn with_no_source_present_the_list_is_not_available() {
        let sources = boxed(vec![Fixed("a", Err(SourceError::Absent))]);
        assert_eq!(list_all(&sources, &RecordingRunner::default()).unwrap_err(), unavailable_error(Feature::ServiceListing));
    }

    #[test]
    fn a_present_source_with_no_services_is_an_empty_list_not_an_error() {
        let sources = boxed(vec![Fixed("a", Ok(Vec::new())), Fixed("b", Err(SourceError::Absent))]);
        assert!(list_all(&sources, &RecordingRunner::default()).unwrap().is_empty());
    }
}
