//! Reading a real media file's actual properties — duration, exact frame
//! rate, resolution, audio presence — so `offcut_model::Source` is built
//! from what the file *is*, not from hardcoded demo constants.
//!
//! This is the import path's first step (the `Source` struct
//! and §2's note that "offcut-engine is responsible for actually probing a
//! file's duration/fps/etc. before calling `add_clip_for_source`").
//!
//! Frame rate comes back as an exact `offcut_model::Rational`, never an
//! `f64`: GStreamer stores it as a fraction (30000/1001 for 29.97) and
//! The design is emphatic that keeping it a fraction end-to-end is what
//! prevents off-by-one-frame drift. Converting to float here and back
//! later would silently reintroduce exactly the bug the model layer's
//! rational time math was built to avoid.

use crate::error::EngineError;
use gstreamer as gst;
use gstreamer_pbutils as gst_pbutils;
use gstreamer_pbutils::prelude::*;
use std::path::Path;
use offcut_model::{Rational, Time};

/// What a media file actually contains. Maps 1:1 onto the fields
/// `offcut_model::Source` needs, minus the `SourceId`/path the caller owns.
#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub duration: Time,
    pub fps: Rational,
    pub resolution: (u32, u32),
    pub has_audio: bool,
    /// Human-readable video codec ("H.264"), for the titlebar/Source
    /// block the design system specifies. `None` if it couldn't be named.
    pub video_codec: Option<String>,
}

/// Probe a file with `gst-discoverer`, the same machinery
/// `gst-discoverer-1.0(1)` uses. Blocking; the caller runs it off the UI
/// thread (in this app, from the engine thread or a `Task`).
///
/// `timeout` guards against a pathological/corrupt file hanging the
/// import — a real failure mode for user-supplied media, and one that
/// would otherwise present as the app freezing with no explanation.
pub fn probe_file(path: &Path, timeout: gst::ClockTime) -> Result<MediaInfo, EngineError> {
    crate::pipeline::ensure_gst_init()?;

    let uri = path_to_uri(path)?;

    let discoverer = gst_pbutils::Discoverer::new(timeout)
        .map_err(|e| EngineError::ProbeFailed(format!("could not create discoverer: {e}")))?;
    let info = discoverer
        .discover_uri(&uri)
        .map_err(|e| EngineError::ProbeFailed(format!("could not discover {uri}: {e}")))?;

    let duration = info
        .duration()
        .map(|d| Time::from_nanos(d.nseconds()))
        .ok_or_else(|| EngineError::ProbeFailed(format!("{uri} reports no duration")))?;

    let video_streams = info.video_streams();
    let video = video_streams
        .first()
        .ok_or_else(|| EngineError::ProbeFailed(format!("{uri} contains no video stream")))?;

    let width = video.width();
    let height = video.height();

    // Keep the frame rate as the exact fraction GStreamer reports.
    let fps_frac = video.framerate();
    let (num, den) = (fps_frac.numer(), fps_frac.denom());
    let fps = if num > 0 && den > 0 {
        Rational::new(num as u32, den as u32)
    } else {
        // Some containers genuinely omit a frame rate (or report 0/1 for
        // variable-frame-rate content -- the design rule flags VFR as the #1
        // source of A/V drift). Falling back to 30/1 keeps the import
        // usable rather than failing outright, and the caller can warn.
        Rational::WEB_30
    };

    let video_codec = video
        .caps()
        .and_then(|caps| caps.structure(0).map(|s| s.name().to_string()));

    Ok(MediaInfo {
        duration,
        fps,
        resolution: (width, height),
        has_audio: !info.audio_streams().is_empty(),
        video_codec: video_codec.map(friendly_codec_name),
    })
}

/// GStreamer caps names (`video/x-h264`) are not what a user should read
/// in a titlebar; the design system's reference render shows "H.264".
fn friendly_codec_name(caps_name: String) -> String {
    match caps_name.as_str() {
        "video/x-h264" => "H.264".to_string(),
        "video/x-h265" => "HEVC".to_string(),
        "video/x-vp8" => "VP8".to_string(),
        "video/x-vp9" => "VP9".to_string(),
        "video/x-av1" => "AV1".to_string(),
        other => other.trim_start_matches("video/x-").to_uppercase(),
    }
}

/// Convert a filesystem path to a `file://` URI. Uses GStreamer's own
/// converter rather than string concatenation so paths with spaces,
/// non-UTF-8 bytes, or `#`/`?` characters are escaped correctly — a
/// classic source of "works on my machine, breaks on the user's Downloads
/// folder" bugs.
pub fn path_to_uri(path: &Path) -> Result<String, EngineError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| EngineError::ProbeFailed(format!("cannot resolve relative path: {e}")))?
            .join(path)
    };
    gst::glib::filename_to_uri(&absolute, None)
        .map(|g| g.to_string())
        .map_err(|e| EngineError::ProbeFailed(format!("cannot convert path to URI: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_path(name: &str) -> std::path::PathBuf {
        // tools/make-sample.sh writes these; CARGO_MANIFEST_DIR is
        // crates/offcut-engine, so the repo's media/ dir is three up.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../media")
            .join(name)
    }

    fn require_sample(name: &str) -> std::path::PathBuf {
        let p = sample_path(name);
        assert!(
            p.exists(),
            "missing test fixture {}\nRun `offcut/tools/make-sample.sh` to generate it.",
            p.display()
        );
        p
    }

    #[test]
    fn probes_a_real_mp4_for_its_true_properties() {
        let path = require_sample("sample.mp4");
        let info = probe_file(&path, gst::ClockTime::from_seconds(15)).expect("probe failed");

        // These are asserted against what tools/make-sample.sh actually
        // encodes -- real values read out of a real container, not
        // constants the code also made up.
        assert_eq!(info.resolution, (640, 360));
        assert_eq!(info.fps, Rational::new(30, 1));
        assert!(info.has_audio, "sample.mp4 is muxed with an AAC track");
        assert_eq!(info.video_codec.as_deref(), Some("H.264"));

        // ~5s, allowing for the encoder's final-GOP rounding.
        let secs = info.duration.as_secs_f64();
        assert!((4.8..5.3).contains(&secs), "expected ~5s, got {secs}s");
    }

    #[test]
    fn probes_the_second_fixture_with_its_own_distinct_duration() {
        let path = require_sample("sample-bars.mp4");
        let info = probe_file(&path, gst::ClockTime::from_seconds(15)).expect("probe failed");
        assert_eq!(info.resolution, (640, 360));
        let secs = info.duration.as_secs_f64();
        assert!((2.8..3.3).contains(&secs), "expected ~3s, got {secs}s");
    }

    #[test]
    fn missing_file_errors_cleanly_rather_than_panicking() {
        let result = probe_file(
            std::path::Path::new("/nonexistent/definitely-not-here.mp4"),
            gst::ClockTime::from_seconds(5),
        );
        assert!(matches!(result, Err(EngineError::ProbeFailed(_))));
    }

    #[test]
    fn non_media_file_errors_cleanly() {
        // This source file is definitely not a video.
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/probe.rs");
        let result = probe_file(&path, gst::ClockTime::from_seconds(10));
        assert!(result.is_err(), "a .rs file should not probe as media");
    }

    #[test]
    fn path_to_uri_escapes_spaces() {
        let uri = path_to_uri(std::path::Path::new("/tmp/a file with spaces.mp4")).expect("uri");
        assert!(uri.starts_with("file:///"));
        assert!(!uri.contains(' '), "spaces must be percent-encoded, got {uri}");
        assert!(uri.contains("%20"));
    }

    #[test]
    fn friendly_codec_names_match_design_md_wording() {
        assert_eq!(friendly_codec_name("video/x-h264".into()), "H.264");
        assert_eq!(friendly_codec_name("video/x-h265".into()), "HEVC");
    }
}
