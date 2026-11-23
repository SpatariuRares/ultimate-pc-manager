//! macOS: signatures are not checked yet, so no file has a verified signature.

use crate::signature::Signature;

/// The verified signature of the file at `path`: none, not available yet.
pub fn verify(_path: &str) -> Option<Signature> {
    None
}
