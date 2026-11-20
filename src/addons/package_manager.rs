//! The Package Manager add-on: installs, updates and removes programs from
//! winget's catalogs. Starting it launches nothing; opening its page checks
//! that winget is ready (missing, not registered, too old or ready), offers
//! the matching repair, and then runs the first catalog update.
//!
//! A search runs when Enter is pressed (never when the page opens) in every
//! registered catalog; results already installed are marked. Installing a
//! result enters the app's install queue, runs the catalog with fixed
//! arguments and a log, then checks that the package shows as installed.
//!
//! A bundle (see [`bundle`]) is made from the installed packages plus
//! manual entries (name and link) and saved; opened, its ticked entries
//! install in series, the manual ones listed in the summary with their link.
//! Each package entry shows its version and scope, editable before starting
//! (empty: the latest version, winget's default scope). A `winget export`
//! file opens as a bundle, and a bundle exports to one without its manual
//! entries ([`winget_file`]).
//!
//! Turning it off ends what runs and drops every late result, except an
//! install, which finishes while the page follows it.
//!
//! Each series keeps a journal on disk ([`journal`]): a run cut short is
//! offered at the next start ("Continue" installs only what is missing),
//! and closing the app during a series asks first.
//!
//! Its settings, saved under its id and kept while it is off: automatic
//! acceptance of the agreements (on by default, with a note on the page
//! until the first install), the bundle folder and the results' sort.

/// A series of installs: each package's status, the order, the summary.
pub mod batch;
/// Bundles: the format of a list of programs in a file.
pub mod bundle;
/// Catalogs: the interface every catalog answers, and winget's.
pub mod catalog;
/// The installed packages: updates and uninstalls (#118).
pub mod installed;
/// The journal of a series on disk, and resuming an interrupted one.
pub mod journal;
/// The page's layout in Design A: band, scopes, list and side panel.
pub mod page;
/// Whether winget can be used, and how to repair it.
pub mod readiness;
/// `winget export` files: imported as a bundle and written from one.
pub mod winget_file;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use egui::{RichText, Ui};

use crate::addons::{is_default, read_settings, write_settings, Addon, Consumption, Requirements};
use crate::platform::install_queue::{Request, Work};
use crate::platform::series::{Run, Series};
use crate::platform::winget::{classify, describe, last_lines, ExitClass};
use crate::platform::{lock_state, AddonContext, Generation, Job, PageContext, Ran};

use batch::{Item, Kind, Status};
use bundle::BundleView;
use catalog::{Found, PackageSource, Pin, Target};
use readiness::{Answer, Readiness, Repair};

/// The add-on's id, and its page's.
pub const ID: &str = "package-manager";

/// The note under its requirements in Settings › Add-ons.
pub const NEEDS: &str = "Installing needs the internet and winget (App Installer). Some installers ask for administrator \
rights through their own UAC prompt; this app does not start as administrator.";

/// The status bar's link, from the start of the app, to a run that was
/// interrupted; it opens the page, which offers to continue it.
pub const INTERRUPTED_NOTICE: &str = "Package Manager: the last run was interrupted";

/// The note above the search until the first install, while agreements
/// are accepted automatically.
pub const AGREEMENTS_NOTE: &str = "License agreements are accepted automatically: installing accepts the package's and the \
catalog's agreements without asking. You can turn this off in Settings.";

/// Where the winget check is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Check {
    /// Not checked yet: the page has not been opened.
    #[default]
    NotChecked,
    /// Asking winget (and PowerShell) now.
    Checking,
    /// Registering App Installer again, then checking.
    Repairing,
    /// The answer.
    Known(Readiness),
}

/// Where the first catalog update is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Catalogs {
    /// Not started (it starts once winget is ready).
    #[default]
    NotUpdated,
    /// winget is updating its catalogs.
    Updating,
    /// Done.
    Updated,
    /// winget could not update them; why.
    Failed(String),
}

/// What the page shows, filled by the background work.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct State {
    pub check: Check,
    pub catalogs: Catalogs,
    /// The catalogs a source serves that are removed or disabled on this
    /// PC (known once the catalogs were updated).
    pub inactive_catalogs: Vec<String>,
    /// Why the last repair did not work, if it did not.
    pub repair_error: Option<String>,
    pub search: Search,
    /// The last install (running or done).
    pub install: Option<Install>,
    /// Held while an install job runs (or waits for the install queue).
    pub busy: Option<String>,
    /// The last series of installs (running or done).
    pub batch: Option<Batch>,
    /// The bundle being made or opened.
    pub bundle: Option<BundleView>,
    /// The installed packages, their updates and the last uninstall.
    pub installed: installed::Installed,
    /// A series of an earlier run that did not end, offered to continue.
    pub interrupted: Option<journal::Interrupted>,
}

impl State {
    /// Background work is running.
    fn busy(&self) -> bool {
        matches!(self.check, Check::Checking | Check::Repairing) || self.catalogs == Catalogs::Updating || matches!(self.search, Search::Searching(_)) || self.busy.is_some() || self.installed.listing == installed::Listing::Reading
    }
}

/// Where the search is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Search {
    /// Nothing searched yet.
    #[default]
    NotSearched,
    /// Searching for this text.
    Searching(String),
    /// What the search for `query` found; `note` says what is missing
    /// (which packages are installed could not be read).
    Found { query: String, packages: Vec<Found>, note: Option<String> },
    /// The search failed; why.
    Failed(String),
}

/// What the page says about the search, besides its results.
pub fn search_text(search: &Search) -> Option<String> {
    match search {
        Search::NotSearched => None,
        Search::Searching(q) => Some(format!("Searching the catalogs for \u{201c}{q}\u{201d}\u{2026}")),
        Search::Found { query, packages, .. } if packages.is_empty() => Some(format!("No package matches \u{201c}{query}\u{201d}.")),
        Search::Found { note, .. } => note.clone(),
        Search::Failed(why) => Some(why.clone()),
    }
}

/// An install, running or done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Install {
    pub package: Found,
    /// winget's output so far, line by line.
    pub lines: Vec<String>,
    /// The installer's log.
    pub log: PathBuf,
    /// How it ended (`None` while it runs).
    pub outcome: Option<Outcome>,
}

impl Install {
    /// The last lines of its output worth showing (no progress bars).
    pub fn last_lines(&self) -> String {
        last_lines(&self.lines.join("\n"), 5)
    }
}

/// How an install ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// winget could not be started; why.
    NotStarted(String),
    /// winget ran and exited with `code`; `found` is whether the package
    /// shows as installed afterwards (asked only when the code says it was
    /// installed; `None` when not asked or winget could not tell).
    Exited { code: i32, found: Option<bool> },
}

impl Outcome {
    /// The outcome in words, with the raw code.
    pub fn text(&self, name: &str) -> String {
        let (code, found) = match self {
            Outcome::NotStarted(why) => return format!("{name} was not started: {}", why.trim()),
            Outcome::Exited { code, found } => (*code, *found),
        };
        let words = describe(code);
        match classify(code) {
            ExitClass::Installed | ExitClass::InstalledRestart if found == Some(false) => {
                format!("{name} was installed according to winget ({}), but it is not found among the installed packages.", code_text(code))
            }
            ExitClass::Installed => format!("{name} was installed ({}).", code_text(code)),
            ExitClass::InstalledRestart => format!("{name} was installed; {words}."),
            ExitClass::Skipped(_) => format!("{name} was skipped: {words}."),
            ExitClass::Cancelled => format!("{name} was cancelled: {words}."),
            ExitClass::Failed => format!("{name} started installing but failed: {words}."),
        }
    }

    /// The status it gives a package of a series.
    pub fn status(&self) -> Status {
        let (code, found) = match self {
            Outcome::NotStarted(why) => return Status::NotStarted(why.trim().trim_end_matches('.').to_string()),
            Outcome::Exited { code, found } => (*code, *found),
        };
        match classify(code) {
            ExitClass::Installed | ExitClass::InstalledRestart if found == Some(false) => {
                Status::Failed(format!("installed according to winget ({}), but not found among the installed packages", code_text(code)))
            }
            ExitClass::Installed => Status::Ok,
            ExitClass::InstalledRestart => Status::OkRestart,
            ExitClass::Skipped(_) => Status::Skipped(describe(code)),
            ExitClass::Cancelled => Status::Cancelled,
            ExitClass::Failed => Status::Failed(describe(code)),
        }
    }
}

/// An exit code as a raw code ("exit code 0"), or in words with the raw
/// code when it has some.
fn code_text(code: i32) -> String {
    if code == 0 {
        "exit code 0".into()
    } else {
        describe(code)
    }
}

/// What the user asks of the page; applied at its next frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Enter in the search box.
    Search(String),
    /// "Install" on a result.
    Install(Found),
    /// "Open log" under the last install.
    OpenLog,
    /// A heading of the results: sorts by it, A to Z, or flips the
    /// direction when it already sorts by it.
    Sort(Column),
    /// "Accept license agreements automatically" in its settings.
    AcceptAgreements(bool),
    /// The bundle folder chosen in its settings (None: back to Documents).
    BundleFolder(Option<String>),
    /// "Install selected": these packages, in series.
    InstallAll(Vec<Found>),
    /// "Stop after this one" while a series runs.
    StopAfterCurrent,
    /// "Retry the failed ones" under a series' summary.
    RetryFailed,
    /// "Open the log folder".
    OpenLogs,
    /// "Open log" beside a package of the series (its index).
    OpenItemLog(usize),
    /// "Create a bundle": lists the installed packages into a new bundle.
    CreateBundle,
    /// "Add" under a bundle being made: an entry no catalog knows.
    AddManualEntry { name: String, url: String },
    /// The tick of the bundle's entry at this index.
    TickBundleEntry(usize, bool),
    /// "Save the bundle…": the ticked entries, to this file.
    SaveBundle(PathBuf),
    /// "Open a bundle…": this file.
    OpenBundle(PathBuf),
    /// "Install selected" under an opened bundle: its ticked entries, in series.
    InstallBundle,
    /// "Close" under a bundle.
    CloseBundle,
    /// "Check for updates": lists the installed packages.
    ListInstalled,
    /// "Update" beside an installed package (at its newer version).
    Update(Found),
    /// "Update all": these packages, in series.
    UpdateAll(Vec<Found>),
    /// "Uninstall" beside an installed package.
    Uninstall(installed::Package),
    /// "Stop waiting" for a program's own uninstaller.
    StopWaiting,
    /// "Continue" on the interrupted run: installs its missing packages.
    ContinueInterrupted,
    /// "Ignore" on the interrupted run: closes it.
    IgnoreInterrupted,
    /// The version box of the bundle's entry at this index (empty: the
    /// latest version).
    SetBundleVersion(usize, String),
    /// The scope choice of the bundle's entry at this index (None: winget's
    /// default).
    SetBundleScope(usize, Option<catalog::Scope>),
    /// "Export for winget…": the ticked package entries, to this file.
    ExportWinget(PathBuf),
    /// "Open the site" beside a package of the series to install by hand
    /// (its index).
    OpenItemSite(usize),
}

/// A column of the search results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    Name,
    Id,
    Version,
    Catalog,
}

impl Column {
    /// The heading's text.
    fn heading(self) -> &'static str {
        match self {
            Column::Name => "Name",
            Column::Id => "Id",
            Column::Version => "Version",
            Column::Catalog => "Catalog",
        }
    }
}

/// Sorts `packages` by `sort` (Z to A when `desc`); without a column they
/// keep the catalogs' order. Equal values keep their order.
fn sort_packages(packages: &mut [Found], sort: Option<Column>, desc: bool) {
    let Some(column) = sort else { return };
    let text = |a: &str, b: &str| a.to_lowercase().cmp(&b.to_lowercase());
    packages.sort_by(|a, b| {
        let order = match column {
            Column::Name => text(&a.name, &b.name),
            Column::Id => text(&a.id, &b.id),
            Column::Version => crate::addons::installed_apps::page::compare_versions(&a.version, &b.version),
            Column::Catalog => text(&a.catalog, &b.catalog),
        };
        if desc {
            order.reverse()
        } else {
            order
        }
    });
}


/// A series of installs, running or done.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Batch {
    /// Its packages, in the order they run.
    pub items: Vec<Item>,
    /// The output of the package installing now, line by line.
    pub lines: Vec<String>,
    /// "Stop after this one" was asked.
    pub stopping: bool,
    /// The series ended.
    pub done: bool,
    /// What it does to its packages; its words come from it.
    pub kind: Kind,
}

impl Batch {
    /// The summary in words (see [`batch::summary`]).
    pub fn summary(&self) -> Vec<String> {
        batch::summary(&self.items, self.kind)
    }
}

/// Whether installing `id` from `catalog` needs administrator rights, as
/// its catalog declares.
fn catalog_needs_admin(catalog: &str, id: &str) -> Option<bool> {
    catalog::source_of(catalog).and_then(|s| s.needs_admin(id))
}

/// The page's commands waiting for its next frame.
type Commands = Arc<Mutex<Vec<Command>>>;

/// Where install logs go: %LOCALAPPDATA%\UltimatePCManager\package-manager\logs.
fn default_logs() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(crate::settings::DATA_FOLDER).join(ID).join("logs")
}

/// Where the series' journals go: the add-on's folder beside the app's
/// settings, %APPDATA%\UltimatePCManager\package-manager\journal. Tests get a
/// folder of their own, never the user's.
fn default_journals() -> PathBuf {
    if cfg!(test) {
        return readiness::temp_output().with_extension("journal");
    }
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join(crate::settings::DATA_FOLDER).join(ID).join("journal")
}

/// The bundle folder when none was chosen: the user's Documents.
fn default_bundle_folder() -> PathBuf {
    let home = std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    home.join("Documents")
}

/// A new log file in `logs` for `verb` ("install", "update", "uninstall")
/// of `catalog`/`id`: "install-winget-VideoLAN.VLC-<millis>.log".
fn log_path(logs: &Path, verb: &str, catalog: &str, id: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let id: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    logs.join(format!("{verb}-{catalog}-{id}-{stamp}.log"))
}

/// Its settings, saved under its id; fields left at their default are not
/// written.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq)]
#[serde(default)]
struct Prefs {
    /// Accept the package's and the catalog's agreements without asking
    /// (on by default).
    #[serde(rename = "acceptAgreements", skip_serializing_if = "is_true")]
    accept_agreements: bool,
    /// Where bundles are saved and opened (None: the Documents folder).
    #[serde(rename = "bundleFolder", skip_serializing_if = "Option::is_none")]
    bundle_folder: Option<String>,
    /// The column the results are sorted by (None: the catalogs' order).
    #[serde(skip_serializing_if = "Option::is_none")]
    sort: Option<Column>,
    /// The sort goes Z to A.
    #[serde(rename = "sortDesc", skip_serializing_if = "is_default")]
    sort_desc: bool,
    /// An install was started, so the note about the agreements was seen.
    #[serde(rename = "agreementsNoted", skip_serializing_if = "is_default")]
    agreements_noted: bool,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs { accept_agreements: true, bundle_folder: None, sort: None, sort_desc: false, agreements_noted: false, rest: Default::default() }
    }
}

/// The value is `true` (a setting on by default, left out when saved).
fn is_true(value: &bool) -> bool {
    *value
}

/// Installs `package` with `source`, its log in `log` (inside the folder
/// `logs`, made first), then checks that it shows as installed when the
/// exit code says it was. `on_line` gets each line of winget's output. An
/// install is never cut short.
fn install_package(context: &AddonContext, source: &dyn PackageSource, package: &Found, pin: &Pin, log: &Path, logs: &Path, accept: bool, on_line: &mut dyn FnMut(&str)) -> Outcome {
    // An install is never cut short: nothing sets this.
    let never = AtomicBool::new(false);
    let target = Target { catalog: &package.catalog, id: &package.id, log, accept_agreements: accept, pin };
    let ran = match std::fs::create_dir_all(logs) {
        Err(e) => Ran::NotStarted(format!("the log folder {} could not be made ({e}).", logs.display())),
        Ok(()) => source.install(context, &target, &never, on_line),
    };
    match ran {
        Ran::NotStarted(why) => Outcome::NotStarted(why),
        Ran::Stopped => Outcome::NotStarted("it was stopped.".into()),
        Ran::Exited(code) => {
            let installed = matches!(classify(code), ExitClass::Installed | ExitClass::InstalledRestart);
            let found = if installed { source.is_installed(context, &package.catalog, &package.id, accept, &never) } else { None };
            Outcome::Exited { code, found }
        }
    }
}

/// Marks `package` installed among the search results once `outcome` shows
/// it installed.
fn mark_installed(st: &mut State, package: &Found, outcome: &Outcome) {
    if let (Outcome::Exited { found: Some(true), .. }, Search::Found { packages, .. }) = (outcome, &mut st.search) {
        packages.iter_mut().filter(|p| p.catalog == package.catalog && p.id == package.id).for_each(|p| p.installed = true);
    }
}

/// Ends a series' batch in `st`: it is done in the same moment its job
/// gives the busy text back, so whatever the page offers once it shows
/// the series done ("Retry the failed ones") can start.
fn end_batch(run: &Run<State>, st: &mut State) {
    if let Some(b) = st.batch.as_mut() {
        b.done = true;
    }
    run.release(st);
}

/// Ends the batch when its job panics: the package running failed, the
/// packages not reached are not started, and the series is done (so the
/// app can close and the failed packages be retried).
struct EndBatch<'a>(&'a Run<State>);

impl Drop for EndBatch<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let why = "Package Manager stopped unexpectedly";
        let mut st = self.0.follow();
        if let Some(b) = st.batch.as_mut() {
            for item in &mut b.items {
                // Only one of the two applies (running, or not reached).
                item.set(Status::Failed(why.into()));
                item.set(Status::NotStarted(why.into()));
            }
        }
        end_batch(self.0, &mut st);
    }
}

/// The Package Manager add-on.
pub struct PackageManager {
    /// Set while it is on.
    context: Option<AddonContext>,
    state: Arc<Mutex<State>>,
    generation: Generation,
    /// Ends the programs it runs when it is turned off (never an install).
    stop: Arc<AtomicBool>,
    /// Runs its install jobs; made when it is turned on.
    series: Option<Series<State>>,
    commands: Commands,
    /// The search box's text.
    query: String,
    /// Where install logs go.
    logs: PathBuf,
    /// Its settings, saved under its id.
    prefs: Prefs,
    /// Whether a package needs administrator rights (its catalog's word;
    /// tests declare their own).
    needs_admin: fn(&str, &str) -> Option<bool>,
    /// The results ticked for "Install selected", by (catalog, id).
    selected: Vec<(String, String)>,
    /// The name and link typed for a bundle's manual entry.
    manual_entry: (String, String),
    /// How what no catalog handles is uninstalled.
    fallback: installed::Fallback,
    /// Where the series' journals go.
    journals: PathBuf,
    /// What the page shows: the scope, the entry picked, the job followed.
    view: page::View,
    /// Tests: its jobs wait for this lock before they run (see
    /// [`Series::gated`]).
    #[cfg(test)]
    job_gate: Option<Arc<Mutex<()>>>,
}

impl Default for PackageManager {
    fn default() -> Self {
        PackageManager {
            context: None,
            state: Default::default(),
            generation: Default::default(),
            stop: Default::default(),
            series: None,
            commands: Default::default(),
            query: String::new(),
            logs: default_logs(),
            prefs: Prefs::default(),
            needs_admin: catalog_needs_admin,
            selected: Vec::new(),
            manual_entry: Default::default(),
            fallback: Default::default(),
            journals: default_journals(),
            view: page::View::default(),
            #[cfg(test)]
            job_gate: None,
        }
    }
}

impl PackageManager {
    /// What the page shows now.
    pub fn state(&self) -> State {
        lock_state(&self.state).clone()
    }

    /// The state shared with its background work (tests watch it while the
    /// add-on is inside a host).
    #[cfg(test)]
    pub fn view(&self) -> Arc<Mutex<State>> {
        self.state.clone()
    }

    /// The line the page's actions go through (tests act through it while
    /// the add-on is inside a host).
    #[cfg(test)]
    pub fn commands(&self) -> Commands {
        self.commands.clone()
    }

    /// Applies the commands waiting for this frame.
    fn apply_commands(&mut self) {
        let commands = std::mem::take(&mut *lock_state(&self.commands));
        for c in commands {
            self.view.follow(&c);
            match c {
                Command::Search(q) => self.search(&q),
                Command::Install(p) => self.install(p),
                Command::OpenLog => self.open_log(),
                Command::Sort(column) => self.sort_by(column),
                Command::AcceptAgreements(on) => self.prefs.accept_agreements = on,
                Command::BundleFolder(folder) => self.prefs.bundle_folder = folder,
                Command::InstallAll(packages) => self.install_all(packages),
                Command::StopAfterCurrent => self.stop_after_current(),
                Command::RetryFailed => self.retry_failed(),
                Command::OpenLogs => self.open_logs(),
                Command::OpenItemLog(i) => self.open_item_log(i),
                Command::CreateBundle => self.create_bundle(),
                Command::AddManualEntry { name, url } => self.add_manual_entry(&name, &url),
                Command::TickBundleEntry(i, on) => self.tick_bundle_entry(i, on),
                Command::SaveBundle(path) => self.save_bundle(&path),
                Command::OpenBundle(path) => self.open_bundle(&path),
                Command::InstallBundle => self.install_bundle(),
                Command::CloseBundle => lock_state(&self.state).bundle = None,
                Command::ListInstalled => self.list_installed(),
                Command::Update(p) => self.update_all(vec![p]),
                Command::UpdateAll(packages) => self.update_all(packages),
                Command::Uninstall(p) => self.uninstall(p),
                Command::StopWaiting => self.stop_waiting(),
                Command::ContinueInterrupted => self.close_interrupted(true),
                Command::IgnoreInterrupted => self.close_interrupted(false),
                Command::SetBundleVersion(i, version) => self.set_bundle_version(i, &version),
                Command::SetBundleScope(i, scope) => self.set_bundle_scope(i, scope),
                Command::ExportWinget(path) => self.export_winget(&path),
                Command::OpenItemSite(i) => self.open_item_site(i),
            }
        }
    }

    /// Where bundles are saved and opened: the folder chosen in its
    /// settings, else Documents.
    pub fn bundle_folder(&self) -> PathBuf {
        self.prefs.bundle_folder.as_ref().map(PathBuf::from).unwrap_or_else(default_bundle_folder)
    }

    /// Sorts the results by `column`, A to Z, or flips the direction when
    /// they are already sorted by it; the choice is saved.
    fn sort_by(&mut self, column: Column) {
        let prefs = &mut self.prefs;
        prefs.sort_desc = prefs.sort == Some(column) && !prefs.sort_desc;
        prefs.sort = Some(column);
        if let Search::Found { packages, .. } = &mut lock_state(&self.state).search {
            sort_packages(packages, prefs.sort, prefs.sort_desc);
        }
    }

    /// Searches every catalog for `query` in the background (nothing for
    /// a blank query, or while a search runs).
    pub fn search(&mut self, query: &str) {
        let query = query.trim().to_string();
        let Some(context) = self.context.clone() else { return };
        if query.is_empty() || matches!(self.state().search, Search::Searching(_)) {
            return;
        }
        lock_state(&self.state).search = Search::Searching(query.clone());
        let (job, stop, accept) = (Job::new(&self.state, &self.generation), self.stop.clone(), self.prefs.accept_agreements);
        let (sort, desc) = (self.prefs.sort, self.prefs.sort_desc);
        std::thread::spawn(move || {
            let mut packages = Vec::new();
            for source in catalog::SOURCES {
                match source.search(&context, &query, accept, &stop) {
                    None => return,
                    Some(Ok(found)) => packages.extend(found),
                    Some(Err(e)) => return job.publish(|s| s.search = Search::Failed(e)),
                }
            }
            let mut note = None;
            for source in catalog::SOURCES {
                match source.list_installed(&context, accept, &stop) {
                    None => return,
                    Some(Ok(installed)) => catalog::mark_installed(&mut packages, &installed),
                    Some(Err(e)) => note = Some(format!("Which packages are installed is not known: {e}")),
                }
            }
            sort_packages(&mut packages, sort, desc);
            job.publish(|s| s.search = Search::Found { query, packages, note });
        });
    }

    /// Installs `package` in the background through the app's install
    /// queue. Nothing when the add-on is off, its catalog is unknown or an
    /// install already runs. Turning the add-on off lets the install finish.
    pub fn install(&mut self, package: Found) {
        let (Some(context), Some(series)) = (self.context.clone(), self.series.clone()) else { return };
        let Some(source) = catalog::source_of(&package.catalog) else { return };
        let (logs, accept) = (self.logs.clone(), self.prefs.accept_agreements);
        let busy = format!("Installing {}\u{2026}", package.name);
        self.prefs.agreements_noted = true;
        series.start(Some(busy), true, move |run| {
            let request = Request { addon: ID, name: "Package Manager", work: Work::Install, items: vec![(package.catalog.clone(), package.id.clone())] };
            let Some(turn) = run.queue(context.install_queue(), request) else { return };
            let log = log_path(&logs, "install", &package.catalog, &package.id);
            run.follow().install = Some(Install { package: package.clone(), lines: Vec::new(), log: log.clone(), outcome: None });
            let outcome = install_package(&context, source, &package, &Pin::default(), &log, &logs, accept, &mut |line| {
                if let Some(i) = run.follow().install.as_mut() {
                    i.lines.push(line.to_string());
                }
            });
            drop(turn);
            let mut st = run.follow();
            mark_installed(&mut st, &package, &outcome);
            if let Some(i) = st.install.as_mut() {
                i.outcome = Some(outcome);
            }
        });
    }

    /// Installs `packages` one after another in the background, through the
    /// app's install queue (one request for the whole series): those whose
    /// catalog declares that they need administrator rights first, then the
    /// order given. A package already installed is skipped; an outcome never
    /// stops the packages after it. "Stop after this one" and turning the
    /// add-on off let the current package finish and start no other.
    pub fn install_all(&mut self, packages: Vec<Found>) {
        let items = self.new_items(Kind::Install, packages);
        self.run_series(Kind::Install, items);
    }

    /// The items of a new series of `kind` over `packages`: an install
    /// whose catalog declares that it needs administrator rights runs first.
    pub(super) fn new_items(&self, kind: Kind, packages: Vec<Found>) -> Vec<Item> {
        let needs_admin = self.needs_admin;
        packages.into_iter().map(|p| Item { admin: kind == Kind::Install && needs_admin(&p.catalog, &p.id) == Some(true), ..Item::new(p, false) }).collect()
    }

    /// Runs the pending `items` in series as `kind` (see
    /// [`Self::install_all`]); the others (a bundle's manual entries) are
    /// only listed. The series is journalled, so one cut short can be
    /// continued at the next start as the same kind of work.
    pub(super) fn run_series(&mut self, kind: Kind, items: Vec<Item>) {
        let (Some(context), Some(series)) = (self.context.clone(), self.series.clone()) else { return };
        if items.is_empty() {
            return;
        }
        let items = batch::order(items);
        let pending: Vec<usize> = (0..items.len()).filter(|&i| items[i].status == Status::Pending).collect();
        if pending.is_empty() {
            lock_state(&self.state).batch = Some(Batch { items, kind, done: true, ..Batch::default() });
            return;
        }
        let (logs, accept, journals) = (self.logs.clone(), self.prefs.accept_agreements, self.journals.clone());
        self.prefs.agreements_noted = true;
        let busy = match pending.len() {
            1 => format!("{} {}\u{2026}", kind.doing(), items[pending[0]].package.name),
            n => format!("{} {n} packages\u{2026}", kind.doing()),
        };
        // The batch exists from the moment the series is accepted (under
        // the lock `close_question` reads), so closing the app right after
        // the start still asks first.
        let batch = Batch { items: items.clone(), kind, ..Batch::default() };
        series.start_with(Some(busy), true, move |st| st.batch = Some(batch), move |run| {
            let _end = EndBatch(run);
            let journal = std::cell::RefCell::new(journal::Journal::begin(&journals, &logs, &items, kind));
            // Sets the status of package `i` (with its exit code once it
            // ran) and rewrites the journal.
            let set_with = |i: usize, status: Status, code: Option<i32>| {
                let item = run.follow().batch.as_mut().and_then(|b| b.items[i].set(status).then(|| b.items[i].clone()));
                if let Some(item) = item {
                    journal.borrow_mut().record(i, &item, code);
                }
            };
            let set = |i: usize, status: Status| set_with(i, status, None);
            let request = Request { addon: ID, name: "Package Manager", work: kind.work(), items: pending.iter().map(|&i| (items[i].package.catalog.clone(), items[i].package.id.clone())).collect() };
            let turn = run.queue(context.install_queue(), request);
            if turn.is_some() {
                let total = items.len();
                run.each(&pending, |&i, _| format!("{} {} ({} of {total})\u{2026}", kind.doing(), items[i].package.name, i + 1), |&i| {
                    let package = &items[i].package;
                    set(i, Status::Running);
                    let mut code = None;
                    let status = match catalog::source_of(&package.catalog) {
                        _ if kind == Kind::Install && package.installed => Status::Skipped("already installed".into()),
                        None => Status::NotStarted(format!("its catalog \u{201c}{}\u{201d} is not available", package.catalog)),
                        Some(source) => {
                            let log = log_path(&logs, kind.log_word(), &package.catalog, &package.id);
                            if let Some(b) = run.follow().batch.as_mut() {
                                b.lines.clear();
                                b.items[i].log = Some(log.clone());
                            }
                            let mut on_line = |line: &str| {
                                if let Some(b) = run.follow().batch.as_mut() {
                                    b.lines.push(line.to_string());
                                }
                            };
                            let (outcome, status) = match kind {
                                Kind::Install => {
                                    let outcome = install_package(&context, source, package, &items[i].pin, &log, &logs, accept, &mut on_line);
                                    mark_installed(&mut run.follow(), package, &outcome);
                                    let status = outcome.status();
                                    (outcome, status)
                                }
                                Kind::Update => {
                                    let outcome = installed::update_package(&context, source, package, &log, &logs, accept, &mut on_line);
                                    let status = installed::update_status(&outcome, &package.version);
                                    (outcome, status)
                                }
                            };
                            if let Outcome::Exited { code: c, .. } = outcome {
                                code = Some(c);
                            }
                            status
                        }
                    };
                    set_with(i, status, code);
                });
            }
            drop(turn);
            let why = if run.stopped() { "Package Manager was turned off" } else { "the series was stopped" };
            // A package left unstarted means the series was cut short.
            let ended = run.follow().batch.as_mut().map(|b| (b.items.iter_mut().fold(false, |cut, item| item.set(Status::NotStarted(why.into())) || cut), b.items.clone()));
            if let Some((cut, items)) = ended {
                let mut journal = journal.borrow_mut();
                journal.record_all(&items);
                if !cut {
                    journal.finish();
                }
            }
            // Done only once the journal is written.
            end_batch(run, &mut run.follow());
        });
    }

    /// Ends the running series after its current package.
    fn stop_after_current(&mut self) {
        let Some(series) = &self.series else { return };
        let mut st = lock_state(&self.state);
        if let Some(b) = st.batch.as_mut().filter(|b| !b.done) {
            b.stopping = true;
            series.stop_after_current();
        }
    }

    /// Runs again, as the same kind of work, the packages of the last
    /// series that failed or were never started.
    fn retry_failed(&mut self) {
        let Some(b) = self.state().batch.filter(|b| b.done) else { return };
        let items = self.new_items(b.kind, batch::failed(&b.items));
        self.run_series(b.kind, items);
    }

    /// Opens the folder of the install logs (made when missing).
    fn open_logs(&mut self) {
        let Some(context) = self.context.clone() else { return };
        let error = std::fs::create_dir_all(&self.logs).err().map(|e| format!("The log folder {} could not be made ({e}).", self.logs.display()));
        let error = error.or_else(|| context.open(&self.logs.to_string_lossy()).err());
        lock_state(&self.state).repair_error = error;
    }

    /// Opens the log of the series' package at `index`.
    fn open_item_log(&mut self, index: usize) {
        let Some(context) = self.context.clone() else { return };
        let Some(log) = self.state().batch.and_then(|b| b.items.get(index).and_then(|i| i.log.clone())) else { return };
        let error = context.open(&log.to_string_lossy()).err();
        lock_state(&self.state).repair_error = error;
    }

    /// Opens the site of a package of the series to install by hand.
    fn open_item_site(&mut self, index: usize) {
        let Some(context) = self.context.clone() else { return };
        let Some(url) = self.state().batch.and_then(|b| match b.items.get(index).map(|i| i.status.clone()) {
            Some(Status::Manual(url)) => Some(url),
            _ => None,
        }) else {
            return;
        };
        let error = context.open(&url).err();
        lock_state(&self.state).repair_error = error;
    }

    /// Opens the last install's log.
    fn open_log(&mut self) {
        let Some(context) = self.context.clone() else { return };
        let Some(log) = self.state().install.map(|i| i.log) else { return };
        let error = context.open(&log.to_string_lossy()).err();
        lock_state(&self.state).repair_error = error;
    }

    /// Starts the work the page needs next: the winget check when it was
    /// never made, the first catalog update once winget is ready.
    fn advance(&mut self) {
        let st = self.state();
        match st.check {
            Check::NotChecked => self.begin_check(),
            Check::Known(Readiness::Ready(_)) if st.catalogs == Catalogs::NotUpdated => self.begin_catalog_update(),
            _ => {}
        }
    }

    /// Checks winget again in the background.
    fn begin_check(&mut self) {
        self.spawn(Check::Checking, |context, stop, job| {
            if let Some(r) = readiness::check(context, stop) {
                job.publish(|s| s.check = Check::Known(r));
            }
        });
    }

    /// Runs `work` on a thread with the add-on's context, after setting
    /// the check to `check`. Does nothing while the add-on is off.
    fn spawn(&mut self, check: Check, work: impl FnOnce(&AddonContext, &AtomicBool, &Job<State>) + Send + 'static) {
        let Some(context) = self.context.clone() else { return };
        {
            let mut s = lock_state(&self.state);
            s.check = check;
            s.repair_error = None;
        }
        let (job, stop) = (Job::new(&self.state, &self.generation), self.stop.clone());
        std::thread::spawn(move || work(&context, &stop, &job));
    }

    /// Lets winget update its catalogs (slow the first time).
    fn begin_catalog_update(&mut self) {
        let Some(context) = self.context.clone() else { return };
        lock_state(&self.state).catalogs = Catalogs::Updating;
        let (job, stop) = (Job::new(&self.state, &self.generation), self.stop.clone());
        std::thread::spawn(move || {
            let outcome = match readiness::ask(&context, "winget", &["source", "update"], &stop) {
                None => return,
                Some(Answer::Exited(0, _)) => Catalogs::Updated,
                Some(Answer::Exited(code, _)) => Catalogs::Failed(crate::platform::winget::describe(code)),
                Some(Answer::NotStarted) => Catalogs::Failed("winget did not start.".into()),
            };
            // The catalogs removed or disabled on this PC: their bundle
            // entries cannot be installed.
            let mut inactive = Vec::new();
            for source in catalog::SOURCES {
                let Some(found) = source.inactive_catalogs(&context, &stop) else { return };
                inactive.extend(found);
            }
            job.publish(|s| {
                s.catalogs = outcome;
                s.inactive_catalogs = inactive;
                bundle::untick_problems(s);
            });
        });
    }

    /// Runs the repair the current readiness offers (nothing when winget
    /// is ready or not checked yet).
    pub fn repair(&mut self) {
        let Check::Known(r) = self.state().check else { return };
        let Some(context) = self.context.clone() else { return };
        match r.repair() {
            None => {}
            Some((Repair::Open(link), _)) => {
                let error = context.open(link).err();
                lock_state(&self.state).repair_error = error;
            }
            Some((Repair::Register, _)) => self.spawn(Check::Repairing, |context, stop, job| {
                let args = readiness::powershell_args(readiness::REGISTER_APP_INSTALLER);
                let error = match readiness::ask(context, "powershell.exe", &args, stop) {
                    None => return,
                    Some(Answer::Exited(0, _)) => None,
                    Some(Answer::Exited(code, out)) => Some(format!("Registering App Installer failed (exit code {code}). {}", out.trim())),
                    Some(Answer::NotStarted) => Some("PowerShell did not start.".into()),
                };
                if let Some(r) = readiness::check(context, stop) {
                    job.publish(|s| {
                        s.check = Check::Known(r);
                        s.repair_error = error.map(|e| e.trim().to_string());
                    });
                }
            }),
        }
    }

    /// The app's install queue while another add-on holds it: who, doing
    /// what, and how many items wait (its own requests included).
    fn draw_queue(&mut self, ui: &mut Ui) {
        let Some(context) = &self.context else { return };
        let queue = context.install_queue();
        let Some(holder) = queue.holder().filter(|h| h.addon != ID) else { return };
        let text = match queue.waiting() {
            0 => format!("Install queue: {}.", holder.doing()),
            n => format!("Install queue: {}; {} wait.", holder.doing(), crate::addons::plural(n, "item")),
        };
        let amber = crate::theme::of_ui(ui).amber;
        crate::widgets::note(ui, crate::widgets::Tone::Neutral, |ui| {
            ui.horizontal_wrapped(|ui| {
                crate::widgets::dot(ui, amber);
                ui.label(RichText::new(text).size(13.0));
            });
        });
        ui.ctx().request_repaint_after(Duration::from_millis(500));
    }
}

/// A spinner with `text`.
fn busy_line(ui: &mut Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.spinner();
        ui.label(RichText::new(text).size(13.0));
    });
}

/// What the page says about the first catalog update.
pub fn catalogs_text(catalogs: &Catalogs) -> Option<String> {
    match catalogs {
        Catalogs::NotUpdated => None,
        Catalogs::Updating => Some("Updating the catalogs\u{2026} The first update can take a minute.".into()),
        Catalogs::Updated => Some("The catalogs are up to date.".into()),
        Catalogs::Failed(why) => Some(format!("Could not update the catalogs: {why}")),
    }
}

impl Addon for PackageManager {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Package Manager"
    }
    fn description(&self) -> &'static str {
        "Install, update and remove programs from the winget catalogs."
    }
    fn requirements(&self) -> Requirements {
        Requirements { admin: true, network: true, programs: &["winget (App Installer)", "PowerShell"], note: Some(NEEDS) }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Low", network: "While checking, searching or installing", disk: "While installing", when: "Only while its page is open or a job runs" }
    }
    /// Its settings ([`Prefs`]); `Null` while nothing differs from the
    /// defaults.
    fn settings(&self) -> serde_json::Value {
        write_settings(&self.prefs)
    }
    fn load_settings(&mut self, settings: &serde_json::Value) {
        self.prefs = read_settings(settings);
    }
    /// Automatic acceptance of the agreements and the bundle folder; the
    /// results' sort is chosen on the page.
    fn settings_section(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.apply_commands();
        let mut chosen = Vec::new();
        use crate::widgets::parts::{group_heading, setting_folder, setting_switch, FolderAct};
        let _ = page;
        group_heading(ui, "Installs");
        let mut accept = self.prefs.accept_agreements;
        let why = "Installs accept the package's and the catalog's agreements without asking, so a series does not stop on a question";
        if setting_switch(ui, "Accept license agreements automatically", Some(why), &mut accept) {
            chosen.push(Command::AcceptAgreements(accept));
        }
        let folder = self.bundle_folder().display().to_string();
        match setting_folder(ui, "Bundle folder", None, &folder, Some(("Reset to default", self.prefs.bundle_folder.is_some())), true) {
            FolderAct::Choose => {
                if let Some(dir) = self.context.as_ref().and_then(|c| c.pick_folder("Choose the folder for bundles", "Choose")) {
                    chosen.push(Command::BundleFolder(Some(dir)));
                }
            }
            FolderAct::Reset => chosen.push(Command::BundleFolder(None)),
            FolderAct::None => {}
        }
        if !chosen.is_empty() {
            lock_state(&self.commands).extend(chosen);
            ui.ctx().request_repaint();
        }
    }
    fn start(&mut self, context: &AddonContext) {
        self.context = Some(context.clone());
        self.stop = Arc::new(AtomicBool::new(false));
        let series = Series::new(&self.state, &self.generation, |s: &mut State| &mut s.busy);
        #[cfg(test)]
        let series = series.gated(self.job_gate.clone());
        self.series = Some(series);
        // Reading a small folder: no program runs, and a broken journal is
        // passed over.
        lock_state(&self.state).interrupted = journal::interrupted(&self.journals);
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        match &self.series {
            // A running install stays on the page until it ends.
            Some(series) => series.stop(|s| {
                let removal = s.installed.removal.take();
                *s = State { install: s.install.take(), busy: s.busy.take(), batch: s.batch.take(), installed: installed::Installed { removal, ..Default::default() }, ..State::default() }
            }),
            None => {
                let mut s = lock_state(&self.state);
                self.generation.bump();
                *s = State::default();
            }
        }
        lock_state(&self.commands).clear();
        self.context = None;
    }
    /// An install-queue job ended: the installed packages are read again
    /// when they were read.
    fn packages_changed(&mut self, _by: &'static str) {
        self.relist();
    }
    fn uninterruptible(&self) -> bool {
        self.series.as_ref().is_some_and(|s| s.running())
    }
    /// Asked while a series runs: closing lets the current package finish
    /// and starts no other; the journal offers the rest at the next start.
    fn close_question(&self) -> Option<String> {
        let b = self.state().batch.filter(|b| !b.done)?;
        let done = b.items.iter().filter(|i| i.status.finished()).count();
        Some(format!(
            "A series of {} is running ({done} of {} done). Closing lets the current package finish and starts no other; the rest can be continued at the next start.",
            b.kind.works(),
            b.items.len()
        ))
    }
    /// While a run that was interrupted waits for an answer: a link that
    /// opens the page with its offer to continue (also when the page was
    /// never opened since the app started).
    fn status_bar(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let Some(i) = self.state().interrupted else { return };
        let text = format!("{INTERRUPTED_NOTICE} ({} of {} done)", i.total - i.missing.len(), i.total);
        if ui.link(text).on_hover_text("Open Package Manager to continue or ignore it").clicked() {
            page.actions.push(crate::platform::Action::OpenPage(ID));
        }
    }
    fn refresh(&mut self) {
        if !self.state().busy() {
            self.begin_check();
        }
    }
    fn ui(&mut self, ui: &mut Ui, _page: &mut PageContext) {
        self.apply_commands();
        self.advance();
        let st = self.state();
        if st.busy() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        self.draw_page(ui, &st);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{until, AddonHost, Entry};
    use crate::platform::{Launch, RecordingRunner, When};
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use serde_json::json;

    /// Runs one windowless egui frame with a page context.
    fn frame(mut f: impl FnMut(&mut egui::Ui, &mut PageContext)) {
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        // The app\'s faces, as at start-up: pages draw in them.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| f(ui, &mut PageContext::new(&settings, &sample)));
        });
    }

    /// A host holding only Package Manager (on), with a view of its state.
    fn host() -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>) {
        let pm = PackageManager::default();
        let view = pm.view();
        let (host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true } }));
        (host, runner, view)
    }

    fn open_page(host: &mut AddonHost) {
        frame(|ui, page| {
            host.ui(ID, ui, page);
        });
    }

    fn check_of(view: &Mutex<State>) -> Check {
        lock_state(view).check.clone()
    }

    /// The readiness the page reaches with the replies `prepare` sets up.
    fn readiness_with(prepare: impl FnOnce(&RecordingRunner)) -> (Readiness, Vec<Launch>) {
        let (mut host, runner, view) = host();
        prepare(&runner);
        open_page(&mut host);
        until("the check", || matches!(check_of(&view), Check::Known(_)));
        let Check::Known(r) = check_of(&view) else { unreachable!() };
        (r, runner.launches())
    }

    fn run_args(launch: &Launch) -> (String, Vec<String>) {
        match launch {
            Launch::Run { program, args, .. } => (program.clone(), args.clone()),
            other => panic!("expected a run, got {other:?}"),
        }
    }

    #[test]
    fn its_requirements_name_the_internet_winget_and_admin_only_through_uac() {
        let (host, _) = AddonHost::for_tests(&json!({ "package-manager": { "enabled": false } }));
        let d = crate::addons::overview::Overview::of(&host, Some("package-manager")).detail.expect("listed in Settings › Add-ons");
        assert_eq!(d.requirements, ["Asks for administrator rights for some actions", "Uses the internet", "Runs winget (App Installer)", "Runs PowerShell"]);
        let note = d.note.expect("a note under the requirements");
        for words in ["internet", "winget", "UAC", "not start as administrator"] {
            assert!(note.contains(words), "the note says {words:?}: {note}");
        }
    }

    #[test]
    fn turned_on_it_asks_for_nothing_until_its_page_opens() {
        let (host, runner, view) = host();
        assert!(host.is_enabled(ID));
        std::thread::sleep(Duration::from_millis(30));
        assert!(runner.launches().is_empty(), "starting launched {:?}", runner.launches());
        assert_eq!(check_of(&view), Check::NotChecked);
    }

    #[test]
    fn turned_off_its_page_asks_for_nothing() {
        let (mut host, runner, view) = host();
        host.set_enabled(ID, false);
        open_page(&mut host);
        assert!(!host.refresh(ID));
        std::thread::sleep(Duration::from_millis(30));
        assert!(runner.launches().is_empty());
        assert_eq!(check_of(&view), Check::NotChecked);
    }

    #[test]
    fn opening_the_page_finds_winget_ready() {
        let (r, launches) = readiness_with(|runner| runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200\r\n"));
        assert_eq!(r, Readiness::Ready("1.9.25200".into()));
        assert_eq!(run_args(&launches[0]), ("winget".into(), vec!["--version".into()]));
    }

    #[test]
    fn opening_the_page_finds_winget_too_old() {
        let (r, _) = readiness_with(|runner| runner.prepare(When::Arg("--version".into()), 0, "v1.2.10271"));
        assert_eq!(r, Readiness::TooOld("1.2.10271".into()));
    }

    #[test]
    fn opening_the_page_finds_winget_not_registered_when_app_installer_is_there() {
        let (r, launches) = readiness_with(|runner| runner.prepare(When::Nth(1), 0, "1.21.3482.0\r\n"));
        assert_eq!(r, Readiness::NotRegistered);
        let (program, args) = run_args(&launches[1]);
        assert_eq!(program, "powershell.exe");
        assert!(args.iter().any(|a| a.contains("Get-AppxPackage") && a.contains("Microsoft.DesktopAppInstaller")), "{args:?}");
    }

    #[test]
    fn opening_the_page_finds_winget_missing_when_nothing_answers() {
        let (r, launches) = readiness_with(|_| {});
        assert_eq!(r, Readiness::Missing);
        assert_eq!(launches.len(), 2, "winget, then PowerShell: {launches:?}");
    }

    #[test]
    fn the_first_catalog_update_shows_that_it_runs() {
        let (mut host, runner, view) = host();
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        runner.hold();
        open_page(&mut host);
        runner.wait_for(2);
        assert_eq!(run_args(&runner.launches()[1]), ("winget".into(), vec!["source".into(), "update".into()]));
        assert_eq!(lock_state(&view).catalogs, Catalogs::Updating);
        assert!(catalogs_text(&Catalogs::Updating).unwrap().starts_with("Updating the catalogs"));
        runner.prepare(When::Arg("source".into()), 0, "Done");
        runner.release();
        until("updated", || lock_state(&view).catalogs == Catalogs::Updated);
        open_page(&mut host);
        let updates = winget_runs(&runner, "source").iter().filter(|a| a[1] == "update").count();
        assert_eq!(updates, 1, "updated once");
    }

    #[test]
    fn turning_it_off_during_the_check_ends_it_and_drops_its_answer() {
        let (mut host, runner, view) = host();
        runner.hold();
        open_page(&mut host);
        runner.wait_for(1);
        host.set_enabled(ID, false);
        until("the run is ended", || runner.stopped() == 1);
        runner.release();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(*lock_state(&view), State::default());
        assert_eq!(runner.launches().len(), 1);
    }

    /// Package Manager on alone (outside a host), its check done with the
    /// replies `prepare` sets up.
    fn checked(prepare: impl FnOnce(&RecordingRunner)) -> (PackageManager, Arc<RecordingRunner>) {
        let (context, runner) = AddonContext::recording();
        let mut pm = PackageManager::default();
        pm.start(&context);
        prepare(&runner);
        frame(|ui, page| pm.ui(ui, page));
        until("the check", || matches!(pm.state().check, Check::Known(_)));
        (pm, runner)
    }

    #[test]
    fn not_registered_is_repaired_by_registering_app_installer_then_checking_again() {
        let (mut pm, runner) = checked(|runner| runner.prepare(When::Nth(1), 0, "1.21.3482.0"));
        assert_eq!(pm.state().check, Check::Known(Readiness::NotRegistered));
        runner.prepare(When::Nth(2), 0, "");
        runner.prepare(When::Nth(3), 0, "v1.9.25200");
        pm.repair();
        until("checked again", || pm.state().check == Check::Known(Readiness::Ready("1.9.25200".into())));
        let (program, args) = run_args(&runner.launches()[2]);
        assert_eq!(program, "powershell.exe");
        assert!(args.iter().any(|a| a.contains("Add-AppxPackage -RegisterByFamilyName") && a.contains("Microsoft.DesktopAppInstaller_8wekyb3d8bbwe")), "{args:?}");
        assert_eq!(pm.state().repair_error, None);
    }

    #[test]
    fn a_failed_registration_says_so() {
        let (mut pm, runner) = checked(|runner| runner.prepare(When::Nth(1), 0, "1.21.3482.0"));
        runner.prepare(When::Nth(2), 1, "Deployment failed");
        runner.prepare(When::Nth(4), 0, "1.21.3482.0");
        pm.repair();
        until("checked again", || pm.state().check == Check::Known(Readiness::NotRegistered));
        let error = pm.state().repair_error.expect("an error");
        assert!(error.contains("exit code 1") && error.contains("Deployment failed"), "{error}");
    }

    #[test]
    fn missing_and_too_old_open_where_to_get_or_update_app_installer() {
        let (mut pm, runner) = checked(|_| {});
        pm.repair();
        assert_eq!(runner.launches().last(), Some(&Launch::Open(readiness::GET_WINGET.into())));
        let (mut pm, runner) = checked(|runner| runner.prepare(When::Arg("--version".into()), 0, "v1.2.10271"));
        pm.repair();
        assert_eq!(runner.launches().last(), Some(&Launch::Open(readiness::STORE_PAGE.into())));
    }

    #[test]
    fn ready_offers_no_repair() {
        let (mut pm, runner) = checked(|runner| runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200"));
        let before = runner.launches().len();
        pm.repair();
        assert_eq!(runner.launches().len(), before);
    }

    // ------------------------------------------------ search and install

    /// A scratch folder for the install logs (outside the worktree).
    fn scratch(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("upcm-pm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// A host holding only Package Manager (on, its logs in `logs`), with
    /// a view of its state and the line its page's actions go through.
    fn host_with_logs(logs: &std::path::Path) -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>, Commands) {
        let mut pm = PackageManager::default();
        pm.logs = logs.to_path_buf();
        pm.journals = logs.join("journal");
        let (view, commands) = (pm.view(), pm.commands());
        let (host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true } }));
        (host, runner, view, commands)
    }

    /// The user does `command` on the page (applied at its next frame).
    fn act(host: &mut AddonHost, commands: &Commands, command: Command) {
        lock_state(commands).push(command);
        open_page(host);
    }

    /// The arguments of each winget run whose first argument is `verb`.
    fn winget_runs(runner: &RecordingRunner, verb: &str) -> Vec<Vec<String>> {
        runner
            .launches()
            .iter()
            .filter_map(|l| match l {
                Launch::Run { program, args, .. } if program == "winget" && args.first().map(String::as_str) == Some(verb) => Some(args.clone()),
                _ => None,
            })
            .collect()
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    const SEARCH: &str = "Name                           Id                               Version     Match           Source\n\
-------------------------------------------------------------------------------------------------------\n\
VLC media player               VideoLAN.VLC                     3.0.21      Moniker: vlc    winget\n\
VLC Skins Pack                 SomeVendor.VeryLongPackageName.\u{2026}  1.2         Tag: vlc        winget\n\
VLC Remote                     Other.VeryLongRemoteNameForVlc.\u{2026}  4.0        Tag: vlc        winget\n\
VLC UWP                        9NBLGGH4VVNH                     Unknown                     msstore\n";

    const EXPORT: &str = r#"{ "Sources": [ { "Packages": [ { "PackageIdentifier": "VideoLAN.VLC", "Version": "3.0.21" } ], "SourceDetails": { "Name": "winget" } } ] }"#;

    fn found(name: &str, id: &str, version: &str, catalog: &str, installed: bool) -> Found {
        Found { name: name.into(), id: id.into(), version: version.into(), catalog: catalog.into(), installed }
    }

    fn found_of(view: &Mutex<State>) -> Option<Vec<Found>> {
        match &lock_state(view).search {
            Search::Found { packages, .. } => Some(packages.clone()),
            _ => None,
        }
    }

    #[test]
    fn opening_the_page_never_searches_and_enter_searches_the_catalogs() {
        let logs = scratch("enter");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        open_page(&mut host);
        open_page(&mut host);
        std::thread::sleep(Duration::from_millis(30));
        assert!(winget_runs(&runner, "search").is_empty(), "{:?}", runner.launches());
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        act(&mut host, &commands, Command::Search("  vlc ".into()));
        until("searched", || !matches!(lock_state(&view).search, Search::Searching(_)));
        assert_eq!(winget_runs(&runner, "search")[0], strings(&["search", "--query", "vlc", "--disable-interactivity", "--accept-source-agreements"]));
        act(&mut host, &commands, Command::Search("   ".into()));
        assert_eq!(winget_runs(&runner, "search").len(), 3, "an empty query searches nothing (the other two re-ask shortened ids)");
    }

    #[test]
    fn a_search_shows_name_id_version_catalog_and_which_are_installed() {
        let logs = scratch("marks");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        runner.prepare_file(When::Arg("export".into()), "--output", 0, EXPORT);
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        let packages = found_of(&view).unwrap();
        assert_eq!(packages[0], found("VLC media player", "VideoLAN.VLC", "3.0.21", "winget", true));
        assert_eq!(packages.last(), Some(&found("VLC UWP", "9NBLGGH4VVNH", "Unknown", "msstore", false)));
        let export = &winget_runs(&runner, "export")[0];
        assert_eq!((export[1].as_str(), export[3..].to_vec()), ("--output", strings(&["--include-versions", "--disable-interactivity", "--accept-source-agreements"])));
    }

    #[test]
    fn a_shortened_id_is_asked_again_and_otherwise_left_out_never_installable() {
        let logs = scratch("short");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        let whole = "Name            Id                                    Version  Source\n\
------------------------------------------------------------------------\n\
VLC Skins Pack  SomeVendor.VeryLongPackageName.Skins  1.2      winget\n";
        runner.prepare(When::Arg("SomeVendor.VeryLongPackageName.".into()), 0, whole);
        runner.prepare(When::Arg("Other.VeryLongRemoteNameForVlc.".into()), crate::platform::winget::NO_PACKAGE_FOUND, "No package found matching input criteria.");
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        let packages = found_of(&view).unwrap();
        assert_eq!(packages.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["VideoLAN.VLC", "SomeVendor.VeryLongPackageName.Skins", "9NBLGGH4VVNH"]);
        assert_eq!(
            winget_runs(&runner, "search")[1],
            strings(&["search", "--id", "SomeVendor.VeryLongPackageName.", "--source", "winget", "--disable-interactivity", "--accept-source-agreements"])
        );
        assert!(packages.iter().all(|p| !p.id.ends_with(crate::platform::winget::ELLIPSIS)));
    }

    #[test]
    fn a_search_finding_nothing_or_failing_says_so() {
        let logs = scratch("none");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--query".into()), crate::platform::winget::NO_PACKAGE_FOUND, "No package found matching input criteria.");
        act(&mut host, &commands, Command::Search("zzz".into()));
        until("found", || found_of(&view).is_some());
        assert_eq!(found_of(&view), Some(vec![]));
        assert_eq!(search_text(&lock_state(&view).search).as_deref(), Some("No package matches \u{201c}zzz\u{201d}."));
        runner.prepare(When::Arg("--query".into()), 0x8A15_0003_u32 as i32, "");
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("failed", || matches!(lock_state(&view).search, Search::Failed(_)));
        let Search::Failed(why) = lock_state(&view).search.clone() else { unreachable!() };
        assert!(why.contains("0x8A150003"), "{why}");
    }

    fn vlc() -> Found {
        found("VLC media player", "VideoLAN.VLC", "3.0.21", "winget", false)
    }

    fn outcome_of(view: &Mutex<State>) -> Option<Outcome> {
        lock_state(view).install.as_ref().and_then(|i| i.outcome.clone())
    }

    /// Installs VLC with the install's exit code and output and the
    /// verification's reply prepared; returns the install's state.
    fn install_vlc(tag: &str, prepare: impl FnOnce(&RecordingRunner)) -> (Install, Arc<RecordingRunner>, std::path::PathBuf) {
        let logs = scratch(tag);
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        prepare(&runner);
        act(&mut host, &commands, Command::Install(vlc()));
        until("installed", || outcome_of(&view).is_some());
        let install = lock_state(&view).install.clone().unwrap();
        (install, runner, logs)
    }

    const LISTED: &str = "Name              Id            Version  Source\n\
-----------------------------------------------------\n\
VLC media player  VideoLAN.VLC  3.0.21   winget\n";

    #[test]
    fn installing_runs_winget_with_the_fixed_arguments_then_checks_the_package_is_there() {
        let (install, runner, logs) = install_vlc("args", |runner| {
            runner.prepare(When::Arg("install".into()), 0, "Found VLC media player [VideoLAN.VLC]\n  \u{2588}\u{2588}\u{2588}\u{2592}\u{2592}  40%\nStarting package install...\nSuccessfully installed\n");
            runner.prepare(When::Arg("list".into()), 0, LISTED);
        });
        let log = install.log.to_string_lossy().into_owned();
        assert!(install.log.starts_with(&logs) && log.ends_with(".log"), "{log}");
        assert_eq!(
            winget_runs(&runner, "install"),
            [strings(&[
                "install",
                "--id",
                "VideoLAN.VLC",
                "--exact",
                "--source",
                "winget",
                "--silent",
                "--disable-interactivity",
                "--no-upgrade",
                "--log",
                &log,
                "--accept-package-agreements",
                "--accept-source-agreements"
            ])]
        );
        assert_eq!(winget_runs(&runner, "list"), [strings(&["list", "--id", "VideoLAN.VLC", "--exact", "--disable-interactivity", "--accept-source-agreements"])]);
        assert_eq!(install.outcome, Some(Outcome::Exited { code: 0, found: Some(true) }));
        assert_eq!(install.outcome.as_ref().unwrap().text("VLC media player"), "VLC media player was installed (exit code 0).");
        assert_eq!(install.last_lines(), "Found VLC media player [VideoLAN.VLC]\nStarting package install...\nSuccessfully installed", "no progress bar");
    }

    #[test]
    fn installed_but_not_found_is_told_apart() {
        let (install, ..) = install_vlc("missing", |runner| {
            runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
            runner.prepare(When::Arg("list".into()), crate::platform::winget::NO_PACKAGE_FOUND, "No installed package found matching input criteria.");
        });
        let outcome = install.outcome.unwrap();
        assert_eq!(outcome, Outcome::Exited { code: 0, found: Some(false) });
        assert_eq!(outcome.text("VLC"), "VLC was installed according to winget (exit code 0), but it is not found among the installed packages.");
    }

    #[test]
    fn each_class_of_exit_code_reads_in_words_with_the_raw_code_and_is_not_checked() {
        for (code, words) in [
            (0x8A15_0061_u32 as i32, "VLC was skipped: a version of the package is already installed (0x8A150061)."),
            (1602, "VLC was cancelled: the installation was cancelled (1602)."),
            (3010, "VLC was installed; restart Windows to finish (3010)."),
            (1603, "VLC started installing but failed: exit code 1603."),
        ] {
            let (install, runner, _) = install_vlc("classes", |runner| {
                runner.prepare(When::Arg("install".into()), code, "");
                runner.prepare(When::Arg("list".into()), 0, LISTED);
            });
            assert_eq!(install.outcome.as_ref().unwrap().text("VLC"), words);
            let checked = winget_runs(&runner, "list").len();
            assert_eq!(checked, usize::from(code == 3010), "only an install is checked ({code})");
        }
    }

    #[test]
    fn winget_not_starting_is_told_apart_from_starting_and_failing() {
        let (install, runner, _) = install_vlc("notstarted", |_| {});
        let outcome = install.outcome.unwrap();
        assert!(matches!(outcome, Outcome::NotStarted(_)));
        assert!(outcome.text("VLC").starts_with("VLC was not started: "), "{}", outcome.text("VLC"));
        assert!(winget_runs(&runner, "list").is_empty());
    }

    #[test]
    fn an_install_waits_for_the_install_queue() {
        let logs = scratch("queue");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        let request = crate::platform::install_queue::Request { addon: "installed-apps", name: "Installed Apps", work: crate::platform::install_queue::Work::Update, items: vec![] };
        let held = host.context().install_queue().enter(request, |_| {}).unwrap();
        runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
        runner.prepare(When::Arg("list".into()), 0, LISTED);
        act(&mut host, &commands, Command::Install(vlc()));
        until("waiting", || lock_state(&view).busy.as_deref().is_some_and(|b| b.starts_with("Installed Apps is updating")));
        assert!(winget_runs(&runner, "install").is_empty());
        assert_eq!(host.context().install_queue().waiting(), 1);
        drop(held);
        until("installed", || outcome_of(&view).is_some());
        assert_eq!(winget_runs(&runner, "install").len(), 1);
        assert_eq!(host.context().install_queue().take_finished(), ["installed-apps", ID]);
    }

    #[test]
    fn a_successful_install_marks_its_search_result_installed() {
        let logs = scratch("mark");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        assert!(!found_of(&view).unwrap()[0].installed);
        runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
        runner.prepare(When::Arg("list".into()), 0, LISTED);
        act(&mut host, &commands, Command::Install(found_of(&view).unwrap()[0].clone()));
        until("installed", || outcome_of(&view).is_some());
        assert!(found_of(&view).unwrap()[0].installed);
    }

    #[test]
    fn the_install_log_opens() {
        let logs = scratch("log");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("install".into()), 1603, "");
        act(&mut host, &commands, Command::Install(vlc()));
        until("done", || outcome_of(&view).is_some());
        act(&mut host, &commands, Command::OpenLog);
        let log = lock_state(&view).install.as_ref().unwrap().log.to_string_lossy().into_owned();
        // The page's winget check runs beside it, so the Open need not be
        // the last launch.
        assert!(runner.launches().contains(&Launch::Open(log)));
        assert!(logs.is_dir(), "the log folder is made before winget runs");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn turned_off_during_an_install_it_finishes_and_the_page_follows_it() {
        let logs = scratch("off");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::Install(vlc()));
        until("installing", || !winget_runs(&runner, "install").is_empty());
        host.set_enabled(ID, false);
        assert!(host.is_stopping(ID));
        runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
        runner.prepare(When::Arg("list".into()), 0, LISTED);
        runner.release();
        until("installed", || outcome_of(&view).is_some());
        assert_eq!(outcome_of(&view), Some(Outcome::Exited { code: 0, found: Some(true) }), "the install was not cut short");
        until("off", || host.settle() == [ID]);
    }

    // ------------------------------------------------------------ settings

    /// A host holding only Package Manager (on, its logs in `logs`) with
    /// `settings` saved under its id.
    fn host_with_settings(logs: &std::path::Path, settings: serde_json::Value) -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>, Commands) {
        let mut pm = PackageManager::default();
        pm.logs = logs.to_path_buf();
        let (view, commands) = (pm.view(), pm.commands());
        let (host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true, "settings": settings } }));
        (host, runner, view, commands)
    }

    #[test]
    fn with_agreements_turned_off_no_accept_flag_is_passed() {
        let logs = scratch("noaccept");
        let (mut host, runner, view, commands) = host_with_settings(&logs, json!({ "acceptAgreements": false }));
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
        runner.prepare(When::Arg("list".into()), 0, LISTED);
        act(&mut host, &commands, Command::Install(vlc()));
        until("installed", || outcome_of(&view).is_some());
        let runs: Vec<Vec<String>> = ["search", "export", "install", "list"].iter().flat_map(|verb| winget_runs(&runner, verb)).collect();
        assert!(runs.iter().any(|a| a[0] == "install"), "{runs:?}");
        for args in &runs {
            assert!(!args.iter().any(|a| a.starts_with("--accept-")), "no agreement accepted: {args:?}");
        }
    }

    /// Every text the page shows in one frame, a line each.
    fn page_text(host: &mut AddonHost) -> String {
        drawn_text(|ui, page| {
            host.ui(ID, ui, page);
        })
        .0
    }

    /// Every text `draw` shows in one frame, and the actions it asked for.
    fn drawn_text(mut draw: impl FnMut(&mut egui::Ui, &mut PageContext)) -> (String, Vec<crate::platform::Action>) {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        // The app\'s faces, as at start-up: pages draw in them.
        crate::theme::install_fonts(&ctx);
        let mut actions = Vec::new();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut page = PageContext::new(&settings, &sample);
                draw(ui, &mut page);
                actions = std::mem::take(&mut page.actions);
            });
        });
        let mut out = Vec::new();
        output.shapes.iter().for_each(|c| collect(&c.shape, &mut out));
        (out.join("\n"), actions)
    }

    /// What the status bar shows of the add-ons that are on.
    fn status_bar_text(host: &mut AddonHost) -> String {
        drawn_text(|ui, page| {
            host.status_bar_areas(ui, page);
        })
        .0
    }

    #[test]
    fn a_note_about_the_agreements_shows_until_the_first_install() {
        let note = "License agreements are accepted automatically";
        let logs = scratch("note");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        assert!(page_text(&mut host).contains(note), "{}", page_text(&mut host));

        runner.prepare(When::Arg("install".into()), 0, "Successfully installed");
        runner.prepare(When::Arg("list".into()), 0, LISTED);
        act(&mut host, &commands, Command::Install(vlc()));
        until("installed", || outcome_of(&view).is_some());
        assert_eq!(host.settings(ID), Some(json!({ "agreementsNoted": true })), "the note was seen before an install");
        host.set_enabled(ID, false);
        host.set_enabled(ID, true);
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        assert!(!page_text(&mut host).contains(note), "not after the first install");

        let (mut host, runner, view, _) = host_with_settings(&logs, json!({ "acceptAgreements": false }));
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        assert!(!page_text(&mut host).contains(note), "nothing is accepted automatically");
    }

    #[test]
    fn the_three_settings_are_saved_and_found_again_after_turning_it_off_and_on_and_restarting() {
        let logs = scratch("restart");
        let (mut host, _, _, commands) = host_with_logs(&logs);
        act(&mut host, &commands, Command::AcceptAgreements(false));
        act(&mut host, &commands, Command::BundleFolder(Some(r"D:\Bundles".into())));
        act(&mut host, &commands, Command::Sort(Column::Version));
        let saved = json!({ "acceptAgreements": false, "bundleFolder": r"D:\Bundles", "sort": "version" });
        assert_eq!(host.settings(ID), Some(saved.clone()));
        host.set_enabled(ID, false);
        assert_eq!(host.settings(ID), Some(saved.clone()), "kept while off");
        host.set_enabled(ID, true);
        assert_eq!(host.settings(ID), Some(saved.clone()), "and on again");

        let mut section = json!({});
        host.write_section(&mut section);
        let (restarted, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(PackageManager::default()))], &section);
        assert_eq!(restarted.settings(ID), Some(saved), "read back at the next start");

        act(&mut host, &commands, Command::BundleFolder(None));
        act(&mut host, &commands, Command::AcceptAgreements(true));
        assert_eq!(host.settings(ID), Some(json!({ "sort": "version" })), "defaults are not written");
    }

    #[test]
    fn bundles_go_to_documents_unless_another_folder_was_chosen() {
        let mut pm = PackageManager::default();
        assert!(pm.bundle_folder().ends_with("Documents"), "{}", pm.bundle_folder().display());
        pm.load_settings(&json!({ "bundleFolder": r"D:\Bundles" }));
        assert_eq!(pm.bundle_folder(), std::path::PathBuf::from(r"D:\Bundles"));
    }

    fn ids_of(view: &Mutex<State>) -> Vec<String> {
        found_of(view).unwrap_or_default().into_iter().map(|p| p.id).collect()
    }

    #[test]
    fn the_results_keep_the_catalog_order_until_a_heading_is_clicked_and_the_sort_comes_back() {
        let logs = scratch("sort");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        assert_eq!(ids_of(&view), ["VideoLAN.VLC", "9NBLGGH4VVNH"], "the catalog's order");
        assert_eq!(host.settings(ID), Some(serde_json::Value::Null), "nothing chosen, nothing saved");

        act(&mut host, &commands, Command::Sort(Column::Id));
        assert_eq!(ids_of(&view), ["9NBLGGH4VVNH", "VideoLAN.VLC"], "A to Z");
        act(&mut host, &commands, Command::Sort(Column::Id));
        assert_eq!(ids_of(&view), ["VideoLAN.VLC", "9NBLGGH4VVNH"], "the same heading again: Z to A");
        assert_eq!(host.settings(ID), Some(json!({ "sort": "id", "sortDesc": true })));

        host.set_enabled(ID, false);
        host.set_enabled(ID, true);
        assert_eq!(host.settings(ID), Some(json!({ "sort": "id", "sortDesc": true })), "kept while off and on again");
        runner.prepare(When::Arg("--query".into()), 0, SEARCH);
        act(&mut host, &commands, Command::Search("vlc".into()));
        until("found", || found_of(&view).is_some());
        assert_eq!(ids_of(&view), ["VideoLAN.VLC", "9NBLGGH4VVNH"], "a new search is sorted the same way");
        act(&mut host, &commands, Command::Sort(Column::Catalog));
        assert_eq!(ids_of(&view), ["9NBLGGH4VVNH", "VideoLAN.VLC"], "msstore before winget");
        assert_eq!(host.settings(ID), Some(json!({ "sort": "catalog" })), "another heading starts A to Z");
    }

    // ------------------------------------------------------------ series

    fn pkg(name: &str) -> Found {
        found(name, &format!("Vendor.{name}"), "1.0", "winget", false)
    }

    /// Prepares the install of `name` to exit with `code` (its check after
    /// gets no reply, so it is not known whether it is listed).
    fn exits(runner: &RecordingRunner, name: &str, code: i32) {
        runner.prepare(When::Arg(format!("Vendor.{name}")), code, "");
    }

    /// The ids winget was asked to install, in order.
    fn installed_ids(runner: &RecordingRunner) -> Vec<String> {
        winget_runs(runner, "install").iter().map(|a| a[2].clone()).collect()
    }

    fn batch_of(view: &Mutex<State>) -> Option<Batch> {
        lock_state(view).batch.clone()
    }

    fn statuses(view: &Mutex<State>) -> Vec<(String, Status)> {
        batch_of(view).map(|b| b.items.iter().map(|i| (i.package.name.clone(), i.status.clone())).collect()).unwrap_or_default()
    }

    fn series_done(view: &Mutex<State>) -> bool {
        batch_of(view).is_some_and(|b| b.done)
    }

    /// Installs `names` in series with the replies `prepare` sets up, and
    /// waits for the series to end.
    fn series_of(tag: &str, names: &[&str], prepare: impl FnOnce(&RecordingRunner)) -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>, Commands, std::path::PathBuf) {
        let logs = scratch(tag);
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        prepare(&runner);
        act(&mut host, &commands, Command::InstallAll(names.iter().map(|n| pkg(n)).collect()));
        until("the series ends", || series_done(&view));
        (host, runner, view, commands, logs)
    }

    fn named(v: &[(&str, Status)]) -> Vec<(String, Status)> {
        v.iter().map(|(n, s)| (n.to_string(), s.clone())).collect()
    }

    #[test]
    fn a_failure_in_the_middle_of_a_series_stops_none_of_the_packages_after_it() {
        let (_host, runner, view, _, logs) = series_of("middle", &["A", "B", "C"], |runner| {
            exits(runner, "A", 0);
            exits(runner, "B", 1603);
            exits(runner, "C", 0);
        });
        assert_eq!(installed_ids(&runner), ["Vendor.A", "Vendor.B", "Vendor.C"]);
        assert_eq!(statuses(&view), named(&[("A", Status::Ok), ("B", Status::Failed("exit code 1603".into())), ("C", Status::Ok)]));
        let batch = batch_of(&view).unwrap();
        assert_eq!(batch.summary(), ["Installed: A and C.", "Failed: B (exit code 1603)."]);
        assert!(batch.items.iter().all(|i| i.log.as_ref().is_some_and(|l| l.starts_with(&logs))), "each package has its log");
        assert_eq!(lock_state(&view).busy, None, "the series gave the busy text back");
    }

    #[test]
    fn a_series_installs_the_packages_needing_administrator_rights_first() {
        let logs = scratch("admin");
        let mut pm = PackageManager::default();
        pm.logs = logs.clone();
        pm.needs_admin = |_, id| Some(id == "Vendor.C");
        let (view, commands) = (pm.view(), pm.commands());
        let (mut host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true } }));
        for n in ["A", "B", "C"] {
            exits(&runner, n, 0);
        }
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B"), pkg("C")]));
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.C", "Vendor.A", "Vendor.B"]);
        assert_eq!(statuses(&view).iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["C", "A", "B"]);
    }

    #[test]
    fn a_closed_uac_prompt_cancels_its_package_and_the_series_goes_on() {
        let (_host, runner, view, ..) = series_of("uac", &["A", "B", "C"], |runner| {
            exits(runner, "A", 0);
            exits(runner, "B", 1602);
            exits(runner, "C", 3010);
        });
        assert_eq!(installed_ids(&runner).len(), 3);
        assert_eq!(statuses(&view), named(&[("A", Status::Ok), ("B", Status::Cancelled), ("C", Status::OkRestart)]));
        assert_eq!(batch_of(&view).unwrap().summary(), ["Installed: A.", "Installed, restart Windows to finish: C.", "Cancelled: B."]);
    }

    #[test]
    fn an_installed_package_is_skipped_without_running_winget() {
        let logs = scratch("skip");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        exits(&runner, "B", 0);
        act(&mut host, &commands, Command::InstallAll(vec![Found { installed: true, ..pkg("A") }, pkg("B")]));
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.B"]);
        assert_eq!(statuses(&view), named(&[("A", Status::Skipped("already installed".into())), ("B", Status::Ok)]));
    }

    #[test]
    fn stop_during_a_held_install_lets_it_finish_and_starts_no_other() {
        let logs = scratch("stop");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B"), pkg("C")]));
        until("installing A", || installed_ids(&runner).len() == 1);
        act(&mut host, &commands, Command::StopAfterCurrent);
        assert!(batch_of(&view).unwrap().stopping);
        assert_eq!(statuses(&view)[0].1, Status::Running);
        exits(&runner, "A", 0);
        runner.release();
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.A"]);
        let stopped = Status::NotStarted("the series was stopped".into());
        assert_eq!(statuses(&view), named(&[("A", Status::Ok), ("B", stopped.clone()), ("C", stopped)]));
        assert!(!host.is_stopping(ID) && host.is_enabled(ID));
    }

    #[test]
    fn turned_off_during_a_held_install_it_finishes_that_one_then_turns_off() {
        let logs = scratch("seriesoff");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("installing A", || installed_ids(&runner).len() == 1);
        host.set_enabled(ID, false);
        assert!(host.is_stopping(ID), "it will turn off when the series ends");
        exits(&runner, "A", 0);
        runner.release();
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.A"]);
        assert_eq!(statuses(&view), named(&[("A", Status::Ok), ("B", Status::NotStarted("Package Manager was turned off".into()))]));
        until("off", || host.settle() == [ID]);
    }

    #[test]
    fn retrying_installs_only_the_failed_packages_again() {
        let (mut host, runner, view, commands, _) = series_of("retry", &["A", "B", "C"], |runner| {
            exits(runner, "A", 0);
            exits(runner, "B", 1603);
            exits(runner, "C", 0);
        });
        exits(&runner, "B", 0);
        act(&mut host, &commands, Command::RetryFailed);
        until("the retry ends", || series_done(&view) && statuses(&view).len() == 1);
        assert_eq!(installed_ids(&runner), ["Vendor.A", "Vendor.B", "Vendor.C", "Vendor.B"]);
        assert_eq!(statuses(&view), named(&[("B", Status::Ok)]));
        assert_eq!(batch_of(&view).unwrap().summary(), ["B was installed."]);
    }

    #[test]
    fn retry_is_accepted_as_soon_as_the_series_shows_it_is_done() {
        for n in 0..30 {
            let logs = scratch(&format!("retry-now{n}"));
            let (mut host, runner, view, commands) = host_with_logs(&logs);
            exits(&runner, "A", 1603);
            act(&mut host, &commands, Command::InstallAll(vec![pkg("A")]));
            // The page never shows the series done while it still holds the
            // busy text (Retry would be refused).
            loop {
                let st = lock_state(&view);
                if st.batch.as_ref().is_some_and(|b| b.done) {
                    assert_eq!(st.busy, None, "run {n}: done while still busy");
                    break;
                }
            }
            runner.hold();
            act(&mut host, &commands, Command::RetryFailed);
            assert!(lock_state(&view).busy.is_some(), "run {n}: the retry was refused");
            runner.release();
            until("the retry ends", || series_done(&view) && lock_state(&view).busy.is_none());
            let _ = std::fs::remove_dir_all(&logs);
        }
    }

    fn drivers_request() -> Request {
        Request { addon: "drivers", name: "Drivers", work: Work::Install, items: vec![("inf".into(), "x".into())] }
    }

    #[test]
    fn closing_the_app_while_a_series_waits_for_the_queue_installs_nothing_and_keeps_the_journal_unfinished() {
        let logs = scratch("close-waiting");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        let held = host.context().install_queue().enter(drivers_request(), |_| {}).unwrap();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("waiting", || host.context().install_queue().waiting() == 2);
        assert_eq!(host.close_questions().len(), 1);
        host.close();
        until("the app can close while Drivers still holds the queue", || host.can_close());
        drop(held);
        assert!(installed_ids(&runner).is_empty(), "nothing installed");
        assert!(statuses(&view).iter().all(|(_, s)| matches!(s, Status::NotStarted(_))), "{:?}", statuses(&view));
        assert_eq!(journal(&logs)["finished"], json!(false));
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn turned_off_as_the_queue_frees_up_its_series_never_installs() {
        for n in 0..20 {
            let logs = scratch(&format!("off-race{n}"));
            let (mut host, runner, _view, commands) = host_with_logs(&logs);
            let queue = host.context().install_queue().clone();
            let held = queue.enter(drivers_request(), |_| {}).unwrap();
            act(&mut host, &commands, Command::InstallAll(vec![pkg("A")]));
            until("waiting", || queue.waiting() == 1);
            let release = std::thread::spawn(move || drop(held));
            host.set_enabled(ID, false);
            release.join().unwrap();
            until("off", || {
                host.settle();
                !host.is_stopping(ID)
            });
            assert!(installed_ids(&runner).is_empty(), "run {n}: it installed after being turned off");
            let _ = std::fs::remove_dir_all(&logs);
        }
    }

    #[test]
    fn the_page_shows_who_holds_the_install_queue_and_how_many_items_wait() {
        let logs = scratch("queue-line");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        assert!(!page_text(&mut host).contains("Install queue"), "nothing while the queue is free");
        let held = host.context().install_queue().enter(drivers_request(), |_| {}).unwrap();
        assert!(page_text(&mut host).contains("Install queue: Drivers is installing 1 item."));
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("waiting", || host.context().install_queue().waiting() == 2);
        assert!(page_text(&mut host).contains("Install queue: Drivers is installing 1 item; 2 items wait."));
        exits(&runner, "A", 0);
        exits(&runner, "B", 0);
        drop(held);
        until("the series ends", || series_done(&view));
        assert!(!page_text(&mut host).contains("Install queue"), "its own job is not shown as another's");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_series_that_panics_ends_its_batch_so_the_app_can_close_and_retry() {
        let pm = PackageManager::default();
        let series = Series::new(&pm.state, &Generation::default(), |s: &mut State| &mut s.busy);
        assert!(series.start(Some("Installing 2 packages\u{2026}".into()), true, |run| {
            run.follow().batch = Some(Batch { items: vec![Item::new(pkg("A"), false), Item::new(pkg("B"), false)], ..Batch::default() });
            let _end = EndBatch(run);
            run.follow().batch.as_mut().unwrap().items[0].set(Status::Running);
            panic!("an item broke");
        }));
        until("the series ends", || series_done(&pm.state) && !series.running());
        assert_eq!(lock_state(&pm.state).busy, None);
        assert_eq!(
            statuses(&pm.state),
            named(&[("A", Status::Failed("Package Manager stopped unexpectedly".into())), ("B", Status::NotStarted("Package Manager stopped unexpectedly".into()))])
        );
        assert_eq!(pm.close_question(), None, "closing the app is not held up");
        assert_eq!(batch::failed(&batch_of(&pm.state).unwrap().items).len(), 2, "both can be retried");
    }

    #[test]
    fn the_log_folder_and_a_failed_package_log_open() {
        let (mut host, runner, view, commands, logs) = series_of("folder", &["A"], |runner| exits(runner, "A", 1603));
        // The series' own runs (its check after the install) may still be
        // recorded after it ends: look for the Open among the launches made
        // since the command, not only at the very end.
        let opened_since = |from: usize, path: &std::path::Path| {
            let (wanted, runner) = (Launch::Open(path.to_string_lossy().into_owned()), &runner);
            move || runner.launches().get(from..).is_some_and(|l| l.contains(&wanted))
        };
        let before = runner.launches().len();
        act(&mut host, &commands, Command::OpenLogs);
        until("the log folder opens", opened_since(before, &logs));
        let log = batch_of(&view).unwrap().items[0].log.clone().unwrap();
        let before = runner.launches().len();
        act(&mut host, &commands, Command::OpenItemLog(0));
        until("the failed package's log opens", opened_since(before, &log));
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_series_install_also_notes_the_agreements() {
        let (host, ..) = series_of("seriesnote", &["A"], |runner| exits(runner, "A", 0));
        assert_eq!(host.settings(ID), Some(json!({ "agreementsNoted": true })));
    }

    // ------------------------------------------------------- journal (#115)

    /// The newest journal written beside the logs in `logs` (tests keep
    /// the journals in `logs\journal`).
    fn journal(logs: &std::path::Path) -> serde_json::Value {
        let mut files: Vec<_> = std::fs::read_dir(logs.join("journal")).unwrap().map(|e| e.unwrap().path()).filter(|p| p.extension().is_some_and(|x| x == "json")).collect();
        files.sort();
        serde_json::from_slice(&std::fs::read(files.last().expect("a journal")).unwrap()).unwrap()
    }

    /// Each journal entry's id and status word.
    fn journal_statuses(logs: &std::path::Path) -> Vec<(String, String)> {
        let j = journal(logs);
        j["entries"].as_array().unwrap().iter().map(|e| (e["id"].as_str().unwrap().to_string(), e["status"].as_str().unwrap().to_string())).collect()
    }

    fn pairs(v: &[(&str, &str)]) -> Vec<(String, String)> {
        v.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    #[test]
    fn a_series_writes_its_journal_and_updates_it_at_each_change() {
        let logs = scratch("journal");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("installing A", || installed_ids(&runner).len() == 1);
        until("A noted running", || journal_statuses(&logs) == pairs(&[("Vendor.A", "running"), ("Vendor.B", "pending")]));
        let j = journal(&logs);
        assert_eq!(j["finished"], json!(false));
        assert!(j["run"].is_string() && j["started"].is_u64(), "{j}");
        assert_eq!(j["logs"], json!(logs.to_string_lossy()));
        exits(&runner, "A", 0);
        exits(&runner, "B", 1603);
        runner.release();
        until("the series ends", || series_done(&view));
        let j = journal(&logs);
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.A", "ok"), ("Vendor.B", "failed")]));
        assert_eq!((j["entries"][0]["code"].clone(), j["entries"][1]["code"].clone()), (json!(0), json!(1603)));
        assert_eq!(j["entries"][1]["why"], json!("exit code 1603"));
        let log = batch_of(&view).unwrap().items[1].log.clone().unwrap();
        assert_eq!(j["entries"][1]["log"], json!(log.to_string_lossy()));
        assert_eq!(j["finished"], json!(true), "the series ran to its end");
    }

    #[test]
    fn a_series_stopped_after_the_current_package_leaves_its_journal_unfinished() {
        let logs = scratch("journalstop");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("installing A", || installed_ids(&runner).len() == 1);
        act(&mut host, &commands, Command::StopAfterCurrent);
        exits(&runner, "A", 0);
        runner.release();
        until("the series ends", || series_done(&view));
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.A", "ok"), ("Vendor.B", "not_started")]));
        assert_eq!(journal(&logs)["finished"], json!(false));
    }

    const INTERRUPTED: &str = r#"{ "run": "run-000000000000001-000000", "started": 1700000000, "finished": false, "logs": "C:\\logs", "entries": [
        { "name": "A", "catalog": "winget", "id": "Vendor.A", "version": "1.0", "status": "ok", "code": 0 },
        { "name": "B", "catalog": "winget", "id": "Vendor.B", "version": "1.0", "status": "running" },
        { "name": "C", "catalog": "winget", "id": "Vendor.C", "version": "1.0", "status": "not_started", "why": "the series was stopped" },
        { "name": "D", "catalog": "winget", "id": "Vendor.D", "version": "1.0", "status": "skipped", "why": "already installed" }
    ] }"#;

    /// A host started with an interrupted journal from an earlier run.
    fn host_after_an_interrupted_run(tag: &str) -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>, Commands, std::path::PathBuf) {
        let logs = scratch(tag);
        std::fs::create_dir_all(logs.join("journal")).unwrap();
        std::fs::write(logs.join("journal").join("run-000000000000001-000000.json"), INTERRUPTED).unwrap();
        let (host, runner, view, commands) = host_with_logs(&logs);
        (host, runner, view, commands, logs)
    }

    const OFFER: &str = "The last run was interrupted: 2 of 4 done.";

    #[test]
    fn continuing_an_interrupted_run_installs_only_the_missing_packages() {
        let (mut host, runner, view, commands, logs) = host_after_an_interrupted_run("resume");
        let text = page_text(&mut host);
        assert!(text.contains(OFFER) && text.contains("Continue") && text.contains("Ignore"), "{text}");
        exits(&runner, "B", 0);
        exits(&runner, "C", 0);
        act(&mut host, &commands, Command::ContinueInterrupted);
        until("the resumed series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.B", "Vendor.C"]);
        assert!(!page_text(&mut host).contains(OFFER), "offered once");
        let old: serde_json::Value = serde_json::from_slice(&std::fs::read(logs.join("journal").join("run-000000000000001-000000.json")).unwrap()).unwrap();
        assert_eq!(old["finished"], json!(true), "continued: not offered again");
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.B", "ok"), ("Vendor.C", "ok")]), "the resumed series has its own journal");
    }

    #[test]
    fn an_update_series_is_journalled_and_continued_as_updates() {
        let logs = scratch("resumeupdate");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::UpdateAll(vec![pkg("A"), pkg("B")]));
        until("updating A", || winget_runs(&runner, "upgrade").len() == 1);
        act(&mut host, &commands, Command::StopAfterCurrent);
        exits(&runner, "A", 0);
        runner.release();
        until("the series ends", || series_done(&view));
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.A", "ok"), ("Vendor.B", "not_started")]));
        assert_eq!(journal(&logs)["finished"], json!(false));
        assert_eq!(journal(&logs)["updating"], json!(true));
        let (mut again, runner, view, commands) = host_with_logs(&logs);
        assert!(page_text(&mut again).contains("The last run was interrupted: 1 of 2 done."));
        exits(&runner, "B", 0);
        act(&mut again, &commands, Command::ContinueInterrupted);
        until("the resumed series ends", || series_done(&view));
        assert!(winget_runs(&runner, "install").is_empty(), "continued as updates, not installs");
        assert_eq!(winget_runs(&runner, "upgrade").len(), 1);
        assert!(lock_state(&view).batch.as_ref().is_some_and(|b| b.kind == Kind::Update));
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.B", "ok")]));
    }

    #[test]
    fn an_interrupted_run_is_offered_at_start_in_the_status_bar_without_opening_the_page() {
        let (mut host, _runner, view, commands, _logs) = host_after_an_interrupted_run("startnotice");
        let text = status_bar_text(&mut host);
        assert!(text.contains(INTERRUPTED_NOTICE), "{text}");
        assert!(lock_state(&view).interrupted.is_some());
        act(&mut host, &commands, Command::IgnoreInterrupted);
        assert!(!status_bar_text(&mut host).contains(INTERRUPTED_NOTICE), "gone once answered");
    }

    #[test]
    fn ignoring_an_interrupted_run_installs_nothing_and_closes_it() {
        let (mut host, runner, view, commands, logs) = host_after_an_interrupted_run("ignore");
        act(&mut host, &commands, Command::IgnoreInterrupted);
        assert!(!page_text(&mut host).contains(OFFER));
        assert_eq!(lock_state(&view).batch, None);
        assert!(winget_runs(&runner, "install").is_empty());
        assert_eq!(journal(&logs)["finished"], json!(true));
        let (mut again, ..) = host_with_logs(&logs);
        assert!(!page_text(&mut again).contains(OFFER), "not offered at the next start");
    }

    #[test]
    fn closing_the_app_during_a_series_asks_first_then_finishes_the_current_package_only() {
        let logs = scratch("close");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        assert!(host.close_questions().is_empty(), "nothing runs: the app closes without asking");
        runner.hold();
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        until("installing A", || installed_ids(&runner).len() == 1);
        let questions = host.close_questions();
        assert_eq!(questions.len(), 1, "{questions:?}");
        assert!(questions[0].contains("series of installs is running"), "{questions:?}");
        host.close();
        assert!(!host.can_close(), "the current package finishes first");
        exits(&runner, "A", 0);
        runner.release();
        until("the app can close", || host.can_close());
        assert_eq!(installed_ids(&runner), ["Vendor.A"]);
        assert_eq!(statuses(&view)[0].1, Status::Ok);
        assert!(matches!(statuses(&view)[1].1, Status::NotStarted(_)));
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.A", "ok"), ("Vendor.B", "not_started")]));
        assert_eq!(journal(&logs)["finished"], json!(false), "offered at the next start");
        let mut section = json!({});
        host.write_section(&mut section);
        assert_eq!(section[ID]["enabled"], json!(true), "closing the app does not turn it off");
    }

    #[test]
    fn closing_the_app_right_after_a_series_is_accepted_asks_first() {
        let logs = scratch("close-at-start");
        let gate = Arc::new(Mutex::new(()));
        let mut pm = PackageManager::default();
        pm.logs = logs.clone();
        pm.journals = logs.join("journal");
        pm.job_gate = Some(gate.clone());
        let commands = pm.commands();
        let (mut host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true } }));
        runner.hold();
        // The series' job is started but has not run yet.
        let held = lock_state(&gate);
        act(&mut host, &commands, Command::InstallAll(vec![pkg("A"), pkg("B")]));
        let questions = host.close_questions();
        assert_eq!(questions.len(), 1, "a series accepted asks before the app closes: {questions:?}");
        assert!(questions[0].contains("series of installs is running (0 of 2 done)"), "{questions:?}");
        drop(held);
        until("installing A", || installed_ids(&runner).len() == 1);
        host.close();
        exits(&runner, "A", 0);
        runner.release();
        until("the app can close", || host.can_close());
    }

    // ------------------------------------------------------------ bundles

    fn bundle_of(view: &Mutex<State>) -> Option<BundleView> {
        lock_state(view).bundle.clone()
    }

    /// The rows of the bundle on the page: (name, ticked).
    fn bundle_rows(view: &Mutex<State>) -> Vec<(String, bool)> {
        match bundle_of(view) {
            Some(BundleView::Draft { rows, .. } | BundleView::Opened { rows, .. }) => rows.iter().map(|r| (r.entry.name().to_string(), r.ticked)).collect(),
            _ => Vec::new(),
        }
    }

    /// A host with Package Manager's page open and winget ready (bundles show
    /// under the search).
    fn ready_host(logs: &std::path::Path) -> (AddonHost, Arc<RecordingRunner>, Arc<Mutex<State>>, Commands) {
        let (mut host, runner, view, commands) = host_with_logs(logs);
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        open_page(&mut host);
        until("the catalog update ends", || !matches!(lock_state(&view).catalogs, Catalogs::Updating));
        (host, runner, view, commands)
    }

    const EXPORT_TWO: &str = r#"{ "Sources": [
        { "Packages": [ { "PackageIdentifier": "Git.Git", "Version": "2.47.0" } ], "SourceDetails": { "Name": "winget" } },
        { "Packages": [ { "PackageIdentifier": "9NBLGGH4VVNH", "Version": "1.0" } ], "SourceDetails": { "Name": "msstore" } } ] }"#;

    #[test]
    fn a_bundle_is_made_from_the_installed_apps_with_a_manual_entry_and_saved() {
        let logs = scratch("bundlemake");
        let (mut host, runner, view, commands) = ready_host(&logs);
        runner.prepare_file(When::Arg("export".into()), "--output", 0, EXPORT_TWO);
        act(&mut host, &commands, Command::CreateBundle);
        until("the draft", || matches!(bundle_of(&view), Some(BundleView::Draft { .. })));
        assert_eq!(bundle_rows(&view), [("Git.Git".to_string(), true), ("9NBLGGH4VVNH".to_string(), true)]);
        act(&mut host, &commands, Command::TickBundleEntry(1, false));
        act(&mut host, &commands, Command::AddManualEntry { name: "Tool".into(), url: "https://example.com/tool".into() });
        act(&mut host, &commands, Command::AddManualEntry { name: "Bad".into(), url: "file:///C:/x.exe".into() });
        assert_eq!(bundle_rows(&view).len(), 3, "the bad link is not added");
        assert!(page_text(&mut host).contains("not a web address"));
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("mine.upmbundle");
        act(&mut host, &commands, Command::SaveBundle(path.clone()));
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("saved")).unwrap();
        assert_eq!(saved["schema_version"], json!(1));
        assert_eq!(saved["created"].as_str().map(str::len), Some("2026-11-18T09:30:00Z".len()));
        assert_eq!(saved["entries"], json!([{ "catalog": "winget", "id": "Git.Git" }, { "manual": true, "name": "Tool", "url": "https://example.com/tool" }]));
        assert!(page_text(&mut host).contains("mine.upmbundle"), "the page says where it was saved");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn an_opened_bundle_installs_its_ticked_entries_with_only_the_allowed_arguments() {
        let logs = scratch("bundleopen");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("received.upmbundle");
        let file = json!({
            "schema_version": 1,
            "created": "2026-11-18T09:30:00Z",
            "entries": [
                { "catalog": "winget", "id": "Git.Git", "version": "2.47.0", "scope": "machine", "override": "/S", "args": ["--force"], "custom": "--location C:\\x" },
                { "catalog": "msstore", "id": "9NBLGGH4VVNH", "name": "Store app" },
                { "catalog": "winget", "id": "--override" },
                { "catalog": "scoop", "id": "git" },
                { "manual": true, "name": "Tool", "url": "https://example.com/tool" }
            ]
        });
        std::fs::write(&path, file.to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        let shown = ["Git.Git", "Store app", "git", "Tool"].map(String::from);
        assert_eq!(bundle_rows(&view), [(shown[0].clone(), true), (shown[1].clone(), true), (shown[2].clone(), false), (shown[3].clone(), true)]);
        let text = page_text(&mut host);
        for words in ["Entry 3 was skipped", "catalog not available", "https://example.com/tool", "2.47.0", "machine"] {
            assert!(text.contains(words), "the page shows {words:?}: {text}");
        }
        runner.prepare(When::Arg("Git.Git".into()), 0, "");
        runner.prepare(When::Arg("9NBLGGH4VVNH".into()), 0, "");
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        let batch = batch_of(&view).unwrap();
        let log = |i: usize| batch.items[i].log.clone().unwrap().to_string_lossy().into_owned();
        let fixed = ["--silent", "--disable-interactivity", "--no-upgrade"];
        let agree = ["--accept-package-agreements", "--accept-source-agreements"];
        let mut git = strings(&["install", "--id", "Git.Git", "--exact", "--source", "winget"]);
        git.extend(strings(&fixed).into_iter().chain([String::from("--log"), log(0)]).chain(strings(&agree)).chain(strings(&["--version", "2.47.0", "--scope", "machine"])));
        let mut store = strings(&["install", "--id", "9NBLGGH4VVNH", "--exact", "--source", "msstore"]);
        store.extend(strings(&fixed).into_iter().chain([String::from("--log"), log(1)]).chain(strings(&agree)));
        assert_eq!(winget_runs(&runner, "install"), [git, store]);
        assert_eq!(statuses(&view), named(&[("Git.Git", Status::Ok), ("Store app", Status::Ok), ("Tool", Status::Manual("https://example.com/tool".into()))]));
        assert_eq!(batch.summary(), ["Installed: Git.Git and Store app.", "To install by hand: Tool (https://example.com/tool)."]);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_package_to_install_by_hand_opens_its_site_from_the_series() {
        let logs = scratch("bundlemanual");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("manual.upmbundle");
        let file = json!({ "schema_version": 1, "entries": [{ "manual": true, "name": "Tool", "url": "https://example.com/tool" }] });
        std::fs::write(&path, file.to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        let text = page_text(&mut host);
        assert!(text.contains("Open the site"), "the series row offers the site: {text}");
        act(&mut host, &commands, Command::OpenItemSite(0));
        assert_eq!(runner.launches().last(), Some(&Launch::Open("https://example.com/tool".into())));
        let _ =std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn closing_the_app_during_an_update_series_speaks_of_updates() {
        let logs = scratch("close-updates");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.hold();
        act(&mut host, &commands, Command::UpdateAll(vec![pkg("A"), pkg("B")]));
        until("updating A", || winget_runs(&runner, "upgrade").len() == 1 && batch_of(&view).is_some());
        assert_eq!(
            host.close_questions(),
            ["A series of updates is running (0 of 2 done). Closing lets the current package finish and starts no other; the rest can be continued at the next start."]
        );
        runner.release();
        act(&mut host, &commands, Command::StopAfterCurrent);
        until("the series ends", || series_done(&view));
        let _ = std::fs::remove_dir_all(&logs);
    }

    /// `winget source list` on a PC where the Microsoft Store catalog was
    /// removed.
    const SOURCES_WITHOUT_STORE: &str = "Name   Argument                               Explicit\n\
-----------------------------------------------------------------\n\
winget https://cdn.winget.microsoft.com/cache false\n";

    #[test]
    fn bundle_entries_of_a_catalog_not_active_on_this_pc_are_flagged_and_not_installed() {
        let logs = scratch("bundlesources");
        let (mut host, runner, view, commands) = host_with_logs(&logs);
        runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        runner.prepare(When::Arg("list".into()), 0, SOURCES_WITHOUT_STORE);
        open_page(&mut host);
        until("ready", || matches!(check_of(&view), Check::Known(Readiness::Ready(_))));
        open_page(&mut host);
        until("the catalog update ends", || !matches!(lock_state(&view).catalogs, Catalogs::NotUpdated | Catalogs::Updating));
        assert!(winget_runs(&runner, "source").iter().any(|a| a[1] == "list"), "the catalogs registered on this PC are asked");

        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("store.upmbundle");
        let entries = json!([{ "catalog": "winget", "id": "Vendor.A" }, { "catalog": "msstore", "id": "9NBLGGH4VVNH", "name": "Store app" }]);
        std::fs::write(&path, json!({ "schema_version": 1, "created": "2026-11-18T09:30:00Z", "entries": entries }).to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        assert_eq!(bundle_rows(&view), [("Vendor.A".to_string(), true), ("Store app".to_string(), false)]);
        assert!(page_text(&mut host).contains("catalog not available"));
        act(&mut host, &commands, Command::TickBundleEntry(1, true));
        assert_eq!(bundle_rows(&view)[1], ("Store app".to_string(), false), "it cannot be ticked");
        exits(&runner, "A", 0);
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.A"]);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_bundle_installs_its_machine_wide_entries_first() {
        let logs = scratch("bundleadmin");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("order.upmbundle");
        let entries = json!([
            { "catalog": "winget", "id": "Vendor.A" },
            { "catalog": "winget", "id": "Vendor.B", "scope": "user" },
            { "catalog": "winget", "id": "Vendor.C", "scope": "machine" },
            { "catalog": "winget", "id": "Vendor.D" },
            { "catalog": "winget", "id": "Vendor.E", "scope": "machine" }
        ]);
        std::fs::write(&path, json!({ "schema_version": 1, "created": "2026-11-18T09:30:00Z", "entries": entries }).to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        for name in ["A", "B", "C", "D", "E"] {
            exits(&runner, name, 0);
        }
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        assert_eq!(installed_ids(&runner), ["Vendor.C", "Vendor.E", "Vendor.A", "Vendor.B", "Vendor.D"], "scope machine needs administrator rights: first, so every UAC prompt comes at the start");
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_file_that_is_not_a_bundle_says_so_and_installs_nothing() {
        let logs = scratch("bundlebad");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("notes.json");
        std::fs::write(&path, "{ \"hello\": 1 }").unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        assert!(matches!(bundle_of(&view), Some(BundleView::Failed(ref why)) if why.starts_with("This file is not a bundle")), "{:?}", bundle_of(&view));
        act(&mut host, &commands, Command::InstallBundle);
        std::thread::sleep(Duration::from_millis(30));
        assert!(winget_runs(&runner, "install").is_empty());
        assert_eq!(batch_of(&view), None);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_bundle_series_is_journalled_and_its_resume_installs_with_the_same_pin() {
        let logs = scratch("bundleresume");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("pinned.upmbundle");
        let file = json!({
            "schema_version": 1,
            "created": "2026-11-18T09:30:00Z",
            "entries": [
                { "catalog": "winget", "id": "Vendor.A", "scope": "machine" },
                { "catalog": "winget", "id": "Vendor.G", "version": "2.47.0", "scope": "user" },
                { "manual": true, "name": "Tool", "url": "https://example.com/tool" }
            ]
        });
        std::fs::write(&path, file.to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        runner.hold();
        act(&mut host, &commands, Command::InstallBundle);
        until("installing A", || installed_ids(&runner).len() == 1);
        act(&mut host, &commands, Command::StopAfterCurrent);
        exits(&runner, "A", 0);
        runner.release();
        until("the series ends", || series_done(&view));
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.A", "ok"), ("Vendor.G", "not_started"), ("", "manual")]));
        let j = journal(&logs);
        assert_eq!(j["finished"], json!(false), "cut short: offered at the next start");
        assert_eq!((j["entries"][1]["pin_version"].clone(), j["entries"][1]["scope"].clone()), (json!("2.47.0"), json!("user")));
        assert_eq!((j["entries"][2]["name"].clone(), j["entries"][2]["why"].clone()), (json!("Tool"), json!("https://example.com/tool")));
        let (mut again, runner, view, commands) = host_with_logs(&logs);
        assert!(page_text(&mut again).contains("The last run was interrupted: 1 of 2 done."), "the manual entry is not counted");
        exits(&runner, "G", 0);
        act(&mut again, &commands, Command::ContinueInterrupted);
        until("the resumed series ends", || series_done(&view));
        let runs = winget_runs(&runner, "install");
        assert_eq!(runs.len(), 1, "only the missing package, never the manual entry: {runs:?}");
        assert_eq!(runs[0][2], "Vendor.G");
        assert!(runs[0].ends_with(&strings(&["--version", "2.47.0", "--scope", "user"])), "the same pin: {:?}", runs[0]);
        assert_eq!(batch_of(&view).unwrap().items[0].pin, Pin { version: Some("2.47.0".into()), scope: Some(catalog::Scope::User) });
        assert_eq!(journal_statuses(&logs), pairs(&[("Vendor.G", "ok")]));
        assert_eq!(journal(&logs)["entries"][0]["pin_version"], json!("2.47.0"), "the resumed journal keeps the pin");
        let _ = std::fs::remove_dir_all(&logs);
    }

    /// The winget install runs of the series: each one's id and what follows
    /// its agreement flags (the pin, if any).
    fn pins_of_installs(runner: &RecordingRunner) -> Vec<(String, Vec<String>)> {
        winget_runs(runner, "install")
            .into_iter()
            .map(|args| {
                let after = args.iter().position(|a| a == "--accept-source-agreements").map_or(args.len(), |p| p + 1);
                (args[2].clone(), args[after..].to_vec())
            })
            .collect()
    }

    #[test]
    fn a_winget_export_opens_as_a_bundle_and_installs_with_version_and_scope_only_when_given_and_valid() {
        let logs = scratch("wingetimport");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("exported.json");
        let file = json!({
            "$schema": "https://aka.ms/winget-packages.schema.2.0.json",
            "CreationDate": "2026-11-18T10:00:00.000-00:00",
            "Sources": [
                { "Packages": [
                    { "PackageIdentifier": "Git.Git", "Version": "2.47.0", "Scope": "machine", "InitialOverrideArguments": "/S /D=C:\\x" },
                    { "PackageIdentifier": "Vendor.Bad", "Version": "1.0 --override /S" },
                    { "PackageIdentifier": "Vendor.Odd", "Scope": "everyone" }
                  ],
                  "SourceDetails": { "Argument": "https://cdn.winget.microsoft.com/cache", "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe", "Name": "winget", "Type": "Microsoft.PreIndexed.Package" } },
                { "Packages": [ { "PackageIdentifier": "9NBLGGH4VVNH" } ],
                  "SourceDetails": { "Argument": "https://storeedgefd.dsx.mp.microsoft.com/v9.0", "Identifier": "StoreEdgeFD", "Name": "msstore", "Type": "Microsoft.Rest" } }
            ]
        });
        std::fs::write(&path, file.to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        assert_eq!(bundle_rows(&view), [("Git.Git".to_string(), true), ("9NBLGGH4VVNH".to_string(), true)]);
        let text = page_text(&mut host);
        for words in ["Package 2 was skipped", "Package 3 was skipped", "2.47.0", "machine", "latest"] {
            assert!(text.contains(words), "the page shows {words:?} before starting: {text}");
        }
        runner.prepare(When::Arg("Git.Git".into()), 0, "");
        runner.prepare(When::Arg("9NBLGGH4VVNH".into()), 0, "");
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        assert_eq!(pins_of_installs(&runner), [("Git.Git".to_string(), strings(&["--version", "2.47.0", "--scope", "machine"])), ("9NBLGGH4VVNH".to_string(), vec![])]);
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn version_and_scope_are_edited_per_entry_validated_and_passed_only_when_set() {
        let logs = scratch("bundlepin");
        let (mut host, runner, view, commands) = ready_host(&logs);
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("plain.upmbundle");
        let file = json!({ "schema_version": 1, "created": "", "entries": [
            { "catalog": "winget", "id": "Vendor.A" },
            { "catalog": "winget", "id": "Vendor.B", "version": "9.9", "scope": "machine" },
            { "catalog": "winget", "id": "Vendor.C" }
        ] });
        std::fs::write(&path, file.to_string()).unwrap();
        act(&mut host, &commands, Command::OpenBundle(path));
        act(&mut host, &commands, Command::SetBundleVersion(0, "1.2.3".into()));
        act(&mut host, &commands, Command::SetBundleScope(0, Some(catalog::Scope::User)));
        act(&mut host, &commands, Command::SetBundleVersion(1, String::new()));
        act(&mut host, &commands, Command::SetBundleScope(1, None));
        act(&mut host, &commands, Command::SetBundleVersion(2, "1.0 --override /S".into()));
        let text = page_text(&mut host);
        for words in ["1.2.3", "user", "not allowed"] {
            assert!(text.contains(words), "the page shows {words:?} before starting: {text}");
        }
        for i in ["Vendor.A", "Vendor.B", "Vendor.C"] {
            runner.prepare(When::Arg(i.into()), 0, "");
        }
        act(&mut host, &commands, Command::InstallBundle);
        until("the series ends", || series_done(&view));
        assert_eq!(
            pins_of_installs(&runner),
            [("Vendor.A".to_string(), strings(&["--version", "1.2.3", "--scope", "user"])), ("Vendor.B".to_string(), vec![]), ("Vendor.C".to_string(), vec![])],
            "a cleared pin and a refused version pass nothing"
        );
        let _ = std::fs::remove_dir_all(&logs);
    }

    #[test]
    fn a_bundle_is_exported_for_winget_without_its_manual_entries() {
        let logs = scratch("wingetexport");
        let (mut host, runner, view, commands) = ready_host(&logs);
        runner.prepare_file(When::Arg("export".into()), "--output", 0, EXPORT_TWO);
        act(&mut host, &commands, Command::CreateBundle);
        until("the draft", || matches!(bundle_of(&view), Some(BundleView::Draft { .. })));
        act(&mut host, &commands, Command::AddManualEntry { name: "Tool".into(), url: "https://example.com/tool".into() });
        act(&mut host, &commands, Command::SetBundleScope(0, Some(catalog::Scope::Machine)));
        act(&mut host, &commands, Command::TickBundleEntry(1, false));
        std::fs::create_dir_all(&logs).unwrap();
        let path = logs.join("for-winget.json");
        act(&mut host, &commands, Command::ExportWinget(path.clone()));
        let saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("exported")).unwrap();
        assert_eq!(saved["Sources"].as_array().map(Vec::len), Some(1), "{saved}");
        assert_eq!(saved["Sources"][0]["SourceDetails"]["Name"], json!("winget"));
        assert_eq!(saved["Sources"][0]["Packages"], json!([{ "PackageIdentifier": "Git.Git", "Scope": "machine" }]), "only the ticked packages");
        let text = page_text(&mut host);
        assert!(text.contains("for-winget.json") && text.contains("1 manual entry was left out"), "{text}");
        let _ = std::fs::remove_dir_all(&logs);
    }
}
