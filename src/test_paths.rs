//! Test fixtures name places the way Windows does (`R:\big\a.bin`); on
//! other systems the same fixture is spelled with that system's paths, so a
//! test of app logic runs everywhere.

/// The fixture path `windows_path` as a path of the system running the test:
/// unchanged on Windows; elsewhere the drive `R:\` becomes the folder `/r`
/// and every `\` a `/`.
pub fn host_owned(windows_path: &str) -> String {
    #[cfg(windows)]
    return windows_path.to_string();
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return posix_spelling(windows_path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn posix_spelling(windows_path: &str) -> String {
    let mut chars = windows_path.chars();
    match (chars.next(), chars.next()) {
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic() => format!("/{}{}", drive.to_ascii_lowercase(), windows_path[2..].replace('\\', "/")).trim_end_matches('/').to_string(),
        _ => windows_path.replace('\\', "/"),
    }
}

/// [`host_owned`] of a literal, as a literal.
pub fn host(windows_path: &'static str) -> &'static str {
    #[cfg(windows)]
    return windows_path;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    return Box::leak(host_owned(windows_path).into_boxed_str());
}

#[cfg(test)]
mod tests {
    use super::host;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_windows_fixture_becomes_a_path_of_this_system() {
        assert_eq!(host("R:\\"), "/r");
        assert_eq!(host("R:\\d\\e\\f.zip"), "/r/d/e/f.zip");
        assert_eq!(host("relative\\x"), "relative/x");
    }

    #[cfg(windows)]
    #[test]
    fn on_windows_a_fixture_is_unchanged() {
        assert_eq!(host("R:\\d\\f.zip"), "R:\\d\\f.zip");
    }
}
