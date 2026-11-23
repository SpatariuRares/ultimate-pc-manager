//! Linux: IGCL is a Windows library, so there is none.

/// The library: not available on this system.
pub fn library() -> Result<Box<dyn crate::sensors::vendor_gpu::Library>, String> {
    Err("IGCL is a Windows library".to_string())
}
