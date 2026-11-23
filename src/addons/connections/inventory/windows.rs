//! Windows: the IP Helper calls.

use super::*;
use windows_sys::Win32::NetworkManagement::IpHelper::*;

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

/// A table's rows, each with its age by Windows' creation time.
type Table = Result<Vec<(Row, Option<Duration>)>, TableError>;

/// Reads the four tables of this PC, and the services each process runs.
pub fn read() -> Inventory {
    let (tables, ages) = tables();
    Inventory { ages, services: services_by_pid(crate::winsys::running_service_pids()), ..assemble(tables) }
}

/// TCP4, TCP6, UDP4, UDP6 (the "owner module" tables, which carry each
/// socket's creation time), and the age of each socket that has one.
pub fn tables() -> ([Result<Vec<Row>, TableError>; 4], HashMap<Key, Duration>) {
    let now = now_filetime();
    let read = [tcp(AF_INET, now), tcp(AF_INET6, now), udp(AF_INET, now), udp(AF_INET6, now)];
    let mut ages = HashMap::new();
    let tables = read.map(|table| {
        table.map(|rows| {
            rows.into_iter()
                .map(|(row, age)| {
                    if let Some(age) = age {
                        ages.insert(super::super::groups::key(&row), age);
                    }
                    row
                })
                .collect()
        })
    });
    (tables, ages)
}

/// The current time as a FILETIME (100 ns steps since 1601).
fn now_filetime() -> i64 {
    const UNIX_TO_1601: i64 = 116_444_736_000_000_000;
    let since_unix = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    UNIX_TO_1601 + (since_unix.as_nanos() / 100) as i64
}

/// Calls `get(buffer, size)` until the buffer is big enough; the buffer
/// is u64-aligned, as the owner module tables need.
fn fetch(get: impl Fn(*mut core::ffi::c_void, &mut u32) -> u32) -> Result<Vec<u64>, TableError> {
    let mut size = 0u32;
    get(std::ptr::null_mut(), &mut size);
    for _ in 0..5 {
        // Room for rows added between the two calls.
        let mut buf = vec![0u64; (size as usize + 8192) / 8 + 1];
        let mut len = (buf.len() * 8) as u32;
        match get(buf.as_mut_ptr() as *mut _, &mut len) {
            0 => return Ok(buf),
            ERROR_INSUFFICIENT_BUFFER => size = len,
            _ => return Err(TableError),
        }
    }
    Err(TableError)
}

/// The rows of a fetched table whose header is `dwNumEntries` then rows.
unsafe fn rows_of<R>(buf: &[u64]) -> &[R] {
    let n = *(buf.as_ptr() as *const u32) as usize;
    let first = (buf.as_ptr() as *const u8).add(4);
    // The table array starts at the row type's alignment after the count.
    let offset = first.align_offset(std::mem::align_of::<R>());
    std::slice::from_raw_parts(first.add(offset) as *const R, n)
}

fn tcp(family: u32, now: i64) -> Table {
    let buf = fetch(|p, size| unsafe { GetExtendedTcpTable(p, size, 0, family, TCP_TABLE_OWNER_MODULE_ALL, 0) })?;
    Ok(unsafe {
        if family == AF_INET {
            rows_of::<MIB_TCPROW_OWNER_MODULE>(&buf)
                .iter()
                .filter_map(|r| {
                    let l = IpAddr::V4(Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes()));
                    let rem = IpAddr::V4(Ipv4Addr::from(r.dwRemoteAddr.to_ne_bytes()));
                    let row = tcp_row(l, r.dwLocalPort, rem, r.dwRemotePort, r.dwState, r.dwOwningPid)?;
                    Some((row, socket_age(r.liCreateTimestamp, now)))
                })
                .collect()
        } else {
            rows_of::<MIB_TCP6ROW_OWNER_MODULE>(&buf)
                .iter()
                .filter_map(|r| {
                    let l = IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr));
                    let rem = IpAddr::V6(Ipv6Addr::from(r.ucRemoteAddr));
                    let row = tcp_row(l, r.dwLocalPort, rem, r.dwRemotePort, r.dwState, r.dwOwningPid)?;
                    Some((row, socket_age(r.liCreateTimestamp, now)))
                })
                .collect()
        }
    })
}

fn udp(family: u32, now: i64) -> Table {
    let buf = fetch(|p, size| unsafe { GetExtendedUdpTable(p, size, 0, family, UDP_TABLE_OWNER_MODULE, 0) })?;
    Ok(unsafe {
        if family == AF_INET {
            rows_of::<MIB_UDPROW_OWNER_MODULE>(&buf)
                .iter()
                .map(|r| {
                    let local = IpAddr::V4(Ipv4Addr::from(r.dwLocalAddr.to_ne_bytes()));
                    (udp_row(local, r.dwLocalPort, r.dwOwningPid), socket_age(r.liCreateTimestamp, now))
                })
                .collect()
        } else {
            rows_of::<MIB_UDP6ROW_OWNER_MODULE>(&buf)
                .iter()
                .map(|r| {
                    let local = IpAddr::V6(Ipv6Addr::from(r.ucLocalAddr));
                    (udp_row(local, r.dwLocalPort, r.dwOwningPid), socket_age(r.liCreateTimestamp, now))
                })
                .collect()
        }
    })
}
