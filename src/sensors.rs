//! The shared sensor service (platform): sensor providers, the telemetry
//! frame they fill, and Min/Max statistics. Not tied to a page: Power & Freq
//! and, later, Flight Recorder (Thermals) read it through
//! [`crate::platform::AddonContext::sensors`].

/// The telemetry frame (pure): devices, groups, readings and their merge.
pub mod model;
/// Min/Max and the last minute of every metric.
pub mod stats;
/// The provider trait and the list of the real app's providers.
pub mod provider;
/// The sensor service: one worker thread per provider while someone looks.
pub mod service;
/// The CPU provider (Windows counters).
pub mod cpu;
/// Package, core and system power (Windows Energy Meter).
pub mod energy;
/// AMD Ryzen Master Monitoring SDK: Ryzen power, temperature, voltages, FCLK.
pub mod ryzen;
/// The power plan provider (Windows power plan).
pub mod power_plan;
/// The disk provider (physical disk counters).
pub mod disk;
/// Power source, battery, thermal state and ACPI thermal zones.
pub mod power;
/// Video cards (pure): readings, heading line, card matching.
pub mod gpu;
/// NVIDIA cards (NVML, `nvml.dll` loaded only while the service runs).
pub mod nvml;
/// The list of video cards (DXCore).
pub mod dxcore;
/// A video-card provider over a vendor library (ADLX, IGCL).
pub mod vendor_gpu;
/// AMD cards (ADLX, `amdadlx64.dll` loaded only while the service runs).
pub mod adlx;
/// Intel cards (IGCL, `ControlLib.dll` loaded only while the service runs).
pub mod igcl;
/// NPUs (pure): readings, "No NPU in this PC".
pub mod npu;
#[cfg(windows)]
/// A PDH counter query (Windows performance counters).
mod pdh;
/// Firmware tables (SMBIOS): bus clock, motherboard, memory (#60).
pub mod smbios;
/// The firmware provider: Bus Clock, Motherboard and Memory (#60).
pub mod board;
/// Intel Dynamic Tuning: its row, grey until reading DTT is supported.
pub mod intel_dtt;

pub use model::Frame;
pub use service::{Interest, SensorService};
pub use stats::Stats;
