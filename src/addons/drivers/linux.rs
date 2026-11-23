//! The Linux reader: not available yet, so every list says so.

use super::engine::{DeviceDriver, KernelDriver, SecurityReading};
use super::filters::FilterRead;
use super::store::StorePackage;
use super::ListingReader;
use crate::availability::{unavailable_error, Feature};

/// The Drivers add-on's reader on a real PC.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemListing;

impl ListingReader for SystemListing {
    fn devices(&self) -> Result<Vec<DeviceDriver>, String> {
        Err(unavailable_error(Feature::DeviceSetupApi))
    }
    fn kernel(&self) -> Result<Vec<KernelDriver>, String> {
        Err(unavailable_error(Feature::DeviceSetupApi))
    }
    fn security(&self) -> SecurityReading {
        SecurityReading::unreadable(&unavailable_error(Feature::DeviceSetupApi))
    }
    fn store(&self) -> Result<Vec<StorePackage>, String> {
        Err(unavailable_error(Feature::DeviceSetupApi))
    }
    fn filters(&self) -> FilterRead {
        FilterRead::Failed(unavailable_error(Feature::DeviceSetupApi))
    }
    fn signature(&self, path: &str) -> super::signature::Signature {
        super::signature::check(path)
    }
}
