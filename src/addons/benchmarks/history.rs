//! The benchmark history: one entry per completed run, kept in
//! `benchmark-history.json` in the app's data folder. Add a run, read the
//! runs of one test, clear one test; a file that cannot be read is set
//! aside and replaced, with a warning, so the page keeps working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The history file's name, in the app's data folder.
pub const FILE: &str = "benchmark-history.json";

/// What the history panel says while a test has no runs.
pub const EMPTY: &str = "Benchmark runs will appear here as they complete.";

/// One number a run measured, as saved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedFigure {
    pub name: String,
    pub value: f64,
    pub unit: String,
}

/// One completed run.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Run {
    /// The test's short name ("CPU").
    pub test: String,
    /// When it completed, ISO-8601 local time ("2026-11-17T21:30:00").
    pub date: String,
    pub figures: Vec<SavedFigure>,
    /// Its score, when the test has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    /// The app version that ran it.
    pub version: String,
    /// "windows x86_64".
    pub platform: String,
    /// The hardware it tested ("AMD Ryzen 5 7600X · 6 cores, 12 threads").
    pub hardware: String,
}

/// The file's content.
#[derive(Default, Serialize, Deserialize)]
struct Saved {
    runs: Vec<Run>,
}

/// The runs, and the file they are kept in (none: kept in memory only).
#[derive(Default, Debug)]
pub struct History {
    path: Option<PathBuf>,
    runs: Vec<Run>,
}

impl History {
    /// Reads the history at `path` (none: in memory only). A missing file
    /// is an empty history; a file that cannot be read is renamed beside
    /// itself, a new empty one is written, and the warning tells so.
    pub fn open(path: Option<PathBuf>) -> (History, Option<String>) {
        let Some(path) = path else { return (History::default(), None) };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (History { path: Some(path), runs: Vec::new() }, None),
            Err(e) => {
                // Not even readable: keep it in place, keep runs in memory.
                return (History::default(), Some(format!("The benchmark history could not be read ({e}); new runs will not be saved.")));
            }
        };
        if let Ok(saved) = serde_json::from_str::<Saved>(&text) {
            return (History { path: Some(path), runs: saved.runs }, None);
        }
        let aside = set_aside_name(&path, crate::platform::local_secs());
        let history = History { path: Some(path.clone()), runs: Vec::new() };
        let warning = match std::fs::rename(&path, &aside).map_err(|e| e.to_string()).and_then(|_| history.write()) {
            Ok(()) => format!(
                "The benchmark history could not be read. It was set aside as {} and a new one was started.",
                aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            ),
            Err(e) => format!("The benchmark history could not be read and could not be replaced ({e})."),
        };
        (history, Some(warning))
    }

    /// Adds a completed run and saves the file.
    pub fn add(&mut self, run: Run) -> Result<(), String> {
        self.runs.push(run);
        self.write()
    }

    /// The runs of `test`, newest first.
    pub fn runs(&self, test: &str) -> Vec<&Run> {
        self.runs.iter().rev().filter(|r| r.test == test).collect()
    }

    /// Removes every run of `test` (the other tests keep theirs) and saves
    /// the file.
    pub fn clear(&mut self, test: &str) -> Result<(), String> {
        self.runs.retain(|r| r.test != test);
        self.write()
    }

    /// Writes the file (a temp file, then renamed over it).
    fn write(&self) -> Result<(), String> {
        let Some(path) = &self.path else { return Ok(()) };
        let text = serde_json::to_string_pretty(&Saved { runs: self.runs.clone() }).map_err(|e| e.to_string())?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("Couldn't save the benchmark history: {e}"))
    }
}

/// "benchmark-history.unreadable-<secs>.json", beside `path`.
fn set_aside_name(path: &Path, now: i64) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}.unreadable-{now}.json"))
}

/// The history file in the app's data folder, beside its settings.
#[cfg_attr(test, allow(dead_code))]
pub fn app_file() -> Option<PathBuf> {
    Some(crate::settings::app_dir()?.join(FILE))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fresh folder under the temp dir, removed when dropped.
    pub(crate) struct TempDir(pub PathBuf);
    impl TempDir {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mtm-bench-history-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempDir(dir)
        }
        pub fn file(&self) -> PathBuf {
            self.0.join(FILE)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn run(test: &str, date: &str, value: f64) -> Run {
        Run {
            test: test.into(),
            date: date.into(),
            figures: vec![SavedFigure { name: "Single core".into(), value, unit: "Mops/sec".into() }],
            score: None,
            version: "1.2.3".into(),
            platform: "windows x86_64".into(),
            hardware: "Test CPU".into(),
        }
    }

    #[test]
    fn added_runs_are_saved_and_read_back_per_test_newest_first() {
        let dir = TempDir::new("add");
        let (mut h, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None, "no file yet: an empty history, no warning");
        assert!(h.runs("CPU").is_empty());
        h.add(run("CPU", "2026-11-16T10:00:00", 1000.0)).unwrap();
        h.add(run("Disk", "2026-11-16T11:00:00", 500.0)).unwrap();
        h.add(run("CPU", "2026-11-17T21:30:00", 1234.0)).unwrap();

        let (again, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None);
        let dates: Vec<_> = again.runs("CPU").iter().map(|r| r.date.as_str()).collect();
        assert_eq!(dates, ["2026-11-17T21:30:00", "2026-11-16T10:00:00"]);
        assert_eq!(again.runs("CPU")[0], &run("CPU", "2026-11-17T21:30:00", 1234.0));
        assert_eq!(again.runs("Disk").len(), 1);
        assert!(again.runs("Memory").is_empty());
    }

    #[test]
    fn clearing_a_test_removes_only_its_runs() {
        let dir = TempDir::new("clear");
        let (mut h, _) = History::open(Some(dir.file()));
        h.add(run("CPU", "2026-11-16T10:00:00", 1000.0)).unwrap();
        h.add(run("Disk", "2026-11-16T11:00:00", 500.0)).unwrap();
        h.clear("CPU").unwrap();
        assert!(h.runs("CPU").is_empty());
        let (again, _) = History::open(Some(dir.file()));
        assert!(again.runs("CPU").is_empty(), "the clear is saved");
        assert_eq!(again.runs("Disk").len(), 1, "other tests keep their runs");
    }

    #[test]
    fn an_unreadable_file_is_set_aside_and_replaced_with_a_warning() {
        let dir = TempDir::new("corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.file(), "{ not json").unwrap();
        let (mut h, warning) = History::open(Some(dir.file()));
        let warning = warning.expect("a warning");
        assert!(warning.starts_with("The benchmark history could not be read. It was set aside as benchmark-history.unreadable-"), "{warning}");
        assert!(h.runs("CPU").is_empty());

        let names: Vec<String> = std::fs::read_dir(&dir.0).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        let aside: Vec<_> = names.iter().filter(|n| n.starts_with("benchmark-history.unreadable-")).collect();
        assert_eq!(aside.len(), 1, "{names:?}");
        assert_eq!(std::fs::read_to_string(dir.0.join(aside[0])).unwrap(), "{ not json", "the old file is kept as it was");

        h.add(run("CPU", "2026-11-17T21:30:00", 1234.0)).unwrap();
        let (again, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None, "the new file reads");
        assert_eq!(again.runs("CPU").len(), 1);
    }

    #[test]
    fn a_history_without_a_file_keeps_runs_in_memory() {
        let (mut h, warning) = History::open(None);
        assert_eq!(warning, None);
        h.add(run("CPU", "2026-11-17T21:30:00", 1.0)).unwrap();
        assert_eq!(h.runs("CPU").len(), 1);
    }
}
