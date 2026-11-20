//! Settings › Add-ons as data: what the page shows (count, rows, the
//! selected add-on's details, the status line), read from the host only.
//! Windowless, so the page's logic is tested without egui.

use super::{Addon, AddonHost, Entry};

/// An add-on's switch.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Switch {
    On,
    Off,
    /// Turned off during an operation that is not cut short: it turns off
    /// when that operation ends.
    Stopping,
}

impl Switch {
    /// The switch's words.
    pub fn label(self) -> &'static str {
        match self {
            Switch::On => "On",
            Switch::Off => "Off",
            Switch::Stopping => "Turns off when the current operation ends",
        }
    }

    /// Why the switch cannot be used now, if it cannot.
    pub fn hint(self) -> Option<&'static str> {
        match self {
            Switch::Stopping => Some("An install, uninstall or update is running and is not cut short."),
            Switch::On | Switch::Off => None,
        }
    }

    /// The short tag shown next to a locked switch in the list.
    pub fn badge(self) -> Option<&'static str> {
        match self {
            Switch::Stopping => Some("Turns off when done"),
            Switch::On | Switch::Off => None,
        }
    }
}

/// The kind of a requirement, which picks its icon.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Need {
    Admin,
    Internet,
    /// An external program (winget, PowerShell, ...).
    Program,
}

impl Need {
    /// The kind of a short requirement label ("Admin", "Internet", a program).
    pub fn of(need: &str) -> Need {
        match need {
            "Admin" => Need::Admin,
            "Internet" => Need::Internet,
            _ => Need::Program,
        }
    }
}

/// How heavy a declared use is, for the dot next to it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Intensity {
    None,
    Low,
    Medium,
    High,
}

/// The intensity of a declared use ("None", "Low", "Medium", "High while
/// running", "On update checks", ...): anything but none, medium or high
/// counts as low.
pub fn intensity(value: &str) -> Intensity {
    let v = value.to_ascii_lowercase();
    if v == "none" {
        Intensity::None
    } else if v.starts_with("high") {
        Intensity::High
    } else if v.starts_with("medium") {
        Intensity::Medium
    } else {
        Intensity::Low
    }
}

/// One add-on in the list.
pub struct Row {
    pub id: &'static str,
    pub name: &'static str,
    /// What it does, in one sentence (the line under its name).
    pub description: &'static str,
    pub switch: Switch,
    /// "Free" or "Paid".
    pub tier: &'static str,
    /// Short requirement labels: "Admin", "Internet", then each program.
    pub needs: Vec<&'static str>,
    /// Shown in the side panel.
    pub selected: bool,
}

impl Row {
    /// The row's requirement icons with their tooltips: admin, internet,
    /// then one icon for all the external programs.
    pub fn marks(&self) -> Vec<(Need, String)> {
        let mut marks: Vec<(Need, String)> =
            self.needs.iter().filter(|n| Need::of(n) != Need::Program).map(|n| (Need::of(n), need_hint(n))).collect();
        let programs: Vec<&str> = self.needs.iter().copied().filter(|n| Need::of(n) == Need::Program).collect();
        if !programs.is_empty() {
            marks.push((Need::Program, format!("Runs {}", programs.join(" and "))));
        }
        marks
    }
}

/// What the side panel lets the user do with the selected add-on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    TurnOn,
    TurnOff,
    /// Go to its page (only while it is on).
    OpenPage,
}

impl Act {
    /// The button's words.
    pub fn label(self) -> &'static str {
        match self {
            Act::TurnOn => "Turn on",
            Act::TurnOff => "Turn off",
            Act::OpenPage => "Open page",
        }
    }
}

/// The side panel: the selected add-on in full.
pub struct Detail {
    pub id: &'static str,
    pub name: &'static str,
    pub tier: &'static str,
    pub switch: Switch,
    pub description: &'static str,
    /// One sentence per requirement, or "Needs nothing extra".
    pub requirements: Vec<String>,
    /// The kind of each requirement, in the same order (empty when it
    /// needs nothing extra).
    pub needs: Vec<Need>,
    /// A warning under the requirements, if the add-on gives one.
    pub note: Option<&'static str>,
    /// CPU, Network, Disk and When, as declared by the add-on.
    pub consumption: [(&'static str, &'static str); 4],
    pub actions: Vec<Act>,
}

/// What Settings › Add-ons shows.
pub struct Overview {
    /// Add-ons that are on.
    pub on: usize,
    /// Every add-on in the registry.
    pub total: usize,
    /// The add-ons, in registry order.
    pub rows: Vec<Row>,
    /// The core pages' names: always on, no switch.
    pub core: Vec<&'static str>,
    /// The side panel (`None` only if there is no add-on at all).
    pub detail: Option<Detail>,
}

/// The side panel for `addon`.
fn detail(host: &AddonHost, addon: &dyn Addon) -> Detail {
    let short = needs(addon);
    let kinds = short.iter().map(|n| Need::of(n)).collect();
    let mut requirements: Vec<String> = short.into_iter().map(need_hint).collect();
    if requirements.is_empty() {
        requirements.push("Needs nothing extra".to_string());
    }
    let c = addon.consumption();
    let switch = switch(host, addon.id());
    let actions = match switch {
        Switch::On => vec![Act::TurnOff, Act::OpenPage],
        Switch::Off => vec![Act::TurnOn],
        Switch::Stopping => vec![],
    };
    Detail {
        id: addon.id(),
        name: addon.name(),
        tier: addon.tier().label(),
        switch,
        description: addon.description(),
        requirements,
        needs: kinds,
        note: addon.requirements().note,
        consumption: [("CPU", c.cpu), ("Network", c.network), ("Disk", c.disk), ("When", c.when)],
        actions,
    }
}

/// The switch of the add-on `id`.
fn switch(host: &AddonHost, id: &str) -> Switch {
    if host.is_stopping(id) {
        Switch::Stopping
    } else if host.is_enabled(id) {
        Switch::On
    } else {
        Switch::Off
    }
}

/// A requirement label ("Admin", "Internet", a program) in words.
pub fn need_hint(need: &str) -> String {
    match need {
        "Admin" => "Asks for administrator rights for some actions".to_string(),
        "Internet" => "Uses the internet".to_string(),
        program => format!("Runs {program}"),
    }
}

/// The short requirement labels of `addon`.
fn needs(addon: &dyn Addon) -> Vec<&'static str> {
    let r = addon.requirements();
    let mut needs = Vec::new();
    if r.admin {
        needs.push("Admin");
    }
    if r.network {
        needs.push("Internet");
    }
    needs.extend(r.programs.iter().copied());
    needs
}

impl Overview {
    /// The page for `selected` (an add-on id; anything else selects the
    /// first add-on).
    pub fn of(host: &AddonHost, selected: Option<&str>) -> Self {
        let chosen = host.addons().map(|(a, _)| a).find(|a| Some(a.id()) == selected).or_else(|| host.addons().map(|(a, _)| a).next());
        let chosen_id = chosen.map(|a| a.id());
        let rows = host
            .addons()
            .map(|(a, _)| Row {
                id: a.id(),
                name: a.name(),
                description: a.description(),
                switch: switch(host, a.id()),
                tier: a.tier().label(),
                needs: needs(a),
                selected: Some(a.id()) == chosen_id,
            })
            .collect();
        let core = host.entries().filter(|(e, _)| matches!(e, Entry::Core(_))).map(|(e, _)| e.name()).collect();
        Overview {
            on: host.addons().filter(|(_, on)| *on).count(),
            total: host.addons().count(),
            rows,
            core,
            detail: chosen.map(|a| detail(host, a)),
        }
    }

    /// The text of the band at the top.
    pub fn banner(&self) -> String {
        format!("{} of {} add-ons on", self.on, self.total)
    }

    /// The note in the band: who is waiting to turn off, else a reminder
    /// that the core is always on.
    pub fn note(&self) -> String {
        if self.rows.iter().any(|r| r.switch == Switch::Stopping) {
            self.status()
        } else {
            "Core pages are always on.".to_string()
        }
    }

    /// The status line at the bottom: add-ons waiting to turn off, if any.
    pub fn status(&self) -> String {
        let waiting: Vec<_> = self.rows.iter().filter(|r| r.switch == Switch::Stopping).map(|r| r.name).collect();
        match waiting.as_slice() {
            [] => "Changes apply at once and are saved.".to_string(),
            [one] => format!("{one} turns off when the current operation ends."),
            many => format!("{} turn off when their current operations end.", many.join(", ")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::{Consumption, Requirements, Tier};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn the_band_counts_the_add_ons_that_are_on_and_follows_the_switches() {
        let (mut host, _) = AddonHost::for_tests(&json!({ "drivers": { "enabled": false } }));
        assert_eq!(Overview::of(&host, None).banner(), "7 of 8 add-ons on");
        host.set_enabled("benchmarks", false);
        assert_eq!(Overview::of(&host, None).banner(), "6 of 8 add-ons on");
        host.set_enabled("drivers", true);
        assert_eq!(Overview::of(&host, None).banner(), "7 of 8 add-ons on");
    }

    #[test]
    fn each_row_shows_its_switch_tier_and_requirements_from_the_registry() {
        let (host, _) = AddonHost::for_tests(&json!({ "drivers": { "enabled": false } }));
        let view = Overview::of(&host, None);
        let names: Vec<_> = view.rows.iter().map(|r| r.name).collect();
        assert_eq!(names, ["Power & Freq", "Flight Recorder", "Connections", "Installed Apps", "Drivers", "Disk Space", "Benchmarks", "Package Manager"]);
        let drivers = view.rows.iter().find(|r| r.id == "drivers").unwrap();
        assert_eq!(drivers.switch, Switch::Off);
        assert_eq!(drivers.tier, "Free");
        assert_eq!(drivers.needs, ["Admin", "Internet", "PowerShell", "pnputil", "sc"]);
        let recorder = view.rows.iter().find(|r| r.id == "flight-recorder").unwrap();
        assert_eq!(recorder.switch, Switch::On);
        assert!(recorder.needs.is_empty());
        assert_eq!(drivers.description, "Installed drivers, their versions and available updates.");
    }

    #[test]
    fn the_side_panel_shows_the_selected_add_on_its_requirements_and_its_use() {
        // As after `--page Drivers` with Drivers off.
        let (host, _) = AddonHost::for_tests(&json!({ "drivers": { "enabled": false } }));
        let view = Overview::of(&host, Some("drivers"));
        let selected: Vec<_> = view.rows.iter().filter(|r| r.selected).map(|r| r.id).collect();
        assert_eq!(selected, ["drivers"]);
        let d = view.detail.unwrap();
        assert_eq!((d.id, d.name, d.tier, d.switch), ("drivers", "Drivers", "Free", Switch::Off));
        assert_eq!(d.description, "Installed drivers, their versions and available updates.");
        assert_eq!(d.requirements, ["Asks for administrator rights for some actions", "Uses the internet", "Runs PowerShell", "Runs pnputil", "Runs sc"]);
        assert_eq!(
            d.consumption,
            [("CPU", "Medium"), ("Network", "On update checks"), ("Disk", "On install"), ("When", "On open and on your actions")]
        );
    }

    #[test]
    fn an_add_on_that_needs_nothing_says_so() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        let d = Overview::of(&host, Some("connections")).detail.unwrap();
        assert_eq!(d.requirements, ["Needs nothing extra"]);
        assert_eq!(d.note, None);
    }

    #[test]
    fn flight_recorder_needs_nothing_extra_but_says_traces_take_disk_space() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        let d = Overview::of(&host, Some("flight-recorder")).detail.unwrap();
        assert_eq!(d.requirements, ["Needs nothing extra"]);
        assert_eq!(d.note, Some("Needs disk space for the traces you save (up to about 64 MB each)."));
    }

    #[test]
    fn disk_space_needs_nothing_extra_but_warns_it_reads_the_whole_disk() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        let d = Overview::of(&host, Some("disk-space")).detail.unwrap();
        assert_eq!(d.requirements, ["Needs nothing extra"]);
        assert_eq!(d.note, Some("Reads every folder of the drive you scan, and saves each finished scan to its History."));
    }

    #[test]
    fn without_a_valid_selection_the_panel_shows_the_first_add_on() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        for selected in [None, Some("summary"), Some("no-such-page")] {
            let view = Overview::of(&host, selected);
            assert_eq!(view.detail.unwrap().id, "power-freq", "{selected:?}");
            assert!(view.rows[0].selected);
        }
    }

    #[test]
    fn the_panel_offers_turning_on_an_add_on_that_is_off_and_opening_one_that_is_on() {
        let (mut host, _) = AddonHost::for_tests(&json!({ "drivers": { "enabled": false } }));
        assert_eq!(Overview::of(&host, Some("drivers")).detail.unwrap().actions, [Act::TurnOn]);
        host.set_enabled("drivers", true);
        assert_eq!(Overview::of(&host, Some("drivers")).detail.unwrap().actions, [Act::TurnOff, Act::OpenPage]);
    }

    /// An add-on whose uninterruptible operation runs while `busy` is set.
    struct Busy(Arc<AtomicBool>);

    impl Addon for Busy {
        fn id(&self) -> &'static str {
            "busy"
        }
        fn name(&self) -> &'static str {
            "Busy"
        }
        fn description(&self) -> &'static str {
            "Installs things."
        }
        fn requirements(&self) -> Requirements {
            Requirements::default()
        }
        fn consumption(&self) -> Consumption {
            Consumption { cpu: "Low", network: "None", disk: "None", when: "On your actions" }
        }
        fn tier(&self) -> Tier {
            Tier::Free
        }
        fn uninterruptible(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        fn ui(&mut self, _: &mut egui::Ui, _: &mut crate::platform::PageContext) {}
    }

    #[test]
    fn an_add_on_turned_off_during_an_install_says_it_turns_off_when_done() {
        let busy = Arc::new(AtomicBool::new(true));
        let entries = vec![Entry::Core(crate::addons::Core::Summary), Entry::Addon(Box::new(Busy(busy.clone())))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        assert_eq!(Overview::of(&host, None).status(), "Changes apply at once and are saved.");

        host.set_enabled("busy", false);
        let view = Overview::of(&host, None);
        assert_eq!(view.rows[0].switch, Switch::Stopping);
        assert_eq!(view.banner(), "0 of 1 add-ons on");
        assert!(view.detail.as_ref().unwrap().actions.is_empty());
        assert_eq!(view.status(), "Busy turns off when the current operation ends.");

        busy.store(false, Ordering::SeqCst);
        host.settle();
        let view = Overview::of(&host, None);
        assert_eq!(view.rows[0].switch, Switch::Off);
        assert_eq!(view.status(), "Changes apply at once and are saved.");
        assert_eq!(view.detail.unwrap().actions, [Act::TurnOn]);
    }

    #[test]
    fn switches_and_actions_have_their_words() {
        assert_eq!(Switch::On.label(), "On");
        assert_eq!(Switch::Off.label(), "Off");
        assert_eq!(Switch::Stopping.label(), "Turns off when the current operation ends");
        assert_eq!(Switch::Stopping.hint(), Some("An install, uninstall or update is running and is not cut short."));
        assert_eq!(Switch::On.hint(), None);
        assert_eq!([Act::TurnOn, Act::TurnOff, Act::OpenPage].map(Act::label), ["Turn on", "Turn off", "Open page"]);
    }

    #[test]
    fn programs_share_one_requirement_icon_and_the_panel_knows_each_kind() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        let view = Overview::of(&host, Some("installed-apps"));
        let row = view.rows.iter().find(|r| r.id == "installed-apps").unwrap();
        let marks = row.marks();
        let kinds: Vec<_> = marks.iter().map(|(k, _)| *k).collect();
        assert_eq!(kinds, [Need::Admin, Need::Internet, Need::Program]);
        let programs: Vec<_> = row.needs.iter().skip(2).copied().collect();
        assert_eq!(marks[2].1, format!("Runs {}", programs.join(" and ")));
        let d = view.detail.unwrap();
        assert_eq!(d.needs.len(), d.requirements.len());
        assert_eq!(d.needs[..2], [Need::Admin, Need::Internet]);
        assert!(Overview::of(&host, Some("flight-recorder")).detail.unwrap().needs.is_empty());
    }

    #[test]
    fn declared_use_reads_as_an_intensity() {
        assert_eq!(intensity("None"), Intensity::None);
        assert_eq!(intensity("Low"), Intensity::Low);
        assert_eq!(intensity("On update checks"), Intensity::Low);
        assert_eq!(intensity("Medium"), Intensity::Medium);
        assert_eq!(intensity("High while running"), Intensity::High);
    }

    #[test]
    fn the_band_note_names_who_waits_else_the_core() {
        let busy = Arc::new(AtomicBool::new(true));
        let entries = vec![Entry::Core(crate::addons::Core::Summary), Entry::Addon(Box::new(Busy(busy)))];
        let (mut host, _) = AddonHost::for_tests_with(entries, &json!({}));
        assert_eq!(Overview::of(&host, None).note(), "Core pages are always on.");
        host.set_enabled("busy", false);
        let view = Overview::of(&host, None);
        assert_eq!(view.note(), "Busy turns off when the current operation ends.");
        assert_eq!(view.rows[0].switch.badge(), Some("Turns off when done"));
        assert_eq!(Switch::On.badge(), None);
    }

    #[test]
    fn the_core_pages_are_listed_as_always_on() {
        let (host, _) = AddonHost::for_tests(&json!({}));
        assert_eq!(
            Overview::of(&host, None).core,
            ["Summary", "Performance", "Processes", "System Info", "App history", "Startup apps", "Users", "Services", "Settings", "Colors"]
        );
    }
}
