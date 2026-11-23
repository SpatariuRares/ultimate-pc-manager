//! Windows: the FileDescription from the file's version resource.

/// The FileDescription of the file at `path`, if its version details have
/// a non-empty one: in the first language the file lists
/// (`\VarFileInfo\Translation`), else in the usual US English tables.
pub fn description(path: &str) -> Option<String> {
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW};
    if path.is_empty() {
        return None;
    }
    let wide = crate::win_ffi::wide(path);
    // SAFETY: `wide` is a NUL-terminated path; `data` holds `size` bytes
    // (u32s keep it aligned for the u16 strings inside) and outlives every
    // pointer `query` returns into it.
    unsafe {
        let size = GetFileVersionInfoSizeW(wide.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u32; (size as usize).div_ceil(4)];
        if GetFileVersionInfoW(wide.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let block = data.as_ptr().cast();
        let mut langs: Vec<(u16, u16)> = match query(block, r"\VarFileInfo\Translation") {
            Some((ptr, bytes)) => std::slice::from_raw_parts(ptr.cast::<u16>(), bytes as usize / 2).chunks_exact(2).map(|c| (c[0], c[1])).collect(),
            None => Vec::new(),
        };
        langs.extend([(0x0409, 0x04b0), (0x0409, 0x04e4), (0x0000, 0x04b0)]);
        langs.into_iter().find_map(|(lang, codepage)| {
            let (ptr, chars) = query(block, &format!(r"\StringFileInfo\{lang:04x}{codepage:04x}\FileDescription"))?;
            value_text(&data, ptr, chars)
        })
    }
}

/// The string value at `ptr` inside the version details `data`, of
/// `chars` characters as VerQueryValueW says: never read past the end of
/// `data` (a count too large is cut to it) and cut at the first NUL;
/// trimmed, None when empty or when `ptr` is not inside `data`.
fn value_text(data: &[u32], ptr: *const std::ffi::c_void, chars: u32) -> Option<String> {
    let units: &[u16] = {
        let base = data.as_ptr() as usize;
        let bytes = std::mem::size_of_val(data);
        let at = (ptr as usize).checked_sub(base).filter(|at| *at < bytes && at % 2 == 0)?;
        // SAFETY: `ptr` is inside `data` at an even offset (checked above)
        // and `data` is u32-aligned, so the u16s from there to the end of
        // `data` are readable and aligned; no more than those are taken.
        unsafe { std::slice::from_raw_parts(ptr.cast::<u16>(), (chars as usize).min((bytes - at) / 2)) }
    };
    let units = units.split(|&u| u == 0).next().unwrap_or_default();
    let text = String::from_utf16_lossy(units);
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// The value at `sub` in a version-details `block` and its length (bytes
/// for binary values, characters for strings), if it has one.
///
/// # Safety
/// `block` must be a buffer filled by GetFileVersionInfoW.
unsafe fn query(block: *const std::ffi::c_void, sub: &str) -> Option<(*const std::ffi::c_void, u32)> {
    use windows_sys::Win32::Storage::FileSystem::VerQueryValueW;
    let sub = crate::win_ffi::wide(sub);
    let mut ptr = std::ptr::null_mut();
    let mut len = 0u32;
    let found = VerQueryValueW(block, sub.as_ptr(), &mut ptr, &mut len) != 0;
    (found && !ptr.is_null() && len > 0).then_some((ptr as *const _, len))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// "Hello", a NUL, then "junk", as UTF-16 in a u32-aligned buffer.
    fn details() -> Vec<u32> {
        let units: Vec<u16> = "Hello\0junk".encode_utf16().collect();
        let mut data = vec![0u32; units.len().div_ceil(2)];
        for (i, u) in units.iter().enumerate() {
            data[i / 2] |= u32::from(*u) << (16 * (i % 2));
        }
        data
    }

    #[test]
    fn a_value_is_cut_at_its_first_nul_and_never_read_past_the_buffer() {
        let data = details();
        let at = data.as_ptr().cast::<std::ffi::c_void>();
        assert_eq!(value_text(&data, at, 10).as_deref(), Some("Hello"), "what follows the NUL is not the value");
        assert_eq!(value_text(&data, at, 1_000_000).as_deref(), Some("Hello"), "a count past the buffer is cut to it");
        assert_eq!(value_text(&data, at, 3).as_deref(), Some("Hel"));
        let outside = data.as_ptr().wrapping_add(data.len()).cast::<std::ffi::c_void>();
        assert_eq!(value_text(&data, outside, 4), None, "a value outside the buffer is not read");
    }
}
