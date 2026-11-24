//! The drives of a Linux machine, read from the kernel's mount table
//! (`/proc/self/mountinfo`).
//!
//! A drive is a real filesystem: pseudo filesystems (proc, sysfs, tmpfs,
//! cgroups, overlays, ...) and read-only package images (squashfs) are not
//! drives. One filesystem is one drive however many times it is mounted:
//! only the mount closest to `/` is listed, so a bind mount, or a further
//! btrfs subvolume of a filesystem that is already listed, is not a drive of
//! its own.

use crate::fs_root::FsRoot;

/// Where the mount table is read.
const MOUNTINFO: &str = "proc/self/mountinfo";

/// Filesystem types that hold no files of the user's: kernel interfaces, memory-backed, layered or read-only image
/// filesystems.
const NOT_DRIVES: &[&str] = &[
    "autofs", "bpf", "binfmt_misc", "cgroup", "cgroup2", "configfs", "debugfs", "devpts", "devtmpfs", "efivarfs", "fusectl", "hugetlbfs", "mqueue", "nsfs", "overlay", "proc", "pstore",
    "ramfs", "rpc_pipefs", "securityfs", "selinuxfs", "squashfs", "sysfs", "tmpfs", "tracefs",
];

/// A mounted drive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Where it is mounted ("/", "/home").
    pub path: String,
    /// The filesystem type ("ext4", "btrfs").
    pub fs: String,
    /// What is mounted: a device ("/dev/sda1") or a dataset.
    pub source: String,
}

/// The drives mounted now, closest to `/` first.
pub fn drives(root: &FsRoot) -> Vec<Mount> {
    drives_in(&root.read(MOUNTINFO).unwrap_or_default())
}

/// The drives listed in the text of a mount table.
pub fn drives_in(table: &str) -> Vec<Mount> {
    // (depth, mount id, drive's key, mount)
    let mut found: Vec<(usize, u32, String, Mount)> = Vec::new();
    for line in table.lines() {
        let Some(m) = parse(line) else { continue };
        if !is_drive(&m.mount) {
            continue;
        }
        let depth = m.mount.path.split('/').filter(|p| !p.is_empty()).count();
        match found.iter_mut().find(|(_, _, key, _)| *key == m.key) {
            Some(known) if (depth, m.id) < (known.0, known.1) => *known = (depth, m.id, m.key, m.mount),
            Some(_) => {}
            None => found.push((depth, m.id, m.key, m.mount)),
        }
    }
    found.sort_by_key(|f| (f.0, f.1));
    found.into_iter().map(|(_, _, _, m)| m).collect()
}

/// Whether a mount is a real filesystem on a device.
fn is_drive(m: &Mount) -> bool {
    !NOT_DRIVES.contains(&m.fs.as_str()) && (m.source.starts_with("/dev/") || m.fs == "zfs")
}

struct Parsed {
    id: u32,
    /// What makes two mounts the same filesystem: the device they come from
    /// (so the subvolumes of one btrfs filesystem share it), else the
    /// kernel's device number.
    key: String,
    mount: Mount,
}

/// One line of the table: "id parent maj:min root path options [tags] - fs source super-options".
fn parse(line: &str) -> Option<Parsed> {
    let mut words = line.split(' ');
    let id = words.next()?.parse().ok()?;
    let _parent = words.next()?;
    let device = words.next()?;
    let _root = words.next()?;
    let path = unescape(words.next()?);
    let mut rest = words.skip_while(|w| *w != "-");
    rest.next()?;
    let fs = rest.next()?.to_string();
    let source = unescape(rest.next()?);
    let key = if source.starts_with("/dev/") { source.clone() } else { device.to_string() };
    Some(Parsed { id, key, mount: Mount { path, fs, source } })
}

/// The table writes a space, tab, newline or backslash in a name as a
/// backslash and three octal digits.
fn unescape(name: &str) -> String {
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let octal = (b[i] == b'\\' && i + 3 < b.len()).then(|| std::str::from_utf8(&b[i + 1..i + 4]).ok().and_then(|d| u8::from_str_radix(d, 8).ok())).flatten();
        match octal {
            Some(byte) => {
                out.push(byte);
                i += 4;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn paths(m: &[Mount]) -> Vec<&str> {
        m.iter().map(|m| m.path.as_str()).collect()
    }

    #[test]
    fn only_real_filesystems_are_drives() {
        let m = drives(&fixture("disk-mounts"));
        let all: Vec<&str> = paths(&m);
        for pseudo in ["/proc", "/sys", "/dev", "/run", "/tmp", "/snap/core/1", "/var/lib/docker/overlay2/x/merged", "/sys/fs/cgroup"] {
            assert!(!all.contains(&pseudo), "{pseudo} is no drive: {all:?}");
        }
        assert_eq!(all, ["/", "/home", "/boot/efi", "/mnt/data space"]);
    }

    #[test]
    fn a_bind_mount_and_a_further_subvolume_are_not_drives_of_their_own() {
        let m = drives(&fixture("disk-mounts"));
        // /srv (bind of /home/shared), /home/user/.snapshots and /var/lib/docker (subvolume mounts of the
        // root filesystem) are listed under the root filesystem's mount only.
        assert_eq!(m.iter().filter(|m| m.source == "/dev/nvme0n1p2").count(), 1, "{m:?}");
        assert_eq!(m.iter().find(|m| m.source == "/dev/nvme0n1p2").unwrap().path, "/");
        assert_eq!(m.iter().filter(|m| m.source == "/dev/sda1").count(), 1);
    }

    #[test]
    fn the_mount_closest_to_the_root_is_the_drive_whatever_the_order_of_the_table() {
        let table = "30 1 8:1 /data /mnt/deep/bind rw - ext4 /dev/sda1 rw\n20 1 8:1 / /mnt/a rw - ext4 /dev/sda1 rw\n";
        assert_eq!(paths(&drives_in(table)), ["/mnt/a"]);
        let table = "20 1 8:1 /data /mnt/bind rw - ext4 /dev/sda1 rw\n30 1 8:1 / /mnt/deep/real rw - ext4 /dev/sda1 rw\n";
        assert_eq!(paths(&drives_in(table)), ["/mnt/bind"]);
    }

    #[test]
    fn a_machine_without_a_readable_table_has_no_drives() {
        assert!(drives(&fixture("memory")).is_empty());
    }
}
