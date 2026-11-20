//! The Installed Apps page, design A ("updates first", see the Installed
//! Apps page redesign canvas): a status band that answers "are my apps up
//! to date?" and splits the updates by use (apps opened in the last six
//! months, tools and parts of other programs, apps not opened for six
//! months, where uninstalling may beat updating), the list, and a side
//! panel with installed and offered versions side by side. Where the disk
//! space goes belongs to the Disk Space page.

use std::collections::BTreeSet;

use egui::{vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Ui};

use super::apps::{self, App, Hub, Source};
use crate::page_perf::{display, fit};
use crate::platform::{csv_field, AddonContext};
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, BandArea, Mark, StatusBand};
use crate::widgets::{long_date, separator, short_time, split_version, tick};

/// The group of apps listed: the Updates, All apps and Store apps
/// buttons (saved as "group").
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ListGroup {
    Updates,
    All,
    Store,
}

/// The column the list is sorted by (saved as "sort").
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Sort {
    #[default]
    Name,
    Version,
    Size,
    LastUsed,
}

impl Sort {
    /// The direction a heading starts in when first clicked: names and
    /// versions A to Z, size and last opened biggest and newest first.
    fn starts_descending(self) -> bool {
        matches!(self, Sort::Size | Sort::LastUsed)
    }
}

/// The column the list is sorted by and which way it runs. Saved as
/// "sort" (the column) and "sortDesc", which is left out while the column
/// runs its own way, so settings saved before the direction was load with
/// that column's own direction.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Deserialize, serde::Serialize)]
#[serde(from = "SavedSort", into = "SavedSort")]
pub struct SortOrder {
    pub column: Sort,
    /// Biggest, newest or Z first.
    pub desc: bool,
}

/// [`SortOrder`] as saved.
#[derive(Clone, Copy, Default, serde::Deserialize, serde::Serialize)]
#[serde(default)]
struct SavedSort {
    #[serde(rename = "sort", skip_serializing_if = "crate::addons::is_default")]
    column: Sort,
    #[serde(rename = "sortDesc", skip_serializing_if = "Option::is_none")]
    desc: Option<bool>,
}

impl From<SavedSort> for SortOrder {
    fn from(s: SavedSort) -> Self {
        SortOrder { column: s.column, desc: s.desc.unwrap_or(s.column.starts_descending()) }
    }
}

impl From<SortOrder> for SavedSort {
    fn from(s: SortOrder) -> Self {
        SavedSort { column: s.column, desc: (s.desc != s.column.starts_descending()).then_some(s.desc) }
    }
}

/// Compares two versions by their numeric parts, so "3.10.0" comes after
/// "3.9.2". Versions that do not start with a number come after those that
/// do, and a missing version comes after both.
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let parts = |v: &str| -> Vec<String> { v.split(|c: char| !c.is_alphanumeric()).filter(|p| !p.is_empty()).map(str::to_lowercase).collect() };
    let rank = |p: &[String]| match p.first() {
        None => 2,
        Some(f) if f.chars().all(|c| c.is_ascii_digit()) => 0,
        Some(_) => 1,
    };
    let (pa, pb) = (parts(a), parts(b));
    rank(&pa).cmp(&rank(&pb)).then_with(|| {
        for (x, y) in pa.iter().zip(&pb) {
            let o = match (x.parse::<u128>(), y.parse::<u128>()) {
                (Ok(n), Ok(m)) => n.cmp(&m),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => x.cmp(y),
            };
            if o != Ordering::Equal {
                return o;
            }
        }
        pa.len().cmp(&pb.len())
    })
}

/// Orders the listed apps by a column. A missing version stays at the
/// bottom whichever way the Version column runs.
pub fn sort_shown(apps: &mut [&App], sort: Sort, desc: bool) {
    apps.sort_by(|a, b| {
        if sort == Sort::Version {
            let (ea, eb) = (a.version.trim().is_empty(), b.version.trim().is_empty());
            if ea || eb {
                return ea.cmp(&eb);
            }
        }
        let o = match sort {
            Sort::Name => display_name(&a.name, &a.version).to_lowercase().cmp(&display_name(&b.name, &b.version).to_lowercase()),
            Sort::Version => compare_versions(&a.version, &b.version),
            Sort::Size => a.size().cmp(&b.size()),
            Sort::LastUsed => a.last_used.cmp(&b.last_used),
        };
        if desc { o.reverse() } else { o }
    });
}

/// The three kinds of update the band and the list tell apart.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    /// Opened in the last six months.
    Recent,
    /// Not opened for over six months: uninstalling may beat updating.
    Unused,
    /// Never seen opened from Start: tools and parts of other programs.
    Tools,
}

/// What the page remembers between frames.
#[derive(Default)]
pub struct AppsUi {
    /// None: Updates when there are updates, else All apps.
    pub list_group: Option<ListGroup>,
    pub sort: SortOrder,
    pub search: String,
    /// Ticked apps, by key.
    pub checked: BTreeSet<String>,
    pub selected: Option<String>,
    pub confirm_uninstall: Option<Vec<String>>,
    pub confirm_update: Option<Vec<String>>,
    pub show_all_recent: bool,
    pub show_tools: bool,
    pub scroll_to_group: Option<Group>,
    /// Height of the tallest card in the status band, so all match.
    pub card_h: f32,
    pub started: bool,
}

impl AppsUi {
    /// Sorts by a column heading: a new column starts in its natural
    /// direction, the active one flips.
    pub fn click_heading(&mut self, column: Sort) {
        if self.sort.column == column {
            self.sort.desc = !self.sort.desc;
        } else {
            self.sort = SortOrder { column, desc: column.starts_descending() };
        }
    }

    /// Selects app `key`'s row and clears what would hide it (search,
    /// list group, the tools filter).
    pub fn reveal(&mut self, key: String) {
        self.selected = Some(key);
        self.search.clear();
        self.list_group = Some(ListGroup::All);
        self.show_tools = true;
    }
}


/// Apps not opened for this long count as unused.
const UNUSED_DAYS: i64 = 180;
/// Rows of the "recent" group shown before "Show N more".
const RECENT_ROWS: usize = 5;

fn text(size: f32) -> FontId {
    FontId::proportional(size)
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn fmt_size(b: u64) -> String {
    if b == 0 {
        return String::new();
    }
    let mut v = b as f64;
    let mut unit = "B";
    for u in ["KB", "MB", "GB", "TB"] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    if unit == "B" { format!("{v:.0} B") } else { format!("{v:.1} {unit}") }
}

/// How strongly to shade a Size cell, 0.0 to 1.0: the app's share of the
/// largest app shown. No size, no shade.
pub fn size_shade(size: u64, largest: u64) -> f32 {
    if size == 0 || largest == 0 {
        return 0.0;
    }
    (size as f64 / largest as f64).min(1.0) as f32
}

/// "Today", "Yesterday", "12 days ago", "5 months ago", "2 years ago".
pub fn ago(secs: i64, now: i64) -> String {
    let days = (now - secs).max(0) / 86_400;
    match days {
        0 => "Today".into(),
        1 => "Yesterday".into(),
        2..=44 => format!("{days} days ago"),
        45..=729 => format!("{} months ago", (days + 15) / 30),
        _ => format!("{} years ago", days / 365),
    }
}

fn unused(a: &App, now: i64) -> bool {
    a.last_used.is_some_and(|t| now - t > UNUSED_DAYS * 86_400)
}

pub fn group_of(a: &App, now: i64) -> Group {
    match a.last_used {
        None => Group::Tools,
        Some(_) if unused(a, now) => Group::Unused,
        Some(_) => Group::Recent,
    }
}

/// The name without the version many installers append to it:
/// "Audacity 3.7.8" → "Audacity", "LocalSend versione 1.17.0" → "LocalSend".
pub fn display_name(name: &str, version: &str) -> String {
    let n = name.trim();
    let v = version.trim();
    if !v.is_empty() && n.len() > v.len() + 1 {
        for suffix in [format!(" {v}"), format!(" v{v}"), format!(" - {v}")] {
            if let Some(s) = n.strip_suffix(suffix.as_str()) {
                let s = s.trim_end();
                let s = s.strip_suffix(" versione").or_else(|| s.strip_suffix(" version")).unwrap_or(s);
                return s.trim_end().to_string();
            }
        }
    }
    n.to_string()
}

/// The name for a sentence: no version and no "(Python 3.13.5 64-bit)".
fn short_name(name: &str, version: &str) -> String {
    let n = display_name(name, version);
    match n.find(" (") {
        Some(i) if i > 0 => n[..i].to_string(),
        _ => n,
    }
}

/// "4.0.2.263060742" → "4.0.2" for a button; short versions stay whole.
fn short_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() > 3 && parts[3].len() > 4 {
        parts[..3].join(".")
    } else {
        v.to_string()
    }
}

/// "A, B, C and 4 more".
fn name_list(names: &[String], show: usize) -> String {
    match names.len() {
        0 => String::new(),
        n if n <= show => match n {
            1 => names[0].clone(),
            _ => format!("{} and {}", names[..n - 1].join(", "), names[n - 1]),
        },
        n => format!("{} and {} more", names[..show].join(", "), n - show),
    }
}

/// What the app list says besides its rows: a title and a detail line.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListNote {
    /// Above the rows, which are from the last read.
    Above(&'static str, &'static str),
    /// In place of the rows: there are none to show.
    Instead(&'static str, Option<&'static str>),
}

const SCANNING: (&str, &str) = ("Scanning", "Reading the machine and current-user application inventory.");

/// What the list says while `shown` apps are listed. `total` counts every
/// app Windows reported, before the group and the search.
fn list_note(listing: bool, shown: usize, total: usize, search: &str, group: ListGroup) -> Option<ListNote> {
    let note = if listing {
        if shown > 0 {
            ListNote::Above(SCANNING.0, SCANNING.1)
        } else {
            ListNote::Instead(SCANNING.0, Some(SCANNING.1))
        }
    } else if shown > 0 {
        return None;
    } else if total == 0 {
        ListNote::Instead("No installed applications were reported", Some("Windows did not publish any visible uninstall entries."))
    } else if !search.trim().is_empty() {
        ListNote::Instead("No app or publisher matches what you typed.", None)
    } else if group == ListGroup::Updates {
        ListNote::Instead("No app has a newer version.", None)
    } else {
        ListNote::Instead("No apps here.", None)
    };
    Some(note)
}

fn kind_words(a: &App) -> &'static str {
    match a.source {
        Source::Msix => "Microsoft Store app",
        Source::Msi => "Windows Installer (MSI)",
        Source::Registry => "Classic installer",
    }
}

fn details_text(a: &App) -> String {
    format!(
        "{}\nPublisher: {}\nVersion: {}{}\nInstalled: {}\nLast opened: {}\nSize: {}\nScope: {}\nSource: {}{}\nFolder: {}\nwinget id: {}\nUninstall: {}",
        a.name,
        a.publisher,
        a.version,
        if a.has_update() { format!(" ({} on winget)", a.available) } else { String::new() },
        a.installed,
        a.last_used.map(apps::ymd).unwrap_or_else(|| "not seen".into()),
        fmt_size(a.size()),
        a.scope_text(),
        a.source.label(),
        // A Store app has no registry key: no line for it.
        match a.registry_key() {
            "" => String::new(),
            key => format!("\nRegistry key: {key}"),
        },
        a.folder().unwrap_or_default(),
        a.winget_id,
        a.uninstall_text()
    )
}

/// The page while the add-on is on: built by `start`, dropped by `stop`.
pub struct AppsPage {
    pub apps: Hub,
    pub apps_ui: AppsUi,
    context: AddonContext,
    /// Status-bar messages for the app to show after the frame.
    pub said: Vec<String>,
    /// Asks winget when the page first opens (its setting, off by default).
    pub check_on_open: bool,
}

impl AppsPage {
    pub fn new(apps: Hub, context: AddonContext) -> Self {
        AppsPage { apps, apps_ui: AppsUi::default(), context, said: Vec::new(), check_on_open: false }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.said.push(msg.into());
    }

    /// Opens an app's folder in File Explorer through the platform's
    /// executor; a failure is told in the status bar.
    pub fn open_folder(&mut self, dir: &str) {
        if let Err(e) = self.context.open(dir) {
            self.say(e);
        }
    }

    /// The page is shown: the first time, the list is read (and winget
    /// asked, if "Check for updates when the page opens" is on).
    pub fn opened(&mut self) {
        if !self.apps_ui.started {
            self.apps_ui.started = true;
            self.apps.refresh();
            if self.check_on_open {
                self.apps.check_updates_on_open();
            }
        }
    }

    /// Draws the page below the app's header (title, Refresh).
    pub fn ui(&mut self, ui: &mut Ui) {
        let th = theme::of_ui(ui);
        self.opened();
        let st = self.apps.lock();
        let list = st.apps.clone();
        let listing = st.listing;
        let measuring = st.measuring;
        let checked_at = st.checked_at.clone();
        let checked = st.winget.is_some();
        let busy = st.busy.clone();
        let waiting = st.waiting;
        let message = st.message.clone();
        let output = st.output.clone();
        drop(st);
        let idle = busy.is_none();
        let t = now();
        self.apps_ui.checked.retain(|k| list.iter().any(|a| &a.key == k));
        let n_updates = list.iter().filter(|a| a.has_update()).count();
        let group = self.apps_ui.list_group.unwrap_or(if checked && n_updates > 0 { ListGroup::Updates } else { ListGroup::All });

        // ------------------------------------------------ actions (the app's header has the title and Refresh)
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = ui.add(egui::Button::new("More actions      ").min_size(vec2(0.0, 32.0)));
                let c = Pos2::new(more.rect.right() - 16.0, more.rect.center().y);
                crate::widgets::parts::chevron_down(ui.painter(), c);
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(260.0);
                    if ui.add(egui::Button::new("Export the list as CSV\u{2026}").min_size(vec2(0.0, 30.0))).clicked() {
                        self.export_apps_csv(&list);
                        ui.close();
                    }
                    if ui.add(egui::Button::new("Open Windows Settings \u{203a} Installed apps").min_size(vec2(0.0, 30.0))).clicked() {
                        ui.ctx().open_url(egui::OpenUrl::new_tab("ms-settings:appsfeatures"));
                        ui.close();
                    }
                });
            });
        });
        ui.add_space(8.0);

        self.apps_band(ui, &list, checked, &checked_at, busy.as_deref(), waiting, message.as_ref(), t, idle);
        if !output.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Output of the last winget run").color(th.label)).id_salt("apps-output").show(ui, |ui| {
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    ui.label(RichText::new(&output).monospace().size(11.0));
                });
            });
        }
        ui.add_space(12.0);

        // ------------------------------------------------ group row
        let n_store = list.iter().filter(|a| a.source == Source::Msix).count();
        ui.horizontal(|ui| {
            let mut list_groups = Vec::new();
            if checked {
                list_groups.push((ListGroup::Updates, "Updates", n_updates));
            }
            list_groups.push((ListGroup::All, "All apps", list.len()));
            list_groups.push((ListGroup::Store, "Store apps", n_store));
            if let Some(s) = parts::scope_group(ui, group, &list_groups).clicked {
                self.apps_ui.list_group = Some(s);
            }
            ui.add_space(10.0);
            let mut note = String::new();
            if listing {
                note = "Reading the list\u{2026}".into();
            } else if let Some((done, all)) = measuring {
                note = format!("Measuring app folders, {done} of {all}\u{2026}");
            }
            if !note.is_empty() {
                ui.spinner();
                ui.label(RichText::new(note).size(12.0).color(th.label));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                crate::widgets::search_field(ui, &mut self.apps_ui.search, "Find an app or publisher", 240.0);
            });
        });
        ui.add_space(10.0);

        // ------------------------------------------------ list + side panel
        let q = self.apps_ui.search.trim().to_lowercase();
        let shown: Vec<&App> = list
            .iter()
            .filter(|a| match group {
                ListGroup::Updates => a.has_update(),
                ListGroup::All => true,
                ListGroup::Store => a.source == Source::Msix,
            })
            .filter(|a| q.is_empty() || a.name.to_lowercase().contains(&q) || a.publisher.to_lowercase().contains(&q))
            .collect();
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let side_w = 370.0f32.min(area.width() * 0.38);
        let list_rect = Rect::from_min_max(area.min, Pos2::new(area.right() - side_w - 16.0, area.bottom()));
        let side_rect = Rect::from_min_max(Pos2::new(area.right() - side_w, area.top()), area.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side_rect));
        self.apps_list(&mut list_ui, group, shown, list.len(), listing, t);
        self.apps_side(&mut side_ui, &list, checked, t, idle);
        self.apps_dialogs(ui, &list);
    }

    /// The status band: the page's answer and its main actions.
    #[allow(clippy::too_many_arguments)]
    fn apps_band(&mut self, ui: &mut Ui, list: &[App], checked: bool, checked_at: &str, busy: Option<&str>, waiting: bool, message: Option<&(String, bool)>, t: i64, idle: bool) {
        let th = theme::of_ui(ui);
        let updates: Vec<&App> = list.iter().filter(|a| a.has_update()).collect();
        let n = updates.len();
        let headline = match busy {
            Some(b) => b.to_string(),
            None if !checked => "Apps have not been checked for newer versions yet".into(),
            None if n == 0 => "Every app winget knows is up to date".into(),
            None if n == 1 => "1 app has a newer version".into(),
            None => format!("{n} apps have a newer version"),
        };
        let mark = match busy {
            Some(_) => Mark::Busy,
            None if !checked => Mark::None,
            None if n == 0 => Mark::Tick,
            None => Mark::Dot(th.amber),
        };
        let note = format!("Checked today at {} with winget", short_time(checked_at));
        let mut band = StatusBand::new(mark, &headline).actions_width(170.0);
        if checked && busy.is_none() {
            band = band.note(&note);
        }
        if busy.is_some() {
            band = band.sub("You can keep using the app while this runs.");
        } else if !checked {
            band = band.sub("Checking asks winget (App Installer) which of your apps have a newer version. It takes a few seconds and needs no administrator rights.");
        }
        band.show(ui, |ui, area| match area {
            BandArea::Body => {
                if let Some((m, err)) = message.filter(|_| busy.is_none()) {
                    ui.label(RichText::new(m).size(12.5).color(if *err { th.red } else { th.green }));
                }
            }
            BandArea::Actions => {
                if waiting {
                    if parts::outline_button(ui, "Stop waiting", true).clicked() {
                        self.apps.stop_waiting();
                    }
                } else if !checked {
                    if parts::primary_button(ui, "Check for updates", idle).clicked() {
                        self.apps.check_updates();
                    }
                } else if ui.add_enabled(idle, egui::Button::new(RichText::new("Check again").color(th.link)).min_size(vec2(0.0, 30.0))).clicked() {
                    self.apps.check_updates();
                }
            }
            BandArea::Below => {
                if checked && !updates.is_empty() && busy.is_none() {
                    self.update_cards(ui, &th, &updates, t, idle);
                }
            }
        });
    }

    /// The band's cards: the updates split by use, each with its action.
    fn update_cards(&mut self, ui: &mut Ui, th: &Theme, updates: &[&App], t: i64, idle: bool) {
        ui.add_space(4.0);
        let in_group = |g: Group| -> Vec<&App> { updates.iter().copied().filter(|a| group_of(a, t) == g).collect() };
        let groups: Vec<(Group, Vec<&App>)> = [Group::Recent, Group::Tools, Group::Unused].into_iter().map(|g| (g, in_group(g))).filter(|(_, v)| !v.is_empty()).collect();
        let card_h = self.apps_ui.card_h;
        let mut tallest = 0.0f32;
        ui.columns(groups.len(), |cols| {
            for (i, (g, apps_in)) in groups.iter().enumerate() {
                // Not justified: wrapped text would be stretched.
                cols[i].with_layout(Layout::top_down(Align::Min), |ui| {
                    let n = apps_in.len();
                    let card = egui::Frame::new()
                        .fill(th.panel)
                        .stroke(Stroke::new(1.0, if *g == Group::Recent { th.primary } else { th.line }))
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(egui::Margin::symmetric(16, 14))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            // As tall as the tallest card last frame.
                            ui.set_min_height((card_h - 30.0).max(100.0));
                            ui.spacing_mut().item_spacing.y = 8.0;
                            let (title, count_col) = match g {
                                Group::Recent => ("you opened in the last 6 months", th.text),
                                Group::Tools => ("tools and parts of other programs", th.text),
                                Group::Unused => ("not opened for over 6 months", th.amber),
                            };
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(n.to_string()).font(th.fonts.figure_large.clone()).color(count_col));
                                ui.label(RichText::new(title).strong().color(th.text));
                            });
                            let mut sorted = apps_in.clone();
                            match g {
                                Group::Recent => sorted.sort_by_key(|a| std::cmp::Reverse(a.last_used)),
                                _ => sorted.sort_by_key(|a| display_name(&a.name, &a.version).to_lowercase()),
                            }
                            let names: Vec<String> = sorted.iter().map(|a| short_name(&a.name, &a.version)).collect();
                            let body = match g {
                                Group::Recent => format!("{}.", name_list(&names, 6)),
                                Group::Tools => format!("{}. Windows has not seen them opened from Start, so other programs may use them.", name_list(&names, 5)),
                                Group::Unused => {
                                    let size: u64 = apps_in.iter().map(|a| a.size()).sum();
                                    format!("{}. Uninstalling {} instead frees {}.", name_list(&names, 4), if n == 1 { "it" } else { "them" }, fmt_size(size))
                                }
                            };
                            ui.add(egui::Label::new(RichText::new(body).color(th.soft)).wrap());
                            ui.horizontal_wrapped(|ui| match g {
                                Group::Recent => {
                                    let label = if n == 1 { format!("Update {}", names[0]) } else { format!("Update these {n}") };
                                    if parts::primary_button(ui, &label, idle).clicked() {
                                        self.apps_ui.confirm_update = Some(sorted.iter().map(|a| a.key.clone()).collect());
                                    }
                                    ui.label(RichText::new("Installers may ask for permission").size(11.5).color(th.label));
                                }
                                _ => {
                                    if parts::outline_button(ui, &format!("Review {n}"), true).clicked() {
                                        self.apps_ui.list_group = Some(ListGroup::Updates);
                                        self.apps_ui.search.clear();
                                        if *g == Group::Tools {
                                            self.apps_ui.show_tools = true;
                                        }
                                        self.apps_ui.scroll_to_group = Some(*g);
                                        self.apps_ui.selected = sorted.first().map(|a| a.key.clone());
                                    }
                                }
                            });
                        });
                    tallest = tallest.max(card.response.rect.height());
                });
            }
        });
        if tallest > card_h + 0.5 {
            // The other cards grow to match on the next frame.
            self.apps_ui.card_h = tallest;
            ui.ctx().request_repaint();
        }
    }

    fn apps_list(&mut self, ui: &mut Ui, group: ListGroup, mut shown: Vec<&App>, total: usize, listing: bool, t: i64) {
        let th = theme::of_ui(ui);
        match list_note(listing, shown.len(), total, &self.apps_ui.search, group) {
            Some(ListNote::Instead(title, detail)) => {
                ui.label(RichText::new(title).color(th.label));
                if let Some(detail) = detail {
                    ui.label(RichText::new(detail).size(12.0).color(th.faint));
                }
                return;
            }
            Some(ListNote::Above(title, detail)) => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new(title).color(th.label));
                    ui.label(RichText::new(detail).size(12.0).color(th.faint));
                });
                ui.add_space(4.0);
            }
            None => {}
        }
        let SortOrder { column: sort, desc } = self.apps_ui.sort;
        sort_shown(&mut shown, sort, desc);

        let w = ui.available_width();
        let gap = 12.0;
        let fixed = [118.0, 84.0];
        let flex = (w - 30.0 - fixed.iter().sum::<f32>() - gap * 3.0 - 10.0).max(220.0);
        let cols = [flex * 0.56, flex * 0.44, fixed[0], fixed[1]];
        let x0 = 30.0;
        let xs: Vec<f32> = cols
            .iter()
            .scan(x0, |x, c| {
                let start = *x;
                *x += c + gap;
                Some(start)
            })
            .collect();

        // Header: names that sort when clicked.
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        let heads: [(&str, Option<Sort>); 4] = [("Application", Some(Sort::Name)), ("Version", Some(Sort::Version)), ("Last opened", Some(Sort::LastUsed)), ("Size", Some(Sort::Size))];
        for (i, (h, s)) in heads.iter().enumerate() {
            let on = s.is_some_and(|s| s == sort);
            let pos = if i == 3 { Pos2::new(hr.left() + xs[i] + cols[i], hr.center().y) } else { Pos2::new(hr.left() + xs[i], hr.center().y) };
            let align = if i == 3 { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER };
            let r = ui.painter().text(pos, align, *h, text(12.0), if on { th.text } else { th.label });
            if on {
                // After the words; before them in the right-aligned Size column.
                let x = if i == 3 { r.left() - 9.0 } else { r.right() + 9.0 };
                crate::widgets::parts::sort_mark(ui.painter(), Pos2::new(x, r.center().y), desc);
            }
            if let Some(s) = s {
                let resp = ui.interact(r.expand(4.0), ui.id().with(("apps-head", i)), Sense::click());
                let resp = if i == 2 { resp.on_hover_text("Windows records apps opened from Start, the taskbar or Explorer. A dash means it has not seen this app opened that way.") } else { resp };
                if resp.clicked() {
                    self.apps_ui.click_heading(*s);
                }
            }
        }
        // Tick all shown.
        let all_ticked = shown.iter().all(|a| self.apps_ui.checked.contains(&a.key));
        let tr = Rect::from_center_size(Pos2::new(hr.left() + 15.0, hr.center().y), vec2(14.0, 14.0));
        let tresp = ui.interact(tr.expand(4.0), ui.id().with("apps-tick-all"), Sense::click()).on_hover_text("Tick every app shown");
        tick_box(ui.painter(), tr, all_ticked, tresp.hovered());
        if tresp.clicked() {
            for a in &shown {
                if all_ticked {
                    self.apps_ui.checked.remove(&a.key);
                } else {
                    self.apps_ui.checked.insert(a.key.clone());
                }
            }
        }
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);

        // Updates: three groups. Other list_groups: one plain list.
        let mut sections: Vec<(Option<Group>, Vec<&App>)> = Vec::new();
        if group == ListGroup::Updates {
            for g in [Group::Recent, Group::Unused, Group::Tools] {
                let v: Vec<&App> = shown.iter().copied().filter(|a| group_of(a, t) == g).collect();
                if !v.is_empty() {
                    sections.push((Some(g), v));
                }
            }
        } else {
            sections.push((None, shown.clone()));
        }
        // Nothing selected (or it is gone): the first row shown.
        if self.apps_ui.selected.as_ref().is_none_or(|k| !shown.iter().any(|a| &a.key == k)) {
            self.apps_ui.selected = sections.first().and_then(|(_, v)| v.first()).map(|a| a.key.clone());
        }

        let largest = shown.iter().map(|a| a.size()).max().unwrap_or(0);
        let searching = !self.apps_ui.search.trim().is_empty();
        let mut clicked: Option<String> = None;
        let mut toggled: Option<String> = None;
        let mut menu_uninstall: Option<String> = None;
        let mut copy: Option<String> = None;
        let mut open: Option<String> = None;
        let scroll_id = match group {
            ListGroup::Updates => "apps-list-updates",
            ListGroup::All => "apps-list-all",
            ListGroup::Store => "apps-list-store",
        };
        egui::ScrollArea::vertical().id_salt(scroll_id).auto_shrink([false, false]).show(ui, |ui| {
            for (g, rows) in &sections {
                let mut limit = rows.len();
                if let Some(g) = g {
                    let (title, col, hint) = match g {
                        Group::Recent => ("Opened in the last 6 months", th.text, ""),
                        Group::Unused => ("Not opened for over 6 months", th.amber, "Consider uninstalling"),
                        Group::Tools => ("Tools and parts of other programs", th.text, "Not seen opened from Start"),
                    };
                    let (gr, gresp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                    let tr = ui.painter().text(Pos2::new(gr.left() + 10.0, gr.bottom() - 7.0), Align2::LEFT_BOTTOM, title, FontId::proportional(13.5), col);
                    ui.painter().text(Pos2::new(tr.right() + 8.0, gr.bottom() - 7.0), Align2::LEFT_BOTTOM, rows.len().to_string(), text(12.0), th.faint);
                    ui.painter().text(Pos2::new(gr.right() - 10.0, gr.bottom() - 7.0), Align2::RIGHT_BOTTOM, hint, text(12.0), th.faint);
                    if self.apps_ui.scroll_to_group == Some(*g) {
                        gresp.scroll_to_me(Some(Align::Min));
                        self.apps_ui.scroll_to_group = None;
                    }
                    // A search shows every match, folded groups included.
                    match g {
                        _ if searching => {}
                        Group::Recent if !self.apps_ui.show_all_recent => limit = rows.len().min(RECENT_ROWS),
                        Group::Tools if !self.apps_ui.show_tools => limit = 0,
                        _ => {}
                    }
                }
                for a in rows.iter().take(limit) {
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 42.0), Sense::click());
                    let selected = self.apps_ui.selected.as_deref() == Some(a.key.as_str());
                    let ticked = self.apps_ui.checked.contains(&a.key);
                    let p = ui.painter();
                    parts::row_fill(p, r, selected, resp.hovered());
                    let tick_rect = Rect::from_center_size(Pos2::new(r.left() + 15.0, r.center().y), vec2(14.0, 14.0));
                    let over_tick = resp.hover_pos().is_some_and(|pos| pos.x < r.left() + 28.0);
                    tick_box(p, tick_rect, ticked, over_tick);
                    let name = display_name(&a.name, &a.version);
                    let y1 = r.top() + 13.0;
                    let y2 = r.top() + 30.0;
                    let n = fit(p, &name, &text(13.5), cols[0]);
                    p.text(Pos2::new(r.left() + xs[0], y1), Align2::LEFT_CENTER, n, text(13.5), if selected { th.on_selected } else { th.text });
                    let pubs = if a.publisher.is_empty() { kind_words(a).to_string() } else { a.publisher.clone() };
                    let s = fit(p, &pubs, &text(11.5), cols[0]);
                    p.text(Pos2::new(r.left() + xs[0], y2), Align2::LEFT_CENTER, s, text(11.5), parts::sub_colour(&th, selected));
                    // Version, and the newer one when winget has it.
                    let vx = r.left() + xs[1];
                    if a.has_update() {
                        let from = fit(p, &a.version, &text(13.0), cols[1] * 0.45);
                        let fr = p.text(Pos2::new(vx, r.center().y), Align2::LEFT_CENTER, from, text(13.0), th.label);
                        let arrow_right = fr.right() + 18.0;
                        crate::widgets::icons::draw_glyph(p, Pos2::new(fr.right() + 12.0, r.center().y), 12.0, "arrow-right", crate::widgets::icons::Ink::Theme);
                        let to = fit(p, &a.available, &text(13.0), (vx + cols[1] - arrow_right - 6.0).max(30.0));
                        p.text(Pos2::new(arrow_right + 6.0, r.center().y), Align2::LEFT_CENTER, to, text(13.0), th.amber);
                    } else {
                        let v = fit(p, &a.version, &text(13.0), cols[1]);
                        p.text(Pos2::new(vx, r.center().y), Align2::LEFT_CENTER, v, text(13.0), th.soft);
                    }
                    let (used, col) = match a.last_used {
                        Some(s) if unused(a, t) => (ago(s, t), th.amber),
                        Some(s) => (ago(s, t), th.soft),
                        None => ("\u{2014}".to_string(), th.faint),
                    };
                    p.text(Pos2::new(r.left() + xs[2], r.center().y), Align2::LEFT_CENTER, used, text(13.0), col);
                    // Size: brighter once measured in the folder.
                    let size = a.size();
                    if size > 0 {
                        let right = r.left() + xs[3] + cols[3];
                        // Shade in proportion to the largest app shown, so big apps stand out.
                        let shade = size_shade(size, largest);
                        let cell = Rect::from_min_max(Pos2::new(r.left() + xs[3] - 6.0, r.top() + 5.0), Pos2::new(right + 6.0, r.bottom() - 5.0));
                        p.rect_filled(cell, CornerRadius::same(3), th.primary.gamma_multiply(0.55 * shade));
                        p.text(Pos2::new(right, r.center().y), Align2::RIGHT_CENTER, fmt_size(size), text(13.0), if a.size_measured.is_some() { th.soft } else { th.faint });
                    }
                    if resp.clicked() {
                        if over_tick {
                            toggled = Some(a.key.clone());
                        } else {
                            clicked = Some(a.key.clone());
                        }
                    }
                    let key = a.key.clone();
                    let folder = a.folder();
                    let details = details_text(a);
                    let blocked = a.uninstall_blocker().is_some();
                    resp.context_menu(|ui| {
                        if ui.add_enabled(!blocked, egui::Button::new("Uninstall\u{2026}")).clicked() {
                            menu_uninstall = Some(key.clone());
                            ui.close();
                        }
                        if let Some(dir) = &folder {
                            if ui.button("Open folder").clicked() {
                                open = Some(dir.clone());
                                ui.close();
                            }
                        }
                        if ui.button("Copy details").clicked() {
                            copy = Some(details.clone());
                            ui.close();
                        }
                    });
                }
                if let Some(g) = g {
                    let more = match g {
                        _ if searching => None,
                        Group::Recent if rows.len() > RECENT_ROWS => Some(if self.apps_ui.show_all_recent { "Show fewer".to_string() } else { format!("Show {} more", rows.len() - RECENT_ROWS) }),
                        Group::Tools => Some(if self.apps_ui.show_tools { "Show fewer".to_string() } else { format!("Show all {}", rows.len()) }),
                        _ => None,
                    };
                    if let Some(label) = more {
                        ui.horizontal(|ui| {
                            ui.add_space(10.0);
                            if ui.link(RichText::new(label).size(12.5)).clicked() {
                                match g {
                                    Group::Recent => self.apps_ui.show_all_recent = !self.apps_ui.show_all_recent,
                                    _ => self.apps_ui.show_tools = !self.apps_ui.show_tools,
                                }
                            }
                        });
                    }
                    ui.add_space(4.0);
                }
            }
        });
        if let Some(k) = toggled {
            if !self.apps_ui.checked.remove(&k) {
                self.apps_ui.checked.insert(k);
            }
        }
        if let Some(k) = clicked {
            self.apps_ui.selected = Some(k);
        }
        if let Some(k) = menu_uninstall {
            self.apps_ui.confirm_uninstall = Some(vec![k]);
        }
        if let Some(c) = copy {
            ui.ctx().copy_text(c);
            self.say("Details copied");
        }
        if let Some(dir) = open {
            self.open_folder(&dir);
        }
    }

    fn apps_side(&mut self, ui: &mut Ui, list: &[App], checked: bool, t: i64, idle: bool) {
        let ticked: Vec<&App> = list.iter().filter(|a| self.apps_ui.checked.contains(&a.key)).collect();
        let shown = if ticked.len() > 1 {
            Some(None)
        } else {
            self.apps_ui.selected.as_ref().and_then(|k| list.iter().find(|a| &a.key == k)).map(|a| Some(a.clone()))
        };
        parts::SidePanel::new("apps-side").show(ui, shown, |ui, app| match app {
            None => self.side_ticked(ui, &ticked, idle),
            Some(a) => self.side_app(ui, &a, checked, t, idle),
        });
    }

    fn side_ticked(&mut self, ui: &mut Ui, ticked: &[&App], idle: bool) {
        let th = theme::of_ui(ui);
        let size: u64 = ticked.iter().map(|a| a.size()).sum();
        ui.label(RichText::new(format!("{} apps ticked", ticked.len())).font(display(17.0)).color(th.text));
        ui.label(RichText::new(format!("{} in their folders", fmt_size(size))).size(12.0).color(th.label));
        ui.add_space(4.0);
        for a in ticked.iter().take(12) {
            ui.horizontal(|ui| {
                ui.label(RichText::new(display_name(&a.name, &a.version)).color(th.soft));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(fmt_size(a.size())).size(12.0).color(th.faint));
                });
            });
        }
        if ticked.len() > 12 {
            ui.label(RichText::new(format!("and {} more", ticked.len() - 12)).color(th.faint));
        }
        ui.add_space(6.0);
        separator(ui);
        ui.add_space(6.0);
        let upd: Vec<String> = ticked.iter().filter(|a| a.has_update()).map(|a| a.key.clone()).collect();
        ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
            if !upd.is_empty() && parts::primary_button(ui, &format!("Update {} with a newer version", upd.len()), idle).clicked() {
                self.apps_ui.confirm_update = Some(upd.clone());
            }
            if parts::destructive_button(ui, &format!("Uninstall {} apps\u{2026}", ticked.len()), idle).clicked() {
                self.apps_ui.confirm_uninstall = Some(ticked.iter().map(|a| a.key.clone()).collect());
            }
            if parts::outline_button(ui, "Clear ticks", true).clicked() {
                self.apps_ui.checked.clear();
            }
        });
    }

    fn side_app(&mut self, ui: &mut Ui, a: &App, checked: bool, t: i64, idle: bool) {
        let th = theme::of_ui(ui);
        let name = display_name(&a.name, &a.version);
        ui.add(egui::Label::new(RichText::new(&name).font(display(19.0)).color(th.text)).wrap());
        let mut sub = if a.publisher.is_empty() { kind_words(a).to_string() } else { a.publisher.clone() };
        if !a.installed.is_empty() {
            sub.push_str(&format!(" \u{00b7} installed {}", long_date(&a.installed)));
        }
        ui.label(RichText::new(sub).size(12.0).color(th.label));
        ui.add_space(6.0);

        // ---- versions
        // Versions such as "N-125365-g9a01c1cb6a-20260630" have no place to
        // wrap: they break anywhere rather than widen the panel.
        let version = |ui: &mut Ui, v: &str, other: &str, col: Color32, size: f32, width: f32| {
            let (head, tail) = split_version(v, other);
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = width;
            job.wrap.break_anywhere = true;
            job.append(&head, 0.0, egui::TextFormat { font_id: display(size), color: th.faint, ..Default::default() });
            job.append(&tail, 0.0, egui::TextFormat { font_id: display(size), color: col, ..Default::default() });
            let galley = ui.painter().layout_job(job);
            ui.label(galley);
        };
        if a.has_update() {
            let half = ((ui.available_width() - 22.0) / 2.0).max(60.0);
            egui::Grid::new("apps-compare").striped(false).num_columns(2).spacing([22.0, 4.0]).min_col_width(half).max_col_width(half).show(ui, |ui| {
                ui.label(RichText::new("Installed").size(12.0).color(th.label));
                ui.label(RichText::new(format!("On winget{}", if a.winget_source == "msstore" { " (Microsoft Store)" } else { "" })).size(12.0).color(th.amber));
                ui.end_row();
                version(ui, &a.version, &a.available, th.text, 15.0, half);
                version(ui, &a.available, &a.version, th.amber, 15.0, half);
                ui.end_row();
            });
        } else {
            ui.label(RichText::new("Version").size(12.0).color(th.label));
            let w = ui.available_width();
            version(ui, if a.version.is_empty() { "\u{2014}" } else { &a.version }, "", th.text, 14.0, w);
            if checked && !a.winget_id.is_empty() {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                    tick(ui.painter(), r.center());
                    ui.label(RichText::new("Up to date on winget").color(th.green));
                });
            } else if checked {
                ui.label(RichText::new("winget does not know this app, so it can't tell whether a newer version exists.").size(12.0).color(th.label));
            } else {
                ui.label(RichText::new("Not checked for a newer version yet.").size(12.0).color(th.label));
            }
        }

        // ---- use
        let (note, warm) = match a.last_used {
            Some(s) if unused(a, t) => (
                format!(
                    "Not opened for {} (last on {}).{}",
                    ago(s, t).trim_end_matches(" ago").to_lowercase(),
                    long_date(&apps::ymd(s)),
                    if a.size() > 0 { format!(" Uninstalling it frees {}.", fmt_size(a.size())) } else { String::new() }
                ),
                true,
            ),
            Some(s) => (format!("Last opened {} ({}).", ago(s, t).to_lowercase(), long_date(&apps::ymd(s))), false),
            None => ("Windows has not seen it opened from Start, the taskbar or Explorer. Other programs may still use it.".into(), false),
        };
        if warm {
            parts::warm_note(ui, &note);
        } else {
            parts::neutral_note(ui, &note);
        }

        // ---- facts
        ui.add_space(2.0);
        egui::Grid::new("apps-facts").striped(false).num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
            let mut fact = |k: &str, v: String| {
                if !v.is_empty() {
                    ui.label(RichText::new(k).color(th.label));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                }
            };
            fact(
                "Size",
                match (a.size_measured, a.size_reported) {
                    (Some(m), _) => format!("{} in its folder", fmt_size(m)),
                    (None, r) if r > 0 => format!("{} (reported by the app)", fmt_size(r)),
                    _ => "Unknown".into(),
                },
            );
            fact("Scope", a.scope_text());
            fact("Installed", if a.installed.is_empty() { String::new() } else { long_date(&a.installed) });
            fact("Source", a.source.label().into());
            fact("Registry key", a.registry_key().into());
            fact("Folder", a.folder().unwrap_or_default());
            fact("winget id", a.winget_id.clone());
        });

        // ---- actions
        ui.add_space(8.0);
        separator(ui);
        ui.add_space(8.0);
        if a.has_update() {
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                let to = short_version(&a.available);
                let label = if to.chars().count() > 18 { "Update to the newer version".to_string() } else { format!("Update to {to}") };
                if parts::primary_button(ui, &label, idle).clicked() {
                    self.apps.update(vec![a.key.clone()]);
                }
            });
            ui.label(RichText::new(format!("The installer may ask for permission. Close {} first if it is open.", short_name(&a.name, &a.version))).size(11.5).color(th.label));
            ui.add_space(4.0);
        }
        let blocker = a.uninstall_blocker();
        ui.horizontal_wrapped(|ui| {
            let un = parts::destructive_button(ui, "Uninstall\u{2026}", idle && blocker.is_none());
            if let Some(b) = &blocker {
                un.on_disabled_hover_text(b);
            } else if un.clicked() {
                self.apps_ui.confirm_uninstall = Some(vec![a.key.clone()]);
            }
            if a.can_modify() && parts::outline_button(ui, "Modify", idle).on_hover_text("Starts the app's own setup to add or remove parts of it").clicked() {
                self.apps.modify(a.key.clone(), false);
            }
            if a.can_repair() && parts::outline_button(ui, "Repair", idle).on_hover_text("Windows Installer puts back missing or damaged files").clicked() {
                self.apps.modify(a.key.clone(), true);
            }
        });
        if let Some(b) = &blocker {
            ui.label(RichText::new(b).size(11.5).color(th.label));
        }
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 16.0;
            if let Some(dir) = a.folder() {
                if ui.link(RichText::new("Open folder").size(12.5)).clicked() {
                    self.open_folder(&dir);
                }
            }
            let site = if a.about_url.starts_with("http") { a.about_url.clone() } else { a.help_url.clone() };
            if site.starts_with("http") && ui.link(RichText::new("Website").size(12.5)).clicked() {
                ui.ctx().open_url(egui::OpenUrl::new_tab(site));
            }
            if ui.link(RichText::new("Copy details").size(12.5)).clicked() {
                ui.ctx().copy_text(details_text(a));
                self.say("Details copied");
            }
        });
    }

    fn apps_dialogs(&mut self, ui: &mut Ui, list: &[App]) {
        let th = theme::of_ui(ui);
        if let Some(keys) = self.apps_ui.confirm_update.clone() {
            let targets: Vec<&App> = keys.iter().filter_map(|k| list.iter().find(|a| &a.key == k)).collect();
            let mut close = false;
            let title = if targets.len() == 1 { format!("Update {}?", display_name(&targets[0].name, &targets[0].version)) } else { format!("Update {} apps?", targets.len()) };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ui.ctx(), |ui| {
                    ui.set_max_width(480.0);
                    ui.label("winget downloads and runs each installer in turn. Installers may ask for administrator permission, and an app that is open may need to be closed first.");
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                        for a in &targets {
                            ui.label(RichText::new(format!("\u{2022} {}  {} \u{2192} {}", display_name(&a.name, &a.version), a.version, a.available)).color(th.soft));
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let label = if targets.len() == 1 { "Update".to_string() } else { format!("Update {} apps", targets.len()) };
                        if parts::primary_button(ui, &label, true).clicked() {
                            self.apps.update(keys.clone());
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if close {
                self.apps_ui.confirm_update = None;
            }
        }
        let Some(keys) = self.apps_ui.confirm_uninstall.clone() else { return };
        let targets: Vec<&App> = keys.iter().filter_map(|k| list.iter().find(|a| &a.key == k)).collect();
        if targets.is_empty() {
            self.apps_ui.confirm_uninstall = None;
            return;
        }
        let mut close = false;
        let title = if targets.len() == 1 { format!("Uninstall {}?", display_name(&targets[0].name, &targets[0].version)) } else { format!("Uninstall {} applications?", targets.len()) };
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ui.ctx(), |ui| {
                ui.set_max_width(460.0);
                if targets.len() == 1 {
                    ui.label(single_uninstall_note(targets[0]));
                } else {
                    ui.label("They are removed one after another: each app's own uninstaller starts in turn, and Windows removes a Store app for your account. An uninstaller may ask questions of its own. This can't be undone from here.");
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical().max_height(180.0).show(ui, |ui| {
                        for a in &targets {
                            ui.label(RichText::new(format!("\u{2022} {}  {}", display_name(&a.name, &a.version), fmt_size(a.size()))).color(th.soft));
                        }
                    });
                }
                for b in targets.iter().filter_map(|a| a.uninstall_blocker()) {
                    ui.label(RichText::new(b).color(th.amber).size(12.0));
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let label = if targets.len() == 1 { "Uninstall".to_string() } else { format!("Uninstall {} apps", targets.len()) };
                    if parts::destructive_button(ui, &label, true).clicked() {
                        self.apps.uninstall(keys.clone());
                        self.apps_ui.checked.clear();
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if close {
            self.apps_ui.confirm_uninstall = None;
        }
    }

    fn export_apps_csv(&mut self, list: &[App]) {
        let Some(path) = self.context.pick_save_as(&crate::platform::CSV_FILE, "installed-apps.csv") else { return };
        let out = apps_csv(list);
        match std::fs::write(&path, format!("\u{feff}{out}")) {
            Ok(()) => self.say(format!("Saved {}", path)),
            Err(e) => self.say(format!("Could not save: {e}")),
        }
    }
}

/// The CSV export: the main columns (Application, Publisher, Version,
/// Installed, Estimated size, Scope, Source) first, then the details.
fn apps_csv(list: &[App]) -> String {
    let mut out = String::from(
        "Application,Publisher,Version,Installed,Estimated size,Scope,Source,Newer version,Last opened,Size measured,32-bit,Registry key,Folder,winget id,Uninstall\n",
    );
    for a in list {
        let row = [
            a.name.clone(),
            a.publisher.clone(),
            a.version.clone(),
            a.installed.clone(),
            a.size_reported.to_string(),
            a.scope_label().into(),
            a.source.label().into(),
            a.available.clone(),
            a.last_used.map(apps::ymd).unwrap_or_default(),
            a.size_measured.map(|m| m.to_string()).unwrap_or_default(),
            if a.bits32 { "yes".into() } else { "no".into() },
            a.registry_key().into(),
            a.folder().unwrap_or_default(),
            a.winget_id.clone(),
            a.uninstall_text().into(),
        ];
        out.push_str(&row.iter().map(|s| csv_field(s)).collect::<Vec<_>>().join(","));
        out.push('\n');
    }
    out
}

/// A tick box painted in a row.
fn tick_box(p: &egui::Painter, r: Rect, on: bool, hovered: bool) {
    let th = theme::current(p.ctx());
    if on {
        p.rect_filled(r, CornerRadius::same(3), th.primary);
        p.line(vec![r.center() + vec2(-3.5, 0.0), r.center() + vec2(-1.0, 2.8), r.center() + vec2(3.8, -2.8)], Stroke::new(1.8, th.on_primary));
    } else {
        p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, if hovered { th.soft } else { th.control_line }), StrokeKind::Inside);
    }
}

/// What the confirmation says before one app is uninstalled: a Store app is
/// removed by Windows for this account, any other runs its own uninstaller.
fn single_uninstall_note(a: &App) -> &'static str {
    if a.source == apps::Source::Msix {
        "Windows removes this Store app for your account. This can't be undone from here."
    } else {
        "The app's own uninstaller will start. It may ask questions of its own, and it decides what is removed. This can't be undone from here."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_store_app_is_not_said_to_run_its_own_uninstaller() {
        let store = App { source: apps::Source::Msix, ..Default::default() };
        let desktop = App { source: apps::Source::Registry, ..Default::default() };
        assert!(single_uninstall_note(&store).starts_with("Windows removes this Store app"));
        assert!(single_uninstall_note(&desktop).starts_with("The app's own uninstaller will start"));
    }

    /// Installed Apps must keep working with Drivers turned off.
    #[test]
    fn installed_apps_uses_nothing_from_the_drivers_add_on() {
        for (file, src) in [("apps.rs", include_str!("apps.rs")), ("page.rs", include_str!("page.rs")), ("installed_apps.rs", include_str!("../installed_apps.rs"))] {
            assert!(!src.contains(concat!("crate::", "drivers")), "{file} uses the Drivers module");
            assert!(!src.contains(concat!("crate::", "page_drivers")), "{file} uses the Drivers page");
            assert!(!src.contains(concat!("addons::", "drivers")) && !src.contains(concat!("super::", "drivers")), "{file} uses the Drivers add-on");
        }
    }

    #[test]
    fn size_shading_grows_with_the_app_against_the_largest_shown() {
        let gb = 1 << 30;
        // The largest app shown gets the full shade.
        assert_eq!(size_shade(4 * gb, 4 * gb), 1.0);
        // Halfway and a quarter of the way read in proportion.
        assert_eq!(size_shade(2 * gb, 4 * gb), 0.5);
        assert_eq!(size_shade(gb, 4 * gb), 0.25);
        // The smallest app still gets a faint shade, never more than its share.
        let tiny = size_shade(1, 4 * gb);
        assert!(tiny > 0.0 && tiny < 0.01, "tiny app shade {tiny}");
        // No size: no shade, even when nothing else has a size either.
        assert_eq!(size_shade(0, 4 * gb), 0.0);
        assert_eq!(size_shade(0, 0), 0.0);
    }

    #[test]
    fn ages_read_well() {
        let n = 1_000 * 86_400;
        assert_eq!(ago(n, n), "Today");
        assert_eq!(ago(n - 86_400, n), "Yesterday");
        assert_eq!(ago(n - 10 * 86_400, n), "10 days ago");
        assert_eq!(ago(n - 200 * 86_400, n), "7 months ago");
        assert_eq!(ago(n - 800 * 86_400, n), "2 years ago");
    }

    #[test]
    fn names_lose_the_version_they_repeat() {
        assert_eq!(display_name("Audacity 3.7.8", "3.7.8"), "Audacity");
        assert_eq!(display_name("LocalSend versione 1.17.0", "1.17.0"), "LocalSend");
        assert_eq!(display_name("Anaconda3 2025.06-1 (Python 3.13.5 64-bit)", "2025.06-1"), "Anaconda3 2025.06-1 (Python 3.13.5 64-bit)");
        assert_eq!(display_name("7-Zip", ""), "7-Zip");
    }

    #[test]
    fn updates_split_by_use() {
        let n = 1_000 * 86_400;
        let a = |last: Option<i64>| App { last_used: last, ..Default::default() };
        assert_eq!(group_of(&a(Some(n - 5 * 86_400)), n), Group::Recent);
        assert_eq!(group_of(&a(Some(n - 400 * 86_400)), n), Group::Unused);
        assert_eq!(group_of(&a(None), n), Group::Tools);
    }

    #[test]
    fn versions_compare_by_numeric_parts() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("3.10.0", "3.9.2"), Greater);
        assert_eq!(compare_versions("3.9.2", "3.10.0"), Less);
        assert_eq!(compare_versions("10.0", "9.99.99"), Greater);
        assert_eq!(compare_versions("1.2", "1.2.1"), Less);
        assert_eq!(compare_versions("2.5.0", "2.5.0"), Equal);
        assert_eq!(compare_versions("2025.06-1", "2025.06-2"), Less);
    }

    #[test]
    fn non_numeric_versions_come_after_numeric_ones() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("N-125365-g9a01c1cb6a-20260630", "99.0"), Greater);
        assert_eq!(compare_versions("1.0", "beta"), Less);
        assert_eq!(compare_versions("alpha", "Beta"), Less);
    }

    #[test]
    fn missing_versions_come_last() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("", "1.0"), Greater);
        assert_eq!(compare_versions("  ", "unknown"), Greater);
        assert_eq!(compare_versions("1.0", ""), Less);
        assert_eq!(compare_versions("", ""), Equal);
    }

    #[test]
    fn version_sort_goes_both_ways_with_missing_last() {
        let app = |key: &str, v: &str| App { key: key.into(), name: key.into(), version: v.into(), ..Default::default() };
        let all = [app("a", ""), app("b", "3.9.2"), app("c", "beta"), app("d", "3.10.0")];
        let mut apps: Vec<&App> = all.iter().collect();
        let keys = |apps: &[&App]| apps.iter().map(|a| a.key.clone()).collect::<Vec<_>>().join("");
        sort_shown(&mut apps, Sort::Version, false);
        assert_eq!(keys(&apps), "bdca");
        sort_shown(&mut apps, Sort::Version, true);
        assert_eq!(keys(&apps), "cdba");
    }

    #[test]
    fn clicking_the_active_heading_flips_the_direction() {
        let mut ui = AppsUi::default();
        let order = |column, desc| SortOrder { column, desc };
        assert_eq!(ui.sort, order(Sort::Name, false));
        ui.click_heading(Sort::Version);
        assert_eq!(ui.sort, order(Sort::Version, false));
        ui.click_heading(Sort::Version);
        assert_eq!(ui.sort, order(Sort::Version, true));
        ui.click_heading(Sort::Size);
        assert_eq!(ui.sort, order(Sort::Size, true), "size starts biggest first");
        ui.click_heading(Sort::Size);
        assert_eq!(ui.sort, order(Sort::Size, false));
        ui.click_heading(Sort::LastUsed);
        assert_eq!(ui.sort, order(Sort::LastUsed, true), "last opened starts newest first");
        ui.click_heading(Sort::Name);
        ui.click_heading(Sort::Name);
        assert_eq!(ui.sort, order(Sort::Name, true), "names Z to A");
    }

    #[test]
    fn button_versions_stay_short() {
        assert_eq!(short_version("4.0.2.263060742"), "4.0.2");
        assert_eq!(short_version("13.827.2.6342"), "13.827.2.6342");
        assert_eq!(name_list(&["A".into(), "B".into(), "C".into()], 2), "A, B and 1 more");
        assert_eq!(name_list(&["A".into(), "B".into()], 4), "A and B");
    }

    #[test]
    fn empty_list_says_why_it_is_empty() {
        let reading = ListNote::Instead("Scanning", Some("Reading the machine and current-user application inventory."));
        let nothing = ListNote::Instead("No installed applications were reported", Some("Windows did not publish any visible uninstall entries."));
        let no_match = ListNote::Instead("No app or publisher matches what you typed.", None);
        let no_update = ListNote::Instead("No app has a newer version.", None);
        let note = |listing, total, search, group| list_note(listing, 0, total, search, group);

        // Reading wins over everything else, even with a search typed.
        assert_eq!(note(true, 0, "zip", ListGroup::Updates), Some(reading));
        assert_eq!(note(true, 12, "", ListGroup::All), Some(reading));
        // Windows reported nothing: not a filter problem.
        assert_eq!(note(false, 0, "", ListGroup::All), Some(nothing));
        assert_eq!(note(false, 0, "zip", ListGroup::Updates), Some(nothing));
        // Apps exist but the search hides them.
        assert_eq!(note(false, 12, "  zip ", ListGroup::Updates), Some(no_match));
        assert_eq!(note(false, 12, "zip", ListGroup::All), Some(no_match));
        // Apps exist, no search, none has an update.
        assert_eq!(note(false, 12, "   ", ListGroup::Updates), Some(no_update));
    }

    #[test]
    fn a_read_in_progress_shows_above_the_apps_of_the_last_read() {
        // Refresh: the apps of the last read stay listed, the scan is told above them.
        let above = ListNote::Above("Scanning", "Reading the machine and current-user application inventory.");
        assert_eq!(list_note(true, 30, 30, "", ListGroup::All), Some(above));
        assert_eq!(list_note(true, 2, 30, "zip", ListGroup::Updates), Some(above));
        // Read: the rows alone.
        assert_eq!(list_note(false, 30, 30, "", ListGroup::All), None);
    }
}

/// The CSV export (issue #76).
#[cfg(test)]
mod csv_export_tests {
    use super::*;

    #[test]
    fn csv_has_the_main_columns_then_the_details() {
        let audacity = App {
            key: "HKLM\\Audacity_is1".into(),
            name: "Audacity 3.7.8".into(),
            publisher: "Audacity Team, \"Muse\"".into(),
            version: "3.7.8".into(),
            installed: "2026-01-02".into(),
            size_reported: 1000,
            size_measured: Some(2048),
            source: Source::Registry,
            winget_id: "Audacity.Audacity".into(),
            ..Default::default()
        };
        let csv = apps_csv(&[audacity]);
        let mut lines = csv.lines();
        let head: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(&head[..7], ["Application", "Publisher", "Version", "Installed", "Estimated size", "Scope", "Source"]);
        for ours in ["Newer version", "Last opened", "Size measured", "32-bit", "Registry key", "Folder", "winget id", "Uninstall"] {
            assert!(head.contains(&ours), "missing column {ours}");
        }
        let row = lines.next().unwrap();
        assert!(row.starts_with("Audacity 3.7.8,\"Audacity Team, \"\"Muse\"\"\",3.7.8,2026-01-02,1000,Machine,Registry,"), "{row}");
        assert!(row.contains(",2048,no,Audacity_is1,"), "{row}");
        assert!(lines.next().is_none());
    }

    #[test]
    fn fields_with_commas_and_quotes_are_quoted() {
        let a = App {
            name: "Foo, the \"best\" app".into(),
            publisher: "Acme, Inc.".into(),
            version: "1.2".into(),
            size_reported: 2048,
            uninstall: "\"C:\\Program Files\\Foo\\uninst.exe\" /S".into(),
            ..Default::default()
        };
        let csv = apps_csv(&[a]);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("Application,Publisher,Version,"));
        assert_eq!(lines[1], "\"Foo, the \"\"best\"\" app\",\"Acme, Inc.\",1.2,,2048,Machine,Registry,,,,no,,,,\"\"\"C:\\Program Files\\Foo\\uninst.exe\"\" /S\"");
    }

    #[test]
    fn plain_fields_stay_bare() {
        let a = App { name: "Bar".into(), publisher: "Acme".into(), user_scope: true, size_measured: Some(10), source: apps::Source::Msix, ..Default::default() };
        assert_eq!(apps_csv(&[a]).lines().nth(1), Some("Bar,Acme,,,0,Current user,MSIX,,,10,no,,,,"));
    }

    #[test]
    fn a_store_app_has_no_registry_key_and_is_uninstalled_by_its_package() {
        let calc = App {
            key: "MSIX\\Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe".into(),
            name: "Calculator".into(),
            user_scope: true,
            source: Source::Msix,
            package: "Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe".into(),
            ..Default::default()
        };
        let row = apps_csv(std::slice::from_ref(&calc));
        let row = row.lines().nth(1).unwrap();
        assert!(row.ends_with(",no,,,,Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe"), "no registry key; Uninstall is the package: {row}");
        let details = details_text(&calc);
        assert!(!details.contains("Registry key"), "no Registry key line: {details}");
        assert!(details.ends_with("Uninstall: Microsoft.WindowsCalculator_11.2502.2.0_x64__8wekyb3d8bbwe"), "{details}");

        let classic = App { key: "HKLM\\Audacity_is1".into(), uninstall: "unins000.exe".into(), ..Default::default() };
        assert!(details_text(&classic).contains("\nRegistry key: Audacity_is1\n"), "a classic program keeps its key");
    }
}
