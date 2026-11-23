//! Windows: ADLX, loaded from `amdadlx64.dll` only while the service runs.

use std::ffi::{c_char, c_void, CStr};
use std::mem::transmute;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{FreeLibrary, HMODULE};
use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};

use std::time::Instant;

use super::{pci_id, readings, Answer, Field, Metrics, Support};
use crate::sensors::gpu::{Adapter, GpuSample, AMD};
use crate::sensors::model::{Device, Reading};
use crate::sensors::vendor_gpu::Library;

const ADLX_OK: i32 = 0;
type QueryVersion = unsafe extern "C" fn(*mut u64) -> i32;
type Initialize = unsafe extern "C" fn(u64, *mut *mut c_void) -> i32;
type Terminate = unsafe extern "C" fn() -> i32;
type Get = unsafe extern "C" fn(*mut c_void, *mut *mut c_void) -> i32;
type Text = unsafe extern "C" fn(*mut c_void, *mut *const c_char) -> i32;
type Size = unsafe extern "C" fn(*mut c_void) -> u32;
type At = unsafe extern "C" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
type Metric = unsafe extern "C" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> i32;
type Double = unsafe extern "C" fn(*mut c_void, *mut f64) -> i32;
type Int = unsafe extern "C" fn(*mut c_void, *mut i32) -> i32;
type Release = unsafe extern "C" fn(*mut c_void) -> i32;
/// `IsSupported…(pThis, adlx_bool*)`; `adlx_bool` is one byte.
type Bool = unsafe extern "C" fn(*mut c_void, *mut u8) -> i32;
/// `Get…Range(pThis, adlx_int* minValue, adlx_int* maxValue)`.
type Range = unsafe extern "C" fn(*mut c_void, *mut i32, *mut i32) -> i32;

// Vtable slots (the C interfaces of the ADLX SDK headers).
/// IADLXSystem (not reference counted).
const SYSTEM_GET_GPUS: usize = 1;
const SYSTEM_GET_PERFORMANCE_MONITORING: usize = 9;
/// IADLXInterface: Acquire 0, Release 1, QueryInterface 2.
const RELEASE: usize = 1;
const LIST_SIZE: usize = 3;
const GPU_LIST_AT: usize = 11;
const GPU_VENDOR_ID: usize = 3;
const GPU_NAME: usize = 7;
const GPU_DEVICE_ID: usize = 14;
const CURRENT_GPU_METRICS: usize = 18;
const METRIC_USAGE: usize = 4;
const METRIC_CLOCK: usize = 5;
const METRIC_VRAM_CLOCK: usize = 6;
const METRIC_TEMPERATURE: usize = 7;
const METRIC_HOTSPOT: usize = 8;
const METRIC_GPU_POWER: usize = 9;
const METRIC_BOARD_POWER: usize = 10;
const METRIC_FAN_SPEED: usize = 11;
/// IADLXPerformanceMonitoringServices::GetSupportedGPUMetrics.
const SUPPORTED_GPU_METRICS: usize = 21;
/// IADLXGPUMetricsSupport: IsSupported… and Get…Range.
const SUPPORTS_CLOCK: usize = 4;
const SUPPORTS_VRAM_CLOCK: usize = 5;
const SUPPORTS_GPU_POWER: usize = 8;
const SUPPORTS_BOARD_POWER: usize = 9;
const SUPPORTS_FAN_SPEED: usize = 10;
const RANGE_CLOCK: usize = 14;
const RANGE_VRAM_CLOCK: usize = 15;
const RANGE_GPU_POWER: usize = 18;
const RANGE_FAN_SPEED: usize = 19;
const RANGE_BOARD_POWER: usize = 22;

/// The `i`-th method of an ADLX object's vtable.
unsafe fn slot(object: *mut c_void, i: usize) -> *const c_void {
    *(*(object as *const *const *const c_void)).add(i)
}

/// A reference-counted ADLX object, released when dropped.
struct Obj(*mut c_void);

impl Drop for Obj {
    fn drop(&mut self) {
        // SAFETY: slot 1 is IADLXInterface::Release of a live object.
        unsafe {
            let release: Release = transmute(slot(self.0, RELEASE));
            release(self.0);
        }
    }
}

/// A started ADLX: the library, the monitoring services and the AMD
/// cards. Dropped = objects released, ADLX terminated, library freed.
pub struct Adlx {
    // Field order = drop order: objects before `Terminate`.
    gpus: Vec<(Obj, Adapter, Support)>,
    monitoring: Option<Obj>,
    terminate: Option<Terminate>,
    lib: HMODULE,
}

/// A string an ADLX getter returns.
unsafe fn text(object: *mut c_void, i: usize) -> Option<String> {
    let get: Text = transmute(slot(object, i));
    let mut p: *const c_char = std::ptr::null();
    (get(object, &mut p) == ADLX_OK && !p.is_null()).then(|| CStr::from_ptr(p).to_string_lossy().into_owned())
}


/// What the driver supports of `gpu` (IADLXGPUMetricsSupport); the
/// support object is released before returning.
unsafe fn support(monitoring: &Obj, gpu: &Obj) -> Support {
    let get: Metric = transmute(slot(monitoring.0, SUPPORTED_GPU_METRICS));
    let mut p = null_mut();
    let code = get(monitoring.0, gpu.0, &mut p);
    if code != ADLX_OK || p.is_null() {
        return Support::failed(if code == ADLX_OK { super::NOT_SUPPORTED } else { code });
    }
    let s = Obj(p);
    let field = |supported: usize, range: usize| {
        let is: Bool = transmute(slot(s.0, supported));
        let mut b = 0u8;
        let code = is(s.0, &mut b);
        let supported: Answer<bool> = if code == ADLX_OK { Ok(b != 0) } else { Err(code) };
        let get: Range = transmute(slot(s.0, range));
        let (mut min, mut max) = (0, 0);
        let code = get(s.0, &mut min, &mut max);
        Field { supported, range: if code == ADLX_OK { Ok((min, max)) } else { Err(code) } }
    };
    Support {
        clock: field(SUPPORTS_CLOCK, RANGE_CLOCK),
        vram_clock: field(SUPPORTS_VRAM_CLOCK, RANGE_VRAM_CLOCK),
        board_power: field(SUPPORTS_BOARD_POWER, RANGE_BOARD_POWER),
        gpu_power: field(SUPPORTS_GPU_POWER, RANGE_GPU_POWER),
        fan_speed: field(SUPPORTS_FAN_SPEED, RANGE_FAN_SPEED),
    }
}

impl Adlx {
    /// Loads `amdadlx64.dll`, starts ADLX and finds the AMD cards.
    pub fn open() -> Result<Adlx, String> {
        let name: Vec<u16> = "amdadlx64.dll\0".encode_utf16().collect();
        // SAFETY: signatures and slots are ADLX's (ADLX.h, ISystem.h,
        // IPerformanceMonitoring.h); `s` cleans up on every early return.
        unsafe {
            let lib = LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
            if lib.is_null() {
                return Err("amdadlx64.dll not found (no AMD driver)".into());
            }
            let mut s = Adlx { gpus: Vec::new(), monitoring: None, terminate: None, lib };
            let sym = |n: &CStr| GetProcAddress(lib, n.as_ptr().cast());
            let (Some(query), Some(init), Some(term)) = (sym(c"ADLXQueryFullVersion"), sym(c"ADLXInitialize"), sym(c"ADLXTerminate")) else {
                return Err("amdadlx64.dll is too old".into());
            };
            let (query, init): (QueryVersion, Initialize) = (transmute(query), transmute(init));
            let mut version = 0u64;
            if query(&mut version) != ADLX_OK {
                return Err("ADLX did not report its version".into());
            }
            let mut system = null_mut();
            let code = init(version, &mut system);
            if code != ADLX_OK || system.is_null() {
                return Err(format!("ADLX did not start (error {code})"));
            }
            s.terminate = Some(transmute(term));
            let get = |i: usize| {
                let f: Get = transmute(slot(system, i));
                let mut p = null_mut();
                (f(system, &mut p) == ADLX_OK && !p.is_null()).then(|| Obj(p))
            };
            s.monitoring = get(SYSTEM_GET_PERFORMANCE_MONITORING);
            if s.monitoring.is_none() {
                return Err("ADLX performance monitoring not available".into());
            }
            let Some(list) = get(SYSTEM_GET_GPUS) else { return Err("ADLX could not list the GPUs".into()) };
            let (size, at): (Size, At) = (transmute(slot(list.0, LIST_SIZE)), transmute(slot(list.0, GPU_LIST_AT)));
            for i in 0..size(list.0) {
                let mut p = null_mut();
                if at(list.0, i, &mut p) != ADLX_OK || p.is_null() {
                    continue;
                }
                let gpu = Obj(p);
                let id = |i: usize| text(gpu.0, i).and_then(|t| pci_id(&t)).unwrap_or(0);
                let adapter = Adapter { name: text(gpu.0, GPU_NAME).unwrap_or_else(|| "AMD GPU".into()), vendor_id: id(GPU_VENDOR_ID), device_id: id(GPU_DEVICE_ID) };
                if adapter.vendor_id == AMD {
                    let support = s.monitoring.as_ref().map_or(Support::failed(super::NOT_SUPPORTED), |m| support(m, &gpu));
                    s.gpus.push((gpu, adapter, support));
                }
            }
            Ok(s)
        }
    }

    fn metrics(&self, gpu: &Obj) -> Option<Metrics> {
        let monitoring = self.monitoring.as_ref()?;
        // SAFETY: slots of IADLXPerformanceMonitoringServices and
        // IADLXGPUMetrics; the metrics object is released when dropped.
        unsafe {
            let current: Metric = transmute(slot(monitoring.0, CURRENT_GPU_METRICS));
            let mut p = null_mut();
            if current(monitoring.0, gpu.0, &mut p) != ADLX_OK || p.is_null() {
                return None;
            }
            let m = Obj(p);
            let double = |i: usize| {
                let f: Double = transmute(slot(m.0, i));
                let mut v = 0.0;
                (f(m.0, &mut v) == ADLX_OK).then_some(v)
            };
            let int = |i: usize| {
                let f: Int = transmute(slot(m.0, i));
                let mut v = 0;
                (f(m.0, &mut v) == ADLX_OK).then_some(f64::from(v))
            };
            Some(Metrics {
                usage_percent: double(METRIC_USAGE),
                clock_mhz: int(METRIC_CLOCK),
                vram_clock_mhz: int(METRIC_VRAM_CLOCK),
                temperature_c: double(METRIC_TEMPERATURE),
                hotspot_c: double(METRIC_HOTSPOT),
                gpu_power_w: double(METRIC_GPU_POWER),
                board_power_w: double(METRIC_BOARD_POWER),
                fan_rpm: int(METRIC_FAN_SPEED),
            })
        }
    }
}

impl Library for Adlx {
    fn cards(&self) -> Vec<Adapter> {
        self.gpus.iter().map(|(_, a, _)| a.clone()).collect()
    }

    fn sample(&mut self, card: usize) -> GpuSample {
        self.gpus.get(card).and_then(|(gpu, _, support)| self.metrics(gpu).map(|m| m.sample(support))).unwrap_or_default()
    }

    fn readings(&mut self, card: usize, device: &Device, _source: &'static str, _why: &str, at: Instant) -> Vec<Reading> {
        let Some((gpu, _, support)) = self.gpus.get(card) else { return Vec::new() };
        readings(&self.metrics(gpu).unwrap_or_default(), support, device, at)
    }
}

impl Drop for Adlx {
    fn drop(&mut self) {
        self.gpus.clear();
        self.monitoring = None;
        // SAFETY: every ADLX object was released above; the library was
        // loaded by `open` and nothing of it is used afterwards.
        unsafe {
            if let Some(terminate) = self.terminate {
                terminate();
            }
            FreeLibrary(self.lib);
        }
    }
}

/// The library, if it loads.
pub fn library() -> Result<Box<dyn crate::sensors::vendor_gpu::Library>, String> {
    Adlx::open().map(|l| Box::new(l) as Box<dyn crate::sensors::vendor_gpu::Library>)
}
