//! macOS: Wait Chain Traversal is not available yet.

use crate::proc_info::Missing;
use crate::wait_chain::ThreadChain;

/// The wait chain of each thread in `ids`: not available yet.
pub fn chains(_ids: &[u32]) -> Result<Vec<ThreadChain>, Missing> {
    Err(Missing::Unavailable)
}
