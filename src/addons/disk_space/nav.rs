//! Where the user is in the measured tree (pure: no UI, no layout): the
//! current level, "This Folder Only", the selected piece; and what the
//! level holds, as pieces any drawing (Balls, Block) lays out.

use std::path::{Path, PathBuf};

use super::kind::Category;
use super::tree::{NodeId, SpaceTree};

/// How many pieces a level shows at most; the smaller rest become one.
pub const PIECES: usize = 48;

/// What a piece stands for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    /// A sub-folder of the level.
    Folder(PathBuf),
    /// "this folder": the files directly in the level.
    ThisFolder,
    /// One file (in This Folder Only).
    File(PathBuf),
    /// The pieces too small to show one by one.
    Rest,
}

/// One piece of the level, ready to be laid out and drawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub key: Key,
    /// "Users", "this folder", "movie.mkv", "12 other items".
    pub label: String,
    pub bytes: u64,
    /// The prevalent file type (its colour).
    pub category: Category,
}

/// One part of the breadcrumbs ("C:\", "Users", "spata").
#[derive(Clone, Debug, PartialEq)]
pub struct Crumb {
    pub label: String,
    pub path: PathBuf,
}

/// A navigation request, from the canvas, the breadcrumbs, the keyboard
/// (#87) or an item action ("Open the folder", #90).
#[derive(Clone, Debug, PartialEq)]
pub enum Move {
    /// A double click on a piece: see [`Nav::activate`].
    Activate(Key),
    /// A click on a piece at a point of the canvas (its coordinates):
    /// selects it and opens its chip there (#223).
    Pick(Key, [f32; 2]),
    /// A breadcrumb.
    Crumb(PathBuf),
    /// "Parent".
    Parent,
    ThisFolderOnly(bool),
    Select(Option<Key>),
    /// "Open the folder" on a file or folder (#90).
    Open(PathBuf),
}

/// The navigation state; the root level when nothing was entered.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Nav {
    level: Option<PathBuf>,
    this_only: bool,
    selected: Option<Key>,
    /// Where the selected piece was clicked (None: chosen otherwise).
    point: Option<[f32; 2]>,
}

impl Nav {
    /// The node of the current level (the root while the folder entered
    /// is not in the tree yet).
    pub fn level(&self, t: &SpaceTree) -> NodeId {
        self.level.as_deref().and_then(|p| t.find(p)).unwrap_or(SpaceTree::ROOT)
    }

    /// Applies one navigation request.
    pub fn go(&mut self, t: &SpaceTree, m: Move) {
        match m {
            Move::Activate(key) => self.activate(&key),
            Move::Crumb(path) => self.enter(path),
            Move::Parent => {
                self.parent(t);
            }
            Move::ThisFolderOnly(on) => self.set_this_folder_only(on),
            Move::Select(key) => self.select(key),
            Move::Pick(key, at) => {
                self.select(Some(key));
                self.point = Some(at);
            }
            Move::Open(path) => self.open(t, &path),
        }
    }

    /// Goes into `folder` (a sub-folder or a breadcrumb).
    pub fn enter(&mut self, folder: PathBuf) {
        self.level = Some(folder);
        self.this_only = false;
        self.select(None);
    }

    /// "Parent": leaves This Folder Only, else goes up one level. False
    /// when already at the root.
    pub fn parent(&mut self, t: &SpaceTree) -> bool {
        if self.this_only {
            self.set_this_folder_only(false);
            return true;
        }
        match t.parent(self.level(t)) {
            Some(up) => {
                self.enter(t.path(up).to_path_buf());
                true
            }
            None => {
                self.level = None;
                false
            }
        }
    }

    /// "Open the folder" (#80): enters `path` when it is a folder of the
    /// tree, else the folder that holds it.
    pub fn open(&mut self, t: &SpaceTree, path: &Path) {
        let folder = if t.find(path).is_some() { Some(path) } else { path.parent().filter(|d| t.find(d).is_some()) };
        if let Some(folder) = folder {
            self.enter(folder.to_path_buf());
        }
    }

    /// What a double click on a piece does: a folder is entered,
    /// "this folder" shows its files, anything else is selected.
    pub fn activate(&mut self, key: &Key) {
        match key {
            Key::Folder(p) => self.enter(p.clone()),
            Key::ThisFolder => self.set_this_folder_only(true),
            _ => self.select(Some(key.clone())),
        }
    }

    /// Whether only the files directly in the level are shown.
    pub fn this_folder_only(&self) -> bool {
        self.this_only
    }

    pub fn set_this_folder_only(&mut self, on: bool) {
        if on != self.this_only {
            self.this_only = on;
            self.select(None);
        }
    }

    pub fn selected(&self) -> Option<&Key> {
        self.selected.as_ref()
    }

    /// Selects `key` (None: nothing), without a click point.
    pub fn select(&mut self, key: Option<Key>) {
        self.selected = key;
        self.point = None;
    }

    /// Where the selected piece was clicked, in canvas coordinates.
    pub fn point(&self) -> Option<[f32; 2]> {
        self.point
    }

    /// The breadcrumbs from the root to the level: the root's full path,
    /// then each folder's name.
    pub fn crumbs(&self, t: &SpaceTree) -> Vec<Crumb> {
        let mut out = Vec::new();
        let mut at = Some(self.level(t));
        while let Some(n) = at {
            let path = t.path(n);
            let label = match (t.parent(n), path.file_name()) {
                (Some(_), Some(name)) => name.to_string_lossy().into_owned(),
                _ => path.display().to_string(),
            };
            out.push(Crumb { label, path });
            at = t.parent(n);
        }
        out.reverse();
        out
    }

    /// The pieces of the level, largest first: its sub-folders and "this
    /// folder" (or, in This Folder Only, its largest files and the rest),
    /// at most [`PIECES`], the smallest folded into "N other items".
    pub fn pieces(&self, t: &SpaceTree) -> Vec<Piece> {
        let n = self.level(t);
        let (loose, _) = t.loose(n);
        let mut all: Vec<Piece> = if self.this_only {
            let files = t.loose_files(n);
            let listed: u64 = files.iter().map(|f| f.1).sum();
            let mut out: Vec<Piece> = files
                .into_iter()
                .map(|(path, bytes)| Piece { label: name_of(&path), category: Category::of(&path), key: Key::File(path), bytes })
                .collect();
            if loose > listed {
                let others = t.loose(n).1 - out.len() as u64;
                out.push(Piece { key: Key::Rest, label: format!("{others} other files"), bytes: loose - listed, category: t.loose_prevalent(n) });
            }
            out
        } else {
            let mut out: Vec<Piece> = t
                .children(n)
                .into_iter()
                .filter(|&c| t.bytes(c) > 0)
                .map(|c| Piece { key: Key::Folder(t.path(c)), label: name_of(&t.path(c)), bytes: t.bytes(c), category: t.prevalent(c) })
                .collect();
            if loose > 0 {
                out.push(Piece { key: Key::ThisFolder, label: "this folder".into(), bytes: loose, category: t.loose_prevalent(n) });
            }
            out
        };
        all.sort_by(|a, b| b.bytes.cmp(&a.bytes));
        if all.len() > PIECES {
            let rest = all.split_off(PIECES - 1);
            let bytes = rest.iter().map(|p| p.bytes).sum();
            all.push(Piece { key: Key::Rest, label: format!("{} other items", rest.len()), bytes, category: Category::Other });
        }
        all
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::disk_space::tree::Event;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn tree() -> SpaceTree {
        let mut t = SpaceTree::new("C:/", 8);
        t.apply(vec![
            Event::File(p("C:/Users/spata/movie.mkv"), 5000),
            Event::File(p("C:/Users/spata/Docs/a.pdf"), 300),
            Event::File(p("C:/Users/note.txt"), 100),
            Event::File(p("C:/Windows/sys.dll"), 2000),
            Event::File(p("C:/pagefile.sys"), 1000),
            Event::File(p("C:/x.bin"), 50),
        ]);
        t
    }

    fn labels(c: &[Crumb]) -> Vec<&str> {
        c.iter().map(|c| c.label.as_str()).collect()
    }

    fn pieces(n: &Nav, t: &SpaceTree) -> Vec<(String, u64, Category)> {
        n.pieces(t).into_iter().map(|p| (p.label, p.bytes, p.category)).collect()
    }

    #[test]
    fn entering_folders_builds_the_breadcrumbs_and_parent_goes_back_up() {
        let t = tree();
        let mut n = Nav::default();
        assert_eq!(labels(&n.crumbs(&t)), ["C:/"]);
        assert!(!n.parent(&t), "nothing above the root");
        n.enter(p("C:/Users"));
        n.enter(p("C:/Users/spata"));
        assert_eq!(t.path(n.level(&t)), Path::new("C:/Users/spata"));
        assert_eq!(labels(&n.crumbs(&t)), ["C:/", "Users", "spata"]);
        assert_eq!(n.crumbs(&t)[1].path, p("C:/Users"));
        assert!(n.parent(&t));
        assert_eq!(labels(&n.crumbs(&t)), ["C:/", "Users"]);
        n.enter(n.crumbs(&t)[0].path.clone());
        assert_eq!(n.level(&t), SpaceTree::ROOT);
    }

    #[test]
    fn a_folder_not_measured_yet_shows_the_root() {
        let t = tree();
        let mut n = Nav::default();
        n.enter(p("C:/Program Files"));
        assert_eq!(n.level(&t), SpaceTree::ROOT);
    }

    #[test]
    fn a_level_holds_its_folders_and_this_folder_largest_first() {
        let t = tree();
        let mut n = Nav::default();
        assert_eq!(
            pieces(&n, &t),
            vec![
                ("Users".to_string(), 5400, Category::Media),
                ("Windows".to_string(), 2000, Category::System),
                ("this folder".to_string(), 1050, Category::System),
            ]
        );
        n.enter(p("C:/Users/spata/Docs"));
        assert_eq!(pieces(&n, &t), vec![("this folder".to_string(), 300, Category::Documents)]);
    }

    #[test]
    fn clicking_a_folder_enters_it_and_clicking_this_folder_shows_its_files() {
        let t = tree();
        let mut n = Nav::default();
        n.activate(&Key::Folder(p("C:/Users")));
        assert_eq!(labels(&n.crumbs(&t)), ["C:/", "Users"]);
        n.parent(&t);
        n.activate(&Key::ThisFolder);
        assert!(n.this_folder_only());
        assert_eq!(n.level(&t), SpaceTree::ROOT);
        let files: Vec<Key> = n.pieces(&t).into_iter().map(|p| p.key).collect();
        assert_eq!(files, vec![Key::File(p("C:/pagefile.sys")), Key::File(p("C:/x.bin"))]);
        assert!(n.parent(&t), "Parent leaves This Folder Only first");
        assert!(!n.this_folder_only());
        assert_eq!(n.level(&t), SpaceTree::ROOT);
    }

    #[test]
    fn clicking_a_file_selects_it_and_moving_clears_the_selection() {
        let mut n = Nav::default();
        n.set_this_folder_only(true);
        n.activate(&Key::File(p("C:/x.bin")));
        assert_eq!(n.selected(), Some(&Key::File(p("C:/x.bin"))));
        n.set_this_folder_only(false);
        assert_eq!(n.selected(), None);
        n.select(Some(Key::Folder(p("C:/Users"))));
        n.enter(p("C:/Users"));
        assert_eq!(n.selected(), None);
    }

    #[test]
    fn open_the_folder_enters_a_folder_or_the_folder_holding_a_file() {
        let t = tree();
        let mut n = Nav::default();
        n.open(&t, Path::new("C:/Users/spata/Docs"));
        assert_eq!(t.path(n.level(&t)), Path::new("C:/Users/spata/Docs"));
        n.open(&t, Path::new("C:/Windows/sys.dll"));
        assert_eq!(t.path(n.level(&t)), Path::new("C:/Windows"));
    }

    #[test]
    fn every_move_goes_through_one_door() {
        let t = tree();
        let mut n = Nav::default();
        n.go(&t, Move::Activate(Key::Folder(p("C:/Users"))));
        n.go(&t, Move::Activate(Key::Folder(p("C:/Users/spata"))));
        n.go(&t, Move::Crumb(p("C:/Users")));
        assert_eq!(t.path(n.level(&t)), Path::new("C:/Users"));
        n.go(&t, Move::Parent);
        assert_eq!(n.level(&t), SpaceTree::ROOT);
        n.go(&t, Move::ThisFolderOnly(true));
        assert!(n.this_folder_only());
        n.go(&t, Move::Open(p("C:/Users/spata/movie.mkv")));
        assert_eq!(t.path(n.level(&t)), Path::new("C:/Users/spata"));
        n.go(&t, Move::Select(Some(Key::ThisFolder)));
        assert_eq!(n.selected(), Some(&Key::ThisFolder));
    }

    #[test]
    fn a_crowded_level_folds_the_smallest_pieces_into_one() {
        let mut t = SpaceTree::new("R", 8);
        for i in 0..PIECES + 5 {
            t.add_file(&p(&format!("R/d{i:03}/f")), 1000 - i as u64);
        }
        let all = Nav::default().pieces(&t);
        assert_eq!(all.len(), PIECES);
        let rest = all.last().unwrap();
        assert_eq!(rest.key, Key::Rest);
        assert_eq!(rest.label, "6 other items");
        let shown: u64 = all.iter().map(|p| p.bytes).sum();
        assert_eq!(shown, t.bytes(SpaceTree::ROOT));
    }
}
