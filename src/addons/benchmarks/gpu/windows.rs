//! Windows: wgpu runs on Direct3D 12.

/// The backend the GPU test uses.
pub const BACKENDS: wgpu::Backends = wgpu::Backends::DX12;

/// The GPU test's error when no card can be tested.
pub const NO_GPU: &str = "A hardware GPU with Direct3D 12 is unavailable on this system.";

/// The GPU view's hardware line when no card is listed.
pub const NO_CARD: &str = "No Direct3D 12 GPU found";
