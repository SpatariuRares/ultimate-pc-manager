//! The Windows adapter: the Service Control Manager and the startup
//! locations, through [`crate::winsys`].

use super::{ServiceRow, SystemLists};

/// The machine this app runs on.
pub struct NativeSystem;

impl SystemLists for NativeSystem {
    fn list_services(&self) -> Result<Vec<ServiceRow>, String> {
        crate::winsys::list_services()
    }

    fn control_service(&self, name: &str, action: &str) -> Result<(), String> {
        crate::winsys::control_service(name, action)
    }

    // Startup entries are added here.
}
