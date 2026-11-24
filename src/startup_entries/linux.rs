//! Startup entries on Linux: every source of the running system, listed
//! together: XDG autostart files and the user's systemd units. More join
//! the list in [`sources`].

mod systemd_user;
mod xdg_autostart;

use super::{list_all, set_enabled_in, StartupEntry, StartupSource};

/// The sources of this system, in the order their entries are shown.
fn sources() -> Vec<Box<dyn StartupSource>> {
    vec![Box::new(xdg_autostart::XdgAutostart::system()), Box::new(systemd_user::SystemdUser::system())]
}

/// Where the list comes from, as the page's band says it.
pub fn sources_note() -> &'static str {
    "From the autostart folders of the user and the system, and the user's systemd services"
}

/// The entries of every source that can be read, and a notice ("source:
/// why") for each one that cannot (it lists nothing; the others still show).
pub fn list_startup_noticed() -> (Vec<StartupEntry>, Vec<String>) {
    noticed(&sources())
}

/// What a source is called in a notice.
fn label(id: &str) -> &str {
    match id {
        xdg_autostart::ID => "autostart folders",
        systemd_user::ID => "systemd user services",
        other => other,
    }
}

fn noticed(sources: &[Box<dyn StartupSource>]) -> (Vec<StartupEntry>, Vec<String>) {
    let (entries, failures) = list_all(sources);
    (entries, failures.into_iter().map(|(id, why)| format!("{}: {why}", label(id))).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{RecordingRunner, When};
    use std::sync::Arc;

    #[test]
    fn a_source_that_cannot_be_read_is_told_while_the_others_still_list() {
        let runner = Arc::new(RecordingRunner::default());
        runner.prepare(When::Nth(0), 1, "boom");
        runner.prepare(When::Nth(1), 1, "boom");
        let root = crate::fs_root::fixture("autostart");
        let env: crate::app_folders::Env = &|name| (name == "HOME").then(|| "/home/user".into());
        let sources: Vec<Box<dyn StartupSource>> =
            vec![Box::new(xdg_autostart::XdgAutostart::new(root.clone(), env)), Box::new(systemd_user::SystemdUser::over(runner, root, env))];
        let (entries, notices) = noticed(&sources);
        assert!(!entries.is_empty(), "the autostart entries are still listed");
        assert_eq!(notices, ["systemd user services: boom"]);
    }
}

/// Turns an entry on or off through the source that read it.
pub fn set_startup_enabled(entry: &StartupEntry, enable: bool) -> Result<(), String> {
    set_enabled_in(&sources(), entry, enable)
}
