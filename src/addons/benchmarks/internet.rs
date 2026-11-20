//! The Internet speed test: downloads blocks of growing size from
//! speed.cloudflare.com, then uploads blocks to it, and reports Download and
//! Upload in Mbit/s. The network sits behind [`Service`]: the app uses
//! [`Cloudflare`] (HTTPS through the system proxy, with timeouts), the tests
//! use fakes. It runs only when the user starts it.

use std::time::Duration;

use super::engine::{Probe, MBITS};

/// The speed-test service.
pub const HOST: &str = "speed.cloudflare.com";
/// The phases' names.
pub const DOWNLOAD_PHASE: &str = "Testing download speed";
pub const UPLOAD_PHASE: &str = "Testing upload speed";
/// The figures.
pub const DOWNLOAD: &str = "Download";
pub const UPLOAD: &str = "Upload";

/// Which way a transfer goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Direction {
    Download,
    Upload,
}

/// What is wrong with the server's certificate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Certificate {
    Revoked,
    Expired,
    NotForServers,
    Invalid,
    OtherName,
    Untrusted,
    RevocationUnknown,
}

/// Why a transfer failed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Failure {
    /// The request could not be built.
    Request,
    /// No connection to the service (refused, no route, proxy failure…).
    Connect,
    /// The service answered with this HTTP status.
    Status(u16),
    /// The service kept answering "too many requests".
    Refusing,
    /// A timeout; `any_data` tells whether some bytes went through first.
    TimedOut { any_data: bool },
    /// The connection ended before the whole block went through.
    Incomplete,
    /// TLS: the server's certificate was refused.
    Certificate(Certificate),
    /// The user cancelled.
    Cancelled,
}

impl Failure {
    /// The message for this failure of a `direction` transfer.
    pub fn message(self, direction: Direction) -> String {
        let text = match self {
            Failure::Request => "Unable to create a speed-test request.",
            Failure::Connect => "Unable to connect to the speed-test service.",
            Failure::Status(code) => return format!("The speed-test service rejected a transfer (HTTP {code})."),
            Failure::Refusing => "The speed-test service kept refusing transfers.",
            Failure::TimedOut { any_data: false } => match direction {
                Direction::Download => "The speed-test download timed out without receiving any data.",
                Direction::Upload => "The speed-test upload timed out without sending any data.",
            },
            Failure::TimedOut { any_data: true } | Failure::Incomplete => "The speed-test transfer did not complete.",
            Failure::Certificate(c) => match c {
                Certificate::Revoked => "The server's certificate has been revoked.",
                Certificate::Expired => "The server's certificate has expired or is not valid yet. Check this computer's date and time.",
                Certificate::NotForServers => "The server's certificate is not valid for securing a server.",
                Certificate::Invalid => "The server's certificate is not valid.",
                Certificate::OtherName => "The server's certificate was issued for a different name.",
                Certificate::Untrusted => {
                    return format!(
                        "The server's certificate was not trusted on this computer. A proxy, VPN or security program may be intercepting secure connections. Ask whoever manages it to exclude {HOST} from HTTPS inspection."
                    )
                }
                Certificate::RevocationUnknown => "Windows could not check whether the server's certificate has been revoked.",
            },
            Failure::Cancelled => "Cancelled",
        };
        text.to_string()
    }
}

/// Megabits per second for `bytes` in `seconds`.
pub fn mbits(bytes: u64, seconds: f64) -> f64 {
    if seconds > 0.0 {
        (bytes * 8) as f64 / seconds / 1e6
    } else {
        0.0
    }
}

/// The service the test talks to.
pub trait Service {
    /// Moves a block of `bytes` the `direction` way and returns how many
    /// seconds it took. `cancelled` is checked while the block moves.
    fn transfer(&mut self, direction: Direction, bytes: u64, cancelled: &dyn Fn() -> bool) -> Result<f64, Failure>;
}

/// How the test grows its blocks and handles a busy service.
#[derive(Clone, Debug)]
pub struct Plan {
    pub downloads: &'static [u64],
    pub uploads: &'static [u64],
    /// A block that takes this long is the last one: its speed is the result.
    pub enough_seconds: f64,
    /// Tries of a block while the service says "too many requests".
    pub tries: u32,
    pub retry_wait: Duration,
}

/// The app's plan: 100 kB up to 50 MB down (the service refuses 100 MB
/// with HTTP 403), 100 kB up to 25 MB up.
pub const PLAN: Plan = Plan {
    downloads: &[100_000, 1_000_000, 10_000_000, 25_000_000, 50_000_000],
    uploads: &[100_000, 1_000_000, 10_000_000, 25_000_000],
    enough_seconds: 1.0,
    tries: 3,
    retry_wait: Duration::from_secs(2),
};

/// The test: download then upload, each reported in Mbit/s.
pub fn run(probe: &Probe, service: &mut dyn Service, plan: &Plan) -> Result<(), String> {
    probe.phase(DOWNLOAD_PHASE);
    let down = measure(probe, service, plan, Direction::Download, 0.0)?;
    probe.figure(DOWNLOAD, down, MBITS);
    probe.phase(UPLOAD_PHASE);
    let up = measure(probe, service, plan, Direction::Upload, 0.5)?;
    probe.figure(UPLOAD, up, MBITS);
    probe.progress(1.0);
    Ok(())
}

/// The speed one way, in Mbit/s: blocks grow until one takes long enough
/// (or the largest went through); the last block's speed is the result.
/// Progress goes from `from` to `from` + 0.5.
fn measure(probe: &Probe, service: &mut dyn Service, plan: &Plan, direction: Direction, from: f32) -> Result<f64, String> {
    let blocks = match direction {
        Direction::Download => plan.downloads,
        Direction::Upload => plan.uploads,
    };
    let mut speed = 0.0;
    for (i, &bytes) in blocks.iter().enumerate() {
        probe.progress(from + i as f32 / blocks.len() as f32 * 0.5);
        let seconds = transfer(probe, service, plan, direction, bytes).map_err(|f| f.message(direction))?;
        speed = mbits(bytes, seconds);
        if seconds >= plan.enough_seconds {
            break;
        }
    }
    Ok(speed)
}

/// One block, tried again while the service says "too many requests"
/// (HTTP 429), up to the plan's tries.
fn transfer(probe: &Probe, service: &mut dyn Service, plan: &Plan, direction: Direction, bytes: u64) -> Result<f64, Failure> {
    let cancelled = || probe.cancelled();
    for _ in 0..plan.tries {
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        match service.transfer(direction, bytes, &cancelled) {
            Err(Failure::Status(429)) => sleep_unless_cancelled(probe, plan.retry_wait),
            other => return other,
        }
    }
    Err(Failure::Refusing)
}

/// Waits `wait`, waking early on a cancel.
fn sleep_unless_cancelled(probe: &Probe, wait: Duration) {
    let t = std::time::Instant::now();
    while t.elapsed() < wait && !probe.cancelled() {
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The system proxy URL for `host`, from the Windows Internet settings
/// `server` ("host:port" or "http=host:port;https=host:port") and `bypass`
/// list ("*.example.com;<local>"); `None` when `host` goes direct.
fn proxy_for(server: &str, bypass: &str, host: &str) -> Option<String> {
    let host = host.to_ascii_lowercase();
    let bypassed = bypass.split(';').map(|e| e.trim().to_ascii_lowercase()).any(|entry| match entry.as_str() {
        "" => false,
        "<local>" => !host.contains('.'),
        _ => wildcard(&entry, &host),
    });
    if bypassed {
        return None;
    }
    let entries: Vec<&str> = server.split(';').map(str::trim).filter(|e| !e.is_empty()).collect();
    let pick = |scheme: &str| entries.iter().find_map(|e| e.strip_prefix(scheme).and_then(|r| r.strip_prefix('=')));
    let address = match entries.as_slice() {
        [only] if !only.contains('=') => Some(*only),
        _ => pick("https").or_else(|| pick("http")),
    }?;
    Some(if address.contains("://") { address.to_string() } else { format!("http://{address}") })
}

/// `pattern` ("*" matches any run of characters) matches all of `text`.
fn wildcard(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, rest)) => {
            let Some(text) = text.strip_prefix(head) else { return false };
            (0..=text.len()).filter(|&i| text.is_char_boundary(i)).any(|i| wildcard(rest, &text[i..]))
        }
    }
}

/// The system proxy for [`HOST`], from the user's Windows Internet settings
/// (proxy auto-config scripts are not read).
#[cfg(windows)]
fn system_proxy() -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    let key = winreg::RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings", KEY_READ)
        .ok()?;
    if key.get_value::<u32, _>("ProxyEnable").ok()? != 1 {
        return None;
    }
    let server: String = key.get_value("ProxyServer").ok()?;
    let bypass: String = key.get_value("ProxyOverride").unwrap_or_default();
    proxy_for(&server, &bypass, HOST)
}

#[cfg(not(windows))]
fn system_proxy() -> Option<String> {
    None
}

/// The failure a ureq error means; `any_data` tells whether some bytes of
/// the block went through first.
fn classify(error: &ureq::Error, any_data: bool) -> Failure {
    use std::io::ErrorKind;
    use ureq::Error as E;
    match error {
        E::StatusCode(code) => Failure::Status(*code),
        E::BadUri(_) | E::Http(_) | E::InvalidProxyUrl | E::RequireHttpsOnly(_) => Failure::Request,
        E::Timeout(ureq::Timeout::Resolve | ureq::Timeout::Connect) => Failure::Connect,
        E::Timeout(_) => Failure::TimedOut { any_data },
        E::Rustls(e) => tls(e),
        E::Io(e) => match e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
            Some(e) => tls(e),
            None if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Failure::TimedOut { any_data },
            None if any_data => Failure::Incomplete,
            None => Failure::Connect,
        },
        _ if any_data => Failure::Incomplete,
        _ => Failure::Connect,
    }
}

/// The failure a TLS error means.
fn tls(error: &rustls::Error) -> Failure {
    use rustls::CertificateError as C;
    let rustls::Error::InvalidCertificate(c) = error else { return Failure::Connect };
    Failure::Certificate(match c {
        C::Revoked => Certificate::Revoked,
        C::Expired | C::ExpiredContext { .. } | C::NotValidYet | C::NotValidYetContext { .. } => Certificate::Expired,
        C::InvalidPurpose | C::InvalidPurposeContext { .. } => Certificate::NotForServers,
        C::NotValidForName | C::NotValidForNameContext { .. } => Certificate::OtherName,
        C::UnknownIssuer => Certificate::Untrusted,
        C::UnknownRevocationStatus => Certificate::RevocationUnknown,
        _ => Certificate::Invalid,
    })
}

/// The real service: HTTPS to speed.cloudflare.com, through the proxy of
/// the `HTTPS_PROXY`-style variables or else the Windows system proxy.
pub struct Cloudflare {
    agent: ureq::Agent,
}

/// Connecting gives up after this long.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Waiting for the answer, or for any byte to go through, gives up after this long.
const STALL_TIMEOUT: Duration = Duration::from_secs(15);
/// One block gives up after this long in all.
const BLOCK_TIMEOUT: Duration = Duration::from_secs(90);

impl Cloudflare {
    pub fn new() -> Cloudflare {
        let proxy = ureq::Proxy::try_from_env().or_else(|| system_proxy().and_then(|url| ureq::Proxy::new(&url).ok()));
        let config = ureq::Agent::config_builder()
            .proxy(proxy)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_recv_response(Some(STALL_TIMEOUT))
            .timeout_per_read(Some(STALL_TIMEOUT))
            .timeout_per_write(Some(STALL_TIMEOUT))
            .timeout_global(Some(BLOCK_TIMEOUT))
            .build();
        Cloudflare { agent: ureq::Agent::new_with_config(config) }
    }

    fn download(&self, bytes: u64, cancelled: &dyn Fn() -> bool) -> Result<f64, Failure> {
        let mut answer = self.agent.get(format!("https://{HOST}/__down?bytes={bytes}")).call().map_err(|e| classify(&e, false))?;
        let status = answer.status().as_u16();
        if status != 200 {
            return Err(Failure::Status(status));
        }
        let t = std::time::Instant::now();
        let mut reader = answer.body_mut().as_reader();
        let mut buf = vec![0u8; 64 * 1024];
        let mut got = 0u64;
        while got < bytes {
            if cancelled() {
                return Err(Failure::Cancelled);
            }
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) => return Err(Failure::Incomplete),
                Ok(n) => got += n as u64,
                Err(e) => return Err(classify(&ureq::Error::from(e), got > 0)),
            }
        }
        Ok(t.elapsed().as_secs_f64())
    }

    fn upload(&self, bytes: u64, cancelled: &dyn Fn() -> bool) -> Result<f64, Failure> {
        let mut body = Zeros { left: bytes, sent: 0, cancelled };
        let t = std::time::Instant::now();
        let res = self
            .agent
            .post(format!("https://{HOST}/__up"))
            .header("Content-Type", "application/octet-stream")
            .header("Content-Length", bytes.to_string())
            .send(ureq::SendBody::from_reader(&mut body));
        let seconds = t.elapsed().as_secs_f64();
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        let answer = res.map_err(|e| classify(&e, body.sent > 0))?;
        match answer.status().as_u16() {
            200 => Ok(seconds),
            status => Err(Failure::Status(status)),
        }
    }
}

impl Service for Cloudflare {
    fn transfer(&mut self, direction: Direction, bytes: u64, cancelled: &dyn Fn() -> bool) -> Result<f64, Failure> {
        match direction {
            Direction::Download => self.download(bytes, cancelled),
            Direction::Upload => self.upload(bytes, cancelled),
        }
    }
}

/// An upload body: `left` zero bytes; it stops when cancelled.
struct Zeros<'a> {
    left: u64,
    sent: u64,
    cancelled: &'a dyn Fn() -> bool,
}

impl std::io::Read for Zeros<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::Error::other("cancelled"));
        }
        let n = (buf.len() as u64).min(self.left) as usize;
        buf[..n].fill(0);
        self.left -= n as u64;
        self.sent += n as u64;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addons::benchmarks::engine::{wait_idle, Engine, EngineState, Figure, Outcome, Unit};
    use std::time::Instant;

    const QUICK: Plan = Plan { downloads: &[1_000, 10_000, 100_000], uploads: &[1_000, 10_000], enough_seconds: 1.0, tries: 3, retry_wait: Duration::ZERO };

    /// A fake service: answers each transfer from `answer`, and logs them.
    struct Fake<F> {
        answer: F,
        log: std::sync::Arc<std::sync::Mutex<Vec<(Direction, u64)>>>,
    }

    impl<F: FnMut(Direction, u64, &dyn Fn() -> bool) -> Result<f64, Failure>> Service for Fake<F> {
        fn transfer(&mut self, direction: Direction, bytes: u64, cancelled: &dyn Fn() -> bool) -> Result<f64, Failure> {
            self.log.lock().unwrap().push((direction, bytes));
            (self.answer)(direction, bytes, cancelled)
        }
    }

    type Log = std::sync::Arc<std::sync::Mutex<Vec<(Direction, u64)>>>;

    /// Runs the test against a fake answering with `answer`.
    fn run_fake(answer: impl FnMut(Direction, u64, &dyn Fn() -> bool) -> Result<f64, Failure> + Send + 'static) -> (EngineState, Log) {
        let log = Log::default();
        let mut fake = Fake { answer, log: log.clone() };
        let mut engine = Engine::default();
        assert!(engine.start(vec![Unit::new("Internet", move |p| run(p, &mut fake, &QUICK))]));
        (wait_idle(&engine), log)
    }

    #[test]
    fn mbits_counts_eight_bits_per_byte_in_millions_per_second() {
        assert_eq!(mbits(12_500_000, 1.0), 100.0);
        assert_eq!(mbits(1_000_000, 2.0), 4.0);
        assert_eq!(mbits(1_000_000, 0.0), 0.0, "no time measured, no speed");
    }

    #[test]
    fn mbit_figures_read_with_one_decimal() {
        assert_eq!(Figure { name: DOWNLOAD, value: 943.21, unit: MBITS }.text(), "943.2 Mbit/s");
        assert_eq!(Figure { name: UPLOAD, value: 12.0, unit: MBITS }.text(), "12.0 Mbit/s");
    }

    #[test]
    fn failures_read_as_their_messages() {
        use Direction::*;
        let m = |f: Failure, d| f.message(d);
        assert_eq!(m(Failure::Request, Download), "Unable to create a speed-test request.");
        assert_eq!(m(Failure::Connect, Upload), "Unable to connect to the speed-test service.");
        assert_eq!(m(Failure::Status(503), Download), "The speed-test service rejected a transfer (HTTP 503).");
        assert_eq!(m(Failure::Refusing, Download), "The speed-test service kept refusing transfers.");
        assert_eq!(m(Failure::TimedOut { any_data: false }, Download), "The speed-test download timed out without receiving any data.");
        assert_eq!(m(Failure::TimedOut { any_data: false }, Upload), "The speed-test upload timed out without sending any data.");
        assert_eq!(m(Failure::TimedOut { any_data: true }, Upload), "The speed-test transfer did not complete.");
        assert_eq!(m(Failure::Incomplete, Download), "The speed-test transfer did not complete.");
        assert_eq!(m(Failure::Certificate(Certificate::Expired), Download), "The server's certificate has expired or is not valid yet. Check this computer's date and time.");
        assert_eq!(m(Failure::Certificate(Certificate::OtherName), Download), "The server's certificate was issued for a different name.");
        assert_eq!(
            m(Failure::Certificate(Certificate::Untrusted), Download),
            "The server's certificate was not trusted on this computer. A proxy, VPN or security program may be intercepting secure connections. Ask whoever manages it to exclude speed.cloudflare.com from HTTPS inspection."
        );
    }

    #[test]
    fn blocks_grow_until_one_takes_long_enough_and_its_speed_is_the_result() {
        // Each block takes 1 s per 10 000 bytes: 1 000 (0.1 s), 10 000 (1 s) is enough.
        let (st, log) = run_fake(|_, bytes, _| Ok(bytes as f64 / 10_000.0));
        assert_eq!(st.outcome, Some(Outcome::Complete));
        let figures: Vec<_> = st.results("Internet").iter().map(|f| (f.name, f.value, f.unit)).collect();
        assert_eq!(figures, [(DOWNLOAD, 0.08, MBITS), (UPLOAD, 0.08, MBITS)]);
        let log = log.lock().unwrap().clone();
        assert_eq!(log, [(Direction::Download, 1_000), (Direction::Download, 10_000), (Direction::Upload, 1_000), (Direction::Upload, 10_000)]);
    }

    #[test]
    fn a_fast_link_uses_the_largest_block() {
        let (st, log) = run_fake(|_, bytes, _| Ok(bytes as f64 / 1e9));
        assert!((st.results("Internet")[0].value - 8_000.0).abs() < 1e-6);
        assert_eq!(log.lock().unwrap().iter().filter(|(d, _)| *d == Direction::Download).count(), 3);
    }

    #[test]
    fn a_failed_transfer_fails_the_test_with_its_message() {
        let (st, _) = run_fake(|_, _, _| Err(Failure::Connect));
        assert_eq!(st.outcome, Some(Outcome::Failed { test: "Internet", error: "Unable to connect to the speed-test service.".into() }));
        let (st, _) = run_fake(|d, _, _| if d == Direction::Upload { Err(Failure::TimedOut { any_data: false }) } else { Ok(1.0) });
        assert_eq!(st.status().as_deref(), Some("Internet test failed: The speed-test upload timed out without sending any data."));
    }

    #[test]
    fn too_many_requests_is_tried_again_then_reads_kept_refusing() {
        let mut busy = 1;
        let (st, _) = run_fake(move |_, bytes, _| {
            if busy > 0 {
                busy -= 1;
                return Err(Failure::Status(429));
            }
            Ok(bytes as f64)
        });
        assert_eq!(st.outcome, Some(Outcome::Complete), "one 429 is tried again");
        let (st, log) = run_fake(|_, _, _| Err(Failure::Status(429)));
        assert_eq!(st.outcome, Some(Outcome::Failed { test: "Internet", error: "The speed-test service kept refusing transfers.".into() }));
        assert_eq!(log.lock().unwrap().len(), 3, "three tries");
    }

    #[test]
    fn cancel_stops_the_transfer_and_the_test() {
        let (started, wait_started) = std::sync::mpsc::channel();
        let log = Log::default();
        let mut fake = Fake {
            answer: move |_, _, cancelled: &dyn Fn() -> bool| {
                let _ = started.send(());
                let t = Instant::now();
                while !cancelled() && t.elapsed() < Duration::from_secs(5) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(Failure::Cancelled)
            },
            log: log.clone(),
        };
        let mut engine = Engine::default();
        engine.start(vec![Unit::new("Internet", move |p| run(p, &mut fake, &QUICK))]);
        wait_started.recv().unwrap();
        engine.cancel();
        let st = wait_idle(&engine);
        assert_eq!(st.outcome, Some(Outcome::Cancelled));
        assert_eq!(log.lock().unwrap().len(), 1, "no transfer after the cancel");
        assert!(st.results("Internet").is_empty());
    }

    #[test]
    fn the_system_proxy_is_used_unless_the_host_is_bypassed() {
        assert_eq!(proxy_for("proxy.corp:8080", "", HOST).as_deref(), Some("http://proxy.corp:8080"));
        assert_eq!(proxy_for("http=a:80;https=b:8443", "", HOST).as_deref(), Some("http://b:8443"), "the HTTPS entry wins");
        assert_eq!(proxy_for("http=a:80", "", HOST).as_deref(), Some("http://a:80"));
        assert_eq!(proxy_for("ftp=f:21", "", HOST), None);
        assert_eq!(proxy_for("", "", HOST), None);
        assert_eq!(proxy_for("p:1", "<local>", HOST).as_deref(), Some("http://p:1"), "<local> is only dotless names");
        assert_eq!(proxy_for("p:1", "intranet;*.CloudFlare.com", HOST), None);
        assert_eq!(proxy_for("p:1", "speed.cloudflare.com", HOST), None);
    }

    #[test]
    fn network_errors_become_failures() {
        use std::io::{Error as Io, ErrorKind};
        use rustls::CertificateError as C;
        let tls = |c: C| ureq::Error::Rustls(rustls::Error::InvalidCertificate(c));
        assert_eq!(classify(&ureq::Error::Io(Io::from(ErrorKind::ConnectionRefused)), false), Failure::Connect);
        assert_eq!(classify(&ureq::Error::HostNotFound, false), Failure::Connect);
        assert_eq!(classify(&ureq::Error::ConnectionFailed, false), Failure::Connect);
        assert_eq!(classify(&ureq::Error::Timeout(ureq::Timeout::Connect), false), Failure::Connect);
        assert_eq!(classify(&ureq::Error::Timeout(ureq::Timeout::PerRead), false), Failure::TimedOut { any_data: false });
        assert_eq!(classify(&ureq::Error::Io(Io::from(ErrorKind::TimedOut)), true), Failure::TimedOut { any_data: true });
        assert_eq!(classify(&ureq::Error::Io(Io::from(ErrorKind::UnexpectedEof)), true), Failure::Incomplete);
        assert_eq!(classify(&ureq::Error::BadUri("x".into()), false), Failure::Request);
        assert_eq!(classify(&ureq::Error::StatusCode(503), false), Failure::Status(503));
        assert_eq!(classify(&tls(C::Expired), false), Failure::Certificate(Certificate::Expired));
        assert_eq!(classify(&tls(C::NotValidYet), false), Failure::Certificate(Certificate::Expired));
        assert_eq!(classify(&tls(C::Revoked), false), Failure::Certificate(Certificate::Revoked));
        assert_eq!(classify(&tls(C::UnknownIssuer), false), Failure::Certificate(Certificate::Untrusted));
        assert_eq!(classify(&tls(C::NotValidForName), false), Failure::Certificate(Certificate::OtherName));
        assert_eq!(classify(&tls(C::InvalidPurpose), false), Failure::Certificate(Certificate::NotForServers));
        assert_eq!(classify(&tls(C::UnknownRevocationStatus), false), Failure::Certificate(Certificate::RevocationUnknown));
        assert_eq!(classify(&tls(C::BadSignature), false), Failure::Certificate(Certificate::Invalid));
        // rustls errors also come wrapped in an I/O error.
        let wrapped = Io::new(ErrorKind::InvalidData, rustls::Error::InvalidCertificate(C::UnknownIssuer));
        assert_eq!(classify(&ureq::Error::Io(wrapped), false), Failure::Certificate(Certificate::Untrusted));
    }

    /// The real test against Cloudflare (needs the network).
    #[test]
    #[ignore]
    fn measures_this_connection_against_cloudflare() {
        let mut engine = Engine::default();
        assert!(engine.start(vec![Unit::new("Internet", |p| run(p, &mut Cloudflare::new(), &PLAN))]));
        let t = Instant::now();
        while engine.busy() && t.elapsed() < Duration::from_secs(300) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let st = engine.state();
        println!("{:?} {:?}", st.outcome, st.results("Internet").iter().map(Figure::text).collect::<Vec<_>>());
        assert_eq!(st.outcome, Some(Outcome::Complete));
        assert!(st.results("Internet").iter().all(|f| f.value > 0.0));
    }
}
