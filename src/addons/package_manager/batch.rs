//! A series of packages installed one after another: the status of each
//! package, the order they run in (those that need administrator rights
//! first) and the summary in words once the series ends.
//!
//! A package's status moves `Pending → Running → (Ok | OkRestart |
//! Skipped | Cancelled | Failed | NotStarted)`; a package the series never
//! reaches goes from `Pending` to `NotStarted`. `Manual` is for packages no
//! catalog knows (bundles): they are never run, only listed in the summary.

use std::path::PathBuf;

use crate::platform::install_queue::Work;
use crate::platform::winget::join_names;

use super::catalog::{Found, Pin};

/// What a series does to its packages; the series' words come from it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Install,
    Update,
}

impl Kind {
    /// "Installing", "Updating".
    pub fn doing(self) -> &'static str {
        match self {
            Kind::Install => "Installing",
            Kind::Update => "Updating",
        }
    }

    /// "installed", "updated".
    pub fn done(self) -> &'static str {
        match self {
            Kind::Install => "installed",
            Kind::Update => "updated",
        }
    }

    /// [`Kind::done`] starting with a capital: "Installed", "Updated".
    pub fn done_title(self) -> &'static str {
        match self {
            Kind::Install => "Installed",
            Kind::Update => "Updated",
        }
    }

    /// The series as a whole: "installs", "updates".
    pub fn works(self) -> &'static str {
        match self {
            Kind::Install => "installs",
            Kind::Update => "updates",
        }
    }

    /// The word starting its packages' log files: "install", "update".
    pub fn log_word(self) -> &'static str {
        match self {
            Kind::Install => "install",
            Kind::Update => "update",
        }
    }

    /// Its work in the app's install queue.
    pub fn work(self) -> Work {
        match self {
            Kind::Install => Work::Install,
            Kind::Update => Work::Update,
        }
    }
}

/// Where a package of a series is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not reached yet.
    Pending,
    /// Installing now.
    Running,
    /// Installed.
    Ok,
    /// Installed; Windows must restart to finish.
    OkRestart,
    /// Nothing was done, for this reason.
    Skipped(String),
    /// The installation was cancelled (a UAC prompt was closed).
    Cancelled,
    /// It started and failed, for this reason (with the raw code).
    Failed(String),
    /// It was never started, for this reason.
    NotStarted(String),
    /// To install by hand from this address (no catalog knows it).
    Manual(String),
}

impl Status {
    /// The status may move to `next`.
    pub fn can_become(&self, next: &Status) -> bool {
        use Status::*;
        match (self, next) {
            (Pending, Running | NotStarted(_)) => true,
            (Running, Ok | OkRestart | Skipped(_) | Cancelled | Failed(_) | NotStarted(_)) => true,
            _ => false,
        }
    }

    /// It will not change any more.
    pub fn finished(&self) -> bool {
        !matches!(self, Status::Pending | Status::Running)
    }

    /// The status in a few words, for its row while a series of `kind`
    /// runs.
    pub fn text(&self, kind: Kind) -> String {
        match self {
            Status::Pending => "Waiting".into(),
            Status::Running => format!("{}\u{2026}", kind.doing()),
            Status::Ok => kind.done_title().into(),
            Status::OkRestart => format!("{}; restart Windows to finish", kind.done_title()),
            Status::Skipped(why) => format!("Skipped: {why}"),
            Status::Cancelled => "Cancelled".into(),
            Status::Failed(why) => format!("Failed: {why}"),
            Status::NotStarted(why) => format!("Not started: {why}"),
            Status::Manual(url) => format!("To install by hand: {url}"),
        }
    }
}

/// One package of a series.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub package: Found,
    /// Its catalog declares that installing it needs administrator rights.
    pub admin: bool,
    pub status: Status,
    /// The installer's log, once it ran.
    pub log: Option<PathBuf>,
    /// The version and scope its bundle asked for.
    pub pin: Pin,
}

impl Item {
    /// A package waiting for its turn.
    pub fn new(package: Found, admin: bool) -> Self {
        Item { package, admin, status: Status::Pending, log: None, pin: Pin::default() }
    }

    /// Moves to `next` when the status allows it; returns whether it did.
    pub fn set(&mut self, next: Status) -> bool {
        let allowed = self.status.can_become(&next);
        if allowed {
            self.status = next;
        }
        allowed
    }
}

/// The order a series runs in: the packages that need administrator rights
/// first (so every UAC prompt comes at the start), then the order given.
pub fn order(mut items: Vec<Item>) -> Vec<Item> {
    // A stable sort keeps the order given within each group.
    items.sort_by_key(|i| !i.admin);
    items
}

/// The summary of a finished series of `kind`, one line per class of
/// outcome (one sentence for a single package).
pub fn summary(items: &[Item], kind: Kind) -> Vec<String> {
    let (done, title) = (kind.done(), kind.done_title());
    if let [one] = items {
        let name = &one.package.name;
        return vec![match &one.status {
            Status::Pending | Status::Running => format!("{name} is not done yet."),
            Status::Ok => format!("{name} was {done}."),
            Status::OkRestart => format!("{name} was {done}; restart Windows to finish."),
            Status::Skipped(why) => format!("{name} was skipped: {why}."),
            Status::Cancelled => format!("{name} was cancelled."),
            Status::Failed(why) => format!("{name} failed: {why}."),
            Status::NotStarted(why) => format!("{name} was not started: {why}."),
            Status::Manual(url) => format!("{name} is to be installed by hand: {url}"),
        }];
    }
    if items.len() > 1 && items.iter().all(|i| i.status == Status::Ok) {
        return vec![format!("All {} packages were {done}.", items.len())];
    }
    let names = |pick: fn(&Status) -> bool| -> Vec<String> { items.iter().filter(|i| pick(&i.status)).map(|i| i.package.name.clone()).collect() };
    let reasons = |pick: fn(&Status) -> Option<&String>| -> Vec<String> {
        items.iter().filter_map(|i| pick(&i.status).map(|why| format!("{} ({why})", i.package.name))).collect()
    };
    let classes = [
        (title.to_string(), join_names(&names(|s| *s == Status::Ok))),
        (format!("{title}, restart Windows to finish"), join_names(&names(|s| *s == Status::OkRestart))),
        ("Skipped".to_string(), reasons(|s| if let Status::Skipped(w) = s { Some(w) } else { None }).join("; ")),
        ("Cancelled".to_string(), join_names(&names(|s| *s == Status::Cancelled))),
        ("Failed".to_string(), reasons(|s| if let Status::Failed(w) = s { Some(w) } else { None }).join("; ")),
        ("Not started".to_string(), reasons(|s| if let Status::NotStarted(w) = s { Some(w) } else { None }).join("; ")),
        ("To install by hand".to_string(), reasons(|s| if let Status::Manual(u) = s { Some(u) } else { None }).join("; ")),
    ];
    classes.into_iter().filter(|(_, list)| !list.is_empty()).map(|(class, list)| format!("{class}: {list}.")).collect()
}

/// The packages "Retry the failed ones" installs again: those that failed
/// or were never started.
pub fn failed(items: &[Item]) -> Vec<Found> {
    items.iter().filter(|i| matches!(i.status, Status::Failed(_) | Status::NotStarted(_))).map(|i| i.package.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, status: Status) -> Item {
        let package = Found { name: name.into(), id: format!("Vendor.{name}"), version: "1.0".into(), catalog: "winget".into(), installed: false };
        Item { status, ..Item::new(package, false) }
    }

    fn admin(name: &str) -> Item {
        Item { admin: true, ..item(name, Status::Pending) }
    }

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(|i| i.package.name.as_str()).collect()
    }

    fn ends() -> Vec<Status> {
        vec![Status::Ok, Status::OkRestart, Status::Skipped("x".into()), Status::Cancelled, Status::Failed("x".into()), Status::NotStarted("x".into())]
    }

    #[test]
    fn a_package_moves_from_pending_to_running_to_one_outcome() {
        assert!(Status::Pending.can_become(&Status::Running));
        for end in ends() {
            assert!(Status::Running.can_become(&end), "running → {end:?}");
            assert!(end.finished(), "{end:?} is final");
            for next in ends().into_iter().chain([Status::Pending, Status::Running]) {
                assert!(!end.can_become(&next), "{end:?} → {next:?}");
            }
        }
        assert!(!Status::Pending.finished() && !Status::Running.finished());
    }

    #[test]
    fn a_package_the_series_never_reaches_is_not_started_and_nothing_else() {
        assert!(Status::Pending.can_become(&Status::NotStarted("stopped".into())));
        for end in ends().into_iter().filter(|e| !matches!(e, Status::NotStarted(_))) {
            assert!(!Status::Pending.can_become(&end), "pending → {end:?}");
        }
        assert!(!Status::Running.can_become(&Status::Pending));
    }

    #[test]
    fn a_manual_package_never_runs() {
        let manual = Status::Manual("https://example.com".into());
        assert!(manual.finished());
        for next in ends().into_iter().chain([Status::Pending, Status::Running]) {
            assert!(!manual.can_become(&next), "manual → {next:?}");
            assert!(!next.can_become(&manual), "{next:?} → manual");
        }
    }

    #[test]
    fn setting_a_status_the_machine_does_not_allow_changes_nothing() {
        let mut i = item("A", Status::Pending);
        assert!(!i.set(Status::Ok));
        assert_eq!(i.status, Status::Pending);
        assert!(i.set(Status::Running) && i.set(Status::Cancelled));
        assert!(!i.set(Status::Ok));
        assert_eq!(i.status, Status::Cancelled);
    }

    #[test]
    fn packages_needing_administrator_rights_run_first_then_the_order_given() {
        let items = vec![item("A", Status::Pending), admin("B"), item("C", Status::Pending), admin("D"), item("E", Status::Pending)];
        assert_eq!(names(&order(items)), ["B", "D", "A", "C", "E"]);
        assert_eq!(names(&order(vec![item("A", Status::Pending), item("B", Status::Pending)])), ["A", "B"]);
    }

    #[test]
    fn a_series_of_updates_speaks_of_updating() {
        let update = Kind::Update;
        assert_eq!(Status::Running.text(update), "Updating\u{2026}");
        assert_eq!(Status::Ok.text(update), "Updated");
        assert_eq!(Status::OkRestart.text(update), "Updated; restart Windows to finish");
        assert_eq!(Status::Failed("exit code 1603".into()).text(update), "Failed: exit code 1603");
        assert_eq!(summary(&[item("VLC", Status::Ok)], update), ["VLC was updated."]);
        let all: Vec<Item> = ["A", "B", "C"].iter().map(|n| item(n, Status::Ok)).collect();
        assert_eq!(summary(&all, update), ["All 3 packages were updated."]);
        let mixed = vec![item("A", Status::Ok), item("B", Status::Failed("exit code 1603".into())), item("C", Status::OkRestart)];
        assert_eq!(summary(&mixed, update), ["Updated: A.", "Updated, restart Windows to finish: C.", "Failed: B (exit code 1603)."]);
        assert_eq!(Status::Ok.text(Kind::Install), "Installed", "installs keep their words");
    }

    #[test]
    fn one_package_is_summed_up_in_one_sentence() {
        let one = |status| summary(&[item("VLC", status)], Kind::Install);
        assert_eq!(one(Status::Ok), ["VLC was installed."]);
        assert_eq!(one(Status::OkRestart), ["VLC was installed; restart Windows to finish."]);
        assert_eq!(one(Status::Skipped("already installed".into())), ["VLC was skipped: already installed."]);
        assert_eq!(one(Status::Cancelled), ["VLC was cancelled."]);
        assert_eq!(one(Status::Failed("exit code 1603".into())), ["VLC failed: exit code 1603."]);
        assert_eq!(one(Status::NotStarted("the series was stopped".into())), ["VLC was not started: the series was stopped."]);
        assert_eq!(one(Status::Manual("https://videolan.org".into())), ["VLC is to be installed by hand: https://videolan.org"]);
    }

    #[test]
    fn many_packages_all_installed_are_counted() {
        let items = [item("A", Status::Ok), item("B", Status::Ok), item("C", Status::Ok)];
        assert_eq!(summary(&items, Kind::Install), ["All 3 packages were installed."]);
    }

    #[test]
    fn a_mixed_series_lists_each_class_with_its_reasons() {
        let items = [
            item("A", Status::Ok),
            item("B", Status::Failed("exit code 1603".into())),
            item("C", Status::Ok),
            item("D", Status::OkRestart),
            item("E", Status::Skipped("already installed".into())),
            item("F", Status::Cancelled),
            item("G", Status::Failed("winget code 0x8A150102".into())),
            item("H", Status::NotStarted("the series was stopped".into())),
            item("I", Status::Manual("https://example.com/i".into())),
        ];
        assert_eq!(
            summary(&items, Kind::Install),
            [
                "Installed: A and C.",
                "Installed, restart Windows to finish: D.",
                "Skipped: E (already installed).",
                "Cancelled: F.",
                "Failed: B (exit code 1603); G (winget code 0x8A150102).",
                "Not started: H (the series was stopped).",
                "To install by hand: I (https://example.com/i).",
            ]
        );
    }

    #[test]
    fn a_series_needing_a_restart_says_so_even_when_all_were_installed() {
        let items = [item("A", Status::Ok), item("B", Status::OkRestart), item("C", Status::OkRestart)];
        assert_eq!(summary(&items, Kind::Install), ["Installed: A.", "Installed, restart Windows to finish: B and C."]);
    }

    #[test]
    fn manual_packages_are_listed_with_their_link_after_the_others() {
        let items = [item("A", Status::Ok), item("B", Status::Ok), item("Tool", Status::Manual("https://tool.example".into()))];
        assert_eq!(summary(&items, Kind::Install), ["Installed: A and B.", "To install by hand: Tool (https://tool.example)."]);
    }

    #[test]
    fn retrying_takes_the_failed_and_the_not_started_packages_in_order() {
        let items = [
            item("A", Status::Ok),
            item("B", Status::Failed("x".into())),
            item("C", Status::Cancelled),
            item("D", Status::NotStarted("x".into())),
            item("E", Status::Skipped("x".into())),
        ];
        assert_eq!(failed(&items).iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["B", "D"]);
    }
}
