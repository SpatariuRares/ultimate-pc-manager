//! macOS folder rules: the user's Library folders under `HOME`.

use super::Env;
use std::path::PathBuf;

fn home(env: Env) -> Option<PathBuf> {
    env("HOME").filter(|h| !h.is_empty()).map(PathBuf::from)
}

/// Settings: `~/Library/Application Support`.
pub fn config(env: Env) -> Option<PathBuf> {
    home(env).map(|h| h.join("Library").join("Application Support"))
}

/// Saved data: `~/Library/Application Support`.
pub fn data(env: Env) -> Option<PathBuf> {
    config(env)
}

/// Logs and scratch files: `~/Library/Caches`.
pub fn cache(env: Env) -> Option<PathBuf> {
    home(env).map(|h| h.join("Library").join("Caches"))
}

/// The user's Documents: `~/Documents`.
pub fn documents(env: Env) -> Option<PathBuf> {
    home(env).map(|h| h.join("Documents"))
}
