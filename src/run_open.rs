//! Showing a file in the file manager, starting a program, folder or
//! document by name, and opening the power settings.
//!
//! The module holds the checks of what the user typed; the `windows`,
//! `linux` and `macos` submodules do the opening.

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as system;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as system;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as system;

pub use system::{open_power_options, reveal_in_explorer};

/// Starts `launch` (program, then its arguments) without waiting for it: the
/// one place the Linux and macOS openers start a process.
#[cfg(not(windows))]
fn spawn(launch: &[String]) -> Result<(), String> {
    let (program, args) = launch.split_first().ok_or_else(|| "nothing to run".to_string())?;
    #[cfg(test)]
    if recorded::note(launch) {
        return Ok(());
    }
    std::process::Command::new(program).args(args).spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Test seam: what [`spawn`] was asked to start, without starting it.
#[cfg(all(test, not(windows)))]
pub(crate) mod recorded {
    use std::cell::RefCell;

    thread_local! {
        static LAUNCHES: RefCell<Option<Vec<Vec<String>>>> = const { RefCell::new(None) };
    }

    /// Runs `f` with every launch of this thread recorded, not started; gives
    /// `f`'s result and the launches (program, then arguments).
    pub fn launches<T>(f: impl FnOnce() -> T) -> (T, Vec<Vec<String>>) {
        LAUNCHES.with(|l| *l.borrow_mut() = Some(Vec::new()));
        let result = f();
        let launches = LAUNCHES.with(|l| l.borrow_mut().take()).unwrap_or_default();
        (result, launches)
    }

    /// Notes a launch when recording; whether it was.
    pub(super) fn note(launch: &[String]) -> bool {
        LAUNCHES.with(|l| l.borrow_mut().as_mut().map(|v| v.push(launch.to_vec())).is_some())
    }
}

#[cfg(all(test, not(windows)))]
mod spawn_tests {
    #[test]
    fn an_empty_launch_is_an_error_not_a_start() {
        assert_eq!(super::spawn(&[]), Err("nothing to run".to_string()));
    }
}

pub fn open_file_location(path: &str) -> Result<(), String> {
    if path.is_empty() {
        return Err("The file location of this process can't be read.".into());
    }
    system::show_file_location(path)
}

pub fn run_new_task(cmdline: &str) -> Result<(), String> {
    let cmdline = cmdline.trim();
    if cmdline.is_empty() {
        return Err("Type the name of a program, folder or document.".into());
    }
    system::start_command(cmdline)
}
