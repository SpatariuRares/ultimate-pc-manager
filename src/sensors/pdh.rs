//! A small PDH query for providers: add English counter paths under keys,
//! collect, read every instance as `(instance, value)`. Not `Send`: build it
//! on the provider's worker thread. Energy Meter, thermal zones and disk
//! providers can reuse it.

use std::ptr::{null, null_mut};

use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetRawCounterArrayW, PdhOpenQueryW,
    PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PDH_RAW_COUNTER_ITEM_W,
};

use super::model::{CounterSet, Instance};

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
                    (it.FmtValue.CStatus <= 1).then(|| (crate::win_ffi::from_wide(it.szName), it.FmtValue.Anonymous.doubleValue))
                })
                .collect()
        }
    }
}

impl Query {
    /// Every instance of a counter as the shape providers report counters
    /// in ([`CounterSet`]); with `raw`, each also carries the raw value (for
    /// a "per second" counter, the running total since boot). The set has no
    /// instance when the counter is missing or has no valid data.
    pub fn counted(&self, key: &'static str, raw: bool) -> CounterSet {
        let raws = if raw { self.raw(key) } else { Vec::new() };
        let instances = self
            .values(key)
            .into_iter()
            .map(|(name, value)| {
                let total = raws.iter().find(|(n, _)| *n == name).map(|(_, v)| *v);
                Instance { name, value, raw: total }
            })
            .collect();
        CounterSet::new(key, instances)
    }

    /// The raw value of every instance of a counter.
    fn raw(&self, key: &str) -> Vec<(String, i64)> {
        let Some(&(_, h)) = self.counters.iter().find(|(k, _)| *k == key) else { return Vec::new() };
        // SAFETY: the buffer is sized by the first call and outlives the
        // items read from it.
        unsafe {
            let (mut size, mut count) = (0u32, 0u32);
            if PdhGetRawCounterArrayW(h, &mut size, &mut count, null_mut()) != PDH_MORE_DATA || size == 0 {
                return Vec::new();
            }
            let mut buf = vec![0u64; size as usize / 8 + 1];
            let items = buf.as_mut_ptr() as *mut PDH_RAW_COUNTER_ITEM_W;
            if PdhGetRawCounterArrayW(h, &mut size, &mut count, items) != 0 {
                return Vec::new();
            }
            (0..count as usize)
                .filter_map(|i| {
                    let it = &*items.add(i);
                    (it.RawValue.CStatus <= 1).then(|| (crate::win_ffi::from_wide(it.szName), it.RawValue.FirstValue))
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


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_comes_with_its_instances_and_the_raw_total_since_boot() {
        let mut q = Query::open(&[("pagein", r"\Memory\Pages Input/sec"), ("missing", r"\No Such Object\Nothing")]).expect("PDH works on this PC");
        std::thread::sleep(std::time::Duration::from_millis(200));
        q.collect();
        let set = q.counted("pagein", true);
        assert_eq!(set.key, "pagein");
        let one = set.instances.first().expect("the Memory counter has one instance");
        assert!(one.raw.is_some_and(|r| r > 0), "pages read since boot: {one:?}");
        assert!(q.counted("pagein", false).instances[0].raw.is_none(), "no raw unless asked");
        assert!(!q.counted("missing", true).answered(), "a counter that does not exist has not answered");
    }
}
