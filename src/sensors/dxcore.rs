//! The DXCore provider: lists the video cards (hardware adapters with
//! Direct3D 12 graphics, so not the NPU nor the Basic Render Driver) and
//! gives each one its heading with grey rows saying why it has no values.
//! A vendor provider listed before it (NVML) fills the rows it can read.
//! It also opens the NPUs (machine-learning adapters) and reads their load,
//! temperature and clocks, or says "No NPU in this PC".

use std::time::Instant;

use super::gpu::{self, Adapter, GpuSample};
use super::model::Reading;
use super::npu::{self, NpuSample};
use super::provider::Provider;

/// The provider's name in "Sources on this PC" / "Read from".
pub const SOURCE: &str = "DXCore";

/// The provider. Lists the cards once, when built.
pub struct DxCore {
    listed: Result<Vec<Adapter>, String>,
    npus: Npus,
}

impl DxCore {
    pub fn new() -> Self {
        DxCore { listed: adapters(), npus: Npus::open() }
    }
}

/// What DXCore does not report about an NPU (power, hence interval energy).
const NOT_REPORTED: &str = "DXCore does not report it";

/// The NPUs (#58), opened once and read every interval: engine running
/// times become the load, between two reads.
struct Npus {
    #[cfg(windows)]
    session: Option<com::Session>,
    /// Each NPU's engines' running time at the last read.
    last: Vec<Vec<Option<u64>>>,
    at: Option<Instant>,
}

impl Npus {
    fn open() -> Self {
        Npus {
            #[cfg(windows)]
            session: com::Session::open().ok(),
            last: Vec::new(),
            at: None,
        }
    }

    #[cfg(not(windows))]
    fn read(&mut self, at: Instant) -> Vec<Reading> {
        npu::readings(&[], SOURCE, NOT_REPORTED, at)
    }

    #[cfg(windows)]
    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let elapsed = self.at.replace(at).map(|t| at.saturating_duration_since(t)).unwrap_or_default();
        let open = self.session.as_ref().map(|s| s.npus.as_slice()).unwrap_or_default();
        let mut last = Vec::new();
        let samples: Vec<NpuSample> = open
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let times: Vec<Option<u64>> = (0..n.engines).map(|e| n.running_time_us(e)).collect();
                let before = self.last.get(i);
                let load = times
                    .iter()
                    .enumerate()
                    .filter_map(|(e, t)| npu::busy_percent(before.and_then(|b| b.get(e).copied().flatten()), (*t)?, elapsed))
                    .reduce(f64::max);
                let clock = (0..n.engines).filter_map(|e| n.engine_hz(e)).reduce(f64::max);
                last.push(times);
                NpuSample {
                    name: n.name.clone(),
                    utilization_percent: load,
                    power_w: None,
                    energy_j: npu::interval_energy_j(None, elapsed),
                    temperature_c: n.temperature_c(),
                    engine_hz: clock,
                    memory_hz: n.memory_hz(),
                }
            })
            .collect();
        self.last = last;
        npu::readings(&samples, SOURCE, NOT_REPORTED, at)
    }
}

impl Provider for DxCore {
    fn name(&self) -> &'static str {
        SOURCE
    }

    fn availability(&mut self) -> Result<(), String> {
        self.listed.as_ref().map(|_| ()).map_err(Clone::clone)
    }

    fn read(&mut self, at: Instant) -> Vec<Reading> {
        let listed = self.listed.as_deref().unwrap_or_default();
        gpu::cards(listed)
            .iter()
            .zip(listed)
            .flat_map(|(card, a)| GpuSample::default().readings(card, SOURCE, &gpu::no_provider(a.vendor_id), at))
            .chain(self.npus.read(at))
            .collect()
    }
}

/// The video cards of this PC, in DXCore's order.
#[cfg(not(windows))]
pub fn adapters() -> Result<Vec<Adapter>, String> {
    Err("DXCore is a Windows interface".into())
}

/// The video cards of this PC, in DXCore's order.
#[cfg(windows)]
pub fn adapters() -> Result<Vec<Adapter>, String> {
    use windows_sys::Win32::Foundation::FreeLibrary;
    use windows_sys::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
    let name: Vec<u16> = "dxcore.dll\0".encode_utf16().collect();
    // SAFETY: the library is freed after every DXCore object was released
    // (`com::list` returns only plain data).
    unsafe {
        let lib = LoadLibraryExW(name.as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if lib.is_null() {
            return Err(com::MISSING.into());
        }
        let out = com::list(lib);
        FreeLibrary(lib);
        out
    }
}

/// DXCore's COM interfaces, called through their vtables
/// (`dxcore_interface.h`).
#[cfg(windows)]
mod com {
    use std::ffi::c_void;
    use std::mem::transmute;
    use std::ptr::null_mut;

    use windows_sys::core::GUID;
    use windows_sys::Win32::Foundation::HMODULE;
    use windows_sys::Win32::System::LibraryLoader::GetProcAddress;

    use super::Adapter;

    /// Why nothing is listed when dxcore.dll or its factory is missing.
    pub const MISSING: &str = "DXCore not available on this Windows";
    const IID_FACTORY: GUID = GUID::from_u128(0x78ee5945_c36e_4b13_a669_005dd11c0f06);
    const IID_LIST: GUID = GUID::from_u128(0x526c7776_40e9_459b_b711_f32ad76dfc28);
    const IID_ADAPTER: GUID = GUID::from_u128(0xf0db4c7f_fe5a_42a2_bd62_f2a6cf6fc83e);
    /// DXCORE_ADAPTER_ATTRIBUTE_D3D12_GRAPHICS.
    const D3D12_GRAPHICS: GUID = GUID::from_u128(0x0c9ece4d_2f6e_4f01_8c96_e89e331b47b1);
    // DXCoreAdapterProperty.
    const DRIVER_DESCRIPTION: u32 = 2;
    const HARDWARE_ID: u32 = 3;
    const IS_HARDWARE: u32 = 11;

    type Create = unsafe extern "system" fn(*const GUID, *mut *mut c_void) -> i32;
    type CreateList = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *const GUID, *mut *mut c_void) -> i32;
    type GetAdapter = unsafe extern "system" fn(*mut c_void, u32, *const GUID, *mut *mut c_void) -> i32;
    type Count = unsafe extern "system" fn(*mut c_void) -> u32;
    type GetProperty = unsafe extern "system" fn(*mut c_void, u32, usize, *mut c_void) -> i32;
    type GetPropertySize = unsafe extern "system" fn(*mut c_void, u32, *mut usize) -> i32;
    type Release = unsafe extern "system" fn(*mut c_void) -> u32;
    type IsAttributeSupported = unsafe extern "system" fn(*mut c_void, *const GUID) -> bool;
    type QueryState = unsafe extern "system" fn(*mut c_void, u32, usize, *const c_void, usize, *mut c_void) -> i32;
    type GetPropertyWithInput = unsafe extern "system" fn(*mut c_void, u32, usize, *const c_void, usize, *mut c_void) -> i32;
    type QueryInterface = unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> i32;

    /// A COM object, released when dropped.
    struct Com(*mut c_void);

    impl Com {
        /// The `i`-th method of its vtable.
        unsafe fn slot(&self, i: usize) -> *const c_void {
            *(*(self.0 as *const *const *const c_void)).add(i)
        }
    }

    impl Drop for Com {
        fn drop(&mut self) {
            // SAFETY: slot 2 is IUnknown::Release of a live object.
            unsafe {
                let release: Release = transmute(self.slot(2));
                release(self.0);
            }
        }
    }

    /// The hardware graphics adapters DXCore lists.
    pub unsafe fn list(lib: HMODULE) -> Result<Vec<Adapter>, String> {
        let create = GetProcAddress(lib, c"DXCoreCreateAdapterFactory".as_ptr().cast()).ok_or(MISSING)?;
        let create: Create = transmute(create);
        let mut p = null_mut();
        if create(&IID_FACTORY, &mut p) < 0 || p.is_null() {
            return Err("DXCore could not list the video cards".into());
        }
        let factory = Com(p);
        let create_list: CreateList = transmute(factory.slot(3));
        let mut p = null_mut();
        if create_list(factory.0, 1, &D3D12_GRAPHICS, &IID_LIST, &mut p) < 0 || p.is_null() {
            return Err("DXCore could not list the video cards".into());
        }
        let list = Com(p);
        let (get, count): (GetAdapter, Count) = (transmute(list.slot(3)), transmute(list.slot(4)));
        let mut out = Vec::new();
        for i in 0..count(list.0) {
            let mut p = null_mut();
            if get(list.0, i, &IID_ADAPTER, &mut p) >= 0 && !p.is_null() {
                out.extend(describe(&Com(p)));
            }
        }
        Ok(out)
    }

    /// A hardware adapter's name and PCI ids; `None` for software ones.
    unsafe fn describe(a: &Com) -> Option<Adapter> {
        let (get, size): (GetProperty, GetPropertySize) = (transmute(a.slot(6)), transmute(a.slot(7)));
        let mut hardware = 0u8;
        if get(a.0, IS_HARDWARE, 1, (&mut hardware as *mut u8).cast()) < 0 || hardware == 0 {
            return None;
        }
        // DXCoreHardwareID: vendorID, deviceID, subSysID, revision.
        let mut ids = [0u32; 4];
        if get(a.0, HARDWARE_ID, 16, ids.as_mut_ptr().cast()) < 0 {
            return None;
        }
        let mut n = 0usize;
        let mut name = String::new();
        if size(a.0, DRIVER_DESCRIPTION, &mut n) >= 0 && n > 0 {
            let mut buf = vec![0u8; n];
            if get(a.0, DRIVER_DESCRIPTION, n, buf.as_mut_ptr().cast()) >= 0 {
                name = String::from_utf8_lossy(&buf).trim_end_matches('\0').trim().to_string();
            }
        }
        Some(Adapter { name, vendor_id: ids[0], device_id: ids[1] })
    }

    // --- NPUs (#58) ---

    const IID_ADAPTER1: GUID = GUID::from_u128(0xa0783366_cfa3_43be_9d79_55b2da97c63c);
    /// DXCORE_ADAPTER_ATTRIBUTE_D3D12_GENERIC_ML.
    const GENERIC_ML: GUID = GUID::from_u128(0xb71b0d41_1088_422f_a27c_0250b7d3a988);
    /// DXCORE_HARDWARE_TYPE_ATTRIBUTE_NPU (Windows 11 24H2+).
    const NPU: GUID = GUID::from_u128(0xd46140c4_add7_451b_9e56_06fe8c3b58ed);
    const ENGINE_COUNT: u32 = 16;
    // DXCoreAdapterState.
    const ENGINE_RUNNING_TIME_US: u32 = 4;
    const TEMPERATURE_C: u32 = 6;
    const ENGINE_HZ: u32 = 9;
    const MEMORY_HZ: u32 = 10;

    /// DXCoreAdapterEngineIndex.
    #[repr(C)]
    #[derive(Default)]
    struct EngineIndex {
        pub physical: u32,
        pub engine: u32,
    }

    /// DXCoreEngineQueryInput (the process id is ignored by the
    /// whole-adapter query; checked on this PC's cards).
    #[repr(C)]
    #[derive(Default)]
    struct EngineQuery {
        index: EngineIndex,
        process_id: u32,
    }

    /// DXCoreEngineQueryOutput.
    #[repr(C)]
    #[allow(dead_code)] // the C struct's layout
    #[derive(Default)]
    struct EngineTime {
        pub running_us: u64,
        pub process_ok: bool,
    }

    /// DXCoreFrequencyQueryOutput.
    #[repr(C)]
    #[allow(dead_code)] // the C struct's layout
    #[derive(Default)]
    struct Frequency {
        pub hz: u64,
        pub max_hz: u64,
        pub max_overclocked_hz: u64,
    }

    /// `QueryState` with typed input and output; `None` when it fails.
    unsafe fn query<I, O: Default>(a: &Com, state: u32, input: &I) -> Option<O> {
        let q: QueryState = transmute(a.slot(9));
        let mut out = O::default();
        let hr = q(a.0, state, size_of::<I>(), (input as *const I).cast(), size_of::<O>(), (&mut out as *mut O).cast());
        (hr >= 0).then_some(out)
    }

    /// An NPU DXCore lists, kept open between reads.
    pub struct Npu {
        adapter: Com,
        pub name: String,
        pub engines: u32,
    }

    impl Npu {
        /// How long engine `e` has run so far (microseconds).
        pub fn running_time_us(&self, e: u32) -> Option<u64> {
            // SAFETY: a live adapter; the structs match dxcore_interface.h.
            let input = EngineQuery { index: EngineIndex { physical: 0, engine: e }, process_id: 0 };
            unsafe { query::<EngineQuery, EngineTime>(&self.adapter, ENGINE_RUNNING_TIME_US, &input) }
                .map(|t| t.running_us)
        }

        /// The temperature (°C).
        pub fn temperature_c(&self) -> Option<f64> {
            // SAFETY: as above; the output is a float (checked on this PC's cards).
            unsafe { query::<u32, f32>(&self.adapter, TEMPERATURE_C, &0) }.map(f64::from).filter(|t| *t > 0.0)
        }

        /// Engine `e`'s clock (Hz).
        pub fn engine_hz(&self, e: u32) -> Option<f64> {
            // SAFETY: as above.
            unsafe { query::<EngineIndex, Frequency>(&self.adapter, ENGINE_HZ, &EngineIndex { physical: 0, engine: e }) }
                .map(|f| f.hz as f64)
                .filter(|hz| *hz > 0.0)
        }

        /// The memory clock (Hz).
        pub fn memory_hz(&self) -> Option<f64> {
            // SAFETY: as above.
            unsafe { query::<u32, Frequency>(&self.adapter, MEMORY_HZ, &0) }.map(|f| f.hz as f64).filter(|hz| *hz > 0.0)
        }
    }

    /// The adapters DXCore lists with `attribute` (hardware or not).
    unsafe fn adapters_with(lib: HMODULE, attribute: &GUID) -> Result<Vec<Com>, String> {
        let create = GetProcAddress(lib, c"DXCoreCreateAdapterFactory".as_ptr().cast()).ok_or(MISSING)?;
        let create: Create = transmute(create);
        let mut p = null_mut();
        if create(&IID_FACTORY, &mut p) < 0 || p.is_null() {
            return Err(MISSING.into());
        }
        let factory = Com(p);
        let create_list: CreateList = transmute(factory.slot(3));
        let mut p = null_mut();
        if create_list(factory.0, 1, attribute, &IID_LIST, &mut p) < 0 || p.is_null() {
            return Ok(Vec::new());
        }
        let list = Com(p);
        let (get, count): (GetAdapter, Count) = (transmute(list.slot(3)), transmute(list.slot(4)));
        let mut out = Vec::new();
        for i in 0..count(list.0) {
            let mut p = null_mut();
            if get(list.0, i, &IID_ADAPTER, &mut p) >= 0 && !p.is_null() {
                out.push(Com(p));
            }
        }
        Ok(out)
    }

    /// The NPUs: hardware machine-learning adapters that DXCore calls NPUs
    /// or that have no Direct3D 12 graphics (older Windows).
    pub unsafe fn npus(lib: HMODULE) -> Result<Vec<Npu>, String> {
        let mut out = Vec::new();
        for adapter in adapters_with(lib, &GENERIC_ML)? {
            let supports: IsAttributeSupported = transmute(adapter.slot(4));
            let is_npu = supports(adapter.0, &NPU) || !supports(adapter.0, &D3D12_GRAPHICS);
            let Some(described) = describe(&adapter).filter(|_| is_npu) else { continue };
            let engines = engine_count(&adapter).unwrap_or(1).max(1);
            out.push(Npu { adapter, name: described.name, engines });
        }
        Ok(out)
    }

    /// The engines of physical adapter 0 (`IDXCoreAdapter1`).
    unsafe fn engine_count(a: &Com) -> Option<u32> {
        let qi: QueryInterface = transmute(a.slot(0));
        let mut p = null_mut();
        if qi(a.0, &IID_ADAPTER1, &mut p) < 0 || p.is_null() {
            return None;
        }
        let a1 = Com(p);
        let get: GetPropertyWithInput = transmute(a1.slot(13));
        let (input, mut n) = (0u32, 0u32);
        let hr = get(a1.0, ENGINE_COUNT, 4, (&input as *const u32).cast(), 4, (&mut n as *mut u32).cast());
        (hr >= 0).then_some(n)
    }

    /// dxcore.dll and the NPUs opened from it; dropped = released, then freed.
    pub struct Session {
        lib: HMODULE,
        pub npus: Vec<Npu>,
    }

    impl Session {
        /// Loads dxcore.dll and opens its NPUs.
        pub fn open() -> Result<Session, String> {
            use windows_sys::Win32::System::LibraryLoader::{LoadLibraryExW, LOAD_LIBRARY_SEARCH_SYSTEM32};
            let name: Vec<u16> = "dxcore.dll\0".encode_utf16().collect();
            // SAFETY: the library outlives every object (see `Drop`).
            unsafe {
                let lib = LoadLibraryExW(name.as_ptr(), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
                if lib.is_null() {
                    return Err(MISSING.into());
                }
                let mut s = Session { lib, npus: Vec::new() };
                s.npus = npus(lib)?;
                Ok(s)
            }
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            self.npus.clear();
            // SAFETY: every DXCore object was released just above.
            unsafe { windows_sys::Win32::Foundation::FreeLibrary(self.lib) };
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "reads this PC's video cards through the NPU queries"]
        fn npu_queries_work_on_this_pcs_video_cards() {
            let s = Session::open().expect("DXCore");
            for a in unsafe { adapters_with(s.lib, &D3D12_GRAPHICS) }.unwrap() {
                let gpu = Npu { name: unsafe { describe(&a) }.map(|d| d.name).unwrap_or_default(), engines: unsafe { engine_count(&a) }.unwrap_or(0), adapter: a };
                let busy: Vec<_> = (0..gpu.engines).map(|e| gpu.running_time_us(e)).collect();
                let clocks: Vec<_> = (0..gpu.engines).map(|e| gpu.engine_hz(e)).collect();
                println!("{}: {} engines, running {busy:?}, clocks {clocks:?}, temp {:?}, memory {:?}", gpu.name, gpu.engines, gpu.temperature_c(), gpu.memory_hz());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "reads this PC's video cards"]
    fn lists_this_pc() {
        let listed = adapters().expect("DXCore works on Windows 10 2004+");
        for a in &listed {
            println!("{:<40} {:04X}:{:04X}", a.name, a.vendor_id, a.device_id);
        }
        assert!(listed.iter().any(|a| a.vendor_id == gpu::NVIDIA && a.name.contains("RTX 4080")), "the RTX 4080");
        assert!(listed.iter().all(|a| !a.name.contains("Basic Render")), "no software adapter");
    }

    #[test]
    #[ignore = "reads this PC's NPUs"]
    fn this_pc_has_no_npu() {
        let mut provider = DxCore::new();
        let rows = provider.read(Instant::now());
        let frame = crate::sensors::Frame::build(0, &[rows], vec![]);
        let npus: Vec<String> = frame.readings().filter(|r| r.id.starts_with("npu")).map(|r| format!("{} = {}", r.id, r.text())).collect();
        println!("{npus:#?}");
        assert_eq!(npus, vec![format!("npu.present = {}", npu::NO_NPU)]);
    }
}
