//! Linux: the portable scanner, until this system has a listing of its own.

pub use super::portable::list;

/// Lowers the calling thread's CPU priority: not changed on this system yet.
pub fn lower_priority() {}
