//! Services on Linux: the list from the service managers the system runs
//! (systemd now), through [`super::sources`], and Start, Stop, Restart and
//! enable or disable at boot, which each source does for its own services.

use super::sources::{control, list_all, ControlHost, ServiceSource};
use super::systemd::Systemd;
use super::{RunningService, ServiceRow};
use crate::action_elevation::spawn_and_wait;
use crate::platform::SystemRunner;
use crate::system_lists::ServiceKey;

/// Every service manager this app can read, in the order they are asked.
fn sources() -> Vec<Box<dyn ServiceSource>> {
    vec![Box::new(Systemd)]
}

/// The services of every source present, or why none could be read.
pub fn list_services() -> Result<Vec<ServiceRow>, String> {
    list_all(&sources(), &SystemRunner)
}

/// No running services: Go to service(s) from Processes finds none here.
/// Filling this from the listing would run `systemctl` on every sampler
/// tick, which is too much for a link.
pub fn running_services() -> Vec<RunningService> {
    Vec::new()
}

/// No service descriptions.
pub fn service_description(_: &str) -> String {
    String::new()
}

/// Does `action` ("start", "stop", "restart", "enable", "disable") to the
/// service `service` names, through the source that listed it.
pub fn control_service(service: &ServiceKey, action: &str) -> Result<(), String> {
    let env = |name: &str| std::env::var(name).ok();
    control(&sources(), &ControlHost { launch: &spawn_and_wait, env: &env }, service, action)
}
