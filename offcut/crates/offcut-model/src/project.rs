//! the data model and §4.1's edit operations (split / trim /
//! ripple delete), implemented as plain functions over `Project` so every
//! operation is a unit-testable state transition with no I/O, no
//! GStreamer, no wgpu — per §2's crate-layout rationale.

use crate::adjust::AdjustSettings;
use crate::crop::CropTransform;
use crate::error::EditError;
use crate::ids::{ClipId, SourceId};
use crate::speed::Speed;
use crate::time::{Rational, Time};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    pub path: PathBuf,
    pub duration: Time,
    pub fps: Rational,
    pub resolution: (u32, u32),
    pub has_audio: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub source: SourceId,
    /// In SOURCE time, per the design rule
    pub in_point: Time,
    /// In SOURCE time, exclusive, per the design rule
    pub out_point: Time,
    pub speed: Speed,
    pub muted: bool,
    pub volume: f32, // 0.0 to 1.0
    pub crop: CropTransform,
    pub adjust: AdjustSettings,
}

impl Clip {
    /// Source span (out - in), always well-defined because every
    /// `Project`-mutating function in this module upholds `out > in` as an
    /// invariant and returns `Err` rather than construct a violating clip.
    pub fn source_span(&self) -> Time {
        self.out_point
            .checked_sub(self.in_point)
            .expect("Clip invariant violated: out_point <= in_point")
    }

    /// Source span / speed factor — the `timeline_duration`.
    pub fn timeline_duration(&self) -> Time {
        self.source_span().div_f64(self.speed.factor())
    }

    /// The design rule: per-clip mute is a stored flag consulted by the
    /// engine and export path — never a destructive removal. §4.3 adds:
    /// 4x speed implies mute regardless of the stored flag, so the
    /// *effective* muted state (what the engine/export should actually
    /// honor) is this, not `self.muted` directly.
    pub fn effective_muted(&self) -> bool {
        self.muted || self.speed.implies_mute()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Project {
    pub sources: Vec<Source>,
    /// Ordered = timeline order, per the design rule
    pub clips: Vec<Clip>,
    pub master_muted: bool,
}

impl Project {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(&mut self, source: Source) {
        self.sources.push(source);
    }

    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }

    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }

    /// Mutable counterpart to `clip`. Added for the UI layer (`offcut-ui`
    /// Phase 2): toggling speed/mute from a click needs to mutate a
    /// specific clip in place, and going through a full `EditError`-typed
    /// operation for "flip this bool" would be needless ceremony for a
    /// mutation that cannot violate the `out > in` invariant (unlike
    /// `trim_clip`/`split_clip`, no field reachable through this handle
    /// affects that invariant... except `in_point`/`out_point` themselves.
    /// This is deliberately `pub` and unguarded, not a design contradiction:
    /// `offcut-model` stays pure/IO-free either way, and the UI is
    /// responsible for using `trim_clip`/`split_clip` (not this) whenever
    /// it needs to move `in_point`/`out_point` specifically, exactly as
    /// The design rule describes those as the sanctioned mutation path.
    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }

    fn clip_index(&self, id: ClipId) -> Result<usize, EditError> {
        self.clips
            .iter()
            .position(|c| c.id == id)
            .ok_or(EditError::ClipNotFound(id))
    }

    /// Append a new full-length clip covering a source's entire duration.
    /// This is the "import" entry point at the model layer; offcut-engine
    /// is responsible for actually probing a file's duration/fps/etc.
    /// before calling this — this function does not touch the filesystem
    /// (the design rule: offcut-model is pure).
    pub fn add_clip_for_source(&mut self, source_id: SourceId) -> Result<ClipId, EditError> {
        let source = self.source(source_id).ok_or(EditError::SourceNotFound(source_id))?;
        let id = ClipId::next();
        self.clips.push(Clip {
            id,
            source: source_id,
            in_point: Time::ZERO,
            out_point: source.duration,
            speed: Speed::default(),
            muted: false,
            volume: 1.0,
            crop: CropTransform::identity(),
            adjust: AdjustSettings::default(),
        });
        Ok(id)
    }

    /// The design rule: "Split at playhead... Splits the clip under the
    /// playhead into `[in, playhead)` and `[playhead, out)`. No re-encode,
    /// no re-decode — pure model mutation." `playhead` is in SOURCE time
    /// (the same space as `in_point`/`out_point`) — callers translate from
    /// timeline time before calling this, because that translation depends
    /// on every preceding clip's speed and is exactly the kind of
    /// engine/UI-layer concern offcut-model stays out of.
    ///
    /// Returns the id of the newly created second half; `clip_id` continues
    /// to refer to the first half.
    pub fn split_clip(&mut self, clip_id: ClipId, playhead_source_time: Time) -> Result<ClipId, EditError> {
        let idx = self.clip_index(clip_id)?;
        let clip = &self.clips[idx];

        // Strictly inside (in, out): a split exactly on a boundary
        // produces a zero-length half, which is the exact invariant §8
        // forbids ("must never produce a clip with out <= in").
        if playhead_source_time <= clip.in_point || playhead_source_time >= clip.out_point {
            return Err(EditError::SplitPointOutsideClip(clip_id));
        }

        let mut second_half = clip.clone();
        second_half.id = ClipId::next();
        second_half.in_point = playhead_source_time;

        self.clips[idx].out_point = playhead_source_time;
        self.clips.insert(idx + 1, second_half.clone());
        Ok(second_half.id)
    }

    /// The design rule: "Trim: drag the rounded handle at either end of the
    /// selected clip." `new_in`/`new_out` are `Option` so a caller can move
    /// just one handle; `None` leaves that end unchanged. Validates against
    /// both the clip-invariant (`out > in`) and the source's actual
    /// duration (`EditError::TrimOutsideSourceDuration`) — the /// property test requires both to hold for every random trim.
    pub fn trim_clip(
        &mut self,
        clip_id: ClipId,
        new_in: Option<Time>,
        new_out: Option<Time>,
    ) -> Result<(), EditError> {
        let idx = self.clip_index(clip_id)?;
        let source_id = self.clips[idx].source;
        let source_duration = self
            .source(source_id)
            .ok_or(EditError::SourceNotFound(source_id))?
            .duration;

        let candidate_in = new_in.unwrap_or(self.clips[idx].in_point);
        let candidate_out = new_out.unwrap_or(self.clips[idx].out_point);

        if candidate_out > source_duration {
            return Err(EditError::TrimOutsideSourceDuration(source_id));
        }
        if candidate_out <= candidate_in {
            return Err(EditError::TrimWouldInvertClip(clip_id));
        }

        self.clips[idx].in_point = candidate_in;
        self.clips[idx].out_point = candidate_out;
        Ok(())
    }

    /// Replace the timeline with **exactly one clip covering `[in, out)`
    /// of `source_id`** — the "cut a piece out of a long video" operation.
    ///
    /// # Why this is its own operation rather than add-then-trim
    ///
    /// The product's first job to be done is "trim the dead air", and the
    /// dominant real case is one long source (a screen recording, a
    /// movie, a phone clip) that the user wants a short range out of.
    /// Expressing that as `add_clip_for_source` followed by `trim_clip`
    /// works, but it is two steps that can each half-fail, and it leaves
    /// the intermediate state — a clip spanning the *entire* 101-minute
    /// source — briefly real. Undo then has to walk back through it.
    ///
    /// As one operation it is atomic: it either produces the requested
    /// range or changes nothing, so a single undo returns exactly where
    /// the user was, and the range is validated *before* anything is
    /// mutated.
    ///
    /// The source file itself is never touched — the product's
    /// non-negotiable "Never mutates a source file. Ever." A range is a
    /// pair of numbers on a clip, which is why this is instant regardless
    /// of how long the source is.
    pub fn set_range(
        &mut self,
        source_id: SourceId,
        in_point: Time,
        out_point: Time,
    ) -> Result<ClipId, EditError> {
        let source = self.source(source_id).ok_or(EditError::SourceNotFound(source_id))?;
        let source_duration = source.duration;

        // Validate everything before mutating, so a rejected range leaves
        // the project byte-for-byte as it was.
        if out_point > source_duration {
            return Err(EditError::TrimOutsideSourceDuration(source_id));
        }
        if out_point <= in_point {
            return Err(EditError::EmptyRange {
                in_nanos: in_point.as_nanos(),
                out_nanos: out_point.as_nanos(),
            });
        }

        let clip = Clip {
            id: ClipId::next(),
            source: source_id,
            in_point,
            out_point,
            speed: Speed::default(),
            muted: false,
            volume: 1.0,
            crop: CropTransform::default(),
            adjust: AdjustSettings::default(),
        };
        let id = clip.id;
        self.clips = vec![clip];
        Ok(id)
    }

    /// The design rule: "Ripple delete: Del removes the clip and closes the
    /// gap. There are no gaps in v1 — the timeline is a sequence, not a
    /// canvas." Because `clips` is a plain ordered `Vec`, removal alone
    /// closes the gap — there is no explicit position field to update,
    /// which is the whole point of representing the timeline this way.
    pub fn ripple_delete(&mut self, clip_id: ClipId) -> Result<(), EditError> {
        let idx = self.clip_index(clip_id)?;
        self.clips.remove(idx);
        Ok(())
    }

    /// Total timeline duration: sum of every clip's `timeline_duration`.
    /// the property test: "a total timeline duration inconsistent
    /// with the sum of its parts" must never occur — this function *is*
    /// that sum, so any test comparing it against a manually-tracked total
    /// is testing that no operation silently drops or duplicates a clip.
    pub fn total_timeline_duration(&self) -> Time {
        self.clips
            .iter()
            .fold(Time::ZERO, |acc, c| acc.checked_add(c.timeline_duration()).unwrap_or(acc))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_source(duration_secs: u64) -> Source {
        Source {
            id: SourceId::next(),
            path: PathBuf::from("/tmp/test.mp4"),
            duration: Time::from_nanos(duration_secs * 1_000_000_000),
            fps: Rational::NTSC,
            resolution: (1920, 1080),
            has_audio: true,
        }
    }

    fn project_with_one_clip(duration_secs: u64) -> (Project, ClipId) {
        let mut p = Project::new();
        let source = test_source(duration_secs);
        let source_id = source.id;
        p.add_source(source);
        let clip_id = p.add_clip_for_source(source_id).unwrap();
        (p, clip_id)
    }

    fn secs(s: f64) -> Time {
        Time::from_nanos((s * 1e9) as u64)
    }

    /// The headline case this operation exists for: pull a short range out
    /// of a long source. 101 minutes in, 8 seconds out.
    #[test]
    fn set_range_cuts_a_short_clip_out_of_a_very_long_source() {
        let mut p = Project::new();
        let source = test_source(101 * 60); // a feature-length file
        let source_id = source.id;
        p.add_source(source);

        let id = p.set_range(source_id, secs(300.0), secs(308.0)).expect("set_range failed");

        assert_eq!(p.clips.len(), 1, "the timeline should hold exactly the requested range");
        let clip = p.clip(id).expect("the returned id must address the new clip");
        assert_eq!(clip.in_point, secs(300.0));
        assert_eq!(clip.out_point, secs(308.0));
        assert_eq!(clip.source_span(), secs(8.0));
        assert!(
            (p.total_timeline_duration().as_secs_f64() - 8.0).abs() < 1e-9,
            "the timeline is the range's length, not the source's"
        );
    }

    /// The product rules: "Never mutates a source file. Ever." At the model
    /// layer that means the `Source` record — its path and duration — is
    /// untouched by an edit; only the clip's numbers change.
    #[test]
    fn set_range_leaves_the_source_record_completely_untouched() {
        let mut p = Project::new();
        let source = test_source(600);
        let source_id = source.id;
        let original = source.clone();
        p.add_source(source);

        p.set_range(source_id, secs(10.0), secs(20.0)).expect("set_range");

        let after = p.source(source_id).expect("source must still exist");
        assert_eq!(after.path, original.path);
        assert_eq!(after.duration, original.duration, "the source's own duration must not change");
        assert_eq!(after.resolution, original.resolution);
    }

    /// The atomicity claim in `set_range`'s doc comment, tested rather
    /// than asserted: a rejected range must leave the project exactly as
    /// it was, not half-applied.
    #[test]
    fn a_rejected_range_changes_nothing_at_all() {
        let (mut p, _) = project_with_one_clip(10);
        let source_id = p.sources[0].id;
        let before = p.clips.clone();

        // Past the end of the source.
        let err = p.set_range(source_id, secs(1.0), secs(99.0)).unwrap_err();
        assert!(matches!(err, EditError::TrimOutsideSourceDuration(_)), "got {err:?}");
        assert_eq!(p.clips, before, "a rejected range must not touch the timeline");

        // Inverted.
        let err = p.set_range(source_id, secs(5.0), secs(5.0)).unwrap_err();
        assert!(matches!(err, EditError::EmptyRange { .. }), "got {err:?}");
        assert_eq!(p.clips, before, "an empty range must not touch the timeline either");
    }

    #[test]
    fn set_range_on_an_unknown_source_is_an_error_not_a_panic() {
        let mut p = Project::new();
        let orphan = SourceId::next();
        let err = p.set_range(orphan, secs(0.0), secs(1.0)).unwrap_err();
        assert!(matches!(err, EditError::SourceNotFound(_)), "got {err:?}");
        assert!(p.clips.is_empty());
    }

    /// Re-ranging replaces rather than accumulates: dragging the trim
    /// handles repeatedly must not pile up clips.
    #[test]
    fn set_range_replaces_the_previous_range_rather_than_appending() {
        let mut p = Project::new();
        let source = test_source(60);
        let source_id = source.id;
        p.add_source(source);

        p.set_range(source_id, secs(1.0), secs(2.0)).unwrap();
        p.set_range(source_id, secs(10.0), secs(20.0)).unwrap();

        assert_eq!(p.clips.len(), 1, "each range replaces the last");
        assert_eq!(p.clips[0].in_point, secs(10.0));
        assert_eq!(p.clips[0].out_point, secs(20.0));
    }

    /// The full-source range is legal and is the natural initial state
    /// when a file is first opened.
    #[test]
    fn set_range_accepts_the_entire_source_exactly() {
        let mut p = Project::new();
        let source = test_source(30);
        let source_id = source.id;
        let duration = source.duration;
        p.add_source(source);

        let id = p.set_range(source_id, Time::ZERO, duration).expect("full range must be legal");
        assert_eq!(p.clip(id).unwrap().source_span(), duration);
    }

    #[test]
    fn add_clip_covers_full_source_duration() {
        let (p, clip_id) = project_with_one_clip(60);
        let clip = p.clip(clip_id).unwrap();
        assert_eq!(clip.in_point, Time::ZERO);
        assert_eq!(clip.out_point.as_nanos(), 60_000_000_000);
        assert_eq!(clip.timeline_duration(), clip.source_span());
    }

    #[test]
    fn split_at_midpoint_produces_two_clips_summing_to_original() {
        let (mut p, clip_id) = project_with_one_clip(60);
        let mid = Time::from_nanos(30_000_000_000);
        let second_id = p.split_clip(clip_id, mid).unwrap();

        assert_eq!(p.clips.len(), 2);
        let first = p.clip(clip_id).unwrap();
        let second = p.clip(second_id).unwrap();
        assert_eq!(first.out_point, mid);
        assert_eq!(second.in_point, mid);
        assert_eq!(
            first.source_span().checked_add(second.source_span()),
            Some(Time::from_nanos(60_000_000_000))
        );
    }

    #[test]
    fn split_at_or_outside_boundary_is_rejected() {
        let (mut p, clip_id) = project_with_one_clip(60);
        let clip = p.clip(clip_id).unwrap().clone();

        assert_eq!(
            p.split_clip(clip_id, clip.in_point),
            Err(EditError::SplitPointOutsideClip(clip_id))
        );
        assert_eq!(
            p.split_clip(clip_id, clip.out_point),
            Err(EditError::SplitPointOutsideClip(clip_id))
        );
        assert_eq!(
            p.split_clip(clip_id, Time::from_nanos(999_000_000_000)),
            Err(EditError::SplitPointOutsideClip(clip_id))
        );
    }

    #[test]
    fn split_never_produces_zero_length_or_inverted_clip() {
        let (mut p, clip_id) = project_with_one_clip(10);
        // Split as close to the start as possible without hitting it.
        let near_start = Time::from_nanos(1);
        let second_id = p.split_clip(clip_id, near_start).unwrap();
        for c in [p.clip(clip_id).unwrap(), p.clip(second_id).unwrap()] {
            assert!(c.out_point > c.in_point, "clip {:?} has out <= in", c.id);
        }
    }

    #[test]
    fn trim_shrinks_clip_and_rejects_inversion() {
        let (mut p, clip_id) = project_with_one_clip(60);
        p.trim_clip(clip_id, Some(Time::from_nanos(5_000_000_000)), None).unwrap();
        assert_eq!(p.clip(clip_id).unwrap().in_point.as_nanos(), 5_000_000_000);

        // Trimming out_point to before in_point must be rejected, not
        // silently clamped.
        let result = p.trim_clip(clip_id, None, Some(Time::from_nanos(1_000_000_000)));
        assert_eq!(result, Err(EditError::TrimWouldInvertClip(clip_id)));
        // And the clip must be unchanged after the rejected call.
        assert_eq!(p.clip(clip_id).unwrap().in_point.as_nanos(), 5_000_000_000);
    }

    #[test]
    fn trim_beyond_source_duration_is_rejected() {
        let (mut p, clip_id) = project_with_one_clip(10);
        let result = p.trim_clip(clip_id, None, Some(Time::from_nanos(999_000_000_000)));
        assert!(matches!(result, Err(EditError::TrimOutsideSourceDuration(_))));
    }

    #[test]
    fn ripple_delete_closes_the_gap_with_no_explicit_position_field() {
        let mut p = Project::new();
        let source = test_source(90);
        let source_id = source.id;
        p.add_source(source);
        let a = p.add_clip_for_source(source_id).unwrap();
        p.trim_clip(a, None, Some(Time::from_nanos(30_000_000_000))).unwrap();
        let b = p.add_clip_for_source(source_id).unwrap();
        p.trim_clip(b, Some(Time::from_nanos(30_000_000_000)), Some(Time::from_nanos(60_000_000_000)))
            .unwrap();
        let c = p.add_clip_for_source(source_id).unwrap();
        p.trim_clip(c, Some(Time::from_nanos(60_000_000_000)), Some(Time::from_nanos(90_000_000_000)))
            .unwrap();

        assert_eq!(p.clips.len(), 3);
        p.ripple_delete(b).unwrap();
        assert_eq!(p.clips.len(), 2);
        // Order preserved, b truly gone, no gap object left behind.
        assert_eq!(p.clips[0].id, a);
        assert_eq!(p.clips[1].id, c);
    }

    #[test]
    fn ripple_delete_unknown_clip_errors() {
        let mut p = Project::new();
        let bogus = ClipId::from_raw_for_test(999_999);
        assert_eq!(p.ripple_delete(bogus), Err(EditError::ClipNotFound(bogus)));
    }

    #[test]
    fn effective_muted_true_at_4x_even_if_muted_flag_is_false() {
        let (mut p, clip_id) = project_with_one_clip(10);
        {
            let clip = p.clips.iter_mut().find(|c| c.id == clip_id).unwrap();
            clip.speed = Speed::Four;
            clip.muted = false;
        }
        assert!(p.clip(clip_id).unwrap().effective_muted());
    }

    #[test]
    fn total_timeline_duration_matches_sum_of_parts_after_mixed_speed_split() {
        let (mut p, clip_id) = project_with_one_clip(60);
        let second_id = p.split_clip(clip_id, Time::from_nanos(20_000_000_000)).unwrap();
        {
            let first = p.clips.iter_mut().find(|c| c.id == clip_id).unwrap();
            first.speed = Speed::Two; // 20s source -> 10s timeline
        }
        {
            let second = p.clips.iter_mut().find(|c| c.id == second_id).unwrap();
            second.speed = Speed::Half; // 40s source -> 80s timeline
        }
        let expected: u64 = 10_000_000_000 + 80_000_000_000;
        assert_eq!(p.total_timeline_duration().as_nanos(), expected);

        let manual_sum: u64 = p.clips.iter().map(|c| c.timeline_duration().as_nanos()).sum();
        assert_eq!(p.total_timeline_duration().as_nanos(), manual_sum);
    }

    // --- Property-style sweep: the design rule ---
    // "random sequences of split/trim/delete/speed must never produce a
    // clip with out <= in, a negative duration, or a total timeline
    // duration inconsistent with the sum of its parts."
    //
    // Implemented as a deterministic pseudo-random sweep (a fixed LCG) so
    // the test is reproducible without adding the `proptest` dependency
    // yet; every clip's invariant is checked after every operation, not
    // just at the end, so a single bad intermediate state fails loudly at
    // the operation that caused it.
    #[test]
    fn property_random_split_trim_delete_sequence_never_violates_invariants() {
        fn lcg_next(state: &mut u64) -> u64 {
            *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            *state >> 33
        }

        let mut rng_state: u64 = 0xC0FFEE_u64;
        let mut p = Project::new();
        let source = test_source(600); // 10 minutes
        let source_id = source.id;
        p.add_source(source);
        let first = p.add_clip_for_source(source_id).unwrap();
        let _ = first;

        for _step in 0..500 {
            if p.clips.is_empty() {
                p.add_clip_for_source(source_id).unwrap();
                continue;
            }
            let clip_idx = (lcg_next(&mut rng_state) as usize) % p.clips.len();
            let clip_id = p.clips[clip_idx].id;
            let op = lcg_next(&mut rng_state) % 4;

            match op {
                0 => {
                    // split at a pseudo-random point inside the clip's span
                    let clip = p.clip(clip_id).unwrap();
                    let span = clip.source_span().as_nanos();
                    if span < 4 {
                        continue; // too small to split meaningfully
                    }
                    let offset = 1 + (lcg_next(&mut rng_state) % (span - 2));
                    let point = Time::from_nanos(clip.in_point.as_nanos() + offset);
                    let _ = p.split_clip(clip_id, point); // Err is fine, must not corrupt state
                }
                1 => {
                    // trim in_point forward by a small random amount
                    let clip = p.clip(clip_id).unwrap();
                    let span = clip.source_span().as_nanos();
                    if span < 4 {
                        continue;
                    }
                    let delta = lcg_next(&mut rng_state) % (span / 2).max(1);
                    let new_in = Time::from_nanos(clip.in_point.as_nanos() + delta);
                    let _ = p.trim_clip(clip_id, Some(new_in), None);
                }
                2 => {
                    let _ = p.ripple_delete(clip_id);
                }
                _ => {
                    if let Some(clip) = p.clips.iter_mut().find(|c| c.id == clip_id) {
                        clip.speed = Speed::ALL[(lcg_next(&mut rng_state) as usize) % 4];
                    }
                }
            }

            // Invariant check after every single operation.
            for c in &p.clips {
                assert!(
                    c.out_point > c.in_point,
                    "invariant violated after op {op}: clip {:?} has out {:?} <= in {:?}",
                    c.id,
                    c.out_point,
                    c.in_point
                );
            }
            let manual_sum: u64 = p.clips.iter().map(|c| c.timeline_duration().as_nanos()).sum();
            assert_eq!(
                p.total_timeline_duration().as_nanos(),
                manual_sum,
                "total duration drifted from sum-of-parts after op {op}"
            );
        }
    }
}
