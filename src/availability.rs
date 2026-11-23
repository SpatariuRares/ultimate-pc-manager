//! Availability: is a named system facility available here, and if not, why
//! not.
//!
//! One question, one place. Pages ask here instead of deciding for themselves
//! what to do off Windows, and every "not here" text is built in this module,
//! so the wording cannot drift. A [`Feature`] is named by the facility of the
//! system that may be missing, never by the page or add-on that needs it. On
//! Windows every current feature is available. An empty result ("nothing
//! found") is a different thing from an unavailable feature: callers report
//! the second through this module and never as an empty list, and hide an
//! unavailable feature while its page stays.
//!
//! Tests can make a feature unavailable with [`unavailable_in_test`], so the
//! unavailable state of a page can be exercised on Windows too.

/// Declares [`Feature`] together with [`Feature::ALL`], so the list of every
/// feature cannot fall out of step with the enum.
macro_rules! features {
    ($($(#[$doc:meta])* $name:ident,)*) => {
        /// A facility of the system the app runs on, which may be missing.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum Feature {
            $($(#[$doc])* $name,)*
        }

        impl Feature {
            /// Every feature, in declaration order.
            pub const ALL: &'static [Feature] = &[$(Feature::$name,)*];
        }
    };
}

features! {
    /// Running PowerShell scripts.
    PowerShell,
    /// The winget package manager.
    Winget,
    /// Starting programs and following their output.
    ProgramLaunch,
    /// Restarting the app with elevated rights.
    Elevation,
    /// Desktop notifications.
    Notifications,
    /// Listing and controlling services.
    ServiceControl,
    /// Priority, affinity, suspend and efficiency of processes.
    ProcessControl,
    /// The entries that start with the system.
    StartupEntries,
    /// The system's power options panel.
    PowerOptionsPanel,
    /// The counters and drivers the sensor providers read.
    HardwareSensors,
    /// The device setup API the driver listing is read from.
    DeviceSetupApi,
    /// The registry of installed programs, and their uninstallers.
    UninstallRegistry,
    /// The handles open files and resources are listed from.
    OpenHandles,
    /// The machine's fixed facts: processor, memory, disks, graphics.
    MachineInformation,
}

/// The reason the app knows for a feature, when it has a specific one.
fn reason(feature: Feature) -> Option<&'static str> {
    match feature {
        Feature::Winget => Some("needs winget"),
        Feature::PowerOptionsPanel => Some("a Windows panel"),
        _ => None,
    }
}

/// Whether this system has the feature at all.
fn system_has(_feature: Feature) -> bool {
    cfg!(windows)
}

/// Is the feature available on this system.
pub fn is_available(feature: Feature) -> bool {
    #[cfg(test)]
    if test_override::hides(feature) {
        return false;
    }
    system_has(feature)
}

/// What the user reads when the feature is not here, with the reason the app
/// knows for it. The one wording of "not available on this system"; the text
/// a stub off Windows returns, whatever the system the code runs on.
pub fn unavailable_error(feature: Feature) -> String {
    match reason(feature) {
        Some(reason) => format!("Not available on this system: {reason}."),
        None => "Not available on this system.".to_string(),
    }
}

/// What a page shows in place of the feature: `None` when it is available,
/// else the [`unavailable_error`] text.
pub fn unavailable_text(feature: Feature) -> Option<String> {
    (!is_available(feature)).then(|| unavailable_error(feature))
}

/// The features this system lacks (none on Windows).
// The list of what a supported system lacks is part of the spec (story 10);
// no page shows it yet.
#[cfg_attr(not(test), allow(dead_code))]
pub fn unavailable_features() -> Vec<Feature> {
    Feature::ALL.iter().copied().filter(|f| !is_available(*f)).collect()
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

    #[cfg(windows)]
    #[test]
    fn every_current_feature_is_available_on_windows() {
        for &feature in Feature::ALL {
            assert!(is_available(feature), "{feature:?}");
            assert_eq!(unavailable_text(feature), None);
        }
        assert_eq!(unavailable_features(), Vec::<Feature>::new());
    }

    #[cfg(not(windows))]
    #[test]
    fn no_current_feature_is_available_off_windows() {
        for &feature in Feature::ALL {
            assert!(!is_available(feature), "{feature:?}");
            assert!(unavailable_text(feature).is_some());
        }
        assert_eq!(unavailable_features(), Feature::ALL);
    }

    #[test]
    fn the_wording_is_one_sentence_with_or_without_a_reason() {
        assert_eq!(unavailable_error(Feature::Notifications), "Not available on this system.");
        assert_eq!(unavailable_error(Feature::Winget), "Not available on this system: needs winget.");
        assert_eq!(unavailable_error(Feature::PowerOptionsPanel), "Not available on this system: a Windows panel.");
        assert_eq!(unavailable_error(Feature::HardwareSensors), "Not available on this system.");
    }

    #[test]
    fn a_test_answer_makes_only_the_named_features_unavailable_until_dropped() {
        {
            let _guard = unavailable_in_test(&[Feature::DeviceSetupApi]);
            assert!(!is_available(Feature::DeviceSetupApi));
            assert_eq!(unavailable_text(Feature::DeviceSetupApi).as_deref(), Some("Not available on this system."));
            assert_eq!(is_available(Feature::Winget), cfg!(windows), "others keep the system's answer");
        }
        assert_eq!(is_available(Feature::DeviceSetupApi), cfg!(windows));
    }

    #[test]
    fn the_features_a_system_lacks_are_listed() {
        let lacking_here = unavailable_features();
        let _guard = unavailable_in_test(&[Feature::Winget, Feature::Elevation]);
        let lacking = unavailable_features();
        assert!(lacking.contains(&Feature::Winget) && lacking.contains(&Feature::Elevation), "{lacking:?}");
        assert_eq!(lacking.len() - lacking_here.len(), if cfg!(windows) { 2 } else { 0 }, "only the named ones are added");
        assert!(lacking.iter().all(|f| Feature::ALL.contains(f)));
    }
}
