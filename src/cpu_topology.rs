//! Which logical processors are performance cores and which are efficiency
//! cores. On a hybrid processor Windows gives each core an efficiency
//! class: the highest class is the performance cores, every lower one the
//! efficiency cores. Without hybrid cores every processor is a performance
//! core.

use std::sync::OnceLock;

/// The class of every logical processor, by its index (the order of
/// [`crate::sampler::Snapshot::cpu_per_core`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoreTopology {
    /// `true` for an efficiency core.
    efficient: Vec<bool>,
}

impl CoreTopology {
    /// A topology from each logical processor's efficiency class: the
    /// highest class is performance, every lower one efficiency.
    pub fn from_efficiency_classes(classes: &[u8]) -> Self {
        let top = classes.iter().copied().max().unwrap_or(0);
        CoreTopology { efficient: classes.iter().map(|&c| c < top).collect() }
    }

    /// Whether `cores` logical processors are described here; a topology
    /// that does not match is ignored (every core counts as performance).
    fn fits(&self, cores: usize) -> bool {
        self.efficient.len() == cores
    }

    /// Whether logical processor `index` of `cores` is an efficiency core.
    fn is_efficient(&self, index: usize, cores: usize) -> bool {
        self.fits(cores) && self.efficient[index]
    }

    /// How many of `cores` logical processors are efficiency cores.
    pub fn efficiency_count(&self, cores: usize) -> usize {
        (0..cores).filter(|&i| self.is_efficient(i, cores)).count()
    }

    /// `per_core` (one value per logical processor) split into the
    /// performance cores' values and the efficiency cores', in order.
    pub fn split<T: Clone>(&self, per_core: &[T]) -> (Vec<T>, Vec<T>) {
        let n = per_core.len();
        let (mut performance, mut efficiency) = (Vec::with_capacity(n), Vec::new());
        for (i, v) in per_core.iter().enumerate() {
            if self.is_efficient(i, n) {
                efficiency.push(v.clone());
            } else {
                performance.push(v.clone());
            }
        }
        (performance, efficiency)
    }
}

/// This PC's topology, read once.
pub fn this_pc() -> &'static CoreTopology {
    static TOPOLOGY: OnceLock<CoreTopology> = OnceLock::new();
    TOPOLOGY.get_or_init(|| CoreTopology::from_efficiency_classes(&imp::efficiency_classes()))
}

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
pub use linux::{cpu_facts, cpu_name_from};
#[cfg(target_os = "macos")]
mod macos;

#[cfg(windows)]
use windows as imp;
#[cfg(target_os = "linux")]
use linux as imp;
#[cfg(target_os = "macos")]
use macos as imp;

#[cfg(test)]
mod tests {
    use super::*;

    /// A hybrid processor: logical 0-3 performance (class 1), 4-5 efficiency (class 0).
    fn hybrid() -> CoreTopology {
        CoreTopology::from_efficiency_classes(&[1, 1, 1, 1, 0, 0])
    }

    #[test]
    fn a_hybrid_processor_splits_its_cores_by_class() {
        let (p, e) = hybrid().split(&[10.0, 11.0, 12.0, 13.0, 50.0, 51.0]);
        assert_eq!(p, [10.0, 11.0, 12.0, 13.0]);
        assert_eq!(e, [50.0, 51.0]);
        assert_eq!(hybrid().efficiency_count(6), 2);
    }

    #[test]
    fn without_hybrid_cores_every_core_is_a_performance_core() {
        let uniform = CoreTopology::from_efficiency_classes(&[0; 12]);
        assert_eq!(uniform.efficiency_count(12), 0);
        let (p, e) = uniform.split(&[1.0; 12]);
        assert_eq!((p.len(), e.len()), (12, 0));
        assert_eq!(CoreTopology::default().efficiency_count(4), 0, "unknown topology");
    }

    #[test]
    fn a_topology_of_another_core_count_is_ignored() {
        let (p, e) = hybrid().split(&[1.0, 2.0]);
        assert_eq!((p, e), (vec![1.0, 2.0], vec![]));
        assert_eq!(hybrid().efficiency_count(2), 0);
    }

    /// The reference PC: 12 logical processors, none efficient.
    #[test]
    #[ignore]
    fn this_pc_has_twelve_performance_cores_and_no_efficiency_cores() {
        let cores = crate::cpu_topology::this_pc();
        let n = std::thread::available_parallelism().map_or(0, |n| n.get());
        println!("{cores:?}");
        assert_eq!(n, 12);
        assert_eq!(cores.efficient.len(), 12, "every logical processor is placed");
        assert_eq!(cores.efficiency_count(n), 0);
        assert_eq!(cores.split(&vec![0u8; n]).0.len(), 12);
    }
}
