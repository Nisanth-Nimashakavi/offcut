//! Undo/redo — a snapshot stack over `Project`.
//!
//! The design rule predicted this shape: "undo/redo is a plain snapshot or
//! command stack over a `Project` value." It picks snapshot, for a reason
//! worth stating rather than assuming: a command stack requires every
//! operation to have a correct hand-written inverse, and the operations
//! here (`ripple_delete` especially, which must restore a clip *at its old
//! index*) have inverses that are easy to write subtly wrong and hard to
//! test exhaustively. A `Project` is a `Vec<Source>` + `Vec<Clip>` of
//! plain `Copy`-ish fields; cloning one for a 10-minute project is a few
//! kilobytes, which is nothing against the product's 500 MB budget.
//!
//! The design system's titlebar shows undo and redo as real, dimmable buttons —
//! `can_undo`/`can_redo` exist so the UI dims them from actual state, not
//! from a guess.
//!
//! **Why the titlebar's buttons were unwired before this:** they were
//! drawn in the design render and had nothing behind them. That is the
//! exact gap this module closes.

use crate::project::Project;

/// A bounded undo/redo stack of `Project` snapshots.
///
/// The invariant, upheld by every method: `present` is always the current
/// project state, `past` is oldest-first, and `future` is
/// nearest-first (so `redo` pops, it does not shift).
#[derive(Debug, Clone)]
pub struct History {
    past: Vec<Project>,
    present: Project,
    future: Vec<Project>,
    limit: usize,
}

impl History {
    /// The product's memory bar (500 MB RSS with a 10-minute project) is
    /// nowhere near threatened by this, but an unbounded stack in a
    /// long-running session is still a leak, so it is bounded on purpose.
    pub const DEFAULT_LIMIT: usize = 100;

    pub fn new(project: Project) -> Self {
        Self { past: Vec::new(), present: project, future: Vec::new(), limit: Self::DEFAULT_LIMIT }
    }

    pub fn project(&self) -> &Project {
        &self.present
    }

    /// Direct mutable access **without** recording an undo entry. This is
    /// for transient, continuous mutation — dragging a trim handle emits a
    /// mutation per pixel of mouse movement, and recording each one would
    /// make a single drag take 200 undos to reverse. The UI pairs this
    /// with one `checkpoint()` before the drag starts.
    pub fn project_mut_uncheckpointed(&mut self) -> &mut Project {
        &mut self.present
    }

    /// Record the current state as an undo point, then hand back a mutable
    /// reference to mutate. This is the normal path for a discrete edit
    /// (split, delete, speed change, mute toggle).
    pub fn edit(&mut self) -> &mut Project {
        self.checkpoint();
        &mut self.present
    }

    /// Push the current state onto the undo stack without mutating
    /// anything. Used before a continuous gesture (see
    /// `project_mut_uncheckpointed`).
    pub fn checkpoint(&mut self) {
        self.past.push(self.present.clone());
        if self.past.len() > self.limit {
            // Drop the oldest entry. `remove(0)` is O(n) but n is bounded
            // by `limit` and this happens at most once per edit -- a
            // VecDeque would trade that for a less obvious type.
            self.past.remove(0);
        }
        // Any new edit invalidates the redo branch: this is a linear
        // history, not a tree. Silently keeping a stale future is how
        // "redo" starts replaying an edit the user has since diverged
        // from.
        self.future.clear();
    }

    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Step back one edit. Returns false (a no-op) when there is nothing
    /// to undo, so the UI can call it unconditionally on Ctrl+Z.
    pub fn undo(&mut self) -> bool {
        let Some(previous) = self.past.pop() else { return false };
        let current = std::mem::replace(&mut self.present, previous);
        self.future.push(current);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop() else { return false };
        let current = std::mem::replace(&mut self.present, next);
        self.past.push(current);
        true
    }

    /// Replace the whole project and clear the history — opening a
    /// different file. Undoing across a file open would restore clips
    /// pointing at a source that is no longer loaded, which is a worse
    /// outcome than losing the undo stack.
    pub fn reset(&mut self, project: Project) {
        self.past.clear();
        self.future.clear();
        self.present = project;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::SourceId;
    use crate::project::Source;
    use crate::speed::Speed;
    use crate::time::{Rational, Time};

    fn secs(n: u64) -> Time {
        Time::from_nanos(n * 1_000_000_000)
    }

    fn one_clip_project() -> Project {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/t.mp4".into(),
            duration: secs(20),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let source_id = source.id;
        project.add_source(source);
        project.add_clip_for_source(source_id).unwrap();
        project
    }

    #[test]
    fn a_fresh_history_can_neither_undo_nor_redo() {
        let history = History::new(one_clip_project());
        assert!(!history.can_undo());
        assert!(!history.can_redo());
    }

    #[test]
    fn edit_then_undo_restores_the_previous_state() {
        let mut history = History::new(one_clip_project());
        assert_eq!(history.project().clips.len(), 1);

        history.edit().split_at_timeline_time(secs(10)).unwrap();
        assert_eq!(history.project().clips.len(), 2);
        assert!(history.can_undo());

        assert!(history.undo());
        assert_eq!(history.project().clips.len(), 1);
        assert!(history.can_redo());
    }

    #[test]
    fn redo_reapplies_an_undone_edit() {
        let mut history = History::new(one_clip_project());
        history.edit().split_at_timeline_time(secs(10)).unwrap();
        history.undo();
        assert!(history.redo());
        assert_eq!(history.project().clips.len(), 2);
        assert!(!history.can_redo());
    }

    #[test]
    fn several_edits_undo_in_reverse_order() {
        let mut history = History::new(one_clip_project());
        history.edit().split_at_timeline_time(secs(10)).unwrap();
        history.edit().split_at_timeline_time(secs(5)).unwrap();
        history.edit().clips[0].speed = Speed::Two;

        assert_eq!(history.project().clips[0].speed, Speed::Two);
        history.undo();
        assert_eq!(history.project().clips[0].speed, Speed::One);
        assert_eq!(history.project().clips.len(), 3);
        history.undo();
        assert_eq!(history.project().clips.len(), 2);
        history.undo();
        assert_eq!(history.project().clips.len(), 1);
        assert!(!history.can_undo());
    }

    /// The linear-history rule: a new edit after an undo must discard the
    /// redo branch. Keeping it would let "redo" replay an edit from a
    /// timeline the user abandoned.
    #[test]
    fn a_new_edit_after_undo_clears_the_redo_branch() {
        let mut history = History::new(one_clip_project());
        history.edit().split_at_timeline_time(secs(10)).unwrap();
        history.undo();
        assert!(history.can_redo());

        history.edit().clips[0].muted = true;
        assert!(!history.can_redo(), "the abandoned branch must not survive a new edit");
    }

    #[test]
    fn undo_and_redo_on_an_empty_stack_are_safe_no_ops() {
        let mut history = History::new(one_clip_project());
        assert!(!history.undo());
        assert!(!history.redo());
        assert_eq!(history.project().clips.len(), 1);
    }

    /// A drag gesture: one checkpoint, many uncheckpointed mutations, one
    /// undo to reverse the whole gesture. This is the behavior the
    /// `project_mut_uncheckpointed` doc comment promises.
    #[test]
    fn a_checkpointed_gesture_undoes_as_one_step() {
        let mut history = History::new(one_clip_project());
        let clip_id = history.project().clips[0].id;

        history.checkpoint();
        for out_secs in [19u64, 18, 17, 16, 15] {
            history
                .project_mut_uncheckpointed()
                .trim_clip(clip_id, None, Some(secs(out_secs)))
                .unwrap();
        }
        assert_eq!(history.project().clips[0].out_point, secs(15));

        assert!(history.undo());
        assert_eq!(history.project().clips[0].out_point, secs(20), "one undo reverses the whole drag");
        assert!(!history.can_undo());
    }

    #[test]
    fn the_stack_is_bounded_and_drops_the_oldest_entries() {
        let mut history = History { past: Vec::new(), present: one_clip_project(), future: Vec::new(), limit: 3 };
        for _ in 0..10 {
            history.edit().master_muted = true;
        }
        assert_eq!(history.past.len(), 3, "the stack must not grow without bound");
        // Still fully usable after trimming.
        assert!(history.undo());
    }

    #[test]
    fn reset_replaces_the_project_and_clears_both_stacks() {
        let mut history = History::new(one_clip_project());
        history.edit().split_at_timeline_time(secs(10)).unwrap();
        history.undo();
        assert!(history.can_redo());

        history.reset(one_clip_project());
        assert!(!history.can_undo());
        assert!(!history.can_redo(), "opening a file must not leave a redo into the old project");
        assert_eq!(history.project().clips.len(), 1);
    }
}
