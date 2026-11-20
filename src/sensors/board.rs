//! The firmware provider ("Firmware tables"): the CPU's Bus Clock and the
//! Motherboard and Memory devices from SMBIOS. Board voltages, temperatures
//! and fans and memory temperatures need a vendor provider (Super I/O,
//! SPD hub); until one exists their rows stay grey with the reason. A later
//! provider fills them by emitting the same metric ids.

use std::time::Instant;

use super::model::{Device, DeviceKind, Group, Reading, Unit};
use super::provider::Provider;
use super::smbios::Firmware;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "Firmware tables";
/// The CPU's bus clock (BCLK), CPU › Clocks.
pub const BUS_CLOCK: &str = "cpu.package.frequency.bus_hz";
/// Why a board or memory sensor has no value.
pub const NO_PROVIDER: &str = "no provider on this PC";

/// The readings of what the firmware reported (`None` = no firmware table).
pub fn readings(fw: Option<&Firmware>, cpu_name: &str, at: Instant) -> Vec<Reading> {
    const NO_TABLE: &str = "no firmware table on this PC";
    let missing = if fw.is_some() { "the firmware does not report it" } else { NO_TABLE };
    let cpu = Device::cpu(cpu_name);
    let board = Device::new(DeviceKind::Motherboard, "board", fw.and_then(|f| f.board.as_deref()).unwrap_or("Motherboard"));
    let modules = fw.map_or(&[][..], |f| &f.modules[..]);
    let total_gb = modules.iter().map(|m| m.size_mb).sum::<u64>() / 1024;
    let memory_name = match (total_gb, modules.iter().find_map(|m| m.ddr)) {
        (0, _) => "Memory".to_string(),
        (gb, Some(ddr)) => format!("{gb} GB {ddr}"),
        (gb, None) => format!("{gb} GB"),
    };
    let memory = Device::new(DeviceKind::Memory, "memory", &memory_name);
    // DDR transfers twice per clock.
    let clock = |mts: Option<f64>| mts.map(|v| v / 2.0 * 1e6);
    let configured = modules.iter().find_map(|m| m.configured_mts.or(m.speed_mts));
    let rated = modules.iter().find_map(|m| m.speed_mts);
    let row = |id: &str, device: &Device, group, name: &str, unit| Reading::measure(id, device.clone(), group, name, unit, SOURCE, at);
    vec![
        row(BUS_CLOCK, &cpu, Group::Clocks, "Bus Clock", Unit::Hz).with(fw.and_then(|f| f.bus_mhz).map(|v| v * 1e6), missing),
        row("board.temperature.system_c", &board, Group::Temperatures, "Motherboard", Unit::Celsius).with(None, NO_PROVIDER),
        row("board.voltage.cpu_core_v", &board, Group::Voltages, "CPU Core", Unit::Volt).with(None, NO_PROVIDER),
        row("board.fan.cpu_rpm", &board, Group::Fans, "CPU Fan", Unit::Rpm).with(None, NO_PROVIDER),
        row("memory.frequency.clock_hz", &memory, Group::Clocks, "Memory Clock", Unit::Hz).with(clock(configured), if fw.is_some() { missing } else { NO_TABLE }),
        row("memory.frequency.rated_hz", &memory, Group::Clocks, "Rated Clock", Unit::Hz).with(clock(rated), missing),
        row("memory.temperature.module_c", &memory, Group::Temperatures, "Memory", Unit::Celsius).with(None, NO_PROVIDER),
    ]
}

/// The provider: reads the SMBIOS table once, then repeats it (it does not
/// change while Windows runs).
pub struct FirmwareTables {
    firmware: Option<Firmware>,
    cpu_name: String,
}

impl FirmwareTables {
    pub fn new() -> Self {
        FirmwareTables { firmware: crate::perfdata::smbios_table().map(|raw| super::smbios::parse(&raw)), cpu_name: super::cpu::cpu_name() }
    }
}

impl Provider for FirmwareTables {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        readings(self.firmware.as_ref(), &self.cpu_name, at)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::Frame;
    use crate::sensors::smbios::Module;

    fn ddr5(locator: &str) -> Module {
        Module { locator: locator.into(), size_mb: 16_384, ddr: Some("DDR5"), speed_mts: Some(6_000.0), configured_mts: Some(4_800.0) }
    }

    fn this_pc() -> Firmware {
        Firmware { board: Some("ASUSTeK ROG STRIX B650E-F".into()), bus_mhz: Some(100.0), modules: vec![ddr5("DIMM_A2"), ddr5("DIMM_B2")] }
    }

    fn frame(fw: Option<&Firmware>) -> Frame {
        Frame::build(1, &[readings(fw, "AMD Ryzen 5 7600X", Instant::now())], Vec::new())
    }

    fn groups(frame: &Frame, kind: DeviceKind) -> Vec<(&'static str, Vec<String>)> {
        let d = frame.devices.iter().find(|d| d.device.kind == kind).unwrap();
        d.groups.iter().map(|g| (g.group.name(), g.readings.iter().map(|r| format!("{}: {}", r.name, r.text())).collect())).collect()
    }

    #[test]
    fn the_bus_clock_sits_in_the_cpu_clocks() {
        let f = frame(Some(&this_pc()));
        let bus = f.reading(BUS_CLOCK).unwrap();
        assert_eq!((bus.name.as_str(), bus.group, bus.value), ("Bus Clock", Group::Clocks, Some(100e6)));
        assert_eq!(bus.device, Device::cpu("AMD Ryzen 5 7600X"), "the same device as the CPU counters");
    }

    #[test]
    fn a_bus_clock_the_firmware_does_not_give_is_grey_with_the_reason() {
        let mut fw = this_pc();
        fw.bus_mhz = None;
        assert_eq!(frame(Some(&fw)).reading(BUS_CLOCK).unwrap().text(), "the firmware does not report it");
        assert_eq!(frame(None).reading(BUS_CLOCK).unwrap().text(), "no firmware table on this PC");
    }

    #[test]
    fn the_motherboard_has_voltages_temperatures_and_fans_waiting_for_a_provider() {
        let f = frame(Some(&this_pc()));
        let board = f.devices.iter().find(|d| d.device.kind == DeviceKind::Motherboard).unwrap();
        assert_eq!(board.device.name, "ASUSTeK ROG STRIX B650E-F");
        assert_eq!(
            groups(&f, DeviceKind::Motherboard),
            vec![
                ("Temperatures", vec![format!("Motherboard: {NO_PROVIDER}")]),
                ("Voltages", vec![format!("CPU Core: {NO_PROVIDER}")]),
                ("Fans", vec![format!("CPU Fan: {NO_PROVIDER}")]),
            ]
        );
        assert_eq!(frame(None).devices.iter().find(|d| d.device.kind == DeviceKind::Motherboard).unwrap().device.name, "Motherboard");
    }

    #[test]
    fn the_memory_shows_its_clock_from_the_firmware_and_waits_for_temperatures() {
        let f = frame(Some(&this_pc()));
        let memory = f.devices.iter().find(|d| d.device.kind == DeviceKind::Memory).unwrap();
        assert_eq!(memory.device.name, "32 GB DDR5", "two 16 GB modules");
        assert_eq!(
            groups(&f, DeviceKind::Memory),
            vec![("Temperatures", vec![format!("Memory: {NO_PROVIDER}")]), ("Clocks", vec!["Memory Clock: 2.40 GHz".into(), "Rated Clock: 3.00 GHz".into()])],
            "DDR transfers twice per clock: 4800 MT/s = 2400 MHz"
        );
    }

    #[test]
    fn memory_the_firmware_does_not_describe_is_grey() {
        let none = frame(None);
        assert_eq!(none.devices.iter().find(|d| d.device.kind == DeviceKind::Memory).unwrap().device.name, "Memory");
        assert_eq!(none.reading("memory.frequency.clock_hz").unwrap().text(), "no firmware table on this PC");
        let mut fw = this_pc();
        fw.modules[0].configured_mts = None;
        fw.modules[1].configured_mts = None;
        assert_eq!(frame(Some(&fw)).reading("memory.frequency.clock_hz").unwrap().value, Some(3_000e6), "configured speed missing: the rated one");
    }

    /// Reads this PC's firmware tables and prints what they give.
    #[test]
    #[ignore]
    fn reads_this_pc_firmware() {
        let raw = crate::perfdata::smbios_table().expect("Windows gives the SMBIOS table");
        let fw = crate::sensors::smbios::parse(&raw);
        println!("{fw:#?}");
        let mut p = FirmwareTables::new();
        for r in p.read(Instant::now()) {
            println!("{:<12} {:<14} {:<14} {}", r.device.name, r.group.name(), r.name, r.text());
        }
        assert!(fw.board.is_some(), "the board names itself");
        assert!(!fw.modules.is_empty(), "memory modules listed");
    }
}
