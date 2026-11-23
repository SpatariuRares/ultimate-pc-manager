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
    /// The service manager the row comes from ("systemd"); empty on Windows,
    /// which has the one.
    pub source: String,
    /// The scope the service belongs to ("System" or "User"); empty when the
    /// manager has none.
    pub scope: String,
}

/// What the Services and Startup apps pages ask of the machine.
pub trait SystemLists {
    /// Every service with its state, or why the list could not be read.
    fn list_services(&self) -> Result<Vec<ServiceRow>, String>;

    /// Starts, stops or restarts a service (`action` is "start", "stop" or
    /// "restart"), or says why it could not be done.
    fn control_service(&self, name: &str, action: &str) -> Result<(), String>;

    /// Every startup entry with its state. Startup has no error state: a
    /// location that cannot be read simply lists nothing.
    fn list_startup(&self) -> Vec<StartupRow>;

    /// Enables or disables the startup entry named by `id` (a
    /// [`StartupRow::id`]), or says why it could not.
    fn set_startup_enabled(&self, id: &str, enable: bool) -> Result<(), String>;
}

/// One startup entry as the Startup apps page shows it.
#[derive(Clone, Debug, Default)]
pub struct StartupRow {
    /// Names this entry for as long as it exists, whatever its position in
    /// the list: an action carries it instead of an index.
    pub id: String,
    pub name: String,
    pub command: String,
    pub location: String,
    pub enabled: bool,
}

/// Said when an action names a startup entry that has left the list.
pub const STARTUP_GONE: &str = "That startup app is no longer in the list";
