//! The fake adapter for tests: scripted lists and outcomes, and a record of
//! what was asked. Tests keep an `Arc<FakeSystem>` and hand a clone to the
//! app, so they can script and read it after the app owns the seam.

use std::sync::Mutex;

use super::{ServiceKey, ServiceRow, StartupRow, SystemLists};

struct State {
    services: Result<Vec<ServiceRow>, String>,
    service_reads: usize,
    service_control: Result<(), String>,
    service_requests: Vec<(String, String)>,
    service_targets: Vec<ServiceKey>,

    startup: Vec<StartupRow>,
    startup_reads: usize,
    startup_outcome: Result<(), String>,
    startup_requests: Vec<(String, bool)>,
}

pub struct FakeSystem {
    state: Mutex<State>,
}

impl Default for FakeSystem {
    /// An empty, readable services list.
    fn default() -> Self {
        FakeSystem {
            state: Mutex::new(State {
                services: Ok(Vec::new()),
                service_reads: 0,
                service_control: Ok(()),
                service_requests: Vec::new(),
                service_targets: Vec::new(),
                startup: Vec::new(),
                startup_reads: 0,
                startup_outcome: Ok(()),
                startup_requests: Vec::new(),
            }),
        }
    }
}

impl FakeSystem {
    /// The list the next services reads return.
    pub fn script_services(&self, rows: Vec<ServiceRow>) {
        self.state.lock().unwrap().services = Ok(rows);
    }

    /// Makes the next services reads fail with `reason`.
    pub fn fail_services(&self, reason: &str) {
        self.state.lock().unwrap().services = Err(reason.into());
    }

    /// How many times the services list was read.
    pub fn service_reads(&self) -> usize {
        self.state.lock().unwrap().service_reads
    }

    /// Makes the next service controls fail with `reason`.
    pub fn fail_service_control(&self, reason: &str) {
        self.state.lock().unwrap().service_control = Err(reason.into());
    }

    /// Every service control asked for, as (service name, action), in order.
    pub fn service_requests(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().service_requests.clone()
    }

    /// The services control was asked for (name, manager and scope), in order.
    pub fn service_targets(&self) -> Vec<ServiceKey> {
        self.state.lock().unwrap().service_targets.clone()
    }

    /// The list the next startup reads return.
    pub fn script_startup(&self, rows: Vec<StartupRow>) {
        self.state.lock().unwrap().startup = rows;
    }

    /// Makes the next enable or disable fail with `reason`.
    pub fn fail_startup_toggle(&self, reason: &str) {
        self.state.lock().unwrap().startup_outcome = Err(reason.into());
    }

    /// How many times the startup list was read.
    pub fn startup_reads(&self) -> usize {
        self.state.lock().unwrap().startup_reads
    }

    /// Every enable or disable asked for, as (entry id, enable), in order.
    pub fn startup_requests(&self) -> Vec<(String, bool)> {
        self.state.lock().unwrap().startup_requests.clone()
    }
}

impl SystemLists for FakeSystem {
    fn list_services(&self) -> Result<Vec<ServiceRow>, String> {
        let mut s = self.state.lock().unwrap();
        s.service_reads += 1;
        s.services.clone()
    }

    fn control_service(&self, service: &ServiceKey, action: &str) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.service_requests.push((service.name.clone(), action.into()));
        s.service_targets.push(service.clone());
        s.service_control.clone()
    }

    fn list_startup(&self) -> Vec<StartupRow> {
        let mut s = self.state.lock().unwrap();
        s.startup_reads += 1;
        s.startup.clone()
    }

    fn set_startup_enabled(&self, id: &str, enable: bool) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.startup_requests.push((id.to_string(), enable));
        s.startup_outcome.clone()
    }
}
