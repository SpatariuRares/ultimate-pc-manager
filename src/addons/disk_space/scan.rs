//! The scanner: walks a folder on the calling thread and reports what it
//! finds as [`Event`]s in batches. Sizes are the space each file takes
//! on disk (allocated size, owner decision on #80), read from the
//! directory listing itself (no file is opened). Junctions (mount
//! points), symbolic links and other links are not followed and are
//! skipped silently; cloud-file (OneDrive), deduplication and
//! container-isolation folders are descended. A root that is itself a
//! link is measured through it. A file with several hard links is
//! counted once, at the first link met (see [`LINK_CAP`]).

use std::path::{Path, PathBuf};

use super::tree::Event;

/// How many events go to the sink at once.
pub const BATCH: usize = 4096;

/// One entry of a folder listing.
struct Entry {
    name: std::ffi::OsString,
    folder: bool,
    /// A folder that is a link not to follow (see [`descends`]).
    link: bool,
    /// Space taken on disk, in bytes.
    size: u64,
    /// For a file: its volume serial number and file ID (see [`Links`]),
    /// when read.
    id: Option<FileKey>,
}

/// A file's volume serial number and 128-bit file ID: the same for every
/// hard link of the file.
type FileKey = (u32, [u64; 2]);

/// How many hard-linked files a scan tracks: past it, further links are counted again and the scan says so.
pub const LINK_CAP: usize = 262_144;

/// The file key of a listed file with volume serial number `serial` and
/// file ID `id`; None for a zero or invalid ID (FILE_INVALID_FILE_ID),
/// which some file systems give: such a file is never deduplicated.
#[allow(dead_code)]
fn file_key(serial: u32, id: u128) -> Option<FileKey> {
    (id != 0 && id != u64::MAX as u128 && id != u128::MAX).then(|| (serial, [id as u64, (id >> 64) as u64]))
}

/// How many independently locked parts the met files are split into, so
/// the listers seldom wait for each other.
const SHARDS: usize = 64;

/// The files met so far, shared by the listers. No directory listing
/// gives a file's link count, and asking it per file doubled the scan
/// time of C:\Windows (#232), so every file's ID is remembered: one met
/// again is a further hard link and adds nothing, so a hard-linked file
/// counts at its first link. At most [`LINK_CAP`] hard-linked files are tracked; past that their
/// further links are counted again and the scan says so.
///
/// Memory: [`LINK_CAP`] bounds only the tracked hard-linked files; every
/// file met is remembered, so the memory grows with the files scanned.
/// A file of the walk's volume with a 64-bit ID (NTFS) is remembered by
/// that ID alone, exactly: measured 19.5 bytes per file, about 78 MB for
/// the 4 million files of a full C: (69 bytes, 278 MB, with whole keys
/// behind one lock); others (another volume, a 128-bit ReFS ID) keep
/// their whole key. The listers lock only the part holding the file.
struct Links {
    cap: usize,
    /// The volume serial number of the walk's first file met.
    volume: std::sync::OnceLock<u32>,
    /// The files met, split by file ID.
    shards: Box<[std::sync::Mutex<Shard>]>,
    /// How many hard-linked files are tracked.
    tracked: std::sync::atomic::AtomicUsize,
    /// The tracking limit was reached.
    full: std::sync::atomic::AtomicBool,
}

/// One part of the files met.
#[derive(Default)]
struct Shard {
    /// Files of the walk's volume with a 64-bit ID, by that ID.
    near: Met<u64>,
    /// Any other file, by its whole key.
    far: Met<FileKey>,
}

/// Files met, and those of them that are tracked hard-linked files.
struct Met<K> {
    all: std::collections::HashSet<K, Mix>,
    tracked: std::collections::HashSet<K, Mix>,
}

impl<K> Default for Met<K> {
    fn default() -> Self {
        Met { all: Default::default(), tracked: Default::default() }
    }
}

impl<K: std::hash::Hash + Eq + Copy> Met<K> {
    /// Whether this link of `key` is counted: the first one met, or a
    /// further one of a file `track` could not start tracking.
    fn counts(&mut self, key: K, track: impl FnOnce() -> bool) -> bool {
        if self.all.insert(key) {
            return true;
        }
        if self.tracked.contains(&key) {
            return false;
        }
        if track() {
            self.tracked.insert(key);
            return false;
        }
        true
    }
}

/// A fast hash for file IDs (the SplitMix64 finaliser), which needs no
/// protection against chosen keys.
type Mix = std::hash::BuildHasherDefault<MixHasher>;

#[derive(Default)]
struct MixHasher(u64);

impl std::hash::Hasher for MixHasher {
    fn finish(&self) -> u64 {
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(b as u64);
        }
    }

    fn write_u32(&mut self, n: u32) {
        self.write_u64(n as u64);
    }

    fn write_u64(&mut self, n: u64) {
        self.0 = self.finish().rotate_left(17) ^ n;
    }
}

impl Links {
    fn new(cap: usize) -> Links {
        Links {
            cap,
            volume: Default::default(),
            shards: (0..SHARDS).map(|_| Default::default()).collect(),
            tracked: Default::default(),
            full: Default::default(),
        }
    }

    /// Whether this link of the file `id` is counted: the first one met,
    /// or any further one once the tracking is full. Reaching the limit
    /// adds [`Event::HardLinkLimit`] to `events`, once per walk.
    fn counts(&self, id: FileKey, events: &mut Vec<Event>) -> bool {
        use std::sync::atomic::Ordering::SeqCst;
        let (serial, [low, high]) = id;
        // Every decision about one file is taken under its shard's lock.
        let mut shard = self.shards[low as usize % SHARDS].lock().unwrap_or_else(|e| e.into_inner());
        let track = || {
            if self.tracked.fetch_update(SeqCst, SeqCst, |n| (n < self.cap).then_some(n + 1)).is_ok() {
                return true;
            }
            if !self.full.swap(true, SeqCst) {
                events.push(Event::HardLinkLimit);
            }
            false
        };
        if high == 0 && *self.volume.get_or_init(|| serial) == serial {
            shard.near.counts(low, track)
        } else {
            shard.far.counts(id, track)
        }
    }
}

/// A folder that could not be listed: [`Event::Denied`] when access was
/// denied (readable only by the system), else [`Event::Unreadable`]. Asked
/// once more only for such a folder, so the walk does not slow down.
fn unreadable(dir: PathBuf) -> Event {
    match std::fs::read_dir(crate::long_path::long_path(&dir)) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => Event::Denied(dir),
        _ => Event::Unreadable(dir),
    }
}

/// How many threads list folders at once. Listing is mostly system
/// calls, so several listers keep the disk and the cores busy where one
/// waits on each folder in turn (measured on C:\Windows, see the
/// architecture notes).
pub const WORKERS: usize = 4;

/// How many folder listings may wait for the calling thread at once.
/// Past that the listers wait too, so a slow sink holds the walk back
/// instead of letting its results pile up in memory.
pub const PENDING: usize = 64;

/// Walks `root`, handing the files found and the folders that could not
/// be listed to `sink` (on the calling thread) in batches of up to
/// [`BATCH`] events (up to [`PENDING`] listings wait for a slow `sink`;
/// then the listers wait too). [`WORKERS`] threads list the folders, at a lower
/// CPU priority so the rest of the PC (and the app's own page) comes
/// first. Each checks `keep_going` before each folder (which may wait
/// there, for a pause) and the walk stops, reporting nothing more, once
/// it is false. Folders that are links are skipped (see [`descends`]);
/// `root` itself is listed through its link when it is one.
pub fn walk(root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    walk_with(WORKERS, root, keep_going, sink);
}

/// The folders still to list and how many listers are busy: the walk
/// ends when both are none.
struct Queue {
    folders: Vec<PathBuf>,
    busy: usize,
    stopped: bool,
}

/// [`walk`] with `workers` listing threads. Bench seam.
pub fn walk_with(workers: usize, root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    walk_links(workers, Some(LINK_CAP), root, keep_going, sink);
}

/// [`walk_with`] tracking at most `cap` hard-linked files, or reading no
/// link counts at all (each link counted, as before #232) when None.
/// Test and bench seam.
pub fn walk_links(workers: usize, cap: Option<usize>, root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    walk_faulty(workers, cap, Faults::default(), root, keep_going, sink);
}

/// [`walk`] where the listing of folder `dir` fails once `after` entries
/// were read, as a disk or network error part-way would. Test seam.
#[cfg(test)]
pub fn walk_failing(dir: &Path, after: usize, root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    walk_faulty(WORKERS, Some(LINK_CAP), Faults { fail: Some((dir.to_path_buf(), after)), ..Faults::default() }, root, keep_going, sink);
}

/// [`walk`] on a volume that rejects the listing with file IDs, as FAT
/// and some network or virtual drives do. Test seam.
#[cfg(test)]
pub fn walk_without_file_ids(root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    walk_faulty(WORKERS, Some(LINK_CAP), Faults { no_ids: true, ..Faults::default() }, root, keep_going, sink);
}

/// Failures injected into a walk (test seams).
#[derive(Clone, Default)]
struct Faults {
    /// The listing of this folder fails once that many entries were read
    /// (see [`walk_failing`]).
    fail: Option<(PathBuf, usize)>,
    /// Every listing with file IDs is rejected (see [`walk_without_file_ids`]).
    no_ids: bool,
}

/// [`walk_links`] with optional injected failures (see [`Faults`]).
fn walk_faulty(workers: usize, cap: Option<usize>, faults: Faults, root: &Path, keep_going: &(dyn Fn() -> bool + Sync), sink: &mut dyn FnMut(Vec<Event>)) {
    use std::sync::{mpsc, Condvar, Mutex};
    let faults = &faults;
    let links = cap.map(Links::new);
    let links = links.as_ref();
    let queue = Mutex::new(Queue { folders: vec![root.to_path_buf()], busy: 0, stopped: false });
    let wake = Condvar::new();
    // Each listed folder's events, from the listers to this thread; a
    // lister waits when [`PENDING`] listings are already waiting.
    let (tx, rx) = mpsc::sync_channel::<Vec<Event>>(PENDING);
    std::thread::scope(|scope| {
        for _ in 0..workers.max(1) {
            let (tx, queue, wake) = (tx.clone(), &queue, &wake);
            scope.spawn(move || {
                lower_priority();
                let mut lister = Lister { links: links.is_some(), faults: faults.clone(), ..Lister::default() };
                loop {
                    let dir = {
                        let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                        loop {
                            if q.stopped {
                                return;
                            }
                            if let Some(dir) = q.folders.pop() {
                                q.busy += 1;
                                break dir;
                            }
                            if q.busy == 0 {
                                wake.notify_all();
                                return;
                            }
                            q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                        }
                    };
                    let go = keep_going();
                    let mut events = Vec::new();
                    let mut found = Vec::new();
                    if go {
                        let listing = lister.list(&dir, dir.as_path() == root);
                        for e in listing.entries {
                            let path = dir.join(&e.name);
                            if !e.folder {
                                // A further link of a file already counted adds nothing.
                                if let (Some(id), Some(links)) = (e.id, links) {
                                    if !links.counts(id, &mut events) {
                                        continue;
                                    }
                                }
                                events.push(Event::File(path, e.size));
                            } else if !e.link {
                                found.push(path);
                            }
                        }
                        // What was read before an error is kept; the folder
                        // still counts as unreadable.
                        if !listing.complete {
                            events.push(unreadable(dir));
                        }
                    }
                    let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                    q.busy -= 1;
                    if !go {
                        q.stopped = true;
                    }
                    if q.stopped {
                        wake.notify_all();
                        return;
                    }
                    let more = found.len();
                    q.folders.append(&mut found);
                    let ended = q.folders.is_empty() && q.busy == 0;
                    drop(q);
                    // Wake as many idle listers as there are new folders
                    // (all of them when the walk ended, so they leave).
                    if ended || more > 1 {
                        wake.notify_all();
                    } else if more == 1 {
                        wake.notify_one();
                    }
                    if !events.is_empty() && tx.send(events).is_err() {
                        return;
                    }
                }
            });
        }
        drop(tx);
        let mut batch = Vec::with_capacity(BATCH);
        for events in rx {
            if queue.lock().unwrap_or_else(|e| e.into_inner()).stopped {
                return;
            }
            for e in events {
                batch.push(e);
                if batch.len() >= BATCH {
                    sink(std::mem::replace(&mut batch, Vec::with_capacity(BATCH)));
                }
            }
        }
        if !batch.is_empty() && !queue.lock().unwrap_or_else(|e| e.into_inner()).stopped {
            sink(batch);
        }
    });
}

/// Whether the scan descends into a folder whose reparse tag is `tag`.
/// Mount points (junctions), symbolic links and any other tag are not
/// followed: the same data would be measured twice or the walk could
/// loop. Cloud files (OneDrive and other sync providers), deduplication and container isolation are
/// descended, since what they hold locally is stored on this disk.
#[allow(dead_code)]
fn descends(tag: u32) -> bool {
    /// IO_REPARSE_TAG_DEDUP.
    const DEDUP: u32 = 0x8000_0013;
    /// IO_REPARSE_TAG_WCI and IO_REPARSE_TAG_WCI_1.
    const CONTAINER: [u32; 2] = [0x8000_0018, 0x9000_1018];
    /// IO_REPARSE_TAG_CLOUD; IO_REPARSE_TAG_CLOUD_1..F set bits 12-15.
    const CLOUD: u32 = 0x9000_001A;
    const CLOUD_MASK: u32 = 0xFFFF_0FFF;
    tag == DEDUP || CONTAINER.contains(&tag) || tag & CLOUD_MASK == CLOUD
}


/// Lowers the calling thread's CPU priority (below normal) for the rest
/// of its life. Its I/O priority stays normal: Windows background mode
/// (THREAD_MODE_BACKGROUND_BEGIN), used before, also drops I/O to "very
/// low", which the system throttles behind any other disk work.
fn lower_priority() {
    system::lower_priority()
}

/// A folder's listing: the entries read and whether it was read to the
/// end. One that fails part-way keeps what was read; one that cannot be
/// opened has no entries. Either way an incomplete folder is unreadable.
#[derive(Default)]
struct Listing {
    entries: Vec<Entry>,
    complete: bool,
}

/// Lists folders, reusing one buffer for the listings.
#[derive(Default)]
struct Lister {
    #[allow(dead_code)]
    buf: Vec<u64>,
    /// Reads each file's ID (see [`Entry::id`]).
    links: bool,
    /// Failures injected (test seams).
    faults: Faults,
}

impl Lister {
    /// The listing of folder `dir` (without "." and ".."). `through` opens
    /// `dir` through its link when it is one (the scan's root).
    fn list(&mut self, dir: &Path, through: bool) -> Listing {
        let fail_after = self.faults.fail.as_ref().filter(|(d, _)| d == dir).map(|(_, n)| *n);
        list(dir, through, self.links, &mut self.buf, fail_after, self.faults.no_ids)
    }
}

/// The listing of folder `dir` (without "." and ".."): what was read before any error,
/// incomplete when it could not be read to the end (nothing at all when it
/// could not be opened). `through` opens `dir` following its link, otherwise the
/// folder itself is opened. With `links`, each file's identity is kept too (see
/// [`Links`]). `buf` is the reusable read buffer. `fail_after` makes the listing
/// fail once that many entries were read, `no_ids` makes the class with file IDs
/// rejected (test seams).
use system::list;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[allow(dead_code)]
mod portable;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use windows as system;

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A fresh folder under the temp dir, removed when dropped.
    pub(crate) struct TempTree(pub PathBuf);
    impl TempTree {
        pub fn new(name: &str) -> Self {
            let p = std::env::temp_dir().join(format!("mtm-disk-scan-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            TempTree(p)
        }
        pub fn file(&self, rel: &str, size: usize) {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, vec![7u8; size]).unwrap();
        }
        /// Makes `link` a second hard link to the file `target`.
        pub fn hard_link(&self, link: &str, target: &str) {
            let p = self.0.join(link);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::hard_link(self.0.join(target), p).unwrap();
        }
        /// Makes `link` a junction (mount point) to the folder `target`.
        #[cfg(windows)]
        pub fn junction(&self, link: &str, target: &str) {
            let ok = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(self.0.join(link))
                .arg(self.0.join(target))
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "mklink /J failed");
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scan(root: &Path) -> (BTreeMap<String, u64>, Vec<PathBuf>, usize) {
        let mut found = BTreeMap::new();
        let mut unreadable = Vec::new();
        let mut batches = 0;
        walk(root, &|| true, &mut |events| {
            batches += 1;
            for e in events {
                match e {
                    Event::File(p, size) => {
                        let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                        assert!(found.insert(rel, size).is_none(), "a file was reported twice");
                    }
                    Event::Unreadable(p) | Event::Denied(p) => unreadable.push(p),
                    Event::HardLinkLimit => {}
                }
            }
        });
        (found, unreadable, batches)
    }

    /// The space a file of `len` bytes takes: at least its length, within one 64 KB unit.
    fn allocated(size: u64, len: u64) -> bool {
        size >= len && size < len + 65536
    }

    #[test]
    fn it_finds_every_file_once_with_the_space_it_takes() {
        let t = TempTree::new("walk");
        t.file("big/movie.mkv", 50_000);
        t.file("big/deep/clip.mp4", 30_000);
        t.file("loose.bin", 7_000);
        let (found, unreadable, _) = scan(&t.0);
        assert_eq!(found.keys().cloned().collect::<Vec<_>>(), vec!["big/deep/clip.mp4", "big/movie.mkv", "loose.bin"]);
        assert!(allocated(found["big/movie.mkv"], 50_000), "{found:?}");
        assert!(allocated(found["big/deep/clip.mp4"], 30_000), "{found:?}");
        assert!(allocated(found["loose.bin"], 7_000), "{found:?}");
        assert!(unreadable.is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn junctions_are_not_followed() {
        let t = TempTree::new("junction");
        t.file("real/data.bin", 20_000);
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(t.0.join("link"))
            .arg(t.0.join("real"))
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok, "mklink /J failed");
        assert!(t.0.join("link/data.bin").exists(), "the junction works");
        let (found, _, _) = scan(&t.0);
        assert_eq!(found.keys().cloned().collect::<Vec<_>>(), vec!["real/data.bin"]);
    }

    #[test]
    fn links_are_skipped_and_cloud_dedup_and_container_folders_are_descended() {
        // Mount points (junctions) and symbolic links: not followed.
        assert!(!descends(0xA000_0003), "mount point");
        assert!(!descends(0xA000_000C), "symbolic link");
        // Cloud files (OneDrive and other sync providers), deduplication and
        // container isolation: their content is on this disk.
        assert!(descends(0x9000_001A), "cloud");
        assert!(descends(0x9000_101A), "cloud 1");
        assert!(descends(0x9000_F01A), "cloud F");
        assert!(descends(0x8000_0013), "dedup");
        assert!(descends(0x8000_0018), "container isolation");
        assert!(descends(0x9000_1018), "container isolation 1");
        // Anything else is skipped.
        assert!(!descends(0x8000_001B), "app exec link");
        assert!(!descends(0), "no tag");
    }

    #[test]
    fn a_zero_or_invalid_file_id_is_never_deduplicated() {
        assert_eq!(file_key(7, 0), None, "no ID");
        assert_eq!(file_key(7, u64::MAX as u128), None, "FILE_INVALID_FILE_ID");
        assert_eq!(file_key(7, u128::MAX), None, "an invalid 128-bit ID");
        assert_eq!(file_key(7, 42), Some((7, [42, 0])));
        assert_eq!(file_key(7, 1 << 64), Some((7, [0, 1])), "a 128-bit ID (ReFS)");
    }

    /// Several listers meeting the same files at once: each file counts
    /// exactly once, whichever lister meets it first.
    #[test]
    fn listers_meeting_the_same_files_at_once_count_each_once() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let links = Links::new(LINK_CAP);
        let counted = AtomicUsize::new(0);
        const FILES: u64 = 20_000;
        std::thread::scope(|s| {
            for t in 0..8u64 {
                let (links, counted) = (&links, &counted);
                s.spawn(move || {
                    let mut events = Vec::new();
                    for i in 0..FILES {
                        // Each thread in its own order; one file on another volume.
                        let i = (i + t * 2_503) % FILES;
                        let key = if i == 5 { (2, [i + 1, 0]) } else { (1, [i + 1, if i == 6 { 9 } else { 0 }]) };
                        if links.counts(key, &mut events) {
                            counted.fetch_add(1, SeqCst);
                        }
                    }
                    assert!(events.is_empty(), "the limit is not reached");
                });
            }
        });
        assert_eq!(counted.load(SeqCst), FILES as usize);
    }

    /// A volume that rejects the listing with file IDs (FAT, some network
    /// or virtual drives) is listed without them: every file is found,
    /// none of its folders is unreadable, hard links are not deduplicated.
    #[cfg(windows)]
    #[test]
    fn a_volume_without_file_ids_is_listed_without_deduplicating() {
        let t = TempTree::new("no-ids");
        t.file("a/data.bin", 20_000);
        t.hard_link("b/again.bin", "a/data.bin");
        t.file("c/other.bin", 5_000);
        let mut found = BTreeMap::new();
        let mut unreadable = Vec::new();
        walk_without_file_ids(&t.0, &|| true, &mut |events| {
            for e in events {
                match e {
                    Event::File(p, size) => {
                        found.insert(p.strip_prefix(&t.0).unwrap().to_string_lossy().replace('\\', "/"), size);
                    }
                    Event::Unreadable(p) | Event::Denied(p) => unreadable.push(p),
                    Event::HardLinkLimit => {}
                }
            }
        });
        assert_eq!(found.keys().cloned().collect::<Vec<_>>(), ["a/data.bin", "b/again.bin", "c/other.bin"]);
        assert!(allocated(found["b/again.bin"], 20_000) && allocated(found["c/other.bin"], 5_000), "{found:?}");
        assert!(unreadable.is_empty(), "{unreadable:?}");
    }

    /// Benchmark (story 18): what tracking hard links costs per file met,
    /// in this process's private bytes, for 4 million files (a full C:),
    /// and how long 4 listers take to meet 1 million files each.
    /// Ignored: it needs the process to itself
    /// (`cargo test --release --lib -- --ignored --exact <name> --nocapture --test-threads=1`).
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn hard_link_tracking_memory_per_file() {
        use crate::addons::disk_space::tree::tests::private_bytes;
        // NTFS file IDs: a record number with a sequence number on top.
        let id = |i: u64| (0x1234_5678u32, [i | ((i % 7 + 1) << 48), 0]);
        let before = private_bytes();
        let links = Links::new(LINK_CAP);
        let mut events = Vec::new();
        for i in 0..4_000_000u64 {
            links.counts(id(i), &mut events);
        }
        let per_file = private_bytes().saturating_sub(before) as f64 / 4e6;
        drop(links);
        let links = Links::new(LINK_CAP);
        let t = std::time::Instant::now();
        std::thread::scope(|s| {
            for w in 0..4u64 {
                let links = &links;
                s.spawn(move || {
                    let mut events = Vec::new();
                    for i in 0..1_000_000u64 {
                        links.counts(id(w * 1_000_000 + i), &mut events);
                    }
                });
            }
        });
        println!("hard-link tracking: {per_file:.1} bytes per file ({:.0} MB for 4 M files); 4 listers x 1 M files in {:.0} ms", per_file * 4.0, t.elapsed().as_secs_f64() * 1000.0);
        assert!(per_file < 24.0, "{per_file:.1} bytes per file");
    }

    #[test]
    fn a_folder_that_cannot_be_listed_is_reported_unreadable() {
        let missing = std::env::temp_dir().join(format!("mtm-disk-scan-missing-{}", std::process::id()));
        let (found, unreadable, _) = scan(&missing);
        assert!(found.is_empty());
        assert_eq!(unreadable, vec![missing]);
    }

    #[test]
    fn events_come_in_batches_and_stopping_ends_the_walk() {
        let t = TempTree::new("batches");
        for i in 0..(BATCH + 10) {
            t.file(&format!("d{}/f{i}", i % 3), 1);
        }
        let (found, _, batches) = scan(&t.0);
        assert_eq!(found.len(), BATCH + 10);
        assert_eq!(batches, 2);

        let mut seen = 0;
        walk(&t.0, &|| false, &mut |events| seen += events.len());
        assert_eq!(seen, 0, "a walk told to stop reports nothing more");
    }

    /// Small folders past the bound, each with one file.
    const SMALL: usize = PENDING * 3;

    /// A root holding one folder "a" with a full batch of files (so the
    /// sink is called as soon as its listing arrives) and [`SMALL`]
    /// subfolders of one file each, found only once "a" is listed.
    fn backlog_tree(name: &str) -> TempTree {
        let t = TempTree::new(name);
        for i in 0..BATCH {
            t.file(&format!("a/f{i}"), 1);
        }
        for i in 0..SMALL {
            t.file(&format!("a/s{i}/f"), 1);
        }
        t
    }

    /// Waits until `count` stops moving (the listers are all waiting) and
    /// returns it.
    fn settled(count: &std::sync::atomic::AtomicUsize) -> usize {
        use std::sync::atomic::Ordering::SeqCst;
        let mut last = count.load(SeqCst);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(300));
            let now = count.load(SeqCst);
            if now == last {
                return now;
            }
            last = now;
        }
    }

    /// Runs `walk` in its own thread and fails instead of hanging when it
    /// does not end within a minute.
    fn walk_in_time(root: PathBuf, keep_going: impl Fn() -> bool + Sync + Send + 'static, sink: impl FnMut(Vec<Event>) + Send + 'static) {
        let (done, ended) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut sink = sink;
            walk(&root, &keep_going, &mut sink);
            let _ = done.send(());
        });
        ended.recv_timeout(std::time::Duration::from_secs(60)).expect("the walk ended (no deadlock)");
    }

    #[test]
    fn a_slow_sink_holds_the_listers_back_instead_of_piling_results_up() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;
        let t = backlog_tree("bound");
        // Folders taken up by a lister (each checks keep_going first).
        let listed = Arc::new(AtomicUsize::new(0));
        let found = Arc::new(AtomicUsize::new(0));
        let (l, f) = (listed.clone(), found.clone());
        let mut undelivered = None;
        let (report, outcome) = std::sync::mpsc::channel();
        walk_in_time(
            t.0.clone(),
            move || {
                l.fetch_add(1, SeqCst);
                true
            },
            move |events| {
                if undelivered.is_none() {
                    // The sink is stuck in its first call: whatever the
                    // listers find meanwhile is not delivered.
                    let folders: std::collections::BTreeSet<_> = events.iter().map(|e| if let Event::File(p, _) = e { p.parent().unwrap().to_path_buf() } else { unreachable!() }).collect();
                    // The root has no files, so it sends nothing.
                    let waiting = settled(&listed) - 1 - folders.len();
                    undelivered = Some(waiting);
                    let _ = report.send(waiting);
                }
                f.fetch_add(events.len(), SeqCst);
            },
        );
        let waiting = outcome.recv().unwrap();
        // Up to PENDING in the channel, one held by each lister waiting to
        // send it and the rest of a listing split across two batches.
        assert!(waiting <= PENDING + WORKERS + 1, "{waiting} listings waited for a sink stuck on its first batch");
        assert_eq!(found.load(SeqCst), BATCH + SMALL, "every file is still found once the sink catches up");
    }

    #[test]
    fn pause_resume_and_stop_work_while_the_listers_wait_on_a_full_channel() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst};
        use std::sync::Arc;
        let t = backlog_tree("bound-pause");

        // Pause while the listers are held back, then resume: the walk ends
        // with every file.
        let paused = Arc::new(AtomicBool::new(false));
        let listed = Arc::new(AtomicUsize::new(0));
        let found = Arc::new(AtomicUsize::new(0));
        let (p, l, f) = (paused.clone(), listed.clone(), found.clone());
        let (p2, l2) = (paused.clone(), listed.clone());
        let (held, at_pause) = std::sync::mpsc::channel();
        let before_pause = Arc::new(AtomicUsize::new(0));
        let before_pause2 = before_pause.clone();
        let mut first = true;
        walk_in_time(
            t.0.clone(),
            move || {
                l.fetch_add(1, SeqCst);
                while p.load(SeqCst) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                true
            },
            move |events| {
                if std::mem::take(&mut first) {
                    before_pause2.store(settled(&l2), SeqCst);
                    p2.store(true, SeqCst);
                    // Resume from elsewhere once the paused listers settle
                    // (the sink returns now and drains the channel).
                    let (p3, l3) = (p2.clone(), l2.clone());
                    let held = held.clone();
                    std::thread::spawn(move || {
                        let _ = held.send(settled(&l3));
                        p3.store(false, SeqCst);
                    });
                }
                f.fetch_add(events.len(), SeqCst);
            },
        );
        let listed_while_paused = at_pause.recv().unwrap();
        // How many folders were listed before the pause depends on thread
        // timing; what counts is that, once it is set, no lister takes up
        // more than the one folder it was about to.
        let extra = listed_while_paused - before_pause.load(SeqCst);
        assert!(extra <= WORKERS, "the pause held the walk ({extra} more folders listed after it)");
        assert_eq!(found.load(SeqCst), BATCH + SMALL, "a resumed walk finds every file");

        // Stop while the listers are held back: the walk ends early.
        let stop = Arc::new(AtomicBool::new(false));
        let listed = Arc::new(AtomicUsize::new(0));
        let (s, l) = (stop.clone(), listed.clone());
        let (s2, l2) = (stop.clone(), listed.clone());
        let at_stop = Arc::new(AtomicUsize::new(0));
        let at_stop2 = at_stop.clone();
        let mut first = true;
        walk_in_time(
            t.0.clone(),
            move || {
                l.fetch_add(1, SeqCst);
                !s.load(SeqCst)
            },
            move |_| {
                if std::mem::take(&mut first) {
                    at_stop2.store(settled(&l2), SeqCst);
                    s2.store(true, SeqCst);
                }
            },
        );
        // How many folders were listed before the stop depends on thread
        // timing; what counts is that no lister takes up more than the one
        // folder it was about to, once the stop is set.
        let extra = listed.load(SeqCst) - at_stop.load(SeqCst);
        assert!(extra <= WORKERS, "a stopped walk lists no more folders ({extra} more after the stop)");
    }
}
