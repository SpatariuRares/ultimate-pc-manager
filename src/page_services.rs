//! Services: the list of Windows services with Start / Stop / Restart. The
//! page owns its list, its selection and its filter.

use egui::{Align, Layout, Sense, Ui};
use egui_extras::{Column, TableBuilder};

use crate::addons::Core;
use crate::availability::{is_available, Feature};
use crate::core_page::{page_title, CorePage, PageCtx};
use crate::list_pages::{fact_values, panel_title, summary_band, GAP};
use crate::platform::Action;
use crate::system_lists::{ServiceKey, ServiceRow, SystemLists};
use crate::theme;
use crate::widgets::parts::{
    self, cell_text, column_heading, design_table, facts, list_with_panel, CellInk, Mark, SidePanel, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT,
};
use crate::widgets::refresh_button;

/// What Services can do to a service: the button's words and the action.
const SERVICE_ACTIONS: [(&str, &str); 3] = [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")];

/// Turning a service on or off at boot, offered for the services of the
/// system's service managers (not the Windows ones) by their start type:
/// the one that changes it.
fn boot_action(s: &ServiceRow) -> Option<(&'static str, &'static str)> {
    if s.source.is_empty() {
        return None;
    }
    match s.start_type.as_str() {
        "Enabled" => Some(("Disable at boot", "disable")),
        "Disabled" => Some(("Enable at boot", "enable")),
        _ => None,
    }
}

/// Is `row` the service `key` names? A key made of a name alone (from Go to
/// service(s)) names the rows of that name, whatever their manager and scope.
fn names(key: &ServiceKey, row: &ServiceRow) -> bool {
    key.name == row.name && (key.source.is_empty() || key.source == row.source) && (key.scope.is_empty() || key.scope == row.scope)
}

#[derive(Default)]
pub struct ServicesPage {
    /// The list as last read; `None` until the page is first shown.
    list: Option<Result<Vec<ServiceRow>, String>>,
    /// The chosen service.
    selected: Option<ServiceKey>,
    /// The filter text.
    search: String,
}

impl ServicesPage {
    /// The one place the services list is read: every reader (first draw,
    /// Refresh, F5, the sidebar, Go to services, a control) comes here.
    pub fn reload(&mut self, system: &dyn SystemLists) {
        self.list = Some(system.list_services());
    }

    /// Whether the list has been read yet.
    pub fn is_loaded(&self) -> bool {
        self.list.is_some()
    }

    /// Go to service(s): the page filtered to `search` with `selected`
    /// chosen.
    pub fn show(&mut self, selected: Option<String>, search: String) {
        self.selected = selected.map(|name| ServiceKey::from(name.as_str()));
        self.search = search;
    }
}

impl CorePage for ServicesPage {
    fn refresh(&mut self, ctx: &PageCtx) {
        self.reload(ctx.system);
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        if self.list.is_none() {
            self.reload(ctx.system);
        }
        ui.horizontal(|ui| {
            page_title(ui, Core::Services.id(), "Services");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if refresh_button(ui).clicked() {
                    self.reload(ctx.system);
                }
                // The chosen row's own key, so a system and a user service of
                // one name are told apart.
                let sel = self.selected.as_ref().map(|k| match self.list.as_ref() {
                    Some(Ok(list)) => list.iter().find(|s| names(k, s)).map(ServiceRow::key).unwrap_or_else(|| k.clone()),
                    _ => k.clone(),
                });
                // Control is hidden, not disabled, where the system has none.
                if is_available(Feature::ServiceControl) {
                    for (label, what) in SERVICE_ACTIONS.into_iter().rev() {
                        if parts::header_button(ui, label, sel.is_some()).clicked() {
                            if let Some(n) = &sel {
                                actions.push(Action::Service(n.clone(), what));
                            }
                        }
                    }
                }
                crate::widgets::search_field(ui, &mut self.search, "Filter services", 260.0);
            });
        });
        ui.add_space(GAP);
        let list = match self.list.as_ref() {
            Some(Ok(l)) => l.clone(),
            Some(Err(e)) => {
                let t = theme::of_ui(ui);
                summary_band(ui, Mark::Dot(t.red), "The services could not be read", e);
                return;
            }
            None => Vec::new(),
        };
        // Rows that carry a scope come from the system's service managers
        // (not the Service Control Manager).
        let scoped = list.iter().any(|s| !s.scope.is_empty());
        let source_note = if scoped {
            "From the service managers of this system, read when the page opens and on Refresh."
        } else {
            "From the Service Control Manager, read when the page opens, on Refresh and after each start, stop or restart."
        };
        summary_band(ui, Mark::None, &services_headline(&list), source_note);
        let q = self.search.trim().to_lowercase();
        // By name or display name, or by the PID of the host (Go to
        // service(s) from Processes filters by it).
        let rows: Vec<&ServiceRow> = list
            .iter()
            .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.display.to_lowercase().contains(&q) || (s.pid > 0 && s.pid.to_string() == q))
            .collect();
        let selected = self.selected.clone();
        let mut panel_action = None;
        let new_sel = list_with_panel(
            ui,
            |ui, _| {
                let mut new_sel = None;
                design_table(ui, |ui| {
                    let mut table = TableBuilder::new(ui)
                        .resizable(true)
                        .sense(Sense::click())
                        .cell_layout(Layout::left_to_right(Align::Center))
                        .column(Column::initial(180.0).clip(true))
                        .column(Column::initial(60.0))
                        .column(Column::remainder().at_least(120.0).clip(true))
                        .column(Column::initial(90.0))
                        .column(Column::initial(100.0).clip(true));
                    let mut headings = vec!["Name", "PID", "Description", "Status", "Start type"];
                    if scoped {
                        table = table.column(Column::initial(70.0));
                        headings.push("Scope");
                    }
                    table
                        .header(TABLE_HEADER_HEIGHT, |mut h| {
                            for t in headings {
                                h.col(|ui| column_heading(ui, t));
                            }
                        })
                        .body(|body| {
                            body.rows(TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                                let s = rows[row.index()];
                                let on = selected.as_ref().is_some_and(|k| names(k, s));
                                row.set_selected(on);
                                row.col(|ui| {
                                    cell_text(ui, &s.name, CellInk::Main, on);
                                });
                                row.col(|ui| {
                                    cell_text(ui, &if s.pid > 0 { s.pid.to_string() } else { String::new() }, CellInk::Main, on);
                                });
                                row.col(|ui| {
                                    cell_text(ui, &s.display, CellInk::Soft, on);
                                });
                                row.col(|ui| {
                                    let ink = if s.status == "Running" { CellInk::Good } else { CellInk::Soft };
                                    cell_text(ui, &s.status, ink, on);
                                });
                                row.col(|ui| {
                                    cell_text(ui, &s.start_type, CellInk::Soft, on);
                                });
                                if scoped {
                                    row.col(|ui| {
                                        cell_text(ui, &s.scope, CellInk::Soft, on);
                                    });
                                }
                                let r = row.response();
                                if r.clicked() || r.secondary_clicked() {
                                    new_sel = Some(s.key());
                                }
                                r.context_menu(|ui| {
                                    if is_available(Feature::ServiceControl) {
                                        for (label, what) in SERVICE_ACTIONS {
                                            if ui.button(label).clicked() {
                                                actions.push(Action::Service(s.key(), what));
                                                ui.close();
                                            }
                                        }
                                        if let Some((label, what)) = boot_action(s) {
                                            if ui.button(label).clicked() {
                                                actions.push(Action::Service(s.key(), what));
                                                ui.close();
                                            }
                                        }
                                        ui.separator();
                                    }
                                    if has_process_jump(s) && ui.button(GO_TO_PROCESS).clicked() {
                                        actions.push(Action::GoToProcess(s.pid));
                                        ui.close();
                                    }
                                    if ui.button("Copy name").clicked() {
                                        actions.push(Action::Copy(s.name.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Search online").clicked() {
                                        actions.push(Action::SearchOnline(s.name.clone()));
                                        ui.close();
                                    }
                                });
                            });
                        });
                });
                new_sel
            },
            |ui, new_sel| {
                let key = new_sel.as_ref().or(selected.as_ref());
                let chosen = key.and_then(|k| list.iter().find(|s| names(k, s)));
                SidePanel::new("svc-detail").show(ui, chosen, |ui, s| {
                    panel_action = service_detail(ui, s).map(|what| if what == GO_TO_PROCESS_ACTION { Action::GoToProcess(s.pid) } else { Action::Service(s.key(), what) });
                });
            },
        );
        actions.extend(panel_action);
        if new_sel.is_some() {
            self.selected = new_sel;
        }
    }
}

/// Services' side panel: the chosen service's fields (as the list shows
/// them) and Start / Stop / Restart. The action clicked.
fn service_detail(ui: &mut Ui, s: &ServiceRow) -> Option<&'static str> {
    panel_title(ui, &s.name);
    let pid = if s.pid > 0 { s.pid.to_string() } else { String::new() };
    let mut labels = vec!["Description", "PID", "Status", "Start type"];
    let mut values = vec![s.display.as_str(), pid.as_str(), s.status.as_str(), s.start_type.as_str()];
    if !s.scope.is_empty() {
        labels.push("Scope");
        values.push(s.scope.as_str());
    }
    if !s.source.is_empty() {
        labels.push("Source");
        values.push(s.source.as_str());
    }
    facts(ui, &labels, |ui, i| fact_values(ui, &values, i));
    ui.add_space(8.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        if is_available(Feature::ServiceControl) {
            for (label, what) in SERVICE_ACTIONS {
                if parts::outline_button(ui, label, true).clicked() {
                    clicked = Some(what);
                }
            }
            if let Some((label, what)) = boot_action(s) {
                if parts::outline_button(ui, label, true).clicked() {
                    clicked = Some(what);
                }
            }
        }
        if has_process_jump(s) && parts::outline_button(ui, GO_TO_PROCESS, true).clicked() {
            clicked = Some(GO_TO_PROCESS_ACTION);
        }
    });
    clicked
}

/// The jump to Processes: the words and the panel's answer for it.
const GO_TO_PROCESS: &str = "Go to process";
const GO_TO_PROCESS_ACTION: &str = "go-to-process";

/// A service that runs shows the jump to its process when its manager says
/// which one (rows with a source; the Windows list keeps its look).
fn has_process_jump(s: &ServiceRow) -> bool {
    s.pid > 0 && !s.source.is_empty()
}

/// "339 services (152 running, 187 stopped)".
fn services_headline(rows: &[ServiceRow]) -> String {
    let running = rows.iter().filter(|s| s.status == "Running").count();
    let stopped = rows.iter().filter(|s| s.status == "Stopped").count();
    let noun = if rows.len() == 1 { "service" } else { "services" };
    format!("{} {noun} ({running} running, {stopped} stopped)", rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_pages::testing::{click, has, texts, Env};
    use crate::widgets::parts::EMPTY_DETAIL;

    fn service(status: &str) -> ServiceRow {
        ServiceRow { name: "Svc".into(), display: String::new(), pid: 0, status: status.into(), start_type: "Manual".into(), ..Default::default() }
    }

    fn running(name: &str) -> ServiceRow {
        ServiceRow { name: name.into(), status: "Running".into(), ..service("Running") }
    }

    fn hosted(name: &str, pid: u32) -> ServiceRow {
        ServiceRow { pid, ..running(name) }
    }

    fn words(draw: impl FnMut(&mut Ui)) -> Vec<String> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        crate::test_ui::texts(&crate::test_ui::run(&ctx, (1300.0, 800.0), vec![], draw))
    }

    fn ui_ctx() -> egui::Context {
        crate::test_ui::context(theme::Mode::Dark)
    }

    #[test]
    fn the_services_side_panel_shows_the_chosen_service_and_its_actions() {
        let env = Env::new();
        let wsearch = ServiceRow { name: "WSearch".into(), display: "Windows Search".into(), pid: 14192, status: "Running".into(), start_type: "Automatic".into(), ..Default::default() };
        env.system.script_services(vec![wsearch, service("Stopped")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        let none = texts(&ui, &mut page, &env.ctx());
        assert!(has(&none, EMPTY_DETAIL), "{none:?}");
        page.show(Some("WSearch".into()), String::new());
        let chosen = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&chosen, EMPTY_DETAIL));
        assert_eq!(chosen.iter().filter(|w| *w == "14192").count(), 2, "the PID in the list and in the panel");
        for action in ["Start", "Stop", "Restart"] {
            assert_eq!(chosen.iter().filter(|w| *w == action).count(), 2, "{action} in the header and in the panel");
        }
    }

    #[test]
    fn the_services_panel_holds_the_rows_fields() {
        let s = ServiceRow { name: "WSearch".into(), display: "Windows Search".into(), pid: 14192, status: "Running".into(), start_type: "Automatic".into(), ..Default::default() };
        let shown = words(|ui| {
            service_detail(ui, &s);
        });
        for w in ["WSearch", "Description", "Windows Search", "PID", "14192", "Status", "Running", "Start type", "Automatic", "Start", "Stop", "Restart"] {
            assert!(has(&shown, w), "{w} in {shown:?}");
        }
    }

    fn systemd_row(name: &str, scope: &str, pid: u32) -> ServiceRow {
        ServiceRow { name: name.into(), display: "A daemon".into(), pid, status: "Running".into(), start_type: "Enabled".into(), source: "systemd".into(), scope: scope.into() }
    }

    #[test]
    fn rows_with_a_scope_show_it_in_the_list_and_the_panel_with_their_source() {
        let env = Env::new();
        env.system.script_services(vec![systemd_row("ssh.service", "System", 812), systemd_row("pipewire.service", "User", 2210)]);
        let mut page = ServicesPage::default();
        page.show(Some("pipewire.service".into()), String::new());
        let shown = texts(&ui_ctx(), &mut page, &env.ctx());
        for w in ["Scope", "System", "User", "Source", "systemd", "2210"] {
            assert!(has(&shown, w), "{w} in {shown:?}");
        }
        let windows_like = texts(&ui_ctx(), &mut ServicesPage::default(), &Env::new().ctx());
        assert!(!has(&windows_like, "Scope"), "a list without scopes keeps the Windows columns: {windows_like:?}");
    }

    #[test]
    fn a_running_service_of_a_manager_links_to_its_process() {
        let s = systemd_row("ssh.service", "System", 812);
        let shown = words(|ui| {
            service_detail(ui, &s);
        });
        assert!(has(&shown, GO_TO_PROCESS), "{shown:?}");
        let stopped = ServiceRow { pid: 0, status: "Stopped".into(), ..s };
        assert!(!has(&words(|ui| {
            service_detail(ui, &stopped);
        }), GO_TO_PROCESS), "no process, no jump");
        let windows = ServiceRow { name: "WSearch".into(), pid: 14192, ..service("Running") };
        assert!(!has(&words(|ui| {
            service_detail(ui, &windows);
        }), GO_TO_PROCESS), "the Windows panel is unchanged");
    }

    #[test]
    fn control_is_hidden_where_the_system_has_none_and_the_jump_stays() {
        let _guard = crate::availability::unavailable_in_test(&[Feature::ServiceControl]);
        let env = Env::new();
        env.system.script_services(vec![systemd_row("ssh.service", "System", 812)]);
        let mut page = ServicesPage::default();
        page.show(Some("ssh.service".into()), String::new());
        let shown = texts(&ui_ctx(), &mut page, &env.ctx());
        for action in ["Start", "Stop", "Restart"] {
            assert!(!has(&shown, action), "{action} hidden: {shown:?}");
        }
        assert!(has(&shown, GO_TO_PROCESS) && has(&shown, "ssh.service"), "{shown:?}");
    }

    #[test]
    fn a_services_panel_action_acts_on_the_chosen_service() {
        let s = ServiceRow { name: "WSearch".into(), ..service("Running") };
        let ctx = ui_ctx();
        let mut clicked = None;
        let out = crate::test_ui::run(&ctx, (400.0, 400.0), vec![], |ui| {
            service_detail(ui, &s);
        });
        let stop = crate::test_ui::flat_shapes(&out)
            .iter()
            .find_map(|sh| match sh {
                egui::Shape::Text(g) if g.galley.text() == "Stop" => Some(g.visual_bounding_rect().center()),
                _ => None,
            })
            .unwrap();
        let press = |pressed| egui::Event::PointerButton { pos: stop, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        crate::test_ui::run(&ctx, (400.0, 400.0), vec![egui::Event::PointerMoved(stop), press(true)], |ui| {
            service_detail(ui, &s);
        });
        crate::test_ui::run(&ctx, (400.0, 400.0), vec![press(false)], |ui| clicked = service_detail(ui, &s).or(clicked));
        assert_eq!(clicked, Some("stop"));
    }

    #[test]
    fn the_services_band_counts_running_and_stopped_services() {
        let rows = [service("Running"), service("Stopped"), service("Stopped"), service("Paused")];
        assert_eq!(services_headline(&rows), "4 services (1 running, 2 stopped)");
        assert_eq!(services_headline(&rows[..1]), "1 service (1 running, 0 stopped)");
    }

    #[test]
    fn the_first_draw_of_services_reads_the_list_once() {
        let env = Env::new();
        env.system.script_services(vec![running("WSearch"), running("Spooler")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        let first = texts(&ui, &mut page, &env.ctx());
        let second = texts(&ui, &mut page, &env.ctx());
        assert_eq!(env.system.service_reads(), 1, "drawn twice, read once");
        assert!(has(&first, "WSearch") && has(&second, "Spooler"), "{first:?}");
        assert!(has(&first, "2 services (2 running, 0 stopped)"), "{first:?}");
    }

    #[test]
    fn refresh_on_services_rereads_the_list() {
        let env = Env::new();
        env.system.script_services(vec![running("WSearch")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        texts(&ui, &mut page, &env.ctx());
        env.system.script_services(vec![running("WSearch"), running("Spooler")]);
        click(&ui, &mut page, &env.ctx(), "Refresh");
        assert_eq!(env.system.service_reads(), 2, "the first draw and the click");
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(has(&shown, "Spooler"), "the reread list is shown: {shown:?}");
    }

    #[test]
    fn a_refresh_asked_of_the_page_rereads_the_list() {
        let env = Env::new();
        let mut page = ServicesPage::default();
        texts(&ui_ctx(), &mut page, &env.ctx());
        page.refresh(&env.ctx());
        assert_eq!(env.system.service_reads(), 2);
    }

    #[test]
    fn a_failed_services_read_shows_the_band_with_its_reason() {
        let env = Env::new();
        env.system.fail_services("Access is denied. (os error 5)");
        let shown = texts(&ui_ctx(), &mut ServicesPage::default(), &env.ctx());
        assert!(has(&shown, "The services could not be read"), "{shown:?}");
        assert!(has(&shown, "Access is denied. (os error 5)"), "{shown:?}");
    }

    #[test]
    fn services_keep_their_own_filter() {
        let env = Env::new();
        env.system.script_services(vec![running("WSearch"), running("Spooler")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        let unfiltered = texts(&ui, &mut page, &env.ctx());
        assert!(has(&unfiltered, "Filter services"), "{unfiltered:?}");
        assert!(!has(&unfiltered, "Filter by name, user, or PID"));
        page.show(None, "wsearch".into());
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(has(&shown, "WSearch"), "{shown:?}");
        assert!(!has(&shown, "Spooler"));
        assert!(!shown.iter().any(|t| t.starts_with("Clear the search box")), "the processes message stays on Processes");
    }

    #[test]
    fn the_services_filter_also_matches_the_pid_of_their_host() {
        let env = Env::new();
        env.system.script_services(vec![hosted("WSearch", 300), hosted("Winmgmt", 200), hosted("Schedule", 200)]);
        let mut page = ServicesPage::default();
        page.show(None, "200".into());
        let shown = texts(&ui_ctx(), &mut page, &env.ctx());
        assert!(has(&shown, "Winmgmt") && has(&shown, "Schedule"), "{shown:?}");
        assert!(!has(&shown, "WSearch"), "another host's service");
    }

    #[test]
    fn clicking_a_service_chooses_it_and_it_stays_chosen_while_the_page_is_hidden() {
        let env = Env::new();
        env.system.script_services(vec![running("WSearch"), running("Spooler")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        click(&ui, &mut page, &env.ctx(), "Spooler");
        let chosen = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&chosen, EMPTY_DETAIL), "its panel is open: {chosen:?}");
        // Another page is shown for a while: this one is not drawn.
        let back = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&back, EMPTY_DETAIL), "still chosen when shown again: {back:?}");
        assert_eq!(back.iter().filter(|w| *w == "Stop").count(), 2, "the actions of the chosen one");
    }

    #[test]
    fn a_header_action_names_the_chosen_service_and_does_nothing_without_one() {
        let env = Env::new();
        env.system.script_services(vec![running("WSearch")]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        assert!(click(&ui, &mut page, &env.ctx(), "Stop").is_empty(), "nothing is chosen");
        page.show(Some("WSearch".into()), String::new());
        assert_eq!(click(&ui, &mut page, &env.ctx(), "Stop"), vec![Action::Service(ServiceKey::from("WSearch"), "stop")]);
    }
    #[test]
    fn control_names_the_chosen_row_by_name_manager_and_scope() {
        let env = Env::new();
        env.system.script_services(vec![systemd_row("sync.service", "System", 0), systemd_row("sync.service", "User", 0)]);
        let mut page = ServicesPage::default();
        let ui = ui_ctx();
        click(&ui, &mut page, &env.ctx(), "User");
        let user = ServiceKey { name: "sync.service".into(), source: "systemd".into(), scope: "User".into() };
        assert_eq!(click(&ui, &mut page, &env.ctx(), "Stop"), vec![Action::Service(user, "stop")]);
    }

    #[test]
    fn a_service_of_a_manager_offers_the_change_of_its_start_type_at_boot() {
        let enabled = systemd_row("ssh.service", "System", 812);
        let shown = words(|ui| {
            service_detail(ui, &enabled);
        });
        assert!(has(&shown, "Disable at boot") && !has(&shown, "Enable at boot"), "{shown:?}");
        let disabled = ServiceRow { start_type: "Disabled".into(), ..enabled.clone() };
        let shown = words(|ui| {
            service_detail(ui, &disabled);
        });
        assert!(has(&shown, "Enable at boot") && !has(&shown, "Disable at boot"), "{shown:?}");
        let fixed = ServiceRow { start_type: "Static".into(), ..enabled };
        let shown = words(|ui| {
            service_detail(ui, &fixed);
        });
        assert!(!has(&shown, "Enable at boot") && !has(&shown, "Disable at boot"), "a unit that cannot change has neither: {shown:?}");
    }

    #[test]
    fn the_windows_panel_has_no_boot_buttons() {
        let s = ServiceRow { name: "WSearch".into(), start_type: "Automatic".into(), ..service("Running") };
        let shown = words(|ui| {
            service_detail(ui, &s);
        });
        assert!(!has(&shown, "Enable at boot") && !has(&shown, "Disable at boot"), "{shown:?}");
        let disabled = ServiceRow { start_type: "Disabled".into(), ..s };
        assert_eq!(boot_action(&disabled), None);
    }

    #[test]
    fn the_boot_button_asks_for_the_service_key() {
        let env = Env::new();
        let mut row = systemd_row("cron.service", "System", 0);
        row.start_type = "Disabled".into();
        env.system.script_services(vec![row.clone()]);
        let mut page = ServicesPage::default();
        page.show(Some("cron.service".into()), String::new());
        assert_eq!(click(&ui_ctx(), &mut page, &env.ctx(), "Enable at boot"), vec![Action::Service(row.key(), "enable")]);
    }
}
