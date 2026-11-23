//! The Drivers page, design A ("update center", see the Drivers page
//! redesign canvas): a status band that answers "are my drivers up to
//! date?" with one main action, then the drivers people actually update
//! (third-party hardware drivers), grouped by kind with duplicates folded,
//! and a side panel for the selected driver. Windows built-in drivers and
//! kernel modules are one click away.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui};

use super::filters::{self, FilterDriver, FilterRead};
use super::engine::{self as drivers, DeviceDriver, DriverUpdate, HashFor, Hub, KernelDriver, KernelFigures, KernelScope};
use super::engine::virustotal_url;
use super::problems;
use super::signature::{signature_text, Signature, SignatureBook};
use super::store::{self, Flags, Selection, StoreFigures, StoreList, StorePackage};
use crate::addons::plural;
use crate::platform::{csv_field, csv_text_field, Action, AddonContext};
use crate::theme;
use crate::widgets::parts::{self, BandArea, Mark, StatusBand};
use crate::widgets::{long_date, parse_date, separator, short_time, split_version, tick};
use crate::page_perf::{display, fit};

#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// Third-party drivers: the ones people update.
    #[default]
    Hardware,
    /// Drivers that ship with Windows.
    Builtin,
    /// Devices with a Configuration Manager problem code.
    Problems,
    /// Kernel driver services.
    Kernel,
    /// The driver packages Windows keeps.
    Store,
    /// The loaded file-system minifilters (read-only).
    Filters,
}

/// Shown in a confirm dialog while another driver job holds the page.
const BUSY_TEXT: &str = "Another driver job is running. Wait for it to finish.";

/// A device action the user asked for and has not yet confirmed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceConfirm {
    pub action: drivers::DeviceAction,
    pub instance_id: String,
    pub name: String,
    /// The stronger warning, for a protected class; the confirm button stays
    /// off until `understood` is ticked.
    pub warning: Option<String>,
    pub understood: bool,
}

impl DeviceConfirm {
    pub fn new(action: drivers::DeviceAction, d: &DeviceDriver) -> Self {
        DeviceConfirm { action, instance_id: d.instance_id.clone(), name: d.name.clone(), warning: drivers::strong_warning(action, &d.class), understood: false }
    }

    /// May the action run: no warning, or the user ticked it.
    pub fn confirmable(&self) -> bool {
        self.warning.is_none() || self.understood
    }
}

/// What the page remembers between frames.
#[derive(Default)]
pub struct DriversUi {
    pub scope: Scope,
    /// The kernel module list's scope.
    pub kernel_filter: KernelScope,
    pub search: String,
    pub show_disconnected: bool,
    pub selected_device: Option<String>,
    pub selected_kernel: Option<String>,
    /// The published name ("oem42.inf") of the selected store package.
    pub selected_store: Option<String>,
    /// The packages ticked on the Driver store tab.
    pub ticked: Selection,
    /// A removal waiting for its confirmation.
    pub confirm_remove: Option<RemoveAsk>,
    /// A backup waiting for its folder and confirmation.
    pub confirm_backup: Option<BackupAsk>,
    /// The folder the last backup was made in (the dated subfolder goes inside).
    pub backup_folder: String,
    pub confirm_unload: Option<String>,
    /// The SHA-256 of a file waiting for the first-use consent to be sent
    /// to VirusTotal.
    pub confirm_virustotal: Option<String>,
    /// An offered driver that is not newer, waiting for "Install anyway".
    pub confirm_older: Option<DriverUpdate>,
    /// An install waiting for its confirm dialog (with the restore point option).
    pub confirm_job: Option<drivers::Redo>,
    /// A device action waiting for its confirm dialog.
    pub confirm_device: Option<DeviceConfirm>,
    /// Bring the selected row into view on the next frame.
    pub scroll_to_selected: bool,
    pub started: bool,
}


/// What a removal waiting for its confirmation removes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveKind {
    /// The ticked packages, or the one selected.
    Selected,
    /// "Remove old packages".
    Old,
    /// "Remove even if in use": one package, forced.
    Forced,
}

/// A removal waiting for its confirmation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveAsk {
    pub kind: RemoveKind,
    /// The published names it would remove.
    pub names: Vec<String>,
    /// "I understand, continue anyway" (the forced removal only).
    pub understood: bool,
}

/// A backup waiting for its folder: the names, or every third-party package.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupAsk {
    pub names: Option<Vec<String>>,
    pub folder: String,
}

/// Third-party drivers this many years old or more get "check the maker",
/// unless the user chose another threshold.
pub const DEFAULT_OLD_YEARS: i64 = 3;

fn text(size: f32) -> FontId {
    FontId::proportional(size)
}

fn fmt_size(b: u64) -> String {
    let mut v = b as f64;
    let mut unit = "B";
    for u in ["KB", "MB", "GB"] {
        if v < 1024.0 {
            break;
        }
        v /= 1024.0;
        unit = u;
    }
    if unit == "B" { format!("{v:.0} B") } else { format!("{v:.1} {unit}") }
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

fn today() -> (i64, u32, u32) {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    civil((secs / 86_400) as i64)
}

/// Whole years between a "yyyy-mm-dd" date and `today`.
fn years_between(s: &str, today: (i64, u32, u32)) -> Option<i64> {
    let (y, m, d) = parse_date(s)?;
    let (ty, tm, td) = today;
    Some(ty - y - if (tm, td) < (m, d) { 1 } else { 0 })
}

/// Windows' own drivers carry the placeholder date 2006-06-21; showing it
/// makes them look ancient, so the date reads "With Windows".
fn with_windows(d: &DeviceDriver) -> bool {
    !d.third_party() && parse_date(&d.date).is_some_and(|(y, _, _)| y <= 2006)
}

/// How old a third-party driver is, when it is at least `threshold` years
/// old on `today`: the page then says "check the maker".
fn check_the_maker(d: &DeviceDriver, today: (i64, u32, u32), threshold: i64) -> Option<i64> {
    years_between(&d.date, today).filter(|y| d.third_party() && *y >= threshold)
}

/// The kind of device, in words people use, for grouping the list.
fn kind_of(class: &str) -> &'static str {
    match class.to_ascii_lowercase().as_str() {
        "display" => "Display",
        "net" | "netservice" | "nettrans" | "netclient" => "Network",
        "media" | "audioendpoint" | "audioprocessingobject" => "Sound",
        "hidclass" | "keyboard" | "mouse" | "xnacomposite" | "xboxcomposite" => "Mice, keyboards and controllers",
        "bluetooth" => "Bluetooth",
        "camera" | "image" => "Cameras",
        "printer" | "printqueue" | "wsdprintdevice" => "Printers",
        "scsiadapter" | "hdc" | "diskdrive" | "volume" | "volumesnapshot" | "scmdisk" => "Storage",
        "usb" | "usbdevice" => "USB",
        "system" | "processor" | "computer" | "firmware" => "Chipset and system",
        "softwarecomponent" | "softwaredevice" => "Software components",
        "monitor" => "Monitors",
        "securitydevices" | "smartcardreader" => "Security",
        "ports" => "Ports",
        _ => "Other",
    }
}

fn kind_rank(kind: &str) -> usize {
    [
        "Display", "Network", "Sound", "Mice, keyboards and controllers", "Bluetooth", "Cameras", "Printers", "Storage", "USB",
        "Monitors", "Ports", "Security", "Chipset and system", "Software components", "Other",
    ]
    .iter()
    .position(|k| *k == kind)
    .unwrap_or(99)
}

/// What a driver list says in place of its rows when it has none to show.
#[derive(Clone, PartialEq, Eq, Debug)]
enum ListNote {
    /// The first read is still running.
    Reading,
    /// The read failed, for this reason.
    Failed(String),
    /// Windows reported no drivers at all.
    NothingReported,
    /// The scope or filter leaves nothing.
    FilteredOut,
    /// The search leaves nothing.
    NoMatch,
}

impl ListNote {
    /// The title and the detail line for `scope`.
    fn texts(&self, scope: Scope) -> (String, Option<String>) {
        if scope == Scope::Store {
            return self.store_texts();
        }
        let kernel = scope == Scope::Kernel;
        let title = match self {
            ListNote::Reading if kernel => "Asking the service control manager for every driver registered on this machine.",
            ListNote::Reading => "Reading the drivers\u{2026}",
            ListNote::Failed(reason) => return ("Drivers could not be enumerated".into(), Some(reason.clone())),
            ListNote::NothingReported => return ("No drivers were reported".into(), Some("Windows published no driver registrations.".into())),
            ListNote::FilteredOut if kernel => "No matching drivers.",
            ListNote::FilteredOut if scope == Scope::Problems => "No device has a problem.",
            ListNote::FilteredOut => "No drivers here.",
            ListNote::NoMatch if kernel => KERNEL_NO_MATCH,
            ListNote::NoMatch => "No driver or maker matches what you typed.",
        };
        (title.into(), None)
    }
}

impl ListNote {
    /// The title and the detail line on the Driver store tab.
    fn store_texts(&self) -> (String, Option<String>) {
        let title = match self {
            ListNote::Reading => "Reading the driver store\u{2026}",
            ListNote::Failed(reason) => return ("The driver store could not be read".into(), Some(reason.clone())),
            ListNote::NothingReported => return ("The driver store holds no packages".into(), Some("Windows reported no third-party driver packages.".into())),
            ListNote::FilteredOut => "No packages here.",
            ListNote::NoMatch => STORE_NO_MATCH,
        };
        (title.into(), None)
    }
}

/// The empty-filter text on the Driver store tab.
const STORE_NO_MATCH: &str = "No package, INF, provider or device matches what you typed.";

/// The note a list shows while `shown` of the `total` drivers its read
/// returned are listed (`total` before scope, filter and search); `error`
/// is why that read failed.
fn list_note(listing: bool, error: Option<&str>, total: usize, shown: usize, searching: bool) -> Option<ListNote> {
    Some(if shown > 0 {
        return None;
    } else if listing && total == 0 {
        ListNote::Reading
    } else if let Some(e) = error {
        ListNote::Failed(e.to_string())
    } else if total == 0 {
        ListNote::NothingReported
    } else if searching {
        ListNote::NoMatch
    } else {
        ListNote::FilteredOut
    })
}

/// Draws the note in place of a list's rows.
/// The filter hints of the two lists.
const DEVICE_FILTER_HINT: &str = "Find a driver or maker";
const KERNEL_FILTER_HINT: &str = "Driver name, service or vendor";

/// The filter hint of the Filter drivers tab.
const FILTER_NAME_HINT: &str = "Filter name";

/// How many filters a read found (the tab's count).
fn filters_count(read: &FilterRead) -> usize {
    match read {
        FilterRead::Filters(list) => list.len(),
        _ => 0,
    }
}

/// The filters whose name contains the lowercase text `q` (empty: all).
fn filter_rows<'a>(list: &'a [FilterDriver], q: &str) -> Vec<&'a FilterDriver> {
    list.iter().filter(|f| q.is_empty() || f.name.to_lowercase().contains(q)).collect()
}

/// What the Filter drivers tab says in place of its rows: the title and the
/// detail line; None when there are rows to show.
fn filter_note(read: Option<&FilterRead>, shown: usize, searching: bool) -> Option<(String, Option<String>)> {
    match read {
        None => Some(("Reading the filters\u{2026}".into(), None)),
        Some(FilterRead::NeedsAdmin) => Some((filters::NEEDS_ADMIN.into(), None)),
        Some(FilterRead::Failed(why)) => Some(("Filters could not be listed".into(), Some(why.clone()))),
        Some(FilterRead::Filters(_)) if shown > 0 => None,
        Some(FilterRead::Filters(_)) if searching => Some(("No filter matches what you typed.".into(), None)),
        Some(FilterRead::Filters(_)) => Some(("No filters are loaded.".into(), None)),
    }
}

/// What the side panel says while nothing is selected.
const DEVICE_EMPTY: &str = "Select a driver to see its details and what Windows Update offers for it.";
const KERNEL_EMPTY: &str = "Select a kernel module to see where it lives, how much memory it holds and whether it can be unloaded.";

fn empty_panel(ui: &mut Ui, text: &str) {
    let th = theme::of_ui(ui);
    parts::SidePanel::new("drv-side").show(ui, Some(()), |ui, ()| {
        ui.add(egui::Label::new(RichText::new(text).color(th.label)).wrap());
    });
}

/// What "Open file location" does for `path`: File Explorer selects the
/// file, or the status bar says why it cannot.
fn reveal(path: &str, exists: impl Fn(&std::path::Path) -> bool) -> Action {
    if path.trim().is_empty() {
        return Action::Say("The executable path is not available for this item.".into());
    }
    let file = std::path::PathBuf::from(path);
    if exists(&file) {
        Action::ShowInExplorer(file)
    } else {
        Action::Say(format!("The file {path} was not found."))
    }
}

fn show_note(ui: &mut Ui, note: &ListNote, scope: Scope) {
    let th = theme::of_ui(ui);
    let (title, detail) = note.texts(scope);
    let red = matches!(note, ListNote::Failed(_));
    ui.label(RichText::new(title).color(if red { th.red } else { th.label }));
    if let Some(detail) = detail {
        ui.label(RichText::new(detail).size(12.0).color(th.faint));
    }
}

/// Graphics makers publish newer drivers on their own site first.
fn maker_downloads(d: &DeviceDriver) -> Option<(&'static str, &'static str)> {
    if kind_of(&d.class) != "Display" {
        return None;
    }
    let p = d.provider.to_lowercase();
    if p.contains("nvidia") {
        Some(("NVIDIA", "https://www.nvidia.com/Download/index.aspx"))
    } else if p.contains("advanced micro") || p == "amd" {
        Some(("AMD", "https://www.amd.com/en/support/download/drivers.html"))
    } else if p.contains("intel") {
        Some(("Intel", "https://www.intel.com/content/www/us/en/download-center/home.html"))
    } else {
        None
    }
}

/// What a device row says on its second line, first match wins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sub {
    /// Windows Update offers an older driver than the installed one.
    OlderOffer,
    /// A third-party driver old enough for "check the maker".
    CheckMaker,
    /// Identical entries folded into this row.
    Folded,
    NotConnected,
    Problem,
}

/// One device row: the first of a run of identical entries.
struct DeviceRow {
    /// Index into the devices snapshot.
    dev: usize,
    /// Identical entries folded into this row.
    count: usize,
    /// Index of the update Windows Update offers for it.
    update: Option<usize>,
    /// Years old, when "check the maker" applies.
    check: Option<i64>,
    sub: Option<Sub>,
}

/// One line of the device list: a kind's heading or a driver.
enum Item {
    Kind { name: &'static str, count: usize },
    Device(DeviceRow),
}

impl Item {
    fn height(&self) -> f32 {
        match self {
            Item::Kind { .. } => 32.0,
            Item::Device(r) if r.sub.is_some() => 40.0,
            Item::Device(_) => 30.0,
        }
    }
}

/// What the derived data depends on besides the lists.
#[derive(Clone, PartialEq)]
struct Inputs {
    search: String,
    show_disconnected: bool,
    scope: Scope,
    kernel_filter: KernelScope,
    old_years: i64,
    today: (i64, u32, u32),
}

/// A kernel module matches the lowercase text filter `q` (empty: all).
fn kernel_text_matches(d: &KernelDriver, q: &str) -> bool {
    q.is_empty() || [&d.display, &d.service, &d.vendor].iter().any(|s| s.to_lowercase().contains(q))
}

/// Kernel memory as a tile shows it.
fn kernel_memory_text(memory: Option<u64>) -> String {
    memory.map(fmt_size).unwrap_or_else(|| "an unknown amount".into())
}

/// Said under the tiles when kernel memory could not be worked out.
const KERNEL_MEMORY_NOTE: &str = "The list of loaded kernel modules could not be read, so kernel memory is not known.";

/// Everything the page works out from the lists and the filter: scope
/// counts, the device list's kinds and folded rows, the kernel rows and
/// their figures. It is built once and reused every frame until the data
/// or a filter changes (see [`DriversPage::derived`]); the one place to
/// add a count, a scope or a column's precomputed value.
pub struct Derived {
    devices: Arc<[DeviceDriver]>,
    kernel: Arc<[KernelDriver]>,
    updates: Option<Arc<[DriverUpdate]>>,
    /// The signature results these counts were worked out from.
    signatures: Arc<SignatureBook>,
    inputs: Inputs,
    /// Third-party drivers after folding, for the scope button.
    hardware_count: usize,
    /// Windows built-in drivers after folding, for the scope button.
    builtin_count: usize,
    /// Devices with a problem after folding, for the scope button and the band.
    problem_count: usize,
    /// Devices matching the scope and the search, before folding.
    matching: usize,
    /// The device list, kind by kind (empty on the kernel scope).
    items: Vec<Item>,
    /// Where each item starts, without the gaps between items.
    tops: Vec<f32>,
    /// Indexes into `kernel` left after the scope and the search.
    kernel_rows: Vec<usize>,
    /// Each kernel scope with its count.
    kernel_counts: Vec<(KernelScope, usize)>,
    figures: KernelFigures,
}

impl Derived {
    fn build(devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>, signatures: &Arc<SignatureBook>, inputs: Inputs) -> Derived {
        let scoped = |hardware: bool| -> Vec<usize> {
            devices
                .iter()
                .enumerate()
                .filter(|(_, d)| d.third_party() == hardware && (inputs.show_disconnected || d.present))
                .map(|(i, _)| i)
                .collect()
        };
        let hardware_count = fold(devices, scoped(true)).len();
        let builtin_count = fold(devices, scoped(false)).len();
        let problem_rows: Vec<usize> = devices.iter().enumerate().filter(|(_, d)| problems::is_problem(d)).map(|(i, _)| i).collect();
        let problem_count = fold(devices, problem_rows.clone()).len();
        let q = inputs.search.trim().to_lowercase();

        let mut items = Vec::new();
        let mut matching = 0;
        if matches!(inputs.scope, Scope::Hardware | Scope::Builtin | Scope::Problems) {
            let in_scope = if inputs.scope == Scope::Problems { problem_rows } else { scoped(inputs.scope == Scope::Hardware) };
            let matches: Vec<usize> = in_scope
                .into_iter()
                .filter(|&i| {
                    let d = &devices[i];
                    q.is_empty() || [&d.name, &d.provider, &d.manufacturer, &d.class].iter().any(|s| s.to_lowercase().contains(&q))
                })
                .collect();
            matching = matches.len();
            // The first offer for a device wins.
            let mut offered: HashMap<&str, usize> = HashMap::new();
            for (i, u) in updates.iter().flat_map(|u| u.iter()).enumerate() {
                if let Some(id) = u.device.as_deref() {
                    offered.entry(id).or_insert(i);
                }
            }
            // Group by kind, keep a stable order inside each group.
            let mut groups: Vec<(&'static str, Vec<usize>)> = Vec::new();
            for i in matches {
                let k = kind_of(&devices[i].class);
                match groups.iter_mut().find(|g| g.0 == k) {
                    Some(g) => g.1.push(i),
                    None => groups.push((k, vec![i])),
                }
            }
            groups.sort_by_key(|g| kind_rank(g.0));
            for (kind, mut idx) in groups {
                idx.sort_by_cached_key(|&i| devices[i].name.to_lowercase());
                let rows = fold(devices, idx);
                items.push(Item::Kind { name: kind, count: rows.len() });
                for (dev, count) in rows {
                    let d = &devices[dev];
                    let update = offered.get(d.instance_id.as_str()).copied();
                    let older = update.and_then(|i| updates.as_ref().map(|u| &u[i])).is_some_and(|u| !u.worth_installing());
                    let check = check_the_maker(d, inputs.today, inputs.old_years);
                    let sub = if inputs.scope == Scope::Problems {
                        Some(Sub::Problem)
                    } else if older {
                        Some(Sub::OlderOffer)
                    } else if check.is_some() {
                        Some(Sub::CheckMaker)
                    } else if count > 1 {
                        Some(Sub::Folded)
                    } else if !d.present {
                        Some(Sub::NotConnected)
                    } else if d.problem != 0 {
                        Some(Sub::Problem)
                    } else {
                        None
                    };
                    items.push(Item::Device(DeviceRow { dev, count, update, check, sub }));
                }
            }
        }
        let mut tops = Vec::with_capacity(items.len());
        let mut y = 0.0;
        for it in &items {
            tops.push(y);
            y += it.height();
        }

        let kernel_rows = if inputs.scope == Scope::Kernel {
            kernel
                .iter()
                .enumerate()
                .filter(|(_, d)| inputs.kernel_filter.matches(d, signatures.of(&d.path)))
                .filter(|(_, d)| kernel_text_matches(d, &q))
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        let kernel_counts = KernelScope::SHOWN.iter().map(|&s| (s, s.count(kernel, signatures))).collect();
        Derived {
            devices: devices.clone(),
            kernel: kernel.clone(),
            updates: updates.clone(),
            signatures: signatures.clone(),
            hardware_count,
            builtin_count,
            problem_count,
            matching,
            items,
            tops,
            kernel_rows,
            kernel_counts,
            // The text filter decides the figures, never the scope.
            figures: KernelFigures::of(kernel.iter().filter(|d| kernel_text_matches(d, &q))),
            inputs,
        }
    }

    /// Still right for these lists and this filter.
    fn is_for(&self, devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>, signatures: &Arc<SignatureBook>, inputs: &Inputs) -> bool {
        let same_updates = match (&self.updates, updates) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        Arc::ptr_eq(&self.devices, devices) && Arc::ptr_eq(&self.kernel, kernel) && same_updates && Arc::ptr_eq(&self.signatures, signatures) && self.inputs == *inputs
    }

    fn update_of(&self, row: &DeviceRow) -> Option<&DriverUpdate> {
        row.update.and_then(|i| self.updates.as_ref().map(|u| &u[i]))
    }
}

/// The page and its background work, while the add-on is on.
pub struct DriversPage {
    pub drv: Hub,
    pub drv_ui: DriversUi,
    context: AddonContext,
    /// Status-bar messages for the app to show after the frame.
    pub said: Vec<String>,
    /// "Check Windows Update when the page opens".
    pub check_on_open: bool,
    /// "Create a restore point first": on by default, remembered.
    pub restore_point: bool,
    /// Third-party drivers this many years old get "check the maker".
    pub old_years: i64,
    /// "Look up on VirusTotal" is offered (off by default).
    pub virustotal: bool,
    /// The user agreed to send a file's SHA-256 to VirusTotal.
    pub virustotal_consent: bool,
    /// The data worked out from the lists and the filter, reused until
    /// either changes.
    derived: Option<Arc<Derived>>,
    /// How many times `derived` was built (a test reads it).
    derived_builds: usize,
    /// The Driver store tab's rows and figures, reused until the read or
    /// the filter text changes.
    store_list: Option<Arc<StoreList>>,
    /// How many times `store_list` was built (a test reads it).
    store_builds: usize,
}

impl DriversPage {
    pub fn new(drv: Hub, context: AddonContext) -> Self {
        DriversPage {
            drv,
            drv_ui: DriversUi::default(),
            context,
            said: Vec::new(),
            check_on_open: false,
            restore_point: true,
            old_years: DEFAULT_OLD_YEARS,
            virustotal: false,
            virustotal_consent: false,
            derived: None,
            derived_builds: 0,
            store_list: None,
            store_builds: 0,
        }
    }

    /// How many times the Driver store's rows were worked out so far.
    #[cfg(test)]
    pub fn store_builds(&self) -> usize {
        self.store_builds
    }

    /// The Driver store's rows and figures for this read and the current
    /// filter text: built only when either changed since the last call.
    fn store_view(&mut self, packages: &Arc<[StorePackage]>) -> Arc<StoreList> {
        if let Some(list) = &self.store_list {
            if list.is_for(packages, &self.drv_ui.search) {
                return list.clone();
            }
        }
        let list = Arc::new(StoreList::build(packages, &self.drv_ui.search));
        self.store_builds += 1;
        self.store_list = Some(list.clone());
        list
    }

    /// How many times the counts and groups were worked out so far.
    #[cfg(test)]
    pub fn derived_builds(&self) -> usize {
        self.derived_builds
    }

    /// The counts, groups and rows for these lists and the current filter:
    /// built when the lists or a filter changed since the last call, else
    /// the same data again.
    fn derived(&mut self, devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>, signatures: &Arc<SignatureBook>) -> Arc<Derived> {
        let inputs = Inputs {
            search: self.drv_ui.search.clone(),
            show_disconnected: self.drv_ui.show_disconnected,
            scope: self.drv_ui.scope,
            kernel_filter: self.drv_ui.kernel_filter,
            old_years: self.old_years,
            today: today(),
        };
        if let Some(d) = &self.derived {
            if d.is_for(devices, kernel, updates, signatures, &inputs) {
                return d.clone();
            }
        }
        let d = Arc::new(Derived::build(devices, kernel, updates, signatures, inputs));
        self.derived_builds += 1;
        self.derived = Some(d.clone());
        d
    }

    /// The page is shown: the first time, the list is read (and Windows
    /// Update searched, if "Check Windows Update when the page opens" is on).
    pub fn opened(&mut self) {
        if !self.drv_ui.started {
            self.drv_ui.started = true;
            self.drv.refresh();
            if self.check_on_open {
                self.drv.check_updates_on_open();
            }
        }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.said.push(msg.into());
    }

    /// Opens Device Manager through the add-on's context; a failure is
    /// told in the status bar.
    pub fn open_device_manager(&mut self) {
        if let Err(e) = self.context.open("devmgmt.msc") {
            self.say(e);
        }
    }

    /// Starts hashing the file of a kernel module; a missing path or file is
    /// told at once.
    fn hash_module(&mut self, path: &str, purpose: HashFor, actions: &mut Vec<Action>) {
        match reveal(path, |p| p.exists()) {
            Action::Say(why) => actions.push(Action::Say(why)),
            _ => self.drv.hash_file(path.to_string(), purpose),
        }
    }

    /// Acts on a finished hash: copies it, or goes on to the VirusTotal lookup.
    pub fn settle_hash(&mut self, actions: &mut Vec<Action>) {
        let Some(hashed) = self.drv.take_hashed() else { return };
        match (hashed.result, hashed.purpose) {
            (Err(why), _) => actions.push(Action::Say(why)),
            (Ok(sha), HashFor::Copy) => {
                actions.push(Action::Copy(sha));
                actions.push(Action::Say("SHA-256 copied.".into()));
            }
            (Ok(sha), HashFor::VirusTotal) => self.look_up_on_virustotal(sha),
        }
    }

    /// Opens the VirusTotal page of this SHA-256 in the browser: nothing
    /// when the option is off, and the first time only once the user agreed
    /// to send the hash.
    pub fn look_up_on_virustotal(&mut self, sha: String) {
        if !self.virustotal {
            return;
        }
        if !self.virustotal_consent {
            self.drv_ui.confirm_virustotal = Some(sha);
            return;
        }
        let Some(url) = virustotal_url(&sha) else { return };
        if let Err(e) = self.context.open(&url) {
            self.say(e);
        }
    }

    /// Draws the page below the app's header (title, Refresh).
    pub fn ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let th = theme::of_ui(ui);
        self.opened();
        self.settle_hash(actions);
        let st = self.drv.lock();
        let devices = st.devices.clone();
        let kernel = st.kernel.clone();
        let updates = st.updates.clone();
        let busy = st.busy.clone();
        let message = st.message.clone();
        let output = st.output.clone();
        let reboot = st.reboot_required;
        let listing = st.listing;
        let devices_error = st.devices_error.clone();
        let kernel_error = st.kernel_error.clone();
        let checked_at = st.checked_at.clone();
        let listed_at = st.listed_at.clone();
        let security = st.security.clone();
        let store = st.store.clone();
        let store_error = st.store_error.clone();
        let store_reading = st.store_reading;
        let filter_read = st.filters.clone();
        let signatures = st.signatures.clone();
        drop(st);
        let admin = self.context.is_admin();
        let idle = busy.is_none();
        let view = self.derived(&devices, &kernel, &updates, &signatures);

        // ------------------------------------------------ actions (the app's header has the title and Refresh)
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let more = ui.add(egui::Button::new("More actions      ").min_size(vec2(0.0, 32.0)));
                let c = Pos2::new(more.rect.right() - 16.0, more.rect.center().y);
                parts::chevron_down(ui.painter(), c);
                egui::Popup::menu(&more).show(|ui| {
                    ui.set_min_width(260.0);
                    if parts::admin_button(ui, "Install from an INF file\u{2026}", false, idle).clicked() {
                        if let Some(p) = self.context.pick_open_file(&[drivers::INF_FILE, crate::platform::ANY_FILE], None) {
                            self.drv_ui.confirm_job = Some(drivers::Redo::Inf(p));
                        }
                        ui.close();
                    }
                    if parts::admin_button(ui, "Scan for hardware changes", false, idle).clicked() {
                        self.drv.scan();
                        ui.close();
                    }
                    if ui.add(egui::Button::new("      Export the list as CSV\u{2026}").min_size(vec2(0.0, 32.0))).clicked() {
                        self.export_drivers_csv(&devices, &kernel, updates.as_deref(), &signatures);
                        ui.close();
                    }
                });
                ui.add_space(8.0);
                if parts::admin_button(ui, BACKUP_ALL_LABEL, false, idle).clicked() {
                    self.drv_ui.confirm_backup = Some(BackupAsk { names: None, folder: self.drv_ui.backup_folder.clone() });
                }
            });
        });
        ui.add_space(8.0);

        self.drivers_band(ui, &devices, updates.as_deref(), busy.as_deref(), message.as_ref(), reboot, &checked_at, idle, view.problem_count);
        if !output.is_empty() {
            egui::CollapsingHeader::new(RichText::new("Output of the last tool").color(th.label)).id_salt("drv-output").show(ui, |ui| {
                ui.label(RichText::new(&output).monospace().size(11.0));
            });
        }
        ui.add_space(8.0);
        self.security_strip(ui, security.as_ref());
        ui.add_space(12.0);

        // ------------------------------------------------ scope row
        ui.horizontal(|ui| {
            let mut scopes = vec![(Scope::Hardware, "Hardware drivers", view.hardware_count), (Scope::Builtin, "Windows built-in", view.builtin_count)];
            // The scope shows only while there is a problem to list (or it is the chosen one).
            if view.problem_count > 0 || self.drv_ui.scope == Scope::Problems {
                scopes.push((Scope::Problems, "Problems", view.problem_count));
            }
            scopes.extend([
                (Scope::Kernel, "Kernel modules", kernel.len()),
                (Scope::Store, "Driver store", store.len()),
                (Scope::Filters, "Filter drivers", filter_read.as_deref().map_or(0, filters_count)),
            ]);
            if let Some(scope) = parts::ScopeGroup::new(self.drv_ui.scope, &scopes).alert(Scope::Problems).show(ui).clicked {
                self.drv_ui.scope = scope;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if matches!(self.drv_ui.scope, Scope::Hardware | Scope::Builtin) {
                    ui.checkbox(&mut self.drv_ui.show_disconnected, "Show disconnected");
                    ui.add_space(8.0);
                }
                let hint = match self.drv_ui.scope {
                    Scope::Kernel => KERNEL_FILTER_HINT,
                    Scope::Store => STORE_FILTER_HINT,
                    Scope::Filters => FILTER_NAME_HINT,
                    _ => DEVICE_FILTER_HINT,
                };
                crate::widgets::search_field(ui, &mut self.drv_ui.search, hint, 240.0);
            });
        });
        if self.drv_ui.scope == Scope::Kernel {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (s, n) in &view.kernel_counts {
                    ui.selectable_value(&mut self.drv_ui.kernel_filter, *s, format!("{}  {}", s.label(), n));
                }
                ui.add_space(12.0);
                let shown = view.kernel_rows.len();
                ui.label(RichText::new(format!("{shown} shown")).color(th.label).size(12.0));
                if !listed_at.is_empty() {
                    ui.label(RichText::new(format!("Updated {listed_at}")).color(th.faint).size(12.0));
                }
            });
        }
        if self.drv_ui.scope == Scope::Filters {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Filter drivers").strong().color(th.soft));
                ui.label(RichText::new(filters::FILTERS_INTRO).size(12.0).color(th.label));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(filters::FILTERS_READ_ONLY).size(12.0).color(th.faint));
                });
            });
        }
        ui.add_space(10.0);

        // ------------------------------------------------ list + side panel
        // A click or a keystroke above may have changed the filter.
        let view = self.derived(&devices, &kernel, &updates, &signatures);
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, Sense::hover());
        let side_w = 350.0f32.min(area.width() * 0.36);
        let list_rect = Rect::from_min_max(area.min, Pos2::new(area.right() - side_w - 16.0, area.bottom()));
        let side_rect = Rect::from_min_max(Pos2::new(area.right() - side_w, area.top()), area.max);
        let mut list_ui = ui.new_child(egui::UiBuilder::new().max_rect(list_rect));
        let mut side_ui = ui.new_child(egui::UiBuilder::new().max_rect(side_rect));
        match self.drv_ui.scope {
            Scope::Kernel => {
                self.kernel_list(&mut list_ui, actions, &view, admin, listing, kernel_error.as_deref());
                self.kernel_side(&mut side_ui, actions, &kernel, &signatures, admin, idle);
            }
            Scope::Store => {
                let list = self.store_view(&store);
                self.drv_ui.ticked.keep_only(&store);
                self.store_list(&mut list_ui, actions, &list, store_reading, store_error.as_deref(), idle);
                self.store_side(&mut side_ui, actions, &list, idle);
            }
            Scope::Filters => {
                self.filter_list(&mut list_ui, filter_read.as_deref());
                empty_panel(&mut side_ui, filters::FILTERS_SIDE);
            }
            scope => {
                let searching = !self.drv_ui.search.trim().is_empty();
                let note = list_note(listing, devices_error.as_deref(), devices.len(), view.matching, searching);
                if let Some(note) = note {
                    show_note(&mut list_ui, &note, scope);
                } else {
                    self.device_list(&mut list_ui, actions, &view, idle);
                }
                self.device_side(&mut side_ui, actions, &devices, updates.as_deref(), &signatures, idle);
            }
        }

        self.drivers_dialogs(ui);
    }

    /// The read-only "Security" strip under the status band: Memory
    /// integrity and the vulnerable-driver blocklist, and a link to Windows
    /// Security, where they are changed. The page never writes them.
    fn security_strip(&mut self, ui: &mut Ui, reading: Option<&drivers::SecurityReading>) {
        let th = theme::of_ui(ui);
        let view = drivers::security_view(reading);
        let mut open = false;
        crate::widgets::strip(ui, |ui| {
            ui.label(RichText::new("Security").size(12.0).strong().color(th.soft));
            for (name, state) in view.items {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let color = match state {
                        Some(drivers::Setting::On) => th.green,
                        Some(drivers::Setting::Off) => th.amber,
                        _ => th.faint,
                    };
                    crate::widgets::dot(ui, color);
                    ui.label(RichText::new(format!("{name}:")).size(12.0).color(th.soft));
                    if let Some(state) = state {
                        ui.label(RichText::new(state.text()).size(12.0).strong().color(th.soft));
                    }
                });
            }
            if !view.note.is_empty() {
                ui.label(RichText::new(&view.note).size(12.0).color(th.label));
            }
            if ui.link(RichText::new("Open Windows Security").size(12.0)).clicked() {
                open = true;
            }
            ui.label(RichText::new(drivers::SECURITY_READ_ONLY).size(12.0).color(th.faint));
        });
        if open {
            self.open_windows_security();
        }
    }

    /// Opens Windows Security's core isolation page, where Memory integrity
    /// is changed; a failure is told in the status bar.
    pub fn open_windows_security(&mut self) {
        if let Err(e) = self.context.open(drivers::CORE_ISOLATION_LINK) {
            self.say(e);
        }
    }

    /// The status band: the page's answer and its main action.
    #[allow(clippy::too_many_arguments)]
    fn drivers_band(
        &mut self,
        ui: &mut Ui,
        devices: &[DeviceDriver],
        updates: Option<&[DriverUpdate]>,
        busy: Option<&str>,
        message: Option<&(String, bool)>,
        reboot: bool,
        checked_at: &str,
        idle: bool,
        problem_count: usize,
    ) {
        let th = theme::of_ui(ui);
        let newer: Vec<&DriverUpdate> = updates.map(|u| u.iter().filter(|x| x.worth_installing()).collect()).unwrap_or_default();
        let older: Vec<&DriverUpdate> = updates.map(|u| u.iter().filter(|x| !x.worth_installing()).collect()).unwrap_or_default();
        let device = |u: &DriverUpdate| u.device.as_ref().and_then(|id| devices.iter().find(|d| &d.instance_id == id));
        let mut select: Option<String> = None;
        let mut show_problems = false;

        let n = newer.len();
        let (mark, headline) = if let Some(b) = busy {
            (Mark::Busy, b.to_string())
        } else if updates.is_none() {
            (Mark::None, "Drivers have not been checked for updates yet".to_string())
        } else if newer.is_empty() {
            (Mark::Tick, "All drivers are up to date on Windows Update".to_string())
        } else {
            (Mark::Dot(th.amber), format!("{} {} ready for this PC", plural(n, "newer driver"), if n == 1 { "is" } else { "are" }))
        };
        let mut band = StatusBand::new(mark, &headline).actions_width(270.0);
        if busy.is_some() {
            band = band.sub("You can keep using the app while this runs.");
        } else if updates.is_none() {
            band = band.sub("Checking compares every driver with Windows Update. It takes up to a minute and needs no administrator rights.");
        } else if newer.is_empty() {
            band = band.sub("Graphics cards and some other devices can have newer drivers on their maker's site.");
        }
        band.show(ui, |ui, area| match area {
            BandArea::Body => {
                if problem_count > 0 && busy.is_none() {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(problems::band_text(problem_count)).strong().color(th.red));
                        if ui.link("Show them").clicked() {
                            show_problems = true;
                        }
                    });
                }
                if busy.is_none() && !newer.is_empty() {
                    for u in newer.iter().take(3) {
                        let dev = device(u);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 14.0;
                            let name = dev.map(|d| d.name.clone()).unwrap_or_else(|| u.model.clone());
                            let r = ui.add(egui::Label::new(RichText::new(name).size(14.0).color(th.soft)).sense(Sense::click()));
                            if let Some(d) = dev {
                                if r.on_hover_text("Show this driver").clicked() {
                                    select = Some(d.instance_id.clone());
                                }
                            }
                            if let Some(d) = dev {
                                ui.label(RichText::new(&d.version).font(display(n_font(n))).color(th.label));
                                let (ar, _) = ui.allocate_exact_size(vec2(18.0, 18.0), egui::Sense::hover());
                                crate::widgets::icons::draw_glyph(ui.painter(), ar.center(), 15.0, "arrow-right", crate::widgets::icons::Ink::Theme);
                            }
                            ui.label(RichText::new(if u.version.is_empty() { &u.date } else { &u.version }).font(display(n_font(n))).color(th.amber));
                        });
                    }
                    if n > 3 {
                        ui.label(RichText::new(format!("and {} more", n - 3)).color(th.label));
                    }
                    if n == 1 {
                        let u = newer[0];
                        ui.label(
                            RichText::new(format!("From {}, released {}. {} download from Windows Update.", u.provider, long_date(&u.date), fmt_size(u.size as u64)))
                                .size(12.0)
                                .color(th.label),
                        );
                    }
                }
                if let Some((m, err)) = message.filter(|_| busy.is_none()) {
                    ui.label(RichText::new(m).size(12.5).color(if *err { th.red } else { th.green }));
                }
                if reboot {
                    ui.label(RichText::new("Restart Windows to finish installing drivers.").size(12.5).strong().color(th.amber));
                }
                if !older.is_empty() && busy.is_none() {
                    ui.add_space(4.0);
                    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                    ui.painter().rect_filled(r, CornerRadius::ZERO, th.line);
                    for u in &older {
                        let dev = device(u);
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            let name = dev.map(|d| d.name.clone()).unwrap_or_else(|| u.model.clone());
                            let have = dev
                                .map(|d| format!("which is older than the {} you have, so it is left out.", d.version))
                                .unwrap_or_else(|| "which is not newer than the installed driver, so it is left out.".into());
                            ui.label(RichText::new(format!("Windows Update also offers {name} {}, {have}", u.version)).size(12.5).color(th.soft));
                            if ui.link(RichText::new("Install it anyway").size(12.5)).clicked() {
                                self.drv_ui.confirm_older = Some((*u).clone());
                            }
                        });
                    }
                }
            }
            BandArea::Actions => {
                if updates.is_none() || busy.is_some() {
                    if parts::primary_button(ui, "Check for updates", idle).clicked() {
                        self.drv.check_updates();
                    }
                    ui.label(RichText::new("Searches Windows Update").size(11.5).color(th.label));
                } else {
                    if !newer.is_empty() {
                        let label = if newer.len() == 1 { "Install update".to_string() } else { format!("Install {} updates", newer.len()) };
                        if parts::admin_button(ui, &label, true, idle).clicked() {
                            self.drv_ui.confirm_job = Some(drivers::Redo::Updates(newer.iter().map(|u| u.id.clone()).collect()));
                        }
                        ui.label(RichText::new("Windows asks for administrator permission").size(11.5).color(th.label));
                        ui.add_space(6.0);
                    }
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.add_enabled(idle, egui::Button::new(RichText::new("Check again").color(th.link)).min_size(vec2(0.0, 28.0))).clicked() {
                                self.drv.check_updates();
                            }
                            ui.label(RichText::new(format!("Checked today at {}", short_time(checked_at))).size(12.0).color(th.label));
                        });
                    });
                }
            }
            BandArea::Below => {}
        });

        if show_problems {
            self.drv_ui.scope = Scope::Problems;
            self.drv_ui.search.clear();
        }
        if let Some(id) = select {
            if let Some(d) = devices.iter().find(|d| d.instance_id == id) {
                self.drv_ui.scope = if d.third_party() { Scope::Hardware } else { Scope::Builtin };
                if !d.present {
                    self.drv_ui.show_disconnected = true;
                }
                self.drv_ui.search.clear();
            }
            self.drv_ui.selected_device = Some(id);
            self.drv_ui.scroll_to_selected = true;
        }
    }

    /// The device list: only the rows in view are built, the rest of the
    /// scroll area is reserved from the heights worked out with the rows.
    fn device_list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, view: &Derived, idle: bool) {
        let th = theme::of_ui(ui);
        let updates = view.updates.as_deref();

        let w = ui.available_width();
        let fixed = [140.0, 110.0, 130.0];
        let gap = 12.0;
        let flex = (w - fixed.iter().sum::<f32>() - gap * 4.0 - 20.0).max(200.0);
        let cols = [flex * 0.66, flex * 0.34, fixed[0], fixed[1], fixed[2]];
        let xs: Vec<f32> = cols.iter().scan(10.0, |x, c| {
            let start = *x;
            *x += c + gap;
            Some(start)
        }).collect();

        // Header
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        for (i, h) in ["Driver", "Maker", "Version", "Date", "Windows Update"].iter().enumerate() {
            ui.painter().text(Pos2::new(hr.left() + xs[i], hr.center().y), Align2::LEFT_CENTER, *h, text(12.0), th.label);
        }
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);

        let mut clicked: Option<String> = None;
        let mut device_pick: Option<DeviceConfirm> = None;
        let gap_y = ui.spacing().item_spacing.y;
        let items = &view.items;
        let top = |i: usize| view.tops[i] + i as f32 * gap_y;
        let total = items.last().map_or(0.0, |last| top(items.len() - 1) + last.height());
        egui::ScrollArea::vertical().id_salt(("drv-device-list", self.drv_ui.scope == Scope::Hardware)).auto_shrink([false, false]).show_viewport(ui, |ui, viewport| {
            let origin = ui.cursor().min;
            ui.allocate_rect(Rect::from_min_size(origin, vec2(w, total)), Sense::hover());
            if self.drv_ui.scroll_to_selected {
                let at = items.iter().position(|it| matches!(it, Item::Device(r) if self.drv_ui.selected_device.as_deref() == Some(view.devices[r.dev].instance_id.as_str())));
                if let Some(i) = at {
                    ui.scroll_to_rect(Rect::from_min_size(origin + vec2(0.0, top(i)), vec2(w, items[i].height())), Some(Align::Center));
                    self.drv_ui.scroll_to_selected = false;
                }
            }
            // The first index where `keeps_going` stops holding (items are in order).
            let boundary = |keeps_going: &dyn Fn(usize) -> bool| {
                let (mut lo, mut hi) = (0, items.len());
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    if keeps_going(mid) {
                        lo = mid + 1;
                    } else {
                        hi = mid;
                    }
                }
                lo
            };
            let first = boundary(&|i| top(i) + items[i].height() < viewport.min.y);
            let last = boundary(&|i| top(i) <= viewport.max.y);
            for (i, item) in items.iter().enumerate().take(last).skip(first) {
                let r = Rect::from_min_size(origin + vec2(0.0, top(i)), vec2(w, item.height()));
                let row = match item {
                    Item::Kind { name, count } => {
                        let t = ui.painter().text(Pos2::new(r.left() + 10.0, r.bottom() - 6.0), Align2::LEFT_BOTTOM, *name, FontId::proportional(13.5), th.text);
                        ui.painter().text(Pos2::new(t.right() + 8.0, r.bottom() - 6.0), Align2::LEFT_BOTTOM, count.to_string(), text(12.0), th.faint);
                        continue;
                    }
                    Item::Device(row) => row,
                };
                let d = &view.devices[row.dev];
                let upd = view.update_of(row);
                let resp = ui.interact(r, ui.id().with(("drv-device", d.instance_id.as_str())), Sense::click());
                let selected = self.drv_ui.selected_device.as_deref() == Some(d.instance_id.as_str());
                let p = ui.painter();
                parts::row_fill(p, r, selected, resp.hovered());
                let sub = row.sub.map(|s| match s {
                    Sub::OlderOffer => (format!("Windows Update offers the older {}", upd.map(|u| u.version.as_str()).unwrap_or_default()), th.faint),
                    Sub::CheckMaker => (format!("{} years old: check the maker", row.check.unwrap_or_default()), th.amber),
                    Sub::Folded => (format!("{} entries folded into one", row.count), th.faint),
                    Sub::NotConnected => ("Not connected".to_string(), th.faint),
                    Sub::Problem => (problems::row_text(d), th.red),
                });
                let y1 = if sub.is_some() { r.top() + 12.0 } else { r.center().y };
                let cell = |i: usize, s: &str, col: Color32| {
                    let s = fit(p, s, &text(13.0), cols[i]);
                    p.text(Pos2::new(r.left() + xs[i], y1), Align2::LEFT_CENTER, s, text(13.0), col);
                };
                cell(0, &d.name, if selected { th.on_selected } else { th.text });
                cell(1, &d.provider, th.soft);
                cell(2, &d.version, th.soft);
                let (date, date_col) = if with_windows(d) {
                    ("With Windows".to_string(), th.faint)
                } else if row.check.is_some() {
                    (d.date.clone(), th.amber)
                } else {
                    (d.date.clone(), th.soft)
                };
                cell(3, &date, date_col);
                match (updates, upd) {
                    (None, _) => {}
                    (Some(_), Some(u)) if u.worth_installing() => cell(4, &format!("New: {}", u.version), th.amber),
                    (Some(_), _) => cell(4, "Up to date", th.green),
                }
                if let Some((s, col)) = &sub {
                    let s = fit(p, s, &text(11.5), cols[0] + cols[1]);
                    // A faint explaining line turns light blue on the selection.
                    let col = if *col == th.faint { parts::sub_colour(&th, selected) } else { *col };
                    p.text(Pos2::new(r.left() + xs[0], r.top() + 29.0), Align2::LEFT_CENTER, s, text(11.5), col);
                }
                if resp.clicked() {
                    clicked = Some(d.instance_id.clone());
                }
                let details = device_details(d, upd, device_signature(d, &view.signatures));
                let inf = d.inf_path();
                let name = d.name.clone();
                let offered = [drivers::DeviceAction::toggle_for(d), drivers::DeviceAction::Restart];
                resp.context_menu(|ui| {
                    if ui.button("Open the driver's folder").clicked() {
                        actions.push(Action::OpenLocation(inf.clone()));
                        ui.close();
                    }
                    if ui.button("Copy details").clicked() {
                        actions.push(Action::Copy(details.clone()));
                        ui.close();
                    }
                    if ui.button("Search online").clicked() {
                        actions.push(Action::SearchOnline(format!("{name} driver")));
                        ui.close();
                    }
                    ui.separator();
                    ui.label(RichText::new("Device").size(11.5).color(th.faint));
                    // Roll back driver (#31) goes first in this group.
                    for action in offered {
                        if crate::widgets::admin_menu_item(ui, action.label(), idle).clicked() {
                            device_pick = Some(DeviceConfirm::new(action, d));
                            ui.close();
                        }
                    }
                });
            }
        });
        if let Some(id) = clicked {
            self.drv_ui.selected_device = Some(id);
        }
        if device_pick.is_some() {
            self.drv_ui.confirm_device = device_pick;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn device_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, devices: &[DeviceDriver], updates: Option<&[DriverUpdate]>, signatures: &SignatureBook, idle: bool) {
        let chosen = self.drv_ui.selected_device.clone().and_then(|id| devices.iter().find(|d| d.instance_id == id));
        if chosen.is_none() {
            empty_panel(ui, DEVICE_EMPTY);
            return;
        }
        parts::SidePanel::new("drv-side").show(ui, chosen, |ui, d| {
            let th = theme::of_ui(ui);
            let upd = updates.and_then(|u| u.iter().find(|x| x.device.as_deref() == Some(d.instance_id.as_str())));
            let copies = devices.iter().filter(|o| o.name == d.name && o.version == d.version && o.provider == d.provider && (o.present || !d.present)).count();
            ui.label(RichText::new(&d.name).font(display(15.0)).color(th.text));
            let signature = device_signature(d, signatures);
            let state = if d.present { format!("connected, {}", d.status().to_lowercase()) } else { "not connected".into() };
            ui.label(RichText::new(format!("{}, {state}", kind_of(&d.class))).size(12.0).color(th.label));
            ui.add_space(6.0);
            egui::Grid::new("drv-side-grid").striped(false).num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
                let mut row = |k: &str, v: String| {
                    ui.label(RichText::new(k).color(th.label));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                };
                row("Maker", or_dash(&d.provider));
                row("Version", or_dash(&d.version));
                for (k, v) in signature_rows(signature) {
                    row(k, v);
                }
                row("Released", if with_windows(d) { "With Windows".into() } else { long_date(&d.date) });
                row("Package", or_dash(&d.inf));
                if !d.service.is_empty() {
                    row("Service", d.service.clone());
                }
                if copies > 1 {
                    row("Entries", format!("{copies} identical entries"));
                }
            });
            if problems::is_problem(d) {
                ui.add_space(8.0);
                self.problem_section(ui, d, upd, idle);
            }
            ui.add_space(8.0);
            separator(ui);
            ui.add_space(8.0);
            match (updates, upd) {
                (None, _) => {
                    ui.label(RichText::new("Not compared with Windows Update yet.").color(th.label));
                }
                (Some(_), Some(u)) if u.worth_installing() => {
                    ui.label(RichText::new("A newer driver is on Windows Update").strong().color(th.amber));
                    ui.add_space(4.0);
                    compare(ui, d, u);
                    ui.add_space(6.0);
                    if parts::admin_button(ui, "Install update", true, idle).clicked() {
                        self.drv_ui.confirm_job = Some(drivers::Redo::Updates(vec![u.id.clone()]));
                    }
                    ui.label(RichText::new("Windows asks for administrator permission. Some drivers need a restart.").size(11.5).color(th.label));
                }
                (Some(_), Some(u)) => {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        tick(ui.painter(), r.center());
                        ui.label(RichText::new("Up to date").strong().color(th.green));
                    });
                    ui.label(RichText::new(format!("Windows Update offers {}, which is older than this one.", u.version)).size(12.0).color(th.label));
                    if parts::admin_button(ui, &format!("Install {} anyway\u{2026}", u.version), false, idle).clicked() {
                        self.drv_ui.confirm_older = Some(u.clone());
                    }
                }
                (Some(_), None) => {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                        tick(ui.painter(), r.center());
                        ui.label(RichText::new("Nothing newer on Windows Update").strong().color(th.green));
                    });
                }
            }
            if let Some((maker, url)) = maker_downloads(d) {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("Graphics drivers usually reach {maker}'s site before Windows Update.")).size(12.0).color(th.label));
                if ui.link(format!("Open {maker} driver downloads")).clicked() {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            } else if check_the_maker(d, today(), self.old_years).is_some() {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("This driver is from {}. Windows Update has nothing newer; the maker's site may.", long_date(&d.date))).size(12.0).color(th.label));
                if ui.link("Search for a newer driver").clicked() {
                    actions.push(Action::SearchOnline(format!("{} {} driver", d.provider, d.name)));
                }
            }
            ui.add_space(8.0);
            separator(ui);
            ui.add_space(8.0);
            ui.label(RichText::new("Device").strong().color(th.soft));
            ui.add_space(4.0);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                // Roll back driver (#31) goes first in this group.
                for action in [drivers::DeviceAction::toggle_for(d), drivers::DeviceAction::Restart] {
                    if parts::admin_button(ui, action.label(), false, idle).clicked() {
                        self.drv_ui.confirm_device = Some(DeviceConfirm::new(action, d));
                    }
                }
            });
            if let Some(kind) = drivers::protected_kind(&d.class) {
                ui.label(RichText::new(format!("This is a {kind}: disabling or restarting it asks for a stronger confirmation.")).size(11.5).color(th.amber));
            }
            ui.add_space(10.0);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                if parts::outline_button(ui, "Open the driver's folder", true).clicked() {
                    actions.push(Action::OpenLocation(d.inf_path()));
                }
                if parts::outline_button(ui, "Open in Device Manager", true).clicked() {
                    self.open_device_manager();
                }
                if parts::outline_button(ui, "Copy details", true).clicked() {
                    actions.push(Action::Copy(device_details(d, upd, signature)));
                }
            });
        });
    }

    /// The side panel's problem section: the code in plain words and what to
    /// try, linked to the page's own action when it has one.
    fn problem_section(&mut self, ui: &mut Ui, d: &DeviceDriver, upd: Option<&DriverUpdate>, idle: bool) {
        let th = theme::of_ui(ui);
        let p = problems::explain(d.problem);
        ui.label(RichText::new(format!("Code {}: {}", p.code, p.explanation)).strong().color(th.red));
        let Some(fix) = p.fix else { return };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new("Try:").size(12.0).color(th.label));
            // The action the suggestion links to, when this page has it for this device.
            let device_action = match fix {
                problems::Fix::Restart => Some(drivers::DeviceAction::Restart),
                problems::Fix::Enable => Some(drivers::DeviceAction::toggle_for(d)).filter(|a| *a == drivers::DeviceAction::Enable),
                problems::Fix::Driver => None,
            };
            let update = if fix == problems::Fix::Driver { upd.filter(|u| u.worth_installing()) } else { None };
            if device_action.is_some() || update.is_some() {
                if ui.add_enabled(idle, egui::Link::new(RichText::new(fix.text()).size(12.0))).clicked() {
                    if let Some(action) = device_action {
                        self.drv_ui.confirm_device = Some(DeviceConfirm::new(action, d));
                    } else if let Some(u) = update {
                        self.drv_ui.confirm_job = Some(drivers::Redo::Updates(vec![u.id.clone()]));
                    }
                }
            } else {
                ui.label(RichText::new(fix.text()).size(12.0).color(th.soft));
            }
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn kernel_list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, view: &Derived, admin: bool, listing: bool, error: Option<&str>) {
        let th = theme::of_ui(ui);
        let kernel = &view.kernel;
        // Reading, failed or nothing reported: no figures to give.
        if let Some(note) = list_note(listing, error, kernel.len(), kernel.len(), false) {
            show_note(ui, &note, Scope::Kernel);
            return;
        }
        kernel_tiles(ui, view.figures);
        if view.figures.memory.is_none() {
            ui.add_space(4.0);
            ui.label(RichText::new(KERNEL_MEMORY_NOTE).size(12.0).color(th.faint));
        }
        ui.add_space(10.0);
        let rows = &view.kernel_rows;
        if let Some(note) = list_note(listing, error, kernel.len(), rows.len(), !self.drv_ui.search.trim().is_empty()) {
            show_note(ui, &note, Scope::Kernel);
            return;
        }
        let w = ui.available_width();
        let fixed = [80.0, 85.0, 85.0, 130.0, 130.0];
        let gap = 12.0;
        let flex = (w - fixed.iter().sum::<f32>() - gap * 7.0 - 20.0).max(240.0);
        let cols = [flex * 0.45, flex * 0.2, fixed[0], fixed[1], fixed[2], fixed[3], flex * 0.35, fixed[4]];
        let xs: Vec<f32> = cols.iter().scan(10.0, |x, c| {
            let start = *x;
            *x += c + gap;
            Some(start)
        }).collect();
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        for (i, h) in ["Driver", "Name", "State", "Start type", "Type", "Version", "Vendor", "Signature"].iter().enumerate() {
            ui.painter().text(Pos2::new(hr.left() + xs[i], hr.center().y), Align2::LEFT_CENTER, *h, text(12.0), th.label);
        }
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);
        let mut clicked = None;
        let (mut open_manager, mut unload) = (false, None);
        let (virustotal, mut hash) = (self.virustotal, None);
        egui::ScrollArea::vertical().id_salt("drv-kernel-list").auto_shrink([false, false]).show_rows(ui, 28.0, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let d = &kernel[i];
                let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
                let p = ui.painter();
                let selected = self.drv_ui.selected_kernel.as_deref() == Some(d.service.as_str());
                parts::row_fill(p, r, selected, resp.hovered());
                let signature = view.signatures.of(&d.path);
                let signature_label = signature_text(signature);
                let vals = [
                    d.display.as_str(),
                    d.service.as_str(),
                    d.state_label(),
                    d.start_type,
                    d.kind.label(),
                    if d.version.is_empty() { "\u{2014}" } else { d.version.as_str() },
                    if d.vendor.is_empty() { "\u{2014}" } else { d.vendor.as_str() },
                    signature_label.as_str(),
                ];
                for (i, v) in vals.iter().enumerate() {
                    let col = match (i, selected) {
                        (0, true) => th.on_selected,
                        (0, false) => th.text,
                        (7, _) => match signature {
                            Some(Signature::Whql { .. }) => th.green,
                            Some(Signature::Unsigned) => th.red,
                            Some(Signature::Signed { .. }) => th.soft,
                            _ => th.faint,
                        },
                        _ => th.soft,
                    };
                    p.text(Pos2::new(r.left() + xs[i], r.center().y), Align2::LEFT_CENTER, fit(p, v, &text(13.0), cols[i]), text(13.0), col);
                }
                if resp.clicked() {
                    clicked = Some(d.service.clone());
                }
                let (path, service, details) = (d.path.clone(), d.service.clone(), kernel_details(d, admin, signature));
                let unloadable = d.unload_note(admin).0;
                resp.context_menu(|ui| {
                    if ui.button("Open file location").clicked() {
                        actions.push(reveal(&path, |p| p.exists()));
                        ui.close();
                    }
                    if ui.button("Open Device Manager").clicked() {
                        open_manager = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("Copy details").clicked() {
                        actions.push(Action::Copy(details.clone()));
                        ui.close();
                    }
                    ui.separator();
                    ui.label(RichText::new("Advanced").size(11.5).color(th.faint));
                    if ui.button("Copy service name").clicked() {
                        actions.push(Action::Copy(service.clone()));
                        ui.close();
                    }
                    if ui.button("Copy SHA-256").clicked() {
                        hash = Some((path.clone(), HashFor::Copy));
                        ui.close();
                    }
                    if virustotal && ui.button("Look up on VirusTotal").clicked() {
                        hash = Some((path.clone(), HashFor::VirusTotal));
                        ui.close();
                    }
                    if unloadable && ui.button("Unload\u{2026}").clicked() {
                        unload = Some(service.clone());
                        ui.close();
                    }
                });
            }
        });
        if let Some(s) = clicked {
            self.drv_ui.selected_kernel = Some(s);
        }
        if open_manager {
            self.open_device_manager();
        }
        if unload.is_some() {
            self.drv_ui.confirm_unload = unload;
        }
        if let Some((path, purpose)) = hash {
            self.hash_module(&path, purpose, actions);
        }
    }

    /// The Filter drivers tab: name, altitude, instances and frame, or why
    /// there is no list. Read-only: rows have no actions.
    fn filter_list(&mut self, ui: &mut Ui, read: Option<&FilterRead>) {
        let th = theme::of_ui(ui);
        let q = self.drv_ui.search.trim().to_lowercase();
        let rows = match read {
            Some(FilterRead::Filters(list)) => filter_rows(list, &q),
            _ => Vec::new(),
        };
        if let Some((title, detail)) = filter_note(read, rows.len(), !q.is_empty()) {
            ui.label(RichText::new(title).color(if matches!(read, Some(FilterRead::Failed(_))) { th.red } else { th.label }));
            if let Some(detail) = detail {
                ui.label(RichText::new(detail).size(12.0).color(th.faint));
            }
            return;
        }
        let w = ui.available_width();
        let fixed = [110.0, 90.0, 70.0];
        let gap = 12.0;
        let name_w = (w - fixed.iter().sum::<f32>() - gap * 3.0 - 20.0).max(160.0);
        let right = [10.0 + name_w + gap + fixed[0], 10.0 + name_w + gap * 2.0 + fixed[0] + fixed[1], 10.0 + name_w + gap * 3.0 + fixed[0] + fixed[1] + fixed[2]];
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        let p = ui.painter();
        p.text(Pos2::new(hr.left() + 10.0, hr.center().y), Align2::LEFT_CENTER, "Filter", text(12.0), th.label);
        for (x, h) in right.iter().zip(["Altitude", "Instances", "Frame"]) {
            p.text(Pos2::new(hr.left() + x, hr.center().y), Align2::RIGHT_CENTER, h, text(12.0), th.label);
        }
        p.rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);
        egui::ScrollArea::vertical().id_salt("drv-filter-list").auto_shrink([false, false]).show_rows(ui, 28.0, rows.len(), |ui, range| {
            for f in &rows[range] {
                let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
                let p = ui.painter();
                parts::row_fill(p, r, false, resp.hovered());
                p.text(Pos2::new(r.left() + 10.0, r.center().y), Align2::LEFT_CENTER, fit(p, &f.name, &text(13.0), name_w), text(13.0), th.text);
                let nums = [f.altitude.clone(), f.instances.to_string(), f.frame.to_string()];
                for (x, v) in right.iter().zip(nums) {
                    p.text(Pos2::new(r.left() + x, r.center().y), Align2::RIGHT_CENTER, v, text(13.0), th.soft);
                }
            }
        });
    }

    fn kernel_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, kernel: &[KernelDriver], signatures: &SignatureBook, admin: bool, idle: bool) {
        let chosen = self.drv_ui.selected_kernel.clone().and_then(|s| kernel.iter().find(|d| d.service == s));
        if chosen.is_none() {
            empty_panel(ui, KERNEL_EMPTY);
            return;
        }
        let (virustotal, mut hash) = (self.virustotal, None);
        let path = chosen.map(|d| d.path.clone());
        parts::SidePanel::new("drv-side").show(ui, chosen, |ui, d| {
            let th = theme::of_ui(ui);
            let (offered, _) = d.unload_note(admin);
            let signature = signatures.of(&d.path);
            ui.label(RichText::new(&d.display).font(display(15.0)).color(th.text));
            ui.add_space(6.0);
            egui::Grid::new("drv-kernel-side").striped(false).num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
                for (k, v) in kernel_detail_rows(d, admin, signature) {
                    ui.label(RichText::new(k).color(th.label));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                }
            });
            ui.add_space(8.0);
            separator(ui);
            ui.add_space(10.0);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                if parts::outline_button(ui, "Open file location", true).clicked() {
                    actions.push(reveal(&d.path, |p| p.exists()));
                }
                if parts::outline_button(ui, "Open in Device Manager", true).clicked() {
                    self.open_device_manager();
                }
                if parts::outline_button(ui, "Copy service name", true).clicked() {
                    actions.push(Action::Copy(d.service.clone()));
                }
                if parts::outline_button(ui, "Copy details", true).clicked() {
                    actions.push(Action::Copy(kernel_details(d, admin, signature)));
                }
                ui.label(RichText::new("Advanced").size(11.5).color(th.faint));
                if parts::outline_button(ui, "Copy SHA-256", true).clicked() {
                    hash = Some(HashFor::Copy);
                }
                if virustotal && parts::outline_button(ui, "Look up on VirusTotal", true).clicked() {
                    hash = Some(HashFor::VirusTotal);
                }
                if offered {
                    if parts::admin_button(ui, "Unload\u{2026}", false, idle).clicked() {
                        self.drv_ui.confirm_unload = Some(d.service.clone());
                    }
                }
            });
        });
        if let (Some(purpose), Some(path)) = (hash, path) {
            self.hash_module(&path, purpose, actions);
        }
    }

    /// The Driver store tab: the figures, then one row per package.
    fn store_list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, list: &StoreList, reading: bool, error: Option<&str>, idle: bool) {
        let th = theme::of_ui(ui);
        let packages = &list.packages;
        // Reading, failed or nothing reported: no figures to give.
        if let Some(note) = list_note(reading, error, packages.len(), packages.len(), false) {
            show_note(ui, &note, Scope::Store);
            return;
        }
        store_strip(ui, &list.figures);
        ui.add_space(8.0);
        self.store_actions(ui, list, idle);
        ui.add_space(10.0);
        let rows = &list.rows;
        if let Some(note) = list_note(reading, error, packages.len(), rows.len(), !self.drv_ui.search.trim().is_empty()) {
            show_note(ui, &note, Scope::Store);
            return;
        }
        let w = ui.available_width();
        let fixed = [110.0, 100.0, 90.0];
        let gap = 12.0;
        let flex = (w - fixed.iter().sum::<f32>() - gap * 6.0 - 20.0 - STORE_TICK_W).max(300.0);
        // Package, provider, class, version, date, signer, flags.
        let cols = [fixed[0] + 40.0, flex * 0.3, fixed[1], fixed[1] + 20.0, fixed[2], flex * 0.3, flex * 0.4];
        let xs: Vec<f32> = cols
            .iter()
            .scan(10.0 + STORE_TICK_W, |x, c| {
                let start = *x;
                *x += c + gap;
                Some(start)
            })
            .collect();
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        for (i, h) in ["Package", "Provider", "Class", "Version", "Date", "Signer", "Used by"].iter().enumerate() {
            ui.painter().text(Pos2::new(hr.left() + xs[i], hr.center().y), Align2::LEFT_CENTER, *h, text(12.0), th.label);
        }
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);
        let mut clicked = None;
        let mut toggled = None;
        egui::ScrollArea::vertical().id_salt("drv-store-list").auto_shrink([false, false]).show_rows(ui, STORE_ROW, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let p = &packages[i];
                let flags = list.flags[i];
                let (r, resp) = ui.allocate_exact_size(vec2(w, STORE_ROW), Sense::click());
                let selected = self.drv_ui.selected_store.as_deref() == Some(p.published.as_str());
                parts::row_fill(ui.painter(), r, selected, resp.hovered());
                // Inbox packages get no tick box.
                if !flags.inbox {
                    let box_rect = Rect::from_min_size(Pos2::new(r.left() + 4.0, r.top() + 8.0), vec2(24.0, 24.0));
                    let on = self.drv_ui.ticked.contains(&p.published);
                    if crate::widgets::tick_box(ui, box_rect, ui.id().with(("store-tick", &p.published)), on).clicked() {
                        toggled = Some(p.clone());
                    }
                }
                let painter = ui.painter();
                let cell = |i: usize, s: &str, col: Color32| {
                    let s = fit(painter, s, &text(13.0), cols[i]);
                    painter.text(Pos2::new(r.left() + xs[i], r.top() + 13.0), Align2::LEFT_CENTER, s, text(13.0), col);
                };
                cell(0, &p.published, if selected { th.on_selected } else { th.text });
                cell(1, &p.provider, th.soft);
                cell(2, &p.class, th.soft);
                cell(3, &p.version, th.soft);
                cell(4, &or_dash(&p.date), th.soft);
                cell(5, &or_dash(&p.signer), th.soft);
                // The INF under the name; the devices under the flags.
                let faint = parts::sub_colour(&th, selected);
                painter.text(Pos2::new(r.left() + xs[0], r.top() + 30.0), Align2::LEFT_CENTER, fit(painter, &p.original, &text(11.5), cols[0]), text(11.5), faint);
                let mut x = r.left() + xs[6];
                for word in flags.words() {
                    let colour = match word {
                        "Old" => th.amber,
                        "In use" => th.green,
                        _ => th.faint,
                    };
                    x = painter.text(Pos2::new(x, r.top() + 13.0), Align2::LEFT_CENTER, word, text(12.0), colour).right() + 8.0;
                }
                if !p.devices.is_empty() {
                    painter.text(Pos2::new(r.left() + xs[6], r.top() + 30.0), Align2::LEFT_CENTER, plural(p.devices.len(), "device"), text(11.5), faint);
                }
                if resp.clicked() {
                    clicked = Some(p.published.clone());
                }
                let details = store_details(p, flags);
                resp.context_menu(|ui| {
                    if ui.button("Copy details").clicked() {
                        actions.push(Action::Copy(details.clone()));
                        ui.close();
                    }
                });
            }
        });
        if let Some(name) = clicked {
            self.drv_ui.selected_store = Some(name);
        }
        if let Some(p) = toggled {
            self.drv_ui.ticked.toggle(&p);
        }
    }

    /// The buttons over the store's list: remove the old packages, remove
    /// the ticked ones, back the ticked ones up.
    fn store_actions(&mut self, ui: &mut Ui, list: &StoreList, idle: bool) {
        let old: Vec<String> = list.packages.iter().zip(&list.flags).filter(|(_, f)| f.removable_old()).map(|(p, _)| p.published.clone()).collect();
        let n = self.drv_ui.ticked.len();
        let remove = if n == 0 { STORE_REMOVE_LABEL.to_string() } else { format!("Remove {n}\u{2026}") };
        ui.horizontal(|ui| {
            if parts::admin_button(ui, REMOVE_OLD_LABEL, false, idle && !old.is_empty()).clicked() {
                self.drv_ui.confirm_remove = Some(RemoveAsk { kind: RemoveKind::Old, names: old.clone(), understood: false });
            }
            if parts::admin_button(ui, &remove, false, idle && n > 0).clicked() {
                self.drv_ui.confirm_remove = Some(RemoveAsk { kind: RemoveKind::Selected, names: self.drv_ui.ticked.names(), understood: false });
            }
            if parts::admin_button(ui, BACKUP_SELECTED_LABEL, false, idle && n > 0).clicked() {
                self.drv_ui.confirm_backup = Some(BackupAsk { names: Some(self.drv_ui.ticked.names()), folder: self.drv_ui.backup_folder.clone() });
            }
        });
    }

    /// The selected package's details and "Copy details".
    fn store_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, list: &StoreList, idle: bool) {
        let at = self.drv_ui.selected_store.as_deref().and_then(|name| list.packages.iter().position(|p| p.published == name));
        let mut ask = None;
        parts::SidePanel::new("drv-store-side").show(ui, at, |ui, at| {
            let th = theme::of_ui(ui);
            let (p, flags) = (&list.packages[at], list.flags[at]);
            ui.label(RichText::new(&p.published).font(display(15.0)).color(th.text));
            ui.add_space(6.0);
            egui::Grid::new("drv-store-grid").striped(false).num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
                for (k, v) in store_detail_rows(p, flags).into_iter().skip(1) {
                    ui.label(RichText::new(k).color(th.label));
                    ui.add(egui::Label::new(v).wrap());
                    ui.end_row();
                }
            });
            if flags.inbox {
                ui.add_space(4.0);
                ui.label(RichText::new(STORE_INBOX_NOTE).size(12.0).color(th.faint));
            }
            ui.add_space(8.0);
            separator(ui);
            ui.add_space(10.0);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                if parts::outline_button(ui, "Copy details", true).clicked() {
                    actions.push(Action::Copy(store_details(p, flags)));
                }
                // Inbox packages are never offered for removal.
                if !flags.inbox {
                    let one = vec![p.published.clone()];
                    if parts::admin_button(ui, STORE_REMOVE_LABEL, false, idle).clicked() {
                        ask = Some(RemoveAsk { kind: RemoveKind::Selected, names: one.clone(), understood: false });
                    }
                    if flags.in_use {
                        ui.label(RichText::new(store::NOT_REMOVED_IN_USE).size(12.0).color(th.faint));
                        if parts::destructive_button(ui, FORCE_LABEL, idle).clicked() {
                            ask = Some(RemoveAsk { kind: RemoveKind::Forced, names: one, understood: false });
                        }
                    }
                }
            });
        });
        if ask.is_some() {
            self.drv_ui.confirm_remove = ask;
        }
    }

    fn drivers_dialogs(&mut self, ui: &mut Ui) {
        let th = theme::of_ui(ui);
        if let Some(sha) = self.drv_ui.confirm_virustotal.clone() {
            let mut decided = None;
            egui::Window::new("Look up on VirusTotal").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label("Send this file's SHA-256 to VirusTotal? The file itself is not uploaded.");
                ui.label(RichText::new("Asked once. Off by default in Settings \u{203a} Add-ons \u{203a} Drivers.").color(th.label));
                ui.horizontal(|ui| {
                    if ui.button("Send SHA-256").clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if decided == Some(true) {
                self.virustotal_consent = true;
                self.look_up_on_virustotal(sha);
            }
            if decided.is_some() {
                self.drv_ui.confirm_virustotal = None;
            }
        }
        if let Some(service) = self.drv_ui.confirm_unload.clone() {
            let mut decided = None;
            egui::Window::new("Unload this driver?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(format!("Windows will be asked to stop \"{service}\"."));
                ui.label(RichText::new("Devices that use it stop working until it loads again or Windows restarts.").color(th.label));
                ui.horizontal(|ui| {
                    if parts::admin_button(ui, "Unload", false, true).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if decided == Some(true) {
                self.drv.unload(service);
            }
            if decided.is_some() {
                self.drv_ui.confirm_unload = None;
            }
        }
        if let Some(u) = self.drv_ui.confirm_older.clone() {
            let mut decided = None;
            egui::Window::new("Install an older driver?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(format!("Windows Update offers {} ({}).", u.model, u.version));
                ui.label(RichText::new("It is not newer than the driver you have. Install it only to go back to that version.").color(th.label));
                crate::widgets::restore_point_option(ui, &mut self.restore_point);
                ui.horizontal(|ui| {
                    if parts::admin_button(ui, &format!("Install {}", u.version), false, true).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if decided == Some(true) {
                self.drv.install_with(vec![u.id.clone()], self.restore_point);
            }
            if decided.is_some() {
                self.drv_ui.confirm_older = None;
            }
        }
        if let Some(mut confirm) = self.drv_ui.confirm_device.clone() {
            let idle = self.drv.lock().busy.is_none();
            let mut decided = None;
            egui::Window::new(format!("{} {}?", confirm.action.label().split(' ').next().unwrap_or_default(), confirm.name))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ui.ctx(), |ui| {
                    ui.label(RichText::new(confirm.action.consequence()).color(th.soft));
                    if let Some(warning) = confirm.warning.clone() {
                        crate::widgets::strong_confirm(ui, &warning, &mut confirm.understood);
                    }
                    crate::widgets::restore_point_option(ui, &mut self.restore_point);
                    if !idle {
                        ui.label(RichText::new(BUSY_TEXT).color(th.amber));
                    }
                    ui.horizontal(|ui| {
                        if parts::admin_button(ui, confirm.action.label(), false, idle && confirm.confirmable()).clicked() {
                            decided = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decided = Some(false);
                        }
                    });
                });
            match decided {
                Some(true) => {
                    self.drv.device_action_with(confirm.action, confirm.instance_id.clone(), self.restore_point);
                    self.drv_ui.confirm_device = None;
                }
                Some(false) => self.drv_ui.confirm_device = None,
                None => self.drv_ui.confirm_device = Some(confirm),
            }
        }
        if let Some(job) = self.drv_ui.confirm_job.clone().filter(|j| !matches!(j, drivers::Redo::Remove { .. })) {
            let (title, what, label) = match &job {
                drivers::Redo::Remove { .. } => ("", String::new(), ""),
                drivers::Redo::Updates(ids) => ("Install driver updates?", format!("Windows Update will download and install {}.", plural(ids.len(), "driver update")), "Install"),
                drivers::Redo::Inf(path) => ("Install this driver package?", format!("{path} will be added and installed on matching devices."), "Install"),
                // Device actions have their own dialog (`confirm_device`).
                drivers::Redo::Device(action, _) => (action.label(), action.consequence().to_string(), action.label()),
            };
            let mut decided = None;
            egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(what);
                crate::widgets::restore_point_option(ui, &mut self.restore_point);
                ui.horizontal(|ui| {
                    if parts::admin_button(ui, label, false, true).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if decided == Some(true) {
                match job {
                    drivers::Redo::Updates(ids) => self.drv.install_with(ids, self.restore_point),
                    drivers::Redo::Inf(path) => self.drv.install_inf_with(path, self.restore_point),
                    drivers::Redo::Remove { .. } => {}
                    drivers::Redo::Device(action, id) => self.drv.device_action_with(action, id, self.restore_point),
                }
            }
            if decided.is_some() {
                self.drv_ui.confirm_job = None;
            }
        }
        self.remove_dialog(ui);
        self.backup_dialog(ui);
        // The restore point could not be made, so nothing was changed yet.
        let prompt = self.drv.lock().restore_prompt.clone();
        if let Some(prompt) = prompt {
            let mut decided = None;
            egui::Window::new("Restore point not created").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
                ui.label(RichText::new(&prompt.reason).color(th.amber));
                ui.label("Continue without a new restore point?");
                ui.horizontal(|ui| {
                    if parts::admin_button(ui, "Continue", false, true).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            match decided {
                Some(true) => self.drv.continue_without_restore(),
                Some(false) => self.drv.cancel_restore(),
                None => {}
            }
        }
    }

    /// The confirmation of a removal: lists the packages, offers the restore
    /// point, and asks for more before a forced removal.
    fn remove_dialog(&mut self, ui: &mut Ui) {
        let Some(ask) = self.drv_ui.confirm_remove.clone() else { return };
        let th = theme::of_ui(ui);
        let packages = self.drv.lock().store.clone();
        let lines: Vec<(String, bool)> = ask
            .names
            .iter()
            .map(|n| match packages.iter().find(|p| &p.published == n) {
                Some(p) => (format!("{} \u{2014} {}, {} {}", p.published, p.original, p.provider, p.version), p.in_use()),
                None => (n.clone(), false),
            })
            .collect();
        let title = match (ask.kind, ask.names.as_slice()) {
            (RemoveKind::Old, names) => format!("Remove {}?", plural(names.len(), "old package")),
            (RemoveKind::Forced, [one]) => format!("Remove {one} even if in use?"),
            (_, [one]) => format!("Remove {one}?"),
            (_, names) => format!("Remove {}?", plural(names.len(), "driver package")),
        };
        let label = match ask.kind {
            RemoveKind::Old => REMOVE_OLD_LABEL,
            RemoveKind::Forced => "Remove even if in use",
            RemoveKind::Selected => "Remove",
        };
        let mut decided = None;
        let mut understood = ask.understood;
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            for (line, in_use) in &lines {
                ui.label(line);
                if *in_use && ask.kind == RemoveKind::Selected {
                    ui.label(RichText::new("In use: will be refused").size(12.0).color(th.amber));
                }
            }
            match ask.kind {
                RemoveKind::Selected => {
                    ui.label(RichText::new(REMOVE_SELECTED_NOTE).size(12.0).color(th.label));
                }
                RemoveKind::Old => {
                    ui.label(RichText::new(REMOVE_OLD_NOTE).size(12.0).color(th.label));
                }
                RemoveKind::Forced => {
                    let name = ask.names.first().map(String::as_str).unwrap_or_default();
                    ui.label(RichText::new(format!("Runs pnputil /delete-driver {name} /uninstall /force.")).size(12.0).color(th.label));
                    let users: Vec<&str> = packages.iter().find(|p| p.published == name).map(|p| p.devices.iter().map(|d| d.name.as_str()).collect()).unwrap_or_default();
                    ui.label(RichText::new(format!("{} stops working and may not come back until you reinstall its driver.", if users.is_empty() { "A device".to_string() } else { users.join(", ") })).strong().color(th.red));
                    ui.checkbox(&mut understood, FORCE_UNDERSTOOD);
                }
            }
            crate::widgets::restore_point_option(ui, &mut self.restore_point);
            ui.horizontal(|ui| {
                let enabled = ask.kind != RemoveKind::Forced || understood;
                if parts::admin_button(ui, label, false, enabled).clicked() {
                    decided = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
            });
        });
        match decided {
            Some(true) => {
                match ask.kind {
                    RemoveKind::Selected => self.drv.remove_packages(ask.names, self.restore_point),
                    RemoveKind::Old => self.drv.remove_old(self.restore_point),
                    RemoveKind::Forced => {
                        if let Some(name) = ask.names.into_iter().next() {
                            self.drv.remove_forced(name, self.restore_point);
                        }
                    }
                }
                self.drv_ui.ticked.clear();
                self.drv_ui.confirm_remove = None;
            }
            Some(false) => self.drv_ui.confirm_remove = None,
            None => {
                if let Some(open) = &mut self.drv_ui.confirm_remove {
                    open.understood = understood;
                }
            }
        }
    }

    /// The backup's folder and confirmation, with the note on restoring it.
    fn backup_dialog(&mut self, ui: &mut Ui) {
        let Some(mut ask) = self.drv_ui.confirm_backup.clone() else { return };
        let th = theme::of_ui(ui);
        let n = match &ask.names {
            Some(names) => names.len(),
            None => store::third_party(&self.drv.lock().store).len(),
        };
        let title = if ask.names.is_none() { "Back up every third-party driver".to_string() } else { format!("Back up {}", plural(n, "selected package")) };
        let dated = store::backup_folder(ask.folder.trim(), today());
        let mut decided = None;
        let mut browse = false;
        egui::Window::new(title).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ui.ctx(), |ui| {
            ui.label(RichText::new("Folder").size(12.0).color(th.label));
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut ask.folder).hint_text("Choose a folder").desired_width(280.0));
                if ui.button("Browse\u{2026}").clicked() {
                    browse = true;
                }
            });
            let sub = dated.rsplit('\\').next().unwrap_or_default();
            ui.label(RichText::new(format!("A dated subfolder is created ({sub}).")).size(12.0).color(th.label));
            ui.label(RichText::new(store::BACKUP_RESTORE_NOTE).size(12.0).color(th.label));
            ui.horizontal(|ui| {
                let label = format!("Back up {}", plural(n, "package"));
                if parts::admin_button(ui, &label, false, !ask.folder.trim().is_empty() && n > 0).clicked() {
                    decided = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decided = Some(false);
                }
            });
        });
        if browse {
            if let Some(folder) = self.context.pick_folder("Back up drivers to", "Choose") {
                ask.folder = folder;
            }
        }
        match decided {
            Some(true) => {
                self.drv_ui.backup_folder = ask.folder.trim().to_string();
                self.drv.back_up(ask.names, dated);
                self.drv_ui.confirm_backup = None;
            }
            Some(false) => self.drv_ui.confirm_backup = None,
            None => self.drv_ui.confirm_backup = Some(ask),
        }
    }

    fn export_drivers_csv(&mut self, devices: &[DeviceDriver], kernel: &[KernelDriver], updates: Option<&[DriverUpdate]>, signatures: &SignatureBook) {
        let Some(path) = self.context.pick_save_as(&crate::platform::CSV_FILE, "drivers.csv") else { return };
        let s = drivers_csv(devices, kernel, updates, signatures);
        match std::fs::write(&path, format!("\u{feff}{s}")) {
            Ok(()) => self.say(format!("Saved {path}")),
            Err(e) => self.say(format!("Could not save: {e}")),
        }
    }
}

/// The CSV text of the export. Text cells are neutralised against spreadsheet
/// formulas; numbers (load index, kernel memory in bytes) stay plain.
/// The kernel-only columns come last and are empty on device rows.
fn drivers_csv(devices: &[DeviceDriver], kernel: &[KernelDriver], updates: Option<&[DriverUpdate]>, signatures: &SignatureBook) -> String {
    let mut s = String::from("Kind,Name,Class/Type,Provider/Vendor,Version,Date,Status,INF/Service,Windows Update,Instance ID/Path,Start type,Driver kind,Load index,Kernel memory (bytes),Load order group,Depends on,Signature\n");
    for d in devices {
        let u = updates.and_then(|u| u.iter().find(|x| x.device.as_deref() == Some(d.instance_id.as_str())));
        let up = match (updates, u) {
            (None, _) => "not checked".to_string(),
            (Some(_), Some(u)) if u.worth_installing() => format!("newer: {}", u.version),
            (Some(_), Some(u)) => format!("up to date (offers older {})", u.version),
            (Some(_), None) => "up to date".to_string(),
        };
        let f = ["Device", &d.name, &d.class, &d.provider, &d.version, &d.date, &d.status(), &d.inf, &up, &d.instance_id];
        let mut line: Vec<String> = f.iter().map(|x| csv_text_field(x)).collect();
        line.extend(std::iter::repeat(String::new()).take(7));
        s.push_str(&line.join(","));
        s.push('\n');
    }
    for d in kernel {
        let depends: Vec<String> = d.depends_services.iter().cloned().chain(d.depends_groups.iter().map(|g| format!("group {g}"))).collect();
        let text = [
            "Kernel driver", &d.display, d.kind.label(), &d.vendor, &d.version, "", if d.running { "Running" } else { "Stopped" },
            &d.service, "", &d.path, d.start_type, d.kind.label(),
        ];
        let mut cells: Vec<String> = text.iter().map(|x| csv_text_field(x)).collect();
        cells.push(csv_field(&d.load_index.map(|i| i.to_string()).unwrap_or_default()));
        cells.push(csv_field(&d.image_size.map(|i| i.to_string()).unwrap_or_default()));
        cells.push(csv_text_field(&d.group));
        cells.push(csv_text_field(&depends.join("; ")));
        cells.push(csv_text_field(&signature_text(signatures.of(&d.path))));
        s.push_str(&cells.join(","));
        s.push('\n');
    }
    s
}

/// Michroma size for the version change: smaller when several are listed.
fn n_font(n: usize) -> f32 {
    if n == 1 { 20.0 } else { 15.0 }
}

/// Identical entries (same name, version and maker) become one row.
/// Takes indexes into `devices` and gives back (index, how many) per row.
fn fold(devices: &[DeviceDriver], idx: Vec<usize>) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    for i in idx {
        let d = &devices[i];
        let same = |o: &DeviceDriver| o.name == d.name && o.version == d.version && o.provider == d.provider;
        match out.iter_mut().find(|f| same(&devices[f.0])) {
            Some(f) => {
                f.1 += 1;
                // Prefer showing an entry that is plugged in.
                if !devices[f.0].present && d.present {
                    f.0 = i;
                }
            }
            None => out.push((i, 1)),
        }
    }
    out
}

/// Installed and offered side by side, the part of the version that
/// changes highlighted.
fn compare(ui: &mut Ui, d: &DeviceDriver, u: &DriverUpdate) {
    let th = theme::of_ui(ui);
    egui::Grid::new("drv-compare").striped(false).num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("");
        ui.label(RichText::new("Installed").size(12.0).color(th.label));
        ui.label(RichText::new("On Windows Update").size(12.0).color(th.amber));
        ui.end_row();
        ui.label(RichText::new("Version").color(th.label));
        let version = |ui: &mut Ui, v: &str, other: &str, col: Color32| {
            let (head, tail) = split_version(v, other);
            let mut job = egui::text::LayoutJob::default();
            job.append(&head, 0.0, egui::TextFormat { font_id: display(13.0), color: th.faint, ..Default::default() });
            job.append(&tail, 0.0, egui::TextFormat { font_id: display(13.0), color: col, ..Default::default() });
            ui.label(job);
        };
        version(ui, &d.version, &u.version, th.text);
        version(ui, &u.version, &d.version, th.amber);
        ui.end_row();
        ui.label(RichText::new("Released").color(th.label));
        ui.label(long_date(&d.date));
        ui.label(long_date(&u.date));
        ui.end_row();
        ui.label(RichText::new("Download").color(th.label));
        ui.label(RichText::new("On this PC").color(th.faint));
        ui.label(fmt_size(u.size as u64));
        ui.end_row();
    });
}

fn or_dash(s: &str) -> String {
    if s.is_empty() { "\u{2014}".into() } else { s.to_string() }
}


/// What the signature check found for a device's driver: its INF.
fn device_signature<'a>(d: &DeviceDriver, signatures: &'a SignatureBook) -> Option<&'a Signature> {
    if d.inf.is_empty() {
        return None;
    }
    signatures.of(&d.inf_path())
}

/// The Signature row (what the check found) and, when there is one, the
/// Signer row (who signed), as both detail panels show them.
fn signature_rows(signature: Option<&Signature>) -> Vec<(&'static str, String)> {
    let mut rows = vec![("Signature", signature_text(signature))];
    if let Some(signer) = signature.and_then(Signature::signer) {
        rows.push(("Signer", signer.to_string()));
    }
    rows
}

fn device_details(d: &DeviceDriver, u: Option<&DriverUpdate>, signature: Option<&Signature>) -> String {
    let mut s = format!(
        "Device: {}\nClass: {}\nManufacturer: {}\nProvider: {}\nVersion: {}\nDate: {}\nINF: {}\nService: {}\nStatus: {}\nInstance ID: {}\nHardware IDs: {}\n",
        d.name,
        d.class,
        d.manufacturer,
        d.provider,
        d.version,
        d.date,
        d.inf,
        d.service,
        d.status(),
        d.instance_id,
        d.hardware_ids.join("; ")
    );
    for (k, v) in signature_rows(signature) {
        s.push_str(&format!("{k}: {v}\n"));
    }
    if let Some(u) = u {
        let what = if u.worth_installing() { "newer" } else { "not newer" };
        s.push_str(&format!("Windows Update offers: {} ({}, {what})\n", u.title, u.version));
    }
    s
}

/// Height of a Driver store row: the name and the INF under it.
const STORE_ROW: f32 = 40.0;

/// The filter box's hint on the Driver store tab.
const STORE_FILTER_HINT: &str = "Package, INF, provider or device";

/// Width of the tick box column of the store's rows.
const STORE_TICK_W: f32 = 28.0;

const REMOVE_OLD_LABEL: &str = "Remove old packages";
const STORE_REMOVE_LABEL: &str = "Remove\u{2026}";
const BACKUP_SELECTED_LABEL: &str = "Back up selected";
const BACKUP_ALL_LABEL: &str = "Back up drivers\u{2026}";
const FORCE_LABEL: &str = "Remove even if in use\u{2026}";
const REMOVE_SELECTED_NOTE: &str = "Never forced. Packages in use are refused.";
const REMOVE_OLD_NOTE: &str = "Only packages flagged Old and not In use.";
const FORCE_UNDERSTOOD: &str = "I understand, continue anyway";

/// Said in the side panel of an inbox package.
const STORE_INBOX_NOTE: &str = "Inbox (Microsoft) package: not offered for removal.";

/// What the strip over the store says: each figure as its value and the
/// words after it, in order. The disk figures say "unknown" when no
/// package's folder could be measured.
fn store_figure_words(f: &StoreFigures) -> Vec<(String, &'static str)> {
    let size = |b: Option<u64>| b.map(fmt_size).unwrap_or_else(|| "unknown".into());
    let mut words = vec![
        (f.packages.to_string(), "packages"),
        (f.third_party.to_string(), "third-party"),
        (f.old.to_string(), "old"),
        (size(f.bytes), "used"),
    ];
    if f.freeable.is_some() {
        words.push((size(f.freeable), "can be freed"));
    }
    words
}

/// The Driver store's figures in a strip, with the reminder that reading
/// needs no administrator rights.
fn store_strip(ui: &mut Ui, f: &StoreFigures) {
    let th = theme::of_ui(ui);
    crate::widgets::strip(ui, |ui| {
        ui.label(RichText::new("Driver store").size(13.0).strong().color(th.soft));
        for (value, what) in store_figure_words(f) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(RichText::new(value).size(13.0).strong().color(th.text));
                ui.label(RichText::new(what).size(13.0).color(th.label));
            });
        }
        ui.label(RichText::new("Read without administrator rights. Microsoft inbox packages are never offered for removal.").size(12.0).color(th.faint));
    });
}

/// A package's detail fields, labelled and in a fixed order. Shared by the
/// side panel (which leaves out the first, its title) and "Copy details".
fn store_detail_rows(p: &StorePackage, flags: Flags) -> Vec<(&'static str, String)> {
    let used_by = if p.devices.is_empty() { "No device".to_string() } else { p.devices.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join("; ") };
    let words = flags.words();
    vec![
        ("Package", p.published.clone()),
        ("Original INF", p.original.clone()),
        ("Provider", or_dash(&p.provider)),
        ("Class", or_dash(&p.class)),
        ("Version", or_dash(&p.version)),
        ("Date", or_dash(&p.date)),
        ("Signer", or_dash(&p.signer)),
        ("Size", p.size.map(fmt_size).unwrap_or_else(|| "\u{2014}".into())),
        ("Used by", used_by),
        ("Flags", if words.is_empty() { "\u{2014}".to_string() } else { words.join(", ") }),
    ]
}

fn store_details(p: &StorePackage, flags: Flags) -> String {
    store_detail_rows(p, flags).iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
}

/// The four kernel figures as tiles: value, label and subtitle.
fn kernel_tiles(ui: &mut Ui, f: KernelFigures) {
    let th = theme::of_ui(ui);
    let tiles = [
        ("Drivers", f.drivers.to_string(), "Registered with Windows"),
        ("Loaded", f.loaded.to_string(), "Linked into the kernel now"),
        ("Third-party", f.third_party.to_string(), "Not shipped by Microsoft"),
        ("Kernel memory", kernel_memory_text(f.memory), "Held by loaded driver images"),
    ];
    let gap = 10.0;
    let w = ((ui.available_width() - gap * 3.0) / 4.0).max(120.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (label, value, sub) in tiles {
            let (r, _) = ui.allocate_exact_size(vec2(w, 72.0), Sense::hover());
            let p = ui.painter();
            p.rect(r, CornerRadius::same(6), th.panel, Stroke::new(1.0, th.line), egui::StrokeKind::Inside);
            p.text(r.left_top() + vec2(12.0, 10.0), Align2::LEFT_TOP, label, text(12.0), th.label);
            p.text(r.left_top() + vec2(12.0, 27.0), Align2::LEFT_TOP, value, text(20.0), th.text);
            p.text(r.left_top() + vec2(12.0, 53.0), Align2::LEFT_TOP, sub, text(11.5), th.faint);
        }
    });
}

/// The empty-filter text in the kernel scope.
const KERNEL_NO_MATCH: &str = "No driver name, display name or vendor matches the filter.";

/// The kernel module's detail fields, labelled and in a fixed order.
/// Shared by the side panel and "Copy details".
fn kernel_detail_rows(d: &KernelDriver, admin: bool, signature: Option<&Signature>) -> Vec<(&'static str, String)> {
    let dash = || "\u{2014}".to_string();
    let load_index = match (d.load_index, d.group.is_empty()) {
        (Some(i), true) => i.to_string(),
        (Some(i), false) => format!("{i} ({})", d.group),
        (None, _) => dash(),
    };
    let mut rows = vec![
        ("Driver", d.display.clone()),
        ("Service name", d.service.clone()),
        ("State", d.state_label().into()),
        ("Kind", d.kind.label().into()),
        ("Start type", d.start_type.into()),
        ("Version", or_dash(&d.version)),
        ("Vendor", or_dash(&d.vendor)),
    ];
    rows.extend(signature_rows(signature));
    rows.push(("Load index", load_index));
    // Rows that exist only when the service has the value.
    if !d.group.is_empty() {
        rows.push(("Load order group", d.group.clone()));
    }
    let depends = d.depends_on_text();
    if !depends.is_empty() {
        rows.push(("Depends on", depends));
    }
    rows.extend([
        ("Kernel memory", d.image_size.map(fmt_size).unwrap_or_else(dash)),
        ("Location", d.path.clone()),
        ("Unload", d.unload_note(admin).1.into()),
    ]);
    rows
}

fn kernel_details(d: &KernelDriver, admin: bool, signature: Option<&Signature>) -> String {
    kernel_detail_rows(d, admin, signature).iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_entries_fold() {
        let a = DeviceDriver { name: "Apex 7".into(), version: "2.11.5.0".into(), provider: "SteelSeries".into(), present: true, ..Default::default() };
        let b = a.clone();
        let c = DeviceDriver { name: "Aerox 9".into(), ..a.clone() };
        let rows = fold(&[a, b, c], vec![0, 1, 2]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, 2);
    }

    /// A reader that has nothing: the tests fill the hub's lists by hand.
    struct NoReader;
    impl crate::addons::drivers::ListingReader for NoReader {
        fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
            Ok(Vec::new())
        }
        fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
            Ok(Vec::new())
        }
    }

    pub(super) fn page_with(devices: Vec<DeviceDriver>, kernel: Vec<KernelDriver>) -> DriversPage {
        page_and_runner(devices, kernel).0
    }

    pub(super) fn page_and_runner(devices: Vec<DeviceDriver>, kernel: Vec<KernelDriver>) -> (DriversPage, Arc<crate::platform::RecordingRunner>) {
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx.clone(), Arc::new(NoReader));
        {
            let mut st = hub.lock();
            st.devices = devices.into();
            st.kernel = kernel.into();
        }
        let mut page = DriversPage::new(hub, ctx);
        page.drv_ui.started = true;
        (page, runner)
    }

    pub(super) fn gpu(name: &str) -> DeviceDriver {
        DeviceDriver {
            instance_id: name.into(),
            name: name.into(),
            class: "Display".into(),
            provider: "Acme".into(),
            version: "1.0.0.0".into(),
            date: "2025-01-01".into(),
            present: true,
            ..Default::default()
        }
    }

    /// Every text of one frame of the page, in drawing order.
    fn frame(page: &mut DriversPage) -> Vec<String> {
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))), ..Default::default() };
        let mut out = Vec::new();
        // Two passes: the first one only sizes what the page lays out.
        for _ in 0..2 {
            out.clear();
            let output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| page.ui(ui, &mut Vec::new()));
            });
            output.shapes.iter().for_each(|s| texts(&s.shape, &mut out));
        }
        out
    }

    #[test]
    fn counts_and_groups_are_worked_out_once_per_change_of_data_or_filter() {
        let mut page = page_with(vec![gpu("Alpha GPU"), gpu("Beta GPU")], vec![KernelDriver { service: "disk".into(), display: "Disk".into(), ..Default::default() }]);
        for _ in 0..5 {
            frame(&mut page);
        }
        assert_eq!(page.derived_builds(), 1, "five frames, nothing changed");

        page.drv_ui.search = "alpha".into();
        let shown = frame(&mut page);
        frame(&mut page);
        assert_eq!(page.derived_builds(), 2, "the filter changed once");
        assert!(shown.iter().any(|t| t == "Alpha GPU") && !shown.iter().any(|t| t == "Beta GPU"));

        page.drv.lock().devices = vec![gpu("Alpha GPU"), gpu("Beta GPU"), gpu("Gamma GPU")].into();
        frame(&mut page);
        frame(&mut page);
        assert_eq!(page.derived_builds(), 3, "a new read of the devices");

        page.drv_ui.scope = Scope::Kernel;
        frame(&mut page);
        frame(&mut page);
        assert_eq!(page.derived_builds(), 4, "another scope");
        page.drv_ui.kernel_filter = KernelScope::Loaded;
        frame(&mut page);
        frame(&mut page);
        assert_eq!(page.derived_builds(), 5, "the kernel scope changed");
    }

    fn loaded(service: &str, vendor: &str, path: &str, size: Option<u64>) -> KernelDriver {
        KernelDriver { service: service.into(), display: service.into(), vendor: vendor.into(), running: true, path: path.into(), image_size: size, ..Default::default() }
    }

    fn tile_values(page: &mut DriversPage) -> Vec<String> {
        let t = frame(page);
        let at = t.iter().position(|x| x == "Drivers").expect("the tiles are shown");
        // Each tile draws its label, value and subtitle in turn.
        (0..4).map(|i| t[at + i * 3 + 1].clone()).collect()
    }

    #[test]
    fn figures_follow_the_text_filter_and_not_the_scope() {
        let kernel = vec![
            loaded("nvlddmkm", "NVIDIA", "nv.sys", Some(2048)),
            loaded("disk", "Microsoft", "disk.sys", Some(1024)),
            KernelDriver { service: "vbox".into(), display: "vbox".into(), vendor: "Oracle".into(), ..Default::default() },
        ];
        let mut page = page_with(Vec::new(), kernel);
        page.drv_ui.scope = Scope::Kernel;
        assert_eq!(tile_values(&mut page), ["3", "2", "2", "3.0 KB"]);
        page.drv_ui.search = "nvlddmkm".into();
        assert_eq!(tile_values(&mut page), ["1", "1", "1", "2.0 KB"]);
        // Another scope changes the rows, not the figures.
        page.drv_ui.kernel_filter = KernelScope::NotLoaded;
        assert_eq!(tile_values(&mut page), ["1", "1", "1", "2.0 KB"]);
    }

    #[test]
    fn an_unreadable_module_list_says_an_unknown_amount_and_a_note() {
        let mut row = loaded("disk", "Microsoft", "disk.sys", None);
        row.modules_unread = true;
        let mut page = page_with(Vec::new(), vec![row]);
        page.drv_ui.scope = Scope::Kernel;
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "an unknown amount"), "{t:?}");
        assert!(t.iter().any(|x| x == KERNEL_MEMORY_NOTE));
    }

    #[test]
    fn the_page_shares_the_lists_instead_of_copying_them() {
        let mut page = page_with(vec![gpu("Alpha GPU")], Vec::new());
        frame(&mut page);
        let st = page.drv.lock();
        let view = page.derived.as_ref().unwrap();
        assert!(Arc::ptr_eq(&view.devices, &st.devices), "the page holds the hub's snapshot");
    }

    #[test]
    fn rows_come_out_grouped_sorted_and_folded_as_before() {
        let mut devices = vec![gpu("beta GPU"), gpu("Alpha GPU"), gpu("beta GPU")];
        devices.push(DeviceDriver { class: "Net".into(), ..gpu("Zeta NIC") });
        let mut page = page_with(devices, Vec::new());
        let shown = frame(&mut page);
        let at = |what: &str| shown.iter().position(|t| t == what).unwrap_or_else(|| panic!("{what} is shown: {shown:?}"));
        assert!(at("Display") < at("Alpha GPU") && at("Alpha GPU") < at("beta GPU") && at("beta GPU") < at("Network") && at("Network") < at("Zeta NIC"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "2 entries folded into one"));
        assert_eq!(shown.iter().filter(|t| *t == "beta GPU").count(), 1, "identical entries are one row");
    }

    #[test]
    fn only_the_rows_in_view_are_built_and_the_selection_stays() {
        let devices: Vec<DeviceDriver> = (0..3000).map(|i| gpu(&format!("Driver {i:04}"))).collect();
        let mut page = page_with(devices, Vec::new());
        page.drv_ui.selected_device = Some("Driver 0002".into());
        let shown = frame(&mut page);
        assert!(shown.iter().any(|t| t == "Driver 0000"), "the first rows are built");
        assert!(!shown.iter().any(|t| t == "Driver 2999"), "the last row is not");
        let rows = shown.iter().filter(|t| t.starts_with("Driver 0") || t.starts_with("Driver 1") || t.starts_with("Driver 2")).count();
        assert!(rows < 100, "{rows} rows built for a screen of rows");
        assert!(shown.iter().any(|t| t == "Open in Device Manager"), "the side panel shows the selected driver");
    }

    #[test]
    fn the_filter_tab_says_why_it_has_no_rows() {
        let list = |names: &[&str]| FilterRead::Filters(names.iter().map(|n| FilterDriver { name: n.to_string(), altitude: "1".into(), instances: 1, frame: 0 }).collect());
        assert_eq!(filter_note(None, 0, false).unwrap().0, "Reading the filters\u{2026}");
        assert_eq!(filter_note(Some(&FilterRead::NeedsAdmin), 0, false), Some(("Needs administrator rights to list filters.".to_string(), None)));
        assert_eq!(filter_note(Some(&FilterRead::Failed("boom".into())), 0, false), Some(("Filters could not be listed".to_string(), Some("boom".to_string()))));
        assert_eq!(filter_note(Some(&list(&[])), 0, false).unwrap().0, "No filters are loaded.");
        assert_eq!(filter_note(Some(&list(&["a"])), 0, true).unwrap().0, "No filter matches what you typed.");
        assert_eq!(filter_note(Some(&list(&["a"])), 1, true), None);
    }

    #[test]
    fn the_filter_search_matches_names_in_any_case() {
        let f = |n: &str| FilterDriver { name: n.into(), altitude: "1".into(), instances: 1, frame: 0 };
        let all = [f("WdFilter"), f("bindflt"), f("FileCrypt")];
        assert_eq!(filter_rows(&all, "").len(), 3);
        assert_eq!(filter_rows(&all, "wdf").iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["WdFilter"]);
        assert_eq!(filters_count(&FilterRead::NeedsAdmin), 0);
    }

    #[test]
    fn the_other_tabs_still_draw_without_the_filter_tab_in_the_way() {
        let mut page = page_with(vec![gpu("Alpha GPU")], vec![loaded("disk", "Microsoft", "disk.sys", Some(1024))]);
        assert!(frame(&mut page).iter().any(|t| t == "Alpha GPU"));
        page.drv_ui.scope = Scope::Kernel;
        assert!(frame(&mut page).iter().any(|t| t == "disk"));
    }

    #[test]
    fn kinds_group_audio_together() {
        assert_eq!(kind_of("MEDIA"), "Sound");
        assert_eq!(kind_of("AudioProcessingObject"), "Sound");
        assert_eq!(kind_of("HIDClass"), "Mice, keyboards and controllers");
    }

    // ---------------------------------------------------- signatures (#25)

    fn book(entries: &[(&str, Signature)]) -> Arc<SignatureBook> {
        let mut b = SignatureBook::default();
        for (path, sig) in entries {
            b.insert(path, sig.clone());
        }
        Arc::new(b)
    }

    fn whql() -> Signature {
        Signature::Whql { signer: crate::addons::drivers::signature::WHQL_PUBLISHER.into() }
    }

    fn kernel_page(signatures: Arc<SignatureBook>) -> DriversPage {
        let kernel = vec![
            loaded("nvlddmkm", "NVIDIA", r"C:\d\nv.sys", Some(2048)),
            loaded("cpuz", "CPUID", r"C:\d\cpuz.sys", Some(1024)),
            loaded("mycomp", "", r"C:\d\mycomp.sys", None),
            loaded("fresh", "Acme", r"C:\d\fresh.sys", None),
        ];
        let mut page = page_with(Vec::new(), kernel);
        page.drv.lock().signatures = signatures;
        page.drv_ui.scope = Scope::Kernel;
        page
    }

    fn sample_book() -> Arc<SignatureBook> {
        book(&[
            (r"C:\d\nv.sys", whql()),
            (r"C:\d\cpuz.sys", Signature::Unsigned),
            (r"C:\d\mycomp.sys", Signature::CouldNotCheck("the catalog database is busy".into())),
        ])
    }

    #[test]
    fn kernel_modules_have_a_signature_column_with_every_value() {
        let mut page = kernel_page(sample_book());
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "Signature"), "the column header: {t:?}");
        for value in ["WHQL", "Unsigned", "Not checked"] {
            assert!(t.iter().any(|x| x == value), "{value} is shown: {t:?}");
        }
        // A long result is cut to the column's width.
        assert!(t.iter().any(|x| x.starts_with("Could not check")), "{t:?}");
    }

    #[test]
    fn the_unsigned_scope_has_its_count_and_lists_only_unsigned_modules() {
        let mut page = kernel_page(sample_book());
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "Unsigned  1"), "the scope button counts the unsigned ones: {t:?}");
        assert!(t.iter().any(|x| x == "All  4"));
        page.drv_ui.kernel_filter = KernelScope::Unsigned;
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "cpuz"), "{t:?}");
        for hidden in ["nvlddmkm", "mycomp", "fresh"] {
            assert!(!t.iter().any(|x| x == hidden), "{hidden} is not unsigned: {t:?}");
        }
        assert!(t.iter().any(|x| x == "1 shown"));
    }

    #[test]
    fn a_signature_that_arrives_later_updates_the_count_once_and_the_rest_is_not_redone() {
        let mut page = kernel_page(Arc::new(SignatureBook::default()));
        for _ in 0..3 {
            frame(&mut page);
        }
        assert_eq!(page.derived_builds(), 1, "three frames, nothing changed");
        assert!(frame(&mut page).iter().any(|x| x == "Unsigned  0"));
        page.drv.lock().signatures = sample_book();
        let t = frame(&mut page);
        frame(&mut page);
        assert_eq!(page.derived_builds(), 2, "the new results are worked out once, in the same derived data");
        assert!(t.iter().any(|x| x == "Unsigned  1"), "{t:?}");
    }

    #[test]
    fn both_detail_panels_show_a_signature_row_and_the_signer() {
        let mut page = kernel_page(sample_book());
        page.drv_ui.selected_kernel = Some("nvlddmkm".into());
        let t = frame(&mut page);
        let at = t.iter().position(|x| x == "Signer").unwrap_or_else(|| panic!("a Signer row: {t:?}"));
        assert_eq!(t[at + 1], crate::addons::drivers::signature::WHQL_PUBLISHER);
        assert!(t.iter().filter(|x| *x == "Signature").count() >= 2, "the header and the panel's row: {t:?}");

        let device = DeviceDriver { inf: "camo.inf".into(), provider: "Reincubate".into(), ..gpu("Camo") };
        let mut page = page_with(vec![device.clone()], Vec::new());
        page.drv.lock().signatures = book(&[(&device.inf_path(), Signature::Signed { signer: "Reincubate Ltd".into() })]);
        page.drv_ui.selected_device = Some("Camo".into());
        let t = frame(&mut page);
        let at = t.iter().position(|x| x == "Signature").unwrap_or_else(|| panic!("a Signature row: {t:?}"));
        assert_eq!(t[at + 1], "Signed");
        assert_eq!(t[at + 2], "Signer");
        assert_eq!(t[at + 3], "Reincubate Ltd");
    }

    #[test]
    fn a_driver_not_checked_yet_says_so_and_has_no_signer_row() {
        let rows = kernel_detail_rows(&KernelDriver::default(), true, None);
        assert!(rows.contains(&("Signature", "Not checked".to_string())));
        assert!(!rows.iter().any(|(k, _)| *k == "Signer"));
        let failed = Signature::CouldNotCheck("in use".into());
        assert!(kernel_details(&KernelDriver::default(), true, Some(&failed)).contains("Signature: Could not check: in use\n"));
        let d = DeviceDriver { name: "Camo".into(), ..Default::default() };
        assert!(device_details(&d, None, None).contains("Signature: Not checked\n"));
        assert!(device_details(&d, None, Some(&whql())).contains(&format!("Signer: {}\n", crate::addons::drivers::signature::WHQL_PUBLISHER)));
    }

    #[test]
    fn the_csv_fills_the_signature_column_for_kernel_modules_only() {
        let k = |service: &str, path: &str| KernelDriver { service: service.into(), path: path.into(), ..Default::default() };
        let signatures = book(&[(r"C:\d\a.sys", whql()), (r"C:\d\b.sys", Signature::CouldNotCheck("busy, try later".into()))]);
        let device = DeviceDriver { name: "Dev".into(), ..Default::default() };
        let csv = drivers_csv(&[device], &[k("a", r"C:\d\a.sys"), k("b", r"C:\d\b.sys"), k("c", r"C:\d\c.sys")], None, &signatures);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].ends_with(",Depends on,Signature"));
        assert!(lines[1].ends_with(",,,,,,,"), "a device row leaves it empty: {}", lines[1]);
        assert!(lines[2].ends_with(",WHQL"), "{}", lines[2]);
        assert!(lines[3].ends_with(",\"Could not check: busy, try later\""), "a cell with a comma is quoted: {}", lines[3]);
        assert!(lines[4].ends_with(",Not checked"), "{}", lines[4]);
    }
}

#[cfg(test)]
mod kernel_detail_tests {
    use super::*;

    #[test]
    fn csv_export_carries_kernel_columns_and_neutralises_text() {
        let k = KernelDriver {
            service: "=svc".into(), display: "@Disk".into(), vendor: "-Acme".into(), running: true, start_type: "Boot",
            load_index: Some(7), image_size: Some(4096), group: "+Base".into(),
            depends_services: vec!["Tcpip".into()], depends_groups: vec!["NDIS".into()], ..Default::default()
        };
        let csv = drivers_csv(&[], &[k], None, &SignatureBook::default());
        let mut lines = csv.lines();
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();
        assert_eq!(&header[10..], ["Start type", "Driver kind", "Load index", "Kernel memory (bytes)", "Load order group", "Depends on", "Signature"]);
        let row = lines.next().unwrap();
        assert!(row.contains(",'@Disk,") && row.contains(",'-Acme,") && row.contains(",'=svc,"));
        assert!(row.ends_with(",Boot,Kernel,7,4096,'+Base,Tcpip; group NDIS,Not checked"), "{row}");
    }

    #[test]
    fn csv_export_device_rows_keep_their_columns() {
        let d = DeviceDriver { name: "=Evil".into(), instance_id: "PCI-X".into(), ..Default::default() };
        let csv = drivers_csv(&[d], &[], None, &SignatureBook::default());
        let row = csv.lines().nth(1).unwrap();
        assert!(row.starts_with("Device,'=Evil,"), "{row}");
        assert!(row.ends_with(",not checked,PCI-X,,,,,,,"), "{row}");
    }

    fn sample() -> KernelDriver {
        KernelDriver {
            service: "acpiex".into(),
            display: "Microsoft ACPIEx Driver".into(),
            running: true,
            start_type: "Boot",
            start_code: Some(0),
            kind: drivers::KernelKind::Kernel,
            path: r"C:\Windows\System32\Drivers\acpiex.sys".into(),
            version: "10.0.26100.1".into(),
            vendor: "Microsoft Corporation".into(),
            group: "Boot Bus Extender".into(),
            image_size: Some(155_648),
            load_index: Some(7),
            ..Default::default()
        }
    }

    #[test]
    fn copy_details_uses_fixed_labels_in_order() {
        let expected = "Driver: Microsoft ACPIEx Driver\n\
Service name: acpiex\n\
State: Running\n\
Kind: Kernel\n\
Start type: Boot\n\
Version: 10.0.26100.1\n\
Vendor: Microsoft Corporation\n\
Signature: Not checked\n\
Load index: 7 (Boot Bus Extender)\n\
Load order group: Boot Bus Extender\n\
Kernel memory: 152.0 KB\n\
Location: C:\\Windows\\System32\\Drivers\\acpiex.sys\n\
Unload: Not offered: Windows loaded it at boot, and the kernel refuses to stop a driver it started itself.\n";
        assert_eq!(kernel_details(&sample(), true, None), expected);
    }

    #[test]
    fn missing_values_show_a_dash() {
        let d = KernelDriver { running: false, version: String::new(), vendor: String::new(), group: String::new(), image_size: None, load_index: None, ..sample() };
        let text = kernel_details(&d, false, None);
        assert!(text.contains("State: Stopped\n"));
        assert!(text.contains("Version: \u{2014}\n"));
        assert!(text.contains("Vendor: \u{2014}\n"));
        assert!(text.contains("Load index: \u{2014}\n"));
        assert!(text.contains("Kernel memory: \u{2014}\n"));
        assert!(text.contains("Unload: Not loaded.\n"));
    }

    #[test]
    fn group_and_dependencies_rows_appear_only_when_present_and_in_order() {
        let d = KernelDriver { depends_services: vec!["Tcpip".into()], depends_groups: vec!["NDIS".into()], ..sample() };
        let text = kernel_details(&d, true, None);
        assert!(text.contains("Load index: 7 (Boot Bus Extender)\nLoad order group: Boot Bus Extender\nDepends on: Tcpip, NDIS (group)\nKernel memory: "), "{text}");
        let bare = kernel_details(&KernelDriver { group: String::new(), ..sample() }, true, None);
        assert!(!bare.contains("Load order group") && !bare.contains("Depends on"), "{bare}");
        assert!(kernel_details(&sample(), true, None).contains("Load order group: Boot Bus Extender\n"));
        assert!(!kernel_details(&sample(), true, None).contains("Depends on"));
    }

    #[test]
    fn pending_states_show_in_the_state_row() {
        let d = KernelDriver { running: false, state_code: Some(6), ..sample() };
        assert!(kernel_details(&d, true, None).contains("State: Pause pending\n"));
    }

    #[test]
    fn load_index_without_group_has_no_parentheses() {
        let d = KernelDriver { group: String::new(), ..sample() };
        assert!(kernel_details(&d, true, None).contains("Load index: 7\n"));
    }

    #[test]
    fn empty_filter_uses_the_kernel_no_match_text() {
        assert_eq!(KERNEL_NO_MATCH, "No driver name, display name or vendor matches the filter.");
    }
}

#[cfg(test)]
mod list_note_tests {
use super::*;
use super::tests::{gpu, page_and_runner, page_with};

    /// The note for an empty list (`shown` 0) with these inputs.
    fn note(listing: bool, error: Option<&str>, total: usize, searching: bool) -> Option<ListNote> {
        list_note(listing, error, total, 0, searching)
    }

    #[test]
    fn rows_to_show_need_no_note() {
        assert_eq!(list_note(true, Some("x"), 3, 3, true), None);
    }

    #[test]
    fn reading_with_nothing_yet_says_so_per_scope() {
        assert_eq!(note(true, None, 0, false), Some(ListNote::Reading));
        assert_eq!(
            ListNote::Reading.texts(Scope::Kernel),
            ("Asking the service control manager for every driver registered on this machine.".to_string(), None)
        );
        assert_eq!(ListNote::Reading.texts(Scope::Hardware), ("Reading the drivers\u{2026}".to_string(), None));
        assert_eq!(ListNote::Reading.texts(Scope::Builtin), ("Reading the drivers\u{2026}".to_string(), None));
    }

    #[test]
    fn a_failed_read_shows_the_error_with_its_reason() {
        let n = note(false, Some("access is denied"), 0, false);
        assert_eq!(n, Some(ListNote::Failed("access is denied".into())));
        assert_eq!(n.unwrap().texts(Scope::Kernel), ("Drivers could not be enumerated".to_string(), Some("access is denied".to_string())));
    }

    #[test]
    fn a_failed_read_wins_over_the_search() {
        assert_eq!(note(false, Some("gone"), 0, true), Some(ListNote::Failed("gone".into())));
    }

    #[test]
    fn nothing_reported_is_not_an_empty_filter() {
        let n = note(false, None, 0, true);
        assert_eq!(n, Some(ListNote::NothingReported));
        assert_eq!(
            ListNote::NothingReported.texts(Scope::Kernel),
            ("No drivers were reported".to_string(), Some("Windows published no driver registrations.".to_string()))
        );
    }

    #[test]
    fn filter_states_differ_from_empty_states() {
        assert_eq!(note(false, None, 40, false), Some(ListNote::FilteredOut));
        assert_eq!(ListNote::FilteredOut.texts(Scope::Hardware), ("No drivers here.".to_string(), None));
        assert_eq!(ListNote::FilteredOut.texts(Scope::Kernel), ("No matching drivers.".to_string(), None));
        assert_eq!(note(false, None, 40, true), Some(ListNote::NoMatch));
        assert_eq!(ListNote::NoMatch.texts(Scope::Hardware), ("No driver or maker matches what you typed.".to_string(), None));
        assert_eq!(ListNote::NoMatch.texts(Scope::Kernel), (KERNEL_NO_MATCH.to_string(), None));
    }

    #[test]
    fn a_new_read_over_older_rows_shows_the_filter_state() {
        assert_eq!(note(true, None, 40, false), Some(ListNote::FilteredOut));
    }

    /// One persistent egui context: the tests click through several frames.
    struct Screen {
        ctx: egui::Context,
        time: f64,
        actions: Vec<Action>,
    }

    impl Screen {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            Screen { ctx, time: 0.0, actions: Vec::new() }
        }

        /// One frame with `events`; the texts drawn, with their positions.
        fn step(&mut self, page: &mut DriversPage, events: Vec<egui::Event>) -> Vec<(Pos2, String)> {
            fn texts(shape: &egui::Shape, out: &mut Vec<(Pos2, String)>) {
                match shape {
                    egui::Shape::Text(t) => out.push((t.pos, t.galley.text().to_string())),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                    _ => {}
                }
            }
            self.time += 0.1;
            let input = egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 800.0))),
                time: Some(self.time),
                events,
                ..Default::default()
            };
            let mut mine = Vec::new();
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| page.ui(ui, &mut mine));
            });
            self.actions.extend(mine);
            let mut out = Vec::new();
            output.shapes.iter().for_each(|s| texts(&s.shape, &mut out));
            out
        }

        fn settle(&mut self, page: &mut DriversPage) -> Vec<(Pos2, String)> {
            self.step(page, vec![]);
            self.step(page, vec![])
        }

        fn click(&mut self, page: &mut DriversPage, at: Pos2, button: egui::PointerButton) -> Vec<(Pos2, String)> {
            let m = egui::Modifiers::NONE;
            self.step(page, vec![egui::Event::PointerMoved(at)]);
            self.step(page, vec![egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
            self.step(page, vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
            self.settle(page)
        }
    }

    fn at(texts: &[(Pos2, String)], what: &str) -> Pos2 {
        texts.iter().find(|(_, t)| t == what).unwrap_or_else(|| panic!("{what:?} is not drawn: {texts:?}")).0 + vec2(4.0, 4.0)
    }

    fn module(path: &str) -> KernelDriver {
        KernelDriver { service: "acme".into(), display: "Acme Kernel".into(), running: true, start_code: Some(3), path: path.into(), ..Default::default() }
    }

    /// The page on Kernel modules with a right click on the only row; the
    /// menu items, top to bottom, with their heights.
    fn opened_menu(screen: &mut Screen, page: &mut DriversPage) -> (Pos2, Vec<(f32, String)>) {
        page.drv_ui.scope = Scope::Kernel;
        screen.settle(page);
        let row = at(&screen.settle(page), "Acme Kernel");
        let after = screen.click(page, row, egui::PointerButton::Secondary);
        let mut items: Vec<(f32, String)> = after
            .into_iter()
            .filter(|(p, t)| p.y > row.y + 2.0 && p.x >= row.x && p.x < row.x + 300.0 && !t.is_empty())
            .map(|(p, t)| (p.y, t))
            .collect();
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        (row, items)
    }

    #[test]
    fn the_kernel_menu_lists_common_actions_first_then_the_advanced_ones() {
        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let (_, items) = opened_menu(&mut screen, &mut page);
        let labels: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            labels,
            ["Open file location", "Open Device Manager", "Copy details", "Advanced", "Copy service name", "Copy SHA-256", "Unload\u{2026}"],
            "Unload sits among the advanced actions, under their caption"
        );
        let gap = |a: usize, b: usize| items[b].0 - items[a].0;
        assert!(gap(1, 2) > gap(0, 1), "a separator sits between Open Device Manager and Copy details");
    }

    #[test]
    fn the_menu_hides_unload_where_it_is_not_offered() {
        let boot = KernelDriver { start_code: Some(0), ..module(r"C:\x\acme.sys") };
        let (mut page, mut screen) = (page_with(vec![], vec![boot]), Screen::new());
        let (_, items) = opened_menu(&mut screen, &mut page);
        assert!(!items.iter().any(|(_, t)| t.starts_with("Unload")), "{items:?}");
        assert!(items.iter().any(|(_, t)| t == "Copy service name"));
    }

    #[test]
    fn the_menu_entries_still_copy_and_open_device_manager() {
        let ((mut page, runner), mut screen) = (page_and_runner(vec![], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let (row, items) = opened_menu(&mut screen, &mut page);
        let y = items.iter().find(|(_, t)| t == "Copy service name").unwrap().0;
        screen.click(&mut page, Pos2::new(row.x + 10.0, y + 4.0), egui::PointerButton::Primary);
        assert_eq!(screen.actions, vec![Action::Copy("acme".into())]);
        let (_, items) = opened_menu(&mut screen, &mut page);
        let y = items.iter().find(|(_, t)| t == "Open Device Manager").unwrap().0;
        screen.click(&mut page, Pos2::new(row.x + 10.0, y + 4.0), egui::PointerButton::Primary);
        assert_eq!(runner.launches(), vec![crate::platform::Launch::Open("devmgmt.msc".into())]);
    }

    #[test]
    fn the_menu_offers_the_virustotal_lookup_only_when_the_option_is_on() {
        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let (_, items) = opened_menu(&mut screen, &mut page);
        assert!(!items.iter().any(|(_, t)| t.contains("VirusTotal")), "{items:?}");

        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\x\acme.sys")]), Screen::new());
        page.virustotal = true;
        let (_, items) = opened_menu(&mut screen, &mut page);
        let labels: Vec<&str> = items.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(labels[4..], ["Copy service name", "Copy SHA-256", "Look up on VirusTotal", "Unload\u{2026}"]);
    }

    #[test]
    fn copy_sha256_from_the_menu_says_when_the_file_is_missing() {
        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\no\such\acme.sys")]), Screen::new());
        let (row, items) = opened_menu(&mut screen, &mut page);
        let y = items.iter().find(|(_, t)| t == "Copy SHA-256").unwrap().0;
        screen.click(&mut page, Pos2::new(row.x + 10.0, y + 4.0), egui::PointerButton::Primary);
        assert_eq!(screen.actions, vec![Action::Say(r"The file C:\no\such\acme.sys was not found.".into())]);
    }

    #[test]
    fn the_consent_dialog_asks_the_question_and_sends_nothing_before_the_answer() {
        let (mut page, runner) = page_and_runner(vec![], vec![module(r"C:\x\acme.sys")]);
        page.virustotal = true;
        page.drv_ui.confirm_virustotal = Some("a".repeat(64));
        let shown = Screen::new().settle(&mut page).into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n");
        assert!(shown.contains("Send this file's SHA-256 to VirusTotal? The file itself is not uploaded."), "{shown}");
        assert!(shown.contains("Send SHA-256") && shown.contains("Cancel"), "{shown}");
        assert!(runner.launches().is_empty());
    }

    #[test]
    fn unload_from_the_menu_asks_for_the_confirmation() {
        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let (row, items) = opened_menu(&mut screen, &mut page);
        let y = items.iter().find(|(_, t)| t.starts_with("Unload")).unwrap().0;
        screen.click(&mut page, Pos2::new(row.x + 10.0, y + 4.0), egui::PointerButton::Primary);
        assert_eq!(page.drv_ui.confirm_unload.as_deref(), Some("acme"));
    }

    #[test]
    fn open_file_location_selects_the_file_or_says_why_not() {
        let there = |_: &std::path::Path| true;
        let gone = |_: &std::path::Path| false;
        assert_eq!(reveal(r"C:\x\acme.sys", there), Action::ShowInExplorer(std::path::PathBuf::from(r"C:\x\acme.sys")));
        assert_eq!(reveal("", there), Action::Say("The executable path is not available for this item.".into()));
        assert_eq!(reveal(r"C:\x\acme.sys", gone), Action::Say(r"The file C:\x\acme.sys was not found.".into()));
    }

    #[test]
    fn open_file_location_in_the_menu_reveals_a_file_that_exists() {
        let here = std::env::current_exe().unwrap().to_string_lossy().to_string();
        let (mut page, mut screen) = (page_with(vec![], vec![module(&here)]), Screen::new());
        let (row, items) = opened_menu(&mut screen, &mut page);
        screen.click(&mut page, Pos2::new(row.x + 10.0, items[0].0 + 4.0), egui::PointerButton::Primary);
        assert_eq!(screen.actions, vec![Action::ShowInExplorer(std::path::PathBuf::from(here))]);
    }

    #[test]
    fn open_file_location_in_the_menu_explains_a_missing_file() {
        let (mut page, mut screen) = (page_with(vec![], vec![module(r"C:\no\such\acme.sys")]), Screen::new());
        let (row, items) = opened_menu(&mut screen, &mut page);
        screen.click(&mut page, Pos2::new(row.x + 10.0, items[0].0 + 4.0), egui::PointerButton::Primary);
        assert_eq!(screen.actions, vec![Action::Say(r"The file C:\no\such\acme.sys was not found.".into())]);
    }

    /// Like [`at`], for a button whose words follow the shield's room.
    fn at_button(texts: &[(Pos2, String)], what: &str) -> Pos2 {
        texts.iter().find(|(_, t)| t.trim() == what).unwrap_or_else(|| panic!("{what:?} is not drawn: {texts:?}")).0 + vec2(20.0, 4.0)
    }

    fn drawn(texts: &[(Pos2, String)], what: &str) -> bool {
        texts.iter().any(|(_, t)| t.trim() == what)
    }

    fn device(name: &str, class: &str) -> DeviceDriver {
        DeviceDriver { class: class.into(), instance_id: format!(r"ROOT\{name}\0000"), ..gpu(name) }
    }

    /// Right click on the only device row; every text drawn afterwards.
    fn device_menu(screen: &mut Screen, page: &mut DriversPage, name: &str) -> Vec<(Pos2, String)> {
        let row = at(&screen.settle(page), name);
        screen.click(page, row, egui::PointerButton::Secondary)
    }

    fn wait_for_launches(runner: &crate::platform::RecordingRunner, n: usize) {
        for _ in 0..200 {
            if runner.launches().len() >= n {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn the_device_menu_offers_disable_and_restart_and_a_disabled_device_offers_enable() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        let shown = device_menu(&mut screen, &mut page, "Alpha NIC");
        assert!(drawn(&shown, "Disable device") && drawn(&shown, "Restart device") && !drawn(&shown, "Enable device"), "{shown:?}");
        for old in ["Open the driver's folder", "Copy details", "Search online"] {
            assert!(drawn(&shown, old), "{old} is still in the menu");
        }

        let off = DeviceDriver { problem: 22, ..device("Beta NIC", "Net") };
        let (mut page, mut screen) = (page_with(vec![off], vec![]), Screen::new());
        let shown = device_menu(&mut screen, &mut page, "Beta NIC");
        assert!(drawn(&shown, "Enable device") && drawn(&shown, "Restart device") && !drawn(&shown, "Disable device"), "{shown:?}");
    }

    #[test]
    fn the_panel_offers_the_device_actions_with_the_same_words() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Alpha NIC\0000".into());
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "Disable device") && drawn(&shown, "Restart device"), "{shown:?}");
        assert!(!shown.iter().any(|(_, t)| t.contains("stronger confirmation")), "an ordinary device has no stronger warning");
    }

    #[test]
    fn the_panel_says_a_protected_device_asks_for_a_stronger_confirmation() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha GPU", "Display")], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Alpha GPU\0000".into());
        let shown = screen.settle(&mut page);
        assert!(shown.iter().any(|(_, t)| t.starts_with("This is a display adapter") && t.contains("stronger confirmation")), "{shown:?}");
    }

    fn broken(name: &str, code: u32) -> DeviceDriver {
        DeviceDriver { problem: code, ..device(name, "Net") }
    }

    fn says(texts: &[(Pos2, String)], part: &str) -> bool {
        texts.iter().any(|(_, t)| t.contains(part))
    }

    #[test]
    fn no_problem_means_no_scope_and_no_band_line() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net"), DeviceDriver { present: false, problem: 45, ..device("Gone NIC", "Net") }], vec![]), Screen::new());
        page.drv_ui.show_disconnected = true;
        let shown = screen.settle(&mut page);
        assert!(!says(&shown, "Problems") && !says(&shown, "have a problem") && !says(&shown, "has a problem") && !drawn(&shown, "Show them"), "{shown:?}");
    }

    #[test]
    fn the_band_counts_the_devices_with_a_problem_and_show_them_opens_the_problems_scope() {
        let devices = vec![device("Alpha NIC", "Net"), broken("Beta NIC", 10), broken("Gamma NIC", 28), DeviceDriver { present: false, ..broken("Gone NIC", 45) }];
        let (mut page, mut screen) = (page_with(devices, vec![]), Screen::new());
        page.drv_ui.show_disconnected = true;
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "2 devices have a problem"), "a disconnected device is not counted: {shown:?}");
        assert!(shown.iter().any(|(_, t)| t.starts_with("Problems")), "the scope is there: {shown:?}");
        assert!(drawn(&shown, "Alpha NIC"), "the usual scope lists every device");

        let shown = screen.click(&mut page, at(&shown, "Show them"), egui::PointerButton::Primary);
        assert!(page.drv_ui.scope == Scope::Problems);
        assert!(drawn(&shown, "Beta NIC") && drawn(&shown, "Gamma NIC"), "{shown:?}");
        assert!(!drawn(&shown, "Alpha NIC") && !drawn(&shown, "Gone NIC"), "only the devices with a problem: {shown:?}");
        assert!(drawn(&shown, "Code 10: The device cannot start.") && drawn(&shown, "Code 28: No driver is installed."), "{shown:?}");
    }

    #[test]
    fn a_single_problem_is_told_in_the_singular() {
        let (mut page, mut screen) = (page_with(vec![broken("Beta NIC", 43)], vec![]), Screen::new());
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "1 device has a problem"), "{shown:?}");
    }

    #[test]
    fn the_problems_scope_is_searched_like_the_others_and_says_so_when_empty() {
        let (mut page, mut screen) = (page_with(vec![broken("Beta NIC", 10), broken("Gamma NIC", 28)], vec![]), Screen::new());
        page.drv_ui.scope = Scope::Problems;
        page.drv_ui.search = "gamma".into();
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "Gamma NIC") && !drawn(&shown, "Beta NIC"), "{shown:?}");

        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        page.drv_ui.scope = Scope::Problems;
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "No device has a problem."), "{shown:?}");
    }

    #[test]
    fn the_scope_count_is_worked_out_with_the_other_counts_once_per_change() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net"), broken("Beta NIC", 10)], vec![]), Screen::new());
        for _ in 0..4 {
            screen.settle(&mut page);
        }
        assert_eq!(page.derived_builds(), 1, "the count came with the first build");
        page.drv_ui.scope = Scope::Problems;
        screen.settle(&mut page);
        screen.settle(&mut page);
        assert_eq!(page.derived_builds(), 2, "the scope changed once");
        page.drv.lock().devices = vec![broken("Beta NIC", 10), broken("Gamma NIC", 28)].into();
        let shown = screen.settle(&mut page);
        screen.settle(&mut page);
        assert_eq!(page.derived_builds(), 3, "a new read of the devices");
        assert!(drawn(&shown, "2 devices have a problem"), "{shown:?}");
    }

    #[test]
    fn the_panel_explains_the_code_and_links_each_suggestion_to_its_action() {
        // 43: Restart the device opens the same dialog as the Restart device button.
        let (mut page, mut screen) = (page_with(vec![broken("Beta NIC", 43)], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Beta NIC\0000".into());
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "Code 43: Windows stopped the device because it reported problems."), "{shown:?}");
        screen.click(&mut page, at(&shown, "Restart the device"), egui::PointerButton::Primary);
        let pending = page.drv_ui.confirm_device.clone().expect("the dialog opens");
        assert!(pending.action == drivers::DeviceAction::Restart && pending.instance_id == r"ROOT\Beta NIC\0000");

        // 22: Enable the device.
        let (mut page, mut screen) = (page_with(vec![broken("Off NIC", 22)], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Off NIC\0000".into());
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "Code 22: The device is disabled."), "{shown:?}");
        screen.click(&mut page, at(&shown, "Enable the device"), egui::PointerButton::Primary);
        assert!(page.drv_ui.confirm_device.as_ref().is_some_and(|c| c.action == drivers::DeviceAction::Enable));

        // 10: the page has no driver action for it without an offer, so it is advice only.
        let (mut page, mut screen) = (page_with(vec![broken("Bad NIC", 10)], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Bad NIC\0000".into());
        let shown = screen.settle(&mut page);
        assert!(drawn(&shown, "Update or reinstall the driver"), "{shown:?}");
        screen.click(&mut page, at(&shown, "Update or reinstall the driver"), egui::PointerButton::Primary);
        assert!(page.drv_ui.confirm_device.is_none() && page.drv_ui.confirm_job.is_none(), "nothing to link to");
    }

    #[test]
    fn a_working_device_has_no_problem_section_in_the_panel() {
        let (mut page, mut screen) = (page_with(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        page.drv_ui.selected_device = Some(r"ROOT\Alpha NIC\0000".into());
        let shown = screen.settle(&mut page);
        assert!(!says(&shown, "Code ") && !drawn(&shown, "Try:"), "{shown:?}");
    }

    #[test]
    fn a_device_action_asks_first_saying_what_will_happen_and_launches_nothing_before_the_answer() {
        let ((mut page, runner), mut screen) = (page_and_runner(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        let shown = device_menu(&mut screen, &mut page, "Alpha NIC");
        screen.click(&mut page, at_button(&shown, "Disable device"), egui::PointerButton::Primary);
        let dialog = screen.settle(&mut page);
        assert!(drawn(&dialog, "The device stops working until you enable it again."), "{dialog:?}");
        assert!(drawn(&dialog, "Create a restore point first") || dialog.iter().any(|(_, t)| t.contains("restore point")), "{dialog:?}");
        assert!(runner.launches().is_empty());
        screen.click(&mut page, at(&dialog, "Cancel"), egui::PointerButton::Primary);
        assert!(page.drv_ui.confirm_device.is_none());
        assert!(runner.launches().is_empty(), "cancel changes nothing");
    }

    #[test]
    fn confirming_an_ordinary_device_action_launches_the_job_with_the_restore_point_option() {
        let ((mut page, runner), mut screen) = (page_and_runner(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        let shown = device_menu(&mut screen, &mut page, "Alpha NIC");
        screen.click(&mut page, at_button(&shown, "Restart device"), egui::PointerButton::Primary);
        let dialog = screen.settle(&mut page);
        screen.click(&mut page, at_button(&dialog, "Restart device"), egui::PointerButton::Primary);
        wait_for_launches(&runner, 1);
        let launches = runner.launches();
        assert_eq!(launches.len(), 1, "{launches:?}");
        match &launches[0] {
            crate::platform::Launch::PowerShell { args, elevated, .. } => {
                assert!(*elevated);
                assert!(args.contains(&("Mode".to_string(), "restart-device".to_string())));
                assert!(args.contains(&("Arg".to_string(), r"ROOT\Alpha NIC\0000".to_string())));
                assert!(args.contains(&("Steps".to_string(), "restore-point,restart-device".to_string())), "the option is on by default");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_protected_device_needs_the_stronger_warning_ticked_before_the_confirm_button_works() {
        let ((mut page, runner), mut screen) = (page_and_runner(vec![device("Alpha GPU", "Display")], vec![]), Screen::new());
        let shown = device_menu(&mut screen, &mut page, "Alpha GPU");
        screen.click(&mut page, at_button(&shown, "Disable device"), egui::PointerButton::Primary);
        let dialog = screen.settle(&mut page);
        assert!(
            drawn(&dialog, "This is a display adapter: you may lose your screen or your boot disk until Windows restarts."),
            "{dialog:?}"
        );
        // The button is off until the box is ticked: a click launches nothing.
        screen.click(&mut page, at_button(&dialog, "Disable device"), egui::PointerButton::Primary);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(runner.launches().is_empty(), "the confirm button is inert before the explicit tick");
        assert!(page.drv_ui.confirm_device.is_some(), "the dialog stays open");

        let dialog = screen.settle(&mut page);
        screen.click(&mut page, at(&dialog, "I understand, continue anyway"), egui::PointerButton::Primary);
        assert!(page.drv_ui.confirm_device.as_ref().is_some_and(|c| c.understood));
        let dialog = screen.settle(&mut page);
        screen.click(&mut page, at_button(&dialog, "Disable device"), egui::PointerButton::Primary);
        wait_for_launches(&runner, 1);
        assert_eq!(runner.launches().len(), 1);
    }

    #[test]
    fn enabling_a_protected_device_needs_no_stronger_warning() {
        let off = DeviceDriver { problem: 22, ..device("Alpha GPU", "Display") };
        let mut page = page_with(vec![off.clone()], vec![]);
        page.drv_ui.confirm_device = Some(DeviceConfirm::new(drivers::DeviceAction::Enable, &off));
        assert!(page.drv_ui.confirm_device.as_ref().unwrap().confirmable());
        let restart = DeviceConfirm::new(drivers::DeviceAction::Restart, &off);
        assert!(!restart.confirmable(), "restarting it does");
    }

    #[test]
    fn the_device_actions_are_off_while_another_job_runs() {
        let ((mut page, runner), mut screen) = (page_and_runner(vec![device("Alpha NIC", "Net")], vec![]), Screen::new());
        page.drv.lock().busy = Some("Installing\u{2026}".into());
        let shown = device_menu(&mut screen, &mut page, "Alpha NIC");
        screen.click(&mut page, at_button(&shown, "Disable device"), egui::PointerButton::Primary);
        screen.settle(&mut page);
        assert!(page.drv_ui.confirm_device.is_none(), "a disabled entry opens nothing");
        assert!(runner.launches().is_empty());
    }

    #[test]
    fn each_tab_shows_its_own_filter_hint() {
        let (mut page, mut screen) = (page_with(vec![gpu("Alpha GPU")], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let shown = |t: &[(Pos2, String)], s: &str| t.iter().any(|(_, x)| x == s);
        let devices = screen.settle(&mut page);
        assert!(shown(&devices, "Find a driver or maker"), "Device drivers hint unchanged");
        assert!(!shown(&devices, "Driver name, service or vendor"));
        page.drv_ui.scope = Scope::Kernel;
        let kernel = screen.settle(&mut page);
        assert!(shown(&kernel, "Driver name, service or vendor"));
        assert!(!shown(&kernel, "Find a driver or maker"));
    }

    #[test]
    fn each_tab_shows_its_own_empty_panel_text() {
        let (mut page, mut screen) = (page_with(vec![gpu("Alpha GPU")], vec![module(r"C:\x\acme.sys")]), Screen::new());
        let shown = |t: &[(Pos2, String)], s: &str| t.iter().any(|(_, x)| x == s);
        let drivers = "Select a driver to see its details and what Windows Update offers for it.";
        let kernel_text = "Select a kernel module to see where it lives, how much memory it holds and whether it can be unloaded.";
        let devices = screen.settle(&mut page);
        assert!(shown(&devices, drivers) && !shown(&devices, kernel_text));
        page.drv_ui.scope = Scope::Kernel;
        let kernel = screen.settle(&mut page);
        assert!(shown(&kernel, kernel_text) && !shown(&kernel, drivers));
    }
}

/// "With Windows" and "check the maker" (#71).
#[cfg(test)]
mod age_tests {
    use super::*;

    fn driver(provider: &str, date: &str) -> DeviceDriver {
        DeviceDriver { provider: provider.into(), date: date.into(), ..Default::default() }
    }

    #[test]
    fn windows_own_drivers_with_the_placeholder_date_read_with_windows() {
        assert!(with_windows(&driver("Microsoft", "2006-06-21")));
        assert!(!with_windows(&driver("Microsoft", "2023-04-02")), "a real date is shown");
        assert!(!with_windows(&driver("Realtek", "2006-06-21")), "a third-party driver keeps its date");
        assert!(!with_windows(&driver("Microsoft", "")), "no date, nothing to replace");
    }

    #[test]
    fn third_party_drivers_older_than_the_threshold_get_check_the_maker() {
        let today = (2026, 11, 17);
        let old = driver("Realtek", "2023-11-17");
        assert_eq!(check_the_maker(&old, today, 3), Some(3), "exactly three years");
        assert_eq!(check_the_maker(&driver("Realtek", "2023-11-18"), today, 3), None, "a day short of three years");
        assert_eq!(check_the_maker(&old, today, 4), None, "a higher threshold");
        assert_eq!(check_the_maker(&old, today, 2), Some(3), "a lower threshold");
        assert_eq!(check_the_maker(&driver("Microsoft", "2015-01-01"), today, 3), None, "Windows' own drivers come with Windows Update");
        assert_eq!(check_the_maker(&driver("Realtek", "unknown"), today, 3), None);
    }
}

/// The Driver store tab (#26).
#[cfg(test)]
mod store_tests {
    use super::*;
    use crate::addons::drivers::store::parse_store;

    /// The recorded `pnputil /enum-drivers /devices /format xml`.
    const FIXTURE: &str = include_str!("fixtures/pnputil-enum-drivers.xml");

    struct NoReader;
    impl crate::addons::drivers::ListingReader for NoReader {
        fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
            Ok(Vec::new())
        }
        fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
            Ok(Vec::new())
        }
    }

    /// A page on the Driver store tab with the recorded packages, each
    /// measured at 1 MB.
    fn page() -> DriversPage {
        let mut packages = parse_store(FIXTURE).expect("the recording parses");
        packages.iter_mut().for_each(|p| p.size = Some(1_048_576));
        let (ctx, _) = AddonContext::recording();
        let hub = Hub::new(ctx.clone(), Arc::new(NoReader));
        hub.lock().store = packages.into();
        let mut page = DriversPage::new(hub, ctx);
        page.drv_ui.started = true;
        page.drv_ui.scope = Scope::Store;
        page
    }

    fn frame(page: &mut DriversPage) -> Vec<String> {
        fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => out.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                _ => {}
            }
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), ..Default::default() };
        let mut out = Vec::new();
        for _ in 0..2 {
            out.clear();
            let output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| page.ui(ui, &mut Vec::new()));
            });
            output.shapes.iter().for_each(|s| texts(&s.shape, &mut out));
        }
        out
    }

    #[test]
    fn the_tab_sits_after_kernel_modules_with_its_count() {
        let mut page = page();
        let t = frame(&mut page);
        let at = |what: &str| t.iter().position(|x| x == what).unwrap_or_else(|| panic!("{what} is shown: {t:?}"));
        // Each tab is its name and its count.
        assert!(at("Kernel modules  0") + 1 == at("Driver store  16"), "{t:?}");
    }

    #[test]
    fn the_four_figures_say_packages_third_party_old_and_disk_space() {
        let mut page = page();
        let t = frame(&mut page);
        let at = t.iter().position(|x| x == "packages").expect("the strip is shown");
        // Each figure draws its value, then its words.
        let pairs: Vec<(&str, &str)> = (0..5).map(|i| (t[at - 1 + i * 2].as_str(), t[at + i * 2].as_str())).collect();
        assert_eq!(pairs, [("16", "packages"), ("12", "third-party"), ("5", "old"), ("16.0 MB", "used"), ("5.0 MB", "can be freed")]);
        assert!(t.iter().any(|x| x.starts_with("Read without administrator rights")));
    }

    #[test]
    fn an_unmeasured_store_says_unknown_for_disk_space() {
        let f = StoreFigures { packages: 3, third_party: 3, old: 1, bytes: None, freeable: None };
        let words = store_figure_words(&f);
        assert_eq!(words.last().unwrap(), &("unknown".to_string(), "used"), "never 0 B");
        assert_eq!(words.len(), 4, "nothing is said about what can be freed");
    }

    #[test]
    fn rows_show_the_columns_and_the_flags() {
        let mut page = page();
        let t = frame(&mut page);
        for want in ["oem146.inf", "amdfendr.inf", "Advanced Micro Devices, Inc.", "System", "24.30.0.2", "2024-10-28", "In use", "Old", "1 device"] {
            assert!(t.iter().any(|x| x == want), "{want} is shown: {t:?}");
        }
        for header in ["Package", "Provider", "Class", "Version", "Date", "Signer", "Used by"] {
            assert!(t.iter().any(|x| x == header), "{header}");
        }
    }

    #[test]
    fn inbox_packages_are_flagged_and_the_filter_finds_them() {
        let mut page = page();
        page.drv_ui.search = "microsoft".into();
        let t = frame(&mut page);
        assert_eq!(t.iter().filter(|x| *x == "Inbox").count(), 4, "{t:?}");
        assert!(!t.iter().any(|x| x == "oem146.inf"), "the rest is filtered out");
        page.drv_ui.search = "gigabit".into();
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "oem50.inf"), "a device's name matches: {t:?}");
    }

    #[test]
    fn a_filter_that_matches_nothing_says_so_and_keeps_the_figures() {
        let mut page = page();
        page.drv_ui.search = "no such package".into();
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == STORE_NO_MATCH));
        assert!(t.iter().any(|x| x == "packages"), "the figures still describe the store");
    }

    #[test]
    fn the_rows_are_worked_out_once_per_read_and_filter_change() {
        let mut page = page();
        for _ in 0..5 {
            frame(&mut page);
        }
        assert_eq!(page.store_builds(), 1, "five frames, nothing changed");
        page.drv_ui.search = "nv".into();
        frame(&mut page);
        frame(&mut page);
        assert_eq!(page.store_builds(), 2, "the filter changed once");
        let mut again = parse_store(FIXTURE).unwrap();
        again.pop();
        page.drv.lock().store = again.into();
        frame(&mut page);
        frame(&mut page);
        assert_eq!(page.store_builds(), 3, "a new read");
        page.drv_ui.scope = Scope::Kernel;
        frame(&mut page);
        assert_eq!(page.store_builds(), 3, "the other tabs build nothing for the store");
    }

    #[test]
    fn a_selected_package_shows_its_details_and_copy_details() {
        let mut page = page();
        page.drv_ui.selected_store = Some("oem50.inf".into());
        let t = frame(&mut page);
        for want in ["Original INF", "axusbeth.inf", "Signer", "Microsoft Windows Hardware Compatibility Publisher", "Used by", "ASIX USB to Gigabit Ethernet Family Adapter", "Old, In use", "Copy details", "1.0 MB"] {
            assert!(t.iter().any(|x| x == want), "{want} is shown: {t:?}");
        }
    }

    #[test]
    fn copy_details_uses_fixed_labels_in_order() {
        let packages = parse_store(FIXTURE).unwrap();
        let list = StoreList::build(&packages.into(), "");
        let at = list.packages.iter().position(|p| p.published == "oem10.inf").unwrap();
        let text = store_details(&list.packages[at], list.flags[at]);
        let labels: Vec<&str> = text.lines().map(|l| l.split(':').next().unwrap()).collect();
        assert_eq!(labels, ["Package", "Original INF", "Provider", "Class", "Version", "Date", "Signer", "Size", "Used by", "Flags"]);
        assert!(text.contains("Used by: No device\n") && text.contains("Flags: Inbox\n") && text.contains("Size: \u{2014}\n"), "{text}");
    }

    #[test]
    fn an_inbox_package_says_it_is_not_offered_for_removal() {
        let mut page = page();
        page.drv_ui.selected_store = Some("oem10.inf".into());
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == STORE_INBOX_NOTE), "{t:?}");
        page.drv_ui.selected_store = Some("oem130.inf".into());
        assert!(!frame(&mut page).iter().any(|x| x == STORE_INBOX_NOTE));
    }

    #[test]
    fn reading_and_failing_have_their_own_notes() {
        let mut page = page();
        page.drv.lock().store = Vec::new().into();
        page.drv.lock().store_reading = true;
        assert!(frame(&mut page).iter().any(|x| x == "Reading the driver store\u{2026}"));
        {
            let mut st = page.drv.lock();
            st.store_reading = false;
            st.store_error = Some("pnputil is missing".into());
        }
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "The driver store could not be read") && t.iter().any(|x| x == "pnputil is missing"), "{t:?}");
        page.drv.lock().store_error = None;
        assert!(frame(&mut page).iter().any(|x| x == "The driver store holds no packages"));
    }

    #[test]
    fn the_filter_box_has_the_stores_hint_there_and_its_own_elsewhere() {
        let mut page = page();
        assert!(frame(&mut page).iter().any(|x| x == STORE_FILTER_HINT));
        page.drv_ui.scope = Scope::Hardware;
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "Find a driver or maker") && !t.iter().any(|x| x == STORE_FILTER_HINT));
    }

    #[test]
    fn the_other_tabs_keep_their_rows_and_show_disconnected_only_where_it_applies() {
        let mut page = page();
        page.drv.lock().devices = vec![DeviceDriver { instance_id: "a".into(), name: "Alpha GPU".into(), class: "Display".into(), provider: "Acme".into(), present: true, ..Default::default() }].into();
        page.drv_ui.scope = Scope::Hardware;
        let t = frame(&mut page);
        assert!(t.iter().any(|x| x == "Alpha GPU") && t.iter().any(|x| x == "Show disconnected"));
        page.drv_ui.scope = Scope::Store;
        let t = frame(&mut page);
        assert!(!t.iter().any(|x| x == "Show disconnected" || x == "Alpha GPU"));
    }

    #[test]
    fn the_store_tab_is_saved_by_name_and_the_old_names_still_read() {
        assert_eq!(serde_json::to_value(Scope::Store).unwrap(), "store");
        assert!(serde_json::from_value::<Scope>(serde_json::json!("store")).unwrap() == Scope::Store);
        assert!(serde_json::from_value::<Scope>(serde_json::json!("builtin")).unwrap() == Scope::Builtin);
        assert!(serde_json::from_value::<Scope>(serde_json::json!("kernel")).unwrap() == Scope::Kernel);
    }

    // ------------------------------------------------ removal and backup (#28, #29)

    use crate::platform::{Launch, RecordingRunner, When};

    fn page_with_runner() -> (DriversPage, Arc<RecordingRunner>) {
        let mut packages = parse_store(FIXTURE).expect("the recording parses");
        packages.iter_mut().for_each(|p| p.size = Some(1_048_576));
        let (ctx, runner) = AddonContext::recording();
        let hub = Hub::new(ctx.clone(), Arc::new(NoReader));
        hub.lock().store = packages.into();
        let mut page = DriversPage::new(hub, ctx);
        page.drv_ui.started = true;
        page.drv_ui.scope = Scope::Store;
        (page, runner)
    }

    /// One egui context kept over several frames, so clicks reach the page.
    struct Screen {
        ctx: egui::Context,
        time: f64,
    }

    impl Screen {
        fn new() -> Self {
            let ctx = egui::Context::default();
            crate::theme::install_fonts(&ctx);
            Screen { ctx, time: 0.0 }
        }

        fn step(&mut self, page: &mut DriversPage, events: Vec<egui::Event>) -> Vec<(Pos2, String)> {
            fn texts(shape: &egui::Shape, out: &mut Vec<(Pos2, String)>) {
                match shape {
                    egui::Shape::Text(t) => out.push((t.pos, t.galley.text().to_string())),
                    egui::Shape::Vec(v) => v.iter().for_each(|s| texts(s, out)),
                    _ => {}
                }
            }
            self.time += 0.1;
            let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1400.0, 900.0))), time: Some(self.time), events, ..Default::default() };
            let output = self.ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| page.ui(ui, &mut Vec::new()));
            });
            let mut out = Vec::new();
            output.shapes.iter().for_each(|s| texts(&s.shape, &mut out));
            out
        }

        fn settle(&mut self, page: &mut DriversPage) -> Vec<(Pos2, String)> {
            self.step(page, vec![]);
            self.step(page, vec![])
        }

        fn click(&mut self, page: &mut DriversPage, at: Pos2) -> Vec<(Pos2, String)> {
            let m = egui::Modifiers::NONE;
            let button = egui::PointerButton::Primary;
            self.step(page, vec![egui::Event::PointerMoved(at)]);
            self.step(page, vec![egui::Event::PointerButton { pos: at, button, pressed: true, modifiers: m }]);
            self.step(page, vec![egui::Event::PointerButton { pos: at, button, pressed: false, modifiers: m }]);
            self.settle(page)
        }
    }

    /// Where the last text equal to `what` is drawn (a dialog is drawn after
    /// the page under it).
    fn last(texts: &[(Pos2, String)], what: &str) -> Pos2 {
        texts.iter().rev().find(|(_, t)| t == what).unwrap_or_else(|| panic!("{what:?} is not drawn: {texts:?}")).0 + vec2(8.0, 8.0)
    }

    fn drawn(texts: &[(Pos2, String)], what: &str) -> bool {
        texts.iter().any(|(_, t)| t == what)
    }

    /// The tick box of the row whose package name is drawn at `name`.
    fn tick_of(name: Pos2) -> Pos2 {
        Pos2::new(name.x - 8.0 - STORE_TICK_W + 14.0, name.y + 6.0)
    }

    fn wait_for_the_job(page: &DriversPage) {
        for _ in 0..1000 {
            if page.drv.lock().busy.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the job did not finish");
    }

    fn admin(label: &str) -> String {
        format!("      {label}")
    }

    #[test]
    fn a_tick_box_selects_a_package_and_an_inbox_row_has_none() {
        let (mut page, _) = page_with_runner();
        let mut screen = Screen::new();
        let t = screen.settle(&mut page);
        assert!(drawn(&t, &admin("Remove\u{2026}")) && !drawn(&t, &admin("Remove 1\u{2026}")), "nothing ticked yet");
        let name = last(&t, "oem50.inf") - vec2(8.0, 8.0);
        let t = screen.click(&mut page, tick_of(name));
        assert_eq!(page.drv_ui.ticked.names(), ["oem50.inf"]);
        assert!(drawn(&t, &admin("Remove 1\u{2026}")), "the count follows the ticks");
        assert_eq!(page.drv_ui.selected_store, None, "ticking is not selecting the row");
        let t = screen.click(&mut page, tick_of(name));
        assert!(page.drv_ui.ticked.is_empty() && drawn(&t, &admin("Remove\u{2026}")), "a second click unticks");

        let inbox = last(&t, "oem10.inf") - vec2(8.0, 8.0);
        screen.click(&mut page, tick_of(inbox));
        assert!(page.drv_ui.ticked.is_empty(), "the inbox package has no tick box");
    }

    #[test]
    fn the_buttons_over_the_list_need_a_free_job_and_ticks_where_they_work_on_ticks() {
        let (mut page, _) = page_with_runner();
        page.drv.lock().busy = Some("Installing\u{2026}".into());
        let mut screen = Screen::new();
        let t = screen.settle(&mut page);
        for label in ["Remove old packages", "Back up selected", "Back up drivers\u{2026}"] {
            assert!(drawn(&t, &admin(label)), "{label} is shown even when it cannot be used");
        }
        let at = last(&t, &admin("Remove old packages"));
        screen.click(&mut page, at);
        assert!(page.drv_ui.confirm_remove.is_none(), "another job holds the busy text");
        page.drv.lock().busy = None;
        screen.click(&mut page, at);
        let ask = page.drv_ui.confirm_remove.clone().expect("the confirmation opens");
        assert_eq!((ask.kind, ask.names), (RemoveKind::Old, store::old_removable(&page.drv.lock().store)));
    }

    #[test]
    fn remove_old_confirms_with_the_list_then_runs_one_elevated_launch_without_force() {
        let (mut page, runner) = page_with_runner();
        let old = store::old_removable(&page.drv.lock().store);
        assert_eq!(old.len(), 5);
        let mut screen = Screen::new();
        screen.settle(&mut page);
        page.drv_ui.confirm_remove = Some(RemoveAsk { kind: RemoveKind::Old, names: old.clone(), understood: false });
        let t = screen.settle(&mut page);
        assert!(drawn(&t, "Remove 5 old packages?") && drawn(&t, REMOVE_OLD_NOTE), "{t:?}");
        for name in &old {
            assert!(t.iter().any(|(_, x)| x.starts_with(name.as_str())), "{name} is listed");
        }
        assert!(drawn(&t, "Create a restore point first"));
        let items: Vec<String> = old.iter().map(|n| format!(r#"{{"name":"{n}","code":0,"output":""}}"#)).collect();
        runner.prepare(When::Nth(0), 0, &format!(r#"{{"ok":true,"items":[{}],"steps":{{"restore-point":{{"state":"created","drive":"C:"}}}}}}"#, items.join(",")));
        let at = last(&t, &admin(REMOVE_OLD_LABEL));
        screen.click(&mut page, at);
        wait_for_the_job(&page);
        let launches = runner.launches();
        let Launch::PowerShell { args, elevated, .. } = &launches[0] else { panic!("a PowerShell launch") };
        assert!(*elevated);
        let arg = |k: &str| args.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!((arg("Mode"), arg("Names"), arg("Force"), arg("Steps")), (Some("delete"), Some(old.join(",").as_str()), Some("0"), Some("restore-point,delete")));
        assert!(page.drv_ui.confirm_remove.is_none());
        assert_eq!(page.drv.lock().message.as_ref().map(|m| m.0.as_str()), Some("Restore point created. Removed 5 packages, freed 5.0 MB."));
    }

    #[test]
    fn cancel_in_the_removal_dialog_runs_nothing() {
        let (mut page, runner) = page_with_runner();
        let mut screen = Screen::new();
        page.drv_ui.confirm_remove = Some(RemoveAsk { kind: RemoveKind::Selected, names: vec!["oem50.inf".into(), store::old_removable(&page.drv.lock().store).remove(0)], understood: false });
        let t = screen.settle(&mut page);
        assert!(drawn(&t, "Remove 2 driver packages?") && drawn(&t, REMOVE_SELECTED_NOTE), "{t:?}");
        assert!(drawn(&t, "In use: will be refused"), "a ticked package in use is marked in the list");
        let at = last(&t, "Cancel");
        screen.click(&mut page, at);
        assert!(page.drv_ui.confirm_remove.is_none());
        assert!(runner.launches().is_empty());
    }

    #[test]
    fn a_package_in_use_says_a_plain_removal_is_refused_and_offers_the_forced_one() {
        let (mut page, _) = page_with_runner();
        let mut screen = Screen::new();
        page.drv_ui.selected_store = Some("oem50.inf".into());
        let t = screen.settle(&mut page);
        assert!(drawn(&t, store::NOT_REMOVED_IN_USE) && drawn(&t, FORCE_LABEL) && drawn(&t, &admin("Remove\u{2026}")), "{t:?}");
        let free = store::old_removable(&page.drv.lock().store).remove(0);
        page.drv_ui.selected_store = Some(free);
        let t = screen.settle(&mut page);
        assert!(!drawn(&t, store::NOT_REMOVED_IN_USE) && !drawn(&t, FORCE_LABEL), "a package no device uses has no forced removal");
        page.drv_ui.selected_store = Some("oem10.inf".into());
        let t = screen.settle(&mut page);
        assert!(drawn(&t, STORE_INBOX_NOTE) && !drawn(&t, FORCE_LABEL), "an inbox package is not offered");
        assert_eq!(t.iter().filter(|(_, x)| *x == admin("Remove\u{2026}")).count(), 1, "only the list's button is left: none in the panel");
    }

    #[test]
    fn the_forced_removal_asks_twice_and_is_the_only_one_that_sends_force() {
        let (mut page, runner) = page_with_runner();
        let mut screen = Screen::new();
        page.drv_ui.selected_store = Some("oem50.inf".into());
        let t = screen.settle(&mut page);
        let at = last(&t, FORCE_LABEL);
        let t = screen.click(&mut page, at);
        let ask = page.drv_ui.confirm_remove.clone().expect("the stronger confirmation opens");
        assert_eq!((ask.kind, ask.names.as_slice()), (RemoveKind::Forced, ["oem50.inf".to_string()].as_slice()));
        assert!(drawn(&t, "Remove oem50.inf even if in use?") && drawn(&t, FORCE_UNDERSTOOD), "{t:?}");
        assert!(t.iter().any(|(_, x)| x.starts_with("Runs pnputil /delete-driver oem50.inf /uninstall /force")));
        assert!(t.iter().any(|(_, x)| x.contains("stops working and may not come back")), "the stronger warning names the consequence");
        // The button does nothing until the user says they understand.
        let button = last(&t, &admin("Remove even if in use"));
        screen.click(&mut page, button);
        assert!(runner.launches().is_empty() && page.drv_ui.confirm_remove.is_some());
        let check = last(&t, FORCE_UNDERSTOOD);
        screen.click(&mut page, check);
        assert!(page.drv_ui.confirm_remove.as_ref().is_some_and(|a| a.understood));
        runner.prepare(When::Nth(0), 0, r#"{"ok":true,"items":[{"name":"oem50.inf","code":0,"output":""}],"steps":{"restore-point":{"state":"created","drive":"C:"}}}"#);
        let t = screen.settle(&mut page);
        let button = last(&t, &admin("Remove even if in use"));
        screen.click(&mut page, button);
        wait_for_the_job(&page);
        let Launch::PowerShell { args, .. } = &runner.launches()[0] else { panic!("a PowerShell launch") };
        assert!(args.contains(&("Force".into(), "1".into())) && args.contains(&("Names".into(), "oem50.inf".into())), "{args:?}");
    }

    #[test]
    fn a_plain_removal_of_a_package_in_use_is_refused_with_the_stated_words_and_runs_nothing() {
        let (mut page, runner) = page_with_runner();
        let mut screen = Screen::new();
        page.drv_ui.confirm_remove = Some(RemoveAsk { kind: RemoveKind::Selected, names: vec!["oem50.inf".into()], understood: false });
        let t = screen.settle(&mut page);
        let at = last(&t, &admin("Remove"));
        screen.click(&mut page, at);
        assert!(runner.launches().is_empty(), "no UAC prompt for a refusal");
        assert_eq!(page.drv.lock().message.as_ref().map(|m| m.0.as_str()), Some("Not removed: oem50.inf (a device uses it)"));
    }

    #[test]
    fn the_header_button_and_back_up_selected_open_the_backup_dialog_with_the_note() {
        let (mut page, runner) = page_with_runner();
        let mut screen = Screen::new();
        let t = screen.settle(&mut page);
        let at = last(&t, &admin("Back up drivers\u{2026}"));
        let t = screen.click(&mut page, at);
        assert_eq!(page.drv_ui.confirm_backup.as_ref().map(|a| a.names.clone()), Some(None), "every third-party package");
        assert!(drawn(&t, "Back up every third-party driver") && drawn(&t, store::BACKUP_RESTORE_NOTE), "{t:?}");
        assert!(drawn(&t, &format!("A dated subfolder is created (Drivers {}).", store::backup_folder("", today()).trim_start_matches("\\Drivers "))));
        assert!(drawn(&t, &admin("Back up 12 packages")), "twelve third-party packages: {t:?}");
        // No folder yet: the button does nothing.
        let go = last(&t, &admin("Back up 12 packages"));
        screen.click(&mut page, go);
        assert!(runner.launches().is_empty() && page.drv_ui.confirm_backup.is_some());
        let cancel = last(&t, "Cancel");
        screen.click(&mut page, cancel);
        assert!(page.drv_ui.confirm_backup.is_none());

        // Back up selected names the ticked packages.
        let free = store::old_removable(&page.drv.lock().store).remove(0);
        let package = page.drv.lock().store.iter().find(|p| p.published == free).unwrap().clone();
        page.drv_ui.ticked.toggle(&package);
        let t = screen.settle(&mut page);
        let at = last(&t, &admin(BACKUP_SELECTED_LABEL));
        let t = screen.click(&mut page, at);
        assert_eq!(page.drv_ui.confirm_backup.as_ref().and_then(|a| a.names.clone()), Some(vec![free.clone()]));
        assert!(drawn(&t, "Back up 1 selected package") && drawn(&t, &admin("Back up 1 package")), "{t:?}");
    }

    #[test]
    fn confirming_the_backup_runs_one_elevated_export_into_the_dated_subfolder_without_a_restore_point() {
        let (mut page, runner) = page_with_runner();
        let mut screen = Screen::new();
        page.drv_ui.confirm_backup = Some(BackupAsk { names: None, folder: r"D:\Backups".into() });
        let t = screen.settle(&mut page);
        let all = store::third_party(&page.drv.lock().store);
        let items: Vec<String> = all.iter().map(|n| format!(r#"{{"name":"{n}","code":0,"output":""}}"#)).collect();
        runner.prepare(When::Nth(0), 0, &format!(r#"{{"ok":true,"items":[{}]}}"#, items.join(",")));
        let go = last(&t, &admin("Back up 12 packages"));
        screen.click(&mut page, go);
        wait_for_the_job(&page);
        let folder = store::backup_folder(r"D:\Backups", today());
        let Launch::PowerShell { args, elevated, script } = &runner.launches()[0] else { panic!("a PowerShell launch") };
        assert!(*elevated && script.as_str() == drivers::STORE_SCRIPT, "the store script, no restore point step");
        assert_eq!(
            args,
            &[("Mode".to_string(), "export".to_string()), ("Names".into(), all.join(",")), ("Folder".into(), folder.clone()), ("Force".into(), "0".into())]
        );
        assert_eq!(page.drv.lock().message.as_ref().map(|m| m.0.clone()), Some(format!("Backed up 12 driver packages to {folder}.")));
        assert_eq!(page.drv_ui.backup_folder, r"D:\Backups", "the folder is remembered for the next backup");
        assert!(page.drv_ui.confirm_backup.is_none());
    }
}
