//! Small Windows FFI helpers the system-facing concerns share: wide
//! strings, owned handles and strings read back from the system. Compiled
//! on Windows only.

/// `s` as a NUL-terminated UTF-16 string, for the wide Windows calls.
#[cfg(windows)]
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `path` as a NUL-terminated UTF-16 string, for the wide Windows calls.
#[cfg(windows)]
pub fn wide_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

/// A Windows handle from a successful open, closed once when dropped.
#[cfg(windows)]
pub struct Handle(pub windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open (never
        // INVALID_HANDLE_VALUE) and is owned by this value alone, so it is
        // closed exactly once.
        unsafe { windows_sys::Win32::Foundation::CloseHandle(self.0) };
    }
}

/// A NUL-terminated UTF-16 string from Windows (empty for null).
///
/// # Safety
/// `p` is null or points to a readable NUL-terminated UTF-16 string.
pub unsafe fn from_wide(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    // SAFETY: the caller guarantees a NUL-terminated string, so every
    // unit up to and including the NUL is readable.
    while *p.add(len) != 0 {
        len += 1;
    }
    String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
}
