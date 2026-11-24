//! Windows: wgpu runs on Direct3D 12.

/// The backend the GPU test uses.
pub const BACKENDS: wgpu::Backends = wgpu::Backends::DX12;

/// The GPU test's error when no card can be tested.
pub const NO_GPU: &str = "A hardware GPU with Direct3D 12 is unavailable on this system.";

/// The GPU view's hardware line when no card is listed.
pub const NO_CARD: &str = "No Direct3D 12 GPU found";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_missing_gpu_messages_name_this_systems_graphics_api() {
        assert!(NO_GPU.contains("Direct3D 12") && NO_CARD.contains("Direct3D 12"));
    }
}
