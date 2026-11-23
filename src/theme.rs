//! The app's theme, from Design A: every colour, font role and size the pages
//! use, for the dark or the light mode. Pages take colours from here and
//! never write them by hand (a test guards this).
//!
//! The surfaces, lines and text are those of the Design A boards
//! `Main.dc.html` and `P17-Colors.dc.html`; the light ones keep the same roles
//! and meanings, picked for contrast. The accents (main action, selection,
//! links) are the brand's phosphor green (assets/brand/palette.md). Surfaces Design A keeps dark (graph backgrounds, the
//! Disk Space canvas, the LED bar, the timer) are the same in both modes.

use egui::{Color32, FontFamily, FontId};

/// Dark or light.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Dark,
    Light,
}

impl Mode {
    pub fn from_dark(dark: bool) -> Mode {
        if dark { Mode::Dark } else { Mode::Light }
    }
}

/// The graph colours: the built-in palette, fixed and equal in both modes
/// (graphs are drawn on an always-dark background).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphPalette {
    /// CPU graph frame.
    pub cpu: Color32,
    /// CPU utilisation line.
    pub utilisation: Color32,
    /// Kernel time.
    pub kernel: Color32,
    pub memory: Color32,
    /// Disk read / active time.
    pub disk: Color32,
    pub disk_write: Color32,
    pub network: Color32,
    pub gpu: Color32,
    /// CPU power and energy.
    pub power: Color32,
    /// Temperatures.
    pub thermal: Color32,
    pub npu: Color32,
    /// Figures and values next to graphs.
    pub value: Color32,
    /// Network sent, under the received line.
    pub network_sent: Color32,
    /// The frame of a device's small graph in Performance's list.
    pub frame: Color32,
    /// Grid lines over a graph (white, mostly transparent).
    pub grid: Color32,
    /// The arrow at a line's newest point.
    pub marker: Color32,
    /// Behind a grid of per-processor graphs and the small graphs.
    pub cells: Color32,
    /// A selected device's small graph.
    pub selected: Color32,
    /// The dark base a big graph's background is tinted from.
    pub tint_base: Color32,
    /// The CPU LED meter: green, yellow, red along its length.
    pub meter_low: Color32,
    pub meter_mid: Color32,
    pub meter_high: Color32,
}

/// The text roles and their faces and sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct FontRoles {
    /// Michroma 26.
    pub page_title: FontId,
    /// Michroma 15, 18 and 22: figures and versions.
    pub figure_small: FontId,
    pub figure: FontId,
    pub figure_large: FontId,
    /// Segoe UI Semibold 20: the status band's headline.
    pub band_title: FontId,
    /// Segoe UI 13: rows and text.
    pub row: FontId,
    /// Segoe UI 12: labels and sub-lines.
    pub label: FontId,
    /// Segoe UI Semibold 13.5: a group of settings' heading.
    pub group_heading: FontId,
    /// Segoe UI Semibold 13: a part of a side panel, under a rule.
    pub panel_heading: FontId,
    /// Michroma 14: a side panel section's title.
    pub section_title: FontId,
}

/// Every Design A token for one mode, by role.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub mode: Mode,
    /// Page background.
    pub page: Color32,
    /// Navigation (sidebar) background.
    pub sidebar: Color32,
    /// Status band background.
    pub band: Color32,
    /// Side panel and cards.
    pub panel: Color32,
    /// Borders and separators.
    pub line: Color32,
    /// Border of outlined buttons and inputs.
    pub control_line: Color32,
    /// A hovered row.
    pub hover: Color32,
    /// The selection: row, scope, page.
    pub selected: Color32,
    /// Text on the selection.
    pub on_selected: Color32,
    /// A selected row's explaining line.
    pub selected_sub: Color32,
    /// The main action's fill.
    pub primary: Color32,
    /// Text on the main action.
    pub on_primary: Color32,
    /// Names and figures.
    pub text: Color32,
    /// Secondary text.
    pub soft: Color32,
    /// Labels.
    pub label: Color32,
    /// Faint text: hints, placeholders, sub-lines of unselected rows.
    pub faint: Color32,
    /// Fine, up to date.
    pub green: Color32,
    /// A decision is waiting.
    pub amber: Color32,
    /// A problem or a destructive action.
    pub red: Color32,
    /// Border of destructive buttons.
    pub destructive_line: Color32,
    /// Links and values.
    pub link: Color32,
    pub link_hover: Color32,
    /// Warm note (something to decide) and its text.
    pub warm_note: Color32,
    pub warm_note_text: Color32,
    /// Neutral note (what Windows can't tell) and its text.
    pub neutral_note: Color32,
    pub neutral_note_text: Color32,
    /// The Connections globe's wireframe and the routes drawn on it.
    pub globe_wire: Color32,
    pub globe_route: Color32,
    /// The alert note (something is wrong with every result) and its words;
    /// its border is `destructive_line`.
    pub alert_note: Color32,
    pub alert_note_text: Color32,
    /// A chart's or a cell's well, and its edge (Flight Recorder).
    pub well: Color32,
    pub well_line: Color32,
    /// Memory's words and tags outside graphs (the graph purple, darker in
    /// the light mode for contrast).
    pub memory_ink: Color32,
    /// A replay: its words and its edge.
    pub replay: Color32,
    pub replay_line: Color32,
    /// Record's dot (the same in both modes: it sits on the primary green)
    /// and the fill behind Stop while recording.
    pub record: Color32,
    pub record_fill: Color32,
    /// Always dark: graph background.
    pub graph_bg: Color32,
    /// Always dark: the Disk Space canvas and its border.
    pub canvas_bg: Color32,
    pub canvas_line: Color32,
    /// Always dark: the LED bar's housing and its border.
    pub led_bg: Color32,
    pub led_line: Color32,
    /// Always dark: behind the recording timer.
    pub timer_bg: Color32,
    pub graphs: GraphPalette,
    pub fonts: FontRoles,
}

/// The theme in use: the one whose mode the context's visuals have (set by
/// [`apply`]).
pub fn current(ctx: &egui::Context) -> Theme {
    Theme::of(Mode::from_dark(ctx.style().visuals.dark_mode))
}

/// The theme in use for a `Ui`.
pub fn of_ui(ui: &egui::Ui) -> Theme {
    Theme::of(Mode::from_dark(ui.visuals().dark_mode))
}

/// `--theme dark|light`: a development option that picks the theme for this
/// run (e.g. for `--screenshot`) without changing the saved setting. Absent
/// or unknown: `None`, the saved theme.
pub fn mode_from_args(args: &[String]) -> Option<Mode> {
    let value = args.iter().position(|a| a == "--theme").and_then(|i| args.get(i + 1))?;
    match value.to_ascii_lowercase().as_str() {
        "dark" => Some(Mode::Dark),
        "light" => Some(Mode::Light),
        _ => None,
    }
}

/// Sets egui's visuals (fills, lines, selection, links, corners) and
/// spacing from the theme of this mode.
pub fn apply(ctx: &egui::Context, mode: Mode) {
    ctx.set_visuals(visuals(&Theme::of(mode)));
    ctx.style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        s.spacing.button_padding = egui::vec2(10.0, 4.0);
    });
}

/// egui's visuals from the tokens. The control fills (button, hover, press)
/// and the table stripes are not Design A tokens; they are set here.
fn visuals(t: &Theme) -> egui::Visuals {
    use egui::{CornerRadius, Stroke};
    let dark = t.mode == Mode::Dark;
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    let (input, stripe, control, control_hover, control_active, hovered_line) = if dark {
        (hex(0x161616), hex(0x252525), hex(0x2b2b2b), hex(0x363636), hex(0x404040), hex(0x606060))
    } else {
        (hex(0xffffff), hex(0xececec), hex(0xe6e6e6), hex(0xdcdcdc), hex(0xd0d0d0), hex(0x6a6a6a))
    };
    v.panel_fill = t.page;
    v.window_fill = t.panel;
    v.extreme_bg_color = input;
    v.faint_bg_color = stripe;
    v.striped = true;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.line);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, t.text);
    v.widgets.inactive.weak_bg_fill = control;
    v.widgets.inactive.bg_fill = control;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, t.control_line);
    v.widgets.hovered.weak_bg_fill = control_hover;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, hovered_line);
    v.widgets.active.weak_bg_fill = control_active;
    for w in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
        w.corner_radius = CornerRadius::same(3);
    }
    v.selection.bg_fill = t.selected;
    v.selection.stroke = Stroke::new(1.0, t.on_selected);
    v.hyperlink_color = t.link;
    v
}

/// Installs the theme's faces: on Windows Segoe UI for text and Segoe UI
/// Semibold for band headlines, both read from the system (not bundled); on
/// Linux and macOS the bundled Selawik (SIL Open Font License, see assets/),
/// which has Segoe UI's metrics, so labels take the same space everywhere.
/// Michroma, a wide display face (same licence), draws titles and figures.
///
/// Fallback: without the semibold file the band face is the body face
/// (headlines lose their weight, not their size); without the body file every
/// face uses egui's default fonts after its own first choice.
pub fn install_fonts(ctx: &egui::Context) {
    let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
    let fonts = std::path::Path::new(&windir).join("Fonts");
    ctx.set_fonts(installed_fonts(|file| std::fs::read(fonts.join(file)).ok()));
}

/// The font set for this system: elsewhere than Windows the bundled
/// Selawik, on Windows Segoe UI read through `read`.
fn installed_fonts(read: impl Fn(&str) -> Option<Vec<u8>>) -> egui::FontDefinitions {
    match bundled_body() {
        Some((regular, semibold)) => font_definitions_named("selawik", Some(regular), Some(semibold)),
        None => font_definitions(read("segoeui.ttf"), read("seguisb.ttf")),
    }
}

/// The bundled body faces (regular, semibold); only off Windows, so the
/// Windows binary does not carry them.
#[cfg(not(windows))]
fn bundled_body() -> Option<(Vec<u8>, Vec<u8>)> {
    Some((
        include_bytes!("../assets/Selawik-Regular.ttf").to_vec(),
        include_bytes!("../assets/Selawik-Semibold.ttf").to_vec(),
    ))
}

#[cfg(windows)]
fn bundled_body() -> Option<(Vec<u8>, Vec<u8>)> {
    None
}

/// The font set from the Segoe UI and Segoe UI Semibold files, if found.
fn font_definitions(segoe: Option<Vec<u8>>, segoe_semibold: Option<Vec<u8>>) -> egui::FontDefinitions {
    font_definitions_named("segoe", segoe, segoe_semibold)
}

/// The font set from a body face and its semibold (named `<name>` and
/// `<name>-semibold`), if given.
fn font_definitions_named(name: &str, body: Option<Vec<u8>>, semibold_file: Option<Vec<u8>>) -> egui::FontDefinitions {
    use std::sync::Arc;
    let mut fonts = egui::FontDefinitions::default();
    if let Some(bytes) = body {
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(bytes)));
        if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
            f.insert(0, name.into());
        }
    }
    let proportional = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut band = Vec::new();
    if let Some(bytes) = semibold_file {
        let semi = format!("{name}-semibold");
        fonts.font_data.insert(semi.clone(), Arc::new(egui::FontData::from_owned(bytes)));
        band.push(semi);
    }
    band.extend(proportional.iter().cloned());
    fonts.families.insert(semibold(), band);
    fonts
        .font_data
        .insert("michroma".into(), Arc::new(egui::FontData::from_static(include_bytes!("../assets/Michroma-Regular.ttf"))));
    let mut face = vec!["michroma".to_string()];
    face.extend(proportional);
    fonts.families.insert(display(), face);
    fonts
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The face of page titles and figures (Michroma).
pub fn display() -> FontFamily {
    FontFamily::Name("display".into())
}

/// The face of band headlines (Segoe UI Semibold, or Selawik Semibold off
/// Windows; the body face if missing).
pub fn semibold() -> FontFamily {
    FontFamily::Name("semibold".into())
}

/// Shading laid over coloured pieces (Disk Space's balls and blocks), the
/// same in both modes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shading {
    /// A ball's highlight (white, 22 %).
    pub highlight: Color32,
    /// A ball's darker rim (black, 28 %).
    pub rim_shadow: Color32,
    /// The outline of a ball (black, 50 %).
    pub ball_line: Color32,
    /// The outline of a block (black, 35 %).
    pub block_line: Color32,
    /// The dark edge around words on a piece (black, 45 %).
    pub halo: Color32,
    /// The drop shadow under the selection chip on the canvas (black, 35 %).
    pub chip_shadow: Color32,
}

/// The tint that draws an image as it is (the page icons carry their own
/// colours, one set per theme).
pub const UNTINTED: Color32 = Color32::WHITE;

pub const SHADING: Shading = Shading {
    highlight: Color32::from_rgba_premultiplied(56, 56, 56, 56),
    rim_shadow: Color32::from_black_alpha(72),
    ball_line: Color32::from_black_alpha(128),
    block_line: Color32::from_black_alpha(90),
    halo: Color32::from_black_alpha(115),
    chip_shadow: Color32::from_black_alpha(90),
};

/// The graph palette, also usable in constants (it is the same in both
/// modes): `Theme::of(mode).graphs` is this.
pub const GRAPHS: GraphPalette = GraphPalette {
    cpu: hex(0x31d1ff),
    utilisation: hex(0x64d245),
    kernel: hex(0xff7070),
    memory: hex(0xbe69ff),
    disk: hex(0x36d366),
    disk_write: hex(0x63e6e2),
    network: hex(0x39b1ff),
    gpu: hex(0x39b1ff),
    power: hex(0xffd60a),
    thermal: hex(0xff9a3c),
    npu: hex(0xff5a5a),
    value: hex(0x53befa),
    network_sent: hex(0xa8dcff),
    frame: hex(0xb8b8b8),
    grid: Color32::from_rgba_unmultiplied_const(0xff, 0xff, 0xff, 30),
    marker: hex(0xf4f4f4),
    cells: hex(0x1c1c1c),
    selected: hex(0x1d5a35),
    tint_base: hex(0x1f1f1f),
    meter_low: hex(0x4cd23c),
    meter_mid: hex(0xf0d020),
    meter_high: hex(0xff483c),
};

impl Theme {
    pub fn of(mode: Mode) -> Theme {
        let graphs = GRAPHS;
        let fonts = FontRoles {
            page_title: FontId::new(26.0, display()),
            figure_small: FontId::new(15.0, display()),
            figure: FontId::new(18.0, display()),
            figure_large: FontId::new(22.0, display()),
            band_title: FontId::new(20.0, semibold()),
            row: FontId::proportional(13.0),
            label: FontId::proportional(12.0),
            group_heading: FontId::new(13.5, semibold()),
            panel_heading: FontId::new(13.0, semibold()),
            section_title: FontId::new(14.0, display()),
        };
        // Always dark, whatever the mode.
        let graph_bg = hex(0x141414);
        let canvas_bg = hex(0x0b0d13);
        let canvas_line = hex(0x1f2330);
        let led_bg = hex(0x090d0a);
        let led_line = hex(0x1c2a20);
        let timer_bg = hex(0x101010);
        match mode {
            Mode::Dark => Theme {
                mode,
                page: hex(0x202020),
                sidebar: hex(0x1a1a1a),
                band: hex(0x262628),
                panel: hex(0x1c1c1c),
                line: hex(0x3a3a3e),
                control_line: hex(0x454545),
                hover: hex(0x2a2a2a),
                selected: hex(0x1c4a2e),
                on_selected: hex(0xf2f2f2),
                selected_sub: hex(0xa8e8bf),
                primary: hex(0x52ff8c),
                on_primary: hex(0x07120c),
                text: hex(0xf2f2f2),
                soft: hex(0xc8c8c8),
                label: hex(0xa6a6a6),
                faint: hex(0x8f8f8f),
                green: hex(0x4cd26a),
                amber: hex(0xf0b42c),
                red: hex(0xff8a8a),
                destructive_line: hex(0x6a3a3a),
                link: hex(0x52ff8c),
                link_hover: hex(0xa2ffaa),
                warm_note: hex(0x2a2418),
                warm_note_text: hex(0xf2e2bd),
                neutral_note: hex(0x242424),
                neutral_note_text: hex(0xc8c8c8),
                globe_wire: hex(0x2a7f86),
                globe_route: hex(0x6d5bd0),
                alert_note: hex(0x3a1f1f),
                alert_note_text: hex(0xffb3b3),
                well: hex(0x181818),
                well_line: hex(0x2c2c2c),
                memory_ink: hex(0xbe69ff),
                replay: hex(0x8fb0ff),
                replay_line: hex(0x435fbc),
                record: hex(0xff5c5c),
                record_fill: hex(0xff6b6b),
                graph_bg,
                canvas_bg,
                canvas_line,
                led_bg,
                led_line,
                timer_bg,
                graphs,
                fonts,
            },
            Mode::Light => Theme {
                mode,
                page: hex(0xf3f3f3),
                sidebar: hex(0xe8e8e8),
                band: hex(0xebebed),
                panel: hex(0xfafafa),
                line: hex(0xd0d0d4),
                control_line: hex(0x8a8a8a),
                hover: hex(0xe2e2e2),
                selected: hex(0xc3e8cf),
                on_selected: hex(0x111111),
                selected_sub: hex(0x0b4f2a),
                primary: hex(0x08713a),
                on_primary: hex(0xf1faf3),
                text: hex(0x1a1a1a),
                soft: hex(0x3d3d3d),
                label: hex(0x505050),
                faint: hex(0x5f5f5f),
                green: hex(0x1b6e2f),
                amber: hex(0x825400),
                red: hex(0xb42323),
                destructive_line: hex(0xd49a9a),
                link: hex(0x08713a),
                link_hover: hex(0x055a2d),
                warm_note: hex(0xfbf0d9),
                warm_note_text: hex(0x4f3b00),
                neutral_note: hex(0xebebeb),
                neutral_note_text: hex(0x3d3d3d),
                globe_wire: hex(0x2a7f86),
                globe_route: hex(0x5b48c2),
                alert_note: hex(0xfde8e8),
                alert_note_text: hex(0xb42323),
                well: hex(0xffffff),
                well_line: hex(0xe0e2e6),
                memory_ink: hex(0x8a3fc4),
                replay: hex(0x2f55c8),
                replay_line: hex(0xa9bbef),
                record: hex(0xff5c5c),
                record_fill: hex(0xb33a30),
                graph_bg,
                canvas_bg,
                canvas_line,
                led_bg,
                led_line,
                timer_bg,
                graphs,
                fonts,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn css(c: Color32) -> String {
        format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
    }

    #[test]
    fn the_dark_tokens_are_the_design_a_values() {
        let t = Theme::of(Mode::Dark);
        let expected = [
            ("page", t.page, "#202020"),
            ("sidebar", t.sidebar, "#1a1a1a"),
            ("band", t.band, "#262628"),
            ("panel", t.panel, "#1c1c1c"),
            ("line", t.line, "#3a3a3e"),
            ("control line", t.control_line, "#454545"),
            ("hover", t.hover, "#2a2a2a"),
            ("selected", t.selected, "#1c4a2e"),
            ("selected sub-line", t.selected_sub, "#a8e8bf"),
            ("primary", t.primary, "#52ff8c"),
            ("on primary", t.on_primary, "#07120c"),
            ("text", t.text, "#f2f2f2"),
            ("soft", t.soft, "#c8c8c8"),
            ("label", t.label, "#a6a6a6"),
            ("faint", t.faint, "#8f8f8f"),
            ("green", t.green, "#4cd26a"),
            ("amber", t.amber, "#f0b42c"),
            ("red", t.red, "#ff8a8a"),
            ("destructive line", t.destructive_line, "#6a3a3a"),
            ("link", t.link, "#52ff8c"),
            ("link hover", t.link_hover, "#a2ffaa"),
            ("warm note", t.warm_note, "#2a2418"),
            ("warm note text", t.warm_note_text, "#f2e2bd"),
            ("neutral note", t.neutral_note, "#242424"),
            ("neutral note text", t.neutral_note_text, "#c8c8c8"),
            ("alert note", t.alert_note, "#3a1f1f"),
            ("alert note text", t.alert_note_text, "#ffb3b3"),
            ("graph background", t.graph_bg, "#141414"),
            ("timer background", t.timer_bg, "#101010"),
        ];
        for (name, got, want) in expected {
            assert_eq!(css(got), want, "{name}");
        }
    }

    #[test]
    fn the_graph_palette_is_the_built_in_one() {
        for mode in [Mode::Dark, Mode::Light] {
            let g = Theme::of(mode).graphs;
            let expected = [
                ("cpu", g.cpu, "#31d1ff"),
                ("utilisation", g.utilisation, "#64d245"),
                ("kernel", g.kernel, "#ff7070"),
                ("memory", g.memory, "#be69ff"),
                ("disk", g.disk, "#36d366"),
                ("disk write", g.disk_write, "#63e6e2"),
                ("network", g.network, "#39b1ff"),
                ("gpu", g.gpu, "#39b1ff"),
                ("power", g.power, "#ffd60a"),
                ("thermal", g.thermal, "#ff9a3c"),
                ("npu", g.npu, "#ff5a5a"),
                ("value", g.value, "#53befa"),
                ("network sent", g.network_sent, "#a8dcff"),
                ("small graph frame", g.frame, "#b8b8b8"),
                ("newest-point marker", g.marker, "#f4f4f4"),
                ("graph cells", g.cells, "#1c1c1c"),
                ("selected small graph", g.selected, "#1d5a35"),
                ("tint base", g.tint_base, "#1f1f1f"),
                ("meter low", g.meter_low, "#4cd23c"),
                ("meter middle", g.meter_mid, "#f0d020"),
                ("meter high", g.meter_high, "#ff483c"),
            ];
            for (name, got, want) in expected {
                assert_eq!(css(got), want, "{name} in {mode:?}");
            }
            assert_eq!(g.grid, Color32::from_rgba_unmultiplied(0xff, 0xff, 0xff, 30), "grid lines in {mode:?}");
        }
    }

    #[test]
    fn the_connections_globe_has_the_boards_wire_and_route() {
        let (d, l) = (Theme::of(Mode::Dark), Theme::of(Mode::Light));
        assert_eq!((css(d.globe_wire), css(d.globe_route)), ("#2a7f86".to_string(), "#6d5bd0".to_string()));
        assert_eq!((css(l.globe_wire), css(l.globe_route)), ("#2a7f86".to_string(), "#5b48c2".to_string()));
    }

    #[test]
    fn the_font_roles_have_the_design_a_faces_and_sizes() {
        let f = Theme::of(Mode::Dark).fonts;
        assert_eq!(f.page_title, FontId::new(26.0, display()));
        assert_eq!(f.figure_small, FontId::new(15.0, display()));
        assert_eq!(f.figure, FontId::new(18.0, display()));
        assert_eq!(f.figure_large, FontId::new(22.0, display()));
        assert_eq!(f.band_title, FontId::new(20.0, semibold()));
        assert_eq!(f.row, FontId::proportional(13.0));
        assert_eq!(f.label, FontId::proportional(12.0));
        assert_eq!(Theme::of(Mode::Light).fonts, f, "fonts do not change with the mode");
    }

    /// WCAG contrast ratio of two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        fn lum(c: Color32) -> f32 {
            let ch = |v: u8| {
                let v = v as f32 / 255.0;
                if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
        }
        let (x, y) = (lum(a) + 0.05, lum(b) + 0.05);
        x.max(y) / x.min(y)
    }

    #[test]
    fn the_contrast_check_matches_known_values() {
        assert!((contrast(Color32::BLACK, Color32::WHITE) - 21.0).abs() < 0.01);
        assert!((contrast(Color32::from_rgb(0x77, 0x77, 0x77), Color32::WHITE) - 4.48).abs() < 0.01);
    }

    #[test]
    fn every_light_text_is_readable_on_its_background() {
        let t = Theme::of(Mode::Light);
        let grounds = [("page", t.page), ("sidebar", t.sidebar), ("band", t.band), ("panel", t.panel), ("hover", t.hover)];
        let texts = [
            ("text", t.text),
            ("soft", t.soft),
            ("label", t.label),
            ("faint", t.faint),
            ("green", t.green),
            ("amber", t.amber),
            ("red", t.red),
            ("link", t.link),
            ("link hover", t.link_hover),
            ("memory ink", t.memory_ink),
            ("replay", t.replay),
        ];
        let mut pairs: Vec<(String, Color32, Color32, f32)> = Vec::new();
        for (g, bg) in grounds {
            for (n, fg) in texts {
                pairs.push((format!("{n} on {g}"), fg, bg, 4.5));
            }
        }
        pairs.push(("text on selected".into(), t.on_selected, t.selected, 4.5));
        pairs.push(("sub-line on selected".into(), t.selected_sub, t.selected, 4.5));
        pairs.push(("text on primary".into(), t.on_primary, t.primary, 4.5));
        pairs.push(("warm note".into(), t.warm_note_text, t.warm_note, 4.5));
        pairs.push(("neutral note".into(), t.neutral_note_text, t.neutral_note, 4.5));
        pairs.push(("alert note".into(), t.alert_note_text, t.alert_note, 4.5));
        pairs.push(("red outline text".into(), t.red, t.page, 4.5));
        pairs.push(("Stop on the recording fill".into(), t.on_primary, t.record_fill, 4.5));
        pairs.push(("alert figure on panel".into(), t.amber, t.panel, 4.5));
        pairs.push(("alert words on panel".into(), t.alert_note_text, t.panel, 4.5));
        // Large text (page titles, band headlines, big figures): 3:1.
        pairs.push(("page title".into(), t.text, t.page, 3.0));
        pairs.push(("band title".into(), t.text, t.band, 3.0));
        for (name, fg, bg, min) in pairs {
            let c = contrast(fg, bg);
            assert!(c >= min, "{name}: {c:.2} < {min}");
        }
    }

    #[test]
    fn every_dark_token_has_its_own_light_variant() {
        let (d, l) = (Theme::of(Mode::Dark), Theme::of(Mode::Light));
        // Equal in both modes on purpose.
        let shared = [d.graph_bg, d.canvas_bg, d.canvas_line, d.led_bg, d.led_line, d.timer_bg, d.globe_wire, d.record];
        let pairs = [
            (d.primary, l.primary), (d.on_primary, l.on_primary), (d.page, l.page), (d.sidebar, l.sidebar), (d.band, l.band), (d.panel, l.panel), (d.line, l.line),
            (d.control_line, l.control_line), (d.hover, l.hover), (d.selected, l.selected), (d.on_selected, l.on_selected),
            (d.selected_sub, l.selected_sub), (d.text, l.text), (d.soft, l.soft), (d.label, l.label), (d.faint, l.faint),
            (d.green, l.green), (d.amber, l.amber), (d.red, l.red), (d.destructive_line, l.destructive_line),
            (d.link, l.link), (d.link_hover, l.link_hover), (d.warm_note, l.warm_note), (d.warm_note_text, l.warm_note_text),
            (d.neutral_note, l.neutral_note), (d.neutral_note_text, l.neutral_note_text),
            (d.alert_note, l.alert_note), (d.alert_note_text, l.alert_note_text), (d.globe_route, l.globe_route),
            (d.well, l.well), (d.well_line, l.well_line), (d.memory_ink, l.memory_ink), (d.replay, l.replay),
            (d.replay_line, l.replay_line), (d.record_fill, l.record_fill),
        ];
        for (dark, light) in pairs {
            assert!(!shared.contains(&dark));
            assert_ne!(dark, light, "{} has no light variant", css(dark));
        }
        assert_eq!((l.mode, d.mode), (Mode::Light, Mode::Dark));
    }

    #[test]
    fn the_always_dark_surfaces_are_equal_in_both_modes() {
        let (d, l) = (Theme::of(Mode::Dark), Theme::of(Mode::Light));
        assert_eq!(
            (d.graph_bg, d.canvas_bg, d.canvas_line, d.led_bg, d.led_line, d.timer_bg),
            (l.graph_bg, l.canvas_bg, l.canvas_line, l.led_bg, l.led_line, l.timer_bg)
        );
        assert_eq!(d.graphs, l.graphs);
        assert_eq!((d.globe_wire, d.record), (l.globe_wire, l.record));
        assert_eq!(css(l.primary), "#08713a", "the light main action is the brand's dark green");
    }

    #[test]
    fn applying_a_theme_sets_egui_visuals_from_its_tokens() {
        for mode in [Mode::Dark, Mode::Light] {
            let ctx = egui::Context::default();
            apply(&ctx, mode);
            let t = Theme::of(mode);
            let v = ctx.style().visuals.clone();
            assert_eq!(v.dark_mode, mode == Mode::Dark);
            assert_eq!(v.panel_fill, t.page);
            assert_eq!(v.window_fill, t.panel);
            assert_eq!(v.selection.bg_fill, t.selected, "selection is the theme's, not the accent's");
            assert_eq!(v.selection.stroke.color, t.on_selected);
            assert_eq!(v.hyperlink_color, t.link);
            assert_eq!(v.widgets.noninteractive.bg_stroke.color, t.line);
            assert_eq!(v.widgets.inactive.bg_stroke.color, t.control_line);
            assert_eq!(v.widgets.noninteractive.fg_stroke.color, t.text);
            assert_eq!(current(&ctx), t, "the current theme is read back from the context");
        }
        assert_eq!(css(Theme::of(Mode::Dark).selected), "#1c4a2e");
    }

    #[test]
    fn a_context_without_a_theme_reads_its_visuals_mode() {
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        assert_eq!(current(&ctx).mode, Mode::Light);
        ctx.set_visuals(egui::Visuals::dark());
        assert_eq!(current(&ctx).mode, Mode::Dark);
    }

    fn family(f: &egui::FontDefinitions, family: FontFamily) -> Vec<String> {
        f.families.get(&family).cloned().unwrap_or_default()
    }

    #[test]
    fn the_band_face_is_segoe_semibold_when_windows_has_it() {
        let f = font_definitions(Some(vec![1]), Some(vec![2]));
        assert_eq!(family(&f, semibold())[0], "segoe-semibold");
        assert_eq!(family(&f, FontFamily::Proportional)[0], "segoe");
        assert_eq!(family(&f, display())[0], "michroma");
    }

    #[test]
    fn without_segoe_semibold_the_band_face_falls_back_to_segoe() {
        let f = font_definitions(Some(vec![1]), None);
        assert!(!f.font_data.contains_key("segoe-semibold"));
        assert_eq!(family(&f, semibold()), family(&f, FontFamily::Proportional));
        assert_eq!(family(&f, semibold())[0], "segoe");
    }

    #[test]
    fn without_segoe_every_face_falls_back_to_egui_defaults() {
        let f = font_definitions(None, None);
        let proportional = family(&f, FontFamily::Proportional);
        assert!(!proportional.is_empty() && !proportional.contains(&"segoe".to_string()));
        assert_eq!(family(&f, semibold()), proportional);
        assert_eq!(family(&f, display())[0], "michroma");
    }

    #[test]
    fn selawik_is_the_body_face_off_windows_and_segoe_on_windows() {
        let f = installed_fonts(|_| Some(vec![1]));
        let (body, semi) = if cfg!(windows) { ("segoe", "segoe-semibold") } else { ("selawik", "selawik-semibold") };
        assert_eq!(family(&f, FontFamily::Proportional)[0], body);
        assert_eq!(family(&f, semibold())[0], semi);
        assert_eq!(family(&f, display())[0], "michroma");
    }

    #[test]
    fn the_selawik_face_has_a_regular_and_a_semibold_file() {
        let f = font_definitions_named("selawik", Some(vec![1]), Some(vec![2]));
        assert_eq!(family(&f, FontFamily::Proportional)[0], "selawik");
        assert_eq!(family(&f, semibold())[0], "selawik-semibold");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_bundled_selawik_files_are_truetype_fonts() {
        let (regular, semibold) = bundled_body().expect("bundled off Windows");
        for bytes in [regular, semibold] {
            assert_eq!(&bytes[..4], &[0, 1, 0, 0], "a TrueType file starts with version 1.0");
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn the_theme_option_picks_the_mode_for_this_run() {
        assert_eq!(mode_from_args(&args(&["app", "--page", "Drivers", "--theme", "light"])), Some(Mode::Light));
        assert_eq!(mode_from_args(&args(&["app", "--theme", "Dark", "--screenshot", "x.png"])), Some(Mode::Dark));
        assert_eq!(mode_from_args(&args(&["app", "--screenshot", "x.png"])), None, "absent: the saved theme");
        assert_eq!(mode_from_args(&args(&["app", "--theme", "blue"])), None, "unknown: the saved theme");
        assert_eq!(mode_from_args(&args(&["app", "--theme"])), None);
    }

    /// Files that still write colours by hand, to align with the theme. The
    /// list shrinks with each Design A ticket and ends empty.
    const TO_ALIGN: &[&str] = &[];

    /// Whether this code builds a colour from literal values
    /// (`Color32::from_rgb(0x20, ..)`, `Color32::from_gray(160)`,
    /// `Color32::WHITE`...). `TRANSPARENT` is not a colour.
    fn writes_a_colour(code: &str) -> bool {
        let constructors =
            ["from_rgb(", "from_rgba_unmultiplied(", "from_rgba_premultiplied(", "from_gray(", "from_hex(", "from_white_alpha(", "from_black_alpha(", "from_additive_luminance("];
        code.match_indices("Color32::").any(|(at, m)| {
            let rest = &code[at + m.len()..];
            if let Some(c) = constructors.iter().find(|c| rest.starts_with(**c)) {
                let arg = rest[c.len()..].trim_start();
                return arg.starts_with(|ch: char| ch.is_ascii_digit() || ch == '"');
            }
            let name: String = rest.chars().take_while(|ch| ch.is_ascii_uppercase() || *ch == '_').collect();
            !name.is_empty() && name != "TRANSPARENT" && name != "PLACEHOLDER"
        })
    }

    fn sources(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn the_guard_sees_literal_colours_and_nothing_else() {
        assert!(writes_a_colour("let c = Color32::from_rgb(0x20, 0x20, 0x20);"));
        assert!(writes_a_colour("Color32::from_gray(
    160)"));
        assert!(writes_a_colour("fill(Color32::WHITE)"));
        assert!(writes_a_colour("Color32::from_rgba_unmultiplied(0, 0, 0, 90)"));
        assert!(writes_a_colour("Color32::from_white_alpha(56)"));
        assert!(writes_a_colour("Color32::from_black_alpha(72)"));
        assert!(writes_a_colour("Color32::from_additive_luminance(40)"));
        assert!(!writes_a_colour("Color32::from_black_alpha(alpha)"));
        assert!(!writes_a_colour("Color32::from_rgb(c[0], c[1], c[2])"));
        assert!(!writes_a_colour("Color32::TRANSPARENT"));
        assert!(!writes_a_colour("fn f(c: Color32) -> Color32 { t.text }"));
    }

    #[test]
    fn colours_come_from_the_theme_outside_the_files_still_to_align() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        sources(&root, &mut files);
        let mut writing: Vec<String> = files
            .iter()
            .filter_map(|path| {
                let name = path.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                if name == "theme.rs" || path.file_name().is_some_and(|f| f == "tests.rs") {
                    return None;
                }
                let text = std::fs::read_to_string(path).unwrap();
                let code = text.split("#[cfg(test)]").next().unwrap();
                writes_a_colour(code).then_some(name)
            })
            .collect();
        writing.sort();
        let mut listed: Vec<String> = TO_ALIGN.iter().map(|s| s.to_string()).collect();
        listed.sort();
        let new: Vec<&String> = writing.iter().filter(|f| !listed.contains(f)).collect();
        let done: Vec<&String> = listed.iter().filter(|f| !writing.contains(f)).collect();
        assert!(new.is_empty(), "these files write colours by hand; take them from crate::theme: {new:?}");
        assert!(done.is_empty(), "these files are aligned; remove them from TO_ALIGN: {done:?}");
    }
}
