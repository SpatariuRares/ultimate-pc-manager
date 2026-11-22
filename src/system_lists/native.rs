//! The Windows adapter: the Service Control Manager and the startup
//! locations, through [`crate::winsys`].

use super::{ServiceRow, SystemLists};

/// The machine this app runs on.
pub struct NativeSystem;

impl SystemLists for NativeSystem {
    fn list_services(&self) -> Result<Vec<ServiceRow>, String> {
        crate::winsys::list_services()
    }

    // Service control is added here.

    // Startup entries are added here.
}
