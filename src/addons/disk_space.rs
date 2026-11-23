//! The Disk Space add-on: used and free space of every drive, and a scan
//! that measures where the space of a drive or folder goes (largest
//! folders and files). The scan runs in its own low-priority thread only
//! when the user presses Start, and keeps running while the page is
//! hidden; turning the add-on off stops it and drops what it measured.
//!
//! Layout: this file owns the scan's state, its thread, the settings
//! and the add-on's hooks; the submodules below hold the rest (pure logic
//! apart from `page`, `dialogs` and `canvas`, which draw).

/// The item actions (Show in File Explorer, Open the folder, Copy path).
pub mod actions;
/// The Balls layout: packed circles.
pub mod balls;
/// The Block layout: a squarified treemap.
pub mod block;
/// The canvas drawing a level as Balls or Block, with hover and clicks.
pub mod canvas;
/// The Stop / Clear History confirmations and the header menu.
mod dialogs;
/// The saved scans: snapshot format and the history folder.
pub mod history;
/// The canvas' keyboard moves.
pub mod keys;
/// The file type categories (Media, Documents, ...).
pub mod kind;
/// Where the user is in the measured tree; the pieces of a level.
pub mod nav;
/// The page as drawn: toolbar, strip, side panel, main column.
mod page;
/// The scan's phases (running, paused, stopped...) and their words.
pub mod phase;
/// The walk of a folder that produces the tree's events.
pub mod scan;
/// What a scan measures (a drive, a folder, All disks) and `--scan`.
pub mod target;
/// The words and numbers shown.
pub mod text;
/// The space tree fed by the scan's events.
pub mod tree;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{RichText, Ui};

use crate::addons::{is_default, read_settings, write_settings, Addon, Consumption, Requirements};
use crate::platform::{lock_state, Action, AddonContext, Generation, PageContext};
use crate::sampler::{DiskRow, Snapshot};
use kind::Category;
use phase::{Phase, Step};
use target::{Plan, ScanRequest, Target};
use tree::{Event, SpaceTree};

/// How many entries the largest folders / files lists keep.
pub const LARGEST: usize = 8;
/// The shortest and longest lists the "listLength" setting allows.
pub const MIN_LIST: usize = 3;
/// The longest list the "listLength" setting allows.
pub const MAX_LIST: usize = 25;

/// A scan that takes at least this long is announced when it ends while
/// its page is hidden.
pub const LONG_SCAN_SECS: u64 = 10;

/// The time left of a scan is shown only after this long (the first
/// seconds' pace says little).
pub const ESTIMATE_AFTER_SECS: u64 = 5;

/// The "Keep only the latest" scans count offered first.
pub const KEEP_SCANS: usize = 50;

/// The warning under its requirements in Settings › Add-ons.
pub const READS_EVERYTHING: &str = "Reads every folder of the drive you scan, and saves each finished scan to its History.";

/// The scan as the thread and the page share it.
#[derive(Default)]
pub struct ScanState {
    /// The folder being measured, while a scan runs.
    running: Option<String>,
    /// What was (or is being) measured; shared so a finished scan is
    /// saved off the lock while the page keeps drawing it.
    tree: Option<Arc<SpaceTree>>,
    /// The space used on the drive being measured (None for a folder):
    /// the scan's progress is estimated from it.
    expected: Option<u64>,
    started: Option<Instant>,
    /// Since when the scan is not running (pausing or paused).
    paused_at: Option<Instant>,
    /// How long the scan spent pausing or paused before.
    paused_for: Duration,
    done: Option<Done>,
    /// When the scan was stopped (its results are partial).
    stopped: Option<Done>,
    /// Running, paused, stopped… (only a Finished scan is saved).
    phase: Phase,
    /// The scan's own serial, new at every Start: its thread's last word
    /// (Finished, Stopping -> Stopped, its History save) lands only on
    /// its own scan.
    serial: Option<u64>,
    /// The skipped-items note was dismissed (until the next scan).
    dismissed: bool,
    /// A saved scan that does not say how many folders could not be read.
    unreadable_unknown: bool,
    /// Where the user is in the measured tree (canvas, lists, legend).
    /// It goes with the tree: a new one (a Start, a saved scan opened)
    /// is shown from its root with nothing selected.
    nav: nav::Nav,
}

/// When and how fast a scan finished.
#[derive(Clone, Debug)]
struct Done {
    secs: u64,
    /// Local "hh:mm".
    at: String,
    /// Local day number.
    day: i64,
}

impl Done {
    /// A scan that took `secs` and finished at local time `finished`.
    fn at(finished: i64, secs: u64) -> Done {
        let d = finished.rem_euclid(86_400);
        Done { secs, at: format!("{:02}:{:02}", d / 3600, d % 3600 / 60), day: finished.div_euclid(86_400) }
    }
}

/// What the page shows about the scan, read from the shared state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The folder being measured, while a scan runs.
    pub running: Option<String>,
    /// The folder the results below belong to.
    pub root: Option<String>,
    /// Files counted under the root.
    pub files: u64,
    /// Bytes counted under the root.
    pub bytes: u64,
    /// Folders that could not be read (the total is a floor).
    pub unreadable: u64,
    /// While a drive is measured: the share of its used space counted so far.
    pub progress: Option<f32>,
    /// The level's largest sub-folders (path, bytes).
    pub folders: Vec<(String, u64)>,
    /// The level's largest files, largest first (path, bytes).
    pub largest: Vec<(String, u64)>,
    /// The largest file of the whole scan (the status band).
    pub largest_file: Option<(String, u64)>,
    /// Where the space goes (path, bytes): the deepest folder holding at
    /// least half of its parent, else the largest sub-folder.
    pub largest_folder: Option<(String, u64)>,
    /// "Finished today at 21:12 after 2 min 22 s: …", once finished.
    pub finished: Option<String>,
    /// The status band's sentence, once finished.
    pub where_it_goes: Option<String>,
    /// Every sub-folder of the level shown, largest first.
    pub level: Vec<(String, u64)>,
    /// The breadcrumbs from the root to the level shown.
    pub crumbs: Vec<nav::Crumb>,
    /// What the canvas draws for the level (Balls or Block).
    pub pieces: Vec<nav::Piece>,
    /// Only the files directly in the level are shown.
    pub this_folder_only: bool,
    /// The File Type legend for the level: the seven categories in their order.
    pub types: Vec<TypeRow>,
    /// Where the scan is (Idle before any).
    pub phase: Phase,
    /// The note about unreadable folders, unless dismissed.
    pub skipped: Option<text::Skipped>,
    /// Up to eight unreadable folders, shown under the note.
    pub skipped_examples: Vec<String>,
    /// Under the note: "For example C:\X." and, when access was denied,
    /// that only the system can read it.
    pub skipped_why: Option<String>,
    /// While a drive or All Disks scan runs: "about 2 min left".
    pub left: Option<String>,
    /// When the results shown finished ("today at 14:03").
    pub last_scan: Option<String>,
    /// "Hard-link tracking limit reached. …" once the scan stopped
    /// tracking hard links (some may then be counted more than once).
    pub hard_link_limit: Option<String>,
    /// The selection chip on the canvas, while a piece of the level is
    /// selected (#223).
    pub chip: Option<Chip>,
}

/// The selection chip on the canvas:
/// the piece clicked (or chosen with the keyboard) and what it says.
#[derive(Clone, Debug, PartialEq)]
pub struct Chip {
    /// The piece it stands for (its actions use it).
    pub key: nav::Key,
    /// "v", "this folder", "movie.mkv", "12 other items".
    pub name: String,
    /// The second line: "2 files" for a folder, a file's type ("Media").
    pub detail: String,
    /// Its size.
    pub bytes: u64,
    /// Its full path (None for "N other items").
    pub path: Option<String>,
    /// Where it was clicked, in canvas coordinates (None when chosen
    /// with the keyboard: it is drawn on its piece).
    pub at: Option<[f32; 2]>,
}

/// One category of the File Type legend.
#[derive(Clone, Debug, PartialEq)]
pub struct TypeRow {
    /// The category.
    pub category: Category,
    /// Its files in the level.
    pub files: u64,
    /// Their bytes.
    pub bytes: u64,
}

/// What the lists (and later the canvas) are grouped by: the "Group:" menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Group {
    /// The largest files.
    #[default]
    FileSize,
    /// The file type categories.
    FileType,
    /// The folders of the level.
    FolderSize,
}

impl Group {
    /// Every choice, in the menu's order.
    pub const ALL: [Group; 3] = [Group::FileSize, Group::FileType, Group::FolderSize];

    /// Its words in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Group::FileSize => "File Size",
            Group::FileType => "File Type",
            Group::FolderSize => "Folder Size",
        }
    }
}

/// How the canvas draws the level: the "View:" menu.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Drawing {
    /// Packed circles.
    #[default]
    Balls,
    /// Rectangles (a treemap).
    Block,
}

impl Drawing {
    /// Every choice, in the menu's order.
    pub const ALL: [Drawing; 2] = [Drawing::Balls, Drawing::Block];

    /// Its words in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Drawing::Balls => "Balls",
            Drawing::Block => "Block",
        }
    }
}

/// One entry of the grouped list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The path of a file or folder, or a category's name.
    pub label: String,
    /// Its size.
    pub bytes: u64,
    /// A second line ("2 files" for a category).
    pub detail: Option<String>,
    /// Where it is, for the item actions (None for a category).
    pub path: Option<String>,
    /// Its colour (a file's or a category's type; None for a folder).
    pub category: Option<Category>,
}

impl View {
    /// The status strip's right-hand words: when the results shown
    /// finished ("Done today at 14:03") or were stopped ("Partial …"),
    /// or "Not scanned".
    pub fn strip_last(&self) -> String {
        let prefix = if matches!(self.phase, Phase::Stopping | Phase::Stopped) { "Partial" } else { "Done" };
        self.last_scan.as_ref().map_or_else(|| "Not scanned".into(), |at| format!("{prefix} {at}"))
    }

    /// How much of the status strip's LED bar is lit (0 to 1): the share
    /// of the used space counted so far, for a drive or All disks only;
    /// full once such a scan finished. A folder has no known total, so its
    /// strip stays empty (the words under it count the files instead).
    pub fn strip_fill(&self) -> f32 {
        let whole = self.root.as_deref().and_then(Target::parse).is_some_and(|t| !matches!(t, Target::Folder(_)));
        match (whole, &self.running) {
            (false, _) => 0.0,
            (true, Some(_)) => self.progress.unwrap_or(0.0),
            (true, None) if self.phase == Phase::Finished => 1.0,
            (true, None) => 0.0,
        }
    }

    /// The list for the group chosen: the largest files, the file types
    /// present (largest first) or the folders of the level.
    pub fn rows(&self, group: Group) -> Vec<Row> {
        let place = |(p, b): &(String, u64), category| Row { label: p.clone(), bytes: *b, detail: None, path: Some(p.clone()), category };
        match group {
            Group::FileSize => self.largest.iter().map(|f| place(f, Some(Category::of(Path::new(&f.0))))).collect(),
            Group::FolderSize => self.level.iter().map(|f| place(f, None)).collect(),
            Group::FileType => {
                let mut present: Vec<&TypeRow> = self.types.iter().filter(|t| t.files > 0).collect();
                present.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(a.category.index().cmp(&b.category.index())));
                present
                    .into_iter()
                    .map(|t| Row { label: t.category.name().into(), bytes: t.bytes, detail: Some(text::files(t.files)), path: None, category: Some(t.category) })
                    .collect()
            }
        }
    }
}

/// The add-on: the scan's state, and the target the user chose.
#[derive(Default)]
pub struct DiskSpace {
    state: Arc<Mutex<ScanState>>,
    generation: Generation,
    /// What the Start button measures (its setting); None until chosen.
    target: Option<Target>,
    /// The "Group:" choice (its setting).
    group: Group,
    /// The "View:" choice (its setting).
    drawing: Drawing,
    /// The settings it was given, kept for the fields it does not know.
    saved: serde_json::Value,
    /// For the folder dialog.
    context: Option<AddonContext>,
    /// The Balls / Block canvas and its last layout.
    canvas: canvas::Canvas,
    /// The last link to an Installed Apps row (asked at most every 2 s).
    links: actions::Links,
    /// A `--scan` waiting for the first sample (it lists the drives).
    pending: Option<ScanRequest>,
    /// "Don't ask again" ticked in the Stop confirmation (its setting).
    suppress_stop_confirm: bool,
    /// How many items the largest lists show (None = [`LARGEST`]).
    list_len: Option<usize>,
    /// How many finished scans History keeps (None = all).
    keep_scans: Option<usize>,
    /// The Summary tile is on (off by default).
    summary_tile: bool,
    /// Its page was drawn and has not been hidden since.
    page_shown: bool,
    /// The scan (serial) already announced or seen on its page.
    noticed: Option<u64>,
    /// [`LONG_SCAN_SECS`] unless a test says otherwise.
    long_after: Option<u64>,
    /// The Stop confirmation, while it is open.
    confirm: Option<StopQuestion>,
    /// The "History folder" setting (None = the app's folder).
    history_folder: Option<String>,
    /// The history folder (None until started; never the real one in tests).
    store: Option<Arc<history::Store>>,
    /// The saved scans as listed, shared with the thread that saves.
    shelf: Arc<Mutex<history::Shelf>>,
    /// The target whose last saved scan was looked up.
    shown_for: Option<String>,
    /// The Clear History confirmation is open.
    clear_asked: bool,
    /// Scan threads (a stopped one ends at its next folder) and threads
    /// reading a saved scan, still alive.
    threads: Arc<std::sync::atomic::AtomicUsize>,
    /// Run by the next scan's thread once that scan is published as
    /// finished, before it is saved (tests hold a finishing scan there).
    #[cfg(test)]
    ending: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Run by the next saved scan's reading thread before it reads the
    /// file (tests hold a saved scan being opened there).
    #[cfg(test)]
    loading: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// How far tests moved the clock ahead.
    #[cfg(test)]
    skew: Mutex<Duration>,
    /// The last view computed from the tree, kept until the tree, the
    /// level, the group or the list length changes.
    view_cache: Mutex<ViewCache>,
}

/// What a view computed from the tree depends on.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ViewKey {
    revision: u64,
    level: tree::NodeId,
    this_folder_only: bool,
    group: Group,
    list_length: usize,
}

/// The tree's part of the last view (see [`DiskSpace::view`]).
#[derive(Default)]
struct ViewCache {
    key: Option<ViewKey>,
    view: View,
    /// Where the space goes: (short name, bytes), for the status sentence.
    big: Option<(String, u64)>,
    /// How many views were computed (a test counts them).
    computed: usize,
}

/// Counts a live scan thread until dropped.
struct Alive(Arc<std::sync::atomic::AtomicUsize>);

impl Alive {
    fn new(count: &Arc<std::sync::atomic::AtomicUsize>) -> Alive {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Alive(count.clone())
    }
}

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// The open Stop confirmation.
#[derive(Clone, Debug, PartialEq)]
pub struct StopQuestion {
    /// "Stop scanning C:\?"
    pub title: String,
    /// How far the scan is, and that a stopped scan is not saved.
    pub text: String,
    /// The target chosen while the scan ran, taken once it is stopped.
    then_target: Option<Target>,
    /// The "Don't ask again" box.
    dont_ask: bool,
}

/// Disk Space's own settings, as saved: a default value is left out,
/// and the fields it does not know are kept as they were.
#[derive(serde::Deserialize, serde::Serialize, Default)]
#[serde(default)]
struct Prefs {
    /// The drive or folder last chosen for a scan ("drive" before #81).
    #[serde(alias = "drive", skip_serializing_if = "String::is_empty")]
    target: String,
    #[serde(skip_serializing_if = "is_default")]
    group: Group,
    #[serde(skip_serializing_if = "is_default")]
    view: Drawing,
    #[serde(rename = "diskSpaceSuppressStopConfirm", skip_serializing_if = "is_default")]
    suppress_stop_confirm: bool,
    #[serde(rename = "listLength", skip_serializing_if = "Option::is_none")]
    list_length: Option<usize>,
    #[serde(rename = "keepScans", skip_serializing_if = "Option::is_none")]
    keep_scans: Option<usize>,
    #[serde(rename = "summaryTile", skip_serializing_if = "is_default")]
    summary_tile: bool,
    /// Where History is kept (None = the app's folder).
    #[serde(rename = "historyFolder", skip_serializing_if = "Option::is_none")]
    history_folder: Option<String>,
    /// The fields of newer versions, written back untouched.
    #[serde(flatten)]
    rest: serde_json::Map<String, serde_json::Value>,
}

/// A running scan's handle to the shared state.
pub type Job = crate::platform::Job<ScanState>;

/// The scanner's folder boundary: holds it while the scan is paused
/// (turning Pause pending into Paused) and says whether to go on.
pub fn checkpoint(job: &Job) -> bool {
    loop {
        let mut paused = false;
        job.publish(|s| {
            s.phase = s.phase.after(Step::Parked);
            paused = s.phase == Phase::Paused;
        });
        if job.cancelled() {
            return false;
        }
        if !paused {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Hands a batch of the scanner's events to the scan's tree.
pub fn deliver(job: &Job, events: Vec<Event>) {
    job.publish(|s| {
        if let Some(t) = s.tree.as_mut() {
            Arc::make_mut(t).apply(events);
        }
    });
}

fn path_text(p: &Path) -> String {
    p.display().to_string()
}

/// The last part of a path, for sentences ("DockerSandboxes").
fn short_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path_text(p))
}

impl DiskSpace {
    /// What the page shows about the scan now. The tree's part is
    /// computed once per delivery (and per level, group and list length),
    /// however often the page and the menu ask.
    pub fn view(&self) -> View {
        let s = lock_state(&self.state);
        let Some(t) = &s.tree else { return View::default() };
        let root = SpaceTree::ROOT;
        let key = ViewKey { revision: t.revision(), level: s.nav.level(t), this_folder_only: s.nav.this_folder_only(), group: self.group, list_length: self.list_length() };
        let mut cache = lock_state(&self.view_cache);
        if cache.key != Some(key) {
            let (view, big) = self.tree_view(t, &s.nav, key);
            *cache = ViewCache { key: Some(key), view, big, computed: cache.computed + 1 };
        }
        let mut v = cache.view.clone();
        let bytes = v.bytes;
        v.finished = s.done.as_ref().map(|d| {
            let today = crate::platform::local_secs().div_euclid(86_400);
            text::finished(&text::day_word(d.day, today), &d.at, d.secs, bytes, v.files)
        });
        let root_text = v.root.as_deref().unwrap_or_default();
        v.where_it_goes = s.done.as_ref().and(cache.big.as_ref()).map(|(name, b)| text::where_it_goes(name, *b, bytes, root_text));
        v.skipped = match (s.dismissed, s.unreadable_unknown) {
            (true, _) => None,
            (false, true) => Some(text::Skipped { heading: None, count: None, sentence: text::unreadable_unknown() }),
            (false, false) => text::skipped(t.unreadable(root), s.done.is_some()),
        };
        v.skipped_examples = if v.skipped.is_some() { t.unreadable_examples().iter().map(|p| path_text(p)).collect() } else { Vec::new() };
        // The example is one only the system can read, when there is one.
        let (denied, first_denied) = t.denied();
        let example = first_denied.map(path_text).or_else(|| v.skipped_examples.first().cloned());
        v.skipped_why = example.filter(|_| v.skipped.is_some() && !s.unreadable_unknown).map(|e| text::skipped_why(&e, denied));
        v.left = self.time_left(&s, bytes);
        v.running = s.running.clone();
        v.progress = s.running.as_ref().and(s.expected).filter(|e| *e > 0).map(|e| (bytes as f32 / e as f32).min(0.99));
        v.phase = s.phase;
        v.last_scan = s.done.as_ref().or(s.stopped.as_ref()).map(|d| format!("{} at {}", text::day_word(d.day, crate::platform::local_secs().div_euclid(86_400)), d.at));
        v.chip = Self::chip(t, &s.nav, &v.pieces);
        v
    }

    /// The chip of the piece selected in `nav`, when it is a piece of the level.
    fn chip(t: &SpaceTree, nav: &nav::Nav, pieces: &[nav::Piece]) -> Option<Chip> {
        let key = nav.selected()?;
        let piece = pieces.iter().find(|p| &p.key == key)?;
        let level = nav.level(t);
        let detail = match key {
            nav::Key::Folder(p) => text::files(t.find(p).map_or(0, |n| t.files(n))),
            nav::Key::ThisFolder => text::files(t.loose(level).1),
            nav::Key::File(_) => piece.category.name().to_string(),
            nav::Key::Rest => format!("mostly {}", piece.category.name()),
        };
        let path = actions::item(key, &t.path(level)).map(|p| path_text(&p));
        Some(Chip { key: key.clone(), name: piece.label.clone(), detail, bytes: piece.bytes, path, at: nav.point() })
    }

    /// The part of the view read from the tree alone, for `key` (the
    /// level of `nav`); and where the space goes (short name, bytes).
    fn tree_view(&self, t: &SpaceTree, nav: &nav::Nav, key: ViewKey) -> (View, Option<(String, u64)>) {
        let root = SpaceTree::ROOT;
        let (level, only, length) = (key.level, key.this_folder_only, key.list_length);
        let list = |v: Vec<(PathBuf, u64)>| v.into_iter().map(|(p, b)| (path_text(&p), b)).collect::<Vec<_>>();
        let big = t.dominant_folder().or_else(|| t.largest_folder());
        // The level's sub-folders, sorted once for both lists.
        let all: Vec<(String, u64)> = if only { Vec::new() } else { list(t.largest_folders(level, usize::MAX)) };
        let view = View {
            root: Some(path_text(&t.path(root))),
            files: t.files(root),
            bytes: t.bytes(root),
            unreadable: t.unreadable(root),
            folders: all.iter().take(length).cloned().collect(),
            largest: list(if only { t.loose_files(level).into_iter().take(length).collect() } else { t.largest_files(level, length) }),
            largest_file: t.largest_files(root, 1).first().map(|(p, b)| (path_text(p), *b)),
            largest_folder: big.map(|n| (path_text(&t.path(n)), t.bytes(n))),
            level: all,
            crumbs: nav.crumbs(t),
            pieces: nav.pieces(t),
            this_folder_only: only,
            hard_link_limit: t.hard_link_limit().then(|| text::HARD_LINK_LIMIT.to_string()),
            types: Category::ALL
                .iter()
                .map(|&category| {
                    let (bytes, files) = if only { t.loose_category(level, category) } else { t.category(level, category) };
                    TypeRow { category, files, bytes }
                })
                .collect(),
            ..View::default()
        };
        (view, big.map(|n| (short_name(&t.path(n)), t.bytes(n))))
    }

    /// How many times [`DiskSpace::view`] computed the tree's part.
    #[cfg(test)]
    pub fn view_computations(&self) -> usize {
        lock_state(&self.view_cache).computed
    }

    /// Starts measuring the folder `root` unless a scan is running.
    #[cfg(test)]
    pub fn scan(&self, root: impl Into<PathBuf>, expected: Option<u64>) {
        let root = path_text(&root.into());
        self.measure(&Target::Folder(root.clone()), Plan { roots: vec![root], expected });
    }

    /// Starts measuring `target` as `plan` says (one walk per root; All
    /// disks under one virtual root) unless a scan is running.
    pub fn measure(&self, target: &Target, plan: Plan) {
        let tree = match target {
            Target::AllDisks => SpaceTree::virtual_root(target::ALL_DISKS, &plan.roots.iter().map(String::as_str).collect::<Vec<_>>(), self.list_length()),
            _ => SpaceTree::new(plan.roots.first().cloned().unwrap_or_default(), self.list_length()),
        };
        let roots = plan.roots;
        self.scan_tree_with(tree, plan.expected, move |job| {
            for root in &roots {
                scan::walk(Path::new(root), &|| checkpoint(job), &mut |events| deliver(job, events));
            }
        });
    }

    /// Runs `work` in a thread as the scan of `root`, with fresh results;
    /// `work` feeds the tree with [`deliver`]. Test seam.
    #[cfg(test)]
    pub fn scan_with(&self, root: impl Into<PathBuf>, expected: Option<u64>, work: impl FnOnce(&Job) + Send + 'static) {
        self.scan_tree_with(SpaceTree::new(root, self.list_length()), expected, work);
    }

    /// Runs `work` in a thread as the scan that fills `tree`.
    fn scan_tree_with(&self, tree: SpaceTree, expected: Option<u64>, work: impl FnOnce(&Job) + Send + 'static) {
        let (job, serial) = {
            let mut s = lock_state(&self.state);
            if s.running.is_some() {
                return;
            }
            // Every Start takes a new serial: every older job is cancelled
            // (it publishes nothing more) and its thread's last word no
            // longer lands on this scan.
            self.generation.bump();
            let serial = self.generation.current();
            *s = ScanState {
                running: Some(path_text(&tree.path(SpaceTree::ROOT))),
                tree: Some(Arc::new(tree)),
                expected,
                started: Some(self.now()),
                paused_at: None,
                paused_for: Duration::ZERO,
                done: None,
                stopped: None,
                phase: Phase::Idle.after(Step::Start),
                serial: Some(serial),
                dismissed: false,
                unreadable_unknown: false,
                nav: nav::Nav::default(),
            };
            (Job::new(&self.state, &self.generation), serial)
        };
        let state = self.state.clone();
        let (store, shelf, keep) = (self.store.clone(), self.shelf.clone(), self.keep_scans);
        let alive = Alive::new(&self.threads);
        #[cfg(test)]
        let ending = lock_state(&self.ending).take();
        std::thread::spawn(move || {
            let _alive = alive;
            work(&job);
            let now = crate::platform::local_secs();
            // The scan ends under one lock: a Start comes either before
            // (a new serial, so this thread says nothing more) or after
            // (this scan is no longer running), never in between.
            let mut s = lock_state(&state);
            if s.serial != Some(serial) {
                return;
            }
            // A stopped job publishes nothing, but its scan still learns
            // that the scanner ended (Stopping -> Stopped).
            if !job.cancelled() {
                let secs = s.started.map_or(0, |t| t.elapsed().as_secs());
                s.done = Some(Done::at(now, secs));
                s.running = None;
            }
            s.phase = s.phase.after(Step::Ended);
            // Only a finished scan goes to History.
            let Some(store) = store.filter(|_| s.phase.saveable()) else { return };
            let secs = s.done.as_ref().map_or(0, |d| d.secs) as i64;
            // The snapshot is built off the lock, from the finished tree
            // the page keeps showing (nothing changes it any more), so the
            // window keeps drawing while a large scan is saved.
            let Some(tree) = s.tree.clone() else { return };
            drop(s);
            let snapshot = history::Snapshot::of(&tree, &history::new_id(), now - secs, now, history::LIMIT);
            drop(tree);
            #[cfg(test)]
            if let Some(ending) = ending {
                ending();
            }
            let saved = store.save(&snapshot);
            // Past the "keepScans" setting the oldest go; History is
            // listed again from the folder so it shows what is left.
            let listed = keep.map(|k| store.prune(k).map(|_| store.list()));
            let mut shelf = lock_state(&shelf);
            match (saved, listed) {
                (Ok(summary), None) => shelf.entries.insert(0, summary),
                (saved, Some(Ok(listed))) => *shelf = history::Shelf { error: saved.err(), ..listed },
                (Err(e), _) | (_, Some(Err(e))) => shelf.error = Some(e),
            }
        });
    }

    /// Holds the next scan's thread in `gate` once that scan is published
    /// as finished, before it is saved to History. Test seam.
    #[cfg(test)]
    pub fn gate_ending(&self, gate: impl FnOnce() + Send + 'static) {
        *lock_state(&self.ending) = Some(Box::new(gate));
    }

    /// Holds the next saved scan opened in `gate`, before its file is
    /// read. Test seam.
    #[cfg(test)]
    pub fn gate_loading(&self, gate: impl FnOnce() + Send + 'static) {
        *lock_state(&self.loading) = Some(Box::new(gate));
    }

    /// The clock the scan's elapsed time is read from.
    fn now(&self) -> Instant {
        #[cfg(test)]
        return Instant::now() + *lock_state(&self.skew);
        #[cfg(not(test))]
        Instant::now()
    }

    /// What is left of a running drive or All Disks scan: from the bytes
    /// counted against the space in use, at the pace so far (time spent
    /// paused does not count). None for a folder (no expected size), in
    /// the first seconds, while paused, or once the count passed the space
    /// in use.
    fn time_left(&self, s: &ScanState, bytes: u64) -> Option<String> {
        let (expected, started) = (s.expected?, s.started?);
        if s.running.is_none() || s.phase != Phase::Running || bytes == 0 || bytes >= expected {
            return None;
        }
        let elapsed = self.now().saturating_duration_since(started).saturating_sub(s.paused_for).as_secs_f64();
        if elapsed < ESTIMATE_AFTER_SECS as f64 {
            return None;
        }
        let secs = elapsed * (expected - bytes) as f64 / bytes as f64;
        Some(text::left(secs.round() as u64))
    }

    /// Moves the clock the time left is estimated with `by` ahead. Test seam.
    #[cfg(test)]
    pub fn advance_clock(&self, by: Duration) {
        *lock_state(&self.skew) += by;
    }

    /// Lists the saved scans of `store`, the history folder from now on.
    pub fn use_history(&mut self, store: history::Store) {
        *lock_state(&self.shelf) = store.list();
        self.store = Some(Arc::new(store));
        self.shown_for = None;
    }

    /// Lists History from the "History folder" setting, or from the
    /// app's folder when it is not set.
    fn list_history(&mut self) {
        match self.history_folder.clone().map(|d| Ok(history::Store::new(d))).or_else(app_history) {
            Some(Ok(store)) => self.use_history(store),
            Some(Err(e)) => {
                self.store = None;
                *lock_state(&self.shelf) = history::Shelf { error: Some(e), ..history::Shelf::default() };
            }
            None => {
                self.store = None;
                *lock_state(&self.shelf) = history::Shelf::default();
            }
        }
        self.shown_for = None;
    }

    /// The "History folder" setting: `dir`, or the app's folder for None
    /// (Reset to default). History is listed again from there.
    pub fn set_history_folder(&mut self, dir: Option<String>) {
        self.history_folder = dir;
        self.list_history();
    }

    /// How many items the largest lists show.
    pub fn list_length(&self) -> usize {
        self.list_len.unwrap_or(LARGEST)
    }

    /// The "Ask before stopping a scan" setting (turns "Don't ask again"
    /// back off).
    pub fn ask_before_stopping(&mut self, ask: bool) {
        self.suppress_stop_confirm = !ask;
    }

    /// The Summary tile is on.
    pub fn shows_summary_tile(&self) -> bool {
        self.summary_tile
    }

    /// Its page is being drawn: a scan finished now is seen there.
    pub fn page_seen(&mut self) {
        self.page_shown = true;
        self.notice();
    }

    /// Once per finished scan that took long and ended while the page was
    /// hidden: a short line for the status bar.
    pub fn notice(&mut self) -> Option<String> {
        let s = lock_state(&self.state);
        let done = s.done.as_ref().filter(|_| s.phase == Phase::Finished && s.serial.is_some() && s.serial != self.noticed)?;
        self.noticed = s.serial;
        if self.page_shown || done.secs < self.long_after.unwrap_or(LONG_SCAN_SECS) {
            return None;
        }
        let t = s.tree.as_ref()?;
        let root = path_text(&t.path(SpaceTree::ROOT));
        Some(format!("Disk Space finished measuring {root}: {} in {}.", text::size(t.bytes(SpaceTree::ROOT)), text::files(t.files(SpaceTree::ROOT))))
    }

    /// How many scan threads are still running.
    #[cfg(test)]
    pub fn scan_threads(&self) -> usize {
        self.threads.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// The saved scans as listed.
    pub fn history(&self) -> history::Shelf {
        lock_state(&self.shelf).clone()
    }

    /// Shows saved scan `id` as the result on screen (not during a scan);
    /// one that cannot be read is counted and left out of History. It is
    /// read in the background; the page keeps what it shows until then.
    pub fn open_saved(&self, id: &str) {
        let Some(store) = self.store.clone() else { return };
        // Opening takes a new serial, as a Start does: a Start or another
        // opening made meanwhile wins, and this one then shows nothing.
        let job = {
            let s = lock_state(&self.state);
            if s.running.is_some() {
                return;
            }
            self.generation.bump();
            Job::new(&self.state, &self.generation)
        };
        let (shelf, keep, id) = (self.shelf.clone(), self.list_length(), id.to_string());
        let alive = Alive::new(&self.threads);
        #[cfg(test)]
        let loading = lock_state(&self.loading).take();
        std::thread::spawn(move || {
            let _alive = alive;
            #[cfg(test)]
            if let Some(loading) = loading {
                loading();
            }
            match store.load(&id) {
                Ok(saved) => {
                    let secs = (saved.finished - saved.started).max(0) as u64;
                    let loaded = ScanState { tree: Some(Arc::new(saved.tree(keep))), done: Some(Done::at(saved.finished, secs)), phase: Phase::Finished, unreadable_unknown: saved.unreadable_count.is_none(), ..ScanState::default() };
                    job.publish(|s| {
                        if s.running.is_none() {
                            *s = loaded;
                        }
                    });
                }
                Err(_) => {
                    let mut shelf = lock_state(&shelf);
                    shelf.entries.retain(|e| e.id != id);
                    shelf.left_out += 1;
                }
            }
        });
    }

    /// A scan or a saved scan being opened still works in the background
    /// (the page is drawn again until it is done).
    pub fn working(&self) -> bool {
        self.threads.load(std::sync::atomic::Ordering::SeqCst) > 0
    }

    /// When the page opens or the target changes: shows the target's last
    /// saved scan, unless a scan runs or the target's results are shown.
    pub fn show_last_of_target(&mut self) {
        let Some(name) = self.target.as_ref().map(|t| t.name().to_string()) else { return };
        if self.shown_for.as_deref() == Some(name.as_str()) {
            return;
        }
        self.shown_for = Some(name.clone());
        let v = self.view();
        if v.running.is_some() || v.root.as_deref() == Some(name.as_str()) {
            return;
        }
        let last = lock_state(&self.shelf).entries.iter().find(|e| e.root_path == name).map(|e| e.id.clone());
        if let Some(id) = last {
            self.open_saved(&id);
        }
    }

    /// Clear History…: asks first (nothing to ask while History is empty).
    pub fn ask_clear(&mut self) {
        self.clear_asked = !lock_state(&self.shelf).entries.is_empty();
    }

    /// The open Clear History confirmation (title, text).
    pub fn clear_question(&self) -> Option<(&'static str, String)> {
        self.clear_asked.then(|| (history::CLEAR_TITLE, history::clear_text(lock_state(&self.shelf).entries.len())))
    }

    /// The answer to Clear History: removes every saved scan when `clear`.
    pub fn answer_clear(&mut self, clear: bool) {
        if !std::mem::take(&mut self.clear_asked) || !clear {
            return;
        }
        let Some(store) = &self.store else { return };
        let result = store.clear();
        let mut shelf = store.list();
        if let Err(e) = result {
            shelf.error = Some(e);
        }
        *lock_state(&self.shelf) = shelf;
    }

    /// Moves through the measured tree (canvas, breadcrumbs, keyboard,
    /// "Open the folder"); nothing happens before a scan.
    pub fn navigate(&mut self, m: nav::Move) {
        let mut s = lock_state(&self.state);
        let s = &mut *s;
        if let Some(t) = &s.tree {
            s.nav.go(t, m);
        }
    }

    /// The piece selected on the canvas or in a list.
    fn selected(&self) -> Option<nav::Key> {
        lock_state(&self.state).nav.selected().cloned()
    }

    /// The item a piece stands for ("this folder" = the level's folder);
    /// None for "N other items" or before a scan.
    fn item(&self, key: &nav::Key) -> Option<PathBuf> {
        item_of(&self.state, key)
    }

    /// Carries out an item action (panel or context menu): Explorer and
    /// the clipboard go to the app as shared actions, "Open the folder"
    /// moves inside Disk Space, a failure is said in the status bar.
    pub fn act(&mut self, verb: actions::Verb, key: &nav::Key, page: &mut PageContext) {
        match actions::outcome(verb, self.item(key).as_deref(), actions::on_disk) {
            actions::Outcome::Open(path) => self.navigate(nav::Move::Open(path)),
            actions::Outcome::Reveal(path) => page.actions.push(Action::ShowInExplorer(path)),
            actions::Outcome::Copy(text) => page.actions.push(Action::Copy(text)),
            actions::Outcome::Say(why) => page.actions.push(Action::Say(why.into())),
        }
    }

    /// The row of another add-on that is on (Installed Apps) that owns the
    /// item, for a link to it.
    pub fn owner(&self, key: &nav::Key, page: &PageContext) -> Option<crate::platform::Owner> {
        self.item(key).and_then(|p| page.owner_of(&p))
    }

    /// Follows that link: the app opens the owner's page on its row.
    pub fn show_owner(&self, key: &nav::Key, page: &mut PageContext) {
        if let (Some(o), Some(path)) = (self.owner(key, page), self.item(key)) {
            page.actions.push(Action::ShowIn { addon: o.addon, path });
        }
    }

    /// Moves the scan to the phase after `step`, timing how long it is not
    /// running (pausing or paused).
    fn step(&self, step: Step) {
        let mut s = lock_state(&self.state);
        let was_running = s.phase == Phase::Running;
        s.phase = s.phase.after(step);
        let now = self.now();
        match (was_running, s.phase == Phase::Running) {
            (true, false) => s.paused_at = Some(now),
            (false, true) => {
                if let Some(at) = s.paused_at.take() {
                    s.paused_for += now.saturating_duration_since(at);
                }
            }
            _ => {}
        }
    }

    /// Asks the running scan to pause at the next folder.
    pub fn pause_scan(&self) {
        self.step(Step::Pause);
    }

    /// Lets a paused scan go on.
    pub fn resume_scan(&self) {
        self.step(Step::Resume);
    }

    /// Stops the running scan; what it measured stays on screen, but it
    /// is not saved.
    pub fn stop_scan(&self) {
        let mut s = lock_state(&self.state);
        if s.running.is_some() {
            self.generation.bump();
            s.running = None;
            s.phase = s.phase.after(Step::Stop);
            let secs = s.started.map_or(0, |t| self.now().saturating_duration_since(t).as_secs());
            s.stopped = Some(Done::at(crate::platform::local_secs(), secs));
        }
    }

    /// Runs a pending `--scan` once `disks` can tell what it means: at
    /// once for a folder, at the first sample listing drives otherwise.
    fn run_pending(&mut self, disks: &[DiskRow]) {
        let Some(request) = &self.pending else { return };
        let target = match request {
            ScanRequest::Target(t) => Some(t.clone()),
            ScanRequest::Saved => self.target.clone(),
        };
        let target = match target {
            Some(t) => t,
            None if disks.is_empty() => return,
            None => Target::Drive(disks[0].mount.clone()),
        };
        if disks.is_empty() && !matches!(target, Target::Folder(_)) {
            return;
        }
        self.pending = None;
        if let Ok(plan) = target.plan(disks) {
            self.measure(&target, plan);
        }
        self.target = Some(target);
    }

    /// Stop Scanning: asks first (unless "Don't ask again" was ticked).
    /// `then_target` is the target chosen while the scan ran, taken once
    /// it is stopped.
    pub fn ask_stop(&mut self, then_target: Option<Target>) {
        if self.suppress_stop_confirm {
            self.stop_scan();
            if let Some(t) = then_target {
                self.target = Some(t);
            }
            return;
        }
        let v = self.view();
        self.confirm = Some(StopQuestion {
            title: phase::stop_title(v.running.as_deref().unwrap_or_default()),
            text: phase::stop_text(v.progress),
            then_target,
            dont_ask: false,
        });
    }

    /// The answer to the Stop confirmation: Stop Scanning (`stop`) or
    /// Keep Scanning; "Don't ask again" counts only when stopping.
    pub fn answer_stop(&mut self, stop: bool, dont_ask: bool) {
        let Some(q) = self.confirm.take() else { return };
        if stop {
            self.suppress_stop_confirm |= dont_ask;
            self.stop_scan();
            if let Some(t) = q.then_target {
                self.target = Some(t);
            }
        }
    }

    /// A target picked in the drop-down: during a scan, asks to stop it first.
    pub fn choose_target(&mut self, target: Option<Target>) {
        if target == self.target {
            return;
        }
        if self.view().running.is_some() {
            self.ask_stop(target);
        } else {
            self.target = target;
        }
    }

    /// Hides the skipped-items note until the next scan.
    pub fn dismiss_skipped(&self) {
        lock_state(&self.state).dismissed = true;
    }

    /// What the toolbar and the Disk Space menu offer now, with `disks`
    /// connected.
    pub fn menu(&self, disks: &[DiskRow]) -> Menu {
        let v = self.view();
        let running = v.running.is_some();
        let start = self.target.as_ref().map_or_else(|| "Start".to_string(), |t| start_label(t, v.root.as_deref()));
        let can_start = !running && self.target.as_ref().is_some_and(|t| t.plan(disks).is_ok());
        let pause = if matches!(v.phase, Phase::Paused | Phase::PausePending) { "Resume" } else { "Pause" };
        Menu { start, can_start, running, pause, can_clear: !running && !self.history().entries.is_empty() }
    }

    /// Carries out a toolbar or menu command.
    pub fn command(&mut self, c: Command, disks: &[DiskRow]) {
        match c {
            Command::Start => {
                let Some(t) = self.target.clone() else { return };
                if self.view().running.is_some() {
                    return;
                }
                if let Ok(plan) = t.plan(disks) {
                    self.measure(&t, plan);
                }
            }
            Command::Pause => self.pause_scan(),
            Command::Resume => self.resume_scan(),
            Command::Stop => {
                if self.view().running.is_some() {
                    self.ask_stop(None);
                }
            }
            Command::Target(t) => self.choose_target(Some(t)),
            Command::Group(g) => self.group = g,
            Command::View(d) => self.drawing = d,
            Command::ClearHistory => self.ask_clear(),
        }
    }

    /// Choose Folder…: the folder picker (None when cancelled).
    fn pick_folder(&self) -> Option<Target> {
        self.context.as_ref().and_then(|c| c.pick_folder("Choose a folder to measure", "Choose")).map(Target::Folder)
    }
}

/// The main button's words: "Start: measure C:\" or "… again" when the
/// results shown are that target's.
pub fn start_label(t: &Target, root: Option<&str>) -> String {
    let again = root == Some(t.name());
    format!("Start: measure {}{}", t.name(), if again { " again" } else { "" })
}

/// What the toolbar and the Disk Space menu offer now.
#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    /// "Start: measure C:\" / "… again".
    pub start: String,
    /// No scan runs and the target can be measured.
    pub can_start: bool,
    /// A scan runs (Pause/Resume and Stop are on).
    pub running: bool,
    /// "Pause" or "Resume".
    pub pause: &'static str,
    /// Some scan is saved and none runs.
    pub can_clear: bool,
}

/// A command of the toolbar or of the Disk Space menu.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    Start,
    Pause,
    Resume,
    Stop,
    Target(Target),
    Group(Group),
    View(Drawing),
    ClearHistory,
}

impl Addon for DiskSpace {
    fn id(&self) -> &'static str {
        "disk-space"
    }
    fn name(&self) -> &'static str {
        "Disk Space"
    }
    fn aliases(&self) -> &'static [&'static str] {
        // The page's alternative id.
        &["diskspace"]
    }
    fn description(&self) -> &'static str {
        "Free and used space on every drive, and the folders and files that take it."
    }
    /// No administrator (protected folders are counted as unreadable), no
    /// internet, no other program; it warns that it reads the whole disk.
    fn requirements(&self) -> Requirements {
        Requirements { note: Some(READS_EVERYTHING), ..Requirements::default() }
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "Low", network: "None", disk: "High while scanning", when: "Only when you start a scan" }
    }

    /// Lists the scan history (small summaries only); never scans.
    fn start(&mut self, context: &AddonContext) {
        self.context = Some(context.clone());
        self.list_history();
    }

    fn ui(&mut self, ui: &mut Ui, page: &mut PageContext) {
        self.draw_page(ui, page);
    }

    /// Its settings ([`Prefs`]), with the other fields it was given
    /// (`Null` while nothing was chosen nor saved).
    fn settings(&self) -> serde_json::Value {
        let prefs = Prefs {
            target: self.target.as_ref().map(Target::setting).unwrap_or_default(),
            group: self.group,
            view: self.drawing,
            suppress_stop_confirm: self.suppress_stop_confirm,
            list_length: self.list_len,
            keep_scans: self.keep_scans,
            summary_tile: self.summary_tile,
            history_folder: self.history_folder.clone(),
            rest: read_settings::<Prefs>(&self.saved).rest,
        };
        match write_settings(&prefs) {
            serde_json::Value::Null if !self.saved.is_object() => self.saved.clone(),
            v => v,
        }
    }

    fn load_settings(&mut self, settings: &serde_json::Value) {
        let prefs = read_settings::<Prefs>(settings);
        self.target = Target::parse(&prefs.target);
        self.group = prefs.group;
        self.drawing = prefs.view;
        self.suppress_stop_confirm = prefs.suppress_stop_confirm;
        self.list_len = prefs.list_length.map(|n| n.clamp(MIN_LIST, MAX_LIST));
        self.keep_scans = prefs.keep_scans.map(|n| n.max(1));
        self.summary_tile = prefs.summary_tile;
        self.history_folder = prefs.history_folder.filter(|d| !d.is_empty());
        self.saved = settings.clone();
    }

    /// `--scan` / `--scan=<target>`: measure without a click, as soon as
    /// the drives are known.
    fn command_line(&mut self, args: &[String]) -> bool {
        self.pending = target::scan_request(args);
        false
    }

    fn page_hidden(&mut self) {
        self.page_shown = false;
    }

    fn menu_title(&self) -> Option<&'static str> {
        Some("Disk Space")
    }

    /// Start, Pause/Resume, Stop, target, group, view, Clear History…
    fn header_menu(&mut self, ui: &mut Ui, page: &mut PageContext) {
        let disks = &page.sample.disks;
        if let Some(c) = self.menu_items(ui, disks) {
            self.command(c, disks);
        }
    }

    /// A long scan that ended while the page was hidden says so in the
    /// status bar (draws nothing).
    fn status_bar(&mut self, _ui: &mut Ui, page: &mut PageContext) {
        if let Some(line) = self.notice() {
            page.actions.push(Action::Say(line));
        }
    }

    /// Each drive's free space and the last scan, when turned on.
    fn summary(&mut self, ui: &mut Ui, page: &mut PageContext) {
        if !self.shows_summary_tile() {
            return;
        }
        let sub = page.sub_text();
        let today = crate::platform::local_secs().div_euclid(86_400);
        let history = self.history();
        ui.add_space(10.0);
        ui.label(RichText::new("Disk Space").strong());
        for line in tile(&page.sample.disks, history.entries.first(), today) {
            ui.label(RichText::new(line).color(sub));
        }
    }

    /// The Stop confirmation, the lists' length, how many scans History
    /// keeps and the Summary tile.
    fn settings_section(&mut self, ui: &mut Ui, page: &mut PageContext) {
        use crate::widgets::parts::{group_heading, setting_folder, setting_number, setting_switch, sub_setting_number, FolderAct};
        let _ = page;
        group_heading(ui, "Scans");
        let mut ask = !self.suppress_stop_confirm;
        if setting_switch(ui, "Ask before stopping a scan", None, &mut ask) {
            self.ask_before_stopping(ask);
        }
        let mut n = self.list_length();
        if setting_number(ui, "Items in the largest files and folders lists", None, &mut n, MIN_LIST..=MAX_LIST, "items") {
            self.list_len = Some(n);
        }
        group_heading(ui, "History");
        let mut limited = self.keep_scans.is_some();
        if setting_switch(ui, "Keep only the latest scans", Some("Older scans are removed when the next one is saved"), &mut limited) {
            self.keep_scans = limited.then_some(KEEP_SCANS);
        }
        let mut keep = self.keep_scans.unwrap_or(KEEP_SCANS);
        let changed = ui.add_enabled_ui(limited, |ui| sub_setting_number(ui, "Scans to keep in History", None, &mut keep, 1..=1000, "scans")).inner;
        if changed && limited {
            self.keep_scans = Some(keep);
        }
        let shown = self.history_folder.clone().unwrap_or_else(history::default_folder);
        match setting_folder(ui, "History folder", None, &shown, Some(("Reset to default", self.history_folder.is_some())), true) {
            FolderAct::Choose => {
                if let Some(dir) = self.context.as_ref().and_then(|c| c.pick_folder("Choose the folder for the scan history", "Choose")) {
                    self.set_history_folder(Some(dir));
                }
            }
            FolderAct::Reset => self.set_history_folder(None),
            FolderAct::None => {}
        }
        group_heading(ui, "Summary");
        setting_switch(ui, "Show a tile on Summary", Some("Free space and the last scan"), &mut self.summary_tile);
    }

    fn on_sample(&mut self, sample: &Snapshot) {
        self.run_pending(&sample.disks);
    }

    /// Stops the running scan and forgets every result.
    fn stop(&mut self) {
        self.pending = None;
        let mut s = lock_state(&self.state);
        self.generation.bump();
        *s = ScanState::default();
        self.confirm = None;
        self.clear_asked = false;
        self.shown_for = None;
    }
}

/// The app's history folder (never in tests: they use temp folders).
#[cfg(not(test))]
fn app_history() -> Option<Result<history::Store, String>> {
    Some(history::Store::app())
}

#[cfg(test)]
fn app_history() -> Option<Result<history::Store, String>> {
    None
}

/// The Summary tile's lines: each drive's free space, then the last scan.
pub fn tile(disks: &[DiskRow], last: Option<&history::Summary>, today: i64) -> Vec<String> {
    let mut lines: Vec<String> = disks.iter().map(|d| format!("{}   {} free of {}", d.mount, text::size(d.free), text::size(d.total))).collect();
    if lines.is_empty() {
        lines.push("Reading drives\u{2026}".into());
    }
    lines.push(match last {
        Some(e) => {
            let done = Done::at(e.finished, 0);
            format!("Last scan: {} {} at {}", e.root_path, text::day_word(done.day, today), done.at)
        }
        None => "No scan yet".into(),
    });
    lines
}

/// The item a piece stands for, in the level shown (see [`actions::item`]).
fn item_of(state: &Mutex<ScanState>, key: &nav::Key) -> Option<PathBuf> {
    let s = lock_state(state);
    let t = s.tree.as_ref()?;
    actions::item(key, &t.path(s.nav.level(t)))
}

#[cfg(test)]
mod tests;
