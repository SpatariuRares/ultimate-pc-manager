//! Users: the accounts running processes, with their processes.

use std::collections::BTreeMap;

use egui::{vec2, RichText, Sense, Ui};

use crate::core_page::{CorePage, PageCtx};
use crate::platform::Action;
use crate::sampler::ProcRow;
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, Mark, SidePanel, StatusBand};
use crate::widgets::{fmt_bytes, fmt_mb};

/// Users keeps its chosen and open accounts in egui's memory, so it has
/// no state of its own.
#[derive(Default)]
pub struct UsersPage;

impl CorePage for UsersPage {
    fn title(&self) -> Option<&'static str> {
        Some("Users")
    }

    fn draw(&mut self, ui: &mut Ui, ctx: &PageCtx, _actions: &mut Vec<Action>) {
        let t = ctx.settings.theme();
        let accounts = accounts(&ctx.snapshot.procs);
        let n: usize = accounts.iter().map(|a| a.procs.len()).sum();
        let cpu: f32 = accounts.iter().map(|a| a.cpu).sum();
        let mem: u64 = accounts.iter().map(|a| a.mem).sum();
        let sub = format!("{n} processes \u{b7} CPU {cpu:.1}% \u{b7} {} memory", fmt_bytes(mem));
        StatusBand::new(Mark::None, &users_headline(&accounts)).sub(&sub).show(ui, |_, _| {});
        ui.add_space(14.0);

        // The chosen account (its details beside the list) and the expanded
        // ones are kept in egui's memory.
        let chosen_id = egui::Id::new("users-chosen");
        let before: Option<String> = ui.data_mut(|d| d.get_persisted(chosen_id)).flatten();
        let chosen = parts::list_with_panel(
            ui,
            |ui, list_w| {
                let mut chosen = before;
                user_head(ui, &t, list_w);
                egui::ScrollArea::vertical().id_salt("users-list").auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for a in &accounts {
                        let open_id = egui::Id::new(("users-open", &a.name));
                        let mut open: bool = ui.data_mut(|d| d.get_persisted(open_id)).unwrap_or(false);
                        let on = chosen.as_deref() == Some(a.name.as_str());
                        if account_row(ui, &t, list_w - 12.0, a, on, open).clicked() {
                            open = !open || !on;
                            chosen = Some(a.name.clone());
                            ui.data_mut(|d| d.insert_persisted(open_id, open));
                        }
                        if open {
                            for p in a.procs.iter().take(200) {
                                member_row(ui, &t, list_w - 12.0, p);
                            }
                        }
                    }
                });
                chosen
            },
            |ui, chosen| {
                let item = chosen.as_ref().and_then(|c| accounts.iter().find(|a| &a.name == c));
                SidePanel::new("user-detail").show(ui, item, |ui, a| account_detail(ui, &t, a));
            },
        );
        ui.data_mut(|d| d.insert_persisted(chosen_id, chosen));
    }
}

/// An account running processes, with its processes by memory.
#[derive(Debug)]
pub(crate) struct Account {
    pub name: String,
    pub cpu: f32,
    pub mem: u64,
    pub procs: Vec<ProcRow>,
}

/// The accounts that run processes, by name; processes without a user
/// are "System".
pub(crate) fn accounts(procs: &[ProcRow]) -> Vec<Account> {
    let mut by: BTreeMap<String, Account> = BTreeMap::new();
    for p in procs {
        let name = if p.user.is_empty() { "System".to_string() } else { p.user.clone() };
        let a = by.entry(name.clone()).or_insert_with(|| Account { name, cpu: 0.0, mem: 0, procs: Vec::new() });
        a.cpu += p.cpu;
        a.mem += p.mem;
        a.procs.push(p.clone());
    }
    let mut all: Vec<Account> = by.into_values().collect();
    for a in &mut all {
        a.procs.sort_by(|x, y| y.mem.cmp(&x.mem));
    }
    all
}

/// The Users band's headline.
pub(crate) fn users_headline(accounts: &[Account]) -> String {
    match accounts {
        [] => "No user sessions were reported".to_string(),
        [one] => format!("Only {} is running processes", one.name),
        many => format!("{} accounts are running processes", many.len()),
    }
}

/// x of each Users column from the right edge: CPU, Memory, Processes.
const USER_COLUMNS: [f32; 3] = [200.0, 110.0, 12.0];

fn user_head(ui: &mut Ui, t: &Theme, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width - 12.0, 24.0), Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    p.text(egui::pos2(rect.left() + 12.0, y), egui::Align2::LEFT_CENTER, "User", t.fonts.label.clone(), t.label);
    for (label, x) in ["CPU", "Memory", "Processes"].iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, *label, t.fonts.label.clone(), t.label);
    }
    p.hline(rect.x_range(), rect.bottom(), egui::Stroke::new(1.0, t.line));
    ui.add_space(4.0);
}

/// An account's row: chevron, name and how many processes, its figures.
fn account_row(ui: &mut Ui, t: &Theme, width: f32, a: &Account, selected: bool, open: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 42.0), Sense::click());
    let p = ui.painter();
    parts::row_fill(p, rect, selected, resp.hovered());
    let ink = if selected { t.on_selected } else { t.text };
    let sub = parts::sub_colour(t, selected);
    let y = rect.center().y;
    // The chevron: down while the account's processes show, right otherwise.
    let c = egui::pos2(rect.left() + 17.0, y);
    crate::widgets::disclosure(p, c, open);
    p.text(egui::pos2(rect.left() + 30.0, rect.top() + 13.0), egui::Align2::LEFT_CENTER, &a.name, egui::FontId::new(13.0, theme::semibold()), ink);
    let count = match a.procs.len() {
        1 => "1 process".to_string(),
        n => format!("{n} processes"),
    };
    p.text(egui::pos2(rect.left() + 30.0, rect.top() + 30.0), egui::Align2::LEFT_CENTER, &count, egui::FontId::proportional(11.5), sub);
    let figures = [format!("{:.1}%", a.cpu), fmt_bytes(a.mem), a.procs.len().to_string()];
    for (f, x) in figures.iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, f, t.fonts.row.clone(), ink);
    }
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, &a.name));
    resp
}

/// One process under an open account: name and PID, CPU and memory.
fn member_row(ui: &mut Ui, t: &Theme, width: f32, proc: &ProcRow) {
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let p = ui.painter();
    parts::row_fill(p, rect, false, resp.hovered());
    let y = rect.center().y;
    let name = p.text(egui::pos2(rect.left() + 40.0, y), egui::Align2::LEFT_CENTER, &proc.name, t.fonts.row.clone(), t.text);
    p.text(egui::pos2(name.right() + 8.0, y), egui::Align2::LEFT_CENTER, format!("PID {}", proc.pid), egui::FontId::proportional(11.5), t.faint);
    let figures = [format!("{:.1}%", proc.cpu), fmt_mb(proc.mem)];
    for (f, x) in figures.iter().zip(USER_COLUMNS) {
        p.text(egui::pos2(rect.right() - x, y), egui::Align2::RIGHT_CENTER, f, t.fonts.row.clone(), t.soft);
    }
}

/// The chosen account beside the Users list: its name and its figures.
fn account_detail(ui: &mut Ui, t: &Theme, a: &Account) {
    ui.label(RichText::new(&a.name).font(t.fonts.figure.clone()).color(t.text));
    ui.add_space(8.0);
    ui.columns(3, |cols| {
        let figures = [
            ("CPU", format!("{:.1}%", a.cpu), t.graphs.cpu),
            ("Memory", fmt_bytes(a.mem), t.graphs.memory),
            ("Processes", a.procs.len().to_string(), t.text),
        ];
        for (ui, (label, value, ink)) in cols.iter_mut().zip(figures) {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(label).font(t.fonts.label.clone()).color(t.label));
            ui.label(RichText::new(value).font(t.fonts.figure_small.clone()).color(ink));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_page::testing::draw;
    use crate::sampler::Snapshot;
    use crate::settings::Settings;

    fn snapshot(rows: &[(&str, &str, f32, u64)]) -> Snapshot {
        let procs = rows
            .iter()
            .enumerate()
            .map(|(i, (name, user, cpu, mem))| ProcRow { pid: i as u32 + 1, name: (*name).into(), user: (*user).into(), cpu: *cpu, mem: *mem, ..Default::default() })
            .collect();
        Snapshot { procs, ..Default::default() }
    }

    #[test]
    fn it_lists_the_accounts_of_a_snapshot_without_the_app() {
        let snap = snapshot(&[("chrome.exe", "spata", 5.0, 2_000_000_000), ("Code.exe", "spata", 3.2, 1_000_000_000), ("svchost.exe", "", 1.0, 50_000_000)]);
        let drawn = draw(&mut UsersPage, &Settings::default(), &snap);
        let has = |t: &str| drawn.texts.iter().any(|s| s == t);
        assert!(has("2 accounts are running processes"), "{:?}", drawn.texts);
        assert!(has("3 processes \u{b7} CPU 9.2% \u{b7} 2.8 GB memory"), "{:?}", drawn.texts);
        assert!(has("spata") && has("System"), "{:?}", drawn.texts);
        assert!(has("2 processes") && has("1 process"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_says_so_when_no_session_is_reported() {
        let drawn = draw(&mut UsersPage, &Settings::default(), &snapshot(&[]));
        assert!(drawn.texts.iter().any(|t| t == "No user sessions were reported"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_names_the_only_account_running_processes() {
        let drawn = draw(&mut UsersPage, &Settings::default(), &snapshot(&[("a.exe", "spata", 1.0, 1)]));
        assert!(drawn.texts.iter().any(|t| t == "Only spata is running processes"), "{:?}", drawn.texts);
    }

    #[test]
    fn it_asks_nothing_of_the_app() {
        assert!(draw(&mut UsersPage, &Settings::default(), &snapshot(&[("a.exe", "spata", 1.0, 1)])).actions.is_empty());
    }
}
