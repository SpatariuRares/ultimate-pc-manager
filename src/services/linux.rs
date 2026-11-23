//! Services on Linux: the list from the service managers the system runs
//! (systemd now), through [`super::sources`]. Start, stop and restart are
//! not available yet.

use super::sources::{list_all, ServiceSource};
use super::systemd::Systemd;
use super::{RunningService, ServiceRow};
use crate::availability::{unavailable_error, Feature};
use crate::platform::SystemRunner;

/// Every service manager this app can read, in the order they are asked.
fn sources() -> Vec<Box<dyn ServiceSource>> {
    vec![Box::new(Systemd)]
}

/// The services of every source present, or why none could be read.
pub fn list_services() -> Result<Vec<ServiceRow>, String> {
    list_all(&sources(), &SystemRunner)
}

/// No running services (the Processes page does not link to them yet).
pub fn running_services() -> Vec<RunningService> {
    Vec::new()
}

/// No service descriptions.
pub fn service_description(_: &str) -> String {
    String::new()
}

/// Services cannot be started or stopped yet: an error.
pub fn control_service(_: &str, _: &str) -> Result<(), String> {
    Err(unavailable_error(Feature::ServiceControl))
}
