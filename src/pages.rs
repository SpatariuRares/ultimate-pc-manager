//! Leftover tests of the pages that moved to modules (System Info, Users and
//! the Settings hiding options); the helper `custom_cpu` is shared by the
//! graph tests.

#[cfg(test)]
pub(crate) mod tests {
    use crate::app::TaskApp;
    use crate::page_system::system_sections;
    use crate::page_users::{accounts, users_headline};
    use crate::theme;
    use crate::sampler::ProcRow;
    use crate::widgets::parts;
    use egui::Color32;

    fn proc(pid: u32, name: &str, user: &str, cpu: f32, mem: u64) -> ProcRow {
        ProcRow { pid, name: name.into(), user: user.into(), cpu, mem, ..Default::default() }
    }

    fn sample() -> Vec<ProcRow> {
        vec![
            proc(0, "System Idle Process", "", 90.0, 0),
            proc(4, "System", "", 0.4, 0),
            proc(35308, "chrome.exe", "spata", 0.2, 6_000_000_000),
            proc(16244, "Code.exe", "spata", 8.0, 4_300_000_000),
            proc(8944, "explorer.exe", "spata", 0.0, 470_000_000),
        ]
    }

    /// Settings whose CPU colour is the user's own.
    pub(crate) fn custom_cpu() -> (crate::settings::Settings, Color32) {
        let mut settings = crate::settings::Settings::default();
        settings.cpu_color = [0x12, 0x34, 0x56];
        (settings, Color32::from_rgb(0x12, 0x34, 0x56))
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

    #[test]
    fn users_are_the_accounts_running_processes() {
        let accounts = accounts(&sample());
        let names: Vec<&str> = accounts.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["System", "spata"]);
        let spata = &accounts[1];
        assert_eq!(spata.procs.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["chrome.exe", "Code.exe", "explorer.exe"], "by memory");
        assert!((spata.cpu - 8.2).abs() < 1e-4);
        assert_eq!(spata.mem, 10_770_000_000);
        assert_eq!(users_headline(&accounts), "2 accounts are running processes");
        assert_eq!(users_headline(&accounts[1..]), "Only spata is running processes");
        assert_eq!(users_headline(&[]), "No user sessions were reported");
    }

    #[test]
    fn both_hiding_settings_are_on_the_settings_page_not_in_process_view() {
        const LABELS: [&str; 2] = ["Show Task Manager in Reports", "Hide Microsoft Services and Processes"];
        let ctx = crate::test_ui::context(crate::theme::Mode::Dark);
        let mut app = TaskApp::headless(crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        app.page = crate::app::Page::Core(crate::addons::Core::Settings);
        let texts = crate::test_ui::texts(&crate::test_ui::run(&ctx, (1400.0, 2400.0), vec![], |ui| app.draw_page(ui, &mut Vec::new())));
        for label in LABELS {
            assert!(texts.iter().any(|t| t == label), "Settings shows {label}: {texts:?}");
        }
        assert!(texts.iter().any(|t| t.starts_with("Hides services and processes whose program file carries a verified Microsoft Windows signature")));
        let menu: Vec<&str> = crate::page_processes::process_view_items(&app.settings)
            .iter()
            .filter_map(|i| match i {
                parts::DropItem::Item { label, .. } => Some(*label),
                parts::DropItem::Separator => None,
            })
            .collect();
        assert!(LABELS.iter().all(|l| !menu.contains(l)), "Process View lists {menu:?}");
    }
}
