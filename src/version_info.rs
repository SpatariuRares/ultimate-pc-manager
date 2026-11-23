//! What a program file says about itself in its version details: the
//! description Explorer shows (FileDescription). Read on a worker thread,
//! once per path, like the signatures (`signature.rs`), so the UI and the
//! sampler never wait for it. The Processes table's Description column
//! reads it.

use crate::path_cache::PathCache;

/// The description cache: ask for paths, read what is known so far. Each
/// path's description, or None for none, read on a worker thread
/// ([`PathCache`]).
pub struct Descriptions {
    cache: PathCache<Option<String>>,
}

impl Default for Descriptions {
    fn default() -> Self {
        Descriptions { cache: PathCache::new("descriptions", description) }
    }
}

impl Descriptions {
    /// Queues the paths not read or asked for yet; returns at once.
    pub fn ask<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        self.cache.ask(paths);
    }

    /// The description of `path`: None while it is not read yet,
    /// Some(None) for a file without one.
    pub fn of(&self, path: &str) -> Option<Option<String>> {
        self.cache.of(path)
    }
}

/// The FileDescription of the file at `path`, if its version details have
/// a non-empty one. Windows reads the first language the file lists
/// (`\VarFileInfo\Translation`), else the usual US English tables; the other
/// systems have none yet.
pub use imp::description;

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(test)]
mod tests {
    use super::*;

    // Windows only: version resources of an executable.
    #[cfg(windows)]
    #[test]
    fn the_tests_own_executable_shows_its_description() {
        // build.rs embeds the version details into every executable of the crate.
        let me = std::env::current_exe().expect("own path");
        assert_eq!(description(&me.display().to_string()).as_deref(), Some("Ultimate PC Manager"));
    }

    #[test]
    fn a_file_without_version_details_has_no_description() {
        assert_eq!(description(r"C:\no\such\file.exe"), None);
        assert_eq!(description(""), None);
    }
}
