//! offcut-model: the pure edit model.
//!
//! The design rule: "`offcut-model` being pure (no GStreamer, no wgpu, no iced)
//! is deliberate: every edit operation is then a unit-testable function,
//! and undo/redo is a plain snapshot or command stack over a `Project`
//! value." This crate has no I/O and no GPU dependency — enforced by its
//! `Cargo.toml` dependency list (serde + thiserror only), not just by
//! convention.

pub mod adjust;
pub mod crop;
pub mod error;
pub mod history;
pub mod ids;
pub mod project;
pub mod speed;
pub mod time;
pub mod timeline;

pub use adjust::{AdjustSettings, AdjustValue};
pub use crop::{AspectPreset, CropGrid, CropHandle, CropTransform, NormalizedRect};
pub use error::EditError;
pub use history::History;
pub use ids::{ClipId, SourceId};
pub use project::{Clip, Project, Source};
pub use speed::Speed;
pub use time::{Rational, Time};
pub use timeline::TimelinePosition;
