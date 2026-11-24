//! Linux: NVIDIA cards read through the NVIDIA management library
//! (`libnvidia-ml.so.1`, installed with the proprietary driver). The library
//! is opened at run time; a PC without it (or without an NVIDIA card) is not
//! an error: nothing is loaded and the provider is not listed.
//!
//! The cards sit behind the [`Driver`] seam so the reading path is tested
//! with a fake; [`Library`] is the real one.

use std::ffi::{c_char, c_int, c_void, CStr};
use std::mem::transmute_copy;
use std::time::Instant;

use crate::fs_root::FsRoot;
use crate::sensors::gpu::{self, Adapter, GpuSample};
use crate::sensors::model::Reading;
use crate::sensors::nvml::{Extras, NOT_REPORTED, SOURCE};

/// The library's file name (its SONAME).
const LIBRARY: &CStr = c"libnvidia-ml.so.1";
const RTLD_NOW: c_int = 2;
/// NVIDIA's PCI vendor id as the kernel writes it.
const NVIDIA_VENDOR: &str = "0x10de";

#[link(name = "dl")]
extern "C" {
    fn dlopen(file: *const c_char, mode: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, name: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> c_int;
}

/// What the reader needs of the management library: the cards it sees and
/// one read of each, by index.
pub trait Driver {
    /// The cards, in the library's order.
    fn cards(&self) -> Vec<Adapter>;
    fn sample(&self, card: usize) -> GpuSample;
    fn extras(&self, card: usize) -> Extras;
    /// The card's own memory in bytes, `(used, total)`, when the library says.
    fn memory(&self, card: usize) -> Option<(u64, u64)>;
}

/// The provider's NVML session: a started library, or why there is none.
pub struct Reader {
    session: Result<Session, String>,
}

/// A started driver and the devices of its cards.
struct Session {
    driver: Box<dyn Driver>,
    cards: Vec<crate::sensors::model::Device>,
}

impl Reader {
    pub fn open() -> Self {
        Reader::with(Library::open().map(|l| Box::new(l) as Box<dyn Driver>))
    }

    /// A reader over `driver`, or the reason there is none.
    pub fn with(driver: Result<Box<dyn Driver>, String>) -> Self {
        Reader {
            session: driver.and_then(|driver| {
                let seen = driver.cards();
                if seen.is_empty() {
                    return Err("no NVIDIA GPU".into());
                }
                // Linux has no list of cards to match: each keeps its own key.
                let cards = gpu::match_cards(&[], &seen, "nvidia");
                Ok(Session { driver, cards })
            }),
        }
    }

    pub fn availability(&self) -> Result<(), String> {
        self.session.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    pub fn read(&self, at: Instant) -> Vec<Reading> {
        let Ok(s) = &self.session else { return Vec::new() };
        s.cards
            .iter()
            .enumerate()
            .flat_map(|(i, card)| {
                let mut readings = s.driver.sample(i).readings(card, SOURCE, NOT_REPORTED, at);
                readings.extend(s.driver.extras(i).readings(card, at));
                readings.extend(memory_readings(card, s.driver.memory(i), at));
                readings
            })
            .collect()
    }
}

/// The card's dedicated memory in use and in all (bytes), as two rows of the
/// card; grey with the reason when the library did not say.
fn memory_readings(card: &crate::sensors::model::Device, memory: Option<(u64, u64)>, at: Instant) -> Vec<Reading> {
    use crate::sensors::model::{Group, Unit};
    let why = "NVML reports no memory figures";
    [("memory.dedicated_used_bytes", "Dedicated Memory Used", memory.map(|m| m.0)), ("memory.dedicated_total_bytes", "Dedicated Memory", memory.map(|m| m.1))]
        .into_iter()
        .map(|(what, name, bytes)| Reading::measure(&gpu::metric(card, what), card.clone(), Group::General, name, Unit::Bytes, SOURCE, at).with(bytes.map(|b| b as f64), why))
        .collect()
}

/// Whether this PC has an NVIDIA card (the kernel lists its vendor), the one
/// rule for listing the provider on Linux.
pub fn has_card(fs: &FsRoot) -> bool {
    let Ok(entries) = std::fs::read_dir(fs.path("sys/class/drm")) else { return false };
    entries.flatten().any(|e| {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let is_card = name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
        is_card && fs.read(&format!("sys/class/drm/{name}/device/vendor")).is_some_and(|v| v.eq_ignore_ascii_case(NVIDIA_VENDOR))
    })
}

type Dev = *mut c_void;
type Plain = unsafe extern "C" fn() -> u32;
type Count = unsafe extern "C" fn(*mut u32) -> u32;
type Handle = unsafe extern "C" fn(u32, *mut Dev) -> u32;
type Name = unsafe extern "C" fn(Dev, *mut u8, u32) -> u32;
type Pci = unsafe extern "C" fn(Dev, *mut PciInfo) -> u32;
type Typed = unsafe extern "C" fn(Dev, u32, *mut u32) -> u32;
type Value = unsafe extern "C" fn(Dev, *mut u32) -> u32;
type Utilization = unsafe extern "C" fn(Dev, *mut [u32; 2]) -> u32;
type Codec = unsafe extern "C" fn(Dev, *mut u32, *mut u32) -> u32;
type Mask = unsafe extern "C" fn(Dev, *mut u64) -> u32;
type MemoryInfo = unsafe extern "C" fn(Dev, *mut MemoryCounts) -> u32;

const NVML_SUCCESS: u32 = 0;
const CLOCK_GRAPHICS: u32 = 0;
const CLOCK_MEM: u32 = 2;
const TEMPERATURE_GPU: u32 = 0;

/// nvmlMemory_t: the card's memory in bytes.
#[repr(C)]
struct MemoryCounts {
    total: u64,
    free: u64,
    used: u64,
}

/// nvmlPciInfo_t (only the PCI ids are read).
#[repr(C)]
#[allow(dead_code)]
struct PciInfo {
    bus_id_legacy: [u8; 16],
    domain: u32,
    bus: u32,
    device: u32,
    /// (device id << 16) | vendor id.
    pci_device_id: u32,
    pci_sub_system_id: u32,
    bus_id: [u8; 32],
}

/// The real management library: opened, initialised, shut down and closed
/// when dropped.
pub struct Library {
    lib: *mut c_void,
    shutdown: Option<Plain>,
    clock: Option<Typed>,
    max_clock: Option<Typed>,
    temperature: Option<Typed>,
    power: Option<Value>,
    power_limit: Option<Value>,
    fan: Option<Value>,
    utilization: Option<Utilization>,
    encoder: Option<Codec>,
    decoder: Option<Codec>,
    clock_reasons: Result<Mask, String>,
    pstate: Option<Value>,
    pcie_gen: Option<Value>,
    pcie_width: Option<Value>,
    pcie_max_gen: Option<Value>,
    pcie_max_width: Option<Value>,
    memory: Option<MemoryInfo>,
    handles: Vec<Dev>,
    seen: Vec<Adapter>,
}

/// A function of the library, if it has it.
///
/// # Safety
/// `T` must be the function pointer type of the symbol `name`.
unsafe fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    let f = dlsym(lib, name.as_ptr());
    (!f.is_null()).then(|| transmute_copy(&f))
}

impl Library {
    /// Opens and starts the library; `Err` says why it cannot.
    pub fn open() -> Result<Library, String> {
        // SAFETY: a NUL-terminated name.
        let lib = unsafe { dlopen(LIBRARY.as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return Err("libnvidia-ml.so.1 not found (no NVIDIA driver)".into());
        }
        // SAFETY: the signatures are NVML's (nvml.h); `l` closes the library
        // when dropped, on every early return.
        unsafe {
            let mut l = Library {
                lib,
                shutdown: None,
                clock: sym(lib, c"nvmlDeviceGetClockInfo"),
                max_clock: sym(lib, c"nvmlDeviceGetMaxClockInfo"),
                temperature: sym(lib, c"nvmlDeviceGetTemperature"),
                power: sym(lib, c"nvmlDeviceGetPowerUsage"),
                power_limit: sym(lib, c"nvmlDeviceGetEnforcedPowerLimit"),
                fan: sym(lib, c"nvmlDeviceGetFanSpeed"),
                utilization: sym(lib, c"nvmlDeviceGetUtilizationRates"),
                encoder: sym(lib, c"nvmlDeviceGetEncoderUtilization"),
                decoder: sym(lib, c"nvmlDeviceGetDecoderUtilization"),
                clock_reasons: sym(lib, c"nvmlDeviceGetCurrentClocksEventReasons")
                    .or_else(|| sym(lib, c"nvmlDeviceGetCurrentClocksThrottleReasons"))
                    .ok_or_else(|| super::missing("nvmlDeviceGetCurrentClocksEventReasons")),
                pstate: sym(lib, c"nvmlDeviceGetPerformanceState"),
                pcie_gen: sym(lib, c"nvmlDeviceGetCurrPcieLinkGeneration"),
                pcie_width: sym(lib, c"nvmlDeviceGetCurrPcieLinkWidth"),
                pcie_max_gen: sym(lib, c"nvmlDeviceGetMaxPcieLinkGeneration"),
                pcie_max_width: sym(lib, c"nvmlDeviceGetMaxPcieLinkWidth"),
                memory: sym(lib, c"nvmlDeviceGetMemoryInfo"),
                handles: Vec::new(),
                seen: Vec::new(),
            };
            let (Some(init), Some(count), Some(handle)) = (
                sym::<Plain>(lib, c"nvmlInit_v2"),
                sym::<Count>(lib, c"nvmlDeviceGetCount_v2"),
                sym::<Handle>(lib, c"nvmlDeviceGetHandleByIndex_v2"),
            ) else {
                return Err("libnvidia-ml.so.1 is too old".into());
            };
            let code = init();
            if code != NVML_SUCCESS {
                return Err(format!("NVML did not start (error {code})"));
            }
            l.shutdown = sym(lib, c"nvmlShutdown");
            let (name, pci) = (sym::<Name>(lib, c"nvmlDeviceGetName"), sym::<Pci>(lib, c"nvmlDeviceGetPciInfo_v3"));
            let mut n = 0u32;
            if count(&mut n) != NVML_SUCCESS || n == 0 {
                return Err("no NVIDIA GPU".into());
            }
            for i in 0..n {
                let mut d: Dev = std::ptr::null_mut();
                if handle(i, &mut d) != NVML_SUCCESS {
                    continue;
                }
                let mut buf = [0u8; 96];
                let name = match name {
                    Some(f) if f(d, buf.as_mut_ptr(), buf.len() as u32) == NVML_SUCCESS => {
                        String::from_utf8_lossy(&buf[..buf.iter().position(|&b| b == 0).unwrap_or(buf.len())]).into_owned()
                    }
                    _ => "NVIDIA GPU".into(),
                };
                let mut info: PciInfo = std::mem::zeroed();
                let ids = match pci {
                    Some(f) if f(d, &mut info) == NVML_SUCCESS => info.pci_device_id,
                    _ => 0,
                };
                l.seen.push(Adapter { name, vendor_id: ids & 0xFFFF, device_id: ids >> 16 });
                l.handles.push(d);
            }
            Ok(l)
        }
    }
}

impl Driver for Library {
    fn cards(&self) -> Vec<Adapter> {
        self.seen.clone()
    }

    fn sample(&self, card: usize) -> GpuSample {
        let d = self.handles[card];
        // SAFETY: `d` is a handle of this library; out-pointers are valid.
        unsafe {
            let typed = |f: Option<Typed>, t: u32| {
                let mut v = 0u32;
                f.filter(|f| f(d, t, &mut v) == NVML_SUCCESS).map(|_| f64::from(v))
            };
            let value = |f: Option<Value>| {
                let mut v = 0u32;
                f.filter(|f| f(d, &mut v) == NVML_SUCCESS).map(|_| f64::from(v))
            };
            let codec = |f: Option<Codec>| {
                let (mut v, mut period) = (0u32, 0u32);
                f.filter(|f| f(d, &mut v, &mut period) == NVML_SUCCESS).map(|_| f64::from(v))
            };
            let mut util = [0u32; 2];
            let load = self.utilization.filter(|f| f(d, &mut util) == NVML_SUCCESS).map(|_| f64::from(util[0]));
            GpuSample {
                graphics_mhz: typed(self.clock, CLOCK_GRAPHICS),
                graphics_max_mhz: typed(self.max_clock, CLOCK_GRAPHICS),
                memory_mhz: typed(self.clock, CLOCK_MEM),
                memory_max_mhz: typed(self.max_clock, CLOCK_MEM),
                power_w: value(self.power).map(|mw| mw / 1000.0),
                power_limit_w: value(self.power_limit).map(|mw| mw / 1000.0),
                temperature_c: typed(self.temperature, TEMPERATURE_GPU),
                hotspot_c: None,
                fan_percent: value(self.fan),
                load_percent: load,
                encoder_percent: codec(self.encoder),
                decoder_percent: codec(self.decoder),
            }
        }
    }

    fn memory(&self, card: usize) -> Option<(u64, u64)> {
        let d = self.handles[card];
        let mut counts = MemoryCounts { total: 0, free: 0, used: 0 };
        // SAFETY: `d` is a handle of this library; the out-pointer is valid.
        self.memory.filter(|f| unsafe { f(d, &mut counts) } == NVML_SUCCESS).map(|_| (counts.used, counts.total))
    }

    fn extras(&self, card: usize) -> Extras {
        let d = self.handles[card];
        // SAFETY: `d` is a handle of this library; out-pointers are valid.
        unsafe {
            let value = |f: Option<Value>, name: &str| {
                let f = f.ok_or_else(|| super::missing(name))?;
                let mut v = 0u32;
                match f(d, &mut v) {
                    NVML_SUCCESS => Ok(v),
                    code => Err(super::failed(code)),
                }
            };
            let clock_reasons = self.clock_reasons.clone().and_then(|f| {
                let mut mask = 0u64;
                match f(d, &mut mask) {
                    NVML_SUCCESS => Ok(mask),
                    code => Err(super::failed(code)),
                }
            });
            let current = value(self.pcie_gen, "nvmlDeviceGetCurrPcieLinkGeneration")
                .and_then(|g| value(self.pcie_width, "nvmlDeviceGetCurrPcieLinkWidth").map(|w| (g, w)));
            let max = value(self.pcie_max_gen, "nvmlDeviceGetMaxPcieLinkGeneration")
                .and_then(|g| value(self.pcie_max_width, "nvmlDeviceGetMaxPcieLinkWidth").map(|w| (g, w)))
                .ok();
            Extras { clock_reasons, pstate: value(self.pstate, "nvmlDeviceGetPerformanceState"), pcie: current.map(|c| (c, max)) }
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        // SAFETY: shutdown only after a successful init; the library was
        // opened by `open` and nothing of it is used afterwards.
        unsafe {
            if let Some(shutdown) = self.shutdown {
                shutdown();
            }
            dlclose(self.lib);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;
    use crate::sensors::model::Frame;

    struct Fake {
        cards: Vec<Adapter>,
    }

    impl Driver for Fake {
        fn cards(&self) -> Vec<Adapter> {
            self.cards.clone()
        }
        fn sample(&self, card: usize) -> GpuSample {
            GpuSample { load_percent: Some(40.0 + card as f64), temperature_c: Some(55.0), power_w: Some(120.5), graphics_mhz: Some(1800.0), ..Default::default() }
        }
        fn extras(&self, _card: usize) -> Extras {
            Extras { clock_reasons: Ok(0x4), pstate: Ok(2), pcie: Ok(((4, 16), Some((4, 16)))) }
        }
        fn memory(&self, card: usize) -> Option<(u64, u64)> {
            (card == 0).then_some((6_000_000_000, 16_000_000_000))
        }
    }

    fn nvidia(name: &str) -> Adapter {
        Adapter { name: name.into(), vendor_id: gpu::NVIDIA, device_id: 0x2704 }
    }

    #[test]
    fn nvidia_cards_read_through_the_library_show_load_temperature_power_and_states() {
        let reader = Reader::with(Ok(Box::new(Fake { cards: vec![nvidia("NVIDIA GeForce RTX 4080"), nvidia("NVIDIA GeForce RTX 3060")] })));
        assert_eq!(reader.availability(), Ok(()));
        let frame = Frame::build(0, &[reader.read(Instant::now())], Vec::new());
        assert_eq!(frame.devices.len(), 2, "one heading per card");
        assert_eq!(frame.devices[0].device.name, "NVIDIA GeForce RTX 4080");
        let text = |id: &str| frame.reading(id).unwrap_or_else(|| panic!("no {id}")).text();
        assert_eq!(text("gpu.nvidia0.utilization.gpu_percent"), "40 %");
        assert_eq!(text("gpu.nvidia1.utilization.gpu_percent"), "41 %", "each card has its own rows");
        assert_eq!(text("gpu.nvidia0.temperature.gpu_c"), "55.0 °C");
        assert_eq!(text("gpu.nvidia0.power.board_w"), "120.5 W");
        assert_eq!(text("gpu.nvidia0.clock.limit_reasons"), "Power cap");
        assert_eq!(text("gpu.nvidia0.pcie.link"), "Gen 4 x16 of Gen 4 x16");
        assert!(frame.readings().all(|r| r.source == SOURCE));
    }

    #[test]
    fn the_cards_dedicated_memory_in_use_and_in_all_is_read_through_the_library() {
        let reader = Reader::with(Ok(Box::new(Fake { cards: vec![nvidia("NVIDIA GeForce RTX 4080"), nvidia("NVIDIA GeForce RTX 3060")] })));
        let frame = Frame::build(0, &[reader.read(Instant::now())], Vec::new());
        let text = |id: &str| frame.reading(id).unwrap_or_else(|| panic!("no {id}")).text();
        assert_eq!(text("gpu.nvidia0.memory.dedicated_used_bytes"), "6.0 GB");
        assert_eq!(text("gpu.nvidia0.memory.dedicated_total_bytes"), "16.0 GB");
        assert_eq!(text("gpu.nvidia1.memory.dedicated_used_bytes"), "NVML reports no memory figures", "a card the library gave no figures for is grey, never zero");
    }

    #[test]
    fn a_pc_without_the_library_reads_nothing_and_says_why() {
        let reader = Reader::with(Err("libnvidia-ml.so.1 not found (no NVIDIA driver)".into()));
        assert_eq!(reader.availability(), Err("libnvidia-ml.so.1 not found (no NVIDIA driver)".to_string()));
        assert!(reader.read(Instant::now()).is_empty(), "absence is not an error: no rows, no panic");
    }

    #[test]
    fn a_library_that_sees_no_card_is_no_nvidia_gpu() {
        let reader = Reader::with(Ok(Box::new(Fake { cards: Vec::new() })));
        assert_eq!(reader.availability(), Err("no NVIDIA GPU".to_string()));
        assert!(reader.read(Instant::now()).is_empty());
    }

    #[test]
    fn the_provider_is_listed_only_where_the_kernel_shows_an_nvidia_card() {
        assert!(has_card(&fixture("gpu-nvidia")));
        assert!(!has_card(&fixture("gpu-amd-intel")), "AMD and Intel only: no NVML line");
        assert!(!has_card(&fixture("arm-board")), "no DRM folder at all");
    }

    #[test]
    fn opening_the_real_library_never_panics_where_it_is_missing() {
        // Either the library is here (a started session) or the reason says it is not.
        if let Err(why) = Library::open() {
            assert!(!why.is_empty());
        }
    }
}
