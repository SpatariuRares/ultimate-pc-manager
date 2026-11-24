//! The Benchmarks add-on: quick CPU, GPU, memory and disk speed tests, run by
//! the benchmark [`engine`] one at a time in its own thread. A running test
//! can be cancelled; turning the add-on off cancels it and waits for its
//! thread to end.

mod disk;
mod engine;
mod gpu;
mod history;
mod internet;
mod page;
mod score;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui};

use crate::addons::{is_default, Addon, Consumption, Requirements};
use crate::platform::{AddonContext, PageContext};
use crate::theme::{self, Theme};
use crate::widgets::{self, parts};

use disk::DiskJob;
use engine::{Engine, Measure, Probe, Unit, GBS, GOPS, MBITS, MBS, MOPS};
use history::History;
use page::View;

/// One of the speed tests the page offers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Test {
    Cpu,
    Gpu,
    Memory,
    Disk,
    Internet,
}

impl Test {
    /// Its short name, as in "%1 test failed: %2".
    pub fn name(self) -> &'static str {
        match self {
            Test::Cpu => "CPU",
            Test::Gpu => gpu::NAME,
            Test::Memory => "Memory",
            Test::Disk => "Disk",
            Test::Internet => "Internet",
        }
    }

    /// What it does, in one line.
    pub fn about(self) -> &'static str {
        match self {
            Test::Cpu => "Math on one core for 2 seconds, then on every core for 2 seconds.",
            Test::Gpu => "Runs a Direct3D 11 compute shader, then draws with a pixel shader, on the chosen video card: about 4 seconds.",
            Test::Memory => "Copies a 256 MB block eight times.",
            Test::Disk => "Writes and reads a 256 MB file in the test folder, then deletes it.",
            Test::Internet => "Downloads, then uploads, blocks of growing size. Only when you press Start.",
        }
    }

    /// The figures it shows, in order, with their units.
    pub fn figures(self) -> &'static [(&'static str, Measure)] {
        match self {
            Test::Cpu => &[(SINGLE_CORE, MOPS), (MULTI_CORE, GOPS)],
            Test::Gpu => &[(gpu::COMPUTE, gpu::GFLOPS), (gpu::GRAPHICS, gpu::GPIXELS)],
            Test::Memory => &[(COPY_SPEED, GBS)],
            Test::Disk => &[(WRITE_SPEED, MBS), (READ_SPEED, MBS)],
            Test::Internet => &[(internet::DOWNLOAD, MBITS), (internet::UPLOAD, MBITS)],
        }
    }

    /// The engine unit that runs it (the GPU test on the high-performance card).
    fn unit(self) -> Unit {
        match self {
            Test::Cpu => Unit::new(self.name(), |p| cpu(p, CPU_SECONDS)),
            Test::Gpu => gpu::unit(gpu::adapters().into_iter().next()),
            Test::Memory => Unit::new(self.name(), |p| memory(p, MEMORY_BLOCK, MEMORY_ROUNDS)),
            Test::Disk => disk_unit(disk::test_folder(None, disk::default_folder())),
            Test::Internet => Unit::new(self.name(), |p| internet::run(p, &mut internet::Cloudflare::new(), &internet::PLAN)),
        }
    }
}

/// The tests "Run All Benchmarks" runs, in order.
pub const RUN_ALL: [Test; 4] = [Test::Cpu, Test::Gpu, Test::Disk, Test::Internet];

/// The warning shown in a debug build, whose numbers mean nothing.
pub const DEBUG_BUILD: &str = "You are running a debug build, benchmarks are not valid";

/// The debug-build warning, in a debug build only.
pub fn debug_build_warning() -> Option<&'static str> {
    cfg!(debug_assertions).then_some(DEBUG_BUILD)
}

/// How the last run of a test ended, as its rail entry tells.
#[derive(Clone, Copy, Debug, PartialEq)]
enum RunEnd {
    /// Completed at this local time (seconds since 1970).
    Complete { at: i64 },
    Cancelled,
    Failed,
}

/// The add-on: the engine that runs its tests and keeps their results,
/// and what its page shows.
#[derive(Default)]
pub struct Benchmarks {
    engine: Engine,
    /// The rail entry whose view is shown.
    selected: View,
    /// The tests of the run started from this page, in order, until their
    /// ends are noted (more than one: "Run All").
    watching: Vec<Test>,
    /// How the last run of each test ended.
    ended: HashMap<&'static str, RunEnd>,
    /// "AMD Ryzen 5 7600X · 6 cores, 12 threads", read once and on Refresh.
    cpu_line: Option<String>,
    /// The video cards DXGI lists, read once and on Refresh.
    adapters: Option<Vec<gpu::Adapter>>,
    /// The LUID of the card chosen for the GPU test (none: the high-performance one).
    gpu_choice: Option<i64>,
    /// The name of the card chosen for the GPU test, as saved: LUIDs change
    /// on every boot, so a saved choice is found again by its name.
    gpu_name: Option<String>,
    /// The card the last GPU run was started on.
    gpu_tested: Option<String>,
    /// The disk test's folder, as chosen (none: the temp folder).
    disk_folder: Option<String>,
    /// The Summary tile is on (off by default).
    summary_tile: bool,
    /// The settings as loaded, to write back the fields it does not know.
    saved: serde_json::Value,
    /// The platform (its folder picker), once started.
    context: Option<AddonContext>,
    /// The completed runs, read when the add-on starts.
    history: History,
    /// The history file to use instead of the app's (tests); kept when the
    /// add-on is turned off.
    history_file: Option<PathBuf>,
    /// Why the history could not be read or saved, shown in the band.
    history_warning: Option<String>,
    /// The Clear History confirmation is open (for the selected test).
    clear_asked: bool,
    /// A debug build, whose numbers mean nothing: its runs are shown but
    /// never saved (set when the add-on starts).
    debug_build: bool,
    /// The total RAM, for the Memory hardware line, read once and on
    /// Refresh.
    mem_total: Option<u64>,
    /// The widgets of the last frame, for the keyboard test.
    #[cfg(test)]
    drawn: Drawn,
}

/// The ids of the rail entries and of Start in the last frame.
#[cfg(test)]
#[derive(Default)]
struct Drawn {
    rail: Vec<egui::Id>,
    start: Option<egui::Id>,
}

impl Benchmarks {
    /// Starts `test` unless one is running.
    pub fn run(&mut self, test: Test) -> bool {
        let unit = self.unit_of(test);
        self.launch(test, unit)
    }

    /// The engine unit of `test` as chosen on this page: the GPU test on the
    /// chosen card, the disk test in the test folder.
    fn unit_of(&mut self, test: Test) -> Unit {
        match test {
            Test::Gpu => gpu::unit(self.gpu_card()),
            Test::Disk => disk_unit(self.test_folder()),
            _ => test.unit(),
        }
    }

    /// The disk test's folder: the chosen one, else the temp folder.
    fn test_folder(&self) -> Result<std::path::PathBuf, &'static str> {
        disk::test_folder(self.disk_folder.as_deref(), disk::default_folder())
    }

    /// The video cards, listed on first use.
    fn adapters(&mut self) -> &[gpu::Adapter] {
        self.adapters.get_or_insert_with(gpu::adapters)
    }

    /// The card the GPU test would run on now.
    fn gpu_card(&mut self) -> Option<gpu::Adapter> {
        let (choice, name) = (self.gpu_choice, self.gpu_name.clone());
        let cards = self.adapters();
        let choice = choice.or_else(|| cards.iter().find(|c| Some(&c.name) == name.as_ref()).map(|c| c.luid));
        gpu::chosen(cards, choice).cloned()
    }

    /// Chooses `card` for the GPU test (saved by its name).
    fn choose_card(&mut self, card: &gpu::Adapter) {
        self.gpu_choice = Some(card.luid);
        self.gpu_name = Some(card.name.clone());
    }

    /// Starts `unit` as the run of `test` unless one is running.
    fn launch(&mut self, test: Test, unit: Unit) -> bool {
        self.launch_all(vec![(test, unit)])
    }

    /// Starts the runs of `units`, in order, unless one is running; notes
    /// the card a GPU run starts on.
    fn launch_all(&mut self, units: Vec<(Test, Unit)>) -> bool {
        self.settle();
        let (tests, units): (Vec<Test>, Vec<Unit>) = units.into_iter().unzip();
        let card = tests.contains(&Test::Gpu).then(|| self.gpu_card().map(|c| c.name));
        let started = self.engine.start(units);
        if started {
            if let Some(card) = card {
                self.gpu_tested = card;
            }
            self.watching = tests;
        }
        started
    }

    /// "Run All Benchmarks": CPU, GPU (on the chosen card), Disk (in the
    /// test folder) and Internet, in a row, unless a test is running.
    pub fn run_all(&mut self) -> bool {
        self.run_all_with(Self::unit_of)
    }

    /// Runs the [`RUN_ALL`] tests in a row, each as `unit` builds it,
    /// unless a test is running.
    fn run_all_with(&mut self, mut unit: impl FnMut(&mut Self, Test) -> Unit) -> bool {
        if !self.can_start() {
            return false;
        }
        let units = RUN_ALL.iter().map(|&test| (test, unit(self, test))).collect();
        self.launch_all(units)
    }

    /// While "Run All" runs: "Running all benchmarks: CPU done, now
    /// testing GPU graphics".
    fn run_all_line(&self, st: &engine::EngineState) -> Option<String> {
        if self.watching.len() < 2 {
            return None;
        }
        let running = st.running?;
        let now = match st.phase {
            Some(phase) => {
                let mut chars = phase.chars();
                chars.next().map_or_else(String::new, |c| c.to_lowercase().chain(chars).collect())
            }
            None => format!("testing {running}"),
        };
        // "CPU done, GPU failed": the tests that ended so far, as they ended.
        let ended: Vec<String> = st
            .ended
            .iter()
            .map(|(name, outcome)| match outcome {
                engine::Outcome::Complete => format!("{name} done"),
                engine::Outcome::Failed { .. } => format!("{name} failed"),
                engine::Outcome::Cancelled => format!("{name} cancelled"),
            })
            .collect();
        Some(if ended.is_empty() {
            format!("Running all benchmarks: now {now}")
        } else {
            format!("Running all benchmarks: {}, now {now}", ended.join(", "))
        })
    }

    /// A test can be started now (no test is running).
    pub fn can_start(&self) -> bool {
        !self.engine.busy()
    }

    /// Asks the running test to stop.
    pub fn cancel(&self) {
        self.engine.cancel();
    }

    /// Notes how each watched test ended, once the run has; tests a cancel
    /// kept from starting are left as they were.
    fn settle(&mut self) {
        if self.engine.busy() || self.watching.is_empty() {
            return;
        }
        let st = self.engine.state();
        let at = crate::platform::local_secs();
        for test in std::mem::take(&mut self.watching) {
            let Some((_, outcome)) = st.ended.iter().find(|(name, _)| *name == test.name()) else { continue };
            let end = match outcome {
                engine::Outcome::Complete => RunEnd::Complete { at },
                engine::Outcome::Failed { .. } => RunEnd::Failed,
                engine::Outcome::Cancelled => RunEnd::Cancelled,
            };
            if let RunEnd::Complete { at } = end {
                if !self.debug_build {
                    self.save_run(test, st.results(test.name()), at);
                }
            }
            self.ended.insert(test.name(), end);
        }
    }

    /// Adds a completed run of `test` to the history.
    fn save_run(&mut self, test: Test, figures: &[engine::Figure], at: i64) {
        let run = history::Run {
            test: test.name().to_string(),
            date: crate::platform::iso_text(at),
            figures: figures.iter().map(|f| history::SavedFigure { name: f.name.into(), value: f.value, unit: f.unit.label.into() }).collect(),
            score: score::test_score(test, figures.iter().map(|f| (f.name, f.value))),
            version: env!("CARGO_PKG_VERSION").to_string(),
            platform: format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
            hardware: self.hardware(test),
        };
        if let Err(e) = self.history.add(run) {
            self.history_warning = Some(e);
        }
    }

    /// Reads the history file (and its warning, if it could not be read).
    fn read_history(&mut self) {
        let (history, warning) = History::open(self.history_file.clone().or_else(app_history_file));
        self.history = history;
        self.history_warning = warning;
    }

    /// The name of the card the GPU test would run on now.
    fn card_name(&mut self) -> String {
        self.gpu_card().map_or_else(|| NO_CARD.to_string(), |c| c.name)
    }

    /// The hardware `test` measures: the CPU line, the total RAM, the card
    /// the last GPU run started on or the test folder.
    fn hardware(&mut self, test: Test) -> String {
        match test {
            Test::Cpu => self.cpu_line.get_or_insert_with(cpu_line).clone(),
            Test::Memory => format!("{} RAM", crate::widgets::fmt_bytes(*self.mem_total.get_or_insert_with(total_ram))),
            Test::Gpu => match self.gpu_tested.clone() {
                Some(name) => name,
                None => self.card_name(),
            },
            Test::Disk => match self.test_folder() {
                Ok(folder) => format!("Test folder {}", folder.display()),
                Err(e) => e.to_string(),
            },
            Test::Internet => "This connection".to_string(),
        }
    }

    /// The last saved run of `test`: its first figure and when it ran.
    fn last_saved(&self, test: Test) -> Option<(Option<String>, i64)> {
        let run = *self.history.runs(test.name()).first()?;
        let at = crate::platform::iso_secs(&run.date)?;
        let figure = run.figures.first().map(|f| match test.figures().iter().find(|(n, _)| *n == f.name) {
            Some(&(name, unit)) => engine::Figure { name, value: f.value, unit }.text(),
            None => format!("{} {}", f.value, f.unit),
        });
        Some((figure, at))
    }

    /// The last saved run of `test` that has a score.
    fn last_scored(&self, test: Test) -> Option<&history::Run> {
        self.history.runs(test.name()).into_iter().find(|r| r.score.is_some())
    }

    /// The overall score (of the last scored run of each scored test) and
    /// the date of the newest run it counts.
    fn overall_score(&self) -> Option<(f64, String)> {
        let runs: Vec<_> = score::SCORED.iter().filter_map(|&t| self.last_scored(t)).collect();
        let overall = score::overall(runs.iter().filter_map(|r| r.score))?;
        let newest = runs.iter().map(|r| &r.date).max()?;
        Some((overall, newest.clone()))
    }

    /// The Summary tile's lines, while it is on: "Ultimate score 123" and
    /// "Last run 2026-11-17 21:30", or "No score yet".
    pub fn summary_lines(&self) -> Option<Vec<String>> {
        if !self.summary_tile {
            return None;
        }
        Some(match self.overall_score() {
            Some((overall, date)) => vec![format!("{} {}", score::NAME, score::text(overall)), format!("Last run {}", short_date(&date))],
            None => vec![NO_SCORE.to_string()],
        })
    }

    /// The Score view: the overall score, each scored test's score with a
    /// bar that marks 100, the reference machine and the version note.
    fn score_view(&self, ui: &mut Ui) {
        let t = theme::of_ui(ui);
        let overall = self.overall_score();
        let card = egui::Frame::new()
            .fill(t.panel)
            .stroke(egui::Stroke::new(1.0, t.line))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::same(20));
        card.show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 14.0;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                match &overall {
                    Some((s, _)) => ui.label(RichText::new(score::text(*s)).font(widgets::display_font(ui, 52.0)).color(t.amber)),
                    None => ui.label(RichText::new(NOT_RUN).size(20.0).color(t.label)),
                };
                ui.label(RichText::new(SCORE_HOW).color(t.soft));
            });
            if let Some((_, date)) = &overall {
                ui.label(RichText::new(format!("Last run {}", short_date(date))).color(t.label));
            }
            let bar_width = (ui.available_width() - 56.0 - 90.0 - 28.0).max(80.0);
            egui::Grid::new("bench-scores").num_columns(3).min_col_width(56.0).spacing([14.0, 12.0]).show(ui, |ui| {
                for test in score::SCORED {
                    ui.label(RichText::new(test.name()).color(t.soft));
                    let s = self.last_scored(test).and_then(|r| r.score);
                    let (fill, mark) = score::bar(s.unwrap_or(0.0));
                    parts::bullet_bar(ui, bar_width, fill, test_color(&t, test), Some(mark), None);
                    match s {
                        Some(s) => ui.label(RichText::new(score::text(s)).font(widgets::display_font(ui, 14.0)).color(t.text)),
                        None => ui.label(RichText::new(NOT_RUN).color(t.label)),
                    };
                    ui.end_row();
                }
            });
            ui.separator();
            ui.spacing_mut().item_spacing.y = 4.0;
            let note = format!("{} {} {NO_SCORE_TESTS}", score::REFERENCE, score::NOTE);
            ui.add(egui::Label::new(RichText::new(note).size(12.0).color(t.label)).wrap());
        });
    }

    /// Clear History… is offered: the selected test has runs and no test
    /// is running.
    fn can_clear(&self) -> bool {
        self.selected.test().is_some_and(|t| !self.history.runs(t.name()).is_empty()) && !self.engine.busy()
    }

    /// Clear History…: asks first (nothing to ask while the selected test
    /// has no runs).
    pub fn ask_clear(&mut self) {
        self.clear_asked = self.selected.test().is_some_and(|t| !self.history.runs(t.name()).is_empty());
    }

    /// The open Clear History question: "Clear CPU benchmark history?".
    pub fn clear_question(&self) -> Option<String> {
        let test = self.selected.test()?;
        self.clear_asked.then(|| format!("Clear {} benchmark history?", test.name()))
    }

    /// The answer to Clear History: removes the selected test's runs when
    /// `clear`.
    pub fn answer_clear(&mut self, clear: bool) {
        if !std::mem::take(&mut self.clear_asked) || !clear {
            return;
        }
        let Some(test) = self.selected.test() else { return };
        if let Err(e) = self.history.clear(test.name()) {
            self.history_warning = Some(e);
        }
    }

    /// The side panel: the selected test's history and Clear History….
    fn history_panel(&mut self, ui: &mut Ui) {
        let Some(test) = self.selected.test() else { return };
        let t = theme::of_ui(ui);
        let count = self.history.runs(test.name()).len();
        let can_clear = self.can_clear() && !self.clear_asked;
        let mut ask = false;
        egui::Frame::new()
            .fill(t.panel)
            .stroke(egui::Stroke::new(1.0, t.line))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(18, 14))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new(history_title(test)).size(14.0).strong().color(t.text));
                    if count > 0 {
                        ui.label(RichText::new(if count == 1 { "1 run".to_string() } else { format!("{count} runs") }).size(12.0).color(t.label));
                    }
                    if can_clear {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ask = ui.add(egui::Button::new(RichText::new("Clear History\u{2026}").color(t.red)).frame(false)).clicked();
                        });
                    }
                });
                self.clear_confirm(ui);
                let runs = self.history.runs(test.name());
                if runs.is_empty() {
                    ui.label(RichText::new(history::EMPTY).color(t.label));
                } else {
                    egui::ScrollArea::vertical().id_salt("bench-history").max_height(220.0).show(ui, |ui| {
                        let columns = history_columns(test);
                        let width = ((ui.available_width() - 10.0 * columns.len() as f32) / columns.len() as f32).max(60.0);
                        egui::Grid::new("bench-history-grid").num_columns(columns.len()).min_col_width(width).spacing([10.0, 8.0]).show(ui, |ui| {
                            for column in &columns {
                                ui.label(RichText::new(column).size(11.5).color(t.label));
                            }
                            ui.end_row();
                            for (i, run) in runs.iter().enumerate() {
                                let ink = if i == 0 { t.text } else { t.soft };
                                ui.label(RichText::new(short_date(&run.date)).color(ink));
                                for &(name, unit) in test.figures() {
                                    let text = run.figures.iter().find(|f| f.name == *name).map(|f| widgets::fmt_grouped(f.value, unit.decimals));
                                    ui.label(RichText::new(text.unwrap_or_else(|| "\u{2014}".into())).color(ink));
                                }
                                if score::SCORED.contains(&test) {
                                    ui.label(RichText::new(run.score.map_or_else(|| "\u{2014}".to_string(), score::text)).color(ink));
                                }
                                ui.label(RichText::new(&run.version).color(t.label));
                                ui.end_row();
                            }
                        });
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.label(RichText::new(IF_A_TEST_FAILS).size(12.0).color(t.label));
                    ui.label(RichText::new(failure_tip(test)).size(12.0).color(t.label));
                });
            });
        if ask {
            self.ask_clear();
        }
    }

    /// The Clear History confirmation, inline in the history, while it is open.
    fn clear_confirm(&mut self, ui: &mut Ui) {
        let Some(question) = self.clear_question() else { return };
        let t = theme::of_ui(ui);
        let mut decided = None;
        egui::Frame::new().fill(t.warm_note).corner_radius(egui::CornerRadius::same(5)).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(question).color(t.text));
                ui.label(RichText::new(CANT_BE_UNDONE).color(t.soft));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if parts::outline_button(ui, "Keep", true).clicked() {
                        decided = Some(false);
                    }
                    if parts::destructive_button(ui, "Clear", true).clicked() {
                        decided = Some(true);
                    }
                });
            });
        });
        if let Some(clear) = decided {
            self.answer_clear(clear);
        }
    }

    /// The short state under a rail entry: "Not available yet", "Not run
    /// yet", "Running · 40%", "1234 Mops/sec · today at 21:30", "Cancelled"
    /// or "Failed · last result kept". `now` is the local time (seconds since 1970).
    fn rail_status(&self, view: View, st: &engine::EngineState, now: i64) -> String {
        if view == View::Score {
            return self.overall_score().map_or_else(|| NOT_RUN.to_string(), |(s, _)| score::text(s));
        }
        let Some(test) = view.test() else { return page::NOT_AVAILABLE.to_string() };
        if st.running == Some(test.name()) {
            return format!("Running \u{b7} {:.0}%", st.progress * 100.0);
        }
        if st.running.is_some() && self.watching.contains(&test) && !st.ended.iter().any(|(n, _)| *n == test.name()) {
            return WAITING.to_string();
        }
        match self.ended.get(test.name()) {
            None => match self.last_saved(test) {
                Some((Some(figure), at)) => format!("{figure} \u{b7} {}", page::when(at, now)),
                Some((None, at)) => page::when(at, now),
                None => NOT_RUN.to_string(),
            },
            Some(RunEnd::Cancelled) => "Cancelled".to_string(),
            Some(RunEnd::Failed) => FAILED_KEPT.to_string(),
            Some(RunEnd::Complete { at }) => match st.results(test.name()).first() {
                Some(f) => format!("{} \u{b7} {}", f.text(), page::when(*at, now)),
                None => page::when(*at, now),
            },
        }
    }

    /// The tested-hardware line of `view` and when it last ran.
    fn hardware_line(&mut self, view: View, now: i64) -> Option<String> {
        let test = view.test()?;
        let hardware = match test {
            // The card the last saved run tested (also after a restart), else
            // the one it would test.
            Test::Gpu => match self.history.runs(gpu::NAME).first() {
                Some(run) => format!("Tested on {}", run.hardware),
                None => self.card_name(),
            },
            _ => self.hardware(test),
        };
        let ran = match self.last_saved(test) {
            Some((_, at)) => format!("last run {}", page::when(at, now)),
            None => NOT_RUN.to_string(),
        };
        Some(format!("{hardware} \u{b7} {ran}"))
    }

    /// The rail: CPU, GPU, Disk | Internet | Score, then Memory (added),
    /// each with its state.
    fn rail(&mut self, ui: &mut Ui, st: &engine::EngineState, now: i64) {
        #[cfg(test)]
        self.drawn.rail.clear();
        let t = theme::of_ui(ui);
        ui.spacing_mut().item_spacing.y = 2.0;
        for view in View::RAIL {
            if view.added() {
                ui.add_space(24.0);
            } else if view.separated() {
                ui.add_space(6.0);
                let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::hover());
                ui.painter().rect_filled(r.shrink2(egui::vec2(10.0, 0.0)), 0.0, t.line);
                ui.add_space(6.0);
            }
            let (score, status) = match view.test() {
                None => (self.overall_score().map(|(s, _)| (score::text(s), t.amber)), SCORE_RAIL.to_string()),
                Some(test) => (self.last_scored(test).and_then(|r| r.score).map(|s| (score::text(s), t.text)), self.rail_status(view, st, now)),
            };
            let status_color = if status.starts_with("Running") {
                t.link
            } else if status == FAILED_KEPT {
                t.red
            } else {
                t.label
            };
            let item = parts::RailItem { name: view.label(), tag: view.added().then_some("Added"), score, status: &status, status_color };
            let r = parts::rail_item(ui, &item, self.selected == view);
            if r.clicked() {
                self.selected = view;
            }
            #[cfg(test)]
            self.drawn.rail.push(r.id);
        }
    }

    /// The status band: a dot, how the run is going or how it ended and a
    /// line under it; Run All Benchmarks on the right (Cancel during Run All).
    fn band(&mut self, ui: &mut Ui, st: &engine::EngineState) {
        let t = theme::of_ui(ui);
        let run_all = self.run_all_line(st);
        let (dot, head, head_color, sub) = if let Some(line) = &run_all {
            (t.link, line.clone(), t.text, RUN_ALL_LINE.to_string())
        } else if let Some(name) = st.running {
            let test = RUN_ALL.into_iter().chain([Test::Memory]).find(|t| t.name() == name);
            let title = test.map_or(name, |t| View::RAIL.into_iter().find(|v| v.test() == Some(t)).map_or(name, View::title));
            let head = match (st.status(), st.phase) {
                (Some(s), _) => s,
                (None, Some(phase)) => {
                    let mut c = phase.chars();
                    let lower: String = c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default();
                    format!("{title}: {lower}")
                }
                (None, None) => title.to_string(),
            };
            (t.link, head, t.text, test.map(|t| self.hardware(t)).unwrap_or_default())
        } else {
            match (&st.outcome, st.status()) {
                (Some(engine::Outcome::Failed { .. }), Some(s)) => (t.red, s, t.red, "The previous results are kept.".to_string()),
                (Some(engine::Outcome::Cancelled), Some(s)) => (t.label, s, t.text, "Previous results are unchanged.".to_string()),
                (_, Some(s)) => (t.green, s, t.text, READY_LINE.to_string()),
                (_, None) => (t.green, "Ready".to_string(), t.text, READY_LINE.to_string()),
            }
        };
        let mut clicked = false;
        egui::Frame::new()
            .fill(t.band)
            .stroke(egui::Stroke::new(1.0, t.line))
            .corner_radius(egui::CornerRadius::same(8))
            .inner_margin(egui::Margin::symmetric(18, 12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.0;
                    let (r, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().circle_filled(r.center(), 5.0, dot);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 2.0;
                        ui.label(RichText::new(head).size(15.0).strong().color(head_color));
                        if !sub.is_empty() {
                            ui.label(RichText::new(sub).color(t.label));
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if run_all.is_some() {
                            let label = if st.cancelling { "Cancelling\u{2026}" } else { "Cancel" };
                            if parts::secondary_button(ui, label, !st.cancelling).clicked() {
                                self.cancel();
                            }
                        } else {
                            let b = parts::secondary_button(ui, "Run All Benchmarks", self.can_start());
                            clicked = b.on_hover_text("CPU \u{2192} GPU \u{2192} Disk \u{2192} Internet, then the score").clicked();
                        }
                    });
                });
            });
        if clicked {
            self.run_all();
        }
    }

    /// The "Video card" choice of the GPU view (when DXGI lists more than
    /// one card it matters which one is tested).
    fn card_choice(&mut self, ui: &mut Ui, enabled: bool) {
        let cards = self.adapters().to_vec();
        let Some(current) = self.gpu_card() else { return };
        let mut choice = current.luid;
        ui.add_enabled_ui(enabled, |ui| {
            parts::setting_combo(ui, "benchmarks-card", "Video card", Some("The card the video card test runs on"), current.name.as_str(), |ui| {
                for card in &cards {
                    ui.selectable_value(&mut choice, card.luid, card.name.as_str());
                }
            });
        });
        if let Some(card) = cards.iter().find(|c| c.luid == choice && choice != current.luid) {
            self.choose_card(card);
        }
    }

    /// The selected test: title, hardware line, gauges, progress, Start.
    fn view(&mut self, ui: &mut Ui, st: &engine::EngineState, now: i64) {
        let t = theme::of_ui(ui);
        let view = self.selected;
        let test = view.test();
        let running = test.is_some_and(|t| st.running == Some(t.name()));
        ui.spacing_mut().item_spacing.y = 8.0;
        // Header: title and hardware on the left, Start (or Cancel) on the right.
        let hardware = match view {
            View::Score => Some(SCORE_HARDWARE.to_string()),
            _ => self.hardware_line(view, now),
        };
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.label(RichText::new(view.title()).font(widgets::display_font(ui, 20.0)).color(t.text));
                if let Some(line) = hardware {
                    ui.label(RichText::new(line).color(t.label));
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let size = egui::vec2(120.0, 38.0);
                match test {
                    Some(_) if running => {
                        let label = if st.cancelling { "Cancelling\u{2026}" } else { "Cancel" };
                        if sized(ui, size, |ui| parts::secondary_button(ui, label, !st.cancelling)).clicked() {
                            self.cancel();
                        }
                    }
                    Some(test) => {
                        let enabled = self.can_start();
                        let r = sized(ui, size, |ui| parts::primary_button(ui, "Start", enabled));
                        #[cfg(test)]
                        {
                            self.drawn.start = Some(r.id);
                        }
                        if r.clicked() {
                            self.run(test);
                        }
                    }
                    None => {
                        sized(ui, size, |ui| parts::primary_button(ui, "Start", false));
                    }
                }
            });
        });
        let Some(test) = test else {
            if view == View::Score {
                self.score_view(ui);
            } else {
                ui.label(page::NOT_AVAILABLE);
            }
            return;
        };
        if view == View::Disk {
            self.disk_folder_row(ui, st);
        }
        ui.label(RichText::new(test.about()).color(t.label));
        if view == View::Gpu {
            self.card_choice(ui, !self.engine.busy());
        }
        if view == View::Internet {
            ui.label(RichText::new(format!("Uses {}", internet::HOST)).color(t.label));
        }
        if running {
            let phase = st.status().or(st.phase.map(str::to_string)).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.label(RichText::new(phase).size(12.0).color(t.soft));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(format!("{:.0}%", st.progress * 100.0)).size(12.0).color(t.soft));
                });
            });
            parts::progress_line(ui, st.progress, test_color(&t, test));
        }
        ui.add_space(4.0);
        let shown = if running { &st.partial[..] } else { st.results(test.name()) };
        self.figure_cards(ui, test, shown, running);
    }

    /// One card per figure of `test`: the value `shown` (else the last saved
    /// run's, unless `live`), its score, a bar against the reference machine
    /// and the best run, and a line of the last runs.
    fn figure_cards(&self, ui: &mut Ui, test: Test, shown: &[engine::Figure], live: bool) {
        let t = theme::of_ui(ui);
        let runs = self.history.runs(test.name());
        let figures = self.selected.gauges();
        let gap = 16.0;
        let avail = ui.available_width();
        let columns = if figures.len() > 1 && avail >= 2.0 * 300.0 + gap { 2 } else { 1 };
        let width = (avail - gap * (columns - 1) as f32) / columns as f32;
        for row in figures.chunks(columns) {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                for &(name, unit) in row {
                    let saved = |run: &history::Run| run.figures.iter().find(|f| f.name == name).map(|f| f.value);
                    let value = shown.iter().find(|f| f.name == name).map(|f| f.value).or_else(|| if live { None } else { runs.first().and_then(|r| saved(r)) });
                    let mut past: Vec<f64> = runs.iter().take(SPARK_RUNS).filter_map(|r| saved(r)).collect();
                    past.reverse();
                    let best = past.iter().copied().reduce(f64::max);
                    let reference = score::reference_figure(test, name);
                    let scale = page::card_scale(&[value, best, reference]);
                    let at = |v: f64| (v / scale) as f32;
                    let text = |v: f64| widgets::fmt_grouped(v, unit.decimals);
                    let card_score = match (reference, value) {
                        (None, _) => Some((NO_FIGURE_SCORE.to_string(), t.label)),
                        (Some(_), Some(v)) => score::figure_score(test, name, v)
                            .map(|s| (format!("Score {}", score::text(s)), if score::meets_reference(s) { t.green } else { t.amber })),
                        (Some(_), None) => None,
                    };
                    let card = parts::FigureCard {
                        label: name,
                        color: test_color(&t, test),
                        score: card_score,
                        value: value.map_or_else(|| "\u{2014}".to_string(), text),
                        unit: unit.label,
                        value_color: match value {
                            None => t.faint,
                            Some(_) if live => t.soft,
                            Some(_) => t.text,
                        },
                    };
                    parts::figure_card(ui, width, &card, |ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        let bar = ui.available_width();
                        parts::bullet_bar(ui, bar, value.map_or(0.0, at), test_color(&t, test), reference.map(at), best.map(at));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            if let Some(r) = reference {
                                parts::legend_item(ui, &format!("Reference PC {}", text(r)), t.text);
                                ui.add_space(9.0);
                            }
                            if let Some(b) = best {
                                parts::legend_item(ui, &format!("Your best {}", text(b)), t.soft);
                            }
                        });
                        if past.len() > 1 {
                            ui.add_space(5.0);
                            ui.separator();
                            ui.horizontal(|ui| {
                                parts::run_sparkline(ui, egui::vec2(160.0, 30.0), &past, test_color(&t, test));
                                let lo = past.iter().copied().fold(f64::MAX, f64::min);
                                let line = format!("Last {} runs \u{b7} {} \u{2013} {}", past.len(), text(lo), text(best.unwrap_or(lo)));
                                ui.label(RichText::new(line).size(11.5).color(t.label));
                            });
                        }
                    });
                }
            });
        }
    }
}

/// The GPU view's hardware line when DXGI lists no hardware card.
const NO_CARD: &str = "No Direct3D 11 GPU found";

impl Benchmarks {
    /// Under the test folder: its free space, "Choose…", and the refusal
    /// when the test file would not fit.
    fn disk_folder_row(&mut self, ui: &mut Ui, st: &engine::EngineState) {
        let sub = theme::of_ui(ui).label;
        let free = self.test_folder().ok().and_then(|f| disk::free_space(&f));
        ui.horizontal(|ui| {
            if let Some(free) = free {
                ui.label(RichText::new(format!("{} free", crate::widgets::fmt_bytes(free))).color(sub));
            }
            self.choose_folder(ui, st.running.is_none());
        });
        if free.is_some_and(|f| f < (disk::CHUNK * disk::CHUNKS) as u64) {
            ui.colored_label(theme::of_ui(ui).red, disk::NOT_ENOUGH_SPACE);
        }
    }

    /// "Choose…" (enabled when `enabled` and the platform can pick): sets
    /// the disk test's folder to the one picked.
    fn choose_folder(&mut self, ui: &mut Ui, enabled: bool) {
        if ui.add_enabled(enabled && self.context.is_some(), egui::Button::new("Choose\u{2026}")).clicked() {
            if let Some(dir) = self.context.as_ref().and_then(|c| c.pick_folder(CHOOSE_FOLDER, "Choose")) {
                self.disk_folder = Some(dir);
            }
        }
    }
}

/// The note under its requirements in Settings › Add-ons.
const NEEDS: &str =
    "Uses the internet only for the Internet test, when you start it. The disk test needs free space in its test folder. Never asks for administrator rights.";

/// The title of the disk test's folder picker.
const CHOOSE_FOLDER: &str = "Choose a folder on the drive to benchmark";

/// The disk test in `folder` (or failing with why there is none).
fn disk_unit(folder: Result<std::path::PathBuf, &'static str>) -> Unit {
    Unit::new(Test::Disk.name(), move |p| disk::disk(p, &DiskJob::new(folder.map_err(String::from)?)))
}

/// The add-on's own settings, as saved: a default value is left out, and
/// the fields it does not know are kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(default)]
struct Prefs {
    /// The disk test's folder.
    #[serde(rename = "diskBenchmarkFolder", skip_serializing_if = "Option::is_none")]
    disk_folder: Option<String>,
    /// The name of the card the GPU test runs on (none: the high-performance one).
    #[serde(rename = "gpuAdapter", skip_serializing_if = "Option::is_none")]
    gpu_adapter: Option<String>,
    /// The rail entry shown, by its key (none: CPU, the first one).
    #[serde(rename = "selectedTest", skip_serializing_if = "Option::is_none")]
    selected_test: Option<String>,
    /// The Summary tile is on.
    #[serde(rename = "summaryTile", skip_serializing_if = "is_default")]
    summary_tile: bool,
    /// The fields of other versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// The heading of a test's history: "CPU history".
fn history_title(test: Test) -> String {
    format!("{} history", test.name())
}

/// The columns of a test's history: Run, its figures with their units,
/// Score (scored tests only), Version.
fn history_columns(test: Test) -> Vec<String> {
    let mut columns = vec!["Run".to_string()];
    columns.extend(test.figures().iter().map(|(name, unit)| format!("{name} ({unit})")));
    if score::SCORED.contains(&test) {
        columns.push("Score".into());
    }
    columns.push("Version".into());
    columns
}

/// The history's help on failures, for every test (then the test's own tip).
const IF_A_TEST_FAILS: &str = "If a test fails, the status band says why in red and the previous results stay.";
/// The rail line of a test whose last run failed.
const FAILED_KEPT: &str = "Failed \u{b7} last result kept";
/// After the debug-build warning in its banner.
const NOT_SAVED: &str = "Runs are not saved.";

/// What to try when `test` fails.
fn failure_tip(test: Test) -> &'static str {
    match test {
        Test::Cpu | Test::Memory => "Close heavy apps and run it again.",
        Test::Gpu => "Update the video card driver, or choose another video card.",
        Test::Disk => "Choose a test folder you can write to, on a drive with 256 MB free.",
        Test::Internet => "Check the connection: the test needs speed.cloudflare.com.",
    }
}

/// What a test or the score says before its first run.
const NOT_RUN: &str = "Not run yet";
/// A test Run All has still to run.
const WAITING: &str = "Waiting";
/// Width of the rail of tests; the selected test's view gets the rest of the row.
const RAIL_WIDTH: f32 = 220.0;
/// The runs a card's sparkline shows, newest last.
const SPARK_RUNS: usize = 10;
/// A figure card of a test without a score.
const NO_FIGURE_SCORE: &str = "No score";
/// The Score entry's line on the rail.
const SCORE_RAIL: &str = "CPU, GPU and disk vs 100";
/// The Score view's line under its title.
const SCORE_HARDWARE: &str = "Reference PC = 100 \u{b7} from the latest CPU, GPU and disk runs";
/// What the overall score is, beside it.
const SCORE_HOW: &str = "Geometric mean of CPU, GPU and disk";
/// Which tests have no score.
const NO_SCORE_TESTS: &str = "Internet and Memory have no score.";
/// After the Clear History question.
const CANT_BE_UNDONE: &str = "This can't be undone.";
/// The band's line while idle.
const READY_LINE: &str = "Pick a test and press Start, or run them all. 100 = the reference PC.";
/// The band's line during Run All.
const RUN_ALL_LINE: &str = "CPU \u{2192} GPU \u{2192} Disk \u{2192} Internet. The score is updated at the end.";

/// The colour of `test`: its device's colour in the theme's graph palette
/// (as on the Performance page).
fn test_color(t: &Theme, test: Test) -> Color32 {
    let g = &t.graphs;
    match test {
        Test::Cpu => g.cpu,
        Test::Gpu => g.gpu,
        Test::Internet => g.network,
        Test::Disk => g.disk,
        Test::Memory => g.memory,
    }
}

/// Runs `add` in a box of exactly `size` that the button it adds fills
/// (Start and Cancel are 120 x 38).
fn sized(ui: &mut Ui, size: egui::Vec2, add: impl FnOnce(&mut Ui) -> egui::Response) -> egui::Response {
    ui.allocate_ui_with_layout(size, egui::Layout::centered_and_justified(egui::Direction::LeftToRight), add).inner
}

/// The Summary tile before the first scored run.
const NO_SCORE: &str = "No score yet";

/// "2026-11-17 21:30" from an ISO date "2026-11-17T21:30:00".
fn short_date(iso: &str) -> String {
    iso.get(..16).unwrap_or(iso).replace('T', " ")
}


/// The app's history file (never in tests: they use temp folders).
#[cfg(not(test))]
fn app_history_file() -> Option<PathBuf> {
    history::app_file()
}

#[cfg(test)]
fn app_history_file() -> Option<PathBuf> {
    None
}

/// The total RAM, in bytes.
fn total_ram() -> u64 {
    let mut sys = sysinfo::System::new();
    sys.refresh_memory();
    sys.total_memory()
}

/// "AMD Ryzen 5 7600X · 6 cores, 12 threads".
fn cpu_line() -> String {
    let cores = sysinfo::System::physical_core_count().unwrap_or(0);
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0);
    format!("{} \u{b7} {cores} cores, {threads} threads", crate::widgets::cpu_brand())
}

impl Addon for Benchmarks {
    fn id(&self) -> &'static str {
        "benchmarks"
    }
    fn name(&self) -> &'static str {
        "Benchmarks"
    }
    fn description(&self) -> &'static str {
        "Quick CPU, GPU, memory, disk and Internet speed tests."
    }
    /// The internet for the Internet test only; free space for the disk
    /// test; no administrator rights and no external program.
    fn requirements(&self) -> Requirements {
        Requirements { admin: false, network: true, programs: &[], note: Some(NEEDS), ..Requirements::default() }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "High while running", network: "Only during the Internet test", disk: "High while running", when: "Only when you start a test" }
    }

    fn menu_title(&self) -> Option<&'static str> {
        Some("Benchmarks")
    }

    fn menu_in_menu_bar(&self) -> bool {
        true
    }

    /// The "Benchmarks" menu: Run All, Start the selected test, Cancel.
    fn header_menu(&mut self, ui: &mut Ui, _page: &mut PageContext) {
        if ui.add_enabled(self.can_start(), egui::Button::new("Run All Benchmarks")).clicked() {
            self.run_all();
            ui.close();
        }
        let test = self.selected.test();
        if ui.add_enabled(test.is_some() && self.can_start(), egui::Button::new("Start")).clicked() {
            if let Some(test) = test {
                self.run(test);
            }
            ui.close();
        }
        if ui.add_enabled(self.engine.busy(), egui::Button::new("Cancel")).clicked() {
            self.cancel();
            ui.close();
        }
        if ui.add_enabled(self.can_clear(), egui::Button::new("Clear History\u{2026}")).clicked() {
            self.ask_clear();
            ui.close();
        }
    }

    /// Reads the history (and its warning, if the file could not be read).
    fn start(&mut self, context: &AddonContext) {
        self.context = Some(context.clone());
        self.debug_build = debug_build_warning().is_some();
        self.read_history();
    }

    /// Its settings ([`Prefs`]); `Null` while nothing was chosen nor saved.
    fn settings(&self) -> serde_json::Value {
        let prefs = Prefs {
            disk_folder: self.disk_folder.clone(),
            gpu_adapter: self.gpu_name.clone(),
            selected_test: (self.selected != View::default()).then(|| self.selected.key().to_string()),
            summary_tile: self.summary_tile,
            rest: crate::addons::read_settings::<Prefs>(&self.saved).rest,
        };
        match crate::addons::write_settings(&prefs) {
            serde_json::Value::Null if !self.saved.is_object() => self.saved.clone(),
            v => v,
        }
    }

    fn load_settings(&mut self, settings: &serde_json::Value) {
        let prefs = crate::addons::read_settings::<Prefs>(settings);
        self.disk_folder = prefs.disk_folder.filter(|d| !d.is_empty());
        self.gpu_name = prefs.gpu_adapter.filter(|n| !n.is_empty());
        self.gpu_choice = None;
        self.selected = prefs.selected_test.as_deref().and_then(View::from_key).unwrap_or_default();
        self.summary_tile = prefs.summary_tile;
        self.saved = settings.clone();
    }

    /// The test folder, the video card the GPU test runs on, and the
    /// Summary tile.
    fn settings_section(&mut self, ui: &mut Ui, _page: &mut PageContext) {
        let idle = !self.engine.busy();
        parts::group_heading(ui, "Tests");
        let folder = self.test_folder().map_or_else(|e| e.to_string(), |f| f.display().to_string());
        if parts::setting_folder(ui, "Test folder", Some("The disk test runs on this folder's drive"), &folder, None, idle && self.context.is_some()) == parts::FolderAct::Choose {
            if let Some(dir) = self.context.as_ref().and_then(|c| c.pick_folder(CHOOSE_FOLDER, "Choose")) {
                self.disk_folder = Some(dir);
            }
        }
        self.card_choice(ui, idle);
        parts::group_heading(ui, "Summary");
        parts::setting_switch(ui, "Show a tile on Summary", Some("The last score and its date"), &mut self.summary_tile);
    }

    /// The last overall score and its date, when the tile is on.
    fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
        if let Some(lines) = self.summary_lines() {
            crate::addons::summary_tile(ui, page, "Benchmarks", lines);
        }
    }

    /// Reads the history and the hardware lines again.
    fn refresh(&mut self) {
        self.read_history();
        self.cpu_line = Some(cpu_line());
        self.mem_total = Some(total_ram());
        self.adapters = Some(gpu::adapters());
    }

    fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.settle();
        let st = self.engine.state();
        let t = theme::of_ui(ui);
        let now = crate::platform::local_secs();
        if st.running.is_some() || !self.watching.is_empty() {
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        ui.spacing_mut().item_spacing.y = 12.0;
        // Red banners above the band: the debug build, an unreadable history.
        if let Some(warning) = debug_build_warning() {
            parts::alert_banner(ui, &format!("{warning}. {NOT_SAVED}"));
        }
        if let Some(warning) = self.history_warning.clone() {
            parts::alert_banner(ui, &warning);
        }
        self.band(ui, &st);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 20.0;
            ui.vertical(|ui| {
                ui.set_width(RAIL_WIDTH);
                self.rail(ui, &st, now);
            });
            ui.vertical(|ui| {
                ui.set_width(ui.available_width());
                self.view(ui, &st, now);
                if self.selected.test().is_some() {
                    ui.label(RichText::new(format!("Close heavy apps for steadier results. CPU use now: {:.0}%", page.sample.cpu_total)).size(12.0).color(t.label));
                    ui.add_space(6.0);
                    self.history_panel(ui);
                }
            });
        });
    }

    /// Cancels the running test (its test file is deleted), waits for its
    /// thread to end and forgets every result; the selected test and the
    /// settings stay, and the history stays in its file for the next start.
    fn stop(&mut self) {
        self.engine.shutdown();
        *self = Benchmarks {
            selected: self.selected,
            history_file: self.history_file.take(),
            disk_folder: self.disk_folder.take(),
            gpu_choice: self.gpu_choice,
            gpu_name: self.gpu_name.take(),
            summary_tile: self.summary_tile,
            saved: std::mem::take(&mut self.saved),
            ..Benchmarks::default()
        };
    }
}

/// CPU figure: work steps per second on one core.
const SINGLE_CORE: &str = "Single core";
/// CPU figure: work steps per second on every core.
const MULTI_CORE: &str = "Multi core";
/// Memory figure.
const COPY_SPEED: &str = "Copy speed";
/// Disk figures.
const WRITE_SPEED: &str = "Write speed";
const READ_SPEED: &str = "Read speed";

/// How long each CPU phase runs.
const CPU_SECONDS: f64 = 2.0;
/// The memory test copies a block of this size this many times.
const MEMORY_BLOCK: usize = 256 * 1024 * 1024;
const MEMORY_ROUNDS: usize = 8;

/// One unit of CPU work: mixed integer and floating-point math.
fn cpu_work(iters: u64) -> f64 {
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut f = 1.0f64;
    for i in 0..iters {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        f = (f * 1.000_000_1 + (x & 0xff) as f64 * 1e-9).sqrt() + (i & 3) as f64 * 1e-12;
    }
    f + (x & 1) as f64
}

/// Work steps between two looks at the clock and the cancel.
const CPU_STEP: u64 = 200_000;

/// Runs CPU work for `seconds` (or until cancelled): the work steps done.
fn cpu_steps(probe: &Probe, seconds: f64, progress: impl Fn(f32)) -> u64 {
    let t = Instant::now();
    let mut n = 0u64;
    let mut sink = 0.0;
    while t.elapsed().as_secs_f64() < seconds && !probe.cancelled() {
        sink += cpu_work(CPU_STEP);
        n += CPU_STEP;
        progress((t.elapsed().as_secs_f64() / seconds).min(1.0) as f32);
    }
    std::hint::black_box(sink);
    n
}

/// "Testing one CPU core" for `seconds` (Single core, Mops/sec), then
/// "Testing all CPU cores" for `seconds` (Multi core, Gop/sec).
fn cpu(probe: &Probe, seconds: f64) -> Result<(), String> {
    probe.phase("Testing one CPU core");
    let t = Instant::now();
    let n = cpu_steps(probe, seconds, |p| probe.progress(p * 0.5));
    if probe.cancelled() {
        return Ok(());
    }
    probe.figure(SINGLE_CORE, n as f64 / t.elapsed().as_secs_f64() / 1e6, MOPS);

    probe.phase("Testing all CPU cores");
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let t = Instant::now();
    let total: u64 = std::thread::scope(|s| {
        let workers: Vec<_> = (1..threads).map(|_| s.spawn(|| cpu_steps(probe, seconds, |_| ()))).collect();
        let own = cpu_steps(probe, seconds, |p| probe.progress(0.5 + p * 0.5));
        own + workers.into_iter().map(|w| w.join().unwrap_or(0)).sum::<u64>()
    });
    if probe.cancelled() {
        return Ok(());
    }
    probe.figure(MULTI_CORE, total as f64 / t.elapsed().as_secs_f64() / 1e9, GOPS);
    Ok(())
}

/// "Testing memory": copies a `size`-byte block `rounds` times; Copy speed
/// in GB/s.
fn memory(probe: &Probe, size: usize, rounds: usize) -> Result<(), String> {
    probe.phase("Testing memory");
    let src = vec![1u8; size];
    let mut dst = vec![0u8; size];
    let t = Instant::now();
    for r in 0..rounds {
        if probe.cancelled() {
            return Ok(());
        }
        dst.copy_from_slice(&src);
        std::hint::black_box(&dst);
        probe.progress((r + 1) as f32 / rounds as f32);
    }
    probe.figure(COPY_SPEED, (size * rounds) as f64 / t.elapsed().as_secs_f64() / 1e9, GBS);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::Addon;
    use crate::platform::AddonContext;
    use engine::wait_idle;
    use std::sync::mpsc::channel;

    /// The (name, unit) of each figure `test` left.
    fn shown(state: &engine::EngineState, test: Test) -> Vec<(&'static str, Measure)> {
        state.results(test.name()).iter().map(|f| (f.name, f.unit)).collect()
    }

    #[test]
    fn each_test_takes_its_colour_from_the_theme_graph_palette() {
        for mode in [crate::theme::Mode::Dark, crate::theme::Mode::Light] {
            let t = crate::theme::Theme::of(mode);
            let g = t.graphs;
            let colours: Vec<_> = [Test::Cpu, Test::Gpu, Test::Disk, Test::Internet, Test::Memory].map(|test| test_color(&t, test)).to_vec();
            assert_eq!(colours, [g.cpu, g.gpu, g.disk, g.network, g.memory], "{mode:?}");
        }
    }

    #[test]
    fn each_test_view_has_its_title_and_test_name() {
        let views: Vec<_> = [View::Cpu, View::Disk, View::Memory].iter().map(|v| (v.title(), v.test().map(Test::name))).collect();
        assert_eq!(
            views,
            [("CPU Benchmark", Some("CPU")), ("Disk Benchmark", Some("Disk")), ("Memory Benchmark", Some("Memory"))]
        );
    }

    /// A fake CPU run that measures 1234 Mops/sec, or fails with "boom".
    pub(super) fn fake_cpu(fail: bool) -> Unit {
        Unit::new("CPU", move |p| {
            p.figure("Single core", 1234.0, MOPS);
            if fail {
                Err("boom".into())
            } else {
                Ok(())
            }
        })
    }

    #[test]
    fn the_rail_tells_each_test_state() {
        let mut b = Benchmarks::default();
        let now = crate::platform::local_secs();
        let status = |b: &mut Benchmarks, v: View| {
            wait_idle(&b.engine);
            b.settle();
            b.rail_status(v, &b.engine.state(), now)
        };
        assert_eq!(status(&mut b, View::Cpu), "Not run yet");
        assert_eq!(status(&mut b, View::Gpu), "Not run yet");
        assert_eq!(status(&mut b, View::Internet), "Not run yet");
        assert_eq!(status(&mut b, View::Score), "Not run yet");

        assert!(b.launch(Test::Cpu, fake_cpu(false)));
        let done = status(&mut b, View::Cpu);
        assert!(done.starts_with("1234 Mops/sec \u{b7} today at "), "{done}");
        assert_eq!(status(&mut b, View::Memory), "Not run yet", "other tests are untouched");

        assert!(b.launch(Test::Cpu, fake_cpu(true)));
        assert_eq!(status(&mut b, View::Cpu), "Failed \u{b7} last result kept");
        assert_eq!(b.engine.state().status().as_deref(), Some("CPU test failed: boom"));
    }

    #[test]
    fn the_rail_shows_a_running_test_progress() {
        let mut b = Benchmarks::default();
        let (started, wait_started) = channel();
        b.launch(
            Test::Disk,
            Unit::new("Disk", move |p| {
                p.progress(0.4);
                started.send(()).unwrap();
                while !p.cancelled() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Ok(())
            }),
        );
        wait_started.recv().unwrap();
        assert_eq!(b.rail_status(View::Disk, &b.engine.state(), 0), "Running \u{b7} 40%");
        b.cancel();
        wait_idle(&b.engine);
        b.settle();
        assert_eq!(b.rail_status(View::Disk, &b.engine.state(), 0), "Cancelled");
    }

    #[test]
    fn tab_from_the_rail_reaches_start() {
        let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let mut b = Benchmarks::default();
        let tab = egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        let mut on_rail = false;
        for i in 0..20 {
            let events = if i == 0 { vec![] } else { vec![tab.clone()] };
            let _ = ctx.run(egui::RawInput { events, ..Default::default() }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| b.ui(ui, &mut PageContext::new(&settings, &sample)));
            });
            let focused = ctx.memory(|m| m.focused());
            on_rail |= focused.is_some_and(|f| b.drawn.rail.contains(&f));
            if on_rail && focused.is_some() && focused == b.drawn.start {
                return;
            }
        }
        panic!("Tab went through the rail (reached it: {on_rail}) but never reached Start");
    }

    #[test]
    fn the_internet_view_offers_its_test_but_never_starts_it_alone() {
        assert_eq!(View::Internet.test(), Some(Test::Internet));
        assert_eq!(Test::Internet.name(), "Internet");
        assert_eq!(Test::Internet.figures(), [("Download", engine::MBITS), ("Upload", engine::MBITS)]);
        let mut b = Benchmarks { selected: View::Internet, ..Default::default() };
        let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        for _ in 0..3 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| b.ui(ui, &mut PageContext::new(&settings, &sample)));
            });
        }
        assert!(b.drawn.start.is_some(), "Start is offered");
        assert!(!b.engine.busy(), "showing the view connects to nothing");
        assert!(b.requirements().network, "the add-on page says it needs the network");
    }

    #[test]
    fn views_without_a_test_cannot_start() {
        let mut b = Benchmarks { selected: View::Score, ..Default::default() };
        let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| b.ui(ui, &mut PageContext::new(&settings, &sample)));
        });
        assert_eq!(b.drawn.start, None, "no enabled Start on a view that is not available");
        assert!(!b.engine.busy());
    }

    pub(super) fn cards() -> Vec<gpu::Adapter> {
        vec![
            gpu::Adapter { name: "AMD Radeon(TM) Graphics".into(), luid: 7 },
            gpu::Adapter { name: "NVIDIA GeForce RTX 4080".into(), luid: 9 },
        ]
    }

    /// Starts the GPU test on the chosen card, as `unit` builds it from it.
    fn launch_gpu(b: &mut Benchmarks, unit: impl FnOnce(Option<gpu::Adapter>) -> Unit) -> bool {
        let card = b.gpu_card();
        b.launch(Test::Gpu, unit(card))
    }

    #[test]
    fn the_gpu_test_runs_on_the_chosen_card_and_the_view_names_it() {
        let mut b = Benchmarks { adapters: Some(cards()), ..Default::default() };
        assert_eq!(b.hardware_line(View::Gpu, 0).as_deref(), Some("AMD Radeon(TM) Graphics \u{b7} Not run yet"), "the first listed card by default");
        b.gpu_choice = Some(9);
        assert_eq!(b.hardware_line(View::Gpu, 0).as_deref(), Some("NVIDIA GeForce RTX 4080 \u{b7} Not run yet"));

        let (opened, wait_opened) = channel();
        assert!(launch_gpu(&mut b, move |card| {
            opened.send(card).unwrap();
            Unit::new("GPU", |p| {
                p.figure("Compute", 43293.0, gpu::GFLOPS);
                Ok(())
            })
        }));
        assert_eq!(wait_opened.recv().unwrap().map(|c| c.name), Some("NVIDIA GeForce RTX 4080".to_string()));
        wait_idle(&b.engine);
        b.settle();
        // Choosing another card after the run still names the tested one.
        b.gpu_choice = Some(7);
        let now = crate::platform::local_secs();
        let line = b.hardware_line(View::Gpu, now).unwrap();
        assert!(line.starts_with("Tested on NVIDIA GeForce RTX 4080 \u{b7} last run today at "), "{line}");
        assert!(b.rail_status(View::Gpu, &b.engine.state(), now).starts_with("43293 GFLOPS \u{b7} today at "));
    }

    #[test]
    fn without_a_card_the_gpu_test_fails_with_its_message() {
        let mut b = Benchmarks { adapters: Some(vec![]), ..Default::default() };
        assert_eq!(b.hardware_line(View::Gpu, 0).as_deref(), Some("No Direct3D 11 GPU found \u{b7} Not run yet"));
        assert!(b.run(Test::Gpu));
        let s = wait_idle(&b.engine);
        assert_eq!(s.status().as_deref(), Some("GPU test failed: A hardware Direct3D 11 GPU is unavailable on this system."));
    }

    #[test]
    fn the_cpu_test_reports_single_core_in_mops_and_multi_core_in_gops() {
        let mut engine = Engine::default();
        engine.start(vec![Unit::new("CPU", |p| cpu(p, 0.05))]);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        assert_eq!(shown(&s, Test::Cpu), [("Single core", MOPS), ("Multi core", GOPS)]);
        assert_eq!(shown(&s, Test::Cpu), Test::Cpu.figures());
        assert!(s.results("CPU").iter().all(|f| f.value > 0.0));
    }

    /// Runs `test` as the app runs it and prints its figures.
    fn print_real_run(test: Test) {
        let mut engine = Engine::default();
        engine.start(vec![test.unit()]);
        let t = Instant::now();
        while engine.busy() && t.elapsed() < Duration::from_secs(120) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let s = engine.state();
        assert_eq!(s.status().as_deref(), Some("Test complete"), "{:?}", s.outcome);
        let figures = s.results(test.name());
        for f in figures {
            println!("{}: {} ({})", f.name, f.text(), f.value);
        }
        assert_eq!(figures.iter().map(|f| (f.name, f.unit)).collect::<Vec<_>>(), test.figures());
    }

    /// The real CPU test on this PC (2 s on one core, 2 s on all):
    /// `cargo test --release cpu_test_on_this_pc -- --ignored --nocapture`
    #[test]
    #[ignore = "real CPU work; numbers only mean something in a release build"]
    fn cpu_test_on_this_pc() {
        print_real_run(Test::Cpu);
    }

    /// The real disk test on this PC, in the temp folder (256 MB, deleted):
    /// `cargo test --release disk_test_on_this_pc -- --ignored --nocapture`
    #[test]
    #[ignore = "writes and reads a 256 MB file in the temp folder"]
    fn disk_test_on_this_pc() {
        print_real_run(Test::Disk);
    }

    #[test]
    fn the_memory_test_reports_copy_speed_in_gbs() {
        let mut engine = Engine::default();
        engine.start(vec![Unit::new("Memory", |p| memory(p, 1024 * 1024, 8))]);
        let s = wait_idle(&engine);
        assert_eq!(shown(&s, Test::Memory), [("Copy speed", GBS)]);
        assert!(s.results("Memory")[0].value > 0.0);
    }

    #[test]
    fn the_chosen_test_folder_is_remembered_and_survives_turning_off() {
        let mut b = Benchmarks::default();
        assert_eq!(b.settings(), serde_json::Value::Null, "nothing saved while nothing was chosen");
        assert_eq!(b.test_folder().ok(), disk::default_folder(), "this system's default folder");
        b.load_settings(&serde_json::json!({ "diskBenchmarkFolder": "D:\\Bench", "other": 1 }));
        assert_eq!(b.test_folder().ok(), Some(std::path::PathBuf::from("D:\\Bench")));
        b.stop();
        assert_eq!(b.settings(), serde_json::json!({ "diskBenchmarkFolder": "D:\\Bench", "other": 1 }));
    }

    #[test]
    fn without_a_temp_folder_the_disk_test_fails_with_its_message() {
        let mut engine = Engine::default();
        engine.start(vec![disk_unit(Err(disk::NO_DEFAULT_FOLDER))]);
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some(format!("Disk test failed: {}", disk::NO_DEFAULT_FOLDER).as_str()));
    }

    #[test]
    fn other_start_buttons_are_disabled_while_a_test_runs() {
        let mut b = Benchmarks::default();
        assert!(b.can_start());
        let (started, wait_started) = channel();
        b.engine.start(vec![Unit::new("CPU", move |probe| {
            started.send(()).unwrap();
            while !probe.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(())
        })]);
        wait_started.recv().unwrap();
        assert!(!b.can_start());
        assert!(!b.run(Test::Memory), "a second test does not start");
        b.cancel();
        let s = wait_idle(&b.engine);
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert!(b.can_start());
    }

    #[test]
    fn a_debug_build_warns_that_benchmarks_are_not_valid() {
        let expected = if cfg!(debug_assertions) { Some("You are running a debug build, benchmarks are not valid") } else { None };
        assert_eq!(debug_build_warning(), expected);
        if expected.is_some() {
            let texts = crate::test_ui::drawn_lines(|ui, page| Benchmarks::default().ui(ui, page));
            let banner = "You are running a debug build, benchmarks are not valid. Runs are not saved.";
            assert!(texts.iter().any(|t| t == banner), "{texts:?}");
        }
    }

    #[test]
    fn turning_benchmarks_off_during_a_run_cancels_it_and_leaves_no_thread() {
        let (ctx, runner) = AddonContext::recording();
        let mut b = Benchmarks::default();
        b.start(&ctx);
        let (started, wait_started) = channel();
        let (finished, wait_finished) = channel();
        b.engine.start(vec![Unit::new("CPU", move |probe| {
            started.send(()).unwrap();
            let t = Instant::now();
            while !probe.cancelled() && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            probe.figure("Single core", 123.0, MOPS);
            finished.send(probe.cancelled()).unwrap();
            Ok(())
        })]);
        wait_started.recv().unwrap();
        assert!(b.engine.has_thread());

        b.stop();
        assert!(wait_finished.try_recv().unwrap(), "the run was cancelled and had ended when stop returned");
        assert!(!b.engine.has_thread());
        b.start(&ctx);
        let s = b.engine.state();
        assert_eq!(s.running, None);
        assert!(s.results("CPU").is_empty(), "results of the stopped run are dropped");
        assert_eq!(s.outcome, None);
        assert!(runner.launches().is_empty());
    }

    /// Benchmarks keeping its history in `dir`, started.
    pub(super) fn with_history(dir: &history::tests::TempDir) -> Benchmarks {
        let (ctx, _) = AddonContext::recording();
        let mut b = Benchmarks { history_file: Some(dir.file()), ..Default::default() };
        b.start(&ctx);
        // Tests run in a debug build: save runs as a release build would.
        b.debug_build = false;
        b
    }

    /// Runs `unit` as `test` to its end and notes it.
    pub(super) fn run_to_end(b: &mut Benchmarks, test: Test, unit: Unit) {
        assert!(b.launch(test, unit));
        wait_idle(&b.engine);
        b.settle();
    }

    #[test]
    fn the_gpu_test_and_the_addon_say_they_test_compute_and_graphics() {
        assert_eq!(Test::Gpu.about(), "Runs a Direct3D 11 compute shader, then draws with a pixel shader, on the chosen video card: about 4 seconds.");
        assert_eq!(Benchmarks::default().description(), "Quick CPU, GPU, memory, disk and Internet speed tests.");
    }

    #[test]
    fn a_debug_build_shows_its_runs_but_never_saves_them() {
        let dir = history::tests::TempDir::new("debug-build");
        let mut b = with_history(&dir);
        b.debug_build = true;
        run_to_end(&mut b, Test::Cpu, fake_cpu(false));
        assert!(b.history.runs("CPU").is_empty(), "no skewed numbers in the history");
        assert_eq!(b.overall_score(), None, "nor in the score");
        let status = b.rail_status(View::Cpu, &b.engine.state(), crate::platform::local_secs());
        assert!(status.starts_with("1234 Mops/sec \u{b7} today at "), "{status}");
    }

    #[test]
    fn refresh_reads_the_history_again() {
        let dir = history::tests::TempDir::new("refresh");
        let mut other = with_history(&dir);
        let mut b = with_history(&dir);
        run_to_end(&mut other, Test::Cpu, fake_cpu(false));
        assert!(b.history.runs("CPU").is_empty());
        b.refresh();
        assert_eq!(b.history.runs("CPU").len(), 1);
    }

    #[test]
    fn after_a_restart_the_gpu_view_names_the_card_the_last_saved_run_tested() {
        let dir = history::tests::TempDir::new("gpu-restart");
        let mut b = with_history(&dir);
        b.adapters = Some(cards());
        b.gpu_choice = Some(9);
        assert!(launch_gpu(&mut b, |_| Unit::new("GPU", |p| {
            p.figure("Compute", 43293.0, gpu::GFLOPS);
            Ok(())
        })));
        wait_idle(&b.engine);
        b.settle();
        drop(b);

        let mut next = with_history(&dir);
        next.adapters = Some(cards());
        let line = next.hardware_line(View::Gpu, crate::platform::local_secs()).unwrap();
        assert!(line.starts_with("Tested on NVIDIA GeForce RTX 4080 \u{b7} last run today at "), "{line}");
    }

    #[test]
    fn the_history_panel_stays_on_screen_in_a_normal_window() {
        let mut b = Benchmarks { selected: View::Disk, ..Default::default() };
        let texts = crate::test_ui::drawn_lines_in(egui::vec2(1100.0, 700.0), |ui, page| b.ui(ui, page));
        assert!(texts.iter().any(|t| t == "Disk history"), "history panel not drawn in {texts:?}");
    }

    #[test]
    fn the_history_panel_says_what_to_do_if_a_test_fails() {
        let mut b = Benchmarks { selected: View::Disk, ..Default::default() };
        let texts = crate::test_ui::drawn_lines(|ui, _| b.history_panel(ui));
        for wanted in ["If a test fails, the status band says why in red and the previous results stay.",
            "Choose a test folder you can write to, on a drive with 256 MB free.",
        ] {
            assert!(texts.iter().any(|t| t == wanted), "{wanted:?} not drawn in {texts:?}");
        }
    }

    #[test]
    fn completed_runs_are_saved_and_survive_turning_the_addon_off() {
        let dir = history::tests::TempDir::new("addon-off");
        let mut b = with_history(&dir);
        run_to_end(&mut b, Test::Cpu, fake_cpu(false));
        run_to_end(&mut b, Test::Cpu, fake_cpu(true));

        let runs = b.history.runs("CPU");
        assert_eq!(runs.len(), 1, "only completed runs are saved");
        let run = runs[0];
        assert_eq!(run.figures, [history::SavedFigure { name: "Single core".into(), value: 1234.0, unit: "Mops/sec".into() }]);
        assert_eq!(run.version, env!("CARGO_PKG_VERSION"));
        assert_eq!(run.platform, format!("{} {}", std::env::consts::OS, std::env::consts::ARCH));
        assert!(crate::platform::iso_secs(&run.date).is_some(), "ISO date: {}", run.date);
        assert!(!run.hardware.is_empty());

        b.stop();
        let (ctx, _) = AddonContext::recording();
        b.start(&ctx);
        assert_eq!(b.history.runs("CPU").len(), 1, "read back when the add-on starts again");
        let status = b.rail_status(View::Cpu, &b.engine.state(), crate::platform::local_secs());
        assert!(status.starts_with("1234 Mops/sec \u{b7} today at "), "{status}");
    }

    #[test]
    fn a_memory_run_saved_before_any_frame_names_the_ram() {
        let mut b = Benchmarks::default();
        run_to_end(&mut b, Test::Memory, Unit::new("Memory", |p| {
            p.figure("Copy speed", 20.0, GBS);
            Ok(())
        }));
        let hardware = &b.history.runs("Memory")[0].hardware;
        assert!(hardware.ends_with(" RAM") && hardware != "0 B RAM", "{hardware}");
    }

    #[test]
    fn clear_history_asks_first_and_clears_only_the_selected_test() {
        let dir = history::tests::TempDir::new("addon-clear");
        let mut b = with_history(&dir);
        b.ask_clear();
        assert_eq!(b.clear_question(), None, "nothing to ask while the test has no runs");
        run_to_end(&mut b, Test::Cpu, fake_cpu(false));
        run_to_end(&mut b, Test::Memory, Unit::new("Memory", |p| {
            p.figure("Copy speed", 20.0, GBS);
            Ok(())
        }));

        b.ask_clear();
        assert_eq!(b.clear_question().as_deref(), Some("Clear CPU benchmark history?"));
        b.answer_clear(false);
        assert_eq!(b.history.runs("CPU").len(), 1, "Cancel keeps it");

        b.ask_clear();
        b.answer_clear(true);
        assert!(b.history.runs("CPU").is_empty());
        assert_eq!(b.history.runs("Memory").len(), 1, "other tests keep theirs");
        assert_eq!(b.clear_question(), None);
    }

    #[test]
    fn the_history_panel_has_the_test_columns_and_an_empty_text() {
        assert_eq!(history_title(Test::Cpu), "CPU history");
        assert_eq!(history_columns(Test::Cpu), ["Run", "Single core (Mops/sec)", "Multi core (Gop/sec)", "Score", "Version"]);
        assert_eq!(history_columns(Test::Disk), ["Run", "Write speed (MB/sec)", "Read speed (MB/sec)", "Score", "Version"]);
        assert_eq!(history_columns(Test::Internet), ["Run", "Download (Mbit/s)", "Upload (Mbit/s)", "Version"], "no score");
        assert_eq!(history::EMPTY, "Benchmark runs will appear here as they complete.");
    }

    #[test]
    fn an_unreadable_history_warns_without_blocking_the_page() {
        let dir = history::tests::TempDir::new("addon-corrupt");
        std::fs::create_dir_all(&dir.0).unwrap();
        std::fs::write(dir.file(), "garbage").unwrap();
        let mut b = with_history(&dir);
        assert!(b.history_warning.as_deref().is_some_and(|w| w.contains("set aside")), "{:?}", b.history_warning);
        assert!(b.can_start());
        run_to_end(&mut b, Test::Cpu, fake_cpu(false));
        assert_eq!(b.history.runs("CPU").len(), 1, "the new history keeps runs");
    }
}

/// Requirements, the Settings section and what turning it off keeps (#34).
#[cfg(test)]
mod settings_tests {
    use super::tests::{cards, fake_cpu, run_to_end, with_history};
    use super::*;
    use crate::addons::overview::Overview;
    use crate::addons::AddonHost;
    use serde_json::json;

    #[test]
    fn its_requirements_are_the_internet_for_the_internet_test_only_and_free_space_for_the_disk_test() {
        let (host, _) = AddonHost::for_tests(&json!({ "benchmarks": { "enabled": false } }));
        let view = Overview::of(&host, Some("benchmarks"));
        let row = view.rows.iter().find(|r| r.id == "benchmarks").unwrap();
        assert_eq!(row.needs, ["Internet"], "no administrator, no external program");
        let d = view.detail.unwrap();
        assert_eq!(d.requirements, ["Uses the internet"]);
        assert_eq!(
            d.note,
            Some("Uses the internet only for the Internet test, when you start it. The disk test needs free space in its test folder. Never asks for administrator rights.")
        );
    }

    #[test]
    fn the_folder_card_selected_test_and_summary_tile_are_saved_and_come_back_after_turning_it_off_and_on() {
        let (ctx, runner) = AddonContext::recording();
        let mut b = Benchmarks { adapters: Some(cards()), ..Default::default() };
        b.load_settings(&serde_json::Value::Null);
        b.start(&ctx);
        assert_eq!(b.settings(), serde_json::Value::Null, "nothing chosen, nothing saved");
        assert!(!b.summary_tile, "the Summary tile is off by default");

        b.disk_folder = Some("D:\\Bench".into());
        b.choose_card(&cards()[1]);
        b.selected = View::Disk;
        b.summary_tile = true;
        let saved = json!({ "diskBenchmarkFolder": "D:\\Bench", "gpuAdapter": "NVIDIA GeForce RTX 4080", "selectedTest": "disk", "summaryTile": true });
        assert_eq!(b.settings(), saved, "polled by the host while on");

        b.stop();
        assert_eq!(b.settings(), saved, "kept while off");
        b.start(&ctx);
        // Turning it off forgets the card list; give back the same cards so
        // the test does not read this PC's real ones.
        b.adapters = Some(cards());
        assert_eq!(b.settings(), saved, "and once on again");
        assert_eq!(b.selected, View::Disk);
        assert_eq!(b.gpu_card().map(|c| c.name), Some("NVIDIA GeForce RTX 4080".to_string()));
        assert!(runner.launches().is_empty());

        // The next run of the app: the card is found again by its name (its
        // LUID changes on every boot).
        let mut next = Benchmarks { adapters: Some(vec![gpu::Adapter { name: "NVIDIA GeForce RTX 4080".into(), luid: 42 }, cards()[0].clone()]), ..Default::default() };
        next.load_settings(&json!({ "gpuAdapter": "NVIDIA GeForce RTX 4080", "selectedTest": "memory", "fromANewerVersion": 1 }));
        assert_eq!(next.gpu_card().map(|c| c.luid), Some(42));
        assert_eq!(next.selected, View::Memory);
        assert_eq!(next.settings()["fromANewerVersion"], 1, "fields it does not know are kept");
    }

    #[test]
    fn settings_it_cannot_read_give_its_defaults_without_losing_the_others() {
        let mut b = Benchmarks { adapters: Some(cards()), ..Default::default() };
        b.load_settings(&json!({ "diskBenchmarkFolder": "D:\\Bench", "gpuAdapter": "A card no longer here", "selectedTest": "a-view-of-a-newer-version" }));
        assert_eq!(b.selected, View::Cpu);
        assert_eq!(b.gpu_card().map(|c| c.name), Some("AMD Radeon(TM) Graphics".to_string()), "the first listed card");
        assert_eq!(b.test_folder().ok(), Some(std::path::PathBuf::from("D:\\Bench")));
    }

    #[test]
    fn turned_off_it_keeps_its_settings_and_history_and_runs_nothing() {
        let dir = history::tests::TempDir::new("addon-contract");
        let mut b = with_history(&dir);
        run_to_end(&mut b, Test::Cpu, fake_cpu(false));
        drop(b);

        let settings = json!({ "diskBenchmarkFolder": "D:\\Bench", "gpuAdapter": "NVIDIA GeForce RTX 4080", "selectedTest": "gpu", "summaryTile": true });
        let section = json!({ "benchmarks": { "enabled": true, "settings": settings } });
        let (mut host, runner) = AddonHost::for_tests(&section);
        assert!(host.set_enabled("benchmarks", false));
        assert!(!host.visible().contains(&"benchmarks"), "out of the menu and of --page");
        assert!(!host.sync_settings());
        assert_eq!(host.settings("benchmarks"), Some(settings.clone()), "its values stay while off");
        let mut written = json!({});
        host.write_section(&mut written);
        assert_eq!(written["benchmarks"], json!({ "enabled": false, "settings": settings }));
        assert!(host.set_enabled("benchmarks", true));
        assert_eq!(host.settings("benchmarks"), Some(settings), "back once on again");
        assert!(runner.launches().is_empty(), "no program, no test started by turning it off or on");

        let again = with_history(&dir);
        assert_eq!(again.history.runs("CPU").len(), 1, "the history stays in its file");
        assert!(!again.engine.busy() && !again.engine.has_thread(), "nothing runs until Start");
    }
}

/// The score: saved with each run, the Score view and the Summary tile (#29).
#[cfg(test)]
mod score_tests {
    use super::tests::{run_to_end, with_history};
    use crate::test_ui::drawn_lines;
    use super::*;
    use crate::addons::AddonHost;
    use serde_json::json;

    /// A run of `test` measuring `figures`.
    fn fake(test: Test, figures: &'static [(&'static str, f64, Measure)]) -> Unit {
        Unit::new(test.name(), move |p| {
            for &(name, value, unit) in figures {
                p.figure(name, value, unit);
            }
            Ok(())
        })
    }

    /// Twice the reference machine's CPU (score 200).
    fn cpu_200() -> Unit {
        fake(Test::Cpu, &[(SINGLE_CORE, 152.8 * 2.0, MOPS), (MULTI_CORE, 1.868 * 2.0, GOPS)])
    }

    /// Half the reference machine's disk (score 50).
    fn disk_50() -> Unit {
        fake(Test::Disk, &[(WRITE_SPEED, 3863.0 / 2.0, MBS), (READ_SPEED, 5161.0 / 2.0, MBS)])
    }

    #[test]
    fn a_completed_run_is_saved_with_its_score() {
        let dir = history::tests::TempDir::new("score-saved");
        let mut b = with_history(&dir);
        run_to_end(&mut b, Test::Cpu, cpu_200());
        run_to_end(&mut b, Test::Memory, fake(Test::Memory, &[(COPY_SPEED, 20.0, GBS)]));
        let score = b.history.runs("CPU")[0].score;
        assert!(score.is_some_and(|s| (s - 200.0).abs() < 1e-9), "{score:?}");
        assert_eq!(b.history.runs("Memory")[0].score, None, "Memory has no score");
    }

    #[test]
    fn the_overall_score_is_the_geometric_mean_of_the_last_scored_runs_without_internet() {
        let dir = history::tests::TempDir::new("score-overall");
        let mut b = with_history(&dir);
        assert_eq!(b.overall_score(), None);
        run_to_end(&mut b, Test::Disk, disk_50());
        run_to_end(&mut b, Test::Cpu, fake(Test::Cpu, &[(SINGLE_CORE, 152.8, MOPS), (MULTI_CORE, 1.868, GOPS)]));
        run_to_end(&mut b, Test::Cpu, cpu_200());
        run_to_end(&mut b, Test::Internet, fake(Test::Internet, &[(internet::DOWNLOAD, 900.0, MBITS), (internet::UPLOAD, 90.0, MBITS)]));
        let (overall, date) = b.overall_score().unwrap();
        assert!((overall - 100.0).abs() < 1e-9, "sqrt(200 x 50), the last CPU run only: {overall}");
        assert_eq!(date, b.history.runs("CPU")[0].date, "the date of the newest scored run");
        assert_eq!(b.history.runs("Internet")[0].score, None);
    }

    #[test]
    fn the_score_view_shows_the_overall_and_each_test_with_the_reference_and_the_note() {
        let dir = history::tests::TempDir::new("score-view");
        let mut b = with_history(&dir);
        b.selected = View::Score;
        let now = crate::platform::local_secs();
        assert_eq!(b.rail_status(View::Score, &b.engine.state(), now), "Not run yet");
        run_to_end(&mut b, Test::Cpu, cpu_200());
        run_to_end(&mut b, Test::Disk, disk_50());
        assert_eq!(b.rail_status(View::Score, &b.engine.state(), now), "100");
        assert_eq!(View::Score.label(), "Ultimate score");

        let texts = drawn_lines(|ui, page| b.ui(ui, page));
        for wanted in [
            "Ultimate score",
            "100",
            "CPU",
            "200",
            "Disk",
            "50",
            "GPU",
            "Not run yet",
        ] {
            assert!(texts.iter().any(|t| t == wanted), "{wanted:?} not drawn in {texts:?}");
        }
        let note = "100 is the reference machine: an AMD Ryzen 5 7600X with an NVIDIA GeForce RTX 4080 and a Samsung 980 PRO. \
            Score comparisons are valid only per version, per platform. Internet and Memory have no score.";
        assert!(texts.iter().any(|t| t == note), "one paragraph: {texts:?}");
    }

    #[test]
    fn the_summary_tile_gives_the_last_overall_score_and_its_date() {
        let dir = history::tests::TempDir::new("score-tile");
        let mut b = with_history(&dir);
        assert_eq!(b.summary_lines(), None, "the tile is off by default");
        b.load_settings(&json!({ "summaryTile": true }));
        assert_eq!(b.summary_lines(), Some(vec!["No score yet".to_string()]));
        run_to_end(&mut b, Test::Cpu, cpu_200());
        run_to_end(&mut b, Test::Disk, disk_50());
        let date = b.history.runs("Disk")[0].date.clone();
        let shown = format!("Last run {}", date[..16].replace('T', " "));
        assert_eq!(b.summary_lines(), Some(vec!["Ultimate score 100".to_string(), shown]));
    }

    #[test]
    fn the_summary_tile_is_gone_with_the_addon_off() {
        let section = json!({ "benchmarks": { "enabled": true, "settings": { "summaryTile": true } } });
        let (mut host, _) = AddonHost::for_tests(&section);
        let on = drawn_lines(|ui, page| host.summary_tiles(ui, page));
        assert!(on.iter().any(|t| t == "Benchmarks") && on.iter().any(|t| t == "No score yet"), "{on:?}");
        assert!(host.set_enabled("benchmarks", false));
        let off = drawn_lines(|ui, page| host.summary_tiles(ui, page));
        assert!(!off.iter().any(|t| t == "Benchmarks" || t == "No score yet"), "{off:?}");
    }
}

/// "Run All Benchmarks" (#33).
#[cfg(test)]
mod run_all_tests {
    use crate::test_ui::drawn_lines;
    use super::tests::with_history;
    use super::*;
    use std::sync::mpsc::{channel, Sender};

    /// Waits for the run to end and notes how each test ended.
    fn finish(b: &mut Benchmarks) {
        engine::wait_idle(&b.engine);
        b.settle();
    }

    /// A fake run of `test` that tells it ran and measures the reference
    /// machine's figures (score 100).
    fn fake(test: Test, ran: Sender<Test>) -> Unit {
        let figures: &'static [(&'static str, f64, Measure)] = match test {
            Test::Cpu => &[(SINGLE_CORE, 152.8, MOPS), (MULTI_CORE, 1.868, GOPS)],
            Test::Disk => &[(WRITE_SPEED, 3863.0, MBS), (READ_SPEED, 5161.0, MBS)],
            Test::Internet => &[(internet::DOWNLOAD, 900.0, MBITS), (internet::UPLOAD, 90.0, MBITS)],
            _ => &[],
        };
        Unit::new(test.name(), move |p| {
            ran.send(test).unwrap();
            for &(name, value, unit) in figures {
                p.figure(name, value, unit);
            }
            Ok(())
        })
    }

    #[test]
    fn run_all_runs_cpu_gpu_disk_internet_in_order_and_updates_the_score() {
        let dir = history::tests::TempDir::new("run-all");
        let mut b = with_history(&dir);
        let (ran, seen) = channel();
        assert!(b.run_all_with(|_, test| fake(test, ran.clone())));
        finish(&mut b);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), [Test::Cpu, Test::Gpu, Test::Disk, Test::Internet]);
        for test in [Test::Cpu, Test::Gpu, Test::Disk, Test::Internet] {
            assert_eq!(b.history.runs(test.name()).len(), 1, "{test:?} saved");
        }
        let (overall, _) = b.overall_score().expect("the score is updated at the end");
        assert!((overall - 100.0).abs() < 1e-9, "{overall}");
        assert_eq!(b.engine.state().status().as_deref(), Some("Test complete"));
    }

    #[test]
    fn the_band_tells_the_test_done_and_the_test_running_and_cancel_stops_the_sequence() {
        let dir = history::tests::TempDir::new("run-all-cancel");
        let mut b = with_history(&dir);
        let (ran, seen) = channel();
        let (started, wait_started) = channel();
        b.run_all_with(|_, test| match test {
            Test::Gpu => {
                let (ran, started) = (ran.clone(), started.clone());
                Unit::new(test.name(), move |p| {
                    ran.send(test).unwrap();
                    p.phase("Testing GPU graphics");
                    started.send(()).unwrap();
                    let t = Instant::now();
                    while !p.cancelled() && t.elapsed() < Duration::from_secs(5) {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                })
            }
            _ => fake(test, ran.clone()),
        });
        wait_started.recv().unwrap();
        let line = "Running all benchmarks: CPU done, now testing GPU graphics";
        assert_eq!(b.run_all_line(&b.engine.state()).as_deref(), Some(line));
        let texts = drawn_lines(|ui, page| b.ui(ui, page));
        assert!(texts.iter().any(|t| t == line), "{texts:?}");
        assert!(texts.iter().any(|t| t == "Cancel"), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Run All Benchmarks"), "{texts:?}");

        b.cancel();
        finish(&mut b);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), [Test::Cpu, Test::Gpu], "Disk and Internet never ran");
        assert_eq!(b.engine.state().status().as_deref(), Some("Cancelled"));
        assert_eq!(b.history.runs("CPU").len(), 1, "the CPU run is kept");
        assert!(b.history.runs("GPU").is_empty());
        assert_eq!(b.ended.get("GPU"), Some(&RunEnd::Cancelled));
        assert_eq!(b.ended.get("Disk"), None, "never ran");
        assert_eq!(b.run_all_line(&b.engine.state()), None);
    }

    #[test]
    fn a_failing_test_does_not_stop_run_all_nor_touch_its_previous_results() {
        let dir = history::tests::TempDir::new("run-all-fail");
        let mut b = with_history(&dir);
        let (ran, seen) = channel();
        b.run_all_with(|_, test| fake(test, ran.clone()));
        finish(&mut b);
        let _ = seen.try_iter().count();

        b.run_all_with(|_, test| match test {
            Test::Disk => Unit::new(test.name(), |_| Err("Not enough free space to run the test.".into())),
            _ => fake(test, ran.clone()),
        });
        finish(&mut b);
        assert_eq!(seen.try_iter().collect::<Vec<_>>(), [Test::Cpu, Test::Gpu, Test::Internet]);
        assert_eq!(b.history.runs("CPU").len(), 2);
        assert_eq!(b.history.runs("Internet").len(), 2);
        assert_eq!(b.history.runs("Disk").len(), 1, "the earlier Disk run is kept");
        assert_eq!(b.ended.get("Disk"), Some(&RunEnd::Failed));
        assert_eq!(b.engine.state().status().as_deref(), Some("Disk test failed: Not enough free space to run the test."));
        assert!(b.overall_score().is_some());
    }

    #[test]
    fn the_band_names_a_failed_test_as_failed_not_done() {
        let dir = history::tests::TempDir::new("run-all-band-failed");
        let mut b = with_history(&dir);
        let (ran, _seen) = channel();
        let (started, wait_started) = channel();
        b.run_all_with(|_, test| match test {
            Test::Gpu => Unit::new(test.name(), |_| Err("no card".into())),
            Test::Disk => {
                let started = started.clone();
                Unit::new(test.name(), move |p| {
                    started.send(()).unwrap();
                    while !p.cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                })
            }
            _ => fake(test, ran.clone()),
        });
        wait_started.recv().unwrap();
        assert_eq!(b.run_all_line(&b.engine.state()).as_deref(), Some("Running all benchmarks: CPU done, GPU failed, now testing Disk"));
        b.cancel();
        finish(&mut b);
    }

    #[test]
    fn the_rail_marks_the_tests_run_all_has_still_to_run_as_waiting() {
        let dir = history::tests::TempDir::new("run-all-waiting");
        let mut b = with_history(&dir);
        let (ran, _seen) = channel();
        let (started, wait_started) = channel();
        b.run_all_with(|_, test| match test {
            Test::Gpu => {
                let started = started.clone();
                Unit::new(test.name(), move |p| {
                    started.send(()).unwrap();
                    while !p.cancelled() {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Ok(())
                })
            }
            _ => fake(test, ran.clone()),
        });
        wait_started.recv().unwrap();
        let now = crate::platform::local_secs();
        let st = b.engine.state();
        assert_eq!(b.rail_status(View::Disk, &st, now), "Waiting");
        assert_eq!(b.rail_status(View::Internet, &st, now), "Waiting");
        assert_eq!(b.rail_status(View::Memory, &st, now), "Not run yet", "not part of Run All");
        assert!(!b.rail_status(View::Cpu, &st, now).starts_with("Waiting"), "CPU already ran");
        b.cancel();
        finish(&mut b);
        assert_eq!(b.rail_status(View::Disk, &b.engine.state(), now), "Not run yet", "a cancel keeps it from running");
    }

    #[test]
    fn run_all_is_in_the_band_and_in_the_benchmarks_menu_when_idle() {
        let mut b = Benchmarks::default();
        let band = drawn_lines(|ui, page| b.ui(ui, page));
        assert!(band.iter().any(|t| t == "Run All Benchmarks"), "{band:?}");
        let menu = drawn_lines(|ui, page| b.header_menu(ui, page));
        assert!(menu.iter().any(|t| t == "Run All Benchmarks"), "{menu:?}");
    }
}
