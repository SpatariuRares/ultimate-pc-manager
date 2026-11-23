//! Opening things on Windows: File Explorer, the shell, `cmd start` and the
//! Control Panel.

use crate::long_path::long_path;
use crate::win_ffi::wide_path;

/// Selects the file at `path` in File Explorer.
pub fn show_file_location(path: &str) -> Result<(), String> {
    std::process::Command::new("explorer")
        .arg(format!("/select,{}", path))
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Opens the folder holding `path` in File Explorer with the item
/// selected, through the shell (SHOpenFolderAndSelectItems).
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    use windows_sys::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows_sys::Win32::UI::Shell::{ILFree, SHOpenFolderAndSelectItems};
    // SAFETY: the item list is freed once; COM is uninitialised only if
    // this call initialised it (a thread already in another apartment
    // keeps it).
    unsafe {
        let joined = CoInitializeEx(std::ptr::null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let item = shell_item(path);
        let hr = if item.is_null() { -1 } else { SHOpenFolderAndSelectItems(item, 0, std::ptr::null(), 0) };
        if !item.is_null() {
            ILFree(item);
        }
        if joined {
            CoUninitialize();
        }
        if hr >= 0 {
            Ok(())
        } else {
            Err(format!("The shell could not show {} (0x{hr:08x}).", path.display()))
        }
    }
}

/// The shell's item list for `path` (null when the shell cannot resolve
/// it); the caller frees it with ILFree. The shell refuses the `\\?\`
/// form (E_INVALIDARG, measured), so it gets the plain path, which it
/// resolves past 260 characters when long paths are enabled; failing
/// that, the 8.3 short form, read through [`long_path`], which is shorter.
#[cfg(windows)]
fn shell_item(path: &std::path::Path) -> *mut windows_sys::Win32::UI::Shell::Common::ITEMIDLIST {
    use windows_sys::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows_sys::Win32::UI::Shell::ILCreateFromPathW;
    let wide = wide_path(path);
    // SAFETY: `wide` is NUL-terminated and outlives the call.
    let item = unsafe { ILCreateFromPathW(wide.as_ptr()) };
    if !item.is_null() {
        return item;
    }
    let long = wide_path(&long_path(path));
    let mut short = vec![0u16; 32_768];
    // SAFETY: `long` is NUL-terminated; the system writes at most the
    // given length of UTF-16 units into `short`, which holds exactly that.
    let n = unsafe { GetShortPathNameW(long.as_ptr(), short.as_mut_ptr(), short.len() as u32) } as usize;
    if n == 0 || n >= short.len() {
        return std::ptr::null_mut();
    }
    short.truncate(n);
    // The short form keeps the prefix it was asked with; the shell takes it plain.
    let units = |s: &str| s.encode_utf16().collect::<Vec<u16>>();
    let (long_prefix, unc, share) = (units(r"\\?\"), units(r"UNC\"), units(r"\\"));
    let plain = match short.strip_prefix(&long_prefix[..]) {
        Some(rest) if rest.starts_with(&unc) => [&share[..], &rest[unc.len()..]].concat(),
        Some(rest) => rest.to_vec(),
        None => short,
    };
    let plain: Vec<u16> = plain.into_iter().chain(std::iter::once(0)).collect();
    // SAFETY: `plain` is NUL-terminated and outlives the call.
    unsafe { ILCreateFromPathW(plain.as_ptr()) }
}

/// Opens Windows' Power Options control panel.
pub fn open_power_options() -> Result<(), String> {
    std::process::Command::new("control").arg("powercfg.cpl").spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Starts `cmdline` the way Run does.
pub fn start_command(cmdline: &str) -> Result<(), String> {
    std::process::Command::new("cmd")
        .args(["/C", "start", ""])
        .arg(cmdline)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shell_finds_an_item_whose_path_is_longer_than_260_characters() {
        let root = std::env::temp_dir().join(format!("mtm-winsys-long-{}", std::process::id()));
        let mut dir = root.clone();
        while dir.as_os_str().len() < 300 {
            dir.push("a-rather-long-folder-name-for-the-test");
        }
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("deep.txt");
        std::fs::write(&file, b"x").unwrap();
        let item = shell_item(&file);
        let found = !item.is_null();
        if found {
            // SAFETY: a non-null item list from ILCreateFromPathW, freed once.
            unsafe { windows_sys::Win32::UI::Shell::ILFree(item) };
        }
        let _ = std::fs::remove_dir_all(&root);
        assert!(found, "the shell resolves {} characters", file.as_os_str().len());
    }
}
