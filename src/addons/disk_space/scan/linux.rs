//! Linux: the folder listing with the space each file takes on disk, hard
//! links told apart by device and inode, and mounts left alone.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::{file_key, Entry, FileKey, Listing, Mounts};

/// The unit `st_blocks` counts in, whatever the filesystem's block size.
const BLOCK: u64 = 512;

/// The key of a file with hard links: its whole 64-bit device number and its
/// inode, so files of different devices never share a key.
fn hard_link_key(dev: u64, ino: u64) -> Option<FileKey> {
    file_key(dev, ino as u128)
}

/// The mount points of this machine, read from its mount table.
pub fn mounts() -> Mounts {
    match crate::linux_mounts::mount_points(&crate::fs_root::FsRoot::system()) {
        Some(points) => Mounts::of(points),
        None => Mounts::default(),
    }
}

/// The listing of folder `dir` (without "." and ".."): what was read before
/// any error, incomplete when it could not be read to the end (nothing at
/// all when it could not be opened, e.g. no permission). Sizes are the
/// space allocated on disk (blocks), not the file's length, so sparse and
/// compressed files are measured as they are stored. With `links`, a file
/// with several hard links carries its device and inode (see
/// [`super::Links`]). A folder that is a mount point (another filesystem, a
/// bind mount of this one, or a pseudo one such as /proc) is reported as a
/// link, which the walk does not enter; a folder of the same filesystem is
/// entered whatever its device number (every btrfs subvolume has its own).
/// `through` follows `dir` when it is itself a link. `fail_after` makes the
/// listing fail once that many entries were read (test seam).
pub fn list(dir: &Path, through: bool, links: bool, _buf: &mut Vec<u64>, fail_after: Option<usize>, _no_ids: bool, mounts: &Mounts) -> Listing {
    list_with(dir, through, links, fail_after, mounts, &|_, md| md.dev())
}

/// [`list`] where `device` tells which device a listed path is on (test seam
/// for the fallback, which compares device numbers when the mount table is
/// not known, and cannot be made to differ in a test).
pub(super) fn list_with(dir: &Path, through: bool, links: bool, fail_after: Option<usize>, mounts: &Mounts, device: &dyn Fn(&Path, &Metadata) -> u64) -> Listing {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Listing::default();
    };
    // Only the fallback needs the folder's own device.
    let own = if through { std::fs::metadata(dir) } else { std::fs::symlink_metadata(dir) };
    let own = own.ok().map(|md| device(dir, &md));
    let mut entries = Vec::new();
    for e in read {
        let Ok(e) = e else {
            return Listing { entries, complete: false };
        };
        if fail_after.is_some_and(|n| entries.len() >= n) {
            return Listing { entries, complete: false };
        }
        let Ok(kind) = e.file_type() else { continue };
        let md = e.metadata().ok();
        let symlink = kind.is_symlink();
        let folder = kind.is_dir() || symlink;
        // A folder that is another mount is left alone like a link.
        let mount = kind.is_dir()
            && match mounts.is_mount(&e.path()) {
                Some(is_mount) => is_mount,
                None => md.as_ref().zip(own).is_some_and(|(md, own)| device(&e.path(), md) != own),
            };
        let id = md.as_ref().filter(|md| links && !folder && md.nlink() > 1).and_then(|md| hard_link_key(md.dev(), md.ino()));
        entries.push(Entry { name: e.file_name(), folder, link: symlink || mount, size: if folder { 0 } else { md.map_or(0, |md| md.blocks() * BLOCK) }, id });
    }
    Listing { entries, complete: true }
}

/// Lowers the calling thread's CPU priority: not changed on this system yet.
pub fn lower_priority() {}

#[cfg(test)]
mod tests {
    use super::super::tests::TempTree;
    use super::super::{walk, Event};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    /// The files found under `root` with their sizes, and the folders that could not be read.
    fn scan(root: &std::path::Path) -> (BTreeMap<String, u64>, Vec<PathBuf>) {
        let (mut found, mut skipped) = (BTreeMap::new(), Vec::new());
        walk(root, &|| true, &mut |events| {
            for e in events {
                match e {
                    Event::File(p, size) => {
                        found.insert(p.strip_prefix(root).unwrap().to_string_lossy().into_owned(), size);
                    }
                    Event::Unreadable(p) | Event::Denied(p) => skipped.push(p),
                    Event::HardLinkLimit => {}
                }
            }
        });
        (found, skipped)
    }

    #[test]
    fn a_hard_linked_file_is_counted_once() {
        let t = TempTree::new("linux-links");
        t.file("a/data.bin", 20_000);
        t.hard_link("b/again.bin", "a/data.bin");
        t.hard_link("c/third.bin", "a/data.bin");
        t.file("c/other.bin", 5_000);
        let (found, skipped) = scan(&t.0);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found.contains_key("c/other.bin"));
        assert_eq!(found.keys().filter(|k| k.ends_with("data.bin") || k.ends_with("again.bin") || k.ends_with("third.bin")).count(), 1, "{found:?}");
        assert!(skipped.is_empty());
    }

    #[test]
    fn a_file_is_measured_by_the_space_it_takes_not_its_length() {
        let t = TempTree::new("linux-sparse");
        // 100 MB long, nothing written: a sparse file takes (almost) no space.
        let f = std::fs::File::create(t.0.join("sparse.img")).unwrap();
        f.set_len(100_000_000).unwrap();
        drop(f);
        // A file that needs blocks on every filesystem (not a few bytes that may live in the inode).
        t.file("dense.bin", 1_000_000);
        let (found, _) = scan(&t.0);
        assert!(found["sparse.img"] < 1_000_000, "{found:?}");
        // The written file takes real space, in whole blocks (a compressing filesystem may take less than its length).
        assert!(found["dense.bin"] > found["sparse.img"] && found["dense.bin"] % 512 == 0, "{found:?}");
    }

    #[test]
    fn a_folder_that_cannot_be_read_is_skipped_and_the_scan_goes_on() {
        use std::os::unix::fs::PermissionsExt;
        let t = TempTree::new("linux-denied");
        t.file("open/a.bin", 4_000);
        t.file("closed/secret.bin", 4_000);
        t.file("z.bin", 4_000);
        let closed = t.0.join("closed");
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
        // A superuser reads it anyway: nothing to test then.
        let readable = std::fs::read_dir(&closed).is_ok();
        let (found, skipped) = scan(&t.0);
        std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o755)).unwrap();
        if readable {
            return;
        }
        assert_eq!(found.keys().cloned().collect::<Vec<_>>(), ["open/a.bin", "z.bin"]);
        assert_eq!(skipped, [closed]);
    }

    use super::super::Mounts;
    use super::{hard_link_key, list_with};

    /// The device of a path as the seam reports it: the real one, plus one for each folder in `other`.
    fn on_devices<'a>(other: &'a [&'a std::path::Path]) -> impl Fn(&std::path::Path, &std::fs::Metadata) -> u64 + 'a {
        move |p, md| {
            use std::os::unix::fs::MetadataExt;
            md.dev() + other.iter().any(|o| p.ends_with(o)) as u64
        }
    }

    #[test]
    fn a_folder_listed_as_a_mount_point_is_not_entered() {
        let t = TempTree::new("linux-mount");
        t.file("home/a.bin", 4_000);
        t.file("mnt/other-disk/b.bin", 4_000);
        t.file("mnt/plain/c.bin", 4_000);
        // The table in the format of /proc/self/mountinfo, read the way the scan reads it.
        let table = format!("40 1 8:1 / {}/mnt/other-disk rw - ext4 /dev/sda1 rw\n", t.0.display());
        let mounts = Mounts::of(crate::linux_mounts::mount_points_in(&table));
        let list = list_with(&t.0.join("mnt"), false, false, None, &mounts, &on_devices(&[]));
        assert!(list.complete);
        let by = |name: &str| list.entries.iter().find(|e| e.name == name).unwrap();
        assert!(by("other-disk").folder && by("other-disk").link, "a mount is left alone like a link");
        assert!(by("plain").folder && !by("plain").link, "another folder is entered");
    }

    #[test]
    fn a_folder_on_another_device_that_is_not_a_mount_point_is_entered() {
        // A btrfs subvolume has a device number of its own but is part of the filesystem.
        let t = TempTree::new("linux-subvolume");
        t.file("data/sub/a.bin", 4_000);
        let mounts = Mounts::of(Vec::new());
        let list = list_with(&t.0.join("data"), false, false, None, &mounts, &on_devices(&[std::path::Path::new("data/sub")]));
        let sub = list.entries.iter().find(|e| e.name == "sub").unwrap();
        assert!(sub.folder && !sub.link, "not in the mount table: part of the same filesystem");
    }

    #[test]
    fn without_a_mount_table_a_folder_on_another_device_is_not_entered() {
        let t = TempTree::new("linux-no-table");
        t.file("data/sub/a.bin", 4_000);
        t.file("data/same/b.bin", 4_000);
        let other = [std::path::Path::new("data/sub")];
        let list = list_with(&t.0.join("data"), false, false, None, &Mounts::default(), &on_devices(&other));
        let by = |name: &str| list.entries.iter().find(|e| e.name == name).unwrap();
        assert!(by("sub").link, "the fallback compares device numbers");
        assert!(!by("same").link);
    }

    #[test]
    fn the_scan_of_a_mount_point_itself_is_listed() {
        let t = TempTree::new("linux-root-mount");
        t.file("a.bin", 4_000);
        let mounts = Mounts::of(vec![t.0.display().to_string()]);
        let list = list_with(&t.0, false, false, None, &mounts, &on_devices(&[]));
        assert_eq!(list.entries.len(), 1, "the scan's own root is the filesystem to measure");
    }

    #[test]
    fn a_bind_mount_of_the_same_device_is_not_entered_by_a_walk() {
        let t = TempTree::new("linux-bind");
        t.file("real/a.bin", 4_000);
        t.file("bound/b.bin", 4_000);
        let mounts = Mounts::of(vec![t.0.join("bound").display().to_string()]);
        let mut found = Vec::new();
        super::super::walk_with_mounts(mounts, &t.0, &mut |events| {
            found.extend(events.into_iter().filter_map(|e| if let Event::File(p, _) = e { Some(p.strip_prefix(&t.0).unwrap().to_path_buf()) } else { None }));
        });
        assert_eq!(found, [PathBuf::from("real/a.bin")]);
    }

    #[test]
    fn hard_links_on_different_devices_never_share_a_key() {
        // These two device numbers fold to the same 32 bits when xor-ed halves are used.
        let (a, b) = (0x0000_0001_0000_0001u64, 0u64);
        assert_ne!(hard_link_key(a, 7), hard_link_key(b, 7));
        assert_eq!(hard_link_key(a, 7), hard_link_key(a, 7), "the same file always has the same key");
        assert_ne!(hard_link_key(a, 7), hard_link_key(a, 8));
        assert_eq!(hard_link_key(a, 0), None, "no inode, no key");
    }

    #[test]
    fn hard_links_of_two_devices_with_the_same_inode_are_two_files() {
        use super::super::{Links, LINK_CAP};
        let links = Links::new(LINK_CAP);
        let mut events = Vec::new();
        let (a, b) = (hard_link_key(0x0000_0001_0000_0001, 7).unwrap(), hard_link_key(0, 7).unwrap());
        assert!(links.counts(a, &mut events) && links.counts(b, &mut events), "different files: both counted");
        assert!(!links.counts(a, &mut events), "the same file again: counted once");
    }

    #[test]
    #[ignore = "reads the real / and /proc of this machine"]
    fn a_scan_of_the_real_root_does_not_enter_the_pseudo_filesystems() {
        let list = super::list(std::path::Path::new("/"), true, false, &mut Vec::new(), None, false, &super::mounts());
        let proc = list.entries.iter().find(|e| e.name == "proc").expect("/proc exists");
        assert!(proc.link, "/proc is a mount of its own");
    }

    #[test]
    fn a_symbolic_link_is_not_followed() {
        let t = TempTree::new("linux-symlink");
        t.file("real/data.bin", 4_000);
        std::os::unix::fs::symlink(t.0.join("real"), t.0.join("alias")).unwrap();
        let (found, _) = scan(&t.0);
        assert_eq!(found.keys().cloned().collect::<Vec<_>>(), ["real/data.bin"]);
    }
}
