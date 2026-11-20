//! Helpers for tests that draw egui frames without a window.

use crate::theme::{self, Mode};
use egui::{Event, Pos2, RawInput, Rect, Shape, Ui};

/// A context drawing in `mode` with the app's faces.
pub fn context(mode: Mode) -> egui::Context {
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::apply(&ctx, mode);
    ctx
}

/// One frame of a `width` x `height` page with `events`.
pub fn run(ctx: &egui::Context, (width, height): (f32, f32), events: Vec<Event>, mut draw: impl FnMut(&mut Ui)) -> egui::FullOutput {
    let input = RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(width, height))), events, ..Default::default() };
    ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| draw(ui));
    })
}

/// Every shape drawn, nested ones flattened.
pub fn flat_shapes(out: &egui::FullOutput) -> Vec<Shape> {
    fn flat(s: &Shape, out: &mut Vec<Shape>) {
        match s {
            Shape::Vec(v) => v.iter().for_each(|s| flat(s, out)),
            s => out.push(s.clone()),
        }
    }
    let mut all = Vec::new();
    out.shapes.iter().for_each(|c| flat(&c.shape, &mut all));
    all
}

/// Every text drawn, in drawing order.
pub fn texts(out: &egui::FullOutput) -> Vec<String> {
    flat_shapes(out)
        .iter()
        .filter_map(|s| match s {
            Shape::Text(t) => Some(t.galley.text().to_string()),
            _ => None,
        })
        .collect()
}

/// Every colour drawn: fills, outlines, lines, words and mesh vertices.
pub fn colours(out: &egui::FullOutput) -> Vec<egui::Color32> {
    let mut all = Vec::new();
    for s in flat_shapes(out) {
        match s {
            Shape::Rect(r) => all.extend([r.fill, r.stroke.color]),
            Shape::Circle(c) => all.extend([c.fill, c.stroke.color]),
            Shape::LineSegment { stroke, .. } => all.push(stroke.color),
            Shape::Path(p) => {
                all.push(p.fill);
                if let egui::epaint::ColorMode::Solid(c) = p.stroke.color {
                    all.push(c);
                }
            }
            Shape::Text(t) => all.extend(t.galley.job.sections.iter().map(|s| s.format.color)),
            Shape::Mesh(m) => all.extend(m.vertices.iter().map(|v| v.color)),
            _ => {}
        }
    }
    all
}
