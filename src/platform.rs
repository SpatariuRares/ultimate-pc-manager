//! Services the platform shares with every add-on (egui only for colours):
//! the executor of external programs (PowerShell, winget, installers), the
//! administrator check, file dialogs and CSV fields.
//!
//! The executor is the only place add-ons launch processes from; its
//! generic [`AddonContext::run`] writes a program's output to a file and
//! can be stopped. Tests use [`AddonContext::recording`], whose executor
//! records each request and launches nothing; replies prepared with
//! [`RecordingRunner::prepare`] let a test give winget and run requests an
//! exit code and output.
//!
//! [`winget`] reads winget's output and exit codes for every add-on;
//! [`uninstaller`] removes installed programs (registry, MSI, MSIX);
//! [`series`] runs an add-on's jobs one at a time and its lists of items
//! one after another; [`install_queue`] lets one install, update or
//! uninstall job run at a time in the whole app.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

pub mod install_queue;
pub mod series;
/// Windows notifications (toasts) through the notification area.
pub mod toast;
pub mod uninstaller;
pub mod winget;

use crate::addons::leases::{Lease, Leases, Sensors};
use crate::sampler::Snapshot;
use crate::sensors::SensorService;
use crate::settings::Settings;

/// A shared action a page asks for while it is drawn; the app runs the
/// queue after drawing.
#[derive(Clone, PartialEq, Debug)]
pub enum Action {
    End { pid: u32, force: bool, tree: bool, name: String },
    /// The same end for several processes (several rows selected on
    /// Processes).
    EndMany { pids: Vec<u32>, force: bool, tree: bool },
    Priority(u32, &'static str),
    OpenLocation(String),
    SearchOnline(String),
    Copy(String),
    Service(String, &'static str),
    Startup(usize, bool),
    /// A short message in the status bar.
    Say(String),
    /// Opens Windows' Power Options (the app never changes the plan).
    OpenPowerOptions,
    /// File Explorer opens the item's folder with the item selected; a
    /// failure says "File Explorer could not reveal this item.".
    ShowInExplorer(std::path::PathBuf),
    /// Opens the page of another add-on on the row that owns `path` (see
    /// [`PageContext::owner_of`]); nothing when that add-on is off.
    ShowIn { addon: &'static str, path: std::path::PathBuf },
    /// Opens the page of the add-on with this id (e.g. from its Summary
    /// tile); nothing when that add-on is off.
    OpenPage(&'static str),
    /// Opens Processes with the process of this PID selected.
    GoToProcess(u32),
    /// Opens the page of the add-on with this id on the process `pid` (the
    /// item its Processes column adds to a process's menu); nothing when
    /// that add-on is off.
    ShowProcessIn { addon: &'static str, pid: u32 },
    /// Opens the Process Inspector window of the process of this PID.
    Inspect(u32),
    /// Analyze wait chain: opens the wait chain window of the process of
    /// this PID, which walks its threads' wait chains off the UI thread.
    AnalyzeWaitChain(u32),
    /// Resource values: the Processes table shows Memory, Disk and Peak
    /// memory as percents of the machine's, or as values (saved).
    ResourceValues { percents: bool },
    /// Sets the app's update speed (an index of `settings::SPEEDS`; the
    /// last is Paused), as View › Update speed and Settings do.
    UpdateSpeed(usize),
    /// Asks where to save `text` as a CSV file, offering `file_name`.
    ExportCsv { file_name: &'static str, text: String },
    /// Suspends every thread of each process (NtSuspendProcess).
    Suspend(Vec<u32>),
    /// Resumes each suspended process (NtResumeProcess).
    Resume(Vec<u32>),
    /// Efficiency mode: each process gets power throttling and the Low
    /// priority class. The Processes page asks first while "Ask before
    /// applying Efficiency mode" is on.
    Efficiency(Vec<u32>),
    /// An item of the Processes page's Process View, also the View
    /// submenu of a process's menu.
    ProcessView(crate::pages::ProcView),
    /// Restart on the shell's Windows Explorer (the process of this PID):
    /// ends it and starts explorer.exe again.
    RestartExplorer(u32),
    /// Follow process (`Some`) or Stop following (`None`): the followed
    /// row stays selected and in view across snapshots and sorts.
    Follow(Option<u32>),
    /// Go to service(s): opens Services on the services the process `pid`
    /// hosts, with `service` (a service row's own) or its first selected.
    GoToServices { pid: u32, service: Option<String> },
    /// Set affinity: each process may run only on the logical processors
    /// whose bits are set in the mask.
    Affinity(Vec<u32>, u64),
    /// Opens the File activity window of the process of this PID.
    FileActivity(u32),
    /// "Restart as Administrator": starts this app again elevated (UAC
    /// asks) on Processes and closes this window; a refusal is said.
    RestartAsAdministrator,
    /// Create memory dump file: a full dump of the process `pid` (named
    /// `name`) goes to the temp folder, off the UI thread; once written the
    /// app asks "Show it in File Explorer?".
    Dump { pid: u32, name: String },
    /// UAC virtualization: each process's token has it switched the other
    /// way (TokenVirtualizationEnabled).
    UacVirtualization(Vec<u32>),
    /// Bring to front: the process's main window comes to the foreground.
    BringToFront(u32),
    /// Starts (true) or stops (false) the sensor recording of the add-on
    /// that records sensors for other pages (see
    /// [`crate::addons::AddonHost::record_sensors`]); nothing when it is off.
    RecordSensors(bool),
}

/// Said when Restart ends Windows Explorer but explorer.exe does not start
/// again.
pub const EXPLORER_NOT_RESTARTED: &str = "Task Manager ended Windows Explorer, but it did not start again. Use Run new task to start explorer.exe.";

/// Said, before the reason, when Restart cannot end Windows Explorer.
pub const EXPLORER_NOT_ENDED: &str = "Task Manager could not end Windows Explorer.";

/// Said when [`Action::ShowInExplorer`] fails.
pub const REVEAL_FAILED: &str = "File Explorer could not reveal this item.";

/// The row of another add-on that owns a path (e.g. the installed app
/// whose folder holds a file), for a link to it.
#[derive(Clone, Debug, PartialEq)]
pub struct Owner {
    /// The add-on's id (for [`Action::ShowIn`]).
    pub addon: &'static str,
    /// The add-on's shown name (for the link's words).
    pub page: &'static str,
    /// The row's name ("Steam").
    pub label: String,
}

/// Asks the add-ons that are on who owns a path.
pub type Owners<'a> = &'a dyn Fn(&std::path::Path) -> Option<Owner>;

/// What an add-on gets while it draws (its page, Summary tile or settings
/// section): the latest performance sample, the global settings (theme,
/// units, speed) and the queue of shared [`Action`]s.
pub struct PageContext<'a> {
    pub settings: &'a Settings,
    pub sample: &'a Snapshot,
    /// Run by the app after the frame is drawn.
    pub actions: Vec<Action>,
    /// The other add-ons that are on, asked by [`PageContext::owner_of`]
    /// (set by the host while it draws an add-on's page).
    pub owners: Option<Owners<'a>>,
    /// Where the host keeps the sensor interest of the surface being drawn
    /// (set by the host; see [`PageContext::sensors`]).
    pub lease: Option<Lease>,
}

impl<'a> PageContext<'a> {
    /// A context with these settings and sample and no action queued.
    pub fn new(settings: &'a Settings, sample: &'a Snapshot) -> Self {
        PageContext { settings, sample, actions: Vec::new(), owners: None, lease: None }
    }

    /// The row of another add-on that is on which owns `path` (None when
    /// no add-on that is on claims it).
    pub fn owner_of(&self, path: &std::path::Path) -> Option<Owner> {
        self.owners.and_then(|ask| ask(path))
    }

    /// The shared sensor service, for what is drawn now: the latest frame,
    /// history, statistics and "read now". Asking is what keeps the service
    /// running: the host keeps the interest while this is asked in every
    /// frame and drops it at the end of the first frame it is not. Store
    /// nothing: ask again at every draw. None outside a host.
    pub fn sensors(&self) -> Option<Sensors> {
        self.lease.as_ref().map(Lease::ask)
    }

    /// The colour of secondary text in the current theme.
    pub fn sub_text(&self) -> egui::Color32 {
        crate::widgets::sub_text(&crate::theme::Theme::of(crate::theme::Mode::from_dark(self.settings.dark)))
    }
}

/// An add-on's life counter. Its `stop()` bumps it; background work
/// started under an older value is stale: it stops early and must not
/// publish its results (compare under the same lock that guards them).
#[derive(Clone, Default)]
pub struct Generation(Arc<AtomicU64>);

impl Generation {
    /// The value now; work started under it compares against it later.
    pub fn current(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }

    /// Makes every work started so far stale.
    pub fn bump(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// The error of a program ended because its add-on was turned off.
pub const STOPPED: &str = "Stopped.";

/// Locks a page state shared with background work. A thread that panicked
/// while holding it leaves the state usable (the poison is cleared).
pub fn lock_state<S>(state: &Mutex<S>) -> MutexGuard<'_, S> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// A background job's handle to the page state `S` it fills. It was born
/// under one [`Generation`]; once that moves on (the user stopped it or
/// the add-on was turned off) the job is cancelled and publishes nothing.
pub struct Job<S> {
    state: Arc<Mutex<S>>,
    generation: Generation,
    born: u64,
}

impl<S> Clone for Job<S> {
    fn clone(&self) -> Self {
        Job { state: self.state.clone(), generation: self.generation.clone(), born: self.born }
    }
}

impl<S> Job<S> {
    /// A job born now, under the generation's current value.
    pub fn new(state: &Arc<Mutex<S>>, generation: &Generation) -> Self {
        Job { state: state.clone(), generation: generation.clone(), born: generation.current() }
    }

    /// The generation moved on since this job started: stop working.
    pub fn cancelled(&self) -> bool {
        self.generation.current() != self.born
    }

    /// Changes the page state, unless the job was cancelled meanwhile
    /// (checked under the state's lock, so nothing lands after a cancel
    /// made under that lock).
    pub fn publish(&self, f: impl FnOnce(&mut S)) {
        let mut s = lock_state(&self.state);
        if !self.cancelled() {
            f(&mut s);
        }
    }
}

/// How waiting for a started program ended.
pub enum Waited {
    /// It ended, with the exit code of the program started (`None` when
    /// Windows gave no process to read it from).
    Finished(Option<i32>),
    Stopped,
    TimedOut,
}

/// How a program started by [`ProcessRunner::run`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ran {
    /// It could not be started; why.
    NotStarted(String),
    /// It ended by itself with this exit code.
    Exited(i32),
    /// It was ended because `stop` was set.
    Stopped,
}

/// One request to launch an external program.
#[cfg_attr(not(test), allow(dead_code))] // read by the recording executor
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// A PowerShell script (elevated through UAC when asked).
    PowerShell { script: String, args: Vec<(String, String)>, elevated: bool },
    /// winget with these arguments.
    Winget(Vec<String>),
    /// A program run without a window through [`ProcessRunner::run`],
    /// its output written to `output`.
    Run { program: String, args: Vec<String>, output: std::path::PathBuf },
    /// A program started through the shell and waited for.
    Program { file: String, params: String },
    /// A program, document or console opened without waiting ("Run new task").
    Open(String),
    /// This app started again as administrator (UAC asks) with these arguments.
    Elevated(Vec<String>),
    /// A Windows notification (toast) with this title and text.
    Notify { title: String, body: String },
}

/// The error an elevated launch returns when the UAC prompt was dismissed
/// (callers compare with it to tell "nothing was changed" from a failure).
pub const ELEVATION_CANCELLED: &str = "Cancelled: Windows did not get administrator permission.";

/// Launches external programs. The real one runs them; the test one records.
pub trait ProcessRunner: Send + Sync {
    /// Runs a PowerShell script that writes its JSON result to `-Out`.
    fn powershell(&self, script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String>;
    /// Runs a PowerShell script without elevation, like [`ProcessRunner::powershell`],
    /// ending its process (and deleting its script and result files) as soon
    /// as `stop` is set; it then returns an error.
    fn powershell_until(&self, script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String>;
    /// Runs winget without a window: (exit code, output).
    fn winget(&self, args: &[&str]) -> Result<(i32, String), String>;
    /// Runs `program` without a window, each argument passed as is (never
    /// joined into one string). Its output (stdout and stderr) goes to the
    /// file `output`, not to a pipe, so closing this app does not cut the
    /// program's output short. Each new line is passed to `on_line` while
    /// it runs; a set `stop` ends the program.
    fn run(&self, program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran;
    /// Starts a program (Windows asks for permission when it needs it) and
    /// waits for it and every process it starts, until `stop` is set.
    fn run_and_wait(&self, file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String>;
    /// Opens a program, document or console (e.g. `devmgmt.msc`) without waiting.
    fn open(&self, target: &str) -> Result<(), String>;
    /// Starts this app again as administrator (UAC asks) with `args`,
    /// without waiting. The caller then closes this instance.
    fn restart_elevated(&self, args: &[&str]) -> Result<(), String>;
    /// Shows a Windows notification (toast) without waiting for it.
    fn notify(&self, title: &str, body: &str) -> Result<(), String>;
}

/// The executor that really launches programs.
pub struct SystemRunner;

impl ProcessRunner for SystemRunner {
    fn powershell(&self, script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String> {
        imp::run_script(script, args, elevated)
    }
    fn powershell_until(&self, script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String> {
        imp::run_script_until(script, args, stop)
    }
    fn winget(&self, args: &[&str]) -> Result<(i32, String), String> {
        imp::winget(args)
    }
    fn run(&self, program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        imp::run(program, args, output, stop, on_line)
    }
    fn run_and_wait(&self, file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String> {
        imp::run_and_wait(file, params, stop)
    }
    fn open(&self, target: &str) -> Result<(), String> {
        crate::winsys::run_new_task(target)
    }
    fn restart_elevated(&self, args: &[&str]) -> Result<(), String> {
        imp::restart_elevated(args)
    }
    fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        toast::show(title, body)
    }
}

/// An executor that records every request and launches nothing.
#[cfg(test)]
#[derive(Default)]
pub struct RecordingRunner {
    launches: std::sync::Mutex<Vec<Launch>>,
    /// While true, every request is recorded and then waits (a program
    /// that is still running), until [`RecordingRunner::release`].
    held: std::sync::Mutex<bool>,
    released: std::sync::Condvar,
    /// The error every request gets (see [`RecordingRunner::answer`]).
    reply: std::sync::Mutex<Option<String>>,
    /// Prepared replies, in the order they were prepared (see
    /// [`RecordingRunner::prepare`]).
    prepared: std::sync::Mutex<Vec<Prepared>>,
    /// Held runs ended early because their `stop` was set (a real
    /// executor would have ended the program).
    stopped: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
impl RecordingRunner {
    /// The requests so far, in order.
    pub fn launches(&self) -> Vec<Launch> {
        self.launches.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    /// From now on requests do not return until [`RecordingRunner::release`]:
    /// lets a test act while a program "runs".
    pub fn hold(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = true;
    }
    /// From now on every request fails with `error` (e.g.
    /// [`ELEVATION_CANCELLED`], as if the UAC prompt was dismissed).
    pub fn answer(&self, error: &str) {
        *self.reply.lock().unwrap_or_else(|e| e.into_inner()) = Some(error.into());
    }
    /// Lets every held request (and later ones) return.
    pub fn release(&self) {
        *self.held.lock().unwrap_or_else(|e| e.into_inner()) = false;
        self.released.notify_all();
    }
    /// Waits (a few seconds at most) until `n` requests were made.
    pub fn wait_for(&self, n: usize) {
        let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while self.launches().len() < n {
            assert!(std::time::Instant::now() < until, "expected {n} launches, got {:?}", self.launches());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    /// How many held runs were ended by their `stop` flag.
    pub fn stopped(&self) -> usize {
        self.stopped.load(Ordering::SeqCst)
    }
    /// Prepares the reply (exit code and output) of one later winget,
    /// [`ProcessRunner::run`] or [`ProcessRunner::run_and_wait`] request
    /// (its parameters split at spaces are its arguments): the first one
    /// `when` matches gets it, once. Requests nothing matches get the usual
    /// error.
    pub fn prepare(&self, when: When, code: i32, output: &str) {
        self.prepared.lock().unwrap_or_else(|e| e.into_inner()).push(Prepared { when, code, output: output.into(), file: None });
    }
    /// Like [`RecordingRunner::prepare`] for a [`ProcessRunner::run`]
    /// request that writes a file named by its argument after `flag`
    /// (`winget export --output <path>`): `contents` goes to that file,
    /// nothing to the output.
    pub fn prepare_file(&self, when: When, flag: &str, code: i32, contents: &str) {
        self.prepared.lock().unwrap_or_else(|e| e.into_inner()).push(Prepared { when, code, output: contents.into(), file: Some(flag.into()) });
    }
    /// Notes `launch`, then waits while held; returns the error every
    /// request gets (nothing is launched).
    fn record(&self, launch: Launch) -> String {
        match self.record_until(launch, &AtomicBool::new(false)) {
            Answer::Prepared(..) | Answer::Stopped => unreachable!("only winget, run and program-wait requests are answered; nothing stops this wait"),
            Answer::Error(e) => e,
        }
    }
    /// Like [`RecordingRunner::record`], but a set `stop` ends the wait
    /// (counted in [`RecordingRunner::stopped`]), and a winget or run
    /// request may get a prepared reply.
    fn record_until(&self, launch: Launch, stop: &AtomicBool) -> Answer {
        let answerable = matches!(launch, Launch::Winget(_) | Launch::Run { .. } | Launch::Program { .. });
        let index = {
            let mut launches = self.launches.lock().unwrap_or_else(|e| e.into_inner());
            launches.push(launch.clone());
            launches.len() - 1
        };
        let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
        while *held {
            if stop.load(Ordering::SeqCst) {
                self.stopped.fetch_add(1, Ordering::SeqCst);
                return Answer::Stopped;
            }
            held = self.released.wait_timeout(held, std::time::Duration::from_millis(5)).unwrap_or_else(|e| e.into_inner()).0;
        }
        drop(held);
        if answerable {
            if let Some(p) = self.take_prepared(index, &launch) {
                return Answer::Prepared(p);
            }
        }
        Answer::Error(self.reply.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| "Not launched: test executor.".into()))
    }
    /// Removes and returns the first prepared reply that matches the
    /// request `launch`, the `index`-th one made.
    fn take_prepared(&self, index: usize, launch: &Launch) -> Option<Prepared> {
        let args: Vec<String> = match launch {
            Launch::Winget(args) | Launch::Run { args, .. } => args.clone(),
            Launch::Program { params, .. } => params.split_whitespace().map(String::from).collect(),
            _ => Vec::new(),
        };
        let mut prepared = self.prepared.lock().unwrap_or_else(|e| e.into_inner());
        let at = prepared.iter().position(|p| match &p.when {
            When::Arg(arg) => args.contains(arg),
            When::Nth(n) => *n == index,
        })?;
        Some(prepared.remove(at))
    }
}

/// A prepared reply (see [`RecordingRunner::prepare`]).
#[cfg(test)]
struct Prepared {
    when: When,
    code: i32,
    output: String,
    /// The flag whose next argument names the file `output` is written to
    /// instead (see [`RecordingRunner::prepare_file`]).
    file: Option<String>,
}

/// Which request a prepared reply answers (see [`RecordingRunner::prepare`]).
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub enum When {
    /// The first request with this argument.
    Arg(String),
    /// The request in this position (0 is the first request made).
    Nth(usize),
}

/// How the recording executor answers a request.
#[cfg(test)]
enum Answer {
    Prepared(Prepared),
    Stopped,
    Error(String),
}

#[cfg(test)]
impl ProcessRunner for RecordingRunner {
    fn powershell(&self, script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String> {
        let args = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Err(self.record(Launch::PowerShell { script: script.into(), args, elevated }))
    }
    fn powershell_until(&self, script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String> {
        let args = args.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        match self.record_until(Launch::PowerShell { script: script.into(), args, elevated: false }, stop) {
            Answer::Stopped => Err(STOPPED.into()),
            Answer::Prepared(..) => unreachable!("PowerShell requests are not answered"),
            Answer::Error(e) => Err(e),
        }
    }
    fn winget(&self, args: &[&str]) -> Result<(i32, String), String> {
        match self.record_until(Launch::Winget(args.iter().map(|a| a.to_string()).collect()), &AtomicBool::new(false)) {
            Answer::Prepared(p) => Ok((p.code, p.output)),
            Answer::Stopped => unreachable!("nothing stops a winget wait"),
            Answer::Error(e) => Err(e),
        }
    }
    fn run(&self, program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        let launch = Launch::Run { program: program.into(), args: args.iter().map(|a| a.to_string()).collect(), output: output.into() };
        match self.record_until(launch, stop) {
            Answer::Prepared(Prepared { code, output: text, file, .. }) => {
                let named = file.and_then(|flag| args.iter().position(|a| *a == flag).and_then(|i| args.get(i + 1)));
                let (path, text) = match named {
                    Some(path) => {
                        if let Err(e) = std::fs::write(path, &text) {
                            return Ran::NotStarted(format!("Could not write {path}: {e}"));
                        }
                        (output, String::new())
                    }
                    None => (output, text),
                };
                if let Err(e) = std::fs::write(path, &text) {
                    return Ran::NotStarted(format!("Could not write the output file: {e}"));
                }
                text.lines().for_each(|l| on_line(l));
                Ran::Exited(code)
            }
            Answer::Stopped => Ran::Stopped,
            Answer::Error(e) => Ran::NotStarted(e),
        }
    }
    fn run_and_wait(&self, file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String> {
        // A held program is no longer waited for once `stop` is set (it
        // keeps running), as with the real executor.
        match self.record_until(Launch::Program { file: file.into(), params: params.into() }, stop) {
            Answer::Stopped => Ok(Waited::Stopped),
            Answer::Prepared(p) => Ok(Waited::Finished(Some(p.code))),
            Answer::Error(e) => Err(e),
        }
    }
    fn open(&self, target: &str) -> Result<(), String> {
        Err(self.record(Launch::Open(target.into())))
    }
    fn restart_elevated(&self, args: &[&str]) -> Result<(), String> {
        Err(self.record(Launch::Elevated(args.iter().map(|a| a.to_string()).collect())))
    }
    fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        Err(self.record(Launch::Notify { title: title.into(), body: body.into() }))
    }
}

/// What the platform gives every add-on. Cheap to clone; background
/// threads keep a clone.
#[derive(Clone)]
pub struct AddonContext {
    runner: Arc<dyn ProcessRunner>,
    admin: bool,
    dialogs: bool,
    sensors: SensorService,
    leases: Leases,
    queue: install_queue::InstallQueue,
}

impl AddonContext {
    /// The real context: launches programs and shows dialogs.
    pub fn system() -> Self {
        let sensors = SensorService::grouped(crate::sensors::provider::standard());
        let leases = Leases::new(sensors.clone());
        AddonContext { runner: Arc::new(SystemRunner), admin: is_admin(), dialogs: true, sensors, leases, queue: Default::default() }
    }

    /// A context for tests: not admin, no dialogs, an executor that
    /// records the requests (returned too) without launching anything, and
    /// a sensor service over a fixed, made-up CPU.
    #[cfg(test)]
    pub fn recording() -> (Self, Arc<RecordingRunner>) {
        Self::recording_over(SensorService::new(vec![crate::sensors::cpu::made_up(), crate::sensors::power_plan::made_up()]))
    }

    /// [`AddonContext::recording`] over a sensor service of the test's own
    /// (made of fake providers).
    #[cfg(test)]
    pub fn recording_over(sensors: SensorService) -> (Self, Arc<RecordingRunner>) {
        let runner = Arc::new(RecordingRunner::default());
        let leases = Leases::new(sensors.clone());
        (AddonContext { runner: runner.clone(), admin: false, dialogs: false, sensors, leases, queue: Default::default() }, runner)
    }

    /// The shared sensor service. It runs only while someone holds an
    /// [`crate::sensors::Interest`]. For anything that is drawn, take nothing
    /// by hand: a page or tile reads sensors through [`PageContext::sensors`]
    /// while it draws, and the host keeps the interest while it is drawn and
    /// drops it at the end of the first frame it is not, and when the add-on
    /// is turned off. There is nothing to drop in `page_hidden` or `stop`. Only
    /// an interest that does not depend on what is on screen (a recording, an
    /// alert) is taken here, by the add-on that owns it, and dropped by it.
    pub fn sensors(&self) -> &SensorService {
        &self.sensors
    }

    /// The host's keeper of the interests that depend on what is on screen.
    pub fn leases(&self) -> &Leases {
        &self.leases
    }

    /// The app's install queue: every install, update or uninstall job
    /// enters it, so only one runs at a time in the whole app.
    pub fn install_queue(&self) -> &install_queue::InstallQueue {
        &self.queue
    }

    /// Runs a PowerShell script (elevated through UAC when asked) that
    /// writes its JSON result to `-Out`.
    pub fn powershell(&self, script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String> {
        self.runner.powershell(script, args, elevated)
    }

    /// Runs a PowerShell script without elevation, ending it (and deleting
    /// its temporary files) as soon as `stop` is set.
    pub fn powershell_until(&self, script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String> {
        self.runner.powershell_until(script, args, stop)
    }

    /// Runs winget without a window: (exit code, output).
    pub fn winget(&self, args: &[&str]) -> Result<(i32, String), String> {
        self.runner.winget(args)
    }

    /// Runs `program` without a window with these arguments (each passed
    /// as is), its output written to the file `output`; `on_line` gets
    /// each new line while it runs and a set `stop` ends it.
    pub fn run(&self, program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        self.runner.run(program, args, output, stop, on_line)
    }

    /// Starts a program (Windows asks for permission when it needs it) and
    /// waits for it and every process it starts, until `stop` is set.
    pub fn run_and_wait(&self, file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String> {
        self.runner.run_and_wait(file, params, stop)
    }

    /// Opens a program, document or console (e.g. `devmgmt.msc`) without waiting.
    pub fn open(&self, target: &str) -> Result<(), String> {
        self.runner.open(target)
    }

    /// "Restart as administrator": starts this app again elevated (UAC
    /// asks) on the page `page` (a `--page` value). On `Ok` the caller
    /// closes this window; on `Err` (e.g. the prompt was dismissed) it stays.
    pub fn restart_elevated(&self, page: &str) -> Result<(), String> {
        self.runner.restart_elevated(&["--page", page])
    }

    /// Shows a Windows notification (toast) with `title` and `body`,
    /// without waiting; it also shows while the window is minimized.
    pub fn notify(&self, title: &str, body: &str) -> Result<(), String> {
        self.runner.notify(title, body)
    }

    /// Whether this app runs with administrator rights.
    pub fn is_admin(&self) -> bool {
        self.admin
    }

    /// A save dialog for a file of type `kind`; `default` is the suggested
    /// name, or a whole path to start in its folder (None when cancelled).
    pub fn pick_save_as(&self, kind: &FileType, default: &str) -> Option<String> {
        self.pick_save(&file_filter(std::slice::from_ref(kind)), kind.extension, default)
    }

    /// A save dialog with a Win32 filter, opened on `default_path` (a full
    /// path picks the starting folder too); `ext` is added to a name typed
    /// without one. None when cancelled.
    pub fn pick_save(&self, filter: &str, ext: &str, default_path: &str) -> Option<String> {
        if !self.dialogs {
            return None;
        }
        imp::file_dialog(true, filter, ext, default_path, "")
    }

    /// An open dialog listing files of the types `kinds` (the first one
    /// shown first), starting in `folder` when given (None when cancelled).
    pub fn pick_open_file(&self, kinds: &[FileType], folder: Option<&std::path::Path>) -> Option<String> {
        if !self.dialogs {
            return None;
        }
        let folder = folder.map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        imp::file_dialog(false, &file_filter(kinds), "", "", &folder)
    }

    /// A folder picker with a title and the text of its OK button (None
    /// when cancelled).
    pub fn pick_folder(&self, title: &str, ok: &str) -> Option<String> {
        if !self.dialogs {
            return None;
        }
        imp::folder_dialog(title, ok)
    }

    /// An Open dialog titled `title` with a Win32 filter, opened in the
    /// folder `initial_dir` (Windows' choice when empty): Ok(None) when
    /// cancelled, Err when the dialog could not be shown.
    pub fn pick_open(&self, title: &str, filter: &str, initial_dir: &str) -> Result<Option<String>, ()> {
        if !self.dialogs {
            return Ok(None);
        }
        imp::open_dialog(title, filter, initial_dir)
    }
}

/// Windows' Save dialog for a CSV file, offering `default_name` (None when
/// cancelled). Pages without an [`AddonContext`] use it through
/// [`Action::ExportCsv`].
pub fn pick_csv(default_name: &str) -> Option<String> {
    imp::file_dialog(true, &file_filter(&[CSV_FILE]), CSV_FILE.extension, default_name, "")
}

/// A type of file the file dialogs offer, by its extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileType {
    /// What the dialog calls it ("CSV").
    pub name: &'static str,
    /// Its extension without the dot ("csv"; "*" for any file).
    pub extension: &'static str,
}

/// CSV files (exports).
pub const CSV_FILE: FileType = FileType { name: "CSV", extension: "csv" };
/// JSON files.
pub const JSON_FILE: FileType = FileType { name: "JSON", extension: "json" };
/// Any file.
pub const ANY_FILE: FileType = FileType { name: "All files", extension: "*" };

/// The Win32 filter listing `kinds` ("CSV (*.csv)\0*.csv\0\0").
pub fn file_filter(kinds: &[FileType]) -> String {
    let mut filter: String = kinds.iter().map(|k| format!("{} (*.{1})\0*.{1}\0", k.name, k.extension)).collect();
    filter.push('\0');
    filter
}

/// A CSV field, quoted when it needs to be.
pub fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// Starts this app again as administrator (UAC asks) on the page `page` (a
/// `--page` value), without waiting: the executor [`AddonContext::restart_elevated`]
/// uses, for core pages that have no add-on context.
pub fn restart_elevated(page: &str) -> Result<(), String> {
    SystemRunner.restart_elevated(&["--page", page])
}

/// Said when winget cannot be started.
pub const WINGET_MISSING: &str = "winget is not installed. Install \u{201c}App Installer\u{201d} from the Microsoft Store to check for newer versions.";

/// Runs winget through `runner`'s [`ProcessRunner::run`] with its output
/// in a file of `dir` (deleted afterwards): (exit code, output).
#[cfg_attr(not(windows), allow(dead_code))]
fn winget_via_run(runner: &dyn ProcessRunner, dir: &std::path::Path, args: &[&str]) -> Result<(i32, String), String> {
    let output = dir.join(format!("winget-{}.txt", unique_stamp()));
    let ran = runner.run("winget", args, &output, &AtomicBool::new(false), &mut |_| {});
    let text = std::fs::read(&output).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
    let _ = std::fs::remove_file(&output);
    match ran {
        Ran::Exited(code) => Ok((code, text)),
        Ran::NotStarted(_) => Err(WINGET_MISSING.into()),
        Ran::Stopped => Err(STOPPED.into()),
    }
}

/// A name part no other temporary file of this app shares: the process id
/// and the time in nanoseconds.
#[cfg_attr(not(windows), allow(dead_code))]
fn unique_stamp() -> String {
    format!("{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0))
}

/// Seconds since 1970-01-01 in local time (divide by 86,400 for a local
/// day number): the app's one local wall clock (sensors' statistics, the
/// add-ons' history files).
pub fn local_secs() -> i64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs as i64 + local_offset_secs()
}

fn local_offset_secs() -> i64 {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::Time::{GetTimeZoneInformation, TIME_ZONE_INFORMATION};
        let mut tz: TIME_ZONE_INFORMATION = std::mem::zeroed();
        let r = GetTimeZoneInformation(&mut tz);
        let mut bias = tz.Bias;
        if r == 2 {
            bias += tz.DaylightBias;
        } else if r == 1 {
            bias += tz.StandardBias;
        }
        return -(bias as i64) * 60;
    }
    #[allow(unreachable_code)]
    0
}

/// Whether this process runs with administrator rights: the one place that
/// asks Windows (add-ons use [`AddonContext::is_admin`]; sensor providers,
/// built on their own threads, call this).
pub fn is_admin() -> bool {
    #[cfg(windows)]
    // SAFETY: IsUserAnAdmin takes no argument and only reads this
    // process's token.
    unsafe {
        windows_sys::Win32::UI::Shell::IsUserAnAdmin() != 0
    }
    #[cfg(not(windows))]
    false
}

// -------------------------------------------------------------- platform

/// Stand-ins where Windows is missing: every launch fails, no dialog.
#[cfg(not(windows))]
mod imp {
    use super::*;
    /// PowerShell is Windows only.
    pub fn run_script(_: &str, _: &[(&str, &str)], _: bool) -> Result<serde_json::Value, String> {
        Err("Only available on Windows.".into())
    }
    /// PowerShell is Windows only.
    pub fn run_script_until(_: &str, _: &[(&str, &str)], _: &AtomicBool) -> Result<serde_json::Value, String> {
        Err("Only available on Windows.".into())
    }
    /// winget is Windows only.
    pub fn winget(_: &[&str]) -> Result<(i32, String), String> {
        Err("winget is only available on Windows.".into())
    }
    /// Running programs is Windows only.
    pub fn run(_: &str, _: &[&str], _: &std::path::Path, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Ran {
        Ran::NotStarted("Only available on Windows.".into())
    }
    /// Starting programs this way is Windows only.
    pub fn run_and_wait(_: &str, _: &str, _: &AtomicBool) -> Result<Waited, String> {
        Err("Only available on Windows.".into())
    }
    /// Elevation is Windows only.
    pub fn restart_elevated(_: &[&str]) -> Result<(), String> {
        Err("Only available on Windows.".into())
    }
    /// No file dialog: always cancelled.
    pub fn file_dialog(_: bool, _: &str, _: &str, _: &str, _: &str) -> Option<String> {
        None
    }
    /// No folder dialog: always cancelled.
    pub fn folder_dialog(_: &str, _: &str) -> Option<String> {
        None
    }
    /// No open dialog: it cannot be shown.
    pub fn open_dialog(_: &str, _: &str, _: &str) -> Result<Option<String>, ()> {
        Err(())
    }
}

/// The Windows implementation: PowerShell, winget, start-and-wait and
/// the common file dialogs.
#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::mem::zeroed;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessId, WaitForSingleObject, INFINITE};
    use windows_sys::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, GetSaveFileNameW, OPENFILENAMEW};
    use windows_sys::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};

    use crate::winsys::wide;

    /// The text of a NUL-terminated UTF-16 buffer.
    fn from_wide(buf: &[u16]) -> String {
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..n])
    }

    // --------------------------------------------------- PowerShell

    /// Writes `script` to a file, runs it with Windows PowerShell (elevated
    /// through UAC when asked) and reads the JSON it writes to -Out.
    pub fn run_script(script: &str, args: &[(&str, &str)], elevated: bool) -> Result<serde_json::Value, String> {
        let never = AtomicBool::new(false);
        run_in(&jobs_dir()?, script, args, |params| if elevated { run_elevated(params) } else { run_hidden(params, &never) })
    }

    /// Like [`run_script`] without elevation; a set `stop` ends PowerShell
    /// at once (its files are deleted all the same).
    pub fn run_script_until(script: &str, args: &[(&str, &str)], stop: &AtomicBool) -> Result<serde_json::Value, String> {
        run_in(&jobs_dir()?, script, args, |params| run_hidden(params, stop))
    }

    /// Where scripts and their results are written while they run.
    fn jobs_dir() -> Result<std::path::PathBuf, String> {
        let base = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let dir = base.join(crate::settings::DATA_FOLDER).join("jobs");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(dir)
    }

    /// Writes `script` into `dir`, lets `run` start PowerShell with the
    /// parameters, reads the JSON result and deletes both files, however
    /// the run ended.
    pub(super) fn run_in(dir: &std::path::Path, script: &str, args: &[(&str, &str)], run: impl FnOnce(&str) -> Result<(), String>) -> Result<serde_json::Value, String> {
        let stamp = unique_stamp();
        let ps1 = dir.join(format!("job-{stamp}.ps1"));
        let out = dir.join(format!("job-{stamp}.json"));
        // A BOM so Windows PowerShell reads the file as UTF-8.
        std::fs::write(&ps1, format!("\u{feff}{script}")).map_err(|e| e.to_string())?;
        let mut params = format!(
            "-NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File \"{}\" -Out \"{}\"",
            ps1.display(),
            out.display()
        );
        for (k, v) in args {
            params.push_str(&format!(" -{k} \"{}\"", v.replace('"', "")));
        }
        let result = run(&params).and_then(|_| {
            let text = std::fs::read_to_string(&out).map_err(|_| "PowerShell finished without a result.".to_string())?;
            serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("Unreadable result: {e}"))
        });
        let _ = std::fs::remove_file(&ps1);
        let _ = std::fs::remove_file(&out);
        result
    }

    /// Runs Windows PowerShell with `params`, without a window, and waits;
    /// a set `stop` ends it at once.
    pub(super) fn run_hidden(params: &str, stop: &AtomicBool) -> Result<(), String> {
        use std::os::windows::process::CommandExt;
        if stop.load(Ordering::SeqCst) {
            return Err(crate::platform::STOPPED.into());
        }
        let mut child = std::process::Command::new("powershell.exe")
            .raw_arg(params)
            .creation_flags(0x0800_0000)
            .spawn()
            .map_err(|e| format!("Could not start PowerShell: {e}"))?;
        loop {
            if child.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Ok(());
            }
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(crate::platform::STOPPED.into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Runs Windows PowerShell with `params` as administrator (UAC asks)
    /// and waits.
    fn run_elevated(params: &str) -> Result<(), String> {
        unsafe {
            let verb = wide("runas");
            let file = wide("powershell.exe");
            let p = wide(params);
            let mut info: SHELLEXECUTEINFOW = zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = p.as_ptr();
            info.nShow = 0;
            if ShellExecuteExW(&mut info) == 0 {
                return Err(if GetLastError() == 1223 {
                    ELEVATION_CANCELLED.into()
                } else {
                    format!("Could not start an elevated PowerShell (error {}).", GetLastError())
                });
            }
            if !info.hProcess.is_null() {
                WaitForSingleObject(info.hProcess, INFINITE);
                CloseHandle(info.hProcess);
            }
            Ok(())
        }
    }

    /// Starts this executable again as administrator (UAC asks) with
    /// `args`, without waiting.
    pub fn restart_elevated(args: &[&str]) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let params: Vec<String> = args.iter().map(|a| format!("\"{}\"", a.replace('"', ""))).collect();
        // SAFETY: SHELLEXECUTEINFOW is zeroed (valid: all-null pointers)
        // with cbSize set; the verb, file and parameter strings are
        // NUL-terminated and live until ShellExecuteExW returns; no process
        // handle is asked for (no SEE_MASK_NOCLOSEPROCESS), so none leaks.
        unsafe {
            let verb = wide("runas");
            let file = wide(&exe.to_string_lossy());
            let p = wide(&params.join(" "));
            let mut info: SHELLEXECUTEINFOW = zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.lpVerb = verb.as_ptr();
            info.lpFile = file.as_ptr();
            info.lpParameters = p.as_ptr();
            info.nShow = 1; // SW_SHOWNORMAL
            if ShellExecuteExW(&mut info) == 0 {
                return Err(if GetLastError() == 1223 {
                    ELEVATION_CANCELLED.into()
                } else {
                    format!("Could not restart as administrator (error {}).", GetLastError())
                });
            }
        }
        Ok(())
    }

    // ------------------------------------------------------- winget

    /// Runs winget without a window; (exit code, output).
    pub fn winget(args: &[&str]) -> Result<(i32, String), String> {
        winget_via_run(&SystemRunner, &jobs_dir()?, args)
    }

    // ------------------------------------------------- generic run

    /// Runs `program` without a window, its stdout and stderr written to
    /// the file `output`; passes each new line to `on_line` while it runs
    /// (and the last unfinished one at the end); a set `stop` ends it.
    pub fn run(program: &str, args: &[&str], output: &std::path::Path, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        use std::os::windows::process::CommandExt;
        if stop.load(Ordering::SeqCst) {
            return Ran::Stopped;
        }
        let (out, err) = match std::fs::File::create(output).and_then(|f| f.try_clone().map(|g| (f, g))) {
            Ok(files) => files,
            Err(e) => return Ran::NotStarted(format!("Could not create the output file: {e}")),
        };
        let mut child = match std::process::Command::new(program)
            .args(args)
            .creation_flags(0x0800_0000)
            .stdin(std::process::Stdio::null())
            .stdout(out)
            .stderr(err)
            .spawn()
        {
            Ok(c) => c,
            Err(e) => return Ran::NotStarted(format!("Could not start {program}: {e}")),
        };
        let mut lines = NewLines::new(output);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    lines.follow(on_line);
                    lines.finish(on_line);
                    return Ran::Exited(status.code().unwrap_or(-1));
                }
                Ok(None) => {}
                Err(e) => return Ran::NotStarted(e.to_string()),
            }
            if stop.load(Ordering::SeqCst) {
                let _ = child.kill();
                let _ = child.wait();
                return Ran::Stopped;
            }
            lines.follow(on_line);
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Reads the lines a running program adds to its output file.
    struct NewLines {
        file: Option<std::fs::File>,
        /// Bytes read so far.
        read: u64,
        /// The start of a line not finished yet.
        pending: Vec<u8>,
    }

    impl NewLines {
        fn new(path: &std::path::Path) -> Self {
            NewLines { file: std::fs::File::open(path).ok(), read: 0, pending: Vec::new() }
        }

        /// Passes every line finished since the last call to `on_line`.
        fn follow(&mut self, on_line: &mut dyn FnMut(&str)) {
            use std::io::{Read, Seek, SeekFrom};
            let Some(file) = self.file.as_mut() else { return };
            let mut buf = Vec::new();
            if file.seek(SeekFrom::Start(self.read)).is_err() || file.read_to_end(&mut buf).is_err() {
                return;
            }
            self.read += buf.len() as u64;
            self.pending.extend_from_slice(&buf);
            while let Some(at) = self.pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = self.pending.drain(..=at).collect();
                on_line(String::from_utf8_lossy(&line).trim_end_matches(['\n', '\r']));
            }
        }

        /// Passes the last line, when the output does not end with one.
        fn finish(&mut self, on_line: &mut dyn FnMut(&str)) {
            if !self.pending.is_empty() {
                on_line(String::from_utf8_lossy(&self.pending).trim_end_matches('\r'));
                self.pending.clear();
            }
        }
    }

    // ---------------------------------------------- start and wait

    /// Every running process id with its parent's.
    fn process_parents() -> HashMap<u32, u32> {
        let mut map = HashMap::new();
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
            if snap == INVALID_HANDLE_VALUE {
                return map;
            }
            let mut e: PROCESSENTRY32W = zeroed();
            e.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
            if Process32FirstW(snap, &mut e) != 0 {
                loop {
                    map.insert(e.th32ProcessID, e.th32ParentProcessID);
                    if Process32NextW(snap, &mut e) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
        }
        map
    }

    /// Starts a program (Windows asks for permission when it needs it) and
    /// waits for it and every process it starts: uninstallers often copy
    /// themselves to a temporary folder and exit at once. Errors may hold
    /// "%1" for the caller to replace with the app's name.
    pub fn run_and_wait(file: &str, params: &str, stop: &AtomicBool) -> Result<Waited, String> {
        let before: HashSet<u32> = process_parents().keys().copied().collect();
        let (wfile, wparams) = (wide(file), wide(params));
        unsafe {
            let mut info: SHELLEXECUTEINFOW = zeroed();
            info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
            info.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
            info.lpFile = wfile.as_ptr();
            info.lpParameters = wparams.as_ptr();
            info.nShow = 1;
            if ShellExecuteExW(&mut info) == 0 {
                return Err(if GetLastError() == 1223 {
                    "%1 was not uninstalled: the administrator prompt was dismissed.".into()
                } else {
                    format!("Windows could not start the uninstaller for %1 (error {}).", GetLastError())
                });
            }
            if info.hProcess.is_null() {
                return Ok(Waited::Finished(None));
            }
            let pid = GetProcessId(info.hProcess);
            // The handle keeps the pid from being reused while we watch.
            let h = info.hProcess;
            let started = Instant::now();
            let mut tracked: HashSet<u32> = HashSet::from([pid]);
            let mut root_done = false;
            loop {
                if stop.load(Ordering::Relaxed) {
                    CloseHandle(h);
                    return Ok(Waited::Stopped);
                }
                if started.elapsed() > Duration::from_secs(3600) {
                    CloseHandle(h);
                    return Ok(Waited::TimedOut);
                }
                if !root_done && WaitForSingleObject(h, 0) == WAIT_OBJECT_0 {
                    root_done = true;
                }
                let parents = process_parents();
                // Adopt new children of what we watch.
                let mut grew = true;
                while grew {
                    grew = false;
                    for (&p, &parent) in &parents {
                        if !tracked.contains(&p) && !before.contains(&p) && tracked.contains(&parent) {
                            tracked.insert(p);
                            grew = true;
                        }
                    }
                }
                tracked.retain(|p| parents.contains_key(p) && !(root_done && *p == pid));
                if root_done && tracked.is_empty() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            let mut code = 0u32;
            let read = GetExitCodeProcess(h, &mut code) != 0;
            CloseHandle(h);
            Ok(Waited::Finished(read.then_some(code as i32)))
        }
    }

    // ------------------------------------------------------ dialogs

    /// A Save (or Open) dialog with a Win32 filter, the extension a saved
    /// name gets when it has none, a suggested name (or path) and the
    /// folder it starts in ("" for Windows' choice); the chosen path, or
    /// None when cancelled.
    pub fn file_dialog(save: bool, filter: &str, extension: &str, default_name: &str, folder: &str) -> Option<String> {
        unsafe {
            let filter: Vec<u16> = filter.encode_utf16().collect();
            let mut file = vec![0u16; 1024];
            for (i, c) in default_name.encode_utf16().take(1000).enumerate() {
                file[i] = c;
            }
            let ext = wide(extension);
            let folder = wide(folder);
            let mut ofn: OPENFILENAMEW = zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            if folder.len() > 1 {
                ofn.lpstrInitialDir = folder.as_ptr();
            }
            // OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST, plus FILEMUSTEXIST or OVERWRITEPROMPT
            ofn.Flags = 0x8 | 0x800 | if save { 0x2 } else { 0x1000 };
            if save {
                ofn.lpstrDefExt = ext.as_ptr();
            }
            let ok = if save { GetSaveFileNameW(&mut ofn) } else { GetOpenFileNameW(&mut ofn) };
            (ok != 0).then(|| from_wide(&file))
        }
    }

    /// The shell's folder picker titled `title`, its OK button saying `ok`;
    /// the chosen folder, or None when cancelled.
    pub fn folder_dialog(title: &str, ok: &str) -> Option<String> {
        use windows_sys::Win32::Foundation::{HWND, LPARAM};
        use windows_sys::Win32::UI::Shell::{
            ILFree, SHBrowseForFolderW, SHGetPathFromIDListW, BFFM_INITIALIZED, BFFM_SETOKTEXT, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::SendMessageW;

        /// Sets the OK button's text once the dialog is up.
        unsafe extern "system" fn on_event(hwnd: HWND, msg: u32, _: LPARAM, data: LPARAM) -> i32 {
            if msg == BFFM_INITIALIZED {
                // SAFETY: `hwnd` is the live dialog calling us; `data` is the
                // NUL-terminated `ok` text, alive until the dialog returns.
                unsafe { SendMessageW(hwnd, BFFM_SETOKTEXT, 0, data) };
            }
            0
        }

        let title = wide(title);
        let ok = wide(ok);
        let mut name = vec![0u16; 260];
        // SAFETY: every pointer in `bi` (title, OK text, the 260-unit name
        // buffer MAX_PATH asks for) outlives the modal call; the item list
        // returned is freed once; `path` is far larger than the MAX_PATH
        // units SHGetPathFromIDListW may write.
        unsafe {
            let mut bi: BROWSEINFOW = zeroed();
            bi.pszDisplayName = name.as_mut_ptr();
            bi.lpszTitle = title.as_ptr();
            bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
            bi.lpfn = Some(on_event);
            bi.lParam = ok.as_ptr() as LPARAM;
            let pidl = SHBrowseForFolderW(&bi);
            if pidl.is_null() {
                return None;
            }
            let mut path = vec![0u16; 32_768];
            let got = SHGetPathFromIDListW(pidl, path.as_mut_ptr());
            ILFree(pidl);
            (got != 0).then(|| from_wide(&path)).filter(|p| !p.is_empty())
        }
    }

    /// An Open dialog titled `title` with a Win32 filter, opened in
    /// `initial_dir` (Windows' choice when empty): the chosen file, None
    /// when cancelled, Err when the dialog itself failed.
    pub fn open_dialog(title: &str, filter: &str, initial_dir: &str) -> Result<Option<String>, ()> {
        use windows_sys::Win32::UI::Controls::Dialogs::CommDlgExtendedError;
        let filter: Vec<u16> = filter.encode_utf16().collect();
        let title = wide(title);
        let dir = wide(initial_dir);
        let mut file = vec![0u16; 1024];
        // SAFETY: the filter, title, folder and file buffer outlive the
        // modal call, and nMaxFile is the buffer's length.
        unsafe {
            let mut ofn: OPENFILENAMEW = zeroed();
            ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
            ofn.lpstrFilter = filter.as_ptr();
            ofn.lpstrTitle = title.as_ptr();
            if !initial_dir.is_empty() {
                ofn.lpstrInitialDir = dir.as_ptr();
            }
            ofn.lpstrFile = file.as_mut_ptr();
            ofn.nMaxFile = file.len() as u32;
            // OFN_NOCHANGEDIR | OFN_PATHMUSTEXIST | OFN_FILEMUSTEXIST
            ofn.Flags = 0x8 | 0x800 | 0x1000;
            if GetOpenFileNameW(&mut ofn) != 0 {
                return Ok(Some(from_wide(&file)));
            }
            // Zero means the user cancelled; anything else is a failure.
            if CommDlgExtendedError() == 0 {
                Ok(None)
            } else {
                Err(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn the_test_executor_records_requests_and_launches_nothing() {
        let (ctx, runner) = AddonContext::recording();
        assert!(ctx.winget(&["list", "--disable-interactivity"]).is_err());
        assert!(ctx.powershell("param([string]$Out)", &[("Mode", "scan")], true).is_err());
        assert!(ctx.run_and_wait("MsiExec.exe", "/X{1234}", &AtomicBool::new(false)).is_err());
        assert_eq!(
            runner.launches(),
            vec![
                Launch::Winget(vec!["list".into(), "--disable-interactivity".into()]),
                Launch::PowerShell { script: "param([string]$Out)".into(), args: vec![("Mode".into(), "scan".into())], elevated: true },
                Launch::Program { file: "MsiExec.exe".into(), params: "/X{1234}".into() },
            ]
        );
    }

    #[test]
    fn a_prepared_reply_answers_the_winget_request_with_its_argument() {
        let (ctx, runner) = AddonContext::recording();
        runner.prepare(When::Arg("upgrade".into()), 0, "Successfully installed");
        assert_eq!(ctx.winget(&["list"]), Err("Not launched: test executor.".into()));
        assert_eq!(ctx.winget(&["upgrade", "--id", "Git.Git"]), Ok((0, "Successfully installed".into())));
        assert_eq!(ctx.winget(&["upgrade", "--id", "Git.Git"]), Err("Not launched: test executor.".into()), "a reply answers once");
    }

    #[test]
    fn a_prepared_reply_answers_the_request_in_its_position() {
        let (ctx, runner) = AddonContext::recording();
        runner.prepare(When::Nth(1), 0x8A15002B_u32 as i32, "No applicable update found.");
        assert!(ctx.winget(&["upgrade", "--id", "A"]).is_err());
        assert_eq!(ctx.winget(&["upgrade", "--id", "B"]), Ok((0x8A15002B_u32 as i32, "No applicable update found.".into())));
    }

    /// A file in a fresh folder outside the repository, for run output.
    fn scratch_output(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("upm-run-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("output.txt")
    }

    #[test]
    fn a_run_with_a_prepared_reply_writes_its_output_and_reports_its_lines() {
        let (ctx, runner) = AddonContext::recording();
        let output = scratch_output("prepared");
        runner.prepare(When::Arg("install".into()), 0, "Found Git\nSuccessfully installed");
        let mut lines = Vec::new();
        let ran = ctx.run("winget", &["install", "--id", "Git.Git"], &output, &AtomicBool::new(false), &mut |l| lines.push(l.to_string()));
        let written = std::fs::read_to_string(&output).unwrap();
        let _ = std::fs::remove_dir_all(output.parent().unwrap());
        assert_eq!(ran, Ran::Exited(0));
        assert_eq!(lines, ["Found Git", "Successfully installed"]);
        assert_eq!(written, "Found Git\nSuccessfully installed");
        assert_eq!(
            runner.launches(),
            vec![Launch::Run { program: "winget".into(), args: vec!["install".into(), "--id".into(), "Git.Git".into()], output: output.clone() }]
        );
    }

    #[test]
    fn a_run_without_a_prepared_reply_is_not_started_with_the_usual_error() {
        let (ctx, _) = AddonContext::recording();
        let ran = ctx.run("winget", &["list"], &scratch_output("none"), &AtomicBool::new(false), &mut |_| {});
        assert_eq!(ran, Ran::NotStarted("Not launched: test executor.".into()));
    }

    #[test]
    fn stopping_a_held_run_ends_it_as_stopped() {
        let (ctx, runner) = AddonContext::recording();
        runner.hold();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = {
            let (ctx, stop) = (ctx.clone(), stop.clone());
            std::thread::spawn(move || ctx.run("winget", &["install", "--id", "A"], &scratch_output("held"), &stop, &mut |_| {}))
        };
        runner.wait_for(1);
        stop.store(true, Ordering::SeqCst);
        assert_eq!(worker.join().unwrap(), Ran::Stopped);
        assert_eq!(runner.stopped(), 1);
        runner.release();
    }

    #[test]
    fn winget_goes_through_the_generic_run_and_leaves_no_output_file() {
        let runner = RecordingRunner::default();
        let dir = scratch_output("winget").parent().unwrap().to_path_buf();
        runner.prepare(When::Arg("list".into()), 0, "Name  Id\nGit   Git.Git");
        let result = winget_via_run(&runner, &dir, &["list", "--disable-interactivity"]);
        let left = std::fs::read_dir(&dir).unwrap().count();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result, Ok((0, "Name  Id\nGit   Git.Git".into())));
        assert!(matches!(&runner.launches()[..], [Launch::Run { program, args, .. }] if program == "winget" && args == &["list", "--disable-interactivity"]));
        assert_eq!(left, 0, "the output file is deleted");
    }

    #[test]
    fn winget_that_cannot_start_says_it_is_not_installed() {
        let runner = RecordingRunner::default();
        let dir = scratch_output("missing").parent().unwrap().to_path_buf();
        let result = winget_via_run(&runner, &dir, &["list"]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(result, Err(WINGET_MISSING.into()));
    }

    #[test]
    fn the_test_context_is_not_admin_and_opens_no_dialogs() {
        let (ctx, _) = AddonContext::recording();
        assert!(!ctx.is_admin());
        assert_eq!(ctx.pick_save_as(&CSV_FILE, "apps.csv"), None);
        assert_eq!(ctx.pick_open_file(&[ANY_FILE], None), None);
    }

    #[test]
    fn the_file_dialog_filter_lists_each_type_by_extension() {
        assert_eq!(file_filter(&[CSV_FILE]), "CSV (*.csv)\0*.csv\0\0");
        assert_eq!(file_filter(&[JSON_FILE, ANY_FILE]), "JSON (*.json)\0*.json\0All files (*.*)\0*.*\0\0");
    }

    #[test]
    fn csv_quotes_commas() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}

#[cfg(all(test, windows))]
mod script_tests {
    use super::imp::{run_hidden, run_in};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    #[test]
    fn a_stopped_script_leaves_no_files_behind() {
        let dir = std::env::temp_dir().join(format!("upm-jobs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut seen = 0;
        let result = run_in(&dir, "Start-Sleep 60", &[("Mode", "x")], |params| {
            seen = std::fs::read_dir(&dir).unwrap().count();
            assert!(params.contains("-Mode \"x\""));
            Err(crate::platform::STOPPED.into())
        });
        let left = std::fs::read_dir(&dir).unwrap().count();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(seen, 1, "the script was written while it ran");
        assert_eq!(result.unwrap_err(), crate::platform::STOPPED);
        assert_eq!(left, 0, "the script and its result are deleted");
    }

    #[test]
    fn stopping_ends_a_running_powershell_at_once() {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let result = run_hidden("-NoProfile -NonInteractive -Command Start-Sleep -Seconds 60", &stop);
        assert_eq!(result.unwrap_err(), crate::platform::STOPPED);
        assert!(started.elapsed() < Duration::from_secs(15), "PowerShell was ended, not waited for");
    }

    /// A fresh folder outside the repository for one run's output.
    fn run_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("upm-imp-run-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_run_writes_its_output_to_the_file_and_reports_each_line() {
        let dir = run_dir("lines");
        let output = dir.join("out.txt");
        let mut lines = Vec::new();
        let ran = super::imp::run("cmd.exe", &["/c", "echo first&& echo second&& exit 3"], &output, &AtomicBool::new(false), &mut |l| lines.push(l.trim_end().to_string()));
        let written = std::fs::read_to_string(&output).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ran, super::Ran::Exited(3));
        assert_eq!(lines, ["first", "second"]);
        assert!(written.contains("first") && written.contains("second"));
    }

    #[test]
    fn a_run_passes_each_argument_as_is() {
        let dir = run_dir("args");
        let output = dir.join("out.txt");
        let script = dir.join("count.ps1");
        std::fs::write(&script, "Write-Output $args.Count").unwrap();
        let script = script.to_string_lossy().into_owned();
        let ran = super::imp::run("powershell.exe", &["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", &script, "a b", "c"], &output, &AtomicBool::new(false), &mut |_| {});
        let written = std::fs::read_to_string(&output).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ran, super::Ran::Exited(0));
        assert_eq!(written.trim(), "2", "\"a b\" stays one argument");
    }

    #[test]
    fn stopping_ends_a_running_program_at_once() {
        let dir = run_dir("stop");
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let ran = super::imp::run("powershell.exe", &["-NoProfile", "-Command", "Start-Sleep -Seconds 60"], &dir.join("out.txt"), &stop, &mut |_| {});
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(ran, super::Ran::Stopped);
        assert!(started.elapsed() < Duration::from_secs(15), "the program was ended, not waited for");
    }

    #[test]
    fn a_program_that_cannot_start_is_not_started_with_a_reason() {
        let dir = run_dir("missing");
        let ran = super::imp::run("no-such-program-upm.exe", &[], &dir.join("out.txt"), &AtomicBool::new(false), &mut |_| {});
        let _ = std::fs::remove_dir_all(&dir);
        assert!(matches!(ran, super::Ran::NotStarted(ref why) if why.contains("no-such-program-upm.exe")), "{ran:?}");
    }

    #[test]
    fn a_stop_set_before_starting_launches_nothing() {
        let result = run_hidden("-NoProfile -Command exit", &AtomicBool::new(true));
        assert_eq!(result.unwrap_err(), crate::platform::STOPPED);
    }
}
