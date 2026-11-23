//! Windows: Intel DTT runs as the `esifsvc` service on an Intel processor
//! (vendor id from the registry).

use crate::sensors::intel_dtt::{availability, for_a_cpu_from, SERVICE};

/// Whether DTT runs here: an Intel processor with its service running.
pub fn provider_availability() -> Result<(), String> {
    availability(&cpu_vendor(), crate::services::service_running(SERVICE))
}

/// Whether this PC's processor is one DTT is for.
pub fn for_this_pc() -> bool {
    for_a_cpu_from(&cpu_vendor())
}

/// The CPU's vendor id ("GenuineIntel", "AuthenticAMD").
fn cpu_vendor() -> String {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
        .and_then(|k| k.get_value::<String, _>("VendorIdentifier"))
        .unwrap_or_default()
}
