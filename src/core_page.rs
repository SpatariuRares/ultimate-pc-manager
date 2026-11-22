//! The one interface every core page shares: its own state, what it does
//! with a new sample, what it does on refresh, and how it draws. The app
//! keeps only the choice of the page shown and the order a sample reaches
//! them in (see [`SampleSlot`]).

use std::collections::VecDeque;

use egui::Ui;

use crate::addons::{AddonHost, Core};
use crate::platform::Action;
use crate::sampler::Snapshot;
use crate::settings::Settings;

/// The Performance history: one series per name (`cpu`, `cpu.k`, `net.r`,
/// ...), newest last. Performance draws it and Summary reads from it.
pub type PerfHistory = std::collections::BTreeMap<String, VecDeque<f32>>;

/// Histories more than one page reads; the app owns them and lends them.
#[derive(Clone, Copy)]
pub struct Histories<'a> {
    /// CPU total, in percent, per sample (Summary, Colors).
    pub cpu: &'a VecDeque<f32>,
    /// Memory used, in percent, per sample (Summary, Colors).
    pub mem: &'a VecDeque<f32>,
    /// The Performance series (Performance, Summary).
    pub perf: &'a PerfHistory,
}

/// What a page is given: the settings, the latest snapshot and the app's
/// shared histories. A new field here is additive for every page.
#[derive(Clone, Copy)]
pub struct PageCtx<'a> {
    pub settings: &'a Settings,
    /// While a sample is delivered, the new sample; otherwise the latest.
    pub snapshot: &'a Snapshot,
    pub histories: Histories<'a>,
}

/// Where in the app's handling of a new sample a page's
/// [`CorePage::on_sample`] runs. The variants are in the order the app
/// handles a sample: add-ons first (not a page), then these.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum SampleSlot {
    /// With the Performance history.
    Performance,
    /// With App history's totals.
    AppHistory,
    /// With the Processes view's tracking of ended processes.
    ProcessTracking,
    /// With the Summary rebuild.
    Summary,
    /// After all of those, before the signatures are asked for.
    Last,
}

pub trait CorePage {
    /// The title the standard header (title, Refresh, Run new task) shows
    /// above the page; `None` when the page draws its own header.
    fn title(&self) -> Option<&'static str> {
        None
    }

    /// Where a new sample reaches this page.
    fn sample_slot(&self) -> SampleSlot {
        SampleSlot::Last
    }

    /// A new sample arrived (never while paused, unless it was forced).
    fn on_sample(&mut self, _ctx: &PageCtx) {}

    /// Refresh was asked while this page is shown.
    fn refresh(&mut self, _ctx: &PageCtx) {}

    /// Draws the page; what it asks of the app goes in `actions`.
    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, actions: &mut Vec<Action>);

    /// Draws a page that also needs the add-on host (Settings lists the
    /// add-ons and draws their settings). The app always calls this one;
    /// the default ignores the host.
    fn draw_with_addons(&mut self, ui: &mut Ui, ctx: &PageCtx, _host: &mut AddonHost, actions: &mut Vec<Action>) {
        self.draw(ui, ctx, actions);
    }
}

/// A page's icon from `assets/icons`; a page without one gets a plain
/// square outline in `ink`.
pub fn page_icon(painter: &egui::Painter, center: egui::Pos2, id: &str, ink: egui::Color32) {
    if !crate::widgets::icons::draw(painter, center, id) {
        crate::widgets::plain_icon(painter, center, ink);
    }
}

/// The title a page draws itself when it has no standard header: the icon
/// of the page `id` and a large, widely spaced heading.
pub fn page_title(ui: &mut Ui, id: &str, title: &str) {
    let theme = crate::theme::of_ui(ui);
    let (r, _) = ui.allocate_exact_size(egui::vec2(26.0, 40.0), egui::Sense::hover());
    page_icon(ui.painter(), r.center(), id, theme.label);
    ui.label(egui::RichText::new(title).font(theme.fonts.page_title.clone()).color(theme.text));
}

/// Every core page that has moved to the interface, one field each.
#[derive(Default)]
pub struct CorePages {
    pub performance: crate::page_perf::PerformancePage,
    pub system_info: crate::page_system::SystemInfoPage,
    pub users: crate::page_users::UsersPage,
    pub settings: crate::page_settings::SettingsPage,
    pub colors: crate::page_colors::ColorsPage,
}

impl CorePages {
    /// Each moved page with its identity: one line per page.
    fn all_mut(&mut self) -> Vec<(Core, &mut dyn CorePage)> {
        vec![
            (Core::Performance, &mut self.performance),
            (Core::SystemInfo, &mut self.system_info),
            (Core::Users, &mut self.users),
            (Core::Settings, &mut self.settings),
            (Core::Colors, &mut self.colors),
        ]
    }

    /// The page for `core`, `None` while it has not moved.
    pub fn page_mut(&mut self, core: Core) -> Option<&mut dyn CorePage> {
        self.all_mut().into_iter().find(|(c, _)| *c == core).map(|(_, p)| p)
    }

    /// Delivers a new sample to the pages whose slot is `slot`.
    pub fn on_sample(&mut self, slot: SampleSlot, ctx: &PageCtx) {
        deliver(self.all_mut().into_iter().map(|(_, p)| p), slot, ctx);
    }
}

/// Delivers a sample to the pages of `slot`, in the order given.
fn deliver<'a>(pages: impl Iterator<Item = &'a mut dyn CorePage>, slot: SampleSlot, ctx: &PageCtx) {
    for p in pages.filter(|p| p.sample_slot() == slot) {
        p.on_sample(ctx);
    }
}

/// Helpers for testing a page through the interface, without the app.
#[cfg(test)]
pub mod testing {
    use super::*;
    use crate::theme::Mode;

    /// What one frame of a page drew and queued.
    pub struct Drawn {
        pub texts: Vec<String>,
        pub actions: Vec<Action>,
    }

    /// Draws `page` once in a 1400 x 900 frame with `settings`, `snapshot`
    /// and empty shared histories.
    pub fn draw(page: &mut dyn CorePage, settings: &Settings, snapshot: &Snapshot) -> Drawn {
        draw_with(page, settings, snapshot, &VecDeque::new(), &VecDeque::new())
    }

    /// The same with the app's short histories given.
    pub fn draw_with(page: &mut dyn CorePage, settings: &Settings, snapshot: &Snapshot, cpu: &VecDeque<f32>, mem: &VecDeque<f32>) -> Drawn {
        let ctx = crate::test_ui::context(Mode::Dark);
        let perf = PerfHistory::new();
        draw_in(&ctx, page, &PageCtx { settings, snapshot, histories: Histories { cpu, mem, perf: &perf } })
    }

    /// The same for a page that needs the add-on host (Settings).
    pub fn draw_with_addons(page: &mut dyn CorePage, settings: &Settings, snapshot: &Snapshot, host: &mut AddonHost) -> Drawn {
        let ctx = crate::test_ui::context(Mode::Dark);
        let perf = PerfHistory::new();
        draw_hosted_in(&ctx, page, &PageCtx { settings, snapshot, histories: Histories { cpu: &VecDeque::new(), mem: &VecDeque::new(), perf: &perf } }, host)
    }

    /// Draws with the host in `ctx`, keeping what egui remembers.
    pub fn draw_hosted_in(ctx: &egui::Context, page: &mut dyn CorePage, page_ctx: &PageCtx, host: &mut AddonHost) -> Drawn {
        let mut actions = Vec::new();
        let out = crate::test_ui::run(ctx, (1400.0, 900.0), vec![], |ui| page.draw_with_addons(ui, page_ctx, host, &mut actions));
        Drawn { texts: crate::test_ui::texts(&out), actions }
    }

    /// Where the text `word` was drawn in `out`.
    pub fn place(out: &egui::FullOutput, word: &str) -> egui::Pos2 {
        crate::test_ui::flat_shapes(out)
            .iter()
            .find_map(|s| match s {
                egui::Shape::Text(t) if t.galley.text() == word => Some(t.pos + t.galley.rect.center().to_vec2()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{word:?} is drawn"))
    }

    /// Clicks the text `word` of `page` (drawn first with `settings` and
    /// `snapshot`) and returns what the frame of the release queued.
    pub fn click(page: &mut dyn CorePage, settings: &Settings, snapshot: &Snapshot, word: &str) -> Vec<Action> {
        use egui::{Event, PointerButton};
        let (cpu, mem) = (VecDeque::new(), VecDeque::new());
        let perf = PerfHistory::new();
        let page_ctx = PageCtx { settings, snapshot, histories: Histories { cpu: &cpu, mem: &mem, perf: &perf } };
        let ctx = crate::test_ui::context(Mode::Dark);
        let size = (1400.0, 900.0);
        let mut frame = |events: Vec<Event>| {
            let mut actions = Vec::new();
            let out = crate::test_ui::run(&ctx, size, events, |ui| page.draw(ui, &page_ctx, &mut actions));
            (out, actions)
        };
        let at = place(&frame(vec![]).0, word);
        let press = |pressed| Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Default::default() };
        frame(vec![Event::PointerMoved(at), press(true)]);
        frame(vec![press(false)]).1
    }

    /// Draws in `ctx` (draw again in the same one to keep what egui
    /// remembers of the page between frames).
    pub fn draw_in(ctx: &egui::Context, page: &mut dyn CorePage, page_ctx: &PageCtx) -> Drawn {
        let mut actions = Vec::new();
        let out = crate::test_ui::run(ctx, (1400.0, 900.0), vec![], |ui| page.draw(ui, page_ctx, &mut actions));
        Drawn { texts: crate::test_ui::texts(&out), actions }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Recorder {
        name: &'static str,
        slot: SampleSlot,
        log: std::rc::Rc<std::cell::RefCell<Vec<String>>>,
    }

    impl CorePage for Recorder {
        fn sample_slot(&self) -> SampleSlot {
            self.slot
        }
        fn on_sample(&mut self, ctx: &PageCtx) {
            self.log.borrow_mut().push(format!("{} saw seq {}", self.name, ctx.snapshot.seq));
        }
        fn draw(&mut self, _: &mut Ui, _: &PageCtx, _: &mut Vec<Action>) {}
    }

    #[test]
    fn a_sample_reaches_only_the_pages_of_the_slot_asked() {
        let log = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut perf = Recorder { name: "perf", slot: SampleSlot::Performance, log: log.clone() };
        let mut summary = Recorder { name: "summary", slot: SampleSlot::Summary, log: log.clone() };
        let settings = Settings::default();
        let snapshot = Snapshot { seq: 7, ..Default::default() };
        let (cpu, mem, history) = (VecDeque::new(), VecDeque::new(), PerfHistory::new());
        let ctx = PageCtx { settings: &settings, snapshot: &snapshot, histories: Histories { cpu: &cpu, mem: &mem, perf: &history } };
        deliver([&mut summary as &mut dyn CorePage, &mut perf].into_iter(), SampleSlot::Performance, &ctx);
        assert_eq!(*log.borrow(), ["perf saw seq 7"]);
        deliver([&mut summary as &mut dyn CorePage, &mut perf].into_iter(), SampleSlot::Summary, &ctx);
        assert_eq!(*log.borrow(), ["perf saw seq 7", "summary saw seq 7"]);
    }

    #[test]
    fn the_slots_follow_the_order_a_sample_is_handled_in() {
        use SampleSlot::*;
        assert!([Performance, AppHistory, ProcessTracking, Summary, Last].windows(2).all(|w| w[0] < w[1]));
    }
}
