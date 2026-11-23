//! System Info: the machine's facts in Design A cards, under a status band.

use egui::Ui;

use crate::app::{health_colour, status_parts, ProviderState};
use crate::core_page::{CorePage, PageCtx};
use crate::platform::Action;
use crate::theme;
use crate::widgets::parts::{self, Mark, StatusBand};
use crate::widgets::{cpu_brand, fmt_bytes, fmt_uptime, local_ip};

/// System Info keeps no state: every fact comes from the snapshot, plus on
/// Linux the facts read from `/proc`, `/sys` and `/etc` ([`crate::linux_facts`]).
/// `extra` stands in for those facts in tests.
#[derive(Default)]
pub struct SystemInfoPage {
    extra: Option<Vec<(&'static str, String)>>,
}

/// The system's own facts: Linux reads them, other systems add nothing.
fn system_extra() -> Vec<(&'static str, String)> {
    #[cfg(target_os = "linux")]
    return crate::linux_facts::system_fields();
    #[cfg(any(windows, target_os = "macos"))]
    return Vec::new();
}

/// `fields` with each of `extra` replacing the field of its name, else appended.
pub(crate) fn merge_fields(mut fields: Vec<(&'static str, String)>, extra: Vec<(&'static str, String)>) -> Vec<(&'static str, String)> {
    for (k, v) in extra {
        match fields.iter_mut().find(|(n, _)| *n == k) {
            Some(slot) => slot.1 = v,
            None => fields.push((k, v)),
        }
    }
    fields
}

impl CorePage for SystemInfoPage {
    fn title(&self) -> Option<&'static str> {
        Some("System Info")
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, _actions: &mut Vec<Action>) {
        let t = theme::of_ui(ui);
        let s = ctx.snapshot;
        let computer = sysinfo::System::host_name().unwrap_or_default();
        let os = sysinfo::System::long_os_version().unwrap_or_default();
        let base = vec![
            ("Computer name", computer.clone()),
            ("Operating system", os.clone()),
            ("Version", sysinfo::System::os_version().unwrap_or_default()),
            ("Kernel", sysinfo::System::kernel_version().unwrap_or_default()),
            ("Architecture", sysinfo::System::cpu_arch()),
            ("Processor", cpu_brand()),
            ("Cores", sysinfo::System::physical_core_count().map(|c| c.to_string()).unwrap_or_default()),
            ("Logical processors", s.cpu_per_core.len().to_string()),
            ("Speed", format!("{:.2} GHz", s.cpu_freq_mhz as f64 / 1000.0)),
            ("Installed memory", fmt_bytes(s.mem_total)),
            ("Page file", fmt_bytes(s.swap_total)),
            ("Drives", s.disks.len().to_string()),
            ("Network adapters", s.net.len().to_string()),
            ("Up time", fmt_uptime(s.uptime)),
        ];
        let fields = merge_fields(base, self.extra.clone().unwrap_or_else(system_extra));
        let computer = fields.iter().find(|(k, _)| *k == "Computer name").map(|(_, v)| v.clone()).unwrap_or(computer);
        let os = fields.iter().find(|(k, _)| *k == "Operating system").map(|(_, v)| v.clone()).unwrap_or(os);
        let paused = ctx.settings.interval_ms() == 0;
        let providers = ProviderState::of(s);
        let [state, procs, ip, generation] = status_parts(paused, providers, s.procs.len(), &local_ip(), s.seq);
        let sub = [procs, ip, generation, computer, os].into_iter().filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" \u{b7} ");
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            StatusBand::new(Mark::Dot(health_colour(&t, paused, providers)), &state).sub(&sub).show(ui, |_, _| {});
            for section in system_sections(&fields) {
                ui.add_space(14.0);
                parts::field_card(ui, section.group, section.title, &section.fields);
            }
        });
    }
}

/// A System Info card: its group, its title and its fields.
pub(crate) struct Section<'a> {
    pub group: &'a str,
    pub title: &'a str,
    pub fields: Vec<(&'a str, String)>,
}

/// Which card each System Info field sits in, as Design A groups them.
const SECTIONS: &[(&str, &str, &[&str])] = &[
    ("Software", "Operating System", &["Computer name", "Operating system", "Version", "Kernel", "Session", "Desktop", "Up time"]),
    ("Hardware", "Processor", &["Processor", "Architecture", "Cores", "Logical processors", "Speed"]),
    ("Hardware", "Board", &["Vendor", "Board", "BIOS"]),
    ("Hardware", "Memory", &["Installed memory", "Page file"]),
    ("Hardware", "Storage", &["Drives"]),
    ("Network", "Network", &["Network adapters"]),
];

/// The page's fields in their cards; a card with no field is left out.
pub(crate) fn system_sections<'a>(fields: &[(&'a str, String)]) -> Vec<Section<'a>> {
    SECTIONS
        .iter()
        .map(|(group, title, keys)| Section {
            group,
            title,
            fields: keys.iter().filter_map(|k| fields.iter().find(|(f, _)| f == k).cloned()).collect(),
        })
        .filter(|s| !s.fields.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_page::testing::draw;
    use crate::sampler::{NetRow, Snapshot};
    use crate::settings::Settings;

    fn snapshot() -> Snapshot {
        Snapshot {
            cpu_per_core: vec![0.0; 12],
            cpu_freq_mhz: 3_600,
            mem_total: 16 * 1024 * 1024 * 1024,
            swap_total: 4 * 1024 * 1024 * 1024,
            uptime: 90_061,
            net: vec![NetRow::default(), NetRow::default()],
            seq: 3,
            ..Default::default()
        }
    }

    #[test]
    fn it_draws_the_machine_facts_of_a_snapshot_without_the_app() {
        let drawn = draw(&mut SystemInfoPage::default(), &Settings::default(), &snapshot());
        let has = |t: &str| drawn.texts.iter().any(|s| s == t);
        for card in ["Operating System", "Processor", "Memory", "Storage", "Network"] {
            assert!(has(card), "card {card}: {:?}", drawn.texts);
        }
        assert!(has("Logical processors") && has("12"), "{:?}", drawn.texts);
        assert!(has("Speed") && has("3.60 GHz"), "{:?}", drawn.texts);
        assert!(has("Installed memory") && has("16.0 GB"), "{:?}", drawn.texts);
        assert!(has("Network adapters") && has("2"), "{:?}", drawn.texts);
        assert!(has("Up time"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_says_updates_are_paused_when_they_are() {
        let mut paused = Settings::default();
        paused.speed = crate::settings::SPEEDS.len() - 1;
        assert_eq!(paused.interval_ms(), 0, "the last speed pauses");
        let drawn = draw(&mut SystemInfoPage::default(), &paused, &snapshot());
        assert!(drawn.texts.iter().any(|t| t == "Updates paused"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_asks_nothing_of_the_app() {
        assert!(draw(&mut SystemInfoPage::default(), &Settings::default(), &snapshot()).actions.is_empty());
    }

    #[test]
    fn system_info_keeps_every_field_in_one_card() {
        let keys = [
            "Computer name", "Operating system", "Version", "Kernel", "Architecture", "Processor", "Cores",
            "Logical processors", "Speed", "Installed memory", "Page file", "Drives", "Network adapters", "Up time",
        ];
        let fields: Vec<(&str, String)> = keys.iter().map(|k| (*k, format!("value of {k}"))).collect();
        let sections = system_sections(&fields);
        let shown: Vec<(&str, String)> = sections.iter().flat_map(|s| s.fields.clone()).collect();
        assert_eq!(shown.len(), keys.len());
        for k in keys {
            assert_eq!(shown.iter().filter(|(f, v)| *f == k && *v == format!("value of {k}")).count(), 1, "{k}");
        }
        let titles: Vec<&str> = sections.iter().map(|s| s.title).collect();
        assert_eq!(titles, ["Operating System", "Processor", "Memory", "Storage", "Network"]);
    }

    fn linux_page(tree: &str, env: &'static [(&'static str, &'static str)]) -> SystemInfoPage {
        let lookup = move |k: &str| env.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string());
        let extra = crate::linux_facts::LinuxFacts::read(&crate::fs_root::fixture(tree), &lookup).fields();
        SystemInfoPage { extra: Some(extra) }
    }

    #[test]
    fn on_linux_the_page_shows_the_facts_read_through_the_root() {
        let mut page = linux_page("desktop", &[("XDG_SESSION_TYPE", "wayland"), ("XDG_CURRENT_DESKTOP", "GNOME")]);
        let drawn = draw(&mut page, &Settings::default(), &snapshot());
        let has = |t: &str| drawn.texts.iter().any(|s| s == t);
        for t in ["Board", "Ubuntu 24.04.1 LTS", "6.8.0-45-generic", "Wayland", "GNOME", "workstation", "1:01:01:01", "ASUS", "American Megatrends Inc. 3801"] {
            assert!(has(t), "{t}: {:?}", drawn.texts);
        }
        assert!(has("Logical processors"), "the snapshot's own fields stay");
    }

    #[test]
    fn on_linux_a_fact_the_system_does_not_expose_is_hidden() {
        let drawn = draw(&mut linux_page("arm-board", &[]), &Settings::default(), &snapshot());
        let has = |t: &str| drawn.texts.iter().any(|s| s == t);
        assert!(has("Raspberry Pi 4 Model B Rev 1.4"), "{:?}", drawn.texts);
        for t in ["Session", "Desktop", "Vendor", "BIOS"] {
            assert!(!has(t), "{t} must not be drawn: {:?}", drawn.texts);
        }
    }

    #[test]
    fn extra_facts_replace_the_field_of_their_name_and_add_the_others() {
        let merged = merge_fields(vec![("Kernel", "old".into()), ("Cores", "4".into())], vec![("Kernel", "new".into()), ("Board", "b".into())]);
        assert_eq!(merged, [("Kernel", "new".to_string()), ("Cores", "4".to_string()), ("Board", "b".to_string())]);
    }
}
