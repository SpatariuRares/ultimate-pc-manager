//! A video-card provider over a vendor library (ADLX for AMD, IGCL for
//! Intel): the library is loaded when the provider is built (the sensor
//! service is running) and unloaded when it is dropped (nobody asks for
//! sensors any more). Its cards take the keys and names DXCore gave them
//! ([`gpu::match_cards`]), so its values and reasons fill DXCore's grey
//! rows. When the library is missing nothing is loaded: the provider is
//! unavailable with the reason ("Sources on this PC") and its vendor's
//! cards get grey rows with that reason.

use std::time::Instant;

use super::gpu::{self, Adapter, GpuSample};
use super::model::{Device, Reading};
use super::provider::Provider;

/// A loaded vendor library: the cards it sees and one read of a card.
/// Dropped = the library is shut down and freed.
pub trait Library {
    /// The cards it sees, in its order.
    fn cards(&self) -> Vec<Adapter>;
    /// One read of its `card`-th card; what it cannot read stays `None`.
    fn sample(&mut self, card: usize) -> GpuSample;
    /// One read of its `card`-th card as the readings of `device`: by
    /// default the sample's rows, what it lacks unavailable for `why`. A
    /// library with its own rows or a reason per field (ADLX) overrides it.
    fn readings(&mut self, card: usize, device: &Device, source: &'static str, why: &str, at: Instant) -> Vec<Reading> {
        self.sample(card).readings(device, source, why, at)
    }
}

/// What the provider shows of its vendor.
pub struct Vendor {
    /// Its name in "Sources on this PC" / "Read from" ("ADLX").
    pub source: &'static str,
    /// The PCI vendor id of its cards ([`gpu::AMD`]).
    pub vendor_id: u32,
    /// The reason of a value the library does not give.
    pub not_reported: &'static str,
    /// The key prefix of a card DXCore did not list (`amd` -> `amd0`).
    pub prefix: &'static str,
    /// Why it is unavailable when the library sees no card ("no AMD GPU").
    pub no_card: &'static str,
}

/// The provider: a loaded library and its cards' devices, or why not.
pub struct VendorGpu {
    vendor: Vendor,
    library: Result<(Box<dyn Library>, Vec<Device>), String>,
    /// DXCore's cards, for the grey rows when the library is unavailable.
    listed: Vec<Adapter>,
}

impl VendorGpu {
    /// The provider over `library` (or why it did not load), its cards
    /// matched to the ones DXCore `listed`.
    pub fn new(vendor: Vendor, library: Result<Box<dyn Library>, String>, listed: &[Adapter]) -> Self {
        let library = library.and_then(|lib| {
            let seen = lib.cards();
            if seen.is_empty() {
                return Err(vendor.no_card.to_string());
            }
            let devices = gpu::match_cards(listed, &seen, vendor.prefix);
            Ok((lib, devices))
        });
        VendorGpu { vendor, library, listed: listed.to_vec() }
    }
}

impl Provider for VendorGpu {
    fn name(&self) -> &'static str {
        self.vendor.source
    }

    fn availability(&mut self) -> Result<(), String> {
        self.library.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    /// Only a PC with a card of its vendor lists it (no ADLX on an
    /// Intel-only PC).
    fn for_this_pc(&mut self) -> bool {
        self.listed.iter().any(|a| a.vendor_id == self.vendor.vendor_id)
    }

    /// Grey rows with `why` on DXCore's cards of its vendor (they win over
    /// DXCore's generic reason).
    fn unavailable_readings(&mut self, why: &str, at: Instant) -> Vec<Reading> {
        gpu::cards(&self.listed)
            .iter()
            .zip(&self.listed)
            .filter(|(_, a)| a.vendor_id == self.vendor.vendor_id)
            .flat_map(|(card, _)| GpuSample::default().readings(card, self.vendor.source, why, at))
            .collect()
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let Ok((lib, devices)) = &mut self.library else { return Vec::new() };
        let (source, why) = (self.vendor.source, self.vendor.not_reported);
        devices.iter().enumerate().flat_map(|(i, card)| lib.readings(i, card, source, why, at)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::gpu::{AMD, INTEL, NVIDIA};
    use crate::sensors::model::Frame;
    use std::cell::Cell;
    use std::rc::Rc;

    fn intel() -> Vendor {
        Vendor { source: "IGCL", vendor_id: INTEL, not_reported: "IGCL does not report it", prefix: "intel", no_card: "no Intel GPU" }
    }

    fn adapter(name: &str, vendor_id: u32, device_id: u32) -> Adapter {
        Adapter { name: name.into(), vendor_id, device_id }
    }

    /// This PC: an NVIDIA card and an AMD iGPU, no Intel GPU.
    fn this_pc() -> Vec<Adapter> {
        vec![adapter("NVIDIA GeForce RTX 4080", NVIDIA, 0x2704), adapter("AMD Radeon(TM) Graphics", AMD, 0x164E)]
    }

    /// DXCore's grey rows for `listed`.
    fn dxcore(listed: &[Adapter], at: Instant) -> Vec<Reading> {
        gpu::cards(listed).iter().zip(listed).flat_map(|(card, a)| GpuSample::default().readings(card, "DXCore", &gpu::no_provider(a.vendor_id), at)).collect()
    }

    fn text(frame: &Frame, id: &str) -> String {
        frame.reading(id).unwrap_or_else(|| panic!("no row {id}")).text()
    }

    /// A made-up library: fixed cards, a fixed sample, and a flag set when
    /// it is dropped (unloaded).
    struct Fake {
        cards: Vec<Adapter>,
        sample: GpuSample,
        unloaded: Rc<Cell<bool>>,
    }

    impl Library for Fake {
        fn cards(&self) -> Vec<Adapter> {
            self.cards.clone()
        }
        fn sample(&mut self, _card: usize) -> GpuSample {
            self.sample.clone()
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            self.unloaded.set(true);
        }
    }

    #[test]
    fn without_the_library_the_source_says_why_and_the_cards_keep_dxcores_grey_rows() {
        let at = Instant::now();
        let mut p = VendorGpu::new(intel(), Err("ControlLib.dll not found (no Intel graphics driver)".into()), &this_pc());
        assert_eq!(p.name(), "IGCL");
        assert_eq!(p.availability(), Err("ControlLib.dll not found (no Intel graphics driver)".into()));
        let mine = p.read(at);
        assert!(mine.is_empty(), "no rows of its own");
        let frame = Frame::build(0, &[mine, dxcore(&this_pc(), at)], Vec::new());
        assert_eq!(text(&frame, "gpu.gpu1.clock.graphics_hz"), "ADLX not available (AMD driver)");
    }

    #[test]
    fn an_intel_card_dxcore_lists_without_igcl_has_grey_rows_with_igcls_reason() {
        let at = Instant::now();
        let listed = [adapter("NVIDIA GeForce RTX 4080", NVIDIA, 0x2704), adapter("Intel(R) Arc(TM) A770 Graphics", INTEL, 0x56A0)];
        let mut p = VendorGpu::new(intel(), Err("IGCL did not start (error 0x40000001)".into()), &listed);
        let why = p.availability().unwrap_err();
        let mine = p.unavailable_readings(&why, at);
        assert!(mine.iter().all(|r| r.device.key == "gpu1"), "only its vendor's cards");
        let frame = Frame::build(0, &[mine, dxcore(&listed, at)], Vec::new());
        assert!(frame.readings().all(|r| r.value.is_none()), "never shown as zero");
        assert_eq!(text(&frame, "gpu.gpu1.clock.graphics_hz"), "IGCL did not start (error 0x40000001)");
        assert_eq!(frame.reading("gpu.gpu1.clock.graphics_hz").unwrap().source, "IGCL");
        assert_eq!(text(&frame, "gpu.gpu0.clock.graphics_hz"), "NVML not found (NVIDIA driver)");
        let intel_card = frame.devices.iter().find(|d| d.device.key == "gpu1").unwrap();
        assert_eq!(gpu::summary(intel_card), "Clock n/a   Power n/a   Memory n/a");
    }

    #[test]
    fn without_igcl_dxcore_still_names_the_missing_library() {
        let at = Instant::now();
        let listed = [adapter("Intel(R) Arc(TM) A770 Graphics", INTEL, 0x56A0)];
        let frame = Frame::build(0, &[dxcore(&listed, at)], Vec::new());
        assert_eq!(text(&frame, "gpu.gpu0.clock.graphics_hz"), "IGCL not available (Intel graphics driver)");
    }

    #[test]
    fn a_library_that_sees_no_card_makes_the_source_unavailable() {
        let fake = Fake { cards: Vec::new(), sample: GpuSample::default(), unloaded: Rc::default() };
        let unloaded = fake.unloaded.clone();
        let mut p = VendorGpu::new(intel(), Ok(Box::new(fake)), &this_pc());
        assert_eq!(p.availability(), Err("no Intel GPU".into()));
        assert!(unloaded.get(), "a library with nothing to read is not kept loaded");
    }

    #[test]
    fn a_vendors_library_is_for_this_pc_only_with_a_card_of_that_vendor() {
        let no_library = || Err::<Box<dyn Library>, String>("ControlLib.dll not found".into());
        let mut on_this_pc = VendorGpu::new(intel(), no_library(), &this_pc());
        assert!(!on_this_pc.for_this_pc(), "no Intel card here: IGCL is not listed, whatever the reason");
        let listed = [adapter("Intel(R) Arc(TM) A770 Graphics", INTEL, 0x56A0)];
        let mut with_arc = VendorGpu::new(intel(), no_library(), &listed);
        assert!(with_arc.for_this_pc(), "an Intel card without its driver still lists IGCL, with why");
    }

    #[test]
    fn the_librarys_values_fill_the_card_dxcore_listed_and_say_what_it_does_not_report() {
        let at = Instant::now();
        let listed = [adapter("NVIDIA GeForce RTX 4080", NVIDIA, 0x2704), adapter("Intel(R) Arc(TM) A770 Graphics", INTEL, 0x56A0)];
        let sample = GpuSample { graphics_mhz: Some(2400.0), power_w: Some(180.0), ..Default::default() };
        let fake = Fake { cards: vec![adapter("Intel Arc A770", INTEL, 0x56A0)], sample, unloaded: Rc::default() };
        let mut p = VendorGpu::new(intel(), Ok(Box::new(fake)), &listed);
        assert_eq!(p.availability(), Ok(()));
        let frame = Frame::build(0, &[p.read(at), dxcore(&listed, at)], Vec::new());
        assert_eq!(frame.devices.len(), 2, "one heading per card");
        let card = frame.devices.iter().find(|d| d.device.key == "gpu1").expect("DXCore's key");
        assert_eq!(card.device.name, "Intel(R) Arc(TM) A770 Graphics", "DXCore's name");
        assert_eq!(text(&frame, "gpu.gpu1.clock.graphics_hz"), "2.40 GHz");
        assert_eq!(text(&frame, "gpu.gpu1.power.board_w"), "180.0 W");
        assert_eq!(text(&frame, "gpu.gpu1.temperature.hotspot_c"), "IGCL does not report it");
        assert_eq!(frame.reading("gpu.gpu1.clock.graphics_hz").unwrap().source, "IGCL");
        assert_eq!(text(&frame, "gpu.gpu0.clock.graphics_hz"), "NVML not found (NVIDIA driver)", "other vendors untouched");
    }

    #[test]
    fn a_library_without_rows_of_its_own_keeps_the_twelve_card_rows() {
        let at = Instant::now();
        let listed = [adapter("Intel(R) Arc(TM) A770 Graphics", INTEL, 0x56A0)];
        let sample = GpuSample { fan_percent: Some(40.0), ..Default::default() };
        let fake = Fake { cards: listed.to_vec(), sample: sample.clone(), unloaded: Rc::default() };
        let mut p = VendorGpu::new(intel(), Ok(Box::new(fake)), &listed);
        let rows = p.read(at);
        let card = &gpu::cards(&listed)[0];
        assert_eq!(rows, sample.readings(card, "IGCL", "IGCL does not report it", at), "the sample's rows, unchanged");
        assert!(rows.iter().all(|r| !r.id.ends_with(".fan_rpm")), "Fan Speed is ADLX's only");
    }

    #[test]
    fn dropping_the_provider_unloads_the_library() {
        let fake = Fake { cards: vec![adapter("AMD Radeon(TM) Graphics", AMD, 0x164E)], sample: GpuSample::default(), unloaded: Rc::default() };
        let unloaded = fake.unloaded.clone();
        let mut p = VendorGpu::new(intel(), Ok(Box::new(fake)), &this_pc());
        p.read(Instant::now());
        assert!(!unloaded.get(), "kept loaded while the service runs");
        drop(p);
        assert!(unloaded.get(), "unloaded at stop");
    }
}
