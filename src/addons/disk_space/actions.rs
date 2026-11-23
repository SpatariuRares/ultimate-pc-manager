//! The actions on a chosen item (pure: no UI, nothing launched): "Show in
//! File Explorer", "Open the folder" and "Copy path". Each one is turned
//! into an [`Outcome`] the add-on carries out: a move inside Disk Space,
//! a shared action the app runs, or a message.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::nav::Key;
use crate::platform::Owner;

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
}

impl Verb {
    /// Every action, in the order they are offered.
    pub const ALL: [Verb; 3] = [Verb::ShowInExplorer, Verb::OpenFolder, Verb::CopyPath];

    /// Its words on the button and in the context menu.
    pub fn label(self) -> &'static str {
        match self {
            Verb::ShowInExplorer => "Show in File Explorer",
            Verb::OpenFolder => "Open the folder",
            Verb::CopyPath => "Copy path",
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
        let labels: Vec<_> = Verb::ALL.iter().map(|v| v.label()).collect();
        assert_eq!(labels, ["Show in File Explorer", "Open the folder", "Copy path"]);
    }
}
