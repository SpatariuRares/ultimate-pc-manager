//! macOS: no hybrid-core information is read yet, so every core counts as a
//! performance core.

/// Each logical processor's efficiency class. Empty: not available yet.
pub fn efficiency_classes() -> Vec<u8> {
    Vec::new()
}
