//! Catalogs: where packages come from. Every catalog answers the same
//! interface ([`PackageSource`]) with plain data (no egui or Windows type),
//! and the registry ([`SOURCES`]) lists them. A package is known by its
//! catalog and its id: ("winget", "VideoLAN.VLC"), ("msstore", "9NBLGGH4VVNH").
//!
//! v1 has one source, winget, which serves two catalogs: its own community
//! catalog and the Microsoft Store (reached with `--source msstore`).
//! Adding Scoop or Chocolatey means one module and one row in [`SOURCES`].

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::platform::winget::{parse_local_id, parse_search_table, parse_source_list, parse_winget_table, SearchRow, ELLIPSIS, NO_PACKAGE_FOUND};
use crate::platform::{AddonContext, Ran};

use super::installed::Package;
use super::readiness::{self, Readiness};

/// A package a search found.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Found {
    pub name: String,
    /// The package's whole id (never a shortened one).
    pub id: String,
    pub version: String,
    /// The catalog it comes from ("winget", "msstore").
    pub catalog: String,
    /// It is installed on this PC already.
    pub installed: bool,
}

/// A package installed on this PC, as a catalog knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPackage {
    pub catalog: String,
    pub id: String,
    pub version: String,
}

/// Who a package is installed for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    User,
    Machine,
}

impl Scope {
    /// The word catalogs and bundles use ("user", "machine").
    pub fn word(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Machine => "machine",
        }
    }

    /// The scope a word names, if any.
    pub fn from_word(word: &str) -> Option<Scope> {
        match word {
            "user" => Some(Scope::User),
            "machine" => Some(Scope::Machine),
            _ => None,
        }
    }
}

/// What a bundle may fix about an install besides the package: its version
/// and its scope (both validated before they get here). Nothing else ever
/// reaches the catalog's arguments.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pin {
    pub version: Option<String>,
    pub scope: Option<Scope>,
}

/// One package to install or update.
#[derive(Clone, Debug)]
pub struct Target<'a> {
    pub catalog: &'a str,
    pub id: &'a str,
    /// Where the installer's log goes.
    pub log: &'a Path,
    /// Accept the package's and the catalog's agreements without asking.
    pub accept_agreements: bool,
    /// The version and scope a bundle asked for.
    pub pin: &'a Pin,
}

/// A catalog source (see the module docs). `None` from a method that takes
/// `stop`: it was set, and the work was ended.
pub trait PackageSource: Send + Sync {
    /// The source's id ("winget").
    #[cfg_attr(not(test), allow(dead_code))] // in the catalog interface #113 defines; v1 has one source and nothing asks it yet
    fn id(&self) -> &'static str;
    /// The catalogs it serves, by the name packages carry.
    fn catalogs(&self) -> &'static [&'static str];
    /// Whether installing `id` needs administrator rights, when known.
    fn needs_admin(&self, id: &str) -> Option<bool>;
    /// It can install for the current user only.
    #[cfg_attr(not(test), allow(dead_code))] // in the catalog interface #113 defines; v1 has one source and nothing asks it yet
    fn supports_user_scope(&self) -> bool;
    /// It can uninstall programs it did not install.
    #[cfg_attr(not(test), allow(dead_code))] // in the catalog interface #113 defines; v1 has one source and nothing asks it yet
    fn can_uninstall_foreign(&self) -> bool;
    /// Whether it can be used, and how to repair it.
    fn availability(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Readiness>;
    /// Lets it bring its catalogs up to date (slow the first time); `Err`
    /// says why it could not.
    fn refresh_catalogs(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Result<(), String>>;
    /// The catalogs it serves that are not active on this PC (removed or
    /// disabled there); empty when it could not tell.
    fn inactive_catalogs(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Vec<String>>;
    /// The packages matching `query` in every catalog it serves.
    fn search(&self, ctx: &AddonContext, query: &str, accept_agreements: bool, stop: &AtomicBool) -> Option<Result<Vec<Found>, String>>;
    /// Every package installed on this PC as it lists them, also those no
    /// catalog handles, with the newer version its catalog has; the second
    /// value is how many rows were left out (a shortened id is never
    /// updated or uninstalled).
    fn list_packages(&self, ctx: &AddonContext, accept_agreements: bool, stop: &AtomicBool) -> Option<Result<(Vec<Package>, usize), String>>;
    /// The packages installed on this PC that its catalogs know.
    fn list_installed(&self, ctx: &AddonContext, accept_agreements: bool, stop: &AtomicBool) -> Option<Result<Vec<InstalledPackage>, String>>;
    /// The version the package shows as installed now: `Some(None)` when
    /// it is not installed, `None` when it could not be asked.
    fn installed_version(&self, ctx: &AddonContext, catalog: &str, id: &str, accept_agreements: bool, stop: &AtomicBool) -> Option<Option<String>>;
    /// Whether the package shows as installed now (`None` when it could not
    /// be asked).
    fn is_installed(&self, ctx: &AddonContext, catalog: &str, id: &str, accept_agreements: bool, stop: &AtomicBool) -> Option<bool> {
        self.installed_version(ctx, catalog, id, accept_agreements, stop).map(|v| v.is_some())
    }
    /// The id Windows lists the installed package under, when it names an
    /// uninstall registry entry (`ARP\{Machine|User}\{X64|X86}\{key}`);
    /// `None` when it does not or could not be asked.
    fn local_id(&self, ctx: &AddonContext, catalog: &str, id: &str, accept_agreements: bool, stop: &AtomicBool) -> Option<String>;
    /// Installs a package; `on_line` gets each line of its output.
    fn install(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran;
    /// Updates an installed package.
    fn update(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran;
    /// Uninstalls a package.
    fn uninstall(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran;
}

/// The registry of catalog sources.
pub static SOURCES: &[&dyn PackageSource] = &[&Winget];

/// The source serving `catalog`, if one is registered.
pub fn source_of(catalog: &str) -> Option<&'static dyn PackageSource> {
    SOURCES.iter().copied().find(|s| s.catalogs().contains(&catalog))
}

// ---------------------------------------------------------------- winget

/// winget: its community catalog and the Microsoft Store.
pub struct Winget;

/// winget's catalogs.
const WINGET_CATALOGS: &[&str] = &["winget", "msstore"];

impl Winget {
    /// Runs winget with `args`, its output in a temporary file (deleted
    /// afterwards). None when `stop` ended it.
    fn ask(ctx: &AddonContext, args: &[&str], stop: &AtomicBool) -> Option<Result<(i32, String), String>> {
        match readiness::ask(ctx, "winget", args, stop)? {
            readiness::Answer::Exited(code, text) => Some(Ok((code, text))),
            readiness::Answer::NotStarted => Some(Err("winget did not start.".into())),
        }
    }

    /// The whole row a shortened one stands for: winget asked again for
    /// ids starting with what it showed. `None` when no single row fits
    /// (the row is then left out: a shortened id is never installed).
    fn resolve(ctx: &AddonContext, row: &SearchRow, catalog: &str, accept: bool, stop: &AtomicBool) -> Option<Option<SearchRow>> {
        let prefix = row.id.trim_end_matches(ELLIPSIS);
        let args = with_source_agreements(vec!["search", "--id", prefix, "--source", catalog, "--disable-interactivity"], accept);
        let Ok((0, text)) = Self::ask(ctx, &args, stop)? else { return Some(None) };
        let fits: Vec<SearchRow> = parse_search_table(&text).into_iter().filter(|r| !r.truncated && r.id.starts_with(prefix)).collect();
        let named: Vec<&SearchRow> = fits.iter().filter(|r| r.name == row.name).collect();
        Some(match (fits.len(), named.len()) {
            (1, _) => fits.into_iter().next(),
            (_, 1) => Some(named[0].clone()),
            _ => None,
        })
    }

    /// winget's arguments to install or update `target` (`verb`): the id
    /// as an argument of its own, always exact, silent and logged.
    fn package_args<'a>(verb: &'a str, target: &'a Target, log: &'a str) -> Vec<&'a str> {
        let mut args = vec![verb, "--id", target.id, "--exact", "--source", target.catalog, "--silent", "--disable-interactivity"];
        if verb == "install" {
            args.push("--no-upgrade");
        }
        args.extend(["--log", log]);
        if target.accept_agreements {
            args.extend(["--accept-package-agreements", "--accept-source-agreements"]);
        }
        if let Some(version) = &target.pin.version {
            args.extend(["--version", version.as_str()]);
        }
        if let Some(scope) = target.pin.scope {
            args.extend(["--scope", scope.word()]);
        }
        args
    }

    /// Runs winget for a package, its output in a temporary file.
    fn run_package(ctx: &AddonContext, args: &[&str], stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        let output = readiness::temp_output();
        let ran = ctx.run("winget", args, &output, stop, on_line);
        let _ = std::fs::remove_file(&output);
        ran
    }
}

impl PackageSource for Winget {
    fn id(&self) -> &'static str {
        "winget"
    }
    fn catalogs(&self) -> &'static [&'static str] {
        WINGET_CATALOGS
    }
    fn needs_admin(&self, _id: &str) -> Option<bool> {
        // winget's search does not say; the installer asks through UAC.
        None
    }
    fn supports_user_scope(&self) -> bool {
        true
    }
    fn can_uninstall_foreign(&self) -> bool {
        true
    }
    fn availability(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Readiness> {
        readiness::check(ctx, stop)
    }
    fn refresh_catalogs(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Result<(), String>> {
        Some(match readiness::ask(ctx, "winget", &["source", "update"], stop)? {
            readiness::Answer::Exited(0, _) => Ok(()),
            readiness::Answer::Exited(code, _) => Err(crate::platform::winget::describe(code)),
            readiness::Answer::NotStarted => Err("winget did not start.".into()),
        })
    }
    fn inactive_catalogs(&self, ctx: &AddonContext, stop: &AtomicBool) -> Option<Vec<String>> {
        Some(match Self::ask(ctx, &["source", "list", "--disable-interactivity"], stop)? {
            Ok((0, text)) => {
                let active = parse_source_list(&text);
                WINGET_CATALOGS.iter().filter(|c| !active.iter().any(|a| a.eq_ignore_ascii_case(c))).map(|c| c.to_string()).collect()
            }
            _ => Vec::new(),
        })
    }
    fn search(&self, ctx: &AddonContext, query: &str, accept: bool, stop: &AtomicBool) -> Option<Result<Vec<Found>, String>> {
        let args = with_source_agreements(vec!["search", "--query", query, "--disable-interactivity"], accept);
        let text = match Self::ask(ctx, &args, stop)? {
            Ok((0, text)) => text,
            Ok((NO_PACKAGE_FOUND, _)) => return Some(Ok(Vec::new())),
            Ok((code, _)) => return Some(Err(format!("The search failed: {}.", crate::platform::winget::describe(code)))),
            Err(e) => return Some(Err(e)),
        };
        let mut found = Vec::new();
        for row in parse_search_table(&text) {
            // One catalog searched alone prints no Source column.
            let catalog = if row.source.is_empty() { "winget".to_string() } else { row.source.clone() };
            let row = if row.truncated {
                match Self::resolve(ctx, &row, &catalog, accept, stop)? {
                    Some(whole) => whole,
                    None => continue,
                }
            } else {
                row
            };
            found.push(Found { name: row.name, id: row.id, version: row.version, catalog, installed: false });
        }
        Some(Ok(found))
    }
    fn list_installed(&self, ctx: &AddonContext, accept: bool, stop: &AtomicBool) -> Option<Result<Vec<InstalledPackage>, String>> {
        let file = readiness::temp_output().with_extension("json");
        let path = file.to_string_lossy().into_owned();
        let args = with_source_agreements(vec!["export", "--output", path.as_str(), "--include-versions", "--disable-interactivity"], accept);
        let asked = Self::ask(ctx, &args, stop);
        let json = std::fs::read(&file).map(|b| String::from_utf8_lossy(&b).into_owned());
        let _ = std::fs::remove_file(&file);
        if let Err(e) = asked? {
            return Some(Err(e));
        }
        // winget may exit with an error after writing the file (packages no
        // catalog knows): the file is what counts.
        Some(json.map_err(|_| "winget did not list the installed packages.".to_string()).and_then(|j| parse_export(&j)))
    }
    fn list_packages(&self, ctx: &AddonContext, accept: bool, stop: &AtomicBool) -> Option<Result<(Vec<Package>, usize), String>> {
        let args = with_source_agreements(vec!["list", "--disable-interactivity"], accept);
        Some(match Self::ask(ctx, &args, stop)? {
            Err(e) => Err(e),
            Ok((code, text)) => {
                let (packages, shortened) = parse_list(&text);
                if packages.is_empty() && code != 0 {
                    Err(format!("winget could not list the installed packages: {}.", crate::platform::winget::describe(code)))
                } else {
                    Ok((packages, shortened))
                }
            }
        })
    }
    fn installed_version(&self, ctx: &AddonContext, _catalog: &str, id: &str, accept: bool, stop: &AtomicBool) -> Option<Option<String>> {
        let args = with_source_agreements(vec!["list", "--id", id, "--exact", "--disable-interactivity"], accept);
        match Self::ask(ctx, &args, stop)? {
            Ok((0, text)) => Some(parse_winget_table(&text).into_iter().find(|r| r.id.eq_ignore_ascii_case(id)).map(|r| r.version)),
            Ok((NO_PACKAGE_FOUND, _)) => Some(None),
            _ => None,
        }
    }
    fn local_id(&self, ctx: &AddonContext, _catalog: &str, id: &str, accept: bool, stop: &AtomicBool) -> Option<String> {
        // `--details` names the entry ("Local Identifier"); older winget
        // versions refuse the flag, and the id is then not known.
        let args = with_source_agreements(vec!["list", "--id", id, "--exact", "--details", "--disable-interactivity"], accept);
        match Self::ask(ctx, &args, stop)? {
            Ok((0, text)) => parse_local_id(&text),
            _ => None,
        }
    }
    fn install(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        let log = target.log.to_string_lossy();
        Self::run_package(ctx, &Self::package_args("install", target, &log), stop, on_line)
    }
    fn update(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        let log = target.log.to_string_lossy();
        Self::run_package(ctx, &Self::package_args("upgrade", target, &log), stop, on_line)
    }
    fn uninstall(&self, ctx: &AddonContext, target: &Target, stop: &AtomicBool, on_line: &mut dyn FnMut(&str)) -> Ran {
        let log = target.log.to_string_lossy();
        let args = ["uninstall", "--id", target.id, "--exact", "--source", target.catalog, "--silent", "--disable-interactivity", "--log", &log];
        Self::run_package(ctx, &args, stop, on_line)
    }
}

/// `args` with `--accept-source-agreements` when the catalogs' agreements
/// are accepted.
fn with_source_agreements(mut args: Vec<&str>, accept: bool) -> Vec<&str> {
    if accept {
        args.push("--accept-source-agreements");
    }
    args
}

/// The rows of `winget list` as packages; a row whose catalog no source
/// serves keeps an empty catalog. Rows whose id winget shortened are left
/// out (never updated or uninstalled by a shortened id); the second value
/// says how many.
pub fn parse_list(text: &str) -> (Vec<Package>, usize) {
    let mut shortened = 0;
    let mut out = Vec::new();
    for row in parse_winget_table(text) {
        if row.id.ends_with(ELLIPSIS) {
            shortened += 1;
            continue;
        }
        let catalog = if source_of(&row.source).is_some() { row.source } else { String::new() };
        out.push(Package { name: row.name, id: row.id, version: row.version, available: row.available, catalog });
    }
    (out, shortened)
}

/// The packages of a `winget export` file, by catalog.
pub fn parse_export(json: &str) -> Result<Vec<InstalledPackage>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("winget's list of installed packages could not be read: {e}"))?;
    let mut out = Vec::new();
    for source in v["Sources"].as_array().into_iter().flatten() {
        let catalog = source["SourceDetails"]["Name"].as_str().unwrap_or_default();
        for p in source["Packages"].as_array().into_iter().flatten() {
            if let Some(id) = p["PackageIdentifier"].as_str() {
                out.push(InstalledPackage { catalog: catalog.into(), id: id.into(), version: p["Version"].as_str().unwrap_or_default().into() });
            }
        }
    }
    Ok(out)
}

/// Marks the packages of `found` that `installed` lists.
pub fn mark_installed(found: &mut [Found], installed: &[InstalledPackage]) {
    for f in found {
        f.installed = installed.iter().any(|p| p.catalog == f.catalog && p.id.eq_ignore_ascii_case(&f.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXPORT: &str = r#"{
        "$schema": "https://aka.ms/winget-packages.schema.2.0.json",
        "CreationDate": "2026-11-18T10:00:00.000-00:00",
        "Sources": [
            { "Packages": [ { "PackageIdentifier": "VideoLAN.VLC", "Version": "3.0.21" }, { "PackageIdentifier": "7zip.7zip" } ],
              "SourceDetails": { "Argument": "https://cdn.winget.microsoft.com/cache", "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe", "Name": "winget", "Type": "Microsoft.PreIndexed.Package" } },
            { "Packages": [ { "PackageIdentifier": "9NBLGGH4VVNH" } ],
              "SourceDetails": { "Argument": "https://storeedgefd.dsx.mp.microsoft.com/v9.0", "Identifier": "StoreEdgeFD", "Name": "msstore", "Type": "Microsoft.Rest" } }
        ],
        "WinGetVersion": "1.9.25200"
    }"#;

    #[test]
    fn an_export_lists_the_packages_by_catalog() {
        let p = parse_export(EXPORT).unwrap();
        assert_eq!(
            p,
            vec![
                InstalledPackage { catalog: "winget".into(), id: "VideoLAN.VLC".into(), version: "3.0.21".into() },
                InstalledPackage { catalog: "winget".into(), id: "7zip.7zip".into(), version: "".into() },
                InstalledPackage { catalog: "msstore".into(), id: "9NBLGGH4VVNH".into(), version: "".into() },
            ]
        );
    }

    #[test]
    fn an_export_that_is_not_json_is_an_error() {
        assert!(parse_export("Installed package is not available").is_err());
    }

    #[test]
    fn installed_marks_need_the_same_catalog_and_id_in_any_case() {
        let mut found = vec![
            Found { id: "videolan.vlc".into(), catalog: "winget".into(), ..Default::default() },
            Found { id: "VideoLAN.VLC".into(), catalog: "msstore".into(), ..Default::default() },
            Found { id: "Git.Git".into(), catalog: "winget".into(), ..Default::default() },
        ];
        mark_installed(&mut found, &parse_export(EXPORT).unwrap());
        assert_eq!(found.iter().map(|f| f.installed).collect::<Vec<_>>(), [true, false, false]);
    }

    /// The arguments of the one winget run `act` makes.
    fn winget_args(act: impl FnOnce(&AddonContext)) -> Vec<String> {
        let (ctx, runner) = AddonContext::recording();
        act(&ctx);
        match runner.launches().as_slice() {
            [crate::platform::Launch::Run { program, args, .. }] if program == "winget" => args.clone(),
            other => panic!("expected one winget run, got {other:?}"),
        }
    }

    #[test]
    fn update_and_uninstall_name_the_package_exactly_and_log() {
        let never = AtomicBool::new(false);
        let log = std::env::temp_dir().join("upcm-pm-test.log");
        let target = Target { catalog: "msstore", id: "9NBLGGH4VVNH", log: &log, accept_agreements: false, pin: &Pin::default() };
        let l = log.to_string_lossy().into_owned();
        let update = winget_args(|ctx| drop(Winget.update(ctx, &target, &never, &mut |_| {})));
        assert_eq!(update, ["upgrade", "--id", "9NBLGGH4VVNH", "--exact", "--source", "msstore", "--silent", "--disable-interactivity", "--log", &l]);
        let uninstall = winget_args(|ctx| drop(Winget.uninstall(ctx, &target, &never, &mut |_| {})));
        assert_eq!(uninstall, ["uninstall", "--id", "9NBLGGH4VVNH", "--exact", "--source", "msstore", "--silent", "--disable-interactivity", "--log", &l]);
    }

    /// Tiny, well-known portable tools from the winget catalog, tried in
    /// this order by the real-PC test.
    const REAL_PACKAGES: [&str; 4] = ["jqlang.jq", "junegunn.fzf", "sharkdp.fd", "BurntSushi.ripgrep.MSVC"];

    /// On a real PC with winget. A package that was installed before the
    /// test is never uninstalled: installing the first one already there
    /// must be skipped ("already installed"). The first one not installed
    /// is searched for, installed, checked, uninstalled and checked gone.
    /// Run by hand: `cargo test real_winget -- --ignored`.
    #[test]
    #[ignore = "installs and uninstalls a real package with winget"]
    fn real_winget_searches_installs_and_uninstalls_a_small_package() {
        let ctx = AddonContext::system();
        let never = AtomicBool::new(false);
        let installed: Vec<(&str, bool)> = REAL_PACKAGES
            .iter()
            .map(|&id| (id, Winget.is_installed(&ctx, "winget", id, true, &never).unwrap_or_else(|| panic!("winget tells whether {id} is installed"))))
            .collect();
        let log = |id: &str| std::env::temp_dir().join(format!("upcm-pm-real-{}-{id}.log", std::process::id()));
        if let Some(&(id, _)) = installed.iter().find(|(_, on)| *on) {
            // Already there before the test: installing it is skipped, and it stays.
            let path = log(id);
            let target = Target { catalog: "winget", id, log: &path, accept_agreements: true, pin: &Pin::default() };
            let mut lines = Vec::new();
            let ran = Winget.install(&ctx, &target, &never, &mut |l| lines.push(l.to_string()));
            let Ran::Exited(code) = ran else { panic!("{id}: {ran:?} {lines:?}") };
            assert!(matches!(crate::platform::winget::classify(code), crate::platform::winget::ExitClass::Skipped(_)), "{id}: exit {code} {lines:?}");
            assert_eq!(Winget.is_installed(&ctx, "winget", id, true, &never), Some(true), "{id} stays installed");
            let _ = std::fs::remove_file(&path);
        }
        let Some(&(id, _)) = installed.iter().find(|(_, on)| !*on) else {
            eprintln!("every package of {REAL_PACKAGES:?} was already installed: the install and uninstall were not tried");
            return;
        };
        let found = Winget.search(&ctx, id, true, &never).unwrap().expect("the search runs");
        let package = found.iter().find(|f| f.id == id && f.catalog == "winget").unwrap_or_else(|| panic!("{id} is found"));
        assert!(!package.installed, "{id} is not installed before the test");
        let path = log(id);
        let target = Target { catalog: "winget", id, log: &path, accept_agreements: true, pin: &Pin::default() };
        let mut lines = Vec::new();
        assert_eq!(Winget.install(&ctx, &target, &never, &mut |l| lines.push(l.to_string())), Ran::Exited(0), "{lines:?}");
        assert_eq!(Winget.is_installed(&ctx, "winget", id, true, &never), Some(true));
        let listed = Winget.list_installed(&ctx, true, &never).unwrap().expect("the export runs");
        assert!(listed.iter().any(|p| p.id == id), "{id} is listed");
        assert_eq!(Winget.uninstall(&ctx, &target, &never, &mut |_| {}), Ran::Exited(0));
        assert_eq!(Winget.is_installed(&ctx, "winget", id, true, &never), Some(false));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn availability_is_winget_readiness() {
        let (ctx, runner) = AddonContext::recording();
        runner.prepare(crate::platform::When::Arg("--version".into()), 0, "v1.9.25200");
        assert_eq!(Winget.availability(&ctx, &AtomicBool::new(false)), Some(Readiness::Ready("1.9.25200".into())));
    }

    #[test]
    fn refreshing_the_catalogs_is_winget_source_update() {
        let stop = AtomicBool::new(false);
        let (ctx, runner) = AddonContext::recording();
        runner.prepare(crate::platform::When::Arg("source".into()), 0, "Done");
        assert_eq!(Winget.refresh_catalogs(&ctx, &stop), Some(Ok(())));
        assert_eq!(runner.launches().len(), 1);
        let (ctx, runner) = AddonContext::recording();
        runner.prepare(crate::platform::When::Arg("source".into()), 1, "boom");
        let Some(Err(why)) = Winget.refresh_catalogs(&ctx, &stop) else { panic!("a failed update is an error") };
        assert_eq!(why, crate::platform::winget::describe(1));
    }

    #[test]
    fn the_registry_has_winget_serving_its_catalog_and_the_store() {
        assert_eq!(SOURCES.iter().map(|s| s.id()).collect::<Vec<_>>(), ["winget"]);
        assert_eq!(source_of("winget").map(|s| s.id()), Some("winget"));
        assert_eq!(source_of("msstore").map(|s| s.id()), Some("winget"));
        assert!(source_of("scoop").is_none());
        let w = source_of("winget").unwrap();
        assert_eq!((w.needs_admin("Git.Git"), w.supports_user_scope(), w.can_uninstall_foreign()), (None, true, true));
    }
}
