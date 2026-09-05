//! The on-screen video preview: an `iced::widget::shader` `Primitive` that
//! draws a decoded `offcut_engine::Frame` as a textured quad, inside the
//! *same* wgpu render pass iced's own UI draws with — the "one
//! shared render pass, not a compositing step" claim, now actually
//! implemented, not just architected.
//!
//! **Design note on where the upload happens:** iced owns exactly one
//! `wgpu::Device`/`wgpu::Queue` pair (created inside its own renderer,
//! `iced_wgpu::Engine`) and does not expose it to application code ahead
//! of time — `Primitive::prepare` is the *only* place a custom primitive
//! ever sees that device. That means the frame's raw CPU bytes must be
//! carried on `VideoPrimitive` (not a `wgpu::Texture` created against some
//! *other* device via `offcut-render::RenderContext`, which would be a
//! silent cross-device resource bug) and uploaded lazily inside
//! `prepare`, using `upload.rs`'s already-tested `upload_frame`, the first
//! time iced hands this module a real device. Every draw after the first
//! for the same frame just rebinds the cached texture — no re-upload,
//! matching the "UI thread never touches pixels" framing (the
//! upload happens once per *decoded frame*, not once per redraw).
//!
//! **Crop and Adjust (the design rule/§4.6) live here too**, as of the
//! session that made them real. The fragment shader is
//! `effects.rs`'s `EFFECTS_SHADER` — the *same* WGSL source
//! `offscreen.rs` uses to bake an exported frame — and the crop rect,
//! straighten angle, and five adjust values arrive as one small uniform
//! buffer. That shared-source arrangement is what makes §4.5's "preview
//! cost: effectively free" and §8's "preview and export must not drift"
//! true structurally rather than by inspection.
//!
//! This module deliberately depends on `iced_wgpu`/`iced_graphics`'
//! `Primitive`/`Pipeline` traits directly (not through the `iced` facade
//! crate) so `offcut-render` does not have to pull in all of `iced` (fonts,
//! widgets, the application runtime) just to draw a quad — `offcut-ui` is
//! the crate that wires this `Primitive` into an actual `shader::Program`
//! and exposes it as a widget.

use crate::effects::{EFFECTS_SHADER, EffectsUniform};
use crate::upload::upload_frame;
use iced_wgpu::wgpu;
use std::sync::{Arc, Mutex};
use offcut_engine::Frame;

/// The `shader::Program` primitive offcut-ui's video widget hands to
/// `iced::widget::Shader`. Carries the decoded frame's raw CPU data (an
/// `Arc<Frame>` so cloning this primitive every redraw, which iced's
/// `shader` widget does via `Program::draw`, is cheap) — see the module
/// doc comment for why this is CPU bytes and not a pre-made
/// `wgpu::Texture`.
#[derive(Debug, Clone)]
pub struct VideoPrimitive {
    pub frame: Option<Arc<Frame>>,
    /// The selected clip's crop + adjust state, already translated for
    /// the shader. Changing this is a `write_buffer` of 48 bytes, not a
    /// pipeline rebuild — which is why dragging an Adjust slider previews
    /// live.
    pub effects: EffectsUniform,
}

impl Default for VideoPrimitive {
    fn default() -> Self {
        Self { frame: None, effects: EffectsUniform::identity(1.0) }
    }
}

/// GPU state shared across every `VideoPrimitive` draw call: the render
/// pipeline, sampler, effects uniform buffer, and the current frame's
/// texture + bind group. A new texture is only allocated when the frame's
/// resolution actually changes (`upload.rs`'s `write_into_texture` re-uses
/// the existing allocation for same-size frames, matching that module's
/// documented steady-state playback path).
pub struct VideoPipeline {
    pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
    effects_buffer: wgpu::Buffer,
    current: Mutex<Option<CurrentFrame>>,
}

struct CurrentFrame {
    pts_nanos: u64,
    width: u32,
    height: u32,
    texture: wgpu::Texture,
    bind_group: wgpu::BindGroup,
}

/// The bind group layout both this module and `offscreen.rs` use:
/// texture, sampler, effects uniform. Shared so the two pipelines cannot
/// drift into incompatible binding numbers while claiming to run the same
/// shader.
pub(crate) fn effects_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("offcut-effects-bind-group-layout"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                // VERTEX too: the vertex stage reads `fit_scale` to
                // shrink the quad so the picture keeps its proportions.
                // Fragment-only visibility here is a validation error
                // that names the binding, not the cause.
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    })
}

/// Build the render pipeline for `effects.wgsl` against a target format.
/// Shared with `offscreen.rs` for the same reason as the layout above.
pub(crate) fn effects_render_pipeline(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("offcut-effects-shader"),
        source: wgpu::ShaderSource::Wgsl(EFFECTS_SHADER.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("offcut-effects-pipeline-layout"),
        bind_group_layouts: &[layout],
        push_constant_ranges: &[],
    });

    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("offcut-effects-pipeline"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleStrip, ..Default::default() },
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview: None,
        cache: None,
    })
}

pub(crate) fn effects_sampler(device: &wgpu::Device) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("offcut-effects-sampler"),
        // Clamp, not repeat: a straighten rotation samples past the frame
        // edge, and a repeating wrap would tile the opposite edge of the
        // image into the corner. The shader also explicitly blacks out
        // out-of-range samples, so this is belt-and-braces.
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    })
}

pub(crate) fn effects_uniform_buffer(device: &wgpu::Device) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("offcut-effects-uniform"),
        size: EffectsUniform::SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn effects_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &wgpu::TextureView,
    sampler: &wgpu::Sampler,
    uniform: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("offcut-effects-bind-group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
        ],
    })
}

impl iced_wgpu::primitive::Pipeline for VideoPipeline {
    fn new(device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let bind_group_layout = effects_bind_group_layout(device);
        let pipeline = effects_render_pipeline(device, &bind_group_layout, format, Some(wgpu::BlendState::REPLACE));
        Self {
            pipeline,
            sampler: effects_sampler(device),
            bind_group_layout,
            effects_buffer: effects_uniform_buffer(device),
            current: Mutex::new(None),
        }
    }
}

impl iced_wgpu::primitive::Primitive for VideoPrimitive {
    type Pipeline = VideoPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &iced_wgpu::core::Rectangle,
        _viewport: &iced_wgpu::graphics::Viewport,
    ) {
        // The effects uniform is written every prepare, unconditionally:
        // it is 48 bytes, and tracking "did it change" would cost more
        // (a comparison plus a stored copy) than the write it saves,
        // while adding a way for a slider drag to fail to take effect.
        queue.write_buffer(&pipeline.effects_buffer, 0, self.effects.as_bytes());

        let Some(frame) = &self.frame else { return };
        let mut current = pipeline.current.lock().expect("video pipeline current-frame lock poisoned");

        let same_frame = current
            .as_ref()
            .is_some_and(|c| c.pts_nanos == frame.pts.as_nanos() && c.width == frame.width && c.height == frame.height);
        if same_frame {
            return;
        }

        let can_reuse_texture = current.as_ref().is_some_and(|c| c.width == frame.width && c.height == frame.height);

        if can_reuse_texture {
            let c = current.as_mut().expect("checked Some above");
            if crate::upload::write_into_texture(queue, &c.texture, frame).is_ok() {
                c.pts_nanos = frame.pts.as_nanos();
                return;
            }
            // Fall through to full re-upload if the write somehow failed
            // (e.g. a malformed frame slipped through) -- rebuilding from
            // scratch below still gives a well-defined, non-panicking
            // outcome via upload_frame's own MalformedFrame guard.
        }

        let Ok(texture) = upload_frame(device, queue, frame) else {
            // A malformed frame is logged and dropped, not displayed --
            // the previous good frame (if any) simply stays on screen one
            // extra tick, which is the same "never panic on a decode
            // hiccup" posture offcut-engine's own frame guard uses.
            return;
        };
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = effects_bind_group(
            device,
            &pipeline.bind_group_layout,
            &view,
            &pipeline.sampler,
            &pipeline.effects_buffer,
        );
        *current = Some(CurrentFrame {
            pts_nanos: frame.pts.as_nanos(),
            width: frame.width,
            height: frame.height,
            texture,
            bind_group,
        });
    }

    fn draw(&self, pipeline: &Self::Pipeline, render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        let current = pipeline.current.lock().expect("video pipeline current-frame lock poisoned");
        let Some(current) = &*current else {
            // No frame yet: let the caller's clear color (the letterbox
            // background, per the design system) show through -- draw nothing.
            return true;
        };

        render_pass.set_pipeline(&pipeline.pipeline);
        render_pass.set_bind_group(0, &current.bind_group, &[]);
        render_pass.draw(0..4, 0..1);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::RenderContext;
    use offcut_engine::PixelFormat;
    use offcut_model::Time;

    fn test_frame(width: u32, height: u32, pts_nanos: u64) -> Frame {
        let stride = width * 4;
        let mut data = vec![0u8; (stride * height) as usize];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        Frame { width, height, stride, format: PixelFormat::Rgba8, data, pts: Time::from_nanos(pts_nanos) }
    }

    fn offscreen_target(device: &wgpu::Device, width: u32, height: u32) -> (wgpu::Texture, wgpu::TextureView) {
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("test-render-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());
        (target, view)
    }

    fn draw_once(ctx: &RenderContext, pipeline: &mut VideoPipeline, primitive: &VideoPrimitive, size: u32) -> bool {
        let (_, target_view) = offscreen_target(&ctx.device, size, size);
        let viewport = iced_wgpu::graphics::Viewport::with_physical_size(iced_wgpu::core::Size::new(size, size), 1.0);
        let bounds = iced_wgpu::core::Rectangle { x: 0.0, y: 0.0, width: size as f32, height: size as f32 };

        <VideoPrimitive as iced_wgpu::primitive::Primitive>::prepare(primitive, pipeline, &ctx.device, &ctx.queue, &bounds, &viewport);

        let mut encoder = ctx.device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let drew;
        {
            let mut render_pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("test-render-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target_view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            drew = <VideoPrimitive as iced_wgpu::primitive::Primitive>::draw(primitive, pipeline, &mut render_pass);
        }
        ctx.queue.submit(Some(encoder.finish()));
        ctx.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).expect("poll failed");
        drew
    }

    fn new_pipeline(ctx: &RenderContext) -> VideoPipeline {
        <VideoPipeline as iced_wgpu::primitive::Pipeline>::new(&ctx.device, &ctx.queue, wgpu::TextureFormat::Rgba8UnormSrgb)
    }

    /// The full path this module exists for: decode-shaped bytes -> this
    /// module's `prepare` uploading them via `upload.rs` using the
    /// device/queue iced itself hands in -> a real draw call against a
    /// real (GL/llvmpipe, in this sandbox) adapter, into an offscreen
    /// render target, with no panic and no wgpu validation error.
    #[test]
    fn video_pipeline_draws_a_frame_without_gpu_validation_errors() {
        pollster::block_on(async {
            let ctx = RenderContext::new_headless().await.expect("no adapter");
            let mut gpu_pipeline = new_pipeline(&ctx);
            let primitive = VideoPrimitive { frame: Some(Arc::new(test_frame(64, 64, 0))), ..Default::default() };
            let drew = draw_once(&ctx, &mut gpu_pipeline, &primitive, 64);
            assert!(drew, "VideoPrimitive::draw should report it drew within the render pass");
        });
    }

    #[test]
    fn video_primitive_with_no_frame_draws_nothing_but_reports_handled() {
        pollster::block_on(async {
            let ctx = RenderContext::new_headless().await.expect("no adapter");
            let mut gpu_pipeline = new_pipeline(&ctx);
            let primitive = VideoPrimitive::default();
            let handled = draw_once(&ctx, &mut gpu_pipeline, &primitive, 8);
            assert!(handled);
        });
    }

    /// Exercises the steady-state playback path: a second frame at the
    /// SAME resolution but a different `pts` should reuse the existing
    /// texture allocation via `write_into_texture` rather than allocate a
    /// new one.
    #[test]
    fn second_frame_at_same_resolution_reuses_pipeline_state_without_error() {
        pollster::block_on(async {
            let ctx = RenderContext::new_headless().await.expect("no adapter");
            let mut gpu_pipeline = new_pipeline(&ctx);

            let frame1 = VideoPrimitive { frame: Some(Arc::new(test_frame(32, 32, 0))), ..Default::default() };
            assert!(draw_once(&ctx, &mut gpu_pipeline, &frame1, 32));

            let frame2 = VideoPrimitive { frame: Some(Arc::new(test_frame(32, 32, 33_366_666))), ..Default::default() };
            assert!(draw_once(&ctx, &mut gpu_pipeline, &frame2, 32));
        });
    }

    /// A resolution change (e.g. switching source clips) must not panic
    /// -- it should fall back to a fresh `upload_frame` allocation.
    #[test]
    fn resolution_change_between_frames_does_not_panic() {
        pollster::block_on(async {
            let ctx = RenderContext::new_headless().await.expect("no adapter");
            let mut gpu_pipeline = new_pipeline(&ctx);

            let small = VideoPrimitive { frame: Some(Arc::new(test_frame(16, 16, 0))), ..Default::default() };
            assert!(draw_once(&ctx, &mut gpu_pipeline, &small, 16));

            let large = VideoPrimitive { frame: Some(Arc::new(test_frame(64, 64, 1_000_000))), ..Default::default() };
            assert!(draw_once(&ctx, &mut gpu_pipeline, &large, 64));
        });
    }

    /// The new surface: a non-identity crop/adjust uniform must compile,
    /// bind, and draw against the shared effects shader. This is the
    /// on-screen half of the "same shader for preview and export" claim;
    /// `offscreen.rs` proves the pixels actually change.
    #[test]
    fn a_non_identity_effects_uniform_draws_without_validation_errors() {
        pollster::block_on(async {
            let ctx = RenderContext::new_headless().await.expect("no adapter");
            let mut gpu_pipeline = new_pipeline(&ctx);

            let mut crop = offcut_model::CropTransform::identity();
            crop.apply_aspect(offcut_model::AspectPreset::Square, 1.0);
            crop.set_straighten_deg(12.0);
            let adjust = offcut_model::AdjustSettings::mockup_reference();

            let primitive = VideoPrimitive {
                frame: Some(Arc::new(test_frame(32, 32, 0))),
                effects: EffectsUniform::new(&crop, &adjust, 1.0),
            };
            assert!(draw_once(&ctx, &mut gpu_pipeline, &primitive, 32));
        });
    }
}
