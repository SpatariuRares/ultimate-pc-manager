//! The actions on a chosen item (pure: no UI, nothing launched but the
//! trash command): "Show in File Explorer" ("Open location" on Linux),
//! "Open the folder", "Copy path" and, on Linux, "Move to trash". Each one is turned
//! into an [`Outcome`] the add-on carries out: a move inside Disk Space,
//! a shared action the app runs, or a message.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::nav::Key;
use crate::platform::{AddonContext, Owner, Ran};
use crate::system::System;

/// Said when the item is no longer where the scan found it.
pub const MOVED: &str = "This item may have moved or been deleted since the scan.";
/// Said for a piece that is no single item ("N other items").
pub const UNAVAILABLE: &str = "Item unavailable";

/// One action on an item, in the order the panel and menus list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verb {
    /// Explorer opens the item's folder with the item selected.
    ShowInExplorer,
    /// Disk Space enters the folder (a file: the folder holding it), #80.
    OpenFolder,
    /// The item's full path goes to the clipboard.
    CopyPath,
    /// The item goes to the desktop's trash (Linux only).
    MoveToTrash,
}

impl Verb {
    /// Every action, in the order they are offered.
    pub const ALL: [Verb; 4] = [Verb::ShowInExplorer, Verb::OpenFolder, Verb::CopyPath, Verb::MoveToTrash];

    /// The actions this system offers, in order: the trash is Linux's.
    pub fn offered_on(system: System) -> Vec<Verb> {
        Verb::ALL.into_iter().filter(|v| *v != Verb::MoveToTrash || system == System::Linux).collect()
    }

    /// Its words on the button and in the context menu on this system.
    pub fn label(self) -> &'static str {
        self.label_on(System::current())
    }

    /// Its words on `system`.
    pub fn label_on(self, system: System) -> &'static str {
        match (self, system) {
            (Verb::ShowInExplorer, System::Windows) => "Show in File Explorer",
            (Verb::ShowInExplorer, _) => "Open location",
            (Verb::OpenFolder, _) => "Open the folder",
            (Verb::CopyPath, _) => "Copy path",
            (Verb::MoveToTrash, _) => "Move to trash",
        }
    }
}

/// What carrying out a verb comes to.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Enter this folder (or the folder holding this file) in the canvas.
    Open(PathBuf),
    /// Explorer, with the item selected.
    Reveal(PathBuf),
    /// The clipboard.
    Copy(String),
    /// The trash.
    Trash(PathBuf),
    /// Nothing done; this is why.
    Say(&'static str),
}

/// The item a piece stands for: a folder or file by its path, "this
/// folder" as the level's own folder; None for "N other items".
pub fn item(key: &Key, level: &Path) -> Option<PathBuf> {
    match key {
        Key::Folder(p) | Key::File(p) => Some(p.clone()),
        Key::ThisFolder => Some(level.to_path_buf()),
        Key::Rest => None,
    }
}

/// What `verb` on `item` comes to. Explorer and the clipboard first check
/// with `exists` that the item is still there; "Open the folder" moves
/// inside the scan's results, which keep it either way.
pub fn outcome(verb: Verb, item: Option<&Path>, exists: impl Fn(&Path) -> bool) -> Outcome {
    let Some(path) = item else {
        return Outcome::Say(UNAVAILABLE);
    };
    if verb != Verb::OpenFolder && !exists(path) {
        return Outcome::Say(MOVED);
    }
    match verb {
        Verb::ShowInExplorer => Outcome::Reveal(path.to_path_buf()),
        Verb::OpenFolder => Outcome::Open(path.to_path_buf()),
        Verb::CopyPath => Outcome::Copy(path.display().to_string()),
        Verb::MoveToTrash => Outcome::Trash(path.to_path_buf()),
    }
}

/// Said when the trash tool is not there.
pub const NO_TRASH_TOOL: &str = "Couldn't move the item to the trash because the gio tool is not installed.";

/// The program and arguments that move `path` to the trash. `gio trash`
/// follows the FreeDesktop trash rules (the trash of the item's own drive,
/// with the information needed to restore it), which a copy of them here
/// would only get partly right.
pub fn trash_command(path: &Path) -> (&'static str, Vec<String>) {
    ("gio", vec!["trash".into(), "--".into(), path.display().to_string()])
}

/// Moves `path` to the trash; what to say afterwards, or why it failed.
pub fn move_to_trash(context: &AddonContext, path: &Path) -> Result<String, String> {
    let (program, args) = trash_command(path);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let output = std::env::temp_dir().join(format!("upm-trash-{}-{nanos}.log", std::process::id()));
    let mut said = Vec::new();
    let ran = context.run(program, &args, &output, &std::sync::atomic::AtomicBool::new(false), &mut |l| said.push(l.to_string()));
    let _ = std::fs::remove_file(&output);
    match ran {
        Ran::Exited(0) => Ok(format!("Moved to the trash: {}", path.display())),
        Ran::Exited(_) => {
            let why = said.into_iter().find(|l| !l.trim().is_empty()).unwrap_or_default();
            Err(format!("Couldn't move the item to the trash. {why}").trim_end().to_string())
        }
        Ran::NotStarted(_) | Ran::Stopped => Err(NO_TRASH_TOOL.to_string()),
    }
}

/// How long a link to another add-on's row is trusted before asking again.
pub const LINK_FOR: Duration = Duration::from_secs(2);

/// The last answer to "who owns this item?" (asking Installed Apps looks
/// at every app's folder, too slow for every frame).
#[derive(Default)]
pub struct Links {
    last: Option<(PathBuf, Instant, Option<Owner>)>,
}

impl Links {
    /// The owner of `path`, from `ask` when the item changed or the
    /// answer is older than [`LINK_FOR`].
    pub fn get(&mut self, path: &Path, now: Instant, ask: impl FnOnce(&Path) -> Option<Owner>) -> Option<Owner> {
        match &self.last {
            Some((p, at, owner)) if p == path && now.duration_since(*at) < LINK_FOR => owner.clone(),
            _ => {
                let owner = ask(path);
                self.last = Some((path.to_path_buf(), now, owner.clone()));
                owner
            }
        }
    }
}

/// Whether the item is still on disk (a broken link counts: it is there),
/// however long its path.
pub fn on_disk(path: &Path) -> bool {
    std::fs::symlink_metadata(crate::long_path::long_path(path)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn there(_: &Path) -> bool {
        true
    }

    fn gone(_: &Path) -> bool {
        false
    }

    #[test]
    fn a_piece_names_its_item_and_this_folder_is_the_level() {
        let level = Path::new(r"C:\Users");
        assert_eq!(item(&Key::Folder(PathBuf::from(r"C:\Users\a")), level), Some(PathBuf::from(r"C:\Users\a")));
        assert_eq!(item(&Key::File(PathBuf::from(r"C:\Users\a.txt")), level), Some(PathBuf::from(r"C:\Users\a.txt")));
        assert_eq!(item(&Key::ThisFolder, level), Some(PathBuf::from(r"C:\Users")));
        assert_eq!(item(&Key::Rest, level), None);
    }

    #[test]
    fn each_verb_on_an_item_that_is_there() {
        let p = Path::new(r"C:\Users\a\movie.mkv");
        assert_eq!(outcome(Verb::ShowInExplorer, Some(p), there), Outcome::Reveal(p.to_path_buf()));
        assert_eq!(outcome(Verb::OpenFolder, Some(p), there), Outcome::Open(p.to_path_buf()));
        assert_eq!(outcome(Verb::CopyPath, Some(p), there), Outcome::Copy(r"C:\Users\a\movie.mkv".into()));
    }

    #[test]
    fn an_item_gone_since_the_scan_is_not_shown_nor_copied_but_its_folder_still_opens() {
        let p = Path::new(r"C:\gone.bin");
        assert_eq!(outcome(Verb::ShowInExplorer, Some(p), gone), Outcome::Say("This item may have moved or been deleted since the scan."));
        assert_eq!(outcome(Verb::CopyPath, Some(p), gone), Outcome::Say("This item may have moved or been deleted since the scan."));
        assert_eq!(outcome(Verb::OpenFolder, Some(p), gone), Outcome::Open(p.to_path_buf()));
    }

    #[test]
    fn the_rest_is_no_item() {
        for verb in Verb::ALL {
            assert_eq!(outcome(verb, None, there), Outcome::Say("Item unavailable"));
        }
    }

    #[test]
    fn the_owner_is_asked_again_only_for_another_item_or_after_a_while() {
        let tool = Owner { addon: "installed-apps", page: "Installed Apps", label: "Tool".into() };
        let (mut links, t0, asked) = (Links::default(), Instant::now(), std::cell::Cell::new(0));
        let ask = |_: &Path| {
            asked.set(asked.get() + 1);
            Some(tool.clone())
        };
        let a = Path::new(r"C:\Tool\a.exe");
        assert_eq!(links.get(a, t0, ask), Some(tool.clone()));
        assert_eq!(links.get(a, t0 + Duration::from_millis(500), ask), Some(tool.clone()));
        assert_eq!(asked.get(), 1, "the same item soon after: the last answer");
        links.get(Path::new(r"C:\b.txt"), t0 + Duration::from_millis(600), ask);
        links.get(Path::new(r"C:\b.txt"), t0 + Duration::from_secs(3), ask);
        assert_eq!(asked.get(), 3, "another item, then the same one later");
    }

    #[test]
    fn the_labels_name_each_action() {
        let words = |system| Verb::offered_on(system).into_iter().map(|v| v.label_on(system)).collect::<Vec<_>>();
        assert_eq!(words(System::Windows), ["Show in File Explorer", "Open the folder", "Copy path"], "Windows is as it was");
        assert_eq!(words(System::Linux), ["Open location", "Open the folder", "Copy path", "Move to trash"]);
        assert_eq!(words(System::MacOs), ["Open location", "Open the folder", "Copy path"]);
    }

    #[test]
    fn trashing_an_item_that_is_there_or_gone() {
        let p = Path::new("/home/a/movie.mkv");
        assert_eq!(outcome(Verb::MoveToTrash, Some(p), there), Outcome::Trash(p.to_path_buf()));
        assert_eq!(outcome(Verb::MoveToTrash, Some(p), gone), Outcome::Say(MOVED));
    }

    #[test]
    fn the_trash_command_keeps_a_dashed_name_as_a_name() {
        let (program, args) = trash_command(Path::new("/home/a/-rf"));
        assert_eq!((program, args), ("gio", vec!["trash".to_string(), "--".into(), "/home/a/-rf".into()]));
    }

    #[test]
    fn the_trash_runs_gio_and_says_how_it_went() {
        use crate::platform::{Launch, When};
        let path = Path::new("/home/a/movie.mkv");
        let run = |code: i32, text: &str| {
            let (ctx, runner) = AddonContext::recording();
            runner.prepare(When::Nth(0), code, text);
            let result = move_to_trash(&ctx, path);
            (result, runner.launches())
        };
        let (done, launches) = run(0, "");
        assert_eq!(done, Ok("Moved to the trash: /home/a/movie.mkv".to_string()));
        assert!(matches!(&launches[..], [Launch::Run { program, args, .. }] if program == "gio" && args == &["trash", "--", "/home/a/movie.mkv"]), "{launches:?}");
        let (failed, _) = run(1, "gio: /home/a/movie.mkv: Unable to trash file: Permission denied");
        assert_eq!(failed, Err("Couldn't move the item to the trash. gio: /home/a/movie.mkv: Unable to trash file: Permission denied".to_string()));
        let (ctx, _) = AddonContext::recording();
        assert_eq!(move_to_trash(&ctx, path), Err(NO_TRASH_TOOL.to_string()), "the tool could not be started");
    }
}
