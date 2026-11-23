//! The add-on platform: a windowless host that owns the registry of every
//! page (core pages as fixed entries, add-ons as entries that can be turned
//! off) and their on/off state. The egui code only reads from here, and
//! reaches an add-on's page, Refresh, Summary tile and Settings section only
//! through the host, so an add-on that is off gets none of them. Each
//! migrated add-on lives in `addons/<id>.rs`.

use crate::platform::{AddonContext, PageContext};
use crate::sampler::Snapshot;
use std::collections::HashMap;

/// Quick CPU, memory and disk speed tests.
pub mod benchmarks;
/// Open TCP connections and the processes that own them.
pub mod connections;
/// Space used on every drive and where it goes.
pub mod disk_space;
/// Installed drivers and their updates.
pub mod drivers;
/// A pretend add-on that puts the contract tests on a new add-on.
#[cfg(test)]
pub mod fake;
/// Records the performance samples to a CSV file.
pub mod flight_recorder;
/// Installed programs, their updates and uninstalling them.
pub mod installed_apps;
/// Sensor interests that last while a page is drawn, kept by the host.
pub mod leases;
/// Settings › Add-ons as data.
pub mod overview;
/// Installs, updates and removes programs from the winget catalogs.
pub mod package_manager;
/// Hardware sensors: CPU, GPU, NPU, disks, power and battery.
pub mod power_freq;

/// Whether an add-on is free or will be sold (only declared for now).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Free,
    /// Kept for a future paid add-on; no add-on uses it today.
    #[allow(dead_code)]
    Paid,
}

/// Tier lines of features that are not add-ons (Power & Freq's alerts and
/// pins), by feature id. An add-on's own tier is its row in [`ADDONS`].
/// Every feature is free for now (the project lives on donations).
pub const FEATURE_TIERS: &[(&str, Tier)] = &[
    // Power & Freq's threshold alerts (#194).
    ("power-freq.alerts", Tier::Free),
    // Power & Freq's pinned sensors (#193).
    ("power-freq.pins", Tier::Free),
];

/// The tier of the add-on or feature `id` (Free when it has no line): an
/// add-on's row in [`ADDONS`], else a line of [`FEATURE_TIERS`].
pub fn tier_of(id: &str) -> Tier {
    ADDONS
        .iter()
        .map(|(i, tier, _)| (*i, *tier))
        .chain(FEATURE_TIERS.iter().copied())
        .find(|(i, _)| *i == id)
        .map_or(Tier::Free, |(_, t)| t)
}

impl Tier {
    /// The tier's shown word.
    pub fn label(self) -> &'static str {
        match self {
            Tier::Free => "Free",
            Tier::Paid => "Paid",
        }
    }
}

/// What an add-on needs from the PC.
#[derive(Clone, Copy, Debug, Default)]
pub struct Requirements {
    /// Some actions ask for administrator rights.
    pub admin: bool,
    /// Contacts services on the internet.
    pub network: bool,
    /// External programs it launches (PowerShell, winget, ...).
    pub programs: &'static [&'static str],
    /// A warning shown under the requirements, if any.
    pub note: Option<&'static str>,
}

/// The resource use an add-on declares, in words.
#[derive(Clone, Copy, Debug)]
pub struct Consumption {
    pub cpu: &'static str,
    pub network: &'static str,
    pub disk: &'static str,
    /// When it does its work ("only while the page is open", ...).
    pub when: &'static str,
}

/// The interface every add-on implements.
pub trait Addon {
    /// Stable, lowercase identifier (saved in settings); never the shown name.
    fn id(&self) -> &'static str;
    /// The name shown in the menu, the page title and Settings › Add-ons.
    fn name(&self) -> &'static str;
    /// One line telling what it is for.
    fn description(&self) -> &'static str;
    /// What it needs from the PC (shown before turning it on).
    fn requirements(&self) -> Requirements;
    /// The resources it uses, in words.
    fn consumption(&self) -> Consumption;
    /// Free or paid: its row in [`ADDONS`]. Only test add-ons override it.
    fn tier(&self) -> Tier {
        tier_of(self.id())
    }
    /// Called when the add-on is turned on; must be cheap and launch no
    /// program (heavy work starts when its page opens or on a user action).
    fn start(&mut self, _context: &AddonContext) {}
    /// Called when the add-on is turned off; stops what can be stopped and
    /// bumps its generation, so late results of older work are dropped.
    fn stop(&mut self) {}
    /// An operation that must not be cut short (install, uninstall,
    /// update) is running. Turning the add-on off then waits for its end
    /// ("turns off when done"); see [`AddonHost::is_stopping`].
    fn uninterruptible(&self) -> bool {
        false
    }
    /// How many threads of its own run now (0 by default). Contract-tested:
    /// none once it is off.
    #[cfg_attr(not(test), allow(dead_code))] // read by the contract tests
    fn threads(&self) -> usize {
        0
    }
    /// Draws its page below the header the app draws (title, Refresh).
    fn ui(&mut self, ui: &mut egui::Ui, page: &mut PageContext);
    /// "Refresh" while its page is open (optional).
    fn refresh(&mut self) {}
    /// Its tile on Summary, below the core cards (optional; none by default).
    fn summary(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// Its section in Settings (optional; none by default). Values it keeps
    /// stay saved while it is off.
    fn settings_section(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// Called once for every new performance sample while the add-on is
    /// on, whether its page is open or not (optional). Short work only: it
    /// runs on the UI thread and must stay within [`SAMPLE_HOOK_BUDGET`].
    fn on_sample(&mut self, _sample: &Snapshot) {}
    /// Its controls in the status bar's area on the right (optional; none
    /// by default), drawn right to left. Only drawn while it is on.
    fn status_bar(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// The title of its menu in the page header (optional; none by
    /// default), e.g. "Disk Space".
    fn menu_title(&self) -> Option<&'static str> {
        None
    }
    /// The items of that menu, drawn inside it while it is open.
    fn header_menu(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// That menu sits in the app's menu bar (after View) instead of the
    /// page header, still only while its page is open (optional; the
    /// header by default), e.g. "Benchmarks".
    fn menu_in_menu_bar(&self) -> bool {
        false
    }
    /// The line next to its page title (optional; none by default), e.g.
    /// "Connection Management".
    fn subtitle(&self) -> Option<&'static str> {
        None
    }
    /// Its status in the page header, left of the header's buttons
    /// (optional; nothing by default), e.g. the inventory line.
    fn header_status(&mut self, _ui: &mut egui::Ui, _page: &mut PageContext) {}
    /// Called once when its page, drawn in the last frame, is not drawn any
    /// more (another page opened). Drop here what only a visible page
    /// needs. Not for sensors: the host drops a page's sensor interest
    /// itself (see [`AddonContext::sensors`]).
    fn page_hidden(&mut self) {}
    /// Called once when Summary, drawn with its tile in the last frame, is
    /// not drawn any more. Drop here what only a visible tile needs.
    fn summary_hidden(&mut self) {}
    /// Other names `--page` accepts for it (e.g. a longer page id).
    fn aliases(&self) -> &'static [&'static str] {
        &[]
    }
    /// The app's command-line arguments, once at start-up and only while
    /// it is on (optional), for flags it owns (e.g. Disk Space's `--scan`).
    /// Returns true when its flags ask for its page, which then opens
    /// unless `--page` names another one.
    fn command_line(&mut self, _args: &[String]) -> bool {
        false
    }
    /// "Packages changed": a job of the install queue (see
    /// [`AddonContext::install_queue`]) ended; `by` is the id of the add-on
    /// that ran it (it may be this one). Told only while it is on, once per
    /// job; reload here what lists installed software (optional).
    fn packages_changed(&mut self, _by: &'static str) {}
    /// The capability it offers when `asked` for it (optional); none by
    /// default. The host asks one capability at a time, so an add-on can
    /// offer several: it answers each it offers with itself and the others
    /// with `None`. Whether an add-on counts as offering it is decided once,
    /// by the lookup that asks (see [`Capability`]).
    fn offers(&mut self, _asked: Asked) -> Option<Capability<'_>> {
        None
    }
    /// Its own settings, opaque to the host (optional). `Null`, the
    /// default, means it keeps none of its own and the host keeps the value
    /// last given to it. Saved with the add-on's entry, kept while it is off.
    fn settings(&self) -> serde_json::Value {
        serde_json::Value::Null
    }
    /// Hands it its saved settings (optional): once at start-up, whether it
    /// is on or off, and on [`AddonHost::set_settings`]. Settings it cannot
    /// read give its defaults (see [`read_settings`]).
    fn load_settings(&mut self, _settings: &serde_json::Value) {}
}

/// Lists the capabilities an add-on can offer to the others once: each gets a
/// variant of [`Asked`] (what the host asks for) and of [`Capability`] (what an
/// add-on lends when it is asked), so a capability is never named twice.
macro_rules! capabilities {
    ($($(#[$doc:meta])* $name:ident($offered:path),)*) => {
        /// What the host asks an add-on to offer (see [`Addon::offers`]).
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Asked {
            $($(#[$doc])* $name,)*
        }

        /// What an add-on lends of itself when it is [`Asked`] for a
        /// capability: the variant is the one asked for.
        pub enum Capability<'a> {
            $($name(&'a mut dyn $offered),)*
        }
    };
}

// To add a capability: a trait for it, one line here, a host method that asks
// `provider` (or `providers`) for it, and the add-ons that offer it answering
// that `Asked` in `Addon::offers`.
capabilities! {
    /// Recording sensors for other add-ons' pages (Flight Recorder).
    SensorRecorder(SensorRecorder),
    /// A column in the Processes table and an item in a process's menu
    /// (Connections).
    ProcessColumn(ProcessColumnProvider),
    /// Owning paths, so other add-ons' pages can link to its row (Installed
    /// Apps).
    PathOwner(PathOwner),
    /// Holding work that turning it off or closing the app would lose
    /// (Flight Recorder's recording).
    UnsavedWork(UnsavedWork),
    /// A reason to wait before the app closes (Package Manager's series of
    /// installs).
    CloseWait(CloseWait),
}

/// The Processes column capability: a column in the Processes table and an
/// item in a process's menu, only while the add-on that offers it is on.
pub trait ProcessColumnProvider {
    /// Its column and menu item; keep it cheap.
    fn process_column(&self) -> ProcessColumn;
    /// The figure of each process (by PID) in that column, asked once per
    /// frame while the column shows: it may start reading then (see
    /// [`ProcessColumnProvider::process_column_hidden`]). None until it has
    /// a reading.
    fn process_figures(&mut self, settings: &crate::settings::Settings) -> Option<HashMap<u32, usize>>;
    /// Called once when its column, drawn in the last frame, is not drawn
    /// any more (hidden, or another page opened). Drop here what only the
    /// column needs.
    fn process_column_hidden(&mut self);
    /// Opens its page on the process `pid` (its menu item was chosen); the
    /// app then opens the page.
    fn reveal_process(&mut self, pid: u32);
}

/// The owner-of-paths capability: what Disk Space's link to a row talks to.
pub trait PathOwner {
    /// The name of its row that owns `path` (e.g. the installed app whose
    /// folder holds it). Asked while it is on; keep it cheap.
    fn owner_of(&self, path: &std::path::Path) -> Option<String>;
    /// Shows the row that owns `path` on its page (after
    /// [`PathOwner::owner_of`] named it); the app then opens the page.
    fn reveal(&mut self, path: &std::path::Path);
}

/// The unsaved work capability: what the app asks about before it turns
/// the add-on off or closes.
pub trait UnsavedWork {
    /// Work that turning it off would lose (e.g. a recording running):
    /// the question the app asks first, with Save / Discard / Cancel (see
    /// [`AddonHost::off_question`]). None: nothing would be lost.
    fn unsaved_work(&self) -> Option<&'static str>;
    /// Save on that question: keeps the work (e.g. writes it to a file).
    /// False when it was not saved (dialog cancelled, write failed): the
    /// add-on then stays on with its work.
    fn save_work(&mut self) -> bool;
}

/// The reason to wait before the app closes while the add-on works (e.g. a
/// series of installs).
pub trait CloseWait {
    /// What the app asks before it closes; `None` closes without asking.
    /// See [`AddonHost::close_questions`].
    fn close_reason(&self) -> Option<String>;
}

/// The sensor recorder capability: what Power & Freq's "Record" talks to.
pub trait SensorRecorder {
    /// What it can do now at the update speed `interval_ms`; keep it cheap.
    fn sensor_recording(&self, interval_ms: u64) -> Recording;
    /// Starts (`on`) or stops its sensor recording for another add-on's
    /// page, through its own Record and Stop (see
    /// [`AddonHost::record_sensors`]).
    fn set_sensor_recording(&mut self, on: bool, page: &mut PageContext) -> Result<(), &'static str>;
}

/// What the add-on that records sensors for other pages (Flight Recorder)
/// can do now, as the host tells another add-on's page (see
/// [`SensorRecorder::sensor_recording`] and [`PageContext::recording`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Recording {
    /// It is off (or stopping, or not registered): nothing can be recorded.
    #[default]
    Off,
    /// It is on but cannot start a recording now, for this reason.
    Locked(&'static str),
    /// It can start a recording.
    Ready,
    /// A recording runs.
    Running,
}

/// Said when [`AddonHost::record_sensors`] finds no add-on that records
/// sensors on. Generic: the host does not know which add-on records (a page
/// that names it says so in its own words).
pub const RECORDING_OFF: &str = "No add-on that records sensors is on: turn one on in Settings \u{203a} Add-ons to record.";

/// An add-on's settings read as its type `T`: settings it cannot read (or
/// none saved) give `T::default()`, for this add-on only.
pub fn read_settings<T: serde::de::DeserializeOwned + Default>(settings: &serde_json::Value) -> T {
    T::deserialize(settings).unwrap_or_default()
}

/// An add-on's settings written from its type `T` (which leaves default
/// values out): `Null` when nothing is left, so an add-on nobody changed
/// saves nothing.
pub fn write_settings<T: serde::Serialize>(settings: &T) -> serde_json::Value {
    match serde_json::to_value(settings) {
        Ok(serde_json::Value::Object(map)) if map.is_empty() => serde_json::Value::Null,
        Ok(v) => v,
        Err(_) => serde_json::Value::Null,
    }
}

/// The value is its type's default (a field [`write_settings`] leaves
/// out, with `#[serde(skip_serializing_if = "is_default")]`).
pub fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// Whether a setting that is on by default is still on (to leave it out
/// of the saved settings, as [`is_default`] does for the others).
pub fn is_true(v: &bool) -> bool {
    *v
}

/// "1 app", "3 apps": `n` and `word`, with an "s" unless `n` is 1.
pub fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The lines of a Summary tile that tells the outcome of a check the user
/// made: what was found, then when ("Last checked at 14:03").
pub fn checked_tile_lines(found: String, checked_at: &str) -> Vec<String> {
    vec![found, format!("Last checked at {checked_at}")]
}

/// Draws an add-on's Summary tile: its `title`, then `lines` in the
/// secondary text colour.
pub fn summary_tile(ui: &mut egui::Ui, page: &PageContext, title: &str, lines: Vec<String>) {
    let sub = page.sub_text();
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title).strong());
    for line in lines {
        ui.label(egui::RichText::new(line).color(sub));
    }
}

/// Waits (up to 20 s, then fails the test) until `done` is true.
#[cfg(test)]
pub fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done() {
        assert!(std::time::Instant::now() < end, "timed out waiting: {what}");
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Draws the page `id` of `host` for one windowless frame over `sample`,
/// then ends the frame (what the app does every frame).
#[cfg(test)]
pub fn draw_page_with(host: &mut AddonHost, id: &str, sample: &crate::sampler::Snapshot) {
    let settings = crate::settings::Settings::default();
    let ctx = egui::Context::default();
    // The app's faces, as at start-up: pages draw in them.
    crate::theme::install_fonts(&ctx);
    let _ = ctx.run(egui::RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            host.ui(id, ui, &mut PageContext::new(&settings, sample));
        });
    });
    host.end_frame();
}

/// Sensor threads alive in the shared service of `host`.
#[cfg(test)]
pub fn sensor_threads(host: &AddonHost) -> usize {
    host.context.sensors().threads()
}

/// The longest an add-on's [`Addon::on_sample`] may take (contract-tested).
#[cfg_attr(not(test), allow(dead_code))] // enforced by the contract tests
pub const SAMPLE_HOOK_BUDGET: std::time::Duration = std::time::Duration::from_millis(1);

/// A core page: always on, drawn by the app itself. The only place its id
/// and shown name are written down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Core {
    Summary,
    Performance,
    Processes,
    SystemInfo,
    AppHistory,
    Startup,
    Users,
    Services,
    Settings,
    Colors,
}

impl Core {
    /// Every core page, in registry order.
    pub const ALL: [Core; 10] = [
        Core::Summary,
        Core::Performance,
        Core::Processes,
        Core::SystemInfo,
        Core::AppHistory,
        Core::Startup,
        Core::Users,
        Core::Services,
        Core::Settings,
        Core::Colors,
    ];

    /// Stable identifier (saved in settings).
    pub const fn id(self) -> &'static str {
        match self {
            Core::Summary => "summary",
            Core::Performance => "performance",
            Core::Processes => "processes",
            Core::SystemInfo => "system-info",
            Core::AppHistory => "app-history",
            Core::Startup => "startup-apps",
            Core::Users => "users",
            Core::Services => "services",
            Core::Settings => "settings",
            Core::Colors => "colors",
        }
    }

    /// The shown name.
    pub fn name(self) -> &'static str {
        match self {
            Core::Summary => "Summary",
            Core::Performance => "Performance",
            Core::Processes => "Processes",
            Core::SystemInfo => "System Info",
            Core::AppHistory => "App history",
            Core::Startup => "Startup apps",
            Core::Users => "Users",
            Core::Services => "Services",
            Core::Settings => "Settings",
            Core::Colors => "Colors",
        }
    }

    /// The core page with this id, if any.
    pub fn of(id: &str) -> Option<Core> {
        Core::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// One page of the registry.
pub enum Entry {
    /// A core page: always on.
    Core(Core),
    /// An add-on: can be turned off.
    Addon(Box<dyn Addon>),
}

impl Entry {
    /// The page's stable identifier.
    pub fn id(&self) -> &'static str {
        match self {
            Entry::Core(core) => core.id(),
            Entry::Addon(a) => a.id(),
        }
    }

    /// The page's shown name.
    pub fn name(&self) -> &'static str {
        match self {
            Entry::Core(core) => core.name(),
            Entry::Addon(a) => a.name(),
        }
    }

    /// Other names a page request may use.
    pub fn aliases(&self) -> &'static [&'static str] {
        match self {
            Entry::Core(_) => &[],
            Entry::Addon(a) => a.aliases(),
        }
    }
}

/// Every add-on, in menu order: its id (the one [`Addon::id`] gives), its tier
/// and how to make it. Adding an add-on is one line here. Every add-on is free
/// for now.
const ADDONS: &[(&str, Tier, fn() -> Box<dyn Addon>)] = &[
    ("power-freq", Tier::Free, || Box::<power_freq::PowerFreq>::default()),
    ("flight-recorder", Tier::Free, || Box::<flight_recorder::FlightRecorder>::default()),
    ("connections", Tier::Free, || Box::<connections::Connections>::default()),
    ("installed-apps", Tier::Free, || Box::<installed_apps::InstalledApps>::default()),
    ("drivers", Tier::Free, || Box::<drivers::Drivers>::default()),
    ("disk-space", Tier::Free, || Box::<disk_space::DiskSpace>::default()),
    ("benchmarks", Tier::Free, || Box::<benchmarks::Benchmarks>::default()),
    ("package-manager", Tier::Free, || Box::<package_manager::PackageManager>::default()),
];

/// The ordered registry of every page: the core, then the add-ons.
pub fn registry() -> Vec<Entry> {
    let mut entries: Vec<Entry> = Core::ALL.into_iter().map(Entry::Core).collect();
    entries.extend(ADDONS.iter().map(|(_, _, make)| Entry::Addon(make())));
    entries
}

/// One registry entry with its on/off state and saved settings.
struct Slot {
    entry: Entry,
    enabled: bool,
    /// Turned off during an uninterruptible operation: off in every way
    /// (no hook, capability or new work; its page stays drawn, without
    /// sensors, to follow the operation) but waiting for that operation to
    /// end. The work it still holds counts until then (see
    /// [`providers_until_off`]).
    stopping: bool,
    /// The add-on's own settings, opaque to the host (`Null` for core pages
    /// and add-ons that saved none).
    settings: serde_json::Value,
}

/// An add-on that offers a capability, as the lookup answers.
struct Provider<'s> {
    id: &'static str,
    /// The add-on's shown name.
    name: &'static str,
    capability: Capability<'s>,
}

/// THE place that decides whether a capability counts: it counts only while
/// its add-on is on and not stopping. Every use of a capability goes through
/// this lookup (or [`providers_until_off`], below). The add-ons among `slots`
/// that offer what is `asked`, in registry order; a disabled or stopping
/// add-on offers nothing.
fn providers<'s>(slots: impl IntoIterator<Item = &'s mut Slot>, asked: Asked) -> impl Iterator<Item = Provider<'s>> {
    offering(slots, asked, false)
}

/// The first of [`providers`]: the add-on that answers for `asked`.
fn provider<'s>(slots: impl IntoIterator<Item = &'s mut Slot>, asked: Asked) -> Option<Provider<'s>> {
    providers(slots, asked).next()
}

/// The documented exception to "not stopping", for the capabilities that
/// speak for work still held: [`UnsavedWork`] (the question before turning
/// off or closing, its answers) and [`CloseWait`] (the reason to wait before
/// closing). An add-on turned off during an uninterruptible operation is
/// stopping but still on, holds that work and the app still has to ask about
/// it before closing (pinned by the close tests), so these count until it is
/// off for good.
fn providers_until_off<'s>(slots: impl IntoIterator<Item = &'s mut Slot>, asked: Asked) -> impl Iterator<Item = Provider<'s>> {
    offering(slots, asked, true)
}

/// The one implementation of the rule behind [`providers`] and
/// [`providers_until_off`].
fn offering<'s>(slots: impl IntoIterator<Item = &'s mut Slot>, asked: Asked, until_off: bool) -> impl Iterator<Item = Provider<'s>> {
    slots.into_iter().filter(move |s| s.enabled && (until_off || !s.stopping)).filter_map(move |s| match &mut s.entry {
        Entry::Addon(a) => {
            let (id, name) = (a.id(), a.name());
            a.offers(asked).map(|capability| Provider { id, name, capability })
        }
        Entry::Core(_) => None,
    })
}

/// The one other exception: asks an add-on that is off whatever its state,
/// for the notice that tells a column it is no longer drawn (the add-on may
/// have been turned off in this very frame, so no lookup would reach it).
fn ask_directly(addon: &mut dyn Addon, asked: Asked) -> Option<Capability<'_>> {
    addon.offers(asked)
}

/// The question about the unsaved work of the add-on in `slot`, which is
/// being turned off or closed with (see [`providers_until_off`]).
fn unsaved_work_of(slot: &mut Slot) -> Option<&'static str> {
    providers_until_off(std::iter::once(slot), Asked::UnsavedWork).find_map(|p| match p.capability {
        Capability::UnsavedWork(work) => work.unsaved_work(),
        _ => None,
    })
}

/// Saves the unsaved work of the add-on in `slot` (true when there is none to
/// save or it was saved).
fn save_work_of(slot: &mut Slot) -> bool {
    providers_until_off(std::iter::once(slot), Asked::UnsavedWork)
        .find_map(|p| match p.capability {
            Capability::UnsavedWork(work) => Some(work.save_work()),
            _ => None,
        })
        .unwrap_or(true)
}

/// The page every invalid request falls back to.
pub const SUMMARY: &str = Core::Summary.id();
/// The start page value meaning "reopen the last page".
pub const LAST_USED: &str = "Last used";
/// The `--page` value that opens Settings › Add-ons.
pub const ADDONS_PAGE: &str = "Add-ons";

/// What the command line did to the add-ons.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct CommandLine {
    /// The add-ons it reached (those that are on), in registry order.
    pub reached: Vec<&'static str>,
    /// The first add-on whose flags ask for its page (e.g. Drivers'
    /// `--drivers-scope`), opened when no `--page` is given.
    pub page: Option<&'static str>,
}

/// Where a page request leads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Landing {
    /// A page that can be shown, by id.
    Page(&'static str),
    /// Settings › Add-ons, with an add-on selected or none.
    AddonSettings(Option<&'static str>),
}

/// A column an add-on adds to the Processes table (an addition, hidden
/// until the user shows it), with the item it adds to a process's menu.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ProcessColumn {
    /// The column's title, e.g. the number of something per process.
    pub title: &'static str,
    /// The menu item that opens its page on a process ("Go to ...").
    pub menu_item: &'static str,
}

/// The question asked before turning off an add-on with unsaved work.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OffQuestion {
    pub id: &'static str,
    /// The add-on's shown name.
    pub name: &'static str,
    /// What would be lost (its [`UnsavedWork`]).
    pub question: &'static str,
}

/// The answer to an [`OffQuestion`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OffAnswer {
    /// Save the work, then turn off (stays on if it was not saved).
    Save,
    /// Turn off and lose the work.
    Discard,
    /// Stay on.
    Cancel,
}

/// The buttons of an [`OffQuestion`], in order.
pub const OFF_ANSWERS: [(OffAnswer, &str); 3] = [(OffAnswer::Save, "Save"), (OffAnswer::Discard, "Discard"), (OffAnswer::Cancel, "Cancel")];

/// Owns the registry, the on/off state and the context shared with the
/// add-ons. Works without a window.
pub struct AddonHost {
    slots: Vec<Slot>,
    context: AddonContext,
    /// The add-on asked to turn off while holding unsaved work, waiting
    /// for the answer to its [`OffQuestion`].
    asking: Option<&'static str>,
    /// The app was asked to close while add-ons held unsaved work: it
    /// waits for the answer to their [`AddonHost::close_save_question`].
    closing: bool,
    /// Add-on pages drawn in this frame and in the last one.
    shown: FrameSet,
    /// Add-ons whose Summary tile was drawn in this frame and the last one.
    tiles: FrameSet,
    /// Add-ons whose Processes column was drawn in this frame and the last one.
    columns: FrameSet,
}

/// The add-ons something was drawn for in this frame and in the last one.
#[derive(Default)]
struct FrameSet {
    now: Vec<&'static str>,
    last: Vec<&'static str>,
}

impl FrameSet {
    /// `id` was drawn in this frame.
    fn mark(&mut self, id: &'static str) {
        if !self.now.contains(&id) {
            self.now.push(id);
        }
    }

    /// Ends the frame: the ids drawn in the last frame but not in this one.
    fn end_frame(&mut self) -> Vec<&'static str> {
        let gone = self.last.iter().copied().filter(|id| !self.now.contains(id)).collect();
        self.last = std::mem::take(&mut self.now);
        gone
    }
}

impl AddonHost {
    /// The standard registry with every add-on on.
    #[cfg(test)]
    pub fn standard() -> Self {
        Self::from_section(&serde_json::Value::Null)
    }

    /// The standard registry with each add-on's state read from the
    /// settings' `addons` section (`{ id: { "enabled": bool, "settings":
    /// <opaque>, ... } }`). Add-ons not in the section are on. The add-ons
    /// that are on are started.
    pub fn from_section(section: &serde_json::Value) -> Self {
        Self::with_context(registry(), section, AddonContext::system())
    }

    /// Like [`AddonHost::from_section`], for tests: the add-ons get a context
    /// whose executor records the requests (returned too) and launches nothing.
    #[cfg(test)]
    pub fn for_tests(section: &serde_json::Value) -> (Self, std::sync::Arc<crate::platform::RecordingRunner>) {
        Self::for_tests_with(registry(), section)
    }

    /// The standard registry with every add-on on, over a context whose
    /// sensor service is `sensors` (made of fake providers).
    #[cfg(test)]
    pub fn over_sensors(sensors: crate::sensors::SensorService) -> Self {
        let (context, _) = AddonContext::recording_over(sensors);
        Self::with_context(registry(), &serde_json::json!({}), context)
    }

    /// Like [`AddonHost::for_tests`], with a registry of the test's own.
    #[cfg(test)]
    pub fn for_tests_with(entries: Vec<Entry>, section: &serde_json::Value) -> (Self, std::sync::Arc<crate::platform::RecordingRunner>) {
        let (context, runner) = AddonContext::recording();
        (Self::with_context(entries, section, context), runner)
    }

    /// The context the add-ons get (Summary's tiles and the tests use its
    /// sensor service).
    pub fn context(&self) -> &AddonContext {
        &self.context
    }

    /// The host for `entries`: each add-on gets its saved settings (on or
    /// off), then the add-ons that are on are started with `context`.
    fn with_context(entries: Vec<Entry>, section: &serde_json::Value, context: AddonContext) -> Self {
        let slots = entries
            .into_iter()
            .map(|entry| {
                let saved = section.get(entry.id()).filter(|_| matches!(entry, Entry::Addon(_)));
                let enabled = saved.and_then(|e| e.get("enabled")).and_then(|v| v.as_bool()).unwrap_or(true);
                Slot { entry, enabled, stopping: false, settings: serde_json::Value::Null }
            })
            .collect();
        let mut host = AddonHost { slots, context, asking: None, closing: false, shown: FrameSet::default(), tiles: FrameSet::default(), columns: FrameSet::default() };
        for i in 0..host.slots.len() {
            let id = host.slots[i].entry.id();
            let saved = section.get(id).and_then(|e| e.get("settings")).cloned().unwrap_or_default();
            host.set_settings(id, saved);
            let slot = &mut host.slots[i];
            if let (Entry::Addon(addon), true) = (&mut slot.entry, slot.enabled) {
                addon.start(&host.context);
            }
        }
        host
    }

    /// Writes each add-on's on/off state and settings into the `addons`
    /// section, keeping every other field and every id it does not know.
    pub fn write_section(&self, section: &mut serde_json::Value) {
        if !section.is_object() {
            *section = serde_json::Value::Object(Default::default());
        }
        let map = section.as_object_mut().expect("object");
        for slot in &self.slots {
            let Entry::Addon(addon) = &slot.entry else { continue };
            let entry = map.entry(addon.id()).or_insert_with(|| serde_json::json!({}));
            if !entry.is_object() {
                *entry = serde_json::json!({});
            }
            entry["enabled"] = serde_json::Value::Bool(slot.enabled);
            match self.settings(addon.id()).unwrap_or_default() {
                serde_json::Value::Null => {
                    if let Some(m) = entry.as_object_mut() {
                        m.remove("settings");
                    }
                }
                settings => entry["settings"] = settings,
            }
        }
    }

    /// An add-on's own settings: what it reports ([`Addon::settings`]),
    /// else what was saved for it (`Null` if none). `None` for core pages
    /// and unknown ids.
    pub fn settings(&self, id: &str) -> Option<serde_json::Value> {
        let slot = self.slot(id)?;
        let Entry::Addon(addon) = &slot.entry else { return None };
        Some(match addon.settings() {
            serde_json::Value::Null => slot.settings.clone(),
            own => own,
        })
    }

    /// Replaces an add-on's own settings and hands them to it
    /// ([`Addon::load_settings`]). Returns false for core pages and unknown
    /// ids. Saved by the next `write_section`.
    pub fn set_settings(&mut self, id: &str, settings: serde_json::Value) -> bool {
        match self.slots.iter_mut().find(|s| s.entry.id() == id) {
            Some(Slot { entry: Entry::Addon(addon), settings: kept, .. }) => {
                addon.load_settings(&settings);
                *kept = settings;
                true
            }
            _ => false,
        }
    }

    /// Takes in the settings the add-ons changed themselves (a choice on
    /// their page). Returns true if any changed, so the caller saves the
    /// section; the app calls it every frame.
    pub fn sync_settings(&mut self) -> bool {
        let mut changed = false;
        for slot in &mut self.slots {
            let Entry::Addon(addon) = &slot.entry else { continue };
            let own = addon.settings();
            if !own.is_null() && own != slot.settings {
                slot.settings = own;
                changed = true;
            }
        }
        changed
    }

    /// Every page id, in registry order.
    #[cfg(test)]
    pub fn ids(&self) -> Vec<&'static str> {
        self.slots.iter().map(|s| s.entry.id()).collect()
    }

    /// The pages to show: the core and the add-ons that are on, in order.
    pub fn visible(&self) -> Vec<&'static str> {
        self.slots.iter().filter(|s| s.enabled).map(|s| s.entry.id()).collect()
    }

    /// Whether the page can be shown. Core pages are always on; unknown ids
    /// are off; an add-on that is stopping is still on until it is off.
    pub fn is_enabled(&self, id: &str) -> bool {
        self.slot(id).is_some_and(|s| s.enabled)
    }

    /// Turns an add-on on or off, calling its start/stop. Returns false for
    /// core pages and unknown ids, which cannot change, and for turning on
    /// an add-on still waiting to turn off (see [`AddonHost::is_stopping`]).
    ///
    /// Turning off stops at once what can be stopped. If the add-on then
    /// still runs an uninterruptible operation it is stopping: it stays in
    /// the menu with its page open to follow that operation (and is saved
    /// as on) until [`AddonHost::settle`] sees the operation end.
    ///
    /// An add-on holding unsaved work ([`UnsavedWork`]) is not
    /// turned off yet: the host asks first ([`AddonHost::off_question`])
    /// and turns it off on the answer ([`AddonHost::answer_off`]).
    pub fn set_enabled(&mut self, id: &str, on: bool) -> bool {
        let Some(slot) = self.slots.iter_mut().find(|s| s.entry.id() == id) else {
            return false;
        };
        let Entry::Addon(addon) = &mut slot.entry else {
            return false;
        };
        let id = addon.id();
        if slot.stopping {
            return !on;
        }
        if self.asking == Some(id) {
            self.asking = None;
        }
        if slot.enabled != on {
            if on {
                addon.start(&self.context);
                slot.enabled = true;
            } else if unsaved_work_of(slot).is_some() {
                self.asking = Some(id);
            } else {
                Self::turn_off(slot, &self.context);
            }
        }
        true
    }

    /// Stops the add-on of `slot`: off at once, or stopping while an
    /// uninterruptible operation runs.
    fn turn_off(slot: &mut Slot, context: &AddonContext) {
        if let Entry::Addon(addon) = &mut slot.entry {
            addon.stop();
            // What its pages read from the sensors ends with it.
            context.leases().release(addon.id());
            // Its jobs still waiting for the install queue never run.
            context.install_queue().withdraw(addon.id());
            slot.stopping = addon.uninterruptible();
            slot.enabled = slot.stopping;
        }
    }

    /// The question to ask before turning off an add-on that holds unsaved
    /// work, while it waits for the answer (None once that work is gone).
    pub fn off_question(&mut self) -> Option<OffQuestion> {
        let id = self.asking?;
        let slot = self.slots.iter_mut().find(|s| s.entry.id() == id)?;
        let question = unsaved_work_of(slot)?;
        Some(OffQuestion { id, name: slot.entry.name(), question })
    }

    /// Answers the [`OffQuestion`]: Save saves the work and turns the
    /// add-on off (it stays on when the work was not saved), Discard turns
    /// it off, Cancel leaves it on. Returns true when its state changed,
    /// so the caller saves the section.
    pub fn answer_off(&mut self, answer: OffAnswer) -> bool {
        let Some(id) = self.asking.take() else { return false };
        let Some(slot) = self.slots.iter_mut().find(|s| s.entry.id() == id) else { return false };
        let off = match answer {
            OffAnswer::Save => save_work_of(slot),
            OffAnswer::Discard => true,
            OffAnswer::Cancel => false,
        };
        if off {
            Self::turn_off(slot, &self.context);
        }
        off
    }

    /// The first add-on that is on and holds unsaved work, with its question.
    fn first_unsaved(&mut self) -> Option<OffQuestion> {
        providers_until_off(self.slots.iter_mut(), Asked::UnsavedWork).find_map(|p| match p.capability {
            Capability::UnsavedWork(work) => Some(OffQuestion { id: p.id, name: p.name, question: work.unsaved_work()? }),
            _ => None,
        })
    }

    /// The app is asked to close (window X, File › Exit, Alt+F4). True
    /// when it may close now; false when an add-on holds unsaved work:
    /// the app stays open and asks first ([`AddonHost::close_save_question`]).
    /// Replaces a pending [`AddonHost::off_question`].
    pub fn request_close(&mut self) -> bool {
        self.asking = None;
        self.closing = self.first_unsaved().is_some();
        !self.closing
    }

    /// The save question to ask before closing the app: the same as turning off
    /// ([`OffQuestion`]), for the next add-on whose work would be lost.
    pub fn close_save_question(&mut self) -> Option<OffQuestion> {
        if self.closing {
            self.first_unsaved()
        } else {
            None
        }
    }

    /// Answers the [`AddonHost::close_save_question`]. Save saves the work
    /// (stays open when it was not saved), Discard stops the add-on so
    /// its work and threads are gone, Cancel stays open. The add-ons stay
    /// on (saved as they are). True when the app may close now; false
    /// while it stays open or another add-on still has to be asked.
    pub fn answer_close(&mut self, answer: OffAnswer) -> bool {
        let Some(q) = self.close_save_question() else { return false };
        let Some(slot) = self.slots.iter_mut().find(|s| s.entry.id() == q.id) else { return false };
        let done = match answer {
            OffAnswer::Save => save_work_of(slot),
            OffAnswer::Discard => {
                if let Entry::Addon(addon) = &mut slot.entry {
                    addon.stop();
                    self.context.leases().release(addon.id());
                    self.context.install_queue().withdraw(addon.id());
                }
                true
            }
            OffAnswer::Cancel => false,
        };
        self.closing = done && self.first_unsaved().is_some();
        done && !self.closing
    }

    /// The threads of its own the add-on `id` runs now (0 for core pages
    /// and unknown ids).
    #[cfg(test)]
    pub fn threads(&self, id: &str) -> usize {
        match self.slot(id).map(|s| &s.entry) {
            Some(Entry::Addon(addon)) => addon.threads(),
            _ => 0,
        }
    }

    /// The add-on was turned off during an uninterruptible operation and
    /// turns off for good when it ends ("turns off when done").
    pub fn is_stopping(&self, id: &str) -> bool {
        self.slot(id).is_some_and(|s| s.stopping)
    }

    /// Completes the stops whose uninterruptible operation has ended and
    /// returns their ids: those add-ons are now off (out of the menu, their
    /// page closed) and the caller saves the section. The app calls it
    /// every frame.
    pub fn settle(&mut self) -> Vec<&'static str> {
        if self.asking.is_some() && self.off_question().is_none() {
            // The work it asked about is gone: the question is dropped.
            self.asking = None;
        }
        let mut done = Vec::new();
        for slot in self.slots.iter_mut().filter(|s| s.stopping) {
            if let Entry::Addon(addon) = &slot.entry {
                if !addon.uninterruptible() {
                    slot.stopping = false;
                    slot.enabled = false;
                    done.push(addon.id());
                }
            }
        }
        done
    }

    /// What the add-ons that are on ask before the app closes (see
    /// [`CloseWait`]); empty: it closes without asking.
    pub fn close_questions(&mut self) -> Vec<String> {
        providers_until_off(self.slots.iter_mut(), Asked::CloseWait)
            .filter_map(|p| match p.capability {
                Capability::CloseWait(wait) => wait.close_reason(),
                _ => None,
            })
            .collect()
    }

    /// The app is closing: stops every add-on that is on, as turning it off
    /// does (an uninterruptible operation finishes its current step; see
    /// [`AddonHost::can_close`]), but keeps each one saved as on.
    pub fn close(&mut self) {
        for slot in self.slots.iter_mut().filter(|s| s.enabled && !s.stopping) {
            if let Entry::Addon(addon) = &mut slot.entry {
                addon.stop();
                self.context.leases().release(addon.id());
                self.context.install_queue().withdraw(addon.id());
            }
        }
    }

    /// No add-on runs an uninterruptible operation: the app may close.
    pub fn can_close(&self) -> bool {
        self.slots.iter().all(|s| match &s.entry {
            Entry::Addon(addon) => !addon.uninterruptible(),
            Entry::Core(_) => true,
        })
    }

    /// Tells the add-ons that are on, once per install-queue job that ended
    /// since the last call, that the installed packages changed (see
    /// [`Addon::packages_changed`]). The app calls it every frame.
    pub fn tell_package_changes(&mut self) {
        for by in self.context.install_queue().take_finished() {
            for slot in self.slots.iter_mut().filter(|s| s.enabled && !s.stopping) {
                if let Entry::Addon(addon) = &mut slot.entry {
                    addon.packages_changed(by);
                }
            }
        }
    }

    /// Whether `id` is a core page (always on).
    pub fn is_core(&self, id: &str) -> bool {
        self.slot(id).is_some_and(|s| matches!(s.entry, Entry::Core(_)))
    }

    /// Every entry with its state, in registry order.
    pub fn entries(&self) -> impl Iterator<Item = (&Entry, bool)> {
        self.slots.iter().map(|s| (&s.entry, s.enabled))
    }

    /// The add-ons with whether they are on, in registry order (one that is
    /// stopping counts as off).
    pub fn addons(&self) -> impl Iterator<Item = (&dyn Addon, bool)> {
        self.slots.iter().filter_map(|s| match &s.entry {
            Entry::Addon(a) => Some((a.as_ref(), s.enabled && !s.stopping)),
            Entry::Core(_) => None,
        })
    }

    /// The id of the page named by `s`: its id, or the shown name older
    /// versions saved.
    fn find(&self, s: &str) -> Option<&'static str> {
        self.slots.iter().map(|s| &s.entry).find(|e| e.id() == s || e.name() == s || e.aliases().contains(&s)).map(|e| e.id())
    }

    /// A saved page (start page, last page, or the page open now) as a page
    /// that can be shown: unknown pages and add-ons that are off give Summary.
    pub fn resolve_page(&self, saved: &str) -> &'static str {
        self.find(saved).filter(|id| self.is_enabled(id)).unwrap_or(SUMMARY)
    }

    /// Where the app opens. `request` is the `--page` argument; without it
    /// the start page opens, or the last page when the start page is
    /// [`LAST_USED`]. Pages are ids, or shown names saved by older versions.
    pub fn landing(&self, request: Option<&str>, start_page: &str, last_page: &str) -> Landing {
        match request {
            Some(ADDONS_PAGE) => Landing::AddonSettings(None),
            Some(r) => match self.find(r) {
                Some(id) if !self.is_enabled(id) => Landing::AddonSettings(Some(id)),
                _ => Landing::Page(self.resolve_page(r)),
            },
            None if start_page == LAST_USED => Landing::Page(self.resolve_page(last_page)),
            None => Landing::Page(self.resolve_page(start_page)),
        }
    }

    /// Refreshes an add-on that is on. Returns false (and does nothing) for
    /// add-ons that are off, core pages and unknown ids.
    pub fn refresh(&mut self, id: &str) -> bool {
        match self.enabled_addon(id) {
            Some(addon) => {
                addon.refresh();
                self.read_held_sensors();
                true
            }
            None => false,
        }
    }

    /// "Refresh": every sensor is read at once through every interest the
    /// surfaces drawn now hold (pages, Summary tiles, the core Summary).
    pub fn read_held_sensors(&self) {
        self.context.leases().read_now();
    }

    /// Draws the page of an add-on that is on. Returns false (and draws
    /// nothing) for add-ons that are off, core pages and unknown ids.
    /// A stopping add-on's page is drawn too, so the user can follow the
    /// operation it waits for.
    pub fn ui(&mut self, id: &str, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        let Some(i) = self.slots.iter().position(|s| s.enabled && matches!(&s.entry, Entry::Addon(a) if a.id() == id)) else {
            return false;
        };
        let (before, rest) = self.slots.split_at_mut(i);
        let Some((slot, after)) = rest.split_first_mut() else {
            return false;
        };
        let Entry::Addon(addon) = &mut slot.entry else {
            return false;
        };
        // The page is told what the add-on that records sensors can do now.
        let interval_ms = page.settings.interval_ms();
        let recording = match provider(before.iter_mut().chain(after.iter_mut()), Asked::SensorRecorder) {
            Some(Provider { capability: Capability::SensorRecorder(recorder), .. }) => recorder.sensor_recording(interval_ms),
            _ => Recording::Off,
        };
        // And it may ask the other add-ons that are on who owns a path.
        let others = std::cell::RefCell::new(|path: &std::path::Path| {
            providers(before.iter_mut().chain(after.iter_mut()), Asked::PathOwner).find_map(|p| match p.capability {
                Capability::PathOwner(owner) => owner.owner_of(path).map(|label| crate::platform::Owner { addon: p.id, page: p.name, label }),
                _ => None,
            })
        });
        let id = addon.id();
        let mut inner = PageContext { settings: page.settings, sample: page.sample, actions: std::mem::take(&mut page.actions), owners: Some(&others), lease: None, recording };
        // A stopping add-on is off: its page reads no sensors.
        inner.lease = (!slot.stopping).then(|| self.context.leases().lease(leases::Surface::Page(id)));
        addon.ui(ui, &mut inner);
        page.actions = inner.actions;
        self.shown.mark(id);
        true
    }

    /// Draws the header menu of the add-on `id` (see [`Addon::menu_title`]).
    /// False (and nothing drawn) when it is off, has none, is core or unknown.
    pub fn header_menu(&mut self, id: &str, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        self.page_menu(id, false, ui, page)
    }

    /// Draws the menu of the add-on `id` in the app's menu bar, when it
    /// asks for it there (see [`Addon::menu_in_menu_bar`]). False (and
    /// nothing drawn) when it is off, has none there, is core or unknown.
    pub fn menu_bar_menu(&mut self, id: &str, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        self.page_menu(id, true, ui, page)
    }

    /// Draws the menu of the add-on `id` if it belongs `in_menu_bar` (or
    /// in the header).
    fn page_menu(&mut self, id: &str, in_menu_bar: bool, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        let Some(addon) = self.enabled_addon(id) else { return false };
        let Some(title) = addon.menu_title() else { return false };
        if addon.menu_in_menu_bar() != in_menu_bar {
            return false;
        }
        ui.menu_button(title, |ui| addon.header_menu(ui, page));
        true
    }

    /// The subtitle of the add-on `id` (see [`Addon::subtitle`]); None
    /// when it is off, has none, is core or unknown.
    pub fn subtitle(&mut self, id: &str) -> Option<&'static str> {
        self.enabled_addon(id)?.subtitle()
    }

    /// Draws the header status of the add-on `id` (see
    /// [`Addon::header_status`]). False (and nothing drawn) when it is
    /// off, is core or unknown.
    pub fn header_status(&mut self, id: &str, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        let Some(addon) = self.enabled_addon(id) else { return false };
        addon.header_status(ui, page);
        true
    }

    /// Shows the row of the add-on `id` that owns `path` (see
    /// [`PathOwner::reveal`]). False (and nothing done) when it is off or
    /// does not offer owning paths.
    pub fn reveal(&mut self, id: &str, path: &std::path::Path) -> bool {
        match providers(&mut self.slots, Asked::PathOwner).find(|p| p.id == id) {
            Some(Provider { capability: Capability::PathOwner(owner), .. }) => {
                owner.reveal(path);
                true
            }
            _ => false,
        }
    }

    /// The Processes column offered by the add-on that is on, by add-on id
    /// (see [`ProcessColumnProvider`]); empty when none offers it.
    pub fn process_columns(&mut self) -> Vec<(&'static str, ProcessColumn)> {
        match provider(&mut self.slots, Asked::ProcessColumn) {
            Some(Provider { id, capability: Capability::ProcessColumn(column), .. }) => vec![(id, column.process_column())],
            _ => Vec::new(),
        }
    }

    /// The figures of the add-on `id`'s Processes column, which is drawn in
    /// this frame (see [`ProcessColumnProvider::process_figures`]). None when
    /// it is off, offers no column or has no reading yet.
    pub fn process_figures(&mut self, id: &str, settings: &crate::settings::Settings) -> Option<HashMap<u32, usize>> {
        let Some(Provider { id: provider_id, capability: Capability::ProcessColumn(column), .. }) = provider(&mut self.slots, Asked::ProcessColumn) else { return None };
        if provider_id != id {
            return None;
        }
        let figures = column.process_figures(settings);
        self.columns.mark(provider_id);
        figures
    }

    /// Opens the page of the add-on `id` on the process `pid` (see
    /// [`ProcessColumnProvider::reveal_process`]). False (and nothing done)
    /// when it is off or offers no column.
    pub fn reveal_process(&mut self, id: &str, pid: u32) -> bool {
        match provider(&mut self.slots, Asked::ProcessColumn) {
            Some(Provider { id: provider_id, capability: Capability::ProcessColumn(column), .. }) if provider_id == id => {
                column.reveal_process(pid);
                true
            }
            _ => false,
        }
    }

    /// [`crate::platform::Action::RecordSensors`]: starts (`on`) or stops
    /// the sensor recording of the add-on that records sensors, through its
    /// own Record and Stop. Refused with [`RECORDING_OFF`] while it is off.
    pub fn record_sensors(&mut self, on: bool, page: &mut PageContext) -> Result<(), &'static str> {
        match provider(&mut self.slots, Asked::SensorRecorder) {
            Some(Provider { capability: Capability::SensorRecorder(recorder), .. }) => recorder.set_sensor_recording(on, page),
            _ => Err(RECORDING_OFF),
        }
    }


    /// The sensors for a core surface (the Summary page), asked in this
    /// frame: the host keeps them while the surface asks and drops them at
    /// the end of the first frame it does not.
    pub fn core_sensors(&self, surface: leases::Surface) -> leases::Sensors {
        self.context.leases().ask(surface)
    }

    /// Ends a frame: add-ons whose page was drawn in the last frame but not
    /// in this one are told ([`Addon::page_hidden`]). The app calls it once
    /// per frame, after drawing. Returns the core surfaces (the Summary page)
    /// that asked for sensors in the last frame but not in this one.
    pub fn end_frame(&mut self) -> Vec<leases::Surface> {
        let hidden = self.shown.end_frame();
        let tiles_hidden = self.tiles.end_frame();
        let columns_hidden = self.columns.end_frame();
        // The sensors only a hidden page read stop with this frame.
        let dropped = self.context.leases().end_frame();
        for slot in &mut self.slots {
            if let Entry::Addon(a) = &mut slot.entry {
                if hidden.contains(&a.id()) {
                    a.page_hidden();
                }
                if tiles_hidden.contains(&a.id()) {
                    a.summary_hidden();
                }
                if columns_hidden.contains(&a.id()) {
                    if let Some(Capability::ProcessColumn(column)) = ask_directly(a.as_mut(), Asked::ProcessColumn) {
                        column.process_column_hidden();
                    }
                }
            }
        }
        dropped.into_iter().filter(|s| matches!(s, leases::Surface::Summary)).collect()
    }

    /// Draws the Summary tiles of the add-ons that are on.
    pub fn summary_tiles(&mut self, ui: &mut egui::Ui, page: &mut PageContext) {
        let mut drawn = Vec::new();
        let leases = self.context.leases().clone();
        let own = page.lease.take();
        for addon in self.enabled_addons() {
            // The tile reads sensors through its own lease, kept while it is drawn.
            page.lease = Some(leases.lease(leases::Surface::Tile(addon.id())));
            addon.summary(ui, page);
            drawn.push(addon.id());
        }
        page.lease = own;
        for id in drawn {
            self.tiles.mark(id);
        }
    }

    /// Draws the Settings sections of the add-ons that are on (the tests'
    /// way to reach every add-on's hook at once).
    #[cfg(test)]
    pub fn settings_sections(&mut self, ui: &mut egui::Ui, page: &mut PageContext) {
        for addon in self.enabled_addons() {
            addon.settings_section(ui, page);
        }
    }

    /// The add-ons that are on, as Settings lists them: id, name and
    /// description, in registry order.
    pub fn settings_addons(&mut self) -> Vec<(&'static str, &'static str, &'static str)> {
        self.enabled_addons().map(|a| (a.id(), a.name(), a.description())).collect()
    }

    /// Draws the Settings section of add-on `id` when it is on; returns
    /// whether it was drawn.
    pub fn settings_section(&mut self, id: &str, ui: &mut egui::Ui, page: &mut PageContext) -> bool {
        match self.enabled_addons().find(|a| a.id() == id) {
            Some(addon) => {
                addon.settings_section(ui, page);
                true
            }
            None => false,
        }
    }

    /// Draws the status-bar areas of the add-ons that are on; returns the
    /// ids drawn, in registry order.
    pub fn status_bar_areas(&mut self, ui: &mut egui::Ui, page: &mut PageContext) -> Vec<&'static str> {
        self.enabled_addons()
            .map(|addon| {
                addon.status_bar(ui, page);
                addon.id()
            })
            .collect()
    }

    /// Hands a new performance sample to every add-on that is on; returns
    /// the ids it reached, in registry order.
    pub fn on_sample(&mut self, sample: &Snapshot) -> Vec<&'static str> {
        self.enabled_addons()
            .map(|addon| {
                addon.on_sample(sample);
                addon.id()
            })
            .collect()
    }

    /// Hands the command line to every add-on that is on. An add-on that
    /// is off ignores its flags.
    pub fn command_line(&mut self, args: &[String]) -> CommandLine {
        let mut out = CommandLine::default();
        for addon in self.enabled_addons() {
            if addon.command_line(args) && out.page.is_none() {
                out.page = Some(addon.id());
            }
            out.reached.push(addon.id());
        }
        out
    }

    /// The add-ons that are on and not stopping, in registry order.
    fn enabled_addons(&mut self) -> impl Iterator<Item = &mut Box<dyn Addon>> {
        self.slots.iter_mut().filter(|s| s.enabled && !s.stopping).filter_map(|s| match &mut s.entry {
            Entry::Addon(a) => Some(a),
            Entry::Core(_) => None,
        })
    }

    /// The add-on `id` if it is on.
    fn enabled_addon(&mut self, id: &str) -> Option<&mut Box<dyn Addon>> {
        self.enabled_addons().find(|a| a.id() == id)
    }

    /// The slot of the page `id`.
    fn slot(&self, id: &str) -> Option<&Slot> {
        self.slots.iter().find(|s| s.entry.id() == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::PageContext;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use serde_json::json;

    /// The hooks every test add-on shares: its id, name and description,
    /// no requirements, no consumption, free.
    macro_rules! test_addon {
        ($id:literal, $name:literal, $description:literal) => {
            fn id(&self) -> &'static str {
                $id
            }
            fn name(&self) -> &'static str {
                $name
            }
            fn description(&self) -> &'static str {
                $description
            }
            fn requirements(&self) -> Requirements {
                Requirements::default()
            }
            fn consumption(&self) -> Consumption {
                Consumption { cpu: "None", network: "None", disk: "None", when: "Never" }
            }
            fn tier(&self) -> Tier {
                Tier::Free
            }
        };
    }

    /// Runs one windowless egui frame with a page context.
    fn frame(mut f: impl FnMut(&mut egui::Ui, &mut PageContext)) {
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        // The app's faces, as at start-up: pages draw in them.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| f(ui, &mut PageContext::new(&settings, &sample)));
        });
    }

    #[test]
    fn every_registry_row_names_the_id_of_the_add_on_it_makes() {
        for (id, tier, make) in ADDONS {
            let addon = make();
            assert_eq!(*id, addon.id(), "the row's id is the add-on's");
            assert_eq!(tier_of(id), *tier, "its tier is read from the row");
        }
    }

    /// An add-on owning the files under `C:\Apps\Tool` (as "Tool"); notes
    /// what it was asked to reveal.
    struct Owns(std::sync::Arc<std::sync::Mutex<Vec<std::path::PathBuf>>>);

    impl Addon for Owns {
        test_addon!("owns", "Owns", "Owns a folder.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::PathOwner => Some(Capability::PathOwner(self)),
                _ => None,
            }
        }
    }

    impl PathOwner for Owns {
        fn owner_of(&self, path: &std::path::Path) -> Option<String> {
            path.starts_with(r"C:\Apps\Tool").then(|| "Tool".to_string())
        }
        fn reveal(&mut self, path: &std::path::Path) {
            self.0.lock().unwrap().push(path.to_path_buf());
        }
    }

    /// An add-on whose page asks who owns `C:\Apps\Tool\tool.exe`.
    struct Asks(std::sync::Arc<std::sync::Mutex<Option<crate::platform::Owner>>>);

    impl Addon for Asks {
        test_addon!("asks", "Asks", "Asks who owns a file.");
        fn ui(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
            *self.0.lock().unwrap() = page.owner_of(std::path::Path::new(r"C:\Apps\Tool\tool.exe"));
            page.actions.push(crate::platform::Action::Say("asked".into()));
        }
    }

    #[test]
    fn a_page_links_to_the_row_of_another_addon_only_while_that_addon_is_on() {
        let (asked, revealed) = (std::sync::Arc::default(), std::sync::Arc::default());
        use std::sync::Arc;
        let entries = vec![Entry::Addon(Box::new(Asks(Arc::clone(&asked)))), Entry::Addon(Box::new(Owns(Arc::clone(&revealed))))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        let mut said = 0;
        frame(|ui, page| {
            host.ui("asks", ui, page);
            said = page.actions.len();
        });
        let owner = asked.lock().unwrap().clone().expect("owned while on");
        assert_eq!((owner.addon, owner.page, owner.label.as_str()), ("owns", "Owns", "Tool"));
        assert_eq!(said, 1, "the page's own actions reach the app");
        let file = std::path::Path::new(r"C:\Apps\Tool\tool.exe");
        assert!(host.reveal("owns", file));
        assert_eq!(*revealed.lock().unwrap(), vec![file.to_path_buf()]);

        host.set_enabled("owns", false);
        frame(|ui, page| {
            host.ui("asks", ui, page);
        });
        assert_eq!(*asked.lock().unwrap(), None, "no link to an add-on that is off");
        assert!(!host.reveal("owns", file), "nor a reveal");
        assert_eq!(revealed.lock().unwrap().len(), 1);
    }

    /// The sensors' update interval of the tests that look at what a read
    /// brings: far longer than a test, so nothing reads by itself.
    const LONG_INTERVAL_MS: u64 = 600_000;

    /// The sequence number of the newest sensor frame a test add-on's
    /// surface saw (0: none yet).
    #[derive(Clone, Default)]
    struct Seq(std::sync::Arc<std::sync::atomic::AtomicU64>);

    impl Seq {
        /// Notes the latest frame of `sensors`, at the long interval.
        fn note(&self, sensors: &leases::Sensors) {
            sensors.set_interval_ms(LONG_INTERVAL_MS);
            if let Some(frame) = sensors.latest() {
                self.0.store(frame.seq, std::sync::atomic::Ordering::SeqCst);
            }
        }

        fn get(&self) -> u64 {
            self.0.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    /// Where the page and the tile of a sensor-reading add-on note the frames
    /// they saw.
    #[derive(Clone, Default)]
    struct Seen {
        page: Seq,
        tile: Seq,
    }

    /// Defines a test add-on whose page and Summary tile read the sensors
    /// every time they draw and store nothing: the host decides how long the
    /// sensors run.
    macro_rules! sensor_reader {
        ($name:ident, $id:literal, $title:literal) => {
            #[derive(Default)]
            struct $name(Seen);

            impl Addon for $name {
                test_addon!($id, $title, "Reads sensors while drawn.");
                fn ui(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
                    if let Some(sensors) = page.sensors() {
                        self.0.page.note(&sensors);
                    }
                }
                fn summary(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
                    if let Some(sensors) = page.sensors() {
                        self.0.tile.note(&sensors);
                    }
                }
            }
        };
    }

    sensor_reader!(Looks, "looks", "Looks");
    sensor_reader!(LooksToo, "looks-too", "Looks too");

    /// Draws the page `id` for one frame and ends the frame.
    fn draw_page(host: &mut AddonHost, id: &str) {
        draw_page_with(host, id, &Snapshot::default());
    }

    /// Draws the Summary tiles for one frame and ends the frame.
    fn draw_tiles(host: &mut AddonHost) {
        frame(|ui, page| host.summary_tiles(ui, page));
        host.end_frame();
    }

    /// The add-on `id` has a surface the host keeps sensors for, and the
    /// service runs for it.
    fn holds_sensors(host: &AddonHost, id: &'static str) -> bool {
        host.context.leases().held_by(id).is_some() && sensor_threads(host) > 0
    }

    #[test]
    fn the_sensors_run_while_a_page_that_reads_them_is_drawn_every_frame_and_stop_one_frame_after() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks::default()))], &json!({}));
        for _ in 0..3 {
            draw_page(&mut host, "looks");
            assert!(sensor_threads(&host) > 0, "drawn in every frame");
        }
        host.end_frame(); // the first frame without the page
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    #[test]
    fn hiding_one_of_two_pages_that_read_the_sensors_keeps_them_running_and_hiding_both_stops_them() {
        let entries = vec![Entry::Addon(Box::new(Looks::default())), Entry::Addon(Box::new(LooksToo::default()))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        draw_page(&mut host, "looks");
        draw_page(&mut host, "looks-too");
        draw_page(&mut host, "looks-too"); // "looks" is not drawn any more
        draw_page(&mut host, "looks-too");
        assert!(holds_sensors(&host, "looks-too"), "the other page still looks");
        host.end_frame();
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    #[test]
    fn the_core_summary_runs_the_sensors_while_it_asks_every_frame_and_stops_one_frame_after() {
        let (mut host, _) = AddonHost::for_tests_with(Vec::new(), &json!({}));
        for _ in 0..3 {
            host.core_sensors(leases::Surface::Summary).latest();
            host.end_frame();
            assert!(sensor_threads(&host) > 0, "asked in every frame");
        }
        host.end_frame(); // the first frame without Summary
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    #[test]
    fn the_host_says_when_the_core_summary_stops_asking() {
        let (mut host, _) = AddonHost::for_tests_with(Vec::new(), &json!({}));
        host.core_sensors(leases::Surface::Summary).latest();
        assert!(host.end_frame().is_empty(), "Summary asked in this frame");
        assert_eq!(host.end_frame(), vec![leases::Surface::Summary], "the first frame without Summary");
        assert!(host.end_frame().is_empty(), "told once");
    }

    #[test]
    fn leaving_the_core_summary_keeps_the_sensors_an_add_on_page_still_reads() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks::default()))], &json!({}));
        host.core_sensors(leases::Surface::Summary).latest();
        draw_page(&mut host, "looks");
        draw_page(&mut host, "looks"); // Summary is not asked any more
        draw_page(&mut host, "looks");
        assert!(holds_sensors(&host, "looks"), "the add-on page still looks");
    }

    #[test]
    fn a_page_that_does_not_read_the_sensors_starts_nothing() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Spy(Default::default())))], &json!({}));
        draw_page(&mut host, "spy");
        assert_eq!(sensor_threads(&host), 0, "no read was asked, so none runs");
    }

    #[test]
    fn turning_an_addon_off_stops_the_sensors_it_alone_kept_running() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks::default()))], &json!({}));
        draw_page(&mut host, "looks");
        assert!(sensor_threads(&host) > 0);
        host.set_enabled("looks", false);
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    #[test]
    fn turning_an_addon_off_leaves_the_sensors_another_page_still_reads() {
        let entries = vec![Entry::Addon(Box::new(Looks::default())), Entry::Addon(Box::new(LooksToo::default()))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        draw_page(&mut host, "looks");
        frame(|ui, page| {
            host.ui("looks-too", ui, page);
        });
        host.set_enabled("looks", false);
        assert!(holds_sensors(&host, "looks-too"), "the other add-on's page is still drawn");
    }

    /// Draws until the surface saw a frame, calls `refresh`, and waits for a
    /// newer one. The update interval is far away: only the refresh reads.
    fn refresh_brings_a_new_frame(host: &mut AddonHost, seq: &Seq, draw: impl Fn(&mut AddonHost), refresh: impl Fn(&mut AddonHost)) {
        until("the first frame", || {
            draw(host);
            seq.get() > 0
        });
        let before = seq.get();
        refresh(host);
        until("a frame newer than the one before Refresh", || {
            draw(host);
            seq.get() > before
        });
    }

    #[test]
    fn refresh_reads_the_sensors_again_for_the_page_that_holds_them() {
        let seen = Seen::default();
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks(seen.clone())))], &json!({}));
        refresh_brings_a_new_frame(&mut host, &seen.page, |h| draw_page(h, "looks"), |h| assert!(h.refresh("looks")));
    }

    #[test]
    fn refresh_reads_the_sensors_again_for_a_summary_tile_that_holds_them() {
        let seen = Seen::default();
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks(seen.clone())))], &json!({}));
        refresh_brings_a_new_frame(&mut host, &seen.tile, draw_tiles, |h| assert!(h.refresh("looks")));
    }

    #[test]
    fn refresh_reads_the_sensors_again_for_the_core_summary_that_holds_them() {
        let seq = Seq::default();
        let (mut host, _) = AddonHost::for_tests_with(Vec::new(), &json!({}));
        let draw = |h: &mut AddonHost| {
            seq.note(&h.core_sensors(leases::Surface::Summary));
            h.end_frame();
        };
        refresh_brings_a_new_frame(&mut host, &seq, draw, |h| h.read_held_sensors());
    }

    #[test]
    fn closing_the_app_stops_the_sensors_the_add_ons_pages_kept_running() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Looks::default()))], &json!({}));
        draw_page(&mut host, "looks");
        assert!(sensor_threads(&host) > 0);
        host.close();
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    /// An add-on that reads the sensors and holds work closing would lose.
    struct LooksAndKeeps(bool);

    impl UnsavedWork for LooksAndKeeps {
        fn unsaved_work(&self) -> Option<&'static str> {
            self.0.then_some("Not saved.")
        }
        fn save_work(&mut self) -> bool {
            true
        }
    }

    impl Addon for LooksAndKeeps {
        test_addon!("keeps", "Keeps", "Reads sensors and holds work.");
        fn ui(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
            if let Some(sensors) = page.sensors() {
                sensors.latest();
            }
        }
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::UnsavedWork => Some(Capability::UnsavedWork(self)),
                _ => None,
            }
        }
        fn stop(&mut self) {
            self.0 = false; // the work is gone
        }
    }

    #[test]
    fn discarding_the_work_on_the_close_question_stops_the_sensors_the_add_on_kept_running() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(LooksAndKeeps(true)))], &json!({}));
        draw_page(&mut host, "keeps");
        assert!(sensor_threads(&host) > 0);
        assert!(!host.request_close(), "it holds work");
        assert!(host.answer_close(OffAnswer::Discard));
        until("no sensor thread", || sensor_threads(&host) == 0);
    }

    /// An add-on with a series running: it gives a reason to wait before
    /// the app closes.
    struct Series(bool);

    impl CloseWait for Series {
        fn close_reason(&self) -> Option<String> {
            self.0.then(|| "A series is running.".to_string())
        }
    }

    impl Addon for Series {
        test_addon!("series", "Series", "Runs a series.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::CloseWait => Some(Capability::CloseWait(self)),
                _ => None,
            }
        }
    }

    #[test]
    fn an_add_on_that_waits_gives_its_reason_to_close_only_while_it_is_on_and_works() {
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Series(true)))], &json!({}));
        assert_eq!(host.close_questions(), vec!["A series is running.".to_string()]);
        host.set_enabled("series", false);
        assert!(host.close_questions().is_empty(), "off: closing does not wait for it");
        let (mut idle, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Series(false)))], &json!({}));
        assert!(idle.close_questions().is_empty(), "nothing runs: closes without asking");
    }

    /// An add-on that records sensors for other pages: Ready, or Running
    /// once asked to start.
    struct Records(bool);

    impl SensorRecorder for Records {
        fn sensor_recording(&self, _interval_ms: u64) -> Recording {
            if self.0 {
                Recording::Running
            } else {
                Recording::Ready
            }
        }
        fn set_sensor_recording(&mut self, on: bool, _page: &mut PageContext) -> Result<(), &'static str> {
            self.0 = on;
            Ok(())
        }
    }

    impl Addon for Records {
        test_addon!("records", "Records", "Records sensors.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::SensorRecorder => Some(Capability::SensorRecorder(self)),
                _ => None,
            }
        }
    }

    /// A recorder with its own id, Ready until it is asked to start; it
    /// notes in the log that it was started.
    struct NamedRecorder(&'static str, std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);

    impl SensorRecorder for NamedRecorder {
        fn sensor_recording(&self, _interval_ms: u64) -> Recording {
            Recording::Ready
        }
        fn set_sensor_recording(&mut self, on: bool, _page: &mut PageContext) -> Result<(), &'static str> {
            if on {
                self.1.lock().unwrap().push(self.0);
            }
            Ok(())
        }
    }

    impl Addon for NamedRecorder {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn description(&self) -> &'static str {
            "A named recorder."
        }
        fn requirements(&self) -> Requirements {
            Requirements::default()
        }
        fn consumption(&self) -> Consumption {
            Consumption { cpu: "None", network: "None", disk: "None", when: "Never" }
        }
        fn tier(&self) -> Tier {
            Tier::Free
        }
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::SensorRecorder => Some(Capability::SensorRecorder(self)),
                _ => None,
            }
        }
    }

    /// A recorder in an operation that cannot be interrupted: turned off,
    /// it is stopping.
    struct RecorderKeeping;

    impl SensorRecorder for RecorderKeeping {
        fn sensor_recording(&self, _interval_ms: u64) -> Recording {
            Recording::Ready
        }
        fn set_sensor_recording(&mut self, _on: bool, _page: &mut PageContext) -> Result<(), &'static str> {
            Ok(())
        }
    }

    impl Addon for RecorderKeeping {
        test_addon!("keeping-recorder", "Keeping recorder", "Records and holds work.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::SensorRecorder => Some(Capability::SensorRecorder(self)),
                _ => None,
            }
        }
        fn uninterruptible(&self) -> bool {
            true
        }
    }

    /// A page that notes what the host told it about the sensor recording.
    struct Shows(std::sync::Arc<std::sync::Mutex<Option<Recording>>>);

    impl Addon for Shows {
        test_addon!("shows", "Shows", "Offers to record.");
        fn ui(&mut self, _: &mut egui::Ui, page: &mut PageContext) {
            *self.0.lock().unwrap() = Some(page.recording);
        }
    }

    /// What the page "shows" is told by `host` when it is drawn.
    fn recording_shown_by(host: &mut AddonHost, told: &std::sync::Mutex<Option<Recording>>) -> Option<Recording> {
        frame(|ui, page| {
            host.ui("shows", ui, page);
        });
        told.lock().unwrap().take()
    }

    fn record(host: &mut AddonHost, on: bool) -> Result<(), &'static str> {
        let (settings, sample) = (Settings::default(), Snapshot::default());
        host.record_sensors(on, &mut PageContext::new(&settings, &sample))
    }

    /// What a Processes-column test add-on was asked: `(hidden, revealed pids)`.
    type ColumnLog = std::sync::Arc<std::sync::Mutex<(u32, Vec<u32>)>>;

    /// An add-on that offers a Processes column of its own; it has no page
    /// of the kind Connections has.
    struct Counts(ColumnLog);

    impl ProcessColumnProvider for Counts {
        fn process_column(&self) -> ProcessColumn {
            ProcessColumn { title: "Counts", menu_item: "Go to counts" }
        }
        fn process_figures(&mut self, _settings: &Settings) -> Option<HashMap<u32, usize>> {
            Some(HashMap::from([(7, 3)]))
        }
        fn process_column_hidden(&mut self) {
            self.0.lock().unwrap().0 += 1;
        }
        fn reveal_process(&mut self, pid: u32) {
            self.0.lock().unwrap().1.push(pid);
        }
    }

    impl Addon for Counts {
        test_addon!("counts", "Counts", "Counts per process.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
            match asked {
                Asked::ProcessColumn => Some(Capability::ProcessColumn(self)),
                _ => None,
            }
        }
    }

    fn counts_host() -> (AddonHost, ColumnLog) {
        let log = ColumnLog::default();
        let entries = vec![Entry::Addon(Box::new(Records(false))), Entry::Addon(Box::new(Counts(std::sync::Arc::clone(&log))))];
        (AddonHost::for_tests_with(entries, &json!({})).0, log)
    }

    #[test]
    fn a_processes_column_comes_from_whichever_addon_offers_it_while_that_addon_is_on() {
        let (mut host, _) = counts_host();
        let column = ProcessColumn { title: "Counts", menu_item: "Go to counts" };
        assert_eq!(host.process_columns(), [("counts", column)]);
        assert_eq!(host.process_figures("counts", &Settings::default()), Some(HashMap::from([(7, 3)])));
        host.set_enabled("counts", false);
        assert!(host.process_columns().is_empty(), "no column while it is off");
        assert_eq!(host.process_figures("counts", &Settings::default()), None);
    }

    #[test]
    fn go_to_a_process_reaches_the_offering_addon_only_while_it_is_on() {
        let (mut host, log) = counts_host();
        assert!(host.reveal_process("counts", 42));
        assert!(!host.reveal_process("records", 42), "an add-on that offers no column is not asked");
        host.set_enabled("counts", false);
        assert!(!host.reveal_process("counts", 42));
        assert_eq!(log.lock().unwrap().1, [42]);
    }

    #[test]
    fn a_column_no_longer_drawn_is_told_to_drop_what_only_it_needs() {
        let (mut host, log) = counts_host();
        host.process_figures("counts", &Settings::default());
        host.end_frame();
        assert_eq!(log.lock().unwrap().0, 0, "still drawn in the last frame");
        host.end_frame();
        assert_eq!(log.lock().unwrap().0, 1);
    }

    #[test]
    fn a_page_starts_and_stops_another_addons_sensor_recording_through_the_host() {
        let told = std::sync::Arc::default();
        let entries = vec![Entry::Addon(Box::new(Shows(std::sync::Arc::clone(&told)))), Entry::Addon(Box::new(Records(false)))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Ready), "the page is told before it is drawn");
        assert_eq!(record(&mut host, true), Ok(()));
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Running), "the page follows the recorder's state");
        assert_eq!(record(&mut host, false), Ok(()));
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Ready));
    }

    #[test]
    fn a_disabled_recorder_says_off() {
        let told = std::sync::Arc::default();
        let entries = vec![Entry::Addon(Box::new(Shows(std::sync::Arc::clone(&told)))), Entry::Addon(Box::new(Records(false)))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        host.set_enabled("records", false);
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Off), "off is told too");
        assert_eq!(record(&mut host, true), Err(RECORDING_OFF), "nothing starts while it is off");
    }

    #[test]
    fn without_an_addon_that_records_sensors_the_recording_is_off() {
        let told = std::sync::Arc::default();
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(Shows(std::sync::Arc::clone(&told))))], &json!({}));
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Off));
        assert_eq!(record(&mut host, true), Err(RECORDING_OFF));
    }

    #[test]
    fn a_stopping_recorder_does_not_count() {
        let told = std::sync::Arc::default();
        let entries = vec![Entry::Addon(Box::new(Shows(std::sync::Arc::clone(&told)))), Entry::Addon(Box::new(RecorderKeeping))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Ready), "on: it counts");
        host.set_enabled("keeping-recorder", false);
        assert_eq!(recording_shown_by(&mut host, &told), Some(Recording::Off), "stopping: it does not count");
        assert_eq!(record(&mut host, true), Err(RECORDING_OFF));
    }

    #[test]
    fn with_two_recorders_the_first_in_registry_order_answers() {
        let log = std::sync::Arc::default();
        let entries = vec![Entry::Addon(Box::new(NamedRecorder("first", std::sync::Arc::clone(&log)))), Entry::Addon(Box::new(NamedRecorder("second", std::sync::Arc::clone(&log))))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        assert_eq!(record(&mut host, true), Ok(()));
        assert_eq!(*log.lock().unwrap(), ["first"]);
        host.set_enabled("first", false);
        assert_eq!(record(&mut host, true), Ok(()));
        assert_eq!(*log.lock().unwrap(), ["first", "second"], "with the first off, the next one answers");
    }

    /// An add-on that notes which of its hooks the host called.
    struct Spy(std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>);

    impl Addon for Spy {
        test_addon!("spy", "Spy", "Notes its hooks.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {
            self.0.lock().unwrap().push("page");
        }
        fn refresh(&mut self) {
            self.0.lock().unwrap().push("refresh");
        }
        fn summary(&mut self, _: &mut egui::Ui, _: &mut PageContext) {
            self.0.lock().unwrap().push("summary");
        }
        fn settings_section(&mut self, _: &mut egui::Ui, _: &mut PageContext) {
            self.0.lock().unwrap().push("settings");
        }
        fn on_sample(&mut self, _: &Snapshot) {
            self.0.lock().unwrap().push("sample");
        }
        fn status_bar(&mut self, _: &mut egui::Ui, _: &mut PageContext) {
            self.0.lock().unwrap().push("status");
        }
        fn page_hidden(&mut self) {
            self.0.lock().unwrap().push("hidden");
        }
        fn summary_hidden(&mut self) {
            self.0.lock().unwrap().push("tile hidden");
        }
        fn aliases(&self) -> &'static [&'static str] {
            &["the-spy"]
        }
        fn command_line(&mut self, args: &[String]) -> bool {
            if args.iter().any(|a| a == "--scan") {
                self.0.lock().unwrap().push("--scan");
            }
            args.iter().any(|a| a == "--spy-page")
        }
    }

    #[test]
    fn an_addon_is_told_once_when_its_summary_tile_stops_being_shown() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &json!({}));
        host.end_frame();
        for _ in 0..2 {
            frame(|ui, page| host.summary_tiles(ui, page));
            host.end_frame();
        }
        assert!(!calls.lock().unwrap().contains(&"tile hidden"), "Summary drawn in every frame");
        host.end_frame(); // another page
        host.end_frame();
        let calls = calls.lock().unwrap();
        assert_eq!(calls.iter().filter(|c| **c == "tile hidden").count(), 1, "told once");
        assert!(!calls.contains(&"hidden"), "its page was never shown");
    }

    #[test]
    fn an_addon_is_told_once_when_its_page_stops_being_shown() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &json!({}));
        host.end_frame();
        assert!(calls.lock().unwrap().is_empty(), "never shown: nothing to tell");
        for _ in 0..2 {
            frame(|ui, page| {
                host.ui("spy", ui, page);
            });
            host.end_frame();
        }
        assert!(!calls.lock().unwrap().contains(&"hidden"), "shown in every frame");
        host.end_frame(); // a frame where another page was drawn
        host.end_frame();
        assert_eq!(calls.lock().unwrap().iter().filter(|c| **c == "hidden").count(), 1, "told once");
    }

    #[test]
    fn page_requests_accept_an_addons_other_names() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls)))], &json!({}));
        for request in ["spy", "Spy", "the-spy"] {
            assert_eq!(host.landing(Some(request), LAST_USED, SUMMARY), Landing::Page("spy"), "{request}");
        }
        host.set_enabled("spy", false);
        assert_eq!(host.landing(Some("the-spy"), LAST_USED, SUMMARY), Landing::AddonSettings(Some("spy")));
    }

    #[test]
    fn power_and_freq_opens_by_its_name_and_by_its_alias() {
        let host = AddonHost::standard();
        for request in ["Power & Freq", "power-and-freq"] {
            assert_eq!(host.landing(Some(request), LAST_USED, SUMMARY), Landing::Page("power-freq"), "{request}");
        }
    }

    #[test]
    fn the_status_bar_area_is_drawn_only_while_the_addon_is_on() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &json!({}));
        let mut reached = Vec::new();
        frame(|ui, page| reached = host.status_bar_areas(ui, page));
        assert_eq!(reached, vec!["spy"]);
        assert!(calls.lock().unwrap().iter().all(|c| *c == "status"), "only its area, no page");
        host.set_enabled("spy", false);
        calls.lock().unwrap().clear();
        frame(|ui, page| reached = host.status_bar_areas(ui, page));
        assert!(reached.is_empty());
        assert!(calls.lock().unwrap().is_empty(), "an add-on that is off draws nothing in the status bar");
    }

    #[test]
    fn every_new_sample_reaches_an_addon_only_while_it_is_on_page_open_or_not() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &json!({}));
        let sample = Snapshot::default();
        assert_eq!(host.on_sample(&sample), vec!["spy"]);
        assert_eq!(host.on_sample(&sample), vec!["spy"]);
        assert_eq!(*calls.lock().unwrap(), vec!["sample", "sample"], "once per sample, no page drawn");
        host.set_enabled("spy", false);
        assert!(host.on_sample(&sample).is_empty());
        assert_eq!(calls.lock().unwrap().len(), 2, "an add-on that is off gets no sample");
        host.set_enabled("spy", true);
        host.on_sample(&sample);
        assert_eq!(calls.lock().unwrap().len(), 3, "turned on again, it gets samples again");
    }

    #[test]
    fn the_command_line_reaches_only_addons_that_are_on() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let section = json!({ "spy": { "enabled": false } });
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &section);
        let args = vec!["app.exe".to_string(), "--scan".to_string()];
        assert!(host.command_line(&args).reached.is_empty());
        assert!(calls.lock().unwrap().is_empty(), "an add-on that is off ignores its flags");
        host.set_enabled("spy", true);
        assert_eq!(host.command_line(&args), CommandLine { reached: vec!["spy"], page: None });
        assert_eq!(*calls.lock().unwrap(), vec!["--scan"]);
    }

    #[test]
    fn an_addon_flag_can_ask_for_its_page_only_while_it_is_on() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let section = json!({ "spy": { "enabled": false } });
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &section);
        let args = vec!["app.exe".to_string(), "--spy-page".to_string()];
        assert_eq!(host.command_line(&args).page, None, "off: ignored");
        host.set_enabled("spy", true);
        assert_eq!(host.command_line(&args).page, Some("spy"));
        assert_eq!(host.command_line(&args[..1]).page, None, "no flag, no page");
    }

    #[test]
    fn optional_hooks_reach_an_addon_only_while_it_is_on() {
        let calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let core = Entry::Core(Core::Summary);
        let (mut host, _) = AddonHost::for_tests_with(vec![core, Entry::Addon(Box::new(Spy(calls.clone())))], &json!({}));
        let use_hooks =|host: &mut AddonHost| {
            host.refresh("spy");
            frame(|ui, page| {
                host.ui("spy", ui, page);
                host.summary_tiles(ui, page);
                host.settings_sections(ui, page);
            });
        };
        use_hooks(&mut host);
        assert_eq!(*calls.lock().unwrap(), vec!["refresh", "page", "summary", "settings"]);
        host.set_enabled("spy", false);
        use_hooks(&mut host);
        assert_eq!(calls.lock().unwrap().len(), 4, "an add-on that is off gets no hook");
    }

    #[test]
    fn a_page_is_drawn_only_while_its_addon_is_on() {
        let mut host = AddonHost::standard();
        let mut drawn = Vec::new();
        frame(|ui, page| drawn.push(host.ui("benchmarks", ui, page)));
        host.set_enabled("benchmarks", false);
        frame(|ui, page| drawn.push(host.ui("benchmarks", ui, page)));
        frame(|ui, page| drawn.push(host.ui("summary", ui, page)));
        assert_eq!(drawn, vec![true, false, false]);
    }

    #[test]
    fn refresh_reaches_only_an_addon_that_is_on() {
        let mut host = AddonHost::standard();
        assert!(host.refresh("benchmarks"));
        host.set_enabled("benchmarks", false);
        assert!(!host.refresh("benchmarks"));
        assert!(!host.refresh("summary"), "core pages refresh themselves");
        assert!(!host.refresh("nope"));
    }

    #[test]
    fn ids_are_non_empty_and_unique() {
        let host = AddonHost::standard();
        let ids = host.ids();
        assert_eq!(ids.len(), 18);
        let mut seen = std::collections::HashSet::new();
        for id in ids {
            assert!(!id.is_empty());
            assert!(seen.insert(id), "duplicate id {id}");
        }
    }

    #[test]
    fn every_page_is_visible_by_default() {
        let host = AddonHost::standard();
        assert_eq!(host.visible(), host.ids());
        assert!(host.is_enabled("drivers"));
    }

    #[test]
    fn turning_an_addon_off_hides_it_at_once() {
        let mut host = AddonHost::standard();
        assert!(host.set_enabled("drivers", false));
        assert!(!host.is_enabled("drivers"));
        assert!(!host.visible().contains(&"drivers"));
        assert!(host.visible().contains(&"installed-apps"));
        assert!(host.set_enabled("drivers", true));
        assert!(host.visible().contains(&"drivers"));
    }

    #[test]
    fn the_core_cannot_be_turned_off() {
        let mut host = AddonHost::standard();
        for id in ["summary", "performance", "processes", "system-info", "app-history", "startup-apps", "users", "services", "settings", "colors"] {
            assert!(host.is_core(id), "{id} should be core");
            assert!(!host.set_enabled(id, false));
            assert!(host.is_enabled(id));
            assert!(host.visible().contains(&id));
        }
        assert_eq!(host.addons().count(), 8);
    }

    #[test]
    fn without_a_section_every_addon_is_on() {
        let host = AddonHost::from_section(&serde_json::Value::Null);
        assert_eq!(host.visible(), host.ids());
        let host = AddonHost::from_section(&serde_json::json!({}));
        assert_eq!(host.visible(), host.ids());
    }

    #[test]
    fn state_is_saved_in_the_section_and_read_back() {
        let mut host = AddonHost::standard();
        host.set_enabled("benchmarks", false);
        let mut section = serde_json::json!({});
        host.write_section(&mut section);
        assert_eq!(section["benchmarks"]["enabled"], serde_json::json!(false));
        assert_eq!(section["drivers"]["enabled"], serde_json::json!(true));

        let again = AddonHost::from_section(&section);
        assert!(!again.is_enabled("benchmarks"));
        assert!(again.is_enabled("drivers"));
    }

    #[test]
    fn writing_the_section_keeps_what_it_does_not_own() {
        let mut section = serde_json::json!({
            "drivers": { "enabled": true, "settings": { "sort": "date" } },
            "future-addon": { "enabled": false }
        });
        let mut host = AddonHost::from_section(&section);
        host.set_enabled("drivers", false);
        host.write_section(&mut section);
        assert_eq!(section["drivers"]["enabled"], serde_json::json!(false));
        assert_eq!(section["drivers"]["settings"]["sort"], serde_json::json!("date"));
        assert_eq!(section["future-addon"]["enabled"], serde_json::json!(false));
        assert!(section.get("summary").is_none(), "core pages are not saved");
    }

    #[test]
    fn an_addon_reads_and_writes_its_own_settings() {
        let section = json!({ "drivers": { "enabled": true, "settings": { "sort": "date" } } });
        let mut host = AddonHost::from_section(&section);
        assert_eq!(host.settings("drivers"), Some(json!({ "sort": "date" })));
        assert_eq!(host.settings("benchmarks"), Some(serde_json::Value::Null));

        assert!(host.set_settings("benchmarks", json!({ "size_mb": 256 })));
        let mut saved = section.clone();
        host.write_section(&mut saved);
        assert_eq!(saved["benchmarks"]["settings"], json!({ "size_mb": 256 }));
        assert_eq!(saved["drivers"]["settings"], json!({ "sort": "date" }));

        let again = AddonHost::from_section(&saved);
        assert_eq!(again.settings("benchmarks"), Some(json!({ "size_mb": 256 })));
    }

    #[derive(serde::Deserialize, Default, PartialEq, Debug)]
    #[serde(default)]
    struct DriverPrefs {
        sort: String,
        hide_microsoft: bool,
    }

    #[test]
    fn settings_an_addon_cannot_read_fall_back_to_its_defaults_alone() {
        let section = json!({
            "drivers": { "enabled": true, "settings": "garbage" },
            "benchmarks": { "enabled": true, "settings": { "sort": "name", "hide_microsoft": true } }
        });
        let host = AddonHost::from_section(&section);
        assert_eq!(read_settings::<DriverPrefs>(&host.settings("drivers").unwrap()), DriverPrefs::default());
        assert_eq!(
            read_settings::<DriverPrefs>(&host.settings("benchmarks").unwrap()),
            DriverPrefs { sort: "name".into(), hide_microsoft: true }
        );
        assert_eq!(read_settings::<DriverPrefs>(&serde_json::Value::Null), DriverPrefs::default());
    }

    /// An add-on that keeps settings of its own (shared with the test).
    struct Prefs(std::sync::Arc<std::sync::Mutex<serde_json::Value>>);

    impl Addon for Prefs {
        test_addon!("prefs", "Prefs", "Keeps a unit.");
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn settings(&self) -> serde_json::Value {
            self.0.lock().unwrap().clone()
        }
        fn load_settings(&mut self, settings: &serde_json::Value) {
            *self.0.lock().unwrap() = settings.clone();
        }
    }

    #[test]
    fn an_addon_gets_its_own_settings_and_what_it_changes_is_saved() {
        let mine = std::sync::Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
        let entries = vec![Entry::Core(Core::Summary), Entry::Addon(Box::new(Prefs(mine.clone())))];
        let section = json!({ "prefs": { "enabled": false, "settings": { "unit": "GB" } } });
        let (mut host, _) = AddonHost::for_tests_with(entries, &section);
        assert_eq!(*mine.lock().unwrap(), json!({ "unit": "GB" }), "handed over at start-up, also while off");
        assert!(!host.sync_settings(), "nothing changed");

        *mine.lock().unwrap() = json!({ "unit": "MB" }); // the user picks another unit
        assert!(host.sync_settings(), "the change is seen, to be saved");
        assert!(!host.sync_settings(), "once");
        assert_eq!(host.settings("prefs"), Some(json!({ "unit": "MB" })));
        let mut saved = section.clone();
        host.write_section(&mut saved);
        assert_eq!(saved["prefs"]["settings"], json!({ "unit": "MB" }));

        host.set_enabled("prefs", true);
        host.set_enabled("prefs", false);
        host.set_enabled("prefs", true);
        assert_eq!(*mine.lock().unwrap(), json!({ "unit": "MB" }), "off and on again, the same settings");
        assert!(host.set_settings("prefs", json!({ "unit": "KB" })));
        assert_eq!(*mine.lock().unwrap(), json!({ "unit": "KB" }), "set through the host, the add-on gets them");
    }

    #[test]
    fn a_corrupt_entry_falls_back_to_defaults_alone_from_and_to_json() {
        use crate::settings::Settings;
        let text = r#"{
            "dark": false,
            "addons": {
                "drivers": "not an object",
                "benchmarks": { "enabled": "yes", "settings": { "size_mb": 64 } },
                "connections": { "enabled": false, "settings": { "resolve": true } }
            }
        }"#;
        let mut s = Settings::from_json(text);
        let host = AddonHost::from_section(&s.addons);
        assert!(host.is_enabled("drivers"));
        assert_eq!(host.settings("drivers"), Some(serde_json::Value::Null));
        assert!(host.is_enabled("benchmarks"), "unreadable enabled means on");
        assert_eq!(host.settings("benchmarks"), Some(json!({ "size_mb": 64 })));
        assert!(!host.is_enabled("connections"));
        assert_eq!(host.settings("connections"), Some(json!({ "resolve": true })));

        host.write_section(&mut s.addons);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["dark"], json!(false));
        assert_eq!(saved["addons"]["drivers"], json!({ "enabled": true }));
        assert_eq!(saved["addons"]["connections"], json!({ "enabled": false, "settings": { "resolve": true } }));
    }

    #[test]
    fn unknown_fields_survive_a_read_save_cycle_through_the_host() {
        use crate::settings::Settings;
        let text = r#"{
            "newer_root": [1, 2],
            "addons": {
                "newer_addons_field": "x",
                "drivers": { "enabled": true, "settings": { "sort": "date" }, "newer_entry_field": 3 },
                "not-registered": { "enabled": false, "settings": { "a": 1 } }
            }
        }"#;
        let mut s = Settings::from_json(text);
        let mut host = AddonHost::from_section(&s.addons);
        host.set_enabled("disk-space", false);
        host.write_section(&mut s.addons);
        let saved: serde_json::Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(saved["newer_root"], json!([1, 2]));
        assert_eq!(saved["addons"]["newer_addons_field"], json!("x"));
        assert_eq!(saved["addons"]["drivers"]["newer_entry_field"], json!(3));
        assert_eq!(saved["addons"]["drivers"]["settings"], json!({ "sort": "date" }));
        assert_eq!(saved["addons"]["not-registered"], json!({ "enabled": false, "settings": { "a": 1 } }));
    }

    #[test]
    fn turning_an_addon_off_and_on_keeps_its_settings() {
        let section = json!({ "drivers": { "enabled": true, "settings": { "sort": "date" } } });
        let mut host = AddonHost::from_section(&section);
        host.set_enabled("drivers", false);
        let mut saved = section.clone();
        host.write_section(&mut saved);
        assert_eq!(saved["drivers"], json!({ "enabled": false, "settings": { "sort": "date" } }));

        let mut host = AddonHost::from_section(&saved);
        assert_eq!(host.settings("drivers"), Some(json!({ "sort": "date" })));
        host.set_enabled("drivers", true);
        host.write_section(&mut saved);
        assert_eq!(saved["drivers"], json!({ "enabled": true, "settings": { "sort": "date" } }));
    }

    #[test]
    fn core_pages_and_unknown_ids_have_no_settings() {
        let mut host = AddonHost::standard();
        assert_eq!(host.settings("summary"), None);
        assert_eq!(host.settings("nope"), None);
        assert!(!host.set_settings("summary", json!(1)));
        assert!(!host.set_settings("nope", json!(1)));
    }

    #[test]
    fn a_saved_page_is_found_by_id_or_by_its_old_shown_name() {
        let host = AddonHost::standard();
        assert_eq!(host.resolve_page("drivers"), "drivers");
        assert_eq!(host.resolve_page("system-info"), "system-info");
        assert_eq!(host.resolve_page("Installed Apps"), "installed-apps");
        assert_eq!(host.resolve_page("System Info"), "system-info");
        assert_eq!(host.resolve_page("Colors"), "colors");
    }

    #[test]
    fn an_unknown_saved_page_gives_summary() {
        let host = AddonHost::standard();
        assert_eq!(host.resolve_page("Nonexistent"), "summary");
        assert_eq!(host.resolve_page(""), "summary");
    }

    #[test]
    fn a_saved_page_whose_addon_is_off_gives_summary() {
        let mut host = AddonHost::standard();
        host.set_enabled("drivers", false);
        assert_eq!(host.resolve_page("drivers"), "summary");
        assert_eq!(host.resolve_page("Drivers"), "summary");
    }

    #[test]
    fn turning_off_the_open_addon_leaves_summary_as_the_open_page() {
        let mut host = AddonHost::standard();
        assert_eq!(host.resolve_page("benchmarks"), "benchmarks");
        host.set_enabled("benchmarks", false);
        assert_eq!(host.resolve_page("benchmarks"), "summary");
        assert_eq!(host.resolve_page("processes"), "processes");
    }

    #[test]
    fn page_flag_opens_the_named_page() {
        let host = AddonHost::standard();
        assert_eq!(host.landing(Some("Power & Freq"), "Last used", "processes"), Landing::Page("power-freq"));
        assert_eq!(host.landing(Some("users"), "Last used", "processes"), Landing::Page("users"));
    }

    #[test]
    fn page_flag_with_an_unknown_page_opens_summary() {
        let host = AddonHost::standard();
        assert_eq!(host.landing(Some("Nope"), "Last used", "processes"), Landing::Page("summary"));
    }

    #[test]
    fn page_flag_on_an_addon_that_is_off_opens_settings_addons_with_it_selected() {
        let mut host = AddonHost::standard();
        host.set_enabled("drivers", false);
        assert_eq!(host.landing(Some("Drivers"), "Last used", "processes"), Landing::AddonSettings(Some("drivers")));
        assert_eq!(host.landing(Some("drivers"), "Last used", "processes"), Landing::AddonSettings(Some("drivers")));
    }

    #[test]
    fn page_flag_add_ons_opens_settings_addons() {
        let host = AddonHost::standard();
        assert_eq!(host.landing(Some("Add-ons"), "Last used", "processes"), Landing::AddonSettings(None));
    }

    #[test]
    fn without_page_flag_the_start_page_or_the_last_page_opens() {
        let mut host = AddonHost::standard();
        assert_eq!(host.landing(None, "Last used", "drivers"), Landing::Page("drivers"));
        assert_eq!(host.landing(None, "Last used", "Disk Space"), Landing::Page("disk-space"));
        assert_eq!(host.landing(None, "services", "drivers"), Landing::Page("services"));
        assert_eq!(host.landing(None, "Services", "drivers"), Landing::Page("services"));
        assert_eq!(host.landing(None, "Gone", "drivers"), Landing::Page("summary"));
        host.set_enabled("drivers", false);
        assert_eq!(host.landing(None, "Last used", "drivers"), Landing::Page("summary"));
        assert_eq!(host.landing(None, "drivers", "processes"), Landing::Page("summary"));
    }

    /// The contract every registered add-on keeps. Each test loops over the
    /// registry, so a new add-on is covered without new tests.
    mod contract {
        use super::*;

        /// The registry the contract runs on: the real one plus the fake
        /// add-on, added the way any new add-on is (one registry line).
        fn registry() -> Vec<Entry> {
            crate::addons::fake::test_registry()
        }

        /// A test host on that registry.
        fn host(section: &serde_json::Value) -> (AddonHost, std::sync::Arc<crate::platform::RecordingRunner>) {
            AddonHost::for_tests_with(registry(), section)
        }

        fn addon_ids() -> Vec<&'static str> {
            registry().iter().filter(|e| matches!(e, Entry::Addon(_))).map(|e| e.id()).collect()
        }

        #[test]
        fn the_feature_tier_lines_name_no_addon() {
            let ids = addon_ids();
            for (id, _) in FEATURE_TIERS {
                assert!(!ids.contains(id), "{id} is an add-on: its tier belongs to its registry row");
            }
        }

        #[test]
        fn power_freq_alerts_ship_free() {
            assert_eq!(tier_of(crate::addons::power_freq::alerts::TIER_ID), Tier::Free);
        }

        #[test]
        fn power_freq_pins_ship_free() {
            assert_eq!(tier_of(crate::addons::power_freq::PINS_TIER_ID), Tier::Free);
        }

        #[test]
        fn every_addon_is_free() {
            for entry in crate::addons::registry() {
                if let Entry::Addon(a) = entry {
                    assert_eq!(a.tier(), Tier::Free, "{}", a.id());
                }
            }
            assert_eq!(tier_of("not-in-the-table"), Tier::Free);
        }

        #[test]
        fn a_new_addon_is_covered_without_new_tests() {
            assert!(addon_ids().contains(&"fake"), "the fake add-on's registry line puts it under the contract");
        }

        /// A section where only `id` is on.
        fn alone(id: &str) -> serde_json::Value {
            let mut section = json!({});
            for other in addon_ids() {
                section[other] = json!({ "enabled": other == id });
            }
            section
        }

        /// Uses every hook the host routes for `id`: `[refresh, sample, page,
        /// status-bar area]`.
        fn use_every_hook(host: &mut AddonHost, id: &str) -> Vec<bool> {
            let mut routed = vec![host.refresh(id), host.on_sample(&busy_sample()).contains(&id)];
            // egui may run a frame's closure twice (layout passes): note
            // whether the page was drawn at all.
            let (mut drawn, mut status) = (false, false);
            frame(|ui, page| {
                drawn |= host.ui(id, ui, page);
                host.summary_tiles(ui, page);
                host.settings_sections(ui, page);
                status |= host.status_bar_areas(ui, page).contains(&id);
            });
            routed.push(drawn);
            routed.push(status);
            routed
        }

        /// A sample from a big, busy PC: many cores, processes and adapters.
        fn busy_sample() -> Snapshot {
            use crate::sampler::{NetRow, ProcRow};
            Snapshot {
                cpu_total: 37.5,
                cpu_per_core: (0..64).map(|i| (i % 100) as f32).collect(),
                cpu_freq_mhz: 3_400,
                cpu_freqs: (0..64).map(|i| 2_000 + i * 37).collect(),
                procs: (0..600).map(|i| ProcRow { pid: i, name: format!("proc{i}.exe"), cpu: (i % 7) as f32, ..Default::default() }).collect(),
                net: (0..4).map(|i| NetRow { name: format!("eth{i}"), rx_bps: 1e6, tx_bps: 2e5, ..Default::default() }).collect(),
                mem_total: 64 << 30,
                mem_used: 20 << 30,
                ..Default::default()
            }
        }

        #[test]
        fn the_sample_hook_stays_within_its_budget() {
            const RUNS: u32 = 500;
            let sample = busy_sample();
            for id in addon_ids() {
                let (mut host, _) = host(&alone(id));
                host.on_sample(&sample); // first sample: allocations
                let t = std::time::Instant::now();
                for _ in 0..RUNS {
                    host.on_sample(&sample);
                }
                let each = t.elapsed() / RUNS;
                assert!(each < SAMPLE_HOOK_BUDGET, "{id} took {each:?} per sample (budget {SAMPLE_HOOK_BUDGET:?})");
            }
        }

        #[test]
        fn ids_are_stable_lowercase_and_unique() {
            let ids = addon_ids();
            assert_eq!(ids, addon_ids(), "the same ids every time");
            let mut seen = std::collections::HashSet::new();
            for id in ids {
                assert!(!id.is_empty());
                assert!(id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'), "{id} is not lowercase");
                assert!(seen.insert(id), "duplicate id {id}");
            }
        }

        #[test]
        fn only_an_addon_that_is_on_and_has_a_page_menu_shows_it_in_the_header() {
            let drawn = |section: &serde_json::Value, id: &str| {
                let (mut host, _) = host(section);
                let mut shown = false;
                frame(|ui, page| shown |= host.header_menu(id, ui, page));
                shown
            };
            assert!(drawn(&alone("disk-space"), "disk-space"), "Disk Space has its menu");
            assert!(!drawn(&alone("power-freq"), "disk-space"), "off: no menu");
            assert!(!drawn(&alone("disk-space"), "processes"), "a core page has none");
            assert!(!drawn(&json!({}), "unknown"));
        }

        #[test]
        fn only_an_addon_that_is_on_and_asks_for_it_shows_its_menu_in_the_menu_bar() {
            let drawn = |section: &serde_json::Value, id: &str| {
                let (mut host, _) = host(section);
                let (mut bar, mut header) = (false, false);
                frame(|ui, page| {
                    bar |= host.menu_bar_menu(id, ui, page);
                    header |= host.header_menu(id, ui, page);
                });
                (bar, header)
            };
            assert_eq!(drawn(&alone("benchmarks"), "benchmarks"), (true, false), "Benchmarks: in the menu bar only");
            assert_eq!(drawn(&alone("disk-space"), "disk-space"), (false, true), "Disk Space: in the header only");
            assert_eq!(drawn(&alone("disk-space"), "benchmarks"), (false, false), "off: no menu");
            assert_eq!(drawn(&alone("benchmarks"), "processes"), (false, false), "a core page has none");
        }

        #[test]
        fn only_an_addon_that_is_on_and_has_a_subtitle_shows_it_next_to_the_title() {
            let subtitle = |section: &serde_json::Value, id: &str| host(section).0.subtitle(id);
            assert_eq!(subtitle(&alone("connections"), "connections"), Some("Connection Management"));
            assert_eq!(subtitle(&alone("disk-space"), "connections"), None, "off: no subtitle");
            assert_eq!(subtitle(&alone("disk-space"), "disk-space"), None, "Disk Space has none");
            assert_eq!(subtitle(&alone("disk-space"), "processes"), None, "a core page has none");
        }

        #[test]
        fn only_an_addon_that_is_on_draws_its_header_status() {
            let drawn = |section: &serde_json::Value, id: &str| {
                let (mut host, _) = host(section);
                let mut shown = false;
                frame(|ui, page| shown |= host.header_status(id, ui, page));
                shown
            };
            assert!(drawn(&alone("connections"), "connections"));
            assert!(!drawn(&alone("disk-space"), "connections"), "off: nothing");
            assert!(!drawn(&json!({}), "unknown"));
        }

        #[test]
        fn starting_launches_no_program() {
            let (_, runner) = host(&json!({}));
            assert!(runner.launches().is_empty(), "all add-ons started");
            for id in addon_ids() {
                let (host, runner) = host(&alone(id));
                assert!(host.is_enabled(id));
                assert!(runner.launches().is_empty(), "{id} launched a program on start");
            }
        }

        #[test]
        fn an_addon_that_is_off_gets_no_work_and_launches_no_program() {
            for id in addon_ids() {
                let (mut host, runner) = host(&alone(id));
                assert!(host.set_enabled(id, false));
                assert_eq!(use_every_hook(&mut host, id), vec![false, false, false, false], "{id} got work while off");
                assert!(runner.launches().is_empty(), "{id} launched a program while off");
            }
        }

        #[test]
        fn turning_off_and_on_keeps_its_settings() {
            for id in addon_ids() {
                let mine = json!({ "kept": id, "n": [1, 2] });
                let section = json!({ id: { "enabled": true, "settings": mine } });
                let (mut host, _) = host(&section);
                let before = host.settings(id).unwrap();
                assert!(before.get("kept").is_some(), "{id} lost settings it does not know: {before}");
                host.set_enabled(id, false);
                host.set_enabled(id, true);
                host.sync_settings();
                assert_eq!(host.settings(id), Some(before.clone()), "{id}");
                let mut saved = json!({});
                host.write_section(&mut saved);
                assert_eq!(saved[id]["settings"], before, "{id}");
            }
        }

        #[test]
        fn with_no_operation_running_it_turns_off_at_once() {
            for id in addon_ids() {
                let (mut host, _) = host(&alone(id));
                assert!(host.set_enabled(id, false));
                assert!(!host.is_enabled(id), "{id}");
                assert!(!host.is_stopping(id), "{id} waits for nothing");
            }
        }

        /// An add-on whose "install" (started by Refresh) runs a program
        /// through the executor and must not be cut short.
        struct Installer {
            context: Option<AddonContext>,
            generation: crate::platform::Generation,
            running: std::sync::Arc<std::sync::atomic::AtomicUsize>,
            result: std::sync::Arc<std::sync::Mutex<Option<u64>>>,
        }

        impl Addon for Installer {
            fn id(&self) -> &'static str {
                "installer"
            }
            fn name(&self) -> &'static str {
                "Installer"
            }
            fn description(&self) -> &'static str {
                "Installs things."
            }
            fn requirements(&self) -> Requirements {
                Requirements::default()
            }
            fn consumption(&self) -> Consumption {
                Consumption { cpu: "None", network: "None", disk: "None", when: "On install" }
            }
            fn tier(&self) -> Tier {
                Tier::Free
            }
            fn start(&mut self, context: &AddonContext) {
                self.context = Some(context.clone());
            }
            fn stop(&mut self) {
                let mut result = self.result.lock().unwrap();
                self.generation.bump();
                *result = None;
            }
            fn uninterruptible(&self) -> bool {
                self.running.load(std::sync::atomic::Ordering::SeqCst) > 0
            }
            fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
            fn refresh(&mut self) {
                let (ctx, generation, running, result) =
                    (self.context.clone().unwrap(), self.generation.clone(), self.running.clone(), self.result.clone());
                let born = generation.current();
                running.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::thread::spawn(move || {
                    let _ = ctx.winget(&["install", "thing"]);
                    let mut r = result.lock().unwrap();
                    if generation.current() == born {
                        *r = Some(born);
                    }
                    drop(r);
                    running.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                });
            }
        }

        #[test]
        fn turning_off_during_an_uninterruptible_operation_waits_for_its_end() {
            let running = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let result = std::sync::Arc::new(std::sync::Mutex::new(None));
            let installer = Installer { context: None, generation: Default::default(), running: running.clone(), result: result.clone() };
            let entries = vec![Entry::Core(Core::Summary), Entry::Addon(Box::new(installer))];
            let (mut host, runner) = AddonHost::for_tests_with(entries, &json!({}));
            runner.hold();
            assert!(host.refresh("installer"));
            runner.wait_for(1);

            assert!(host.set_enabled("installer", false));
            assert!(host.is_stopping("installer"), "the switch shows it turns off when done");
            assert!(host.visible().contains(&"installer"), "it stays in the menu meanwhile");
            assert_eq!(host.resolve_page("installer"), "installer", "its open page stays open");
            assert_eq!(
                use_every_hook(&mut host, "installer"),
                vec![false, false, true, false],
                "only its page, to follow the operation: no refresh, sample or status area"
            );
            assert!(!host.set_enabled("installer", true), "cannot turn back on while waiting");
            assert!(host.settle().is_empty(), "still running");
            assert!(host.is_stopping("installer"));
            let mut saved = json!({});
            host.write_section(&mut saved);
            assert_eq!(saved["installer"]["enabled"], json!(true), "saved off only once it is off");

            runner.release();
            let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while running.load(std::sync::atomic::Ordering::SeqCst) > 0 {
                assert!(std::time::Instant::now() < until);
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            assert_eq!(host.settle(), vec!["installer"], "off once the operation ended");
            assert!(!host.is_stopping("installer"));
            assert!(!host.is_enabled("installer"));
            assert!(!host.visible().contains(&"installer"), "gone from the menu");
            assert_eq!(host.resolve_page("installer"), SUMMARY, "its open page gives way to Summary");
            assert_eq!(use_every_hook(&mut host, "installer"), vec![false, false, false, false]);
            host.write_section(&mut saved);
            assert_eq!(saved["installer"]["enabled"], json!(false), "and the choice is saved");
            assert!(host.settle().is_empty(), "settled once");
            assert!(host.set_enabled("installer", true));
            assert_eq!(*result.lock().unwrap(), None, "the late result of the old operation is dropped");
        }

        /// An add-on whose Refresh asks the install queue for an install
        /// (noting whether it got its turn) and which notes every "packages
        /// changed" it is told, with who changed them.
        struct Queuer {
            id: &'static str,
            context: Option<AddonContext>,
            ran: std::sync::Arc<std::sync::Mutex<Vec<bool>>>,
            told: std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>,
        }

        impl Queuer {
            fn new(id: &'static str) -> (Self, std::sync::Arc<std::sync::Mutex<Vec<bool>>>, std::sync::Arc<std::sync::Mutex<Vec<&'static str>>>) {
                let (ran, told) = (std::sync::Arc::default(), std::sync::Arc::default());
                (Queuer { id, context: None, ran: std::sync::Arc::clone(&ran), told: std::sync::Arc::clone(&told) }, ran, told)
            }
        }

        impl Addon for Queuer {
            fn id(&self) -> &'static str {
                self.id
            }
            fn name(&self) -> &'static str {
                "Queuer"
            }
            fn description(&self) -> &'static str {
                "Installs through the queue."
            }
            fn requirements(&self) -> Requirements {
                Requirements::default()
            }
            fn consumption(&self) -> Consumption {
                Consumption { cpu: "None", network: "None", disk: "None", when: "On install" }
            }
            fn tier(&self) -> Tier {
                Tier::Free
            }
            fn start(&mut self, context: &AddonContext) {
                self.context = Some(context.clone());
            }
            fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
            fn refresh(&mut self) {
                let (ctx, ran, id) = (self.context.clone().unwrap(), self.ran.clone(), self.id);
                std::thread::spawn(move || {
                    let request = crate::platform::install_queue::Request { addon: id, name: "Queuer", work: crate::platform::install_queue::Work::Install, items: vec![("winget".into(), "Vendor.App".into())] };
                    // The turn (if any) is given back before `ran` tells.
                    let got = ctx.install_queue().enter(request, |_| {}).is_some();
                    ran.lock().unwrap().push(got);
                });
            }
            fn packages_changed(&mut self, by: &'static str) {
                self.told.lock().unwrap().push(by);
            }
        }

        #[test]
        fn turning_an_addon_off_withdraws_its_requests_waiting_for_the_install_queue() {
            let (queuer, ran, _) = Queuer::new("queuer");
            let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(queuer))], &json!({}));
            let queue = host.context().install_queue().clone();
            let held = queue.enter(crate::platform::install_queue::Request { addon: "other", name: "Other", work: crate::platform::install_queue::Work::Update, items: vec![] }, |_| {}).unwrap();
            assert!(host.refresh("queuer"));
            crate::addons::until("its request waits", || queue.waiting() == 1);

            assert!(host.set_enabled("queuer", false));
            crate::addons::until("its request is withdrawn", || !ran.lock().unwrap().is_empty());
            assert_eq!(*ran.lock().unwrap(), [false], "it never got the queue");
            assert_eq!(queue.waiting(), 0);
            drop(held);
        }

        #[test]
        fn the_addons_that_are_on_are_told_once_when_a_queue_job_ends() {
            let (on, ran, told_on) = Queuer::new("on");
            let (off, _, told_off) = Queuer::new("off");
            let entries = vec![Entry::Addon(Box::new(on)), Entry::Addon(Box::new(off))];
            let (mut host, _) = AddonHost::for_tests_with(entries, &json!({ "off": { "enabled": false } }));
            host.tell_package_changes();
            assert!(told_on.lock().unwrap().is_empty(), "no job ended yet");

            assert!(host.refresh("on"));
            crate::addons::until("the job ends", || !ran.lock().unwrap().is_empty());
            assert!(told_on.lock().unwrap().is_empty(), "told by the host, not by the queue's thread");
            host.tell_package_changes();
            assert_eq!(*told_on.lock().unwrap(), ["on"], "the add-on that ran the job is told too");
            assert!(told_off.lock().unwrap().is_empty(), "an add-on that is off is not told");
            host.tell_package_changes();
            assert_eq!(told_on.lock().unwrap().len(), 1, "told once");
        }

        /// What the [`Notebook`] add-on did, seen by the test.
        #[derive(Default)]
        struct Notes {
            /// Work not saved yet (turning off would lose it).
            dirty: bool,
            /// Whether Save manages to save (false: its dialog was cancelled).
            save_works: bool,
            saved: usize,
            stopped: usize,
        }

        /// An add-on holding work that turning it off would lose.
        struct Notebook(std::sync::Arc<std::sync::Mutex<Notes>>);

        impl Addon for Notebook {
            fn id(&self) -> &'static str {
                "notebook"
            }
            fn name(&self) -> &'static str {
                "Notebook"
            }
            fn description(&self) -> &'static str {
                "Keeps notes."
            }
            fn requirements(&self) -> Requirements {
                Requirements::default()
            }
            fn consumption(&self) -> Consumption {
                Consumption { cpu: "None", network: "None", disk: "None", when: "Never" }
            }
            fn tier(&self) -> Tier {
                Tier::Free
            }
            fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
            fn stop(&mut self) {
                let mut n = self.0.lock().unwrap();
                n.stopped += 1;
                n.dirty = false;
            }
            fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
                match asked {
                    Asked::UnsavedWork => Some(Capability::UnsavedWork(self)),
                    _ => None,
                }
            }
        }

        impl UnsavedWork for Notebook {
            fn unsaved_work(&self) -> Option<&'static str> {
                self.0.lock().unwrap().dirty.then_some("The notes are not saved.")
            }
            fn save_work(&mut self) -> bool {
                let mut n = self.0.lock().unwrap();
                if n.save_works {
                    n.saved += 1;
                    n.dirty = false;
                }
                n.save_works
            }
        }

        /// A host with the Notebook on, holding unsaved work; the notes too.
        fn notebook(save_works: bool) -> (AddonHost, std::sync::Arc<std::sync::Mutex<Notes>>) {
            let notes = std::sync::Arc::new(std::sync::Mutex::new(Notes { dirty: true, save_works, ..Default::default() }));
            let entries = vec![Entry::Core(Core::Summary), Entry::Addon(Box::new(Notebook(notes.clone())))];
            (AddonHost::for_tests_with(entries, &json!({})).0, notes)
        }

        #[test]
        fn turning_off_with_unsaved_work_asks_first_and_keeps_it_on_meanwhile() {
            let (mut host, notes) = notebook(true);
            assert!(host.set_enabled("notebook", false));
            assert_eq!(host.off_question(), Some(OffQuestion { id: "notebook", name: "Notebook", question: "The notes are not saved." }));
            assert!(host.is_enabled("notebook"), "still on until the user answers");
            assert_eq!(notes.lock().unwrap().stopped, 0, "nothing stopped yet");
        }

        #[test]
        fn save_on_the_question_saves_then_turns_off() {
            let (mut host, notes) = notebook(true);
            host.set_enabled("notebook", false);
            assert!(host.answer_off(OffAnswer::Save), "the state changed: save the section");
            let n = notes.lock().unwrap();
            assert_eq!((n.saved, n.stopped), (1, 1));
            drop(n);
            assert!(!host.is_enabled("notebook"));
            assert_eq!(host.off_question(), None);
        }

        #[test]
        fn discard_on_the_question_turns_off_without_saving() {
            let (mut host, notes) = notebook(true);
            host.set_enabled("notebook", false);
            assert!(host.answer_off(OffAnswer::Discard));
            let n = notes.lock().unwrap();
            assert_eq!((n.saved, n.stopped), (0, 1));
            drop(n);
            assert!(!host.is_enabled("notebook"));
            assert_eq!(host.off_question(), None);
        }

        #[test]
        fn cancel_on_the_question_leaves_it_on_with_its_work() {
            let (mut host, notes) = notebook(true);
            host.set_enabled("notebook", false);
            assert!(!host.answer_off(OffAnswer::Cancel), "nothing changed");
            assert!(host.is_enabled("notebook"));
            assert_eq!(notes.lock().unwrap().stopped, 0);
            assert!(notes.lock().unwrap().dirty, "its work is kept");
            assert_eq!(host.off_question(), None);
        }

        #[test]
        fn a_save_that_does_not_happen_leaves_it_on() {
            let (mut host, notes) = notebook(false);
            host.set_enabled("notebook", false);
            assert!(!host.answer_off(OffAnswer::Save));
            assert!(host.is_enabled("notebook"), "the work would be lost: it stays on");
            assert_eq!(notes.lock().unwrap().stopped, 0);
            assert_eq!(host.off_question(), None);
        }

        #[test]
        fn without_unsaved_work_it_turns_off_without_a_question() {
            let (mut host, notes) = notebook(true);
            notes.lock().unwrap().dirty = false;
            host.set_enabled("notebook", false);
            assert_eq!(host.off_question(), None);
            assert!(!host.is_enabled("notebook"));
        }

        #[test]
        fn a_question_whose_work_is_gone_is_dropped() {
            let (mut host, notes) = notebook(true);
            host.set_enabled("notebook", false);
            notes.lock().unwrap().dirty = false;
            assert_eq!(host.off_question(), None, "nothing left to lose");
            host.settle();
            notes.lock().unwrap().dirty = true;
            assert_eq!(host.off_question(), None, "an old question does not come back");
        }

        #[test]
        fn without_unsaved_work_the_app_closes_at_once() {
            let (mut host, notes) = notebook(true);
            notes.lock().unwrap().dirty = false;
            assert!(host.request_close());
            assert_eq!(host.close_save_question(), None);
        }

        #[test]
        fn closing_with_unsaved_work_asks_the_same_question_first() {
            let (mut host, notes) = notebook(true);
            assert!(!host.request_close(), "the app stays open until the user answers");
            assert_eq!(host.close_save_question(), Some(OffQuestion { id: "notebook", name: "Notebook", question: "The notes are not saved." }));
            assert_eq!(notes.lock().unwrap().stopped, 0, "nothing stopped yet");
        }

        #[test]
        fn save_on_the_close_question_saves_then_lets_the_app_close() {
            let (mut host, notes) = notebook(true);
            host.request_close();
            assert!(host.answer_close(OffAnswer::Save));
            assert_eq!(notes.lock().unwrap().saved, 1);
            assert!(host.is_enabled("notebook"), "closing the app does not turn it off");
            assert_eq!(host.close_save_question(), None);
            assert!(host.request_close(), "nothing left to lose: the close goes through");
        }

        #[test]
        fn a_save_that_does_not_happen_keeps_the_app_open_with_the_work() {
            let (mut host, notes) = notebook(false);
            host.request_close();
            assert!(!host.answer_close(OffAnswer::Save));
            assert!(notes.lock().unwrap().dirty, "the work stays unsaved");
            assert_eq!(host.close_save_question(), None, "the question is closed");
        }

        #[test]
        fn discard_on_the_close_question_drops_the_work_and_lets_the_app_close() {
            let (mut host, notes) = notebook(true);
            host.request_close();
            assert!(host.answer_close(OffAnswer::Discard));
            let n = notes.lock().unwrap();
            assert_eq!((n.saved, n.stopped, n.dirty), (0, 1, false), "stopped: its work and threads are gone");
            drop(n);
            assert!(host.is_enabled("notebook"), "it stays on for the next start");
            assert!(host.request_close());
        }

        #[test]
        fn cancel_on_the_close_question_keeps_the_app_open_and_the_work() {
            let (mut host, notes) = notebook(true);
            host.request_close();
            assert!(!host.answer_close(OffAnswer::Cancel));
            let n = notes.lock().unwrap();
            assert_eq!((n.saved, n.stopped, n.dirty), (0, 0, true));
            drop(n);
            assert_eq!(host.close_save_question(), None);
        }

        #[test]
        fn closing_replaces_a_pending_off_question() {
            let (mut host, _) = notebook(true);
            host.set_enabled("notebook", false);
            host.request_close();
            assert_eq!(host.off_question(), None, "one question at a time");
            assert!(host.close_save_question().is_some());
        }

        #[test]
        fn no_addon_keeps_a_thread_while_off() {
            for id in addon_ids() {
                let (mut host, _) = host(&alone(id));
                use_every_hook(&mut host, id);
                host.set_enabled(id, false);
                if host.off_question().is_some() {
                    host.answer_off(OffAnswer::Discard);
                }
                assert!(!host.visible().contains(&id), "{id} is not shown");
                assert_eq!(host.threads(id), 0, "{id} keeps a thread while off");
            }
        }

        #[test]
        fn installed_apps_and_drivers_each_work_with_the_other_off() {
            for (on, off) in [("installed-apps", "drivers"), ("drivers", "installed-apps")] {
                let (mut host, _) = host(&json!({ off: { "enabled": false } }));
                assert!(!host.is_enabled(off));
                assert!(use_every_hook(&mut host, on).iter().all(|&routed| routed), "{on} with {off} off");
            }
        }

        /// Waits (up to 5 s) until the host's sensor service runs no thread.
        fn until_idle(host: &AddonHost, why: &str) {
            let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while host.context().sensors().threads() > 0 {
                assert!(std::time::Instant::now() < end, "sensor threads alive {why}");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        #[test]
        fn no_sensor_thread_outlives_its_page_or_its_add_on() {
            for id in addon_ids() {
                let (mut host, _) = host(&alone(id));
                until_idle(&host, &format!("before the page of {id} was shown"));
                use_every_hook(&mut host, id);
                host.end_frame();
                host.end_frame(); // a frame without its page
                until_idle(&host, &format!("after the page of {id} closed"));
                use_every_hook(&mut host, id);
                assert!(host.set_enabled(id, false));
                until_idle(&host, &format!("after {id} was turned off"));
            }
        }

        #[test]
        fn no_sensor_thread_outlives_a_summary_tile_or_its_add_on() {
            let id = "power-freq";
            let mut section = alone(id);
            section[id]["settings"] = json!({ "summary_tile": true });
            let (mut host, _) = host(&section);
            frame(|ui, page| host.summary_tiles(ui, page));
            host.end_frame();
            assert!(host.context().sensors().threads() > 0, "the tile reads the sensors");
            host.end_frame(); // Summary left
            until_idle(&host, "after Summary closed");
            frame(|ui, page| host.summary_tiles(ui, page));
            assert!(host.set_enabled(id, false));
            until_idle(&host, "after it was turned off");
        }

        #[test]
        fn on_alone_it_starts_works_and_stops() {
            for id in addon_ids() {
                let (mut host, _) = host(&alone(id));
                assert_eq!(host.visible().iter().filter(|v| !host.is_core(v)).collect::<Vec<_>>(), vec![&id]);
                assert_eq!(use_every_hook(&mut host, id), vec![true, true, true, true], "{id}");
                assert!(host.set_enabled(id, false), "{id} stops");
                assert!(host.set_enabled(id, true), "{id} starts again");
            }
        }
    }
}

/// The install queue shared by Installed Apps and Drivers (#111).
#[cfg(test)]
mod install_queue_tests {
    use super::drivers::Drivers;
    use super::installed_apps::apps::App;
    use super::installed_apps::InstalledApps;
    use super::{until, Addon};
    use crate::platform::install_queue::Work;
    use crate::platform::{AddonContext, Launch};

    fn tool(key: &str) -> App {
        App { key: key.into(), name: key.into(), uninstall: format!("{key}.exe /S"), ..Default::default() }
    }

    /// An app winget can update ("Vendor.<key>" to 2.0).
    fn newer(key: &str) -> App {
        App { winget_id: format!("Vendor.{key}"), available: "2.0".into(), ..tool(key) }
    }

    /// The elevated PowerShell runs (driver installs) among `launches`.
    fn elevated(launches: &[Launch]) -> usize {
        launches.iter().filter(|l| matches!(l, Launch::PowerShell { elevated: true, .. })).count()
    }

    #[test]
    fn a_driver_install_launches_nothing_while_an_update_holds_the_install_queue() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone();
        hub.lock().apps = vec![newer("tool")];
        runner.hold();
        hub.update(vec!["tool".into()]);
        runner.wait_for(1);

        let mut drivers = Drivers::default();
        drivers.start(&ctx);
        let drv = drivers.page().unwrap().drv.clone();
        drv.install(vec!["update-1".into()]);
        until("Drivers says who holds the queue", || {
            drv.lock().busy.as_deref() == Some("Installed Apps is updating 1 item; this starts when it is done\u{2026}")
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(runner.launches().len(), 1, "only the update runs: {:?}", runner.launches());
        assert_eq!(ctx.install_queue().holder().map(|h| h.addon), Some("installed-apps"));
        assert_eq!(ctx.install_queue().waiting(), 1, "the driver update waits");

        runner.release();
        until("the driver install runs once the update ended", || elevated(&runner.launches()) == 1);
        let launches = runner.launches();
        let upgrade = launches.iter().position(|l| matches!(l, Launch::Winget(a) if a[0] == "upgrade")).unwrap();
        let install = launches.iter().position(|l| matches!(l, Launch::PowerShell { elevated: true, .. })).unwrap();
        assert!(upgrade < install, "{launches:?}");
        until("the install ends", || !drivers.uninterruptible());
    }

    #[test]
    fn an_uninstall_waiting_for_the_queue_never_runs_once_turned_off() {
        let (ctx, runner) = AddonContext::recording();
        let mut drivers = Drivers::default();
        drivers.start(&ctx);
        runner.hold();
        drivers.page().unwrap().drv.install_inf(r"C:\pkg\driver.inf".into());
        runner.wait_for(1);

        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone();
        hub.lock().apps = vec![tool("first")];
        hub.uninstall(vec!["first".into()]);
        until("the uninstall says who holds the queue", || hub.lock().busy.as_deref() == Some("Drivers is installing 1 item; this starts when it is done\u{2026}"));
        assert_eq!(ctx.install_queue().waiting(), 1);

        addon.stop();
        runner.release();
        until("the uninstall's job ended", || !addon.uninterruptible());
        until("the queue is free", || ctx.install_queue().holder().is_none());
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!runner.launches().iter().any(|l| matches!(l, Launch::Program { .. })), "no uninstaller ran: {:?}", runner.launches());
        until("the driver install ends", || !drivers.uninterruptible());
    }

    /// Every text an add-on's page shows in one frame.
    fn page_text(addon: &mut dyn Addon) -> String {
        fn collect(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let ctx = egui::Context::default();
        // The app's faces, as at start-up: pages draw in them.
        crate::theme::install_fonts(&ctx);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| addon.ui(ui, &mut crate::platform::PageContext::new(&settings, &sample)));
        });
        let mut out = Vec::new();
        output.shapes.iter().for_each(|c| collect(&c.shape, &mut out));
        out.join("\n")
    }

    #[test]
    fn installed_apps_and_drivers_pages_show_who_holds_the_queue_while_they_wait() {
        let (ctx, runner) = AddonContext::recording();
        let mut apps = InstalledApps::default();
        apps.start(&ctx);
        let hub = apps.page().unwrap().apps.clone();
        hub.lock().apps = vec![newer("tool"), tool("other")];
        let mut drivers = Drivers::default();
        drivers.start(&ctx);
        let drv = drivers.page().unwrap().drv.clone();

        runner.hold();
        hub.update(vec!["tool".into()]);
        runner.wait_for(1);
        drv.install_inf(r"C:\pkg\driver.inf".into());
        until("the Drivers page says who holds the queue", || page_text(&mut drivers).contains("Installed Apps is updating 1 item; this starts when it is done\u{2026}"));
        assert_eq!(ctx.install_queue().waiting(), 1);
        runner.release();
        until("the update and the driver install end", || !apps.uninterruptible() && !drivers.uninterruptible());
        until("both pages are idle", || hub.lock().busy.is_none() && drv.lock().busy.is_none());
        hub.lock().apps = vec![tool("other")];

        runner.hold();
        let before = runner.launches().len();
        drv.install_inf(r"C:\pkg\driver.inf".into());
        runner.wait_for(before + 1);
        hub.uninstall(vec!["other".into()]);
        until("Installed Apps waits", || ctx.install_queue().waiting() == 1);
        until("the Installed Apps page says who holds the queue", || page_text(&mut apps).contains("Drivers is installing 1 item; this starts when it is done\u{2026}"));
        assert_eq!(ctx.install_queue().waiting(), 1);
        hub.stop_waiting();
        runner.release();
        until("both end", || !apps.uninterruptible() && !drivers.uninterruptible());
    }

    #[test]
    fn change_and_repair_wait_for_the_install_queue_and_say_what_they_do() {
        for (repair, verb) in [(false, "changing"), (true, "repairing")] {
            let (ctx, runner) = AddonContext::recording();
            let mut drivers = Drivers::default();
            drivers.start(&ctx);
            runner.hold();
            drivers.page().unwrap().drv.install_inf(r"C:\pkg\driver.inf".into());
            runner.wait_for(1);

            let mut addon = InstalledApps::default();
            addon.start(&ctx);
            let hub = addon.page().unwrap().apps.clone();
            hub.lock().apps = vec![App { modify: "tool-setup.exe /modify".into(), msi_code: "{TOOL}".into(), ..tool("tool") }];
            hub.modify("tool".into(), repair);
            until("it says who holds the queue", || hub.lock().busy.as_deref() == Some("Drivers is installing 1 item; this starts when it is done\u{2026}"));
            assert_eq!(ctx.install_queue().waiting(), 1);
            assert!(!runner.launches().iter().any(|l| matches!(l, Launch::Program { .. })), "no setup runs while Drivers installs");

            runner.release();
            until("it ends", || !addon.uninterruptible() && !drivers.uninterruptible());
            let launches = runner.launches();
            let setup = launches.iter().position(|l| matches!(l, Launch::Program { .. })).expect("the setup ran once Drivers was done");
            assert!(elevated(&launches[..setup]) == 1, "{repair}: after the driver install: {launches:?}");
            assert_eq!(ctx.install_queue().take_finished(), ["drivers", "installed-apps"], "{repair}: it held the queue");
            assert_eq!(ctx.install_queue().holder(), None);
            let holder = crate::platform::install_queue::Holder { addon: "installed-apps", name: "Installed Apps", work: if repair { Work::Repair } else { Work::Change }, items: 1 };
            assert_eq!(holder.text(), format!("Installed Apps is {verb} 1 item; this starts when it is done\u{2026}"));
        }
    }
}
