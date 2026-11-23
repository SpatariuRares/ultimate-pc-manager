//! XDG autostart: the `.desktop` files the desktop starts at login.
//!
//! The user's files (`$XDG_CONFIG_HOME/autostart`, else `~/.config/autostart`)
//! and the system's (`$XDG_CONFIG_DIRS/autostart`, else `/etc/xdg/autostart`)
//! are read as the desktop reads them: a user file replaces the system file
//! with the same name, `Hidden=true` removes an entry, and `OnlyShowIn` /
//! `NotShowIn` are matched against `XDG_CURRENT_DESKTOP`. System files are
//! never edited: turning an entry off writes a per-user override (a copy with
//! `Hidden=true`), turning it on takes that override away.

use super::super::{StartupEntry, StartupSource};
use crate::app_folders::{xdg_config_home, Env};
use crate::fs_root::FsRoot;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Names this source in [`StartupEntry::source`].
pub const ID: &str = "xdg-autostart";
const USER: &str = "User autostart";
const SYSTEM: &str = "System autostart";
const GROUP: &str = "[Desktop Entry]";
const HIDDEN: &str = "Hidden";
const GNOME_ENABLED: &str = "X-GNOME-Autostart-enabled";

/// Where the autostart files are and which desktop is running.
pub struct XdgAutostart {
    root: FsRoot,
    /// The user's autostart folder, as a path under the root.
    user_dir: Option<String>,
    /// The system's autostart folders, first one wins.
    system_dirs: Vec<String>,
    /// `XDG_CURRENT_DESKTOP`, split at `:`.
    desktops: Vec<String>,
}

/// `path` without its leading slash, as an [`FsRoot`] path.
fn rel(path: &std::path::Path) -> String {
    path.to_string_lossy().trim_start_matches('/').to_string()
}

impl XdgAutostart {
    pub fn new(root: FsRoot, env: Env) -> Self {
        let text = |name: &str| env(name).map(|v| v.to_string_lossy().into_owned()).filter(|v| !v.is_empty());
        let mut system_dirs: Vec<String> = text("XDG_CONFIG_DIRS")
            .map(|dirs| dirs.split(':').filter(|d| d.starts_with('/')).map(|d| format!("{}/autostart", rel(std::path::Path::new(d)).trim_end_matches('/'))).collect())
            .unwrap_or_default();
        if system_dirs.is_empty() {
            system_dirs.push("etc/xdg/autostart".to_string());
        }
        XdgAutostart {
            root,
            user_dir: xdg_config_home(env).map(|c| format!("{}/autostart", rel(&c))),
            system_dirs,
            desktops: text("XDG_CURRENT_DESKTOP").map(|d| d.split(':').map(str::to_string).collect()).unwrap_or_default(),
        }
    }

    /// The running system's autostart.
    pub fn system() -> Self {
        Self::new(FsRoot::system(), &|name| std::env::var_os(name))
    }

    fn user_file(&self, key: &str) -> Option<PathBuf> {
        self.user_dir.as_ref().map(|d| self.root.path(&format!("{d}/{key}")))
    }

    /// The system file named `key` the desktop would use (the first folder that has it).
    fn system_file(&self, key: &str) -> Option<PathBuf> {
        self.system_dirs.iter().map(|d| self.root.path(&format!("{d}/{key}"))).find(|p| p.is_file())
    }

    /// Whether the desktop running would start an entry with these filters.
    fn shown_here(&self, d: &Desktop) -> bool {
        let named = |list: &[String]| list.iter().any(|w| self.desktops.contains(w));
        if let Some(only) = &d.only_show_in {
            if !named(only) {
                return false;
            }
        }
        !d.not_show_in.as_deref().is_some_and(named)
    }

    fn row(&self, key: &str, text: &str, from_user: bool, path: &std::path::Path, enabled: bool) -> StartupEntry {
        let d = Desktop::parse(text);
        StartupEntry {
            name: d.name.clone().unwrap_or_else(|| key.trim_end_matches(".desktop").to_string()),
            command: d.exec.unwrap_or_default(),
            location: if from_user { USER } else { SYSTEM }.to_string(),
            enabled,
            key: key.to_string(),
            file: path.display().to_string(),
            ..Default::default()
        }
    }

    /// The files named like `key`, the user's first, then each system folder's.
    fn candidates(&self) -> BTreeMap<String, Vec<(bool, PathBuf)>> {
        let mut found: BTreeMap<String, Vec<(bool, PathBuf)>> = BTreeMap::new();
        let folders = self.user_dir.iter().map(|d| (true, d)).chain(self.system_dirs.iter().map(|d| (false, d)));
        for (from_user, dir) in folders {
            let Ok(read) = std::fs::read_dir(self.root.path(dir)) else { continue };
            let mut names: Vec<_> = read.flatten().filter(|e| e.path().is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            names.sort();
            for name in names.into_iter().filter(|n| n.ends_with(".desktop")) {
                found.entry(name.clone()).or_default().push((from_user, self.root.path(&format!("{dir}/{name}"))));
            }
        }
        found
    }
}

impl StartupSource for XdgAutostart {
    fn id(&self) -> &'static str {
        ID
    }

    fn list(&self) -> Result<Vec<StartupEntry>, String> {
        let mut rows = Vec::new();
        for (key, files) in self.candidates() {
            let Some((from_user, path)) = files.first() else { continue };
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let winner = Desktop::parse(&text);
            if !winner.hidden {
                if self.shown_here(&winner) {
                    rows.push(self.row(&key, &text, *from_user, path, winner.gnome_enabled));
                }
                continue;
            }
            // Hidden: the entry is gone, unless the hiding file is the user's
            // override of a system entry, which then shows as turned off.
            if !*from_user {
                continue;
            }
            let Some(system) = files.iter().skip(1).find(|(user, _)| !user) else { continue };
            let Ok(under) = std::fs::read_to_string(&system.1) else { continue };
            let d = Desktop::parse(&under);
            if !d.hidden && self.shown_here(&d) {
                rows.push(self.row(&key, &under, false, &system.1, false));
            }
        }
        rows.sort_by_key(|r| r.name.to_lowercase());
        Ok(rows)
    }

    fn set_enabled(&self, entry: &StartupEntry, enable: bool) -> Result<(), String> {
        let key = entry.key.as_str();
        // A name from a listing is a file name; refuse anything that is a path.
        if key.is_empty() || key.contains('/') || key == "." || key == ".." {
            return Err("That startup app is no longer in the list".to_string());
        }
        let user = self.user_file(key).ok_or("The user's autostart folder is not known.")?;
        let system = self.system_file(key);
        let user_text = std::fs::read_to_string(&user).ok();
        let denied = |e: std::io::Error| format!("Could not change {key}: {e}");
        match (user_text, system) {
            (Some(text), system) => {
                let hides = Desktop::parse(&text).hidden;
                if enable {
                    match system {
                        // The user's file only hides the system's entry: drop it.
                        Some(sys) if hides => {
                            std::fs::remove_file(&user).map_err(denied)?;
                            let under = std::fs::read_to_string(&sys).map_err(denied)?;
                            if !Desktop::parse(&under).gnome_enabled {
                                write_user(&user, &set_key(&under, GNOME_ENABLED, Some("true"))).map_err(denied)?;
                            }
                            Ok(())
                        }
                        _ => write_user(&user, &set_key(&set_key(&text, HIDDEN, None), GNOME_ENABLED, None)).map_err(denied),
                    }
                } else {
                    // Hidden=true would make the user's own entry vanish from the
                    // list; it is turned off in place instead.
                    write_user(&user, &set_key(&text, GNOME_ENABLED, Some("false"))).map_err(denied)
                }
            }
            (None, Some(sys)) => {
                let text = std::fs::read_to_string(&sys).map_err(denied)?;
                let copy = if enable { set_key(&text, GNOME_ENABLED, Some("true")) } else { set_key(&text, HIDDEN, Some("true")) };
                write_user(&user, &copy).map_err(denied)
            }
            (None, None) => Err("That startup app is no longer in the list".to_string()),
        }
    }
}

/// Writes a per-user file, making the folder when it is not there yet.
fn write_user(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, text)
}

/// The keys of a desktop entry the autostart rules read.
#[derive(Debug, Default)]
struct Desktop {
    name: Option<String>,
    exec: Option<String>,
    hidden: bool,
    /// `X-GNOME-Autostart-enabled`: on unless it says `false`.
    gnome_enabled: bool,
    only_show_in: Option<Vec<String>>,
    not_show_in: Option<Vec<String>>,
}

impl Desktop {
    fn parse(text: &str) -> Desktop {
        let mut d = Desktop { gnome_enabled: true, ..Default::default() };
        let mut in_group = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_group = line == GROUP;
                continue;
            }
            let Some((k, v)) = line.split_once('=').filter(|_| in_group) else { continue };
            let (k, v) = (k.trim(), v.trim());
            let list = || v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>();
            match k {
                "Name" => d.name = Some(v.to_string()),
                "Exec" => d.exec = Some(v.to_string()),
                HIDDEN => d.hidden = v == "true",
                GNOME_ENABLED => d.gnome_enabled = v != "false",
                "OnlyShowIn" => d.only_show_in = Some(list()),
                "NotShowIn" => d.not_show_in = Some(list()),
                _ => {}
            }
        }
        d
    }
}

/// `text` with `key` in the `[Desktop Entry]` group set to `value`, or
/// removed when `value` is `None`; everything else stays as it was.
fn set_key(text: &str, key: &str, value: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut in_group = false;
    let mut placed = false;
    // Where the group ends: after its last line that is not blank.
    let mut group_end = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if in_group && !placed {
                if let Some(v) = value {
                    out.insert(group_end.unwrap_or(out.len()), format!("{key}={v}"));
                }
                placed = true;
            }
            in_group = t == GROUP;
            out.push(line.to_string());
            if in_group {
                group_end = Some(out.len());
            }
            continue;
        }
        if in_group {
            if t.split_once('=').is_some_and(|(k, _)| k.trim() == key) {
                if let Some(v) = value.filter(|_| !placed) {
                    out.push(format!("{key}={v}"));
                    group_end = Some(out.len());
                    placed = true;
                }
                continue;
            }
            out.push(line.to_string());
            if !t.is_empty() {
                group_end = Some(out.len());
            }
            continue;
        }
        out.push(line.to_string());
    }
    if in_group && !placed {
        if let Some(v) = value {
            out.insert(group_end.unwrap_or(out.len()), format!("{key}={v}"));
        }
    }
    let mut joined = out.join("\n");
    joined.push('\n');
    joined
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn env_for(desktop: &'static str) -> impl Fn(&str) -> Option<std::ffi::OsString> {
        move |name| match name {
            "HOME" => Some("/home/user".into()),
            "XDG_CURRENT_DESKTOP" => Some(desktop.into()),
            _ => None,
        }
    }

    fn source(desktop: &'static str) -> XdgAutostart {
        XdgAutostart::new(fixture("autostart"), &env_for(desktop))
    }

    /// A copy of the fixture tree that a test may change.
    struct Copy(PathBuf);

    impl Copy {
        fn new(label: &str) -> Copy {
            let dir = std::env::temp_dir().join(format!("upcm-autostart-{label}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            copy_tree(&fixture("autostart").path(""), &dir);
            Copy(dir)
        }
        fn source(&self) -> XdgAutostart {
            XdgAutostart::new(FsRoot::at(&self.0), &env_for("GNOME"))
        }
        fn user(&self, name: &str) -> PathBuf {
            self.0.join("home/user/.config/autostart").join(name)
        }
        fn system(&self, name: &str) -> PathBuf {
            self.0.join("etc/xdg/autostart").join(name)
        }
    }

    impl Drop for Copy {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(e.file_name());
            if e.path().is_dir() {
                copy_tree(&e.path(), &target);
            } else {
                std::fs::copy(e.path(), target).unwrap();
            }
        }
    }

    fn row(rows: &[StartupEntry], name: &str) -> StartupEntry {
        rows.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}")).clone()
    }

    #[test]
    fn user_and_system_entries_are_listed_each_with_its_source() {
        let rows = source("GNOME").list().unwrap();
        let seen: Vec<_> = rows.iter().map(|r| (r.name.as_str(), r.location.as_str(), r.enabled)).collect();
        assert_eq!(
            seen,
            [
                ("Cloud Sync", SYSTEM, true),
                ("Gnome Only", SYSTEM, true),
                ("My Notes", USER, true),
                ("Off By Default", SYSTEM, false),
                ("Tray Helper", SYSTEM, false),
            ]
        );
        let notes = row(&rows, "My Notes");
        assert_eq!(notes.command, "/home/user/bin/notes --background");
        assert_eq!(notes.key, "notes.desktop");
        assert!(notes.file.ends_with("home/user/.config/autostart/notes.desktop"), "{}", notes.file);
    }

    #[test]
    fn hidden_entries_are_gone_and_the_desktop_filters_apply() {
        let gnome: Vec<_> = source("GNOME").list().unwrap().into_iter().map(|r| r.name).collect();
        assert!(!gnome.contains(&"Removed".to_string()), "Hidden=true is gone");
        assert!(!gnome.contains(&"Kde Only".to_string()), "OnlyShowIn=KDE");
        assert!(!gnome.contains(&"Not On Gnome".to_string()), "NotShowIn=GNOME");
        let kde: Vec<_> = source("KDE").list().unwrap().into_iter().map(|r| r.name).collect();
        assert!(kde.contains(&"Kde Only".to_string()) && kde.contains(&"Not On Gnome".to_string()));
        assert!(!kde.contains(&"Gnome Only".to_string()));
        let stacked: Vec<_> = source("ubuntu:GNOME").list().unwrap().into_iter().map(|r| r.name).collect();
        assert!(stacked.contains(&"Gnome Only".to_string()), "a colon list names several desktops");
    }

    #[test]
    fn a_user_file_replaces_the_system_file_with_the_same_name() {
        let rows = source("GNOME").list().unwrap();
        assert_eq!(rows.iter().filter(|r| r.key == "tray.desktop").count(), 1);
        // The user's tray.desktop only hides; the entry shows as turned off.
        assert!(!row(&rows, "Tray Helper").enabled);
    }

    #[test]
    fn config_dirs_and_config_home_are_read_from_the_environment() {
        let env = |name: &str| match name {
            "HOME" => Some("/nowhere".into()),
            "XDG_CONFIG_HOME" => Some("/home/user/.config".into()),
            "XDG_CONFIG_DIRS" => Some("/missing:/etc/xdg:relative".into()),
            "XDG_CURRENT_DESKTOP" => Some("GNOME".into()),
            _ => None,
        };
        let rows = XdgAutostart::new(fixture("autostart"), &env).list().unwrap();
        assert!(rows.iter().any(|r| r.name == "Cloud Sync") && rows.iter().any(|r| r.name == "My Notes"));
    }

    #[test]
    fn disabling_a_system_entry_writes_a_hidden_copy_for_the_user_and_leaves_the_system_file() {
        let tree = Copy::new("disable-system");
        let before = std::fs::read_to_string(tree.system("sync.desktop")).unwrap();
        let sync = row(&tree.source().list().unwrap(), "Cloud Sync");
        tree.source().set_enabled(&sync, false).unwrap();
        let copy = std::fs::read_to_string(tree.user("sync.desktop")).unwrap();
        assert!(copy.contains("Hidden=true") && copy.contains("Exec=cloud-sync --quiet"), "{copy}");
        assert_eq!(std::fs::read_to_string(tree.system("sync.desktop")).unwrap(), before, "system files are never edited");
        assert!(!row(&tree.source().list().unwrap(), "Cloud Sync").enabled);
    }

    #[test]
    fn enabling_takes_the_override_away() {
        let tree = Copy::new("enable-override");
        let tray = row(&tree.source().list().unwrap(), "Tray Helper");
        assert!(!tray.enabled);
        tree.source().set_enabled(&tray, true).unwrap();
        assert!(!tree.user("tray.desktop").exists());
        assert!(row(&tree.source().list().unwrap(), "Tray Helper").enabled);
    }

    #[test]
    fn a_system_entry_that_is_off_by_itself_is_enabled_with_a_user_copy() {
        let tree = Copy::new("enable-off");
        let off = row(&tree.source().list().unwrap(), "Off By Default");
        tree.source().set_enabled(&off, true).unwrap();
        assert!(std::fs::read_to_string(tree.user("off.desktop")).unwrap().contains("X-GNOME-Autostart-enabled=true"));
        assert!(std::fs::read_to_string(tree.system("off.desktop")).unwrap().contains("X-GNOME-Autostart-enabled=false"));
        assert!(row(&tree.source().list().unwrap(), "Off By Default").enabled);
    }

    #[test]
    fn a_users_own_entry_is_turned_off_and_on_in_place_and_stays_listed() {
        let tree = Copy::new("user-entry");
        let notes = row(&tree.source().list().unwrap(), "My Notes");
        tree.source().set_enabled(&notes, false).unwrap();
        let off = row(&tree.source().list().unwrap(), "My Notes");
        assert!(!off.enabled, "still listed, turned off");
        tree.source().set_enabled(&off, true).unwrap();
        let text = std::fs::read_to_string(tree.user("notes.desktop")).unwrap();
        assert!(!text.contains("Hidden") && !text.contains("X-GNOME"), "{text}");
        assert!(row(&tree.source().list().unwrap(), "My Notes").enabled);
    }

    #[test]
    fn a_name_that_is_not_in_the_list_or_is_a_path_changes_nothing() {
        let tree = Copy::new("unknown");
        let gone = StartupEntry { key: "nothing.desktop".into(), ..Default::default() };
        assert!(tree.source().set_enabled(&gone, false).is_err());
        let path = StartupEntry { key: "../x.desktop".into(), ..Default::default() };
        assert!(tree.source().set_enabled(&path, false).is_err());
        assert!(!tree.user("nothing.desktop").exists());
    }

    #[test]
    fn setting_a_key_keeps_the_rest_of_the_file() {
        let text = "[Desktop Entry]\nName=A\n\n[Desktop Action x]\nName=B\n";
        assert_eq!(set_key(text, "Hidden", Some("true")), "[Desktop Entry]\nName=A\nHidden=true\n\n[Desktop Action x]\nName=B\n");
        assert_eq!(set_key("[Desktop Entry]\nHidden=false\nName=A\n", "Hidden", Some("true")), "[Desktop Entry]\nHidden=true\nName=A\n");
        assert_eq!(set_key("[Desktop Entry]\nHidden=true\nName=A\n", "Hidden", None), "[Desktop Entry]\nName=A\n");
    }
}
