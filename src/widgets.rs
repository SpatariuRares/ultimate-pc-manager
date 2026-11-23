//! Small drawing helpers and number formatting.

pub mod icons;
pub mod parts;

use std::collections::VecDeque;

use egui::{vec2, Color32, CornerRadius, Mesh, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind, Ui};

use crate::theme::{self, Mode, Theme};

/// Samples kept for each graph: 60 points, "over 60 seconds" at Normal speed.
pub const HISTORY: usize = 60;

pub fn push(h: &mut VecDeque<f32>, v: f32) {
    h.push_back(v);
    while h.len() > HISTORY {
        h.pop_front();
    }
}

/// The colour of secondary text: `t`'s label grey.
pub fn sub_text(t: &Theme) -> Color32 {
    t.label
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn pct(a: u64, b: u64) -> f64 {
    if b == 0 { 0.0 } else { a as f64 * 100.0 / b as f64 }
}

pub fn auto_max(h: &VecDeque<f32>, floor: f32) -> f32 {
    let m = h.iter().copied().fold(0.0f32, f32::max);
    (m * 1.25).max(floor)
}

pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    let k = 1024f64;
    if b >= k.powi(4) {
        format!("{:.1} TB", b / k.powi(4))
    } else if b >= k.powi(3) {
        format!("{:.1} GB", b / k.powi(3))
    } else if b >= k.powi(2) {
        format!("{:.1} MB", b / k.powi(2))
    } else if b >= k {
        format!("{:.0} KB", b / k)
    } else {
        format!("{b:.0} B")
    }
}

pub fn fmt_mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / 1_048_576.0)
}

pub fn fmt_uptime(s: u64) -> String {
    format!("{}:{:02}:{:02}:{:02}", s / 86400, (s % 86400) / 3600, (s % 3600) / 60, s % 60)
}

/// A process's CPU time as the table shows it: "0:00:03" under a
/// minute, "5:21.01" (minutes, seconds, hundredths) under an hour, and
/// "1:02:03" from an hour on.
pub fn fmt_cpu_time(t: std::time::Duration) -> String {
    let s = t.as_secs();
    if s < 60 {
        format!("0:00:{s:02}")
    } else if s < 3600 {
        format!("{}:{:02}.{:02}", s / 60, s % 60, t.subsec_millis() / 10)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }
}

pub fn shorten(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}\u{2026}", s.chars().take(n - 1).collect::<String>())
    }
}

pub fn first_path(cmd: &str) -> String {
    let c = cmd.trim().trim_start_matches('"');
    if let Some(i) = c.to_lowercase().find(".exe") {
        return c[..i + 4].to_string();
    }
    c.split('"').next().unwrap_or(c).to_string()
}

pub fn cpu_brand() -> String {
    use std::sync::OnceLock;
    static BRAND: OnceLock<String> = OnceLock::new();
    BRAND
        .get_or_init(|| {
            let mut s = sysinfo::System::new();
            s.refresh_cpu_all();
            s.cpus().first().map(|c| c.brand().trim().to_string()).unwrap_or_default()
        })
        .clone()
}

/// A last-minute graph of `data` (0..`max`) in `color`, with its grid, on
/// the always-dark graph surface (dark in both themes, as Design A keeps it).
pub fn graph(ui: &mut Ui, size: egui::Vec2, data: &[f32], max: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    paint_graph(ui.painter(), rect, data, max, color, true);
}

pub fn paint_graph(p: &egui::Painter, rect: Rect, data: &[f32], max: f32, color: Color32, grid: bool) {
    // The same in both modes: graphs stay dark.
    p.rect_filled(rect, CornerRadius::ZERO, Theme::of(Mode::Dark).graph_bg);
    if grid {
        let gc = color.linear_multiply(0.18);
        for i in 1..10 {
            let x = rect.left() + rect.width() * i as f32 / 10.0;
            p.line_segment([Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())], Stroke::new(1.0, gc));
        }
        for i in 1..5 {
            let y = rect.top() + rect.height() * i as f32 / 5.0;
            p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, gc));
        }
    }
    paint_series(p, rect, data, max, color);
    p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, color.gamma_multiply(0.5)), StrokeKind::Inside);
}

/// One series of a last-minute graph (filled under its line) in `rect`,
/// without the surface: a second series drawn over [`paint_graph`]. A NaN
/// point is a sample with no reading: the line breaks there.
pub fn paint_series(p: &egui::Painter, rect: Rect, data: &[f32], max: f32, color: Color32) {
    let max = max.max(0.0001);
    let step = rect.width() / (HISTORY - 1) as f32;
    let start = HISTORY.saturating_sub(data.len());
    let point = |i: usize, v: f32| Pos2::new(rect.left() + (start + i) as f32 * step, rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height());
    let mut run: Vec<Pos2> = Vec::new();
    for (i, v) in data.iter().enumerate() {
        if v.is_nan() {
            paint_run(p, rect, std::mem::take(&mut run), color);
        } else {
            run.push(point(i, *v));
        }
    }
    paint_run(p, rect, run, color);
}

/// One unbroken stretch of a series: its line and the fill under it.
fn paint_run(p: &egui::Painter, rect: Rect, pts: Vec<Pos2>, color: Color32) {
    if pts.len() >= 2 {
        let mut mesh = Mesh::default();
        let fill = color.linear_multiply(0.25);
        for w in pts.windows(2) {
            let base = mesh.vertices.len() as u32;
            mesh.colored_vertex(w[0], fill);
            mesh.colored_vertex(w[1], fill);
            mesh.colored_vertex(Pos2::new(w[1].x, rect.bottom()), fill);
            mesh.colored_vertex(Pos2::new(w[0].x, rect.bottom()), fill);
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        p.add(Shape::mesh(mesh));
        p.add(Shape::line(pts, Stroke::new(1.5, color)));
    }
}

pub fn stats_grid(ui: &mut Ui, id: &str, items: &[(&str, String)], sub: Color32) {
    egui::Grid::new(id).num_columns(4).spacing(vec2(28.0, 6.0)).show(ui, |ui| {
        for (i, (k, v)) in items.iter().enumerate() {
            ui.vertical(|ui| {
                ui.set_min_width(120.0);
                ui.add(egui::Label::new(RichText::new(*k).size(11.5).color(sub)).extend());
                ui.add(egui::Label::new(RichText::new(v).size(17.0)).extend());
            });
            if i % 4 == 3 {
                ui.end_row();
            }
        }
    });
}

/// The outlined "Refresh" button with the refresh glyph, top right of a page.
pub fn refresh_button(ui: &mut Ui) -> egui::Response {
    let resp = ui.add(egui::Button::new("      Refresh").min_size(vec2(90.0, 32.0)));
    let c = Pos2::new(resp.rect.left() + 17.0, resp.rect.center().y);
    icons::draw_glyph(ui.painter(), c, 15.0, "refresh", icons::Ink::Theme);
    resp.on_hover_text("Refresh now (F5)")
}

/// The "Run new task" button with the plus glyph, beside Refresh.
pub fn run_task_button(ui: &mut Ui) -> egui::Response {
    let resp = ui.add(egui::Button::new("      Run new task").min_size(vec2(0.0, 32.0)));
    let c = Pos2::new(resp.rect.left() + 17.0, resp.rect.center().y);
    icons::draw_glyph(ui.painter(), c, 13.0, "plus", icons::Ink::Theme);
    resp
}

/// The brand's wordmark (the U symbol and "Ultimate PC Manager"), in its
/// version for the dark or the light sidebar.
pub fn logo(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let tex = brand_logo(ui.ctx(), ui.visuals().dark_mode);
    let [w, h] = tex.size().map(|v| v as f32);
    // Drawn at half its size (sharp on 2x screens), narrower if the sidebar is.
    let width = (w / 2.0).min(r.width() - 4.0);
    let size = vec2(width, width * h / w);
    let rect = egui::Rect::from_min_size(Pos2::new(r.left() + 2.0, r.center().y - size.y / 2.0), size);
    egui::Image::new((tex.id(), size)).paint_at(ui, rect);
}

/// The wordmark's texture, loaded once per theme and kept in egui's memory.
fn brand_logo(ctx: &egui::Context, dark: bool) -> egui::TextureHandle {
    let id = egui::Id::new(("brand-logo", dark));
    if let Some(tex) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }
    let tex = ctx.load_texture("brand-logo", decode_png(logo_png(dark)), egui::TextureOptions::LINEAR);
    ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
    tex
}

fn logo_png(dark: bool) -> &'static [u8] {
    if dark {
        include_bytes!("../assets/brand/sidebar-dark.png")
    } else {
        include_bytes!("../assets/brand/sidebar-light.png")
    }
}

/// A bundled PNG as an egui image.
fn decode_png(bytes: &[u8]) -> egui::ColorImage {
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).expect("bundled PNG").to_rgba8();
    egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
}

pub fn local_ip() -> String {
    use std::sync::OnceLock;
    static IP: OnceLock<String> = OnceLock::new();
    IP.get_or_init(|| {
        // Asks the system which address it would use; no data is sent.
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|s| {
                s.connect("8.8.8.8:80")?;
                s.local_addr()
            })
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|_| "offline".into())
    })
    .clone()
}

/// A Design A graph card: a panel with the title on the left and the value
/// on the right, both in the display face, the value in the series `color`
/// (dark theme; the text colour in the light one), as Summary's "CPU
/// OVERVIEW" card, then `add`.
pub fn card(ui: &mut Ui, title: &str, value: &str, color: Color32, width: f32, add: impl FnOnce(&mut Ui)) {
    let t = theme::of_ui(ui);
    // The series colours are made for dark surfaces; on the light panel the
    // value is written in the text colour, for contrast.
    let ink = if t.mode == Mode::Dark { color } else { t.text };
    egui::Frame::new()
        .fill(t.panel)
        .stroke(Stroke::new(1.0, t.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(width - 30.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).font(display_font(ui, 15.0)).color(t.text));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(value).font(display_font(ui, 18.0)).color(ink));
                    });
                });
                add(ui);
            });
        });
}

/// A row of small squares, one per core, brighter when busier.
pub fn core_squares(ui: &mut Ui, values: &[f32], color: Color32) {
    let size = 10.0;
    let gap = 3.0;
    let per_row = ((ui.available_width() + gap) / (size + gap)).floor().max(1.0) as usize;
    let rows = values.len().div_ceil(per_row).max(1);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * (size + gap)), Sense::hover());
    let idle = theme::of_ui(ui).hover;
    for (i, v) in values.iter().enumerate() {
        let x = rect.left() + (i % per_row) as f32 * (size + gap);
        let y = rect.top() + (i / per_row) as f32 * (size + gap);
        let a = (v / 100.0).clamp(0.0, 1.0);
        let fill = if a < 0.02 { idle } else { color.linear_multiply(0.2 + 0.8 * a) };
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(x, y), vec2(size, size)), CornerRadius::ZERO, fill);
    }
}

/// The stroke of the sidebar icons.
pub fn icon_stroke(ink: Color32) -> Stroke {
    Stroke::new(1.2, ink)
}

/// The icon of a page that draws none of its own: a rounded square.
pub fn plain_icon(p: &egui::Painter, c: Pos2, ink: Color32) {
    p.rect_stroke(Rect::from_center_size(c, vec2(11.0, 11.0)), CornerRadius::same(2), icon_stroke(ink), StrokeKind::Inside);
}

// ------------------------------------------- shared by the add-on pages

/// "yyyy-mm-dd" → (year, month, day).
pub fn parse_date(s: &str) -> Option<(i64, u32, u32)> {
    let mut it = s.split('-');
    Some((it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
}

/// "2026-07-22" → "22 July 2026".
pub fn long_date(s: &str) -> String {
    const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    match parse_date(s) {
        Some((y, m, d)) if (1..=12).contains(&m) => format!("{d} {} {y}", MONTHS[m as usize - 1]),
        _ => s.to_string(),
    }
}

/// "17:52:36" → "17:52".
pub fn short_time(t: &str) -> String {
    t.rsplit_once(':').filter(|(h, _)| h.contains(':')).map(|(h, _)| h.to_string()).unwrap_or_else(|| t.to_string())
}

/// Version text split at the first part that differs from `other`:
/// (same start, changed rest).
pub fn split_version(v: &str, other: &str) -> (String, String) {
    let a: Vec<&str> = v.split('.').collect();
    let b: Vec<&str> = other.split('.').collect();
    let same = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    if same == 0 {
        return (String::new(), v.to_string());
    }
    let head = a[..same].join(".") + ".";
    let tail = a[same..].join(".");
    if tail.is_empty() { (String::new(), v.to_string()) } else { (head, tail) }
}

/// A thin full-width rule.
pub fn separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_stroke(r, CornerRadius::ZERO, Stroke::new(1.0, theme::of_ui(ui).line), StrokeKind::Inside);
}

/// The open / closed mark of a row that expands: the "chevron-down" glyph
/// when `open`, else "chevron-right" (trees, accounts).
pub fn disclosure(p: &egui::Painter, c: Pos2, open: bool) {
    let name = if open { "chevron-down" } else { "chevron-right" };
    icons::draw_glyph(p, c, 11.0, name, icons::Ink::Theme);
}

/// The separator between the parts of a path ("Settings › Add-ons"): the
/// "chevron-right" glyph, small.
pub fn crumb_separator(ui: &mut Ui) {
    let (r, _) = ui.allocate_exact_size(vec2(10.0, 16.0), Sense::hover());
    icons::draw_glyph(ui.painter(), r.center(), 9.0, "chevron-right", icons::Ink::Theme);
}

/// A check mark (the "done" glyph) centred on `c`.
pub fn tick(p: &egui::Painter, c: Pos2) {
    icons::draw_glyph(p, c, 14.0, "done", icons::Ink::Theme);
}

// ------------------------------------------- requirement icons

/// The icon of a requirement: the UAC shield (administrator rights), a
/// globe (internet) or a terminal (an external program).
pub fn need_icon(p: &egui::Painter, c: Pos2, need: crate::addons::overview::Need) {
    use crate::addons::overview::Need;
    let name = match need {
        Need::Admin => "admin-shield",
        Need::Internet => "internet",
        Need::Program => "program",
    };
    icons::draw_glyph(p, c, 14.0, name, icons::Ink::Theme);
}

/// A small coloured dot inline with text.
pub fn dot(ui: &mut Ui, color: Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
    ui.painter().circle_filled(r.center(), 4.0, color);
}

/// A compact bordered strip (panel fill, 1 px rule, 6 radius) that lays its
/// contents out in a wrapping row (a read-only summary band).
pub fn strip(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    let t = theme::of_ui(ui);
    egui::Frame::new().fill(t.panel).stroke(Stroke::new(1.0, t.line)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(14.0, 6.0);
            add(ui);
        });
    });
}

// ------------------------------------------- drawn line icons and tree rows

/// A line icon drawn on a 24-unit grid (the prototypes' SVG icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Thermometer,
    Gauge,
    Clock,
    Bolt,
    Wave,
    Sliders,
    ShieldCheck,
    Trend,
    Info,
    Fan,
    Monitor,
    Chip,
    GraphicsCard,
    Hexagon,
    Board,
    MemoryStick,
    Drive,
    Battery,
    Search,
    /// An arrow pointing up ("Parent").
    Up,
}

/// Draws `g` centred on `c`, `size` points wide, in `ink` (its own
/// colours, faded, or tinted for a filled background).
pub fn glyph(p: &egui::Painter, c: Pos2, size: f32, g: Glyph, ink: icons::Ink) {
    icons::draw_glyph(p, c, size, g.file(), ink);
}

impl Glyph {
    /// Its file in `assets/glyphs`.
    pub fn file(self) -> &'static str {
        match self {
            Glyph::Thermometer => "thermometer",
            Glyph::Gauge => "gauge",
            Glyph::Clock => "clock",
            Glyph::Bolt => "bolt",
            Glyph::Wave => "wave",
            Glyph::Sliders => "sliders",
            Glyph::ShieldCheck => "shield-check",
            Glyph::Trend => "trend",
            Glyph::Info => "info",
            Glyph::Fan => "fan",
            Glyph::Monitor => "monitor",
            Glyph::Chip => "chip",
            Glyph::GraphicsCard => "graphics-card",
            Glyph::Hexagon => "hexagon",
            Glyph::Board => "motherboard",
            Glyph::MemoryStick => "memory-stick",
            Glyph::Drive => "drive",
            Glyph::Battery => "battery",
            Glyph::Search => "search",
            Glyph::Up => "up",
        }
    }
}

/// The colours that tell kinds of readings apart (temperature orange,
/// load green, clocks red, power yellow, ...), in the dark or light theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hue {
    Orange,
    Green,
    Red,
    Yellow,
    Violet,
    Teal,
    Blue,
    Grey,
}

/// The colour of `h` in `t`'s graph palette (the user's saved colours
/// with `Settings::theme`, as on Summary and Performance); grey is the
/// theme's label grey.
pub fn hue(h: Hue, t: &Theme) -> Color32 {
    let g = t.graphs;
    match h {
        Hue::Orange => g.thermal,
        Hue::Green => g.utilisation,
        Hue::Red => g.kernel,
        Hue::Yellow => g.power,
        Hue::Violet => g.memory,
        Hue::Teal => g.disk_write,
        Hue::Blue => g.cpu,
        Hue::Grey => t.label,
    }
}

/// The wide display face at `size`, or the body face where it is not
/// loaded (a bare context, as in tests).
pub fn display_font(ui: &Ui, size: f32) -> egui::FontId {
    let family = egui::FontFamily::Name("display".into());
    if ui.fonts(|f| f.definitions().families.contains_key(&family)) { egui::FontId::new(size, family) } else { egui::FontId::proportional(size) }
}

/// One line of text laid out to fit `width` (cut with "…").
pub fn fitted(ui: &Ui, text: &str, size: f32, color: Color32, italics: bool, width: f32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat { font_id: egui::FontId::proportional(size), color, italics, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(1.0));
    ui.painter().layout_job(job)
}

/// A button with a drawn icon before its words; `primary` fills it with
/// that colour and writes in the theme's `on_primary`.
pub fn icon_button(ui: &mut Ui, g: Glyph, text: &str, primary: Option<Color32>) -> egui::Response {
    let ink = if primary.is_some() { theme::of_ui(ui).on_primary } else { ui.visuals().text_color() };
    let mut b = egui::Button::new(RichText::new(format!("      {text}")).size(13.0).color(ink)).min_size(vec2(0.0, 28.0));
    if let Some(fill) = primary {
        b = b.fill(fill).stroke(Stroke::new(1.0, fill));
    }
    let resp = ui.add(b);
    let look = if primary.is_some() { icons::Ink::On(ink) } else { icons::Ink::Theme };
    glyph(ui.painter(), Pos2::new(resp.rect.left() + 16.0, resp.rect.center().y), 15.0, g, look);
    resp
}

/// A horizontal meter: a track filled to `value` (0..1) with a tick at
/// `mark` (0..1).
pub fn meter_with_tick(ui: &mut Ui, width: f32, value: f32, mark: Option<f32>, color: Color32) {
    let t = crate::theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    let track = Rect::from_center_size(r.center(), vec2(width, 8.0));
    let p = ui.painter();
    p.rect_filled(track, CornerRadius::same(4), t.hover);
    let fill = Rect::from_min_max(track.min, Pos2::new(track.left() + track.width() * value.clamp(0.0, 1.0), track.bottom()));
    p.rect_filled(fill, CornerRadius::same(4), color);
    if let Some(m) = mark {
        let x = track.left() + track.width() * m.clamp(0.0, 1.0);
        p.rect_filled(Rect::from_min_max(Pos2::new(x - 1.0, r.top()), Pos2::new(x + 1.0, r.bottom())), CornerRadius::same(1), t.text);
    }
}

/// A last-minute line on a soft rounded field, scaled to its own range,
/// with a dashed line at `base` and a dot on the latest value.
pub fn sparkline(ui: &mut Ui, size: egui::Vec2, data: &[f32], base: Option<f32>, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let p = ui.painter();
    let sub = sub_text(&theme::of_ui(ui));
    p.rect_filled(rect, CornerRadius::same(4), ui.visuals().faint_bg_color);
    let guide = ui.visuals().widgets.noninteractive.bg_stroke.color.gamma_multiply(0.7);
    for f in [0.25, 0.5, 0.75] {
        let y = rect.top() + rect.height() * f;
        p.line_segment([Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)], Stroke::new(1.0, guide));
    }
    if data.len() < 2 {
        p.text(rect.center(), egui::Align2::CENTER_CENTER, "No readings to draw", egui::FontId::proportional(11.0), sub);
        return;
    }
    let (mut lo, mut hi) = data.iter().copied().chain(base).fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v), b.max(v)));
    if hi - lo < 1e-6 {
        hi += 1.0;
        lo = (lo - 1.0).max(0.0);
    }
    let span = hi - lo;
    let (lo, hi) = (lo - span * 0.1, hi + span * 0.1);
    let inner = rect.shrink(4.0);
    let step = inner.width() / (HISTORY - 1) as f32;
    let start = HISTORY.saturating_sub(data.len());
    let y = |v: f32| inner.bottom() - (v - lo) / (hi - lo) * inner.height();
    let pts: Vec<Pos2> = data.iter().enumerate().map(|(i, v)| Pos2::new(inner.left() + (start + i) as f32 * step, y(*v))).collect();
    let mut mesh = Mesh::default();
    let fill = color.gamma_multiply(0.14);
    for w in pts.windows(2) {
        let b = mesh.vertices.len() as u32;
        mesh.colored_vertex(w[0], fill);
        mesh.colored_vertex(w[1], fill);
        mesh.colored_vertex(Pos2::new(w[1].x, rect.bottom()), fill);
        mesh.colored_vertex(Pos2::new(w[0].x, rect.bottom()), fill);
        mesh.add_triangle(b, b + 1, b + 2);
        mesh.add_triangle(b, b + 2, b + 3);
    }
    p.add(Shape::mesh(mesh));
    let last = *pts.last().unwrap_or(&rect.center());
    p.add(Shape::line(pts, Stroke::new(1.6, color)));
    if let Some(b) = base {
        let yb = y(b);
        p.extend(Shape::dashed_line(&[Pos2::new(rect.left(), yb), Pos2::new(rect.right(), yb)], Stroke::new(1.0, sub), 4.0, 3.0));
    }
    p.circle_filled(last, 3.0, color);
}

/// What comes before a tree row's name.
#[derive(Clone, Copy, Debug)]
pub enum RowLead {
    /// An arrow (open or closed) and an optional icon: a row that opens.
    Caret { open: bool, icon: Option<Glyph> },
    /// An icon in the arrow's place's neighbour: a heading that does not open.
    Icon(Glyph),
    /// A small dot of the group's colour: a leaf.
    Dot(Color32),
}

/// One row of a Sensor / Value / Min / Avg / Max tree.
pub struct TreeRow<'a> {
    pub depth: u8,
    pub lead: RowLead,
    pub name: &'a str,
    /// Headings of devices: semibold.
    pub strong: bool,
    /// Faint name: nothing to read.
    pub grey: bool,
    /// Small faint words after the name (a GPU's clock and power).
    pub note: &'a str,
    pub value: &'a str,
    /// The value is why it is missing: faint italics.
    pub missing: bool,
    /// The value is old: an amber "(stale)" after it.
    pub stale: bool,
    pub min: &'a str,
    pub avg: &'a str,
    pub max: &'a str,
    pub selected: bool,
}

/// Widths of the Value, Min, Avg and Max columns of [`tree_row`].
pub const TREE_COLUMNS: [f32; 4] = [210.0, 92.0, 92.0, 92.0];
/// Height of a [`tree_row`].
pub const TREE_ROW_HEIGHT: f32 = 28.0;

/// Right-aligns `g` in the column ending at `right`.
fn cell(p: &egui::Painter, g: std::sync::Arc<egui::Galley>, right: f32, cy: f32) {
    let pos = Pos2::new(right - 10.0 - g.size().x, cy - g.size().y / 2.0);
    p.galley(pos, g, Color32::PLACEHOLDER);
}

/// The header of a tree (Design A): Sensor on the left, the value columns
/// right-aligned, in the label grey over a rule.
pub fn tree_header(ui: &mut Ui, width: f32, labels: [&str; 5]) {
    let t = crate::theme::of_ui(ui);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 30.0), Sense::hover());
    let p = ui.painter();
    p.line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, t.line));
    let cy = rect.center().y;
    p.text(Pos2::new(rect.left() + 10.0, cy), egui::Align2::LEFT_CENTER, labels[0], t.fonts.label.clone(), t.label);
    let mut right = rect.right();
    for (i, w) in TREE_COLUMNS.iter().enumerate().rev() {
        p.text(Pos2::new(right - 10.0, cy), egui::Align2::RIGHT_CENTER, labels[i + 1], t.fonts.label.clone(), t.label);
        right -= w;
    }
}

/// Draws one tree row `width` wide (Design A); click it to open/close or
/// select it. Value, Min, Avg and Max are right-aligned; a cut value shows in
/// full on hover.
pub fn tree_row(ui: &mut Ui, row: &TreeRow, width: f32) -> egui::Response {
    let t = crate::theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, TREE_ROW_HEIGHT), Sense::click());
    let ink = if row.grey {
        t.faint
    } else if row.selected {
        t.on_selected
    } else {
        t.text
    };
    let p = ui.painter().clone();
    if row.selected {
        p.rect_filled(rect, CornerRadius::same(5), t.selected);
    } else if resp.hovered() {
        p.rect_filled(rect, CornerRadius::same(5), t.hover);
    }
    let cy = rect.center().y;
    let mut x = rect.left() + 10.0 + row.depth as f32 * 18.0;
    match row.lead {
        RowLead::Caret { open, icon } => {
            disclosure(&p, Pos2::new(x + 7.0, cy), open);
            x += 20.0;
            if let Some(g) = icon {
                glyph(&p, Pos2::new(x + 8.0, cy), 16.0, g, icons::Ink::Theme);
                x += 22.0;
            }
        }
        RowLead::Icon(g) => {
            x += 20.0;
            glyph(&p, Pos2::new(x + 8.0, cy), 16.0, g, icons::Ink::Theme);
            x += 22.0;
        }
        RowLead::Dot(col) => {
            x += 20.0;
            p.circle_filled(Pos2::new(x + 3.0, cy), 3.0, col);
            x += 12.0;
        }
    }
    let name_end = rect.right() - TREE_COLUMNS.iter().sum::<f32>() - 8.0;
    let name = if row.strong {
        let mut job = egui::text::LayoutJob::single_section(row.name.to_string(), egui::TextFormat { font_id: egui::FontId::new(13.0, crate::theme::semibold()), color: ink, ..Default::default() });
        job.wrap = egui::text::TextWrapping::truncate_at_width((name_end - x).max(1.0));
        ui.painter().layout_job(job)
    } else {
        fitted(ui, row.name, 13.0, ink, false, name_end - x)
    };
    let after = x + name.size().x;
    p.galley(Pos2::new(x, cy - name.size().y / 2.0), name, ink);
    let sub = crate::widgets::parts::sub_colour(&t, row.selected);
    if !row.note.is_empty() && name_end - after > 40.0 {
        let note = fitted(ui, row.note, 12.0, sub, false, name_end - after - 12.0);
        p.galley(Pos2::new(after + 12.0, cy - note.size().y / 2.0), note, sub);
    }
    let mut right = rect.right();
    let [vw, nw, aw, xw] = TREE_COLUMNS;
    for (text, w) in [(row.max, xw), (row.avg, aw), (row.min, nw)] {
        if !text.is_empty() {
            cell(&p, fitted(ui, text, 13.0, t.soft, false, w - 16.0), right, cy);
        }
        right -= w;
    }
    let mut cut = false;
    if !row.value.is_empty() {
        let room = vw - 20.0;
        if row.stale {
            let tag = fitted(ui, "(stale)", 11.5, t.amber, false, room);
            let tag_w = tag.size().x;
            cell(&p, tag, right, cy);
            let g = fitted(ui, row.value, 13.0, t.faint, false, room - tag_w - 5.0);
            cut = g.elided;
            cell(&p, g, right - tag_w - 5.0, cy);
        } else {
            let color = if row.missing { t.faint } else { ink };
            let g = fitted(ui, row.value, 13.0, color, row.missing, room);
            cut = g.elided;
            cell(&p, g, right, cy);
        }
    }
    if cut { resp.on_hover_text(row.value) } else { resp }
}

/// The "Create a restore point first" option of a confirm dialog: the same
/// administrator prompt carries it, so it costs no extra prompt.
pub fn restore_point_option(ui: &mut Ui, on: &mut bool) -> egui::Response {
    let th = crate::theme::of_ui(ui);
    ui.horizontal(|ui| {
        let r = ui.checkbox(on, "Create a restore point first");
        ui.label(egui::RichText::new("(same administrator prompt)").size(11.5).color(th.label));
        r
    })
    .inner
}

/// A search field with the magnifier glyph, up to `width` wide. The
/// magnifier stays on the left in a right-to-left header too.
pub fn search_field(ui: &mut Ui, text: &mut String, hint: &str, width: f32) -> egui::Response {
    let line = ui.visuals().widgets.inactive.bg_stroke;
    let mut out = None;
    egui::Frame::new().stroke(line).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let (r, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
            glyph(ui.painter(), r.center(), 15.0, Glyph::Search, icons::Ink::Theme);
            out = Some(ui.add(egui::TextEdit::singleline(text).hint_text(hint).frame(false).desired_width(width - 37.0)));
        });
    });
    out.expect("drawn above")
}

// ------------------------------------------- Design A colours and parts


/// A progress bar of `segments` green LEDs in a dark housing (dark in both
/// themes), the first `fraction` of them lit.
pub fn led_strip(ui: &mut Ui, width: f32, segments: usize, fraction: f32) -> egui::Response {
    let t = theme::of_ui(ui);
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(40.0), 17.0), Sense::hover());
    let p = ui.painter();
    p.rect(rect, CornerRadius::same(4), t.led_bg, Stroke::new(1.0, t.led_line), StrokeKind::Inside);
    let inner = rect.shrink(4.0);
    let n = segments.max(1);
    let gap = 2.0;
    let w = ((inner.width() - gap * (n - 1) as f32) / n as f32).max(1.0);
    let lit = (fraction.clamp(0.0, 1.0) * n as f32).round() as usize;
    // The disk graph's green (Design A); unlit LEDs are a dim copy of it.
    let on = t.graphs.disk;
    let off = on.gamma_multiply(0.25);
    for i in 0..n {
        let r = Rect::from_min_size(Pos2::new(inner.left() + i as f32 * (w + gap), inner.top()), vec2(w, inner.height()));
        if i < lit {
            p.rect_filled(r.expand(1.0), CornerRadius::same(2), on.gamma_multiply(0.3));
            p.rect_filled(r, CornerRadius::same(1), on);
        } else {
            p.rect_filled(r, CornerRadius::same(1), off);
        }
    }
    resp
}

/// A ball in `fill` with a soft highlight up-left and a darker rim, so it
/// reads as a sphere, outlined by `outline`.
pub fn shaded_ball(p: &egui::Painter, centre: Pos2, r: f32, fill: Color32, outline: Stroke) {
    p.circle_filled(centre, r, fill);
    // A radial gradient as a mesh: light at the highlight, clear halfway,
    // dark at the rim.
    let (light, dark) = (theme::SHADING.highlight, theme::SHADING.rim_shadow);
    let spot = centre + vec2(-0.12 * r, -0.18 * r);
    let steps = (r * 0.8).clamp(24.0, 72.0) as usize;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(spot, light);
    for i in 0..steps {
        let a = i as f32 / steps as f32 * std::f32::consts::TAU;
        let rim = centre + vec2(a.cos(), a.sin()) * r;
        mesh.colored_vertex(spot + (rim - spot) * 0.55, Color32::TRANSPARENT);
        mesh.colored_vertex(rim, dark);
    }
    for i in 0..steps as u32 {
        let (m0, r0) = (1 + 2 * i, 2 + 2 * i);
        let j = (i + 1) % steps as u32;
        let (m1, r1) = (1 + 2 * j, 2 + 2 * j);
        mesh.add_triangle(0, m0, m1);
        mesh.add_triangle(m0, r0, r1);
        mesh.add_triangle(m0, r1, m1);
    }
    p.add(Shape::mesh(mesh));
    if outline.width > 0.0 {
        p.circle_stroke(centre, r, outline);
    }
}

/// A dashed outline around `rect`.
pub fn dashed_rect(p: &egui::Painter, rect: Rect, stroke: Stroke) {
    let pts = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom(), rect.left_top()];
    p.extend(Shape::dashed_line(&pts, stroke, 6.0, 4.0));
}

/// A path cut in the middle to at most `max` characters, keeping the
/// drive and first folder and as many of the last parts as fit:
/// `C:\Users\…\Desktop\progetti`. A path too short to cut is cut at the end.
pub fn mid_truncate(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    // Paths given on the command line may use '/'.
    let sep = if path.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = path.split(sep).filter(|s| !s.is_empty()).collect();
    if parts.len() >= 3 {
        let head = format!("{}{sep}{}", parts[0], parts[1]);
        let mut tail = parts[parts.len() - 1].to_string();
        for i in (2..parts.len() - 1).rev() {
            let t = format!("{}{sep}{}", parts[i], tail);
            if head.chars().count() + 3 + t.chars().count() > max {
                break;
            }
            tail = t;
        }
        let cut = format!("{head}{sep}\u{2026}{sep}{tail}");
        if cut.chars().count() <= max {
            return cut;
        }
    }
    let mut s: String = path.chars().take(max.saturating_sub(1)).collect();
    s.push('\u{2026}');
    s
}

/// `path` cut in the middle to fit `width` points in `font`; true when cut.
pub fn fit_path(ui: &Ui, path: &str, font: &egui::FontId, width: f32) -> (String, bool) {
    let measure = |s: &str| ui.painter().layout_no_wrap(s.to_string(), font.clone(), Color32::PLACEHOLDER).size().x;
    let full = measure(path);
    if full <= width {
        return (path.to_string(), false);
    }
    let mut n = path.chars().count();
    let mut out = path.to_string();
    for _ in 0..8 {
        let next = ((n as f32 * width / measure(&out).max(1.0)).floor() as usize).min(n.saturating_sub(1)).max(4);
        n = next;
        out = mid_truncate(path, n);
        if measure(&out) <= width || n <= 4 {
            break;
        }
    }
    (out, true)
}

// ------------------------------------------- number formatting

/// `value` with `decimals` decimals and its thousands grouped, as en-US
/// writes it: "43,020", "1,234,567.89".
pub fn fmt_grouped(value: f64, decimals: usize) -> String {
    let text = format!("{:.*}", decimals, value.abs());
    let (int, frac) = text.split_once('.').map_or((text.as_str(), None), |(i, f)| (i, Some(f)));
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if let Some(f) = frac {
        out.push('.');
        out.push_str(f);
    }
    if value < 0.0 && out.chars().any(|c| c.is_ascii_digit() && c != '0') {
        out.insert(0, '-');
    }
    out
}

// ------------------------------------------- Design A shell (#126): sidebar, header, status bar

/// The recording timer's text: hours, minutes, seconds and hundredths
/// ("01:02:03.45"). Hundredths are cut, not rounded.
pub fn recording_timer_text(seconds: f64) -> String {
    // The small nudge keeps 3723.45 (stored as 3723.4499…) at .45.
    let cs = (seconds.max(0.0) * 100.0 + 1e-6).floor() as u64;
    format!("{:02}:{:02}:{:02}.{:02}", cs / 360_000, (cs / 6_000) % 60, (cs / 100) % 60, cs % 100)
}

/// The status bar's recording timer: Michroma in the value blue (#53befa)
/// on the always-dark timer box, so it reads the same in both themes.
pub fn recording_timer(ui: &mut Ui, seconds: f64) -> egui::Response {
    let t = crate::theme::of_ui(ui);
    let face = t.fonts.figure_small.clone();
    // A bare context (as in tests) has no Michroma: use the body face there.
    let face = if ui.fonts(|f| f.definitions().families.contains_key(&face.family)) { face } else { egui::FontId::proportional(face.size) };
    let mut job = egui::text::LayoutJob::default();
    job.append(&recording_timer_text(seconds), 0.0, egui::TextFormat { font_id: face, color: t.graphs.value, extra_letter_spacing: 1.0, ..Default::default() });
    let galley = ui.painter().layout_job(job);
    // A box just around the digits, not the whole height of the bar.
    let (rect, resp) = ui.allocate_exact_size(galley.size() + vec2(16.0, 8.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(4), t.timer_bg);
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, t.graphs.value);
    resp
}

/// A sidebar heading ("Add-ons"): small faint words, then a thin rule to
/// the edge.
pub fn nav_heading(ui: &mut Ui, text: &str) {
    let t = crate::theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let y = r.top() + 16.0;
    let words = ui.painter().text(Pos2::new(r.left() + 12.0, y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(11.0), t.faint);
    ui.painter().line_segment([Pos2::new(words.right() + 8.0, y), Pos2::new(r.right() - 4.0, y)], Stroke::new(1.0, t.line));
}

/// The back of a sidebar entry: the selected fill for the open page, the
/// hover fill under the pointer. Returns the ink for its icon and name.
pub fn nav_entry_back(ui: &Ui, rect: Rect, selected: bool, hovered: bool) -> Color32 {
    let t = crate::theme::of_ui(ui);
    if selected {
        ui.painter().rect_filled(rect, CornerRadius::same(5), t.selected);
        t.on_selected
    } else {
        if hovered {
            ui.painter().rect_filled(rect, CornerRadius::same(5), t.hover);
        }
        t.text
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;

    #[test]
    fn the_recording_timer_reads_hours_minutes_seconds_and_hundredths() {
        assert_eq!(recording_timer_text(0.0), "00:00:00.00");
        assert_eq!(recording_timer_text(3723.45), "01:02:03.45");
        assert_eq!(recording_timer_text(59.999), "00:00:59.99");
    }
}

// ------------------------------------------- Design A notes, row buttons and tables

/// What a [`note`] tells: plain information, something to weigh, a problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Warm,
    Bad,
}

/// The fill and the text colour of a note in `tone`, from the theme's
/// neutral, warm and alert note tokens.
fn note_colors(t: &Theme, tone: Tone) -> (Color32, Color32) {
    match tone {
        Tone::Neutral => (t.neutral_note, t.neutral_note_text),
        Tone::Warm => (t.warm_note, t.warm_note_text),
        Tone::Bad => (t.alert_note, t.alert_note_text),
    }
}

/// Design A's note: a rounded box across the width in `tone`'s fill,
/// holding `add` (its text in the tone's colour). For a note of words
/// only see [`note_text`] (and [`parts::warm_note`], [`parts::neutral_note`]).
pub fn note<R>(ui: &mut Ui, tone: Tone, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (fill, ink) = note_colors(&theme::of_ui(ui), tone);
    egui::Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(12, 9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.visuals_mut().override_text_color = Some(ink);
            add(ui)
        })
        .inner
}

/// A [`note`] holding only `text`, wrapped.
pub fn note_text(ui: &mut Ui, tone: Tone, text: &str) {
    note(ui, tone, |ui| {
        ui.add(egui::Label::new(RichText::new(text).size(13.0)).wrap());
    });
}

/// A small button of a table row (24 points tall), outlined in the
/// theme's control line.
pub fn row_button(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let t = theme::of_ui(ui);
    let ink = if enabled { t.text } else { t.faint };
    let b = egui::Button::new(RichText::new(text).size(12.0).color(ink)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, t.control_line)).corner_radius(CornerRadius::same(4)).min_size(vec2(0.0, 24.0));
    ui.add_enabled(enabled, b)
}

/// A destructive [`row_button`]: red words in the destructive outline (the
/// row-sized [`parts::destructive_button`]).
pub fn row_danger_button(ui: &mut Ui, text: &str, enabled: bool) -> egui::Response {
    let t = theme::of_ui(ui);
    let ink = if enabled { t.red } else { t.faint };
    let b = egui::Button::new(RichText::new(text).size(12.0).color(ink)).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, t.destructive_line)).corner_radius(CornerRadius::same(4)).min_size(vec2(0.0, 24.0));
    ui.add_enabled(enabled, b)
}

/// The width of a column of a [`table_line`]: points, or a share of what
/// the fixed columns leave.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Col {
    Px(f32),
    Share(f32),
}

/// The space between two columns of a [`table_line`].
pub const TABLE_GAP: f32 = 12.0;

/// The space before the first and after the last column.
pub const TABLE_PAD: f32 = 12.0;

/// The widths of `cols` across `width`, [`TABLE_GAP`] apart inside
/// [`TABLE_PAD`] on both sides; shares never go below zero.
pub fn col_widths(width: f32, cols: &[Col]) -> Vec<f32> {
    let fixed: f32 = cols.iter().map(|c| if let Col::Px(w) = c { *w } else { 0.0 }).sum();
    let shares: f32 = cols.iter().map(|c| if let Col::Share(s) = c { *s } else { 0.0 }).sum();
    let gaps = TABLE_GAP * cols.len().saturating_sub(1) as f32;
    let left = (width - 2.0 * TABLE_PAD - fixed - gaps).max(0.0);
    cols.iter()
        .map(|c| match c {
            Col::Px(w) => *w,
            Col::Share(s) if shares > 0.0 => left * s / shares,
            Col::Share(_) => 0.0,
        })
        .collect()
}

/// Design A's table line across the width, `height` tall: a heading
/// (`header`, its rule in the line colour) or a row that can be clicked,
/// filled while `selected` or under the pointer ([`parts::row_fill`]).
/// `cell(i, ui)` draws column `i` (left to right, centred, clipped to the
/// column). Returns the line's response.
pub fn table_line(ui: &mut Ui, cols: &[Col], height: f32, header: bool, selected: bool, mut cell: impl FnMut(usize, &mut Ui)) -> egui::Response {
    let t = theme::of_ui(ui);
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), if header { Sense::hover() } else { Sense::click() });
    if !header {
        parts::row_fill(ui.painter(), rect, selected, resp.hovered());
    }
    let rule = if header { t.line } else { t.hover };
    ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, rule));
    let mut x = rect.left() + TABLE_PAD;
    for (i, w) in col_widths(width, cols).into_iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.top()), vec2(w, height));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(egui::Layout::left_to_right(egui::Align::Center)));
        child.set_clip_rect(r.intersect(ui.clip_rect()));
        if selected {
            child.visuals_mut().override_text_color = Some(t.on_selected);
        }
        cell(i, &mut child);
        x += w + TABLE_GAP;
    }
    resp
}

/// The title of a group of rows: its words, then its count in the label
/// colour.
pub fn group_title(ui: &mut Ui, title: &str, count: &str) {
    let t = theme::of_ui(ui);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(TABLE_PAD);
        ui.label(RichText::new(title).font(t.fonts.group_heading.clone()).color(t.text));
        ui.label(RichText::new(count).size(13.0).color(t.label));
    });
}

/// A program's output: monospaced lines in the theme's well, wrapped.
pub fn output_box(ui: &mut Ui, text: &str) {
    let t = theme::of_ui(ui);
    egui::Frame::new().fill(t.well).stroke(Stroke::new(1.0, t.well_line)).corner_radius(CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.add(egui::Label::new(RichText::new(text).monospace().size(11.5).color(t.soft)).wrap());
    });
}

/// Pairs of a key (in the label colour) and its value, a line each.
pub fn key_values(ui: &mut Ui, id: &str, rows: Vec<(&str, RichText)>) {
    let t = theme::of_ui(ui);
    egui::Grid::new(id).num_columns(2).min_col_width(96.0).spacing(vec2(12.0, 8.0)).show(ui, |ui| {
        for (key, value) in rows {
            ui.label(RichText::new(key).color(t.label));
            ui.add(egui::Label::new(value).wrap());
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod table_tests {
    use super::*;

    #[test]
    fn table_columns_share_what_the_fixed_ones_leave() {
        let cols = [Col::Px(16.0), Col::Share(2.0), Col::Share(1.0), Col::Px(80.0)];
        // 400 wide: 24 of padding, 36 of gaps, 96 fixed, 244 to share.
        let w = col_widths(400.0, &cols);
        assert_eq!(w[0], 16.0);
        assert!((w[1] - 244.0 * 2.0 / 3.0).abs() < 0.01 && (w[2] - 244.0 / 3.0).abs() < 0.01, "{w:?}");
        assert_eq!(w[3], 80.0);
        assert_eq!(col_widths(50.0, &cols)[1], 0.0, "too narrow: the shares are empty");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fills of the rectangles drawn by `draw` in one frame in `mode`.
    fn fills(mode: crate::theme::Mode, draw: impl FnOnce(&mut Ui)) -> Vec<Color32> {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, mode);
        let mut draw = Some(draw);
        let out = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| (draw.take().unwrap())(ui));
        });
        fn flat(s: &Shape, out: &mut Vec<Color32>) {
            match s {
                Shape::Vec(v) => v.iter().for_each(|s| flat(s, out)),
                Shape::Rect(r) => out.push(r.fill),
                _ => {}
            }
        }
        let mut all = Vec::new();
        out.shapes.iter().for_each(|c| flat(&c.shape, &mut all));
        all
    }

    #[test]
    fn a_graph_keeps_its_always_dark_surface_in_both_themes() {
        use crate::theme::{Mode, Theme};
        for mode in [Mode::Dark, Mode::Light] {
            let t = Theme::of(mode);
            let drawn = fills(mode, |ui| graph(ui, vec2(200.0, 80.0), &[10.0, 50.0, 30.0], 100.0, t.graphs.cpu));
            assert!(drawn.contains(&t.graph_bg), "{mode:?}: {drawn:?}");
        }
    }

    #[test]
    fn a_card_sits_on_the_panel_colour_of_its_theme() {
        use crate::theme::{Mode, Theme};
        for mode in [Mode::Dark, Mode::Light] {
            let t = Theme::of(mode);
            let drawn = fills(mode, |ui| card(ui, "CPU", "15%", t.graphs.cpu, 300.0, |ui| { ui.label("4.90 GHz"); }));
            assert!(drawn.contains(&t.panel), "{mode:?}: {drawn:?}");
        }
    }

    #[test]
    fn the_reading_tones_are_the_graph_palette_in_both_modes() {
        for dark in [true, false] {
            let t = crate::theme::Theme::of(crate::theme::Mode::from_dark(dark));
            let g = t.graphs;
            assert_eq!(hue(Hue::Orange, &t), g.thermal);
            assert_eq!(hue(Hue::Green, &t), g.utilisation);
            assert_eq!(hue(Hue::Red, &t), g.kernel);
            assert_eq!(hue(Hue::Yellow, &t), g.power);
            assert_eq!(hue(Hue::Violet, &t), g.memory);
            assert_eq!(hue(Hue::Teal, &t), g.disk_write);
            assert_eq!(hue(Hue::Blue, &t), g.cpu, "the CPU's colour, as on Summary and Performance");
            assert_eq!(hue(Hue::Grey, &t), t.label);
        }
    }

    #[test]
    fn long_paths_are_cut_in_the_middle() {
        let p = "C:\\Users\\me\\Desktop\\progetti\\UltimatePCManager\\src\\addons";
        assert_eq!(mid_truncate(p, 100), p);
        let cut = mid_truncate(p, 32);
        assert!(cut.starts_with("C:\\Users\\\u{2026}\\"), "{cut}");
        assert!(cut.ends_with("\\addons"), "{cut}");
        assert!(cut.chars().count() <= 32, "{cut}");
        assert_eq!(mid_truncate("C:\\averyveryverylongname.iso", 12).chars().count(), 12);
    }

    #[test]
    fn dates_read_well() {
        assert_eq!(long_date("2026-07-22"), "22 July 2026");
        assert_eq!(long_date("not a date"), "not a date");
        assert_eq!(short_time("17:52:36"), "17:52");
    }

    #[test]
    fn version_split_marks_the_changed_part() {
        assert_eq!(split_version("1.0.4.7057", "1.0.4.55794"), ("1.0.4.".into(), "7057".into()));
        assert_eq!(split_version("4.22.1.0", "4.20.2.0"), ("4.".into(), "22.1.0".into()));
        assert_eq!(split_version("2.0", "1.0"), ("".into(), "2.0".into()));
    }

    #[test]
    fn a_number_is_grouped_by_thousands_with_its_decimals_as_in_en_us() {
        assert_eq!(fmt_grouped(43020.0, 0), "43,020");
        assert_eq!(fmt_grouped(1234567.891, 2), "1,234,567.89");
        assert_eq!(fmt_grouped(152.8, 1), "152.8");
        assert_eq!(fmt_grouped(999.6, 0), "1,000", "rounded before grouping");
        assert_eq!(fmt_grouped(-4500.0, 0), "-4,500");
        assert_eq!(fmt_grouped(0.0, 2), "0.00");
    }
}
