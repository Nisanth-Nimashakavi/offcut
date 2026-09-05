//! offcut-render: wgpu texture bridge, DMABUF import, YUV->RGB shader
//! (the crate layout).
//!
//! **2026-08-28 correction (see `context.rs`'s module doc comment for the
//! full account):** the previous version of this comment said "this
//! development sandbox has no `/dev/dri` GPU device node" and treated
//! that as blocking every GPU-dependent test. That premise is still true
//! but the conclusion was wrong — a real, working wgpu adapter
//! (`llvmpipe`, Mesa's software rasterizer, reached through EGL's
//! surfaceless platform, which does not need `/dev/dri`) exists in this
//! sandbox and every test in this crate now runs against it for real, not
//! `#[ignore]`d. `context.rs`, `upload.rs`, and `preview.rs` all have
//! genuine, passing, non-mocked GPU tests as of this session.
//!
//! `preview.rs` is the new piece: an `iced_wgpu`-compatible `Primitive`
//! that draws a decoded frame's texture as a quad inside iced's own
//! render pass — the actual implementation of the "one shared
//! render pass, not a compositing step" claim, wired up and tested (draw
//! call issued against a real adapter with no wgpu validation error) but
//! not yet plumbed into a live `offcut-engine` pipeline inside a running
//! `offcut-app` window —
//! exactly where that boundary currently sits.
//!
//! DMABUF zero-copy import is unaffected by the above: it is
//! `VK_EXT_external_memory_dma_buf`-specific (Vulkan-only), so it still
//! cannot be authored or verified against a real DMABUF in this sandbox.

pub mod context;
pub mod effects;
pub mod error;
pub mod offscreen;
pub mod preview;
pub mod upload;

pub use context::RenderContext;
pub use effects::{EFFECTS_SHADER, EffectsUniform};
pub use offscreen::EffectsRenderer;
pub use error::RenderError;
pub use preview::{VideoPipeline, VideoPrimitive};
