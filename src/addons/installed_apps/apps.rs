//! Installed applications: what is installed, how big it is, when it was
//! last opened, whether a newer version exists, and uninstalling, modifying,
//! repairing and updating it.
//!
//! - Classic programs and MSI products: the three uninstall registry views
//!   (64-bit, 32-bit and the current user's).
//! - Store apps (MSIX): Get-AppxPackage in a PowerShell child, display names
//!   from each package's manifest.
//! - Last opened: the shell's UserAssist counters (what Start, the taskbar
//!   and Explorer launched), matched to apps through their folders and the
//!   Start menu shortcuts. UserAssist needs no administrator rights,
//!   unlike SRUM.
//! - Size: the size the app reported, then the size measured in its folder.
//! - Updates: `winget list`, which also says which registry entry or package
//!   each winget package is.
//! - Uninstalling starts the app's own uninstaller (or removes a Store app
//!   for this account), waits for it and everything it started, then checks
//!   that the app is gone.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::addons::plural;
use crate::availability::{unavailable_text, Feature};
use crate::platform::uninstaller::expand_env;
use crate::platform::uninstaller::{not_registered, Target, Uninstaller};
use crate::platform::winget::{describe, last_lines, match_rows, uninstall_summary, update_summary, InstalledEntry};
use crate::platform::winget::{parse_winget_table, WingetRow};
use crate::platform::install_queue::{Request, Work};
use crate::platform::series::{Run, Series};
use crate::platform::{AddonContext, Generation, Waited};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Source {
    #[default]
    Registry,
    Msi,
    Msix,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Registry => "Registry",
            Source::Msi => "MSI",
            Source::Msix => "MSIX",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct App {
    /// Unique: "HKLM\<subkey>", "HKLM32\<subkey>", "HKCU\<subkey>" or
    /// "MSIX\<package full name>".
    pub key: String,
    pub name: String,
    pub publisher: String,
    pub version: String,
    /// "yyyy-mm-dd", or empty.
    pub installed: String,
    /// Bytes the app reported to Windows (EstimatedSize).
    pub size_reported: u64,
    /// Bytes found in its folder, once measured.
    pub size_measured: Option<u64>,
    pub location: String,
    pub icon: String,
    pub uninstall: String,
    pub modify: String,
    pub no_remove: bool,
    pub no_modify: bool,
    pub no_repair: bool,
    /// MSI product code ("{…}") when Windows Installer manages it.
    pub msi_code: String,
    pub source: Source,
    pub user_scope: bool,
    pub bits32: bool,
    pub about_url: String,
    pub help_url: String,
    /// Store apps: package full name and family name.
    pub package: String,
    pub family: String,
    /// Unix seconds of the last time Start, the taskbar or Explorer opened it.
    pub last_used: Option<i64>,
    pub launches: u32,
    /// winget's id for it, when winget recognises it.
    pub winget_id: String,
    /// A newer version winget offers.
    pub available: String,
    pub winget_source: String,
}

impl App {
    pub fn size(&self) -> u64 {
        self.size_measured.unwrap_or(self.size_reported)
    }

    pub fn scope_label(&self) -> &'static str {
        if self.user_scope {
            "Current user"
        } else {
            "Machine"
        }
    }

    /// The side panel's Scope row: [`App::scope_label`], plus "32-bit" when the entry comes from the 32-bit registry
    /// view.
    pub fn scope_text(&self) -> String {
        let mut text = self.scope_label().to_string();
        if !self.user_scope {
            text.push_str(": everyone on this PC");
        }
        if self.bits32 {
            text.push_str(" \u{00b7} 32-bit");
        }
        text
    }

    /// The uninstall registry subkey the entry was read from (e.g.
    /// "Audacity_is1"); empty for Store apps, which have none.
    pub fn registry_key(&self) -> &str {
        if self.source == Source::Msix {
            return "";
        }
        self.key.split_once('\\').map(|(_, sub)| sub).unwrap_or("")
    }

    /// What removes it, as Copy details and the CSV show it: a Store app's
    /// package, else the uninstall command it registered.
    pub fn uninstall_text(&self) -> &str {
        if self.source == Source::Msix {
            &self.package
        } else {
            &self.uninstall
        }
    }

    pub fn has_update(&self) -> bool {
        !self.available.is_empty() && !self.winget_id.is_empty()
    }

    /// Why it can't be uninstalled, if it can't.
    pub fn uninstall_blocker(&self) -> Option<String> {
        if self.no_remove {
            return Some(format!("Windows marks {} as an application that cannot be uninstalled.", self.name));
        }
        if self.source != Source::Msix && self.uninstall.trim().is_empty() && self.msi_code.is_empty() {
            return Some(not_registered(&self.name));
        }
        None
    }

    /// The app as an item of the install queue: (catalog, id).
    pub fn queue_item(&self) -> (String, String) {
        if self.source == Source::Msix {
            ("msix".into(), self.package.clone())
        } else {
            ("registry".into(), self.key.clone())
        }
    }

    /// What the platform's uninstaller removes for this app.
    pub fn uninstall_target(&self) -> Target {
        if self.source == Source::Msix {
            Target::Msix { package: self.package.clone() }
        } else {
            Target::Registry { key: self.key.clone(), command: self.uninstall.clone(), msi_code: self.msi_code.clone() }
        }
    }

    pub fn can_modify(&self) -> bool {
        self.source != Source::Msix && !self.no_modify && (!self.modify.trim().is_empty() || !self.msi_code.is_empty())
    }

    pub fn can_repair(&self) -> bool {
        !self.msi_code.is_empty() && !self.no_repair
    }

    /// The folder its files live in, when it is specific to this app.
    pub fn folder(&self) -> Option<String> {
        let loc = clean_path(&self.location);
        if specific_folder(&loc) {
            return Some(loc);
        }
        let icon = clean_path(&self.icon);
        let dir = parent_dir(&icon);
        specific_folder(&dir).then_some(dir)
    }
}

/// Everything the Installed Apps page shows, filled by background threads.
#[derive(Default)]
pub struct State {
    pub apps: Vec<App>,
    pub listing: bool,
    pub listed_at: String,
    /// (measured, to measure) while folder sizes are being measured.
    pub measuring: Option<(usize, usize)>,
    /// None until winget has been asked.
    pub winget: Option<Vec<WingetRow>>,
    pub checked_at: String,
    /// The user pressed Check and winget answered (in this session).
    pub checked_by_hand: bool,
    /// What is running now ("Checking winget…").
    pub busy: Option<String>,
    /// An uninstaller is running and the page offers "Stop waiting".
    pub waiting: bool,
    /// Last outcome; the flag marks an error.
    pub message: Option<(String, bool)>,
    /// Output of the last winget run.
    pub output: String,
}

#[derive(Clone)]
pub struct Hub {
    st: Arc<Mutex<State>>,
    /// Every program (winget, PowerShell, setups) starts through the
    /// platform's executor.
    ctx: AddonContext,
    /// The platform's uninstaller, also for "Change" and "Repair" setups
    /// (its "Stop waiting" covers them).
    uninstaller: Uninstaller,
    /// Bumped by every refresh, so an older measuring pass stops.
    pass: Arc<AtomicU64>,
    sizes: Arc<Mutex<HashMap<String, u64>>>,
    /// Runs checks, updates, uninstalls and setups one at a time. Stopped
    /// once by [`Hub::stop`]: from then on its work stops early and
    /// publishes nothing; updates, uninstalls and setups are not cut short.
    series: Series<State>,
    /// Measures an app's folder ([`folder_size`]; a fake in tests).
    measure_folder: Arc<MeasureFolder>,
    /// Reads the uninstall registry views (a fake list in tests).
    list_registry: Arc<ListRegistry>,
}

/// The apps the uninstall registry views list.
type ListRegistry = dyn Fn() -> Vec<App> + Send + Sync;

/// Bytes in a folder, skipping the nested folders given.
type MeasureFolder = dyn Fn(&std::path::Path, &[String]) -> u64 + Send + Sync;

impl Hub {
    pub fn new(ctx: AddonContext) -> Self {
        let st = Arc::new(Mutex::new(State::default()));
        let series = Series::new(&st, &Generation::default(), |s: &mut State| &mut s.busy);
        Hub {
            st,
            uninstaller: Uninstaller::new(ctx.clone()),
            ctx,
            pass: Default::default(),
            sizes: Default::default(),
            series,
            measure_folder: Arc::new(folder_size),
            list_registry: Arc::new(imp::list_registry),
        }
    }

    /// The hub with folders measured by `measure` (a test's slow fake).
    #[cfg(test)]
    pub fn with_measure(self, measure: impl Fn(&std::path::Path, &[String]) -> u64 + Send + Sync + 'static) -> Self {
        Hub { measure_folder: Arc::new(measure), ..self }
    }

    /// The hub with the registry read as `list` (a test's fake apps).
    #[cfg(test)]
    pub fn with_registry(self, list: impl Fn() -> Vec<App> + Send + Sync + 'static) -> Self {
        Hub { list_registry: Arc::new(list), ..self }
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state to publish into, unless the hub was stopped (checked under
    /// the lock, so nothing lands after `stop`).
    fn live(&self) -> Option<MutexGuard<'_, State>> {
        let st = self.lock();
        (!self.stopped()).then_some(st)
    }

    /// The state an update, uninstall or setup reports into: also after
    /// [`Hub::stop`], so the page that stays open while the add-on waits to
    /// turn off can follow the operation to its end.
    fn follow(&self) -> MutexGuard<'_, State> {
        self.lock()
    }

    /// The add-on was turned off: stop working.
    fn stopped(&self) -> bool {
        self.series.stopped()
    }

    /// Stops what can be stopped (reading the list, measuring, asking
    /// winget, the next app of a batch). With nothing else running the
    /// state is forgotten; a running update, uninstall or setup goes on and
    /// keeps reporting its progress and outcome (see [`Hub::follow`]).
    pub fn stop(&self) {
        self.pass.fetch_add(1, Ordering::Relaxed);
        self.series.stop(|st| {
            st.listing = false;
            st.measuring = None;
        });
    }

    /// An update, uninstall or setup is running.
    pub fn running(&self) -> bool {
        self.series.running()
    }

    /// Runs `job` in a thread unless another job runs (see
    /// [`Series::start`]). An uninterruptible job counts in
    /// [`Hub::running`] until it ends.
    fn spawn(&self, busy: String, uninterruptible: bool, job: impl FnOnce(&Hub, &Run<State>) + Send + 'static) -> bool {
        let hub = self.clone();
        self.series.start(Some(busy), uninterruptible, move |run| job(&hub, run))
    }

    /// Re-reads the list after an operation (unless stopped meanwhile).
    fn relist(&self) {
        if self.stopped() {
            return;
        }
        let mut apps = (self.list_registry)();
        let (msix, links) = imp::list_msix_and_links(&self.ctx);
        apps.extend(msix);
        sort_apps(&mut apps);
        apply_usage(&mut apps, &imp::user_assist(), &links);
        self.publish(&mut apps, true);
    }

    /// Another add-on's install-queue job changed the installed software:
    /// re-reads the list, if it was read already (else the page reads it
    /// when it opens).
    pub fn packages_changed(&self) {
        if self.lock().listed_at.is_empty() {
            return;
        }
        self.refresh();
    }

    /// Stops waiting for an uninstaller (it keeps running).
    pub fn stop_waiting(&self) {
        self.uninstaller.stop_waiting();
    }

    /// Re-reads the list (registry at once, then Store apps, last use and
    /// folder sizes).
    pub fn refresh(&self) {
        {
            let Some(mut st) = self.live() else { return };
            if st.listing {
                return;
            }
            // A system without the listing says so; it never reads as "no apps".
            // The `true` of a message marks an error.
            if let Some(why) = unavailable_text(Feature::UninstallRegistry) {
                st.apps.clear();
                st.listed_at = crate::platform::clock_now();
                st.message = Some((why, true));
                return;
            }
            st.listing = true;
        }
        let hub = self.clone();
        std::thread::spawn(move || hub.refresh_now());
    }

    fn refresh_now(&self) {
        let generation = self.pass.fetch_add(1, Ordering::Relaxed) + 1;
        let mut apps = (self.list_registry)();
        self.publish(&mut apps, false);
        if self.stopped() {
            return;
        }
        let (msix, links) = imp::list_msix_and_links(&self.ctx);
        apps.extend(msix);
        sort_apps(&mut apps);
        let usage = imp::user_assist();
        apply_usage(&mut apps, &usage, &links);
        self.publish(&mut apps, true);
        self.measure(generation);
    }

    /// Puts a fresh list in the state, keeping what is known about each app.
    fn publish(&self, apps: &mut [App], done: bool) {
        {
            let sizes = self.sizes.lock().unwrap_or_else(|e| e.into_inner());
            for a in apps.iter_mut() {
                if let Some(dir) = a.folder() {
                    a.size_measured = sizes.get(&dir.to_lowercase()).copied();
                }
            }
        }
        let Some(mut st) = self.live() else { return };
        let old: HashMap<String, Option<i64>> = st.apps.iter().map(|a| (a.key.clone(), a.last_used)).collect();
        for a in apps.iter_mut() {
            if a.last_used.is_none() {
                a.last_used = old.get(&a.key).copied().flatten();
            }
        }
        if let Some(rows) = &st.winget {
            match_winget(apps, rows);
        }
        st.apps = apps.to_vec();
        if done {
            st.listing = false;
            st.listed_at = crate::platform::clock_now();
        }
    }

    /// Measures app folders not measured yet, one at a time.
    fn measure(&self, generation: u64) {
        let (todo, all): (Vec<String>, Vec<String>) = {
            let st = self.lock();
            let sizes = self.sizes.lock().unwrap_or_else(|e| e.into_inner());
            let all: Vec<String> = st.apps.iter().filter_map(|a| a.folder()).collect();
            let mut v: Vec<String> = all.iter().filter(|d| !sizes.contains_key(&d.to_lowercase())).cloned().collect();
            v.sort_by_key(|d| d.to_lowercase());
            v.dedup_by_key(|d| d.to_lowercase());
            (v, all)
        };
        let total = todo.len();
        for (i, dir) in todo.into_iter().enumerate() {
            if self.pass.load(Ordering::Relaxed) != generation {
                return;
            }
            match self.live() {
                Some(mut st) => st.measuring = Some((i, total)),
                None => return,
            }
            let bytes = (self.measure_folder)(std::path::Path::new(&dir), &nested_folders(&dir, &all));
            let key = dir.to_lowercase();
            self.sizes.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone(), bytes);
            let Some(mut st) = self.live() else { return };
            for a in st.apps.iter_mut() {
                if a.folder().is_some_and(|d| d.to_lowercase() == key) {
                    a.size_measured = Some(bytes);
                }
            }
        }
        if self.pass.load(Ordering::Relaxed) == generation {
            if let Some(mut st) = self.live() {
                st.measuring = None;
            }
        }
    }

    /// Asks winget which apps have a newer version (a few seconds).
    /// The user pressed Check: its outcome can be shown on Summary.
    pub fn check_updates(&self) {
        self.check(true);
    }

    /// The check "Check for updates when the page opens" asks for: its
    /// outcome stays on the page.
    pub fn check_updates_on_open(&self) {
        self.check(false);
    }

    fn check(&self, by_hand: bool) {
        self.spawn("Asking winget for newer versions\u{2026}".into(), false, move |hub, _| hub.check_now(by_hand));
    }

    fn check_now(&self, by_hand: bool) {
        let result = self.ctx.winget(&["list", "--accept-source-agreements", "--disable-interactivity"]);
        let Some(mut st) = self.live() else { return };
        match result {
            Ok((_, text)) => {
                let rows = parse_winget_table(&text);
                if rows.is_empty() {
                    st.message = Some(("winget did not return a list of apps. See the output below.".into(), true));
                    st.output = text;
                    return;
                }
                let State { apps, .. } = &mut *st;
                match_winget(apps, &rows);
                st.winget = Some(rows);
                st.checked_at = crate::platform::clock_now();
                st.checked_by_hand |= by_hand;
                st.message = None;
            }
            Err(e) => st.message = Some((e, true)),
        }
    }

    /// Updates the given apps with winget, one after another.
    pub fn update(&self, keys: Vec<String>) {
        let targets: Vec<App> = {
            let st = self.lock();
            keys.iter().filter_map(|k| st.apps.iter().find(|a| &a.key == k && a.has_update()).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let n = targets.len();
        self.spawn(format!("Updating {} \u{2014} installers may ask for permission\u{2026}", plural(n, "app")), true, move |hub, run| {
            let label = |a: &App, i: usize| {
                if n == 1 {
                    format!("Updating {} to {} \u{2014} its installer may ask for permission\u{2026}", a.name, a.available)
                } else {
                    format!("Updating {} ({} of {n}) \u{2014} installers may ask for permission\u{2026}", a.name, i + 1)
                }
            };
            let items = targets.iter().map(|a| ("winget".to_string(), a.winget_id.clone())).collect();
            let Some(turn) = run.queue(hub.ctx.install_queue(), queue_request(Work::Update, items)) else { return };
            // Turned off: no further app of the batch starts.
            let outcomes = run.each(&targets, label, |a| {
                let mut args = vec!["upgrade", "--id", a.winget_id.as_str(), "--exact", "--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"];
                if !a.winget_source.is_empty() {
                    args.push("--source");
                    args.push(a.winget_source.as_str());
                }
                hub.ctx.winget(&args)
            });
            drop(turn);
            let mut ok = Vec::new();
            let mut failed = Vec::new();
            let mut log = String::new();
            for (a, outcome) in targets.iter().zip(outcomes) {
                match outcome {
                    Some(Ok((0, text))) => {
                        ok.push(a.name.clone());
                        log.push_str(&format!("== {}\n{}\n", a.name, last_lines(&text, 12)));
                    }
                    Some(Ok((code, text))) => {
                        failed.push(format!("{} ({})", a.name, describe(code)));
                        log.push_str(&format!("== {}\n{}\n", a.name, last_lines(&text, 12)));
                    }
                    Some(Err(e)) => failed.push(format!("{} ({e})", a.name)),
                    None => {}
                }
            }
            {
                let mut st = hub.follow();
                st.output = log;
                st.message = Some(update_summary(&ok, &failed));
            }
            if hub.stopped() {
                return;
            }
            hub.relist();
            let message = hub.lock().message.clone();
            hub.check_now(false);
            if let Some(mut st) = hub.live() {
                st.message = message;
            }
        });
    }

    /// Uninstalls the given apps one after another with their own
    /// uninstallers (Store apps: removed for this account).
    pub fn uninstall(&self, keys: Vec<String>) {
        let targets: Vec<App> = {
            let st = self.lock();
            keys.iter().filter_map(|k| st.apps.iter().find(|a| &a.key == k).cloned()).collect()
        };
        if targets.is_empty() {
            return;
        }
        let n = targets.len();
        self.uninstaller.wait_again();
        self.spawn(format!("Uninstalling {}\u{2026}", plural(n, "app")), true, move |hub, run| {
            let label = |a: &App, i: usize| {
                if n == 1 {
                    format!("Waiting for the uninstaller of {} \u{2014} answer its questions in its own window\u{2026}", a.name)
                } else {
                    format!("Uninstalling {} ({} of {n}) \u{2014} answer its questions in its own window\u{2026}", a.name, i + 1)
                }
            };
            let items = targets.iter().map(App::queue_item).collect();
            let Some(turn) = run.queue(hub.ctx.install_queue(), queue_request(Work::Uninstall, items)) else { return };
            // Turned off: no further app of the batch starts.
            let outcomes = run.each(&targets, label, |a| uninstall_one(hub, a));
            drop(turn);
            let mut gone = Vec::new();
            let mut notes = Vec::new();
            for (a, outcome) in targets.iter().zip(outcomes) {
                match outcome {
                    Some(Ok(())) => gone.push(a.name.clone()),
                    Some(Err(e)) => notes.push(e),
                    None => {}
                }
            }
            {
                let mut st = hub.follow();
                st.waiting = false;
                st.message = Some(uninstall_summary(n, &gone, &notes));
            }
            hub.relist();
        });
    }

    /// Starts the app's "Change" (modify) setup, or its repair, through
    /// the app's install queue, waits for it, then re-reads the list.
    pub fn modify(&self, key: String, repair: bool) {
        let Some(a) = self.lock().apps.iter().find(|a| a.key == key).cloned() else { return };
        let what = if repair { "repair" } else { "setup" };
        self.uninstaller.wait_again();
        self.spawn(format!("Waiting for the {what} of {} to finish\u{2026}", a.name), true, move |hub, run| {
            let work = if repair { Work::Repair } else { Work::Change };
            let Some(turn) = run.queue(hub.ctx.install_queue(), queue_request(work, vec![a.queue_item()])) else { return };
            let cmd = if repair {
                format!("MsiExec.exe /fa{}", a.msi_code)
            } else if !a.modify.trim().is_empty() {
                a.modify.clone()
            } else {
                format!("MsiExec.exe /I{}", a.msi_code)
            };
            hub.follow().waiting = true;
            let result = hub.uninstaller.start_and_wait(&cmd).map_err(|e| e.replace("%1", &a.name));
            drop(turn);
            {
                let mut st = hub.follow();
                st.waiting = false;
                st.message = Some(match result {
                    Ok(Waited::Finished(_)) => (format!("The {what} of {} closed.", a.name), false),
                    Ok(Waited::Stopped) => (format!("Stopped waiting; the {what} of {} may still be open.", a.name), false),
                    Ok(Waited::TimedOut) => (format!("The {what} of {} was still running after an hour.", a.name), true),
                    Err(e) => (e, true),
                });
            }
            hub.relist();
        });
    }
}

/// This add-on's request for the app's install queue.
fn queue_request(work: Work, items: Vec<(String, String)>) -> Request {
    Request { addon: super::ID, name: super::NAME, work, items }
}

/// Uninstalls one app through the platform's uninstaller, telling the
/// page while its uninstaller is waited for.
fn uninstall_one(hub: &Hub, a: &App) -> Result<(), String> {
    if let Some(why) = a.uninstall_blocker() {
        return Err(why);
    }
    hub.uninstaller.uninstall(&a.name, &a.uninstall_target(), |waiting| hub.follow().waiting = waiting)
}

fn sort_apps(apps: &mut [App]) {
    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.key.cmp(&b.key)));
}

// ------------------------------------------------------- winget matching

/// Tells each app what winget knows about it.
pub fn match_winget(apps: &mut [App], rows: &[WingetRow]) {
    for a in apps.iter_mut() {
        a.winget_id.clear();
        a.available.clear();
        a.winget_source.clear();
    }
    let listed: Vec<InstalledEntry> = apps.iter().map(|a| InstalledEntry { key: &a.key, name: &a.name, version: &a.version }).collect();
    let found = match_rows(&listed, rows);
    for (r, idx) in rows.iter().zip(found) {
        let Some(i) = idx else { continue };
        let a = &mut apps[i];
        // A Store app whose manifest name could not be read: winget's name.
        if a.source == Source::Msix && !r.name.is_empty() && !a.name.contains(' ') && a.name.contains('.') {
            a.name = r.name.clone();
        }
        if r.id.starts_with("ARP\\") || r.id.starts_with("MSIX\\") {
            continue;
        }
        a.winget_id = r.id.clone();
        a.winget_source = r.source.clone();
        a.available = r.available.clone();
    }
}

// ----------------------------------------------------------------- usage

/// A Start menu shortcut: where it points and the app id the shell gives it.
#[derive(Clone, Debug, Default)]
pub struct Link {
    pub path: String,
    pub name: String,
    pub target: String,
    pub aumid: String,
}

/// A UserAssist entry: what was opened, when (unix seconds), how often.
#[derive(Clone, Debug)]
pub struct Opened {
    pub what: String,
    pub when: i64,
    pub count: u32,
}

/// Gives each app the latest time any of its programs was opened.
pub fn apply_usage(apps: &mut [App], usage: &[Opened], links: &[Link]) {
    let by_path: HashMap<String, &Link> = links.iter().map(|l| (l.path.to_lowercase(), l)).collect();
    let mut by_aumid: HashMap<String, Vec<&Link>> = HashMap::new();
    for l in links.iter().filter(|l| !l.aumid.is_empty()) {
        by_aumid.entry(l.aumid.to_lowercase()).or_default().push(l);
    }
    // Every entry as (target path or package family, shortcut name).
    let mut seen: Vec<(String, String, i64, u32)> = Vec::new();
    for o in usage {
        let w = o.what.to_lowercase();
        if w.ends_with(".lnk") {
            if let Some(l) = by_path.get(&w) {
                seen.push((l.target.to_lowercase(), l.name.to_lowercase(), o.when, o.count));
            }
        } else if !w.contains('\\') {
            if let Some((family, _)) = w.split_once('!') {
                seen.push((format!("msix:{family}"), String::new(), o.when, o.count));
            } else if let Some(ls) = by_aumid.get(&w) {
                for l in ls {
                    seen.push((l.target.to_lowercase(), l.name.to_lowercase(), o.when, o.count));
                }
            }
        } else {
            seen.push((w, String::new(), o.when, o.count));
        }
    }
    for a in apps.iter_mut() {
        let family = format!("msix:{}", a.family.to_lowercase());
        let dir = a.folder().map(|d| format!("{}\\", d.trim_end_matches('\\').to_lowercase()));
        let name = a.name.trim().to_lowercase();
        let mut best: Option<i64> = None;
        let mut count = 0u32;
        for (target, link_name, when, n) in &seen {
            let hit = if a.source == Source::Msix {
                !a.family.is_empty() && *target == family
            } else {
                dir.as_ref().is_some_and(|d| target.starts_with(d.as_str()))
                    || (!link_name.is_empty() && (*link_name == name || name.strip_prefix(link_name.as_str()).is_some_and(|r| r.starts_with(' ') && r.trim_start().starts_with(|c: char| c.is_ascii_digit() || c == '('))))
            };
            if hit {
                best = Some(best.map_or(*when, |b| b.max(*when)));
                count += n;
            }
        }
        if best.is_some() {
            a.last_used = best;
            a.launches = count;
        }
    }
}

/// ROT13, the shell's way of hiding UserAssist names.
pub fn rot13(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => (((c as u8 - b'a' + 13) % 26) + b'a') as char,
            'A'..='Z' => (((c as u8 - b'A' + 13) % 26) + b'A') as char,
            _ => c,
        })
        .collect()
}

/// Known-folder ids UserAssist puts in front of paths.
fn known_folder(guid: &str) -> Option<String> {
    let env = |k: &str| std::env::var(k).ok();
    let windir = env("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    Some(match guid.to_uppercase().as_str() {
        "{6D809377-6AF0-444B-8957-A3773F02200E}" => env("ProgramW6432").or_else(|| env("ProgramFiles"))?,
        "{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}" => env("ProgramFiles(x86)")?,
        "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}" => format!(r"{windir}\System32"),
        "{D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27}" => format!(r"{windir}\SysWOW64"),
        "{F38BF404-1D43-42F2-9305-67DE0B28FC23}" => windir,
        "{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}" => env("LOCALAPPDATA")?,
        "{3EB685DB-65F9-4CF6-A03A-E3EF65729F3D}" => env("APPDATA")?,
        "{A77F5D77-2E2B-44C3-A6A2-ABA601054A51}" => format!(r"{}\Microsoft\Windows\Start Menu\Programs", env("APPDATA")?),
        "{0139D44E-6AFE-49F2-8690-3DAFCAE6FFB8}" => format!(r"{}\Microsoft\Windows\Start Menu\Programs", env("ProgramData")?),
        "{9E3995AB-1F9C-4F13-B827-48B24B6C7174}" => format!(r"{}\Microsoft\Internet Explorer\Quick Launch\User Pinned", env("APPDATA")?),
        "{62AB5D82-FDC1-4DC3-A9DD-070D1D495D97}" => env("ProgramData")?,
        "{5E6C858F-0E22-4760-9AFE-EA3317B67173}" => env("USERPROFILE")?,
        _ => return None,
    })
}

/// "{6D809377-…}\Git\git-bash.exe" → "C:\Program Files\Git\git-bash.exe".
pub fn expand_known_folder(p: &str) -> String {
    if p.starts_with('{') {
        if let Some(end) = p.find('}') {
            if let Some(base) = known_folder(&p[..=end]) {
                return format!("{base}{}", &p[end + 1..]);
            }
        }
    }
    p.to_string()
}

// ------------------------------------------------------------ paths, size

/// Strips quotes and an icon index: "\"C:\x\a.exe\",0" → "C:\x\a.exe".
pub fn clean_path(p: &str) -> String {
    let mut s = p.trim().to_string();
    if let Some(stripped) = s.strip_prefix('"') {
        s = stripped.split('"').next().unwrap_or("").to_string();
    } else if let Some(i) = s.rfind(',') {
        if s[i + 1..].trim().trim_start_matches('-').chars().all(|c| c.is_ascii_digit()) {
            s.truncate(i);
        }
    }
    expand_env(s.trim())
}


fn parent_dir(p: &str) -> String {
    match p.rfind('\\') {
        Some(i) if p.to_lowercase().ends_with(".exe") || p.to_lowercase().ends_with(".ico") || p.to_lowercase().ends_with(".dll") => p[..i].to_string(),
        _ => String::new(),
    }
}

/// A folder that belongs to one app: not a drive, not Windows, not a
/// shared root like Program Files itself.
pub fn specific_folder(dir: &str) -> bool {
    let d = dir.trim_end_matches('\\').to_lowercase();
    if d.len() < 4 || !d.contains(":\\") {
        return false;
    }
    let env = |k: &str| std::env::var(k).map(|v| v.trim_end_matches('\\').to_lowercase()).unwrap_or_default();
    let windir = env("WINDIR");
    if !windir.is_empty() && (d == windir || d.starts_with(&format!("{windir}\\"))) {
        return false;
    }
    let roots = [
        env("ProgramW6432"),
        env("ProgramFiles"),
        env("ProgramFiles(x86)"),
        env("ProgramData"),
        env("LOCALAPPDATA"),
        env("APPDATA"),
        env("USERPROFILE"),
        format!("{}\\programs", env("LOCALAPPDATA")),
        format!("{}\\common files", env("ProgramFiles")),
        format!("{}\\common files", env("ProgramFiles(x86)")),
        format!("{}\\package cache", env("ProgramData")),
    ];
    if roots.iter().any(|r| !r.is_empty() && (d == *r)) {
        return false;
    }
    if d.contains("\\package cache\\") || d.contains("\\installer\\") || d.contains("\\installshield installation information") {
        return false;
    }
    // "C:\Games" is fine, "C:" is not.
    d.matches('\\').count() >= 1 && std::path::Path::new(dir).is_dir()
}

/// Other apps' folders inside `dir` (GOG Galaxy\Games\…): counted for
/// those apps, not twice. Lower case, no trailing backslash.
pub fn nested_folders(dir: &str, all: &[String]) -> Vec<String> {
    let base = format!("{}\\", dir.trim_end_matches('\\').to_lowercase());
    let mut v: Vec<String> = all.iter().map(|d| d.trim_end_matches('\\').to_lowercase()).filter(|d| d.starts_with(&base)).collect();
    v.sort();
    v.dedup();
    v
}

/// Total size of the files in a folder, not following links and skipping
/// the folders in `skip`; stops counting after a very large number of files.
pub fn folder_size(dir: &std::path::Path, skip: &[String]) -> u64 {
    let mut total = 0u64;
    let mut files = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            #[cfg(windows)]
            {
                use std::os::windows::fs::MetadataExt;
                if m.file_attributes() & 0x400 != 0 {
                    continue; // reparse point: a link to somewhere else
                }
            }
            if m.is_dir() {
                let p = e.path();
                if !skip.is_empty() && skip.contains(&p.to_string_lossy().trim_end_matches('\\').to_lowercase()) {
                    continue;
                }
                stack.push(p);
            } else {
                total += m.len();
                files += 1;
                if files > 600_000 {
                    return total;
                }
            }
        }
    }
    total
}

/// Unix seconds → "yyyy-mm-dd".
pub fn ymd(secs: i64) -> String {
    let (y, m, d) = civil(secs.div_euclid(86_400));
    format!("{y:04}-{m:02}-{d:02}")
}

pub fn civil(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
}

// --------------------------------------------------------------- scripts

/// Store apps (with their display names from the manifest) and the Start
/// menu shortcuts (target and app id), in one PowerShell run.
const INVENTORY_SCRIPT: &str = r#"param([string]$Out)
$ErrorActionPreference = 'Continue'
$res = [ordered]@{ ok = $false; error = ''; packages = @(); links = @() }
try {
  $pk = @()
  foreach ($p in Get-AppxPackage -PackageTypeFilter Main) {
    if ($p.IsFramework -or [string]$p.SignatureKind -eq 'System' -or -not $p.InstallLocation) { continue }
    $mf = Join-Path $p.InstallLocation 'AppxManifest.xml'
    if (-not (Test-Path -LiteralPath $mf)) { continue }
    try { [xml]$m = Get-Content -LiteralPath $mf -Raw -Encoding UTF8 } catch { continue }
    $apps = @($m.Package.Applications.Application)
    $listed = $false
    foreach ($a in $apps) {
      if ($a -eq $null) { continue }
      $ve = $a.VisualElements
      if ($ve -eq $null -or [string]$ve.AppListEntry -ne 'none') { $listed = $true }
    }
    if (-not $listed) { continue }
    $pk += [ordered]@{
      Name = [string]$m.Package.Properties.DisplayName; Publisher = [string]$m.Package.Properties.PublisherDisplayName
      Id = [string]$p.Name; FullName = [string]$p.PackageFullName; Family = [string]$p.PackageFamilyName
      Version = [string]$p.Version; Location = [string]$p.InstallLocation; NonRemovable = [bool]$p.NonRemovable
      Kind = [string]$p.SignatureKind; Arch = [string]$p.Architecture
    }
  }
  $res.packages = $pk
  $sh = New-Object -ComObject WScript.Shell
  $shell = New-Object -ComObject Shell.Application
  $ln = @()
  foreach ($d in @([Environment]::GetFolderPath('CommonPrograms'), [Environment]::GetFolderPath('Programs'))) {
    if (-not $d -or -not (Test-Path -LiteralPath $d)) { continue }
    foreach ($f in Get-ChildItem -LiteralPath $d -Recurse -Filter *.lnk -ErrorAction SilentlyContinue) {
      $t = ''; $id = ''
      try { $t = [string]$sh.CreateShortcut($f.FullName).TargetPath } catch {}
      try { $id = [string]$shell.Namespace($f.DirectoryName).ParseName($f.Name).ExtendedProperty('System.AppUserModel.ID') } catch {}
      $ln += [ordered]@{ Path = $f.FullName; Name = $f.BaseName; Target = $t; Aumid = $id }
    }
  }
  $res.links = $ln
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res -Depth 5 -Compress | Out-File -FilePath $Out -Encoding utf8
"#;

// -------------------------------------------------------------- platform

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn list_registry() -> Vec<App> {
        Vec::new()
    }
    pub fn list_msix_and_links(_: &AddonContext) -> (Vec<App>, Vec<Link>) {
        (Vec::new(), Vec::new())
    }
    pub fn user_assist() -> Vec<Opened> {
        Vec::new()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};
    use winreg::enums::*;
    use winreg::RegKey;

    const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
    const UNINSTALL32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    fn views() -> [(&'static str, RegKey, &'static str); 3] {
        [
            ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL),
            ("HKLM32", RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL32),
            ("HKCU", RegKey::predef(HKEY_CURRENT_USER), UNINSTALL),
        ]
    }

    pub fn list_registry() -> Vec<App> {
        let mut out: Vec<App> = Vec::new();
        for (hive, root, path) in views() {
            let Ok(root) = root.open_subkey(path) else { continue };
            for sub in root.enum_keys().flatten() {
                let Ok(k) = root.open_subkey(&sub) else { continue };
                let s = |n: &str| k.get_value::<String, _>(n).unwrap_or_default().trim().to_string();
                let d = |n: &str| k.get_value::<u32, _>(n).unwrap_or(0);
                let name = s("DisplayName");
                // Hidden the way Settings hides them: no name, parts of
                // Windows or of another product, and updates.
                if name.is_empty() || d("SystemComponent") == 1 || !s("ParentKeyName").is_empty() {
                    continue;
                }
                let release = s("ReleaseType").to_lowercase();
                if release.contains("update") || release.contains("hotfix") {
                    continue;
                }
                let msi = d("WindowsInstaller") == 1 && sub.starts_with('{');
                let mut installed = s("InstallDate");
                if installed.len() == 8 && installed.chars().all(|c| c.is_ascii_digit()) {
                    installed = format!("{}-{}-{}", &installed[0..4], &installed[4..6], &installed[6..8]);
                } else {
                    // When the app did not say, the time its entry was written.
                    installed = k
                        .query_info()
                        .ok()
                        .map(|i| {
                            let ft = ((i.last_write_time.dwHighDateTime as u64) << 32) | i.last_write_time.dwLowDateTime as u64;
                            ymd((ft / 10_000_000) as i64 - 11_644_473_600)
                        })
                        .unwrap_or_default();
                }
                let app = App {
                    key: format!("{hive}\\{sub}"),
                    publisher: s("Publisher"),
                    version: s("DisplayVersion"),
                    installed,
                    size_reported: d("EstimatedSize") as u64 * 1024,
                    location: s("InstallLocation"),
                    icon: s("DisplayIcon"),
                    uninstall: s("UninstallString"),
                    modify: s("ModifyPath"),
                    no_remove: d("NoRemove") == 1,
                    no_modify: d("NoModify") == 1,
                    no_repair: d("NoRepair") == 1,
                    msi_code: if msi { sub.clone() } else { String::new() },
                    source: if msi { Source::Msi } else { Source::Registry },
                    user_scope: hive == "HKCU",
                    bits32: hive == "HKLM32",
                    about_url: s("URLInfoAbout"),
                    help_url: s("HelpLink"),
                    name,
                    ..Default::default()
                };
                // The same product registered twice (both views): keep one.
                if out.iter().any(|o| o.name == app.name && o.version == app.version && o.publisher == app.publisher) {
                    continue;
                }
                out.push(app);
            }
        }
        out
    }

    /// "ms-resource:AppName" → the text it names in the package.
    fn resource(text: &str, full_name: &str, id: &str) -> String {
        let Some(rest) = text.strip_prefix("ms-resource:") else { return text.to_string() };
        let uri = if rest.starts_with("//") {
            format!("ms-resource:{rest}")
        } else if let Some(r) = rest.strip_prefix('/') {
            format!("ms-resource://{id}/{r}")
        } else if rest.contains('/') {
            format!("ms-resource://{id}/{rest}")
        } else {
            format!("ms-resource://{id}/Resources/{rest}")
        };
        let source = wide(&format!("@{{{full_name}?{uri}}}"));
        let mut out = vec![0u16; 512];
        unsafe {
            if windows_sys::Win32::UI::Shell::SHLoadIndirectString(source.as_ptr(), out.as_mut_ptr(), 512, std::ptr::null()) == 0 {
                let n = out.iter().position(|&c| c == 0).unwrap_or(0);
                return String::from_utf16_lossy(&out[..n]);
            }
        }
        String::new()
    }

    pub fn list_msix_and_links(ctx: &AddonContext) -> (Vec<App>, Vec<Link>) {
        let Ok(v) = ctx.powershell(INVENTORY_SCRIPT, &[], false) else { return (Vec::new(), Vec::new()) };
        let mut apps = Vec::new();
        for p in v["packages"].as_array().cloned().unwrap_or_default() {
            let g = |k: &str| p[k].as_str().unwrap_or_default().to_string();
            let (full, id) = (g("FullName"), g("Id"));
            let mut name = resource(&g("Name"), &full, &id);
            if name.is_empty() {
                name = id.clone();
            }
            let location = g("Location");
            let installed = std::fs::metadata(&location)
                .and_then(|m| m.created())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| ymd(d.as_secs() as i64))
                .unwrap_or_default();
            apps.push(App {
                key: format!("MSIX\\{full}"),
                publisher: resource(&g("Publisher"), &full, &id),
                version: g("Version"),
                installed,
                location,
                no_remove: p["NonRemovable"].as_bool() == Some(true),
                source: Source::Msix,
                user_scope: true,
                bits32: g("Arch") == "X86",
                package: full,
                family: g("Family"),
                name,
                ..Default::default()
            });
        }
        let links = v["links"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|l| Link {
                path: l["Path"].as_str().unwrap_or_default().to_string(),
                name: l["Name"].as_str().unwrap_or_default().to_string(),
                target: l["Target"].as_str().unwrap_or_default().to_string(),
                aumid: l["Aumid"].as_str().unwrap_or_default().to_string(),
            })
            .collect();
        (apps, links)
    }

    pub fn user_assist() -> Vec<Opened> {
        let mut out = Vec::new();
        let Ok(root) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist") else {
            return out;
        };
        for g in root.enum_keys().flatten() {
            let Ok(count) = root.open_subkey(format!(r"{g}\Count")) else { continue };
            for (name, value) in count.enum_values().flatten() {
                let b = &value.bytes;
                if b.len() < 68 {
                    continue;
                }
                let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                let ft = u64::from_le_bytes(b[60..68].try_into().unwrap_or([0; 8]));
                if ft == 0 {
                    continue;
                }
                let what = rot13(&name);
                if what.starts_with("UEME_") {
                    continue;
                }
                out.push(Opened { what: expand_known_folder(&what), when: (ft / 10_000_000) as i64 - 11_644_473_600, count: n });
            }
        }
        out
    }

    #[allow(dead_code)]
    fn now() -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::uninstaller::split_command;
    use crate::platform::Launch;

    fn entry(key: &str, source: Source, user_scope: bool, bits32: bool) -> App {
        App { key: key.into(), source, user_scope, bits32, ..Default::default() }
    }

    #[test]
    fn panel_values_are_right_for_each_registry_view_and_store_apps() {
        let machine64 = entry("HKLM\\Audacity_is1", Source::Registry, false, false);
        let machine32 = entry("HKLM32\\7-Zip", Source::Registry, false, true);
        let user = entry("HKCU\\Discord", Source::Registry, true, false);
        let msi = entry("HKLM\\{23170F69-40C1-2702-2409-000001000000}", Source::Msi, false, false);
        let msix = entry("MSIX\\Microsoft.WindowsCalculator_11.2_x64__8wekyb3d8bbwe", Source::Msix, true, false);

        assert_eq!(machine64.scope_text(), "Machine: everyone on this PC");
        assert_eq!(machine32.scope_text(), "Machine: everyone on this PC \u{00b7} 32-bit");
        assert_eq!(user.scope_text(), "Current user");
        assert_eq!(msi.scope_text(), "Machine: everyone on this PC");
        assert_eq!(msix.scope_text(), "Current user");

        assert_eq!(machine64.source.label(), "Registry");
        assert_eq!(machine32.source.label(), "Registry");
        assert_eq!(user.source.label(), "Registry");
        assert_eq!(msi.source.label(), "MSI");
        assert_eq!(msix.source.label(), "MSIX");

        assert_eq!(machine64.registry_key(), "Audacity_is1");
        assert_eq!(machine32.registry_key(), "7-Zip");
        assert_eq!(user.registry_key(), "Discord");
        assert_eq!(msi.registry_key(), "{23170F69-40C1-2702-2409-000001000000}");
        assert_eq!(msix.registry_key(), "", "Store apps have no uninstall registry key");
    }

    const LIST: &str = "   - \r   \\ \rName                       Id                               Version   Available Source\n\
-------------------------------------------------------------------------------------------\n\
AMD Chipset Software       ARP\\Machine\\X86\\AMD_Chipse       7.06.24\n\
Antigravity 2.5.0          Google.Antigravity               2.5.0     2.32.1    winget\n\
Canva                      XP8K17RNMM8MTN                   1.119.0   1.128.0   msstore\n\
Xbox TCUI                  MSIX\\Xbox.TCUI_1.24_x64__8       1.24.1\n";

    #[test]
    fn matches_winget_rows_to_apps() {
        let mut apps = vec![
            App { key: "HKLM32\\AMD_Chipset_IODrivers".into(), name: "AMD Chipset Software".into(), ..Default::default() },
            App { key: "HKCU\\Antigravity".into(), name: "Antigravity 2.5.0".into(), version: "2.5.0".into(), ..Default::default() },
            App { key: "MSIX\\Canva_1.119.0_x64__abc".into(), name: "Canva".into(), source: Source::Msix, ..Default::default() },
        ];
        match_winget(&mut apps, &parse_winget_table(LIST));
        assert!(!apps[0].has_update());
        assert_eq!((apps[1].winget_id.as_str(), apps[1].available.as_str()), ("Google.Antigravity", "2.32.1"));
        assert_eq!((apps[2].winget_id.as_str(), apps[2].winget_source.as_str()), ("XP8K17RNMM8MTN", "msstore"));
    }

    #[test]
    fn splits_command_lines() {
        assert_eq!(split_command("\"C:\\Program Files\\X\\unins000.exe\" /SILENT"), ("C:\\Program Files\\X\\unins000.exe".into(), "/SILENT".into()));
        assert_eq!(split_command("C:\\Program Files\\Y Z\\uninst.exe /S"), ("C:\\Program Files\\Y Z\\uninst.exe".into(), "/S".into()));
        assert_eq!(split_command("MsiExec.exe /X{1234}"), ("MsiExec.exe".into(), "/X{1234}".into()));
    }

    #[test]
    fn nested_app_folders_are_not_counted_twice() {
        let all = vec![r"C:\Program Files (x86)\GOG Galaxy\".to_string(), r"C:\Program Files (x86)\GOG Galaxy\Games\Kerbal Space Program\".to_string(), r"C:\Program Files (x86)\GOG GalaxyX".to_string()];
        assert_eq!(nested_folders(r"C:\Program Files (x86)\GOG Galaxy\", &all), vec![r"c:\program files (x86)\gog galaxy\games\kerbal space program".to_string()]);
        assert!(nested_folders(r"C:\Program Files (x86)\GOG Galaxy\Games\Kerbal Space Program", &all).is_empty());
    }

    #[test]
    fn unhides_user_assist_names() {
        assert_eq!(rot13("Zvpebfbsg.JvaqbjfPnyphyngbe_8jrxlo3q8oojr!Ncc"), "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App");
    }

    #[test]
    fn cleans_icon_paths() {
        assert_eq!(clean_path("\"C:\\A\\b.exe\",0"), "C:\\A\\b.exe");
        assert_eq!(clean_path("C:\\A\\b.exe,-101"), "C:\\A\\b.exe");
    }

    #[test]
    fn usage_follows_shortcuts_and_packages() {
        let mut apps = vec![
            App { key: "HKCU\\Discord".into(), name: "Discord".into(), ..Default::default() },
            App { key: "MSIX\\Calc".into(), name: "Calculator".into(), source: Source::Msix, family: "Microsoft.WindowsCalculator_8wekyb3d8bbwe".into(), ..Default::default() },
        ];
        let links = vec![Link { path: "c:\\start\\discord.lnk".into(), name: "Discord".into(), target: "c:\\users\\me\\appdata\\local\\discord\\update.exe".into(), aumid: "com.squirrel.Discord.Discord".into() }];
        let usage = vec![
            Opened { what: "com.squirrel.Discord.Discord".into(), when: 100, count: 2 },
            Opened { what: "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into(), when: 200, count: 1 },
        ];
        apply_usage(&mut apps, &usage, &links);
        assert_eq!(apps[0].last_used, Some(100));
        assert_eq!(apps[1].last_used, Some(200));
    }

    fn wait_until_idle(hub: &Hub) {
        for _ in 0..1000 {
            if hub.lock().busy.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not finish");
    }

    #[test]
    fn checking_for_updates_asks_winget_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx);
        hub.check_updates();
        wait_until_idle(&hub);
        assert_eq!(runner.launches(), vec![Launch::Winget(vec!["list".into(), "--accept-source-agreements".into(), "--disable-interactivity".into()])]);
        assert!(hub.lock().message.as_ref().is_some_and(|(_, error)| *error), "the failure is shown");
    }

    #[test]
    fn uninstalling_launches_only_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx);
        hub.lock().apps = vec![
            App { key: "MSIX\\Pkg_1.0_x64__abc".into(), name: "Store app".into(), source: Source::Msix, package: "Pkg_1.0_x64__abc".into(), ..Default::default() },
            App { key: "HKCU\\NotReallyInstalled".into(), name: "Tool".into(), uninstall: "uninst.exe /S".into(), ..Default::default() },
        ];
        hub.uninstall(vec!["MSIX\\Pkg_1.0_x64__abc".into(), "HKCU\\NotReallyInstalled".into()]);
        wait_until_idle(&hub);
        let launches = runner.launches();
        assert!(matches!(&launches[0], Launch::PowerShell { args, elevated: false, .. } if args == &vec![("Package".to_string(), "Pkg_1.0_x64__abc".to_string())]));
        assert_eq!(launches[1], Launch::Program { file: "uninst.exe".into(), params: "/S".into() });
        // Then the list is read again, Store apps through PowerShell.
        assert!(launches[2..].iter().all(|l| matches!(l, Launch::PowerShell { elevated: false, .. })));
    }

    #[test]
    #[ignore]
    fn lists_this_pc() {
        let t = std::time::Instant::now();
        let mut apps = imp::list_registry();
        println!("registry: {} in {:?}", apps.len(), t.elapsed());
        let (msix, links) = imp::list_msix_and_links(&AddonContext::system());
        println!("msix: {} links: {} in {:?}", msix.len(), links.len(), t.elapsed());
        apps.extend(msix);
        sort_apps(&mut apps);
        let usage = imp::user_assist();
        apply_usage(&mut apps, &usage, &links);
        println!("usage entries {}, apps with last use {}", usage.len(), apps.iter().filter(|a| a.last_used.is_some()).count());
        let (code, text) = AddonContext::system().winget(&["list", "--accept-source-agreements", "--disable-interactivity"]).unwrap();
        let rows = parse_winget_table(&text);
        match_winget(&mut apps, &rows);
        println!("winget code {code} rows {} matched {} updates {}", rows.len(), apps.iter().filter(|a| !a.winget_id.is_empty()).count(), apps.iter().filter(|a| a.has_update()).count());
        let unmatched: Vec<&WingetRow> = rows.iter().filter(|r| !r.available.is_empty() && !apps.iter().any(|a| a.winget_id == r.id)).collect();
        println!("unmatched with update: {unmatched:#?}");
        if let Ok(dump) = std::env::var("APPS_DUMP") {
            let all: Vec<String> = apps.iter().filter_map(|a| a.folder()).collect();
            for a in apps.iter_mut() {
                if let Some(d) = a.folder() {
                    a.size_measured = Some(folder_size(std::path::Path::new(&d), &nested_folders(&d, &all)));
                }
            }
            let rows: Vec<serde_json::Value> = apps
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "name": a.name, "publisher": a.publisher, "version": a.version, "available": a.available,
                        "installed": a.installed, "last_used": a.last_used.map(ymd), "size": a.size(),
                        "measured": a.size_measured.is_some(), "scope": a.scope_label(), "source": a.source.label(),
                        "winget": a.winget_id, "folder": a.folder(), "modify": a.can_modify(), "repair": a.can_repair(),
                        "blocker": a.uninstall_blocker(),
                    })
                })
                .collect();
            std::fs::write(dump, serde_json::to_string_pretty(&rows).unwrap()).unwrap();
        }
        let no_folder = apps.iter().filter(|a| a.folder().is_none()).count();
        println!("without a folder: {no_folder}");
        for a in apps.iter().take(400) {
            println!("{:<55} | {:<8} | {:<14} | {:<10} | used {:<10} | {} | {}", a.name.chars().take(55).collect::<String>(), a.source.label(), a.version.chars().take(14).collect::<String>(), a.installed, a.last_used.map(ymd).unwrap_or_default(), a.available, a.folder().unwrap_or_default());
        }
    }
}

/// The pure rules behind the actions and their outcomes (issue #76).
#[cfg(test)]
mod action_rules_tests {
    use super::*;
    use crate::platform::uninstaller::missing_program;

    fn app(name: &str) -> App {
        App { key: format!("HKLM\\{name}"), name: name.into(), ..Default::default() }
    }

    /// A real, app-specific folder under the temp directory.
    fn temp_app_folder(tag: &str) -> String {
        let dir = std::env::temp_dir().join(format!("upcm-ia76-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().trim_end_matches('\\').to_string()
    }

    #[test]
    fn an_app_windows_marks_as_not_removable_is_blocked() {
        let a = App { no_remove: true, uninstall: "C:\\X\\uninst.exe".into(), ..app("Edge") };
        assert_eq!(a.uninstall_blocker().as_deref(), Some("Windows marks Edge as an application that cannot be uninstalled."));
    }

    #[test]
    fn an_app_without_an_uninstaller_is_blocked() {
        let a = App { uninstall: "   ".into(), ..app("Orphan") };
        assert_eq!(a.uninstall_blocker().as_deref(), Some("Orphan did not register an uninstaller with Windows."));
    }

    #[test]
    fn an_msi_product_or_store_app_needs_no_uninstall_string() {
        assert_eq!(App { msi_code: "{1234}".into(), ..app("Msi") }.uninstall_blocker(), None);
        assert_eq!(App { source: Source::Msix, ..app("Store") }.uninstall_blocker(), None);
        assert_eq!(App { uninstall: "C:\\X\\uninst.exe /S".into(), ..app("Plain") }.uninstall_blocker(), None);
    }

    #[test]
    fn a_registered_uninstaller_that_is_gone_is_reported_missing() {
        assert_eq!(
            missing_program("\"C:\\Definitely\\Not\\Here\\uninst.exe\" /S").as_deref(),
            Some("The uninstaller %1 registered is missing (C:\\Definitely\\Not\\Here\\uninst.exe).")
        );
        // A bare program name is found through PATH, not checked here.
        assert_eq!(missing_program("MsiExec.exe /X{1234}"), None);
        let dir = temp_app_folder("uninst");
        let exe = format!("{dir}\\uninst.exe");
        std::fs::write(&exe, b"").unwrap();
        assert_eq!(missing_program(&format!("\"{exe}\" /S")), None);
    }

    #[test]
    fn modify_needs_a_change_command_or_an_msi_product() {
        assert!(App { modify: "C:\\X\\setup.exe /modify".into(), ..app("A") }.can_modify());
        assert!(App { msi_code: "{1234}".into(), ..app("B") }.can_modify());
        assert!(!app("C").can_modify());
        assert!(!App { msi_code: "{1234}".into(), no_modify: true, ..app("D") }.can_modify());
        assert!(!App { modify: "x.exe".into(), source: Source::Msix, ..app("E") }.can_modify());
    }

    #[test]
    fn repair_needs_an_msi_product_that_allows_it() {
        assert!(App { msi_code: "{1234}".into(), ..app("A") }.can_repair());
        assert!(!App { msi_code: "{1234}".into(), no_repair: true, ..app("B") }.can_repair());
        assert!(!App { modify: "setup.exe".into(), ..app("C") }.can_repair());
    }

    #[test]
    fn an_update_needs_both_a_newer_version_and_a_winget_id() {
        assert!(App { available: "2.0".into(), winget_id: "Vendor.App".into(), ..app("A") }.has_update());
        assert!(!App { available: "2.0".into(), ..app("B") }.has_update());
        assert!(!App { winget_id: "Vendor.App".into(), ..app("C") }.has_update());
    }

    // Windows only: a registry location and icon on a drive.
    #[cfg(windows)]
    #[test]
    fn the_folder_comes_from_the_location_then_the_icon() {
        let dir = temp_app_folder("folder");
        assert_eq!(App { location: format!("\"{dir}\""), ..app("A") }.folder(), Some(dir.clone()));
        assert_eq!(App { icon: format!("\"{dir}\\app.exe\",0"), ..app("B") }.folder(), Some(dir.clone()));
        // A shared root is not the app's own folder.
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
        assert_eq!(App { location: windir.clone(), icon: format!("{windir}\\System32\\shell32.dll,3"), ..app("C") }.folder(), None);
        assert_eq!(App { location: "C:\\".into(), ..app("D") }.folder(), None);
        assert_eq!(app("E").folder(), None);
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn update_summaries() {
        assert_eq!(update_summary(&names(&["Foo"]), &[]), ("Updated Foo.".into(), false));
        assert_eq!(update_summary(&names(&["A", "B", "C"]), &[]), ("Updated A, B and C.".into(), false));
        assert_eq!(update_summary(&[], &names(&["Foo (the installer failed)"])), ("Not updated: Foo (the installer failed).".into(), true));
        assert_eq!(
            update_summary(&names(&["A", "B"]), &names(&["C (no package found)", "D (restart Windows to finish)"])),
            ("Updated A and B. Not updated: C (no package found); D (restart Windows to finish).".into(), true)
        );
    }

    #[test]
    fn uninstall_summaries() {
        assert_eq!(uninstall_summary(1, &names(&["Foo"]), &[]), ("Foo was uninstalled.".into(), false));
        assert_eq!(uninstall_summary(1, &[], &names(&["Foo did not register an uninstaller with Windows."])), ("Foo did not register an uninstaller with Windows.".into(), true));
        assert_eq!(uninstall_summary(3, &names(&["A", "B", "C"]), &[]), ("3 of 3 applications were uninstalled.".into(), false));
        assert_eq!(
            uninstall_summary(3, &names(&["A"]), &names(&["B is still installed after its uninstaller closed.", "Stopped waiting for the uninstaller of C."])),
            ("1 of 3 applications were uninstalled. B is still installed after its uninstaller closed. Stopped waiting for the uninstaller of C.".into(), true)
        );
    }
}
