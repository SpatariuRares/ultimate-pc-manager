//! The benchmark history: one entry per completed run, kept in
//! `benchmark-history.json` in the app's data folder. Add a run, read the
//! runs of one test, clear one test; a file that cannot be read is set
//! aside and replaced, with a warning, so the page keeps working.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The history file's name, in the app's data folder.
pub const FILE: &str = "benchmark-history.json";

/// The revision of the GPU test whose results the file holds. A file
/// without the number was written when the GPU test used Direct3D 11
/// (revision 0); the wgpu test is revision 1.
pub const GPU_TEST_REVISION: u32 = 1;

/// The name the GPU test's runs are saved under.
const GPU_TEST: &str = "GPU";

/// The one-time line shown after older GPU results were removed.
pub const GPU_PURGED_NOTICE: &str = "Older GPU results were removed: the GPU test changed";

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
    /// The GPU test revision the runs were made with (absent: 0).
    #[serde(default)]
    gpu_test: u32,
}

/// The runs, and the file they are kept in (none: kept in memory only).
#[derive(Default, Debug)]
pub struct History {
    path: Option<PathBuf>,
    runs: Vec<Run>,
    /// The GPU test revision the runs in the file belong to.
    gpu_test: u32,
    /// The one-time line for a purge made by `open`.
    notice: Option<String>,
}

impl History {
    /// Reads the history at `path` (none: in memory only). A missing file
    /// is an empty history; a file that cannot be read is renamed beside
    /// itself, a new empty one is written, and the warning tells so.
    pub fn open(path: Option<PathBuf>) -> (History, Option<String>) {
        let Some(path) = path else { return (History { gpu_test: GPU_TEST_REVISION, ..History::default() }, None) };
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (History { path: Some(path), gpu_test: GPU_TEST_REVISION, ..History::default() }, None),
            Err(e) => {
                // Not even readable: keep it in place, keep runs in memory.
                return (History { gpu_test: GPU_TEST_REVISION, ..History::default() }, Some(format!("The benchmark history could not be read ({e}); new runs will not be saved.")));
            }
        };
        if let Ok(saved) = serde_json::from_str::<Saved>(&text) {
            let mut history = History { path: Some(path), runs: saved.runs, gpu_test: saved.gpu_test, notice: None };
            let warning = history.purge_old_gpu_results();
            return (history, warning);
        }
        let aside = set_aside_name(&path, crate::platform::local_secs());
        let history = History { path: Some(path.clone()), gpu_test: GPU_TEST_REVISION, ..History::default() };
        let warning = match std::fs::rename(&path, &aside).map_err(|e| e.to_string()).and_then(|_| history.write()) {
            Ok(()) => format!(
                "The benchmark history could not be read. It was set aside as {} and a new one was started.",
                aside.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            ),
            Err(e) => format!("The benchmark history could not be read and could not be replaced ({e})."),
        };
        (history, Some(warning))
    }

    /// Removes, once, the GPU runs made by the old GPU test: when the file
    /// belongs to an older GPU test revision, its "GPU" runs go and the
    /// file is saved with the current revision. Every other run stays. The
    /// file is replaced only by a complete new one (temp file, then rename),
    /// so a failed save leaves it as it was; the runs and the revision then
    /// stay as they were too and the purge is tried again at the next start.
    /// Returns a warning when the save failed.
    fn purge_old_gpu_results(&mut self) -> Option<String> {
        if self.gpu_test >= GPU_TEST_REVISION {
            return None;
        }
        let before = self.runs.len();
        let kept: Vec<Run> = self.runs.iter().filter(|r| r.test != GPU_TEST).cloned().collect();
        let removed = kept.len() != before;
        let purged = History { path: self.path.clone(), runs: kept, gpu_test: GPU_TEST_REVISION, notice: None };
        match purged.write() {
            Ok(()) => {
                self.runs = purged.runs;
                self.gpu_test = GPU_TEST_REVISION;
                if removed {
                    self.notice = Some(GPU_PURGED_NOTICE.to_string());
                }
                None
            }
            Err(e) => Some(format!("Older GPU results could not be removed ({e}); they stay for now.")),
        }
    }

    /// The one-time line about the GPU results `open` removed, if any.
    pub fn take_notice(&mut self) -> Option<String> {
        self.notice.take()
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
        let text = serde_json::to_string_pretty(&Saved { runs: self.runs.clone(), gpu_test: self.gpu_test }).map_err(|e| e.to_string())?;
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

    /// A history file as the version with the Direct3D 11 GPU test wrote it:
    /// no GPU test revision in it.
    const OLD_FILE: &str = r#"{
  "runs": [
    {"test": "CPU", "date": "2026-11-10T10:00:00", "figures": [{"name": "Single core", "value": 1.0, "unit": "Mops/sec"}], "score": 100.0, "version": "0.3.1", "platform": "windows x86_64", "hardware": "CPU"},
    {"test": "GPU", "date": "2026-11-11T10:00:00", "figures": [{"name": "Triangles", "value": 2.0, "unit": "Gtri/sec"}], "score": 900.0, "version": "0.3.1", "platform": "windows x86_64", "hardware": "Card"},
    {"test": "Memory", "date": "2026-11-12T10:00:00", "figures": [], "version": "0.3.1", "platform": "windows x86_64", "hardware": "RAM"},
    {"test": "GPU", "date": "2026-11-13T10:00:00", "figures": [], "score": 950.0, "version": "0.3.2", "platform": "windows x86_64", "hardware": "Card"},
    {"test": "Disk", "date": "2026-11-14T10:00:00", "figures": [], "version": "0.3.1", "platform": "windows x86_64", "hardware": "SSD"},
    {"test": "Internet", "date": "2026-11-15T10:00:00", "figures": [], "version": "0.3.1", "platform": "windows x86_64", "hardware": "Net"}
  ]
}"#;

    fn open_old(name: &str) -> (TempDir, History, Option<String>) {
        let dir = TempDir::new(name);
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.file(), OLD_FILE).unwrap();
        let (h, warning) = History::open(Some(dir.file()));
        (dir, h, warning)
    }

    #[test]
    fn the_first_open_removes_only_the_old_gpu_results() {
        let (dir, mut h, warning) = open_old("purge-only-gpu");
        assert_eq!(warning, None);
        assert!(h.runs("GPU").is_empty(), "old GPU results are gone");
        for test in ["CPU", "Memory", "Disk", "Internet"] {
            assert_eq!(h.runs(test).len(), 1, "{test} stays");
        }
        assert_eq!(h.take_notice().as_deref(), Some("Older GPU results were removed: the GPU test changed"));
        // The removal is saved.
        let (again, _) = History::open(Some(dir.file()));
        assert!(again.runs("GPU").is_empty());
        assert_eq!(again.runs("CPU")[0].score, Some(100.0), "kept runs are unchanged");
    }

    #[test]
    fn the_removal_happens_once() {
        let (dir, mut h, _) = open_old("purge-once");
        assert!(h.take_notice().is_some());
        // A GPU run made by the new test is kept by every later start.
        h.add(run("GPU", "2026-11-20T10:00:00", 5.0)).unwrap();
        let (mut later, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None);
        assert_eq!(later.runs("GPU").len(), 1, "later starts remove nothing");
        assert_eq!(later.take_notice(), None);
        let (third, _) = History::open(Some(dir.file()));
        assert_eq!(third.runs("GPU").len(), 1);
    }

    #[test]
    fn the_line_is_shown_once_after_the_removal() {
        let (_dir, mut h, _) = open_old("purge-notice");
        assert!(h.take_notice().is_some());
        assert_eq!(h.take_notice(), None, "it is taken only once");
    }

    #[test]
    fn no_line_when_there_was_nothing_to_remove() {
        let dir = TempDir::new("purge-nothing");
        std::fs::create_dir_all(&dir.0).unwrap();
        let cpu_only = OLD_FILE.lines().filter(|l| !l.contains("\"GPU\"")).collect::<Vec<_>>().join("
");
        std::fs::write(dir.file(), cpu_only).unwrap();
        let (mut h, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None);
        assert_eq!(h.runs("CPU").len(), 1);
        assert_eq!(h.take_notice(), None);
    }

    #[test]
    fn a_new_history_has_no_line_and_keeps_its_first_gpu_run_at_the_next_start() {
        let dir = TempDir::new("purge-fresh");
        let (mut h, warning) = History::open(Some(dir.file()));
        assert_eq!((warning, h.take_notice()), (None, None));
        h.add(run("GPU", "2026-11-20T10:00:00", 5.0)).unwrap();
        let (again, _) = History::open(Some(dir.file()));
        assert_eq!(again.runs("GPU").len(), 1);
    }

    #[test]
    fn a_failed_removal_keeps_the_file_and_its_runs_and_is_tried_again() {
        let dir = TempDir::new("purge-fails");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.file(), OLD_FILE).unwrap();
        // The temp file cannot be created: its name is a folder.
        std::fs::create_dir_all(dir.0.join("benchmark-history.json.tmp")).unwrap();
        let (mut h, warning) = History::open(Some(dir.file()));
        assert!(warning.is_some_and(|w| w.starts_with("Older GPU results could not be removed")));
        assert_eq!(std::fs::read_to_string(dir.file()).unwrap(), OLD_FILE, "the file is untouched");
        assert_eq!(h.runs("GPU").len(), 2, "no half removal in memory");
        assert_eq!(h.take_notice(), None, "nothing was removed");

        // The next start, with the obstacle gone, removes them.
        std::fs::remove_dir_all(dir.0.join("benchmark-history.json.tmp")).unwrap();
        let (mut next, warning) = History::open(Some(dir.file()));
        assert_eq!(warning, None);
        assert!(next.runs("GPU").is_empty());
        assert!(next.take_notice().is_some());
    }

    #[test]
    fn an_unreadable_file_is_still_set_aside_not_purged() {
        let dir = TempDir::new("purge-corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.file(), "{ not json").unwrap();
        let (mut h, _) = History::open(Some(dir.file()));
        assert_eq!(h.take_notice(), None);
    }
}
