//! The Windows services: which run, with their host process, their
//! description, and Start, Stop and Restart.
//!
//! The module holds the shared row types; the `windows`, `linux` and `macos`
//! submodules read and control the services (Linux and macOS report that
//! they are not available yet).

// The service managers a Linux system can run (tested on every system).
#[cfg(any(target_os = "linux", test))]
pub(crate) mod sources;
#[cfg(any(target_os = "linux", test))]
pub(crate) mod systemd;

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

/// The Services page's row (the page owns it; this adapter fills it).
pub use crate::system_lists::ServiceRow;

pub use system::{control_service, list_services, running_services, service_description};
/// Read by Windows-only code (the sensors and the connections add-on).
#[cfg(windows)]
pub use system::{running_service_pids, service_running};

/// A running Win32 service and the process that hosts it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RunningService {
    pub pid: u32,
    pub name: String,
    pub display: String,
    /// It shares its process with other services (else it has its own).
    pub shared: bool,
}
