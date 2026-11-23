//! The Processes page's view: the snapshot's rows plus the view options in,
//! the ordered lines to draw out (group headers and process rows, with their
//! tree depth, chevron and the sums a tree parent shows). The page only
//! draws what `view` returns.
//!
//! Later slices add to it without reshaping it: exited rows and statuses
//! keep their state in `ViewState`; smoothing, highlights, values or
//! percents, hiding the app itself or Microsoft's processes, selections and
//! the services under a host process add fields to `ViewOptions`,
//! `ViewState` or `ProcessLine`.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::ProcCol;
use crate::sampler::{ProcRow, ServiceInfo};
use crate::settings::Settings;

/// The two groups of the table, Apps first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Group {
    /// Processes that own a visible top-level window.
    Apps,
    /// Every other process.
    Background,
}

impl Group {
    /// Both groups, in the order the table shows them.
    pub const ALL: [Group; 2] = [Group::Apps, Group::Background];

    /// The group's header: "Apps (15)", "Background processes (49)".
    pub fn title(self, count: usize) -> String {
        match self {
            Group::Apps => format!("Apps ({count})"),
            Group::Background => format!("Background processes ({count})"),
        }
    }
}

/// What the user chose for the view; the page builds it each frame.
#[derive(Clone, Debug)]
pub struct ViewOptions {
    /// Process tree on: children under their parent. Off: a flat list.
    pub tree: bool,
    /// The filter box's text: name, user or command contain it, or it is
    /// the PID.
    pub filter: String,
    /// The column sorted by and whether the largest comes first.
    pub sort: (ProcCol, bool),
    /// The add-ons' column figures, for sorting by an add-on's column.
    pub figures: Figures,
    /// What the Publisher and Description columns show, for sorting by them.
    pub files: FileTexts,
    /// The processes Settings keeps out of the table.
    pub hide: Hide,
    /// Show exited processes: processes that ended stay a short while.
    pub show_exited: bool,
    /// Smooth CPU readings: each process's CPU is a moving average of its
    /// readings instead of the last one.
    pub smooth: bool,
    /// Highlight Process Updates: a row whose figures changed is lit for
    /// [`LIT_FOR`]; off, no row is lit.
    pub highlight: bool,
    /// The time of this frame, to tell which rows are still lit.
    pub now: Instant,
    /// The services each host process runs, by the host's PID: in the
    /// tree they are its first children.
    pub services: Services,
}

/// The running services by the PID of the process that hosts them.
pub type Services = Arc<HashMap<u32, Vec<ServiceInfo>>>;

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            tree: true,
            filter: String::new(),
            sort: (ProcCol::Mem, true),
            figures: Figures::new(),
            files: FileTexts::default(),
            hide: Hide::default(),
            show_exited: true,
            smooth: false,
            highlight: true,
            now: Instant::now(),
            services: Services::default(),
        }
    }
}

/// Why Settings keeps a process out of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hidden {
    /// The app itself, while "Show Task Manager in Reports" is off.
    Own,
    /// Its program file carries a verified Microsoft Windows signature and
    /// "Hide Microsoft Services and Processes" is on.
    Microsoft,
}

/// The processes Settings keeps out of the table. The default hides none.
#[derive(Clone, Debug, Default)]
pub struct Hide {
    /// The app's own PID, while "Show Task Manager in Reports" is off.
    pub own_pid: Option<u32>,
    /// While "Hide Microsoft Services and Processes" is on: the program
    /// files known to carry a verified Microsoft Windows signature (a file
    /// not checked yet is shown).
    pub microsoft: Option<Arc<HashSet<String>>>,
}

impl Hide {
    /// What `settings` hide: the app (`own_pid`) unless it is shown in
    /// reports, and the `microsoft` program files while that option is on.
    pub fn new(settings: &Settings, own_pid: u32, microsoft: Arc<HashSet<String>>) -> Self {
        Hide { own_pid: (!settings.show_self_in_reports).then_some(own_pid), microsoft: settings.hide_microsoft.then_some(microsoft) }
    }

    /// Why `p` is kept out of the table, if it is.
    pub fn why(&self, p: &ProcRow) -> Option<Hidden> {
        if self.own_pid == Some(p.pid) {
            Some(Hidden::Own)
        } else if self.microsoft.as_ref().is_some_and(|m| m.contains(&p.exe)) {
            Some(Hidden::Microsoft)
        } else {
            None
        }
    }
}

/// The status of a process that has ended and is kept for a while.
pub const EXITED: &str = "Exited";

/// How long an exited process stays in the table (Show exited processes).
pub const EXITED_FOR: Duration = Duration::from_secs(3);

/// How long a row stays lit after its figures changed (Highlight Process
/// Updates).
pub const LIT_FOR: Duration = Duration::from_secs(1);

/// The weight of a new CPU reading in the smoothed figure (Smooth CPU
/// readings): the rest is the figure so far.
pub const SMOOTHING: f32 = 0.3;

/// What the page keeps between frames: which tree rows are open, which
/// groups are folded, and the processes that have just ended.
#[derive(Clone, Debug, Default)]
pub struct ViewState {
    /// PIDs of the tree parents whose children show.
    pub expanded: HashSet<u32>,
    /// Groups whose rows are hidden under their header.
    pub folded: HashSet<Group>,
    /// The letters typed to jump to a row.
    pub type_ahead: TypeAhead,
    /// The rows of the last sample, by PID, to notice which ones end.
    last: HashMap<u32, ProcRow>,
    /// Processes that have ended, as last seen (status "Exited"), and
    /// when their end was noticed.
    exited: HashMap<u32, (ProcRow, Instant)>,
    /// Each live process's smoothed CPU (Smooth CPU readings).
    smoothed: HashMap<u32, f32>,
    /// When each process's CPU or Memory last changed, to light its row.
    lit: HashMap<u32, Instant>,
}

/// A tree row's chevron.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chevron {
    /// No children: no chevron.
    Leaf,
    /// Children hidden (pointing right).
    Closed,
    /// Children shown (pointing down).
    Open,
}

/// A group header line.
#[derive(Clone, Debug, PartialEq)]
pub struct GroupLine {
    pub group: Group,
    /// The processes in the group, those inside closed trees included.
    pub count: usize,
    pub folded: bool,
}

/// A process line.
#[derive(Clone, Debug)]
pub struct ProcessLine {
    /// The process; a tree parent carries its tree's sums in Memory, Disk
    /// read, Disk write, Threads and GPU.
    pub row: ProcRow,
    /// 0 for a top row, 1 for its children, and so on.
    pub depth: usize,
    pub chevron: Chevron,
    /// The process has ended: drawn tinted red, its GPU "Unavailable".
    pub exited: bool,
    /// Its CPU or Memory changed less than [`LIT_FOR`] ago and Highlight
    /// Process Updates is on: its CPU cell is lit.
    pub lit: bool,
}

impl ProcessLine {
    /// Whether the line shows a value in `col`: a tree parent reads as a
    /// total and leaves the per-process fields (User name, Peak memory,
    /// CPU time, Page faults, Handles, Priority) empty; a service under its
    /// host has no figures of its own (only Name, PID, Status, User name).
    pub fn shows(&self, col: ProcCol) -> bool {
        if self.row.service.is_some() {
            return matches!(col, ProcCol::Name | ProcCol::Pid | ProcCol::Status | ProcCol::User);
        }
        self.chevron == Chevron::Leaf
            || !matches!(col, ProcCol::User | ProcCol::PeakMem | ProcCol::CpuTime | ProcCol::PageFaults | ProcCol::Handles | ProcCol::Priority)
    }
}

/// One line of the table.
// Almost every line is a process: boxing them would only add allocations.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Line {
    Group(GroupLine),
    Process(ProcessLine),
}

/// The lines to draw and how many processes the filter lets through.
#[derive(Clone, Debug, Default)]
pub struct View {
    pub lines: Vec<Line>,
    /// Processes that match the filter, shown or inside closed trees.
    pub shown: usize,
}

impl View {
    /// The process lines, in order.
    pub fn processes(&self) -> impl Iterator<Item = &ProcessLine> {
        self.lines.iter().filter_map(|l| match l {
            Line::Process(p) => Some(p),
            Line::Group(_) => None,
        })
    }

    /// The line of the process `pid`, if it is drawn.
    pub fn line_of(&self, pid: u32) -> Option<usize> {
        self.lines.iter().position(|l| matches!(l, Line::Process(p) if p.row.pid == pid))
    }
}

/// The figures of the add-ons' columns drawn now: by add-on id, each
/// process's figure by PID. An add-on without a reading yet has none.
pub type Figures = std::collections::HashMap<&'static str, std::collections::HashMap<u32, usize>>;

/// A process's figure in an add-on's column: None while the add-on has no
/// reading; 0 for a process it has nothing for.
pub fn figure_of(figures: &Figures, addon: &str, pid: u32) -> Option<usize> {
    figures.get(addon).map(|by_pid| by_pid.get(&pid).copied().unwrap_or(0))
}

/// What the columns read from a process's program file show (Publisher,
/// Description; marked additions), by the file's path. Filled for the
/// columns drawn, like [`Figures`]; a path not read yet shows nothing.
#[derive(Clone, Debug, Default)]
pub struct FileTexts {
    /// The verified signer, or "Not verified".
    pub publisher: HashMap<String, String>,
    /// The file's version-details description.
    pub description: HashMap<String, String>,
}

impl FileTexts {
    /// What column `col` shows for the program file `exe` (empty for a
    /// column that does not read the file).
    pub fn of(&self, col: ProcCol, exe: &str) -> &str {
        let by_path = match col {
            ProcCol::Publisher => &self.publisher,
            ProcCol::Description => &self.description,
            _ => return "",
        };
        by_path.get(exe).map_or("", String::as_str)
    }
}

/// How two processes compare in a column, smallest first (an add-on's
/// column by its figures, no figure lowest; a file's column by its text,
/// case aside).
pub fn compare(col: ProcCol, a: &ProcRow, b: &ProcRow, figures: &Figures, files: &FileTexts) -> Ordering {
    use std::cmp::Ordering::Equal;
    // Priorities compare by rank, Low below Realtime; unknown lowest.
    let rank = |p: &ProcRow| crate::process_actions::PRIORITIES.iter().rev().position(|c| *c == p.priority);
    match col {
        ProcCol::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        ProcCol::Pid => a.pid.cmp(&b.pid),
        ProcCol::Status => a.status.cmp(&b.status),
        ProcCol::User => a.user.cmp(&b.user),
        ProcCol::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(Equal),
        ProcCol::Mem => a.mem.cmp(&b.mem),
        ProcCol::DiskRead => a.disk_read.partial_cmp(&b.disk_read).unwrap_or(Equal),
        ProcCol::DiskWrite => a.disk_write.partial_cmp(&b.disk_write).unwrap_or(Equal),
        ProcCol::Threads => a.threads.cmp(&b.threads),
        ProcCol::Gpu => a.gpu.partial_cmp(&b.gpu).unwrap_or(Equal),
        ProcCol::PeakMem => a.peak_mem.cmp(&b.peak_mem),
        ProcCol::CpuTime => a.cpu_time.cmp(&b.cpu_time),
        ProcCol::PageFaults => a.page_faults.cmp(&b.page_faults),
        ProcCol::Handles => a.handles.cmp(&b.handles),
        ProcCol::Priority => rank(a).cmp(&rank(b)),
        ProcCol::Arch => a.arch.cmp(b.arch),
        ProcCol::Cmd => a.cmd.cmp(&b.cmd),
        ProcCol::Publisher | ProcCol::Description => files.of(col, &a.exe).to_lowercase().cmp(&files.of(col, &b.exe).to_lowercase()),
        ProcCol::Addon(id) => figure_of(figures, id, a.pid).cmp(&figure_of(figures, id, b.pid)),
    }
}

/// The machine's totals a process's figures are a share of (Resource
/// values ▸ Percents).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Totals {
    /// Physical memory, bytes.
    pub mem: u64,
    /// The machine's disk read and write rates, bytes per second.
    pub disk_read: f64,
    pub disk_write: f64,
}

/// A process's figure in `col` as a percent of the machine's: Memory and
/// Peak memory of the physical memory, Disk read and Disk write of the
/// machine's rate (0 while the disk is idle); CPU and GPU already are.
/// None for the other columns.
pub fn percent(col: ProcCol, row: &ProcRow, totals: &Totals) -> Option<f32> {
    let share = |part: f64, whole: f64| if whole > 0.0 { (part / whole * 100.0) as f32 } else { 0.0 };
    match col {
        ProcCol::Cpu => Some(row.cpu),
        ProcCol::Gpu => Some(row.gpu),
        ProcCol::Mem => Some(share(row.mem as f64, totals.mem as f64)),
        ProcCol::PeakMem => Some(share(row.peak_mem as f64, totals.mem as f64)),
        ProcCol::DiskRead => Some(share(row.disk_read, totals.disk_read)),
        ProcCol::DiskWrite => Some(share(row.disk_write, totals.disk_write)),
        _ => None,
    }
}

/// The group of a process by its own window: a tree goes to the group of
/// its top process.
fn group_of(p: &ProcRow) -> Group {
    if p.app {
        Group::Apps
    } else {
        Group::Background
    }
}

/// Whether the filter box's text (trimmed, lower case) lets `p` through:
/// its name, user or command contain it, or it is the PID.
fn matches(p: &ProcRow, q: &str) -> bool {
    q.is_empty() || p.name.to_lowercase().contains(q) || p.pid.to_string() == q || p.user.to_lowercase().contains(q) || p.cmd.to_lowercase().contains(q)
}

/// The parent of each row among `rows`, by index. A parent must be listed
/// and at least as old as its child: a younger process with the parent's
/// PID reused it after the real parent ended.
fn parents(rows: &[&ProcRow]) -> Vec<Option<usize>> {
    let at: HashMap<u32, usize> = rows.iter().enumerate().map(|(i, p)| (p.pid, i)).collect();
    rows.iter()
        .map(|p| {
            let i = *at.get(&p.parent?)?;
            (rows[i].pid != p.pid && rows[i].run_time >= p.run_time).then_some(i)
        })
        .collect()
}

/// The rows as a forest: the top rows and each row's children, by index.
/// A loop of parents (possible only with equal start times) is cut where
/// it is first met.
fn forest(rows: &[&ProcRow]) -> (Vec<usize>, Vec<Vec<usize>>) {
    let parent = parents(rows);
    let mut children = vec![Vec::new(); rows.len()];
    let mut tops = Vec::new();
    for (i, p) in parent.iter().enumerate() {
        match p {
            Some(p) => children[*p].push(i),
            None => tops.push(i),
        }
    }
    // Rows not reached from a top row sit on a loop: the first of each
    // becomes a top row and the link into it is dropped.
    let mut reached = vec![false; rows.len()];
    let mut stack = tops.clone();
    let mut next = 0;
    loop {
        while let Some(i) = stack.pop() {
            if !std::mem::replace(&mut reached[i], true) {
                stack.extend(children[i].iter().copied());
            }
        }
        while next < rows.len() && reached[next] {
            next += 1;
        }
        if next == rows.len() {
            break;
        }
        if let Some(p) = parent[next] {
            children[p].retain(|&c| c != next);
        }
        tops.push(next);
        stack.push(next);
    }
    (tops, children)
}

/// The row of the service `s` under its host `host`: the host's PID and
/// user, named "Service: Winmgmt (Windows Management Instrumentation)".
fn service_row(host: &ProcRow, s: &ServiceInfo) -> ProcRow {
    ProcRow {
        pid: host.pid,
        parent: Some(host.pid),
        name: format!("Service: {} ({})", s.name, s.display_name),
        status: "Running".into(),
        user: host.user.clone(),
        app: host.app,
        service: Some(s.clone()),
        ..Default::default()
    }
}

/// The lines of the table for `rows` under `opts` and `state`.
pub fn view(rows: &[ProcRow], opts: &ViewOptions, state: &ViewState) -> View {
    let q = opts.filter.trim().to_lowercase();
    // The processes that ended a moment ago join the live ones (a PID
    // listed again belongs to a new process).
    let live: HashSet<u32> = rows.iter().map(|p| p.pid).collect();
    let mut gone: Vec<&ProcRow> = if opts.show_exited { state.exited.values().map(|(r, _)| r).filter(|r| !live.contains(&r.pid)).collect() } else { Vec::new() };
    gone.sort_by_key(|r| r.pid);
    // Rows Settings hides go, as if they did not run.
    let kept: Vec<&ProcRow> = rows.iter().chain(gone).filter(|p| opts.hide.why(p).is_none() && matches(p, &q)).collect();
    let (tops, children) = if opts.tree { forest(&kept) } else { ((0..kept.len()).collect(), vec![Vec::new(); kept.len()]) };
    // Each row as it is drawn: a parent carries its tree's sums.
    let mut shown: Vec<Option<ProcRow>> = vec![None; kept.len()];
    fn total(i: usize, kept: &[&ProcRow], children: &[Vec<usize>], shown: &mut [Option<ProcRow>]) {
        let mut r = kept[i].clone();
        for &c in &children[i] {
            total(c, kept, children, shown);
            let child = shown[c].as_ref().expect("summed above");
            r.mem += child.mem;
            r.disk_read += child.disk_read;
            r.disk_write += child.disk_write;
            r.threads += child.threads;
            // GPU is capped at 100%, a tree's sum too (owner decision).
            r.gpu = (r.gpu + child.gpu).min(100.0);
        }
        shown[i] = Some(r);
    }
    for &t in &tops {
        total(t, &kept, &children, &mut shown);
    }
    let mut shown: Vec<ProcRow> = shown.into_iter().map(|r| r.expect("every row is in a tree")).collect();
    // Smooth CPU readings: a live process's CPU is its moving average (a
    // tree parent shows its own CPU, not a sum, so this comes after).
    if opts.smooth {
        for r in shown.iter_mut().filter(|r| live.contains(&r.pid)) {
            if let Some(cpu) = state.smoothed.get(&r.pid) {
                r.cpu = *cpu;
            }
        }
    }
    let (col, desc) = opts.sort;
    let order = |list: &mut Vec<usize>| {
        list.sort_by(|&a, &b| {
            let o = compare(col, &shown[a], &shown[b], &opts.figures, &opts.files);
            let o = if desc { o.reverse() } else { o };
            o.then(shown[a].pid.cmp(&shown[b].pid))
        })
    };
    let mut children = children;
    children.iter_mut().for_each(&order);
    let mut tops = tops;
    order(&mut tops);
    let size = |t: usize| -> usize {
        let mut n = 0;
        let mut stack = vec![t];
        while let Some(i) = stack.pop() {
            n += 1;
            stack.extend(children[i].iter().copied());
        }
        n
    };
    let mut lines = Vec::new();
    for group in Group::ALL {
        let mine: Vec<usize> = tops.iter().copied().filter(|&t| group_of(&shown[t]) == group).collect();
        if mine.is_empty() {
            continue;
        }
        let folded = state.folded.contains(&group);
        lines.push(Line::Group(GroupLine { group, count: mine.iter().map(|&t| size(t)).sum(), folded }));
        if folded {
            continue;
        }
        // Depth first, children in their sorted order.
        let mut stack: Vec<(usize, usize)> = mine.iter().rev().map(|&t| (t, 0)).collect();
        while let Some((i, depth)) = stack.pop() {
            let open = state.expanded.contains(&shown[i].pid);
            // In the tree, a live host's services are its first children.
            let services: &[ServiceInfo] = match opts.services.get(&shown[i].pid) {
                Some(s) if opts.tree && live.contains(&shown[i].pid) => s,
                _ => &[],
            };
            let chevron = match (children[i].is_empty() && services.is_empty(), open) {
                (true, _) => Chevron::Leaf,
                (false, false) => Chevron::Closed,
                (false, true) => Chevron::Open,
            };
            if chevron == Chevron::Open {
                stack.extend(children[i].iter().rev().map(|&c| (c, depth + 1)));
            }
            let exited = !live.contains(&shown[i].pid);
            let lit = opts.highlight && !exited && state.lit.get(&shown[i].pid).is_some_and(|at| opts.now.saturating_duration_since(*at) < LIT_FOR);
            lines.push(Line::Process(ProcessLine { row: shown[i].clone(), depth, chevron, exited, lit }));
            if chevron == Chevron::Open {
                let mut services: Vec<&ServiceInfo> = services.iter().collect();
                services.sort_by_key(|s| s.name.to_lowercase());
                for s in services {
                    let row = service_row(&shown[i], s);
                    lines.push(Line::Process(ProcessLine { row, depth: depth + 1, chevron: Chevron::Leaf, exited: false, lit: false }));
                }
            }
        }
    }
    View { lines, shown: kept.len() }
}

impl ViewState {
    /// Takes a new sample's `rows`, seen at `now`: a process of the last
    /// sample that is not listed any more has ended and is kept for
    /// [`EXITED_FOR`] when `keep` (Show exited processes) is on; with it
    /// off, none is kept. It also moves each process's smoothed CPU
    /// ([`SMOOTHING`]) and notes the rows whose CPU (by 0.1%) or Memory (by
    /// 0.1 MB) changed, to light them. The sampler keeps nothing: the view
    /// does.
    pub fn track(&mut self, rows: &[ProcRow], now: Instant, keep: bool) {
        let live: HashMap<u32, ProcRow> = rows.iter().map(|p| (p.pid, p.clone())).collect();
        self.smoothed = rows
            .iter()
            .map(|p| (p.pid, self.smoothed.get(&p.pid).map_or(p.cpu, |old| old + (p.cpu - old) * SMOOTHING)))
            .collect();
        for p in rows {
            let changed = self.last.get(&p.pid).is_some_and(|old| (p.cpu - old.cpu).abs() >= 0.1 || p.mem.abs_diff(old.mem) >= 1_048_576 / 10);
            if changed {
                self.lit.insert(p.pid, now);
            }
        }
        self.lit.retain(|pid, at| live.contains_key(pid) && now.saturating_duration_since(*at) < LIT_FOR);
        if keep {
            for (pid, row) in &self.last {
                if !live.contains_key(pid) && !self.exited.contains_key(pid) {
                    let gone = ProcRow { status: EXITED.into(), cpu: 0.0, gpu: 0.0, disk_read: 0.0, disk_write: 0.0, ..row.clone() };
                    self.exited.insert(*pid, (gone, now));
                }
            }
            self.exited.retain(|pid, (_, at)| !live.contains_key(pid) && now.saturating_duration_since(*at) < EXITED_FOR);
        } else {
            self.exited.clear();
        }
        self.last = live;
    }

    /// The processes that have ended and are still kept, as last seen.
    pub fn exited_rows(&self) -> impl Iterator<Item = &ProcRow> {
        self.exited.values().map(|(r, _)| r)
    }

    /// Forgets every exited process (Show exited processes turned off).
    pub fn drop_exited(&mut self) {
        self.exited.clear();
    }

    /// Opens (`open`) or closes the tree row of `pid`: Right and Left on
    /// the selected row. Rows without children are left alone.
    pub fn open(&mut self, view: &View, pid: u32, open: bool) {
        let parent = view.processes().any(|p| p.row.pid == pid && p.chevron != Chevron::Leaf);
        if !parent {
            return;
        }
        if open {
            self.expanded.insert(pid);
        } else {
            self.expanded.remove(&pid);
        }
    }

    /// Folds or unfolds a group (a double-click on its header).
    pub fn toggle_group(&mut self, group: Group) {
        if !self.folded.remove(&group) {
            self.folded.insert(group);
        }
    }

    /// Expand all: opens every tree parent among `rows`.
    pub fn expand_all(&mut self, rows: &[ProcRow]) {
        let all: Vec<&ProcRow> = rows.iter().collect();
        let (_, children) = forest(&all);
        self.expanded.extend(all.iter().zip(&children).filter(|(_, c)| !c.is_empty()).map(|(p, _)| p.pid));
    }

    /// Collapse all: closes every tree row.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Makes `pid` visible: opens its ancestors and unfolds its group (Go
    /// to the process from another page).
    pub fn reveal(&mut self, rows: &[ProcRow], pid: u32) {
        let all: Vec<&ProcRow> = rows.iter().collect();
        let parent = parents(&all);
        let Some(mut i) = all.iter().position(|p| p.pid == pid) else { return };
        let mut seen = HashSet::new();
        while let Some(p) = parent[i] {
            if !seen.insert(p) {
                break;
            }
            self.expanded.insert(all[p].pid);
            i = p;
        }
        // Its tree's group, or its own in a flat list.
        self.folded.remove(&group_of(all[i]));
        self.folded.remove(&group_of(all.iter().find(|p| p.pid == pid).expect("found above")));
    }
}

/// How a click on a process row changes the selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// A plain click: only this row.
    One,
    /// Ctrl+click: this row in or out, the others kept.
    Toggle,
    /// Shift+click: the drawn process rows from the last row clicked to
    /// this one (group headers are never selected).
    Range,
}

impl Pick {
    /// The pick the held keys make: Shift ranges, Ctrl toggles.
    pub fn of(ctrl: bool, shift: bool) -> Pick {
        match (ctrl, shift) {
            (_, true) => Pick::Range,
            (true, false) => Pick::Toggle,
            _ => Pick::One,
        }
    }
}

/// The selected process rows: their PIDs and the row a Shift+click ranges
/// from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Selection {
    pids: std::collections::BTreeSet<u32>,
    anchor: Option<u32>,
    /// Set when the row selected is a service under its host (the host's
    /// PID is then the one in `pids`).
    service: Option<String>,
}

impl Selection {
    /// Only the process `pid`.
    pub fn only(pid: u32) -> Selection {
        Selection { pids: [pid].into(), anchor: Some(pid), service: None }
    }

    /// Only the service row `row` (a line [`view`] puts under its host).
    /// No process counts as selected then: nothing ends its host.
    pub fn service(row: &ProcRow) -> Selection {
        Selection { pids: [row.pid].into(), anchor: Some(row.pid), service: row.service.as_ref().map(|s| s.name.clone()) }
    }

    /// Whether the line of `row` is selected: a process by its PID, a
    /// service by its host and name.
    pub fn has(&self, row: &ProcRow) -> bool {
        self.service.as_deref() == row.service.as_ref().map(|s| s.name.as_str()) && self.pids.contains(&row.pid)
    }

    /// The selected service's line in `view`, when a service row is
    /// selected.
    pub fn service_line<'a>(&self, view: &'a View) -> Option<&'a ProcessLine> {
        self.service.as_ref()?;
        view.processes().find(|l| self.has(&l.row))
    }

    /// A click on the row of `pid` in `view`.
    pub fn click(&mut self, view: &View, pid: u32, pick: Pick) {
        // From a service row, any click on a process starts over.
        let pick = if self.service.is_some() { Pick::One } else { pick };
        match pick {
            Pick::One => *self = Selection::only(pid),
            Pick::Toggle => {
                if !self.pids.remove(&pid) {
                    self.pids.insert(pid);
                }
                self.anchor = Some(pid);
            }
            Pick::Range => {
                let from = self.anchor.and_then(|a| view.line_of(a));
                let (Some(a), Some(b)) = (from, view.line_of(pid)) else {
                    *self = Selection::only(pid);
                    return;
                };
                self.pids = view.lines[a.min(b)..=a.max(b)]
                    .iter()
                    .filter_map(|l| match l {
                        Line::Process(p) => Some(p.row.pid),
                        Line::Group(_) => None,
                    })
                    .collect();
            }
        }
    }

    /// The selected PIDs, smallest first.
    #[cfg(test)]
    pub fn pids(&self) -> impl Iterator<Item = u32> + '_ {
        self.pids.iter().copied()
    }

    /// Whether the process `pid` is selected (a service row selected is
    /// no process).
    pub fn contains(&self, pid: u32) -> bool {
        self.service.is_none() && self.pids.contains(&pid)
    }

    /// The PID when exactly one process row is selected.
    pub fn single(&self) -> Option<u32> {
        if self.service.is_some() {
            return None;
        }
        match self.pids.len() {
            1 => self.pids.first().copied(),
            _ => None,
        }
    }
}

/// Follow process: the process kept selected and in view, and the line
/// of the table it was last drawn on.
#[derive(Clone, Debug, PartialEq)]
pub struct Follow {
    pub pid: u32,
    line: Option<usize>,
}

impl Follow {
    pub fn new(pid: u32) -> Follow {
        Follow { pid, line: None }
    }

    /// Whether the followed row is drawn on another line of `view` than
    /// at the last call (a new snapshot or sort moved it, or it is drawn
    /// for the first time): the table then scrolls to it.
    pub fn moved(&mut self, view: &View) -> bool {
        let now = view.line_of(self.pid);
        let moved = now.is_some() && now != self.line;
        self.line = now;
        moved
    }
}

/// How long typed letters keep adding to the name searched for.
pub const TYPE_AHEAD_SECONDS: f64 = 2.0;

/// Type-ahead on the table: letters typed within two seconds of each other
/// spell the start of a process name.
#[derive(Clone, Debug, Default)]
pub struct TypeAhead {
    typed: String,
    last: Option<f64>,
}

impl TypeAhead {
    /// `text` typed at `now` (seconds): the PID of the first drawn process
    /// whose name starts with what was typed, ignoring case.
    pub fn key(&mut self, view: &View, text: &str, now: f64) -> Option<u32> {
        if self.last.is_none_or(|t| now - t > TYPE_AHEAD_SECONDS) {
            self.typed.clear();
        }
        self.last = Some(now);
        self.typed.push_str(&text.to_lowercase());
        view.processes().filter(|p| p.row.service.is_none()).find(|p| p.row.name.to_lowercase().starts_with(&self.typed)).map(|p| p.row.pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(pid: u32, parent: Option<u32>, name: &str, mem: u64) -> ProcRow {
        ProcRow { pid, parent, name: name.into(), user: "spata".into(), mem, ..Default::default() }
    }

    fn app(r: ProcRow) -> ProcRow {
        ProcRow { app: true, ..r }
    }

    /// The drawn lines as words: "# Apps (2)" for a header, then
    /// indentation, chevron and name for each process.
    fn words(v: &View) -> Vec<String> {
        v.lines
            .iter()
            .map(|l| match l {
                Line::Group(g) => format!("# {}", g.group.title(g.count)),
                Line::Process(p) => {
                    let mark = match p.chevron {
                        Chevron::Leaf => "",
                        Chevron::Closed => "> ",
                        Chevron::Open => "v ",
                    };
                    format!("{}{mark}{}", "  ".repeat(p.depth), p.row.name)
                }
            })
            .collect()
    }

    /// Steam (an app) with two children, one with a child of its own, and
    /// two background processes.
    fn family() -> Vec<ProcRow> {
        vec![
            app(row(100, Some(1), "steam.exe", 300)),
            row(110, Some(100), "steamwebhelper.exe", 200),
            row(111, Some(110), "gpu-helper.exe", 50),
            row(120, Some(100), "steamservice.exe", 100),
            row(200, None, "svchost.exe", 400),
            row(300, Some(200), "dllhost.exe", 10),
        ]
    }

    fn open_all() -> ViewState {
        let mut s = ViewState::default();
        s.expand_all(&family());
        s
    }

    fn pids(s: &Selection) -> Vec<u32> {
        s.pids().collect()
    }

    #[test]
    fn a_click_selects_one_row_ctrl_toggles_and_shift_ranges_over_the_shown_rows() {
        let v = view(&family(), &ViewOptions::default(), &open_all());
        let mut s = Selection::default();
        s.click(&v, 110, Pick::One);
        assert_eq!(pids(&s), [110]);
        assert_eq!(s.single(), Some(110));
        s.click(&v, 200, Pick::Toggle);
        s.click(&v, 300, Pick::Toggle);
        assert_eq!(pids(&s), [110, 200, 300], "Ctrl adds");
        s.click(&v, 200, Pick::Toggle);
        assert_eq!(pids(&s), [110, 300], "Ctrl on a selected row removes it");
        assert_eq!(s.single(), None);
        // Shift: from the last row clicked (svchost, 200) up to
        // steamservice, the Background header skipped.
        s.click(&v, 120, Pick::Range);
        assert_eq!(pids(&s), [120, 200]);
        // Shift again ranges from the same anchor.
        s.click(&v, 111, Pick::Range);
        assert_eq!(pids(&s), [111, 120, 200]);
        s.click(&v, 100, Pick::One);
        assert_eq!(pids(&s), [100], "a plain click starts over");
    }

    #[test]
    fn shift_ranges_over_the_drawn_lines_only() {
        // steam closed: its children are not drawn and not selected.
        let v = view(&family(), &ViewOptions::default(), &ViewState::default());
        let mut s = Selection::only(100);
        s.click(&v, 200, Pick::Range);
        assert_eq!(pids(&s), [100, 200], "the closed rows' children are left out");
        let open = view(&family(), &ViewOptions::default(), &open_all());
        s.click(&open, 300, Pick::Range);
        assert_eq!(pids(&s), [100, 110, 111, 120, 200, 300]);
    }

    #[test]
    fn type_ahead_jumps_to_the_first_drawn_name_and_starts_over_after_two_seconds() {
        let v = view(&family(), &ViewOptions::default(), &open_all());
        let mut t = TypeAhead::default();
        assert_eq!(t.key(&v, "s", 10.0), Some(100), "steam.exe, the first S");
        assert_eq!(t.key(&v, "T", 10.5), Some(100));
        assert_eq!(t.key(&v, "eamS", 11.0), Some(120), "steamservice.exe");
        assert_eq!(t.key(&v, "x", 11.5), None, "no name starts with steamsx");
        assert_eq!(t.key(&v, "d", 13.6), Some(300), "after 2 s a key starts a new name: dllhost.exe");
        assert_eq!(t.key(&v, "zz", 20.0), None);
    }

    #[test]
    fn publisher_and_description_sort_by_their_text_with_nothing_read_first() {
        let rows: Vec<ProcRow> = ["a", "b", "c"]
            .iter()
            .enumerate()
            .map(|(i, n)| ProcRow { pid: i as u32 + 1, name: format!("{n}.exe"), exe: format!(r"C:\{n}.exe"), ..Default::default() })
            .collect();
        let mut files = FileTexts::default();
        files.publisher.insert(r"C:\a.exe".into(), "zeta Ltd".into());
        files.publisher.insert(r"C:\b.exe".into(), "Alpha Inc".into());
        files.description.insert(r"C:\a.exe".into(), "Alpha tool".into());
        files.description.insert(r"C:\c.exe".into(), "beta tool".into());
        let order = |col| {
            let opts = ViewOptions { tree: false, sort: (col, false), files: files.clone(), ..Default::default() };
            view(&rows, &opts, &ViewState::default()).processes().map(|l| l.row.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(order(ProcCol::Publisher), ["c.exe", "b.exe", "a.exe"]);
        assert_eq!(order(ProcCol::Description), ["b.exe", "a.exe", "c.exe"]);
    }

    #[test]
    fn the_tree_puts_children_under_their_parent_one_level_deeper() {
        let v = view(&family(), &ViewOptions::default(), &open_all());
        assert_eq!(
            words(&v),
            [
                "# Apps (4)",
                "v steam.exe",
                "  v steamwebhelper.exe",
                "    gpu-helper.exe",
                "  steamservice.exe",
                "# Background processes (2)",
                "v svchost.exe",
                "  dllhost.exe",
            ]
        );
    }

    #[test]
    fn a_closed_parent_hides_its_children_behind_a_right_chevron() {
        let v = view(&family(), &ViewOptions::default(), &ViewState::default());
        assert_eq!(words(&v), ["# Apps (4)", "> steam.exe", "# Background processes (2)", "> svchost.exe"]);
        assert_eq!(v.shown, 6, "closed trees still count");
    }

    #[test]
    fn a_parent_row_shows_its_trees_sums_and_leaves_the_per_process_fields_empty() {
        let mut rows = family();
        for (i, r) in rows.iter_mut().enumerate() {
            r.disk_read = 10.0 * (i + 1) as f64;
            r.disk_write = 1.0;
            r.threads = 2;
            r.cpu = 1.5;
            r.handles = 99;
        }
        rows[1].gpu = 12.5;
        rows[2].gpu = 3.0;
        let v = view(&rows, &ViewOptions::default(), &open_all());
        let steam = v.processes().find(|p| p.row.pid == 100).unwrap();
        assert_eq!(steam.row.mem, 650, "300 + 200 + 50 + 100");
        assert_eq!(steam.row.disk_read, 100.0, "10 + 20 + 30 + 40");
        assert_eq!(steam.row.disk_write, 4.0);
        assert_eq!(steam.row.threads, 8);
        assert_eq!(steam.row.gpu, 15.5);
        assert_eq!(steam.row.cpu, 1.5, "CPU stays the process's own");
        // GPU is capped at 100% (owner decision): a tree's sum cannot pass it.
        let mut busy = rows.clone();
        busy[0].gpu = 60.0;
        busy[1].gpu = 50.0;
        busy[2].gpu = 40.0;
        let v = view(&busy, &ViewOptions::default(), &open_all());
        assert_eq!(v.processes().find(|p| p.row.pid == 100).unwrap().row.gpu, 100.0, "60 + 50 + 40 capped at 100");
        assert_eq!(v.processes().find(|p| p.row.pid == 110).unwrap().row.gpu, 90.0, "a sum under the cap stays");
        for col in [ProcCol::User, ProcCol::PeakMem, ProcCol::CpuTime, ProcCol::PageFaults, ProcCol::Handles, ProcCol::Priority] {
            assert!(!steam.shows(col), "{col:?} is empty on a parent");
        }
        for col in [ProcCol::Name, ProcCol::Pid, ProcCol::Status, ProcCol::Cpu, ProcCol::Mem, ProcCol::DiskRead, ProcCol::DiskWrite, ProcCol::Threads, ProcCol::Gpu, ProcCol::Arch, ProcCol::Cmd] {
            assert!(steam.shows(col), "{col:?} shows on a parent");
        }
        let helper = v.processes().find(|p| p.row.pid == 111).unwrap();
        assert!(helper.shows(ProcCol::Handles), "a single process fills every column");
        assert_eq!(helper.row.mem, 50);
    }

    #[test]
    fn rows_split_into_apps_and_background_processes_with_their_counts() {
        let rows = vec![app(row(1, None, "Code.exe", 5)), row(2, None, "svchost.exe", 9), app(row(3, None, "chrome.exe", 7)), row(4, None, "dwm.exe", 1)];
        let flat = ViewOptions { tree: false, ..Default::default() };
        let v = view(&rows, &flat, &ViewState::default());
        assert_eq!(words(&v), ["# Apps (2)", "chrome.exe", "Code.exe", "# Background processes (2)", "svchost.exe", "dwm.exe"]);
        let only_background = view(&rows[1..2], &flat, &ViewState::default());
        assert_eq!(words(&only_background), ["# Background processes (1)", "svchost.exe"], "an empty group has no header");
    }

    #[test]
    fn a_tree_goes_to_the_group_of_its_top_process() {
        let v = view(&family(), &ViewOptions::default(), &open_all());
        let w = words(&v);
        let at = |name: &str| w.iter().position(|l| l.trim_start().trim_start_matches("v ") == name).unwrap();
        let background = w.iter().position(|l| l.starts_with("# Background")).unwrap();
        assert!(at("gpu-helper.exe") < background, "a window-less child of an app stays with the app: {w:?}");
        assert!(at("dllhost.exe") > background);
    }

    #[test]
    fn turning_the_tree_off_gives_a_flat_list() {
        let flat = ViewOptions { tree: false, ..Default::default() };
        let v = view(&family(), &flat, &open_all());
        assert!(v.processes().all(|p| p.depth == 0 && p.chevron == Chevron::Leaf));
        assert_eq!(words(&v), ["# Apps (1)", "steam.exe", "# Background processes (5)", "svchost.exe", "steamwebhelper.exe", "steamservice.exe", "gpu-helper.exe", "dllhost.exe"]);
        let steam = v.processes().find(|p| p.row.pid == 100).unwrap();
        assert_eq!(steam.row.mem, 300, "no sums in a flat list");
    }

    #[test]
    fn the_filter_matches_name_user_command_or_the_exact_pid() {
        let mut rows = vec![row(1, None, "Code.exe", 1), row(2, None, "svchost.exe", 1), row(23, None, "dwm.exe", 1), row(4, None, "cmd.exe", 1)];
        rows[1].user = "SYSTEM".into();
        rows[2].cmd = "C:\\Windows\\System\\dwm.exe --flag".into();
        rows[0].user = "".into();
        let names = |filter: &str| {
            let opts = ViewOptions { tree: false, filter: filter.into(), sort: (ProcCol::Pid, false), ..Default::default() };
            view(&rows, &opts, &ViewState::default()).processes().map(|p| p.row.name.clone()).collect::<Vec<_>>()
        };
        assert_eq!(names("  CODE "), ["Code.exe"], "name, any case, trimmed");
        assert_eq!(names("system"), ["svchost.exe", "dwm.exe"], "user, or command");
        assert_eq!(names("--FLAG"), ["dwm.exe"], "command");
        assert_eq!(names("2"), ["svchost.exe"], "the PID matches whole, not a part of 23");
        assert_eq!(names(""), ["Code.exe", "svchost.exe", "cmd.exe", "dwm.exe"], "no filter: every row, by PID");
        let opts = ViewOptions { tree: false, filter: "exe".into(), ..Default::default() };
        assert_eq!(view(&rows, &opts, &ViewState::default()).shown, 4);
    }

    #[test]
    fn a_filtered_out_parent_leaves_its_matching_child_at_the_top() {
        let opts = ViewOptions { filter: "gpu".into(), ..Default::default() };
        let v = view(&family(), &opts, &open_all());
        assert_eq!(words(&v), ["# Background processes (1)", "gpu-helper.exe"]);
        assert_eq!(v.shown, 1);
    }

    #[test]
    fn equal_values_keep_a_stable_pid_order_in_both_directions() {
        let rows = vec![row(30, None, "c.exe", 5), row(10, None, "a.exe", 5), row(20, None, "b.exe", 9), row(40, None, "d.exe", 5)];
        let pids = |desc| {
            let opts = ViewOptions { tree: false, filter: String::new(), sort: (ProcCol::Mem, desc), ..Default::default() };
            view(&rows, &opts, &ViewState::default()).processes().map(|p| p.row.pid).collect::<Vec<_>>()
        };
        assert_eq!(pids(true), [20, 10, 30, 40], "largest first, ties by PID");
        assert_eq!(pids(false), [10, 30, 40, 20], "smallest first, ties by PID");
    }

    #[test]
    fn tree_siblings_sort_by_what_their_rows_show() {
        // steamwebhelper's own 200 is below steamservice's 220, but its
        // tree (250) is above it: smallest first, steamservice comes first.
        let mut rows = family();
        rows[3].mem = 220;
        let opts = ViewOptions { sort: (ProcCol::Mem, false), ..Default::default() };
        let v = view(&rows, &opts, &open_all());
        let children: Vec<String> = words(&v).into_iter().filter(|w| w.starts_with("  ") && !w.starts_with("    ")).collect();
        assert_eq!(children, ["  steamservice.exe", "  v steamwebhelper.exe", "  dllhost.exe"]);
    }

    #[test]
    fn a_younger_process_is_not_the_parent_of_an_older_one() {
        // PID 100 was reused: the old parent is gone and this one started later.
        let rows = vec![ProcRow { run_time: 5, ..row(100, None, "new.exe", 1) }, ProcRow { run_time: 500, ..row(7, Some(100), "orphan.exe", 1) }];
        let mut state = ViewState::default();
        state.expand_all(&rows);
        let v = view(&rows, &ViewOptions::default(), &state);
        assert!(v.processes().all(|p| p.depth == 0 && p.chevron == Chevron::Leaf), "{:?}", words(&v));
    }

    #[test]
    fn a_folded_group_keeps_its_header_and_count_and_hides_its_rows() {
        let mut state = open_all();
        state.toggle_group(Group::Apps);
        let v = view(&family(), &ViewOptions::default(), &state);
        assert_eq!(words(&v), ["# Apps (4)", "# Background processes (2)", "v svchost.exe", "  dllhost.exe"]);
        assert!(matches!(&v.lines[0], Line::Group(GroupLine { folded: true, .. })));
        state.toggle_group(Group::Apps);
        assert_eq!(view(&family(), &ViewOptions::default(), &state).lines.len(), 8, "a second double-click unfolds it");
    }

    #[test]
    fn right_opens_and_left_closes_the_selected_tree_row() {
        let mut state = ViewState::default();
        let opts = ViewOptions::default();
        let v = view(&family(), &opts, &state);
        state.open(&v, 100, true);
        let v = view(&family(), &opts, &state);
        assert_eq!(words(&v)[1..4], ["v steam.exe", "  > steamwebhelper.exe", "  steamservice.exe"]);
        state.open(&v, 120, true);
        assert!(!state.expanded.contains(&120), "a row without children has nothing to open");
        state.open(&v, 100, false);
        assert_eq!(words(&view(&family(), &opts, &state))[1], "> steam.exe");
    }

    #[test]
    fn expand_all_opens_every_parent_and_collapse_all_closes_them() {
        let mut state = ViewState::default();
        state.expand_all(&family());
        assert_eq!(view(&family(), &ViewOptions::default(), &state).lines.len(), 8);
        state.collapse_all();
        assert_eq!(view(&family(), &ViewOptions::default(), &state).lines.len(), 4);
    }

    #[test]
    fn revealing_a_process_opens_its_ancestors_and_its_group() {
        let mut state = ViewState::default();
        state.folded.insert(Group::Apps);
        state.reveal(&family(), 111);
        let v = view(&family(), &ViewOptions::default(), &state);
        assert!(v.line_of(111).is_some(), "{:?}", words(&v));
        assert!(!state.expanded.contains(&120) && !state.expanded.contains(&200), "nothing else opens");
    }

    #[test]
    fn an_add_on_column_sorts_the_view_by_its_figures_none_lowest() {
        let rows = vec![row(10, None, "a.exe", 1), row(20, None, "b.exe", 1), row(30, None, "c.exe", 1)];
        let pids = |figures: Figures| {
            let opts = ViewOptions { tree: false, sort: (ProcCol::Addon("plug"), true), figures, ..Default::default() };
            view(&rows, &opts, &ViewState::default()).processes().map(|p| p.row.pid).collect::<Vec<_>>()
        };
        let by_pid = HashMap::from([(10, 2), (30, 7)]);
        assert_eq!(pids(Figures::from([("plug", by_pid)])), [30, 10, 20], "largest first; a process without a figure counts 0");
        assert_eq!(pids(Figures::new()), [10, 20, 30], "no reading yet: every figure equal, PID order");
    }

    /// The family with program files: svchost.exe and dllhost.exe are
    /// Windows's, the Steam ones are not.
    fn with_files() -> Vec<ProcRow> {
        family()
            .into_iter()
            .map(|r| {
                let dir = if r.name.starts_with("s") && r.name != "svchost.exe" || r.name.starts_with("gpu") { r"C:\Steam" } else { r"C:\Windows\System32" };
                ProcRow { exe: format!(r"{dir}\{}", r.name), ..r }
            })
            .collect()
    }

    /// A fake "is Microsoft" set: the program files of svchost.exe and dllhost.exe.
    fn windows_files() -> Arc<HashSet<String>> {
        Arc::new([r"C:\Windows\System32\svchost.exe", r"C:\Windows\System32\dllhost.exe"].iter().map(|p| p.to_string()).collect())
    }

    fn names(v: &View) -> Vec<String> {
        v.processes().map(|p| p.row.name.clone()).collect()
    }

    #[test]
    fn microsoft_signed_rows_are_hidden_only_while_the_option_is_on() {
        let flat = |hide_microsoft: bool| {
            let settings = { let mut s = Settings::default(); s.hide_microsoft = hide_microsoft; s };
            ViewOptions { tree: false, sort: (ProcCol::Pid, false), hide: Hide::new(&settings, 1, windows_files()), ..Default::default() }
        };
        let off = view(&with_files(), &flat(false), &ViewState::default());
        assert_eq!(names(&off), ["steam.exe", "steamwebhelper.exe", "gpu-helper.exe", "steamservice.exe", "svchost.exe", "dllhost.exe"]);
        let on = view(&with_files(), &flat(true), &ViewState::default());
        assert_eq!(names(&on), ["steam.exe", "steamwebhelper.exe", "gpu-helper.exe", "steamservice.exe"]);
        assert_eq!(on.shown, 4, "hidden rows are not counted");
        let rows = with_files();
        assert_eq!(flat(true).hide.why(&rows[4]), Some(Hidden::Microsoft));
        assert_eq!(flat(true).hide.why(&rows[0]), None);
    }

    #[test]
    fn the_child_of_a_hidden_microsoft_row_stays_as_a_top_row() {
        let mut rows = with_files();
        rows[5].exe = r"C:\Tools\tool.exe".into();
        rows[5].name = "tool.exe".into();
        let settings = { let mut s = Settings::default(); s.hide_microsoft = true; s };
        let opts = ViewOptions { hide: Hide::new(&settings, 1, windows_files()), ..Default::default() };
        let v = view(&rows, &opts, &ViewState::default());
        assert_eq!(words(&v), ["# Apps (4)", "> steam.exe", "# Background processes (1)", "tool.exe"]);
    }

    #[test]
    fn the_apps_own_row_is_hidden_by_default_and_shown_in_reports_when_asked() {
        let mut rows = family();
        rows.push(row(4242, None, "ultimate-pc-manager.exe", 90));
        let opts = |settings: &Settings| ViewOptions { tree: false, hide: Hide::new(settings, 4242, Arc::default()), ..Default::default() };
        let default = view(&rows, &opts(&Settings::default()), &ViewState::default());
        assert!(!names(&default).contains(&"ultimate-pc-manager.exe".to_string()), "hidden by default");
        assert_eq!(default.shown, 6);
        let shown = { let mut s = Settings::default(); s.show_self_in_reports = true; s };
        assert!(names(&view(&rows, &opts(&shown), &ViewState::default())).contains(&"ultimate-pc-manager.exe".to_string()));
    }

    /// Steam's family sampled at `t0`, then without gpu-helper.exe (it
    /// exited) at `t0 + 1 s`, tracked with Show exited processes `keep`.
    fn helper_exits(keep: bool) -> (ViewState, Vec<ProcRow>, Instant) {
        let t0 = Instant::now();
        let mut rows = family();
        rows[2].cpu = 4.0;
        rows[2].gpu = 9.0;
        let mut state = open_all();
        state.track(&rows, t0, keep);
        rows.remove(2);
        state.track(&rows, t0 + Duration::from_secs(1), keep);
        (state, rows, t0)
    }

    #[test]
    fn an_exited_process_is_kept_tinted_in_its_group_then_dropped() {
        let (mut state, rows, t0) = helper_exits(true);
        let v = view(&rows, &ViewOptions::default(), &state);
        assert_eq!(words(&v)[..4], ["# Apps (4)", "v steam.exe", "  v steamwebhelper.exe", "    gpu-helper.exe"], "kept under its parent, inside Apps");
        let gone = v.processes().find(|p| p.row.pid == 111).unwrap();
        assert!(gone.exited, "drawn as exited");
        assert_eq!(gone.row.status, "Exited");
        assert_eq!((gone.row.cpu, gone.row.gpu), (0.0, 0.0), "an exited process uses nothing");
        assert!(v.processes().filter(|p| p.row.pid != 111).all(|p| !p.exited));
        state.track(&rows, t0 + Duration::from_secs(1) + EXITED_FOR / 2, true);
        assert!(view(&rows, &ViewOptions::default(), &state).line_of(111).is_some(), "still kept a moment later");
        state.track(&rows, t0 + Duration::from_secs(1) + EXITED_FOR, true);
        let v = view(&rows, &ViewOptions::default(), &state);
        assert_eq!(v.line_of(111), None, "dropped after {EXITED_FOR:?}");
        assert_eq!(words(&v)[0], "# Apps (3)");
    }

    #[test]
    fn with_show_exited_processes_off_an_exited_process_goes_at_once() {
        let (state, rows, _) = helper_exits(false);
        assert_eq!(view(&rows, &ViewOptions::default(), &state).line_of(111), None);
        let (state, rows, _) = helper_exits(true);
        let off = ViewOptions { show_exited: false, ..Default::default() };
        assert_eq!(view(&rows, &off, &state).line_of(111), None, "turning the option off hides the kept rows too");
    }

    #[test]
    fn a_reused_pid_is_the_new_live_process_not_the_exited_one() {
        let (mut state, mut rows, t0) = helper_exits(true);
        rows.push(row(111, None, "new.exe", 1));
        state.track(&rows, t0 + Duration::from_secs(2), true);
        let v = view(&rows, &ViewOptions::default(), &state);
        let lines: Vec<&ProcessLine> = v.processes().filter(|p| p.row.pid == 111).collect();
        assert_eq!(lines.len(), 1);
        assert_eq!((lines[0].row.name.as_str(), lines[0].exited), ("new.exe", false));
    }

    /// The line of `pid` in `rows` under `opts` and `state`.
    fn line(rows: &[ProcRow], opts: &ViewOptions, state: &ViewState, pid: u32) -> ProcessLine {
        view(rows, opts, state).processes().find(|p| p.row.pid == pid).cloned().expect("drawn")
    }

    #[test]
    fn smoothing_damps_a_spike() {
        let t0 = Instant::now();
        let mut rows = vec![row(1, None, "a.exe", 10)];
        let mut state = ViewState::default();
        state.track(&rows, t0, true);
        rows[0].cpu = 100.0;
        state.track(&rows, t0 + Duration::from_secs(1), true);
        let smooth = ViewOptions { smooth: true, ..Default::default() };
        // As the CPU cell shows it.
        let cpu = |opts: &ViewOptions, state: &ViewState| format!("{:.1}", line(&rows, opts, state, 1).row.cpu);
        assert_eq!(cpu(&smooth, &state), "30.0", "0% then a 100% spike reads 30%");
        assert_eq!(cpu(&ViewOptions::default(), &state), "100.0", "off: the last reading");
        let mut state2 = state.clone();
        let mut calm = rows.clone();
        calm[0].cpu = 0.0;
        state2.track(&calm, t0 + Duration::from_secs(2), true);
        assert_eq!(format!("{:.1}", line(&calm, &smooth, &state2, 1).row.cpu), "21.0", "and the spike fades");
    }

    #[test]
    fn a_row_whose_cpu_or_memory_changed_is_lit_for_a_moment() {
        let t0 = Instant::now();
        let mut rows = vec![row(1, None, "a.exe", 10_000_000), row(2, None, "b.exe", 10_000_000), row(3, None, "c.exe", 10_000_000)];
        let mut state = ViewState::default();
        state.track(&rows, t0, true);
        rows[0].cpu = 5.0;
        rows[1].mem += 1_048_576;
        rows[2].mem += 100;
        let t1 = t0 + Duration::from_secs(1);
        state.track(&rows, t1, true);
        let at = |now| ViewOptions { now, ..Default::default() };
        let lit = |now, pid| line(&rows, &at(now), &state, pid).lit;
        assert!(lit(t1, 1), "its CPU changed");
        assert!(lit(t1, 2), "its Memory changed");
        assert!(!lit(t1, 3), "100 bytes is not a change the cell shows");
        assert!(lit(t1 + LIT_FOR / 2, 1), "still lit a moment later");
        assert!(!lit(t1 + LIT_FOR, 1), "dark after {LIT_FOR:?}");
    }

    #[test]
    fn turning_highlights_off_stops_the_lit_rows() {
        let t0 = Instant::now();
        let mut rows = vec![row(1, None, "a.exe", 10)];
        let mut state = ViewState::default();
        state.track(&rows, t0, true);
        rows[0].cpu = 50.0;
        let t1 = t0 + Duration::from_secs(1);
        state.track(&rows, t1, true);
        let off = ViewOptions { highlight: false, now: t1, ..Default::default() };
        assert!(!line(&rows, &off, &state, 1).lit);
        assert!(line(&rows, &ViewOptions { highlight: true, ..off }, &state, 1).lit, "on, the same row is lit");
    }

    #[test]
    fn percents_are_computed_against_the_totals() {
        let p = ProcRow { cpu: 12.5, gpu: 3.0, mem: 4_000_000_000, peak_mem: 8_000_000_000, disk_read: 25.0, disk_write: 0.0, threads: 9, ..Default::default() };
        let totals = Totals { mem: 16_000_000_000, disk_read: 100.0, disk_write: 0.0 };
        let got = |col| percent(col, &p, &totals);
        assert_eq!(got(ProcCol::Mem), Some(25.0));
        assert_eq!(got(ProcCol::PeakMem), Some(50.0));
        assert_eq!(got(ProcCol::DiskRead), Some(25.0), "a quarter of what the machine reads");
        assert_eq!(got(ProcCol::DiskWrite), Some(0.0), "an idle disk is 0%, not a division by zero");
        assert_eq!(got(ProcCol::Cpu), Some(12.5));
        assert_eq!(got(ProcCol::Gpu), Some(3.0));
        assert_eq!(got(ProcCol::Threads), None, "counts have no percent");
    }

    fn service(name: &str, display: &str) -> crate::sampler::ServiceInfo {
        crate::sampler::ServiceInfo { name: name.into(), display_name: display.into(), host: "svchost.exe (200)".into(), ..Default::default() }
    }

    /// The family with svchost (200) hosting Winmgmt and Schedule.
    fn hosting() -> ViewOptions {
        let services = [(200, vec![service("Winmgmt", "Windows Management Instrumentation"), service("Schedule", "Task Scheduler")])];
        ViewOptions { services: Arc::new(services.into_iter().collect()), ..Default::default() }
    }

    #[test]
    fn a_host_with_services_gets_them_as_children_one_level_down() {
        let closed = view(&family(), &hosting(), &ViewState::default());
        assert!(words(&closed).contains(&"> svchost.exe".to_string()), "a host with services opens like a parent: {:?}", words(&closed));
        let v = view(&family(), &hosting(), &open_all());
        assert_eq!(
            words(&v)[5..],
            [
                "# Background processes (2)",
                "v svchost.exe",
                "  Service: Schedule (Task Scheduler)",
                "  Service: Winmgmt (Windows Management Instrumentation)",
                "  dllhost.exe",
            ],
            "the services first, by name, then the child processes; the count holds processes only"
        );
        let s = v.processes().find(|l| l.row.service.is_some()).expect("a service line");
        assert_eq!((s.row.pid, s.depth, s.chevron), (200, 1, Chevron::Leaf), "it names its host's PID");
        assert_eq!(s.row.service.as_ref().map(|i| i.name.as_str()), Some("Schedule"), "it carries the service for the details pane");
        assert!(s.shows(ProcCol::User) && !s.shows(ProcCol::Mem) && !s.shows(ProcCol::Cpu), "a service has no figures of its own");
        // A flat list has no tree to put them in.
        let flat = view(&family(), &ViewOptions { tree: false, ..hosting() }, &open_all());
        assert!(flat.processes().all(|l| l.row.service.is_none()));
    }

    #[test]
    fn a_service_row_is_selected_on_its_own_not_with_its_host() {
        let v = view(&family(), &hosting(), &open_all());
        let s = v.processes().find(|l| l.row.service.is_some()).expect("a service line").row.clone();
        let host = v.processes().find(|l| l.row.pid == 200 && l.row.service.is_none()).expect("the host").row.clone();
        let sel = Selection::service(&s);
        assert!(sel.has(&s) && !sel.has(&host), "the service, not its host");
        assert_eq!((sel.single(), sel.contains(200)), (None, false), "no process is selected");
        let mut sel = sel;
        sel.click(&v, 200, Pick::Toggle);
        assert_eq!(pids(&sel), [200], "a click on a process starts over");
        assert!(sel.has(&host) && !sel.has(&s));
    }
}
