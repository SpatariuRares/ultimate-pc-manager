//! The pure parts of the Benchmarks page: the views of its rail, the full
//! scale of a gauge, and the short texts the rail and the view show.

/// One entry of the page's rail, in order: CPU, GPU, Disk | Internet |
/// Score, then Memory at the bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum View {
    #[default]
    Cpu,
    Gpu,
    Disk,
    Internet,
    Score,
    Memory,
}

impl View {
    /// The rail, top to bottom.
    pub const RAIL: [View; 6] = [View::Cpu, View::Gpu, View::Disk, View::Internet, View::Score, View::Memory];

    /// Its label on the rail.
    pub fn label(self) -> &'static str {
        match self {
            View::Cpu => "CPU",
            View::Gpu => "GPU",
            View::Disk => "Disk",
            View::Internet => "Internet",
            View::Score => super::score::NAME,
            View::Memory => "Memory",
        }
    }

    /// Its key in the saved settings ("cpu", "gpu", ...).
    pub fn key(self) -> &'static str {
        match self {
            View::Cpu => "cpu",
            View::Gpu => "gpu",
            View::Disk => "disk",
            View::Internet => "internet",
            View::Score => "score",
            View::Memory => "memory",
        }
    }

    /// The view saved as `key` (none for a key it does not know).
    pub fn from_key(key: &str) -> Option<View> {
        View::RAIL.into_iter().find(|v| v.key() == key)
    }

    /// A separator sits above it on the rail.
    pub fn separated(self) -> bool {
        matches!(self, View::Internet | View::Score | View::Memory)
    }

    /// Whether the rail tags it "Added" (Memory, which has no score).
    pub fn added(self) -> bool {
        self == View::Memory
    }

    /// The title of its view.
    pub fn title(self) -> &'static str {
        match self {
            View::Cpu => "CPU Benchmark",
            View::Gpu => "GPU Benchmark",
            View::Disk => "Disk Benchmark",
            View::Internet => "Internet Benchmark",
            View::Score => super::score::NAME,
            View::Memory => "Memory Benchmark",
        }
    }

    /// The test it runs, when that test exists yet.
    pub fn test(self) -> Option<super::Test> {
        match self {
            View::Cpu => Some(super::Test::Cpu),
            View::Gpu => Some(super::Test::Gpu),
            View::Disk => Some(super::Test::Disk),
            View::Memory => Some(super::Test::Memory),
            View::Internet => Some(super::Test::Internet),
            View::Score => None,
        }
    }

    /// The figures its gauges show, with their units (one gauge each).
    pub fn gauges(self) -> &'static [(&'static str, super::engine::Measure)] {
        self.test().map(|t| t.figures()).unwrap_or(&[])
    }
}

/// What a view that has no test yet says.
pub const NOT_AVAILABLE: &str = "Not available yet";

/// The full scale of a gauge showing `value`: the first 1, 2 or 5 times a
/// power of ten above the value (1 for nothing measured).
pub fn full_scale(value: f64) -> f64 {
    if !value.is_finite() || value <= 0.0 {
        return 1.0;
    }
    let mut exp = value.log10().floor() as i32;
    loop {
        for m in [1.0, 2.0, 5.0] {
            // Divide for negative powers so 0.5 is exactly 5 / 10.
            let step = if exp >= 0 { m * 10f64.powi(exp) } else { m / 10f64.powi(-exp) };
            if step > value {
                return step;
            }
        }
        exp += 1;
    }
}

/// The full scale of a figure card's bar: a round number with room (15%)
/// above the largest of its value, the best run and the reference figure.
pub fn card_scale(values: &[Option<f64>]) -> f64 {
    let top = values.iter().flatten().copied().filter(|v| v.is_finite()).fold(0.0, f64::max);
    full_scale(top * 1.15)
}

/// When a run that ended at `at` happened, seen at `now` (both local
/// seconds since 1970): "today at 21:30", "yesterday at 09:05" or
/// "3 days ago".
pub fn when(at: i64, now: i64) -> String {
    let today = now.div_euclid(86_400);
    let day = at.div_euclid(86_400).min(today);
    let word = crate::addons::disk_space::text::day_word(day, today);
    if today - day > 1 {
        return word;
    }
    let d = at.rem_euclid(86_400);
    format!("{word} at {:02}:{:02}", d / 3600, (d % 3600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gauge_full_scale_is_the_next_round_number_above_the_value() {
        assert_eq!(full_scale(1234.0), 2000.0);
        assert_eq!(full_scale(12.35), 20.0);
        assert_eq!(full_scale(3.2), 5.0);
        assert_eq!(full_scale(7.0), 10.0);
        assert_eq!(full_scale(0.37), 0.5);
        assert_eq!(full_scale(4500.0), 5000.0);
    }

    #[test]
    fn a_gauge_full_scale_stays_above_a_round_value() {
        assert_eq!(full_scale(5.0), 10.0);
        assert_eq!(full_scale(100.0), 200.0);
        assert_eq!(full_scale(2000.0), 5000.0);
    }

    #[test]
    fn a_gauge_with_nothing_measured_reads_up_to_one() {
        assert_eq!(full_scale(0.0), 1.0);
        assert_eq!(full_scale(-3.0), 1.0);
        assert_eq!(full_scale(f64::NAN), 1.0);
    }

    #[test]
    fn a_card_scale_is_a_round_number_with_room_above_the_value_the_best_and_the_reference() {
        assert_eq!(card_scale(&[Some(156.2), Some(156.2), Some(152.8)]), 200.0);
        assert_eq!(card_scale(&[Some(1.912), Some(1.912), Some(1.868)]), 5.0);
        assert_eq!(card_scale(&[Some(3790.0), Some(3901.0), Some(3863.0)]), 5000.0);
        assert_eq!(card_scale(&[Some(312.0), Some(312.0), None]), 500.0, "no reference: the value and the best");
        assert_eq!(card_scale(&[None, None, None]), 1.0, "nothing measured");
    }

    #[test]
    fn the_rail_is_cpu_gpu_disk_internet_score_then_memory_added() {
        let labels: Vec<_> = View::RAIL.iter().map(|v| v.label()).collect();
        assert_eq!(labels, ["CPU", "GPU", "Disk", "Internet", "Ultimate score", "Memory"]);
        let separated: Vec<_> = View::RAIL.iter().filter(|v| v.separated()).map(|v| v.label()).collect();
        assert_eq!(separated, ["Internet", "Ultimate score", "Memory"]);
        let added: Vec<_> = View::RAIL.iter().filter(|v| v.added()).map(|v| v.label()).collect();
        assert_eq!(added, ["Memory"]);
    }

    #[test]
    fn each_test_view_has_two_gauges_and_only_cpu_gpu_disk_and_memory_run() {
        let gauges = |v: View| v.gauges().iter().map(|g| g.0).collect::<Vec<_>>();
        assert_eq!(gauges(View::Cpu), ["Single core", "Multi core"]);
        assert_eq!(gauges(View::Gpu), ["Compute", "Graphics"]);
        assert_eq!(gauges(View::Disk), ["Write speed", "Read speed"]);
        assert_eq!(gauges(View::Internet), ["Download", "Upload"]);
        assert_eq!(gauges(View::Memory), ["Copy speed"]);
        let runnable: Vec<_> = View::RAIL.iter().filter(|v| v.test().is_some()).map(|v| v.label()).collect();
        assert_eq!(runnable, ["CPU", "GPU", "Disk", "Internet", "Memory"]);
        assert_eq!(View::Gpu.gauges()[0], ("Compute", super::super::gpu::GFLOPS));
        assert_eq!(View::Gpu.gauges()[1], ("Graphics", super::super::gpu::GPIXELS));
    }

    #[test]
    fn a_run_reads_today_yesterday_or_days_ago() {
        let day = 86_400;
        let now = 20_000 * day + 22 * 3600;
        assert_eq!(when(20_000 * day + 21 * 3600 + 30 * 60, now), "today at 21:30");
        assert_eq!(when(19_999 * day + 9 * 3600 + 5 * 60, now), "yesterday at 09:05");
        assert_eq!(when(19_997 * day, now), "3 days ago");
    }
}
