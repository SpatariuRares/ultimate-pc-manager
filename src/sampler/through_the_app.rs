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
}

/// An app whose counters are what `script` says, with the collector's gaps
/// (`collector_gaps`) in every snapshot.
fn app_over(script: &Arc<Script>, speed: usize, collector_gaps: &'static [&'static str]) -> (TaskApp, SensorService) {
    let service = SensorService::grouped(vec![(ProviderGroup::Counters, fake::provider("Windows counters", script))]);
    let app = TaskApp::headless_over(settings(speed), service.clone(), move |snap: &mut Snapshot| {
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
    let disk = DiskLive { number: 0, active: 50.0, read_bps: 3_000_000.0, write_bps: 1_000_000.0, total_read: 9_000_000, total_written: 4_000_000, ..Default::default() };
    let snap = Snapshot { seq: 1, perf: PerfLive { cpu_perf_pct: perf, disks: vec![disk], cpu_power_w: Some(47.5), ..Default::default() }, ..Default::default() };
    TaskApp::headless(settings(NORMAL), snap)
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
