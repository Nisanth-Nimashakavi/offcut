//! The video-preview widget: wraps `offcut_render::VideoPrimitive` as an
//! `iced::widget::shader::Program` so it can sit directly inside the
//! Shell's stage area (`shell.rs`). This is the piece that makes
//! the "one shared render pass" claim visible on screen, not
//! just true in the dependency graph.
//!
//! It is also where the **interactive crop box** lives. `shader::Program`
//! has `update`/`mouse_interaction` hooks exactly like `canvas::Program`,
//! so the overlay can be dragged without adding a second widget — which
//! matters here, because this crate has already been bitten once by
//! sibling canvases refusing to compose. Drawing the
//! box in the same fragment shader that draws the frame sidesteps that
//! entirely: there is only ever one surface.

use iced::widget::shader;
use iced::{Element, Event, Length, Point, Rectangle, mouse};
use std::sync::Arc;
use offcut_engine::Frame;
use offcut_model::{CropHandle, NormalizedRect};
use offcut_render::{EffectsUniform, VideoPrimitive};

/// Half-size of a crop handle's grab zone, in **output pixels**.
///
/// Larger than the ~9px disc the shader draws, for the reason every hit
/// target in this codebase is: the visual affordance and the grab zone
/// are allowed to differ, and a disc you must hit exactly is miserable
/// with a mouse and impossible on a trackpad.
const HANDLE_GRAB: f32 = 14.0;

/// What the pointer is doing to the crop box right now.
#[derive(Debug, Clone, Copy, Default)]
pub struct CropDrag {
    pub handle: CropHandle,
    /// Pointer position when the drag began, in output pixels.
    pub origin_pointer: Option<Point>,
    /// The rect as it was when the drag began.
    ///
    /// Every event resolves against **this**, never against the previous
    /// event's result. Accumulating per-event deltas loses the remainder
    /// each time a clamp fires, so a box dragged into a corner and back
    /// would not return to where it started.
    pub origin_rect: NormalizedRect,
}

/// Messages the preview emits.
#[derive(Debug, Clone, PartialEq)]
pub enum VideoMessage {
    /// The crop rect changed through a drag.
    CropChanged(NormalizedRect),
    /// A drag began — the shell takes one undo checkpoint here, so the
    /// whole gesture undoes in a single step rather than per pixel.
    CropGestureBegan,
    CropGestureEnded,
}

/// A thin `shader::Program`: all the actual GPU work lives in
/// `offcut_render::VideoPrimitive`/`VideoPipeline` (kept in `offcut-render`
/// so that crate stays the single owner of every wgpu resource, per
/// The design rule).
#[derive(Debug, Clone)]
pub struct VideoWidget {
    pub frame: Option<Arc<Frame>>,
    /// The selected clip's crop + adjust state, already in shader form.
    pub effects: EffectsUniform,
    /// The crop rect in normalized coordinates, and whether it is
    /// editable right now. `None` outside the Crop tab: the box is an
    /// editing affordance, not part of the picture.
    pub crop: Option<NormalizedRect>,
    /// Whether the aspect lock is on, needed by the drag maths.
    pub lock_aspect: bool,
    /// The source frame's display aspect, likewise.
    pub frame_aspect: f32,
}

impl Default for VideoWidget {
    fn default() -> Self {
        Self {
            frame: None,
            effects: EffectsUniform::identity(1.0),
            crop: None,
            lock_aspect: true,
            frame_aspect: 16.0 / 9.0,
        }
    }
}

impl VideoWidget {
    /// The display aspect of what is actually on screen.
    ///
    /// While cropping, the preview shows the **whole frame**, so the
    /// shape to preserve is the source's. Otherwise it shows the cropped
    /// region, whose shape is the source aspect scaled by the rect's own
    /// proportions.
    fn displayed_aspect(&self) -> f32 {
        if self.crop.is_some() {
            return self.frame_aspect;
        }
        let c = self.effects.crop;
        if c[3] > 0.0 {
            self.frame_aspect * c[2] / c[3]
        } else {
            self.frame_aspect
        }
    }

    /// The sub-rectangle of `bounds` the picture actually occupies once
    /// fitted, in widget pixels.
    ///
    /// This is the same computation `EffectsUniform::fit_to_viewport`
    /// does for the vertex shader, expressed in pixels instead of clip
    /// space. Both must agree or the drawn box and its grab zones sit in
    /// different places.
    fn picture_rect(&self, bounds: Rectangle) -> Rectangle {
        let content = self.displayed_aspect().max(0.0001);
        let widget = (bounds.width / bounds.height.max(0.0001)).max(0.0001);

        let (w, h) = if content > widget {
            (bounds.width, bounds.width / content)
        } else {
            (bounds.height * content, bounds.height)
        };
        Rectangle {
            x: bounds.x + (bounds.width - w) / 2.0,
            y: bounds.y + (bounds.height - h) / 2.0,
            width: w,
            height: h,
        }
    }

    /// Which part of the crop box is at `point`, in output pixels.
    ///
    /// Corners are tested before edges and edges before the interior, so
    /// the smaller, more specific target always wins where they overlap.
    /// Without that ordering a corner would be unreachable: it lies
    /// inside both adjacent edges' zones and inside the box itself.
    fn handle_at(&self, point: Point, bounds: Rectangle) -> CropHandle {
        let Some(rect) = self.crop else { return CropHandle::None };
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return CropHandle::None;
        }
        // Hit-test against the **fitted picture**, not the whole widget.
        // The image is letterboxed inside the stage, so a box addressed
        // in widget coordinates would put its handles out on the black
        // margins -- visibly beside the picture they claim to bound.
        let pic = self.picture_rect(bounds);

        // # The letterbox offset, which this used to drop on the floor
        //
        // `picture_rect` returns a rect in **widget-absolute** coordinates
        // -- origin included -- while `point` arrives from
        // `cursor.position_in(bounds)` and is relative to the widget. The
        // scaling below used the picture's width and height and silently
        // ignored its x and y, so every handle was hit-tested as though
        // the picture began at the widget's top-left corner.
        //
        // The error is exactly the letterbox margin. A 16:9 source in a
        // 1000x400 stage is pillarboxed by 144px on each side, so the grab
        // zones sat 144px to the left of the discs they belonged to: the
        // left handles were unreachable off the edge of the picture, and
        // pressing a visible handle hit nothing at all.
        //
        // With a ratio preset the chips still resized the box, so the
        // damage was invisible there. **Free has no chips** -- dragging a
        // handle is its only way to change the box -- so the whole preset
        // read as "Free does not change sizes".
        let origin_x = pic.x - bounds.x;
        let origin_y = pic.y - bounds.y;

        // Mirrors the shader's handle inset: at the frame edge the discs
        // are pulled inward so they are not half-clipped, and the grab
        // zone has to follow or the visible handle and its hit target
        // would sit in different places -- the classic "the button does
        // not work where it looks like it is" defect.
        let inset_px = HANDLE_GRAB * 0.45;
        let to_px = |fx: f32, fy: f32| {
            let x = (rect.x + rect.width * fx) * pic.width;
            let y = (rect.y + rect.height * fy) * pic.height;
            Point::new(
                origin_x + x.clamp(inset_px, (pic.width - inset_px).max(inset_px)),
                origin_y + y.clamp(inset_px, (pic.height - inset_px).max(inset_px)),
            )
        };

        for handle in CropHandle::RESIZE {
            let (fx, fy) = handle.anchor();
            let at = to_px(fx, fy);
            if (point.x - at.x).abs() <= HANDLE_GRAB && (point.y - at.y).abs() <= HANDLE_GRAB {
                return handle;
            }
        }

        let l = origin_x + rect.x * pic.width;
        let t = origin_y + rect.y * pic.height;
        let r = origin_x + (rect.x + rect.width) * pic.width;
        let b = origin_y + (rect.y + rect.height) * pic.height;
        if point.x >= l && point.x <= r && point.y >= t && point.y <= b {
            return CropHandle::Move;
        }

        CropHandle::None
    }
}

impl shader::Program<VideoMessage> for VideoWidget {
    type State = CropDrag;
    type Primitive = VideoPrimitive;

    fn update(
        &self,
        state: &mut Self::State,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<shader::Action<VideoMessage>> {
        // # A latched drag has to be released, even when the box is gone
        //
        // The early return here used to bail out *before* the release
        // handler, so leaving the Crop tab mid-drag (or any lost
        // button-up, e.g. the pointer leaving the window) left
        // `state.handle` set forever. A latched drag captures every
        // subsequent event, so the ratio chips stopped responding: the
        // shell never saw the clicks at all. That is the reported
        // "unable to change Free after leaving the tab and coming back".
        //
        // Releasing the button always clears the drag, whether or not
        // there is still a box to drag.

        // No box, nothing to drag. Any stale drag state belongs to a
        // gesture whose box has since disappeared -- clear it rather
        // than leaving it to capture events for a control that is no
        // longer on screen.
        if matches!(event, Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))) {
            if state.handle == CropHandle::None {
                return None;
            }
            *state = CropDrag::default();
            return Some(shader::Action::publish(VideoMessage::CropGestureEnded).and_capture());
        }

        if self.crop.is_none() {
            *state = CropDrag::default();
            return None;
        }

        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let point = cursor.position_in(bounds)?;
                let handle = self.handle_at(point, bounds);
                if handle == CropHandle::None {
                    return None;
                }
                state.handle = handle;
                state.origin_pointer = Some(point);
                state.origin_rect = self.crop?;
                Some(shader::Action::publish(VideoMessage::CropGestureBegan).and_capture())
            }

            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.handle == CropHandle::None {
                    return None;
                }
                // The raw position, not `position_in`: a drag that
                // wanders outside the stage must keep tracking, or the
                // box freezes the moment the pointer leaves the viewport.
                // A drag with no reportable pointer position has lost
                // its button-up somewhere (the pointer left the window,
                // or a grab was broken). Ending it here is what stops it
                // latching and swallowing later clicks.
                let (Some(absolute), Some(origin)) = (cursor.position(), state.origin_pointer)
                else {
                    *state = CropDrag::default();
                    return Some(
                        shader::Action::publish(VideoMessage::CropGestureEnded).and_capture(),
                    );
                };
                // Normalized against the PICTURE's size: a drag of N
                // pixels means a different fraction of the image than of
                // the widget once letterboxing is in play.
                let picture = self.picture_rect(bounds);
                let dx = (absolute.x - bounds.x - origin.x) / picture.width.max(1.0);
                let dy = (absolute.y - bounds.y - origin.y) / picture.height.max(1.0);

                let rect = offcut_model::CropTransform::drag_rect_with(
                    self.lock_aspect,
                    state.handle,
                    state.origin_rect,
                    dx,
                    dy,
                    f64::from(self.frame_aspect),
                );
                Some(shader::Action::publish(VideoMessage::CropChanged(rect)).and_capture())
            }

            _ => None,
        }
    }

    fn draw(&self, _state: &Self::State, _cursor: mouse::Cursor, bounds: Rectangle) -> Self::Primitive {
        // Fit the picture to the widget it is actually being drawn into.
        // `bounds` is only available here, which is why the shell cannot
        // precompute it: the stage's shape depends on the window, and a
        // uniform built without it stretches the image to fill whatever
        // rectangle it lands in.
        let mut effects = self.effects;
        effects.fit_to_viewport(self.displayed_aspect(), (bounds.width, bounds.height));
        VideoPrimitive { frame: self.frame.clone(), effects }
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        let active = if state.handle != CropHandle::None {
            state.handle
        } else {
            match cursor.position_in(bounds) {
                Some(p) => self.handle_at(p, bounds),
                None => CropHandle::None,
            }
        };
        crop_cursor(active)
    }
}

/// The pointer shape for a crop handle.
///
/// Diagonal resize cursors are not in iced's set, so corners use the
/// nearest honest thing rather than a misleading one: `Grab` says
/// "this point is draggable" without claiming an axis it cannot show.
fn crop_cursor(handle: CropHandle) -> mouse::Interaction {
    use CropHandle::*;
    match handle {
        Left | Right => mouse::Interaction::ResizingHorizontally,
        Top | Bottom => mouse::Interaction::ResizingVertically,
        TopLeft | TopRight | BottomLeft | BottomRight => mouse::Interaction::Grab,
        Move => mouse::Interaction::Grab,
        None => mouse::Interaction::default(),
    }
}

/// Build the actual widget element, filling its container.
#[allow(clippy::too_many_arguments)]
pub fn video_preview<'a>(
    frame: Option<Arc<Frame>>,
    effects: EffectsUniform,
    crop: Option<NormalizedRect>,
    lock_aspect: bool,
    frame_aspect: f32,
) -> Element<'a, VideoMessage> {
    shader(VideoWidget { frame, effects, crop, lock_aspect, frame_aspect })
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn widget(rect: NormalizedRect) -> VideoWidget {
        VideoWidget { crop: Some(rect), ..Default::default() }
    }

    const B: Rectangle = Rectangle { x: 0.0, y: 0.0, width: 800.0, height: 450.0 };

    use iced::widget::shader::Program as _;

    fn press(at: Point) -> Event {
        let _ = at;
        Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
    }

    fn release() -> Event {
        Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
    }

    /// A drag interrupted by the crop box disappearing (leaving the Crop
    /// tab) must not stay latched.
    ///
    /// This is the reported bug. `update` returned early when there was
    /// no box, *before* the release handler, so a drag begun on the Crop
    /// tab and released elsewhere left `state.handle` set forever. A
    /// latched drag captures every later event, so the ratio chips
    /// stopped responding — clicking "Free" did nothing because the shell
    /// never received the click.
    #[test]
    fn a_drag_does_not_stay_latched_when_the_crop_box_disappears() {
        let rect = NormalizedRect::new(0.25, 0.25, 0.5, 0.5);
        let with_box = widget(rect);
        let mut state = CropDrag::default();
        let cursor = mouse::Cursor::Available(Point::new(
            (rect.x * B.width) + B.x,
            (rect.y * B.height) + B.y,
        ));

        // Grab the top-left handle.
        let _ = with_box.update(&mut state, &press(Point::ORIGIN), B, cursor);
        assert_ne!(state.handle, CropHandle::None, "the press should have grabbed a handle");

        // The user leaves the Crop tab: the widget now has no box.
        let without_box = VideoWidget::default();
        let _ = without_box.update(&mut state, &release(), B, cursor);

        assert_eq!(
            state.handle,
            CropHandle::None,
            "the drag stayed latched after the box disappeared, so every later \
             click is captured and the ratio chips stop working"
        );
    }

    /// Even without an explicit release, a widget with no box must clear
    /// stale drag state rather than carry it forever.
    #[test]
    fn a_widget_with_no_box_clears_stale_drag_state() {
        let mut state = CropDrag {
            handle: CropHandle::BottomRight,
            origin_pointer: Some(Point::new(10.0, 10.0)),
            origin_rect: NormalizedRect::FULL,
        };
        let inert = VideoWidget::default();
        let cursor = mouse::Cursor::Available(Point::new(50.0, 50.0));

        let action = inert.update(&mut state, &Event::Mouse(mouse::Event::CursorMoved {
            position: Point::new(50.0, 50.0),
        }), B, cursor);

        assert!(action.is_none(), "an inert preview must not capture events");
        assert_eq!(state.handle, CropHandle::None, "stale drag state was kept");
    }

    /// The ordinary path still works: press, drag, release.
    #[test]
    fn a_normal_drag_still_begins_and_ends_cleanly() {
        let rect = NormalizedRect::new(0.25, 0.25, 0.5, 0.5);
        let w = widget(rect);
        let mut state = CropDrag::default();
        let cursor = mouse::Cursor::Available(Point::new(
            (rect.x * B.width) + B.x,
            (rect.y * B.height) + B.y,
        ));

        let began = w.update(&mut state, &press(Point::ORIGIN), B, cursor);
        assert!(began.is_some(), "a press on a handle should start a gesture");
        assert_ne!(state.handle, CropHandle::None);

        let ended = w.update(&mut state, &release(), B, cursor);
        assert!(ended.is_some(), "a release should end the gesture");
        assert_eq!(state.handle, CropHandle::None);
    }

    /// Every one of the eight handles must be reachable at its own
    /// anchor. A mask or anchor typo shows up on exactly one of them,
    /// so all eight are checked rather than a sample.
    #[test]
    fn every_crop_handle_is_grabbable_at_its_anchor() {
        let w = widget(NormalizedRect::new(0.25, 0.25, 0.5, 0.5));
        for handle in CropHandle::RESIZE {
            let (fx, fy) = handle.anchor();
            let rect = w.crop.unwrap();
            let p = Point::new(
                (rect.x + rect.width * fx) * B.width,
                (rect.y + rect.height * fy) * B.height,
            );
            assert_eq!(w.handle_at(p, B), handle, "{handle:?} was not grabbable at its anchor");
        }
    }

    /// The same eight handles, on a stage that **letterboxes** the
    /// picture.
    ///
    /// # Why this is a separate test from the one above
    ///
    /// `B` is 800×450 — exactly 16:9, exactly `frame_aspect`'s default.
    /// The picture fills the widget edge to edge, the letterbox margin is
    /// zero, and a hit test that ignores the margin entirely is
    /// indistinguishable from a correct one. Every crop test in this file
    /// shared that fixture, so all eight handles passed while the widget
    /// was hit-testing them in the wrong place.
    ///
    /// `handle_at` scaled by `picture_rect`'s width and height and threw
    /// away its x and y. A 16:9 source in a 1000×400 stage is pillarboxed
    /// by 144px, so every grab zone sat 144px left of the disc it
    /// belonged to. With a ratio preset the chips still resized the box
    /// and hid the damage; **Free has no chips**, so its only means of
    /// resizing was the broken one — reported as "Free doesn't change
    /// sizes".
    #[test]
    fn every_handle_is_grabbable_when_the_picture_is_letterboxed() {
        // Wider than 16:9, so the picture is pillarboxed inside it.
        const WIDE: Rectangle = Rectangle { x: 0.0, y: 0.0, width: 1000.0, height: 400.0 };
        let w = widget(NormalizedRect::new(0.25, 0.25, 0.5, 0.5));
        let pic = w.picture_rect(WIDE);
        assert!(pic.x > 100.0, "fixture must actually letterbox, got x={}", pic.x);

        for handle in CropHandle::RESIZE {
            let (fx, fy) = handle.anchor();
            let rect = w.crop.unwrap();
            // The point where the handle is genuinely *drawn*: inside the
            // fitted picture, offset by the letterbox margin.
            let p = Point::new(
                (pic.x - WIDE.x) + (rect.x + rect.width * fx) * pic.width,
                (pic.y - WIDE.y) + (rect.y + rect.height * fy) * pic.height,
            );
            assert_eq!(
                w.handle_at(p, WIDE),
                handle,
                "{handle:?} is not grabbable where it is drawn on a letterboxed stage"
            );
        }
    }

    /// Taller than the source, so the picture letterboxes on the other
    /// axis: the y offset has to be honoured too, not just x.
    #[test]
    fn the_interior_is_grabbable_when_the_picture_is_letterboxed_vertically() {
        const TALL: Rectangle = Rectangle { x: 0.0, y: 0.0, width: 600.0, height: 700.0 };
        let w = widget(NormalizedRect::new(0.25, 0.25, 0.5, 0.5));
        let pic = w.picture_rect(TALL);
        assert!(pic.y > 100.0, "fixture must letterbox vertically, got y={}", pic.y);

        let centre = Point::new(
            (pic.x - TALL.x) + 0.5 * pic.width,
            (pic.y - TALL.y) + 0.5 * pic.height,
        );
        assert_eq!(w.handle_at(centre, TALL), CropHandle::Move);

        // Above the picture is black surround, not the box.
        let above = Point::new(centre.x, (pic.y - TALL.y) - 20.0);
        assert_eq!(w.handle_at(above, TALL), CropHandle::None);
    }

    /// A corner lies inside both adjacent edges' zones and inside the
    /// box. It must still win, or corners are unreachable — the most
    /// common resize gesture there is.
    #[test]
    fn corners_win_over_edges_and_the_interior() {
        let rect = NormalizedRect::new(0.2, 0.2, 0.6, 0.6);
        let w = widget(rect);
        let corner = Point::new(rect.x * B.width, rect.y * B.height);
        assert_eq!(w.handle_at(corner, B), CropHandle::TopLeft);
    }

    /// The middle of the box moves it; outside it is not the box at all.
    #[test]
    fn the_interior_moves_and_the_outside_is_untouched() {
        let rect = NormalizedRect::new(0.25, 0.25, 0.5, 0.5);
        let w = widget(rect);
        assert_eq!(w.handle_at(Point::new(400.0, 225.0), B), CropHandle::Move);
        assert_eq!(w.handle_at(Point::new(10.0, 10.0), B), CropHandle::None);
    }

    /// With no crop box (any tab but Crop) the preview is inert, so
    /// clicks reach whatever is beneath it.
    #[test]
    fn without_a_crop_box_nothing_is_grabbable() {
        let w = VideoWidget::default();
        assert_eq!(w.handle_at(Point::new(400.0, 225.0), B), CropHandle::None);
    }

    /// A full-frame crop — the default state of every clip, and so the
    /// first thing anyone tries to drag — must still have grabbable
    /// handles. Untouched, they sit exactly on the viewport boundary
    /// where half of each disc is clipped away.
    #[test]
    fn a_full_frame_crop_still_has_grabbable_handles() {
        let w = widget(NormalizedRect::FULL);
        for handle in CropHandle::RESIZE {
            let (fx, fy) = handle.anchor();
            let p = Point::new(fx * B.width, fy * B.height);
            assert_eq!(
                w.handle_at(p, B),
                handle,
                "{handle:?} is unreachable on a full-frame crop"
            );
        }
    }

    /// A degenerate viewport must not panic or divide by zero.
    #[test]
    fn a_zero_sized_viewport_is_handled_without_panicking() {
        let w = widget(NormalizedRect::new(0.1, 0.1, 0.5, 0.5));
        let zero = Rectangle { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
        assert_eq!(w.handle_at(Point::new(0.0, 0.0), zero), CropHandle::None);
    }

    /// Each handle advertises a pointer shape, and the interior says it
    /// is draggable — a box that looks inert will not be discovered.
    #[test]
    fn every_handle_advertises_a_cursor() {
        for handle in CropHandle::RESIZE {
            assert_ne!(
                crop_cursor(handle),
                mouse::Interaction::default(),
                "{handle:?} gives no pointer feedback"
            );
        }
        assert_ne!(crop_cursor(CropHandle::Move), mouse::Interaction::default());
        assert_eq!(crop_cursor(CropHandle::None), mouse::Interaction::default());
    }

    /// The grab zone must be more generous than the drawn disc.
    #[test]
    fn the_grab_zone_exceeds_the_drawn_handle() {
        const { assert!(HANDLE_GRAB > 9.0, "grab zone should exceed the ~9px drawn disc") };
    }

}
