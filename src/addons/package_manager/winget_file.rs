//! `winget export` files: imported as a bundle and written from one, so a
//! bundle also works without the app (`winget import -i <file>`).
//!
//! The file is winget's packages list (schema
//! `https://aka.ms/winget-packages.schema.2.0.json`): `Sources`, each with
//! its `SourceDetails` (`Name` is the catalog) and its `Packages`
//! (`PackageIdentifier`, optional `Version` and `Scope`).
//!
//! - Import takes each package's catalog, id, version and scope, validated
//!   as a bundle's are; an invalid package is reported and skipped. Nothing
//!   else is kept: not `InitialOverrideArguments` nor `InitialCustomSwitches`.
//! - Export writes the package entries of the catalogs winget serves, with
//!   their version and scope; manual entries are left out.

use std::path::Path;

use serde_json::{json, Value};

use crate::platform::lock_state;

use super::bundle::{self, Bundle, Entry, Opened, Rest, SCHEMA_VERSION};
use super::catalog::{Pin, Scope};
use super::PackageManager;

/// winget's packages list schema.
const SCHEMA: &str = "https://aka.ms/winget-packages.schema.2.0.json";

/// The catalogs winget serves, with the `SourceDetails` `winget import`
/// needs for each (name, argument, identifier, type), in the order written.
const SOURCES: &[(&str, &str, &str, &str)] = &[
    ("winget", "https://cdn.winget.microsoft.com/cache", "Microsoft.Winget.Source_8wekyb3d8bbwe", "Microsoft.PreIndexed.Package"),
    ("msstore", "https://storeedgefd.dsx.mp.microsoft.com/v9.0", "StoreEdgeFD", "Microsoft.Rest"),
];

/// A `winget export` file written from bundle entries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exported {
    /// The file's text (pretty JSON).
    pub text: String,
    /// How many entries were left out: manual ones and those of a catalog
    /// winget does not serve.
    pub left_out: usize,
}

/// Whether `text` is a `winget export` file rather than a bundle (an
/// object with `Sources` and no `schema_version`).
pub fn is_winget_export(text: &str) -> bool {
    matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(o)) if o.contains_key("Sources") && !o.contains_key("schema_version"))
}

/// Reads a `winget export` file as a bundle. Fails only when the text is
/// not such a file at all; a bad package is skipped and said why.
pub fn import(text: &str) -> Result<Opened, String> {
    const NOT_AN_EXPORT: &str = "This file is not a winget export";
    let value: Value = serde_json::from_str(text).map_err(|e| format!("{NOT_AN_EXPORT}: {e}."))?;
    let Value::Object(top) = value else { return Err(format!("{NOT_AN_EXPORT}.")) };
    let Some(Value::Array(sources)) = top.get("Sources") else { return Err(format!("{NOT_AN_EXPORT}: it has no list of sources.")) };
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    let mut n = 0;
    for source in sources {
        let catalog = source["SourceDetails"]["Name"].as_str().map(str::trim).unwrap_or_default();
        for p in source["Packages"].as_array().into_iter().flatten() {
            n += 1;
            match package(catalog, p) {
                Ok(e) => entries.push(e),
                Err(why) => skipped.push(format!("Package {n} was skipped: {why}.")),
            }
        }
    }
    let created = top.get("CreationDate").and_then(Value::as_str).unwrap_or_default().to_string();
    Ok(Opened { bundle: Bundle { schema_version: SCHEMA_VERSION, created, entries, rest: Rest::new() }, skipped, newer: false })
}

/// One package of the file, validated; only its id, version and scope.
fn package(catalog: &str, p: &Value) -> Result<Entry, String> {
    if catalog.is_empty() {
        return Err("its source names no catalog".into());
    }
    let text = |key: &str| -> Result<Option<String>, String> {
        match p.get(key) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("its {key} is not text")),
        }
    };
    let id = text("PackageIdentifier")?.ok_or("it has no id")?;
    bundle::check_token("id", &id)?;
    let version = text("Version")?;
    if let Some(v) = &version {
        bundle::check_token("version", v)?;
    }
    let scope = match text("Scope")? {
        None => None,
        Some(word) => Some(Scope::from_word(&word.to_ascii_lowercase()).ok_or_else(|| format!("its scope \u{201c}{word}\u{201d} is neither user nor machine"))?),
    };
    Ok(Entry::Package { catalog: catalog.into(), id, pin: Pin { version, scope }, name: None, rest: Rest::new() })
}

/// The `winget export` file of `entries` (`created` as its date): the
/// packages of the catalogs winget serves, grouped by catalog.
pub fn export(entries: &[Entry], created: &str) -> Exported {
    let mut sources = Vec::new();
    let mut written = 0;
    for (name, argument, identifier, kind) in SOURCES {
        let packages: Vec<Value> = entries
            .iter()
            .filter_map(|e| match e {
                Entry::Package { catalog, id, pin, .. } if catalog == name => {
                    let mut p = Rest::new();
                    p.insert("PackageIdentifier".into(), id.as_str().into());
                    if let Some(v) = &pin.version {
                        p.insert("Version".into(), v.as_str().into());
                    }
                    if let Some(s) = pin.scope {
                        p.insert("Scope".into(), s.word().into());
                    }
                    Some(Value::Object(p))
                }
                _ => None,
            })
            .collect();
        if packages.is_empty() {
            continue;
        }
        written += packages.len();
        sources.push(json!({
            "Packages": packages,
            "SourceDetails": { "Argument": argument, "Identifier": identifier, "Name": name, "Type": kind }
        }));
    }
    let file = json!({ "$schema": SCHEMA, "CreationDate": created, "Sources": sources });
    Exported { text: serde_json::to_string_pretty(&file).unwrap_or_default(), left_out: entries.len() - written }
}

impl PackageManager {
    /// Writes the ticked entries of the bundle on the page to `path` as a
    /// `winget export` file, and says where and what was left out.
    pub(super) fn export_winget(&mut self, path: &Path) {
        let mut st = lock_state(&self.state);
        let Some(view) = &mut st.bundle else { return };
        let Some(rows) = bundle::rows_of(view) else { return };
        let entries: Vec<Entry> = rows.iter().filter(|r| r.ticked).map(|r| r.entry.clone()).collect();
        let manual = entries.iter().filter(|e| matches!(e, Entry::Manual { .. })).count();
        let exported = export(&entries, &bundle::now());
        let text = match std::fs::write(path, &exported.text) {
            Err(e) => format!("Could not export {}: {e}", path.display()),
            Ok(()) => {
                let mut text = format!("Exported for winget to {}.", path.display());
                match manual {
                    0 => {}
                    1 => text.push_str(" 1 manual entry was left out."),
                    n => text.push_str(&format!(" {n} manual entries were left out.")),
                }
                text
            }
        };
        bundle::say(view, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(catalog: &str, id: &str, version: Option<&str>, scope: Option<Scope>) -> Entry {
        Entry::Package { catalog: catalog.into(), id: id.into(), pin: Pin { version: version.map(Into::into), scope }, name: None, rest: Rest::new() }
    }

    #[test]
    fn a_winget_export_imports_its_packages_by_catalog_with_version_and_scope() {
        let file = json!({
            "$schema": "https://aka.ms/winget-packages.schema.2.0.json",
            "CreationDate": "2026-11-18T10:00:00.000-00:00",
            "Sources": [
                { "Packages": [ { "PackageIdentifier": "VideoLAN.VLC", "Version": "3.0.21", "Scope": "machine" }, { "PackageIdentifier": "7zip.7zip" } ],
                  "SourceDetails": { "Argument": "https://cdn.winget.microsoft.com/cache", "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe", "Name": "winget", "Type": "Microsoft.PreIndexed.Package" } },
                { "Packages": [ { "PackageIdentifier": "9NBLGGH4VVNH", "Scope": "user" } ],
                  "SourceDetails": { "Argument": "https://storeedgefd.dsx.mp.microsoft.com/v9.0", "Identifier": "StoreEdgeFD", "Name": "msstore", "Type": "Microsoft.Rest" } }
            ],
            "WinGetVersion": "1.9.25200"
        });
        let opened = import(&file.to_string()).expect("a winget export");
        assert_eq!(opened.skipped, Vec::<String>::new());
        assert!(!opened.newer);
        assert_eq!(opened.bundle.schema_version, SCHEMA_VERSION);
        assert_eq!(
            opened.bundle.entries,
            [
                package("winget", "VideoLAN.VLC", Some("3.0.21"), Some(Scope::Machine)),
                package("winget", "7zip.7zip", None, None),
                package("msstore", "9NBLGGH4VVNH", None, Some(Scope::User)),
            ]
        );
    }

    #[test]
    fn invalid_packages_are_skipped_and_installer_arguments_never_kept() {
        let file = json!({ "Sources": [ { "SourceDetails": { "Name": "winget" }, "Packages": [
            { "PackageIdentifier": "--override" },
            { "PackageIdentifier": "Git.Git", "Version": "1.0 --override /S" },
            { "PackageIdentifier": "Git.Git", "Scope": "everyone" },
            { "Version": "1.0" },
            { "PackageIdentifier": "Git.Git", "InitialOverrideArguments": "/S /D=C:\\x", "InitialCustomSwitches": "--force", "Channel": "beta" }
        ] } ] });
        let opened = import(&file.to_string()).unwrap();
        assert_eq!(opened.bundle.entries, [package("winget", "Git.Git", None, None)], "nothing but id, version and scope is taken");
        assert_eq!(opened.skipped.len(), 4, "{:?}", opened.skipped);
        assert!(opened.skipped[0].starts_with("Package 1 was skipped") && opened.skipped[0].contains("--override"), "{}", opened.skipped[0]);
        assert!(opened.skipped[2].contains("everyone"), "{}", opened.skipped[2]);
    }

    #[test]
    fn a_file_that_is_not_a_winget_export_is_refused() {
        for text in ["", "not json", "[]", r#"{"schema_version": 1, "entries": []}"#, r#"{"Sources": {}}"#] {
            assert!(import(text).is_err(), "{text:?}");
        }
    }

    #[test]
    fn export_then_import_gives_back_the_packages_without_the_manual_entries() {
        let bundle = Bundle {
            schema_version: SCHEMA_VERSION,
            created: "2026-11-18T09:30:00Z".into(),
            entries: vec![
                package("winget", "Git.Git", Some("2.47.0"), Some(Scope::User)),
                Entry::Manual { name: "Tool".into(), url: "https://example.com".into(), rest: Rest::new() },
                package("msstore", "9NBLGGH4VVNH", None, None),
                package("winget", "VideoLAN.VLC", None, Some(Scope::Machine)),
            ],
            rest: Rest::new(),
        };
        let exported = export(&bundle.entries, &bundle.created);
        assert_eq!(exported.left_out, 1, "the manual entry");
        let back = import(&exported.text).unwrap();
        assert_eq!(back.skipped, Vec::<String>::new());
        assert_eq!(
            back.bundle.entries,
            [
                package("winget", "Git.Git", Some("2.47.0"), Some(Scope::User)),
                package("winget", "VideoLAN.VLC", None, Some(Scope::Machine)),
                package("msstore", "9NBLGGH4VVNH", None, None),
            ]
        );
        assert!(!exported.text.contains("Tool") && !exported.text.contains("example.com"), "{}", exported.text);
    }

    #[test]
    fn the_export_follows_winget_s_schema_so_winget_import_reads_it() {
        let entries = [package("winget", "Git.Git", Some("2.47.0"), None), package("msstore", "9NBLGGH4VVNH", None, Some(Scope::User)), package("scoop", "git", None, None)];
        let exported = export(&entries, "2026-11-18T09:30:00Z");
        assert_eq!(exported.left_out, 1, "a catalog winget does not serve is left out");
        let v: Value = serde_json::from_str(&exported.text).unwrap();
        assert_eq!(v["$schema"], json!("https://aka.ms/winget-packages.schema.2.0.json"));
        assert_eq!(v["CreationDate"], json!("2026-11-18T09:30:00Z"));
        assert_eq!(
            v["Sources"],
            json!([
                { "Packages": [ { "PackageIdentifier": "Git.Git", "Version": "2.47.0" } ],
                  "SourceDetails": { "Argument": "https://cdn.winget.microsoft.com/cache", "Identifier": "Microsoft.Winget.Source_8wekyb3d8bbwe", "Name": "winget", "Type": "Microsoft.PreIndexed.Package" } },
                { "Packages": [ { "PackageIdentifier": "9NBLGGH4VVNH", "Scope": "user" } ],
                  "SourceDetails": { "Argument": "https://storeedgefd.dsx.mp.microsoft.com/v9.0", "Identifier": "StoreEdgeFD", "Name": "msstore", "Type": "Microsoft.Rest" } }
            ])
        );
    }

    #[test]
    fn a_winget_export_is_told_apart_from_a_bundle() {
        assert!(is_winget_export(r#"{ "Sources": [] }"#));
        assert!(!is_winget_export(r#"{ "schema_version": 1, "entries": [] }"#));
        assert!(!is_winget_export("not json"));
    }
}
