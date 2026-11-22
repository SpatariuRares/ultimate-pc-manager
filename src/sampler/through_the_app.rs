//! The Sampler and the app over a sensor service of fake providers: the
//! counters of the core pages come from the shared service, per provider
//! group.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::app::{PerfItem, ProviderState, TaskApp};
use crate::perfdata::{counter, DiskLive, PerfLive};
use crate::sampler::Snapshot;
use crate::sensors::fake::{self, Script};
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::service::ProviderGroup;
use crate::sensors::SensorService;
use crate::settings::Settings;

const NORMAL: usize = 2;
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

/// An app whose counters are what `script` says, over the sysinfo part of a
/// PC.
fn app_over(script: &Arc<Script>, speed: usize) -> (TaskApp, SensorService) {
    let service = counters_over(script);
    let app = TaskApp::headless_over(settings(speed), service.clone(), sysinfo_part);
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

#[test]
fn a_headless_app_over_fake_providers_shows_their_figures_on_the_core_pages() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, NORMAL);
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
    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != counter::DWRITE).collect());
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert!(!app.snap.perf.answered(counter::DWRITE), "no write rate: a gap");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded);

    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != counter::PERF).collect());
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert!(!app.snap.perf.answered(counter::PERF), "no performance level: a gap");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "an optional counter never degrades");
}

#[test]
fn when_no_counter_answers_the_state_is_unavailable() {
    let script = Script::new(Vec::new());
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Unavailable);
    assert_eq!(ProviderState::of(&Snapshot::default()), ProviderState::Waiting, "nothing sampled yet");
}

#[test]
fn paused_a_new_reading_does_not_change_the_pages_and_refresh_reads_once_after_the_request() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, PAUSED);
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

const FAST: usize = 0;

/// The next sample the Sampler sends.
fn next_sample(app: &TaskApp) -> Snapshot {
    app.sampler.rx.recv_timeout(Duration::from_secs(10)).expect("the Sampler sends a sample")
}

fn counters_over(script: &Arc<Script>) -> SensorService {
    SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", script))])
}

#[test]
fn the_counters_are_read_once_per_tick_at_the_settings_speed_whatever_interval_an_addon_asks() {
    let script = Script::new(counted(110.0));
    let service = counters_over(&script);
    let app = TaskApp::headless_over(settings(FAST), service.clone(), |_: &mut Snapshot| {});
    // An add-on that wants the sensors every 20 ms (Power & Freq does).
    let addon = service.interest();
    addon.set_interval_ms(20);
    for tick in 1..=3 {
        let snap = next_sample(&app);
        assert_eq!(snap.seq, tick);
        assert_eq!(script.reads(), tick as usize, "one read per Sampler tick, not one per 20 ms: tick {tick}");
    }
    drop(addon);
}

#[test]
fn a_snapshots_counters_come_from_a_read_that_began_after_its_tick() {
    let script = Script::new(counted(100.0));
    let app = TaskApp::headless_over(settings(FAST), counters_over(&script), |_: &mut Snapshot| {});
    assert_eq!(next_sample(&app).perf.cpu_perf_pct, 100.0);
    for level in [120.0, 140.0, 90.0] {
        // The PC changes right after a sample: the next tick starts a quarter of a second later.
        script.set(counted(level));
        assert_eq!(next_sample(&app).perf.cpu_perf_pct, level as f32, "not a frame read before the tick");
    }
}

#[test]
fn the_settings_speed_is_the_counters_speed_and_not_the_hardwares() {
    let counters = Script::new(counted(110.0));
    let hardware = Script::new(Vec::new());
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", &counters)), (ProviderGroup::Hardware, fake::provider("Vendor GPU", &hardware))]);
    let looking = service.interest_in(ProviderGroup::Hardware);
    looking.set_interval_ms(10);
    let app = TaskApp::headless_over(settings(FAST), service.clone(), |_: &mut Snapshot| {});
    for tick in 1..=2 {
        next_sample(&app);
        assert_eq!(counters.reads(), tick, "the counters read at the settings speed");
    }
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

/// A hand-built snapshot with the same figures `counted(perf_level)` counts.
fn by_hand(perf_level: f32) -> TaskApp {
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
    TaskApp::headless_over(settings(NORMAL), counters_over(script), fill)
}

fn top(app: &TaskApp, item: PerfItem, key: &str) -> String {
    app.stats(item).top.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()).unwrap_or_else(|| panic!("no {key}"))
}

#[test]
fn the_memory_and_disk_figures_the_provider_counted_show_as_hand_built_ones_do() {
    let script = Script::new(counted(110.0));
    let mut app = app_filled(&script, |snap: &mut Snapshot| {
        sysinfo_part(snap);
        snap.swap_used = 2_000_000_000;
    });
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let mut hand = by_hand(110.0);
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
    let script = Script::new(counted(110.0));
    let mut app = app_filled(&script, sysinfo_part);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(app.summary_view().cpu_kernel.last().copied(), Some(6.0), "kernel time of the whole CPU");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy);

    script.set(counted(110.0).into_iter().filter(|c| c.key != counter::KERNEL).collect());
    let seq = app.snap.seq;
    pull_until(&mut app, "a sample without the kernel counter", |a| a.snap.seq > seq && !a.snap.perf.answered(counter::KERNEL));
    assert!(app.summary_view().cpu_kernel.last().is_some_and(|v| v.is_nan()), "a gap in the kernel series, not 0");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded, "kernel time is a required counter");
}

#[test]
fn a_failed_page_input_or_response_time_counter_degrades_the_state() {
    for gone in [counter::PAGEIN, counter::DRESP] {
        let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != gone).collect());
        let mut app = app_filled(&script, sysinfo_part);
        pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
        assert!(!app.snap.perf.answered(gone), "{gone} did not answer");
        assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded, "{gone} is a required counter");
    }
}

#[test]
fn the_compressed_memory_and_the_page_file_counters_are_optional() {
    for gone in [counter::COMPR, counter::PAGEFILE] {
        let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != gone).collect());
        let mut app = app_filled(&script, sysinfo_part);
        pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
        assert!(!app.snap.perf.answered(gone));
        assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "{gone}: an optional counter never degrades");
    }
}

#[test]
fn a_failed_performance_info_call_is_degraded_and_cached_is_a_gap() {
    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != crate::perfdata::PERF_INFO).collect());
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
    let script = Script::new(counted(110.0).into_iter().filter(|c| c.key != counter::COMPR).collect());
    let mut app = app_filled(&script, compression);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(top(&app, PerfItem::Memory, "Compressed"), "300.0 MB");
    // The counter, when it answers, wins over the process.
    let script = Script::new(counted(110.0));
    let mut app = app_filled(&script, compression);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(top(&app, PerfItem::Memory, "Compressed"), "512.0 MB");
}

#[test]
fn when_sysinfo_sees_no_page_file_use_the_page_file_counters_percent() {
    let script = Script::new(counted(110.0));
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

/// Fixed facts a fake PC reports: rated 3.4 GHz, 8 cores, one disk.
fn machine_facts(base_mhz: u32) -> crate::perfdata::StaticInfo {
    crate::perfdata::StaticInfo {
        cpu: crate::perfdata::CpuInfo { base_mhz, cores: 8, ..Default::default() },
        disks: vec![crate::perfdata::DiskInfo { number: 0, model: "Fake SSD".into(), ..Default::default() }],
        ..Default::default()
    }
}

fn row(app: &TaskApp, item: PerfItem, name: &str) -> Option<String> {
    app.stats(item).table.into_iter().find(|(k, _)| k == name).map(|(_, v)| v)
}

#[test]
fn the_core_pages_show_the_machine_facts_their_providers_report() {
    let script = Script::new(counted(110.0));
    *script.machine.lock().unwrap() = Some(machine_facts(3_400));
    let (mut app, _service) = app_over(&script, NORMAL);
    // The facts join the frame after the first figures (a read of their own).
    pull_until(&mut app, "the machine facts", |a| a.snap.seq >= 1 && a.snap.machine.is_some());
    let top = app.stats(PerfItem::Cpu).top;
    assert!(top.iter().any(|(k, v)| k == "Speed" && v == "3.74 GHz"), "the base clock the provider reported x the performance level: {top:?}");
    assert_eq!(row(&app, PerfItem::Cpu, "Base speed:").as_deref(), Some("3.40 GHz"));
    assert_eq!(row(&app, PerfItem::Cpu, "Physical cores:").as_deref(), Some("8"));
}

#[test]
fn without_machine_facts_the_speed_and_base_speed_are_blank() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let top = app.stats(PerfItem::Cpu).top;
    assert!(top.iter().any(|(k, v)| k == "Speed" && v == "\u{2014}"), "no base clock: no speed: {top:?}");
    assert_eq!(row(&app, PerfItem::Cpu, "Base speed:").as_deref(), Some("\u{2014}"));
}

#[test]
fn the_override_of_the_machine_wins_over_what_the_providers_report() {
    let script = Script::new(counted(110.0));
    *script.machine.lock().unwrap() = Some(machine_facts(3_400));
    let (mut app, _service) = app_over(&script, NORMAL);
    app.machine_info = Some(crate::summary_view::tests::clocked(2_000));
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1 && a.snap.machine.is_some());
    let top = app.stats(PerfItem::Cpu).top;
    assert!(top.iter().any(|(k, v)| k == "Speed" && v == "2.20 GHz"), "the override: {top:?}");
}

#[test]
fn the_machine_facts_stay_when_a_later_reading_has_none() {
    let script = Script::new(counted(110.0));
    *script.machine.lock().unwrap() = Some(machine_facts(3_400));
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.machine.is_some());
    let seq = app.snap.seq;
    *script.machine.lock().unwrap() = None;
    pull_until(&mut app, "a later sample", |a| a.snap.seq >= seq + 2);
    assert_eq!(row(&app, PerfItem::Cpu, "Base speed:").as_deref(), Some("3.40 GHz"), "the last facts stand");
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
    for key in [counter::KERNEL, counter::PAGEIN, counter::DRESP, crate::perfdata::PERF_INFO] {
        assert!(perf.answered(key), "{key} answered");
    }
    assert!(perf.threads > 0 && perf.handles > 0 && perf.commit > 0, "GetPerformanceInfo's figures: {perf:?}");
    assert!(perf.page_ins > 0, "pages read since boot");
    assert_eq!(service.threads_in(ProviderGroup::Hardware), 0, "only the counters run");
}

/// What one adapter counts (luid 0x1670C): per-process engine instances, an
/// engine of the same type twice, and the adapter's memory.
fn gpu_counted() -> Vec<CounterSet> {
    let engine = |pid: u32, eng: u32, ty: &str, v: f64| Instance::new(&format!("pid_{pid}_luid_0x00000000_0x0001670C_phys_0_eng_{eng}_engtype_{ty}"), v);
    vec![
        CounterSet::new(
            counter::GPU,
            vec![engine(1234, 0, "3D", 10.0), engine(88, 0, "3D", 20.0), engine(88, 1, "Copy", 1.5), engine(5, 2, "VideoDecode", 5.0), engine(1234, 3, "Copy", 2.5)],
        ),
        CounterSet::new(counter::GDED, vec![Instance::new("luid_0x00000000_0x0001670C_phys_0", 2_000_000_000.0)]),
        CounterSet::new(counter::GSH, vec![Instance::new("luid_0x00000000_0x0001670C_phys_0", 500_000_000.0)]),
    ]
}

const LUID: u64 = 0x1670C;

#[test]
fn the_gpu_figures_of_the_core_pages_come_from_the_provider() {
    let script = Script::new(counted(110.0).into_iter().chain(gpu_counted()).collect());
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let gpus = &app.snap.perf.gpus;
    assert_eq!(gpus.len(), 1);
    let gpu = &gpus[0];
    assert_eq!(gpu.luid, LUID);
    let named = |v: &[(String, f32)]| v.iter().map(|(n, x)| (n.clone(), *x)).collect::<Vec<_>>();
    assert_eq!(named(&gpu.cells), [("3D".to_string(), 30.0), ("Copy 1".into(), 1.5), ("Video decode".into(), 5.0), ("Copy 2".into(), 2.5)], "every engine in index order, repeated types numbered");
    assert_eq!(named(&gpu.engines), [("3D".to_string(), 30.0), ("Copy".into(), 2.5), ("Video decode".into(), 5.0)], "the busiest engine of each type");
    assert_eq!((gpu.util, gpu.dedicated, gpu.shared), (30.0, 2_000_000_000, 500_000_000));
    // The same figures, built by hand, give the same page.
    let hand_gpu = crate::perfdata::GpuLive { luid: LUID, util: 30.0, engines: gpu.engines.clone(), cells: gpu.cells.clone(), dedicated: 2_000_000_000, shared: 500_000_000 };
    let hand = TaskApp::headless(settings(NORMAL), Snapshot { seq: 1, perf: PerfLive { gpus: vec![hand_gpu], ..Default::default() }, ..Default::default() });
    let top = app.stats(PerfItem::Gpu(LUID)).top.clone();
    assert!(top.iter().any(|(k, v)| k == "Utilization" && v.starts_with("30")), "{top:?}");
    assert_eq!(top, hand.stats(PerfItem::Gpu(LUID)).top, "the GPU page");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "a GPU never degrades the state");
}

#[test]
fn each_process_shows_the_gpu_of_its_engines_and_one_without_engines_shows_zero() {
    let script = Script::new(counted(110.0).into_iter().chain(gpu_counted()).collect());
    let mut app = TaskApp::headless_over(settings(NORMAL), counters_over(&script), |snap: &mut Snapshot| {
        let row = |pid, gpu| crate::sampler::ProcRow { pid, gpu, ..Default::default() };
        snap.procs = vec![row(1234, 99.0), row(88, 99.0), row(77, 99.0)];
    });
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    let by_pid: Vec<(u32, f32)> = app.snap.procs.iter().map(|p| (p.pid, p.gpu)).collect();
    assert_eq!(by_pid, [(1234, 12.5), (88, 21.5), (77, 0.0)]);
}

#[test]
fn flight_recorder_is_handed_the_gpu_engines_in_every_sample() {
    let script = Script::new(counted(110.0).into_iter().chain(gpu_counted()).collect());
    let (app, _service) = app_over(&script, NORMAL);
    // The samples as they go to the add-ons (the app hands each one on).
    let mut seen = Vec::new();
    let end = Instant::now() + Duration::from_secs(10);
    while seen.len() < 3 {
        assert!(Instant::now() < end, "timed out waiting for three samples");
        if let Ok(sample) = app.sampler.rx.recv_timeout(Duration::from_millis(100)) {
            seen.push(sample);
        }
    }
    for s in &seen {
        let engines = crate::addons::flight_recorder::layout::engines_of(&s.perf.gpus, &[]);
        assert_eq!(engines.map(|e| e.cells.len()), Some(4), "sample {} carries the engines: {:?}", s.seq, s.perf.gpus);
    }
}

#[test]
fn a_pc_whose_gpu_counters_give_nothing_has_no_gpu_figures_and_no_gap_for_them() {
    let script = Script::new(counted(110.0));
    let (mut app, _service) = app_over(&script, NORMAL);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert!(app.snap.perf.gpus.is_empty() && app.snap.perf.gpu_by_pid.is_empty());
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy, "no GPU driver is not a problem");
}

#[test]
fn the_first_figures_do_not_wait_for_the_machine_facts() {
    let script = Script::new(counted(110.0));
    *script.machine.lock().unwrap() = Some(machine_facts(3_400));
    *script.machine_delay.lock().unwrap() = Duration::from_millis(1_500);
    let start = Instant::now();
    let app = TaskApp::headless_over(settings(FAST), counters_over(&script), |_: &mut Snapshot| {});
    let first = next_sample(&app);
    assert!(start.elapsed() < Duration::from_millis(700), "the first snapshot waited for the facts: {:?}", start.elapsed());
    assert_eq!(first.perf.cpu_perf_pct, 110.0, "with its counter figures");
    assert!(first.machine.is_none(), "the facts come when they are read");
    let mut seen = first;
    while seen.machine.is_none() {
        seen = next_sample(&app);
    }
    assert_eq!(seen.machine.unwrap().cpu.base_mhz, 3_400);
}

#[test]
fn a_provider_that_stops_answering_turns_its_figures_into_gaps_and_degrades_the_state() {
    let (disks, rest): (Vec<_>, Vec<_>) = counted(110.0).into_iter().partition(|c| [counter::PERF, counter::IDLE, counter::DREAD, counter::DWRITE, counter::POWER].contains(&c.key));
    let (disks, rest) = (Script::new(disks), Script::new(rest));
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Disk counters", &disks)), (ProviderGroup::Counters, fake::provider("System counters", &rest))]);
    let mut app = TaskApp::headless_over(settings(FAST), service, sysinfo_part);
    pull_until(&mut app, "the first sample", |a| a.snap.seq >= 1);
    assert_eq!(app.summary_view().cpu_kernel.last().copied(), Some(6.0));
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Healthy);
    // The system counters stop answering (a read that never ends).
    rest.slow(Duration::from_secs(20));
    pull_until(&mut app, "the read timeout", |a| !a.snap.perf.answered(counter::KERNEL));
    assert!(app.summary_view().cpu_kernel.last().is_some_and(|v| v.is_nan()), "a gap, not 0 and not the old figure");
    assert_eq!(ProviderState::of(&app.snap), ProviderState::Degraded);
    assert_eq!(crate::app::status_parts(false, ProviderState::of(&app.snap), 0, "", 1)[0], "Native providers degraded");
    assert!(app.snap.perf.answered(counter::DREAD), "the other provider's figures stay");
}

#[test]
fn one_reading_gives_a_core_page_and_an_addon_the_same_speed() {
    let script = Script::cpu(crate::sensors::cpu::CpuSample { name: "Test CPU".into(), base_mhz: Some(3_400.0), performance_percent: Some(110.0), ..Default::default() });
    *script.machine.lock().unwrap() = Some(machine_facts(3_400));
    let service = counters_over(&script);
    let mut app = TaskApp::headless_over(settings(FAST), service.clone(), |_: &mut Snapshot| {});
    // What an add-on reads: the latest frame, as Power & Freq's Core Average does.
    let addon = service.interest();
    for (level, shown) in [(110.0, "3.74 GHz"), (150.0, "5.10 GHz")] {
        script.cpu.lock().unwrap().as_mut().unwrap().performance_percent = Some(level);
        script.set(vec![CounterSet::new(counter::PERF, vec![Instance::new("_Total", level)])]);
        pull_until(&mut app, "a sample of this reading", |a| a.snap.perf.cpu_perf_pct == level as f32 && a.snap.machine.is_some());
        let frame = addon.latest().expect("a frame");
        let hz = frame.reading(crate::sensors::cpu::CORE_AVERAGE).and_then(|r| r.value).expect("Core Average");
        assert_eq!(crate::metrics::fmt_ghz((hz / 1e6) as f32), shown, "the add-on");
        assert_eq!(top(&app, PerfItem::Cpu, "Speed"), shown, "the core page");
    }
}
