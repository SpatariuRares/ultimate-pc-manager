//! Windows: the uninstall registry.

use winreg::enums::*;
use winreg::RegKey;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

/// Whether the uninstall registry still has the entry `key`
/// ("HKLM\{id}", "HKLM32\{id}" or "HKCU\{id}").
pub fn still_listed(key: &str) -> bool {
    open(key).is_some()
}

/// The entry `key`'s UninstallString and whether it is a Windows
/// Installer product (WindowsInstaller = 1).
pub fn entry(key: &str) -> Option<(String, bool)> {
    let k = open(key)?;
    let command = k.get_value::<String, _>("UninstallString").unwrap_or_default().trim().to_string();
    Some((command, k.get_value::<u32, _>("WindowsInstaller").unwrap_or(0) == 1))
}

/// Opens the entry `key`, if it is there.
fn open(key: &str) -> Option<RegKey> {
    let (hive, sub) = key.split_once('\\')?;
    let (root, path) = match hive {
        "HKLM" => (RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL),
        "HKLM32" => (RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL32),
        "HKCU" => (RegKey::predef(HKEY_CURRENT_USER), UNINSTALL),
        _ => return None,
    };
    root.open_subkey(format!(r"{path}\{sub}")).ok()
}
