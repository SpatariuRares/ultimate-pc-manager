//! Settings, Settings › Add-ons and Colors, in Design A (boards P16, P18
//! and P17). Colours come only from the theme.

use std::collections::HashMap;
use std::sync::atomic::Ordering;

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Ui};

use crate::addons::overview::{intensity, Act, Detail, Intensity, Overview, Row, Switch};
use crate::app::{nav_items, page_name, Action, Page, TaskApp};
use crate::platform::PageContext;
use crate::settings::{GraphColor, Settings, SPEEDS};
use crate::theme::{self, Theme};
use crate::widgets::parts::{
    category_heading, facts, group_heading, link_button, neutral_note, outline_button, panel, primary_button, row_fill, segmented,
    set_settings_query, setting_combo, setting_row, setting_switch, settings_card, settings_category, settings_searching, sub_colour, switch,
    tag, warm_note, BandArea, Mark, SidePanel, StatusBand,
};
use crate::widgets::{graph, need_icon, rgb, tick};

/// The project's repository, opened by "GitHub" on Settings.
pub const REPOSITORY_URL: &str = "https://github.com/SpatariuRares/ultimate-pc-manager";
/// Where users can support the project, opened by "Buy me a coffee".
pub const DONATE_URL: &str = "https://buymeacoffee.com/spatariurares";

/// "Buy me a coffee" (the main action) and "GitHub", side by side on
/// Settings › About & support. Returns the link of the one clicked.
fn project_links(ui: &mut Ui) -> Option<&'static str> {
    let mut url = None;
    ui.horizontal(|ui| {
        if primary_button(ui, "Buy me a coffee", true).on_hover_text(DONATE_URL).clicked() {
            url = Some(DONATE_URL);
        }
        if outline_button(ui, "GitHub", true).on_hover_text(REPOSITORY_URL).clicked() {
            url = Some(REPOSITORY_URL);
        }
    });
    url
}

/// About & support: the app's name and version, why it asks for a
/// coffee, and the project's links.
fn about_rows(ui: &mut Ui) {
    let t = theme::of_ui(ui);
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Ultimate").font(t.fonts.figure_small.clone()).color(t.primary));
        ui.label(RichText::new("PC Manager").font(t.fonts.figure_small.clone()).color(t.text));
        ui.label(RichText::new(concat!("v", env!("CARGO_PKG_VERSION"))).font(egui::FontId::new(12.0, theme::display())).color(t.label));
    });
    ui.add_space(10.0);
    egui::Frame::new().fill(t.selected).corner_radius(6).inner_margin(egui::Margin::symmetric(14, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(DONATE_LINE).font(t.fonts.row.clone()).color(t.on_selected));
    });
    ui.add_space(12.0);
    if let Some(url) = project_links(ui) {
        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
    }
    ui.add_space(12.0);
    ui.add(egui::Label::new(RichText::new(REMEMBERED).font(t.fonts.label.clone()).color(t.faint)).wrap());
    ui.add_space(8.0);
}

/// Why About & support asks for a coffee.
const DONATE_LINE: &str = "Every add-on is free. If the app saves you time, a coffee helps keep it going.";
/// What the app remembers without a setting.
const REMEMBERED: &str =
    "Settings are saved in settings.json with the app's data. Column widths, sort orders and the window position are remembered without a control here.";

/// Width of the Settings category list.
const CATEGORIES: f32 = 230.0;

/// A part of Settings › General.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SettingsCat {
    #[default]
    General,
    Appearance,
    /// The settings of the add-on with this id.
    Addon(&'static str),
    About,
}

impl SettingsCat {
    /// The key its search count is kept under.
    fn key(self) -> &'static str {
        match self {
            SettingsCat::General => "general",
            SettingsCat::Appearance => "appearance",
            SettingsCat::Addon(id) => id,
            SettingsCat::About => "about",
        }
    }
}

/// What Settings › General shows: the category, the search, and how many
/// rows each card matched last frame (for the category list).
#[derive(Default)]
pub struct SettingsView {
    pub cat: SettingsCat,
    pub query: String,
    pub(crate) counts: HashMap<&'static str, usize>,
}

/// Width of the side panel beside a list.
const PANEL: f32 = 340.0;
/// Room between a list and its side panel.
const GAP: f32 = 16.0;

/// Under "Hide Microsoft Services and Processes": what the switch hides.
const HIDE_MICROSOFT_NOTE: &str = "Hides services and processes whose program file carries a verified Microsoft Windows signature.";

/// The face of small headings ("Core, always on", "Add-ons").
impl TaskApp {
    pub fn page_settings(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        if self.show_addons {
            // Settings › Add-ons: "Settings" leads back.
            let mut back = false;
            ui.horizontal(|ui| {
                ui.set_min_height(52.0);
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 40.0), Sense::hover());
                self.page_icon(ui.painter(), r.center(), self.page, t.label);
                let link = egui::Label::new(RichText::new("Settings").font(t.fonts.row.clone()).color(t.label)).sense(Sense::click());
                back = ui.add(link).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                crate::widgets::crumb_separator(ui);
                ui.label(RichText::new("Add-ons").font(t.fonts.page_title.clone()).color(t.text));
            });
            if back {
                self.show_addons = false;
            }
            ui.add_space(6.0);
            self.settings_addons(ui);
        } else {
            ui.horizontal(|ui| {
                ui.set_min_height(52.0);
                self.page_title(ui, "Settings");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    crate::widgets::search_field(ui, &mut self.settings_view.query, "Find a setting", 260.0);
                });
            });
            ui.add_space(6.0);
            self.settings_general(ui, actions);
        }
    }

    /// Settings › Add-ons: the band, the always-on core, the list with one
    /// switch per add-on, and the selected add-on in a side panel. Its
    /// status line goes to the app's status bar.
    fn settings_addons(&mut self, ui: &mut Ui) {
        let view = Overview::of(&self.addons, self.addon_selected);
        let t = theme::of_ui(ui);
        let mut input = AddonInput::default();
        let stopping = view.rows.iter().any(|r| r.switch == Switch::Stopping);
        let headline = view.banner();
        let note = view.note();
        StatusBand::new(Mark::Dot(if stopping { t.amber } else { t.green }), &headline).sub(&note).actions_width(0.0).show(ui, |_, _| {});
        ui.add_space(14.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            let list_w = (ui.available_width() - PANEL - GAP).max(360.0);
            ui.vertical(|ui| {
                ui.set_width(list_w);
                ui.set_height(height);
                egui::ScrollArea::vertical().id_salt("settings-addons").auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_width(list_w - 12.0);
                    core_block(ui, &view.core);
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new("Add-ons").font(t.fonts.group_heading.clone()).color(t.text));
                        ui.label(RichText::new(view.rows.len().to_string()).color(t.faint));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(20.0);
                            ui.label(RichText::new("On").font(t.fonts.label.clone()).color(t.faint));
                        });
                    });
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for row in &view.rows {
                        addon_row(ui, row, &mut input);
                    }
                });
            });
            ui.add_space(GAP);
            ui.vertical(|ui| {
                ui.set_width(PANEL);
                ui.set_height(height);
                SidePanel::new("addon-panel").show(ui, view.detail.as_ref(), |ui, d| addon_panel(ui, d, &mut input));
            });
        });
        if input.picked.is_some() {
            self.addon_selected = input.picked;
        }
        for (id, v) in input.toggles {
            self.set_addon(id, v);
        }
        if let Some(p) = input.open {
            self.page = p;
        }
    }
}

/// What the user did on Settings › Add-ons this frame, applied after drawing.
#[derive(Default)]
struct AddonInput {
    toggles: Vec<(&'static str, bool)>,
    picked: Option<&'static str>,
    open: Option<Page>,
}

/// The tag of a tier: "Free" in green, "Paid" on the warm note.
fn tier_tag(ui: &mut Ui, tier: &str) -> egui::Response {
    let t = theme::of_ui(ui);
    if tier == "Free" {
        tag(ui, tier, t.green.gamma_multiply(0.16), t.green)
    } else {
        tag(ui, tier, t.warm_note, t.warm_note_text)
    }
}

/// The dot's colour for a declared use: faint none, green low, amber
/// medium, red high.
fn intensity_colour(t: &Theme, i: Intensity) -> Color32 {
    match i {
        Intensity::None => t.faint,
        Intensity::Low => t.green,
        Intensity::Medium => t.amber,
        Intensity::High => t.red,
    }
}

/// One add-on in the list: its name with its requirement icons, what it
/// does, its tier and its switch. Click the row to show it in the side
/// panel.
fn addon_row(ui: &mut Ui, row: &Row, input: &mut AddonInput) {
    let t = theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    row_fill(ui.painter(), rect, row.selected, resp.hovered());
    if resp.clicked() {
        input.picked = Some(row.id);
    }
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(vec2(12.0, 0.0))).layout(Layout::right_to_left(Align::Center)));
    ui.style_mut().interaction.selectable_labels = false;
    ui.spacing_mut().item_spacing.x = 14.0;
    let stopping = row.switch == Switch::Stopping;
    let on = row.switch != Switch::Off;
    let sw = switch(&mut ui, on, !stopping);
    if let Some(why) = row.switch.hint() {
        sw.on_hover_text(why);
    } else if sw.clicked() {
        input.toggles.push((row.id, !on));
    }
    if let Some(badge) = row.switch.badge() {
        tag(&mut ui, badge, t.warm_note, t.warm_note_text).on_hover_text(row.switch.label());
    }
    tier_tag(&mut ui, row.tier);
    let words = ui.available_rect_before_wrap();
    let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(words).layout(Layout::top_down(Align::Min)));
    ui.spacing_mut().item_spacing = vec2(8.0, 2.0);
    // An add-on that is off is drawn fainter.
    let fade = |c: Color32| if on { c } else { c.gamma_multiply(0.6) };
    let name_ink = fade(if row.selected { t.on_selected } else { t.text });
    let sub_ink = sub_colour(&t, row.selected);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(egui::Label::new(RichText::new(row.name).font(egui::FontId::new(13.0, theme::semibold())).color(name_ink)).truncate());
        for (need, hint) in row.marks() {
            let (r, resp) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
            need_icon(ui.painter(), r.center(), need);
            resp.on_hover_text(hint);
        }
    });
    ui.add(egui::Label::new(RichText::new(row.description).size(12.0).color(sub_ink)).truncate());
}

/// The core pages in one block, always on.
fn core_block(ui: &mut Ui, core: &[&str]) {
    let t = theme::of_ui(ui);
    panel(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Core, always on").font(t.fonts.group_heading.clone()).color(t.text));
            ui.label(RichText::new(core.len().to_string()).color(t.faint));
        });
        ui.add(egui::Label::new(RichText::new(core.join(", ")).size(12.5).color(t.label)).wrap());
    });
}

/// The side panel: the selected add-on in full, with what it needs, what it
/// uses and what can be done with it.
fn addon_panel(ui: &mut Ui, d: &Detail, input: &mut AddonInput) {
    let t = theme::of_ui(ui);
    ui.spacing_mut().item_spacing.y = 8.0;
    ui.label(RichText::new(d.name).font(egui::FontId::new(19.0, theme::display())).color(t.text));
    ui.horizontal(|ui| {
        tier_tag(ui, d.tier);
        ui.label(RichText::new(format!("Add-on \u{b7} {}", d.switch.label().to_lowercase())).font(t.fonts.label.clone()).color(t.label));
    });
    ui.add(egui::Label::new(RichText::new(d.description).color(t.soft)).wrap());
    if let Some(note) = d.note {
        warm_note(ui, note);
    }
    let mut names = vec!["Needs"];
    names.extend(d.consumption.iter().map(|(what, _)| *what));
    facts(ui, &names, |ui, i| {
        ui.spacing_mut().item_spacing = vec2(6.0, 4.0);
        if i == 0 {
            ui.vertical(|ui| {
                for (k, text) in d.requirements.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        match d.needs.get(k) {
                            Some(need) => need_icon(ui.painter(), r.center(), *need),
                            None => tick(ui.painter(), r.center()),
                        }
                        ui.add(egui::Label::new(RichText::new(text).color(t.text)).wrap());
                    });
                }
            });
            return;
        }
        let (what, value) = d.consumption[i - 1];
        if what != "When" {
            let (r, _) = ui.allocate_exact_size(vec2(8.0, 14.0), Sense::hover());
            ui.painter().circle_filled(r.center(), 4.0, intensity_colour(&t, intensity(value)));
        }
        ui.add(egui::Label::new(RichText::new(value).color(t.text)).wrap());
    });
    ui.add_space(6.0);
    match d.switch.hint() {
        Some(why) => warm_note(ui, &format!("{} {why}", d.switch.label())),
        None => {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for &act in &d.actions {
                    let clicked = match act {
                        Act::TurnOn => primary_button(ui, act.label(), true).clicked(),
                        Act::TurnOff => outline_button(ui, act.label(), true).clicked(),
                        Act::OpenPage => ui.link(RichText::new(act.label()).color(t.link)).clicked(),
                    };
                    if clicked {
                        match act {
                            Act::TurnOn => input.toggles.push((d.id, true)),
                            Act::TurnOff => input.toggles.push((d.id, false)),
                            Act::OpenPage => input.open = Some(Page::of(d.id)),
                        }
                    }
                }
            });
        }
    }
}

impl TaskApp {
    /// Settings › General: the band, the list of categories (the app's,
    /// then one per add-on that is on, then About & support) and the chosen
    /// category's card; while searching, every card with a matching row.
    fn settings_general(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let before = self.settings.clone();
        let mut speed = self.settings.speed;
        let mut reset = false;
        StatusBand::new(Mark::None, "Every setting applies as soon as you change it")
            .sub("There is nothing to save: settings are saved automatically.")
            .actions_width(170.0)
            .show(ui, |ui, area| {
                if area == BandArea::Actions {
                    reset = outline_button(ui, "Reset to defaults", true).clicked();
                }
            });
        ui.add_space(14.0);
        let addons = self.addons.settings_addons();
        if let SettingsCat::Addon(id) = self.settings_view.cat {
            if !addons.iter().any(|a| a.0 == id) {
                self.settings_view.cat = SettingsCat::General;
            }
        }
        set_settings_query(ui, &self.settings_view.query);
        let searching = settings_searching(ui);
        let mut manage = false;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(CATEGORIES);
                manage = self.settings_categories(ui, &addons, searching);
            });
            ui.add_space(GAP);
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().id_salt("settings-general").auto_shrink([false, false]).show(ui, |ui| {
                    self.settings_cards(ui, &addons, searching, &mut speed, actions);
                });
            });
        });
        if self.settings.hide_microsoft && !before.hide_microsoft {
            self.ask_signatures();
        }
        if manage {
            self.show_addons = true;
        }
        if speed != self.settings.speed {
            self.set_speed(speed);
        }
        if reset {
            // Add-on choices have their own page and are kept, as are
            // fields written by a newer version.
            let addons = std::mem::take(&mut self.settings.addons);
            let unknown = std::mem::take(&mut self.settings.unknown);
            self.settings = Settings::default();
            self.settings.addons = addons;
            self.settings.unknown = unknown;
            self.sampler.interval_ms.store(self.settings.interval_ms(), Ordering::Relaxed);
        }
        if before.dark != self.settings.dark
            || before.start_page != self.settings.start_page
            || before.bits_per_second != self.settings.bits_per_second
            || before.fahrenheit != self.settings.fahrenheit
            || before.always_on_top != self.settings.always_on_top
            || before.speed != self.settings.speed
            || before.highlight_updates != self.settings.highlight_updates
            || before.show_self_in_reports != self.settings.show_self_in_reports
            || before.hide_microsoft != self.settings.hide_microsoft
        {
            self.settings.save();
        }
    }

    /// The category list; returns whether "Manage" (Settings › Add-ons)
    /// was clicked. While searching each entry shows its matches, and a
    /// click shows that category and ends the search.
    fn settings_categories(&mut self, ui: &mut Ui, addons: &[(&'static str, &'static str, &'static str)], searching: bool) -> bool {
        let t = theme::of_ui(ui);
        let view = &mut self.settings_view;
        let mut picked = None;
        let mut entry = |ui: &mut Ui, cat: SettingsCat, name: &str| {
            let count = searching.then(|| view.counts.get(cat.key()).copied().unwrap_or(0));
            let dim = count == Some(0);
            if settings_category(ui, name, count.filter(|n| *n > 0), !searching && view.cat == cat, dim).clicked() {
                picked = Some(cat);
            }
        };
        ui.spacing_mut().item_spacing.y = 2.0;
        category_heading(ui, "App");
        entry(ui, SettingsCat::General, "General");
        entry(ui, SettingsCat::Appearance, "Appearance");
        let mut manage = false;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("Add-ons").font(egui::FontId::proportional(11.5)).color(t.faint));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(12.0);
                manage = link_button(ui, "Manage").clicked();
            });
        });
        for (id, name, _) in addons {
            entry(ui, SettingsCat::Addon(id), name);
        }
        ui.add_space(8.0);
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.x_range(), r.top(), egui::Stroke::new(1.0, t.line));
        ui.add_space(8.0);
        entry(ui, SettingsCat::About, "About & support");
        if let Some(cat) = picked {
            view.cat = cat;
            view.query.clear();
        }
        manage
    }

    /// The chosen category's card, or while searching every card with a
    /// matching row (and a note when none matches).
    fn settings_cards(&mut self, ui: &mut Ui, addons: &[(&'static str, &'static str, &'static str)], searching: bool, speed: &mut usize, actions: &mut Vec<Action>) {
        let cats: Vec<SettingsCat> = if searching {
            [SettingsCat::General, SettingsCat::Appearance].into_iter().chain(addons.iter().map(|a| SettingsCat::Addon(a.0))).collect()
        } else {
            vec![self.settings_view.cat]
        };
        let mut counts = HashMap::new();
        for cat in cats {
            let shown = match cat {
                SettingsCat::General => settings_card(ui, "General", "How the app starts and refreshes.", None, |ui| self.general_rows(ui, speed)),
                SettingsCat::Appearance => settings_card(ui, "Appearance", "The theme and the units figures are shown in.", None, |ui| self.appearance_rows(ui)),
                SettingsCat::Addon(id) => {
                    let (_, name, blurb) = addons.iter().copied().find(|a| a.0 == id).unwrap_or((id, id, ""));
                    let open = format!("Open {name}");
                    let mut page = PageContext::new(&self.settings, &self.snap);
                    let host = &mut self.addons;
                    let shown = settings_card(ui, name, blurb, Some(&open), |ui| {
                        host.settings_section(id, ui, &mut page);
                    });
                    actions.append(&mut page.actions);
                    if shown.open {
                        actions.push(Action::OpenPage(id));
                    }
                    shown
                }
                SettingsCat::About => settings_card(ui, "About & support", "The project lives on donations.", None, about_rows),
            };
            counts.insert(cat.key(), shown.rows);
        }
        if searching {
            if counts.values().all(|n| *n == 0) {
                neutral_note(ui, &format!("No setting matches \u{201c}{}\u{201d}.", self.settings_view.query.trim()));
            }
            if counts != self.settings_view.counts {
                // The category list was drawn with last frame's counts.
                ui.ctx().request_repaint();
            }
            self.settings_view.counts = counts;
        }
    }

    /// Appearance: the theme, then the temperature and network units.
    fn appearance_rows(&mut self, ui: &mut Ui) {
        group_heading(ui, "Look");
        let s = &mut self.settings;
        setting_row(ui, "App theme", None, |ui| {
            if let Some(dark) = segmented(ui, s.dark, &[(false, "Light"), (true, "Dark")]).clicked {
                s.set_dark(dark);
            }
        });
        group_heading(ui, "Units");
        setting_row(ui, "Temperature", None, |ui| {
            if let Some(f) = segmented(ui, s.fahrenheit, &[(false, "Celsius (\u{b0}C)"), (true, "Fahrenheit (\u{b0}F)")]).clicked {
                s.fahrenheit = f;
            }
        });
        setting_row(ui, "Network speed", Some("Bits per second (Mbps) or bytes per second (MB/sec)"), |ui| {
            if let Some(bits) = segmented(ui, s.bits_per_second, &[(false, "MB/sec"), (true, "Mbps")]).clicked {
                s.bits_per_second = bits;
            }
        });
    }

    /// General: update speed and start page, the Processes switches, then
    /// the window.
    fn general_rows(&mut self, ui: &mut Ui, speed: &mut usize) {
        group_heading(ui, "Start and refresh");
        let current = SPEEDS.get(*speed).map(|s| s.0).unwrap_or("");
        setting_combo(ui, "speed", "Real-time update speed", Some("How often graphs and lists read the PC"), current, |ui| {
            for (i, (label, _)) in SPEEDS.iter().enumerate() {
                ui.selectable_value(speed, i, *label);
            }
        });
        // Saved by id; older versions saved the shown name.
        let start_text = match self.settings.start_page.as_str() {
            crate::addons::LAST_USED => crate::addons::LAST_USED,
            s => page_name(&self.addons, Page::of(self.addons.resolve_page(s))),
        };
        let items = nav_items(&self.addons);
        let start = &mut self.settings.start_page;
        setting_combo(ui, "start", "Default start page", Some("The page shown when the app opens"), start_text, |ui| {
            let last = crate::addons::LAST_USED;
            ui.selectable_value(start, last.to_string(), last);
            for item in &items {
                ui.selectable_value(start, item.page.id().to_string(), item.name);
            }
        });
        group_heading(ui, "Processes");
        let s = &mut self.settings;
        setting_switch(ui, "Highlight Process Updates", None, &mut s.highlight_updates);
        setting_switch(ui, "Show Task Manager in Reports", None, &mut s.show_self_in_reports);
        setting_switch(ui, "Hide Microsoft Services and Processes", Some(HIDE_MICROSOFT_NOTE), &mut s.hide_microsoft);
        setting_switch(ui, "Ask before applying Efficiency mode", None, &mut s.ask_efficiency);
        group_heading(ui, "Window");
        setting_switch(ui, "Always on top", Some("Keep the app above other windows"), &mut self.settings.always_on_top);
    }

    /// Colors: whether the graph colours are the built-in ones, each graph's
    /// built-in and saved colour with "Use default", and the CPU graph in
    /// the saved colour.
    pub fn page_colors(&mut self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        ui.horizontal(|ui| {
            ui.set_min_height(52.0);
            self.page_title(ui, "Colors");
        });
        ui.add_space(6.0);
        let before: Vec<[u8; 3]> = GraphColor::ALL.iter().map(|c| c.get(&self.settings)).collect();
        let all_original = GraphColor::ALL.iter().all(|c| c.is_original(&self.settings));
        let (mark, headline) = if all_original {
            (Mark::Tick, "Your graph colours are the default palette")
        } else {
            (Mark::Dot(t.amber), "Your graph colours differ from the default palette")
        };
        let mut everywhere = false;
        StatusBand::new(mark, headline)
            .sub("Every device has one fixed default colour. \u{201c}Use default\u{201d} puts a graph's back.")
            .actions_width(300.0)
            .show(ui, |ui, area| {
                if area == BandArea::Actions {
                    everywhere = primary_button(ui, "Use the default palette everywhere", !all_original).clicked();
                }
            });
        ui.add_space(14.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            let list_w = (ui.available_width() - PANEL - GAP).max(420.0);
            ui.vertical(|ui| {
                ui.set_width(list_w);
                self.palette_list(ui);
            });
            ui.add_space(GAP);
            ui.vertical(|ui| {
                ui.set_width(PANEL);
                ui.set_height(height);
                self.cpu_graph_panel(ui);
            });
        });
        if everywhere {
            for c in GraphColor::ALL {
                c.use_original(&mut self.settings);
            }
        }
        if GraphColor::ALL.iter().zip(&before).any(|(c, b)| c.get(&self.settings) != *b) {
            self.settings.save();
        }
    }

    /// The palette: per graph, the built-in colour, the colour saved
    /// here (with its picker) and "Use default".
    fn palette_list(&mut self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            ui.label(RichText::new("The default palette").font(egui::FontId::new(14.0, theme::semibold())).color(t.text));
        });
        ui.add_space(4.0);
        let swatch = |ui: &mut Ui, c: [u8; 3]| {
            let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
            ui.painter().rect_filled(r, 4.0, rgb(c));
            ui.painter().rect_stroke(r, 4.0, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
        };
        let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        let grid = egui::Grid::new("colors").num_columns(4).striped(false).spacing(vec2(24.0, 12.0)).min_col_width(130.0);
        let top = ui.cursor().top();
        grid.show(ui, |ui| {
            for head in ["Graph", "Default", "Saved here now", ""] {
                ui.label(RichText::new(head).font(t.fonts.label.clone()).color(t.label));
            }
            ui.end_row();
            for c in GraphColor::ALL {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(c.name()).font(t.fonts.row.clone()).color(t.text));
                    if !c.sub().is_empty() {
                        ui.label(RichText::new(c.sub()).size(11.5).color(t.faint));
                    }
                });
                ui.horizontal(|ui| {
                    swatch(ui, c.original());
                    ui.label(RichText::new(hex(c.original())).color(t.soft));
                });
                let original = c.is_original(&self.settings);
                ui.horizontal(|ui| {
                    ui.color_edit_button_srgb(c.get_mut(&mut self.settings)).on_hover_text("Pick your own colour");
                    let saved = c.get(&self.settings);
                    if original {
                        ui.label(RichText::new("same").color(t.faint));
                    } else {
                        ui.label(RichText::new(hex(saved)).color(t.amber));
                    }
                });
                if outline_button(ui, "Use default", !original).clicked() {
                    c.use_original(&mut self.settings);
                }
                ui.end_row();
            }
        });
        // The line under the headings.
        let rule = top + 24.0;
        let r = ui.min_rect();
        ui.painter().hline(r.left()..=r.right(), rule, egui::Stroke::new(1.0, t.line));
        ui.add_space(10.0);
        neutral_note(ui, "You can pick your own colour for a graph from its row.");
    }

    /// The side panel: the CPU graph drawn in the colour saved here.
    fn cpu_graph_panel(&mut self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        let saved = GraphColor::Cpu.get(&self.settings);
        let original = GraphColor::Cpu.original();
        let data: Vec<f32> = self.cpu_hist.iter().copied().collect();
        let mut reset = false;
        SidePanel::new("colors-cpu").show(ui, Some(()), |ui, ()| {
            ui.label(RichText::new("CPU graph").font(t.fonts.figure.clone()).color(t.text));
            ui.label(RichText::new("In the colour saved here").font(t.fonts.label.clone()).color(t.label));
            let w = ui.available_width();
            // Graphs stay dark in both modes, as on Performance.
            graph(ui, vec2(w, 120.0), &data, 100.0, rgb(saved));
            let caption = if saved == original { format!("Saved here: {} (the default)", hex(saved)) } else { format!("Saved here: {} \u{b7} default {}", hex(saved), hex(original)) };
            ui.label(RichText::new(caption).font(t.fonts.label.clone()).color(t.label));
            ui.add_space(6.0);
            reset = outline_button(ui, &format!("Use default {}", hex(original)), saved != original).clicked();
        });
        if reset {
            GraphColor::Cpu.use_original(&mut self.settings);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui::{context, run, texts};
    use crate::theme::Mode;
    use egui::{Event, PointerButton, Pos2};

    /// Where the text `word` was drawn in `out`.
    fn place(out: &egui::FullOutput, word: &str) -> Pos2 {
        crate::test_ui::flat_shapes(out)
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == word => Some(t.pos + t.galley.rect.center().to_vec2()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{word:?} is drawn"))
    }

    /// Clicks the button showing `word` and returns the link it asked for.
    fn click(word: &str) -> Option<&'static str> {
        let ctx = context(Mode::Dark);
        let size = (1200.0, 200.0);
        let out = run(&ctx, size, vec![], |ui| {
            project_links(ui);
        });
        let at = place(&out, word);
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        run(&ctx, size, vec![Event::PointerMoved(at), press(true)], |ui| {
            project_links(ui);
        });
        let mut got = None;
        run(&ctx, size, vec![press(false)], |ui| got = project_links(ui));
        got
    }

    /// The words Settings › General draws for `app`.
    fn settings_words(app: &mut TaskApp) -> Vec<String> {
        let ctx = context(Mode::Dark);
        let mut actions = Vec::new();
        let mut words = Vec::new();
        // Twice: the category list shows last frame's search counts.
        for _ in 0..2 {
            let out = run(&ctx, (1335.0, 1400.0), vec![], |ui| app.page_settings(ui, &mut actions));
            words = texts(&out);
        }
        words
    }

    #[test]
    fn settings_shows_the_chosen_category_and_every_add_on_that_is_on_in_the_list() {
        let mut app = TaskApp::headless(Settings::default(), Default::default());
        let words = settings_words(&mut app);
        for w in ["App", "General", "Appearance", "Add-ons", "Manage", "About & support", "Real-time update speed", "Always on top"] {
            assert!(words.iter().any(|x| x == w), "{w:?} in {words:?}");
        }
        for (_, name, _) in app.addons.settings_addons() {
            assert!(words.iter().any(|x| x == name), "{name} is listed");
        }
        assert!(!words.iter().any(|w| w == "App theme"), "Appearance is not shown with General");
        app.settings_view.cat = SettingsCat::Addon("disk-space");
        let words = settings_words(&mut app);
        assert!(words.iter().any(|w| w == "Open Disk Space") && words.iter().any(|w| w == "History folder"), "{words:?}");
        assert!(!words.iter().any(|w| w == "Real-time update speed"));
    }

    #[test]
    fn a_search_shows_every_matching_setting_across_categories_or_says_none_matches() {
        let mut app = TaskApp::headless(Settings::default(), Default::default());
        app.settings_view.query = "summary".into();
        let words = settings_words(&mut app);
        let tiles = words.iter().filter(|w| *w == "Show a tile on Summary").count();
        assert!(tiles >= 2, "tiles of several add-ons: {words:?}");
        assert!(!words.iter().any(|w| w == "Always on top"));
        assert_eq!(app.settings_view.counts.get("general"), Some(&0));
        app.settings_view.query = "no such setting".into();
        let words = settings_words(&mut app);
        assert!(words.iter().any(|w| w == "No setting matches \u{201c}no such setting\u{201d}."), "{words:?}");
    }

    #[test]
    fn settings_offers_the_repository_and_a_donation_link() {
        let ctx = context(Mode::Light);
        let out = run(&ctx, (1200.0, 200.0), vec![], |ui| {
            project_links(ui);
        });
        let words = texts(&out);
        assert!(words.iter().any(|w| w == "GitHub"), "{words:?}");
        assert!(words.iter().any(|w| w == "Buy me a coffee"), "{words:?}");
        assert_eq!(click("GitHub"), Some("https://github.com/SpatariuRares/ultimate-pc-manager"));
        assert_eq!(click("Buy me a coffee"), Some("https://buymeacoffee.com/spatariurares"));
    }

    #[test]
    fn settings_general_has_the_switches_of_processes() {
        let mut app = TaskApp::headless(Settings::default(), crate::sampler::Snapshot::default());
        let words = settings_words(&mut app);
        for w in ["Highlight Process Updates", "Show Task Manager in Reports", "Hide Microsoft Services and Processes", "Ask before applying Efficiency mode"] {
            assert!(words.iter().any(|x| x == w), "{w:?} in {words:?}");
        }
    }
}
