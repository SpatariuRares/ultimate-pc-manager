//! A cache of one fact per file path, read on a worker thread so the UI
//! and the sampler never wait for it: ask for paths, read what is known so
//! far. The signatures (`signature.rs`) and the descriptions
//! (`version_info.rs`) are both kept this way.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, MutexGuard};

/// How a path's fact is read: the real Windows call, or a fake in tests.
pub type Read<T> = fn(&str) -> T;

/// How the worker thread is started (a fake that fails in tests).
type Spawn = fn(&'static str, Box<dyn FnOnce() + Send>) -> std::io::Result<()>;

/// Starts `job` on a thread named `name`.
fn spawn_thread(name: &'static str, job: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
    std::thread::Builder::new().name(name.into()).spawn(job).map(|_| ())
}

/// What the worker has read, shared with the UI.
pub struct Found<T> {
    /// Each path read, with its fact.
    pub by_path: HashMap<String, T>,
    /// The paths asked for and not read yet.
    asked: HashSet<String>,
    /// How many facts were added so far (readers rebuild what they derive
    /// from `by_path` only when it changed).
    pub generation: u64,
}

impl<T> Default for Found<T> {
    fn default() -> Self {
        Found { by_path: HashMap::new(), asked: HashSet::new(), generation: 0 }
    }
}

/// The cache: [`PathCache::ask`] queues paths, [`PathCache::of`] reads.
pub struct PathCache<T> {
    found: Arc<Mutex<Found<T>>>,
    read: Read<T>,
    /// The worker thread's name.
    name: &'static str,
    spawn: Spawn,
    /// The worker's queue, started on the first ask (and again on a later
    /// ask when it could not be started or has stopped).
    queue: Mutex<Option<Sender<String>>>,
}

impl<T: Clone + Send + 'static> PathCache<T> {
    /// A cache whose worker, named `name`, reads each path with `read`.
    pub fn new(name: &'static str, read: Read<T>) -> Self {
        PathCache { found: Default::default(), read, name, spawn: spawn_thread, queue: Mutex::new(None) }
    }

    /// The facts known so far, locked.
    pub fn found(&self) -> MutexGuard<'_, Found<T>> {
        crate::platform::lock_state(&self.found)
    }

    /// Queues the paths not read or asked for yet; returns at once. When
    /// the worker cannot be started the paths are not left asked: the next
    /// ask tries again.
    pub fn ask<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        let mut f = self.found();
        let new: Vec<String> = paths
            .into_iter()
            .filter(|p| !p.is_empty() && !f.by_path.contains_key(*p) && !f.asked.contains(*p))
            .map(str::to_string)
            .collect();
        if new.is_empty() {
            return;
        }
        f.asked.extend(new.iter().cloned());
        drop(f);
        let mut queue = crate::platform::lock_state(&self.queue);
        let mut unsent = new;
        for _ in 0..2 {
            if queue.is_none() {
                *queue = self.start();
            }
            let Some(tx) = queue.as_ref() else { break };
            match unsent.iter().position(|p| tx.send(p.clone()).is_err()) {
                None => return,
                // The worker has stopped: start a new one for the rest.
                Some(at) => {
                    unsent.drain(..at);
                    *queue = None;
                }
            }
        }
        let mut f = self.found();
        for p in &unsent {
            f.asked.remove(p);
        }
    }

    /// The worker: reads each path asked for, one at a time. None when the
    /// thread could not be started.
    fn start(&self) -> Option<Sender<String>> {
        let (tx, rx) = channel::<String>();
        let found = Arc::clone(&self.found);
        let read = self.read;
        let job = move || {
            for path in rx {
                let fact = read(&path);
                let mut f = crate::platform::lock_state(&found);
                f.asked.remove(&path);
                f.by_path.insert(path, fact);
                f.generation += 1;
            }
        };
        (self.spawn)(self.name, Box::new(job)).ok().map(|()| tx)
    }

    /// The fact of `path`: None while it is not read yet.
    pub fn of(&self, path: &str) -> Option<T> {
        self.found().by_path.get(path).cloned()
    }

    /// A cache whose worker thread can never start (tests).
    #[cfg(test)]
    fn failing_to_start(read: Read<T>) -> Self {
        fn refuse(_: &'static str, _: Box<dyn FnOnce() + Send>) -> std::io::Result<()> {
            Err(std::io::Error::other("no thread"))
        }
        PathCache { spawn: refuse, ..PathCache::new("test", read) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn length(path: &str) -> usize {
        std::thread::sleep(Duration::from_millis(100));
        path.len()
    }

    fn wait_for(c: &PathCache<usize>, path: &str) -> Option<usize> {
        let until = Instant::now() + Duration::from_secs(5);
        while c.of(path).is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        c.of(path)
    }

    #[test]
    fn asking_returns_at_once_and_each_path_is_read_once_on_the_worker() {
        let c = PathCache::new("test-cache", length);
        let start = Instant::now();
        c.ask(["abc", "de", "abc", ""]);
        assert!(start.elapsed() < Duration::from_millis(80), "asking does not wait for the reads");
        assert_eq!(c.of("abc"), None, "not read yet");
        assert_eq!(wait_for(&c, "de"), Some(2));
        assert_eq!(wait_for(&c, "abc"), Some(3));
        assert_eq!(c.of(""), None, "an empty path is never asked");
        assert_eq!(c.found().generation, 2, "abc is read once");
    }

    #[test]
    fn a_worker_that_cannot_start_leaves_nothing_asked_and_the_next_ask_tries_again() {
        let mut c = PathCache::failing_to_start(length);
        c.ask(["abc"]);
        assert!(c.found().asked.is_empty(), "abc is not stuck as asked");
        assert_eq!(c.of("abc"), None);
        // Threads can start again: the same path is read on the next ask.
        c.spawn = spawn_thread;
        c.ask(["abc"]);
        assert_eq!(wait_for(&c, "abc"), Some(3));
    }
}
