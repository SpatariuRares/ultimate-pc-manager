//! Linux: which processors are performance and efficiency cores, and the
//! processor's fixed facts (sockets, cores, caches, virtualisation, rated
//! clock), read from `/sys/devices` and `/proc/cpuinfo` through an [`FsRoot`].
//! Every interface is probed, never inferred from the kernel version.

use std::collections::{BTreeMap, BTreeSet};

use crate::fs_root::FsRoot;
use crate::perfdata::CpuInfo;

const CPU_DIR: &str = "sys/devices/system/cpu";

/// A processor whose top speed is below this share of the fastest one's is
/// an efficiency core, when the kernel names no core types.
const CLUSTER_RATIO: f64 = 0.85;

/// The numbers of a kernel CPU list ("0-3,8,10-11").
pub fn parse_cpu_list(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((a, b)) => {
                if let (Ok(a), Ok(b)) = (a.trim().parse::<usize>(), b.trim().parse::<usize>()) {
                    out.extend(a..=b);
                }
            }
            None => out.extend(part.trim().parse::<usize>()),
        }
    }
    out
}

/// The logical processors the kernel knows, by number.
fn logical_cpus(root: &FsRoot) -> Vec<usize> {
    let list = root.read_first(&[&format!("{CPU_DIR}/present"), &format!("{CPU_DIR}/online"), &format!("{CPU_DIR}/possible")]);
    list.map(|l| parse_cpu_list(&l)).unwrap_or_default()
}

fn read_number(root: &FsRoot, rel: &str) -> Option<u64> {
    root.read(rel)?.parse().ok()
}

/// A cache size as the kernel writes it ("32K", "8192K", "32M") in bytes.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let (digits, unit) = text.split_at(text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len()));
    let n: u64 = digits.parse().ok()?;
    let scale = match unit.trim().to_ascii_uppercase().as_str() {
        "" => 1,
        "K" => 1 << 10,
        "M" => 1 << 20,
        "G" => 1 << 30,
        _ => return None,
    };
    Some(n * scale)
}

/// The efficiency class of every logical processor (1 performance, 0
/// efficiency); empty when the processor is not hybrid. Hybrid cores come
/// from the per-core-type devices (`cpu_core`, `cpu_atom`); without them the
/// processors whose top speed is well below the fastest one's are the
/// efficiency cores.
pub fn efficiency_classes_at(root: &FsRoot) -> Vec<u8> {
    let cpus = logical_cpus(root);
    let Some(len) = cpus.iter().max().map(|m| m + 1) else { return Vec::new() };
    let atoms = root.read("sys/devices/cpu_atom/cpus").map(|l| parse_cpu_list(&l)).unwrap_or_default();
    if !atoms.is_empty() && root.probe("sys/devices/cpu_core/cpus") {
        let mut classes = vec![1u8; len];
        for a in atoms.into_iter().filter(|a| *a < len) {
            classes[a] = 0;
        }
        return classes;
    }
    let max_freq = |n: usize| read_number(root, &format!("{CPU_DIR}/cpu{n}/cpufreq/cpuinfo_max_freq"));
    let freqs: Vec<Option<u64>> = (0..len).map(max_freq).collect();
    classes_from_max_frequencies(&freqs)
}

/// The classes of processors told apart by their top speeds (kHz, `None` for
/// an unknown one); empty when they are one cluster or any speed is unknown.
pub fn classes_from_max_frequencies(freqs: &[Option<u64>]) -> Vec<u8> {
    let Some(known) = freqs.iter().copied().collect::<Option<Vec<u64>>>() else { return Vec::new() };
    let Some(&top) = known.iter().max() else { return Vec::new() };
    let efficient = |f: u64| (f as f64) < top as f64 * CLUSTER_RATIO;
    if !known.iter().any(|f| efficient(*f)) {
        return Vec::new();
    }
    known.iter().map(|f| u8::from(!efficient(*f))).collect()
}

/// The efficiency classes of this PC.
pub fn efficiency_classes() -> Vec<u8> {
    efficiency_classes_at(&FsRoot::system())
}

/// The processor's marketing name in the text of `/proc/cpuinfo`.
pub fn cpu_name_from(cpuinfo: &str) -> String {
    for key in ["model name", "Model Name", "cpu model", "Hardware", "Processor"] {
        let found = cpuinfo.lines().filter_map(|l| l.split_once(':')).find(|(k, v)| k.trim() == key && !v.trim().is_empty());
        if let Some((_, v)) = found {
            return v.trim().to_string();
        }
    }
    String::new()
}

/// A virtual machine's makers, as the firmware names them.
const VM_MARKERS: [&str; 8] = ["virtual machine", "vmware", "virtualbox", "kvm", "qemu", "xen", "parallels", "hyper-v"];

/// The processor's fixed facts: sockets, physical cores, cache sizes, whether
/// virtualisation is on offer or this is a virtual machine, and the rated
/// clock. A fact the kernel does not give stays at its blank value.
pub fn cpu_facts(root: &FsRoot) -> CpuInfo {
    let cpus = logical_cpus(root);
    let mut sockets = BTreeSet::new();
    let mut cores = BTreeSet::new();
    let mut caches: BTreeMap<(u64, String, String), u64> = BTreeMap::new();
    for n in &cpus {
        let dir = format!("{CPU_DIR}/cpu{n}");
        let package = read_number(root, &format!("{dir}/topology/physical_package_id"));
        let core = read_number(root, &format!("{dir}/topology/core_id"));
        if let Some(p) = package {
            sockets.insert(p);
            cores.insert((p, core.unwrap_or(*n as u64)));
        }
        for k in 0..16 {
            let cache = format!("{dir}/cache/index{k}");
            let (Some(level), Some(size)) = (read_number(root, &format!("{cache}/level")), root.read(&format!("{cache}/size")).and_then(|s| parse_size(&s))) else { continue };
            let kind = root.read(&format!("{cache}/type")).unwrap_or_default();
            let shared = root.read(&format!("{cache}/shared_cpu_list")).unwrap_or_else(|| n.to_string());
            caches.insert((level, kind, shared), size);
        }
    }
    let level = |l: u64| caches.iter().filter(|((lv, ..), _)| *lv == l).map(|(_, s)| *s).sum::<u64>();
    let mut info = CpuInfo {
        sockets: sockets.len() as u32,
        cores: if cores.is_empty() { cpus.len() as u32 } else { cores.len() as u32 },
        l1: level(1),
        l2: level(2),
        l3: level(3),
        ..Default::default()
    };
    let cpuinfo = root.read("proc/cpuinfo").unwrap_or_default();
    let flags: Option<Vec<&str>> = cpuinfo.lines().filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim() == "flags").map(|(_, v)| v.split_whitespace().collect());
    let hypervisor = flags.as_ref().is_some_and(|f| f.contains(&"hypervisor")) || root.probe("sys/hypervisor/type");
    let hardware = flags.as_ref().is_some_and(|f| f.contains(&"vmx") || f.contains(&"svm"));
    info.virtualization = flags.map(|_| hardware || hypervisor);
    let maker = [root.read("sys/class/dmi/id/sys_vendor"), root.read("sys/class/dmi/id/product_name")].into_iter().flatten().collect::<Vec<_>>().join(" ").to_lowercase();
    info.vm = hypervisor || VM_MARKERS.iter().any(|m| maker.contains(m));
    info.base_mhz = base_mhz(root, cpus.first().copied().unwrap_or(0), &cpuinfo);
    info
}

/// The rated clock in MHz: the cpufreq base clock, else the processor's top
/// speed, else the first clock `/proc/cpuinfo` shows; 0 when unknown.
fn base_mhz(root: &FsRoot, cpu: usize, cpuinfo: &str) -> u32 {
    let freq = |name: &str| read_number(root, &format!("{CPU_DIR}/cpu{cpu}/cpufreq/{name}")).filter(|k| *k > 0);
    if let Some(khz) = freq("base_frequency").or_else(|| freq("cpuinfo_max_freq")) {
        return (khz / 1000) as u32;
    }
    let shown = cpuinfo.lines().filter_map(|l| l.split_once(':')).find(|(k, _)| k.trim() == "cpu MHz");
    shown.and_then(|(_, v)| v.trim().parse::<f64>().ok()).map_or(0, |m| m.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs_root::fixture;

    #[test]
    fn a_cpu_list_expands_ranges_and_singles() {
        assert_eq!(parse_cpu_list("0-2,5,8-9"), vec![0, 1, 2, 5, 8, 9]);
        assert_eq!(parse_cpu_list("\n"), Vec::<usize>::new());
    }

    #[test]
    fn hybrid_cores_come_from_the_core_type_devices() {
        assert_eq!(efficiency_classes_at(&fixture("cpu-modern")), vec![1, 1, 0, 0]);
    }

    #[test]
    fn an_old_kernel_without_core_type_devices_and_one_cluster_is_not_hybrid() {
        assert_eq!(efficiency_classes_at(&fixture("cpu-old")), Vec::<u8>::new());
    }

    #[test]
    fn without_core_type_devices_the_slower_clusters_are_efficiency_cores() {
        let khz = |v: &[u64]| v.iter().map(|f| Some(*f)).collect::<Vec<_>>();
        assert_eq!(classes_from_max_frequencies(&khz(&[3_000_000, 3_000_000, 1_800_000, 1_800_000])), vec![1, 1, 0, 0]);
        assert_eq!(classes_from_max_frequencies(&khz(&[4_900_000, 4_700_000, 4_800_000])), Vec::<u8>::new(), "favoured cores are not a second cluster");
        assert_eq!(classes_from_max_frequencies(&[Some(3_000_000), None]), Vec::<u8>::new(), "an unknown speed tells nothing");
    }

    #[test]
    fn a_current_kernel_gives_sockets_cores_caches_and_the_rated_clock() {
        let c = cpu_facts(&fixture("cpu-modern"));
        assert_eq!((c.sockets, c.cores), (1, 4));
        // Two cores' own L1 (data and instruction) and L2; the shared L3 once.
        assert_eq!((c.l1, c.l2, c.l3), (4 * 32 * 1024, 2 * 1024 * 1024, 8 * 1024 * 1024));
        assert_eq!(c.base_mhz, 2400);
    }

    #[test]
    fn hardware_virtualisation_without_a_hypervisor_is_a_real_pc() {
        let c = cpu_facts(&fixture("cpu-modern"));
        assert_eq!((c.virtualization, c.vm), (Some(true), false));
    }

    #[test]
    fn an_old_virtual_machine_is_told_by_its_hypervisor_flag_and_has_no_caches() {
        let c = cpu_facts(&fixture("cpu-old"));
        assert_eq!((c.sockets, c.cores, c.l1, c.l2, c.l3), (1, 2, 0, 0, 0));
        assert_eq!((c.virtualization, c.vm), (Some(true), true));
        assert_eq!(c.base_mhz, 2800, "the top speed when no base clock is given");
    }

    #[test]
    fn the_marketing_name_comes_from_cpuinfo() {
        use crate::fs_root::fixture as f;
        assert_eq!(cpu_name_from(&f("cpu-modern").read("proc/cpuinfo").unwrap()), "Test Hybrid CPU 12th Gen");
        assert_eq!(cpu_name_from("Hardware\t: BCM2711\n"), "BCM2711");
        assert_eq!(cpu_name_from(""), "");
    }

    #[test]
    fn sizes_are_read_in_bytes() {
        assert_eq!((parse_size("32K"), parse_size("8M"), parse_size("512"), parse_size("x")), (Some(32768), Some(8 << 20), Some(512), None));
    }
}
