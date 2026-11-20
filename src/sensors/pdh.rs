//! A small PDH query for providers: add English counter paths under keys,
//! collect, read every instance as `(instance, value)`. Not `Send`: build it
//! on the provider's worker thread. Energy Meter, thermal zones and disk
//! providers can reuse it.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};

/// Values above 100 % are kept (% Processor Performance boosts above it).
const PDH_FMT_NOCAP100: u32 = 0x8000;

/// An open PDH query and its counters by key; closed when dropped.
pub struct Query {
    q: PDH_HQUERY,
    counters: Vec<(&'static str, PDH_HCOUNTER)>,
}

impl Query {
    /// Opens a query with the counters that exist on this PC (missing
    /// ones are skipped: their reads are empty). `None` if PDH is unusable.
    pub fn open(paths: &[(&'static str, &str)]) -> Option<Self> {
        let mut q: PDH_HQUERY = null_mut();
        // SAFETY: plain PDH calls with valid out-pointers; the query is
        // closed in Drop.
        unsafe {
            if PdhOpenQueryW(null(), 0, &mut q) != 0 {
                return None;
            }
            let mut counters = Vec::new();
            for (key, path) in paths {
                let w: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
                let mut h: PDH_HCOUNTER = null_mut();
                if PdhAddEnglishCounterW(q, w.as_ptr(), 0, &mut h) == 0 {
                    counters.push((*key, h));
                }
            }
            PdhCollectQueryData(q);
            Some(Query { q, counters })
        }
    }

    /// Takes a new sample of every counter.
    pub fn collect(&mut self) {
        // SAFETY: the query handle is open.
        unsafe {
            PdhCollectQueryData(self.q);
        }
    }

    /// Every instance of a counter as `(instance name, value)`; empty when
    /// the counter is missing or has no valid data.
    pub fn values(&self, key: &str) -> Vec<(String, f64)> {
        let Some(&(_, h)) = self.counters.iter().find(|(k, _)| *k == key) else { return Vec::new() };
        // SAFETY: the buffer is sized by the first call and outlives the
        // items read from it.
        unsafe {
            let flags = PDH_FMT_DOUBLE | PDH_FMT_NOCAP100;
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetFormattedCounterArrayW(h, flags, &mut size, &mut count, null_mut()) != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u64; size as usize / 8 + 1];
            let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(h, flags, &mut size, &mut count, items) != 0 {
                return Vec::new();
            }
            (0..count as usize)
                .filter_map(|i| {
                    let it = &*items.add(i);
                    // 0 = valid data, 1 = new data.
                    (it.FmtValue.CStatus <= 1).then(|| (crate::winsys::from_wide(it.szName), it.FmtValue.Anonymous.doubleValue))
                })
                .collect()
        }
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: closed once, here.
        unsafe {
            PdhCloseQuery(self.q);
        }
    }
}

