//! Services on Linux: the list from the service managers the system runs
//! (systemd, OpenRC and runit), through [`super::sources`], and Start, Stop,
//! Restart and enable or disable at boot, which each source does for its own
//! services.

use super::openrc::OpenRc;
use super::runit::Runit;
use super::sources::{control, list_all, ControlHost, ServiceSource};

use super::systemd::Systemd;
use crate::fs_root::FsRoot;
use super::{RunningService, ServiceRow};
use crate::action_elevation::spawn_and_wait;
use crate::platform::SystemRunner;
use crate::system_lists::ServiceKey;

/// Every service manager this app can read, in the order they are asked.
fn sources() -> Vec<Box<dyn ServiceSource>> {
    vec![Box::new(Systemd), Box::new(OpenRc), Box::new(Runit::new(FsRoot::system()))]
}

/// The services of every source present, or why none could be read.
pub fn list_services() -> Result<Vec<ServiceRow>, String> {
    list_all(&sources(), &SystemRunner)
}

/// The running systemd services with their host processes, found from the
/// processes' cgroups (reading the service list would run `systemctl` on
/// every sampler tick, which is too much for a link).
pub fn running_services() -> Vec<RunningService> {
    running_in(&crate::proc_info::hosted_units())
}

/// [`RunningService`]s of the hosted `units` (host PID, unit name). The
/// names are the ones the Services page lists, so Go to service(s) finds them.
fn running_in(units: &[(u32, String)]) -> Vec<RunningService> {
    units.iter().map(|(pid, unit)| RunningService { pid: *pid, name: unit.clone(), display: unit.clone(), shared: false }).collect()
}

/// The folders unit files are read from, the first that has the unit wins.
const UNIT_DIRS: [&str; 6] = [
    "etc/systemd/system",
    "run/systemd/system",
    "usr/lib/systemd/system",
    "lib/systemd/system",
    "etc/systemd/user",
    "usr/lib/systemd/user",
];

/// The description of a service, from its unit file; empty when unknown.
pub fn service_description(name: &str) -> String {
    description_in(&FsRoot::system(), name)
}

/// [`service_description`] under `root`.
fn description_in(root: &FsRoot, name: &str) -> String {
    if name.contains('/') || !name.ends_with(".service") {
        return String::new();
    }
    UNIT_DIRS
        .iter()
        .filter_map(|dir| root.read(&format!("{dir}/{name}")))
        .find_map(|text| unit_description(&text))
        .unwrap_or_default()
}

/// The `Description=` of a unit file's `[Unit]` group.
fn unit_description(text: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            inside = line == "[Unit]";
        } else if inside {
            if let Some(value) = line.strip_prefix("Description=") {
                return Some(value.trim().to_string()).filter(|d| !d.is_empty());
            }
        }
    }
    None
}

/// Does `action` ("start", "stop", "restart", "enable", "disable") to the
/// service `service` names, through the source that listed it.
pub fn control_service(service: &ServiceKey, action: &str) -> Result<(), String> {
    let env = |name: &str| std::env::var(name).ok();
    control(&sources(), &ControlHost { launch: &spawn_and_wait, env: &env }, service, action)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    #[test]
    fn a_hosted_unit_is_a_running_service_under_the_name_the_services_page_lists() {
        let running = running_in(&[(100, "pipewire.service".to_string()), (300, "nginx.service".to_string())]);
        assert_eq!(running.iter().map(|s| (s.pid, s.name.as_str(), s.display.as_str())).collect::<Vec<_>>(), [(100, "pipewire.service", "pipewire.service"), (300, "nginx.service", "nginx.service")]);
        assert!(running.iter().all(|s| !s.shared));
    }

    #[test]
    fn a_service_takes_its_description_from_its_unit_file_the_admins_copy_first() {
        let root = fixture("units");
        assert_eq!(description_in(&root, "nginx.service"), "Local nginx override");
        assert_eq!(description_in(&root, "pipewire.service"), "PipeWire Multimedia Service", "a user unit");
        assert_eq!(description_in(&root, "ghost.service"), "");
        assert_eq!(description_in(&root, "../etc/os-release"), "", "a name is not a path");
    }

    #[test]
    fn only_the_unit_group_has_the_description() {
        assert_eq!(unit_description("[Service]\nDescription=no\n[Unit]\nDescription= yes \n").as_deref(), Some("yes"));
        assert_eq!(unit_description("[Unit]\nAfter=x\n"), None);
    }
}
