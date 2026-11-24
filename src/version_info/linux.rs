//! Linux: a program has no version details, but the desktop entry that
//! launches it (`.desktop` file) describes it: that text is its description.
//!
//! The entry whose `Exec` (or `TryExec`) names the program is the one: by the
//! full path when it gives one, else by the program's file name. A full-path
//! match wins over a name match.

use crate::fs_root::FsRoot;

/// The description of the program at `path` from the desktop entries of
/// this system.
pub fn description(path: &str) -> Option<String> {
    let dirs = application_dirs(&|name| std::env::var(name).ok());
    description_in(&FsRoot::system(), &dirs, path)
}

/// The folders (relative to the root) the desktop entries are read from: the
/// user's data folder, the system's data folders and Flatpak's exports.
pub fn application_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let set = |name: &str| env(name).filter(|v| !v.is_empty());
    let home = set("XDG_DATA_HOME").or_else(|| set("HOME").map(|h| format!("{}/.local/share", h.trim_end_matches('/'))));
    let system = set("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    let flatpak = ["/var/lib/flatpak/exports/share".to_string()];
    let mut dirs: Vec<String> = Vec::new();
    for base in home.into_iter().chain(system.split(':').map(String::from)).chain(flatpak) {
        let dir = format!("{}/applications", base.trim_end_matches('/'));
        if !base.is_empty() && !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

/// [`description`] over the folders `dirs` of `root`.
pub fn description_in(root: &FsRoot, dirs: &[String], path: &str) -> Option<String> {
    let file_name = path.rsplit('/').next().filter(|n| !n.is_empty())?;
    let mut by_name = None;
    for dir in dirs {
        let dir = dir.trim_start_matches('/');
        for entry in root.list_dir(dir).into_iter().filter(|n| n.ends_with(".desktop")) {
            let Some(text) = root.read(&format!("{dir}/{entry}")) else { continue };
            let entry = DesktopEntry::parse(&text);
            let Some(text) = entry.description() else { continue };
            let programs = entry.programs();
            if programs.iter().any(|p| p == path) {
                return Some(text);
            }
            if by_name.is_none() && programs.iter().any(|p| !p.contains('/') && p == file_name) {
                by_name = Some(text);
            }
        }
    }
    by_name
}

/// The keys of a desktop entry's `[Desktop Entry]` group this reader uses.
#[derive(Default)]
struct DesktopEntry {
    exec: String,
    try_exec: String,
    comment: String,
    generic_name: String,
    name: String,
}

impl DesktopEntry {
    fn parse(text: &str) -> Self {
        let mut entry = DesktopEntry::default();
        let mut inside = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                inside = line == "[Desktop Entry]";
            } else if inside {
                let Some((key, value)) = line.split_once('=') else { continue };
                let field = match key.trim() {
                    "Exec" => &mut entry.exec,
                    "TryExec" => &mut entry.try_exec,
                    "Comment" => &mut entry.comment,
                    "GenericName" => &mut entry.generic_name,
                    "Name" => &mut entry.name,
                    _ => continue,
                };
                *field = value.trim().to_string();
            }
        }
        entry
    }

    /// What the entry says the program is: its comment, else its generic
    /// name, else its name.
    fn description(&self) -> Option<String> {
        [&self.comment, &self.generic_name, &self.name].into_iter().find(|t| !t.is_empty()).cloned()
    }

    /// The programs the entry launches, leaving out the generic launchers:
    /// an entry that starts `flatpak` or `python3` describes what it was
    /// given to run, not every process of that program.
    fn programs(&self) -> Vec<String> {
        [exec_program(&self.exec), exec_program(&self.try_exec)].into_iter().flatten().filter(|p| !is_generic_launcher(p)).collect()
    }
}

/// The program an `Exec` line starts: its first word (quotes honoured),
/// after an `env` prefix and its `NAME=value` words.
fn exec_program(exec: &str) -> Option<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let (mut quoted, mut started) = (false, false);
    for c in exec.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    words.push(std::mem::take(&mut word));
                    started = false;
                }
            }
            c => {
                word.push(c);
                started = true;
            }
        }
    }
    if started {
        words.push(word);
    }
    let mut words = words.into_iter().peekable();
    if words.peek().is_some_and(|w| w == "env") {
        words.next();
    }
    words.find(|w| !w.is_empty() && !is_assignment(w))
}

/// Programs that start other programs (shells, `env`, interpreters, package
/// launchers): a desktop entry naming one says nothing about the program.
fn is_generic_launcher(program: &str) -> bool {
    const LAUNCHERS: [&str; 24] = [
        "env", "sh", "bash", "dash", "zsh", "fish", "flatpak", "snap", "python", "python2", "python3", "perl", "ruby", "node", "nodejs", "java", "wine", "mono", "dotnet", "sudo", "pkexec", "xdg-open",
        "gio", "systemd-run",
    ];
    let name = program.rsplit('/').next().unwrap_or(program);
    // `python3.12`, `python3.12.1`: the interpreter with its version.
    let family = name.strip_prefix("python").is_some_and(|v| v.chars().all(|c| c.is_ascii_digit() || c == '.'));
    LAUNCHERS.contains(&name) || family
}

/// `NAME=value`, the words `env` takes before the program.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    fn describe(path: &str) -> Option<String> {
        description_in(&fixture("apps"), &["usr/share/applications".to_string(), "/usr/local/share/applications".to_string()], path)
    }

    #[test]
    fn a_program_with_a_desktop_entry_takes_its_comment_as_description() {
        assert_eq!(describe("/usr/lib/firefox/firefox").as_deref(), Some("Browse the World Wide Web"), "Exec with the full path");
    }

    #[test]
    fn a_program_named_without_a_path_in_the_entry_matches_by_its_file_name() {
        assert_eq!(describe("/usr/bin/gedit").as_deref(), Some("Text Editor"), "no comment: the generic name; Exec=gedit %U");
        assert_eq!(describe("/usr/bin/vlc").as_deref(), Some("VLC media player"), "no comment and no generic name: the name; quoted Exec");
    }

    #[test]
    fn an_env_prefix_and_other_groups_do_not_hide_the_program() {
        assert_eq!(describe("/usr/bin/gimp").as_deref(), Some("Create images and edit photographs"), "Exec=env GDK_BACKEND=x11 gimp");
        assert_eq!(describe("/usr/bin/other"), None, "an entry's Actions group is not the entry");
    }

    #[test]
    fn a_full_path_match_wins_over_a_file_name_match() {
        assert_eq!(describe("/opt/tool/tool").as_deref(), Some("The tool in /opt"));
        assert_eq!(describe("/usr/bin/tool").as_deref(), Some("A tool by name"));
    }

    #[test]
    fn an_entry_that_starts_a_generic_launcher_gives_its_description_to_no_program_of_that_name() {
        for program in ["/usr/bin/flatpak", "/usr/bin/python3.12", "/usr/bin/sh", "/usr/bin/env"] {
            assert_eq!(describe(program), None, "{program}");
        }
        assert_eq!(describe("/usr/bin/gedit").as_deref(), Some("Text Editor"), "an entry that names its own program still describes it");
    }

    #[test]
    fn a_program_without_an_entry_has_no_description() {
        assert_eq!(describe("/usr/bin/ssh"), None);
        assert_eq!(describe(""), None);
        assert_eq!(describe("/usr/bin/"), None);
    }

    #[test]
    fn the_entries_are_read_from_the_users_the_systems_and_flatpaks_folders() {
        let env = |name: &str| match name {
            "HOME" => Some("/home/me".to_string()),
            "XDG_DATA_DIRS" => Some("/usr/share:/opt/share/".to_string()),
            _ => None,
        };
        assert_eq!(
            application_dirs(&env),
            ["/home/me/.local/share/applications", "/usr/share/applications", "/opt/share/applications", "/var/lib/flatpak/exports/share/applications"]
        );
        assert_eq!(
            application_dirs(&|_| None),
            ["/usr/local/share/applications", "/usr/share/applications", "/var/lib/flatpak/exports/share/applications"],
            "the defaults of the desktop specification"
        );
    }
}
