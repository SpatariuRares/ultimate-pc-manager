//! Summary: the status band, the four cards, the busiest processes and the
//! tiles. It owns its view (built once per new sample, and again when a
//! setting it shows changes) and the latest sensor frame its tiles read;
//! its sensors come from the host lease, asked every frame it is drawn.

use std::sync::Arc;

use egui::{vec2, Color32, RichText, Sense, Ui};

use crate::addons::AddonHost;
use crate::core_page::{CorePage, PageCtx, SampleSlot};
use crate::platform::{Action, PageContext};
use crate::sampler::ProcRow;
use crate::sensors::Frame;
use crate::summary_view::{Built, Metric, SummaryView};
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, Mark, StatusBand};
use crate::widgets::*;

#[derive(Default)]
pub struct SummaryPage {
    /// What the page draws; `None` until the first draw or sample.
    view: Option<SummaryView>,
    /// The latest sensor frame the tiles read (GPU temperature and power,
    /// S.M.A.R.T.); `None` when no sensor has reported.
    frame: Option<Arc<Frame>>,
}

impl SummaryPage {
    /// The page left the screen: its old sensor frame goes, so coming back
    /// never shows it.
    pub fn drop_frame(&mut self) {
        self.frame = None;
    }

    /// Builds the view from the sample in `ctx`.
    fn rebuild(&mut self, ctx: &PageCtx) {
        self.view = Some(SummaryView::build(ctx, self.frame.as_deref()));
    }

    /// The view, rebuilt first if the pause, the network units or "Show Task
    /// Manager in Reports" changed since it was built (the figures stay
    /// those of the last sample).
    pub fn view(&mut self, ctx: &PageCtx) -> &SummaryView {
        if self.view.as_ref().map(|v| v.built) != Some(Built::of(ctx.settings)) {
            self.rebuild(ctx);
        }
        self.view.get_or_insert_with(Default::default)
    }

    /// Asks the host for the sensors while Summary is drawn (the host keeps
    /// them while this is called every frame and drops them after) and,
    /// unless paused, puts a new sensor frame into the tiles at once.
    fn read_sensors(&mut self, ctx: &PageCtx, host: &AddonHost) {
        let sensors = host.core_sensors(crate::addons::leases::Surface::Summary);
        let interval_ms = ctx.settings.interval_ms();
        sensors.set_interval_ms(interval_ms);
        if interval_ms == 0 {
            // Paused: the tiles keep the last view.
            return;
        }
        let Some(frame) = sensors.latest() else { return };
        if self.frame.as_ref().map(|f| f.seq) != Some(frame.seq) {
            self.frame = Some(frame);
            if let Some(view) = &mut self.view {
                view.tiles = crate::summary_tiles::summary_tiles(ctx, self.frame.as_deref());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn has_frame(&self) -> bool {
        self.frame.is_some()
    }
}

impl CorePage for SummaryPage {
    fn title(&self) -> Option<&'static str> {
        Some("Summary")
    }

    fn sample_slot(&self) -> SampleSlot {
        SampleSlot::Summary
    }

    fn on_sample(&mut self, ctx: &PageCtx) {
        self.rebuild(ctx);
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        self.draw_page(ui, ctx, None, actions);
    }

    fn draw_with_addons(&mut self, ui: &mut Ui, ctx: &PageCtx, host: &mut AddonHost, actions: &mut Vec<Action>) {
        self.draw_page(ui, ctx, Some(host), actions);
    }
}

impl SummaryPage {
    /// Draws the page; with the add-on host it also holds the sensors and
    /// draws the add-ons' tiles.
    fn draw_page(&mut self, ui: &mut Ui, ctx: &PageCtx, mut host: Option<&mut AddonHost>, actions: &mut Vec<Action>) {
        let t = ctx.settings.theme();
        // Drawn from the view built once per sample (see `summary_view`).
        self.view(ctx);
        if let Some(host) = host.as_deref() {
            self.read_sensors(ctx, host);
        }
        let Some(view) = self.view.as_ref() else { return };
        let colour = |m: Metric| match m {
            Metric::Cpu => rgb(ctx.settings.cpu_color),
            Metric::Memory => rgb(ctx.settings.mem_color),
            Metric::Disk => rgb(ctx.settings.disk_color),
            Metric::Network => rgb(ctx.settings.net_color),
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
            if let Some(host) = host.as_deref_mut() {
                let mut page = PageContext::new(ctx.settings, ctx.snapshot);
                host.summary_tiles(ui, &mut page);
                actions.append(&mut page.actions);
            }
        });

    }
}

#[cfg(test)]
impl crate::app::TaskApp {
    /// Rebuilds Summary's view from the app's latest sample (for tests that
    /// set `snap` or `machine_info` directly).
    pub(crate) fn rebuild_summary(&mut self) {
        let snap = self.snapshot_with_machine();
        let histories = crate::core_page::Histories { cpu: &self.cpu_hist, mem: &self.mem_hist, perf: &self.ph };
        self.pages.summary.on_sample(&PageCtx { settings: &self.settings, snapshot: &snap, histories, system: &*self.system, processes: crate::page_processes::ProcessInputs::new(&self.signatures, &*self.actions) });
    }

    /// Summary's view as it would be drawn now.
    pub(crate) fn summary_view(&mut self) -> &SummaryView {
        let snap = self.snapshot_with_machine();
        let histories = crate::core_page::Histories { cpu: &self.cpu_hist, mem: &self.mem_hist, perf: &self.ph };
        self.pages.summary.view(&PageCtx { settings: &self.settings, snapshot: &snap, histories, system: &*self.system, processes: crate::page_processes::ProcessInputs::new(&self.signatures, &*self.actions) })
    }

    /// Draws Summary once, so that it asks the host for its sensors.
    pub(crate) fn read_summary_sensors(&mut self) {
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        crate::test_ui::run(&ctx, (1400.0, 900.0), vec![], |ui| {
            self.page = crate::app::Page::Core(crate::addons::Core::Summary);
            self.draw_page(ui, &mut Vec::new())
        });
    }

    /// The latest snapshot with the machine facts a test set on the app.
    fn snapshot_with_machine(&self) -> crate::sampler::Snapshot {
        let mut snap = self.snap.clone();
        if let Some(info) = &self.machine_info {
            snap.machine = Some(Arc::new(info.clone()));
        }
        snap
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
mod tests {
    use super::*;
    use crate::core_page::testing::{draw, draw_hosted_in, draw_in};
    use crate::core_page::{Histories, PerfHistory};
    use crate::perfdata::PerfLive;
    use crate::sampler::{NetRow, Snapshot};
    use crate::settings::{Settings, SPEEDS};

    fn proc(pid: u32, name: &str, user: &str, cpu: f32, mem: u64) -> ProcRow {
        ProcRow { pid, name: name.into(), user: user.into(), cpu, mem, ..Default::default() }
    }

    fn procs() -> Vec<ProcRow> {
        vec![
            proc(0, "System Idle Process", "", 90.0, 0),
            proc(4, "System", "", 0.4, 0),
            proc(35308, "chrome.exe", "spata", 0.2, 6_000_000_000),
            proc(16244, "Code.exe", "spata", 8.0, 4_300_000_000),
            proc(8944, "explorer.exe", "spata", 0.0, 470_000_000),
        ]
    }

    /// A sample with Code.exe busy and 2 MB/s on the network.
    fn sample() -> Snapshot {
        Snapshot {
            seq: 1,
            cpu_total: 15.4,
            cpu_per_core: vec![20.0, 10.0],
            mem_total: 64 << 30,
            mem_used: 25 << 30,
            perf: PerfLive { cpu_perf_pct: 110.0, ..Default::default() },
            net: vec![NetRow { name: "Ethernet".into(), rx_bps: 1_500_000.0, tx_bps: 500_000.0, total_rx: 9, total_tx: 9 }],
            procs: procs(),
            ..Default::default()
        }
    }

    fn has(texts: &[String], wanted: &str) -> bool {
        texts.iter().any(|t| t == wanted)
    }

    /// The app's shared histories, empty.
    #[derive(Default)]
    struct Empty {
        short: std::collections::VecDeque<f32>,
        perf: PerfHistory,
        system: crate::system_lists::fake::FakeSystem,
        standalone: crate::page_processes::Standalone,
    }

    fn ctx_of<'a>(settings: &'a Settings, snapshot: &'a Snapshot, empty: &'a Empty) -> PageCtx<'a> {
        PageCtx { settings, snapshot, histories: Histories { cpu: &empty.short, mem: &empty.short, perf: &empty.perf }, system: &empty.system, processes: empty.standalone.inputs() }
    }

    #[test]
    fn summary_draws_the_cards_and_the_busiest_processes_from_a_snapshot() {
        let drawn = draw(&mut SummaryPage::default(), &Settings::default(), &sample());
        for wanted in ["CPU", "Memory", "Disk", "Network", "15%", "Code.exe", "16244"] {
            assert!(has(&drawn.texts, wanted), "{wanted} is drawn: {:?}", drawn.texts);
        }
        assert!(drawn.actions.is_empty());
    }

    #[test]
    fn summary_is_a_page_with_the_standard_header_that_a_sample_reaches_after_the_histories() {
        let page = SummaryPage::default();
        assert_eq!(page.title(), Some("Summary"));
        assert_eq!(page.sample_slot(), SampleSlot::Summary);
    }

    #[test]
    fn a_new_sample_changes_what_summary_draws() {
        let settings = Settings::default();
        let empty = Empty::default();
        let first = sample();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let drawn = draw_in(&ctx, &mut page, &ctx_of(&settings, &first, &empty));
        assert!(has(&drawn.texts, "15%"));
        let second = Snapshot { seq: 2, cpu_total: 99.0, ..sample() };
        let page_ctx = ctx_of(&settings, &second, &empty);
        page.on_sample(&page_ctx);
        let drawn = draw_in(&ctx, &mut page, &page_ctx);
        assert!(has(&drawn.texts, "99%"), "the new sample is drawn: {:?}", drawn.texts);
        assert!(!has(&drawn.texts, "15%"));
    }

    #[test]
    fn summary_rebuilds_when_the_network_units_change_without_a_new_sample() {
        let mut settings = Settings::default();
        settings.bits_per_second = true;
        let empty = Empty::default();
        let snap = sample();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let before = draw_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty));
        assert!(has(&before.texts, "16.0 Mbps"), "{:?}", before.texts);
        settings.bits_per_second = false;
        let after = draw_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty));
        assert!(has(&after.texts, "1.9 MB/s"), "{:?}", after.texts);
        assert!(!has(&after.texts, "16.0 Mbps"));
    }

    #[test]
    fn paused_summary_says_so() {
        let mut settings = Settings::default();
        settings.speed = SPEEDS.iter().position(|s| s.1 == 0).expect("a Paused speed");
        let drawn = draw(&mut SummaryPage::default(), &settings, &sample());
        assert!(has(&drawn.texts, "Updates paused"), "{:?}", drawn.texts);
    }

    #[test]
    fn a_custom_cpu_colour_reaches_summary() {
        let (settings, cpu) = crate::pages::tests::custom_cpu();
        let snap = Snapshot::default();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        let empty = Empty::default();
        let page_ctx = ctx_of(&settings, &snap, &empty);
        let out = crate::test_ui::run(&ctx, (1200.0, 800.0), vec![], |ui| page.draw(ui, &page_ctx, &mut Vec::new()));
        assert!(crate::test_ui::colours(&out).contains(&cpu));
    }

    /// The test add-on host: a made-up CPU, no real sensor.
    fn host() -> crate::addons::AddonHost {
        crate::addons::AddonHost::for_tests(&serde_json::json!({})).0
    }

    #[test]
    fn summary_holds_the_sensors_only_while_it_is_drawn() {
        let (settings, snap, empty) = (Settings::default(), sample(), Empty::default());
        let mut addons = host();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        draw_hosted_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty), &mut addons);
        crate::addons::until("the sensors run", || addons.context().sensors().threads() > 0);
        addons.end_frame();
        draw_hosted_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty), &mut addons); // still drawn: kept
        addons.end_frame();
        assert!(addons.context().sensors().threads() > 0, "still drawn: kept");
        addons.end_frame(); // another page: Summary did not ask
        crate::addons::until("the sensors stop", || addons.context().sensors().threads() == 0);
    }

    #[test]
    fn summary_fills_its_sensor_tiles_from_the_sensors_while_it_is_drawn() {
        let (settings, snap, empty) = (Settings::default(), sample(), Empty::default());
        let mut addons = host();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        crate::addons::until("a sensor frame reaches the tiles", || {
            draw_hosted_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty), &mut addons);
            page.has_frame()
        });
    }

    #[test]
    fn dropping_the_frame_when_summary_leaves_means_coming_back_never_shows_it() {
        let (settings, snap, empty) = (Settings::default(), sample(), Empty::default());
        let mut addons = host();
        let mut page = SummaryPage::default();
        let ctx = crate::test_ui::context(theme::Mode::Dark);
        crate::addons::until("a sensor frame reaches the tiles", || {
            draw_hosted_in(&ctx, &mut page, &ctx_of(&settings, &snap, &empty), &mut addons);
            page.has_frame()
        });
        page.drop_frame();
        assert!(!page.has_frame(), "the old frame is gone");
    }

    #[test]
    fn the_summary_band_sums_up_cpu_speed_memory_and_processes() {
        assert_eq!(summary_sub(15.42, 4900, 40.5, 389), "CPU 15.4% at 4.90 GHz \u{b7} memory 40.5% \u{b7} 389 processes");
        assert_eq!(summary_sub(0.0, 0, 0.0, 1), "CPU 0.0% \u{b7} memory 0.0% \u{b7} 1 process", "no speed reading: no speed");
    }

    #[test]
    fn summary_lists_the_busiest_processes_without_the_idle_one() {
        let top: Vec<u32> = top_by_cpu(&procs(), 3).iter().map(|p| p.pid).collect();
        assert_eq!(top, vec![16244, 4, 35308], "by CPU, the idle process (pid 0) left out");
        let mut tied = procs();
        tied.push(proc(77, "tied.exe", "spata", 0.2, 9_000_000_000));
        let top: Vec<u32> = top_by_cpu(&tied, 4).iter().map(|p| p.pid).collect();
        assert_eq!(top, vec![16244, 4, 77, 35308], "equal CPU: more memory first");
    }

}
