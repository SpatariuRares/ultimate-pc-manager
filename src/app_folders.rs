//! Where the app keeps its own folders: settings (config), saved data
//! (histories, journals), cache (logs, scratch files) and the user's
//! Documents.
//!
//! # The per-concern layout
//!
//! Each system-dependent concern of the app is one module like this one:
//!
//! * the module file holds the shared types and the public functions every
//!   caller uses, and nothing else decides what system it runs on;
//! * one submodule per system (`windows`, `linux`, `macos`) exposes the same
//!   functions, each holding that system's rules;
//! * no page or add-on reads a system's environment variables, registry or
//!   native API itself: it calls the concern.
//!
//! A concern whose submodules call a system API (the registry, D-Bus, ...)
//! compiles each submodule only on its system (`#[cfg(windows)] mod windows;`
//! and so on). This concern only applies rules to environment values, so
//! its three submodules compile everywhere: each takes an injected
//! environment lookup ([`Env`]) and the tests resolve every system's folders
//! on any host. [`System::current`] is the single place that picks the
//! running system; the public functions pass it with the process
//! environment.
//!
//! # Rules
//!
//! | Folder    | Windows                   | Linux                              | macOS                           |
//! |-----------|---------------------------|------------------------------------|---------------------------------|
//! | config    | `%APPDATA%` (else `HOME`) | `XDG_CONFIG_HOME`, `~/.config`     | `~/Library/Application Support` |
//! | data      | `%APPDATA%`               | `XDG_DATA_HOME`, `~/.local/share`  | `~/Library/Application Support` |
//! | cache     | `%LOCALAPPDATA%`          | `XDG_CACHE_HOME`, `~/.cache`       | `~/Library/Caches`              |
//! | Documents | `%USERPROFILE%\Documents` | `XDG_DOCUMENTS_DIR`, `~/Documents` | `~/Documents`                   |
//!
//! The app's folder is [`APP_FOLDER`] inside the first three. A function
//! gives `None` when the system names no such folder (no `HOME`, say); the
//! caller picks its own fallback.

use std::ffi::OsString;
use std::path::PathBuf;

mod linux;
mod macos;
mod windows;

/// The name of the app's folders inside the system's config, data and
/// cache folders.
pub const APP_FOLDER: &str = "UltimatePCManager";

/// An environment lookup: the value of a variable, if it is set. The public
/// functions read the process environment; tests pass their own values.
pub type Env<'a> = &'a dyn Fn(&str) -> Option<OsString>;

/// A system whose folder rules the concern knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Windows,
    Linux,
    MacOs,
}

impl System {
    /// The system the app runs on.
    pub const fn current() -> System {
        #[cfg(windows)]
        return System::Windows;
        #[cfg(target_os = "linux")]
        return System::Linux;
        #[cfg(target_os = "macos")]
        return System::MacOs;
    }
}

/// A kind of folder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Folder {
    /// Settings.
    Config,
    /// What the app saves and the user would miss (histories, journals).
    Data,
    /// What can be made again (logs, scratch files).
    Cache,
    /// The user's Documents (not the app's own).
    Documents,
}

/// The folder of `kind` on `system` as `env` says: the system's base folder,
/// with [`APP_FOLDER`] inside it for the app's own kinds.
pub fn resolve(system: System, kind: Folder, env: Env) -> Option<PathBuf> {
    let (base, own) = match (system, kind) {
        (System::Windows, Folder::Config) => (windows::config(env), true),
        (System::Windows, Folder::Data) => (windows::data(env), true),
        (System::Windows, Folder::Cache) => (windows::cache(env), true),
        (System::Windows, Folder::Documents) => (windows::documents(env), false),
        (System::Linux, Folder::Config) => (linux::config(env), true),
        (System::Linux, Folder::Data) => (linux::data(env), true),
        (System::Linux, Folder::Cache) => (linux::cache(env), true),
        (System::Linux, Folder::Documents) => (linux::documents(env), false),
        (System::MacOs, Folder::Config) => (macos::config(env), true),
        (System::MacOs, Folder::Data) => (macos::data(env), true),
        (System::MacOs, Folder::Cache) => (macos::cache(env), true),
        (System::MacOs, Folder::Documents) => (macos::documents(env), false),
    };
    base.map(|b| if own { b.join(APP_FOLDER) } else { b })
}

/// The folders where the app may have kept data under its previous name, so
/// they can be moved (see `settings::move_old_data_folders`). Only Windows
/// ever had any.
pub fn legacy_bases_of(system: System, env: Env) -> Vec<PathBuf> {
    match system {
        System::Windows => windows::legacy_bases(env),
        System::Linux | System::MacOs => Vec::new(),
    }
}

fn process_env(name: &str) -> Option<OsString> {
    std::env::var_os(name)
}

/// The app's settings folder.
pub fn config() -> Option<PathBuf> {
    resolve(System::current(), Folder::Config, &process_env)
}

/// The app's data folder (saved histories, journals).
pub fn data() -> Option<PathBuf> {
    resolve(System::current(), Folder::Data, &process_env)
}

/// The app's cache folder (logs, scratch files).
pub fn cache() -> Option<PathBuf> {
    resolve(System::current(), Folder::Cache, &process_env)
}

/// The user's Documents folder.
pub fn documents() -> Option<PathBuf> {
    resolve(System::current(), Folder::Documents, &process_env)
}

/// See [`legacy_bases_of`], for the running system.
pub fn legacy_bases() -> Vec<PathBuf> {
    legacy_bases_of(System::current(), &process_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::Path;

    fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| (k.to_string(), OsString::from(v))).collect();
        move |name| map.get(name).cloned()
    }

    fn at(system: System, kind: Folder, pairs: &[(&str, &str)]) -> Option<PathBuf> {
        // A Linux or macOS path joined on a Windows host gets a `\` between
        // parts: compare the paths as they read on their own system.
        let path = resolve(system, kind, &env_of(pairs))?;
        Some(if system == System::Windows { path } else { PathBuf::from(path.to_string_lossy().replace('\\', "/")) })
    }

    #[test]
    fn windows_folders_are_the_known_ones() {
        let env = [("APPDATA", r"C:\U\AppData\Roaming"), ("LOCALAPPDATA", r"C:\U\AppData\Local"), ("USERPROFILE", r"C:\U"), ("HOME", r"C:\H")];
        let w = |kind| at(System::Windows, kind, &env);
        assert_eq!(w(Folder::Config), Some(Path::new(r"C:\U\AppData\Roaming").join("UltimatePCManager")));
        assert_eq!(w(Folder::Data), Some(Path::new(r"C:\U\AppData\Roaming").join("UltimatePCManager")));
        assert_eq!(w(Folder::Cache), Some(Path::new(r"C:\U\AppData\Local").join("UltimatePCManager")));
        assert_eq!(w(Folder::Documents), Some(Path::new(r"C:\U").join("Documents")));
    }

    #[test]
    fn windows_settings_fall_back_to_home_and_the_rest_to_nothing() {
        let env = [("HOME", r"C:\H")];
        assert_eq!(at(System::Windows, Folder::Config, &env), Some(Path::new(r"C:\H").join("UltimatePCManager")));
        for kind in [Folder::Data, Folder::Cache, Folder::Documents] {
            assert_eq!(at(System::Windows, kind, &env), None, "{kind:?}");
        }
    }

    #[test]
    fn windows_old_folders_are_appdata_and_local_appdata() {
        let both = env_of(&[("APPDATA", "a"), ("LOCALAPPDATA", "l")]);
        assert_eq!(legacy_bases_of(System::Windows, &both), [PathBuf::from("a"), PathBuf::from("l")]);
        assert_eq!(legacy_bases_of(System::Windows, &env_of(&[])), Vec::<PathBuf>::new());
        assert!(legacy_bases_of(System::Linux, &both).is_empty() && legacy_bases_of(System::MacOs, &both).is_empty());
    }

    #[test]
    fn linux_follows_the_xdg_base_directories() {
        let env = [("HOME", "/home/u"), ("XDG_CONFIG_HOME", "/x/config"), ("XDG_DATA_HOME", "/x/data"), ("XDG_CACHE_HOME", "/x/cache"), ("XDG_DOCUMENTS_DIR", "/x/docs")];
        let l = |kind| at(System::Linux, kind, &env);
        assert_eq!(l(Folder::Config), Some(PathBuf::from("/x/config/UltimatePCManager")));
        assert_eq!(l(Folder::Data), Some(PathBuf::from("/x/data/UltimatePCManager")));
        assert_eq!(l(Folder::Cache), Some(PathBuf::from("/x/cache/UltimatePCManager")));
        assert_eq!(l(Folder::Documents), Some(PathBuf::from("/x/docs")));
    }

    #[test]
    fn linux_uses_the_standard_fallbacks_under_home() {
        let env = [("HOME", "/home/u")];
        let l = |kind| at(System::Linux, kind, &env);
        assert_eq!(l(Folder::Config), Some(PathBuf::from("/home/u/.config/UltimatePCManager")));
        assert_eq!(l(Folder::Data), Some(PathBuf::from("/home/u/.local/share/UltimatePCManager")));
        assert_eq!(l(Folder::Cache), Some(PathBuf::from("/home/u/.cache/UltimatePCManager")));
        assert_eq!(l(Folder::Documents), Some(PathBuf::from("/home/u/Documents")));
    }

    #[test]
    fn linux_ignores_empty_and_relative_xdg_values() {
        let env = [("HOME", "/home/u"), ("XDG_DATA_HOME", ""), ("XDG_CONFIG_HOME", "relative/dir"), ("XDG_CACHE_HOME", "."), ("XDG_DOCUMENTS_DIR", "")];
        assert_eq!(at(System::Linux, Folder::Data, &env), Some(PathBuf::from("/home/u/.local/share/UltimatePCManager")));
        assert_eq!(at(System::Linux, Folder::Config, &env), Some(PathBuf::from("/home/u/.config/UltimatePCManager")));
        assert_eq!(at(System::Linux, Folder::Cache, &env), Some(PathBuf::from("/home/u/.cache/UltimatePCManager")));
        assert_eq!(at(System::Linux, Folder::Documents, &env), Some(PathBuf::from("/home/u/Documents")));
    }

    #[test]
    fn linux_without_home_or_xdg_has_no_folders() {
        for kind in [Folder::Config, Folder::Data, Folder::Cache, Folder::Documents] {
            assert_eq!(at(System::Linux, kind, &[("HOME", "")]), None, "{kind:?}");
        }
    }

    #[test]
    fn macos_uses_application_support_and_caches() {
        let env = [("HOME", "/Users/u"), ("XDG_CONFIG_HOME", "/ignored")];
        let m = |kind| at(System::MacOs, kind, &env);
        assert_eq!(m(Folder::Config), Some(PathBuf::from("/Users/u/Library/Application Support/UltimatePCManager")));
        assert_eq!(m(Folder::Data), Some(PathBuf::from("/Users/u/Library/Application Support/UltimatePCManager")));
        assert_eq!(m(Folder::Cache), Some(PathBuf::from("/Users/u/Library/Caches/UltimatePCManager")));
        assert_eq!(m(Folder::Documents), Some(PathBuf::from("/Users/u/Documents")));
        assert_eq!(at(System::MacOs, Folder::Data, &[]), None);
    }

    #[test]
    fn the_public_functions_go_through_resolve_with_the_process_environment() {
        assert_eq!(config(), resolve(System::current(), Folder::Config, &process_env));
        assert_eq!(documents(), resolve(System::current(), Folder::Documents, &process_env));
    }
}
