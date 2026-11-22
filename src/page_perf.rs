//! The Performance page: a list of devices on
//! the left (CPU, Memory, physical disks, Network, GPUs, CPU Power) and the
//! chosen device on the right — its name in the display face, an LED meter
//! with the current value, its graphs and its figures.
//!
//! The graphs draw smooth curves with a glow, 16 pixels per update, and
//! squeeze older time to the left (a history multiplier of 15).

use std::collections::VecDeque;

use egui::{vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui};

use crate::app::{PerfItem, TaskApp};
use crate::core_page::{CorePage, PageCtx, PerfHistory};
use crate::platform::Action;
use crate::settings::Settings;
use crate::perfdata::{self, StaticInfo};
use crate::sampler::Snapshot;
use crate::theme::{self, Mode, Theme};
use crate::widgets::{cpu_brand, fmt_uptime, local_ip, refresh_button};

/// Samples kept per series: the 57 samples a 920-pixel graph shows at
/// 16 pixels each, times a history multiplier of 15.
pub const PERF_HISTORY: usize = 900;
/// Pixels per update at the newest end of a full-size graph.
const PPU: f32 = 16.0;

static EMPTY: VecDeque<f32> = VecDeque::new();

/// The wide display face (Michroma) used for titles and device names.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("display".into()))
}

fn text(size: f32) -> FontId {
    FontId::proportional(size)
}

// ------------------------------------------------------------- formatting

/// "22.4 GB", "318.8 MB", "384.0 KB", "512 B".
fn fmt_size(b: u64) -> String {
    let mut v = b as f64;
    if v < 1024.0 {
        return format!("{v:.0} B");
    }
    let mut unit = "B";
    for u in ["KB", "MB", "GB", "TB", "PB"] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    format!("{v:.1} {unit}")
}

/// Three significant digits: "9.93", "13.1", "193".
fn sig3(v: f64) -> String {
    if v >= 100.0 {
        format!("{v:.0}")
    } else if v >= 10.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

/// "997 B/sec", "1.21 KB/sec", "13.1 MB/sec".
pub(crate) fn fmt_bytes_rate(bps: f64) -> String {
    let mut v = bps.max(0.0);
    if v < 1024.0 {
        return format!("{v:.0} B/sec");
    }
    let mut unit = "B";
    for u in ["KB", "MB", "GB", "TB"] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    format!("{} {unit}/sec", sig3(v))
}

/// Bytes per second shown as bits: "9.93 Kbps", "1.20 Gbps".
pub(crate) fn fmt_bit_rate(bytes_per_sec: f64) -> String {
    let mut v = bytes_per_sec.max(0.0) * 8.0;
    if v < 1000.0 {
        return format!("{v:.0} bps");
    }
    let mut unit = "";
    for u in ["K", "M", "G", "T"] {
        if v < 1000.0 {
            break;
        }
        v /= 1000.0;
        unit = u;
    }
    format!("{} {unit}bps", sig3(v))
}

fn pct1(v: f32) -> String {
    format!("{v:.1}%")
}

fn letters(d: &perfdata::DiskInfo) -> String {
    if d.letters.is_empty() {
        format!("Disk {}", d.number)
    } else {
        let l: Vec<String> = d.letters.iter().map(|c| format!("{c}:")).collect();
        format!("Disk {} ({})", d.number, l.join(", "))
    }
}

// ---------------------------------------------------------------- devices

#[derive(Clone, Copy)]
enum Icon {
    Cpu,
    Mem,
    Disk,
    Net,
    Gpu,
    Power,
}

/// One entry of the device list (Summary's tiles are mapped from it).
pub(crate) struct Dev {
    pub(crate) item: PerfItem,
    pub(crate) title: String,
    pub(crate) line1: String,
    pub(crate) line2: String,
    /// History key of the line in the small graph.
    key: String,
    max: f32,
    color: Color32,
    icon: Icon,
}

/// Figures under the graphs: a grid of label-over-value cells, then a
/// two-column "key: value" table (pairs in reading order).
#[derive(Default)]
pub(crate) struct Stats {
    pub(crate) top: Vec<(String, String)>,
    pub(crate) table: Vec<(String, String)>,
}

impl Stats {
    fn top(&mut self, k: &str, v: impl Into<String>) {
        self.top.push((k.into(), v.into()));
    }
    fn row(&mut self, k: &str, v: impl Into<String>) {
        self.table.push((k.into(), v.into()));
    }
    fn height(&self) -> f32 {
        let rows = self.top.len().div_ceil(4) as f32;
        let table = self.table.len().div_ceil(2) as f32;
        rows * 34.0 + if table > 0.0 { 8.0 + table * 20.0 } else { 0.0 }
    }
}

impl TaskApp {
    /// Appends the newest sample of every Performance series.
    pub fn push_perf(&mut self, s: &Snapshot) {
        let ph = &mut self.ph;
        let mut put = |k: String, v: f32| {
            let h = ph.entry(k).or_default();
            h.push_back(if v.is_finite() { v } else { 0.0 });
            while h.len() > PERF_HISTORY {
                h.pop_front();
            }
        };
        put("cpu".into(), s.cpu_total);
        put("cpu.k".into(), s.perf.kernel_total.min(s.cpu_total));
        for (i, v) in s.cpu_per_core.iter().enumerate() {
            put(format!("core{i}"), *v);
            put(format!("core{i}.k"), s.perf.kernel_per_core.get(i).copied().unwrap_or(0.0).min(*v));
        }
        let mem = if s.mem_total > 0 { s.mem_used as f32 * 100.0 / s.mem_total as f32 } else { 0.0 };
        put("mem".into(), mem);
        put("mem.hf".into(), s.perf.hard_faults);
        for d in &s.perf.disks {
            put(format!("disk{}.act", d.number), d.active);
            put(format!("disk{}.r", d.number), d.read_bps as f32);
            put(format!("disk{}.w", d.number), d.write_bps as f32);
        }
        let disks = crate::metrics::physical_disk_bps(&s.perf.disks).map_or(0.0, |(r, w)| r + w);
        put(crate::metrics::DISKS_TOTAL.into(), disks as f32);
        let adapters = crate::metrics::combine_adapters(&s.net);
        put("net.r".into(), adapters.rx_bps as f32);
        put("net.s".into(), adapters.tx_bps as f32);
        for g in &s.perf.gpus {
            put(format!("gpu{}", g.luid), g.util);
            for (n, v) in &g.engines {
                put(format!("gpu{}.e.{n}", g.luid), *v);
            }
            put(format!("gpu{}.ded", g.luid), g.dedicated as f32);
            put(format!("gpu{}.sh", g.luid), g.shared as f32);
        }
        if let Some(w) = s.perf.cpu_power_w {
            put("power".into(), w);
        }
        // A figure whose counter failed is a gap (NaN) in its series, not 0.
        let gap = |ph: &mut std::collections::BTreeMap<String, VecDeque<f32>>, key: &str| {
            if let Some(last) = ph.get_mut(key).and_then(|h| h.back_mut()) {
                *last = f32::NAN;
            }
        };
        if !s.perf.answered(perfdata::counter::KERNEL) {
            gap(&mut self.ph, "cpu.k");
        }
        if !(s.perf.answered(perfdata::counter::DREAD) && s.perf.answered(perfdata::counter::DWRITE)) {
            gap(&mut self.ph, crate::metrics::DISKS_TOTAL);
        }
    }
}

/// A choice Performance asks the app to save.
#[derive(Clone, PartialEq, Debug)]
pub enum PerfSetting {
    /// The selected device, by its key (see [`PerfItem::key`]).
    Device(String),
    /// Graphs per logical processor (true) or overall (false).
    Logical(bool),
    /// Kernel times drawn in the CPU graphs.
    Kernel(bool),
    /// GPU graphs by engine (true) or overall (false).
    GpuEngines(bool),
}

impl PerfSetting {
    /// Applies the choice to `settings` (the app saves them after).
    pub fn apply(self, settings: &mut Settings) {
        match self {
            PerfSetting::Device(key) => settings.perf_device = key,
            PerfSetting::Logical(on) => settings.perf_logical = on,
            PerfSetting::Kernel(on) => settings.perf_kernel = on,
            PerfSetting::GpuEngines(on) => settings.perf_gpu_engines = on,
        }
    }
}

/// The Performance page. Its own state is the device chosen; the series it
/// draws are the app's Performance history (shared with Summary).
#[derive(Default)]
pub struct PerformancePage {
    /// Chosen since the app started; until then the saved choice
    /// (`Settings::perf_device`) stands.
    chosen: Option<PerfItem>,
}

impl PerformancePage {
    fn selected(&self, settings: &Settings) -> PerfItem {
        self.chosen.unwrap_or_else(|| PerfItem::from_key(&settings.perf_device))
    }

    fn frame<'a>(&self, ctx: &'a PageCtx) -> Frame<'a> {
        Frame { ph: ctx.histories.perf, settings: ctx.settings, snap: ctx.snapshot, machine: machine_of(ctx.snapshot, None), perf: self.selected(ctx.settings) }
    }

    /// The "Performance" menu added to the menu bar on this page.
    pub fn menu(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        let frame = self.frame(ctx);
        ui.menu_button("Performance", |ui| {
            ui.menu_button("Change graph to", |ui| {
                let logical = ctx.settings.perf_logical;
                if ui.button(format!("{}Overall utilization", if logical { "   " } else { "\u{2022} " })).clicked() {
                    actions.push(Action::PerfSetting(PerfSetting::Logical(false)));
                    ui.close();
                }
                if ui.button(format!("{}Logical processors", if logical { "\u{2022} " } else { "   " })).clicked() {
                    actions.push(Action::PerfSetting(PerfSetting::Logical(true)));
                    ui.close();
                }
            });
            let mut k = ctx.settings.perf_kernel;
            if ui.checkbox(&mut k, "Show kernel times").changed() {
                actions.push(Action::PerfSetting(PerfSetting::Kernel(k)));
            }
            ui.separator();
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(frame.copy_text());
                actions.push(Action::Say("Copied".into()));
                ui.close();
            }
        });
    }
}

impl CorePage for PerformancePage {
    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(26.0, 40.0), Sense::hover());
            if !crate::widgets::icons::draw(ui.painter(), r.center(), crate::addons::Core::Performance.id()) {
                crate::widgets::plain_icon(ui.painter(), r.center(), t.label);
            }
            ui.label(egui::RichText::new("Performance").font(t.fonts.page_title.clone()).color(t.text));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if refresh_button(ui).clicked() {
                    actions.push(Action::Refresh);
                }
            });
        });
        ui.add_space(4.0);
        let before = actions.len();
        self.frame(ctx).page(ui, actions);
        // A click on a tile chooses the device at once, not after the app
        // has saved it.
        for a in &actions[before..] {
            if let Action::PerfSetting(PerfSetting::Device(key)) = a {
                self.chosen = Some(PerfItem::from_key(key));
            }
        }
    }
}

/// What the page reads while it draws: the sample, the settings, the
/// machine's facts and the Performance history, with the device chosen.
struct Frame<'a> {
    ph: &'a PerfHistory,
    settings: &'a Settings,
    snap: &'a Snapshot,
    machine: &'a StaticInfo,
    perf: PerfItem,
}

/// The machine's facts: `fixed` when set (tests), else the sample's, else
/// none known.
pub(crate) fn machine_of<'a>(snap: &'a Snapshot, fixed: Option<&'a StaticInfo>) -> &'a StaticInfo {
    static NONE: std::sync::OnceLock<StaticInfo> = std::sync::OnceLock::new();
    fixed.or(snap.machine.as_deref()).unwrap_or_else(|| NONE.get_or_init(StaticInfo::default))
}

/// The device list Performance shows and Summary's tiles are mapped from
/// (one source, so the two pages never disagree).
pub(crate) fn devices(settings: &Settings, snap: &Snapshot, machine: &StaticInfo, ph: &PerfHistory) -> Vec<Dev> {
    Frame { ph, settings, snap, machine, perf: PerfItem::Cpu }.devices()
}

impl TaskApp {
    /// The figures under the graphs of `item`, from the latest sample.
    pub(crate) fn stats(&self, item: PerfItem) -> Stats {
        Frame { ph: &self.ph, settings: &self.settings, snap: &self.snap, machine: self.machine(), perf: item }.stats(item)
    }
}

impl<'a> Frame<'a> {
    fn machine(&self) -> &StaticInfo {
        self.machine
    }

    /// The CPU's base frequency in MHz, from the machine's fixed facts
    /// (0 when unknown).
    fn base_mhz(&self) -> Option<f64> {
        crate::metrics::base_clock_mhz(Some(self.machine.cpu.base_mhz as f64), None, None)
    }

    /// The graph palette with the user's saved colours.
    fn graphs(&self) -> theme::GraphPalette {
        self.settings.theme().graphs
    }

    fn hist(&self, k: &str) -> &VecDeque<f32> {
        self.ph.get(k).unwrap_or(&EMPTY)
    }

    fn rate(&self, bps: f64) -> String {
        if self.settings.bits_per_second { fmt_bit_rate(bps) } else { fmt_bytes_rate(bps) }
    }

    fn net_interfaces(&self) -> usize {
        crate::metrics::combine_adapters(&self.snap.net).count.max(1)
    }

    /// The device list Performance shows and Summary's tiles are mapped
    /// from (one source, so the two pages never disagree).
    pub(crate) fn devices(&self) -> Vec<Dev> {
        let s = &self.snap;
        let info = self.machine();
        let mut v = vec![
            Dev {
                item: PerfItem::Cpu,
                title: "CPU".into(),
                line1: pct1(s.cpu_total),
                line2: format!("{} logical processors", s.cpu_per_core.len()),
                key: "cpu".into(),
                max: 100.0,
                color: self.graphs().cpu,
                icon: Icon::Cpu,
            },
            Dev {
                item: PerfItem::Memory,
                title: "Memory".into(),
                line1: format!("{} / {}", fmt_size(s.mem_used), fmt_size(s.mem_total)),
                line2: pct1(if s.mem_total > 0 { s.mem_used as f32 * 100.0 / s.mem_total as f32 } else { 0.0 }),
                key: "mem".into(),
                max: 100.0,
                color: self.graphs().memory,
                icon: Icon::Mem,
            },
        ];
        let mut numbers: Vec<u32> = info.disks.iter().map(|d| d.number).collect();
        for d in &s.perf.disks {
            if !numbers.contains(&d.number) {
                numbers.push(d.number);
            }
        }
        numbers.sort_unstable();
        for n in numbers {
            let live = s.perf.disks.iter().find(|d| d.number == n);
            let fixed = info.disks.iter().find(|d| d.number == n);
            v.push(Dev {
                item: PerfItem::Disk(n),
                title: fixed.map(|d| d.model.clone()).unwrap_or_else(|| format!("Disk {n}")),
                line1: fixed.map(letters).unwrap_or_else(|| format!("Disk {n}")),
                line2: pct1(live.map(|d| d.active).unwrap_or(0.0)),
                key: format!("disk{n}.act"),
                max: 100.0,
                color: self.graphs().disk,
                icon: Icon::Disk,
            });
        }
        let k = self.net_interfaces();
        let crate::metrics::Adapters { rx_bps: rx, tx_bps: tx, .. } = crate::metrics::combine_adapters(&s.net);
        v.push(Dev {
            item: PerfItem::Network,
            title: "Network".into(),
            line1: format!("{k} network interface{}", if k == 1 { "" } else { "s" }),
            line2: format!("S: {}  R: {}", self.rate(tx), self.rate(rx)),
            key: "net.r".into(),
            max: auto_max(self.hist("net.r"), 1024.0),
            color: self.graphs().network,
            icon: Icon::Net,
        });
        for g in &info.gpus {
            let util = s.perf.gpus.iter().find(|l| l.luid == g.luid).map(|l| l.util).unwrap_or(0.0);
            v.push(Dev {
                item: PerfItem::Gpu(g.luid),
                title: g.name.clone(),
                line1: "GPU adapter".into(),
                line2: pct1(util),
                key: format!("gpu{}", g.luid),
                max: 100.0,
                color: self.graphs().gpu,
                icon: Icon::Gpu,
            });
        }
        if s.perf.cpu_power_w.is_some() || !self.hist("power").is_empty() {
            v.push(Dev {
                item: PerfItem::Power,
                title: "CPU Power".into(),
                line1: "CPU package".into(),
                line2: format!("{:.1} W", s.perf.cpu_power_w.unwrap_or(0.0)),
                key: "power".into(),
                max: auto_max(self.hist("power"), 10.0),
                color: self.graphs().power,
                icon: Icon::Power,
            });
        }
        v
    }

    /// Everything shown for the selected device, as plain text.
    fn copy_text(&self) -> String {
        let devs = self.devices();
        let Some(d) = devs.iter().find(|d| d.item == self.perf).or(devs.first()) else { return String::new() };
        let (title, sub) = self.detail_titles(d);
        let st = self.stats(d.item);
        let mut out = format!("{title}\n{sub}\n\n");
        for (k, v) in st.top.iter().chain(st.table.iter()) {
            out.push_str(&format!("{k}: {v}\n"));
        }
        out
    }

    /// Draws the device list and the chosen device below the header.
    fn page(&self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let devs = self.devices();
        // A device can be missing for a moment (CPU Power before the first
        // sample): show the CPU meanwhile but keep the saved choice.
        let shown = if devs.iter().any(|d| d.item == self.perf) { self.perf } else { PerfItem::Cpu };
        let full = ui.available_rect_before_wrap();
        ui.allocate_rect(full, Sense::hover());

        // Device list on a darker band.
        // The list sits right against the sidebar.
        let list = Rect::from_min_max(Pos2::new(full.left() - 21.0, full.top()), Pos2::new(full.left() + 215.0, full.bottom()));
        let t = theme::of_ui(ui);
        ui.painter().rect_filled(Rect::from_min_max(list.min, list.max + vec2(0.0, 8.0)), CornerRadius::ZERO, t.panel);
        for (i, d) in devs.iter().enumerate() {
            let r = Rect::from_min_size(list.min + vec2(2.0, 2.0 + i as f32 * 64.0), vec2(list.width() - 4.0, 56.0));
            if r.top() > list.bottom() {
                break;
            }
            let resp = ui.interact(r, ui.id().with(("perf-device", i)), Sense::click());
            if resp.clicked() {
                actions.push(Action::PerfSetting(PerfSetting::Device(d.item.key())));
            }
            self.paint_tile(ui.painter(), r, d, d.item == shown, resp.hovered());
        }

        let detail = Rect::from_min_max(Pos2::new(list.right() + 30.0, full.top()), Pos2::new(full.right() - 4.0, full.bottom()));
        if let Some(d) = devs.iter().find(|d| d.item == shown) {
            self.paint_detail(ui, detail, d, actions);
        }
    }

    fn paint_tile(&self, p: &Painter, r: Rect, d: &Dev, selected: bool, hovered: bool) {
        let t = theme::current(p.ctx());
        if selected {
            p.rect_filled(r, CornerRadius::same(4), t.selected);
        } else if hovered {
            p.rect_filled(r, CornerRadius::same(4), t.hover);
        }
        paint_icon(p, Pos2::new(r.left() + 24.0, r.top() + 28.0), d.icon);
        let g = Rect::from_min_size(Pos2::new(r.right() - 82.0, r.top() + 6.0), vec2(75.0, 44.0));
        let x = r.left() + 50.0;
        let w = g.left() - 8.0 - x;
        p.text(Pos2::new(x, r.top() + 5.0), Align2::LEFT_TOP, fit(p, &d.title, &display(13.0), w), display(13.0), t.text);
        p.text(Pos2::new(x, r.top() + 24.0), Align2::LEFT_TOP, fit(p, &d.line1, &text(11.5), w), text(11.5), t.soft);
        p.text(Pos2::new(x, r.top() + 37.0), Align2::LEFT_TOP, fit(p, &d.line2, &text(11.5), w), text(11.5), t.soft);
        let opts = GraphOpts {
            frame: self.graphs().frame,
            bg: if selected { self.graphs().selected } else { self.graphs().cells },
            grid: false,
            ppu: 5.0,
        };
        plot(p, g, &[(self.hist(&d.key), d.color)], d.max, &opts);
    }

    /// Big title and the line under the LED meter.
    fn detail_titles(&self, d: &Dev) -> (String, String) {
        let s = &self.snap;
        let info = self.machine();
        match d.item {
            PerfItem::Cpu => {
                let b = cpu_brand();
                (if b.is_empty() { "CPU".into() } else { b }, format!("{} logical processors", s.cpu_per_core.len()))
            }
            PerfItem::Memory => (
                "Memory".into(),
                format!("{}{}", fmt_size(s.mem_total), info.mem.kind.map(|k| format!(" {k}")).unwrap_or_default()),
            ),
            PerfItem::Disk(_) => (d.title.clone(), d.line1.clone()),
            PerfItem::Network => {
                let k = self.net_interfaces();
                ("Network".into(), format!("{k} network interface{} combined", if k == 1 { "" } else { "s" }))
            }
            PerfItem::Gpu(_) => (d.title.clone(), "GPU adapter".into()),
            PerfItem::Power => ("CPU Power".into(), "CPU package".into()),
        }
    }

    fn paint_detail(&self, ui: &mut Ui, area: Rect, d: &Dev, actions: &mut Vec<Action>) {
        let p = ui.painter().clone();
        let t = theme::of_ui(ui);
        let (title, sub) = self.detail_titles(d);
        p.text(area.left_top() + vec2(-2.0, 10.0), Align2::LEFT_TOP, fit(&p, &title, &display(26.0), area.width() - 8.0), display(26.0), t.text);

        // LED meter with the current value at its right.
        let s = &self.snap;
        let (frac, value, value_color): (f32, String, Color32) = match d.item {
            PerfItem::Cpu => (s.cpu_total / 100.0, pct1(s.cpu_total), t.link),
            PerfItem::Memory => {
                let m = if s.mem_total > 0 { s.mem_used as f32 / s.mem_total as f32 } else { 0.0 };
                (m, pct1(m * 100.0), t.link)
            }
            PerfItem::Disk(n) => {
                let a = s.perf.disks.iter().find(|x| x.number == n).map(|x| x.active).unwrap_or(0.0);
                (a / 100.0, pct1(a), self.graphs().disk)
            }
            PerfItem::Network => {
                let r = self.hist("net.r").back().copied().unwrap_or(0.0) + self.hist("net.s").back().copied().unwrap_or(0.0);
                let top = auto_max(self.hist("net.r"), 1024.0) + auto_max(self.hist("net.s"), 1024.0);
                (r / top, self.rate(r as f64), t.link)
            }
            PerfItem::Gpu(l) => {
                let u = s.perf.gpus.iter().find(|g| g.luid == l).map(|g| g.util).unwrap_or(0.0);
                (u / 100.0, pct1(u), t.link)
            }
            PerfItem::Power => {
                let w = s.perf.cpu_power_w.unwrap_or(0.0);
                (w / d.max.max(1.0), format!("{w:.1} W"), self.graphs().power)
            }
        };
        let led_rect = Rect::from_min_size(area.left_top() + vec2(0.0, 46.0), vec2((area.width() - 130.0).max(80.0), 18.0));
        let palette: fn(f32) -> Color32 = match d.item {
            PerfItem::Cpu => cpu_gradient,
            PerfItem::Memory => |_| graph_palette().memory,
            PerfItem::Disk(_) => |_| graph_palette().disk,
            PerfItem::Power => |_| graph_palette().power,
            _ => |_| graph_palette().network,
        };
        led_meter(&p, led_rect, frac, palette);
        p.text(Pos2::new(area.right(), led_rect.center().y), Align2::RIGHT_CENTER, value, text(21.0), value_color);
        p.text(area.left_top() + vec2(0.0, 69.0), Align2::LEFT_TOP, sub, text(12.0), t.label);

        let stats = self.stats(d.item);
        let label_y = area.top() + 92.0;
        let graphs = Rect::from_min_max(
            Pos2::new(area.left(), label_y + 16.0),
            Pos2::new(area.right(), (area.bottom() - stats.height() - 26.0).max(label_y + 140.0)),
        );
        let label = match d.item {
            PerfItem::Cpu if self.settings.perf_logical => "% Utilization by logical processor",
            PerfItem::Cpu => "% Utilization",
            PerfItem::Memory => "Memory utilization",
            PerfItem::Disk(_) => "% Active time",
            PerfItem::Network => "Throughput",
            PerfItem::Gpu(_) if self.settings.perf_gpu_engines => "% Utilization by engine",
            PerfItem::Gpu(_) => "% Utilization",
            PerfItem::Power => "CPU package power",
        };
        p.text(Pos2::new(area.left() + 2.0, label_y), Align2::LEFT_TOP, label, text(12.0), t.label);
        match d.item {
            PerfItem::Cpu => self.cpu_graph_menu(ui, Pos2::new(area.right(), label_y), actions),
            PerfItem::Gpu(_) => self.gpu_toggle(ui, Pos2::new(area.right(), label_y - 4.0), actions),
            _ => {}
        }

        match d.item {
            PerfItem::Cpu => self.cpu_graphs(&p, graphs),
            PerfItem::Memory => {
                let (a, b) = split(graphs, 0.58, 30.0);
                plot(&p, a, &[(self.hist("mem"), self.graphs().memory)], 100.0, &big(self.graphs().memory));
                p.text(Pos2::new(b.left() + 2.0, b.top() - 16.0), Align2::LEFT_TOP, "Hard faults, pages read from disk (file-backed paging included)", text(12.0), t.label);
                plot(&p, b, &[(self.hist("mem.hf"), self.graphs().memory)], auto_max(self.hist("mem.hf"), 10.0), &big(self.graphs().memory));
            }
            PerfItem::Disk(n) => {
                let (a, b) = split(graphs, 0.58, 30.0);
                plot(&p, a, &[(self.hist(&format!("disk{n}.act")), self.graphs().disk)], 100.0, &big(self.graphs().disk));
                p.text(Pos2::new(b.left() + 2.0, b.top() - 16.0), Align2::LEFT_TOP, "Disk transfer rate", text(12.0), t.label);
                let (r, w) = (self.hist(&format!("disk{n}.r")), self.hist(&format!("disk{n}.w")));
                let max = auto_max(r, 64.0 * 1024.0).max(auto_max(w, 64.0 * 1024.0));
                plot(&p, b, &[(w, self.graphs().disk_write), (r, self.graphs().disk)], max, &big(self.graphs().disk));
            }
            PerfItem::Network => {
                let (r, s) = (self.hist("net.r"), self.hist("net.s"));
                let max = auto_max(r, 1024.0).max(auto_max(s, 1024.0));
                plot(&p, graphs, &[(s, self.graphs().network_sent), (r, self.graphs().network)], max, &big(self.graphs().network));
            }
            PerfItem::Gpu(l) => self.gpu_graphs(&p, graphs, l),
            PerfItem::Power => {
                plot(&p, graphs, &[(self.hist("power"), self.graphs().power)], d.max, &big(self.graphs().power));
            }
        }

        paint_stats(&p, Rect::from_min_max(Pos2::new(area.left() + 2.0, graphs.bottom() + 26.0), area.max), &stats);
    }

    /// "CPUs Expanded ⌄": choose overall or per-processor graphs.
    fn cpu_graph_menu(&self, ui: &mut Ui, right_top: Pos2, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        let label = if self.settings.perf_logical { "CPUs Expanded" } else { "CPU Overall" };
        let galley = ui.painter().layout_no_wrap(label.into(), text(12.0), t.link);
        let r = Rect::from_min_size(Pos2::new(right_top.x - galley.size().x - 18.0, right_top.y - 1.0), vec2(galley.size().x + 16.0, galley.size().y + 2.0));
        let resp = ui.interact(r, ui.id().with("cpu-graph-mode"), Sense::click());
        let col = if resp.hovered() { t.link_hover } else { t.link };
        ui.painter().text(r.left_top() + vec2(0.0, 1.0), Align2::LEFT_TOP, label, text(12.0), col);
        let c = Pos2::new(r.right() - 5.0, r.center().y + 1.0);
        // In the link's colour, like the words before it.
        crate::widgets::icons::draw_glyph(ui.painter(), c, 9.0, "chevron-down", crate::widgets::icons::Ink::On(col));
        egui::Popup::menu(&resp).show(|ui| {
            let logical = self.settings.perf_logical;
            if ui.button(format!("{}Overall utilization", if logical { "   " } else { "\u{2022} " })).clicked() {
                actions.push(Action::PerfSetting(PerfSetting::Logical(false)));
            }
            if ui.button(format!("{}Logical processors", if logical { "\u{2022} " } else { "   " })).clicked() {
                actions.push(Action::PerfSetting(PerfSetting::Logical(true)));
            }
            ui.separator();
            let mut k = self.settings.perf_kernel;
            if ui.checkbox(&mut k, "Show kernel times").changed() {
                actions.push(Action::PerfSetting(PerfSetting::Kernel(k)));
            }
        });
    }

    /// The "Overall | Engines" switch for GPUs.
    fn gpu_toggle(&self, ui: &mut Ui, right_top: Pos2, actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        let w = 69.0;
        let whole = Rect::from_min_size(Pos2::new(right_top.x - 2.0 * w, right_top.y), vec2(2.0 * w, 20.0));
        for (i, (label, engines)) in [("Overall", false), ("Engines", true)].into_iter().enumerate() {
            let r = Rect::from_min_size(whole.min + vec2(i as f32 * w, 0.0), vec2(w, 20.0));
            let resp = ui.interact(r, ui.id().with(("gpu-mode", i)), Sense::click());
            let on = self.settings.perf_gpu_engines == engines;
            if on {
                ui.painter().rect_filled(r, CornerRadius::ZERO, t.primary);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, CornerRadius::ZERO, t.hover);
            }
            ui.painter().text(r.center(), Align2::CENTER_CENTER, label, text(12.0), if on { t.on_primary } else { t.link });
            if resp.clicked() && !on {
                actions.push(Action::PerfSetting(PerfSetting::GpuEngines(engines)));
            }
        }
        ui.painter().rect_stroke(whole, CornerRadius::ZERO, Stroke::new(1.0, self.graphs().gpu), StrokeKind::Inside);
    }

    fn cpu_graphs(&self, p: &Painter, area: Rect) {
        let kernel = self.settings.perf_kernel;
        if !self.settings.perf_logical {
            let mut series = vec![(self.hist("cpu"), self.graphs().utilisation)];
            if kernel {
                series.insert(0, (self.hist("cpu.k"), self.graphs().kernel));
            }
            plot(p, area, &series, 100.0, &big(self.graphs().cpu));
            return;
        }
        let n = self.snap.cpu_per_core.len().max(1);
        let cols = match n {
            1..=4 => n,
            5..=8 => 4,
            9..=15 => 5,
            16..=24 => 6,
            25..=32 => 8,
            33..=64 => 10,
            _ => 16,
        };
        let rows = n.div_ceil(cols);
        let gap = 5.0;
        let cw = (area.width() - gap * (cols - 1) as f32) / cols as f32;
        let ch = ((area.height() - gap * (rows - 1) as f32) / rows as f32).max(30.0);
        p.rect_filled(area, CornerRadius::ZERO, self.graphs().cells);
        let ppu = (cw / 16.0).clamp(3.0, PPU);
        let on_graph = graph_label();
        for i in 0..n {
            let r = Rect::from_min_size(area.min + vec2((i % cols) as f32 * (cw + gap), (i / cols) as f32 * (ch + gap)), vec2(cw, ch));
            let u = self.hist(&format!("core{i}"));
            let mut series = vec![(u, self.graphs().utilisation)];
            if kernel {
                series.insert(0, (self.hist(&format!("core{i}.k")), self.graphs().kernel));
            }
            let opts = GraphOpts { frame: self.graphs().cpu, bg: tint(self.graphs().utilisation), grid: true, ppu };
            plot(p, r, &series, 100.0, &opts);
            p.text(r.left_top() + vec2(5.0, 3.0), Align2::LEFT_TOP, format!("CPU {i}"), text(11.0), on_graph);
        }
    }

    fn gpu_graphs(&self, p: &Painter, area: Rect, luid: u64) {
        let info = self.machine().gpus.iter().find(|g| g.luid == luid);
        let live = self.snap.perf.gpus.iter().find(|g| g.luid == luid);
        let (top, mem) = split(area, 0.66, 6.0);
        if self.settings.perf_gpu_engines {
            let engines: Vec<(String, f32)> = live.map(|g| g.engines.clone()).unwrap_or_default();
            let n = engines.len().max(1);
            let cols = 3usize.min(n);
            let rows = n.div_ceil(cols);
            let gap = 5.0;
            let cw = (top.width() - gap * (cols - 1) as f32) / cols as f32;
            let ch = (top.height() - gap * (rows - 1) as f32) / rows as f32;
            for (i, (name, v)) in engines.iter().enumerate() {
                let r = Rect::from_min_size(top.min + vec2((i % cols) as f32 * (cw + gap), (i / cols) as f32 * (ch + gap)), vec2(cw, ch));
                let opts = GraphOpts { frame: self.graphs().gpu, bg: tint(self.graphs().gpu), grid: true, ppu: (cw / 16.0).clamp(3.0, PPU) };
                plot(p, r, &[(self.hist(&format!("gpu{luid}.e.{name}")), self.graphs().gpu)], 100.0, &opts);
                cell_title(p, r, name, &pct1(*v));
            }
        } else {
            plot(p, top, &[(self.hist(&format!("gpu{luid}")), self.graphs().gpu)], 100.0, &big(self.graphs().gpu));
        }
        let half = (mem.width() - 6.0) / 2.0;
        let ded = Rect::from_min_size(mem.min, vec2(half, mem.height()));
        let sh = Rect::from_min_size(mem.min + vec2(half + 6.0, 0.0), vec2(half, mem.height()));
        let ded_total = info.map(|g| g.dedicated as f32).unwrap_or(0.0);
        let sh_total = info.map(|g| g.shared as f32).unwrap_or(0.0);
        let hd = self.hist(&format!("gpu{luid}.ded"));
        let hs = self.hist(&format!("gpu{luid}.sh"));
        let opts = GraphOpts { frame: self.graphs().gpu, bg: tint(self.graphs().gpu), grid: true, ppu: (half / 16.0).clamp(3.0, PPU) };
        plot(p, ded, &[(hd, self.graphs().gpu)], if ded_total > 0.0 { ded_total } else { auto_max(hd, 1.0) }, &opts);
        plot(p, sh, &[(hs, self.graphs().gpu)], if sh_total > 0.0 { sh_total } else { auto_max(hs, 1.0) }, &opts);
        cell_title(p, ded, "Dedicated GPU memory", &fmt_size(live.map(|g| g.dedicated).unwrap_or(0)));
        cell_title(p, sh, "Shared GPU memory", &fmt_size(live.map(|g| g.shared).unwrap_or(0)));
    }

    pub(crate) fn stats(&self, item: PerfItem) -> Stats {
        let s = &self.snap;
        let info = self.machine();
        let pf = &s.perf;
        let mut st = Stats::default();
        let dash = "\u{2014}";
        match item {
            PerfItem::Cpu => {
                let c = &info.cpu;
                // The shared rule; blank when either input is unknown.
                let speed = crate::metrics::cpu_speed_mhz(self.base_mhz(), Some(pf.cpu_perf_pct as f64));
                st.top("Utilization", pct1(s.cpu_total));
                st.top("Speed", speed.map_or_else(|| dash.into(), |mhz| crate::metrics::fmt_ghz(mhz as f32)));
                st.top("Processes", s.procs.len().to_string());
                st.top("Threads", pf.threads.to_string());
                st.top("Up time", fmt_uptime(s.uptime));
                let yes_no = |b: bool| if b { "Yes" } else { "No" };
                st.row("Fast Startup:", c.fast_startup.map(yes_no).unwrap_or(dash));
                st.row("Base speed:", if c.base_mhz > 0 { format!("{:.2} GHz", c.base_mhz as f32 / 1000.0) } else { dash.into() });
                st.row("Sockets:", c.sockets.max(1).to_string());
                st.row("Physical cores:", c.cores.to_string());
                st.row("Logical processors:", s.cpu_per_core.len().to_string());
                st.row("Virtualization:", c.virtualization.map(|v| if v { "Enabled" } else { "Disabled" }).unwrap_or(dash));
                st.row("Virtual machine:", yes_no(c.vm));
                st.row("L1 cache:", fmt_size(c.l1));
                st.row("L2 cache:", fmt_size(c.l2));
                st.row("L3 cache:", fmt_size(c.l3));
                st.row("Frequency driver:", "Windows processor power management");
                st.row("Frequency governor:", if c.plan.is_empty() { dash.to_string() } else { c.plan.clone() });
                st.row("Power preference:", if c.overlay.is_empty() { dash.to_string() } else { c.overlay.clone() });
                st.row("Handles:", pf.handles.to_string());
            }
            PerfItem::Memory => {
                let m = &info.mem;
                st.top("In use", fmt_size(s.mem_used));
                st.top("Available", fmt_size(s.mem_avail));
                st.top("Committed", fmt_size(pf.commit));
                st.top("Cached", fmt_size(pf.cached));
                st.top("Swap used", fmt_size(s.swap_used));
                st.top("Swap available", fmt_size(s.swap_total.saturating_sub(s.swap_used)));
                st.top("Speed", m.speed_mts.map(|v| format!("{v} MT/s")).unwrap_or_else(|| dash.into()));
                st.top("Slots used", if m.slots > 0 { format!("{} of {}", m.used_slots, m.slots) } else { dash.into() });
                st.top("Form factor", m.form.unwrap_or(dash));
                st.top("Type", m.kind.unwrap_or(dash));
                st.top("Compressed", fmt_size(s.compressed));
                st.top("Page ins", pf.page_ins.to_string());
                st.top("Hard faults", format!("{:.0} pages/s", pf.hard_faults));
            }
            PerfItem::Disk(n) => {
                let live = pf.disks.iter().find(|d| d.number == n).cloned().unwrap_or_default();
                let fixed = info.disks.iter().find(|d| d.number == n);
                st.top("Read speed", fmt_bytes_rate(live.read_bps));
                st.top("Write speed", fmt_bytes_rate(live.write_bps));
                st.top("Total read", fmt_size(live.total_read));
                st.top("Total written", fmt_size(live.total_written));
                st.top("Active time", pct1(live.active));
                st.top("Avg. response time", format!("{:.2} ms", live.resp_ms));
                st.top("Capacity", fixed.map(|d| fmt_size(d.capacity)).unwrap_or_else(|| dash.into()));
                st.top("Formatted", fixed.filter(|d| d.formatted > 0).map(|d| fmt_size(d.formatted)).unwrap_or_else(|| "Unavailable".into()));
                st.top("System disk", if fixed.map(|d| d.system).unwrap_or(false) { "Yes" } else { "No" });
                st.top("Type", fixed.map(|d| d.bus).unwrap_or(dash));
            }
            PerfItem::Network => {
                let crate::metrics::Adapters { rx_bps: rx, tx_bps: tx, .. } = crate::metrics::combine_adapters(&s.net);
                let multiple = self.net_interfaces() > 1;
                st.top("Receive", self.rate(rx));
                st.top("Send", self.rate(tx));
                st.top("Total received", fmt_size(s.net.iter().map(|n| n.total_rx).sum()));
                st.top("Total sent", fmt_size(s.net.iter().map(|n| n.total_tx).sum()));
                st.top("Interface name", "All interfaces");
                st.top("Connection type", "Combined");
                st.top("Hardware address", "N/A");
                st.top("IPv4 address", if multiple { "Multiple".to_string() } else { local_ip() });
                st.top("IPv6 address", if multiple { "Multiple" } else { dash });
                st.row("Receive (bit rate):", fmt_bit_rate(rx));
                st.row("Send (bit rate):", fmt_bit_rate(tx));
            }
            PerfItem::Gpu(l) => {
                let live = pf.gpus.iter().find(|g| g.luid == l).cloned().unwrap_or_default();
                let fixed = info.gpus.iter().find(|g| g.luid == l);
                let engine = |prefix: &str| {
                    live.engines
                        .iter()
                        .filter(|(n, _)| n.to_lowercase().starts_with(prefix))
                        .map(|(_, v)| *v)
                        .reduce(f32::max)
                        .map(pct1)
                        .unwrap_or_else(|| dash.into())
                };
                st.top("Utilization", pct1(live.util));
                st.top("Clock speed", dash);
                st.top("Power draw", dash);
                st.top(
                    "Dedicated GPU memory",
                    format!("{} / {}", fmt_size(live.dedicated), fixed.map(|g| fmt_size(g.dedicated)).unwrap_or_else(|| dash.into())),
                );
                st.row("Shared GPU memory:", fmt_size(live.shared));
                st.row("Video encode:", engine("video encode"));
                st.row("Video decode:", engine("video decode"));
                st.row("Temperature:", dash);
                st.row("Graphics API:", "Direct3D 12");
                st.row("PCI Express:", "Unavailable");
                st.row("Driver version:", fixed.map(|g| g.driver.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| dash.into()));
                st.row("Physical location:", fixed.map(|g| g.location.clone()).filter(|v| !v.is_empty()).unwrap_or_else(|| dash.into()));
            }
            PerfItem::Power => {
                let h = self.hist("power");
                let avg = if h.is_empty() { 0.0 } else { h.iter().sum::<f32>() / h.len() as f32 };
                let peak = h.iter().copied().fold(0.0f32, f32::max);
                st.top("Power draw", format!("{:.1} W", pf.cpu_power_w.unwrap_or(0.0)));
                st.top("Average", format!("{avg:.1} W"));
                st.top("Peak", format!("{peak:.1} W"));
                st.top("Source", "Windows energy meter");
            }
        }
        st
    }
}

// ---------------------------------------------------------------- drawing

struct GraphOpts {
    frame: Color32,
    bg: Color32,
    grid: bool,
    /// Pixels per update at the newest end.
    ppu: f32,
}

fn big(c: Color32) -> GraphOpts {
    GraphOpts { frame: c, bg: tint(c), grid: true, ppu: PPU }
}

/// The page background with a trace of the graph's colour.
fn tint(c: Color32) -> Color32 {
    let base = graph_palette().tint_base;
    let mix = |a: u8, b: u8| (a as f32 * 0.945 + b as f32 * 0.055) as u8;
    Color32::from_rgb(mix(base.r(), c.r()), mix(base.g(), c.g()), mix(base.b(), c.b()))
}

/// Words on a graph: the dark theme's label grey, as graphs stay dark.
fn graph_label() -> Color32 {
    Theme::of(Mode::Dark).label
}

/// The graph palette, the same in both modes.
fn graph_palette() -> theme::GraphPalette {
    Theme::of(Mode::Dark).graphs
}

fn auto_max(h: &VecDeque<f32>, floor: f32) -> f32 {
    (h.iter().copied().fold(0.0f32, f32::max) * 1.15).max(floor)
}

/// Two graphs one above the other; the first takes `share` of the height.
fn split(r: Rect, share: f32, gap: f32) -> (Rect, Rect) {
    let h = (r.height() - gap).max(0.0);
    let a = Rect::from_min_size(r.min, vec2(r.width(), h * share));
    let b = Rect::from_min_max(Pos2::new(r.left(), a.bottom() + gap), r.max);
    (a, b)
}

/// Distance from the right edge of a sample `age` updates old. The newest
/// samples get `ppu` pixels each; older ones are squeezed logarithmically so
/// the whole history fits ("compressed graph time").
fn time_map(width: f32, ppu: f32, n: f32) -> impl Fn(f32) -> f32 {
    let linear = width / ppu >= n;
    let mut c = 1.0f32;
    if !linear {
        let (mut lo, mut hi) = (1e-3f32, 1e6f32);
        for _ in 0..60 {
            let mid = (lo * hi).sqrt();
            let slope = width / (mid * (1.0 + n / mid).ln());
            if slope > ppu { lo = mid } else { hi = mid }
        }
        c = (lo * hi).sqrt();
    }
    let denom = (1.0 + n / c).ln();
    move |a: f32| if linear { a * ppu } else { width * (1.0 + a / c).ln() / denom }
}

fn catmull(p0: Pos2, p1: Pos2, p2: Pos2, p3: Pos2, t: f32) -> Pos2 {
    let t2 = t * t;
    let t3 = t2 * t;
    let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3);
    Pos2::new(f(p0.x, p1.x, p2.x, p3.x), f(p0.y, p1.y, p2.y, p3.y))
}

/// Line graph: optional grid, glowing smooth lines, a marker at the newest
/// point of each line and a frame. Series are drawn in order (last on top).
fn plot(p: &Painter, rect: Rect, series: &[(&VecDeque<f32>, Color32)], max: f32, o: &GraphOpts) {
    let t = theme::current(p.ctx());
    p.rect_filled(rect, CornerRadius::ZERO, o.bg);
    let inner = rect.shrink(1.0);
    let n = (PERF_HISTORY - 1) as f32;
    let map = time_map(inner.width(), o.ppu, n);
    if o.grid {
        // A line every 18 px down, every 36 px
        // across where time is not squeezed.
        let gc = t.graphs.grid;
        let mut y = inner.top() + 18.0;
        while y < inner.bottom() - 2.0 {
            let yy = y.round() + 0.5;
            p.line_segment([Pos2::new(inner.left(), yy), Pos2::new(inner.right(), yy)], Stroke::new(1.0, gc));
            y += 18.0;
        }
        let step = 36.0 / o.ppu;
        let mut a = step;
        let mut last_x = f32::MAX;
        while a <= n {
            let x = (inner.right() - map(a)).round() + 0.5;
            if x <= inner.left() {
                break;
            }
            if last_x - x >= 4.0 {
                p.line_segment([Pos2::new(x, inner.top()), Pos2::new(x, inner.bottom())], Stroke::new(1.0, gc));
                last_x = x;
            }
            a += step;
        }
    }
    let max = max.max(1e-6);
    let clip = p.with_clip_rect(inner);
    let mut markers = Vec::new();
    for (data, color) in series {
        if data.is_empty() {
            continue;
        }
        let y_of = |v: f32| inner.bottom() - 1.0 - (v / max).clamp(0.0, 1.0) * (inner.height() - 3.0);
        // Newest first, dropping points closer than a pixel to the last kept
        // one; a NaN point (no reading) breaks the line into runs.
        let mut runs: Vec<Vec<Pos2>> = vec![Vec::new()];
        for (age, v) in data.iter().rev().enumerate() {
            let x = inner.right() - map(age as f32);
            if v.is_nan() {
                if runs.last().is_some_and(|r| !r.is_empty()) {
                    runs.push(Vec::new());
                }
            } else {
                let pts = runs.last_mut().expect("a run");
                if let Some(last) = pts.last() {
                    if last.x - x < 1.0 && age + 1 < data.len() {
                        continue;
                    }
                }
                pts.push(Pos2::new(x, y_of(*v)));
            }
            if x < inner.left() {
                break;
            }
        }
        if let Some(newest) = data.back().filter(|v| !v.is_nan()) {
            markers.push((Pos2::new(inner.right(), y_of(*newest)), *color));
        }
        for mut pts in runs {
            if pts.len() < 2 {
                continue;
            }
            pts.reverse();
            let mut line = Vec::with_capacity(pts.len() * 3);
            for i in 0..pts.len() - 1 {
                let (p0, p1, p2, p3) = (pts[i.saturating_sub(1)], pts[i], pts[i + 1], pts[(i + 2).min(pts.len() - 1)]);
                let steps = (((p2.x - p1.x) / 3.0).ceil() as usize).clamp(1, 10);
                for k in 0..steps {
                    let q = catmull(p0, p1, p2, p3, k as f32 / steps as f32);
                    line.push(Pos2::new(q.x, q.y.clamp(inner.top(), inner.bottom() - 1.0)));
                }
            }
            line.push(*pts.last().unwrap());
            clip.add(Shape::line(line.clone(), Stroke::new(6.0, color.gamma_multiply(0.10))));
            clip.add(Shape::line(line.clone(), Stroke::new(3.2, color.gamma_multiply(0.28))));
            clip.add(Shape::line(line, Stroke::new(1.5, *color)));
        }
    }
    p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, o.frame), StrokeKind::Inside);
    for (pt, _) in markers {
        let x = inner.right();
        let y = pt.y.clamp(inner.top() + 4.0, inner.bottom() - 4.0);
        p.add(Shape::convex_polygon(
            vec![Pos2::new(x, y - 4.0), Pos2::new(x, y + 4.0), Pos2::new(x - 5.0, y)],
            t.graphs.marker,
            Stroke::NONE,
        ));
    }
}

/// Name at the top left and value at the top right, inside a graph.
fn cell_title(p: &Painter, r: Rect, name: &str, value: &str) {
    let label = graph_label();
    let vw = p.layout_no_wrap(value.into(), text(11.0), label).size().x;
    p.text(r.left_top() + vec2(5.0, 3.0), Align2::LEFT_TOP, fit(p, name, &text(11.0), r.width() - vw - 18.0), text(11.0), label);
    p.text(r.right_top() + vec2(-5.0, 3.0), Align2::RIGHT_TOP, value, text(11.0), label);
}

/// Green to yellow to red along the CPU meter.
fn cpu_gradient(t: f32) -> Color32 {
    let g = graph_palette();
    let rgb = |c: Color32| [c.r() as f32, c.g() as f32, c.b() as f32];
    let lerp = |a: [f32; 3], b: [f32; 3], k: f32| Color32::from_rgb((a[0] + (b[0] - a[0]) * k) as u8, (a[1] + (b[1] - a[1]) * k) as u8, (a[2] + (b[2] - a[2]) * k) as u8);
    let (g, y, r) = (rgb(g.meter_low), rgb(g.meter_mid), rgb(g.meter_high));
    if t < 0.5 { lerp(g, y, t * 2.0) } else { lerp(y, r, (t - 0.5) * 2.0) }
}

/// A row of LED segments; lit ones glow, the rest show a dim version.
fn led_meter(p: &Painter, rect: Rect, frac: f32, color: fn(f32) -> Color32) {
    let (seg, gap) = (9.0, 3.0);
    let n = (((rect.width() + gap) / (seg + gap)).floor() as usize).max(1);
    let frac = if frac.is_finite() { frac.clamp(0.0, 1.0) } else { 0.0 };
    let lit = (frac * n as f32).round() as usize;
    let lit = if frac > 0.0 { lit.max(1) } else { 0 };
    for i in 0..n {
        let c = color(i as f32 / (n - 1).max(1) as f32);
        let r = Rect::from_min_size(Pos2::new(rect.left() + i as f32 * (seg + gap), rect.top()), vec2(seg, rect.height()));
        if i < lit {
            p.rect_filled(r.expand(1.5), CornerRadius::same(3), c.gamma_multiply(0.25));
            p.rect_filled(r, CornerRadius::same(2), c);
        } else {
            let d = |v: u8| (v as f32 * 0.16) as u8;
            p.rect_filled(r, CornerRadius::same(2), Color32::from_rgb(d(c.r()), d(c.g()), d(c.b())));
        }
    }
}

fn paint_stats(p: &Painter, area: Rect, st: &Stats) {
    let t = theme::current(p.ctx());
    let col = area.width() / 4.0;
    let mut y = area.top();
    for (i, (k, v)) in st.top.iter().enumerate() {
        let x = area.left() + (i % 4) as f32 * col;
        let ry = y + (i / 4) as f32 * 34.0;
        p.text(Pos2::new(x, ry), Align2::LEFT_TOP, k, text(11.5), t.label);
        p.text(Pos2::new(x, ry + 15.0), Align2::LEFT_TOP, fit(p, v, &text(12.5), col - 8.0), text(12.5), t.text);
    }
    y += st.top.len().div_ceil(4) as f32 * 34.0 + 8.0;
    let half = area.width() / 2.0;
    for (i, (k, v)) in st.table.iter().enumerate() {
        let x = area.left() + (i % 2) as f32 * half;
        let ry = y + (i / 2) as f32 * 20.0;
        p.text(Pos2::new(x, ry), Align2::LEFT_TOP, k, text(11.5), t.label);
        p.text(Pos2::new(x + 126.0, ry), Align2::LEFT_TOP, fit(p, v, &text(11.5), half - 134.0), text(11.5), t.text);
    }
}

/// Shortens `s` with an ellipsis until it fits `w` pixels.
pub fn fit(p: &Painter, s: &str, font: &FontId, w: f32) -> String {
    let width = |t: &str| p.layout_no_wrap(t.to_string(), font.clone(), Color32::PLACEHOLDER).size().x;
    if width(s) <= w {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    for n in (1..chars.len()).rev() {
        let t = format!("{}\u{2026}", chars[..n].iter().collect::<String>().trim_end());
        if width(&t) <= w {
            return t;
        }
    }
    "\u{2026}".into()
}

/// The device's glyph (chip, memory stick, drive, network, graphics card,
/// bolt) in the theme's own colours.
fn paint_icon(p: &Painter, c: Pos2, icon: Icon) {
    let name = match icon {
        Icon::Cpu => "chip",
        Icon::Mem => "memory-stick",
        Icon::Disk => "drive",
        Icon::Net => "network",
        Icon::Gpu => "graphics-card",
        Icon::Power => "bolt",
    };
    crate::widgets::icons::draw_glyph(p, c, 16.0, name, crate::widgets::icons::Ink::Theme);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::core_page::testing::draw;
    use crate::core_page::Histories;

    const HARD_FAULTS: &str = "Hard faults, pages read from disk (file-backed paging included)";

    fn shows(texts: &[String], word: &str) -> bool {
        texts.iter().any(|t| t == word)
    }

    /// One frame of `page` in `ctx` with the given Performance history.
    fn frame_with(ctx: &egui::Context, page: &mut PerformancePage, settings: &Settings, snap: &Snapshot, ph: &PerfHistory, events: Vec<egui::Event>) -> (Vec<String>, Vec<Action>, egui::FullOutput) {
        let (cpu, mem) = (VecDeque::new(), VecDeque::new());
        let pc = PageCtx { settings, snapshot: snap, histories: Histories { cpu: &cpu, mem: &mem, perf: ph }, system: &crate::system_lists::fake::FakeSystem::default() };
        let mut actions = Vec::new();
        let out = crate::test_ui::run(ctx, (1400.0, 900.0), events, |ui| page.draw(ui, &pc, &mut actions));
        (crate::test_ui::texts(&out), actions, out)
    }

    /// The events of a click on the first text `word` drawn in `out`.
    fn click_on(out: &egui::FullOutput, word: &str) -> Vec<egui::Event> {
        let at = crate::test_ui::flat_shapes(out)
            .into_iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == word => Some(t.pos + vec2(6.0, 6.0)),
                _ => None,
            })
            .expect("the word is drawn");
        let button = |pressed| egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    #[test]
    fn it_draws_the_device_list_and_the_cpu_from_a_snapshot_without_the_app() {
        let drawn = draw(&mut PerformancePage::default(), &Settings::default(), &Snapshot::default());
        for word in ["Performance", "CPU", "Memory", "Network", "% Utilization by logical processor"] {
            assert!(shows(&drawn.texts, word), "{word} is drawn");
        }
        assert!(!shows(&drawn.texts, HARD_FAULTS), "the CPU is the device shown, not Memory");
        assert!(drawn.actions.is_empty(), "drawing alone asks for nothing");
    }

    #[test]
    fn it_opens_on_the_device_saved_in_the_settings() {
        let mut settings = Settings::default();
        settings.perf_device = "memory".into();
        let drawn = draw(&mut PerformancePage::default(), &settings, &Snapshot::default());
        assert!(shows(&drawn.texts, HARD_FAULTS), "Memory is shown");
        assert!(!shows(&drawn.texts, "% Utilization by logical processor"), "not the CPU");
    }

    #[test]
    fn a_click_on_a_device_shows_it_and_asks_the_app_to_save_the_choice() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (settings, snap, ph) = (Settings::default(), Snapshot::default(), PerfHistory::new());
        let mut page = PerformancePage::default();
        let (_, _, first) = frame_with(&ctx, &mut page, &settings, &snap, &ph, vec![]);
        let (_, actions, _) = frame_with(&ctx, &mut page, &settings, &snap, &ph, click_on(&first, "Memory"));
        assert_eq!(actions, [Action::PerfSetting(PerfSetting::Device("memory".into()))]);
        let (texts, actions, _) = frame_with(&ctx, &mut page, &settings, &snap, &ph, vec![]);
        assert!(shows(&texts, HARD_FAULTS), "Memory stays shown with the saved setting not yet applied");
        assert!(actions.is_empty());
    }

    #[test]
    fn a_device_missing_for_a_moment_shows_the_cpu_and_keeps_the_saved_choice() {
        let mut settings = Settings::default();
        settings.perf_device = "power".into();
        let mut page = PerformancePage::default();
        let drawn = draw(&mut page, &settings, &Snapshot::default());
        assert!(shows(&drawn.texts, "% Utilization by logical processor"), "the CPU meanwhile");
        let mut live = Snapshot::default();
        live.perf.cpu_power_w = Some(12.5);
        let drawn = draw(&mut page, &settings, &live);
        assert!(shows(&drawn.texts, "CPU package power"), "the saved device once it exists");
    }

    #[test]
    fn the_cpu_graph_menu_asks_for_the_settings_it_changes_through_actions() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (settings, snap, ph) = (Settings::default(), Snapshot::default(), PerfHistory::new());
        let mut page = PerformancePage::default();
        let (texts, actions, first) = frame_with(&ctx, &mut page, &settings, &snap, &ph, vec![]);
        assert!(shows(&texts, "CPUs Expanded"));
        assert!(actions.is_empty());
        // Opening the menu and choosing "Logical processors".
        let _ = frame_with(&ctx, &mut page, &settings, &snap, &ph, click_on(&first, "CPUs Expanded"));
        let (_, _, open) = frame_with(&ctx, &mut page, &settings, &snap, &ph, vec![]);
        let (_, actions, _) = frame_with(&ctx, &mut page, &settings, &snap, &ph, click_on(&open, "   Overall utilization"));
        assert_eq!(actions, [Action::PerfSetting(PerfSetting::Logical(false))]);
    }

    #[test]
    fn refresh_is_asked_of_the_app() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (settings, snap, ph) = (Settings::default(), Snapshot::default(), PerfHistory::new());
        let mut page = PerformancePage::default();
        let (_, _, first) = frame_with(&ctx, &mut page, &settings, &snap, &ph, vec![]);
        let (_, actions, _) = frame_with(&ctx, &mut page, &settings, &snap, &ph, click_on(&first, "      Refresh"));
        assert_eq!(actions, [Action::Refresh]);
    }

    #[test]
    fn the_graph_draws_the_history_the_app_lends() {
        let (settings, snap) = (Settings::default(), Snapshot::default());
        let ctx = crate::test_ui::context(Mode::Dark);
        let shapes = |ph: &PerfHistory| {
            let (_, _, out) = frame_with(&ctx, &mut PerformancePage::default(), &settings, &snap, ph, vec![]);
            crate::test_ui::flat_shapes(&out).len()
        };
        let ph: PerfHistory = [("cpu".to_string(), (0..60).map(|i| (i % 50) as f32).collect::<VecDeque<f32>>())].into();
        assert!(shapes(&ph) > shapes(&PerfHistory::new()), "a history adds a line to the graph");
    }

    #[test]
    fn a_custom_cpu_colour_reaches_performance() {
        let (settings, cpu) = crate::pages::tests::custom_cpu();
        let snap = Snapshot::default();
        assert_eq!(devices(&settings, &snap, machine_of(&snap, None), &PerfHistory::new())[0].color, cpu, "the CPU tile");
        let ctx = crate::test_ui::context(Mode::Dark);
        let (_, _, out) = frame_with(&ctx, &mut PerformancePage::default(), &settings, &snap, &PerfHistory::new(), vec![]);
        let drawn = crate::test_ui::colours(&out);
        assert!(drawn.contains(&cpu), "the CPU graph");
        assert!(!drawn.contains(&theme::GRAPHS.cpu), "no default CPU colour left");
    }

    /// Performance's "Speed" for a sample at `perf_pct`% of a `base_mhz`
    /// base clock, with sysinfo reporting 2.90 GHz, and Summary's view of it.
    fn speeds(base_mhz: u32, perf_pct: f32) -> (String, crate::summary_view::SummaryView) {
        let snap = Snapshot { seq: 1, cpu_freq_mhz: 2_900, perf: crate::perfdata::PerfLive { cpu_perf_pct: perf_pct, ..Default::default() }, ..Default::default() };
        let mut app = TaskApp::headless(crate::settings::Settings::default(), snap);
        app.machine_info = Some(crate::summary_view::tests::clocked(base_mhz));
        app.rebuild_summary();
        let stats = app.stats(PerfItem::Cpu);
        let speed = stats.top.iter().find(|(k, _)| k == "Speed").map(|(_, v)| v.clone()).expect("a Speed figure");
        (speed, app.summary_view().clone())
    }

    #[test]
    fn the_cpu_speed_is_the_base_clock_times_the_performance_level_and_blank_when_either_is_unknown() {
        assert_eq!(speeds(3_400, 110.0).0, "3.74 GHz", "base clock x performance level");
        assert_eq!(speeds(3_400, 0.0).0, "\u{2014}", "no performance level: blank, by the one shared rule");
        assert_eq!(speeds(0, 110.0).0, "\u{2014}", "no base clock: blank, by the one shared rule");
    }

    #[test]
    fn summary_shows_the_cpu_speed_performance_shows() {
        let (performance, summary) = speeds(3_400, 110.0);
        assert_eq!(summary.cpu.sub_text(), performance);
    }

    /// Power & Freq's "Core Average" (Hz) for a CPU read at `perf_pct`% of a
    /// `base_mhz` base clock, as the page's frame carries it.
    fn core_average_speed_hz(base_mhz: u32, perf_pct: f32) -> Option<f64> {
        let sample = crate::sensors::cpu::CpuSample {
            base_mhz: Some(base_mhz as f64),
            performance_percent: Some(perf_pct as f64),
            ..Default::default()
        };
        let frame = crate::sensors::model::Frame::build(1, &[sample.readings(std::time::Instant::now())], Vec::new());
        frame.reading(crate::sensors::cpu::CORE_AVERAGE).and_then(|r| r.value)
    }
    /// The same inputs through the app: one fake processor provider, read
    /// by the sampler for the core pages and by a page for the sensors.
    /// Returns Performance's "Speed", Summary's view and the sensor
    /// pipeline's "Core Average" (Hz).
    fn speeds_through_the_app(base_mhz: u32, perf_pct: f32) -> (String, crate::summary_view::SummaryView, Option<f64>) {
        use crate::sensors::{fake, service::ProviderGroup, SensorService};
        let sample = crate::sensors::cpu::CpuSample { base_mhz: Some(base_mhz as f64), performance_percent: Some(perf_pct as f64), ..Default::default() };
        let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", &fake::Script::cpu(sample)))]);
        let mut app = TaskApp::headless_over(crate::settings::Settings::default(), service.clone(), |snap: &mut Snapshot| snap.cpu_freq_mhz = 2_900);
        app.machine_info = Some(crate::summary_view::tests::clocked(base_mhz));
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.snap.seq == 0 {
            app.pull_samples();
            assert!(std::time::Instant::now() < end, "no sample");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        app.rebuild_summary();
        let speed = app.stats(PerfItem::Cpu).top.iter().find(|(k, _)| k == "Speed").map(|(_, v)| v.clone()).expect("a Speed figure");
        let page = service.interest();
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let frame = loop {
            if let Some(frame) = page.latest() {
                break frame;
            }
            assert!(std::time::Instant::now() < end, "no sensor frame");
            std::thread::sleep(std::time::Duration::from_millis(10));
        };
        let hz = frame.reading(crate::sensors::cpu::CORE_AVERAGE).and_then(|r| r.value);
        (speed, app.summary_view().clone(), hz)
    }

    #[test]
    fn summary_performance_and_the_core_average_give_the_same_speed_for_the_same_inputs() {
        let (performance, summary, hz) = speeds_through_the_app(3_400, 110.0);
        let hz = hz.expect("a Core Average");
        assert_eq!(crate::metrics::fmt_ghz((hz / 1e6) as f32), performance);
        assert_eq!(summary.cpu.sub_text(), performance);
        assert_eq!(performance, "3.74 GHz");
    }
    #[test]
    fn the_speed_is_blank_on_summary_and_the_core_average_when_an_input_is_unknown() {
        for (base, pct) in [(3_400, 0.0), (0, 110.0)] {
            assert_eq!(speeds(base, pct).1.cpu.sub_text(), "\u{2014}", "Summary at {base} MHz x {pct}%");
            assert_eq!(speeds(base, pct).0, "\u{2014}", "Performance at {base} MHz x {pct}%");
            assert_eq!(core_average_speed_hz(base, pct), None,"Power & Freq at {base} MHz x {pct}%");
        }
    }
    /// Performance's combined Network device for `net`.
    fn network_device(net: Vec<crate::sampler::NetRow>) -> Dev {
        let mut app = TaskApp::headless(crate::settings::Settings::default(), Snapshot { seq: 1, net, ..Default::default() });
        app.settings.bits_per_second = false;
        devices(&app.settings, &app.snap, app.machine(), &app.ph).into_iter().find(|d| d.item == PerfItem::Network).expect("a Network device")
    }

    #[test]
    fn performance_network_device_is_unchanged_by_the_shared_combining() {
        use crate::sampler::NetRow;
        let both = network_device(vec![
            NetRow { name: "Ethernet".into(), rx_bps: 1024.0, tx_bps: 2048.0, total_rx: 10, total_tx: 0 },
            NetRow { name: "Wi-Fi".into(), rx_bps: 1024.0, tx_bps: 0.0, total_rx: 0, total_tx: 5 },
            NetRow { name: "Bluetooth".into(), ..Default::default() },
        ]);
        assert_eq!(both.line1, "2 network interfaces", "adapters that carried bytes");
        let two_kb = fmt_bytes_rate(2048.0);
        assert_eq!(both.line2, format!("S: {two_kb}  R: {two_kb}"), "sent and received summed over every adapter");
        assert_eq!(network_device(vec![]).line1, "1 network interface", "never fewer than one, as before");
    }
}
