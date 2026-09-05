//! Decoding a clip's audio span into raw PCM, at a **known exact length**.
//!
//! This module exists to serve `offcut-export`'s audio branch, and its
//! whole design follows from one requirement that the video path does not
//! have: **A/V sync must hold by construction, not by hope**.
//!
//! # Why the output length is computed, not observed
//!
//! The video path can afford to decode "until the out-point" and emit
//! whatever frames arrive, because it re-derives each frame's output index
//! from that frame's own timestamp — a dropped or duplicated decode
//! self-corrects on the next frame. Audio has no such anchor: it is a
//! continuous stream, and a segment that comes out 3 ms short does not
//! announce itself. It silently shifts *every subsequent clip* earlier,
//! and those errors accumulate across the timeline until the last clip's
//! audio is visibly ahead of its picture.
//!
//! So this module inverts the usual control flow. The caller states the
//! span and the speed; `decode_span` computes exactly how many sample
//! frames that span must produce:
//!
//! ```text
//!   frames = round(span_seconds / speed * rate)
//! ```
//!
//! and then guarantees that many, padding with silence if the decoder ran
//! dry early and truncating if it overshot. A clip's audio is therefore
//! always exactly as long as the same clip's video, and concatenation
//! cannot drift — the property the design asks for ("audio stays in sync
//! over a 10-min timeline") is enforced per segment rather than measured
//! at the end.
//!
//! # Why `pitch` is in the graph rather than applied afterwards
//!
//! The design rule and §4.3 settle on `soundtouch`'s `pitch tempo=` (this
//! build has no `scaletempo`). It sits *in* the decode graph so the
//! samples this module returns are already time-stretched and
//! pitch-corrected: a 2× clip yields half as many samples, at the original
//! pitch. Doing it after the fact would mean shipping a time-stretch
//! implementation in Rust, which is precisely the wheel `soundtouch`
//! already is.
//!
//! At 4× the question does not arise: `Speed::implies_mute` is true, the
//! caller asks for silence, and no decode happens at all.
//!
//! # Why a fixed format
//!
//! Everything is resampled to F32LE / 48 kHz / stereo interleaved before
//! it leaves here. The encoder wants one format, the muxer wants one
//! stream, and a timeline whose clips come from sources at 44.1 and 48 kHz
//! must not produce a file that changes sample rate halfway through. One
//! conversion, at the edge, in the pipeline where `audioresample` does it
//! properly.

use crate::error::EngineError;
use gstreamer as gst;
use std::path::Path;
use offcut_model::Time;

/// The one interchange format for all exported audio. See the module doc
/// comment: mixed-rate sources must not produce a mixed-rate file.
pub const EXPORT_SAMPLE_RATE: u32 = 48_000;
pub const EXPORT_CHANNELS: u32 = 2;

/// A block of interleaved F32 PCM at [`EXPORT_SAMPLE_RATE`] /
/// [`EXPORT_CHANNELS`].
#[derive(Clone, Debug, PartialEq)]
pub struct AudioBlock {
    /// Interleaved samples: `frames * EXPORT_CHANNELS` values.
    pub samples: Vec<f32>,
}

impl AudioBlock {
    /// `frames` sample-frames of digital silence.
    pub fn silence(frames: usize) -> Self {
        Self { samples: vec![0.0; frames * EXPORT_CHANNELS as usize] }
    }

    /// Sample frames (per channel), not interleaved values.
    pub fn frame_count(&self) -> usize {
        self.samples.len() / EXPORT_CHANNELS as usize
    }

    pub fn duration(&self) -> Time {
        Time::from_nanos(
            (self.frame_count() as u64).saturating_mul(1_000_000_000) / EXPORT_SAMPLE_RATE as u64,
        )
    }

    /// True when every sample is exactly zero. Used by the export tests to
    /// tell "muted, so silence was written" apart from "audio was lost".
    pub fn is_silent(&self) -> bool {
        self.samples.iter().all(|s| *s == 0.0)
    }

    /// Largest absolute sample, i.e. whether there is anything audible
    /// here at all.
    pub fn peak(&self) -> f32 {
        self.samples.iter().fold(0.0f32, |m, s| if s.is_finite() { m.max(s.abs()) } else { m })
    }

    /// Force this block to exactly `frames` sample-frames, padding with
    /// silence or truncating. This is the invariant the module doc comment
    /// describes, factored out so it is testable without a decoder.
    fn conform_to(&mut self, frames: usize) {
        self.samples.resize(frames * EXPORT_CHANNELS as usize, 0.0);
    }
}

/// How many sample frames a clip of `span` played at `speed` must produce.
///
/// Public because `offcut-export` needs the same number to lay out the
/// output timeline, and two independent roundings of the same quantity is
/// exactly how a one-sample-per-clip drift gets introduced.
pub fn frames_for_span(span: Time, speed: f64) -> usize {
    if !(speed.is_finite() && speed > 0.0) {
        return 0;
    }
    (span.as_secs_f64() / speed * EXPORT_SAMPLE_RATE as f64).round().max(0.0) as usize
}

/// Decode `[from, to)` of `path`'s audio, time-stretched by `speed`,
/// returning exactly [`frames_for_span`] sample frames.
///
/// Returns silence of the correct length — not an error — when the file
/// has no audio track or its audio cannot be decoded. A source without
/// sound is an ordinary thing to put on a timeline, and it must not fail
/// an export or, worse, shorten the audio stream and desynchronize
/// everything after it.
pub fn decode_span(path: &Path, from: Time, to: Time, speed: f64) -> Result<AudioBlock, EngineError> {
    let span = Time::from_nanos(to.as_nanos().saturating_sub(from.as_nanos()));
    let wanted = frames_for_span(span, speed);
    if wanted == 0 {
        return Ok(AudioBlock { samples: Vec::new() });
    }

    let mut block = match decode_span_inner(path, from, to, speed, wanted) {
        Ok(b) => b,
        // A missing/undecodable audio track yields silence of the right
        // length rather than propagating: see the doc comment above.
        Err(_) => AudioBlock::silence(wanted),
    };
    block.conform_to(wanted);
    Ok(block)
}

fn decode_span_inner(
    path: &Path,
    from: Time,
    to: Time,
    speed: f64,
    wanted: usize,
) -> Result<AudioBlock, EngineError> {
    crate::pipeline::ensure_gst_init()?;
    let uri = crate::probe::path_to_uri(path)?;

    // `audioconvert ! audioresample ! pitch ! audioconvert ! caps`:
    // the second `audioconvert` is not redundant. `pitch` negotiates its
    // own output format (it works in F32 internally but may hand back
    // whatever the downstream accepted), and pinning the final caps
    // directly onto it makes negotiation fail on some builds. Converting
    // once more after the stretch is cheap and always negotiates.
    let tempo = speed.clamp(0.1, 10.0);
    let description = format!(
        "uridecodebin uri={uri} ! queue ! audioconvert ! audioresample ! \
         pitch tempo={tempo} ! audioconvert ! audioresample ! \
         audio/x-raw,format=F32LE,rate={EXPORT_SAMPLE_RATE},channels={EXPORT_CHANNELS},layout=interleaved ! \
         appsink name=sink sync=false max-buffers=64 drop=false"
    );

    let pipeline = crate::pipeline::Pipeline::from_description(&description)?;
    pipeline.pause()?;
    pipeline.wait_until_ready(gst::ClockTime::from_seconds(15))?;

    // ACCURATE, exactly as the video path does for the same reason: an
    // export must begin at the trimmed in-point, not at the nearest
    // keyframe before it. An audio segment that starts early is not
    // merely wrong content, it is a sync error for the whole clip.
    pipeline.seek_accurate(from)?;
    pipeline.play()?;

    let appsink = pipeline.appsink();
    let mut samples: Vec<f32> = Vec::with_capacity(wanted * EXPORT_CHANNELS as usize);
    let want_values = wanted * EXPORT_CHANNELS as usize;

    // Stop on sample count, not on timestamp. After `pitch` the outgoing
    // timestamps are on the *stretched* timeline, so comparing them
    // against `to` (a source-timeline value) would cut a 2× clip at the
    // wrong place. Counting is speed-independent and is what makes the
    // length exact.
    let _ = to;
    while samples.len() < want_values {
        let Some(sample) = appsink.try_pull_sample(gst::ClockTime::from_seconds(5)) else {
            break; // EOS or stall: the caller pads to length.
        };
        let Some(buffer) = sample.buffer() else { continue };
        let Ok(map) = buffer.map_readable() else { continue };
        for chunk in map.as_slice().chunks_exact(4) {
            let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            samples.push(if v.is_finite() { v } else { 0.0 });
        }
    }

    let _ = pipeline.stop();
    samples.truncate(want_values);
    Ok(AudioBlock { samples })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../media").join(name);
        assert!(p.exists(), "missing fixture {} — run offcut/tools/make-sample.sh", p.display());
        p
    }

    fn secs(s: f64) -> Time {
        Time::from_nanos((s * 1e9) as u64)
    }

    #[test]
    fn frames_for_span_is_rate_times_seconds_at_1x() {
        assert_eq!(frames_for_span(secs(1.0), 1.0), 48_000);
        assert_eq!(frames_for_span(secs(2.0), 1.0), 96_000);
    }

    /// The speed rule, at the level that matters for sync: a 2× clip
    /// occupies half as much of the output timeline, so it must contribute
    /// half as many samples.
    #[test]
    fn frames_for_span_halves_at_2x_and_doubles_at_half_speed() {
        assert_eq!(frames_for_span(secs(2.0), 2.0), 48_000);
        assert_eq!(frames_for_span(secs(2.0), 0.5), 192_000);
    }

    #[test]
    fn frames_for_span_refuses_nonsense_speeds_instead_of_panicking() {
        assert_eq!(frames_for_span(secs(1.0), 0.0), 0);
        assert_eq!(frames_for_span(secs(1.0), -1.0), 0);
        assert_eq!(frames_for_span(secs(1.0), f64::NAN), 0);
    }

    #[test]
    fn silence_has_the_requested_length_and_is_actually_silent() {
        let block = AudioBlock::silence(1000);
        assert_eq!(block.frame_count(), 1000);
        assert_eq!(block.samples.len(), 2000, "stereo interleaved");
        assert!(block.is_silent());
        assert_eq!(block.peak(), 0.0);
    }

    #[test]
    fn conform_to_pads_a_short_block_and_truncates_a_long_one() {
        let mut short = AudioBlock { samples: vec![0.5; 100] };
        short.conform_to(100);
        assert_eq!(short.frame_count(), 100);
        assert!(!short.is_silent(), "padding must not erase the real samples");

        let mut long = AudioBlock { samples: vec![0.5; 1000] };
        long.conform_to(10);
        assert_eq!(long.frame_count(), 10);
    }

    #[test]
    fn a_blocks_duration_matches_its_frame_count() {
        let block = AudioBlock::silence(EXPORT_SAMPLE_RATE as usize);
        assert!((block.duration().as_secs_f64() - 1.0).abs() < 1e-9);
    }

    /// The headline property, against a real file with a real AAC track:
    /// the returned block is exactly the requested length and contains
    /// actual audio, not silence.
    #[test]
    fn decodes_real_audio_from_a_real_file_at_exactly_the_requested_length() {
        let path = fixture("sample.mp4");
        let block = decode_span(&path, secs(0.5), secs(2.5), 1.0).expect("decode failed");

        assert_eq!(
            block.frame_count(),
            frames_for_span(secs(2.0), 1.0),
            "the block must be exactly as long as the span it was asked for"
        );
        assert!(
            block.peak() > 0.001,
            "sample.mp4 has an audible tone; got a peak of {} — audio was lost, not decoded",
            block.peak()
        );
    }

    /// The sync property stated as a test: at 2×, the same source span
    /// must yield half the samples — which is what makes a sped-up clip's
    /// audio end when its picture does.
    #[test]
    fn a_2x_span_yields_exactly_half_the_samples_of_the_same_span_at_1x() {
        let path = fixture("sample.mp4");
        let one_x = decode_span(&path, secs(0.0), secs(2.0), 1.0).expect("1x decode");
        let two_x = decode_span(&path, secs(0.0), secs(2.0), 2.0).expect("2x decode");

        assert_eq!(two_x.frame_count() * 2, one_x.frame_count());
        assert!(two_x.peak() > 0.001, "a 2x clip should still be audible, not silent");
    }

    /// A slowed clip must *gain* samples, and still be audible — the
    /// `pitch` element is doing a real time-stretch here, not a resample.
    #[test]
    fn a_half_speed_span_yields_twice_the_samples_and_stays_audible() {
        let path = fixture("sample.mp4");
        let half = decode_span(&path, secs(0.0), secs(1.0), 0.5).expect("0.5x decode");
        assert_eq!(half.frame_count(), frames_for_span(secs(1.0), 0.5));
        assert!(half.peak() > 0.001, "a slowed clip should still be audible");
    }

    /// A file that does not exist must not fail the export: it yields
    /// silence of the correct length, keeping the output stream
    /// continuous. This is the no-audio-track case in its most extreme
    /// form, and it is the one that would otherwise desynchronize every
    /// clip after it.
    #[test]
    fn a_missing_file_yields_silence_of_the_right_length_rather_than_an_error() {
        let block = decode_span(Path::new("/nonexistent/file.mp4"), secs(0.0), secs(1.0), 1.0)
            .expect("must not error");
        assert_eq!(block.frame_count(), frames_for_span(secs(1.0), 1.0));
        assert!(block.is_silent());
    }

    #[test]
    fn a_zero_length_span_yields_an_empty_block_rather_than_hanging() {
        let path = fixture("sample.mp4");
        let block = decode_span(&path, secs(1.0), secs(1.0), 1.0).expect("decode");
        assert_eq!(block.frame_count(), 0);
    }
}
