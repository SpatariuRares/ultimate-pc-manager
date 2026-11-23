//! Linux folder rules: the XDG base directories with their standard
//! fallbacks under `HOME`. An empty or relative XDG value is ignored, as the
//! specification says.

use super::Env;
use std::path::{Path, PathBuf};

fn home(env: Env) -> Option<PathBuf> {
    env("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// `var` if set to an absolute path, else `fallback` under `HOME`.
fn xdg(env: Env, var: &str, fallback: &str) -> Option<PathBuf> {
    // Absolute: a leading `/`, or whatever the host calls absolute (so the
    // rules can be tried on another host with its own paths).
    match env(var).map(PathBuf::from).filter(|p| p.is_absolute() || p.to_string_lossy().starts_with('/')) {
        Some(dir) => Some(dir),
        None => home(env).map(|h| h.join(Path::new(fallback))),
    }
}

/// Settings: `XDG_CONFIG_HOME`, else `~/.config`.
pub fn config(env: Env) -> Option<PathBuf> {
    xdg(env, "XDG_CONFIG_HOME", ".config")
}

/// Saved data: `XDG_DATA_HOME`, else `~/.local/share`.
pub fn data(env: Env) -> Option<PathBuf> {
    xdg(env, "XDG_DATA_HOME", ".local/share")
}

/// Logs and scratch files: `XDG_CACHE_HOME`, else `~/.cache`.
pub fn cache(env: Env) -> Option<PathBuf> {
    xdg(env, "XDG_CACHE_HOME", ".cache")
}

/// The user's Documents: `XDG_DOCUMENTS_DIR`, else `~/Documents`.
pub fn documents(env: Env) -> Option<PathBuf> {
    xdg(env, "XDG_DOCUMENTS_DIR", "Documents")
}
