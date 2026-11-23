//! Paths handed to Windows: the `\\?\` long-path prefix.

/// `path` as handed to Windows: with the `\\?\` long-path prefix
/// (`\\?\UNC\` for a share) when it is too long for the plain form, else
/// unchanged. The prefix turns Windows' own normalisation off, so the path
/// is normalised first (`/` to `\`, `.` and `..` resolved); a relative
/// path cannot take it and is left as it is. The length is counted in
/// UTF-16 units, as Windows counts it. Paths are kept and shown
/// unprefixed; every place that hands one to Windows goes through this,
/// and nothing else builds the prefix.
pub fn long_path(path: &std::path::Path) -> std::path::PathBuf {
    /// Below MAX_PATH less room for an 8.3 name, as folders need.
    const PLAIN_MAX: usize = 240;
    let units = |p: &std::path::Path| p.as_os_str().to_string_lossy().encode_utf16().count();
    if !cfg!(windows) || !path.is_absolute() || units(path) < PLAIN_MAX || path.as_os_str().to_string_lossy().starts_with(r"\\?\") {
        return path.to_path_buf();
    }
    let full = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = full.as_os_str().to_string_lossy();
    if units(&full) < PLAIN_MAX || s.starts_with(r"\\?\") {
        full
    } else if let Some(unc) = s.strip_prefix(r"\\") {
        std::path::PathBuf::from(format!(r"\\?\UNC\{unc}"))
    } else {
        std::path::PathBuf::from(format!(r"\\?\{s}"))
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn only_a_path_too_long_for_the_plain_form_gets_the_long_path_prefix() {
        use std::path::{Path, PathBuf};
        let deep = "d".repeat(250);
        assert_eq!(long_path(Path::new(r"C:\Users\a.txt")), PathBuf::from(r"C:\Users\a.txt"));
        assert_eq!(long_path(&Path::new(r"C:\").join(&deep)), PathBuf::from(format!(r"\\?\C:\{deep}")));
        assert_eq!(long_path(&Path::new(r"\\srv\share").join(&deep)), PathBuf::from(format!(r"\\?\UNC\srv\share\{deep}")));
        let prefixed = PathBuf::from(format!(r"\\?\C:\{deep}"));
        assert_eq!(long_path(&prefixed), prefixed, "already prefixed");
    }

    #[test]
    fn the_long_path_prefix_goes_only_on_a_normalised_absolute_path() {
        use std::path::{Path, PathBuf};
        let deep = "d".repeat(250);
        // The prefix turns normalisation off, so it is done first.
        assert_eq!(long_path(Path::new(&format!("C:/Data/{deep}"))), PathBuf::from(format!(r"\\?\C:\Data\{deep}")), "slashes");
        assert_eq!(long_path(Path::new(&format!(r"C:\Data\.\skip\..\{deep}"))), PathBuf::from(format!(r"\\?\C:\Data\{deep}")), "dot folders");
        assert_eq!(long_path(Path::new(&format!(r"\\srv\share\x\..\{deep}"))), PathBuf::from(format!(r"\\?\UNC\srv\share\{deep}")), "a share");
        // A relative path cannot take the prefix.
        let relative = PathBuf::from(format!(r"Data\{deep}"));
        assert_eq!(long_path(&relative), relative, "relative");
        // The limit is in UTF-16 units, as Windows counts: 200 "é" are 400
        // bytes but 200 units.
        let accented = PathBuf::from(format!(r"C:\{}", "é".repeat(200)));
        assert_eq!(long_path(&accented), accented, "short in UTF-16 units");
        let wide = PathBuf::from(format!(r"C:\{}", "é".repeat(250)));
        assert_eq!(long_path(&wide), PathBuf::from(format!(r"\\?\{}", wide.display())), "long in UTF-16 units");
    }
}
