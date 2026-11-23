//! The "Filter drivers" list: the file-system minifilters loaded now, as
//! `fltmc filters` shows them. Read-only: there is no unload or any other
//! action. This file holds the parts that need no Windows: what one entry
//! of the Filter Manager's answer means, and how a failed read is told.

/// One loaded minifilter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterDriver {
    pub name: String,
    /// The altitude, as Windows writes it ("328010", "40500.5").
    pub altitude: String,
    /// How many volumes it is attached to.
    pub instances: u32,
    /// The Filter Manager frame it lives in.
    pub frame: u32,
}

/// What reading the loaded filters came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilterRead {
    Filters(Vec<FilterDriver>),
    /// Windows lists filters to administrators only.
    NeedsAdmin,
    /// The read failed, for this reason.
    Failed(String),
}

/// Said in place of the list when it needs administrator rights.
pub const NEEDS_ADMIN: &str = "Needs administrator rights to list filters.";
/// Said under the tab's title.
pub const FILTERS_INTRO: &str = "File-system minifilters loaded now, as fltmc shows them.";
/// The read-only reminder at the right of the tab's strip.
pub const FILTERS_READ_ONLY: &str = "Read-only: unloading a filter can disable security software.";
/// Said in the side area of the tab.
pub const FILTERS_SIDE: &str = "Filter drivers are listed for information only.";

/// `FILTER_INFORMATION_CLASS::FilterAggregateStandardInformation`.
pub const AGGREGATE_STANDARD_INFORMATION: i32 = 2;
/// `FLTFL_ASI_IS_MINIFILTER`.
const IS_MINIFILTER: u32 = 1;

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// A UTF-16 string of `len` bytes at `offset` in `b`.
fn utf16_at(b: &[u8], offset: u16, len: u16) -> Option<String> {
    let bytes = b.get(offset as usize..offset as usize + len as usize)?;
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    Some(String::from_utf16_lossy(&units))
}

/// One `FILTER_AGGREGATE_STANDARD_INFORMATION` entry as the Filter Manager
/// wrote it. A minifilter gives its row; a legacy filter, which has no
/// altitude frame or instances, and a damaged entry give none.
pub fn parse_entry(b: &[u8]) -> Option<FilterDriver> {
    // NextEntryOffset (0), Flags (4), then the MiniFilter member from 8:
    // Flags, FrameID, NumberOfInstances, name length and offset, altitude
    // length and offset. Offsets count from the start of the entry.
    if u32_at(b, 4)? & IS_MINIFILTER == 0 {
        return None;
    }
    Some(FilterDriver {
        frame: u32_at(b, 12)?,
        instances: u32_at(b, 16)?,
        name: utf16_at(b, u16_at(b, 22)?, u16_at(b, 20)?)?,
        altitude: utf16_at(b, u16_at(b, 26)?, u16_at(b, 24)?)?,
    })
}

/// The list in `fltmc` order: highest altitude first.
pub fn sorted(mut list: Vec<FilterDriver>) -> Vec<FilterDriver> {
    let key = |f: &FilterDriver| f.altitude.parse::<f64>().unwrap_or(0.0);
    list.sort_by(|a, b| key(b).total_cmp(&key(a)).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    list
}

/// `HRESULT_FROM_WIN32(ERROR_NO_MORE_ITEMS)`: the list ends.
pub const NO_MORE_ITEMS: i32 = 0x8007_0103_u32 as i32;

/// How a failed Filter Manager call is told: access denied and a missing
/// privilege mean "run as administrator"; anything else is a failure.
pub fn failure(hresult: i32) -> FilterRead {
    match hresult as u32 {
        // E_ACCESSDENIED, ERROR_ACCESS_DENIED, ERROR_PRIVILEGE_NOT_HELD
        0x8007_0005 | 0x8007_0522 | 0x8000_4005 => FilterRead::NeedsAdmin,
        code => FilterRead::Failed(format!("The Filter Manager answered 0x{code:08X}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An entry as the Filter Manager lays it out.
    fn entry(flags: u32, frame: u32, instances: u32, name: &str, altitude: &str) -> Vec<u8> {
        let wide = |s: &str| s.encode_utf16().flat_map(u16::to_le_bytes).collect::<Vec<u8>>();
        let (n, a) = (wide(name), wide(altitude));
        let mut b = Vec::new();
        for v in [0u32, flags, flags, frame, instances] {
            b.extend(v.to_le_bytes());
        }
        let name_at = 28u16;
        let alt_at = name_at + n.len() as u16;
        for v in [n.len() as u16, name_at, a.len() as u16, alt_at] {
            b.extend(v.to_le_bytes());
        }
        b.extend(n);
        b.extend(a);
        b
    }

    fn filter(name: &str, altitude: &str) -> FilterDriver {
        FilterDriver { name: name.into(), altitude: altitude.into(), instances: 1, frame: 0 }
    }

    #[test]
    fn a_minifilter_entry_gives_name_altitude_instances_and_frame() {
        let got = parse_entry(&entry(1, 2, 5, "WdFilter", "328010"));
        assert_eq!(got, Some(FilterDriver { name: "WdFilter".into(), altitude: "328010".into(), instances: 5, frame: 2 }));
    }

    #[test]
    fn a_legacy_or_damaged_entry_gives_no_row() {
        assert_eq!(parse_entry(&entry(2, 0, 0, "Legacy", "1")), None, "a legacy filter");
        let mut cut = entry(1, 0, 1, "WdFilter", "328010");
        cut.truncate(34);
        assert_eq!(parse_entry(&cut), None, "the name runs past the end");
        assert_eq!(parse_entry(&[0u8; 6]), None, "too short for its header");
    }

    #[test]
    fn the_list_runs_from_the_highest_altitude_down_like_fltmc() {
        let list = sorted(vec![filter("low", "40500.5"), filter("high", "409800"), filter("mid", "328010")]);
        let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["high", "mid", "low"]);
    }

    #[test]
    fn access_denied_and_a_missing_privilege_need_administrator_rights_and_other_errors_fail() {
        assert_eq!(failure(0x8007_0005_u32 as i32), FilterRead::NeedsAdmin);
        assert_eq!(failure(0x8007_0522_u32 as i32), FilterRead::NeedsAdmin);
        assert_eq!(failure(0x8007_0057_u32 as i32), FilterRead::Failed("The Filter Manager answered 0x80070057.".into()));
    }
}
