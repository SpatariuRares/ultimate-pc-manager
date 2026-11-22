//! The Sampler and the app over a sensor service of fake providers: the
//! counters of the core pages come from the shared service, per provider
//! group.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::{ProviderState, TaskApp};
use crate::app::PerfItem;
use crate::perfdata::{counter, DiskLive, PerfLive};
use crate::sampler::Snapshot;
use crate::sensors::fake::{self, Script};
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::service::ProviderGroup;
use crate::sensors::SensorService;
use crate::settings::Settings;

const NORMAL: usize = 2;
const SLOW: usize = 3;
const PAUSED: usize = 4;

fn settings(speed: usize) -> Settings {
    let mut settings = Settings::default();
    settings.speed = speed;
    settings
}

/// What a PC counts: the performance level, one disk, the package power.
fn counted(perf: f64) -> Vec<CounterSet> {
    vec![
        CounterSet::new(counter::PERF, vec![Instance::new("_Total", perf)]),
        CounterSet::new(counter::IDLE, vec![Instance::new("0 C:", 50.0), Instance::new("_Total", 50.0)]),
        CounterSet::new(counter::DREAD, vec![Instance::new("0 C:", 3_000_000.0).with_raw(9_000_000), Instance::new("_Total", 3_000_000.0)]),
        CounterSet::new(counter::DWRITE, vec![Instance::new("0 C:", 1_000_000.0).with_raw(4_000_000), Instance::new("_Total", 1_000_000.0)]),
        CounterSet::new(counter::POWER, vec![Instance::new("rapl_package0_pkg", 47_500.0)]),
    ]
    .into_iter()
    .chain(more_counted())
    .collect()
}

/// An app whose counters are what `script` says, with the collector's gaps
/// (`collector_gaps`) in every snapshot.
fn app_over(script: &Arc<Script>, speed: usize, collector_gaps: &'static [&'static str]) -> (TaskApp, SensorService) {
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", script))]);
    let app = TaskApp::headless_over(settings(speed), service.clone(), move |snap: &mut Snapshot| {
        sysinfo_part(snap);
        snap.perf.unanswered = collector_gaps.iter().copied().collect();
    });
    (app, service)
}

/// Pulls samples until `cond` holds.
fn pull_until(app: &mut TaskApp, what: &str, cond: impl Fn(&TaskApp) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        app.pull_samples();
        if cond(app) {
            return;
        }
        assert!(Instant::now() < end, "timed out waiting: {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The same figures as a hand-built snapshot.
fn by_hand(perf: f32) -> TaskApp {
    by_hand_all(perf)
}

#[test]
fn a_headless_app_over_fake_providers_shows_their_figures_on_the_core_pages() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, NORMAL, &[]);
    app.machine_info = Some(crate::summary_view::tests::clocked(3_400));
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let mut hand = by_hand(110.0);
    hand.machine_info = Some(crate::summary_view::tests::clocked(3_400));
    let top = |a: &TaskApp, item| a.stats(item).top.clone();
    assert!(top(&app, PerfItem::Cpu).iter().any(|(k, v)| k == "Speed" && v == "3.74 GHz"), "base clock x the fake performance level");
    assert_eq!(top(&app, PerfItem::Cpu), top(&hand, PerfItem::Cpu), "the CPU page");
    assert!(!top(&app, PerfItem::Disk(0)).is_empty());
    assert_eq!(top(&app, PerfItem::Disk(0)), top(&hand, PerfItem::Disk(0)), "the disk page: activity, rates and totals");
    assert_eq!(app.snap.perf.cpu_power_w, Some(47.5));
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy);
}

#[test]
fn a_required_counter_with_no_value_is_a_gap_and_degrades_the_state_an_optional_one_does_not() {
    // Every other source answered (the collector reports no gap).
    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != counter::DWRITE).collect());
    let (mut app, _service) = app_over(&script, NORMAL, &[]);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert!(!app.snap.perf.answered(counter::DWRITE), "no write rate: a gap");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded);

    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != counter::PERF).collect());
    let (mut app, _service) = app_over(&script, NORMAL, &[]);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert!(!app.snap.perf.answered(counter::PERF), "no performance level: a gap");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "an optional counter never degrades");
}

#[test]
fn when_no_counter_answers_the_state_is_unavailable() {
    let script = Script::new(Vec::new());
    // The collector's own sources failed too.
    let (mut app, _service) = app_over(&script, NORMAL, &[counter::KERNEL, counter::PAGEIN, counter::DRESP, crate::perfdata::PERF_INFO]);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Unavailable);
    assert_eq!(ProviderState::of(&Snapshot::default()), ProviderState::Waiting, "nothing sampled yet");
}

#[test]
fn paused_a_new_reading_does_not_change_the_pages_and_refresh_reads_once_after_the_request() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, PAUSED, &[]);
    pull_until(&mut app, "the sample taken at start", |a| a.snap.seq >= 1);
    assert_eq!(app.snap.perf.cpu_perf_pct, 110.0);
    let reads = script.reads();
    // The PC changes while updates are paused.
    script.set(counted(55.0));
    script.slow(Duration::from_millis(150));
    std::thread::sleep(Duration::from_millis(300));
    app.pull_samples();
    assert_eq!(app.snap.perf.cpu_perf_pct, 110.0, "paused: the pages keep their figures");
    assert_eq!(script.reads(), reads, "paused: nothing is read");
    app.refresh_now();
    pull_until(&mut app, "the Refresh sample", |a| a.snap.seq >= 2);
    assert_eq!(app.snap.perf.cpu_perf_pct, 55.0, "built from a frame read after the Refresh, not the old one");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(script.reads(), reads + 1, "Refresh reads once");
}

#[test]
fn the_first_figures_do_not_wait_for_the_hardware_sensors() {
    let counters = Script::new(counted(110.0));
    let hardware = Script::new(Vec::new());
    hardware.slow(Duration::from_secs(30));
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", &counters)), (ProviderGroup::Hardware, fake::provider("Vendor GPU", &hardware))]);
    // A page is looking: the hardware provider is reading (for a long time).
    let _page = service.interest();
    let mut app = TaskApp::headless_over(settings(NORMAL), service.clone(), |_: &mut Snapshot| {});
    crate::addons::until("the hardware provider is reading", || hardware.reads() == 1);
    let start = Instant::now();
    while app.snap.seq == 0 {
        app.pull_samples();
        assert!(start.elapsed() < Duration::from_secs(1), "the first figures wait for the hardware");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(app.snap.perf.cpu_perf_pct, 110.0);
    assert_eq!(hardware.reads(), 1, "and the hardware provider is still in its first read");
}

#[test]
fn the_core_pages_follow_the_settings_speed_whatever_a_page_asks_of_the_sensors() {
    let script = Script::new(counted(110.0));
    let (mut app, service) = app_over(&script, SLOW, &[]);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    // A page that wants the sensors every 20 ms.
    let page = service.interest();
    page.set_interval_ms(20);
    std::thread::sleep(Duration::from_millis(700));
    app.pull_samples();
    assert_eq!(app.snap.seq, 1, "Slow: one sample every two seconds, not every 20 ms");
    drop(page);
}

#[test]
fn the_settings_speed_is_the_counters_speed_and_not_the_hardwares() {
    let counters = Script::new(counted(110.0));
    let hardware = Script::new(Vec::new());
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", &counters)), (ProviderGroup::Hardware, fake::provider("Vendor GPU", &hardware))]);
    let looking = service.interest_in(ProviderGroup::Hardware);
    looking.set_interval_ms(10);
    let mut app = TaskApp::headless_over(settings(SLOW), service.clone(), |_: &mut Snapshot| {});
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    std::thread::sleep(Duration::from_millis(700));
    assert!(counters.reads() <= 2, "the counters read at the settings speed: {}", counters.reads());
    assert!(hardware.reads() > 10, "the hardware at the speed of the page that looks: {}", hardware.reads());
}

#[test]
fn the_hardware_stops_with_the_last_page_while_the_sampler_keeps_the_counters() {
    let counters = Script::new(counted(110.0));
    let hardware = Script::new(Vec::new());
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", &counters)), (ProviderGroup::Hardware, fake::provider("Vendor GPU", &hardware))]);
    let mut app = TaskApp::headless_over(settings(NORMAL), service.clone(), |_: &mut Snapshot| {});
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(service.threads_in(ProviderGroup::Hardware), 0, "nobody looks at the hardware: it is not read");
    assert_eq!(hardware.reads(), 0);
    app.read_summary_sensors();
    crate::addons::until("the hardware runs for Summary", || service.threads_in(ProviderGroup::Hardware) > 0);
    app.addons.end_frame(); // Summary was drawn in this frame
    app.addons.end_frame(); // another page: Summary did not ask
    crate::addons::until("the hardware stops", || service.threads_in(ProviderGroup::Hardware) == 0);
    assert_eq!(service.threads_in(ProviderGroup::Counters), 1, "the sampler still reads the counters");
}

/// What the providers of the rest of the counters count: kernel time per
/// core, pages read, a disk's response time, the compressed memory, the page
/// file and what GetPerformanceInfo returns.
fn more_counted() -> Vec<CounterSet> {
    let named = |name: &str, value: f64| Instance::new(name, value);
    vec![
        CounterSet::new(counter::KERNEL, vec![named("0", 8.0), named("1", 4.0), named("_Total", 6.0)]),
        CounterSet::new(counter::PAGEIN, vec![Instance::new("", 120.0).with_raw(9_000)]),
        CounterSet::new(counter::DRESP, vec![named("0 C:", 0.0035), named("_Total", 0.0035)]),
        CounterSet::new(counter::COMPR, vec![named("memory compression", 512.0 * 1024.0 * 1024.0)]),
        CounterSet::new(counter::PAGEFILE, vec![named("_total", 40.0)]),
        CounterSet::new(crate::perfdata::PERF_INFO, vec![named("threads", 2_000.0), named("handles", 90_000.0), named("commit", 8e9), named("commit_limit", 3e10), named("cached", 4e9)]),
    ]
}

fn all_counted() -> Vec<CounterSet> {
    counted(110.0)
}

/// A hand-built snapshot with the same figures `all_counted` counts.
fn by_hand_all(perf_level: f32) -> TaskApp {
    let disk = DiskLive { number: 0, active: 50.0, read_bps: 3_000_000.0, write_bps: 1_000_000.0, total_read: 9_000_000, total_written: 4_000_000, resp_ms: 3.5, ..Default::default() };
    let perf = PerfLive {
        cpu_perf_pct: perf_level,
        disks: vec![disk],
        cpu_power_w: Some(47.5),
        kernel_total: 6.0,
        kernel_per_core: vec![8.0, 4.0],
        hard_faults: 120.0,
        page_ins: 9_000,
        compressed: 512 * 1024 * 1024,
        pagefile_pct: 40.0,
        threads: 2_000,
        handles: 90_000,
        commit: 8_000_000_000,
        commit_limit: 30_000_000_000,
        cached: 4_000_000_000,
        ..Default::default()
    };
    TaskApp::headless(settings(NORMAL), Snapshot { seq: 1, perf, compressed: 512 * 1024 * 1024, swap_total: 10_000_000_000, swap_used: 2_000_000_000, mem_total: 16_000_000_000, cpu_total: 20.0, cpu_per_core: vec![30.0, 10.0], ..Default::default() })
}

/// What the PC's sysinfo part reads: 20 % busy, a 10 GB page file.
fn sysinfo_part(snap: &mut Snapshot) {
    snap.cpu_total = 20.0;
    snap.cpu_per_core = vec![30.0, 10.0];
    snap.mem_total = 16_000_000_000;
    snap.swap_total = 10_000_000_000;
}

fn app_filled(script: &Arc<Script>, fill: impl FnMut(&mut Snapshot) + Send + 'static) -> TaskApp {
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", script))]);
    TaskApp::headless_over(settings(NORMAL), service, fill)
}

fn top(app: &TaskApp, item: PerfItem, key: &str) -> String {
    app.stats(item).top.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn the_memory_and_disk_figures_the_provider_counted_show_as_the_collector_showed_them() {
    let script = Script::new(all_counted());
    let mut app = app_filled(&script, |snap: &mut Snapshot| {
        sysinfo_part(snap);
        snap.swap_used = 2_000_000_000;
    });
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let mut hand = by_hand_all(110.0);
    for key in ["Committed", "Cached", "Compressed", "Page ins", "Hard faults", "Swap used"] {
        assert_eq!(top(&app, PerfItem::Memory, key), top(&hand, PerfItem::Memory, key), "Memory > {key}");
    }
    assert_eq!(top(&app, PerfItem::Memory, "Page ins"), "9000");
    assert_eq!(top(&app, PerfItem::Disk(0), "Avg. response time"), "3.50 ms", "the disk response time still shows");
    assert_eq!(top(&app, PerfItem::Cpu, "Threads"), "2000");
    assert_eq!(app.summary_view().memory_footer, hand.summary_view().memory_footer);
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy);
}

#[test]
fn kernel_time_from_the_provider_fills_the_kernel_series_and_a_failed_counter_is_a_gap() {
    let script = Script::new(all_counted());
    let mut app = app_filled(&script, sysinfo_part);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(app.summary_view().cpu_kernel.last().copied(), Some(6.0), "kernel time of the whole CPU");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy);

    script.set(all_counted().into_iter().filter(|c| c.key != counter::KERNEL).collect());
    let seq = app.snap.seq;
    pull_until(&mut app, "a sample without the kernel counter", |a| a.snap.seq > seq && !a.snap.perf.answered(counter::KERNEL));
    assert!(app.summary_view().cpu_kernel.last().is_some_and(|v| v.is_nan()), "a gap in the kernel series, not 0");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded, "kernel time is a required counter");
}

#[test]
fn a_failed_page_input_or_response_time_counter_degrades_the_state() {
    for gone in [counter::PAGEIN, counter::DRESP] {
        let script = Script::new(all_counted().into_iter().filter(|c| c.key != gone).collect());
        let mut app = app_filled(&script, sysinfo_part);
        pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
        assert!(!app.snap.perf.answered(gone), "{gone} did not answer");
        assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded, "{gone} is a required counter");
    }
}

#[test]
fn the_compressed_memory_and_the_page_file_counters_are_optional() {
    for gone in [counter::COMPR, counter::PAGEFILE] {
        let script = Script::new(all_counted().into_iter().filter(|c| c.key != gone).collect());
        let mut app = app_filled(&script, sysinfo_part);
        pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
        assert!(!app.snap.perf.answered(gone));
        assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "{gone}: an optional counter never degrades");
    }
}

#[test]
fn a_failed_performance_info_call_is_degraded_and_cached_is_a_gap() {
    let script = Script::new(all_counted().into_iter().filter(|c| c.key != crate::perfdata::PERF_INFO).collect());
    let mut app = app_filled(&script, sysinfo_part);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded);
    assert!(app.summary_view().memory_footer.contains("Cached \u{2014}"), "cached is a gap: {}", app.summary_view().memory_footer);
}

#[test]
fn without_a_compression_counter_the_compressed_memory_is_the_compression_process() {
    let compression = |snap: &mut Snapshot| {
        sysinfo_part(snap);
        snap.procs.push(crate::sampler::ProcRow { pid: 4, name: "Memory Compression".into(), mem: 300 * 1024 * 1024, ..Default::default() });
    };
    let script = Script::new(all_counted().into_iter().filter(|c| c.key != counter::COMPR).collect());
    let mut app = app_filled(&script, compression);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(top(&app, PerfItem::Memory, "Compressed"), "300.0 MB");
    // The counter, when it answers, wins over the process.
    let script = Script::new(all_counted());
    let mut app = app_filled(&script, compression);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(top(&app, PerfItem::Memory, "Compressed"), "512.0 MB");
}

#[test]
fn when_sysinfo_sees_no_page_file_use_the_page_file_counters_percent() {
    let script = Script::new(all_counted());
    let mut app = app_filled(&script, sysinfo_part);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(app.snap.swap_used, 4_000_000_000, "40 % of the 10 GB page file");
    // What sysinfo read wins.
    let mut app = app_filled(&script, |snap: &mut Snapshot| {
        sysinfo_part(snap);
        snap.swap_used = 1_000_000_000;
    });
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(app.snap.swap_used, 1_000_000_000);
}

/// The real providers on this PC (needs Windows counters): what the sampler
/// builds from them.
#[test]
#[ignore = "reads this PC's performance counters"]
fn on_this_pc_the_real_counter_providers_fill_the_snapshot() {
    let service = SensorService::grouped(crate::sensors::provider::standard());
    let mut app = TaskApp::headless_over(settings(NORMAL), service.clone(), |_: &mut Snapshot| {});
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let perf = &app.snap.perf;
    eprintln!("perf {:?} power {:?} gaps {:?}", perf.cpu_perf_pct, perf.cpu_power_w, perf.unanswered);
    for d in &perf.disks {
        eprintln!("disk {} active {} read {} write {} total_read {} total_written {}", d.number, d.active, d.read_bps, d.write_bps, d.total_read, d.total_written);
    }
    assert!(perf.answered(counter::IDLE) && perf.answered(counter::DREAD) && perf.answered(counter::DWRITE), "disk counters answered");
    assert!(perf.disks.iter().any(|d| d.total_read > 0), "raw totals since boot come through");
    assert!(perf.cpu_perf_pct > 0.0, "the processor performance level");
    assert_eq!(service.threads_in(ProviderGroup::Hardware), 0, "only the counters run");
}
