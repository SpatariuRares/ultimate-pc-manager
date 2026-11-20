//! The connection worker: a thread that reads the inventory at the app's
//! update speed and on Refresh, off the drawing thread. Updates paused
//! (interval 0), it reads only on Refresh. Dropping it stops the thread and
//! waits for it.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::inventory::Inventory;
use crate::platform::lock_state;

/// Reads an inventory (the system's, or a test's).
pub type Reader = Arc<dyn Fn() -> Inventory + Send + Sync>;

/// What the page asks of the thread.
#[derive(Default)]
struct Control {
    quit: bool,
    refresh: bool,
    /// The app's update interval; 0 = paused.
    interval_ms: u64,
}

/// The state shared with the thread.
#[derive(Default)]
struct Shared {
    control: Mutex<Control>,
    wake: Condvar,
    latest: Mutex<Option<Arc<Inventory>>>,
}

/// The running worker.
pub struct Worker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts the thread; it reads at once when `read_now`, else first at
    /// the interval (or on Refresh when paused).
    pub fn start(reader: Reader, interval_ms: u64, read_now: bool) -> Worker {
        let shared = Arc::new(Shared::default());
        {
            let mut c = lock_state(&shared.control);
            c.interval_ms = interval_ms;
            c.refresh = read_now;
        }
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("connection worker".into())
            .spawn(move || run(&s, &*reader))
            .ok();
        Worker { shared, thread }
    }

    /// The last inventory read, if any.
    pub fn latest(&self) -> Option<Arc<Inventory>> {
        lock_state(&self.shared.latest).clone()
    }

    /// Follows the app's update interval (0 = paused).
    pub fn set_interval(&self, interval_ms: u64) {
        let mut c = lock_state(&self.shared.control);
        if c.interval_ms != interval_ms {
            c.interval_ms = interval_ms;
            self.shared.wake.notify_all();
        }
    }

    /// Reads again now.
    pub fn refresh(&self) {
        lock_state(&self.shared.control).refresh = true;
        self.shared.wake.notify_all();
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        lock_state(&self.shared.control).quit = true;
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The thread: waits for Refresh or the interval, reads, publishes; until
/// asked to quit.
fn run(shared: &Shared, reader: &dyn Fn() -> Inventory) {
    let mut last = Instant::now();
    loop {
        {
            let mut c = lock_state(&shared.control);
            loop {
                if c.quit {
                    return;
                }
                if c.refresh {
                    c.refresh = false;
                    break;
                }
                if c.interval_ms == 0 {
                    c = shared.wake.wait(c).unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                let due = last + Duration::from_millis(c.interval_ms);
                let now = Instant::now();
                if now >= due {
                    break;
                }
                c = shared.wake.wait_timeout(c, due - now).unwrap_or_else(|e| e.into_inner()).0;
            }
        }
        let inventory = Arc::new(reader());
        last = Instant::now();
        if lock_state(&shared.control).quit {
            return;
        }
        *lock_state(&shared.latest) = Some(inventory);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::assemble;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A reader of empty inventories that counts its reads.
    fn counting() -> (Reader, Arc<AtomicUsize>) {
        let reads = Arc::new(AtomicUsize::new(0));
        let r = reads.clone();
        let reader: Reader = Arc::new(move || {
            r.fetch_add(1, Ordering::SeqCst);
            assemble([Ok(vec![]), Ok(vec![]), Ok(vec![]), Ok(vec![])])
        });
        (reader, reads)
    }

    /// Waits up to two seconds for `done`.
    fn eventually(done: impl Fn() -> bool) -> bool {
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(2) {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn it_reads_at_once_and_publishes_the_inventory() {
        let (reader, reads) = counting();
        let w = Worker::start(reader, 1000, true);
        assert!(eventually(|| w.latest().is_some()));
        assert_eq!(reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn it_reads_again_at_the_update_interval() {
        let (reader, reads) = counting();
        let _w = Worker::start(reader, 10, true);
        assert!(eventually(|| reads.load(Ordering::SeqCst) >= 4));
    }

    #[test]
    fn paused_it_reads_only_on_refresh() {
        let (reader, reads) = counting();
        let w = Worker::start(reader, 0, true);
        assert!(eventually(|| reads.load(Ordering::SeqCst) == 1));
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::SeqCst), 1, "paused: no further reading");
        w.refresh();
        assert!(eventually(|| reads.load(Ordering::SeqCst) == 2));
    }

    #[test]
    fn started_paused_without_reading_now_it_waits_for_refresh() {
        let (reader, reads) = counting();
        let w = Worker::start(reader, 0, false);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        w.refresh();
        assert!(eventually(|| reads.load(Ordering::SeqCst) == 1));
    }

    #[test]
    fn unpausing_resumes_reading() {
        let (reader, reads) = counting();
        let w = Worker::start(reader, 0, true);
        assert!(eventually(|| reads.load(Ordering::SeqCst) == 1));
        w.set_interval(10);
        assert!(eventually(|| reads.load(Ordering::SeqCst) >= 3));
    }

    #[test]
    fn dropped_it_stops_reading() {
        let (reader, reads) = counting();
        let w = Worker::start(reader, 10, true);
        assert!(eventually(|| reads.load(Ordering::SeqCst) >= 1));
        drop(w);
        let after = reads.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(reads.load(Ordering::SeqCst), after);
    }

    #[test]
    fn it_reads_off_the_calling_thread() {
        let here = std::thread::current().id();
        let seen = Arc::new(Mutex::new(None));
        let s = seen.clone();
        let reader: Reader = Arc::new(move || {
            *s.lock().unwrap() = Some(std::thread::current().id());
            assemble([Ok(vec![]), Ok(vec![]), Ok(vec![]), Ok(vec![])])
        });
        let w = Worker::start(reader, 1000, true);
        assert!(eventually(|| w.latest().is_some()));
        assert_ne!(*seen.lock().unwrap(), Some(here));
    }
}
