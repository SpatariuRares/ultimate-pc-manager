//! The app's install queue: one install, update or uninstall job at a time
//! in the whole app, whichever add-on asks.
//!
//! A job enters with a [`Request`] (the add-on, what it does and its items)
//! and waits, in the order asked, until the queue is free; it holds the
//! queue until its [`Turn`] is dropped. While it waits it is told who holds
//! the queue ([`Holder::text`]). Turning an add-on off withdraws its
//! requests still waiting ([`InstallQueue::withdraw`]). Every job that ends
//! is noted for the host, which tells the add-ons that are on that the
//! installed packages changed ([`InstallQueue::take_finished`]).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use super::lock_state;

/// What a job does to its items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Work {
    Install,
    Update,
    Uninstall,
    /// Runs an installed program's own "Change" (modify) setup.
    Change,
    /// Runs an installed program's own repair.
    Repair,
}

impl Work {
    /// The verb shown while the job holds the queue ("installing").
    fn verb(self) -> &'static str {
        match self {
            Work::Install => "installing",
            Work::Update => "updating",
            Work::Uninstall => "uninstalling",
            Work::Change => "changing",
            Work::Repair => "repairing",
        }
    }
}

/// One job asking for the queue.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    /// The asking add-on's id (its requests go when it is turned off).
    pub addon: &'static str,
    /// The add-on's shown name, for the other add-ons' waiting text.
    pub name: &'static str,
    pub work: Work,
    /// The items, as (catalog, id): ("winget", "Vendor.App").
    pub items: Vec<(String, String)>,
}

/// The job holding the queue now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    pub addon: &'static str,
    pub name: &'static str,
    pub work: Work,
    /// How many items it works on.
    pub items: usize,
}

impl Holder {
    /// What it does: "Drivers is installing 1 item".
    pub fn doing(&self) -> String {
        format!("{} is {} {}", self.name, self.work.verb(), crate::addons::plural(self.items, "item"))
    }

    /// What an add-on waiting for the queue shows: "Drivers is installing
    /// 1 item; this starts when it is done…".
    pub fn text(&self) -> String {
        format!("{}; this starts when it is done\u{2026}", self.doing())
    }
}

/// The queue's state behind its lock.
#[derive(Default)]
struct Inner {
    /// The job holding the queue, by ticket.
    holder: Option<(u64, Holder)>,
    /// Requests waiting, in the order asked.
    waiting: VecDeque<(u64, Request)>,
    next_ticket: u64,
    /// The add-ons whose jobs ended since the host last asked.
    finished: Vec<&'static str>,
}

/// The app's install queue (see the module docs). Cheap to clone; every
/// clone is the same queue.
#[derive(Clone, Default)]
pub struct InstallQueue {
    inner: Arc<(Mutex<Inner>, Condvar)>,
}

impl InstallQueue {
    /// [`InstallQueue::enter_unless`] never cancelled (tests stand in for
    /// an add-on's job with it).
    #[cfg(test)]
    pub fn enter(&self, request: Request, waiting: impl FnMut(&Holder)) -> Option<Turn> {
        self.enter_unless(request, || false, waiting)
    }

    /// Waits, in the order asked, until the queue is free, then holds it
    /// until the returned [`Turn`] is dropped. While another job holds it,
    /// `waiting` is told (once per holder, outside the queue's lock).
    /// `None` when the request was withdrawn meanwhile, or as soon as
    /// `cancelled` is true while it waits (its add-on was turned off): the
    /// job must not run. So a job whose add-on was turned off never waits
    /// behind another add-on's job, even when it asked after its add-on's
    /// requests were withdrawn.
    pub fn enter_unless(&self, request: Request, cancelled: impl Fn() -> bool, mut waiting: impl FnMut(&Holder)) -> Option<Turn> {
        let (lock, changed) = &*self.inner;
        let ticket = {
            let mut inner = lock_state(lock);
            let ticket = inner.next_ticket;
            inner.next_ticket += 1;
            inner.waiting.push_back((ticket, request));
            ticket
        };
        // Takes the request out of the line however the wait ends (also
        // when `waiting` or `cancelled` panics).
        let _leave = Leave { queue: self, ticket };
        let mut told: Option<Holder> = None;
        loop {
            if cancelled() {
                return None;
            }
            let holder = {
                let mut inner = lock_state(lock);
                let Some(at) = inner.waiting.iter().position(|(t, _)| *t == ticket) else {
                    return None;
                };
                if inner.holder.is_none() && at == 0 {
                    let (_, request) = inner.waiting.pop_front().expect("first");
                    let holder = Holder { addon: request.addon, name: request.name, work: request.work, items: request.items.len() };
                    inner.holder = Some((ticket, holder));
                    return Some(Turn { queue: self.clone(), addon: request.addon, ran: true });
                }
                let holder = inner.holder.as_ref().map(|(_, h)| h.clone());
                // Woken by a release or a withdrawal; the timeout only
                // guards against a missed wake-up.
                let _ = changed.wait_timeout(inner, Duration::from_millis(50));
                holder
            };
            if let Some(h) = holder {
                if told.as_ref() != Some(&h) {
                    waiting(&h);
                    told = Some(h);
                }
            }
        }
    }

    /// The job holding the queue now, if any.
    pub fn holder(&self) -> Option<Holder> {
        lock_state(&self.inner.0).holder.as_ref().map(|(_, h)| h.clone())
    }

    /// How many items the requests still waiting carry.
    pub fn waiting(&self) -> usize {
        lock_state(&self.inner.0).waiting.iter().map(|(_, r)| r.items.len()).sum()
    }

    /// Removes the requests of `addon` still waiting (it was turned off);
    /// their [`InstallQueue::enter`] returns `None`. A job holding the
    /// queue is not touched.
    pub fn withdraw(&self, addon: &str) {
        let (lock, changed) = &*self.inner;
        lock_state(lock).waiting.retain(|(_, r)| r.addon != addon);
        changed.notify_all();
    }

    /// The add-ons whose jobs ended since the last call, in the order they
    /// ended (the host then tells the add-ons that are on).
    pub fn take_finished(&self) -> Vec<&'static str> {
        std::mem::take(&mut lock_state(&self.inner.0).finished)
    }
}

/// A request's place in the line, given up when its wait ends: once it got
/// its turn the request is no longer in the line and nothing changes.
struct Leave<'a> {
    queue: &'a InstallQueue,
    ticket: u64,
}

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        let (lock, changed) = &*self.queue.inner;
        let mut inner = lock_state(lock);
        let before = inner.waiting.len();
        inner.waiting.retain(|(t, _)| *t != self.ticket);
        if inner.waiting.len() != before {
            drop(inner);
            changed.notify_all();
        }
    }
}

/// A job's hold on the queue; dropping it frees the queue for the next
/// request and notes the job as finished.
pub struct Turn {
    queue: InstallQueue,
    addon: &'static str,
    /// Noted as finished when dropped (false: given back unused).
    ran: bool,
}

impl Turn {
    /// Frees the queue without noting a finished job: the job did nothing
    /// (its add-on was turned off just as its turn came).
    pub fn give_back(mut self) {
        self.ran = false;
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let (lock, changed) = &*self.queue.inner;
        {
            let mut inner = lock_state(lock);
            inner.holder = None;
            if self.ran {
                inner.finished.push(self.addon);
            }
        }
        changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::until;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn request(addon: &'static str, name: &'static str, work: Work, n: usize) -> Request {
        Request { addon, name, work, items: (0..n).map(|i| ("winget".to_string(), format!("Vendor.App{i}"))).collect() }
    }

    /// Enters the queue in a thread; the flag turns true once it got its
    /// turn, which it then keeps until the returned sender is dropped.
    fn enter_in_thread(queue: &InstallQueue, r: Request, seen: Arc<Mutex<Vec<String>>>) -> (Arc<AtomicBool>, std::sync::mpsc::Sender<()>, std::thread::JoinHandle<bool>) {
        let got = Arc::new(AtomicBool::new(false));
        let (done, hold) = std::sync::mpsc::channel::<()>();
        let (q, flag) = (queue.clone(), got.clone());
        let thread = std::thread::spawn(move || match q.enter(r, |h| seen.lock().unwrap().push(h.text())) {
            Some(turn) => {
                flag.store(true, Ordering::SeqCst);
                let _ = hold.recv();
                drop(turn);
                true
            }
            None => false,
        });
        (got, done, thread)
    }

    #[test]
    fn a_second_job_waits_for_the_first_to_end_and_is_told_who_holds_the_queue() {
        let queue = InstallQueue::default();
        let first = queue.enter(request("installed-apps", "Installed Apps", Work::Update, 2), |_| panic!("the queue was free")).expect("free");
        assert_eq!(queue.holder(), Some(Holder { addon: "installed-apps", name: "Installed Apps", work: Work::Update, items: 2 }));

        let seen = Arc::new(Mutex::new(Vec::new()));
        let (got, done, thread) = enter_in_thread(&queue, request("drivers", "Drivers", Work::Install, 1), seen.clone());
        until("the second request waits", || queue.waiting() == 1);
        until("it is told who holds the queue", || !seen.lock().unwrap().is_empty());
        assert!(!got.load(Ordering::SeqCst), "it does not run while the queue is held");
        assert_eq!(*seen.lock().unwrap(), ["Installed Apps is updating 2 items; this starts when it is done\u{2026}"]);

        drop(first);
        until("the second job gets its turn", || got.load(Ordering::SeqCst));
        assert_eq!(queue.holder().map(|h| h.addon), Some("drivers"));
        assert_eq!(queue.waiting(), 0);
        drop(done);
        assert!(thread.join().unwrap());
        assert_eq!(queue.holder(), None, "free once the job ends");
    }

    #[test]
    fn requests_get_their_turn_in_the_order_asked() {
        let queue = InstallQueue::default();
        let first = queue.enter(request("a", "A", Work::Install, 1), |_| {}).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (b_got, b_done, b) = enter_in_thread(&queue, request("b", "B", Work::Install, 1), seen.clone());
        until("b waits", || queue.waiting() == 1);
        let (c_got, c_done, c) = enter_in_thread(&queue, request("c", "C", Work::Uninstall, 3), seen);
        until("c waits", || queue.waiting() == 4);
        drop(first);
        until("b goes first", || b_got.load(Ordering::SeqCst));
        assert!(!c_got.load(Ordering::SeqCst));
        drop(b_done);
        until("then c", || c_got.load(Ordering::SeqCst));
        drop(c_done);
        assert!(b.join().unwrap() && c.join().unwrap());
    }

    #[test]
    fn withdrawing_an_addon_removes_its_waiting_requests_only() {
        let queue = InstallQueue::default();
        let held = queue.enter(request("drivers", "Drivers", Work::Install, 1), |_| {}).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (_, _done, apps) = enter_in_thread(&queue, request("installed-apps", "Installed Apps", Work::Uninstall, 2), seen);
        until("it waits", || queue.waiting() == 2);

        queue.withdraw("installed-apps");
        assert!(!apps.join().unwrap(), "the withdrawn request never runs");
        assert_eq!(queue.waiting(), 0);
        queue.withdraw("drivers");
        assert_eq!(queue.holder().map(|h| h.addon), Some("drivers"), "the job holding the queue goes on");
        drop(held);
    }

    #[test]
    fn every_job_that_ends_is_noted_once_for_the_host() {
        let queue = InstallQueue::default();
        assert!(queue.take_finished().is_empty());
        drop(queue.enter(request("drivers", "Drivers", Work::Install, 1), |_| {}).unwrap());
        drop(queue.enter(request("installed-apps", "Installed Apps", Work::Update, 1), |_| {}).unwrap());
        assert_eq!(queue.take_finished(), ["drivers", "installed-apps"]);
        assert!(queue.take_finished().is_empty(), "each ending is told once");
    }

    #[test]
    fn a_turn_given_back_unused_frees_the_queue_without_a_finished_job() {
        let queue = InstallQueue::default();
        queue.enter(request("drivers", "Drivers", Work::Install, 1), |_| {}).unwrap().give_back();
        assert_eq!(queue.holder(), None);
        assert!(queue.take_finished().is_empty());
    }

    #[test]
    fn a_waiting_job_that_panics_leaves_the_queue_to_the_next_request() {
        let queue = InstallQueue::default();
        let held = queue.enter(request("a", "A", Work::Install, 1), |_| {}).unwrap();
        let q = queue.clone();
        let panicked = std::thread::spawn(move || q.enter(request("b", "B", Work::Install, 1), |_| panic!("the page broke")).is_some()).join();
        assert!(panicked.is_err());
        assert_eq!(queue.waiting(), 0, "its request is gone");
        drop(held);

        let (q, (tx, rx)) = (queue.clone(), std::sync::mpsc::channel());
        std::thread::spawn(move || {
            let _ = tx.send(q.enter(request("c", "C", Work::Install, 1), |_| {}).is_some());
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)), Ok(true), "the next request gets its turn");
    }

    #[test]
    fn a_request_cancelled_while_waiting_leaves_at_once() {
        let queue = InstallQueue::default();
        let held = queue.enter(request("a", "A", Work::Install, 1), |_| {}).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (q, flag) = (queue.clone(), cancelled.clone());
        let waiter = std::thread::spawn(move || q.enter_unless(request("b", "B", Work::Install, 1), || flag.load(Ordering::SeqCst), |_| {}).is_some());
        until("it waits", || queue.waiting() == 1);

        cancelled.store(true, Ordering::SeqCst);
        until("it leaves while the queue is still held", || waiter.is_finished());
        assert!(!waiter.join().unwrap(), "a cancelled request never runs");
        assert_eq!(queue.waiting(), 0);
        assert_eq!(queue.holder().map(|h| h.addon), Some("a"));
        drop(held);
    }

    #[test]
    fn one_item_is_singular_in_the_waiting_text() {
        let h = Holder { addon: "drivers", name: "Drivers", work: Work::Install, items: 1 };
        assert_eq!(h.text(), "Drivers is installing 1 item; this starts when it is done\u{2026}");
    }
}
