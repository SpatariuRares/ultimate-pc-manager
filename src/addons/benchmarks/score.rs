//! The score (pure): each scored test against the reference machine, where
//! the reference machine scores 100, and the overall score of a PC.
//!
//! - A figure's ratio is its value over the reference machine's value.
//! - A test's score is 100 times the geometric mean of its figures' ratios
//!   (CPU: Single core and Multi core; GPU: Compute and Graphics; Disk:
//!   Write speed and Read speed), so each figure weighs the same and twice
//!   the speed on both figures doubles the score. A run missing one of
//!   them has no score.
//! - The overall score is the geometric mean of the test scores available;
//!   Internet (it measures the connection, not the PC) and Memory have
//!   none.

use super::Test;

/// The score's name (provisional, until the app is renamed; #24).
pub const NAME: &str = "Ultimate score";

/// The sentence that names the reference machine.
pub const REFERENCE: &str =
    "100 is the reference machine: an AMD Ryzen 5 7600X with an NVIDIA GeForce RTX 4080 and a Samsung 980 PRO.";

/// The note under the score.
pub const NOTE: &str = "Score comparisons are valid only per version, per platform.";

/// The tests that have a score, in the rail's order.
pub const SCORED: [Test; 3] = [Test::Cpu, Test::Gpu, Test::Disk];

/// The reference machine's figures, the only place they are written down:
/// the median of three release-build runs of this app's tests on the
/// owner's PC (AMD Ryzen 5 7600X, NVIDIA GeForce RTX 4080, Samsung 980 PRO;
/// disk test in the temp folder), measured on 2026-11-17 (GPU Compute on
/// 2026-11-18, with our own compute shader). Changing a test's
/// work or these values changes what the score means: scores are compared
/// per version only.
const BASE: [(Test, &str, f64); 6] = [
    (Test::Cpu, super::SINGLE_CORE, 152.8),  // Mops/sec
    (Test::Cpu, super::MULTI_CORE, 1.868),   // Gop/sec
    (Test::Gpu, super::gpu::COMPUTE, 40580.0), // GFLOPS
    (Test::Gpu, super::gpu::GRAPHICS, 61.45),  // Gpixels/sec
    (Test::Disk, super::WRITE_SPEED, 3863.0), // MB/sec
    (Test::Disk, super::READ_SPEED, 5161.0),  // MB/sec
];

/// The geometric mean of `values` (none for no value).
fn geometric_mean(values: impl IntoIterator<Item = f64>) -> Option<f64> {
    let (sum, n) = values.into_iter().fold((0.0, 0u32), |(sum, n), v| (sum + v.ln(), n + 1));
    (n > 0).then(|| (sum / f64::from(n)).exp())
}

/// The score of a run of `test` that measured `figures` (name, value):
/// none for a test without a score, or when a figure is missing or not a
/// positive number.
pub fn test_score<'a>(test: Test, figures: impl IntoIterator<Item = (&'a str, f64)> + Clone) -> Option<f64> {
    let bases: Vec<_> = BASE.iter().filter(|(t, ..)| *t == test).collect();
    if bases.is_empty() {
        return None;
    }
    let mut ratios = Vec::with_capacity(bases.len());
    for &&(_, name, base) in &bases {
        let value = figures.clone().into_iter().find(|(n, _)| *n == name)?.1;
        if !(value.is_finite() && value > 0.0) {
            return None;
        }
        ratios.push(value / base);
    }
    geometric_mean(ratios).map(|r| r * 100.0)
}

/// The overall score: the geometric mean of the test scores available
/// (none while there is none).
pub fn overall(scores: impl IntoIterator<Item = f64>) -> Option<f64> {
    geometric_mean(scores.into_iter().filter(|s| s.is_finite() && *s > 0.0))
}

/// A score as shown: "123".
pub fn text(score: f64) -> String {
    format!("{score:.0}")
}

/// Where a score bar ends and where its 100 mark sits, both as fractions of
/// its width: the bar runs to 200, or further when the score is higher.
pub fn bar(score: f64) -> (f32, f32) {
    let full = score.max(200.0);
    ((score.max(0.0) / full) as f32, (100.0 / full) as f32)
}

/// The reference machine's value of the figure `name` of `test` (none for
/// a test without a score).
pub fn reference_figure(test: Test, name: &str) -> Option<f64> {
    BASE.iter().find(|(t, n, _)| *t == test && *n == name).map(|b| b.2)
}

/// One figure's score: 100 times its value over the reference machine's.
pub fn figure_score(test: Test, name: &str, value: f64) -> Option<f64> {
    reference_figure(test, name).map(|base| value / base * 100.0)
}

/// The score reaches the reference machine's 100, as shown (rounded).
pub fn meets_reference(score: f64) -> bool {
    score.round() >= 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The reference machine's figures of `test`, each times `factor`.
    fn reference(test: Test, factor: f64) -> Vec<(&'static str, f64)> {
        BASE.iter().filter(|(t, ..)| *t == test).map(|&(_, n, v)| (n, v * factor)).collect()
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 1e-9)
    }

    #[test]
    fn the_reference_machine_scores_100_and_twice_its_speed_scores_200() {
        for test in SCORED {
            assert!(close(test_score(test, reference(test, 1.0)), 100.0), "{test:?}");
            assert!(close(test_score(test, reference(test, 2.0)), 200.0), "{test:?}");
        }
    }

    #[test]
    fn the_reference_values_are_the_measured_ones() {
        let cpu = [(super::super::SINGLE_CORE, 152.8), (super::super::MULTI_CORE, 1.868)];
        assert!(close(test_score(Test::Cpu, cpu), 100.0));
        assert!(close(test_score(Test::Gpu, [("Compute", 40580.0), ("Graphics", 61.45)]), 100.0));
        assert!(close(test_score(Test::Disk, [("Write speed", 3863.0), ("Read speed", 5161.0)]), 100.0));
    }

    #[test]
    fn a_two_figure_test_scores_the_geometric_mean_of_its_ratios() {
        // Twice the reference on one figure, half on the other: 100.
        assert!(close(test_score(Test::Disk, [("Write speed", 3863.0 * 2.0), ("Read speed", 5161.0 / 2.0)]), 100.0));
        // Four times on one, the reference on the other: 200.
        assert!(close(test_score(Test::Gpu, [("Compute", 40580.0 * 4.0), ("Graphics", 61.45)]), 200.0));
    }

    #[test]
    fn a_run_missing_a_figure_or_a_test_without_a_score_has_none() {
        assert_eq!(test_score(Test::Gpu, [("Compute", 40580.0)]), None);
        assert_eq!(test_score(Test::Disk, [("Write speed", 0.0), ("Read speed", 5161.0)]), None);
        assert_eq!(test_score(Test::Internet, [("Download", 500.0), ("Upload", 50.0)]), None);
        assert_eq!(test_score(Test::Memory, [("Copy speed", 20.0)]), None);
    }

    #[test]
    fn the_overall_score_is_the_geometric_mean_of_the_scores_available() {
        assert!(close(overall([100.0, 400.0]), 200.0));
        assert!(close(overall([50.0, 200.0, 100.0]), 100.0));
        assert!(close(overall([150.0]), 150.0));
        assert_eq!(overall([]), None);
    }

    #[test]
    fn a_figure_scores_against_the_reference_figure_and_tests_without_a_score_have_none() {
        assert_eq!(reference_figure(Test::Cpu, super::super::SINGLE_CORE), Some(152.8));
        assert_eq!(reference_figure(Test::Disk, super::super::READ_SPEED), Some(5161.0));
        assert_eq!(reference_figure(Test::Internet, super::super::internet::DOWNLOAD), None);
        assert_eq!(reference_figure(Test::Memory, super::super::COPY_SPEED), None);
        assert!(close(figure_score(Test::Cpu, super::super::SINGLE_CORE, 305.6), 200.0));
        assert_eq!(figure_score(Test::Internet, super::super::internet::DOWNLOAD, 300.0), None);
    }

    #[test]
    fn a_score_meets_the_reference_from_100_as_shown() {
        assert!(meets_reference(100.0));
        assert!(meets_reference(99.5), "shown as 100");
        assert!(!meets_reference(99.4), "shown as 99");
    }

    #[test]
    fn the_bar_marks_100_and_grows_past_200() {
        assert_eq!(bar(100.0), (0.5, 0.5));
        assert_eq!(bar(50.0), (0.25, 0.5));
        assert_eq!(bar(400.0), (1.0, 0.25));
    }

    #[test]
    fn the_texts_name_the_score_the_reference_machine_and_the_version_note() {
        assert_eq!(NAME, "Ultimate score");
        assert_eq!(text(123.4), "123");
        assert!(REFERENCE.contains("AMD Ryzen 5 7600X") && REFERENCE.contains("RTX 4080") && REFERENCE.contains("Samsung 980 PRO"));
        assert_eq!(NOTE, "Score comparisons are valid only per version, per platform.");
    }
}
