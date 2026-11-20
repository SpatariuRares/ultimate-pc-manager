//! A pretend add-on for the tests: it shows that adding an add-on needs only
//! its own module and one line in the registry (see `test_registry`). It
//! implements just what every add-on must, so the defaults of every
//! optional hook (icon included) are used.

use crate::addons::{Addon, Consumption, Entry, Requirements, Tier};
use crate::platform::PageContext;

/// The pretend add-on: only the required methods.
#[derive(Default)]
pub struct Fake;

impl Addon for Fake {
    fn id(&self) -> &'static str {
        "fake"
    }
    fn name(&self) -> &'static str {
        "Fake"
    }
    fn description(&self) -> &'static str {
        "Pretends to be an add-on."
    }
    fn requirements(&self) -> Requirements {
        Requirements::default()
    }
    fn consumption(&self) -> Consumption {
        Consumption { cpu: "None", network: "None", disk: "None", when: "Never" }
    }
    fn tier(&self) -> Tier {
        Tier::Free
    }
    fn ui(&mut self, ui: &mut egui::Ui, _: &mut PageContext) {
        ui.label("Fake page");
    }
}

/// The real registry plus the fake add-on: the one line a new add-on adds.
pub fn test_registry() -> Vec<Entry> {
    let mut entries = crate::addons::registry();
    entries.push(Entry::Addon(Box::<Fake>::default()));
    entries
}
