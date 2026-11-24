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

/// The default test folder is the temp folder, which exists.
pub const CREATE_DEFAULT: bool = false;

pub const NO_DEFAULT_FOLDER: &str = "Windows could not locate the temporary folder.";

/// The temp folder.
pub fn default_folder() -> Option<std::path::PathBuf> {
    Some(std::env::temp_dir())
}

/// Windows has no direct I/O refusal of its own: its errors keep their message.
pub fn refuses_direct_io(_e: &std::io::Error) -> bool {
    false
}
