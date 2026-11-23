//! The operating systems the app supports, and which one it runs on.

/// An operating system the app supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum System {
    Windows,
    Linux,
    MacOs,
}

impl System {
    /// The three supported systems.
    pub const ALL: [System; 3] = [System::Windows, System::Linux, System::MacOs];

    /// The system the app runs on.
    pub const fn current() -> System {
        #[cfg(windows)]
        return System::Windows;
        #[cfg(target_os = "linux")]
        return System::Linux;
        #[cfg(target_os = "macos")]
        return System::MacOs;
    }
}

#[cfg(test)]
mod tests {
    use super::System;

    #[test]
    fn the_current_system_is_one_of_the_supported_ones() {
        assert!(System::ALL.contains(&System::current()));
        assert_eq!(System::ALL, [System::Windows, System::Linux, System::MacOs]);
    }
}
