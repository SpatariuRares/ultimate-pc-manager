//! Notifications ("toasts") through the notification area (Windows):
//! [`show`] hands a title and a text to one background thread, which adds
//! the app's icon to the notification area, shows the notification, and
//! removes the icon (and ends) after [`IDLE`] without a new one, or at once
//! on [`shutdown`] (the app quits). Windows shows such a notification as a
//! toast and keeps it in the notification centre while the icon lives.
//!
//! Only the real executor ([`super::SystemRunner`]) calls it; tests use the
//! recording executor, which shows nothing.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long the icon stays in the notification area after the last
/// notification (the thread then ends).
#[allow(dead_code)]
pub const IDLE: Duration = Duration::from_secs(60);

/// How long [`shutdown`] waits for the thread to remove the icon.
#[allow(dead_code)]
pub const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// Shows a notification without waiting for it.
pub fn show(title: &str, body: &str) -> Result<(), String> {
    system::show(title, body)
}

/// Removes the app's icon from the notification area now, when one is
/// shown, waiting at most [`SHUTDOWN_WAIT`] (the app quits: a thread
/// killed with the process would leave a ghost icon).
pub fn shutdown() {
    system::shutdown()
}

/// `s` in a fixed UTF-16 buffer, cut to fit with its NUL between whole
/// characters (a surrogate pair is never split).
#[allow(dead_code)]
fn fill<const N: usize>(buf: &mut [u16; N], s: &str) {
    let mut units: Vec<u16> = Vec::with_capacity(N);
    for c in s.chars() {
        if units.len() + c.len_utf16() > N - 1 {
            break;
        }
        let mut pair = [0u16; 2];
        units.extend_from_slice(c.encode_utf16(&mut pair));
    }
    buf[..units.len()].copy_from_slice(&units);
    buf[units.len()] = 0;
}

/// The app's icon in the notification area, owned by the notification
/// thread (a seam: Windows' `Shell_NotifyIconW` in the app, a recorder in
/// tests). Dropping it frees what it holds (its window).
pub trait Tray {
    /// Adds the icon (`NIM_ADD`); whether it was added.
    fn add(&mut self) -> bool;
    /// Shows a notification on the icon (`NIM_MODIFY`).
    fn notify(&mut self, title: &str, body: &str);
    /// Removes the icon (`NIM_DELETE`).
    fn remove(&mut self);
    /// Handles the thread's pending window messages.
    fn pump(&mut self) {}
}

/// A title and a text.
type Note = (String, String);

/// The running notification thread: its inbox and its handle.
struct Running {
    inbox: Sender<Note>,
    thread: JoinHandle<()>,
}

/// The notification thread, started by the first notification and ended
/// after `idle` without one, or by [`Toasts::shutdown`].
#[allow(dead_code)]
pub struct Toasts {
    running: Mutex<Option<Running>>,
    idle: Duration,
}

#[allow(dead_code)]
impl Toasts {
    pub const fn new(idle: Duration) -> Self {
        Toasts { running: Mutex::new(None), idle }
    }

    fn lock(&self) -> MutexGuard<'_, Option<Running>> {
        self.running.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Shows a notification through the running thread, or starts one that
    /// builds its icon with `tray` (on that thread; `None` = it cannot).
    pub fn show<T: Tray + 'static>(&'static self, title: &str, body: &str, tray: fn() -> Option<T>) -> Result<(), String> {
        let mut running = self.lock();
        let note = (title.to_string(), body.to_string());
        let note = match running.as_ref() {
            Some(r) => match r.inbox.send(note) {
                Ok(()) => return Ok(()),
                // Its thread ended: start another.
                Err(e) => e.0,
            },
            None => note,
        };
        let (tx, rx) = channel();
        tx.send(note).map_err(|_| "Could not queue the notification.".to_string())?;
        let thread = std::thread::Builder::new()
            .name("toast".into())
            .spawn(move || match tray() {
                Some(tray) => self.serve(rx, tray),
                None => *self.lock() = None,
            })
            .map_err(|e| format!("Could not start the notification thread: {e}"))?;
        *running = Some(Running { inbox: tx, thread });
        Ok(())
    }

    /// The thread: shows each note, then removes the icon once idle or
    /// told to stop (its inbox closed).
    fn serve<T: Tray>(&self, rx: Receiver<Note>, mut tray: T) {
        let mut added = false;
        loop {
            tray.pump();
            let next = match rx.recv_timeout(self.idle) {
                Ok(note) => Some(note),
                Err(RecvTimeoutError::Disconnected) => None,
                Err(RecvTimeoutError::Timeout) => {
                    // Ends only when no notification came in meanwhile
                    // (`show` sends while holding the lock).
                    let mut running = self.lock();
                    match rx.try_recv() {
                        Ok(note) => Some(note),
                        Err(_) => {
                            *running = None;
                            None
                        }
                    }
                }
            };
            let Some((title, body)) = next else { break };
            if !added {
                added = tray.add();
            }
            tray.notify(&title, &body);
        }
        if added {
            tray.remove();
        }
    }

    /// Ends the running thread, if any: its icon is removed before this
    /// returns, unless the thread takes longer than `wait`.
    pub fn shutdown(&self, wait: Duration) {
        // Taken out first: the thread locks it too when it goes idle.
        let Some(Running { inbox, thread }) = self.lock().take() else { return };
        drop(inbox);
        let end = Instant::now() + wait;
        while !thread.is_finished() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.is_finished() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as system;
#[cfg(target_os = "macos")]
use macos as system;
#[cfg(windows)]
use windows as system;


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering::SeqCst};

    #[test]
    fn a_cut_text_keeps_whole_characters() {
        let mut buf = [0xFFFFu16; 4];
        fill(&mut buf, "ab\u{1F600}");
        assert_eq!(buf[..3], [u16::from(b'a'), u16::from(b'b'), 0], "the emoji's two units do not fit with the NUL: it is left out whole");
        let mut buf = [0xFFFFu16; 5];
        fill(&mut buf, "ab\u{1F600}");
        assert_eq!(String::from_utf16(&buf[..4]).ok().as_deref(), Some("ab\u{1F600}"));
        assert_eq!(buf[4], 0);
        let mut buf = [0xFFFFu16; 4];
        fill(&mut buf, "\u{e8}t\u{e9}s");
        assert_eq!(String::from_utf16(&buf[..3]).ok().as_deref(), Some("\u{e8}t\u{e9}"));
        assert_eq!(buf[3], 0);
    }

    #[test]
    fn fill_tiny_and_exact_buffers() {
        let mut one = [0xFFFFu16; 1];
        fill(&mut one, "Hot");
        assert_eq!(one, [0], "a one-unit field keeps only the NUL");
        let title = "x".repeat(64);
        let mut field = [0xFFFFu16; 64];
        fill(&mut field, &title);
        assert_eq!(field[..63], [u16::from(b'x'); 63], "63 units of a 64-unit title");
        assert_eq!(field[63], 0, "then the NUL");
    }

    /// The icon of the tests: `removed` is set by its `NIM_DELETE`.
    struct Recorder;
    static ADDED: AtomicBool = AtomicBool::new(false);
    static REMOVED: AtomicBool = AtomicBool::new(false);

    impl Tray for Recorder {
        fn add(&mut self) -> bool {
            ADDED.store(true, SeqCst);
            true
        }
        fn notify(&mut self, _: &str, _: &str) {}
        fn remove(&mut self) {
            // A slow shell: the delete still lands before shutdown returns.
            std::thread::sleep(Duration::from_millis(50));
            REMOVED.store(true, SeqCst);
        }
    }

    #[test]
    fn shutdown_removes_the_icon_before_it_returns() {
        static TOASTS: Toasts = Toasts::new(IDLE);
        TOASTS.show("CPU Package", "above 90 °C", || Some(Recorder)).unwrap();
        let end = Instant::now() + Duration::from_secs(5);
        while !ADDED.load(SeqCst) {
            assert!(Instant::now() < end, "the icon was never added");
            std::thread::sleep(Duration::from_millis(5));
        }
        TOASTS.shutdown(SHUTDOWN_WAIT);
        assert!(REMOVED.load(SeqCst), "NIM_DELETE ran before shutdown returned (no ghost icon)");
        assert!(TOASTS.lock().is_none(), "no thread left running");
        TOASTS.shutdown(SHUTDOWN_WAIT); // nothing running: returns at once
    }
}
