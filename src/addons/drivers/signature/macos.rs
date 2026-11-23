//! macOS: driver signatures are not checked yet.

use super::Signature;

/// The signature of the driver file at `path`, with normal rights.
pub fn check(_path: &str) -> Signature {
    Signature::CouldNotCheck("signatures are checked on Windows only".into())
}
