//! macOS: ETW is missing, so a trace cannot be started.

use crate::file_activity::{Feed, Stopper, TraceMsg};
use std::sync::Arc;

/// Stops the trace session: there is none.
pub fn stop_session(_: u64) {}

/// Starts a trace: not available yet, says so at once.
pub fn run(_: u32, _: Arc<Stopper>, tx: Feed) {
    // ERROR_NOT_SUPPORTED
    let _ = tx.send(TraceMsg::Failed(50));
}
