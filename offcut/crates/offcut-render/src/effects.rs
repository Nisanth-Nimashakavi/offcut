//! The GPU-side representation of `CropTransform` + `AdjustSettings`, and
//! the one place `effects.wgsl` is loaded.
//!
//! `offcut-model` owns the *values* (clamping, aspect-lock math, the
//! `0..=100` range). This module owns only their translation into the
//! bytes the shader reads. Keeping that translation in exactly one place
//! is what lets the "preview and export must not drift"
//! requirement be a structural fact rather than a review checklist item:
//! both paths build an `EffectsUniform` from the same `Clip`.

use offcut_model::{AdjustSettings, CropTransform};

/// The WGSL `Effects` uniform block, laid out to match `effects.wgsl`
/// exactly.
///
/// `repr(C)` plus three `[f32; 4]` members is deliberate: WGSL's uniform
/// address space aligns every member to 16 bytes, so a struct of loose
/// `f32`s would need manual padding that silently misaligns the moment
/// someone inserts a field. Packing into `vec4`-shaped arrays makes the
/// layout self-evident and the Rust and WGSL sides trivially comparable.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct EffectsUniform {
    /// `(x, y, width, height)` of the crop rect, normalized source coords.
    pub crop: [f32; 4],
    /// `(straighten_radians, smooth, tint, skin_tone)`.
    pub straighten_smooth_tint_skin: [f32; 4],
    /// `(blue_tone, vignette, aspect_ratio, _pad)`.
    pub blue_vignette_aspect_pad: [f32; 4],
    /// `(letterbox_pad_x, letterbox_pad_y, grid_divisions, grid_opacity)`.
    ///
    /// The two pad values are the **total** fraction of the output
    /// consumed by black bars on each axis, taken straight from
    /// `CropTransform::letterbox_padding`. The shader does no aspect
    /// arithmetic of its own, so preview, export, and the inspector's
    /// readout cannot disagree about how thick the bars are.
    ///
    /// `grid_divisions` is a count carried as `f32` because a `vec4<f32>`
    /// cannot hold a bare `u32`; the shader rounds it back. Zero means
    /// "draw no guides", which is why there is no separate enable flag.
    pub letterbox_grid: [f32; 4],
    /// `(fit_scale_x, fit_scale_y, _pad, _pad)`.
    ///
    /// How the picture is scaled to sit inside the widget **without
    /// distorting it**. One of the two is always 1.0; the other is < 1.0
    /// on the axis that has spare room.
    ///
    /// This exists because the quad spans the whole widget, so a 16:9
    /// video in a taller stage was simply stretched to fill it — a ball
    /// rendered as an ellipse and faces came out wrong-shaped. The
    /// preview was lying about the footage's proportions.
    pub fit_scale: [f32; 4],
    /// The interactive crop box in **output** uv: `(x, y, w, h)`.
    ///
    /// Deliberately distinct from `crop` above. That is the region being
    /// *sampled* — the picture you already see. This is the editing
    /// overlay drawn on top of the full frame while you choose it, which
    /// is why the preview shows the whole image with a box over it
    /// rather than the already-cropped result. `w <= 0` disables it, and
    /// the export constructor always leaves it that way.
    pub crop_box: [f32; 4],
}

/// How strongly the crop guides are drawn over the picture.
///
/// Guides must be visible on both a white sky and a black shadow, and
/// must never be mistaken for something in the footage. A half-opacity
/// white hairline is the convention every camera viewfinder uses.
const GRID_OPACITY: f32 = 0.5;

impl EffectsUniform {
    pub const SIZE: u64 = std::mem::size_of::<Self>() as u64;

    /// The no-op transform: full frame, no rotation, every adjust at zero.
    /// `aspect` still has to be real, because `source_uv` divides by it.
    pub fn identity(aspect: f32) -> Self {
        Self {
            crop: [0.0, 0.0, 1.0, 1.0],
            straighten_smooth_tint_skin: [0.0; 4],
            blue_vignette_aspect_pad: [0.0, 0.0, sane_aspect(aspect), 0.0],
            letterbox_grid: [0.0; 4],
            fit_scale: [1.0, 1.0, 0.0, 0.0],
            crop_box: [0.0; 4],
        }
    }

    /// Build from the model's own types. `aspect` is the *source frame's*
    /// width/height, which the shader needs to rotate without shearing —
    /// it is a property of the decoded frame, not of the crop, which is
    /// why it is a separate argument rather than derived from `crop`.
    ///
    /// Guides are **off** here. Composition guides are a framing aid, not
    /// part of the image: baking them into every frame would export them.
    /// `with_guides` is the opt-in the preview uses.
    pub fn new(crop: &CropTransform, adjust: &AdjustSettings, aspect: f32) -> Self {
        Self {
            crop: [crop.rect.x, crop.rect.y, crop.rect.width, crop.rect.height],
            straighten_smooth_tint_skin: [
                crop.straighten_deg().to_radians(),
                adjust.smooth.as_uniform(),
                adjust.tint.as_uniform(),
                adjust.skin_tone.as_uniform(),
            ],
            blue_vignette_aspect_pad: [
                adjust.blue_tone.as_uniform(),
                adjust.vignette.as_uniform(),
                sane_aspect(aspect),
                0.0,
            ],
            letterbox_grid: [0.0, 0.0, 0.0, 0.0],
            // Identity: the export target is already the right shape, so
            // the picture fills it exactly. Only the on-screen preview,
            // whose widget is whatever shape the window makes it, needs
            // to fit the image inside itself.
            fit_scale: [1.0, 1.0, 0.0, 0.0],
            // Never baked: an export must contain the cropped picture,
            // not a drawing of the box used to choose it.
            crop_box: [0.0; 4],
        }
    }

    /// The same uniform with composition guides enabled.
    ///
    /// Separate from `new` so that "what the export bakes" and "what the
    /// editor draws over it" are different values by construction. An
    /// export path physically cannot reach this constructor by accident,
    /// which is what stops a rule-of-thirds overlay from being burned
    /// into somebody's finished video.
    pub fn with_guides(crop: &CropTransform, adjust: &AdjustSettings, aspect: f32) -> Self {
        let mut uniform = Self::new(crop, adjust, aspect);
        uniform.letterbox_grid[2] = crop.grid.divisions() as f32;
        uniform.letterbox_grid[3] = if crop.grid.divisions() == 0 { 0.0 } else { GRID_OPACITY };
        uniform
    }

    /// Fit the picture inside a widget of `viewport` pixels without
    /// distorting it.
    ///
    /// `content` is the display aspect of what is being shown. The
    /// picture is scaled down on whichever axis has spare room, leaving
    /// the widget's background visible on the other — the standard
    /// letterbox/pillarbox, and the only honest way to show footage in a
    /// container of a different shape.
    ///
    /// Without this the quad simply spanned the whole widget and the
    /// image was stretched to fit: a circle rendered as an ellipse.
    pub fn fit_to_viewport(&mut self, content_aspect: f32, viewport: (f32, f32)) {
        let (vw, vh) = viewport;
        if !(vw > 0.0 && vh > 0.0) {
            return;
        }
        let content = sane_aspect(content_aspect);
        let widget = sane_aspect(vw / vh);

        let (sx, sy) = if content > widget {
            // Picture is wider than the widget: full width, spare height.
            (1.0, widget / content)
        } else {
            // Taller/narrower: full height, spare width.
            (content / widget, 1.0)
        };
        self.fit_scale = [sx, sy, 0.0, 0.0];
    }

    /// The uniform for **editing** a crop: the full frame is shown, with
    /// the crop rect drawn over it as a draggable box.
    ///
    /// # Why the sample rect is reset to full frame here
    ///
    /// Everywhere else, `crop.rect` is the region being sampled, so the
    /// preview shows the cropped result. That is exactly wrong while
    /// choosing the crop: you cannot frame a shot against footage that
    /// has already been cut away, and the eight handles would sit on the
    /// edges of a picture that fills the whole viewport. The reference
    /// behaves the same way — the image stays whole and a box moves over
    /// it.
    ///
    /// So this shows the entire frame and hands the rect to the overlay
    /// instead. `new`, which the export uses, cannot reach this.
    pub fn editing_crop(crop: &CropTransform, adjust: &AdjustSettings, aspect: f32) -> Self {
        let mut uniform = Self::new(crop, adjust, aspect);
        // Show the whole picture...
        uniform.crop = [0.0, 0.0, 1.0, 1.0];
        // ...with no bars, which describe an output shape that does not
        // apply while framing.
        uniform.letterbox_grid[0] = 0.0;
        uniform.letterbox_grid[1] = 0.0;
        // Guides belong to the box, drawn by the overlay.
        uniform.letterbox_grid[2] = crop.grid.divisions() as f32;
        uniform.letterbox_grid[3] = 0.0;
        uniform.crop_box = [
            crop.rect.x,
            crop.rect.y,
            crop.rect.width.max(0.0),
            crop.rect.height.max(0.0),
        ];
        uniform
    }

    /// Raw bytes for `Queue::write_buffer`.
    ///
    /// Hand-rolled rather than pulled from `bytemuck` because this is the
    /// only type in the workspace that needs it, and every field is
    /// already `f32` — there is no padding to get wrong and no invariant a
    /// derive would check that `SIZE`'s own test does not.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: `Self` is `repr(C)` and contains only `[f32; 4]`
        // members, so it has no padding bytes, no pointers, and no
        // invalid bit patterns — every possible byte sequence of this
        // size is a valid value of this type. The returned slice borrows
        // `self`, so it cannot outlive the value it views.
        unsafe { std::slice::from_raw_parts((self as *const Self).cast::<u8>(), std::mem::size_of::<Self>()) }
    }

    /// True when this uniform is a pure pass-through — every adjust at
    /// zero, no rotation, full-frame crop. the perf gate
    /// ("frame-time delta between Crop/Adjust at rest vs. active must be
    /// ~0") compares these two states; this predicate names the "at rest"
    /// one so the test does not re-derive it.
    /// # Why letterboxing and guides count as "not at rest"
    ///
    /// `offscreen.rs` uses this predicate as a **fast path**: an at-rest
    /// uniform skips the shader entirely and copies the frame straight
    /// through. That is a real optimization and worth keeping — but it
    /// means anything this function forgets is silently *not rendered*.
    ///
    /// Letterboxing was exactly that bug, caught by a GPU pixel test
    /// rather than by review: a clip with bars but no adjustments has a
    /// full-frame crop rect and all-zero adjust values, so the original
    /// version of this predicate called it at rest, the bake took the
    /// pass-through path, and the exported frame had no bars at all
    /// despite the uniform carrying the correct padding.
    ///
    /// The rule this encodes: at rest means *the output is byte-for-byte
    /// the input*. Bars change pixels. Guides change pixels. Both belong
    /// here.
    pub fn is_at_rest(&self) -> bool {
        self.crop == [0.0, 0.0, 1.0, 1.0]
            && self.straighten_smooth_tint_skin == [0.0; 4]
            && self.blue_vignette_aspect_pad[0] == 0.0
            && self.blue_vignette_aspect_pad[1] == 0.0
            // Bars are painted pixels, not a no-op.
            && self.letterbox_grid[0] == 0.0
            && self.letterbox_grid[1] == 0.0
            // So are guides.
            && self.letterbox_grid[3] == 0.0
            // And so is the crop-editing overlay.
            && self.crop_box[2] <= 0.0
            // And so is fitting: a scaled-down quad leaves letterbox
            // margins, which the pass-through path cannot produce.
            //
            // This is the *second* time this predicate has silently
            // skipped a new feature -- letterbox bars were the first.
            // The rule it encodes is worth restating: at rest means the
            // output is byte-for-byte the input, so anything that moves
            // or resizes the quad belongs here.
            && self.fit_scale[0] >= 1.0
            && self.fit_scale[1] >= 1.0
    }
}

/// A zero or non-finite aspect would make the shader divide by zero and
/// paint NaN (which most drivers render as black, some as garbage). The
/// frame that produced it is still perfectly displayable, so clamp rather
/// than refuse to draw.
fn sane_aspect(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 { aspect } else { 1.0 }
}


/// The shared WGSL source for crop/straighten/adjust. Both `preview.rs`
/// (on-screen) and `offscreen.rs` (export bake) create their pipelines
/// from this exact constant.
pub const EFFECTS_SHADER: &str = include_str!("effects.wgsl");

#[cfg(test)]
mod tests {
    use super::*;
    use offcut_model::{AdjustValue, AspectPreset};

    #[test]
    fn uniform_size_matches_the_wgsl_block_layout() {
        // Six vec4<f32> = 6 * 16 bytes. If this ever fails, the Rust
        // struct and effects.wgsl's `Effects` have diverged, and the
        // shader would read garbage for whichever field moved.
        assert_eq!(EffectsUniform::SIZE, 96);
    }

    /// Guides are an editing aid, not part of the picture. The export
    /// path builds its uniform with `new`, so `new` must never enable
    /// them — otherwise a rule-of-thirds overlay gets burned into the
    /// finished video.
    #[test]
    fn the_export_constructor_never_enables_guides() {
        let mut crop = CropTransform::identity();
        crop.grid = offcut_model::CropGrid::Fine;

        let baked = EffectsUniform::new(&crop, &AdjustSettings::default(), 16.0 / 9.0);
        assert_eq!(baked.letterbox_grid[2], 0.0, "export must not draw guides");
        assert_eq!(baked.letterbox_grid[3], 0.0, "export must not draw guides");

        let previewed = EffectsUniform::with_guides(&crop, &AdjustSettings::default(), 16.0 / 9.0);
        assert_eq!(previewed.letterbox_grid[2], 4.0, "the preview should show the chosen guide");
        assert!(previewed.letterbox_grid[3] > 0.0);
    }

    /// Turning the grid off must produce a uniform the shader skips
    /// entirely, not a zero-division guide.
    #[test]
    fn the_off_grid_reaches_the_shader_as_a_disabled_overlay() {
        let mut crop = CropTransform::identity();
        crop.grid = offcut_model::CropGrid::None;
        let u = EffectsUniform::with_guides(&crop, &AdjustSettings::default(), 1.0);
        assert_eq!(u.letterbox_grid[2], 0.0);
        assert_eq!(u.letterbox_grid[3], 0.0);
    }



    #[test]
    fn identity_is_a_full_frame_pass_through() {
        let u = EffectsUniform::identity(16.0 / 9.0);
        assert!(u.is_at_rest());
        assert_eq!(u.crop, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn identity_from_the_models_own_identity_values_is_also_at_rest() {
        // The two "nothing applied" paths must agree, or a freshly
        // imported clip would render differently from an explicitly reset
        // one.
        let u = EffectsUniform::new(&CropTransform::identity(), &AdjustSettings::default(), 1.777);
        assert!(u.is_at_rest());
    }

    #[test]
    fn adjust_values_reach_the_uniform_in_the_documented_slots() {
        let adjust = AdjustSettings {
            smooth: AdjustValue::new(100),
            tint: AdjustValue::new(50),
            skin_tone: AdjustValue::new(25),
            blue_tone: AdjustValue::new(0),
            vignette: AdjustValue::new(100),
        };
        let u = EffectsUniform::new(&CropTransform::identity(), &adjust, 1.0);
        assert_eq!(u.straighten_smooth_tint_skin[1], 1.0, "smooth");
        assert_eq!(u.straighten_smooth_tint_skin[2], 0.5, "tint");
        assert_eq!(u.straighten_smooth_tint_skin[3], 0.25, "skin tone");
        assert_eq!(u.blue_vignette_aspect_pad[0], 0.0, "blue tone");
        assert_eq!(u.blue_vignette_aspect_pad[1], 1.0, "vignette");
        assert!(!u.is_at_rest());
    }

    #[test]
    fn straighten_is_converted_to_radians() {
        let mut crop = CropTransform::identity();
        crop.set_straighten_deg(45.0);
        let u = EffectsUniform::new(&crop, &AdjustSettings::default(), 1.0);
        assert!((u.straighten_smooth_tint_skin[0] - std::f32::consts::FRAC_PI_4).abs() < 1e-6);
        assert!(!u.is_at_rest());
    }

    #[test]
    fn a_crop_preset_reaches_the_uniform_as_its_rect() {
        let mut crop = CropTransform::identity();
        crop.apply_aspect(AspectPreset::Square, 16.0 / 9.0);
        let u = EffectsUniform::new(&crop, &AdjustSettings::default(), 16.0 / 9.0);
        assert_eq!(u.crop, [crop.rect.x, crop.rect.y, crop.rect.width, crop.rect.height]);
        assert!(!u.is_at_rest(), "a 1:1 crop of a frame is not a pass-through");
    }

    /// Every value the shader consumes must be finite. A NaN uniform does
    /// not fail loudly — it paints black or garbage, which is the hardest
    /// class of rendering bug to trace back to its source.
    #[test]
    fn every_uniform_field_is_finite_for_every_extreme_input() {
        let mut crop = CropTransform::identity();
        crop.set_straighten_deg(f32::NAN); // clamped by the model
        let adjust = AdjustSettings {
            smooth: AdjustValue::new(255), // clamped to 100 by the model
            ..AdjustSettings::default()
        };
        for aspect in [0.0f32, -1.0, f32::NAN, f32::INFINITY, 1.777] {
            let u = EffectsUniform::new(&crop, &adjust, aspect);
            for value in u.crop.iter().chain(&u.straighten_smooth_tint_skin).chain(&u.blue_vignette_aspect_pad) {
                assert!(value.is_finite(), "non-finite uniform {value} at aspect {aspect}");
            }
            assert!(u.blue_vignette_aspect_pad[2] > 0.0, "aspect must stay positive at aspect {aspect}");
        }
    }

    #[test]
    fn as_bytes_has_exactly_the_declared_size() {
        let u = EffectsUniform::identity(1.0);
        assert_eq!(u.as_bytes().len() as u64, EffectsUniform::SIZE);
    }

    #[test]
    fn the_shader_source_declares_the_entry_points_both_pipelines_bind() {
        assert!(EFFECTS_SHADER.contains("fn vs_main"));
        assert!(EFFECTS_SHADER.contains("fn fs_main"));
        assert!(EFFECTS_SHADER.contains("var<uniform> effects"));
    }
}
