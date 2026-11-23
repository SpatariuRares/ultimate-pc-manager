//! Devices with a problem: which devices count as one, and what Windows'
//! Configuration Manager problem codes mean, in plain words, with a
//! suggested action.

use super::engine::DeviceDriver;

/// What the page can offer for a problem, when it has the matching action.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    /// Update or reinstall the driver (Windows Update's offer for the device).
    Driver,
    /// Restart the device.
    Restart,
    /// Enable the device.
    Enable,
}

impl Fix {
    /// The suggestion as the page words it.
    pub fn text(self) -> &'static str {
        match self {
            Fix::Driver => "Update or reinstall the driver",
            Fix::Restart => "Restart the device",
            Fix::Enable => "Enable the device",
        }
    }
}

/// A problem code in plain words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Problem {
    pub code: u32,
    pub explanation: &'static str,
    /// What to try; none when nothing the page does helps.
    pub fix: Option<Fix>,
}

/// The documented Configuration Manager problem codes (CM_PROB_*), each
/// with a short explanation and a suggestion.
const TABLE: &[(u32, &str, Option<Fix>)] = &[
    (1, "The device is not configured correctly.", Some(Fix::Driver)),
    (3, "The driver may be corrupted, or the PC is running low on memory.", Some(Fix::Restart)),
    (10, "The device cannot start.", Some(Fix::Driver)),
    (12, "The device cannot find enough free resources to use.", None),
    (14, "The device cannot work properly until the PC restarts.", Some(Fix::Restart)),
    (16, "Windows cannot identify all the resources the device uses.", None),
    (18, "The drivers for this device need to be reinstalled.", Some(Fix::Driver)),
    (19, "The registry has incomplete or damaged information about the device.", Some(Fix::Driver)),
    (21, "Windows is removing the device.", None),
    (22, "The device is disabled.", Some(Fix::Enable)),
    (24, "The device is not working properly or does not have all its drivers installed.", Some(Fix::Driver)),
    (28, "No driver is installed.", Some(Fix::Driver)),
    (29, "The device is disabled because its firmware gave it no resources.", None),
    (31, "Windows cannot load the drivers this device needs.", Some(Fix::Driver)),
    (32, "A driver service for this device is disabled.", Some(Fix::Driver)),
    (33, "Windows cannot work out which resources the device needs.", None),
    (34, "Windows cannot work out the settings for the device.", None),
    (35, "The firmware of this PC does not have enough information to set the device up.", None),
    (36, "The device is asking for an interrupt that is already in use.", None),
    (37, "Windows cannot start the device driver.", Some(Fix::Driver)),
    (38, "Windows cannot load the driver because a previous copy is still in memory.", Some(Fix::Restart)),
    (39, "Windows cannot load the driver: it may be corrupted or missing.", Some(Fix::Driver)),
    (40, "Windows cannot reach the device because its service entry in the registry is missing or wrong.", Some(Fix::Driver)),
    (41, "Windows loaded the driver but cannot find the device.", Some(Fix::Driver)),
    (42, "Windows cannot load the driver because another device has the same identifier.", Some(Fix::Restart)),
    (43, "Windows stopped the device because it reported problems.", Some(Fix::Restart)),
    (44, "An application or service shut the device down.", Some(Fix::Restart)),
    (45, "The device is not connected.", None),
    (46, "Windows cannot use the device because the PC is shutting down.", None),
    (47, "Windows cannot use the device because it was prepared for safe removal.", None),
    (48, "Windows blocked the software for this device because of known problems.", Some(Fix::Driver)),
    (49, "Windows cannot start new devices because its system settings are too large.", None),
    (50, "Windows cannot apply all the properties of the device.", Some(Fix::Driver)),
    (51, "The device is waiting for another device to start.", None),
    (52, "Windows cannot verify the digital signature of the driver.", Some(Fix::Driver)),
    (53, "The device is reserved for the Windows kernel debugger.", None),
    (54, "The device failed and Windows is resetting it.", Some(Fix::Restart)),
];

/// What a problem code means. A code the table does not know still gets a
/// sentence naming it, and no suggestion.
pub fn explain(code: u32) -> Problem {
    match TABLE.iter().find(|(c, _, _)| *c == code) {
        Some(&(code, explanation, fix)) => Problem { code, explanation, fix },
        None => Problem { code, explanation: "Windows reported a problem with this device.", fix: None },
    }
}

/// A device has a problem when it is plugged in and the Configuration
/// Manager gave it a problem code. A disconnected device is not a problem.
pub fn is_problem(d: &DeviceDriver) -> bool {
    d.present && d.problem != 0
}

/// The row's second line: the code and what it means.
pub fn row_text(d: &DeviceDriver) -> String {
    let p = explain(d.problem);
    format!("Code {}: {}", p.code, p.explanation)
}

/// The status band's sentence for `n` devices with a problem.
pub fn band_text(n: usize) -> String {
    if n == 1 { "1 device has a problem".into() } else { format!("{n} devices have a problem") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(problem: u32, present: bool) -> DeviceDriver {
        DeviceDriver { name: "Dev".into(), present, problem, ..Default::default() }
    }

    #[test]
    fn the_documented_codes_are_explained_in_plain_words() {
        assert_eq!(explain(10).explanation, "The device cannot start.");
        assert_eq!(explain(28).explanation, "No driver is installed.");
        assert_eq!(explain(43).explanation, "Windows stopped the device because it reported problems.");
        assert_eq!(explain(22).explanation, "The device is disabled.");
        // Every documented code, 1 to 54, is in the table or is one Windows never reports.
        for code in [1, 3, 10, 12, 14, 16, 18, 19, 21, 22, 24, 28, 29, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 54] {
            assert!(TABLE.iter().any(|(c, _, _)| *c == code), "code {code} is explained");
        }
    }

    #[test]
    fn suggestions_name_the_action_to_try() {
        assert_eq!(explain(10).fix.map(Fix::text), Some("Update or reinstall the driver"));
        assert_eq!(explain(28).fix, Some(Fix::Driver));
        assert_eq!(explain(43).fix.map(Fix::text), Some("Restart the device"));
        assert_eq!(explain(22).fix.map(Fix::text), Some("Enable the device"));
        assert_eq!(explain(12).fix, None, "nothing the page does frees resources");
    }

    #[test]
    fn an_unknown_code_is_still_named_and_suggests_nothing() {
        let p = explain(9999);
        assert_eq!((p.code, p.fix), (9999, None));
        assert_eq!(row_text(&device(9999, true)), "Code 9999: Windows reported a problem with this device.");
    }

    #[test]
    fn a_connected_device_with_a_code_is_a_problem_and_a_disconnected_one_is_not() {
        assert!(is_problem(&device(10, true)));
        assert!(!is_problem(&device(0, true)), "working");
        assert!(!is_problem(&device(45, false)), "not connected");
        assert!(!is_problem(&device(22, false)));
    }

    #[test]
    fn the_row_and_band_words() {
        assert_eq!(row_text(&device(10, true)), "Code 10: The device cannot start.");
        assert_eq!(band_text(1), "1 device has a problem");
        assert_eq!(band_text(3), "3 devices have a problem");
    }
}
