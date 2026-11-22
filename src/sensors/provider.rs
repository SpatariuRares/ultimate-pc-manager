//! The interface every sensor provider implements, and the list of the
//! providers the app uses. A new provider (Energy Meter, battery, disks,
//! NVML, ...) is a module under `sensors/` plus one line in [`standard`].

use std::sync::Arc;
use std::time::{Duration, Instant};

use super::model::{CounterSet, Reading};
use super::service::ProviderGroup;

/// How long a read may take before the provider counts as not responding.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);

/// A source of readings. It is built on, read on and dropped on its own
/// worker thread (so it need not be `Send`: PDH handles, vendor DLLs), and
/// dropped when nobody asks for sensors any more (DLLs unloaded then).
pub trait Provider {
    /// Its name in "Sources on this PC" and "Read from" ("Windows counters").
    fn name(&self) -> &'static str;
    /// Whether it can work on this PC; `Err` says why not ("needs
    /// administrator", "no NVIDIA GPU"). Checked once, before any read.
    fn availability(&mut self) -> Result<(), String> {
        Ok(())
    }
    /// Whether this PC has the hardware it is for (an AMD processor for the
    /// Ryzen SDK, an NVIDIA card for NVML). When it is unavailable and not for
    /// this PC, it is left out of "Sources on this PC" and shows no rows, so
    /// an Intel PC never sees AMD's sources. Asked only when
    /// [`Provider::availability`] fails. Default: yes.
    fn for_this_pc(&mut self) -> bool {
        true
    }
    /// The rows it would fill, unavailable for `why`, listed when
    /// [`Provider::availability`] fails (e.g. "needs AMD's add-on" on every
    /// Ryzen power row). Default: none.
    fn unavailable_readings(&mut self, _why: &str, _at: Instant) -> Vec<Reading> {
        Vec::new()
    }
    /// Reads what it can now. Partial results are fine: a reading it cannot
    /// get is returned unavailable with its reason, or left out.
    fn read(&mut self, at: Instant) -> Vec<Reading>;
    /// What the last [`Provider::read`] counted, for what is more than one
    /// plain number (see [`CounterSet`]); called once after each read, on the
    /// same thread. Default: nothing.
    fn counted(&mut self) -> Vec<CounterSet> {
        Vec::new()
    }
    /// The longest one read may take before its readings are reported as
    /// "not responding" (the other providers go on meanwhile).
    fn timeout(&self) -> Duration {
        DEFAULT_TIMEOUT
    }
}

/// Builds a provider on its worker thread.
pub type Factory = Arc<dyn Fn() -> Box<dyn Provider> + Send + Sync>;

/// The providers of the real app, in the order their sources are listed,
/// each with the group that wakes it (see [`ProviderGroup`]).
pub fn standard() -> Vec<(ProviderGroup, Factory)> {
    use ProviderGroup::{Counters, Hardware};
    fn of(group: ProviderGroup, factory: Factory) -> (ProviderGroup, Factory) {
        (group, factory)
    }
    vec![
        of(Counters, Arc::new(|| Box::new(super::cpu::CpuCounters::new()))),
        of(Counters, Arc::new(|| Box::new(super::energy::EnergyMeter::new()))),
        of(Hardware, Arc::new(|| Box::new(super::ryzen::RyzenMaster::new()))),
        of(Hardware, Arc::new(|| Box::new(super::power_plan::PowerPlan::new()))),
        of(Counters, Arc::new(|| Box::new(super::disk::DiskCounters::new()))),
        of(Hardware, Arc::new(|| Box::new(super::power::PowerStatusProvider::new()))),
        // Vendor GPU providers before DXCore: their values and reasons win
        // over DXCore's grey rows for the same card.
        of(Hardware, Arc::new(|| Box::new(super::nvml::Nvml::new()))),
        of(Hardware, Arc::new(|| Box::new(super::adlx::provider()))),
        of(Hardware, Arc::new(|| Box::new(super::igcl::provider()))),
        of(Hardware, Arc::new(|| Box::new(super::dxcore::DxCore::new()))),
        of(Hardware, Arc::new(|| Box::new(super::board::FirmwareTables::new()))),
        of(Hardware, Arc::new(|| Box::new(super::intel_dtt::IntelDtt::new()))),
        // A new provider: one line here (its module under `sensors/`), in
        // the group that should wake it.
    ]
}
