//! Linux: the free space of a filesystem (`statvfs`), direct I/O
//! (`O_DIRECT`, bypassing the page cache) and the XDG cache as the default
//! folder, because `/tmp` is often in memory.

use std::path::{Path, PathBuf};

pub const NO_DEFAULT_FOLDER: &str = "Could not locate the cache folder.";

/// The app's folder in the XDG cache (`~/.cache/ultimate-pc-manager`),
/// made when missing (the cache may not exist yet): none when it cannot be had.
pub fn default_folder() -> Option<PathBuf> {
    default_folder_in(&|name| std::env::var_os(name)).filter(|d| std::fs::create_dir_all(d).is_ok())
}

fn default_folder_in(env: crate::app_folders::Env) -> Option<PathBuf> {
    crate::app_folders::xdg_cache_home(env).map(|c| c.join("ultimate-pc-manager"))
}

/// The bytes free for this user on the filesystem of `folder`.
// The statvfs fields are u32 on some targets and u64 on others: the casts
// widen on the first and are no-ops on the second.
#[allow(clippy::unnecessary_cast)]
pub fn free_space(folder: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(folder.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is NUL-terminated and `st` is a valid out-pointer.
    if unsafe { libc::statvfs(path.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs filled `st` since it returned 0.
    let st = unsafe { st.assume_init() };
    Some(st.f_bavail as u64 * st.f_frsize as u64)
}

/// Opens the test file with direct I/O: the drive, not the page cache.
pub fn direct_io(o: &mut std::fs::OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    o.custom_flags(libc::O_DIRECT);
}

/// The filesystem does not allow direct I/O: opening with `O_DIRECT` fails
/// with EINVAL (tmpfs, some network or FUSE filesystems) or EOPNOTSUPP/ENOTSUP
/// (the same number on Linux; both named for the filesystems that say so).
pub fn refuses_direct_io(e: &std::io::Error) -> bool {
    // Compared one by one: EOPNOTSUPP and ENOTSUP are the same number here, which a match pattern would flag.
    e.raw_os_error().is_some_and(|code| code == libc::EINVAL || code == libc::EOPNOTSUPP || code == libc::ENOTSUP)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{left_in, run, scratch, small_job};
    use super::super::DIRECT_IO_REFUSED;
    use super::*;
    use std::ffi::OsString;

    fn raw(code: i32) -> std::io::Error {
        std::io::Error::from_raw_os_error(code)
    }

    #[test]
    fn a_refused_open_gives_the_direct_io_message_for_each_refusal_code() {
        for code in [libc::EINVAL, libc::EOPNOTSUPP, libc::ENOTSUP] {
            assert!(refuses_direct_io(&raw(code)), "errno {code}");
            assert_eq!(super::super::open_failure(&raw(code)), DIRECT_IO_REFUSED);
        }
        assert!(!refuses_direct_io(&raw(libc::ENOENT)));
    }

    #[test]
    fn the_message_says_linux_cache_when_there_is_no_default_folder() {
        assert!(!NO_DEFAULT_FOLDER.contains("Windows"), "{NO_DEFAULT_FOLDER}");
        assert!(NO_DEFAULT_FOLDER.starts_with("Could not locate"));
    }

    #[test]
    fn free_space_of_an_existing_folder_is_read_and_a_missing_one_is_unknown() {
        let dir = scratch("free");
        assert!(free_space(&dir).is_some_and(|f| f > 0));
        assert_eq!(free_space(&dir.join("not-there")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// In memory (tmpfs) the kernel either allows direct I/O (6.6+) or refuses
    /// it: a refusal must say so and leave no file and no score.
    #[test]
    fn a_filesystem_refusing_direct_io_gives_the_message_and_no_score() {
        let shm = Path::new("/dev/shm");
        if !shm.is_dir() {
            return;
        }
        let dir = shm.join(format!("bench-directio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let s = run(small_job(&dir, None));
        match s.status().as_deref() {
            Some("Test complete") => {}
            Some(status) => {
                assert_eq!(status, format!("Disk test failed: {DIRECT_IO_REFUSED}"));
                assert!(s.results("Disk").is_empty());
            }
            None => panic!("no status"),
        }
        assert!(left_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_default_folder_is_in_the_xdg_cache() {
        let at = |vars: &'static [(&str, &str)]| default_folder_in(&move |n| vars.iter().find(|(k, _)| *k == n).map(|(_, v)| OsString::from(v)));
        assert_eq!(at(&[("HOME", "/home/u")]), Some(PathBuf::from("/home/u/.cache/ultimate-pc-manager")));
        assert_eq!(at(&[("HOME", "/home/u"), ("XDG_CACHE_HOME", "/x/cache")]), Some(PathBuf::from("/x/cache/ultimate-pc-manager")));
        assert_eq!(at(&[]), None);
    }
}
