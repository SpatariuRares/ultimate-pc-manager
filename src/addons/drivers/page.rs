//! The Drivers page, design A ("update center", see the Drivers page
//! redesign canvas): a status band that answers "are my drivers up to
//! date?" with one main action, then the drivers people actually update
//! (third-party hardware drivers), grouped by kind with duplicates folded,
//! and a side panel for the selected driver. Windows built-in drivers and
//! kernel modules are one click away.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui};

use super::engine::{self as drivers, DeviceDriver, DriverUpdate, Hub, KernelDriver, KernelFigures, KernelScope};
use super::store::{Flags, StoreFigures, StoreList, StorePackage};
use crate::addons::plural;
use crate::platform::{csv_field, Action, AddonContext};
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
    /// Kernel driver services.
    Kernel,
    /// The driver packages Windows keeps.
    Store,
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
    pub confirm_unload: Option<String>,
    /// An offered driver that is not newer, waiting for "Install anyway".
    pub confirm_older: Option<DriverUpdate>,
    /// Bring the selected row into view on the next frame.
    pub scroll_to_selected: bool,
    pub started: bool,
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
    inputs: Inputs,
    /// Third-party drivers after folding, for the scope button.
    hardware_count: usize,
    /// Windows built-in drivers after folding, for the scope button.
    builtin_count: usize,
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
    fn build(devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>, inputs: Inputs) -> Derived {
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
        let q = inputs.search.trim().to_lowercase();

        let mut items = Vec::new();
        let mut matching = 0;
        if !matches!(inputs.scope, Scope::Kernel | Scope::Store) {
            let matches: Vec<usize> = scoped(inputs.scope == Scope::Hardware)
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
                    let sub = if older {
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
                .filter(|(_, d)| inputs.kernel_filter.matches(d))
                .filter(|(_, d)| kernel_text_matches(d, &q))
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        let kernel_counts = KernelScope::ALL.iter().map(|&s| (s, s.count(kernel))).collect();
        Derived {
            devices: devices.clone(),
            kernel: kernel.clone(),
            updates: updates.clone(),
            hardware_count,
            builtin_count,
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
    fn is_for(&self, devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>, inputs: &Inputs) -> bool {
        let same_updates = match (&self.updates, updates) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        };
        Arc::ptr_eq(&self.devices, devices) && Arc::ptr_eq(&self.kernel, kernel) && same_updates && self.inputs == *inputs
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
    /// Third-party drivers this many years old get "check the maker".
    pub old_years: i64,
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
            old_years: DEFAULT_OLD_YEARS,
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
    fn derived(&mut self, devices: &Arc<[DeviceDriver]>, kernel: &Arc<[KernelDriver]>, updates: &Option<Arc<[DriverUpdate]>>) -> Arc<Derived> {
        let inputs = Inputs {
            search: self.drv_ui.search.clone(),
            show_disconnected: self.drv_ui.show_disconnected,
            scope: self.drv_ui.scope,
            kernel_filter: self.drv_ui.kernel_filter,
            old_years: self.old_years,
            today: today(),
        };
        if let Some(d) = &self.derived {
            if d.is_for(devices, kernel, updates, &inputs) {
                return d.clone();
            }
        }
        let d = Arc::new(Derived::build(devices, kernel, updates, inputs));
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

    /// Draws the page below the app's header (title, Refresh).
    pub fn ui(&mut self, ui: &mut Ui, actions: &mut Vec<Action>) {
        let th = theme::of_ui(ui);
        self.opened();
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
        drop(st);
        let admin = self.context.is_admin();
        let idle = busy.is_none();
        let view = self.derived(&devices, &kernel, &updates);

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
                            self.drv.install_inf(p);
                        }
                        ui.close();
                    }
                    if parts::admin_button(ui, "Scan for hardware changes", false, idle).clicked() {
                        self.drv.scan();
                        ui.close();
                    }
                    if ui.add(egui::Button::new("      Export the list as CSV\u{2026}").min_size(vec2(0.0, 32.0))).clicked() {
                        self.export_drivers_csv(&devices, &kernel, updates.as_deref());
                        ui.close();
                    }
                });
            });
        });
        ui.add_space(8.0);

        self.drivers_band(ui, &devices, updates.as_deref(), busy.as_deref(), message.as_ref(), reboot, &checked_at, idle);
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
            let scopes = [
                (Scope::Hardware, "Hardware drivers", view.hardware_count),
                (Scope::Builtin, "Windows built-in", view.builtin_count),
                (Scope::Kernel, "Kernel modules", kernel.len()),
                (Scope::Store, "Driver store", store.len()),
            ];
            if let Some(scope) = parts::scope_group(ui, self.drv_ui.scope, &scopes).clicked {
                self.drv_ui.scope = scope;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !matches!(self.drv_ui.scope, Scope::Kernel | Scope::Store) {
                    ui.checkbox(&mut self.drv_ui.show_disconnected, "Show disconnected");
                    ui.add_space(8.0);
                }
                let hint = if self.drv_ui.scope == Scope::Store { STORE_FILTER_HINT } else { "Find a driver or maker" };
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
        ui.add_space(10.0);

        // ------------------------------------------------ list + side panel
        // A click or a keystroke above may have changed the filter.
        let view = self.derived(&devices, &kernel, &updates);
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
                self.kernel_side(&mut side_ui, actions, &kernel, admin, idle);
            }
            Scope::Store => {
                let list = self.store_view(&store);
                self.store_list(&mut list_ui, actions, &list, store_reading, store_error.as_deref());
                self.store_side(&mut side_ui, actions, &list);
            }
            scope => {
                let searching = !self.drv_ui.search.trim().is_empty();
                let note = list_note(listing, devices_error.as_deref(), devices.len(), view.matching, searching);
                if let Some(note) = note {
                    show_note(&mut list_ui, &note, scope);
                } else {
                    self.device_list(&mut list_ui, actions, &view);
                }
                self.device_side(&mut side_ui, actions, &devices, updates.as_deref(), idle);
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
    ) {
        let th = theme::of_ui(ui);
        let newer: Vec<&DriverUpdate> = updates.map(|u| u.iter().filter(|x| x.worth_installing()).collect()).unwrap_or_default();
        let older: Vec<&DriverUpdate> = updates.map(|u| u.iter().filter(|x| !x.worth_installing()).collect()).unwrap_or_default();
        let device = |u: &DriverUpdate| u.device.as_ref().and_then(|id| devices.iter().find(|d| &d.instance_id == id));
        let mut select: Option<String> = None;

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
                            self.drv.install(newer.iter().map(|u| u.id.clone()).collect());
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
    fn device_list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, view: &Derived) {
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
                    Sub::Problem => (d.status(), th.red),
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
                let details = device_details(d, upd);
                let inf = d.inf_path();
                let name = d.name.clone();
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
                });
            }
        });
        if let Some(id) = clicked {
            self.drv_ui.selected_device = Some(id);
        }
    }

    fn device_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, devices: &[DeviceDriver], updates: Option<&[DriverUpdate]>, idle: bool) {
        let chosen = self.drv_ui.selected_device.clone().and_then(|id| devices.iter().find(|d| d.instance_id == id));
        parts::SidePanel::new("drv-side").show(ui, chosen, |ui, d| {
            let th = theme::of_ui(ui);
            let upd = updates.and_then(|u| u.iter().find(|x| x.device.as_deref() == Some(d.instance_id.as_str())));
            let copies = devices.iter().filter(|o| o.name == d.name && o.version == d.version && o.provider == d.provider && (o.present || !d.present)).count();
            ui.label(RichText::new(&d.name).font(display(15.0)).color(th.text));
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
                row("Released", if with_windows(d) { "With Windows".into() } else { long_date(&d.date) });
                row("Package", or_dash(&d.inf));
                if !d.service.is_empty() {
                    row("Service", d.service.clone());
                }
                if copies > 1 {
                    row("Entries", format!("{copies} identical entries"));
                }
            });
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
                        self.drv.install(vec![u.id.clone()]);
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
            ui.add_space(10.0);
            ui.with_layout(Layout::top_down_justified(Align::Min), |ui| {
                if parts::outline_button(ui, "Open the driver's folder", true).clicked() {
                    actions.push(Action::OpenLocation(d.inf_path()));
                }
                if parts::outline_button(ui, "Open in Device Manager", true).clicked() {
                    self.open_device_manager();
                }
                if parts::outline_button(ui, "Copy details", true).clicked() {
                    actions.push(Action::Copy(device_details(d, upd)));
                }
            });
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
        let fixed = [80.0, 85.0, 85.0, 130.0];
        let gap = 12.0;
        let flex = (w - fixed.iter().sum::<f32>() - gap * 6.0 - 20.0).max(240.0);
        let cols = [flex * 0.45, flex * 0.2, fixed[0], fixed[1], fixed[2], fixed[3], flex * 0.35];
        let xs: Vec<f32> = cols.iter().scan(10.0, |x, c| {
            let start = *x;
            *x += c + gap;
            Some(start)
        }).collect();
        let (hr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
        for (i, h) in ["Driver", "Name", "State", "Start type", "Type", "Version", "Vendor"].iter().enumerate() {
            ui.painter().text(Pos2::new(hr.left() + xs[i], hr.center().y), Align2::LEFT_CENTER, *h, text(12.0), th.label);
        }
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(hr.left(), hr.bottom() - 1.0), vec2(w, 1.0)), CornerRadius::ZERO, th.line);
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("drv-kernel-list").auto_shrink([false, false]).show_rows(ui, 28.0, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let d = &kernel[i];
                let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
                let p = ui.painter();
                let selected = self.drv_ui.selected_kernel.as_deref() == Some(d.service.as_str());
                parts::row_fill(p, r, selected, resp.hovered());
                let vals = [
                    d.display.as_str(),
                    d.service.as_str(),
                    d.state_label(),
                    d.start_type,
                    d.kind.label(),
                    if d.version.is_empty() { "\u{2014}" } else { d.version.as_str() },
                    if d.vendor.is_empty() { "\u{2014}" } else { d.vendor.as_str() },
                ];
                for (i, v) in vals.iter().enumerate() {
                    let col = match (i, selected) {
                        (0, true) => th.on_selected,
                        (0, false) => th.text,
                        _ => th.soft,
                    };
                    p.text(Pos2::new(r.left() + xs[i], r.center().y), Align2::LEFT_CENTER, fit(p, v, &text(13.0), cols[i]), text(13.0), col);
                }
                if resp.clicked() {
                    clicked = Some(d.service.clone());
                }
                let (path, service, details) = (d.path.clone(), d.service.clone(), kernel_details(d, admin));
                resp.context_menu(|ui| {
                    if ui.button("Open file location").clicked() {
                        actions.push(Action::OpenLocation(path.clone()));
                        ui.close();
                    }
                    if ui.button("Copy service name").clicked() {
                        actions.push(Action::Copy(service.clone()));
                        ui.close();
                    }
                    if ui.button("Copy details").clicked() {
                        actions.push(Action::Copy(details.clone()));
                        ui.close();
                    }
                });
            }
        });
        if let Some(s) = clicked {
            self.drv_ui.selected_kernel = Some(s);
        }
    }

    fn kernel_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, kernel: &[KernelDriver], admin: bool, idle: bool) {
        let chosen = self.drv_ui.selected_kernel.clone().and_then(|s| kernel.iter().find(|d| d.service == s));
        parts::SidePanel::new("drv-side").show(ui, chosen, |ui, d| {
            let th = theme::of_ui(ui);
            let (offered, _) = d.unload_note(admin);
            ui.label(RichText::new(&d.display).font(display(15.0)).color(th.text));
            ui.add_space(6.0);
            egui::Grid::new("drv-kernel-side").striped(false).num_columns(2).spacing([14.0, 5.0]).show(ui, |ui| {
                for (k, v) in kernel_detail_rows(d, admin) {
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
                    actions.push(Action::OpenLocation(d.path.clone()));
                }
                if parts::outline_button(ui, "Open in Device Manager", true).clicked() {
                    self.open_device_manager();
                }
                if parts::outline_button(ui, "Copy service name", true).clicked() {
                    actions.push(Action::Copy(d.service.clone()));
                }
                if parts::outline_button(ui, "Copy details", true).clicked() {
                    actions.push(Action::Copy(kernel_details(d, admin)));
                }
                if offered && parts::admin_button(ui, "Unload\u{2026}", false, idle).clicked() {
                    self.drv_ui.confirm_unload = Some(d.service.clone());
                }
            });
        });
    }

    /// The Driver store tab: the figures, then one row per package.
    fn store_list(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, list: &StoreList, reading: bool, error: Option<&str>) {
        let th = theme::of_ui(ui);
        let packages = &list.packages;
        // Reading, failed or nothing reported: no figures to give.
        if let Some(note) = list_note(reading, error, packages.len(), packages.len(), false) {
            show_note(ui, &note, Scope::Store);
            return;
        }
        store_strip(ui, &list.figures);
        ui.add_space(10.0);
        let rows = &list.rows;
        if let Some(note) = list_note(reading, error, packages.len(), rows.len(), !self.drv_ui.search.trim().is_empty()) {
            show_note(ui, &note, Scope::Store);
            return;
        }
        let w = ui.available_width();
        let fixed = [110.0, 100.0, 90.0];
        let gap = 12.0;
        let flex = (w - fixed.iter().sum::<f32>() - gap * 6.0 - 20.0).max(300.0);
        // Package, provider, class, version, date, signer, flags.
        let cols = [fixed[0] + 40.0, flex * 0.3, fixed[1], fixed[1] + 20.0, fixed[2], flex * 0.3, flex * 0.4];
        let xs: Vec<f32> = cols
            .iter()
            .scan(10.0, |x, c| {
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
        egui::ScrollArea::vertical().id_salt("drv-store-list").auto_shrink([false, false]).show_rows(ui, STORE_ROW, rows.len(), |ui, range| {
            for &i in &rows[range] {
                let p = &packages[i];
                let flags = list.flags[i];
                let (r, resp) = ui.allocate_exact_size(vec2(w, STORE_ROW), Sense::click());
                let painter = ui.painter();
                let selected = self.drv_ui.selected_store.as_deref() == Some(p.published.as_str());
                parts::row_fill(painter, r, selected, resp.hovered());
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
    }

    /// The selected package's details and "Copy details".
    fn store_side(&mut self, ui: &mut Ui, actions: &mut Vec<Action>, list: &StoreList) {
        let at = self.drv_ui.selected_store.as_deref().and_then(|name| list.packages.iter().position(|p| p.published == name));
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
            });
        });
    }

    fn drivers_dialogs(&mut self, ui: &mut Ui) {
        let th = theme::of_ui(ui);
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
                self.drv.install(vec![u.id.clone()]);
            }
            if decided.is_some() {
                self.drv_ui.confirm_older = None;
            }
        }
    }

    fn export_drivers_csv(&mut self, devices: &[DeviceDriver], kernel: &[KernelDriver], updates: Option<&[DriverUpdate]>) {
        let Some(path) = self.context.pick_save_as(&crate::platform::CSV_FILE, "drivers.csv") else { return };
        let mut s = String::from("Kind,Name,Class/Type,Provider/Vendor,Version,Date,Status,INF/Service,Windows Update,Instance ID/Path\n");
        for d in devices {
            let u = updates.and_then(|u| u.iter().find(|x| x.device.as_deref() == Some(d.instance_id.as_str())));
            let up = match (updates, u) {
                (None, _) => "not checked".to_string(),
                (Some(_), Some(u)) if u.worth_installing() => format!("newer: {}", u.version),
                (Some(_), Some(u)) => format!("up to date (offers older {})", u.version),
                (Some(_), None) => "up to date".to_string(),
            };
            let f = ["Device", &d.name, &d.class, &d.provider, &d.version, &d.date, &d.status(), &d.inf, &up, &d.instance_id];
            s.push_str(&f.iter().map(|x| csv_field(x)).collect::<Vec<_>>().join(","));
            s.push('\n');
        }
        for d in kernel {
            let f = ["Kernel driver", &d.display, d.kind.label(), &d.vendor, &d.version, "", if d.running { "Running" } else { "Stopped" }, &d.service, "", &d.path];
            s.push_str(&f.iter().map(|x| csv_field(x)).collect::<Vec<_>>().join(","));
            s.push('\n');
        }
        match std::fs::write(&path, format!("\u{feff}{s}")) {
            Ok(()) => self.say(format!("Saved {path}")),
            Err(e) => self.say(format!("Could not save: {e}")),
        }
    }
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


fn device_details(d: &DeviceDriver, u: Option<&DriverUpdate>) -> String {
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
fn kernel_detail_rows(d: &KernelDriver, admin: bool) -> Vec<(&'static str, String)> {
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
        ("Load index", load_index),
    ];
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

fn kernel_details(d: &KernelDriver, admin: bool) -> String {
    kernel_detail_rows(d, admin).iter().map(|(k, v)| format!("{k}: {v}\n")).collect()
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

    fn page_with(devices: Vec<DeviceDriver>, kernel: Vec<KernelDriver>) -> DriversPage {
        let (ctx, _) = AddonContext::recording();
        let hub = Hub::new(ctx.clone(), Arc::new(NoReader));
        {
            let mut st = hub.lock();
            st.devices = devices.into();
            st.kernel = kernel.into();
        }
        let mut page = DriversPage::new(hub, ctx);
        page.drv_ui.started = true;
        page
    }

    fn gpu(name: &str) -> DeviceDriver {
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
    fn kinds_group_audio_together() {
        assert_eq!(kind_of("MEDIA"), "Sound");
        assert_eq!(kind_of("AudioProcessingObject"), "Sound");
        assert_eq!(kind_of("HIDClass"), "Mice, keyboards and controllers");
    }
}

#[cfg(test)]
mod kernel_detail_tests {
    use super::*;

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
Load index: 7 (Boot Bus Extender)\n\
Load order group: Boot Bus Extender\n\
Kernel memory: 152.0 KB\n\
Location: C:\\Windows\\System32\\Drivers\\acpiex.sys\n\
Unload: Not offered: Windows loaded it at boot, and the kernel refuses to stop a driver it started itself.\n";
        assert_eq!(kernel_details(&sample(), true), expected);
    }

    #[test]
    fn missing_values_show_a_dash() {
        let d = KernelDriver { running: false, version: String::new(), vendor: String::new(), group: String::new(), image_size: None, load_index: None, ..sample() };
        let text = kernel_details(&d, false);
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
        let text = kernel_details(&d, true);
        assert!(text.contains("Load index: 7 (Boot Bus Extender)\nLoad order group: Boot Bus Extender\nDepends on: Tcpip, NDIS (group)\nKernel memory: "), "{text}");
        let bare = kernel_details(&KernelDriver { group: String::new(), ..sample() }, true);
        assert!(!bare.contains("Load order group") && !bare.contains("Depends on"), "{bare}");
        assert!(kernel_details(&sample(), true).contains("Load order group: Boot Bus Extender\n"));
        assert!(!kernel_details(&sample(), true).contains("Depends on"));
    }

    #[test]
    fn pending_states_show_in_the_state_row() {
        let d = KernelDriver { running: false, state_code: Some(6), ..sample() };
        assert!(kernel_details(&d, true).contains("State: Pause pending\n"));
    }

    #[test]
    fn load_index_without_group_has_no_parentheses() {
        let d = KernelDriver { group: String::new(), ..sample() };
        assert!(kernel_details(&d, true).contains("Load index: 7\n"));
    }

    #[test]
    fn empty_filter_uses_the_kernel_no_match_text() {
        assert_eq!(KERNEL_NO_MATCH, "No driver name, display name or vendor matches the filter.");
    }
}

#[cfg(test)]
mod list_note_tests {
    use super::*;

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
}
