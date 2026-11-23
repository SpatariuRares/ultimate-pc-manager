//! Linux: the kernel's per-processor time tallies (`/proc/stat`) and what
//! they say between two reads: how much of the time went to user work, to the
//! kernel, or to nothing.

use crate::fs_root::FsRoot;

/// Time tallies of one processor (or of all), in the kernel's clock ticks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Times {
    /// User and niced work.
    pub user: u64,
    /// System, interrupt and soft-interrupt work.
    pub kernel: u64,
    /// Idle and waiting for input or output.
    pub idle: u64,
    /// Time taken by a hypervisor (steal): counted in the total only.
    pub other: u64,
}

/// How a span of time was spent, in percent of the span.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub user: f64,
    pub kernel: f64,
    /// User plus kernel: everything but idle (and stolen) time.
    pub busy: f64,
}

impl Times {
    fn total(&self) -> u64 {
        self.user + self.kernel + self.idle + self.other
    }

    /// How the time since `earlier` was spent; `None` when no time passed.
    pub fn since(&self, earlier: &Times) -> Option<Usage> {
        let total = self.total().checked_sub(earlier.total()).filter(|t| *t > 0)? as f64;
        let share = |now: u64, before: u64| (now.saturating_sub(before) as f64 / total * 100.0).clamp(0.0, 100.0);
        let (user, kernel) = (share(self.user, earlier.user), share(self.kernel, earlier.kernel));
        Some(Usage { user, kernel, busy: (user + kernel).min(100.0) })
    }
}

/// The tallies of all processors and of each one, by logical number.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stat {
    pub total: Times,
    pub cores: Vec<(usize, Times)>,
}

/// The tallies in the text of `/proc/stat`; `None` without a total line.
pub fn parse(text: &str) -> Option<Stat> {
    let mut total = None;
    let mut cores = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let Some(label) = words.next().and_then(|l| l.strip_prefix("cpu")) else { continue };
        let n: Vec<u64> = words.map_while(|w| w.parse().ok()).collect();
        if n.len() < 4 {
            continue;
        }
        let at = |i: usize| n.get(i).copied().unwrap_or(0);
        // user nice system idle iowait irq softirq steal (guest time is already in user).
        let times = Times { user: at(0) + at(1), kernel: at(2) + at(5) + at(6), idle: at(3) + at(4), other: at(7) };
        if label.is_empty() {
            total = Some(times);
        } else if let Ok(i) = label.parse::<usize>() {
            cores.push((i, times));
        }
    }
    cores.sort_by_key(|c| c.0);
    Some(Stat { total: total?, cores })
}

/// The tallies now, from `proc/stat` under `root`.
pub fn read(root: &FsRoot) -> Option<Stat> {
    parse(&root.read("proc/stat")?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEFORE: &str = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 60 0 20 400 20 0 0 0 0 0\ncpu1 40 0 30 400 30 0 0 0 0 0\nintr 12345\n";
    const AFTER: &str = "cpu  160 10 90 850 60 5 5 0 0 0\ncpu0 100 0 40 420 20 0 0 0 0 0\ncpu1 70 10 60 430 40 5 5 0 0 0\nctxt 9\n";

    #[test]
    fn a_span_splits_into_user_and_kernel_time() {
        let (a, b) = (parse(BEFORE).unwrap(), parse(AFTER).unwrap());
        // 180 ticks passed: user +70 (with nice), kernel +50 (system, irq, softirq), idle +60.
        let u = b.total.since(&a.total).unwrap();
        assert_eq!((u.user.round(), u.kernel.round(), u.busy.round()), (39.0, 28.0, 67.0));
    }

    #[test]
    fn each_processor_is_kept_by_its_logical_number() {
        let s = parse(AFTER).unwrap();
        assert_eq!(s.cores.iter().map(|c| c.0).collect::<Vec<_>>(), vec![0, 1]);
        assert_eq!(s.cores[0].1, Times { user: 100, kernel: 40, idle: 440, other: 0 });
    }

    #[test]
    fn no_time_passed_is_no_answer() {
        let s = parse(BEFORE).unwrap();
        assert_eq!(s.total.since(&s.total), None);
        assert_eq!(parse("intr 1\n"), None);
    }
}
