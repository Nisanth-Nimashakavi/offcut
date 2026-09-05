//! offcut-engine: the GStreamer pipeline, clock, seek, and frame delivery
//! layer named in the architecture diagram ("Engine thread —
//! owns the GStreamer pipeline + clock").
//!
//! # What this crate honestly is, right now
//!
//! This crate is the CPU-side half of the original decode spike: building a
//! real GStreamer pipeline, driving it to `Playing`, pulling real decoded
//! frames out through `appsink`, and two-tier seeking. Every test in
//! `pipeline.rs` runs a genuine GStreamer pipeline (`videotestsrc`, which
//! needs no external video file and is exactly the kind of headless-safe
//! source this environment can exercise) and asserts on real pulled frame
//! bytes, not mocked data.
//!
//! **2026-08-28 update:** the GPU half this doc comment previously said
//! "could not be built or verified" (citing this sandbox's missing
//! `/dev/dri` device node) has since been built and verified, in
//! `offcut-render`'s `preview.rs` and `offcut-app`'s `main.rs` — see those
//! crates' doc comments and the crate docs's "The GPU correction" /
//! "Visual proof" sections for the full account. Short version: `wgpu`'s
//! GL backend, via EGL's surfaceless platform, does not need `/dev/dri`,
//! and this exact crate's `Pipeline::test_pattern` now feeds a real,
//! live, on-screen video preview in a real Wayland window on this
//! machine — the "no `/dev/dri`" fact was correct; generalizing it to
//! "no GPU-dependent code can run here" was not.
//!
//! What remains genuinely unverified here (not a repeat of the same
//! mistake — this one is Vulkan-specific, and this sandbox's Vulkan
//! backend really does find zero adapters) is DMABUF zero-copy import,
//! which needs the real target machine's Vulkan ICD.
//!
//! Run `cargo test -p offcut-engine` to see it pull real frames, or
//! `cargo run --bin offcut` (in the `offcut-app` crate) to see those frames
//! reach an actual window.

pub mod audio;
pub mod caps;
pub mod error;
pub mod frame;
pub mod pipeline;
pub mod probe;
pub mod thread;
pub mod thumbs;

pub use audio::{AudioBlock, EXPORT_CHANNELS, EXPORT_SAMPLE_RATE, decode_span, frames_for_span};
pub use caps::Capabilities;
pub use error::EngineError;
pub use frame::{Frame, PixelFormat};
pub use pipeline::Pipeline;
pub use probe::{MediaInfo, probe_file};
pub use thread::{EngineCommand, EngineEvent, EngineHandle};
pub use thumbs::{Thumbnail, thumbnails_for, waveform_peaks, waveform_peaks_streaming, waveform_peaks_within};
