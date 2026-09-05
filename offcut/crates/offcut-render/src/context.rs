//! A minimal, headless-constructible wgpu context: instance, adapter,
//! device, queue. Deliberately does *not* require a `wgpu::Surface` to
//! construct — the architecture shares one wgpu device between
//! UI and video preview, but a device/queue pair on its own is also
//! exactly what's needed to validate the texture-upload path (`upload.rs`)
//! against a real (or, on a machine with one, headless/offscreen) adapter,
//! independent of ever opening a window.
//!
//! # What is and is not verified in this codebase
//!
//! **Corrected 2026-08-28 — this sandbox is NOT GPU-less.** The previous
//! version of this comment said `wgpu::Instance::enumerate_adapters` finds
//! zero adapters here "because the sandbox has no `/dev/dri` device node."
//! That premise (no `/dev/dri`) is still true, but the conclusion was
//! wrong: it was checked only against `Backends::VULKAN`. A direct probe
//! (`WGPU_BACKEND=gl`, or simply requesting `Backends::all()` /
//! `Backends::from_env().unwrap_or_default()`) finds **one real, working
//! adapter**: `llvmpipe` (Mesa's software rasterizer) via EGL's
//! `EGL_PLATFORM_SURFACELESS_MESA`, which does not need `/dev/dri` at all
//! — it's a pure-CPU GL context. Verified end-to-end in this session,
//! outside this crate first (a throwaway probe binary), then here:
//! adapter enumeration, device/queue creation, `write_texture`, a real
//! render pass, `copy_texture_to_buffer`, and a mapped read-back all
//! succeed and round-trip correct pixel data. A live Wayland window
//! (`winit` + `wgpu::Surface`) also presents real frames against this
//! backend on this machine's actual Hyprland compositor.
//!
//! Consequently `RenderContext::new_headless` below requests
//! `Backends::all()` (or `WGPU_BACKEND` if set), not `Backends::VULKAN`
//! only: on the real target machine (the design rule, real Intel Arc + Vulkan
//! ICD) this resolves to the Vulkan adapter exactly as before; in this
//! sandbox it falls back to the GL/llvmpipe adapter automatically, the
//! same "software path is the safe, always-available default; hardware is
//! a bonus" philosophy the design rule already applies to decode/encode. The
//! tests below are **no longer `#[ignore]`d** — they run for real, in this
//! environment, on every `cargo test`.
//!
//! DMABUF zero-copy import is unaffected by this correction: it is
//! `VK_EXT_external_memory_dma_buf`-specific (Vulkan-only), so it still
//! cannot be authored or verified against a real DMABUF in this sandbox —
//! llvmpipe has no DMABUF-backed decode path feeding it. That gap is
//! tracked in the design rule Phase 0 as before.

use crate::error::RenderError;

pub struct RenderContext {
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

impl RenderContext {
    /// Headless construction: no compatible surface required. Backend
    /// selection is `Backends::from_env()` (honors `WGPU_BACKEND=gl`,
    /// `vulkan`, etc. for diagnostics) falling back to `Backends::all()`,
    /// which lets wgpu pick the best available adapter on whatever machine
    /// this runs on — Vulkan on a machine with a GPU, GL/llvmpipe
    /// without one. A windowed context additionally passes
    /// `compatible_surface` when requesting the adapter.
    pub async fn new_headless() -> Result<Self, RenderError> {
        let backends = wgpu::Backends::from_env().unwrap_or(wgpu::Backends::all());
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor { backends, ..Default::default() });

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                compatible_surface: None,
                force_fallback_adapter: false,
            })
            .await
            .map_err(|_| RenderError::NoAdapter)?;

        let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor::default()).await?;

        Ok(Self { instance, adapter, device, queue })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No longer `#[ignore]`d — see the module doc comment: this sandbox
    /// has a real, working GL/llvmpipe wgpu adapter even without
    /// `/dev/dri`. Confirmed to pass in this exact environment.
    #[test]
    fn headless_context_can_be_created() {
        let result = pollster::block_on(RenderContext::new_headless());
        assert!(result.is_ok(), "expected a headless wgpu context: {:?}", result.err());
        let ctx = result.unwrap();
        // Document which adapter we actually got, for the test log.
        eprintln!("RenderContext adapter: {:?}", ctx.adapter.get_info());
    }
}
