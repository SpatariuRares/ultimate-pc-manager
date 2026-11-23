//! macOS: DXCore is a Windows interface, so there are no cards or NPUs to list.

use std::time::Instant;

use crate::sensors::dxcore::{NOT_REPORTED, SOURCE};
use crate::sensors::gpu::Adapter;
use crate::sensors::model::Reading;
use crate::sensors::npu;

/// The NPUs: none, not available yet.
pub struct Npus;

impl Npus {
    pub fn open() -> Self {
        Npus
    }

    pub fn read(&mut self, at: Instant) -> Vec<Reading> {
        npu::readings(&[], SOURCE, NOT_REPORTED, at)
    }
}

/// The video cards of this PC: not available on this system.
pub fn adapters() -> Result<Vec<Adapter>, String> {
    Err("DXCore is a Windows interface".into())
}
