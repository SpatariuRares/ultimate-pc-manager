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
use crate::platform::install_queue::{Request, Work};
use crate::platform::series::{Run, Series};
use crate::platform::{AddonContext, Generation};

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
    /// 0 boot, 1 system, 2 automatic, 3 manual, 4 disabled
    pub start_code: u32,
    pub kind: KernelKind,
    pub path: String,
    pub version: String,
    pub vendor: String,
    pub group: String,
    pub image_size: Option<u64>,
    pub load_index: Option<u32>,
}

impl KernelDriver {
    pub fn third_party(&self) -> bool {
        !self.vendor.is_empty() && !self.vendor.to_lowercase().starts_with("microsoft")
    }
    /// Whether "Unload" is offered and, if not, why.
    pub fn unload_note(&self, admin: bool) -> (bool, &'static str) {
        if !self.running {
            (false, "Not loaded.")
        } else if self.start_code <= 1 {
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
            Self::Loaded => d.running,
            Self::NotLoaded => !d.running,
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
    /// Bytes held by loaded driver images.
    pub memory: u64,
}

impl KernelFigures {
    pub fn of(list: &[KernelDriver]) -> Self {
        Self {
            drivers: list.len(),
            loaded: KernelScope::Loaded.count(list),
            third_party: KernelScope::ThirdParty.count(list),
            memory: list.iter().filter(|d| d.running).filter_map(|d| d.image_size).sum(),
        }
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
    pub devices: Vec<DeviceDriver>,
    pub kernel: Vec<KernelDriver>,
    pub listed_at: String,
    pub listing: bool,
    /// Why the last read of the devices failed (then `devices` is empty).
    pub devices_error: Option<String>,
    /// Why the last read of the driver services failed (then `kernel` is empty).
    pub kernel_error: Option<String>,
    /// None until the first check.
    pub updates: Option<Vec<DriverUpdate>>,
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
}

#[derive(Clone)]
pub struct Hub {
    st: Arc<Mutex<State>>,
    /// Every PowerShell run goes through the platform's executor.
    ctx: AddonContext,
    /// Runs the listing, the search and the installs one at a time.
    /// Stopped once by [`Hub::stop`]: from then on its work publishes
    /// nothing and no new work starts; driver installs are not cut short.
    series: Series<State>,
    /// Set by [`Hub::stop`]: ends a running Windows Update search's PowerShell.
    halt: Arc<AtomicBool>,
}

impl Hub {
    pub fn new(ctx: AddonContext) -> Self {
        let st = Arc::new(Mutex::new(State::default()));
        let series = Series::new(&st, &Generation::default(), |s: &mut State| &mut s.busy);
        Hub { st, ctx, series, halt: Default::default() }
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
            st.listing = true;
        }
        self.spawn(None, false, |hub, _| hub.refresh_now());
    }

    fn refresh_now(&self) {
        if self.live().is_none() {
            return;
        }
        let devices = imp::list_devices();
        let kernel = imp::list_kernel();
        let Some(mut st) = self.live() else { return };
        publish_listing(&mut st, devices, kernel);
        st.listed_at = crate::pro::clock_now();
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
            publish_check(&mut st, result.and_then(|v| parse_search(&v)), by_hand, &crate::pro::clock_now());
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
                if let Some(u) = st.updates.as_mut() {
                    u.retain(|x| !done.contains(&x.id));
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
  $r = $s.CreateUpdateSearcher().Search("IsInstalled=0 and Type='Driver'")
  $col = New-Object -ComObject Microsoft.Update.UpdateColl
  foreach ($u in $r.Updates) {
    if ($want -contains $u.Identity.UpdateID) {
      if (-not $u.EulaAccepted) { $u.AcceptEula() }
      [void]$col.Add($u)
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
$res = [ordered]@{ ok = $false; error = ''; code = 0; output = '' }
try {
  switch ($Mode) {
    'inf' {
      if (Test-Path -LiteralPath $Arg -PathType Container) {
        $o = & pnputil.exe /add-driver (Join-Path $Arg '*.inf') /subdirs /install 2>&1
      } else {
        $o = & pnputil.exe /add-driver $Arg /install 2>&1
      }
    }
    'scan' { $o = & pnputil.exe /scan-devices 2>&1 }
    'stop' { $o = & sc.exe stop $Arg 2>&1 }
    default { throw "Unknown mode $Mode" }
  }
  $res.code = $LASTEXITCODE
  $res.output = ($o | Out-String).Trim()
  $res.ok = $true
} catch { $res.error = $_.Exception.Message }
ConvertTo-Json -InputObject $res -Depth 5 | Out-File -FilePath $Out -Encoding utf8
"#;

/// Puts the outcome of a read of devices and driver services into `st`: a
/// failed read leaves its list empty and keeps the reason, so the page can
/// tell an error from Windows reporting nothing.
fn publish_listing(st: &mut State, devices: Result<Vec<DeviceDriver>, String>, kernel: Result<Vec<KernelDriver>, String>) {
    /// A read's rows and why it failed: no rows on failure.
    fn split<T>(read: Result<Vec<T>, String>) -> (Vec<T>, Option<String>) {
        read.map_or_else(|e| (Vec::new(), Some(e)), |list| (list, None))
    }
    (st.devices, st.devices_error) = split(devices);
    (st.kernel, st.kernel_error) = split(kernel);
    st.listing = false;
    let State { devices, updates, .. } = st;
    if let Some(u) = updates {
        match_updates(devices, u);
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
            st.updates = Some(list);
        }
        Err(e) => st.message = Some((format!("Could not check Windows Update: {e}"), true)),
    }
}

// -------------------------------------------------------------- platform

#[cfg(not(windows))]
mod imp {
    use super::*;
    pub fn list_devices() -> Result<Vec<DeviceDriver>, String> {
        Ok(Vec::new())
    }
    pub fn list_kernel() -> Result<Vec<KernelDriver>, String> {
        Ok(Vec::new())
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::collections::HashMap;
    use std::ffi::c_void;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows_sys::Win32::Devices::Properties::*;
    use windows_sys::Win32::Foundation::DEVPROPKEY;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    use windows_sys::Win32::System::Services::*;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        let mut n = 0;
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, n))
    }

    // ------------------------------------------------------- devices

    unsafe fn prop(dn: u32, key: &DEVPROPKEY) -> Option<(u32, Vec<u8>)> {
        let (mut ty, mut size) = (0u32, 0u32);
        CM_Get_DevNode_PropertyW(dn, key, &mut ty, null_mut(), &mut size, 0);
        if size == 0 {
            return None;
        }
        let mut b = vec![0u8; size as usize];
        (CM_Get_DevNode_PropertyW(dn, key, &mut ty, b.as_mut_ptr(), &mut size, 0) == CR_SUCCESS).then_some((ty, b))
    }

    fn utf16(b: &[u8]) -> Vec<u16> {
        b.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }

    unsafe fn prop_str(dn: u32, key: &DEVPROPKEY) -> String {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_STRING, b)) => {
                let w = utf16(&b);
                let end = w.iter().position(|&c| c == 0).unwrap_or(w.len());
                String::from_utf16_lossy(&w[..end]).trim().to_string()
            }
            _ => String::new(),
        }
    }

    unsafe fn prop_list(dn: u32, key: &DEVPROPKEY) -> Vec<String> {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_STRING_LIST, b)) => utf16(&b)
                .split(|&c| c == 0)
                .filter(|s| !s.is_empty())
                .map(String::from_utf16_lossy)
                .collect(),
            _ => Vec::new(),
        }
    }

    unsafe fn prop_date(dn: u32, key: &DEVPROPKEY) -> String {
        match prop(dn, key) {
            Some((DEVPROP_TYPE_FILETIME, b)) if b.len() >= 8 => {
                let ft = u64::from_le_bytes(b[..8].try_into().unwrap_or([0; 8]));
                if ft == 0 {
                    return String::new();
                }
                // 100 ns ticks since 1601-01-01 → days since 1970-01-01.
                let days = (ft / 864_000_000_000) as i64 - 134_774;
                let (y, m, d) = civil(days);
                format!("{y:04}-{m:02}-{d:02}")
            }
            _ => String::new(),
        }
    }

    /// Days since 1970-01-01 → (year, month, day).
    fn civil(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
    }

    pub fn list_devices() -> Result<Vec<DeviceDriver>, String> {
        let mut out = Vec::new();
        unsafe {
            let mut len = 0u32;
            let cr = CM_Get_Device_ID_List_SizeW(&mut len, null(), CM_GETIDLIST_FILTER_NONE);
            if cr != CR_SUCCESS {
                return Err(format!("the configuration manager could not size the device list (CONFIGRET {cr:#x})"));
            }
            if len == 0 {
                return Ok(out);
            }
            let mut buf = vec![0u16; len as usize];
            let cr = CM_Get_Device_ID_ListW(null(), buf.as_mut_ptr(), len, CM_GETIDLIST_FILTER_NONE);
            if cr != CR_SUCCESS {
                return Err(format!("the configuration manager could not list the devices (CONFIGRET {cr:#x})"));
            }
            for id in buf.split(|&c| c == 0).filter(|s| !s.is_empty()) {
                let mut wid = id.to_vec();
                wid.push(0);
                let mut dn = 0u32;
                if CM_Locate_DevNodeW(&mut dn, wid.as_ptr(), CM_LOCATE_DEVNODE_PHANTOM) != CR_SUCCESS {
                    continue;
                }
                // Devices without a driver package have nothing to update.
                let inf = prop_str(dn, &DEVPKEY_Device_DriverInfPath);
                if inf.is_empty() {
                    continue;
                }
                let (mut status, mut problem) = (0u32, 0u32);
                let present = CM_Get_DevNode_Status(&mut status, &mut problem, dn, 0) == CR_SUCCESS;
                let mut name = prop_str(dn, &DEVPKEY_Device_FriendlyName);
                if name.is_empty() {
                    name = prop_str(dn, &DEVPKEY_Device_DeviceDesc);
                }
                out.push(DeviceDriver {
                    instance_id: String::from_utf16_lossy(id),
                    name,
                    class: prop_str(dn, &DEVPKEY_Device_Class),
                    manufacturer: prop_str(dn, &DEVPKEY_Device_Manufacturer),
                    provider: prop_str(dn, &DEVPKEY_Device_DriverProvider),
                    version: prop_str(dn, &DEVPKEY_Device_DriverVersion),
                    date: prop_date(dn, &DEVPKEY_Device_DriverDate),
                    inf,
                    service: prop_str(dn, &DEVPKEY_Device_Service),
                    hardware_ids: prop_list(dn, &DEVPKEY_Device_HardwareIds),
                    compatible_ids: prop_list(dn, &DEVPKEY_Device_CompatibleIds),
                    present,
                    started: present && status & DN_STARTED != 0,
                    problem: if present && status & DN_HAS_PROBLEM != 0 { problem } else { 0 },
                });
            }
        }
        out.sort_by(|a, b| (a.class.to_lowercase(), a.name.to_lowercase()).cmp(&(b.class.to_lowercase(), b.name.to_lowercase())));
        Ok(out)
    }

    // ------------------------------------------------ kernel drivers

    /// "@%SystemRoot%\System32\drivers\tcpipreg.sys,-10110" -> the text it
    /// points to. Some entries carry a stray trailing comma; when nothing
    /// can be loaded the service name stands in.
    fn indirect(s: &str, service: &str) -> String {
        if !s.starts_with('@') {
            return if s.is_empty() { service.to_string() } else { s.to_string() };
        }
        let clean = s.trim_end_matches([',', ';', ' ']);
        for candidate in [s, clean] {
            unsafe {
                let w = wide(candidate);
                let mut out = vec![0u16; 512];
                if windows_sys::Win32::UI::Shell::SHLoadIndirectString(w.as_ptr(), out.as_mut_ptr(), 512, null()) == 0 {
                    let t = from_wide(out.as_ptr());
                    if !t.is_empty() {
                        return t;
                    }
                }
            }
        }
        service.to_string()
    }

    /// "\SystemRoot\System32\drivers\ACPI.sys", "System32\drivers\x.sys",
    /// "\??\C:\…" or nothing → a normal path.
    fn resolve_image(path: &str, service: &str) -> String {
        let windir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
        let p = path.trim().trim_matches('"');
        if p.is_empty() {
            return format!(r"{windir}\System32\drivers\{service}.sys");
        }
        let lower = p.to_lowercase();
        if let Some(rest) = lower.strip_prefix(r"\systemroot\") {
            return format!(r"{windir}\{}", &p[p.len() - rest.len()..]);
        }
        if let Some(rest) = p.strip_prefix(r"\??\") {
            return rest.to_string();
        }
        if lower.starts_with("system32\\") || lower.starts_with("syswow64\\") {
            return format!(r"{windir}\{p}");
        }
        p.to_string()
    }

    /// File version ("10.0.26100.1") and CompanyName from the version resource.
    pub fn file_version(path: &str) -> (String, String) {
        unsafe {
            let w = wide(path);
            let mut handle = 0u32;
            let size = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
            if size == 0 {
                return (String::new(), String::new());
            }
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
                return (String::new(), String::new());
            }
            let mut version = String::new();
            let (mut p, mut len): (*mut c_void, u32) = (null_mut(), 0);
            let root = wide("\\");
            if VerQueryValueW(data.as_ptr() as *const c_void, root.as_ptr(), &mut p, &mut len) != 0 && len >= 16 {
                let f = p as *const u32;
                let (ms, ls) = (*f.add(2), *f.add(3));
                version = format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff);
            }
            let mut company = String::new();
            let tr = wide("\\VarFileInfo\\Translation");
            let mut langs: Vec<String> = Vec::new();
            if VerQueryValueW(data.as_ptr() as *const c_void, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
                let t = p as *const u16;
                langs.push(format!("{:04x}{:04x}", *t, *t.add(1)));
            }
            langs.extend(["040904b0".to_string(), "040904e4".to_string(), "000004b0".to_string()]);
            for l in langs {
                let key = wide(&format!("\\StringFileInfo\\{l}\\CompanyName"));
                if VerQueryValueW(data.as_ptr() as *const c_void, key.as_ptr(), &mut p, &mut len) != 0 && len > 0 {
                    company = from_wide(p as *const u16).trim().to_string();
                    if !company.is_empty() {
                        break;
                    }
                }
            }
            (version, company)
        }
    }

    /// Loaded kernel images by lowercase file name → (size, load order index),
    /// from NtQuerySystemInformation(SystemModuleInformation).
    fn kernel_modules() -> HashMap<String, (u64, u32)> {
        let mut map = HashMap::new();
        unsafe {
            let lib = wide("ntdll.dll");
            let h = LoadLibraryW(lib.as_ptr());
            if h.is_null() {
                return map;
            }
            let Some(f) = GetProcAddress(h, b"NtQuerySystemInformation\0".as_ptr()) else { return map };
            let query: unsafe extern "system" fn(u32, *mut c_void, u32, *mut u32) -> i32 = std::mem::transmute(f);
            let mut len = 0u32;
            query(11, null_mut(), 0, &mut len);
            if len == 0 {
                return map;
            }
            let mut buf = vec![0u64; (len as usize + 65536) / 8];
            if query(11, buf.as_mut_ptr() as *mut c_void, (buf.len() * 8) as u32, &mut len) < 0 {
                return map;
            }
            let base = buf.as_ptr() as *const u8;
            let count = *(base as *const u32) as usize;
            // RTL_PROCESS_MODULE_INFORMATION is 296 bytes; the list starts at 8.
            for i in 0..count {
                let e = base.add(8 + i * 296);
                if 8 + (i + 1) * 296 > buf.len() * 8 {
                    break;
                }
                let size = *(e.add(24) as *const u32) as u64;
                let index = *(e.add(32) as *const u16) as u32;
                let off = *(e.add(38) as *const u16) as usize;
                let path = std::slice::from_raw_parts(e.add(40), 256);
                let end = path.iter().position(|&b| b == 0).unwrap_or(256);
                let name = String::from_utf8_lossy(&path[off.min(end)..end]).to_lowercase();
                map.insert(name, (size, index));
            }
        }
        map
    }

    pub fn list_kernel() -> Result<Vec<KernelDriver>, String> {
        let modules = kernel_modules();
        let mut out = Vec::new();
        unsafe {
            let scm = OpenSCManagerW(null(), null(), SC_MANAGER_ENUMERATE_SERVICE | SC_MANAGER_CONNECT);
            if scm.is_null() {
                return Err(format!("the service control manager could not be opened: {}", std::io::Error::last_os_error()));
            }
            let (mut needed, mut count, mut resume) = (0u32, 0u32, 0u32);
            EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_DRIVER, SERVICE_STATE_ALL, null_mut(), 0, &mut needed, &mut count, &mut resume, null());
            let mut buf = vec![0u8; needed as usize + 4096];
            resume = 0;
            if EnumServicesStatusExW(scm, SC_ENUM_PROCESS_INFO, SERVICE_DRIVER, SERVICE_STATE_ALL, buf.as_mut_ptr(), buf.len() as u32, &mut needed, &mut count, &mut resume, null()) == 0 {
                let e = std::io::Error::last_os_error();
                CloseServiceHandle(scm);
                return Err(format!("the service control manager could not list the driver services: {e}"));
            }
            let items = std::slice::from_raw_parts(buf.as_ptr() as *const ENUM_SERVICE_STATUS_PROCESSW, count as usize);
            let mut cfg = vec![0u64; 1024];
            for it in items {
                let service = from_wide(it.lpServiceName);
                let st = &it.ServiceStatusProcess;
                let mut d = KernelDriver {
                    display: indirect(&from_wide(it.lpDisplayName), &service),
                    running: st.dwCurrentState == SERVICE_RUNNING,
                    kind: match st.dwServiceType {
                        t if t & SERVICE_FILE_SYSTEM_DRIVER != 0 => KernelKind::FileSystem,
                        t if t & SERVICE_KERNEL_DRIVER != 0 => KernelKind::Kernel,
                        t if t & 8 != 0 => KernelKind::Recognizer,
                        t if t & 4 != 0 => KernelKind::Adapter,
                        _ => KernelKind::Kernel,
                    },
                    start_type: "",
                    start_code: 3,
                    ..Default::default()
                };
                let mut raw_path = String::new();
                let h = OpenServiceW(scm, it.lpServiceName, SERVICE_QUERY_CONFIG);
                if !h.is_null() {
                    let mut need = 0u32;
                    if QueryServiceConfigW(h, cfg.as_mut_ptr() as *mut QUERY_SERVICE_CONFIGW, (cfg.len() * 8) as u32, &mut need) != 0 {
                        let c = &*(cfg.as_ptr() as *const QUERY_SERVICE_CONFIGW);
                        d.start_code = c.dwStartType;
                        raw_path = from_wide(c.lpBinaryPathName);
                        d.group = from_wide(c.lpLoadOrderGroup);
                    }
                    CloseServiceHandle(h);
                }
                d.start_type = match d.start_code {
                    0 => "Boot",
                    1 => "System",
                    2 => "Automatic",
                    3 => "Manual",
                    4 => "Disabled",
                    _ => "",
                };
                d.path = resolve_image(&raw_path, &service);
                if std::path::Path::new(&d.path).exists() {
                    let (v, c) = file_version(&d.path);
                    d.version = v;
                    d.vendor = c;
                }
                if d.running {
                    let file = d.path.rsplit('\\').next().unwrap_or("").to_lowercase();
                    if let Some((size, index)) = modules.get(&file) {
                        d.image_size = Some(*size);
                        d.load_index = Some(*index);
                    }
                }
                d.service = service;
                out.push(d);
            }
            CloseServiceHandle(scm);
        }
        out.sort_by_key(|d| d.display.to_lowercase());
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let hub = Hub::new(ctx);
        hub.check_updates();
        wait_until_idle(&hub);
        assert_eq!(runner.launches(), vec![Launch::PowerShell { script: SEARCH_SCRIPT.into(), args: vec![], elevated: false }]);
        assert!(hub.lock().message.as_ref().is_some_and(|(_, error)| *error), "the failure is shown");
    }

    #[test]
    fn scanning_asks_for_an_elevated_powershell_through_the_context() {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx);
        hub.scan();
        wait_until_idle(&hub);
        assert_eq!(
            runner.launches(),
            vec![Launch::PowerShell { script: TOOL_SCRIPT.into(), args: vec![("Mode".into(), "scan".into()), ("Arg".into(), String::new())], elevated: true }]
        );
    }

    /// Reads this PC's drivers: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn lists_this_pc() {
        let devices = imp::list_devices().expect("devices listed");
        let kernel = imp::list_kernel().expect("driver services listed");
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
        let devices = imp::list_devices().expect("devices listed");
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
        assert_eq!(f, KernelFigures { drivers: 6, loaded: 3, third_party: 2, memory: 43_100_000 });
    }

    #[test]
    fn kernel_memory_counts_only_loaded_images() {
        let mut list = sample();
        list[3].image_size = Some(9_999);
        assert_eq!(KernelFigures::of(&list).memory, 43_100_000);
    }

    #[test]
    fn four_figures_of_an_empty_list() {
        assert_eq!(KernelFigures::of(&[]), KernelFigures::default());
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
        State { devices: vec![dev], ..Default::default() }
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
        assert_eq!(st.updates.as_ref().map(Vec::len), Some(0), "but the page shows its outcome");
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
        let mut st = State { kernel: vec![KernelDriver::default()], ..Default::default() };
        publish_listing(&mut st, Ok(vec![]), Err("gone".into()));
        assert!(st.kernel.is_empty(), "rows of an older read do not pass for this one");
    }
}

/// Outcomes of the elevated actions, read from what their scripts report.
#[cfg(test)]
mod outcome_tests {
    use super::*;
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
        let hub = Hub::new(ctx);
        hub.install_inf(r"C:\drivers\x.inf".into());
        wait_until_idle(&hub);
        assert_eq!(hub.lock().message, Some((CANCELLED.to_string(), false)));
        hub.install(vec!["a".into()]);
        wait_until_idle(&hub);
        assert_eq!(hub.lock().message, Some((CANCELLED.to_string(), false)));
    }

    fn kernel(running: bool, start_code: u32) -> KernelDriver {
        KernelDriver { running, start_code, ..Default::default() }
    }

    #[test]
    fn unload_is_not_offered_for_a_driver_that_is_not_loaded() {
        assert_eq!(kernel(false, 3).unload_note(true), (false, "Not loaded."));
        assert_eq!(kernel(false, 0).unload_note(false), (false, "Not loaded."));
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
