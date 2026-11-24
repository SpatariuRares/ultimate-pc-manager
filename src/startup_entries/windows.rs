//! Startup entries on Windows: the Run keys and the Startup folder.

use super::StartupEntry;
use winreg::enums::*;
use winreg::RegKey;

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED_RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const APPROVED_FOLDER: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\StartupFolder";

fn approved(hive: &RegKey, key: &str, name: &str) -> bool {
    hive.open_subkey(key)
        .ok()
        .and_then(|k| k.get_raw_value(name).ok())
        .map(|v| v.bytes.first().map(|b| b & 1 == 0).unwrap_or(true))
        .unwrap_or(true)
}

/// Where the list comes from, as the page's band says it.
pub fn sources_note() -> &'static str {
    "From the Run keys and Startup folders"
}

pub fn list_startup() -> Vec<StartupEntry> {
    let mut out = Vec::new();
    for (hkcu, hive, label) in [
        (true, RegKey::predef(HKEY_CURRENT_USER), "Registry (current user)"),
        (false, RegKey::predef(HKEY_LOCAL_MACHINE), "Registry (all users)"),
    ] {
        if let Ok(k) = hive.open_subkey(RUN) {
            for (name, value) in k.enum_values().flatten() {
                let command = value.to_string().trim_matches('"').to_string();
                out.push(StartupEntry {
                    enabled: approved(&hive, APPROVED_RUN, &name),
                    name,
                    command,
                    location: label.into(),
                    hkcu,
                    approved_key: APPROVED_RUN.into(),
                    ..Default::default()
                });
            }
        }
    }
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    if let Ok(appdata) = std::env::var("APPDATA") {
        let dir = std::path::Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let file = e.file_name().to_string_lossy().into_owned();
                if file.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                out.push(StartupEntry {
                    enabled: approved(&hkcu, APPROVED_FOLDER, &file),
                    name: file.trim_end_matches(".lnk").to_string(),
                    command: e.path().display().to_string(),
                    location: "Startup folder".into(),
                    hkcu: true,
                    approved_key: APPROVED_FOLDER.into(),
                    ..Default::default()
                });
            }
        }
    }
    out
}

pub fn set_startup_enabled(row: &StartupEntry, enable: bool) -> Result<(), String> {
    let hive = if row.hkcu { RegKey::predef(HKEY_CURRENT_USER) } else { RegKey::predef(HKEY_LOCAL_MACHINE) };
    let (key, _) = hive
        .create_subkey(&row.approved_key)
        .map_err(|_| "Access denied. Run the app as administrator to change this entry.".to_string())?;
    let value_name = if row.location == "Startup folder" {
        std::path::Path::new(&row.command).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
    } else {
        row.name.clone()
    };
    let mut bytes = vec![0u8; 12];
    bytes[0] = if enable { 0x02 } else { 0x03 };
    key.set_raw_value(&value_name, &winreg::RegValue { bytes: bytes.into(), vtype: REG_BINARY })
        .map_err(|e| e.to_string())
}
