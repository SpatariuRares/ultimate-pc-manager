//! Availability: is a named feature available on this system, and if not,
//! why not.
//!
//! One question, one place. Pages and features ask here instead of deciding
//! for themselves what to do off Windows, and every "not here" text is built
//! by [`not_on_this_system`], so the wording cannot drift. On Windows every
//! current feature is available. An empty result ("nothing found") is a
//! different thing from an unavailable feature: callers report the second
//! through this module and never as an empty list.
//!
//! Tests can make a feature unavailable with [`unavailable_in_test`], so
//! the unavailable state of a page can be exercised on Windows too.

// Part of this surface is used only by the stubs off Windows and by the
// silent-empty stubs that report through it.
#![allow(dead_code)]

/// A part of the app that depends on the system it runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Feature {
    /// Running PowerShell scripts.
    PowerShell,
    /// The winget package manager.
    Winget,
    /// Starting programs and following their output.
    RunPrograms,
    /// Restarting the app with elevated rights.
    Elevation,
    /// Desktop notifications.
    Notifications,
    /// Listing and controlling services.
    Services,
    /// Priority, affinity, suspend and efficiency of processes.
    ProcessControl,
    /// The startup entries.
    StartupEntries,
    /// The system's power options panel.
    PowerOptions,
    /// The sensor providers that read the system's counters and drivers.
    SensorProviders,
    /// The drivers listing.
    DriversListing,
    /// The installed apps listing.
    InstalledApps,
    /// The resources open files and handles are listed from.
    OpenResources,
    /// The performance page's static information.
    PerformanceInfo,
    /// The registry the uninstaller reads entries from.
    UninstallRegistry,
}

/// The answer to "is this feature available here".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    /// Not here; the reason when the app knows a specific one.
    Unavailable(Option<&'static str>),
}

impl Availability {
    /// Whether the feature can be used.
    pub fn is_available(&self) -> bool {
        matches!(self, Availability::Available)
    }

    /// What the user reads when the feature is not here; `None` when it is.
    pub fn text(&self) -> Option<String> {
        match self {
            Availability::Available => None,
            Availability::Unavailable(reason) => Some(not_on_this_system(*reason)),
        }
    }
}

/// The one wording of "not available on this system", with an optional
/// specific reason.
pub fn not_on_this_system(reason: Option<&str>) -> String {
    match reason {
        Some(reason) => format!("Not available on this system: {reason}."),
        None => "Not available on this system.".to_string(),
    }
}

/// The reason the app knows for a feature, when it has a specific one.
fn reason(feature: Feature) -> Option<&'static str> {
    match feature {
        Feature::Winget => Some("needs winget"),
        Feature::PowerOptions => Some("a Windows panel"),
        _ => None,
    }
}

/// Whether this system has the feature at all.
fn system_has(_feature: Feature) -> bool {
    cfg!(windows)
}

/// Is the feature available on this system.
pub fn availability(feature: Feature) -> Availability {
    #[cfg(test)]
    if test_override::hides(feature) {
        return Availability::Unavailable(reason(feature));
    }
    if system_has(feature) {
        Availability::Available
    } else {
        Availability::Unavailable(reason(feature))
    }
}

/// Is the feature available on this system.
pub fn is_available(feature: Feature) -> bool {
    availability(feature).is_available()
}

/// The text of the error a stub off Windows returns for the feature,
/// whatever the system the code runs on.
pub fn unavailable_error(feature: Feature) -> String {
    not_on_this_system(reason(feature))
}

#[cfg(test)]
mod test_override {
    use super::Feature;
    use std::cell::RefCell;

    thread_local! {
        static HIDDEN: RefCell<Vec<Feature>> = const { RefCell::new(Vec::new()) };
    }

    pub fn hides(feature: Feature) -> bool {
        HIDDEN.with(|h| h.borrow().contains(&feature))
    }

    /// Keeps the features unavailable on this thread until it is dropped.
    pub struct Guard(Vec<Feature>);

    impl Drop for Guard {
        fn drop(&mut self) {
            HIDDEN.with(|h| h.borrow_mut().retain(|f| !self.0.contains(f)));
        }
    }

    pub fn hide(features: &[Feature]) -> Guard {
        HIDDEN.with(|h| h.borrow_mut().extend_from_slice(features));
        Guard(features.to_vec())
    }
}

/// Make the features unavailable on the current thread, as on a system
/// without them, until the guard is dropped.
#[cfg(test)]
pub fn unavailable_in_test(features: &[Feature]) -> impl Drop {
    test_override::hide(features)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Feature; 15] = [
        Feature::PowerShell,
        Feature::Winget,
        Feature::RunPrograms,
        Feature::Elevation,
        Feature::Notifications,
        Feature::Services,
        Feature::ProcessControl,
        Feature::StartupEntries,
        Feature::PowerOptions,
        Feature::SensorProviders,
        Feature::DriversListing,
        Feature::InstalledApps,
        Feature::OpenResources,
        Feature::PerformanceInfo,
        Feature::UninstallRegistry,
    ];

    #[cfg(windows)]
    #[test]
    fn every_current_feature_is_available_on_windows() {
        for feature in ALL {
            assert_eq!(availability(feature), Availability::Available, "{feature:?}");
            assert!(is_available(feature));
            assert_eq!(availability(feature).text(), None);
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn no_current_feature_is_available_off_windows() {
        for feature in ALL {
            assert!(!is_available(feature), "{feature:?}");
            assert!(availability(feature).text().is_some());
        }
    }

    #[test]
    fn the_wording_is_one_sentence_with_or_without_a_reason() {
        assert_eq!(not_on_this_system(None), "Not available on this system.");
        assert_eq!(not_on_this_system(Some("needs winget")), "Not available on this system: needs winget.");
    }

    #[test]
    fn a_specific_reason_shows_in_the_text_of_its_feature() {
        assert_eq!(unavailable_error(Feature::Winget), "Not available on this system: needs winget.");
        assert_eq!(unavailable_error(Feature::PowerOptions), "Not available on this system: a Windows panel.");
        assert_eq!(unavailable_error(Feature::Notifications), "Not available on this system.");
        assert_eq!(unavailable_error(Feature::SensorProviders), "Not available on this system.");
    }

    #[test]
    fn a_test_answer_makes_only_the_named_features_unavailable_until_dropped() {
        {
            let _guard = unavailable_in_test(&[Feature::DriversListing]);
            let answer = availability(Feature::DriversListing);
            assert!(!answer.is_available());
            assert_eq!(answer.text().as_deref(), Some("Not available on this system."));
            assert_eq!(is_available(Feature::Winget), cfg!(windows), "others keep the system's answer");
        }
        assert_eq!(is_available(Feature::DriversListing), cfg!(windows));
    }
}
