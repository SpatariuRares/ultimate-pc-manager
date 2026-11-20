//! The disk test: writes a test file in the chosen folder with write-through,
//! then reads it back without Windows' cache, so both speeds are the
//! drive's. The file is deleted whatever happens: when the test completes,
//! is cancelled (also by turning the add-on off) or fails.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::engine::{Probe, MBS};
use super::{READ_SPEED, WRITE_SPEED};

/// The disk test's messages.
pub const NOT_ENOUGH_SPACE: &str = "Not enough free space to run the test.";
pub const NO_TEMP_FOLDER: &str = "Windows could not locate the temporary folder.";
const CREATE_FAILED: &str = "Unable to create the test file.";
const WRITE_FAILED: &str = "Writing the test file failed.";
const FLUSH_FAILED: &str = "Flushing the test file failed.";
const REWIND_FAILED: &str = "Rewinding the test file failed.";
const READ_FAILED: &str = "Reading the test file failed.";

/// The test file's name, before the process id (a leading dot keeps it
/// hidden).
pub const FILE_PREFIX: &str = ".ultimate-pc-manager-disk-benchmark-";

/// The prefix before the app was renamed: such files left behind are
/// deleted too.
const OLD_FILE_PREFIX: &str = ".my-task-manager-disk-benchmark-";
/// The real test: 64 chunks of 4 MB (256 MB).
pub const CHUNK: usize = 4 * 1024 * 1024;
pub const CHUNKS: usize = 64;
/// Unbuffered I/O needs buffers aligned to the drive's sector size.
const ALIGN: usize = 4096;

/// What one disk run does: in which folder, how much it writes, and how it
/// learns the folder's free space (none when unknown).
pub struct DiskJob {
    pub folder: PathBuf,
    pub chunk: usize,
    pub chunks: usize,
    pub free: FreeSpace,
}

/// How a run learns a folder's free space (none when unknown).
pub type FreeSpace = Box<dyn Fn(&Path) -> Option<u64> + Send>;

impl DiskJob {
    /// The real test (256 MB) in `folder`.
    pub fn new(folder: PathBuf) -> DiskJob {
        DiskJob { folder, chunk: CHUNK, chunks: CHUNKS, free: Box::new(free_space) }
    }
}

/// The folder the test writes in: the chosen one, else the temp folder
/// `temp` ("Windows could not locate the temporary folder." when missing).
pub fn test_folder(chosen: Option<&str>, temp: PathBuf) -> Result<PathBuf, &'static str> {
    match chosen.filter(|c| !c.is_empty()) {
        Some(c) => Ok(PathBuf::from(c)),
        None if temp.is_dir() => Ok(temp),
        None => Err(NO_TEMP_FOLDER),
    }
}

/// The bytes free for this user on the drive of `folder`.
pub fn free_space(folder: &Path) -> Option<u64> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut free = 0u64;
        // SAFETY: `wide` is a NUL-terminated path; the other outputs may be null.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, std::ptr::null_mut(), std::ptr::null_mut())
        };
        (ok != 0).then_some(free)
    }
    #[cfg(not(windows))]
    {
        let _ = folder;
        None
    }
}

/// `bytes` moved in `secs`, in MB/sec (1 MB = 1024 x 1024 bytes).
pub fn mb_per_sec(bytes: usize, secs: f64) -> f64 {
    bytes as f64 / secs / 1_048_576.0
}

/// "Writing test file" then "Reading test file": Write speed and Read
/// speed in MB/sec. Refuses to start without room for the file.
pub fn disk(probe: &Probe, job: &DiskJob) -> Result<(), String> {
    let size = (job.chunk * job.chunks) as u64;
    if (job.free)(&job.folder).is_some_and(|free| free < size) {
        return Err(NOT_ENOUGH_SPACE.into());
    }
    if let Some((write, read)) = speeds(probe, job)? {
        probe.figure(WRITE_SPEED, write, MBS);
        probe.figure(READ_SPEED, read, MBS);
    }
    Ok(())
}

/// The test file, deleted when dropped (also on an error or a panic).
struct TestFile {
    path: PathBuf,
    file: Option<File>,
}

impl Drop for TestFile {
    fn drop(&mut self) {
        self.file.take();
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Opens the file for write-through, unbuffered I/O.
fn create(path: &Path) -> std::io::Result<File> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true).write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_NO_BUFFERING, FILE_FLAG_WRITE_THROUGH};
        o.custom_flags(FILE_FLAG_WRITE_THROUGH | FILE_FLAG_NO_BUFFERING);
    }
    o.open(path)
}

/// A buffer of `len` bytes aligned to [`ALIGN`]: (storage, offset).
fn aligned(len: usize, fill: u8) -> (Vec<u8>, usize) {
    let v = vec![fill; len + ALIGN];
    let off = v.as_ptr().align_offset(ALIGN);
    (v, off)
}

/// Deletes the test files in `folder` left by processes that ended without
/// deleting theirs (killed): those whose process id is not `alive`.
fn remove_stale(folder: &Path, alive: impl Fn(u32) -> bool) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name.to_str().and_then(|n| n.strip_prefix(FILE_PREFIX).or_else(|| n.strip_prefix(OLD_FILE_PREFIX))).and_then(|p| p.parse::<u32>().ok());
        if pid.is_some_and(|pid| !alive(pid)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A process with this id is running.
fn process_alive(pid: u32) -> bool {
    use sysinfo::{Pid, ProcessesToUpdate, System};
    let pid = Pid::from_u32(pid);
    let mut sys = System::new();
    sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);
    sys.process(pid).is_some()
}

/// Writes, flushes, rewinds and reads the file: (write, read) in MB/sec,
/// or none when cancelled. First deletes the files of killed earlier runs.
fn speeds(probe: &Probe, job: &DiskJob) -> Result<Option<(f64, f64)>, String> {
    remove_stale(&job.folder, process_alive);
    let path = job.folder.join(format!("{FILE_PREFIX}{}", std::process::id()));
    let file = create(&path).map_err(|_| CREATE_FAILED.to_string())?;
    let mut test = TestFile { path, file: Some(file) };
    let f = test.file.as_mut().expect("the file is open");
    let (n, total) = (job.chunks, job.chunk * job.chunks);
    let (mut buf, off) = aligned(job.chunk, 0xA5);

    probe.phase("Writing test file");
    let t = Instant::now();
    for i in 0..n {
        if probe.cancelled() {
            return Ok(None);
        }
        f.write_all(&buf[off..off + job.chunk]).map_err(|_| WRITE_FAILED.to_string())?;
        probe.progress((i + 1) as f32 / n as f32 * 0.5);
    }
    f.sync_all().map_err(|_| FLUSH_FAILED.to_string())?;
    let write = mb_per_sec(total, t.elapsed().as_secs_f64());

    f.seek(SeekFrom::Start(0)).map_err(|_| REWIND_FAILED.to_string())?;
    probe.phase("Reading test file");
    let t = Instant::now();
    for i in 0..n {
        if probe.cancelled() {
            return Ok(None);
        }
        f.read_exact(&mut buf[off..off + job.chunk]).map_err(|_| READ_FAILED.to_string())?;
        probe.progress(0.5 + (i + 1) as f32 / n as f32 * 0.5);
    }
    let read = mb_per_sec(total, t.elapsed().as_secs_f64());
    Ok(Some((write, read)))
}

#[cfg(test)]
mod tests {
    use super::super::engine::{wait_idle, Engine, EngineState, Unit};
    use super::*;
    use std::time::Duration;

    /// An empty folder of its own under the temp folder, for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bench-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The names of the files left in `dir`.
    fn left_in(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()
    }

    /// A small disk job (2 x 4 KB) in `folder`, seeing `free` bytes free.
    fn small_job(folder: &Path, free: Option<u64>) -> DiskJob {
        DiskJob { folder: folder.to_path_buf(), chunk: 4096, chunks: 2, free: Box::new(move |_| free) }
    }

    fn run(job: DiskJob) -> EngineState {
        let mut engine = Engine::default();
        engine.start(vec![Unit::new("Disk", move |p| disk(p, &job))]);
        wait_idle(&engine)
    }

    #[test]
    fn the_disk_test_refuses_to_start_without_enough_free_space() {
        let dir = scratch("nospace");
        let s = run(small_job(&dir, Some(8191)));
        assert_eq!(s.status().as_deref(), Some("Disk test failed: Not enough free space to run the test."));
        assert!(left_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_completed_run_shows_write_and_read_speed_and_deletes_the_file() {
        let dir = scratch("complete");
        let s = run(small_job(&dir, None));
        assert_eq!(s.status().as_deref(), Some("Test complete"));
        let figures: Vec<_> = s.results("Disk").iter().map(|f| (f.name, f.unit)).collect();
        assert_eq!(figures, [("Write speed", MBS), ("Read speed", MBS)]);
        assert!(s.results("Disk").iter().all(|f| f.value > 0.0));
        assert!(left_in(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_test_file_is_deleted_after_a_cancel() {
        let dir = scratch("cancel");
        let job = small_job(&dir, None);
        let (created, saw_created) = std::sync::mpsc::channel();
        let (started, wait_started) = std::sync::mpsc::channel();
        let mut engine = Engine::default();
        engine.start(vec![Unit::new("Disk", move |p| {
            started.send(()).ok();
            while !p.cancelled() {
                std::thread::sleep(Duration::from_millis(1));
            }
            // Cancelled while it runs: the file is made, then must go.
            let job = DiskJob {
                free: Box::new(move |_| {
                    created.send(()).ok();
                    None
                }),
                ..job
            };
            disk(p, &job)
        })]);
        wait_started.recv().unwrap();
        engine.cancel();
        let s = wait_idle(&engine);
        assert_eq!(s.status().as_deref(), Some("Cancelled"));
        assert!(saw_created.try_recv().is_ok(), "the run reached the disk test");
        assert!(left_in(&dir).is_empty(), "left: {:?}", left_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_run_removes_the_test_files_left_by_ended_processes_only() {
        let dir = scratch("stale");
        for name in [format!("{FILE_PREFIX}101"), format!("{FILE_PREFIX}202"), format!("{FILE_PREFIX}x"), format!("{OLD_FILE_PREFIX}303"), "keep.txt".into()] {
            std::fs::write(dir.join(name), b"left").unwrap();
        }
        remove_stale(&dir, |pid| pid == 202);
        let mut left = left_in(&dir);
        left.sort();
        assert_eq!(left, [format!("{FILE_PREFIX}202"), format!("{FILE_PREFIX}x"), "keep.txt".into()], "202 still runs");

        // A real run cleans up after a process that no longer exists.
        let gone = format!("{FILE_PREFIX}{}", u32::MAX - 1);
        std::fs::write(dir.join(&gone), b"left").unwrap();
        assert_eq!(run(small_job(&dir, None)).status().as_deref(), Some("Test complete"));
        assert!(!left_in(&dir).contains(&gone), "left: {:?}", left_in(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_that_cannot_hold_the_file_says_so() {
        let dir = scratch("missing").join("not-there");
        let s = run(small_job(&dir, None));
        assert_eq!(s.status().as_deref(), Some("Disk test failed: Unable to create the test file."));
    }

    #[test]
    fn the_test_folder_is_the_chosen_one_or_the_temp_folder() {
        let temp = scratch("temp");
        assert_eq!(test_folder(None, temp.clone()), Ok(temp.clone()));
        assert_eq!(test_folder(Some(""), temp.clone()), Ok(temp.clone()));
        assert_eq!(test_folder(Some("D:\\Bench"), temp.clone()), Ok(PathBuf::from("D:\\Bench")));
        assert_eq!(test_folder(None, temp.join("gone")), Err("Windows could not locate the temporary folder."));
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn speeds_are_megabytes_per_second() {
        let speed = mb_per_sec(256 * 1024 * 1024, 0.5);
        assert_eq!(speed, 512.0);
        let f = super::super::engine::Figure { name: "Read speed", value: speed, unit: super::super::engine::MBS };
        assert_eq!(f.text(), "512 MB/sec");
    }
}
