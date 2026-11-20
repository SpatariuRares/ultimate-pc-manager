//! The ADLX provider: AMD cards read through `amdadlx64.dll` (AMD Device
//! Library eXtra, installed with the AMD driver), loaded only while the
//! sensor service runs ([`super::vendor_gpu`]). Values come from ADLX's
//! performance monitoring (current GPU metrics) and, for the optional
//! fields (maximum clocks, power limit, fan), from what the driver says it
//! supports (`IADLXGPUMetricsSupport`, asked once when ADLX starts).
//!
//! ADLX interfaces used (AMD ADLX SDK, `SDK/Include/IPerformanceMonitoring.h`,
//! <https://gpuopen.com/manuals/adlx/adlx-_d_o_x__i_a_d_l_x_g_p_u_metrics_support/>):
//! - `IADLXPerformanceMonitoringServices::GetSupportedGPUMetrics` gives the
//!   card's `IADLXGPUMetricsSupport`;
//! - `IsSupportedGPUClockSpeed` / `GetGPUClockSpeedRange` (MHz): the maximum
//!   graphics clock is the top of the range;
//! - `IsSupportedGPUVRAMClockSpeed` / `GetGPUVRAMClockSpeedRange` (MHz): the
//!   maximum memory clock;
//! - `IsSupportedGPUTotalBoardPower` / `GetGPUTotalBoardPowerRange` (W): the
//!   board power limit is the top of the range; without Total Board Power
//!   (APUs) the top of `GetGPUPowerRange` (W) is used, as Board Power falls
//!   back to the GPU chip's power. (`IADLXManualPowerTuning::GetPowerLimit`
//!   is a percentage offset of the tuning page, not watts, so it is not used.)
//! - `IsSupportedGPUFanSpeed` / `GetGPUFanSpeedRange` (RPM) and
//!   `IADLXGPUMetrics::GPUFanSpeed` (RPM): Fan Speed, and Fan % as the
//!   current speed over the top of the range.

use std::time::Instant;

use super::gpu::{self, GpuSample};
use super::model::{Device, Group, Reading, Unit};
use super::vendor_gpu::{Vendor, VendorGpu};

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "ADLX";

/// The reason of a current metric ADLX did not give.
pub const NOT_REPORTED: &str = "ADLX does not report it";

/// The reason of a field the installed driver does not support.
pub const UNSUPPORTED: &str = "not supported by the installed AMD driver";

/// `ADLX_NOT_SUPPORTED` (`ADLX_RESULT` in `ADLXDefines.h`).
pub const NOT_SUPPORTED: i32 = 12;

/// The card's fan speed in RPM (`gpu.<key>.fan_rpm`), only AMD cards have it.
pub const FAN_RPM: &str = "fan_rpm";

/// What the provider shows of AMD cards.
pub fn vendor() -> Vendor {
    Vendor { source: SOURCE, vendor_id: super::gpu::AMD, not_reported: NOT_REPORTED, prefix: "amd", no_card: "no AMD GPU" }
}

/// An ADLX getter's answer: its value, or the `ADLX_RESULT` it returned.
pub type Answer<T> = Result<T, i32>;

/// What the driver says of one optional field: whether it is supported
/// (`IsSupported…`) and its range (`Get…Range`, min and max).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Field {
    pub supported: Answer<bool>,
    pub range: Answer<(i32, i32)>,
}

/// The reason of a failed ADLX answer.
fn failed(code: i32) -> String {
    if code == NOT_SUPPORTED {
        UNSUPPORTED.to_string()
    } else {
        format!("ADLX did not answer (error {code})")
    }
}

impl Field {
    /// Whether the driver supports the field, or why not.
    pub fn supported(&self) -> Result<(), String> {
        match self.supported {
            Ok(true) => Ok(()),
            Ok(false) => Err(UNSUPPORTED.to_string()),
            Err(code) => Err(failed(code)),
        }
    }

    /// The top of the field's range, or why there is none.
    pub fn maximum(&self) -> Result<f64, String> {
        self.supported()?;
        match self.range {
            Ok((_, max)) if max > 0 => Ok(f64::from(max)),
            Ok(_) => Err("ADLX reports no maximum".to_string()),
            Err(code) => Err(failed(code)),
        }
    }
}

/// What the driver supports of a card, asked once when ADLX starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Support {
    pub clock: Field,
    pub vram_clock: Field,
    pub board_power: Field,
    pub gpu_power: Field,
    pub fan_speed: Field,
}

impl Support {
    /// Every field failed with the same ADLX `code` (the support object
    /// could not be had).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn failed(code: i32) -> Support {
        let f = Field { supported: Err(code), range: Err(code) };
        Support { clock: f, vram_clock: f, board_power: f, gpu_power: f, fan_speed: f }
    }

    /// The board power limit: the top of the Total Board Power range, else
    /// of the GPU power range; the Total Board Power reason when neither.
    fn power_limit(&self) -> Result<f64, String> {
        self.board_power.maximum().or_else(|why| self.gpu_power.maximum().map_err(|_| why))
    }
}

/// The readings of an AMD card: its current `metrics` and the optional
/// fields its driver `support`s; what is missing says why.
pub fn readings(metrics: &Metrics, support: &Support, device: &Device, at: Instant) -> Vec<Reading> {
    let rpm = support.fan_speed.supported().and_then(|()| metrics.fan_rpm.ok_or_else(|| NOT_REPORTED.to_string()));
    let fan_percent = support.fan_speed.maximum().and_then(|max| rpm.clone().map(|rpm| rpm / max * 100.0));
    let optional = [
        ("clock.graphics_max_hz", support.clock.maximum()),
        ("clock.memory_max_hz", support.vram_clock.maximum()),
        ("power.limit_w", support.power_limit()),
        ("fan_percent", fan_percent.clone()),
    ];
    let sample = GpuSample {
        graphics_max_mhz: optional[0].1.clone().ok(),
        memory_max_mhz: optional[1].1.clone().ok(),
        power_limit_w: optional[2].1.clone().ok(),
        fan_percent: fan_percent.ok(),
        ..metrics.sample(support)
    };
    let mut rows = sample.readings(device, SOURCE, NOT_REPORTED, at);
    for (what, answer) in optional {
        let id = gpu::metric(device, what);
        if let (Err(why), Some(row)) = (answer, rows.iter_mut().find(|r| r.id == id)) {
            row.unavailable = Some(why);
        }
    }
    let fan = Reading::measure(&gpu::metric(device, FAN_RPM), device.clone(), Group::Fans, "Fan Speed", Unit::Rpm, SOURCE, at);
    rows.push(match rpm {
        Ok(rpm) => fan.with(Some(rpm), NOT_REPORTED),
        Err(why) => fan.with(None, &why),
    });
    rows
}

/// ADLX's current metrics of a card (`None` = not supported).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Metrics {
    pub usage_percent: Option<f64>,
    pub clock_mhz: Option<f64>,
    pub vram_clock_mhz: Option<f64>,
    pub temperature_c: Option<f64>,
    pub hotspot_c: Option<f64>,
    /// The GPU chip's power (APUs and older cards).
    pub gpu_power_w: Option<f64>,
    /// The whole card's power (Total Board Power, recent discrete cards).
    pub board_power_w: Option<f64>,
    /// The fan speed in RPM.
    pub fan_rpm: Option<f64>,
}

impl Metrics {
    /// The card's values: Board Power is the whole card's when its driver
    /// `support`s Total Board Power and ADLX gives it, else the GPU chip's
    /// (a value ADLX gives for an unsupported field is not trusted).
    pub fn sample(&self, support: &Support) -> GpuSample {
        let board = self.board_power_w.filter(|_| support.board_power.supported().is_ok());
        GpuSample {
            graphics_mhz: self.clock_mhz,
            memory_mhz: self.vram_clock_mhz,
            power_w: board.or(self.gpu_power_w),
            temperature_c: self.temperature_c,
            hotspot_c: self.hotspot_c,
            load_percent: self.usage_percent,
            ..Default::default()
        }
    }
}

/// A PCI id as ADLX writes it ("1002", "0x164E").
pub fn pci_id(text: &str) -> Option<u32> {
    let t = text.trim();
    let t = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")).unwrap_or(t);
    u32::from_str_radix(t, 16).ok()
}

/// The provider: `amdadlx64.dll` loaded if present, the AMD cards it sees
/// matched to DXCore's.
pub fn provider() -> VendorGpu {
    #[cfg(windows)]
    let library = imp::Adlx::open().map(|l| Box::new(l) as Box<dyn super::vendor_gpu::Library>);
    #[cfg(not(windows))]
    let library = Err("ADLX is a Windows library".to_string());
    VendorGpu::new(vendor(), library, &super::dxcore::adapters().unwrap_or_default())
}

#[cfg(windows)]
mod imp {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Reading};
    use std::time::Instant;

    #[test]
    fn adlx_writes_pci_ids_in_hex() {
        assert_eq!(pci_id("1002"), Some(0x1002));
        assert_eq!(pci_id("0x164E"), Some(0x164E));
        assert_eq!(pci_id(" 164e "), Some(0x164E));
        assert_eq!(pci_id("AMD"), None);
    }

    #[test]
    fn board_power_is_the_whole_cards_when_adlx_gives_it_else_the_chips() {
        let (apu, apu_support) = apu();
        let s = apu.sample(&apu_support);
        assert_eq!(s.power_w, Some(4.5));
        assert_eq!((s.graphics_mhz, s.memory_mhz, s.load_percent, s.temperature_c), (Some(600.0), Some(2400.0), Some(3.0), Some(41.0)));
        assert_eq!(s.hotspot_c, None, "an APU has no hot spot sensor");
        let (_, card_support) = discrete();
        let card = Metrics { gpu_power_w: Some(180.0), board_power_w: Some(240.0), ..Default::default() };
        assert_eq!(card.sample(&card_support).power_w, Some(240.0));
        let silent = Metrics { gpu_power_w: Some(180.0), ..Default::default() };
        assert_eq!(silent.sample(&card_support).power_w, Some(180.0), "no board value: the chip's");
    }

    fn field(supported: Answer<bool>, range: Answer<(i32, i32)>) -> Field {
        Field { supported, range }
    }

    #[test]
    fn an_optional_field_is_its_maximum_or_why_not() {
        assert_eq!(field(Ok(true), Ok((0, 2600))).maximum(), Ok(2600.0));
        assert_eq!(field(Ok(false), Ok((0, 2600))).maximum(), Err(UNSUPPORTED.to_string()));
        assert_eq!(field(Err(NOT_SUPPORTED), Err(NOT_SUPPORTED)).maximum(), Err(UNSUPPORTED.to_string()));
        assert_eq!(field(Err(3), Ok((0, 2600))).maximum(), Err("ADLX did not answer (error 3)".to_string()));
        assert_eq!(field(Ok(true), Err(NOT_SUPPORTED)).maximum(), Err(UNSUPPORTED.to_string()));
        assert_eq!(field(Ok(true), Err(4)).maximum(), Err("ADLX did not answer (error 4)".to_string()));
        assert_eq!(field(Ok(true), Ok((0, 0))).maximum(), Err("ADLX reports no maximum".to_string()));
    }

    fn text_of(rows: &[Reading], what: &str) -> String {
        let id = format!("gpu.gpu1.{what}");
        rows.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no row {id}")).text()
    }

    /// A discrete card whose driver supports every optional field.
    fn discrete() -> (Metrics, Support) {
        let metrics = Metrics { clock_mhz: Some(2400.0), vram_clock_mhz: Some(2500.0), gpu_power_w: Some(180.0), board_power_w: Some(240.0), fan_rpm: Some(1500.0), ..Default::default() };
        let support = Support {
            clock: field(Ok(true), Ok((500, 2600))),
            vram_clock: field(Ok(true), Ok((96, 2500))),
            board_power: field(Ok(true), Ok((0, 263))),
            gpu_power: field(Ok(true), Ok((0, 200))),
            fan_speed: field(Ok(true), Ok((0, 3000))),
        };
        (metrics, support)
    }

    /// An APU: no board power, no fan.
    fn apu() -> (Metrics, Support) {
        let metrics = Metrics { usage_percent: Some(3.0), clock_mhz: Some(600.0), vram_clock_mhz: Some(2400.0), temperature_c: Some(41.0), gpu_power_w: Some(4.5), ..Default::default() };
        let support = Support {
            clock: field(Ok(true), Ok((0, 2200))),
            vram_clock: field(Ok(true), Ok((0, 2400))),
            board_power: field(Ok(false), Err(NOT_SUPPORTED)),
            gpu_power: field(Ok(true), Ok((0, 0))),
            fan_speed: field(Ok(false), Ok((0, 0))),
        };
        (metrics, support)
    }

    #[test]
    fn an_amd_card_shows_its_maximum_clocks_power_limit_and_fan() {
        let (metrics, support) = discrete();
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon RX 7800 XT"), Instant::now());
        assert_eq!(text_of(&rows, "clock.graphics_max_hz"), "2.60 GHz");
        assert_eq!(text_of(&rows, "clock.memory_max_hz"), "2.50 GHz");
        assert_eq!(text_of(&rows, "power.limit_w"), "263.0 W", "the top of the Total Board Power range");
        assert_eq!(text_of(&rows, FAN_RPM), "1,500 RPM");
        assert_eq!(text_of(&rows, "fan_percent"), "50 %", "1500 of 3000 RPM");
    }

    #[test]
    fn fields_the_driver_does_not_support_stay_grey_with_a_reason() {
        let (metrics, support) = apu();
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon(TM) Graphics"), Instant::now());
        assert_eq!(text_of(&rows, "clock.graphics_max_hz"), "2.20 GHz");
        assert_eq!(text_of(&rows, FAN_RPM), UNSUPPORTED);
        assert_eq!(text_of(&rows, "fan_percent"), UNSUPPORTED);
        assert_eq!(text_of(&rows, "power.limit_w"), UNSUPPORTED, "no board power and no GPU power maximum");
        assert_eq!(text_of(&rows, "temperature.hotspot_c"), NOT_REPORTED, "a current metric ADLX did not give");
        let failing = Support { clock: field(Err(3), Err(3)), ..support };
        let rows = readings(&metrics, &failing, &Device::gpu("gpu1", "AMD Radeon(TM) Graphics"), Instant::now());
        assert_eq!(text_of(&rows, "clock.graphics_max_hz"), "ADLX did not answer (error 3)");
    }

    #[test]
    fn a_card_without_total_board_power_support_always_shows_its_gpu_power() {
        // Seen on an idle iGPU: 3 W, then a stray 60 W board power the driver
        // says it does not support.
        let (mut metrics, support) = apu();
        metrics.gpu_power_w = Some(3.0);
        metrics.board_power_w = Some(60.0);
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon(TM) Graphics"), Instant::now());
        assert_eq!(text_of(&rows, "power.board_w"), "3.0 W");
        assert_eq!(metrics.sample(&support).power_w, Some(3.0));
        let (card, card_support) = discrete();
        assert_eq!(card.sample(&card_support).power_w, Some(240.0), "a supported Total Board Power still wins");
    }

    #[test]
    fn adlx_failed_support_greys_optional_rows() {
        let (mut metrics, _) = discrete();
        let support = Support::failed(NOT_SUPPORTED);
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon RX 7800 XT"), Instant::now());
        for what in ["clock.graphics_max_hz", "clock.memory_max_hz", "power.limit_w", "fan_percent", FAN_RPM] {
            assert_eq!(text_of(&rows, what), UNSUPPORTED, "{what}");
        }
        assert!(rows.iter().find(|r| r.id == "gpu.gpu1.fan_rpm").is_some_and(|r| r.name == "Fan Speed" && r.value.is_none()));
        metrics.board_power_w = Some(240.0);
        metrics.gpu_power_w = Some(180.0);
        assert_eq!(metrics.sample(&support).power_w, Some(180.0), "a board power on an unsupported field falls back to the GPU power");
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon RX 7800 XT"), Instant::now());
        assert_eq!(text_of(&rows, "power.board_w"), "180.0 W");
    }

    #[test]
    fn the_power_limit_falls_back_to_the_gpu_power_maximum() {
        let (metrics, mut support) = apu();
        support.gpu_power = field(Ok(true), Ok((0, 25)));
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon(TM) Graphics"), Instant::now());
        assert_eq!(text_of(&rows, "power.limit_w"), "25.0 W");
    }

    #[test]
    fn the_existing_adlx_readings_still_hold() {
        let (metrics, support) = apu();
        let rows = readings(&metrics, &support, &Device::gpu("gpu1", "AMD Radeon(TM) Graphics"), Instant::now());
        assert_eq!(text_of(&rows, "clock.graphics_hz"), "600 MHz");
        assert_eq!(text_of(&rows, "clock.memory_hz"), "2.40 GHz");
        assert_eq!(text_of(&rows, "power.board_w"), "4.5 W");
        assert_eq!(text_of(&rows, "temperature.gpu_c"), "41.0 °C");
        assert_eq!(text_of(&rows, "utilization.gpu_percent"), "3 %");
        assert_eq!(text_of(&rows, "utilization.encoder_percent"), NOT_REPORTED);
        assert!(rows.iter().all(|r| r.source == SOURCE));
        assert_eq!(rows.len(), 13, "the twelve card rows and Fan Speed");
    }

    #[test]
    #[ignore = "reads this PC's AMD GPU through ADLX"]
    #[cfg(windows)]
    fn reads_this_pc_adlx_and_unloads_it_when_dropped() {
        use crate::sensors::gpu;
        use crate::sensors::model::Frame;
        use crate::sensors::provider::Provider;
        use std::time::Instant;
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        let loaded = || {
            let name: Vec<u16> = "amdadlx64.dll\0".encode_utf16().collect();
            // SAFETY: a NUL-terminated name; no reference is taken.
            !unsafe { GetModuleHandleW(name.as_ptr()) }.is_null()
        };
        assert!(!loaded(), "nothing loads ADLX before the provider");
        let mut p = provider();
        assert_eq!(p.availability(), Ok(()));
        assert!(loaded());
        let frame = Frame::build(1, &[p.read(Instant::now())], Vec::new());
        let card = frame.devices.iter().find(|d| d.device.name.contains("Radeon")).expect("the AMD iGPU");
        assert_eq!(card.device.key, "gpu1", "matched to DXCore's card");
        assert!(frame.reading(&gpu::metric(&card.device, gpu::GRAPHICS_CLOCK)).and_then(|r| r.value).unwrap() > 0.0);
        for d in &frame.devices {
            println!("{} ({})   {}", d.device.name, d.device.key, gpu::summary(d));
            for g in &d.groups {
                for r in &g.readings {
                    println!("    {:<22} {}", r.name, r.text());
                }
            }
        }
        drop(p);
        assert!(!loaded(), "ADLX terminated and amdadlx64.dll unloaded at stop");
    }
}
