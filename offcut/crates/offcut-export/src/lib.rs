//! offcut-export: the edited timeline out to a real MP4.
//!
//! the crate layout listed this crate from the start and
//! §6 Phase 6 left it unstarted; this is that phase, built.
//!
//! What it does, honestly:
//!
//! - Walks the timeline clip by clip, decoding each clip's trimmed source
//!   span with `offcut-engine`.
//! - Bakes that clip's crop, straighten, and five adjust values into each
//!   frame with `offcut-render`'s `EffectsRenderer` — **the same WGSL
//!   shader the on-screen preview uses**, which is what makes the export
//!   match what was previewed rather than approximately match it.
//! - Pushes the baked frames into one `appsrc`-fed encode graph with
//!   output-timeline timestamps, so speed changes apply and clips
//!   concatenate without the encoder ever seeing a seam.
//! - Writes to a temp file beside the target and renames on success, so a
//!   cancelled or failed export never leaves a truncated file where the
//!   user asked for a finished one.
//!
//! What it does not do yet, stated rather than implied: **audio is not
//! muxed**. See `encode.rs`'s module doc comment for why that ordering
//! was chosen and what it takes to close.

pub mod encode;
pub mod error;
pub mod settings;

pub use encode::{CancelFlag, export, output_resolution};
pub use error::ExportError;
pub use settings::{
    Container, ExportProgress, ExportSettings, ResolutionPreset, VideoCodec, output_framerate,
};
