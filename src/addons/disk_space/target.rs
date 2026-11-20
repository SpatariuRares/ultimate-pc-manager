//! What a scan measures: a drive, every local drive ("All disks") or a
//! folder; how it is saved and given on the command line, and what it
//! means against the drives connected now. Pure: no egui, no Windows.

use crate::sampler::DiskRow;

/// What the Start button (or `--scan`) measures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Every local fixed drive under one virtual root.
    AllDisks,
    /// A drive, by its mount ("D:\").
    Drive(String),
    /// A folder picked with Choose Folder… (or given with `--scan=`).
    Folder(String),
}

/// What `--scan` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanRequest {
    /// `--scan` (or `--scan=`): the target chosen last time.
    Saved,
    /// `--scan=<target>`.
    Target(Target),
}

/// The `--scan` request among the app's arguments, if any.
pub fn scan_request(args: &[String]) -> Option<ScanRequest> {
    args.iter().skip(1).find_map(|a| {
        if a == "--scan" {
            return Some(ScanRequest::Saved);
        }
        let value = a.strip_prefix("--scan=")?;
        Some(Target::parse(value).map_or(ScanRequest::Saved, ScanRequest::Target))
    })
}

/// The saved value of [`Target::AllDisks`].
pub const ALL: &str = "all";

impl Target {
    /// Reads a saved or typed target; None when empty. "all" (or "All
    /// disks") is every local drive, "D:" or "d:\" the drive, anything
    /// else a folder.
    pub fn parse(text: &str) -> Option<Target> {
        let text = text.trim().trim_matches('"').trim();
        if text.is_empty() {
            return None;
        }
        if text.eq_ignore_ascii_case(ALL) || text.eq_ignore_ascii_case(ALL_DISKS) {
            return Some(Target::AllDisks);
        }
        let b = text.as_bytes();
        let drive = b[0].is_ascii_alphabetic() && b.get(1) == Some(&b':') && (b.len() == 2 || (b.len() == 3 && (b[2] == b'\\' || b[2] == b'/')));
        Some(if drive { Target::Drive(format!("{}:\\", (b[0] as char).to_ascii_uppercase())) } else { Target::Folder(text.to_string()) })
    }
}

/// The shown name of [`Target::AllDisks`], also the virtual root's name.
pub const ALL_DISKS: &str = "All disks";

/// A target as the drives connected now make it: where to walk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The folders walked (several for All disks).
    pub roots: Vec<String>,
    /// The space used on the drives walked (None for a folder): the
    /// scan's progress is estimated from it.
    pub expected: Option<u64>,
}

impl Target {
    /// The value saved in the settings.
    pub fn setting(&self) -> String {
        match self {
            Target::AllDisks => ALL.to_string(),
            Target::Drive(p) | Target::Folder(p) => p.clone(),
        }
    }

    /// Its line in the drop-down.
    pub fn label(&self, disks: &[DiskRow]) -> String {
        match self {
            Target::AllDisks => {
                let n = local_drives(disks).count();
                format!("{ALL_DISKS}   {n} local drive{}", if n == 1 { "" } else { "s" })
            }
            Target::Drive(mount) => match find(disks, mount) {
                Some(d) => format!("{mount}   {} used, {} free", super::text::size(used(d)), super::text::size(d.free)),
                None => format!("{mount}   not connected"),
            },
            Target::Folder(path) => path.clone(),
        }
    }

    /// The name of what it measures ("All disks", "D:\", the folder).
    pub fn name(&self) -> &str {
        match self {
            Target::AllDisks => ALL_DISKS,
            Target::Drive(p) | Target::Folder(p) => p,
        }
    }

    /// The connected drives it lies on (the side panel's cards): its drive,
    /// the local drives of All disks, the drive holding a folder.
    pub fn drives<'a>(&self, disks: &'a [DiskRow]) -> Vec<&'a DiskRow> {
        match self {
            Target::AllDisks => local_drives(disks).collect(),
            Target::Drive(mount) => find(disks, mount).into_iter().collect(),
            Target::Folder(path) => {
                let lower = path.to_ascii_lowercase();
                disks.iter().filter(|d| !d.mount.is_empty() && lower.starts_with(&d.mount.to_ascii_lowercase())).take(1).collect()
            }
        }
    }

    /// What measuring it means with `disks` connected; why it cannot be
    /// measured now otherwise (e.g. the drive was unplugged).
    pub fn plan(&self, disks: &[DiskRow]) -> Result<Plan, String> {
        match self {
            Target::AllDisks => {
                let drives: Vec<&DiskRow> = local_drives(disks).collect();
                if drives.is_empty() {
                    return Err("No local drives.".into());
                }
                Ok(Plan { roots: drives.iter().map(|d| d.mount.clone()).collect(), expected: Some(drives.iter().map(|d| used(d)).sum()) })
            }
            Target::Drive(mount) => match find(disks, mount) {
                Some(d) => Ok(Plan { roots: vec![d.mount.clone()], expected: Some(used(d)) }),
                None => Err(format!("{mount} is not connected.")),
            },
            Target::Folder(path) => Ok(Plan { roots: vec![path.clone()], expected: None }),
        }
    }
}

/// The drives All disks measures: the local fixed ones (network drives
/// are never listed; removable ones are measured one by one).
pub fn local_drives(disks: &[DiskRow]) -> impl Iterator<Item = &DiskRow> {
    disks.iter().filter(|d| !d.removable)
}

/// One target per connected drive, then All disks.
pub fn choices(disks: &[DiskRow]) -> Vec<Target> {
    disks.iter().map(|d| Target::Drive(d.mount.clone())).chain([Target::AllDisks]).collect()
}

fn find<'a>(disks: &'a [DiskRow], mount: &str) -> Option<&'a DiskRow> {
    disks.iter().find(|d| d.mount.eq_ignore_ascii_case(mount))
}

fn used(d: &DiskRow) -> u64 {
    d.total.saturating_sub(d.free)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        std::iter::once("app.exe").chain(list.iter().copied()).map(String::from).collect()
    }

    #[test]
    fn the_command_line_asks_for_a_scan_only_with_the_scan_flag() {
        assert_eq!(scan_request(&args(&[])), None);
        assert_eq!(scan_request(&args(&["--page", "disk-space"])), None);
        assert_eq!(scan_request(&args(&["--scanner"])), None, "only the exact flag");
        assert_eq!(scan_request(&args(&["--page", "diskspace", "--scan"])), Some(ScanRequest::Saved));
        assert_eq!(scan_request(&args(&["--scan="])), Some(ScanRequest::Saved), "an empty target is the saved one");
    }

    #[test]
    fn the_scan_flag_names_a_drive_every_local_drive_or_a_folder() {
        let target = |a: &str| match scan_request(&args(&[a])) {
            Some(ScanRequest::Target(t)) => t,
            other => panic!("{a}: {other:?}"),
        };
        assert_eq!(target("--scan=D:"), Target::Drive("D:\\".into()));
        assert_eq!(target("--scan=d:\\"), Target::Drive("D:\\".into()));
        assert_eq!(target("--scan=all"), Target::AllDisks);
        assert_eq!(target("--scan=All disks"), Target::AllDisks);
        assert_eq!(target("--scan=C:\\Users\\Me"), Target::Folder("C:\\Users\\Me".into()));
        assert_eq!(target("--scan=\"E:\\Games\""), Target::Folder("E:\\Games".into()), "quotes left by a shell are dropped");
    }

    fn disk(mount: &str, total: u64, free: u64, removable: bool) -> DiskRow {
        DiskRow { mount: mount.into(), total, free, removable, ..Default::default() }
    }

    fn this_pc() -> Vec<DiskRow> {
        vec![disk("C:\\", 1000, 400, false), disk("D:\\", 500, 100, false), disk("E:\\", 64, 60, true)]
    }

    #[test]
    fn all_disks_measures_the_local_fixed_drives_under_one_root() {
        let plan = Target::AllDisks.plan(&this_pc()).unwrap();
        assert_eq!(plan.roots, vec!["C:\\".to_string(), "D:\\".to_string()], "a removable drive is measured on its own only");
        assert_eq!(plan.expected, Some(1000), "used space of C: and D:");
        assert_eq!(Target::AllDisks.label(&this_pc()), "All disks   2 local drives");
        assert_eq!(Target::AllDisks.label(&this_pc()[..1]), "All disks   1 local drive");
        assert_eq!(Target::AllDisks.plan(&this_pc()[2..]).unwrap_err(), "No local drives.");
    }

    #[test]
    fn a_drive_is_measured_while_it_is_connected() {
        let plan = Target::Drive("E:\\".into()).plan(&this_pc()).unwrap();
        assert_eq!((plan.roots, plan.expected), (vec!["E:\\".to_string()], Some(4)));
        assert_eq!(Target::Drive("F:\\".into()).plan(&this_pc()).unwrap_err(), "F:\\ is not connected.", "unplugged since it was chosen");
        let plan = Target::Folder("C:\\Users".into()).plan(&[]).unwrap();
        assert_eq!((plan.roots, plan.expected), (vec!["C:\\Users".to_string()], None), "a folder's total is not known in advance");
    }

    #[test]
    fn the_side_panel_cards_are_the_drives_the_target_lies_on() {
        let mounts = |t: Target| t.drives(&this_pc()).iter().map(|d| d.mount.clone()).collect::<Vec<_>>();
        assert_eq!(mounts(Target::Drive("d:\\".into())), vec!["D:\\"]);
        assert_eq!(mounts(Target::AllDisks), vec!["C:\\", "D:\\"], "the local fixed drives All disks measures");
        assert_eq!(mounts(Target::Folder("e:\\Photos\\2024".into())), vec!["E:\\"], "the drive holding the folder");
        assert!(mounts(Target::Drive("F:\\".into())).is_empty(), "unplugged: no card");
        assert!(mounts(Target::Folder("\\\\server\\share".into())).is_empty(), "a folder on no listed drive");
    }

    #[test]
    fn the_target_is_saved_as_text_it_reads_back() {
        for t in [Target::AllDisks, Target::Drive("D:\\".into()), Target::Folder("E:\\Games".into())] {
            assert_eq!(Target::parse(&t.setting()), Some(t.clone()), "{t:?}");
        }
        assert_eq!(Target::AllDisks.setting(), "all");
    }
}
