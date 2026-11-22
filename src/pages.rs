//! Summary and Processes pages (System Info and Users are in `page_system`
//! and `page_users`).

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Ui};
use egui_extras::Column;

use crate::app::{Action, ProcCol, TaskApp};
use crate::settings::SPEEDS;
use crate::page_perf::fmt_bytes_rate;
use crate::platform::PageContext;
use crate::process_view::{self, figure_of, percent, Chevron, FileTexts, Figures, Line, Pick, ProcessLine, Selection, Totals, ViewOptions};
use crate::sampler::ProcRow;
use crate::summary_view::Metric;
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, cell_text, BandArea, CellInk, Mark, SidePanel, StatusBand, TABLE_ROW_HEIGHT};
use crate::widgets::*;
#[cfg(test)]
use crate::page_system::system_sections;
#[cfg(test)]
use crate::page_users::{accounts, users_headline};

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
}
