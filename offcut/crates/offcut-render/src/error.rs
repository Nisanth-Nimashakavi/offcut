use thiserror::Error;

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("no wgpu adapter available (no GPU device, or backend not supported here)")]
    NoAdapter,

    #[error("wgpu device request failed: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),

    #[error("frame pixel format {0:?} has no texture upload mapping yet")]
    UnsupportedFormat(offcut_engine::PixelFormat),

    #[error("frame failed its own well-formedness check (stride/data length mismatch) before upload")]
    MalformedFrame,
}
