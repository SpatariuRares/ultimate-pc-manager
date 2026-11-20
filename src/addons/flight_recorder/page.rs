//! The Flight Recorder page in Design A (#40, styled on the theme #135):
//! the band with LIVE · Record · Replay…, the RECORDING or REPLAY card, the
//! panel grid and the "Recordings" side panel. What it shows comes from
//! [`super::layout`].

use egui::{vec2, Align, Color32, Layout, RichText, Sense, Ui};

use super::layout::{self, Line, Mode, PanelView, Slot};
use super::recorder::{LIMIT_REACHED, SAFE_CAPTURE_LIMIT};
use super::thermals;
use super::replay::{CLOSE_REPLAY_FIRST, NOTHING_LIVE, TOO_MANY_SAMPLES};
use super::trace::{default_file_name, format_elapsed, samples_label, TRACE_EXTENSION};
use super::{FlightRecorder, CANCEL, DISCARD, NOTHING_TO_EXPORT, PAUSED, SAVE_FAILED, UNSAVED};
use crate::platform::{Action, PageContext};
use crate::theme::{self, GraphPalette};
use crate::widgets::parts::{
    self, cell_grid, destructive_button, line_chart, link_outline_button, marked_button, outline_button, outlined_card, outlined_tag, primary_button,
    ruled_heading, telemetry_panel, warm_note_with, BandArea, ButtonMark, ChartLine, LineStyle, StatusBand,
};
use crate::widgets::{meter_with_tick, recording_timer};

/// The side panel's width.
const SIDE_WIDTH: f32 = 330.0;

/// The room the band keeps on its right for LIVE, Record and Replay….
const CONTROLS_WIDTH: f32 = 300.0;

/// How often the RECORDING card's timer is drawn again, so its hundredths
/// run as the status bar timer's.
const TIMER_REPAINT: std::time::Duration = std::time::Duration::from_millis(50);

/// The side panel's title.
pub const RECORDINGS: &str = "Recordings";

/// Under "Saving" while no trace waits (the approved prototype's text).
pub const SAVE_HINT: &str = "Stop opens Windows' Save dialog on a dated name. Cancel it and the trace waits here as unsaved.";

/// Under the panel grid: what the three lines mean (the prototype's text).
pub const LEGEND: &str = "Solid line: load %. Dashed: effective clock (cores) or GPU clock. Dotted red: temperature, where the sensor exists. Efficiency cores appears as a seventh panel only on hybrid processors.";

/// The trace format, as the side panel names it.
pub fn format_line() -> String {
    format!("Flight Recorder trace (.{TRACE_EXTENSION}): one JSON file, header plus samples from 00:00")
}

/// Under "Opening a trace": Replay… checks the file first.
pub fn opening_hint() -> String {
    format!("Replay\u{2026} checks the file before it plays and refuses a broken one with the reason, e.g. \"{TOO_MANY_SAMPLES}\"")
}

impl FlightRecorder {
    /// The page, top to bottom: band, mode card, then the panel grid with
    /// the "Recordings" panel on its right.
    pub(super) fn page(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.band(ui, page);
        ui.add_space(14.0);
        if self.mode() != Mode::Live {
            self.mode_card(ui);
            ui.add_space(14.0);
        }
        let main = (ui.available_width() - SIDE_WIDTH - 16.0).max(320.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(main);
                egui::ScrollArea::vertical().id_salt("flight-recorder-grid").auto_shrink([false, false]).show(ui, |ui| {
                    self.grid(ui, page);
                });
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.set_width(SIDE_WIDTH);
                self.recordings(ui, page);
            });
        });
    }

    /// The band: the machine's name, the hardware line and what Record
    /// does on the left; LIVE, Record (Stop while recording) and Replay…
    /// on the right; the pause note under them.
    fn band(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let t = theme::of_ui(ui);
        let (performance, efficiency, gpu) = match &self.replay {
            Some(replay) => layout::header_counts(&replay.trace().header),
            None => {
                let (performance, efficiency) = layout::live_counts(&self.history, page.sample);
                (performance, efficiency, self.history.engines().map_or(0, |e| e.cells.len()))
            }
        };
        let heading = layout::machine_heading(self.replay.as_ref()).to_owned();
        let hardware = layout::hardware_line(performance, efficiency, gpu);
        let paused = page.settings.interval_ms() == 0 && !self.is_recording();
        StatusBand::new(parts::Mark::None, &heading).sub(&hardware).actions_width(CONTROLS_WIDTH).show(ui, |ui, area| match area {
            BandArea::Body => {
                ui.add(egui::Label::new(RichText::new(layout::EXPLANATION).size(12.5).color(t.soft)).wrap());
            }
            BandArea::Actions => {
                ui.horizontal(|ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        self.replay_button(ui, page);
                        self.record_or_stop(ui, page);
                        if self.mode().is_live() {
                            outlined_tag(ui, Mode::Live.word(), t.green, true);
                        }
                    });
                });
            }
            BandArea::Below => {
                if paused {
                    ui.label(RichText::new(PAUSED).size(12.0).color(t.warm_note_text));
                }
            }
        });
    }

    /// Record (off while updates are paused), or Stop while recording.
    fn record_or_stop(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let t = theme::of_ui(ui);
        if self.is_recording() {
            let stop = marked_button(ui, "Stop", ButtonMark::Stop, t.record_fill, Some(t.destructive_line), true);
            if stop.on_hover_text("Stop recording").clicked() {
                if let Some(trace) = self.finish() {
                    self.offer_save(trace, &mut page.actions);
                }
            }
            return;
        }
        let interval = page.settings.interval_ms();
        let blocked = self.record_blocked(interval);
        let record = marked_button(ui, "Record", ButtonMark::Record, t.primary, None, blocked.is_none());
        if record.on_hover_text("Start recording").on_disabled_hover_text(blocked.unwrap_or_default()).clicked() {
            match self.record(interval, page.sample) {
                Err(CLOSE_REPLAY_FIRST) => self.replay_note = Some(CLOSE_REPLAY_FIRST),
                Err(e) => page.actions.push(Action::Say(e.into())),
                Ok(()) => {}
            }
        }
    }

    /// Replay… (off during a recording, saying why).
    fn replay_button(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let blocked = self.replay_blocked();
        let button = outline_button(ui, "Replay\u{2026}", blocked.is_none());
        if button.on_disabled_hover_text(blocked.unwrap_or_default()).clicked() {
            self.open_replay(&mut page.actions);
        }
    }

    /// What the page shows now.
    pub(super) fn mode(&self) -> Mode {
        Mode::of(self.is_recording(), self.is_replaying())
    }

    /// The mode card: RECORDING with the status bar's timer, the samples
    /// and the share of the safe capture limit used, or REPLAY.
    fn mode_card(&mut self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        if self.mode() == Mode::Replay {
            outlined_card(ui, t.panel, t.replay_line, |ui| self.replay_card(ui));
            return;
        }
        let Some(rec) = &self.recorder else { return };
        let status = rec.status();
        let seconds = rec.elapsed_ms() as f64 / 1000.0;
        outlined_card(ui, t.panel, t.destructive_line, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.horizontal(|ui| {
                outlined_tag(ui, Mode::Recording.word(), t.red, false);
                recording_timer(ui, seconds);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(samples_label(status.samples)).size(12.0).color(t.label));
                });
            });
            meter_with_tick(ui, ui.available_width(), status.limit_fraction, None, t.red);
            let note = if status.limit_reached {
                LIMIT_REACHED.to_owned()
            } else {
                format!("Share of the safe capture limit used ({}).", samples_label(SAFE_CAPTURE_LIMIT))
            };
            ui.label(RichText::new(note).size(11.5).color(t.label));
        });
        // The hundredths run: draw again soon.
        ui.ctx().request_repaint_after(TIMER_REPAINT);
    }

    /// The REPLAY card: the trace's machine and date, Close Replay, the
    /// "Replay position" bar with the start, current and total times, the
    /// note that nothing is live and, after Record, why it was refused.
    fn replay_card(&mut self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        let Some(replay) = &self.replay else { return };
        let total = replay.duration_ms();
        let recorded = replay.recorded_label();
        let machine = replay.machine().to_owned();
        let mut close = false;
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.horizontal(|ui| {
            outlined_tag(ui, Mode::Replay.word(), t.replay, false);
            ui.label(RichText::new(machine).color(t.text));
            ui.label(RichText::new(recorded).size(12.0).color(t.label));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                close = outline_button(ui, "Close Replay", true).clicked();
            });
        });
        ui.label(RichText::new("Replay position").size(11.5).color(t.label));
        ui.spacing_mut().slider_width = ui.available_width();
        ui.add(egui::Slider::new(&mut self.position_ms, 0..=total).show_value(false)).on_hover_text("Replay position");
        ui.columns(3, |c| {
            c[0].label(RichText::new(format_elapsed(0)).size(11.0).color(t.faint));
            c[1].vertical_centered(|ui| ui.label(RichText::new(format_elapsed(self.position_ms)).size(11.0).color(t.text)));
            c[2].with_layout(Layout::right_to_left(Align::Min), |ui| ui.label(RichText::new(format_elapsed(total)).size(11.0).color(t.faint)));
        });
        ui.label(RichText::new(NOTHING_LIVE).size(11.5).color(t.warm_note_text));
        if let Some(note) = self.replay_note {
            ui.label(RichText::new(note).size(11.5).color(t.red));
        }
        if close {
            self.close_replay();
        }
    }

    /// The panel grid, two a row: live, or the trace at the replay
    /// position; then what the lines mean.
    fn grid(&mut self, ui: &mut Ui, page: &PageContext) {
        let fahrenheit = page.settings.fahrenheit;
        let panels = match &self.replay {
            Some(replay) => layout::replay_panels(replay, self.position_ms, fahrenheit),
            None => layout::live_panels(&self.history, page.sample, fahrenheit),
        };
        let gap = 12.0;
        let w = ((ui.available_width() - gap) / 2.0).max(240.0);
        for pair in panels.chunks(2) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for panel in pair {
                    panel_card(ui, panel, w);
                }
            });
            ui.add_space(gap);
        }
        parts::neutral_note(ui, LEGEND);
    }

    /// The "Recordings" side panel: the trace format, name, folder and
    /// limit; saving (the trace waiting, or why it failed); opening a trace
    /// (Replay…) and "Export as CSV".
    fn recordings(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let t = theme::of_ui(ui);
        parts::panel(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            parts::section_title(ui, RECORDINGS);
            let folder = if self.prefs.trace_folder.is_empty() {
                "Documents (Settings \u{203a} Add-ons)".to_owned()
            } else {
                self.prefs.trace_folder().to_string_lossy().into_owned()
            };
            let values = [
                (format_line(), t.text),
                (default_file_name(crate::pro::local_secs()), t.text),
                (folder, t.text),
                (layout::limit_line(SAFE_CAPTURE_LIMIT), t.soft),
            ];
            ui.scope(|ui| {
                parts::facts(ui, &["Format", "Name", "Folder", "Limit"], |ui, i| {
                    let (words, ink) = &values[i];
                    ui.add(egui::Label::new(RichText::new(words.as_str()).color(*ink)).wrap());
                });
            });

            ruled_heading(ui, "Saving");
            if let Some(reason) = self.save_failure().map(str::to_owned) {
                self.failure_note(ui, &reason);
            } else if let Some(trace) = &self.unsaved {
                let line = format!("{}  \u{00b7}  {}", samples_label(trace.samples.len()), format_elapsed(trace.samples.last().map_or(0, |s| s.t_ms)));
                let (mut save, mut discard) = (false, false);
                outlined_card(ui, t.band, t.line, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.label(RichText::new(UNSAVED).font(egui::FontId::new(13.0, theme::semibold())).color(t.text));
                    ui.label(RichText::new(line).size(12.0).color(t.label));
                    ui.horizontal(|ui| {
                        save = primary_button(ui, "Save\u{2026}", true).clicked();
                        discard = destructive_button(ui, DISCARD, true).clicked();
                    });
                });
                if save {
                    if let Some(trace) = self.unsaved.take() {
                        self.offer_save(trace, &mut page.actions);
                    }
                } else if discard {
                    self.discard();
                }
            } else {
                ui.add(egui::Label::new(RichText::new(SAVE_HINT).size(12.0).color(t.faint)).wrap());
            }

            ruled_heading(ui, "Opening a trace");
            ui.add(egui::Label::new(RichText::new(opening_hint()).size(12.0).color(t.label)).wrap());
            self.replay_button(ui, page);
            if let Some(error) = &self.replay_error {
                ui.add(egui::Label::new(RichText::new(error).size(12.0).color(t.red)).wrap());
            }

            ruled_heading(ui, "Export");
            let export = link_outline_button(ui, "Export as CSV", self.exportable().is_some());
            if export.on_hover_text("Export the trace as CSV").on_disabled_hover_text(NOTHING_TO_EXPORT).clicked() {
                self.export_csv(&mut page.actions);
            }
        });
    }

    /// The failed-save question under "Saving": the reason, the question,
    /// Discard and Cancel.
    fn failure_note(&mut self, ui: &mut Ui, reason: &str) {
        let t = theme::of_ui(ui);
        let mut discard = None;
        warm_note_with(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.add(egui::Label::new(RichText::new(reason).size(12.0).color(t.red)).wrap());
            ui.add(egui::Label::new(RichText::new(SAVE_FAILED).size(12.0).color(t.warm_note_text)).wrap());
            ui.horizontal(|ui| {
                if destructive_button(ui, DISCARD, true).clicked() {
                    discard = Some(true);
                }
                if outline_button(ui, CANCEL, true).clicked() {
                    discard = Some(false);
                }
            });
        });
        match discard {
            Some(true) => self.discard_failed(),
            Some(false) => self.cancel_failed(),
            None => {}
        }
    }
}

/// A panel's colours from the graph palette, one row per slot: its own
/// (cores green, GPU blue, Neural and Media Engine yellow, Thermals orange,
/// Memory purple) and its dashed line's (the value blue for the cores'
/// clock, the lighter blue for the GPU clock, else its own).
fn slot_colors(slot: Slot, g: &GraphPalette) -> (Color32, Color32) {
    match slot {
        Slot::PerformanceCores | Slot::EfficiencyCores => (g.utilisation, g.value),
        Slot::GpuCores => (g.gpu, g.network_sent),
        Slot::NeuralEngine | Slot::MediaEngine => (g.power, g.power),
        Slot::Thermals => (g.thermal, g.thermal),
        Slot::Memory => (g.memory, g.memory),
    }
}

/// One panel: title, what it covers, value top right, the chart (solid
/// load, dashed clock, dotted red temperature) on the always-dark graph
/// surface, the cells (the hottest sensor ringed) and the note.
fn panel_card(ui: &mut Ui, panel: &PanelView, width: f32) {
    let t = theme::of_ui(ui);
    let g = t.graphs;
    let (color, dashed) = slot_colors(panel.slot, &g);
    telemetry_panel(ui, panel.slot.title(), &panel.sub, &panel.value, color, width, |ui| {
        let lines: Vec<ChartLine> = panel
            .series
            .iter()
            .map(|s| {
                let (style, color) = match s.line {
                    Line::Solid => (LineStyle::Solid, color),
                    Line::Dashed => (LineStyle::Dashed, dashed),
                    Line::Third => (LineStyle::Dotted, g.kernel),
                };
                ChartLine { values: &s.values, max: s.max, style, color }
            })
            .collect();
        line_chart(ui, vec2(ui.available_width(), 80.0), &lines);
        let ringed = (panel.slot == Slot::Thermals).then(|| thermals::hottest(&panel.cells)).flatten().map(|i| (i, color.lerp_to_gamma(g.marker, 0.6)));
        cell_grid(ui, &panel.cells, &panel.names, color, ringed);
        if !panel.note.is_empty() {
            ui.add(egui::Label::new(RichText::new(panel.note.as_str()).size(11.5).color(t.faint)).wrap());
        }
    });
}

/// A coloured dot and a word in `ink`, as the Summary tile's.
pub(super) fn dot_word(ui: &mut Ui, dot: Color32, ink: Color32, word: &str) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(vec2(6.0, 6.0), Sense::hover());
        ui.painter().circle_filled(r.center(), 3.0, dot);
        ui.label(RichText::new(word).size(11.0).color(ink));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::Addon;
    use crate::platform::AddonContext;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;
    use crate::theme::{Mode, Theme};
    use egui::Shape;

    use crate::test_ui::context;

    /// One 1400x900 frame of `draw`; every shape drawn, nested ones flattened.
    fn shapes(ctx: &egui::Context, draw: impl FnMut(&mut Ui)) -> Vec<Shape> {
        crate::test_ui::flat_shapes(&crate::test_ui::run(ctx, (1400.0, 900.0), vec![], draw))
    }

    fn fills(shapes: &[Shape]) -> Vec<Color32> {
        shapes.iter().filter_map(|s| if let Shape::Rect(r) = s { Some(r.fill) } else { None }).collect()
    }

    /// The first text drawn that `matches`: its colour and face.
    fn text(shapes: &[Shape], matches: impl Fn(&str) -> bool) -> Option<(Color32, egui::FontId)> {
        shapes.iter().find_map(|s| match s {
            Shape::Text(t) if matches(t.galley.text()) => t.galley.job.sections.first().map(|s| (s.format.color, s.format.font_id.clone())),
            _ => None,
        })
    }

    fn sample() -> Snapshot {
        Snapshot { cpu_total: 10.0, cpu_per_core: vec![10.0, 5.0], mem_total: 100, mem_used: 25, ..Default::default() }
    }

    fn recording() -> FlightRecorder {
        let mut fr = FlightRecorder::default();
        fr.start(&AddonContext::recording().0);
        fr.record(1000, &sample()).expect("recording starts");
        fr
    }

    fn is_timer(s: &str) -> bool {
        s.len() == 11 && s.starts_with("00:00:") && s.as_bytes()[8] == b'.'
    }

    #[test]
    fn the_recording_cards_time_looks_like_the_status_bars_timer() {
        for mode in [Mode::Dark, Mode::Light] {
            let ctx = context(mode);
            let bar = shapes(&ctx, |ui| {
                crate::widgets::recording_timer(ui, 1.5);
            });
            let mut fr = recording();
            let card = shapes(&ctx, |ui| fr.mode_card(ui));
            fr.stop();
            let t = Theme::of(mode);
            let timer = text(&card, is_timer).expect("the card shows HH:MM:SS.cc");
            assert_eq!(Some(timer.clone()), text(&bar, is_timer), "{mode:?}: same face and colour as the status bar's timer");
            assert_eq!(timer.0, t.graphs.value);
            assert!(fills(&card).contains(&t.timer_bg), "{mode:?}: on the always-dark timer box");
        }
    }

    #[test]
    fn each_panel_takes_its_colours_from_the_graph_palette_in_both_modes() {
        for mode in [Mode::Dark, Mode::Light] {
            let g = Theme::of(mode).graphs;
            assert_eq!(slot_colors(Slot::PerformanceCores, &g), (g.utilisation, g.value));
            assert_eq!(slot_colors(Slot::EfficiencyCores, &g), (g.utilisation, g.value));
            assert_eq!(slot_colors(Slot::GpuCores, &g), (g.gpu, g.network_sent));
            assert_eq!(slot_colors(Slot::NeuralEngine, &g), (g.power, g.power));
            assert_eq!(slot_colors(Slot::MediaEngine, &g), (g.power, g.power));
            assert_eq!(slot_colors(Slot::Thermals, &g), (g.thermal, g.thermal));
            assert_eq!(slot_colors(Slot::Memory, &g), (g.memory, g.memory));
        }
    }

    #[test]
    fn in_light_mode_the_page_is_light_and_its_charts_stay_dark() {
        let ctx = context(Mode::Light);
        let mut settings = Settings::default();
        settings.dark = false;
        let sample = sample();
        let mut fr = FlightRecorder::default();
        fr.on_sample(&sample);
        fr.on_sample(&sample);
        let drawn = shapes(&ctx, |ui| fr.page(ui, &mut PageContext::new(&settings, &sample)));
        let (t, f) = (Theme::of(Mode::Light), fills(&drawn));
        for (token, name) in [(t.band, "band"), (t.panel, "panel"), (t.primary, "Record"), (t.neutral_note, "legend"), (t.graph_bg, "chart")] {
            assert!(f.contains(&token), "{name} missing: {f:?}");
        }
        assert_eq!(text(&drawn, |s| s.trim() == "Record").map(|c| c.0), Some(t.on_primary));
        assert_eq!(text(&drawn, |s| s == "Saving").map(|c| c.0), Some(t.text));
    }

    #[test]
    fn the_page_reads_the_theme_not_the_old_palette() {
        for source in [include_str!("page.rs"), include_str!("../flight_recorder.rs")] {
            let code = source.split("#[cfg(test)]").next().unwrap_or_default();
            assert!(!code.contains("palette("), "the page still reads widgets::palette");
        }
    }
}
