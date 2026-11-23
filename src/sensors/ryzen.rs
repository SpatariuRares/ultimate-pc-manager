//! The AMD Ryzen Master Monitoring SDK provider: package, cores and SoC
//! power, the PPT limit, package temperature, core and SoC voltages and the
//! Fabric Clock (FCLK) of AMD processors.
//!
//! The SDK is AMD's free add-on: found through its registry key, it needs
//! administrator rights and AMD's driver, and does not work in a virtual
//! machine. Each of those is the reason shown on every Ryzen row (and in
//! "Sources on this PC") until it is solved, and Power & Freq's band offers
//! "Get AMD's add-on" and "Restart as administrator" from those reasons.
//! The SDK's `Platform.dll` is loaded only when its registry key exists and
//! everything above is met, and unloaded when the last provider using it is
//! dropped (the add-on stops). The SDK is one object per process
//! (`GetPlatform` returns the same `IPlatform` to every caller), so every
//! provider shares one [`Session`]: started once, closed once. On other
//! processors the provider lists no row.
//!
//! # The SDK's interface
//!
//! The SDK is C++ classes with pure virtual methods (no COM, no IUnknown),
//! shipped as headers in its `include` folder (`IPlatform.h`,
//! `IDeviceManager.h`, `IDevice.h`, `ICPUEx.h`; AMD's readme in the SDK and
//! <https://www.amd.com/en/developer/ryzen-master-monitoring-sdk.html>).
//! The binding below follows the 2024-2025 headers of SDK 3.1 (as
//! redistributed, e.g. github.com/AhmadZaqiq/FusionHUD
//! `FusionHUD.Performance/Dependencies/AMD/include`), cross-checked with two
//! independent bindings that call the SDK without C++
//! (github.com/ExplodingCB/tempmanager `src/sensors/ryzen_sdk.rs`, layout
//! test against the installed x64 headers; github.com/Data-Corruption/NanoMonitor
//! `src/cpu.zig`) and C++ shims built against SDK 3.1.1.5478
//! (github.com/rgroenewoudt/HwMonitor `lib/ryzenmaster`,
//! github.com/eddygk/cpu-temp-widget `native/RmTemp.cpp`):
//!
//! ```cpp
//! extern "C" IPlatform& GetPlatform(void);          // Platform.dll export
//! class IPlatform {                                  // no virtual destructor
//!   virtual bool Init(const char* pszBoardVendor = NULL, bool bUseCPUOnly = false); // 0
//!   virtual bool UnInit(void);                                                       // 1
//!   virtual IDeviceManager& GetIDeviceManager(void);                                 // 2
//! };
//! class IDeviceManager {                             // no virtual destructor
//!   virtual bool Init(const char*, bool);            // 0
//!   virtual bool UnInit(void);                       // 1
//!   virtual IDevice* GetDevice(AOD_DEVICE_TYPE, unsigned long);   // 2 or 3 (overloads:
//!   virtual IDevice* GetDevice(unsigned long);                    // MSVC may swap them)
//!   ...
//! };
//! class IDevice { Init 0, UnInit 1, GetName 2, GetDescription 3, GetVendor 4,
//!   GetRole 5, GetClassName 6, GetType 7, GetIndex 8, virtual ~IDevice() 9 };
//! class ICPUEx : public IDevice { /* overrides reuse 2..8 */ GetL1DataCache 10,
//!   GetL1InstructionCache 11, GetL2Cache 12, GetL3Cache 13, GetCoreCount 14,
//!   GetCorePark 15, GetPackage 16, GetCPUParameters 17, GetChipsetName 18,
//!   GetFamily 19, GetStepping 20, GetModel 21 };
//! enum AOD_DEVICE_TYPE { dtInvalid = -1, dtCPU = 0, dtBIOS = 1 };
//! enum AOD_STATUS_CODE { bFailure = -1, bSuccess = 0, bInvalidInput = 1,
//!   bBiosFuncNotImpemented = 2, bCoreParkingDisabled = 3, bUnsupportedFunction = 4 };
//! ```
//!
//! Only the slots whose place does not depend on overload order are relied
//! on; `GetDevice` is called as `(dtCPU, 0)`, which both overloads answer
//! with the first CPU. Before any read, the device must say it is a CPU
//! (`GetType`) and give a plausible core count (`GetCoreCount`), so a wrong
//! slot map is reported as a reason instead of read as values. Older SDKs
//! (2023 headers: shorter `EffectiveFreqData` and `CPUParameters`, mangled
//! `GetPlatform`) are refused by their `Platform.dll` file version
//! ([`MIN_SDK`]). AMD asks for at most one `GetCPUParameters` a second.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use super::model::{Device, Group, Reading, Unit};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "AMD Ryzen Master SDK";
/// Where AMD offers the SDK ("Get AMD's add-on").
pub const AMD_URL: &str = "https://www.amd.com/en/developer/ryzen-master-monitoring-sdk.html";

/// Not an AMD processor: no Ryzen rows at all.
pub const NOT_AMD: &str = "AMD processors only";
/// The SDK is not installed.
pub const NEEDS_SDK: &str = "needs AMD's add-on";
/// The SDK is installed, but older than the interface this app calls.
pub const SDK_TOO_OLD: &str = "AMD's add-on is too old; install the latest";
/// The SDK is installed but this app runs without administrator rights.
pub const NEEDS_ADMIN: &str = "needs administrator";
/// The SDK's driver is not running.
pub const DRIVER_STOPPED: &str = "AMD's driver is not running";
/// The SDK refuses to work in a virtual machine.
pub const VIRTUAL_MACHINE: &str = "not available in a virtual machine";
/// The SDK answered, but with zero for this value.
pub const ZEROS: &str = "AMD's SDK reports zero";
/// `Platform.dll` could not be loaded, or its `Init` refused.
pub const SDK_DID_NOT_START: &str = "AMD's SDK did not start";
/// The SDK started but gave no usable CPU device.
pub const SDK_NO_CPU: &str = "AMD's SDK found no processor";
/// `GetCPUParameters` answered `bUnsupportedFunction`.
pub const SDK_UNSUPPORTED: &str = "AMD's SDK does not support this processor";
/// `GetCPUParameters` answered another failure.
pub const SDK_READ_FAILED: &str = "AMD's SDK could not read the processor";

/// The oldest SDK whose interface matches the binding (major, minor).
pub const MIN_SDK: (u16, u16) = (3, 1);
/// AMD: `GetCPUParameters` at most once a second (it loads the SMU).
pub const MIN_CALL_INTERVAL: Duration = Duration::from_secs(1);

/// What decides whether the SDK can work on this PC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    /// The processor is an AMD one.
    pub amd: bool,
    /// The SDK's folder, when its registry key and DLL are there.
    pub sdk: Option<PathBuf>,
    /// `Platform.dll`'s file version (a.b.c.d), when it has one.
    pub version: Option<[u16; 4]>,
    pub virtual_machine: bool,
    pub admin: bool,
    pub driver_running: bool,
}

/// Whether the SDK can be read here: its folder, or the first reason it
/// cannot (in the order the user has to solve them).
pub fn diagnose(p: &Probe) -> Result<PathBuf, &'static str> {
    if !p.amd {
        return Err(NOT_AMD);
    }
    let Some(sdk) = &p.sdk else { return Err(NEEDS_SDK) };
    if !p.version.is_some_and(|v| (v[0], v[1]) >= MIN_SDK) {
        Err(SDK_TOO_OLD)
    } else if p.virtual_machine {
        Err(VIRTUAL_MACHINE)
    } else if !p.admin {
        Err(NEEDS_ADMIN)
    } else if !p.driver_running {
        Err(DRIVER_STOPPED)
    } else {
        Ok(sdk.clone())
    }
}

/// How starting or calling the SDK went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// `Platform.dll` (or a DLL it needs) did not load.
    NotLoaded,
    /// No `extern "C"` `GetPlatform` export (an SDK older than 3.x).
    NoEntryPoint,
    /// `IPlatform::Init` returned false.
    InitRefused,
    /// No device, not a CPU, or no plausible core count.
    NoCpu,
    /// `GetCPUParameters` returned this `AOD_STATUS_CODE` (not `bSuccess`).
    Status(i32),
}

/// The reason a [`Failure`] gives on the rows and in "Sources on this PC".
pub fn failure_reason(f: Failure) -> &'static str {
    match f {
        Failure::NotLoaded | Failure::InitRefused => SDK_DID_NOT_START,
        Failure::NoEntryPoint => SDK_TOO_OLD,
        Failure::NoCpu => SDK_NO_CPU,
        Failure::Status(4) => SDK_UNSUPPORTED,
        Failure::Status(_) => SDK_READ_FAILED,
    }
}

/// What the SDK's `GetCPUParameters` gives, in plain units (0 = not reported).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CpuParameters {
    /// fPPTValue: the package power.
    pub package_power_w: f64,
    /// fVDDCR_VDD_Power: all cores.
    pub cores_power_w: f64,
    /// fVDDCR_SOC_Power.
    pub soc_power_w: f64,
    /// fPPTLimit.
    pub ppt_limit_w: f64,
    /// dTemperature.
    pub temperature_c: f64,
    /// dAvgCoreVoltage.
    pub core_voltage_v: f64,
    /// dSocVoltage.
    pub soc_voltage_v: f64,
    /// fFCLKP0Freq (MHz).
    pub fclk_mhz: f64,
}

/// `EffectiveFreqData` of `IDevice.h` (SDK 3.1). The arrays are optional:
/// with `uLength` 0 and null pointers the SDK fills only the scalars.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawFreqData {
    pub length: u32,
    pub freq: *mut f64,
    pub state: *mut f64,
    pub current_freq: *mut f64,
    pub current_temp: *mut f64,
}

/// `CPUParameters` of `IDevice.h` (SDK 3.1), field for field. `repr(C)`
/// pads like MSVC: 4 bytes after `eMode` (the next field is 8-aligned).
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RawCpuParameters {
    pub oc_mode: u32,
    pub freq_data: RawFreqData,
    pub peak_core_voltage: f64,
    pub peak_core_voltage_1: f64,
    pub soc_voltage: f64,
    pub temperature: f64,
    pub avg_core_voltage: f64,
    pub avg_core_voltage_1: f64,
    pub peak_speed: f64,
    pub ppt_limit: f32,
    pub ppt_value: f32,
    pub tdc_limit_vdd: f32,
    pub tdc_value_vdd: f32,
    pub tdc_value_vdd_1: f32,
    pub edc_limit_vdd: f32,
    pub edc_value_vdd: f32,
    pub edc_value_vdd_1: f32,
    pub chtc_limit: f32,
    pub fclk_p0_freq: f32,
    pub cclk_fmax: f32,
    pub tdc_limit_soc: f32,
    pub tdc_value_soc: f32,
    pub edc_limit_soc: f32,
    pub edc_value_soc: f32,
    pub vddcr_vdd_power: f32,
    pub vddcr_soc_power: f32,
    pub tdc_limit_ccd: f32,
    pub tdc_value_ccd: f32,
    pub edc_limit_ccd: f32,
    pub edc_value_ccd: f32,
}

impl Default for RawCpuParameters {
    fn default() -> Self {
        // SAFETY: plain numbers and raw pointers; all zero bits is valid
        // (null pointers, length 0).
        unsafe { std::mem::zeroed() }
    }
}

/// The SDK's answer in plain units. A value that is not a finite positive
/// number stays 0 (shown as [`ZEROS`]).
pub fn parameters(raw: &RawCpuParameters) -> CpuParameters {
    let f = |v: f64| if v.is_finite() && v > 0.0 { v } else { 0.0 };
    CpuParameters {
        package_power_w: f(raw.ppt_value as f64),
        cores_power_w: f(raw.vddcr_vdd_power as f64),
        soc_power_w: f(raw.vddcr_soc_power as f64),
        ppt_limit_w: f(raw.ppt_limit as f64),
        temperature_c: f(raw.temperature),
        core_voltage_v: f(raw.avg_core_voltage),
        soc_voltage_v: f(raw.soc_voltage),
        fclk_mhz: f(raw.fclk_p0_freq as f64),
    }
}

/// What one `GetCPUParameters` call answered: the status code and the
/// filled struct.
pub fn answer(status: i32, raw: &RawCpuParameters) -> Result<CpuParameters, &'static str> {
    if status == 0 {
        Ok(parameters(raw))
    } else {
        Err(failure_reason(Failure::Status(status)))
    }
}

/// The Ryzen rows: with the SDK's values, or every one unavailable for
/// `Err(why)`. A zero from the SDK is unavailable ([`ZEROS`]), never 0.
/// `cpu_name` is the CPU device's name (shared with the other providers).
pub fn readings(params: Result<&CpuParameters, &str>, cpu_name: &str, at: Instant) -> Vec<Reading> {
    let cpu = Device::cpu(cpu_name);
    let p = params.ok().copied().unwrap_or_default();
    let why = params.err().unwrap_or(ZEROS);
    let rows: [(&str, Group, &str, Unit, f64); 8] = [
        (super::cpu::PACKAGE_TEMPERATURE, Group::Temperatures, "CPU Package", Unit::Celsius, p.temperature_c),
        ("cpu.fabric.frequency_hz", Group::Clocks, "Fabric Clock", Unit::Hz, p.fclk_mhz * 1e6),
        (super::energy::PACKAGE, Group::Powers, "CPU Package", Unit::Watt, p.package_power_w),
        ("cpu.package.cores_power_w", Group::Powers, "Cores", Unit::Watt, p.cores_power_w),
        ("cpu.soc.power_w", Group::Powers, "SoC", Unit::Watt, p.soc_power_w),
        ("cpu.power.ppt_limit_w", Group::Powers, "PPT Limit", Unit::Watt, p.ppt_limit_w),
        ("cpu.voltage.core_v", Group::Voltages, "Core", Unit::Volt, p.core_voltage_v),
        ("cpu.voltage.soc_v", Group::Voltages, "SoC", Unit::Volt, p.soc_voltage_v),
    ];
    let mut out: Vec<Reading> = rows
        .into_iter()
        .map(|(id, group, name, unit, v)| Reading::measure(id, cpu.clone(), group, name, unit, SOURCE, at).with((v > 0.0).then_some(v), why))
        .collect();
    // This PC › Powers › SoC (Energy Meter lists the row; a value fills it).
    if p.soc_power_w > 0.0 {
        out.push(Reading::measure("pc.power.soc_w", Device::this_pc(), Group::Powers, "SoC", Unit::Watt, SOURCE, at).with(Some(p.soc_power_w), ""));
    }
    out
}

/// The SDK once loaded: one call per read. Dropping it uninitialises the
/// SDK and unloads its DLL.
pub trait Sdk {
    fn cpu_parameters(&mut self) -> Result<CpuParameters, String>;
}

/// The SDK, started, as every user shares it.
type Started = Arc<Mutex<Box<dyn Sdk + Send>>>;

/// The SDK's one session in this process. `GetPlatform` returns the same
/// `IPlatform` to every caller, so the SDK cannot be started per provider:
/// a new run of the sensor service starts while the old worker may still
/// read, and the old provider's `UnInit` + `FreeLibrary` would pull the SDK
/// from under the new one. Every provider holds a [`Lease`] instead: the
/// first one starts the SDK, the last one dropped closes it (so no SDK DLL
/// stays loaded once nobody reads it).
pub struct Session {
    users: Mutex<Users>,
}

struct Users {
    count: usize,
    sdk: Option<Started>,
}

impl Session {
    pub const fn new() -> Self {
        Session { users: Mutex::new(Users { count: 0, sdk: None }) }
    }

    fn lock(&self) -> MutexGuard<'_, Users> {
        self.users.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A lease on the session; `open` starts the SDK when nobody holds one
    /// (its failure is returned, with nothing held).
    pub fn acquire(&'static self, open: impl FnOnce() -> Result<Box<dyn Sdk + Send>, String>) -> Result<Lease, String> {
        let mut users = self.lock();
        let sdk = match &users.sdk {
            Some(sdk) => sdk.clone(),
            None => {
                let sdk: Started = Arc::new(Mutex::new(open()?));
                users.sdk = Some(sdk.clone());
                sdk
            }
        };
        users.count += 1;
        Ok(Lease { session: self, sdk: Some(sdk) })
    }
}

/// One provider's use of the [`Session`]; reads go one at a time.
pub struct Lease {
    session: &'static Session,
    sdk: Option<Started>,
}

impl Sdk for Lease {
    fn cpu_parameters(&mut self) -> Result<CpuParameters, String> {
        let sdk = self.sdk.as_ref().expect("held until dropped");
        sdk.lock().unwrap_or_else(|e| e.into_inner()).cpu_parameters()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut users = self.session.lock();
        drop(self.sdk.take());
        users.count -= 1;
        if users.count == 0 {
            // The last user: the SDK closes here, under the lock, so a new
            // lease never meets a half-closed SDK.
            drop(users.sdk.take());
        }
    }
}

/// `AOD_DEVICE_TYPE::dtCPU`.
const DT_CPU: i32 = 0;
/// `AOD_STATUS_CODE::bSuccess`.
const B_SUCCESS: i32 = 0;
/// The two `IDeviceManager::GetDevice` overloads' vtable slots, in either order.
const MANAGER_GET_DEVICE: [usize; 2] = [2, 3];
/// The most cores a plausible `GetCoreCount` answers.
const MAX_CORES: u32 = 256;

/// The SDK's device calls once it started (a seam: `load` calls the C++
/// vtable slots, tests fake them).
pub trait Devices {
    type Device: Copy;
    /// `IPlatform::GetIDeviceManager` gave a manager (not null).
    fn manager(&mut self) -> bool;
    /// `IDeviceManager::GetDevice(dtCPU, 0)` through vtable `slot`; `None` when null.
    fn device(&mut self, slot: usize) -> Option<Self::Device>;
    /// `IDevice::GetType`.
    fn device_type(&mut self, device: Self::Device) -> i32;
    /// `ICPUEx::GetCoreCount`: its status and the count.
    fn core_count(&mut self, device: Self::Device) -> (i32, u32);
    /// `IPlatform::UnInit`.
    fn uninit(&mut self);
    /// `FreeLibrary` of `Platform.dll`.
    fn unload(&mut self);
}

/// The SDK's CPU device: the first `GetDevice` slot giving a device that
/// says it is a CPU, with a plausible core count. Without one the SDK is
/// closed (`UnInit`, then `FreeLibrary`, once each): [`Failure::NoCpu`].
pub fn find_cpu<D: Devices>(sdk: &mut D) -> Result<D::Device, Failure> {
    let mut cpu = None;
    if sdk.manager() {
        for slot in MANAGER_GET_DEVICE {
            if let Some(device) = sdk.device(slot) {
                if sdk.device_type(device) == DT_CPU {
                    cpu = Some(device);
                    break;
                }
            }
        }
    }
    if let Some(device) = cpu {
        let (status, cores) = sdk.core_count(device);
        if status == B_SUCCESS && (1..=MAX_CORES).contains(&cores) {
            return Ok(device);
        }
    }
    sdk.uninit();
    sdk.unload();
    Err(Failure::NoCpu)
}

type Loader = fn(&std::path::Path) -> Result<Box<dyn Sdk>, String>;

/// The provider. Built on its worker thread; the SDK (if any) is loaded in
/// `availability` and dropped with the provider.
pub struct RyzenMaster {
    probe: Probe,
    name: String,
    loader: Loader,
    sdk: Option<Box<dyn Sdk>>,
    /// The last SDK answer and when it came ([`MIN_CALL_INTERVAL`]).
    last: Option<(Instant, Result<CpuParameters, String>)>,
}

impl RyzenMaster {
    pub fn new() -> Self {
        let name = super::cpu::cpu_name();
        RyzenMaster { probe: probe(&name), name, loader: imp::load, sdk: None, last: None }
    }
}

impl Provider for RyzenMaster {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        let folder = diagnose(&self.probe)?;
        self.sdk = Some((self.loader)(&folder)?);
        Ok(())
    }

    /// Only a PC with an AMD processor lists AMD's SDK.
    fn for_this_pc(&mut self) -> bool {
        self.probe.amd
    }

    fn unavailable_readings(&mut self, why: &str, at: Instant) -> Vec<Reading> {
        if self.probe.amd {
            readings(Err(why), &self.name, at)
        } else {
            Vec::new()
        }
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let Some(sdk) = self.sdk.as_mut() else {
            return readings(Err(SDK_DID_NOT_START), &self.name, at);
        };
        let fresh = self.last.as_ref().is_some_and(|(when, _)| at.saturating_duration_since(*when) < MIN_CALL_INTERVAL);
        if !fresh {
            self.last = Some((at, sdk.cpu_parameters()));
        }
        match &self.last {
            Some((_, Ok(p))) => readings(Ok(p), &self.name, at),
            Some((_, Err(why))) => readings(Err(why), &self.name, at),
            None => unreachable!("just read"),
        }
    }
}

/// Whether the BIOS names a virtual machine ("VMware, Inc.", "QEMU",
/// "VirtualBox", Hyper-V's "Virtual Machine").
pub fn looks_virtual(bios: &str) -> bool {
    let b = bios.to_ascii_lowercase();
    ["vmware", "virtualbox", "qemu", "kvm", "virtual machine", "xen", "parallels"].iter().any(|v| b.contains(v))
}

/// Where `Platform.dll` is in the SDK's folder.
fn platform_dll(folder: &std::path::Path) -> PathBuf {
    folder.join("bin").join("Platform.dll")
}

/// What this PC says about the SDK.
fn probe(cpu_name: &str) -> Probe {
    imp::probe(cpu_name.to_ascii_uppercase().contains("AMD"))
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

    fn ready() -> Probe {
        Probe { amd: true, sdk: Some(PathBuf::from(r"C:\AMD\SDK")), version: Some([3, 1, 1, 5478]), virtual_machine: false, admin: true, driver_running: true }
    }

    #[test]
    fn the_sdk_is_read_only_on_an_amd_pc_with_the_sdk_elevated_and_its_driver() {
        assert_eq!(diagnose(&ready()), Ok(PathBuf::from(r"C:\AMD\SDK")));
        assert_eq!(diagnose(&Probe { amd: false, ..ready() }), Err(NOT_AMD));
        assert_eq!(diagnose(&Probe { sdk: None, admin: false, ..ready() }), Err(NEEDS_SDK), "the add-on comes first");
        assert_eq!(diagnose(&Probe { virtual_machine: true, ..ready() }), Err(VIRTUAL_MACHINE));
        assert_eq!(diagnose(&Probe { admin: false, driver_running: false, ..ready() }), Err(NEEDS_ADMIN));
        assert_eq!(diagnose(&Probe { driver_running: false, ..ready() }), Err(DRIVER_STOPPED));
    }

    #[test]
    fn an_sdk_older_than_3_1_or_without_a_version_is_too_old() {
        assert_eq!(diagnose(&Probe { version: Some([2, 6, 0, 1000]), admin: false, ..ready() }), Err(SDK_TOO_OLD), "before administrator");
        assert_eq!(diagnose(&Probe { version: Some([3, 0, 1, 4971]), ..ready() }), Err(SDK_TOO_OLD));
        assert_eq!(diagnose(&Probe { version: None, ..ready() }), Err(SDK_TOO_OLD));
        assert!(diagnose(&Probe { version: Some([3, 2, 0, 0]), ..ready() }).is_ok());
        assert!(diagnose(&Probe { version: Some([4, 0, 0, 0]), ..ready() }).is_ok());
    }

    #[test]
    fn a_vm_is_told_by_its_bios() {
        assert!(looks_virtual("VMware, Inc. VMware7,1"));
        assert!(looks_virtual("Microsoft Corporation Virtual Machine"));
        assert!(!looks_virtual("Micro-Star International Co., Ltd. MS-7D75"));
    }

    #[test]
    fn the_raw_struct_has_the_layout_of_the_sdk_3_1_x64_header() {
        use std::mem::{offset_of, size_of};
        assert_eq!(size_of::<RawFreqData>(), 40);
        assert_eq!(size_of::<RawCpuParameters>(), 192);
        assert_eq!(offset_of!(RawCpuParameters, freq_data), 8);
        assert_eq!(offset_of!(RawCpuParameters, soc_voltage), 64);
        assert_eq!(offset_of!(RawCpuParameters, temperature), 72);
        assert_eq!(offset_of!(RawCpuParameters, avg_core_voltage), 80);
        assert_eq!(offset_of!(RawCpuParameters, ppt_limit), 104);
        assert_eq!(offset_of!(RawCpuParameters, ppt_value), 108);
        assert_eq!(offset_of!(RawCpuParameters, fclk_p0_freq), 140);
        assert_eq!(offset_of!(RawCpuParameters, vddcr_vdd_power), 164);
        assert_eq!(offset_of!(RawCpuParameters, vddcr_soc_power), 168);
        assert_eq!(offset_of!(RawCpuParameters, edc_value_ccd), 184);
    }

    /// A sample answer of a Ryzen 5 7600X under load.
    fn sample() -> RawCpuParameters {
        RawCpuParameters {
            temperature: 71.5,
            avg_core_voltage: 1.25,
            peak_core_voltage: 1.35,
            soc_voltage: 1.05,
            ppt_limit: 142.0,
            ppt_value: 88.0,
            vddcr_vdd_power: 60.5,
            vddcr_soc_power: 9.25,
            fclk_p0_freq: 2000.0,
            ..RawCpuParameters::default()
        }
    }

    fn text(rows: &[Reading], id: &str) -> String {
        rows.iter().find(|r| r.id == id).unwrap_or_else(|| panic!("no {id}")).text()
    }

    #[test]
    fn a_successful_sdk_answer_fills_every_ryzen_row() {
        let p = answer(0, &sample()).unwrap();
        let rows = readings(Ok(&p), "AMD Ryzen 5 7600X 6-Core Processor", Instant::now());
        assert_eq!(text(&rows, "cpu.package.temperature_c"), "71.5 °C");
        assert_eq!(text(&rows, "cpu.fabric.frequency_hz"), "2.00 GHz");
        assert_eq!(text(&rows, "cpu.package.power_w"), "88.0 W");
        assert_eq!(text(&rows, "cpu.package.cores_power_w"), "60.5 W");
        assert_eq!(text(&rows, "cpu.soc.power_w"), "9.2 W");
        assert_eq!(text(&rows, "cpu.power.ppt_limit_w"), "142.0 W");
        assert_eq!(text(&rows, "cpu.voltage.core_v"), "1.250 V");
        assert_eq!(text(&rows, "cpu.voltage.soc_v"), "1.050 V");
        assert_eq!(text(&rows, "pc.power.soc_w"), "9.2 W", "fills This PC › Powers › SoC");
        assert!(rows.iter().all(|r| r.source == SOURCE));
        assert!(rows.iter().filter(|r| r.id.starts_with("cpu.")).all(|r| r.device.name == "AMD Ryzen 5 7600X 6-Core Processor"), "merges under the CPU device");
    }

    #[test]
    fn an_all_zero_answer_greys_every_row_with_zeros() {
        let p = answer(0, &RawCpuParameters::default()).unwrap();
        let rows = readings(Ok(&p), "AMD", Instant::now());
        assert_eq!(rows.len(), 8, "no This PC SoC row without a value");
        assert!(rows.iter().all(|r| r.value.is_none() && r.text() == "AMD's SDK reports zero"));
    }

    #[test]
    fn negative_or_not_a_number_values_are_unavailable_too() {
        let raw = RawCpuParameters { temperature: f64::NAN, ppt_value: -3.0, soc_voltage: f64::INFINITY, ..sample() };
        let rows = readings(Ok(&parameters(&raw)), "AMD", Instant::now());
        assert_eq!(text(&rows, "cpu.package.temperature_c"), ZEROS);
        assert_eq!(text(&rows, "cpu.package.power_w"), ZEROS);
        assert_eq!(text(&rows, "cpu.voltage.soc_v"), ZEROS);
        assert_eq!(text(&rows, "cpu.voltage.core_v"), "1.250 V");
    }

    #[test]
    fn each_sdk_failure_has_its_reason() {
        assert_eq!(answer(4, &sample()), Err("AMD's SDK does not support this processor"));
        assert_eq!(answer(-1, &sample()), Err("AMD's SDK could not read the processor"));
        assert_eq!(answer(1, &sample()), Err(SDK_READ_FAILED));
        assert_eq!(failure_reason(Failure::NotLoaded), "AMD's SDK did not start");
        assert_eq!(failure_reason(Failure::InitRefused), SDK_DID_NOT_START);
        assert_eq!(failure_reason(Failure::NoEntryPoint), SDK_TOO_OLD, "the 2023 SDK has no extern \"C\" GetPlatform");
        assert_eq!(failure_reason(Failure::NoCpu), "AMD's SDK found no processor");
    }

    struct Fixed {
        answer: Option<CpuParameters>,
        calls: std::rc::Rc<std::cell::Cell<u32>>,
        dropped: std::rc::Rc<std::cell::Cell<bool>>,
    }
    impl Sdk for Fixed {
        fn cpu_parameters(&mut self) -> Result<CpuParameters, String> {
            self.calls.set(self.calls.get() + 1);
            self.answer.ok_or_else(|| SDK_READ_FAILED.to_string())
        }
    }
    impl Drop for Fixed {
        fn drop(&mut self) {
            self.dropped.set(true);
        }
    }

    thread_local! {
        static FAKE: std::cell::RefCell<Option<Fixed>> = const { std::cell::RefCell::new(None) };
    }

    /// A provider whose loader hands out `answer` (`None` = the SDK fails
    /// each read); returns the call counter and the dropped flag.
    fn provider(probe: Probe, answer: Option<CpuParameters>) -> (RyzenMaster, std::rc::Rc<std::cell::Cell<u32>>, std::rc::Rc<std::cell::Cell<bool>>) {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let dropped = std::rc::Rc::new(std::cell::Cell::new(false));
        FAKE.with(|f| *f.borrow_mut() = Some(Fixed { answer, calls: calls.clone(), dropped: dropped.clone() }));
        let loader: Loader = |_| Ok(Box::new(FAKE.with(|f| f.borrow_mut().take()).expect("loaded once")));
        (RyzenMaster { probe, name: "AMD Ryzen 5 7600X 6-Core Processor".into(), loader, sdk: None, last: None }, calls, dropped)
    }

    #[test]
    fn with_the_sdk_ready_the_ryzen_rows_have_values() {
        let (mut p, _, _) = provider(ready(), Some(parameters(&sample())));
        assert_eq!(p.availability(), Ok(()));
        let rows = p.read(Instant::now());
        assert_eq!(text(&rows, "cpu.package.temperature_c"), "71.5 °C");
        assert_eq!(text(&rows, "cpu.package.power_w"), "88.0 W");
    }

    #[test]
    fn the_sdk_is_called_at_most_once_a_second() {
        let (mut p, calls, _) = provider(ready(), Some(parameters(&sample())));
        p.availability().unwrap();
        let t0 = Instant::now();
        p.read(t0);
        let again = p.read(t0 + Duration::from_millis(500));
        assert_eq!(calls.get(), 1, "half a second later: the last answer again");
        assert_eq!(text(&again, "cpu.package.power_w"), "88.0 W");
        assert_eq!(again[0].at, t0 + Duration::from_millis(500), "dated now");
        p.read(t0 + Duration::from_millis(1000));
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn an_sdk_read_failure_greys_every_row_with_its_reason() {
        let (mut p, _, _) = provider(ready(), None);
        p.availability().unwrap();
        assert!(p.read(Instant::now()).iter().all(|r| r.text() == SDK_READ_FAILED));
    }

    #[test]
    fn the_sdk_is_released_when_the_provider_stops() {
        let (mut p, _, dropped) = provider(ready(), Some(parameters(&sample())));
        p.availability().unwrap();
        assert!(!dropped.get());
        drop(p);
        assert!(dropped.get(), "UnInit and FreeLibrary run in the SDK's Drop");
    }

    #[test]
    fn the_sdk_is_not_loaded_unless_its_probe_passes() {
        for probe in [Probe { sdk: None, ..ready() }, Probe { admin: false, ..ready() }, Probe { version: Some([3, 0, 0, 0]), ..ready() }] {
            let (mut p, _, _) = provider(probe.clone(), None);
            assert!(p.availability().is_err(), "{probe:?}");
            assert!(p.sdk.is_none(), "{probe:?}");
            assert!(FAKE.with(|f| f.borrow().is_some()), "the loader was not called: {probe:?}");
        }
    }

    #[test]
    fn without_the_sdk_every_ryzen_row_needs_amds_add_on_and_the_band_offers_it() {
        let (mut p, _, _) = provider(Probe { sdk: None, version: None, ..ready() }, None);
        let why = p.availability().unwrap_err();
        assert_eq!(why, NEEDS_SDK);
        let rows = p.unavailable_readings(&why, Instant::now());
        assert_eq!(rows.len(), 8);
        assert!(rows.iter().all(|r| r.value.is_none() && r.unavailable.as_deref() == Some(NEEDS_SDK)));
        assert!(rows.iter().all(|r| r.id != "pc.power.soc_w"), "Energy Meter's own reason stays on This PC");
        let sources = vec![crate::sensors::model::SourceStatus { name: SOURCE, problem: Some(why) }];
        let frame = crate::sensors::Frame::build(1, &[rows], sources);
        let n = crate::addons::power_freq::notice::notice(&frame, false).expect("a band");
        assert!(n.get_addon && !n.restart);
    }

    #[test]
    fn without_administrator_every_ryzen_row_says_so_and_the_band_offers_a_restart() {
        let (mut p, _, _) = provider(Probe { admin: false, ..ready() }, None);
        let why = p.availability().unwrap_err();
        assert_eq!(why, NEEDS_ADMIN);
        let rows = p.unavailable_readings(&why, Instant::now());
        assert!(rows.iter().all(|r| r.text() == NEEDS_ADMIN));
        let sources = vec![crate::sensors::model::SourceStatus { name: SOURCE, problem: Some(why) }];
        let frame = crate::sensors::Frame::build(1, &[rows], sources);
        let n = crate::addons::power_freq::notice::notice(&frame, false).expect("a band");
        assert!(n.restart && !n.get_addon);
    }

    #[test]
    fn on_another_processor_there_is_no_ryzen_row() {
        let (mut p, _, _) = provider(Probe { amd: false, ..Probe::default() }, None);
        let why = p.availability().unwrap_err();
        assert_eq!(why, NOT_AMD);
        assert!(p.unavailable_readings(&why, Instant::now()).is_empty());
        assert!(!p.for_this_pc(), "an Intel PC does not list AMD's SDK");
        let (mut amd, _, _) = provider(Probe { amd: true, ..Probe::default() }, None);
        assert!(amd.for_this_pc(), "a Ryzen without the SDK still lists it, with how to get it");
    }

    /// A ready provider with this loader (the tests' loaders take a lease
    /// on a session, as the real `sdk::load` does).
    fn sharing(loader: Loader) -> RyzenMaster {
        RyzenMaster { probe: ready(), name: "AMD Ryzen 5 7600X 6-Core Processor".into(), loader, sdk: None, last: None }
    }

    #[test]
    fn ryzen_overlapping_sessions_share_one_init() {
        use std::sync::atomic::{AtomicU32, Ordering::SeqCst};
        static SESSION: Session = Session::new();
        static INITS: AtomicU32 = AtomicU32::new(0);
        static UNINITS: AtomicU32 = AtomicU32::new(0);
        struct Counted;
        impl Sdk for Counted {
            fn cpu_parameters(&mut self) -> Result<CpuParameters, String> {
                Ok(parameters(&sample()))
            }
        }
        impl Drop for Counted {
            fn drop(&mut self) {
                UNINITS.fetch_add(1, SeqCst);
            }
        }
        let loader: Loader = |_| {
            SESSION
                .acquire(|| {
                    INITS.fetch_add(1, SeqCst);
                    Ok(Box::new(Counted))
                })
                .map(|lease| Box::new(lease) as Box<dyn Sdk>)
        };
        let (mut a, mut b) = (sharing(loader), sharing(loader));
        assert_eq!(a.availability(), Ok(()));
        assert_eq!(b.availability(), Ok(()));
        assert_eq!(INITS.load(SeqCst), 1, "one Init for the whole process");
        drop(a);
        assert_eq!(UNINITS.load(SeqCst), 0, "B still uses the SDK");
        let rows = b.read(Instant::now());
        assert_eq!(text(&rows, "cpu.package.power_w"), "88.0 W", "B still reads after A stopped");
        drop(b);
        assert_eq!(UNINITS.load(SeqCst), 1, "the last user closes the SDK, once");
        let mut c = sharing(loader);
        assert_eq!(c.availability(), Ok(()));
        assert_eq!(INITS.load(SeqCst), 2, "a later provider starts it again");
        drop(c);
        assert_eq!(UNINITS.load(SeqCst), 2);
    }

    #[test]
    fn a_session_that_fails_to_start_holds_nothing() {
        static SESSION: Session = Session::new();
        let loader: Loader = |_| SESSION.acquire(|| Err(SDK_DID_NOT_START.to_string())).map(|lease| Box::new(lease) as Box<dyn Sdk>);
        let mut p = sharing(loader);
        assert_eq!(p.availability(), Err(SDK_DID_NOT_START.to_string()));
        assert!(p.sdk.is_none());
        assert_eq!(SESSION.lock().count, 0);
        assert!(SESSION.lock().sdk.is_none());
    }

    #[test]
    fn service_new_run_while_old_worker_mid_read() {
        use crate::sensors::service::{Factory, SensorService};
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        use std::sync::Condvar;
        static SESSION: Session = Session::new();
        /// SDK sessions started and not closed yet, and the most at once.
        static LIVE: AtomicUsize = AtomicUsize::new(0);
        static MOST: AtomicUsize = AtomicUsize::new(0);
        /// Reads begun (each then waits for a permit).
        static READS: AtomicUsize = AtomicUsize::new(0);
        static PERMITS: Mutex<usize> = Mutex::new(0);
        static GATE: Condvar = Condvar::new();
        struct GatedSdk;
        impl Sdk for GatedSdk {
            fn cpu_parameters(&mut self) -> Result<CpuParameters, String> {
                READS.fetch_add(1, SeqCst);
                let permits = PERMITS.lock().unwrap();
                let mut permits = GATE.wait_while(permits, |n| *n == 0).unwrap();
                *permits -= 1;
                Ok(parameters(&sample()))
            }
        }
        impl Drop for GatedSdk {
            fn drop(&mut self) {
                LIVE.fetch_sub(1, SeqCst);
            }
        }
        fn permit(n: usize) {
            *PERMITS.lock().unwrap() += n;
            GATE.notify_all();
        }
        fn until(what: &str, cond: impl Fn() -> bool) {
            let end = Instant::now() + Duration::from_secs(5);
            while !cond() {
                assert!(Instant::now() < end, "timed out waiting: {what}");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        let loader: Loader = |_| {
            SESSION
                .acquire(|| {
                    let live = LIVE.fetch_add(1, SeqCst) + 1;
                    MOST.fetch_max(live, SeqCst);
                    Ok(Box::new(GatedSdk))
                })
                .map(|lease| Box::new(lease) as Box<dyn Sdk>)
        };
        let factory: Factory = Arc::new(move || Box::new(sharing(loader)) as Box<dyn Provider>);
        let service = SensorService::new(vec![factory]);
        let first = service.interest();
        until("the old worker is blocked mid-read", || READS.load(SeqCst) == 1);
        drop(first);
        let second = service.interest();
        until("the new run's worker took the SDK", || SESSION.lock().count == 2 || MOST.load(SeqCst) > 1);
        assert_eq!(MOST.load(SeqCst), 1, "at most one SDK session initialised at a time");
        permit(1_000);
        until("the old worker ended", || service.threads() == 1);
        let frame = second.latest();
        assert!(frame.is_none_or(|f| f.reading("cpu.package.power_w").is_some()));
        assert_eq!(LIVE.load(SeqCst), 1, "the new run still holds the session");
        drop(second);
        until("every worker ended", || service.threads() == 0);
        assert_eq!(LIVE.load(SeqCst), 0, "the SDK closed when its last user stopped");
        assert_eq!(MOST.load(SeqCst), 1);
    }

    /// `GetDevice` slots 2 and 3 answering a device of this `GetType`
    /// (`None` = null), every device with these cores.
    #[derive(Default)]
    struct FakeDevices {
        manager: bool,
        types: [Option<i32>; 2],
        cores: (i32, u32),
        uninits: u32,
        unloads: u32,
    }

    impl Devices for FakeDevices {
        type Device = usize;
        fn manager(&mut self) -> bool {
            self.manager
        }
        fn device(&mut self, slot: usize) -> Option<usize> {
            let i = MANAGER_GET_DEVICE.iter().position(|&s| s == slot).expect("a GetDevice slot");
            self.types[i].map(|_| slot)
        }
        fn device_type(&mut self, slot: usize) -> i32 {
            let i = MANAGER_GET_DEVICE.iter().position(|&s| s == slot).unwrap();
            self.types[i].unwrap()
        }
        fn core_count(&mut self, _: usize) -> (i32, u32) {
            self.cores
        }
        fn uninit(&mut self) {
            assert_eq!(self.unloads, 0, "UnInit before FreeLibrary");
            self.uninits += 1;
        }
        fn unload(&mut self) {
            self.unloads += 1;
        }
    }

    #[test]
    fn get_device_falls_back_to_second_slot() {
        let mut d = FakeDevices { manager: true, types: [Some(1), Some(0)], cores: (0, 8), ..Default::default() };
        assert_eq!(find_cpu(&mut d), Ok(3), "slot 2 gave a BIOS device: the CPU of slot 3");
        assert_eq!((d.uninits, d.unloads), (0, 0), "a found CPU keeps the SDK open");
        let mut d = FakeDevices { manager: true, types: [Some(0), Some(0)], cores: (0, 8), ..Default::default() };
        assert_eq!(find_cpu(&mut d), Ok(2), "the first CPU");
        let mut d = FakeDevices { manager: true, types: [None, None], cores: (0, 8), ..Default::default() };
        assert_eq!(find_cpu(&mut d), Err(Failure::NoCpu));
        assert_eq!((d.uninits, d.unloads), (1, 1), "UnInit and FreeLibrary once each");
        let mut d = FakeDevices { manager: false, types: [Some(0), Some(0)], cores: (0, 8), ..Default::default() };
        assert_eq!(find_cpu(&mut d), Err(Failure::NoCpu), "no device manager");
        assert_eq!((d.uninits, d.unloads), (1, 1));
    }

    #[test]
    fn implausible_core_count_is_no_cpu() {
        for cores in [(0, 0), (0, 257), (-1, 8), (4, 8)] {
            let mut d = FakeDevices { manager: true, types: [Some(0), None], cores, ..Default::default() };
            let got = find_cpu(&mut d);
            assert_eq!(got, Err(Failure::NoCpu), "{cores:?}");
            assert_eq!(failure_reason(got.unwrap_err()), SDK_NO_CPU);
            assert_eq!((d.uninits, d.unloads), (1, 1), "{cores:?}");
        }
        for cores in [(0, 1), (0, 256)] {
            let mut d = FakeDevices { manager: true, types: [Some(0), None], cores, ..Default::default() };
            assert_eq!(find_cpu(&mut d), Ok(2), "{cores:?}");
        }
    }

    /// This PC has no SDK: the real provider keeps working as before (no
    /// DLL loaded, every Ryzen row grey with the reason the probe gives,
    /// nothing on another processor).
    #[test]
    fn on_this_pc_without_the_sdk_the_real_provider_loads_nothing() {
        let mut p = RyzenMaster::new();
        if p.probe.sdk.is_some() {
            return; // the SDK is installed here: covered by the ignored test
        }
        let why = p.availability().unwrap_err();
        assert_eq!(why, if p.probe.amd { NEEDS_SDK } else { NOT_AMD });
        assert!(p.sdk.is_none());
        assert!(!imp::loaded(), "no SDK DLL mapped");
        let rows = p.unavailable_readings(&why, Instant::now());
        assert_eq!(rows.len(), if p.probe.amd { 8 } else { 0 });
        assert!(rows.iter().all(|r| r.unavailable.as_deref() == Some(why.as_str())));
    }

    #[test]
    #[ignore = "reads this PC"]
    fn reads_this_pcs_ryzen_sdk_state() {
        let mut p = RyzenMaster::new();
        println!("probe: {:?}", p.probe);
        println!("diagnose: {:?}", diagnose(&p.probe));
        let available = p.availability();
        println!("availability: {available:?}");
        let rows = match &available {
            Ok(()) => p.read(Instant::now()),
            Err(why) => p.unavailable_readings(why, Instant::now()),
        };
        for r in &rows {
            println!("    {:<14} {:<12} {}", r.device.name.chars().take(14).collect::<String>(), r.name, r.text());
        }
        println!("SDK DLL loaded while running: {}", imp::loaded());
        drop(p);
        println!("SDK DLL loaded after stop: {}", imp::loaded());
        assert!(!imp::loaded(), "no SDK library stays loaded after the add-on stops");
    }
}
