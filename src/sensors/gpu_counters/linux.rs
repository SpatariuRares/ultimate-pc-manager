//! Linux: the GPU counters from the kernel's DRM interface, in the shape the
//! Sampler already builds the GPU page from (see `perfdata::gpu_adapters`).
//!
//! - `/sys/class/drm/card<N>/device`: `gpu_busy_percent` (AMD) is the whole
//!   card's utilisation; `mem_info_vram_used` and `mem_info_gtt_used` are the
//!   dedicated and shared memory in bytes. A card that does not expose a
//!   file simply has no such figure.
//! - `/proc/<pid>/fdinfo/<fd>`: a process that has a card open reports its
//!   busy time per engine (`drm-engine-<name>: <n> ns`) under the card's PCI
//!   address (`drm-pdev`) and a client id. The engines' share of the time
//!   between two reads is each process's utilisation of that engine; where
//!   the driver reports no such lines (or the first read has no earlier one
//!   to compare with), there are no engines, only the card's figure.
//!
//! The card's number (`N`) plays the part of Windows' adapter id: the
//! instances are named like the Windows counters' ones (`pid_<pid>_luid_..
//! _eng_<i>_engtype_<Type>`), so the page and the per-process figure need no
//! second code path. Every read goes through an [`FsRoot`].

use std::collections::HashMap;
use std::time::Instant;

use crate::availability::{unavailable_error, Feature};
use crate::fs_root::FsRoot;
use crate::perfdata::counter;
use crate::sensors::model::{CounterSet, Instance};

/// The GPU counters query, over a root.
pub struct Counters {
    fs: FsRoot,
    /// Busy nanoseconds of the last read, per process, client and engine.
    last: Option<(Instant, HashMap<Key, u64>)>,
}

/// A process's client of a card and one engine of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    pid: u32,
    client: String,
    card: u32,
    engine: String,
}

/// One card of `/sys/class/drm`.
#[derive(Debug)]
struct Card {
    index: u32,
    /// `0000:03:00.0`, as `drm-pdev` names it.
    pci: Option<String>,
    busy_percent: Option<f64>,
    vram_used: Option<f64>,
    gtt_used: Option<f64>,
}

impl Counters {
    pub fn open() -> Self {
        Counters::over(FsRoot::system())
    }

    fn over(fs: FsRoot) -> Self {
        Counters { fs, last: None }
    }

    pub fn availability(&self) -> Result<(), String> {
        if cards(&self.fs).is_empty() {
            return Err(unavailable_error(Feature::HardwareSensors));
        }
        Ok(())
    }

    /// What the cards count now; `None` when there is no card.
    pub fn read(&mut self) -> Option<Vec<CounterSet>> {
        self.read_at(Instant::now())
    }

    fn read_at(&mut self, now: Instant) -> Option<Vec<CounterSet>> {
        let cards = cards(&self.fs);
        if cards.is_empty() {
            return None;
        }
        let busy = clients(&self.fs, &cards);
        let mut engines: Vec<Instance> = Vec::new();
        let mut dedicated: Vec<Instance> = Vec::new();
        let mut shared: Vec<Instance> = Vec::new();
        for card in &cards {
            if let Some(percent) = card.busy_percent {
                engines.push(Instance::new(&format!("{}_eng_0_engtype_Overall", luid(card.index)), percent));
            }
            if let Some(bytes) = card.vram_used {
                dedicated.push(Instance::new(&luid(card.index), bytes));
            }
            if let Some(bytes) = card.gtt_used {
                shared.push(Instance::new(&luid(card.index), bytes));
            }
        }
        if let Some((then, before)) = &self.last {
            let span = now.saturating_duration_since(*then).as_nanos() as f64;
            // The engines of each card, numbered after the whole-card figure.
            let mut numbers: HashMap<u32, Vec<&str>> = HashMap::new();
            for key in busy.keys() {
                let names = numbers.entry(key.card).or_default();
                if !names.contains(&key.engine.as_str()) {
                    names.push(&key.engine);
                }
            }
            numbers.values_mut().for_each(|names| names.sort_unstable());
            let mut keys: Vec<&Key> = busy.keys().collect();
            keys.sort_by(|a, b| (a.card, &a.engine, a.pid, &a.client).cmp(&(b.card, &b.engine, b.pid, &b.client)));
            for key in keys {
                let Some(earlier) = before.get(key) else { continue };
                let percent = percent_of(busy[key].saturating_sub(*earlier), span);
                let number = numbers[&key.card].iter().position(|n| *n == key.engine).unwrap_or(0) + 1;
                let name = format!("pid_{}_{}_eng_{number}_engtype_{}", key.pid, luid(key.card), engine_type(&key.engine));
                engines.push(Instance::new(&name, percent));
            }
        }
        self.last = Some((now, busy));
        Some(vec![CounterSet::new(counter::GPU, engines), CounterSet::new(counter::GDED, dedicated), CounterSet::new(counter::GSH, shared)])
    }
}

/// The share of `span` nanoseconds that `busy` nanoseconds make, in percent.
fn percent_of(busy: u64, span: f64) -> f64 {
    if span <= 0.0 {
        return 0.0;
    }
    (busy as f64 / span * 100.0).clamp(0.0, 100.0)
}

/// How the Windows counters name an adapter: the card's number as its id.
fn luid(card: u32) -> String {
    format!("luid_0x00000000_0x{card:08X}_phys_0")
}

/// The engine type a DRM engine name stands for, spelled as the page's engine
/// names are (see `perfdata`): `render` and `gfx` are 3D, `dma`/`blitter` copy.
fn engine_type(drm: &str) -> String {
    match drm {
        "render" | "gfx" => "3D",
        "compute" => "Compute",
        "copy" | "dma" | "blitter" => "Copy",
        "video" => "Video",
        "dec" => "VideoDecode",
        "enc" => "VideoEncode",
        "video-enhance" | "vecs" => "VideoProcessing",
        other => return other.replace('-', "_"),
    }
    .into()
}

/// The cards of `sys/class/drm` (`card<N>`, not their connectors nor render
/// nodes), by number.
fn cards(fs: &FsRoot) -> Vec<Card> {
    let Ok(entries) = std::fs::read_dir(fs.path("sys/class/drm")) else { return Vec::new() };
    let mut cards: Vec<Card> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let index: u32 = name.strip_prefix("card")?.parse().ok()?;
            let device = |file: &str| fs.read(&format!("sys/class/drm/{name}/device/{file}"));
            device("vendor")?;
            let number = |file: &str| device(file).and_then(|t| t.parse::<f64>().ok());
            let pci = device("uevent").and_then(|u| u.lines().find_map(|l| l.strip_prefix("PCI_SLOT_NAME=").map(str::to_string)));
            Some(Card {
                index,
                pci,
                busy_percent: number("gpu_busy_percent").map(|p| p.clamp(0.0, 100.0)),
                vram_used: number("mem_info_vram_used"),
                gtt_used: number("mem_info_gtt_used"),
            })
        })
        .collect();
    cards.sort_by_key(|c| c.index);
    cards
}

/// The busy nanoseconds of every process's clients of `cards`, per engine,
/// from `proc/<pid>/fdinfo`. Several descriptors of one client count once; a
/// process whose files cannot be read is left out.
fn clients(fs: &FsRoot, cards: &[Card]) -> HashMap<Key, u64> {
    let mut found = HashMap::new();
    let Ok(procs) = std::fs::read_dir(fs.path("proc")) else { return found };
    for process in procs.flatten() {
        let Some(pid) = process.file_name().to_string_lossy().parse::<u32>().ok() else { continue };
        let Ok(fds) = std::fs::read_dir(process.path().join("fdinfo")) else { continue };
        for fd in fds.flatten() {
            let Ok(text) = std::fs::read_to_string(fd.path()) else { continue };
            if !text.contains("drm-pdev:") {
                continue;
            }
            let field = |name: &str| text.lines().find_map(|l| l.strip_prefix(name)?.strip_prefix(':').map(str::trim));
            let (Some(pdev), Some(client)) = (field("drm-pdev"), field("drm-client-id")) else { continue };
            let Some(card) = cards.iter().find(|c| c.pci.as_deref() == Some(pdev)) else { continue };
            for line in text.lines() {
                let Some(rest) = line.strip_prefix("drm-engine-") else { continue };
                let Some((engine, value)) = rest.split_once(':') else { continue };
                // `drm-engine-capacity-<name>` says how many engines there are, not how busy.
                if engine.starts_with("capacity-") {
                    continue;
                }
                let Some(ns) = value.split_whitespace().next().and_then(|n| n.parse::<u64>().ok()) else { continue };
                found.insert(Key { pid, client: client.to_string(), card: card.index, engine: engine.to_string() }, ns);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;
    use std::time::Duration;

    fn counted(sets: &[CounterSet], key: &str) -> Vec<(String, f64)> {
        let set = sets.iter().find(|s| s.key == key).unwrap_or_else(|| panic!("no counter {key}"));
        let mut all: Vec<_> = set.instances.iter().map(|i| (i.name.clone(), i.value)).collect();
        all.sort_by(|a, b| a.0.cmp(&b.0));
        all
    }

    #[test]
    fn an_amd_card_gives_its_busy_percent_and_its_memory_and_an_intel_one_gives_none_it_does_not_expose() {
        let mut counters = Counters::over(fixture("gpu-amd-intel"));
        assert_eq!(counters.availability(), Ok(()));
        let sets = counters.read_at(Instant::now()).expect("cards");
        let amd = "luid_0x00000000_0x00000000_phys_0";
        assert_eq!(counted(&sets, counter::GPU), vec![(format!("{amd}_eng_0_engtype_Overall"), 37.0)], "no engines yet: the first read has nothing to compare with");
        assert_eq!(counted(&sets, counter::GDED), vec![(amd.to_string(), 2_147_483_648.0)]);
        assert_eq!(counted(&sets, counter::GSH), vec![(amd.to_string(), 536_870_912.0)], "the Intel card has no memory file: no figure");
    }

    #[test]
    fn the_engines_a_driver_reports_are_each_processs_share_of_the_time_between_two_reads() {
        let mut counters = Counters::over(fixture("gpu-amd-intel"));
        let t = Instant::now();
        counters.read_at(t);
        counters.fs = fixture("gpu-amd-intel-later");
        let sets = counters.read_at(t + Duration::from_secs(1)).expect("cards");
        let gpu = counted(&sets, counter::GPU);
        let value = |name: &str| gpu.iter().find(|(n, _)| n == name).unwrap_or_else(|| panic!("no {name} in {gpu:?}")).1;
        // The AMD process: 0.5 s of gfx in 1 s (two descriptors of one client count once); compute stayed idle.
        assert_eq!(value("pid_4242_luid_0x00000000_0x00000000_phys_0_eng_2_engtype_3D"), 50.0);
        assert_eq!(value("pid_4242_luid_0x00000000_0x00000000_phys_0_eng_1_engtype_Compute"), 0.0);
        // The Intel process: a quarter of a second of render; video idle.
        assert_eq!(value("pid_300_luid_0x00000000_0x00000001_phys_0_eng_1_engtype_3D"), 25.0);
        assert_eq!(value("pid_300_luid_0x00000000_0x00000001_phys_0_eng_2_engtype_Video"), 0.0);
        assert_eq!(value("luid_0x00000000_0x00000000_phys_0_eng_0_engtype_Overall"), 37.0);
        assert_eq!(gpu.len(), 5, "capacity lines and the process with no card are not engines: {gpu:?}");
    }

    #[test]
    fn the_sampler_builds_each_adapters_engines_and_each_processs_share_from_what_is_counted() {
        let mut counters = Counters::over(fixture("gpu-amd-intel"));
        let t = Instant::now();
        counters.read_at(t);
        counters.fs = fixture("gpu-amd-intel-later");
        let sets = counters.read_at(t + Duration::from_secs(1)).expect("cards");
        let pairs = |key: &str| counted(&sets, key);
        let adapters = crate::perfdata::gpu_adapters(&pairs(counter::GPU), &pairs(counter::GDED), &pairs(counter::GSH));
        assert_eq!(adapters.len(), 2);
        let amd = &adapters[0];
        assert_eq!((amd.util, amd.dedicated, amd.shared), (50.0, 2_147_483_648, 536_870_912), "the busiest engine, the memory");
        assert_eq!(amd.engines.iter().map(|e| e.0.as_str()).collect::<Vec<_>>(), vec!["Overall", "Compute", "3D"]);
        assert_eq!(adapters[1].util, 25.0);
        let by_pid = crate::perfdata::gpu_by_pid(&pairs(counter::GPU));
        assert_eq!((by_pid[&4242], by_pid[&300]), (50.0, 25.0), "each process's share of its card");
    }

    #[test]
    fn a_pc_with_no_drm_card_has_no_gpu_counters() {
        let mut counters = Counters::over(fixture("arm-board"));
        assert!(counters.availability().is_err());
        assert_eq!(counters.read_at(Instant::now()), None);
    }

    #[test]
    fn the_names_of_drm_engines_read_as_the_pages_engine_types() {
        assert_eq!(engine_type("render"), "3D");
        assert_eq!(engine_type("gfx"), "3D");
        assert_eq!(engine_type("copy"), "Copy");
        assert_eq!(engine_type("video-enhance"), "VideoProcessing");
        assert_eq!(engine_type("something-new"), "something_new", "an engine nobody named yet keeps its own name");
        assert_eq!(percent_of(500, 1000.0), 50.0);
        assert_eq!(percent_of(5000, 1000.0), 100.0, "more busy time than wall time is a full engine");
    }
}
