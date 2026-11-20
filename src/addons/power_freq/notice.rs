//! The secondary band of Power & Freq (pure): why CPU power and
//! temperature are missing, with "Get AMD's add-on" and "Restart as
//! administrator" when they help.

use crate::sensors::ryzen::{self, NEEDS_ADMIN, NEEDS_SDK, SDK_TOO_OLD};
use crate::sensors::Frame;

/// The band's words and the actions it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub text: String,
    /// "Get AMD's add-on" (opens [`ryzen::AMD_URL`]).
    pub get_addon: bool,
    /// "Restart as administrator".
    pub restart: bool,
}

/// The words of the administrator line when the AMD line comes first.
pub const SOME_NEED_ADMIN: &str = "Some sensors need administrator";

/// What a line of the band offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Offer {
    GetAddon,
    Restart,
}

impl Notice {
    /// The band as drawn: one line per reason, each with its button.
    pub fn lines(&self) -> Vec<(Offer, String)> {
        let mut out = Vec::new();
        if self.get_addon {
            out.push((Offer::GetAddon, self.text.clone()));
        }
        if self.restart {
            out.push((Offer::Restart, if self.get_addon { SOME_NEED_ADMIN.to_string() } else { self.text.clone() }));
        }
        out
    }
}

/// The band for this frame, or `None` when nothing the user can do is
/// missing. `admin` = the app already runs as administrator.
pub fn notice(frame: &Frame, admin: bool) -> Option<Notice> {
    let why = |reason: &str| frame.readings().any(|r| r.unavailable.as_deref() == Some(reason)) || frame.sources.iter().any(|s| s.problem.as_deref() == Some(reason));
    let too_old = why(SDK_TOO_OLD);
    let get_addon = why(NEEDS_SDK) || too_old;
    let restart = !admin && why(NEEDS_ADMIN);
    let ryzen_admin = frame.sources.iter().any(|s| s.name == ryzen::SOURCE && s.problem.as_deref() == Some(NEEDS_ADMIN));
    let site = ryzen::AMD_URL.trim_start_matches("https://www.");
    let text = if too_old {
        format!("Ryzen temperature and power need a newer add-on from AMD; see {site}")
    } else if get_addon {
        format!("Ryzen temperature and power need a free add-on from AMD; see {site}")
    } else if restart && ryzen_admin {
        "Restart as administrator to read Ryzen temperature and power".to_string()
    } else if restart {
        SOME_NEED_ADMIN.to_string()
    } else {
        return None;
    };
    Some(Notice { text, get_addon, restart })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::model::{Device, Group, Reading, SourceStatus, Unit};
    use std::time::Instant;

    fn frame(rows: Vec<Reading>, sources: Vec<SourceStatus>) -> Frame {
        Frame::build(1, &[rows], sources)
    }

    fn ryzen_rows(why: &str) -> (Vec<Reading>, Vec<SourceStatus>) {
        let rows = ryzen::readings(Err(why), "AMD Ryzen 5 7600X", Instant::now());
        (rows, vec![SourceStatus { name: ryzen::SOURCE, problem: Some(why.into()) }])
    }

    #[test]
    fn without_amds_sdk_the_band_offers_the_add_on() {
        let (rows, sources) = ryzen_rows(NEEDS_SDK);
        let n = notice(&frame(rows, sources), false).unwrap();
        assert_eq!(n.text, "Ryzen temperature and power need a free add-on from AMD; see amd.com/en/developer/ryzen-master-monitoring-sdk.html");
        assert!(n.get_addon);
        assert!(!n.restart, "administrator would not help yet");
    }

    #[test]
    fn with_an_sdk_too_old_the_band_offers_the_add_on_again() {
        let (rows, sources) = ryzen_rows(ryzen::SDK_TOO_OLD);
        let n = notice(&frame(rows, sources), false).unwrap();
        assert_eq!(n.text, "Ryzen temperature and power need a newer add-on from AMD; see amd.com/en/developer/ryzen-master-monitoring-sdk.html");
        assert!(n.get_addon && !n.restart);
    }

    #[test]
    fn sdk_failures_the_user_cannot_fix_here_draw_no_band() {
        for why in [ryzen::DRIVER_STOPPED, ryzen::VIRTUAL_MACHINE, ryzen::ZEROS, ryzen::SDK_DID_NOT_START, ryzen::SDK_READ_FAILED] {
            let (rows, sources) = ryzen_rows(why);
            assert_eq!(notice(&frame(rows, sources), false), None, "{why}");
        }
    }

    #[test]
    fn when_the_sdk_needs_administrator_the_band_offers_a_restart() {
        let (rows, sources) = ryzen_rows(NEEDS_ADMIN);
        let n = notice(&frame(rows, sources), false).unwrap();
        assert_eq!(n, Notice { text: "Restart as administrator to read Ryzen temperature and power".into(), get_addon: false, restart: true });
    }

    #[test]
    fn other_sensors_needing_administrator_also_offer_a_restart() {
        let smart = Reading::measure("disk0.smart.health", Device::cpu("Disk"), Group::Smart, "Health", Unit::Count, "S.M.A.R.T.", Instant::now()).with(None, NEEDS_ADMIN);
        let n = notice(&frame(vec![smart], vec![]), false).unwrap();
        assert_eq!((n.text.as_str(), n.restart), ("Some sensors need administrator", true));
    }

    #[test]
    fn the_band_draws_one_line_per_reason_each_with_its_button() {
        let both = Notice { text: "AMD".into(), get_addon: true, restart: true };
        assert_eq!(both.lines(), vec![(Offer::GetAddon, "AMD".to_string()), (Offer::Restart, SOME_NEED_ADMIN.to_string())]);
        let admin = Notice { text: "Restart".into(), get_addon: false, restart: true };
        assert_eq!(admin.lines(), vec![(Offer::Restart, "Restart".to_string())]);
    }

    #[test]
    fn running_as_administrator_never_offers_a_restart() {
        let (rows, sources) = ryzen_rows(NEEDS_ADMIN);
        assert_eq!(notice(&frame(rows, sources), true), None);
    }

    #[test]
    fn with_no_provider_to_unlock_there_is_no_band() {
        let none = Reading::measure("pc.power.gpu_w", Device::this_pc(), Group::Powers, "GPU", Unit::Watt, "Energy Meter", Instant::now()).with(None, "no provider on this PC");
        let not_amd = vec![SourceStatus { name: ryzen::SOURCE, problem: Some(ryzen::NOT_AMD.into()) }];
        assert_eq!(notice(&frame(vec![none], not_amd), false), None);
    }
}
