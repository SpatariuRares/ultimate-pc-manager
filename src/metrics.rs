//! The rules Performance and Summary share, so the two pages read the
//! same figures the same way: the CPU speed, the combined adapters and
//! the physical disks' bytes.

use crate::perfdata::DiskLive;
use crate::sampler::NetRow;

/// The CPU speed in MHz: the base frequency times the live "% Processor
/// Performance" / 100 (the rule Performance and Summary share). `None`
/// when either input is missing.
pub fn cpu_speed_mhz(base_mhz: u32, performance_pct: f32) -> Option<f32> {
    (base_mhz > 0 && performance_pct > 0.0).then(|| base_mhz as f32 * performance_pct / 100.0)
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
