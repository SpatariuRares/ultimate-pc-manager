//! Linux: the free space of a filesystem (`statvfs`), direct I/O
//! (`O_DIRECT`, bypassing the page cache) and the XDG cache as the default
//! folder, because `/tmp` is often in memory.

use std::path::{Path, PathBuf};

/// The default folder is made when missing (the cache may not exist yet).
pub const CREATE_DEFAULT: bool = true;

pub const NO_DEFAULT_FOLDER: &str = "Could not locate the cache folder.";

/// The app's folder in the XDG cache (`~/.cache/ultimate-pc-manager`).
pub fn default_folder() -> Option<PathBuf> {
    default_folder_in(&|name| std::env::var_os(name))
}

fn default_folder_in(env: crate::app_folders::Env) -> Option<PathBuf> {
    crate::app_folders::xdg_cache_home(env).map(|c| c.join("ultimate-pc-manager"))
}

/// The bytes free for this user on the filesystem of `folder`.
pub fn free_space(folder: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is a valid out-pointer.
    let ok = unsafe { libc::statvfs(path.as_ptr(), st.as_mut_ptr()) } == 0;
    // SAFETY: statvfs filled `st` when it returned 0.
    // The fields are u32 on some targets: the casts widen there.
    #[allow(clippy::unnecessary_cast)]
    let free = ok.then(|| unsafe { st.assume_init() }).map(|st| st.f_bavail as u64 * st.f_frsize as u64);
    free
}

/// Opens the test file with direct I/O: the drive, not the page cache.
pub fn direct_io(o: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    o.custom_flags(libc::O_DIRECT);
}

/// The filesystem does not allow direct I/O (EINVAL, as tmpfs and some
/// network or FUSE filesystems answer to `O_DIRECT`).
pub fn refuses_direct_io(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::EINVAL)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn the_default_folder_is_in_the_xdg_cache() {
        let at = |vars: &'static [(&str, &str)]| default_folder_in(&move |n| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| OsString::from(v)));
        assert_eq!(at(&[("HOME", "/home/u")]), Some(PathBuf::from("/home/u/.cache/ultimate-pc-manager")));
        assert_eq!(at(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/x/cache")]), Some(PathBuf::from("/x/cache/ultimate-pc-manager")));
        assert_eq!(at(&[]), None);
    }
}
