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
    #[allow(dead_code)] // the Windows reader uses it
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
