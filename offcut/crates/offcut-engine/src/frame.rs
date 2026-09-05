//! A decoded video frame, handed from the engine thread toward the render
//! path.
//!
//! the "one rule that keeps this fast": a decoded frame never
//! crosses the process boundary as bytes on the happy (DMABUF) path — it's
//! a texture handle. `Frame` as defined here is deliberately the *other*
//! path: owned CPU-side bytes. This is the honest state of this crate
//! today (see the crate-level doc comment in `lib.rs`): this sandbox has no
//! GPU device, so the DMABUF-import path described in the design rule cannot
//! be implemented *or verified* here. `Frame` is what `gldownload`/software
//! decode already produces, and it is exactly the fallback path the design rule
//! says must exist regardless — "If DMABUF import fails... we degrade to
//! `gldownload` → staging buffer... That degradation is the only place a
//! per-frame `memcpy` is allowed to exist." This type *is* that staging
//! buffer's shape, built and tested now so offcut-render has a real,
//! working target to bind a texture from once it exists.

use offcut_model::Time;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PixelFormat {
    Rgba8,
    // Extend as real-file decode needs more formats (I420, NV12, etc.);
    // deliberately not speculatively listing formats this crate has not
    // yet pulled a single real buffer in.
}

/// One decoded frame, owned CPU-side bytes, tightly packed per the plan's
/// stride handling: `data.len() == stride * height`, and `stride >= width *
/// bytes_per_pixel(format)`. GStreamer's own stride (which can exceed the
/// tight `width * bpp` for alignment reasons) is preserved here rather than
/// re-packed, so no extra copy happens converting one packing to another —
/// consistent with the "avoid every copy that isn't load-bearing."
#[derive(Clone, Debug)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: PixelFormat,
    pub data: Vec<u8>,
    /// Presentation timestamp in SOURCE time, same unit as
    /// `offcut_model::Time` (nanoseconds) — chosen deliberately so a caller
    /// comparing this to a `Clip`'s `in_point`/`out_point` never needs a
    /// unit conversion at the boundary (the design rule: "Time is rational, not
    /// float... Float seconds anywhere in the edit model is how
    /// frame-accurate trims turn into off-by-one frames").
    pub pts: Time,
}

impl Frame {
    pub fn bytes_per_pixel(format: &PixelFormat) -> u32 {
        match format {
            PixelFormat::Rgba8 => 4,
        }
    }

    /// True iff `data`'s length is consistent with `stride * height` — the
    /// invariant every `Frame` returned by this crate upholds. Exists so
    /// tests (and later, offcut-render) can assert it rather than trust
    /// construction sites blindly, the same pattern as
    /// `NormalizedRect::is_valid` in offcut-model.
    pub fn is_well_formed(&self) -> bool {
        let min_stride = self.width * Self::bytes_per_pixel(&self.format);
        self.stride >= min_stride && self.data.len() as u64 == (self.stride as u64) * (self.height as u64)
    }

    /// True iff at least one byte in the buffer is non-zero. A cheap,
    /// intentionally crude "this is not an uninitialized/black buffer"
    /// check — the same one the throwaway probe used to prove real pixel
    /// data crossed the appsink boundary, promoted here so every real
    /// pipeline test in this crate uses the identical check.
    pub fn has_non_zero_data(&self) -> bool {
        self.data.iter().any(|&b| b != 0)
    }
}
