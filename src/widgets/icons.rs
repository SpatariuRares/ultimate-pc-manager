//! The app's picture icons, built from the same kit of pieces as the logo
//! (sockets, modules, bricks and studs): 96 px PNGs in a dark and a light
//! set, decoded once per context and drawn with mipmaps so they stay clean
//! at 14-18 px.
//! - Page icons (`assets/icons`): the sidebar, the page titles and the
//!   Settings and Colors buttons.
//! - Glyphs (`assets/glyphs`): sensors, devices, actions and marks inside
//!   the pages (search fields, Refresh, the UAC shield, ticks...).

use egui::{pos2, Color32, Context, Id, Painter, Pos2, Rect, TextureFilter, TextureHandle, TextureOptions, Vec2};

/// Side of a page icon, in points.
pub const SIZE: f32 = 18.0;

macro_rules! icon_set {
    ($dir:literal: $($id:literal),* $(,)?) => {
        &[$((
            $id,
            include_bytes!(concat!("../../assets/", $dir, "/dark/", $id, ".png")) as &[u8],
            include_bytes!(concat!("../../assets/", $dir, "/light/", $id, ".png")) as &[u8],
        )),*]
    };
}

/// Each page id with its dark-theme and light-theme PNG.
pub const ICONS: &[(&str, &[u8], &[u8])] = icon_set!["icons":
    "summary", "performance", "processes", "system-info", "app-history", "startup-apps", "users", "services",
    "power-freq", "flight-recorder", "connections", "installed-apps", "drivers", "disk-space", "benchmarks",
    "package-manager", "settings", "colors",
];

/// Each glyph name with its dark-theme and light-theme PNG.
pub const GLYPHS: &[(&str, &[u8], &[u8])] = icon_set!["glyphs":
    "thermometer", "gauge", "clock", "bolt", "wave", "sliders", "shield-check", "trend", "info", "fan",
    "monitor", "chip", "graphics-card", "hexagon", "motherboard", "memory-stick", "drive", "battery", "network",
    "search", "up", "refresh", "admin-shield", "internet", "program", "record", "stop", "done",
    // Small marks shown beside words (10-14 px).
    "chevron-down", "chevron-right", "sort-up", "sort-down", "plus", "minus", "arrow-right", "warning",
];

/// The PNG named `id` in `set` for the dark or light theme, if it has one.
fn find(set: &[(&str, &'static [u8], &'static [u8])], id: &str, dark: bool) -> Option<&'static [u8]> {
    set.iter().find(|(i, _, _)| *i == id).map(|(_, d, l)| if dark { *d } else { *l })
}

/// The PNG of page `id` for the dark or light theme, if it has one.
pub fn png(id: &str, dark: bool) -> Option<&'static [u8]> {
    find(ICONS, id, dark)
}

/// The PNG of glyph `name` for the dark or light theme, if it has one.
pub fn glyph_png(name: &str, dark: bool) -> Option<&'static [u8]> {
    find(GLYPHS, name, dark)
}

/// Decodes an icon's PNG into an egui image.
pub fn decode(bytes: &[u8]) -> Option<egui::ColorImage> {
    let rgba = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// The texture of `bytes` under `name`, loaded on first use.
fn texture(ctx: &Context, name: &str, bytes: &[u8]) -> Option<TextureHandle> {
    let key = Id::new(("icon-texture", name));
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return Some(tex);
    }
    let options = TextureOptions { mipmap_mode: Some(TextureFilter::Linear), ..TextureOptions::LINEAR };
    let tex = ctx.load_texture(name, decode(bytes)?, options);
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    Some(tex)
}

fn paint(painter: &Painter, tex: &TextureHandle, center: Pos2, size: f32, tint: Color32) {
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    painter.image(tex.id(), Rect::from_center_size(center, Vec2::splat(size)), uv, tint);
}

/// Draws page `id`'s icon centred on `center`, in the set of the current
/// theme. Returns false (drawing nothing) when the page has no icon file.
pub fn draw(painter: &Painter, center: Pos2, id: &str) -> bool {
    let dark = painter.ctx().style().visuals.dark_mode;
    let Some(bytes) = png(id, dark) else { return false };
    let Some(tex) = texture(painter.ctx(), &format!("page-icon-{id}-{dark}"), bytes) else { return false };
    paint(painter, &tex, center, SIZE, crate::theme::UNTINTED);
    true
}

/// How a glyph is coloured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Ink {
    /// Its own grey and green, in the set of the current theme.
    Theme,
    /// The same, at half strength (a disabled action).
    Faded,
    /// On a filled background (a primary button): the light set multiplied
    /// by `colour`, so it keeps its two tones in the button's ink.
    On(Color32),
}

/// Draws glyph `name` centred on `center`, `size` points wide. Returns
/// false (drawing nothing) for a name with no file.
pub fn draw_glyph(painter: &Painter, center: Pos2, size: f32, name: &str, ink: Ink) -> bool {
    let theme_dark = painter.ctx().style().visuals.dark_mode;
    // On a filled background the dark set's light pieces take the tint best.
    let dark = matches!(ink, Ink::On(_)) || theme_dark;
    let Some(bytes) = glyph_png(name, dark) else { return false };
    let Some(tex) = texture(painter.ctx(), &format!("glyph-{name}-{dark}"), bytes) else { return false };
    let tint = match ink {
        Ink::Theme => crate::theme::UNTINTED,
        Ink::Faded => crate::theme::UNTINTED.gamma_multiply(0.5),
        Ink::On(colour) => colour,
    };
    paint(painter, &tex, center, size, tint);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui::{context, flat_shapes, run};
    use crate::theme::Mode;

    /// The rectangles and tints of the textured meshes drawn in `out`.
    fn images(out: &egui::FullOutput) -> Vec<(Rect, Color32)> {
        flat_shapes(out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => Some((m.calc_bounds(), m.vertices[0].color)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn every_page_has_a_square_icon_in_both_themes() {
        let mut ids: Vec<&str> = crate::addons::registry().iter().map(|e| e.id()).filter(|id| *id != "fake").collect();
        ids.extend(["settings", "colors"]);
        for id in ids {
            for dark in [true, false] {
                let image = png(id, dark).and_then(decode).unwrap_or_else(|| panic!("{id} has a {} icon", if dark { "dark" } else { "light" }));
                assert_eq!(image.size, [96, 96], "{id}");
            }
        }
    }

    #[test]
    fn every_glyph_has_a_square_png_in_both_themes() {
        assert_eq!(GLYPHS.len(), 36);
        for (name, _, _) in GLYPHS {
            for dark in [true, false] {
                let image = glyph_png(name, dark).and_then(decode).unwrap_or_else(|| panic!("{name} ({dark})"));
                assert_eq!(image.size, [96, 96], "{name}");
            }
        }
    }

    #[test]
    fn an_icon_is_drawn_as_one_image_of_the_icon_size_and_a_page_without_one_draws_nothing() {
        let ctx = context(Mode::Dark);
        let (mut known, mut unknown) = (false, true);
        let out = run(&ctx, (200.0, 200.0), vec![], |ui| {
            known = draw(ui.painter(), pos2(50.0, 50.0), "drivers");
            unknown = draw(ui.painter(), pos2(100.0, 100.0), "no-such-page");
        });
        assert!(known && !unknown);
        let rects: Vec<Rect> = images(&out).into_iter().map(|(r, _)| r).collect();
        assert_eq!(rects, [Rect::from_center_size(pos2(50.0, 50.0), Vec2::splat(SIZE))]);
    }

    #[test]
    fn a_glyph_is_drawn_at_its_size_in_its_ink_and_an_unknown_name_draws_nothing() {
        let ctx = context(Mode::Dark);
        let on = Color32::from_rgb(10, 20, 30);
        let mut drawn = Vec::new();
        let out = run(&ctx, (200.0, 200.0), vec![], |ui| {
            drawn = vec![
                draw_glyph(ui.painter(), pos2(20.0, 20.0), 16.0, "search", Ink::Theme),
                draw_glyph(ui.painter(), pos2(60.0, 20.0), 14.0, "done", Ink::Faded),
                draw_glyph(ui.painter(), pos2(100.0, 20.0), 15.0, "admin-shield", Ink::On(on)),
                draw_glyph(ui.painter(), pos2(140.0, 20.0), 15.0, "no-such-glyph", Ink::Theme),
            ];
        });
        assert_eq!(drawn, [true, true, true, false]);
        let got = images(&out);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], (Rect::from_center_size(pos2(20.0, 20.0), Vec2::splat(16.0)), crate::theme::UNTINTED));
        assert_eq!(got[1].1, crate::theme::UNTINTED.gamma_multiply(0.5));
        assert_eq!(got[2].1, on);
    }
}
