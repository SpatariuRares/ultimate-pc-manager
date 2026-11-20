//! The Disk Space add-on's tests (scan, History, settings, menu).

use super::*;
use crate::addons::Addon;
use crate::platform::AddonContext;
use history::tests::TempStore;
use scan::tests::TempTree;
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

fn wait_idle(d: &DiskSpace) -> View {
    let t = Instant::now();
    while d.view().running.is_some() && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(1));
    }
    d.view()
}

fn name(p: &str) -> &str {
    Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p)
}

fn file(p: &str, size: u64) -> Event {
    Event::File(PathBuf::from(p), size)
}

#[test]
fn a_scan_measures_the_folder_and_says_where_the_space_goes() {
    let t = TempTree::new("addon-measure");
    t.file("big/movie.mkv", 500_000);
    t.file("big/deep/clip.mp4", 300_000);
    t.file("small/note.txt", 10_000);
    t.file("loose.bin", 70_000);
    let d = DiskSpace::default();
    d.scan(&t.0, None);
    let v = wait_idle(&d);
    assert_eq!(v.files, 4);
    assert!(v.bytes >= 880_000, "allocated sizes are at least the lengths: {}", v.bytes);
    assert_eq!(v.unreadable, 0);
    assert_eq!(v.progress, None);
    assert_eq!(v.folders.iter().map(|(p, _)| name(p)).collect::<Vec<_>>(), vec!["big", "small"]);
    assert_eq!(v.largest.iter().map(|(p, _)| name(p)).collect::<Vec<_>>(), vec!["movie.mkv", "clip.mp4", "loose.bin", "note.txt"]);
    assert_eq!(v.largest_folder.as_ref().map(|(p, _)| name(p)), Some("big"));
    let finished = v.finished.unwrap();
    assert!(finished.starts_with("Finished today at "), "{finished}");
    assert!(finished.ends_with(" in 4 files."), "{finished}");
    let sentence = v.where_it_goes.unwrap();
    assert!(sentence.starts_with("big holds "), "{sentence}");
}

#[cfg(windows)]
#[test]
fn a_junction_to_a_sibling_folder_is_skipped_silently_and_the_sibling_counted_once() {
    let t = TempTree::new("addon-junction");
    t.file("real/data.bin", 20_000);
    t.junction("link", "real");
    let d = DiskSpace::default();
    d.scan(&t.0, None);
    let v = wait_idle(&d);
    assert_eq!(v.files, 1, "the sibling is counted once");
    assert!(v.bytes >= 20_000 && v.bytes < 2 * 20_000, "{}", v.bytes);
    assert_eq!(v.folders.iter().map(|(p, _)| name(p)).collect::<Vec<_>>(), vec!["real"]);
    assert_eq!(v.unreadable, 0, "a skipped link is not unreadable");
    assert_eq!(v.skipped, None);
}

#[cfg(windows)]
#[test]
fn a_file_with_a_second_hard_link_is_counted_once() {
    let t = TempTree::new("addon-hard-link");
    t.file("a/data.bin", 40_000);
    t.hard_link("b/same.bin", "a/data.bin");
    t.file("b/other.bin", 1_000);
    let d = DiskSpace::default();
    d.scan(&t.0, None);
    let v = wait_idle(&d);
    assert_eq!(v.files, 2, "the hard-linked file once, plus the other file");
    assert!(v.bytes >= 41_000 && v.bytes < 40_000 * 2, "its size counted once: {}", v.bytes);
    assert_eq!(v.hard_link_limit, None, "far from the tracking limit");
}

#[cfg(windows)]
#[test]
fn reaching_the_hard_link_tracking_limit_says_so_and_counts_further_links() {
    let t = TempTree::new("addon-hard-link-cap");
    t.file("a/one.bin", 10_000);
    t.hard_link("b/one.bin", "a/one.bin");
    t.file("a/two.bin", 10_000);
    t.hard_link("b/two.bin", "a/two.bin");
    let d = DiskSpace::default();
    let root = t.0.clone();
    // Tracks one hard-linked file only: the second is past the limit.
    d.scan_with(&t.0, None, move |job| scan::walk_links(scan::WORKERS, Some(1), &root, &|| checkpoint(job), &mut |events| deliver(job, events)));
    let v = wait_idle(&d);
    assert_eq!(v.files, 3, "the tracked file once, the other one at each of its links");
    assert_eq!(v.hard_link_limit.as_deref(), Some("Hard-link tracking limit reached. Scan a smaller folder."));
}

#[cfg(windows)]
#[test]
fn a_target_that_is_itself_a_junction_is_measured_through_the_link() {
    let t = TempTree::new("addon-junction-target");
    t.file("real/data.bin", 20_000);
    t.file("real/deep/more.bin", 10_000);
    t.junction("link", "real");
    let d = DiskSpace::default();
    d.scan(&t.0.join("link"), None);
    let v = wait_idle(&d);
    assert_eq!(v.files, 2, "the linked folder is measured");
    assert!(v.bytes >= 30_000, "{}", v.bytes);
    assert_eq!(v.unreadable, 0);
}

#[test]
fn a_folder_whose_listing_fails_part_way_keeps_what_was_read_and_counts_as_unreadable() {
    let t = TempTree::new("addon-part-way");
    for i in 0..6 {
        t.file(&format!("flaky/f{i}.bin"), 1_000);
    }
    t.file("fine/ok.bin", 1_000);
    let d = DiskSpace::default();
    let (root, flaky) = (t.0.clone(), t.0.join("flaky"));
    // The listing of "flaky" fails after its first two entries.
    d.scan_with(&t.0, None, move |job| scan::walk_failing(&flaky, 2, &root, &|| checkpoint(job), &mut |events| deliver(job, events)));
    let v = wait_idle(&d);
    assert_eq!(v.files, 3, "the two entries read before the error, plus the other folder's file");
    assert_eq!(v.unreadable, 1, "the folder still counts as unreadable");
}

#[test]
fn a_folder_deeper_than_260_characters_is_measured_shown_and_copied() {
    let t = TempTree::new("addon-long-path");
    let mut rel = PathBuf::new();
    while t.0.join(&rel).as_os_str().len() < 300 {
        rel.push("a-rather-long-folder-name-for-the-test");
    }
    let rel = rel.join("deep.bin");
    t.file(&rel.to_string_lossy(), 20_000);
    let deep = t.0.join(&rel);
    assert!(deep.as_os_str().len() > 260);
    let mut d = DiskSpace::default();
    d.scan(&t.0, None);
    let v = wait_idle(&d);
    assert_eq!(v.files, 1, "the deep file is measured");
    assert!(v.bytes >= 20_000, "{}", v.bytes);
    assert_eq!(v.unreadable, 0);

    let file = nav::Key::File(deep.clone());
    let said = with_page(|page| {
        d.act(actions::Verb::ShowInExplorer, &file, page);
        d.act(actions::Verb::CopyPath, &file, page);
        std::mem::take(&mut page.actions)
    });
    // The path stays unprefixed; the prefix is added only when it is handed to Windows.
    assert!(matches!(&said[..], [Action::ShowInExplorer(p), Action::Copy(s)] if *p == deep && *s == deep.display().to_string()), "{said:?}");
}

#[test]
fn the_status_strip_gives_the_last_scans_time_or_not_scanned() {
    assert_eq!(View::default().strip_last(), "Not scanned");
    let d = DiskSpace::default();
    d.scan_with("R:\\", None, |job| deliver(job, vec![file("R:\\a", 10)]));
    let last = wait_idle(&d).strip_last();
    assert!(last.starts_with("Done today at "), "{last}");
    assert_eq!(last.len(), "Done today at 14:03".len(), "{last}");
}

#[test]
fn the_status_strip_names_each_state() {
    let v = View::default();
    assert_eq!((v.phase.label(), v.strip_last().as_str()), ("Idle", "Not scanned"), "before any scan");

    let d = DiskSpace::default();
    let (go, kept_going) = two_step_scan(&d);
    d.pause_scan();
    go.send(()).unwrap();
    assert_eq!(wait_phase(&d, Phase::Paused).phase.label(), "Paused");
    d.resume_scan();
    assert!(kept_going.recv_timeout(Duration::from_secs(2)).unwrap());
    let v = wait_idle(&d);
    assert_eq!(v.phase.label(), "Finished");
    assert!(v.strip_last().starts_with("Done today at "), "{}", v.strip_last());

    let d = DiskSpace::default();
    let (go, kept_going) = two_step_scan(&d);
    d.stop_scan();
    go.send(()).unwrap();
    assert!(!kept_going.recv_timeout(Duration::from_secs(2)).unwrap());
    let v = wait_phase(&d, Phase::Stopped);
    assert_eq!(v.phase.label(), "Partial");
    let last = v.strip_last();
    assert!(last.starts_with("Partial today at "), "{last}");
    assert_eq!(last.len(), "Partial today at 14:03".len(), "{last}");
}

/// A fake scan of `root` (`expected` bytes in use) that has counted
/// `bytes` and waits for the returned sender.
fn drive_scan_at(d: &DiskSpace, root: &str, expected: Option<u64>, bytes: u64) -> std::sync::mpsc::Sender<()> {
    let (go, wait_go) = channel::<()>();
    let (delivered, wait_delivered) = channel();
    let at = format!("{}\\a", root.trim_end_matches('\\'));
    d.scan_with(root, expected, move |job| {
        deliver(job, vec![file(&at, bytes)]);
        delivered.send(()).unwrap();
        let _ = wait_go.recv();
    });
    wait_delivered.recv().unwrap();
    go
}

#[test]
fn a_drive_scan_half_way_after_n_seconds_has_about_n_seconds_left() {
    let d = DiskSpace::default();
    let go = drive_scan_at(&d, "C:\\", Some(1000), 500);
    d.advance_clock(Duration::from_secs(40));
    assert_eq!(d.view().left.as_deref(), Some("about 40 s left"));
    d.advance_clock(Duration::from_secs(140));
    assert_eq!(d.view().left.as_deref(), Some("about 3 min left"), "180 s in, half way");
    go.send(()).unwrap();
    assert_eq!(wait_idle(&d).left, None, "nothing is left once finished");
}

#[test]
fn no_time_left_in_the_first_seconds_nor_for_a_folder_without_a_size() {
    let d = DiskSpace::default();
    let go = drive_scan_at(&d, "C:\\", Some(1000), 500);
    d.advance_clock(Duration::from_secs(2));
    assert_eq!(d.view().left, None, "too early to tell");
    go.send(()).unwrap();
    wait_idle(&d);

    let d = DiskSpace::default();
    let go = drive_scan_at(&d, "R:\\folder", None, 500);
    d.advance_clock(Duration::from_secs(40));
    assert_eq!(d.view().left, None, "a folder has no expected size");
    go.send(()).unwrap();
}

#[test]
fn time_spent_paused_does_not_count_toward_the_time_left() {
    let d = DiskSpace::default();
    let go = drive_scan_at(&d, "C:\\", Some(1000), 500);
    d.advance_clock(Duration::from_secs(20));
    d.pause_scan();
    d.advance_clock(Duration::from_secs(600));
    assert_eq!(d.view().left, None, "nothing is estimated while paused");
    d.resume_scan();
    d.advance_clock(Duration::from_secs(20));
    assert_eq!(d.view().left.as_deref(), Some("about 40 s left"), "40 s running, half way: the 10 min paused do not count");
    // A second pause adds up with the first.
    d.pause_scan();
    d.advance_clock(Duration::from_secs(300));
    d.resume_scan();
    assert_eq!(d.view().left.as_deref(), Some("about 40 s left"));
    go.send(()).unwrap();
    wait_idle(&d);
}

#[test]
fn skipped_folders_give_an_example_and_say_when_only_the_system_can_read_them() {
    let scan = |events: Vec<Event>| {
        let d = DiskSpace::default();
        d.scan_with("R", None, move |job| deliver(job, events));
        wait_idle(&d).skipped_why
    };
    let p = |s: &str| PathBuf::from(s);
    let x = |s: &str| path_text(Path::new(s));
    assert_eq!(scan(vec![Event::Unreadable(p("R/gone"))]), Some(format!("For example {}.", x("R/gone"))));
    assert_eq!(
        scan(vec![Event::Unreadable(p("R/gone")), Event::Denied(p("R/locked"))]),
        Some(format!("For example {}. It is readable only by the system.", x("R/locked"))),
        "the example is one only the system can read"
    );
    assert_eq!(
        scan(vec![Event::Denied(p("R/a")), Event::Denied(p("R/b"))]),
        Some(format!("For example {}. They are readable only by the system.", x("R/a")))
    );
    assert_eq!(scan(vec![file("R/f", 5)]), None, "nothing skipped");
}

#[test]
fn the_disk_space_menu_offers_what_the_scan_allows_and_its_commands_act() {
    let store = TempStore::new("addon-menu");
    let tree = TempTree::new("addon-menu");
    tree.file("a/movie.mkv", 5_000);
    let folder = tree.0.display().to_string();
    let disks = vec![DiskRow { mount: "C:\\".into(), total: 1000, free: 400, ..Default::default() }];
    let mut d = with_history(&store);

    d.target = Some(Target::Drive("F:\\".into()));
    let m = d.menu(&disks);
    assert_eq!((m.start.as_str(), m.can_start, m.running, m.can_clear), ("Start: measure F:\\", false, false, false), "F:\\ is unplugged");

    d.command(Command::Target(Target::Folder(folder.clone())), &disks);
    d.command(Command::Group(Group::FileType), &disks);
    d.command(Command::View(Drawing::Block), &disks);
    assert_eq!((d.group, d.drawing), (Group::FileType, Drawing::Block));
    let m = d.menu(&disks);
    assert_eq!((m.start, m.can_start), (format!("Start: measure {folder}"), true));

    d.command(Command::Start, &disks);
    wait_saved(&d, 1);
    let m = d.menu(&disks);
    assert_eq!((m.start, m.can_start, m.running, m.can_clear), (format!("Start: measure {folder} again"), true, false, true));

    d.command(Command::ClearHistory, &disks);
    assert!(d.clear_question().is_some(), "Clear History… asks first");
}

#[test]
fn during_a_scan_the_menu_pauses_resumes_and_stops_it() {
    let disks = Vec::new();
    let d = &mut DiskSpace::default();
    let (go, _kept_going) = two_step_scan(d);
    let m = d.menu(&disks);
    assert_eq!((m.can_start, m.running, m.pause), (false, true, "Pause"));
    d.command(Command::Pause, &disks);
    go.send(()).unwrap();
    wait_phase(d, Phase::Paused);
    assert_eq!(d.menu(&disks).pause, "Resume");
    d.command(Command::Resume, &disks);
    assert_eq!(d.view().phase, Phase::Running);
    d.command(Command::Stop, &disks);
    assert!(d.confirm.is_some(), "Stop asks first, as the toolbar's does");
}

fn locked_folders(job: &Job) {
    let mut events: Vec<Event> = (0..10).map(|i| Event::Unreadable(PathBuf::from(format!("R/locked/{i}")))).collect();
    events.push(file("R/open/f", 5));
    deliver(job, events);
}

#[test]
fn skipped_folders_give_a_note_with_up_to_eight_examples_until_dismissed() {
    let d = DiskSpace::default();
    d.scan_with("R", None, locked_folders);
    let v = wait_idle(&d);
    assert_eq!(v.unreadable, 10);
    assert_eq!(v.skipped, text::skipped(10, true));
    assert_eq!(v.skipped_examples.len(), 8);
    assert_eq!(v.skipped_examples[0], path_text(Path::new("R/locked/0")));

    d.dismiss_skipped();
    let v = d.view();
    assert_eq!((v.skipped, v.skipped_examples.len()), (None, 0), "Dismiss hides the note");
    assert_eq!(v.unreadable, 10, "the count stays");

    d.scan_with("R", None, locked_folders);
    assert!(wait_idle(&d).skipped.is_some(), "a new scan shows its own note");
}

#[test]
fn a_scan_without_skipped_folders_has_no_note() {
    let d = DiskSpace::default();
    d.scan_with("R", None, |job| deliver(job, vec![file("R/a/f", 5)]));
    let v = wait_idle(&d);
    assert_eq!((v.skipped, v.skipped_examples.len()), (None, 0));
}

#[test]
fn a_drive_scan_shows_its_progress_against_the_used_space() {
    let d = DiskSpace::default();
    let (go, wait_go) = channel::<()>();
    let (delivered, wait_delivered) = channel();
    d.scan_with("C:\\", Some(1000), move |job| {
        deliver(job, vec![file("C:\\a\\x", 600), file("C:\\b\\y", 40)]);
        delivered.send(()).unwrap();
        let _ = wait_go.recv();
    });
    wait_delivered.recv().unwrap();
    let v = d.view();
    assert_eq!(v.running.as_deref(), Some("C:\\"));
    assert_eq!(v.progress, Some(0.64));
    assert_eq!(text::progress(v.progress, v.files, v.bytes), "64% \u{00b7} 2 files \u{00b7} 640 B so far");
    assert_eq!(v.finished, None, "no result line while running");
    go.send(()).unwrap();
    let v = wait_idle(&d);
    assert_eq!(v.progress, None);
    assert_eq!(v.where_it_goes.as_deref(), Some("a holds 600 B, most of what the scan counted on C:\\."));
}

fn args(list: &[&str]) -> Vec<String> {
    std::iter::once("app.exe").chain(list.iter().copied()).map(String::from).collect()
}

#[test]
fn the_scan_flag_measures_the_given_folder_without_a_click_and_remembers_it() {
    let t = TempTree::new("addon-flag");
    t.file("a/x.bin", 5000);
    t.file("y.bin", 100);
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "target": "all" }));
    d.start(&ctx);
    d.command_line(&args(&["--page", "diskspace", &format!("--scan={}", t.0.display())]));
    d.on_sample(&crate::sampler::Snapshot::default());
    let v = wait_idle(&d);
    assert_eq!(v.root, Some(t.0.display().to_string()));
    assert_eq!(v.files, 2);
    assert_eq!(d.settings()["target"], serde_json::json!(t.0.display().to_string()));
    d.on_sample(&crate::sampler::Snapshot::default());
    assert!(wait_idle(&d).finished.is_some(), "one scan per flag");
}

#[test]
fn the_scan_flag_on_a_drive_no_longer_connected_measures_nothing() {
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "target": "F:\\" }));
    d.start(&ctx);
    d.command_line(&args(&["--scan"]));
    let sample = crate::sampler::Snapshot { disks: vec![DiskRow { mount: "C:\\".into(), total: 10, ..Default::default() }], ..Default::default() };
    d.on_sample(&sample);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(d.view(), View::default());
    assert_eq!(d.target, Some(Target::Drive("F:\\".into())), "the choice is kept for when it is plugged in again");
}

#[test]
fn all_disks_measures_every_drive_under_one_root() {
    let (c, e) = (TempTree::new("addon-all-c"), TempTree::new("addon-all-d"));
    c.file("big/movie.mkv", 50_000);
    e.file("note.txt", 10);
    let d = DiskSpace::default();
    let roots = vec![c.0.display().to_string(), e.0.display().to_string()];
    d.measure(&Target::AllDisks, Plan { roots: roots.clone(), expected: None });
    let v = wait_idle(&d);
    assert_eq!(v.root.as_deref(), Some(target::ALL_DISKS));
    assert_eq!(v.files, 2);
    assert_eq!(v.folders.iter().map(|(p, _)| p.clone()).collect::<Vec<_>>(), roots, "one entry per drive, largest first");
}

#[test]
fn its_page_opens_by_id_and_by_its_alias() {
    use crate::addons::{AddonHost, Landing, LAST_USED, SUMMARY};
    let host = AddonHost::standard();
    for request in ["disk-space", "diskspace", "Disk Space"] {
        assert_eq!(host.landing(Some(request), LAST_USED, SUMMARY), Landing::Page("disk-space"), "{request}");
    }
}

#[test]
fn nothing_is_scanned_until_start() {
    let (ctx, runner) = AddonContext::recording();
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "target": "C:\\" }));
    d.start(&ctx);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(d.view(), View::default());
    assert!(runner.launches().is_empty());
}

#[test]
fn turning_disk_space_off_during_a_scan_stops_it_and_its_results_never_show() {
    let (ctx, runner) = AddonContext::recording();
    let mut d = DiskSpace::default();
    d.start(&ctx);
    let (started, wait_started) = channel();
    let (finished, wait_finished) = channel();
    d.scan_with("C:\\", None, move |job| {
        deliver(job, vec![file("C:\\a", 10)]);
        started.send(()).unwrap();
        let t = Instant::now();
        while !job.cancelled() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(1));
        }
        let stopped = job.cancelled();
        deliver(job, vec![file("C:\\b", 999)]);
        finished.send(stopped).unwrap();
    });
    wait_started.recv().unwrap();
    assert_eq!(d.view().running.as_deref(), Some("C:\\"));

    d.stop();
    assert!(wait_finished.recv_timeout(Duration::from_secs(2)).unwrap(), "the scan saw it was stopped");
    d.start(&ctx);
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(d.view(), View::default(), "nothing of the stopped scan is left");
    assert!(runner.launches().is_empty());
}

#[test]
fn the_chosen_target_is_its_setting_and_survives_turning_off_and_on() {
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    assert!(d.settings().is_null(), "no target chosen, no settings of its own");
    d.load_settings(&serde_json::json!({ "drive": "D:\\", "newer": 1 }));
    d.start(&ctx);
    assert_eq!(d.target, Some(Target::Drive("D:\\".into())), "the setting saved before #81 is read");
    d.target = Some(Target::Folder("E:\\Games".into())); // a folder picked with Choose Folder…
    assert_eq!(d.settings(), serde_json::json!({ "target": "E:\\Games", "newer": 1 }), "keeps what it does not know");
    d.stop();
    d.start(&ctx);
    assert_eq!(d.settings(), serde_json::json!({ "target": "E:\\Games", "newer": 1 }));
    d.load_settings(&serde_json::json!("garbage"));
    assert_eq!(d.target, None, "unreadable settings give the default");
    d.load_settings(&serde_json::json!({ "target": "all" }));
    assert_eq!(d.target, Some(Target::AllDisks));
    assert_eq!(d.settings(), serde_json::json!({ "target": "all" }));
}

fn finished_scan(events: Vec<Event>) -> View {
    let d = DiskSpace::default();
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    wait_idle(&d)
}

#[test]
fn the_file_type_legend_counts_every_category_in_a_fixed_order() {
    let v = finished_scan(vec![file("R:\\v\\a.MP4", 3000), file("R:\\v\\b.jpg", 1000), file("R:\\d\\c.pdf", 50), file("R:\\setup.exe", 400)]);
    let legend: Vec<_> = v.types.iter().map(|t| (t.category.name(), t.files, t.bytes)).collect();
    assert_eq!(
        legend,
        vec![("Media", 2, 4000), ("Documents", 1, 50), ("Code", 0, 0), ("Archives", 0, 0), ("Applications", 1, 400), ("System", 0, 0), ("Other", 0, 0)]
    );
    assert_eq!(text::files(v.types[0].files), "2 files");
    assert_eq!(text::files(v.types[1].files), "1 file");
}

#[test]
fn the_lists_follow_the_group_chosen() {
    let v = finished_scan(vec![
        file("R:\\v\\a.mp4", 3000),
        file("R:\\v\\b.mp4", 1000),
        file("R:\\d\\c.pdf", 50),
        file("R:\\d\\e\\f.zip", 2500),
        file("R:\\setup.exe", 400),
    ]);
    let rows = |g: Group| v.rows(g).into_iter().map(|r| (r.label, r.bytes)).collect::<Vec<_>>();
    let names = |g: Group| v.rows(g).into_iter().map(|r| (name(&r.label).to_string(), r.bytes)).collect::<Vec<_>>();
    let s = |l: &[(&str, u64)]| l.iter().map(|(n, b)| (n.to_string(), *b)).collect::<Vec<_>>();
    assert_eq!(names(Group::FileSize), s(&[("a.mp4", 3000), ("f.zip", 2500), ("b.mp4", 1000), ("setup.exe", 400), ("c.pdf", 50)]));
    assert_eq!(rows(Group::FileType), s(&[("Media", 4000), ("Archives", 2500), ("Applications", 400), ("Documents", 50)]), "only types present, largest first");
    assert_eq!(v.rows(Group::FileType)[0].detail.as_deref(), Some("2 files"));
    assert_eq!(names(Group::FolderSize), s(&[("v", 4000), ("d", 2550)]));
    assert_eq!(v.rows(Group::FileSize)[0].category, Some(Category::Media), "files are coloured by type");
    assert_eq!(v.rows(Group::FolderSize)[0].path.as_deref().map(name), Some("v"));
    assert_eq!(v.rows(Group::FileType)[0].path, None, "a type is not a place");
    assert_eq!(text::file_types(v.rows(Group::FileType).len() as u64), "4 file types");
}

#[test]
fn the_lists_and_the_legend_follow_the_level_entered() {
    let mut d = DiskSpace::default();
    let events = vec![file("R:\\v\\a.mp4", 3000), file("R:\\d\\c.pdf", 50), file("R:\\d\\e\\f.zip", 2500), file("R:\\setup.exe", 400)];
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    wait_idle(&d);
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\d")));
    let v = d.view();
    let crumbs: Vec<_> = v.crumbs.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(crumbs, ["R:\\", "d"]);
    let names = |g: Group| v.rows(g).into_iter().map(|r| (name(&r.label).to_string(), r.bytes)).collect::<Vec<_>>();
    assert_eq!(names(Group::FileSize), [("f.zip".to_string(), 2500), ("c.pdf".to_string(), 50)]);
    assert_eq!(names(Group::FolderSize), [("e".to_string(), 2500)]);
    let legend: Vec<_> = v.types.iter().filter(|t| t.files > 0).map(|t| (t.category.name(), t.bytes)).collect();
    assert_eq!(legend, [("Documents", 50), ("Archives", 2500)]);
    let pieces: Vec<_> = v.pieces.iter().map(|p| (p.label.as_str(), p.bytes)).collect();
    assert_eq!(pieces, [("e", 2500), ("this folder", 50)]);
    assert_eq!(v.largest_file.as_ref().map(|f| name(&f.0)), Some("a.mp4"), "the status band stays on the whole scan");

    d.navigate(nav::Move::ThisFolderOnly(true));
    let v = d.view();
    assert_eq!(names(Group::FolderSize), [("e".to_string(), 2500)], "the old view is unchanged");
    let only = |g: Group| v.rows(g).into_iter().map(|r| (name(&r.label).to_string(), r.bytes)).collect::<Vec<_>>();
    assert_eq!(only(Group::FileSize), [("c.pdf".to_string(), 50)]);
    assert!(only(Group::FolderSize).is_empty());
    assert_eq!(v.types.iter().map(|t| t.bytes).sum::<u64>(), 50);
    assert!(v.this_folder_only);
}

/// Runs `f` with a page context whose other add-ons own nothing.
fn with_page<T>(f: impl FnOnce(&mut PageContext) -> T) -> T {
    let (settings, sample) = (crate::settings::Settings::default(), Snapshot::default());
    f(&mut PageContext::new(&settings, &sample))
}

#[test]
fn the_item_actions_show_open_or_copy_the_item_or_say_why_not() {
    let t = TempTree::new("addon-actions");
    t.file("games/big.pak", 4000);
    t.file("games/gone.pak", 1000);
    let (dir, pak, gone) = (t.0.join("games"), t.0.join("games").join("big.pak"), t.0.join("games").join("gone.pak"));
    let mut d = DiskSpace::default();
    let events = vec![Event::File(pak.clone(), 4000), Event::File(gone.clone(), 1000)];
    d.scan_with(t.0.clone(), None, move |job| deliver(job, events));
    wait_idle(&d);
    std::fs::remove_file(&gone).unwrap();
    let file = nav::Key::File(pak.clone());

    let said = with_page(|page| {
        d.act(actions::Verb::ShowInExplorer, &file, page);
        d.act(actions::Verb::CopyPath, &file, page);
        d.act(actions::Verb::ShowInExplorer, &nav::Key::File(gone.clone()), page);
        d.act(actions::Verb::CopyPath, &nav::Key::Rest, page);
        std::mem::take(&mut page.actions)
    });
    assert!(matches!(&said[0], Action::ShowInExplorer(p) if *p == pak), "Explorer, the file selected");
    assert!(matches!(&said[1], Action::Copy(s) if *s == pak.display().to_string()));
    assert!(matches!(&said[2], Action::Say(s) if s == "This item may have moved or been deleted since the scan."));
    assert!(matches!(&said[3], Action::Say(s) if s == "Item unavailable"));
    assert_eq!(said.len(), 4);

    with_page(|page| {
        d.act(actions::Verb::OpenFolder, &file, page);
        assert!(page.actions.is_empty(), "Open the folder stays in Disk Space");
    });
    let level = d.view().crumbs.last().map(|c| c.path.clone());
    assert_eq!(level, Some(dir.clone()), "it enters the folder holding the file");
    let this = with_page(|page| {
        d.act(actions::Verb::ShowInExplorer, &nav::Key::ThisFolder, page);
        std::mem::take(&mut page.actions)
    });
    assert!(matches!(&this[0], Action::ShowInExplorer(p) if *p == dir), "this folder is the level's folder");
}

#[test]
fn an_item_in_an_installed_apps_folder_links_to_its_row_there() {
    let d = DiskSpace::default();
    let events = vec![file("R:\\Apps\\Tool\\tool.exe", 2500), file("R:\\setup.exe", 400)];
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    wait_idle(&d);
    let (settings, sample) = (crate::settings::Settings::default(), Snapshot::default());
    let owns = |p: &Path| p.starts_with("R:\\Apps\\Tool").then(|| crate::platform::Owner { addon: "installed-apps", page: "Installed Apps", label: "Tool".into() });
    let mut page = PageContext::new(&settings, &sample);
    page.owners = Some(&owns);
    let tool = nav::Key::File(PathBuf::from("R:\\Apps\\Tool\\tool.exe"));
    assert_eq!(d.owner(&tool, &page).map(|o| o.label), Some("Tool".into()));
    assert_eq!(d.owner(&nav::Key::File(PathBuf::from("R:\\setup.exe")), &page), None);
    assert_eq!(d.owner(&tool, &PageContext::new(&settings, &sample)), None, "Installed Apps off: no link");
    d.show_owner(&tool, &mut page);
    assert!(matches!(&page.actions[..], [Action::ShowIn { addon: "installed-apps", path }] if *path == PathBuf::from("R:\\Apps\\Tool\\tool.exe")));
}

#[test]
fn navigating_moves_through_the_measured_tree() {
    let mut d = DiskSpace::default();
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\d")));
    assert!(d.view().crumbs.is_empty(), "nothing measured, nowhere to go");
    let events = vec![file("R:\\d\\e\\f.zip", 2500), file("R:\\setup.exe", 400)];
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    wait_idle(&d);
    d.navigate(nav::Move::Open(PathBuf::from("R:\\d\\e\\f.zip")));
    let crumbs: Vec<_> = d.view().crumbs.into_iter().map(|c| c.label).collect();
    assert_eq!(crumbs, ["R:\\", "d", "e"]);
    d.navigate(nav::Move::Parent);
    assert_eq!(d.view().crumbs.len(), 2);
}

#[test]
fn the_group_chosen_is_remembered() {
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    assert_eq!(d.group, Group::FileSize, "the page opens on File Size");
    d.load_settings(&serde_json::json!({ "target": "C:\\", "newer": 1 }));
    d.group = Group::FolderSize;
    let saved = d.settings();
    assert_eq!(saved, serde_json::json!({ "target": "C:\\", "group": "folder-size", "newer": 1 }));
    let mut again = DiskSpace::default();
    again.load_settings(&saved);
    again.start(&ctx);
    assert_eq!(again.group, Group::FolderSize);
    let mut fresh = DiskSpace::default();
    fresh.group = Group::FileType;
    assert_eq!(fresh.settings(), serde_json::json!({ "group": "file-type" }), "remembered even before a target is chosen");
    let labels: Vec<_> = Group::ALL.iter().map(|g| g.label()).collect();
    assert_eq!(labels, ["File Size", "File Type", "Folder Size"]);
}

#[test]
fn the_view_chosen_is_remembered() {
    let mut d = DiskSpace::default();
    assert_eq!(d.drawing, Drawing::Balls, "the page opens on Balls");
    d.load_settings(&serde_json::json!({ "target": "C:\\", "newer": 1 }));
    d.drawing = Drawing::Block;
    let saved = d.settings();
    assert_eq!(saved, serde_json::json!({ "target": "C:\\", "view": "block", "newer": 1 }));
    let mut again = DiskSpace::default();
    again.load_settings(&saved);
    assert_eq!(again.drawing, Drawing::Block);
    again.drawing = Drawing::Balls;
    assert_eq!(again.settings(), serde_json::json!({ "target": "C:\\", "newer": 1 }), "the default is not written");
    let mut fresh = DiskSpace::default();
    fresh.drawing = Drawing::Block;
    assert_eq!(fresh.settings(), serde_json::json!({ "view": "block" }), "remembered even before a target is chosen");
    let labels: Vec<_> = Drawing::ALL.iter().map(|g| g.label()).collect();
    assert_eq!(labels, ["Balls", "Block"]);
}

#[test]
fn stopping_a_scan_keeps_what_it_measured_so_far_without_a_result_line() {
    let d = DiskSpace::default();
    let (started, wait_started) = channel();
    d.scan_with("D:\\", None, move |job| {
        deliver(job, vec![file("D:\\x\\a", 10)]);
        started.send(()).unwrap();
        while !job.cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        deliver(job, vec![file("D:\\x\\b", 999)]);
    });
    wait_started.recv().unwrap();
    d.stop_scan();
    std::thread::sleep(Duration::from_millis(20));
    let v = wait_phase(&d, Phase::Stopped);
    assert_eq!((v.running, v.files, v.bytes), (None, 1, 10));
    assert_eq!(v.root.as_deref(), Some("D:\\"));
    assert_eq!((v.finished, v.where_it_goes), (None, None));
}

fn wait_phase(d: &DiskSpace, phase: Phase) -> View {
    let t = Instant::now();
    while d.view().phase != phase && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    d.view()
}

/// A fake scanner: one file, then waits for `go`, then a folder
/// boundary (where a pause holds it), then another file.
fn two_step_scan(d: &DiskSpace) -> (std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<bool>) {
    let (go, wait_go) = channel::<()>();
    let (started, wait_started) = channel();
    let (kept_going, wait_kept_going) = channel();
    d.scan_with("C:\\", Some(1000), move |job| {
        deliver(job, vec![file("C:\\a\\x", 100)]);
        started.send(()).unwrap();
        let _ = wait_go.recv();
        let go_on = checkpoint(job);
        if go_on {
            deliver(job, vec![file("C:\\b\\y", 50)]);
        }
        kept_going.send(go_on).unwrap();
    });
    wait_started.recv().unwrap();
    (go, wait_kept_going)
}

#[test]
fn pause_holds_the_scanner_at_the_next_folder_and_resume_lets_it_finish() {
    let d = DiskSpace::default();
    let (go, kept_going) = two_step_scan(&d);
    assert_eq!(d.view().phase, Phase::Running);
    d.pause_scan();
    assert_eq!(d.view().phase, Phase::PausePending, "pending until the scanner reaches a folder");
    go.send(()).unwrap();
    let v = wait_phase(&d, Phase::Paused);
    assert_eq!((v.phase, v.files), (Phase::Paused, 1));
    assert!(v.running.is_some(), "a paused scan is still under way");
    assert!(kept_going.recv_timeout(Duration::from_millis(50)).is_err(), "held while paused");
    d.resume_scan();
    assert!(kept_going.recv_timeout(Duration::from_secs(2)).unwrap());
    let v = wait_idle(&d);
    assert_eq!((v.phase, v.files), (Phase::Finished, 2));
    assert!(v.finished.is_some());
}

#[test]
fn stopping_a_paused_scan_ends_it_without_a_result_line() {
    let d = DiskSpace::default();
    let (go, kept_going) = two_step_scan(&d);
    d.pause_scan();
    go.send(()).unwrap();
    wait_phase(&d, Phase::Paused);
    d.stop_scan();
    assert!(!kept_going.recv_timeout(Duration::from_secs(2)).unwrap(), "the scanner was told to stop");
    let v = wait_phase(&d, Phase::Stopped);
    assert_eq!((v.phase, v.files, v.running, v.finished), (Phase::Stopped, 1, None, None));
    assert!(!v.phase.saveable());
}

#[test]
fn stop_asks_first_and_keep_scanning_leaves_the_scan_alone() {
    let d = &mut DiskSpace::default();
    let (go, _kept_going) = two_step_scan(d);
    d.ask_stop(None);
    let ask = d.confirm.clone().expect("a question");
    assert_eq!(ask.title, "Stop scanning C:\\?");
    assert!(ask.text.starts_with("The scan is 10% through. "), "{}", ask.text);
    d.answer_stop(false, true);
    assert_eq!(d.confirm, None);
    assert_eq!(d.view().phase, Phase::Running);
    assert!(!d.suppress_stop_confirm, "Don't ask again only counts when stopping");
    d.ask_stop(None);
    d.answer_stop(true, false);
    assert_eq!(d.view().phase, Phase::Stopping);
    drop(go);
    assert_eq!(wait_phase(d, Phase::Stopped).files, 1, "what was measured stays on screen");
}

#[test]
fn dont_ask_again_is_remembered_in_its_setting_and_stops_at_once() {
    let d = &mut DiskSpace::default();
    let (go, _kept_going) = two_step_scan(d);
    d.ask_stop(None);
    d.answer_stop(true, true);
    assert_eq!(d.settings()["diskSpaceSuppressStopConfirm"], serde_json::json!(true));
    drop(go);
    wait_phase(d, Phase::Stopped);

    let d2 = &mut DiskSpace::default();
    d2.load_settings(&d.settings());
    let (go, _kept_going) = two_step_scan(d2);
    d2.ask_stop(None);
    assert_eq!(d2.confirm, None, "not asked again");
    assert_eq!(d2.view().phase, Phase::Stopping);
    drop(go);
}

#[test]
fn the_stop_confirmation_can_be_turned_back_on_from_settings() {
    let d = &mut DiskSpace::default();
    d.load_settings(&serde_json::json!({ "diskSpaceSuppressStopConfirm": true, "newer": 1 }));
    d.ask_before_stopping(true);
    assert_eq!(d.settings(), serde_json::json!({ "newer": 1 }));
    let (go, _kept_going) = two_step_scan(d);
    d.ask_stop(None);
    assert!(d.confirm.is_some(), "asked again");
    d.answer_stop(true, false);
    drop(go);
}

#[test]
fn the_list_length_setting_sets_how_many_items_the_largest_lists_show() {
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "listLength": 3 }));
    let events = (1..=5).map(|i| file(&format!("R:\\f{i}.bin"), i * 100)).collect::<Vec<_>>();
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    let v = wait_idle(&d);
    assert_eq!(v.largest.iter().map(|(p, _)| name(p)).collect::<Vec<_>>(), ["f5.bin", "f4.bin", "f3.bin"]);
    assert_eq!(d.settings(), serde_json::json!({ "listLength": 3 }));
    d.load_settings(&serde_json::json!({ "listLength": 1000 }));
    assert_eq!(d.list_length(), MAX_LIST, "kept within its range");
    d.load_settings(&serde_json::json!({}));
    assert_eq!(d.list_length(), LARGEST);
}

#[test]
fn history_keeps_only_as_many_scans_as_the_setting_says() {
    let store = TempStore::new("addon-keep");
    let mut d = with_history(&store);
    d.load_settings(&serde_json::json!({ "keepScans": 2 }));
    for n in 1..=3 {
        d.scan_with("R:\\", None, r_scan);
        wait_saved(&d, n.min(2));
        wait_no_thread(&d);
    }
    let shelf = store.0.list();
    assert_eq!(shelf.entries.len(), 2, "the oldest is removed");
    assert_eq!(d.history(), shelf);
    assert_eq!(d.settings(), serde_json::json!({ "keepScans": 2 }));
}

fn drive(mount: &str, total: u64, free: u64) -> DiskRow {
    DiskRow { mount: mount.into(), total, free, ..DiskRow::default() }
}

#[test]
fn the_summary_tile_gives_each_drives_free_space_and_the_last_scan() {
    const GB: u64 = 1 << 30;
    let disks = [drive("C:\\", 500 * GB, 120 * GB), drive("D:\\", 2000 * GB, 1500 * GB)];
    let last = history::Summary {
        id: "x".into(),
        root_path: "C:\\".into(),
        started: 0,
        finished: 10 * 86_400 + 14 * 3600 + 2 * 60,
        total_bytes: 380 * GB,
        file_count: 1_234_567,
        unreadable_count: 0,
    };
    assert_eq!(
        tile(&disks, Some(&last), 11),
        ["C:\\   120 GB free of 500 GB", "D:\\   1,500 GB free of 2,000 GB", "Last scan: C:\\ yesterday at 14:02"]
    );
    assert_eq!(tile(&[], None, 11), ["Reading drives\u{2026}", "No scan yet"]);
}

#[test]
fn the_summary_tile_is_off_until_turned_on() {
    let mut d = DiskSpace::default();
    assert!(!d.shows_summary_tile());
    d.load_settings(&serde_json::json!({ "summaryTile": true }));
    assert!(d.shows_summary_tile());
}

fn long_finished_scan(d: &mut DiskSpace) {
    d.long_after = Some(0);
    d.scan_with("R:\\", None, r_scan);
    wait_idle(d);
    wait_phase(d, Phase::Finished);
}

#[test]
fn a_long_scan_that_ends_while_its_page_is_hidden_is_announced_once() {
    let mut d = DiskSpace::default();
    d.page_seen();
    d.page_hidden();
    long_finished_scan(&mut d);
    assert_eq!(d.notice().as_deref(), Some("Disk Space finished measuring R:\\: 5 KB in 2 files."));
    assert_eq!(d.notice(), None, "once");
}

#[test]
fn a_scan_that_ends_on_its_page_or_quickly_is_not_announced() {
    let mut d = DiskSpace::default();
    d.page_seen();
    long_finished_scan(&mut d);
    d.page_seen();
    d.page_hidden();
    assert_eq!(d.notice(), None, "seen on its page");

    let mut quick = DiskSpace::default();
    quick.scan_with("R:\\", None, r_scan);
    wait_idle(&quick);
    wait_phase(&quick, Phase::Finished);
    assert_eq!(quick.notice(), None, "a short scan");
}

#[test]
fn the_scan_flag_does_nothing_while_disk_space_is_off() {
    let (mut host, runner) = crate::addons::AddonHost::for_tests(&serde_json::json!({ "disk-space": { "enabled": false } }));
    assert!(!host.command_line(&args(&["--scan=R:\\"])).reached.contains(&"disk-space"));
    assert!(runner.launches().is_empty());
    assert!(!host.is_enabled("disk-space"));
}

#[test]
fn its_settings_survive_turning_it_off_and_on() {
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    let saved = serde_json::json!({ "target": "D:\\", "group": "file-type", "view": "block", "diskSpaceSuppressStopConfirm": true, "listLength": 12, "keepScans": 5, "summaryTile": true });
    d.load_settings(&saved);
    d.start(&ctx);
    d.stop();
    d.start(&ctx);
    assert_eq!(d.settings(), saved);
}

#[test]
fn changing_the_target_during_a_scan_asks_to_stop_it_first() {
    let drive = |m: &str| Target::parse(m);
    let d = &mut DiskSpace::default();
    d.choose_target(drive("D:\\"));
    assert_eq!(d.target, drive("D:\\"), "no scan: chosen at once");
    let (go, _kept_going) = two_step_scan(d);
    d.choose_target(drive("E:\\"));
    assert_eq!(d.target, drive("D:\\"), "not yet");
    assert!(d.confirm.is_some());
    d.answer_stop(false, false);
    assert_eq!((d.target.clone(), d.view().phase), (drive("D:\\"), Phase::Running));
    d.choose_target(drive("E:\\"));
    d.answer_stop(true, false);
    assert_eq!((d.target.clone(), d.view().phase), (drive("E:\\"), Phase::Stopping));
    drop(go);
}

fn wait_saved(d: &DiskSpace, n: usize) -> history::Shelf {
    let t = Instant::now();
    while d.history().entries.len() < n && t.elapsed() < Duration::from_secs(10) {
        std::thread::sleep(Duration::from_millis(1));
    }
    d.history()
}

fn wait_no_thread(d: &DiskSpace) {
    let t = Instant::now();
    while d.scan_threads() > 0 && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn with_history(store: &TempStore) -> DiskSpace {
    let mut d = DiskSpace::default();
    d.use_history(store.0.clone());
    d
}

fn r_scan(job: &Job) {
    deliver(job, vec![file("R:\\big\\movie.mkv", 5000), file("R:\\small\\note.txt", 100), Event::Unreadable(PathBuf::from("R:\\locked"))]);
}

#[test]
fn a_finished_scan_is_saved_to_history_and_a_stopped_one_is_not() {
    let store = TempStore::new("addon-save");
    let d = with_history(&store);
    assert_eq!(d.history(), history::Shelf::default());
    d.scan_with("R:\\", None, r_scan);
    let shelf = wait_saved(&d, 1);
    assert_eq!(shelf.entries.len(), 1);
    let e = &shelf.entries[0];
    assert_eq!((e.root_path.as_str(), e.total_bytes, e.file_count, e.unreadable_count), ("R:\\", 5100, 2, 1));
    assert_eq!(store.0.list(), shelf, "it is on disk");

    let (stopped, wait_stopped) = channel();
    d.scan_with("R:\\", None, move |job| {
        r_scan(job);
        while !job.cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        stopped.send(()).unwrap();
    });
    d.stop_scan();
    wait_stopped.recv().unwrap();
    wait_phase(&d, Phase::Stopped);
    std::thread::sleep(Duration::from_millis(30));
    assert_eq!(d.history().entries.len(), 1, "a stopped scan is not saved");
    assert_eq!(store.0.list().entries.len(), 1);
}

fn wait_files(d: &DiskSpace, n: u64) -> View {
    let t = Instant::now();
    while d.view().files != n && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
    d.view()
}

fn wait_threads_down_to(d: &DiskSpace, n: usize) {
    let t = Instant::now();
    while d.scan_threads() > n && t.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A fake scan of S:\ that measures one file, waits for `go`, then
/// measures a second one.
fn s_scan_waiting(d: &DiskSpace) -> std::sync::mpsc::Sender<()> {
    let (go, wait_go) = channel::<()>();
    d.scan_with("S:\\", None, move |job| {
        deliver(job, vec![file("S:\\new\\a.bin", 7)]);
        let _ = wait_go.recv();
        deliver(job, vec![file("S:\\new\\b.bin", 3)]);
    });
    wait_files(d, 1);
    go
}

#[test]
fn a_scan_started_while_the_last_one_is_finishing_is_finished_and_saved_only_by_itself() {
    let store = TempStore::new("addon-race");
    let d = with_history(&store);
    let (ending, wait_ending) = channel();
    let (release, held) = channel::<()>();
    d.gate_ending(move || {
        ending.send(()).unwrap();
        let _ = held.recv();
    });
    d.scan_with("R:\\", None, r_scan);
    wait_ending.recv().unwrap();

    let go = s_scan_waiting(&d);
    release.send(()).unwrap();
    let shelf = wait_saved(&d, 1);
    wait_threads_down_to(&d, 1);
    let v = d.view();
    assert_eq!((v.phase, v.running.as_deref(), v.root.as_deref(), v.files, v.bytes), (Phase::Running, Some("S:\\"), Some("S:\\"), 1, 7), "the new scan is still running");
    let saved: Vec<_> = shelf.entries.iter().map(|e| (e.root_path.as_str(), e.total_bytes)).collect();
    assert_eq!(saved, [("R:\\", 5100)], "only the old scan, which really finished, is saved");

    go.send(()).unwrap();
    let shelf = wait_saved(&d, 2);
    let v = wait_idle(&d);
    assert_eq!((v.phase, v.files, v.bytes), (Phase::Finished, 2, 10), "the new scan finishes itself");
    let saved: Vec<_> = shelf.entries.iter().map(|e| (e.root_path.as_str(), e.total_bytes)).collect();
    assert_eq!(saved, [("S:\\", 10), ("R:\\", 5100)]);
}

#[test]
fn results_an_old_scan_publishes_after_a_new_start_are_ignored() {
    let d = DiskSpace::default();
    let (leak, leaked) = channel();
    d.scan_with("R:\\", None, move |job| {
        r_scan(job);
        leak.send(job.clone()).unwrap();
    });
    let old = leaked.recv().unwrap();
    wait_idle(&d);
    wait_no_thread(&d);

    let go = s_scan_waiting(&d);
    deliver(&old, vec![file("S:\\new\\late.bin", 999)]);
    let v = d.view();
    assert_eq!((v.phase, v.root.as_deref(), v.files, v.bytes), (Phase::Running, Some("S:\\"), 1, 7), "the old job's results do not land on the new scan");
    assert!(old.cancelled(), "a new Start cancels every older job");

    go.send(()).unwrap();
    let v = wait_idle(&d);
    assert_eq!((v.phase, v.files, v.bytes), (Phase::Finished, 2, 10));
}

#[test]
fn turning_off_during_a_scan_ends_its_thread_and_saves_nothing() {
    let store = TempStore::new("addon-off");
    let mut d = with_history(&store);
    d.scan_with("R:\\", None, |job| {
        r_scan(job);
        while checkpoint(job) {
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    assert_eq!(d.scan_threads(), 1);
    d.stop();
    wait_no_thread(&d);
    assert_eq!(d.scan_threads(), 0, "no scan thread outlives turning off");
    assert!(store.0.list().entries.is_empty(), "the stopped scan is not saved");
    assert_eq!(d.view(), View::default(), "its tree is freed");
}

#[test]
fn a_saved_scan_loads_into_the_page() {
    let store = TempStore::new("addon-open");
    let d = with_history(&store);
    d.scan_with("R:\\", None, r_scan);
    let id = wait_saved(&d, 1).entries[0].id.clone();
    let measured = d.view();
    d.scan_with("Q:\\", None, |job| deliver(job, vec![file("Q:\\x", 1)]));
    assert_eq!(wait_idle(&d).root.as_deref(), Some("Q:\\"));

    d.open_saved(&id);
    wait_no_thread(&d);
    let v = d.view();
    assert_eq!(v, measured, "the saved scan shows as it was measured");
    assert_eq!(v.phase, Phase::Finished);
}

#[test]
fn a_saved_scan_opens_at_its_root_with_nothing_selected() {
    let store = TempStore::new("addon-open-nav");
    let mut d = with_history(&store);
    d.scan_with("R:\\", None, r_scan);
    let id = wait_saved(&d, 1).entries[0].id.clone();
    let big = PathBuf::from("R:\\big");
    d.navigate(nav::Move::Crumb(big.clone()));
    d.navigate(nav::Move::Pick(nav::Key::ThisFolder, [10.0, 10.0]));
    let v = d.view();
    assert_eq!((v.crumbs.len(), v.chip.is_some()), (2, true), "in R:\\big with its files selected");

    d.open_saved(&id);
    wait_no_thread(&d);
    let v = d.view();
    assert_eq!(v.crumbs.iter().map(|c| c.path.clone()).collect::<Vec<_>>(), [PathBuf::from("R:\\")], "back at the root, as a fresh open");
    assert_eq!(v.chip, None, "nothing selected");
}

/// Folders keep only their names (#231): every path the page shows or acts
/// on is still the full one, however deep, live and from History.
#[test]
fn deep_folders_show_and_act_on_their_full_paths_live_and_from_history() {
    let store = TempStore::new("addon-deep");
    let mut d = with_history(&store);
    // The folders exist on disk: actions check their item is still there.
    let tree = TempTree::new("addon-deep");
    let root = tree.0.clone();
    let deep = ["Users", "some one", "AppData", "Local", "Packages", "app.v2", "Cache", "a", "b", "c", "d"].iter().fold(root.clone(), |p, n| p.join(n));
    std::fs::create_dir_all(&deep).unwrap();
    let (parent, big) = (deep.parent().unwrap().to_path_buf(), deep.join("blob.bin"));
    let events = vec![Event::File(big.clone(), 9000), Event::File(parent.join("small.txt"), 10), Event::File(root.join("top.bin"), 100)];
    d.scan_with(root.clone(), None, move |job| deliver(job, events));
    let id = wait_saved(&d, 1).entries[0].id.clone();
    let text = |p: &Path| p.display().to_string();

    let check = |d: &mut DiskSpace, when: &str| {
        let v = d.view();
        assert_eq!(v.largest_folder, Some((text(&deep), 9000)), "{when}: the chip names the deepest folder in full");
        assert_eq!(v.largest_file, Some((text(&big), 9000)), "{when}");
        d.navigate(nav::Move::Crumb(parent.clone()));
        let v = d.view();
        assert_eq!(v.crumbs.last().map(|c| c.path.clone()), Some(parent.clone()), "{when}");
        assert_eq!(v.crumbs.len(), 11, "{when}: one crumb per level");
        assert_eq!(v.rows(Group::FolderSize)[0].path.as_deref(), Some(text(&deep).as_str()), "{when}");
        assert_eq!(v.rows(Group::FileSize).iter().map(|r| r.label.clone()).collect::<Vec<_>>(), [text(&big), text(&parent.join("small.txt"))], "{when}");
        let said = with_page(|page| {
            d.act(actions::Verb::CopyPath, &nav::Key::ThisFolder, page);
            d.act(actions::Verb::CopyPath, &nav::Key::Folder(deep.clone()), page);
            std::mem::take(&mut page.actions)
        });
        assert!(matches!(&said[..], [Action::Copy(a), Action::Copy(b)] if *a == text(&parent) && *b == text(&deep)), "{when}: {said:?}");
        d.navigate(nav::Move::Crumb(root.clone()));
    };
    check(&mut d, "live");
    d.scan_with("Q:\\", None, |job| deliver(job, vec![file("Q:\\x", 1)]));
    wait_idle(&d);
    d.open_saved(&id);
    wait_no_thread(&d);
    check(&mut d, "from History");
}

#[test]
fn a_saved_scan_shows_as_many_items_as_the_list_length_setting() {
    let store = TempStore::new("addon-open-length");
    let mut d = with_history(&store);
    d.load_settings(&serde_json::json!({ "listLength": 12 }));
    let events = (1..=12).map(|i| file(&format!("R:\\f{i:02}.bin"), i * 100)).collect::<Vec<_>>();
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    let id = wait_saved(&d, 1).entries[0].id.clone();
    d.open_saved(&id);
    wait_no_thread(&d);
    assert_eq!(d.view().largest.len(), 12, "as many as a live scan shows");
}

#[test]
fn the_history_folder_setting_moves_history_and_survives_turning_off_and_on() {
    let (first, second) = (TempStore::new("addon-folder-a"), TempStore::new("addon-folder-b"));
    let dir = |s: &TempStore| s.0.dir.display().to_string();
    let mut there = SpaceTree::new("Q:\\", LARGEST);
    there.add_file(Path::new("Q:\\f"), 30);
    second.0.save(&history::Snapshot::of(&there, &history::new_id(), 0, 60, history::LIMIT)).unwrap();
    let (ctx, _) = AddonContext::recording();
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "historyFolder": dir(&first) }));
    d.start(&ctx);
    assert_eq!(d.history(), history::Shelf::default(), "listed from the folder chosen");
    d.scan_with("R:\\", None, r_scan);
    wait_saved(&d, 1);
    wait_no_thread(&d);
    assert_eq!(first.0.list().entries.len(), 1, "saved in the folder chosen");

    d.set_history_folder(Some(dir(&second)));
    assert_eq!(d.history(), second.0.list(), "changing it lists History again");
    assert_eq!(d.settings(), serde_json::json!({ "historyFolder": dir(&second) }));
    d.stop();
    d.start(&ctx);
    assert_eq!(d.history(), second.0.list(), "kept when turned off and on");

    d.set_history_folder(None);
    assert!(d.settings().get("historyFolder").is_none(), "Reset to default forgets it");
    assert_eq!(d.history(), history::Shelf::default(), "tests never list the real folder");
}

#[test]
fn a_saved_scan_without_an_unreadable_count_says_some_items_could_not_be_read() {
    let store = TempStore::new("addon-open-unknown");
    let d = with_history(&store);
    d.scan_with("R:\\", None, |job| deliver(job, vec![file("R:\\a\\f", 5)]));
    let id = wait_saved(&d, 1).entries[0].id.clone();
    let path = store.0.dir.join(format!("{id}.json"));
    let mut saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    saved.as_object_mut().unwrap().remove("unreadableCount");
    std::fs::write(&path, saved.to_string()).unwrap();
    d.open_saved(&id);
    wait_no_thread(&d);
    let v = d.view();
    assert_eq!(v.root.as_deref(), Some("R:\\"), "it still loads");
    assert_eq!(v.skipped.map(|s| s.sentence), Some(text::unreadable_unknown()));
}

#[test]
fn a_saved_scan_that_cannot_be_read_is_left_out() {
    let store = TempStore::new("addon-corrupt");
    let d = with_history(&store);
    d.scan_with("R:\\", None, r_scan);
    let id = wait_saved(&d, 1).entries[0].id.clone();
    std::fs::write(store.0.dir.join(format!("{id}.json")), "garbage").unwrap();
    d.open_saved(&id);
    wait_no_thread(&d);
    let shelf = d.history();
    assert_eq!((shelf.entries.len(), shelf.left_out), (0, 1));
    assert_eq!(d.view().root.as_deref(), Some("R:\\"), "what was on screen stays");
}

#[test]
fn the_view_can_be_read_while_a_finished_scan_is_being_saved() {
    let store = TempStore::new("addon-saving");
    let d = with_history(&store);
    let (ending, wait_ending) = channel();
    let (release, held) = channel::<()>();
    d.gate_ending(move || {
        ending.send(()).unwrap();
        let _ = held.recv();
    });
    d.scan_with("R:\\", None, r_scan);
    wait_ending.recv().unwrap();
    let v = d.view();
    assert_eq!((v.phase, v.running.as_deref(), v.files, v.bytes), (Phase::Finished, None, 2, 5100), "the finished scan stays on screen while it is saved");
    release.send(()).unwrap();
    assert_eq!(wait_saved(&d, 1).entries[0].total_bytes, 5100);
}

#[test]
fn opening_a_saved_scan_keeps_the_page_until_it_is_read() {
    let store = TempStore::new("addon-open-async");
    let d = with_history(&store);
    d.scan_with("R:\\", None, r_scan);
    let id = wait_saved(&d, 1).entries[0].id.clone();
    let measured = d.view();
    d.scan_with("Q:\\", None, |job| deliver(job, vec![file("Q:\\x", 1)]));
    let shown = wait_idle(&d);
    wait_no_thread(&d);

    let (loading, wait_loading) = channel();
    let (release, held) = channel::<()>();
    d.gate_loading(move || {
        loading.send(()).unwrap();
        let _ = held.recv();
    });
    d.open_saved(&id);
    wait_loading.recv_timeout(Duration::from_secs(5)).expect("the saved scan is read in the background");
    assert_eq!(d.view(), shown, "the page keeps what it showed while the saved scan is read");
    release.send(()).unwrap();
    wait_no_thread(&d);
    assert_eq!(d.view(), measured, "then shows the saved scan");
}

#[test]
fn a_saved_scan_read_after_a_new_start_never_replaces_it() {
    let store = TempStore::new("addon-open-late");
    let d = with_history(&store);
    d.scan_with("R:\\", None, r_scan);
    let id = wait_saved(&d, 1).entries[0].id.clone();
    wait_no_thread(&d);

    let (loading, wait_loading) = channel();
    let (release, held) = channel::<()>();
    d.gate_loading(move || {
        loading.send(()).unwrap();
        let _ = held.recv();
    });
    d.open_saved(&id);
    wait_loading.recv_timeout(Duration::from_secs(5)).expect("the saved scan is read in the background");
    let go = s_scan_waiting(&d);
    release.send(()).unwrap();
    wait_threads_down_to(&d, 1);
    let v = d.view();
    assert_eq!((v.phase, v.root.as_deref(), v.files), (Phase::Running, Some("S:\\"), 1), "the scan started meanwhile stays");
    go.send(()).unwrap();
    wait_idle(&d);
}

#[test]
fn a_size_capped_saved_scan_loads_and_can_be_navigated() {
    let store = TempStore::new("addon-open-capped");
    let mut t = SpaceTree::new("R:\\", LARGEST);
    for i in 0..20u64 {
        for j in 0..20u64 {
            t.add_file(Path::new(&format!("R:\\top{i:02}\\sub{j:02}\\deep\\file.bin")), 1000 + i * 100 + j);
        }
    }
    let full = history::encode(&history::Snapshot::of(&t, &history::new_id(), 0, 60, history::LIMIT)).unwrap().len();
    let capped = history::Snapshot::of(&t, &history::new_id(), 0, 60, full / 5);
    store.0.save(&capped).unwrap();
    let mut d = with_history(&store);
    d.open_saved(&capped.id);
    wait_no_thread(&d);
    assert_eq!((d.view().bytes, d.view().files), (t.bytes(SpaceTree::ROOT), 400), "the totals stay whole");

    d.navigate(nav::Move::Open(PathBuf::from("R:\\top19")));
    let v = d.view();
    assert_eq!(v.level.first(), Some(&("R:\\top19\\sub19".to_string(), 2919)));
    d.navigate(nav::Move::Open(PathBuf::from("R:\\top19\\sub19")));
    let v = d.view();
    let crumbs: Vec<_> = v.crumbs.iter().map(|c| c.label.as_str()).collect();
    assert_eq!(crumbs, ["R:\\", "top19", "sub19"]);
    assert!(v.level.is_empty(), "its sub-folders were not saved");
    assert_eq!(v.types.iter().map(|r| r.bytes).sum::<u64>(), 2919, "its files are still counted");
}

#[test]
fn opening_the_page_shows_the_last_finished_scan_of_the_target() {
    let store = TempStore::new("addon-last");
    let tree = |root: &str, size| {
        let mut t = SpaceTree::new(root, LARGEST);
        t.add_file(&Path::new(root).join("f"), size);
        t
    };
    for (root, size, finished) in [("R:\\", 10, 100), ("R:\\", 20, 300), ("Q:\\", 30, 400)] {
        store.0.save(&history::Snapshot::of(&tree(root, size), &history::new_id(), finished - 60, finished, history::LIMIT)).unwrap();
    }
    let mut d = DiskSpace::default();
    d.load_settings(&serde_json::json!({ "target": "R:\\" }));
    d.use_history(store.0.clone());
    assert_eq!(d.view(), View::default(), "nothing shown before the page opens");
    d.show_last_of_target();
    wait_no_thread(&d);
    let v = d.view();
    assert_eq!((v.root.as_deref(), v.bytes), (Some("R:\\"), 20));
    assert!(v.finished.is_some(), "with its result line");

    d.choose_target(Some(Target::Drive("Z:\\".into())));
    d.show_last_of_target();
    wait_no_thread(&d);
    assert_eq!(d.view().root.as_deref(), Some("R:\\"), "a target never measured keeps what is shown");
    d.choose_target(Some(Target::Folder("Q:\\".into())));
    d.show_last_of_target();
    wait_no_thread(&d);
    assert_eq!(d.view().bytes, 30);
}

#[test]
fn clear_history_asks_first_and_removes_every_saved_scan() {
    let store = TempStore::new("addon-clear");
    let mut d = with_history(&store);
    d.ask_clear();
    assert_eq!(d.clear_question(), None, "nothing to clear, nothing asked");
    d.scan_with("R:\\", None, r_scan);
    wait_saved(&d, 1);
    d.ask_clear();
    assert_eq!(d.clear_question(), Some((history::CLEAR_TITLE, "One saved scan will be removed. This cannot be undone.".to_string())));
    d.answer_clear(false);
    assert_eq!((d.clear_question(), d.history().entries.len()), (None, 1), "Cancel keeps it");
    d.ask_clear();
    d.answer_clear(true);
    assert_eq!(d.history(), history::Shelf::default());
    assert_eq!(store.0.list(), history::Shelf::default());
}

/// Real folders of this PC: the project folder, and the default user
/// profile, whose "Application Data" etc. are junctions.
#[test]
#[ignore]
fn scans_real_folders_of_this_pc() {
    let project = Path::new(env!("CARGO_MANIFEST_DIR"));
    let d = DiskSpace::default();
    d.scan(project, None);
    let v = wait_idle(&d);
    println!("{}: {} in {}, largest {:?}", project.display(), text::size(v.bytes), text::files(v.files), v.largest.first());
    assert!(v.bytes > 0);
    let (largest, _) = v.largest.first().expect("a largest file");
    assert!(Path::new(largest).is_file(), "{largest} exists");
    println!("{:?}", v.rows(Group::FileType).iter().map(|r| (&r.label, text::size(r.bytes))).collect::<Vec<_>>());
    assert!(v.types.iter().any(|t| t.category == Category::Code && t.files > 0), "the project's sources count as Code");

    let profile = Path::new("C:\\Users\\Default");
    let d = DiskSpace::default();
    d.scan(profile, None);
    let v = wait_idle(&d);
    println!("{}: {} in {}, {} unreadable", profile.display(), text::size(v.bytes), text::files(v.files), v.unreadable);
    assert!(v.files > 0);
    let through_junction = v.folders.iter().chain(&v.largest).any(|(p, _)| p.contains("Application Data") || p.contains("Local Settings"));
    assert!(!through_junction, "junctions are not followed: {:?}", v.folders);
    assert!(v.unreadable == 0, "junctions are skipped, not counted as unreadable");
}

#[test]
fn the_led_strip_shows_the_share_of_used_space_only_for_drives_and_all_disks() {
    let view = |root: &str, running: bool, progress: Option<f32>, phase: Phase| View {
        root: Some(root.into()),
        running: running.then(|| root.to_string()),
        progress,
        phase,
        ..Default::default()
    };
    assert_eq!(View::default().strip_fill(), 0.0, "nothing measured yet");
    // A drive or All disks: the share of the used space counted so far.
    assert_eq!(view("C:\\", true, Some(0.25), Phase::Running).strip_fill(), 0.25);
    assert_eq!(view(target::ALL_DISKS, true, Some(0.5), Phase::Running).strip_fill(), 0.5);
    assert_eq!(view("C:\\", false, None, Phase::Finished).strip_fill(), 1.0, "a finished drive scan is full");
    assert_eq!(view("C:\\", false, None, Phase::Stopped).strip_fill(), 0.0, "a stopped one is not");
    // A folder has no known total: the strip stays empty, running or done.
    assert_eq!(view(r"C:\Users\spata\src", true, None, Phase::Running).strip_fill(), 0.0);
    assert_eq!(view(r"C:\Users\spata\src", false, None, Phase::Finished).strip_fill(), 0.0);
}

/// Benchmark (not a check): scans `MTM_BENCH_DIR` (default C:\Windows)
/// along the scanner's paths and prints files per second for each.
/// `cargo test --release --lib -- --ignored scan_speed --nocapture`
#[test]
#[ignore]
fn scan_speed() {
    let dir = PathBuf::from(std::env::var("MTM_BENCH_DIR").unwrap_or_else(|_| r"C:\Windows".into()));
    let report = |label: &str, files: u64, secs: f64| println!("{label:<36} {files:>9} files {secs:>7.2}s {:>10.0} files/s", files as f64 / secs);
    let timed = |label: String, workers: usize, real_tree: bool| {
        let mut tree = SpaceTree::new(dir.clone(), 25);
        let mut files = 0u64;
        let t = Instant::now();
        scan::walk_with(workers, &dir, &|| true, &mut |events| {
            files += events.len() as u64;
            if real_tree {
                tree.apply(events);
            }
        });
        report(&label, files, t.elapsed().as_secs_f64());
    };
    timed("warm-up (4 workers, no-op sink)".into(), 4, false);
    for workers in [1, 2, 4, 8, 16] {
        timed(format!("walk, {workers} workers, no-op sink"), workers, false);
    }
    timed(format!("walk + tree, {} workers", scan::WORKERS), scan::WORKERS, true);
    // The cost of reading each file's hard-link count (#232), no-op sink;
    // the rows above read them.
    for _ in 0..2 {
        for (label, cap) in [("links not read (before #232)", None), ("links read and deduplicated", Some(scan::LINK_CAP))] {
            let mut files = 0u64;
            let t = Instant::now();
            scan::walk_links(scan::WORKERS, cap, &dir, &|| true, &mut |events| files += events.len() as u64);
            report(label, files, t.elapsed().as_secs_f64());
        }
    }

    // The add-on's own path: its thread, checkpoints, deliver under the
    // lock, while a "UI" reads the view every frame.
    let d = DiskSpace::default();
    let t = Instant::now();
    d.measure(&Target::Folder(path_text(&dir)), target::Plan { roots: vec![path_text(&dir)], expected: None });
    while d.view().running.is_some() {
        std::thread::sleep(Duration::from_millis(16));
    }
    report("add-on scan (measure + view/16ms)", d.view().files, t.elapsed().as_secs_f64());
    let t = Instant::now();
    for _ in 0..10 {
        d.view();
    }
    println!("view() on the finished tree: {:.2} ms", t.elapsed().as_secs_f64() * 100.0);

    let d = DiskSpace::default();
    let t = Instant::now();
    d.measure(&Target::Folder(path_text(&dir)), target::Plan { roots: vec![path_text(&dir)], expected: None });
    while d.scan_threads() > 0 {
        std::thread::sleep(Duration::from_millis(16));
    }
    report("add-on scan (measure, no view)", d.view().files, t.elapsed().as_secs_f64());
}

/// A scan held open by the test: each batch handed to [`Held::deliver`]
/// is in the tree when it returns; dropping it ends the scan.
struct Held {
    batches: std::sync::mpsc::Sender<Vec<Event>>,
    delivered: std::sync::mpsc::Receiver<()>,
}

impl Held {
    fn start(d: &DiskSpace, first: Vec<Event>) -> Held {
        let (batches, incoming) = channel::<Vec<Event>>();
        let (done, delivered) = channel();
        d.scan_with("R:\\", None, move |job| {
            deliver(job, first);
            let _ = done.send(());
            for batch in incoming {
                deliver(job, batch);
                let _ = done.send(());
            }
        });
        delivered.recv().unwrap();
        Held { batches, delivered }
    }

    fn deliver(&self, batch: Vec<Event>) {
        self.batches.send(batch).unwrap();
        self.delivered.recv().unwrap();
    }
}

#[test]
fn the_view_is_computed_once_per_delivery_not_once_per_ask() {
    let mut d = DiskSpace::default();
    let scan = Held::start(&d, vec![file("R:\\d\\a.mp4", 3000), file("R:\\d\\e\\b.zip", 2000), file("R:\\c.pdf", 50)]);
    let first = d.view();
    let computed = d.view_computations();
    assert_eq!(d.view(), first);
    let _menu = d.menu(&[]);
    assert_eq!(d.view_computations(), computed, "asking again without a delivery reuses the view");

    scan.deliver(vec![file("R:\\d\\e\\f.bin", 9000)]);
    assert_eq!(d.view().largest[0].0, "R:\\d\\e\\f.bin");
    let after = d.view_computations();
    assert_eq!(after, computed + 1, "a delivery makes one new view");
    d.view();
    assert_eq!(d.view_computations(), after);

    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\d")));
    assert_eq!(d.view().crumbs.len(), 2, "entering a level makes its view");
    d.navigate(nav::Move::ThisFolderOnly(true));
    assert!(d.view().this_folder_only, "so does This Folder Only");
    d.list_len = Some(1);
    assert_eq!(d.view().largest.len(), 1, "and the list length");
    let computed = d.view_computations();
    d.view();
    assert_eq!(d.view_computations(), computed);
    drop(scan);
    let done = wait_idle(&d);
    assert!(done.finished.is_some(), "the end of the scan still shows: {done:?}");
}

#[test]
fn the_largest_files_of_a_level_are_those_of_its_whole_subtree_during_and_after_a_scan() {
    let mut d = DiskSpace::default();
    let scan = Held::start(&d, vec![file("R:\\d\\a.mp4", 300), file("R:\\d\\e\\deep\\b.zip", 200), file("R:\\x\\c.pdf", 5000)]);
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\d")));
    let largest = |v: View| v.largest;
    let s = |l: &[(&str, u64)]| l.iter().map(|(n, b)| (n.to_string(), *b)).collect::<Vec<_>>();
    assert_eq!(largest(d.view()), s(&[("R:\\d\\a.mp4", 300), ("R:\\d\\e\\deep\\b.zip", 200)]));

    scan.deliver(vec![file("R:\\d\\e\\deep\\er\\z.iso", 900), file("R:\\x\\y.bin", 10_000), file("R:\\d\\e\\small.txt", 1)]);
    assert_eq!(largest(d.view()), s(&[("R:\\d\\e\\deep\\er\\z.iso", 900), ("R:\\d\\a.mp4", 300), ("R:\\d\\e\\deep\\b.zip", 200), ("R:\\d\\e\\small.txt", 1)]));

    drop(scan);
    wait_idle(&d);
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\d\\e")));
    assert_eq!(largest(d.view()), s(&[("R:\\d\\e\\deep\\er\\z.iso", 900), ("R:\\d\\e\\deep\\b.zip", 200), ("R:\\d\\e\\small.txt", 1)]));
}

#[test]
fn entering_a_large_level_during_a_scan_stays_fast() {
    // 600 folders of 500 sub-folders, one file each: 300,000 folders
    // under R:\big, like C:\ inside All Disks.
    let mut events = Vec::new();
    for i in 0..600u64 {
        for j in 0..500u64 {
            events.push(file(&format!("R:\\big\\d{i:03}\\s{j:03}\\f.bin"), 1 + i * 1000 + j));
        }
    }
    let mut d = DiskSpace::default();
    let scan = Held::start(&d, events);
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\big")));
    // The fastest of five views, so a busy machine does not fail it.
    let mut fastest = Duration::MAX;
    for k in 0..5u64 {
        scan.deliver(vec![file(&format!("R:\\big\\d000\\new{k}.bin"), 10)]);
        let t = Instant::now();
        let v = d.view();
        fastest = fastest.min(t.elapsed());
        assert_eq!(v.largest[0].0, "R:\\big\\d599\\s499\\f.bin");
    }
    // Measured against what a view cost when it walked the whole level
    // (each folder's files, merged), on the same machine and moment, so a
    // busy machine slows both: the view must take a small part of it.
    let tree = lock_state(&d.state).tree.clone().unwrap();
    let mut walk = Duration::MAX;
    for _ in 0..3 {
        let t = Instant::now();
        let mut largest = Vec::new();
        let mut folders = vec![tree.find(Path::new("R:\\big")).unwrap()];
        while let Some(n) = folders.pop() {
            largest.extend(tree.loose_files(n).into_iter().take(LARGEST));
            folders.extend(tree.children(n));
        }
        largest.sort_by(|a, b| b.1.cmp(&a.1));
        largest.truncate(LARGEST);
        walk = walk.min(t.elapsed());
        assert_eq!(largest[0].0, PathBuf::from("R:\\big\\d599\\s499\\f.bin"));
    }
    println!("a view of R:\\big: {fastest:?}; a walk of it: {walk:?}");
    assert!(fastest * 10 < walk, "a view of R:\\big took {fastest:?}, a tenth of a walk of the level is {:?}", walk / 10);
}

/// A finished scan of R:\ with a folder of two videos and a file at the root.
fn chip_scan() -> DiskSpace {
    let d = DiskSpace::default();
    let events = vec![file("R:\\v\\a.mp4", 3000), file("R:\\v\\b.mp4", 1000), file("R:\\setup.exe", 400)];
    d.scan_with("R:\\", None, move |job| deliver(job, events));
    wait_idle(&d);
    d
}

type ChipWords = Option<(String, String, u64, Option<String>)>;

/// The chip's words: name, second line, size, path.
fn chip_words(d: &DiskSpace) -> ChipWords {
    d.view().chip.map(|c| (c.name, c.detail, c.bytes, c.path))
}

fn words(name: &str, detail: &str, bytes: u64, path: Option<&str>) -> ChipWords {
    Some((name.into(), detail.into(), bytes, path.map(String::from)))
}

#[test]
fn clicking_a_piece_shows_its_chip_at_the_click_point_with_name_files_or_type_size_and_path() {
    let mut d = chip_scan();
    assert_eq!(d.view().chip, None, "nothing clicked yet");
    let v = nav::Key::Folder(PathBuf::from("R:\\v"));
    d.navigate(nav::Move::Pick(v.clone(), [120.0, 80.0]));
    let chip = d.view().chip.expect("a chip");
    assert_eq!((chip.key, chip.at), (v.clone(), Some([120.0, 80.0])), "at the click point");
    assert_eq!(chip_words(&d), words("v", "2 files", 4000, Some("R:\\v")));
    d.navigate(nav::Move::Pick(nav::Key::ThisFolder, [10.0, 20.0]));
    assert_eq!(chip_words(&d), words("this folder", "1 file", 400, Some("R:\\")), "this folder is the level's folder");
    d.navigate(nav::Move::Activate(v.clone()));
    d.navigate(nav::Move::ThisFolderOnly(true));
    d.navigate(nav::Move::Pick(nav::Key::File(PathBuf::from("R:\\v\\a.mp4")), [5.0, 5.0]));
    assert_eq!(chip_words(&d), words("a.mp4", "Media", 3000, Some("R:\\v\\a.mp4")), "a file says its type");
}

#[test]
fn the_chip_goes_on_a_click_on_empty_canvas_and_on_another_level() {
    let mut d = chip_scan();
    let v = nav::Key::Folder(PathBuf::from("R:\\v"));
    let pick = |d: &mut DiskSpace, key: &nav::Key| {
        d.navigate(nav::Move::Pick(key.clone(), [50.0, 50.0]));
        assert!(d.view().chip.is_some(), "{key:?}");
    };
    pick(&mut d, &v);
    d.navigate(nav::Move::Select(None));
    assert_eq!(d.view().chip, None, "a click on empty canvas");
    pick(&mut d, &v);
    d.navigate(nav::Move::Activate(v.clone()));
    assert_eq!(d.view().chip, None, "entering a folder");
    pick(&mut d, &nav::Key::ThisFolder);
    d.navigate(nav::Move::ThisFolderOnly(true));
    assert_eq!(d.view().chip, None, "This Folder Only");
    pick(&mut d, &nav::Key::File(PathBuf::from("R:\\v\\a.mp4")));
    d.navigate(nav::Move::Parent);
    assert_eq!(d.view().chip, None, "Parent");
    d.navigate(nav::Move::Parent);
    pick(&mut d, &nav::Key::ThisFolder);
    d.navigate(nav::Move::Crumb(PathBuf::from("R:\\")));
    assert_eq!(d.view().chip, None, "a breadcrumb");
}

#[test]
fn start_clears_the_chip() {
    let store = TempStore::new("addon-chip-start");
    let t = TempTree::new("addon-chip-start");
    t.file("games/big.pak", 4000);
    let mut d = with_history(&store);
    d.command(Command::Target(Target::Folder(t.0.display().to_string())), &[]);
    d.command(Command::Start, &[]);
    wait_idle(&d);
    let games = nav::Key::Folder(t.0.join("games"));
    d.navigate(nav::Move::Pick(games.clone(), [50.0, 50.0]));
    assert!(d.view().chip.is_some());
    d.command(Command::Start, &[]);
    let v = wait_idle(&d);
    assert!(v.pieces.iter().any(|p| p.key == games), "the new scan has the same folder");
    assert_eq!(v.chip, None);
}

#[test]
fn the_selected_item_shows_its_chip_on_its_piece_for_the_keyboard() {
    let mut d = chip_scan();
    let v = nav::Key::Folder(PathBuf::from("R:\\v"));
    d.navigate(nav::Move::Select(Some(v.clone())));
    let chip = d.view().chip.expect("the selection's chip");
    assert_eq!((chip.key, chip.at), (v, None), "no click point: drawn on its piece");
    assert_eq!(chip_words(&d), words("v", "2 files", 4000, Some("R:\\v")));
    d.navigate(nav::Move::Select(Some(nav::Key::Folder(PathBuf::from("R:\\elsewhere")))));
    assert_eq!(d.view().chip, None, "an item that is no piece of the level has no chip");
}

#[test]
fn the_chips_show_in_file_explorer_and_copy_path_are_the_item_actions() {
    let t = TempTree::new("addon-chip-actions");
    t.file("games/big.pak", 4000);
    t.file("old/gone.pak", 1000);
    let (games, old) = (t.0.join("games"), t.0.join("old"));
    let mut d = DiskSpace::default();
    let events = vec![Event::File(games.join("big.pak"), 4000), Event::File(old.join("gone.pak"), 1000)];
    d.scan_with(t.0.clone(), None, move |job| deliver(job, events));
    wait_idle(&d);
    std::fs::remove_dir_all(&old).unwrap();
    let chip_of = |d: &mut DiskSpace, dir: &Path| {
        d.navigate(nav::Move::Pick(nav::Key::Folder(dir.to_path_buf()), [1.0, 1.0]));
        d.view().chip.expect("a chip").key
    };
    let said = with_page(|page| {
        let key = chip_of(&mut d, &games);
        d.act(actions::Verb::ShowInExplorer, &key, page);
        d.act(actions::Verb::CopyPath, &key, page);
        let key = chip_of(&mut d, &old);
        d.act(actions::Verb::ShowInExplorer, &key, page);
        d.act(actions::Verb::CopyPath, &key, page);
        std::mem::take(&mut page.actions)
    });
    assert!(matches!(&said[0], Action::ShowInExplorer(p) if *p == games));
    assert!(matches!(&said[1], Action::Copy(s) if *s == games.display().to_string()));
    for gone in &said[2..] {
        assert!(matches!(gone, Action::Say(s) if s == "This item may have moved or been deleted since the scan."), "{gone:?}");
    }
    assert_eq!(said.len(), 4);
}
