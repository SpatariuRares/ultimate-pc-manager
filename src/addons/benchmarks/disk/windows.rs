//! Windows: the free space of a drive and write-through, unbuffered I/O.

use std::path::Path;

/// The bytes free for this user on the drive of `folder`.
pub fn free_space(folder: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut free = 0u64;
    // SAFETY: `wide` is a NUL-terminated path; the other outputs may be null.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
    };
    (ok != 0).then_some(free)
}

/// Opens the test file for write-through, unbuffered I/O.
pub fn direct_io(o: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH};
    o.custom_flags(FILE_FLAG_WRITE_THROUGH | FILE_FLAG_NO_BUFFERING);
}

pub const NO_DEFAULT_FOLDER: &str = "Windows could not locate the temporary folder.";

/// The temp folder, which exists.
pub fn default_folder() -> Option<std::path::PathBuf> {
    Some(std::env::temp_dir())
}

/// Windows has no direct I/O refusal of its own: its errors keep their message.
pub fn refuses_direct_io(_e: &std::io::Error) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::super::tests::scratch;
    use super::*;

    #[test]
    fn the_missing_default_folder_names_windows() {
        assert_eq!(NO_DEFAULT_FOLDER, "Windows could not locate the temporary folder.");
    }

    #[test]
    fn no_error_is_a_direct_io_refusal() {
        assert!(!refuses_direct_io(&std::io::Error::from_raw_os_error(5)));
        assert!(!refuses_direct_io(&std::io::Error::from_raw_os_error(22)));
    }

    #[test]
    fn free_space_of_an_existing_folder_is_read_and_a_missing_one_is_unknown() {
        let dir = scratch("free");
        assert!(free_space(&dir).is_some_and(|f| f > 0));
        assert_eq!(free_space(&dir.join("not-there")), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
