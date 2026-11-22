//! Summary, Processes, System Info and Users pages.

use std::collections::BTreeMap;

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::Column;

use crate::app::{status_parts, Action, ProcCol, TaskApp};
use crate::settings::SPEEDS;
use crate::page_perf::fmt_bytes_rate;
use crate::platform::PageContext;
use crate::process_view::{self, figure_of, percent, Chevron, FileTexts, Figures, Line, Pick, ProcessLine, Selection, Totals, ViewOptions};
use crate::sampler::ProcRow;
use crate::summary_view::Metric;
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, cell_text, BandArea, CellInk, Mark, SidePanel, StatusBand, TABLE_ROW_HEIGHT};
use crate::widgets::*;

impl TaskApp {
    pub fn page_summary(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        self.header(ui, "Summary");
        let t = self.settings.theme();
        // Drawn from the view built once per sample (see `summary_view`),
        // taken out while drawing (not copied) and put back after.
        self.read_summary_sensors();
        self.summary_view();
        let view = std::mem::take(&mut self.summary);
        let colour = |m: Metric| match m {
            Metric::Cpu => rgb(self.settings.cpu_color),
            Metric::Memory => rgb(self.settings.mem_color),
            Metric::Disk => rgb(self.settings.disk_color),
            Metric::Network => rgb(self.settings.net_color),
        };
        let cards = view.cards().map(|c| (c, colour(c.metric)));
        let band = &view.band;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            StatusBand::new(Mark::Dot(health_colour(&t, band.paused, band.providers)), &band.headline).sub(&band.sub).actions_width(0.0).show(ui, |_, _| {});
            ui.add_space(14.0);
            // The graphs on the left, the busiest processes beside them (under
            // them in a narrow window), as on board P01.
            let gap = 16.0;
            let wide = ui.available_width() >= 900.0;
            let aside = if wide { SUMMARY_ASIDE } else { ui.available_width() };
            let main = if wide { ui.available_width() - aside - gap } else { ui.available_width() };
            let graphs = |ui: &mut Ui| {
                ui.set_width(main);
                let w = ((main - 14.0) / 2.0).max(200.0);
                for pair in cards.chunks(2) {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for (c, color) in pair {
                            card(ui, c.metric.title(), c.value_text(), *color, w, |ui| {
                                ui.label(RichText::new(c.sub_text()).font(t.fonts.label.clone()).color(t.label));
                                if c.metric == Metric::Cpu {
                                    cpu_overview(ui, &t, &view, *color);
                                } else {
                                    graph(ui, vec2(ui.available_width(), 96.0), &c.history, c.max, *color);
                                }
                                if c.metric == Metric::Memory {
                                    // "Available X · Cached Y · Swap Z".
                                    ui.label(RichText::new(&view.memory_footer).font(t.fonts.label.clone()).color(t.label));
                                }
                            });
                        }
                    });
                    ui.add_space(14.0);
                }
            };
            if wide {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    ui.vertical(graphs);
                    ui.vertical(|ui| top_processes(ui, &t, aside, &view.top));
                });
            } else {
                ui.vertical(graphs);
                top_processes(ui, &t, aside, &view.top);
            }
            // Summary's tiles, then the add-ons' after them.
            crate::summary_tiles::tile_row(ui, &view.tiles);
            let mut page = PageContext::new(&self.settings, &self.snap);
            self.addons.summary_tiles(ui, &mut page);
            actions.append(&mut page.actions);
        });
        self.summary = view;
    }

    pub fn page_processes(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        // The header: Process View ⌄, the filter, Refresh. Run new task
        // sits under File (Ctrl+N).
        let (mut refresh, mut view) = (false, None);
        let view_items = self.process_view_items();
        ui.horizontal(|ui| {
            self.page_title(ui, "Processes");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                refresh = refresh_button(ui).clicked();
                crate::widgets::search_field(ui, &mut self.search, "Filter by name, user, or PID", 300.0);
                view = parts::dropdown_button(ui, "Process View", &view_items);
            });
        });
        if refresh {
            self.refresh_now();
        }
        if let Some(item) = view {
            self.apply_process_view(item);
        }
        ui.add_space(10.0);

        // Follow process: the followed row stays the one selected, its tree
        // open; following ends with the process.
        if let Some(pid) = self.following.as_ref().map(|f| f.pid) {
            if self.snap.procs.iter().any(|p| p.pid == pid) {
                self.selected = Selection::only(pid);
                self.proc_view.reveal(&self.snap.procs, pid);
            } else {
                self.following = None;
            }
        }
        // "Go to the process": open the rows above it first.
        if let (true, Some(pid)) = (self.reveal_selected, self.selected.single()) {
            self.proc_view.reveal(&self.snap.procs, pid);
        }
        let q = self.search.trim().to_lowercase();
        // The standard columns, then those of the add-ons that are on;
        // the ones shown, in the user's order. An add-on's column is read
        // only while it shows.
        let extra = self.addons.process_columns();
        let cols = columns_with(&extra);
        let shown: Vec<Placed> = layout_of(&cols, &self.settings.process_table).into_iter().filter(|p| p.shown).collect();
        let mut figures = Figures::new();
        for p in &shown {
            if let ProcCol::Addon(id) = p.col {
                if let Some(by_pid) = self.addons.process_figures(id, &self.settings) {
                    figures.insert(id, by_pid);
                }
            }
        }
        let files = self.file_texts(&shown);
        let sort = proc_sort_in(&cols, &self.settings.process_table);
        let opts = ViewOptions {
            tree: self.settings.process_tree,
            filter: self.search.clone(),
            sort,
            figures,
            files,
            show_exited: self.settings.show_exited,
            hide: self.hide(),
            smooth: self.settings.smooth_cpu,
            highlight: self.settings.highlight_updates,
            now: std::time::Instant::now(),
            services: self.snap.services.clone(),
        };
        let mut view = process_view::view(&self.snap.procs, &opts, &self.proc_view);
        // Right and Left open and close the selected tree row (not while
        // the filter box has the keys).
        let keys_free = !ui.memory(|m| m.focused().is_some());
        if let (Some(pid), true) = (self.selected.single(), keys_free) {
            let (left, right) = ui.input(|i| (i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight)));
            if left || right {
                self.proc_view.open(&view, pid, right);
                view = process_view::view(&self.snap.procs, &opts, &self.proc_view);
            }
        }
        if keys_free {
            // Type-ahead: letters typed jump to the row whose name starts
            // with them.
            let (typed, now) = ui.input(|i| {
                let typed: String = i.events.iter().filter_map(|e| if let egui::Event::Text(s) = e { Some(s.as_str()) } else { None }).collect();
                (typed, i.time)
            });
            if !typed.is_empty() {
                if let Some(pid) = self.proc_view.type_ahead.key(&view, &typed, now) {
                    self.selected = Selection::only(pid);
                    self.reveal_selected = true;
                }
            }
            // Delete = End task, after asking.
            if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
                let ask = EndAsk::task(&rows_of(&self.snap.procs, &self.selected));
                if ask.is_some() {
                    self.confirm_end = ask;
                }
            }
        }
        let lines = &view.lines;
        // A lit row goes dark after a moment, even without a new sample.
        if view.processes().any(|l| l.lit) {
            ui.ctx().request_repaint_after(process_view::LIT_FOR);
        }
        let figures = &opts.figures;
        let files = &opts.files;

        let s = &self.snap;
        let band = ProcessesBand::of(&s.procs, view.shown, &q);
        let load = format!(
            "CPU {:.0}% \u{b7} Memory {:.0}% \u{b7} Disk {:.1} MB/s",
            s.cpu_total,
            pct(s.mem_used, s.mem_total),
            (s.disk_read_bps + s.disk_write_bps) / 1_048_576.0
        );
        let sub = band.shown.clone().map_or(load.clone(), |shown| format!("{shown} \u{b7} {load}"));
        StatusBand::new(Mark::None, &band.headline).sub(&sub).actions_width(340.0).show(ui, |ui, area| {
            if area == BandArea::Actions {
                let busiest: Vec<(&str, &String)> = [("Most memory", &band.most_memory), ("Most CPU now", &band.most_cpu)]
                    .into_iter()
                    .filter_map(|(label, value)| value.as_ref().map(|v| (label, v)))
                    .collect();
                // The band's actions column is right-aligned; lay the pairs out left to right in it.
                ui.with_layout(Layout::left_to_right(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.x = 22.0;
                    for (label, value) in &busiest {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.label(RichText::new(*label).font(t.fonts.label.clone()).color(t.label));
                            ui.label(RichText::new(shorten(value, 34)).font(t.fonts.row.clone()).color(t.link));
                        });
                    }
                });
            }
        });
        ui.add_space(12.0);

        // The columns shown, in the user's order. CPU, Memory and the disk
        // columns carry the machine's total above their title.
        let headers: Vec<(ProcCol, String)> = shown
            .iter()
            .filter_map(|p| column_of(&cols, p.col))
            .map(|&ProcColumn { col: c, title, .. }| {
                let total = match c {
                    ProcCol::Cpu => Some(format!("{:.0}%", s.cpu_total)),
                    ProcCol::Mem => Some(format!("{:.0}%", pct(s.mem_used, s.mem_total))),
                    ProcCol::DiskRead => Some(fmt_bytes_rate(s.disk_read_bps)),
                    ProcCol::DiskWrite => Some(fmt_bytes_rate(s.disk_write_bps)),
                    _ => None,
                };
                (c, total.map_or(title.to_string(), |v| format!("{v}
{title}")))
            })
            .collect();
        // Resource values ▸ Percents: figures as a share of the machine's.
        let percents = self.settings.process_percents;
        let totals = Totals { mem: s.mem_total, disk_read: s.disk_read_bps, disk_write: s.disk_write_bps };
        // The items the add-ons' columns add to a process's menu.
        let links: Vec<(&'static str, &'static str)> = extra.iter().map(|(id, c)| (*id, c.menu_item)).collect();
        // Follow process: scroll to the followed row whenever a snapshot or
        // a sort moves it.
        if let Some(f) = &mut self.following {
            if f.moved(&view) {
                self.reveal_selected = true;
            }
        }
        let following = self.following.as_ref().map(|f| f.pid);
        let services = self.snap.services.clone();
        let system = self.actions.clone();
        let mut new_sort = None;
        let mut new_sel = None;
        let mut service_sel = None;
        let mut toggle_row = None;
        let mut toggle_group = None;
        let selected = self.selected.clone();
        let reveal = std::mem::take(&mut self.reveal_selected)
            .then(|| selected.single().and_then(|pid| view.line_of(pid)))
            .flatten();
        // A service row selected shows its own line; otherwise the
        // selected processes that still run.
        let chosen = match selected.service_line(&view) {
            Some(l) => vec![&l.row],
            None => rows_of(&self.snap.procs, &selected),
        };
        // Each header cell's width as drawn (the table keeps the widths
        // to itself), its span, and a header dragged onto another.
        let mut drawn: Vec<(ProcCol, f32, egui::Rangef)> = Vec::new();
        let mut dropped: Option<(ProcCol, f32)> = None;
        let mut table_layout = self.settings.process_table.clone();
        // A changed set of columns starts from the saved widths.
        let keys: Vec<ProcCol> = shown.iter().map(|p| p.col).collect();
        let keys_id = egui::Id::new("processes-columns-drawn");
        let reset = ui.data_mut(|d| d.get_temp::<Vec<ProcCol>>(keys_id)).as_ref() != Some(&keys);
        ui.data_mut(|d| d.insert_temp(keys_id, keys));
        // The table's toolbar: the column chooser, then (marked additions)
        // Pause, the app's update speed and Export to CSV.
        let paused = self.settings.interval_ms() == 0;
        let mut speed = None;
        let mut export = false;
        ui.horizontal(|ui| {
            ui.menu_button(columns_label_in(&cols, &table_layout), |ui| column_menu(ui, &cols, &mut table_layout));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                export = parts::link_button(ui, "Export to CSV").clicked();
                ui.add_space(12.0);
                let now = SPEEDS.get(self.settings.speed).map_or("", |s| s.0);
                egui::ComboBox::from_id_salt("processes-speed").selected_text(now).width(150.0).show_ui(ui, |ui| {
                    for (i, (label, ms)) in SPEEDS.iter().enumerate() {
                        if *ms != 0 && ui.selectable_label(self.settings.speed == i, *label).clicked() {
                            speed = Some(i);
                        }
                    }
                });
                ui.label("Update speed");
                ui.add_space(12.0);
                // Paused, the button resumes at the default speed.
                let (label, to) = if paused { ("Resume", crate::settings::Settings::default().speed) } else { ("Pause", SPEEDS.iter().position(|s| s.1 == 0).unwrap_or(SPEEDS.len() - 1)) };
                if parts::link_outline_button(ui, label, true).clicked() {
                    speed = Some(to);
                }
            });
        });
        if let Some(i) = speed {
            actions.push(Action::UpdateSpeed(i));
        }
        if export {
            let text = process_csv(view.processes(), &cols, &table_layout, &totals, percents, figures, files);
            actions.push(Action::ExportCsv { file_name: "processes.csv", text });
        }
        ui.add_space(6.0);
        // The table, then the selected process's details under it.
        let detail_h = 240.0;
        let table_h = (ui.available_height() - detail_h - 12.0).max(120.0);
        let width = ui.available_width();
        // A filter that hides every row says how to get them back.
        let hidden_by_filter = view.shown == 0 && !q.is_empty();
        let table = ui.allocate_ui(vec2(width, table_h), |ui| {
            // The columns keep their widths; when they do not fit, the table
            // scrolls sideways.
            egui::ScrollArea::horizontal().id_salt("processes-sideways").auto_shrink([false, false]).show(ui, |ui| {
            parts::design_table(ui, |ui| {
                let mut table = egui_extras::TableBuilder::new(ui)
                    .id_salt("processes")
                    .sense(Sense::click())
                    .cell_layout(Layout::left_to_right(Align::Center));
                if reset {
                    table.reset();
                }
                if let Some(i) = reveal {
                    table = table.scroll_to_row(i, Some(Align::Center));
                }
                for p in &shown {
                    table = table.column(Column::initial(p.width).at_least(MIN_COLUMN_WIDTH).clip(true));
                }
                table
                    .resizable(true)
                    .auto_shrink([false, false])
                    .max_scroll_height(f32::INFINITY)
                    .header(36.0, |mut header| {
                        for (c, label) in headers.iter() {
                            header.col(|ui| {
                                let cell_rect = ui.max_rect();
                                drawn.push((*c, cell_rect.width(), cell_rect.x_range()));
                                // Drag a header to move its column; right-click
                                // for the chooser.
                                let cell = ui.interact(cell_rect, ui.id().with(("processes-header", *c)), Sense::click_and_drag());
                                let heading = parts::sort_heading(ui, label, (sort.0 == *c).then_some(sort.1));
                                if heading.clicked() || cell.clicked() {
                                    new_sort = Some(*c);
                                }
                                if cell.dragged() {
                                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                                }
                                if cell.drag_stopped() {
                                    if let Some(at) = ui.ctx().pointer_interact_pos() {
                                        dropped = Some((*c, at.x));
                                    }
                                }
                                cell.union(heading).context_menu(|ui| column_menu(ui, &cols, &mut table_layout));
                            });
                        }
                    })
                    .body(|body| {
                        body.rows(TABLE_ROW_HEIGHT, lines.len(), |mut row| {
                            let l = match &lines[row.index()] {
                                Line::Group(g) => {
                                    // A group header: its title in the Name
                                    // column; a double-click folds it.
                                    for c in &shown {
                                        row.col(|ui| {
                                            if c.col == ProcCol::Name {
                                                parts::tree_chevron(ui, !g.folded, false);
                                                cell_text(ui, &g.group.title(g.count), CellInk::Main, false);
                                            }
                                        });
                                    }
                                    if row.response().double_clicked() {
                                        toggle_group = Some(g.group);
                                    }
                                    return;
                                }
                                Line::Process(l) => l,
                            };
                            let p = &l.row;
                            let on = selected.has(p);
                            // A service under its host: selected on its
                            // own, with no process menu (it is no process).
                            let is_service = p.service.is_some();
                            // Empty on a tree parent, which reads as a total.
                            let shows = |c| l.shows(c);
                            row.set_selected(on);
                            for c in &shown {
                                row.col(|ui| {
                                    // An exited process is tinted red.
                                    if l.exited && !on {
                                        let half_gap = ui.spacing().item_spacing.x / 2.0;
                                        ui.painter().rect_filled(ui.max_rect().expand2(vec2(half_gap, 0.0)), 0.0, t.alert_note);
                                    }
                                    if c.col == ProcCol::Name {
                                        ui.add_space(l.depth as f32 * parts::CHEVRON_WIDTH);
                                        if l.chevron == Chevron::Leaf {
                                            ui.add_space(parts::CHEVRON_WIDTH);
                                        } else if parts::tree_chevron(ui, l.chevron == Chevron::Open, on).clicked() {
                                            toggle_row = Some((p.pid, l.chevron == Chevron::Closed));
                                        }
                                    }
                                    if shows(c.col) {
                                        proc_cell(ui, c.col, l, on, &totals, percents, figures, files);
                                    }
                                });
                            }
                            let resp = row.response();
                            // Ctrl adds or removes a row, Shift adds the
                            // rows from the last one clicked; a right-click
                            // keeps a selection the row is part of.
                            if is_service {
                                if resp.clicked() || resp.secondary_clicked() || resp.double_clicked() {
                                    service_sel = Some(Selection::service(p));
                                }
                                return;
                            }
                            if resp.clicked() {
                                let m = resp.ctx.input(|i| i.modifiers);
                                new_sel = Some((p.pid, Pick::of(m.command || m.ctrl, m.shift)));
                            }
                            if resp.secondary_clicked() && !on {
                                new_sel = Some((p.pid, Pick::One));
                            }
                            // A double-click on a process row inspects it,
                            // like the menu's default item.
                            if resp.double_clicked() {
                                new_sel = Some((p.pid, Pick::One));
                                actions.push(Action::Inspect(p.pid));
                            }
                            resp.context_menu(|ui| {
                                let rows = if on { chosen.clone() } else { vec![p] };
                                let state = MenuState {
                                    links: &links,
                                    percents,
                                    view: &view_items,
                                    priority: None,
                                    affinity: None,
                                    shell: crate::winsys::shell_pid(),
                                    following,
                                    services: Some(&*services),
                                    windowed: None,
                                };
                                let state = crate::widgets::read_menu_state(&*system, p, &rows, &state);
                                process_menu(ui, p, &rows, &state, actions);
                            });
                        });
                    });
            });
            });
        });
        if hidden_by_filter {
            let at = table.response.rect.left_top() + vec2(12.0, 36.0 + 14.0);
            ui.painter().text(at, egui::Align2::LEFT_TOP, "Clear the search box to restore all process rows.", t.fonts.row.clone(), t.soft);
        }
        ui.add_space(12.0);
        let mut end = None;
        ui.allocate_ui(vec2(width, detail_h), |ui| {
            if process_details(ui, &t, &chosen, &self.snap.procs) {
                end = EndAsk::task(&chosen);
            }
        });
        if end.is_some() {
            self.confirm_end = end;
        }
        // The widths as dragged, the column dropped on another header, and
        // the sort clicked: all saved in the settings.
        for (c, w, _) in &drawn {
            set_column_width_in(&cols, &mut table_layout, *c, *w);
        }
        if let Some((from, x)) = dropped {
            if let Some((onto, _, _)) = drawn.iter().find(|(_, _, span)| span.contains(x)) {
                move_column_in(&cols, &mut table_layout, from, *onto);
            }
        }
        if let Some(c) = new_sort {
            let now = if sort.0 == c { (c, !sort.1) } else { (c, starts_descending(c)) };
            set_proc_sort_in(&cols, &mut table_layout, now);
        }
        self.settings.process_table = table_layout;
        if let Some((pid, pick)) = new_sel {
            self.selected.click(&view, pid, pick);
            // Picking another row stops following.
            if self.following.as_ref().is_some_and(|f| f.pid != pid || pick != Pick::One) {
                self.following = None;
            }
        }
        if let Some(s) = service_sel {
            self.selected = s;
            self.following = None;
        }
        if let Some((pid, open)) = toggle_row {
            if open {
                self.proc_view.expanded.insert(pid);
            } else {
                self.proc_view.expanded.remove(&pid);
            }
        }
        if let Some(g) = toggle_group {
            self.proc_view.toggle_group(g);
        }
        // "Force quit" and "End process tree" ask first: move them into the
        // confirmation dialog.
        let mut ask = None;
        actions.retain(|a| match EndAsk::before(a) {
            Some(q) => {
                ask = Some(q);
                false
            }
            None => true,
        });
        if let Some(q) = ask {
            self.confirm_end = Some(q);
        }
        // Set priority ▸ Realtime asks first.
        let mut realtime = Vec::new();
        actions.retain(|a| match a {
            Action::Priority(pid, "Realtime") => {
                realtime.push(*pid);
                false
            }
            _ => true,
        });
        if !realtime.is_empty() {
            self.confirm_realtime = Some(realtime);
        }
        // Efficiency mode asks first while "Ask before applying Efficiency
        // mode" is on.
        if self.settings.ask_efficiency {
            actions.retain(|a| match a {
                Action::Efficiency(pids) => {
                    self.confirm_efficiency = Some(EfficiencyAsk { pids: pids.clone(), dont_ask: false });
                    false
                }
                _ => true,
            });
        }
    }

    pub fn page_system(&mut self, ui: &mut Ui) {
        self.header(ui, "System Info");
        let t = theme::of_ui(ui);
        let s = &self.snap;
        let computer = sysinfo::System::host_name().unwrap_or_default();
        let os = sysinfo::System::long_os_version().unwrap_or_default();
        let fields = vec![
            ("Computer name", computer.clone()),
            ("Operating system", os.clone()),
            ("Version", sysinfo::System::os_version().unwrap_or_default()),
            ("Kernel", sysinfo::System::kernel_version().unwrap_or_default()),
            ("Architecture", sysinfo::System::cpu_arch()),
            ("Processor", cpu_brand()),
            ("Cores", sysinfo::System::physical_core_count().map(|c| c.to_string()).unwrap_or_default()),
            ("Logical processors", s.cpu_per_core.len().to_string()),
            ("Speed", format!("{:.2} GHz", s.cpu_freq_mhz as f64 / 1000.0)),
            ("Installed memory", fmt_bytes(s.mem_total)),
            ("Page file", fmt_bytes(s.swap_total)),
            ("Drives", s.disks.len().to_string()),
            ("Network adapters", s.net.len().to_string()),
            ("Up time", fmt_uptime(s.uptime)),
        ];
        let paused = self.settings.interval_ms() == 0;
        let providers = crate::app::ProviderState::of(s);
        let [state, procs, ip, generation] = status_parts(paused, providers, s.procs.len(), &local_ip(), s.seq);
        let sub = [procs, ip, generation, computer, os].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            StatusBand::new(Mark::Dot(health_colour(&t, paused, providers)), &state).sub(&sub).show(ui, |_, _| {});
            for section in system_sections(&fields) {
                ui.add_space(14.0);
                parts::field_card(ui, section.group, section.title, &section.fields);
            }
        });
    }

    pub fn page_users(&mut self, ui: &mut Ui) {
        self.header(ui, "Users");
        let t = self.settings.theme();
        let accounts = accounts(&self.snap.procs);
        let n: usize = accounts.iter().map(|a| a.procs.len()).sum();
        let cpu: f32 = accounts.iter().map(|a| a.cpu).sum();
        let mem: u64 = accounts.iter().map(|a| a.mem).sum();
        let sub = format!("{n} processes \u{b7} CPU {cpu:.1}% \u{b7} {} memory", fmt_bytes(mem));
        StatusBand::new(Mark::None, &users_headline(&accounts)).sub(&sub).show(ui, |_, _| {});
        ui.add_space(14.0);

        // The chosen account (its details beside the list) and the expanded
        // ones are kept in egui's memory.
        let chosen_id = egui::Id::new("users-chosen");
        let before: Option<String> = ui.data_mut(|d| d.get_persisted(chosen_id)).flatten();
        let chosen = parts::list_with_panel(
            ui,
            |ui, list_w| {
                let mut chosen = before;
                user_head(ui, &t, list_w);
                egui::ScrollArea::vertical().id_salt("users-list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for a in &accounts {
                        let open_id = egui::Id::new(("users-open", &a.name));
                        let mut open: bool = ui.data_mut(|d| d.get_persisted(open_id)).unwrap_or(false);
                        let on = chosen.as_deref() == Some(a.name.as_str());
                        if account_row(ui, &t, list_w - 12.0, a, on, open).clicked() {
                            open = !open || !on;
                            chosen = Some(a.name.clone());
                            ui.data_mut(|d| d.insert_persisted(open_id, open));
                        }
                        if open {
                            for p in a.procs.iter().take(200) {
                                member_row(ui, &t, list_w - 12.0, p);
                            }
                        }
                    }
                });
                chosen
            },
            |ui, chosen| {
                let item = chosen.as_ref().and_then(|c| accounts.iter().find(|a| &a.name == c));
                SidePanel::new("user-detail").show(ui, item, |ui, a| account_detail(ui, &t, a));
            },
        );
        ui.data_mut(|d| d.insert_persisted(chosen_id, chosen));
    }
}

/// A column of the Processes table: its key in the settings, its title,
/// its default width and whether a fresh layout hides it.
#[derive(Clone, Copy, Debug)]
pub struct ProcColumn {
    pub col: ProcCol,
    /// The name the settings save it by (`ProcessTable`); never changes.
    pub key: &'static str,
    pub title: &'static str,
    /// The column's default width, in points.
    pub width: f32,
    /// Hidden until the user shows it (the additions are, the standard
    /// columns are not).
    pub hidden: bool,
}

const fn column(col: ProcCol, key: &'static str, title: &'static str, width: f32) -> ProcColumn {
    ProcColumn { col, key, title, width, hidden: false }
}

/// A marked addition's column: after the standard ones, hidden until shown.
const fn addition(col: ProcCol, key: &'static str, title: &'static str, width: f32) -> ProcColumn {
    ProcColumn { col, key, title, width, hidden: true }
}

/// The Processes table's standard columns, left to right, with their titles
/// and default widths, then the marked additions (Publisher, Description),
/// hidden by default.
/// A new column is one more row here: the layout, the chooser and the saved
/// settings follow.
pub const PROC_COLUMNS: [ProcColumn; 19] = [
    column(ProcCol::Name, "name", "Name", 165.0),
    column(ProcCol::Pid, "pid", "PID", 50.0),
    column(ProcCol::Status, "status", "Status", 70.0),
    column(ProcCol::User, "user", "User name", 85.0),
    column(ProcCol::Cpu, "cpu", "CPU", 105.0),
    column(ProcCol::Mem, "memory", "Memory", 90.0),
    column(ProcCol::DiskRead, "disk_read", "Disk read", 70.0),
    column(ProcCol::DiskWrite, "disk_write", "Disk write", 70.0),
    column(ProcCol::Threads, "threads", "Threads", 55.0),
    column(ProcCol::Gpu, "gpu", "GPU", 60.0),
    column(ProcCol::PeakMem, "peak_memory", "Peak memory", 90.0),
    column(ProcCol::CpuTime, "cpu_time", "CPU time", 75.0),
    column(ProcCol::PageFaults, "page_faults", "Page faults", 75.0),
    column(ProcCol::Handles, "handles", "Handles", 60.0),
    column(ProcCol::Priority, "priority", "Priority", 85.0),
    column(ProcCol::Arch, "architecture", "Architecture", 70.0),
    column(ProcCol::Cmd, "command", "Command", 135.0),
    addition(ProcCol::Publisher, "publisher", "Publisher", 130.0),
    addition(ProcCol::Description, "description", "Description", 160.0),
];

/// The narrowest a column can be dragged.
const MIN_COLUMN_WIDTH: f32 = 40.0;

/// The column that cannot be hidden: a row needs a name.
const ALWAYS_SHOWN: ProcCol = ProcCol::Name;

/// One column as the table lays it out now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placed {
    pub col: ProcCol,
    pub shown: bool,
    pub width: f32,
}

fn column_of(columns: &[ProcColumn], col: ProcCol) -> Option<&ProcColumn> {
    columns.iter().find(|c| c.col == col)
}

/// The default width of a column an add-on adds.
const ADDON_COLUMN_WIDTH: f32 = 90.0;

/// The Processes table's columns: the standard ones, then the columns of the
/// add-ons that are on (`(add-on id, column)`), hidden until the user shows
/// them. An add-on's column is saved by its add-on's id.
pub fn columns_with(addons: &[(&'static str, crate::addons::ProcessColumn)]) -> Vec<ProcColumn> {
    PROC_COLUMNS
        .iter()
        .map(|c| ProcColumn { ..*c })
        .chain(addons.iter().map(|(id, c)| ProcColumn { col: ProcCol::Addon(id), key: id, title: c.title, width: ADDON_COLUMN_WIDTH, hidden: true }))
        .collect()
}

/// Every column of `columns`, hidden ones too, in the user's order, with
/// whether it shows and its width. Columns the saved layout has never seen
/// (added by a later version) take the slot after their neighbour in
/// `columns` and their default visibility; keys this version does not know
/// are skipped.
pub fn layout_of(columns: &[ProcColumn], saved: &crate::settings::ProcessTable) -> Vec<Placed> {
    let mut order: Vec<&ProcColumn> = Vec::new();
    for key in &saved.column_order {
        if let Some(c) = columns.iter().find(|c| c.key == key) {
            if !order.iter().any(|o| o.key == c.key) {
                order.push(c);
            }
        }
    }
    for (i, c) in columns.iter().enumerate() {
        if order.iter().any(|o| o.key == c.key) {
            continue;
        }
        // After the nearest earlier column of the table that is placed.
        let at = columns[..i].iter().rev().find_map(|prev| order.iter().position(|o| o.key == prev.key)).map_or(0, |p| p + 1);
        order.insert(at, c);
    }
    order
        .into_iter()
        .map(|c| {
            let seen = saved.column_order.iter().any(|k| k == c.key);
            let hidden = if seen { saved.hidden_columns.iter().any(|k| k == c.key) } else { c.hidden };
            let width = saved.column_widths.get(c.key).copied().unwrap_or(c.width).max(MIN_COLUMN_WIDTH);
            Placed { col: c.col, shown: !hidden || c.col == ALWAYS_SHOWN, width }
        })
        .collect()
}

/// The Processes table's layout from the saved settings.
#[cfg(test)]
pub fn proc_layout(saved: &crate::settings::ProcessTable) -> Vec<Placed> {
    layout_of(&PROC_COLUMNS, saved)
}

/// Writes `layout` (every column, in order) into the saved settings,
/// keeping the keys of columns this version does not know.
fn store_layout(columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable, layout: &[Placed]) {
    let known = |k: &String| columns.iter().any(|c| c.key == k);
    let key = |p: &Placed| column_of(columns, p.col).map(|c| c.key.to_string());
    let unknown_order: Vec<String> = saved.column_order.iter().filter(|k| !known(k)).cloned().collect();
    let unknown_hidden: Vec<String> = saved.hidden_columns.iter().filter(|k| !known(k)).cloned().collect();
    saved.column_order = layout.iter().filter_map(key).chain(unknown_order).collect();
    saved.hidden_columns = layout.iter().filter(|p| !p.shown).filter_map(key).chain(unknown_hidden).collect();
}

/// Hides or shows a column. A column shown again keeps its old slot; the
/// Name column always shows.
pub fn set_column_shown_in(columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable, col: ProcCol, shown: bool) {
    if col == ALWAYS_SHOWN && !shown {
        return;
    }
    let mut layout = layout_of(columns, saved);
    if let Some(p) = layout.iter_mut().find(|p| p.col == col) {
        p.shown = shown;
    }
    store_layout(columns, saved, &layout);
}

/// [`set_column_shown_in`] for the Processes table.
#[cfg(test)]
pub fn set_column_shown(saved: &mut crate::settings::ProcessTable, col: ProcCol, shown: bool) {
    set_column_shown_in(&PROC_COLUMNS, saved, col, shown);
}

/// Moves column `from` to the slot of column `onto`, as dropping a dragged
/// header on another one does: moving right it lands after `onto`, moving
/// left before it.
#[cfg(test)]
pub fn move_column(saved: &mut crate::settings::ProcessTable, from: ProcCol, onto: ProcCol) {
    move_column_in(&PROC_COLUMNS, saved, from, onto);
}

/// [`move_column`] among `columns`.
fn move_column_in(columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable, from: ProcCol, onto: ProcCol) {
    let mut layout = layout_of(columns, saved);
    let (Some(a), Some(b)) = (layout.iter().position(|p| p.col == from), layout.iter().position(|p| p.col == onto)) else {
        return;
    };
    if a == b {
        return;
    }
    let p = layout.remove(a);
    layout.insert(b, p);
    store_layout(columns, saved, &layout);
}

/// Saves the width a column was resized to (half a point or more off what
/// is saved).
#[cfg(test)]
fn set_column_width(saved: &mut crate::settings::ProcessTable, col: ProcCol, width: f32) {
    set_column_width_in(&PROC_COLUMNS, saved, col, width);
}

/// [`set_column_width`] for a column of `columns`.
fn set_column_width_in(columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable, col: ProcCol, width: f32) {
    let Some(c) = column_of(columns, col) else { return };
    let now = saved.column_widths.get(c.key).copied().unwrap_or(c.width).max(MIN_COLUMN_WIDTH);
    if (now - width).abs() >= 0.5 {
        saved.column_widths.insert(c.key.to_string(), width.round());
    }
}

/// The saved sort: a column and whether largest first. A column this
/// version does not know sorts by Memory, largest first.
#[cfg(test)]
pub fn proc_sort(saved: &crate::settings::ProcessTable) -> (ProcCol, bool) {
    proc_sort_in(&PROC_COLUMNS, saved)
}

/// [`proc_sort`] among `columns` (the column of an add-on that is off is
/// not among them: Memory then).
fn proc_sort_in(columns: &[ProcColumn], saved: &crate::settings::ProcessTable) -> (ProcCol, bool) {
    match columns.iter().find(|c| c.key == saved.sort_column) {
        Some(c) => (c.col, saved.sort_direction != "ascending"),
        None => (ProcCol::Mem, true),
    }
}

#[cfg(test)]
fn set_proc_sort(saved: &mut crate::settings::ProcessTable, sort: (ProcCol, bool)) {
    set_proc_sort_in(&PROC_COLUMNS, saved, sort);
}

fn set_proc_sort_in(columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable, (col, desc): (ProcCol, bool)) {
    if let Some(c) = column_of(columns, col) {
        saved.sort_column = c.key.to_string();
        saved.sort_direction = if desc { "descending" } else { "ascending" }.to_string();
    }
}

/// One line of the column chooser: a column of the table, in the table's
/// own order, ticked while it shows.
#[derive(Debug, PartialEq)]
pub struct ColumnChoice {
    pub col: ProcCol,
    pub title: &'static str,
    pub shown: bool,
    /// False for the column that always shows.
    pub enabled: bool,
}

/// The column chooser's lines (header right-click).
#[cfg(test)]
pub fn column_choices(saved: &crate::settings::ProcessTable) -> Vec<ColumnChoice> {
    column_choices_in(&PROC_COLUMNS, saved)
}

/// [`column_choices`] for `columns`: the add-ons' columns come last.
pub fn column_choices_in(columns: &[ProcColumn], saved: &crate::settings::ProcessTable) -> Vec<ColumnChoice> {
    let layout = layout_of(columns, saved);
    columns
        .iter()
        .map(|c| ColumnChoice {
            col: c.col,
            title: c.title,
            shown: layout.iter().any(|p| p.col == c.col && p.shown),
            enabled: c.col != ALWAYS_SHOWN,
        })
        .collect()
}

/// "Columns 16 of 17": how many columns show.
#[cfg(test)]
fn columns_label(saved: &crate::settings::ProcessTable) -> String {
    columns_label_in(&PROC_COLUMNS, saved)
}

fn columns_label_in(columns: &[ProcColumn], saved: &crate::settings::ProcessTable) -> String {
    let layout = layout_of(columns, saved);
    format!("Columns {} of {}", layout.iter().filter(|p| p.shown).count(), layout.len())
}

/// Draws the column chooser; a ticked or unticked line hides or shows its
/// column in `saved`.
fn column_menu(ui: &mut Ui, columns: &[ProcColumn], saved: &mut crate::settings::ProcessTable) {
    for choice in column_choices_in(columns, saved) {
        let mut on = choice.shown;
        if ui.add_enabled(choice.enabled, egui::Checkbox::new(&mut on, choice.title)).changed() {
            set_column_shown_in(columns, saved, choice.col, on);
        }
    }
}

/// Draws one process's cell of line `l` in column `col`: CPU, Memory,
/// Disk, GPU and Peak memory shaded by load, the CPU cell lit green while
/// the row is lit, Memory, Disk and Peak memory as shares of `totals`
/// with `percents` (Resource values), and an exited process's GPU
/// "Unavailable". Publisher and Description show the program file's
/// `files` texts.
#[allow(clippy::too_many_arguments)]
fn proc_cell(ui: &mut Ui, col: ProcCol, l: &ProcessLine, on: bool, totals: &Totals, percents: bool, figures: &Figures, files: &FileTexts) {
    let p = &l.row;
    let mem_total = totals.mem.max(1);
    // The shading of the columns of load.
    match col {
        ProcCol::Cpu if l.lit => parts::lit_cell(ui),
        ProcCol::Cpu => parts::heat_cell(ui, p.cpu / 100.0 * 4.0),
        ProcCol::Mem => parts::heat_cell(ui, p.mem as f32 / mem_total as f32 * 10.0),
        ProcCol::DiskRead => parts::heat_cell(ui, (p.disk_read / 1_048_576.0) as f32 / 20.0),
        ProcCol::DiskWrite => parts::heat_cell(ui, (p.disk_write / 1_048_576.0) as f32 / 20.0),
        ProcCol::Gpu if !l.exited => parts::heat_cell(ui, p.gpu / 100.0 * 4.0),
        ProcCol::PeakMem => parts::heat_cell(ui, p.peak_mem as f32 / mem_total as f32 * 10.0),
        _ => {}
    }
    let soft = matches!(col, ProcCol::Status | ProcCol::Priority | ProcCol::Arch | ProcCol::Cmd | ProcCol::Publisher | ProcCol::Description) || (col == ProcCol::Gpu && l.exited);
    cell_text(ui, &cell_value(col, l, totals, percents, figures, files), if soft { CellInk::Soft } else { CellInk::Main }, on);
}

/// The words of line `l`'s cell in column `col`, as the table and the CSV
/// export write them: Memory, Disk and Peak memory as shares of `totals`
/// with `percents` (Resource values), an exited process's GPU
/// "Unavailable".
fn cell_value(col: ProcCol, l: &ProcessLine, totals: &Totals, percents: bool, figures: &Figures, files: &FileTexts) -> String {
    let p = &l.row;
    // A figure as a percent with Resource values ▸ Percents, else `value`.
    let shown = |value: String| match percent(col, p, totals) {
        Some(share) if percents => format!("{share:.1}%"),
        _ => value,
    };
    match col {
        ProcCol::Name => p.name.clone(),
        ProcCol::Pid => p.pid.to_string(),
        ProcCol::Status => p.status.clone(),
        ProcCol::User => p.user.clone(),
        ProcCol::Cpu => format!("{:.1}%", p.cpu),
        ProcCol::Mem => shown(fmt_mb(p.mem)),
        ProcCol::DiskRead => shown(fmt_bytes_rate(p.disk_read)),
        ProcCol::DiskWrite => shown(fmt_bytes_rate(p.disk_write)),
        ProcCol::Threads => p.threads.to_string(),
        ProcCol::Gpu if l.exited => "Unavailable".into(),
        ProcCol::Gpu => format!("{:.1}%", p.gpu),
        ProcCol::PeakMem => shown(fmt_mb(p.peak_mem)),
        ProcCol::CpuTime => fmt_cpu_time(p.cpu_time),
        ProcCol::PageFaults => p.page_faults.to_string(),
        ProcCol::Handles => p.handles.to_string(),
        ProcCol::Priority => p.priority.to_string(),
        ProcCol::Arch => p.arch.to_string(),
        ProcCol::Cmd => p.cmd.clone(),
        ProcCol::Publisher | ProcCol::Description => files.of(col, &p.exe).to_string(),
        ProcCol::Addon(id) => figure_of(figures, id, p.pid).map_or(String::new(), |n| n.to_string()),
    }
}

/// The CSV text of the process `lines`: a header of the column titles
/// shown, in the user's order, then one row per line as the table writes
/// it (a field a line leaves empty is empty; fields are quoted as CSV
/// needs).
#[allow(clippy::too_many_arguments)]
pub fn process_csv<'a>(lines: impl Iterator<Item = &'a ProcessLine>, columns: &[ProcColumn], saved: &crate::settings::ProcessTable, totals: &Totals, percents: bool, figures: &Figures, files: &FileTexts) -> String {
    use crate::platform::csv_field;
    let shown: Vec<&ProcColumn> = layout_of(columns, saved).iter().filter(|p| p.shown).filter_map(|p| column_of(columns, p.col)).collect();
    let mut out = shown.iter().map(|c| csv_field(c.title)).collect::<Vec<_>>().join(",");
    out.push('\n');
    for l in lines {
        let fields: Vec<String> = shown.iter().map(|c| if l.shows(c.col) { csv_field(&cell_value(c.col, l, totals, percents, figures, files)) } else { String::new() }).collect();
        out.push_str(&fields.join(","));
        out.push('\n');
    }
    out
}

/// An item of the Processes header's "Process View ⌄" menu. It holds
/// five, in this order: Process tree, Smooth CPU readings, Show exited
/// processes, a separator, Expand all, Collapse all.
/// Each arrives with its ticket (#165 tree and Expand / Collapse all, #166
/// exited processes, #167 smoothing) as a variant here, a line in
/// [`TaskApp::process_view_items`] and an arm in
/// [`TaskApp::apply_process_view`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ProcView {
    /// Children under their parent, or a flat list (saved).
    Tree,
    /// Each process's CPU as a moving average of its readings (saved).
    Smooth,
    /// Processes that end stay a short while, tinted (saved).
    ShowExited,
    /// Opens every tree row; greyed while the tree is off.
    ExpandAll,
    /// Closes every tree row; greyed while the tree is off.
    CollapseAll,
}

impl TaskApp {
    /// What the Publisher and Description columns show, for the program
    /// files of the live and exited processes, while each column is among
    /// the `shown`. Asks the caches for the files not read yet (off the UI
    /// thread); those show nothing until read.
    fn file_texts(&self, shown: &[Placed]) -> FileTexts {
        let mut files = FileTexts::default();
        let on = |col| shown.iter().any(|p| p.col == col);
        let exes = || self.snap.procs.iter().chain(self.proc_view.exited_rows()).map(|p| p.exe.as_str());
        if on(ProcCol::Publisher) {
            self.signatures.ask(exes());
            files.publisher = exes().filter_map(|e| Some((e.to_string(), crate::signature::publisher(self.signatures.of(e)?)))).collect();
        }
        if on(ProcCol::Description) {
            self.descriptions.ask(exes());
            files.description = exes().filter_map(|e| Some((e.to_string(), self.descriptions.of(e).flatten()?))).collect();
        }
        files
    }

    /// What the "Process View ⌄" menu lists now, top to bottom.
    fn process_view_items(&self) -> Vec<parts::DropItem<ProcView>> {
        let tree = self.settings.process_tree;
        let item = |id, label, checked, enabled| parts::DropItem::Item { id, label, checked, enabled };
        vec![
            item(ProcView::Tree, "Process tree", Some(tree), true),
            item(ProcView::Smooth, "Smooth CPU readings", Some(self.settings.smooth_cpu), true),
            item(ProcView::ShowExited, "Show exited processes", Some(self.settings.show_exited), true),
            parts::DropItem::Separator,
            item(ProcView::ExpandAll, "Expand all", None, tree),
            item(ProcView::CollapseAll, "Collapse all", None, tree),
        ]
    }

    /// Does what the Process View item clicked asks.
    pub(crate) fn apply_process_view(&mut self, item: ProcView) {
        match item {
            ProcView::Tree => {
                self.settings.process_tree = !self.settings.process_tree;
                self.settings.save();
            }
            ProcView::Smooth => {
                self.settings.smooth_cpu = !self.settings.smooth_cpu;
                self.settings.save();
            }
            ProcView::ShowExited => {
                self.settings.show_exited = !self.settings.show_exited;
                if !self.settings.show_exited {
                    self.proc_view.drop_exited();
                }
                self.settings.save();
            }
            ProcView::ExpandAll => {
                self.proc_view.expand_all(&self.snap.procs);
                // A host of services opens too.
                self.proc_view.expanded.extend(self.snap.services.keys());
            }
            ProcView::CollapseAll => self.proc_view.collapse_all(),
        }
    }
}

/// Whether a first click on the column sorts the largest first: the
/// columns of amounts do, the columns of words do not.
fn starts_descending(col: ProcCol) -> bool {
    !matches!(col, ProcCol::Name | ProcCol::Pid | ProcCol::Status | ProcCol::User | ProcCol::Arch | ProcCol::Cmd | ProcCol::Publisher | ProcCol::Description)
}

/// What the Processes band says: how many run, how many the filter shows,
/// and the busiest processes now.
#[derive(Debug, PartialEq)]
struct ProcessesBand {
    headline: String,
    /// "12 shown for "chrome"" while a filter is typed.
    shown: Option<String>,
    most_cpu: Option<String>,
    most_memory: Option<String>,
}

impl ProcessesBand {
    fn of(procs: &[ProcRow], shown: usize, query: &str) -> Self {
        let real = || procs.iter().filter(|p| p.pid != 0);
        let n = real().count();
        let headline = match n {
            1 => "1 process is running".to_string(),
            n => format!("{n} processes are running"),
        };
        let most_cpu = real()
            .filter(|p| p.cpu > 0.0)
            .max_by(|a, b| a.cpu.partial_cmp(&b.cpu).unwrap_or(std::cmp::Ordering::Equal))
            .map(|p| format!("{}, {:.1}%", p.name, p.cpu));
        let most_memory = real().filter(|p| p.mem > 0).max_by_key(|p| p.mem).map(|p| format!("{}, {}", p.name, fmt_bytes(p.mem)));
        let shown = (!query.is_empty()).then(|| format!("{shown} shown for \u{201c}{query}\u{201d}"));
        ProcessesBand { headline, shown, most_cpu, most_memory }
    }
}

/// How many of the busiest processes Summary lists.
pub(crate) const SUMMARY_TOP: usize = 25;
/// The width of Summary's process list beside the graphs.
const SUMMARY_ASIDE: f32 = 340.0;

/// The CPU Overview card's body: utilization with the kernel time under it
/// (in Performance's kernel colour), their legend, the per-core squares
/// and the "N logical processors · Speed X GHz" footer.
fn cpu_overview(ui: &mut Ui, t: &Theme, view: &crate::summary_view::SummaryView, color: Color32) {
    let kernel = t.graphs.kernel;
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 96.0), Sense::hover());
    paint_graph(ui.painter(), rect, &[], view.cpu.max, color, true);
    paint_series(ui.painter(), rect, &view.cpu_kernel, view.cpu.max, kernel);
    paint_series(ui.painter(), rect, &view.cpu.history, view.cpu.max, color);
    ui.horizontal(|ui| {
        parts::legend_item(ui, "Utilization", color);
        ui.add_space(8.0);
        parts::legend_item(ui, "Kernel", kernel);
    });
    core_squares(ui, &view.per_core, color);
    ui.label(RichText::new(&view.cpu_footer).font(t.fonts.label.clone()).color(t.label));
}

/// Summary's band line: "CPU 15.4% at 4.90 GHz · memory 40.5% · 389
/// processes" (no speed without a reading).
pub(crate) fn summary_sub(cpu: f32, freq_mhz: u64, memory: f64, procs: usize) -> String {
    let speed = if freq_mhz > 0 { format!(" at {}", crate::metrics::fmt_ghz(freq_mhz as f32)) } else { String::new() };
    let procs = if procs == 1 { "1 process".to_string() } else { format!("{procs} processes") };
    format!("CPU {cpu:.1}%{speed} \u{b7} memory {memory:.1}% \u{b7} {procs}")
}

/// The `n` busiest processes by CPU (more memory first on a tie), without
/// the idle process (pid 0).
pub(crate) fn top_by_cpu(procs: &[ProcRow], n: usize) -> Vec<ProcRow> {
    let mut top: Vec<&ProcRow> = procs.iter().filter(|p| p.pid != 0).collect();
    top.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal).then(b.mem.cmp(&a.mem)));
    top.into_iter().take(n).cloned().collect()
}

/// Summary's aside: the busiest processes in a panel, PID, name, CPU and
/// memory, under a display-face heading in the CPU colour (the link blue in the light
/// theme), as on board P01.
fn top_processes(ui: &mut Ui, t: &Theme, width: f32, top: &[ProcRow]) {
    egui::Frame::new()
        .fill(t.panel)
        .stroke(egui::Stroke::new(1.0, t.line))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(width - 32.0);
            // The CPU colour is made for dark surfaces; the link blue reads on the light panel.
            let heading_ink = if t.mode == theme::Mode::Dark { t.graphs.cpu } else { t.link };
            let heading = egui::text::LayoutJob::single_section(
                "TOP CPU PROCESSES".into(),
                egui::TextFormat { font_id: display_font(ui, 15.0), color: heading_ink, extra_letter_spacing: 2.0, ..Default::default() },
            );
            ui.label(heading);
            ui.add_space(4.0);
            let columns = [50.0, 0.0, 46.0, 66.0];
            let row = |ui: &mut Ui, cells: [(&str, Color32); 4], size: f32| {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), size + 10.0), Sense::hover());
                let name_w = rect.width() - columns[0] - columns[2] - columns[3] - 18.0;
                let mut x = rect.left();
                for (i, (text, ink)) in cells.into_iter().enumerate() {
                    let w = if i == 1 { name_w } else { columns[i] };
                    let galley = fitted(ui, text, size, ink, false, w);
                    let right = i != 1;
                    let at = if right { x + w - galley.size().x } else { x };
                    ui.painter().galley(egui::pos2(at, rect.center().y - galley.size().y / 2.0), galley, ink);
                    x += w + 6.0;
                }
                rect
            };
            let head = row(ui, [("PID", t.label), ("Name", t.label), ("CPU", t.label), ("Memory", t.label)], 11.5);
            ui.painter().hline(head.x_range(), head.bottom(), egui::Stroke::new(1.0, t.line));
            for p in top {
                let (pid, cpu, mem) = (p.pid.to_string(), format!("{:.1}%", p.cpu), fmt_mb(p.mem));
                row(ui, [(&pid, t.label), (&p.name, t.text), (&cpu, t.soft), (&mem, t.soft)], 12.0);
            }
        });
}

/// The rows of the selected processes that still run, in the snapshot's
/// order.
pub fn rows_of<'a>(procs: &'a [ProcRow], selected: &Selection) -> Vec<&'a ProcRow> {
    procs.iter().filter(|p| selected.contains(p.pid)).collect()
}

/// Which end the user is asked to agree to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EndKind {
    /// "End task" in the detail panel: a graceful end of one process.
    Task,
    /// "Force quit" in the menu: a forced end of one process.
    ForceQuit,
    /// "End process tree" in the menu: a graceful end of the process and
    /// every process it started.
    Tree,
}

/// The processes to end once the user agrees: "End task" in the detail
/// panel or the Delete key, "Force quit" or "End process tree" in the menu,
/// and any end of several selected rows.
#[derive(Clone, Debug, PartialEq)]
pub struct EndAsk {
    pub pids: Vec<u32>,
    /// The process's name when there is one; empty for several.
    pub name: String,
    pub kind: EndKind,
}

impl EndAsk {
    /// "End task" for the selected `rows` (the detail panel's button and
    /// the Delete key); None with nothing selected.
    pub fn task(rows: &[&ProcRow]) -> Option<EndAsk> {
        let name = match rows {
            [] => return None,
            [p] => p.name.clone(),
            _ => String::new(),
        };
        Some(EndAsk { pids: rows.iter().map(|p| p.pid).collect(), name, kind: EndKind::Task })
    }

    /// The question a menu end asks before it runs, if it asks one. For one
    /// process "End task" and "Force quit process tree" run as they are;
    /// for several only "Force quit process tree" does.
    pub fn before(a: &Action) -> Option<EndAsk> {
        let (pids, name, force, tree) = match a {
            Action::End { pid, force, tree, name } => (vec![*pid], name.clone(), *force, *tree),
            Action::EndMany { pids, force, tree } => (pids.clone(), String::new(), *force, *tree),
            _ => return None,
        };
        let kind = match (force, tree) {
            (true, false) => EndKind::ForceQuit,
            (false, true) => EndKind::Tree,
            (false, false) if pids.len() > 1 => EndKind::Task,
            _ => return None,
        };
        Some(EndAsk { pids, name, kind })
    }

    fn many(&self) -> bool {
        self.pids.len() > 1
    }

    /// The dialog's title and its button: the menu item's label.
    pub fn title(&self) -> &'static str {
        let one = match self.kind {
            EndKind::Task => "End task",
            EndKind::ForceQuit => "Force quit",
            EndKind::Tree => "End process tree",
        };
        crate::widgets::menu_label(one, self.many())
    }

    pub fn question(&self) -> String {
        match self.kind {
            EndKind::Task if self.many() => format!("End {} selected processes?", self.pids.len()),
            EndKind::Task => format!("End {}?", self.name),
            EndKind::ForceQuit => "Force quit the selected task(s)?".into(),
            EndKind::Tree => "End the selected process tree(s)?".into(),
        }
    }

    pub fn note(&self) -> &'static str {
        match self.kind {
            EndKind::Task if self.many() => "Unsaved data in these processes may be lost.",
            EndKind::Task => "Unsaved data in this process may be lost.",
            // Force quit asks without a note.
            EndKind::ForceQuit => "",
            EndKind::Tree => "This ends the process and every process it started. Unsaved work may be lost.",
        }
    }

    /// What runs once the user agrees.
    pub fn action(&self) -> Action {
        let force = self.kind == EndKind::ForceQuit;
        let tree = self.kind == EndKind::Tree;
        match self.pids[..] {
            [pid] => Action::End { pid, force, tree, name: self.name.clone() },
            _ => Action::EndMany { pids: self.pids.clone(), force, tree },
        }
    }
}

/// Efficiency mode's question, waiting for the user's answer.
#[derive(Clone, Debug, PartialEq)]
pub struct EfficiencyAsk {
    pub pids: Vec<u32>,
    /// "Don't ask again" ticked.
    pub dont_ask: bool,
}

/// The question Efficiency mode asks before it runs.
pub const EFFICIENCY_QUESTION: &str = "Efficiency mode lowers this process's priority and power consumption. Continue?";

/// The question Set priority ▸ Realtime asks before it runs.
pub const REALTIME_QUESTION: &str = "Realtime priority can make the system unresponsive. Continue?";

impl EfficiencyAsk {
    /// The dialog's title and its button: the menu item's label.
    pub fn title(&self) -> &'static str {
        crate::widgets::menu_label("Efficiency mode", self.pids.len() > 1)
    }
}

/// One group of the details pane: its heading and its (label, value,
/// dimmed) fields.
type DetailGroup = (&'static str, Vec<(&'static str, String, bool)>);

/// A process's parent as the details pane and the Inspector show it: its
/// PID, or "9916 (exited)" (true: drawn dimmed) when it no longer runs.
pub(crate) fn parent_text(p: &ProcRow, procs: &[ProcRow]) -> (String, bool) {
    match p.parent {
        Some(pp) if procs.iter().any(|q| q.pid == pp) => (pp.to_string(), false),
        Some(pp) => (format!("{pp} (exited)"), true),
        None => (String::new(), false),
    }
}

/// The details pane's fields for one process, grouped as the approved
/// prototype groups the 22 fields (IDENTITY, PROCESSOR, MEMORY, I/O),
/// plus SERVICE on a service row. `procs` tells whether the parent still
/// runs: a gone parent reads "9916 (exited)", dimmed.
fn detail_groups(p: &ProcRow, procs: &[ProcRow]) -> Vec<DetailGroup> {
    let parent = parent_text(p, procs);
    let f = |k: &'static str, v: String| (k, v, false);
    let mut groups = vec![
        ("IDENTITY", vec![
            f("PID", p.pid.to_string()),
            ("Parent", parent.0, parent.1),
            f("Session", p.session.to_string()),
            f("Architecture", p.arch.to_string()),
            f("Image path", p.exe.clone()),
            f("Group", p.group.to_string()),
        ]),
        ("PROCESSOR", vec![
            f("CPU time", fmt_cpu_time(p.cpu_time)),
            f("Kernel time", fmt_cpu_time(p.kernel_time)),
            f("User time", fmt_cpu_time(p.user_time)),
            f("GPU", format!("{:.1}%", p.gpu)),
            f("Priority", p.priority.to_string()),
            f("Base priority", p.base_priority.to_string()),
        ]),
        ("MEMORY", vec![
            f("Working set", fmt_mb(p.working_set)),
            f("Peak working set", fmt_mb(p.peak_mem)),
            f("Commit size", fmt_mb(p.commit)),
            f("Page faults", p.page_faults.to_string()),
            f("Handles", p.handles.to_string()),
        ]),
        ("I/O", vec![
            f("I/O read", fmt_bytes(p.io_read)),
            f("I/O write", fmt_bytes(p.io_write)),
            f("I/O other", fmt_bytes(p.io_other)),
            f("I/O read ops", p.io_read_ops.to_string()),
            f("I/O write ops", p.io_write_ops.to_string()),
        ]),
    ];
    if let Some(s) = &p.service {
        groups.push(("SERVICE", vec![
            f("Service name", s.name.clone()),
            f("Display name", s.display_name.clone()),
            f("Description", s.description.clone()),
            f("Host process", s.host.clone()),
            f("Group", s.group.clone()),
            f("Registration", s.registration.clone()),
        ]));
    }
    groups
}

/// The details pane under the Processes table: with exactly one row in
/// `chosen`, its name and PID, its image path, End task and its grouped
/// fields; otherwise "Select a single entry for detail information.". True when End task was clicked: the page asks before
/// ending.
fn process_details(ui: &mut Ui, t: &Theme, chosen: &[&ProcRow], procs: &[ProcRow]) -> bool {
    let mut end = false;
    SidePanel::new("process-detail").show(ui, (chosen.len() == 1).then(|| chosen[0]), |ui, p| {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&p.name).font(t.fonts.figure_small.clone()).color(t.text));
            ui.label(RichText::new(format!("({})", p.pid)).font(t.fonts.figure_small.clone()).color(t.label));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                end = parts::destructive_button(ui, "End task", true).clicked();
            });
        });
        let program = if p.exe.is_empty() { &p.cmd } else { &p.exe };
        if !program.is_empty() {
            ui.add(egui::Label::new(RichText::new(program).font(t.fonts.label.clone()).color(t.label)).truncate());
        }
        let groups = detail_groups(p, procs);
        ui.columns(groups.len(), |cols| {
            for (i, (col, (heading, fields))) in cols.iter_mut().zip(groups.iter()).enumerate() {
                col.label(RichText::new(*heading).font(t.fonts.label.clone()).color(t.label));
                egui::Grid::new(("process-detail-group", i)).num_columns(2).spacing(vec2(8.0, 2.0)).show(col, |ui| {
                    for (k, v, dim) in fields {
                        ui.label(RichText::new(*k).font(t.fonts.label.clone()).color(t.label));
                        let ink = if *dim { t.soft } else { t.text };
                        ui.add(egui::Label::new(RichText::new(v.as_str()).font(t.fonts.row.clone()).color(ink)).truncate());
                        ui.end_row();
                    }
                });
            }
        });
    });
    end
}

/// A System Info card: its group, its title and its fields.
struct Section<'a> {
    group: &'a str,
    title: &'a str,
    fields: Vec<(&'a str, String)>,
}

/// Which card each System Info field sits in, as Design A groups them.
const SECTIONS: &[(&str, &str, &[&str])] = &[
    ("Software", "Operating System", &["Computer name", "Operating system", "Version", "Kernel", "Up time"]),
    ("Hardware", "Processor", &["Processor", "Architecture", "Cores", "Logical processors", "Speed"]),
    ("Hardware", "Memory", &["Installed memory", "Page file"]),
    ("Hardware", "Storage", &["Drives"]),
    ("Network", "Network", &["Network adapters"]),
];

/// The page's fields in their cards; a card with no field is left out.
fn system_sections<'a>(fields: &[(&'a str, String)]) -> Vec<Section<'a>> {
    SECTIONS
        .iter()
        .map(|(group, title, keys)| Section {
            group,
            title,
            fields: keys.iter().filter_map(|k| fields.iter().find(|(f, _)| f == k).cloned()).collect(),
        })
        .filter(|s| !s.fields.is_empty())
        .collect()
}

/// An account running processes, with its processes by memory.
#[derive(Debug)]
struct Account {
    name: String,
    cpu: f32,
    mem: u64,
    procs: Vec<ProcRow>,
}

/// The accounts that run processes, by name; processes without a user
/// are "System".
fn accounts(procs: &[ProcRow]) -> Vec<Account> {
    let mut by: BTreeMap<String, Account> = BTreeMap::new();
    for p in procs {
        let name = if p.user.is_empty() { "System".to_string() } else { p.user.clone() };
        let a = by.entry(name.clone()).or_insert_with(|| Account { name, cpu: 0.0, mem: 0, procs: Vec::new() });
        a.cpu += p.cpu;
        a.mem += p.mem;
        a.procs.push(p.clone());
    }
    let mut all: Vec<Account> = by.into_values().collect();
    for a in &mut all {
        a.procs.sort_by(|x, y| y.mem.cmp(&x.mem));
    }
    all
}

/// The Users band's headline.
fn users_headline(accounts: &[Account]) -> String {
    match accounts {
        [] => "No user sessions were reported".to_string(),
        [one] => format!("Only {} is running processes", one.name),
        many => format!("{} accounts are running processes", many.len()),
    }
}

/// x of each Users column from the right edge: CPU, Memory, Processes.
const USER_COLUMNS: [f32; 3] = [200.0, 110.0, 12.0];

fn user_head(ui: &mut Ui, t: &Theme, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width - 12.0, 24.0), Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    p.text(egui::pos2(rect.left() + 12.0, y), egui::Align2::LEFT_CENTER, "User", t.fonts.label.clone(), t.label);
    for (label, x) in ["CPU", "Memory", "Processes"].iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, *label, t.fonts.label.clone(), t.label);
    }
    p.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.line));
    ui.add_space(4.0);
}

/// An account's row: chevron, name and how many processes, its figures.
fn account_row(ui: &mut Ui, t: &Theme, width: f32, a: &Account, selected: bool, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 42.0), Sense::click());
    let p = ui.painter();
    parts::row_fill(p, rect, selected, resp.hovered());
    let ink = if selected { t.on_selected } else { t.text };
    let sub = parts::sub_colour(t, selected);
    let y = rect.center().y;
    // The chevron: down while the account's processes show, right otherwise.
    let c = egui::pos2(rect.left() + 17.0, y);
    crate::widgets::disclosure(p, c, open);
    p.text(egui::pos2(rect.left() + 30.0, rect.top() + 13.0), egui::Align2::LEFT_CENTER, &a.name, egui::FontId::new(13.0, theme::semibold()), ink);
    let count = match a.procs.len() {
        1 => "1 process".to_string(),
        n => format!("{n} processes"),
    };
    p.text(egui::pos2(rect.left() + 30.0, rect.top() + 30.0), egui::Align2::LEFT_CENTER, &count, egui::FontId::proportional(11.5), sub);
    let figures = [format!("{:.1}%", a.cpu), fmt_bytes(a.mem), a.procs.len().to_string()];
    for (f, x) in figures.iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, f, t.fonts.row.clone(), ink);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &a.name));
    resp
}

/// One process under an open account: name and PID, CPU and memory.
fn member_row(ui: &mut Ui, t: &Theme, width: f32, proc: &ProcRow) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let p = ui.painter();
    parts::row_fill(p, rect, false, resp.hovered());
    let y = rect.center().y;
    let name = p.text(egui::pos2(rect.left() + 40.0, y), egui::Align2::LEFT_CENTER, &proc.name, t.fonts.row.clone(), t.text);
    p.text(egui::pos2(name.right() + 8.0, y), egui::Align2::LEFT_CENTER, format!("PID {}", proc.pid), egui::FontId::proportional(11.5), t.faint);
    let figures = [format!("{:.1}%", proc.cpu), fmt_mb(proc.mem)];
    for (f, x) in figures.iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, f, t.fonts.row.clone(), t.soft);
    }
}

/// The chosen account beside the Users list: its name and its figures.
fn account_detail(ui: &mut Ui, t: &Theme, a: &Account) {
    ui.label(RichText::new(&a.name).font(t.fonts.figure.clone()).color(t.text));
    ui.add_space(8.0);
    ui.columns(3, |cols| {
        let figures = [
            ("CPU", format!("{:.1}%", a.cpu), t.graphs.cpu),
            ("Memory", fmt_bytes(a.mem), t.graphs.memory),
            ("Processes", a.procs.len().to_string(), t.text),
        ];
        for (ui, (label, value, ink)) in cols.iter_mut().zip(figures) {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).font(t.fonts.label.clone()).color(t.label));
            ui.label(RichText::new(value).font(t.fonts.figure_small.clone()).color(ink));
        }
    });
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, user: &str, cpu: f32, mem: u64) -> ProcRow {
        ProcRow { pid, name: name.into(), user: user.into(), cpu, mem, ..Default::default() }
    }

    /// Settings whose CPU colour is the user's own.
    pub(crate) fn custom_cpu() -> (crate::settings::Settings, Color32) {
        let mut settings = crate::settings::Settings::default();
        settings.cpu_color = [0x12, 0x34, 0x56];
        (settings, Color32::from_rgb(0x12, 0x34, 0x56))
    }

    #[test]
    fn a_custom_cpu_colour_reaches_summary() {
        let (settings, cpu) = custom_cpu();
        let mut app = TaskApp::headless(settings, crate::sampler::Snapshot::default());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1200.0, 800.0), vec![], |ui| app.page_summary(ui, &mut Vec::new()));
        assert!(crate::test_ui::colours(&out).contains(&cpu));
    }

    #[test]
    fn end_task_in_the_detail_panel_asks_before_ending() {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let t = Theme::of(theme::Mode::Dark);
        let p = proc(16244, "Code.exe", "spata", 8.0, 4_300_000_000);
        // The panel gets no action queue: End task can only ask.
        let draw = |events, asked: &mut bool| crate::test_ui::run(&ctx, (1200.0, 260.0), events, |ui| *asked |= process_details(ui, &t, &[&p], &[]));
        let mut asked = false;
        let out = draw(vec![], &mut asked);
        let button = crate::test_ui::flat_shapes(&out)
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(g) if g.galley.text() == "End task" => Some(g.visual_bounding_rect().center()),
                _ => None,
            })
            .expect("an End task button");
        let press = |pressed| egui::Event::PointerButton { pos: button, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        draw(vec![egui::Event::PointerMoved(button), press(true)], &mut asked);
        draw(vec![press(false)], &mut asked);
        assert!(asked, "the click asks for confirmation");
    }

    /// NordVPN.exe as the approved prototype shows it, its parent gone.
    fn nordvpn() -> ProcRow {
        ProcRow {
            parent: Some(9916),
            exe: r"C:\Program Files\NordVPN\NordVPN.exe".into(),
            session: 1,
            arch: "x64",
            group: 0,
            cpu_time: std::time::Duration::from_millis(321_010),
            kernel_time: std::time::Duration::from_millis(62_400),
            user_time: std::time::Duration::from_millis(258_610),
            gpu: 0.0,
            priority: "Normal",
            base_priority: 8,
            working_set: 620_550_144,
            peak_mem: 652_420_301,
            commit: 349_175_808,
            page_faults: 414_119,
            handles: 1642,
            io_read: 1_288_490_189,
            io_write: 327_155_712,
            io_other: 50_331_648,
            io_read_ops: 91_204,
            io_write_ops: 23_877,
            ..proc(26920, "NordVPN.exe", "spata", 0.0, 620_550_144)
        }
    }

    /// The details pane alone, for the rows given as selected.
    fn details(chosen: &[&ProcRow], procs: &[ProcRow]) -> Vec<String> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let t = Theme::of(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1400.0, 400.0), vec![], |ui| {
            process_details(ui, &t, chosen, procs);
        });
        crate::test_ui::texts(&out)
    }

    #[test]
    fn the_details_pane_draws_its_22_fields_for_a_selected_process() {
        let mut snap = crate::sampler::Snapshot { procs: vec![nordvpn()], mem_total: 16_000_000_000, ..Default::default() };
        snap.procs.push(proc(4, "System", "SYSTEM", 0.0, 0));
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        app.selected = Selection::only(26920);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 1000.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let texts = crate::test_ui::texts(&out);
        let has = |s: &str| texts.iter().any(|t| t == s);
        for label in [
            "PID", "Parent", "Session", "Architecture", "Image path", "CPU time", "Kernel time", "User time", "Working set", "Peak working set", "Commit size",
            "Page faults", "Handles", "I/O read", "I/O write", "I/O other", "I/O read ops", "I/O write ops", "GPU", "Priority", "Base priority", "Group",
        ] {
            assert!(has(label), "{label} in {texts:?}");
        }
        for group in ["IDENTITY", "PROCESSOR", "MEMORY", "I/O"] {
            assert!(has(group), "{group} heading in {texts:?}");
        }
        for value in ["9916 (exited)", "1:02.40", "4:18.61", "591.8 MB", "622.2 MB", "333.0 MB", "1.2 GB", "312.0 MB", "48.0 MB", "91204", "23877"] {
            assert!(has(value), "{value} in {texts:?}");
        }
        assert!(!has(parts::EMPTY_DETAIL), "a selected process shows its details");
    }

    #[test]
    fn the_details_pane_names_a_running_parent_without_exited() {
        let parent = proc(9916, "explorer.exe", "spata", 0.0, 0);
        let texts = details(&[&nordvpn()], &[parent, nordvpn()]);
        assert!(texts.iter().any(|t| t == "9916"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "9916 (exited)"));
    }

    #[test]
    fn the_details_pane_says_select_a_single_entry_for_none_or_several() {
        let (a, b) = (nordvpn(), proc(4, "System", "SYSTEM", 0.0, 0));
        for chosen in [vec![], vec![&a, &b]] {
            let texts = details(&chosen, &[a.clone(), b.clone()]);
            assert!(texts.iter().any(|t| t == "Select a single entry for detail information."), "{} selected: {texts:?}", chosen.len());
            assert!(!texts.iter().any(|t| t == "Kernel time"), "no fields with {} selected", chosen.len());
        }
        // The page with nothing selected says the same.
        let mut app = TaskApp::headless(crate::settings::Settings::default(), crate::sampler::Snapshot { procs: vec![a], ..Default::default() });
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 1000.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        assert!(crate::test_ui::texts(&out).iter().any(|t| t == parts::EMPTY_DETAIL));
    }

    #[test]
    fn a_service_row_adds_the_service_fields() {
        let service = ProcRow {
            service: Some(crate::sampler::ServiceInfo {
                name: "Winmgmt".into(),
                display_name: "Windows Management Instrumentation".into(),
                description: "Provides a common interface and object model.".into(),
                host: "svchost.exe (2148)".into(),
                group: "netsvcs".into(),
                registration: "Shared process".into(),
            }),
            ..nordvpn()
        };
        let texts = details(&[&service], &[]);
        let has = |s: &str| texts.iter().any(|t| t == s);
        for shown in ["SERVICE", "Service name", "Display name", "Description", "Host process", "Registration", "Winmgmt", "svchost.exe (2148)", "netsvcs", "Shared process"] {
            assert!(has(shown), "{shown} in {texts:?}");
        }
        assert_eq!(texts.iter().filter(|t| *t == "Group").count(), 2, "the process's Group and the service's Group");
        let plain = details(&[&nordvpn()], &[]);
        assert!(!plain.iter().any(|t| t == "Service name"), "a process row has no service fields");
    }

    /// What the menu's end item sends, and the question asked first, if any.
    fn menu_end(label: &str) -> (Action, Option<EndAsk>) {
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let item = crate::widgets::END_ITEMS.iter().find(|(_, l)| *l == label).expect("a menu item").0;
        let sent = item.action(&[&p]);
        let ask = EndAsk::before(&sent);
        (sent, ask)
    }

    #[test]
    fn end_task_keeps_its_confirmation_wording() {
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let task = EndAsk::task(&[&p]).expect("one row selected");
        assert_eq!(task.title(), "End task");
        assert_eq!(task.question(), "End Code.exe?");
        assert_eq!(task.note(), "Unsaved data in this process may be lost.");
        assert_eq!(task.action(), Action::End { pid: 7, force: false, tree: false, name: "Code.exe".into() });
        assert_eq!(EndAsk::task(&[]), None, "nothing selected, nothing to ask");
        // The menu's End task for one row still runs as it is.
        assert_eq!(menu_end("End task").1, None);
    }

    /// One frame of the Processes page with `events`, keys `held` and the
    /// clock at `time` seconds.
    fn processes_frame(ctx: &egui::Context, app: &mut TaskApp, events: Vec<egui::Event>, held: egui::Modifiers, time: f64) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1800.0, 900.0))),
            events,
            modifiers: held,
            time: Some(time),
            ..Default::default()
        };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.page_processes(ui, &mut Vec::new()));
        })
    }

    /// A click on the row named `name`, with `held` keys down.
    fn click_row(ctx: &egui::Context, app: &mut TaskApp, name: &str, held: egui::Modifiers, time: f64) {
        let out = processes_frame(ctx, app, vec![], held, time);
        let at = text_spots(&out).into_iter().find(|(t, _)| t == name).expect("the row").1.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: held };
        processes_frame(ctx, app, vec![egui::Event::PointerMoved(at), press(true)], held, time);
        processes_frame(ctx, app, vec![press(false)], held, time);
    }

    fn sorted_pids(app: &TaskApp) -> Vec<u32> {
        app.selected.pids().collect()
    }

    #[test]
    fn three_selected_rows_take_the_plural_labels_and_ask_end_3_selected_processes() {
        let mut snap = disk_sample();
        snap.procs.push(proc(14, "writes-most.exe", "spata", 1.0, 1_000_000));
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let ctrl = egui::Modifiers::CTRL | egui::Modifiers::COMMAND;
        click_row(&ctx, &mut app, "reads-most.exe", egui::Modifiers::NONE, 1.0);
        click_row(&ctx, &mut app, "reads-least.exe", ctrl, 3.0);
        click_row(&ctx, &mut app, "writes-most.exe", ctrl, 5.0);
        assert_eq!(sorted_pids(&app), [11, 12, 14], "Ctrl adds rows");
        let rows = rows_of(&app.snap.procs, &app.selected);
        let entries = crate::widgets::end_entries(&rows);
        let labels: Vec<&str> = entries.iter().map(|(l, _)| *l).collect();
        assert_eq!(labels, ["End selected tasks", "Force quit selected tasks", "End selected process trees", "Force quit selected process trees"]);
        let ask = EndAsk::before(&entries[0].1).expect("ending several asks first");
        assert_eq!(ask.question(), "End 3 selected processes?");
        assert_eq!(ask.note(), "Unsaved data in these processes may be lost.");
        assert_eq!(ask.title(), "End selected tasks");
        assert_eq!(ask.action(), Action::EndMany { pids: vec![11, 12, 14], force: false, tree: false });
        assert_eq!(EndAsk::before(&entries[1].1).map(|a| a.question()).as_deref(), Some("Force quit the selected task(s)?"));
        assert_eq!(EndAsk::before(&entries[3].1), None, "Force quit process tree runs as it is");
        // Ctrl on a selected row takes it out; Shift ranges from it.
        click_row(&ctx, &mut app, "writes-most.exe", ctrl, 7.0);
        assert_eq!(sorted_pids(&app), [11, 12]);
        click_row(&ctx, &mut app, "reads-most.exe", egui::Modifiers::NONE, 9.0);
        click_row(&ctx, &mut app, "writes-most.exe", egui::Modifiers::SHIFT, 11.0);
        // Every row drawn between the two, both included.
        let out = processes_frame(&ctx, &mut app, vec![], egui::Modifiers::NONE, 12.0);
        let spots = text_spots(&out);
        let y = |name: &str| spots.iter().find(|(t, _)| t == name).expect("drawn").1.center().y;
        let (a, b) = (y("reads-most.exe").min(y("writes-most.exe")), y("reads-most.exe").max(y("writes-most.exe")));
        let mut between: Vec<u32> = app.snap.procs.iter().filter(|p| (a..=b).contains(&y(&p.name))).map(|p| p.pid).collect();
        between.sort();
        assert!(between.len() >= 2);
        assert_eq!(sorted_pids(&app), between, "Shift selects every drawn row between");
    }

    #[test]
    fn delete_asks_before_ending_the_selected_task() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        processes_frame(&ctx, &mut app, vec![key(egui::Key::Delete)], egui::Modifiers::NONE, 1.0);
        assert_eq!(app.confirm_end, None, "nothing selected, nothing asked");
        app.selected = Selection::only(12);
        processes_frame(&ctx, &mut app, vec![key(egui::Key::Delete)], egui::Modifiers::NONE, 2.0);
        let ask = app.confirm_end.clone().expect("Delete asks first");
        assert_eq!((ask.question(), ask.note()), ("End reads-most.exe?".to_string(), "Unsaved data in this process may be lost."));
        assert_eq!(ask.action(), Action::End { pid: 12, force: false, tree: false, name: "reads-most.exe".into() });
    }

    #[test]
    fn type_ahead_selects_the_row_whose_name_starts_with_the_letters_typed() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let text = |s: &str| egui::Event::Text(s.into());
        processes_frame(&ctx, &mut app, vec![text("r")], egui::Modifiers::NONE, 1.0);
        processes_frame(&ctx, &mut app, vec![text("eads-s")], egui::Modifiers::NONE, 1.5);
        assert_eq!(app.selected, Selection::only(13), "reads-some.exe");
        // Two seconds later the letters start a new name.
        processes_frame(&ctx, &mut app, vec![text("reads-m")], egui::Modifiers::NONE, 4.0);
        assert_eq!(app.selected, Selection::only(12), "reads-most.exe");
    }

    #[test]
    fn confirming_end_process_tree_sends_a_graceful_tree_end() {
        let (_, ask) = menu_end("End process tree");
        let ask = ask.expect("End process tree asks first");
        assert_eq!(ask.question(), "End the selected process tree(s)?");
        assert_eq!(ask.action(), Action::End { pid: 7, force: false, tree: true, name: "Code.exe".into() });
    }

    #[test]
    fn force_quit_process_tree_sends_a_forced_tree_end() {
        let labels: Vec<&str> = crate::widgets::END_ITEMS.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels, ["End task", "Force quit", "End process tree", "Force quit process tree"]);
        let (sent, ask) = menu_end("Force quit process tree");
        assert_eq!(sent, Action::End { pid: 7, force: true, tree: true, name: "Code.exe".into() });
        assert_eq!(ask, None, "it is sent as it is");
    }

    #[test]
    fn force_quit_asks_before_it_sends_a_forced_end() {
        let (_, ask) = menu_end("Force quit");
        let ask = ask.expect("Force quit asks first");
        assert_eq!(ask.question(), "Force quit the selected task(s)?");
        assert_eq!(ask.note(), "", "the Force quit question has no note");
        assert_eq!(ask.action(), Action::End { pid: 7, force: true, tree: false, name: "Code.exe".into() });
    }

    #[test]
    fn inspect_is_the_bold_first_item_of_the_process_menu() {
        // The menu drawn in a plain panel: no context menu needed.
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let mut actions = Vec::new();
        let draw = |events, actions: &mut Vec<Action>| crate::test_ui::run(&ctx, (400.0, 600.0), events, |ui| crate::widgets::process_menu(ui, &p, &[&p], &no_view_state(&[]), actions));
        let out = draw(vec![], &mut actions);
        let texts: Vec<(String, egui::FontFamily, egui::Rect)> = crate::test_ui::flat_shapes(&out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(g) => Some((g.galley.text().to_string(), g.galley.job.sections[0].format.font_id.family.clone(), g.visual_bounding_rect())),
                _ => None,
            })
            .collect();
        let (first, family, at) = texts.first().cloned().expect("menu items");
        assert_eq!(first, "Inspect");
        assert_eq!(family, theme::semibold(), "drawn bold, as the default item");
        assert_eq!(texts[1].0, "End task", "the end items follow");
        let press = |pressed| egui::Event::PointerButton { pos: at.center(), button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        draw(vec![egui::Event::PointerMoved(at.center()), press(true)], &mut actions);
        draw(vec![press(false)], &mut actions);
        assert_eq!(actions, [Action::Inspect(7)]);
    }

    #[test]
    fn a_double_click_on_a_process_row_inspects_it_and_a_click_only_selects_it() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let mut actions = Vec::new();
        let draw = |events, actions: &mut Vec<Action>, app: &mut TaskApp| crate::test_ui::run(&ctx, (1800.0, 900.0), events, |ui| app.page_processes(ui, actions));
        let out = draw(vec![], &mut actions, &mut app);
        let at = text_spots(&out).into_iter().find(|(t, _)| t == "reads-most.exe").expect("the row").1.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        draw(vec![egui::Event::PointerMoved(at), press(true)], &mut actions, &mut app);
        draw(vec![press(false)], &mut actions, &mut app);
        assert_eq!(app.selected, Selection::only(12), "a click selects the row");
        assert!(actions.is_empty(), "a click inspects nothing: {actions:?}");
        draw(vec![press(true)], &mut actions, &mut app);
        draw(vec![press(false)], &mut actions, &mut app);
        assert_eq!(actions, [Action::Inspect(12)], "the second click of a double-click inspects the row");
    }

    fn sample() -> Vec<ProcRow> {
        vec![
            proc(0, "System Idle Process", "", 90.0, 0),
            proc(4, "System", "", 0.4, 0),
            proc(35308, "chrome.exe", "spata", 0.2, 6_000_000_000),
            proc(16244, "Code.exe", "spata", 8.0, 4_300_000_000),
            proc(8944, "explorer.exe", "spata", 0.0, 470_000_000),
        ]
    }

    #[test]
    fn the_summary_band_sums_up_cpu_speed_memory_and_processes() {
        assert_eq!(summary_sub(15.42, 4900, 40.5, 389), "CPU 15.4% at 4.90 GHz \u{b7} memory 40.5% \u{b7} 389 processes");
        assert_eq!(summary_sub(0.0, 0, 0.0, 1), "CPU 0.0% \u{b7} memory 0.0% \u{b7} 1 process", "no speed reading: no speed");
    }

    #[test]
    fn summary_lists_the_busiest_processes_without_the_idle_one() {
        let top: Vec<u32> = top_by_cpu(&sample(), 3).iter().map(|p| p.pid).collect();
        assert_eq!(top, vec![16244, 4, 35308], "by CPU, the idle process (pid 0) left out");
        let mut tied = sample();
        tied.push(proc(77, "tied.exe", "spata", 0.2, 9_000_000_000));
        let top: Vec<u32> = top_by_cpu(&tied, 4).iter().map(|p| p.pid).collect();
        assert_eq!(top, vec![16244, 4, 77, 35308], "equal CPU: more memory first");
    }

    #[test]
    fn the_processes_band_counts_the_processes_and_names_the_busiest() {
        let band = ProcessesBand::of(&sample(), 4, "");
        assert_eq!(band.headline, "4 processes are running");
        assert_eq!(band.shown, None);
        assert_eq!(band.most_cpu.as_deref(), Some("Code.exe, 8.0%"), "the idle process is not a process");
        assert_eq!(band.most_memory.as_deref(), Some(format!("chrome.exe, {}", fmt_bytes(6_000_000_000)).as_str()));
        let filtered = ProcessesBand::of(&sample(), 1, "code");
        assert_eq!(filtered.shown.as_deref(), Some("1 shown for \u{201c}code\u{201d}"));
        assert_eq!(ProcessesBand::of(&sample()[..2], 1, "").headline, "1 process is running");
        assert_eq!(ProcessesBand::of(&[], 0, ""), ProcessesBand { headline: "0 processes are running".into(), shown: None, most_cpu: None, most_memory: None });
    }

    /// Three processes whose disk reads and writes run in opposite orders.
    fn disk_sample() -> crate::sampler::Snapshot {
        let row = |pid, name: &str, read, write| ProcRow { disk_read: read, disk_write: write, ..proc(pid, name, "spata", 1.0, 1_000_000) };
        crate::sampler::Snapshot {
            procs: vec![row(11, "reads-least.exe", 100.0, 900.0), row(12, "reads-most.exe", 900.0, 100.0), row(13, "reads-some.exe", 500.0, 500.0)],
            mem_total: 16_000_000_000,
            ..Default::default()
        }
    }

    /// Where each text shape was painted, by its words.
    fn text_spots(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
        crate::test_ui::flat_shapes(out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(g) => Some((g.galley.text().to_string(), g.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    /// A column heading's words end with its title (CPU, Memory and the
    /// disk columns carry the machine's total on a line above).
    fn is_heading(text: &str, title: &str) -> bool {
        text == title || text.ends_with(&format!("\n{title}"))
    }

    #[test]
    fn the_processes_page_draws_the_standard_columns_in_order() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let spots = text_spots(&out);
        let titles = [
            "Name", "PID", "Status", "User name", "CPU", "Memory", "Disk read", "Disk write", "Threads", "GPU", "Peak memory", "CPU time",
            "Page faults", "Handles", "Priority", "Architecture", "Command",
        ];
        let lefts: Vec<f32> = titles
            .iter()
            .map(|title| spots.iter().find(|(text, _)| is_heading(text, title)).unwrap_or_else(|| panic!("a {title} heading")).1.left())
            .collect();
        assert!(lefts.windows(2).all(|w| w[0] < w[1]), "left to right in the table's order: {lefts:?}");
        assert!(!spots.iter().any(|(text, _)| is_heading(text, "Disk")), "no combined Disk column");
    }

    #[test]
    fn disk_read_and_disk_write_are_separate_sortable_columns() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let mut draw = |events| crate::test_ui::run(&ctx, (1800.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new()));
        let order = |out: &egui::FullOutput| {
            let mut rows: Vec<(f32, String)> =
                text_spots(out).into_iter().filter(|(text, _)| text.starts_with("reads-") && text.ends_with(".exe")).map(|(text, r)| (r.top(), text)).collect();
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            rows.into_iter().map(|(_, text)| text).collect::<Vec<_>>()
        };
        let mut click = |title: &str| {
            let at = text_spots(&draw(vec![])).into_iter().find(|(text, _)| is_heading(text, title)).expect("the heading").1.center();
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            draw(vec![egui::Event::PointerMoved(at), press(true)]);
            draw(vec![press(false)]);
            order(&draw(vec![]))
        };
        assert_eq!(click("Disk write"), ["reads-least.exe", "reads-some.exe", "reads-most.exe"], "most written first");
        assert_eq!(click("Disk read"), ["reads-most.exe", "reads-some.exe", "reads-least.exe"], "most read first");
        assert_eq!(click("Disk read"), ["reads-least.exe", "reads-some.exe", "reads-most.exe"], "a second click turns it round");
    }

    #[test]
    fn process_facts_are_shown_in_the_documented_units() {
        let mut snap = disk_sample();
        snap.procs = vec![ProcRow {
            disk_read: 512.0,
            disk_write: 2048.0,
            threads: 37,
            handles: 1204,
            peak_mem: 300 * 1_048_576,
            page_faults: 98765,
            cpu_time: std::time::Duration::from_millis(321_010),
            priority: "Above normal",
            arch: "x64",
            ..proc(21, "busy.exe", "spata", 2.0, 1_000_000)
        }, ProcRow {
            cpu_time: std::time::Duration::from_secs(3),
            ..proc(22, "quiet.exe", "spata", 0.0, 1_000_000)
        }];
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let texts = crate::test_ui::texts(&out);
        for shown in ["512 B/sec", "2.00 KB/sec", "37", "1204", "300.0 MB", "98765", "5:21.01", "0:00:03", "Above normal", "x64"] {
            assert!(texts.iter().any(|t| t == shown), "{shown} in {texts:?}");
        }
    }

    /// The words drawn by one frame of the Processes page of `app`.
    fn processes_texts(app: &mut TaskApp) -> Vec<String> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        crate::test_ui::texts(&out).into_iter().map(|t| t.trim().to_string()).collect()
    }

    #[test]
    fn the_processes_header_shows_process_view_the_filter_hint_and_refresh() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let texts = processes_texts(&mut app);
        for shown in ["Processes", "Process View", "Filter by name, user, or PID", "Refresh"] {
            assert!(texts.iter().any(|t| t == shown), "{shown} in {texts:?}");
        }
        assert!(!texts.iter().any(|t| t.contains("Run new task")), "Run new task lives under File: {texts:?}");
    }

    #[test]
    fn a_filter_that_hides_every_row_asks_to_clear_the_search_box() {
        const CLEAR: &str = "Clear the search box to restore all process rows.";
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        app.search = "no-such-process".into();
        assert!(processes_texts(&mut app).iter().any(|t| t == CLEAR));
        app.search = "reads-most".into();
        let texts = processes_texts(&mut app);
        assert!(texts.iter().any(|t| t == "reads-most.exe"), "the filter keeps its match");
        assert!(!texts.iter().any(|t| t == CLEAR), "a row is shown: no message");
        app.search.clear();
        app.snap.procs.clear();
        assert!(!processes_texts(&mut app).iter().any(|t| t == CLEAR), "no filter, no rows: nothing to clear");
    }

    #[test]
    fn the_band_still_names_the_busiest_processes() {
        let snap = crate::sampler::Snapshot { procs: sample(), mem_total: 16_000_000_000, ..Default::default() };
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        let texts = processes_texts(&mut app);
        for shown in ["Most memory", "Most CPU now", "Code.exe, 8.0%"] {
            assert!(texts.iter().any(|t| t == shown), "{shown} in {texts:?}");
        }
        assert!(texts.iter().any(|t| t.starts_with("chrome.exe, ")), "{texts:?}");
    }

    #[test]
    fn services_keep_their_own_filter_on_the_shared_search_text() {
        let svc = |name: &str| crate::winsys::ServiceRow { name: name.into(), display: String::new(), pid: 0, status: "Running".into(), start_type: "Manual".into() };
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        app.services = Some(Ok(vec![svc("WSearch"), svc("Spooler")]));
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let draw = |app: &mut TaskApp| {
            let out = crate::test_ui::run(&ctx, (1400.0, 900.0), vec![], |ui| app.page_services(ui, &mut Vec::new()));
            crate::test_ui::texts(&out).into_iter().map(|t| t.trim().to_string()).collect::<Vec<_>>()
        };
        let texts = draw(&mut app);
        assert!(texts.iter().any(|t| t == "Filter services"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Filter by name, user, or PID"));
        app.search = "wsearch".into();
        let texts = draw(&mut app);
        assert!(texts.iter().any(|t| t == "WSearch"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Spooler"));
        assert!(!texts.iter().any(|t| t.starts_with("Clear the search box")), "the processes message stays on Processes");
    }

    #[test]
    fn the_services_filter_also_matches_the_pid_of_their_host() {
        let svc = |name: &str, pid| crate::winsys::ServiceRow { name: name.into(), display: String::new(), pid, status: "Running".into(), start_type: "Manual".into() };
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        app.services = Some(Ok(vec![svc("WSearch", 300), svc("Winmgmt", 200), svc("Schedule", 200)]));
        app.search = "200".into();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1400.0, 900.0), vec![], |ui| app.page_services(ui, &mut Vec::new()));
        let texts: Vec<String> = crate::test_ui::texts(&out).into_iter().map(|t| t.trim().to_string()).collect();
        assert!(texts.iter().any(|t| t == "Winmgmt") && texts.iter().any(|t| t == "Schedule"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "WSearch"), "another host's service");
    }

    /// A process busy on the GPU, one less so and one without GPU engines.
    fn gpu_sample() -> crate::sampler::Snapshot {
        let row = |pid, name: &str, cpu, gpu| ProcRow { gpu, ..proc(pid, name, "spata", cpu, 1_000_000) };
        crate::sampler::Snapshot {
            procs: vec![row(31, "gpu-some.exe", 1.0, 12.5), row(32, "gpu-none.exe", 3.0, 0.0), row(33, "gpu-most.exe", 2.0, 47.0)],
            mem_total: 16_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn the_gpu_column_shows_each_processs_engine_percent() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), gpu_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let spots = text_spots(&out);
        let gpu = spots.iter().find(|(text, _)| is_heading(text, "GPU")).expect("a GPU heading").1;
        let in_gpu_column = |shown: &str| spots.iter().any(|(text, r)| text == shown && r.left() >= gpu.left() - 8.0 && r.left() < gpu.left() + 60.0);
        assert!(in_gpu_column("12.5%"), "the summed engines of gpu-some.exe");
        assert!(in_gpu_column("47.0%"));
        assert!(in_gpu_column("0.0%"), "a process without GPU engines shows 0.0%");
    }

    #[test]
    fn sorting_by_gpu_puts_the_busiest_first_then_turns_round() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), gpu_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let mut draw = |events| crate::test_ui::run(&ctx, (1800.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new()));
        let order = |out: &egui::FullOutput| {
            let mut rows: Vec<(f32, String)> =
                text_spots(out).into_iter().filter(|(text, _)| text.starts_with("gpu-") && text.ends_with(".exe")).map(|(text, r)| (r.top(), text)).collect();
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            rows.into_iter().map(|(_, text)| text).collect::<Vec<_>>()
        };
        let mut click = |title: &str| {
            let at = text_spots(&draw(vec![])).into_iter().find(|(text, _)| is_heading(text, title)).expect("the heading").1.center();
            let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            draw(vec![egui::Event::PointerMoved(at), press(true)]);
            draw(vec![press(false)]);
            order(&draw(vec![]))
        };
        assert_eq!(click("GPU"), ["gpu-most.exe", "gpu-some.exe", "gpu-none.exe"], "busiest first");
        assert_eq!(click("GPU"), ["gpu-none.exe", "gpu-some.exe", "gpu-most.exe"], "a second click turns it round");
        assert_eq!(click("CPU"), ["gpu-none.exe", "gpu-most.exe", "gpu-some.exe"], "CPU still sorts by CPU");
    }

    #[test]
    fn the_processes_table_scrolls_sideways_when_the_columns_do_not_fit() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let mut draw = |events| crate::test_ui::run(&ctx, (800.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new()));
        let command = |out: &egui::FullOutput| text_spots(out).into_iter().find(|(text, _)| text == "Command").map(|(_, r)| r);
        assert_eq!(command(&draw(vec![])), None, "Command is off to the right");
        let over_table = egui::pos2(400.0, 400.0);
        let wheel = egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(-2000.0, 0.0), modifiers: Default::default() };
        draw(vec![egui::Event::PointerMoved(over_table), wheel]);
        let mut seen = None;
        for _ in 0..30 {
            seen = command(&draw(vec![]));
        }
        let seen = seen.expect("scrolling sideways brings Command into view");
        assert!(seen.right() <= 800.0, "{seen:?}");
    }

    #[test]
    fn expand_all_and_collapse_all_are_greyed_while_the_tree_is_off() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        for tree in [true, false] {
            app.settings.process_tree = tree;
            let items: Vec<(&str, Option<bool>, bool)> = app
                .process_view_items()
                .into_iter()
                .filter_map(|i| match i {
                    parts::DropItem::Item { label, checked, enabled, .. } => Some((label, checked, enabled)),
                    parts::DropItem::Separator => None,
                })
                .collect();
            assert_eq!(
                items,
                [
                    ("Process tree", Some(tree), true),
                    ("Smooth CPU readings", Some(false), true),
                    ("Show exited processes", Some(true), true),
                    ("Expand all", None, tree),
                    ("Collapse all", None, tree)
                ]
            );
        }
    }

    #[test]
    fn process_view_lists_show_exited_processes_ticked_as_saved() {
        let mut settings = crate::settings::Settings::default();
        settings.show_exited = false;
        let mut app = TaskApp::headless(settings, tree_sample());
        let ticked = |app: &TaskApp| {
            app.process_view_items().into_iter().find_map(|i| match i {
                parts::DropItem::Item { id: ProcView::ShowExited, label, checked, .. } => Some((label, checked)),
                _ => None,
            })
        };
        assert_eq!(ticked(&app), Some(("Show exited processes", Some(false))));
        app.settings.show_exited = true;
        assert_eq!(ticked(&app), Some(("Show exited processes", Some(true))));
    }

    #[test]
    fn an_exited_process_reads_exited_tinted_red_with_its_gpu_unavailable() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let t0 = std::time::Instant::now();
        app.proc_view.track(&app.snap.procs, t0, true);
        app.snap.procs.retain(|p| p.pid != 200);
        app.proc_view.track(&app.snap.procs, t0 + std::time::Duration::from_secs(1), true);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let texts = crate::test_ui::texts(&out);
        assert!(texts.iter().any(|t| t == "svchost.exe") && texts.iter().any(|t| t == "Background processes (1)"), "kept in its group: {texts:?}");
        assert!(texts.iter().any(|t| t == "Exited"), "{texts:?}");
        assert_eq!(texts.iter().filter(|t| *t == "Unavailable").count(), 1, "only the exited row's GPU: {texts:?}");
        for word in [concat!("Termin", "ated"), "Not responding", "Suspended"] {
            assert!(!texts.iter().any(|t| t == word), "{word} in {texts:?}");
        }
        assert!(crate::test_ui::colours(&out).contains(&Theme::of(theme::Mode::Dark).alert_note), "the exited row is tinted red");
    }

    /// The Processes page drawn once, after svchost.exe's CPU changed
    /// between two samples a moment ago.
    fn page_after_a_change(settings: crate::settings::Settings) -> egui::FullOutput {
        let mut app = TaskApp::headless(settings, tree_sample());
        let now = std::time::Instant::now();
        let before = now.checked_sub(std::time::Duration::from_secs(1)).unwrap_or(now);
        app.proc_view.track(&app.snap.procs, before, true);
        app.snap.procs[2].cpu = 40.0;
        app.proc_view.track(&app.snap.procs, now, true);
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()))
    }

    #[test]
    fn a_row_whose_figures_changed_lights_its_cpu_cell_green() {
        let lit = parts::lit_fill(&Theme::of(theme::Mode::Dark));
        let out = page_after_a_change(crate::settings::Settings::default());
        assert!(crate::test_ui::colours(&out).contains(&lit), "Highlight Process Updates starts on");
    }

    #[test]
    fn turning_highlight_process_updates_off_stops_the_lit_rows() {
        let lit = parts::lit_fill(&Theme::of(theme::Mode::Dark));
        let mut settings = crate::settings::Settings::default();
        settings.highlight_updates = false;
        let out = page_after_a_change(settings);
        assert!(!crate::test_ui::colours(&out).contains(&lit));
        assert!(crate::test_ui::texts(&out).iter().any(|t| t == "40.0%"), "the figure itself still shows");
    }

    #[test]
    fn resource_values_percents_shows_memory_disk_and_peak_memory_as_shares() {
        let mut sample = tree_sample();
        sample.procs[2].peak_mem = 4_000_000_000;
        sample.procs[2].disk_read = 30.0;
        sample.disk_read_bps = 120.0;
        let draw = |percents| {
            let mut settings = crate::settings::Settings::default();
            settings.process_percents = percents;
            set_column_shown_in(&columns_with(&[]), &mut settings.process_table, ProcCol::PeakMem, true);
            let mut app = TaskApp::headless(settings, sample.clone());
            let ctx = crate::test_ui::context(theme::Mode::Dark);
            crate::test_ui::texts(&crate::test_ui::run(&ctx, (2400.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new())))
        };
        let texts = draw(true);
        // svchost.exe: 100 MB of 16 GB, 4 GB peak, a quarter of the reads.
        for share in ["0.6%", "25.0%"] {
            assert!(texts.iter().any(|t| t == share), "{share} in {texts:?}");
        }
        assert!(!texts.iter().any(|t| t == "95.4 MB" || t == "100.0 MB"), "no values: {texts:?}");
        let texts = draw(false);
        assert!(!texts.iter().any(|t| t == "25.0%"), "Values: {texts:?}");
    }

    /// A menu state with no add-on item, Values, Normal priority and
    /// `view` as the Process View items.
    fn no_view_state(view: &[parts::DropItem<ProcView>]) -> MenuState<'_> {
        MenuState { links: &[], percents: false, view, priority: Some("Normal"), affinity: None, shell: None, following: None, services: None, windowed: None }
    }

    /// The labels of menu lines ("" for a separator).
    fn labels(entries: &[MenuEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.label()).collect()
    }

    /// The lines of the submenu called `name`.
    fn submenu<'a>(entries: &'a [MenuEntry], name: &str) -> &'a [MenuEntry] {
        entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Sub { label, items } if *label == name => Some(&items[..]),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} in {:?}", labels(entries)))
    }

    /// The item called `name` among `entries`: (enabled, what it sends).
    fn menu_item(entries: &[MenuEntry], name: &str) -> (bool, Vec<Action>) {
        entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Item { label, enabled, actions, .. } if label == name => Some((*enabled, actions.clone())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} in {:?}", labels(entries)))
    }

    #[test]
    fn resource_values_follows_view_with_values_and_percents() {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let view = app.process_view_items();
        let out = crate::test_ui::run(&ctx, (400.0, 800.0), vec![], |ui| crate::widgets::process_menu(ui, &p, &[&p], &no_view_state(&view), &mut Vec::new()));
        let texts = crate::test_ui::texts(&out);
        let at = |w: &str| texts.iter().position(|t| t == w).unwrap_or_else(|| panic!("{w} in {texts:?}"));
        assert_eq!(at("Follow process"), at("View") + 1, "{texts:?}");
        assert_eq!(at("Resource values"), at("Follow process") + 1, "{texts:?}");
        assert_eq!(at("Efficiency mode"), at("Resource values") + 1, "{texts:?}");
        assert_eq!(at("Set priority"), at("Efficiency mode") + 1, "{texts:?}");
        assert_eq!(crate::widgets::RESOURCE_VALUES.map(|(_, label)| label), ["Values", "Percents"]);
    }

    #[test]
    fn the_process_menu_holds_its_items_and_separators_in_order() {
        use crate::widgets::{MenuLine::*, MenuSlot::*};
        // Every item, the later tickets' included.
        assert_eq!(
            crate::widgets::PROCESS_MENU,
            [
                Item(Inspect), Separator,
                Item(End(EndItem::Task)), Item(Restart), Item(End(EndItem::ForceQuit)), Item(End(EndItem::Tree)), Item(End(EndItem::ForceQuitTree)), Separator,
                Item(Suspend), Item(Resume), Separator,
                Item(View), Item(Follow), Separator,
                Item(ResourceValues), Separator,
                Item(EfficiencyMode), Item(SetPriority), Item(SetAffinity), Item(AnalyzeWaitChain), Item(FileActivity), Item(UacVirtualization), Separator,
                Item(CreateDump), Item(GoToServices), Item(BringToFront), Item(OpenFileLocation), Item(SearchOnline), Item(CopyName),
            ]
        );
        let app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let view = app.process_view_items();
        let mut p = proc(7, "Code.exe", "spata", 0.0, 0);
        p.exe = "C:/Code/Code.exe".into();
        let one = menu_entries(&p, &[&p], &no_view_state(&view));
        assert_eq!(
            labels(&one),
            [
                "Inspect", "",
                "End task", "Force quit", "End process tree", "Force quit process tree", "",
                "Suspend", "Resume", "",
                "View", "Follow process", "",
                "Resource values", "",
                "Efficiency mode", "Set priority", "Set affinity", "Analyze wait chain", "File activity", "UAC virtualization", "",
                "Create memory dump file", "Go to service(s)", "Bring to front", "Open file location", "Search online", "Copy process name",
            ]
        );
        assert_eq!(labels(submenu(&one, "View")), ["Process tree", "Expand all", "Collapse all", "", "Smooth CPU readings", "Show exited processes"]);
        assert_eq!(labels(submenu(&one, "Set priority")), ["Realtime", "High", "Above normal", "Normal", "Below normal", "Low"]);
        // Several rows: the plurals, without the single-row items.
        let q = proc(8, "node.exe", "spata", 0.0, 0);
        let many = menu_entries(&p, &[&p, &q], &no_view_state(&view));
        assert_eq!(
            labels(&many),
            [
                "End selected tasks", "Force quit selected tasks", "End selected process trees", "Force quit selected process trees", "",
                "Suspend selected tasks", "Resume selected tasks", "",
                "View", "",
                "Resource values", "",
                "Efficiency mode for selected tasks", "Set priority", "Set affinity", "Analyze wait chain", "UAC virtualization", "",
                "Go to service(s)", "Search online", "Copy process name",
            ]
        );
        // An exited process: the short menu.
        p.status = crate::process_view::EXITED.into();
        assert_eq!(labels(&menu_entries(&p, &[&p], &no_view_state(&view))), ["Inspect", "", "Open file location", "Search online", "Copy process name"]);
    }

    #[test]
    fn analyze_wait_chain_analyses_the_right_clicked_process_with_one_or_several_rows() {
        let app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let view = app.process_view_items();
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let q = proc(8, "node.exe", "spata", 0.0, 0);
        for rows in [vec![&p], vec![&p, &q]] {
            let entries = menu_entries(&p, &rows, &no_view_state(&view));
            let found = entries.iter().find_map(|e| match e {
                crate::widgets::MenuEntry::Item { label, enabled, actions, .. } if label == "Analyze wait chain" => Some((*enabled, actions.clone())),
                _ => None,
            });
            assert_eq!(found, Some((true, vec![Action::AnalyzeWaitChain(7)])), "{} rows", rows.len());
        }
    }

    #[test]
    fn file_activity_is_never_greyed_and_opens_the_window_of_the_row() {
        let app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let view = app.process_view_items();
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let one = menu_entries(&p, &[&p], &no_view_state(&view));
        assert_eq!(menu_item(&one, "File activity"), (true, vec![Action::FileActivity(7)]), "the administrator check runs on the click");
        let q = proc(8, "node.exe", "spata", 0.0, 0);
        assert!(!labels(&menu_entries(&p, &[&p, &q], &no_view_state(&view))).contains(&"File activity"), "a one-process window");
    }

    #[test]
    fn the_view_submenu_sends_the_process_view_items_and_greys_expand_all_with_the_tree_off() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        for tree in [true, false] {
            app.settings.process_tree = tree;
            let view = app.process_view_items();
            let entries = menu_entries(&p, &[&p], &no_view_state(&view));
            let sub = submenu(&entries, "View");
            assert_eq!(menu_item(sub, "Process tree"), (true, vec![Action::ProcessView(ProcView::Tree)]));
            assert_eq!(menu_item(sub, "Expand all"), (tree, vec![Action::ProcessView(ProcView::ExpandAll)]));
            assert_eq!(menu_item(sub, "Collapse all"), (tree, vec![Action::ProcessView(ProcView::CollapseAll)]));
            assert_eq!(menu_item(sub, "Smooth CPU readings").1, [Action::ProcessView(ProcView::Smooth)]);
            assert_eq!(menu_item(sub, "Show exited processes").1, [Action::ProcessView(ProcView::ShowExited)]);
            let ticked = sub.iter().find_map(|e| match e {
                MenuEntry::Item { label, checked, .. } if label == "Process tree" => *checked,
                _ => None,
            });
            assert_eq!(ticked, Some(tree), "ticked as in the dropdown");
        }
    }

    #[test]
    fn suspend_and_resume_send_their_actions_for_the_selected_rows() {
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let mut q = proc(8, "node.exe", "spata", 0.0, 0);
        let one = menu_entries(&p, &[&p], &no_view_state(&[]));
        assert_eq!(menu_item(&one, "Suspend"), (true, vec![Action::Suspend(vec![7])]));
        assert_eq!(menu_item(&one, "Resume"), (false, vec![Action::Resume(vec![7])]), "greyed: nothing to resume");
        q.status = "Suspended".into();
        let both = menu_entries(&p, &[&p, &q], &no_view_state(&[]));
        assert_eq!(menu_item(&both, "Suspend selected tasks"), (true, vec![Action::Suspend(vec![7, 8])]));
        assert_eq!(menu_item(&both, "Resume selected tasks"), (true, vec![Action::Resume(vec![7, 8])]));
        assert_eq!(menu_item(&both, "Efficiency mode for selected tasks").1, [Action::Efficiency(vec![7, 8])]);
        let ticked = submenu(&one, "Set priority").iter().find_map(|e| match e {
            MenuEntry::Item { label, checked: Some(true), .. } => Some(label.clone()),
            _ => None,
        });
        assert_eq!(ticked.as_deref(), Some("Normal"));
        assert_eq!(menu_item(submenu(&both, "Set priority"), "Low").1, [Action::Priority(7, "Low"), Action::Priority(8, "Low")]);
    }

    #[test]
    fn create_dump_uac_virtualization_and_bring_to_front_send_their_actions() {
        let mut p = proc(7, "Code.exe", "spata", 0.0, 0);
        p.app = true;
        let one = menu_entries(&p, &[&p], &no_view_state(&[]));
        assert_eq!(menu_item(&one, "Create memory dump file"), (true, vec![Action::Dump { pid: 7, name: "Code.exe".into() }]));
        assert_eq!(menu_item(&one, "UAC virtualization"), (true, vec![Action::UacVirtualization(vec![7])]));
        assert_eq!(menu_item(&one, "Bring to front"), (true, vec![Action::BringToFront(7)]));
        // UAC virtualization stays with several rows and acts on them all.
        let q = proc(8, "node.exe", "spata", 0.0, 0);
        let both = menu_entries(&p, &[&p, &q], &no_view_state(&[]));
        assert_eq!(menu_item(&both, "UAC virtualization"), (true, vec![Action::UacVirtualization(vec![7, 8])]));
    }

    #[test]
    fn bring_to_front_is_greyed_for_a_process_without_a_window() {
        let p = proc(7, "svchost.exe", "SYSTEM", 0.0, 0);
        assert!(!p.app, "a process without a window");
        let one = menu_entries(&p, &[&p], &no_view_state(&[]));
        assert_eq!(menu_item(&one, "Bring to front"), (false, vec![Action::BringToFront(7)]));
        let none = menu_entries(&p, &[&p], &MenuState { windowed: Some(false), ..no_view_state(&[]) });
        assert!(!menu_item(&none, "Bring to front").0, "no window at all");
    }

    #[test]
    fn bring_to_front_is_offered_for_a_process_with_a_window_that_is_not_an_app() {
        // A tool window or an untitled one: not listed under Apps, but
        // there is a window to bring up (US58 greys only "no window").
        let p = proc(7, "helper.exe", "spata", 0.0, 0);
        assert!(!p.app);
        let one = menu_entries(&p, &[&p], &MenuState { windowed: Some(true), ..no_view_state(&[]) });
        assert_eq!(menu_item(&one, "Bring to front"), (true, vec![Action::BringToFront(7)]));
    }

    #[test]
    fn restart_is_offered_only_for_the_shells_explorer() {
        let shell = proc(4242, "explorer.exe", "spata", 0.0, 0);
        let other = proc(7, "explorer.exe", "spata", 0.0, 0);
        let state = MenuState { shell: Some(4242), ..no_view_state(&[]) };
        let one = menu_entries(&shell, &[&shell], &state);
        let names = labels(&one);
        let at = names.iter().position(|l| *l == "Restart").unwrap_or_else(|| panic!("Restart in {names:?}"));
        assert_eq!((names[at - 1], names[at + 1]), ("End task", "Force quit"), "between End task and Force quit");
        assert_eq!(menu_item(&one, "Restart"), (true, vec![Action::RestartExplorer(4242)]));
        assert!(!labels(&menu_entries(&other, &[&other], &state)).contains(&"Restart"), "not for another explorer.exe");
        assert!(!labels(&menu_entries(&shell, &[&shell, &other], &state)).contains(&"Restart"), "not with several rows");
        assert!(!labels(&menu_entries(&shell, &[&shell], &no_view_state(&[]))).contains(&"Restart"), "not without a shell window");
    }

    #[test]
    fn follow_process_is_ticked_and_reads_stop_following_while_on() {
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let checked = |entries: &[MenuEntry], name: &str| {
            entries.iter().find_map(|e| match e {
                MenuEntry::Item { label, checked, .. } if label == name => *checked,
                _ => None,
            })
        };
        let off = menu_entries(&p, &[&p], &no_view_state(&[]));
        assert_eq!(menu_item(&off, "Follow process"), (true, vec![Action::Follow(Some(7))]));
        assert_eq!(checked(&off, "Follow process"), Some(false));
        let on = menu_entries(&p, &[&p], &MenuState { following: Some(7), ..no_view_state(&[]) });
        assert_eq!(menu_item(&on, "Stop following"), (true, vec![Action::Follow(None)]));
        assert_eq!(checked(&on, "Stop following"), Some(true));
        // Following another process: this row offers to follow itself.
        let other = menu_entries(&p, &[&p], &MenuState { following: Some(8), ..no_view_state(&[]) });
        assert_eq!(menu_item(&other, "Follow process").1, [Action::Follow(Some(7))]);
    }

    #[test]
    fn go_to_services_is_greyed_without_a_service_and_sends_the_host_and_service() {
        let host = proc(200, "svchost.exe", "SYSTEM", 0.0, 0);
        let info = |name: &str| crate::sampler::ServiceInfo { name: name.into(), ..Default::default() };
        let none = menu_entries(&host, &[&host], &no_view_state(&[]));
        assert_eq!(menu_item(&none, "Go to service(s)"), (false, vec![Action::GoToServices { pid: 200, service: None }]), "greyed: hosts no service");
        let map: std::collections::HashMap<u32, Vec<crate::sampler::ServiceInfo>> = [(200, vec![info("Winmgmt"), info("Schedule")])].into_iter().collect();
        let state = MenuState { services: Some(&map), ..no_view_state(&[]) };
        let hosting = menu_entries(&host, &[&host], &state);
        assert_eq!(menu_item(&hosting, "Go to service(s)"), (true, vec![Action::GoToServices { pid: 200, service: None }]));
        // A service row under its host goes to that service.
        let row = ProcRow { service: Some(info("Schedule")), ..host.clone() };
        assert_eq!(menu_item(&menu_entries(&row, &[&row], &state), "Go to service(s)"), (true, vec![Action::GoToServices { pid: 200, service: Some("Schedule".into()) }]));
    }

    /// 80 background processes; the first takes `mem` bytes, the others 1000.
    fn many_sample(mem: u64) -> crate::sampler::Snapshot {
        crate::sampler::Snapshot {
            procs: (0..80).map(|i| proc(1000 + i, &format!("p{i}.exe"), "spata", 0.0, if i == 0 { mem } else { 1000 })).collect(),
            mem_total: 16_000_000_000,
            ..Default::default()
        }
    }

    #[test]
    fn a_followed_row_stays_selected_and_in_view_after_the_order_changes() {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let draw = |app: &mut TaskApp| crate::test_ui::texts(&crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new())));
        let mut followed = TaskApp::headless(crate::settings::Settings::default(), many_sample(9_000_000));
        followed.follow(Some(1000));
        assert!(draw(&mut followed).iter().any(|t| t == "p0.exe"), "on top by memory");
        assert_eq!(followed.selected, Selection::only(1000), "following selects the row");
        // The next snapshot puts it last (Memory, largest first).
        let mut plain = TaskApp::headless(crate::settings::Settings::default(), many_sample(1));
        assert!(!draw(&mut plain).iter().any(|t| t == "p0.exe"), "last is out of view unfollowed");
        followed.snap = many_sample(1);
        followed.selected = Selection::only(1001);
        draw(&mut followed);
        let texts = draw(&mut followed);
        assert_eq!(followed.selected, Selection::only(1000), "still selected");
        assert!(texts.iter().any(|t| t == "p0.exe"), "scrolled into view: {texts:?}");
        // Stop following: the selection is the user's again.
        followed.follow(None);
        followed.selected = Selection::only(1001);
        draw(&mut followed);
        assert_eq!(followed.selected, Selection::only(1001));
    }

    #[test]
    fn efficiency_mode_asks_once_then_not_again_after_dont_ask_again() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let frame = |app: &mut TaskApp, actions: &mut Vec<Action>| crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, actions));
        let mut actions = vec![Action::Efficiency(vec![12])];
        frame(&mut app, &mut actions);
        assert!(actions.is_empty(), "held for the question: {actions:?}");
        let ask = app.confirm_efficiency.clone().expect("Efficiency mode asks first");
        assert_eq!(ask, EfficiencyAsk { pids: vec![12], dont_ask: false });
        assert_eq!(EFFICIENCY_QUESTION, "Efficiency mode lowers this process's priority and power consumption. Continue?");
        assert_eq!(ask.title(), "Efficiency mode");
        // Agreed with Don't ask again ticked.
        let mut sent = Vec::new();
        app.confirm_efficiency = None;
        app.answer_efficiency(&EfficiencyAsk { dont_ask: true, ..ask }, &mut sent);
        assert_eq!(sent, [Action::Efficiency(vec![12])]);
        assert!(!app.settings.ask_efficiency, "Ask before applying Efficiency mode is off");
        // The next time it runs as it is.
        let mut actions = vec![Action::Efficiency(vec![13])];
        frame(&mut app, &mut actions);
        assert_eq!(actions, [Action::Efficiency(vec![13])]);
        assert_eq!(app.confirm_efficiency, None);
    }

    #[test]
    fn realtime_priority_asks_before_it_is_applied() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        // Set priority ▸ Realtime on two selected rows, and High as it is.
        let mut actions = vec![Action::Priority(12, "Realtime"), Action::Priority(13, "Realtime"), Action::Priority(14, "High")];
        crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut actions));
        assert_eq!(actions, [Action::Priority(14, "High")], "Realtime held for the question");
        assert_eq!(app.confirm_realtime, Some(vec![12, 13]));
        assert_eq!(REALTIME_QUESTION, "Realtime priority can make the system unresponsive. Continue?");
        // Agreed: Realtime is set on both.
        let mut sent = Vec::new();
        TaskApp::answer_realtime(&[12, 13], &mut sent);
        assert_eq!(sent, [Action::Priority(12, "Realtime"), Action::Priority(13, "Realtime")]);
    }

    /// Code.exe's Set affinity submenu read as `affinity`.
    fn affinity_menu(affinity: Option<crate::winsys::Affinity>, rows: usize) -> Vec<MenuEntry> {
        let p = proc(7, "Code.exe", "spata", 0.0, 0);
        let q = proc(8, "node.exe", "spata", 0.0, 0);
        let state = MenuState { affinity, ..no_view_state(&[]) };
        let all = [&p, &q];
        let entries = menu_entries(&p, &all[..rows], &state);
        match entries.iter().find(|e| e.label() == "Set affinity") {
            Some(MenuEntry::Sub { items, .. }) => items.clone(),
            other => vec![other.unwrap_or_else(|| panic!("Set affinity in {:?}", labels(&entries))).clone()],
        }
    }

    #[test]
    fn the_process_menu_reads_priority_affinity_and_window_through_the_seam() {
        use crate::winsys::Affinity;
        let app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let seam = app.recording();
        let (p, q) = (proc(4_000_000_001, "a.exe", "spata", 0.0, 0), proc(4_000_000_002, "b.exe", "spata", 0.0, 0));
        let mask = Affinity::Mask { process: 0b01, system: 0b11 };
        seam.script_priority(p.pid, "High");
        seam.script_affinity(p.pid, mask);
        seam.script_window(p.pid);
        let blank = MenuState { priority: None, ..no_view_state(&[]) };
        let one = crate::widgets::read_menu_state(&*seam, &p, &[&p], &blank);
        assert_eq!((one.priority, one.affinity, one.windowed), (Some("High"), Some(mask), Some(true)));
        let ticked = menu_entries(&p, &[&p], &one);
        let sub = ticked.iter().find_map(|e| match e { MenuEntry::Sub { label: "Set priority", items } => Some(items.clone()), _ => None }).expect("Set priority");
        assert!(sub.iter().any(|e| matches!(e, MenuEntry::Item { label, checked: Some(true), .. } if label == "High")), "High is ticked");
        // Several rows: no priority; the affinity is still the row's own.
        let many = crate::widgets::read_menu_state(&*seam, &p, &[&p, &q], &blank);
        assert_eq!((many.priority, many.affinity, many.windowed), (None, Some(mask), None));
        // A process the adapter knows nothing about: nothing ticked, no window.
        let none = crate::widgets::read_menu_state(&*seam, &q, &[&q], &blank);
        assert_eq!((none.priority, none.affinity, none.windowed), (None, None, Some(false)));
    }

    #[test]
    fn set_affinity_lists_every_logical_cpu_ticked_from_the_mask() {
        use crate::winsys::Affinity;
        // Four logical processors, the process on CPU 0 and CPU 2.
        let sub = affinity_menu(Some(Affinity::Mask { process: 0b0101, system: 0b1111 }), 1);
        assert_eq!(labels(&sub), ["All processors", "", "CPU 0", "CPU 1", "CPU 2", "CPU 3"]);
        let ticks: Vec<Option<bool>> = sub
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item { checked, .. } => Some(*checked),
                _ => None,
            })
            .collect();
        assert_eq!(ticks, [Some(false), Some(true), Some(false), Some(true), Some(false)]);
        assert_eq!(menu_item(&sub, "All processors"), (true, vec![Action::Affinity(vec![7], 0b1111)]));
        assert_eq!(menu_item(&sub, "CPU 1"), (true, vec![Action::Affinity(vec![7], 0b0111)]), "ticked on");
        assert_eq!(menu_item(&sub, "CPU 2"), (true, vec![Action::Affinity(vec![7], 0b0001)]), "ticked off");
        // Several rows: the new mask goes to every selected process.
        let sub = affinity_menu(Some(Affinity::Mask { process: 0b1111, system: 0b1111 }), 2);
        assert_eq!(menu_item(&sub, "CPU 3").1, [Action::Affinity(vec![7, 8], 0b0111)]);
        let full = sub.iter().find_map(|e| match e {
            MenuEntry::Item { label, checked, .. } if label == "All processors" => *checked,
            _ => None,
        });
        assert_eq!(full, Some(true), "ticked with every processor");
    }

    #[test]
    fn clearing_the_last_cpu_is_refused_with_the_message() {
        let sub = affinity_menu(Some(crate::winsys::Affinity::Mask { process: 0b0100, system: 0b1111 }), 1);
        assert_eq!(menu_item(&sub, "CPU 2"), (true, vec![Action::Say("At least one processor must remain selected.".into())]));
    }

    #[test]
    fn set_affinity_is_greyed_for_several_processor_groups_and_for_a_process_it_cannot_open() {
        let sub = affinity_menu(Some(crate::winsys::Affinity::ManyGroups), 1);
        assert_eq!(labels(&sub), ["Unavailable for this processor-group topology"]);
        assert_eq!(menu_item(&sub, "Unavailable for this processor-group topology"), (false, vec![]));
        let item = affinity_menu(None, 1);
        assert_eq!(menu_item(&item, "Set affinity"), (false, vec![]), "greyed when it cannot be read");
    }

    /// An app with a child, and a background process.
    fn tree_sample() -> crate::sampler::Snapshot {
        crate::sampler::Snapshot {
            procs: vec![
                ProcRow { app: true, handles: 4321, ..proc(100, "steam.exe", "spata", 1.0, 300_000_000) },
                ProcRow { parent: Some(100), ..proc(110, "steamwebhelper.exe", "spata", 1.0, 200_000_000) },
                proc(200, "svchost.exe", "SYSTEM", 1.0, 100_000_000),
            ],
            mem_total: 16_000_000_000,
            ..Default::default()
        }
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Default::default() }
    }

    #[test]
    fn right_and_left_open_and_close_the_selected_tree_row() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        // Nothing selected yet: the details pane would show the handles.
        let texts = crate::test_ui::texts(&crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new())));
        assert!(texts.iter().any(|t| t == "Apps (2)") && texts.iter().any(|t| t == "Background processes (1)"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "steamwebhelper.exe"), "the child starts closed");
        assert!(!texts.iter().any(|t| t == "4321"), "a parent leaves Handles empty");
        app.selected = Selection::only(100);
        let mut draw = |events| crate::test_ui::texts(&crate::test_ui::run(&ctx, (1800.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new())));
        draw(vec![key(egui::Key::ArrowRight)]);
        assert!(draw(vec![]).iter().any(|t| t == "steamwebhelper.exe"), "Right opens the row");
        draw(vec![key(egui::Key::ArrowLeft)]);
        assert!(!draw(vec![]).iter().any(|t| t == "steamwebhelper.exe"), "Left closes it");
    }

    #[test]
    fn double_clicking_a_group_header_folds_and_unfolds_it() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), tree_sample());
        // What the page asked for: a header's double-click inspects nothing.
        let mut actions = Vec::new();
        // A fresh context for each double-click: egui's clicks share one
        // instant here, so a second pair would count as a triple click.
        let mut double_click = || {
            let ctx = crate::test_ui::context(theme::Mode::Dark);
            let mut draw = |events| crate::test_ui::run(&ctx, (1800.0, 900.0), events, |ui| app.page_processes(ui, &mut actions));
            let header = text_spots(&draw(vec![])).into_iter().find(|(t, _)| t == "Background processes (1)").expect("the header").1.center();
            let press = |pressed| egui::Event::PointerButton { pos: header, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
            draw(vec![egui::Event::PointerMoved(header), press(true)]);
            draw(vec![press(false)]);
            draw(vec![press(true)]);
            draw(vec![press(false)]);
            crate::test_ui::texts(&draw(vec![]))
        };
        let folded = double_click();
        assert!(!folded.iter().any(|t| t == "svchost.exe"), "{folded:?}");
        assert!(folded.iter().any(|t| t == "Background processes (1)"), "the header stays");
        assert!(double_click().iter().any(|t| t == "svchost.exe"), "a second double-click unfolds it");
        assert!(actions.is_empty(), "a group header's double-click opens no Inspector: {actions:?}");
    }

    /// The headings of the Processes table, left to right.
    fn headings_drawn(app: &mut TaskApp) -> Vec<String> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (3000.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        let mut found: Vec<(f32, String)> = text_spots(&out)
            .into_iter()
            .filter_map(|(text, r)| PROC_COLUMNS.iter().find(|c| is_heading(&text, c.title)).map(|c| (r.left(), c.title.to_string())))
            .collect();
        found.sort_by(|a, b| a.0.total_cmp(&b.0));
        found.dedup_by(|a, b| a.1 == b.1);
        found.into_iter().map(|(_, title)| title).collect()
    }

    const ALL_TITLES: [&str; 17] = [
        "Name", "PID", "Status", "User name", "CPU", "Memory", "Disk read", "Disk write", "Threads", "GPU", "Peak memory", "CPU time",
        "Page faults", "Handles", "Priority", "Architecture", "Command",
    ];

    #[test]
    fn hiding_a_column_removes_it_and_showing_it_again_restores_its_old_slot() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        set_column_shown(&mut app.settings.process_table, ProcCol::Threads, false);
        let hidden = headings_drawn(&mut app);
        assert!(!hidden.iter().any(|t| t == "Threads"), "{hidden:?}");
        assert_eq!(hidden.len(), 16);
        // Columns move around it while it is hidden.
        move_column(&mut app.settings.process_table, ProcCol::Cmd, ProcCol::Name);
        set_column_shown(&mut app.settings.process_table, ProcCol::Threads, true);
        let shown = headings_drawn(&mut app);
        let at = shown.iter().position(|t| t == "Threads").expect("Threads is back");
        assert_eq!((shown[at - 1].as_str(), shown[at + 1].as_str()), ("Disk write", "GPU"), "{shown:?}");
        assert_eq!(shown[0], "Command", "the move is kept");
    }

    #[test]
    fn the_chooser_lists_every_column_and_keeps_name_shown() {
        let mut table = crate::settings::ProcessTable::default();
        set_column_shown(&mut table, ProcCol::Status, false);
        set_column_shown(&mut table, ProcCol::Name, false);
        let choices = column_choices(&table);
        let titles: Vec<&str> = choices.iter().map(|c| c.title).collect();
        assert_eq!(titles[..17], ALL_TITLES, "in the table's own order");
        assert_eq!(titles[17..], ["Publisher", "Description"], "then the additions");
        let of = |col| choices.iter().find(|c| c.col == col).unwrap();
        assert!(!of(ProcCol::Status).shown);
        assert!(of(ProcCol::Name).shown && !of(ProcCol::Name).enabled, "Name cannot be hidden");
        assert_eq!(columns_label(&table), "Columns 16 of 19");
    }

    #[test]
    fn the_column_layout_and_sort_survive_a_restart() {
        let mut settings = crate::settings::Settings::default();
        let t = &mut settings.process_table;
        move_column(t, ProcCol::Pid, ProcCol::Mem);
        set_column_shown(t, ProcCol::Handles, false);
        set_column_width(t, ProcCol::Cmd, 300.0);
        set_proc_sort(t, (ProcCol::Name, false));
        t.column_order.push("from_a_newer_version".into());
        let before = settings.process_table.clone();
        let restarted = crate::settings::Settings::from_json(&settings.to_json());
        assert_eq!(restarted.process_table, before);
        let saved: serde_json::Value = serde_json::from_str(&settings.to_json()).unwrap();
        assert_eq!(saved["process_table"]["sort_column"], "name");
        assert_eq!(saved["process_table"]["sort_direction"], "ascending");
        assert_eq!(saved["process_table"]["column_widths"]["command"], 300.0);

        let mut app = TaskApp::headless(restarted, disk_sample());
        let drawn = headings_drawn(&mut app);
        let mut expected: Vec<&str> = ALL_TITLES.iter().copied().filter(|t| *t != "Handles" && *t != "PID").collect();
        expected.insert(expected.iter().position(|t| *t == "Memory").unwrap() + 1, "PID");
        assert_eq!(drawn, expected);
        assert_eq!(proc_sort(&app.settings.process_table), (ProcCol::Name, false));
        let width = proc_layout(&app.settings.process_table).iter().find(|p| p.col == ProcCol::Cmd).unwrap().width;
        assert_eq!(width, 300.0);
    }

    #[test]
    fn a_fresh_install_shows_all_columns_sorted_by_memory_largest_first() {
        for settings in [crate::settings::Settings::default(), crate::settings::Settings::from_json("{}")] {
            assert_eq!(proc_sort(&settings.process_table), (ProcCol::Mem, true));
            let layout = proc_layout(&settings.process_table);
            // Every standard column shows; the marked additions do not.
            assert!(layout.iter().all(|p| p.shown == !matches!(p.col, ProcCol::Publisher | ProcCol::Description)), "{layout:?}");
            let widths: Vec<f32> = layout.iter().map(|p| p.width).collect();
            assert_eq!(widths, PROC_COLUMNS.iter().map(|c| c.width).collect::<Vec<_>>(), "the default widths");
            let mut snap = disk_sample();
            snap.procs = vec![proc(1, "small.exe", "spata", 50.0, 1_000), proc(2, "big.exe", "spata", 1.0, 9_000), proc(3, "mid.exe", "spata", 9.0, 5_000)];
            let mut app = TaskApp::headless(settings, snap);
            assert_eq!(headings_drawn(&mut app), ALL_TITLES);
            let ctx = crate::test_ui::context(theme::Mode::Dark);
            let out = crate::test_ui::run(&ctx, (3000.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
            let mut rows: Vec<(f32, String)> = text_spots(&out).into_iter().filter(|(t, _)| t.ends_with(".exe")).map(|(t, r)| (r.top(), t)).collect();
            rows.sort_by(|a, b| a.0.total_cmp(&b.0));
            let names: Vec<&str> = rows.iter().map(|(_, t)| t.as_str()).filter(|t| ["small.exe", "big.exe", "mid.exe"].contains(t)).collect();
            assert_eq!(names, ["big.exe", "mid.exe", "small.exe"]);
        }
    }

    #[test]
    fn a_column_hidden_by_default_shows_only_once_the_user_shows_it() {
        static TABLE: [ProcColumn; 3] = [
            column(ProcCol::Name, "name", "Name", 165.0),
            ProcColumn { col: ProcCol::Cmd, key: "command", title: "Command", width: 135.0, hidden: true },
            column(ProcCol::Pid, "pid", "PID", 50.0),
        ];
        let shown = |t: &crate::settings::ProcessTable| layout_of(&TABLE, t).iter().filter(|p| p.shown).map(|p| p.col).collect::<Vec<_>>();
        let mut t = crate::settings::ProcessTable::default();
        assert_eq!(shown(&t), [ProcCol::Name, ProcCol::Pid], "fresh");
        // A layout saved before the column existed: it arrives hidden, in its slot.
        t.column_order = vec!["pid".into(), "name".into()];
        assert_eq!(layout_of(&TABLE, &t).iter().map(|p| p.col).collect::<Vec<_>>(), [ProcCol::Pid, ProcCol::Name, ProcCol::Cmd]);
        assert_eq!(shown(&t), [ProcCol::Pid, ProcCol::Name]);
        set_column_shown_in(&TABLE, &mut t, ProcCol::Cmd, true);
        assert_eq!(shown(&t), [ProcCol::Pid, ProcCol::Name, ProcCol::Cmd]);
    }

    #[test]
    fn dragging_a_header_moves_its_column_and_a_divider_saves_the_width() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let mut draw = |events| crate::test_ui::run(&ctx, (3000.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new()));
        let spot = |out: &egui::FullOutput, title: &str| text_spots(out).into_iter().find(|(t, _)| is_heading(t, title)).unwrap().1;
        let out = draw(vec![]);
        let (from, onto) = (spot(&out, "Status").center(), spot(&out, "CPU").center());
        let press = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        draw(vec![egui::Event::PointerMoved(from), press(from, true)]);
        for step in 1..=10 {
            draw(vec![egui::Event::PointerMoved(from + (onto - from) * step as f32 / 10.0)]);
        }
        draw(vec![press(onto, false)]);
        draw(vec![]);
        let drawn = headings_drawn(&mut app);
        assert_eq!(&drawn[..5], ["Name", "PID", "User name", "CPU", "Status"], "{drawn:?}");

        // The Name column's divider sits halfway in the gap after its 165
        // points.
        let mut draw = |events| crate::test_ui::run(&ctx, (3000.0, 900.0), events, |ui| app.page_processes(ui, &mut Vec::new()));
        let out = draw(vec![]);
        let (name, pid) = (spot(&out, "Name"), spot(&out, "PID"));
        let divider = egui::pos2((name.left() + 165.0 + pid.left()) / 2.0, name.center().y);
        draw(vec![egui::Event::PointerMoved(divider), press(divider, true)]);
        for step in 1..=10 {
            draw(vec![egui::Event::PointerMoved(divider + vec2(6.0 * step as f32, 0.0))]);
        }
        draw(vec![press(divider + vec2(60.0, 0.0), false)]);
        draw(vec![]);
        let width = proc_layout(&app.settings.process_table).iter().find(|p| p.col == ProcCol::Name).unwrap().width;
        assert!((width - 225.0).abs() <= 2.0, "{width}");
    }

    #[test]
    fn column_changes_on_the_processes_page_are_saved_once_the_mouse_is_up() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        assert!(!app.process_table_unsaved(false), "nothing changed");
        set_column_shown(&mut app.settings.process_table, ProcCol::Pid, false);
        assert!(!app.process_table_unsaved(true), "not while a divider is dragged");
        assert!(app.process_table_unsaved(false));
        assert!(!app.process_table_unsaved(false), "saved once");
    }

    #[test]
    fn the_connections_column_counts_each_processs_connections_once_shown() {
        use crate::addons::connections::inventory::{assemble, Proto, Row, TcpState};
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = |pid, port: u16| Row { proto: Proto::Tcp, local: format!("0.0.0.0:{port}").parse().unwrap(), remote: None, state: Some(TcpState::Listening), pid };
        let rows = vec![listener(41, 135), listener(41, 445), listener(41, 5040), listener(42, 8080)];
        let reads = std::sync::Arc::new(AtomicUsize::new(0));
        let counter = reads.clone();
        let fake = crate::addons::connections::Connections::with_reader(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            assemble([Ok(rows.clone()), Ok(vec![]), Ok(vec![]), Ok(vec![])])
        });
        let mut snap = disk_sample();
        snap.procs = vec![proc(41, "many.exe", "spata", 1.0, 3_000), proc(42, "one.exe", "spata", 1.0, 2_000), proc(43, "none.exe", "spata", 1.0, 1_000)];
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        app.addons = crate::addons::AddonHost::for_tests_with(vec![crate::addons::Entry::Addon(Box::new(fake))], &serde_json::json!({})).0;
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let draw = |app: &mut TaskApp| text_spots(&crate::test_ui::run(&ctx, (3200.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new())));
        // An addition: hidden until the user shows it, and nothing read meanwhile.
        let spots = draw(&mut app);
        assert!(!spots.iter().any(|(t, _)| t == "Connections"), "hidden by default");
        let cols = columns_with(&app.addons.process_columns());
        let choices = column_choices_in(&cols, &app.settings.process_table);
        assert_eq!(choices.last().map(|c| (c.title, c.shown)), Some(("Connections", false)), "last in the chooser, unticked");
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(reads.load(Ordering::SeqCst), 0);

        set_column_shown_in(&cols, &mut app.settings.process_table, ProcCol::Addon("connections"), true);
        let mut spots = Vec::new();
        crate::addons::until("the counts", || {
            spots = draw(&mut app);
            spots.iter().any(|(t, _)| t == "3")
        });
        let heading = spots.iter().find(|(t, _)| t == "Connections").expect("a Connections heading").1;
        let row_of = |name: &str| spots.iter().find(|(t, _)| t == name).unwrap().1.center().y;
        let in_column = |shown: &str, name: &str| {
            spots.iter().any(|(t, r)| t == shown && (r.center().y - row_of(name)).abs() < 4.0 && r.left() >= heading.left() - 8.0 && r.left() < heading.left() + 80.0)
        };
        assert!(in_column("3", "many.exe") && in_column("1", "one.exe") && in_column("0", "none.exe"), "{spots:?}");
    }

    #[test]
    fn publisher_and_description_are_hidden_until_chosen_then_show_the_programs_signer_and_description() {
        // The test's own executable: build.rs gives it a description, and
        // nothing signs it.
        let me = std::env::current_exe().expect("own path").display().to_string();
        let mut snap = disk_sample();
        snap.procs = vec![ProcRow { exe: me, ..proc(77, "tests.exe", "spata", 1.0, 1_000) }];
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        assert_eq!(headings_drawn(&mut app), ALL_TITLES, "hidden by default");
        let choices = column_choices(&app.settings.process_table);
        let of = |title| choices.iter().find(|c| c.title == title).map(|c| (c.shown, c.enabled));
        assert_eq!((of("Publisher"), of("Description")), (Some((false, true)), Some((false, true))), "unticked in the chooser");
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let draw = |app: &mut TaskApp| crate::test_ui::texts(&crate::test_ui::run(&ctx, (3600.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new())));
        assert!(!draw(&mut app).iter().any(|t| t == "Ultimate PC Manager"), "nothing read while hidden");

        set_column_shown(&mut app.settings.process_table, ProcCol::Publisher, true);
        set_column_shown(&mut app.settings.process_table, ProcCol::Description, true);
        let mut texts = Vec::new();
        crate::addons::until("the program file's details", || {
            texts = draw(&mut app);
            texts.iter().any(|t| t == "Ultimate PC Manager") && texts.iter().any(|t| t == "Not verified")
        });
        assert!(headings_drawn(&mut app).ends_with(&["Publisher".to_string(), "Description".to_string()]));
    }

    #[test]
    fn with_connections_off_neither_its_column_nor_its_menu_item_shows() {
        let p = proc(42, "one.exe", "spata", 1.0, 2_000);
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        let links = |app: &mut TaskApp| {
            let extra = app.addons.process_columns();
            let links: Vec<(&'static str, &'static str)> = extra.iter().map(|(id, c)| (*id, c.menu_item)).collect();
            crate::widgets::process_links(&links, &p)
        };
        assert_eq!(links(&mut app), [("Go to connections", Action::ShowProcessIn { addon: "connections", pid: 42 })], "on: the item");
        // The user showed the column while it was on, then turned it off.
        let cols = columns_with(&app.addons.process_columns());
        set_column_shown_in(&cols, &mut app.settings.process_table, ProcCol::Addon("connections"), true);
        app.addons.set_enabled("connections", false);
        assert!(links(&mut app).is_empty(), "off: no item");
        let cols = columns_with(&app.addons.process_columns());
        assert!(!column_choices_in(&cols, &app.settings.process_table).iter().any(|c| c.title == "Connections"), "nor a line in the chooser");
        let drawn = headings_drawn(&mut app);
        assert_eq!(drawn, ALL_TITLES, "nor a column");
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (3200.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        assert!(!crate::test_ui::texts(&out).iter().any(|t| t == "Connections"));
        assert!(app.settings.process_table.column_order.iter().any(|k| k == "connections"), "its saved slot is kept for when it is on again");
    }

    #[test]
    fn system_info_keeps_every_field_in_one_card() {
        let keys = [
            "Computer name", "Operating system", "Version", "Kernel", "Architecture", "Processor", "Cores",
            "Logical processors", "Speed", "Installed memory", "Page file", "Drives", "Network adapters", "Up time",
        ];
        let fields: Vec<(&str, String)> = keys.iter().map(|k| (*k, format!("value of {k}"))).collect();
        let sections = system_sections(&fields);
        let shown: Vec<(&str, String)> = sections.iter().flat_map(|s| s.fields.clone()).collect();
        assert_eq!(shown.len(), keys.len());
        for k in keys {
            assert_eq!(shown.iter().filter(|(f, v)| *f == k && *v == format!("value of {k}")).count(), 1, "{k}");
        }
        let titles: Vec<&str> = sections.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Operating System", "Processor", "Memory", "Storage", "Network"]);
    }

    #[test]
    fn users_are_the_accounts_running_processes() {
        let accounts = accounts(&sample());
        let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["System", "spata"]);
        let spata = &accounts[1];
        assert_eq!(spata.procs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["chrome.exe", "Code.exe", "explorer.exe"], "by memory");
        assert!((spata.cpu - 8.2).abs() < 1e-4);
        assert_eq!(spata.mem, 10_770_000_000);
        assert_eq!(users_headline(&accounts), "2 accounts are running processes");
        assert_eq!(users_headline(&accounts[1..]), "Only spata is running processes");
        assert_eq!(users_headline(&[]), "No user sessions were reported");
    }

    #[test]
    fn both_hiding_settings_are_on_the_settings_page_not_in_process_view() {
        const LABELS: [&str; 2] = ["Show Task Manager in Reports", "Hide Microsoft Services and Processes"];
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = TaskApp::headless(crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let texts = crate::test_ui::texts(&crate::test_ui::run(&ctx, (1400.0, 2400.0), vec![], |ui| app.page_settings(ui, &mut Vec::new())));
        for label in LABELS {
            assert!(texts.iter().any(|t| t == label), "Settings shows {label}: {texts:?}");
        }
        assert!(texts.iter().any(|t| t.starts_with("Hides services and processes whose program file carries a verified Microsoft Windows signature")));
        let menu: Vec<&str> = app
            .process_view_items()
            .iter()
            .filter_map(|i| match i {
                parts::DropItem::Item { label, .. } => Some(*label),
                parts::DropItem::Separator => None,
            })
            .collect();
        assert!(LABELS.iter().all(|l| !menu.contains(l)), "Process View lists {menu:?}");
    }

    #[test]
    fn the_csv_holds_the_shown_rows_in_the_visible_columns_in_their_order() {
        let mut saved = crate::settings::ProcessTable::default();
        for c in PROC_COLUMNS.iter().filter(|c| !matches!(c.col, ProcCol::Name | ProcCol::Pid | ProcCol::Cpu | ProcCol::User)) {
            set_column_shown(&mut saved, c.col, false);
        }
        set_column_shown(&mut saved, ProcCol::User, false);
        move_column(&mut saved, ProcCol::Pid, ProcCol::Name);
        let rows = vec![proc(9, "plain.exe", "SYSTEM", 0.0, 0), proc(7, "say \"hi\", world.exe", "spata", 12.5, 0)];
        let opts = ViewOptions { tree: false, sort: (ProcCol::Pid, false), ..Default::default() };
        let v = process_view::view(&rows, &opts, &Default::default());
        let csv = process_csv(v.processes(), &PROC_COLUMNS, &saved, &Totals::default(), false, &Figures::new(), &FileTexts::default());
        assert_eq!(csv, "PID,Name,CPU\n7,\"say \"\"hi\"\", world.exe\",12.5%\n9,plain.exe,0.0%\n");
    }

    /// One frame of the Processes page with `events`, its actions kept.
    fn processes_frame_into(ctx: &egui::Context, app: &mut TaskApp, events: Vec<egui::Event>, actions: &mut Vec<Action>) -> egui::FullOutput {
        let input = egui::RawInput { screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1800.0, 900.0))), events, ..Default::default() };
        ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.page_processes(ui, actions));
        })
    }

    /// The actions a click on the text `label` of the Processes page queues.
    fn click_text(app: &mut TaskApp, label: &str) -> Vec<Action> {
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let out = processes_frame_into(&ctx, app, vec![], &mut Vec::new());
        let at = text_spots(&out).into_iter().find(|(t, _)| t == label).unwrap_or_else(|| panic!("{label} is drawn")).1.center();
        let press = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        let mut actions = Vec::new();
        processes_frame_into(&ctx, app, vec![egui::Event::PointerMoved(at), press(true)], &mut actions);
        processes_frame_into(&ctx, app, vec![press(false)], &mut actions);
        actions
    }

    #[test]
    fn pause_and_resume_in_the_table_toolbar_set_the_apps_update_speed() {
        let paused = crate::settings::SPEEDS.iter().position(|s| s.1 == 0).expect("a Paused speed");
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        assert!(processes_texts(&mut app).iter().any(|t| t == "Update speed"), "the speed sits by Pause");
        assert_eq!(click_text(&mut app, "Pause"), [Action::UpdateSpeed(paused)]);
        app.settings.speed = paused;
        assert_eq!(click_text(&mut app, "Resume"), [Action::UpdateSpeed(crate::settings::Settings::default().speed)], "paused, the button resumes");
    }

    #[test]
    fn export_to_csv_sends_the_shown_rows_and_columns() {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), disk_sample());
        set_column_shown(&mut app.settings.process_table, ProcCol::Status, false);
        let actions = click_text(&mut app, "Export to CSV");
        let [Action::ExportCsv { file_name, text }] = actions.as_slice() else { panic!("one export: {actions:?}") };
        assert_eq!(*file_name, "processes.csv");
        let mut lines = text.lines();
        assert!(lines.next().is_some_and(|h| h.starts_with("Name,PID,User name,CPU,")), "Status is hidden: {text}");
        assert_eq!(lines.filter(|l| l.contains("reads-")).count(), 3, "every shown process: {text}");
    }

    #[test]
    fn a_selected_service_row_shows_its_service_fields_and_ends_nothing() {
        let mut snap = disk_sample();
        let info = crate::sampler::ServiceInfo { name: "Winmgmt".into(), display_name: "Windows Management Instrumentation".into(), host: "reads-most.exe (12)".into(), ..Default::default() };
        snap.services = std::sync::Arc::new([(12, vec![info])].into_iter().collect());
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        app.proc_view.expanded.insert(12);
        click_text(&mut app, "Service: Winmgmt (Windows Management Instrumentation)");
        let texts = processes_texts(&mut app);
        assert!(texts.iter().any(|t| t == "Service name") && texts.iter().any(|t| t == "Winmgmt"), "{texts:?}");
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        processes_frame_into(&ctx, &mut app, vec![key(egui::Key::Delete)], &mut Vec::new());
        assert_eq!(app.confirm_end, None, "Delete on a service does not end its host");
    }
}
