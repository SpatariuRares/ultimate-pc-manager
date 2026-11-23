//! Windows: NVIDIA cards read through `nvml.dll`.

use std::ffi::c_void;
use std::mem::transmute_copy;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use crate::sensors::gpu::{self, Adapter, GpuSample};
use crate::sensors::model::{Device, Reading};
use crate::sensors::nvml::{NOT_REPORTED, SOURCE};
use std::time::Instant;

type Dev = *mut c_void;
type Plain = unsafe extern "C" fn() -> u32;
type Count = unsafe extern "C" fn(*mut u32) -> u32;
type Handle = unsafe extern "C" fn(u32, *mut Dev) -> u32;
type Name = unsafe extern "C" fn(Dev, *mut u8, u32) -> u32;
type Pci = unsafe extern "C" fn(Dev, *mut PciInfo) -> u32;
/// GetClockInfo, GetMaxClockInfo, GetTemperature: (device, type, out).
type Typed = unsafe extern "C" fn(Dev, u32, *mut u32) -> u32;
type Value = unsafe extern "C" fn(Dev, *mut u32) -> u32;
type Utilization = unsafe extern "C" fn(Dev, *mut [u32; 2]) -> u32;
type Codec = unsafe extern "C" fn(Dev, *mut u32, *mut u32) -> u32;
/// nvmlDeviceGetCurrentClocksEventReasons(nvmlDevice_t, unsigned long long*)
/// (R535+) and the deprecated nvmlDeviceGetCurrentClocksThrottleReasons
/// with the same signature (NVML API Reference, Device Queries:
/// <https://docs.nvidia.com/deploy/archive/R535/nvml-api/group__nvmlDeviceQueries.html>).
type Mask = unsafe extern "C" fn(Dev, *mut u64) -> u32;
// nvmlDeviceGetPerformanceState(nvmlDevice_t, nvmlPstates_t*) and
// nvmlDeviceGetCurr/MaxPcieLinkGeneration/Width(nvmlDevice_t,
// unsigned int*) are [`Value`]s (nvmlPstates_t is a C enum, 32 bits).

const NVML_SUCCESS: u32 = 0;
const CLOCK_GRAPHICS: u32 = 0;
const CLOCK_MEM: u32 = 2;
const TEMPERATURE_GPU: u32 = 0;
const NVSMI: &str = r"C:\Program Files\NVIDIA Corporation\NVSMI\nvml.dll";

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

/// A started NVML: the library, its functions and the cards it sees.
/// Dropped = NVML shut down and `nvml.dll` unloaded.
pub struct Session {
    lib: HMODULE,
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
    pub cards: Vec<(Dev, Device)>,
}

/// A function of the library, if it has it.
unsafe fn sym<T: Copy>(lib: HMODULE, name: &std::ffi::CStr) -> Option<T> {
    GetProcAddress(lib, name.as_ptr().cast()).map(|f| transmute_copy(&f))
}

fn load() -> HMODULE {
    // SAFETY: plain loader calls with NUL-terminated wide strings.
    unsafe {
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let lib = LoadLibraryExW(wide("nvml.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if !lib.is_null() {
            return lib;
        }
        LoadLibraryExW(wide(NVSMI).as_ptr(), std::ptr::null_mut(), 0)
    }
}

impl Session {
    /// Loads `nvml.dll`, starts NVML and finds the NVIDIA cards.
    pub fn open() -> Result<Session, String> {
        let lib = load();
        if lib.is_null() {
            return Err("nvml.dll not found (no NVIDIA driver)".into());
        }
        // SAFETY: the signatures are NVML's (nvml.h); `s` frees the
        // library on every early return.
        unsafe {
            let mut s = Session {
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
                cards: Vec::new(),
            };
            let (Some(init), Some(count), Some(handle)) = (
                sym::<Plain>(lib, c"nvmlInit_v2"),
                sym::<Count>(lib, c"nvmlDeviceGetCount_v2"),
                sym::<Handle>(lib, c"nvmlDeviceGetHandleByIndex_v2"),
            ) else {
                return Err("nvml.dll is too old".into());
            };
            let code = init();
            if code != NVML_SUCCESS {
                return Err(format!("NVML did not start (error {code})"));
            }
            s.shutdown = sym(lib, c"nvmlShutdown");
            let (name, pci) = (sym::<Name>(lib, c"nvmlDeviceGetName"), sym::<Pci>(lib, c"nvmlDeviceGetPciInfo_v3"));
            let mut n = 0u32;
            if count(&mut n) != NVML_SUCCESS || n == 0 {
                return Err("no NVIDIA GPU".into());
            }
            let mut seen = Vec::new();
            let mut handles = Vec::new();
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
                seen.push(Adapter { name, vendor_id: ids & 0xFFFF, device_id: ids >> 16 });
                handles.push(d);
            }
            let listed = super::super::dxcore::adapters().unwrap_or_default();
            s.cards = handles.into_iter().zip(gpu::match_cards(&listed, &seen, "nvidia")).collect();
            Ok(s)
        }
    }

    /// One read of a card; what NVML refuses stays `None`.
    pub fn sample(&self, d: Dev) -> GpuSample {
        // SAFETY: `d` is a handle of this session; out-pointers are valid.
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
}

impl Session {
    /// One read of a card's NVIDIA-only states; a function this
    /// driver lacks or a call NVML refuses gives only that reading's
    /// reason.
    pub fn extras(&self, d: Dev) -> super::Extras {
        // SAFETY: `d` is a handle of this session; out-pointers are valid.
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
            super::Extras {
                clock_reasons,
                pstate: value(self.pstate, "nvmlDeviceGetPerformanceState"),
                pcie: current.map(|c| (c, max)),
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: shutdown only after a successful init; the library
        // was loaded by `load` and nothing of it is used afterwards.
        unsafe {
            if let Some(shutdown) = self.shutdown {
                shutdown();
            }
            FreeLibrary(self.lib);
        }
    }
}

/// The provider's NVML session, or why there is none.
pub struct Reader {
    session: Result<Session, String>,
}

impl Reader {
    pub fn open() -> Self {
        Reader { session: Session::open() }
    }

    pub fn availability(&self) -> Result<(), String> {
        self.session.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    pub fn read(&self, at: Instant) -> Vec<Reading> {
        let Ok(s) = &self.session else { return Vec::new() };
        s.cards
            .iter()
            .flat_map(|(handle, card)| {
                let mut readings = s.sample(*handle).readings(card, SOURCE, NOT_REPORTED, at);
                readings.extend(s.extras(*handle).readings(card, at));
                readings
            })
            .collect()
    }
}
