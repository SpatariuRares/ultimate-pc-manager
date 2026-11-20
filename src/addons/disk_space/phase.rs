//! The scan's phases (pure): Idle, Running, Pause pending, Paused,
//! Stopping, Finished, Stopped, moved by what the user and the scanner
//! do; and the words of the Stop confirmation.

/// Where a scan is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Idle,
    Running,
    /// Pause was asked; the scanner has not reached a folder boundary yet.
    PausePending,
    Paused,
    /// Stop was asked; the scanner has not noticed yet.
    Stopping,
    Finished,
    Stopped,
}

/// What moves a scan from one phase to the next.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Start,
    Pause,
    /// The scanner reached a folder boundary and waits.
    Parked,
    Resume,
    Stop,
    /// The scanner's walk returned.
    Ended,
}

impl Phase {
    /// The phase after `step`; a step that makes no sense here changes nothing.
    pub fn after(self, step: Step) -> Phase {
        use Phase::*;
        match (self, step) {
            (Idle | Finished | Stopped, Step::Start) => Running,
            (Running, Step::Pause) => PausePending,
            (PausePending, Step::Parked) => Paused,
            (PausePending | Paused, Step::Resume) => Running,
            (Running | PausePending | Paused, Step::Stop) => Stopping,
            (Running | PausePending | Paused, Step::Ended) => Finished,
            (Stopping, Step::Ended) => Stopped,
            (phase, _) => phase,
        }
    }

    /// A scan is under way (running, pausing, paused or stopping).
    pub fn active(self) -> bool {
        matches!(self, Phase::Running | Phase::PausePending | Phase::Paused | Phase::Stopping)
    }

    /// Only a scan that ran to its end goes to History.
    pub fn saveable(self) -> bool {
        self == Phase::Finished
    }

    /// The status strip's word: Idle, Paused, Partial and Finished, or the
    /// step under way.
    pub fn label(self) -> &'static str {
        match self {
            Phase::Idle => "Idle",
            Phase::Running => "Measuring\u{2026}",
            Phase::PausePending => "Pause pending",
            Phase::Paused => "Paused",
            Phase::Stopping => "Stopping\u{2026}",
            Phase::Finished => "Finished",
            Phase::Stopped => "Partial",
        }
    }
}

/// The Stop confirmation's title: "Stop scanning C:\?".
pub fn stop_title(root: &str) -> String {
    format!("Stop scanning {root}?")
}

/// The Stop confirmation's text, with how far the scan is ("64%"; a
/// folder's total is not known in advance, so it is "partway" there).
pub fn stop_text(done: Option<f32>) -> String {
    let through = done.map_or_else(|| "partway".to_string(), |f| format!("{:.0}%", (f * 100.0).floor()));
    format!("The scan is {through} through. Stopping keeps what has been measured so far on screen, but does not save it to history.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use Phase::*;

    fn run(steps: &[Step]) -> Phase {
        steps.iter().fold(Idle, |p, s| p.after(*s))
    }

    #[test]
    fn a_scan_that_runs_to_its_end_is_finished_and_only_that_is_saved() {
        let p = run(&[Step::Start, Step::Ended]);
        assert_eq!(p, Finished);
        assert!(p.saveable());
        for p in [Idle, Running, PausePending, Paused, Stopping, Stopped] {
            assert!(!p.saveable(), "{p:?} is not saved");
        }
    }

    #[test]
    fn pause_is_pending_until_the_scanner_parks_and_resume_runs_again() {
        assert_eq!(run(&[Step::Start, Step::Pause]), PausePending);
        assert_eq!(run(&[Step::Start, Step::Pause, Step::Parked]), Paused);
        assert_eq!(run(&[Step::Start, Step::Pause, Step::Parked, Step::Resume]), Running);
        assert_eq!(run(&[Step::Start, Step::Pause, Step::Resume]), Running, "resumed before it parked");
        assert_eq!(run(&[Step::Start, Step::Parked]), Running, "parking without a pause asked changes nothing");
        assert_eq!(run(&[Step::Pause, Step::Resume]), Idle, "nothing to pause");
    }

    #[test]
    fn a_stopped_scan_is_stopping_until_the_scanner_ends_and_is_not_saved() {
        assert_eq!(run(&[Step::Start, Step::Stop]), Stopping);
        let p = run(&[Step::Start, Step::Pause, Step::Parked, Step::Stop, Step::Ended]);
        assert_eq!(p, Stopped);
        assert!(!p.saveable());
        assert_eq!(run(&[Step::Start, Step::Stop, Step::Resume, Step::Ended]), Stopped, "no way back once stopping");
        assert_eq!(run(&[Step::Start, Step::Stop, Step::Ended, Step::Start]), Running, "a new scan starts again");
    }

    #[test]
    fn the_status_strip_says_what_the_scan_does() {
        let words: Vec<_> = [Idle, Running, PausePending, Paused, Stopping, Finished, Stopped].iter().map(|p| p.label()).collect();
        assert_eq!(words, ["Idle", "Measuring\u{2026}", "Pause pending", "Paused", "Stopping\u{2026}", "Finished", "Partial"]);
        assert!(Paused.active() && Stopping.active() && !Finished.active() && !Idle.active());
    }

    #[test]
    fn the_stop_confirmation_says_how_far_the_scan_is() {
        assert_eq!(stop_title("C:\\"), "Stop scanning C:\\?");
        assert_eq!(
            stop_text(Some(0.649)),
            "The scan is 64% through. Stopping keeps what has been measured so far on screen, but does not save it to history."
        );
        assert!(stop_text(None).starts_with("The scan is partway through. "));
    }
}
