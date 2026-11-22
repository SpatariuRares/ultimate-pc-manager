//! Colors, in Design A (board P17): the graph colours, each one's built-in
//! and saved value, and the CPU graph in the saved colour on live data.
//! Colours come only from the theme.

use egui::{vec2, RichText, Sense, Ui};

use crate::addons::Core;
use crate::core_page::{page_title, CorePage, PageCtx};
use crate::platform::Action;
use crate::settings::{GraphColor, Settings};
use crate::theme;
use crate::widgets::parts::{neutral_note, outline_button, primary_button, BandArea, Mark, SidePanel, StatusBand};
use crate::widgets::{graph, rgb};

/// Width of the side panel beside the palette.
const PANEL: f32 = 340.0;
/// Room between the palette and its side panel.
const GAP: f32 = 16.0;

/// Colors keeps nothing of its own: the colours are settings, and the
/// preview graph reads the app's CPU history.
#[derive(Default)]
pub struct ColorsPage;

impl CorePage for ColorsPage {
    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>) {
        // Edits go to a copy; the app is asked to keep it.
        let mut s = ctx.settings.clone();
        let t = theme::of_ui(ui);
        ui.horizontal(|ui| {
            ui.set_min_height(52.0);
            page_title(ui, Core::Colors.id(), "Colors");
        });
        ui.add_space(6.0);
        let all_original = GraphColor::ALL.iter().all(|c| c.is_original(&s));
        let (mark, headline) = if all_original {
            (Mark::Tick, "Your graph colours are the default palette")
        } else {
            (Mark::Dot(t.amber), "Your graph colours differ from the default palette")
        };
        let mut everywhere = false;
        StatusBand::new(mark, headline)
            .sub("Every device has one fixed default colour. \u{201c}Use default\u{201d} puts a graph's back.")
            .actions_width(300.0)
            .show(ui, |ui, area| {
                if area == BandArea::Actions {
                    everywhere = primary_button(ui, "Use the default palette everywhere", !all_original).clicked();
                }
            });
        ui.add_space(14.0);
        let height = ui.available_height();
        ui.horizontal_top(|ui| {
            let list_w = (ui.available_width() - PANEL - GAP).max(420.0);
            ui.vertical(|ui| {
                ui.set_width(list_w);
                palette_list(ui, &mut s);
            });
            ui.add_space(GAP);
            ui.vertical(|ui| {
                ui.set_width(PANEL);
                ui.set_height(height);
                cpu_graph_panel(ui, &mut s, ctx);
            });
        });
        if everywhere {
            for c in GraphColor::ALL {
                c.use_original(&mut s);
            }
        }
        if s != *ctx.settings {
            actions.push(Action::ChangeSettings(Box::new(s)));
            ui.ctx().request_repaint();
        }
    }
}

/// The palette: per graph, the built-in colour, the colour saved
/// here (with its picker) and "Use default".
fn palette_list(ui: &mut Ui, s: &mut Settings) {
    let t = theme::of_ui(ui);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        ui.label(RichText::new("The default palette").font(egui::FontId::new(14.0, theme::semibold())).color(t.text));
    });
    ui.add_space(4.0);
    let swatch = |ui: &mut Ui, c: [u8; 3]| {
        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        ui.painter().rect_filled(r, 4.0, rgb(c));
        ui.painter().rect_stroke(r, 4.0, egui::Stroke::new(1.0, t.line), egui::StrokeKind::Inside);
    };
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    let grid = egui::Grid::new("colors").num_columns(4).striped(false).spacing(vec2(24.0, 12.0)).min_col_width(130.0);
    let top = ui.cursor().top();
    grid.show(ui, |ui| {
        for head in ["Graph", "Default", "Saved here now", ""] {
            ui.label(RichText::new(head).font(t.fonts.label.clone()).color(t.label));
        }
        ui.end_row();
        for c in GraphColor::ALL {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                ui.label(RichText::new(c.name()).font(t.fonts.row.clone()).color(t.text));
                if !c.sub().is_empty() {
                    ui.label(RichText::new(c.sub()).size(11.5).color(t.faint));
                }
            });
            ui.horizontal(|ui| {
                swatch(ui, c.original());
                ui.label(RichText::new(hex(c.original())).color(t.soft));
            });
            let original = c.is_original(s);
            ui.horizontal(|ui| {
                ui.color_edit_button_srgb(c.get_mut(s)).on_hover_text("Pick your own colour");
                let saved = c.get(s);
                if original {
                    ui.label(RichText::new("same").color(t.faint));
                } else {
                    ui.label(RichText::new(hex(saved)).color(t.amber));
                }
            });
            if outline_button(ui, "Use default", !original).clicked() {
                c.use_original(s);
            }
            ui.end_row();
        }
    });
    // The line under the headings.
    let rule = top + 24.0;
    let r = ui.min_rect();
    ui.painter().hline(r.left()..=r.right(), rule, egui::Stroke::new(1.0, t.line));
    ui.add_space(10.0);
    neutral_note(ui, "You can pick your own colour for a graph from its row.");
}

/// The side panel: the CPU graph drawn in the colour saved here.
fn cpu_graph_panel(ui: &mut Ui, s: &mut Settings, ctx: &PageCtx) {
    let t = theme::of_ui(ui);
    let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
    let saved = GraphColor::Cpu.get(s);
    let original = GraphColor::Cpu.original();
    let data: Vec<f32> = ctx.histories.cpu.iter().copied().collect();
    let mut reset = false;
    SidePanel::new("colors-cpu").show(ui, Some(()), |ui, ()| {
        ui.label(RichText::new("CPU graph").font(t.fonts.figure.clone()).color(t.text));
        ui.label(RichText::new("In the colour saved here").font(t.fonts.label.clone()).color(t.label));
        let w = ui.available_width();
        // Graphs stay dark in both modes, as on Performance.
        graph(ui, vec2(w, 120.0), &data, 100.0, rgb(saved));
        let caption = if saved == original { format!("Saved here: {} (the default)", hex(saved)) } else { format!("Saved here: {} \u{b7} default {}", hex(saved), hex(original)) };
        ui.label(RichText::new(caption).font(t.fonts.label.clone()).color(t.label));
        ui.add_space(6.0);
        reset = outline_button(ui, &format!("Use default {}", hex(original)), saved != original).clicked();
    });
    if reset {
        GraphColor::Cpu.use_original(s);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::core_page::testing::{click, draw, draw_with};
    use crate::core_page::{Histories, PerfHistory};
    use crate::sampler::Snapshot;
    use crate::test_ui::{context, flat_shapes, run};
    use crate::theme::Mode;

    /// The points of every polyline drawn, with its colour.
    fn lines(out: &egui::FullOutput) -> Vec<(usize, egui::Color32)> {
        flat_shapes(out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Path(p) => match p.stroke.color {
                    egui::epaint::ColorMode::Solid(c) => Some((p.points.len(), c)),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    /// One frame of Colors with `settings` and the CPU history `cpu`.
    fn frame(settings: &Settings, cpu: &VecDeque<f32>) -> egui::FullOutput {
        let (snapshot, mem) = (Snapshot::default(), VecDeque::new());
        let perf = PerfHistory::new();
        let page_ctx = PageCtx { settings, snapshot: &snapshot, histories: Histories { cpu, mem: &mem, perf: &perf } };
        run(&context(Mode::Dark), (1400.0, 900.0), vec![], |ui| ColorsPage.draw(ui, &page_ctx, &mut Vec::new()))
    }

    #[test]
    fn colors_draws_the_palette_and_says_when_it_is_the_default() {
        let drawn = draw(&mut ColorsPage, &Settings::default(), &Snapshot::default());
        for w in ["Colors", "Your graph colours are the default palette", "The default palette", "CPU graph", "In the colour saved here"] {
            assert!(drawn.texts.iter().any(|t| t == w), "{w:?} in {:?}", drawn.texts);
        }
        assert!(drawn.actions.is_empty());
        let mut changed = Settings::default();
        GraphColor::Cpu.get_mut(&mut changed)[0] ^= 0x40;
        let drawn = draw(&mut ColorsPage, &changed, &Snapshot::default());
        assert!(drawn.texts.iter().any(|t| t == "Your graph colours differ from the default palette"), "{:?}", drawn.texts);
    }

    #[test]
    fn the_preview_graph_draws_the_cpu_history_it_is_lent() {
        let settings = Settings::default();
        let three: VecDeque<f32> = [10.0, 50.0, 90.0].into();
        let five: VecDeque<f32> = [10.0, 20.0, 30.0, 40.0, 50.0].into();
        let saved = rgb(GraphColor::Cpu.get(&settings));
        assert!(lines(&frame(&settings, &three)).contains(&(3, saved)), "three samples in the saved colour");
        let after = lines(&frame(&settings, &five));
        assert!(after.contains(&(5, saved)) && !after.iter().any(|l| l.0 == 3), "the next frame shows the new history: {after:?}");
        assert!(!lines(&frame(&settings, &VecDeque::new())).iter().any(|l| l.1 == saved), "no history, no line");
    }

    #[test]
    fn the_preview_graph_is_drawn_in_the_colour_saved_here() {
        let mut settings = Settings::default();
        *GraphColor::Cpu.get_mut(&mut settings) = [10, 20, 30];
        let cpu: VecDeque<f32> = [10.0, 50.0].into();
        assert!(lines(&frame(&settings, &cpu)).contains(&(2, egui::Color32::from_rgb(10, 20, 30))));
    }

    #[test]
    fn use_the_default_palette_everywhere_asks_the_app_for_the_original_colours() {
        let mut settings = Settings::default();
        *GraphColor::Cpu.get_mut(&mut settings) = [1, 2, 3];
        let mut expected = settings.clone();
        for c in GraphColor::ALL {
            c.use_original(&mut expected);
        }
        let asked = click(&mut ColorsPage, &settings, &Snapshot::default(), "Use the default palette everywhere");
        assert_eq!(asked, [Action::ChangeSettings(Box::new(expected))]);
    }

    #[test]
    fn use_default_on_the_cpu_graph_asks_the_app_to_put_its_colour_back() {
        let mut settings = Settings::default();
        *GraphColor::Cpu.get_mut(&mut settings) = [1, 2, 3];
        let mut expected = settings.clone();
        GraphColor::Cpu.use_original(&mut expected);
        let [r, g, b] = GraphColor::Cpu.original();
        let asked = click(&mut ColorsPage, &settings, &Snapshot::default(), &format!("Use default #{r:02x}{g:02x}{b:02x}"));
        assert_eq!(asked, [Action::ChangeSettings(Box::new(expected))]);
    }

    #[test]
    fn a_frame_with_nothing_changed_queues_nothing() {
        let cpu: VecDeque<f32> = [5.0, 6.0].into();
        let drawn = draw_with(&mut ColorsPage, &Settings::default(), &Snapshot::default(), &cpu, &VecDeque::new());
        assert!(drawn.actions.is_empty());
    }
}
