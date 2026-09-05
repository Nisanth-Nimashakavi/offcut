//! Timeline ⇄ source time mapping.
//!
//! `Project::split_clip` takes a playhead in **source** time and says so
//! explicitly: "callers translate from timeline time before calling this,
//! because that translation depends on every preceding clip's speed and is
//! exactly the kind of engine/UI-layer concern offcut-model stays out of."
//!
//! That was the right call about *where the policy lives* and the wrong
//! call about *where the arithmetic lives*. The translation is pure
//! integer math over `Clip::timeline_duration`, it has exactly the same
//! off-by-one-frame hazards `time.rs` exists to guard against, and it was
//! about to be written twice — once in `offcut-ui` for the playhead and
//! once in `offcut-export` for segment boundaries — which is how the two
//! drift apart and an exported cut lands one frame away from the previewed
//! one. So the math lives here, tested, and the *decision* to call it
//! still belongs to the UI.
//!
//! Two coordinate spaces, named consistently across the whole workspace:
//!
//! - **timeline time** — position in the edited sequence, after speed is
//!   applied. This is what the ruler, the playhead, and the transport
//!   timecode show.
//! - **source time** — position within a source file, before speed. This
//!   is what `Clip::in_point`/`out_point` store and what a `gst` seek
//!   wants.

use crate::ids::ClipId;
use crate::project::Project;
use crate::time::Time;

/// Where a timeline instant lands: which clip, and where inside it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct TimelinePosition {
    /// Index into `Project::clips`.
    pub clip_index: usize,
    pub clip_id: ClipId,
    /// Where this clip starts on the timeline.
    pub clip_start: Time,
    /// Offset into the clip, in TIMELINE time (speed already applied).
    pub offset_in_clip: Time,
    /// The same instant expressed in the clip's SOURCE time — this is the
    /// value `split_clip`, `trim_clip`, and a `gst` seek all want.
    pub source_time: Time,
}

impl Project {
    /// Timeline start time of the clip at `index`: the sum of every
    /// preceding clip's `timeline_duration`.
    pub fn clip_start_time(&self, index: usize) -> Time {
        self.clips
            .iter()
            .take(index)
            .fold(Time::ZERO, |acc, c| {
                acc.checked_add(c.timeline_duration()).unwrap_or(acc)
            })
    }

    /// Every clip's timeline start, in order. One pass instead of the
    /// quadratic repeated-`clip_start_time` shape the timeline widget
    /// would otherwise use while laying out N clips per redraw.
    pub fn clip_start_times(&self) -> Vec<Time> {
        let mut acc = Time::ZERO;
        let mut out = Vec::with_capacity(self.clips.len());
        for clip in &self.clips {
            out.push(acc);
            acc = acc.checked_add(clip.timeline_duration()).unwrap_or(acc);
        }
        out
    }

    /// Resolve a timeline instant to a clip and a source time.
    ///
    /// Returns `None` for an empty project or a time at/after the end of
    /// the timeline — deliberately **not** clamping to the last clip. A
    /// playhead parked past the end is a real state (playback just
    /// finished) and the caller should render it as "past the end", not as
    /// "on the final frame of the last clip", which would make the split
    /// button offer to split a clip the playhead is not actually inside.
    ///
    /// The end of the timeline is exclusive for the same reason
    /// `Clip::out_point` is: a position exactly on a boundary belongs to
    /// the clip that *starts* there, never to the one that ends there.
    pub fn resolve_timeline_time(&self, timeline_time: Time) -> Option<TimelinePosition> {
        let mut clip_start = Time::ZERO;
        for (clip_index, clip) in self.clips.iter().enumerate() {
            let duration = clip.timeline_duration();
            let clip_end = clip_start.checked_add(duration)?;
            if timeline_time < clip_end {
                let offset_in_clip = timeline_time.saturating_sub(clip_start);
                // Timeline offset -> source offset is a multiply by the
                // speed factor (the inverse of `timeline_duration`'s
                // divide), then add the clip's in_point.
                let source_offset =
                    Time::from_nanos((offset_in_clip.as_nanos() as f64 * clip.speed.factor()).round() as u64);
                let source_time = clip
                    .in_point
                    .checked_add(source_offset)
                    // A rounding overshoot at the very last nanosecond of
                    // a clip must never produce a source time past
                    // out_point -- that would seek into the *next* clip's
                    // footage while still reporting this clip's index.
                    .map(|t| t.min(clip.out_point))
                    .unwrap_or(clip.out_point);
                return Some(TimelinePosition {
                    clip_index,
                    clip_id: clip.id,
                    clip_start,
                    offset_in_clip,
                    source_time,
                });
            }
            clip_start = clip_end;
        }
        None
    }

    /// Inverse of `resolve_timeline_time`: given a clip and a source time
    /// inside it, where is that on the timeline?
    ///
    /// `None` when the clip is not in this project, or `source_time` is
    /// outside its `[in_point, out_point]` span — an out-of-span answer
    /// would be a timeline position that does not exist, and silently
    /// clamping it is how a trim handle ends up dragging the playhead
    /// somewhere the user never pointed.
    pub fn timeline_time_of(&self, clip_id: ClipId, source_time: Time) -> Option<Time> {
        let index = self.clips.iter().position(|c| c.id == clip_id)?;
        let clip = &self.clips[index];
        if source_time < clip.in_point || source_time > clip.out_point {
            return None;
        }
        let source_offset = source_time.saturating_sub(clip.in_point);
        let timeline_offset = source_offset.div_f64(clip.speed.factor());
        self.clip_start_time(index).checked_add(timeline_offset)
    }

    /// Split whatever clip the playhead is inside, taking the playhead in
    /// **timeline** time — the coordinate the UI actually has.
    ///
    /// This is the operation the design rule describes as "Split at
    /// playhead"; `split_clip` is its source-time primitive. Returns
    /// `Ok(None)` (not an error) when the playhead is past the end of the
    /// timeline or exactly on a clip boundary: there is nothing to split
    /// at a boundary, and a user pressing `S` there has made a no-op, not
    /// a mistake worth an error dialog.
    pub fn split_at_timeline_time(
        &mut self,
        timeline_time: Time,
    ) -> Result<Option<ClipId>, crate::error::EditError> {
        let Some(position) = self.resolve_timeline_time(timeline_time) else {
            return Ok(None);
        };
        let clip = &self.clips[position.clip_index];
        if position.source_time <= clip.in_point || position.source_time >= clip.out_point {
            return Ok(None);
        }
        self.split_clip(position.clip_id, position.source_time).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Source;
    use crate::ids::SourceId;
    use crate::speed::Speed;
    use crate::time::Rational;

    fn secs(n: f64) -> Time {
        Time::from_nanos((n * 1_000_000_000.0) as u64)
    }

    /// One 40s source split into four 10s clips.
    fn four_clip_project() -> Project {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/t.mp4".into(),
            duration: secs(40.0),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let source_id = source.id;
        project.add_source(source);
        let a = project.add_clip_for_source(source_id).unwrap();
        let b = project.split_clip(a, secs(10.0)).unwrap();
        let c = project.split_clip(b, secs(20.0)).unwrap();
        let _d = project.split_clip(c, secs(30.0)).unwrap();
        project
    }

    #[test]
    fn clip_start_times_accumulate_in_order() {
        let project = four_clip_project();
        let starts = project.clip_start_times();
        assert_eq!(starts, vec![secs(0.0), secs(10.0), secs(20.0), secs(30.0)]);
    }

    #[test]
    fn clip_start_times_matches_the_scalar_accessor_for_every_index() {
        // The vectorized and scalar forms exist for different call sites
        // (layout vs. a single lookup); they must never disagree.
        let project = four_clip_project();
        for (i, start) in project.clip_start_times().iter().enumerate() {
            assert_eq!(*start, project.clip_start_time(i), "index {i}");
        }
    }

    #[test]
    fn resolve_finds_the_right_clip_and_source_time_at_1x() {
        let project = four_clip_project();
        let position = project.resolve_timeline_time(secs(25.0)).expect("inside clip 3");
        assert_eq!(position.clip_index, 2);
        assert_eq!(position.clip_start, secs(20.0));
        assert_eq!(position.offset_in_clip, secs(5.0));
        // Clip 3 spans source [20s, 30s); 5s into it is source 25s.
        assert_eq!(position.source_time, secs(25.0));
    }

    /// The case the whole module exists for: with a 2x clip in the middle,
    /// timeline time and source time genuinely diverge, and a naive
    /// "timeline time == source time" assumption (which is correct for an
    /// all-1x project, so it survives careless testing) is now wrong.
    #[test]
    fn resolve_accounts_for_speed_when_mapping_to_source_time() {
        let mut project = four_clip_project();
        project.clips[0].speed = Speed::Two; // 10s of source -> 5s of timeline

        let starts = project.clip_start_times();
        assert_eq!(starts, vec![secs(0.0), secs(5.0), secs(15.0), secs(25.0)]);

        // 2s into the timeline is 4s into the 2x clip's source.
        let position = project.resolve_timeline_time(secs(2.0)).expect("inside clip 1");
        assert_eq!(position.clip_index, 0);
        assert_eq!(position.source_time, secs(4.0));

        // 6s into the timeline is 1s past clip 2's start, and clip 2 is
        // 1x, so source 11s.
        let position = project.resolve_timeline_time(secs(6.0)).expect("inside clip 2");
        assert_eq!(position.clip_index, 1);
        assert_eq!(position.source_time, secs(11.0));
    }

    #[test]
    fn a_boundary_instant_belongs_to_the_clip_that_starts_there() {
        let project = four_clip_project();
        let position = project.resolve_timeline_time(secs(10.0)).expect("on boundary");
        assert_eq!(position.clip_index, 1, "10s is clip 2's first frame, not clip 1's last");
        assert_eq!(position.offset_in_clip, Time::ZERO);
    }

    #[test]
    fn past_the_end_resolves_to_none_rather_than_clamping() {
        let project = four_clip_project();
        assert_eq!(project.total_timeline_duration(), secs(40.0));
        assert!(project.resolve_timeline_time(secs(40.0)).is_none(), "end is exclusive");
        assert!(project.resolve_timeline_time(secs(99.0)).is_none());
    }

    #[test]
    fn empty_project_resolves_to_none_without_panicking() {
        let project = Project::new();
        assert!(project.resolve_timeline_time(Time::ZERO).is_none());
    }

    #[test]
    fn timeline_time_of_is_the_inverse_of_resolve() {
        let mut project = four_clip_project();
        project.clips[1].speed = Speed::Half; // 10s source -> 20s timeline

        for nanos in [0u64, 3_000_000_000, 9_999_999_999, 12_000_000_000, 29_000_000_000] {
            let t = Time::from_nanos(nanos);
            let position = project.resolve_timeline_time(t).expect("in range");
            let back = project
                .timeline_time_of(position.clip_id, position.source_time)
                .expect("round trip");
            // Round-trip through a speed factor is a float multiply then a
            // float divide, so allow a nanosecond of slack -- but only a
            // nanosecond: anything larger would be a real mapping bug, not
            // rounding.
            let delta = back.as_nanos().abs_diff(t.as_nanos());
            assert!(delta <= 1, "round trip of {t:?} came back as {back:?} (delta {delta}ns)");
        }
    }

    #[test]
    fn timeline_time_of_rejects_a_source_time_outside_the_clip() {
        let project = four_clip_project();
        let clip_id = project.clips[1].id; // source span [10s, 20s)
        assert!(project.timeline_time_of(clip_id, secs(5.0)).is_none());
        assert!(project.timeline_time_of(clip_id, secs(25.0)).is_none());
        assert!(project.timeline_time_of(clip_id, secs(15.0)).is_some());
    }

    #[test]
    fn split_at_timeline_time_splits_the_clip_under_the_playhead() {
        let mut project = four_clip_project();
        let new_id = project.split_at_timeline_time(secs(25.0)).unwrap().expect("split happened");
        assert_eq!(project.clips.len(), 5);
        // Clip 3 was source [20,30); splitting at 25 gives [20,25) + [25,30).
        assert_eq!(project.clips[2].out_point, secs(25.0));
        assert_eq!(project.clips[3].in_point, secs(25.0));
        assert_eq!(project.clips[3].id, new_id);
        // The whole point of ripple-free splitting: total duration is
        // unchanged.
        assert_eq!(project.total_timeline_duration(), secs(40.0));
    }

    #[test]
    fn split_at_a_clip_boundary_is_a_no_op_not_an_error() {
        let mut project = four_clip_project();
        assert_eq!(project.split_at_timeline_time(secs(10.0)).unwrap(), None);
        assert_eq!(project.clips.len(), 4, "no clip was split");
    }

    #[test]
    fn split_past_the_end_is_a_no_op_not_an_error() {
        let mut project = four_clip_project();
        assert_eq!(project.split_at_timeline_time(secs(40.0)).unwrap(), None);
        assert_eq!(project.split_at_timeline_time(secs(999.0)).unwrap(), None);
        assert_eq!(project.clips.len(), 4);
    }

    #[test]
    fn split_under_a_2x_clip_cuts_at_the_right_source_frame() {
        // The bug this guards: splitting a 2x clip at timeline 2s must cut
        // source 4s, not source 2s. Getting this wrong is invisible in an
        // all-1x project.
        let mut project = four_clip_project();
        project.clips[0].speed = Speed::Two;
        project.split_at_timeline_time(secs(2.0)).unwrap().expect("split happened");
        assert_eq!(project.clips[0].out_point, secs(4.0));
        assert_eq!(project.clips[1].in_point, secs(4.0));
    }
}
