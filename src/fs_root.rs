//! The filesystem-root seam of the Linux readers.
//!
//! Every Linux reader of `/proc`, `/sys` and `/etc` goes through an
//! [`FsRoot`] instead of opening absolute paths. Production uses
//! [`FsRoot::system`] (`/`, no configuration); tests point a root at a
//! checked-in fixture tree under `tests/fixtures/linux`.
//!
//! # Probing, not version numbers
//!
//! Whether the kernel offers a newer interface is decided by asking for it:
//! [`FsRoot::probe`] answers "does this file or folder exist under the
//! root", and the reader falls back when it does not. Nothing here parses
//! a kernel version.

use std::path::PathBuf;

/// A filesystem root: `/` in production, a fixture tree in tests.
#[derive(Debug, Clone)]
pub struct FsRoot {
    root: PathBuf,
}

impl FsRoot {
    /// The running system's own root.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn system() -> Self {
        Self { root: PathBuf::from("/") }
    }

    /// A tree standing in for the system's root (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `rel` (written like `proc/uptime`, without a leading slash) under the root.
    pub fn path(&self, rel: &str) -> PathBuf {
        self.root.join(rel.trim_start_matches('/'))
    }

    /// Whether the interface `rel` exists under the root.
    pub fn probe(&self, rel: &str) -> bool {
        self.path(rel).exists()
    }

    /// The names of the entries of the folder `rel` (links included, names
    /// starting with a dot left out), sorted; empty when it is missing.
    pub fn list_dir(&self, rel: &str) -> Vec<String> {
        let Ok(entries) = std::fs::read_dir(self.path(rel)) else { return Vec::new() };
        let mut names: Vec<String> = entries.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| !n.starts_with('.')).collect();
        names.sort();
        names
    }
    /// The text of `rel`, trimmed; `None` when it is missing, unreadable or empty.
    /// A device-tree string ends in a NUL byte; that is trimmed too.
    pub fn read(&self, rel: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.path(rel)).ok()?;
        let text = text.trim_matches(|c: char| c.is_whitespace() || c == '\0');
        (!text.is_empty()).then(|| text.to_string())
    }

    /// The first of `rels` that can be read.
    pub fn read_first(&self, rels: &[&str]) -> Option<String> {
        rels.iter().find_map(|r| self.read(r))
    }
}

/// The checked-in fixture tree `name` under `tests/fixtures/linux`.
#[cfg(test)]
pub fn fixture(name: &str) -> FsRoot {
    FsRoot::at(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/linux").join(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_root_is_slash_with_no_configuration() {
        assert_eq!(FsRoot::system().path("proc/uptime"), std::path::Path::new("/").join("proc/uptime"));
    }

    #[test]
    fn a_folder_lists_its_entries_sorted_and_a_missing_one_lists_none() {
        assert_eq!(fixture("runit").list_dir("etc/sv"), ["dhcpcd", "socklog", "sshd"]);
        assert!(fixture("runit").list_dir("etc/nothing").is_empty());
    }
    #[test]
    fn a_probe_tells_whether_the_interface_exists_under_the_root() {
        let full = fixture("desktop");
        assert!(full.probe("proc/pressure/memory"));
        assert!(!fixture("arm-board").probe("proc/pressure/memory"));
    }

    #[test]
    fn read_trims_and_hides_missing_or_empty_files() {
        let root = fixture("desktop");
        assert_eq!(root.read("proc/sys/kernel/osrelease").as_deref(), Some("6.8.0-45-generic"));
        assert_eq!(root.read("proc/nothing"), None);
        assert_eq!(fixture("arm-board").read("sys/firmware/devicetree/base/model").as_deref(), Some("Raspberry Pi 4 Model B Rev 1.4"));
    }
}
