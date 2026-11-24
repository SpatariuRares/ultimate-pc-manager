//! Disk Space's dialogs (the Stop and Clear History confirmations) and
//! the Disk Space menu in the page header.

use egui::{Align, Layout, RichText, Ui};

use super::{target, Command, DiskSpace, Drawing, Group};
use crate::platform::PageContext;
use crate::sampler::DiskRow;
use crate::theme;
use crate::widgets::parts::{destructive_button, outline_button, primary_button};
use crate::widgets::{confirm_window, ConfirmWindow};

impl DiskSpace {
    /// The Stop confirmation, while it is open: Stop Scanning / Keep
    /// Scanning with "Don't ask again".
    pub(super) fn stop_dialog(&mut self, ui: &mut Ui) {
        let Some(q) = self.confirm.as_mut() else { return };
        let mut decided = None;
        egui::Window::new(q.title.clone()).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            let t = theme::of_ui(ui);
            ui.set_max_width(400.0);
            ui.label(RichText::new(&q.text).size(13.0).color(t.soft));
            ui.add_space(4.0);
            ui.checkbox(&mut q.dont_ask, "Don't ask again");
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if destructive_button(ui, "Stop Scanning", true).clicked() {
                    decided = Some(true);
                }
                if primary_button(ui, "Keep Scanning", true).clicked() {
                    decided = Some(false);
                }
            });
        });
        if let Some(stop) = decided {
            let dont_ask = q.dont_ask;
            self.answer_stop(stop, dont_ask);
        }
    }

    /// The Move to trash confirmation, while it is open: the item and its
    /// size, Move to trash / Cancel.
    pub(super) fn trash_dialog(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let Some((title, text)) = self.trash_question() else { return };
        let decided = confirm_window(ui.ctx(), ConfirmWindow::new(title, "Move to trash"), |ui| {
            let t = theme::of_ui(ui);
            ui.set_max_width(400.0);
            ui.label(RichText::new(text).size(13.0).color(t.soft));
            ui.add_space(8.0);
        });
        if let Some(yes) = decided {
            self.answer_trash(yes, page);
        }
    }

    /// The Clear History confirmation, while it is open.
    pub(super) fn clear_dialog(&mut self, ui: &mut Ui) {
        let Some((title, text)) = self.clear_question() else { return };
        let mut decided = None;
        egui::Window::new(title).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            let t = theme::of_ui(ui);
            ui.set_max_width(400.0);
            ui.label(RichText::new(text).size(13.0).color(t.soft));
            ui.add_space(8.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if destructive_button(ui, "Clear History", true).clicked() {
                    decided = Some(true);
                }
                if outline_button(ui, "Cancel", true).clicked() {
                    decided = Some(false);
                }
            });
        });
        if let Some(clear) = decided {
            self.answer_clear(clear);
        }
    }

    /// The Disk Space menu (page header): Start, Pause/Resume, Stop,
    /// target, group, view, Clear History…
    pub(super) fn menu_items(&mut self, ui: &mut Ui, disks: &[DiskRow]) -> Option<Command> {
        let m = self.menu(disks);
        let mut picked = None;
        if ui.add_enabled(m.can_start, egui::Button::new(&m.start)).clicked() {
            picked = Some(Command::Start);
        }
        if ui.add_enabled(m.running, egui::Button::new(m.pause)).clicked() {
            picked = Some(if m.pause == "Resume" { Command::Resume } else { Command::Pause });
        }
        if ui.add_enabled(m.running, egui::Button::new("Stop")).clicked() {
            picked = Some(Command::Stop);
        }
        ui.separator();
        ui.menu_button("Target", |ui| {
            for t in target::choices(disks) {
                let label = t.label(disks);
                if ui.radio(self.target.as_ref() == Some(&t), label).clicked() {
                    picked = Some(Command::Target(t));
                }
            }
            if ui.button("Choose Folder\u{2026}").clicked() {
                picked = self.pick_folder().map(Command::Target);
            }
        });
        ui.menu_button("Group", |ui| {
            for g in Group::ALL {
                if ui.radio(self.group == g, g.label()).clicked() {
                    picked = Some(Command::Group(g));
                }
            }
        });
        ui.menu_button("View", |ui| {
            for d in Drawing::ALL {
                if ui.radio(self.drawing == d, d.label()).clicked() {
                    picked = Some(Command::View(d));
                }
            }
        });
        ui.separator();
        if ui.add_enabled(m.can_clear, egui::Button::new("Clear History\u{2026}")).clicked() {
            picked = Some(Command::ClearHistory);
        }
        if picked.is_some() {
            ui.close();
        }
        picked
    }
}
