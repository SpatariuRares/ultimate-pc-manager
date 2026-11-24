//! macOS: the portable scanner, until this system has a listing of its own.

use super::Mounts;
pub use super::portable::list;

/// macOS scans with the portable listing, which keeps no mount points out.
pub fn mounts() -> Mounts {
    Mounts::default()
}

/// Lowers the calling thread's CPU priority: not changed on this system yet.
pub fn lower_priority() {}
