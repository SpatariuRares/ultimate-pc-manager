//! Analyze wait chain: Wait Chain Traversal over every thread of a
//! process, run on a worker thread, and a window with six counts.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Duration;

use egui::{vec2, RichText, Ui};

use crate::proc_info::Missing;
use crate::sampler::ProcRow;
use crate::theme;

/// The window's title, before the process's name.
pub const TITLE: &str = "Analyze wait chain";
/// Shown while the threads are walked.
pub const LOADING: &str = "Loading";
/// The six counts, in the order the window lists them.
pub const LABELS: [&str; 6] = ["Threads analyzed", "Threads refused", "Threads skipped", "Blocked wait chains", "Deadlock cycles", "Chains truncated"];

/// One node of a wait chain, as far as the counts need it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Node {
    /// A thread (WctThreadType), not an object it waits on.
    pub thread: bool,
    /// Its status is WctStatusBlocked.
    pub blocked: bool,
}

/// What Wait Chain Traversal gave for one thread.
#[derive(Clone, Debug, PartialEq)]
pub enum ThreadChain {
    /// The chain, starting with the thread itself; `cycle` when Windows
    /// found a deadlock cycle, `cut` when the chain was longer than
    /// WCT_MAX_NODE_COUNT nodes.
    Chain { nodes: Vec<Node>, cycle: bool, cut: bool },
    /// Windows refused to open the thread (access denied).
    Refused,
    /// Any other failure, e.g. the thread ended meanwhile.
    Skipped,
}

/// The six counts of an analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Counts {
    pub analyzed: u32,
    pub refused: u32,
    pub skipped: u32,
    pub blocked: u32,
    pub deadlocks: u32,
    pub truncated: u32,
}

impl Counts {
    /// The counts under their labels, in [`LABELS`]' order.
    pub fn rows(&self) -> [(&'static str, u32); 6] {
        let values = [self.analyzed, self.refused, self.skipped, self.blocked, self.deadlocks, self.truncated];
        [0, 1, 2, 3, 4, 5].map(|i| (LABELS[i], values[i]))
    }
}

/// Counts the chains of a process's threads. A chain is blocked when its
/// thread is blocked on something another thread holds (a later node is a
/// thread); a wait on an event or a timer alone is not.
pub fn count(chains: &[ThreadChain]) -> Counts {
    let mut c = Counts::default();
    for chain in chains {
        match chain {
            ThreadChain::Refused => c.refused += 1,
            ThreadChain::Skipped => c.skipped += 1,
            ThreadChain::Chain { nodes, cycle, cut } => {
                c.analyzed += 1;
                let blocked = nodes.first().is_some_and(|n| n.blocked) && nodes.iter().skip(1).any(|n| n.thread);
                c.blocked += u32::from(blocked);
                c.deadlocks += u32::from(*cycle);
                c.truncated += u32::from(*cut);
            }
        }
    }
    c
}

/// Walks the wait chain of every thread of `pid`, the threads taken from
/// the system's process list (blocking: run it off the UI thread).
pub fn analyze(pid: u32) -> Result<Counts, Missing> {
    let ids = crate::proc_info::thread_ids(pid).ok_or(Missing::Exited)?;
    imp::chains(&ids).map(|chains| count(&chains))
}

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

/// The egui id of the wait chain window of `pid`.
pub fn window_id(pid: u32) -> egui::Id {
    egui::Id::new(("wait-chain", pid))
}

/// One Analyze wait chain window.
pub struct WaitChain {
    pub pid: u32,
    /// The process's name when the analysis started.
    name: String,
    /// The last analysis; `None` while it runs.
    result: Option<Result<Counts, Missing>>,
    /// The analysis under way, on its own thread.
    running: Option<Receiver<Result<Counts, Missing>>>,
}

impl WaitChain {
    /// A window analysing `pid` (named from `procs`) on a worker thread.
    pub fn open(pid: u32, procs: &[ProcRow]) -> Self {
        let mut w = WaitChain { pid, name: String::new(), result: None, running: None };
        w.rerun(procs);
        w
    }

    /// Analyses the process again, unless an analysis is under way.
    pub fn rerun(&mut self, procs: &[ProcRow]) {
        let pid = self.pid;
        self.run_with(procs, move || analyze(pid));
    }

    /// [`WaitChain::rerun`] with `job` as the analysis.
    fn run_with(&mut self, procs: &[ProcRow], job: impl FnOnce() -> Result<Counts, Missing> + Send + 'static) {
        if self.running.is_some() {
            return;
        }
        if let Some(p) = procs.iter().find(|p| p.pid == self.pid) {
            self.name = p.name.clone();
        }
        self.result = None;
        let (tx, rx) = channel();
        let started = std::thread::Builder::new().name("wait-chain".into()).spawn(move || {
            let _ = tx.send(job());
        });
        match started {
            Ok(_) => self.running = Some(rx),
            Err(_) => self.result = Some(Err(Missing::Unavailable)),
        }
    }

    /// Takes the worker's result when it is done.
    fn poll(&mut self) {
        let Some(rx) = &self.running else { return };
        match rx.try_recv() {
            Ok(r) => {
                self.result = Some(r);
                self.running = None;
            }
            Err(TryRecvError::Disconnected) => {
                self.result = Some(Err(Missing::Unavailable));
                self.running = None;
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Draws the window; false once the user closed it. Never waits for
    /// the analysis.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        self.poll();
        if self.running.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = true;
        let title = if self.name.is_empty() { TITLE.to_string() } else { format!("{TITLE} \u{b7} {}", self.name) };
        egui::Window::new(title).id(window_id(self.pid)).open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| self.body(ui));
        open
    }

    /// The window's content: the six counts, or why there are none.
    fn body(&self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        let soft = |ui: &mut Ui, text: &str| ui.label(RichText::new(text).font(t.fonts.row.clone()).color(t.soft));
        match &self.result {
            None => {
                soft(ui, LOADING);
            }
            Some(Err(m)) => {
                soft(ui, crate::inspector::missing_text(*m));
            }
            Some(Ok(c)) => {
                egui::Grid::new(("wait-chain-counts", self.pid)).num_columns(2).spacing(vec2(10.0, 3.0)).show(ui, |ui| {
                    for (i, (label, value)) in c.rows().into_iter().enumerate() {
                        ui.label(RichText::new(label).font(t.fonts.row.clone()).color(t.label));
                        // Blocked wait chains and Deadlock cycles: green at
                        // none, red otherwise.
                        let colour = match i {
                            3 | 4 if value == 0 => t.green,
                            3 | 4 => t.red,
                            _ => t.text,
                        };
                        ui.label(RichText::new(value.to_string()).font(t.fonts.row.clone()).color(colour));
                        ui.end_row();
                    }
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Mode;

    fn chain(nodes: &[(bool, bool)], cycle: bool, cut: bool) -> ThreadChain {
        ThreadChain::Chain { nodes: nodes.iter().map(|&(thread, blocked)| Node { thread, blocked }).collect(), cycle, cut }
    }

    #[test]
    fn the_counts_come_from_the_chains_of_the_threads() {
        let running = chain(&[(true, false)], false, false);
        // Waits on an event: blocked, but on no other thread.
        let idle = chain(&[(true, true), (false, false)], false, false);
        // Waits on a lock another thread owns.
        let waits_on_thread = chain(&[(true, true), (false, false), (true, false)], false, false);
        // Two threads each holding what the other waits for.
        let deadlock = chain(&[(true, true), (false, false), (true, true), (false, false), (true, true)], true, false);
        let long = chain(&[(true, true), (false, false), (true, true)], false, true);
        let chains = [running, idle, waits_on_thread, deadlock, long, ThreadChain::Refused, ThreadChain::Refused, ThreadChain::Skipped];
        assert_eq!(count(&chains), Counts { analyzed: 5, refused: 2, skipped: 1, blocked: 3, deadlocks: 1, truncated: 1 });
        assert_eq!(count(&[]), Counts::default());
    }

    #[test]
    fn the_rows_are_the_six_labels_in_order() {
        let c = Counts { analyzed: 49, refused: 1, skipped: 2, blocked: 3, deadlocks: 4, truncated: 5 };
        assert_eq!(
            c.rows(),
            [("Threads analyzed", 49), ("Threads refused", 1), ("Threads skipped", 2), ("Blocked wait chains", 3), ("Deadlock cycles", 4), ("Chains truncated", 5)]
        );
    }

    #[cfg(windows)]
    #[test]
    fn analysing_the_tests_own_process_counts_its_threads() {
        let c = analyze(std::process::id()).expect("the test's own process can be analysed");
        assert!(c.analyzed >= 1, "{c:?}");
        assert_eq!(c.deadlocks, 0, "{c:?}");
    }

    #[cfg(windows)]
    #[test]
    fn two_analyses_at_once_both_finish() {
        // Two windows may analyse at the same time; Wait Chain Traversal
        // run on two threads of one process at once crashes it.
        let others: Vec<_> = (0..3).map(|_| std::thread::spawn(|| analyze(std::process::id()))).collect();
        let mine = analyze(std::process::id());
        for other in others {
            assert!(other.join().unwrap().is_ok_and(|c| c.analyzed >= 1));
        }
        assert!(mine.is_ok_and(|c| c.analyzed >= 1));
    }

    #[cfg(windows)]
    #[test]
    fn a_process_that_is_not_running_says_so() {
        // PIDs are multiples of 4; this one is never given.
        assert_eq!(analyze(3), Err(Missing::Exited));
    }

    /// The texts of two frames of `w` (a window is sized on its first).
    fn texts(ctx: &egui::Context, w: &mut WaitChain) -> Vec<String> {
        crate::test_ui::run(ctx, (900.0, 700.0), vec![], |ui| {
            w.show(ui.ctx());
        });
        let out = crate::test_ui::run(ctx, (900.0, 700.0), vec![], |ui| {
            w.show(ui.ctx());
        });
        crate::test_ui::texts(&out)
    }

    fn steam() -> Vec<ProcRow> {
        vec![ProcRow { pid: 5120, name: "steam.exe".into(), ..Default::default() }]
    }

    #[test]
    fn the_window_stays_drawn_while_the_analysis_runs_then_shows_all_six_counts() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let (go, wait) = channel::<()>();
        let mut w = WaitChain { pid: 5120, name: String::new(), result: None, running: None };
        w.run_with(&steam(), move || {
            let _ = wait.recv();
            Ok(Counts { analyzed: 49, refused: 1, skipped: 2, blocked: 3, deadlocks: 0, truncated: 5 })
        });
        // The worker is held: a frame is drawn at once, saying it runs.
        let started = std::time::Instant::now();
        let drawn = texts(&ctx, &mut w);
        assert!(started.elapsed() < Duration::from_secs(2), "a frame never waits for the analysis");
        assert!(drawn.contains(&"Analyze wait chain \u{b7} steam.exe".to_string()), "{drawn:?}");
        assert!(drawn.contains(&LOADING.to_string()), "{drawn:?}");
        go.send(()).unwrap();
        let mut drawn = Vec::new();
        for _ in 0..200 {
            drawn = texts(&ctx, &mut w);
            if !drawn.contains(&LOADING.to_string()) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let at = |s: &str| drawn.iter().position(|t| t == s).unwrap_or_else(|| panic!("{s} not in {drawn:?}"));
        for (label, value) in [("Threads analyzed", "49"), ("Threads refused", "1"), ("Threads skipped", "2"), ("Blocked wait chains", "3"), ("Deadlock cycles", "0"), ("Chains truncated", "5")] {
            assert_eq!(drawn[at(label) + 1], value, "{label} {drawn:?}");
        }
        let order: Vec<usize> = LABELS.iter().map(|l| at(l)).collect();
        assert!(order.windows(2).all(|p| p[0] < p[1]), "in the listed order: {drawn:?}");
    }

    #[test]
    fn a_failed_analysis_says_why_instead_of_counts() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut w = WaitChain { pid: 5120, name: "steam.exe".into(), result: Some(Err(Missing::Exited)), running: None };
        let drawn = texts(&ctx, &mut w);
        assert!(drawn.contains(&crate::inspector::EXITED.to_string()), "{drawn:?}");
        assert!(!drawn.contains(&"Threads analyzed".to_string()), "{drawn:?}");
    }
}
