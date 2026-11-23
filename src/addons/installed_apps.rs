//! The Installed Apps add-on: installed programs, their updates (winget)
//! and uninstalling them. Reading the list and asking winget stop at once
//! when the add-on is turned off; an update, uninstall or setup that is
//! running is not cut short: the add-on turns off when it ends, and starts
//! no further app of a batch meanwhile.

/// The list of apps, winget and the operations on them.
pub mod apps;
/// The Installed Apps page.
pub mod page;

use crate::addons::{is_default, read_settings, write_settings, Addon, Consumption, Asked, Capability, PathOwner, Requirements};
use crate::platform::{Action, AddonContext, PageContext};

use apps::Hub;

/// Its id and shown name (also in its install-queue requests).
const ID: &str = "installed-apps";
const NAME: &str = "Installed Apps";
use page::AppsPage;

/// The note under its requirements in Settings › Add-ons.
pub const NEEDS: &str = "Updates need the internet and winget (App Installer); the Store apps are read with PowerShell. \
Administrator rights are asked through UAC only when an installer or uninstaller needs them: do not start this app as administrator.";

/// The add-on: its page while it is on, and while it waits to turn off.
#[derive(Default)]
pub struct InstalledApps {
    /// The page and its background work, while the add-on is on. Turned
    /// off during an update, uninstall or setup, the page stays (its hub
    /// stopped) to follow that operation until it ends.
    page: Option<AppsPage>,
    /// Its settings; the group and sort are taken from the page while
    /// there is one.
    prefs: Prefs,
}

/// Installed Apps' own settings, as saved: a default value is left out,
/// and the fields it does not know are kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Default, Clone)]
#[serde(default)]
struct Prefs {
    /// The group of apps listed (None: Updates when there are updates).
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<page::ListGroup>,
    /// Saved as "sort" and "sortDesc" (see [`page::SortOrder`]).
    #[serde(flatten)]
    sort: page::SortOrder,
    /// "Check for updates when the page opens" (off by default).
    #[serde(rename = "checkOnOpen", skip_serializing_if = "is_default")]
    check_on_open: bool,
    /// The Summary tile is on (off by default).
    #[serde(rename = "summaryTile", skip_serializing_if = "is_default")]
    summary_tile: bool,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl Prefs {
    /// Puts the saved group, sort and "Check for updates when the page
    /// opens" on the page.
    fn apply_to(&self, page: &mut AppsPage) {
        page.apps_ui.list_group = self.group;
        page.apps_ui.sort = self.sort;
        page.check_on_open = self.check_on_open;
    }
}

impl InstalledApps {
    /// The page while the add-on is on (or waits to turn off).
    #[cfg(test)]
    pub fn page(&mut self) -> Option<&mut AppsPage> {
        self.page.as_mut()
    }

    /// The Summary tile's lines: how many apps have a newer version and
    /// when winget was asked. Only when the tile is on and the user
    /// pressed Check in this session; the tile never starts a check.
    pub fn summary_lines(&self) -> Option<Vec<String>> {
        if !self.prefs.summary_tile {
            return None;
        }
        let st = self.page.as_ref()?.apps.lock();
        if !st.checked_by_hand {
            return None;
        }
        let newer = st.apps.iter().filter(|a| a.has_update()).count();
        let found = format!("{} {} a newer version", crate::addons::plural(newer, "app"), if newer == 1 { "has" } else { "have" });
        Some(crate::addons::checked_tile_lines(found, &st.checked_at))
    }

    /// The settings with the page's current choices.
    fn current(&self) -> Prefs {
        let mut prefs = self.prefs.clone();
        if let Some(page) = &self.page {
            prefs.group = page.apps_ui.list_group;
            prefs.sort = page.apps_ui.sort;
        }
        prefs
    }
}

impl Addon for InstalledApps {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        NAME
    }
    fn description(&self) -> &'static str {
        "Installed programs, their updates and uninstall, via winget."
    }
    /// The internet and winget for the updates, PowerShell for the Store
    /// apps; administrator rights only when a setup asks for them.
    fn requirements(&self) -> Requirements {
        Requirements { admin: true, network: true, programs: &["winget (App Installer)", "PowerShell"], note: Some(NEEDS), ..Requirements::default() }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Medium", network: "On checks and updates", disk: "On install and uninstall", when: "On open and on your actions" }
    }

    /// Cheap: the list is read when the page first opens. A page left
    /// from waiting to turn off is replaced by a fresh one.
    /// The page opens on the group and sort last chosen.
    fn start(&mut self, context: &AddonContext) {
        self.prefs = self.current();
        let mut page = AppsPage::new(Hub::new(context.clone()), context.clone());
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

    /// "Check for updates when the page opens" and the Summary tile; the
    /// six months after which an app counts as unused stay fixed.
    fn settings_section(&mut self, ui: &mut egui::Ui, _page: &mut PageContext) {
        use crate::widgets::parts::{group_heading, setting_switch};
        group_heading(ui, "Updates");
        if setting_switch(ui, "Check for updates when the page opens", None, &mut self.prefs.check_on_open) {
            if let Some(page) = &mut self.page {
                page.check_on_open = self.prefs.check_on_open;
            }
        }
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("Apps with a newer version, after you check"), &mut self.prefs.summary_tile);
    }

    /// How many apps have a newer version, once the user checked.
    fn summary(&mut self, ui: &mut egui::Ui, page: &mut PageContext) {
        if let Some(lines) = self.summary_lines() {
            crate::addons::summary_tile(ui, page, "Installed Apps", lines);
        }
    }

    /// Stops the hub; the page goes at once unless an operation it must
    /// not cut short still runs, which the page keeps following. The
    /// group and sort chosen are kept.
    fn stop(&mut self) {
        self.prefs = self.current();
        if let Some(page) = &self.page {
            page.apps.stop();
            if !page.apps.running() {
                self.page = None;
            }
        }
    }

    fn uninterruptible(&self) -> bool {
        self.page.as_ref().is_some_and(|p| p.apps.running())
    }

    fn ui(&mut self, ui: &mut egui::Ui, ctx: &mut PageContext) {
        if let Some(page) = &mut self.page {
            page.ui(ui);
            ctx.actions.extend(page.said.drain(..).map(Action::Say));
        }
    }

    fn refresh(&mut self) {
        if let Some(page) = &self.page {
            page.apps.refresh();
        }
    }

    /// Re-reads its list after another add-on's install job (its own
    /// updates and uninstalls re-read it themselves).
    fn packages_changed(&mut self, by: &'static str) {
        if let (Some(page), false) = (&self.page, by == ID) {
            page.apps.packages_changed();
        }
    }

    fn offers(&mut self, asked: Asked) -> Option<Capability<'_>> {
        match asked {
            Asked::PathOwner => Some(Capability::PathOwner(self)),
            _ => None,
        }
    }
}

impl PathOwner for InstalledApps {
    /// The app whose own folder holds `path` (Disk Space links to it).
    /// Only apps already listed count: the list is read when the page
    /// first opens.
    fn owner_of(&self, path: &std::path::Path) -> Option<String> {
        let page = self.page.as_ref()?;
        let st = page.apps.lock();
        owner(&st.apps, path).map(|a| a.name.clone())
    }

    /// Selects that app's row and clears what would hide it.
    fn reveal(&mut self, path: &std::path::Path) {
        let Some(page) = &mut self.page else {
            return;
        };
        let key = owner(&page.apps.lock().apps, path).map(|a| a.key.clone());
        if let Some(key) = key {
            page.apps_ui.reveal(key);
        }
    }
}

/// The app whose own folder holds `path`, the deepest one when folders
/// nest (case and a trailing `\` aside).
fn owner<'a>(apps: &'a [apps::App], path: &std::path::Path) -> Option<&'a apps::App> {
    let norm = |s: &str| s.replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let file = norm(&path.to_string_lossy());
    apps.iter()
        .filter_map(|a| a.folder().map(|f| (a, norm(&f))))
        .filter(|(_, dir)| !dir.is_empty() && file.strip_prefix(dir.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('\\')))
        .max_by_key(|(_, dir)| dir.len())
        .map(|(a, _)| a)
}

#[cfg(test)]
mod tests {
    use super::apps::App;
    use super::*;
    use crate::addons::until;
    use crate::platform::Launch;

    fn tool(key: &str) -> App {
        App { key: key.into(), name: key.into(), uninstall: format!("{key}.exe /S"), ..Default::default() }
    }

    fn at(key: &str, location: &str) -> App {
        App { location: location.into(), ..tool(key) }
    }

    #[test]
    fn it_reloads_its_list_when_another_addon_changed_the_installed_packages() {
        let (ctx, _) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let listed = std::sync::Arc::new(std::sync::Mutex::new(vec![tool("old")]));
        let source = listed.clone();
        let hub = addon.page().unwrap().apps.clone().with_registry(move || source.lock().unwrap().clone());
        addon.page().unwrap().apps = hub.clone();
        addon.refresh();
        until("the list is read", || !hub.lock().listing && hub.lock().apps.len() == 1);

        *listed.lock().unwrap() = vec![tool("old"), tool("new")];
        addon.packages_changed("installed-apps");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(hub.lock().apps.len(), 1, "its own jobs re-read the list themselves");
        addon.packages_changed("drivers");
        until("the list is read again", || hub.lock().apps.len() == 2);
    }

    #[test]
    fn an_unavailable_listing_reports_it_and_reads_nothing() {
        use crate::availability::{unavailable_in_test, Feature};
        let _unavailable = unavailable_in_test(&[Feature::UninstallRegistry]);
        let (ctx, _) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone().with_registry(|| vec![tool("a")]);
        addon.page().unwrap().apps = hub.clone();
        addon.refresh();
        until("the listing ends", || !hub.lock().listing);
        let st = hub.lock();
        assert_eq!(st.message, Some(("Not available on this system.".to_string(), true)), "shown as unavailable");
        assert!(st.apps.is_empty(), "the registry is not read");
    }

    #[test]
    fn a_list_never_read_is_not_read_when_packages_change() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        addon.packages_changed("drivers");
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(runner.launches().is_empty(), "the list is read when the page opens");
        assert!(!addon.page().unwrap().apps.lock().listing);
    }

    #[test]
    fn its_requirements_name_winget_powershell_and_admin_only_through_uac() {
        let (host, _) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "installed-apps": { "enabled": false } }));
        let d = crate::addons::overview::Overview::of(&host, Some("installed-apps")).detail.unwrap();
        assert_eq!(
            d.requirements,
            ["Asks for administrator rights for some actions", "Uses the internet", "Runs winget (App Installer)", "Runs PowerShell"],
            "shown before turning it on"
        );
        let note = d.note.expect("a note under the requirements");
        for words in ["winget", "Store apps", "UAC", "not start this app as administrator"] {
            assert!(note.contains(words), "the note says {words:?}: {note}");
        }
    }

    #[test]
    fn the_group_and_sort_chosen_come_back_after_turning_it_off_and_on() {
        use page::{ListGroup, Sort, SortOrder};
        let (ctx, _) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.load_settings(&serde_json::Value::Null);
        addon.start(&ctx);
        assert_eq!(addon.settings(), serde_json::Value::Null, "nothing chosen, nothing saved");
        let ui = &mut addon.page().unwrap().apps_ui;
        ui.list_group = Some(ListGroup::Store);
        ui.click_heading(Sort::Size);
        addon.stop();
        let saved = addon.settings();
        assert_eq!(saved, serde_json::json!({ "group": "store", "sort": "size" }), "saved under its id while off; biggest first is Size's own direction");
        addon.start(&ctx);
        let ui = &addon.page().unwrap().apps_ui;
        assert_eq!((ui.list_group, ui.sort), (Some(ListGroup::Store), SortOrder { column: Sort::Size, desc: true }), "back once on again");

        let mut next_run = InstalledApps::default();
        next_run.load_settings(&serde_json::json!({ "group": "updates", "sort": "last-used", "fromANewerVersion": 1 }));
        next_run.start(&ctx);
        let ui = &next_run.page().unwrap().apps_ui;
        assert_eq!((ui.list_group, ui.sort), (Some(ListGroup::Updates), SortOrder { column: Sort::LastUsed, desc: true }), "read from the saved settings");
        assert_eq!(next_run.settings()["fromANewerVersion"], 1, "fields it does not know are kept");
    }

    #[test]
    fn the_sort_direction_comes_back_after_turning_it_off_and_on() {
        use page::{Sort, SortOrder};
        let (ctx, _) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let ui = &mut addon.page().unwrap().apps_ui;
        ui.click_heading(Sort::Size);
        ui.click_heading(Sort::Size);
        addon.stop();
        assert_eq!(addon.settings(), serde_json::json!({ "sort": "size", "sortDesc": false }), "smallest first is saved");
        addon.start(&ctx);
        assert_eq!(addon.page().unwrap().apps_ui.sort, SortOrder { column: Sort::Size, desc: false }, "and comes back");

        addon.page().unwrap().apps_ui.click_heading(Sort::Name);
        addon.page().unwrap().apps_ui.click_heading(Sort::Name);
        assert_eq!(addon.settings(), serde_json::json!({ "sortDesc": true }), "names Z to A");
        addon.stop();
        addon.start(&ctx);
        assert_eq!(addon.page().unwrap().apps_ui.sort, SortOrder { column: Sort::Name, desc: true });

        // Settings saved before the direction was: each column's own direction.
        for (saved, column, desc) in [("size", Sort::Size, true), ("last-used", Sort::LastUsed, true), ("version", Sort::Version, false), ("name", Sort::Name, false)] {
            let mut old = InstalledApps::default();
            old.load_settings(&serde_json::json!({ "sort": saved }));
            old.start(&ctx);
            assert_eq!(old.page().unwrap().apps_ui.sort, SortOrder { column, desc }, "{saved}");
        }
    }

    // Windows only: the list is read through the registry and PowerShell.
    #[cfg(windows)]
    #[test]
    fn checking_for_updates_when_the_page_opens_is_off_by_default() {
        let asks_winget = |runner: &crate::platform::RecordingRunner| runner.launches().iter().any(|l| matches!(l, Launch::Winget(_)));
        for (settings, checks) in [(serde_json::Value::Null, false), (serde_json::json!({ "checkOnOpen": true }), true)] {
            let (ctx, runner) = AddonContext::recording();
            let mut addon = InstalledApps::default();
            addon.load_settings(&settings);
            addon.start(&ctx);
            assert!(runner.launches().is_empty(), "turning on launches nothing");
            addon.page().unwrap().opened();
            until("the list is read", || runner.launches().iter().any(|l| matches!(l, Launch::PowerShell { .. })));
            if checks {
                until("winget is asked", || asks_winget(&runner));
            } else {
                std::thread::sleep(std::time::Duration::from_millis(100));
                assert!(!asks_winget(&runner), "no check unless asked for");
            }
            addon.stop();
            assert_eq!(addon.settings(), settings, "saved only when turned on");
        }
    }

    #[test]
    fn the_summary_tile_shows_only_after_a_check_by_hand_and_only_when_turned_on() {
        let newer = |key: &str| App { available: "2.0".into(), version: "1.0".into(), winget_id: format!("Vendor.{key}"), ..tool(key) };
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        assert_eq!(addon.summary_lines(), None, "off: no tile");
        addon.load_settings(&serde_json::json!({ "summaryTile": true }));
        addon.start(&ctx);
        assert_eq!(addon.summary_lines(), None, "no check yet, and none is started for the tile");
        assert!(runner.launches().is_empty());

        let hub = addon.page().unwrap().apps.clone();
        let mut st = hub.lock();
        st.apps = vec![newer("a"), newer("b"), tool("c")];
        st.winget = Some(Vec::new());
        st.checked_at = "14:03".into();
        drop(st);
        assert_eq!(addon.summary_lines(), None, "a check made when the page opened does not count");
        hub.lock().checked_by_hand = true;
        assert_eq!(addon.summary_lines(), Some(vec!["2 apps have a newer version".to_string(), "Last checked at 14:03".to_string()]));
        hub.lock().apps.truncate(1);
        assert_eq!(addon.summary_lines().unwrap()[0], "1 app has a newer version");

        addon.load_settings(&serde_json::Value::Null);
        assert_eq!(addon.summary_lines(), None, "the tile is off by default");
    }

    /// A folder measure that counts its calls and holds each one until
    /// the gate opens; every folder weighs 42 bytes. The registry lists
    /// the apps given.
    #[derive(Clone, Default)]
    struct SlowMeasure {
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        gate: std::sync::Arc<(std::sync::Mutex<bool>, std::sync::Condvar)>,
    }

    impl SlowMeasure {
        fn hub(&self, ctx: &AddonContext, apps: Vec<App>) -> Hub {
            let me = self.clone();
            Hub::new(ctx.clone()).with_registry(move || apps.clone()).with_measure(move |_, _| {
                me.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (open, cv) = &*me.gate;
                let mut open = open.lock().unwrap();
                while !*open {
                    open = cv.wait(open).unwrap();
                }
                42
            })
        }
        fn calls(&self) -> usize {
            self.calls.load(std::sync::atomic::Ordering::SeqCst)
        }
        fn open(&self) {
            *self.gate.0.lock().unwrap() = true;
            self.gate.1.notify_all();
        }
    }

    /// Two app folders that exist (an app's folder counts only then).
    fn two_app_folders(tag: &str) -> (std::path::PathBuf, Vec<App>) {
        let root = std::env::temp_dir().join(format!("upm-measure-{tag}-{}", std::process::id()));
        let apps = ["One", "Two"]
            .iter()
            .map(|sub| {
                std::fs::create_dir_all(root.join(sub)).unwrap();
                at(&sub.to_lowercase(), &root.join(sub).display().to_string())
            })
            .collect();
        (root, apps)
    }

    // Windows only: an app folder is a registry location on a drive.
    #[cfg(windows)]
    #[test]
    fn turning_off_stops_a_slow_folder_measure_and_drops_its_results() {
        let (root, apps) = two_app_folders("stop");
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let fake = SlowMeasure::default();
        let hub = fake.hub(&ctx, apps);
        addon.page().unwrap().apps = hub.clone();
        addon.refresh();
        until("the first folder is being measured", || fake.calls() == 1);
        assert!(hub.lock().measuring.is_some());
        assert_eq!(hub.lock().apps.len(), 2, "the refresh listed both apps");
        let launched = runner.launches();

        addon.stop();
        assert!(addon.page().is_none());
        assert!(hub.lock().measuring.is_none() && hub.lock().apps.is_empty(), "dropped at once");
        fake.open();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(fake.calls(), 1, "the next folder is not measured");
        let st = hub.lock();
        assert!(st.apps.is_empty() && st.measuring.is_none(), "the late size is dropped");
        assert_eq!(runner.launches(), launched, "nothing is launched once off");
    }

    #[test]
    fn turned_off_it_asks_for_no_program_nor_measure() {
        let (mut host, runner) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "installed-apps": { "enabled": false, "settings": { "checkOnOpen": true, "summaryTile": true } } }));
        assert!(!host.refresh("installed-apps"), "no page to refresh");
        assert!(runner.launches().is_empty(), "off from the start: nothing is launched");

        let (root, apps) = two_app_folders("off");
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let fake = SlowMeasure::default();
        fake.open();
        let hub = fake.hub(&ctx, apps);
        addon.page().unwrap().apps = hub.clone();
        addon.stop();
        hub.refresh();
        hub.check_updates();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let _ = std::fs::remove_dir_all(root);
        assert_eq!(runner.launches(), Vec::<Launch>::new(), "no winget, no PowerShell");
        assert_eq!(fake.calls(), 0, "no folder is measured");
        assert!(addon.summary_lines().is_none());
    }

    // Windows only: an app folder is a registry location on a drive.
    #[cfg(windows)]
    #[test]
    fn a_file_in_an_apps_folder_is_owned_by_that_app_and_revealed_on_its_row() {
        // An app's folder counts only when it exists.
        let games = std::env::temp_dir().join(format!("upm-owner-{}", std::process::id())).join("Games");
        for sub in ["Tool", "Toolbox"] {
            std::fs::create_dir_all(games.join(sub)).unwrap();
        }
        let text = |p: &std::path::Path| p.display().to_string();
        let (ctx, _) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        let file = games.join("Tool").join("Data").join("big.pak");
        assert_eq!(addon.owner_of(&file), None, "off: no page, no owner");
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone();
        let tool_dir = format!("{}\\", text(&games.join("Tool")).to_uppercase());
        hub.lock().apps = vec![at("games", &text(&games)), at("tool", &tool_dir), at("toolbox", &text(&games.join("Toolbox")))];
        assert_eq!(addon.owner_of(&file), Some("tool".into()), "the deepest folder, case and trailing \\ aside");
        assert_eq!(addon.owner_of(&games.join("Tools.txt")), Some("games".into()), "a name sharing a prefix is not inside");
        assert_eq!(addon.owner_of(std::path::Path::new(r"Q:\other.bin")), None);

        let page = addon.page().unwrap();
        page.apps_ui.search = "zzz".into();
        addon.reveal(&file);
        let _ = std::fs::remove_dir_all(games.parent().unwrap());
        let page = addon.page().unwrap();
        assert_eq!(page.apps_ui.selected.as_deref(), Some("tool"));
        assert!(page.apps_ui.search.is_empty(), "the search would hide its row");
    }

    #[test]
    fn turning_off_during_an_uninstall_keeps_its_page_to_follow_it_and_shows_nothing_of_it_later() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone();
        hub.lock().apps = vec![tool("first"), tool("second")];
        runner.hold();
        hub.uninstall(vec!["first".into(), "second".into()]);
        runner.wait_for(1);
        assert!(addon.uninterruptible(), "an uninstaller is running");

        addon.stop();
        assert!(addon.uninterruptible(), "turning off does not cut the uninstall short");
        let page = addon.page().expect("the page stays to follow the uninstall");
        assert!(page.apps.lock().busy.as_deref().is_some_and(|b| b.contains("first")), "it shows the running uninstall");
        assert_eq!(page.apps.lock().apps.len(), 2, "and the list it was started from");
        runner.release();
        until("the uninstall ends", || !addon.uninterruptible());
        assert_eq!(runner.launches(), vec![Launch::Program { file: "first.exe".into(), params: "/S".into() }], "no further app is uninstalled, the list is not read again");
        let st = addon.page().unwrap().apps.lock();
        assert!(st.busy.is_none() && st.message.is_some(), "the page shows how the uninstall ended");
        drop(st);

        addon.start(&ctx);
        let st = addon.page().unwrap().apps.lock();
        assert!(st.apps.is_empty() && st.message.is_none() && st.busy.is_none(), "nothing of the old uninstall is shown");
    }

    #[test]
    fn an_update_that_fails_in_the_middle_of_a_series_does_not_stop_the_apps_after_it() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let hub = addon.page().unwrap().apps.clone();
        let newer = |key: &str| App { winget_id: format!("Vendor.{key}"), available: "2.0".into(), ..tool(key) };
        hub.lock().apps = vec![newer("first"), newer("second"), newer("third")];
        let when = |key: &str| crate::platform::When::Arg(format!("Vendor.{key}"));
        runner.prepare(when("first"), 0, "Successfully installed");
        runner.prepare(when("second"), 1603, "Installer failed");
        runner.prepare(when("third"), 0, "Successfully installed");

        hub.update(vec!["first".into(), "second".into(), "third".into()]);
        until("the updates end", || hub.lock().message.is_some());

        let upgraded: Vec<String> = runner
            .launches()
            .into_iter()
            .filter_map(|l| match l {
                Launch::Winget(args) if args[0] == "upgrade" => Some(args[2].clone()),
                _ => None,
            })
            .collect();
        assert_eq!(upgraded, ["Vendor.first", "Vendor.second", "Vendor.third"], "every app is updated, in order");
        let (text, error) = hub.lock().message.clone().unwrap();
        assert!(error, "a failure is an error: {text}");
        assert!(text.contains("first") && text.contains("third") && text.contains("Not updated: second ("), "{text}");
    }

    #[test]
    fn opening_an_app_folder_goes_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        let page = addon.page().unwrap();
        page.open_folder(r"C:\Program Files\Tool");
        assert_eq!(runner.launches(), vec![Launch::Open(r"C:\Program Files\Tool".into())]);
        assert_eq!(page.said.len(), 1, "the failure is told in the status bar");
    }

    #[test]
    fn turning_off_stops_asking_winget_at_once() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        runner.hold();
        addon.page().unwrap().apps.check_updates();
        runner.wait_for(1);
        assert!(!addon.uninterruptible(), "asking winget can be stopped");

        let old = addon.page().unwrap().apps.clone();
        addon.stop();
        assert!(addon.page().is_none(), "with nothing to wait for, the page goes at once");
        runner.release();
        until("the old check ends", || old.lock().busy.is_none());
        addon.start(&ctx);
        let st = addon.page().unwrap().apps.lock();
        assert!(st.message.is_none() && st.winget.is_none(), "the late winget answer is dropped");
    }

    // Windows only: Store apps are listed through PowerShell.
    #[cfg(windows)]
    #[test]
    fn turning_off_stops_reading_the_list_at_once() {
        let (ctx, runner) = AddonContext::recording();
        let mut addon = InstalledApps::default();
        addon.start(&ctx);
        runner.hold();
        addon.refresh();
        runner.wait_for(1); // Store apps are asked from PowerShell
        assert!(!addon.uninterruptible());
        let old = addon.page().unwrap().apps.clone();
        addon.stop();
        assert!(old.lock().apps.is_empty(), "the partial list is dropped at once");
        runner.release();
        until("the old listing ends", || !old.lock().listing);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(old.lock().apps.is_empty(), "the old listing publishes nothing");
        addon.start(&ctx);
        assert!(addon.page().unwrap().apps.lock().apps.is_empty());
    }
}
