//! The journal of a series: one JSON file per run in the add-on's folder
//! of the app's settings, rewritten atomically at every change of status.
//! At start-up an unfinished journal (the app closed, crashed or was
//! stopped during its series) offers to continue the packages it did not
//! install.

use std::path::{Path, PathBuf};

use egui::{RichText, Ui};

use crate::platform::lock_state;

use super::batch::{Item, Kind, Status};
use super::bundle::check_token;
use super::catalog::{Found, Pin, Scope};
use super::{Command, PackageManager, State};

/// How many journals are kept; older ones are deleted when a series starts.
pub const KEPT: usize = 20;

/// A journal file as saved. Fields of newer versions are kept.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
struct Saved {
    /// The run's id (also its file's name).
    run: String,
    /// When it started, in seconds since 1970.
    started: u64,
    /// It ended (every package ran), or the user continued or ignored it.
    finished: bool,
    /// The folder of the install logs.
    logs: PathBuf,
    /// The series updates its packages (else it installs them: also a
    /// journal written before the field existed).
    #[serde(default, skip_serializing_if = "crate::addons::is_default")]
    updating: bool,
    entries: Vec<Entry>,
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// A package of the run and where it is.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
struct Entry {
    name: String,
    catalog: String,
    id: String,
    #[serde(default)]
    version: String,
    #[serde(default, skip_serializing_if = "crate::addons::is_default")]
    admin: bool,
    /// The version its bundle pinned (a resume installs that version).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pin_version: Option<String>,
    /// The scope its bundle pinned, "user" or "machine".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
    /// pending, running, ok, ok_restart, skipped, cancelled, failed,
    /// not_started or manual.
    status: String,
    /// The reason (or address) the status carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    why: Option<String>,
    /// The installer's exit code, once it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<i32>,
    /// The installer's log, once it ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    log: Option<PathBuf>,
}

/// The saved word of `status` and its reason.
fn status_words(status: &Status) -> (&'static str, Option<String>) {
    match status {
        Status::Pending => ("pending", None),
        Status::Running => ("running", None),
        Status::Ok => ("ok", None),
        Status::OkRestart => ("ok_restart", None),
        Status::Skipped(why) => ("skipped", Some(why.clone())),
        Status::Cancelled => ("cancelled", None),
        Status::Failed(why) => ("failed", Some(why.clone())),
        Status::NotStarted(why) => ("not_started", Some(why.clone())),
        Status::Manual(url) => ("manual", Some(url.clone())),
    }
}

impl Entry {
    fn of(item: &Item, code: Option<i32>) -> Self {
        let (status, why) = status_words(&item.status);
        let p = &item.package;
        let (pin_version, scope) = (item.pin.version.clone(), item.pin.scope.map(|s| s.word().to_string()));
        Entry { name: p.name.clone(), catalog: p.catalog.clone(), id: p.id.clone(), version: p.version.clone(), admin: item.admin, pin_version, scope, status: status.into(), why, code, log: item.log.clone() }
    }

    /// A bundle's manual entry: only listed, never run, so never resumed
    /// and not counted among the run's packages.
    fn manual(&self) -> bool {
        self.status == "manual"
    }

    /// The package to run again, with its pin. `None` when the saved pin is
    /// not one a bundle could carry (a version that is not a plain token, an
    /// unknown scope): such an entry is not resumed rather than run unpinned
    /// or with an argument nobody checked.
    fn to_resume(&self) -> Option<Item> {
        if let Some(v) = &self.pin_version {
            check_token("version", v).ok()?;
        }
        let scope = match &self.scope {
            Some(word) => Some(Scope::from_word(word)?),
            None => None,
        };
        let package = Found { name: self.name.clone(), id: self.id.clone(), version: self.version.clone(), catalog: self.catalog.clone(), installed: false };
        Some(Item { pin: Pin { version: self.pin_version.clone(), scope }, ..Item::new(package, self.admin) })
    }

    /// A resume installs it again: it failed, was never started, or was
    /// installing when the run was cut short (a cancelled UAC prompt was the
    /// user's choice and stays cancelled).
    fn missing(&self) -> bool {
        matches!(self.status.as_str(), "pending" | "running" | "failed" | "not_started")
    }
}

/// The journal of a running series.
pub struct Journal {
    path: PathBuf,
    saved: Saved,
}

impl Journal {
    /// Starts the journal of a series of `items` in `folder` (made when
    /// missing), deleting the oldest journals beyond [`KEPT`]. A resume
    /// does the same `kind` of work again.
    pub fn begin(folder: &Path, logs: &Path, items: &[Item], kind: Kind) -> Self {
        let updating = kind == Kind::Update;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        // Names sort in the order the runs started.
        let run = format!("run-{:015}-{n:06}", now.as_millis());
        let saved = Saved { run: run.clone(), started: now.as_secs(), finished: false, logs: logs.to_path_buf(), updating, entries: items.iter().map(|i| Entry::of(i, None)).collect(), rest: Default::default() };
        let journal = Journal { path: folder.join(format!("{run}.json")), saved };
        let _ = std::fs::create_dir_all(folder);
        journal.save();
        prune(folder, KEPT);
        journal
    }

    /// Notes the package at `index` as `item` now is (with the installer's
    /// exit code once it ran) and rewrites the journal.
    pub fn record(&mut self, index: usize, item: &Item, code: Option<i32>) {
        if let Some(entry) = self.saved.entries.get_mut(index) {
            let code = code.or(entry.code);
            *entry = Entry::of(item, code);
        }
        self.save();
    }

    /// Notes every package as `items` now are and rewrites the journal.
    pub fn record_all(&mut self, items: &[Item]) {
        for (entry, item) in self.saved.entries.iter_mut().zip(items) {
            *entry = Entry::of(item, entry.code);
        }
        self.save();
    }

    /// The series ran to its end: nothing to resume. Rewrites the journal.
    pub fn finish(&mut self) {
        self.saved.finished = true;
        self.save();
    }

    /// Rewrites the file atomically: a whole new file replaces the old one.
    fn save(&self) {
        let _ = write_atomically(&self.path, &self.saved);
    }
}

/// Writes `saved` to a temporary file beside `path`, then renames it over.
fn write_atomically(path: &Path, saved: &Saved) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(saved).map_err(std::io::Error::other)?;
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path)
}

/// The journal files in `folder`, oldest first.
fn journals(folder: &Path) -> Vec<PathBuf> {
    let Ok(dir) = std::fs::read_dir(folder) else { return Vec::new() };
    let mut files: Vec<PathBuf> = dir.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
    files.sort();
    files
}

/// Deletes the oldest journals in `folder` beyond the newest `keep`.
fn prune(folder: &Path, keep: usize) {
    let files = journals(folder);
    for old in files.iter().take(files.len().saturating_sub(keep)) {
        let _ = std::fs::remove_file(old);
    }
}

/// The journal saved at `path`, if it can be read.
fn read(path: &Path) -> Option<Saved> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Marks the journal at `path` finished ("Continue" or "Ignore"), so it is
/// not offered again.
pub fn close(path: &Path) {
    if let Some(mut saved) = read(path) {
        saved.finished = true;
        let _ = write_atomically(path, &saved);
    }
}

/// The last series that did not end: what it did and what is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Interrupted {
    /// Its journal's file.
    pub path: PathBuf,
    /// Its packages (a bundle's manual entries left out).
    pub total: usize,
    /// The packages it did not install and did not reach, in order, each
    /// with the version and scope its bundle pinned.
    pub missing: Vec<Item>,
    /// What it did: continuing does the same to the missing ones.
    pub kind: Kind,
}

/// The newest readable journal in `folder` when it did not end. A missing
/// folder, or a truncated or unreadable file, never stops the start-up.
pub fn interrupted(folder: &Path) -> Option<Interrupted> {
    let (path, saved) = journals(folder).into_iter().rev().find_map(|p| read(&p).map(|s| (p, s)))?;
    if saved.finished {
        return None;
    }
    let missing = saved.entries.iter().filter(|e| e.missing()).filter_map(Entry::to_resume).collect();
    let total = saved.entries.iter().filter(|e| !e.manual()).count();
    let kind = if saved.updating { Kind::Update } else { Kind::Install };
    Some(Interrupted { path, total, missing, kind })
}

// ---------------------------------------------- the page's interrupted run

impl PackageManager {
    /// Closes the interrupted run so it is not offered again; with
    /// `resume`, first installs (in a new series) the packages it did not
    /// install: those that failed, were not started or were installing. A
    /// series of updates is continued as updates.
    pub(super) fn close_interrupted(&mut self, resume: bool) {
        if self.context.is_none() || (resume && self.state().busy.is_some()) {
            return;
        }
        let Some(interrupted) = lock_state(&self.state).interrupted.take() else { return };
        close(&interrupted.path);
        if resume {
            // As the same kind of work; each package with the version and
            // scope its bundle pinned.
            self.run_series(interrupted.kind, interrupted.missing);
        }
    }

    /// The offer to continue an interrupted run: "N of M done — Continue /
    /// Ignore".
    pub(super) fn draw_interrupted(&mut self, ui: &mut Ui, st: &State) {
        let Some(i) = &st.interrupted else { return };
        let mut asked = None;
        crate::widgets::note(ui, crate::widgets::Tone::Warm, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("The last run was interrupted: {} of {} done.", i.total - i.missing.len(), i.total)).size(13.0));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Ignore").size(12.5)).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(0.0, 26.0))).clicked() {
                        asked = Some(Command::IgnoreInterrupted);
                    }
                    if crate::widgets::parts::primary_button(ui, "Continue", st.busy.is_none()).clicked() {
                        asked = Some(Command::ContinueInterrupted);
                    }
                });
            });
        });
        if let Some(c) = asked {
            lock_state(&self.commands).push(c);
            ui.ctx().request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("upcm-pm-journal-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const UNFINISHED: &str = r#"{ "run": "run-1", "started": 1700000000, "finished": false, "logs": "C:\\logs", "entries": [
        { "name": "A", "catalog": "winget", "id": "Vendor.A", "version": "1.0", "status": "ok", "code": 0, "log": "C:\\logs\\a.log" },
        { "name": "B", "catalog": "winget", "id": "Vendor.B", "version": "1.0", "status": "failed", "why": "exit code 1603", "code": 1603 },
        { "name": "C", "catalog": "winget", "id": "Vendor.C", "version": "2.0", "status": "running" },
        { "name": "D", "catalog": "msstore", "id": "9ABC", "version": "Unknown", "status": "pending" },
        { "name": "E", "catalog": "winget", "id": "Vendor.E", "version": "1.0", "status": "cancelled" },
        { "name": "F", "catalog": "winget", "id": "Vendor.F", "version": "1.0", "status": "not_started", "why": "the series was stopped" }
    ] }"#;

    fn ids(found: &[Item]) -> Vec<&str> {
        found.iter().map(|f| f.package.id.as_str()).collect()
    }

    #[test]
    fn an_unfinished_journal_offers_what_was_not_installed_and_not_reached() {
        let dir = scratch("unfinished");
        std::fs::write(dir.join("run-0001.json"), UNFINISHED).unwrap();
        let found = interrupted(&dir).expect("an unfinished journal");
        assert_eq!(found.total, 6);
        assert_eq!(ids(&found.missing), ["Vendor.B", "Vendor.C", "9ABC", "Vendor.F"]);
        assert_eq!(found.missing[2].package, Found { name: "D".into(), id: "9ABC".into(), version: "Unknown".into(), catalog: "msstore".into(), installed: false });
        assert_eq!(found.path, dir.join("run-0001.json"));
    }

    #[test]
    fn a_finished_journal_offers_nothing() {
        let dir = scratch("finished");
        std::fs::write(dir.join("run-0001.json"), UNFINISHED.replace(r#""finished": false"#, r#""finished": true"#)).unwrap();
        assert_eq!(interrupted(&dir), None);
    }

    #[test]
    fn a_missing_folder_offers_nothing() {
        assert_eq!(interrupted(&scratch("missing").join("nothing")), None);
    }

    #[test]
    fn a_truncated_or_unreadable_journal_does_not_block_the_start() {
        let dir = scratch("broken");
        std::fs::write(dir.join("run-0001.json"), UNFINISHED).unwrap();
        std::fs::write(dir.join("run-0002.json"), &UNFINISHED[..UNFINISHED.len() / 2]).unwrap();
        std::fs::write(dir.join("run-0003.json"), [0xff, 0xfe, 0x00, 0x13]).unwrap();
        std::fs::write(dir.join("run-0004.json"), r#"{ "entries": "not a list" }"#).unwrap();
        let found = interrupted(&dir).expect("the newest readable journal");
        assert_eq!(found.path, dir.join("run-0001.json"));
        // Only broken journals: nothing to offer, and no panic.
        std::fs::remove_file(dir.join("run-0001.json")).unwrap();
        assert_eq!(interrupted(&dir), None);
    }

    #[test]
    fn starting_a_journal_deletes_the_oldest_beyond_those_kept() {
        let dir = scratch("prune");
        for n in 0..KEPT + 3 {
            std::fs::write(dir.join(format!("run-{n:015}-000000.json")), UNFINISHED).unwrap();
        }
        let journal = Journal::begin(&dir, Path::new("C:\\logs"), &[], Kind::Install);
        let left = journals(&dir);
        assert_eq!(left.len(), KEPT);
        assert_eq!(left.last(), Some(&journal.path), "the new one is the newest");
        assert!(!dir.join("run-000000000000003-000000.json").exists() && dir.join("run-000000000000004-000000.json").exists());
    }

    #[test]
    fn an_update_series_is_offered_again_as_updates() {
        let dir = scratch("updating");
        std::fs::write(dir.join("run-0001.json"), UNFINISHED).unwrap();
        assert_eq!(interrupted(&dir).expect("an unfinished journal").kind, Kind::Install, "a journal without the field installs");
        let journal = Journal::begin(&dir, Path::new("C:\\logs"), &[], Kind::Update);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&journal.path).unwrap()).unwrap();
        assert_eq!(saved["updating"], serde_json::json!(true), "the field older versions read");
        let found = interrupted(&dir).expect("the update series did not end");
        assert_eq!(found.path, journal.path);
        assert_eq!(found.kind, Kind::Update);
    }

    #[test]
    fn a_bundle_series_keeps_its_pins_and_lists_its_manual_entries_without_resuming_them() {
        let dir = scratch("pins");
        let pinned = Item { pin: Pin { version: Some("2.47.0".into()), scope: Some(Scope::Machine) }, ..Item::new(Found { name: "Git".into(), id: "Git.Git".into(), version: "2.47.0".into(), catalog: "winget".into(), installed: false }, true) };
        let plain = Item::new(Found { name: "B".into(), id: "Vendor.B".into(), version: "1.0".into(), catalog: "winget".into(), installed: false }, false);
        let manual = Item { status: Status::Manual("https://example.com/tool".into()), ..Item::new(Found { name: "Tool".into(), ..Found::default() }, false) };
        let journal = Journal::begin(&dir, Path::new("C:\\logs"), &[pinned.clone(), manual, plain.clone()], Kind::Install);
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&journal.path).unwrap()).unwrap();
        assert_eq!((saved["entries"][0]["pin_version"].clone(), saved["entries"][0]["scope"].clone()), (serde_json::json!("2.47.0"), serde_json::json!("machine")));
        assert!(saved["entries"][2].get("pin_version").is_none() && saved["entries"][2].get("scope").is_none(), "no pin, no fields");
        assert_eq!((saved["entries"][1]["status"].clone(), saved["entries"][1]["why"].clone()), (serde_json::json!("manual"), serde_json::json!("https://example.com/tool")));
        let found = interrupted(&dir).expect("the series did not end");
        assert_eq!(found.missing, [pinned, plain], "the manual entry is not resumed; the pin comes back");
        assert_eq!(found.total, 2, "the manual entry is not one of the run's packages");
    }

    #[test]
    fn a_saved_pin_a_bundle_could_not_carry_is_not_resumed() {
        let dir = scratch("badpin");
        let text = UNFINISHED.replace(r#""id": "Vendor.B", "version": "1.0","#, r#""id": "Vendor.B", "version": "1.0", "pin_version": "--override","#).replace(r#""id": "Vendor.C", "version": "2.0","#, r#""id": "Vendor.C", "version": "2.0", "scope": "everyone","#);
        std::fs::write(dir.join("run-0001.json"), text).unwrap();
        assert_eq!(ids(&interrupted(&dir).expect("an unfinished journal").missing), ["9ABC", "Vendor.F"]);
    }

    #[test]
    fn a_newer_finished_journal_hides_an_older_unfinished_one() {
        let dir = scratch("newer");
        std::fs::write(dir.join("run-0001.json"), UNFINISHED).unwrap();
        std::fs::write(dir.join("run-0002.json"), UNFINISHED.replace(r#""finished": false"#, r#""finished": true"#)).unwrap();
        assert_eq!(interrupted(&dir), None);
    }
}
