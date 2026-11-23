//! The page's layout in Design A (the prototype approved for #121): the
//! status band, which a job takes over while it runs; the search with the
//! scopes (search results, installed packages, bundles); the list of the
//! chosen scope, and the side panel with the selected entry or the job's
//! packages and output. Only how things are shown lives here: every button
//! sends the same [`Command`] as before.

use egui::{vec2, Align, Layout, RichText, Ui};

use crate::platform::lock_state;
use crate::theme::Theme;
use crate::widgets::parts;
use crate::widgets::{
    display_font, key_values, note, note_text, output_box, row_button, search_field,
    table_line, tick, Col, Tone,
};

use super::batch::{self, Status};
use super::bundle::BundleView;
use super::installed::Listing;
use super::readiness::Readiness;
use super::{busy_line, catalogs_text, search_text, Batch, Catalogs, Check, Column, Command, Install, PackageManager, Search, State, AGREEMENTS_NOTE};

/// The list the page shows under the search.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    #[default]
    Results,
    Installed,
    Bundles,
}

/// The entry whose detail the side panel shows: a search result by
/// (catalog, id), an installed package by id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pick {
    Result(String, String),
    Installed(String),
}

/// The kind of job asked for last: the one the band and the panel follow
/// once it ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Latest {
    #[default]
    None,
    Single,
    Series,
}

/// The page's own state: what it shows, never what it does.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub scope: Scope,
    pub pick: Option<Pick>,
    pub latest: Latest,
    /// The user moved on from the job that ended (picked or ticked an
    /// entry): the band and the panel go back to the page.
    pub moved_on: bool,
    /// Every up-to-date package shows, not only the first [`FOLDED`].
    pub show_all: bool,
}

impl View {
    /// Follows `command`: the scope it concerns comes to the front, and a
    /// job it starts is the one the band and the panel follow.
    pub fn follow(&mut self, command: &Command) {
        match command {
            Command::Search(_) => self.scope = Scope::Results,
            Command::ListInstalled => self.scope = Scope::Installed,
            Command::CreateBundle | Command::OpenBundle(_) => self.scope = Scope::Bundles,
            Command::Install(_) => self.started(Latest::Single),
            Command::InstallAll(_) | Command::InstallBundle | Command::RetryFailed | Command::Update(_) | Command::UpdateAll(_) | Command::ContinueInterrupted => {
                self.started(Latest::Series)
            }
            _ => {}
        }
    }

    fn started(&mut self, latest: Latest) {
        self.latest = latest;
        self.moved_on = false;
    }

    /// The user picked or ticked an entry.
    pub(super) fn picked(&mut self, pick: Pick) {
        self.pick = Some(pick);
        self.moved_on = true;
    }
}

/// How many up-to-date packages show before "Show all".
pub const FOLDED: usize = 8;

/// The width of the side panel.
const PANEL_WIDTH: f32 = 356.0;

/// The columns of the search results: tick, name, id, version, catalog, action.
const RESULT_COLS: [Col; 6] = [Col::Px(16.0), Col::Share(2.0), Col::Share(2.0), Col::Share(1.0), Col::Px(80.0), Col::Px(92.0)];

/// The columns of the installed packages: name, id, version, available, actions.
pub(super) const INSTALLED_COLS: [Col; 5] = [Col::Share(2.0), Col::Share(2.0), Col::Share(1.0), Col::Share(1.0), Col::Px(170.0)];

/// The columns of a bundle: tick, name, catalog, id or link, version, scope.
pub(super) const BUNDLE_COLS: [Col; 6] = [Col::Px(16.0), Col::Share(1.6), Col::Px(80.0), Col::Share(1.6), Col::Px(110.0), Col::Px(120.0)];

/// The job the band and the panel show.
#[derive(Clone, Copy)]
enum Job<'a> {
    Series(&'a Batch),
    Single(&'a Install),
    /// Waiting for the install queue (or another job's busy text).
    Waiting(&'a str),
}

/// The job to show: one running first, then one waiting for the install
/// queue (an uninstall shows in its list), then the last one asked for.
fn job<'a>(st: &'a State, view: &View) -> Option<Job<'a>> {
    let (series, single) = (st.batch.as_ref(), st.install.as_ref());
    if let Some(b) = series.filter(|b| !b.done) {
        return Some(Job::Series(b));
    }
    if let Some(i) = single.filter(|i| i.outcome.is_none()) {
        return Some(Job::Single(i));
    }
    let uninstalling = st.installed.removal.as_ref().is_some_and(|r| r.outcome.is_none());
    if let Some(busy) = st.busy.as_deref().filter(|_| !uninstalling) {
        return Some(Job::Waiting(busy));
    }
    if view.moved_on {
        return None;
    }
    match view.latest {
        Latest::Single => single.map(Job::Single).or(series.map(Job::Series)),
        _ => series.map(Job::Series).or(single.map(Job::Single)),
    }
}

/// A round dot of `color`, `size` across, centred on the line.
fn dot(ui: &mut Ui, color: egui::Color32, size: f32) {
    // As tall as the band's headline, so that the dot sits on its middle.
    let (r, _) = ui.allocate_exact_size(vec2(size + 2.0, 27.0), egui::Sense::hover());
    ui.painter().circle_filled(r.center(), size / 2.0, color);
}

/// The band's headline, in the theme's band title face.
fn headline(ui: &mut Ui, pal: &Theme, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).font(pal.fonts.band_title.clone()).color(pal.text)).wrap());
}

/// Design A's status band (the frame of [`parts::StatusBand`]) holding
/// what the page draws in it: its headline, lines and actions.
fn band<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let t = crate::theme::of_ui(ui);
    egui::Frame::new()
        .fill(t.band)
        .stroke(egui::Stroke::new(1.0, t.line))
        .corner_radius(egui::CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(22, 18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A spinner before the band's headline.
fn busy_headline(ui: &mut Ui, pal: &Theme, text: &str) {
    ui.horizontal_wrapped(|ui| {
        ui.spinner();
        headline(ui, pal, text);
    });
}

/// A line of the band under its headline.
fn band_line(ui: &mut Ui, color: egui::Color32, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(13.0).color(color)).wrap());
}

/// The colour of a package's status in a series.
fn status_color(status: &Status, pal: &Theme) -> egui::Color32 {
    match status {
        Status::Ok => pal.green,
        Status::OkRestart => pal.amber,
        Status::Failed(_) => pal.red,
        Status::Running | Status::Manual(_) => pal.link,
        Status::Pending | Status::Skipped(_) | Status::Cancelled | Status::NotStarted(_) => pal.label,
    }
}

/// A status in words split into its word and its reason ("Failed" and
/// "exit code 1603"); the reason is empty when there is none.
fn status_parts(text: &str) -> (&str, &str) {
    text.split_once(": ").unwrap_or((text, ""))
}

/// The headline of a series that ended: how many of its packages were done,
/// failed and not started.
fn series_headline(b: &Batch) -> String {
    let count = |pick: fn(&Status) -> bool| b.items.iter().filter(|i| pick(&i.status)).count();
    let done = count(|s| matches!(s, Status::Ok | Status::OkRestart));
    let mut text = format!("{done} of {} {}", b.items.len(), b.kind.done());
    let failed = count(|s| matches!(s, Status::Failed(_)));
    let not_started = count(|s| matches!(s, Status::NotStarted(_)));
    if failed > 0 {
        text.push_str(&format!(" \u{b7} {failed} failed"));
    }
    if not_started > 0 {
        text.push_str(&format!(" \u{b7} {not_started} not started"));
    }
    text
}

impl PackageManager {
    /// The whole page: the offer to continue an interrupted run, the band,
    /// the install queue held by another add-on, then (once winget is
    /// ready) the search, the scopes, the list and the side panel.
    pub(super) fn draw_page(&mut self, ui: &mut Ui, st: &State) {
        let ready = self.context.is_some() && matches!(st.check, Check::Known(Readiness::Ready(_)));
        let job = job(st, &self.view);
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            self.draw_interrupted(ui, st);
            self.draw_band(ui, st, job);
            self.draw_queue(ui);
            if ready {
                if self.prefs.accept_agreements && !self.prefs.agreements_noted {
                    note(ui, Tone::Neutral, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.label(RichText::new(AGREEMENTS_NOTE).size(13.0));
                            ui.label(RichText::new("(shown until the first install)").size(13.0).color(crate::theme::of_ui(ui).label));
                        });
                    });
                }
                self.draw_toolbar(ui, st);
            } else {
                note_text(ui, Tone::Neutral, "Search, bundles and the installed list appear when winget is ready.");
            }
            ui.add_space(2.0);
        });
        if !ready {
            // A job that outlives the add-on still shows what it does.
            if let Some(job) = job {
                egui::ScrollArea::vertical().id_salt("package-manager-job").show(ui, |ui| {
                    parts::panel(ui, |ui| self.draw_job_panel(ui, st, job));
                });
            }
            return;
        }
        // The list on the left, the side panel on the right, each scrolling.
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        let side_w = PANEL_WIDTH.min(area.width() * 0.4);
        let list_rect = egui::Rect::from_min_max(area.min, egui::pos2(area.right() - side_w - 16.0, area.bottom()));
        let side_rect = egui::Rect::from_min_max(egui::pos2(area.right() - side_w, area.top()), area.max);
        let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side_rect));
        egui::ScrollArea::vertical().id_salt("package-manager-panel-scroll").show(&mut side_ui, |ui| {
            parts::panel(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                match job {
                    Some(job) => self.draw_job_panel(ui, st, job),
                    None => self.draw_pick_panel(ui, st),
                }
            });
        });
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        egui::ScrollArea::vertical().id_salt("package-manager-list").auto_shrink([false, false]).show(&mut list_ui, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            match self.view.scope {
                Scope::Results => self.draw_results(ui, st),
                Scope::Installed => self.draw_installed(ui, st),
                Scope::Bundles => self.draw_bundle(ui, st),
            }
        });
    }

    /// The status band: what is true now. A job running (or the last one,
    /// until the user moves on) takes it; otherwise winget's readiness, its
    /// repair, the catalog update and "Check for updates".
    fn draw_band(&mut self, ui: &mut Ui, st: &State, job: Option<Job>) {
        let pal = crate::theme::of_ui(ui);
        let mut asked = Vec::new();
        band(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            match job {
                Some(job) => self.job_band(ui, &pal, st, job, &mut asked),
                None => self.readiness_band(ui, &pal, st, &mut asked),
            }
        });
        self.ask(ui, asked);
    }

    fn readiness_band(&mut self, ui: &mut Ui, pal: &Theme, st: &State, asked: &mut Vec<Command>) {
        match &st.check {
            Check::NotChecked | Check::Checking => {
                busy_headline(ui, pal, "Checking winget\u{2026}");
                band_line(ui, pal.soft, "The check runs when this page opens, never when the app starts.");
            }
            Check::Repairing => {
                busy_headline(ui, pal, "Registering App Installer\u{2026}");
                band_line(ui, pal.soft, "Runs Add-AppxPackage for your account through PowerShell. This takes a few seconds.");
            }
            Check::Known(r) => {
                let ready = matches!(r, Readiness::Ready(_));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    dot(ui, if ready { pal.green } else { pal.amber }, 10.0);
                    headline(ui, pal, &r.headline());
                    if ready {
                        ui.label(RichText::new(r.detail()).size(12.0).color(pal.label));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some((_, label)) = r.repair() {
                            if crate::widgets::parts::outline_button(ui, "Check again", true).clicked() {
                                self.begin_check();
                            }
                            if crate::widgets::parts::primary_button(ui, label, true).clicked() {
                                self.repair();
                            }
                        } else {
                            let reading = st.installed.listing == Listing::Reading;
                            if crate::widgets::parts::outline_button(ui, "Check for updates", !reading).clicked() {
                                asked.push(Command::ListInstalled);
                            }
                        }
                    });
                });
                if !ready {
                    band_line(ui, pal.soft, &r.detail());
                }
            }
        }
        if let Some(text) = catalogs_text(&st.catalogs) {
            match &st.catalogs {
                Catalogs::Updating => busy_line(ui, &text),
                Catalogs::Failed(_) => band_line(ui, pal.red, &text),
                _ => band_line(ui, pal.soft, &text),
            }
        }
        if let Some(e) = &st.repair_error {
            band_line(ui, pal.red, e);
        }
    }

    fn job_band(&mut self, ui: &mut Ui, pal: &Theme, st: &State, job: Job, asked: &mut Vec<Command>) {
        match job {
            Job::Series(b) if !b.done => {
                let running = b.items.iter().position(|i| i.status == Status::Running);
                let title = match running {
                    Some(i) => format!("{} {} of {}: {}", b.kind.doing(), i + 1, b.items.len(), b.items[i].package.name),
                    None => st.busy.clone().unwrap_or_else(|| format!("{} {}\u{2026}", b.kind.doing(), crate::addons::plural(b.items.len(), "package"))),
                };
                let done = b.items.iter().filter(|i| i.status.finished()).count();
                let waiting = b.items.iter().filter(|i| i.status == Status::Pending).count();
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.spinner();
                    headline(ui, pal, &title);
                    ui.label(RichText::new(format!("{done} done \u{b7} {waiting} waiting")).size(12.0).color(pal.label));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if b.stopping {
                            busy_line(ui, "Stopping after the current package\u{2026}");
                        } else if ui.add(egui::Button::new(RichText::new("Stop after this one").size(13.0)).min_size(vec2(0.0, 34.0))).clicked() {
                            asked.push(Command::StopAfterCurrent);
                        }
                    });
                });
                if self.context.is_none() {
                    note_text(ui, Tone::Warm, "Package Manager turns off when the current package ends. The rest will not start.");
                }
            }
            Job::Series(b) => {
                let retry = !batch::failed(&b.items).is_empty();
                let clean = b.items.iter().all(|i| matches!(i.status, Status::Ok | Status::OkRestart | Status::Skipped(_) | Status::Manual(_)));
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    dot(ui, if clean { pal.green } else { pal.amber }, 10.0);
                    headline(ui, pal, &series_headline(b));
                    if retry {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if crate::widgets::parts::primary_button(ui, "Retry the failed ones", st.busy.is_none()).clicked() {
                                asked.push(Command::RetryFailed);
                            }
                        });
                    }
                });
                if retry {
                    band_line(ui, pal.soft, "Retry runs the failed and the not-started packages again, in the same order.");
                }
            }
            Job::Single(install) => match &install.outcome {
                None => {
                    busy_headline(ui, pal, st.busy.as_deref().unwrap_or("Installing\u{2026}"));
                    band_line(ui, pal.soft, "Install buttons wait until this one ends.");
                }
                Some(outcome) => {
                    let color = match outcome.status() {
                        Status::Ok => pal.green,
                        Status::OkRestart | Status::Skipped(_) | Status::Cancelled => pal.amber,
                        _ => pal.red,
                    };
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        dot(ui, color, 10.0);
                        ui.add(egui::Label::new(RichText::new(outcome.text(&install.package.name)).size(18.0).strong().color(pal.text)).wrap());
                    });
                }
            },
            Job::Waiting(text) => busy_headline(ui, pal, text),
        }
    }

    /// The search box (Enter searches) and the scopes with their counts.
    fn draw_toolbar(&mut self, ui: &mut Ui, st: &State) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let width = (ui.available_width() - 470.0).clamp(240.0, 560.0);
            let edit = search_field(ui, &mut self.query, "Search the catalogs and press Enter", width);
            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                lock_state(&self.commands).push(Command::Search(self.query.clone()));
                ui.ctx().request_repaint();
            }
            let results = match &st.search {
                Search::Found { packages, .. } => packages.len(),
                _ => 0,
            };
            // The updates waiting, in amber, else how many are installed.
            let updates = st.installed.updates().len();
            let installed = match &st.installed.listing {
                Listing::Read { .. } if updates > 0 => updates,
                Listing::Read { packages, .. } => packages.len(),
                _ => 0,
            };
            let bundle = usize::from(matches!(&st.bundle, Some(BundleView::Draft { .. } | BundleView::Opened { .. })));
            let items = [(Scope::Results, "Search results", results), (Scope::Installed, "Installed packages", installed), (Scope::Bundles, "Bundles", bundle)];
            let mut group = parts::ScopeGroup::new(self.view.scope, &items);
            if updates > 0 {
                group = group.alert(Scope::Installed);
            }
            if let Some(scope) = group.show(ui).clicked {
                self.view.scope = scope;
            }
        });
    }

    /// The search results: the tick of each (for "Install selected"), its
    /// name, id, version and catalog, and its Install button or its
    /// "Installed" mark. A heading sorts.
    fn draw_results(&mut self, ui: &mut Ui, st: &State) {
        let pal = crate::theme::of_ui(ui);
        let text = search_text(&st.search);
        let packages = match &st.search {
            Search::NotSearched => return note_text(ui, Tone::Neutral, "Type a name and press Enter. Nothing is searched until you do."),
            Search::Searching(_) => return busy_line(ui, &text.unwrap_or_default()),
            Search::Failed(_) => return note_text(ui, Tone::Bad, &text.unwrap_or_default()),
            Search::Found { packages, .. } if packages.is_empty() => return note_text(ui, Tone::Neutral, &text.unwrap_or_default()),
            Search::Found { packages, .. } => packages,
        };
        let installing = st.busy.is_some();
        // Only results still shown stay ticked.
        self.selected.retain(|(c, id)| packages.iter().any(|p| !p.installed && &p.catalog == c && &p.id == id));
        let ticked: Vec<_> = packages.iter().filter(|p| self.selected.iter().any(|(c, id)| &p.catalog == c && &p.id == id)).cloned().collect();
        let mut asked = Vec::new();
        list_bar(ui, &pal, |ui| {
            ui.label(RichText::new("Tick several packages to install them one after another. Packages that need administrator go first.").size(12.0).color(pal.label));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if crate::widgets::parts::primary_button(ui, &format!("Install selected ({})", ticked.len()), !installing && !ticked.is_empty()).clicked() {
                    asked.push(Command::InstallAll(ticked.clone()));
                    self.selected.clear();
                }
            });
        });
        table_line(ui, &RESULT_COLS, 26.0, true, false, |i, ui| {
            let column = match i {
                1 => Column::Name,
                2 => Column::Id,
                3 => Column::Version,
                4 => Column::Catalog,
                _ => return,
            };
            let sorted = self.prefs.sort == Some(column);
            let color = if sorted { pal.text } else { pal.label };
            // Room after the words for the sort mark.
            let words = if sorted { format!("{}    ", column.heading()) } else { column.heading().to_string() };
            let resp = ui.add(egui::Button::new(RichText::new(words).size(12.0).color(color)).frame(false));
            if sorted {
                let at = egui::pos2(resp.rect.right() - 9.0, resp.rect.center().y);
                crate::widgets::parts::sort_mark(ui.painter(), at, self.prefs.sort_desc);
            }
            if resp.clicked() {
                asked.push(Command::Sort(column));
            }
        });
        let mut picked = None;
        for p in packages {
            let key = (p.catalog.clone(), p.id.clone());
            let selected = self.view.pick == Some(Pick::Result(key.0.clone(), key.1.clone()));
            let row = table_line(ui, &RESULT_COLS, 32.0, false, selected, |i, ui| match i {
                0 => {
                    let mut tick_on = self.selected.contains(&key);
                    if ui.add_enabled(!p.installed, egui::Checkbox::without_text(&mut tick_on)).changed() {
                        if tick_on {
                            self.selected.push(key.clone());
                        } else {
                            self.selected.retain(|k| *k != key);
                        }
                        picked = Some(Pick::Result(key.0.clone(), key.1.clone()));
                    }
                }
                1 => cell_text(ui, &p.name, pal.text, false),
                2 => cell_text(ui, &p.id, pal.label, true),
                3 => cell_text(ui, &p.version, pal.text, false),
                4 => cell_text(ui, &p.catalog, pal.label, false),
                _ => {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if p.installed {
                            ui.label(RichText::new("Installed").size(12.0).color(pal.green));
                            let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), egui::Sense::hover());
                            tick(ui.painter(), r.center());
                        } else if row_button(ui, "Install", !installing).clicked() {
                            asked.push(Command::Install(p.clone()));
                        }
                    });
                }
            });
            if row.clicked() {
                picked = Some(Pick::Result(key.0.clone(), key.1.clone()));
            }
        }
        if let Some(pick) = picked {
            self.view.picked(pick);
        }
        if let Some(note) = text {
            ui.add_space(4.0);
            ui.add(egui::Label::new(RichText::new(note).size(12.0).color(pal.label)).wrap());
        }
        self.ask(ui, asked);
    }

    /// The panel while a job runs (or ended): a series' packages with their
    /// status, the output of the one running and the summary; or the single
    /// install, its output and its log.
    fn draw_job_panel(&mut self, ui: &mut Ui, st: &State, job: Job) {
        let pal = crate::theme::of_ui(ui);
        let mut asked = Vec::new();
        match job {
            Job::Series(b) => {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if b.done { "This run" } else { "This series" }).size(14.0).strong().color(pal.text));
                    ui.label(RichText::new(crate::addons::plural(b.items.len(), "package")).size(14.0).color(pal.label));
                });
                for (i, item) in b.items.iter().enumerate() {
                    let status = item.status.text(b.kind);
                    let (word, why) = status_parts(&status);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(16.0, 18.0), egui::Sense::hover());
                        if item.admin {
                            crate::widgets::parts::shield(ui.painter(), r.center(), crate::widgets::icons::Ink::Theme);
                        }
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.horizontal(|ui| {
                                ui.add(egui::Label::new(RichText::new(&item.package.name).color(pal.text)).truncate());
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if b.done && item.log.is_some() && matches!(item.status, Status::Failed(_) | Status::Cancelled) && ui.link(RichText::new("Open log").size(12.0)).clicked() {
                                        asked.push(Command::OpenItemLog(i));
                                    }
                                    if matches!(item.status, Status::Manual(_)) && ui.link(RichText::new("Open the site").size(12.0)).clicked() {
                                        asked.push(Command::OpenItemSite(i));
                                    }
                                    ui.label(RichText::new(word).size(12.0).color(status_color(&item.status, &pal)));
                                    if item.status == Status::Running {
                                        ui.spinner();
                                    }
                                });
                            });
                            if !why.is_empty() {
                                ui.add(egui::Label::new(RichText::new(why).size(12.0).color(pal.label)).wrap());
                            }
                            if item.admin {
                                ui.label(RichText::new("needs administrator").size(12.0).color(pal.label));
                            }
                        });
                    });
                    crate::widgets::separator(ui);
                }
                if !b.done {
                    let lines = super::primary_source().last_lines(&b.lines.join("\n"), 5);
                    if !lines.is_empty() {
                        let name = b.items.iter().find(|i| i.status == Status::Running).map(|i| i.package.name.as_str()).unwrap_or("the package");
                        ui.label(RichText::new(format!("Last lines from {name}")).size(12.0).color(pal.label));
                        output_box(ui, &lines);
                    }
                } else {
                    for line in b.summary() {
                        ui.add(egui::Label::new(RichText::new(line).color(pal.text)).wrap());
                    }
                    crate::widgets::separator(ui);
                    if ui.link(RichText::new("Open the log folder").size(12.5)).clicked() {
                        asked.push(Command::OpenLogs);
                    }
                }
            }
            Job::Single(install) => {
                ui.add(egui::Label::new(RichText::new(&install.package.name).font(display_font(ui, 20.0)).color(pal.text)).wrap());
                ui.label(RichText::new(format!("{} \u{b7} {}", install.package.id, install.package.catalog)).size(12.0).color(pal.label));
                let lines = install.last_lines();
                if !lines.is_empty() {
                    ui.label(RichText::new("Last lines").size(12.0).color(pal.label));
                    output_box(ui, &lines);
                }
                if install.outcome.is_some() {
                    crate::widgets::separator(ui);
                    if ui.link(RichText::new("Open log").size(12.5)).clicked() {
                        asked.push(Command::OpenLog);
                    }
                }
            }
            Job::Waiting(_) => self.draw_pick_panel(ui, st),
        }
        self.ask(ui, asked);
    }

    /// The panel of the scope: the ticked results, the selected result or
    /// installed package, or the bundle shown.
    fn draw_pick_panel(&mut self, ui: &mut Ui, st: &State) {
        let pal = crate::theme::of_ui(ui);
        let idle = st.busy.is_none();
        let mut asked = Vec::new();
        let placeholder = |ui: &mut Ui| {
            ui.label(RichText::new("Select a single entry for detail information.").color(pal.label));
        };
        match self.view.scope {
            Scope::Results => {
                let Search::Found { packages, .. } = &st.search else { return placeholder(ui) };
                let ticked: Vec<_> = packages.iter().filter(|p| self.selected.iter().any(|(c, id)| &p.catalog == c && &p.id == id)).cloned().collect();
                if ticked.len() > 1 {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(ticked.len().to_string()).font(display_font(ui, 20.0)).color(pal.text));
                        ui.label(RichText::new("packages ticked").size(14.0).strong().color(pal.text));
                    });
                    for p in &ticked {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(format!("\u{2022} {}", p.name)).color(pal.soft));
                            ui.label(RichText::new(&p.version).color(pal.label));
                        });
                    }
                    ui.add(egui::Label::new(RichText::new("They install one after another. You can stop after any one; the rest are listed as not started.").size(12.0).color(pal.label)).wrap());
                    crate::widgets::separator(ui);
                    ui.horizontal_wrapped(|ui| {
                        if crate::widgets::parts::primary_button(ui, &format!("Install selected ({})", ticked.len()), idle).clicked() {
                            asked.push(Command::InstallAll(ticked.clone()));
                            self.selected.clear();
                        }
                        if crate::widgets::parts::outline_button(ui, "Clear ticks", true).clicked() {
                            self.selected.clear();
                        }
                    });
                } else {
                    let found = match &self.view.pick {
                        Some(Pick::Result(c, id)) => packages.iter().find(|p| &p.catalog == c && &p.id == id),
                        _ => None,
                    };
                    let Some(p) = found else { return placeholder(ui) };
                    ui.add(egui::Label::new(RichText::new(&p.name).font(display_font(ui, 20.0)).color(pal.text)).wrap());
                    ui.label(RichText::new(format!("from the {} catalog", p.catalog)).color(pal.label));
                    ui.label(RichText::new("Version").size(12.0).color(pal.label));
                    ui.label(RichText::new(&p.version).font(display_font(ui, 20.0)).color(pal.text));
                    let on_pc = if p.installed { RichText::new("Installed").color(pal.green) } else { RichText::new("Not installed").color(pal.text) };
                    key_values(ui, "package-manager-result", vec![("Id", RichText::new(&p.id).monospace()), ("Catalog", RichText::new(&p.catalog)), ("On this PC", on_pc)]);
                    crate::widgets::separator(ui);
                    if p.installed {
                        ui.label(RichText::new("Already installed").color(pal.green));
                    } else if crate::widgets::parts::primary_button(ui, &format!("Install {}", p.name), idle).clicked() {
                        asked.push(Command::Install(p.clone()));
                    }
                }
            }
            Scope::Installed => {
                let Listing::Read { packages, .. } = &st.installed.listing else { return placeholder(ui) };
                let found = match &self.view.pick {
                    Some(Pick::Installed(id)) => packages.iter().find(|p| &p.id == id),
                    _ => None,
                };
                let Some(p) = found else { return placeholder(ui) };
                ui.add(egui::Label::new(RichText::new(&p.name).font(display_font(ui, 20.0)).color(pal.text)).wrap());
                ui.label(RichText::new(&p.id).monospace().color(pal.label));
                egui::Grid::new("package-manager-installed-versions").num_columns(2).spacing(vec2(18.0, 4.0)).show(ui, |ui| {
                    ui.label(RichText::new("Installed").size(12.0).color(pal.label));
                    ui.label(RichText::new(if p.has_update() { "Available" } else { "" }).size(12.0).color(pal.amber));
                    ui.end_row();
                    ui.label(RichText::new(&p.version).font(display_font(ui, 18.0)).color(pal.text));
                    ui.label(RichText::new(if p.has_update() { p.available.as_str() } else { "" }).font(display_font(ui, 18.0)).color(pal.amber));
                    ui.end_row();
                });
                crate::widgets::separator(ui);
                ui.horizontal_wrapped(|ui| {
                    if p.has_update() && crate::widgets::parts::primary_button(ui, &format!("Update to {}", p.available), idle).clicked() {
                        asked.push(Command::Update(p.update()));
                    }
                    if crate::widgets::parts::destructive_button(ui, "Uninstall", idle).clicked() {
                        asked.push(Command::Uninstall(p.clone()));
                    }
                });
            }
            Scope::Bundles => {
                let Some(BundleView::Draft { rows, .. } | BundleView::Opened { rows, .. }) = &st.bundle else { return placeholder(ui) };
                let ticked = rows.iter().filter(|r| r.ticked).count();
                let manual = rows.iter().filter(|r| matches!(r.entry, super::bundle::Entry::Manual { .. })).count();
                let problems = rows.iter().filter(|r| r.entry.problem(&st.inactive_catalogs).is_some()).count();
                ui.label(RichText::new("About this bundle").size(14.0).strong().color(pal.text));
                let mut lines = vec![
                    ("Entries", RichText::new(format!("{} \u{b7} {ticked} ticked", rows.len()))),
                    ("By hand", RichText::new(format!("{manual} with a link"))),
                ];
                if problems > 0 {
                    lines.push(("Problems", RichText::new(format!("{problems} cannot install here")).color(pal.amber)));
                }
                lines.push(("Folder", RichText::new(self.bundle_folder().display().to_string()).size(12.0)));
                key_values(ui, "package-manager-bundle-about", lines);
                ui.add(egui::Label::new(RichText::new("Version empty means the latest. The default scope lets the installer decide.").size(12.0).color(pal.label)).wrap());
            }
        }
        self.ask(ui, asked);
    }

    /// Sends what the user asked for this frame.
    pub(super) fn ask(&mut self, ui: &Ui, asked: Vec<Command>) {
        if !asked.is_empty() {
            lock_state(&self.commands).extend(asked);
            ui.ctx().request_repaint();
        }
    }
}

/// A cell's text in `color`, cut with "…" to the column (monospaced for
/// ids).
pub(super) fn cell_text(ui: &mut Ui, text: &str, color: egui::Color32, mono: bool) {
    let mut t = RichText::new(text).color(color);
    if mono {
        t = t.monospace().size(12.0);
    }
    ui.add(egui::Label::new(t).truncate());
}

/// The bar above a list: a line of words and the list's main action.
pub(super) fn list_bar(ui: &mut Ui, pal: &Theme, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new().fill(pal.panel).stroke(egui::Stroke::new(1.0, pal.line)).corner_radius(egui::CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(add);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_reads_as_its_word_and_its_reason() {
        assert_eq!(status_parts("Failed: exit code 1603"), ("Failed", "exit code 1603"));
        assert_eq!(status_parts("To install by hand: https://example.com/a"), ("To install by hand", "https://example.com/a"));
        assert_eq!(status_parts("Installed"), ("Installed", ""));
    }

    #[test]
    fn the_scope_follows_the_command_and_a_new_job_takes_the_band_again() {
        let mut view = View { moved_on: true, ..View::default() };
        view.follow(&Command::ListInstalled);
        assert_eq!(view.scope, Scope::Installed);
        view.follow(&Command::CreateBundle);
        assert_eq!(view.scope, Scope::Bundles);
        view.follow(&Command::Search("vlc".into()));
        assert_eq!(view.scope, Scope::Results);
        view.follow(&Command::RetryFailed);
        assert_eq!((view.latest, view.moved_on), (Latest::Series, false));
        view.follow(&Command::Install(Default::default()));
        assert_eq!(view.latest, Latest::Single);
    }
}
