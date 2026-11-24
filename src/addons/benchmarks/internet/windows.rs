//! Windows: the proxy of the user's Internet settings.

use super::{proxy_for, HOST};

/// The proxy of the `HTTPS_PROXY`-style environment variables.
pub fn env_proxy() -> Option<ureq::Proxy> {
    ureq::Proxy::try_from_env()
}

/// The system proxy for [`HOST`], from the user's Windows Internet settings
/// (proxy auto-config scripts are not read).
pub fn system_proxy() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings", KEY_READ)
        .ok()?;
    if key.get_value::<u32, _>("ProxyEnable").ok()? != 1 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
    proxy_for(&server, &bypass, HOST)
}
