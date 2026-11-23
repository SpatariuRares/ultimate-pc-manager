//! Drivers: what is installed, whether a newer version exists, and installing it.
//!
//! - Device drivers: one row per device, as Device Manager shows them
//!   (configuration manager properties: version, date, provider, INF, …).
//! - Kernel drivers: the kernel driver services,
//!   with file version and vendor, and the size and load order of the loaded
//!   image from the kernel's module list.
//! - Updates: Windows Update's driver catalogue, searched through its COM API
//!   in a PowerShell child, matched to devices by hardware ID.
//! - Installing (Windows Update or an INF file through pnputil), scanning
//!   for hardware changes and stopping a driver need administrator rights:
//!   they run in an elevated PowerShell, so Windows asks through UAC.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::addons::plural;
use crate::availability::{unavailable_text, Feature};
use crate::platform::install_queue::{Request, Work};
use crate::platform::series::{Run, Series};
use crate::platform::{AddonContext, Generation};

use super::ListingReader;

#[derive(Clone, Debug, Default)]
pub struct DeviceDriver {
    pub instance_id: String,
    pub name: String,
    pub class: String,
    pub manufacturer: String,
    pub provider: String,
    pub version: String,
    /// yyyy-mm-dd
    pub date: String,
    /// "oem12.inf" or "usb.inf"
    pub inf: String,
    pub service: String,
    pub hardware_ids: Vec<String>,
    pub compatible_ids: Vec<String>,
    /// Plugged in now (Device Manager hides the others by default).
    pub present: bool,
    pub started: bool,
    /// Configuration manager problem code; 0 = working.
    pub problem: u32,
}

impl DeviceDriver {
    pub fn third_party(&self) -> bool {
        !self.provider.is_empty() && !self.provider.to_lowercase().starts_with("microsoft")
    }
    pub fn status(&self) -> String {
        if !self.present {
            "Not connected".into()
        } else if self.problem == 22 {
            "Disabled".into()
        } else if self.problem != 0 {
            format!("Problem (code {})", self.problem)
        } else if self.started {
            "Working".into()
        } else {
            "Not started".into()
        }
    }
    pub fn inf_path(&self) -> String {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        format!(r"{windir}\INF\{}", self.inf)
    }
    /// True when Windows Update's hardware ID belongs to this device.
    pub fn matches(&self, hardware_id: &str) -> bool {
        let h = hardware_id.to_lowercase();
        !h.is_empty() && self.hardware_ids.iter().chain(self.compatible_ids.iter()).any(|i| i.to_lowercase() == h)
    }
}

#[derive(Clone, Debug, Default)]
pub struct KernelDriver {
    pub service: String,
    pub display: String,
    pub running: bool,
    pub start_type: &'static str,
    /// 0 boot, 1 system, 2 automatic, 3 manual, 4 disabled; `None` when Windows did not return the configuration.
    pub start_code: Option<u32>,
    /// The SCM state code (1 stopped ... 7 paused); `None` when it was not read.
    pub state_code: Option<u32>,
    pub kind: KernelKind,
    pub path: String,
    pub version: String,
    pub vendor: String,
    pub group: String,
    pub image_size: Option<u64>,
    pub load_index: Option<u32>,
    /// The list of loaded kernel images could not be read, so `image_size`
    /// and `load_index` are unknown for this row (not absent).
    pub modules_unread: bool,
    /// Services this driver depends on (the configuration's dependency list).
    pub depends_services: Vec<String>,
    /// Load order groups this driver depends on.
    pub depends_groups: Vec<String>,
}

/// One image of the loaded-module list: its full path, size and load order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadedModule {
    pub path: String,
    pub size: u64,
    pub index: u32,
}

/// A path as compared: lowercase, without the `\??\` prefix, `\SystemRoot`
/// spelled out as the Windows folder.
pub fn normal_path(path: &str, windir: &str) -> String {
    let p = path.trim().to_lowercase().replace('/', "\\");
    let p = p.strip_prefix(r"\??\").unwrap_or(&p);
    match p.strip_prefix(r"\systemroot") {
        Some(rest) => format!("{}{rest}", windir.to_lowercase()),
        None => p.to_string(),
    }
}

/// The loaded module of a driver image: matched by full path; by file name
/// only when the image path has no folder and one module has that name, so
/// two files with one name in different folders never share a size.
pub fn find_module<'a>(modules: &'a [LoadedModule], image_path: &str, windir: &str) -> Option<&'a LoadedModule> {
    let want = normal_path(image_path, windir);
    if let Some(m) = modules.iter().find(|m| normal_path(&m.path, windir) == want) {
        return Some(m);
    }
    if want.contains('\\') {
        return None;
    }
    let mut same = modules.iter().filter(|m| normal_path(&m.path, windir).rsplit('\\').next() == Some(want.as_str()));
    match (same.next(), same.next()) {
        (Some(m), None) => Some(m),
        _ => None,
    }
}


/// The SCM state shown for a state code; `None` when the state was not read.
pub fn service_state_name(code: Option<u32>) -> Option<&'static str> {
    match code? {
        1 => Some("Stopped"),
        2 => Some("Start pending"),
        3 => Some("Stop pending"),
        4 => Some("Running"),
        5 => Some("Continue pending"),
        6 => Some("Pause pending"),
        7 => Some("Paused"),
        _ => None,
    }
}

/// The kind of a driver service from its service type bits.
pub fn kernel_kind_from_type(service_type: u32) -> KernelKind {
    match service_type {
        t if t & 2 != 0 => KernelKind::FileSystem,
        t if t & 1 != 0 => KernelKind::Kernel,
        t if t & 8 != 0 => KernelKind::Recognizer,
        t if t & 4 != 0 => KernelKind::Adapter,
        _ => KernelKind::Kernel,
    }
}

/// Splits a service's dependency list (UTF-16 strings, each ended by a nul, the list
/// ended by an empty string) into (services, load order groups). A group
/// is written with the prefix '+' (SC_GROUP_IDENTIFIER).
pub fn parse_dependencies(units: &[u16]) -> (Vec<String>, Vec<String>) {
    let (mut services, mut groups) = (Vec::new(), Vec::new());
    for entry in units.split(|&u| u == 0) {
        if entry.is_empty() {
            break;
        }
        let text = String::from_utf16_lossy(entry);
        match text.strip_prefix('+') {
            Some(group) => groups.push(group.to_string()),
            None => services.push(text),
        }
    }
    (services, groups)
}

/// The start type shown for a service start code; "Unknown" when the configuration was not read.
pub fn start_type_name(code: Option<u32>) -> &'static str {
    match code {
        Some(0) => "Boot",
        Some(1) => "System",
        Some(2) => "Automatic",
        Some(3) => "Manual",
        Some(4) => "Disabled",
        _ => "Unknown",
    }
}

impl KernelDriver {
    /// The state label: the SCM state when read (pending states included), else Running/Stopped.
    pub fn state_label(&self) -> &'static str {
        service_state_name(self.state_code).unwrap_or(if self.running { "Running" } else { "Stopped" })
    }
    /// The services then the groups this driver depends on, comma-separated; empty when none.
    pub fn depends_on_text(&self) -> String {
        let groups = self.depends_groups.iter().map(|g| format!("{g} (group)"));
        self.depends_services.iter().cloned().chain(groups).collect::<Vec<_>>().join(", ")
    }
    /// Loaded: the service is not Stopped, so the pending states count.
    /// Without a read state, only a running service is loaded.
    pub fn is_loaded(&self) -> bool {
        match self.state_code {
            Some(code) => code != 1,
            None => self.running,
        }
    }
    pub fn third_party(&self) -> bool {
        !self.vendor.is_empty() && !self.vendor.to_lowercase().starts_with("microsoft")
    }
    /// Whether "Unload" is offered and, if not, why.
    pub fn unload_note(&self, admin: bool) -> (bool, &'static str) {
        if !self.running {
            (false, "Not loaded.")
        } else if self.start_code.is_none() {
            (false, "Not offered: its configuration could not be read.")
        } else if self.start_code <= Some(1) {
            (false, "Not offered: Windows loaded it at boot, and the kernel refuses to stop a driver it started itself.")
        } else if !admin {
            (true, "Needs administrator rights (Windows will ask); most drivers refuse the request while the system runs.")
        } else {
            (true, "Offered: most drivers refuse the request while the system runs.")
        }
    }
}

/// The five scopes of the kernel module list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KernelScope {
    #[default]
    All,
    Loaded,
    NotLoaded,
    ThirdParty,
    FileSystem,
}

impl KernelScope {
    /// In display order.
    pub const ALL: [KernelScope; 5] = [Self::All, Self::Loaded, Self::NotLoaded, Self::ThirdParty, Self::FileSystem];

    /// The scope saved by older versions as an index into
    /// [`KernelScope::ALL`]; All when unknown.
    fn from_index(i: u64) -> Self {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i)).copied().unwrap_or_default()
    }

    /// Its name in the settings and on the command line (`--drivers-scope`).
    pub fn name(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::Loaded => "loaded",
            Self::NotLoaded => "not-loaded",
            Self::ThirdParty => "third-party",
            Self::FileSystem => "file-system",
        }
    }

    /// The scope named `name` (case does not matter); None when unknown.
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Loaded => "Loaded",
            Self::NotLoaded => "Not loaded",
            Self::ThirdParty => "Third-party",
            Self::FileSystem => "File system",
        }
    }

    pub fn matches(self, d: &KernelDriver) -> bool {
        match self {
            Self::All => true,
            Self::Loaded => d.is_loaded(),
            Self::NotLoaded => !d.is_loaded(),
            Self::ThirdParty => d.third_party(),
            Self::FileSystem => d.kind == KernelKind::FileSystem,
        }
    }

    /// How many drivers of the list fall in this scope.
    pub fn count(self, list: &[KernelDriver]) -> usize {
        list.iter().filter(|d| self.matches(d)).count()
    }
}

/// Saved by its [`KernelScope::name`].
impl serde::Serialize for KernelScope {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// Read by its name, or by the index older versions saved; All when unknown.
impl<'de> serde::Deserialize<'de> for KernelScope {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Saved {
            Index(u64),
            Name(String),
        }
        Ok(match Saved::deserialize(d)? {
            Saved::Index(i) => Self::from_index(i),
            Saved::Name(n) => Self::parse(&n).unwrap_or_default(),
        })
    }
}

/// The type of a kernel driver service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum KernelKind {
    #[default]
    Kernel,
    FileSystem,
    Recognizer,
    Adapter,
}

impl KernelKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Kernel => "Kernel",
            Self::FileSystem => "File system",
            Self::Recognizer => "Recognizer",
            Self::Adapter => "Adapter",
        }
    }
}

/// The four figures above the kernel module list.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KernelFigures {
    /// Registered with Windows.
    pub drivers: usize,
    /// Linked into the kernel now.
    pub loaded: usize,
    /// Not shipped by Microsoft.
    pub third_party: usize,
    /// Bytes held by the loaded driver images, each file once; None
    /// ("an unknown amount") when the loaded-module list could not be read.
    pub memory: Option<u64>,
}

impl KernelFigures {
    /// The figures of these rows (the ones matching the text filter,
    /// whatever the scope).
    pub fn of<'a>(rows: impl IntoIterator<Item = &'a KernelDriver>) -> Self {
        let mut f = Self { memory: Some(0), ..Self::default() };
        let mut images = std::collections::HashSet::new();
        for d in rows {
            f.drivers += 1;
            if KernelScope::Loaded.matches(d) {
                f.loaded += 1;
                if d.modules_unread {
                    f.memory = None;
                } else if let (Some(size), Some(total)) = (d.image_size, f.memory.as_mut()) {
                    let file = if d.path.is_empty() { d.service.to_lowercase() } else { d.path.to_lowercase() };
                    if images.insert(file) {
                        *total += size;
                    }
                }
            }
            f.third_party += KernelScope::ThirdParty.matches(d) as usize;
        }
        f
    }
}

/// A driver update Windows Update offers for this PC.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "PascalCase", default)]
pub struct DriverUpdate {
    pub id: String,
    pub title: String,
    pub hardware_id: String,
    pub model: String,
    pub provider: String,
    pub class: String,
    pub date: String,
    pub size: f64,
    /// Optional update (Settings → Windows Update → Optional updates).
    pub browse_only: bool,
    pub downloaded: bool,
    /// Version read from the title, e.g. "ASIX Net Driver Update (4.20.2.0)".
    #[serde(skip)]
    pub version: String,
    /// Instance ID of the device it is for, when one matches.
    #[serde(skip)]
    pub device: Option<String>,
    /// Compared with the installed driver: Some(true) newer, Some(false)
    /// the same or older (Windows Update can offer an older one), None when
    /// there is nothing to compare with.
    #[serde(skip)]
    pub newer: Option<bool>,
}

impl DriverUpdate {
    /// Worth installing: newer than what is installed, or not comparable.
    pub fn worth_installing(&self) -> bool {
        self.newer != Some(false)
    }
}

/// The outcome of a check the user made by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandCheck {
    /// How many offered updates were worth installing.
    pub newer: usize,
    /// When it ended ("14:03").
    pub at: String,
}

/// Everything the Drivers page shows, filled by background threads.
#[derive(Default)]
pub struct State {
    /// The lists are shared snapshots: the page takes a handle, never a
    /// copy, and a new read replaces the whole snapshot.
    pub devices: Arc<[DeviceDriver]>,
    pub kernel: Arc<[KernelDriver]>,
    pub listed_at: String,
    pub listing: bool,
    /// Why the last read of the devices failed (then `devices` is empty).
    pub devices_error: Option<String>,
    /// Why the last read of the driver services failed (then `kernel` is empty).
    pub kernel_error: Option<String>,
    /// None until the first check.
    pub updates: Option<Arc<[DriverUpdate]>>,
    pub checked_at: String,
    /// The last check the user made by pressing Check in this session,
    /// shown on Summary (a check made when the page opened does not count).
    pub hand_check: Option<HandCheck>,
    /// What is running now ("Checking Windows Update…").
    pub busy: Option<String>,
    /// Last outcome; the flag marks an error.
    pub message: Option<(String, bool)>,
    /// Output of the last pnputil / sc run.
    pub output: String,
    pub reboot_required: bool,
    /// The last read of the security settings; `None` until the first
    /// listing has read them.
    pub security: Option<SecurityReading>,
}

#[derive(Clone)]
pub struct Hub {
    st: Arc<Mutex<State>>,
    /// Every PowerShell run goes through the platform's executor.
    ctx: AddonContext,
    /// Where the devices and kernel drivers are read from.
    reader: Arc<dyn ListingReader>,
    /// Runs the listing, the search and the installs one at a time.
    /// Stopped once by [`Hub::stop`]: from then on its work publishes
    /// nothing and no new work starts; driver installs are not cut short.
    series: Series<State>,
    /// Set by [`Hub::stop`]: ends a running Windows Update search's PowerShell.
    halt: Arc<AtomicBool>,
}

impl Hub {
    pub fn new(ctx: AddonContext, reader: Arc<dyn ListingReader>) -> Self {
        let st = Arc::new(Mutex::new(State::default()));
        let series = Series::new(&st, &Generation::default(), |s: &mut State| &mut s.busy);
        Hub { st, ctx, reader, series, halt: Default::default() }
    }

    pub fn lock(&self) -> MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The state to publish into, unless the hub was stopped (checked under
    /// the lock, so nothing lands after `stop`).
    fn live(&self) -> Option<MutexGuard<'_, State>> {
        let st = self.lock();
        (!self.series.stopped()).then_some(st)
    }

    /// The state a driver install reports into: also after [`Hub::stop`],
    /// so the page that stays open while the add-on waits to turn off can
    /// follow the install to its end.
    fn follow(&self) -> MutexGuard<'_, State> {
        self.lock()
    }

    /// The state an operation publishes into: [`Hub::follow`] for one that
    /// is not cut short, [`Hub::live`] otherwise.
    fn outcome(&self, uninterruptible: bool) -> Option<MutexGuard<'_, State>> {
        if uninterruptible {
            Some(self.follow())
        } else {
            self.live()
        }
    }

    /// The add-on was turned off: every later result of what can be
    /// stopped is dropped and no new work starts. With nothing else running
    /// the state is forgotten; a running driver install goes on (see
    /// [`Hub::running`]) and keeps reporting into the state.
    pub fn stop(&self) {
        self.series.stop(|st| st.listing = false);
        self.halt.store(true, Ordering::SeqCst);
    }

    /// A driver install (Windows Update or an INF package) is running.
    pub fn running(&self) -> bool {
        self.series.running()
    }

    /// Runs `job` in a thread unless the hub was stopped or another job
    /// holds the busy text (see [`Series::start`]). An uninterruptible job
    /// counts in [`Hub::running`] until it ends.
    fn spawn(&self, busy: Option<String>, uninterruptible: bool, job: impl FnOnce(&Hub, &Run<State>) + Send + 'static) -> bool {
        let hub = self.clone();
        self.series.start(busy, uninterruptible, move |run| job(&hub, run))
    }

    /// Re-reads devices and kernel drivers.
    pub fn refresh(&self) {
        {
            let Some(mut st) = self.live() else { return };
            if st.listing {
                return;
            }
            // A system without drivers says so; it never reads as an empty
            // list. Each tab shows its own list's error, so both get it.
            if let Some(why) = unavailable_text(Feature::DeviceSetupApi) {
                publish_listing(&mut st, Err(why.clone()), Err(why));
                st.listed_at = crate::platform::clock_now();
                return;
            }
            st.listing = true;
        }
        self.spawn(None, false, |hub, _| hub.refresh_now());
    }

    fn refresh_now(&self) {
        if self.live().is_none() {
            return;
        }
        let devices = self.reader.devices();
        let kernel = self.reader.kernel();
        // Read-only and with normal rights; skipped once the hub was stopped.
        if self.live().is_none() {
            return;
        }
        let security = self.reader.security();
        let Some(mut st) = self.live() else { return };
        publish_listing(&mut st, devices, kernel);
        st.security = Some(security);
        st.listed_at = crate::platform::clock_now();
    }

    /// Searches Windows Update for driver updates (takes 10–60 s).
    /// The user pressed Check: its outcome can be shown on Summary.
    pub fn check_updates(&self) {
        self.check(true);
    }

    /// The search "Check Windows Update when the page opens" asks for:
    /// its outcome stays on the page.
    pub fn check_updates_on_open(&self) {
        self.check(false);
    }

    fn check(&self, by_hand: bool) {
        self.spawn(Some("Checking Windows Update for driver updates\u{2026}".into()), false, move |hub, run| {
            // Turning the add-on off ends the search's PowerShell at once.
            let result = hub.ctx.powershell_until(SEARCH_SCRIPT, &[], &hub.halt);
            let Some(mut st) = hub.live() else { return };
            run.release(&mut st);
            publish_check(&mut st, result.and_then(|v| parse_search(&v)), by_hand, &crate::platform::clock_now());
        });
    }

    /// Downloads and installs the given Windows Update driver updates.
    pub fn install(&self, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let busy = format!("Installing {} \u{2014} confirm the Windows prompt\u{2026}", plural(ids.len(), "driver update"));
        self.spawn(Some(busy), true, move |hub, run| {
            let items = ids.iter().map(|id| ("windows-update".to_string(), id.clone())).collect();
            let Some(turn) = run.queue(hub.ctx.install_queue(), queue_request(items)) else { return };
            let joined = ids.join(",");
            let outcome = install_outcome(hub.ctx.powershell(INSTALL_SCRIPT, &[("Ids", joined.as_str())], true));
            drop(turn);
            let done = outcome.installed;
            {
                let mut st = hub.follow();
                run.release(&mut st);
                if outcome.reboot {
                    st.reboot_required = true;
                }
                st.message = Some(outcome.message);
                if let Some(u) = &st.updates {
                    let left: Vec<DriverUpdate> = u.iter().filter(|x| !done.contains(&x.id)).cloned().collect();
                    st.updates = Some(left.into());
                }
            }
            if !done.is_empty() {
                hub.refresh_now();
            }
        });
    }

    /// Adds a driver package and installs it on matching devices
    /// (pnputil /add-driver … /install). A folder installs every INF in it.
    pub fn install_inf(&self, path: String) {
        self.pnputil("inf", path, true, "Installing the driver package \u{2014} confirm the Windows prompt\u{2026}");
    }

    /// Asks Windows to look for new or removed hardware (pnputil /scan-devices).
    pub fn scan(&self) {
        self.pnputil("scan", String::new(), false, "Scanning for hardware changes \u{2014} confirm the Windows prompt\u{2026}");
    }

    /// Stops (unloads) a kernel driver service (sc stop).
    pub fn unload(&self, service: String) {
        self.pnputil("stop", service, false, "Asking the driver to unload \u{2014} confirm the Windows prompt\u{2026}");
    }

    /// Installing a package is uninterruptible; scanning and unloading are
    /// not (their outcome is dropped when the add-on is turned off).
    fn pnputil(&self, mode: &'static str, arg: String, uninterruptible: bool, busy: &str) {
        self.spawn(Some(busy.into()), uninterruptible, move |hub, run| {
            // Only adding a driver package installs something.
            let turn = match mode {
                "inf" => match run.queue(hub.ctx.install_queue(), queue_request(vec![("inf".to_string(), arg.clone())])) {
                    Some(turn) => Some(turn),
                    None => return,
                },
                _ => None,
            };
            let outcome = tool_outcome(mode, hub.ctx.powershell(TOOL_SCRIPT, &[("Mode", mode), ("Arg", arg.as_str())], true));
            drop(turn);
            {
                let Some(mut st) = hub.outcome(uninterruptible) else { return };
                run.release(&mut st);
                if outcome.reboot {
                    st.reboot_required = true;
                }
                // A dismissed prompt ran nothing: the last output stays.
                if !outcome.cancelled {
                    st.output = outcome.output;
                }
                st.message = Some(outcome.message);
            }
            hub.refresh_now();
        });
    }
}

/// This add-on's install request for the app's install queue.
fn queue_request(items: Vec<(String, String)>) -> Request {
    Request { addon: super::ID, name: super::NAME, work: Work::Install, items }
}

/// Shown when the UAC prompt was dismissed: distinct from a failure.
const CANCELLED_TEXT: &str = "Cancelled at the Windows prompt: nothing was changed.";

/// The message for an elevated script that did not run: a dismissed UAC
/// prompt (not an error) or a launch failure.
fn not_run(e: String) -> (String, bool) {
    if e == crate::platform::ELEVATION_CANCELLED {
        (CANCELLED_TEXT.into(), false)
    } else {
        (e, true)
    }
}

/// What a Windows Update install reported.
#[derive(Debug)]
pub struct InstallOutcome {
    /// The page's message; the flag marks an error.
    pub message: (String, bool),
    /// Ids of the updates now installed.
    pub installed: Vec<String>,
    pub reboot: bool,
}

/// Reads the install script's result. Per update, Windows Update's result
/// code: 2 succeeded, 3 succeeded with errors, 4 failed, 5 aborted.
pub fn install_outcome(result: Result<serde_json::Value, String>) -> InstallOutcome {
    let mut out = InstallOutcome { message: (String::new(), false), installed: Vec::new(), reboot: false };
    let v = match result {
        Ok(v) => v,
        Err(e) => {
            out.message = not_run(e);
            return out;
        }
    };
    if v["ok"].as_bool() != Some(true) {
        out.message = (format!("Install failed: {}", v["error"].as_str().unwrap_or("unknown error")), true);
        return out;
    }
    let mut with_errors = 0;
    let mut failed = Vec::new();
    for it in v["items"].as_array().cloned().unwrap_or_default() {
        let title = it["Title"].as_str().unwrap_or("?");
        match it["Code"].as_i64().unwrap_or(0) {
            code @ (2 | 3) => {
                with_errors += usize::from(code == 3);
                out.installed.push(it["Id"].as_str().unwrap_or_default().to_string());
            }
            5 => failed.push(format!("{title} (aborted)")),
            _ => failed.push(format!("{title} (0x{:08x})", it["HResult"].as_i64().unwrap_or(0) as u32)),
        }
    }
    out.reboot = v["reboot"].as_bool() == Some(true);
    let ok = out.installed.len();
    out.message = if !failed.is_empty() {
        (format!("Installed {ok}; failed: {}", failed.join(", ")), true)
    } else if with_errors > 0 {
        (format!("Installed {} ({with_errors} with errors).", plural(ok, "driver update")), false)
    } else {
        (format!("Installed {}.", plural(ok, "driver update")), false)
    };
    out
}

/// What a pnputil / sc run reported.
#[derive(Debug)]
pub struct ToolOutcome {
    /// The page's message; the flag marks an error.
    pub message: (String, bool),
    /// The tool's text output (empty when it did not run).
    pub output: String,
    pub reboot: bool,
    /// The UAC prompt was dismissed: nothing ran.
    pub cancelled: bool,
}

/// Reads the tool script's result for `mode` ("inf", "scan" or "stop").
/// pnputil: 0 done, 3010 done but restart needed, 259 nothing newer to
/// install. sc: 0 asked, 1062 not started, 1052 refused, 1051 has dependents.
pub fn tool_outcome(mode: &str, result: Result<serde_json::Value, String>) -> ToolOutcome {
    let v = match result {
        Ok(v) => v,
        Err(e) => {
            let cancelled = e == crate::platform::ELEVATION_CANCELLED;
            return ToolOutcome { message: not_run(e), output: String::new(), reboot: false, cancelled };
        }
    };
    if v["ok"].as_bool() != Some(true) {
        let message = (v["error"].as_str().unwrap_or("unknown error").to_string(), true);
        return ToolOutcome { message, output: String::new(), reboot: false, cancelled: false };
    }
    let code = v["code"].as_i64().unwrap_or(0);
    let message: (String, bool) = match (mode, code) {
        (_, 3010) => ("Done. Restart Windows to finish.".into(), false),
        ("inf", 0) => ("Driver package installed.".into(), false),
        ("inf", 259) => ("The package was added, but no device needed it (its driver is not newer or does not match).".into(), false),
        ("scan", 0) => ("Hardware scan finished.".into(), false),
        ("stop", 0) => ("The driver was asked to unload.".into(), false),
        ("stop", 1062) => ("The driver is not loaded.".into(), false),
        ("stop", 1052) => ("The driver refused to unload; it stays loaded until Windows restarts.".into(), true),
        ("stop", 1051) => ("Not unloaded: other drivers depend on it.".into(), true),
        (_, c) => (format!("Finished with code {c}. See the output below."), true),
    };
    ToolOutcome { message, output: v["output"].as_str().unwrap_or_default().to_string(), reboot: code == 3010, cancelled: false }
}

fn parse_search(v: &serde_json::Value) -> Result<Vec<DriverUpdate>, String> {
    if v["ok"].as_bool() != Some(true) {
        return Err(v["error"].as_str().unwrap_or("unknown error").to_string());
    }
    let mut list: Vec<DriverUpdate> = serde_json::from_value(v["updates"].clone()).unwrap_or_default();
    for u in &mut list {
        u.version = version_from_title(&u.title);
    }
    Ok(list)
}

/// "ASIX Net Driver Update (4.20.2.0)" → "4.20.2.0";
/// "Intel - System - 10.1.1.42" → "10.1.1.42".
pub fn version_from_title(t: &str) -> String {
    let t = t.trim();
    if let (Some(a), true) = (t.rfind('('), t.ends_with(')')) {
        let v = &t[a + 1..t.len() - 1];
        if v.chars().all(|c| c.is_ascii_digit() || c == '.') && v.contains('.') {
            return v.to_string();
        }
    }
    let last = t.rsplit([' ', '-']).next().unwrap_or("");
    if last.chars().all(|c| c.is_ascii_digit() || c == '.') && last.contains('.') {
        last.to_string()
    } else {
        String::new()
    }
}

fn match_updates(devices: &[DeviceDriver], updates: &mut [DriverUpdate]) {
    for u in updates {
        let dev = devices
            .iter()
            .filter(|d| d.matches(&u.hardware_id))
            // Prefer a device that is plugged in.
            .max_by_key(|d| d.present);
        u.device = dev.map(|d| d.instance_id.clone());
        u.newer = dev.and_then(|d| compare_driver(&u.version, &u.date, &d.version, &d.date));
    }
}

/// "4.22.1.0" -> [4, 22, 1, 0]; None when it is not a dotted version.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = v.trim().split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

/// Is the offered driver newer than the installed one? Versions first
/// (missing parts count as 0); dates when a version is missing.
pub fn compare_driver(new_ver: &str, new_date: &str, old_ver: &str, old_date: &str) -> Option<bool> {
    if let (Some(mut a), Some(mut b)) = (parse_version(new_ver), parse_version(old_ver)) {
        let n = a.len().max(b.len());
        a.resize(n, 0);
        b.resize(n, 0);
        return Some(a > b);
    }
    if !new_date.is_empty() && !old_date.is_empty() {
        return Some(new_date > old_date);
    }
    None
}

/// Driver packages in the open dialog.
pub const INF_FILE: crate::platform::FileType = crate::platform::FileType { name: "Driver package", extension: "inf" };

// --------------------------------------------------------------- scripts

pub(crate) const SEARCH_SCRIPT: &str =r#"param([string]$Out)
$ErrorActionPreference = 'Stop'
$res = [ordered]@{ ok = $false; error = ''; updates = @() }
try {
  $s = New-Object -ComObject Microsoft.Update.Session
  $s.ClientApplicationID = 'Ultimate PC Manager'
  $r = $s.CreateUpdateSearcher().Search("IsInstalled=0 and Type='Driver'")
  $list = @()
  foreach ($u in $r.Updates) {
    $date = ''
    if ($u.DriverVerDate) { $date = ([datetime]$u.DriverVerDate).ToString('yyyy-MM-dd') }
    $list += [ordered]@{
      Id = [string]$u.Identity.UpdateID; Title = [string]$u.Title; HardwareId = [string]$u.DriverHardwareID
      Model = [string]$u.DriverModel; Provider = [string]$u.DriverProvider; Class = [string]$u.DriverClass
      Date = $date; Size = [double]$u.MaxDownloadSize; BrowseOnly = [bool]$u.BrowseOnly; Downloaded = [bool]$u.IsDownloaded
    }
  }
  $res.updates = $list
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res -Depth 5 | Out-File -FilePath $Out -Encoding utf8
"#;

const INSTALL_SCRIPT: &str = r#"param([string]$Out, [string]$Ids)
$ErrorActionPreference = 'Stop'
$res = [ordered]@{ ok = $false; error = ''; reboot = $false; items = @() }
try {
  $want = $Ids -split ','
  $s = New-Object -ComObject Microsoft.Update.Session
  $s.ClientApplicationID = 'Ultimate PC Manager'
  $searcher = $s.CreateUpdateSearcher()
  $col = New-Object -ComObject Microsoft.Update.UpdateColl
  $missing = @()
  foreach ($id in $want) {
    # The clicked offer's identity is looked up directly.
    $r = $searcher.Search("UpdateID='$id' and IsInstalled=0")
    if ($r.Updates.Count -eq 0) { $missing += $id; continue }
    $u = $r.Updates.Item(0)
    if (-not $u.EulaAccepted) { $u.AcceptEula() }
    [void]$col.Add($u)
  }
  if ($missing.Count -gt 0) {
    # Only an identity that is no longer valid searches all driver updates again.
    $r = $searcher.Search("IsInstalled=0 and Type='Driver'")
    foreach ($u in $r.Updates) {
      if ($missing -contains $u.Identity.UpdateID) {
        if (-not $u.EulaAccepted) { $u.AcceptEula() }
        [void]$col.Add($u)
      }
    }
  }
  if ($col.Count -eq 0) { throw 'Windows Update no longer offers the selected updates.' }
  $d = $s.CreateUpdateDownloader(); $d.Updates = $col; [void]$d.Download()
  $i = $s.CreateUpdateInstaller(); $i.Updates = $col; $ir = $i.Install()
  $items = @()
  for ($k = 0; $k -lt $col.Count; $k++) {
    $x = $ir.GetUpdateResult($k)
    $items += [ordered]@{ Id = [string]$col.Item($k).Identity.UpdateID; Title = [string]$col.Item($k).Title; Code = [int]$x.ResultCode; HResult = [int]$x.HResult }
  }
  $res.items = $items
  $res.reboot = [bool]$ir.RebootRequired
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res -Depth 5 | Out-File -FilePath $Out -Encoding utf8
"#;

const TOOL_SCRIPT: &str = r#"param([string]$Out, [string]$Mode, [string]$Arg)
$pnputil = Join-Path ([Environment]::SystemDirectory) 'pnputil.exe'
$sc = Join-Path ([Environment]::SystemDirectory) 'sc.exe'
$res = [ordered]@{ ok = $false; error = ''; code = 0; output = '' }
try {
  switch ($Mode) {
    'inf' {
      if (Test-Path -LiteralPath $Arg -PathType Container) {
        $o = & $pnputil /add-driver (Join-Path $Arg '*.inf') /subdirs /install 2>&1
      } else {
        $o = & $pnputil /add-driver $Arg /install 2>&1
      }
    }
    'scan' { $o = & $pnputil /scan-devices 2>&1 }
    'stop' { $o = & $sc stop $Arg 2>&1 }
    default { throw "Unknown mode $Mode" }
  }
  $res.code = $LASTEXITCODE
  $res.output = ($o | Out-String).Trim()
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res -Depth 5 | Out-File -FilePath $Out -Encoding utf8
"#;

/// One registry value of the security band, as read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reading {
    /// The DWORD the value holds.
    Value(u32),
    /// The key or the value does not exist: Windows was never told.
    NotSet,
    /// The read failed (access denied, a value of another type): why.
    Failed(String),
}

/// The two settings the Security band shows, read from the registry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityReading {
    /// `DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity\Enabled`.
    pub memory_integrity: Reading,
    /// `CI\Config\VulnerableDriverBlocklistEnable`.
    pub blocklist: Reading,
}

impl SecurityReading {
    /// Both settings could not be read, for `why`.
    pub fn unreadable(why: &str) -> Self {
        SecurityReading { memory_integrity: Reading::Failed(why.into()), blocklist: Reading::Failed(why.into()) }
    }
}

/// What the band says about one setting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    On,
    Off,
    /// Windows holds no value: the page does not guess.
    NotSet,
    /// The value could not be read (or holds something unexpected).
    Unreadable,
}

impl Setting {
    /// The word shown after the setting's name.
    pub fn text(self) -> &'static str {
        match self {
            Setting::On => "On",
            Setting::Off => "Off",
            Setting::NotSet => "Not set",
            Setting::Unreadable => "could not be read",
        }
    }
}

/// A value read becomes a state: 1 is on, 0 is off, a missing value is not
/// set, and a failed read or any other number is unreadable (never "off").
pub fn setting_of(reading: &Reading) -> Setting {
    match reading {
        Reading::Value(1) => Setting::On,
        Reading::Value(0) => Setting::Off,
        Reading::NotSet => Setting::NotSet,
        Reading::Value(_) | Reading::Failed(_) => Setting::Unreadable,
    }
}

/// The name of the first setting.
pub const MEMORY_INTEGRITY: &str = "Memory integrity";
/// The name of the second setting.
pub const BLOCKLIST: &str = "Vulnerable driver blocklist";
/// The link the band opens: Windows Security's core isolation page.
pub const CORE_ISOLATION_LINK: &str = "windowsdefender://coreisolation";
/// The band's reminder that the page never changes these settings.
pub const SECURITY_READ_ONLY: &str = "Read-only: change them in Windows Security.";

/// What the Security band shows: both settings and one explaining line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SecurityView {
    /// (name, state) in display order; no state while nothing was read yet.
    pub items: [(&'static str, Option<Setting>); 2],
    /// Explains anything that is not plainly on; empty when all is on.
    pub note: String,
}

/// The band for a reading (`None` while it has not been read yet).
pub fn security_view(reading: Option<&SecurityReading>) -> SecurityView {
    let Some(r) = reading else {
        return SecurityView { items: [(MEMORY_INTEGRITY, None), (BLOCKLIST, None)], note: "Reading the security settings\u{2026}".into() };
    };
    let (memory, blocklist) = (setting_of(&r.memory_integrity), setting_of(&r.blocklist));
    let mut notes: Vec<String> = Vec::new();
    match (memory, blocklist) {
        (Setting::Unreadable, Setting::Unreadable) => notes.push("Windows did not let us read these settings, so we do not guess.".into()),
        (Setting::Unreadable, _) => notes.push("Windows did not let us read Memory integrity, so we do not guess.".into()),
        (_, Setting::Unreadable) => notes.push("Windows did not let us read the vulnerable driver blocklist, so we do not guess.".into()),
        _ => {}
    }
    match memory {
        Setting::Off => notes.push("Memory integrity is off: Windows does not isolate the kernel from unsafe code.".into()),
        Setting::NotSet => notes.push("Memory integrity has no value in Windows, so we do not say whether it is on.".into()),
        _ => {}
    }
    match blocklist {
        Setting::Off => notes.push("The blocklist is off: Windows does not block known vulnerable drivers.".into()),
        Setting::NotSet => notes.push("The blocklist has no value in Windows, so we do not say whether it is on.".into()),
        _ => {}
    }
    SecurityView { items: [(MEMORY_INTEGRITY, Some(memory)), (BLOCKLIST, Some(blocklist))], note: notes.join(" ") }
}

/// Puts the outcome of a read of devices and driver services into `st`: a
/// failed read leaves its list empty and keeps the reason, so the page can
/// tell an error from Windows reporting nothing.
fn publish_listing(st: &mut State, devices: Result<Vec<DeviceDriver>, String>, kernel: Result<Vec<KernelDriver>, String>) {
    /// A read's rows and why it failed: no rows on failure.
    fn split<T>(read: Result<Vec<T>, String>) -> (Vec<T>, Option<String>) {
        read.map_or_else(|e| (Vec::new(), Some(e)), |list| (list, None))
    }
    let (devices, devices_error) = split(devices);
    let (kernel, kernel_error) = split(kernel);
    (st.devices, st.devices_error) = (devices.into(), devices_error);
    (st.kernel, st.kernel_error) = (kernel.into(), kernel_error);
    st.listing = false;
    let State { devices, updates, .. } = st;
    if let Some(u) = updates {
        let mut list = u.to_vec();
        match_updates(devices, &mut list);
        *u = list.into();
    }
}

/// Puts the outcome of a Windows Update search that ended at `at` into
/// `st`. Only a search the user asked for `by_hand` is kept for Summary.
fn publish_check(st: &mut State, found: Result<Vec<DriverUpdate>, String>, by_hand: bool, at: &str) {
    match found {
        Ok(mut list) => {
            match_updates(&st.devices, &mut list);
            // The page's status band tells the outcome; clear any older
            // message so it does not repeat or contradict it.
            st.message = None;
            st.checked_at = at.to_string();
            if by_hand {
                let newer = list.iter().filter(|u| u.worth_installing()).count();
                st.hand_check = Some(HandCheck { newer, at: at.to_string() });
            }
            st.updates = Some(list.into());
        }
        Err(e) => st.message = Some((format!("Could not check Windows Update: {e}"), true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::drivers::windows::WindowsListing;
    use crate::platform::Launch;

    #[test]
    fn version_in_title() {
        assert_eq!(version_from_title("ASIX Net Driver Update (4.20.2.0)"), "4.20.2.0");
        assert_eq!(version_from_title("Intel - System - 10.1.1.42"), "10.1.1.42");
        assert_eq!(version_from_title("Realtek Semiconductor Corp. - Extension"), "");
    }

    #[test]
    fn hardware_id_match_ignores_case() {
        let d = DeviceDriver { hardware_ids: vec![r"USB\VID_0B95&PID_1790&REV_0200".into()], ..Default::default() };
        assert!(d.matches(r"usb\vid_0b95&pid_1790&rev_0200"));
        assert!(!d.matches(""));
    }

    #[test]
    fn compares_versions_then_dates() {
        assert_eq!(compare_driver("4.20.2.0", "", "4.22.1.0", ""), Some(false));
        assert_eq!(compare_driver("1.0.4.55794", "", "1.0.4.7057", ""), Some(true));
        assert_eq!(compare_driver("10.1", "", "10.1.0.0", ""), Some(false));
        assert_eq!(compare_driver("", "2026-10-12", "", "2025-01-01"), Some(true));
        assert_eq!(compare_driver("", "", "1.0", ""), None);
    }

    fn wait_until_idle(hub: &Hub) {
        for _ in 0..1000 {
            let st = hub.lock();
            if st.busy.is_none() && !st.listing {
                return;
            }
            drop(st);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not finish");
    }

    #[test]
    fn checking_windows_update_runs_powershell_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx, Arc::new(WindowsListing));
        hub.check_updates();
        wait_until_idle(&hub);
        assert_eq!(runner.launches(), vec![Launch::PowerShell { script: SEARCH_SCRIPT.into(), args: vec![], elevated: false }]);
        assert!(hub.lock().message.as_ref().is_some_and(|(_, error)| *error), "the failure is shown");
    }

    #[test]
    fn scanning_asks_for_an_elevated_powershell_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx, Arc::new(WindowsListing));
        hub.scan();
        wait_until_idle(&hub);
        assert_eq!(
            runner.launches(),
            vec![Launch::PowerShell { script: TOOL_SCRIPT.into(), args: vec![("Mode".into(), "scan".into()), ("Arg".into(), String::new())], elevated: true }]
        );
    }

    /// A reader that only counts its reads.
    #[derive(Default)]
    struct Counting(std::sync::atomic::AtomicUsize);

    impl crate::addons::drivers::ListingReader for Counting {
        fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Vec::new())
        }
        fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
            Ok(Vec::new())
        }
    }

    fn install_hub() -> (Hub, Arc<crate::platform::RecordingRunner>, Arc<Counting>) {
        let (ctx, runner) = AddonContext::recording();
        let reader = Arc::new(Counting::default());
        let hub = Hub::new(ctx, reader.clone());
        wait_until_idle(&hub);
        (hub, runner, reader)
    }

    fn reads(reader: &Counting) -> usize {
        reader.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[test]
    fn installing_launches_the_elevated_script_with_the_identity_of_every_clicked_offer() {
        let (hub, runner, _) = install_hub();
        runner.prepare(crate::platform::When::Nth(0), 0, r#"{"ok":true,"reboot":false,"items":[{"Id":"id-1","Title":"A","Code":2,"HResult":0},{"Id":"id-2","Title":"B","Code":2,"HResult":0}]}"#);
        hub.install(vec!["id-1".into(), "id-2".into()]);
        wait_until_idle(&hub);
        assert_eq!(runner.launches(), vec![Launch::PowerShell { script: INSTALL_SCRIPT.into(), args: vec![("Ids".into(), "id-1,id-2".into())], elevated: true }]);
        assert_eq!(hub.lock().message, Some(("Installed 2 driver updates.".to_string(), false)), "the outcome message is unchanged");
    }

    #[test]
    fn the_install_script_looks_the_identity_up_and_searches_all_again_only_for_one_no_longer_valid() {
        let direct = INSTALL_SCRIPT.find("UpdateID='$id'").expect("a lookup by identity");
        let all = INSTALL_SCRIPT.find("Type='Driver'").expect("the full search");
        assert!(direct < all, "the identity comes first");
        assert!(INSTALL_SCRIPT.contains("if ($missing.Count -gt 0)"), "the full search only runs for what was not found");
        assert!(INSTALL_SCRIPT.contains("Windows Update no longer offers the selected updates."), "the failure message is unchanged");
    }

    #[test]
    fn a_cancelled_uac_prompt_installs_nothing_and_reads_nothing_again() {
        let (hub, runner, reader) = install_hub();
        let before = reads(&reader);
        runner.answer(crate::platform::ELEVATION_CANCELLED);
        hub.install(vec!["id-1".into()]);
        wait_until_idle(&hub);
        assert_eq!(runner.launches().len(), 1, "one elevated launch and no search");
        assert_eq!(reads(&reader), before, "the list is not read again");
        assert_eq!(hub.lock().message, Some((CANCELLED_TEXT.to_string(), false)));
    }

    #[test]
    fn a_finished_install_still_reads_the_list_again() {
        let (hub, runner, reader) = install_hub();
        let before = reads(&reader);
        runner.prepare(crate::platform::When::Nth(0), 0, r#"{"ok":true,"reboot":false,"items":[{"Id":"id-1","Title":"A","Code":2,"HResult":0}]}"#);
        hub.install(vec!["id-1".into()]);
        wait_until_idle(&hub);
        assert_eq!(reads(&reader), before + 1);
    }

    /// Reads this PC's drivers: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_this_pc() {
        let devices = WindowsListing.devices().expect("devices listed");
        let kernel = WindowsListing.kernel().expect("driver services listed");
        let present = devices.iter().filter(|d| d.present).count();
        println!("devices with a driver: {} ({} present)", devices.len(), present);
        for d in devices.iter().filter(|d| d.present && d.third_party()).take(12) {
            println!("  {:<45} {:<12} {:<22} {} {}", d.name, d.class, d.provider, d.version, d.date);
        }
        let loaded = kernel.iter().filter(|k| k.running).count();
        let sized = kernel.iter().filter(|k| k.image_size.is_some()).count();
        let mem: u64 = kernel.iter().filter_map(|k| k.image_size).sum();
        println!("kernel drivers: {} ({} loaded, {} with size, {} MB)", kernel.len(), loaded, sized, mem / 1_048_576);
        for k in kernel.iter().filter(|k| k.running).take(5) {
            println!("  {:<40} {:<10} {:<8} {:<14} {:<22} idx={:?} size={:?}", k.display, k.service, k.start_type, k.version, k.vendor, k.load_index, k.image_size);
        }
        assert!(present > 10);
        assert!(loaded > 10);
    }

    /// Asks Windows Update (takes 10–60 s): `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn checks_windows_update() {
        let devices = WindowsListing.devices().expect("devices listed");
        let v = AddonContext::system().powershell(SEARCH_SCRIPT, &[], false).expect("script ran");
        let mut list = parse_search(&v).expect("search ok");
        match_updates(&devices, &mut list);
        println!("updates: {}", list.len());
        for u in &list {
            let dev = u.device.as_ref().and_then(|id| devices.iter().find(|d| &d.instance_id == id));
            println!("  {} | offered {} | device {:?} installed {:?} | newer {:?}", u.title, u.version, dev.map(|d| &d.name), dev.map(|d| &d.version), u.newer);
        }
    }
}

#[cfg(test)]
mod kernel_scope_tests {
    use super::*;

    fn sample() -> Vec<KernelDriver> {
        let k = |service: &str, running: bool, vendor: &str, kind: KernelKind, size: Option<u64>| KernelDriver {
            service: service.into(),
            running,
            vendor: vendor.into(),
            kind,
            image_size: size,
            ..Default::default()
        };
        vec![
            k("ntfs", true, "Microsoft Corporation", KernelKind::FileSystem, Some(3_000_000)),
            k("disk", true, "Microsoft Corporation", KernelKind::Kernel, Some(100_000)),
            k("nvlddmkm", true, "NVIDIA Corporation", KernelKind::Kernel, Some(40_000_000)),
            k("vbox", false, "Oracle Corporation", KernelKind::Kernel, None),
            k("fastfat", false, "Microsoft Corporation", KernelKind::FileSystem, None),
            k("beep", false, "", KernelKind::Kernel, None),
        ]
    }

    fn services(scope: KernelScope, list: &[KernelDriver]) -> Vec<&str> {
        list.iter().filter(|d| scope.matches(d)).map(|d| d.service.as_str()).collect()
    }

    #[test]
    fn scopes_filter_the_list() {
        let list = sample();
        assert_eq!(services(KernelScope::All, &list).len(), 6);
        assert_eq!(services(KernelScope::Loaded, &list), ["ntfs", "disk", "nvlddmkm"]);
        assert_eq!(services(KernelScope::NotLoaded, &list), ["vbox", "fastfat", "beep"]);
        assert_eq!(services(KernelScope::ThirdParty, &list), ["nvlddmkm", "vbox"]);
        assert_eq!(services(KernelScope::FileSystem, &list), ["ntfs", "fastfat"]);
    }

    #[test]
    fn scopes_keep_a_fixed_order_and_labels() {
        let labels: Vec<&str> = KernelScope::ALL.iter().map(|s| s.label()).collect();
        assert_eq!(labels, ["All", "Loaded", "Not loaded", "Third-party", "File system"]);
        assert_eq!(KernelScope::default(), KernelScope::All);
    }

    #[test]
    fn scope_counts_match_the_sample() {
        let list = sample();
        let counts: Vec<usize> = KernelScope::ALL.iter().map(|s| s.count(&list)).collect();
        assert_eq!(counts, [6, 3, 3, 2, 2]);
    }

    #[test]
    fn four_figures_of_the_sample() {
        let f = KernelFigures::of(&sample());
        assert_eq!(f, KernelFigures { drivers: 6, loaded: 3, third_party: 2, memory: Some(43_100_000) });
    }

    #[test]
    fn kernel_memory_counts_only_loaded_images() {
        let mut list = sample();
        list[3].image_size = Some(9_999);
        assert_eq!(KernelFigures::of(&list).memory, Some(43_100_000));
    }

    fn on_disk(service: &str, path: &str, size: u64) -> KernelDriver {
        KernelDriver { service: service.into(), running: true, path: path.into(), image_size: Some(size), ..Default::default() }
    }

    #[test]
    fn an_empty_vendor_is_not_third_party_and_microsoft_is_case_insensitive() {
        let v = |vendor: &str| KernelDriver { vendor: vendor.into(), ..Default::default() }.third_party();
        assert!(!v(""));
        assert!(!v("MICROSOFT WINDOWS"));
        assert!(!v("microsoft corporation"));
        assert!(v("Oracle Corporation"));
    }

    #[test]
    fn a_service_in_a_pending_state_counts_as_loaded() {
        for code in [2, 3, 4, 5, 6, 7] {
            let d = KernelDriver { running: code == 4, state_code: Some(code), ..Default::default() };
            assert!(KernelScope::Loaded.matches(&d), "state {code}");
            assert!(!KernelScope::NotLoaded.matches(&d), "state {code}");
        }
        let stopped = KernelDriver { state_code: Some(1), ..Default::default() };
        assert!(KernelScope::NotLoaded.matches(&stopped) && !KernelScope::Loaded.matches(&stopped));
    }

    #[test]
    fn two_services_on_one_file_add_its_size_once() {
        let rows = [on_disk("a", r"C:\Windows\System32\drivers\x.sys", 500), on_disk("b", r"c:\windows\system32\DRIVERS\X.SYS", 500), on_disk("c", r"C:\y.sys", 7)];
        assert_eq!(KernelFigures::of(&rows).memory, Some(507));
    }

    #[test]
    fn an_unreadable_module_list_makes_memory_unknown_not_zero() {
        let mut rows = [on_disk("a", r"C:\x.sys", 0)];
        rows[0].image_size = None;
        rows[0].modules_unread = true;
        let f = KernelFigures::of(&rows);
        assert_eq!(f.memory, None);
        assert_eq!(f.loaded, 1);
    }

    #[test]
    fn same_file_name_in_two_folders_gets_its_own_module() {
        let m = |path: &str, size, index| LoadedModule { path: path.into(), size, index };
        let mods = [m(r"\SystemRoot\system32\drivers\a.sys", 10, 1), m(r"\??\D:\other\a.sys", 20, 2)];
        let win = r"C:\Windows";
        assert_eq!(find_module(&mods, r"C:\Windows\System32\drivers\a.sys", win).map(|m| (m.size, m.index)), Some((10, 1)));
        assert_eq!(find_module(&mods, r"D:\Other\a.sys", win).map(|m| (m.size, m.index)), Some((20, 2)));
        assert_eq!(find_module(&mods, r"E:\x\a.sys", win), None, "no guess by name when the folder differs");
    }

    #[test]
    fn a_lone_module_is_found_by_name_when_the_path_has_no_folder() {
        let mods = [LoadedModule { path: r"\SystemRoot\system32\ntoskrnl.exe".into(), size: 9, index: 0 }];
        assert_eq!(find_module(&mods, "ntoskrnl.exe", r"C:\Windows").map(|m| m.size), Some(9));
    }

    #[test]
    fn four_figures_of_an_empty_list() {
        assert_eq!(KernelFigures::of(&[]), KernelFigures { memory: Some(0), ..Default::default() });
    }

    #[test]
    fn a_saved_index_picks_the_scope_and_an_unknown_one_falls_back_to_all() {
        let read = |v: serde_json::Value| serde_json::from_value::<KernelScope>(v).unwrap();
        assert_eq!(read(serde_json::json!(3)), KernelScope::ThirdParty);
        assert_eq!(read(serde_json::json!(4)), KernelScope::FileSystem);
        assert_eq!(read(serde_json::json!(9)), KernelScope::All);
        assert_eq!(read(serde_json::json!("everything")), KernelScope::All);
    }

    #[test]
    fn a_scope_is_saved_and_read_by_its_command_line_name() {
        let names: Vec<serde_json::Value> = KernelScope::ALL.iter().map(|s| serde_json::to_value(s).unwrap()).collect();
        assert_eq!(names, ["all", "loaded", "not-loaded", "third-party", "file-system"]);
        for scope in KernelScope::ALL {
            assert_eq!(serde_json::from_value::<KernelScope>(serde_json::to_value(scope).unwrap()).unwrap(), scope);
        }
        assert_eq!(KernelScope::parse("Not-Loaded"), Some(KernelScope::NotLoaded), "case does not matter");
        assert_eq!(KernelScope::parse("kernel"), None);
    }

    #[test]
    fn the_file_system_scope_follows_the_service_type_not_its_label() {
        let fs = KernelDriver { kind: KernelKind::FileSystem, ..Default::default() };
        assert!(KernelScope::FileSystem.matches(&fs));
        assert_eq!(KernelKind::FileSystem.label(), "File system");
        assert!(!KernelScope::FileSystem.matches(&KernelDriver::default()), "a plain kernel driver");
    }
}

#[cfg(test)]
mod check_tests {
    use super::*;

    /// Updates for the one device of [`state`] at these versions (empty:
    /// for no known device).
    fn found(versions: &[&str]) -> Result<Vec<DriverUpdate>, String> {
        Ok(versions
            .iter()
            .enumerate()
            .map(|(i, v)| DriverUpdate { id: format!("u{i}"), version: v.to_string(), hardware_id: if v.is_empty() { String::new() } else { "PCI\\X".into() }, ..Default::default() })
            .collect())
    }

    /// A state with one device at driver version 2.0.0.0.
    fn state() -> State {
        let dev = DeviceDriver { instance_id: "dev".into(), hardware_ids: vec!["PCI\\X".into()], version: "2.0.0.0".into(), present: true, ..Default::default() };
        State { devices: vec![dev].into(), ..Default::default() }
    }

    #[test]
    fn only_a_check_by_hand_is_kept_for_summary() {
        let mut st = state();
        publish_check(&mut st, found(&["3.0.0.0"]), false, "09:00");
        assert_eq!(st.hand_check, None, "a check made when the page opened does not count");
        assert_eq!(st.checked_at, "09:00", "the page still shows it");

        publish_check(&mut st, found(&["3.0.0.0", "", "1.0.0.0"]), true, "14:03");
        assert_eq!(st.hand_check, Some(HandCheck { newer: 2, at: "14:03".into() }));

        publish_check(&mut st, found(&[]), false, "15:00");
        assert_eq!(st.hand_check, Some(HandCheck { newer: 2, at: "14:03".into() }), "a later automatic check changes nothing on Summary");
        assert_eq!(st.updates.as_ref().map(|u| u.len()), Some(0), "but the page shows its outcome");
    }

    #[test]
    fn a_failed_check_keeps_the_last_one() {
        let mut st = state();
        publish_check(&mut st, found(&[""]), true, "14:03");
        publish_check(&mut st, Err("offline".into()), true, "14:10");
        assert_eq!(st.hand_check, Some(HandCheck { newer: 1, at: "14:03".into() }));
        assert_eq!(st.message, Some(("Could not check Windows Update: offline".into(), true)));
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn a_failed_read_reaches_the_state_with_its_reason_not_as_an_empty_list() {
        let mut st = State { listing: true, ..Default::default() };
        publish_listing(&mut st, Err("the configuration manager answered 0x1f".into()), Err("access is denied".into()));
        assert!(!st.listing);
        assert_eq!(st.devices_error.as_deref(), Some("the configuration manager answered 0x1f"));
        assert_eq!(st.kernel_error.as_deref(), Some("access is denied"));
    }

    #[test]
    fn a_successful_read_clears_the_last_error() {
        let mut st = State { devices_error: Some("old".into()), kernel_error: Some("old".into()), ..Default::default() };
        let kernel = vec![KernelDriver { service: "disk".into(), ..Default::default() }];
        publish_listing(&mut st, Ok(vec![]), Ok(kernel));
        assert_eq!(st.devices_error, None);
        assert_eq!(st.kernel_error, None);
        assert_eq!(st.kernel.len(), 1);
        assert!(st.devices.is_empty());
    }

    #[test]
    fn a_failed_read_drops_the_rows_of_the_last_one() {
        let mut st = State { kernel: vec![KernelDriver::default()].into(), ..Default::default() };
        publish_listing(&mut st, Ok(vec![]), Err("gone".into()));
        assert!(st.kernel.is_empty(), "rows of an older read do not pass for this one");
    }
}

/// Outcomes of the elevated actions, read from what their scripts report.
#[cfg(test)]
mod outcome_tests {
    use super::*;
    use crate::addons::drivers::windows::WindowsListing;
    use crate::platform::ELEVATION_CANCELLED;
    use serde_json::json;

    const CANCELLED: &str = "Cancelled at the Windows prompt: nothing was changed.";

    fn item(id: &str, title: &str, code: i64, hresult: i64) -> serde_json::Value {
        json!({ "Id": id, "Title": title, "Code": code, "HResult": hresult })
    }

    #[test]
    fn a_search_reads_the_offered_updates_with_their_versions() {
        let v = json!({ "ok": true, "error": "", "updates": [{ "Id": "a", "Title": "ASIX Net Driver Update (4.20.2.0)", "HardwareId": "USB\\VID_0B95", "BrowseOnly": true }] });
        let list = parse_search(&v).expect("search ok");
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].id.as_str(), list[0].version.as_str(), list[0].browse_only), ("a", "4.20.2.0", true));
    }

    #[test]
    fn a_search_with_no_updates_is_an_empty_list() {
        assert_eq!(parse_search(&json!({ "ok": true, "error": "", "updates": [] })).map(|l| l.len()), Ok(0));
    }

    #[test]
    fn a_failed_search_reports_the_script_error() {
        assert_eq!(parse_search(&json!({ "ok": false, "error": "0x8024402c" })).map(|l| l.len()), Err("0x8024402c".into()));
    }

    #[test]
    fn installed_updates_are_counted_and_listed() {
        let v = json!({ "ok": true, "reboot": false, "items": [item("a", "A", 2, 0), item("b", "B", 2, 0)] });
        let o = install_outcome(Ok(v));
        assert_eq!(o.message, ("Installed 2 driver updates.".to_string(), false));
        assert_eq!(o.installed, vec!["a".to_string(), "b".to_string()]);
        assert!(!o.reboot);
    }

    #[test]
    fn one_installed_update_is_singular() {
        let o = install_outcome(Ok(json!({ "ok": true, "items": [item("a", "A", 2, 0)] })));
        assert_eq!(o.message, ("Installed 1 driver update.".to_string(), false));
    }

    #[test]
    fn an_update_installed_with_errors_counts_as_installed_and_says_so() {
        let o = install_outcome(Ok(json!({ "ok": true, "items": [item("a", "A", 2, 0), item("b", "B", 3, 0)] })));
        assert_eq!(o.message, ("Installed 2 driver updates (1 with errors).".to_string(), false));
        assert_eq!(o.installed, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn a_failed_update_is_named_with_its_error_code() {
        let o = install_outcome(Ok(json!({ "ok": true, "items": [item("a", "A", 2, 0), item("b", "Intel - System", 4, -2145124329)] })));
        assert_eq!(o.message, ("Installed 1; failed: Intel - System (0x80240017)".to_string(), true));
        assert_eq!(o.installed, vec!["a".to_string()]);
    }

    #[test]
    fn an_aborted_update_is_named_as_aborted() {
        let o = install_outcome(Ok(json!({ "ok": true, "items": [item("a", "Realtek Audio", 5, 0)] })));
        assert_eq!(o.message, ("Installed 0; failed: Realtek Audio (aborted)".to_string(), true));
        assert!(o.installed.is_empty());
    }

    #[test]
    fn an_install_that_needs_a_restart_says_so() {
        let o = install_outcome(Ok(json!({ "ok": true, "reboot": true, "items": [item("a", "A", 2, 0)] })));
        assert!(o.reboot);
    }

    #[test]
    fn a_script_error_is_an_install_failure() {
        let o = install_outcome(Ok(json!({ "ok": false, "error": "Windows Update no longer offers the selected updates." })));
        assert_eq!(o.message, ("Install failed: Windows Update no longer offers the selected updates.".to_string(), true));
        assert!(o.installed.is_empty() && !o.reboot);
    }

    #[test]
    fn a_script_that_could_not_run_is_an_error() {
        let o = install_outcome(Err("PowerShell finished without a result.".into()));
        assert_eq!(o.message, ("PowerShell finished without a result.".to_string(), true));
    }

    #[test]
    fn a_dismissed_uac_prompt_installs_nothing_and_is_not_a_failure() {
        let o = install_outcome(Err(ELEVATION_CANCELLED.into()));
        assert_eq!(o.message, (CANCELLED.to_string(), false));
        assert!(o.installed.is_empty() && !o.reboot);
    }

    fn tool(mode: &str, code: i64) -> ToolOutcome {
        tool_outcome(mode, Ok(json!({ "ok": true, "code": code, "output": "out" })))
    }

    #[test]
    fn the_elevated_tool_script_calls_system_tools_by_their_system32_path() {
        for line in TOOL_SCRIPT.lines() {
            assert!(!line.contains("& pnputil") && !line.contains("& sc.exe"), "found a PATH lookup: {line}");
        }
        assert!(TOOL_SCRIPT.contains("[Environment]::SystemDirectory"));
    }

    #[test]
    fn pnputil_codes_for_a_driver_package() {
        assert_eq!(tool("inf", 0).message, ("Driver package installed.".to_string(), false));
        assert_eq!(tool("inf", 259).message, ("The package was added, but no device needed it (its driver is not newer or does not match).".to_string(), false));
        let restart = tool("inf", 3010);
        assert_eq!(restart.message, ("Done. Restart Windows to finish.".to_string(), false));
        assert!(restart.reboot);
        assert!(!tool("inf", 0).reboot);
        assert_eq!(tool("inf", 2).message, ("Finished with code 2. See the output below.".to_string(), true));
    }

    #[test]
    fn pnputil_codes_for_a_hardware_scan() {
        assert_eq!(tool("scan", 0).message, ("Hardware scan finished.".to_string(), false));
        assert!(tool("scan", 3010).reboot);
        assert_eq!(tool("scan", 5).message, ("Finished with code 5. See the output below.".to_string(), true));
    }

    #[test]
    fn sc_codes_for_an_unload() {
        assert_eq!(tool("stop", 0).message, ("The driver was asked to unload.".to_string(), false));
        assert_eq!(tool("stop", 1062).message, ("The driver is not loaded.".to_string(), false));
        assert_eq!(tool("stop", 1052).message, ("The driver refused to unload; it stays loaded until Windows restarts.".to_string(), true));
        assert_eq!(tool("stop", 1051).message, ("Not unloaded: other drivers depend on it.".to_string(), true));
        assert_eq!(tool("stop", 1060).message, ("Finished with code 1060. See the output below.".to_string(), true));
    }

    #[test]
    fn a_tool_run_keeps_its_output() {
        assert_eq!(tool("scan", 0).output, "out");
    }

    #[test]
    fn a_tool_script_error_or_launch_failure_is_an_error() {
        assert_eq!(tool_outcome("inf", Ok(json!({ "ok": false, "error": "Unknown mode x" }))).message, ("Unknown mode x".to_string(), true));
        assert_eq!(tool_outcome("scan", Err("boom".into())).message, ("boom".to_string(), true));
    }

    #[test]
    fn a_dismissed_uac_prompt_for_a_tool_changes_nothing() {
        for mode in ["inf", "scan", "stop"] {
            let o = tool_outcome(mode, Err(ELEVATION_CANCELLED.into()));
            assert_eq!(o.message, (CANCELLED.to_string(), false), "{mode}");
            assert!(!o.reboot && o.output.is_empty());
        }
    }

    fn wait_until_idle(hub: &Hub) {
        for _ in 0..1000 {
            if hub.lock().busy.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not finish");
    }

    #[test]
    fn the_page_tells_a_dismissed_prompt_apart_from_a_failed_install() {
        let (ctx, runner) = AddonContext::recording();
        runner.answer(ELEVATION_CANCELLED);
        let hub = Hub::new(ctx, Arc::new(WindowsListing));
        hub.install_inf(r"C:\drivers\x.inf".into());
        wait_until_idle(&hub);
        assert_eq!(hub.lock().message, Some((CANCELLED.to_string(), false)));
        hub.install(vec!["a".into()]);
        wait_until_idle(&hub);
        assert_eq!(hub.lock().message, Some((CANCELLED.to_string(), false)));
    }

    fn kernel(running: bool, start_code: u32) -> KernelDriver {
        KernelDriver { running, start_code: Some(start_code), ..Default::default() }
    }

    #[test]
    fn unload_is_not_offered_for_a_driver_that_is_not_loaded() {
        assert_eq!(kernel(false, 3).unload_note(true), (false, "Not loaded."));
        assert_eq!(kernel(false, 0).unload_note(false), (false, "Not loaded."));
    }

    #[test]
    fn an_unread_configuration_shows_unknown_and_offers_no_unload() {
        assert_eq!(start_type_name(None), "Unknown");
        assert_eq!(start_type_name(Some(3)), "Manual");
        assert_eq!(start_type_name(Some(2)), "Automatic");
        let unread = KernelDriver { running: true, start_code: None, ..Default::default() };
        let note = (false, "Not offered: its configuration could not be read.");
        assert_eq!(unread.unload_note(true), note);
        assert_eq!(unread.unload_note(false), note);
        assert_eq!(KernelDriver::default().unload_note(true), (false, "Not loaded."));
    }

    #[test]
    fn every_service_state_has_its_own_label() {
        let names: Vec<_> = (1..=7).map(|c| service_state_name(Some(c)).unwrap()).collect();
        assert_eq!(names, ["Stopped", "Start pending", "Stop pending", "Running", "Continue pending", "Pause pending", "Paused"]);
        assert_eq!(service_state_name(None), None);
        assert_eq!(service_state_name(Some(0)), None);
        assert_eq!(KernelDriver { running: true, ..Default::default() }.state_label(), "Running");
        assert_eq!(KernelDriver { running: false, ..Default::default() }.state_label(), "Stopped");
        assert_eq!(KernelDriver { running: false, state_code: Some(3), ..Default::default() }.state_label(), "Stop pending");
    }

    #[test]
    fn every_service_type_has_its_own_kind() {
        assert_eq!(kernel_kind_from_type(1).label(), "Kernel");
        assert_eq!(kernel_kind_from_type(2).label(), "File system");
        assert_eq!(kernel_kind_from_type(8).label(), "Recognizer");
        assert_eq!(kernel_kind_from_type(4).label(), "Adapter");
        assert_eq!(kernel_kind_from_type(2 | 8).label(), "File system");
    }

    #[test]
    fn dependencies_split_into_services_and_groups() {
        // '#' stands for the nul that ends each string; the list ends with an empty string.
        let units: Vec<u16> = "+Base#Tcpip#Ndis##".encode_utf16().map(|u| if u == 0x23 { 0 } else { u }).collect();
        let (s, g) = parse_dependencies(&units);
        assert_eq!(s, ["Tcpip", "Ndis"]);
        assert_eq!(g, ["Base"]);
        assert_eq!(parse_dependencies(&[0, 0]), (vec![], vec![]));
        assert_eq!(parse_dependencies(&[]), (vec![], vec![]));
    }

    #[test]
    fn depends_on_text_lists_services_then_groups() {
        let d = KernelDriver { depends_services: vec!["Tcpip".into(), "Ndis".into()], depends_groups: vec!["Base".into()], ..Default::default() };
        assert_eq!(d.depends_on_text(), "Tcpip, Ndis, Base (group)");
        assert_eq!(KernelDriver::default().depends_on_text(), "");
    }

    #[test]
    fn unload_is_not_offered_for_a_boot_or_system_start_driver() {
        let note = "Not offered: Windows loaded it at boot, and the kernel refuses to stop a driver it started itself.";
        assert_eq!(kernel(true, 0).unload_note(true), (false, note));
        assert_eq!(kernel(true, 1).unload_note(false), (false, note));
    }

    #[test]
    fn unload_without_administrator_rights_says_windows_will_ask() {
        assert_eq!(kernel(true, 3).unload_note(false), (true, "Needs administrator rights (Windows will ask); most drivers refuse the request while the system runs."));
    }

    #[test]
    fn unload_with_administrator_rights_is_offered() {
        assert_eq!(kernel(true, 2).unload_note(true), (true, "Offered: most drivers refuse the request while the system runs."));
    }
}

/// The Security band: which state a value shows, what the band says, and
/// the read through the hub (#32).
#[cfg(test)]
mod security_tests {
    use super::*;
    use crate::addons::drivers::windows::WindowsListing;
    use crate::addons::until;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn a_value_shows_as_on_off_not_set_or_unreadable() {
        assert_eq!(setting_of(&Reading::Value(1)), Setting::On);
        assert_eq!(setting_of(&Reading::Value(0)), Setting::Off);
        assert_eq!(setting_of(&Reading::NotSet), Setting::NotSet);
        assert_eq!(setting_of(&Reading::Failed("access is denied".into())), Setting::Unreadable);
        assert_eq!(setting_of(&Reading::Value(7)), Setting::Unreadable, "an unexpected number is not guessed as off");
    }

    #[test]
    fn every_state_has_its_own_text() {
        let texts = [Setting::On, Setting::Off, Setting::NotSet, Setting::Unreadable].map(Setting::text);
        assert_eq!(texts, ["On", "Off", "Not set", "could not be read"]);
    }

    fn reading(memory: Reading, blocklist: Reading) -> SecurityReading {
        SecurityReading { memory_integrity: memory, blocklist }
    }

    #[test]
    fn both_on_has_nothing_to_explain() {
        let v = security_view(Some(&reading(Reading::Value(1), Reading::Value(1))));
        assert_eq!(v.items, [(MEMORY_INTEGRITY, Some(Setting::On)), (BLOCKLIST, Some(Setting::On))]);
        assert_eq!(v.note, "");
    }

    #[test]
    fn an_off_setting_says_what_it_means() {
        let v = security_view(Some(&reading(Reading::Value(1), Reading::Value(0))));
        assert_eq!(v.items[1], (BLOCKLIST, Some(Setting::Off)));
        assert_eq!(v.note, "The blocklist is off: Windows does not block known vulnerable drivers.");
        let v = security_view(Some(&reading(Reading::Value(0), Reading::Value(1))));
        assert_eq!(v.note, "Memory integrity is off: Windows does not isolate the kernel from unsafe code.");
    }

    #[test]
    fn an_unreadable_value_says_so_and_is_not_called_off() {
        let denied = || Reading::Failed("access is denied".into());
        let both = security_view(Some(&reading(denied(), denied())));
        assert_eq!(both.items, [(MEMORY_INTEGRITY, Some(Setting::Unreadable)), (BLOCKLIST, Some(Setting::Unreadable))]);
        assert_eq!(both.note, "Windows did not let us read these settings, so we do not guess.");
        let one = security_view(Some(&reading(Reading::Value(1), denied())));
        assert_eq!(one.items[0].1, Some(Setting::On), "the other value is still shown");
        assert_eq!(one.note, "Windows did not let us read the vulnerable driver blocklist, so we do not guess.");
        let other = security_view(Some(&reading(denied(), Reading::Value(0))));
        assert_eq!(
            other.note,
            "Windows did not let us read Memory integrity, so we do not guess. The blocklist is off: Windows does not block known vulnerable drivers."
        );
    }

    #[test]
    fn a_missing_value_is_not_set_and_is_not_guessed() {
        let v = security_view(Some(&reading(Reading::NotSet, Reading::NotSet)));
        assert_eq!(v.items, [(MEMORY_INTEGRITY, Some(Setting::NotSet)), (BLOCKLIST, Some(Setting::NotSet))]);
        assert!(v.note.contains("Memory integrity has no value") && v.note.contains("The blocklist has no value"), "{}", v.note);
    }

    #[test]
    fn before_the_first_read_nothing_is_claimed() {
        let v = security_view(None);
        assert_eq!(v.items, [(MEMORY_INTEGRITY, None), (BLOCKLIST, None)]);
        assert_eq!(v.note, "Reading the security settings\u{2026}");
    }

    /// A reader that answers the security band from a script, counts how
    /// often it was asked and can hold the kernel read until released.
    struct Fake {
        security: SecurityReading,
        asked: AtomicUsize,
        in_kernel: AtomicBool,
        release: AtomicBool,
        hold: bool,
    }

    impl Fake {
        fn new(security: SecurityReading, hold: bool) -> Arc<Fake> {
            Arc::new(Fake { security, asked: AtomicUsize::new(0), in_kernel: AtomicBool::new(false), release: AtomicBool::new(!hold), hold })
        }
    }

    impl ListingReader for Fake {
        fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
            Ok(Vec::new())
        }
        fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
            self.in_kernel.store(true, Ordering::SeqCst);
            while self.hold && !self.release.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Ok(Vec::new())
        }
        fn security(&self) -> SecurityReading {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.security.clone()
        }
    }

    #[test]
    fn a_listing_reads_the_security_band_with_normal_rights_and_starts_nothing_elevated() {
        let (ctx, runner) = AddonContext::recording();
        let fake = Fake::new(reading(Reading::Value(1), Reading::Value(0)), false);
        let hub = Hub::new(ctx, fake.clone());
        assert!(hub.lock().security.is_none(), "nothing is read before the page asks");
        hub.refresh();
        until("the band is read", || hub.lock().security.is_some());
        assert_eq!(hub.lock().security, Some(reading(Reading::Value(1), Reading::Value(0))));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 1);
        assert!(runner.launches().is_empty(), "no PowerShell, no elevation: the registry is read in process");
    }

    #[test]
    fn a_hub_that_was_stopped_reads_nothing_and_publishes_nothing() {
        let (ctx, _) = AddonContext::recording();
        let fake = Fake::new(reading(Reading::Value(1), Reading::Value(1)), false);
        let hub = Hub::new(ctx, fake.clone());
        hub.stop();
        hub.refresh();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 0, "nothing starts while off");
        assert!(hub.lock().security.is_none());
    }

    #[test]
    fn turning_off_while_the_listing_runs_drops_the_security_read() {
        let (ctx, _) = AddonContext::recording();
        let fake = Fake::new(reading(Reading::Value(1), Reading::Value(1)), true);
        let hub = Hub::new(ctx, fake.clone());
        hub.refresh();
        until("the listing is running", || fake.in_kernel.load(Ordering::SeqCst));
        hub.stop();
        fake.release.store(true, Ordering::SeqCst);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(fake.asked.load(Ordering::SeqCst), 0, "the band is not read once the add-on is off");
        assert!(hub.lock().security.is_none(), "and nothing is published");
    }

    /// Reads this PC's two values (run with `--ignored --nocapture`).
    #[test]
    #[ignore]
    fn reads_the_security_values_of_this_pc() {
        let r = WindowsListing.security();
        println!("memory integrity: {:?} -> {:?}", r.memory_integrity, setting_of(&r.memory_integrity));
        println!("blocklist: {:?} -> {:?}", r.blocklist, setting_of(&r.blocklist));
        println!("{:?}", security_view(Some(&r)));
        assert!(!matches!(r.memory_integrity, Reading::Failed(_)), "read with normal rights: {:?}", r.memory_integrity);
        assert!(!matches!(r.blocklist, Reading::Failed(_)), "read with normal rights: {:?}", r.blocklist);
    }
}
