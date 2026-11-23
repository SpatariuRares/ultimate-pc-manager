//! Services on macOS: not available yet, as the app already says.

use super::{RunningService, ServiceRow};
use crate::availability::{unavailable_error, Feature};

/// No Service Control Manager: an error.
pub fn list_services() -> Result<Vec<ServiceRow>, String> {
    Err(unavailable_error(Feature::ServiceListing))
}

/// No running services.
pub fn running_services() -> Vec<RunningService> {
    Vec::new()
}

/// No service descriptions.
pub fn service_description(_: &str) -> String {
    String::new()
}

/// Services cannot be started or stopped: an error.
pub fn control_service(_: &str, _: &str) -> Result<(), String> {
    Err(unavailable_error(Feature::ServiceControl))
}
