// Offcut's one and only frame shader: crop + straighten and
// the five closed-set Adjust tools (§4.6), in a single fragment pass.
//
// This exact file is used by BOTH the on-screen preview (preview.rs, via
// iced's render pass) and the export bake (offscreen.rs, into a texture
// that gets encoded). That is not a convenience -- it is the mechanism
// that makes "what you see is what you export" true by construction.
// the design rule names the failure it prevents: "the crop/adjust shader math
// drifting between preview and export." Two copies of this math would
// drift on the first bug fix applied to only one of them.
//
// §4.5/§4.6's performance claim -- "none of it costs an extra render pass"
// -- is upheld here structurally: crop is a UV transform on the sample
// coordinate, and all five adjust tools are arithmetic on the sampled
// color. No tool adds a pass, a target, or a second sample of the frame,
// except Smooth, which is a 3x3 tap and says so.

struct Effects {
    // Crop rect in normalized source coords (x, y, width, height).
    crop: vec4<f32>,
    // Straighten angle in radians, then the three adjust values that
    // pack into this vector. Packed in vec4s because WGSL uniform buffer
    // layout aligns every member to 16 bytes: five loose f32s would cost
    // the same 80 bytes as this does at 32, and read less clearly.
    straighten_smooth_tint_skin: vec4<f32>,
    // blue_tone, vignette, and the frame's aspect ratio (width/height),
    // which straighten needs to rotate about a visually square axis, plus
    // one slot reserved so this stays a vec4.
    blue_vignette_aspect_pad: vec4<f32>,
    // (unused, unused, grid_divisions, grid_opacity).
    //
    // grid_divisions is a count (3 = thirds) carried as f32 because a
    // vec4<f32> cannot hold a u32. Zero disables the guides entirely.
    //
    // The first two slots held letterbox bar thickness. Bars are gone:
    // the export now produces a file of the CROPPED shape, so there is
    // nothing to pad -- a 1:1 crop writes a square file rather than a
    // square picture inside a widescreen one. The slots are kept so the
    // uniform stays vec4-aligned.
    letterbox_grid: vec4<f32>,
    // The interactive crop box, in OUTPUT uv: (x, y, width, height).
    // Only drawn while the Crop tab is open; width <= 0 disables it.
    //
    // This is deliberately NOT `crop` above. That rect is the region
    // being sampled -- the picture you already see. This one is the
    // editing overlay drawn ON TOP of the full frame while choosing it,
    // which is why the preview shows the whole image with a box over it
    // rather than the cropped result.
    // (fit_scale_x, fit_scale_y, pad, pad): how far the quad is shrunk
    // on each axis so the picture keeps its proportions inside a widget
    // of a different shape. One is always 1.0.
    //
    // Without this the quad spanned the whole widget and the image was
    // STRETCHED to fill it -- videotestsrc's ball rendered as an
    // ellipse, and faces came out the wrong shape. A preview that
    // misreports proportions is worse than no preview.
    //
    // ORDER MATTERS: this block is a byte-for-byte mirror of the
    // #[repr(C)] struct in effects.rs. Declaring a member in a different
    // position there reads a different field's bytes, silently -- which
    // is exactly what happened when this sat after `crop_box`: the
    // vertex stage read zeros for the scale and collapsed the quad.
    fit_scale: vec4<f32>,
    crop_box: vec4<f32>,
};

@group(0) @binding(0) var frame_texture: texture_2d<f32>;
@group(0) @binding(1) var frame_sampler: sampler;
@group(0) @binding(2) var<uniform> effects: Effects;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOutput {
    var positions = array<vec2<f32>, 4>(
        vec2<f32>(-1.0,  1.0),
        vec2<f32>(-1.0, -1.0),
        vec2<f32>( 1.0,  1.0),
        vec2<f32>( 1.0, -1.0),
    );
    var uvs = array<vec2<f32>, 4>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    var out: VertexOutput;
    // Shrink the quad on whichever axis has spare room, so the picture
    // is fitted rather than stretched. The uncovered remainder is left
    // for the fragment stage to paint as letterbox.
    let fit = vec2<f32>(max(effects.fit_scale.x, 0.0001), max(effects.fit_scale.y, 0.0001));
    out.position = vec4<f32>(positions[index] * fit, 0.0, 1.0);
    out.uv = uvs[index];
    return out;
}

// Rec. 709 luma. Used by Smooth (to keep the blur luma-aware) and by
// Vignette (which darkens luminance, not saturation).
fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Map an output UV through crop + straighten into a source UV.
//
// Rotation happens about the crop rect's center, in an aspect-corrected
// space: rotating in raw normalized UV space on a 16:9 frame shears the
// image instead of rotating it, because one UV unit is 1.78x longer on x
// than on y. Multiplying x by the aspect before the rotation and dividing
// after is what makes a straightened horizon actually straight.
fn source_uv(uv: vec2<f32>) -> vec2<f32> {
    let crop = effects.crop;
    let angle = effects.straighten_smooth_tint_skin.x;
    let aspect = max(effects.blue_vignette_aspect_pad.z, 0.0001);

    // UV within the crop rect, centered on zero.
    let centered = uv - vec2<f32>(0.5, 0.5);

    let s = sin(angle);
    let c = cos(angle);
    // Into aspect-corrected space, rotate, back out.
    let scaled = vec2<f32>(centered.x * aspect, centered.y);
    let rotated = vec2<f32>(scaled.x * c - scaled.y * s, scaled.x * s + scaled.y * c);
    let uncorrected = vec2<f32>(rotated.x / aspect, rotated.y);

    // Place inside the crop rect in source coordinates.
    let crop_center = vec2<f32>(crop.x + crop.z * 0.5, crop.y + crop.w * 0.5);
    return crop_center + vec2<f32>(uncorrected.x * crop.z, uncorrected.y * crop.w);
}

// Smooth: a soft 3x3 tap blended by strength, weighted toward keeping
// edges. the product rules scopes this as "skin/detail softening," not a
// denoiser -- blending toward a small blur is exactly that and nothing
// more.
fn apply_smooth(uv: vec2<f32>, base: vec3<f32>, strength: f32) -> vec3<f32> {
    if (strength <= 0.0) {
        return base;
    }
    let dims = vec2<f32>(textureDimensions(frame_texture));
    let texel = vec2<f32>(1.0, 1.0) / max(dims, vec2<f32>(1.0, 1.0));
    var sum = vec3<f32>(0.0, 0.0, 0.0);
    for (var dy = -1; dy <= 1; dy = dy + 1) {
        for (var dx = -1; dx <= 1; dx = dx + 1) {
            let offset = vec2<f32>(f32(dx), f32(dy)) * texel;
            sum = sum + textureSample(frame_texture, frame_sampler, uv + offset).rgb;
        }
    }
    let blurred = sum / 9.0;
    return mix(base, blurred, clamp(strength, 0.0, 1.0));
}

// Tint: a green<->magenta shift, the classic white-balance tint axis
// (distinct from temperature). Positive strength pushes magenta, which is
// the direction that warms skin under fluorescent light -- the reason a
// phone editor offers this control at all.
fn apply_tint(color: vec3<f32>, strength: f32) -> vec3<f32> {
    let shift = strength * 0.18;
    return clamp(
        vec3<f32>(color.r + shift, color.g - shift * 0.6, color.b + shift * 0.5),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

// Skin tone: warms and slightly lifts pixels already near a skin hue,
// leaving everything else alone. The hue mask is what makes this "skin
// tone" rather than "warmth" -- without it, a blue sky warms too, which
// is exactly the whole-image grading the product rules refuses.
fn apply_skin_tone(color: vec3<f32>, strength: f32) -> vec3<f32> {
    let mx = max(color.r, max(color.g, color.b));
    let mn = min(color.r, min(color.g, color.b));
    let chroma = mx - mn;
    // Skin is red-dominant with green above blue and moderate saturation.
    let redness = clamp((color.r - color.b) * 2.0, 0.0, 1.0);
    let midtone = 1.0 - abs(luma(color) - 0.55) * 2.0;
    let mask = clamp(redness * clamp(midtone, 0.0, 1.0) * step(0.04, chroma), 0.0, 1.0);
    let warmed = vec3<f32>(color.r + 0.14, color.g + 0.05, color.b - 0.05);
    return clamp(mix(color, warmed, mask * strength), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Blue tone: deepens blues (sky, water) without touching neutrals, by
// masking on blue dominance.
fn apply_blue_tone(color: vec3<f32>, strength: f32) -> vec3<f32> {
    let mask = clamp((color.b - max(color.r, color.g)) * 3.0, 0.0, 1.0);
    let deepened = vec3<f32>(color.r * 0.88, color.g * 0.96, min(color.b * 1.12 + 0.03, 1.0));
    return clamp(mix(color, deepened, mask * strength), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Vignette: radial luminance falloff from the frame center. Multiplies
// the color rather than blending toward black so it darkens without
// desaturating, which is what makes it read as light falloff instead of
// a black overlay.
fn apply_vignette(uv: vec2<f32>, color: vec3<f32>, strength: f32) -> vec3<f32> {
    let d = distance(uv, vec2<f32>(0.5, 0.5)) / 0.7071068; // normalize to corner
    let falloff = 1.0 - strength * clamp(d * d, 0.0, 1.0) * 0.9;
    return color * clamp(falloff, 0.0, 1.0);
}

// Map an output UV into the letterboxed content area.
//
// With bars, the picture no longer fills the output: it occupies a
// centered sub-rectangle, and the margins are black. This rescales the
// output coordinate so that the content area still spans a full 0..1 UV
// range -- which is what lets every stage downstream (crop, straighten,
// the adjust tools) stay completely unaware that bars exist.
//
// Returns a UV outside 0..1 for any pixel that falls in a bar, which the
// caller renders as black.
fn unpad_uv(uv: vec2<f32>) -> vec2<f32> {
    let pad = vec2<f32>(effects.letterbox_grid.x, effects.letterbox_grid.y);
    // Fraction of the output the content actually occupies.
    let used = max(vec2<f32>(1.0, 1.0) - pad, vec2<f32>(0.0001, 0.0001));
    // Bars are split evenly, so content starts half the padding in.
    let origin = pad * 0.5;
    return (uv - origin) / used;
}

// Rule-of-thirds (or finer) guides, drawn ON TOP of the finished picture.
//
// Only ever enabled for the on-screen preview -- see EffectsUniform's
// `with_guides`. Lines are a constant number of OUTPUT pixels wide rather
// than a fraction of the frame, so they stay hairlines at any window size
// instead of turning into bands when the preview is small.
fn apply_grid(uv: vec2<f32>, color: vec3<f32>) -> vec3<f32> {
    let divisions = i32(round(effects.letterbox_grid.z));
    let opacity = effects.letterbox_grid.w;
    if (divisions < 2 || opacity <= 0.0) {
        return color;
    }

    // Half-width of a line, in UV, derived from the screen-space
    // derivative so it is ~1px regardless of how large this is drawn.
    let half_px = max(fwidth(uv.x), fwidth(uv.y)) * 0.6;

    var hit = 0.0;
    for (var i = 1; i < divisions; i = i + 1) {
        let at = f32(i) / f32(divisions);
        if (abs(uv.x - at) < half_px || abs(uv.y - at) < half_px) {
            hit = 1.0;
        }
    }
    if (hit == 0.0) {
        return color;
    }
    // White line, blended rather than replaced, so the picture underneath
    // stays readable through the guide.
    return mix(color, vec3<f32>(1.0, 1.0, 1.0), opacity);
}

// The crop editing overlay: everything outside the box is dimmed, the
// box is outlined, and eight handles sit on its edges and corners.
//
// Drawn in OUTPUT space over the finished picture, so it is unaffected
// by crop, straighten, or any adjust tool -- an overlay that dimmed with
// the vignette or rotated with straighten would be describing something
// other than the region it claims to.
fn apply_crop_box(uv: vec2<f32>, color: vec3<f32>) -> vec3<f32> {
    let box = effects.crop_box;
    if (box.z <= 0.0) {
        return color;
    }

    let l = box.x;
    let t = box.y;
    let r = box.x + box.z;
    let b = box.y + box.w;

    let inside = uv.x >= l && uv.x <= r && uv.y >= t && uv.y <= b;

    // Hairlines that stay hairlines at any preview size.
    let px = max(fwidth(uv.x), fwidth(uv.y));
    let edge = px * 1.2;
    let near_v = (abs(uv.x - l) < edge || abs(uv.x - r) < edge) && uv.y >= t - edge && uv.y <= b + edge;
    let near_h = (abs(uv.y - t) < edge || abs(uv.y - b) < edge) && uv.x >= l - edge && uv.x <= r + edge;

    var out = color;

    // Dim what will be cut away. Multiplying rather than blending to
    // black keeps the discarded region readable -- you still need to see
    // what you are excluding in order to decide where the box goes.
    if (!inside) {
        out = out * 0.45;
    }

    // Thirds guides inside the box, so composition is judged against the
    // crop rather than against the whole frame.
    let divisions = i32(round(effects.letterbox_grid.z));
    if (inside && divisions >= 2) {
        let fx = (uv.x - l) / max(box.z, 0.0001);
        let fy = (uv.y - t) / max(box.w, 0.0001);
        let gx = px / max(box.z, 0.0001) * 0.7;
        let gy = px / max(box.w, 0.0001) * 0.7;
        for (var i = 1; i < divisions; i = i + 1) {
            let at = f32(i) / f32(divisions);
            if (abs(fx - at) < gx || abs(fy - at) < gy) {
                out = mix(out, vec3<f32>(1.0), 0.35);
            }
        }
    }

    if (near_v || near_h) {
        out = vec3<f32>(1.0, 1.0, 1.0);
    }

    // Eight handles: three positions per axis (0, 0.5, 1), minus the
    // centre. Solid discs, sized in output pixels so they stay grabbable
    // whatever the preview scale.
    //
    // Pulled INWARD by their own radius at the frame edges. A crop that
    // still covers the whole frame puts its handles exactly on the
    // viewport boundary, where half of each disc is clipped away and the
    // remaining half is a slver -- the default state of every clip, and
    // therefore the first thing a user tries to drag. Insetting keeps
    // them whole and grabbable without moving the box they describe.
    let radius = px * 4.5;
    let inset = radius * 1.15;
    for (var i = 0; i < 3; i = i + 1) {
        for (var j = 0; j < 3; j = j + 1) {
            if (i == 1 && j == 1) {
                continue;
            }
            var hx = mix(l, r, f32(i) * 0.5);
            var hy = mix(t, b, f32(j) * 0.5);
            hx = clamp(hx, inset, 1.0 - inset);
            hy = clamp(hy, inset, 1.0 - inset);
            let d = vec2<f32>(uv.x - hx, uv.y - hy);
            if (dot(d, d) < radius * radius) {
                out = vec3<f32>(1.0, 1.0, 1.0);
            }
        }
    }

    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Letterbox first: everything after this works in content space, so
    // no other stage needs to know bars exist.
    let content_uv = unpad_uv(in.uv);
    if (content_uv.x < 0.0 || content_uv.x > 1.0 || content_uv.y < 0.0 || content_uv.y > 1.0) {
        // Inside a bar. Opaque black -- this is picture the user chose to
        // pad with, not absence of picture.
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let src = source_uv(content_uv);

    // Straighten rotates the sample grid, so some output pixels map
    // outside the source. Those become black rather than clamping to the
    // edge pixel: an edge-clamped rotation smears the border pixel into a
    // streak that looks like a rendering bug, while black corners read as
    // the honest "the frame does not cover this" that every photo editor
    // shows mid-straighten.
    if (src.x < 0.0 || src.x > 1.0 || src.y < 0.0 || src.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let sampled = textureSample(frame_texture, frame_sampler, src);
    var color = sampled.rgb;

    let smooth_v = effects.straighten_smooth_tint_skin.y;
    let tint_v = effects.straighten_smooth_tint_skin.z;
    let skin_v = effects.straighten_smooth_tint_skin.w;
    let blue_v = effects.blue_vignette_aspect_pad.x;
    let vignette_v = effects.blue_vignette_aspect_pad.y;

    color = apply_smooth(src, color, smooth_v);
    color = apply_tint(color, tint_v);
    color = apply_skin_tone(color, skin_v);
    color = apply_blue_tone(color, blue_v);
    // Vignette is measured from the CONTENT centre, not the output
    // centre: with bars, those differ, and a vignette that darkened
    // toward the middle of the padded frame would put its falloff in the
    // wrong place entirely.
    color = apply_vignette(content_uv, color, vignette_v);

    // Guides go on last, over the finished image, so no adjust tool can
    // tint or blur them.
    color = apply_grid(content_uv, color);

    // The crop editing overlay sits above even those, in OUTPUT space.
    color = apply_crop_box(in.uv, color);

    return vec4<f32>(color, sampled.a);
}
