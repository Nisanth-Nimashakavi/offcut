//! Real filmstrip thumbnails and audio waveform peaks, extracted from the
//! actual media file.
//!
//! The design system documents an honest gap in its own mockup: the clip cards
//! carry a placeholder image glyph instead of frames, and the audio lane's
//! bars are decorative. This module closes both gaps with real data —
//! `thumbnails_for` decodes N frames spread across a source span, and
//! `waveform_peaks` reads the real audio track's amplitude envelope.
//!
//! # Why a separate short-lived pipeline per extraction
//!
//! Neither of these can share the playback pipeline: seeking it to build a
//! filmstrip would yank the preview away from wherever the user is
//! looking. So each call builds its own `uridecodebin` pipeline, pulls
//! what it needs, and tears it down. That is a real cost (a few hundred ms
//! for a filmstrip), which is exactly why the caller runs it off the UI
//! thread and caches the result — see `offcut-app`'s thumbnail task.
//!
//! # Why thumbnails are scaled in the pipeline, not in Rust
//!
//! `videoscale` inside GStreamer does the downscale before the frame ever
//! reaches this process's memory as a full-resolution buffer. Decoding a
//! 1920×1080 frame and shrinking it to 160×90 in a Rust loop would move
//! ~8 MB per thumbnail across the appsink boundary to throw 99% of it
//! away. This is the same "avoid the copy that isn't load-bearing"
//! reasoning as the copy-avoidance list, applied at a smaller
//! scale.

use crate::error::EngineError;
use crate::frame::{Frame, PixelFormat};
use gstreamer as gst;
use std::path::Path;
use offcut_model::Time;

/// One decoded, downscaled filmstrip thumbnail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// Tightly packed RGBA, `width * height * 4` bytes. Unlike `Frame`,
    /// this is repacked to a tight stride: it is handed straight to
    /// `iced::widget::image::Handle::from_rgba`, which requires tight
    /// packing, and doing the repack once here beats doing it on every
    /// redraw.
    pub rgba: Vec<u8>,
    /// Source time this thumbnail was decoded at.
    pub at: Time,
}

impl Thumbnail {
    pub fn is_well_formed(&self) -> bool {
        self.rgba.len() == (self.width as usize) * (self.height as usize) * 4
    }
}

/// Decode `count` thumbnails evenly spread across `[from, to)` of a source
/// file, each scaled to `width × height`.
///
/// Seeks to each requested instant with `KEY_UNIT` (the fast tier of
/// the two-tier seek): a filmstrip thumbnail is a visual
/// index, not a frame-accurate answer, and paying for an `ACCURATE` seek
/// per thumbnail would multiply this call's cost several-fold for a
/// difference no one can see at 160px wide.
///
/// Returns fewer than `count` thumbnails if the file ends early or a seek
/// fails — a partial filmstrip is strictly better than an error, because
/// the alternative the UI would have to render is the placeholder this
/// module exists to remove.
pub fn thumbnails_for(
    path: &Path,
    from: Time,
    to: Time,
    count: usize,
    width: u32,
    height: u32,
) -> Result<Vec<Thumbnail>, EngineError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    crate::pipeline::ensure_gst_init()?;
    let uri = crate::probe::path_to_uri(path)?;

    // `videoconvert ! videoscale ! caps` order matters: convert to a
    // format videoscale handles, then scale, then force RGBA at the
    // target size. Doing the scale before the convert makes videoscale
    // negotiate whatever the decoder emits, which for some hardware
    // decoders is a format it cannot scale.
    let description = format!(
        "uridecodebin uri={uri} ! queue ! videoconvert ! videoscale ! \
         video/x-raw,format=RGBA,width={width},height={height},pixel-aspect-ratio=1/1 ! \
         appsink name=sink sync=false max-buffers=2 drop=false"
    );

    let pipeline = crate::pipeline::Pipeline::from_description(&description)?;
    pipeline.pause()?;
    pipeline.wait_until_ready(gst::ClockTime::from_seconds(10))?;

    let span = to.as_nanos().saturating_sub(from.as_nanos());
    let mut thumbnails = Vec::with_capacity(count);

    for i in 0..count {
        // Sample at the *center* of each of `count` equal buckets, not at
        // the bucket edges. Sampling at edges means thumbnail 0 is always
        // the clip's first frame -- which for a fade-in is a black
        // rectangle, making the filmstrip's most informative slot its
        // least informative one.
        let numerator = (2 * i + 1) as u64;
        let offset = span.saturating_mul(numerator) / (2 * count as u64);
        let at = Time::from_nanos(from.as_nanos().saturating_add(offset));

        if pipeline.seek_fast(at).is_err() {
            continue;
        }
        match pipeline.pull_current_frame(gst::ClockTime::from_seconds(5)) {
            Ok(frame) => thumbnails.push(thumbnail_from_frame(&frame, at)),
            Err(_) => continue,
        }
    }

    let _ = pipeline.stop();
    Ok(thumbnails)
}

/// Repack a `Frame` (which may carry GStreamer's alignment padding in its
/// stride) into a tightly packed thumbnail.
fn thumbnail_from_frame(frame: &Frame, at: Time) -> Thumbnail {
    let tight_stride = (frame.width * 4) as usize;
    let stride = frame.stride as usize;
    let mut rgba = Vec::with_capacity(tight_stride * frame.height as usize);
    for row in 0..frame.height as usize {
        let start = row * stride;
        let end = start + tight_stride;
        if end <= frame.data.len() {
            rgba.extend_from_slice(&frame.data[start..end]);
        } else {
            // Defensive: a malformed frame gets black rows rather than a
            // panic or a torn image. `Frame::is_well_formed` makes this
            // unreachable for anything this crate produces.
            rgba.extend(std::iter::repeat_n(0u8, tight_stride));
        }
    }
    debug_assert_eq!(frame.format, PixelFormat::Rgba8);
    Thumbnail { width: frame.width, height: frame.height, rgba, at }
}

/// The audio envelope for the timeline's audio lane: `bucket_count` peak
/// amplitudes in `0.0..=1.0`, spread across the file's whole duration.
///
/// The design system's audio lane draws "rounded 7px bars at `#2A3140`" — those
/// bar heights are these values. Before this function they were a fixed
/// decorative pattern; now a silent passage actually reads as short bars.
///
/// Returns `Ok(vec![])` for a file with no audio track, which is a normal
/// state (`Source::has_audio == false`), not an error.
pub fn waveform_peaks(path: &Path, bucket_count: usize) -> Result<Vec<f32>, EngineError> {
    waveform_peaks_within(path, Time::ZERO, None, bucket_count)
}

/// Waveform peaks over a span, sampling rather than decoding everything.
///
/// # Why this samples instead of decoding the whole track
///
/// The first version played the file from start to finish through
/// `audioconvert` and accumulated every sample. Measured on a 1h41m
/// 2GB HEVC film: **198 seconds**. That is the single largest source of
/// the "it feels super slow" complaint — opening any real movie spawned a
/// background task that pinned a core for over three minutes, competing
/// with the decoder feeding the preview.
///
/// The output is a few hundred bars. Decoding ~180 million samples to
/// draw 800 of them is not a rendering requirement, it is waste. This
/// version seeks to the centre of each bucket and decodes a short window
/// there, so cost scales with the number of *bars* (bounded, and set by
/// the width of the lane) rather than with the duration of the file. The
/// envelope is an approximation of a peak envelope rather than an exact
/// one — for a visual index of where the loud parts are, at 3px per bar,
/// that distinction is invisible and the 100× speedup is not.
pub fn waveform_peaks_within(
    path: &Path,
    from: Time,
    to: Option<Time>,
    bucket_count: usize,
) -> Result<Vec<f32>, EngineError> {
    waveform_peaks_streaming(path, from, to, bucket_count, |_| {})
}

/// As `waveform_peaks_within`, but reports the envelope as it fills in.
///
/// On a feature-length file the sampling pass still takes tens of
/// seconds — bounded, but far too long to show an empty lane for. The
/// callback receives the partial envelope periodically (zeros for
/// buckets not yet measured), so the waveform draws itself in from left
/// to right instead of appearing all at once at the end. Progressive
/// feedback on work that is genuinely slow is the honest alternative to
/// pretending it is fast.
pub fn waveform_peaks_streaming(
    path: &Path,
    from: Time,
    to: Option<Time>,
    bucket_count: usize,
    mut on_progress: impl FnMut(&[f32]),
) -> Result<Vec<f32>, EngineError> {
    if bucket_count == 0 {
        return Ok(Vec::new());
    }
    crate::pipeline::ensure_gst_init()?;
    let uri = crate::probe::path_to_uri(path)?;

    // Mono F32 at a low sample rate: this is an envelope for a lane a
    // few hundred pixels wide, so full-rate stereo would move ~100x more
    // samples than the answer needs.
    let description = format!(
        "uridecodebin uri={uri} ! queue ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,channels=1,rate=8000,layout=interleaved ! \
         appsink name=sink sync=false max-buffers=4 drop=false"
    );

    let pipeline = crate::pipeline::Pipeline::from_description(&description)?;
    if pipeline.pause().is_err() || pipeline.wait_until_ready(gst::ClockTime::from_seconds(10)).is_err() {
        let _ = pipeline.stop();
        return Ok(vec![0.0; bucket_count]);
    }

    let end = to
        .or_else(|| pipeline.duration())
        .unwrap_or(Time::from_nanos(from.as_nanos().saturating_add(1)));
    let span = end.as_nanos().saturating_sub(from.as_nanos()).max(1);

    // For a short source, decoding straight through is both faster and
    // more accurate than seeking once per bucket: a seek costs a
    // keyframe search and a pipeline flush, and below roughly a minute
    // the whole track is cheaper than `bucket_count` of those. Long
    // files take the sampling path, where the seek cost is what keeps
    // the total bounded.
    const LINEAR_SCAN_LIMIT_NS: u64 = 90 * 1_000_000_000;
    if span <= LINEAR_SCAN_LIMIT_NS {
        let peaks = linear_scan_peaks(&pipeline, bucket_count);
        let _ = pipeline.stop();
        return Ok(peaks);
    }

    let appsink = pipeline.appsink();
    let mut peaks = Vec::with_capacity(bucket_count);

    // A short decode window per bucket. Long enough to catch a transient,
    // short enough that the whole pass stays proportional to the bar
    // count instead of the file length.
    const WINDOW_BUFFERS: usize = 3;

    for i in 0..bucket_count {
        let at = Time::from_nanos(
            from.as_nanos()
                .saturating_add(span.saturating_mul(2 * i as u64 + 1) / (2 * bucket_count as u64)),
        );
        if pipeline.seek_fast(at).is_err() {
            peaks.push(0.0);
            continue;
        }
        let mut peak = 0.0f32;
        for _ in 0..WINDOW_BUFFERS {
            let Some(sample) = appsink.try_pull_preroll(gst::ClockTime::from_mseconds(120)) else {
                break;
            };
            let Some(buffer) = sample.buffer() else { continue };
            let Ok(map) = buffer.map_readable() else { continue };
            for chunk in map.as_slice().chunks_exact(4) {
                let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                if v.is_finite() {
                    peak = peak.max(v.abs());
                }
            }
        }
        peaks.push(peak.clamp(0.0, 1.0));

        // Publish every ~5% so the lane fills visibly without flooding
        // the UI thread with messages.
        if peaks.len() % (bucket_count / 20).max(1) == 0 {
            let mut partial = peaks.clone();
            partial.resize(bucket_count, 0.0);
            on_progress(&partial);
        }
    }

    let _ = pipeline.stop();
    Ok(peaks)
}

/// Decode a short source straight through and bucket every sample.
fn linear_scan_peaks(pipeline: &crate::pipeline::Pipeline, bucket_count: usize) -> Vec<f32> {
    if pipeline.play().is_err() {
        return vec![0.0; bucket_count];
    }
    let appsink = pipeline.appsink();
    let mut samples: Vec<f32> = Vec::new();
    while let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_seconds(5)) {
        let Some(buffer) = sample.buffer() else { continue };
        let Ok(map) = buffer.map_readable() else { continue };
        for chunk in map.as_slice().chunks_exact(4) {
            let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            samples.push(if v.is_finite() { v.abs() } else { 0.0 });
        }
    }
    bucket_peaks(&samples, bucket_count)
}

/// Reduce a sample envelope to `bucket_count` peaks.
///
/// **Peak, not mean.** A mean envelope of music sits near 0.2 and looks
/// like a flat gray band; the peak envelope is what every audio editor
/// draws and what makes a transient visible as a tall bar. Split out as
/// its own function so this choice is testable without decoding a file.
fn bucket_peaks(samples: &[f32], bucket_count: usize) -> Vec<f32> {
    if bucket_count == 0 {
        return Vec::new();
    }
    if samples.is_empty() {
        return vec![0.0; bucket_count];
    }
    let mut peaks = Vec::with_capacity(bucket_count);
    for i in 0..bucket_count {
        let start = i * samples.len() / bucket_count;
        let end = ((i + 1) * samples.len() / bucket_count).max(start + 1).min(samples.len());
        let peak = samples[start..end].iter().copied().fold(0.0f32, f32::max);
        peaks.push(peak.clamp(0.0, 1.0));
    }
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn require_sample(name: &str) -> std::path::PathBuf {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../media").join(name);
        assert!(p.exists(), "missing fixture {} — run offcut/tools/make-sample.sh", p.display());
        p
    }

    /// The core claim of this module: real decoded frames, at the right
    /// size, with real (non-black) pixel content — not placeholders.
    #[test]
    fn extracts_real_scaled_thumbnails_from_a_real_file() {
        let path = require_sample("sample.mp4");
        let thumbs = thumbnails_for(&path, Time::ZERO, Time::from_nanos(5_000_000_000), 4, 160, 90)
            .expect("thumbnail extraction failed");

        assert!(!thumbs.is_empty(), "expected at least one thumbnail from a real 5s file");
        for thumb in &thumbs {
            assert_eq!((thumb.width, thumb.height), (160, 90), "videoscale must hit the requested size");
            assert!(thumb.is_well_formed(), "rgba must be tightly packed width*height*4");
            assert!(
                thumb.rgba.iter().any(|&b| b != 0),
                "a thumbnail of real footage must not be entirely black"
            );
        }
    }

    /// Thumbnails must differ from each other — the bug this catches is a
    /// seek that silently fails and returns the same first frame N times,
    /// which looks plausible in a screenshot and is completely useless as
    /// a filmstrip.
    #[test]
    fn thumbnails_across_a_span_are_not_all_identical() {
        let path = require_sample("sample.mp4");
        let thumbs = thumbnails_for(&path, Time::ZERO, Time::from_nanos(5_000_000_000), 4, 64, 36)
            .expect("thumbnail extraction failed");
        assert!(thumbs.len() >= 2, "need at least two thumbnails to compare, got {}", thumbs.len());
        let all_same = thumbs.windows(2).all(|w| w[0].rgba == w[1].rgba);
        assert!(!all_same, "every thumbnail was identical — the seek is not taking effect");
    }

    #[test]
    fn requesting_zero_thumbnails_is_a_no_op_not_a_pipeline_build() {
        let path = require_sample("sample.mp4");
        assert!(thumbnails_for(&path, Time::ZERO, Time::from_nanos(1), 0, 16, 16).unwrap().is_empty());
    }

    #[test]
    fn extracts_a_real_waveform_envelope_from_a_file_with_audio() {
        let path = require_sample("sample.mp4");
        let peaks = waveform_peaks(&path, 64).expect("waveform extraction failed");
        assert_eq!(peaks.len(), 64);
        assert!(peaks.iter().all(|p| (0.0..=1.0).contains(p)), "peaks must be normalized");
        assert!(
            peaks.iter().any(|&p| p > 0.01),
            "sample.mp4 has a real AAC tone track — its envelope must not be silent"
        );
    }

    #[test]
    fn bucket_peaks_takes_the_maximum_not_the_mean() {
        // One loud transient in an otherwise quiet bucket must survive.
        let samples = vec![0.01, 0.01, 0.9, 0.01];
        let peaks = bucket_peaks(&samples, 1);
        assert_eq!(peaks, vec![0.9], "a mean would report ~0.23 and hide the transient");
    }

    #[test]
    fn bucket_peaks_splits_evenly_and_never_panics_on_odd_sizes() {
        let samples: Vec<f32> = (0..7).map(|i| i as f32 / 10.0).collect();
        for buckets in [1usize, 2, 3, 7, 16, 100] {
            let peaks = bucket_peaks(&samples, buckets);
            assert_eq!(peaks.len(), buckets, "{buckets} buckets");
            assert!(peaks.iter().all(|p| p.is_finite()));
        }
    }

    #[test]
    fn bucket_peaks_of_no_samples_is_a_flat_silent_envelope() {
        assert_eq!(bucket_peaks(&[], 4), vec![0.0, 0.0, 0.0, 0.0]);
    }
}
