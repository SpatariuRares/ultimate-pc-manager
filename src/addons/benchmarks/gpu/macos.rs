//! macOS: wgpu runs on Metal.

/// The backend the GPU test uses.
pub const BACKENDS: wgpu::Backends = wgpu::Backends::METAL;

/// The GPU test's error when no card can be tested.
pub const NO_GPU: &str = "A hardware Metal GPU is unavailable on this system.";

/// The GPU view's hardware line when no card is listed.
pub const NO_CARD: &str = "No Metal GPU found";
