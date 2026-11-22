//! The seam between the Services and Startup apps pages and the machine.
//!
//! The pages read and control their entries through [`SystemLists`]; the
//! rows belong to the pages (defined here), not to Windows. Two adapters
//! fill the seam: [`native::NativeSystem`] (the Service Control Manager and
//! the startup locations) and, for tests, [`fake::FakeSystem`] (scripted
//! lists and outcomes that records what was asked).
//!
//! The app holds one `Arc<dyn SystemLists>`, chosen when it is built (the
//! normal app gets the native adapter, the headless app the fake), and
//! every place that needs a list calls the app's one reload step for it
//! (`TaskApp::reload_services`).
//!
//! To add an operation: a method here, its native implementation in
//! `native.rs`, its scripted answer and request record in `fake.rs`, and
//! (for a list) a `reload_*` step on `TaskApp` that all readers call.

pub mod native;

#[cfg(test)]
pub mod fake;

/// One service as the Services page shows it.
#[derive(Clone, Debug, Default)]
pub struct ServiceRow {
    pub name: String,
    pub display: String,
    pub pid: u32,
    pub status: String,
    pub start_type: String,
}

/// What the Services and Startup apps pages ask of the machine.
pub trait SystemLists {
    /// Every service with its state, or why the list could not be read.
    fn list_services(&self) -> Result<Vec<ServiceRow>, String>;

    /// Starts, stops or restarts a service (`action` is "start", "stop" or
    /// "restart"), or says why it could not be done.
    fn control_service(&self, name: &str, action: &str) -> Result<(), String>;

    // Startup entries (list, enable, disable) are added here.
}
