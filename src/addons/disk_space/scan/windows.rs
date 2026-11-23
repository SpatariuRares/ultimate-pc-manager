//! Windows: the folder listing through the directory handle, and the
//! scan thread's priority.

use std::path::Path;

use super::{descends, file_key, Entry, Listing};

/// The listing of folder `dir` (without "." and ".."): what was read
/// before any error, incomplete when it could not be read to the end
/// (nothing at all when it could not be opened). Reads the listing in
/// large chunks through the folder's handle (`FileIdExtdDirectoryInfo`):
/// sizes, attributes, reparse tags and file IDs come with the names, no
/// file is opened. A volume that rejects that class (FAT, some network or
/// virtual drives) is listed with `FileFullDirectoryInfo` instead, without
/// file IDs (its hard links are not deduplicated). `through` opens `dir`
/// following its link, otherwise the folder itself is opened. With
/// `links`, each file's volume serial number and file ID are kept too (see
/// [`Links`]). `buf` is the reusable read buffer. `fail_after` makes the
/// listing fail once that many entries were read, `no_ids` makes the class
/// with file IDs rejected (test seams).
pub fn list(dir: &Path, through: bool, links: bool, buf: &mut Vec<u64>, fail_after: Option<usize>, no_ids: bool) -> Listing {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileFullDirectoryInfo, FileFullDirectoryRestartInfo, FileIdExtdDirectoryInfo, GetFileInformationByHandleEx, GetVolumeInformationByHandleW,
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO, FILE_ID_EXTD_DIR_INFO,
        FILE_LIST_DIRECTORY, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };

    let wide = crate::winsys::wide_path(&crate::winsys::long_path(dir));
    let flags = if through { FILE_FLAG_BACKUP_SEMANTICS } else { FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT };
    // SAFETY: `wide` is NUL-terminated and outlives the call; no security
    // attributes or template handle are passed.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_LIST_DIRECTORY,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            flags,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Listing::default();
    }
    // Closes the folder handle on every way out.
    let handle = crate::winsys::Handle(handle);
    // u64 elements keep the buffer 8-byte aligned, as the records need.
    buf.resize(64 * 1024 / 8, 0);
    let mut out = Vec::new();
    // The folder's volume serial number, read at its first file.
    let mut serial = None;
    // Listing with file IDs, until the volume rejects that class.
    let mut ids = true;
    // No chunk was read yet.
    let mut first = true;
    loop {
        let class = match (ids, first) {
            (true, _) => FileIdExtdDirectoryInfo,
            // The fallback starts the listing over.
            (false, true) => FileFullDirectoryRestartInfo,
            (false, false) => FileFullDirectoryInfo,
        };
        let ok = if ids && no_ids {
            // SAFETY: sets the calling thread's last-error value only.
            unsafe { SetLastError(ERROR_INVALID_PARAMETER) };
            0
        } else {
            // SAFETY: the handle is open for listing; the system writes at
            // most the given byte length into `buf`, which it holds exactly.
            unsafe { GetFileInformationByHandleEx(handle.0, class, buf.as_mut_ptr().cast(), (buf.len() * 8) as u32) }
        };
        if ok == 0 {
            // SAFETY: reads the calling thread's last-error value only.
            let err = unsafe { GetLastError() };
            // Rejected at once: the volume does not know the class.
            if ids && first && err != ERROR_NO_MORE_FILES {
                ids = false;
                continue;
            }
            // An error part-way keeps what was read (#235).
            return Listing { entries: out, complete: err == ERROR_NO_MORE_FILES };
        }
        first = false;
        let base = buf.as_ptr().cast::<u8>();
        let mut offset = 0usize;
        loop {
            // The record's next offset, attributes, allocated size, name,
            // reparse tag and file ID (None without IDs).
            let (next, attributes, allocated, name, tag, file_id) = if ids {
                // SAFETY: the system wrote a chain of FILE_ID_EXTD_DIR_INFO
                // records into `buf`, each at an aligned offset given by
                // NextEntryOffset.
                let info = unsafe { &*base.add(offset).cast::<FILE_ID_EXTD_DIR_INFO>() };
                // SAFETY: the record's name holds FileNameLength bytes of
                // UTF-16 inside `buf`, right after its fixed fields.
                let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), info.FileNameLength as usize / 2) };
                (info.NextEntryOffset, info.FileAttributes, info.AllocationSize, name, info.ReparsePointTag, Some(u128::from_le_bytes(info.FileId.Identifier)))
            } else {
                // SAFETY: as above, for a chain of FILE_FULL_DIR_INFO records.
                let info = unsafe { &*base.add(offset).cast::<FILE_FULL_DIR_INFO>() };
                // SAFETY: as above.
                let name = unsafe { std::slice::from_raw_parts(info.FileName.as_ptr(), info.FileNameLength as usize / 2) };
                // For a reparse point, EaSize holds its reparse tag (where
                // FindFirstFile's dwReserved0 reads it from).
                (info.NextEntryOffset, info.FileAttributes, info.AllocationSize, name, info.EaSize, None)
            };
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                let folder = attributes & FILE_ATTRIBUTE_DIRECTORY != 0;
                let reparse = attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0;
                let id = file_id.filter(|_| links && !folder).and_then(|id| {
                    let serial = *serial.get_or_insert_with(|| {
                        let mut n = 0u32;
                        // SAFETY: the handle is open; only the serial number
                        // is asked for (every other buffer is null, size 0).
                        unsafe { GetVolumeInformationByHandleW(handle.0, std::ptr::null_mut(), 0, &mut n, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut(), 0) };
                        n
                    });
                    file_key(serial, id)
                });
                out.push(Entry { name: std::ffi::OsString::from_wide(name), folder, link: folder && reparse && !descends(tag), size: allocated.max(0) as u64, id });
            }
            if fail_after.is_some_and(|n| out.len() >= n) {
                return Listing { entries: out, complete: false };
            }
            if next == 0 {
                break;
            }
            offset += next as usize;
        }
    }
}

/// Lowers the calling thread's CPU priority (below normal) for the rest
/// of its life. Its I/O priority stays normal: Windows background mode
/// (THREAD_MODE_BACKGROUND_BEGIN), used before, also drops I/O to "very
/// low", which the system throttles behind any other disk work.
pub fn lower_priority() {
    // SAFETY: GetCurrentThread returns a pseudo-handle valid for the
    // calling thread, which needs no closing; the call reads no memory of
    // ours.
    unsafe {
        use windows_sys::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
        SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}
