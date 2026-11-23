//! Windows: Wait Chain Traversal over the threads of a process.

use crate::wait_chain::{Node, ThreadChain};
use crate::proc_info::Missing;
use std::ffi::c_void;
use windows_sys::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, ERROR_MORE_DATA};
use windows_sys::Win32::System::Diagnostics::Debug::{
    CloseThreadWaitChainSession, GetThreadWaitChain, OpenThreadWaitChainSession, WctStatusBlocked, WctStatusNoAccess, WctThreadType, WAITCHAIN_NODE_INFO,
    WCT_MAX_NODE_COUNT, WCT_OUT_OF_PROC_FLAG,
};

/// A synchronous Wait Chain Traversal session, closed on drop.
struct Session(*mut c_void);

impl Drop for Session {
    fn drop(&mut self) {
        // SAFETY: the handle came from OpenThreadWaitChainSession and is
        // closed once.
        unsafe { CloseThreadWaitChainSession(self.0) }
    }
}

/// Held while a session is open: Wait Chain Traversal run on two
/// threads of this process at once crashes it, so analyses take turns.
static ONE_AT_A_TIME: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The wait chain of each thread in `ids`, in one session.
pub fn chains(ids: &[u32]) -> Result<Vec<ThreadChain>, Missing> {
    let _turn = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: no flags and no callback open a synchronous session.
    let handle = unsafe { OpenThreadWaitChainSession(0, None) };
    if handle.is_null() {
        return Err(Missing::Unavailable);
    }
    let session = Session(handle);
    Ok(ids.iter().map(|&tid| chain(&session, tid)).collect())
}

fn chain(session: &Session, tid: u32) -> ThreadChain {
    let mut nodes = [WAITCHAIN_NODE_INFO::default(); WCT_MAX_NODE_COUNT as usize];
    let mut count = WCT_MAX_NODE_COUNT;
    let mut cycle = 0;
    // SAFETY: `nodes` holds `count` entries and every out-param is valid
    // for the call; the session is open.
    let ok = unsafe { GetThreadWaitChain(session.0, 0, WCT_OUT_OF_PROC_FLAG, tid, &mut count, nodes.as_mut_ptr(), &mut cycle) };
    // SAFETY: reads this thread's last error, right after the call.
    let error = if ok == 0 { unsafe { GetLastError() } } else { 0 };
    let more = ok == 0 && error == ERROR_MORE_DATA;
    if ok == 0 && !more {
        return if error == ERROR_ACCESS_DENIED { ThreadChain::Refused } else { ThreadChain::Skipped };
    }
    let got = &nodes[..(count as usize).min(nodes.len())];
    // The first node is the thread itself.
    if got.first().is_some_and(|n| n.ObjectStatus == WctStatusNoAccess) {
        return ThreadChain::Refused;
    }
    let nodes = got.iter().map(|n| Node { thread: n.ObjectType == WctThreadType, blocked: n.ObjectStatus == WctStatusBlocked }).collect();
    // A chain that fills every node may go on past what Windows gives.
    let cut = more || (cycle == 0 && count >= WCT_MAX_NODE_COUNT);
    ThreadChain::Chain { nodes, cycle: cycle != 0, cut }
}
