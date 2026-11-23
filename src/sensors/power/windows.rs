//! Windows: `GetSystemPowerStatus`, the battery IOCTLs and the ACPI thermal
//! zones (PDH `Thermal Zone Information`).

use crate::sensors::pdh::Query;
use crate::sensors::power::{BatteryDetails, PowerSample, PowerStatus, Zone};

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, DIGCF_DEVICEINTERFACE,
    DIGCF_PRESENT, GUID_DEVCLASS_BATTERY, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
};
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::Power::{
    BatteryDeviceName, BatteryInformation, GetSystemPowerStatus, BATTERY_CAPACITY_RELATIVE, BATTERY_INFORMATION, BATTERY_QUERY_INFORMATION,
    BATTERY_STATUS, BATTERY_WAIT_STATUS, IOCTL_BATTERY_QUERY_INFORMATION, IOCTL_BATTERY_QUERY_STATUS, IOCTL_BATTERY_QUERY_TAG, SYSTEM_POWER_STATUS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;

/// The power source, battery charge and battery saver as
/// `GetSystemPowerStatus` gives them; `None` when the call fails.
pub fn power_status() -> Option<PowerStatus> {
    // SAFETY: a plain out-struct.
    unsafe {
        let mut s: SYSTEM_POWER_STATUS = zeroed();
        (GetSystemPowerStatus(&mut s) != 0).then_some(PowerStatus {
            ac_line_status: s.ACLineStatus,
            battery_flag: s.BatteryFlag,
            battery_life_percent: s.BatteryLifePercent,
            system_status_flag: s.SystemStatusFlag,
            battery_life_time: s.BatteryLifeTime,
        })
    }
}

/// The first battery's name, capacities and cycle count (the
/// documented SetupDi + IOCTL_BATTERY_* sequence).
pub fn battery_details() -> Option<BatteryDetails> {
    // SAFETY: the device-info list is destroyed and the handle closed
    // before returning; every buffer is sized and passed with its size.
    unsafe {
        let set = SetupDiGetClassDevsW(&GUID_DEVCLASS_BATTERY, null(), null_mut(), DIGCF_PRESENT | DIGCF_DEVICEINTERFACE);
        if set == -1 {
            return None;
        }
        let path = interface_path(set);
        SetupDiDestroyDeviceInfoList(set);
        let path = path?;
        let h = CreateFileW(path.as_ptr(), GENERIC_READ | GENERIC_WRITE, FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, null_mut());
        if h == INVALID_HANDLE_VALUE {
            return None;
        }
        let details = query(h);
        CloseHandle(h);
        details
    }
}

unsafe fn interface_path(set: isize) -> Option<Vec<u16>> {
    let mut data: SP_DEVICE_INTERFACE_DATA = zeroed();
    data.cbSize = size_of::<SP_DEVICE_INTERFACE_DATA>() as u32;
    if SetupDiEnumDeviceInterfaces(set, null(), &GUID_DEVCLASS_BATTERY, 0, &mut data) == 0 {
        return None;
    }
    let mut need = 0u32;
    SetupDiGetDeviceInterfaceDetailW(set, &data, null_mut(), 0, &mut need, null_mut());
    if need == 0 {
        return None;
    }
    let mut buf = vec![0u64; need as usize / 8 + 1];
    let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
    (*detail).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
    if SetupDiGetDeviceInterfaceDetailW(set, &data, detail, need, null_mut(), null_mut()) == 0 {
        return None;
    }
    let p = (*detail).DevicePath.as_ptr();
    let mut n = 0;
    while *p.add(n) != 0 {
        n += 1;
    }
    Some(std::slice::from_raw_parts(p, n + 1).to_vec())
}

unsafe fn query(h: HANDLE) -> Option<BatteryDetails> {
    let mut ret = 0u32;
    let (wait, mut tag) = (0u32, 0u32);
    let ok = DeviceIoControl(h, IOCTL_BATTERY_QUERY_TAG, (&wait as *const u32).cast(), 4, (&mut tag as *mut u32).cast(), 4, &mut ret, null_mut());
    if ok == 0 || tag == 0 {
        return None;
    }
    let mut q = BATTERY_QUERY_INFORMATION { BatteryTag: tag, InformationLevel: BatteryInformation, AtRate: 0 };
    let mut info: BATTERY_INFORMATION = zeroed();
    let size = size_of::<BATTERY_QUERY_INFORMATION>() as u32;
    let info_size = size_of::<BATTERY_INFORMATION>() as u32;
    if DeviceIoControl(h, IOCTL_BATTERY_QUERY_INFORMATION, (&q as *const BATTERY_QUERY_INFORMATION).cast(), size, (&mut info as *mut BATTERY_INFORMATION).cast(), info_size, &mut ret, null_mut()) == 0 {
        return None;
    }
    q.InformationLevel = BatteryDeviceName;
    let mut name = [0u16; 128];
    let named = DeviceIoControl(h, IOCTL_BATTERY_QUERY_INFORMATION, (&q as *const BATTERY_QUERY_INFORMATION).cast(), size, name.as_mut_ptr().cast(), 256, &mut ret, null_mut()) != 0;
    let name = if named { String::from_utf16_lossy(&name[..(ret as usize / 2).min(128)]).trim_end_matches('\0').trim().to_string() } else { String::new() };
    // Timeout 0 and PowerState 0: return the current status at once.
    let wait = BATTERY_WAIT_STATUS { BatteryTag: tag, Timeout: 0, PowerState: 0, LowCapacity: 0, HighCapacity: 0 };
    let mut status: BATTERY_STATUS = zeroed();
    let read = DeviceIoControl(h, IOCTL_BATTERY_QUERY_STATUS, (&wait as *const BATTERY_WAIT_STATUS).cast(), size_of::<BATTERY_WAIT_STATUS>() as u32, (&mut status as *mut BATTERY_STATUS).cast(), size_of::<BATTERY_STATUS>() as u32, &mut ret, null_mut()) != 0;
    Some(BatteryDetails {
        name,
        designed_capacity: info.DesignedCapacity,
        full_charged_capacity: info.FullChargedCapacity,
        cycle_count: info.CycleCount,
        relative: info.Capabilities & BATTERY_CAPACITY_RELATIVE != 0,
        rate: read.then_some(status.Rate),
    })
}

/// The power provider's reads: the power status, the battery and the
/// thermal zones.
pub struct Reader {
    zones: Option<Query>,
}

impl Reader {
    pub fn open() -> Self {
        Reader {
            zones: Query::open(&[
                ("hp", r"\Thermal Zone Information(*)\High Precision Temperature"),
                ("k", r"\Thermal Zone Information(*)\Temperature"),
                ("passive", r"\Thermal Zone Information(*)\% Passive Limit"),
            ]),
        }
    }

    pub fn availability(&self) -> Result<(), String> {
        Ok(())
    }

    pub fn sample(&mut self) -> PowerSample {
        let mut s = PowerSample { status: power_status(), ..Default::default() };
        if s.status.is_some_and(|st| st.battery_flag & 128 == 0 || st.battery_flag == 255) {
            s.battery = battery_details();
        }
        if let Some(q) = self.zones.as_mut() {
            q.collect();
            for (inst, v) in q.values("hp") {
                zone_named(&mut s.zones, &inst).tenths_kelvin = Some(v);
            }
            for (inst, v) in q.values("k") {
                let z = zone_named(&mut s.zones, &inst);
                z.tenths_kelvin = z.tenths_kelvin.or(Some(v * 10.0));
            }
            for (inst, v) in q.values("passive") {
                zone_named(&mut s.zones, &inst).passive_limit = Some(v);
            }
        }
        s
    }
}

/// The zone of a `Thermal Zone Information` instance ("\_TZ.TZ00" is
/// "TZ00"), added if new.
fn zone_named<'a>(zones: &'a mut Vec<Zone>, instance: &str) -> &'a mut Zone {
    let name = instance.rsplit(['.', '\\']).next().unwrap_or(instance).to_string();
    let i = match zones.iter().position(|z| z.name == name) {
        Some(i) => i,
        None => {
            zones.push(Zone { name, ..Default::default() });
            zones.len() - 1
        }
    };
    &mut zones[i]
}
