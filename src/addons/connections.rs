//! The Connections add-on: open network connections (TCP and UDP, IPv4 and
//! IPv6) and the programs that own them. A worker thread reads the tables,
//! at the app's update speed and on Refresh, only while its page is open, its
//! Summary tile is shown or its Processes column is; turning it off stops the worker and drops the
//! inventory, so turning it on again shows a fresh one. Its settings (default scope, grouping by
//! exposure, sort, System traffic, Summary tile) stay saved while it is off.

/// The four connection tables read into an inventory, and the rules over a row.
pub mod inventory;
/// The thread that reads the inventory off the drawing thread.
pub mod worker;
/// The page: the inventory line and the table.
pub mod page;
/// How long each connection has been seen.
pub mod observed;
/// The side panel: the selected row's details and the exposure notes.
pub mod panel;
/// The "Global connections" globe at the top of the side panel.
pub mod globe;
/// The table's rows grouped by exposure and sorted.
pub mod groups;
/// Scope membership and counts, and the Find box.
pub mod scope;
/// The status band: the sentence, its subtitle and the five tiles.
pub mod band;
/// The screen-reader texts of the summary, the scope control and the rows.
pub mod a11y;
/// The actions on a connection, in the panel and the row's menu.
pub mod actions;

use std::collections::HashMap;
use std::sync::Arc;

use egui::{RichText, Ui};

use crate::addons::{is_default, is_true, plural, read_settings, write_settings, Addon, Consumption, Offers, ProcessColumn, ProcessColumnProvider, Requirements};
use crate::platform::{Action, PageContext};
use crate::widgets::parts;

use inventory::{Inventory, Row, Status};
use worker::{Reader, Worker};

/// The add-on's id, and its page's.
pub const ID: &str = "connections";

/// Which rows the page lists.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Every row.
    #[default]
    All,
    /// TCP connections with a remote end.
    Connected,
    /// TCP sockets listening.
    Listening,
    /// TCP listeners on all interfaces.
    Public,
    /// UDP endpoints.
    Udp,
}

impl Scope {
    /// Every scope that can be chosen, in the page's order.
    pub const ALL: [Scope; 5] = [Scope::All, Scope::Connected, Scope::Listening, Scope::Public, Scope::Udp];

    /// The shown name.
    pub fn label(self) -> &'static str {
        match self {
            Scope::All => "All",
            Scope::Connected => "Connected",
            Scope::Listening => "Listening",
            Scope::Public => "Public",
            Scope::Udp => "UDP",
        }
    }
}

/// The column the table is sorted by.
#[derive(serde::Deserialize, serde::Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortColumn {
    #[default]
    Application,
    Protocol,
    State,
    Local,
    Remote,
    Exposure,
}

impl SortColumn {
    /// Every column, in the table's order.
    pub const ALL: [SortColumn; 6] =
        [SortColumn::Application, SortColumn::Protocol, SortColumn::State, SortColumn::Local, SortColumn::Remote, SortColumn::Exposure];

    /// The column's header.
    pub fn label(self) -> &'static str {
        match self {
            SortColumn::Application => "Application",
            SortColumn::Protocol => "Protocol",
            SortColumn::State => "State",
            SortColumn::Local => "Local endpoint",
            SortColumn::Remote => "Remote / service",
            SortColumn::Exposure => "Exposure",
        }
    }
}

/// Connections' own settings, as saved under its id (kept while it is
/// off): a default value is left out, and the fields it does not know are
/// kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Prefs {
    /// The scope the page opens on.
    #[serde(skip_serializing_if = "is_default")]
    pub scope: Scope,
    /// Rows grouped by exposure (on by default).
    #[serde(rename = "groupByExposure", skip_serializing_if = "is_true")]
    pub group_by_exposure: bool,
    /// The column the table is sorted by.
    #[serde(rename = "sortBy", skip_serializing_if = "is_default")]
    pub sort_by: SortColumn,
    /// Sorted from the last value to the first.
    #[serde(rename = "sortDescending", skip_serializing_if = "is_default")]
    pub sort_descending: bool,
    /// The System traffic tile (on by default).
    #[serde(rename = "showSystemTraffic", skip_serializing_if = "is_true")]
    pub show_system_traffic: bool,
    /// The Summary tile is on (off by default).
    #[serde(rename = "summaryTile", skip_serializing_if = "is_default")]
    pub summary_tile: bool,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            scope: Scope::All,
            group_by_exposure: true,
            sort_by: SortColumn::Application,
            sort_descending: false,
            show_system_traffic: true,
            summary_tile: false,
            rest: serde_json::Map::new(),
        }
    }
}

impl Prefs {
    /// A click on `column`'s header: sort by it, or reverse it when the
    /// table is already sorted by it.
    pub fn sort_on(&mut self, column: SortColumn) {
        if self.sort_by == column {
            self.sort_descending = !self.sort_descending;
        } else {
            self.sort_by = column;
            self.sort_descending = false;
        }
    }
}

/// The TCP listeners on all interfaces (`0.0.0.0` or `[::]`): what could
/// be reachable from outside. UDP wildcards do not count.
pub fn public_listeners(rows: &[Row]) -> usize {
    rows.iter().filter(|r| r.is_public_listener()).count()
}

/// The Summary tile's lines for `inventory` (None: still collecting).
pub fn summary_lines_of(inventory: Option<&Inventory>) -> Vec<String> {
    match inventory {
        Some(inv) if inv.status != Status::Unavailable => {
            vec![plural(public_listeners(&inv.rows), "public listener"), plural(inv.rows.len(), "connection")]
        }
        _ => vec![inventory::status_line(inventory)],
    }
}

/// The add-on: the connection inventory, read by its worker at the app's
/// update speed while its page is open or its Summary tile is shown.
pub struct Connections {
    /// Reads the inventory (the system's, or a test's).
    reader: Reader,
    /// The thread reading it, while the page or the tile is shown.
    worker: Option<Worker>,
    /// The last inventory read; dropped when nothing shows it any more.
    inventory: Option<Arc<Inventory>>,
    /// The page was drawn and has not been hidden since.
    page_shown: bool,
    /// The Summary tile was drawn and has not been hidden since.
    tile_shown: bool,
    /// Its Processes column was drawn and has not been hidden since.
    column_shown: bool,
    /// The connections of each process (by PID) in `inventory`, counted
    /// once per reading for the Processes column.
    counts: Option<(Arc<Inventory>, HashMap<u32, usize>)>,
    /// The table's selection and expanded groups.
    table: page::TableState,
    /// The page's Find text (not saved).
    find: String,
    /// How long each connection has been seen (the panel's Observed).
    observed: observed::Observed,
    /// Its settings.
    prefs: Prefs,
}

impl Default for Connections {
    /// Reads the system's tables.
    fn default() -> Self {
        Self::with_reader(inventory::read)
    }
}

impl Connections {
    /// Connections reading its inventory through `reader` (on its worker).
    pub fn with_reader(reader: impl Fn() -> Inventory + Send + Sync + 'static) -> Self {
        Connections {
            reader: Arc::new(reader),
            worker: None,
            inventory: None,
            page_shown: false,
            tile_shown: false,
            column_shown: false,
            counts: None,
            table: page::TableState::default(),
            find: String::new(),
            observed: observed::Observed::default(),
            prefs: Prefs::default(),
        }
    }

    /// Its settings (the page's default scope, grouping, sort and tiles).
    #[cfg_attr(not(test), allow(dead_code))] // the page's scope will read it
    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    /// The last inventory, after starting the worker if needed and making
    /// it follow the app's update speed `interval_ms`. Never reads here.
    fn current(&mut self, interval_ms: u64) -> Option<Arc<Inventory>> {
        match &self.worker {
            Some(w) => w.set_interval(interval_ms),
            None => self.worker = Some(Worker::start(self.reader.clone(), interval_ms, true)),
        }
        if let Some(latest) = self.worker.as_ref().and_then(Worker::latest) {
            if !self.inventory.as_ref().is_some_and(|known| Arc::ptr_eq(known, &latest)) {
                // A new reading: note the connections it has.
                self.observed.take(&latest);
            }
            self.inventory = Some(latest);
        }
        self.inventory.clone()
    }

    /// Stops the worker (waiting for it) and drops what it read. Observed
    /// is kept: leaving the page does not restart it (only `stop` does).
    fn drop_inventory(&mut self) {
        self.worker = None;
        self.inventory = None;
        self.counts = None;
        self.table = page::TableState::default();
    }

    /// Nothing shows the inventory any more: stop reading it.
    fn hidden(&mut self) {
        if !self.page_shown && !self.tile_shown && !self.column_shown {
            self.drop_inventory();
        }
    }
}

impl ProcessColumnProvider for Connections {
    /// The column "Connections" (an addition): how many rows of the
    /// inventory each process owns.
    fn process_column(&self) -> ProcessColumn {
        ProcessColumn { title: "Connections", menu_item: "Go to connections" }
    }

    fn process_figures(&mut self, settings: &crate::settings::Settings) -> Option<HashMap<u32, usize>> {
        self.column_shown = true;
        let inventory = self.current(settings.interval_ms())?;
        if !self.counts.as_ref().is_some_and(|(of, _)| Arc::ptr_eq(of, &inventory)) {
            let mut by_pid = HashMap::new();
            for r in &inventory.rows {
                *by_pid.entry(r.pid).or_insert(0) += 1;
            }
            self.counts = Some((inventory, by_pid));
        }
        self.counts.as_ref().map(|(_, by_pid)| by_pid.clone())
    }

    /// A hidden column keeps no inventory either.
    fn process_column_hidden(&mut self) {
        self.column_shown = false;
        self.hidden();
    }

    /// Go to connections: the page lists that process's rows, in every
    /// scope (Find "PID n").
    fn reveal_process(&mut self, pid: u32) {
        self.find = format!("PID {pid}");
        self.prefs.scope = Scope::All;
        self.table.selected = None;
    }
}

impl Addon for Connections {
    fn id(&self) -> &'static str {
        ID
    }
    fn name(&self) -> &'static str {
        "Connections"
    }
    /// The add-on's one-line pitch.
    fn description(&self) -> &'static str {
        "Find out what ports your computer is exposing to the Internet"
    }
    fn requirements(&self) -> Requirements {
        Requirements::default()
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Low", network: "None", disk: "None", when: "Only while the page, its Summary tile or its Processes column is shown" }
    }
    /// The page's alternative id.
    fn aliases(&self) -> &'static [&'static str] {
        &["network-connections"]
    }

    /// Its settings ([`Prefs`]); `Null` while nothing was chosen nor saved.
    fn settings(&self) -> serde_json::Value {
        write_settings(&self.prefs)
    }

    fn load_settings(&mut self, settings: &serde_json::Value) {
        self.prefs = read_settings(settings);
    }

    /// Stops the worker (and waits for it) and drops the inventory read
    /// so far and Observed: turned on again, it reads a fresh one.
    fn stop(&mut self) {
        self.page_shown = false;
        self.tile_shown = false;
        self.column_shown = false;
        self.drop_inventory();
        self.observed = observed::Observed::default();
    }

    /// Reads again now (also while updates are paused).
    fn refresh(&mut self) {
        if let Some(w) = &self.worker {
            w.refresh();
        }
    }

    /// A closed page keeps no inventory: nothing is read until it opens
    /// again (unless the Summary tile still shows it).
    /// The page's subtitle, "Connection Management".
    fn subtitle(&self) -> Option<&'static str> {
        Some(page::SUBTITLE)
    }

    /// The inventory line, next to Refresh.
    fn header_status(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let inventory = self.current(page.settings.interval_ms());
        ui.label(RichText::new(inventory::status_line(inventory.as_deref())).color(crate::theme::of_ui(ui).label));
    }

    fn page_hidden(&mut self) {
        self.page_shown = false;
        self.hidden();
    }

    /// A hidden tile keeps no inventory either.
    fn summary_hidden(&mut self) {
        self.tile_shown = false;
        self.hidden();
    }

    fn offers(&mut self) -> Offers<'_> {
        Offers { process_column: Some(self), ..Offers::default() }
    }

    /// The default scope, grouping, sort, the System traffic tile and the
    /// Summary tile.
    fn settings_section(&mut self, ui: &mut Ui, _page: &mut PageContext) {
        use parts::{group_heading, setting_combo, setting_switch, sub_setting_switch};
        group_heading(ui, "When the page opens");
        let prefs = &mut self.prefs;
        setting_combo(ui, "connections-scope", "Open on the scope", None, prefs.scope.label(), |ui| {
            for s in Scope::ALL {
                ui.selectable_value(&mut prefs.scope, s, s.label());
            }
        });
        setting_switch(ui, "Group rows by exposure", None, &mut prefs.group_by_exposure);
        setting_combo(ui, "connections-sort", "Sort by", None, prefs.sort_by.label(), |ui| {
            for c in SortColumn::ALL {
                ui.selectable_value(&mut prefs.sort_by, c, c.label());
            }
        });
        sub_setting_switch(ui, "Descending order", Some("Applies to Sort by"), &mut prefs.sort_descending);
        setting_switch(ui, "Show System traffic", None, &mut prefs.show_system_traffic);
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("Public listeners and connections"), &mut prefs.summary_tile);
    }

    /// Public listeners and connections, with a link to the page.
    fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
        if !self.prefs.summary_tile {
            return;
        }
        self.tile_shown = true;
        let inventory = self.current(page.settings.interval_ms());
        crate::addons::summary_tile(ui, page, "Connections", summary_lines_of(inventory.as_deref()));
        if ui.link("Open Connections").clicked() {
            page.actions.push(Action::OpenPage(ID));
        }
    }

    fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.page_shown = true;
        let inventory = self.current(page.settings.interval_ms());
        if inventory.is_none() {
            // Show the first reading as soon as the worker has it.
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
        }
        // The band runs across the page; the panel sits right of the table.
        egui::TopBottomPanel::top("connections-band").frame(egui::Frame::NONE).show_separator_line(false).show_inside(ui, |ui| {
            band::draw(ui, page, inventory.as_deref(), &mut self.prefs.scope, self.prefs.show_system_traffic);
            // The scopes and Find run across the page, above the table and the panel.
            if let Some(inv) = inventory.as_deref() {
                page::draw_scopes(ui, &inv.rows, &mut self.prefs.scope, &mut self.find);
                ui.add_space(4.0);
            }
        });
        let selected = inventory.as_deref().zip(self.table.selected.as_ref()).and_then(|(inv, key)| groups::find(&inv.rows, key));
        let side = egui::Frame::NONE.inner_margin(egui::Margin { left: 16, ..Default::default() });
        egui::SidePanel::right("connections-details").frame(side).show_separator_line(false).resizable(false).exact_width(356.0).show_inside(ui, |ui| {
            // The globe shows with or without a chosen row; the panel says
            // itself what it shows while none is chosen.
            parts::SidePanel::new("connections-details-scroll")
                .show(ui, Some(()), |ui, ()| panel::draw(ui, page, inventory.as_deref(), selected, &self.observed, &mut self.table));
        });
        egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| {
            // The scope and sort chosen on the page are the saved ones: they are remembered.
            page::draw(ui, page, inventory.as_deref(), &mut self.table, &mut self.prefs, &mut self.find);
        });
        // End process… from the panel or a row's menu asks here first.
        actions::confirm(ui.ctx(), &mut self.table.ending, &mut page.actions);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{until, AddonHost, Entry};
    use crate::platform::PageContext;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use inventory::{assemble, Proto, TableError, TcpState};
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// Runs one windowless egui frame with a page context and `settings`.
    fn frame_with(settings: &Settings, mut f: impl FnMut(&mut egui::Ui, &mut PageContext)) {
        let sample = Snapshot::default();
        let ctx = egui::Context::default();
        // The band and the panel use the semibold face.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| f(ui, &mut PageContext::new(settings, &sample)));
        });
    }

    fn frame(f: impl FnMut(&mut egui::Ui, &mut PageContext)) {
        frame_with(&Settings::default(), f);
    }

    /// Settings with the update speed `speed` (an index of SPEEDS).
    fn at_speed(speed: usize) -> Settings {
        let mut s = Settings::default();
        s.speed = speed;
        s
    }

    fn row(proto: Proto, local: &str, remote: Option<&str>, state: Option<TcpState>) -> Row {
        Row { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid: 4 }
    }

    fn sample_rows() -> Vec<Row> {
        vec![
            row(Proto::Tcp, "0.0.0.0:135", None, Some(TcpState::Listening)),
            row(Proto::Tcp, "[::]:445", None, Some(TcpState::Listening)),
            row(Proto::Tcp, "127.0.0.1:80", None, Some(TcpState::Listening)),
            row(Proto::Tcp, "192.168.1.5:50000", Some("140.82.112.3:443"), Some(TcpState::Established)),
            row(Proto::Udp, "0.0.0.0:5353", None, None),
        ]
    }

    fn full(rows: Vec<Row>) -> Inventory {
        assemble([Ok(rows), Ok(vec![]), Ok(vec![]), Ok(vec![])])
    }

    /// A host holding only Connections with `settings`, reading `rows`;
    /// inventory reads are counted.
    fn host_with(rows: Vec<Row>, settings: serde_json::Value) -> (AddonHost, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let conns = Connections::with_reader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            full(rows.clone())
        });
        let section = json!({ "connections": { "enabled": true, "settings": settings } });
        let (host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(conns))], &section);
        (host, reads)
    }

    fn host() -> (AddonHost, Arc<AtomicUsize>) {
        host_with(vec![row(Proto::Tcp, "127.0.0.1:80", None, Some(TcpState::Listening))], serde_json::Value::Null)
    }

    fn open_page_with(host: &mut AddonHost, settings: &Settings) {
        frame_with(settings, |ui, page| {
            host.ui("connections", ui, page);
        });
    }

    fn open_page(host: &mut AddonHost) {
        open_page_with(host, &Settings::default());
    }

    fn reads_reach(reads: &AtomicUsize, n: usize) {
        until(&format!("{n} reads"), || reads.load(Ordering::SeqCst) >= n);
    }

    fn pause() {
        std::thread::sleep(std::time::Duration::from_millis(30));
    }

    #[test]
    fn clicking_a_header_sorts_by_it_and_clicking_it_again_reverses_it() {
        let mut p = Prefs::default();
        p.sort_on(SortColumn::Local);
        assert_eq!((p.sort_by, p.sort_descending), (SortColumn::Local, false));
        p.sort_on(SortColumn::Local);
        assert_eq!((p.sort_by, p.sort_descending), (SortColumn::Local, true));
        p.sort_on(SortColumn::State);
        assert_eq!((p.sort_by, p.sort_descending), (SortColumn::State, false));
    }

    #[test]
    fn the_sort_chosen_on_the_page_is_remembered_in_the_settings() {
        let mut conns = Connections::with_reader(|| full(vec![]));
        conns.prefs.sort_on(SortColumn::Remote);
        conns.prefs.sort_on(SortColumn::Remote);
        let saved = conns.settings();
        assert_eq!((saved["sortBy"].as_str(), saved["sortDescending"].as_bool()), (Some("remote"), Some(true)));
        let mut again = Connections::with_reader(|| full(vec![]));
        again.load_settings(&saved);
        assert_eq!((again.prefs().sort_by, again.prefs().sort_descending), (SortColumn::Remote, true));
    }

    #[test]
    fn readings_feed_observed_and_stopping_forgets_it() {
        let mut conns = Connections::with_reader(|| full(sample_rows()));
        let listener = &sample_rows()[0];
        until("observed", || {
            conns.current(1000);
            conns.observed.age(listener, std::time::Instant::now()).is_some()
        });
        conns.stop();
        assert_eq!(conns.observed.age(listener, std::time::Instant::now()), None);
    }

    #[test]
    fn leaving_the_page_keeps_observed_and_only_turning_it_off_forgets_it() {
        let mut conns = Connections::with_reader(|| full(sample_rows()));
        let listener = &sample_rows()[0];
        until("observed", || {
            conns.current(1000);
            conns.observed.age(listener, std::time::Instant::now()).is_some()
        });
        let before = conns.observed.age(listener, std::time::Instant::now()).unwrap();
        conns.page_hidden();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let kept = conns.observed.age(listener, std::time::Instant::now());
        assert!(kept.is_some_and(|age| age > before), "the first sight survives leaving the page: {kept:?}");
        // Back on the page, a new reading keeps counting from the first sight.
        conns.page_shown = true;
        until("a new reading", || {
            let seen = conns.current(1000).is_some();
            seen && conns.observed.age(listener, std::time::Instant::now()).is_some_and(|age| age > before)
        });
        conns.stop();
        assert_eq!(conns.observed.age(listener, std::time::Instant::now()), None, "turned off, it forgets");
    }

    #[test]
    fn its_description_says_what_the_page_finds() {
        assert_eq!(Connections::default().description(), "Find out what ports your computer is exposing to the Internet");
    }

    #[test]
    fn one_id_names_the_add_on_and_its_page() {
        let conns = Connections::with_reader(|| full(vec![]));
        assert_eq!(conns.id(), ID);
        assert_eq!(ID, "connections");
    }

    #[test]
    fn starting_reads_no_connections_until_the_page_opens() {
        let (mut host, reads) = host();
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        open_page(&mut host);
        reads_reach(&reads, 1);
    }

    #[test]
    fn the_inventory_is_read_off_the_drawing_thread() {
        let here = std::thread::current().id();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let conns = Connections::with_reader(move || {
            s.lock().unwrap().push(std::thread::current().id());
            full(vec![])
        });
        let (mut host, _) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(conns))], &json!({}));
        open_page(&mut host);
        until("a read", || !seen.lock().unwrap().is_empty());
        assert!(seen.lock().unwrap().iter().all(|t| *t != here));
    }

    #[test]
    fn the_page_reads_again_at_the_update_speed() {
        let (mut host, reads) = host();
        open_page_with(&mut host, &at_speed(0)); // 4 a second
        reads_reach(&reads, 3);
    }

    #[test]
    fn paused_the_list_stays_until_refresh() {
        let (mut host, reads) = host();
        let paused = at_speed(4);
        assert_eq!(paused.interval_ms(), 0);
        open_page_with(&mut host, &paused);
        reads_reach(&reads, 1);
        pause();
        open_page_with(&mut host, &paused);
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 1, "paused: no new reading");
        assert!(host.refresh("connections"));
        reads_reach(&reads, 2);
    }

    #[test]
    fn turned_off_it_reads_no_connections() {
        let (mut host, reads) = host();
        host.set_enabled("connections", false);
        open_page(&mut host);
        host.refresh("connections");
        open_page(&mut host);
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn turning_it_off_stops_the_worker() {
        let (mut host, reads) = host();
        open_page_with(&mut host, &at_speed(0));
        reads_reach(&reads, 1);
        host.set_enabled("connections", false);
        let after = reads.load(Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert_eq!(reads.load(Ordering::SeqCst), after, "stop joined the worker");
    }

    #[test]
    fn turned_on_again_it_reads_fresh_connections_at_once() {
        let (mut host, reads) = host();
        open_page(&mut host);
        reads_reach(&reads, 1);
        host.set_enabled("connections", false);
        host.set_enabled("connections", true);
        open_page(&mut host);
        reads_reach(&reads, 2);
    }

    #[test]
    fn refresh_reads_the_connections_again() {
        let (mut host, reads) = host();
        open_page(&mut host);
        reads_reach(&reads, 1);
        assert!(host.refresh("connections"));
        reads_reach(&reads, 2);
    }

    #[test]
    fn page_opens_by_its_id_and_the_network_connections_alias() {
        use crate::addons::Landing;
        let (host, _) = host();
        assert_eq!(host.landing(Some("connections"), "summary", "summary"), Landing::Page("connections"));
        assert_eq!(host.landing(Some("network-connections"), "summary", "summary"), Landing::Page("connections"));
    }

    #[test]
    fn settings_add_ons_shows_it_needs_no_admin_network_or_programs() {
        let (host, _) = host();
        let d = crate::addons::overview::Overview::of(&host, Some("connections")).detail.unwrap();
        assert_eq!(d.requirements, ["Needs nothing extra"]);
        assert_eq!(d.note, None);
    }

    #[test]
    fn nothing_chosen_saves_nothing_and_gives_the_defaults() {
        let (host, _) = host();
        assert_eq!(host.settings("connections"), Some(serde_json::Value::Null));
        let prefs = Prefs::default();
        assert_eq!(prefs.scope, Scope::All);
        assert!(prefs.group_by_exposure, "grouped by exposure by default");
        assert_eq!((prefs.sort_by, prefs.sort_descending), (SortColumn::Application, false));
        assert!(prefs.show_system_traffic);
        assert!(!prefs.summary_tile, "the Summary tile is off by default");
    }

    #[test]
    fn settings_survive_turning_it_off_and_on_again() {
        let (mut host, _) = host();
        let chosen = json!({
            "scope": "public",
            "groupByExposure": false,
            "sortBy": "local",
            "sortDescending": true,
            "showSystemTraffic": false,
            "summaryTile": true,
            "fromANewerVersion": 1
        });
        assert!(host.set_settings("connections", chosen.clone()));
        host.set_enabled("connections", false);
        assert_eq!(host.settings("connections"), Some(chosen.clone()), "kept while off");
        host.set_enabled("connections", true);
        assert_eq!(host.settings("connections"), Some(chosen), "and once on again, unknown fields included");
    }

    #[test]
    fn settings_it_cannot_read_give_its_defaults() {
        let mut addon = Connections::with_reader(|| full(vec![]));
        addon.load_settings(&json!({ "scope": "nowhere", "groupByExposure": "maybe" }));
        assert_eq!(addon.prefs(), &Prefs::default());
    }

    #[test]
    fn grouping_by_exposure_can_be_turned_off() {
        let mut addon = Connections::with_reader(|| full(vec![]));
        addon.load_settings(&json!({ "groupByExposure": false }));
        assert!(!addon.prefs().group_by_exposure);
        assert_eq!(addon.settings(), json!({ "groupByExposure": false }));
    }

    fn show_summary(host: &mut AddonHost) {
        frame(|ui, page| host.summary_tiles(ui, page));
        host.end_frame();
    }

    #[test]
    fn public_listeners_are_tcp_listeners_on_all_interfaces() {
        assert_eq!(public_listeners(&sample_rows()), 2, "the UDP wildcard and the loopback listener do not count");
    }

    #[test]
    fn with_the_tile_off_summary_shows_nothing_and_reads_nothing() {
        let (mut host, reads) = host_with(sample_rows(), serde_json::Value::Null);
        show_summary(&mut host);
        show_summary(&mut host);
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn the_summary_tile_counts_public_listeners_and_connections() {
        let (mut host, reads) = host_with(sample_rows(), json!({ "summaryTile": true }));
        assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing read before Summary shows the tile");
        frame(|ui, page| host.summary_tiles(ui, page));
        reads_reach(&reads, 1);
        assert_eq!(summary_lines_of(None), ["Collecting connection inventory."]);
        assert_eq!(summary_lines_of(Some(&full(sample_rows()))), ["2 public listeners", "5 connections"]);
        let one = full(vec![row(Proto::Tcp, "0.0.0.0:135", None, Some(TcpState::Listening))]);
        assert_eq!(summary_lines_of(Some(&one)), ["1 public listener", "1 connection"]);
        let none = assemble([Err(TableError), Err(TableError), Err(TableError), Err(TableError)]);
        assert_eq!(summary_lines_of(Some(&none)), ["Connection inventory is currently unavailable."]);
    }

    #[test]
    fn turned_off_it_shows_no_tile_and_reads_nothing() {
        let (mut host, reads) = host_with(sample_rows(), json!({ "summaryTile": true }));
        host.set_enabled("connections", false);
        show_summary(&mut host);
        open_page(&mut host);
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn hiding_summary_and_the_page_drops_the_inventory_read_for_them() {
        let (mut host, reads) = host_with(sample_rows(), json!({ "summaryTile": true }));
        show_summary(&mut host);
        reads_reach(&reads, 1);
        host.end_frame(); // Summary is no longer drawn
        show_summary(&mut host);
        reads_reach(&reads, 2); // a hidden tile keeps no stale inventory
        open_page(&mut host);
        assert_eq!(reads.load(Ordering::SeqCst), 2, "the page opened at once shows the inventory just read");
        host.end_frame();
        host.end_frame(); // neither the page nor the tile is drawn
        open_page(&mut host);
        reads_reach(&reads, 3); // nor does a closed page
    }

    /// The sample rows (PID 4) and one listener of PID 9000.
    fn rows_of_two_processes() -> Vec<Row> {
        let mut rows = sample_rows();
        rows.push(Row { pid: 9000, ..row(Proto::Tcp, "0.0.0.0:8080", None, Some(TcpState::Listening)) });
        rows
    }

    #[test]
    fn its_processes_column_counts_the_connections_of_each_process() {
        let (mut host, reads) = host_with(rows_of_two_processes(), serde_json::Value::Null);
        let column = crate::addons::ProcessColumn { title: "Connections", menu_item: "Go to connections" };
        assert_eq!(host.process_columns(), [("connections", column)]);
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 0, "nothing read before the column shows");
        let settings = Settings::default();
        let mut figures = None;
        until("a reading", || {
            figures = host.process_figures("connections", &settings);
            figures.is_some()
        });
        assert_eq!(figures, Some(std::collections::HashMap::from([(4, 5), (9000, 1)])));
    }

    #[test]
    fn a_hidden_processes_column_drops_the_inventory_read_for_it() {
        let (mut host, reads) = host_with(rows_of_two_processes(), serde_json::Value::Null);
        // Paused: the worker reads when it starts, then only on Refresh.
        let paused = at_speed(4);
        host.process_figures("connections", &paused);
        reads_reach(&reads, 1);
        host.end_frame();
        host.process_figures("connections", &paused);
        host.end_frame();
        pause();
        assert_eq!(reads.load(Ordering::SeqCst), 1, "a column still shown keeps its reading");
        host.end_frame(); // the column is no longer drawn
        host.process_figures("connections", &paused);
        reads_reach(&reads, 2); // a hidden column keeps no stale inventory
    }

    #[test]
    fn go_to_connections_opens_the_page_on_that_process_only() {
        let (mut host, _) = host_with(rows_of_two_processes(), json!({ "scope": "udp" }));
        assert!(host.reveal_process("connections", 9000));
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let (settings, sample) = (Settings::default(), Snapshot::default());
        let mut texts = Vec::new();
        until("the process's rows", || {
            let out = crate::test_ui::run(&ctx, (1600.0, 900.0), vec![], |ui| {
                host.ui("connections", ui, &mut PageContext::new(&settings, &sample));
            });
            texts = crate::test_ui::texts(&out);
            texts.iter().any(|t| t.contains("8080"))
        });
        assert!(!texts.iter().any(|t| t.contains(":135") || t == "PID 4"), "only PID 9000's rows: {texts:?}");
        assert!(texts.iter().any(|t| t == "PID 9000"), "Find names the process");
        host.set_enabled("connections", false);
        assert!(!host.reveal_process("connections", 9000), "nothing while it is off");
    }
}

