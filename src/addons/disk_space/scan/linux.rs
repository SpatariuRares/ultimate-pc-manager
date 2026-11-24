//! Linux: the folder listing with the space each file takes on disk, hard
//! links told apart by device and inode, and mounts left alone.

use std::fs::Metadata;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use super::{file_key, Entry, Listing};

/// The unit `st_blocks` counts in, whatever the filesystem's block size.
const BLOCK: u64 = 512;

/// The listing of folder `dir` (without "." and ".."): what was read before
/// any error, incomplete when it could not be read to the end (nothing at
/// all when it could not be opened, e.g. no permission). Sizes are the
/// space allocated on disk (blocks), not the file's length, so sparse and
/// compressed files are measured as they are stored. With `links`, a file
/// with several hard links carries its device and inode (see
/// [`super::Links`]). A folder on another device than `dir` is a mount
/// (another filesystem or a pseudo one such as /proc): it is reported as a
/// link, which the walk does not enter. `through` follows `dir` when it is
/// itself a link. `fail_after` makes the listing fail once that many entries
/// were read (test seam).
pub fn list(dir: &Path, through: bool, links: bool, _buf: &mut Vec<u64>, fail_after: Option<usize>, _no_ids: bool) -> Listing {
    list_on(dir, through, links, fail_after, &|_, md| md.dev())
}

/// [`list`] where `device` tells which device a listed path is on (test seam
/// for mounts, which cannot be made in a test).
pub(super) fn list_on(dir: &Path, through: bool, links: bool, fail_after: Option<usize>, device: &dyn Fn(&Path, &Metadata) -> u64) -> Listing {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Listing::default();
    };
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
        let mount = kind.is_dir() && md.as_ref().zip(own).is_some_and(|(md, own)| device(&e.path(), md) != own);
        let id = md.as_ref().filter(|md| links && !folder && md.nlink() > 1).and_then(|md| file_key(md.dev() as u32 ^ (md.dev() >> 32) as u32, md.ino() as u128));
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
        t.file("small.txt", 10);
        let (found, _) = scan(&t.0);
        assert!(found["sparse.img"] < 1_000_000, "{found:?}");
        // A tiny file takes a whole block.
        assert!(found["small.txt"] >= 512 && found["small.txt"] % 512 == 0, "{found:?}");
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

    #[test]
    fn a_folder_on_another_device_is_not_entered() {
        let t = TempTree::new("linux-mount");
        t.file("home/a.bin", 4_000);
        t.file("mnt/other-disk/b.bin", 4_000);
        let on_other_disk = |p: &std::path::Path, md: &std::fs::Metadata| {
            use std::os::unix::fs::MetadataExt;
            md.dev() + p.ends_with("mnt/other-disk") as u64
        };
        let list = super::list_on(&t.0.join("mnt"), false, false, None, &on_other_disk);
        assert!(list.complete);
        let mount = list.entries.iter().find(|e| e.name == "other-disk").unwrap();
        assert!(mount.folder && mount.link, "a mount is left alone like a link");
        let list = super::list_on(&t.0, false, false, None, &on_other_disk);
        assert!(list.entries.iter().all(|e| !e.link), "same device: entered");
    }

    #[test]
    fn a_scan_of_the_root_does_not_enter_the_pseudo_filesystems() {
        if !std::path::Path::new("/proc/self").exists() {
            return;
        }
        let list = super::list(std::path::Path::new("/"), true, false, &mut Vec::new(), None, false);
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
