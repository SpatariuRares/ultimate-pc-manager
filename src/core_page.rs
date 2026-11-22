//! The one interface every core page shares: its own state, what it does
//! with a new sample, what it does on refresh, and how it draws. The app
//! keeps only the choice of the page shown and the order a sample reaches
//! them in (see [`SampleSlot`]).

use std::collections::VecDeque;

use egui::Ui;

use crate::addons::Core;
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
}

/// Every core page that has moved to the interface, one field each.
#[derive(Default)]
pub struct CorePages {
    pub performance: crate::page_perf::PerformancePage,
    pub system_info: crate::page_system::SystemInfoPage,
    pub users: crate::page_users::UsersPage,
}

impl CorePages {
    /// Each moved page with its identity: one line per page.
    fn all_mut(&mut self) -> Vec<(Core, &mut dyn CorePage)> {
        vec![(Core::Performance, &mut self.performance), (Core::SystemInfo, &mut self.system_info), (Core::Users, &mut self.users)]
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
