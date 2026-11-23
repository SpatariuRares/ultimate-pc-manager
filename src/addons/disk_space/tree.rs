//! The space tree (pure: no UI, no file system): built from the scanner's
//! events "file found (path, size)" and "folder not readable (path)", it
//! keeps the totals of every folder, the largest files of each folder and
//! of the whole scan, and the unreadable folders. It can be read at any
//! time while it grows.

use std::collections::hash_map::{Entry, RandomState};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::kind::Category;

/// What the scanner found.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A file and its size in bytes.
    File(PathBuf, u64),
    /// A folder whose content could not be listed.
    Unreadable(PathBuf),
    /// A folder that could not be listed because access was denied
    /// (readable only by the system): also unreadable.
    Denied(PathBuf),
    /// The scan stopped tracking hard links: further links of a file
    /// may each be counted.
    HardLinkLimit,
}

/// Index of a folder in the tree; the root is [`SpaceTree::ROOT`].
pub type NodeId = usize;

#[derive(Clone)]
struct Node {
    /// The folder's name in its parent; for an anchor (the root, a drive
    /// of a virtual root), its whole path. Full paths are rebuilt from the
    /// names on demand ([`SpaceTree::path`]).
    name: Box<OsStr>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// Bytes and files of everything under this folder.
    bytes: u64,
    files: u64,
    /// Unreadable folders under this folder (itself included).
    unreadable: u64,
    /// Bytes and files of each category under this folder, by [`Category::index`].
    kinds: [(u64, u64); 7],
    /// The largest files directly in this folder (name, bytes), largest first.
    own_largest: Vec<(String, u64)>,
    /// The largest files of its whole subtree, largest first, kept up to
    /// date as files arrive (the root's are the scan's `largest`). Empty
    /// while the folder has no sub-folders: its own files are its
    /// subtree's (most folders are such leaves, #231).
    subtree_largest: Vec<(Arc<Path>, u64)>,
}

impl Node {
    fn new(name: &OsStr, parent: Option<NodeId>) -> Node {
        Node { name: name.into(), parent, children: Vec::new(), bytes: 0, files: 0, unreadable: 0, kinds: [(0, 0); 7], own_largest: Vec::new(), subtree_largest: Vec::new() }
    }
}

#[cfg(test)]
thread_local! {
    /// How many folders' own files were read on this thread: lets a test
    /// tell a view that walks a level from one that reads kept lists.
    pub static LOOSE_FILES_READS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Where tree revisions come from: unique across trees, so a revision
/// names one state of one tree.
static REVISIONS: AtomicU64 = AtomicU64::new(1);

fn next_revision() -> u64 {
    REVISIONS.fetch_add(1, Ordering::Relaxed)
}

/// How many unreadable folders are kept as examples.
pub const EXAMPLES: usize = 8;

/// The tree of a scan.
#[derive(Clone)]
pub struct SpaceTree {
    nodes: Vec<Node>,
    /// The folders whose name is a whole path, where every path starts:
    /// the root first, then the drives of a virtual root.
    anchors: Vec<NodeId>,
    /// Each folder by (parent, hash of its name): the name itself is kept
    /// only in the node (#231).
    index: HashMap<(NodeId, u64), NodeId>,
    /// The folders whose (parent, hash) another folder already holds.
    collisions: Vec<NodeId>,
    names: RandomState,
    /// The last folder files were counted in, with its path: the scanner
    /// delivers a folder's files together.
    last: Option<(PathBuf, NodeId)>,
    /// How many files each list keeps (per folder and for the whole scan).
    keep: usize,
    largest: Vec<(PathBuf, u64)>,
    examples: Vec<PathBuf>,
    /// Unreadable folders whose access was denied (readable only by the
    /// system), and the first of them.
    denied: u64,
    first_denied: Option<PathBuf>,
    /// Changes with every delivery (see [`SpaceTree::revision`]).
    revision: u64,
    /// The scan reached the hard-link tracking limit.
    hard_link_limit: bool,
}

impl SpaceTree {
    pub const ROOT: NodeId = 0;

    /// An empty tree for the scan of `root`, keeping the `keep` largest
    /// files of each folder and of the scan.
    pub fn new(root: impl Into<PathBuf>, keep: usize) -> Self {
        let root = root.into();
        SpaceTree {
            nodes: vec![Node::new(root.as_os_str(), None)],
            anchors: vec![Self::ROOT],
            index: HashMap::new(),
            collisions: Vec::new(),
            names: RandomState::new(),
            last: None,
            keep,
            largest: Vec::new(),
            examples: Vec::new(),
            revision: next_revision(),
            denied: 0,
            first_denied: None,
            hard_link_limit: false,
        }
    }

    /// An empty tree whose root, named `name`, is no folder: its children
    /// are the folders `roots` (All disks: one per local drive).
    pub fn virtual_root(name: &str, roots: &[&str], keep: usize) -> Self {
        let mut tree = Self::new(name, keep);
        for root in roots {
            let n = tree.add_child(Self::ROOT, OsStr::new(root));
            tree.anchors.push(n);
        }
        tree
    }

    /// Applies the scanner's events in order (one delivery: one new
    /// revision).
    pub fn apply(&mut self, events: impl IntoIterator<Item = Event>) {
        for e in events {
            match e {
                Event::File(path, size) => self.count_file(&path, size),
                Event::Unreadable(path) => self.count_unreadable(&path),
                Event::Denied(path) => {
                    if self.folder(&path).is_some() {
                        self.denied += 1;
                        self.first_denied.get_or_insert_with(|| path.clone());
                    }
                    self.count_unreadable(&path);
                }
                Event::HardLinkLimit => self.hard_link_limit = true,
            }
        }
        self.revision = next_revision();
    }

    /// Names the tree's current state: it differs from that of any other
    /// tree and changes whenever files or unreadable folders are added, so
    /// what is computed from the tree can be kept until it changes.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Whether the scan reached the hard-link tracking limit (some links
    /// may then be counted more than once).
    pub fn hard_link_limit(&self) -> bool {
        self.hard_link_limit
    }

    /// Counts one file. Paths outside the root are ignored.
    pub fn add_file(&mut self, path: &Path, size: u64) {
        self.count_file(path, size);
        self.revision = next_revision();
    }

    fn count_file(&mut self, path: &Path, size: u64) {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { return };
        let Some(node) = self.folder(dir) else { return };
        let name = name.to_string_lossy();
        keep_largest(&mut self.nodes[node].own_largest, self.keep, || name.to_string(), size);
        keep_largest(&mut self.largest, self.keep, || path.to_path_buf(), size);
        self.rank_in_subtrees(node, path, size);
        let kind = Category::of(path).index();
        let mut at = Some(node);
        while let Some(n) = at {
            let node = &mut self.nodes[n];
            node.bytes += size;
            node.files += 1;
            node.kinds[kind].0 += size;
            node.kinds[kind].1 += 1;
            at = node.parent;
        }
    }

    /// Puts the file `path` in the largest files of folder `n` and of each
    /// folder above it (the root keeps the scan's own list). A folder's
    /// list holds everything its sub-folder's does, so once the file is too
    /// small for one folder it is too small for those above.
    fn rank_in_subtrees(&mut self, n: NodeId, path: &Path, size: u64) {
        let keep = self.keep;
        let mut shared: Option<Arc<Path>> = None;
        let mut at = Some(n).filter(|&n| n != Self::ROOT);
        while let Some(n) = at {
            let node = &mut self.nodes[n];
            if node.children.is_empty() {
                at = node.parent.filter(|&p| p != Self::ROOT);
                continue;
            }
            if !keep_largest(&mut node.subtree_largest, keep, || shared.get_or_insert_with(|| Arc::from(path)).clone(), size) {
                return;
            }
            at = node.parent.filter(|&p| p != Self::ROOT);
        }
    }

    /// Counts one folder that could not be read.
    pub fn add_unreadable(&mut self, path: &Path) {
        self.count_unreadable(path);
        self.revision = next_revision();
    }

    fn count_unreadable(&mut self, path: &Path) {
        let Some(node) = self.folder(path) else { return };
        if self.examples.len() < EXAMPLES {
            self.examples.push(path.to_path_buf());
        }
        let mut at = Some(node);
        while let Some(n) = at {
            self.nodes[n].unreadable += 1;
            at = self.nodes[n].parent;
        }
    }

    /// The node of folder `dir`, created with its parents if new; None
    /// when `dir` is not under the root.
    fn folder(&mut self, dir: &Path) -> Option<NodeId> {
        if let Some((_, n)) = self.last.as_ref().filter(|(last, _)| last == dir) {
            return Some(*n);
        }
        let (mut at, rest) = self.anchor_of(dir)?;
        for name in rest {
            at = match self.child(at, name) {
                Some(c) => c,
                None => self.add_child(at, name),
            };
        }
        self.last = Some((dir.to_path_buf(), at));
        Some(at)
    }

    /// The node of a folder already in the tree.
    pub fn find(&self, dir: &Path) -> Option<NodeId> {
        let (mut at, rest) = self.anchor_of(dir)?;
        for name in rest {
            at = self.child(at, name)?;
        }
        Some(at)
    }

    /// The anchor `dir` is under (a drive before the root), and the rest
    /// of `dir` below it.
    fn anchor_of<'a>(&self, dir: &'a Path) -> Option<(NodeId, &'a Path)> {
        self.anchors.iter().rev().find_map(|&a| dir.strip_prefix(Path::new(&self.nodes[a].name)).ok().map(|rest| (a, rest)))
    }

    /// The sub-folder `name` of `parent`, if in the tree.
    fn child(&self, parent: NodeId, name: &OsStr) -> Option<NodeId> {
        match self.index.get(&(parent, self.names.hash_one(name))) {
            Some(&n) if *self.nodes[n].name == *name => Some(n),
            Some(_) => self.collisions.iter().copied().find(|&c| self.nodes[c].parent == Some(parent) && *self.nodes[c].name == *name),
            None => None,
        }
    }

    /// Adds the sub-folder `name` to `parent`.
    fn add_child(&mut self, parent: NodeId, name: &OsStr) -> NodeId {
        if parent != Self::ROOT && self.nodes[parent].children.is_empty() && !self.nodes[parent].own_largest.is_empty() {
            // No longer a leaf: its own files start its subtree's list.
            let dir = self.path(parent);
            let node = &mut self.nodes[parent];
            node.subtree_largest = node.own_largest.iter().map(|(file, b)| (Arc::from(dir.join(file).as_path()), *b)).collect();
        }
        let n = self.nodes.len();
        self.nodes.push(Node::new(name, Some(parent)));
        self.nodes[parent].children.push(n);
        match self.index.entry((parent, self.names.hash_one(name))) {
            Entry::Vacant(e) => {
                e.insert(n);
            }
            Entry::Occupied(_) => self.collisions.push(n),
        }
        n
    }

    /// The folder's full path, rebuilt from the names up to its anchor.
    pub fn path(&self, n: NodeId) -> PathBuf {
        let mut names = Vec::new();
        let mut at = n;
        while !self.anchors.contains(&at) {
            names.push(&*self.nodes[at].name);
            at = self.nodes[at].parent.unwrap_or(Self::ROOT);
        }
        let mut path = PathBuf::from(&self.nodes[at].name);
        path.extend(names.iter().rev());
        path
    }

    /// Orders sub-folders of one folder by name, as their paths would.
    fn by_name(&self, a: NodeId, b: NodeId) -> std::cmp::Ordering {
        Path::new(&self.nodes[a].name).cmp(Path::new(&self.nodes[b].name))
    }

    /// Bytes of everything under the folder.
    pub fn bytes(&self, n: NodeId) -> u64 {
        self.nodes[n].bytes
    }

    /// Files under the folder.
    pub fn files(&self, n: NodeId) -> u64 {
        self.nodes[n].files
    }

    /// Bytes and files of category `c` under the folder.
    pub fn category(&self, n: NodeId, c: Category) -> (u64, u64) {
        self.nodes[n].kinds[c.index()]
    }

    /// Unreadable folders under the folder (itself included).
    pub fn unreadable(&self, n: NodeId) -> u64 {
        self.nodes[n].unreadable
    }

    /// How many unreadable folders were denied access, and the first of them.
    pub fn denied(&self) -> (u64, Option<&Path>) {
        (self.denied, self.first_denied.as_deref())
    }

    /// Up to [`EXAMPLES`] unreadable folders, in the order they were found.
    pub fn unreadable_examples(&self) -> &[PathBuf] {
        &self.examples
    }

    /// The folder's sub-folders, largest first (ties by path).
    pub fn children(&self, n: NodeId) -> Vec<NodeId> {
        let mut c = self.nodes[n].children.clone();
        c.sort_by(|&a, &b| self.nodes[b].bytes.cmp(&self.nodes[a].bytes).then_with(|| self.by_name(a, b)));
        c
    }

    /// The `count` largest sub-folders of the folder (path, bytes).
    pub fn largest_folders(&self, n: NodeId, count: usize) -> Vec<(PathBuf, u64)> {
        self.children(n).into_iter().take(count).map(|c| (self.path(c), self.nodes[c].bytes)).collect()
    }

    /// The `count` (at most the tree's `keep`) largest files under the
    /// folder, largest first.
    pub fn largest_files(&self, n: NodeId, count: usize) -> Vec<(PathBuf, u64)> {
        if n == Self::ROOT {
            return self.largest.iter().take(count).cloned().collect();
        }
        if self.nodes[n].children.is_empty() {
            return self.loose_files(n).into_iter().take(count).collect();
        }
        self.nodes[n].subtree_largest.iter().take(count).map(|(p, b)| (p.to_path_buf(), *b)).collect()
    }

    /// The folder holding this one (None for the root).
    pub fn parent(&self, n: NodeId) -> Option<NodeId> {
        self.nodes[n].parent
    }

    /// "this folder": bytes and files directly in the folder (not in its
    /// sub-folders).
    pub fn loose(&self, n: NodeId) -> (u64, u64) {
        let node = &self.nodes[n];
        node.children.iter().fold((node.bytes, node.files), |(b, f), &c| (b - self.nodes[c].bytes, f - self.nodes[c].files))
    }

    /// Every folder holds at least what its sub-folders hold (bytes,
    /// files and each category's), so "this folder" is never negative.
    pub fn consistent(&self) -> bool {
        self.nodes.iter().all(|node| {
            let holds = |own: u64, of: &dyn Fn(&Node) -> u64| node.children.iter().try_fold(0u64, |sum, &c| sum.checked_add(of(&self.nodes[c]))).is_some_and(|sum| sum <= own);
            holds(node.bytes, &|c| c.bytes)
                && holds(node.files, &|c| c.files)
                && (0..node.kinds.len()).all(|i| holds(node.kinds[i].0, &|c| c.kinds[i].0) && holds(node.kinds[i].1, &|c| c.kinds[i].1))
        })
    }

    /// Bytes and files of category `c` directly in the folder.
    pub fn loose_category(&self, n: NodeId, c: Category) -> (u64, u64) {
        let i = c.index();
        let node = &self.nodes[n];
        node.children.iter().fold(node.kinds[i], |(b, f), &ch| (b - self.nodes[ch].kinds[i].0, f - self.nodes[ch].kinds[i].1))
    }

    /// The largest files directly in the folder (at most the tree's
    /// `keep`), largest first.
    pub fn loose_files(&self, n: NodeId) -> Vec<(PathBuf, u64)> {
        #[cfg(test)]
        LOOSE_FILES_READS.with(|c| c.set(c.get() + 1));
        let dir = self.path(n);
        self.nodes[n].own_largest.iter().map(|(name, b)| (dir.join(name), *b)).collect()
    }

    /// The category with the most bytes under the folder ("mostly
    /// Media"); Other when it is empty. Ties go to the earlier category.
    pub fn prevalent(&self, n: NodeId) -> Category {
        prevalent_of(|c| self.category(n, c).0)
    }

    /// The category with the most bytes directly in the folder.
    pub fn loose_prevalent(&self, n: NodeId) -> Category {
        prevalent_of(|c| self.loose_category(n, c).0)
    }

    /// The folder where the space goes: from the root, the largest
    /// sub-folder while it holds at least half of its parent. None when
    /// no sub-folder of the root holds half of it.
    pub fn dominant_folder(&self) -> Option<NodeId> {
        let mut at = Self::ROOT;
        let mut found = None;
        while let Some(big) = self.largest_child(at) {
            if self.nodes[big].bytes == 0 || self.nodes[big].bytes * 2 < self.nodes[at].bytes {
                break;
            }
            found = Some(big);
            at = big;
        }
        found
    }

    /// The largest sub-folder of the root, when there is one with bytes.
    pub fn largest_folder(&self) -> Option<NodeId> {
        self.largest_child(Self::ROOT).filter(|&n| self.nodes[n].bytes > 0)
    }

    /// The first of [`SpaceTree::children`] in one pass, without sorting
    /// them all (the page asks for it every frame, under the scan's lock).
    fn largest_child(&self, n: NodeId) -> Option<NodeId> {
        self.nodes[n].children.iter().copied().min_by(|&a, &b| self.nodes[b].bytes.cmp(&self.nodes[a].bytes).then_with(|| self.by_name(a, b)))
    }
}

/// A folder as the history saves it: its totals, without its files.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedFolder {
    pub path: PathBuf,
    pub bytes: u64,
    pub files: u64,
    pub unreadable: u64,
    /// Levels below the root (the root is 0).
    pub depth: usize,
    /// Bytes and files per category under it, in `Category::ALL` order.
    pub kinds: [(u64, u64); 7],
}

/// Reading a tree out for the history, and building it back (#88).
impl SpaceTree {
    /// Every folder (root first, parents before their sub-folders).
    pub fn saved_folders(&self) -> Vec<SavedFolder> {
        let mut out: Vec<SavedFolder> = Vec::with_capacity(self.nodes.len());
        for (n, node) in self.nodes.iter().enumerate() {
            let (path, depth) = match node.parent {
                Some(p) if !self.anchors.contains(&n) => (out[p].path.join(&*node.name), out[p].depth + 1),
                Some(p) => (PathBuf::from(&node.name), out[p].depth + 1),
                None => (PathBuf::from(&node.name), 0),
            };
            out.push(SavedFolder { path, bytes: node.bytes, files: node.files, unreadable: node.unreadable, depth, kinds: node.kinds });
        }
        out
    }

    /// The largest files directly in the folder, largest first.
    pub fn own_files(&self, n: NodeId) -> Vec<(PathBuf, u64)> {
        self.loose_files(n)
    }

    /// The tree of a saved scan: the folders' totals as saved (parents
    /// before sub-folders; the root's entry, if any, gives its totals),
    /// the files put back in their folders and in the scan's largest, and
    /// the root's categories. Each saved folder keeps its own categories
    /// (so its prevalent type); folders left out of the snapshot are
    /// simply missing. First-level folders
    /// not under the root are its drives (a virtual root: All disks).
    pub fn restore(root: impl Into<PathBuf>, keep: usize, folders: &[SavedFolder], files: &[(PathBuf, u64)], kinds: [(u64, u64); 7], examples: Vec<PathBuf>) -> Self {
        let root = root.into();
        let drives: Vec<String> = folders.iter().filter(|f| f.depth == 1 && !f.path.starts_with(&root)).map(|f| f.path.to_string_lossy().into_owned()).collect();
        let mut t = SpaceTree::virtual_root(&root.to_string_lossy(), &drives.iter().map(String::as_str).collect::<Vec<_>>(), keep);
        for f in folders {
            let Some(n) = t.folder(&f.path) else { continue };
            let node = &mut t.nodes[n];
            (node.bytes, node.files, node.unreadable, node.kinds) = (f.bytes, f.files, f.unreadable, f.kinds);
        }
        for (path, size) in files {
            let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { continue };
            if let Some(n) = t.find(dir) {
                keep_largest(&mut t.nodes[n].own_largest, keep, || name.to_string_lossy().into_owned(), *size);
                t.rank_in_subtrees(n, path, *size);
            }
            keep_largest(&mut t.largest, keep, || path.clone(), *size);
        }
        t.nodes[Self::ROOT].kinds = kinds;
        t.examples = examples;
        t
    }
}

/// The category with the most `bytes`; Other when none has any.
fn prevalent_of(bytes: impl Fn(Category) -> u64) -> Category {
    let mut best = (Category::Other, 0);
    for c in Category::ALL {
        let b = bytes(c);
        if b > best.1 {
            best = (c, b);
        }
    }
    best.0
}

/// Inserts (`item`, `size`) into `list` (largest first) if it is among the
/// `keep` largest; `item` is built only then. True when it was inserted.
fn keep_largest<T>(list: &mut Vec<(T, u64)>, keep: usize, item: impl FnOnce() -> T, size: u64) -> bool {
    if keep == 0 || (list.len() >= keep && size <= list[keep - 1].1) {
        return false;
    }
    let at = list.partition_point(|l| l.1 >= size);
    list.insert(at, (item(), size));
    list.truncate(keep);
    true
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::test_paths::host;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn files() -> Vec<Event> {
        vec![
            Event::File(p("R/big/movie.mkv"), 5000),
            Event::File(p("R/big/deep/clip.mp4"), 3000),
            Event::File(p("R/small/note.txt"), 100),
            Event::File(p("R/loose.bin"), 700),
            Event::File(p("R/big/deep/a.txt"), 10),
        ]
    }

    fn totals(t: &SpaceTree) -> Vec<(String, u64, u64)> {
        ["R", "R/big", "R/big/deep", "R/small"]
            .iter()
            .map(|d| {
                let n = t.find(Path::new(d)).unwrap();
                (d.to_string(), t.bytes(n), t.files(n))
            })
            .collect()
    }

    #[test]
    fn a_virtual_root_holds_several_drives() {
        let mut t = SpaceTree::virtual_root("All disks", &[host("C:\\"), host("D:\\")], 8);
        t.apply([Event::File(p(host("C:\\a\\x")), 600), Event::File(p(host("D:\\y")), 40), Event::File(p(host("E:\\z")), 1), Event::Unreadable(p(host("D:\\locked")))]);
        assert_eq!(t.path(SpaceTree::ROOT), Path::new("All disks"));
        assert_eq!((t.bytes(SpaceTree::ROOT), t.files(SpaceTree::ROOT), t.unreadable(SpaceTree::ROOT)), (640, 2, 1), "E: is not one of its drives");
        let slashed = |windows: &'static str| host(windows).replace('\\', "/");
        assert_eq!(names(&t.largest_folders(SpaceTree::ROOT, 8)), vec![(slashed("C:\\"), 600), (slashed("D:\\"), 40)]);
        assert_eq!(names(&t.largest_files(SpaceTree::ROOT, 8)), vec![(slashed("C:\\a\\x"), 600), (slashed("D:\\y"), 40)]);
    }

    #[test]
    fn totals_are_the_same_whatever_the_order_of_the_files() {
        let mut forward = SpaceTree::new("R", 8);
        forward.apply(files());
        let mut backward = SpaceTree::new("R", 8);
        backward.apply(files().into_iter().rev());
        let expected = vec![
            ("R".to_string(), 8810, 5),
            ("R/big".to_string(), 8010, 3),
            ("R/big/deep".to_string(), 3010, 2),
            ("R/small".to_string(), 100, 1),
        ];
        assert_eq!(totals(&forward), expected);
        assert_eq!(totals(&backward), expected);
    }

    fn names(list: &[(PathBuf, u64)]) -> Vec<(String, u64)> {
        list.iter().map(|(p, b)| (p.to_string_lossy().replace('\\', "/"), *b)).collect()
    }

    fn s(v: &[(&str, u64)]) -> Vec<(String, u64)> {
        v.iter().map(|(p, b)| (p.to_string(), *b)).collect()
    }

    #[test]
    fn sub_folders_come_largest_first() {
        let mut t = SpaceTree::new("R", 8);
        t.apply(vec![
            Event::File(p("R/a/x"), 10),
            Event::File(p("R/b/x"), 30),
            Event::File(p("R/c/x"), 20),
            Event::File(p("R/a/y"), 15),
        ]);
        assert_eq!(names(&t.largest_folders(SpaceTree::ROOT, 8)), s(&[("R/b", 30), ("R/a", 25), ("R/c", 20)]));
        assert_eq!(names(&t.largest_folders(SpaceTree::ROOT, 2)), s(&[("R/b", 30), ("R/a", 25)]));
    }

    #[test]
    fn it_keeps_the_largest_files_of_the_scan_and_of_each_folder() {
        let mut t = SpaceTree::new("R", 3);
        t.apply(files());
        assert_eq!(names(&t.largest_files(SpaceTree::ROOT, 3)), s(&[("R/big/movie.mkv", 5000), ("R/big/deep/clip.mp4", 3000), ("R/loose.bin", 700)]));
        let big = t.find(Path::new("R/big")).unwrap();
        assert_eq!(names(&t.largest_files(big, 2)), s(&[("R/big/movie.mkv", 5000), ("R/big/deep/clip.mp4", 3000)]));
        let deep = t.find(Path::new("R/big/deep")).unwrap();
        assert_eq!(names(&t.largest_files(deep, 3)), s(&[("R/big/deep/clip.mp4", 3000), ("R/big/deep/a.txt", 10)]));
        assert_eq!(names(&t.largest_files(SpaceTree::ROOT, 1)), s(&[("R/big/movie.mkv", 5000)]));
    }

    #[test]
    fn unreadable_folders_are_counted_with_up_to_eight_examples() {
        let mut t = SpaceTree::new("R", 8);
        for i in 0..10 {
            t.add_unreadable(&p(&format!("R/locked/{i}")));
        }
        t.add_file(&p("R/open/f"), 5);
        assert_eq!(t.unreadable(SpaceTree::ROOT), 10);
        assert_eq!(t.unreadable(t.find(Path::new("R/locked")).unwrap()), 10);
        assert_eq!(t.unreadable(t.find(Path::new("R/open")).unwrap()), 0);
        assert_eq!(t.unreadable_examples().len(), EXAMPLES);
        assert_eq!(t.unreadable_examples()[0], p("R/locked/0"));
    }

    #[test]
    fn the_dominant_folder_is_where_at_least_half_the_space_goes_deepest() {
        let mut t = SpaceTree::new("R", 8);
        t.apply(vec![
            Event::File(p("R/Users/me/AppData/Docker/disk.vhdx"), 700),
            Event::File(p("R/Users/me/AppData/Docker/b"), 13),
            Event::File(p("R/Users/me/Videos/v"), 300),
            Event::File(p("R/Windows/w"), 500),
        ]);
        // Users 1013 of 1513 (>= half), me 1013, AppData 713 of 1013, Docker 713 of 713,
        // then no sub-folder: Docker is where the space goes.
        assert_eq!(t.dominant_folder().map(|n| t.path(n).to_path_buf()), Some(p("R/Users/me/AppData/Docker")));
        assert_eq!(t.largest_folder().map(|n| t.path(n).to_path_buf()), Some(p("R/Users")));

        let mut even = SpaceTree::new("R", 8);
        even.apply(vec![Event::File(p("R/a/x"), 10), Event::File(p("R/b/x"), 10), Event::File(p("R/c/x"), 10)]);
        assert_eq!(even.dominant_folder(), None, "no folder holds half");
    }

    /// The page asks for the dominant folder several times a frame while
    /// the scan waits for the same lock: with a crowded folder on the way
    /// (WinSxS, node_modules) it must not sort that folder's sub-folders
    /// each time (it took 110 ms on C:\Windows and starved the scan).
    #[test]
    fn the_dominant_folder_is_found_quickly_past_a_crowded_folder() {
        let mut t = SpaceTree::new("R", 8);
        t.add_file(&p("R/big/0/huge"), 1 << 40);
        for i in 1..60_000 {
            t.add_file(&p(&format!("R/big/{i}/f")), 1);
        }
        // Ties go to the earlier path, as in the sorted listing.
        t.add_file(&p("R/even/b/x"), 5);
        t.add_file(&p("R/even/a/x"), 5);
        assert_eq!(t.dominant_folder().map(|n| t.path(n).to_path_buf()), Some(p("R/big/0")));
        // Timed against sorting the crowded folder in the same run, so a
        // loaded machine slows both sides; the best of a few rounds drops
        // the rounds another process interrupted (#149).
        let big = t.find(&p("R/big")).unwrap();
        let best_of = |f: &dyn Fn()| {
            (0..5)
                .map(|_| {
                    let started = std::time::Instant::now();
                    f();
                    started.elapsed()
                })
                .min()
                .unwrap()
        };
        let lookups = best_of(&|| {
            for _ in 0..20 {
                std::hint::black_box(t.dominant_folder());
            }
        });
        let sorts = best_of(&|| {
            for _ in 0..20 {
                std::hint::black_box(t.children(big).first().copied());
            }
        });
        assert!(lookups * 3 < sorts, "20 lookups took {lookups:?}, as long as sorting the crowded folder 20 times ({sorts:?})");

        let mut tie = SpaceTree::new("R", 8);
        tie.add_file(&p("R/b/x"), 5);
        tie.add_file(&p("R/a/x"), 5);
        assert_eq!(tie.largest_folder().map(|n| tie.path(n).to_path_buf()), Some(p("R/a")));
    }

    #[test]
    fn every_folder_keeps_the_bytes_and_files_of_each_type_under_it() {
        let mut t = SpaceTree::new("R", 8);
        t.apply(files());
        let of = |d: &str, c: Category| t.category(t.find(Path::new(d)).unwrap(), c);
        assert_eq!(of("R", Category::Media), (8000, 2));
        assert_eq!(of("R", Category::Documents), (110, 2));
        assert_eq!(of("R", Category::Other), (700, 1));
        assert_eq!(of("R", Category::Code), (0, 0));
        assert_eq!(of("R/big", Category::Documents), (10, 1));
        assert_eq!(of("R/big/deep", Category::Media), (3000, 1));
        assert_eq!(of("R/small", Category::Media), (0, 0));
    }

    #[test]
    fn files_outside_the_root_are_ignored() {
        let mut t = SpaceTree::new("R", 8);
        t.add_file(&p("Other/x"), 10);
        t.add_unreadable(&p("Other/y"));
        assert_eq!((t.bytes(SpaceTree::ROOT), t.files(SpaceTree::ROOT), t.unreadable(SpaceTree::ROOT)), (0, 0, 0));
    }

    #[test]
    fn this_folder_holds_the_loose_files_of_a_folder() {
        let mut t = SpaceTree::new("R", 8);
        t.apply(files());
        // R: loose.bin (700, Other) directly in it; big holds 8010, small 100.
        assert_eq!(t.loose(SpaceTree::ROOT), (700, 1));
        assert_eq!(t.loose_category(SpaceTree::ROOT, Category::Other), (700, 1));
        assert_eq!(t.loose_category(SpaceTree::ROOT, Category::Media), (0, 0));
        let big = t.find(Path::new("R/big")).unwrap();
        assert_eq!(t.loose(big), (5000, 1));
        assert_eq!(names(&t.loose_files(big)), s(&[("R/big/movie.mkv", 5000)]));
        let small = t.find(Path::new("R/small")).unwrap();
        assert_eq!(t.parent(small), Some(SpaceTree::ROOT));
        assert_eq!(t.parent(SpaceTree::ROOT), None);
    }

    #[test]
    fn the_prevalent_type_is_the_category_with_the_most_bytes() {
        let mut t = SpaceTree::new("R", 8);
        t.apply(files());
        assert_eq!(t.prevalent(SpaceTree::ROOT), Category::Media);
        assert_eq!(t.prevalent(t.find(Path::new("R/small")).unwrap()), Category::Documents);
        assert_eq!(t.loose_prevalent(SpaceTree::ROOT), Category::Other);
        assert_eq!(SpaceTree::new("E", 8).prevalent(SpaceTree::ROOT), Category::Other, "an empty folder is Other");
    }

    /// Folders are found by parent and the hash of their name (#231): two
    /// names with the same hash under one folder are both kept and found.
    #[test]
    fn folders_whose_names_share_a_hash_are_both_found() {
        let mut t = SpaceTree::new("R", 8);
        t.add_file(&p("R/a/x"), 1);
        let a = t.find(Path::new("R/a")).unwrap();
        let clash = t.names.hash_one(OsStr::new("b"));
        t.index.insert((SpaceTree::ROOT, clash), a);
        t.add_file(&p("R/b/y"), 2);
        let b = t.find(Path::new("R/b")).unwrap();
        assert_ne!(a, b);
        assert_eq!((t.path(b), t.bytes(b)), (p("R/b"), 2));
        assert_eq!(t.find(Path::new("R/a")), Some(a));
        assert_eq!(t.find(Path::new("R/c")), None);
        assert_eq!(names(&t.largest_folders(SpaceTree::ROOT, 8)), s(&[("R/b", 2), ("R/a", 1)]));
    }

    /// Bytes this process has committed for itself.
    #[cfg(windows)]
    pub(crate) fn private_bytes() -> usize {
        use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: the counters are plain data, written by the call for the
        // size given in `cb`.
        unsafe {
            let mut c: PROCESS_MEMORY_COUNTERS_EX = std::mem::zeroed();
            c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
            K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c as *mut PROCESS_MEMORY_COUNTERS_EX as *mut PROCESS_MEMORY_COUNTERS, c.cb);
            c.PrivateUsage
        }
    }

    /// Story 18 (#231): a tree of 500k folders, one file each, measured by
    /// the process's private bytes. Folders keep their name, not their
    /// path: measured 925 bytes per folder (441 MB) with a full path in the
    /// node and another in the index, 512 (244 MB) with names only.
    /// Ignored: it needs the process to itself
    /// (`cargo test --release -- --ignored --exact <name> --test-threads=1`).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn a_tree_of_500k_folders_keeps_their_names_not_their_paths() {
        let before = private_bytes();
        let mut t = SpaceTree::new(host("C:\\"), 8);
        let mut path = String::new();
        for i in 0..500_000 {
            use std::fmt::Write;
            path.clear();
            write!(path, "C:\\Users\\someone\\Projects\\area_{}\\group_{:04}\\folder_number_{i:06}\\file.dat", i / 100_000, i / 1000).unwrap();
            t.add_file(Path::new(&path), 1000);
        }
        let used = private_bytes().saturating_sub(before);
        let folders = t.nodes.len();
        std::hint::black_box(&t);
        let per_folder = used / folders;
        println!("{folders} folders: {} MB, {per_folder} bytes per folder", used / (1 << 20));
        assert!(per_folder < 700,"{per_folder} bytes per folder");
    }
}
