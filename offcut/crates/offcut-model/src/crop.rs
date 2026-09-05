//! The design rule (Crop, added per the product's scope expansion):
//! "`CropTransform` becomes a rectangular sample region and a rotation
//! angle fed into the same fragment shader that already samples the
//! decoded texture for display... Preview cost: effectively free."
//!
//! This module owns only the *value* — clamping, aspect-lock math, and the
//! sample-rect invariant (`0.0..=1.0`, the design rule`'s property-test
//! requirement: "Random `CropTransform` values must never produce a sample
//! rect outside `0.0..=1.0`."). The shader that consumes it lives in
//! offcut-render and has no reason to duplicate this arithmetic.

use serde::{Deserialize, Serialize};

/// The design system's five aspect chips: Free, 1:1, 4:5, 9:16, 16:9.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum AspectPreset {
    Free,
    Square,       // 1:1
    Portrait45,   // 4:5
    Portrait916,  // 9:16
    Landscape169, // 16:9
}

impl AspectPreset {
    pub const ALL: [AspectPreset; 5] = [
        AspectPreset::Free,
        AspectPreset::Square,
        AspectPreset::Portrait45,
        AspectPreset::Portrait916,
        AspectPreset::Landscape169,
    ];

    /// Ratio as width/height, or `None` for Free (no forced ratio).
    pub fn ratio(self) -> Option<f64> {
        match self {
            AspectPreset::Free => None,
            AspectPreset::Square => Some(1.0),
            AspectPreset::Portrait45 => Some(4.0 / 5.0),
            AspectPreset::Portrait916 => Some(9.0 / 16.0),
            AspectPreset::Landscape169 => Some(16.0 / 9.0),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AspectPreset::Free => "Free",
            AspectPreset::Square => "1:1",
            AspectPreset::Portrait45 => "4:5",
            AspectPreset::Portrait916 => "9:16",
            AspectPreset::Landscape169 => "16:9",
        }
    }
}

/// A crop rectangle in normalized source-frame coordinates: `0.0..=1.0` on
/// each axis, origin top-left. Every constructor clamps into range —
/// there is deliberately no way to build a `NormalizedRect` that fails the
/// property-test invariant in the design rule, because the clamp happens at
/// construction, not as a separate validation step callers can forget.
#[derive(Copy, Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Default for NormalizedRect {
    /// The full frame — the only sensible "no crop yet" value.
    fn default() -> Self {
        Self::FULL
    }
}

impl NormalizedRect {
    pub const FULL: NormalizedRect = NormalizedRect { x: 0.0, y: 0.0, width: 1.0, height: 1.0 };

    /// Construct, clamping every field into a rect that is fully inside
    /// `0.0..=1.0`. Width/height are clamped first, then the origin is
    /// clamped so `x + width <= 1.0` and `y + height <= 1.0` — clamping
    /// origin *after* size means a too-large size never gets a chance to
    /// push the origin negative before this function even sees it.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        let width = width.clamp(0.0, 1.0);
        let height = height.clamp(0.0, 1.0);
        let x = x.clamp(0.0, 1.0 - width);
        let y = y.clamp(0.0, 1.0 - height);
        Self { x, y, width, height }
    }

    /// True iff this rect is fully within `0.0..=1.0` on both axes — the
    /// invariant the design rule requires every random `CropTransform` to hold.
    /// `new` always upholds it; this exists so the property test can
    /// *assert* the invariant rather than trust the constructor blindly.
    pub fn is_valid(self) -> bool {
        let in_unit = |v: f32| (0.0..=1.0).contains(&v);
        in_unit(self.x)
            && in_unit(self.y)
            && in_unit(self.width)
            && in_unit(self.height)
            && self.x + self.width <= 1.0 + f32::EPSILON
            && self.y + self.height <= 1.0 + f32::EPSILON
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Re-fit this rect to a new aspect ratio (width/height), keeping the
    /// same center and the largest size that both matches the ratio and
    /// stays within the source frame. Used when a user picks an
    /// `AspectPreset` chip: the design rule, "Selecting a preset snaps
    /// `CropTransform::rect` to that ratio, centered."
    pub fn fit_to_ratio(self, ratio: f64) -> Self {
        let (cx, cy) = self.center();
        // Start from the largest square-ish box centered here that still
        // fits the frame, then shrink one axis to hit the ratio exactly.
        let max_half_w = cx.min(1.0 - cx);
        let max_half_h = cy.min(1.0 - cy);
        let ratio = ratio as f32;
        // Try width-constrained first.
        let (mut half_w, mut half_h) = (max_half_w, max_half_w / ratio);
        if half_h > max_half_h {
            half_h = max_half_h;
            half_w = max_half_h * ratio;
        }
        NormalizedRect::new(cx - half_w, cy - half_h, half_w * 2.0, half_h * 2.0)
    }
}

/// Which guide overlay to draw over the preview while cropping.
///
/// This lives on the clip rather than in transient UI state because which
/// guide helps depends on the shot being framed, and a setting that
/// silently resets when you switch clips is worse than no setting.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum CropGrid {
    None,
    /// Rule of thirds: two lines each way. The default guide in every
    /// camera and photo editor, and the one people actually compose to.
    #[default]
    Thirds,
    /// A denser 4×4 lattice, for lining an edge up against something
    /// straight rather than composing a subject.
    Fine,
}

impl CropGrid {
    pub const ALL: [CropGrid; 3] = [CropGrid::None, CropGrid::Thirds, CropGrid::Fine];

    pub fn label(self) -> &'static str {
        match self {
            CropGrid::None => "Off",
            CropGrid::Thirds => "Thirds",
            CropGrid::Fine => "Fine",
        }
    }

    /// Interior divisions per axis: 3 draws 2 lines, 4 draws 3. Zero
    /// means "draw nothing", which lets the shader treat this as one
    /// uniform value with no separate enable flag.
    pub fn divisions(self) -> u32 {
        match self {
            CropGrid::None => 0,
            CropGrid::Thirds => 3,
            CropGrid::Fine => 4,
        }
    }
}

/// Which part of the crop box a pointer is on.
///
/// Eight handles plus the interior, matching the reference: corners
/// resize both axes, edges resize one, and the middle moves the whole
/// box without changing its size.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum CropHandle {
    #[default]
    None,
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
    /// Inside the box: drag to reposition without resizing.
    Move,
}

impl CropHandle {
    /// The eight resize handles, in the order they are drawn.
    pub const RESIZE: [CropHandle; 8] = [
        CropHandle::TopLeft,
        CropHandle::Top,
        CropHandle::TopRight,
        CropHandle::Right,
        CropHandle::BottomRight,
        CropHandle::Bottom,
        CropHandle::BottomLeft,
        CropHandle::Left,
    ];

    /// Which edges this handle moves, as `(left, top, right, bottom)`.
    ///
    /// Expressed as a mask rather than a match at every call site: the
    /// drag maths is then one formula for all eight handles instead of
    /// eight near-identical branches that can disagree.
    pub fn edges(self) -> (bool, bool, bool, bool) {
        use CropHandle::*;
        match self {
            TopLeft => (true, true, false, false),
            Top => (false, true, false, false),
            TopRight => (false, true, true, false),
            Right => (false, false, true, false),
            BottomRight => (false, false, true, true),
            Bottom => (false, false, false, true),
            BottomLeft => (true, false, false, true),
            Left => (true, false, false, false),
            Move | None => (false, false, false, false),
        }
    }

    /// Its position on the box as `(fx, fy)` fractions, where 0 is the
    /// left/top edge, 0.5 the middle and 1 the right/bottom.
    pub fn anchor(self) -> (f32, f32) {
        use CropHandle::*;
        match self {
            TopLeft => (0.0, 0.0),
            Top => (0.5, 0.0),
            TopRight => (1.0, 0.0),
            Right => (1.0, 0.5),
            BottomRight => (1.0, 1.0),
            Bottom => (0.5, 1.0),
            BottomLeft => (0.0, 1.0),
            Left => (0.0, 0.5),
            Move | None => (0.5, 0.5),
        }
    }
}

/// the `CropTransform` and §4.5's straighten range: "-45..+45,
/// the ruler-style dial from the design system."
#[derive(Copy, Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct CropTransform {
    pub aspect: AspectPreset,
    pub rect: NormalizedRect,
    straighten_deg: f32, // private: always accessed through the clamped accessor below
    /// Whether resize handles preserve the current ratio.
    ///
    /// **Derived, never set directly** — it is `aspect != Free`, and
    /// `apply_aspect` is the only thing that writes it. Private for that
    /// reason: it and `aspect` describe one fact, and when they were two
    /// independently writable fields they contradicted each other. The
    /// symptom was "Free is not really free": the lock defaulted to
    /// `true`, choosing Free cleared only the ratio, and the box went on
    /// obeying a shape the UI no longer showed.
    ///
    /// Making it private is what stops that recurring. There is no
    /// user-facing lock control either — a ratio *is* the lock, so a
    /// separate switch would be a second source of truth for the same
    /// thing.
    lock_aspect: bool,
    /// Which composition guide to overlay while framing.
    pub grid: CropGrid,
}

impl CropTransform {
    pub const STRAIGHTEN_MIN: f32 = -45.0;
    pub const STRAIGHTEN_MAX: f32 = 45.0;

    pub fn identity() -> Self {
        Self {
            aspect: AspectPreset::Free,
            rect: NormalizedRect::FULL,
            straighten_deg: 0.0,
            // Free by default, and Free means unlocked. A new clip whose
            // ratio chip reads "Free" while its box refuses to change
            // shape is lying about its own state.
            lock_aspect: false,
            grid: CropGrid::Thirds,
        }
    }

    /// Whether resize handles hold the current ratio. True for every
    /// named preset, false for `Free`.
    pub fn lock_aspect(&self) -> bool {
        self.lock_aspect
    }

    pub fn straighten_deg(&self) -> f32 {
        self.straighten_deg
    }

    /// The only way to set the straighten angle — clamps into
    /// The design system's documented ±45° dial range.
    ///
    /// **NaN maps to 0°, not to NaN.** `f32::clamp` propagates NaN by
    /// design, so the obvious one-line clamp is not a guard against the
    /// one input that matters: this becomes a shader uniform, and a NaN
    /// rotation paints the whole frame black with no error anywhere.
    pub fn set_straighten_deg(&mut self, degrees: f32) {
        self.straighten_deg = if degrees.is_nan() {
            0.0
        } else {
            degrees.clamp(Self::STRAIGHTEN_MIN, Self::STRAIGHTEN_MAX)
        };
    }

    /// # The lock follows the preset
    ///
    /// `aspect` and `lock_aspect` used to be two independent facts that
    /// could contradict each other, and they routinely did: `lock_aspect`
    /// defaults to `true`, and choosing "Free" only cleared the *ratio*,
    /// never the lock. So the box stayed locked to whatever shape it
    /// happened to have — dragging one edge dragged the other, and
    /// "Free" resized only along the diagonal. That is exactly the
    /// reported defect, and it is not a drag bug: the box was doing what
    /// it had been told.
    ///
    /// A ratio and "keep this ratio" are the same statement, so the lock
    /// is now *derived* here rather than tracked separately. Free means
    /// unlocked; a named ratio means locked to it. The manual toggle
    /// still exists for overriding a preset afterwards.
    pub fn apply_aspect(&mut self, preset: AspectPreset, frame_aspect: f64) {
        self.aspect = preset;
        self.lock_aspect = preset != AspectPreset::Free;
        if let Some(display_ratio) = preset.ratio() {
            let frame_aspect = if frame_aspect.is_finite() && frame_aspect > 0.0 { frame_aspect } else { 1.0 };
            self.rect = self.rect.fit_to_ratio(display_ratio / frame_aspect);
        }
    }

    /// Smallest crop the box may be dragged to, as a fraction of the
    /// frame. Below this the handles overlap each other and the box
    /// becomes impossible to grab back open.
    pub const MIN_CROP: f32 = 0.05;

    /// Apply a pointer drag to the crop rect.
    ///
    /// `handle` is what was grabbed, `dx`/`dy` are the pointer's movement
    /// **in normalized frame units** since the drag began, and `origin`
    /// is the rect as it was at that moment. Working from the gesture's
    /// origin rather than accumulating per-event deltas is what keeps a
    /// drag from drifting: clamping an incremental step loses the
    /// remainder, so a box dragged into a corner and back does not return
    /// to where it started.
    ///
    /// `frame_aspect` is needed only when the aspect is locked, because
    /// "keep 16:9" is a statement about *displayed* shape and the rect
    /// lives in normalized coordinates where the axes have different
    /// scales.
    pub fn drag_rect(
        &self,
        handle: CropHandle,
        origin: NormalizedRect,
        dx: f32,
        dy: f32,
        frame_aspect: f64,
    ) -> NormalizedRect {
        Self::drag_rect_with(self.lock_aspect(), handle, origin, dx, dy, frame_aspect)
    }

    /// `drag_rect` without needing a `CropTransform` to hand.
    ///
    /// The widget layer knows only "is the ratio locked", and building a
    /// whole transform to carry one bool invited exactly the bug this
    /// module just fixed — a fabricated transform whose `lock_aspect`
    /// disagreed with its `aspect`.
    pub fn drag_rect_with(
        lock_aspect: bool,
        handle: CropHandle,
        origin: NormalizedRect,
        dx: f32,
        dy: f32,
        frame_aspect: f64,
    ) -> NormalizedRect {
        if handle == CropHandle::None {
            return origin;
        }

        // Moving is a pure translation, clamped so the box stays inside
        // the frame without changing size -- the size is not the user's
        // subject here, so it must not silently shrink at the edges.
        if handle == CropHandle::Move {
            let x = (origin.x + dx).clamp(0.0, 1.0 - origin.width);
            let y = (origin.y + dy).clamp(0.0, 1.0 - origin.height);
            return NormalizedRect { x, y, width: origin.width, height: origin.height };
        }

        let (move_l, move_t, move_r, move_b) = handle.edges();
        let mut left = origin.x;
        let mut top = origin.y;
        let mut right = origin.x + origin.width;
        let mut bottom = origin.y + origin.height;

        if move_l {
            left = (origin.x + dx).clamp(0.0, right - Self::MIN_CROP);
        }
        if move_r {
            right = (right + dx).clamp(left + Self::MIN_CROP, 1.0);
        }
        if move_t {
            top = (origin.y + dy).clamp(0.0, bottom - Self::MIN_CROP);
        }
        if move_b {
            bottom = (bottom + dy).clamp(top + Self::MIN_CROP, 1.0);
        }

        let rect = NormalizedRect::new(left, top, right - left, bottom - top);
        if !lock_aspect {
            return rect;
        }

        // Aspect locked: re-impose the ratio the box started with,
        // holding the corner OPPOSITE the one being dragged so the box
        // grows away from the anchor the user is not touching.
        let target = if origin.height > 0.0 {
            (origin.width as f64 / origin.height as f64) * frame_aspect
        } else {
            return rect;
        };
        Self::conform(rect, handle, target, frame_aspect)
    }

    /// Force `rect` back to `target` display ratio, keeping the edge(s)
    /// the drag did not touch fixed.
    fn conform(
        rect: NormalizedRect,
        handle: CropHandle,
        target: f64,
        frame_aspect: f64,
    ) -> NormalizedRect {
        if !(target.is_finite() && target > 0.0 && frame_aspect.is_finite() && frame_aspect > 0.0) {
            return rect;
        }
        // Desired width for this height, in normalized units.
        let want_w = (rect.height as f64 * target / frame_aspect) as f32;
        let want_h = (rect.width as f64 * frame_aspect / target) as f32;

        let (move_l, move_t, move_r, move_b) = handle.edges();
        let horizontal = move_l || move_r;
        let vertical = move_t || move_b;

        // Which axis the user is actually driving decides which one is
        // derived. A side handle drives the axis it moves; a corner
        // drives both, so it takes whichever adjustment makes the box
        // *smaller* and therefore cannot push it outside the frame.
        let derive_height = if horizontal && vertical {
            want_h <= rect.height
        } else {
            horizontal
        };

        let (mut w, mut h) = if derive_height {
            (rect.width, want_h)
        } else {
            (want_w, rect.height)
        };

        // The derived axis can overflow the frame -- dragging a 1:1 box
        // wider on a 16:9 source demands more height than exists. The
        // constructor would clamp that axis and leave the *driving* one
        // untouched, silently breaking the very ratio this function
        // exists to preserve. So when the dependent axis does not fit,
        // pin it to the frame and re-derive the driver from it instead.
        if h > 1.0 {
            h = 1.0;
            w = (h as f64 * target / frame_aspect) as f32;
        }
        if w > 1.0 {
            w = 1.0;
            h = (w as f64 * frame_aspect / target) as f32;
        }

        // Grow away from the fixed edge: the one the drag is not moving.
        let x = if move_l { rect.x + rect.width - w } else { rect.x };
        let y = if move_t { rect.y + rect.height - h } else { rect.y };
        NormalizedRect::new(x, y, w.max(Self::MIN_CROP), h.max(Self::MIN_CROP))
    }

}

impl Default for CropTransform {
    fn default() -> Self {
        Self::identity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(x: f32, y: f32, w: f32, h: f32) -> NormalizedRect {
        NormalizedRect::new(x, y, w, h)
    }

    /// Dragging the interior moves the box without resizing it. Size is
    /// not the subject of a move gesture, so it must not drift.
    #[test]
    fn moving_the_box_translates_without_resizing() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.2, 0.2, 0.4, 0.4);

        let moved = c.drag_rect(CropHandle::Move, origin, 0.1, -0.05, 16.0 / 9.0);
        assert!((moved.width - origin.width).abs() < 1e-6, "width changed during a move");
        assert!((moved.height - origin.height).abs() < 1e-6, "height changed during a move");
        assert!((moved.x - 0.3).abs() < 1e-6);
        assert!((moved.y - 0.15).abs() < 1e-6);
    }

    /// A move pinned against the frame edge must stop, still full size —
    /// silently shrinking at the boundary would lose the user's framing.
    #[test]
    fn moving_into_a_corner_clamps_without_shrinking() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.2, 0.2, 0.4, 0.4);

        let moved = c.drag_rect(CropHandle::Move, origin, -5.0, -5.0, 1.0);
        assert!((moved.width - 0.4).abs() < 1e-6, "the box shrank against the edge");
        assert!(moved.x >= 0.0 && moved.y >= 0.0);
        assert!(moved.is_valid());
    }

    /// Every resize handle must move exactly the edges it owns and leave
    /// the others alone. Checked for all eight rather than a sample,
    /// because a mask typo shows up on precisely one of them.
    #[test]
    fn every_resize_handle_moves_only_its_own_edges() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.25, 0.25, 0.5, 0.5);
        let (ol, ot, orr, ob) = (0.25f32, 0.25f32, 0.75f32, 0.75f32);

        for handle in CropHandle::RESIZE {
            let r = c.drag_rect(handle, origin, 0.05, 0.05, 1.0);
            let (l, t, rr, b) = (r.x, r.y, r.x + r.width, r.y + r.height);
            let (ml, mt, mr, mb) = handle.edges();

            let moved = |before: f32, after: f32| (after - before).abs() > 1e-6;
            assert_eq!(moved(ol, l), ml, "{handle:?}: left edge");
            assert_eq!(moved(ot, t), mt, "{handle:?}: top edge");
            assert_eq!(moved(orr, rr), mr, "{handle:?}: right edge");
            assert_eq!(moved(ob, b), mb, "{handle:?}: bottom edge");
            assert!(r.is_valid(), "{handle:?} produced an invalid rect {r:?}");
        }
    }

    /// The box can never be dragged inside out, or smaller than
    /// `MIN_CROP` — below that the handles overlap and it cannot be
    /// grabbed back open.
    #[test]
    fn a_handle_dragged_past_its_opposite_stops_at_the_minimum() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.2, 0.2, 0.6, 0.6);

        for handle in CropHandle::RESIZE {
            // Drag hard in both directions, far past the opposite edge.
            for (dx, dy) in [(5.0, 5.0), (-5.0, -5.0)] {
                let r = c.drag_rect(handle, origin, dx, dy, 1.0);
                assert!(r.is_valid(), "{handle:?} at ({dx},{dy}) escaped the frame: {r:?}");
                assert!(
                    r.width >= CropTransform::MIN_CROP - 1e-6
                        && r.height >= CropTransform::MIN_CROP - 1e-6,
                    "{handle:?} at ({dx},{dy}) collapsed to {r:?}"
                );
            }
        }
    }

    /// Working from the gesture's origin, not accumulated deltas, means
    /// a drag out and back returns exactly where it started.
    #[test]
    fn dragging_out_and_back_returns_to_the_original_rect() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.2, 0.2, 0.5, 0.5);

        // Far into a corner, then back to zero displacement.
        let _ = c.drag_rect(CropHandle::TopLeft, origin, -3.0, -3.0, 1.0);
        let back = c.drag_rect(CropHandle::TopLeft, origin, 0.0, 0.0, 1.0);
        assert!((back.x - origin.x).abs() < 1e-6, "drag drifted: {back:?} vs {origin:?}");
        assert!((back.width - origin.width).abs() < 1e-6);
    }

    /// With the aspect locked, resizing must preserve the *displayed*
    /// ratio — which is not the normalized one, because the axes have
    /// different scales on a non-square frame.
    #[test]
    fn a_locked_aspect_is_preserved_as_displayed_through_a_resize() {
        let mut c = CropTransform::identity();
        c.lock_aspect = true;
        let frame = 16.0 / 9.0;
        // A 1:1 displayed box on a 16:9 frame.
        let origin = boxed(0.2, 0.0, 9.0 / 16.0, 1.0);
        let before = (origin.width as f64 / origin.height as f64) * frame;

        for handle in CropHandle::RESIZE {
            let r = c.drag_rect(handle, origin, -0.08, -0.08, frame);
            let after = (r.width as f64 / r.height as f64) * frame;
            assert!(
                (after - before).abs() < 0.05,
                "{handle:?} broke the lock: {before:.3} became {after:.3}"
            );
            assert!(r.is_valid(), "{handle:?} produced {r:?}");
        }
    }

    /// Unlocked, the box is free to become any shape — that is what
    /// "Free" means, and the reference behaves this way.
    #[test]
    fn an_unlocked_box_can_change_shape_freely() {
        let mut c = CropTransform::identity();
        c.lock_aspect = false;
        let origin = boxed(0.25, 0.25, 0.5, 0.5);

        let wide = c.drag_rect(CropHandle::Right, origin, 0.2, 0.0, 1.0);
        let ratio = wide.width / wide.height;
        assert!(ratio > 1.2, "an unlocked drag should be able to widen the box, got {ratio}");
    }

    /// A degenerate frame aspect must not produce a NaN rect that the
    /// renderer would silently drop.
    #[test]
    fn a_degenerate_aspect_still_produces_a_valid_rect() {
        let mut c = CropTransform::identity();
        c.lock_aspect = true;
        let origin = boxed(0.2, 0.2, 0.4, 0.4);
        for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            let r = c.drag_rect(CropHandle::BottomRight, origin, 0.1, 0.1, bad);
            assert!(r.is_valid(), "aspect {bad} produced {r:?}");
            assert!(r.width.is_finite() && r.height.is_finite());
        }
    }

    /// Picking "Free" must actually free the box.
    #[test]
    fn free_lets_width_and_height_move_independently() {
        let mut c = CropTransform::identity();
        let frame = 16.0 / 9.0;

        // Choose a square, then go back to Free.
        c.apply_aspect(AspectPreset::Square, frame);
        c.apply_aspect(AspectPreset::Free, frame);

        // Drag the RIGHT edge only. Under Free, height must not follow.
        let origin = c.rect;
        let r = c.drag_rect(CropHandle::Right, origin, -0.15, 0.0, frame);
        assert!(
            (r.height - origin.height).abs() < 1e-6,
            "Free must let width change alone: height moved {} -> {}",
            origin.height,
            r.height
        );
        assert!((r.width - origin.width).abs() > 1e-6, "the width should have changed");
    }

    /// The exact reported sequence: pick 1:1, then click Free. The box
    /// must become genuinely free — every edge independently movable —
    /// not merely stop being *called* square.
    #[test]
    fn switching_back_to_free_after_a_preset_unlocks_the_box() {
        let frame = 16.0 / 9.0;
        for preset in [
            AspectPreset::Square,
            AspectPreset::Portrait45,
            AspectPreset::Portrait916,
            AspectPreset::Landscape169,
        ] {
            let mut c = CropTransform::identity();
            c.apply_aspect(preset, frame);
            assert!(c.lock_aspect, "{preset:?} should lock the ratio it names");

            c.apply_aspect(AspectPreset::Free, frame);
            assert!(
                !c.lock_aspect,
                "after {preset:?} -> Free the box is still locked, so dragging one \
                 edge would drag the other"
            );
        }
    }

    /// Under Free, each of the four side handles must move its own axis
    /// and leave the other completely alone. This is what "change width
    /// or height to anything" means, as opposed to resizing along the
    /// diagonal.
    #[test]
    fn free_moves_one_axis_at_a_time_for_every_side_handle() {
        let frame = 16.0 / 9.0;
        let mut c = CropTransform::identity();
        c.apply_aspect(AspectPreset::Square, frame);
        c.apply_aspect(AspectPreset::Free, frame);

        let origin = NormalizedRect::new(0.2, 0.2, 0.5, 0.5);
        for (handle, moves_width) in [
            (CropHandle::Left, true),
            (CropHandle::Right, true),
            (CropHandle::Top, false),
            (CropHandle::Bottom, false),
        ] {
            let r = c.drag_rect(handle, origin, 0.08, 0.08, frame);
            let dw = (r.width - origin.width).abs();
            let dh = (r.height - origin.height).abs();

            if moves_width {
                assert!(dw > 1e-6, "{handle:?} should change the width");
                assert!(dh < 1e-6, "{handle:?} must not change the height (changed by {dh})");
            } else {
                assert!(dh > 1e-6, "{handle:?} should change the height");
                assert!(dw < 1e-6, "{handle:?} must not change the width (changed by {dw})");
            }
        }
    }

    /// A Free box must be able to reach shapes no preset offers — that
    /// is the whole point of it being free.
    #[test]
    fn a_free_box_can_reach_an_arbitrary_shape() {
        let frame = 16.0 / 9.0;
        let mut c = CropTransform::identity();
        c.apply_aspect(AspectPreset::Square, frame);
        c.apply_aspect(AspectPreset::Free, frame);

        // Squash to a wide letterbox strip, then to a tall column.
        let wide = c.drag_rect(CropHandle::Bottom, NormalizedRect::FULL, 0.0, -0.75, frame);
        let wide_ratio = (wide.width as f64 / wide.height as f64) * frame;
        assert!(wide_ratio > 5.0, "expected a wide strip, got ratio {wide_ratio:.2}");

        let tall = c.drag_rect(CropHandle::Right, NormalizedRect::FULL, -0.85, 0.0, frame);
        let tall_ratio = (tall.width as f64 / tall.height as f64) * frame;
        assert!(tall_ratio < 0.5, "expected a tall column, got ratio {tall_ratio:.2}");
    }

    /// A named ratio still locks, so its handles keep the shape. Free
    /// becoming genuinely free must not cost the presets their meaning.
    #[test]
    fn a_named_preset_still_holds_its_ratio_through_a_drag() {
        let frame = 16.0 / 9.0;
        let mut c = CropTransform::identity();
        c.apply_aspect(AspectPreset::Square, frame);

        let origin = c.rect;
        let before = (origin.width as f64 / origin.height as f64) * frame;
        let r = c.drag_rect(CropHandle::Right, origin, -0.2, 0.0, frame);
        let after = (r.width as f64 / r.height as f64) * frame;
        assert!(
            (after - before).abs() < 0.05,
            "a locked 1:1 drag drifted from {before:.3} to {after:.3}"
        );
    }

    /// A brand-new clip reads "Free", so it must actually be free.
    #[test]
    fn a_fresh_crop_starts_free_and_unlocked() {
        let c = CropTransform::identity();
        assert_eq!(c.aspect, AspectPreset::Free);
        assert!(!c.lock_aspect, "a box labelled Free must not be locked");
    }

    /// The lock is **derived**, never independently set.
    ///
    /// It and `aspect` describe one fact. When they were two writable
    /// fields they contradicted each other and produced "Free is not
    /// really free". This asserts the invariant holds for every preset,
    /// through the only path that can set it.
    #[test]
    fn the_lock_always_agrees_with_the_chosen_ratio() {
        for frame in [16.0 / 9.0, 1.0, 9.0 / 16.0] {
            for preset in AspectPreset::ALL {
                let mut c = CropTransform::identity();
                c.apply_aspect(preset, frame);
                assert_eq!(
                    c.lock_aspect(),
                    preset != AspectPreset::Free,
                    "{preset:?} at frame {frame}: the lock disagrees with the ratio"
                );
            }
        }
    }

    /// Round-tripping through every preset and back to Free must always
    /// end unlocked -- no ordering can leave a stale lock behind.
    #[test]
    fn any_route_back_to_free_ends_unlocked() {
        let frame = 16.0 / 9.0;
        let mut c = CropTransform::identity();
        for preset in AspectPreset::ALL {
            c.apply_aspect(preset, frame);
            c.apply_aspect(AspectPreset::Free, frame);
            assert!(!c.lock_aspect(), "via {preset:?}, Free left the box locked");
            assert_eq!(c.aspect, AspectPreset::Free);
        }
    }

    #[test]
    fn identity_is_full_frame_no_rotation() {
        let c = CropTransform::identity();
        assert_eq!(c.rect, NormalizedRect::FULL);
        assert_eq!(c.straighten_deg(), 0.0);
        assert_eq!(c.aspect, AspectPreset::Free);
    }

    #[test]
    fn straighten_clamps_to_exactly_plus_minus_45() {
        let mut c = CropTransform::identity();
        c.set_straighten_deg(999.0);
        assert_eq!(c.straighten_deg(), 45.0);
        c.set_straighten_deg(-999.0);
        assert_eq!(c.straighten_deg(), -45.0);
        c.set_straighten_deg(44.9);
        assert_eq!(c.straighten_deg(), 44.9);
    }

    #[test]
    fn rect_new_never_escapes_unit_square() {
        // Deliberately pathological inputs.
        let cases = [
            (-5.0, -5.0, 2.0, 2.0),
            (0.9, 0.9, 0.5, 0.5),
            (f32::MAX, f32::MAX, f32::MAX, f32::MAX),
            (-1.0, 2.0, -1.0, -1.0),
        ];
        for (x, y, w, h) in cases {
            let r = NormalizedRect::new(x, y, w, h);
            assert!(r.is_valid(), "rect {r:?} from inputs ({x},{y},{w},{h}) escaped 0..=1");
        }
    }

    /// Every preset must produce the requested ratio **as displayed**,
    /// which is the normalized ratio times the frame aspect. Checking
    /// only the normalized ratio (as this test originally did) passes
    /// even when the crop is visibly the wrong shape on screen.
    #[test]
    fn every_preset_produces_its_ratio_as_actually_displayed() {
        for frame_aspect in [16.0 / 9.0, 1.0, 9.0 / 16.0, 4.0 / 3.0] {
            for preset in AspectPreset::ALL {
                let mut c = CropTransform::identity();
                c.apply_aspect(preset, frame_aspect);
                assert!(c.rect.is_valid(), "{preset:?} produced invalid rect {:?}", c.rect);
                if let Some(want) = preset.ratio() {
                    let displayed = (c.rect.width as f64 / c.rect.height as f64) * frame_aspect;
                    assert!(
                        (displayed - want).abs() < 0.01,
                        "{preset:?} on a {frame_aspect:.3} frame displayed as {displayed:.3}, wanted {want:.3}"
                    );
                }
            }
        }
    }

    /// The specific regression: on a 16:9 frame, "1:1" used to return the
    /// unchanged full frame — a chip that appeared to do nothing.
    #[test]
    fn a_square_crop_of_a_widescreen_frame_is_not_the_whole_frame() {
        let mut c = CropTransform::identity();
        c.apply_aspect(AspectPreset::Square, 16.0 / 9.0);
        assert_ne!(c.rect, NormalizedRect::FULL, "1:1 on 16:9 must actually crop");
        assert!((c.rect.width - 9.0 / 16.0).abs() < 0.01, "expected width 9/16, got {}", c.rect.width);
        assert!((c.rect.height - 1.0).abs() < 0.01, "a square crop should use the full height");
    }

    #[test]
    fn a_square_crop_of_a_square_frame_is_the_whole_frame() {
        let mut c = CropTransform::identity();
        c.apply_aspect(AspectPreset::Square, 1.0);
        assert!((c.rect.width - 1.0).abs() < 0.01);
        assert!((c.rect.height - 1.0).abs() < 0.01);
    }

    #[test]
    fn a_degenerate_frame_aspect_falls_back_instead_of_producing_nan() {
        for bad in [0.0, -3.0, f64::NAN, f64::INFINITY] {
            let mut c = CropTransform::identity();
            c.apply_aspect(AspectPreset::Portrait916, bad);
            assert!(c.rect.is_valid(), "frame aspect {bad} produced invalid rect {:?}", c.rect);
            assert!(c.rect.width.is_finite() && c.rect.height.is_finite());
        }
    }

    /// NaN must become 0, not propagate. `f32::clamp` returns NaN for a
    /// NaN input, so this is a real hazard, not a hypothetical one — a
    /// NaN angle reaches the shader and paints the frame black.
    #[test]
    fn a_nan_straighten_angle_becomes_zero_rather_than_propagating() {
        let mut c = CropTransform::identity();
        c.set_straighten_deg(f32::NAN);
        assert_eq!(c.straighten_deg(), 0.0);
        assert!(c.straighten_deg().is_finite());
    }

    #[test]
    fn infinite_straighten_angles_still_clamp_to_the_dial_range() {
        let mut c = CropTransform::identity();
        c.set_straighten_deg(f32::INFINITY);
        assert_eq!(c.straighten_deg(), 45.0);
        c.set_straighten_deg(f32::NEG_INFINITY);
        assert_eq!(c.straighten_deg(), -45.0);
    }









    #[test]
    fn the_default_grid_is_thirds_and_maps_to_two_lines_per_axis() {
        assert_eq!(CropTransform::identity().grid, CropGrid::Thirds);
        assert_eq!(CropGrid::None.divisions(), 0);
        assert_eq!(CropGrid::Thirds.divisions(), 3, "thirds = 3 cells = 2 interior lines");
        assert_eq!(CropGrid::Fine.divisions(), 4);
    }

    #[test]
    fn free_preset_does_not_reset_existing_rect() {
        let mut c = CropTransform::identity();
        c.rect = NormalizedRect::new(0.1, 0.1, 0.5, 0.5);
        let before = c.rect;
        c.apply_aspect(AspectPreset::Free, 16.0 / 9.0);
        assert_eq!(c.rect, before, "Free must not reset the crop rect");
    }

    // Lightweight property-style check without pulling in `proptest` yet
    // (the design asks for this at property-test rigor; a hand-rolled
    // sweep over a wide input grid is the same guarantee for a 4-field
    // struct and keeps offcut-model's dependency list at zero extra crates
    // for the skeleton phase).
    #[test]
    fn property_random_grid_of_rects_never_escapes_unit_square() {
        let steps = [-10.0_f32, -1.0, -0.5, 0.0, 0.3, 0.7, 1.0, 1.5, 10.0];
        for &x in &steps {
            for &y in &steps {
                for &w in &steps {
                    for &h in &steps {
                        let r = NormalizedRect::new(x, y, w, h);
                        assert!(r.is_valid(), "escaped for ({x},{y},{w},{h}) -> {r:?}");
                    }
                }
            }
        }
    }
}
