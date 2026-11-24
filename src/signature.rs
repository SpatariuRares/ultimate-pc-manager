//! Who signed a program file: WinVerifyTrust on the file's own signature or,
//! for the many Windows files signed only through a catalog, on the catalog
//! entry for its hash. Checked on a worker thread, once per path, so the UI
//! and the sampler never wait for it. "Hide Microsoft Services and
//! Processes" reads which files carry a verified Microsoft Windows
//! signature; a Publisher column can read the signer's name from the same
//! cache.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use crate::path_cache::PathCache;

/// A verified signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Signature {
    /// The signer's name as Windows shows it ("Microsoft Windows").
    pub signer: String,
    /// The signer is Microsoft Windows and its chain ends at a Microsoft root.
    pub microsoft: bool,
}

/// Whether a signer is Microsoft Windows: its certificate's name is
/// "Microsoft Windows" or "Microsoft Windows Publisher" and its chain ends
/// at a Microsoft root, so a look-alike name under another root is not.
#[allow(dead_code)] // the Windows reader and the tests use it
pub fn is_microsoft_windows(signer: &str, microsoft_root: bool) -> bool {
    microsoft_root && matches!(signer, "Microsoft Windows" | "Microsoft Windows Publisher")
}

/// How a file is checked: the real WinVerifyTrust, or a fake in tests.
type Check = fn(&str) -> Option<Signature>;

/// The signature cache: ask for paths, read what is known so far. Each
/// path's verified signature, or None for none, checked on a worker
/// thread ([`PathCache`]).
pub struct Signatures {
    cache: PathCache<Option<Signature>>,
    /// The paths with a verified Microsoft Windows signature, rebuilt when
    /// the cache has grown so readers take it without copying: (the
    /// cache's generation it was built from, the set).
    microsoft: Mutex<(u64, Arc<HashSet<String>>)>,
}

impl Default for Signatures {
    fn default() -> Self {
        Signatures::with_check(verify)
    }
}

impl Signatures {
    fn with_check(check: Check) -> Self {
        Signatures { cache: PathCache::new("signatures", check), microsoft: Mutex::new((0, Arc::default())) }
    }

    /// A cache that already knows `microsoft` as Microsoft Windows signed
    /// and checks nothing else (tests).
    #[cfg(test)]
    pub fn known(microsoft: &[&str]) -> Self {
        let s = Signatures::with_check(|_| None);
        let mut f = s.cache.found();
        for path in microsoft {
            f.by_path.insert(path.to_string(), Some(Signature { signer: "Microsoft Windows".into(), microsoft: true }));
        }
        f.generation += 1;
        drop(f);
        s
    }

    /// Queues the paths not checked or asked for yet; returns at once.
    pub fn ask<'a>(&self, paths: impl IntoIterator<Item = &'a str>) {
        self.cache.ask(paths);
    }

    /// The paths known to carry a verified Microsoft Windows signature.
    pub fn microsoft(&self) -> Arc<HashSet<String>> {
        let f = self.cache.found();
        let mut built = crate::platform::lock_state(&self.microsoft);
        if built.0 != f.generation {
            let set = f.by_path.iter().filter(|(_, sig)| sig.as_ref().is_some_and(|s| s.microsoft)).map(|(path, _)| path.clone()).collect();
            *built = (f.generation, Arc::new(set));
        }
        Arc::clone(&built.1)
    }

    /// The signature of `path`: None while it is not checked yet,
    /// Some(None) for a file without a verified signature.
    pub fn of(&self, path: &str) -> Option<Option<Signature>> {
        self.cache.of(path)
    }
}

/// What the Publisher column shows for a checked file: its verified
/// signer, or "Not verified" for a file without a verified signature.
pub fn publisher(checked: Option<Signature>) -> String {
    checked.map_or_else(|| NOT_VERIFIED.to_string(), |s| s.signer)
}

/// The Publisher column's text for a file without a verified signature.
#[cfg(not(target_os = "linux"))]
pub const NOT_VERIFIED: &str = "Not verified";
/// The Publisher column's text for a file no package owns (Linux shows the
/// owning package where Windows shows the signer).
#[cfg(target_os = "linux")]
pub const NOT_VERIFIED: &str = "Not from a package";

/// The verified signature of the file at `path`, if any: on Windows its own
/// (embedded) signature, else a catalog's; the other systems have none yet.
pub use imp::verify;

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
    use std::time::{Duration, Instant};

    #[test]
    fn only_a_microsoft_windows_signer_under_a_microsoft_root_counts() {
        assert!(is_microsoft_windows("Microsoft Windows", true));
        assert!(is_microsoft_windows("Microsoft Windows Publisher", true));
        assert!(!is_microsoft_windows("Microsoft Windows", false), "a look-alike under another root");
        assert!(!is_microsoft_windows("Valve Corp.", true));
        assert!(!is_microsoft_windows("Microsoft Corporation", true), "Microsoft's other software is not Windows");
    }

    /// A check that takes a while, like WinVerifyTrust on a large file.
    fn slow(path: &str) -> Option<Signature> {
        std::thread::sleep(Duration::from_millis(200));
        (path == "ms.exe").then(|| Signature { signer: "Microsoft Windows".into(), microsoft: true })
    }

    #[test]
    fn asking_returns_at_once_and_the_answers_arrive_from_the_worker() {
        let s = Signatures::with_check(slow);
        let start = Instant::now();
        s.ask(["ms.exe", "other.exe", "ms.exe", ""]);
        assert!(start.elapsed() < Duration::from_millis(100), "asking does not wait for the checks");
        assert_eq!(s.of("ms.exe"), None, "not checked yet");
        let deadline = Instant::now() + Duration::from_secs(5);
        while s.of("other.exe").is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(s.of("other.exe"), Some(None), "checked: no verified signature");
        assert_eq!(s.of("ms.exe").flatten().map(|s| s.signer), Some("Microsoft Windows".to_string()));
        assert_eq!(*s.microsoft(), HashSet::from(["ms.exe".to_string()]));
    }

    #[cfg(windows)]
    #[test]
    fn the_command_prompt_is_microsoft_windows_signed_and_the_tests_are_not() {
        let own = std::env::current_exe().expect("own path");
        assert!(!verify(&own.to_string_lossy()).is_some_and(|s| s.microsoft), "the test binary is not Microsoft's");
        let cmd = std::env::var("ComSpec").unwrap_or_else(|_| r"C:\Windows\System32\cmd.exe".into());
        let sig = verify(&cmd).expect("cmd.exe carries a verified signature");
        assert!(sig.microsoft, "{sig:?}");
    }
}
