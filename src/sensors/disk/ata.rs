//! ATA S.M.A.R.T. (pure): decoding of the SMART READ DATA page (attributes)
//! and the SMART READ THRESHOLDS page of SATA disks, and their readings
//! under the disk's S.M.A.R.T. and Temperatures groups.

use std::time::Instant;

use super::SMART_SOURCE as SOURCE;
use crate::sensors::model::{Device, Group, Reading, Unit};
#[cfg(test)]
use super::{NEEDS_ADMIN, PROBE_OFF};

/// A SMART page: 512 bytes, 30 entries of 12 bytes after a 2-byte revision.
const PAGE: usize = 512;
const SLOTS: usize = 30;
const ENTRY: usize = 12;

/// One vendor attribute of the SMART READ DATA page.
#[derive(Clone, Debug, PartialEq)]
pub struct Attribute {
    pub id: u8,
    /// Normalized value (higher is better).
    pub current: u8,
    pub worst: u8,
    /// The 48-bit raw value.
    pub raw: u64,
    /// From the thresholds page, when it was read.
    pub threshold: Option<u8>,
}

/// The attributes a SATA disk reports.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AtaSmart {
    pub attributes: Vec<Attribute>,
}

impl AtaSmart {
    /// Decodes the 512-byte attributes page and, when given, the 512-byte
    /// thresholds page; `None` when a buffer is too short.
    pub fn decode(data: &[u8], thresholds: Option<&[u8]>) -> Option<Self> {
        if data.len() < PAGE || thresholds.is_some_and(|t| t.len() < PAGE) {
            return None;
        }
        let entry = |slot: usize| 2 + ENTRY * slot;
        let threshold = |id: u8| {
            let t = thresholds?;
            (0..SLOTS).map(entry).find(|&at| t[at] == id).map(|at| t[at + 1])
        };
        let attributes = (0..SLOTS)
            .map(entry)
            .filter(|&at| data[at] != 0)
            .map(|at| {
                let mut raw = [0u8; 8];
                raw[..6].copy_from_slice(&data[at + 5..at + 11]);
                let id = data[at];
                Attribute { id, current: data[at + 3], worst: data[at + 4], raw: u64::from_le_bytes(raw), threshold: threshold(id) }
            })
            .collect();
        Some(AtaSmart { attributes })
    }
}

impl AtaSmart {
    /// The attribute `id`, if the disk reports it.
    fn get(&self, id: u8) -> Option<&Attribute> {
        self.attributes.iter().find(|a| a.id == id)
    }

    /// "Healthy", or "Warning: SMART threshold exceeded" and which
    /// attributes are at or below their threshold.
    pub fn health(&self) -> String {
        let failing: Vec<String> = self
            .attributes
            .iter()
            .filter(|a| a.threshold.is_some_and(|t| t > 0 && a.current <= t))
            .map(|a| COUNTERS.iter().find(|c| c.0 == a.id).map_or_else(|| format!("attribute {}", a.id), |c| c.2.to_string()))
            .collect();
        if failing.is_empty() {
            "Healthy".into()
        } else {
            format!("Warning: SMART threshold exceeded ({})", failing.join(", "))
        }
    }
}

/// The ATA attributes shown (id, metric, display name, unit), in display
/// order. Ids shared with NVMe mean the same thing.
const COUNTERS: [(u8, &str, &str, Unit); 21] = [
    (4, "start_stop_cycles", "Start-Stop Cycles", Unit::Count),
    (5, "reallocated_sectors", "Reallocated Sectors", Unit::Count),
    (9, "power_on_hours", "Power-on Time", Unit::Hours),
    (12, "power_cycles", "Power Cycles", Unit::Count),
    (171, "program_failures", "Program Failures", Unit::Count),
    (172, "erase_failures", "Erase Failures", Unit::Count),
    (174, "unexpected_power_losses", "Unexpected Power Losses", Unit::Count),
    (181, "program_failures_total", "Program Failures (Total)", Unit::Count),
    (182, "erase_failures_total", "Erase Failures (Total)", Unit::Count),
    (183, "sata_downshifts", "SATA Downshifts", Unit::Count),
    (184, "end_to_end_errors", "End-to-End Errors", Unit::Count),
    (187, "reported_uncorrectable_errors", "Reported Uncorrectable Errors", Unit::Count),
    (188, "command_timeouts", "Command Timeouts", Unit::Count),
    (189, "high_fly_writes", "High Fly Writes", Unit::Count),
    (193, "load_unload_cycles", "Load-Unload Cycles", Unit::Count),
    (196, "reallocation_events", "Reallocation Events", Unit::Count),
    (197, "pending_sectors", "Pending Sectors", Unit::Count),
    (198, "offline_uncorrectable_sectors", "Offline Uncorrectable Sectors", Unit::Count),
    (199, "interface_crc_errors", "Interface CRC Errors", Unit::Count),
    (242, "data_read_bytes", "Data Read", Unit::Bytes),
    (241, "data_written_bytes", "Data Written", Unit::Bytes),
];

/// Drive temperature (°C in the raw value's low byte).
const TEMPERATURE: u8 = 194;
/// Total LBAs read / written count 512-byte sectors.
const LBA_BYTES: f64 = 512.0;

/// The shown value of an attribute's raw counter: most are 48-bit
/// counters; vendors pack other fields above Power-on Hours' low 32 bits
/// and Command Timeouts' low 16 bits.
fn value(a: &Attribute) -> f64 {
    match a.id {
        9 => (a.raw & 0xFFFF_FFFF) as f64,
        188 => (a.raw & 0xFFFF) as f64,
        241 | 242 => a.raw as f64 * LBA_BYTES,
        _ => a.raw as f64,
    }
}

/// What the last attributes query of a SATA disk gave.
pub type Smart = super::Probe<AtaSmart>;

/// Reads the attributes and thresholds pages of `\\.\PhysicalDrive{n}`
/// (SMART_RCV_DRIVE_DATA, SMART READ DATA / READ THRESHOLDS). The caller
/// checks administrator rights and the Manual SMART Probe consent
/// ([`super::blocked`]).
#[cfg(windows)]
pub fn query(n: u32) -> Smart {
    use windows_sys::Win32::System::IO::DeviceIoControl;

    const SMART_RCV_DRIVE_DATA: u32 = 0x0007_C088;
    const READ_ATTRIBUTES: u8 = 0xD0;
    const READ_THRESHOLDS: u8 = 0xD1;
    // SENDCMDINPARAMS (packed): cBufferSize u32, IDEREGS (8 bytes),
    // bDriveNumber, 3 reserved, 4 reserved u32, then the buffer.
    const IN: usize = 4 + 8 + 1 + 3 + 16;
    // SENDCMDOUTPARAMS: cBufferSize u32, DRIVERSTATUS (12 bytes), the page.
    const OUT_HEADER: usize = 4 + 12;

    let drive = match super::Drive::open(n) {
        Ok(d) => d,
        Err(why) => return Smart::Failed(why),
    };
    let read = |feature: u8| -> Option<Vec<u8>> {
        let mut input = [0u8; IN + 1];
        input[0..4].copy_from_slice(&(PAGE as u32).to_le_bytes());
        // Features, sector count, sector number, cylinder low/high (SMART
        // signature 4Fh/C2h), drive/head, command (SMART B0h).
        input[4..11].copy_from_slice(&[feature, 1, 1, 0x4F, 0xC2, 0xA0 | (((n & 1) as u8) << 4), 0xB0]);
        input[12] = n as u8;
        let mut out = vec![0u8; OUT_HEADER + PAGE];
        let mut returned = 0u32;
        // SAFETY: `drive` is an open handle; `input` holds IN readable
        // bytes and `out` its full length writable, both alive for the
        // call; no OVERLAPPED (synchronous).
        let ok = unsafe {
            DeviceIoControl(drive.raw(), SMART_RCV_DRIVE_DATA, input.as_ptr().cast(), IN as u32, out.as_mut_ptr().cast(), out.len() as u32, &mut returned, std::ptr::null_mut())
        } != 0;
        (ok && returned as usize >= OUT_HEADER + PAGE).then(|| out.split_off(OUT_HEADER))
    };
    let data = read(READ_ATTRIBUTES);
    let thresholds = data.as_ref().and_then(|_| read(READ_THRESHOLDS));
    drop(drive);
    match data.as_deref().and_then(|d| AtaSmart::decode(d, thresholds.as_deref())) {
        Some(s) => Smart::Read(s),
        None => Smart::Failed("the disk gives no SMART data".into()),
    }
}

/// The S.M.A.R.T. rows ("Health" first: the group's summary, then the
/// attributes the disk reports) and Drive Temperature of SATA disk `n`.
pub fn readings(n: u32, disk: &Device, smart: &Smart, at: Instant) -> Vec<Reading> {
    let (data, why) = smart.value();
    let id = |what: &str| format!("disk{n}.{what}");
    let mut out = vec![Reading::state(&id("smart.health"), disk.clone(), Group::Smart, "Health", SOURCE, at).with_text(data.map(AtaSmart::health), why)];
    if let Some(d) = data {
        for (attr, what, name, unit) in COUNTERS {
            if let Some(a) = d.get(attr) {
                out.push(Reading::measure(&id(&format!("smart.{what}")), disk.clone(), Group::Smart, name, unit, SOURCE, at).with(Some(value(a)), "").counter());
            }
        }
    }
    let temperature = data.and_then(|d| d.get(TEMPERATURE)).map(|a| f64::from((a.raw & 0xFF) as u8));
    if data.is_none() || temperature.is_some() {
        out.push(Reading::measure(&id("temperature.drive_c"), disk.clone(), Group::Temperatures, "Drive Temperature", Unit::Celsius, SOURCE, at).with(temperature, why));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{DeviceKind, Frame, Kind};

    /// An entry of the attributes page: id, flags, current, worst, raw[6], reserved.
    fn put(page: &mut [u8], slot: usize, id: u8, current: u8, raw: u64) {
        let at = 2 + 12 * slot;
        page[at] = id;
        page[at + 3] = current;
        page[at + 4] = current;
        page[at + 5..at + 11].copy_from_slice(&raw.to_le_bytes()[..6]);
    }

    /// A SATA SSD's attributes page (little endian), as smartctl would list it.
    pub(super) fn sample_data() -> Vec<u8> {
        let mut page = vec![0u8; 512];
        page[0] = 0x10; // revision
        put(&mut page, 0, 5, 100, 2); // reallocated sectors
        put(&mut page, 1, 9, 95, 0x0001_0000_30B9); // power-on hours 12,473 (high bytes: vendor)
        put(&mut page, 2, 12, 99, 1_234); // power cycles
        put(&mut page, 3, 194, 67, 0x0028_0014_0021); // 33 °C (min 20, max 40)
        put(&mut page, 4, 199, 100, 0); // interface CRC errors
        put(&mut page, 5, 241, 99, 20_000_000_000); // total LBAs written
        put(&mut page, 6, 242, 99, 30_000_000_000); // total LBAs read
        page
    }

    /// The thresholds page: id, threshold, reserved[10] per entry.
    pub(super) fn sample_thresholds() -> Vec<u8> {
        let mut page = vec![0u8; 512];
        for (slot, (id, t)) in [(5u8, 10u8), (9, 0), (12, 0), (194, 0), (199, 0), (241, 0), (242, 0)].iter().enumerate() {
            page[2 + 12 * slot] = *id;
            page[3 + 12 * slot] = *t;
        }
        page
    }

    #[test]
    fn decodes_the_ata_attributes_page() {
        let s = AtaSmart::decode(&sample_data(), Some(&sample_thresholds())).unwrap();
        let ids: Vec<u8> = s.attributes.iter().map(|a| a.id).collect();
        assert_eq!(ids, vec![5, 9, 12, 194, 199, 241, 242], "empty slots (id 0) left out");
        let realloc = &s.attributes[0];
        assert_eq!((realloc.current, realloc.worst, realloc.raw, realloc.threshold), (100, 100, 2, Some(10)));
        assert_eq!(s.attributes[5].raw, 20_000_000_000, "48-bit raw value");
    }

    #[test]
    fn a_short_buffer_is_not_an_attributes_page() {
        assert_eq!(AtaSmart::decode(&[0u8; 100], None), None);
        assert_eq!(AtaSmart::decode(&sample_data(), Some(&[0u8; 10])), None);
    }

    fn frame(smart: &Smart) -> Frame {
        let disk = Device::new(DeviceKind::Disk, "disk1", "Samsung SSD 870 EVO 1TB");
        Frame::build(1, &[readings(1, &disk, smart, Instant::now())], Vec::new())
    }

    fn text(frame: &Frame, id: &str) -> String {
        frame.reading(id).unwrap_or_else(|| panic!("no {id}")).text()
    }

    fn read() -> Smart {
        Smart::Read(AtaSmart::decode(&sample_data(), Some(&sample_thresholds())).unwrap())
    }

    #[test]
    fn ata_counters_use_their_display_names_with_health_first() {
        let frame = frame(&read());
        let smart = frame.devices[0].groups.iter().find(|g| g.group == Group::Smart).unwrap();
        let names: Vec<_> = smart.readings.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, vec!["Health", "Reallocated Sectors", "Power-on Time", "Power Cycles", "Interface CRC Errors", "Data Read", "Data Written"]);
        assert!(smart.readings.iter().all(|r| r.kind == Kind::Counter), "no Min/Max");
        assert_eq!(text(&frame, "disk1.smart.health"), "Healthy");
        assert_eq!(text(&frame, "disk1.smart.reallocated_sectors"), "2");
        assert_eq!(text(&frame, "disk1.smart.power_on_hours"), "12,473 h", "vendor bytes above 32 bits ignored");
        assert_eq!(text(&frame, "disk1.smart.power_cycles"), "1,234");
        assert_eq!(text(&frame, "disk1.smart.data_written_bytes"), "10.24 TB", "LBAs of 512 bytes");
        assert_eq!(text(&frame, "disk1.smart.data_read_bytes"), "15.36 TB");
    }

    #[test]
    fn drive_temperature_is_the_low_byte_and_has_min_max() {
        let frame = frame(&read());
        let temps = &frame.devices[0].groups[0];
        assert_eq!(temps.group, Group::Temperatures, "Temperatures come first");
        let t = frame.reading("disk1.temperature.drive_c").unwrap();
        assert_eq!((t.name.as_str(), t.text(), t.kind), ("Drive Temperature", "33.0 °C".to_string(), Kind::Measure));
    }

    #[test]
    fn an_attribute_at_or_below_its_threshold_is_a_warning() {
        let mut data = sample_data();
        data[2 + 3] = 10; // reallocated sectors: current 10, threshold 10
        let frame = frame(&Smart::Read(AtaSmart::decode(&data, Some(&sample_thresholds())).unwrap()));
        assert_eq!(text(&frame, "disk1.smart.health"), "Warning: SMART threshold exceeded (Reallocated Sectors)");
    }

    #[test]
    fn a_disk_not_read_shows_health_and_temperature_with_the_reason() {
        for (smart, why) in [(Smart::NeedsAdmin, NEEDS_ADMIN), (Smart::Off, PROBE_OFF), (Smart::Failed("no answer".into()), "no answer")] {
            let frame = frame(&smart);
            let rows: Vec<_> = frame.readings().map(|r| (r.name.as_str(), r.value, r.text())).collect();
            assert_eq!(rows, vec![("Drive Temperature", None, why.to_string()), ("Health", None, why.to_string())]);
        }
    }

    #[test]
    fn without_the_thresholds_page_no_attribute_has_a_threshold() {
        let s = AtaSmart::decode(&sample_data(), None).unwrap();
        assert!(s.attributes.iter().all(|a| a.threshold.is_none()));
    }
}
