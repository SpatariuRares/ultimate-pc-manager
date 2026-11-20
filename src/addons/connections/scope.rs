//! Which rows a scope holds, the count beside each scope, and the Find box
//! (an addition) combined with the scope. Pure, no UI.

use super::inventory::{Proto, Row, TcpState};
use super::page::Cells;
use super::Scope;

/// The scope Windows cannot fill, shown disabled after the others.
pub const LOCAL_IPC: &str = "Local IPC";
/// Why Local IPC is disabled.
pub const LOCAL_IPC_NOTE: &str = "unavailable on Windows";
/// Shown when the scope and Find leave no row.
pub const NO_MATCH: &str = "No connections match";

/// Whether `row` belongs to `scope`.
pub fn contains(scope: Scope, row: &Row) -> bool {
    match scope {
        Scope::All => true,
        Scope::Connected => {
            row.proto == Proto::Tcp
                && !matches!(row.state, None | Some(TcpState::Listening | TcpState::Closed | TcpState::DeleteTcb))
        }
        Scope::Listening => row.is_tcp_listener(),
        Scope::Public => row.is_public_listener(),
        Scope::Udp => row.proto == Proto::Udp,
    }
}

/// The rows of `rows` in `scope`.
pub fn count(scope: Scope, rows: &[Row]) -> usize {
    rows.iter().filter(|r| contains(scope, r)).count()
}

/// Whether a row with `cells` (application, PID, endpoints, state) matches
/// the Find text `query`; an empty query matches every row. "PID n" (what
/// Go to connections types) matches the process with that PID only.
pub fn found(cells: &Cells, query: &str) -> bool {
    let q = query.trim().to_lowercase();
    if q.strip_prefix("pid ").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())) {
        let detail = cells.detail.to_lowercase();
        return detail == q || detail.ends_with(&format!("\u{b7} {q}"));
    }
    q.is_empty()
        || [cells.application.as_str(), &cells.detail, &cells.local, &cells.remote, cells.state]
            .iter()
            .any(|t| t.to_lowercase().contains(&q))
}

/// The indexes of the rows in `scope` that match the Find text `query`;
/// `cells` gives a row's texts and is called only for rows in the scope,
/// and only when there is something to find.
pub fn shown(rows: &[Row], scope: Scope, query: &str, cells: impl Fn(&Row) -> Cells) -> Vec<usize> {
    let finding = !query.trim().is_empty();
    rows.iter()
        .enumerate()
        .filter(|(_, r)| contains(scope, r) && (!finding || found(&cells(r), query)))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(proto: Proto, local: &str, remote: Option<&str>, state: Option<TcpState>) -> Row {
        Row { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid: 4 }
    }

    fn listener(local: &str) -> Row {
        row(Proto::Tcp, local, None, Some(TcpState::Listening))
    }

    fn connection(state: TcpState) -> Row {
        row(Proto::Tcp, "192.168.1.5:50000", Some("140.82.112.3:443"), Some(state))
    }

    fn udp(local: &str) -> Row {
        row(Proto::Udp, local, None, None)
    }

    fn scopes_of(r: &Row) -> Vec<Scope> {
        Scope::ALL.into_iter().filter(|s| contains(*s, r)).collect()
    }

    #[test]
    fn connected_is_every_tcp_state_with_a_remote_end() {
        use TcpState::*;
        for s in [SynSent, SynReceived, Established, FinWait1, FinWait2, CloseWait, Closing, LastAck, TimeWait] {
            assert_eq!(scopes_of(&connection(s)), [Scope::All, Scope::Connected], "{s:?}");
        }
        for s in [Closed, DeleteTcb] {
            assert_eq!(scopes_of(&connection(s)), [Scope::All], "{s:?} is not connected");
        }
    }

    #[test]
    fn public_is_a_tcp_listener_on_all_interfaces() {
        assert_eq!(scopes_of(&listener("0.0.0.0:135")), [Scope::All, Scope::Listening, Scope::Public]);
        assert_eq!(scopes_of(&listener("[::]:445")), [Scope::All, Scope::Listening, Scope::Public]);
        assert_eq!(scopes_of(&listener("127.0.0.1:80")), [Scope::All, Scope::Listening]);
        assert_eq!(scopes_of(&listener("192.168.1.5:3389")), [Scope::All, Scope::Listening]);
    }

    #[test]
    fn a_udp_wildcard_is_only_udp_not_public() {
        assert_eq!(scopes_of(&udp("0.0.0.0:5353")), [Scope::All, Scope::Udp]);
        assert_eq!(scopes_of(&udp("[::1]:1900")), [Scope::All, Scope::Udp]);
    }

    /// The ledger's reading: 254 rows, 128 connected, 54 listening
    /// (23 on all interfaces) and 72 UDP.
    fn ledger_rows() -> Vec<Row> {
        let mut rows = Vec::new();
        rows.extend((0..23).map(|i| listener(&format!("0.0.0.0:{}", 1000 + i))));
        rows.extend((0..31).map(|i| listener(&format!("127.0.0.1:{}", 2000 + i))));
        rows.extend((0..100).map(|_| connection(TcpState::Established)));
        rows.extend((0..28).map(|_| connection(TcpState::TimeWait)));
        rows.extend((0..72).map(|i| udp(&format!("[::]:{}", 3000 + i))));
        rows
    }

    #[test]
    fn the_counts_match_the_ledger() {
        let rows = ledger_rows();
        let counts: Vec<(Scope, usize)> = Scope::ALL.into_iter().map(|s| (s, count(s, &rows))).collect();
        assert_eq!(
            counts,
            [(Scope::All, 254), (Scope::Connected, 128), (Scope::Listening, 54), (Scope::Public, 23), (Scope::Udp, 72)]
        );
    }

    fn cells(application: &str, pid: u32, local: &str, remote: &str, state: &'static str) -> Cells {
        Cells {
            application: application.into(),
            detail: format!("PID {pid}"),
            protocol: "TCP4",
            state,
            local: local.into(),
            remote: remote.into(),
            exposure: "Internet",
        }
    }

    #[test]
    fn find_looks_at_application_address_port_state_and_pid() {
        let c = cells("svchost.exe", 1868, "192.168.1.5:50000", "140.82.112.3:443", "Established");
        for q in ["", "SVCHOST", "140.82", "192.168.1.5", ":443", "50000", "established", "1868", "pid 1868", "  svchost  "] {
            assert!(found(&c, q), "{q:?} finds it");
        }
        for q in ["chrome", "10.0.0.1", "Listening", "9999"] {
            assert!(!found(&c, q), "{q:?} does not");
        }
    }

    #[test]
    fn find_for_pid_n_matches_that_process_only() {
        let c = cells("svchost.exe", 1868, "192.168.1.5:50000", "140.82.112.3:443", "Established");
        let with_exe = Cells { detail: "C:\\Windows\\System32\\svchost.exe \u{b7} PID 1868".into(), ..c.clone() };
        for q in ["PID 1868", "pid 1868", " PID 1868 "] {
            assert!(found(&c, q) && found(&with_exe, q), "{q:?} finds it");
        }
        for q in ["PID 186", "PID 868", "PID 18680"] {
            assert!(!found(&c, q) && !found(&with_exe, q), "{q:?} is another process");
        }
        assert!(found(&c, "186"), "a bare number still finds part of a PID or port");
    }

    #[test]
    fn find_works_inside_the_chosen_scope() {
        let rows = vec![
            listener("0.0.0.0:135"),
            connection(TcpState::Established),
            Row { pid: 9000, ..listener("0.0.0.0:8080") },
            udp("0.0.0.0:5353"),
        ];
        let cells_of = |r: &Row| {
            let app = if r.pid == 9000 { "node.exe" } else { "svchost.exe" };
            let remote = r.remote.map(|a| a.to_string()).unwrap_or_default();
            cells(app, r.pid, &r.local.to_string(), &remote, "")
        };
        assert_eq!(shown(&rows, Scope::Public, "svchost", cells_of), [0], "the svchost connection and UDP socket are outside Public");
        assert_eq!(shown(&rows, Scope::All, "svchost", cells_of), [0, 1, 3]);
        assert_eq!(shown(&rows, Scope::Public, "", cells_of), [0, 2]);
        assert!(shown(&rows, Scope::Udp, "node", cells_of).is_empty(), "nothing: the page says {NO_MATCH:?}");
    }
}
