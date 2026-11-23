//! What Services, Startup apps and App history share, in Design A's look:
//! the band that sums a list up and the side panel's pieces. The pages themselves are in `page_services`, `page_startup` and
//! `page_history`.

use egui::{RichText, Ui};

use crate::app::TaskApp;
use crate::theme;
use crate::widgets::parts::{Mark, StatusBand};

/// Space between the header, the band and the list.
pub const GAP: f32 = 8.0;

/// A band that only sums the page up: a headline and the line under it.
pub fn summary_band(ui: &mut Ui, mark: Mark, headline: &str, sub: &str) {
    StatusBand::new(mark, headline).sub(sub).actions_width(0.0).show(ui, |_, _| {});
    ui.add_space(GAP);
}

/// The side panel's title: the row's name in the small figure face.
pub fn panel_title(ui: &mut Ui, name: &str) {
    let t = theme::of_ui(ui);
    ui.add(egui::Label::new(RichText::new(name).font(t.fonts.figure_small.clone()).color(t.text)).wrap());
    ui.add_space(4.0);
}

/// The values of a fact list, in the text colour, wrapped.
pub fn fact_values(ui: &mut Ui, values: &[&str], i: usize) {
    let t = theme::of_ui(ui);
    ui.add(egui::Label::new(RichText::new(values[i]).color(t.text)).wrap());
}

impl TaskApp {
    /// Reads the services list again through the seam: every reader outside
    /// the page (the sidebar, Go to services, a service control) comes here.
    pub fn reload_services(&mut self) {
        self.pages.services.reload(&*self.system);
    }

    /// The same for the startup list (the sidebar, an Enable or Disable).
    pub fn reload_startup(&mut self) {
        self.pages.startup.reload(&*self.system);
    }
}

/// Helpers for testing the three pages through the interface.
#[cfg(test)]
pub mod testing {
    use egui::{Event, PointerButton, Shape};

    pub use crate::core_page::testing::Env;
    use crate::core_page::{CorePage, PageCtx};
    use crate::platform::Action;

    pub const SIZE: (f32, f32) = (1300.0, 800.0);

    /// The texts of one frame of `page`.
    pub fn texts(ctx: &egui::Context, page: &mut dyn CorePage, page_ctx: &PageCtx) -> Vec<String> {
        crate::core_page::testing::draw_in(ctx, page, page_ctx).texts
    }

    /// Clicks the first text that reads `label` (press in one frame, release
    /// in the next) and returns the actions queued by the release frame.
    pub fn click(ctx: &egui::Context, page: &mut dyn CorePage, page_ctx: &PageCtx, label: &str) -> Vec<Action> {
        let out = crate::test_ui::run(ctx, SIZE, vec![], |ui| page.draw(ui, page_ctx, &mut Vec::new()));
        let at = crate::test_ui::flat_shapes(&out)
            .iter()
            .find_map(|sh| match sh {
                Shape::Text(g) if g.galley.text().trim() == label => Some(g.visual_bounding_rect().center()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("a {label} to click"));
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        crate::test_ui::run(ctx, SIZE, vec![Event::PointerMoved(at), press(true)], |ui| page.draw(ui, page_ctx, &mut Vec::new()));
        let mut actions = Vec::new();
        crate::test_ui::run(ctx, SIZE, vec![press(false)], |ui| page.draw(ui, page_ctx, &mut actions));
        actions
    }

    pub fn has(words: &[String], text: &str) -> bool {
        words.iter().any(|w| w.trim() == text)
    }
}
