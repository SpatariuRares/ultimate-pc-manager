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
    /// The service manager the row comes from ("systemd", "OpenRC" or "runit"); empty on Windows,
    /// which has the one.
    pub source: String,
    /// The scope the service belongs to ("System" or "User"); empty when the
    /// manager has none.
    pub scope: String,
    /// The actions ("start", "stop", "restart", "enable", "disable") the
    /// row's manager can do to it; `None` where the system offers every one
    /// (Windows).
    pub actions: Option<&'static [&'static str]>,
}

impl ServiceRow {
    /// Does the page offer `action` for this service?
    pub fn can(&self, action: &str) -> bool {
        self.actions.is_none_or(|list| list.contains(&action))
    }

    /// What names this service in an action: its name, with the manager and
    /// scope it belongs to (a system and a user service can share a name).
    pub fn key(&self) -> ServiceKey {
        ServiceKey { name: self.name.clone(), source: self.source.clone(), scope: self.scope.clone() }
    }
}

/// Names one service for an action: the name alone is not enough where
/// several managers and scopes can hold the same one. `source` and `scope`
/// are empty where the system has one manager and no scopes (Windows).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServiceKey {
    pub name: String,
    pub source: String,
    pub scope: String,
}

impl From<&str> for ServiceKey {
    /// A service known by its name only.
    fn from(name: &str) -> Self {
        ServiceKey { name: name.into(), ..Default::default() }
    }
}

/// The services read, and a notice for each source that could not be read
/// (the rows of the others are still here).
#[derive(Clone, Debug, Default)]
pub struct ServiceList {
    pub rows: Vec<ServiceRow>,
    pub notices: Vec<String>,
}

/// The startup entries read, and a notice for each source that could not be
/// read (the entries of the others are still here).
#[derive(Clone, Debug, Default)]
pub struct StartupList {
    pub rows: Vec<StartupRow>,
    pub notices: Vec<String>,
}

/// What the Services and Startup apps pages ask of the machine.
pub trait SystemLists {
    /// Every service with its state, or why the list could not be read.
    fn list_services(&self) -> Result<ServiceList, String>;

    /// Starts, stops or restarts a service, or turns it on or off at boot
    /// (`action` is "start", "stop", "restart", "enable" or "disable"), or
    /// says why it could not be done.
    fn control_service(&self, service: &ServiceKey, action: &str) -> Result<(), String>;

    /// Every startup entry with its state. Startup has no error state: a
    /// location that cannot be read lists nothing and is told in a notice.
    fn list_startup(&self) -> StartupList;

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
    /// The file the entry lives in, when it has one to show ("Open file
    /// location" opens its folder); empty when the command is the target.
    pub file: String,
}

/// Said when an action names a startup entry that has left the list.
pub const STARTUP_GONE: &str = "That startup app is no longer in the list";
