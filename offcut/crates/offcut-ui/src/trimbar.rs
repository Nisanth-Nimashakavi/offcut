//! The trim bar: cutting a range out of one long source.
//!
//! # Why this exists next to the timeline rather than inside it
//!
//! The timeline is **duration-scaled**: horizontal distance is time at
//! `zoom` pixels per second, which is the only honest mapping when several
//! clips sit in sequence — the ruler, the playhead, and the clips all have
//! to agree about where a given instant is.
//!
//! That mapping is exactly wrong for this job. The product's first job to
//! be done is "trim the dead air", and the dominant case is *one* long
//! source — a screen recording, a phone clip, a 101-minute movie — that the
//! user wants a short range out of. At any zoom that makes a 3-second
//! selection draggable, a 101-minute source is some 40 metres wide, and the
//! out-point handle is not merely off-screen, it is unreachable without
//! scrolling that itself needs a scrollbar the design does not have.
//!
//! So this widget uses the one mapping the timeline cannot: **the whole
//! source is always exactly the full width**. Both handles are therefore
//! always on screen and always grabbable, for a 5-second clip and a
//! feature film alike. Precision is recovered by the readout beneath it
//! (which reports real timecodes and the resulting duration) and by the
//! timeline below (which stays zoom-scaled for frame-accurate work).
//! Neither control is redundant: this one answers "which part of the
//! file", the timeline answers "exactly which frame".
//!
//! # Shape
//!
//! ```text
//!   ┌───────────────────────────────────────────────────┐
//!   │▓▓▓▓▓▓▓( )═══════════ kept range ═══════════( )▓▓▓▓│   <- track
//!   └───────────────────────────────────────────────────┘
//!    excluded  in-point                      out-point  excluded
//! ```
//!
//! The kept range is the bright figure and the excluded head/tail are
//! dimmed ground, because the range you keep is the subject of this
//! control — inverting that (bright ground, dim selection) reads as "most
//! of the file is selected" at a glance, which is the opposite of the
//! truth when you are cutting 8 seconds out of an hour.
//!
//! # Why the two handles are different colors
//!
//! In-point is `accent` (mint) and out-point is `trim_out` (amber). They
//! are not interchangeable — one sets where the clip begins and the other
//! where it ends — and while dragging, hue is the fastest available signal
//! for which edge is moving. The design system's restrained-palette rule is
//! preserved rather than broken: each saturated color still owns exactly
//! one meaning, and `theme.rs` has a test asserting these two stay hue-
//! distinct from each other and from the mute and playhead colors.

use crate::theme::Palette;
use iced::widget::canvas::{self, Path};
use iced::{Point, Rectangle, Size};
use offcut_model::Time;

/// Track geometry. The strip is deliberately short — it is a range
/// picker, not a second timeline, and giving it filmstrip height would
/// make two controls compete to look like the primary one. It is also
/// burned over the picture now, so every pixel of height is a pixel of
/// frame it dims.
pub const BAR_HEIGHT: f32 = 44.0;
pub const TRACK_HEIGHT: f32 = 30.0;
/// The track's corner radius. A well, not a card: 8px reads as an inset
/// control at this height, where the 12px used for cards would make the
/// bar look like a container holding something.
pub const TRACK_RADIUS: f32 = 8.0;
/// Drawn radius of a handle. The interaction references this product
/// names (Samsung, Pixel) all use round thumbs that sit *on* the track
/// edge rather than beside it, and it is the idiom a person reaches for
/// without instruction.
pub const HANDLE_RADIUS: f32 = 11.0;
/// Horizontal inset of the track within its band.
///
/// Equal to the window's shared content `RAIL`, and that equality is the
/// point: the readout sits directly above the bar, so a range whose t=0
/// did not align with the numbers reporting it is a visible lie about
/// where the source starts. These were 24 and 11 once — two halves of one
/// control, 13px out on both edges. The test below pins them equal.
pub const EDGE_INSET: f32 = 16.0;
/// Half-width of a handle's grab zone. Larger than the drawn handle for
/// the same reason `timeline.rs`'s `TRIM_GRAB` is: the visual affordance
/// and the hit target are allowed to differ, and should.
const GRAB: f32 = 16.0;

/// The narrowest the kept range is ever **drawn**, in pixels.
///
/// Not a clamp on the value — see `draw_into` for why the drawing may
/// widen while `in_point`/`out_point` stay exact. This is what keeps the
/// widget's headline case legible: 8 seconds out of 101 minutes maps to
/// 1.15px, at which the selection, its two white rules, and the in-point
/// thumb all vanish beneath the out-point thumb.
///
/// Sized to the handle diameter, so a range at the floor still reads as
/// *two* handles with material between them rather than as one mark.
pub const MIN_VISIBLE_RANGE: f32 = HANDLE_RADIUS * 2.0;

/// Half-width of the playhead's grab zone.
///
/// Narrower than `GRAB`: the red mark is a 2px line, not a 26px disc, and
/// an over-wide invisible target would swallow clicks intended for the
/// track. Still well above the 2px it draws, because a 2px hit target is
/// unusable with a mouse and impossible on a trackpad.
const PLAYHEAD_GRAB: f32 = 9.0;

/// How far past the playhead the pointer must travel before a handle
/// stops resting against it and starts pushing it.
///
/// This is a **detent**, not a lock. Dragging an edge up to the current
/// time is by far the most common precise trim — you park the playhead on
/// the frame you want and pull the edge to it — so the handle should
/// *land* there rather than skate past and force a second correcting
/// nudge. But the edge must still be able to go further, because
/// sometimes the cut really is past where you are looking.
///
/// 14px is roughly the handle's own radius: far enough that it cannot be
/// crossed by hand tremor or a trackpad's sub-pixel jitter, close enough
/// that deliberately pushing through does not feel like fighting the
/// control.
pub const DETENT_SLOP: f32 = 14.0;

/// How far the pointer must travel **beyond** the stop, once caught, to
/// break free of it.
///
/// # Why this is larger than `DETENT_SLOP`
///
/// A stop you can leave as easily as you entered it is not a stop — it
/// is a speed bump. This is the *hysteresis*: catching costs nothing
/// (you were heading there anyway), but escaping is a deliberate act.
/// The asymmetry is what makes the stop feel like it is holding on
/// rather than merely marking a position.
///
/// This value is what makes the difference between "it caught for a
/// frame and then let go" and a **rolling stop** — the handle rests on
/// the line, stays there while you keep pushing, and releases only when
/// you clearly mean it.
pub const DETENT_ESCAPE: f32 = 26.0;

/// The minimum number of pointer events the stop holds for once caught.
///
/// # Why a distance threshold alone is not enough
///
/// A pixel threshold has to serve two very different hands. A slow drag
/// moves 1–3px per event, so it needs a *small* threshold or the stop
/// feels like glue. A fast flick moves 50–60px per event and clears any
/// small threshold **within a single event** — so the stop caught and
/// released between two consecutive frames, far too briefly to see or
/// feel. That is the "doesn't work if I move the end points fast"
/// report: not a failure to catch, but a failure to *hold*.
///
/// Counting events adds the dimension distance cannot express: **time**.
/// At a typical 60–120 events per second, holding for 6 keeps the stop
/// on screen for roughly 50–100ms — long enough to register as a pause
/// and to see the bump load, short enough that a deliberate push through
/// never feels obstructed.
///
/// Both conditions must be met to break free, so a slow drag still has
/// to travel the full escape distance and a fast one still has to keep
/// moving for a few frames. Neither speed can shortcut the other's test.
pub const DETENT_HOLD_EVENTS: u32 = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Grabbed {
    #[default]
    None,
    In,
    Out,
    /// The red current-time mark itself.
    ///
    /// # Why this variant had to exist
    ///
    /// Scrubbing used to be a *click* and nothing more: pressing the
    /// track emitted one `Scrub`, and the `CursorMoved` arm returned
    /// early because no handle was grabbed. So **dragging the red mark
    /// did nothing** — the preview updated only where you happened to
    /// click, never as you moved, which is exactly "the red mark isn't
    /// showing the current frame".
    ///
    /// The playhead is a thing you grab, like the two edges. Modelling it
    /// as one is what turns a click into a scrub.
    Playhead,
}

/// The live state of one handle drag, threaded between pointer events.
///
/// # Why a drag needs memory
///
/// The first detent was **stateless**: it asked "is the pointer within
/// 14px of the playhead right now?". That is correct only if the pointer
/// visits every position on its way, and it does not. A drag is a stream
/// of discrete events, and a quick flick jumps 30–60px between them — so
/// the pointer could be at −30px on one event and +30px on the next,
/// never once landing inside the window. The handle sailed straight
/// through the line, which is exactly the reported symptom.
///
/// Catching a fast drag requires knowing where the pointer *was*, so the
/// stop can test whether the segment between two events **crossed** the
/// line rather than whether either endpoint sat near it. That is a
/// property of the motion, not of a position, and motion cannot be
/// recovered from a single sample.
#[derive(Debug, Clone, Copy, Default)]
pub struct DragState {
    /// Pointer x at the previous event, if this is not the first.
    pub last_x: Option<f32>,
    /// True once the stop has caught this drag and is holding it.
    pub caught: bool,
    /// How far past the line the pointer was on the event that caught,
    /// in pixels. Escape is measured from here rather than from the line
    /// itself — see `resolve_against_stop` for why that distinction is
    /// the whole difference on a fast drag.
    pub caught_at: Option<f32>,
    /// How many pointer events the stop has been holding for.
    ///
    /// A distance threshold alone cannot serve both drag speeds: a slow
    /// drag needs a small one to feel responsive, a fast drag clears any
    /// small one within a single event. Counting events adds the missing
    /// dimension — **time** — so the stop lasts long enough to be felt
    /// regardless of how fast the hand is moving.
    pub held_events: u32,
}

#[derive(Default)]
pub struct TrimBarState {
    pub grabbed: Grabbed,
    /// Live drag bookkeeping for the detent — see `DragState`.
    pub drag: DragState,
    /// Which handle the pointer is over, for the hover affordance.
    pub hovered: Grabbed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrimBarMessage {
    /// A handle moved to a new SOURCE time. `precise` is false while the
    /// drag is live (fast keyframe seek) and true on release (accurate
    /// seek) — the two-tier seek, which is what keeps
    /// dragging a handle across a long file responsive.
    ///
    /// `push_playhead` is `Some` only when the edge has been pushed past
    /// the playhead and must therefore drag it along, keeping the
    /// playhead inside the clip. In the ordinary case it is `None` and
    /// the red line does not move at all — see `resolve_in_drag`.
    ///
    /// `contact` is how hard the handle is pressed against the playhead
    /// (0..1), used only to draw the bump. It is carried on the message
    /// rather than kept in canvas state because the canvas is redrawn
    /// from data every frame; storing it locally would make the drawn
    /// pressure and the resolved edge two facts that can disagree.
    SetIn { to: Time, precise: bool, push_playhead: Option<Time>, contact: f32 },
    SetOut { to: Time, precise: bool, push_playhead: Option<Time>, contact: f32 },
    /// Clicking the track (not a handle) scrubs to that instant.
    Scrub { to: Time, precise: bool },
    GestureBegan,
    GestureEnded,
}

/// Everything the bar draws. Borrowed per-frame from the shell, like
/// `TimelineData`.
pub struct TrimBarData {
    pub palette: Palette,
    /// The full source duration — the bar's entire horizontal extent.
    pub source_duration: Time,
    pub in_point: Time,
    pub out_point: Time,
    /// Playhead in SOURCE time, drawn as a thin marker so the bar shows
    /// where you are as well as what you kept.
    pub playhead: Time,
}

impl TrimBarData {
    /// Map a source time to an x within `width`.
    ///
    /// The mapping is affine over the *whole source*, inset at both ends
    /// so the handles are never half-clipped. A zero-length source would
    /// divide by zero, so it collapses to the left inset rather than
    /// producing NaN — a NaN x-coordinate silently drops the geometry and
    /// paints nothing, which reads as "the widget is broken" rather than
    /// "this file has no duration".
    pub fn x_of(&self, time: Time, width: f32) -> f32 {
        let usable = (width - 2.0 * EDGE_INSET).max(1.0);
        let total = self.source_duration.as_secs_f64();
        if total <= 0.0 {
            return EDGE_INSET;
        }
        let fraction = (time.as_secs_f64() / total).clamp(0.0, 1.0) as f32;
        EDGE_INSET + fraction * usable
    }

    /// Inverse of `x_of`, clamped into the source.
    pub fn time_at(&self, x: f32, width: f32) -> Time {
        let usable = (width - 2.0 * EDGE_INSET).max(1.0);
        let fraction = ((x - EDGE_INSET) / usable).clamp(0.0, 1.0) as f64;
        Time::from_nanos((fraction * self.source_duration.as_nanos() as f64) as u64)
    }

    /// Which handle, if any, is within grabbing distance of `x`.
    ///
    /// When the two handles overlap (a very short range on a long source,
    /// where both land on nearly the same pixel) the tie is broken by
    /// which side of the *range midpoint* the pointer is on, so a
    /// collapsed range can still be opened back up in both directions
    /// rather than one handle permanently shadowing the other.
    /// The two range edges take priority over the playhead when they
    /// overlap: the edges have a stop that *parks* them on the playhead,
    /// so the two coincide constantly during ordinary trimming, and
    /// letting the red mark shadow them there would make a parked edge
    /// impossible to pick up again.
    pub fn handle_at(&self, x: f32, width: f32) -> Grabbed {
        let in_x = self.x_of(self.in_point, width);
        let out_x = self.x_of(self.out_point, width);
        let (d_in, d_out) = ((x - in_x).abs(), (x - out_x).abs());

        if d_in <= GRAB || d_out <= GRAB {
            if (d_in - d_out).abs() < 0.5 {
                let mid = (in_x + out_x) / 2.0;
                return if x < mid { Grabbed::In } else { Grabbed::Out };
            }
            return if d_in <= d_out { Grabbed::In } else { Grabbed::Out };
        }

        // Not on an edge: is this the red mark? A narrower zone than the
        // edges get, because the playhead is a 2px line rather than a
        // 26px disc and a wide invisible target would swallow clicks
        // meant for the track.

        if (x - self.x_of(self.playhead, width)).abs() <= PLAYHEAD_GRAB {
            return Grabbed::Playhead;
        }

        Grabbed::None
    }

    /// The smallest range the bar will let a drag produce, in source time.
    ///
    /// Without a floor, dragging one handle past the other inverts the
    /// range and `set_range` rejects it — so the drag would appear to
    /// simply stop working with no explanation. Stopping a pixel short of
    /// the other handle is the same "refuse at the boundary" behavior
    /// `timeline.rs`'s trim uses.
    fn min_span_nanos(&self) -> u64 {
        // A tenth of a second, or the whole source if it is somehow
        // shorter than that.
        100_000_000u64.min(self.source_duration.as_nanos().max(1))
    }

    /// Clamp a proposed in-point so it cannot reach or pass the out-point.
    pub fn clamp_in(&self, proposed: Time) -> Time {
        let max = self.out_point.as_nanos().saturating_sub(self.min_span_nanos());
        Time::from_nanos(proposed.as_nanos().min(max))
    }

    /// Clamp a proposed out-point so it cannot reach or pass the in-point.
    pub fn clamp_out(&self, proposed: Time) -> Time {
        let min = self.in_point.as_nanos().saturating_add(self.min_span_nanos());
        let capped = proposed.as_nanos().max(min).min(self.source_duration.as_nanos());
        Time::from_nanos(capped)
    }

    /// The selected range's duration — the number the user is actually
    /// aiming at when cutting a clip.
    pub fn range_duration(&self) -> Time {
        Time::from_nanos(self.out_point.as_nanos().saturating_sub(self.in_point.as_nanos()))
    }

    /// Resolve a handle drag against the playhead.
    ///
    /// # The interaction this implements
    ///
    /// Three rules, in priority order:
    ///
    /// 1. **The playhead does not move when an edge moves.** Trimming
    ///    answers "where does this clip begin"; the playhead answers
    ///    "which frame am I looking at". Yanking the second because you
    ///    changed the first destroys the reference frame you were
    ///    trimming *against*, which is the one thing you needed to keep
    ///    still.
    ///
    /// 2. **An edge detents at the playhead.** Dragging toward the red
    ///    line, the handle stops there — the common precise trim lands
    ///    exactly, with no correcting nudge.
    ///
    /// 3. **Pushing past the detent moves them together.** Once the
    ///    pointer is `DETENT_SLOP` beyond the playhead, the edge is
    ///    clearly meant to go further, so it takes the playhead with it.
    ///    That keeps the invariant the whole design rests on: **the
    ///    playhead can never sit outside the clip.** A red line outside
    ///    the kept range points at a frame the clip does not contain.
    ///
    /// Returns the resolved edge time and, when rule 3 fires, the time
    /// the playhead must be pushed to.
    ///
    /// `pointer_x` and `width` are needed because the detent's tolerance
    /// is a *pixel* distance, not a time one: at feature-length zoom a
    /// fixed nanosecond slop would be thousands of pixels wide, and on a
    /// short clip it would be sub-pixel. The user's hand works in pixels.
    /// `drag` carries the previous pointer position and whether the stop
    /// has already caught, which is what lets a **fast** drag be caught:
    /// see `DragState`.
    pub fn resolve_in_drag(
        &self,
        proposed: Time,
        pointer_x: f32,
        width: f32,
        drag: &mut DragState,
    ) -> DragResolution {
        let clamped = self.clamp_in(proposed);
        let head_x = self.x_of(self.playhead, width);
        // Signed distance past the stop, in the direction of travel.
        let overshoot = pointer_x - head_x;
        let previous = drag.last_x.map(|x| x - head_x);
        drag.last_x = Some(pointer_x);

        // # Why the overtake guard is skipped while caught
        //
        // The shell **commits each resolved edge** before the next
        // pointer event arrives. So the instant this stop catches, the
        // clip's `in_point` *becomes* the playhead — and the guard below,
        // which asks "has the handle already passed the playhead?", reads
        // that resting state as a yes.
        //
        // Left in front, it therefore un-latched the stop on the very
        // next event: the handle caught for exactly one frame and then
        // sailed on, which is indistinguishable from never catching. The
        // earlier tests missed it because they held `TrimBarData` fixed
        // for a whole gesture, which is not how the app runs.
        //
        // While caught, the resting equality is expected and means
        // nothing; only `resolve_against_stop` may end the catch.
        if !drag.caught && self.playhead <= self.in_point.min(clamped) && self.playhead < clamped {
            return DragResolution::pushed(clamped);
        }

        self.resolve_against_stop(clamped, overshoot, previous, drag, |edge| {
            edge <= self.playhead
        })
    }

    /// The out-point counterpart of `resolve_in_drag`, mirrored: the
    /// out-point approaches the playhead from the right, so the sign of
    /// travel flips.
    pub fn resolve_out_drag(
        &self,
        proposed: Time,
        pointer_x: f32,
        width: f32,
        drag: &mut DragState,
    ) -> DragResolution {
        let clamped = self.clamp_out(proposed);
        let head_x = self.x_of(self.playhead, width);
        let overshoot = head_x - pointer_x;
        let previous = drag.last_x.map(|x| head_x - x);
        drag.last_x = Some(pointer_x);

        // Skipped while caught, for the same reason as the in-point's
        // guard: the committed edge rests *on* the playhead, and reading
        // that as "already overtaken" releases the stop after one frame.
        if !drag.caught && self.playhead >= self.out_point.max(clamped) && self.playhead > clamped {
            return DragResolution::pushed(clamped);
        }

        self.resolve_against_stop(clamped, overshoot, previous, drag, |edge| {
            edge >= self.playhead
        })
    }

    /// The direction-agnostic heart of the stop.
    ///
    /// Both handles behave identically once distances are expressed as
    /// *travel past the stop*, so the logic lives here once rather than
    /// being mirrored (and drifting) in two places.
    ///
    /// `overshoot` is how far the pointer is beyond the stop now,
    /// `previous` how far it was at the last event, and `short_of` says
    /// whether a proposed edge has not yet reached the stop.
    ///
    /// # The rolling stop, in three parts
    ///
    /// 1. **Catch on crossing, not on proximity.** If the pointer was
    ///    short of the line and is now past it, the drag crossed it —
    ///    however fast. This is the fix for blowing straight through.
    /// 2. **Hold once caught.** A caught drag stays caught until the
    ///    pointer has travelled `DETENT_ESCAPE` **from where it was when
    ///    it caught**, so the handle rests on the line instead of
    ///    catching for one frame and letting go.
    /// 3. **Release deliberately.** Easy to enter, harder to leave: that
    ///    asymmetry is what makes it feel like a stop rather than a bump
    ///    in the road.
    ///
    /// # Why escape is measured from the catch point, not from the line
    ///
    /// Measuring from the line looks equivalent and is not, because a
    /// **fast** drag does not stop at the line — it flies past it. A
    /// 55px-per-event flick catches on the event that crosses (landing,
    /// say, 50px beyond) and the *very next* event is already 105px out,
    /// which trips any line-relative threshold immediately. The stop then
    /// lasts exactly one frame: too brief to see, let alone feel. That is
    /// precisely the "doesn't work if I move the end points fast"
    /// report, and it survived the crossing fix because catching was
    /// never the problem — *holding* was.
    ///
    /// Anchoring at the catch point makes the escape a genuine amount of
    /// **additional hand movement**, which is what "harder to leave"
    /// has to mean physically. A flick that crosses the line now still
    /// has to travel a further 26px before it breaks free, so the stop
    /// registers at any speed.
    fn resolve_against_stop(
        &self,
        clamped: Time,
        overshoot: f32,
        previous: Option<f32>,
        drag: &mut DragState,
        short_of: impl Fn(Time) -> bool,
    ) -> DragResolution {
        // Part 3: already caught. Release needs BOTH enough extra travel
        // and enough elapsed events -- see `DETENT_HOLD_EVENTS`.
        if drag.caught {
            drag.held_events = drag.held_events.saturating_add(1);
            let anchor = drag.caught_at.unwrap_or(0.0);
            let travelled = overshoot - anchor;

            let far_enough = travelled > DETENT_ESCAPE;
            let long_enough = drag.held_events >= DETENT_HOLD_EVENTS;
            if far_enough && long_enough {
                drag.caught = false;
                drag.caught_at = None;
                drag.held_events = 0;
                return DragResolution::pushed(clamped);
            }
            // Part 2: still holding, whichever side of the line the
            // pointer is on. Contact is reported relative to the anchor
            // so the bump loads with the *extra* travel rather than
            // jumping straight to full on a fast crossing.
            return DragResolution::detented(self.playhead, travelled);
        }

        // Part 1: did this event's motion cross the stop? `previous < 0`
        // means the pointer had not reached it; `overshoot >= 0` means it
        // now has. A jump of any size is caught, which a proximity test
        // cannot do.
        let crossed = previous.is_some_and(|p| p < 0.0) && overshoot >= 0.0;
        if crossed {
            drag.caught = true;
            // Remember how far past the line this event landed. A fast
            // flick can overshoot by 50px on the crossing event alone,
            // and that overshoot is not intent to leave -- it is just
            // sampling. Escape is measured from here.
            drag.caught_at = Some(overshoot);
            return DragResolution::detented(self.playhead, 0.0);
        }

        // Not caught and no crossing. Still short of the stop: free.
        if short_of(clamped) {
            return DragResolution::free(clamped);
        }

        // Past the stop without ever crossing it during this drag --
        // e.g. the handle started beyond the playhead. Catching here
        // would yank it backwards, so it stays free.
        DragResolution::pushed(clamped)
    }

    /// Clamp a scrub target into the kept range.
    ///
    /// The playhead scrubs freely — but only *within the clip*. Outside
    /// it there is no frame to show, so a red line there would point at
    /// footage the clip does not contain.
    pub fn clamp_playhead(&self, proposed: Time) -> Time {
        let nanos = proposed
            .as_nanos()
            .clamp(self.in_point.as_nanos(), self.out_point.as_nanos());
        Time::from_nanos(nanos)
    }
}

/// The outcome of resolving a handle drag against the playhead.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DragResolution {
    /// Where the dragged edge should land.
    pub edge: Time,
    /// Where the playhead must be pushed to, when the edge has moved past
    /// it. `None` in the common case — the playhead stays exactly where
    /// the user parked it.
    pub push_playhead: Option<Time>,
    /// How hard the handle is currently pressed against the playhead,
    /// `0.0` (just touching) to `1.0` (about to break through), or `0.0`
    /// when not in contact at all.
    ///
    /// # Why a continuous value rather than a bool
    ///
    /// A detent that only reported "stuck / not stuck" would let the
    /// handle sit motionless while the pointer travelled 14 pixels, with
    /// nothing on screen acknowledging the input. That reads as *lag* —
    /// the control appearing to have missed the drag — which is the exact
    /// impression a detent is supposed to avoid.
    ///
    /// Reporting the pressure lets the bar deform slightly as the pointer
    /// pushes: the physical language of something resting against a stop
    /// and being leaned on. Movement continues to track the hand even
    /// though the *value* is held, so the control stays visibly alive
    /// while still landing exactly on the frame.
    pub contact: f32,
}

impl DragResolution {
    /// Moving freely, not touching the playhead.
    fn free(edge: Time) -> Self {
        Self { edge, push_playhead: None, contact: 0.0 }
    }

    /// Pushed through the detent; the playhead comes along.
    fn pushed(edge: Time) -> Self {
        Self { edge, push_playhead: Some(edge), contact: 0.0 }
    }

    /// Resting against the playhead, with `overshoot` pixels of pointer
    /// travel past it (negative when the pointer has fallen back behind
    /// the line while still caught).
    fn detented(edge: Time, overshoot: f32) -> Self {
        Self {
            edge,
            push_playhead: None,
            // Pressure is measured against the *escape* distance, not the
            // catch window: the bump should reach full deformation just
            // as the handle is about to break free, so the visual is a
            // preview of the release rather than saturating a third of
            // the way there and then sitting still.
            //
            // Only positive overshoot is pressure — a pointer that has
            // drifted back behind the line is no longer leaning on it.
            contact: (overshoot / DETENT_ESCAPE).clamp(0.0, 1.0),
        }
    }
}

/// Draw the trim bar into `band` of an existing frame.
///
/// A free function taking a `Frame` rather than a `canvas::Program`
/// method: this control shares the timeline's single canvas (see
/// `timeline.rs`'s `TRIM_BAR_HEIGHT` for why), so it draws into a band of
/// someone else's frame and owns no widget of its own.
///
/// `x_of`/`time_at` work in band-local coordinates, so every x here is
/// offset by `band.x` on the way out.
pub fn draw_into(
    frame: &mut canvas::Frame,
    band: Rectangle,
    data: &TrimBarData,
    grabbed: Grabbed,
    hovered: Grabbed,
    contact: f32,
) {
    let palette = data.palette;
    let w = band.width;
    let track_y = band.y + (band.height - TRACK_HEIGHT) / 2.0;
    let in_x = band.x + data.x_of(data.in_point, w);
    let out_x = band.x + data.x_of(data.out_point, w);
    let track_x = band.x + EDGE_INSET;
    let track_w = (w - 2.0 * EDGE_INSET).max(1.0);

    // # The track
    //
    // The whole source, as one rounded well. Dark in both appearances —
    // see `trim_track` in `theme.rs`: this is a viewing instrument like
    // Final Cut's timeline, not a panel, and its amber and red marks are
    // unreadable on a light grey ground.
    let track = Path::rounded_rectangle(
        Point::new(track_x, track_y),
        Size::new(track_w, TRACK_HEIGHT),
        TRACK_RADIUS.into(),
    );
    frame.fill(&track, palette.trim_track);

    // The excluded head and tail: faintly lifted off the well, so they
    // read as material that is *there but cut away* rather than as empty
    // track. Drawn before the selection so the bright figure lands on top.
    //
    // # Why these are clipped to the track instead of filled square
    //
    // They used to be plain `fill_rectangle`s spanning the full track
    // height — square corners painted over a shape with 8px round ones.
    // At each end the rectangle covered the track's curve *and* the bare
    // panel outside it, so the well's rounded corner was replaced by a
    // straight 45° edge: a chamfer, not a radius.
    //
    // It was legible in a screenshot as three tones meeting at a
    // diagonal, and the arithmetic confirms the identification exactly —
    // `trim_range_excluded` is white at 6%, which over `trim_track`
    // (#151515) composites to 35 and over `surface` (#252525) to 50.
    // Those are the two values either side of the diagonal in the
    // capture. The bright edge was this fill sitting on the panel, in
    // the crescent the track had already curved away from.
    //
    // Clipping to the track's own path is the fix rather than
    // re-rounding these two rectangles: their *outer* ends must follow
    // the well's radius while their inner ends stay square against the
    // selection, and no single corner radius expresses that. Intersecting
    // with the shape they are meant to be inside says it directly, and
    // keeps saying it if `TRACK_RADIUS` ever changes.
    // The whole well, as a path. Each excluded span paints *this* shape
    // through a clip window covering only its own end, so the outer
    // corners inherit the track's radius while the inner edge stays
    // square against the selection.
    let well = Path::rounded_rectangle(
        Point::new(track_x, track_y),
        Size::new(track_w, TRACK_HEIGHT),
        TRACK_RADIUS.into(),
    );
    for (x0, x1) in [(track_x, in_x), (out_x, track_x + track_w)] {
        let w = x1 - x0;
        if w > 0.5 {
            frame.with_clip(
                iced::Rectangle { x: x0, y: track_y, width: w, height: TRACK_HEIGHT },
                |frame| frame.fill(&well, palette.trim_range_excluded),
            );
        }
    }

    // The kept range is the figure, and its fill must beat the track by
    // enough to be legible at a glance. This is the measurement that
    // matters here, and it has caught the same defect twice: a chrome
    // tint tuned for grey surfaces measured **1.33:1** on this track, and
    // a 16% white wash measured **1.62:1**. Both rendered as an empty
    // trough. A solid `trim_range_fill` is what fixed it.
    //
    // The range you keep is the subject; inverting that (bright ground,
    // dim selection) says "most of the file is selected", the opposite of
    // the truth when cutting 8 seconds out of an hour.
    //
    // # Why the drawn range has a floor and the value does not
    //
    // The bar's whole justification is the long source: 8 seconds kept
    // from a 101-minute file. At that ratio the selection is **1.15px**
    // wide — measured — so the fill, both white rules, and the in-point
    // thumb all collapsed underneath the out-point thumb, and the control
    // rendered as a single amber dot with nothing between its handles.
    // The one case this widget exists for was the one it could not show.
    //
    // So the *drawing* clamps to a legible minimum while `in_point` and
    // `out_point` keep their exact values. That is the honest direction
    // for the lie to run: the readout beside the bar reports the true
    // duration to the frame, and the product's rule is that the preview
    // may not lie about the *edit* — a selection widened to stay visible
    // does not change a single exported frame. Rounding the value instead
    // would corrupt the edit, which is the version that is not allowed.
    let range_w = (out_x - in_x).max(MIN_VISIBLE_RANGE);
    let range = Path::rounded_rectangle(
        Point::new(in_x, track_y),
        Size::new(range_w, TRACK_HEIGHT),
        TRACK_RADIUS.into(),
    );
    frame.fill(&range, palette.trim_range_fill);

    // Rules bounding the kept range, so it reads as bounded at the edges
    // where a handle is aimed. Full strength here rather than in the
    // fill: a 2px edge can carry white without the wide selection
    // becoming the loudest thing in the window.
    //
    // Clipped to the range's own path, for the same reason the excluded
    // spans above are: these are square 2px bars spanning the *full*
    // width of a shape whose top row is inset by `TRACK_RADIUS` at each
    // end. Unclipped, up to 8px of hard white juts past the blue at all
    // four corners and lands on the dark track — square white tabs on a
    // rounded selection, which is what makes a radius read as fake.
    for y in [track_y, track_y + TRACK_HEIGHT - 2.0] {
        // The rule *is* the range's own path, painted through a 2px
        // window, so its ends follow the selection's curve instead of
        // overhanging it.
        frame.with_clip(
            iced::Rectangle { x: in_x, y, width: range_w, height: 2.0 },
            |frame| frame.fill(&range, palette.trim_range_edge),
        );
    }

    // The playhead, so the bar shows where you are as well as what you
    // kept. It overhangs the track top and bottom, which is what makes it
    // read as a marker crossing the control rather than a mark inside it.
    //
    // Under contact it thickens: something is pressing on it, and the
    // stop should look like it is taking the load rather than like the
    // handle happened to stall there.
    let head_x = band.x + data.x_of(data.playhead, w);
    let ph_w = 2.0 + contact * 1.5;
    let ph_overhang = 4.0 + contact * 3.0;

    // # Why the red line is cased in the well's own dark
    //
    // The playhead has to cross two very different grounds: the dark well
    // and the blue selection. Red on the well is 4.29:1, but red on the
    // selection blue is **1.44:1** — measured — so the marker would fade
    // out over exactly the range being trimmed, which is where a person
    // is looking. Neither colour can be retuned out of it: moving the red
    // far enough off the blue makes it the loudest thing in the window,
    // and it must stay the platform's system red to keep meaning
    // "current frame" rather than "some other state".
    //
    // A casing is the standard fix, and the reason every NLE's playhead
    // has one. The dark rule either side gives the red a constant local
    // ground, so its legibility stops depending on what it happens to be
    // passing over.
    frame.fill_rectangle(
        Point::new(head_x - ph_w / 2.0 - 1.0, track_y - ph_overhang),
        Size::new(ph_w + 2.0, TRACK_HEIGHT + ph_overhang * 2.0),
        palette.trim_track,
    );
    frame.fill_rectangle(
        Point::new(head_x - ph_w / 2.0, track_y - ph_overhang),
        Size::new(ph_w, TRACK_HEIGHT + ph_overhang * 2.0),
        palette.playhead,
    );

    // Handles last, above the range and the playhead.
    //
    // Rounded thumbs straddling the track edge — the idiom the phone
    // editors this product takes its interaction model from all use, and
    // the one a person reaches for without instruction. The product rules
    // names Samsung's "big rounded thumb handles" as an explicit
    // interaction reference.
    // The thumbs are drawn against the same widened range as the fill, so
    // a collapsed selection shows two handles with material between them
    // rather than one thumb hiding the other. `in_x`/`out_x` are untouched
    // for hit-testing, which keeps grabbing exact — only the paint moves.
    let draw_out_x = out_x.max(in_x + range_w);
    let cy = track_y + TRACK_HEIGHT / 2.0;
    for (which, x, base) in [
        (Grabbed::In, in_x, palette.trim_range_edge),
        (Grabbed::Out, draw_out_x, palette.trim_out),
    ] {
        let active = grabbed == which;
        let hot = hovered == which && grabbed == Grabbed::None;
        // Only the handle actually doing the pressing deforms.
        let press = if active { contact } else { 0.0 };

        let color = match (which, active || hot) {
            (Grabbed::Out, true) => palette.trim_out_hover,
            (_, true) => palette.stage_badge_text,
            _ => base,
        };

        // # The bump
        //
        // Pressed against the stop, the handle **squashes**: narrower
        // across the direction of travel, taller across it, conserving
        // roughly the same area. That is what a soft body resting against
        // a hard edge does, and reading it takes no attention because
        // everyone already knows what it means.
        //
        // It also solves a real problem rather than merely decorating.
        // While detented the handle's *position* is pinned, so without
        // this the control would sit motionless through 14px of pointer
        // travel and read as dropped input. The deformation keeps the
        // widget tracking the hand while the value stays exactly on the
        // frame.
        let grow = if active || hot { 1.0 } else { 0.0 };
        let rx = HANDLE_RADIUS + grow - press * 3.0;
        let ry = HANDLE_RADIUS + grow + press * 2.4;

        // Squashing also shifts the centre slightly *back* from the stop,
        // as contact flattens the leading face against it.
        let nudge = if which == Grabbed::In { press * 1.6 } else { -press * 1.6 };
        let hx = x + nudge;

        frame.fill(&ellipse_path(Point::new(hx, cy), rx, ry), color);

        // A grip of two short rules inside the thumb — the standard mark
        // that says "this is draggable", and a second signal beside hue
        // so the handles do not depend on colour alone. Cut in the well's
        // own dark, so the grip reads on both the white in-point thumb and
        // the amber out-point one.
        for offset in [-2.0f32, 2.0] {
            frame.fill_rectangle(
                Point::new(hx + offset - 0.5, cy - ry * 0.34),
                Size::new(1.0, ry * 0.68),
                palette.trim_track,
            );
        }
    }
}

/// An axis-aligned ellipse, since `canvas` offers only circles.
///
/// Built from four cubic segments with the standard circle-to-Bézier
/// constant. The handle needs this because the bump deforms it along one
/// axis only, and scaling a circle uniformly cannot express "squashed".
fn ellipse_path(center: Point, rx: f32, ry: f32) -> Path {
    // Distance to the control points that makes a cubic approximate a
    // quarter ellipse to within ~0.02% — the usual 4/3·(√2−1).
    const K: f32 = 0.552_284_8;
    let (cx, cy) = (center.x, center.y);
    let (rx, ry) = (rx.max(0.5), ry.max(0.5));
    Path::new(|b| {
        b.move_to(Point::new(cx, cy - ry));
        b.bezier_curve_to(
            Point::new(cx + rx * K, cy - ry),
            Point::new(cx + rx, cy - ry * K),
            Point::new(cx + rx, cy),
        );
        b.bezier_curve_to(
            Point::new(cx + rx, cy + ry * K),
            Point::new(cx + rx * K, cy + ry),
            Point::new(cx, cy + ry),
        );
        b.bezier_curve_to(
            Point::new(cx - rx * K, cy + ry),
            Point::new(cx - rx, cy + ry * K),
            Point::new(cx - rx, cy),
        );
        b.bezier_curve_to(
            Point::new(cx - rx, cy - ry * K),
            Point::new(cx - rx * K, cy - ry),
            Point::new(cx, cy - ry),
        );
        b.close();
    })
}

/// Compact `M:SS` / `H:MM:SS` for the readout under the bar. Distinct from
/// `shell::fmt_timecode`'s frame-accurate `HH:MM:SS:FF`: this control is
/// about *which part of the file*, and frame numbers at this scale are
/// noise. The frame-accurate readout still lives on the viewport.
pub fn fmt_duration(time: Time) -> String {
    let total = time.as_secs_f64() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f64) -> Time {
        Time::from_nanos((s * 1e9) as u64)
    }

    fn data(source_secs: f64, in_secs: f64, out_secs: f64) -> TrimBarData {
        TrimBarData {
            palette: Palette::DARK,
            source_duration: secs(source_secs),
            in_point: secs(in_secs),
            out_point: secs(out_secs),
            playhead: secs(in_secs),
        }
    }

    const W: f32 = 1000.0;

    /// **Nothing square may be painted over the well's rounded corners.**
    ///
    /// # The defect this pins, stated as geometry
    ///
    /// Two fills spanned a full width with square ends over shapes whose
    /// corners are rounded to `TRACK_RADIUS`:
    ///
    /// - the excluded head/tail, drawn `TRACK_HEIGHT` tall across the
    ///   whole track;
    /// - the two 2px white rules, drawn across the whole selection.
    ///
    /// A square rectangle over an 8px radius overhangs the curve by up to
    /// the full radius at the corner, so the well's rounded end was
    /// overpainted with a straight 45° edge — a chamfer, not a radius —
    /// and the white rules ended in hard square tabs sitting on the dark
    /// track.
    ///
    /// It is visible in a screenshot as three tones meeting at a
    /// diagonal, and the compositing arithmetic identifies it exactly:
    /// `trim_range_excluded` is white at 6%, which over `trim_track`
    /// (#151515) gives 35 and over `surface` (#252525) gives 50. Both
    /// values appear either side of the diagonal in the capture — the
    /// bright edge is this fill landing on bare panel, in the crescent
    /// the track had already curved away from.
    ///
    /// This test asserts the overhang the fix removes is real and large
    /// enough to matter, so the constants cannot drift back into a
    /// geometry where clipping is skipped as unnecessary.
    #[test]
    fn a_square_fill_would_overhang_the_wells_rounded_corner() {
        // How far the rounded shape's top edge is inset from its bounding
        // box at a given depth below the top: the horizontal gap a square
        // fill would cover but the rounded one leaves empty.
        let inset_at = |depth: f32| {
            let r = TRACK_RADIUS;
            if depth >= r {
                return 0.0;
            }
            r - (r * r - (r - depth) * (r - depth)).sqrt()
        };

        // At the very top row the gap is the whole radius.
        assert!(
            (inset_at(0.0) - TRACK_RADIUS).abs() < 1e-6,
            "the corner's top row should be inset by the full radius"
        );

        // Across the 2px band the white rules occupy, the gap stays wide
        // enough to read as a hard tab rather than as antialiasing.
        let at_rule_bottom = inset_at(2.0);
        assert!(
            at_rule_bottom > 2.0,
            "the edge rules would overhang the fill by only {at_rule_bottom:.2}px, so this \
             test no longer describes a visible defect"
        );

        // And the radius must actually be a radius. At zero these fills
        // could be drawn square and the clipping would be pointless.
        // `const {}`, matching the geometry assertions in `shell.rs`:
        // facts about constants should fail the build, not wait for a
        // test run.
        const {
            assert!(
                TRACK_RADIUS > 0.0,
                "the well has no rounded corners, so nothing here needs clipping"
            );
            assert!(
                TRACK_RADIUS * 2.0 <= TRACK_HEIGHT,
                "the radius exceeds half the track height, so the corners meet and the \
                 well is a stadium rather than a rounded rectangle"
            );
        }
    }

    /// The property that makes this widget worth having: however long the
    /// source is, both handles land inside the visible width. This is the
    /// exact case the zoom-scaled timeline cannot serve.
    #[test]
    fn both_handles_are_on_screen_for_a_feature_length_source() {
        // 101 minutes, an 8-second selection five minutes in.
        let d = data(101.0 * 60.0, 300.0, 308.0);
        let in_x = d.x_of(d.in_point, W);
        let out_x = d.x_of(d.out_point, W);

        for (name, x) in [("in", in_x), ("out", out_x)] {
            assert!(
                (0.0..=W).contains(&x),
                "the {name} handle landed at {x}px, outside the {W}px bar — \
                 a long source must still be fully addressable"
            );
        }
    }

    #[test]
    fn the_full_source_spans_the_full_usable_width() {
        let d = data(60.0, 0.0, 60.0);
        assert_eq!(d.x_of(Time::ZERO, W), EDGE_INSET);
        assert!((d.x_of(secs(60.0), W) - (W - EDGE_INSET)).abs() < 0.01);
    }

    #[test]
    fn x_of_and_time_at_are_inverses() {
        let d = data(600.0, 0.0, 600.0);
        for t in [0.0, 1.0, 137.5, 599.0, 600.0] {
            let x = d.x_of(secs(t), W);
            let back = d.time_at(x, W).as_secs_f64();
            assert!((back - t).abs() < 0.5, "round-trip of {t}s came back as {back}s");
        }
    }

    /// A zero-duration source must not produce NaN coordinates. NaN
    /// geometry is silently dropped by the renderer, so the bug would
    /// present as "the trim bar is invisible", not as a crash.
    #[test]
    fn a_zero_duration_source_does_not_produce_nan_coordinates() {
        let d = data(0.0, 0.0, 0.0);
        let x = d.x_of(Time::ZERO, W);
        assert!(x.is_finite(), "x_of produced {x}");
        assert!(d.time_at(500.0, W).as_nanos() == 0);
    }

    #[test]
    fn handle_hit_testing_finds_the_nearer_handle() {
        let d = data(100.0, 20.0, 80.0);
        let in_x = d.x_of(d.in_point, W);
        let out_x = d.x_of(d.out_point, W);

        assert_eq!(d.handle_at(in_x, W), Grabbed::In);
        assert_eq!(d.handle_at(out_x, W), Grabbed::Out);
        // Dead centre between them is neither.
        assert_eq!(d.handle_at((in_x + out_x) / 2.0, W), Grabbed::None);
    }

    /// The overlap case: a very short range on a long source puts both
    /// handles on nearly the same pixel. Both must still be reachable, or
    /// the range can never be reopened.
    #[test]
    fn overlapping_handles_are_disambiguated_by_side() {
        let d = data(3600.0, 1800.0, 1800.5);
        let in_x = d.x_of(d.in_point, W);
        let out_x = d.x_of(d.out_point, W);
        assert!((out_x - in_x).abs() < 1.0, "this test needs the handles to nearly coincide");

        assert_eq!(d.handle_at(in_x - 4.0, W), Grabbed::In, "left of the pair grabs the in-point");
        assert_eq!(d.handle_at(out_x + 4.0, W), Grabbed::Out, "right of the pair grabs the out-point");
    }

    /// Dragging a handle past its partner must stop at a floor rather than
    /// invert the range — an inverted range is rejected by `set_range`,
    /// which from the user's side would look like the drag silently
    /// breaking.
    #[test]
    fn handles_cannot_cross_each_other() {
        let d = data(100.0, 20.0, 30.0);

        let clamped_in = d.clamp_in(secs(99.0));
        assert!(clamped_in < d.out_point, "in-point must stay before out-point");

        let clamped_out = d.clamp_out(secs(0.0));
        assert!(clamped_out > d.in_point, "out-point must stay after in-point");
    }

    /// Helper: a bar whose playhead is parked somewhere specific.
    fn parked(source: f64, in_s: f64, out_s: f64, head_s: f64) -> TrimBarData {
        TrimBarData {
            palette: Palette::DARK,
            source_duration: secs(source),
            in_point: secs(in_s),
            out_point: secs(out_s),
            playhead: secs(head_s),
        }
    }

    /// Replay a drag as a real gesture: a sequence of pointer positions
    /// through one `DragState`, returning the last resolution.
    ///
    /// Tests must go through this rather than calling the resolver once
    /// with a fresh state. The stop is deliberately **motion-sensitive**
    /// — it catches on the pointer *crossing* the line — and a single
    /// isolated sample has no motion to inspect. A one-shot test would
    /// also be testing a situation the app never produces.
    fn drag_in(d: &TrimBarData, xs: &[f32]) -> DragResolution {
        *drag_in_live(d, xs).last().expect("a drag needs at least one position")
    }

    fn drag_out(d: &TrimBarData, xs: &[f32]) -> DragResolution {
        let mut live = TrimBarData {
            palette: d.palette,
            source_duration: d.source_duration,
            in_point: d.in_point,
            out_point: d.out_point,
            playhead: d.playhead,
        };
        let mut state = DragState::default();
        let mut last = None;
        for &x in xs {
            let r = live.resolve_out_drag(live.time_at(x, W), x, W, &mut state);
            live.out_point = r.edge;
            if let Some(p) = r.push_playhead {
                live.playhead = p;
            }
            last = Some(r);
        }
        last.expect("a drag needs at least one position")
    }

    /// Replay a drag the way the **app** actually runs it: the shell
    /// commits each resolved edge to the clip, so the *next* event sees a
    /// `TrimBarData` whose `in_point` has already moved.
    ///
    /// The earlier simulator held `TrimBarData` constant for the whole
    /// gesture, which is not what happens and is why it certified a stop
    /// that does not hold in practice.
    fn drag_in_live(start: &TrimBarData, xs: &[f32]) -> Vec<DragResolution> {
        let mut d = TrimBarData {
            palette: start.palette,
            source_duration: start.source_duration,
            in_point: start.in_point,
            out_point: start.out_point,
            playhead: start.playhead,
        };
        let mut state = DragState::default();
        let mut out = Vec::new();
        for &x in xs {
            let r = d.resolve_in_drag(d.time_at(x, W), x, W, &mut state);
            // What the shell does with the result.
            d.in_point = r.edge;
            if let Some(p) = r.push_playhead {
                d.playhead = p;
            }
            out.push(r);
        }
        out
    }

    /// The stop must survive the edge actually being committed.
    ///
    /// Once the handle rests on the line, `in_point == playhead`. If any
    /// guard treats that state as "the handle has already overtaken the
    /// playhead", the stop un-latches on the very next event and the
    /// handle carries on through — catching for a single frame and then
    /// releasing, which is indistinguishable from never catching at all.
    #[test]
    fn the_stop_still_holds_once_the_edge_is_committed_each_event() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // Approach, cross, then keep pushing gently -- all well inside
        // the escape window.
        let xs = [
            head_x - 20.0,
            head_x + 2.0,
            head_x + 5.0,
            head_x + 8.0,
            head_x + 11.0,
        ];
        let results = drag_in_live(&d, &xs);

        for (i, r) in results.iter().enumerate().skip(1) {
            assert_eq!(
                r.edge,
                secs(40.0),
                "event {i} ({:+.0}px past the line): the handle left the stop",
                xs[i] - head_x
            );
            assert_eq!(r.push_playhead, None, "event {i} dragged the playhead");
        }
    }

    /// Dragging the red mark must emit a scrub on **every** pointer
    /// event, not only on the initial press.
    ///
    /// The regression this pins: the playhead was not grabbable at all.
    /// Press emitted one `Scrub` and `CursorMoved` returned early because
    /// `trim_grabbed` was `None`, so the preview updated only where you
    /// happened to click and never as you dragged -- "the red mark isn't
    /// showing the current frame".
    #[test]
    fn the_playhead_is_grabbable_so_dragging_it_can_scrub() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        assert_eq!(
            d.handle_at(head_x, W),
            Grabbed::Playhead,
            "the red mark must be grabbable, or dragging it cannot scrub"
        );
        // And a few pixels either side, since a 2px line is not a
        // realistic hit target.
        assert_eq!(d.handle_at(head_x - 5.0, W), Grabbed::Playhead);
        assert_eq!(d.handle_at(head_x + 5.0, W), Grabbed::Playhead);
    }

    /// Bare track is still bare track: the playhead's grab zone must not
    /// swallow clicks meant for scrubbing elsewhere.
    #[test]
    fn the_playhead_grab_zone_does_not_swallow_the_whole_track() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);
        assert_eq!(d.handle_at(head_x + 40.0, W), Grabbed::None);
        assert_eq!(d.handle_at(head_x - 40.0, W), Grabbed::None);
    }

    /// The edges must win when they coincide with the playhead.
    ///
    /// They coincide *constantly*, because the trim stop parks an edge
    /// exactly on the red mark. If the playhead shadowed them there, a
    /// parked edge could never be picked up again -- the stop would trap
    /// the handle it just caught.
    #[test]
    fn an_edge_parked_on_the_playhead_is_still_grabbable() {
        // In-point resting exactly on the playhead, as the stop leaves it.
        let d = parked(100.0, 40.0, 90.0, 40.0);
        let x = d.x_of(secs(40.0), W);
        assert_eq!(
            d.handle_at(x, W),
            Grabbed::In,
            "a parked edge must win over the playhead, or the stop traps it"
        );

        // Same for the out-point.
        let d = parked(100.0, 10.0, 60.0, 60.0);
        let x = d.x_of(secs(60.0), W);
        assert_eq!(d.handle_at(x, W), Grabbed::Out);
    }

    /// A drag that deliberately pushes through the stop: enough events,
    /// and enough travel, to satisfy both escape conditions.
    fn breakthrough_in(d: &TrimBarData, head_x: f32) -> Vec<f32> {
        let mut xs = vec![head_x - 18.0];
        for i in 0..12 {
            xs.push(head_x + 4.0 + i as f32 * 12.0);
        }
        let _ = d;
        xs
    }

    fn breakthrough_out(d: &TrimBarData, head_x: f32) -> Vec<f32> {
        let mut xs = vec![head_x + 18.0];
        for i in 0..12 {
            xs.push(head_x - 4.0 - i as f32 * 12.0);
        }
        let _ = d;
        xs
    }

    /// The reported bug, at a realistic flick speed.
    ///
    /// ~55px per pointer event is an ordinary fast drag. The stop must
    /// **hold across several consecutive events**, not catch on one and
    /// release on the next -- a one-frame catch is invisible, which is
    /// exactly why this felt like no stop at all.
    #[test]
    fn a_fast_flick_holds_the_stop_long_enough_to_be_felt() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // Six events at 55px each, straddling the line.
        let xs: Vec<f32> = (0..6).map(|i| head_x - 60.0 + i as f32 * 55.0).collect();
        let results = drag_in_live(&d, &xs);

        let resting = results.iter().filter(|r| r.edge == secs(40.0)).count();
        assert!(
            resting >= 4,
            "a fast flick rested on the line for only {resting} of 6 events -- \
             too brief to see or feel, which is indistinguishable from no stop"
        );
    }

    /// ...and it must still be escapable. A stop that a fast drag cannot
    /// leave would be worse than none.
    #[test]
    fn a_sustained_fast_drag_still_breaks_through_eventually() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let xs: Vec<f32> = (0..12).map(|i| head_x - 60.0 + i as f32 * 55.0).collect();
        let results = drag_in_live(&d, &xs);

        let last = results.last().unwrap();
        assert!(
            last.edge > secs(40.0),
            "a drag that kept going must eventually break free, got {:?}",
            last.edge
        );
        assert!(last.push_playhead.is_some(), "and must carry the playhead once free");
    }

    /// The stop must hold for a comparable number of events whether the
    /// hand is fast or slow. Speed should change how far you travel while
    /// held, not whether the stop exists.
    #[test]
    fn the_stop_lasts_a_similar_number_of_events_at_any_drag_speed() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        for (label, step) in [("slow", 3.0f32), ("medium", 18.0), ("fast", 55.0)] {
            let xs: Vec<f32> = (0..14)
                .map(|i| head_x - step * 2.0 + i as f32 * step)
                .collect();
            let results = drag_in_live(&d, &xs);
            let resting = results.iter().filter(|r| r.edge == secs(40.0)).count();
            assert!(
                resting >= 4,
                "{label} drag ({step}px/event) held the stop for only {resting} events"
            );
        }
    }

    /// A **slow** drag — a pixel per event, the way a hand actually
    /// creeps up on a target — must catch and then hold for the whole
    /// approach, not flicker in and out of the stop.
    ///
    /// Slow and fast drags exercise different paths (the slow one lands
    /// inside the window; the fast one only crosses it), so both need
    /// covering. This runs 40 events through the live commit loop.
    #[test]
    fn a_slow_creeping_drag_catches_and_then_holds() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let xs: Vec<f32> = (0..40).map(|i| head_x - 12.0 + i as f32).collect();
        let results = drag_in_live(&d, &xs);

        let mut caught_from: Option<usize> = None;
        for (i, r) in results.iter().enumerate() {
            let past = xs[i] - head_x;
            if r.edge == secs(40.0) && past >= 0.0 && caught_from.is_none() {
                caught_from = Some(i);
            }
            if caught_from.is_some() && (0.0..DETENT_ESCAPE).contains(&past) {
                assert_eq!(
                    r.edge,
                    secs(40.0),
                    "event {i} ({past:+.0}px past) let go after catching at event {:?}",
                    caught_from
                );
            }
        }
        assert!(caught_from.is_some(), "a slow drag never caught the stop at all");

        let far = drag_in_live(&d, &breakthrough_in(&d, head_x));
        assert!(
            far.last().unwrap().edge > secs(40.0),
            "the handle must still be able to break free"
        );
    }

    /// **Rule 2.** Dragging the in-point toward the playhead stops *on*
    /// it rather than sliding past — the detent that makes "trim up to
    /// where I am looking" land in one gesture instead of two.
    #[test]
    fn the_in_handle_detents_at_the_playhead_instead_of_passing_it() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // Approach from the left, then step just past the line.
        let r = drag_in(&d, &[head_x - 20.0, head_x - 8.0, head_x + DETENT_SLOP * 0.5]);

        assert_eq!(r.edge, secs(40.0), "the handle should rest exactly on the playhead");
        assert_eq!(r.push_playhead, None, "and must not drag the playhead with it");
    }

    /// **Rule 3.** Push clearly past the playhead and the two move
    /// together, so the playhead cannot be left outside the clip.
    #[test]
    fn pushing_the_in_handle_through_the_detent_carries_the_playhead() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // Approach, catch, then keep going well past the escape window.
        let r = drag_in(&d, &breakthrough_in(&d, head_x));

        assert!(r.edge > secs(40.0), "past the escape window the handle must keep moving");
        assert_eq!(
            r.push_playhead,
            Some(r.edge),
            "and the playhead must come with it, never be left behind the in-point"
        );
    }

    /// The **rolling stop**: once caught, the handle keeps resting on the
    /// line across many further events, rather than catching for a single
    /// frame and letting go.
    ///
    /// This is the difference the user described. A stop that releases on
    /// the next event is a speed bump; one that holds while you push is a
    /// stop.
    #[test]
    fn once_caught_the_handle_keeps_resting_on_the_line() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let mut state = DragState::default();
        let approach = head_x - 18.0;
        let _ = d.resolve_in_drag(d.time_at(approach, W), approach, W, &mut state);

        // Every position from the line out to just short of escape must
        // still hold the edge on the playhead.
        let mut x = head_x + 1.0;
        while x < head_x + DETENT_ESCAPE {
            let r = d.resolve_in_drag(d.time_at(x, W), x, W, &mut state);
            assert_eq!(
                r.edge,
                secs(40.0),
                "at {:.0}px past the line the stop let go too early",
                x - head_x
            );
            assert!(state.caught, "the catch latch must stay set while held");
            x += 2.0;
        }
    }

    /// Escaping must cost more travel than catching did. A stop you can
    /// leave as easily as you entered is a bump, not a stop.
    #[test]
    fn escaping_the_stop_is_harder_than_entering_it() {
        // `const`, so a future edit that inverts these fails to *compile*
        // rather than only at test time.
        const {
            assert!(
                DETENT_ESCAPE > DETENT_SLOP,
                "the escape window must exceed the catch window, or the stop has \
                 no holding power and becomes a speed bump"
            )
        };
    }

    /// Having broken free, a continuing drag must not be re-caught by the
    /// same stop — it would stutter against a line the user has already
    /// deliberately passed.
    #[test]
    fn a_handle_that_broke_free_is_not_immediately_recaught() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let mut state = DragState::default();
        let mut live = TrimBarData {
            palette: d.palette, source_duration: d.source_duration,
            in_point: d.in_point, out_point: d.out_point, playhead: d.playhead,
        };
        for x in breakthrough_in(&d, head_x) {
            let r = live.resolve_in_drag(live.time_at(x, W), x, W, &mut state);
            live.in_point = r.edge;
            if let Some(p) = r.push_playhead { live.playhead = p; }
        }
        assert!(!state.caught, "the stop should have released");

        // Continue past it: must keep moving freely.
        for x in [head_x + 200.0, head_x + 240.0] {
            let r = live.resolve_in_drag(live.time_at(x, W), x, W, &mut state);
            assert!(r.push_playhead.is_some(), "a freed handle must keep travelling");
            assert_eq!(r.contact, 0.0, "and must not look like it is touching anything");
        }
    }

    /// **Rule 1.** Moving an edge *away* from the playhead must not touch
    /// it at all. This is the headline complaint: the red park marker
    /// should stay where it was put.
    #[test]
    fn dragging_an_edge_away_from_the_playhead_never_moves_it() {
        let d = parked(100.0, 20.0, 80.0, 50.0);

        // In-point dragged left, further from the playhead.
        let x = d.x_of(secs(5.0), W);
        let r = d.resolve_in_drag(secs(5.0), x, W, &mut DragState::default());
        assert_eq!(r.push_playhead, None);
        assert_eq!(r.edge, secs(5.0));

        // Out-point dragged right, likewise.
        let x = d.x_of(secs(95.0), W);
        let r = d.resolve_out_drag(secs(95.0), x, W, &mut DragState::default());
        assert_eq!(r.push_playhead, None);
        assert_eq!(r.edge, secs(95.0));
    }

    /// A drag is a stream of DISCRETE pointer events. If the hand moves
    /// quickly, consecutive events can jump tens of pixels, so the
    /// pointer may never once land inside the detent window.
    #[test]
    fn a_fast_drag_still_catches_on_the_playhead() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // One event well before the stop, the next well past it --
        // exactly what a quick flick produces.
        let before = head_x - 30.0;
        let after = head_x + 30.0;

        let r = drag_in(&d, &[before, after]);
        assert_eq!(
            r.edge,
            secs(40.0),
            "a fast drag must still be caught by the stop, not sail through it"
        );
        assert_eq!(r.push_playhead, None, "and must not have dragged the playhead");
    }

    /// The bump's core property: while detented, pressure **rises with
    /// pointer travel** rather than being a bool.
    ///
    /// This is what stops a detented drag from reading as dropped input.
    /// The edge's value is pinned, so if nothing else changed the widget
    /// would sit motionless through 14px of hand movement; the rising
    /// contact keeps it visibly tracking.
    #[test]
    fn contact_pressure_rises_as_the_handle_is_pressed_into_the_playhead() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let mut state = DragState::default();
        let approach = head_x - 18.0;
        let _ = d.resolve_in_drag(d.time_at(approach, W), approach, W, &mut state);

        let mut last = -1.0;
        for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let pointer = head_x + DETENT_ESCAPE * fraction;
            let r = d.resolve_in_drag(d.time_at(pointer, W), pointer, W, &mut state);

            assert_eq!(r.edge, secs(40.0), "the edge must stay pinned while detented");
            assert!(
                r.contact > last,
                "pressure must increase with travel: {fraction} gave {} after {last}",
                r.contact
            );
            assert!((0.0..=1.0).contains(&r.contact), "contact escaped 0..=1: {}", r.contact);
            last = r.contact;
        }
        assert!((last - 1.0).abs() < 1e-6, "at full slop the bump should be fully loaded");
    }

    /// A handle in free travel is not touching anything, so it must not
    /// deform. Otherwise the bump would be ambient decoration rather than
    /// a signal that means one specific thing.
    #[test]
    fn a_handle_in_free_travel_reports_no_contact() {
        let d = parked(100.0, 20.0, 80.0, 50.0);

        let x = d.x_of(secs(30.0), W);
        assert_eq!(d.resolve_in_drag(secs(30.0), x, W, &mut DragState::default()).contact, 0.0);

        let x = d.x_of(secs(70.0), W);
        assert_eq!(d.resolve_out_drag(secs(70.0), x, W, &mut DragState::default()).contact, 0.0);
    }

    /// Once pushed through, the handle is moving freely again — it is no
    /// longer resting on the stop, so the bump must release rather than
    /// stay stuck at full deformation.
    #[test]
    fn breaking_through_the_detent_releases_the_bump() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        // Contact loads with travel *after* the catch, not on the
        // crossing event itself: a fast flick crosses far past the line
        // in one jump, and treating that as full pressure would make the
        // bump snap to maximum instead of easing in.
        let loaded = drag_in(&d, &[head_x - 18.0, head_x + 2.0, head_x + 2.0 + DETENT_ESCAPE]);
        assert!(
            loaded.contact > 0.9,
            "the bump should be near fully loaded at the escape distance, got {}",
            loaded.contact
        );

        let r = drag_in(&d, &breakthrough_in(&d, head_x));
        assert_eq!(r.contact, 0.0, "past the stop the handle is free again");
        assert!(r.push_playhead.is_some(), "and is now carrying the playhead");
    }

    /// Approaching the stop is not yet leaning on it: contact must not go
    /// negative or read as pressure before the handle actually touches.
    #[test]
    fn approaching_the_stop_registers_no_pressure_until_contact() {
        let d = parked(100.0, 0.0, 100.0, 40.0);
        let head_x = d.x_of(secs(40.0), W);

        let before = head_x - 6.0;
        let r = drag_in(&d, &[head_x - 30.0, before]);
        assert_eq!(r.contact, 0.0, "not touching yet");
        assert!(r.contact >= 0.0, "contact must never be negative");
    }

    /// The out-point's bump is symmetric with the in-point's — pressing
    /// from the right must feel the same as pressing from the left.
    #[test]
    fn the_out_handle_bump_mirrors_the_in_handle_bump() {
        let d = parked(100.0, 0.0, 100.0, 50.0);
        let head_x = d.x_of(secs(50.0), W);

        for fraction in [0.25, 0.5, 1.0] {
            let in_x = head_x + DETENT_ESCAPE * fraction;
            let out_x = head_x - DETENT_ESCAPE * fraction;

            let a = drag_in(&d, &[head_x - 18.0, in_x]).contact;
            let b = drag_out(&d, &[head_x + 18.0, out_x]).contact;
            assert!(
                (a - b).abs() < 1e-5,
                "asymmetric bump at {fraction}: in {a} vs out {b}"
            );
        }
    }

    /// The out-point's detent is the mirror image: it approaches the
    /// playhead from the right.
    #[test]
    fn the_out_handle_detents_at_the_playhead_from_the_other_side() {
        let d = parked(100.0, 0.0, 100.0, 60.0);
        let head_x = d.x_of(secs(60.0), W);

        let r = drag_out(&d, &[head_x + 20.0, head_x - DETENT_SLOP * 0.5]);
        assert_eq!(r.edge, secs(60.0), "the out handle should rest on the playhead");
        assert_eq!(r.push_playhead, None);

        let r = drag_out(&d, &breakthrough_out(&d, head_x));
        assert!(r.edge < secs(60.0), "past the escape window it must keep moving");
        assert_eq!(r.push_playhead, Some(r.edge), "and carry the playhead");
    }

    /// **The invariant.** However an edge is dragged, the playhead must
    /// end up inside `[in, out]`. Swept across the whole bar for both
    /// handles rather than spot-checked, because "never outside the clip"
    /// is a claim about every position, not a few.
    #[test]
    fn the_playhead_is_never_left_outside_the_clip_by_any_drag() {
        let d = parked(100.0, 20.0, 80.0, 50.0);

        for step in 0..=100 {
            let x = EDGE_INSET + (W - 2.0 * EDGE_INSET) * (step as f32 / 100.0);
            let raw = d.time_at(x, W);

            let r = d.resolve_in_drag(raw, x, W, &mut DragState::default());
            let head = r.push_playhead.unwrap_or(d.playhead);
            assert!(
                head >= r.edge && head <= d.out_point,
                "in-drag at x={x}: playhead {head:?} escaped [{:?}, {:?}]",
                r.edge,
                d.out_point
            );

            let r = d.resolve_out_drag(raw, x, W, &mut DragState::default());
            let head = r.push_playhead.unwrap_or(d.playhead);
            assert!(
                head >= d.in_point && head <= r.edge,
                "out-drag at x={x}: playhead {head:?} escaped [{:?}, {:?}]",
                d.in_point,
                r.edge
            );
        }
    }

    /// Scrubbing is free — but only inside the kept range. Outside it
    /// there is no frame to display.
    #[test]
    fn scrubbing_is_clamped_into_the_kept_range() {
        let d = parked(100.0, 20.0, 80.0, 50.0);
        assert_eq!(d.clamp_playhead(secs(0.0)), secs(20.0), "before the in-point clamps to it");
        assert_eq!(d.clamp_playhead(secs(99.0)), secs(80.0), "past the out-point clamps to it");
        assert_eq!(d.clamp_playhead(secs(45.0)), secs(45.0), "inside the range is untouched");
    }

    /// The detent is a *pixel* tolerance, so it must behave identically
    /// on a 5-second clip and a feature film — the whole reason
    /// `resolve_*_drag` takes an x rather than working purely in time.
    #[test]
    fn the_detent_tolerance_is_the_same_on_any_source_length() {
        for source_secs in [5.0, 300.0, 101.0 * 60.0] {
            let mid = source_secs / 2.0;
            let d = parked(source_secs, 0.0, source_secs, mid);
            let head_x = d.x_of(secs(mid), W);

            let inside = head_x + DETENT_SLOP * 0.5;
            let r = drag_in(&d, &[head_x - 18.0, inside]);
            assert_eq!(
                r.push_playhead, None,
                "{source_secs}s source: the detent should hold within the slop"
            );

            let r = drag_in(&d, &breakthrough_in(&d, head_x));
            assert!(
                r.push_playhead.is_some(),
                "{source_secs}s source: past the slop the playhead should be pushed"
            );
        }
    }

    /// The bump must read as a **squash**, not as the handle shrinking or
    /// swelling. The two axes move in opposite directions and the area
    /// stays roughly constant — the behaviour of a soft body against a
    /// hard stop, which is why it needs no explanation.
    ///
    /// These are the same expressions `draw_into` uses. Pinning them here
    /// keeps the deformation from drifting into "the handle just gets
    /// smaller", which reads as an error state rather than as contact.
    #[test]
    fn the_bump_squashes_the_handle_rather_than_resizing_it() {
        let axes = |press: f32| {
            let rx = HANDLE_RADIUS - press * 3.0;
            let ry = HANDLE_RADIUS + press * 2.4;
            (rx, ry, rx * ry)
        };

        let (rx0, ry0, a0) = axes(0.0);
        assert_eq!((rx0, ry0), (HANDLE_RADIUS, HANDLE_RADIUS), "at rest it is a circle");

        let (rx1, ry1, a1) = axes(1.0);
        assert!(rx1 < rx0, "the pressed axis must narrow");
        assert!(ry1 > ry0, "the free axis must bulge");
        assert!(
            (a1 / a0 - 1.0).abs() < 0.12,
            "area changed by {:.0}% under full press — a squash conserves area, \
             a resize does not",
            (a1 / a0 - 1.0) * 100.0
        );
        assert!(rx1 > 0.0, "the handle must never invert or vanish");
        // The deformation must be *perceptible*: a sub-pixel change
        // during a 14px detent is the same as no feedback at all.
        assert!(
            (ry1 - ry0) >= 1.0,
            "the free axis grows by only {:.2}px under full press — below one pixel the \
             detent still reads as dropped input",
            ry1 - ry0
        );
    }

    /// An ellipse is only worth the extra geometry if it stays one — a
    /// degenerate or inverted radius would drop the handle out of the
    /// control entirely, which is the one thing a range picker's handle
    /// may never do.
    #[test]
    fn the_handle_ellipse_stays_well_formed_at_every_pressure() {
        for press in [0.0f32, 0.3, 0.7, 1.0] {
            for grow in [0.0f32, 1.0] {
                let rx = HANDLE_RADIUS + grow - press * 3.0;
                let ry = HANDLE_RADIUS + grow + press * 2.4;
                assert!(rx >= 0.5 && ry >= 0.5, "press {press} produced radii ({rx}, {ry})");
                assert!(rx.is_finite() && ry.is_finite());
                // Built without panicking, at every pressure.
                let _ = ellipse_path(Point::new(100.0, 50.0), rx, ry);
            }
        }
    }

    /// Renders the bump at a range of pressures into a PNG so the
    /// deformation can be inspected by eye rather than only asserted on.
    /// `--ignored`, because it writes a file and is a visual aid, not a
    /// gate.
    #[test]
    #[ignore]
    fn write_bump_contact_sheet() {

        const W_PX: u32 = 720;
        const ROW: u32 = 88;
        let pressures = [0.0f32, 0.25, 0.5, 0.75, 1.0];
        let h_px = ROW * pressures.len() as u32;

        let mut img = vec![0u8; (W_PX * h_px * 3) as usize];
        // Paint the panel background.
        for px in img.chunks_exact_mut(3) {
            px.copy_from_slice(&[18, 22, 32]);
        }

        let plot = |img: &mut Vec<u8>, x: f32, y: f32, c: [u8; 3]| {
            if x < 0.0 || y < 0.0 || x >= W_PX as f32 || y >= h_px as f32 {
                return;
            }
            let i = ((y as u32 * W_PX + x as u32) * 3) as usize;
            img[i..i + 3].copy_from_slice(&c);
        };

        for (row, &press) in pressures.iter().enumerate() {
            let cy = row as f32 * ROW as f32 + ROW as f32 / 2.0;
            let head_x = W_PX as f32 / 2.0;

            // Track.
            for x in 24..(W_PX - 24) {
                for dy in -20..20 {
                    plot(&mut img, x as f32, cy + dy as f32, [10, 13, 18]);
                }
            }
            // Playhead, thickening under load (same expressions as draw_into).
            let head_w = 2.0 + press * 1.5;
            let overhang = 2.0 + press * 3.0;
            let hw = (head_w / 2.0).max(0.5);
            let mut x = head_x - hw;
            while x <= head_x + hw {
                let mut y = cy - 20.0 - overhang;
                while y <= cy + 20.0 + overhang {
                    plot(&mut img, x, y, [232, 72, 72]);
                    y += 0.5;
                }
                x += 0.5;
            }

            // The squashed in-handle resting against it.
            let rx = HANDLE_RADIUS + 1.0 - press * 3.0;
            let ry = HANDLE_RADIUS + 1.0 + press * 2.4;
            let hx = head_x + press * 1.6 - rx;
            let mut dy = -ry;
            while dy <= ry {
                let span = rx * (1.0 - (dy / ry) * (dy / ry)).max(0.0).sqrt();
                let mut dx = -span;
                while dx <= span {
                    plot(&mut img, hx + dx, cy + dy, [78, 205, 164]);
                    dx += 0.5;
                }
                dy += 0.5;
            }
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/review/bump-contact-sheet.ppm");
        let mut out = format!("P6\n{W_PX} {h_px}\n255\n").into_bytes();
        out.extend_from_slice(&img);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create review dir");
        }
        std::fs::write(&path, out).expect("write contact sheet");
        eprintln!("wrote {}", path.display());
    }

    #[test]
    fn the_out_point_cannot_be_dragged_past_the_end_of_the_source() {
        let d = data(100.0, 10.0, 50.0);
        assert_eq!(d.clamp_out(secs(1000.0)), secs(100.0));
    }

    #[test]
    fn range_duration_is_out_minus_in() {
        let d = data(100.0, 20.0, 32.5);
        assert!((d.range_duration().as_secs_f64() - 12.5).abs() < 1e-9);
    }

    #[test]
    fn range_duration_never_underflows_on_an_inverted_range() {
        let d = data(100.0, 50.0, 10.0);
        assert_eq!(d.range_duration(), Time::ZERO, "must saturate, not wrap around u64");
    }

    /// The trim bar's t=0 must land on the same x as the ruler's 0:00,
    /// or the bar is quietly lying about where the source begins.
    #[test]
    fn the_bar_shares_the_timelines_content_rail() {
        assert_eq!(
            EDGE_INSET,
            crate::timeline::CONTENT_RAIL,
            "the trim bar and the ruler below it must start at the same x"
        );
    }

    /// A handle centered at t=0 must not be clipped by the canvas edge.
    #[test]
    fn the_end_handles_are_fully_inside_the_canvas() {
        let d = data(100.0, 0.0, 100.0);
        let left = d.x_of(Time::ZERO, W);
        let right = d.x_of(secs(100.0), W);
        assert!(
            left - HANDLE_RADIUS >= 0.0,
            "the in-point handle at t=0 extends to {}, off the left edge",
            left - HANDLE_RADIUS
        );
        assert!(
            right + HANDLE_RADIUS <= W,
            "the out-point handle at the end extends to {}, past the {W}px width",
            right + HANDLE_RADIUS
        );
    }

    /// Every mark on the trim bar must clear its floor **against the
    /// thing it is actually painted on**.
    ///
    /// # Why this composites instead of comparing tokens
    ///
    /// Two of these marks are semi-transparent, and a raw token
    /// comparison silently reads their alpha as opaque — which is how the
    /// bar shipped a selection measuring 1.62:1 while a token test passed.
    /// What reaches the eye is `trim_range_fill` on `trim_track`, and that
    /// stack is what has to clear the floor.
    ///
    /// This has now caught the same class of defect three times. A chrome
    /// tint reused on the dark track: **1.06:1**. A white 22% wash laid on
    /// the picture: **1.00:1 over a white frame**. A white 16% wash on the
    /// well: **1.62:1**. Every other unit test passed each time, the
    /// geometry was correct to the pixel, and the bar rendered as an empty
    /// trough.
    #[test]
    fn every_trim_bar_mark_clears_its_floor_on_its_own_ground() {
        fn luminance(c: iced::Color) -> f32 {
            let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        }
        fn contrast(a: iced::Color, b: iced::Color) -> f32 {
            let (la, lb) = (luminance(a), luminance(b));
            let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
            (hi + 0.05) / (lo + 0.05)
        }
        /// Source-over compositing, the same operation the renderer does.
        fn over(fg: iced::Color, bg: iced::Color) -> iced::Color {
            let a = fg.a;
            iced::Color {
                r: fg.r * a + bg.r * (1.0 - a),
                g: fg.g * a + bg.g * (1.0 - a),
                b: fg.b * a + bg.b * (1.0 - a),
                a: 1.0,
            }
        }

        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            // The order `draw_into` paints in: well, then excluded
            // head/tail on the well, then the selection over that.
            let track = p.trim_track;
            let excluded = over(p.trim_range_excluded, track);
            let kept = over(p.trim_range_fill, track);

            // The selection is the figure. 3:1 is the graphical floor; the
            // three historical failures above all sat under 2:1.
            let ratio = contrast(kept, excluded);
            assert!(
                ratio >= 3.0,
                "{name}: the kept range is {ratio:.2}:1 against the excluded material — \
                 at this ratio the selection is invisible and the bar reads as empty"
            );

            // The excluded head and tail must be present, not absent: a
            // track indistinguishable from its own cut-away material says
            // the source has no head and tail at all.
            let lifted = contrast(excluded, track);
            assert!(
                lifted >= 1.05,
                "{name}: the excluded head/tail is {lifted:.2}:1 against the bare well — \
                 the cut-away material has vanished rather than reading as excluded"
            );

            // The bounding rules sit on the selection fill, and the
            // in-point thumb is the same colour, so this one ratio covers
            // both. Below 3:1 the edge a handle is aimed at disappears.
            let edge = contrast(p.trim_range_edge, kept);
            assert!(
                edge >= 3.0,
                "{name}: the range's edge rules are {edge:.2}:1 against their own fill"
            );

            // The out-point amber is the only handle carried by hue, and
            // it is painted on the bare well.
            let amber = contrast(p.trim_out, track);
            assert!(
                amber >= 3.0,
                "{name}: the out-point handle is {amber:.2}:1 against the well"
            );

            // The playhead is cased in the well's dark, so the ground it
            // is measured against is that casing rather than whatever it
            // is crossing. Without the casing this is 1.44:1 on the
            // selection — the marker fades out over exactly the range
            // being trimmed. See `draw_into`.
            let head = contrast(p.playhead, track);
            assert!(
                head >= 3.0,
                "{name}: the playhead is {head:.2}:1 against its casing"
            );
            // ...and the casing has to be doing real work on the ground it
            // separates the red from, or it is decoration that could be
            // deleted without anything appearing to change.
            let casing = contrast(track, kept);
            assert!(
                casing >= 3.0,
                "{name}: the playhead's casing is {casing:.2}:1 against the selection — \
                 it no longer separates the red line from what it crosses"
            );

            // The grip rules are cut in the well's own dark, and must read
            // on both thumbs — the white in-point and the amber out-point.
            for (thumb_name, thumb) in
                [("the in-point thumb", p.trim_range_edge), ("the out-point thumb", p.trim_out)]
            {
                let grip = contrast(p.trim_track, thumb);
                assert!(
                    grip >= 3.0,
                    "{name}: the grip is {grip:.2}:1 on {thumb_name} — the mark that says \
                     'draggable' is not visible on the handle it is cut into"
                );
            }
        }
    }

    /// The widget's headline case must be **visible**, not merely
    /// hit-testable.
    ///
    /// # The defect this pins
    ///
    /// This module's header says the bar exists so that "a 3-second
    /// selection on a 101-minute source" stays addressable, and
    /// `both_handles_are_on_screen_for_a_feature_length_source` proves
    /// both handles land on screen. Both were true, and the control still
    /// rendered as a **single amber dot**: at that ratio the selection is
    /// 1.15px wide, so the blue fill, its two white rules, and the entire
    /// in-point thumb disappeared beneath the out-point thumb.
    ///
    /// Every existing test passed. They checked positions and hit zones —
    /// nothing asserted that the thing being selected could be *seen*.
    #[test]
    fn a_tiny_selection_on_a_long_source_is_still_visible() {
        // 8 seconds kept from 101 minutes: the case the header names.
        let d = data(101.0 * 60.0, 300.0, 308.0);
        let in_x = d.x_of(d.in_point, W);
        let out_x = d.x_of(d.out_point, W);

        // The true geometry really is sub-pixel — this is the premise.
        assert!(
            out_x - in_x < 2.0,
            "this test needs a selection narrower than the handles; got {:.2}px",
            out_x - in_x
        );

        // What `draw_into` actually paints.
        let drawn = (out_x - in_x).max(MIN_VISIBLE_RANGE);
        let drawn_out_x = out_x.max(in_x + drawn);

        // The two thumbs must not be stacked on the same pixel.
        let separation = drawn_out_x - in_x;
        assert!(
            separation >= HANDLE_RADIUS * 2.0,
            "the in and out thumbs are {separation:.2}px apart but each is \
             {:.0}px across — one is drawn on top of the other and the selection \
             reads as a single mark",
            HANDLE_RADIUS * 2.0
        );

        // ...and the selection itself must be wide enough to read as a
        // band rather than as a hairline between two handles.
        assert!(
            drawn >= MIN_VISIBLE_RANGE,
            "the kept range is drawn {drawn:.2}px wide, below the {MIN_VISIBLE_RANGE:.0}px floor"
        );
    }

    /// Widening the *drawing* must never widen the **edit**.
    ///
    /// The floor above is a paint-time minimum, and it is only defensible
    /// because the values it draws from are untouched. The product rules allows
    /// the preview to render a mark legibly; it does not allow the app to
    /// change which frames are exported. A future "simplification" that
    /// clamped `clamp_in`/`clamp_out` to the same floor would silently
    /// lengthen every short trim, so this pins the separation.
    #[test]
    fn the_visible_minimum_never_alters_the_actual_range() {
        let d = data(101.0 * 60.0, 300.0, 308.0);

        // The exported range is exactly 8 seconds, whatever is drawn.
        let span = d.range_duration().as_secs_f64();
        assert!(
            (span - 8.0).abs() < 1e-6,
            "the selection reports {span}s, but the user chose 8s — the drawing floor \
             has leaked into the edit"
        );

        // And the clamps still permit a range far narrower than the floor.
        let tight = d.clamp_out(secs(300.1));
        assert!(
            tight.as_secs_f64() - 300.0 <= 0.2,
            "clamp_out widened a 0.1s selection to {}s", tight.as_secs_f64() - 300.0
        );
    }

    /// Renders the bar at a **partial** trim, which is the state it is
    /// actually used in and the one a screenshot of a freshly-opened file
    /// cannot show.
    ///
    /// On open, `in`/`out` span the whole source, so the selection fills
    /// the track and the excluded material has zero width — the control
    /// looks like a solid slab and none of its figure/ground reasoning is
    /// visible. This renders 8 seconds kept out of 101, which is the case
    /// the whole widget exists for.
    ///
    /// `--ignored`, like the bump sheet: it writes a file and is a visual
    /// aid, not a gate. The contrast *gates* are unit tests above.
    ///
    /// # What this sheet cannot show
    ///
    /// It **reimplements** the drawing rather than calling `draw_into` —
    /// the loop below is a per-pixel model, and it models the track as a
    /// plain rectangle with no corner radius at all. So it agrees with
    /// the renderer on colour and composition, and says nothing about
    /// geometry.
    ///
    /// That is not academic: the square-fill-over-rounded-corner defect
    /// (see `a_square_fill_would_overhang_the_wells_rounded_corner`)
    /// was invisible here, because a sheet with square corners cannot
    /// render a chamfered one. A user screenshot found it. Treat this
    /// file as a colour proof, and do not read the absence of a visual
    /// defect in it as evidence the renderer is correct.
    #[test]
    #[ignore]
    fn write_trimmed_state_sheet() {
        const W_PX: u32 = 900;
        const H_PX: u32 = 60;

        let p = Palette::DARK;
        let d = TrimBarData {
            palette: p,
            source_duration: secs(101.0 * 60.0),
            in_point: secs(300.0),
            out_point: secs(308.0),
            playhead: secs(304.0),
        };

        let w = W_PX as f32;
        let track_y = (H_PX as f32 - TRACK_HEIGHT) / 2.0;
        let in_x = d.x_of(d.in_point, w);
        // Same widening `draw_into` applies, so the sheet shows what the
        // renderer shows rather than the raw sub-pixel geometry.
        let range_w = (d.x_of(d.out_point, w) - in_x).max(MIN_VISIBLE_RANGE);
        let out_x = in_x + range_w;
        let head_x = d.x_of(d.playhead, w).max(in_x).min(out_x);

        let mut img = vec![0u8; (W_PX * H_PX * 3) as usize];
        let put = |img: &mut Vec<u8>, x: u32, y: u32, c: iced::Color| {
            let i = ((y * W_PX + x) * 3) as usize;
            let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
            img[i..i + 3].copy_from_slice(&[b(c.r), b(c.g), b(c.b)]);
        };
        // Source-over, so the semi-transparent excluded wash is composited
        // exactly as the renderer does it.
        let over = |fg: iced::Color, bg: iced::Color| iced::Color {
            r: fg.r * fg.a + bg.r * (1.0 - fg.a),
            g: fg.g * fg.a + bg.g * (1.0 - fg.a),
            b: fg.b * fg.a + bg.b * (1.0 - fg.a),
            a: 1.0,
        };

        for y in 0..H_PX {
            for x in 0..W_PX {
                let (fx, fy) = (x as f32, y as f32);
                let in_track = fy >= track_y
                    && fy < track_y + TRACK_HEIGHT
                    && fx >= EDGE_INSET
                    && fx < w - EDGE_INSET;

                let mut c = p.surface;
                if in_track {
                    c = if fx >= in_x && fx < out_x {
                        p.trim_range_fill
                    } else {
                        over(p.trim_range_excluded, p.trim_track)
                    };
                    // The 2px rules bounding the kept range.
                    let on_rule = fx >= in_x
                        && fx < out_x
                        && (fy < track_y + 2.0 || fy >= track_y + TRACK_HEIGHT - 2.0);
                    if on_rule {
                        c = p.trim_range_edge;
                    }
                }
                // The cased playhead, overhanging the track.
                let head_span = fy >= track_y - 4.0 && fy < track_y + TRACK_HEIGHT + 4.0;
                if head_span && (fx - head_x).abs() < 2.0 {
                    c = p.trim_track;
                }
                if head_span && (fx - head_x).abs() < 1.0 {
                    c = p.playhead;
                }
                // The two thumbs.
                let cy = track_y + TRACK_HEIGHT / 2.0;
                for (hx, col) in [(in_x, p.trim_range_edge), (out_x, p.trim_out)] {
                    let (dx, dy) = ((fx - hx) / HANDLE_RADIUS, (fy - cy) / HANDLE_RADIUS);
                    if dx * dx + dy * dy <= 1.0 {
                        c = col;
                    }
                }
                put(&mut img, x, y, c);
            }
        }

        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/review/trim-bar-trimmed.ppm");
        let mut out = format!("P6\n{W_PX} {H_PX}\n255\n").into_bytes();
        out.extend_from_slice(&img);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).expect("create review dir");
        }
        std::fs::write(&path, out).expect("write trimmed-state sheet");
        eprintln!("wrote {}", path.display());
    }

    #[test]
    fn duration_formatting_switches_to_hours_only_when_needed() {
        assert_eq!(fmt_duration(secs(2.0)), "0:02");
        assert_eq!(fmt_duration(secs(101.0)), "1:41");
        assert_eq!(fmt_duration(secs(3661.0)), "1:01:01");
    }

    /// The reference readout reads `0:02 / 1:41`; this asserts the exact
    /// pair that image shows, so the format cannot drift away from the
    /// thing it was specified against.
    #[test]
    fn the_readout_matches_the_reference_format() {
        let playhead = fmt_duration(secs(2.0));
        let total = fmt_duration(secs(101.0));
        assert_eq!(format!("{playhead} / {total}"), "0:02 / 1:41");
    }
}
