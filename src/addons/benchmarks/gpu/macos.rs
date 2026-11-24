//! macOS: wgpu runs on Metal.

/// The backend the GPU test uses.
pub const BACKENDS: wgpu::Backends = wgpu::Backends::METAL;

/// The GPU test's error when no card can be tested.
pub const NO_GPU: &str = "A hardware Metal GPU is unavailable on this system.";

/// The GPU view's hardware line when no card is listed.
pub const NO_CARD: &str = "No Metal GPU found";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_missing_gpu_messages_name_this_systems_graphics_api() {
        assert!(NO_GPU.contains("Metal") && NO_CARD.contains("Metal"));
    }
}
