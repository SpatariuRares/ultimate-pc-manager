//! The service managers a system can run side by side (Linux: systemd now,
//! others later), each as one [`ServiceSource`].
//!
//! Several sources coexist at runtime, so a trait is the right shape here:
//! each source lists its own services, marks every row with itself and the
//! scope the service belongs to, and may fail without hiding the others.
//! Control (start, stop, restart) is added per source, on the same trait, by
//! the tickets that need it. Every external program goes through the process
//! runner.

use std::sync::atomic::AtomicBool;

use crate::availability::{unavailable_error, Feature};
use crate::platform::{ProcessRunner, Ran};
use crate::system_lists::ServiceRow;

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
