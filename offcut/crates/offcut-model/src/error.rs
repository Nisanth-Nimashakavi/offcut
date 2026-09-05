use crate::ids::{ClipId, SourceId};
use thiserror::Error;

/// Every way an edit operation can fail. the property-test
/// requirement — "random sequences of split/trim/delete/speed must never
/// produce a clip with `out <= in`" — is enforced by returning `Err` here
/// instead of silently clamping into an invalid state; a caller (the UI
/// layer) decides how to surface each variant, but the model itself never
/// produces a broken `Clip`.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum EditError {
    #[error("clip {0:?} not found in project")]
    ClipNotFound(ClipId),

    #[error("source {0:?} not found in project")]
    SourceNotFound(SourceId),

    #[error("split point is not strictly inside clip {0:?}'s [in, out) span")]
    SplitPointOutsideClip(ClipId),

    #[error("trim would produce out <= in for clip {0:?}")]
    TrimWouldInvertClip(ClipId),

    #[error("trim point is outside source {0:?}'s duration")]
    TrimOutsideSourceDuration(SourceId),

    /// A requested range had `out <= in`. Distinct from
    /// `TrimWouldInvertClip`, which names an *existing* clip: this one is
    /// raised by `set_range` before any clip exists, so there is no id to
    /// report and inventing one would be a lie.
    #[error("requested range is empty or inverted: out ({out_nanos} ns) <= in ({in_nanos} ns)")]
    EmptyRange { in_nanos: u64, out_nanos: u64 },
}
