//! Windows: the processor counters (`\Processor Information(*)`) through a
//! PDH query, the power API's clocks and the registry's processor name.

use std::time::{Duration, Instant};

use crate::sensors::cpu::{logical_index, CoreSample, CpuCounters, CpuSample};
use crate::sensors::model::CounterSet;
use crate::sensors::pdh::Query;

/// Rate counters need this much time between two collections.
const MIN_SPAN: Duration = Duration::from_millis(200);

/// The processor query, if the counters are on.
pub struct Counters {
    query: Option<Query>,
    collected: Instant,
}

impl Counters {
    pub fn open() -> Self {
        Counters {
            query: Query::open(&[
                ("perf", r"\Processor Information(*)\% Processor Performance"),
                ("freq", r"\Processor Information(*)\Processor Frequency"),
                ("park", r"\Processor Information(*)\Parking Status"),
                ("util", r"\Processor Information(*)\% Processor Utility"),
                ("limit", r"\Processor Information(*)\% Performance Limit"),
                ("flags", r"\Processor Information(*)\Performance Limit Flags"),
                ("c1", r"\Processor Information(*)\% C1 Time"),
                ("c2", r"\Processor Information(*)\% C2 Time"),
                ("c3", r"\Processor Information(*)\% C3 Time"),
            ]),
            collected: Instant::now(),
        }
    }

    pub fn availability(&self) -> Result<(), String> {
        match self.query {
            Some(_) => Ok(()),
            None => Err("performance counters are off on this PC".into()),
        }
    }

    pub fn sample(&mut self, name: &str, machine_mhz: u32, counted: &mut Vec<CounterSet>) -> CpuSample {
        let mut s = CpuSample { name: name.to_string(), ..Default::default() };
        let power = processor_power();
        s.cores = power.iter().map(|&(max, cur)| CoreSample { current_mhz: Some(cur), max_mhz: Some(max), ..Default::default() }).collect();
        let Some(q) = self.query.as_mut() else { return s };
        let wait = MIN_SPAN.saturating_sub(self.collected.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        q.collect();
        self.collected = Instant::now();
        // The performance level is also reported as counted (the core pages
        // read it from the frame), once.
        let perf = q.counted(crate::perfdata::counter::PERF, false);
        let total = |key: &str| q.values(key).into_iter().find(|(n, _)| n == "_Total").map(|(_, v)| v);
        s.base_mhz = CpuCounters::base_clock(machine_mhz, total("freq"), power.first().map(|p| p.0));
        s.performance_percent = perf.instance("_Total").map(|i| i.value);
        *counted = vec![perf];
        s.utilization_percent = total("util");
        for (key, set) in [
            ("park", (|c: &mut CoreSample, v: f64| c.parked = Some(v != 0.0)) as fn(&mut CoreSample, f64)),
            ("util", |c, v| c.utilization_percent = Some(v)),
            ("limit", |c, v| c.performance_limit_percent = Some(v)),
            ("flags", |c, v| c.limit_flags = Some(v as u32)),
            ("c1", |c, v| c.c1_percent = Some(v)),
            ("c2", |c, v| c.c2_percent = Some(v)),
            ("c3", |c, v| c.c3_percent = Some(v)),
        ] {
            for (inst, v) in q.values(key) {
                if let Some(i) = logical_index(&inst) {
                    if s.cores.len() <= i {
                        s.cores.resize(i + 1, CoreSample::default());
                    }
                    set(&mut s.cores[i], v);
                }
            }
        }
        s
    }
}

/// The processor's marketing name, from the registry.
pub fn cpu_name() -> String {
        {
        use winreg::enums::HKEY_LOCAL_MACHINE;
        let key = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
        if let Ok(name) = key.and_then(|k| k.get_value::<String, _>("ProcessorNameString")) {
            return name.trim().to_string();
        }
    }
    String::new()
}

/// (MaximumMhz, CurrentMhz) of every logical processor.
fn processor_power() -> Vec<(f64, f64)> {
    use windows_sys::Win32::System::Power::{CallNtPowerInformation, ProcessorInformation, PROCESSOR_POWER_INFORMATION};
    use windows_sys::Win32::System::Threading::{GetActiveProcessorCount, ALL_PROCESSOR_GROUPS};
    // SAFETY: the buffer holds `n` entries and its size is passed.
    unsafe {
        let n = GetActiveProcessorCount(ALL_PROCESSOR_GROUPS) as usize;
        let mut buf: Vec<PROCESSOR_POWER_INFORMATION> = vec![std::mem::zeroed(); n];
        let size = (n * std::mem::size_of::<PROCESSOR_POWER_INFORMATION>()) as u32;
        if CallNtPowerInformation(ProcessorInformation, std::ptr::null(), 0, buf.as_mut_ptr().cast(), size) != 0 {
            return Vec::new();
        }
        buf.iter().map(|p| (p.MaxMhz as f64, p.CurrentMhz as f64)).collect()
    }
}
