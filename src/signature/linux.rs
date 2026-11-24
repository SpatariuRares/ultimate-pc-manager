//! Linux: a program has no signature, but it has an owner: the package that
//! installed it. The Publisher column shows that package, taken from the
//! system's package database (dpkg, pacman or rpm) through the process
//! runner, once per program file.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use crate::fs_root::FsRoot;
use crate::platform::{ProcessRunner, SystemRunner};
use crate::services::sources::{run_text, Output};
use crate::signature::Signature;

/// The package databases this app can ask, in the order they are tried.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Database {
    Dpkg,
    Pacman,
    Rpm,
}

impl Database {
    /// The database the system has, found by probing for its files (never
    /// by the distribution's name).
    pub fn of(root: &FsRoot) -> Option<Self> {
        if root.probe("var/lib/dpkg/status") {
            Some(Self::Dpkg)
        } else if root.probe("var/lib/pacman/local") {
            Some(Self::Pacman)
        } else if root.probe("var/lib/rpm") || root.probe("usr/lib/sysimage/rpm") {
            Some(Self::Rpm)
        } else {
            None
        }
    }

    /// The program and arguments that ask which package owns `path`.
    fn query(self, path: &str) -> (&'static str, Vec<&str>) {
        match self {
            Self::Dpkg => ("dpkg", vec!["-S", path]),
            Self::Pacman => ("pacman", vec!["-Qoq", path]),
            Self::Rpm => ("rpm", vec!["-qf", "--queryformat", "%{NAME}\\n", path]),
        }
    }

    /// The package named in the answer of [`Self::query`] (the first when
    /// several own the file).
    fn package_in(self, answer: &str) -> Option<String> {
        let name = match self {
            // "openssh-client:amd64, other: /usr/bin/ssh"; a line about a
            // diversion is not an owner.
            Self::Dpkg => {
                let line = answer.lines().find(|l| !l.starts_with("diversion by"))?;
                let (owners, _) = line.split_once(": /")?;
                owners.split(',').next()?.trim().split(':').next()?.to_string()
            }
            Self::Pacman | Self::Rpm => answer.lines().next()?.trim().to_string(),
        };
        (!name.is_empty() && !name.contains(char::is_whitespace)).then_some(name)
    }
}

/// The package that owns the program file at `path`, found once per file.
pub struct PackageOwners<'a> {
    root: FsRoot,
    runner: &'a dyn ProcessRunner,
    known: Mutex<HashMap<String, Option<String>>>,
}

impl<'a> PackageOwners<'a> {
    pub fn new(root: FsRoot, runner: &'a dyn ProcessRunner) -> Self {
        Self { root, runner, known: Mutex::new(HashMap::new()) }
    }

    /// The package that owns `path`; `None` for a file no package owns, a
    /// path that is not absolute, or a system without a package database.
    pub fn of(&self, path: &str) -> Option<String> {
        if !path.starts_with('/') || path.contains('\0') {
            return None;
        }
        if let Some(known) = crate::platform::lock_state(&self.known).get(path) {
            return known.clone();
        }
        let owner = self.ask(path);
        crate::platform::lock_state(&self.known).insert(path.to_string(), owner.clone());
        owner
    }

    fn ask(&self, path: &str) -> Option<String> {
        let database = Database::of(&self.root)?;
        let (program, args) = database.query(path);
        match run_text(self.runner, program, &args) {
            Output::Exited(0, answer) => database.package_in(&answer),
            _ => None,
        }
    }
}

static OWNERS: LazyLock<PackageOwners<'static>> = LazyLock::new(|| PackageOwners::new(FsRoot::system(), &SystemRunner));

/// The "signature" of the file at `path`: its package as the signer. Never
/// Microsoft's.
pub fn verify(path: &str) -> Option<Signature> {
    OWNERS.of(path).map(|signer| Signature { signer, microsoft: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;
    use crate::platform::{Launch, RecordingRunner, When};

    fn asked(runner: &RecordingRunner) -> Vec<(String, Vec<String>)> {
        runner
            .launches()
            .into_iter()
            .filter_map(|l| match l {
                Launch::Run { program, args, .. } => Some((program, args)),
                _ => None,
            })
            .collect()
    }

    fn words(w: &[&str]) -> Vec<String> {
        w.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_database_is_found_by_probing_its_files() {
        assert_eq!(Database::of(&fixture("pkg-dpkg")), Some(Database::Dpkg));
        assert_eq!(Database::of(&fixture("pkg-pacman")), Some(Database::Pacman));
        assert_eq!(Database::of(&fixture("pkg-rpm")), Some(Database::Rpm));
        assert_eq!(Database::of(&fixture("desktop")), None);
    }

    #[test]
    fn dpkg_names_the_owning_package_of_a_program() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, "openssh-client:amd64: /usr/bin/ssh\n");
        let owners = PackageOwners::new(fixture("pkg-dpkg"), &runner);
        assert_eq!(owners.of("/usr/bin/ssh").as_deref(), Some("openssh-client"));
        assert_eq!(asked(&runner), [("dpkg".to_string(), words(&["-S", "/usr/bin/ssh"]))]);
    }

    #[test]
    fn pacman_and_rpm_name_the_owning_package_too() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, "openssh\n");
        assert_eq!(PackageOwners::new(fixture("pkg-pacman"), &runner).of("/usr/bin/ssh").as_deref(), Some("openssh"));
        assert_eq!(asked(&runner), [("pacman".to_string(), words(&["-Qoq", "/usr/bin/ssh"]))]);

        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, "openssh-clients\n");
        assert_eq!(PackageOwners::new(fixture("pkg-rpm"), &runner).of("/usr/bin/ssh").as_deref(), Some("openssh-clients"));
        assert_eq!(asked(&runner), [("rpm".to_string(), words(&["-qf", "--queryformat", "%{NAME}\\n", "/usr/bin/ssh"]))]);
    }

    #[test]
    fn a_file_no_package_owns_has_no_owner() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 1, "dpkg-query: no path found matching pattern /home/me/tool\n");
        assert_eq!(PackageOwners::new(fixture("pkg-dpkg"), &runner).of("/home/me/tool"), None);
    }

    #[test]
    fn the_first_of_several_owners_and_a_diversion_line() {
        assert_eq!(Database::Dpkg.package_in("a:amd64, b: /usr/bin/x\n").as_deref(), Some("a"));
        assert_eq!(Database::Dpkg.package_in("diversion by dash from: /bin/sh\nbash: /bin/sh\n").as_deref(), Some("bash"));
        assert_eq!(Database::Rpm.package_in("one\ntwo\n").as_deref(), Some("one"));
        assert_eq!(Database::Pacman.package_in("error: No package owns /x\n"), None, "a message is not a package");
    }

    #[test]
    fn each_program_file_is_asked_once() {
        let runner = RecordingRunner::default();
        runner.prepare(When::Nth(0), 0, "bash: /bin/bash\n");
        let owners = PackageOwners::new(fixture("pkg-dpkg"), &runner);
        assert_eq!(owners.of("/bin/bash").as_deref(), Some("bash"));
        assert_eq!(owners.of("/bin/bash").as_deref(), Some("bash"));
        assert_eq!(asked(&runner).len(), 1, "the second look is from the cache");
        runner.prepare(When::Nth(1), 1, "");
        assert_eq!(owners.of("/opt/x"), None);
        assert_eq!(owners.of("/opt/x"), None);
        assert_eq!(asked(&runner).len(), 2, "an unowned file is remembered too");
    }

    #[test]
    fn nothing_is_asked_without_a_database_or_for_a_path_that_is_not_absolute() {
        let runner = RecordingRunner::default();
        assert_eq!(PackageOwners::new(fixture("desktop"), &runner).of("/usr/bin/ssh"), None);
        assert_eq!(PackageOwners::new(fixture("pkg-dpkg"), &runner).of("ssh"), None);
        assert_eq!(PackageOwners::new(fixture("pkg-dpkg"), &runner).of(""), None);
        assert!(asked(&runner).is_empty());
    }

    #[test]
    fn a_program_that_cannot_be_started_has_no_owner() {
        let runner = RecordingRunner::default();
        runner.answer("dpkg not found");
        assert_eq!(PackageOwners::new(fixture("pkg-dpkg"), &runner).of("/usr/bin/ssh"), None);
    }

    #[test]
    fn the_publisher_column_shows_the_package_and_names_a_file_without_one() {
        let owned = Some(Signature { signer: "openssh-client".into(), microsoft: false });
        assert_eq!(crate::signature::publisher(owned), "openssh-client");
        assert_eq!(crate::signature::publisher(None), "Not from a package");
    }
}
