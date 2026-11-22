//! The rules Performance and Summary share, so the two pages read the
//! same figures the same way: the CPU speed, the combined adapters and
//! the physical disks' bytes.

use crate::perfdata::DiskLive;
use crate::sampler::NetRow;

/// The CPU speed in MHz: the base frequency times the live "% Processor
/// Performance" / 100. The one rule Summary, Performance and Power & Freq
/// share. `None` when either input is unknown (missing, or not above 0).
pub fn cpu_speed_mhz(base_mhz: Option<f64>, performance_pct: Option<f64>) -> Option<f64> {
    let (base, pct) = (base_mhz.filter(|b| *b > 0.0)?, performance_pct.filter(|p| *p > 0.0)?);
    Some(base * pct / 100.0)
}

/// The CPU's base clock in MHz: the one fallback order every pipeline
/// uses. The machine's fixed facts (registry `~MHz`, or the test override)
/// come first, then the `Processor Frequency` counter, then the highest
/// clock the power API reports. A source that is missing or 0 is skipped.
pub fn base_clock_mhz(machine: Option<f64>, counter: Option<f64>, power_max: Option<f64>) -> Option<f64> {
    [machine, counter, power_max].into_iter().flatten().find(|v| *v > 0.0)
}

/// A speed in MHz as shown on the pages: "3.74 GHz".
pub fn fmt_ghz(mhz: f32) -> String {
    format!("{:.2} GHz", mhz / 1000.0)
}

/// Every adapter's current traffic combined, the way Performance's Network
/// device combines them.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Adapters {
    /// Bytes per second received over every adapter.
    pub rx_bps: f64,
    /// Bytes per second sent over every adapter.
    pub tx_bps: f64,
    /// The adapters that carried a byte since boot (0 = no adapter).
    pub count: usize,
}

/// Sums the current send and receive rates of every adapter and counts the
/// adapters that carried traffic since boot.
pub fn combine_adapters(net: &[NetRow]) -> Adapters {
    Adapters {
        rx_bps: net.iter().map(|n| n.rx_bps).sum(),
        tx_bps: net.iter().map(|n| n.tx_bps).sum(),
        count: net.iter().filter(|n| n.total_rx + n.total_tx > 0).count(),
    }
}

/// Read and write bytes per second summed over every physical disk (the
/// performance counters Performance's disk devices use); `None` when no
/// disk counter answered.
pub fn physical_disk_bps(disks: &[DiskLive]) -> Option<(f64, f64)> {
    (!disks.is_empty()).then(|| (disks.iter().map(|d| d.read_bps).sum(), disks.iter().map(|d| d.write_bps).sum()))
}

/// The Performance series that holds the read + write bytes per second of
/// all physical disks (one point per sample; NaN when a disk counter
/// failed, a gap rather than 0).
pub const DISKS_TOTAL: &str = "disks.rw";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cpu_speed_is_blank_when_the_base_clock_or_the_performance_level_is_unknown() {
        assert_eq!(cpu_speed_mhz(Some(3_400.0), Some(110.0)).map(|v| v.round()), Some(3_740.0));
        for (base, pct) in [(None, Some(110.0)), (Some(3_400.0), None), (Some(0.0), Some(110.0)), (Some(3_400.0), Some(0.0))] {
            assert_eq!(cpu_speed_mhz(base, pct), None, "{base:?} x {pct:?}");
        }
    }

    #[test]
    fn the_base_clock_falls_back_machine_then_counter_then_power_api() {
        assert_eq!(base_clock_mhz(Some(3_400.0), Some(3_500.0), Some(3_600.0)), Some(3_400.0));
        assert_eq!(base_clock_mhz(Some(0.0), Some(3_500.0), Some(3_600.0)), Some(3_500.0), "a 0 source is skipped");
        assert_eq!(base_clock_mhz(None, None, Some(3_600.0)), Some(3_600.0));
        assert_eq!(base_clock_mhz(None, Some(0.0), None), None);
    }
}
