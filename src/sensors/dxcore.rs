//! The DXCore provider: lists the video cards (hardware adapters with
//! Direct3D 12 graphics, so not the NPU nor the Basic Render Driver) and
//! gives each one its heading with grey rows saying why it has no values.
//! A vendor provider listed before it (NVML) fills the rows it can read.
//! It also opens the NPUs (machine-learning adapters) and reads their load,
//! temperature and clocks, or says "No NPU in this PC".

use std::time::Instant;

use super::gpu::{self, Adapter, GpuSample};
use super::model::Reading;
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "DXCore";

/// The provider. Lists the cards once, when built.
pub struct DxCore {
    listed: Result<Vec<Adapter>, String>,
    npus: imp::Npus,
}

impl DxCore {
    pub fn new() -> Self {
        DxCore { listed: adapters(), npus: imp::Npus::open() }
    }
}

/// What DXCore does not report about an NPU (power, hence interval energy).
const NOT_REPORTED: &str = "DXCore does not report it";

impl Provider for DxCore {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        self.listed.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let listed = self.listed.as_deref().unwrap_or_default();
        gpu::cards(listed)
            .iter()
            .zip(listed)
            .flat_map(|(card, a)| GpuSample::default().readings(card, SOURCE, &gpu::no_provider(a.vendor_id), at))
            .chain(self.npus.read(at))
            .collect()
    }
}

/// The video cards of this PC, in DXCore's order.
pub use imp::adapters;

#[cfg(windows)]
mod windows;
#[cfg(target_os = "linux")]
mod linux;
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
    use crate::sensors::npu;

    #[test]
    #[ignore = "reads this PC's video cards"]
    fn lists_this_pc() {
        let listed = adapters().expect("DXCore works on Windows 10 2004+");
        for a in &listed {
            println!("{:<40} {:04X}:{:04X}", a.name, a.vendor_id, a.device_id);
        }
        assert!(listed.iter().any(|a| a.vendor_id == gpu::NVIDIA && a.name.contains("RTX 4080")), "the RTX 4080");
        assert!(listed.iter().all(|a| !a.name.contains("Basic Render")), "no software adapter");
    }

    #[test]
    #[ignore = "reads this PC's NPUs"]
    fn this_pc_has_no_npu() {
        let mut provider = DxCore::new();
        let rows = provider.read(Instant::now());
        let frame = crate::sensors::Frame::build(0, &[rows], vec![]);
        let npus: Vec<String> = frame.readings().filter(|r| r.id.starts_with("npu")).map(|r| format!("{} = {}", r.id, r.text())).collect();
        println!("{npus:#?}");
        assert_eq!(npus, vec![format!("npu.present = {}", npu::NO_NPU)]);
    }
}
