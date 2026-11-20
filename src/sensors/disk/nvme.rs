//! NVMe S.M.A.R.T. (pure): decoding of the NVMe health log (log page 02h,
//! 512 bytes) and its readings under the disk's S.M.A.R.T. and
//! Temperatures groups.

use std::time::Instant;

use super::SMART_SOURCE as SOURCE;
use crate::sensors::model::{Device, Group, Reading, Unit};

#[cfg(test)]
use super::NEEDS_ADMIN;

/// What the critical warning byte's bits mean (NVMe 1.4, figure 194).
const WARNINGS: [&str; 6] = [
    "available spare below threshold",
    "temperature out of range",
    "reliability degraded",
    "read-only mode",
    "volatile memory backup failed",
    "persistent memory region read-only",
];

/// One data unit of Data Read / Written: 1000 sectors of 512 bytes.
const DATA_UNIT_BYTES: f64 = 512_000.0;

/// The fields of the NVMe health log the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    /// Composite temperature, Kelvin.
    pub composite_kelvin: u16,
    pub available_spare: u8,
    pub spare_threshold: u8,
    pub percentage_used: u8,
    pub data_units_read: u128,
    pub data_units_written: u128,
    pub host_read_commands: u128,
    pub host_write_commands: u128,
    /// Minutes.
    pub controller_busy_minutes: u128,
    pub power_cycles: u128,
    pub power_on_hours: u128,
    pub unsafe_shutdowns: u128,
    pub media_errors: u128,
    pub error_log_entries: u128,
    /// Temperature Sensors 1-8, Kelvin; 0 = not implemented.
    pub sensors_kelvin: [u16; 8],
}

impl NvmeHealth {
    /// Decodes the 512-byte health log; `None` when the buffer is too short.
    pub fn decode(log: &[u8]) -> Option<Self> {
        if log.len() < 512 {
            return None;
        }
        let u16_at = |at: usize| u16::from_le_bytes([log[at], log[at + 1]]);
        let u128_at = |at: usize| u128::from_le_bytes(log[at..at + 16].try_into().unwrap());
        let mut sensors_kelvin = [0u16; 8];
        for (i, s) in sensors_kelvin.iter_mut().enumerate() {
            *s = u16_at(200 + 2 * i);
        }
        Some(NvmeHealth {
            critical_warning: log[0],
            composite_kelvin: u16_at(1),
            available_spare: log[3],
            spare_threshold: log[4],
            percentage_used: log[5],
            data_units_read: u128_at(32),
            data_units_written: u128_at(48),
            host_read_commands: u128_at(64),
            host_write_commands: u128_at(80),
            controller_busy_minutes: u128_at(96),
            power_cycles: u128_at(112),
            power_on_hours: u128_at(128),
            unsafe_shutdowns: u128_at(144),
            media_errors: u128_at(160),
            error_log_entries: u128_at(176),
            sensors_kelvin,
        })
    }

    /// "Healthy", or "Warning: " and what the critical warning says.
    pub fn health(&self) -> String {
        let set: Vec<&str> = WARNINGS.iter().enumerate().filter(|(bit, _)| self.critical_warning & (1 << bit) != 0).map(|(_, w)| *w).collect();
        match (set.is_empty(), self.critical_warning) {
            (true, 0) => "Healthy".into(),
            (true, _) => "Warning: unknown critical warning".into(),
            (false, _) => format!("Warning: {}", set.join(", ")),
        }
    }
}

/// Degrees Celsius of an NVMe temperature (whole Kelvin, as the drive
/// computed it from °C + 273, like smartctl shows it).
fn celsius(kelvin: u16) -> f64 {
    f64::from(kelvin) - 273.0
}

/// What the last health query of a disk gave.
pub type Smart = super::Probe<NvmeHealth>;

/// Reads the health log of `\\.\PhysicalDrive{n}` through the storage
/// protocol property query. Not tried without administrator rights (the
/// caller checks them too, [`super::blocked`]).
#[cfg(windows)]
pub fn query(n: u32) -> Smart {
    use windows_sys::Win32::System::Ioctl::IOCTL_STORAGE_QUERY_PROPERTY;
    use windows_sys::Win32::System::IO::DeviceIoControl;

    // STORAGE_PROPERTY_QUERY header (PropertyId, QueryType), then
    // STORAGE_PROTOCOL_SPECIFIC_DATA (ten u32), then the 512-byte log.
    const HEADER: usize = 8;
    const PROTOCOL_DATA: usize = 40;
    const STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY: u32 = 50;
    const PROTOCOL_TYPE_NVME: u32 = 3;
    const NVME_DATA_TYPE_LOG_PAGE: u32 = 2;
    const NVME_LOG_PAGE_HEALTH_INFO: u32 = 2;

    if !crate::platform::is_admin() {
        return Smart::NeedsAdmin;
    }
    let mut buf = vec![0u8; HEADER + PROTOCOL_DATA + 512];
    let fields = [
        STORAGE_DEVICE_PROTOCOL_SPECIFIC_PROPERTY,
        0, // PropertyStandardQuery
        PROTOCOL_TYPE_NVME,
        NVME_DATA_TYPE_LOG_PAGE,
        NVME_LOG_PAGE_HEALTH_INFO,
        0,                     // sub value: the whole controller
        PROTOCOL_DATA as u32,  // data offset, from the protocol data
        512,                   // data length
    ];
    for (i, f) in fields.iter().enumerate() {
        buf[4 * i..4 * i + 4].copy_from_slice(&f.to_le_bytes());
    }
    let drive = match super::Drive::open(n) {
        Ok(d) => d,
        Err(why) => return Smart::Failed(why),
    };
    let mut returned = 0u32;
    let ptr = buf.as_mut_ptr() as *mut core::ffi::c_void;
    let len = buf.len() as u32;
    // SAFETY: `drive` is an open handle; `buf` is both the input (the
    // query) and the output, `len` bytes long, alive for the whole call;
    // no OVERLAPPED (synchronous).
    let ok = unsafe { DeviceIoControl(drive.raw(), IOCTL_STORAGE_QUERY_PROPERTY, ptr, len, ptr, len, &mut returned, std::ptr::null_mut()) } != 0;
    if !ok {
        return Smart::Failed("the disk gives no health log".into());
    }
    // Reply: STORAGE_PROTOCOL_DATA_DESCRIPTOR (Version, Size), then the
    // protocol data, whose offset field says where the log starts.
    let offset = u32::from_le_bytes(buf[HEADER + 16..HEADER + 20].try_into().unwrap()) as usize;
    match buf.get(HEADER + offset..).and_then(NvmeHealth::decode) {
        Some(h) => Smart::Read(h),
        None => Smart::Failed("the disk gives no health log".into()),
    }
}

/// The S.M.A.R.T. rows ("Health" first: the group's summary) and the
/// Temperatures rows of NVMe disk `n`.
pub fn readings(n: u32, disk: &Device, smart: &Smart, at: Instant) -> Vec<Reading> {
    let (health, why) = smart.value();
    let id = |what: &str| format!("disk{n}.{what}");
    let counter = |what: &str, name: &str, unit: Unit, value: Option<f64>| {
        Reading::measure(&id(what), disk.clone(), Group::Smart, name, unit, SOURCE, at).with(value, why).counter()
    };
    let n128 = |f: fn(&NvmeHealth) -> u128| health.map(|h| f(h) as f64);
    let mut out = vec![
        Reading::state(&id("smart.health"), disk.clone(), Group::Smart, "Health", SOURCE, at).with_text(health.map(NvmeHealth::health), why),
        counter("smart.available_spare_percent", "Available Spare", Unit::Percent, health.map(|h| f64::from(h.available_spare))),
        counter("smart.available_spare_threshold_percent", "Available Spare Threshold", Unit::Percent, health.map(|h| f64::from(h.spare_threshold))),
        counter("smart.percentage_used", "Percentage Used", Unit::Percent, health.map(|h| f64::from(h.percentage_used))),
        counter("smart.data_read_bytes", "Data Read", Unit::Bytes, n128(|h| h.data_units_read).map(|u| u * DATA_UNIT_BYTES)),
        counter("smart.data_written_bytes", "Data Written", Unit::Bytes, n128(|h| h.data_units_written).map(|u| u * DATA_UNIT_BYTES)),
        counter("smart.host_read_commands", "Host Read Commands", Unit::Count, n128(|h| h.host_read_commands)),
        counter("smart.host_write_commands", "Host Write Commands", Unit::Count, n128(|h| h.host_write_commands)),
        counter("smart.controller_busy_s", "Controller Busy Time", Unit::Seconds, n128(|h| h.controller_busy_minutes).map(|m| m * 60.0)),
        counter("smart.power_cycles", "Power Cycles", Unit::Count, n128(|h| h.power_cycles)),
        counter("smart.power_on_hours", "Power On Hours", Unit::Hours, n128(|h| h.power_on_hours)),
        counter("smart.unsafe_shutdowns", "Unsafe Shutdowns", Unit::Count, n128(|h| h.unsafe_shutdowns)),
        counter("smart.media_errors", "Media and Data Integrity Errors", Unit::Count, n128(|h| h.media_errors)),
        counter("smart.error_log_entries", "Error Log Entries", Unit::Count, n128(|h| h.error_log_entries)),
    ];
    let temperature = |what: &str, name: &str, kelvin: Option<u16>| {
        Reading::measure(&id(what), disk.clone(), Group::Temperatures, name, Unit::Celsius, SOURCE, at).with(kelvin.map(celsius), why)
    };
    out.push(temperature("temperature.composite_c", "Composite", health.map(|h| h.composite_kelvin)));
    if let Some(h) = health {
        for (i, &k) in h.sensors_kelvin.iter().enumerate().filter(|(_, &k)| k != 0) {
            out.push(temperature(&format!("temperature.sensor{}_c", i + 1), &format!("Sensor {}", i + 1), Some(k)));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{DeviceKind, Frame, Kind};

    /// A health log as an NVMe drive returns it (little endian).
    fn sample_log() -> Vec<u8> {
        let mut log = vec![0u8; 512];
        log[0] = 0; // critical warning
        log[1..3].copy_from_slice(&318u16.to_le_bytes()); // composite, K
        log[3] = 100; // available spare
        log[4] = 10; // threshold
        log[5] = 3; // percentage used
        let mut put = |at: usize, v: u128| log[at..at + 16].copy_from_slice(&v.to_le_bytes());
        put(32, 30_000_000); // data units read (×512,000 bytes)
        put(48, 20_000_000); // data units written
        put(64, 1_000_000); // host reads
        put(80, 2_000_000); // host writes
        put(96, 1_500); // controller busy, minutes
        put(112, 1_234); // power cycles
        put(128, 12_345); // power on hours
        put(144, 56); // unsafe shutdowns
        put(160, 0); // media errors
        put(176, 7); // error log entries
        log[200..202].copy_from_slice(&320u16.to_le_bytes()); // sensor 1
        log
    }

    #[test]
    fn decodes_the_nvme_health_log() {
        let h = NvmeHealth::decode(&sample_log()).unwrap();
        assert_eq!((h.available_spare, h.spare_threshold, h.percentage_used), (100, 10, 3));
        assert_eq!(h.power_on_hours, 12_345);
        assert_eq!(h.critical_warning, 0);
        assert_eq!(h.composite_kelvin, 318);
        assert_eq!(h.sensors_kelvin[0], 320);
        assert_eq!(h.health(), "Healthy");
    }

    #[test]
    fn a_short_buffer_is_not_a_health_log() {
        assert_eq!(NvmeHealth::decode(&[0u8; 100]), None);
    }

    #[test]
    fn the_critical_warning_says_what_is_wrong() {
        let mut log = sample_log();
        log[0] = 0b0000_0101;
        let h = NvmeHealth::decode(&log).unwrap();
        assert_eq!(h.health(), "Warning: available spare below threshold, reliability degraded");
    }

    fn frame(smart: &Smart) -> Frame {
        let disk = Device::new(DeviceKind::Disk, "disk0", "Samsung SSD 990 PRO 2TB");
        Frame::build(1, &[readings(0, &disk, smart, Instant::now())], Vec::new())
    }

    fn text(frame: &Frame, id: &str) -> String {
        frame.reading(id).unwrap_or_else(|| panic!("no {id}")).text()
    }

    #[test]
    fn smart_rows_are_counters_with_healthy_first() {
        let frame = frame(&Smart::Read(NvmeHealth::decode(&sample_log()).unwrap()));
        let smart = frame.devices[0].groups.iter().find(|g| g.group == Group::Smart).unwrap();
        let names: Vec<_> = smart.readings.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "Health",
                "Available Spare",
                "Available Spare Threshold",
                "Percentage Used",
                "Data Read",
                "Data Written",
                "Host Read Commands",
                "Host Write Commands",
                "Controller Busy Time",
                "Power Cycles",
                "Power On Hours",
                "Unsafe Shutdowns",
                "Media and Data Integrity Errors",
                "Error Log Entries",
            ]
        );
        assert!(smart.readings.iter().all(|r| r.kind == Kind::Counter), "no Min/Max");
        assert_eq!(text(&frame, "disk0.smart.health"), "Healthy");
        assert_eq!(text(&frame, "disk0.smart.available_spare_percent"), "100 %");
        assert_eq!(text(&frame, "disk0.smart.percentage_used"), "3 %");
        assert_eq!(text(&frame, "disk0.smart.data_read_bytes"), "15.36 TB");
        assert_eq!(text(&frame, "disk0.smart.data_written_bytes"), "10.24 TB");
        assert_eq!(text(&frame, "disk0.smart.host_write_commands"), "2,000,000");
        assert_eq!(text(&frame, "disk0.smart.controller_busy_s"), "25 h 0 min");
        assert_eq!(text(&frame, "disk0.smart.power_on_hours"), "12,345 h");
        assert_eq!(text(&frame, "disk0.smart.error_log_entries"), "7");
    }

    #[test]
    fn composite_temperature_has_min_max_and_sensors_follow() {
        let frame = frame(&Smart::Read(NvmeHealth::decode(&sample_log()).unwrap()));
        let temps = &frame.devices[0].groups[0];
        assert_eq!(temps.group, Group::Temperatures, "Temperatures come first");
        let names: Vec<_> = temps.readings.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Composite", "Sensor 1"], "unimplemented sensors (0 K) left out");
        let composite = frame.reading("disk0.temperature.composite_c").unwrap();
        assert_eq!((composite.text(), composite.kind), ("45.0 °C".to_string(), Kind::Measure));
    }

    #[test]
    fn without_administrator_every_row_says_so() {
        let frame = frame(&Smart::NeedsAdmin);
        let rows: Vec<_> = frame.readings().collect();
        assert_eq!(rows.len(), 15, "14 S.M.A.R.T. rows + Composite");
        for r in rows {
            assert_eq!((r.value, r.text()), (None, NEEDS_ADMIN.to_string()), "{}", r.id);
        }
    }

    #[test]
    fn a_failed_query_says_why() {
        let frame = frame(&Smart::Failed("the disk does not answer".into()));
        assert_eq!(text(&frame, "disk0.smart.health"), "the disk does not answer");
    }
}
