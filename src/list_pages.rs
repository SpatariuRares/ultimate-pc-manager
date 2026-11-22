//! Services, Startup apps and App history, in Design A's look: the header,
//! a status band that sums the list up, then the list (with the chosen
//! row's side panel on Services and Startup apps).

use egui::{Align, Layout, RichText, Sense, Ui};
use egui_extras::{Column, TableBuilder};

use crate::app::{Action, TaskApp};
use crate::theme;
use crate::widgets::parts::{
    self, cell_text, column_heading, design_table, facts, list_with_panel, CellInk, Mark, SidePanel, StatusBand, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT,
};
use crate::widgets::{first_path, fmt_bytes, refresh_button};
use crate::system_lists::ServiceRow;
use crate::winsys::{self, StartupRow};

/// Space between the header, the band and the list.
const GAP: f32 = 8.0;

/// A band that only sums the page up: a headline and the line under it.
fn summary_band(ui: &mut Ui, mark: Mark, headline: &str, sub: &str) {
    StatusBand::new(mark, headline).sub(sub).actions_width(0.0).show(ui, |_, _| {});
    ui.add_space(GAP);
}

/// What Services can do to a service: the button's words and the action.
const SERVICE_ACTIONS: [(&str, &str); 3] = [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")];

/// The side panel's title: the row's name in the small figure face.
fn panel_title(ui: &mut Ui, name: &str) {
    let t = theme::of_ui(ui);
    ui.add(egui::Label::new(RichText::new(name).font(t.fonts.figure_small.clone()).color(t.text)).wrap());
    ui.add_space(4.0);
}

/// The values of a fact list, in the text colour, wrapped.
fn fact_values(ui: &mut Ui, values: &[&str], i: usize) {
    let t = theme::of_ui(ui);
    ui.add(egui::Label::new(RichText::new(values[i]).color(t.text)).wrap());
}

/// Services' side panel: the chosen service's fields (as the list shows
/// them) and Start / Stop / Restart. The action clicked.
fn service_detail(ui: &mut Ui, s: &ServiceRow) -> Option<&'static str> {
    panel_title(ui, &s.name);
    let pid = if s.pid > 0 { s.pid.to_string() } else { String::new() };
    let values = [s.display.as_str(), pid.as_str(), s.status.as_str(), s.start_type.as_str()];
    facts(ui, &["Description", "PID", "Status", "Start type"], |ui, i| fact_values(ui, &values, i));
    ui.add_space(8.0);
    let mut clicked = None;
    ui.horizontal(|ui| {
        for (label, what) in SERVICE_ACTIONS {
            if parts::outline_button(ui, label, true).clicked() {
                clicked = Some(what);
            }
        }
    });
    clicked
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

/// "339 services (152 running, 187 stopped)".
pub fn services_headline(rows: &[ServiceRow]) -> String {
    let running = rows.iter().filter(|s| s.status == "Running").count();
    let stopped = rows.iter().filter(|s| s.status == "Stopped").count();
    let noun = if rows.len() == 1 { "service" } else { "services" };
    format!("{} {noun} ({running} running, {stopped} stopped)", rows.len())
}

/// "9 of 32 startup apps are enabled".
pub fn startup_headline(rows: &[StartupRow]) -> String {
    let on = rows.iter().filter(|r| r.enabled).count();
    let verb = if on == 1 { "is" } else { "are" };
    format!("{on} of {} startup apps {verb} enabled", rows.len())
}

/// "Resource usage since 12 minutes ago".
pub fn history_headline(minutes: u64) -> String {
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("Resource usage since {minutes} {unit} ago")
}

impl TaskApp {
    /// The one place the services list is read: every reader (first draw,
    /// Refresh, F5, the sidebar, Go to services, a control) comes here.
    pub fn reload_services(&mut self) {
        self.services = Some(self.system.list_services());
    }

    pub fn page_services(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        if self.services.is_none() {
            self.reload_services();
        }
        ui.horizontal(|ui| {
            self.page_title(ui, "Services");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if refresh_button(ui).clicked() {
                    self.reload_services();
                }
                let sel = self.svc_selected.clone();
                for (label, what) in SERVICE_ACTIONS.into_iter().rev() {
                    if parts::header_button(ui, label, sel.is_some()).clicked() {
                        if let Some(n) = &sel {
                            actions.push(Action::Service(n.clone(), what));
                        }
                    }
                }
                crate::widgets::search_field(ui, &mut self.search, "Filter services", 260.0);
            });
        });
        ui.add_space(GAP);
        let list = match self.services.as_ref() {
            Some(Ok(l)) => l.clone(),
            Some(Err(e)) => {
                let t = theme::of_ui(ui);
                summary_band(ui, Mark::Dot(t.red), "The services could not be read", e);
                return;
            }
            None => Vec::new(),
        };
        summary_band(ui, Mark::None, &services_headline(&list), "From the Service Control Manager, read when the page opens, on Refresh and after each start, stop or restart.");
        let q = self.search.trim().to_lowercase();
        // By name or display name, or by the PID of the host (Go to
        // service(s) from Processes filters by it).
        let rows: Vec<&ServiceRow> = list
            .iter()
            .filter(|s| q.is_empty() || s.name.to_lowercase().contains(&q) || s.display.to_lowercase().contains(&q) || (s.pid > 0 && s.pid.to_string() == q))
            .collect();
        let selected = self.svc_selected.clone();
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
                        .column(Column::initial(180.0).clip(true))
                        .column(Column::initial(60.0))
                        .column(Column::remainder().at_least(120.0).clip(true))
                        .column(Column::initial(90.0))
                        .column(Column::initial(100.0).clip(true))
                        .header(TABLE_HEADER_HEIGHT, |mut h| {
                            for t in ["Name", "PID", "Description", "Status", "Start type"] {
                                h.col(|ui| column_heading(ui, t));
                            }
                        })
                        .body(|body| {
                            body.rows(TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                                let s = rows[row.index()];
                                let on = selected.as_deref() == Some(s.name.as_str());
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
                                let r = row.response();
                                if r.clicked() || r.secondary_clicked() {
                                    new_sel = Some(s.name.clone());
                                }
                                r.context_menu(|ui| {
                                    for (label, what) in [("Start", "start"), ("Stop", "stop"), ("Restart", "restart")] {
                                        if ui.button(label).clicked() {
                                            actions.push(Action::Service(s.name.clone(), what));
                                            ui.close();
                                        }
                                    }
                                    ui.separator();
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
                let name = new_sel.as_ref().or(selected.as_ref());
                let chosen = name.and_then(|n| list.iter().find(|s| &s.name == n));
                SidePanel::new("svc-detail").show(ui, chosen, |ui, s| {
                    panel_action = service_detail(ui, s).map(|what| Action::Service(s.name.clone(), what));
                });
            },
        );
        actions.extend(panel_action);
        if new_sel.is_some() {
            self.svc_selected = new_sel;
        }
    }

    pub fn page_startup(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        if self.startup.is_none() {
            self.startup = Some(winsys::list_startup());
        }
        let list = self.startup.clone().unwrap_or_default();
        ui.horizontal(|ui| {
            self.page_title(ui, "Startup apps");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if refresh_button(ui).clicked() {
                    self.startup = Some(winsys::list_startup());
                }
                if let Some(i) = self.startup_selected.filter(|i| *i < list.len()) {
                    let en = list[i].enabled;
                    if parts::header_button(ui, if en { "Disable" } else { "Enable" }, true).clicked() {
                        actions.push(Action::Startup(i, !en));
                    }
                }
            });
        });
        ui.add_space(GAP);
        let off = list.iter().filter(|r| !r.enabled).count();
        let sub = format!("{off} of them are turned off. From the Run keys and Startup folders, read when the page opens and on Refresh.");
        summary_band(ui, Mark::None, &startup_headline(&list), &sub);
        let selected = self.startup_selected;
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
                                        actions.push(Action::Startup(i, !r.enabled));
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
                SidePanel::new("startup-detail").show(ui, i.map(|i| (i, &list[i])), |ui, (i, r)| {
                    if startup_detail(ui, r) {
                        panel_action = Some(Action::Startup(i, !r.enabled));
                    }
                });
            },
        );
        actions.extend(panel_action);
        if new_sel.is_some() {
            self.startup_selected = new_sel;
        }
    }

    pub fn page_history(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            self.page_title(ui, "App history");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if parts::header_button(ui, "Delete usage history", true).clicked() {
                    self.history.clear();
                    self.history_since = std::time::Instant::now();
                }
            });
        });
        ui.add_space(GAP);
        let mins = self.history_since.elapsed().as_secs() / 60;
        summary_band(ui, Mark::None, &history_headline(mins), "Counted only while this app is open.");
        let mut rows: Vec<(String, f64, f64)> = self.history.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows.retain(|r| r.1 >= 0.01 || r.2 > 0.0);
        design_table(ui, |ui| {
            TableBuilder::new(ui)
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::initial(300.0).clip(true))
                .column(Column::initial(140.0))
                .column(Column::remainder())
                .header(TABLE_HEADER_HEIGHT, |mut h| {
                    for t in ["Name", "CPU time", "Disk read + write"] {
                        h.col(|ui| column_heading(ui, t));
                    }
                })
                .body(|body| {
                    body.rows(TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                        let (name, cpu, disk) = &rows[row.index()];
                        row.col(|ui| {
                            cell_text(ui, name, CellInk::Main, false);
                        });
                        row.col(|ui| {
                            let s = *cpu as u64;
                            cell_text(ui, &format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60), CellInk::Main, false);
                        });
                        row.col(|ui| {
                            cell_text(ui, &fmt_bytes(*disk as u64), CellInk::Soft, false);
                        });
                    });
                });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::parts::EMPTY_DETAIL;

    fn service(status: &str) -> ServiceRow {
        ServiceRow { name: "Svc".into(), display: String::new(), pid: 0, status: status.into(), start_type: "Manual".into() }
    }

    fn startup(enabled: bool) -> StartupRow {
        StartupRow { enabled, ..Default::default() }
    }

    fn app() -> TaskApp {
        TaskApp::headless(crate::settings::Settings::default(), Default::default())
    }

    fn words(draw: impl FnMut(&mut Ui)) -> Vec<String> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        crate::test_ui::texts(&crate::test_ui::run(&ctx, (1300.0, 800.0), vec![], draw))
    }

    fn has(words: &[String], text: &str) -> bool {
        words.iter().any(|w| w == text)
    }

    #[test]
    fn the_services_side_panel_shows_the_chosen_service_and_its_actions() {
        let mut app = app();
        let wsearch = ServiceRow { name: "WSearch".into(), display: "Windows Search".into(), pid: 14192, status: "Running".into(), start_type: "Automatic".into() };
        app.services = Some(Ok(vec![wsearch, service("Stopped")]));
        let none = words(|ui| app.page_services(ui, &mut Vec::new()));
        assert!(has(&none, EMPTY_DETAIL), "{none:?}");
        app.svc_selected = Some("WSearch".into());
        let chosen = words(|ui| app.page_services(ui, &mut Vec::new()));
        assert!(!has(&chosen, EMPTY_DETAIL));
        assert_eq!(chosen.iter().filter(|w| *w == "14192").count(), 2, "the PID in the list and in the panel");
        for action in ["Start", "Stop", "Restart"] {
            assert_eq!(chosen.iter().filter(|w| *w == action).count(), 2, "{action} in the header and in the panel");
        }
    }

    #[test]
    fn the_services_panel_holds_the_rows_fields() {
        let s = ServiceRow { name: "WSearch".into(), display: "Windows Search".into(), pid: 14192, status: "Running".into(), start_type: "Automatic".into() };
        let shown = words(|ui| {
            service_detail(ui, &s);
        });
        for w in ["WSearch", "Description", "Windows Search", "PID", "14192", "Status", "Running", "Start type", "Automatic", "Start", "Stop", "Restart"] {
            assert!(has(&shown, w), "{w} in {shown:?}");
        }
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
    fn a_services_panel_action_acts_on_the_chosen_service() {
        let s = ServiceRow { name: "WSearch".into(), ..service("Running") };
        let ctx = crate::test_ui::context(theme::Mode::Dark);
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
    fn the_startup_side_panel_shows_the_chosen_app_and_enable_or_disable() {
        let mut app = app();
        let steam = StartupRow { name: "Steam".into(), command: "steam.exe -silent".into(), location: "HKCU Run".into(), enabled: true, ..Default::default() };
        app.startup = Some(vec![steam, startup(false)]);
        let none = words(|ui| app.page_startup(ui, &mut Vec::new()));
        assert!(has(&none, EMPTY_DETAIL), "{none:?}");
        app.startup_selected = Some(0);
        let chosen = words(|ui| app.page_startup(ui, &mut Vec::new()));
        assert!(!has(&chosen, EMPTY_DETAIL));
        assert_eq!(chosen.iter().filter(|w| *w == "HKCU Run").count(), 2, "the location in the list and in the panel");
        assert_eq!(chosen.iter().filter(|w| *w == "Disable").count(), 2, "Disable in the header and in the panel");
    }

    #[test]
    fn the_services_band_counts_running_and_stopped_services() {
        let rows = [service("Running"), service("Stopped"), service("Stopped"), service("Paused")];
        assert_eq!(services_headline(&rows), "4 services (1 running, 2 stopped)");
        assert_eq!(services_headline(&rows[..1]), "1 service (1 running, 0 stopped)");
    }

    #[test]
    fn the_startup_band_counts_the_enabled_apps() {
        assert_eq!(startup_headline(&[startup(true), startup(false), startup(true)]), "2 of 3 startup apps are enabled");
        assert_eq!(startup_headline(&[startup(true), startup(false)]), "1 of 2 startup apps is enabled");
    }

    fn running(name: &str) -> ServiceRow {
        ServiceRow { name: name.into(), status: "Running".into(), ..service("Running") }
    }

    /// An app whose services come from a fake the test keeps.
    fn app_on(fake: &std::sync::Arc<crate::system_lists::fake::FakeSystem>) -> TaskApp {
        app().on_system(fake.clone())
    }

    fn draw_services(app: &mut TaskApp) -> Vec<String> {
        words(|ui| app.page_services(ui, &mut Vec::new()))
    }

    #[test]
    fn the_first_draw_of_services_reads_the_list_once() {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.script_services(vec![running("WSearch"), running("Spooler")]);
        let mut app = app_on(&fake);
        let first = draw_services(&mut app);
        let second = draw_services(&mut app);
        assert_eq!(fake.service_reads(), 1, "drawn twice, read once");
        assert!(has(&first, "WSearch") && has(&second, "Spooler"), "{first:?}");
        assert!(has(&first, "2 services (2 running, 0 stopped)"), "{first:?}");
    }

    #[test]
    fn refresh_on_services_rereads_the_list() {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.script_services(vec![running("WSearch")]);
        let mut app = app_on(&fake);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let size = (1300.0, 800.0);
        let out = crate::test_ui::run(&ctx, size, vec![], |ui| app.page_services(ui, &mut Vec::new()));
        let refresh = crate::test_ui::flat_shapes(&out)
            .iter()
            .find_map(|sh| match sh {
                egui::Shape::Text(g) if g.galley.text().trim() == "Refresh" => Some(g.visual_bounding_rect().center()),
                _ => None,
            })
            .expect("a Refresh button");
        fake.script_services(vec![running("WSearch"), running("Spooler")]);
        let press = |pressed| egui::Event::PointerButton { pos: refresh, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        crate::test_ui::run(&ctx, size, vec![egui::Event::PointerMoved(refresh), press(true)], |ui| app.page_services(ui, &mut Vec::new()));
        crate::test_ui::run(&ctx, size, vec![press(false)], |ui| app.page_services(ui, &mut Vec::new()));
        assert_eq!(fake.service_reads(), 2, "the first draw and the click");
        let shown = draw_services(&mut app);
        assert!(has(&shown, "Spooler"), "the reread list is shown: {shown:?}");
    }

    #[test]
    fn f5_on_services_rereads_the_list() {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        let mut app = app_on(&fake);
        app.page = crate::app::Page::Core(crate::addons::Core::Services);
        draw_services(&mut app);
        app.refresh_now();
        assert_eq!(fake.service_reads(), 2);
    }

    #[test]
    fn a_failed_services_read_shows_the_band_with_its_reason() {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.fail_services("Access is denied. (os error 5)");
        let mut app = app_on(&fake);
        let shown = draw_services(&mut app);
        assert!(has(&shown, "The services could not be read"), "{shown:?}");
        assert!(has(&shown, "Access is denied. (os error 5)"), "{shown:?}");
    }

    #[test]
    fn the_history_band_says_since_when() {
        assert_eq!(history_headline(0), "Resource usage since 0 minutes ago");
        assert_eq!(history_headline(1), "Resource usage since 1 minute ago");
        assert_eq!(history_headline(12), "Resource usage since 12 minutes ago");
    }
}
