//! macOS: the power plan is not read yet.

use crate::availability::{unavailable_error, Feature};
use crate::sensors::power_plan::PlanSample;

/// Whether the power plan can be read: not available yet.
pub fn availability() -> Result<(), String> {
    Err(unavailable_error(Feature::HardwareSensors))
}

/// A sample of the processor with no plan.
pub fn sample(cpu: &str) -> PlanSample {
    PlanSample { cpu: cpu.into(), ..Default::default() }
}

/// The active plan's name: none.
pub fn plan_name() -> Option<String> {
    None
}

/// The Windows power mode's overlay scheme: none.
pub fn overlay() -> Option<u128> {
    None
}
