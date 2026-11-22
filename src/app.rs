//! The window: a sidebar of pages and one panel per page (see pages.rs).

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use egui::{vec2, Align, Color32, Layout, Pos2, RichText, Sense, Stroke, Ui};

use crate::addons::{AddonHost, Core, Landing};
pub use crate::platform::Action;
use crate::platform::PageContext;
use crate::sampler::{Sampler, Snapshot};
use crate::settings::{Settings, SPEEDS};
use crate::widgets::{fmt_bytes, health_colour, local_ip, logo, nav_entry_back, nav_heading, push, refresh_button};
use crate::system_lists::{ServiceRow, StartupRow, SystemLists};
use crate::winsys;

/// The open page: one of the core pages, or the page of an add-on (by its
/// registry id; the add-on draws it through the host).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Core(Core),
    Addon(&'static str),
}

impl Page {
    /// The page's stable identifier in the add-on registry.
    pub fn id(self) -> &'static str {
        match self {
            Page::Core(core) => core.id(),
            Page::Addon(id) => id,
        }
    }
    /// The page with this registry id: a core page, else an add-on's page.
    pub fn of(id: &'static str) -> Page {
        Core::of(id).map_or(Page::Addon(id), Page::Core)
    }
}

/// One row of the sidebar's list.
#[derive(Debug)]
pub struct NavItem {
    pub page: Page,
    pub name: &'static str,
    /// The "Add-ons" heading goes above this row.
    heading: bool,
}

/// The sidebar's list: the core pages, then an "Add-ons" heading and the
/// add-ons that are on, in registry order. Settings and Colors are drawn at
/// the bottom, not here.
pub fn nav_items(host: &AddonHost) -> Vec<NavItem> {
    let mut heading = false;
    host.visible()
        .into_iter()
        .filter(|id| !matches!(Page::of(id), Page::Core(Core::Settings) | Page::Core(Core::Colors)))
        .map(|id| {
            let first_addon = !heading && !host.is_core(id);
            heading |= first_addon;
            NavItem { page: Page::of(id), name: page_name(host, Page::of(id)), heading: first_addon }
        })
        .collect()
}

/// The page's shown name, from the registry.
pub fn page_name(host: &AddonHost, page: Page) -> &'static str {
    let id = page.id();
    host.entries().find(|(e, _)| e.id() == id).map_or(id, |(e, _)| e.name())
}

/// How the native providers did in the current sample. The status bar, the
/// Summary band and the Summary view all read this one state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProviderState {
    /// No sample has arrived yet.
    #[default]
    Waiting,
    /// Every performance counter and the GetPerformanceInfo call answered.
    Healthy,
    /// Some of them failed: their figures are gaps.
    Degraded,
    /// None of them answered.
    Unavailable,
}

impl ProviderState {
    /// The state of a sample, from the sources (counters and the
    /// GetPerformanceInfo call) that answered in it.
    pub fn of(s: &Snapshot) -> Self {
        let failed = crate::perfdata::sources().filter(|c| !s.perf.answered(c)).count();
        match failed {
            _ if s.seq == 0 => ProviderState::Waiting,
            0 => ProviderState::Healthy,
            n if n == crate::perfdata::sources().count() => ProviderState::Unavailable,
            _ => ProviderState::Degraded,
        }
    }
}

/// The status bar's words, as in the Design A anatomy: provider health,
/// process count, IP address and sample generation (joined with " · ").
pub fn status_parts(paused: bool, providers: ProviderState, procs: usize, ip: &str, generation: u64) -> [String; 4] {
    let state = match providers {
        _ if paused => "Updates paused",
        ProviderState::Healthy => "Native providers healthy",
        ProviderState::Degraded => "Native providers degraded",
        ProviderState::Unavailable => "Native providers unavailable",
        ProviderState::Waiting => "Starting\u{2026}",
    };
    [state.to_string(), format!("{procs} processes"), format!("IP {ip}"), format!("Generation {generation}")]
}

/// A column of the Processes table, in display order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ProcCol {
    Name,
    Pid,
    Status,
    User,
    Cpu,
    Mem,
    DiskRead,
    DiskWrite,
    Threads,
    Gpu,
    PeakMem,
    CpuTime,
    PageFaults,
    Handles,
    Priority,
    Arch,
    Cmd,
    /// The program file's verified signer (a marked addition).
    Publisher,
    /// The program file's version-details description (a marked addition).
    Description,
    /// The column an add-on that is on adds, by the add-on's id (an
    /// addition; see `Addon::process_column`).
    Addon(&'static str),
}

/// A device in the Performance page's list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PerfItem {
    Cpu,
    Memory,
    /// Physical disk number.
    Disk(u32),
    /// All network interfaces combined.
    Network,
    /// GPU adapter LUID.
    Gpu(u64),
    /// CPU package power.
    Power,
}

impl PerfItem {
    /// Saved in settings so the page reopens on the same device.
    pub fn key(self) -> String {
        match self {
            PerfItem::Cpu => "cpu".into(),
            PerfItem::Memory => "memory".into(),
            PerfItem::Disk(n) => format!("disk:{n}"),
            PerfItem::Network => "network".into(),
            PerfItem::Gpu(l) => format!("gpu:{l}"),
            PerfItem::Power => "power".into(),
        }
    }
    pub fn from_key(s: &str) -> PerfItem {
        let (kind, arg) = s.split_once(':').unwrap_or((s, ""));
        match kind {
            "memory" => PerfItem::Memory,
            "disk" => arg.parse().map(PerfItem::Disk).unwrap_or(PerfItem::Cpu),
            "network" => PerfItem::Network,
            "gpu" => arg.parse().map(PerfItem::Gpu).unwrap_or(PerfItem::Cpu),
            "power" => PerfItem::Power,
            _ => PerfItem::Cpu,
        }
    }
}

/// Asked when another page goes to a process Hide Microsoft Services and
/// Processes keeps out of the table.
pub const SHOW_MICROSOFT_QUESTION: &str = "Processes does not list this process while Hide Microsoft Services and Processes is on. Turn it off?";

/// Asked once Create memory dump file has written its file.
pub const DUMP_QUESTION: &str = "Show it in File Explorer?";

pub struct TaskApp {
    pub settings: Settings,
    /// The add-on registry and which add-ons are on.
    pub addons: AddonHost,
    /// Settings › Add-ons: the add-on picked out (e.g. by `--page` on one that is off).
    pub addon_selected: Option<&'static str>,
    /// Settings shows its Add-ons tab (set by `--page`, then by the tabs).
    pub show_addons: bool,
    /// Settings: the category shown, and what "Find a setting" holds.
    pub settings_view: crate::pages2::SettingsView,
    pub sampler: Sampler,
    pub snap: Snapshot,
    pub cpu_hist: VecDeque<f32>,
    pub mem_hist: VecDeque<f32>,
    pub disk_hist: VecDeque<f32>,
    pub net_hist: BTreeMap<String, VecDeque<f32>>,
    /// Long histories for the Performance page graphs, by series name.
    pub ph: BTreeMap<String, VecDeque<f32>>,
    /// What the Summary page draws, built once per new sample.
    pub summary: crate::summary_view::SummaryView,
    /// The machine's fixed facts (base clock, disk models, GPUs) when set (tests);
    /// otherwise the machine's are read (see [`TaskApp::machine`]).
    pub machine_info: Option<crate::perfdata::StaticInfo>,
    /// The latest sensor frame Summary's tiles read (GPU temperature and
    /// power, S.M.A.R.T.); `None` when no sensor has reported.
    pub summary_frame: Option<std::sync::Arc<crate::sensors::Frame>>,
    pub page: Page,
    #[allow(dead_code)]
    pub nav_open: bool,
    pub perf: PerfItem,
    pub search: String,
    /// The Processes table's layout as last saved (see
    /// [`TaskApp::process_table_unsaved`]).
    process_table_saved: crate::settings::ProcessTable,
    /// The process rows selected on Processes (Ctrl and Shift add rows).
    pub selected: crate::process_view::Selection,
    /// Processes scrolls to the selected process once (after "Go to the
    /// process" or type-ahead).
    pub reveal_selected: bool,
    /// Follow process: the process Processes keeps selected and in view.
    pub following: Option<crate::process_view::Follow>,
    /// Processes: which tree rows are open and which groups are folded.
    pub proc_view: crate::process_view::ViewState,
    /// What the Services and Startup apps pages read and control: the
    /// machine in the app, a fake in tests (see [`crate::system_lists`]).
    pub(crate) system: std::sync::Arc<dyn SystemLists>,
    pub services: Option<Result<Vec<ServiceRow>, String>>,
    pub svc_selected: Option<String>,
    pub startup: Option<Vec<StartupRow>>,
    pub startup_selected: Option<usize>,
    pub confirm_end: Option<crate::pages::EndAsk>,
    /// Efficiency mode's question, open while it waits for an answer.
    pub confirm_efficiency: Option<crate::pages::EfficiencyAsk>,
    /// The processes Set priority ▸ Realtime waits to change while its
    /// question is open.
    pub confirm_realtime: Option<Vec<u32>>,
    /// The process another page went to while "Hide Microsoft Services and
    /// Processes" hides it: Processes asks whether to turn the option off.
    pub ask_show_microsoft: Option<u32>,
    /// The memory dump file just written, while [`DUMP_QUESTION`] waits
    /// for an answer.
    pub dump_question: Option<std::path::PathBuf>,
    /// The memory dumps being written off the UI thread, each sending
    /// the file written or why Windows refused.
    dumps: Vec<std::sync::mpsc::Receiver<Result<std::path::PathBuf, String>>>,
    /// Restarts of Windows Explorer running off the UI thread, each
    /// sending how it went.
    explorer_restarts: Vec<std::sync::mpsc::Receiver<Result<(), String>>>,
    /// Who signed each program file, checked off the UI thread.
    pub signatures: crate::signature::Signatures,
    /// Each program file's description, read off the UI thread.
    pub descriptions: crate::version_info::Descriptions,
    pub run_dialog: Option<String>,
    /// The open Process Inspector windows, one per process.
    pub inspectors: Vec<crate::inspector::Inspector>,
    /// The open Analyze wait chain windows, one per process.
    pub wait_chains: Vec<crate::wait_chain::WaitChain>,
    /// The open File activity windows, one per process; each holds its
    /// trace, stopped when the window goes.
    pub file_activity: Vec<crate::file_activity::FileActivity>,
    pub message: Option<(String, Instant)>,
    /// What the app asks of the system (see [`crate::system_actions`]): the
    /// Windows adapter, or in tests a recording one that acts on nothing.
    pub actions: std::sync::Arc<dyn crate::system_actions::SystemActions>,
    /// The recording adapter behind `actions` when the app is headless.
    #[cfg(test)]
    recording: Option<std::sync::Arc<crate::system_actions::RecordingActions>>,
    /// App history: per program name, (CPU seconds, disk bytes).
    pub history: BTreeMap<String, (f64, f64)>,
    pub history_since: Instant,
    last_sample_at: Option<Instant>,
    applied: Option<bool>,
    applied_top: Option<bool>,
    screenshot: Option<String>,
    frames: u32,
    shot_sent_at: Option<u32>,
    /// The add-ons' questions shown before closing (see
    /// [`AddonHost::close_questions`]).
    close_questions: Option<Vec<String>>,
    /// Closing was confirmed: the app closes once the add-ons' current
    /// steps end ([`AddonHost::can_close`]).
    closing: bool,
}

impl TaskApp {
    pub fn new(cc: &eframe::CreationContext<'_>, screenshot: Option<String>) -> Self {
        let mut settings = Settings::load();
        // `--theme dark|light` picks the theme for this run, not the saved one.
        if let Some(mode) = crate::theme::mode_from_args(&std::env::args().collect::<Vec<_>>()) {
            settings.theme_for_this_run(mode == crate::theme::Mode::Dark);
        }
        let mut addons = AddonHost::from_section(&settings.addons);
        // The sampler reads the performance counters through the sensor
        // service every page shares, so each counter is read once.
        let sampler = Sampler::start(settings.interval_ms(), addons.context().sensors().clone());
        // `--page <id or name>` opens a given page (handy for pictures of it).
        let args: Vec<String> = std::env::args().collect();
        // Flags owned by add-ons (e.g. `--scan`); one that is off ignores them.
        // An add-on's flag may ask for its page; `--page` wins.
        let asked = addons.command_line(&args).page;
        let request = args.iter().position(|a| a == "--page").and_then(|i| args.get(i + 1)).map(String::as_str).or(asked);
        let (page, addon_selected, show_addons) = match addons.landing(request, &settings.start_page, &settings.last_page) {
            Landing::Page(id) => (Page::of(id), None, false),
            Landing::AddonSettings(sel) => (Page::Core(Core::Settings), sel, true),
        };
        let mut app = Self::build(settings, sampler, addons, page, screenshot, std::sync::Arc::new(crate::system_lists::native::NativeSystem), std::sync::Arc::new(crate::system_actions::WindowsActions));
        app.addon_selected = addon_selected;
        app.show_addons = show_addons;
        crate::theme::install_fonts(&cc.egui_ctx);
        app.apply_theme(&cc.egui_ctx);
        app
    }

    /// An app on `page` with `settings`, before any frame is drawn.
    fn build(
        settings: Settings,
        sampler: Sampler,
        addons: AddonHost,
        page: Page,
        screenshot: Option<String>,
        system: std::sync::Arc<dyn SystemLists>,
        actions: std::sync::Arc<dyn crate::system_actions::SystemActions>,
    ) -> Self {
        let mut app = TaskApp {
            addons,
            addon_selected: None,
            show_addons: false,
            settings_view: Default::default(),
            settings,
            sampler,
            snap: Snapshot::default(),
            cpu_hist: VecDeque::new(),
            mem_hist: VecDeque::new(),
            disk_hist: VecDeque::new(),
            net_hist: BTreeMap::new(),
            ph: BTreeMap::new(),
            summary: Default::default(),
            machine_info: None,
            summary_frame: None,
            page,
            nav_open: true,
            perf: PerfItem::Cpu,

            search: String::new(),
            process_table_saved: Default::default(),
            selected: Default::default(),
            reveal_selected: false,
            following: None,
            proc_view: Default::default(),
            system,
            services: None,
            svc_selected: None,
            startup: None,
            startup_selected: None,
            confirm_end: None,
            confirm_efficiency: None,
            confirm_realtime: None,
            ask_show_microsoft: None,
            dump_question: None,
            dumps: Vec::new(),
            explorer_restarts: Vec::new(),
            signatures: Default::default(),
            descriptions: Default::default(),
            run_dialog: None,
            inspectors: Vec::new(),
            wait_chains: Vec::new(),
            file_activity: Vec::new(),
            message: None,
            actions,
            #[cfg(test)]
            recording: None,
            history: BTreeMap::new(),
            history_since: Instant::now(),
            last_sample_at: None,
            applied: None,
            applied_top: None,
            screenshot,
            frames: 0,
            shot_sent_at: None,
            close_questions: None,
            closing: false,
        };
        app.perf = PerfItem::from_key(&app.settings.perf_device);
        app.process_table_saved = app.settings.process_table.clone();
        app.rebuild_summary();
        app
    }

    /// An app for tests: no sampler thread, the standard add-ons, the
    /// snapshot given.
    #[cfg(test)]
    pub fn headless(settings: Settings, snap: Snapshot) -> Self {
        let (_, rx) = std::sync::mpsc::channel();
        let sampler = Sampler::detached(rx);
        let addons = AddonHost::from_section(&serde_json::json!({}));
        let mut app = Self::recording_build(settings, sampler, addons);
        app.snap = snap;
        app.rebuild_summary();
        app
    }

    /// An app for tests over a sensor service made of fake providers: a real
    /// sampler holds its interest in the counter providers (the rest of a
    /// snapshot, `fill`, is the test's) and the add-ons read the same
    /// service. Samples reach the app when the test calls `pull_samples`.
    #[cfg(test)]
    pub fn headless_over(settings: Settings, sensors: crate::sensors::SensorService, fill: impl FnMut(&mut Snapshot) + Send + 'static) -> Self {
        let sampler = Sampler::start_with(settings.interval_ms(), sensors.clone(), move || Box::new(fill));
        let addons = AddonHost::over_sensors(sensors);
        let mut app = Self::recording_build(settings, sampler, addons);
        app.rebuild_summary();
        app
    }

    /// A headless app on Summary whose system actions are recorded and
    /// acted on nothing (see [`TaskApp::recording`]).
    #[cfg(test)]
    fn recording_build(settings: Settings, sampler: Sampler, addons: AddonHost) -> Self {
        let recording = std::sync::Arc::new(crate::system_actions::RecordingActions::default());
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        let mut app = Self::build(settings, sampler, addons, Page::Core(Core::Summary), None, fake, recording.clone());
        app.recording = Some(recording);
        app
    }

    /// The recording adapter of a headless app: what it was asked, and
    /// what it is scripted to refuse.
    #[cfg(test)]
    pub fn recording(&self) -> std::sync::Arc<crate::system_actions::RecordingActions> {
        self.recording.clone().expect("only a headless app records")
    }

    /// This app over `system` instead of the default fake: tests keep their
    /// own `Arc<FakeSystem>` to script it and read what was asked.
    #[cfg(test)]
    pub fn on_system(mut self, system: std::sync::Arc<dyn SystemLists>) -> Self {
        self.system = system;
        self
    }

    /// [`TaskApp::headless`] with a sender standing in for the sampler.
    #[cfg(test)]
    pub fn fed(settings: Settings, snap: Snapshot) -> (Self, std::sync::mpsc::Sender<Snapshot>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut app = Self::headless(settings, snap);
        app.sampler.rx = rx;
        (app, tx)
    }

    /// Whether the Processes table's layout changed since it was last
    /// saved (it is then counted as saved). Not while the mouse is down, so
    /// dragging a divider saves once, on release.
    pub fn process_table_unsaved(&mut self, pointer_down: bool) -> bool {
        if pointer_down || self.settings.process_table == self.process_table_saved {
            return false;
        }
        self.process_table_saved = self.settings.process_table.clone();
        true
    }

    /// Sets egui's visuals from the theme of the dark or light setting.
    fn apply_theme(&mut self, ctx: &egui::Context) {
        let dark = self.settings.dark;
        if self.applied == Some(dark) {
            return;
        }
        self.applied = Some(dark);
        crate::theme::apply(ctx, crate::theme::Mode::from_dark(dark));
    }

    /// The Design A theme of the dark or light setting.
    fn theme(&self) -> crate::theme::Theme {
        crate::theme::Theme::of(crate::theme::Mode::from_dark(self.settings.dark))
    }

    pub fn sub_text(&self) -> Color32 {
        crate::widgets::sub_text(&self.theme())
    }

    pub fn say(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
    }

    /// Turns an add-on on or off at once and saves the choice.
    pub fn set_addon(&mut self, id: &str, on: bool) {
        if self.addons.set_enabled(id, on) {
            self.save_addons();
        }
    }

    /// Saves every add-on's state and, if the open page's add-on is now
    /// off, goes to Summary.
    fn save_addons(&mut self) {
        self.addons.write_section(&mut self.settings.addons);
        self.settings.save();
        self.page = Page::of(self.addons.resolve_page(self.page.id()));
    }

    /// Completes the stops that waited for an operation to end (see
    /// [`AddonHost::settle`]) and saves them.
    fn settle_addons(&mut self) {
        if !self.addons.settle().is_empty() {
            self.save_addons();
        }
    }

    /// Saves the settings the add-ons changed on their pages (not while
    /// taking a screenshot, which must leave the user's settings alone).
    fn save_addon_settings(&mut self) {
        if self.addons.sync_settings() && self.screenshot.is_none() {
            self.addons.write_section(&mut self.settings.addons);
            self.settings.save();
        }
    }

    pub(crate) fn pull_samples(&mut self) {
        let mut got = None;
        let paused = self.settings.interval_ms() == 0;
        while let Ok(s) = self.sampler.rx.try_recv() {
            // Paused, only a sample taken on Refresh counts (one already on
            // its way when the user paused does not change the pages).
            if paused && !s.forced {
                continue;
            }
            // Every new sample reaches the add-ons that are on, page open or not.
            self.addons.on_sample(&s);
            got = Some(s);
        }
        let Some(s) = got else { return };
        self.push_perf(&s);
        push(&mut self.cpu_hist, s.cpu_total);
        let mem_pct = if s.mem_total > 0 { s.mem_used as f32 * 100.0 / s.mem_total as f32 } else { 0.0 };
        push(&mut self.mem_hist, mem_pct);
        push(&mut self.disk_hist, ((s.disk_read_bps + s.disk_write_bps) / 1_048_576.0) as f32);
        for n in &s.net {
            let h = self.net_hist.entry(n.name.clone()).or_default();
            push(h, ((n.rx_bps + n.tx_bps) * 8.0 / 1000.0) as f32);
        }
        let now = Instant::now();
        if let Some(prev) = self.last_sample_at {
            let dt = now.duration_since(prev).as_secs_f64().min(10.0);
            let ncpu = s.cpu_per_core.len().max(1) as f64;
            for p in &s.procs {
                let e = self.history.entry(p.name.clone()).or_default();
                e.0 += p.cpu as f64 / 100.0 * ncpu * dt;
                e.1 += (p.disk_read + p.disk_write) * dt;
            }
        }
        self.last_sample_at = Some(now);
        // The Processes view notices the processes that ended (it keeps
        // them a moment; the sampler does not).
        self.proc_view.track(&s.procs, now, self.settings.show_exited);
        self.snap = s;
        self.rebuild_summary();
        self.ask_signatures();
    }

    pub fn set_speed(&mut self, idx: usize) {
        self.settings.speed = idx;
        self.sampler.interval_ms.store(self.settings.interval_ms(), Ordering::Relaxed);
        self.settings.save();
    }

    /// Ends a frame after the page is drawn: the add-on host drops what is no
    /// longer shown, and Summary's sensor frame goes with it when Summary was
    /// not drawn (coming back never shows an old one).
    pub(crate) fn end_frame(&mut self) {
        let dropped = self.addons.end_frame();
        if dropped.contains(&crate::addons::leases::Surface::Summary) {
            self.summary_frame = None;
        }
    }

    pub fn refresh_now(&mut self) {
        self.sampler.refresh();
        match self.page {
            Page::Core(Core::Services) => self.reload_services(),
            Page::Core(Core::Startup) => self.reload_startup(),
            p => {
                // An add-on refreshes itself and reads its sensors; for a core
                // page, only the sensors it holds (Summary's) are read.
                if !self.addons.refresh(p.id()) {
                    self.addons.read_held_sensors();
                }
            }
        }
    }

    pub fn fmt_net(&self, bytes_per_sec: f64) -> String {
        if self.settings.bits_per_second {
            let b = bytes_per_sec * 8.0;
            if b >= 1e9 {
                format!("{:.1} Gbps", b / 1e9)
            } else if b >= 1e6 {
                format!("{:.1} Mbps", b / 1e6)
            } else {
                format!("{:.0} Kbps", b / 1e3)
            }
        } else {
            format!("{}/s", fmt_bytes(bytes_per_sec as u64))
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint();
        self.frames += 1;
        let resend = self.shot_sent_at.map(|f| self.frames > f + 240).unwrap_or(false);
        // Wait for real data (two samples) before the picture, at most ~900 frames.
        let ready = self.snap.seq >= 2 || self.frames >= 900;
        if (self.frames >= 120 && ready && self.shot_sent_at.is_none()) || resend {
            self.shot_sent_at = Some(self.frames);
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let [w, h] = img.size;
            let mut bytes = Vec::with_capacity(w * h * 4);
            for p in &img.pixels {
                bytes.extend_from_slice(&p.to_array());
            }
            if let Some(dir) = std::path::Path::new(&path).parent() {
                if !dir.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(dir);
                }
            }
            if let Err(e) = image::save_buffer(&path, &bytes, w as u32, h as u32, image::ColorType::Rgba8) {
                eprintln!("could not save screenshot: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

impl eframe::App for TaskApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.apply_theme(ctx);
        if self.applied_top != Some(self.settings.always_on_top) && self.screenshot.is_none() {
            self.applied_top = Some(self.settings.always_on_top);
            let level = if self.settings.always_on_top { egui::WindowLevel::AlwaysOnTop } else { egui::WindowLevel::Normal };
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(level));
        }
        self.pull_samples();
        self.settle_addons();
        self.addons.tell_package_changes();
        self.guard_close(ctx);
        self.shortcuts(ctx);
        let mut actions: Vec<Action> = Vec::new();

        self.menu_bar(ctx, &mut actions);
        self.nav_panel(ctx);
        self.status_bar(ctx, &mut actions);
        let frame = egui::Frame::new().fill(self.theme().page).inner_margin(egui::Margin { left: 22, right: 18, top: 0, bottom: 8 });
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| match self.page {
            Page::Core(Core::Summary) => self.page_summary(ui, &mut actions),
            Page::Core(Core::Performance) => self.page_performance(ui),
            Page::Core(Core::Processes) => self.page_processes(ui, &mut actions),
            Page::Core(Core::SystemInfo) => self.page_system(ui),
            Page::Core(Core::Startup) => self.page_startup(ui, &mut actions),
            Page::Core(Core::Users) => self.page_users(ui),
            Page::Core(Core::Services) => self.page_services(ui, &mut actions),
            Page::Addon(id) => self.addon_page(ui, id, &mut actions),
            Page::Core(Core::AppHistory) => self.page_history(ui),
            Page::Core(Core::Settings) => self.page_settings(ui, &mut actions),
            Page::Core(Core::Colors) => self.page_colors(ui),
        });
        self.end_frame();
        self.dialogs(ctx, &mut actions);
        self.off_question(ctx);
        self.close_question(ctx);
        for a in actions {
            self.run_action(ctx, a);
        }
        self.save_addon_settings();
        // Columns hidden, moved or resized on Processes (or reset on
        // Settings) are saved like the other settings.
        let pointer_down = ctx.input(|i| i.pointer.any_down());
        if self.process_table_unsaved(pointer_down) && self.screenshot.is_none() {
            self.settings.save();
        }

        if self.screenshot.is_none() && self.settings.last_page != self.page.id() {
            self.settings.last_page = self.page.id().to_string();
            self.settings.save();
        }
        self.handle_screenshot(ctx);
        let wait = match self.settings.interval_ms() {
            0 => 500,
            ms => ms.min(250),
        };
        ctx.request_repaint_after(Duration::from_millis(wait));
    }

    /// The app quits: no File activity trace outlives it (dropping the app
    /// stops them too; this stops them before anything else is torn down),
    /// and the notification icon goes with it (no ghost icon in the
    /// notification area).
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.close_file_activity();
        crate::platform::toast::shutdown();
    }
}

/// Run new task's shortcut, in the File menu.
const RUN_NEW_TASK: egui::KeyboardShortcut = egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::N);

impl TaskApp {
    /// The keyboard shortcuts: F5 refreshes once per press,
    /// Ctrl+N opens Run new task (File), keeping the text of one already open.
    fn shortcuts(&mut self, ctx: &egui::Context) {
        // A held key repeats; only a fresh press refreshes.
        let f5 = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Key { key: egui::Key::F5, pressed: true, repeat: false, .. })));
        if f5 {
            self.refresh_now();
        }
        if ctx.input_mut(|i| i.consume_shortcut(&RUN_NEW_TASK)) {
            self.run_dialog.get_or_insert_with(String::new);
        }
    }

    /// Asks before closing while an add-on works (e.g. a series of
    /// installs). Once confirmed, the add-ons stop as when turned off and
    /// the window closes when their current steps end.
    fn guard_close(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) {
            if self.closing {
                if !self.addons.can_close() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                }
            } else if self.addons.request_close() {
                // Unsaved work is asked about first (see `close_question`);
                // the next close request comes here again.
                let questions = self.addons.close_questions();
                if !questions.is_empty() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.close_questions = Some(questions);
                }
            }
        }
        if self.closing && self.addons.can_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let Some(questions) = self.close_questions.clone() else { return };
        let sub = self.sub_text();
        let mut answer = None;
        egui::Window::new("Close the app?")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                if self.closing {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Closing when the current step ends\u{2026}");
                    });
                    return;
                }
                for q in &questions {
                    ui.label(RichText::new(q).color(sub));
                }
                ui.horizontal(|ui| {
                    use crate::widgets::parts::{outline_button, primary_button};
                    if primary_button(ui, "Close", true).clicked() {
                        answer = Some(true);
                    }
                    if outline_button(ui, "Cancel", true).clicked() {
                        answer = Some(false);
                    }
                });
            });
        match answer {
            Some(true) => {
                self.addons.close();
                self.closing = true;
                ctx.request_repaint();
            }
            Some(false) => self.close_questions = None,
            None => {}
        }
    }

    /// The classic menu bar: File, View, Help.
    fn menu_bar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        egui::TopBottomPanel::top("menu")
            .frame(egui::Frame::new().fill(self.theme().page).inner_margin(egui::Margin::symmetric(4, 2)))
            .show(ctx, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.add(egui::Button::new("Run new task").shortcut_text(ui.ctx().format_shortcut(&RUN_NEW_TASK))).clicked() {
                            self.run_dialog.get_or_insert_with(String::new);
                            ui.close();
                        }
                        if ui.button(Core::Settings.name()).clicked() {
                            self.page = Page::Core(Core::Settings);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Exit").clicked() {
                            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                    ui.menu_button("View", |ui| {
                        if ui.button("Refresh now").clicked() {
                            self.refresh_now();
                            ui.close();
                        }
                        ui.menu_button("Update speed", |ui| {
                            for (i, (label, _)) in SPEEDS.iter().enumerate() {
                                let mark = if self.settings.speed == i { "\u{2022} " } else { "   " };
                                if ui.button(format!("{mark}{label}")).clicked() {
                                    self.set_speed(i);
                                    ui.close();
                                }
                            }
                        });
                        let mut top = self.settings.always_on_top;
                        if ui.checkbox(&mut top, "Always on top").changed() {
                            self.settings.always_on_top = top;
                            self.settings.save();
                        }
                        let mut dark = self.settings.dark;
                        if ui.checkbox(&mut dark, "Dark mode").changed() {
                            self.settings.set_dark(dark);
                            self.settings.save();
                        }
                    });
                    if self.page == Page::Core(Core::Performance) {
                        self.performance_menu(ui);
                    }
                    // An add-on's own menu, when it belongs in the menu bar.
                    if let Page::Addon(id) = self.page {
                        let mut page = PageContext::new(&self.settings, &self.snap);
                        self.addons.menu_bar_menu(id, ui, &mut page);
                        actions.append(&mut page.actions);
                    }
                    ui.menu_button("Help", |ui| {
                        ui.label("Ultimate PC Manager");
                        ui.label(RichText::new("Your own task manager, built from scratch.").color(self.sub_text()));
                    });
                });
            });
    }

    fn nav_panel(&mut self, ctx: &egui::Context) {
        let theme = self.theme();
        let fill = theme.sidebar;
        let line = theme.line;
        egui::SidePanel::left("nav")
            .exact_width(210.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(fill).inner_margin(egui::Margin { left: 14, right: 14, top: 10, bottom: 8 }).stroke(Stroke::new(1.0, line)))
            .show(ctx, |ui| {
                // Bottom first: Settings | Colors, and the logo above them.
                egui::TopBottomPanel::bottom("nav_bottom")
                    .frame(egui::Frame::new().fill(fill))
                    .show_inside(ui, |ui| {
                        logo(ui);
                        ui.add_space(14.0);
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 12.0) / 2.0;
                            for p in [Page::Core(Core::Settings), Page::Core(Core::Colors)] {
                                let label = page_name(&self.addons, p);
                                let (r, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
                                let ink = nav_entry_back(ui, r, self.page == p, resp.hovered());
                                self.page_icon(ui.painter(), Pos2::new(r.left() + 12.0, r.center().y), p, ink);
                                ui.painter().text(Pos2::new(r.left() + 26.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(13.0), ink);
                                if resp.clicked() {
                                    self.page = p;
                                }
                                if p == Page::Core(Core::Settings) {
                                    let x = r.right() + 6.0;
                                    ui.painter().line_segment([Pos2::new(x, r.top() + 4.0), Pos2::new(x, r.bottom() - 4.0)], Stroke::new(1.0, line));
                                    ui.add_space(4.0);
                                }
                            }
                        });
                    });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    // The core first, then an "Add-ons" heading and the add-ons that are on.
                    for item in nav_items(&self.addons) {
                        if item.heading {
                            nav_heading(ui, "Add-ons");
                        }
                        self.nav_item(ui, item.page, item.name);
                    }
                });
            });
    }

    fn nav_item(&mut self, ui: &mut Ui, p: Page, name: &str) {
        let selected = self.page == p;
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 41.6), Sense::click());
        let ink = nav_entry_back(ui, rect.shrink2(vec2(0.0, 4.0)), selected, resp.hovered());
        let painter = ui.painter();
        let c = Pos2::new(rect.left() + 12.0, rect.center().y);
        self.page_icon(painter, c, p, ink);
        painter.text(
            Pos2::new(rect.left() + 35.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            name,
            egui::FontId::proportional(13.0),
            ink,
        );
        let resp = resp.on_hover_text(name);
        if resp.clicked() {
            self.page = p;
            match p {
                Page::Core(Core::Services) if self.services.is_none() => self.reload_services(),
                Page::Core(Core::Startup) if self.startup.is_none() => self.reload_startup(),
                _ => {}
            }
        }
    }

    /// The status line: health dot, process count, IP address and
    /// sample counter on the left; on the right, the area where the add-ons
    /// that are on draw their controls.
    fn status_bar(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        let s = &self.snap;
        let paused = self.settings.interval_ms() == 0;
        let providers = ProviderState::of(s);
        let msg = self
            .message
            .as_ref()
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(6))
            .map(|(m, _)| m.clone());
        // Settings › Add-ons keeps its status line here while it is open.
        let note = (self.page == Page::Core(Core::Settings) && self.show_addons)
            .then(|| crate::addons::overview::Overview::of(&self.addons, None).status());
        let theme = self.theme();
        let [state, procs, ip, generation] = status_parts(paused, providers, s.procs.len(), &local_ip(), s.seq);
        let mut page = PageContext::new(&self.settings, &self.snap);
        let addons = &mut self.addons;
        egui::TopBottomPanel::bottom("status")
            .exact_height(44.0)
            .frame(egui::Frame::new().fill(theme.panel).inner_margin(egui::Margin::symmetric(16, 0)).stroke(Stroke::new(1.0, theme.line)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    ui.painter().circle_filled(r.center(), 3.5, health_colour(&theme, paused, providers));
                    let words = |text: String, color: Color32| RichText::new(text).font(theme.fonts.label.clone()).color(color);
                    let dot = " \u{00b7} ";
                    let gap = ui.spacing().item_spacing.x;
                    ui.add_space(gap);
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(words(format!("{state}{dot}{procs}{dot}"), theme.label));
                    ui.label(words(ip, theme.link));
                    ui.label(words(format!("{dot}{generation}"), theme.label));
                    if let Some(m) = msg {
                        ui.label(words(format!("{dot}{m}"), theme.link));
                    } else if let Some(n) = note {
                        ui.label(words(format!("{dot}{n}"), theme.label));
                    }
                    // The areas of the add-ons that are on, right to left.
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        addons.status_bar_areas(ui, &mut page);
                    });
                });
            });
        actions.append(&mut page.actions);
    }

    /// A page's icon: the core's from the shared set, an add-on's its own.
    /// The page's icon from `assets/icons`; a page without one (a test
    /// add-on) gets a plain square outline in `ink`.
    pub fn page_icon(&self, painter: &egui::Painter, center: Pos2, page: Page, ink: Color32) {
        if !crate::widgets::icons::draw(painter, center, page.id()) {
            crate::widgets::plain_icon(painter, center, ink);
        }
    }

    /// The page title: an outline icon and a large, widely spaced heading.
    pub fn page_title(&self, ui: &mut Ui, title: &str) {
        let theme = self.theme();
        let (r, _) = ui.allocate_exact_size(vec2(26.0, 40.0), Sense::hover());
        self.page_icon(ui.painter(), r.center(), self.page, theme.label);
        ui.label(RichText::new(title).font(theme.fonts.page_title.clone()).color(theme.text));
    }

    /// The page of a migrated add-on: the app's header, then the add-on
    /// draws the rest through the host.
    fn addon_page(&mut self, ui: &mut Ui, id: &str, actions: &mut Vec<Action>) {
        // The add-on's own menu, if it has one, sits in the header.
        let subtitle = self.addons.subtitle(id);
        self.header_with(ui, page_name(&self.addons, self.page), subtitle, |app, ui| {
            let mut page = PageContext::new(&app.settings, &app.snap);
            app.addons.header_menu(id, ui, &mut page);
            app.addons.header_status(id, ui, &mut page);
            actions.append(&mut page.actions);
        });
        let mut page = PageContext::new(&self.settings, &self.snap);
        self.addons.ui(id, ui, &mut page);
        actions.append(&mut page.actions);
    }

    pub fn header(&mut self, ui: &mut Ui, title: &str) {
        self.header_with(ui, title, None, |_, _| {});
    }

    /// The header: the title, then `subtitle` next to it, with `extra`
    /// drawn right to left after Refresh and "+ Run new task" (an add-on's
    /// menu and status).
    fn header_with(&mut self, ui: &mut Ui, title: &str, subtitle: Option<&str>, mut extra: impl FnMut(&mut Self, &mut Ui)) {
        let (mut refresh, mut run) = (false, false);
        ui.horizontal(|ui| {
            self.page_title(ui, title);
            if let Some(sub) = subtitle {
                ui.label(RichText::new(sub).color(ui.visuals().weak_text_color()));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                refresh = refresh_button(ui).clicked();
                run = crate::widgets::run_task_button(ui).clicked();
                extra(self, ui);
            });
        });
        if refresh {
            self.refresh_now();
        }
        if run {
            self.run_dialog = Some(String::new());
        }
        ui.add_space(10.0);
    }

    /// The common question before turning off an add-on that holds
    /// unsaved work (see [`AddonHost::off_question`]): Save, Discard or
    /// Cancel; the answer is saved when the add-on turned off.
    fn off_question(&mut self, ctx: &egui::Context) {
        let Some(q) = self.addons.off_question() else { return };
        if let Some(answer) = Self::ask_unsaved(ctx, &format!("Turn off {}?", q.name), q.question) {
            if self.addons.answer_off(answer) {
                self.save_addons();
            }
        }
    }

    /// Closing the window (X, File › Exit, Alt+F4) while an add-on holds
    /// unsaved work: the close is held back and the same question is asked
    /// (see [`AddonHost::request_close`]); the window closes once every
    /// such add-on saved or discarded its work.
    fn close_question(&mut self, ctx: &egui::Context) {
        if ctx.input(|i| i.viewport().close_requested()) && !self.addons.request_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let Some(q) = self.addons.close_question() else { return };
        let title = format!("{} has unsaved work", q.name);
        if let Some(answer) = Self::ask_unsaved(ctx, &title, q.question) {
            if self.addons.answer_close(answer) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// The common Save / Discard / Cancel modal; the button clicked.
    fn ask_unsaved(ctx: &egui::Context, title: &str, question: &str) -> Option<crate::addons::OffAnswer> {
        let mut answer = None;
        egui::Modal::new(egui::Id::new("addon-off-question")).show(ctx, |ui| {
            ui.set_max_width(400.0);
            ui.label(RichText::new(title).strong());
            ui.add_space(4.0);
            ui.label(question);
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                use crate::widgets::parts::{outline_button, primary_button};
                for (choice, word) in crate::addons::OFF_ANSWERS.iter().rev() {
                    // Save is the main action; Discard and Cancel are outlined.
                    let button = if *choice == crate::addons::OffAnswer::Save { primary_button(ui, word, true) } else { outline_button(ui, word, true) };
                    if button.clicked() {
                        answer = Some(*choice);
                    }
                }
            });
        });
        answer
    }

    fn dialogs(&mut self, ctx: &egui::Context, actions: &mut Vec<Action>) {
        self.finished_dumps();
        self.finished_restarts();
        if self.dump_question.is_some() {
            let (mut yes, mut no) = (false, false);
            egui::Modal::new(egui::Id::new("dump-question")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(DUMP_QUESTION);
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    use crate::widgets::parts::{outline_button, primary_button};
                    yes = primary_button(ui, "Yes", true).clicked();
                    no = outline_button(ui, "No", true).clicked();
                });
            });
            if yes || no {
                self.answer_dump(yes, actions);
            }
        }
        if let Some(mut text) = self.run_dialog.take() {
            let mut open = true;
            let mut done = false;
            egui::Window::new("Run new task")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label("Type the name of a program, folder, document or Internet resource, and it will open for you.");
                    let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(380.0).hint_text("notepad"));
                    let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if !r.has_focus() && !enter {
                        r.request_focus();
                    }
                    ui.horizontal(|ui| {
                        if ui.button("OK").clicked() || enter {
                            done = true;
                        }
                        if ui.button("Cancel").clicked() {
                            open = false;
                        }
                    });
                });
            if done {
                match self.actions.run_new_task(&text) {
                    Ok(()) => self.say(format!("Started \"{}\"", text.trim())),
                    Err(e) => self.say(e),
                }
            } else if open {
                self.run_dialog = Some(text);
            }
        }
        if self.ask_show_microsoft.is_some() {
            let (mut turn_off, mut cancel) = (false, false);
            egui::Modal::new(egui::Id::new("show-microsoft-question")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(SHOW_MICROSOFT_QUESTION);
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    use crate::widgets::parts::{outline_button, primary_button};
                    turn_off = primary_button(ui, "Turn off", true).clicked();
                    cancel = outline_button(ui, "Cancel", true).clicked();
                });
            });
            if turn_off {
                self.show_microsoft_and_go();
                self.settings.save();
            } else if cancel {
                self.ask_show_microsoft = None;
            }
        }
        if let Some(ask) = self.confirm_end.clone() {
            let mut close = false;
            let sub = self.sub_text();
            egui::Window::new(ask.title())
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
                .show(ctx, |ui| {
                    ui.label(ask.question());
                    if !ask.note().is_empty() {
                        ui.label(RichText::new(ask.note()).color(sub));
                    }
                    ui.horizontal(|ui| {
                        if ui.button(ask.title()).clicked() {
                            actions.push(ask.action());
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                });
            if close {
                self.confirm_end = None;
            }
        }
        if let Some(mut ask) = self.confirm_efficiency.clone() {
            let (mut go, mut cancel) = (false, false);
            egui::Modal::new(egui::Id::new("efficiency-question")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(crate::pages::EFFICIENCY_QUESTION);
                ui.add_space(6.0);
                ui.checkbox(&mut ask.dont_ask, "Don't ask again");
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    use crate::widgets::parts::{outline_button, primary_button};
                    go = primary_button(ui, ask.title(), true).clicked();
                    cancel = outline_button(ui, "Cancel", true).clicked();
                });
            });
            self.confirm_efficiency = if go || cancel { None } else { Some(ask.clone()) };
            if go {
                self.answer_efficiency(&ask, actions);
                if ask.dont_ask {
                    self.settings.save();
                }
            }
        }
        if let Some(pids) = self.confirm_realtime.clone() {
            let (mut go, mut cancel) = (false, false);
            egui::Modal::new(egui::Id::new("realtime-question")).show(ctx, |ui| {
                ui.set_max_width(420.0);
                ui.label(crate::pages::REALTIME_QUESTION);
                ui.add_space(10.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    use crate::widgets::parts::{outline_button, primary_button};
                    go = primary_button(ui, "Continue", true).clicked();
                    cancel = outline_button(ui, "Cancel", true).clicked();
                });
            });
            if go || cancel {
                self.confirm_realtime = None;
            }
            if go {
                Self::answer_realtime(&pids, actions);
            }
        }
        // The Process Inspectors; a resize is saved with the Processes
        // table's layout.
        let procs = &self.snap.procs;
        let width = &mut self.settings.process_table.inspector_width;
        self.inspectors.retain_mut(|inspector| inspector.show(ctx, procs, width, actions));
        self.wait_chains.retain_mut(|w| w.show(ctx));
        // The File activity windows; closing one stops its trace.
        let width = &mut self.settings.process_table.file_activity_width;
        self.file_activity.retain_mut(|window| window.show(ctx, procs, width, actions));
    }

    /// Closes every File activity window, stopping their traces (on exit).
    pub fn close_file_activity(&mut self) {
        self.file_activity.clear();
    }

    /// Efficiency mode agreed to: it runs, and "Don't ask again" turns
    /// "Ask before applying Efficiency mode" off (the dialog saves it).
    pub fn answer_efficiency(&mut self, ask: &crate::pages::EfficiencyAsk, actions: &mut Vec<Action>) {
        if ask.dont_ask {
            self.settings.ask_efficiency = false;
        }
        actions.push(Action::Efficiency(ask.pids.clone()));
    }

    /// Realtime priority agreed to: it is set on each of `pids`.
    pub fn answer_realtime(pids: &[u32], actions: &mut Vec<Action>) {
        actions.extend(pids.iter().map(|&pid| Action::Priority(pid, "Realtime")));
    }

    /// A memory dump is done: once written, [`DUMP_QUESTION`] is asked;
    /// a refusal is said in the status bar.
    pub fn dump_done(&mut self, result: Result<std::path::PathBuf, String>) {
        match result {
            Ok(path) => self.dump_question = Some(path),
            Err(e) => self.say(e),
        }
    }

    /// The answer to [`DUMP_QUESTION`]: Yes shows the file in File
    /// Explorer.
    pub fn answer_dump(&mut self, show: bool, actions: &mut Vec<Action>) {
        if let Some(path) = self.dump_question.take() {
            if show {
                actions.push(Action::ShowInExplorer(path));
            }
        }
    }

    /// Takes the memory dumps whose writing has ended ([`Self::dump_done`]).
    fn finished_dumps(&mut self) {
        let mut done = Vec::new();
        self.dumps.retain(|rx| match rx.try_recv() {
            Ok(result) => {
                done.push(result);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                done.push(Err(winsys::DUMP_FAILED.to_string()));
                false
            }
        });
        for result in done {
            self.dump_done(result);
        }
    }

    /// Restart on the shell's Windows Explorer: the system call (which waits
    /// up to several seconds) runs on a worker thread; the status bar says
    /// how it went on a later frame ([`Self::finished_restarts`]).
    fn restart_explorer(&mut self, ctx: &egui::Context, pid: u32) {
        let (tx, rx) = std::sync::mpsc::channel();
        let ctx = ctx.clone();
        let system = self.actions.clone();
        let spawned = std::thread::Builder::new().name("restart-explorer".into()).spawn(move || {
            let _ = tx.send(system.restart_explorer(pid));
            ctx.request_repaint();
        });
        match spawned {
            Ok(_) => self.explorer_restarts.push(rx),
            Err(_) => self.say(crate::platform::EXPLORER_NOT_RESTARTED.to_string()),
        }
    }

    /// Takes the restarts of Windows Explorer that have ended and says
    /// how each went.
    fn finished_restarts(&mut self) {
        let mut done = Vec::new();
        self.explorer_restarts.retain(|rx| match rx.try_recv() {
            Ok(result) => {
                done.push(result);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                done.push(Err(crate::platform::EXPLORER_NOT_RESTARTED.to_string()));
                false
            }
        });
        for result in done {
            match result {
                Ok(()) => {
                    self.say("Restarted Windows Explorer".to_string());
                    self.sampler.refresh_requested.store(true, Ordering::Relaxed);
                }
                Err(e) => self.say(e),
            }
        }
    }

    /// Runs `call` on each of `pids` and says how it went: "<done> <name>"
    /// for one, "<done> N processes" for several, or the first failure.
    fn each_process(&mut self, pids: &[u32], done: &str, call: impl Fn(u32) -> Result<(), String>) {
        let failed: Vec<String> = pids.iter().filter_map(|&pid| call(pid).err()).collect();
        match failed.into_iter().next() {
            Some(e) => self.say(e),
            None => match pids {
                [pid] => {
                    let name = self.snap.procs.iter().find(|p| p.pid == *pid).map_or_else(|| pid.to_string(), |p| p.name.clone());
                    self.say(format!("{done} {name}"))
                }
                _ => self.say(format!("{done} {} processes", pids.len())),
            },
        }
        self.sampler.refresh_requested.store(true, Ordering::Relaxed);
    }

    /// "Turn it off": Hide Microsoft Services and Processes goes off and
    /// Processes shows the process asked for.
    pub fn show_microsoft_and_go(&mut self) {
        if let Some(pid) = self.ask_show_microsoft.take() {
            self.settings.hide_microsoft = false;
            self.go_to_process(pid);
        }
    }

    /// Follow process on `pid`, or Stop following (`None`).
    pub fn follow(&mut self, pid: Option<u32>) {
        self.following = pid.map(crate::process_view::Follow::new);
        if let Some(pid) = pid {
            self.selected = crate::process_view::Selection::only(pid);
        }
    }

    /// Go to service(s): opens Services filtered to the PID of the host
    /// `pid`, with `service` or the host's first service selected.
    fn go_to_services(&mut self, pid: u32, service: Option<String>) {
        let first = || self.snap.services.get(&pid).and_then(|v| v.first()).map(|s| s.name.clone());
        self.svc_selected = service.or_else(first);
        self.search = pid.to_string();
        self.reload_services();
        self.page = Page::Core(Core::Services);
    }

    /// Opens Processes on `pid`: clears the search so the process is
    /// listed, selects it and scrolls to it.
    fn go_to_process(&mut self, pid: u32) {
        self.search.clear();
        self.selected = crate::process_view::Selection::only(pid);
        self.reveal_selected = true;
        self.page = Page::Core(Core::Processes);
    }

    /// The processes Settings keeps out of the Processes table now.
    pub fn hide(&self) -> crate::process_view::Hide {
        crate::process_view::Hide::new(&self.settings, std::process::id(), self.signatures.microsoft())
    }

    /// Asks the signature worker about the program files of the latest
    /// snapshot (only while Hide Microsoft Services and Processes needs them).
    pub fn ask_signatures(&self) {
        if self.settings.hide_microsoft {
            self.signatures.ask(self.snap.procs.iter().map(|p| p.exe.as_str()));
        }
    }

    fn run_action(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::End { pid, force, tree, name } => match self.actions.end_process(pid, force, tree) {
                Ok(()) => {
                    self.say(format!("Ended {name}"));
                    self.sampler.refresh_requested.store(true, Ordering::Relaxed);
                }
                Err(e) => self.say(e),
            },
            Action::EndMany { pids, force, tree } => {
                // Every one is tried; the first failure is said.
                let system = self.actions.clone();
                let failed: Vec<String> = pids.iter().filter_map(|&pid| system.end_process(pid, force, tree).err()).collect();
                match failed.into_iter().next() {
                    None => self.say(format!("Ended {} processes", pids.len())),
                    Some(e) => self.say(e),
                }
                self.sampler.refresh_requested.store(true, Ordering::Relaxed);
            }
            Action::Priority(pid, p) => match winsys::set_priority(pid, p) {
                Ok(()) => self.say(format!("Priority set to {p}")),
                Err(e) => self.say(e),
            },
            Action::OpenLocation(path) => {
                if let Err(e) = self.actions.open_file_location(&path) {
                    self.say(e);
                }
            }
            Action::SearchOnline(name) => {
                let q: String = name
                    .bytes()
                    .map(|c| {
                        if c.is_ascii_alphanumeric() || c == b'.' || c == b'-' {
                            (c as char).to_string()
                        } else {
                            format!("%{c:02X}")
                        }
                    })
                    .collect();
                ctx.open_url(egui::OpenUrl::new_tab(format!("https://www.bing.com/search?q={q}")));
            }
            Action::Say(text) => self.say(text),
            Action::ShowInExplorer(path) => {
                if self.actions.reveal_in_explorer(&path).is_err() {
                    self.say(crate::platform::REVEAL_FAILED.to_string());
                }
            }
            Action::ShowIn { addon, path } => {
                if self.addons.reveal(addon, &path) {
                    self.page = Page::of(addon);
                }
            }
            Action::ShowProcessIn { addon, pid } => {
                if self.addons.reveal_process(addon, pid) {
                    self.page = Page::of(addon);
                }
            }
            Action::OpenPage(addon) => {
                if self.addons.is_enabled(addon) {
                    self.page = Page::of(addon);
                }
            }
            Action::RecordSensors(on) => {
                // Another add-on's page asked; the host reaches the add-on
                // that records sensors, whose own Stop may queue actions.
                let mut page = PageContext::new(&self.settings, &self.snap);
                let result = self.addons.record_sensors(on, &mut page);
                let queued = page.actions;
                if let Err(why) = result {
                    self.say(why.to_string());
                }
                for a in queued {
                    self.run_action(ctx, a);
                }
            }
            Action::GoToProcess(pid) => {
                // A process Hide Microsoft Services and Processes keeps out
                // of the table: ask whether to turn the option off first.
                let hidden = self.snap.procs.iter().find(|p| p.pid == pid).and_then(|p| self.hide().why(p));
                if hidden == Some(crate::process_view::Hidden::Microsoft) {
                    self.ask_show_microsoft = Some(pid);
                } else {
                    self.go_to_process(pid);
                }
            }
            Action::UpdateSpeed(i) => self.set_speed(i),
            Action::ExportCsv { file_name, text } => {
                let Some(path) = crate::platform::pick_csv(file_name) else { return };
                // With a byte order mark, as the other exports: Excel then
                // reads the names right.
                match std::fs::write(&path, format!("\u{feff}{text}")) {
                    Ok(()) => self.say(format!("Saved {path}")),
                    Err(e) => self.say(format!("Could not save: {e}")),
                }
            }
            Action::Inspect(pid) => {
                // One window per process: inspecting it again brings its window up.
                if self.inspectors.iter().any(|i| i.pid == pid) {
                    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, crate::inspector::window_id(pid)));
                } else {
                    self.inspectors.push(crate::inspector::Inspector::open(pid, &self.snap.procs));
                }
            }
            Action::AnalyzeWaitChain(pid) => {
                // One window per process: analysing it again runs it again
                // and brings its window up.
                if let Some(w) = self.wait_chains.iter_mut().find(|w| w.pid == pid) {
                    w.rerun(&self.snap.procs);
                    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, crate::wait_chain::window_id(pid)));
                } else {
                    self.wait_chains.push(crate::wait_chain::WaitChain::open(pid, &self.snap.procs));
                }
            }
            Action::FileActivity(pid) => {
                // One window per process; the administrator check runs on
                // the click (the menu item is never greyed).
                if self.file_activity.iter().any(|w| w.pid == pid) {
                    ctx.move_to_top(egui::LayerId::new(egui::Order::Middle, crate::file_activity::window_id(pid)));
                } else {
                    let admin = self.actions.is_admin();
                    self.file_activity.push(crate::file_activity::FileActivity::open(pid, &self.snap.procs, admin));
                }
            }
            Action::RestartAsAdministrator => match self.actions.restart_elevated(Core::Processes.id()) {
                // The elevated copy takes over; this one closes.
                Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Err(e) => self.say(e),
            },
            Action::ResourceValues { percents } => {
                self.settings.process_percents = percents;
                self.settings.save();
            }
            Action::Suspend(pids) => {
                let system = self.actions.clone();
                self.each_process(&pids, "Suspended", |pid| system.suspend_process(pid))
            }
            Action::Resume(pids) => {
                let system = self.actions.clone();
                self.each_process(&pids, "Resumed", |pid| system.resume_process(pid))
            }
            Action::Efficiency(pids) => {
                let system = self.actions.clone();
                self.each_process(&pids, "Efficiency mode on for", |pid| system.set_efficiency(pid))
            }
            Action::Affinity(pids, mask) => {
                let system = self.actions.clone();
                self.each_process(&pids, "Processor affinity set for", |pid| system.set_affinity(pid, mask))
            }
            Action::ProcessView(item) => self.apply_process_view(item),
            Action::Dump { pid, name } => {
                // A full dump can take seconds: written off the UI thread,
                // taken by `finished_dumps`.
                let (tx, rx) = std::sync::mpsc::channel();
                let ctx = ctx.clone();
                let system = self.actions.clone();
                let spawned = std::thread::Builder::new().name("memory-dump".into()).spawn(move || {
                    let _ = tx.send(system.write_dump(pid, &name, &std::env::temp_dir()));
                    ctx.request_repaint();
                });
                match spawned {
                    Ok(_) => self.dumps.push(rx),
                    Err(_) => self.say(winsys::DUMP_FAILED),
                }
            }
            Action::UacVirtualization(pids) => {
                // Every one is tried; the first refusal is said.
                if let Some(e) = pids.iter().find_map(|&pid| self.actions.toggle_uac_virtualization(pid).err()) {
                    self.say(e);
                }
            }
            Action::BringToFront(pid) => {
                if let Err(e) = self.actions.bring_to_front(pid) {
                    self.say(e);
                }
            }
            Action::Follow(pid) => self.follow(pid),
            Action::RestartExplorer(pid) => self.restart_explorer(ctx, pid),
            Action::GoToServices { pid, service } => self.go_to_services(pid, service),
            Action::OpenPowerOptions => {
                if let Err(e) = self.actions.open_power_options() {
                    self.say(e);
                }
            }
            Action::Copy(text) => {
                ctx.copy_text(text.clone());
                self.say(format!("Copied \"{text}\""));
            }
            Action::Service(name, what) => {
                match self.system.control_service(&name, what) {
                    Ok(()) => {
                        let verb = match what {
                            "start" => "Started",
                            "stop" => "Stopped",
                            _ => "Restarted",
                        };
                        self.say(format!("{verb} {name}"))
                    }
                    Err(e) => self.say(e),
                }
                self.reload_services();
            }
            Action::Startup(id, enable) => {
                // The entry is named by its id, so a reread since the click
                // cannot change the target; a name is only for the message.
                let name = self.startup.as_ref().and_then(|l| l.iter().find(|r| r.id == id)).map(|r| r.name.clone());
                match name {
                    Some(name) => match self.system.set_startup_enabled(&id, enable) {
                        Ok(()) => self.say(format!("{name} {}", if enable { "enabled" } else { "disabled" })),
                        Err(e) => self.say(e),
                    },
                    None => self.say(crate::system_lists::STARTUP_GONE),
                }
                self.reload_startup();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::AddonHost;

    use crate::addons::fake::test_registry;
    use serde_json::json;

    #[test]
    fn the_status_line_reads_like_the_design_a_anatomy() {
        let words = |paused, providers| status_parts(paused, providers, 389, "100.90.15.31", 15).join(" · ");
        assert_eq!(words(false, ProviderState::Healthy), "Native providers healthy · 389 processes · IP 100.90.15.31 · Generation 15");
        assert_eq!(words(true, ProviderState::Healthy), "Updates paused · 389 processes · IP 100.90.15.31 · Generation 15");
        assert_eq!(words(false, ProviderState::Waiting), "Starting\u{2026} · 389 processes · IP 100.90.15.31 · Generation 15");
    }

    #[test]
    fn the_status_line_says_when_the_counters_are_degraded_or_unavailable() {
        let state = |providers| status_parts(false, providers, 389, "100.90.15.31", 15)[0].clone();
        assert_eq!(state(ProviderState::Degraded), "Native providers degraded");
        assert_eq!(state(ProviderState::Unavailable), "Native providers unavailable");
        assert_eq!(state(ProviderState::Healthy), "Native providers healthy");
    }

    /// One frame with `key` pressed (with `modifiers`), run through the
    /// app's keyboard shortcuts.
    fn press(app: &mut TaskApp, key: egui::Key, modifiers: egui::Modifiers) {
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let down = egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        crate::test_ui::run(&ctx, (400.0, 300.0), vec![down], |ui| app.shortcuts(ui.ctx()));
    }

    #[test]
    fn ctrl_n_opens_run_new_task() {
        let mut app = TaskApp::headless(Settings::default(), Snapshot::default());
        press(&mut app, egui::Key::N, egui::Modifiers::NONE);
        assert_eq!(app.run_dialog, None, "N alone opens nothing");
        press(&mut app, egui::Key::N, egui::Modifiers::CTRL | egui::Modifiers::COMMAND);
        assert_eq!(app.run_dialog.as_deref(), Some(""), "Ctrl+N opens an empty Run new task");
    }

    #[test]
    fn f5_still_asks_for_a_new_snapshot() {
        let mut app = TaskApp::headless(Settings::default(), Snapshot::default());
        press(&mut app, egui::Key::F5, egui::Modifiers::NONE);
        assert!(app.sampler.refresh_requested.load(Ordering::Relaxed));
        assert_eq!(app.run_dialog, None);
    }

    #[test]
    fn f5_held_down_refreshes_once_per_press() {
        let mut app = TaskApp::headless(Settings::default(), Snapshot::default());
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let f5 = || egui::Event::Key { key: egui::Key::F5, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        crate::test_ui::run(&ctx, (400.0, 300.0), vec![f5()], |ui| app.shortcuts(ui.ctx()));
        assert!(app.sampler.refresh_requested.swap(false, Ordering::Relaxed), "the press refreshes");
        // Held down: the key repeats without a release in between.
        crate::test_ui::run(&ctx, (400.0, 300.0), vec![f5(), f5()], |ui| app.shortcuts(ui.ctx()));
        assert!(!app.sampler.refresh_requested.load(Ordering::Relaxed), "key repeats refresh nothing more");
        let up = egui::Event::Key { key: egui::Key::F5, physical_key: None, pressed: false, repeat: false, modifiers: egui::Modifiers::NONE };
        crate::test_ui::run(&ctx, (400.0, 300.0), vec![up, f5()], |ui| app.shortcuts(ui.ctx()));
        assert!(app.sampler.refresh_requested.load(Ordering::Relaxed), "the next press refreshes again");
    }

    #[test]
    fn ctrl_n_keeps_the_text_of_an_open_run_new_task() {
        let mut app = TaskApp::headless(Settings::default(), Snapshot::default());
        app.run_dialog = Some("notep".into());
        press(&mut app, egui::Key::N, egui::Modifiers::CTRL | egui::Modifiers::COMMAND);
        assert_eq!(app.run_dialog.as_deref(), Some("notep"));
    }

    #[test]
    fn inspect_opens_one_inspector_window_per_process() {
        let me = crate::sampler::ProcRow { pid: std::process::id(), name: "tests.exe".into(), ..Default::default() };
        let mut app = TaskApp::headless(Settings::default(), Snapshot { procs: vec![me], ..Default::default() });
        let ctx = egui::Context::default();
        app.run_action(&ctx, Action::Inspect(std::process::id()));
        app.run_action(&ctx, Action::Inspect(std::process::id()));
        assert_eq!(app.inspectors.iter().map(|i| i.pid).collect::<Vec<_>>(), [std::process::id()], "inspecting it again opens no second window");
    }

    #[test]
    fn analyze_wait_chain_opens_one_window_per_process() {
        let me = crate::sampler::ProcRow { pid: std::process::id(), name: "tests.exe".into(), ..Default::default() };
        let mut app = TaskApp::headless(Settings::default(), Snapshot { procs: vec![me], ..Default::default() });
        let ctx = egui::Context::default();
        app.run_action(&ctx, Action::AnalyzeWaitChain(std::process::id()));
        app.run_action(&ctx, Action::AnalyzeWaitChain(std::process::id()));
        assert_eq!(app.wait_chains.iter().map(|w| w.pid).collect::<Vec<_>>(), [std::process::id()], "analysing it again opens no second window");
    }

    #[test]
    fn file_activity_opens_one_window_per_process_and_starts_no_trace_without_administrator_rights() {
        let me = crate::sampler::ProcRow { pid: std::process::id(), name: "tests.exe".into(), ..Default::default() };
        let mut app = TaskApp::headless(Settings::default(), Snapshot { procs: vec![me], ..Default::default() });
        let ctx = egui::Context::default();
        app.run_action(&ctx, Action::FileActivity(std::process::id()));
        app.run_action(&ctx, Action::FileActivity(std::process::id()));
        assert_eq!(app.file_activity.iter().map(|w| w.pid).collect::<Vec<_>>(), [std::process::id()], "asking again opens no second window");
        if !crate::platform::is_admin() {
            assert!(!app.file_activity[0].tracing(), "the admin check runs on click; not elevated, no trace");
        }
        // Closing the app stops every trace with its window.
        app.close_file_activity();
        assert!(app.file_activity.is_empty());
    }

    // ---- Process actions through the system-actions seam ----

    const A: u32 = 4_000_000_001;
    const B: u32 = 4_000_000_002;
    const C: u32 = 4_000_000_003;

    /// A headless app listing alpha.exe, beta.exe and gamma.exe (made-up
    /// PIDs no real process has), nothing said and no refresh asked yet.
    fn app_with_three_processes() -> TaskApp {
        use crate::sampler::ProcRow;
        let row = |pid, name: &str| ProcRow { pid, name: name.into(), ..Default::default() };
        let procs = vec![row(A, "alpha.exe"), row(B, "beta.exe"), row(C, "gamma.exe")];
        let app = TaskApp::headless(Settings::default(), Snapshot { procs, ..Default::default() });
        app.sampler.refresh_requested.store(false, Ordering::Relaxed);
        app
    }

    fn said(app: &TaskApp) -> Option<String> {
        app.message.as_ref().map(|(m, _)| m.clone())
    }

    fn refreshed(app: &TaskApp) -> bool {
        app.sampler.refresh_requested.load(Ordering::Relaxed)
    }

    fn end(pid: u32, force: bool, tree: bool, name: &str) -> Action {
        Action::End { pid, force, tree, name: name.into() }
    }

    #[test]
    fn ending_a_process_says_ended_with_its_name_and_refreshes() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, end(A, false, false, "alpha.exe"));
        assert_eq!(said(&app).as_deref(), Some("Ended alpha.exe"));
        assert!(refreshed(&app));
        assert_eq!(app.recording().requests(), [Request::End { pid: A, force: false, tree: false }]);
    }

    #[test]
    fn force_quit_and_end_tree_ask_the_system_for_exactly_that() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, end(A, true, false, "alpha.exe"));
        app.run_action(&ctx, end(B, false, true, "beta.exe"));
        assert_eq!(
            app.recording().requests(),
            [Request::End { pid: A, force: true, tree: false }, Request::End { pid: B, force: false, tree: true }]
        );
    }

    #[test]
    fn a_process_that_cannot_be_ended_says_why_and_does_not_refresh() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.recording().fail(Request::End { pid: A, force: false, tree: false }, "Access is denied.");
        app.run_action(&ctx, end(A, false, false, "alpha.exe"));
        assert_eq!(said(&app).as_deref(), Some("Access is denied."));
        assert!(!refreshed(&app));
    }

    #[test]
    fn ending_many_processes_says_how_many_were_ended_and_refreshes() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, Action::EndMany { pids: vec![A, B, C], force: true, tree: true });
        assert_eq!(said(&app).as_deref(), Some("Ended 3 processes"));
        assert!(refreshed(&app));
        let ended = |pid| Request::End { pid, force: true, tree: true };
        assert_eq!(app.recording().requests(), [ended(A), ended(B), ended(C)]);
    }

    #[test]
    fn ending_many_tries_every_process_and_reports_the_first_failure() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        let ended = |pid| Request::End { pid, force: false, tree: false };
        app.recording().fail(ended(A), "Access is denied.");
        app.recording().fail(ended(B), "The process was not found.");
        app.run_action(&ctx, Action::EndMany { pids: vec![A, B, C], force: false, tree: false });
        assert_eq!(said(&app).as_deref(), Some("Access is denied."), "the first failure, in the order of the pids");
        assert!(refreshed(&app));
        assert_eq!(app.recording().requests(), [ended(A), ended(B), ended(C)], "a failure does not stop the rest");
    }

    /// What every "each process" action says: "<done> <name>" for one
    /// process, "<done> N processes" for several, the first failure when any
    /// failed (every process is tried); the table is refreshed each time.
    fn each_process_action_reports(action: fn(Vec<u32>) -> Action, request: fn(u32) -> crate::system_actions::Request, done: &str) {
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, action(vec![B]));
        assert_eq!(said(&app), Some(format!("{done} beta.exe")), "one process");
        assert!(refreshed(&app));
        app.sampler.refresh_requested.store(false, Ordering::Relaxed);
        app.run_action(&ctx, action(vec![A, B, C]));
        assert_eq!(said(&app), Some(format!("{done} 3 processes")), "several processes");
        assert!(refreshed(&app));
        app.recording().fail(request(A), "Access denied: first.");
        app.recording().fail(request(B), "Access denied: second.");
        let before = app.recording().requests().len();
        app.run_action(&ctx, action(vec![A, B, C]));
        assert_eq!(said(&app).as_deref(), Some("Access denied: first."), "the first failure");
        assert_eq!(app.recording().requests()[before..], [request(A), request(B), request(C)], "every process is tried");
    }

    #[test]
    fn suspend_reports_for_one_process_for_several_or_the_first_failure() {
        each_process_action_reports(Action::Suspend, crate::system_actions::Request::Suspend, "Suspended");
    }

    #[test]
    fn resume_reports_for_one_process_for_several_or_the_first_failure() {
        each_process_action_reports(Action::Resume, crate::system_actions::Request::Resume, "Resumed");
    }

    #[test]
    fn efficiency_mode_reports_for_one_process_for_several_or_the_first_failure() {
        each_process_action_reports(Action::Efficiency, crate::system_actions::Request::Efficiency, "Efficiency mode on for");
    }

    #[test]
    fn set_affinity_reports_for_one_process_for_several_or_the_first_failure() {
        fn affinity(pids: Vec<u32>) -> Action {
            Action::Affinity(pids, 0b101)
        }
        fn request(pid: u32) -> crate::system_actions::Request {
            crate::system_actions::Request::Affinity(pid, 0b101)
        }
        each_process_action_reports(affinity, request, "Processor affinity set for");
    }


    // ---- Elevation, administrator check and the remaining actions ----

    #[test]
    fn restart_as_administrator_closes_the_app_when_the_elevated_copy_takes_over() {
        use crate::system_actions::Request;
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = app_with_three_processes();
        app.run_action(&ctx, Action::RestartAsAdministrator);
        assert_eq!(app.recording().requests(), [Request::RestartElevated(Core::Processes.id().into())], "asks to reopen on Processes");
        assert!(closes(&ctx), "the elevated copy takes over: this one closes");
        assert_eq!(said(&app), None, "nothing to say when it goes through");
    }

    #[test]
    fn restart_as_administrator_that_is_cancelled_says_why_and_keeps_the_app_open() {
        use crate::system_actions::Request;
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = app_with_three_processes();
        app.recording().fail(Request::RestartElevated(Core::Processes.id().into()), "Administrator rights were not granted.");
        app.run_action(&ctx, Action::RestartAsAdministrator);
        assert_eq!(said(&app).as_deref(), Some("Administrator rights were not granted."));
        assert!(!closes(&ctx), "cancelled: the app stays open");
    }

    /// Whether the app asked its window to close since the last frame.
    fn closes(ctx: &egui::Context) -> bool {
        let out = crate::test_ui::run(ctx, (400.0, 300.0), vec![], |_| {});
        out.viewport_output.values().any(|v| v.commands.iter().any(|c| matches!(c, egui::ViewportCommand::Close)))
    }

    #[test]
    fn file_activity_follows_the_administrator_answer_of_the_seam() {
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.recording().set_admin(false);
        app.run_action(&ctx, Action::FileActivity(A));
        assert!(!app.file_activity[0].tracing(), "the seam says not administrator: no trace, whatever the test run is");
        app.close_file_activity();
    }

    #[test]
    fn uac_virtualization_tries_every_process_and_reports_only_the_first_refusal() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, Action::UacVirtualization(vec![A, B]));
        assert_eq!(said(&app), None, "nothing to say when all go through");
        assert_eq!(app.recording().requests(), [Request::UacVirtualization(A), Request::UacVirtualization(B)]);
        app.recording().fail(Request::UacVirtualization(A), "Access is denied.");
        app.recording().fail(Request::UacVirtualization(B), "Not supported.");
        app.run_action(&ctx, Action::UacVirtualization(vec![A, B, C]));
        assert_eq!(said(&app).as_deref(), Some("Access is denied."), "the first refusal only");
        assert_eq!(app.recording().requests()[2..], [Request::UacVirtualization(A)], "the search stops at the first refusal");
    }

    /// The actions that say nothing unless they fail: asks the seam for
    /// exactly `request`, says nothing when it goes through and its error
    /// when it does not.
    fn error_only_action(action: Action, request: crate::system_actions::Request) {
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        app.run_action(&ctx, action.clone());
        assert_eq!(said(&app), None, "nothing to say when it goes through");
        assert_eq!(app.recording().requests(), [request.clone()]);
        app.recording().fail(request, "It failed: reason.");
        app.run_action(&ctx, action);
        assert_eq!(said(&app).as_deref(), Some("It failed: reason."));
    }

    #[test]
    fn bring_to_front_says_only_its_error() {
        error_only_action(Action::BringToFront(A), crate::system_actions::Request::BringToFront(A));
    }

    #[test]
    fn open_file_location_says_only_its_error() {
        error_only_action(Action::OpenLocation(r"C:\Tools\tool.exe".into()), crate::system_actions::Request::OpenFileLocation(r"C:\Tools\tool.exe".into()));
    }

    #[test]
    fn open_power_options_says_only_its_error() {
        error_only_action(Action::OpenPowerOptions, crate::system_actions::Request::OpenPowerOptions);
    }

    #[test]
    fn show_in_file_explorer_says_that_it_could_not_when_it_fails_and_nothing_otherwise() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        let path = std::path::PathBuf::from(r"C:\Tools\tool.dmp");
        app.run_action(&ctx, Action::ShowInExplorer(path.clone()));
        assert_eq!(said(&app), None);
        assert_eq!(app.recording().requests(), [Request::RevealInExplorer(path.clone())]);
        app.recording().fail(Request::RevealInExplorer(path.clone()), "any");
        app.run_action(&ctx, Action::ShowInExplorer(path));
        assert_eq!(said(&app).as_deref(), Some(crate::platform::REVEAL_FAILED));
    }

    #[test]
    fn run_new_task_starts_the_trimmed_text_and_says_so_or_says_its_error() {
        use crate::system_actions::Request;
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = app_with_three_processes();
        app.run_dialog = Some("  notepad ".into());
        let enter = egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE };
        // One frame to take focus, one with Enter.
        for events in [vec![], vec![enter]] {
            crate::test_ui::run(&ctx, (800.0, 600.0), events, |ui| app.dialogs(ui.ctx(), &mut Vec::new()));
        }
        assert_eq!(app.recording().requests(), [Request::RunNewTask("  notepad ".into())]);
        assert_eq!(said(&app).as_deref(), Some("Started \"notepad\""));
        app.recording().fail(Request::RunNewTask("nope".into()), "Windows cannot find 'nope'.");
        app.run_dialog = Some("nope".into());
        for events in [vec![], vec![egui::Event::Key { key: egui::Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE }]] {
            crate::test_ui::run(&ctx, (800.0, 600.0), events, |ui| app.dialogs(ui.ctx(), &mut Vec::new()));
        }
        assert_eq!(said(&app).as_deref(), Some("Windows cannot find 'nope'."));
    }

    #[cfg(windows)]
    #[test]
    fn a_headless_app_never_acts_on_a_real_process() {
        // A real process that would live for half a minute.
        let mut child = std::process::Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let ctx = egui::Context::default();
        let mut app = app_with_three_processes();
        let pid = child.id();
        for action in [end(pid, true, true, "ping.exe"), Action::EndMany { pids: vec![pid], force: true, tree: true }, Action::Suspend(vec![pid])] {
            app.run_action(&ctx, action);
        }
        let alive = child.try_wait().unwrap().is_none();
        let _ = child.kill();
        let _ = child.wait();
        assert!(alive, "the headless app ended or acted on a real process");
        assert_eq!(app.recording().requests().len(), 3, "the requests were recorded instead");
    }

    /// An app with svchost.exe (Windows's, PID 900) and tool.exe (PID 901),
    /// Hide Microsoft Services and Processes on or off.
    fn with_windows_process(hide_microsoft: bool) -> TaskApp {
        use crate::sampler::ProcRow;
        const SVCHOST: &str = r"C:\Windows\System32\svchost.exe";
        let procs = vec![
            ProcRow { pid: 900, name: "svchost.exe".into(), exe: SVCHOST.into(), ..Default::default() },
            ProcRow { pid: 901, name: "tool.exe".into(), exe: r"C:\Tools\tool.exe".into(), ..Default::default() },
        ];
        let mut app = TaskApp::headless({ let mut s = Settings::default(); s.hide_microsoft = hide_microsoft; s }, Snapshot { procs, ..Default::default() });
        app.signatures = crate::signature::Signatures::known(&[SVCHOST]);
        app
    }

    #[test]
    fn going_to_a_process_hide_microsoft_hides_asks_to_turn_it_off() {
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = with_windows_process(true);
        app.run_action(&ctx, Action::GoToProcess(900));
        assert_eq!(app.page, Page::Core(Core::Summary), "it does not open a table without the process");
        assert_eq!(app.ask_show_microsoft, Some(900));
        // A modal sizes itself on its first frame and shows on the next.
        let mut draw = || crate::test_ui::texts(&crate::test_ui::run(&ctx, (1200.0, 800.0), vec![], |ui| app.dialogs(ui.ctx(), &mut Vec::new())));
        draw();
        let texts = draw();
        assert!(texts.iter().any(|t| t == SHOW_MICROSOFT_QUESTION), "{texts:?}");
        assert_eq!(SHOW_MICROSOFT_QUESTION, "Processes does not list this process while Hide Microsoft Services and Processes is on. Turn it off?");
        // "Turn it off": the option goes off and Processes shows the process.
        app.show_microsoft_and_go();
        assert!(!app.settings.hide_microsoft);
        assert_eq!((app.page, app.selected.single(), app.ask_show_microsoft), (Page::Core(Core::Processes), Some(900), None));
    }

    #[test]
    fn going_to_a_process_that_is_listed_asks_nothing() {
        let ctx = egui::Context::default();
        let mut app = with_windows_process(true);
        app.run_action(&ctx, Action::GoToProcess(901));
        assert_eq!((app.page, app.selected.single(), app.ask_show_microsoft), (Page::Core(Core::Processes), Some(901), None), "not Microsoft's");
        let mut app = with_windows_process(false);
        app.run_action(&ctx, Action::GoToProcess(900));
        assert_eq!((app.page, app.selected.single(), app.ask_show_microsoft), (Page::Core(Core::Processes), Some(900), None), "the option is off");
    }

    /// Frames until the status bar says something (or 10 s pass): the
    /// finished background jobs are taken on each.
    fn frames_until_said(app: &mut TaskApp) -> Option<String> {
        use std::time::{Duration, Instant};
        let until = Instant::now() + Duration::from_secs(10);
        while app.message.is_none() && app.dump_question.is_none() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
            app.finished_restarts();
            app.finished_dumps();
        }
        said(app)
    }

    #[test]
    fn restarting_explorer_runs_off_the_ui_thread_and_says_how_it_went_on_a_later_frame() {
        use crate::system_actions::Request;
        use std::time::{Duration, Instant};
        let ctx = egui::Context::default();
        let mut app = with_windows_process(false);
        app.message = None;
        app.recording().delay(Request::RestartExplorer(4242), Duration::from_millis(500));
        let started = Instant::now();
        app.run_action(&ctx, Action::RestartExplorer(4242));
        assert!(started.elapsed() < Duration::from_millis(250), "the UI thread does not wait: {:?}", started.elapsed());
        assert_eq!(app.message, None, "nothing to say yet");
        assert_eq!(frames_until_said(&mut app).as_deref(), Some("Restarted Windows Explorer"));
        assert_eq!(app.recording().requests(), [Request::RestartExplorer(4242)]);
    }

    #[test]
    fn a_restart_of_explorer_that_could_not_end_or_restart_it_says_why() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        for said in [crate::platform::EXPLORER_NOT_ENDED, crate::platform::EXPLORER_NOT_RESTARTED] {
            let mut app = with_windows_process(false);
            app.message = None;
            app.recording().fail(Request::RestartExplorer(4242), said);
            app.run_action(&ctx, Action::RestartExplorer(4242));
            assert_eq!(frames_until_said(&mut app).as_deref(), Some(said));
        }
    }

    #[test]
    fn a_dump_is_written_off_the_ui_thread_then_asks_to_show_the_file() {
        use crate::system_actions::Request;
        use std::time::{Duration, Instant};
        let ctx = egui::Context::default();
        let mut app = with_windows_process(false);
        app.message = None;
        let dir = std::env::temp_dir();
        let asked = Request::Dump { pid: 4242, name: "Code.exe".into(), dir: dir.clone() };
        app.recording().delay(asked.clone(), Duration::from_millis(500));
        let started = Instant::now();
        app.run_action(&ctx, Action::Dump { pid: 4242, name: "Code.exe".into() });
        assert!(started.elapsed() < Duration::from_millis(250), "the UI thread does not wait: {:?}", started.elapsed());
        assert_eq!(app.dump_question, None, "not written yet");
        frames_until_said(&mut app);
        assert_eq!(app.dump_question, Some(dir.join("Code.exe-4242.dmp")));
        assert_eq!(app.message, None);
        assert_eq!(app.recording().requests(), [asked]);
    }

    #[test]
    fn a_dump_that_fails_says_the_dump_failure_text_and_asks_nothing() {
        use crate::system_actions::Request;
        let ctx = egui::Context::default();
        let mut app = with_windows_process(false);
        app.message = None;
        let asked = Request::Dump { pid: 4242, name: "Code.exe".into(), dir: std::env::temp_dir() };
        app.recording().fail(asked.clone(), winsys::DUMP_FAILED);
        app.run_action(&ctx, Action::Dump { pid: 4242, name: "Code.exe".into() });
        assert_eq!(frames_until_said(&mut app).as_deref(), Some(winsys::DUMP_FAILED));
        assert_eq!(app.dump_question, None);
        assert_eq!(app.recording().requests(), [asked]);
    }

    #[test]
    fn a_written_dump_asks_to_show_it_in_file_explorer() {
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = with_windows_process(false);
        let path = std::path::PathBuf::from(r"C:\Temp\TaskManager-tool-901.dmp");
        app.dump_done(Ok(path.clone()));
        assert_eq!(app.dump_question, Some(path.clone()));
        // A modal sizes itself on its first frame and shows on the next.
        let mut draw = || crate::test_ui::texts(&crate::test_ui::run(&ctx, (1200.0, 800.0), vec![], |ui| app.dialogs(ui.ctx(), &mut Vec::new())));
        draw();
        let texts = draw();
        assert!(texts.iter().any(|t| t == DUMP_QUESTION), "{texts:?}");
        assert_eq!(DUMP_QUESTION, "Show it in File Explorer?");
        // Yes: File Explorer shows the file; No: nothing.
        let mut actions = Vec::new();
        app.answer_dump(true, &mut actions);
        assert_eq!((actions, app.dump_question.clone()), (vec![Action::ShowInExplorer(path.clone())], None));
        app.dump_done(Ok(path));
        let mut actions = Vec::new();
        app.answer_dump(false, &mut actions);
        assert_eq!((actions, app.dump_question.clone()), (vec![], None));
        // A dump Windows refused: the status bar says why, nothing is asked.
        app.dump_done(Err("Windows could not write the memory dump file.".into()));
        assert_eq!(app.message.as_ref().map(|(m, _)| m.as_str()), Some("Windows could not write the memory dump file."));
        assert_eq!(app.dump_question, None);
    }

    #[test]
    fn every_registry_id_opens_its_page() {
        let host = AddonHost::standard();
        for id in host.ids() {
            assert_eq!(Page::of(id).id(), id);
            assert_eq!(matches!(Page::of(id), Page::Addon(_)), !host.is_core(id), "{id}");
        }
    }

    #[test]
    fn the_sidebar_lists_the_core_then_the_addons_that_are_on_under_a_heading() {
        let (host, _) = AddonHost::for_tests(&json!({ "drivers": { "enabled": false } }));
        let nav = nav_items(&host);
        let names: Vec<&str> = nav.iter().map(|n| n.name).collect();
        assert_eq!(
            names,
            [
                "Summary", "Performance", "Processes", "System Info", "App history", "Startup apps", "Users", "Services",
                "Power & Freq", "Flight Recorder", "Connections", "Installed Apps", "Disk Space", "Benchmarks", "Package Manager"
            ]
        );
        let headed: Vec<&str> = nav.iter().filter(|n| n.heading).map(|n| n.name).collect();
        assert_eq!(headed, ["Power & Freq"], "one Add-ons heading, before the first add-on");
        assert_eq!(nav[0].page, Page::Core(Core::Summary));
        assert_eq!(nav[8].page, Page::Addon("power-freq"));
    }

    #[test]
    fn go_to_connections_opens_connections_only_while_it_is_on() {
        let ctx = egui::Context::default();
        let mut app = TaskApp::headless(Settings::default(), crate::sampler::Snapshot::default());
        app.page = Page::Core(Core::Processes);
        app.run_action(&ctx, Action::ShowProcessIn { addon: "connections", pid: 9000 });
        assert_eq!(app.page, Page::of("connections"));
        app.page = Page::Core(Core::Processes);
        app.addons.set_enabled("connections", false);
        app.run_action(&ctx, Action::ShowProcessIn { addon: "connections", pid: 9000 });
        assert_eq!(app.page, Page::Core(Core::Processes), "an add-on that is off opens nothing");
    }

    #[test]
    fn go_to_services_opens_services_on_the_services_of_the_host() {
        let ctx = egui::Context::default();
        let info = |name: &str| crate::sampler::ServiceInfo { name: name.into(), ..Default::default() };
        let snap = crate::sampler::Snapshot { services: std::sync::Arc::new([(200, vec![info("Winmgmt"), info("Schedule")])].into_iter().collect()), ..Default::default() };
        let mut app = TaskApp::headless(Settings::default(), snap);
        app.page = Page::Core(Core::Processes);
        app.search = "chrome".into();
        app.run_action(&ctx, Action::GoToServices { pid: 200, service: None });
        assert_eq!(app.page, Page::Core(Core::Services));
        assert_eq!(app.svc_selected.as_deref(), Some("Winmgmt"), "its first service selected");
        assert_eq!(app.search, "200", "filtered to the services of its host");
        app.run_action(&ctx, Action::GoToServices { pid: 200, service: Some("Schedule".into()) });
        assert_eq!(app.svc_selected.as_deref(), Some("Schedule"), "a service row's own service");
    }

    #[test]
    fn go_to_services_reads_the_list_through_the_seam() {
        let row = |name: &str, pid| crate::system_lists::ServiceRow { name: name.into(), pid, status: "Running".into(), ..Default::default() };
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.script_services(vec![row("Winmgmt", 200), row("Spooler", 0)]);
        let mut app = TaskApp::headless(Settings::default(), Default::default()).on_system(fake.clone());
        app.run_action(&egui::Context::default(), Action::GoToServices { pid: 200, service: None });
        assert_eq!(app.page, Page::Core(Core::Services));
        assert_eq!(fake.service_reads(), 1);
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let shown = crate::test_ui::texts(&crate::test_ui::run(&ctx, (1300.0, 800.0), vec![], |ui| app.page_services(ui, &mut Vec::new())));
        assert_eq!(fake.service_reads(), 1, "the page does not read it again");
        assert!(shown.iter().any(|w| w == "Winmgmt") && !shown.iter().any(|w| w == "Spooler"), "the host's services only: {shown:?}");
    }

    fn entry(id: &str, name: &str, enabled: bool) -> crate::system_lists::StartupRow {
        crate::system_lists::StartupRow { id: id.into(), name: name.into(), enabled, ..Default::default() }
    }

    /// An app on the Startup apps page, over a fake holding `rows`.
    fn startup_app(rows: Vec<crate::system_lists::StartupRow>) -> (TaskApp, std::sync::Arc<crate::system_lists::fake::FakeSystem>) {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.script_startup(rows);
        let mut app = TaskApp::headless(Settings::default(), Default::default()).on_system(fake.clone());
        app.page = Page::Core(Core::Startup);
        app.refresh_now();
        (app, fake)
    }

    #[test]
    fn a_startup_toggle_reports_and_rereads() {
        let (mut app, fake) = startup_app(vec![entry("run|Steam", "Steam", true)]);
        let ctx = egui::Context::default();
        let reads = fake.startup_reads();
        fake.script_startup(vec![entry("run|Steam", "Steam", false)]);
        app.run_action(&ctx, Action::Startup("run|Steam".into(), false));
        assert_eq!(fake.startup_requests(), vec![("run|Steam".to_string(), false)]);
        assert_eq!(said(&app).as_deref(), Some("Steam disabled"));
        assert_eq!(fake.startup_reads(), reads + 1, "the list is read again");
        assert!(!app.startup.as_ref().unwrap()[0].enabled, "the reread state is kept");
        app.run_action(&ctx, Action::Startup("run|Steam".into(), true));
        assert_eq!(said(&app).as_deref(), Some("Steam enabled"));
    }

    #[test]
    fn a_refused_startup_toggle_says_the_error_and_rereads() {
        let (mut app, fake) = startup_app(vec![entry("run|Steam", "Steam", true)]);
        fake.fail_startup_toggle("Access denied. Run the app as administrator to change this entry.");
        let reads = fake.startup_reads();
        app.run_action(&egui::Context::default(), Action::Startup("run|Steam".into(), false));
        assert_eq!(said(&app).as_deref(), Some("Access denied. Run the app as administrator to change this entry."));
        assert_eq!(fake.startup_reads(), reads + 1);
    }

    #[test]
    fn a_startup_toggle_targets_the_chosen_entry_after_the_list_changed() {
        let (mut app, fake) = startup_app(vec![entry("run|Alpha", "Alpha", true), entry("run|Beta", "Beta", true)]);
        // Between the click and the action the list is read again and a new
        // entry sorts in front: Beta moves from index 1 to index 2.
        fake.script_startup(vec![entry("run|Aardvark", "Aardvark", true), entry("run|Alpha", "Alpha", true), entry("run|Beta", "Beta", true)]);
        app.refresh_now();
        app.run_action(&egui::Context::default(), Action::Startup("run|Beta".into(), false));
        assert_eq!(fake.startup_requests(), vec![("run|Beta".to_string(), false)]);
        assert_eq!(said(&app).as_deref(), Some("Beta disabled"));
    }

    #[test]
    fn a_startup_toggle_of_an_entry_that_left_the_list_changes_nothing() {
        let (mut app, fake) = startup_app(vec![entry("run|Steam", "Steam", true)]);
        fake.script_startup(vec![]);
        app.refresh_now();
        let reads = fake.startup_reads();
        app.run_action(&egui::Context::default(), Action::Startup("run|Steam".into(), false));
        assert!(fake.startup_requests().is_empty(), "no entry is toggled");
        assert_eq!(said(&app).as_deref(), Some("That startup app is no longer in the list"));
        assert_eq!(fake.startup_reads(), reads + 1, "the list is read again");
    }

    #[test]
    fn the_sidebar_reads_the_startup_list_through_the_seam() {
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.script_startup(vec![entry("run|Steam", "Steam", true)]);
        let mut app = TaskApp::headless(Settings::default(), Default::default()).on_system(fake.clone());
        app.refresh_now();
        assert_eq!(fake.startup_reads(), 0, "refresh on another page reads no startup list");
        app.page = Page::Core(Core::Startup);
        app.refresh_now();
        assert_eq!(fake.startup_reads(), 1);
        assert_eq!(app.startup.as_ref().map(|l| l.len()), Some(1));
    }

    #[test]
    fn a_service_control_reports_and_rereads_through_the_seam() {
        let ctx = egui::Context::default();
        for (what, said_text) in [("start", "Started Spooler"), ("stop", "Stopped Spooler"), ("restart", "Restarted Spooler")] {
            let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
            let mut app = TaskApp::headless(Settings::default(), Default::default()).on_system(fake.clone());
            app.run_action(&ctx, Action::Service("Spooler".into(), what));
            assert_eq!(fake.service_requests(), vec![("Spooler".to_string(), what.to_string())]);
            assert_eq!(said(&app).as_deref(), Some(said_text));
            assert_eq!(fake.service_reads(), 1, "{what} rereads the list");
        }
    }

    #[test]
    fn a_failed_service_control_says_the_error_and_still_rereads() {
        let reason = "Access denied for \"Spooler\". Run the app as administrator to change services.";
        let fake = std::sync::Arc::new(crate::system_lists::fake::FakeSystem::default());
        fake.fail_service_control(reason);
        let mut app = TaskApp::headless(Settings::default(), Default::default()).on_system(fake.clone());
        app.run_action(&egui::Context::default(), Action::Service("Spooler".into(), "stop"));
        assert_eq!(fake.service_requests().len(), 1);
        assert_eq!(said(&app).as_deref(), Some(reason));
        assert_eq!(fake.service_reads(), 1);
    }

    #[test]
    fn the_app_names_no_addon() {
        let sources = [
            ("app.rs", include_str!("app.rs")),
            ("pages.rs", include_str!("pages.rs")),
            ("pages2.rs", include_str!("pages2.rs")),
            ("platform.rs", include_str!("platform.rs")),
            ("widgets.rs", include_str!("widgets.rs")),
            ("settings.rs", include_str!("settings.rs")),
            ("process_view.rs", include_str!("process_view.rs")),
            ("inspector.rs", include_str!("inspector.rs")),
            ("wait_chain.rs", include_str!("wait_chain.rs")),
            ("file_activity.rs", include_str!("file_activity.rs")),
        ];
        let host = AddonHost::standard();
        for (file, src) in sources {
            let code = src.split("#[cfg(test)]").next().unwrap();
            for (addon, _) in host.addons() {
                for word in [addon.id(), addon.name()] {
                    assert!(!code.contains(word), "{file} names the {} add-on ({word})", addon.name());
                }
            }
        }
    }

    #[test]
    fn the_registry_is_the_only_list_of_core_ids_and_names() {
        let code = include_str!("app.rs").split("#[cfg(test)]").next().unwrap();
        let host = AddonHost::standard();
        for (entry, _) in host.entries().filter(|(e, _)| host.is_core(e.id())) {
            assert!(!code.contains(&format!("\"{}\"", entry.id())), "app.rs repeats the core id {}", entry.id());
            assert!(!code.contains(&format!("\"{}\"", entry.name())), "app.rs repeats the core name {}", entry.name());
        }
    }

    #[test]
    fn a_new_addon_needs_only_its_module_and_a_registry_line() {
        let (host, _) = AddonHost::for_tests_with(test_registry(), &json!({}));
        let nav = nav_items(&host);
        let fake = nav.last().expect("in the sidebar");
        assert_eq!((fake.name, fake.page, fake.heading), ("Fake", Page::Addon("fake"), false));
        let page = match host.landing(Some("Fake"), "Last used", "summary") {
            Landing::Page(id) => Page::of(id),
            other => panic!("{other:?}"),
        };
        assert_eq!(page, Page::Addon("fake"), "--page opens it");
        assert_eq!(page_name(&host, page), "Fake", "its page title");
    }

    /// The process names the Processes table draws.
    fn table_names(app: &mut TaskApp) -> Vec<String> {
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let out = crate::test_ui::run(&ctx, (1800.0, 900.0), vec![], |ui| app.page_processes(ui, &mut Vec::new()));
        crate::test_ui::texts(&out).into_iter().filter(|t| t.ends_with(".exe")).collect()
    }

    #[test]
    fn pausing_stops_new_snapshots_from_changing_the_table() {
        let one = |pid, name: &str| Snapshot { procs: vec![crate::sampler::ProcRow { pid, name: name.into(), ..Default::default() }], ..Default::default() };
        // Exited processes are not kept, so the table holds the last sample only.
        let mut settings = Settings::default();
        settings.show_exited = false;
        let (mut app, feed) = TaskApp::fed(settings, one(1, "before.exe"));
        app.settings.speed = SPEEDS.iter().position(|s| s.1 == 0).expect("a Paused speed");
        feed.send(one(2, "late.exe")).expect("sent");
        app.pull_samples();
        assert_eq!(table_names(&mut app), ["before.exe"], "paused, the table stays");
        feed.send(Snapshot { forced: true, ..one(3, "refreshed.exe") }).expect("sent");
        app.pull_samples();
        assert_eq!(table_names(&mut app), ["refreshed.exe"], "Refresh still reads while paused");
        app.settings.speed = Settings::default().speed;
        feed.send(one(4, "running.exe")).expect("sent");
        app.pull_samples();
        assert_eq!(table_names(&mut app), ["running.exe"], "running again, samples reach the table");
    }
}
