//! The Drivers add-on: installed device and kernel drivers, Windows
//! Update's driver updates and installing them. Reading the list, checking
//! Windows Update, scanning for hardware and unloading a driver are dropped
//! at once when the add-on is turned off; a driver install (Windows Update
//! or an INF package) that is running is not cut short: the add-on turns
//! off when it ends.

/// The add-on's command-line flags (`--drivers-scope`).
pub mod cli;
/// Reading drivers and Windows Update, and installing.
pub mod engine;
/// The Drivers page.
pub mod page;
/// The Windows reader of the listing.
pub mod windows;

use crate::addons::{checked_tile_lines, is_default, plural, read_settings, summary_tile, write_settings, Addon, Consumption, Requirements};
use crate::platform::{Action, AddonContext, PageContext};

use std::sync::Arc;

use engine::{DeviceDriver, Hub, KernelDriver, KernelScope};
use page::{DriversPage, Scope, DEFAULT_OLD_YEARS};

/// Its id and shown name (also in its install-queue requests).
const ID: &str = "drivers";
const NAME: &str = "Drivers";

/// The note under its requirements in Settings › Add-ons.
pub const NEEDS: &str = "Searching for driver updates needs the internet and the Windows Update service. \
Administrator rights are asked through UAC only to install, scan for hardware changes and unload drivers \
(with PowerShell, pnputil and sc): do not start this app as administrator.";

/// Where the add-on reads the installed devices and kernel drivers from:
/// the Windows reader in the app, a fake in tests. Owned by the add-on, not
/// by the shared add-on context.
pub trait ListingReader: Send + Sync {
    /// The devices with a driver, or why they could not be read.
    fn devices(&self) -> Result<Vec<DeviceDriver>, String>;
    /// The kernel driver services, or why they could not be read.
    fn kernel(&self) -> Result<Vec<KernelDriver>, String>;
}

/// The add-on: its page while it is on, and while it waits to turn off.
pub struct Drivers {
    /// Asked for the listing each time the page reads it.
    reader: Arc<dyn ListingReader>,
    /// The page and its background work, while the add-on is on. Turned
    /// off during a driver install, the page stays (its hub stopped) to
    /// follow that install until it ends.
    page: Option<DriversPage>,
    /// Its settings; the tab, kernel filter and "Show disconnected" are
    /// taken from the page while there is one.
    prefs: Prefs,
    /// The tab and kernel scope `--drivers-scope` opened the page on: not
    /// saved until the user picks another one.
    from_command_line: Option<(Scope, KernelScope)>,
}

/// Drivers' own settings, as saved: a default value is left out, and the
/// fields it does not know are kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Clone)]
#[serde(default)]
struct Prefs {
    /// The tab: hardware, built-in or kernel drivers.
    #[serde(skip_serializing_if = "is_default")]
    scope: Scope,
    /// The kernel list's scope, by name (older versions saved its index).
    #[serde(rename = "kernelFilter", skip_serializing_if = "is_default")]
    kernel_filter: KernelScope,
    /// "Show disconnected".
    #[serde(rename = "showDisconnected", skip_serializing_if = "is_default")]
    show_disconnected: bool,
    /// "Check Windows Update when the page opens" (off by default).
    #[serde(rename = "checkOnOpen", skip_serializing_if = "is_default")]
    check_on_open: bool,
    /// Years after which a third-party driver gets "check the maker".
    #[serde(rename = "makerYears", skip_serializing_if = "is_default_years")]
    maker_years: i64,
    /// The Summary tile is on (off by default).
    #[serde(rename = "summaryTile", skip_serializing_if = "is_default")]
    summary_tile: bool,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

fn is_default_years(years: &i64) -> bool {
    *years == DEFAULT_OLD_YEARS
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            scope: Scope::default(),
            kernel_filter: KernelScope::All,
            show_disconnected: false,
            check_on_open: false,
            maker_years: DEFAULT_OLD_YEARS,
            summary_tile: false,
            rest: serde_json::Map::new(),
        }
    }
}

impl Prefs {
    /// Puts the saved choices on the page.
    fn apply_to(&self, page: &mut DriversPage) {
        page.drv_ui.scope = self.scope;
        page.drv_ui.kernel_filter = self.kernel_filter;
        page.drv_ui.show_disconnected = self.show_disconnected;
        self.apply_options_to(page);
    }

    /// Puts the Settings options on the page: checking on open and the
    /// maker threshold (at least one year).
    fn apply_options_to(&self, page: &mut DriversPage) {
        page.check_on_open = self.check_on_open;
        page.old_years = self.maker_years.max(1);
    }
}

impl Default for Drivers {
    fn default() -> Self {
        Drivers { reader: Arc::new(windows::WindowsListing), page: None, prefs: Prefs::default(), from_command_line: None }
    }
}

impl Drivers {
    /// The add-on reading its listing from `reader`.
    #[cfg(test)]
    pub fn with_reader(reader: Arc<dyn ListingReader>) -> Self {
        Drivers { reader, ..Drivers::default() }
    }

    /// The page while the add-on is on (or waits to turn off).
    #[cfg(test)]
    pub fn page(&mut self) -> Option<&mut DriversPage> {
        self.page.as_mut()
    }

    /// The Summary tile's lines: how many newer drivers Windows Update
    /// offers and when it was asked. Only when the tile is on and the user
    /// pressed Check in this session (an automatic check on open changes
    /// nothing here); the tile never starts a check.
    pub fn summary_lines(&self) -> Option<Vec<String>> {
        if !self.prefs.summary_tile {
            return None;
        }
        let check = self.page.as_ref()?.drv.lock().hand_check.clone()?;
        Some(checked_tile_lines(plural(check.newer, "newer driver"), &check.at))
    }

    /// The tab and kernel scope shown now, while there is a page.
    fn view(&self) -> Option<(Scope, KernelScope)> {
        self.page.as_ref().map(|p| (p.drv_ui.scope, p.drv_ui.kernel_filter))
    }

    /// The settings with the page's current choices; the tab and scope the
    /// command line asked for are not saved while they are still shown.
    fn current(&self) -> Prefs {
        let mut prefs = self.prefs.clone();
        if let Some(page) = &self.page {
            if self.from_command_line.is_none() || self.from_command_line != self.view() {
                prefs.scope = page.drv_ui.scope;
                prefs.kernel_filter = page.drv_ui.kernel_filter;
            }
            prefs.show_disconnected = page.drv_ui.show_disconnected;
        }
        prefs
    }
}

impl Addon for Drivers {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        NAME
    }
    fn description(&self) -> &'static str {
        "Installed drivers, their versions and available updates."
    }
    /// The internet and Windows Update for the search; administrator
    /// rights only to install, scan and unload.
    fn requirements(&self) -> Requirements {
        Requirements { admin: true, network: true, programs: &["PowerShell", "pnputil", "sc"], note: Some(NEEDS) }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Medium", network: "On update checks", disk: "On install", when: "On open and on your actions" }
    }

    /// Cheap: the list is read when the page first opens. A page left
    /// from waiting to turn off is replaced by a fresh one.
    /// The page opens on the tab, filter and "Show disconnected" last chosen.
    fn start(&mut self, context: &AddonContext) {
        self.prefs = self.current();
        self.from_command_line = None;
        let mut page = DriversPage::new(Hub::new(context.clone(), Arc::clone(&self.reader)), context.clone());
        self.prefs.apply_to(&mut page);
        self.page = Some(page);
    }

    /// Its settings ([`Prefs`]); `Null` while nothing was chosen nor saved.
    fn settings(&self) -> serde_json::Value {
        write_settings(&self.current())
    }

    fn load_settings(&mut self, settings: &serde_json::Value) {
        self.prefs = read_settings(settings);
        if let Some(page) = &mut self.page {
            self.prefs.apply_to(page);
        }
    }

    /// "Check Windows Update when the page opens", the years after which
    /// a driver gets "check the maker", and the Summary tile.
    fn settings_section(&mut self, ui: &mut egui::Ui, _page: &mut PageContext) {
        use crate::widgets::parts::{group_heading, setting_number, setting_switch};
        group_heading(ui, "Updates");
        let mut changed = setting_switch(ui, "Check Windows Update when the page opens", None, &mut self.prefs.check_on_open);
        changed |= setting_number(ui, "Suggest checking the maker's site for drivers older than", None, &mut self.prefs.maker_years, 1..=20, "years");
        if let (true, Some(page)) = (changed, &mut self.page) {
            self.prefs.apply_options_to(page);
        }
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("Newer drivers, after you check"), &mut self.prefs.summary_tile);
    }

    /// How many newer drivers Windows Update offers, once the user checked.
    fn summary(&mut self, ui: &mut egui::Ui, page: &mut PageContext) {
        if let Some(lines) = self.summary_lines() {
            summary_tile(ui, page, "Drivers", lines);
        }
    }

    /// Stops the hub; the page goes at once unless a driver install still
    /// runs, which the page keeps following. The tab and filters chosen
    /// are kept.
    fn stop(&mut self) {
        self.prefs = self.current();
        if let Some(page) = &self.page {
            page.drv.stop();
            if !page.drv.running() {
                self.page = None;
            }
        }
    }

    fn uninterruptible(&self) -> bool {
        self.page.as_ref().is_some_and(|p| p.drv.running())
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut PageContext) {
        if let Some(page) = &mut self.page {
            page.ui(ui, &mut ctx.actions);
            ctx.actions.extend(page.said.drain(..).map(Action::Say));
        }
        // Once the user picks another tab or scope, it is theirs to save.
        if self.from_command_line.is_some() && self.from_command_line != self.view() {
            self.from_command_line = None;
        }
    }

    /// `--drivers-scope <scope>` opens the page (even without `--page`)
    /// on Kernel modules with that scope, without saving it; an invalid
    /// value is ignored.
    fn command_line(&mut self, args: &[String]) -> bool {
        let (Some(page), Some(scope)) = (&mut self.page, cli::kernel_scope(args)) else { return false };
        page.drv_ui.scope = Scope::Kernel;
        page.drv_ui.kernel_filter = scope;
        self.from_command_line = self.view();
        true
    }

    fn refresh(&mut self) {
        if let Some(page) = &self.page {
            page.drv.refresh();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::until;
    use crate::platform::Launch;

    fn on(ctx: &AddonContext) -> Drivers {
        let mut addon = Drivers::default();
        addon.start(ctx);
        addon
    }

    fn hub(addon: &mut Drivers) -> Hub {
        addon.page().unwrap().drv.clone()
    }

    #[test]
    fn turning_off_during_a_windows_update_install_keeps_its_page_to_follow_it_and_shows_nothing_of_it_later() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        runner.hold();
        hub(&mut addon).install(vec!["update-1".into()]);
        runner.wait_for(1);
        assert!(addon.uninterruptible(), "a driver install is running");

        addon.stop();
        assert!(addon.uninterruptible(), "turning off does not cut the install short");
        let old = hub(&mut addon);
        assert!(old.lock().busy.as_deref().is_some_and(|b| b.starts_with("Installing")), "its page shows the running install");
        runner.release();
        until("the install ends", || !addon.uninterruptible());
        let st = old.lock();
        assert!(st.busy.is_none() && st.message.is_some(), "the page shows how the install ended");
        drop(st);
        assert_eq!(runner.launches().len(), 1, "nothing else is started");

        addon.start(&ctx);
        let st = hub(&mut addon).lock().message.clone();
        assert!(st.is_none(), "nothing of the old install is shown once on again");
    }

    #[test]
    fn turning_off_during_an_inf_install_waits_for_it() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        runner.hold();
        hub(&mut addon).install_inf(r"C:\pkg\driver.inf".into());
        runner.wait_for(1);
        addon.stop();
        assert!(addon.uninterruptible(), "the package install goes on");
        runner.release();
        until("the install ends", || !addon.uninterruptible());
    }

    #[test]
    fn turning_off_stops_checking_windows_update_at_once() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        runner.hold();
        hub(&mut addon).check_updates();
        runner.wait_for(1);
        assert!(!addon.uninterruptible(), "a check can be dropped");

        let old = hub(&mut addon);
        addon.stop();
        assert!(!addon.uninterruptible());
        assert!(addon.page().is_none(), "with nothing to wait for, the page goes at once");
        runner.release();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let st = old.lock();
        assert!(st.message.is_none() && st.updates.is_none(), "the late answer of Windows Update is dropped");
        drop(st);
        addon.start(&ctx);
        let h = hub(&mut addon);
        let st = h.lock();
        assert!(st.message.is_none() && st.busy.is_none() && st.updates.is_none());
    }

    #[test]
    fn scanning_and_unloading_are_dropped_at_once() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        let h = hub(&mut addon);
        runner.hold();
        h.scan();
        runner.wait_for(1);
        addon.stop();
        assert!(!addon.uninterruptible(), "a hardware scan does not hold the add-on on");
        runner.release();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(h.lock().message.is_none(), "the late scan outcome is dropped");
    }

    #[test]
    fn turning_off_ends_a_slow_windows_update_search_without_waiting_for_it() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        runner.hold();
        let old = hub(&mut addon);
        old.check_updates();
        runner.wait_for(1);
        assert!(matches!(&runner.launches()[0], Launch::PowerShell { elevated: false, .. }), "the search runs without elevation");

        addon.stop();
        until("the search's PowerShell is ended", || runner.stopped() == 1);
        assert!(old.lock().updates.is_none() && old.lock().message.is_none(), "its result is dropped");
        runner.release();

        addon.start(&ctx);
        let fresh = hub(&mut addon);
        let st = fresh.lock();
        assert!(st.updates.is_none() && st.message.is_none() && st.busy.is_none(), "nothing old once on again");
        assert_eq!(runner.launches().len(), 1, "nothing else is started");
    }

    #[test]
    fn turned_off_it_asks_for_no_program() {
        let off = serde_json::json!({ "drivers": { "enabled": false, "settings": { "checkOnOpen": true, "summaryTile": true } } });
        let (mut host, runner) = crate::addons::AddonHost::for_tests(&off);
        assert!(!host.refresh("drivers"), "no page to refresh");
        assert!(runner.launches().is_empty(), "no PowerShell, pnputil nor sc while off");

        // A page's buttons still pressed after turning it off launch nothing.
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        let h = hub(&mut addon);
        addon.stop();
        h.check_updates();
        h.install(vec!["update-1".into()]);
        h.install_inf(r"C:\pkg\driver.inf".into());
        h.scan();
        h.unload("svc".into());
        h.refresh();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(runner.launches(), Vec::<Launch>::new(), "Check, install, INF install, scan and unload launch nothing");
        assert!(!h.lock().listing && h.lock().busy.is_none() && !addon.uninterruptible());
    }

    #[test]
    fn the_platform_keeps_no_drivers_code() {
        let sources = [
            ("app.rs", include_str!("../app.rs")),
            ("main.rs", include_str!("../main.rs")),
            ("page_colors.rs", include_str!("../page_colors.rs")),
            ("page_settings.rs", include_str!("../page_settings.rs")),
            ("platform.rs", include_str!("../platform.rs")),
            ("pro.rs", include_str!("../pro.rs")),
        ];
        for (file, src) in sources {
            for word in [concat!("drivers", "::"), concat!("page_", "drivers"), concat!("drv", "_ui"), concat!("self.", "drv")] {
                assert!(!src.contains(word), "{file} still has Drivers code ({word})");
            }
        }
    }

    #[test]
    fn opening_device_manager_goes_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = on(&ctx);
        let page = addon.page().unwrap();
        page.open_device_manager();
        assert_eq!(runner.launches(), vec![Launch::Open("devmgmt.msc".into())]);
        assert_eq!(page.said.len(), 1, "the failure is told in the status bar");
    }
}

/// Settings, requirements and the Summary tile (#71).
#[cfg(test)]
mod settings_tests {
    use super::page::Scope;
    use super::*;
    use crate::addons::until;
    use crate::platform::{Launch, RecordingRunner};

    /// How many times Windows Update was searched.
    fn searches(runner: &RecordingRunner) -> usize {
        runner.launches().iter().filter(|l| matches!(l, Launch::PowerShell { script, .. } if script == engine::SEARCH_SCRIPT)).count()
    }

    #[test]
    fn its_requirements_name_windows_update_powershell_pnputil_sc_and_admin_only_through_uac() {
        let (host, _) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "drivers": { "enabled": false } }));
        let d = crate::addons::overview::Overview::of(&host, Some("drivers")).detail.unwrap();
        assert_eq!(
            d.requirements,
            ["Asks for administrator rights for some actions", "Uses the internet", "Runs PowerShell", "Runs pnputil", "Runs sc"],
            "shown before turning it on"
        );
        let note = d.note.expect("a note under the requirements");
        for words in ["Windows Update service", "UAC", "install", "scan", "unload", "not start this app as administrator"] {
            assert!(note.contains(words), "the note says {words:?}: {note}");
        }
    }

    #[test]
    fn the_tab_kernel_filter_and_show_disconnected_come_back_after_turning_it_off_and_on() {
        let (ctx, _) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.load_settings(&serde_json::Value::Null);
        addon.start(&ctx);
        assert_eq!(addon.settings(), serde_json::Value::Null, "nothing chosen, nothing saved");
        let ui = &mut addon.page().unwrap().drv_ui;
        ui.scope = Scope::Kernel;
        ui.kernel_filter = KernelScope::ThirdParty;
        ui.show_disconnected = true;
        addon.stop();
        let saved = addon.settings();
        assert_eq!(saved, serde_json::json!({ "scope": "kernel", "kernelFilter": "third-party", "showDisconnected": true }), "saved under its id while off");
        addon.start(&ctx);
        let ui = &addon.page().unwrap().drv_ui;
        assert!((ui.scope, ui.kernel_filter, ui.show_disconnected) == (Scope::Kernel, KernelScope::ThirdParty, true), "back once on again");

        let mut next_run = Drivers::default();
        next_run.load_settings(&serde_json::json!({ "scope": "builtin", "fromANewerVersion": 1 }));
        next_run.start(&ctx);
        assert!(next_run.page().unwrap().drv_ui.scope == Scope::Builtin, "read from the saved settings");
        assert_eq!(next_run.settings()["fromANewerVersion"], 1, "fields it does not know are kept");
    }

    #[test]
    fn a_kernel_filter_saved_as_a_number_by_an_older_version_is_still_read() {
        let (ctx, _) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.load_settings(&serde_json::json!({ "scope": "kernel", "kernelFilter": 4 }));
        addon.start(&ctx);
        assert!(addon.page().unwrap().drv_ui.kernel_filter == KernelScope::FileSystem);
        assert_eq!(addon.settings()["kernelFilter"], "file-system", "saved again by name");
    }

    #[test]
    fn checking_windows_update_when_the_page_opens_is_off_by_default() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.start(&ctx);
        addon.page().unwrap().opened();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(searches(&runner), 0, "only the list is read");

        let (ctx, runner) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.load_settings(&serde_json::json!({ "checkOnOpen": true }));
        addon.start(&ctx);
        addon.page().unwrap().opened();
        addon.page().unwrap().opened();
        until("Windows Update is asked", || searches(&runner) == 1);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(searches(&runner), 1, "once, the first time the page opens");
        assert_eq!(addon.settings(), serde_json::json!({ "checkOnOpen": true }));
    }

    #[test]
    fn the_maker_threshold_is_three_years_by_default_and_saved_when_changed() {
        let (ctx, _) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.start(&ctx);
        assert_eq!(addon.page().unwrap().old_years, 3);
        addon.load_settings(&serde_json::json!({ "makerYears": 5 }));
        assert_eq!(addon.page().unwrap().old_years, 5, "applied to the open page");
        assert_eq!(addon.settings(), serde_json::json!({ "makerYears": 5 }));
        addon.load_settings(&serde_json::json!({ "makerYears": 0 }));
        assert_eq!(addon.page().unwrap().old_years, 1, "at least one year");
    }

    #[test]
    fn the_summary_tile_shows_only_after_a_check_by_hand_and_only_when_turned_on() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = Drivers::default();
        assert_eq!(addon.summary_lines(), None, "off: no tile");
        addon.load_settings(&serde_json::json!({ "summaryTile": true }));
        addon.start(&ctx);
        assert_eq!(addon.summary_lines(), None, "no check yet, and none is started for the tile");
        assert!(runner.launches().is_empty());

        let h = addon.page().unwrap().drv.clone();
        let mut st = h.lock();
        st.updates = Some(vec![engine::DriverUpdate::default()]);
        st.checked_at = "09:00".into();
        drop(st);
        assert_eq!(addon.summary_lines(), None, "a check made when the page opened does not count");
        h.lock().hand_check = Some(engine::HandCheck { newer: 2, at: "14:03".into() });
        assert_eq!(addon.summary_lines(), Some(vec!["2 newer drivers".to_string(), "Last checked at 14:03".to_string()]));
        h.lock().hand_check = Some(engine::HandCheck { newer: 1, at: "14:05".into() });
        assert_eq!(addon.summary_lines().unwrap()[0], "1 newer driver");

        addon.load_settings(&serde_json::Value::Null);
        assert_eq!(addon.summary_lines(), None, "the tile is off by default");
    }
}

#[cfg(test)]
mod command_line_tests {
    use super::*;
    use page::Scope;

    fn args(a: &[&str]) -> Vec<String> {
        std::iter::once("app.exe").chain(a.iter().copied()).map(String::from).collect()
    }

    fn opened_with(a: &[&str]) -> (Scope, KernelScope) {
        let (ctx, _) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.start(&ctx);
        addon.command_line(&args(a));
        let ui = &addon.page().unwrap().drv_ui;
        (ui.scope, ui.kernel_filter)
    }

    #[test]
    fn a_drivers_scope_opens_kernel_modules_on_that_scope() {
        for scope in KernelScope::ALL {
            assert!(opened_with(&["--page", "Drivers", "--drivers-scope", scope.name()]) == (Scope::Kernel, scope), "{scope:?}");
        }
    }

    #[test]
    fn an_invalid_or_missing_drivers_scope_opens_the_page_as_usual() {
        assert!(opened_with(&["--drivers-scope", "everything"]) == (Scope::Hardware, KernelScope::All));
        assert!(opened_with(&["--page", "Drivers"]) == (Scope::Hardware, KernelScope::All));
    }

    #[test]
    fn a_drivers_scope_alone_opens_the_drivers_page() {
        let (mut host, runner) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "drivers": { "enabled": true } }));
        assert_eq!(host.command_line(&args(&["--drivers-scope", "loaded"])).page, Some("drivers"));
        assert_eq!(host.command_line(&args(&["--drivers-scope", "everything"])).page, None, "an invalid value is ignored");
        assert_eq!(host.command_line(&args(&[])).page, None);
        assert!(runner.launches().is_empty(), "nothing is launched for it");
    }

    #[test]
    fn a_drivers_scope_is_not_saved_until_the_user_picks_another_one() {
        let (ctx, _) = AddonContext::recording();
        let mut addon = Drivers::default();
        addon.load_settings(&serde_json::json!({ "scope": "builtin" }));
        addon.start(&ctx);
        assert!(addon.command_line(&args(&["--drivers-scope", "loaded"])), "asks for its page");
        assert_eq!(addon.settings(), serde_json::json!({ "scope": "builtin" }), "the saved tab stays");
        addon.stop();
        assert_eq!(addon.settings(), serde_json::json!({ "scope": "builtin" }), "also once off");

        addon.start(&ctx);
        addon.command_line(&args(&["--drivers-scope", "loaded"]));
        addon.page().unwrap().drv_ui.kernel_filter = KernelScope::ThirdParty;
        assert_eq!(addon.settings(), serde_json::json!({ "scope": "kernel", "kernelFilter": "third-party" }), "the user's own pick is saved");
    }

    #[test]
    fn a_drivers_scope_while_off_is_ignored() {
        let mut addon = Drivers::default();
        assert!(!addon.command_line(&args(&["--drivers-scope", "loaded"])));
        assert!(addon.page().is_none());
    }
}

/// The listing reader, through the add-on host with a fake reader (#103).
#[cfg(test)]
mod reader_tests {
    use super::*;
    use crate::addons::{until, AddonHost, Entry};
    use crate::platform::When;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use serde_json::json;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// What a read answers, in order (the last answer repeats).
    type Script<T> = Mutex<VecDeque<Result<Vec<T>, String>>>;

    /// A reader that answers from scripts and counts its calls.
    struct Fake {
        devices: Script<DeviceDriver>,
        kernel: Script<KernelDriver>,
        devices_asked: AtomicUsize,
        kernel_asked: AtomicUsize,
    }

    fn next<T: Clone>(script: &Script<T>) -> Result<Vec<T>, String> {
        let mut script = script.lock().unwrap();
        if script.len() > 1 {
            script.pop_front().unwrap()
        } else {
            script.front().cloned().unwrap_or_else(|| Ok(Vec::new()))
        }
    }

    impl Fake {
        fn answering(devices: Vec<Result<Vec<DeviceDriver>, String>>, kernel: Vec<Result<Vec<KernelDriver>, String>>) -> Arc<Fake> {
            Arc::new(Fake {
                devices: Mutex::new(devices.into()),
                kernel: Mutex::new(kernel.into()),
                devices_asked: AtomicUsize::new(0),
                kernel_asked: AtomicUsize::new(0),
            })
        }
        /// A reader whose kernel list is empty.
        fn devices(devices: Vec<Result<Vec<DeviceDriver>, String>>) -> Arc<Fake> {
            Fake::answering(devices, vec![Ok(Vec::new())])
        }
        fn devices_asked(&self) -> usize {
            self.devices_asked.load(Ordering::SeqCst)
        }
        fn kernel_asked(&self) -> usize {
            self.kernel_asked.load(Ordering::SeqCst)
        }
    }

    impl ListingReader for Fake {
        fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
            self.devices_asked.fetch_add(1, Ordering::SeqCst);
            next(&self.devices)
        }
        fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
            self.kernel_asked.fetch_add(1, Ordering::SeqCst);
            next(&self.kernel)
        }
    }

    fn device(name: &str) -> DeviceDriver {
        DeviceDriver { instance_id: name.into(), name: name.into(), class: "Display".into(), provider: "Acme".into(), present: true, ..Default::default() }
    }

    fn host_with(reader: &Arc<Fake>, section: serde_json::Value) -> (AddonHost, Arc<crate::platform::RecordingRunner>) {
        let reader: Arc<dyn ListingReader> = reader.clone();
        let entries = vec![Entry::Addon(Box::new(Drivers::with_reader(reader)))];
        AddonHost::for_tests_with(entries, &json!({ "drivers": section }))
    }

    fn on(reader: &Arc<Fake>) -> AddonHost {
        host_with(reader, json!({ "enabled": true })).0
    }

    /// Every text of one frame of the page, as the user sees it.
    fn shown(host: &mut AddonHost) -> String {
        fn texts(shape: &egui::Shape, out: &mut String) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut text = String::new();
        // Two passes: the first one only sizes what the page lays out.
        for _ in 0..2 {
            text.clear();
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    host.ui("drivers", ui, &mut PageContext::new(&settings, &sample));
                });
            });
            output.shapes.iter().for_each(|s| texts(&s.shape, &mut text));
        }
        text
    }

    #[test]
    fn opening_the_page_reads_devices_and_kernel_drivers_once() {
        let reader = Fake::devices(vec![Ok(vec![device("Alpha GPU")])]);
        let mut host = on(&reader);
        until("the row is shown", || shown(&mut host).contains("Alpha GPU"));
        shown(&mut host);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!((reader.devices_asked(), reader.kernel_asked()), (1, 1), "drawing the page again reads nothing more");
    }

    #[test]
    fn a_failed_device_read_shows_its_reason() {
        let reader = Fake::devices(vec![Err("the listing broke".into())]);
        let mut host = on(&reader);
        until("the failure is shown", || shown(&mut host).contains("the listing broke"));
        assert!(shown(&mut host).contains("Drivers could not be enumerated"));
    }

    #[test]
    fn a_failed_device_read_drops_the_old_rows() {
        let reader = Fake::devices(vec![Ok(vec![device("Alpha GPU")]), Err("the listing broke".into())]);
        let mut host = on(&reader);
        until("the first read is shown", || shown(&mut host).contains("Alpha GPU"));
        assert!(host.refresh("drivers"));
        until("the failure is shown", || shown(&mut host).contains("the listing broke"));
        assert!(!shown(&mut host).contains("Alpha GPU"), "the old rows are gone");
    }

    #[test]
    fn a_successful_read_clears_the_error() {
        let reader = Fake::devices(vec![Err("the listing broke".into()), Ok(vec![device("Beta NIC")])]);
        let mut host = on(&reader);
        until("the failure is shown", || shown(&mut host).contains("the listing broke"));
        assert!(host.refresh("drivers"));
        until("the new rows are shown", || shown(&mut host).contains("Beta NIC"));
        let page = shown(&mut host);
        assert!(!page.contains("the listing broke") && !page.contains("could not be enumerated"), "the error is cleared: {page}");
    }

    #[test]
    fn a_failed_kernel_read_shows_its_reason_on_the_kernel_tab() {
        let reader = Fake::answering(vec![Ok(vec![device("Alpha GPU")])], vec![Err("the service manager refused".into())]);
        let (mut host, _) = host_with(&reader, json!({ "enabled": true, "settings": { "scope": "kernel" } }));
        until("the failure is shown", || shown(&mut host).contains("the service manager refused"));
        assert!(shown(&mut host).contains("Drivers could not be enumerated"));
        assert_eq!(reader.kernel_asked(), 1);
    }

    #[test]
    fn an_unavailable_listing_shows_as_unavailable_and_never_as_an_empty_list() {
        use crate::availability::{unavailable_in_test, Feature};
        let _unavailable = unavailable_in_test(&[Feature::DeviceSetupApi]);
        let reader = Fake::devices(vec![Ok(vec![device("Alpha GPU")])]);
        let mut host = on(&reader);
        until("the unavailable state is shown", || shown(&mut host).contains("Not available on this system."));
        let page = shown(&mut host);
        assert!(!page.contains("No drivers were reported") && !page.contains("Alpha GPU"), "not an empty list: {page}");
        assert_eq!((reader.devices_asked(), reader.kernel_asked()), (0, 0), "nothing is read from a system without drivers");
    }

    #[test]
    fn turned_off_refresh_asks_the_reader_nothing() {
        let reader = Fake::devices(vec![Ok(vec![device("Alpha GPU")])]);
        let (mut host, _) = host_with(&reader, json!({ "enabled": false }));
        assert!(!host.refresh("drivers"), "no page to refresh");
        assert_eq!(shown(&mut host), "", "nothing is drawn");
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!((reader.devices_asked(), reader.kernel_asked()), (0, 0));
    }

    #[test]
    fn windows_update_offers_are_matched_against_the_drivers_a_refresh_just_listed() {
        let installed = |version: &str| DeviceDriver { hardware_ids: vec![r"PCI\VEN_1".into()], version: version.into(), date: "2020-01-01".into(), ..device("Alpha GPU") };
        let reader = Fake::devices(vec![Ok(vec![installed("1.0.0.0")]), Ok(vec![installed("2.0.0.0")])]);
        let (mut host, runner) = host_with(&reader, json!({ "enabled": true, "settings": { "checkOnOpen": true } }));
        let offer = json!({ "ok": true, "updates": [{ "Id": "u1", "Title": "Acme GPU (2.0.0.0)", "HardwareId": r"PCI\VEN_1", "Date": "2024-01-01" }] });
        runner.prepare(When::Nth(0), 0, &offer.to_string());

        until("the offer is newer than the installed driver", || shown(&mut host).contains("New: 2.0.0.0"));

        assert!(host.refresh("drivers"));
        until("the refresh lists the driver the offer now equals", || !shown(&mut host).contains("New: 2.0.0.0"));
        let page = shown(&mut host);
        assert!(page.contains("Up to date"), "{page}");
        assert!(page.contains("Windows Update offers the older 2.0.0.0"), "{page}");
        assert_eq!(reader.devices_asked(), 2);
    }
}
