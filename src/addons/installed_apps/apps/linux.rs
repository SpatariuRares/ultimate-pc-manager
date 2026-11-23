//! Linux: the uninstall registry, Store apps and the shell's usage counters: not available yet.

use super::*;
pub fn list_registry() -> Vec<App> {
    Vec::new()
}
pub fn list_msix_and_links(_: &AddonContext) -> (Vec<App>, Vec<Link>) {
    (Vec::new(), Vec::new())
}
pub fn user_assist() -> Vec<Opened> {
    Vec::new()
}
