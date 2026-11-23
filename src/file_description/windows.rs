//! File descriptions on Windows: the version resource.

use crate::win_ffi::{from_wide, wide};
use std::ptr::null_mut;

/// The FileDescription of an executable's version resource (e.g.
/// "Microsoft Edge"); None when it has none.
pub fn file_description(path: &str) -> Option<String> {
    use std::ffi::c_void;
    use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
    let w = wide(path);
    // SAFETY: every pointer handed to the version API points into `data`
    // or to NUL-terminated wide strings that outlive the calls.
    unsafe {
        let mut handle = 0u32;
        let size = GetFileVersionInfoSizeW(w.as_ptr(), &mut handle);
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(w.as_ptr(), 0, size, data.as_mut_ptr() as *mut c_void) == 0 {
            return None;
        }
        let (mut p, mut len): (*mut c_void, u32) = (null_mut(), 0);
        let mut langs: Vec<String> = Vec::new();
        let tr = wide("\\VarFileInfo\\Translation");
        if VerQueryValueW(data.as_ptr() as *const c_void, tr.as_ptr(), &mut p, &mut len) != 0 && len >= 4 {
            let t = p as *const u16;
            langs.push(format!("{:04x}{:04x}", *t, *t.add(1)));
        }
        langs.extend(["040904b0".to_string(), "040904e4".to_string(), "000004b0".to_string()]);
        for l in langs {
            let key = wide(&format!("\\StringFileInfo\\{l}\\FileDescription"));
            if VerQueryValueW(data.as_ptr() as *const c_void, key.as_ptr(), &mut p, &mut len) != 0 && len > 0 {
                let text = from_wide(p as *const u16).trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
        None
    }
}
