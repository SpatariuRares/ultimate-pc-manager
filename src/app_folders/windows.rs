//! Windows folder rules: the known folders, read from their environment
//! variables exactly as the app always has. The only place that reads them.

use super::Env;
use std::path::PathBuf;

/// Settings: `%APPDATA%`, else `HOME`.
pub fn config(env: Env) -> Option<PathBuf> {
    env("APPDATA").or_else(|| env("HOME")).map(PathBuf::from)
}

/// Saved data: `%APPDATA%`.
pub fn data(env: Env) -> Option<PathBuf> {
    env("APPDATA").map(PathBuf::from)
}

/// Logs and scratch files: `%LOCALAPPDATA%`.
pub fn cache(env: Env) -> Option<PathBuf> {
    env("LOCALAPPDATA").map(PathBuf::from)
}

/// The user's Documents: `%USERPROFILE%\Documents`.
pub fn documents(env: Env) -> Option<PathBuf> {
    env("USERPROFILE").map(|h| PathBuf::from(h).join("Documents"))
}

/// The data folder as a setting shows it: Windows names the variable.
pub fn shown_data() -> PathBuf {
    PathBuf::from(r"%APPDATA%\UltimatePCManager")
}

/// The Windows folder: `%SystemRoot%`, else `%WINDIR%`, else `C:\Windows`.
pub fn windows_dir(env: Env) -> PathBuf {
    env("SystemRoot").or_else(|| env("WINDIR")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

/// Where the app's folders of its previous name may be: `%APPDATA%` and
/// `%LOCALAPPDATA%`.
pub fn legacy_bases(env: Env) -> Vec<PathBuf> {
    ["APPDATA", "LOCALAPPDATA"].into_iter().filter_map(|v| env(v).map(PathBuf::from)).collect()
}
