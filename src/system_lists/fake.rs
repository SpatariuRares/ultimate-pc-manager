//! The fake adapter for tests: scripted lists and outcomes, and a record of
//! what was asked. Tests keep an `Arc<FakeSystem>` and hand a clone to the
//! app, so they can script and read it after the app owns the seam.

use std::sync::Mutex;

use super::{ServiceRow, StartupRow, SystemLists};

struct State {
    services: Result<Vec<ServiceRow>, String>,
    service_reads: usize,
    // Service control state is added here.

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
        FakeSystem { state: Mutex::new(State {
                services: Ok(Vec::new()),
                service_reads: 0,
                startup: Vec::new(),
                startup_reads: 0,
                startup_outcome: Ok(()),
                startup_requests: Vec::new(),
            }) }
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

    // Service control scripts and records are added here.

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

    // Service control is added here.

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
