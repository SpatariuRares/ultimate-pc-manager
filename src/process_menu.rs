//! The process context menu: its lines, the entries built for the selected
//! rows, and the drawing of them. It sits next to the Processes view and is
//! the only place the menu's items and labels are defined.

use egui::{RichText, Ui};

use crate::app::Action;
use crate::availability::{is_available, Feature};
use crate::page_processes::ProcView;
use crate::sampler::ProcRow;
use crate::process_actions::{self, PRIORITIES};
use crate::widgets::parts;

/// One of the end items at the top of a process's menu.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EndItem {
    Task,
    ForceQuit,
    Tree,
    ForceQuitTree,
}

/// The end items of a process's menu, in order. The Processes page asks
/// before Force quit and End process tree run.
pub const END_ITEMS: [(EndItem, &str); 4] = [
    (EndItem::Task, "End task"),
    (EndItem::ForceQuit, "Force quit"),
    (EndItem::Tree, "End process tree"),
    (EndItem::ForceQuitTree, "Force quit process tree"),
];

impl EndItem {
    /// The end this item sends for the selected `rows`: one process, or
    /// all of them at once.
    pub fn action(self, rows: &[&ProcRow]) -> Action {
        let (force, tree) = match self {
            EndItem::Task => (false, false),
            EndItem::ForceQuit => (true, false),
            EndItem::Tree => (false, true),
            EndItem::ForceQuitTree => (true, true),
        };
        match rows {
            [p] => Action::End { pid: p.pid, force, tree, name: p.name.clone() },
            _ => Action::EndMany { pids: rows.iter().map(|p| p.pid).collect(), force, tree },
        }
    }
}

/// The process menu's labels and the plural each takes when several rows
/// are selected.
pub const PLURAL_LABELS: [(&str, &str); 7] = [
    ("End task", "End selected tasks"),
    ("Force quit", "Force quit selected tasks"),
    ("End process tree", "End selected process trees"),
    ("Force quit process tree", "Force quit selected process trees"),
    ("Suspend", "Suspend selected tasks"),
    ("Resume", "Resume selected tasks"),
    ("Efficiency mode", "Efficiency mode for selected tasks"),
];

/// A menu item's label: its plural when `many` rows are selected and it
/// has one.
pub fn menu_label(label: &'static str, many: bool) -> &'static str {
    match PLURAL_LABELS.iter().find(|(one, _)| *one == label) {
        Some((_, plural)) if many => plural,
        _ => label,
    }
}

/// The end items of the menu for the selected `rows`, each with what it
/// sends.
pub fn end_entries(rows: &[&ProcRow]) -> Vec<(&'static str, Action)> {
    END_ITEMS.iter().map(|(item, label)| (menu_label(label, rows.len() > 1), item.action(rows))).collect()
}

/// The items the add-ons' Processes columns add to `p`'s menu (an
/// addition): `links` are `(add-on id, item)`, each opening that add-on's
/// page on `p`. None while no such add-on is on.
pub fn process_links(links: &[(&'static str, &'static str)], p: &ProcRow) -> Vec<(&'static str, Action)> {
    links.iter().map(|(addon, label)| (*label, Action::ShowProcessIn { addon, pid: p.pid })).collect()
}

/// The Resource values submenu of a process's menu, in order: whether the
/// item shows percents, and its label. The current one is ticked; a click
/// sends [`Action::ResourceValues`].
pub const RESOURCE_VALUES: [(bool, &str); 2] = [(false, "Values"), (true, "Percents")];

/// The first item of a process's menu, drawn bold as the default (a
/// double-click on the row does the same).
pub const INSPECT: &str = "Inspect";

/// A named place in a process's menu. The item a later ticket brings
/// (File activity #176) already holds its place here and is not drawn
/// until [`menu_entries`] builds it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuSlot {
    Inspect,
    End(EndItem),
    Restart,
    Suspend,
    Resume,
    View,
    Follow,
    ResourceValues,
    EfficiencyMode,
    SetPriority,
    SetAffinity,
    AnalyzeWaitChain,
    FileActivity,
    UacVirtualization,
    CreateDump,
    GoToServices,
    BringToFront,
    OpenFileLocation,
    SearchOnline,
    CopyName,
}

impl MenuSlot {
    /// The system feature the item needs, if it needs one; an item whose
    /// feature the system lacks is hidden.
    fn feature(self) -> Option<Feature> {
        match self {
            MenuSlot::EfficiencyMode => Some(Feature::ProcessEfficiency),
            MenuSlot::AnalyzeWaitChain => Some(Feature::ProcessWaitChain),
            MenuSlot::UacVirtualization => Some(Feature::ProcessUacVirtualization),
            MenuSlot::CreateDump => Some(Feature::ProcessDump),
            _ => None,
        }
    }
}

/// A line of a process's menu layout: an item's place or a separator.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MenuLine {
    Item(MenuSlot),
    Separator,
}

use MenuLine::{Item as It, Separator as Sep};

/// A process's menu, top to bottom with its separators, the View / Follow
/// / Resource values block included. There is no Provide feedback item.
pub const PROCESS_MENU: [MenuLine; 29] = [
    It(MenuSlot::Inspect),
    Sep,
    It(MenuSlot::End(EndItem::Task)),
    It(MenuSlot::Restart),
    It(MenuSlot::End(EndItem::ForceQuit)),
    It(MenuSlot::End(EndItem::Tree)),
    It(MenuSlot::End(EndItem::ForceQuitTree)),
    Sep,
    It(MenuSlot::Suspend),
    It(MenuSlot::Resume),
    Sep,
    It(MenuSlot::View),
    It(MenuSlot::Follow),
    Sep,
    It(MenuSlot::ResourceValues),
    Sep,
    It(MenuSlot::EfficiencyMode),
    It(MenuSlot::SetPriority),
    It(MenuSlot::SetAffinity),
    It(MenuSlot::AnalyzeWaitChain),
    It(MenuSlot::FileActivity),
    It(MenuSlot::UacVirtualization),
    Sep,
    It(MenuSlot::CreateDump),
    It(MenuSlot::GoToServices),
    It(MenuSlot::BringToFront),
    It(MenuSlot::OpenFileLocation),
    It(MenuSlot::SearchOnline),
    It(MenuSlot::CopyName),
];

/// The short menu of a process that has exited.
pub const EXITED_MENU: [MenuLine; 5] = [It(MenuSlot::Inspect), Sep, It(MenuSlot::OpenFileLocation), It(MenuSlot::SearchOnline), It(MenuSlot::CopyName)];

/// The View submenu, top to bottom: the items of
/// the Process View dropdown, which they share (`None` is the separator).
pub const VIEW_SUBMENU: [Option<ProcView>; 6] =
    [Some(ProcView::Tree), Some(ProcView::ExpandAll), Some(ProcView::CollapseAll), None, Some(ProcView::Smooth), Some(ProcView::ShowExited)];

/// One line of a process's menu as drawn.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuEntry {
    Separator,
    /// `checked` is None for an item that is never ticked.
    Item { label: String, bold: bool, enabled: bool, checked: Option<bool>, actions: Vec<Action> },
    Sub { label: &'static str, items: Vec<MenuEntry> },
}

impl MenuEntry {
    fn item(label: &str, action: Action) -> MenuEntry {
        MenuEntry::Item { label: label.to_string(), bold: false, enabled: true, checked: None, actions: vec![action] }
    }

    /// The words drawn for this line ("" for a separator).
    #[cfg(test)]
    pub fn label(&self) -> &str {
        match self {
            MenuEntry::Separator => "",
            MenuEntry::Item { label, .. } => label,
            MenuEntry::Sub { label, .. } => label,
        }
    }
}

/// What a process's menu reads from the page besides the rows.
#[derive(Clone, Copy)]
pub struct MenuState<'a> {
    /// `(add-on id, item)` of the add-ons' columns ([`process_links`]).
    pub links: &'a [(&'static str, &'static str)],
    /// The Resource values choice now (Percents or Values).
    pub percents: bool,
    /// The Process View dropdown's items, shared by the View submenu.
    pub view: &'a [parts::DropItem<ProcView>],
    /// The priority class of the one row selected, ticked in Set priority
    /// (read by [`process_menu`] when None).
    pub priority: Option<&'static str>,
    /// The PID of the shell's Windows Explorer (the process of
    /// `GetShellWindow`), the one row that gets Restart.
    pub shell: Option<u32>,
    /// The PID of the process Follow process is on.
    pub following: Option<u32>,
    /// The running services by the PID of their host (the sampler's), for
    /// Go to service(s); None reads as hosting none.
    pub services: Option<&'a std::collections::HashMap<u32, Vec<crate::sampler::ServiceInfo>>>,
    /// The processor affinity of the row right-clicked, ticked in Set
    /// affinity (read by [`process_menu`] when None; still None greys the
    /// item, as when the process cannot be opened).
    pub affinity: Option<process_actions::Affinity>,
    /// Whether the row right-clicked has any visible top-level window
    /// (read by [`process_menu`] when None; still None reads as whether
    /// it is listed under Apps).
    pub windowed: Option<bool>,
}

/// Set affinity's first item, ticked while the mask holds every logical
/// processor.
pub const AFFINITY_ALL: &str = "All processors";

/// Set affinity's only, greyed item on a system with several processor
/// groups.
pub const AFFINITY_GROUPS: &str = "Unavailable for this processor-group topology";

/// Said instead of clearing the last processor of an affinity.
pub const AFFINITY_LAST: &str = "At least one processor must remain selected.";

/// The status a process reads while every thread is suspended.
const SUSPENDED: &str = "Suspended";

/// A process's menu as data, top to bottom ([`PROCESS_MENU`],
/// or [`EXITED_MENU`] for an exited process), then the add-ons' items.
/// `p` is the row right-clicked and `rows` the selection it belongs to:
/// with several rows the items take their plural labels and act on all of
/// them, and the single-row items are left out. Places whose ticket has
/// not come are skipped, and a separator never starts, ends or doubles.
pub fn menu_entries(p: &ProcRow, rows: &[&ProcRow], state: &MenuState) -> Vec<MenuEntry> {
    let own = [p];
    let rows = if rows.is_empty() { &own[..] } else { rows };
    let single = rows.len() == 1;
    let many = !single;
    let pids: Vec<u32> = rows.iter().map(|r| r.pid).collect();
    let layout: &[MenuLine] = if single && p.status == crate::process_view::EXITED { &EXITED_MENU } else { &PROCESS_MENU };
    let mut out: Vec<MenuEntry> = Vec::new();
    for line in layout {
        let slot = match line {
            MenuLine::Separator => {
                if matches!(out.last(), Some(e) if *e != MenuEntry::Separator) {
                    out.push(MenuEntry::Separator);
                }
                continue;
            }
            MenuLine::Item(slot) => *slot,
        };
        if slot.feature().is_some_and(|f| !is_available(f)) {
            continue;
        }
        let entry = match slot {
            MenuSlot::Inspect if single => {
                MenuEntry::Item { label: INSPECT.into(), bold: true, enabled: true, checked: None, actions: vec![Action::Inspect(p.pid)] }
            }
            MenuSlot::End(item) => {
                let at = END_ITEMS.iter().position(|(i, _)| *i == item).unwrap_or(0);
                let (label, action) = end_entries(rows).swap_remove(at);
                MenuEntry::item(label, action)
            }
            // Only on the shell's Windows Explorer.
            MenuSlot::Restart if single && p.service.is_none() && state.shell == Some(p.pid) => MenuEntry::item("Restart", Action::RestartExplorer(p.pid)),
            MenuSlot::Follow if single => {
                let on = state.following == Some(p.pid);
                MenuEntry::Item {
                    label: if on { "Stop following" } else { "Follow process" }.into(),
                    bold: false,
                    enabled: true,
                    checked: Some(on),
                    actions: vec![Action::Follow((!on).then_some(p.pid))],
                }
            }
            // Greyed when the process hosts no running service; a service
            // row goes to its own service.
            MenuSlot::GoToServices => {
                let own = p.service.as_ref().map(|s| s.name.clone());
                let hosts = state.services.and_then(|m| m.get(&p.pid)).is_some_and(|v| !v.is_empty());
                MenuEntry::Item {
                    label: "Go to service(s)".into(),
                    bold: false,
                    enabled: own.is_some() || hosts,
                    checked: None,
                    actions: vec![Action::GoToServices { pid: p.pid, service: own }],
                }
            }
            MenuSlot::Suspend => MenuEntry::item(menu_label("Suspend", many), Action::Suspend(pids.clone())),
            // Greyed unless a selected process is suspended.
            MenuSlot::Resume => MenuEntry::Item {
                label: menu_label("Resume", many).into(),
                bold: false,
                enabled: rows.iter().any(|r| r.status == SUSPENDED),
                checked: None,
                actions: vec![Action::Resume(pids.clone())],
            },
            MenuSlot::View => MenuEntry::Sub { label: "View", items: view_items(state.view) },
            MenuSlot::ResourceValues => MenuEntry::Sub {
                label: "Resource values",
                items: RESOURCE_VALUES
                    .iter()
                    .map(|(choice, label)| MenuEntry::Item {
                        label: label.to_string(),
                        bold: false,
                        enabled: true,
                        checked: Some(*choice == state.percents),
                        actions: vec![Action::ResourceValues { percents: *choice }],
                    })
                    .collect(),
            },
            MenuSlot::EfficiencyMode => MenuEntry::item(menu_label("Efficiency mode", many), Action::Efficiency(pids.clone())),
            MenuSlot::SetPriority => MenuEntry::Sub {
                label: "Set priority",
                items: PRIORITIES
                    .iter()
                    .map(|pr| MenuEntry::Item {
                        label: pr.to_string(),
                        bold: false,
                        enabled: true,
                        checked: Some(single && state.priority == Some(*pr)),
                        actions: pids.iter().map(|&pid| Action::Priority(pid, pr)).collect(),
                    })
                    .collect(),
            },
            MenuSlot::SetAffinity => affinity_entry(&pids, state.affinity),
            // Always drawn; it analyses the row right-clicked.
            MenuSlot::AnalyzeWaitChain => MenuEntry::item("Analyze wait chain", Action::AnalyzeWaitChain(p.pid)),
            // Never greyed: the administrator check runs on the click.
            MenuSlot::FileActivity if single => MenuEntry::item("File activity", Action::FileActivity(p.pid)),
            MenuSlot::UacVirtualization => MenuEntry::item("UAC virtualization", Action::UacVirtualization(pids.clone())),
            MenuSlot::CreateDump if single => MenuEntry::item("Create memory dump file", Action::Dump { pid: p.pid, name: p.name.clone() }),
            // Greyed when the process has no window at all (US58); an App
            // always has one.
            MenuSlot::BringToFront if single => MenuEntry::Item {
                label: "Bring to front".into(),
                bold: false,
                enabled: p.app || state.windowed == Some(true),
                checked: None,
                actions: vec![Action::BringToFront(p.pid)],
            },
            MenuSlot::OpenFileLocation if single => MenuEntry::Item {
                label: "Open file location".into(),
                bold: false,
                enabled: !p.exe.is_empty(),
                checked: None,
                actions: vec![Action::OpenLocation(p.exe.clone())],
            },
            MenuSlot::SearchOnline => MenuEntry::item("Search online", Action::SearchOnline(p.name.clone())),
            MenuSlot::CopyName => MenuEntry::item("Copy process name", Action::Copy(p.name.clone())),
            // Single-row items with several rows, and the places of later
            // tickets.
            _ => continue,
        };
        out.push(entry);
    }
    let added = process_links(state.links, p);
    if !added.is_empty() {
        out.push(MenuEntry::Separator);
    }
    out.extend(added.into_iter().map(|(label, action)| MenuEntry::item(label, action)));
    while out.last() == Some(&MenuEntry::Separator) {
        out.pop();
    }
    out
}

/// Set affinity ▸ for `pids` from the right-clicked process's `read`: All
/// processors (ticked when the mask is full), separator, "CPU n" per
/// logical processor ticked from the mask; a click
/// sends the mask with that processor toggled, or says [`AFFINITY_LAST`]
/// instead of clearing the last one. Several processor groups give one
/// greyed [`AFFINITY_GROUPS`]; an unread affinity greys the item.
fn affinity_entry(pids: &[u32], read: Option<process_actions::Affinity>) -> MenuEntry {
    let item = |label: String, enabled: bool, checked: Option<bool>, actions: Vec<Action>| MenuEntry::Item { label, bold: false, enabled, checked, actions };
    let items = match read {
        None => return item("Set affinity".into(), false, None, Vec::new()),
        Some(process_actions::Affinity::ManyGroups) => vec![item(AFFINITY_GROUPS.into(), false, None, Vec::new())],
        Some(process_actions::Affinity::Mask { process, system }) => {
            let process = process & system;
            let mut items = vec![item(AFFINITY_ALL.into(), true, Some(process == system), vec![Action::Affinity(pids.to_vec(), system)]), MenuEntry::Separator];
            for n in (0..64).filter(|n| system & (1u64 << n) != 0) {
                let bit = 1u64 << n;
                let toggled = process ^ bit;
                let send = if toggled == 0 { Action::Say(AFFINITY_LAST.into()) } else { Action::Affinity(pids.to_vec(), toggled) };
                items.push(item(format!("CPU {n}"), true, Some(process & bit != 0), vec![send]));
            }
            items
        }
    };
    MenuEntry::Sub { label: "Set affinity", items }
}

/// The View submenu from the Process View dropdown's items, in
/// [`VIEW_SUBMENU`]'s order; each sends [`Action::ProcessView`].
fn view_items(view: &[parts::DropItem<ProcView>]) -> Vec<MenuEntry> {
    VIEW_SUBMENU
        .iter()
        .filter_map(|want| match want {
            None => Some(MenuEntry::Separator),
            Some(id) => view.iter().find_map(|i| match i {
                parts::DropItem::Item { id: got, label, checked, enabled } if got == id => Some(MenuEntry::Item {
                    label: label.to_string(),
                    bold: false,
                    enabled: *enabled,
                    checked: *checked,
                    actions: vec![Action::ProcessView(*id)],
                }),
                _ => None,
            }),
        })
        .collect()
}

/// `state` with what the menu reads about the row right-clicked, asked of
/// `system`: the priority (one row only), the affinity, whether the row
/// has a window and which process is the shell's Windows Explorer. A value the page already gave is kept.
pub fn read_menu_state<'a>(system: &dyn crate::system_actions::SystemActions, p: &ProcRow, rows: &[&ProcRow], state: &MenuState<'a>) -> MenuState<'a> {
    let priority = if state.priority.is_none() && rows.len() <= 1 { system.priority_of(p.pid) } else { state.priority };
    let affinity = state.affinity.or_else(|| system.affinity_of(p.pid));
    let windowed = state.windowed.or_else(|| (rows.len() <= 1).then(|| p.app || system.has_window(p.pid)));
    let shell = state.shell.or_else(|| system.shell_pid());
    MenuState { priority, affinity, windowed, shell, ..*state }
}

/// The right-click menu of a process ([`menu_entries`]), drawn. The
/// priority, affinity and window of the row come in `state`
/// ([`read_menu_state`] reads them from the seam).
pub fn process_menu(ui: &mut Ui, p: &ProcRow, rows: &[&ProcRow], state: &MenuState, actions: &mut Vec<Action>) {
    draw_entries(ui, &menu_entries(p, rows, state), false, actions);
}

/// Draws menu lines; in a submenu every item keeps room for a tick.
fn draw_entries(ui: &mut Ui, entries: &[MenuEntry], sub: bool, actions: &mut Vec<Action>) {
    for e in entries {
        match e {
            MenuEntry::Separator => {
                ui.separator();
            }
            MenuEntry::Sub { label, items } => {
                ui.menu_button(*label, |ui| draw_entries(ui, items, true, actions));
            }
            MenuEntry::Item { label, bold, enabled, checked, actions: sends } => {
                let mark = match checked {
                    Some(true) => "\u{2022} ",
                    _ if sub => "   ",
                    _ => "",
                };
                let mut text = RichText::new(format!("{mark}{label}"));
                if *bold {
                    text = text.family(crate::theme::semibold());
                }
                if ui.add_enabled(*enabled, egui::Button::new(text)).clicked() {
                    actions.extend(sends.iter().cloned());
                    ui.close();
                }
            }
        }
    }
}
