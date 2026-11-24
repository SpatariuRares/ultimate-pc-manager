//! Linux: the drives are the real filesystems of the kernel's mount table,
//! one per filesystem.

use crate::fs_root::FsRoot;
use crate::linux_mounts::{drives, normalize};

/// Which of the disks mounted at `mounts` are drives: those the mount table
/// lists as one (mount paths compared in their normal form, so a name with a
/// space or a trailing slash matches). All of them when the table cannot be
/// read or lists no drive.
pub fn keep(root: &FsRoot, mounts: &[String]) -> Vec<bool> {
    let real: Vec<String> = drives(root).iter().map(|m| normalize(&m.path)).collect();
    if real.is_empty() {
        return vec![true; mounts.len()];
    }
    mounts.iter().map(|m| real.contains(&normalize(m))).collect()
}
