//! The fake adapter for tests: scripted lists and outcomes, and a record of
//! what was asked. Tests keep an `Arc<FakeSystem>` and hand a clone to the
//! app, so they can script and read it after the app owns the seam.

use std::sync::Mutex;

use super::{ServiceRow, SystemLists};

struct State {
    services: Result<Vec<ServiceRow>, String>,
    service_reads: usize,
    // Service control state is added here.

    // Startup entries state is added here.
}

pub struct FakeSystem {
    state: Mutex<State>,
}

impl Default for FakeSystem {
    /// An empty, readable services list.
    fn default() -> Self {
        FakeSystem { state: Mutex::new(State { services: Ok(Vec::new()), service_reads: 0 }) }
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

    // Startup entries scripts and records are added here.
}

impl SystemLists for FakeSystem {
    fn list_services(&self) -> Result<Vec<ServiceRow>, String> {
        let mut s = self.state.lock().unwrap();
        s.service_reads += 1;
        s.services.clone()
    }

    // Service control is added here.

    // Startup entries are added here.
}
