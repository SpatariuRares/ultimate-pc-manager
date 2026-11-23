//! Opening things on Linux: the file manager and the shell, each
//! built as a launch (program and arguments) that [`super::spawn`] starts.

use crate::availability::{unavailable_error, Feature};

/// The launch that shows the folder holding `path`.
fn file_manager_launch(path: &str) -> Vec<String> {
    let dir = std::path::Path::new(path).parent().map(|p| p.display().to_string()).unwrap_or_default();
    vec!["xdg-open".to_string(), dir]
}

/// The launch that starts `cmdline` through the shell.
fn shell_launch(cmdline: &str) -> Vec<String> {
    vec!["sh".to_string(), "-c".to_string(), cmdline.to_string()]
}

/// Opens the folder holding `path`.
pub fn show_file_location(path: &str) -> Result<(), String> {
    super::spawn(&file_manager_launch(path))
}

/// Opens the folder holding `path`.
pub fn reveal_in_explorer(path: &std::path::Path) -> Result<(), String> {
    super::open_file_location(&path.display().to_string())
}

/// There is no Power Options panel here: an error.
pub fn open_power_options() -> Result<(), String> {
    Err(unavailable_error(Feature::PowerOptionsPanel))
}

/// Starts `cmdline` through the shell.
pub fn start_command(cmdline: &str) -> Result<(), String> {
    super::spawn(&shell_launch(cmdline))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_manager_is_asked_for_the_folder_of_the_file() {
        assert_eq!(file_manager_launch("/home/a/b.txt"), ["xdg-open", "/home/a"]);
    }

    #[test]
    fn the_shell_gets_the_text_as_one_argument() {
        assert_eq!(shell_launch("gedit 'a b'; id"), ["sh", "-c", "gedit 'a b'; id"]);
    }
}
