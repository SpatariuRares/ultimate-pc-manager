//! The installed packages: which have a newer version in their catalog,
//! updating them (one or in series: the same series, queue and summary as
//! installs) and uninstalling them.
//!
//! The list is `winget list`. A row a catalog serves (its Source is
//! "winget" or "msstore") is updated and uninstalled through that catalog;
//! after an uninstall the catalog is asked whether the package is gone. A
//! row no catalog handles (winget names it `ARP\…` for an uninstall
//! registry entry, `MSIX\…` for a Store package) falls back on the shared
//! uninstaller: the registry's command, `MsiExec /X` or the MSIX removal,
//! waiting for every process it starts (until "Stop waiting") and checking
//! that the entry is gone, as Installed Apps does. So does a catalog row
//! winget could not uninstall without administrator rights (a machine-wide
//! MSI run silently cannot ask for them): its machine-wide entry, whose
//! MSI then asks itself.
//!
//! Every update and uninstall goes through the app's install queue, so the
//! host tells the add-ons that are on that the packages changed; this
//! add-on then reads its list again.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use egui::{RichText, Ui};

use crate::platform::install_queue::{Request, Work};
use crate::platform::uninstaller::{self, Uninstaller};
use super::catalog::ExitClass;
use crate::platform::{lock_state, AddonContext, Job, Ran};

use super::batch::{Kind, Status};
use super::catalog::{self, Found, PackageSource, Target};
use super::page::{cell_text, list_bar, Pick, FOLDED, INSTALLED_COLS};
use super::{busy_line, code_text, log_path, Command, Outcome, PackageManager, State, ID};
use crate::widgets::{dot, group_title, note, note_text, row_button, table_line, Tone};

/// A package installed on this PC, as `winget list` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Package {
    pub name: String,
    /// winget's id: a catalog's package id, or `ARP\…` / `MSIX\…` for an
    /// entry no catalog handles.
    pub id: String,
    /// The version installed.
    pub version: String,
    /// The newer version its catalog has (empty: none).
    pub available: String,
    /// The catalog serving it ("winget", "msstore"; empty: none).
    pub catalog: String,
}

impl Package {
    /// Its catalog has a newer version.
    pub fn has_update(&self) -> bool {
        !self.available.is_empty() && !self.catalog.is_empty()
    }

    /// The package to update, at the version it goes to.
    pub fn update(&self) -> Found {
        Found { name: self.name.clone(), id: self.id.clone(), version: self.available.clone(), catalog: self.catalog.clone(), installed: false }
    }

    /// Its item in the app's install queue: (catalog, id), or the shared
    /// uninstaller's ("registry" or "msix") for an entry no catalog handles.
    fn queue_item(&self) -> (String, String) {
        let catalog = match self.catalog.as_str() {
            "" if self.id.starts_with("MSIX\\") => "msix",
            "" => "registry",
            c => c,
        };
        (catalog.to_string(), self.id.clone())
    }
}

/// Where the list of installed packages is.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Listing {
    /// Not asked for yet.
    #[default]
    NotRead,
    /// winget is listing them.
    Reading,
    /// The packages; `note` says which rows were left out.
    Read { packages: Vec<Package>, note: Option<String> },
    /// winget could not list them; why.
    Failed(String),
}

/// An uninstall, running or done.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    pub package: Package,
    /// The shared uninstaller is waiting for the program's uninstaller.
    pub waiting: bool,
    /// How it ended in words (`Err`: it is not gone); `None` while it runs.
    pub outcome: Option<Result<String, String>>,
}

/// What the page shows about the installed packages.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Installed {
    pub listing: Listing,
    /// The last uninstall.
    pub removal: Option<Removal>,
}

impl Installed {
    /// The packages whose catalog has a newer version.
    pub fn updates(&self) -> Vec<Package> {
        match &self.listing {
            Listing::Read { packages, .. } => packages.iter().filter(|p| p.has_update()).cloned().collect(),
            _ => Vec::new(),
        }
    }
}

/// How the add-on uninstalls what no catalog handles.
pub struct Fallback {
    /// The uninstall registry entry for a key ("HKLM\{id}"); tests give
    /// their own.
    pub entry_of: fn(&str) -> Option<uninstaller::Target>,
    /// Whether the app runs as administrator; tests give their own.
    pub admin: fn(&AddonContext) -> bool,
    /// The shared uninstaller of the running uninstall ("Stop waiting").
    uninstaller: Option<Uninstaller>,
}

impl Default for Fallback {
    fn default() -> Self {
        Fallback { entry_of: uninstaller::registry_entry, admin: AddonContext::is_admin, uninstaller: None }
    }
}

/// Added to winget's failure when a machine-wide uninstall needs
/// administrator rights this app cannot ask for.
pub const ADMIN_HINT: &str = "Uninstalling it needs administrator rights: restart the app as administrator and try again.";

/// The shared uninstaller's target for an entry no catalog handles:
/// `MSIX\{full name}` is a Store package; `ARP\{Machine|User}\{X64|X86}\{key}`
/// is an uninstall registry entry, read with `entry_of`. `None` when the id
/// is neither or the entry is gone.
pub fn local_target(id: &str, entry_of: fn(&str) -> Option<uninstaller::Target>) -> Option<uninstaller::Target> {
    if let Some(package) = id.strip_prefix("MSIX\\") {
        return Some(uninstaller::Target::Msix { package: package.to_string() });
    }
    entry_of(&registry_key(id)?)
}

/// The uninstall registry key winget's `ARP\…` id stands for.
pub fn registry_key(id: &str) -> Option<String> {
    let mut parts = id.splitn(4, '\\');
    let (Some("ARP"), Some(scope), Some(arch), Some(sub)) = (parts.next(), parts.next(), parts.next(), parts.next()) else { return None };
    let hive = match (scope, arch) {
        ("User", _) => "HKCU",
        ("Machine", "X86") => "HKLM32",
        ("Machine", _) => "HKLM",
        _ => return None,
    };
    (!sub.is_empty()).then(|| format!("{hive}\\{sub}"))
}

/// Lists the installed packages of every source. None when `stop` ended it.
fn read_list(context: &AddonContext, accept: bool, stop: &AtomicBool) -> Option<Listing> {
    let (mut packages, mut shortened) = (Vec::new(), 0);
    for source in catalog::SOURCES {
        match source.list_packages(context, accept, stop)? {
            Err(why) => return Some(Listing::Failed(why)),
            Ok((found, left_out)) => {
                packages.extend(found);
                shortened += left_out;
            }
        }
    }
    let note = (shortened > 0).then(|| format!("{shortened} packages are not shown: winget shortened their id."));
    Some(Listing::Read { packages, note })
}

/// Updates `package` with `source`, its log in `log` (inside `logs`, made
/// first), then, when the exit code says it was updated, checks that it
/// shows as installed at its new version (`package.version`; any version
/// when that is not known). Never cut short.
pub(super) fn update_package(context: &AddonContext, source: &dyn PackageSource, package: &Found, log: &Path, logs: &Path, accept: bool, on_line: &mut dyn FnMut(&str)) -> Outcome {
    let never = AtomicBool::new(false);
    let target = Target { catalog: &package.catalog, id: &package.id, log, accept_agreements: accept, pin: &super::catalog::Pin::default() };
    let ran = match std::fs::create_dir_all(logs) {
        Err(e) => Ran::NotStarted(format!("the log folder {} could not be made ({e}).", logs.display())),
        Ok(()) => source.update(context, &target, &never, on_line),
    };
    match ran {
        Ran::NotStarted(why) => Outcome::NotStarted(why),
        Ran::Stopped => Outcome::NotStarted("it was stopped.".into()),
        Ran::Exited(code) => {
            let updated = source.classify(code).is_success();
            let found = if updated { source.installed_version(context, &package.catalog, &package.id, accept, &never) } else { None };
            let found = found.map(|version| version.is_some_and(|v| !known_version(&package.version) || v == package.version));
            Outcome::Exited { code, found }
        }
    }
}

/// A version winget names (it says "Unknown" when it does not know).
fn known_version(version: &str) -> bool {
    !version.is_empty() && !version.eq_ignore_ascii_case("unknown")
}

/// The status an update's `outcome` gives its package, which goes to
/// `version`: an update winget reports done but not found at that version
/// is told apart.
pub fn update_status(source: &dyn PackageSource, outcome: &Outcome, version: &str) -> Status {
    match outcome {
        Outcome::Exited { code, found: Some(false) } if source.classify(*code).is_success() => {
            let what = if known_version(version) { format!("version {version} is") } else { "it is".to_string() };
            Status::Failed(format!("updated according to winget ({}), but {what} not found among the installed packages", code_text(source, *code)))
        }
        _ => outcome.status(source),
    }
}

/// How an uninstall winget could not run for want of administrator rights
/// falls back on the shared uninstaller.
struct Elevating<'a> {
    /// The uninstall registry entry for a key.
    entry_of: fn(&str) -> Option<uninstaller::Target>,
    /// Runs the shared uninstaller on the entry.
    uninstall: &'a mut dyn FnMut(&uninstaller::Target) -> Result<(), String>,
}

/// Uninstalls `package` through its catalog, then asks the catalog whether
/// it is gone. When winget could not run its uninstaller and `elevating`
/// is given (the app is not administrator), a package with a machine-wide
/// uninstall registry entry is uninstalled by the shared uninstaller
/// instead: a machine-wide MSI winget runs silently cannot ask for
/// administrator rights, the MSI run by the shared uninstaller asks itself.
fn uninstall_with(context: &AddonContext, source: &dyn PackageSource, package: &Package, logs: &Path, accept: bool, elevating: Option<Elevating>) -> Result<String, String> {
    let never = AtomicBool::new(false);
    let name = &package.name;
    let log = log_path(logs, "uninstall", &package.catalog, &package.id);
    std::fs::create_dir_all(logs).map_err(|e| format!("{name} was not started: the log folder {} could not be made ({e}).", logs.display()))?;
    let target = Target { catalog: &package.catalog, id: &package.id, log: &log, accept_agreements: accept, pin: &super::catalog::Pin::default() };
    let code = match source.uninstall(context, &target, &never, &mut |_| {}) {
        Ran::NotStarted(why) => return Err(format!("{name} was not started: {}", why.trim())),
        Ran::Stopped => return Err(format!("{name} was not started: it was stopped.")),
        Ran::Exited(code) => code,
    };
    if let (true, Some(elevating)) = (source.uninstall_needs_admin(code), elevating) {
        let local = source.local_id(context, &package.catalog, &package.id, accept, &never);
        let entry = local.filter(|l| l.starts_with("ARP\\Machine\\")).and_then(|l| registry_key(&l)).and_then(|key| (elevating.entry_of)(&key));
        let Some(entry) = entry else {
            return Err(format!("{name} could not be uninstalled: {}. {ADMIN_HINT}", source.describe(code)));
        };
        (elevating.uninstall)(&entry)?;
        return match source.is_installed(context, &package.catalog, &package.id, accept, &never) {
            Some(true) => Err(format!("{name} is still installed after its uninstaller closed.")),
            _ => Ok(format!("{name} was uninstalled.")),
        };
    }
    let class = source.classify(code);
    match class {
        _ if class.is_success() && source.is_installed(context, &package.catalog, &package.id, accept, &never) == Some(true) => {
            Err(format!("{name} is still installed after winget uninstalled it ({}).", code_text(source, code)))
        }
        ExitClass::Installed => Ok(format!("{name} was uninstalled.")),
        ExitClass::InstalledRestart => Ok(format!("{name} was uninstalled; {}.", source.describe(code))),
        ExitClass::Cancelled => Err(format!("{name} was not uninstalled: {}.", source.describe(code))),
        ExitClass::Skipped | ExitClass::Failed => Err(format!("{name} could not be uninstalled: {}.", source.describe(code))),
    }
}

impl PackageManager {
    /// Reads the installed packages in the background (nothing while they
    /// are being read or the add-on is off).
    pub(super) fn list_installed(&mut self) {
        let Some(context) = self.context.clone() else { return };
        if lock_state(&self.state).installed.listing == Listing::Reading {
            return;
        }
        lock_state(&self.state).installed.listing = Listing::Reading;
        let (job, stop, accept) = (Job::new(&self.state, &self.generation), self.stop.clone(), self.prefs.accept_agreements);
        std::thread::spawn(move || {
            if let Some(listing) = read_list(&context, accept, &stop) {
                job.publish(|s| s.installed.listing = listing);
            }
        });
    }

    /// The packages changed: reads the list again when it was read.
    pub(super) fn relist(&mut self) {
        if matches!(lock_state(&self.state).installed.listing, Listing::Read { .. } | Listing::Failed(_)) {
            self.list_installed();
        }
    }

    /// Updates `packages` one after another in the background: the same
    /// series as installs (one queue request, "Stop after this one", the
    /// summary, the journal), in update words.
    pub(super) fn update_all(&mut self, packages: Vec<Found>) {
        let items = self.new_items(Kind::Update, packages);
        self.run_series(Kind::Update, items);
    }

    /// Uninstalls `package` in the background through the app's install
    /// queue: through its catalog, or the shared uninstaller when no
    /// catalog handles it. Turning the add-on off lets it finish.
    pub(super) fn uninstall(&mut self, package: Package) {
        let (Some(context), Some(series)) = (self.context.clone(), self.series.clone()) else { return };
        let (logs, accept, entry_of) = (self.logs.clone(), self.prefs.accept_agreements, self.fallback.entry_of);
        let admin = (self.fallback.admin)(&context);
        let uninstaller = Uninstaller::new(context.clone());
        self.fallback.uninstaller = Some(uninstaller.clone());
        let busy = format!("Uninstalling {}\u{2026}", package.name);
        series.start(Some(busy), true, move |run| {
            let request = Request { addon: ID, name: "Package Manager", work: Work::Uninstall, items: vec![package.queue_item()] };
            let Some(turn) = run.queue(context.install_queue(), request) else { return };
            run.follow().installed.removal = Some(Removal { package: package.clone(), waiting: false, outcome: None });
            let mut shared = |target: &uninstaller::Target| {
                uninstaller.uninstall(&package.name, target, |waiting| {
                    if let Some(r) = run.follow().installed.removal.as_mut() {
                        r.waiting = waiting;
                    }
                })
            };
            let outcome = match catalog::source_of(&package.catalog) {
                Some(source) => {
                    let elevating = (!admin).then(|| Elevating { entry_of, uninstall: &mut shared });
                    uninstall_with(&context, source, &package, &logs, accept, elevating)
                }
                None => match local_target(&package.id, entry_of) {
                    None => Err(uninstaller::not_registered(&package.name)),
                    Some(target) => shared(&target),
                }
                .map(|()| format!("{} was uninstalled.", package.name)),
            };
            drop(turn);
            if let Some(r) = run.follow().installed.removal.as_mut() {
                r.waiting = false;
                r.outcome = Some(outcome);
            }
        });
    }

    /// Stops waiting for the running uninstaller (it keeps running).
    pub(super) fn stop_waiting(&mut self) {
        if let Some(u) = &self.fallback.uninstaller {
            u.stop_waiting();
        }
    }

    /// The installed packages: "Check for updates", the last uninstall, the
    /// packages with a newer version first ("Update", "Update all"), then
    /// the others folded after the first few ("Show all"), each with
    /// "Uninstall".
    pub(super) fn draw_installed(&mut self, ui: &mut Ui, st: &State) {
        let pal = crate::theme::of_ui(ui);
        let idle = st.busy.is_none();
        let mut asked = Vec::new();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Installed packages").size(14.0).strong().color(pal.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| match &st.installed.listing {
                Listing::NotRead => {
                    if crate::widgets::parts::primary_button(ui, "Check for updates", true).clicked() {
                        asked.push(Command::ListInstalled);
                    }
                }
                listing => {
                    let reading = *listing == Listing::Reading;
                    if ui.add_enabled(!reading, egui::Button::new(RichText::new("Check again").size(13.0)).min_size(egui::vec2(0.0, 30.0))).clicked() {
                        asked.push(Command::ListInstalled);
                    }
                }
            });
        });
        if let Some(r) = &st.installed.removal {
            match &r.outcome {
                None if r.waiting => note(ui, Tone::Warm, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(format!("Waiting for the uninstaller of {} \u{2014} answer its questions in its own window\u{2026}", r.package.name)).size(13.0));
                        if ui.add(egui::Button::new(RichText::new("Stop waiting").size(12.5)).min_size(egui::vec2(0.0, 26.0))).clicked() {
                            asked.push(Command::StopWaiting);
                        }
                    });
                }),
                None => note(ui, Tone::Neutral, |ui| busy_line(ui, &format!("Uninstalling {}\u{2026}", r.package.name))),
                Some(Ok(text)) => note(ui, Tone::Neutral, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        dot(ui, pal.green);
                        ui.label(RichText::new(text).size(13.0));
                    });
                }),
                Some(Err(text)) => note_text(ui, Tone::Bad, text),
            }
        }
        match &st.installed.listing {
            Listing::NotRead => note_text(ui, Tone::Neutral, "Lists what winget sees on this PC and which packages have a newer version."),
            Listing::Reading => busy_line(ui, "Listing the installed packages\u{2026}"),
            Listing::Failed(why) => note_text(ui, Tone::Bad, why),
            Listing::Read { packages, note: left_out } => {
                let updates = st.installed.updates();
                list_bar(ui, &pal, |ui| {
                    if updates.is_empty() {
                        dot(ui, pal.green);
                        ui.label(RichText::new("Every package is up to date.").size(13.0).color(pal.text));
                    } else {
                        let have = if updates.len() == 1 { "has" } else { "have" };
                        ui.label(RichText::new(format!("{} {have} a newer version", crate::addons::plural(updates.len(), "package"))).size(13.0).color(pal.text));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if crate::widgets::parts::primary_button(ui, &format!("Update all ({})", updates.len()), idle).clicked() {
                                asked.push(Command::UpdateAll(updates.iter().map(Package::update).collect()));
                            }
                        });
                    }
                });
                table_line(ui, &INSTALLED_COLS, 26.0, true, false, |i, ui| {
                    if let Some(heading) = ["Name", "Id", "Version", "Available"].get(i) {
                        ui.label(RichText::new(*heading).size(12.0).color(pal.label));
                    }
                });
                let (newer, current): (Vec<&Package>, Vec<&Package>) = packages.iter().partition(|p| p.has_update());
                let mut picked = None;
                let mut row = |ui: &mut Ui, p: &Package| {
                    let selected = self.view.pick.as_ref() == Some(&Pick::Installed(p.id.clone()));
                    let line = table_line(ui, &INSTALLED_COLS, 32.0, false, selected, |i, ui| match i {
                        0 => cell_text(ui, &p.name, pal.text, false),
                        1 => cell_text(ui, &p.id, pal.label, true),
                        2 => cell_text(ui, &p.version, pal.text, false),
                        3 => cell_text(ui, if p.has_update() { p.available.as_str() } else { "" }, pal.amber, false),
                        _ => {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if crate::widgets::row_danger_button(ui, "Uninstall", idle).clicked() {
                                    asked.push(Command::Uninstall(p.clone()));
                                }
                                if p.has_update() && row_button(ui, "Update", idle).clicked() {
                                    asked.push(Command::Update(p.update()));
                                }
                            });
                        }
                    });
                    if line.clicked() {
                        picked = Some(Pick::Installed(p.id.clone()));
                    }
                };
                if !newer.is_empty() {
                    group_title(ui, "Newer version available", &newer.len().to_string());
                    for p in &newer {
                        row(ui, p);
                    }
                }
                group_title(ui, "Up to date", &current.len().to_string());
                let shown = if self.view.show_all { current.len() } else { current.len().min(FOLDED) };
                for p in &current[..shown] {
                    row(ui, p);
                }
                if current.len() > FOLDED {
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        let words = if self.view.show_all { "Show fewer".to_string() } else { format!("Show all {}", current.len()) };
                        if ui.link(RichText::new(words).size(12.5)).clicked() {
                            self.view.show_all = !self.view.show_all;
                        }
                    });
                }
                if let Some(pick) = picked {
                    self.view.picked(pick);
                }
                if let Some(text) = left_out {
                    ui.add(egui::Label::new(RichText::new(text).size(12.0).color(pal.label)).wrap());
                }
            }
        }
        self.ask(ui, asked);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::super::Commands;
    use super::*;
    use crate::addons::{until, AddonHost, Entry};
    use crate::platform::{Launch, PageContext, RecordingRunner, When};
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use serde_json::json;

    use crate::platform::winget::{describe, NO_PACKAGE_FOUND, UNINSTALL_COMMAND_FAILED};

    /// A `winget list` table with these rows (name, id, version, available,
    /// source), padded the way winget pads it.
    fn table(rows: &[(&str, &str, &str, &str, &str)]) -> String {
        let line = |c: (&str, &str, &str, &str, &str)| format!("{:<22}{:<52}{:<10}{:<11}{}", c.0, c.1, c.2, c.3, c.4).trim_end().to_string();
        let mut out = vec![line(("Name", "Id", "Version", "Available", "Source")), "-".repeat(100)];
        out.extend(rows.iter().map(|r| line(*r)));
        out.join("\n") + "\n"
    }

    fn list() -> String {
        table(&[
            ("VLC media player", "VideoLAN.VLC", "3.0.20", "3.0.21", "winget"),
            ("Git", "Git.Git", "2.47.0", "", "winget"),
            ("Old Tool", "ARP\\Machine\\X64\\OldTool", "1.0", "", ""),
            ("Calculator", "MSIX\\Microsoft.WindowsCalculator_11.2_x64__8wekyb", "11.2", "", ""),
            ("Notes", "9WZDNCRFHVQM", "4.1", "4.2", "msstore"),
        ])
    }

    fn package(name: &str, id: &str, version: &str, available: &str, catalog: &str) -> Package {
        Package { name: name.into(), id: id.into(), version: version.into(), available: available.into(), catalog: catalog.into() }
    }

    fn vlc() -> Package {
        package("VLC media player", "VideoLAN.VLC", "3.0.20", "3.0.21", "winget")
    }

    fn frame(mut f: impl FnMut(&mut egui::Ui, &mut PageContext)) {
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        // The app\'s faces, as at start-up: pages draw in them.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| f(ui, &mut PageContext::new(&settings, &sample)));
        });
    }

    struct Page {
        host: AddonHost,
        runner: Arc<RecordingRunner>,
        view: Arc<Mutex<State>>,
        commands: Commands,
        logs: std::path::PathBuf,
    }

    impl Page {
        /// A host holding only Package Manager (on, its logs in a scratch
        /// folder); registry entries come from `entry_of`.
        fn new(tag: &str, entry_of: fn(&str) -> Option<uninstaller::Target>) -> Page {
            Page::with(tag, entry_of, false)
        }

        /// Like [`Page::new`], the app running as administrator when `admin`.
        fn with(tag: &str, entry_of: fn(&str) -> Option<uninstaller::Target>, admin: bool) -> Page {
            let logs = std::env::temp_dir().join(format!("upcm-pm118-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&logs);
            let mut pm = PackageManager::default();
            pm.logs = logs.clone();
            pm.fallback.entry_of = entry_of;
            pm.fallback.admin = if admin { |_| true } else { |_| false };
            let (view, commands) = (pm.view(), pm.commands());
            let (host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(pm))], &json!({ ID: { "enabled": true } }));
            Page { host, runner, view, commands, logs }
        }

        /// The user does `command` on the page (applied at its next frame).
        fn act(&mut self, command: Command) {
            lock_state(&self.commands).push(command);
            let host = &mut self.host;
            frame(|ui, page| {
                host.ui(ID, ui, page);
            });
        }

        fn state(&self) -> State {
            lock_state(&self.view).clone()
        }

        /// The arguments of each winget run whose first argument is `verb`.
        fn winget(&self, verb: &str) -> Vec<Vec<String>> {
            self.runner
                .launches()
                .iter()
                .filter_map(|l| match l {
                    Launch::Run { program, args, .. } if program == "winget" && args.first().map(String::as_str) == Some(verb) => Some(args.clone()),
                    _ => None,
                })
                .collect()
        }

        /// How many times the whole list was read (not one package's check).
        fn full_lists(&self) -> usize {
            self.winget("list").iter().filter(|a| a.get(1).map(String::as_str) != Some("--id")).count()
        }

        /// Lists the installed packages from `text`.
        fn listed(&mut self, text: &str) {
            self.runner.prepare(When::Arg("list".into()), 0, text);
            self.act(Command::ListInstalled);
            until("the list is read", || matches!(self.state().installed.listing, Listing::Read { .. }));
        }

        fn series_done(&self) -> bool {
            self.state().batch.is_some_and(|b| b.done)
        }

        fn removed(&self) -> Option<Result<String, String>> {
            self.state().installed.removal.and_then(|r| r.outcome)
        }

        /// What the shared uninstaller started (a program, or PowerShell).
        fn fallback(&self) -> Vec<Launch> {
            self.runner.launches().into_iter().filter(|l| matches!(l, Launch::Program { .. } | Launch::PowerShell { .. })).collect()
        }

        /// The add-ons the queue's finished jobs came from.
        fn finished_jobs(&self) -> Vec<&'static str> {
            self.host.context().install_queue().take_finished()
        }
    }

    impl Drop for Page {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.logs);
        }
    }

    fn no_entry(_: &str) -> Option<uninstaller::Target> {
        None
    }

    #[test]
    fn the_list_shows_every_installed_package_and_which_have_a_newer_version() {
        let mut page = Page::new("list", no_entry);
        page.listed(&list());
        assert_eq!(page.winget("list"), [["list", "--disable-interactivity", "--accept-source-agreements"]]);
        let st = page.state();
        let Listing::Read { packages, note } = &st.installed.listing else { unreachable!() };
        assert_eq!(packages.len(), 5);
        assert_eq!(packages[2], package("Old Tool", "ARP\\Machine\\X64\\OldTool", "1.0", "", ""), "no catalog handles it");
        assert_eq!(*note, None);
        assert_eq!(st.installed.updates(), [vlc(), package("Notes", "9WZDNCRFHVQM", "4.1", "4.2", "msstore")]);
    }

    #[test]
    fn a_shortened_id_is_left_out_and_said() {
        let (packages, shortened) = catalog::parse_list(&table(&[("Long", "Vendor.SomeVeryLongPackageNameThatGotCu\u{2026}", "1.0", "1.1", "winget"), ("Git", "Git.Git", "2.47.0", "", "winget")]));
        assert_eq!((packages.len(), shortened), (1, 1));
    }

    #[test]
    fn a_failed_list_says_why() {
        let mut page = Page::new("listfail", no_entry);
        page.runner.prepare(When::Arg("list".into()), 1603, "");
        page.act(Command::ListInstalled);
        until("the list is read", || page.state().installed.listing != Listing::Reading);
        assert_eq!(page.state().installed.listing, Listing::Failed("winget could not list the installed packages: exit code 1603.".into()));
    }

    #[test]
    fn updating_one_package_runs_winget_upgrade_through_the_queue_and_sums_it_up_in_update_words() {
        let mut page = Page::new("update1", no_entry);
        page.runner.prepare(When::Arg("upgrade".into()), 0, "Successfully installed");
        page.act(Command::Update(vlc().update()));
        until("the series ends", || page.series_done());
        let upgrades = page.winget("upgrade");
        assert_eq!(upgrades.len(), 1);
        assert_eq!(upgrades[0][..8], ["upgrade", "--id", "VideoLAN.VLC", "--exact", "--source", "winget", "--silent", "--disable-interactivity"]);
        assert!(!upgrades[0].contains(&"--no-upgrade".to_string()));
        let b = page.state().batch.unwrap();
        assert!(b.items[0].log.as_ref().is_some_and(|l| l.starts_with(&page.logs)), "it has its log");
        assert_eq!(b.summary(), ["VLC media player was updated."]);
        assert_eq!(b.items[0].status.text(b.kind), "Updated");
        assert_eq!(page.finished_jobs(), [ID], "the queue noted the job: the packages changed");
    }

    #[test]
    fn updating_in_series_goes_on_after_a_failure_and_retries_only_the_failed_one_as_an_update() {
        let mut page = Page::new("updateall", no_entry);
        page.runner.prepare(When::Arg("Vendor.A".into()), 0, "");
        page.runner.prepare(When::Arg("Vendor.B".into()), 1603, "");
        page.runner.prepare(When::Arg("Vendor.C".into()), 3010, "");
        let pkg = |n: &str| package(n, &format!("Vendor.{n}"), "1.0", "2.0", "winget").update();
        page.act(Command::UpdateAll(vec![pkg("A"), pkg("B"), pkg("C")]));
        until("the series ends", || page.series_done());
        assert_eq!(page.winget("upgrade").iter().map(|a| a[2].as_str()).collect::<Vec<_>>(), ["Vendor.A", "Vendor.B", "Vendor.C"]);
        assert_eq!(page.state().batch.unwrap().summary(), ["Updated: A.", "Updated, restart Windows to finish: C.", "Failed: B (exit code 1603)."]);
        assert_eq!(page.finished_jobs(), [ID], "one queue job for the whole series");

        page.runner.prepare(When::Arg("Vendor.B".into()), 0, "");
        page.act(Command::RetryFailed);
        until("the retry ends", || page.series_done() && page.state().batch.unwrap().items.len() == 1);
        assert_eq!(page.winget("upgrade").len(), 4, "retried with upgrade");
        assert!(page.winget("install").is_empty(), "never installed");
        assert_eq!(page.state().batch.unwrap().summary(), ["B was updated."]);
    }

    #[test]
    fn an_update_is_checked_with_winget_list_and_one_not_found_at_its_new_version_is_told_apart() {
        let mut page = Page::new("updatecheck", no_entry);
        let row = |version: &str| table(&[("VLC media player", "VideoLAN.VLC", version, "", "winget")]);
        page.runner.prepare(When::Arg("upgrade".into()), 0, "Successfully installed");
        page.runner.prepare(When::Arg("list".into()), 0, &row("3.0.20"));
        page.act(Command::Update(vlc().update()));
        until("the series ends", || page.series_done());
        assert_eq!(page.winget("list"), [["list", "--id", "VideoLAN.VLC", "--exact", "--disable-interactivity", "--accept-source-agreements"]]);
        let b = page.state().batch.unwrap();
        assert_eq!(
            b.summary(),
            ["VLC media player failed: updated according to winget (exit code 0), but version 3.0.21 is not found among the installed packages."]
        );

        page.runner.prepare(When::Arg("upgrade".into()), 0, "Successfully installed");
        page.runner.prepare(When::Arg("list".into()), 0, &row("3.0.21"));
        page.act(Command::Update(vlc().update()));
        until("the second series ends", || page.series_done() && page.winget("list").len() == 2);
        assert_eq!(page.state().batch.unwrap().summary(), ["VLC media player was updated."]);
    }

    #[test]
    fn a_catalog_package_is_uninstalled_by_its_catalog_then_checked_gone() {
        let mut page = Page::new("catalog", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), 0, "");
        page.runner.prepare(When::Arg("--exact".into()), NO_PACKAGE_FOUND, "");
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        let runs = page.winget("uninstall");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0][..8], ["uninstall", "--id", "VideoLAN.VLC", "--exact", "--source", "winget", "--silent", "--disable-interactivity"]);
        assert_eq!(page.winget("list"), [["list", "--id", "VideoLAN.VLC", "--exact", "--disable-interactivity", "--accept-source-agreements"]], "checked gone");
        assert_eq!(page.removed(), Some(Ok("VLC media player was uninstalled.".into())));
        assert!(page.fallback().is_empty(), "the shared uninstaller is not used");
        assert_eq!(page.finished_jobs(), [ID]);
    }

    #[test]
    fn a_failed_uninstall_says_so_in_words_and_a_refused_one_names_administrator_rights() {
        let mut page = Page::new("failed-text", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), 1603, "");
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.removed(), Some(Err("VLC media player could not be uninstalled: exit code 1603.".into())));

        let mut page = Page::new("restart-text", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), 3010, "");
        page.runner.prepare(When::Arg("--exact".into()), NO_PACKAGE_FOUND, "");
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.removed(), Some(Ok("VLC media player was uninstalled; restart Windows to finish (3010).".into())));

        let mut page = Page::new("cancel-text", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), 1602, "");
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.removed(), Some(Err("VLC media player was not uninstalled: the installation was cancelled (1602).".into())));

        let mut page = Page::new("admin-text", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), UNINSTALL_COMMAND_FAILED, "");
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        let removed = page.removed();
        let Some(Err(text)) = removed else { panic!("{removed:?}") };
        assert!(text.starts_with("VLC media player could not be uninstalled: "), "{text}");
        assert!(text.ends_with("Uninstalling it needs administrator rights: restart the app as administrator and try again."), "{text}");
    }

    #[test]
    fn a_catalog_package_still_listed_after_its_uninstall_is_told() {
        let mut page = Page::new("still", no_entry);
        page.runner.prepare(When::Arg("uninstall".into()), 0, "");
        page.runner.prepare(When::Arg("--exact".into()), 0, &table(&[("VLC media player", "VideoLAN.VLC", "3.0.20", "", "winget")]));
        page.act(Command::Uninstall(vlc()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.removed(), Some(Err("VLC media player is still installed after winget uninstalled it (exit code 0).".into())));
    }

    fn old_tool_entry(key: &str) -> Option<uninstaller::Target> {
        (key == "HKLM\\OldTool").then(|| uninstaller::Target::Registry { key: key.into(), command: "uninst.exe /S".into(), msi_code: String::new() })
    }

    #[test]
    fn an_entry_no_catalog_handles_falls_back_on_the_shared_uninstaller() {
        let mut page = Page::new("arp", old_tool_entry);
        page.act(Command::Uninstall(package("Old Tool", "ARP\\Machine\\X64\\OldTool", "1.0", "", "")));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.winget("uninstall").is_empty(), "winget is not asked");
        assert_eq!(page.fallback(), [Launch::Program { file: "uninst.exe".into(), params: "/S".into() }], "the registered uninstaller runs, waited for");
        assert_eq!(page.removed(), Some(Err("Not launched: test executor.".into())), "the shared uninstaller's outcome");
        assert_eq!(page.finished_jobs(), [ID]);
    }

    #[test]
    fn a_store_entry_no_catalog_handles_is_removed_for_this_account() {
        let mut page = Page::new("msix", no_entry);
        page.act(Command::Uninstall(package("Calculator", "MSIX\\Calc_1.0_x64__abc", "1.0", "", "")));
        until("the uninstall ends", || page.removed().is_some());
        assert!(matches!(page.fallback().as_slice(), [Launch::PowerShell { args, .. }] if args == &[("Package".to_string(), "Calc_1.0_x64__abc".to_string())]));
    }

    #[test]
    fn an_entry_gone_from_the_registry_starts_nothing() {
        let mut page = Page::new("gone", no_entry);
        page.act(Command::Uninstall(package("Old Tool", "ARP\\User\\X64\\OldTool", "1.0", "", "")));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty());
        assert_eq!(page.removed(), Some(Err(uninstaller::not_registered("Old Tool"))));
    }

    #[test]
    fn stop_waiting_ends_the_wait_for_a_held_uninstaller() {
        let mut page = Page::new("stopwait", old_tool_entry);
        page.runner.hold();
        page.act(Command::Uninstall(package("Old Tool", "ARP\\Machine\\X64\\OldTool", "1.0", "", "")));
        until("waiting", || page.state().installed.removal.is_some_and(|r| r.waiting));
        page.act(Command::StopWaiting);
        until("the uninstall ends", || page.removed().is_some());
        page.runner.release();
        assert_eq!(page.runner.stopped(), 1, "the wait ended early");
    }

    #[test]
    fn after_an_update_or_an_uninstall_the_list_is_read_again() {
        let mut page = Page::new("relist", old_tool_entry);
        page.listed(&list());
        page.runner.prepare(When::Arg("upgrade".into()), 0, "");
        page.act(Command::Update(vlc().update()));
        until("the series ends", || page.series_done());
        page.runner.prepare(When::Arg("list".into()), 0, &table(&[("VLC media player", "VideoLAN.VLC", "3.0.21", "", "winget")]));
        page.host.tell_package_changes();
        until("read again", || page.state().installed.updates().is_empty() && matches!(page.state().installed.listing, Listing::Read { .. }));
        assert_eq!(page.full_lists(), 2, "the update's own check is not a listing");

        page.act(Command::Uninstall(package("Old Tool", "ARP\\Machine\\X64\\OldTool", "1.0", "", "")));
        until("the uninstall ends", || page.removed().is_some());
        page.runner.prepare(When::Arg("list".into()), 0, &table(&[]));
        page.host.tell_package_changes();
        until("read a third time", || page.full_lists() == 3 && page.state().installed.listing != Listing::Reading);
    }

    /// A made-up product code (never a real product's: the shared
    /// uninstaller checks the real registry afterwards).
    const FZF_CODE: &str = "{00000118-0000-4E7C-9AAA-000000000118}";

    /// fzf from the winget catalog: a machine-wide MSI.
    fn fzf() -> Package {
        package("fzf", "junegunn.fzf", "0.56.3", "", "winget")
    }

    /// `winget list --id junegunn.fzf --exact --details` naming its
    /// registry entry `local_id`.
    fn fzf_details(local_id: &str) -> String {
        format!("Found fzf [junegunn.fzf]\nVersion: 0.56.3\nPublisher: junegunn\nLocal Identifier: {local_id}\nProduct Code: {FZF_CODE}\nInstaller Category: msi\nInstalled Scope: Machine\n")
    }

    fn fzf_entry(key: &str) -> Option<uninstaller::Target> {
        (key == format!("HKLM\\{FZF_CODE}")).then(|| uninstaller::Target::Registry { key: key.into(), command: format!("MsiExec.exe /I{FZF_CODE}"), msi_code: FZF_CODE.into() })
    }

    /// winget fails to run fzf's uninstaller (`code`); its details name
    /// its machine-wide entry.
    fn winget_fails_fzf(page: &Page, code: i32) {
        page.runner.prepare(When::Arg("uninstall".into()), code, "");
        page.runner.prepare(When::Arg("--details".into()), 0, &fzf_details(&format!("ARP\\Machine\\X64\\{FZF_CODE}")));
    }

    fn msiexec_x() -> Launch {
        Launch::Program { file: "MsiExec.exe".into(), params: format!("/X{FZF_CODE} /passive /promptrestart") }
    }

    #[test]
    fn a_machine_wide_msi_winget_cannot_uninstall_without_admin_rights_runs_its_own_installer_which_asks() {
        let mut page = Page::new("msi-fallback", fzf_entry);
        winget_fails_fzf(&page, UNINSTALL_COMMAND_FAILED);
        page.runner.prepare(When::Arg(format!("/X{FZF_CODE}")), 0, "");
        page.runner.prepare(When::Arg("--exact".into()), NO_PACKAGE_FOUND, "");
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.winget("uninstall").len(), 1, "winget tried first");
        assert_eq!(
            page.winget("list"),
            [
                vec!["list", "--id", "junegunn.fzf", "--exact", "--details", "--disable-interactivity", "--accept-source-agreements"],
                vec!["list", "--id", "junegunn.fzf", "--exact", "--disable-interactivity", "--accept-source-agreements"],
            ],
            "its registry entry asked for, then checked gone"
        );
        assert_eq!(page.fallback(), [msiexec_x()], "the MSI runs with its own progress window, so it can ask for administrator rights");
        assert_eq!(page.removed(), Some(Ok("fzf was uninstalled.".into())));
        assert_eq!(page.finished_jobs(), [ID], "one queue job");
    }

    #[test]
    fn as_administrator_or_for_another_failure_winget_s_outcome_stands() {
        let failed = |code: i32| Some(Err(format!("fzf could not be uninstalled: {}.", describe(code))));

        let mut page = Page::with("msi-admin", fzf_entry, true);
        winget_fails_fzf(&page, UNINSTALL_COMMAND_FAILED);
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty(), "nothing else runs");
        assert_eq!(page.removed(), failed(UNINSTALL_COMMAND_FAILED));

        let mut page = Page::new("msi-other", fzf_entry);
        winget_fails_fzf(&page, 1603);
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty(), "nothing else runs");
        assert!(!page.winget("list").iter().any(|a| a.contains(&"--details".to_string())), "no entry asked for");
        assert_eq!(page.removed(), failed(1603));
    }

    #[test]
    fn closing_the_administrator_prompt_of_the_fallback_cancels_the_uninstall() {
        let mut page = Page::new("msi-cancel", fzf_entry);
        winget_fails_fzf(&page, UNINSTALL_COMMAND_FAILED);
        page.runner.prepare(When::Arg(format!("/X{FZF_CODE}")), 1602, "");
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert_eq!(page.fallback(), [msiexec_x()]);
        assert_eq!(page.removed(), Some(Err(uninstaller::cancelled("fzf"))));
        assert_eq!(page.winget("list").len(), 1, "a cancelled uninstall is not checked gone");
    }

    #[test]
    fn without_a_machine_wide_entry_the_failure_says_administrator_rights_are_needed() {
        let failed = Some(Err(format!("fzf could not be uninstalled: {}. {ADMIN_HINT}", describe(UNINSTALL_COMMAND_FAILED))));

        let mut page = Page::new("msi-noentry", no_entry);
        winget_fails_fzf(&page, UNINSTALL_COMMAND_FAILED);
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty(), "the entry is not in the registry");
        assert_eq!(page.removed(), failed);

        let mut page = Page::new("msi-user", fzf_entry);
        page.runner.prepare(When::Arg("uninstall".into()), UNINSTALL_COMMAND_FAILED, "");
        page.runner.prepare(When::Arg("--details".into()), 0, &fzf_details(&format!("ARP\\User\\X64\\{FZF_CODE}")));
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty(), "a per-user entry is not the machine-wide one");
        assert_eq!(page.removed(), failed);

        let mut page = Page::new("msi-nodetails", fzf_entry);
        page.runner.prepare(When::Arg("uninstall".into()), UNINSTALL_COMMAND_FAILED, "");
        page.act(Command::Uninstall(fzf()));
        until("the uninstall ends", || page.removed().is_some());
        assert!(page.fallback().is_empty(), "winget named no entry");
        assert_eq!(page.removed(), failed);
    }

    #[test]
    fn winget_arp_ids_name_their_uninstall_registry_key() {
        assert_eq!(registry_key("ARP\\Machine\\X64\\{ABC}"), Some("HKLM\\{ABC}".into()));
        assert_eq!(registry_key("ARP\\Machine\\X86\\Tool"), Some("HKLM32\\Tool".into()));
        assert_eq!(registry_key("ARP\\User\\X64\\Tool"), Some("HKCU\\Tool".into()));
        assert_eq!(registry_key("VideoLAN.VLC"), None);
        assert_eq!(registry_key("ARP\\Machine\\X64\\"), None);
    }

    /// Every text the page shows in one frame, a line each.
    fn page_text(host: &mut AddonHost) -> String {
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
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                host.ui(ID, ui, &mut PageContext::new(&settings, &sample));
            });
        });
        let mut out = Vec::new();
        output.shapes.iter().for_each(|c| collect(&c.shape, &mut out));
        out.join("\n")
    }

    #[test]
    fn the_page_offers_the_updates_and_an_uninstall_for_every_package() {
        let mut page = Page::new("draw", no_entry);
        page.runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        page.runner.prepare(When::Arg("list".into()), 0, &list());
        page.act(Command::ListInstalled);
        until("winget is ready and the list read", || matches!(page.state().check, super::super::Check::Known(_)) && matches!(page.state().installed.listing, Listing::Read { .. }));
        let text = page_text(&mut page.host);
        for words in ["Installed packages", "Update all (2)", "3.0.21", "Old Tool"] {
            assert!(text.contains(words), "the page shows {words:?}: {text}");
        }
        assert_eq!(text.matches("Uninstall").count(), 5, "one per package: {text}");
        assert_eq!(text.lines().filter(|l| *l == "Update").count(), 2);
    }

    #[test]
    fn the_installed_list_names_its_columns() {
        let mut page = Page::new("columns", no_entry);
        page.runner.prepare(When::Arg("--version".into()), 0, "v1.9.25200");
        page.runner.prepare(When::Arg("list".into()), 0, &list());
        page.act(Command::ListInstalled);
        until("winget is ready and the list read", || matches!(page.state().check, super::super::Check::Known(_)) && matches!(page.state().installed.listing, Listing::Read { .. }));
        let text = page_text(&mut page.host);
        for heading in ["Name", "Id", "Version", "Available"] {
            assert!(text.lines().any(|l| l == heading), "the list has a {heading:?} column: {text}");
        }
    }

    /// On a real PC with winget, not as administrator: installs junegunn.fzf
    /// (a machine-wide MSI) when it is not there, then uninstalls it the way
    /// the page does, so the shared uninstaller's fallback runs and the MSI
    /// asks for UAC (accept it). Never touches fzf when it was there before.
    /// Run by hand: `cargo test real_page_uninstall -- --ignored`.
    #[test]
    #[ignore = "installs and uninstalls a real package and shows a UAC prompt"]
    fn real_page_uninstall_of_a_machine_wide_msi_falls_back_on_its_prompt() {
        let context = AddonContext::system();
        let never = AtomicBool::new(false);
        let source = catalog::source_of("winget").expect("winget is registered");
        let id = "junegunn.fzf";
        if source.is_installed(&context, "winget", id, true, &never) != Some(false) {
            eprintln!("{id} was already installed (or winget did not answer): not touched");
            return;
        }
        let logs = std::env::temp_dir().join(format!("upcm-pm-real-uninstall-{}", std::process::id()));
        std::fs::create_dir_all(&logs).unwrap();
        let log = logs.join("install.log");
        let target = Target { catalog: "winget", id, log: &log, accept_agreements: true, pin: &catalog::Pin::default() };
        assert_eq!(source.install(&context, &target, &never, &mut |_| {}), Ran::Exited(0));
        assert_eq!(source.is_installed(&context, "winget", id, true, &never), Some(true));
        let package = Package { name: "fzf".into(), id: id.into(), version: String::new(), available: String::new(), catalog: "winget".into() };
        let uninstaller = Uninstaller::new(context.clone());
        let mut shared = |t: &uninstaller::Target| uninstaller.uninstall("fzf", t, |_| {});
        let elevating = (!AddonContext::is_admin(&context)).then(|| Elevating { entry_of: uninstaller::registry_entry, uninstall: &mut shared });
        let outcome = uninstall_with(&context, source, &package, &logs, true, elevating);
        assert_eq!(outcome, Ok("fzf was uninstalled.".to_string()));
        assert_eq!(source.is_installed(&context, "winget", id, true, &never), Some(false));
        let _ = std::fs::remove_dir_all(&logs);
    }
}
