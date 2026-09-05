//! Baking crop + adjust into an exportable frame, on the GPU, with the
//! **same shader the preview uses**.
//!
//! The design rule: "Export cost: one extra affine transform per frame in
//! the same encode pass, not an extra encode pass." §8 names the bug this
//! module's design forecloses: "the crop/adjust shader math drifting
//! between preview and export."
//!
//! The mechanism is deliberately boring: `preview.rs` and this module
//! build their pipelines from the *same* `EFFECTS_SHADER` constant, with
//! the same bind group layout (built by the same shared function), fed
//! the same `EffectsUniform` built by the same constructor from the same
//! `Clip`. There is no second copy of the crop math to drift.
//!
//! # Why this reads back to the CPU
//!
//! The exported frame has to reach `x264enc`, which lives in GStreamer,
//! on the CPU. So the round trip is: decoded CPU frame → texture →
//! shader → texture → CPU bytes → appsrc. The two copies that costs are
//! the honest price of doing the effects on the GPU at all, and they are
//! still cheaper than a CPU-side implementation of a 3×3 blur plus four
//! masked color operations per pixel. When a clip has no effects applied,
//! `bake_frame` skips the GPU entirely (see `EffectsUniform::is_at_rest`)
//! and hands the decoded bytes straight through — which is the common
//! case for a trim-only export and makes it as fast as a plain remux of
//! the decoded stream.

use crate::effects::EffectsUniform;
use crate::error::RenderError;
use crate::preview::{effects_bind_group, effects_bind_group_layout, effects_render_pipeline, effects_sampler, effects_uniform_buffer};
use crate::upload::upload_frame;
use offcut_engine::{Frame, PixelFormat};

/// A reusable offscreen renderer for export. Holds the pipeline and
/// sampler so a whole export pays for shader compilation exactly once,
/// not once per frame.
pub struct EffectsRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    uniform: wgpu::Buffer,
    target: Option<TargetTextures>,
}

struct TargetTextures {
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

/// The output format is `Rgba8Unorm`, **not** `Rgba8UnormSrgb`.
///
/// This matters and is easy to get wrong: an sRGB render target applies
/// a linear→sRGB conversion on write. The decoded frame's bytes are
/// already sRGB-encoded (that is what a video decoder emits), so an sRGB
/// target would encode them a second time and every exported frame would
/// come out visibly washed out compared to the preview — the exact
/// preview/export mismatch this module exists to prevent, arriving
/// through the back door of a texture format rather than through the
/// shader math.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl EffectsRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = effects_bind_group_layout(device);
        // `blend: None` (not `REPLACE`): the export target is opaque and
        // starts cleared to black, so there is nothing to blend against,
        // and disabling blending entirely is the cheaper path.
        let pipeline = effects_render_pipeline(device, &layout, TARGET_FORMAT, None);
        Self {
            pipeline,
            layout,
            sampler: effects_sampler(device),
            uniform: effects_uniform_buffer(device),
            target: None,
        }
    }

    fn ensure_target(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        let matches = self.target.as_ref().is_some_and(|t| t.width == width && t.height == height);
        if matches {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offcut-export-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.target = Some(TargetTextures { width, height, texture, view });
    }

    /// Render `frame` through the effects shader and return the result as
    /// a new `Frame` of `out_width × out_height`, tightly packed RGBA.
    ///
    /// When `effects` is at rest **and** the output size matches the
    /// input, this returns the input frame's bytes unchanged without
    /// touching the GPU — see the module doc comment.
    pub fn bake_frame(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame,
        effects: &EffectsUniform,
        out_width: u32,
        out_height: u32,
    ) -> Result<Frame, RenderError> {
        if !frame.is_well_formed() {
            return Err(RenderError::MalformedFrame);
        }
        if effects.is_at_rest() && frame.width == out_width && frame.height == out_height {
            return Ok(tightly_packed(frame));
        }

        self.ensure_target(device, out_width, out_height);
        queue.write_buffer(&self.uniform, 0, effects.as_bytes());

        let source = upload_frame(device, queue, frame)?;
        let source_view = source.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = effects_bind_group(device, &self.layout, &source_view, &self.sampler, &self.uniform);

        let target = self.target.as_ref().expect("ensure_target just ran");
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("offcut-export-encoder"),
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("offcut-export-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..4, 0..1);
        }
        queue.submit(Some(encoder.finish()));

        let data = read_texture_rgba(device, queue, &target.texture, out_width, out_height)?;
        Ok(Frame {
            width: out_width,
            height: out_height,
            stride: out_width * 4,
            format: PixelFormat::Rgba8,
            data,
            pts: frame.pts,
        })
    }
}

/// Repack a frame to a tight stride, copying only if it is not already
/// tight. GStreamer's decoder can hand back a padded stride; `appsrc`
/// downstream wants tight rows.
fn tightly_packed(frame: &Frame) -> Frame {
    let tight = frame.width * 4;
    if frame.stride == tight {
        return frame.clone();
    }
    let mut data = Vec::with_capacity((tight * frame.height) as usize);
    for row in 0..frame.height as usize {
        let start = row * frame.stride as usize;
        data.extend_from_slice(&frame.data[start..start + tight as usize]);
    }
    Frame { width: frame.width, height: frame.height, stride: tight, format: frame.format.clone(), data, pts: frame.pts }
}

/// Copy a render target back to CPU memory as tightly packed RGBA.
///
/// `copy_texture_to_buffer` requires each row to be a multiple of
/// `COPY_BYTES_PER_ROW_ALIGNMENT` (256), so the staging buffer is padded
/// and the padding is stripped row by row here. Skipping that strip is
/// the classic bug that produces a diagonally sheared image for any width
/// that is not a multiple of 64 pixels.
fn read_texture_rgba(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, RenderError> {
    let unpadded = width * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offcut-export-readback"),
        size: (padded as u64) * (height as u64),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &staging,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit(Some(encoder.finish()));

    let slice = staging.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
        .map_err(|_| RenderError::MalformedFrame)?;
    rx.recv()
        .map_err(|_| RenderError::MalformedFrame)?
        .map_err(|_| RenderError::MalformedFrame)?;

    let mapped = slice.get_mapped_range();
    let mut out = Vec::with_capacity((unpadded * height) as usize);
    for row in 0..height as usize {
        let start = row * padded as usize;
        out.extend_from_slice(&mapped[start..start + unpadded as usize]);
    }
    drop(mapped);
    staging.unmap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RenderContext;
    use offcut_model::{AdjustSettings, AdjustValue, AspectPreset, CropTransform, Time};

    /// A frame of one flat, recognizable color, so an effect's presence or
    /// absence is a simple value comparison rather than an image diff.
    fn flat_frame(width: u32, height: u32, rgba: [u8; 4]) -> Frame {
        let stride = width * 4;
        let mut data = Vec::with_capacity((stride * height) as usize);
        for _ in 0..(width * height) {
            data.extend_from_slice(&rgba);
        }
        Frame { width, height, stride, format: PixelFormat::Rgba8, data, pts: Time::ZERO }
    }

    fn pixel(frame: &Frame, x: u32, y: u32) -> [u8; 4] {
        let i = (y * frame.stride + x * 4) as usize;
        [frame.data[i], frame.data[i + 1], frame.data[i + 2], frame.data[i + 3]]
    }

    fn renderer() -> (RenderContext, EffectsRenderer) {
        let ctx = pollster::block_on(RenderContext::new_headless()).expect("no adapter");
        let renderer = EffectsRenderer::new(&ctx.device);
        (ctx, renderer)
    }

    /// The fast path: no effects and no resize means no GPU work at all,
    /// and byte-identical output. A trim-only export depends on this.
    #[test]
    fn an_at_rest_effect_passes_the_frame_through_unchanged() {
        let (ctx, mut r) = renderer();
        let frame = flat_frame(16, 16, [120, 60, 30, 255]);
        let out = r
            .bake_frame(&ctx.device, &ctx.queue, &frame, &EffectsUniform::identity(1.0), 16, 16)
            .expect("bake failed");
        assert_eq!(out.data, frame.data, "at-rest bake must not alter a single byte");
    }

    #[test]
    fn a_padded_stride_is_repacked_tight_on_the_pass_through_path() {
        let (ctx, mut r) = renderer();
        // Stride deliberately larger than width*4, as a decoder may emit.
        let width = 6u32;
        let height = 4u32;
        let stride = width * 4 + 8;
        let mut data = vec![0u8; (stride * height) as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 253) as u8;
        }
        let frame = Frame { width, height, stride, format: PixelFormat::Rgba8, data, pts: Time::ZERO };
        assert!(frame.is_well_formed());

        let out = r
            .bake_frame(&ctx.device, &ctx.queue, &frame, &EffectsUniform::identity(1.0), width, height)
            .expect("bake failed");
        assert_eq!(out.stride, width * 4, "output must be tightly packed for appsrc");
        assert_eq!(out.data.len(), (width * 4 * height) as usize);
    }

    /// The core claim: the shader actually changes pixels. A full-strength
    /// vignette must darken the corners while leaving the center alone.
    #[test]
    fn vignette_darkens_the_corners_and_spares_the_center() {
        let (ctx, mut r) = renderer();
        let frame = flat_frame(64, 64, [200, 200, 200, 255]);
        let adjust = AdjustSettings { vignette: AdjustValue::new(100), ..Default::default() };
        let effects = EffectsUniform::new(&CropTransform::identity(), &adjust, 1.0);

        let out = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 64, 64).expect("bake failed");

        let center = pixel(&out, 32, 32)[0];
        let corner = pixel(&out, 1, 1)[0];
        assert!(center > 180, "the center should be nearly untouched, got {center}");
        assert!(corner < center / 2, "the corner should be strongly darkened: corner {corner} vs center {center}");
    }


    /// Bars appear only when asked for. A cropping clip fills the frame
    /// edge to edge.
    #[test]
    fn cropping_fills_the_frame_with_no_bars() {
        let (ctx, mut r) = renderer();
        let frame = flat_frame(64, 64, [255, 255, 255, 255]);

        let mut crop = CropTransform::identity();
        crop.apply_aspect(offcut_model::AspectPreset::Portrait916, 1.0);
        let effects = EffectsUniform::new(&crop, &AdjustSettings::default(), 1.0);

        let out = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 64, 64).expect("bake failed");
        assert!(pixel(&out, 32, 1)[0] > 200, "cropping must not introduce a top bar");
        assert!(pixel(&out, 32, 62)[0] > 200, "cropping must not introduce a bottom bar");
    }

    /// A circle must render as a circle.
    ///
    /// The quad used to span the whole widget regardless of shape, so a
    /// 16:9 video in a taller stage was **stretched** to fill it —
    /// `videotestsrc`'s ball came out as a tall ellipse and faces were
    /// visibly wrong. This measures the drawn width and height of a disc
    /// and requires them to match.
    #[test]
    fn a_circle_stays_circular_in_a_differently_shaped_viewport() {
        let (ctx, mut r) = renderer();

        // A white disc on black, in a 2:1 source frame.
        let (sw, sh) = (128u32, 64u32);
        let mut data = Vec::with_capacity((sw * sh * 4) as usize);
        for y in 0..sh {
            for x in 0..sw {
                // A true circle in SOURCE pixels. Scaling dy here (as a
                // first attempt did) draws an ellipse in the source, so
                // a faithful renderer reproduces the ellipse and the
                // test measures its own mistake rather than the code's.
                let dx = x as f32 - sw as f32 / 2.0;
                let dy = y as f32 - sh as f32 / 2.0;
                let inside = (dx * dx + dy * dy).sqrt() < 24.0;
                let v = if inside { 255 } else { 0 };
                data.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let frame = Frame {
            width: sw,
            height: sh,
            stride: sw * 4,
            format: PixelFormat::Rgba8,
            data,
            pts: Time::ZERO,
        };

        // Render into a SQUARE target -- a different shape from the
        // source, which is precisely when stretching used to happen.
        let mut effects = EffectsUniform::new(
            &CropTransform::identity(),
            &AdjustSettings::default(),
            sw as f32 / sh as f32,
        );
        effects.fit_to_viewport(sw as f32 / sh as f32, (128.0, 128.0));

        let out = r
            .bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 128, 128)
            .expect("bake failed");

        // Measure the disc through its centre on both axes.
        //
        // The source is 2:1 and the target square, so a correctly
        // *fitted* picture occupies the middle half of the target and
        // the disc stays round. A stretched one fills the square and the
        // disc comes out twice as wide as it is tall -- which is exactly
        // what this measured before the fix: 47x23, ratio 2.04.
        let lit = |x: u32, y: u32| pixel(&out, x, y)[0] > 128;
        let width: u32 = (0..128).filter(|&x| lit(x, 64)).count() as u32;
        let height: u32 = (0..128).filter(|&y| lit(64, y)).count() as u32;

        assert!(width > 4 && height > 4, "the disc did not render ({width}x{height})");
        let ratio = width as f32 / height as f32;
        assert!(
            (ratio - 1.0).abs() < 0.15,
            "a circle rendered {width}x{height} (ratio {ratio:.2}) in a square \
             viewport — the picture is being stretched, not fitted"
        );
    }

    /// Straighten rotates the sample grid, so the output corners fall
    /// outside the source and the shader blacks them out. This is the
    /// documented behavior in `effects.wgsl` — assert it, so a future
    /// "fix" to edge-clamping has to argue with a failing test.
    #[test]
    fn straighten_blacks_out_corners_that_fall_outside_the_source() {
        let (ctx, mut r) = renderer();
        let frame = flat_frame(64, 64, [255, 255, 255, 255]);
        let mut crop = CropTransform::identity();
        crop.set_straighten_deg(45.0);
        let effects = EffectsUniform::new(&crop, &AdjustSettings::default(), 1.0);

        let out = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 64, 64).expect("bake failed");
        assert_eq!(pixel(&out, 1, 1)[0], 0, "a 45° rotation must leave the corner outside the frame");
        assert!(pixel(&out, 32, 32)[0] > 200, "the center must still show the image");
    }

    #[test]
    fn a_crop_rect_selects_a_sub_region_of_the_source() {
        let (ctx, mut r) = renderer();
        // Left half red, right half blue.
        let (w, h) = (64u32, 32u32);
        let mut data = Vec::new();
        for _ in 0..h {
            for x in 0..w {
                if x < w / 2 {
                    data.extend_from_slice(&[255, 0, 0, 255]);
                } else {
                    data.extend_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        let frame = Frame { width: w, height: h, stride: w * 4, format: PixelFormat::Rgba8, data, pts: Time::ZERO };

        // Crop to the right half only.
        let mut crop = CropTransform::identity();
        crop.rect = offcut_model::NormalizedRect::new(0.5, 0.0, 0.5, 1.0);
        let effects = EffectsUniform::new(&crop, &AdjustSettings::default(), 2.0);

        let out = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, w, h).expect("bake failed");
        // Every output pixel should now be blue, since only the right
        // (blue) half was sampled.
        let left = pixel(&out, 4, 16);
        let right = pixel(&out, w - 4, 16);
        assert!(left[2] > 200 && left[0] < 60, "cropped output should be blue at the left edge, got {left:?}");
        assert!(right[2] > 200 && right[0] < 60, "cropped output should be blue at the right edge, got {right:?}");
    }

    #[test]
    fn output_can_be_a_different_resolution_than_the_source() {
        let (ctx, mut r) = renderer();
        let frame = flat_frame(64, 64, [100, 150, 200, 255]);
        let out = r
            .bake_frame(&ctx.device, &ctx.queue, &frame, &EffectsUniform::identity(1.0), 32, 32)
            .expect("bake failed");
        assert_eq!((out.width, out.height), (32, 32));
        assert_eq!(out.data.len(), 32 * 32 * 4);
        // A downscale of a flat color is still that color.
        let p = pixel(&out, 16, 16);
        assert!((p[0] as i32 - 100).abs() < 8 && (p[2] as i32 - 200).abs() < 8, "got {p:?}");
    }

    /// A non-multiple-of-64 width exercises the readback row-padding
    /// strip. If that strip were wrong, this image would come back
    /// sheared rather than flat.
    #[test]
    fn a_width_needing_row_padding_reads_back_without_shearing() {
        let (ctx, mut r) = renderer();
        let (w, h) = (37u32, 11u32);
        let frame = flat_frame(w, h, [10, 220, 60, 255]);
        let adjust = AdjustSettings { vignette: AdjustValue::new(1), ..Default::default() };
        let effects = EffectsUniform::new(&CropTransform::identity(), &adjust, w as f32 / h as f32);

        let out = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, w, h).expect("bake failed");
        assert_eq!(out.data.len(), (w * h * 4) as usize);
        // Every pixel started identical; after a near-zero vignette they
        // must all still be close to the source color. A shear would put
        // black (from the padding) into the row tails.
        for y in 0..h {
            let p = pixel(&out, w - 1, y);
            assert!(p[1] > 120, "row {y} tail was {p:?} — readback padding was not stripped");
        }
    }

    #[test]
    fn a_malformed_frame_is_rejected_rather_than_uploaded() {
        let (ctx, mut r) = renderer();
        let bad = Frame {
            width: 8,
            height: 8,
            stride: 4, // far too small
            format: PixelFormat::Rgba8,
            data: vec![0; 32],
            pts: Time::ZERO,
        };
        let result = r.bake_frame(&ctx.device, &ctx.queue, &bad, &EffectsUniform::identity(1.0), 8, 8);
        assert!(matches!(result, Err(RenderError::MalformedFrame)));
    }

    /// The renderer is reused across an export's whole frame sequence;
    /// baking many frames must not leak state or drift.
    #[test]
    fn repeated_bakes_reuse_the_target_and_stay_deterministic() {
        let (ctx, mut r) = renderer();
        let mut crop = CropTransform::identity();
        crop.apply_aspect(AspectPreset::Square, 16.0 / 9.0);
        let effects = EffectsUniform::new(&crop, &AdjustSettings::mockup_reference(), 1.0);

        let frame = flat_frame(32, 32, [90, 140, 190, 255]);
        let first = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 32, 32).expect("bake 1");
        for _ in 0..5 {
            let again = r.bake_frame(&ctx.device, &ctx.queue, &frame, &effects, 32, 32).expect("bake n");
            assert_eq!(again.data, first.data, "the same input must bake to the same output every time");
        }
    }
}
