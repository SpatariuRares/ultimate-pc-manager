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

/// Every text `draw` shows in one frame of a page, a line each, and the actions it asked for.
pub fn drawn_text<R>(mut draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> (String, Vec<crate::platform::Action>) {
    let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
    let ctx = egui::Context::default();
    // The app's faces, as at start-up: pages draw in them.
    theme::install_fonts(&ctx);
    let mut actions = Vec::new();
    let output = ctx.run(RawInput::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut page = crate::platform::PageContext::new(&settings, &sample);
            draw(ui, &mut page);
            actions = std::mem::take(&mut page.actions);
        });
    });
    (texts(&output).join("\n"), actions)
}

/// Every text `draw` shows in one frame of a page, a line each.
pub fn page_text<R>(draw: impl FnMut(&mut Ui, &mut crate::platform::PageContext) -> R) -> String {
    drawn_text(draw).0
}

/// Settings whose CPU colour is the user's own, and that colour.
pub fn custom_cpu() -> (crate::settings::Settings, egui::Color32) {
    let chosen = [0x12, 0x34, 0x56];
    let mut settings = crate::settings::Settings::default();
    settings.cpu_color = chosen;
    (settings, crate::widgets::rgb(chosen))
}
