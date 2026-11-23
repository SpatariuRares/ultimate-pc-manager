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

/// The entries of every source that can be read.
pub fn list_startup() -> Vec<StartupEntry> {
    // A source that cannot be read lists nothing; the others still show.
    list_all(&sources()).0
}

/// Turns an entry on or off through the source that read it.
pub fn set_startup_enabled(entry: &StartupEntry, enable: bool) -> Result<(), String> {
    set_enabled_in(&sources(), entry, enable)
}
