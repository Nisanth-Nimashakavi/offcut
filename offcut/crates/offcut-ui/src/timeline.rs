//! The timeline: ruler, clip filmstrip, playhead, audio lane — drawn as
//! one `canvas` widget and interactive with the mouse.
//!
//! # Why one canvas rather than a row of widgets
//!
//! The previous shell built the clip lane from `button`s in a `row`,
//! which made every clip a fixed 160px regardless of its duration. That
//! is not a timeline — it is a list. A timeline's defining property is
//! that horizontal distance *is* time, which means clip width must be
//! `duration × pixels_per_second`, the ruler must agree with it to the
//! pixel, and the playhead must land on the same scale. Three widgets
//! that each compute that mapping separately will disagree; one canvas
//! that computes it once cannot.
//!
//! It also buys the things the design system specifies that a `button` row
//! cannot express: real thumbnails tiled across a clip's width, trim
//! handles straddling the selected clip's edges, a playhead line with a
//! timecode cap riding above it, and a waveform whose muted spans draw no
//! bars at all (the design system: "The bars beneath a muted span are removed,
//! not covered").
//!
//! # Coordinate mapping, in one place
//!
//! `x = (timeline_time - scroll) * pixels_per_second + content_left`
//!
//! `TimelineLayout` owns that formula and its inverse. Every hit test,
//! every drawn rectangle, and the playhead all go through it.

use crate::theme::Palette;
use iced::widget::canvas;
use iced::{Element, Length, Rectangle, Renderer, Theme, mouse};
use offcut_model::{Project, Time};

/// Heights from the design system's Layout section: "Timeline 284 (Ruler 30 +
/// Lane 254)", with the lane split into the playhead cap band, the clip
/// row, and the audio lane.
/// The source trim bar's band, drawn at the very top of this canvas.
///
/// # Why the trim bar lives inside the timeline canvas
///
/// It is a logically separate control and was first built as its own
/// `canvas` widget in a `column` above this one. It rendered *nothing*.
/// This is the sibling-canvas defect this crate already documents (see
/// A known iced quirk: three plain squares in a `column` render as one, the last
/// program's geometry at the first widget's position) — adding a second
/// canvas made the new one invisible and would have broken the timeline
/// too had the order differed.
///
/// So the two controls share one canvas and one `Program`, split into
/// bands. `trimbar.rs` still owns all of the trim bar's geometry, hit
/// testing, and clamping as pure functions; only the drawing call and the
/// event dispatch live here. That keeps the logic unit-testable without a
/// renderer while respecting the one-canvas constraint the toolkit
/// actually imposes.
/// The strip's band height.
///
/// Sized to the control and nothing more: a 30px track, an 11px handle
/// radius straddling its edges, and the playhead's overhang past both.
/// That comes to 46, and the remaining air is the band's own breathing
/// room rather than a number picked to look right.
///
/// It is deliberately *not* filmstrip height. This is a range picker, and
/// giving it the depth of a real timeline would make two controls compete
/// to look like the primary one — see this module's header for why the
/// two exist separately at all.
pub const TRIM_BAR_HEIGHT: f32 = 46.0;
pub const RULER_HEIGHT: f32 = 30.0;
pub const CAP_BAND_HEIGHT: f32 = 30.0;
pub const CLIP_HEIGHT: f32 = 132.0;
pub const CLIP_FOOTER_HEIGHT: f32 = 36.0;
pub const AUDIO_LANE_HEIGHT: f32 = 56.0;
/// The horizontal rail every timeline-space x is measured from.
///
/// Derived from the strip's own inset rather than stated independently.
/// These were two separate literals that happened to agree at 24, and the
/// agreement is load-bearing: the strip's t=0 and the ruler's 0:00 must
/// land on the same pixel, or the control is lying about where the source
/// begins. Deriving one from the other makes that impossible to break by
/// editing a single number — which is how it broke before.
pub const CONTENT_RAIL: f32 = crate::trimbar::EDGE_INSET;
/// The canvas is now exactly the trim bar: the ruler, clip lane, and
/// audio lane it used to stack beneath it are no longer drawn (see the
/// `draw` method for why).
pub const TOTAL_HEIGHT: f32 = TRIM_BAR_HEIGHT;

/// What the user is doing with the mouse right now.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Interaction {
    #[default]
    Idle,
    /// Dragging the playhead (from the ruler, the cap, or empty lane).
    ScrubbingPlayhead,
    /// Dragging the selected clip's left or right trim handle.
    TrimmingStart(usize),
    TrimmingEnd(usize),
}

/// Canvas-local state iced persists between events for us.
#[derive(Default)]
pub struct TimelineState {
    pub interaction: Interaction,
    /// The lane width last reported to the app, so the measurement is
    /// published on change rather than every frame.
    pub reported_width: f32,
    /// Which trim-bar handle is being dragged, and which is under the
    /// pointer. Held here rather than in a separate widget state because
    /// the trim bar shares this canvas — see `TRIM_BAR_HEIGHT`.
    pub trim_grabbed: crate::trimbar::Grabbed,
    pub trim_hovered: crate::trimbar::Grabbed,
    /// How hard the dragged handle is currently pressed against the
    /// playhead, 0..1. Purely presentational — it deforms the handle so
    /// a detented drag still visibly tracks the hand.
    pub trim_contact: f32,
    /// Pointer history and catch latch for the current drag. The stop
    /// needs this to catch a *fast* drag, whose consecutive events can
    /// straddle the playhead without ever landing near it.
    pub trim_drag: crate::trimbar::DragState,
}

/// Messages the timeline emits. Deliberately coarse: the timeline reports
/// *intent* ("the user put the playhead here"), never mutations, so all
/// model changes stay in one `update` in the shell.
#[derive(Debug, Clone, PartialEq)]
pub enum TimelineMessage {
    /// Playhead moved to a timeline instant. `precise` is false while
    /// dragging (use the fast keyframe seek) and true on release (issue
    /// the accurate seek) — the two-tier seek, surfaced as
    /// the one bit of information the engine needs to choose a tier.
    Seek { to: Time, precise: bool },
    SelectClip(usize),
    /// A trim drag in progress. `Time` is the new in- or out-point in
    /// SOURCE time.
    TrimStart { clip: usize, to: Time },
    TrimEnd { clip: usize, to: Time },
    /// A drag gesture began — the shell takes an undo checkpoint here, so
    /// the whole drag undoes as one step rather than a hundred.
    GestureBegan,
    GestureEnded,
    /// The lane's real measured width in logical pixels, reported once it
    /// is known. The app uses this to fit a newly opened file to the
    /// window it actually has, rather than to a hardcoded design width.
    LaneMeasured(f32),
    /// Emitted by the source trim bar sharing this canvas. Forwarded
    /// rather than handled here: the bar reports intent in SOURCE time and
    /// the shell owns every model mutation, exactly as the rest of this
    /// widget does.
    TrimBar(crate::trimbar::TrimBarMessage),
}

/// Everything the timeline draws, gathered by the shell so this widget
/// borrows nothing it does not use.
pub struct TimelineData<'a> {
    pub project: &'a Project,
    pub selected_clip: Option<usize>,
    pub playhead: Time,
    pub palette: Palette,
    /// Pixels per second of timeline. Driven by the zoom slider.
    pub pixels_per_second: f32,
    pub fps: offcut_model::Rational,
}

/// The timeline's retained geometry, owned by the shell and handed to the
/// widget each frame.
///
/// # Why the timeline is drawn in two layers
///
/// Dragging the playhead redraws the timeline on every pointer move. The
/// first version rebuilt *everything* in that pass — the ruler's ticks
/// and labels, every clip's border and footer text, every thumbnail
/// image, and hundreds of individual waveform bars — to move one 2px red
/// line. Text shaping and image tessellation dominate that work, and
/// none of it depends on the playhead.
///
/// So the static content lives in a `Cache` that is only rebuilt when
/// something it actually depends on changes (the clips, the zoom, the
/// selection, the theme, the decoded media), and the playhead is drawn
/// fresh each frame into its own cheap layer. Scrubbing now re-tessellates
/// one line and one small pill instead of the entire lane.
#[derive(Default)]


/// The time ⇄ pixel mapping, computed once per draw/event.
#[derive(Copy, Clone, Debug)]
pub struct TimelineLayout {
    pub content_left: f32,
    pub pixels_per_second: f32,
}

impl TimelineLayout {
    pub fn new(pixels_per_second: f32) -> Self {
        Self { content_left: CONTENT_RAIL, pixels_per_second: pixels_per_second.max(1.0) }
    }

    pub fn x_of(&self, time: Time) -> f32 {
        self.content_left + time.as_secs_f64() as f32 * self.pixels_per_second
    }

    /// Timeline time at a pixel, clamped at zero — correct for the
    /// playhead, which cannot go before the start of the timeline.
    pub fn time_at(&self, x: f32) -> Time {
        Time::from_nanos(self.signed_nanos_at(x).max(0) as u64)
    }

    /// Timeline time at a pixel, **signed**, in nanoseconds.
    ///
    /// A trim handle genuinely needs to express "left of where this clip
    /// currently begins" — that is how previously-trimmed footage is
    /// pulled back in. Clamping at zero (as `time_at` correctly does for
    /// the playhead) silently discarded that, which made the start handle
    /// a one-way ratchet: it could shrink a clip and never re-extend it.
    pub fn signed_nanos_at(&self, x: f32) -> i128 {
        let secs = (x - self.content_left) / self.pixels_per_second;
        (secs as f64 * 1_000_000_000.0) as i128
    }

    pub fn width_of(&self, duration: Time) -> f32 {
        duration.as_secs_f64() as f32 * self.pixels_per_second
    }
}

/// Vertical bands of the timeline, derived once so draw and hit-test
/// cannot disagree about where the clip row ends and the audio lane
/// begins.
struct Bands {
    /// The source trim bar, above everything else.
    trim: Rectangle,
    ruler: Rectangle,
    cap: Rectangle,
    clips: Rectangle,
}

fn bands(bounds: Rectangle) -> Bands {
    let w = bounds.width;
    let trim = Rectangle { x: 0.0, y: 0.0, width: w, height: TRIM_BAR_HEIGHT };
    let ruler = Rectangle { x: 0.0, y: TRIM_BAR_HEIGHT, width: w, height: RULER_HEIGHT };
    // Derived from `ruler`, not from `RULER_HEIGHT` directly: every band
    // below the trim bar shifts down with it, and re-deriving each offset
    // from a constant is how one of them gets forgotten.
    let cap = Rectangle { x: 0.0, y: ruler.y + ruler.height, width: w, height: CAP_BAND_HEIGHT };
    let clips = Rectangle { x: 0.0, y: cap.y + cap.height, width: w, height: CLIP_HEIGHT };
    Bands { trim, ruler, cap, clips }
}

/// Half-width of a trim handle's grab zone, in pixels. Larger than the
/// 14px handle the design system draws, because a 14px-wide grab target is
/// frustrating with a mouse and infuriating on a trackpad — the visual
/// affordance and the hit target are allowed to differ, and should.
const TRIM_GRAB: f32 = 11.0;

pub struct TimelineProgram<'a> {
    pub data: TimelineData<'a>,
}

impl<'a> canvas::Program<TimelineMessage> for TimelineProgram<'a> {
    type State = TimelineState;

    fn update(
        &self,
        state: &mut Self::State,
        event: &canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<TimelineMessage>> {
        // Publish the real lane width the first time it is known, and
        // whenever it changes. `update` is the only hook that both sees
        // `bounds` and may emit a message.
        if (state.reported_width - bounds.width).abs() > 0.5 {
            state.reported_width = bounds.width;
            return Some(canvas::Action::publish(TimelineMessage::LaneMeasured(bounds.width)));
        }

        let layout = TimelineLayout::new(self.data.pixels_per_second);
        let bands = bands(bounds);
        let position = cursor.position_in(bounds);

        // The trim bar owns its own band, and owns the pointer outright
        // while one of its handles is grabbed — a drag that wanders out of
        // the 52px band must keep trimming rather than fall through and
        // start scrubbing the lane underneath.
        if let Some(action) = self.update_trim_bar(state, event, bands.trim, bounds, cursor) {
            return Some(action);
        }

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = position?;

                // Trim handles take precedence over selection: they sit
                // *on* the selected clip's edges, so an edge click must
                // start a trim rather than re-select the clip under it.
                if let Some(selected) = self.data.selected_clip
                    && bands.clips.contains(position)
                    && let Some(handle) = self.trim_handle_at(position.x, selected, &layout)
                {
                    state.interaction = handle;
                    return Some(canvas::Action::publish(TimelineMessage::GestureBegan).and_capture());
                }

                if bands.clips.contains(position)
                    && let Some(index) = self.clip_at(position.x, &layout)
                {
                    return Some(canvas::Action::publish(TimelineMessage::SelectClip(index)).and_capture());
                }

                // Anywhere else (ruler, cap band, empty lane) scrubs.
                state.interaction = Interaction::ScrubbingPlayhead;
                let to = self.clamp_to_timeline(layout.time_at(position.x));
                Some(canvas::Action::publish(TimelineMessage::Seek { to, precise: false }).and_capture())
            }

            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.interaction == Interaction::Idle {
                    return None;
                }
                // Use the raw cursor position, not `position_in`: a drag
                // that leaves the widget's bounds must keep tracking, or
                // trimming stops the instant the pointer strays a pixel
                // above the lane.
                let point = cursor.position()?;
                let x = point.x - bounds.x;

                match state.interaction {
                    Interaction::ScrubbingPlayhead => {
                        let to = self.clamp_to_timeline(layout.time_at(x));
                        Some(canvas::Action::publish(TimelineMessage::Seek { to, precise: false }).and_capture())
                    }
                    Interaction::TrimmingStart(index) => {
                        let to = self.trim_source_time(index, x, &layout, true)?;
                        Some(canvas::Action::publish(TimelineMessage::TrimStart { clip: index, to }).and_capture())
                    }
                    Interaction::TrimmingEnd(index) => {
                        let to = self.trim_source_time(index, x, &layout, false)?;
                        Some(canvas::Action::publish(TimelineMessage::TrimEnd { clip: index, to }).and_capture())
                    }
                    Interaction::Idle => None,
                }
            }

            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let was = state.interaction;
                if was == Interaction::Idle {
                    return None;
                }
                state.interaction = Interaction::Idle;

                if was == Interaction::ScrubbingPlayhead {
                    // Tier two of the two-tier seek: one
                    // ACCURATE seek on release, after a whole drag's
                    // worth of cheap KEY_UNIT seeks.
                    if let Some(point) = cursor.position() {
                        let to = self.clamp_to_timeline(layout.time_at(point.x - bounds.x));
                        return Some(canvas::Action::publish(TimelineMessage::Seek { to, precise: true }).and_capture());
                    }
                }
                Some(canvas::Action::publish(TimelineMessage::GestureEnded).and_capture())
            }

            _ => None,
        }
    }

    fn draw(
        &self,
        state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let layout = TimelineLayout::new(self.data.pixels_per_second);
        let bands = bands(bounds);
        let palette = self.data.palette;

        // # One bar, not four
        //
        // This canvas used to draw a ruler, a filmstrip clip lane, and an
        // audio waveform lane beneath the trim bar. They are gone
        // deliberately: the product's job is "cut a range out of one long
        // video", and for that job the trim bar answers the whole question
        // while the other three lanes restate it at a zoom that cannot
        // show a 101-minute file anyway.
        //
        // It is also the cheapest possible answer to "extremely snappy":
        // the removed lanes were the expensive part of every redraw (text
        // shaping for the ruler labels, image tessellation for the
        // thumbnails, hundreds of waveform rectangles), and the retained
        // static-layer cache existed to avoid re-paying for them while
        // scrubbing. What remains is a handful of paths per frame, so the
        // cache no longer earns its complexity and neither layer needs it.
        //
        // The multi-clip machinery (split/delete/duplicate) still works on
        // the model and still exports correctly; it simply has no lane
        // drawing it right now.
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        if let Some(data) = self.trim_bar_data() {
            crate::trimbar::draw_into(
                &mut frame,
                bands.trim,
                &data,
                state.trim_grabbed,
                state.trim_hovered,
                state.trim_contact,
            );
        }
        let _ = (&layout, palette);

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if matches!(state.interaction, Interaction::TrimmingStart(_) | Interaction::TrimmingEnd(_)) {
            return mouse::Interaction::ResizingHorizontally;
        }
        if state.interaction == Interaction::ScrubbingPlayhead {
            return mouse::Interaction::Grabbing;
        }
        // The trim bar's own grabs, including the red mark.
        match state.trim_grabbed {
            crate::trimbar::Grabbed::Playhead => return mouse::Interaction::Grabbing,
            crate::trimbar::Grabbed::In | crate::trimbar::Grabbed::Out => {
                return mouse::Interaction::ResizingHorizontally;
            }
            crate::trimbar::Grabbed::None => {}
        }
        let Some(position) = cursor.position_in(bounds) else {
            return mouse::Interaction::default();
        };
        // Hovering something grabbable in the trim bar should say so --
        // a scrubbable playhead that looks inert will not be discovered.
        let trim_band = bands(bounds).trim;
        if trim_band.contains(position)
            && let Some(data) = self.trim_bar_data()
        {
            match data.handle_at(position.x - trim_band.x, trim_band.width) {
                crate::trimbar::Grabbed::Playhead => return mouse::Interaction::Grab,
                crate::trimbar::Grabbed::In | crate::trimbar::Grabbed::Out => {
                    return mouse::Interaction::ResizingHorizontally;
                }
                crate::trimbar::Grabbed::None => {}
            }
        }
        let layout = TimelineLayout::new(self.data.pixels_per_second);
        let bands = bands(bounds);
        if let Some(selected) = self.data.selected_clip
            && bands.clips.contains(position)
            && self.trim_handle_at(position.x, selected, &layout).is_some()
        {
            return mouse::Interaction::ResizingHorizontally;
        }
        if bands.ruler.contains(position) || bands.cap.contains(position) {
            return mouse::Interaction::Grab;
        }
        if bands.clips.contains(position) && self.clip_at(position.x, &layout).is_some() {
            return mouse::Interaction::Pointer;
        }
        mouse::Interaction::default()
    }
}

impl<'a> TimelineProgram<'a> {

    fn total(&self) -> Time {
        self.data.project.total_timeline_duration()
    }

    /// A playhead may sit anywhere from 0 to the end of the timeline
    /// inclusive — the end is a legitimate parked position, even though
    /// `resolve_timeline_time` treats it as past the last clip.
    fn clamp_to_timeline(&self, time: Time) -> Time {
        Time::from_nanos(time.as_nanos().min(self.total().as_nanos()))
    }

    fn clip_at(&self, x: f32, layout: &TimelineLayout) -> Option<usize> {
        let time = layout.time_at(x);
        self.data.project.resolve_timeline_time(time).map(|p| p.clip_index)
    }

    fn trim_handle_at(&self, x: f32, selected: usize, layout: &TimelineLayout) -> Option<Interaction> {
        let clip = self.data.project.clips.get(selected)?;
        let start = self.data.project.clip_start_time(selected);
        let end = start.checked_add(clip.timeline_duration())?;
        if (x - layout.x_of(start)).abs() <= TRIM_GRAB {
            return Some(Interaction::TrimmingStart(selected));
        }
        if (x - layout.x_of(end)).abs() <= TRIM_GRAB {
            return Some(Interaction::TrimmingEnd(selected));
        }
        None
    }

    /// Convert a drag x-position into the source time a trim handle
    /// should move to, honoring speed and refusing to invert the clip.
    fn trim_source_time(
        &self,
        index: usize,
        x: f32,
        layout: &TimelineLayout,
        is_start: bool,
    ) -> Option<Time> {
        let clip = self.data.project.clips.get(index)?;
        let clip_start = self.data.project.clip_start_time(index);
        let dragged_nanos = layout.signed_nanos_at(x);

        // Distance from the clip's timeline start, converted back to
        // source time by the speed factor -- the same conversion
        // `resolve_timeline_time` does, applied to a handle instead of a
        // playhead.
        // Signed, deliberately. `saturating_sub` here was a real bug: it
        // clamped any drag to the LEFT of the clip's start to zero
        // offset, so the start handle could only ever shrink the clip,
        // never pull previously-trimmed footage back in. The reference
        // editor (`samplers-tricking`'s `TrimBar.tsx`) lets a handle move
        // freely in both directions within the source, and that is the
        // behavior that makes trimming feel like adjusting a range rather
        // than a one-way ratchet you have to undo.
        let source = |timeline_nanos: i128| -> i128 {
            let offset = timeline_nanos - clip_start.as_nanos() as i128;
            clip.in_point.as_nanos() as i128 + (offset as f64 * clip.speed.factor()) as i128
        };

        // One frame of headroom keeps a trim from producing a zero-length
        // clip, which `trim_clip` would (correctly) reject -- refusing at
        // the boundary here means the drag simply stops instead of the
        // model erroring on every further pixel of movement.
        let min_span = self.data.fps.frame_duration().as_nanos().max(1);

        let source_duration = self
            .data
            .project
            .source(clip.source)
            .map(|s| s.duration.as_nanos())
            .unwrap_or(u64::MAX) as i128;

        if is_start {
            // Free to move left (back toward source 0, restoring trimmed
            // footage) and right (up to a frame before out_point).
            let max = clip.out_point.as_nanos() as i128 - min_span as i128;
            Some(Time::from_nanos(source(dragged_nanos).clamp(0, max.max(0)) as u64))
        } else {
            // Free to move right (out to the end of the source) and left
            // (down to a frame after in_point).
            let min = clip.in_point.as_nanos() as i128 + min_span as i128;
            Some(Time::from_nanos(source(dragged_nanos).clamp(min.min(source_duration), source_duration) as u64))
        }
    }

    /// Pointer handling for the trim bar band. Returns `Some` when the bar
    /// consumed the event, so the timeline's own handling is skipped.
    fn update_trim_bar(
        &self,
        state: &mut TimelineState,
        event: &canvas::Event,
        band: Rectangle,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<TimelineMessage>> {
        use crate::trimbar::{Grabbed, TrimBarMessage as T};

        let Some(data) = self.trim_bar_data() else {
            state.trim_grabbed = Grabbed::None;
            return None;
        };
        let publish = |m: T| canvas::Action::publish(TimelineMessage::TrimBar(m));

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let position = cursor.position_in(bounds)?;
                if !band.contains(position) {
                    return None;
                }
                match data.handle_at(position.x - band.x, band.width) {
                    Grabbed::None => {
                        // The track, not a handle: scrub there — but only
                        // within the kept range. Outside it there is no
                        // frame to show, so the playhead has nowhere
                        // legitimate to land.
                        let raw = data.time_at(position.x - band.x, band.width);
                        let to = data.clamp_playhead(raw);
                        Some(publish(T::Scrub { to, precise: true }).and_capture())
                    }
                    // Grabbing the red mark starts a scrub *drag*, not a
                    // one-shot seek. It emits immediately so a click that
                    // lands slightly off still moves the playhead to the
                    // pointer rather than appearing to do nothing.
                    Grabbed::Playhead => {
                        state.trim_grabbed = Grabbed::Playhead;
                        state.trim_drag = crate::trimbar::DragState::default();
                        let raw = data.time_at(position.x - band.x, band.width);
                        let to = data.clamp_playhead(raw);
                        Some(publish(T::Scrub { to, precise: false }).and_capture())
                    }
                    grabbed => {
                        state.trim_grabbed = grabbed;
                        // A fresh gesture starts with no drag history and
                        // nothing caught. Without this reset, the stop
                        // from a previous drag would still be latched and
                        // the next drag would begin stuck.
                        state.trim_drag = crate::trimbar::DragState::default();
                        Some(publish(T::GestureBegan).and_capture())
                    }
                }
            }

            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if state.trim_grabbed == Grabbed::None {
                    // Hover affordance only; do not consume the event, or
                    // the lane below would stop seeing pointer moves.
                    let hovered = cursor
                        .position_in(bounds)
                        .filter(|p| band.contains(*p))
                        .map(|p| data.handle_at(p.x - band.x, band.width))
                        .unwrap_or(Grabbed::None);
                    if hovered != state.trim_hovered {
                        state.trim_hovered = hovered;
                        return Some(canvas::Action::request_redraw());
                    }
                    return None;
                }

                // A live drag. `cursor.position()` rather than
                // `position_in(band)`: leaving a 52px band vertically is
                // trivial, and a drag that froze there would make the last
                // stretch of a long file unreachable.
                let absolute = cursor.position()?;
                let x = absolute.x - bounds.x - band.x;
                let raw = data.time_at(x, band.width);
                // The playhead resolution needs the pointer's own x,
                // because the detent's tolerance is a pixel distance --
                // see `TrimBarData::resolve_in_drag`.
                let message = match state.trim_grabbed {
                    Grabbed::In => {
                        let r = data.resolve_in_drag(raw, x, band.width, &mut state.trim_drag);
                        state.trim_contact = r.contact;
                        T::SetIn {
                            to: r.edge,
                            precise: false,
                            push_playhead: r.push_playhead,
                            contact: r.contact,
                        }
                    }
                    Grabbed::Out => {
                        let r = data.resolve_out_drag(raw, x, band.width, &mut state.trim_drag);
                        state.trim_contact = r.contact;
                        T::SetOut {
                            to: r.edge,
                            precise: false,
                            push_playhead: r.push_playhead,
                            contact: r.contact,
                        }
                    }
                    // Dragging the red mark. `precise: false` for the
                    // same reason the trim handles use it: at 60+ events
                    // a second an accurate seek per move queues flushes
                    // faster than the decoder retires them, and the image
                    // would lag the pointer by seconds on a long file.
                    // The accurate seek fires once, on release.
                    Grabbed::Playhead => T::Scrub {
                        to: data.clamp_playhead(raw),
                        precise: false,
                    },
                    Grabbed::None => return None,
                };
                Some(publish(message).and_capture())
            }

            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.trim_grabbed == Grabbed::None {
                    return None;
                }
                state.trim_grabbed = Grabbed::None;
                // Release the bump with the grab: a handle nobody is
                // holding is not pressing on anything.
                state.trim_contact = 0.0;
                state.trim_drag = crate::trimbar::DragState::default();
                // The accurate half of the two-tier seek fires once, here.
                Some(publish(T::GestureEnded).and_capture())
            }

            _ => None,
        }
    }

    /// The trim bar's per-frame data, or `None` when the bar does not
    /// apply.
    ///
    /// It applies only to a **single-clip** timeline, which is exactly
    /// what it describes: one source, one range. Once the user splits into
    /// several clips the question stops being "which part of this file"
    /// and becomes "how are these arranged", which the lane below answers.
    /// A bar that silently edited clip 0 of 5 would be lying about its
    /// scope.
    fn trim_bar_data(&self) -> Option<crate::trimbar::TrimBarData> {
        let project = self.data.project;
        if project.clips.len() != 1 {
            return None;
        }
        let clip = project.clips.first()?;
        let source = project.source(clip.source)?;

        // The playhead is TIMELINE time; the bar draws SOURCE time.
        let playhead = Time::from_nanos(
            clip.in_point
                .as_nanos()
                .saturating_add((self.data.playhead.as_nanos() as f64 * clip.speed.factor()) as u64),
        );

        Some(crate::trimbar::TrimBarData {
            palette: self.data.palette,
            source_duration: source.duration,
            in_point: clip.in_point,
            out_point: clip.out_point,
            playhead,
        })
    }





}




/// Build the timeline element.
pub fn timeline_canvas<'a>(data: TimelineData<'a>) -> Element<'a, TimelineMessage> {
    iced::widget::canvas(TimelineProgram { data })
        .width(Length::Fill)
        .height(Length::Fixed(TOTAL_HEIGHT))
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offcut_model::{Rational, Source, SourceId, Speed};

    fn secs(n: f64) -> Time {
        Time::from_nanos((n * 1e9) as u64)
    }

    fn project() -> Project {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/t.mp4".into(),
            duration: secs(40.0),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let sid = source.id;
        project.add_source(source);
        let a = project.add_clip_for_source(sid).unwrap();
        let b = project.split_clip(a, secs(10.0)).unwrap();
        let _ = project.split_clip(b, secs(20.0)).unwrap();
        project
    }

    fn program<'a>(project: &'a Project, selected: Option<usize>) -> TimelineProgram<'a> {
        TimelineProgram {
            data: TimelineData {
                project,
                selected_clip: selected,
                playhead: Time::ZERO,
                palette: Palette::DARK,
                pixels_per_second: 10.0,
                fps: Rational::WEB_30,
            },
        }
    }

    #[test]
    fn time_and_pixels_round_trip_through_the_layout() {
        let layout = TimelineLayout::new(12.0);
        for s in [0.0f64, 1.0, 7.5, 63.25] {
            let t = secs(s);
            let back = layout.time_at(layout.x_of(t));
            assert!(
                (back.as_secs_f64() - s).abs() < 0.001,
                "{s}s mapped to x and back to {}s",
                back.as_secs_f64()
            );
        }
    }

    /// t=0 lands on the rail, and the rail *is* the strip's inset.
    ///
    /// The second assertion no longer pins a literal. Pinning 24.0 here
    /// is what made the two constants free to drift apart while both
    /// tests stayed green — each asserted its own number rather than
    /// their equality. What matters is that they agree, and now they
    /// agree by construction.
    #[test]
    fn the_content_rail_offset_matches_the_design_system() {
        let layout = TimelineLayout::new(10.0);
        assert_eq!(layout.x_of(Time::ZERO), CONTENT_RAIL, "t=0 must sit on the content rail");
        assert_eq!(
            CONTENT_RAIL,
            crate::trimbar::EDGE_INSET,
            "the ruler's rail and the strip's inset must be the same value"
        );
    }

    #[test]
    fn zooming_changes_clip_width_proportionally() {
        let narrow = TimelineLayout::new(5.0);
        let wide = TimelineLayout::new(20.0);
        assert_eq!(wide.width_of(secs(10.0)), 4.0 * narrow.width_of(secs(10.0)));
    }

    /// The whole reason this is a canvas: clip width must be duration ×
    /// scale, not a fixed 160px.
    #[test]
    fn clip_width_is_proportional_to_duration_not_fixed() {
        let mut project = project();
        project.clips[0].speed = Speed::Two; // 10s source -> 5s timeline
        let layout = TimelineLayout::new(10.0);

        let first = layout.width_of(project.clips[0].timeline_duration());
        let second = layout.width_of(project.clips[1].timeline_duration());
        assert_eq!(first, 50.0, "a 5s clip at 10px/s is 50px");
        assert_eq!(second, 100.0, "a 10s clip at 10px/s is 100px");
        assert_ne!(first, second, "clips of different durations must differ in width");
    }

    #[test]
    fn clip_hit_testing_finds_the_clip_under_a_given_x() {
        let project = project();
        let program = program(&project, None);
        let layout = TimelineLayout::new(10.0);

        // Clips are [0,10), [10,20), [20,40) seconds.
        assert_eq!(program.clip_at(layout.x_of(secs(5.0)), &layout), Some(0));
        assert_eq!(program.clip_at(layout.x_of(secs(15.0)), &layout), Some(1));
        assert_eq!(program.clip_at(layout.x_of(secs(30.0)), &layout), Some(2));
        assert_eq!(program.clip_at(layout.x_of(secs(50.0)), &layout), None, "past the end");
    }

    #[test]
    fn trim_handles_are_only_grabbable_near_the_selected_clips_edges() {
        let project = project();
        let program = program(&project, Some(1));
        let layout = TimelineLayout::new(10.0);

        // Clip 1 spans timeline [10s, 20s).
        let start_x = layout.x_of(secs(10.0));
        let end_x = layout.x_of(secs(20.0));
        assert_eq!(program.trim_handle_at(start_x, 1, &layout), Some(Interaction::TrimmingStart(1)));
        assert_eq!(program.trim_handle_at(end_x, 1, &layout), Some(Interaction::TrimmingEnd(1)));
        // Mid-clip is not a handle.
        assert_eq!(program.trim_handle_at(layout.x_of(secs(15.0)), 1, &layout), None);
    }

    #[test]
    fn the_trim_grab_zone_is_wider_than_the_drawn_handle() {
        // A 14px-wide visual handle with a 14px hit target is painful to
        // grab; the zone is deliberately larger.
        const { assert!(TRIM_GRAB * 2.0 > 14.0, "grab zone should exceed the 14px drawn handle") };
    }

    #[test]
    fn trimming_the_start_never_crosses_the_out_point() {
        let project = project();
        let program = program(&project, Some(0));
        let layout = TimelineLayout::new(10.0);

        // Drag clip 0's start far past its own end.
        let to = program.trim_source_time(0, layout.x_of(secs(999.0)), &layout, true).unwrap();
        assert!(to < project.clips[0].out_point, "start must stay before out_point, got {to:?}");
    }

    #[test]
    fn trimming_the_end_never_crosses_the_in_point_or_the_source_end() {
        let project = project();
        let program = program(&project, Some(1));
        let layout = TimelineLayout::new(10.0);

        // Drag clip 1's end back before its own start.
        let to = program.trim_source_time(1, layout.x_of(secs(0.0)), &layout, false).unwrap();
        assert!(to > project.clips[1].in_point, "end must stay after in_point, got {to:?}");

        // And forward past the source's duration.
        let to = program.trim_source_time(1, layout.x_of(secs(9999.0)), &layout, false).unwrap();
        assert!(to <= secs(40.0), "end must not exceed the source duration, got {to:?}");
    }

    /// A trim on a 2× clip must move the source point twice as far as the
    /// pointer moved on the timeline — the same speed conversion the
    /// playhead uses, applied to handles.
    /// The reference editor's behavior, and the bug this replaced: a
    /// start handle dragged LEFT must restore previously-trimmed footage,
    /// not clamp at its current position. Trimming is a range you adjust,
    /// not a one-way ratchet.
    #[test]
    fn dragging_the_start_handle_left_restores_trimmed_footage() {
        let mut project = project();
        // Clip 1 currently spans source [0s, 10s). Trim its head in to 4s,
        // then drag the handle back toward the source's start.
        let clip_id = project.clips[0].id;
        project.trim_clip(clip_id, Some(secs(4.0)), None).unwrap();
        assert_eq!(project.clips[0].in_point, secs(4.0));
        let program = program(&project, Some(0));
        let layout = TimelineLayout::new(10.0);
        // The clip now starts at timeline 0 but at source 4s. Dragging
        // the handle to the LEFT of the clip's start must produce a
        // source time BELOW 4s.
        let to = program.trim_source_time(0, layout.x_of(secs(0.0)) - 20.0, &layout, true).unwrap();
        assert!(
            to < secs(4.0),
            "dragging the start handle left must restore footage, got {to:?}"
        );
        assert!(to >= Time::ZERO, "but never past the start of the source");
    }

    #[test]
    fn dragging_the_end_handle_right_extends_up_to_the_source_end() {
        let mut project = project();
        let clip_id = project.clips[0].id;
        project.trim_clip(clip_id, None, Some(secs(6.0))).unwrap();
        let program = program(&project, Some(0));
        let layout = TimelineLayout::new(10.0);
        let to = program.trim_source_time(0, layout.x_of(secs(30.0)), &layout, false).unwrap();
        assert!(to > secs(6.0), "the end handle must be able to extend, got {to:?}");
        assert!(to <= secs(40.0), "but not past the source duration");
    }

    #[test]
    fn trimming_a_2x_clip_converts_pointer_distance_by_the_speed_factor() {
        let mut project = project();
        project.clips[0].speed = Speed::Two;
        let program = program(&project, Some(0));
        let layout = TimelineLayout::new(10.0);

        // Clip 0 is now 5s of timeline. Dragging its end to timeline 2.5s
        // should set out_point to source 5s.
        let to = program.trim_source_time(0, layout.x_of(secs(2.5)), &layout, false).unwrap();
        assert!(
            (to.as_secs_f64() - 5.0).abs() < 0.05,
            "expected source ~5s for timeline 2.5s at 2x, got {}s",
            to.as_secs_f64()
        );
    }

    #[test]
    fn the_playhead_clamps_to_the_timeline_bounds() {
        let project = project();
        let program = program(&project, None);
        assert_eq!(program.clamp_to_timeline(secs(999.0)), secs(40.0));
        assert_eq!(program.clamp_to_timeline(Time::ZERO), Time::ZERO);
    }



    /// The canvas is exactly the trim bar.
    ///
    /// This replaces two tests that pinned the design system's original
    /// "Timeline 284 (Ruler 30 + Lane 254)" stack and the 132px clip card.
    /// Both described lanes this canvas no longer draws — the ruler,
    /// filmstrip, and waveform were removed so the trim bar is the only
    /// bar. Keeping assertions about a layout that is gone would be
    /// testing the documentation rather than the program.
    #[test]
    fn the_canvas_is_exactly_the_trim_bar() {
        assert_eq!(
            TOTAL_HEIGHT, TRIM_BAR_HEIGHT,
            "the timeline canvas should be the trim bar and nothing else"
        );
        let bounds = Rectangle { x: 0.0, y: 0.0, width: 1000.0, height: TOTAL_HEIGHT };
        let bands = bands(bounds);
        assert_eq!(bands.trim.y, 0.0, "the trim bar starts at the top of the canvas");
        assert_eq!(bands.trim.height, TRIM_BAR_HEIGHT);
    }


}
