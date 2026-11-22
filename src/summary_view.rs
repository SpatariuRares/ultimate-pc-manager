//! Summary's view: everything the Summary page draws, built once per new
//! sample (and when the pause or the units change) and kept by the Summary
//! page. The page only draws it, so no snapshot, history or process list is
//! copied or sorted on every frame.
//!
//! A figure the app could not read is a gap (`None`), drawn as [`GAP`],
//! never as 0.

use crate::app::{status_parts, ProviderState};
use crate::core_page::{PageCtx, PerfHistory};
use crate::page_summary::{summary_sub, top_by_cpu, SUMMARY_TOP};
use std::collections::VecDeque;

use crate::perfdata::{counter, PERF_INFO};
use crate::metrics::{combine_adapters, cpu_speed_mhz, fmt_ghz, physical_disk_bps, DISKS_TOTAL};
use crate::sampler::ProcRow;
use crate::widgets::{auto_max, fmt_bytes, local_ip, pct, HISTORY};

/// How a figure with no reading is drawn.
pub const GAP: &str = "\u{2014}";

/// The newest [`HISTORY`] points of the sum of the Performance series
/// `keys`, which grow together (one point per sample each).
fn summed_history(ph: &PerfHistory, keys: &[&str]) -> VecDeque<f32> {
    let series: Vec<&VecDeque<f32>> = keys.iter().filter_map(|k| ph.get(*k)).collect();
    let len = series.iter().map(|h| h.len()).min().unwrap_or(0);
    let take = len.min(HISTORY);
    (len - take..len).map(|i| series.iter().map(|h| h[h.len() - len + i]).sum()).collect()
}

/// The Disk card: physical disk bytes, not process I/O.
fn disk_card(ctx: &PageCtx, waiting: bool) -> Card {
    let perf = &ctx.snapshot.perf;
    let read = !waiting && perf.answered(counter::DREAD) && perf.answered(counter::DWRITE);
    let rw = physical_disk_bps(&perf.disks).filter(|_| read);
    let history = summed_history(ctx.histories.perf, &[DISKS_TOTAL]);
    Card {
        metric: Metric::Disk,
        value: rw.map(|(r, w)| format!("{}/s", fmt_bytes((r + w) as u64))),
        sub: rw.map(|(r, w)| format!("R {}/s  W {}/s", fmt_bytes(r as u64), fmt_bytes(w as u64))),
        max: auto_max(&history, 1_048_576.0),
        history: history.into(),
    }
}

/// The Network card: every adapter combined, with the adapter count.
fn network_card(ctx: &PageCtx, waiting: bool) -> Card {
    let all = combine_adapters(&ctx.snapshot.net);
    let history = summed_history(ctx.histories.perf, &["net.r", "net.s"]);
    let fmt_net = |bps: f64| fmt_net(ctx.settings.bits_per_second, bps);
    let (value, sub) = match (waiting, all.count) {
        (true, _) => (None, None),
        (false, 0) => (None, Some("No adapter".to_string())),
        (false, 1) => (Some(fmt_net(all.rx_bps + all.tx_bps)), Some("1 adapter".to_string())),
        (false, k) => (Some(fmt_net(all.rx_bps + all.tx_bps)), Some(format!("{k} adapters combined"))),
    };
    Card { metric: Metric::Network, value, sub, max: auto_max(&history, 1024.0), history: history.into() }
}

/// A network rate in bits per second or bytes per second, as set.
fn fmt_net(bits_per_second: bool, bytes_per_sec: f64) -> String {
    if bits_per_second {
        let b = bytes_per_sec * 8.0;
        if b >= 1e9 {
            format!("{:.1} Gbps", b / 1e9)
        } else if b >= 1e6 {
            format!("{:.1} Mbps", b / 1e6)
        } else {
            format!("{:.0} Kbps", b / 1e3)
        }
    } else {
        format!("{}/s", fmt_bytes(bytes_per_sec as u64))
    }
}

/// Which figure a Summary card shows (the page picks its colour).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Metric {
    #[default]
    Cpu,
    Memory,
    Disk,
    Network,
}

impl Metric {
    /// The card's title, as drawn above its value.
    pub fn title(self) -> &'static str {
        match self {
            Metric::Cpu => "CPU",
            Metric::Memory => "Memory",
            Metric::Disk => "Disk",
            Metric::Network => "Network",
        }
    }
}

/// One of Summary's cards: the big value, the line under it and the graph.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Card {
    /// Which figure the card shows.
    pub metric: Metric,
    /// The big value; `None` is a gap.
    pub value: Option<String>,
    /// The line under the title; `None` is a gap.
    pub sub: Option<String>,
    /// The graph's points, newest last; NaN is a sample with no reading.
    pub history: Vec<f32>,
    /// The top of the graph's scale.
    pub max: f32,
}

impl Card {
    /// The big value as drawn ("—" for a gap).
    pub fn value_text(&self) -> &str {
        self.value.as_deref().unwrap_or(GAP)
    }

    /// The line under the title as drawn ("—" for a gap).
    pub fn sub_text(&self) -> &str {
        self.sub.as_deref().unwrap_or(GAP)
    }
}

/// The band above the cards: the provider state and the one-line summary.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Band {
    /// The provider state in words (the status bar's first part).
    pub headline: String,
    /// The one-line summary under the headline.
    pub sub: String,
    /// Updates are paused.
    pub paused: bool,
    /// The provider state the headline and the dot show (the status bar's).
    pub providers: ProviderState,
}

/// What the view was built with besides the sample: when one of these
/// changes, the view is rebuilt even without a new sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Built {
    /// Updates are paused.
    pub paused: bool,
    /// Network rates are shown in bits per second.
    pub bits_per_second: bool,
    /// "Show Task Manager in Reports": the app is in the top list.
    pub show_self: bool,
}

impl Built {
    /// The settings the view depends on.
    pub fn of(settings: &crate::settings::Settings) -> Self {
        Built { paused: settings.interval_ms() == 0, bits_per_second: settings.bits_per_second, show_self: settings.show_self_in_reports }
    }
}

/// Everything the Summary page draws.
#[derive(Clone, Debug, Default)]
pub struct SummaryView {
    /// No sample has arrived yet: every card is a gap (and the providers
    /// are not yet known to be healthy). The page reads the same fact from
    /// `band.providers` (`ProviderState::Waiting`).
    #[cfg_attr(not(test), allow(dead_code))]
    pub waiting: bool,
    pub cpu: Card,
    pub memory: Card,
    pub disk: Card,
    pub network: Card,
    pub per_core: Vec<f32>,
    /// The kernel-time history drawn beside the CPU utilization, aligned
    /// with `cpu.history` (newest last).
    pub cpu_kernel: Vec<f32>,
    /// The line under the CPU card: "N logical processors · Speed X GHz".
    pub cpu_footer: String,
    /// The line under the Memory card: "Available X · Cached Y · Swap Z".
    pub memory_footer: String,
    pub band: Band,
    /// The busiest processes, in the order shown.
    pub top: Vec<ProcRow>,
    /// Summary's tiles, mapped from Performance's device list.
    pub tiles: Vec<crate::summary_tiles::Tile>,
    pub built: Built,
}

impl SummaryView {
    /// The cards in the order drawn.
    pub fn cards(&self) -> [&Card; 4] {
        [&self.cpu, &self.memory, &self.disk, &self.network]
    }

    /// The view of the sample, histories and settings in `ctx`, with the
    /// latest sensor `frame` (GPU temperature and power, S.M.A.R.T.).
    pub fn build(ctx: &PageCtx, frame: Option<&crate::sensors::Frame>) -> Self {
        let s = ctx.snapshot;
        let built = Built::of(ctx.settings);
        let waiting = s.seq == 0;
        // Rounded to the 10 MHz the pages show, so that the card and the
        // band print the same digits. The machine's base clock is read only
        // when there is a performance level to apply it to.
        let speed_mhz = (!waiting && s.perf.answered(counter::PERF))
            .then(|| cpu_speed_mhz(base_mhz(s), Some(s.perf.cpu_perf_pct as f64)))
            .flatten()
            .map(|mhz| (mhz / 10.0).round() as u64 * 10);
        let memory = pct(s.mem_used, s.mem_total);
        let providers = ProviderState::of(s);
        let [headline, ..] = status_parts(built.paused, providers, s.procs.len(), &local_ip(), s.seq);
        let band_sub = if waiting {
            "Waiting for the first sample".to_string()
        } else {
            // No speed (0) leaves it out of the line.
            summary_sub(s.cpu_total, speed_mhz.unwrap_or(0), memory, s.procs.len())
        };
        // Before the first sample, no figure is a reading.
        let live = |text: String| (!waiting).then_some(text);
        SummaryView {
            waiting,
            cpu: Card {
                metric: Metric::Cpu,
                value: live(format!("{:.0}%", s.cpu_total)),
                // The CPU speed, the same figure as in the band.
                sub: speed_mhz.map(|mhz| fmt_ghz(mhz as f32)),
                history: ctx.histories.cpu.iter().copied().collect(),
                max: 100.0,
            },
            memory: Card {
                metric: Metric::Memory,
                value: live(format!("{memory:.0}%")),
                sub: live(format!("{} / {}", fmt_bytes(s.mem_used), fmt_bytes(s.mem_total))),
                history: ctx.histories.mem.iter().copied().collect(),
                max: 100.0,
            },
            disk: disk_card(ctx, waiting),
            network: network_card(ctx, waiting),
            per_core: s.cpu_per_core.clone(),
            cpu_kernel: kernel_history(ctx.histories.perf),
            cpu_footer: cpu_footer(s.cpu_per_core.len(), speed_mhz),
            memory_footer: memory_footer(s),
            band: Band {
                headline,
                sub: band_sub,
                paused: built.paused,
                providers,
            },
            top: top_without_self(ctx),
            tiles: crate::summary_tiles::summary_tiles(ctx, frame),
            built,
        }
    }
}

/// Summary's top CPU list: the busiest processes without the app itself
/// unless "Show Task Manager in Reports" is on. Only the own-process part
/// of Settings' hiding applies here, not Hide Microsoft.
fn top_without_self(ctx: &PageCtx) -> Vec<ProcRow> {
    let own = crate::process_view::Hide::new(ctx.settings, std::process::id(), Default::default()).own_pid;
    // The app is at most one row, so one spare row keeps the list full.
    let mut top = top_by_cpu(&ctx.snapshot.procs, SUMMARY_TOP + 1);
    top.retain(|p| Some(p.pid) != own);
    top.truncate(SUMMARY_TOP);
    top
}

/// The kernel-time history Performance keeps (`cpu.k`), cut to the last
/// minute the Summary graphs show so it lines up with the utilization.
fn kernel_history(ph: &PerfHistory) -> Vec<f32> {
    let Some(h) = ph.get("cpu.k") else { return Vec::new() };
    h.iter().skip(h.len().saturating_sub(HISTORY)).copied().collect()
}

/// The CPU card's footer: the logical processor count and the shared CPU
/// speed, a gap for either figure that is missing.
fn cpu_footer(processors: usize, speed_mhz: Option<u64>) -> String {
    let count = if processors > 0 { processors.to_string() } else { GAP.to_string() };
    let speed = speed_mhz.map_or_else(|| GAP.to_string(), |mhz| fmt_ghz(mhz as f32));
    format!("{count} logical processors \u{b7} Speed {speed}")
}

/// The Memory card's footer: available memory, the system cache and the
/// page file in use, a gap for each figure the sample could not read (all
/// of them before the first sample).
fn memory_footer(s: &crate::sampler::Snapshot) -> String {
    let read = |ok: bool, bytes: u64| if s.seq > 0 && ok { fmt_bytes(bytes) } else { GAP.to_string() };
    let available = read(true, s.mem_avail);
    let cached = read(s.perf.answered(PERF_INFO), s.perf.cached);
    // Swap is sysinfo's page file in use, with the page-file counter as a
    // fallback: a gap only when neither read it.
    let swap = read(s.swap_total > 0 || s.perf.answered(counter::PAGEFILE), s.swap_used);
    format!("Available {available} \u{b7} Cached {cached} \u{b7} Swap {swap}")
}

/// The CPU's base frequency in MHz, from the machine's fixed facts the
/// sample carries (`None` when unknown).
fn base_mhz(s: &crate::sampler::Snapshot) -> Option<f64> {
    crate::metrics::base_clock_mhz(Some(crate::page_perf::machine_of(s, None).cpu.base_mhz as f64), None, None)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::TaskApp;
    use crate::perfdata::PerfLive;
    use crate::sampler::Snapshot;
    use crate::settings::{Settings, SPEEDS};

    const GB: u64 = 1 << 30;

    /// A sample of a PC whose CPU runs at 110% of a 3.40 GHz base clock
    /// (sysinfo still reports a stale 2.90 GHz).
    fn sample() -> Snapshot {
        Snapshot {
            seq: 1,
            cpu_total: 15.4,
            cpu_per_core: vec![20.0, 10.0],
            cpu_freq_mhz: 2_900,
            mem_total: 64 * GB,
            mem_used: 25 * GB,
            perf: PerfLive { cpu_perf_pct: 110.0, ..Default::default() },
            procs: vec![ProcRow { pid: 16244, name: "Code.exe".into(), cpu: 8.0, mem: 4_300_000_000, ..Default::default() }],
            ..Default::default()
        }
    }

    /// The machine's fixed facts with a base clock of `base_mhz`.
    pub(crate) fn clocked(base_mhz: u32) -> crate::perfdata::StaticInfo {
        crate::perfdata::StaticInfo { cpu: crate::perfdata::CpuInfo { base_mhz, ..Default::default() }, ..Default::default() }
    }

    /// `snap` as read on a machine with a 3.40 GHz base clock.
    fn on_this_machine(mut snap: Snapshot) -> Snapshot {
        snap.machine = Some(std::sync::Arc::new(clocked(3_400)));
        snap
    }

    /// The Summary view of a headless app on `snap`, with a base clock of
    /// `base_mhz`.
    fn view_of(snap: Snapshot, base_mhz: u32) -> SummaryView {
        view_with(&Settings::default(), snap, base_mhz)
    }

    /// The same with the settings given.
    fn view_with(settings: &Settings, mut snap: Snapshot, base_mhz: u32) -> SummaryView {
        snap.machine = Some(std::sync::Arc::new(clocked(base_mhz)));
        let lent = crate::core_page::testing::Lent::default();
        SummaryView::build(&lent.ctx(settings, &snap), None)
    }

    #[test]
    fn cpu_speed_is_the_base_clock_times_the_live_performance_level() {
        let view = view_of(sample(), 3_400);
        assert_eq!(view.cpu.sub_text(), "3.74 GHz", "3400 MHz x 110%, not sysinfo's 2.90 GHz");
        assert!(view.band.sub.contains("at 3.74 GHz"), "the band reads the same speed: {}", view.band.sub);
    }

    #[test]
    fn without_a_live_clock_or_a_base_clock_the_speed_is_a_gap_on_card_and_band() {
        let no_clock = Snapshot { perf: PerfLive { cpu_perf_pct: 0.0, ..Default::default() }, ..sample() };
        for (view, why) in [(view_of(no_clock, 3_400), "no live performance level"), (view_of(sample(), 0), "no base clock")] {
            assert_eq!(view.cpu.sub_text(), GAP, "{why}: the card shows a gap, not 0.00 GHz");
            assert!(!view.band.sub.contains("GHz"), "{why}: the band leaves the speed out: {}", view.band.sub);
        }
    }

    #[test]
    fn before_the_first_sample_every_card_is_a_gap_and_the_page_waits() {
        let view = view_of(Snapshot::default(), 3_400);
        assert!(view.waiting);
        for card in view.cards() {
            assert_eq!(card.value_text(), GAP, "{} shows no 0% / 0 B/s", card.metric.title());
            assert_eq!(card.sub_text(), GAP, "{} has no sub-line yet", card.metric.title());
        }
        assert_eq!(view.band.sub, "Waiting for the first sample");
    }

    #[test]
    fn paused_updates_keep_the_last_view() {
        let (mut app, feed) = TaskApp::fed(Settings::default(), Snapshot::default());
        feed.send(on_this_machine(sample())).expect("sent");
        app.pull_samples();
        let before = app.summary_view().clone();
        assert_eq!(before.cpu.value_text(), "15%", "the first sample reached the view");
        app.settings.speed = SPEEDS.iter().position(|s| s.1 == 0).expect("a Paused speed");
        feed.send(Snapshot { seq: 2, cpu_total: 99.0, ..on_this_machine(sample()) }).expect("sent");
        app.pull_samples();
        let paused = app.summary_view().clone();
        assert_eq!(paused.cards(), before.cards(), "paused, the figures stay those of the last sample");
        assert_eq!(paused.band.sub, before.band.sub);
        assert_eq!(paused.band.headline, "Updates paused", "the page says it is paused");
    }

    const MIB: f64 = 1_048_576.0;

    /// A sample whose processes did 500 MB/s of I/O (mostly network and
    /// device traffic) while the two physical disks read 4 MB/s and wrote
    /// 6 MB/s, and whose Ethernet (most bytes since boot) and Wi-Fi carry
    /// 2 MB/s together; a third adapter never carried a byte.
    fn traffic() -> Snapshot {
        use crate::perfdata::DiskLive;
        use crate::sampler::NetRow;
        Snapshot {
            disk_read_bps: 300.0 * MIB,
            disk_write_bps: 200.0 * MIB,
            perf: PerfLive {
                cpu_perf_pct: 110.0,
                disks: vec![
                    DiskLive { number: 0, read_bps: MIB, write_bps: 2.0 * MIB, ..Default::default() },
                    DiskLive { number: 1, read_bps: 3.0 * MIB, write_bps: 4.0 * MIB, ..Default::default() },
                ],
                ..Default::default()
            },
            net: vec![
                NetRow { name: "Ethernet".into(), rx_bps: 1_000_000.0, tx_bps: 250_000.0, total_rx: 90 * GB, total_tx: 10 * GB },
                NetRow { name: "Wi-Fi".into(), rx_bps: 500_000.0, tx_bps: 250_000.0, total_rx: GB, total_tx: GB },
                NetRow { name: "Bluetooth".into(), ..Default::default() },
            ],
            ..sample()
        }
    }

    #[test]
    fn the_disk_card_sums_every_physical_disk_not_the_process_io() {
        let view = view_of(traffic(), 3_400);
        assert_eq!(view.disk.value_text(), "10.0 MB/s", "1+2+3+4 MB/s of disk bytes, not 500 MB/s of process I/O");
        assert_eq!(view.disk.sub_text(), "R 4.0 MB/s  W 6.0 MB/s");
    }

    #[test]
    fn the_network_card_combines_every_adapter_and_counts_them() {
        let view = view_of(traffic(), 3_400);
        assert_eq!(view.network.value_text(), "16.0 Mbps", "2 MB/s over both adapters, not Ethernet's 10.0 Mbps alone");
        assert_eq!(view.network.sub_text(), "2 adapters combined", "the adapter that never carried a byte is not counted");
    }

    #[test]
    fn with_no_adapter_the_network_card_is_a_gap() {
        let view = view_of(Snapshot { net: vec![], ..traffic() }, 3_400);
        assert_eq!(view.network.value_text(), GAP, "not 0 B/s");
        assert_eq!(view.network.sub_text(), "No adapter");
    }

    #[test]
    fn the_disk_and_network_graphs_follow_the_new_figures() {
        let (mut app, feed) = TaskApp::fed(Settings::default(), Snapshot::default());
        let mut second = traffic();
        second.seq = 2;
        second.perf.disks.truncate(1);
        second.net.truncate(1);
        for snap in [traffic(), second] {
            feed.send(snap).expect("sent");
            app.pull_samples();
        }
        let view = app.summary_view().clone();
        assert_eq!(view.disk.history, vec![(10.0 * MIB) as f32, (3.0 * MIB) as f32], "disk bytes per second");
        assert_eq!(view.network.history, vec![2_000_000.0, 1_250_000.0], "all adapters' bytes per second");
    }

    #[test]
    fn the_disk_card_is_a_gap_when_a_disk_counter_failed() {
        for failed in ["dread", "dwrite"] {
            let mut snap = traffic();
            snap.perf.unanswered = [failed].into_iter().collect();
            let view = view_of(snap, 3_400);
            assert_eq!(view.disk.value_text(), GAP, "{failed} failed: no stale or partial disk figure");
            assert_eq!(view.disk.sub_text(), GAP, "{failed} failed");
        }
    }

    /// A sample with 40 busy processes (pid 101..=140, CPU 1..=40) plus the
    /// idle process and the app itself, the busiest of all.
    fn crowded() -> Snapshot {
        let mut procs: Vec<ProcRow> = (1..=40)
            .map(|i| ProcRow { pid: 100 + i, name: format!("p{i}.exe"), cpu: i as f32, mem: 1_000, ..Default::default() })
            .collect();
        procs.push(ProcRow { pid: 0, name: "System Idle Process".into(), cpu: 95.0, ..Default::default() });
        procs.push(ProcRow { pid: std::process::id(), name: "tests.exe".into(), cpu: 99.0, ..Default::default() });
        Snapshot { procs, ..sample() }
    }

    fn top_pids(view: &SummaryView) -> Vec<u32> {
        view.top.iter().map(|p| p.pid).collect()
    }

    #[test]
    fn the_top_cpu_list_shows_25_rows() {
        let view = view_of(crowded(), 3_400);
        assert_eq!(view.top.len(), 25, "25 rows when at least 25 processes exist");
    }

    #[test]
    fn the_app_itself_is_left_out_of_the_top_list() {
        let view = view_of(crowded(), 3_400);
        assert!(!top_pids(&view).contains(&std::process::id()), "Show Task Manager in Reports is off");
        assert_eq!(view.top.len(), 25, "the next busiest process takes its place");
    }

    #[test]
    fn the_app_itself_is_listed_when_shown_in_reports() {
        // Hide Microsoft Services and Processes does not reach Summary.
        let mut settings = Settings::default();
        settings.show_self_in_reports = true;
        settings.hide_microsoft = true;
        let view = view_with(&settings, crowded(), 3_400);
        assert_eq!(view.top.first().map(|p| p.pid), Some(std::process::id()), "the busiest, so first");
        assert_eq!(view.top.len(), 25);
    }

    #[test]
    fn the_top_list_goes_by_cpu_then_memory_without_the_idle_process() {
        let mut snap = crowded();
        // Ties with p40 (CPU 40): more memory first.
        snap.procs.push(ProcRow { pid: 200, name: "heavy.exe".into(), cpu: 40.0, mem: 9_000, ..Default::default() });
        let pids = top_pids(&view_of(snap, 3_400));
        assert!(!pids.contains(&0), "the idle process is left out");
        assert_eq!(pids[..4], [200, 140, 139, 138], "highest CPU first, more memory first on a tie");
    }

    /// `sample()` with these performance counters failed.
    fn with_failed(counters: &[&'static str]) -> Snapshot {
        let s = sample();
        Snapshot { perf: PerfLive { unanswered: counters.iter().copied().collect(), ..s.perf.clone() }, ..s }
    }

    #[test]
    fn every_counter_answered_is_healthy() {
        let view = view_of(sample(), 3_400);
        assert_eq!(view.band.providers, ProviderState::Healthy);
        assert_eq!(view.band.headline, "Native providers healthy");
    }

    #[test]
    fn counters_a_healthy_pc_may_never_answer_keep_it_healthy() {
        let view = view_of(with_failed(&["perf", "compr", "pagefile"]), 3_400);
        assert_eq!(view.band.providers, ProviderState::Healthy);
        assert_eq!(view.cpu.sub_text(), GAP, "their own figures are still gaps");
    }

    #[test]
    fn some_failed_counters_are_degraded_and_their_figures_are_gaps() {
        // The speed counter failed: its stale 110% must not be applied.
        let view = view_of(with_failed(&["perf", "dread"]), 3_400);
        assert_eq!(view.band.providers, ProviderState::Degraded);
        assert_eq!(view.band.headline, "Native providers degraded");
        assert_eq!(view.cpu.sub_text(), GAP, "the speed whose counter failed is a gap, not a figure");
        assert!(!view.band.sub.contains("GHz"), "the band leaves the failed speed out: {}", view.band.sub);
        assert_eq!(view.cpu.value_text(), "15%", "figures from other sources are still shown");
    }

    #[test]
    fn no_counter_answered_is_unavailable() {
        let every: Vec<&'static str> = crate::perfdata::COUNTERS.iter().chain(crate::perfdata::OPTIONAL).copied().chain([PERF_INFO]).collect();
        let view = view_of(with_failed(&every), 3_400);
        assert_eq!(view.band.providers, ProviderState::Unavailable);
        assert_eq!(view.band.headline, "Native providers unavailable");
        assert_eq!(view.cpu.sub_text(), GAP);
    }

    #[test]
    fn the_status_bar_says_what_the_band_says() {
        let app = TaskApp::headless(Settings::default(), with_failed(&["dread"]));
        let [status, ..] = status_parts(false, ProviderState::of(&app.snap), 1, "", 1);
        assert_eq!(status, "Native providers degraded");
        assert_eq!(view_of(with_failed(&["dread"]), 3_400).band.headline, status);
    }

    #[test]
    fn the_cpu_graph_carries_the_kernel_series_beside_utilization() {
        let (mut app, feed) = TaskApp::fed(Settings::default(), Snapshot::default());
        app.machine_info = Some(clocked(3_400));
        let with_kernel = |seq: u64, cpu_total: f32, kernel_total: f32| Snapshot {
            seq,
            cpu_total,
            perf: PerfLive { cpu_perf_pct: 110.0, kernel_total, ..Default::default() },
            ..sample()
        };
        feed.send(with_kernel(1, 15.4, 3.0)).expect("sent");
        app.pull_samples();
        feed.send(with_kernel(2, 20.0, 6.5)).expect("sent");
        app.pull_samples();
        // A kernel reading above the utilization is capped to it, as on Performance.
        feed.send(with_kernel(3, 4.0, 9.0)).expect("sent");
        app.pull_samples();
        let view = app.summary_view().clone();
        assert_eq!(view.cpu.history, vec![15.4, 20.0, 4.0]);
        assert_eq!(view.cpu_kernel, vec![3.0, 6.5, 4.0], "one kernel point per utilization point");
    }

    #[test]
    fn the_cpu_footer_names_the_logical_processors_and_the_shared_speed() {
        let view = view_of(sample(), 3_400);
        assert_eq!(view.cpu_footer, "2 logical processors \u{b7} Speed 3.74 GHz");
    }

    #[test]
    fn without_a_speed_the_cpu_footer_shows_a_gap() {
        let view = view_of(sample(), 0);
        assert_eq!(view.cpu_footer, "2 logical processors \u{b7} Speed \u{2014}");
    }

    /// `sample()` with 39 GB available, 12 GB cached and 2 GB of page file used.
    fn with_memory() -> Snapshot {
        let s = sample();
        Snapshot {
            mem_avail: 39 * GB,
            swap_total: 16 * GB,
            swap_used: 2 * GB,
            perf: PerfLive { cached: 12 * GB, ..s.perf.clone() },
            ..s
        }
    }

    #[test]
    fn the_memory_card_shows_available_cached_and_swap() {
        let view = view_of(with_memory(), 3_400);
        assert_eq!(view.memory_footer, "Available 39.0 GB \u{b7} Cached 12.0 GB \u{b7} Swap 2.0 GB");
    }

    #[test]
    fn swap_is_a_gap_when_no_source_read_the_page_file() {
        // sysinfo saw no page file and the page-file counter failed.
        let failed = Snapshot { swap_total: 0, swap_used: 0, perf: PerfLive { unanswered: ["pagefile"].into_iter().collect(), ..with_memory().perf }, ..with_memory() };
        let view = view_of(failed, 3_400);
        assert_eq!(view.memory_footer, "Available 39.0 GB \u{b7} Cached 12.0 GB \u{b7} Swap \u{2014}", "an unreadable page file is not 0 B");
    }

    #[test]
    fn swap_read_by_sysinfo_is_shown_when_only_the_page_file_counter_failed() {
        let failed = Snapshot { perf: PerfLive { unanswered: ["pagefile"].into_iter().collect(), ..with_memory().perf }, ..with_memory() };
        let view = view_of(failed, 3_400);
        assert_eq!(view.memory_footer, "Available 39.0 GB \u{b7} Cached 12.0 GB \u{b7} Swap 2.0 GB", "sysinfo read 2 GB of page file in use");
    }

    #[test]
    fn a_failed_performance_info_call_is_degraded_and_cached_is_a_gap() {
        let failed = Snapshot { perf: PerfLive { unanswered: [crate::perfdata::PERF_INFO].into_iter().collect(), ..with_memory().perf }, ..with_memory() };
        let view = view_of(failed, 3_400);
        assert_eq!(view.band.providers, ProviderState::Degraded, "healthy only when every source answered");
        assert_eq!(view.band.headline, "Native providers degraded");
        assert_eq!(view.memory_footer, "Available 39.0 GB \u{b7} Cached \u{2014} \u{b7} Swap 2.0 GB");
    }

    #[test]
    fn a_failed_disk_read_is_a_gap_in_the_disk_graph_not_zero() {
        let (mut app, feed) = TaskApp::fed(Settings::default(), Snapshot::default());
        let mut failed = traffic();
        failed.seq = 2;
        failed.perf.unanswered = ["dwrite"].into_iter().collect();
        for snap in [traffic(), failed] {
            feed.send(snap).expect("sent");
            app.pull_samples();
        }
        let history = app.summary_view().disk.history.clone();
        assert_eq!(history.len(), 2, "one point per sample");
        assert_eq!(history[0], (10.0 * MIB) as f32);
        assert!(history[1].is_nan(), "the failed sample is a gap, not 0 or a partial sum: {history:?}");
    }

    #[test]
    fn a_failed_kernel_counter_is_a_gap_in_the_kernel_series() {
        let (mut app, feed) = TaskApp::fed(Settings::default(), Snapshot::default());
        let read = Snapshot { perf: PerfLive { cpu_perf_pct: 110.0, kernel_total: 3.0, ..Default::default() }, ..sample() };
        let mut failed = Snapshot { seq: 2, ..read.clone() };
        failed.perf.unanswered = ["kernel"].into_iter().collect();
        failed.perf.kernel_total = 0.0;
        for snap in [read, failed] {
            feed.send(snap).expect("sent");
            app.pull_samples();
        }
        let kernel = app.summary_view().cpu_kernel.clone();
        assert_eq!(kernel.len(), 2, "aligned with the utilization");
        assert_eq!(kernel[0], 3.0);
        assert!(kernel[1].is_nan(), "the failed sample is a gap, not 0: {kernel:?}");
    }

    #[test]
    fn the_top_list_follows_show_task_manager_in_reports_at_once() {
        let mut app = TaskApp::headless(Settings::default(), crowded());
        app.rebuild_summary();
        assert!(!top_pids(app.summary_view()).contains(&std::process::id()));
        app.settings.show_self_in_reports = true;
        assert_eq!(top_pids(app.summary_view()).first(), Some(&std::process::id()), "rebuilt without waiting for a new sample");
    }
}
