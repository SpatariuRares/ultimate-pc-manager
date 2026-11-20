//! The series executor add-ons share: one job at a time per add-on, in a
//! background thread, with the page's "busy" text claimed while it runs.
//!
//! A job may run a list of items one after another
//! ([`Run::each`]): each item's step is published, an item that fails does
//! not stop the ones after it, the series stops after the current item
//! when asked ([`Series::stop_after_current`]) or when the add-on is
//! turned off, and the caller gets one outcome per item for its summary.
//!
//! An uninterruptible job counts in [`Series::running`] until it ends: the
//! add-on reports it as an operation that must not be cut short, so the
//! host turns it off only when it is done.

use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use super::install_queue::{InstallQueue, Request, Turn};
use super::{lock_state, Generation, Job};

/// The page's "busy" text inside its state.
type Busy<S> = fn(&mut S) -> &mut Option<String>;

/// Runs an add-on's jobs one at a time over its page state `S`, under the
/// add-on's [`Generation`] (see the module docs).
pub struct Series<S> {
    /// The page state and the generation the series was born under.
    job: Job<S>,
    busy: Busy<S>,
    /// Uninterruptible jobs running.
    running: Arc<AtomicUsize>,
    /// Set by [`Series::stop_after_current`].
    halt: Arc<AtomicBool>,
    /// Tests: a started job waits for this lock before it runs (see
    /// [`Series::gated`]).
    #[cfg(test)]
    gate: Option<Arc<Mutex<()>>>,
}

impl<S> Clone for Series<S> {
    fn clone(&self) -> Self {
        Series {
            job: self.job.clone(),
            busy: self.busy,
            running: self.running.clone(),
            halt: self.halt.clone(),
            #[cfg(test)]
            gate: self.gate.clone(),
        }
    }
}

impl<S: Default + Send + 'static> Series<S> {
    /// The series over `state`, stopped once `life` moves on; `busy` gives
    /// the state's "busy" text.
    pub fn new(state: &Arc<Mutex<S>>, life: &Generation, busy: Busy<S>) -> Self {
        Series {
            job: Job::new(state, life),
            busy,
            running: Default::default(),
            halt: Default::default(),
            #[cfg(test)]
            gate: None,
        }
    }

    /// Tests: every job started waits until `gate` is free before it runs,
    /// so a test can act in the instant between a start and its job.
    #[cfg(test)]
    pub fn gated(mut self, gate: Option<Arc<Mutex<()>>>) -> Self {
        self.gate = gate;
        self
    }

    /// The add-on was turned off since the series was made.
    pub fn stopped(&self) -> bool {
        self.job.cancelled()
    }

    /// An uninterruptible job is running.
    pub fn running(&self) -> bool {
        self.running.load(Ordering::SeqCst) > 0
    }

    /// Ends the running series after its current item.
    pub fn stop_after_current(&self) {
        self.halt.store(true, Ordering::SeqCst);
    }

    /// The add-on was turned off: no new job starts, a series stops after
    /// its current item. With no uninterruptible job running the state is
    /// forgotten; otherwise `keep` clears what must go and the rest stays
    /// for the page that follows the job to its end.
    pub fn stop(&self, keep: impl FnOnce(&mut S)) {
        let mut st = lock_state(&self.job.state);
        self.job.generation.bump();
        if self.running() {
            keep(&mut st);
        } else {
            *st = S::default();
        }
    }

    /// Runs `job` in a thread unless the add-on was turned off or, when
    /// `busy` is given, another job holds the busy text. That text is
    /// claimed until the job ends or calls [`Run::release`]. An
    /// uninterruptible job counts in [`Series::running`] until it ends.
    pub fn start(&self, busy: Option<String>, uninterruptible: bool, job: impl FnOnce(&Run<S>) + Send + 'static) -> bool {
        self.start_with(busy, uninterruptible, |_| {}, job)
    }

    /// [`Series::start`], and once the job is accepted `accepted` updates
    /// the state under the same lock, before the job's thread exists: what
    /// it sets is seen by every reader from the moment the job is accepted.
    pub fn start_with(&self, busy: Option<String>, uninterruptible: bool, accepted: impl FnOnce(&mut S), job: impl FnOnce(&Run<S>) + Send + 'static) -> bool {
        let claimed = busy.is_some();
        let counted = {
            let mut st = lock_state(&self.job.state);
            if self.stopped() {
                return false;
            }
            if let Some(text) = busy {
                let held = (self.busy)(&mut st);
                if held.is_some() {
                    return false;
                }
                *held = Some(text);
            }
            accepted(&mut st);
            // Counted under the lock, so a `stop` sees it.
            uninterruptible.then(|| {
                self.running.fetch_add(1, Ordering::SeqCst);
                Counted(self.running.clone())
            })
        };
        self.halt.store(false, Ordering::SeqCst);
        let series = self.clone();
        std::thread::spawn(move || {
            #[cfg(test)]
            if let Some(gate) = series.gate.clone() {
                drop(lock_state(&gate));
            }
            let run = Run { series, claimed: Cell::new(claimed), _counted: counted };
            job(&run);
        });
        true
    }
}

/// Counts an uninterruptible job while it lives (also when it panics).
struct Counted(Arc<AtomicUsize>);

impl Drop for Counted {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// A running job's handle: its page state, its series of items and the
/// busy text it holds (given back when the job ends).
pub struct Run<S> {
    series: Series<S>,
    /// The job still holds the busy text.
    claimed: Cell<bool>,
    /// Dropped after the busy text is given back.
    _counted: Option<Counted>,
}

impl<S> Run<S> {
    /// The page state, also after the add-on was turned off (so the page
    /// that stays open can follow an uninterruptible job to its end).
    pub fn follow(&self) -> MutexGuard<'_, S> {
        lock_state(&self.series.job.state)
    }

    /// The add-on was turned off.
    pub fn stopped(&self) -> bool {
        self.series.job.cancelled()
    }

    /// Gives the busy text back now, inside `st` (the state the job is
    /// publishing its outcome into), instead of when the job ends.
    pub fn release(&self, st: &mut S) {
        *(self.series.busy)(st) = None;
        self.claimed.set(false);
    }

    /// Waits for the app's install queue (see [`InstallQueue::enter`]).
    /// Meanwhile the busy text says who holds it; then the job's own text
    /// comes back. `None` when the add-on was turned off meanwhile: the
    /// job must not run (the queue is given back unused).
    pub fn queue(&self, queue: &InstallQueue, request: Request) -> Option<Turn> {
        let own = (self.series.busy)(&mut self.follow()).clone();
        let turn = queue.enter_unless(request, || self.stopped(), |holder| *(self.series.busy)(&mut self.follow()) = Some(holder.text()));
        *(self.series.busy)(&mut self.follow()) = own;
        let turn = turn?;
        if self.stopped() {
            turn.give_back();
            return None;
        }
        Some(turn)
    }

    /// Runs `run` on each item in order, showing `label(item, index)` as
    /// the busy text while it runs. An item's outcome never stops the ones
    /// after it; the series stops after the current item when the add-on
    /// is turned off or [`Series::stop_after_current`] was asked. Returns
    /// each item's outcome, `None` for the items not started.
    pub fn each<T, R>(&self, items: &[T], label: impl Fn(&T, usize) -> String, mut run: impl FnMut(&T) -> R) -> Vec<Option<R>> {
        let mut outcomes = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            if self.stopped() || self.series.halt.load(Ordering::SeqCst) {
                outcomes.push(None);
                continue;
            }
            *(self.series.busy)(&mut self.follow()) = Some(label(item, i));
            outcomes.push(Some(run(item)));
        }
        outcomes
    }
}

impl<S> Drop for Run<S> {
    fn drop(&mut self) {
        if self.claimed.get() {
            *(self.series.busy)(&mut self.follow()) = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{until, Addon, AddonHost, Consumption, Entry, Requirements, Tier};
    use crate::platform::{AddonContext, Launch, PageContext, When};
    use serde_json::json;

    /// What the test add-on's page shows.
    #[derive(Default)]
    struct Shown {
        busy: Option<String>,
        summary: Option<String>,
    }

    /// An add-on whose Refresh installs three packages with winget, one
    /// after another, through the series executor.
    struct Batch {
        context: Option<AddonContext>,
        state: Arc<Mutex<Shown>>,
        series: Series<Shown>,
    }

    impl Batch {
        fn new() -> Self {
            let state = Arc::new(Mutex::new(Shown::default()));
            let series = Series::new(&state, &Generation::default(), |s: &mut Shown| &mut s.busy);
            Batch { context: None, state, series }
        }
    }

    impl Addon for Batch {
        fn id(&self) -> &'static str {
            "batch"
        }
        fn name(&self) -> &'static str {
            "Batch"
        }
        fn description(&self) -> &'static str {
            "Installs several things."
        }
        fn requirements(&self) -> Requirements {
            Requirements::default()
        }
        fn consumption(&self) -> Consumption {
            Consumption { cpu: "None", network: "None", disk: "None", when: "On install" }
        }
        fn tier(&self) -> Tier {
            Tier::Free
        }
        fn start(&mut self, context: &AddonContext) {
            self.context = Some(context.clone());
        }
        fn stop(&mut self) {
            self.series.stop(|_| {});
        }
        fn uninterruptible(&self) -> bool {
            self.series.running()
        }
        fn ui(&mut self, _: &mut egui::Ui, _: &mut PageContext) {}
        fn refresh(&mut self) {
            let ctx = self.context.clone().unwrap();
            let items = vec!["a", "b", "c"];
            self.series.start(Some("Installing\u{2026}".into()), true, move |run| {
                let outcomes = run.each(&items, |id, i| format!("Installing {id} ({} of 3)", i + 1), |id| ctx.winget(&["install", "--id", id]));
                let words: Vec<String> = items
                    .iter()
                    .zip(&outcomes)
                    .map(|(id, outcome)| match outcome {
                        Some(Ok((0, _))) => format!("{id} installed"),
                        Some(Ok((code, _))) => format!("{id} failed ({code})"),
                        Some(Err(e)) => format!("{id} failed ({e})"),
                        None => format!("{id} not started"),
                    })
                    .collect();
                run.follow().summary = Some(words.join(", "));
            });
        }
    }

    /// The host with only the test add-on, the add-on's page state and its
    /// series executor.
    fn host() -> (AddonHost, Arc<crate::platform::RecordingRunner>, Arc<Mutex<Shown>>, Series<Shown>) {
        let batch = Batch::new();
        let (state, series) = (batch.state.clone(), batch.series.clone());
        let (host, runner) = AddonHost::for_tests_with(vec![Entry::Addon(Box::new(batch))], &json!({}));
        (host, runner, state, series)
    }

    fn installs(ids: &[&str]) -> Vec<Launch> {
        ids.iter().map(|id| Launch::Winget(vec!["install".into(), "--id".into(), id.to_string()])).collect()
    }

    #[test]
    fn a_failure_in_the_middle_does_not_stop_the_items_after_it() {
        let (mut host, runner, state, _) = host();
        runner.prepare(When::Arg("a".into()), 0, "done");
        runner.prepare(When::Arg("b".into()), 1603, "fatal error");
        runner.prepare(When::Arg("c".into()), 0, "done");

        assert!(host.refresh("batch"));
        until("the series ends", || lock_state(&state).summary.is_some());

        assert_eq!(runner.launches(), installs(&["a", "b", "c"]), "every item runs, in the order given");
        assert_eq!(lock_state(&state).summary.as_deref(), Some("a installed, b failed (1603), c installed"));
        until("the page is idle", || lock_state(&state).busy.is_none());
    }

    #[test]
    fn each_item_is_published_while_it_runs() {
        let (mut host, runner, state, _) = host();
        runner.hold();
        assert!(host.refresh("batch"));
        runner.wait_for(1);

        assert_eq!(lock_state(&state).busy.as_deref(), Some("Installing a (1 of 3)"));
        runner.release();
        until("the series ends", || lock_state(&state).busy.is_none());
    }

    #[test]
    fn stopping_the_series_ends_it_after_the_current_item() {
        let (mut host, runner, state, series) = host();
        runner.prepare(When::Arg("a".into()), 0, "done");
        runner.hold();
        assert!(host.refresh("batch"));
        runner.wait_for(1);

        series.stop_after_current();
        assert!(series.running(), "the current item is not cut short");
        assert_eq!(runner.stopped(), 0, "its program is not ended");
        runner.release();
        until("the series ends", || lock_state(&state).busy.is_none());

        assert_eq!(runner.launches(), installs(&["a"]), "no item after the current one starts");
        assert_eq!(lock_state(&state).summary.as_deref(), Some("a installed, b not started, c not started"));
        assert!(host.is_enabled("batch"), "stopping the series leaves the add-on on");

        runner.prepare(When::Arg("a".into()), 0, "done");
        runner.prepare(When::Arg("b".into()), 0, "done");
        runner.prepare(When::Arg("c".into()), 0, "done");
        assert!(host.refresh("batch"));
        until("the next series ends", || runner.launches().len() == 4 && lock_state(&state).busy.is_none());
        assert_eq!(runner.launches()[1..], installs(&["a", "b", "c"]), "a new series runs every item again");
    }

    #[test]
    fn turning_off_during_a_series_turns_off_when_the_current_item_ends() {
        let (mut host, runner, state, _) = host();
        runner.hold();
        assert!(host.refresh("batch"));
        runner.wait_for(1);

        assert!(host.set_enabled("batch", false));
        assert!(host.is_stopping("batch"), "the switch shows it turns off when done");
        assert!(host.settle().is_empty(), "the current item still runs");
        assert_eq!(lock_state(&state).busy.as_deref(), Some("Installing a (1 of 3)"), "the page follows it");

        runner.release();
        until("the add-on turns off", || host.settle() == vec!["batch"]);
        assert!(!host.is_enabled("batch"));
        assert_eq!(runner.launches(), installs(&["a"]), "no item after the current one starts");
        assert!(lock_state(&state).summary.is_some(), "the series still tells how it ended");
    }

    #[test]
    fn a_second_job_does_not_start_while_one_runs() {
        let (mut host, runner, state, _) = host();
        runner.hold();
        assert!(host.refresh("batch"));
        runner.wait_for(1);
        assert!(host.refresh("batch"));
        runner.release();
        until("the series ends", || lock_state(&state).busy.is_none());
        assert_eq!(runner.launches(), installs(&["a", "b", "c"]), "the second series was refused");
    }

    fn request(addon: &'static str) -> Request {
        Request { addon, name: "P", work: crate::platform::install_queue::Work::Install, items: vec![("winget".into(), "a".into())] }
    }

    #[test]
    fn a_job_turned_off_before_it_asks_for_the_queue_does_not_wait_behind_another_addon() {
        for _ in 0..30 {
            let state = Arc::new(Mutex::new(Shown::default()));
            let series = Series::new(&state, &Generation::default(), |s: &mut Shown| &mut s.busy);
            let queue = InstallQueue::default();
            let held = queue.enter(request("other"), |_| {}).unwrap();
            let q = queue.clone();
            assert!(series.start(Some("Installing\u{2026}".into()), true, move |run| {
                let _ = run.queue(&q, request("p"));
            }));
            // The host's order when the add-on is turned off.
            series.stop(|_| {});
            queue.withdraw("p");
            until("the job ends while the other add-on still holds the queue", || !series.running());
            assert_eq!(queue.waiting(), 0);
            drop(held);
        }
    }

    #[test]
    fn a_panic_in_an_item_ends_the_series_and_frees_everything() {
        let state = Arc::new(Mutex::new(Shown::default()));
        let series = Series::new(&state, &Generation::default(), |s: &mut Shown| &mut s.busy);
        let queue = InstallQueue::default();
        let q = queue.clone();
        assert!(series.start(Some("Installing\u{2026}".into()), true, move |run| {
            let _turn = run.queue(&q, request("p")).unwrap();
            run.each(&[1, 2], |_, _| "Installing\u{2026}".into(), |_| -> () { panic!("an item broke") });
        }));
        until("everything is given back", || !series.running() && lock_state(&state).busy.is_none());
        assert_eq!(queue.holder(), None);
        assert!(series.start(Some("Again".into()), false, |_| {}), "a new job can start");
    }
}
