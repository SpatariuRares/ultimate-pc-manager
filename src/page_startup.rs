//! Startup apps: the entries that start with Windows, with Enable / Disable.
//! The page owns its list and its selection.

use egui::{Align, Layout, Sense, Ui};
use egui_extras::{Column, TableBuilder};

use crate::addons::Core;
use crate::availability::{is_available, unavailable_text, Feature};
use crate::core_page::{page_title, CorePage, PageCtx};
use crate::list_pages::{fact_values, panel_title, summary_band, GAP};
use crate::platform::Action;
use crate::theme;
use crate::system_lists::{StartupRow, SystemLists};
use crate::widgets::parts::{
    self, cell_text, column_heading, design_table, facts, list_with_panel, CellInk, Mark, SidePanel, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT,
};
use crate::widgets::{first_path, refresh_button};

#[derive(Default)]
pub struct StartupPage {
    /// The list as last read; `None` until the page is first shown.
    list: Option<Vec<StartupRow>>,
    /// The chosen entry, by position in the list.
    selected: Option<usize>,
}

impl StartupPage {
    /// The one place the startup list is read: every reader (first draw,
    /// Refresh, F5, the sidebar, an Enable or Disable) comes here.
    pub fn reload(&mut self, system: &dyn SystemLists) {
        // A system without startup entries has nothing to read; the page says so.
        self.list = Some(if is_available(Feature::StartupEntries) { system.list_startup() } else { Vec::new() });
    }

    /// Whether the list has been read yet.
    pub fn is_loaded(&self) -> bool {
        self.list.is_some()
    }

    /// The name of the entry `id` in the list as last read.
    pub fn name_of(&self, id: &str) -> Option<String> {
        self.list.as_ref()?.iter().find(|r| r.id == id).map(|r| r.name.clone())
    }
}

impl CorePage for StartupPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        self.reload(ctx.system);
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        if self.list.is_none() {
            self.reload(ctx.system);
        }
        let list = self.list.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            page_title(ui, Core::Startup.id(), "Startup apps");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if refresh_button(ui).clicked() {
                    self.reload(ctx.system);
                }
                if let Some(i) = self.selected.filter(|i| *i < list.len()) {
                    let en = list[i].enabled;
                    if parts::header_button(ui, if en { "Disable" } else { "Enable" }, true).clicked() {
                        actions.push(Action::Startup(list[i].id.clone(), !en));
                    }
                }
            });
        });
        ui.add_space(GAP);
        if let Some(why) = unavailable_text(Feature::StartupEntries) {
            let t = theme::of_ui(ui);
            summary_band(ui, Mark::Dot(t.red), "The startup apps could not be read", &why);
            return;
        }
        let off = list.iter().filter(|r| !r.enabled).count();
        let sub = format!("{off} of them are turned off. From the Run keys and Startup folders, read when the page opens and on Refresh.");
        summary_band(ui, Mark::None, &startup_headline(&list), &sub);
        let selected = self.selected;
        let mut panel_action = None;
        let new_sel = list_with_panel(
            ui,
            |ui, _| {
                let mut new_sel = None;
                design_table(ui, |ui| {
                    TableBuilder::new(ui)
                        .resizable(true)
                        .sense(Sense::click())
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .column(Column::initial(220.0).clip(true))
                        .column(Column::initial(90.0))
                        .column(Column::initial(170.0).clip(true))
                        .column(Column::remainder().clip(true))
                        .header(TABLE_HEADER_HEIGHT, |mut h| {
                            for t in ["Name", "Status", "Location", "Command"] {
                                h.col(|ui| column_heading(ui, t));
                            }
                        })
                        .body(|body| {
                            body.rows(TABLE_ROW_HEIGHT, list.len(), |mut row| {
                                let i = row.index();
                                let r = &list[i];
                                let on = selected == Some(i);
                                row.set_selected(on);
                                row.col(|ui| {
                                    cell_text(ui, &r.name, CellInk::Main, on);
                                });
                                row.col(|ui| {
                                    let (words, ink) = if r.enabled { ("Enabled", CellInk::Main) } else { ("Disabled", CellInk::Faint) };
                                    cell_text(ui, words, ink, on);
                                });
                                row.col(|ui| {
                                    cell_text(ui, &r.location, CellInk::Soft, on);
                                });
                                row.col(|ui| {
                                    cell_text(ui, &r.command, CellInk::Soft, on);
                                });
                                let resp = row.response();
                                if resp.clicked() || resp.secondary_clicked() {
                                    new_sel = Some(i);
                                }
                                resp.context_menu(|ui| {
                                    if ui.button(if r.enabled { "Disable" } else { "Enable" }).clicked() {
                                        actions.push(Action::Startup(r.id.clone(), !r.enabled));
                                        ui.close();
                                    }
                                    if ui.button("Open file location").clicked() {
                                        actions.push(Action::OpenLocation(first_path(&r.command)));
                                        ui.close();
                                    }
                                    if ui.button("Search online").clicked() {
                                        actions.push(Action::SearchOnline(r.name.clone()));
                                        ui.close();
                                    }
                                });
                            });
                        });
                });
                new_sel
            },
            |ui, new_sel| {
                let i = new_sel.or(selected).filter(|i| *i < list.len());
                SidePanel::new("startup-detail").show(ui, i.map(|i| (i, &list[i])), |ui, (_, r)| {
                    if startup_detail(ui, r) {
                        panel_action = Some(Action::Startup(r.id.clone(), !r.enabled));
                    }
                });
            },
        );
        actions.extend(panel_action);
        if new_sel.is_some() {
            self.selected = new_sel;
        }
    }
}

/// Startup apps' side panel: the chosen app's fields (as the list shows
/// them) and Enable / Disable. True when that was clicked.
fn startup_detail(ui: &mut Ui, r: &StartupRow) -> bool {
    panel_title(ui, &r.name);
    let values = [if r.enabled { "Enabled" } else { "Disabled" }, r.location.as_str(), r.command.as_str()];
    facts(ui, &["Status", "Location", "Command"], |ui, i| fact_values(ui, &values, i));
    ui.add_space(8.0);
    parts::outline_button(ui, if r.enabled { "Disable" } else { "Enable" }, true).clicked()
}

/// "9 of 32 startup apps are enabled".
fn startup_headline(rows: &[StartupRow]) -> String {
    let on = rows.iter().filter(|r| r.enabled).count();
    let verb = if on == 1 { "is" } else { "are" };
    format!("{on} of {} startup apps {verb} enabled", rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_pages::testing::{click, has, texts, Env};
    use crate::theme;
    use crate::widgets::parts::EMPTY_DETAIL;

    fn startup(enabled: bool) -> StartupRow {
        StartupRow { enabled, ..Default::default() }
    }

    fn entry(id: &str, name: &str, enabled: bool) -> StartupRow {
        StartupRow { id: id.into(), name: name.into(), enabled, ..Default::default() }
    }

    fn ui_ctx() -> egui::Context {
        crate::test_ui::context(theme::Mode::Dark)
    }

    fn words(draw: impl FnMut(&mut Ui)) -> Vec<String> {
        crate::test_ui::texts(&crate::test_ui::run(&ui_ctx(), (1300.0, 800.0), vec![], draw))
    }

    #[test]
    fn the_startup_panel_holds_the_rows_fields() {
        let r = StartupRow { name: "Steam".into(), command: "steam.exe -silent".into(), location: "HKCU Run".into(), enabled: false, ..Default::default() };
        let shown = words(|ui| {
            startup_detail(ui, &r);
        });
        for w in ["Steam", "Status", "Disabled", "Location", "HKCU Run", "Command", "steam.exe -silent", "Enable"] {
            assert!(has(&shown, w), "{w} in {shown:?}");
        }
    }

    #[test]
    fn the_startup_side_panel_shows_the_chosen_app_and_enable_or_disable() {
        let env = Env::new();
        let steam = StartupRow { name: "Steam".into(), command: "steam.exe -silent".into(), location: "HKCU Run".into(), enabled: true, ..Default::default() };
        env.system.script_startup(vec![steam, startup(false)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        let none = texts(&ui, &mut page, &env.ctx());
        assert!(has(&none, EMPTY_DETAIL), "{none:?}");
        click(&ui, &mut page, &env.ctx(), "Steam");
        let chosen = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&chosen, EMPTY_DETAIL));
        assert_eq!(chosen.iter().filter(|w| *w == "HKCU Run").count(), 2, "the location in the list and in the panel");
        assert_eq!(chosen.iter().filter(|w| *w == "Disable").count(), 2, "Disable in the header and in the panel");
    }

    #[test]
    fn an_unavailable_startup_list_shows_the_unavailable_state_and_reads_nothing() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        let _unavailable = crate::availability::unavailable_in_test(&[Feature::StartupEntries]);
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(has(&shown, "Startup apps"), "the page stays: {shown:?}");
        assert!(has(&shown, "Not available on this system."), "{shown:?}");
        assert!(!has(&shown, "Steam") && !shown.iter().any(|w| w.contains("startup apps are enabled")), "not an empty or a real list: {shown:?}");
        assert_eq!(env.system.startup_reads(), 0, "nothing is read");
    }

    #[test]
    fn the_startup_band_counts_the_enabled_apps() {
        assert_eq!(startup_headline(&[startup(true), startup(false), startup(true)]), "2 of 3 startup apps are enabled");
        assert_eq!(startup_headline(&[startup(true), startup(false)]), "1 of 2 startup apps is enabled");
    }

    #[test]
    fn the_first_draw_of_startup_reads_the_list_once_and_keeps_the_headline() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true), entry("b", "Discord", false), entry("c", "Zoom", true)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        let first = texts(&ui, &mut page, &env.ctx());
        let second = texts(&ui, &mut page, &env.ctx());
        assert_eq!(env.system.startup_reads(), 1, "drawn twice, read once");
        assert!(has(&first, "2 of 3 startup apps are enabled"), "{first:?}");
        assert!(has(&second, "Discord"), "{second:?}");
    }

    #[test]
    fn a_refresh_asked_of_the_page_rereads_the_list() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        texts(&ui, &mut page, &env.ctx());
        env.system.script_startup(vec![entry("a", "Steam", true), entry("b", "Discord", true)]);
        page.refresh(&env.ctx());
        assert_eq!(env.system.startup_reads(), 2);
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(has(&shown, "2 of 2 startup apps are enabled"), "{shown:?}");
    }

    #[test]
    fn refresh_on_startup_rereads_the_list() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        texts(&ui, &mut page, &env.ctx());
        click(&ui, &mut page, &env.ctx(), "Refresh");
        assert_eq!(env.system.startup_reads(), 2, "the first draw and the click");
    }

    #[test]
    fn the_startup_header_action_names_the_chosen_entry_not_its_position() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true), entry("b", "Discord", true)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        click(&ui, &mut page, &env.ctx(), "Discord");
        let actions = click(&ui, &mut page, &env.ctx(), "Disable");
        assert_eq!(actions, vec![Action::Startup("b".into(), false)]);
    }

    #[test]
    fn the_chosen_entry_stays_chosen_while_the_page_is_hidden() {
        let env = Env::new();
        env.system.script_startup(vec![entry("a", "Steam", true), entry("b", "Discord", false)]);
        let mut page = StartupPage::default();
        let ui = ui_ctx();
        click(&ui, &mut page, &env.ctx(), "Discord");
        // Another page is shown for a while: this one is not drawn.
        let back = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&back, EMPTY_DETAIL), "{back:?}");
        assert_eq!(back.iter().filter(|w| *w == "Enable").count(), 2, "the actions of the chosen entry");
    }

    #[test]
    fn the_page_names_an_entry_by_its_id_from_the_list_it_last_read() {
        let env = Env::new();
        env.system.script_startup(vec![entry("run|Steam", "Steam", true)]);
        let mut page = StartupPage::default();
        assert_eq!(page.name_of("run|Steam"), None, "nothing is read yet");
        texts(&ui_ctx(), &mut page, &env.ctx());
        assert_eq!(page.name_of("run|Steam").as_deref(), Some("Steam"));
        assert_eq!(page.name_of("run|Gone"), None);
    }
}
