//! Linux: wgpu runs on Vulkan, with hardware cards only.

/// The backend the GPU test uses.
pub const BACKENDS: wgpu::Backends = wgpu::Backends::VULKAN;

/// The GPU test's error when no card can be tested.
pub const NO_GPU: &str = "No hardware Vulkan GPU was found. Install your distribution's Vulkan driver for your video card.";

/// The GPU view's hardware line when no card is listed.
pub const NO_CARD: &str = "No Vulkan GPU found: install your distribution's Vulkan driver";
