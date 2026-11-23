//! App history: the CPU time and disk traffic each program used since the
//! app opened. The page owns the totals and counts them on every sample.

use std::collections::BTreeMap;
use std::time::Instant;

use egui::{Align, Layout, Ui};
use egui_extras::{Column, TableBuilder};

use crate::addons::Core;
use crate::core_page::{page_title, CorePage, PageCtx, SampleSlot};
use crate::list_pages::{summary_band, GAP};
use crate::platform::Action;
use crate::widgets::fmt_bytes;
use crate::widgets::parts::{self, cell_text, column_heading, design_table, CellInk, Mark, TABLE_HEADER_HEIGHT, TABLE_ROW_HEIGHT};

pub struct AppHistoryPage {
    /// Per program name, (CPU seconds, disk bytes).
    totals: BTreeMap<String, (f64, f64)>,
    /// When the totals started: the moment the app was built, or the last
    /// "Delete usage history".
    since: Instant,
    /// When the previous sample arrived; a sample counts the time since it.
    last_sample_at: Option<Instant>,
}

impl Default for AppHistoryPage {
    fn default() -> Self {
        AppHistoryPage { totals: BTreeMap::new(), since: Instant::now(), last_sample_at: None }
    }
}

impl CorePage for AppHistoryPage {
    fn sample_slot(&self) -> SampleSlot {
        SampleSlot::AppHistory
    }

    /// Adds what every process used since the previous sample (counted
    /// from the second sample on, at most 10 seconds' worth).
    fn on_sample(&mut self, ctx: &PageCtx) {
        let s = ctx.snapshot;
        let now = Instant::now();
        if let Some(prev) = self.last_sample_at {
            let dt = now.duration_since(prev).as_secs_f64().min(10.0);
            let ncpu = s.cpu_per_core.len().max(1) as f64;
            for p in &s.procs {
                let e = self.totals.entry(p.name.clone()).or_default();
                e.0 += p.cpu as f64 / 100.0 * ncpu * dt;
                e.1 += (p.disk_read + p.disk_write) * dt;
            }
        }
        self.last_sample_at = Some(now);
    }

    fn draw(&mut self, ui: &mut Ui, _ctx: &PageCtx, _actions: &mut Vec<Action>) {
        ui.horizontal(|ui| {
            page_title(ui, Core::AppHistory.id(), "App history");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if parts::header_button(ui, "Delete usage history", true).clicked() {
                    self.totals.clear();
                    self.since = Instant::now();
                }
            });
        });
        ui.add_space(GAP);
        let mins = self.since.elapsed().as_secs() / 60;
        summary_band(ui, Mark::None, &history_headline(mins), "Counted only while this app is open.");
        let mut rows: Vec<(String, f64, f64)> = self.totals.iter().map(|(k, v)| (k.clone(), v.0, v.1)).collect();
        rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        rows.retain(|r| r.1 >= 0.01 || r.2 > 0.0);
        design_table(ui, |ui| {
            TableBuilder::new(ui)
                .resizable(true)
                .cell_layout(Layout::left_to_right(Align::Center))
                .column(Column::initial(300.0).clip(true))
                .column(Column::initial(140.0))
                .column(Column::remainder())
                .header(TABLE_HEADER_HEIGHT, |mut h| {
                    for t in ["Name", "CPU time", "Disk read + write"] {
                        h.col(|ui| column_heading(ui, t));
                    }
                })
                .body(|body| {
                    body.rows(TABLE_ROW_HEIGHT, rows.len(), |mut row| {
                        let (name, cpu, disk) = &rows[row.index()];
                        row.col(|ui| {
                            cell_text(ui, name, CellInk::Main, false);
                        });
                        row.col(|ui| {
                            let s = *cpu as u64;
                            cell_text(ui, &format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60), CellInk::Main, false);
                        });
                        row.col(|ui| {
                            cell_text(ui, &fmt_bytes(*disk as u64), CellInk::Soft, false);
                        });
                    });
                });
        });
    }
}

/// "Resource usage since 12 minutes ago".
fn history_headline(minutes: u64) -> String {
    let unit = if minutes == 1 { "minute" } else { "minutes" };
    format!("Resource usage since {minutes} {unit} ago")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::list_pages::testing::{click, has, texts, Env};
    use crate::sampler::{ProcRow, Snapshot};
    use crate::theme;

    const MIB: f64 = 1024.0 * 1024.0;

    fn busy(name: &str) -> Snapshot {
        let p = ProcRow { name: name.into(), cpu: 100.0, disk_read: 80.0 * MIB, disk_write: 20.0 * MIB, ..Default::default() };
        Snapshot { procs: vec![p], cpu_per_core: vec![0.0; 2], ..Default::default() }
    }

    /// Delivers `snapshot` to the page as a new sample.
    fn sample(page: &mut AppHistoryPage, env: &mut Env, snapshot: Snapshot) {
        env.snapshot = snapshot;
        page.on_sample(&env.ctx());
    }

    fn ui_ctx() -> egui::Context {
        crate::test_ui::context(theme::Mode::Dark)
    }

    #[test]
    fn app_history_takes_its_samples_with_the_app_history_totals() {
        assert_eq!(AppHistoryPage::default().sample_slot(), SampleSlot::AppHistory);
    }

    #[test]
    fn an_empty_app_history_says_since_when_and_lists_nothing() {
        let env = Env::new();
        let shown = texts(&ui_ctx(), &mut AppHistoryPage::default(), &env.ctx());
        for w in ["App history", "Resource usage since 0 minutes ago", "Counted only while this app is open.", "Name", "CPU time", "Disk read + write", "Delete usage history"] {
            assert!(has(&shown, w), "{w} in {shown:?}");
        }
    }

    #[test]
    fn the_first_sample_counts_nothing_and_the_next_counts_the_time_since_it() {
        let mut env = Env::new();
        let mut page = AppHistoryPage::default();
        let ui = ui_ctx();
        sample(&mut page, &mut env, busy("busy.exe"));
        assert!(!has(&texts(&ui, &mut page, &env.ctx()), "busy.exe"), "no time has passed yet");
        std::thread::sleep(std::time::Duration::from_millis(60));
        sample(&mut page, &mut env, busy("busy.exe"));
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(has(&shown, "busy.exe"), "{shown:?}");
        assert!(has(&shown, "0:00:00"), "two cores at 100% for a moment: {shown:?}");
        assert!(shown.iter().any(|t| t.trim().ends_with("MB")), "the disk traffic of that moment (MB): {shown:?}");
    }

    #[test]
    fn totals_add_up_across_samples_and_rank_by_cpu_time() {
        let mut env = Env::new();
        let mut page = AppHistoryPage::default();
        let ui = ui_ctx();
        let two = |a: f32, b: f32| {
            let row = |name: &str, cpu| ProcRow { name: name.into(), cpu, ..Default::default() };
            Snapshot { procs: vec![row("light.exe", a), row("heavy.exe", b)], cpu_per_core: vec![0.0], ..Default::default() }
        };
        sample(&mut page, &mut env, two(1.0, 90.0));
        std::thread::sleep(std::time::Duration::from_millis(40));
        sample(&mut page, &mut env, two(1.0, 90.0));
        std::thread::sleep(std::time::Duration::from_millis(40));
        sample(&mut page, &mut env, two(1.0, 90.0));
        let shown = texts(&ui, &mut page, &env.ctx());
        let at = |name: &str| shown.iter().position(|t| t == name);
        assert!(at("heavy.exe").is_some(), "{shown:?}");
        assert!(at("light.exe").is_none(), "1% for 80 ms is under 0.01 s of CPU: {shown:?}");
    }

    #[test]
    fn a_long_gap_between_samples_counts_at_most_ten_seconds() {
        let mut env = Env::new();
        let mut page = AppHistoryPage::default();
        let ui = ui_ctx();
        sample(&mut page, &mut env, busy("busy.exe"));
        // The app was suspended for a minute (a laptop asleep, say): the
        // next sample arrives 60 seconds after the previous one.
        page.last_sample_at = Instant::now().checked_sub(std::time::Duration::from_secs(60));
        sample(&mut page, &mut env, busy("busy.exe"));
        let shown = texts(&ui, &mut page, &env.ctx());
        // Two cores at 100% for 10 seconds is 20 seconds of CPU time.
        assert!(has(&shown, "0:00:20"), "{shown:?}");
    }

    #[test]
    fn delete_usage_history_clears_the_totals() {
        let mut env = Env::new();
        let mut page = AppHistoryPage::default();
        let ui = ui_ctx();
        sample(&mut page, &mut env, busy("busy.exe"));
        std::thread::sleep(std::time::Duration::from_millis(60));
        sample(&mut page, &mut env, busy("busy.exe"));
        assert!(has(&texts(&ui, &mut page, &env.ctx()), "busy.exe"));
        click(&ui, &mut page, &env.ctx(), "Delete usage history");
        let shown = texts(&ui, &mut page, &env.ctx());
        assert!(!has(&shown, "busy.exe"), "{shown:?}");
        assert!(has(&shown, "Resource usage since 0 minutes ago"));
    }

    #[test]
    fn the_history_band_says_since_when() {
        assert_eq!(history_headline(0), "Resource usage since 0 minutes ago");
        assert_eq!(history_headline(1), "Resource usage since 1 minute ago");
        assert_eq!(history_headline(12), "Resource usage since 12 minutes ago");
    }
}
