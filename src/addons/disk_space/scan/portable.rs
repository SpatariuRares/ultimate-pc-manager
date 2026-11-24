//! The portable scanner: the standard directory listing, used where the system has no faster listing of its own.

use std::path::Path;

use super::{Entry, Listing, Mounts};

/// Outside Windows: the standard listing, links not followed, sizes as
/// the length of each file.
pub fn list(dir: &Path, _through: bool, _links: bool, _buf: &mut Vec<u64>, fail_after: Option<usize>, _no_ids: bool, _mounts: &Mounts) -> Listing {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Listing::default();
    };
    let mut entries = Vec::new();
    for e in read {
        let Ok(e) = e else {
            return Listing { entries, complete: false };
        };
        if fail_after.is_some_and(|n| entries.len() >= n) {
            return Listing { entries, complete: false };
        }
        if let Ok(kind) = e.file_type() {
            entries.push(Entry { name: e.file_name(), folder: kind.is_dir() || kind.is_symlink(), link: kind.is_symlink(), size: e.metadata().map_or(0, |m| m.len()), id: None });
        }
    }
    Listing { entries, complete: true }
}
