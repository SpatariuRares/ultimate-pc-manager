//! Settings and Settings › Add-ons, in Design A (boards P16 and P18).
//! Colours come only from the theme.

use std::collections::HashMap;

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Ui};

use crate::addons::overview::{intensity, Act, Detail, Intensity, Overview, Row, Switch};
use crate::addons::{AddonHost, Core};
use crate::app::{nav_items, page_name, Page};
use crate::core_page::{page_title, CorePage, PageCtx};
use crate::platform::{Action, PageContext};
use crate::settings::{Settings, SPEEDS};
use crate::theme::{self, Theme};
use crate::widgets::parts::{
    category_heading, facts, group_heading, link_button, neutral_note, outline_button, panel, primary_button, row_fill, segmented,
    set_settings_query, setting_combo, setting_row, setting_switch, settings_card, settings_category, settings_searching, sub_colour, switch,
    tag, warm_note, BandArea, Mark, SidePanel, StatusBand,
};
use crate::widgets::{need_icon, tick};

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

/// Settings: the category, the search, how many rows each card matched
/// last frame (for the category list), and whether Settings › Add-ons is
/// open with which add-on selected.
#[derive(Default)]
pub struct SettingsPage {
    cat: SettingsCat,
    query: String,
    counts: HashMap<&'static str, usize>,
    show_addons: bool,
    addon_selected: Option<&'static str>,
}

impl SettingsPage {
    /// Whether Settings › Add-ons is open (the app keeps its status line
    /// in the status bar while it is).
    pub fn showing_addons(&self) -> bool {
        self.show_addons
    }

    /// Opens Settings › Add-ons with `selected` chosen in its list.
    pub fn open_addons(&mut self, selected: Option<&'static str>) {
        self.show_addons = true;
        self.addon_selected = selected;
    }
}

impl CorePage for SettingsPage {
    /// Without the add-on host: the app's own categories only.
    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        self.show(ui, ctx, None, actions);
    }

    fn draw_with_addons(&mut self, ui: &mut Ui, ctx: &PageCtx, host: &mut AddonHost, actions: &mut Vec<Action>) {
        self.show(ui, ctx, Some(host), actions);
    }
}

/// Width of the side panel beside a list.
const PANEL: f32 = 340.0;
/// Room between a list and its side panel.
const GAP: f32 = 16.0;

/// Under "Hide Microsoft Services and Processes": what the switch hides.
const HIDE_MICROSOFT_NOTE: &str = "Hides services and processes whose program file carries a verified Microsoft Windows signature.";

impl SettingsPage {
    /// Settings, or Settings › Add-ons while it is open (which needs the
    /// add-on host).
    fn show(&mut self, ui: &mut Ui, ctx: &PageCtx, host: Option<&mut AddonHost>, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        let host = match host {
            Some(host) if self.show_addons => {
            // Settings › Add-ons: "Settings" leads back.
            let mut back = false;
            ui.horizontal(|ui| {
                ui.set_min_height(52.0);
                let (r, _) = ui.allocate_exact_size(vec2(26.0, 40.0), Sense::hover());
                crate::core_page::page_icon(ui.painter(), r.center(), Core::Settings.id(), t.label);
                let link = egui::Label::new(RichText::new("Settings").font(t.fonts.row.clone()).color(t.label)).sense(Sense::click());
                back = ui.add(link).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
                crate::widgets::crumb_separator(ui);
                ui.label(RichText::new("Add-ons").font(t.fonts.page_title.clone()).color(t.text));
            });
            if back {
                self.show_addons = false;
            }
            ui.add_space(6.0);
                self.settings_addons(ui, host, actions);
                return;
            }
            host => host,
        };
        ui.horizontal(|ui| {
            ui.set_min_height(52.0);
            page_title(ui, Core::Settings.id(), "Settings");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                crate::widgets::search_field(ui, &mut self.query, "Find a setting", 260.0);
            });
        });
        ui.add_space(6.0);
        self.settings_general(ui, ctx, host, actions);
    }

    /// Settings › Add-ons: the band, the always-on core, the list with one
    /// switch per add-on, and the selected add-on in a side panel. Its
    /// status line goes to the app's status bar.
    fn settings_addons(&mut self, ui: &mut Ui, host: &mut AddonHost, actions: &mut Vec<Action>) {
        let view = Overview::of(host, self.addon_selected);
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
        actions.extend(input.toggles.into_iter().map(|(id, on)| Action::SetAddon(id, on)));
        if let Some(id) = input.open {
            actions.push(Action::OpenPage(id));
        }
    }
}

/// What the user did on Settings › Add-ons this frame, applied after drawing.
#[derive(Default)]
struct AddonInput {
    toggles: Vec<(&'static str, bool)>,
    picked: Option<&'static str>,
    open: Option<&'static str>,
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
                            Act::OpenPage => input.open = Some(d.id),
                        }
                    }
                }
            });
        }
    }
}

impl SettingsPage {
    /// Settings › General: the band, the list of categories (the app's,
    /// then one per add-on that is on, then About & support) and the chosen
    /// category's card; while searching, every card with a matching row.
    /// What the user changed goes to the app as one `ChangeSettings`.
    fn settings_general(&mut self, ui: &mut Ui, ctx: &PageCtx, mut host: Option<&mut AddonHost>, actions: &mut Vec<Action>) {
        let mut s = ctx.settings.clone();
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
        let addons = host.as_deref_mut().map(|h| h.settings_addons()).unwrap_or_default();
        if let SettingsCat::Addon(id) = self.cat {
            if !addons.iter().any(|a| a.0 == id) {
                self.cat = SettingsCat::General;
            }
        }
        set_settings_query(ui, &self.query);
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
                    self.settings_cards(ui, ctx, &mut s, &addons, searching, host.as_deref_mut(), actions);
                });
            });
        });
        if manage {
            self.show_addons = true;
        }
        if reset {
            // Add-on choices have their own page and are kept, as are
            // fields written by a newer version.
            let addons = std::mem::take(&mut s.addons);
            let unknown = std::mem::take(&mut s.unknown);
            s = Settings::default();
            s.addons = addons;
            s.unknown = unknown;
        }
        if s != *ctx.settings {
            actions.push(Action::ChangeSettings(Box::new(s)));
            ui.ctx().request_repaint();
        }
    }

    /// The category list; returns whether "Manage" (Settings › Add-ons)
    /// was clicked. While searching each entry shows its matches, and a
    /// click shows that category and ends the search.
    fn settings_categories(&mut self, ui: &mut Ui, addons: &[(&'static str, &'static str, &'static str)], searching: bool) -> bool {
        let t = theme::of_ui(ui);
        let (chosen, counts) = (self.cat, &self.counts);
        let mut picked = None;
        let mut entry = |ui: &mut Ui, cat: SettingsCat, name: &str| {
            let count = searching.then(|| counts.get(cat.key()).copied().unwrap_or(0));
            let dim = count == Some(0);
            if settings_category(ui, name, count.filter(|n| *n > 0), !searching && chosen == cat, dim).clicked() {
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
            self.cat = cat;
            self.query.clear();
        }
        manage
    }

    /// The chosen category's card, or while searching every card with a
    /// matching row (and a note when none matches).
    #[allow(clippy::too_many_arguments)]
    fn settings_cards(
        &mut self,
        ui: &mut Ui,
        ctx: &PageCtx,
        s: &mut Settings,
        addons: &[(&'static str, &'static str, &'static str)],
        searching: bool,
        mut host: Option<&mut AddonHost>,
        actions: &mut Vec<Action>,
    ) {
        let cats: Vec<SettingsCat> = if searching {
            [SettingsCat::General, SettingsCat::Appearance].into_iter().chain(addons.iter().map(|a| SettingsCat::Addon(a.0))).collect()
        } else {
            vec![self.cat]
        };
        let mut counts = HashMap::new();
        for cat in cats {
            let shown = match cat {
                SettingsCat::General => settings_card(ui, "General", "How the app starts and refreshes.", None, |ui| general_rows(ui, s, host.as_deref())),
                SettingsCat::Appearance => settings_card(ui, "Appearance", "The theme and the units figures are shown in.", None, |ui| appearance_rows(ui, s)),
                SettingsCat::Addon(id) => {
                    let (_, name, blurb) = addons.iter().copied().find(|a| a.0 == id).unwrap_or((id, id, ""));
                    let open = format!("Open {name}");
                    let mut page = PageContext::new(ctx.settings, ctx.snapshot);
                    let shown = settings_card(ui, name, blurb, Some(&open), |ui| {
                        if let Some(host) = host.as_deref_mut() {
                            host.settings_section(id, ui, &mut page);
                        }
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
                neutral_note(ui, &format!("No setting matches \u{201c}{}\u{201d}.", self.query.trim()));
            }
            if counts != self.counts {
                // The category list was drawn with last frame's counts.
                ui.ctx().request_repaint();
            }
            self.counts = counts;
        }
    }
}

/// Appearance: the theme, then the temperature and network units.
fn appearance_rows(ui: &mut Ui, s: &mut Settings) {
    group_heading(ui, "Look");
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
/// the window. The start page needs the add-on host for the page names.
fn general_rows(ui: &mut Ui, s: &mut Settings, host: Option<&AddonHost>) {
    group_heading(ui, "Start and refresh");
    let current = SPEEDS.get(s.speed).map(|s| s.0).unwrap_or("");
    setting_combo(ui, "speed", "Real-time update speed", Some("How often graphs and lists read the PC"), current, |ui| {
        for (i, (label, _)) in SPEEDS.iter().enumerate() {
            ui.selectable_value(&mut s.speed, i, *label);
        }
    });
    if let Some(host) = host {
        // Saved by id; older versions saved the shown name.
        let start_text = match s.start_page.as_str() {
            crate::addons::LAST_USED => crate::addons::LAST_USED,
            id => page_name(host, Page::of(host.resolve_page(id))),
        };
        let items = nav_items(host);
        let start = &mut s.start_page;
        setting_combo(ui, "start", "Default start page", Some("The page shown when the app opens"), start_text, |ui| {
            let last = crate::addons::LAST_USED;
            ui.selectable_value(start, last.to_string(), last);
            for item in &items {
                ui.selectable_value(start, item.page.id().to_string(), item.name);
            }
        });
    }
    group_heading(ui, "Processes");
    setting_switch(ui, "Highlight Process Updates", None, &mut s.highlight_updates);
    setting_switch(ui, "Show Task Manager in Reports", None, &mut s.show_self_in_reports);
    setting_switch(ui, "Hide Microsoft Services and Processes", Some(HIDE_MICROSOFT_NOTE), &mut s.hide_microsoft);
    setting_switch(ui, "Ask before applying Efficiency mode", None, &mut s.ask_efficiency);
    group_heading(ui, "Window");
    setting_switch(ui, "Always on top", Some("Keep the app above other windows"), &mut s.always_on_top);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    use crate::core_page::testing::{self, place};
    use crate::core_page::{Histories, PerfHistory};
    use crate::sampler::Snapshot;
    use crate::test_ui::{context, run, texts};
    use crate::theme::Mode;
    use egui::{Event, PointerButton};

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

    /// A Settings page over a host with every add-on on, drawn as the app
    /// draws it.
    struct Bench {
        page: SettingsPage,
        host: AddonHost,
        settings: Settings,
        ctx: egui::Context,
    }

    impl Bench {
        fn new() -> Self {
            Bench { page: SettingsPage::default(), host: AddonHost::from_section(&serde_json::json!({})), settings: Settings::default(), ctx: context(Mode::Dark) }
        }

        fn frame(&mut self, events: Vec<Event>) -> (egui::FullOutput, Vec<Action>) {
            let snapshot = Snapshot::default();
            let (cpu, mem) = (VecDeque::new(), VecDeque::new());
            let perf = PerfHistory::new();
            let page_ctx = PageCtx { settings: &self.settings, snapshot: &snapshot, histories: Histories { cpu: &cpu, mem: &mem, perf: &perf } };
            let mut actions = Vec::new();
            let (page, host) = (&mut self.page, &mut self.host);
            let out = run(&self.ctx, (1335.0, 1400.0), events, |ui| page.draw_with_addons(ui, &page_ctx, host, &mut actions));
            (out, actions)
        }

        /// The words Settings draws. Twice: the category list shows last
        /// frame's search counts.
        fn words(&mut self) -> Vec<String> {
            self.frame(vec![]);
            texts(&self.frame(vec![]).0)
        }

        /// Clicks the text `word`; what the frame of the release queued.
        fn click(&mut self, word: &str) -> Vec<Action> {
            self.words();
            let at = place(&self.frame(vec![]).0, word);
            let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
            self.frame(vec![Event::PointerMoved(at), press(true)]);
            self.frame(vec![press(false)]).1
        }
    }

    #[test]
    fn settings_shows_the_chosen_category_and_every_add_on_that_is_on_in_the_list() {
        let mut b = Bench::new();
        let words = b.words();
        for w in ["App", "General", "Appearance", "Add-ons", "Manage", "About & support", "Real-time update speed", "Always on top"] {
            assert!(words.iter().any(|x| x == w), "{w:?} in {words:?}");
        }
        for (_, name, _) in b.host.settings_addons() {
            assert!(words.iter().any(|x| x == name), "{name} is listed");
        }
        assert!(!words.iter().any(|w| w == "App theme"), "Appearance is not shown with General");
        b.page.cat = SettingsCat::Addon("disk-space");
        let words = b.words();
        assert!(words.iter().any(|w| w == "Open Disk Space") && words.iter().any(|w| w == "History folder"), "{words:?}");
        assert!(!words.iter().any(|w| w == "Real-time update speed"));
    }

    #[test]
    fn a_search_shows_every_matching_setting_across_categories_or_says_none_matches() {
        let mut b = Bench::new();
        b.page.query = "summary".into();
        let words = b.words();
        let tiles = words.iter().filter(|w| *w == "Show a tile on Summary").count();
        assert!(tiles >= 2, "tiles of several add-ons: {words:?}");
        assert!(!words.iter().any(|w| w == "Always on top"));
        assert_eq!(b.page.counts.get("general"), Some(&0));
        b.page.query = "no such setting".into();
        let words = b.words();
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
        let words = Bench::new().words();
        for w in ["Highlight Process Updates", "Show Task Manager in Reports", "Hide Microsoft Services and Processes", "Ask before applying Efficiency mode"] {
            assert!(words.iter().any(|x| x == w), "{w:?} in {words:?}");
        }
    }
    #[test]
    fn a_setting_changed_is_asked_of_the_app_as_the_whole_new_settings() {
        let mut b = Bench::new();
        b.settings.dark = true;
        b.click("Appearance");
        let mut expected = b.settings.clone();
        expected.set_dark(false);
        assert_eq!(b.click("Light"), [Action::ChangeSettings(Box::new(expected))]);
    }

    #[test]
    fn a_frame_with_nothing_changed_asks_nothing() {
        let mut b = Bench::new();
        b.words();
        assert!(b.frame(vec![]).1.is_empty());
    }

    #[test]
    fn reset_to_defaults_asks_for_the_defaults_and_keeps_the_add_on_choices() {
        let mut b = Bench::new();
        b.settings.fahrenheit = true;
        b.settings.addons = serde_json::json!({"disk-space": {"on": false}});
        let mut expected = Settings::default();
        expected.addons = b.settings.addons.clone();
        assert_eq!(b.click("Reset to defaults"), [Action::ChangeSettings(Box::new(expected))]);
    }

    #[test]
    fn manage_opens_add_ons_and_the_settings_crumb_leads_back() {
        let mut b = Bench::new();
        assert!(!b.page.showing_addons());
        b.click("Manage");
        assert!(b.page.showing_addons());
        let words = b.words();
        assert!(words.iter().any(|w| w == "Core, always on"), "{words:?}");
        assert!(!words.iter().any(|w| w == "Real-time update speed"));
        b.click("Settings");
        assert!(!b.page.showing_addons());
    }

    #[test]
    fn a_page_opened_on_add_ons_draws_them_without_a_click() {
        let mut b = Bench::new();
        b.page.open_addons(Some("disk-space"));
        let words = b.words();
        assert!(words.iter().any(|w| w == "Core, always on"), "{words:?}");
    }

    #[test]
    fn without_the_add_on_host_settings_draws_the_apps_own_categories() {
        let drawn = testing::draw(&mut SettingsPage::default(), &Settings::default(), &Snapshot::default());
        for w in ["General", "Appearance", "About & support", "Real-time update speed"] {
            assert!(drawn.texts.iter().any(|t| t == w), "{w:?} in {:?}", drawn.texts);
        }
        assert!(drawn.actions.is_empty());
    }
}
