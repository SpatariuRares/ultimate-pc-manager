//! Which of the disks the system reports are drives to show.
//!
//! A concern laid out like [`crate::app_folders`]: this module holds the
//! public function every caller uses, and one submodule per system
//! (`windows`, `linux`, `macos`) holds that system's rule, the same function
//! on each. Windows and macOS show every disk the system reports. Linux
//! reports pseudo and image filesystems and every bind mount as disks too,
//! so it keeps only the real ones, one per filesystem (see
//! [`crate::linux_mounts`]).

use crate::fs_root::FsRoot;
use crate::system::System;

mod linux;
mod macos;
mod windows;

/// The drives among `all`, the disks the system reported, in their order;
/// `mount` gives where each is mounted. When the rule would leave nothing of
/// a list that has disks, all stay: a machine always shows its drives.
pub fn real_drives<T>(all: Vec<T>, mount: impl Fn(&T) -> String) -> Vec<T> {
    real_drives_on(System::current(), &FsRoot::system(), all, mount)
}

/// [`real_drives`] by the rule of `system`, reading the mount table under
/// `root` (the seam tests use).
pub fn real_drives_on<T>(system: System, root: &FsRoot, all: Vec<T>, mount: impl Fn(&T) -> String) -> Vec<T> {
    let mounts: Vec<String> = all.iter().map(&mount).collect();
    let keep = match system {
        System::Windows => windows::keep(root, &mounts),
        System::Linux => linux::keep(root, &mounts),
        System::MacOs => macos::keep(root, &mounts),
    };
    if !keep.contains(&true) {
        return all;
    }
    all.into_iter().zip(keep).filter_map(|(disk, keep)| keep.then_some(disk)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn kept(system: System, root: &FsRoot, mounts: &[&str]) -> Vec<String> {
        real_drives_on(system, root, mounts.iter().map(|m| m.to_string()).collect(), |m| m.clone())
    }

    const REPORTED: [&str; 8] = ["/", "/proc", "/srv", "/home/user/.snapshots", "/mnt/data space", "/home/", "/run", "/boot/efi"];

    #[test]
    fn linux_keeps_the_real_filesystems_one_per_filesystem() {
        // Pseudo filesystems, a bind mount (/srv) and a second subvolume of the root filesystem go; a path
        // with a space and one with a trailing slash are the same as the table's.
        let real = kept(System::Linux, &fixture("disk-mounts"), &REPORTED);
        assert_eq!(real, ["/", "/mnt/data space", "/home/", "/boot/efi"]);
    }

    #[test]
    fn windows_and_macos_keep_every_disk() {
        for system in [System::Windows, System::MacOs] {
            assert_eq!(kept(system, &fixture("disk-mounts"), &REPORTED), REPORTED, "{system:?}");
        }
    }

    #[test]
    fn linux_without_a_readable_table_keeps_every_disk() {
        assert_eq!(kept(System::Linux, &fixture("memory"), &REPORTED), REPORTED);
    }

    #[test]
    fn linux_never_ends_with_no_drive_when_the_system_reported_some() {
        assert_eq!(kept(System::Linux, &fixture("disk-mounts"), &["/elsewhere", "/else/where"]), ["/elsewhere", "/else/where"]);
        assert_eq!(kept(System::Linux, &fixture("disk-mounts"), &[]), Vec::<String>::new(), "no disks stays none");
    }
}
