//! The Process Inspector: one window per inspected process, opened by
//! Inspect or a double-click on a Processes row. It holds Refresh, 23
//! properties, the command line, the
//! environment (with Filter and Copy) and the open files, registry keys and
//! sockets. Everything but the properties is read off the UI thread.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Duration;

use egui::{vec2, Align, Layout, RichText, TextStyle, Ui};

use crate::app::Action;
use crate::availability::{unavailable_text, Feature};
use crate::page_perf::fmt_bytes_rate;
use crate::proc_info::{self, Missing, Resource, ResourceKind};
use crate::sampler::ProcRow;
use crate::theme::{self, Theme};
use crate::widgets::{fmt_bytes, fmt_cpu_time, fmt_mb, refresh_button};

/// The window's title, before the process's name and PID.
pub const TITLE: &str = "Process Inspector";
/// Said instead of the details when the process is gone.
pub const EXITED: &str = "The process exited before its details could be read.";
/// A value the process may not be opened for.
pub const DENIED: &str = "Access denied";
/// A value Windows did not give.
pub const UNAVAILABLE: &str = "Unavailable";
/// The open resources while they are read.
const RESOURCES_LOADING: &str = "Open files and keys: Loading";
/// How long one handle's name may take before it is given up (a named
/// pipe can block the query forever).
pub const NAME_TIMEOUT: Duration = Duration::from_millis(100);

/// The egui id of the Inspector window of `pid`.
pub fn window_id(pid: u32) -> egui::Id {
    egui::Id::new(("process-inspector", pid))
}

/// The words for a part that could not be read.
pub fn missing_text(m: Missing) -> &'static str {
    match m {
        Missing::Denied => DENIED,
        Missing::Unavailable => UNAVAILABLE,
        Missing::Exited => EXITED,
    }
}

/// How the Inspector types an open resource's row.
pub fn kind_label(kind: ResourceKind) -> &'static str {
    match kind {
        ResourceKind::File => "Open file",
        ResourceKind::Key => "Registry key",
        ResourceKind::Socket => "Socket",
    }
}

/// What the Inspector reads from the process itself, off the UI thread.
#[derive(Clone, Debug, PartialEq)]
pub struct Details {
    pub command_line: Result<String, Missing>,
    pub environment: Result<Vec<(String, String)>, Missing>,
    pub resources: Result<Vec<Resource>, Missing>,
    pub private_memory: Option<u64>,
}

impl Details {
    /// Reads the process's PEB and its handles. Slow (a system-wide handle
    /// list, a name query per file and key): never call it on the UI thread.
    pub fn read(pid: u32) -> Details {
        let (command_line, environment) = proc_info::command_line_and_environment(pid);
        Details {
            command_line,
            environment,
            resources: proc_info::open_resources(pid, NAME_TIMEOUT),
            private_memory: proc_info::private_memory(pid),
        }
    }

    /// What shows when the read could not run (its worker did not start
    /// or died): every part "Unavailable".
    fn unavailable() -> Details {
        Details { command_line: Err(Missing::Unavailable), environment: Err(Missing::Unavailable), resources: Err(Missing::Unavailable), private_memory: None }
    }

    /// Whether a read found the process gone.
    fn exited(&self) -> bool {
        [self.command_line.as_ref().err(), self.environment.as_ref().err(), self.resources.as_ref().err()].contains(&Some(&Missing::Exited))
    }
}

/// One Process Inspector window.
pub struct Inspector {
    pub pid: u32,
    /// The process as the last snapshot had it at open or Refresh; `None`
    /// once it is gone.
    row: Option<ProcRow>,
    /// Its parent as the details pane shows it.
    parent: String,
    /// The last read of the process itself; `None` while it is read.
    details: Option<Details>,
    /// The read under way, on its own thread.
    reading: Option<Receiver<Details>>,
    /// The Environment's filter.
    filter: String,
}

impl Inspector {
    /// An Inspector of `pid` from the processes in `procs`; the process
    /// itself is read on a worker thread.
    pub fn open(pid: u32, procs: &[ProcRow]) -> Self {
        let mut inspector = Inspector { pid, row: None, parent: String::new(), details: None, reading: None, filter: String::new() };
        inspector.refresh(procs);
        inspector
    }

    /// An Inspector showing `details` as already read (for tests).
    #[cfg(test)]
    pub fn with_details(pid: u32, procs: &[ProcRow], details: Details) -> Self {
        let mut inspector = Inspector { pid, row: None, parent: String::new(), details: Some(details), reading: None, filter: String::new() };
        inspector.take_row(procs);
        inspector
    }

    fn take_row(&mut self, procs: &[ProcRow]) {
        self.row = procs.iter().find(|p| p.pid == self.pid).cloned();
        self.parent = self.row.as_ref().map(|p| crate::page_processes::parent_text(p, procs).0).unwrap_or_default();
    }

    /// Takes the process again from `procs` and reads it again.
    fn refresh(&mut self, procs: &[ProcRow]) {
        self.refresh_with(procs, Details::read);
    }

    /// [`Inspector::refresh`] reading the process with `read`.
    /// A click while a read is under way is ignored: each read holds a
    /// thread and the system's handle list until it is done.
    fn refresh_with(&mut self, procs: &[ProcRow], read: fn(u32) -> Details) {
        if self.reading.is_some() {
            return;
        }
        self.take_row(procs);
        self.details = None;
        if self.row.is_none() {
            self.reading = None;
            return;
        }
        let (tx, rx) = channel();
        let pid = self.pid;
        let started = std::thread::Builder::new().name("process-inspector".into()).spawn(move || {
            let _ = tx.send(read(pid));
        });
        self.reading = started.is_ok().then_some(rx);
        if started.is_err() {
            self.details = Some(Details::unavailable());
        }
    }

    /// Takes the worker's read when it is done.
    fn poll(&mut self) {
        let Some(rx) = &self.reading else { return };
        match rx.try_recv() {
            Ok(d) => {
                self.details = Some(d);
                self.reading = None;
            }
            // The worker died without a read: say so instead of Loading.
            Err(TryRecvError::Disconnected) => {
                self.reading = None;
                self.details = Some(Details::unavailable());
            }
            Err(TryRecvError::Empty) => {}
        }
    }

    /// Whether the window says the process exited instead of its details.
    pub fn exited(&self) -> bool {
        self.row.is_none() || self.details.as_ref().is_some_and(Details::exited)
    }

    /// The 23 properties the inspector lists, top to bottom.
    pub fn properties(&self) -> Vec<(&'static str, String)> {
        let Some(p) = &self.row else { return Vec::new() };
        let or = |v: &str, missing: &str| if v.is_empty() { missing.to_string() } else { v.to_string() };
        let private = match (&self.details, self.details.as_ref().and_then(|d| d.private_memory)) {
            (_, Some(bytes)) => fmt_mb(bytes),
            (None, None) => "Loading".to_string(),
            (Some(_), None) => UNAVAILABLE.to_string(),
        };
        vec![
            ("Process", format!("{} ({})", p.name, p.pid)),
            ("Parent", self.parent.clone()),
            ("Session", p.session.to_string()),
            ("User", or(&p.user, DENIED)),
            ("Status", or(&p.status, UNAVAILABLE)),
            ("Architecture", or(p.arch, UNAVAILABLE)),
            ("Image path", or(&p.exe, DENIED)),
            ("Threads", p.threads.to_string()),
            ("CPU time", fmt_cpu_time(p.cpu_time)),
            ("Kernel time", fmt_cpu_time(p.kernel_time)),
            ("User time", fmt_cpu_time(p.user_time)),
            ("Working set", fmt_mb(p.working_set)),
            ("Peak working set", fmt_mb(p.peak_mem)),
            ("Private memory", private),
            ("Commit size", fmt_mb(p.commit)),
            ("Page faults", p.page_faults.to_string()),
            ("Handles", p.handles.to_string()),
            ("Disk read", fmt_bytes_rate(p.disk_read)),
            ("Disk write", fmt_bytes_rate(p.disk_write)),
            ("I/O read", fmt_bytes(p.io_read)),
            ("I/O write", fmt_bytes(p.io_write)),
            ("Priority", or(p.priority, UNAVAILABLE)),
            ("Base priority", p.base_priority.to_string()),
        ]
    }

    /// The variables the filter keeps (by name or value, any case).
    fn shown_environment(&self) -> Vec<(String, String)> {
        let q = self.filter.trim().to_lowercase();
        match self.details.as_ref().map(|d| &d.environment) {
            Some(Ok(vars)) => vars.iter().filter(|(k, v)| q.is_empty() || k.to_lowercase().contains(&q) || v.to_lowercase().contains(&q)).cloned().collect(),
            _ => Vec::new(),
        }
    }

    /// Draws the window; false once the user closed it. Its content width
    /// opens at `width` and a resize is written back to it (saved with the
    /// Processes table's settings).
    pub fn show(&mut self, ctx: &egui::Context, procs: &[ProcRow], width: &mut f32, actions: &mut Vec<Action>) -> bool {
        self.poll();
        if self.reading.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        let mut open = true;
        let name = self.row.as_ref().map_or(String::new(), |p| format!(" \u{b7} {} ({})", p.name, p.pid));
        let mut drawn = None;
        let mut refresh = false;
        egui::Window::new(format!("{TITLE}{name}"))
            .id(window_id(self.pid))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(*width)
            // As tall as the screen allows; the content scrolls past that.
            .default_height(1000.0)
            .show(ctx, |ui| {
                drawn = Some(ui.max_rect().width());
                refresh = self.body(ui, actions);
            });
        if let Some(w) = drawn {
            if (w - *width).abs() > 0.5 {
                *width = w;
            }
        }
        if refresh {
            self.refresh(procs);
        }
        open
    }

    /// The window's content; true when Refresh was clicked.
    fn body(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) -> bool {
        let t = theme::of_ui(ui);
        let refresh = ui.horizontal(|ui| ui.with_layout(Layout::right_to_left(Align::Center), |ui| refresh_button(ui).clicked()).inner).inner;
        if self.exited() {
            ui.label(RichText::new(EXITED).font(t.fonts.row.clone()).color(t.text));
            return refresh;
        }
        egui::ScrollArea::vertical().id_salt("inspector-body").auto_shrink([false, false]).show(ui, |ui| {
            let props = self.properties();
            two_columns(ui, &t, "inspector-properties", ("Property", "Value"), props.iter().map(|(k, v)| (*k, v.as_str())));
            heading(ui, &t, "Command line");
            let command = match self.details.as_ref().map(|d| &d.command_line) {
                None => "Loading".to_string(),
                Some(Ok(c)) => c.clone(),
                Some(Err(m)) => missing_text(*m).to_string(),
            };
            ui.add(egui::TextEdit::multiline(&mut command.as_str()).desired_rows(2).desired_width(f32::INFINITY).font(TextStyle::Monospace));
            heading(ui, &t, "Environment");
            let shown = self.shown_environment();
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Filter").desired_width(220.0));
                if ui.add_enabled(!shown.is_empty(), egui::Button::new("Copy")).clicked() {
                    let text: Vec<String> = shown.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    ui.ctx().copy_text(text.join("\r\n"));
                    actions.push(Action::Say(format!("Copied {} environment variables", shown.len())));
                }
            });
            match self.details.as_ref().map(|d| &d.environment) {
                None => soft(ui, &t, "Loading"),
                Some(Err(m)) => soft(ui, &t, missing_text(*m)),
                Some(Ok(_)) => two_columns(ui, &t, "inspector-environment", ("Variable", "Value"), shown.iter().map(|(k, v)| (k.as_str(), v.as_str()))),
            }
            heading(ui, &t, "Open resources");
            if let Some(why) = unavailable_text(Feature::OpenHandles) {
                soft(ui, &t, &why);
            } else {
                match self.details.as_ref().map(|d| &d.resources) {
                    None => soft(ui, &t, RESOURCES_LOADING),
                    Some(Err(m)) => soft(ui, &t, missing_text(*m)),
                    Some(Ok(list)) => {
                        let count = |k| list.iter().filter(|r| r.kind == k).count();
                        let sum = format!("{} files, {} keys, {} pipes/sockets", count(ResourceKind::File), count(ResourceKind::Key), count(ResourceKind::Socket));
                        soft(ui, &t, &sum);
                        two_columns(ui, &t, "inspector-resources", ("Type", "Name"), list.iter().map(|r| (kind_label(r.kind), r.name.as_str())));
                    }
                }
            }
        });
        refresh
    }
}

/// A part's title, with a little room above it.
fn heading(ui: &mut Ui, t: &Theme, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text).font(t.fonts.panel_heading.clone()).color(t.text));
}

/// A line of quiet text.
fn soft(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text).font(t.fonts.row.clone()).color(t.soft));
}

/// A two-column list under its two titles, the values cut to the width.
fn two_columns<'a>(ui: &mut Ui, t: &Theme, id: &str, titles: (&str, &str), rows: impl Iterator<Item = (&'a str, &'a str)>) {
    egui::Grid::new(id).num_columns(2).striped(true).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
        ui.label(RichText::new(titles.0).font(t.fonts.label.clone()).color(t.label));
        ui.label(RichText::new(titles.1).font(t.fonts.label.clone()).color(t.label));
        ui.end_row();
        for (k, v) in rows {
            ui.label(RichText::new(k).font(t.fonts.row.clone()).color(t.label));
            ui.add(egui::Label::new(RichText::new(v).font(t.fonts.row.clone()).color(t.text)).truncate());
            ui.end_row();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::availability::unavailable_in_test;
    use crate::theme::Mode;

    /// NordVPN.exe as the approved prototype shows it, its parent gone.
    fn nordvpn() -> ProcRow {
        ProcRow {
            pid: 26920,
            name: "NordVPN.exe".into(),
            parent: Some(9916),
            user: "spata".into(),
            status: "Running".into(),
            exe: r"C:\Program Files\NordVPN\NordVPN.exe".into(),
            session: 1,
            arch: "x64",
            threads: 72,
            cpu_time: Duration::from_millis(321_010),
            kernel_time: Duration::from_millis(62_400),
            user_time: Duration::from_millis(258_610),
            working_set: 620_550_144,
            peak_mem: 652_420_301,
            commit: 349_175_808,
            page_faults: 414_119,
            handles: 1642,
            io_read: 1_288_490_189,
            io_write: 327_155_712,
            priority: "Normal",
            base_priority: 8,
            ..Default::default()
        }
    }

    fn read_ok() -> Details {
        Details {
            command_line: Ok(r#""C:\Program Files\NordVPN\NordVPN.exe" --auto-start"#.into()),
            environment: Ok(vec![("OS".into(), "Windows_NT".into()), ("TEMP".into(), r"C:\Temp".into())]),
            resources: Ok(vec![
                Resource { kind: ResourceKind::File, name: r"C:\Program Files\NordVPN\nordvpn.log".into() },
                Resource { kind: ResourceKind::Key, name: r"HKLM\SOFTWARE\NordVPN".into() },
                Resource { kind: ResourceKind::Socket, name: r"\Device\Afd".into() },
            ]),
            private_memory: Some(349_175_808),
        }
    }

    /// Where each text was drawn by one frame of the window (a window is
    /// only sized on its first frame, so a first one is drawn unseen).
    fn frame(ctx: &egui::Context, inspector: &mut Inspector, width: &mut f32, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let procs = [nordvpn()];
        if ctx.memory(|m| m.area_rect(window_id(inspector.pid)).is_none()) {
            crate::test_ui::run(ctx, (1400.0, 1400.0), vec![], |ui| {
                inspector.show(ui.ctx(), &procs, width, &mut Vec::new());
            });
        }
        let out = crate::test_ui::run(ctx, (1400.0, 1400.0), events, |ui| {
            inspector.show(ui.ctx(), &procs, width, &mut Vec::new());
        });
        crate::test_ui::flat_shapes(&out)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Text(g) => Some((g.galley.text().to_string(), g.visual_bounding_rect())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn unavailable_open_resources_show_as_unavailable_not_as_a_list() {
        let _unavailable = unavailable_in_test(&[Feature::OpenHandles]);
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut inspector = Inspector::with_details(26920, &[nordvpn()], read_ok());
        let texts: Vec<String> = frame(&ctx, &mut inspector, &mut 620.0, vec![]).into_iter().map(|x| x.0).collect();
        assert!(texts.iter().any(|t| t == "Open resources"), "the section stays: {texts:?}");
        assert!(texts.iter().any(|t| t == "Not available on this system."), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Open file" || t.contains("0 files")), "no list, not even an empty one: {texts:?}");
    }

    #[test]
    fn the_inspector_lists_its_23_properties_then_the_command_line_environment_and_resources() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut inspector = Inspector::with_details(26920, &[nordvpn()], read_ok());
        let spots = frame(&ctx, &mut inspector, &mut 620.0, vec![]);
        let top = |s: &str| spots.iter().find(|(t, _)| t == s).unwrap_or_else(|| panic!("{s} in {:?}", spots.iter().map(|x| &x.0).collect::<Vec<_>>())).1.top();
        let labels = [
            "Process", "Parent", "Session", "User", "Status", "Architecture", "Image path", "Threads", "CPU time", "Kernel time", "User time", "Working set",
            "Peak working set", "Private memory", "Commit size", "Page faults", "Handles", "Disk read", "Disk write", "I/O read", "I/O write", "Priority",
            "Base priority",
        ];
        assert_eq!(labels.len(), 23);
        let tops: Vec<f32> = labels.iter().map(|l| top(l)).collect();
        assert!(tops.windows(2).all(|w| w[0] < w[1]), "top to bottom in the listed order: {tops:?}");
        for value in ["NordVPN.exe (26920)", "9916 (exited)", "0 B/sec", "Running", "Normal"] {
            top(value);
        }
        let parts = ["Property", "Command line", "Environment", "Open resources"];
        let part_tops: Vec<f32> = parts.iter().map(|p| top(p)).collect();
        assert!(part_tops.windows(2).all(|w| w[0] < w[1]), "{parts:?} in order");
        for shown in [r#""C:\Program Files\NordVPN\NordVPN.exe" --auto-start"#, "Variable", "Windows_NT", "Copy", "Open file", "Registry key", "Socket", r"HKLM\SOFTWARE\NordVPN"] {
            top(shown);
        }
        assert!(spots.iter().any(|(t, _)| t.starts_with("Process Inspector") && t.contains("NordVPN.exe (26920)")), "the title names the process");
    }

    #[test]
    fn parts_that_cannot_be_read_say_access_denied_or_unavailable() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let details = Details { command_line: Err(Missing::Denied), environment: Err(Missing::Denied), resources: Err(Missing::Unavailable), private_memory: None };
        let mut inspector = Inspector::with_details(26920, &[ProcRow { user: String::new(), ..nordvpn() }], details);
        let texts: Vec<String> = frame(&ctx, &mut inspector, &mut 620.0, vec![]).into_iter().map(|x| x.0).collect();
        let count = |s: &str| texts.iter().filter(|t| *t == s).count();
        assert!(count(DENIED) >= 2, "the command line and the environment: {texts:?}");
        assert!(count(UNAVAILABLE) >= 2, "the open resources and private memory: {texts:?}");
    }

    #[test]
    fn an_inspector_of_a_process_that_has_gone_says_it_exited() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut gone = Inspector::open(4_000_000_001, &[nordvpn()]);
        let texts: Vec<String> = frame(&ctx, &mut gone, &mut 620.0, vec![]).into_iter().map(|x| x.0).collect();
        assert!(texts.iter().any(|t| t == EXITED), "{texts:?}");
        let ended = Details { command_line: Err(Missing::Exited), environment: Err(Missing::Exited), resources: Err(Missing::Exited), private_memory: None };
        let mut read_too_late = Inspector::with_details(26920, &[nordvpn()], ended);
        let texts: Vec<String> = frame(&ctx, &mut read_too_late, &mut 620.0, vec![]).into_iter().map(|x| x.0).collect();
        assert!(texts.iter().any(|t| t == EXITED), "{texts:?}");
        assert!(!texts.iter().any(|t| t == "Private memory"), "no properties under the message");
    }

    #[test]
    fn refresh_while_a_read_is_under_way_starts_no_second_read() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::RwLock;
        static READS: AtomicUsize = AtomicUsize::new(0);
        static GATE: RwLock<()> = RwLock::new(());
        fn read(_pid: u32) -> Details {
            READS.fetch_add(1, Ordering::SeqCst);
            drop(GATE.read());
            read_ok()
        }
        let procs = [nordvpn()];
        let mut inspector = Inspector::with_details(26920, &procs, read_ok());
        let shut = GATE.write().unwrap();
        inspector.refresh_with(&procs, read);
        inspector.refresh_with(&procs, read);
        drop(shut);
        let done = inspector.reading.as_ref().expect("a read under way").recv_timeout(Duration::from_secs(5));
        assert_eq!(done, Ok(read_ok()));
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(READS.load(Ordering::SeqCst), 1, "one read for two clicks");
        // Once that read is in, Refresh reads again.
        inspector.reading = None;
        inspector.refresh_with(&procs, read);
        let again = inspector.reading.as_ref().expect("a second read").recv_timeout(Duration::from_secs(5));
        assert_eq!(again, Ok(read_ok()));
        assert_eq!(READS.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn a_read_whose_worker_dies_says_unavailable_instead_of_loading_for_ever() {
        fn dies(_pid: u32) -> Details {
            panic!("the worker dies (test)")
        }
        let procs = [nordvpn()];
        let mut inspector = Inspector::with_details(26920, &procs, read_ok());
        inspector.refresh_with(&procs, dies);
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while inspector.reading.is_some() && std::time::Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
            inspector.poll();
        }
        assert!(inspector.reading.is_none(), "the read is over");
        let d = inspector.details.as_ref().expect("details, not Loading");
        assert_eq!(d.command_line, Err(Missing::Unavailable));
        assert_eq!(d.resources, Err(Missing::Unavailable));
    }

    #[test]
    fn the_filter_keeps_the_variables_that_match() {
        let mut inspector = Inspector::with_details(26920, &[nordvpn()], read_ok());
        inspector.filter = "temp".into();
        assert_eq!(inspector.shown_environment(), [("TEMP".to_string(), r"C:\Temp".to_string())]);
    }

    #[test]
    fn the_inspector_opens_at_the_saved_width_and_a_resize_is_kept() {
        let ctx = crate::test_ui::context(Mode::Dark);
        let mut inspector = Inspector::with_details(26920, &[nordvpn()], read_ok());
        let mut width = 700.0;
        frame(&ctx, &mut inspector, &mut width, vec![]);
        frame(&ctx, &mut inspector, &mut width, vec![]);
        assert!((width - 700.0).abs() < 1.0, "opens at the saved width: {width}");
        let rect = ctx.memory(|m| m.area_rect(window_id(26920))).expect("the window's area");
        let edge = egui::pos2(rect.right() - 1.0, rect.center().y);
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        let to = edge + vec2(120.0, 0.0);
        frame(&ctx, &mut inspector, &mut width, vec![egui::Event::PointerMoved(edge)]);
        frame(&ctx, &mut inspector, &mut width, vec![button(edge, true)]);
        frame(&ctx, &mut inspector, &mut width, vec![egui::Event::PointerMoved(edge + vec2(60.0, 0.0))]);
        frame(&ctx, &mut inspector, &mut width, vec![egui::Event::PointerMoved(to)]);
        frame(&ctx, &mut inspector, &mut width, vec![button(to, false)]);
        frame(&ctx, &mut inspector, &mut width, vec![]);
        assert!(width > 780.0, "the dragged width is written back: {width}");
    }
}
