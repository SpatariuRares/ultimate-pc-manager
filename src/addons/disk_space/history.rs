//! The scan history: every finished scan saved as a snapshot file with
//! fixed field names (id, rootPath, started, finished, totalBytes,
//! fileCount, categoryBytes, categoryCounts, unreadableCount, folders,
//! files), cut to stay under 16 MiB by keeping the upper levels and the
//! largest folders and files.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use super::kind::Category;
use super::tree::{SavedFolder, SpaceTree};

/// The most a snapshot file may take.
pub const LIMIT: usize = 16 * 1024 * 1024;

pub const TOO_BIG: &str = "Couldn't save scan history because the retained result exceeds the 16 MiB safety limit.";

/// A saved scan, as written in its file.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    /// The scan's id, shaped as a UUID (also its file name).
    pub id: String,
    /// The drive or folder measured ("All disks" for every drive).
    pub root_path: String,
    /// Local seconds since 1970 (written as ISO-8601 local time).
    #[serde(with = "iso")]
    pub started: i64,
    /// When it finished, as `started`.
    #[serde(with = "iso")]
    pub finished: i64,
    /// Bytes counted under the root.
    pub total_bytes: u64,
    /// Files counted under the root.
    pub file_count: u64,
    /// Bytes by category name.
    pub category_bytes: BTreeMap<String, u64>,
    /// Files by category name.
    pub category_counts: BTreeMap<String, u64>,
    /// Folders that could not be read; None when the file does not say
    /// (the note then says "Some items could not be read").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unreadable_count: Option<u64>,
    /// Up to eight unreadable folders, for the skipped-items note.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unreadable_examples: Vec<String>,
    /// Upper levels first, the largest first within a level.
    pub folders: Vec<FolderEntry>,
    /// The scan's largest files, then the largest of each folder kept.
    pub files: Vec<FileEntry>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FolderEntry {
    pub path: String,
    pub bytes: u64,
    pub depth: usize,
    /// Files and unreadable folders under it (0 when missing).
    #[serde(default)]
    pub files: u64,
    #[serde(default)]
    pub unreadable: u64,
    /// Bytes and files per category under it (empty when missing),
    /// so a loaded scan keeps each folder's prevalent type and legend.
    #[serde(rename = "categoryBytes", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub category_bytes: BTreeMap<String, u64>,
    #[serde(rename = "categoryCounts", default, skip_serializing_if = "BTreeMap::is_empty")]
    pub category_counts: BTreeMap<String, u64>,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FileEntry {
    pub path: String,
    pub bytes: u64,
    pub category: String,
}

fn text(p: &std::path::Path) -> String {
    p.display().to_string()
}

/// The length an entry adds to the file (with its comma).
fn cost<T: serde::Serialize>(entry: &T) -> usize {
    serde_json::to_string(entry).map_or(usize::MAX / 4, |s| s.len() + 1)
}

impl Snapshot {
    /// The snapshot of a finished scan, cut to fit `limit` bytes: the
    /// scan's largest files first, then folder after folder by level
    /// (largest first) with their own largest files, until the next one
    /// would not fit.
    pub fn of(tree: &SpaceTree, id: &str, started: i64, finished: i64, limit: usize) -> Snapshot {
        let root = SpaceTree::ROOT;
        let mut s = Snapshot {
            id: id.into(),
            root_path: text(&tree.path(root)),
            started,
            finished,
            total_bytes: tree.bytes(root),
            file_count: tree.files(root),
            category_bytes: Category::ALL.iter().map(|&c| (c.name().to_string(), tree.category(root, c).0)).collect(),
            category_counts: Category::ALL.iter().map(|&c| (c.name().to_string(), tree.category(root, c).1)).collect(),
            unreadable_count: Some(tree.unreadable(root)),
            unreadable_examples: tree.unreadable_examples().iter().map(|p| text(p)).collect(),
            folders: Vec::new(),
            files: Vec::new(),
        };
        let mut left = limit.saturating_sub(cost(&s));
        let file = |(p, b): &(PathBuf, u64)| FileEntry { path: text(p), bytes: *b, category: Category::of(p).name().into() };
        let mut taken: HashSet<PathBuf> = HashSet::new();
        for f in tree.largest_files(root, usize::MAX) {
            let e = file(&f);
            let c = cost(&e);
            if c > left {
                break;
            }
            left -= c;
            taken.insert(f.0);
            s.files.push(e);
        }
        let mut order: Vec<(usize, SavedFolder)> = tree.saved_folders().into_iter().enumerate().collect();
        order.sort_by(|(_, a), (_, b)| a.depth.cmp(&b.depth).then(b.bytes.cmp(&a.bytes)));
        for (n, f) in order {
            let (category_bytes, category_counts) = kind_maps(&f.kinds);
            let entry = FolderEntry { path: text(&f.path), bytes: f.bytes, depth: f.depth, files: f.files, unreadable: f.unreadable, category_bytes, category_counts };
            let own: Vec<FileEntry> = tree.own_files(n).iter().filter(|f| !taken.contains(&f.0)).map(file).collect();
            let c = cost(&entry) + own.iter().map(cost).sum::<usize>();
            if c > left {
                break;
            }
            left -= c;
            s.folders.push(entry);
            s.files.extend(own);
        }
        s
    }

    /// The scan's tree, as far as it was saved, keeping `keep` items in
    /// its largest lists (the "list length" setting, as a live scan).
    pub fn tree(&self, keep: usize) -> SpaceTree {
        let folders: Vec<SavedFolder> = self
            .folders
            .iter()
            .map(|f| SavedFolder { path: PathBuf::from(&f.path), bytes: f.bytes, files: f.files, unreadable: f.unreadable, depth: f.depth, kinds: kind_array(&f.category_bytes, &f.category_counts) })
            .collect();
        let files: Vec<(PathBuf, u64)> = self.files.iter().map(|f| (PathBuf::from(&f.path), f.bytes)).collect();
        let kinds = kind_array(&self.category_bytes, &self.category_counts);
        let root = SavedFolder { path: PathBuf::from(&self.root_path), bytes: self.total_bytes, files: self.file_count, unreadable: self.unreadable_count.unwrap_or(0), depth: 0, kinds };
        let all: Vec<SavedFolder> = std::iter::once(root).chain(folders).collect();
        SpaceTree::restore(&self.root_path, keep, &all, &files, kinds, self.unreadable_examples.iter().map(PathBuf::from).collect())
    }
}

/// Per-category (bytes, files) from the saved maps, in `Category::ALL` order.
fn kind_array(bytes: &BTreeMap<String, u64>, counts: &BTreeMap<String, u64>) -> [(u64, u64); 7] {
    Category::ALL.map(|c| (bytes.get(c.name()).copied().unwrap_or(0), counts.get(c.name()).copied().unwrap_or(0)))
}

/// The saved maps of per-category (bytes, files), leaving out empty categories.
fn kind_maps(kinds: &[(u64, u64); 7]) -> (BTreeMap<String, u64>, BTreeMap<String, u64>) {
    let mut bytes = BTreeMap::new();
    let mut counts = BTreeMap::new();
    for (c, &(b, n)) in Category::ALL.iter().zip(kinds) {
        if b > 0 {
            bytes.insert(c.name().to_string(), b);
        }
        if n > 0 {
            counts.insert(c.name().to_string(), n);
        }
    }
    (bytes, counts)
}

/// The text of a snapshot file.
pub fn encode(s: &Snapshot) -> Result<String, String> {
    let text = serde_json::to_string(s).map_err(|e| e.to_string())?;
    if text.len() > LIMIT {
        return Err(TOO_BIG.into());
    }
    Ok(text)
}

/// Reads a snapshot file; anything malformed or inconsistent is an error.
pub fn decode(text: &str) -> Result<Snapshot, String> {
    let s: Snapshot = serde_json::from_str(text).map_err(|e| format!("invalid JSON: {e}"))?;
    if let Some(f) = s.files.iter().find(|f| !Category::ALL.iter().any(|c| c.name() == f.category)) {
        return Err(format!("a file entry names the unknown category \"{}\"", f.category));
    }
    // A folder holding less than its sub-folders would make "this
    // folder" negative: such a file is not a scan we saved.
    if !s.tree(0).consistent() {
        return Err("a folder holds less than its sub-folders".into());
    }
    Ok(s)
}

pub const NO_FOLDER: &str = "Couldn't locate the scan history folder.";
pub const UNREADABLE_FOLDER: &str = "The scan history folder could not be read.";
pub const BAD_ID: &str = "Couldn't save scan history because the scan id is malformed.";
pub const TOO_MANY: &str = "Couldn't save scan history because its folder contains too many entries to manage safely.";
pub const NOT_WRITTEN: &str = "Couldn't save scan history: the snapshot file could not be written.";
/// Where History is kept unless the "History folder" setting says otherwise.
pub const DEFAULT_FOLDER: &str = "%APPDATA%\\UltimatePCManager\\DiskScans";
/// History's caption while it is empty.
pub const EMPTY: &str = "Finished scans are saved here.";
/// The note beside Start.
pub const SAVED_NOTE: &str = "Saved to History when it finishes";
pub const CLEAR_TITLE: &str = "Clear the scan history?";

/// How many files the history folder may hold before saving stops.
const MAX_ENTRIES: usize = 10_000;

/// "%1 saved scans could not be read and were left out of History."
pub fn left_out(n: u64) -> String {
    if n == 1 {
        "One saved scan could not be read and was left out of History.".into()
    } else {
        format!("{n} saved scans could not be read and were left out of History.")
    }
}

/// The text of the Clear History confirmation.
pub fn clear_text(n: usize) -> String {
    if n == 1 {
        "One saved scan will be removed. This cannot be undone.".into()
    } else {
        format!("{n} saved scans will be removed. This cannot be undone.")
    }
}

/// What History lists of a saved scan (its small `.summary.json` file).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub id: String,
    pub root_path: String,
    #[serde(with = "iso")]
    pub started: i64,
    #[serde(with = "iso")]
    pub finished: i64,
    pub total_bytes: u64,
    pub file_count: u64,
    pub unreadable_count: u64,
}

impl Snapshot {
    pub fn summary(&self) -> Summary {
        Summary {
            id: self.id.clone(),
            root_path: self.root_path.clone(),
            started: self.started,
            finished: self.finished,
            total_bytes: self.total_bytes,
            file_count: self.file_count,
            unreadable_count: self.unreadable_count.unwrap_or(0),
        }
    }
}

/// The history as listed: the saved scans (newest first), how many could
/// not be read, and an error about the folder itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Shelf {
    pub entries: Vec<Summary>,
    pub left_out: u64,
    pub error: Option<String>,
}

/// A new scan id, shaped as a UUID.
pub fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()) as u64;
    let n = COUNT.fetch_add(1, Ordering::Relaxed);
    let a = nanos ^ (u64::from(std::process::id()) << 32);
    let b = n.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ nanos.rotate_left(17);
    format!("{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}", a >> 32, (a >> 16) & 0xffff, a & 0xfff, 0x8000 | (b >> 48) & 0x3fff, b & 0xffff_ffff_ffff)
}

/// An id that is safe as a file name: 8-4-4-4-12 hex digits.
fn valid_id(id: &str) -> bool {
    id.len() == 36 && id.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
}

/// The folder of saved scans: `<id>.json` (the snapshot) and
/// `<id>.summary.json` (what History lists) for each.
#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Store { dir: dir.into() }
    }

    /// The app's history folder, beside its settings.
    #[cfg_attr(test, allow(dead_code))]
    pub fn app() -> Result<Store, String> {
        let base = std::env::var_os("APPDATA").ok_or_else(|| NO_FOLDER.to_string())?;
        Ok(Store::new(std::path::Path::new(&base).join(crate::settings::DATA_FOLDER).join("DiskScans")))
    }

    /// Writes a finished scan; gives what History lists of it.
    pub fn save(&self, s: &Snapshot) -> Result<Summary, String> {
        if !valid_id(&s.id) {
            return Err(BAD_ID.into());
        }
        let text = encode(s)?;
        std::fs::create_dir_all(&self.dir).map_err(|_| NOT_WRITTEN.to_string())?;
        if std::fs::read_dir(&self.dir).map_err(|_| UNREADABLE_FOLDER.to_string())?.count() >= MAX_ENTRIES {
            return Err(TOO_MANY.into());
        }
        let summary = s.summary();
        let write = |name: String, text: &str| -> std::io::Result<()> {
            let tmp = self.dir.join(format!("{name}.tmp"));
            std::fs::write(&tmp, text)?;
            std::fs::rename(&tmp, self.dir.join(name))
        };
        let small = serde_json::to_string(&summary).map_err(|_| NOT_WRITTEN.to_string())?;
        write(format!("{}.json", s.id), &text).and_then(|_| write(format!("{}.summary.json", s.id), &small)).map_err(|_| NOT_WRITTEN.to_string())?;
        Ok(summary)
    }

    /// The saved scans, newest first; a scan whose files cannot be read
    /// is counted and left out.
    pub fn list(&self) -> Shelf {
        let mut shelf = Shelf::default();
        let dir = match std::fs::read_dir(&self.dir) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return shelf,
            Err(_) => {
                shelf.error = Some(UNREADABLE_FOLDER.into());
                return shelf;
            }
        };
        let names: HashSet<String> = dir.filter_map(|e| e.ok()?.file_name().into_string().ok()).collect();
        let mut ids: Vec<&str> = names.iter().filter_map(|n| n.strip_suffix(".json")).map(|n| n.strip_suffix(".summary").unwrap_or(n)).collect();
        ids.sort_unstable();
        ids.dedup();
        for id in ids {
            let summary = std::fs::read_to_string(self.dir.join(format!("{id}.summary.json")))
                .ok()
                .and_then(|t| serde_json::from_str::<Summary>(&t).ok())
                .filter(|s| s.id == id && names.contains(&format!("{id}.json")));
            match summary {
                Some(s) => shelf.entries.push(s),
                None => shelf.left_out += 1,
            }
        }
        shelf.entries.sort_by(|a, b| b.finished.cmp(&a.finished).then_with(|| b.id.cmp(&a.id)));
        shelf
    }

    /// One saved scan, whole.
    pub fn load(&self, id: &str) -> Result<Snapshot, String> {
        if !valid_id(id) {
            return Err(BAD_ID.into());
        }
        let text = std::fs::read_to_string(self.dir.join(format!("{id}.json"))).map_err(|e| e.to_string())?;
        decode(&text)
    }

    /// Removes the oldest saved scans past the newest `keep`.
    pub fn prune(&self, keep: usize) -> Result<(), String> {
        let shelf = self.list();
        if let Some(e) = shelf.error {
            return Err(e);
        }
        for old in shelf.entries.iter().skip(keep) {
            for name in [format!("{}.json", old.id), format!("{}.summary.json", old.id)] {
                std::fs::remove_file(self.dir.join(name)).map_err(|_| UNREADABLE_FOLDER.to_string())?;
            }
        }
        Ok(())
    }

    /// Removes every saved scan.
    pub fn clear(&self) -> Result<(), String> {
        let dir = match std::fs::read_dir(&self.dir) {
            Ok(d) => d,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(UNREADABLE_FOLDER.into()),
        };
        for e in dir.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".json") || name.ends_with(".tmp") {
                std::fs::remove_file(e.path()).map_err(|_| UNREADABLE_FOLDER.to_string())?;
            }
        }
        Ok(())
    }
}

/// `started` / `finished` as ISO-8601 text.
mod iso {
    pub fn serialize<S: serde::Serializer>(secs: &i64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&crate::pro::iso_text(*secs))
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
        let t: String = serde::Deserialize::deserialize(d)?;
        crate::pro::iso_secs(&t).ok_or_else(|| serde::de::Error::custom(format!("\"{t}\" is not an ISO-8601 date")))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::kind::Category;
    use super::super::tree::Event;
    use super::*;
    use std::path::Path;

    const ID: &str = "0b5c3f7e-1d2a-4c6b-9e8f-112233445566";

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    fn sample() -> SpaceTree {
        let mut t = SpaceTree::new("R:\\", 8);
        t.apply(vec![
            Event::File(p("R:\\big\\movie.mkv"), 5000),
            Event::File(p("R:\\big\\deep\\clip.mp4"), 3000),
            Event::File(p("R:\\small\\note.txt"), 100),
            Event::File(p("R:\\loose.bin"), 700),
            Event::Unreadable(p("R:\\locked")),
        ]);
        t
    }

    /// What the page reads from a tree.
    fn reading(t: &SpaceTree) -> (u64, u64, u64, Vec<(PathBuf, u64)>, Vec<(PathBuf, u64)>, Vec<(u64, u64)>, Vec<PathBuf>) {
        let r = SpaceTree::ROOT;
        (
            t.bytes(r),
            t.files(r),
            t.unreadable(r),
            t.largest_folders(r, 8),
            t.largest_files(r, 8),
            Category::ALL.iter().map(|&c| t.category(r, c)).collect(),
            t.unreadable_examples().to_vec(),
        )
    }

    #[test]
    fn a_scan_comes_back_from_its_snapshot_as_it_was() {
        let t = sample();
        let s = Snapshot::of(&t, ID, 1_000, 1_142, LIMIT);
        let back = decode(&encode(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert_eq!((back.started, back.finished), (1_000, 1_142));
        let again = back.tree(8);
        assert_eq!(reading(&again), reading(&t));
        let big = again.find(Path::new("R:\\big")).unwrap();
        assert_eq!(again.bytes(big), 8000);
        assert_eq!(again.prevalent(again.find(Path::new("R:\\small")).unwrap()), Category::Documents, "a sub-folder keeps its prevalent type");
        assert_eq!(again.category(big, Category::Media), (8000, 2));
        assert_eq!(again.largest_folders(big, 8), vec![(p("R:\\big\\deep"), 3000)]);
        assert_eq!(again.largest_files(big, 8), vec![(p("R:\\big\\movie.mkv"), 5000), (p("R:\\big\\deep\\clip.mp4"), 3000)]);
    }

    #[test]
    fn an_all_disks_scan_comes_back_with_its_drives() {
        let mut t = SpaceTree::virtual_root("All disks", &["C:\\", "D:\\"], 8);
        t.apply(vec![Event::File(p("C:\\a\\x.bin"), 600), Event::File(p("D:\\y.bin"), 40)]);
        let back = decode(&encode(&Snapshot::of(&t, ID, 0, 1, LIMIT)).unwrap()).unwrap().tree(8);
        assert_eq!(reading(&back), reading(&t));
        let c = back.find(Path::new("C:\\")).unwrap();
        assert_eq!(back.largest_folders(c, 8), vec![(p("C:\\a"), 600)]);
    }

    #[test]
    fn a_snapshot_too_large_keeps_the_upper_levels_and_the_largest() {
        let mut t = SpaceTree::new("R:\\", 8);
        for i in 0..20u64 {
            for j in 0..20u64 {
                t.add_file(&p(&format!("R:\\top{i:02}\\sub{j:02}\\deep\\file.bin")), 1000 + i * 100 + j);
            }
        }
        let full = encode(&Snapshot::of(&t, ID, 0, 1, LIMIT)).unwrap().len();
        let limit = full / 5;
        let s = Snapshot::of(&t, ID, 0, 1, limit);
        let text = encode(&s).unwrap();
        assert!(text.len() <= limit, "{} > {limit}", text.len());
        let depths: Vec<usize> = s.folders.iter().map(|f| f.depth).collect();
        assert!(depths.windows(2).all(|w| w[0] <= w[1]), "upper levels first");
        assert_eq!(depths.iter().filter(|&&d| d == 1).count(), 20, "every top folder is kept");
        assert!(depths.iter().all(|&d| d <= 2), "the deepest level goes first: {depths:?}");
        let level2: Vec<&FolderEntry> = s.folders.iter().filter(|f| f.depth == 2).collect();
        assert!(!level2.is_empty() && level2.len() < 400);
        let smallest_kept = level2.iter().map(|f| f.bytes).min().unwrap();
        assert!(level2.iter().any(|f| f.path.starts_with("R:\\top19")), "the largest folders are kept");
        assert!(smallest_kept > 1019, "the smallest folders (top00's) are dropped first");
        assert_eq!(s.files[..8].iter().map(|f| f.bytes).collect::<Vec<_>>(), (0..8).map(|k| 2919 - k).collect::<Vec<_>>(), "the largest files are kept");
        let back = decode(&text).unwrap().tree(8);
        assert_eq!((back.bytes(SpaceTree::ROOT), back.files(SpaceTree::ROOT)), (t.bytes(SpaceTree::ROOT), 400), "the totals stay whole");
    }

    #[test]
    fn a_huge_scan_is_saved_under_16_mib() {
        let mut t = SpaceTree::new("R:\\", 8);
        let long = "x".repeat(180);
        for i in 0..60_000u64 {
            t.add_file(&p(&format!("R:\\{}\\{long}{i:06}\\{long}.bin", i % 50)), i);
        }
        let text = encode(&Snapshot::of(&t, ID, 0, 1, LIMIT)).unwrap();
        assert!(text.len() <= LIMIT && text.len() > LIMIT - 1024 * 1024, "{}", text.len());
        assert_eq!(decode(&text).unwrap().file_count, 60_000);
    }

    /// A fresh history folder under the temp dir, removed when dropped.
    pub(crate) struct TempStore(pub Store);
    impl TempStore {
        pub fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("mtm-disk-history-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            TempStore(Store::new(dir))
        }
    }
    impl Drop for TempStore {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0.dir);
        }
    }

    #[test]
    fn saved_scans_are_listed_newest_first_and_load_back() {
        let store = TempStore::new("list");
        assert_eq!(store.0.list(), Shelf::default(), "no folder yet: an empty history");
        let older = Snapshot::of(&sample(), &new_id(), 100, 200, LIMIT);
        let newer = Snapshot::of(&sample(), &new_id(), 300, 400, LIMIT);
        assert_ne!(older.id, newer.id);
        store.0.save(&older).unwrap();
        let summary = store.0.save(&newer).unwrap();
        assert_eq!(summary, Summary { id: newer.id.clone(), root_path: "R:\\".into(), started: 300, finished: 400, total_bytes: 8800, file_count: 4, unreadable_count: 1 });
        let shelf = store.0.list();
        assert_eq!(shelf.entries.iter().map(|e| e.finished).collect::<Vec<_>>(), vec![400, 200]);
        assert_eq!((shelf.left_out, shelf.error), (0, None));
        assert_eq!(store.0.load(&older.id).unwrap(), older);
    }

    #[test]
    fn a_corrupt_saved_scan_is_counted_and_left_out() {
        let store = TempStore::new("corrupt");
        let good = Snapshot::of(&sample(), &new_id(), 100, 200, LIMIT);
        let bad = Snapshot::of(&sample(), &new_id(), 300, 400, LIMIT);
        store.0.save(&good).unwrap();
        store.0.save(&bad).unwrap();
        std::fs::write(store.0.dir.join(format!("{}.summary.json", bad.id)), "{ not json").unwrap();
        let shelf = store.0.list();
        assert_eq!(shelf.entries.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec![good.id.as_str()]);
        assert_eq!(shelf.left_out, 1);
        assert_eq!(left_out(shelf.left_out), "One saved scan could not be read and was left out of History.");
        assert_eq!(left_out(3), "3 saved scans could not be read and were left out of History.");

        std::fs::write(store.0.dir.join(format!("{}.json", good.id)), "{\"id\": 5}").unwrap();
        assert!(store.0.load(&good.id).is_err(), "a corrupt snapshot does not load");
    }

    #[test]
    fn a_saved_scan_whose_folder_is_smaller_than_its_sub_folders_does_not_load() {
        let store = TempStore::new("inconsistent");
        let at = |s: &Snapshot, path: &str| s.folders.iter().position(|f| f.path == path).unwrap();
        let mut bytes = Snapshot::of(&sample(), &new_id(), 100, 200, LIMIT);
        let big = at(&bytes, "R:\\big");
        bytes.folders[big].bytes = 100;
        let mut files = Snapshot::of(&sample(), &new_id(), 100, 200, LIMIT);
        let big = at(&files, "R:\\big");
        files.folders[big].files = 0;
        for s in [&bytes, &files] {
            store.0.save(s).unwrap();
            assert!(store.0.load(&s.id).is_err(), "{s:?} loads");
        }
        let good = Snapshot::of(&sample(), &new_id(), 300, 400, LIMIT);
        store.0.save(&good).unwrap();
        assert_eq!(store.0.load(&good.id).unwrap(), good, "a consistent one still loads");
    }

    #[test]
    fn clearing_removes_every_saved_scan() {
        let store = TempStore::new("clear");
        store.0.save(&Snapshot::of(&sample(), &new_id(), 100, 200, LIMIT)).unwrap();
        store.0.save(&Snapshot::of(&sample(), &new_id(), 300, 400, LIMIT)).unwrap();
        assert_eq!(clear_text(2), "2 saved scans will be removed. This cannot be undone.");
        assert_eq!(clear_text(1), "One saved scan will be removed. This cannot be undone.");
        store.0.clear().unwrap();
        assert_eq!(store.0.list(), Shelf::default());
    }

    #[test]
    fn a_malformed_id_is_never_written() {
        let store = TempStore::new("bad-id");
        let s = Snapshot::of(&sample(), "..\\..\\evil", 100, 200, LIMIT);
        assert_eq!(store.0.save(&s), Err("Couldn't save scan history because the scan id is malformed.".into()));
        assert!(!store.0.dir.exists());
    }

    /// Saves a scan of the project folder into the real history folder
    /// (%APPDATA%\UltimatePCManager\DiskScans), reads it back, then removes it.
    #[test]
    #[ignore]
    fn saves_and_reloads_from_the_real_history_folder() {
        let project = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut t = SpaceTree::new(project, super::super::LARGEST);
        super::super::scan::walk(project, &|| true, &mut |events| t.apply(events));
        let now = crate::pro::local_secs();
        let s = Snapshot::of(&t, &new_id(), now - 5, now, LIMIT);
        let store = Store::app().unwrap();
        store.save(&s).unwrap();
        let listed = store.list().entries.iter().any(|e| e.id == s.id);
        let back = store.load(&s.id);
        for name in [format!("{}.json", s.id), format!("{}.summary.json", s.id)] {
            let _ = std::fs::remove_file(store.dir.join(name));
        }
        println!("{}: {} folders, {} files saved", store.dir.display(), s.folders.len(), s.files.len());
        assert!(listed, "History lists it");
        let back = back.unwrap();
        assert_eq!(back, s);
        assert_eq!(reading(&back.tree(super::super::LARGEST)), reading(&t));
    }

    #[test]
    fn the_snapshot_uses_fixed_field_names() {
        let text = encode(&Snapshot::of(&sample(), ID, 0, 60, LIMIT)).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in ["id", "rootPath", "started", "finished", "totalBytes", "fileCount", "categoryBytes", "categoryCounts", "unreadableCount", "folders", "files"] {
            assert!(v.get(key).is_some(), "{key} missing in {text}");
        }
        assert_eq!(v["started"], "1970-01-01T00:00:00");
        assert_eq!(v["finished"], "1970-01-01T00:01:00");
        assert_eq!(v["categoryBytes"]["Media"], 8000);
        assert_eq!(v["categoryCounts"]["Documents"], 1);
        assert_eq!(v["folders"][0], serde_json::json!({ "path": "R:\\", "bytes": 8800, "depth": 0, "files": 4, "unreadable": 1, "categoryBytes": { "Documents": 100, "Media": 8000, "Other": 700 }, "categoryCounts": { "Documents": 1, "Media": 2, "Other": 1 } }));
        assert_eq!(v["files"][0], serde_json::json!({ "path": "R:\\big\\movie.mkv", "bytes": 5000, "category": "Media" }));
    }
}
