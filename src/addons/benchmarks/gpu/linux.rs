//! Linux: no graphics card is listed and none can be tested yet.

use super::{Adapter, Cancel, Gpu, NO_GPU};

/// The hardware video cards: none are listed yet.
pub fn adapters() -> Vec<Adapter> {
    Vec::new()
}

/// No device can be opened yet.
pub fn open(_adapter: &Adapter, _cancel: Cancel) -> Result<Box<dyn Gpu>, String> {
    Err(NO_GPU.into())
}
