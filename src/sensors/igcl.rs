//! The IGCL provider: Intel cards read through `ControlLib.dll` (Intel
//! Graphics Control Library, installed with the Intel graphics driver),
//! loaded only while the sensor service runs ([`super::vendor_gpu`]).
//! IGCL's power telemetry gives energy and activity as running counters,
//! so power and load come from two reads ([`sample`]).

use super::gpu::GpuSample;
use super::vendor_gpu::{Vendor, VendorGpu};

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "IGCL";

/// What the provider shows of Intel cards.
pub fn vendor() -> Vendor {
    Vendor { source: SOURCE, vendor_id: super::gpu::INTEL, not_reported: "IGCL does not report it", prefix: "intel", no_card: "no Intel GPU" }
}

/// One read of IGCL's power telemetry (`None` = not supported). Energy in
/// joules and activity in seconds are running counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Telemetry {
    pub time_s: Option<f64>,
    pub gpu_energy_j: Option<f64>,
    pub card_energy_j: Option<f64>,
    pub activity_s: Option<f64>,
    pub gpu_mhz: Option<f64>,
    pub vram_mhz: Option<f64>,
    pub temperature_c: Option<f64>,
    pub vram_temperature_c: Option<f64>,
}

/// A card's values from its telemetry `now` and the read before (`prev`):
/// power = energy used / time, load = busy time / time (none on the first
/// read or when the counters went back).
pub fn sample(prev: Option<&Telemetry>, now: &Telemetry) -> GpuSample {
    let rate = |counter: fn(&Telemetry) -> Option<f64>| {
        let prev = prev?;
        let dt = now.time_s? - prev.time_s?;
        let used = counter(now)? - counter(prev)?;
        (dt > 0.0 && used >= 0.0).then(|| used / dt)
    };
    let power = rate(|t| t.card_energy_j).or_else(|| rate(|t| t.gpu_energy_j));
    GpuSample {
        graphics_mhz: now.gpu_mhz,
        memory_mhz: now.vram_mhz,
        power_w: power,
        temperature_c: now.temperature_c,
        load_percent: rate(|t| t.activity_s).map(|busy| (busy * 100.0).clamp(0.0, 100.0)),
        ..Default::default()
    }
}

/// The provider: `ControlLib.dll` loaded if present, the Intel cards it
/// sees matched to DXCore's.
pub fn provider() -> VendorGpu {
    let library = imp::library();
    VendorGpu::new(vendor(), library, &super::dxcore::adapters().unwrap_or_default())
}

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

    fn at(time_s: f64, energy_j: f64, activity_s: f64) -> Telemetry {
        Telemetry { time_s: Some(time_s), gpu_energy_j: Some(energy_j), activity_s: Some(activity_s), gpu_mhz: Some(2100.0), temperature_c: Some(55.0), ..Default::default() }
    }

    #[test]
    fn power_and_load_come_from_two_reads_of_the_counters() {
        let s = sample(Some(&at(10.0, 1000.0, 4.0)), &at(12.0, 1070.0, 5.5));
        assert_eq!(s.power_w, Some(35.0), "70 J in 2 s");
        assert_eq!(s.load_percent, Some(75.0), "1.5 s busy in 2 s");
        assert_eq!(s.graphics_mhz, Some(2100.0));
        assert_eq!(s.temperature_c, Some(55.0));
        assert_eq!(s.hotspot_c, None);
    }

    #[test]
    fn the_first_read_has_clocks_but_no_power_or_load() {
        let s = sample(None, &at(10.0, 1000.0, 4.0));
        assert_eq!((s.power_w, s.load_percent), (None, None), "never shown as zero");
        assert_eq!(s.graphics_mhz, Some(2100.0));
    }

    #[test]
    fn the_whole_card_power_is_preferred_and_a_counter_that_went_back_gives_nothing() {
        let mut a = at(0.0, 0.0, 0.0);
        let mut b = at(1.0, 20.0, 0.5);
        a.card_energy_j = Some(100.0);
        b.card_energy_j = Some(150.0);
        assert_eq!(sample(Some(&a), &b).power_w, Some(50.0), "card, not GPU only");
        assert_eq!(sample(Some(&b), &a).power_w, None, "time went back");
        let restarted = at(2.0, 5.0, 0.6);
        assert_eq!(sample(Some(&b), &restarted).power_w, None, "energy went back (driver restarted): no value");
    }

    #[test]
    #[ignore = "reads this PC (no Intel GPU, no IGCL)"]
    fn on_this_pc_igcl_is_unavailable_and_nothing_stays_loaded() {
        use crate::sensors::provider::Provider;
        let mut p = provider();
        let why = p.availability().expect_err("no Intel GPU on this PC");
        println!("IGCL: {why}");
        assert!(why.contains("ControlLib.dll") || why == "no Intel GPU");
        assert!(p.read(std::time::Instant::now()).is_empty());
    }
}
