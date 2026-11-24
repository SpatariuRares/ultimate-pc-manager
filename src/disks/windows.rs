//! Windows: every disk the system reports is a drive.

use crate::fs_root::FsRoot;

/// Which of the disks mounted at `mounts` are drives: all of them.
pub fn keep(_root: &FsRoot, mounts: &[String]) -> Vec<bool> {
    vec![true; mounts.len()]
}
