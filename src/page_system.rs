//! System Info: the machine's facts in Design A cards, under a status band.

use egui::Ui;

use crate::app::{status_parts, ProviderState};
use crate::core_page::{CorePage, PageCtx};
use crate::platform::Action;
use crate::theme;
use crate::widgets::parts::{self, Mark, StatusBand};
use crate::widgets::{cpu_brand, fmt_bytes, fmt_uptime, health_colour, local_ip};

/// System Info has no state of its own: every fact comes from the snapshot.
#[derive(Default)]
pub struct SystemInfoPage;

impl CorePage for SystemInfoPage {
    fn title(&self) -> Option<&'static str> {
        Some("System Info")
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, _actions: &mut Vec<Action>) {
        if true { return; } // RED-TEMP
        let t = theme::of_ui(ui);
        let s = ctx.snapshot;
        let computer = sysinfo::System::host_name().unwrap_or_default();
        let os = sysinfo::System::long_os_version().unwrap_or_default();
        let fields = vec![
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
    ("Software", "Operating System", &["Computer name", "Operating system", "Version", "Kernel", "Up time"]),
    ("Hardware", "Processor", &["Processor", "Architecture", "Cores", "Logical processors", "Speed"]),
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
        let drawn = draw(&mut SystemInfoPage, &Settings::default(), &snapshot());
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
        let paused = Settings { speed: crate::settings::SPEEDS.len() - 1, ..Default::default() };
        assert_eq!(paused.interval_ms(), 0, "the last speed pauses");
        let drawn = draw(&mut SystemInfoPage, &paused, &snapshot());
        assert!(drawn.texts.iter().any(|t| t == "Updates paused"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_asks_nothing_of_the_app() {
        assert!(draw(&mut SystemInfoPage, &Settings::default(), &snapshot()).actions.is_empty());
    }
}
