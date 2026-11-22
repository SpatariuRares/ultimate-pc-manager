//! Summary's tiles: the tile row (Network, one tile per disk,
//! GPUs, CPU Power), mapped from Performance's device list
//! ([`crate::page_perf::devices`]) so the two pages never disagree, plus the sensor
//! readings Performance does not show (S.M.A.R.T. health, GPU temperature
//! and power). Built with the Summary view, once per sample; the page only
//! draws them.

use egui::Ui;

use crate::core_page::PageCtx;
use crate::app::PerfItem;
use crate::sensors::Frame;
use crate::page_perf::fmt_bytes_rate;
use crate::metrics::combine_adapters;
use crate::perfdata::{counter, DiskLive};
use crate::summary_view::GAP;
use crate::sensors::model::{DeviceKind, Group};

/// What a tile shows when no sensor reads its figure.
pub const NO_READING: &str = "No reading";

/// Which device a tile stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileKind {
    Network,
    /// A physical disk, by its number.
    Disk(u32),
    /// Every GPU, combined.
    Gpus,
    CpuPower,
}

/// One of Summary's tiles: its label, its figure and the lines under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Tile {
    /// Which device the tile stands for.
    pub kind: TileKind,
    /// The tile's title (the device's name).
    pub label: String,
    /// The figure; `None` when nothing reads it ([`NO_READING`]).
    pub value: Option<String>,
    /// The lines under the figure, drawn on one line ([`Tile::note`]).
    pub lines: Vec<String>,
}

impl Tile {
    /// The figure as drawn ([`NO_READING`] when there is none).
    pub fn value_text(&self) -> &str {
        self.value.as_deref().unwrap_or(NO_READING)
    }

    /// The lines under the figure on one line, as drawn.
    pub fn note(&self) -> Option<String> {
        (!self.lines.is_empty()).then(|| self.lines.join(" \u{b7} "))
    }
}

impl crate::app::TaskApp {
    /// The machine's fixed facts (disk models, GPUs): the ones set for a
    /// test, otherwise the machine's.
    pub fn machine(&self) -> &crate::perfdata::StaticInfo {
        crate::page_perf::machine_of(&self.snap, self.machine_info.as_ref())
    }
}

/// Summary's tiles for the sample in `ctx` and the latest sensor `frame`,
/// in this order: Network, one tile per disk, GPUs, CPU Power.
pub fn summary_tiles(ctx: &PageCtx, frame: Option<&Frame>) -> Vec<Tile> {
    let s = ctx.snapshot;
    let fahrenheit = ctx.settings.fahrenheit;
    let read = |id: &str| frame.and_then(|f| f.reading(id)).filter(|r| r.value.is_some()).map(|r| r.text_in(fahrenheit));
    let devices = crate::page_perf::devices(ctx.settings, s, crate::page_perf::machine_of(s, None), ctx.histories.perf);
    let mut tiles = Vec::new();
    // Before the first sample no figure is a reading.
    let waiting = s.seq == 0;
    let adapters = combine_adapters(&s.net).count;
    for d in devices.iter().filter(|d| d.item == PerfItem::Network) {
        // With no adapter, a gap and "No adapter", as on the card (not
        // Performance's "1 network interface" and 0 bps).
        let (value, lines) = match (waiting, adapters) {
            (true, _) => (GAP.to_string(), Vec::new()),
            (false, 0) => (GAP.to_string(), vec!["No adapter".to_string()]),
            (false, _) => (d.line2.clone(), vec![d.line1.clone()]),
        };
        tiles.push(Tile { kind: TileKind::Network, label: d.title.clone(), value: Some(value), lines });
    }
    for d in &devices {
        let PerfItem::Disk(n) = d.item else { continue };
        let live = s.perf.disks.iter().find(|l| l.number == n).filter(|_| !waiting);
        // Each figure only when its counter answered for this disk.
        let figure = |counter: &str, text: &dyn Fn(&DiskLive) -> String| {
            live.filter(|_| s.perf.answered(counter)).map_or_else(|| GAP.to_string(), text)
        };
        let read_rate = figure(counter::DREAD, &|l| fmt_bytes_rate(l.read_bps));
        let write_rate = figure(counter::DWRITE, &|l| fmt_bytes_rate(l.write_bps));
        let active = figure(counter::IDLE, &|_| d.line2.clone());
        let mut lines = vec![d.line1.clone(), format!("Read {read_rate}  Write {write_rate}")];
        // The disk's health only when a sensor read it: never "Healthy"
        // for a disk nobody could ask.
        lines.extend(read(&format!("disk{n}.smart.health")).map(|h| format!("{}: {h}", Group::Smart.name())));
        tiles.push(Tile { kind: TileKind::Disk(n), label: d.title.clone(), value: Some(format!("Active {active}")), lines });
    }
    let gpus: Vec<_> = devices.iter().filter(|d| matches!(d.item, PerfItem::Gpu(_))).collect();
    if !gpus.is_empty() {
        let cards: Vec<_> = frame.map(|f| f.devices.iter().filter(|c| c.device.kind == DeviceKind::Gpu).collect()).unwrap_or_default();
        let lines = gpus
            .iter()
            .map(|d| {
                // The sensors name a card as Windows does, so the name
                // pairs it with Performance's adapter.
                let card = cards.iter().find(|c| c.device.name == d.title);
                let values: Vec<String> = card
                    .map(|c| {
                        [crate::sensors::gpu::TEMPERATURE, crate::sensors::gpu::BOARD_POWER].into_iter().filter_map(|what| read(&crate::sensors::gpu::metric(&c.device, what))).collect()
                    })
                    .unwrap_or_default();
                let values = if values.is_empty() { format!("temperature and power: {}", NO_READING.to_lowercase()) } else { values.join(", ") };
                format!("{}: {}, {values}", d.title, d.line2)
            })
            .collect();
        let util: Vec<&str> = gpus.iter().map(|d| d.line2.as_str()).collect();
        tiles.push(Tile { kind: TileKind::Gpus, label: if gpus.len() == 1 { "GPU".into() } else { "GPUs".into() }, value: Some(util.join(" / ")), lines });
    }
    // Performance's CPU Power device when the Windows energy meter reads
    // the package, else the sensors' package power, else a gap.
    let power = devices
        .iter()
        .find(|d| d.item == PerfItem::Power)
        .filter(|_| s.perf.cpu_power_w.is_some())
        .map(|d| d.line2.clone())
        .or_else(|| read(crate::sensors::energy::PACKAGE));
    tiles.push(Tile { kind: TileKind::CpuPower, label: "CPU Power".into(), value: power, lines: vec!["CPU package".into()] });
    tiles
}

/// Draws the tiles in rows that wrap with the page's width.
pub fn tile_row(ui: &mut Ui, tiles: &[Tile]) {
    if tiles.is_empty() {
        return;
    }
    let gap = 14.0;
    let per_row = ((ui.available_width() + gap) / (220.0 + gap)).floor().max(1.0) as usize;
    let width = (ui.available_width() - gap * (per_row - 1) as f32) / per_row as f32;
    for row in tiles.chunks(per_row) {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for tile in row {
                let note = tile.note();
                crate::widgets::parts::kpi_tile(ui, width, &tile.label, tile.value_text(), note.as_deref(), tile.value.is_none());
            }
        });
        ui.add_space(gap);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::perfdata::{DiskInfo, DiskLive, GpuInfo, GpuLive, PerfLive, StaticInfo};
    use crate::sampler::{NetRow, Snapshot};
    use crate::sensors::model::{Device, Reading};
    use crate::sensors::Frame;
    use crate::settings::Settings;
    use crate::summary_view::SummaryView;

    /// A PC with two disks, one GPU and the CPU package's power.
    fn sample(cpu_power_w: Option<f32>) -> Snapshot {
        Snapshot {
            seq: 1,
            net: vec![
                NetRow { name: "Ethernet".into(), rx_bps: 1000.0, tx_bps: 200.0, total_rx: 5, total_tx: 5 },
                NetRow { name: "Wi-Fi".into(), rx_bps: 24.0, tx_bps: 0.0, total_rx: 9, total_tx: 1 },
            ],
            perf: PerfLive {
                disks: vec![
                    DiskLive { number: 0, active: 1.4, read_bps: 0.0, write_bps: 93_500.0, ..Default::default() },
                    DiskLive { number: 1, active: 12.0, read_bps: 2_048.0, write_bps: 0.0, ..Default::default() },
                ],
                gpus: vec![GpuLive { luid: 7, util: 7.2, ..Default::default() }],
                cpu_power_w,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn machine() -> StaticInfo {
        StaticInfo {
            disks: vec![
                DiskInfo { number: 0, model: "Samsung SSD 990 PRO 2TB".into(), letters: vec!['C'], ..Default::default() },
                DiskInfo { number: 1, model: "WDC WD40EFRX".into(), letters: vec!['D', 'E'], ..Default::default() },
            ],
            gpus: vec![GpuInfo { luid: 7, name: "NVIDIA GeForce RTX 4080".into(), ..Default::default() }],
            ..Default::default()
        }
    }

    /// Sensor readings: disk 0's S.M.A.R.T. health read, disk 1's not
    /// (needs administrator), and the GPU's temperature and Board Power.
    fn sensors() -> Frame {
        let at = Instant::now();
        let health = |n: u32, words: Option<&str>| {
            let disk = Device::new(DeviceKind::Disk, &format!("disk{n}"), &format!("Disk {n}"));
            Reading::state(&format!("disk{n}.smart.health"), disk, Group::Smart, "Health", "t", at).with_text(words.map(String::from), "needs administrator")
        };
        let gpu = crate::sensors::gpu::GpuSample { power_w: Some(285.3), temperature_c: Some(64.0), ..Default::default() };
        let gpu = gpu.readings(&Device::gpu("gpu0", "NVIDIA GeForce RTX 4080"), "t", "not read", at);
        Frame::build(1, &[vec![health(0, Some("Healthy")), health(1, None)], gpu], Vec::new())
    }

    /// The Summary view of `snap`, this machine and these sensor readings.
    fn view_of(mut snap: Snapshot, frame: Option<Frame>) -> SummaryView {
        snap.machine = Some(std::sync::Arc::new(machine()));
        let (short, perf) = (std::collections::VecDeque::new(), crate::core_page::PerfHistory::new());
        let histories = crate::core_page::Histories { cpu: &short, mem: &short, perf: &perf };
        SummaryView::build(&PageCtx { settings: &Settings::default(), snapshot: &snap, histories, system: &crate::system_lists::fake::FakeSystem::default() }, frame.as_ref())
    }

    #[test]
    fn the_tiles_are_network_each_disk_the_gpus_and_cpu_power_from_performances_devices() {
        let view = view_of(sample(Some(45.8)), Some(sensors()));
        let tiles: Vec<(TileKind, &str, &str, Option<String>)> = view.tiles.iter().map(|t| (t.kind, t.label.as_str(), t.value_text(), t.note())).collect();
        assert_eq!(
            tiles,
            vec![
                (TileKind::Network, "Network", "S: 1.60 Kbps  R: 8.19 Kbps", Some("2 network interfaces".to_string())),
                (TileKind::Disk(0), "Samsung SSD 990 PRO 2TB", "Active 1.4%", Some("Disk 0 (C:) \u{b7} Read 0 B/sec  Write 91.3 KB/sec \u{b7} S.M.A.R.T.: Healthy".to_string())),
                (TileKind::Disk(1), "WDC WD40EFRX", "Active 12.0%", Some("Disk 1 (D:, E:) \u{b7} Read 2.00 KB/sec  Write 0 B/sec".to_string())),
                (TileKind::Gpus, "GPU", "7.2%", Some("NVIDIA GeForce RTX 4080: 7.2%, 64.0 \u{b0}C, 285.3 W".to_string())),
                (TileKind::CpuPower, "CPU Power", "45.8 W", Some("CPU package".to_string())),
            ]
        );
        let unread = &view.tiles[2];
        assert!(!unread.lines.iter().any(|l| l.contains("Healthy") || l.contains("S.M.A.R.T.")), "SMART unknown: no health text at all: {:?}", unread.lines);
    }

    #[test]
    fn without_a_sensor_the_gpu_and_cpu_power_tiles_say_there_is_no_reading() {
        let view = view_of(sample(None), None);
        let gpu = view.tiles.iter().find(|t| t.kind == TileKind::Gpus).expect("a GPU tile");
        assert_eq!(gpu.lines, vec!["NVIDIA GeForce RTX 4080: 7.2%, temperature and power: no reading"]);
        let power = view.tiles.last().expect("tiles");
        assert_eq!(power.kind, TileKind::CpuPower, "CPU Power is always the last tile");
        assert_eq!(power.value, None);
        assert_eq!(power.value_text(), NO_READING, "a clear line, not 0.0 W");
        for disk in view.tiles.iter().filter(|t| matches!(t.kind, TileKind::Disk(_))) {
            assert!(!disk.lines.iter().any(|l| l.contains("S.M.A.R.T.")), "no sensor: no health text: {:?}", disk.lines);
        }
    }

    fn disk_tile(view: &SummaryView, n: u32) -> &Tile {
        view.tiles.iter().find(|t| t.kind == TileKind::Disk(n)).expect("a disk tile")
    }

    #[test]
    fn before_the_first_sample_the_disk_tiles_show_gaps_not_zero() {
        let view = view_of(Snapshot { seq: 0, ..Default::default() }, None);
        let disk = disk_tile(&view, 0);
        assert_eq!(disk.value_text(), "Active \u{2014}", "not Active 0.0%");
        assert_eq!(disk.lines[1], "Read \u{2014}  Write \u{2014}", "not 0 B/sec");
    }

    #[test]
    fn a_disk_figure_whose_counter_failed_is_a_gap_on_its_tile() {
        let mut snap = sample(None);
        snap.perf.unanswered = ["dread", "idle"].into_iter().collect();
        let view = view_of(snap, None);
        let disk = disk_tile(&view, 0);
        assert_eq!(disk.value_text(), "Active \u{2014}", "the idle-time counter failed");
        assert_eq!(disk.lines[1], "Read \u{2014}  Write 91.3 KB/sec", "only the read counter failed");
    }

    #[test]
    fn with_no_adapter_the_network_tile_says_so_with_a_gap() {
        let view = view_of(Snapshot { net: vec![], ..sample(None) }, None);
        let net = view.tiles.iter().find(|t| t.kind == TileKind::Network).expect("a network tile");
        assert_eq!(net.value_text(), "\u{2014}", "not S: 0 bps  R: 0 bps");
        assert_eq!(net.note().as_deref(), Some("No adapter"), "not 1 network interface");
    }

    #[test]
    fn the_sensors_package_power_fills_cpu_power_when_the_snapshot_has_none() {
        let at = Instant::now();
        let package = Reading::measure(crate::sensors::energy::PACKAGE, Device::cpu("CPU"), Group::Powers, "CPU Package", crate::sensors::model::Unit::Watt, "t", at).with(Some(41.26), "");
        let view = view_of(sample(None), Some(Frame::build(1, &[vec![package]], Vec::new())));
        assert_eq!(view.tiles.last().map(|t| t.value_text()), Some("41.3 W"));
    }
}
