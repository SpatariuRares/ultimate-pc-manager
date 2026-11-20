//! Bundles: a list of programs in a file, to install them again on another
//! PC. A bundle is JSON (extension `.upmbundle`):
//!
//! ```json
//! {
//!   "schema_version": 1,
//!   "created": "2026-11-18T09:30:00Z",
//!   "entries": [
//!     { "catalog": "winget", "id": "VideoLAN.VLC", "version": "3.0.21", "scope": "machine", "name": "VLC" },
//!     { "manual": true, "name": "Some tool", "url": "https://example.com/download" }
//!   ]
//! }
//! ```
//!
//! - `schema_version` is an integer, 1 for this format; a higher one opens
//!   with only the entries this version knows.
//! - An entry is a package (`catalog` and `id`; `version`, `scope` and
//!   `name` optional) or a manual entry (`manual: true`, `name`, `url`), which
//!   no catalog installs: it is listed with its link.
//! - `id` and `version` hold only `A-Z a-z 0-9 . _ + -`, never start with
//!   `-` and have at most [`MAX_TOKEN_LEN`] characters; `scope` is `user` or
//!   `machine`; `url` is an `http` or `https` link.
//! - A catalog no source serves leaves the entry "catalog not available".
//! - An invalid entry is reported and skipped; the rest of the file opens.
//! - Fields this version does not know are kept, in the file and in each
//!   entry, and written back.
//! - There is no field for installer arguments: only the id, the version
//!   and the scope ever reach the catalog.

use std::path::Path;

use egui::{RichText, Ui};
use serde_json::Value;

use crate::platform::{lock_state, FileType, Job, JSON_FILE};

use super::batch::{Item, Status};
use super::catalog::{self, Found, InstalledPackage, Pin, Scope};
use super::page::{cell_text, BUNDLE_COLS};
use super::{busy_line, winget_file, Command, PackageManager, State};
use crate::theme::Theme;
use crate::widgets::{note, note_text, row_button, table_line, Tone};

/// The format version this app writes.
pub const SCHEMA_VERSION: u64 = 1;

/// The longest id or version a bundle may hold.
pub const MAX_TOKEN_LEN: usize = 128;

/// Bundle files in the file dialogs.
pub const BUNDLE_FILE: FileType = FileType { name: "Package bundle", extension: "upmbundle" };

/// Fields kept as they were found.
pub type Rest = serde_json::Map<String, serde_json::Value>;

/// One entry of a bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A package a catalog installs.
    Package { catalog: String, id: String, pin: Pin, name: Option<String>, rest: Rest },
    /// A program no catalog knows: installed by hand from `url`.
    Manual { name: String, url: String, rest: Rest },
}

/// A bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    pub schema_version: u64,
    /// When it was made (UTC, `YYYY-MM-DDTHH:MM:SSZ`).
    pub created: String,
    pub entries: Vec<Entry>,
    pub rest: Rest,
}

/// A bundle as read from a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub bundle: Bundle,
    /// Why each skipped entry was skipped.
    pub skipped: Vec<String>,
    /// A newer version of the app made it: only the known entries were read.
    pub newer: bool,
}

/// An entry on the page, ticked or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub entry: Entry,
    pub ticked: bool,
}

/// The bundle on the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleView {
    /// Reading the installed packages to make one.
    Listing,
    /// Being made from the installed packages; `note` says what the last
    /// action did (saved where, a refused manual entry).
    Draft { rows: Vec<Row>, note: Option<String> },
    /// Opened from `path`; `notes` say what was skipped.
    Opened { path: std::path::PathBuf, rows: Vec<Row>, notes: Vec<String> },
    /// It could not be made or opened; why.
    Failed(String),
}

/// Reads a bundle from its text. Fails only when the text is not a bundle
/// at all; a bad entry is skipped and said why.
pub fn read(text: &str) -> Result<Opened, String> {
    const NOT_A_BUNDLE: &str = "This file is not a bundle";
    let value: Value = serde_json::from_str(text).map_err(|e| format!("{NOT_A_BUNDLE}: {e}."))?;
    let Value::Object(mut rest) = value else { return Err(format!("{NOT_A_BUNDLE}.")) };
    let schema_version = rest.remove("schema_version").and_then(|v| v.as_u64()).filter(|v| *v >= 1).ok_or_else(|| format!("{NOT_A_BUNDLE}: it has no schema_version."))?;
    let Some(Value::Array(raw)) = rest.remove("entries") else { return Err(format!("{NOT_A_BUNDLE}: it has no list of entries.")) };
    let created = match rest.remove("created") {
        Some(Value::String(s)) => s,
        Some(other) => {
            rest.insert("created".into(), other);
            String::new()
        }
        None => String::new(),
    };
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for (i, value) in raw.into_iter().enumerate() {
        match entry(value) {
            Ok(e) => entries.push(e),
            Err(why) => skipped.push(format!("Entry {} was skipped: {why}.", i + 1)),
        }
    }
    let newer = schema_version > SCHEMA_VERSION;
    Ok(Opened { bundle: Bundle { schema_version, created, entries, rest }, skipped, newer })
}

/// One entry from its JSON, validated.
fn entry(value: Value) -> Result<Entry, String> {
    let Value::Object(mut rest) = value else { return Err("it is not an entry".into()) };
    let manual = match rest.remove("manual") {
        None => false,
        Some(Value::Bool(b)) => b,
        Some(_) => return Err("its manual field is neither true nor false".into()),
    };
    let mut text = |key: &str| -> Result<Option<String>, String> {
        match rest.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s)),
            Some(_) => Err(format!("its {key} is not text")),
        }
    };
    if manual {
        let name = text("name")?.ok_or("a manual entry needs a name")?;
        let url = text("url")?.ok_or("a manual entry needs a link")?;
        let Entry::Manual { name, url, .. } = self::manual(&name, &url)? else { unreachable!("manual() makes manual entries") };
        return Ok(Entry::Manual { name, url, rest });
    }
    let catalog = text("catalog")?.filter(|c| !c.trim().is_empty()).ok_or("it names no catalog")?;
    let id = text("id")?.ok_or("it has no id")?;
    check_token("id", &id)?;
    let version = text("version")?;
    if let Some(v) = &version {
        check_token("version", v)?;
    }
    let scope = match text("scope")? {
        None => None,
        Some(word) => Some(Scope::from_word(&word).ok_or_else(|| format!("its scope \u{201c}{word}\u{201d} is neither user nor machine"))?),
    };
    let name = text("name")?;
    Ok(Entry::Package { catalog, id, pin: Pin { version, scope }, name, rest })
}

/// The bundle's text (pretty JSON), unknown fields included; optional
/// fields left empty are not written.
pub fn write(bundle: &Bundle) -> String {
    let entries: Vec<Value> = bundle
        .entries
        .iter()
        .map(|e| {
            let mut out = Rest::new();
            match e {
                Entry::Package { catalog, id, pin, name, rest } => {
                    out.insert("catalog".into(), catalog.as_str().into());
                    out.insert("id".into(), id.as_str().into());
                    if let Some(v) = &pin.version {
                        out.insert("version".into(), v.as_str().into());
                    }
                    if let Some(s) = pin.scope {
                        out.insert("scope".into(), s.word().into());
                    }
                    if let Some(n) = name {
                        out.insert("name".into(), n.as_str().into());
                    }
                    out.extend(rest.clone());
                }
                Entry::Manual { name, url, rest } => {
                    out.insert("manual".into(), true.into());
                    out.insert("name".into(), name.as_str().into());
                    out.insert("url".into(), url.as_str().into());
                    out.extend(rest.clone());
                }
            }
            Value::Object(out)
        })
        .collect();
    let mut top = Rest::new();
    top.insert("schema_version".into(), bundle.schema_version.into());
    top.insert("created".into(), bundle.created.as_str().into());
    top.insert("entries".into(), Value::Array(entries));
    top.extend(bundle.rest.clone());
    serde_json::to_string_pretty(&Value::Object(top)).unwrap_or_default()
}

/// Whether `value` may be an id or a version (`what` names it in the
/// error): only `A-Z a-z 0-9 . _ + -`, not starting with `-`, at most
/// [`MAX_TOKEN_LEN`] characters.
pub fn check_token(what: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err(format!("its {what} is empty"));
    }
    if value.chars().count() > MAX_TOKEN_LEN {
        return Err(format!("its {what} is too long (more than {MAX_TOKEN_LEN} characters)"));
    }
    if value.starts_with('-') || !value.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')) {
        return Err(format!("its {what} \u{201c}{value}\u{201d} is not allowed"));
    }
    Ok(())
}

/// A manual entry: a name and a web link (`http` or `https`), both trimmed.
pub fn manual(name: &str, url: &str) -> Result<Entry, String> {
    let (name, url) = (name.trim(), url.trim());
    if name.is_empty() {
        return Err("a manual entry needs a name".into());
    }
    let web = ["https://", "http://"].iter().any(|p| url.len() > p.len() && url.get(..p.len()).is_some_and(|s| s.eq_ignore_ascii_case(p)));
    if !web || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("the link \u{201c}{url}\u{201d} is not a web address (http or https)"));
    }
    Ok(Entry::Manual { name: name.into(), url: url.into(), rest: Rest::new() })
}

/// The entries of a bundle made from the installed packages (those whose id
/// a bundle may hold), without version or scope: the other PC gets the
/// latest version.
pub fn from_installed(installed: &[InstalledPackage]) -> Vec<Entry> {
    installed
        .iter()
        .filter(|p| check_token("id", &p.id).is_ok() && !p.catalog.is_empty())
        .map(|p| Entry::Package { catalog: p.catalog.clone(), id: p.id.clone(), pin: Pin::default(), name: None, rest: Rest::new() })
        .collect()
}

/// `secs` since 1970 as UTC `YYYY-MM-DDTHH:MM:SSZ`.
pub fn timestamp(secs: u64) -> String {
    let (days, rest) = ((secs / 86_400) as i64, secs % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, (rest % 3600) / 60, rest % 60)
}

/// Now, as [`timestamp`] writes it.
pub fn now() -> String {
    timestamp(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0))
}

impl Entry {
    /// Why it cannot be installed from its catalog, if it cannot: no
    /// source serves its catalog, or the catalog is among `inactive` (removed
    /// or disabled on this PC). A manual entry says nothing: it is
    /// installed by hand.
    pub fn problem(&self, inactive: &[String]) -> Option<String> {
        match self {
            Entry::Package { catalog, .. } if catalog::source_of(catalog).is_none() || inactive.iter().any(|c| c.eq_ignore_ascii_case(catalog)) => {
                Some("catalog not available".into())
            }
            _ => None,
        }
    }

    /// The name to show: its own, else its id.
    pub fn name(&self) -> &str {
        match self {
            Entry::Package { name: Some(n), .. } if !n.trim().is_empty() => n,
            Entry::Package { id, .. } => id,
            Entry::Manual { name, .. } => name,
        }
    }
}

// ------------------------------------------------------- the page's bundle

impl PackageManager {
    /// Lists the installed packages in the background into a new bundle,
    /// every entry ticked.
    pub(super) fn create_bundle(&mut self) {
        let Some(context) = self.context.clone() else { return };
        if matches!(self.state().bundle, Some(BundleView::Listing)) {
            return;
        }
        lock_state(&self.state).bundle = Some(BundleView::Listing);
        let (job, stop, accept) = (Job::new(&self.state, &self.generation), self.stop.clone(), self.prefs.accept_agreements);
        std::thread::spawn(move || {
            let mut installed = Vec::new();
            for source in catalog::SOURCES {
                match source.list_installed(&context, accept, &stop) {
                    None => return,
                    Some(Ok(found)) => installed.extend(found),
                    Some(Err(e)) => return job.publish(|s| s.bundle = Some(BundleView::Failed(e))),
                }
            }
            let rows = from_installed(&installed).into_iter().map(|entry| Row { entry, ticked: true }).collect();
            job.publish(|s| s.bundle = Some(BundleView::Draft { rows, note: None }));
        });
    }

    /// Adds a manual entry (ticked) to the bundle being made, or says why
    /// it cannot.
    pub(super) fn add_manual_entry(&mut self, name: &str, url: &str) {
        if let Some(BundleView::Draft { rows, note }) = &mut lock_state(&self.state).bundle {
            match manual(name, url) {
                Ok(entry) => {
                    rows.push(Row { entry, ticked: true });
                    *note = None;
                }
                Err(why) => *note = Some(format!("The entry was not added: {why}.")),
            }
        }
    }

    /// Ticks or unticks an entry (one its catalog cannot install stays
    /// unticked).
    pub(super) fn tick_bundle_entry(&mut self, index: usize, on: bool) {
        let mut st = lock_state(&self.state);
        let State { bundle, inactive_catalogs, .. } = &mut *st;
        if let Some(BundleView::Draft { rows, .. } | BundleView::Opened { rows, .. }) = bundle {
            if let Some(row) = rows.get_mut(index) {
                row.ticked = on && row.entry.problem(inactive_catalogs).is_none();
            }
        }
    }

    /// Fixes the version of a package entry (empty: the latest), or says why
    /// it cannot.
    pub(super) fn set_bundle_version(&mut self, index: usize, version: &str) {
        let version = version.trim();
        let checked = if version.is_empty() { Ok(None) } else { check_token("version", version).map(|()| Some(version.to_string())) };
        let mut st = lock_state(&self.state);
        let Some(view) = &mut st.bundle else { return };
        match checked {
            Ok(v) => {
                if let Some(Entry::Package { pin, .. }) = rows_of(view).and_then(|rows| rows.get_mut(index)).map(|r| &mut r.entry) {
                    pin.version = v;
                }
            }
            Err(why) => say(view, format!("The version was not changed: {why}.")),
        }
    }

    /// Fixes the scope of a package entry (None: winget's default).
    pub(super) fn set_bundle_scope(&mut self, index: usize, scope: Option<Scope>) {
        if let Some(Entry::Package { pin, .. }) = lock_state(&self.state).bundle.as_mut().and_then(rows_of).and_then(|rows| rows.get_mut(index)).map(|r| &mut r.entry) {
            pin.scope = scope;
        }
    }

    /// Saves the ticked entries of the bundle being made to `path`.
    pub(super) fn save_bundle(&mut self, path: &Path) {
        let mut st = lock_state(&self.state);
        let Some(BundleView::Draft { rows, note }) = &mut st.bundle else { return };
        let entries = rows.iter().filter(|r| r.ticked).map(|r| r.entry.clone()).collect();
        let bundle = Bundle { schema_version: SCHEMA_VERSION, created: now(), entries, rest: Rest::new() };
        *note = Some(match std::fs::write(path, write(&bundle)) {
            Ok(()) => format!("Saved to {}.", path.display()),
            Err(e) => format!("Could not save {}: {e}", path.display()),
        });
    }

    /// Opens the bundle in `path`, or a `winget export` file as one: every
    /// entry its catalog can install is ticked; skipped entries are said.
    pub(super) fn open_bundle(&mut self, path: &Path) {
        let opened = std::fs::read(path).map_err(|e| format!("{} could not be read: {e}", path.display())).and_then(|b| {
            let text = String::from_utf8_lossy(&b);
            // A file saved by an editor may start with a byte order mark.
            let text = text.trim_start_matches('\u{feff}');
            if winget_file::is_winget_export(text) {
                winget_file::import(text)
            } else {
                read(text)
            }
        });
        let view = match opened {
            Err(why) => BundleView::Failed(why),
            Ok(opened) => {
                let mut notes = Vec::new();
                if opened.newer {
                    notes.push("A newer version of the app made this bundle: only the entries this version knows are shown.".to_string());
                }
                notes.extend(opened.skipped);
                let inactive = self.state().inactive_catalogs;
                let rows = opened.bundle.entries.into_iter().map(|entry| Row { ticked: entry.problem(&inactive).is_none(), entry }).collect();
                BundleView::Opened { path: path.to_path_buf(), rows, notes }
            }
        };
        lock_state(&self.state).bundle = Some(view);
    }

    /// Installs the ticked entries of the opened bundle in series; manual
    /// entries are listed in the summary with their link.
    pub(super) fn install_bundle(&mut self) {
        let Some(BundleView::Opened { rows, .. }) = self.state().bundle else { return };
        let needs_admin = self.needs_admin;
        let items: Vec<Item> = rows
            .into_iter()
            .filter(|r| r.ticked)
            .map(|r| {
                let name = r.entry.name().to_string();
                match r.entry {
                    Entry::Package { catalog, id, pin, .. } => {
                        // A machine-wide install needs administrator rights.
                        let admin = pin.scope == Some(Scope::Machine) || needs_admin(&catalog, &id) == Some(true);
                        let version = pin.version.clone().unwrap_or_default();
                        Item { pin, ..Item::new(Found { name, id, version, catalog, installed: false }, admin) }
                    }
                    Entry::Manual { url, .. } => Item { status: Status::Manual(url), ..Item::new(Found { name, ..Found::default() }, false) },
                }
            })
            .collect();
        self.run_series(super::batch::Kind::Install, items);
    }

    /// The Bundles scope: "Create a bundle" and "Open a bundle…" until one
    /// shows, then the bundle being made (ticks, version, scope, manual
    /// entries, Save) or the one opened (ticks, Install selected).
    pub(super) fn draw_bundle(&mut self, ui: &mut Ui, st: &State) {
        let pal = crate::theme::of_ui(ui);
        let mut asked = Vec::new();
        let start = |me: &mut Self, ui: &mut Ui, asked: &mut Vec<Command>| {
            ui.horizontal_wrapped(|ui| {
                let size = egui::vec2(0.0, 30.0);
                if ui.add_enabled(st.bundle != Some(BundleView::Listing), egui::Button::new(RichText::new("Create a bundle from the installed apps").size(13.0)).min_size(size)).clicked() {
                    asked.push(Command::CreateBundle);
                }
                if ui.add(egui::Button::new(RichText::new("Open a bundle\u{2026}").size(13.0)).min_size(size)).clicked() {
                    let folder = me.bundle_folder();
                    if let Some(path) = me.context.as_ref().and_then(|c| c.pick_open_file(&[BUNDLE_FILE, JSON_FILE], Some(&folder))) {
                        asked.push(Command::OpenBundle(path.into()));
                    }
                }
            });
        };
        match &st.bundle {
            None => {
                start(self, ui, &mut asked);
                note_text(ui, Tone::Neutral, "A bundle is a list of packages, with version and scope, saved as a file you can open on another PC to install them all.");
            }
            Some(BundleView::Listing) => {
                start(self, ui, &mut asked);
                busy_line(ui, "Reading the installed packages\u{2026}");
            }
            Some(BundleView::Failed(why)) => {
                start(self, ui, &mut asked);
                note_text(ui, Tone::Bad, why);
            }
            Some(BundleView::Draft { rows, note: said }) => {
                let ticked = rows.iter().filter(|r| r.ticked).count();
                self.bundle_title(ui, &pal, "New bundle from this PC", rows.len(), ticked, &mut asked, |me, ui, asked| {
                    if crate::widgets::parts::primary_button(ui, "Save the bundle\u{2026}", ticked > 0).clicked() {
                        let default = me.bundle_folder().join(format!("bundle.{}", BUNDLE_FILE.extension));
                        if let Some(path) = me.context.as_ref().and_then(|c| c.pick_save_as(&BUNDLE_FILE, &default.to_string_lossy())) {
                            asked.push(Command::SaveBundle(path.into()));
                        }
                    }
                });
                if let Some(said) = said {
                    note_text(ui, Tone::Neutral, said);
                }
                draw_rows(ui, &pal, rows, &st.inactive_catalogs, &mut asked);
                egui::Frame::new().fill(pal.panel).stroke(egui::Stroke::new(1.0, pal.line)).corner_radius(egui::CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new("Add a program winget does not have; it is listed to install by hand, with its link.").size(12.0).color(pal.label));
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Name").size(12.0).color(pal.label));
                        ui.add(egui::TextEdit::singleline(&mut self.manual_entry.0).hint_text("Name").desired_width(200.0));
                        ui.label(RichText::new("Link").size(12.0).color(pal.label));
                        ui.add(egui::TextEdit::singleline(&mut self.manual_entry.1).hint_text("https://\u{2026}").desired_width(260.0));
                        if row_button(ui, "Add", true).clicked() {
                            let (name, url) = std::mem::take(&mut self.manual_entry);
                            asked.push(Command::AddManualEntry { name, url });
                        }
                    });
                });
            }
            Some(BundleView::Opened { path, rows, notes }) => {
                let ticked = rows.iter().filter(|r| r.ticked).count();
                let idle = st.busy.is_none();
                self.bundle_title(ui, &pal, "Opened bundle", rows.len(), ticked, &mut asked, |_, ui, asked| {
                    if crate::widgets::parts::primary_button(ui, &format!("Install selected ({ticked})"), idle && ticked > 0).clicked() {
                        asked.push(Command::InstallBundle);
                    }
                });
                note(ui, Tone::Neutral, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("Opened").size(13.0));
                        ui.label(RichText::new(path.display().to_string()).monospace().size(12.0));
                    });
                    for said in notes {
                        ui.add(egui::Label::new(RichText::new(said).size(12.5).color(pal.label)).wrap());
                    }
                });
                draw_rows(ui, &pal, rows, &st.inactive_catalogs, &mut asked);
            }
        }
        self.ask(ui, asked);
    }

    /// The title line of a bundle: its words and count, then its main
    /// action (`main`), "Export for winget…" and "Close".
    #[allow(clippy::too_many_arguments)]
    fn bundle_title(&mut self, ui: &mut Ui, pal: &Theme, title: &str, entries: usize, ticked: usize, asked: &mut Vec<Command>, main: impl FnOnce(&mut Self, &mut Ui, &mut Vec<Command>)) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(title).size(14.0).strong().color(pal.text));
            ui.label(RichText::new(crate::addons::plural(entries, "entry")).size(13.0).color(pal.label));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("Close").size(12.5)).fill(egui::Color32::TRANSPARENT).min_size(egui::vec2(0.0, 26.0))).clicked() {
                    asked.push(Command::CloseBundle);
                }
                self.export_button(ui, ticked, asked);
                main(self, ui, asked);
            });
        });
    }

    /// "Export for winget…": the ticked entries as a `winget export` file,
    /// which `winget import` installs without the app.
    fn export_button(&self, ui: &mut Ui, ticked: usize, asked: &mut Vec<Command>) {
        if ui.add_enabled(ticked > 0, egui::Button::new(RichText::new("Export for winget\u{2026}").size(12.5)).min_size(egui::vec2(0.0, 26.0))).clicked() {
            let default = self.bundle_folder().join(format!("winget-packages.{}", JSON_FILE.extension));
            if let Some(path) = self.context.as_ref().and_then(|c| c.pick_save_as(&JSON_FILE, &default.to_string_lossy())) {
                asked.push(Command::ExportWinget(path.into()));
            }
        }
    }
}

// The rows of a bundle being made or opened.
pub(super) fn rows_of(view: &mut BundleView) -> Option<&mut Vec<Row>> {
    match view {
        BundleView::Draft { rows, .. } | BundleView::Opened { rows, .. } => Some(rows),
        _ => None,
    }
}

/// Unticks the entries of the bundle shown that cannot be installed (the
/// catalogs active on this PC became known after it was opened).
pub(super) fn untick_problems(st: &mut State) {
    let State { bundle, inactive_catalogs, .. } = st;
    if let Some(rows) = bundle.as_mut().and_then(rows_of) {
        for row in rows.iter_mut().filter(|r| r.entry.problem(inactive_catalogs).is_some()) {
            row.ticked = false;
        }
    }
}

/// Shows `text` under the bundle being made or opened.
pub(super) fn say(view: &mut BundleView, text: String) {
    match view {
        BundleView::Draft { note, .. } => *note = Some(text),
        BundleView::Opened { notes, .. } => notes.push(text),
        _ => {}
    }
}

//// The entries of a bundle, a row each: the tick, the name (and why its
/// catalog cannot install it), then the catalog, id, version and scope, or
/// the link of a manual entry.
fn draw_rows(ui: &mut Ui, pal: &Theme, rows: &[Row], inactive: &[String], asked: &mut Vec<Command>) {
    table_line(ui, &BUNDLE_COLS, 26.0, true, false, |i, ui| {
        if let Some(heading) = ["", "Name", "Catalog", "Id or link", "Version", "Scope"].get(i) {
            ui.label(RichText::new(*heading).size(12.0).color(pal.label));
        }
    });
    for (i, row) in rows.iter().enumerate() {
        let problem = row.entry.problem(inactive);
        let height = if problem.is_some() { 42.0 } else { 34.0 };
        table_line(ui, &BUNDLE_COLS, height, false, false, |col, ui| match (col, &row.entry) {
            (0, _) => {
                let mut tick = row.ticked;
                if ui.add_enabled(problem.is_none(), egui::Checkbox::without_text(&mut tick)).changed() {
                    asked.push(Command::TickBundleEntry(i, tick));
                }
            }
            (1, entry) => {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    cell_text(ui, entry.name(), pal.text, false);
                    if let Some(p) = &problem {
                        cell_text(ui, p, pal.amber, false);
                    }
                });
            }
            (2, Entry::Package { catalog, .. }) => cell_text(ui, catalog, pal.label, false),
            (2, Entry::Manual { .. }) => cell_text(ui, "manual", pal.label, false),
            (3, Entry::Package { id, .. }) => cell_text(ui, id, pal.label, true),
            (3, Entry::Manual { url, .. }) => cell_text(ui, url, pal.link, false),
            (4, Entry::Package { pin, .. }) => {
                // Empty: the latest version; a refused one is said in a note.
                let mut version = pin.version.clone().unwrap_or_default();
                let width = ui.available_width();
                if ui.add(egui::TextEdit::singleline(&mut version).hint_text("latest").desired_width(width)).changed() {
                    asked.push(Command::SetBundleVersion(i, version));
                }
            }
            (4, Entry::Manual { .. }) => cell_text(ui, "install by hand", pal.label, false),
            (5, Entry::Package { pin, .. }) => {
                let shown = pin.scope.map(Scope::word).unwrap_or("default scope");
                let width = ui.available_width();
                egui::ComboBox::from_id_salt(("package-manager-bundle-scope", i)).width(width).selected_text(shown).show_ui(ui, |ui| {
                    for (scope, text) in [(None, "default scope"), (Some(Scope::User), "user"), (Some(Scope::Machine), "machine")] {
                        if ui.selectable_label(pin.scope == scope, text).clicked() && pin.scope != scope {
                            asked.push(Command::SetBundleScope(i, scope));
                        }
                    }
                });
            }
            _ => {}
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn opened(v: Value) -> Opened {
        read(&v.to_string()).expect("a bundle")
    }

    fn package(catalog: &str, id: &str) -> Entry {
        Entry::Package { catalog: catalog.into(), id: id.into(), pin: Pin::default(), name: None, rest: Rest::new() }
    }

    #[test]
    fn a_bundle_reads_packages_with_version_scope_and_name_and_manual_entries() {
        let o = opened(json!({
            "schema_version": 1,
            "created": "2026-11-18T09:30:00Z",
            "entries": [
                { "catalog": "winget", "id": "VideoLAN.VLC", "version": "3.0.21", "scope": "machine", "name": "VLC" },
                { "catalog": "msstore", "id": "9NBLGGH4VVNH" },
                { "manual": true, "name": "Some tool", "url": "https://example.com/download" }
            ]
        }));
        assert_eq!(o.skipped, Vec::<String>::new());
        assert!(!o.newer);
        assert_eq!(o.bundle.created, "2026-11-18T09:30:00Z");
        assert_eq!(
            o.bundle.entries,
            [
                Entry::Package {
                    catalog: "winget".into(),
                    id: "VideoLAN.VLC".into(),
                    pin: Pin { version: Some("3.0.21".into()), scope: Some(Scope::Machine) },
                    name: Some("VLC".into()),
                    rest: Rest::new()
                },
                package("msstore", "9NBLGGH4VVNH"),
                Entry::Manual { name: "Some tool".into(), url: "https://example.com/download".into(), rest: Rest::new() },
            ]
        );
    }

    #[test]
    fn written_then_read_a_bundle_is_the_same() {
        let bundle = Bundle {
            schema_version: SCHEMA_VERSION,
            created: "2026-11-18T09:30:00Z".into(),
            entries: vec![
                Entry::Package { catalog: "winget".into(), id: "Git.Git".into(), pin: Pin { version: Some("2.47.0".into()), scope: Some(Scope::User) }, name: Some("Git".into()), rest: Rest::new() },
                package("msstore", "9NBLGGH4VVNH"),
                Entry::Manual { name: "Tool".into(), url: "https://example.com".into(), rest: Rest::new() },
            ],
            rest: Rest::new(),
        };
        let text = write(&bundle);
        assert_eq!(read(&text).unwrap(), Opened { bundle, skipped: vec![], newer: false });
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["schema_version"], json!(1));
        assert_eq!(v["entries"][1], json!({ "catalog": "msstore", "id": "9NBLGGH4VVNH" }), "absent optional fields are not written");
        assert_eq!(v["entries"][2], json!({ "manual": true, "name": "Tool", "url": "https://example.com" }));
    }

    #[test]
    fn unknown_fields_are_kept_in_the_file_and_in_each_entry() {
        let o = opened(json!({
            "schema_version": 1,
            "created": "2026-11-18T09:30:00Z",
            "comment": "my PC",
            "entries": [
                { "catalog": "winget", "id": "Git.Git", "pinned": true },
                { "manual": true, "name": "Tool", "url": "https://example.com", "icon": "tool.png" }
            ]
        }));
        let v: Value = serde_json::from_str(&write(&o.bundle)).unwrap();
        assert_eq!(v["comment"], json!("my PC"));
        assert_eq!(v["entries"][0]["pinned"], json!(true));
        assert_eq!(v["entries"][1]["icon"], json!("tool.png"));
    }

    #[test]
    fn an_invalid_entry_is_reported_and_skipped_and_the_rest_opens() {
        let o = opened(json!({
            "schema_version": 1,
            "created": "2026-11-18T09:30:00Z",
            "entries": [
                { "catalog": "winget", "id": "--override" },
                { "catalog": "winget", "id": "Git.Git" },
                { "catalog": "winget", "id": "Vendor App" },
                { "catalog": "winget", "id": "A".repeat(MAX_TOKEN_LEN + 1) },
                { "catalog": "winget", "id": "Git.Git", "version": "1.0 --override /S" },
                { "catalog": "winget", "id": "Git.Git", "scope": "everyone" },
                { "manual": true, "name": "Tool", "url": "file:///C:/Windows/System32/cmd.exe" },
                { "manual": true, "url": "https://example.com" },
                "not an entry",
                { "name": "nothing else" }
            ]
        }));
        assert_eq!(o.bundle.entries, [package("winget", "Git.Git")]);
        assert_eq!(o.skipped.len(), 9, "{:?}", o.skipped);
        assert!(o.skipped[0].starts_with("Entry 1 was skipped") && o.skipped[0].contains("--override"), "{}", o.skipped[0]);
        assert!(o.skipped[1].starts_with("Entry 3 was skipped"), "{}", o.skipped[1]);
        assert!(o.skipped[2].contains("too long"), "{}", o.skipped[2]);
    }

    #[test]
    fn an_id_holds_only_the_allowed_characters_and_a_limited_length() {
        for id in ["VideoLAN.VLC", "9NBLGGH4VVNH", "Notepad++.Notepad++", "Some_Vendor.App-2", &"A".repeat(MAX_TOKEN_LEN)] {
            assert_eq!(check_token("id", id), Ok(()), "{id}");
        }
        for id in ["", "--override", "-x", "Vendor App", "a;b", "a&b", "a\"b", "C:\\x", "a/b", "é", &"A".repeat(MAX_TOKEN_LEN + 1)] {
            assert!(check_token("id", id).is_err(), "{id:?} is refused");
        }
    }

    #[test]
    fn a_newer_bundle_opens_with_only_the_entries_this_version_knows() {
        let o = opened(json!({
            "schema_version": 3,
            "created": "2030-01-01T00:00:00Z",
            "entries": [
                { "catalog": "winget", "id": "Git.Git" },
                { "kind": "script", "run": "format c:" }
            ]
        }));
        assert!(o.newer);
        assert_eq!(o.bundle.entries, [package("winget", "Git.Git")]);
        assert_eq!(o.skipped.len(), 1);
    }

    #[test]
    fn a_file_that_is_not_a_bundle_is_refused() {
        for text in ["", "[]", "not json", r#"{"entries": []}"#, r#"{"schema_version": 0, "entries": []}"#, r#"{"schema_version": 1, "entries": {}}"#] {
            assert!(read(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn bundles_are_upmbundle_files_like_the_apps_other_files() {
        // As Flight Recorder's traces are `.upmtrace`.
        assert_eq!(BUNDLE_FILE.extension, "upmbundle");
        assert!(crate::platform::file_filter(&[BUNDLE_FILE]).contains("*.upmbundle"));
    }

    #[test]
    fn a_catalog_no_source_serves_leaves_the_entry_not_available() {
        let o = opened(json!({ "schema_version": 1, "created": "", "entries": [ { "catalog": "scoop", "id": "git" }, { "catalog": "winget", "id": "Git.Git" } ] }));
        assert_eq!(o.bundle.entries.len(), 2, "the entry is kept");
        assert_eq!(o.bundle.entries[0].problem(&[]), Some("catalog not available".to_string()));
        assert_eq!(o.bundle.entries[1].problem(&[]), None);
        assert_eq!(o.bundle.entries[1].problem(&["winget".to_string()]), Some("catalog not available".to_string()), "a catalog removed on this PC");
    }

    #[test]
    fn a_bundle_made_from_the_installed_packages_lists_each_of_them() {
        let installed = [
            InstalledPackage { catalog: "winget".into(), id: "Git.Git".into(), version: "2.47.0".into() },
            InstalledPackage { catalog: "msstore".into(), id: "9NBLGGH4VVNH".into(), version: "1.0".into() },
            InstalledPackage { catalog: "winget".into(), id: "Bad Id".into(), version: "1".into() },
        ];
        assert_eq!(from_installed(&installed), [package("winget", "Git.Git"), package("msstore", "9NBLGGH4VVNH")]);
    }

    #[test]
    fn a_manual_entry_needs_a_name_and_a_web_link() {
        assert_eq!(manual(" Tool ", " https://example.com/x "), Ok(Entry::Manual { name: "Tool".into(), url: "https://example.com/x".into(), rest: Rest::new() }));
        assert!(manual("", "https://example.com").is_err());
        assert!(manual("Tool", "example.com").is_err());
        assert!(manual("Tool", "javascript:alert(1)").is_err());
        assert!(manual("Tool", "https://exa mple.com").is_err());
    }

    #[test]
    fn the_creation_time_is_utc_and_sortable() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(timestamp(1_795_000_000), "2026-11-18T11:06:40Z");
    }
}
