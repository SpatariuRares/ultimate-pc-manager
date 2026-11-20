//! The table's rows grouped by exposure (design A), most exposed first,
//! each group sorted by the chosen column; long groups show their first
//! rows and a "Show N more" line. No UI here.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};

use super::inventory::{self, Exposure, Proto, Row};
use super::SortColumn;

/// What identifies a connection from one reading to the next: its state
/// may change (Established to Close wait), its ends and owner do not.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    proto: Proto,
    local: SocketAddr,
    remote: Option<SocketAddr>,
    pid: u32,
}

/// The identity of `row`.
pub fn key(row: &Row) -> Key {
    Key { proto: row.proto, local: row.local, remote: row.remote, pid: row.pid }
}

/// The index of the row with `key` in `rows`, if that connection still exists.
pub fn find(rows: &[Row], key: &Key) -> Option<usize> {
    rows.iter().position(|r| self::key(r) == *key)
}

/// A group's place in [`ORDER`].
fn rank(exposure: Exposure) -> usize {
    ORDER.iter().position(|e| *e == exposure).unwrap_or(ORDER.len())
}

/// The value a row is sorted by.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum SortKey {
    Text(String),
    /// Port first, so "sort by port" reads naturally; then the address.
    Local(u16, IpAddr),
    /// Rows without a remote end last.
    Remote(bool, Option<SocketAddr>),
    Rank(usize),
}

fn sort_key(row: &Row, column: SortColumn, name: &impl Fn(&Row) -> String) -> SortKey {
    match column {
        SortColumn::Application => SortKey::Text(name(row).to_lowercase()),
        SortColumn::Protocol => SortKey::Text(inventory::protocol_text(row).into()),
        SortColumn::State => SortKey::Text(inventory::state_text(row).into()),
        SortColumn::Local => SortKey::Local(row.local.port(), row.local.ip()),
        SortColumn::Remote => SortKey::Remote(row.remote.is_none(), row.remote),
        SortColumn::Exposure => SortKey::Rank(rank(inventory::exposure(row))),
    }
}

/// The groups' order: the most exposed first.
pub const ORDER: [Exposure; 5] =
    [Exposure::AllInterfaces, Exposure::Internet, Exposure::LocalNetwork, Exposure::Loopback, Exposure::Unspecified];

/// Rows of the table under one title.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Group {
    /// The rows' exposure; None when the rows are not grouped (no title).
    pub exposure: Option<Exposure>,
    /// Indices into the inventory's rows, in display order.
    pub rows: Vec<usize>,
}

/// The rows at `indices` of `rows`, sorted by `column` (`descending` from
/// the last value) and, when `grouped`, split by exposure in [`ORDER`]
/// (empty groups left out). `name` gives a row's application name.
pub fn arrange(
    rows: &[Row],
    indices: impl IntoIterator<Item = usize>,
    column: SortColumn,
    descending: bool,
    grouped: bool,
    name: impl Fn(&Row) -> String,
) -> Vec<Group> {
    let mut keyed: Vec<(SortKey, usize)> = indices.into_iter().map(|i| (sort_key(&rows[i], column, &name), i)).collect();
    // Equal values keep a stable order: by the ends, then the PID.
    keyed.sort_by(|(ka, a), (kb, b)| {
        let (ra, rb) = (&rows[*a], &rows[*b]);
        let by_value = if descending { kb.cmp(ka) } else { ka.cmp(kb) };
        by_value.then_with(|| (ra.local, ra.remote, ra.pid).cmp(&(rb.local, rb.remote, rb.pid)))
    });
    let sorted = keyed.into_iter().map(|(_, i)| i);
    if !grouped {
        return vec![Group { exposure: None, rows: sorted.collect() }];
    }
    let mut by_exposure: Vec<Vec<usize>> = vec![Vec::new(); ORDER.len()];
    for i in sorted {
        by_exposure[rank(inventory::exposure(&rows[i])).min(ORDER.len() - 1)].push(i);
    }
    ORDER
        .iter()
        .zip(by_exposure)
        .filter(|(_, r)| !r.is_empty())
        .map(|(e, rows)| Group { exposure: Some(*e), rows })
        .collect()
}

/// One line of the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    /// A group's title line (the group's index).
    Title(usize),
    /// An inventory row (its index).
    Row(usize),
    /// "Show N more" under a group (the group's index, N).
    More(usize, usize),
}

/// The rows a collapsed group shows: Loopback (programs on this PC) starts
/// folded, the others show their first six.
pub fn preview(exposure: Exposure) -> usize {
    match exposure {
        Exposure::Loopback => 0,
        _ => 6,
    }
}

/// The table's lines: each group's title, its first rows (all of them when
/// `expanded` holds its exposure) and "Show N more" for the rest.
pub fn lines(groups: &[Group], expanded: &HashSet<Exposure>) -> Vec<Item> {
    let mut out = Vec::new();
    for (g, group) in groups.iter().enumerate() {
        let shown = match group.exposure {
            None => group.rows.len(),
            Some(e) => {
                out.push(Item::Title(g));
                if expanded.contains(&e) {
                    group.rows.len()
                } else {
                    preview(e).min(group.rows.len())
                }
            }
        };
        out.extend(group.rows[..shown].iter().map(|&i| Item::Row(i)));
        if shown < group.rows.len() {
            out.push(Item::More(g, group.rows.len() - shown));
        }
    }
    out
}

/// The text of a "Show N more" line: "Show 111" when the group shows none.
pub fn more_text(hidden: usize, total: usize) -> String {
    if hidden == total {
        format!("Show {hidden}")
    } else {
        format!("Show {hidden} more")
    }
}

/// The hint after a group's title and count (design A).
pub fn hint(exposure: Exposure, rows: &[Row], group: &[usize]) -> String {
    match exposure {
        Exposure::AllInterfaces => {
            let tcp = group.iter().filter(|&&i| rows[i].is_tcp_listener()).count();
            let udp = group.iter().filter(|&&i| rows[i].proto == Proto::Udp).count();
            format!("Public: {} and {}", plural(tcp, "TCP listener"), plural(udp, "UDP socket"))
        }
        Exposure::Internet => "connections going out".into(),
        Exposure::LocalNetwork => match common_subnet(rows, group) {
            Some([a, b, c]) => format!("devices at home, {a}.{b}.{c}.x"),
            None => "devices at home".into(),
        },
        Exposure::Loopback => "programs on this PC talking to each other".into(),
        Exposure::Unspecified => "not enough socket data to classify".into(),
    }
}

fn plural(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// The IPv4 /24 most of the group's rows reach (the remote end, or the
/// local one for listeners and UDP); ties go to the lowest.
fn common_subnet(rows: &[Row], group: &[usize]) -> Option<[u8; 3]> {
    let mut seen: HashMap<[u8; 3], usize> = HashMap::new();
    for &i in group {
        let r = &rows[i];
        let ip = r.remote.unwrap_or(r.local).ip();
        let v4 = match ip {
            IpAddr::V4(a) => Some(a),
            IpAddr::V6(a) => a.to_ipv4_mapped(),
        };
        if let Some(a) = v4 {
            let [x, y, z, _] = a.octets();
            *seen.entry([x, y, z]).or_default() += 1;
        }
    }
    seen.into_iter().max_by(|(sa, na), (sb, nb)| na.cmp(nb).then(sb.cmp(sa))).map(|(s, _)| s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::TcpState;

    fn tcp(local: &str, remote: Option<&str>, state: TcpState, pid: u32) -> Row {
        Row { proto: Proto::Tcp, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state: Some(state), pid }
    }

    fn udp(local: &str, pid: u32) -> Row {
        Row { proto: Proto::Udp, local: local.parse().unwrap(), remote: None, state: None, pid }
    }

    fn by_pid(r: &Row) -> String {
        match r.pid {
            4 => "System".into(),
            10 => "steam".into(),
            20 => "Claude".into(),
            _ => "svchost.exe".into(),
        }
    }

    /// One row of each exposure, in an order unlike the groups'.
    fn mixed() -> Vec<Row> {
        vec![
            tcp("127.0.0.1:5000", None, TcpState::Listening, 30),                                    // 0 Loopback
            tcp("192.168.1.5:50000", Some("160.79.104.10:443"), TcpState::Established, 20),          // 1 Internet
            tcp("192.168.1.5:50001", Some("192.168.1.51:8009"), TcpState::Established, 10),          // 2 Local network
            tcp("[::]:445", None, TcpState::Listening, 4),                                           // 3 All interfaces
            tcp("192.168.1.5:50002", Some("0.0.0.0:0"), TcpState::SynSent, 30),                       // 4 Unspecified
            udp("0.0.0.0:5353", 30),                                                                 // 5 All interfaces
        ]
    }

    fn all(rows: &[Row]) -> std::ops::Range<usize> {
        0..rows.len()
    }

    #[test]
    fn a_connection_keeps_its_identity_when_its_state_changes() {
        let before = tcp("192.168.1.5:50000", Some("160.79.104.10:443"), TcpState::Established, 20);
        let after = Row { state: Some(TcpState::CloseWait), ..before.clone() };
        assert_eq!(key(&before), key(&after));
    }

    #[test]
    fn another_end_or_owner_is_another_connection() {
        let a = tcp("192.168.1.5:50000", Some("160.79.104.10:443"), TcpState::Established, 20);
        for b in [
            Row { pid: 21, ..a.clone() },
            Row { local: "192.168.1.5:50001".parse().unwrap(), ..a.clone() },
            Row { remote: Some("160.79.104.11:443".parse().unwrap()), ..a.clone() },
            Row { proto: Proto::Udp, ..a.clone() },
        ] {
            assert_ne!(key(&a), key(&b), "{b:?}");
        }
    }

    #[test]
    fn the_selection_is_found_again_in_a_new_reading_until_it_is_gone() {
        let old = mixed();
        let picked = key(&old[1]);
        let mut new = old.clone();
        new.remove(0);
        new[0].state = Some(TcpState::CloseWait);
        assert_eq!(find(&new, &picked), Some(0));
        new.remove(0);
        assert_eq!(find(&new, &picked), None);
    }

    #[test]
    fn groups_come_most_exposed_first() {
        let rows = mixed();
        let g = arrange(&rows, all(&rows), SortColumn::Application, false, true, by_pid);
        let order: Vec<_> = g.iter().map(|g| g.exposure.unwrap()).collect();
        assert_eq!(
            order,
            [Exposure::AllInterfaces, Exposure::Internet, Exposure::LocalNetwork, Exposure::Loopback, Exposure::Unspecified]
        );
        assert_eq!(g[0].rows, [5, 3]); // svchost.exe before System
    }

    #[test]
    fn empty_groups_are_left_out_and_only_the_given_rows_are_listed() {
        let rows = mixed();
        let g = arrange(&rows, [0, 1], SortColumn::Application, false, true, by_pid);
        assert_eq!(
            g,
            [Group { exposure: Some(Exposure::Internet), rows: vec![1] }, Group { exposure: Some(Exposure::Loopback), rows: vec![0] }]
        );
    }

    #[test]
    fn ungrouped_rows_are_one_untitled_list_sorted_across_exposures() {
        let rows = mixed();
        let g = arrange(&rows, all(&rows), SortColumn::Application, false, false, by_pid);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].exposure, None);
        // Claude, steam, the svchost.exe rows, System.
        assert_eq!(&g[0].rows[..2], [1, 2]);
        assert_eq!(g[0].rows.last(), Some(&3));
        assert_eq!(g[0].rows.len(), 6);
    }

    #[test]
    fn application_sorting_ignores_case_and_descending_reverses_it() {
        let rows = vec![udp("0.0.0.0:1", 4), udp("0.0.0.0:2", 10), udp("0.0.0.0:3", 20)];
        let asc = arrange(&rows, all(&rows), SortColumn::Application, false, true, by_pid);
        assert_eq!(asc[0].rows, [2, 1, 0]); // Claude, steam, System
        let desc = arrange(&rows, all(&rows), SortColumn::Application, true, true, by_pid);
        assert_eq!(desc[0].rows, [0, 1, 2]);
    }

    #[test]
    fn local_endpoints_sort_by_port_then_address() {
        let rows = vec![udp("0.0.0.0:5353", 1), udp("[::]:135", 1), udp("0.0.0.0:445", 1), udp("0.0.0.0:135", 1)];
        let g = arrange(&rows, all(&rows), SortColumn::Local, false, true, by_pid);
        assert_eq!(g[0].rows, [3, 1, 2, 0]);
    }

    #[test]
    fn rows_without_a_remote_end_sort_last_by_remote() {
        let rows = vec![
            tcp("10.0.0.2:1", None, TcpState::Listening, 1),
            tcp("10.0.0.2:2", Some("10.0.0.9:80"), TcpState::Established, 1),
            tcp("10.0.0.2:3", Some("10.0.0.3:443"), TcpState::Established, 1),
        ];
        let g = arrange(&rows, all(&rows), SortColumn::Remote, false, false, by_pid);
        assert_eq!(g[0].rows, [2, 1, 0]);
    }

    #[test]
    fn exposure_sorting_follows_the_groups_order() {
        let rows = mixed();
        let g = arrange(&rows, all(&rows), SortColumn::Exposure, false, false, by_pid);
        let exp: Vec<_> = g[0].rows.iter().map(|&i| inventory::exposure(&rows[i])).collect();
        assert_eq!(
            exp,
            [
                Exposure::AllInterfaces,
                Exposure::AllInterfaces,
                Exposure::Internet,
                Exposure::LocalNetwork,
                Exposure::Loopback,
                Exposure::Unspecified
            ]
        );
    }

    #[test]
    fn a_long_group_shows_its_first_six_rows_and_n_more() {
        let groups = [Group { exposure: Some(Exposure::Internet), rows: (0..10).collect() }];
        let l = lines(&groups, &HashSet::new());
        assert_eq!(l[0], Item::Title(0));
        assert_eq!(&l[1..7], (0..6).map(Item::Row).collect::<Vec<_>>());
        assert_eq!(l[7], Item::More(0, 4));
        assert_eq!(l.len(), 8);
    }

    #[test]
    fn an_expanded_or_short_group_shows_every_row_and_no_more_line() {
        let groups = [
            Group { exposure: Some(Exposure::Internet), rows: (0..10).collect() },
            Group { exposure: Some(Exposure::LocalNetwork), rows: vec![10, 11] },
        ];
        let l = lines(&groups, &HashSet::from([Exposure::Internet]));
        assert_eq!(l.len(), 1 + 10 + 1 + 2);
        assert!(!l.iter().any(|i| matches!(i, Item::More(..))));
    }

    #[test]
    fn loopback_starts_folded() {
        let groups = [Group { exposure: Some(Exposure::Loopback), rows: (0..111).collect() }];
        assert_eq!(lines(&groups, &HashSet::new()), [Item::Title(0), Item::More(0, 111)]);
        assert_eq!(more_text(111, 111), "Show 111");
        assert_eq!(more_text(49, 55), "Show 49 more");
    }

    #[test]
    fn ungrouped_rows_have_no_title_and_are_never_folded() {
        let groups = [Group { exposure: None, rows: (0..50).collect() }];
        assert_eq!(lines(&groups, &HashSet::new()), (0..50).map(Item::Row).collect::<Vec<_>>());
    }

    #[test]
    fn each_group_has_the_hint_of_design_a() {
        let rows = vec![
            tcp("[::]:445", None, TcpState::Listening, 4),
            tcp("0.0.0.0:135", None, TcpState::Listening, 1),
            udp("0.0.0.0:5353", 1),
            tcp("192.168.178.165:1", Some("192.168.178.51:8009"), TcpState::Established, 1),
            tcp("192.168.178.165:2", Some("192.168.178.109:8009"), TcpState::CloseWait, 1),
            tcp("10.0.0.5:3", Some("10.0.0.1:53"), TcpState::Established, 1),
        ];
        assert_eq!(hint(Exposure::AllInterfaces, &rows, &[0, 1, 2]), "Public: 2 TCP listeners and 1 UDP socket");
        assert_eq!(hint(Exposure::Internet, &rows, &[]), "connections going out");
        assert_eq!(hint(Exposure::LocalNetwork, &rows, &[3, 4, 5]), "devices at home, 192.168.178.x");
        assert_eq!(hint(Exposure::LocalNetwork, &rows, &[]), "devices at home");
        assert_eq!(hint(Exposure::Loopback, &rows, &[]), "programs on this PC talking to each other");
        assert_eq!(hint(Exposure::Unspecified, &rows, &[]), "not enough socket data to classify");
    }
}
