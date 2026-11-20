//! The actions on a connection (all additions): Copy details, Copy remote
//! address, Search online, Go to the process and End process…, shown in
//! the side panel and in the row's context menu.

use super::inventory::{self, Row, SYSTEM_IDLE_PID, SYSTEM_PID};
use egui::{Align2, RichText, Ui};

use super::page::{Cells, Owner, UNKNOWN};
use crate::platform::Action;
use crate::theme;
use crate::widgets::parts;

/// The connection the actions work on.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Target {
    pub pid: u32,
    /// The process name ("chrome.exe"), for Search online and End.
    pub name: String,
    /// The text of Copy details.
    pub details: String,
    /// The text of Copy remote address.
    pub address: String,
}

/// Every field of the row, one "Label: value" per line.
pub fn details_text(row: &Row, cells: &Cells, owner: &Owner) -> String {
    let mut fields = vec![("Application", cells.application.clone())];
    if !owner.exe.is_empty() {
        fields.push(("Executable", owner.exe.clone()));
    }
    fields.extend([
        ("PID", row.pid.to_string()),
        ("User", owner.user.clone()),
        ("Protocol", cells.protocol.to_string()),
        ("State", cells.state.to_string()),
        ("Local endpoint", cells.local.clone()),
        ("Remote / service", cells.remote.clone()),
        ("Exposure", cells.exposure.to_string()),
    ]);
    fields.iter().map(|(label, value)| format!("{label}: {value}\n")).collect()
}

/// The remote endpoint, or the local one for a socket with no remote end
/// (a listener, UDP).
pub fn address(row: &Row) -> String {
    inventory::endpoint_text(row.remote.as_ref().unwrap_or(&row.local))
}

/// Why End process… is off for System.
pub const SYSTEM_REASON: &str = "System is Windows itself and can't be ended.";

/// Why the process `pid` cannot be ended (None: it can be).
pub fn end_blocked(pid: u32) -> Option<&'static str> {
    // System and the System Idle Process are both Windows.
    matches!(pid, SYSTEM_IDLE_PID | SYSTEM_PID).then_some(SYSTEM_REASON)
}

/// The actions on `row`, shown with `cells` and owned by `owner`.
pub fn target(row: &Row, cells: &Cells, owner: &Owner) -> Target {
    Target { pid: row.pid, name: owner.name.clone(), details: details_text(row, cells, owner), address: address(row) }
}

/// One of the five actions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    EndProcess,
    GoToProcess,
    CopyDetails,
    CopyAddress,
    SearchOnline,
}

/// The actions and their words, in the panel's order.
pub const CHOICES: [(Choice, &str); 5] = [
    (Choice::EndProcess, "End process\u{2026}"),
    (Choice::GoToProcess, "Go to the process"),
    (Choice::CopyDetails, "Copy details"),
    (Choice::CopyAddress, "Copy remote address"),
    (Choice::SearchOnline, "Search online"),
];

/// What a choice does.
#[derive(PartialEq, Debug)]
pub enum Step {
    /// Run this shared action now.
    Run(Action),
    /// Ask before ending this process.
    Confirm(Ending),
}

/// A process the user asked to end, waiting for the confirmation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ending {
    pub pid: u32,
    pub name: String,
}

impl Ending {
    /// The action that ends it, once confirmed.
    pub fn action(&self) -> Action {
        // Forced: a program with no window ignores a polite request.
        Action::End { pid: self.pid, force: true, tree: false, name: self.name.clone() }
    }
}

/// What `choice` does on `target`.
pub fn choose(choice: Choice, target: &Target) -> Step {
    Step::Run(match choice {
        Choice::EndProcess => return Step::Confirm(Ending { pid: target.pid, name: target.name.clone() }),
        Choice::GoToProcess => Action::GoToProcess(target.pid),
        Choice::CopyDetails => Action::Copy(target.details.clone()),
        Choice::CopyAddress => Action::Copy(target.address.clone()),
        Choice::SearchOnline => Action::SearchOnline(target.name.clone()),
    })
}

/// Runs `step`: queues its action, or keeps the process to end until the
/// user confirms.
pub fn apply(step: Step, actions: &mut Vec<Action>, ending: &mut Option<Ending>) {
    match step {
        Step::Run(action) => actions.push(action),
        Step::Confirm(asked) => *ending = Some(asked),
    }
}

/// Whether `choice` can run on `target`, and why not.
fn enabled(choice: Choice, target: &Target) -> Result<(), Option<&'static str>> {
    match choice {
        Choice::EndProcess => end_blocked(target.pid).map_or(Ok(()), |why| Err(Some(why))),
        // Searching for "unknown" finds nothing about the process.
        Choice::SearchOnline if target.name == UNKNOWN => Err(None),
        _ => Ok(()),
    }
}

/// Draws the five actions on `target`: in the panel, End process… as a red
/// outlined button (with why it can't run, when it can't) and the other
/// four as links under it; in the row's context menu (`menu`), entries,
/// End process… in red.
pub fn draw(ui: &mut Ui, target: &Target, menu: bool, actions: &mut Vec<Action>, ending: &mut Option<Ending>) {
    let t = theme::of_ui(ui);
    if menu {
        for (choice, label) in CHOICES {
            let ok = enabled(choice, target);
            let add = |ui: &mut Ui, on: bool| {
                let words = match choice {
                    Choice::EndProcess => RichText::new(label).color(t.red),
                    _ => RichText::new(label),
                };
                ui.add_enabled(on, egui::Button::new(words))
            };
            if run(ui, ok, add, choice, target, actions, ending) {
                ui.close();
            }
        }
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let (choice, label) = CHOICES[0];
        let ok = enabled(choice, target);
        let why = ok.err().flatten();
        run(ui, ok, |ui, on| parts::destructive_button(ui, label, on), choice, target, actions, ending);
        if let Some(why) = why {
            ui.label(RichText::new(why).font(t.fonts.label.clone()).color(t.label));
        }
    });
    ui.add_space(4.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 16.0;
        for (choice, label) in &CHOICES[1..] {
            let ok = enabled(*choice, target);
            let link = |ui: &mut Ui, on: bool| ui.add_enabled_ui(on, |ui| parts::link_button(ui, label)).inner;
            run(ui, ok, link, *choice, target, actions, ending);
        }
    });
}

/// Adds the button for `choice` with `add` (disabled, with why, when `ok`
/// says no) and runs the choice when it is clicked. True when it was
/// clicked.
fn run(
    ui: &mut Ui,
    ok: Result<(), Option<&'static str>>,
    add: impl FnOnce(&mut Ui, bool) -> egui::Response,
    choice: Choice,
    target: &Target,
    actions: &mut Vec<Action>,
    ending: &mut Option<Ending>,
) -> bool {
    let mut response = add(ui, ok.is_ok());
    if let Err(Some(why)) = ok {
        response = response.on_disabled_hover_text(why);
    }
    let clicked = response.clicked();
    if clicked {
        apply(choose(choice, target), actions, ending);
    }
    clicked
}

/// Asks before ending the process in `ending`; End process queues it,
/// Cancel forgets it. A failure is said by the app.
pub fn confirm(ctx: &egui::Context, ending: &mut Option<Ending>, actions: &mut Vec<Action>) {
    let Some(asked) = ending.clone() else { return };
    let mut decided = None;
    let t = theme::current(ctx);
    egui::Window::new("End process?").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
        ui.label(RichText::new(format!("Do you want to end \"{}\" (PID {})?", asked.name, asked.pid)).color(t.text));
        ui.label(RichText::new("Its connections close with it. Unsaved work in it may be lost.").color(t.label));
        ui.horizontal(|ui| {
            if parts::destructive_button(ui, "End process", true).clicked() {
                decided = Some(true);
            }
            if parts::outline_button(ui, "Cancel", true).clicked() {
                decided = Some(false);
            }
        });
    });
    if decided == Some(true) {
        actions.push(asked.action());
    }
    if decided.is_some() {
        *ending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{Proto, TcpState};
    use crate::addons::connections::page;

    fn edge() -> (Row, Owner) {
        let row = Row {
            proto: Proto::Tcp,
            local: "192.168.1.20:50123".parse().unwrap(),
            remote: Some("142.250.180.14:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 7420,
        };
        let owner = Owner { name: "msedge.exe".into(), exe: r"C:\Program Files\Edge\msedge.exe".into(), user: "rares".into() };
        (row, owner)
    }

    #[test]
    fn copy_details_lists_every_field_of_the_row() {
        let (row, owner) = edge();
        let cells = page::cells(&row, &owner, "Microsoft Edge");
        assert_eq!(
            details_text(&row, &cells, &owner),
            "Application: Microsoft Edge\n\
             Executable: C:\\Program Files\\Edge\\msedge.exe\n\
             PID: 7420\n\
             User: rares\n\
             Protocol: TCP4\n\
             State: Established\n\
             Local endpoint: 192.168.1.20:50123\n\
             Remote / service: 142.250.180.14:443 \u{b7} HTTPS\n\
             Exposure: Internet\n"
        );
    }

    #[test]
    fn copy_remote_address_copies_the_remote_end_or_a_listener_s_local_one() {
        let (row, _) = edge();
        assert_eq!(address(&row), "142.250.180.14:443");
        let listener = Row { remote: None, state: Some(TcpState::Listening), local: "[::]:445".parse().unwrap(), ..row };
        assert_eq!(address(&listener), "[::]:445");
        let udp = Row { proto: Proto::Udp, remote: None, state: None, local: "0.0.0.0:5353".parse().unwrap(), pid: 1 };
        assert_eq!(address(&udp), "0.0.0.0:5353");
    }

    #[test]
    fn system_cannot_be_ended_and_says_why() {
        assert_eq!(end_blocked(4), Some("System is Windows itself and can't be ended."));
        assert_eq!(end_blocked(0), Some(SYSTEM_REASON), "System Idle Process is Windows too");
        assert_eq!(end_blocked(7420), None);
    }

    fn edge_target() -> Target {
        let (row, owner) = edge();
        target(&row, &page::cells(&row, &owner, "Microsoft Edge"), &owner)
    }

    #[test]
    fn the_five_actions_in_the_panel_s_order() {
        let labels: Vec<&str> = CHOICES.iter().map(|(_, label)| *label).collect();
        assert_eq!(labels, ["End process\u{2026}", "Go to the process", "Copy details", "Copy remote address", "Search online"]);
    }

    #[test]
    fn copy_search_and_go_to_run_the_shared_actions() {
        let t = edge_target();
        assert_eq!(choose(Choice::CopyDetails, &t), Step::Run(Action::Copy(t.details.clone())));
        assert_eq!(choose(Choice::CopyAddress, &t), Step::Run(Action::Copy("142.250.180.14:443".into())));
        assert_eq!(choose(Choice::SearchOnline, &t), Step::Run(Action::SearchOnline("msedge.exe".into())));
        assert_eq!(choose(Choice::GoToProcess, &t), Step::Run(Action::GoToProcess(7420)));
    }

    #[test]
    fn end_process_asks_first_then_ends_the_process() {
        let t = edge_target();
        let Step::Confirm(ending) = choose(Choice::EndProcess, &t) else { panic!("End process\u{2026} must ask first") };
        assert_eq!(ending, Ending { pid: 7420, name: "msedge.exe".into() });
        assert_eq!(ending.action(), Action::End { pid: 7420, force: true, tree: false, name: "msedge.exe".into() });
    }
}
