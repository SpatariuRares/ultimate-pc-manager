//! The firmware tables (SMBIOS) as read by `GetSystemFirmwareTable('RSMB')`:
//! the processor's external (bus) clock, the motherboard's name and the
//! memory modules. Pure parsing; the read is in [`crate::sensors::board`].

/// What the firmware says about the board, the bus clock and the memory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Firmware {
    /// Baseboard manufacturer and product ("ASUSTeK COMPUTER INC. ROG STRIX B650E-F").
    pub board: Option<String>,
    /// The processor's external clock (MHz): the bus clock.
    pub bus_mhz: Option<f64>,
    /// The populated memory slots.
    pub modules: Vec<Module>,
}

/// One populated memory slot.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Module {
    pub locator: String,
    pub size_mb: u64,
    /// "DDR4", "DDR5", ...
    pub ddr: Option<&'static str>,
    /// Rated speed (MT/s).
    pub speed_mts: Option<f64>,
    /// Speed the firmware set (MT/s).
    pub configured_mts: Option<f64>,
}

/// Parses the `RawSMBIOSData` buffer (8-byte header, then the table).
pub fn parse(raw: &[u8]) -> Firmware {
    let mut fw = Firmware::default();
    let Some(len) = raw.get(4..8).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { return fw };
    let table = &raw[8..raw.len().min(8 + len)];
    let mut at = 0;
    while at + 4 <= table.len() {
        let (kind, size) = (table[at], table[at + 1] as usize);
        if size < 4 || at + size > table.len() {
            break;
        }
        let area = &table[at..at + size];
        // The strings end with two NULs.
        let mut end = at + size;
        while end + 1 < table.len() && !(table[end] == 0 && table[end + 1] == 0) {
            end += 1;
        }
        if end + 1 >= table.len() {
            break;
        }
        let strings: Vec<String> = table[at + size..end].split(|b| *b == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).trim().to_string()).collect();
        let byte = |o: usize| area.get(o).copied();
        let word = |o: usize| area.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        let text = |o: usize| byte(o).filter(|i| *i > 0).and_then(|i| strings.get(i as usize - 1)).filter(|s| !s.is_empty()).cloned();
        let known = |v: Option<u16>| v.filter(|v| *v != 0 && *v != 0xFFFF).map(f64::from);
        match kind {
            2 if fw.board.is_none() => {
                let name: Vec<String> = [text(4), text(5)].into_iter().flatten().collect();
                fw.board = (!name.is_empty()).then(|| name.join(" "));
            }
            4 if fw.bus_mhz.is_none() => fw.bus_mhz = known(word(0x12)),
            17 => {
                let size_mb = match word(0x0C) {
                    None | Some(0) | Some(0xFFFF) => 0,
                    Some(0x7FFF) => area.get(0x1C..0x20).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as u64),
                    Some(s) if s & 0x8000 != 0 => u64::from(s & 0x7FFF) / 1024,
                    Some(s) => u64::from(s),
                };
                if size_mb > 0 {
                    fw.modules.push(Module {
                        locator: text(0x10).unwrap_or_default(),
                        size_mb,
                        ddr: byte(0x12).and_then(ddr),
                        speed_mts: known(word(0x15)),
                        configured_mts: known(word(0x20)),
                    });
                }
            }
            127 => break,
            _ => {}
        }
        at = end + 2;
    }
    fw
}

/// The name of an SMBIOS memory type.
fn ddr(memory_type: u8) -> Option<&'static str> {
    Some(match memory_type {
        0x12 => "DDR",
        0x13 => "DDR2",
        0x18 => "DDR3",
        0x1A => "DDR4",
        0x1B => "LPDDR",
        0x1C => "LPDDR2",
        0x1D => "LPDDR3",
        0x1E => "LPDDR4",
        0x22 => "DDR5",
        0x23 => "LPDDR5",
        _ => return None,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One SMBIOS structure: formatted area (type, length, handle, `body`)
    /// then its strings.
    pub(crate) fn structure(kind: u8, body: &[u8], strings: &[&str]) -> Vec<u8> {
        let mut s = vec![kind, (4 + body.len()) as u8, 0, 0];
        s.extend_from_slice(body);
        if strings.is_empty() {
            s.extend_from_slice(&[0, 0]);
        }
        for t in strings {
            s.extend_from_slice(t.as_bytes());
            s.push(0);
        }
        if !strings.is_empty() {
            s.push(0);
        }
        s
    }

    /// A `RawSMBIOSData` buffer of these structures (SMBIOS 3.5).
    pub(crate) fn raw(structures: &[Vec<u8>]) -> Vec<u8> {
        let table: Vec<u8> = structures.concat();
        let mut out = vec![0, 3, 5, 0];
        out.extend_from_slice(&(table.len() as u32).to_le_bytes());
        out.extend(table);
        out
    }

    fn word(body: &mut [u8], at: usize, v: u16) {
        body[at - 4..at - 2].copy_from_slice(&v.to_le_bytes());
    }

    /// Type 2: manufacturer = string 1, product = string 2.
    pub(crate) fn board(maker: &str, product: &str) -> Vec<u8> {
        structure(2, &[1, 2, 0, 0], &[maker, product])
    }

    /// Type 4 with an external clock (MHz) at offset 0x12.
    pub(crate) fn processor(external_mhz: u16) -> Vec<u8> {
        let mut body = vec![0u8; 0x2A - 4];
        word(&mut body, 0x12, external_mhz);
        structure(4, &body, &[])
    }

    /// Type 17: size (MB), memory type, speed and configured speed (MT/s);
    /// device locator = string 1.
    pub(crate) fn module(locator: &str, size_mb: u16, memory_type: u8, speed: u16, configured: u16) -> Vec<u8> {
        let mut body = vec![0u8; 0x28 - 4];
        word(&mut body, 0x0C, size_mb);
        body[0x10 - 4] = 1;
        body[0x12 - 4] = memory_type;
        word(&mut body, 0x15, speed);
        word(&mut body, 0x20, configured);
        structure(17, &body, &[locator])
    }

    #[test]
    fn reads_the_board_the_bus_clock_and_the_populated_memory_slots() {
        let fw = parse(&raw(&[
            structure(0, &[1, 2, 0, 0], &["American Megatrends", "1.0"]),
            board("ASUSTeK COMPUTER INC.", "ROG STRIX B650E-F GAMING WIFI"),
            processor(100),
            module("DIMM_A1", 0, 0x02, 0, 0),
            module("DIMM_A2", 16_384, 0x22, 6_000, 4_800),
            module("DIMM_B2", 16_384, 0x22, 6_000, 4_800),
            structure(127, &[], &[]),
        ]));
        assert_eq!(fw.board.as_deref(), Some("ASUSTeK COMPUTER INC. ROG STRIX B650E-F GAMING WIFI"));
        assert_eq!(fw.bus_mhz, Some(100.0));
        assert_eq!(fw.modules.len(), 2, "an empty slot (size 0) is left out");
        let m = &fw.modules[0];
        assert_eq!((m.locator.as_str(), m.size_mb, m.ddr), ("DIMM_A2", 16_384, Some("DDR5")));
        assert_eq!((m.speed_mts, m.configured_mts), (Some(6_000.0), Some(4_800.0)));
    }

    #[test]
    fn unknown_values_and_broken_tables_give_nothing_not_zero() {
        let fw = parse(&raw(&[processor(0), module("DIMM_A1", 8_192, 0x1A, 0, 0)]));
        assert_eq!(fw.bus_mhz, None, "external clock 0 = unknown");
        assert_eq!(fw.board, None);
        assert_eq!((fw.modules[0].ddr, fw.modules[0].speed_mts, fw.modules[0].configured_mts), (Some("DDR4"), None, None));
        assert_eq!(parse(&[]), Firmware::default());
        assert_eq!(parse(&[0, 3, 5, 0, 200, 0, 0, 0, 4, 99]), Firmware::default(), "a truncated table");
    }
}
