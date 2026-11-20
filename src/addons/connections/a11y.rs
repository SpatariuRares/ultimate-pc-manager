//! The screen-reader texts: the scope summary, the scope
//! control and each row.

use std::collections::HashSet;

use super::inventory::Row;
use super::page::{Cells, Owner};
use super::scope;
use super::Scope;

/// What the summary announces for the rows of `rows` in `scope`:
/// "Scope %1. %2 connections, %3 listening, %4 public listeners, across
/// %5 processes. Local IPC inventory is unavailable on Windows."
pub fn summary_text(chosen: Scope, rows: &[Row]) -> String {
    let in_scope: Vec<Row> = rows.iter().filter(|r| scope::contains(chosen, r)).cloned().collect();
    let processes: HashSet<u32> = in_scope.iter().map(|r| r.pid).collect();
    format!(
        "Scope {}. {} connections, {} listening, {} public listeners, across {} processes. \
         Local IPC inventory is unavailable on Windows.",
        chosen.label(),
        in_scope.len(),
        scope::count(Scope::Listening, &in_scope),
        scope::count(Scope::Public, &in_scope),
        processes.len()
    )
}

/// What a row announces: "%1, PID %2. %3, %4. Local %5. Remote %6.
/// Exposure %7. Owner %8.", from the row's table `cells`.
pub fn row_text(row: &Row, c: &Cells, owner: &Owner) -> String {
    format!(
        "{}, PID {}. {}, {}. Local {}. Remote {}. Exposure {}. Owner {}.",
        c.application, row.pid, c.protocol, c.state, c.local, c.remote, c.exposure, owner.user
    )
}

/// What the scope control announces: "Connection scope: %1".
pub fn scope_text(scope: Scope) -> String {
    format!("Connection scope: {}", scope.label())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::connections::inventory::{Proto, TcpState};
    use crate::addons::connections::page::{cells, Owner};

    #[test]
    fn the_scope_control_names_the_chosen_scope() {
        assert_eq!(scope_text(Scope::All), "Connection scope: All");
        assert_eq!(scope_text(Scope::Udp), "Connection scope: UDP");
    }

    #[test]
    fn a_row_reads_every_column_and_its_owner() {
        let row = Row {
            proto: Proto::Tcp,
            local: "[2a02::5]:50123".parse().unwrap(),
            remote: Some("[2a00:1450::200e]:443".parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 1868,
        };
        let own = Owner { name: "svchost.exe".into(), exe: String::new(), user: "NETWORK SERVICE".into() };
        assert_eq!(
            row_text(&row, &cells(&row, &own, &own.name), &own),
            "svchost.exe, PID 1868. TCP6, Established. Local [2a02::5]:50123. \
             Remote [2a00:1450::200e]:443 \u{b7} HTTPS. Exposure Internet. Owner NETWORK SERVICE."
        );
    }

    #[test]
    fn a_udp_row_of_an_unknown_process_reads_its_gaps() {
        let row = Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 4242 };
        let own = Owner { name: "unknown".into(), exe: String::new(), user: "unknown".into() };
        assert_eq!(
            row_text(&row, &cells(&row, &own, &own.name), &own),
            "unknown, PID 4242. UDP4, Datagram. Local 0.0.0.0:5353. \
             Remote \u{2014}. Exposure All interfaces. Owner unknown."
        );
    }

    fn listener(local: &str, pid: u32) -> Row {
        Row { proto: Proto::Tcp, local: local.parse().unwrap(), remote: None, state: Some(TcpState::Listening), pid }
    }

    /// Two public listeners (PID 4 and 900), one loopback listener (PID 900),
    /// one established connection (PID 1868) and one UDP endpoint (PID 1868).
    fn rows() -> Vec<Row> {
        vec![
            listener("0.0.0.0:445", 4),
            listener("[::]:8443", 900),
            listener("127.0.0.1:631", 900),
            Row {
                proto: Proto::Tcp,
                local: "192.168.1.5:50123".parse().unwrap(),
                remote: Some("142.250.180.14:443".parse().unwrap()),
                state: Some(TcpState::Established),
                pid: 1868,
            },
            Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 1868 },
        ]
    }

    #[test]
    fn the_summary_counts_the_whole_inventory_in_scope_all() {
        assert_eq!(
            summary_text(Scope::All, &rows()),
            "Scope All. 5 connections, 3 listening, 2 public listeners, across 3 processes. \
             Local IPC inventory is unavailable on Windows."
        );
    }

    #[test]
    fn the_summary_counts_only_the_rows_in_the_chosen_scope() {
        assert_eq!(
            summary_text(Scope::Udp, &rows()),
            "Scope UDP. 1 connections, 0 listening, 0 public listeners, across 1 processes. \
             Local IPC inventory is unavailable on Windows."
        );
        assert_eq!(
            summary_text(Scope::Listening, &rows()),
            "Scope Listening. 3 connections, 3 listening, 2 public listeners, across 2 processes. \
             Local IPC inventory is unavailable on Windows."
        );
    }
}
