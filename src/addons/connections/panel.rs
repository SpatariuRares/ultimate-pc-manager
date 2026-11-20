//! The side panel: the selected row's application, protocol and state,
//! PID, User, Local, Remote, Exposure and Observed, the note explaining its
//! exposure, the other exposure notes, and how many other connections its
//! process has.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use egui::{FontId, RichText, Ui};

use super::inventory::{self, Exposure, Inventory, Row};
use super::observed::{duration_text, Observed};
use super::actions;
use super::page::{Owner, TableState};
use crate::platform::PageContext;
use crate::sampler::ProcRow;
use crate::theme::{self, Theme};
use crate::widgets::parts;

/// The six exposure explanations, by exposure name, in a fixed order:
/// each exposure's [`Exposure::note`], and Local IPC's (a scope Windows
/// cannot fill, whose note is still listed).
pub const NOTES: [(&str, &str); 6] = [
    (Exposure::AllInterfaces.label(), Exposure::AllInterfaces.note()),
    (Exposure::Loopback.label(), Exposure::Loopback.note()),
    (Exposure::LocalNetwork.label(), Exposure::LocalNetwork.note()),
    (Exposure::Internet.label(), Exposure::Internet.note()),
    ("Local IPC", "Unix-domain sockets carry local interprocess communication and do not reach the network."),
    (Exposure::Unspecified.label(), Exposure::Unspecified.note()),
];

/// The panel's text with no row selected.
pub const EMPTY: &str = "Select a row to inspect its route and owner";

/// The notes other than `exposure`'s, in [`NOTES`]' order.
pub fn other_notes(exposure: Exposure) -> Vec<(&'static str, &'static str)> {
    NOTES.iter().filter(|(label, _)| *label != exposure.label()).copied().collect()
}

/// How many other rows the process owning `rows[index]` has.
pub fn other_connections(rows: &[Row], index: usize) -> usize {
    let pid = rows[index].pid;
    rows.iter().enumerate().filter(|(i, r)| *i != index && r.pid == pid).count()
}

/// The texts of the panel for one row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Details {
    pub application: String,
    /// "TCP6 · Established".
    pub headline: String,
    /// PID, User, Local, Remote, Exposure, Observed.
    pub fields: [(&'static str, String); 6],
    /// The explanation of the row's exposure.
    pub note: &'static str,
    /// The other notes, under "The other exposure notes".
    pub other_notes: Vec<(&'static str, &'static str)>,
    /// How many other connections the process has.
    pub others: String,
}

/// The panel of `row`, owned by `owner` and shown as `application` (the
/// table's Application text), seen for `observed` (None: not yet), whose
/// process has `others` other connections.
pub fn details(row: &Row, owner: &Owner, application: &str, observed: Option<Duration>, others: usize) -> Details {
    let exposure = inventory::exposure(row);
    let dash = || "\u{2014}".to_string();
    Details {
        application: application.to_string(),
        headline: format!("{} \u{b7} {}", inventory::protocol_text(row), inventory::state_text(row)),
        fields: [
            ("PID", row.pid.to_string()),
            ("User", owner.user.clone()),
            ("Local", inventory::endpoint_text(&row.local)),
            // As the table's Remote / service column: with the port's service.
            ("Remote", inventory::remote_service_text(row)),
            ("Exposure", exposure.label().to_string()),
            ("Observed", observed.map_or_else(dash, duration_text)),
        ],
        note: exposure.note(),
        other_notes: other_notes(exposure),
        others: match others {
            0 => "No other connections of this process".into(),
            1 => "1 other connection of this process".into(),
            n => format!("{n} other connections of this process"),
        },
    }
}

/// Draws the panel for the row at `selected` in `inventory` (None: the
/// empty text), with the actions on it; End process… waits in
/// `table.ending` for the confirmation.
pub fn draw(
    ui: &mut Ui,
    page: &mut PageContext,
    inventory: Option<&Inventory>,
    selected: Option<usize>,
    observed: &Observed,
    table: &mut TableState,
) {
    let t = theme::of_ui(ui);
    super::globe::draw(ui, inventory, selected);
    ui.add_space(8.0);
    let row = inventory.zip(selected).and_then(|(inv, i)| inv.rows.get(i).map(|r| (inv, i, r)));
    let Some((inv, i, row)) = row else {
        ui.vertical_centered(|ui| ui.label(RichText::new(EMPTY).color(t.label)));
        return;
    };
    let procs: HashMap<u32, &ProcRow> = page.sample.procs.iter().map(|p| (p.pid, p)).collect();
    let own = super::page::owner(row.pid, &procs);
    let application = super::page::application(&table.names.name(&own), &own, inv.services.get(&row.pid));
    let target = actions::target(row, &super::page::cells(row, &own, &application), &own);
    let d = details(row, &own, &application, observed.age(row, Instant::now()), other_connections(&inv.rows, i));
    let exposure = super::page::exposure_color(inventory::exposure(row), &t);
    ui.vertical_centered(|ui| {
        ui.add(egui::Label::new(RichText::new(&d.application).font(FontId::new(16.0, theme::display())).color(t.text)).wrap());
        ui.label(RichText::new(&d.headline).font(t.fonts.label.clone()).color(t.label));
    });
    ui.add_space(10.0);
    egui::Grid::new("connections-details").num_columns(2).spacing([12.0, 5.0]).min_col_width(64.0).show(ui, |ui| {
        for (label, value) in &d.fields {
            ui.label(RichText::new(*label).color(t.label));
            let ink = if *label == "Exposure" { exposure } else { t.text };
            ui.add(egui::Label::new(RichText::new(value.as_str()).color(ink)).wrap());
            ui.end_row();
        }
    });
    ui.add_space(10.0);
    parts::warm_note(ui, d.note);
    ui.add_space(6.0);
    ui.label(RichText::new(&d.others).font(t.fonts.label.clone()).color(t.label));
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("The other exposure notes").font(FontId::new(12.5, theme::semibold())).color(t.soft));
    ui.add_space(4.0);
    egui::Grid::new("connections-other-notes").num_columns(2).spacing([10.0, 6.0]).min_col_width(84.0).show(ui, |ui| {
        for (label, text) in &d.other_notes {
            let ink = notes_color(label, &t);
            ui.label(RichText::new(*label).font(t.fonts.label.clone()).color(ink));
            ui.add(egui::Label::new(RichText::new(*text).font(t.fonts.label.clone()).color(t.soft)).wrap());
            ui.end_row();
        }
    });
    ui.add_space(8.0);
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Actions").font(FontId::new(13.0, theme::semibold())).color(t.text));
        ui.label(RichText::new("Addition").small().color(t.faint));
    });
    actions::draw(ui, &target, false, &mut page.actions, &mut table.ending);
}

/// The colour of an exposure note's name: its exposure's colour (Local IPC
/// and Unspecified grey).
fn notes_color(label: &str, t: &Theme) -> egui::Color32 {
    [Exposure::AllInterfaces, Exposure::Internet, Exposure::LocalNetwork, Exposure::Loopback]
        .into_iter()
        .find(|e| e.label() == label)
        .map_or(t.label, |e| super::page::exposure_color(e, t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{Exposure, Proto, Row, TcpState};
    use crate::addons::connections::page::Owner;
    use std::time::Duration;

    #[test]
    fn the_six_exposure_notes_have_fixed_texts() {
        assert_eq!(
            NOTES,
            [
                (
                    "All interfaces",
                    "This socket accepts traffic on every matching network interface. Firewall and router rules still determine whether it is reachable."
                ),
                ("Loopback", "This socket is restricted to software running on this computer."),
                ("Local network", "This endpoint is on a private, link-local, or local IPv6 network."),
                (
                    "Internet",
                    "The remote endpoint is outside the local address ranges. A shared or CDN address may represent more than one service."
                ),
                ("Local IPC", "Unix-domain sockets carry local interprocess communication and do not reach the network."),
                ("Unspecified", "The available socket data is not sufficient to classify exposure."),
            ]
        );
    }

    #[test]
    fn each_exposure_has_its_note_and_the_other_notes_are_the_remaining_five() {
        assert_eq!(Exposure::AllInterfaces.note(), NOTES[0].1);
        assert_eq!(Exposure::Loopback.note(), NOTES[1].1);
        assert_eq!(Exposure::LocalNetwork.note(), NOTES[2].1);
        assert_eq!(Exposure::Internet.note(), NOTES[3].1);
        assert_eq!(Exposure::Unspecified.note(), NOTES[5].1);
        let others: Vec<&str> = other_notes(Exposure::Internet).iter().map(|(label, _)| *label).collect();
        assert_eq!(others, ["All interfaces", "Loopback", "Local network", "Local IPC", "Unspecified"]);
    }

    fn system_owner() -> Owner {
        Owner { name: "svchost.exe".into(), exe: String::new(), user: "SYSTEM".into() }
    }

    #[test]
    fn a_selected_connection_fills_every_field() {
        let row = Row {
            proto: Proto::Tcp,
            local: "[2a02::5]:50123".parse().unwrap(),
            remote: Some("[2a00:1450::200e]:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 1868,
        };
        let friendly = "Host Process for Windows Services (RpcSs)";
        let d = details(&row, &system_owner(), friendly, Some(Duration::from_secs(325)), 3);
        assert_eq!(d.application, friendly, "the table's Application text, not the bare process name");
        assert_eq!(d.headline, "TCP6 \u{b7} Established");
        assert_eq!(
            d.fields,
            [
                ("PID", "1868".to_string()),
                ("User", "SYSTEM".to_string()),
                ("Local", "[2a02::5]:50123".to_string()),
                ("Remote", "[2a00:1450::200e]:443 \u{b7} HTTPS".to_string()),
                ("Exposure", "Internet".to_string()),
                ("Observed", "325s".to_string()),
            ]
        );
        assert_eq!(d.note, NOTES[3].1);
        assert_eq!(d.others, "3 other connections of this process");
    }

    #[test]
    fn a_udp_socket_not_yet_observed_shows_dashes() {
        let row = Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 4 };
        let d = details(&row, &system_owner(), "System", None, 0);
        assert_eq!(d.headline, "UDP4 \u{b7} Datagram");
        assert_eq!(d.fields[3].1, "\u{2014}");
        assert_eq!(d.fields[5].1, "\u{2014}");
        assert_eq!(d.note, NOTES[0].1);
        assert_eq!(d.others, "No other connections of this process");
        assert_eq!(details(&row, &system_owner(), "System", None, 1).others, "1 other connection of this process");
    }

    #[test]
    fn a_listener_shows_its_port_and_well_known_service_as_remote() {
        let row = Row { proto: Proto::Tcp, local: "[::]:445".parse().unwrap(), remote: None, state: Some(TcpState::Listening), pid: 4 };
        let d = details(&row, &system_owner(), "System", None, 0);
        assert_eq!(d.fields[3], ("Remote", "445 \u{b7} SMB".to_string()));
    }

    #[test]
    fn other_connections_count_the_same_pid_but_not_the_row_itself() {
        let r = |local: &str, pid| Row { proto: Proto::Udp, local: local.parse().unwrap(), remote: None, state: None, pid };
        let rows = [r("0.0.0.0:1", 7), r("0.0.0.0:2", 7), r("0.0.0.0:3", 8), r("0.0.0.0:4", 7)];
        assert_eq!(other_connections(&rows, 0), 2);
        assert_eq!(other_connections(&rows, 2), 0);
    }
}
