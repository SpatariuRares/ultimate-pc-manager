//! The Windows adapter: the Service Control Manager and the startup
//! locations, through [`crate::winsys`].

use super::{ServiceRow, StartupRow, SystemLists, STARTUP_GONE};

/// The machine this app runs on.
pub struct NativeSystem;

impl SystemLists for NativeSystem {
    fn list_services(&self) -> Result<Vec<ServiceRow>, String> {
        crate::winsys::list_services()
    }

    // Service control is added here.

    fn list_startup(&self) -> Vec<StartupRow> {
        crate::winsys::list_startup()
            .into_iter()
            .map(|e| StartupRow { id: startup_id(&e), name: e.name, command: e.command, location: e.location, enabled: e.enabled })
            .collect()
    }

    fn set_startup_enabled(&self, id: &str, enable: bool) -> Result<(), String> {
        // Read the entry again by its identity: where it lives (hive, key)
        // is Windows' business, not the page's.
        match crate::winsys::list_startup().iter().find(|e| startup_id(e) == id) {
            Some(entry) => crate::winsys::set_startup_enabled(entry, enable),
            None => Err(STARTUP_GONE.into()),
        }
    }
}

/// A startup entry's identity: its location with its name (registry) or its
/// file (Startup folder).
fn startup_id(e: &crate::winsys::StartupEntry) -> String {
    let key = if e.location == "Startup folder" { &e.command } else { &e.name };
    format!("{}|{key}", e.location)
}
