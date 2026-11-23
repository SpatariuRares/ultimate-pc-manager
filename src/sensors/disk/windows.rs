//! Windows: the `\PhysicalDisk(*)` counters through a PDH query, and the
//! disks' direct S.M.A.R.T. queries.

use std::time::{Duration, Instant};

use crate::perfdata::counter;
use crate::sensors::disk::{DiskSample, DiskSmart};
use crate::sensors::model::{CounterSet, Instance};
use crate::sensors::pdh::Query;

/// Rate counters need this much time between two collections.
const MIN_SPAN: Duration = Duration::from_millis(200);

/// The disk counters query, if the counters are on.
pub struct Counters {
    query: Option<Query>,
    collected: Instant,
}

impl Counters {
    pub fn open() -> Self {
        Counters {
            query: Query::open(&[
                (counter::IDLE, r"\PhysicalDisk(*)\% Idle Time"),
                (counter::DREAD, r"\PhysicalDisk(*)\Disk Read Bytes/sec"),
                (counter::DWRITE, r"\PhysicalDisk(*)\Disk Write Bytes/sec"),
                (counter::DRESP, r"\PhysicalDisk(*)\Avg. Disk sec/Transfer"),
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

    pub fn sample(&mut self, models: &[(u32, String)], smart_disks: &mut [(u32, bool, Option<(DiskSmart, Instant)>)], health: bool, counted_out: &mut Vec<CounterSet>) -> Vec<DiskSample> {
        let mut disks: Vec<DiskSample> = models.iter().map(|(n, m)| DiskSample { number: *n, model: m.clone(), ..Default::default() }).collect();
        let now = Instant::now();
        let admin = crate::platform::is_admin();
        let consent = probe_consent();
        for (n, ata, last) in smart_disks.iter_mut().filter(|_| health) {
            let gate = if *ata { super::blocked(true, admin, consent).map(DiskSmart::Ata) } else { super::blocked(false, admin, consent).map(DiskSmart::Nvme) };
            let smart = match gate {
                Some(not_tried) => {
                    // Not cached: consenting (or elevating) reads at once.
                    *last = None;
                    not_tried
                }
                None => {
                    let (read, when) = match last.take() {
                        Some((cached, t)) if !super::smart_due(Some(t), now) => (cached, t),
                        _ => (if *ata { DiskSmart::Ata(super::ata::query(*n)) } else { DiskSmart::Nvme(super::nvme::query(*n)) }, now),
                    };
                    *last = Some((read.clone(), when));
                    read
                }
            };
            if let Some(d) = disks.iter_mut().find(|d| d.number == *n) {
                d.smart = Some(smart);
            }
        }
        let Some(q) = self.query.as_mut() else { return disks };
        let wait = MIN_SPAN.saturating_sub(self.collected.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
        q.collect();
        self.collected = Instant::now();
        let counted = [q.counted(counter::IDLE, false), q.counted(counter::DREAD, true), q.counted(counter::DWRITE, true)];
        for (counted, set) in counted.iter().zip([
            (|d: &mut DiskSample, v: f64| d.idle_percent = Some(v)) as fn(&mut DiskSample, f64),
            |d, v| d.read_bytes_per_sec = Some(v),
            |d, v| d.write_bytes_per_sec = Some(v),
        ]) {
            for Instance { name: inst, value: v, .. } in &counted.instances {
                let v = *v;
                let Some(n) = super::disk_number(inst) else { continue };
                let i = match disks.iter().position(|d| d.number == n) {
                    Some(i) => i,
                    None => {
                        disks.push(DiskSample { number: n, ..Default::default() });
                        disks.len() - 1
                    }
                };
                set(&mut disks[i], v);
            }
        }
        disks.sort_by_key(|d| d.number);
        // The average response time is only reported (the readings have no row for it).
        *counted_out = counted.into_iter().chain([q.counted(counter::DRESP, false)]).collect();
        disks
    }
}

fn probe_consent() -> bool {
    super::PROBE_CONSENT.load(std::sync::atomic::Ordering::Relaxed)
}
