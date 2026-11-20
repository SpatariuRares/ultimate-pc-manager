//! Video cards (pure): the readings of one GPU, the line on its heading
//! ("Clock %1   Power %2   Memory %3") and how the cards a vendor library
//! sees are matched to the cards DXCore lists. The providers are
//! [`super::dxcore`] (lists every card, grey rows with the reason) and
//! [`super::nvml`] (NVIDIA values).

use std::time::Instant;

use super::model::{Device, DeviceNode, Group, Reading, Unit};

/// One read of a card's raw values; `None` = not read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuSample {
    pub graphics_mhz: Option<f64>,
    pub graphics_max_mhz: Option<f64>,
    pub memory_mhz: Option<f64>,
    pub memory_max_mhz: Option<f64>,
    pub power_w: Option<f64>,
    pub power_limit_w: Option<f64>,
    pub temperature_c: Option<f64>,
    pub hotspot_c: Option<f64>,
    pub fan_percent: Option<f64>,
    pub load_percent: Option<f64>,
    pub encoder_percent: Option<f64>,
    pub decoder_percent: Option<f64>,
}

/// The metric id of a card's reading: `gpu.<key>.<what>`.
pub fn metric(device: &Device, what: &str) -> String {
    format!("gpu.{}.{what}", device.key)
}

/// The graphics clock (`gpu.<key>.clock.graphics_hz`), first on the heading line.
pub const GRAPHICS_CLOCK: &str = "clock.graphics_hz";
/// The memory clock (`gpu.<key>.clock.memory_hz`), last on the heading line.
pub const MEMORY_CLOCK: &str = "clock.memory_hz";
/// The whole card's power (`gpu.<key>.power.board_w`): on the heading line
/// and summed into This PC › Powers › GPU ([`TOTAL_POWER`]).
pub const BOARD_POWER: &str = "power.board_w";
/// The card's temperature (`gpu.<key>.temperature.gpu_c`): on Summary's GPU
/// tile and Power & Frequency's card heading.
pub const TEMPERATURE: &str = "temperature.gpu_c";

/// This PC › Powers › GPU (the row Energy Meter always lists).
pub const TOTAL_POWER: &str = "pc.power.gpu_w";
/// "Read from" of [`TOTAL_POWER`] once the cards' values fill it.
pub const TOTAL_POWER_SOURCE: &str = "sum of the cards' Board Power";

/// Fills This PC › Powers › GPU with the sum of the Board Power of every
/// card that has one (NVML, ADLX, IGCL), integrated cards included: the
/// power of all the PC's graphics. A row that already has a value, or no
/// card with a value, leaves it as it is (with its own reason).
pub(super) fn fill_total_power(merged: &mut [Reading]) {
    let suffix = format!(".{BOARD_POWER}");
    let cards: Vec<(f64, Instant)> = merged.iter().filter(|r| r.id.starts_with("gpu.") && r.id.ends_with(&suffix)).filter_map(|r| r.value.map(|v| (v, r.at))).collect();
    let Some(at) = cards.iter().map(|(_, at)| *at).max() else { return };
    if let Some(total) = merged.iter_mut().find(|r| r.id == TOTAL_POWER && r.value.is_none()) {
        total.value = Some(cards.iter().map(|(v, _)| v).sum());
        (total.unavailable, total.source, total.at) = (None, TOTAL_POWER_SOURCE, at);
    }
}

impl GpuSample {
    /// The readings of this card, read by `source`; a value it lacks is
    /// unavailable for `why`.
    pub fn readings(&self, device: &Device, source: &'static str, why: &str, at: Instant) -> Vec<Reading> {
        let mhz = |v: Option<f64>| v.map(|v| v * 1e6);
        let rows: [(&str, Group, &str, Unit, Option<f64>); 12] = [
            (TEMPERATURE, Group::Temperatures, "GPU Temperature", Unit::Celsius, self.temperature_c),
            ("temperature.hotspot_c", Group::Temperatures, "Hot Spot", Unit::Celsius, self.hotspot_c),
            ("utilization.gpu_percent", Group::Utilization, "GPU Load", Unit::Percent, self.load_percent),
            ("utilization.encoder_percent", Group::Utilization, "Video Encode", Unit::Percent, self.encoder_percent),
            ("utilization.decoder_percent", Group::Utilization, "Video Decode", Unit::Percent, self.decoder_percent),
            (GRAPHICS_CLOCK, Group::Clocks, "Graphics Clock", Unit::Hz, mhz(self.graphics_mhz)),
            ("clock.graphics_max_hz", Group::Clocks, "Graphics Max Clock", Unit::Hz, mhz(self.graphics_max_mhz)),
            (MEMORY_CLOCK, Group::Clocks, "Memory Clock", Unit::Hz, mhz(self.memory_mhz)),
            ("clock.memory_max_hz", Group::Clocks, "Memory Max Clock", Unit::Hz, mhz(self.memory_max_mhz)),
            (BOARD_POWER, Group::Powers, "Board Power", Unit::Watt, self.power_w),
            ("power.limit_w", Group::Powers, "Power Limit", Unit::Watt, self.power_limit_w),
            ("fan_percent", Group::Fans, "Fan", Unit::Percent, self.fan_percent),
        ];
        rows.into_iter()
            .map(|(what, group, name, unit, value)| Reading::measure(&metric(device, what), device.clone(), group, name, unit, source, at).with(value, why))
            .collect()
    }
}

/// The line on a card's heading: "Clock %1   Power %2   Memory %3"
/// ("n/a" for a value nobody read).
pub fn summary(card: &DeviceNode) -> String {
    let value = |what: &str| {
        let id = metric(&card.device, what);
        let r = card.groups.iter().flat_map(|g| &g.readings).find(|r| r.id == id);
        r.and_then(|r| r.value.map(|v| r.unit.format(v))).unwrap_or_else(|| "n/a".into())
    };
    format!("Clock {}   Power {}   Memory {}", value(GRAPHICS_CLOCK), value(BOARD_POWER), value(MEMORY_CLOCK))
}

/// NVIDIA's PCI vendor id (NVML reads its cards).
pub const NVIDIA: u32 = 0x10DE;
/// AMD's PCI vendor id (ADLX reads its cards).
pub const AMD: u32 = 0x1002;
/// Intel's PCI vendor id (IGCL reads its cards).
pub const INTEL: u32 = 0x8086;

/// Why a card of this vendor has no values (when no provider gave them).
pub fn no_provider(vendor_id: u32) -> String {
    match vendor_id {
        NVIDIA => "NVML not found (NVIDIA driver)",
        AMD => "ADLX not available (AMD driver)",
        INTEL => "IGCL not available (Intel graphics driver)",
        _ => "no provider for this GPU",
    }
    .into()
}

/// A video card as DXCore or a vendor library names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
}

/// The devices of the cards DXCore lists: `gpu0`, `gpu1`... in its order.
pub fn cards(listed: &[Adapter]) -> Vec<Device> {
    listed.iter().enumerate().map(|(i, a)| Device::gpu(&format!("gpu{i}"), &a.name)).collect()
}

/// The devices of the cards a vendor library sees (`seen`, in its order):
/// each takes the first card DXCore listed with the same PCI ids and not
/// taken yet, so both providers fill the same heading. A card DXCore did
/// not list keeps the library's name under its own key (`prefix` + its
/// index, e.g. `nvidia2`).
pub fn match_cards(listed: &[Adapter], seen: &[Adapter], prefix: &str) -> Vec<Device> {
    let devices = cards(listed);
    let mut taken = vec![false; listed.len()];
    seen.iter()
        .enumerate()
        .map(|(i, s)| {
            let found = listed.iter().enumerate().position(|(j, a)| !taken[j] && a.vendor_id == s.vendor_id && a.device_id == s.device_id);
            match found {
                Some(j) => {
                    taken[j] = true;
                    devices[j].clone()
                }
                None => Device::gpu(&format!("{prefix}{i}"), &s.name),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::Frame;
    use std::time::Instant;

    #[test]
    fn this_pc_gpu_power_is_the_sum_of_the_cards_board_power() {
        let at = Instant::now();
        let energy = crate::sensors::energy::readings(&[], "CPU", at);
        let card = |key: &str, w: Option<f64>, source| GpuSample { power_w: w, ..Default::default() }.readings(&Device::gpu(key, key), source, "n/a", at);
        let frame = Frame::build(1, &[energy.clone(), card("gpu0", Some(285.5), "NVML"), card("gpu1", Some(12.0), "ADLX")], Vec::new());
        let gpu = frame.reading(TOTAL_POWER).unwrap();
        assert_eq!((gpu.text(), gpu.unavailable.as_deref(), gpu.source), ("297.5 W".to_string(), None, TOTAL_POWER_SOURCE));
        assert_eq!(gpu.device, Device::this_pc(), "stays under This PC › Powers");

        let frame = Frame::build(1, &[energy.clone(), card("gpu0", Some(285.5), "NVML"), card("gpu1", None, "DXCore")], Vec::new());
        assert_eq!(frame.reading(TOTAL_POWER).unwrap().text(), "285.5 W", "cards without a value are left out");

        let frame = Frame::build(1, &[energy, card("gpu0", None, "DXCore")], Vec::new());
        let gpu = frame.reading(TOTAL_POWER).unwrap();
        assert_eq!((gpu.value, gpu.source), (None, crate::sensors::energy::SOURCE), "no card read: Energy Meter's reason stays");
    }

    fn rtx() -> Device {
        Device::gpu("gpu0", "NVIDIA GeForce RTX 4080")
    }

    fn text(frame: &Frame, name: &str) -> String {
        frame.readings().find(|r| r.name == name).unwrap_or_else(|| panic!("no row {name}")).text()
    }

    #[test]
    fn an_nvidia_card_shows_clocks_power_temperatures_fan_and_load() {
        let s = GpuSample {
            graphics_mhz: Some(2730.0),
            graphics_max_mhz: Some(3120.0),
            memory_mhz: Some(11201.0),
            memory_max_mhz: Some(11201.0),
            power_w: Some(285.26),
            power_limit_w: Some(320.0),
            temperature_c: Some(61.0),
            hotspot_c: None,
            fan_percent: Some(45.0),
            load_percent: Some(97.0),
            encoder_percent: Some(0.0),
            decoder_percent: Some(12.0),
        };
        let frame = Frame::build(0, &[s.readings(&rtx(), "NVML", "NVML does not report it", Instant::now())], Vec::new());
        let d = &frame.devices[0];
        assert_eq!(d.device.name, "NVIDIA GeForce RTX 4080");
        let groups: Vec<_> = d.groups.iter().map(|g| g.group.name()).collect();
        assert_eq!(groups, vec!["Temperatures", "Utilization", "Clocks", "Powers", "Fans"]);
        assert_eq!(text(&frame, "Graphics Clock"), "2.73 GHz");
        assert_eq!(text(&frame, "Graphics Max Clock"), "3.12 GHz");
        assert_eq!(text(&frame, "Memory Clock"), "11.20 GHz");
        assert_eq!(text(&frame, "Board Power"), "285.3 W");
        assert_eq!(text(&frame, "Power Limit"), "320.0 W");
        assert_eq!(text(&frame, "GPU Temperature"), "61.0 °C");
        assert_eq!(text(&frame, "Hot Spot"), "NVML does not report it");
        assert_eq!(text(&frame, "Fan"), "45 %");
        assert_eq!(text(&frame, "GPU Load"), "97 %");
        assert_eq!(text(&frame, "Video Encode"), "0 %", "an idle encoder is zero, not missing");
        assert_eq!(text(&frame, "Video Decode"), "12 %");
        assert!(frame.readings().all(|r| r.source == "NVML"));
    }

    #[test]
    fn the_gpu_heading_sums_up_clock_power_and_memory() {
        let s = GpuSample { graphics_mhz: Some(2730.0), memory_mhz: Some(11201.0), power_w: Some(285.26), ..Default::default() };
        let frame = Frame::build(0, &[s.readings(&rtx(), "NVML", "-", Instant::now())], Vec::new());
        assert_eq!(summary(&frame.devices[0]), "Clock 2.73 GHz   Power 285.3 W   Memory 11.20 GHz");
    }

    #[test]
    fn a_card_without_a_provider_has_grey_rows_and_a_heading_without_numbers() {
        let at = Instant::now();
        let card = Device::gpu("gpu1", "AMD Radeon(TM) Graphics");
        let frame = Frame::build(0, &[GpuSample::default().readings(&card, "DXCore", &no_provider(0x1002), at)], Vec::new());
        assert!(frame.readings().all(|r| r.value.is_none()), "never shown as zero");
        assert_eq!(text(&frame, "Graphics Clock"), "ADLX not available (AMD driver)");
        assert_eq!(summary(&frame.devices[0]), "Clock n/a   Power n/a   Memory n/a");
        assert_eq!(no_provider(0x10DE), "NVML not found (NVIDIA driver)");
        assert_eq!(no_provider(0x8086), "IGCL not available (Intel graphics driver)");
        assert_eq!(no_provider(0x1234), "no provider for this GPU");
    }

    fn adapter(name: &str, vendor_id: u32, device_id: u32) -> Adapter {
        Adapter { name: name.into(), vendor_id, device_id }
    }

    #[test]
    fn nvidia_cards_get_the_name_and_key_dxcore_gave_them_matched_by_pci_ids() {
        let listed = [adapter("AMD Radeon(TM) Graphics", AMD, 0x164E), adapter("NVIDIA GeForce RTX 4080", NVIDIA, 0x2704), adapter("NVIDIA GeForce RTX 4080", NVIDIA, 0x2704)];
        let seen = [adapter("NVIDIA GeForce RTX 4080 (nvml)", NVIDIA, 0x2704), adapter("NVIDIA GeForce RTX 4080 (nvml)", NVIDIA, 0x2704), adapter("NVIDIA RTX A2000", NVIDIA, 0x2531)];
        let keys: Vec<_> = match_cards(&listed, &seen, "nvidia").into_iter().map(|d| (d.key, d.name)).collect();
        assert_eq!(
            keys,
            vec![
                ("gpu1".into(), "NVIDIA GeForce RTX 4080".into()),
                ("gpu2".into(), "NVIDIA GeForce RTX 4080".into()),
                ("nvidia2".into(), "NVIDIA RTX A2000".into()),
            ],
            "identical cards in order; a card DXCore did not list keeps its own name"
        );
        let keys: Vec<_> = cards(&listed).into_iter().map(|d| d.key).collect();
        assert_eq!(keys, vec!["gpu0", "gpu1", "gpu2"]);
    }

    #[test]
    fn nvml_values_fill_the_grey_rows_dxcore_gave_the_same_card() {
        let at = Instant::now();
        let card = rtx();
        let nvml = GpuSample { graphics_mhz: Some(2730.0), ..Default::default() }.readings(&card, "NVML", "NVML does not report it", at);
        let dxcore = GpuSample::default().readings(&card, "DXCore", &no_provider(NVIDIA), at);
        let frame = Frame::build(0, &[nvml, dxcore], Vec::new());
        assert_eq!(frame.devices.len(), 1, "one heading per card");
        assert_eq!(text(&frame, "Graphics Clock"), "2.73 GHz");
        assert_eq!(text(&frame, "Hot Spot"), "NVML does not report it", "the provider that read the card says why");
    }
}
