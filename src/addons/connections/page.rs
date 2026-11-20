//! The Connections page: the inventory line and the flat table
//! (Application, Protocol, State, Local endpoint, Remote, Exposure).

use std::collections::{HashMap, HashSet};

use egui::{Align, Color32, Layout, RichText, Sense, Ui, WidgetInfo, WidgetType};
use egui_extras::{Column, TableBuilder};

use super::groups::{self, Item, Key};
use super::a11y;
use super::actions;
use super::inventory::{self, Exposure, Inventory, Row, SYSTEM_IDLE_PID, SYSTEM_PID};
use super::scope::{self, LOCAL_IPC, LOCAL_IPC_NOTE, NO_MATCH};
use super::{Prefs, Scope, SortColumn};
use crate::platform::PageContext;
use crate::sampler::ProcRow;
use crate::theme::{self, Theme};
use crate::widgets::parts::{self, cell_text, design_table, sort_heading, CellInk, ScopeGroup, TABLE_HEADER_HEIGHT};
use crate::widgets::search_field;

/// The line under the page title "Connections".
pub const SUBTITLE: &str = "Connection Management";

/// Shown where Windows does not tell a process's name or user.
pub const UNKNOWN: &str = "unknown";

/// The process that owns a row, from the sampler's samples.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Owner {
    /// Process name ("unknown" when Windows does not tell it).
    pub name: String,
    /// Full path of the executable ("" when not known).
    pub exe: String,
    /// Account running it ("unknown" when Windows does not tell it).
    pub user: String,
}

/// The owner of `pid`: PID 0 and 4 are Windows itself.
pub fn owner(pid: u32, procs: &HashMap<u32, &ProcRow>) -> Owner {
    let known = |s: &str| if s.is_empty() { UNKNOWN.to_string() } else { s.to_string() };
    match procs.get(&pid) {
        Some(p) => Owner { name: known(&p.name), exe: p.exe.clone(), user: known(&p.user) },
        None => {
            let name = match pid {
                SYSTEM_IDLE_PID => "System Idle Process",
                SYSTEM_PID => "System",
                _ => UNKNOWN,
            };
            Owner { name: name.into(), exe: String::new(), user: UNKNOWN.into() }
        }
    }
}

/// Reads an executable's FileDescription ("Microsoft Edge"), if any.
type Describe = Box<dyn Fn(&str) -> Option<String>>;

/// Friendly application names, read once per executable path.
pub struct AppNames {
    describe: Describe,
    /// The description of each path read so far (None: it has none).
    cache: HashMap<String, Option<String>>,
}

impl Default for AppNames {
    /// Names from the files' version resources.
    fn default() -> Self {
        Self::new(crate::winsys::file_description)
    }
}

impl AppNames {
    /// Names read by `describe` (the system's, or a test's).
    pub fn new(describe: impl Fn(&str) -> Option<String> + 'static) -> Self {
        Self { describe: Box::new(describe), cache: HashMap::new() }
    }

    /// The friendly name of `owner`: its executable's description, else
    /// the executable's name.
    pub fn name(&mut self, owner: &Owner) -> String {
        if owner.exe.is_empty() {
            return owner.name.clone();
        }
        let describe = &self.describe;
        let description = self
            .cache
            .entry(owner.exe.clone())
            .or_insert_with(|| describe(&owner.exe).map(|d| d.trim().to_string()).filter(|d| !d.is_empty()));
        description.clone().unwrap_or_else(|| owner.name.clone())
    }
}

/// The Application column: the friendly name, followed for a Service Host
/// (svchost.exe) by the services it runs.
pub fn application(friendly: &str, owner: &Owner, services: Option<&Vec<String>>) -> String {
    match services {
        Some(list) if !list.is_empty() && owner.name.eq_ignore_ascii_case("svchost.exe") => {
            format!("{friendly} ({})", list.join(", "))
        }
        _ => friendly.to_string(),
    }
}

/// The texts of one table row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Cells {
    pub application: String,
    /// The line under the application: executable and "PID n".
    pub detail: String,
    pub protocol: &'static str,
    pub state: &'static str,
    pub local: String,
    pub remote: String,
    pub exposure: &'static str,
}

/// The cells of `row`, owned by `owner` and shown as `application`.
pub fn cells(row: &Row, owner: &Owner, application: &str) -> Cells {
    let pid = format!("PID {}", row.pid);
    Cells {
        application: application.to_string(),
        detail: if owner.exe.is_empty() { pid } else { format!("{} \u{b7} {pid}", owner.exe) },
        protocol: inventory::protocol_text(row),
        state: inventory::state_text(row),
        local: inventory::endpoint_text(&row.local),
        remote: inventory::remote_service_text(row),
        exposure: inventory::exposure(row).label(),
    }
}

/// The table's headers, in order.
pub const COLUMNS: [&str; 6] = ["Application", "Protocol", "State", "Local endpoint", "Remote / service", "Exposure"];

/// The room kept for the last column, Exposure ("All interfaces").
const EXPOSURE_WIDTH: f32 = 110.0;

/// The starting widths of the first five columns for a table `width` wide,
/// leaving `EXPOSURE_WIDTH` and the spacing for Exposure: Protocol and
/// State fixed, the rest shared out, Application the widest.
fn column_widths(width: f32) -> [f32; 5] {
    let (proto, state, spacing) = (60.0, 95.0, 6.0 * 8.0);
    let shared = (width - EXPOSURE_WIDTH - proto - state - spacing).max(330.0);
    [shared * 0.36, proto, state, shared * 0.30, shared * 0.34]
}

/// Draws the scopes with their counts, Local IPC disabled with its note,
/// and on the right Find (an addition).
pub fn draw_scopes(ui: &mut Ui, rows: &[Row], chosen: &mut Scope, find: &mut String) {
    let t = theme::of_ui(ui);
    ui.horizontal_wrapped(|ui| {
        let items: Vec<(Scope, &str, usize)> = Scope::ALL.iter().map(|s| (*s, s.label(), scope::count(*s, rows))).collect();
        let scopes = ScopeGroup::new(*chosen, &items).alert(Scope::Public).unavailable(LOCAL_IPC, LOCAL_IPC_NOTE, LOCAL_IPC_NOTE).show(ui);
        if let Some(s) = scopes.clicked {
            *chosen = s;
        }
        let spoken = a11y::scope_text(*chosen);
        scopes.group.widget_info(|| WidgetInfo::labeled(WidgetType::RadioGroup, true, &spoken));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new("Addition").small().color(t.faint));
            let field = search_field(ui, find, "Find application, address, port, state or PID", 300.0);
            ui.label(RichText::new("Find").font(t.fonts.label.clone()).color(t.label)).labelled_by(field.id);
        });
    });
}

/// The colour of `exposure` in the table and the panel: amber for all
/// interfaces, red for the Internet, blue for the local network, green for
/// loopback, grey when unknown.
pub fn exposure_color(exposure: Exposure, t: &Theme) -> Color32 {
    match exposure {
        Exposure::AllInterfaces => t.amber,
        Exposure::Internet => t.red,
        Exposure::LocalNetwork => t.link,
        Exposure::Loopback => t.green,
        _ => t.label,
    }
}

/// The ink of a row's state: green while established, the text colour
/// while listening, faint otherwise.
fn state_ink(state: &str) -> CellInk {
    match state {
        "Established" => CellInk::Good,
        "Listening" => CellInk::Main,
        _ => CellInk::Faint,
    }
}

/// The table's own state: the selected connection (kept across readings
/// while it exists), the groups expanded past their first rows and the
/// friendly application names read so far.
#[derive(Default)]
pub struct TableState {
    pub selected: Option<Key>,
    pub expanded: HashSet<Exposure>,
    pub names: AppNames,
    /// The process End process… asked to end, until the user confirms.
    pub ending: Option<actions::Ending>,
}

/// Draws the rows in the chosen scope (`prefs.scope`) that match Find
/// (the scopes and Find are drawn above, by [`draw_scopes`]), grouped and
/// sorted as `prefs` say; a header click sorts by that column (saved in
/// `prefs`).
pub fn draw(
    ui: &mut Ui,
    page: &mut PageContext,
    inventory: Option<&Inventory>,
    state: &mut TableState,
    prefs: &mut Prefs,
    find: &mut String,
) {
    let t = theme::of_ui(ui);
    let Some(inv) = inventory else {
        // Until the first reading, the area says what the header says.
        ui.label(RichText::new(inventory::status_line(inventory)).color(t.label));
        return;
    };
    let procs: HashMap<u32, &ProcRow> = page.sample.procs.iter().map(|p| (p.pid, p)).collect();
    // The selection follows its connection; it ends when the connection does.
    let current = state.selected.as_ref().and_then(|k| groups::find(&inv.rows, k));
    if current.is_none() {
        state.selected = None;
    }
    // The Application text of each process, for the cells, Find and the sort.
    let mut apps: HashMap<u32, String> = HashMap::new();
    for r in &inv.rows {
        apps.entry(r.pid).or_insert_with(|| {
            let own = owner(r.pid, &procs);
            application(&state.names.name(&own), &own, inv.services.get(&r.pid))
        });
    }
    let shown = scope::shown(&inv.rows, prefs.scope, find, |r| cells(r, &owner(r.pid, &procs), &apps[&r.pid]));
    if shown.is_empty() {
        ui.label(RichText::new(NO_MATCH).color(t.label));
        return;
    }
    let sub = t.faint;
    if prefs.group_by_exposure {
        // The note sits under the table.
        egui::TopBottomPanel::bottom("connections-grouping-note").frame(egui::Frame::NONE).show_separator_line(false).show_inside(ui, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Rows are grouped by how exposed each connection is.").small().color(sub));
        });
    }
    let arranged = groups::arrange(&inv.rows, shown, prefs.sort_by, prefs.sort_descending, prefs.group_by_exposure, |r| {
        apps[&r.pid].clone()
    });
    let items = groups::lines(&arranged, &state.expanded);
    let heights = items.iter().map(|i| match i {
        Item::Title(_) => 30.0,
        Item::Row(_) => 36.0,
        Item::More(..) => 22.0,
    });
    let table_clip = ui.clip_rect();
    let mut clicked = None;
    let mut sort_on = None;
    let mut expand = None;
    let (sort_by, descending) = (prefs.sort_by, prefs.sort_descending);
    let page_actions = &mut page.actions;
    let ending = &mut state.ending;
    // The widths fit the space left by the side panel, so that all six
    // columns, Exposure last, are visible without scrolling sideways.
    let [app_w, proto_w, state_w, local_w, remote_w] = column_widths(ui.available_width());
    design_table(ui, |ui| TableBuilder::new(ui)
        .id_salt("connections-table")
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(app_w).at_least(120.0).clip(true))
        .column(Column::initial(proto_w).at_least(50.0))
        .column(Column::initial(state_w).at_least(70.0).clip(true))
        .column(Column::initial(local_w).at_least(100.0).clip(true))
        .column(Column::initial(remote_w).at_least(100.0).clip(true))
        .column(Column::remainder().at_least(EXPOSURE_WIDTH).clip(true))
        .header(TABLE_HEADER_HEIGHT, |mut h| {
            for (column, name) in SortColumn::ALL.into_iter().zip(COLUMNS) {
                h.col(|ui| {
                    let sort = (sort_by == column).then_some(descending);
                    if sort_heading(ui, name, sort).on_hover_text("Click to sort (addition)").clicked() {
                        sort_on = Some(column);
                    }
                });
            }
        })
        .body(|body| {
            body.heterogeneous_rows(heights, |mut table_row| match items[table_row.index()] {
                Item::Title(g) => {
                    let group = &arranged[g];
                    let Some(exp) = group.exposure else { return };
                    table_row.col(|ui| {
                        // The title line runs across the row's cells.
                        let cell = ui.max_rect();
                        let mut painter = ui.painter().clone();
                        painter.set_clip_rect(egui::Rect::from_x_y_ranges(table_clip.x_range(), ui.clip_rect().y_range()));
                        let font = |s: egui::TextStyle| s.resolve(ui.style());
                        let fmt = |font_id, color| egui::TextFormat { font_id, color, ..Default::default() };
                        let mut job = egui::text::LayoutJob::default();
                        job.append(exp.label(), 0.0, fmt(egui::FontId::new(13.0, theme::semibold()), t.text));
                        job.append(&group.rows.len().to_string(), 8.0, fmt(font(egui::TextStyle::Body), sub));
                        job.append(&groups::hint(exp, &inv.rows, &group.rows), 8.0, fmt(font(egui::TextStyle::Small), sub));
                        let galley = ui.fonts_mut(|f| f.layout_job(job));
                        let pos = egui::pos2(cell.left(), cell.center().y - galley.size().y / 2.0);
                        painter.galley(pos, galley, sub);
                    });
                }
                Item::More(g, hidden) => {
                    let group = &arranged[g];
                    table_row.col(|ui| {
                        if parts::link_button(ui, &groups::more_text(hidden, group.rows.len())).clicked() {
                            expand = group.exposure;
                        }
                    });
                }
                Item::Row(i) => {
                    let row = &inv.rows[i];
                    let own = &owner(row.pid, &procs);
                    let c = cells(row, own, &apps[&row.pid]);
                    let on = current == Some(i);
                    table_row.set_selected(on);
                    table_row.col(|ui| {
                        ui.vertical(|ui| {
                            ui.spacing_mut().item_spacing.y = 1.0;
                            cell_text(ui, &c.application, CellInk::Main, on);
                            ui.add(egui::Label::new(RichText::new(&c.detail).size(11.5).color(parts::sub_colour(&t, on))).truncate());
                        });
                    });
                    table_row.col(|ui| {
                        cell_text(ui, c.protocol, CellInk::Soft, on);
                    });
                    table_row.col(|ui| {
                        cell_text(ui, c.state, state_ink(c.state), on);
                    });
                    for text in [&c.local, &c.remote] {
                        table_row.col(|ui| {
                            cell_text(ui, text, CellInk::Main, on);
                        });
                    }
                    table_row.col(|ui| {
                        // The exposure keeps its colour on the selection too.
                        let ink = exposure_color(inventory::exposure(row), &t);
                        ui.add(egui::Label::new(RichText::new(c.exposure).font(t.fonts.row.clone()).color(ink)).truncate());
                    });
                    let r = table_row.response();
                    let spoken = a11y::row_text(row, &c, own);
                    r.widget_info(|| WidgetInfo::selected(WidgetType::SelectableLabel, true, current == Some(i), &spoken));
                    if r.clicked() || r.secondary_clicked() {
                        clicked = Some(groups::key(row));
                    }
                    r.context_menu(|ui| {
                        ui.label(RichText::new("Addition").small().color(sub));
                        actions::draw(ui, &actions::target(row, &c, own), true, page_actions, ending);
                    });
                }
            });
        }));
    if clicked.is_some() {
        state.selected = clicked;
    }
    if let Some(column) = sort_on {
        prefs.sort_on(column);
    }
    if let Some(e) = expand {
        state.expanded.insert(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{Proto, TcpState};

    #[test]
    fn the_page_subtitle_is_connection_management() {
        assert_eq!(SUBTITLE, "Connection Management");
    }

    fn procs() -> Vec<ProcRow> {
        vec![
            ProcRow { pid: 1868, name: "svchost.exe".into(), exe: r"C:\Windows\System32\svchost.exe".into(), user: "NETWORK SERVICE".into(), ..Default::default() },
            ProcRow { pid: 777, name: "locked.exe".into(), ..Default::default() },
        ]
    }

    fn by_pid(p: &[ProcRow]) -> HashMap<u32, &ProcRow> {
        p.iter().map(|r| (r.pid, r)).collect()
    }

    #[test]
    fn the_owner_comes_from_the_samples_and_unknown_fills_the_gaps() {
        let p = procs();
        let m = by_pid(&p);
        assert_eq!(
            owner(1868, &m),
            Owner { name: "svchost.exe".into(), exe: r"C:\Windows\System32\svchost.exe".into(), user: "NETWORK SERVICE".into() }
        );
        assert_eq!(owner(777, &m), Owner { name: "locked.exe".into(), exe: String::new(), user: UNKNOWN.into() });
        assert_eq!(owner(4242, &m), Owner { name: UNKNOWN.into(), exe: String::new(), user: UNKNOWN.into() });
        assert_eq!(owner(4, &m).name, "System");
        assert_eq!(owner(0, &m).name, "System Idle Process");
    }

    /// Draws the page once over `rows`.
    fn draw_over(rows: Vec<Row>, state: &mut TableState) {
        let inv = inventory::assemble([Ok(rows), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
        let (settings, sample) = (crate::settings::Settings::default(), crate::sampler::Snapshot::default());
        let (mut prefs, mut find) = (Prefs::default(), String::new());
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                draw(ui, &mut PageContext::new(&settings, &sample), Some(&inv), state, &mut prefs, &mut find)
            });
        });
    }

    #[test]
    fn the_selected_connection_stays_selected_while_it_exists() {
        let conn = Row {
            proto: Proto::Tcp,
            local: "192.168.1.5:50000".parse().unwrap(),
            remote: Some("160.79.104.10:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 20,
        };
        let other = Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 4 };
        let mut state = TableState { selected: Some(groups::key(&conn)), ..Default::default() };
        // A new reading: another row first, the connection now closing.
        draw_over(vec![other.clone(), Row { state: Some(TcpState::CloseWait), ..conn.clone() }], &mut state);
        assert_eq!(state.selected, Some(groups::key(&conn)));
        // The connection is gone.
        draw_over(vec![other], &mut state);
        assert_eq!(state.selected, None);
    }

    /// Names read from a fixed set of descriptions, counting the reads.
    fn names_from(descriptions: &[(&'static str, &'static str)]) -> (AppNames, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = reads.clone();
        let table: HashMap<&str, &str> = descriptions.iter().copied().collect();
        let names = AppNames::new(move |exe| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            table.get(exe).map(|d| d.to_string())
        });
        (names, reads)
    }

    #[test]
    fn the_application_is_the_file_description_read_once_per_path() {
        let (mut names, reads) = names_from(&[(r"C:\Windows\System32\svchost.exe", "Host Process for Windows Services")]);
        let p = procs();
        let svchost = owner(1868, &by_pid(&p));
        assert_eq!(names.name(&svchost), "Host Process for Windows Services");
        assert_eq!(names.name(&svchost), "Host Process for Windows Services");
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn the_executable_name_stands_in_for_a_missing_description() {
        let (mut names, reads) = names_from(&[(r"C:\Tools\blank.exe", "  ")]);
        let blank = Owner { name: "blank.exe".into(), exe: r"C:\Tools\blank.exe".into(), user: UNKNOWN.into() };
        let pathless = Owner { name: "locked.exe".into(), exe: String::new(), user: UNKNOWN.into() };
        assert_eq!(names.name(&blank), "blank.exe");
        assert_eq!(names.name(&blank), "blank.exe");
        assert_eq!(names.name(&pathless), "locked.exe");
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1, "a path is read once, an empty one never");
    }

    #[test]
    fn a_service_host_names_the_services_it_runs() {
        let svchost = Owner { name: "svchost.exe".into(), exe: r"C:\Windows\System32\svchost.exe".into(), user: UNKNOWN.into() };
        let services = vec!["RpcEptMapper".to_string(), "RpcSs".to_string()];
        assert_eq!(application("Service Host", &svchost, Some(&services)), "Service Host (RpcEptMapper, RpcSs)");
        assert_eq!(application("Service Host", &svchost, None), "Service Host");
        let vmms = Owner { name: "vmms.exe".into(), exe: r"C:\Windows\System32\vmms.exe".into(), user: UNKNOWN.into() };
        assert_eq!(application("Virtual Machine Management Service", &vmms, Some(&vec!["vmms".to_string()])), "Virtual Machine Management Service");
    }

    #[test]
    fn the_columns_leave_room_for_exposure() {
        for width in [700.0, 760.0, 1200.0] {
            let w = column_widths(width);
            let used: f32 = w.iter().sum::<f32>() + 6.0 * 8.0;
            assert!(used + EXPOSURE_WIDTH <= width + 0.5, "width {width}: {used} used");
        }
    }

    #[test]
    fn a_tcp_row_fills_the_six_columns() {
        let p = procs();
        let row = Row {
            proto: Proto::Tcp,
            local: "[2a02::5]:50123".parse().unwrap(),
            remote: Some("[2a00:1450::200e]:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 1868,
        };
        let c = cells(&row, &owner(1868, &by_pid(&p)), "Service Host (CryptSvc)");
        assert_eq!(
            c,
            Cells {
                application: "Service Host (CryptSvc)".into(),
                detail: "C:\\Windows\\System32\\svchost.exe \u{b7} PID 1868".into(),
                protocol: "TCP6",
                state: "Established",
                local: "[2a02::5]:50123".into(),
                remote: "[2a00:1450::200e]:443 \u{b7} HTTPS".into(),
                exposure: "Internet",
            }
        );
    }

    #[test]
    fn a_listener_row_shows_its_port_and_service() {
        let p = procs();
        let row = Row { proto: Proto::Tcp, local: "[::]:445".parse().unwrap(), remote: None, state: Some(TcpState::Listening), pid: 4 };
        assert_eq!(cells(&row, &owner(4, &by_pid(&p)), "System").remote, "445 \u{b7} SMB");
    }

    #[test]
    fn a_udp_row_is_a_datagram_with_a_dash_for_remote() {
        let p = procs();
        let row = Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 4242 };
        let c = cells(&row, &owner(4242, &by_pid(&p)), UNKNOWN);
        assert_eq!((c.protocol, c.state, c.remote.as_str()), ("UDP4", "Datagram", "\u{2014}"));
        assert_eq!((c.application.as_str(), c.detail.as_str()), (UNKNOWN, "PID 4242"));
        assert_eq!(c.exposure, "All interfaces");
    }
}
