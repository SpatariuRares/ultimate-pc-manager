//! Linux: the free space is not read and the test file is opened as it is.

use std::path::Path;

/// The bytes free for this user on the drive of `folder`: not read yet.
pub fn free_space(_folder: &Path) -> Option<u64> {
    None
}

/// The test file is opened with the default options on this system.
pub fn direct_io(_o: &mut std::fs::OpenOptions) {}
