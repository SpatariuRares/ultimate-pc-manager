//! The words Disk Space shows (pure): sizes, counts, durations, the
//! progress line and the status band's sentences.

use crate::sensors::model::thousands;

pub const KIB: u64 = 1024;
pub const MIB: u64 = 1024 * KIB;
pub const GIB: u64 = 1024 * MIB;

/// A size in binary units, as the drive cards count them: "512 B",
/// "4 KB", "5.5 MB", "12.3 GB"; from 100 MB/GB on whole numbers with
/// thousands ("1,517 GB": the design keeps large drives in GB).
pub fn size(bytes: u64) -> String {
    let b = bytes as f64;
    let scaled = |unit: u64, name: &str| {
        let v = b / unit as f64;
        if v >= 100.0 {
            format!("{} {name}", thousands(v.floor()))
        } else {
            format!("{v:.1} {name}")
        }
    };
    if bytes >= GIB {
        scaled(GIB, "GB")
    } else if bytes >= MIB {
        scaled(MIB, "MB")
    } else if bytes >= KIB {
        format!("{:.0} KB", b / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

/// "1 file" / "4,032,654 files".
pub fn files(n: u64) -> String {
    if n == 1 {
        "1 file".into()
    } else {
        format!("{} files", thousands(n as f64))
    }
}

/// "1 file type" / "7 file types".
pub fn file_types(n: u64) -> String {
    if n == 1 {
        "1 file type".into()
    } else {
        format!("{} file types", thousands(n as f64))
    }
}

/// How long a scan took: "45 s", "2 min 22 s", "3 h 5 min".
pub fn duration(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs % 3600 / 60, secs % 60);
    match (h, m, s) {
        (0, 0, s) => format!("{s} s"),
        (0, m, 0) => format!("{m} min"),
        (0, m, s) => format!("{m} min {s} s"),
        (h, m, _) => format!("{h} h {m} min"),
    }
}

/// The progress line: "64% · 2,581,000 files · 958 GB so far" (no
/// percentage for a folder, whose total is not known in advance).
pub fn progress(done: Option<f32>, files_so_far: u64, bytes: u64) -> String {
    let counted = format!("{} \u{00b7} {} so far", files(files_so_far), size(bytes));
    match done {
        Some(f) => format!("{:.0}% \u{00b7} {counted}", (f * 100.0).floor()),
        None => counted,
    }
}

/// "Finished today at 21:12 after 2 min 22 s: 1,517 GB in 4,032,654 files."
pub fn finished(day: &str, at: &str, secs: u64, bytes: u64, file_count: u64) -> String {
    format!("Finished {day} at {at} after {}: {} in {}.", duration(secs), size(bytes), files(file_count))
}

/// "today", "yesterday" or "N days ago" for a local day number.
pub fn day_word(day: i64, today: i64) -> String {
    match today - day {
        0 => "today".into(),
        1 => "yesterday".into(),
        n => format!("{n} days ago"),
    }
}

/// The status band's sentence: "DockerSandboxes holds 713 GB, almost half
/// of what the scan counted on C:\."
pub fn where_it_goes(name: &str, bytes: u64, total: u64, root: &str) -> String {
    let share = if total > 0 { bytes as f64 / total as f64 } else { 0.0 };
    let part = if share >= 0.95 {
        "nearly all of".to_string()
    } else if share >= 0.70 {
        "most of".to_string()
    } else if share >= 0.50 {
        "more than half of".to_string()
    } else if share >= 0.40 {
        "almost half of".to_string()
    } else {
        format!("{:.0}% of", share * 100.0)
    };
    format!("{name} holds {}, {part} what the scan counted on {root}.", size(bytes))
}

/// The note when folders could not be read.
pub fn unreadable(folders: u64) -> String {
    let n = if folders == 1 { "1 folder".to_string() } else { format!("{} folders", thousands(folders as f64)) };
    format!("{n} could not be read, so the total is a floor.")
}

/// The note when items were skipped but how many is not known (a saved
/// scan without an unreadable count).
pub fn unreadable_unknown() -> String {
    "Some items could not be read, so the total is a floor.".into()
}

/// The skipped-items note of the status band.
#[derive(Clone, Debug, PartialEq)]
pub struct Skipped {
    /// "Scan finished with skipped items", once the scan finished.
    pub heading: Option<String>,
    /// "37 items could not be measured", once the scan finished.
    pub count: Option<String>,
    pub sentence: String,
}

/// The note for `count` unreadable folders: while scanning (or after a
/// stop) the total is a floor; once finished, the scan is incomplete.
/// None when nothing was skipped.
pub fn skipped(count: u64, finished: bool) -> Option<Skipped> {
    if count == 0 {
        return None;
    }
    if !finished {
        return Some(Skipped { heading: None, count: None, sentence: unreadable(count) });
    }
    let items = if count == 1 { "1 item".to_string() } else { format!("{} items", thousands(count as f64)) };
    Some(Skipped {
        heading: Some("Scan finished with skipped items".into()),
        count: Some(format!("{items} could not be measured")),
        sentence: "The scan has finished. These items could not be counted, so the total is incomplete.".into(),
    })
}

/// The time a scan still needs: "about 2 min left".
pub fn left(secs: u64) -> String {
    format!("about {} left", duration(secs))
}

/// Under the skipped-items note: "For example C:\X." and, when `denied`
/// of them were denied access, that only the system can read it or them.
pub fn skipped_why(example: &str, denied: u64) -> String {
    let why = match denied {
        0 => "",
        1 => " It is readable only by the system.",
        _ => " They are readable only by the system.",
    };
    format!("For example {example}.{why}")
}

/// Under the examples of skipped items.
pub const EXAMPLES_NOTE: &str = "Up to eight examples are shown. Some system-protected or unavailable items may still be skipped.";

/// Once the scan stopped tracking hard links.
pub const HARD_LINK_LIMIT: &str = "Hard-link tracking limit reached. Scan a smaller folder.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_are_short_and_large_ones_get_thousands() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(512), "512 B");
        assert_eq!(size(4096), "4 KB");
        assert_eq!(size(5 * 1024 * 1024 + 512 * 1024), "5.5 MB");
        assert_eq!(size(250 * 1024 * 1024), "250 MB");
        assert_eq!(size(12 * GIB + GIB * 3 / 10), "12.3 GB");
        assert_eq!(size(958 * GIB + GIB / 3), "958 GB");
        assert_eq!(size(1517 * GIB), "1,517 GB");
    }

    #[test]
    fn counts_of_files() {
        assert_eq!(files(0), "0 files");
        assert_eq!(files(1), "1 file");
        assert_eq!(files(4_032_654), "4,032,654 files");
    }

    #[test]
    fn counts_of_file_types() {
        assert_eq!(file_types(1), "1 file type");
        assert_eq!(file_types(7), "7 file types");
        assert_eq!(file_types(0), "0 file types");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(45), "45 s");
        assert_eq!(duration(142), "2 min 22 s");
        assert_eq!(duration(120), "2 min");
        assert_eq!(duration(3 * 3600 + 5 * 60 + 9), "3 h 5 min");
    }

    #[test]
    fn progress_line_with_and_without_a_percentage() {
        assert_eq!(progress(Some(0.643), 2_581_000, 958 * GIB), "64% \u{00b7} 2,581,000 files \u{00b7} 958 GB so far");
        assert_eq!(progress(None, 12, 4096), "12 files \u{00b7} 4 KB so far");
    }

    #[test]
    fn the_finished_line() {
        assert_eq!(
            finished("today", "21:12", 142, 1517 * GIB, 4_032_654),
            "Finished today at 21:12 after 2 min 22 s: 1,517 GB in 4,032,654 files."
        );
    }

    #[test]
    fn day_words() {
        assert_eq!(day_word(20_000, 20_000), "today");
        assert_eq!(day_word(19_999, 20_000), "yesterday");
        assert_eq!(day_word(19_990, 20_000), "10 days ago");
    }

    #[test]
    fn where_the_space_goes() {
        assert_eq!(
            where_it_goes("DockerSandboxes", 713 * GIB, 1517 * GIB, "C:\\"),
            "DockerSandboxes holds 713 GB, almost half of what the scan counted on C:\\."
        );
        assert_eq!(where_it_goes("Users", 98, 100, "D:\\"), "Users holds 98 B, nearly all of what the scan counted on D:\\.");
        assert_eq!(where_it_goes("Users", 75, 100, "D:\\"), "Users holds 75 B, most of what the scan counted on D:\\.");
        assert_eq!(where_it_goes("Users", 55, 100, "D:\\"), "Users holds 55 B, more than half of what the scan counted on D:\\.");
        assert_eq!(where_it_goes("Users", 30, 100, "D:\\"), "Users holds 30 B, 30% of what the scan counted on D:\\.");
    }

    #[test]
    fn unreadable_notes() {
        assert_eq!(unreadable(1), "1 folder could not be read, so the total is a floor.");
        assert_eq!(unreadable(37), "37 folders could not be read, so the total is a floor.");
        assert_eq!(unreadable(1234), "1,234 folders could not be read, so the total is a floor.");
        assert_eq!(unreadable_unknown(), "Some items could not be read, so the total is a floor.");
    }

    #[test]
    fn nothing_skipped_means_no_note() {
        assert_eq!(skipped(0, false), None);
        assert_eq!(skipped(0, true), None);
    }

    #[test]
    fn while_scanning_the_note_says_the_total_is_a_floor() {
        assert_eq!(
            skipped(3, false),
            Some(Skipped { heading: None, count: None, sentence: "3 folders could not be read, so the total is a floor.".into() })
        );
    }

    #[test]
    fn once_finished_the_note_says_the_scan_finished_with_skipped_items() {
        assert_eq!(
            skipped(37, true),
            Some(Skipped {
                heading: Some("Scan finished with skipped items".into()),
                count: Some("37 items could not be measured".into()),
                sentence: "The scan has finished. These items could not be counted, so the total is incomplete.".into(),
            })
        );
        assert_eq!(skipped(1, true).unwrap().count.as_deref(), Some("1 item could not be measured"));
    }
}
