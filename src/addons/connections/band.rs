//! The status band at the top of the page: the sentence that answers the
//! main question (how many sockets listen on all interfaces), its subtitle,
//! the "Show the Public scope" action and the five tiles.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use egui::Ui;

use super::a11y;
use super::inventory::{Inventory, Row, Status, SYSTEM_IDLE_PID, SYSTEM_PID};
use super::page::{owner, Owner};
use crate::platform::PageContext;
use crate::sampler::ProcRow;
use crate::theme;
use crate::widgets::parts::{alert_tile, kpi_tile, primary_button, BandArea, Mark, StatusBand};
use super::scope;
use super::Scope;
use crate::addons::plural;
use crate::page_perf::{fmt_bit_rate, fmt_bytes_rate};
use crate::sampler::NetRow;

/// The most process names the subtitle lists in each group.
const NAMES_SHOWN: usize = 3;

/// The sentence for `public` TCP listeners on all interfaces.
pub fn headline(public: usize) -> String {
    match public {
        0 => "No socket listens on all interfaces".into(),
        1 => "1 socket listens on all interfaces".into(),
        n => format!("{n} sockets listen on all interfaces"),
    }
}

/// Core Windows processes whose executable path a non-elevated app cannot
/// read (they run as protected or SYSTEM processes), told by their name.
const CORE_WINDOWS_PROCESSES: [&str; 10] = [
    "registry",
    "smss.exe",
    "csrss.exe",
    "wininit.exe",
    "winlogon.exe",
    "services.exe",
    "lsass.exe",
    "lsaiso.exe",
    "svchost.exe",
    "spoolsv.exe",
];

/// A process that is part of Windows: System Idle Process and System
/// (PIDs 0 and 4), an executable under the Windows folder `windir`, or,
/// when the executable is not known, one of the core Windows processes by
/// `name`. Any other process with an unknown executable is not Windows.
pub fn is_windows_component(pid: u32, name: &str, exe: &str, windir: &str) -> bool {
    if pid == SYSTEM_IDLE_PID || pid == SYSTEM_PID {
        return true;
    }
    if exe.is_empty() {
        return CORE_WINDOWS_PROCESSES.contains(&name.to_lowercase().as_str());
    }
    let windir = windir.trim_end_matches('\\').to_lowercase();
    !windir.is_empty() && exe.to_lowercase().starts_with(&format!("{windir}\\"))
}

/// "`n` processes" ("1 process").
fn processes_text(n: usize) -> String {
    if n == 1 {
        "1 process".into()
    } else {
        format!("{n} processes")
    }
}

/// "`n` `word`s (a, b, c and 2 more)" for the distinct process `names`.
fn group(n: usize, word: &str, names: &[String]) -> String {
    let shown: Vec<&str> = names.iter().take(NAMES_SHOWN).map(String::as_str).collect();
    let more = names.len().saturating_sub(NAMES_SHOWN);
    let list = if more > 0 { format!("{} and {more} more", shown.join(", ")) } else { shown.join(", ") };
    format!("{} ({list})", plural(n, word))
}

/// The distinct processes (PIDs) that own `rows`.
pub fn processes(rows: &[Row]) -> usize {
    rows.iter().map(|r| r.pid).collect::<BTreeSet<u32>>().len()
}

/// The whole system's receive and send rates (bytes per second): the sum
/// over the sampler's network interfaces.
pub fn system_traffic(net: &[NetRow]) -> (f64, f64) {
    net.iter().fold((0.0, 0.0), |(rx, tx), n| (rx + n.rx_bps, tx + n.tx_bps))
}

/// The System traffic tile's value and note: "↓ 1.80 MB/sec" and
/// "↑ 246 KB/sec", in bits per second when `bits` is set. Down and up sit
/// on two lines so that neither is cut off in a narrow tile.
pub fn traffic_text((rx, tx): (f64, f64), bits: bool) -> (String, String) {
    let rate = if bits { fmt_bit_rate } else { fmt_bytes_rate };
    (format!("\u{2193} {}", rate(rx)), format!("\u{2191} {}", rate(tx)))
}

/// The System traffic tile's title.
pub const SYSTEM_TRAFFIC: &str = "System traffic \u{2193}/\u{2191}";

/// A band tile: its title, its value and an optional note under the value.
pub type Tile = (&'static str, String, Option<String>);

/// The tiles: Connections (the rows kept, as the All scope and the
/// screen-reader summary count them; past the row limit the inventory line
/// says it is partial), Listening, Public, Processes and, when `traffic` is
/// given (its value and note), System traffic.
pub fn tiles(inv: &Inventory, traffic: Option<(String, String)>) -> Vec<Tile> {
    let mut t = vec![
        ("Connections", inv.rows.len().to_string(), None),
        ("Listening", scope::count(Scope::Listening, &inv.rows).to_string(), None),
        ("Public", scope::count(Scope::Public, &inv.rows).to_string(), None),
        ("Processes", processes(&inv.rows).to_string(), None),
    ];
    if let Some((down, up)) = traffic {
        t.push((SYSTEM_TRAFFIC, down, Some(up)));
    }
    t
}

/// The line under the headline: on how many ports and from how many
/// processes the public listeners in `rows` are, which of those processes
/// are Windows components and which are not, and that the firewall and the
/// router still decide reachability. `owner_of` tells a PID's process and
/// `windir` is the Windows folder.
pub fn subtitle(rows: &[Row], owner_of: impl Fn(u32) -> Owner, windir: &str) -> String {
    let public: Vec<&Row> = rows.iter().filter(|r| r.is_public_listener()).collect();
    if public.is_empty() {
        return "No program accepts connections on every network interface.".into();
    }
    let ports: BTreeSet<u16> = public.iter().map(|r| r.local.port()).collect();
    let pids: BTreeSet<u32> = public.iter().map(|r| r.pid).collect();
    let owners: BTreeMap<u32, Owner> = pids.iter().map(|&p| (p, owner_of(p))).collect();
    let (mut windows, mut others) = ((0, Vec::new()), (0, Vec::new()));
    for (&pid, own) in &owners {
        let side = if is_windows_component(pid, &own.name, &own.exe, windir) { &mut windows } else { &mut others };
        side.0 += 1;
        if !side.1.contains(&own.name) {
            side.1.push(own.name.clone());
        }
    }
    let mut groups = Vec::new();
    if windows.0 > 0 {
        groups.push(group(windows.0, "Windows component", &windows.1));
    }
    if others.0 > 0 {
        groups.push(group(others.0, "other program", &others.1));
    }
    let they = if public.len() == 1 { "it is" } else { "they are" };
    format!(
        "On {}, from {}: {}. Firewall and router rules still determine whether {they} reachable.",
        plural(ports.len(), "port"),
        processes_text(pids.len()),
        groups.join(" and "),
    )
}

/// The Windows folder (`%SystemRoot%`).
fn windir() -> String {
    crate::app_folders::windows_dir().to_string_lossy().into_owned()
}

/// Draws the band for `inventory` (nothing while there is none or it could
/// not be read): the headline, its subtitle, "Show the Public scope" (which
/// sets `chosen` to Public) and the tiles, System traffic only when
/// `show_traffic`.
pub fn draw(ui: &mut Ui, page: &PageContext, inventory: Option<&Inventory>, chosen: &mut Scope, show_traffic: bool) {
    let Some(inv) = inventory.filter(|i| i.status != Status::Unavailable) else { return };
    let procs: HashMap<u32, &ProcRow> = page.sample.procs.iter().map(|p| (p.pid, p)).collect();
    let public = super::public_listeners(&inv.rows);
    let t = theme::of_ui(ui);
    // Amber while something listens on every interface.
    let mark = if public > 0 { Mark::Dot(t.amber) } else { Mark::Dot(t.green) };
    let line = headline(public);
    // Screen readers hear a summary of the chosen scope.
    let summary = a11y::summary_text(*chosen, &inv.rows);
    let windir = windir();
    let sub = subtitle(&inv.rows, |pid| owner(pid, &procs), &windir);
    let traffic = show_traffic.then(|| traffic_text(system_traffic(&page.sample.net), page.settings.bits_per_second));
    let tiles = tiles(inv, traffic);
    StatusBand::new(mark, &line).sub(&sub).spoken(&summary).actions_width(190.0).show(ui, |ui, area| match area {
        BandArea::Body => {}
        BandArea::Actions => {
            if primary_button(ui, "Show the Public scope", true).clicked() {
                *chosen = Scope::Public;
            }
        }
        BandArea::Below => {
            ui.add_space(2.0);
            let width = ui.available_width();
            let cols = if width >= 760.0 { tiles.len() } else { 3 };
            let gap = 10.0;
            let tile_w = (width - gap * (cols - 1) as f32) / cols as f32;
            for chunk in tiles.chunks(cols) {
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for (label, value, note) in chunk {
                        if *label == "Public" && public > 0 {
                            alert_tile(ui, tile_w, label, value, note.as_deref(), t.amber);
                        } else {
                            kpi_tile(ui, tile_w, label, value, note.as_deref(), false);
                        }
                    }
                });
            }
        }
    });
    ui.add_space(10.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{assemble, Proto, TcpState};

    #[test]
    fn the_headline_counts_the_public_listeners() {
        assert_eq!(headline(23), "23 sockets listen on all interfaces");
        assert_eq!(headline(1), "1 socket listens on all interfaces");
        assert_eq!(headline(0), "No socket listens on all interfaces");
    }

    const WINDIR: &str = r"C:\Windows";

    fn row(proto: Proto, local: &str, remote: Option<&str>, state: Option<TcpState>, pid: u32) -> Row {
        Row { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid }
    }

    fn listen(local: &str, pid: u32) -> Row {
        row(Proto::Tcp, local, None, Some(TcpState::Listening), pid)
    }

    /// PID 4 System, 900 svchost (Windows), 2000 node and 3000 Spotify
    /// (not Windows), 5000 a process the samples do not know.
    fn owner_of(pid: u32) -> Owner {
        let own = |name: &str, exe: &str| Owner { name: name.into(), exe: exe.into(), user: "u".into() };
        match pid {
            4 => own("System", ""),
            900 => own("svchost.exe", r"C:\WINDOWS\system32\svchost.exe"),
            2000 => own("node.exe", r"C:\Program Files\nodejs\node.exe"),
            3000 => own("Spotify.exe", r"C:\Users\me\AppData\Roaming\Spotify\Spotify.exe"),
            _ => own("unknown", ""),
        }
    }

    #[test]
    fn the_subtitle_tells_ports_processes_and_which_are_windows() {
        let rows = vec![
            listen("0.0.0.0:135", 900),
            listen("[::]:135", 900),
            listen("0.0.0.0:445", 4),
            listen("[::]:445", 4),
            listen("0.0.0.0:3000", 2000),
            listen("0.0.0.0:57621", 3000),
            // Not public: loopback, a connection and a UDP wildcard.
            listen("127.0.0.1:9000", 2000),
            row(Proto::Tcp, "192.168.1.5:50000", Some("140.82.112.3:443"), Some(TcpState::Established), 3000),
            row(Proto::Udp, "0.0.0.0:5353", None, None, 900),
        ];
        assert_eq!(
            subtitle(&rows, owner_of, WINDIR),
            "On 4 ports, from 4 processes: 2 Windows components (System, svchost.exe) and 2 other programs (node.exe, Spotify.exe). \
             Firewall and router rules still determine whether they are reachable."
        );
    }

    #[test]
    fn one_listener_is_told_in_the_singular() {
        assert_eq!(
            subtitle(&[listen("0.0.0.0:3000", 2000)], owner_of, WINDIR),
            "On 1 port, from 1 process: 1 other program (node.exe). \
             Firewall and router rules still determine whether it is reachable."
        );
        assert_eq!(
            subtitle(&[listen("[::]:445", 4)], owner_of, WINDIR),
            "On 1 port, from 1 process: 1 Windows component (System). \
             Firewall and router rules still determine whether it is reachable."
        );
    }

    #[test]
    fn with_no_public_listener_the_subtitle_says_so() {
        let rows = vec![listen("127.0.0.1:9000", 2000), row(Proto::Udp, "0.0.0.0:5353", None, None, 900)];
        assert_eq!(subtitle(&rows, owner_of, WINDIR), "No program accepts connections on every network interface.");
    }

    #[test]
    fn a_process_without_a_known_executable_is_not_a_windows_component() {
        assert_eq!(
            subtitle(&[listen("0.0.0.0:7000", 5000)], owner_of, WINDIR),
            "On 1 port, from 1 process: 1 other program (unknown). \
             Firewall and router rules still determine whether it is reachable."
        );
    }

    #[test]
    fn processes_counts_each_pid_once() {
        let rows = vec![
            listen("0.0.0.0:135", 900),
            listen("[::]:135", 900),
            row(Proto::Udp, "0.0.0.0:5353", None, None, 900),
            listen("127.0.0.1:9000", 2000),
            row(Proto::Tcp, "192.168.1.5:50000", Some("140.82.112.3:443"), Some(TcpState::Established), 3000),
        ];
        assert_eq!(processes(&rows), 3);
        assert_eq!(processes(&[]), 0);
    }

    #[test]
    fn system_traffic_sums_every_interface() {
        let net = |rx: f64, tx: f64| NetRow { rx_bps: rx, tx_bps: tx, ..Default::default() };
        assert_eq!(system_traffic(&[net(9000.0, 1000.0), net(500.0, 3000.0)]), (9500.0, 4000.0));
        assert_eq!(system_traffic(&[]), (0.0, 0.0));
    }

    #[test]
    fn system_traffic_reads_down_then_up_in_the_chosen_unit() {
        assert_eq!(
            traffic_text((1_887_437.0, 251_904.0), false),
            ("\u{2193} 1.80 MB/sec".to_string(), "\u{2191} 246 KB/sec".to_string())
        );
        assert_eq!(traffic_text((1250.0, 0.0), true), ("\u{2193} 10.0 Kbps".to_string(), "\u{2191} 0 bps".to_string()));
    }

    #[test]
    fn core_windows_processes_count_as_windows_without_a_known_executable() {
        assert!(is_windows_component(700, "lsass.exe", "", WINDIR));
        assert!(is_windows_component(650, "wininit.exe", "", WINDIR));
        assert!(is_windows_component(720, "Services.exe", "", WINDIR));
        assert!(!is_windows_component(5000, "unknown", "", WINDIR));
        assert!(!is_windows_component(2000, "node.exe", "", WINDIR));
        // A known path outside Windows wins over a Windows-like name.
        assert!(!is_windows_component(9000, "svchost.exe", r"C:\Users\me\svchost.exe", WINDIR));
    }

    #[test]
    fn the_five_tiles_count_the_inventory() {
        let rows = vec![
            listen("0.0.0.0:135", 900),
            listen("[::]:445", 4),
            listen("127.0.0.1:9000", 2000),
            row(Proto::Tcp, "192.168.1.5:50000", Some("140.82.112.3:443"), Some(TcpState::Established), 3000),
            row(Proto::Udp, "0.0.0.0:5353", None, None, 900),
        ];
        let mut inv = assemble([Ok(rows), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
        // Windows reported more rows than the inventory kept: the tile counts
        // the rows kept, as the All scope and the screen-reader summary do.
        inv.total = 20_003;
        assert_eq!(
            tiles(&inv, Some(("\u{2193} 9 KB/s".into(), "\u{2191} 4 KB/s".into()))),
            vec![
                ("Connections", "5".to_string(), None),
                ("Listening", "3".to_string(), None),
                ("Public", "2".to_string(), None),
                ("Processes", "4".to_string(), None),
                ("System traffic \u{2193}/\u{2191}", "\u{2193} 9 KB/s".to_string(), Some("\u{2191} 4 KB/s".to_string())),
            ]
        );
        // The System traffic tile turned off in the settings.
        assert_eq!(tiles(&inv, None).len(), 4);
    }

    #[test]
    fn past_three_names_the_rest_are_counted() {
        let rows: Vec<Row> = (0..5).map(|i| listen(&format!("0.0.0.0:{}", 8000 + i), 6000 + i)).collect();
        let named = |pid: u32| Owner { name: format!("app{}.exe", pid - 6000), exe: r"D:\apps\a.exe".into(), user: "u".into() };
        assert_eq!(
            subtitle(&rows, named, WINDIR),
            "On 5 ports, from 5 processes: 5 other programs (app0.exe, app1.exe, app2.exe and 2 more). \
             Firewall and router rules still determine whether they are reachable."
        );
    }
}
