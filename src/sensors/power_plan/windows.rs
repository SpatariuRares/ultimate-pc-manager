//! Windows: the active power plan, its processor state levels and the power
//! mode, read with the power plan API.

use crate::sensors::power_plan::PlanSample;
use std::mem::zeroed;
use std::ptr::{null, null_mut};
use windows_sys::core::GUID;
use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::System::Power::{GetSystemPowerStatus, PowerGetActiveScheme, PowerReadACValueIndex, PowerReadDCValueIndex, PowerReadFriendlyName, SYSTEM_POWER_STATUS};

const PROCESSOR: GUID = GUID::from_u128(0x54533251_82be_4824_96c1_47b60b740d00);
const MIN_STATE: GUID = GUID::from_u128(0x893dee8e_2bef_41e0_89c6_b55d0929964c);
const MAX_STATE: GUID = GUID::from_u128(0xbc5038f7_23e0_4960_96da_33abaf5935ec);
const EPP: GUID = GUID::from_u128(0x36687f9e_e3a5_4dbf_b1dc_15eb381c6863);

/// The active scheme's GUID.
fn active() -> Option<GUID> {
    // SAFETY: Windows allocates the GUID with LocalAlloc; it is copied
    // out and freed once.
    unsafe {
        let mut p: *mut GUID = null_mut();
        if PowerGetActiveScheme(null_mut(), &mut p) != 0 || p.is_null() {
            return None;
        }
        let g = *p;
        LocalFree(p.cast());
        Some(g)
    }
}

fn name_of(scheme: &GUID) -> Option<String> {
    // SAFETY: the first call asks the size, the second fills a buffer of it.
    unsafe {
        let mut size = 0u32;
        if PowerReadFriendlyName(null_mut(), scheme, null(), null(), null_mut(), &mut size) != 0 || size < 2 {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2];
        if PowerReadFriendlyName(null_mut(), scheme, null(), null(), buf.as_mut_ptr().cast(), &mut size) != 0 {
            return None;
        }
        let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..end]).trim().to_string()).filter(|s| !s.is_empty())
    }
}

pub fn plan_name() -> Option<String> {
    name_of(&active()?)
}

/// On battery the plan's DC values apply.
fn on_battery() -> bool {
    // SAFETY: plain out-parameter.
    unsafe {
        let mut s: SYSTEM_POWER_STATUS = zeroed();
        GetSystemPowerStatus(&mut s) != 0 && s.ACLineStatus == 0
    }
}

fn value(scheme: &GUID, setting: &GUID, battery: bool) -> Option<u32> {
    let mut v = 0u32;
    // SAFETY: GUID pointers outlive the call; plain out-parameter.
    let r = unsafe {
        if battery {
            PowerReadDCValueIndex(null_mut(), scheme, &PROCESSOR, setting, &mut v)
        } else {
            PowerReadACValueIndex(null_mut(), scheme, &PROCESSOR, setting, &mut v)
        }
    };
    (r == 0).then_some(v)
}

/// The effective power mode overlay, as one number (loaded at run time:
/// not every Windows has it).
pub fn overlay() -> Option<u128> {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    // SAFETY: the function has this signature since Windows 10 1709.
    unsafe {
        let lib: Vec<u16> = "powrprof.dll\0".encode_utf16().collect();
        let h = LoadLibraryW(lib.as_ptr());
        if h.is_null() {
            return None;
        }
        let f = GetProcAddress(h, b"PowerGetEffectiveOverlayScheme\0".as_ptr())?;
        let f: unsafe extern "system" fn(*mut GUID) -> u32 = std::mem::transmute(f);
        let mut g: GUID = zeroed();
        if f(&mut g) != 0 {
            return None;
        }
        Some(((g.data1 as u128) << 96) | ((g.data2 as u128) << 80) | ((g.data3 as u128) << 64) | u64::from_be_bytes(g.data4) as u128)
    }
}

pub fn sample(cpu: &str) -> PlanSample {
    let mut s = PlanSample { cpu: cpu.into(), mode: super::current_power_mode(), ..Default::default() };
    let Some(scheme) = active() else { return s };
    let battery = on_battery();
    s.plan = name_of(&scheme);
    s.min_state = value(&scheme, &MIN_STATE, battery);
    s.max_state = value(&scheme, &MAX_STATE, battery);
    s.epp = value(&scheme, &EPP, battery);
    s
}

/// Whether the power plan can be read.
pub fn availability() -> Result<(), String> {
    Ok(())
}
