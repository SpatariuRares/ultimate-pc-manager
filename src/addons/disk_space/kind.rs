//! File types (pure): the seven categories of the File Type legend and
//! grouping, from a file's extension.

/// A file's category, in the legend's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Category {
    Media,
    Documents,
    Code,
    Archives,
    Applications,
    System,
    Other,
}

/// The extension lists of each category.
const MEDIA: &str = "png jpg jpeg gif heic heif tiff tif bmp webp svg raw cr2 nef dng psd mp3 m4a aac wav aiff flac ogg opus mp4 m4v mov avi mkv webm wmv flv";
const DOCUMENTS: &str = "pdf doc docx pages txt rtf md odt xls xlsx numbers csv tsv ods ppt pptx key odp epub mobi";
const CODE: &str = "swift c h cpp cc hpp m mm rs go py rb js ts jsx tsx java kt cs php sh zsh bash pl lua sql html css json xml yml yaml toml plist cmake";
const ARCHIVES: &str = "zip tar gz bz2 xz 7z rar dmg iso pkg jar war aar cab zst";
/// Includes .exe/.msi (owner decision on #80).
const APPLICATIONS: &str = "app framework bundle xcappdata exe msi";
/// Includes .dll/.sys (owner decision on #80) and .etl (trace logs); .pf
/// (prefetch files) is Other, as the PRD says.
const SYSTEM: &str = "dylib so a o kext log cache db sqlite sqlite3 shm wal swp tmp dll sys etl";

impl Category {
    /// Every category, in the legend's order.
    pub const ALL: [Category; 7] = [
        Category::Media,
        Category::Documents,
        Category::Code,
        Category::Archives,
        Category::Applications,
        Category::System,
        Category::Other,
    ];

    /// The category of a file, from its extension (any case); no or an
    /// unknown extension = Other.
    pub fn of(path: &std::path::Path) -> Category {
        let Some(ext) = path.extension().and_then(|e| e.to_str()).filter(|e| !e.is_empty()) else { return Category::Other };
        let ext = ext.to_ascii_lowercase();
        let lists = [(MEDIA, Category::Media), (DOCUMENTS, Category::Documents), (CODE, Category::Code), (ARCHIVES, Category::Archives), (APPLICATIONS, Category::Applications), (SYSTEM, Category::System)];
        lists.into_iter().find(|(list, _)| list.split(' ').any(|e| e == ext)).map_or(Category::Other, |(_, c)| c)
    }

    /// Its place in [`Category::ALL`].
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Media => "Media",
            Category::Documents => "Documents",
            Category::Code => "Code",
            Category::Archives => "Archives",
            Category::Applications => "Applications",
            Category::System => "System",
            Category::Other => "Other",
        }
    }

    /// Its fixed colour (r, g, b) in the legend and the drawings, the
    /// capture's hues; the same in both themes since the canvas stays dark.
    pub fn colour(self) -> (u8, u8, u8) {
        match self {
            Category::Media => (0xd9, 0x60, 0x5a),
            Category::Documents => (0x4c, 0xaf, 0x78),
            Category::Code => (0x4f, 0x8f, 0xe6),
            Category::Archives => (0xd4, 0xa6, 0x3a),
            Category::Applications => (0x9a, 0x74, 0xdc),
            Category::System => (0x3a, 0xae, 0xbb),
            Category::Other => (0x7d, 0x85, 0x97),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn of(name: &str) -> Category {
        Category::of(Path::new(name))
    }

    #[test]
    fn extensions_fall_in_their_categories_whatever_their_case() {
        assert_eq!(of("movie.mkv"), Category::Media);
        assert_eq!(of("PHOTO.JPG"), Category::Media);
        assert_eq!(of("report.Docx"), Category::Documents);
        assert_eq!(of("main.rs"), Category::Code);
        assert_eq!(of("Cargo.TOML"), Category::Code);
        assert_eq!(of("backup.7z"), Category::Archives);
        assert_eq!(of("data.tar.gz"), Category::Archives);
        assert_eq!(of("cache.db"), Category::System);
    }

    #[test]
    fn windows_programs_are_applications_and_libraries_and_drivers_are_system() {
        // Owner decision on #80.
        assert_eq!(of("setup.EXE"), Category::Applications);
        assert_eq!(of("tool.msi"), Category::Applications);
        assert_eq!(of("kernel32.dll"), Category::System);
        assert_eq!(of("ntfs.sys"), Category::System);
    }

    #[test]
    fn trace_logs_are_system_but_prefetch_files_are_other() {
        // The PRD's decision on #80.
        assert_eq!(of("boot.etl"), Category::System);
        assert_eq!(of("APP.EXE-1A2B3C4D.pf"), Category::Other);
        assert_eq!(of("layout.pf"), Category::Other);
    }

    #[test]
    fn files_without_a_known_extension_are_other() {
        assert_eq!(of("README"), Category::Other);
        assert_eq!(of("disk.vhdx"), Category::Other);
        assert_eq!(of(".gitignore"), Category::Other);
        assert_eq!(of("trailing."), Category::Other);
    }

    #[test]
    fn the_legend_lists_seven_categories_in_a_fixed_order_with_distinct_colours() {
        let names: Vec<_> = Category::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(names, ["Media", "Documents", "Code", "Archives", "Applications", "System", "Other"]);
        let colours: std::collections::HashSet<_> = Category::ALL.iter().map(|c| c.colour()).collect();
        assert_eq!(colours.len(), 7);
        assert!(Category::ALL.iter().enumerate().all(|(i, c)| c.index() == i));
    }
}
