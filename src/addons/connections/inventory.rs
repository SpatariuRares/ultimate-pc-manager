//! The connection inventory: the four IP Helper tables (TCP and UDP, IPv4
//! and IPv6, each with the owning PID) read into one list of rows, and the
//! pure rules over a row (exposure, protocol, state, endpoint and Remote /
//! service texts, the well-known port names). [`read`] also reads which
//! services each process runs. No UI here; the add-on's worker calls it.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use super::groups::Key;

/// The transport of a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Proto {
    Tcp,
    Udp,
}

/// A TCP state as Windows reports it (MIB_TCP_STATE 1..=12).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum TcpState {
    Closed,
    Listening,
    SynSent,
    SynReceived,
    Established,
    FinWait1,
    FinWait2,
    CloseWait,
    Closing,
    LastAck,
    TimeWait,
    DeleteTcb,
}

/// How far a socket reaches.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Exposure {
    Internet,
    AllInterfaces,
    LocalNetwork,
    Loopback,
    Unspecified,
}

/// One socket of the inventory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub proto: Proto,
    pub local: SocketAddr,
    /// The other end of a TCP connection; None for UDP and listeners.
    pub remote: Option<SocketAddr>,
    /// The TCP state; None for UDP (a datagram socket).
    pub state: Option<TcpState>,
    pub pid: u32,
}

/// The System Idle Process: Windows itself (Time wait rows are often its).
pub const SYSTEM_IDLE_PID: u32 = 0;
/// System: Windows itself (the kernel's sockets, e.g. SMB on 445).
pub const SYSTEM_PID: u32 = 4;

impl Row {
    /// A TCP socket listening for connections.
    pub fn is_tcp_listener(&self) -> bool {
        self.proto == Proto::Tcp && self.state == Some(TcpState::Listening)
    }

    /// A TCP listener on all interfaces (`0.0.0.0` or `[::]`): what could
    /// be reachable from outside. UDP wildcards do not count.
    pub fn is_public_listener(&self) -> bool {
        self.is_tcp_listener() && classify(self.local.ip(), true) == Exposure::AllInterfaces
    }
}

/// Whether the inventory holds everything Windows reported.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// All four tables read, within the row limit.
    Full,
    /// A table failed, or the rows were cut at [`ROW_LIMIT`].
    Partial,
    /// No table could be read.
    Unavailable,
}

/// The most rows an inventory keeps.
pub const ROW_LIMIT: usize = 20_000;

/// One reading of the four tables.
#[derive(Clone, Debug)]
pub struct Inventory {
    /// At most [`ROW_LIMIT`] rows: TCP4, TCP6, UDP4, UDP6 in that order.
    pub rows: Vec<Row>,
    pub status: Status,
    /// The rows Windows reported, before the limit cut them.
    pub total: usize,
    /// When it was read.
    pub at: Instant,
    /// How old each socket was at `at`, by the creation time Windows
    /// reports (not every socket has one).
    pub ages: HashMap<Key, Duration>,
    /// The running services of each process, by PID (what a svchost runs).
    pub services: HashMap<u32, Vec<String>>,
}

/// How old a socket created at `created` (a FILETIME) is at `now`; None
/// when Windows gives no creation time (0).
pub fn socket_age(created: i64, now: i64) -> Option<Duration> {
    if created <= 0 {
        return None;
    }
    let steps = now.saturating_sub(created).max(0) as u64;
    Some(Duration::from_nanos(steps.saturating_mul(100)))
}

/// A table that could not be read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TableError;

/// The exposure of an address: `wildcard` says whether 0.0.0.0 / :: means
/// "every interface" (a listener's or UDP socket's local end) or "no data"
/// (a connection's remote end).
pub fn classify(ip: IpAddr, wildcard: bool) -> Exposure {
    match ip {
        IpAddr::V4(a) => classify_v4(a, wildcard),
        IpAddr::V6(a) => match a.to_ipv4_mapped() {
            Some(v4) => classify_v4(v4, wildcard),
            None => classify_v6(a, wildcard),
        },
    }
}

fn classify_v4(a: Ipv4Addr, wildcard: bool) -> Exposure {
    let [b0, b1, ..] = a.octets();
    if a.is_unspecified() {
        if wildcard {
            Exposure::AllInterfaces
        } else {
            Exposure::Unspecified
        }
    } else if b0 == 127 {
        Exposure::Loopback
    } else if b0 == 10 || (b0 == 172 && (16..=31).contains(&b1)) || (b0 == 192 && b1 == 168) || (b0 == 169 && b1 == 254) {
        Exposure::LocalNetwork
    } else {
        Exposure::Internet
    }
}

fn classify_v6(a: Ipv6Addr, wildcard: bool) -> Exposure {
    let first = a.segments()[0];
    if a.is_unspecified() {
        if wildcard {
            Exposure::AllInterfaces
        } else {
            Exposure::Unspecified
        }
    } else if a.is_loopback() {
        Exposure::Loopback
    } else if first & 0xffc0 == 0xfe80 || first & 0xfe00 == 0xfc00 {
        // fe80::/10 link-local, fc00::/7 unique local
        Exposure::LocalNetwork
    } else {
        Exposure::Internet
    }
}

/// The exposure of a row: a connection by its remote end, a listener or
/// UDP socket by its local end.
pub fn exposure(row: &Row) -> Exposure {
    match row.proto {
        Proto::Udp => classify(row.local.ip(), true),
        Proto::Tcp if row.is_tcp_listener() => classify(row.local.ip(), true),
        Proto::Tcp => match row.remote {
            Some(r) => classify(r.ip(), false),
            None => Exposure::Unspecified,
        },
    }
}

impl TcpState {
    /// The state of a MIB_TCP_STATE value; None for anything else (e.g.
    /// Bound), which the inventory leaves out.
    pub fn from_mib(n: u32) -> Option<TcpState> {
        use TcpState::*;
        Some(match n {
            1 => Closed,
            2 => Listening,
            3 => SynSent,
            4 => SynReceived,
            5 => Established,
            6 => FinWait1,
            7 => FinWait2,
            8 => CloseWait,
            9 => Closing,
            10 => LastAck,
            11 => TimeWait,
            12 => DeleteTcb,
            _ => return None,
        })
    }

    /// The state as Windows names it.
    pub fn label(self) -> &'static str {
        use TcpState::*;
        match self {
            Closed => "Closed",
            Listening => "Listening",
            SynSent => "SYN sent",
            SynReceived => "SYN received",
            Established => "Established",
            FinWait1 => "FIN wait 1",
            FinWait2 => "FIN wait 2",
            CloseWait => "Close wait",
            Closing => "Closing",
            LastAck => "Last ACK",
            TimeWait => "Time wait",
            DeleteTcb => "Delete TCB",
        }
    }
}

impl Exposure {
    /// The shown name.
    pub const fn label(self) -> &'static str {
        match self {
            Exposure::Internet => "Internet",
            Exposure::AllInterfaces => "All interfaces",
            Exposure::LocalNetwork => "Local network",
            Exposure::Loopback => "Loopback",
            Exposure::Unspecified => "Unspecified",
        }
    }

    /// The explanation of the exposure (the side panel's note).
    pub const fn note(self) -> &'static str {
        match self {
            Exposure::AllInterfaces => {
                "This socket accepts traffic on every matching network interface. Firewall and router rules still determine whether it is reachable."
            }
            Exposure::Loopback => "This socket is restricted to software running on this computer.",
            Exposure::LocalNetwork => "This endpoint is on a private, link-local, or local IPv6 network.",
            Exposure::Internet => {
                "The remote endpoint is outside the local address ranges. A shared or CDN address may represent more than one service."
            }
            Exposure::Unspecified => "The available socket data is not sufficient to classify exposure.",
        }
    }
}

/// "TCP4", "TCP6", "UDP4" or "UDP6".
pub fn protocol_text(row: &Row) -> &'static str {
    match (row.proto, row.local.is_ipv6()) {
        (Proto::Tcp, false) => "TCP4",
        (Proto::Tcp, true) => "TCP6",
        (Proto::Udp, false) => "UDP4",
        (Proto::Udp, true) => "UDP6",
    }
}

/// The TCP state, or "Datagram" for UDP.
pub fn state_text(row: &Row) -> &'static str {
    match (row.proto, row.state) {
        (Proto::Tcp, Some(s)) => s.label(),
        _ => "Datagram",
    }
}

/// An address and port, IPv6 in brackets.
pub fn endpoint_text(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(a) => format!("{}:{}", a.ip(), a.port()),
        SocketAddr::V6(a) => format!("[{}]:{}", a.ip(), a.port()),
    }
}

/// The remote end, or a dash when there is none (UDP, listeners).
pub fn remote_text(row: &Row) -> String {
    row.remote.as_ref().map_or_else(|| "\u{2014}".to_string(), endpoint_text)
}

/// The well-known service of a port (our own table of common ports).
pub fn service_name(port: u16) -> Option<&'static str> {
    Some(match port {
        20 | 21 => "FTP",
        22 => "SSH",
        23 => "Telnet",
        25 | 587 => "SMTP",
        53 => "DNS",
        67 | 68 => "DHCP",
        80 => "HTTP",
        110 => "POP3",
        123 => "NTP",
        135 => "RPC",
        137..=139 => "NetBIOS",
        143 => "IMAP",
        389 => "LDAP",
        443 => "HTTPS",
        445 => "SMB",
        465 => "SMTPS",
        500 | 4500 => "IPsec",
        631 => "IPP",
        636 => "LDAPS",
        993 => "IMAPS",
        995 => "POP3S",
        1433 => "SQL Server",
        1900 => "SSDP",
        2179 => "Hyper-V",
        3306 => "MySQL",
        3389 => "RDP",
        3702 => "WS-Discovery",
        5353 => "mDNS",
        5355 => "LLMNR",
        5432 => "PostgreSQL",
        5985 | 5986 => "WinRM",
        7680 => "Delivery Optimization",
        8080 => "HTTP alternate",
        _ => return None,
    })
}

/// The "Remote / service" column: "address:port · service" for a
/// connection (the remote port's service, else the local one's), "port ·
/// service" (or the port alone) for a listener, a dash for UDP.
pub fn remote_service_text(row: &Row) -> String {
    let with = |text: String, service: Option<&str>| match service {
        Some(s) => format!("{text} \u{b7} {s}"),
        None => text,
    };
    match (row.proto, row.remote, row.state) {
        (Proto::Tcp, Some(remote), _) => {
            with(endpoint_text(&remote), service_name(remote.port()).or_else(|| service_name(row.local.port())))
        }
        (Proto::Tcp, None, Some(TcpState::Listening)) => with(row.local.port().to_string(), service_name(row.local.port())),
        _ => remote_text(row),
    }
}

/// The four tables (TCP4, TCP6, UDP4, UDP6) made into an inventory.
pub fn assemble(tables: [Result<Vec<Row>, TableError>; 4]) -> Inventory {
    let failed = tables.iter().filter(|t| t.is_err()).count();
    let mut rows: Vec<Row> = tables.into_iter().flatten().flatten().collect();
    let total = rows.len();
    rows.truncate(ROW_LIMIT);
    let status = if failed == 4 {
        Status::Unavailable
    } else if failed > 0 || total > ROW_LIMIT {
        Status::Partial
    } else {
        Status::Full
    };
    Inventory { rows, status, total, at: Instant::now(), ages: HashMap::new(), services: HashMap::new() }
}

/// The inventory line of the page (None: the first reading is running).
pub fn status_line(inventory: Option<&Inventory>) -> String {
    match inventory {
        None => "Collecting connection inventory.".into(),
        Some(inv) => match inv.status {
            Status::Unavailable => "Connection inventory is currently unavailable.".into(),
            // Rows cut at the limit: the chip, which names the
            // limit; a table that failed: its neutral partial sentence.
            Status::Partial if inv.total > ROW_LIMIT => "Partial inventory \u{b7} limited to 20,000 rows".into(),
            Status::Partial => "Connection inventory is partial or limited to 20,000 rows.".into(),
            Status::Full => {
                format!("Full inventory: {} of up to 20,000 rows", crate::sensors::model::thousands(inv.rows.len() as f64))
            }
        },
    }
}

/// A port of an IP Helper row (network byte order in the low 16 bits).
pub fn port_from_mib(raw: u32) -> u16 {
    u16::from_be((raw & 0xffff) as u16)
}

/// A row of a TCP table; None for a state outside the twelve (Bound).
pub fn tcp_row(local: IpAddr, local_port: u32, remote: IpAddr, remote_port: u32, state: u32, pid: u32) -> Option<Row> {
    let state = TcpState::from_mib(state)?;
    let remote = (state != TcpState::Listening).then(|| SocketAddr::new(remote, port_from_mib(remote_port)));
    Some(Row { proto: Proto::Tcp, local: SocketAddr::new(local, port_from_mib(local_port)), remote, state: Some(state), pid })
}

/// A row of a UDP table.
pub fn udp_row(local: IpAddr, local_port: u32, pid: u32) -> Row {
    Row { proto: Proto::Udp, local: SocketAddr::new(local, port_from_mib(local_port)), remote: None, state: None, pid }
}

/// (PID, service) pairs grouped by PID, names sorted; PID 0 (a service
/// that is not running) is left out.
pub fn services_by_pid(pairs: impl IntoIterator<Item = (u32, String)>) -> HashMap<u32, Vec<String>> {
    let mut map: HashMap<u32, Vec<String>> = HashMap::new();
    for (pid, name) in pairs.into_iter().filter(|(pid, _)| *pid != 0) {
        map.entry(pid).or_default().push(name);
    }
    for names in map.values_mut() {
        names.sort_by_key(|n| n.to_lowercase());
    }
    map
}

/// Reads the four tables of this PC, and the services each process runs.
pub use system::read;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use windows as system;

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(s: &str) -> IpAddr {
        IpAddr::V4(s.parse().unwrap())
    }
    fn v6(s: &str) -> IpAddr {
        IpAddr::V6(s.parse().unwrap())
    }

    #[test]
    fn well_known_ports_have_a_service_name() {
        assert_eq!(service_name(80), Some("HTTP"));
        assert_eq!(service_name(443), Some("HTTPS"));
        assert_eq!(service_name(445), Some("SMB"));
        assert_eq!(service_name(135), Some("RPC"));
        assert_eq!(service_name(993), Some("IMAPS"));
        assert_eq!(service_name(631), Some("IPP"));
        assert_eq!(service_name(5353), Some("mDNS"));
        assert_eq!(service_name(5355), Some("LLMNR"));
        assert_eq!(service_name(2179), Some("Hyper-V"));
        assert_eq!(service_name(7680), Some("Delivery Optimization"));
        assert_eq!(service_name(3389), Some("RDP"));
        assert_eq!(service_name(8443), None);
        assert_eq!(service_name(0), None);
    }

    #[test]
    fn a_connection_shows_its_remote_end_and_the_service() {
        let tcp = |local: &str, remote: &str| Row {
            proto: Proto::Tcp,
            local: local.parse().unwrap(),
            remote: Some(remote.parse().unwrap()),
            state: Some(TcpState::Established),
            pid: 1,
        };
        assert_eq!(remote_service_text(&tcp("10.0.0.2:50123", "160.79.104.10:443")), "160.79.104.10:443 \u{b7} HTTPS");
        assert_eq!(remote_service_text(&tcp("[::1]:50123", "[2a00:1450::200e]:993")), "[2a00:1450::200e]:993 \u{b7} IMAPS");
        // Unknown remote port: the local port names the service (an incoming SMB session).
        assert_eq!(remote_service_text(&tcp("192.168.1.5:445", "192.168.1.9:51000")), "192.168.1.9:51000 \u{b7} SMB");
        assert_eq!(remote_service_text(&tcp("10.0.0.2:50123", "10.0.0.3:8009")), "10.0.0.3:8009");
    }

    #[test]
    fn a_listener_shows_its_port_and_the_service() {
        let listener = |local: &str| Row {
            proto: Proto::Tcp,
            local: local.parse().unwrap(),
            remote: None,
            state: Some(TcpState::Listening),
            pid: 4,
        };
        assert_eq!(remote_service_text(&listener("[::]:445")), "445 \u{b7} SMB");
        assert_eq!(remote_service_text(&listener("0.0.0.0:631")), "631 \u{b7} IPP");
        assert_eq!(remote_service_text(&listener("0.0.0.0:8443")), "8443");
    }

    #[test]
    fn running_services_are_grouped_by_process() {
        let by_pid = services_by_pid([
            (1868, "RpcSs".to_string()),
            (4740, "DoSvc".to_string()),
            (1868, "RpcEptMapper".to_string()),
            (0, "Stopped".to_string()),
        ]);
        assert_eq!(by_pid.get(&1868), Some(&vec!["RpcEptMapper".to_string(), "RpcSs".to_string()]));
        assert_eq!(by_pid.get(&4740), Some(&vec!["DoSvc".to_string()]));
        assert_eq!(by_pid.get(&0), None);
    }

    #[test]
    fn an_assembled_inventory_starts_without_services() {
        assert!(assemble([Ok(vec![]), Ok(vec![]), Ok(vec![]), Ok(vec![])]).services.is_empty());
    }

    #[test]
    fn a_udp_endpoint_keeps_the_dash() {
        let udp = Row { proto: Proto::Udp, local: "0.0.0.0:5353".parse().unwrap(), remote: None, state: None, pid: 2976 };
        assert_eq!(remote_service_text(&udp), "\u{2014}");
    }

    #[test]
    fn ipv4_addresses_are_classified() {
        assert_eq!(classify(v4("127.0.0.1"), false), Exposure::Loopback);
        assert_eq!(classify(v4("127.255.0.9"), false), Exposure::Loopback);
        assert_eq!(classify(v4("10.1.2.3"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v4("172.16.0.1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v4("172.31.255.255"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v4("172.15.255.255"), false), Exposure::Internet);
        assert_eq!(classify(v4("172.32.0.0"), false), Exposure::Internet);
        assert_eq!(classify(v4("192.168.178.51"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v4("169.254.10.1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v4("169.253.10.1"), false), Exposure::Internet);
        assert_eq!(classify(v4("160.79.104.10"), false), Exposure::Internet);
    }

    #[test]
    fn ipv6_addresses_are_classified() {
        assert_eq!(classify(v6("::1"), false), Exposure::Loopback);
        assert_eq!(classify(v6("fe80::1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v6("febf::1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v6("fec0::1"), false), Exposure::Internet);
        assert_eq!(classify(v6("fc00::1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v6("fd12:3456::1"), false), Exposure::LocalNetwork);
        assert_eq!(classify(v6("fe00::1"), false), Exposure::Internet);
        assert_eq!(classify(v6("2a00:1450::200e"), false), Exposure::Internet);
        assert_eq!(classify(v6("::ffff:192.168.1.5"), false), Exposure::LocalNetwork, "IPv4-mapped");
        assert_eq!(classify(v6("::ffff:127.0.0.1"), false), Exposure::Loopback, "IPv4-mapped");
    }

    #[test]
    fn the_wildcard_is_all_interfaces_on_a_local_end_and_unspecified_on_a_remote_end() {
        assert_eq!(classify(v4("0.0.0.0"), true), Exposure::AllInterfaces);
        assert_eq!(classify(v6("::"), true), Exposure::AllInterfaces);
        assert_eq!(classify(v4("0.0.0.0"), false), Exposure::Unspecified);
        assert_eq!(classify(v6("::"), false), Exposure::Unspecified);
    }

    fn row(proto: Proto, local: &str, remote: Option<&str>, state: Option<TcpState>) -> Row {
        Row { proto, local: local.parse().unwrap(), remote: remote.map(|r| r.parse().unwrap()), state, pid: 4 }
    }

    #[test]
    fn a_connection_counts_its_remote_end_and_a_listener_its_local_end() {
        let conn = row(Proto::Tcp, "192.168.178.20:50000", Some("160.79.104.10:443"), Some(TcpState::Established));
        assert_eq!(exposure(&conn), Exposure::Internet);
        let lan = row(Proto::Tcp, "[::]:50000", Some("[fe80::5]:8009"), Some(TcpState::TimeWait));
        assert_eq!(exposure(&lan), Exposure::LocalNetwork);
        let listener = row(Proto::Tcp, "[::]:445", None, Some(TcpState::Listening));
        assert_eq!(exposure(&listener), Exposure::AllInterfaces);
        let local_listener = row(Proto::Tcp, "127.0.0.1:631", None, Some(TcpState::Listening));
        assert_eq!(exposure(&local_listener), Exposure::Loopback);
        let udp = row(Proto::Udp, "0.0.0.0:5353", None, None);
        assert_eq!(exposure(&udp), Exposure::AllInterfaces);
        let udp_lan = row(Proto::Udp, "192.168.178.20:137", None, None);
        assert_eq!(exposure(&udp_lan), Exposure::LocalNetwork);
        let closed = row(Proto::Tcp, "10.0.0.2:5000", None, Some(TcpState::Closed));
        assert_eq!(exposure(&closed), Exposure::Unspecified, "a connection with no remote end");
    }

    #[test]
    fn a_public_listener_is_a_tcp_listener_on_all_interfaces() {
        let cases = [
            (row(Proto::Tcp, "0.0.0.0:135", None, Some(TcpState::Listening)), true, true),
            (row(Proto::Tcp, "[::]:445", None, Some(TcpState::Listening)), true, true),
            (row(Proto::Tcp, "127.0.0.1:631", None, Some(TcpState::Listening)), true, false),
            (row(Proto::Tcp, "192.168.1.5:3389", None, Some(TcpState::Listening)), true, false),
            (row(Proto::Udp, "0.0.0.0:5353", None, None), false, false),
            (row(Proto::Tcp, "0.0.0.0:50000", Some("1.2.3.4:443"), Some(TcpState::Established)), false, false),
        ];
        for (r, listener, public) in cases {
            assert_eq!((r.is_tcp_listener(), r.is_public_listener()), (listener, public), "{r:?}");
        }
    }

    #[test]
    fn the_twelve_tcp_states_read_as_windows_reports_them_and_bound_is_dropped() {
        let texts: Vec<&str> = (1..=12).map(|n| TcpState::from_mib(n).expect("a TCP state").label()).collect();
        assert_eq!(
            texts,
            [
                "Closed",
                "Listening",
                "SYN sent",
                "SYN received",
                "Established",
                "FIN wait 1",
                "FIN wait 2",
                "Close wait",
                "Closing",
                "Last ACK",
                "Time wait",
                "Delete TCB"
            ]
        );
        assert_eq!(TcpState::from_mib(0), None);
        assert_eq!(TcpState::from_mib(100), None, "Bound");
    }

    #[test]
    fn a_udp_row_is_a_datagram_with_no_remote_end() {
        let udp = row(Proto::Udp, "[::]:5355", None, None);
        assert_eq!(state_text(&udp), "Datagram");
        assert_eq!(remote_text(&udp), "\u{2014}");
        let tcp = row(Proto::Tcp, "10.0.0.2:50000", Some("1.2.3.4:443"), Some(TcpState::CloseWait));
        assert_eq!(state_text(&tcp), "Close wait");
        assert_eq!(remote_text(&tcp), "1.2.3.4:443");
    }

    #[test]
    fn the_protocol_names_transport_and_ip_family() {
        assert_eq!(protocol_text(&row(Proto::Tcp, "1.2.3.4:1", None, Some(TcpState::Listening))), "TCP4");
        assert_eq!(protocol_text(&row(Proto::Tcp, "[::1]:1", None, Some(TcpState::Listening))), "TCP6");
        assert_eq!(protocol_text(&row(Proto::Udp, "0.0.0.0:1", None, None)), "UDP4");
        assert_eq!(protocol_text(&row(Proto::Udp, "[::]:1", None, None)), "UDP6");
    }

    #[test]
    fn endpoints_put_ipv6_in_brackets() {
        assert_eq!(endpoint_text(&"127.0.0.1:631".parse().unwrap()), "127.0.0.1:631");
        assert_eq!(endpoint_text(&"0.0.0.0:8443".parse().unwrap()), "0.0.0.0:8443");
        assert_eq!(endpoint_text(&"[::1]:7000".parse().unwrap()), "[::1]:7000");
        assert_eq!(endpoint_text(&"[::]:5353".parse().unwrap()), "[::]:5353");
        assert_eq!(endpoint_text(&"[fe80::1%7]:80".parse().unwrap()), "[fe80::1]:80", "no scope id");
    }

    #[test]
    fn exposures_have_their_shown_names() {
        let names: Vec<&str> =
            [Exposure::Internet, Exposure::AllInterfaces, Exposure::LocalNetwork, Exposure::Loopback, Exposure::Unspecified]
                .iter()
                .map(|e| e.label())
                .collect();
        assert_eq!(names, ["Internet", "All interfaces", "Local network", "Loopback", "Unspecified"]);
    }

    fn tcp4() -> Row {
        row(Proto::Tcp, "0.0.0.0:135", None, Some(TcpState::Listening))
    }
    fn tcp6() -> Row {
        row(Proto::Tcp, "[::]:445", None, Some(TcpState::Listening))
    }
    fn udp4() -> Row {
        row(Proto::Udp, "0.0.0.0:5353", None, None)
    }
    fn udp6() -> Row {
        row(Proto::Udp, "[::]:5355", None, None)
    }

    #[test]
    fn four_tables_read_make_a_full_inventory_in_table_order() {
        let inv = assemble([Ok(vec![tcp4()]), Ok(vec![tcp6()]), Ok(vec![udp4()]), Ok(vec![udp6()])]);
        assert_eq!(inv.status, Status::Full);
        assert_eq!(inv.rows, vec![tcp4(), tcp6(), udp4(), udp6()]);
        assert_eq!(inv.total, 4);
    }

    #[test]
    fn one_failed_table_leaves_the_others_and_makes_it_partial() {
        let inv = assemble([Ok(vec![tcp4()]), Err(TableError), Ok(vec![udp4()]), Ok(vec![udp6()])]);
        assert_eq!(inv.status, Status::Partial);
        assert_eq!(inv.rows, vec![tcp4(), udp4(), udp6()]);
    }

    #[test]
    fn no_table_read_makes_it_unavailable() {
        let inv = assemble([Err(TableError), Err(TableError), Err(TableError), Err(TableError)]);
        assert_eq!(inv.status, Status::Unavailable);
        assert!(inv.rows.is_empty());
    }

    #[test]
    fn rows_beyond_twenty_thousand_are_cut_and_make_it_partial() {
        let inv = assemble([Ok(vec![tcp4(); 15_000]), Ok(vec![tcp6(); 5_000]), Ok(vec![]), Ok(vec![])]);
        assert_eq!((inv.status, inv.rows.len()), (Status::Full, 20_000), "exactly the limit is full");
        let inv = assemble([Ok(vec![tcp4(); 15_000]), Ok(vec![tcp6(); 5_000]), Ok(vec![udp4(); 1]), Ok(vec![udp6(); 2])]);
        assert_eq!(inv.status, Status::Partial);
        assert_eq!(inv.rows.len(), 20_000);
        assert_eq!(inv.total, 20_003);
    }

    #[test]
    fn the_status_line_names_each_inventory_state() {
        assert_eq!(status_line(None), "Collecting connection inventory.");
        let full = assemble([Ok(vec![tcp4(); 254]), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
        assert_eq!(status_line(Some(&full)), "Full inventory: 254 of up to 20,000 rows");
        let big = assemble([Ok(vec![tcp4(); 12_345]), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
        assert_eq!(status_line(Some(&big)), "Full inventory: 12,345 of up to 20,000 rows");
        // A failed table: the neutral wording, not the row limit.
        let partial = assemble([Ok(vec![tcp4()]), Err(TableError), Ok(vec![]), Ok(vec![])]);
        assert_eq!(status_line(Some(&partial)), "Connection inventory is partial or limited to 20,000 rows.");
        // Rows cut at the limit: the chip naming the limit.
        let cut = assemble([Ok(vec![tcp4(); 20_001]), Ok(vec![]), Ok(vec![]), Ok(vec![])]);
        assert_eq!(status_line(Some(&cut)), "Partial inventory \u{b7} limited to 20,000 rows");
        let none = assemble([Err(TableError), Err(TableError), Err(TableError), Err(TableError)]);
        assert_eq!(status_line(Some(&none)), "Connection inventory is currently unavailable.");
    }

    #[test]
    fn mib_ports_are_in_network_byte_order() {
        // Port 80 is stored as the bytes 00 50: 0x5000 read little-endian.
        assert_eq!(port_from_mib(0x5000), 80);
        assert_eq!(port_from_mib(0xbd01), 445);
        assert_eq!(port_from_mib(0xffff_bd01), 445, "the upper bytes are ignored");
    }

    #[test]
    fn mib_tcp_rows_become_rows() {
        let local = IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0));
        let any = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        let listener = tcp_row(local, 0xbd01, any, 0, 2, 4).unwrap();
        assert_eq!(listener, row(Proto::Tcp, "0.0.0.0:445", None, Some(TcpState::Listening)), "a listener has no remote end");
        let remote = IpAddr::V6("2a00::1".parse().unwrap());
        let conn = tcp_row(IpAddr::V6(Ipv6Addr::LOCALHOST), 0x5000, remote, 0xbb01, 5, 9).unwrap();
        assert_eq!(conn.remote, Some("[2a00::1]:443".parse().unwrap()));
        assert_eq!((conn.state, conn.pid), (Some(TcpState::Established), 9));
        assert_eq!(tcp_row(local, 1, any, 0, 100, 4), None, "Bound rows are left out");
    }

    #[test]
    fn a_socket_age_comes_from_its_creation_filetime() {
        // FILETIME counts 100 ns steps.
        let now = 134_394_060_060_000_000i64;
        assert_eq!(socket_age(now - 3_250_000_000, now), Some(std::time::Duration::from_secs(325)));
        assert_eq!(socket_age(0, now), None, "Windows gives no time (e.g. Time wait rows of PID 0)");
        assert_eq!(socket_age(now + 10_000_000, now), Some(std::time::Duration::ZERO), "a clock step back");
    }

    /// Reads this PC's tables (run with `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn lists_this_pc() {
        let inv = read();
        assert_eq!(inv.status, Status::Full);
        for proto in ["TCP4", "TCP6", "UDP4", "UDP6"] {
            assert!(inv.rows.iter().any(|r| protocol_text(r) == proto), "no {proto} row");
        }
        assert!(
            inv.rows.iter().any(|r| r.pid == 4 && r.local.port() == 445 && r.state == Some(TcpState::Listening)),
            "System listens on 445"
        );
        for r in &inv.rows {
            let e = exposure(r);
            if r.state == Some(TcpState::Listening) || r.proto == Proto::Udp {
                assert_ne!(e, Exposure::Unspecified, "{r:?}");
            }
        }
        assert!(inv.ages.len() * 2 > inv.rows.len(), "most sockets carry Windows' creation time");
        println!("{} rows, {} with a creation time; {}", inv.rows.len(), inv.ages.len(), status_line(Some(&inv)));
    }

    /// Reads this PC's running services and a system file's description
    /// (run with `cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn names_services_and_descriptions_on_this_pc() {
        let inv = read();
        assert!(inv.services.values().flatten().any(|s| s == "RpcSs"), "RpcSs runs in a svchost");
        let svchost = r"C:\Windows\System32\svchost.exe";
        let description = crate::file_description::file_description(svchost).expect("svchost.exe has a description");
        println!("{svchost}: {description}; {} processes run services", inv.services.len());
    }
}
