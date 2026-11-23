//! Windows: the uninstall registry, Store apps and the shell's usage counters.

use super::*;
use std::time::{SystemTime, UNIX_EPOCH};
use winreg::enums::*;
use winreg::RegKey;

const UNINSTALL: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall";
const UNINSTALL32: &str = r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn views() -> [(&'static str, RegKey, &'static str); 3] {
    [
        ("HKLM", RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL),
        ("HKLM32", RegKey::predef(HKEY_LOCAL_MACHINE), UNINSTALL32),
        ("HKCU", RegKey::predef(HKEY_CURRENT_USER), UNINSTALL),
    ]
}

pub fn list_registry() -> Vec<App> {
    let mut out: Vec<App> = Vec::new();
    for (hive, root, path) in views() {
        let Ok(root) = root.open_subkey(path) else { continue };
        for sub in root.enum_keys().flatten() {
            let Ok(k) = root.open_subkey(&sub) else { continue };
            let s = |n: &str| k.get_value::<String, _>(n).unwrap_or_default().trim().to_string();
            let d = |n: &str| k.get_value::<u32, _>(n).unwrap_or(0);
            let name = s("DisplayName");
            // Hidden the way Settings hides them: no name, parts of
            // Windows or of another product, and updates.
            if name.is_empty() || d("SystemComponent") == 1 || !s("ParentKeyName").is_empty() {
                continue;
            }
            let release = s("ReleaseType").to_lowercase();
            if release.contains("update") || release.contains("hotfix") {
                continue;
            }
            let msi = d("WindowsInstaller") == 1 && sub.starts_with('{');
            let mut installed = s("InstallDate");
            if installed.len() == 8 && installed.chars().all(|c| c.is_ascii_digit()) {
                installed = format!("{}-{}-{}", &installed[0..4], &installed[4..6], &installed[6..8]);
            } else {
                // When the app did not say, the time its entry was written.
                installed = k
                    .query_info()
                    .ok()
                    .map(|i| {
                        let ft = ((i.last_write_time.dwHighDateTime as u64) << 32) | i.last_write_time.dwLowDateTime as u64;
                        ymd((ft / 10_000_000) as i64 - 11_644_473_600)
                    })
                    .unwrap_or_default();
            }
            let app = App {
                key: format!("{hive}\\{sub}"),
                publisher: s("Publisher"),
                version: s("DisplayVersion"),
                installed,
                size_reported: d("EstimatedSize") as u64 * 1024,
                location: s("InstallLocation"),
                icon: s("DisplayIcon"),
                uninstall: s("UninstallString"),
                modify: s("ModifyPath"),
                no_remove: d("NoRemove") == 1,
                no_modify: d("NoModify") == 1,
                no_repair: d("NoRepair") == 1,
                msi_code: if msi { sub.clone() } else { String::new() },
                source: if msi { Source::Msi } else { Source::Registry },
                user_scope: hive == "HKCU",
                bits32: hive == "HKLM32",
                about_url: s("URLInfoAbout"),
                help_url: s("HelpLink"),
                name,
                ..Default::default()
            };
            // The same product registered twice (both views): keep one.
            if out.iter().any(|o| o.name == app.name && o.version == app.version && o.publisher == app.publisher) {
                continue;
            }
            out.push(app);
        }
    }
    out
}

/// "ms-resource:AppName" → the text it names in the package.
fn resource(text: &str, full_name: &str, id: &str) -> String {
    let Some(rest) = text.strip_prefix("ms-resource:") else { return text.to_string() };
    let uri = if rest.starts_with("//") {
        format!("ms-resource:{rest}")
    } else if let Some(r) = rest.strip_prefix('/') {
        format!("ms-resource://{id}/{r}")
    } else if rest.contains('/') {
        format!("ms-resource://{id}/{rest}")
    } else {
        format!("ms-resource://{id}/Resources/{rest}")
    };
    let source = wide(&format!("@{{{full_name}?{uri}}}"));
    let mut out = vec![0u16; 512];
    unsafe {
        if windows_sys::Win32::UI::Shell::SHLoadIndirectString(source.as_ptr(), out.as_mut_ptr(), 512, std::ptr::null()) == 0 {
            let n = out.iter().position(|&c| c == 0).unwrap_or(0);
            return String::from_utf16_lossy(&out[..n]);
        }
    }
    String::new()
}

pub fn list_msix_and_links(ctx: &AddonContext) -> (Vec<App>, Vec<Link>) {
    let Ok(v) = ctx.powershell(INVENTORY_SCRIPT, &[], false) else { return (Vec::new(), Vec::new()) };
    let mut apps = Vec::new();
    for p in v["packages"].as_array().cloned().unwrap_or_default() {
        let g = |k: &str| p[k].as_str().unwrap_or_default().to_string();
        let (full, id) = (g("FullName"), g("Id"));
        let mut name = resource(&g("Name"), &full, &id);
        if name.is_empty() {
            name = id.clone();
        }
        let location = g("Location");
        let installed = std::fs::metadata(&location)
            .and_then(|m| m.created())
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| ymd(d.as_secs() as i64))
            .unwrap_or_default();
        apps.push(App {
            key: format!("MSIX\\{full}"),
            publisher: resource(&g("Publisher"), &full, &id),
            version: g("Version"),
            installed,
            location,
            no_remove: p["NonRemovable"].as_bool() == Some(true),
            source: Source::Msix,
            user_scope: true,
            bits32: g("Arch") == "X86",
            package: full,
            family: g("Family"),
            name,
            ..Default::default()
        });
    }
    let links = v["links"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|l| Link {
            path: l["Path"].as_str().unwrap_or_default().to_string(),
            name: l["Name"].as_str().unwrap_or_default().to_string(),
            target: l["Target"].as_str().unwrap_or_default().to_string(),
            aumid: l["Aumid"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    (apps, links)
}

pub fn user_assist() -> Vec<Opened> {
    let mut out = Vec::new();
    let Ok(root) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Explorer\UserAssist") else {
        return out;
    };
    for g in root.enum_keys().flatten() {
        let Ok(count) = root.open_subkey(format!(r"{g}\Count")) else { continue };
        for (name, value) in count.enum_values().flatten() {
            let b = &value.bytes;
            if b.len() < 68 {
                continue;
            }
            let n = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
            let ft = u64::from_le_bytes(b[60..68].try_into().unwrap_or([0; 8]));
            if ft == 0 {
                continue;
            }
            let what = rot13(&name);
            if what.starts_with("UEME_") {
                continue;
            }
            out.push(Opened { what: expand_known_folder(&what), when: (ft / 10_000_000) as i64 - 11_644_473_600, count: n });
        }
    }
    out
}

#[allow(dead_code)]
fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
