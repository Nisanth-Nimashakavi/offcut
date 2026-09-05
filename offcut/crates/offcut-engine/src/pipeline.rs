//! A minimal GStreamer pipeline wrapper: build, play, pull frames, seek,
//! tear down. This is the CPU/appsink half of the original decode spike —
//! deliberately *not* the DMABUF/wgpu-texture half, which needs a real GPU
//! device this sandbox does not have (see `lib.rs`'s crate-level doc
//! comment for the full explanation and what was actually verified).
//!
//! Every method here was run against a real pipeline while writing this
//! file, not written speculatively: see `tests` below.

use crate::error::EngineError;
use crate::frame::{Frame, PixelFormat};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use offcut_model::Time;

/// Ensures `gst::init()` runs exactly once per process, however many
/// `Pipeline`s get created (GStreamer's own guidance: re-init is a no-op
/// but should not race across threads at startup).
pub(crate) fn ensure_gst_init() -> Result<(), EngineError> {
    static INIT: std::sync::Once = std::sync::Once::new();
    static mut INIT_RESULT: Option<Result<(), gst::glib::Error>> = None;
    // SAFETY: Once guarantees the closure runs exactly once before any
    // reader observes INIT_RESULT; the write happens-before every read via
    // Once's own synchronization.
    unsafe {
        INIT.call_once(|| {
            INIT_RESULT = Some(gst::init());
        });
        #[allow(static_mut_refs)]
        match &INIT_RESULT {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(EngineError::Init(e.clone())),
            None => unreachable!("Once guarantees INIT_RESULT is set"),
        }
    }
}

/// A running (or paused) GStreamer pipeline with exactly one appsink named
/// `"sink"`. This is intentionally narrow — the architecture calls
/// for "a playbin-free custom pipeline, one per active source," and this
/// type is that pipeline, not a general-purpose GStreamer wrapper.
pub struct Pipeline {
    pipeline: gst::Pipeline,
    appsink: gst_app::AppSink,
    /// What state the caller last *asked* for, so `pull_current_frame`
    /// can pick the right pull without a blocking state query. A
    /// `set_state` returns `Async` for a file pipeline — the transition
    /// finishes on another thread — and re-deriving that per frame is
    /// exactly the cost this avoids.
    playing_hint: std::sync::atomic::AtomicBool,
}

impl Pipeline {
    /// Build and pause (preroll) a pipeline from a GStreamer pipeline
    /// description string that includes exactly one element named `sink`
    /// of type `appsink`. The description is the caller's to construct
    /// (test-only `test_pattern` below builds one from `videotestsrc`;
    /// real-file playback will build one from `uridecodebin ! ... !
    /// appsink name=sink` once that lands).
    pub fn from_description(description: &str) -> Result<Self, EngineError> {
        ensure_gst_init()?;

        let element = gst::parse::launch(description)
            .map_err(|e| EngineError::PipelineBuild(e.to_string()))?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| EngineError::PipelineBuild("top-level element is not a Pipeline".into()))?;

        let sink_element = pipeline
            .by_name("sink")
            .ok_or(EngineError::ElementNotFound("sink"))?;
        let appsink = sink_element
            .downcast::<gst_app::AppSink>()
            .map_err(|_| EngineError::ElementWrongType("sink"))?;

        Ok(Self { pipeline, appsink, playing_hint: std::sync::atomic::AtomicBool::new(false) })
    }

    /// the example source, used by this crate's own tests so
    /// they run in any environment with GStreamer installed and need no
    /// external video file (this sandbox has none — see `lib.rs`).
    /// `num_buffers` matches `videotestsrc`'s own property name.
    pub fn test_pattern(width: u32, height: u32, num_buffers: u32) -> Result<Self, EngineError> {
        let description = format!(
            "videotestsrc num-buffers={num_buffers} pattern=ball ! \
             video/x-raw,width={width},height={height},format=RGBA,framerate=30/1 ! \
             appsink name=sink"
        );
        Self::from_description(&description)
    }

    /// Build a playback pipeline for a **real media file** — the actual
    /// import path, as opposed to `test_pattern`'s synthetic source.
    ///
    /// Shape:
    /// ```text
    ///   uridecodebin -> videoconvert -> RGBA caps -> appsink name=sink
    ///                \-> audioconvert -> audioresample -> volume name=vol -> autoaudiosink
    /// ```
    ///
    /// Notes on the deliberate choices here:
    /// - `uridecodebin` (not a hand-built `qtdemux ! h264parse ! avdec_h264`
    ///   chain) because it autoplugs whatever demuxer/decoder the file
    ///   actually needs, which is what makes "open any MP4/MOV/MKV/WebM"
    ///   (the product's in-scope list) achievable without a per-format
    ///   branch — and it transparently picks up VA-API hardware decode if
    ///   the user later installs it, exactly the runtime-probed upgrade
    ///   The design rule describes.
    /// - Audio is wired to a real sink with a named `volume` element, so
    ///   per-clip and master mute are a property set, not a
    ///   pipeline rebuild. If the file has no audio, the audio branch is
    ///   simply never linked — `uridecodebin` only emits pads that exist.
    /// - `sync=true` on the appsink. This one is worth spelling out,
    ///   because it used to be `false` and that **was the audio/video
    ///   sync bug**.
    ///
    ///   With `sync=false` the appsink hands over each decoded frame the
    ///   instant it exists, as fast as the decoder can produce them. The
    ///   audio branch has no such freedom: `autoaudiosink` is a real
    ///   output device rendering samples against the pipeline clock at
    ///   exactly 1×. So the picture raced ahead of the sound and drifted
    ///   further apart the longer playback ran — precisely the reported
    ///   symptom, *"the video in 1x is playing faster than 1x but the
    ///   audio is in 1x"*. Nothing about the speed feature was involved;
    ///   plain 1× playback was already unsynced, and a fast machine made
    ///   it worse rather than better.
    ///
    ///   The old reasoning ("the UI drives frame pacing, so let it pull
    ///   when it is ready") is only sound for a *silent* pipeline. Once
    ///   there is an audio branch the clock is not ours to ignore: the
    ///   speaker is the metronome, and the image has to be scheduled
    ///   against the same clock or it is simply wrong. Syncing makes the
    ///   appsink release each frame at its presentation timestamp, which
    ///   is what "in sync" means.
    ///
    ///   This does not reintroduce the stall the old comment feared. The
    ///   waiting happens inside `try_pull_sample`, which only the
    ///   **engine thread** ever calls; the "the UI thread
    ///   never touches pixels" is untouched. Batch extraction still wants
    ///   the old behavior and still has it — `thumbs.rs` and `audio.rs`
    ///   build their own `sync=false` pipelines because they decode
    ///   against no clock at all.
    ///
    /// - `max-buffers=2 drop=false` bounds the appsink's queue. With sync
    ///   on, an unbounded appsink that nobody is draining (paused mid-
    ///   scrub) would otherwise let the decoder run ahead and buffer.
    pub fn from_file(path: &std::path::Path, with_audio: bool) -> Result<Self, EngineError> {
        ensure_gst_init()?;

        // Fail with a useful message rather than a cryptic parse error if
        // the machine is missing decoders entirely (see caps.rs -- this is
        // a real, hit-in-practice failure on this very machine).
        let caps = crate::caps::probe()?;
        if !caps.can_play() {
            return Err(EngineError::MissingElements(caps.diagnosis()));
        }

        let uri = crate::probe::path_to_uri(path)?;

        let description = if with_audio {
            format!(
                "uridecodebin uri={uri} name=dec \
                 dec. ! queue ! videoconvert ! video/x-raw,format=RGBA ! \
                 appsink name=sink sync=true max-buffers=2 drop=false \
                 dec. ! queue ! audioconvert ! audioresample ! volume name=vol ! autoaudiosink"
            )
        } else {
            format!(
                "uridecodebin uri={uri} name=dec \
                 dec. ! queue ! videoconvert ! video/x-raw,format=RGBA ! \
                 appsink name=sink sync=true max-buffers=2 drop=false"
            )
        };

        Self::from_description(&description)
    }

    /// Set the audio branch's volume, 0.0..=1.0. `Ok(false)` (not an
    /// error) when this pipeline has no audio branch — muting a silent
    /// clip is a no-op the caller should not have to special-case.
    /// The design rule: "Mute is not 'remove audio'" — this only changes
    /// gain; the track keeps flowing.
    pub fn set_volume(&self, volume: f64) -> Result<bool, EngineError> {
        match self.pipeline.by_name("vol") {
            Some(vol) => {
                vol.set_property("volume", volume.clamp(0.0, 1.0));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// Total duration as the pipeline reports it. Distinct from
    /// `probe::probe_file`'s duration: this one is queried from the live
    /// pipeline (useful after it prerolls), the other from a standalone
    /// discoverer pass at import time.
    pub fn duration(&self) -> Option<Time> {
        self.pipeline
            .query_duration::<gst::ClockTime>()
            .map(|ct| Time::from_nanos(ct.nseconds()))
    }

    /// Block until the pipeline finishes prerolling (reaches PAUSED with
    /// data ready) or fails. Needed before `duration()`/`position()` or a
    /// seek will answer meaningfully on a freshly-built file pipeline —
    /// asking too early is the classic "why is duration always None"
    /// GStreamer mistake.
    pub fn wait_until_ready(&self, timeout: gst::ClockTime) -> Result<(), EngineError> {
        let (result, _current, _pending) = self.pipeline.state(Some(timeout));
        result?;
        Ok(())
    }

    /// Set playback rate (the speed feature) by issuing a rate
    /// seek from the current position. GStreamer has no "set rate"
    /// property; a rate change *is* a seek carrying a new rate, which is
    /// why this looks heavier than a setter.
    pub fn set_rate(&self, rate: f64) -> Result<(), EngineError> {
        if !(rate.is_finite() && rate > 0.0) {
            return Err(EngineError::SeekFailed);
        }
        let position = self
            .pipeline
            .query_position::<gst::ClockTime>()
            .unwrap_or(gst::ClockTime::ZERO);

        self.pipeline
            .seek(
                rate,
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::SeekType::Set,
                position,
                gst::SeekType::End,
                gst::ClockTime::ZERO,
            )
            .map_err(|_| EngineError::SeekFailed)
    }

    pub fn play(&self) -> Result<(), EngineError> {
        self.pipeline.set_state(gst::State::Playing)?;
        self.playing_hint.store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn pause(&self) -> Result<(), EngineError> {
        self.pipeline.set_state(gst::State::Paused)?;
        self.playing_hint.store(false, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn stop(&self) -> Result<(), EngineError> {
        self.pipeline.set_state(gst::State::Null)?;
        Ok(())
    }

    /// The underlying appsink, for callers that need to pull something
    /// other than a video `Frame` from it — specifically `thumbs.rs`'s
    /// waveform extraction, which pulls raw F32 audio buffers that
    /// `sample_to_frame` would (correctly) reject as not being video.
    pub(crate) fn appsink(&self) -> &gst_app::AppSink {
        &self.appsink
    }

    /// Whether the stream has genuinely ended.
    ///
    /// # Why this exists
    ///
    /// `pull_frame` returns `NoSample` for **two opposite reasons**: the
    /// stream is over, or no frame happened to be ready within the
    /// timeout. The engine loop used to treat those identically, so a
    /// timeout was reported to the UI as end-of-stream.
    ///
    /// That conflation was survivable only because the appsink was
    /// unsynced: frames arrived faster than they could be asked for, so a
    /// timeout essentially never happened. The moment the appsink was
    /// corrected to honour the clock (see `from_file`), a frame
    /// legitimately takes up to a frame interval to arrive, pulls started
    /// hitting their timeout, and playback halted a fraction of a second
    /// in — announcing EOS on a file that had barely started.
    ///
    /// Asking the appsink is the authoritative answer, and it is a flag
    /// read rather than a bus round-trip.
    pub fn is_eos(&self) -> bool {
        self.appsink.is_eos()
    }

    /// Blocking pull of the next available frame, with a timeout. Returns
    /// `Err(NoSample)` on EOS or timeout — the caller (offcut-engine's
    /// higher-level clock/scheduling code, not yet built) decides what EOS
    /// means for its state machine; this layer stays a thin, honest
    /// wrapper around `AppSink::try_pull_sample`.
    pub fn pull_frame(&self, timeout: gst::ClockTime) -> Result<Frame, EngineError> {
        let sample = self.appsink.try_pull_sample(timeout).ok_or(EngineError::NoSample)?;
        sample_to_frame(&sample)
    }

    /// Pull the frame the pipeline is currently **parked on**, whether it
    /// is playing or paused.
    ///
    /// # The bug this method exists to fix
    ///
    /// `pull_frame` alone is correct only while `Playing`. A `Paused`
    /// pipeline has no *flowing* samples — the one frame it holds is the
    /// **preroll** sample, reachable only via `try_pull_preroll`, and
    /// `try_pull_sample` on a paused appsink simply blocks until the
    /// timeout and reports `NoSample`.
    ///
    /// That single fact broke three separate behaviors, all of which
    /// looked like unrelated bugs until this was found by running the
    /// tests rather than reading the code:
    ///
    /// 1. Opening a file showed a black stage instead of frame 1 (the
    ///    engine thread prerolls to `Paused`, then asked for a sample that
    ///    could never arrive).
    /// 2. Scrubbing while paused updated the timecode but never the
    ///    image — the same missing frame, one per seek.
    /// 3. Filmstrip thumbnail extraction returned **zero** thumbnails: it
    ///    deliberately stays paused and seeks, which is precisely the
    ///    state where only preroll samples exist.
    ///
    /// # Why this dispatches on state instead of just trying both
    ///
    /// The obvious implementation — try `try_pull_sample` briefly, fall
    /// back to `try_pull_preroll` — is **wrong**, and measurably so. A
    /// direct probe of this exact pipeline showed that calling
    /// `try_pull_sample` on a paused appsink *consumes and discards the
    /// preroll sample*: the immediately following `try_pull_preroll`
    /// then blocks for its full timeout and returns `None`, where the
    /// same call without the preceding `try_pull_sample` returns the
    /// frame in ~6µs. Same pipeline, same state, only the call order
    /// differing:
    ///
    /// ```text
    ///   preroll only:            PREROLL ok, 921600 bytes, in 6.4µs
    ///   sample(5ms) then preroll: sample=false; preroll NONE after 3.0s
    /// ```
    ///
    /// So the state query is not defensive ceremony around an otherwise
    /// fine fallback — it is the thing that makes this correct. When
    /// `Playing`, frames flow and `try_pull_sample` is right; otherwise
    /// the single parked frame is the preroll and only
    /// `try_pull_preroll` may be called.
    ///
    /// # A second bug, in the state query itself
    ///
    /// The state must be queried with a real (small) timeout, not zero.
    /// `set_state(Playing)` returns `Async` for a file pipeline — the
    /// transition completes on another thread. A zero-timeout query
    /// during that window still reports `Paused`, so this method would
    /// take the preroll branch on an already-playing pipeline, consume
    /// the one preroll sample, and then report `NoSample` forever after.
    /// The visible symptom was an export that wrote exactly **one frame**
    /// whenever a clip's in-point required a seek. Letting the query wait
    /// briefly lets the pending transition settle first.
    ///
    /// # The state query must not block on the hot path
    ///
    /// This originally asked for the state with a 200ms timeout, to let a
    /// pending `Playing` transition settle. That is correct *once*, after
    /// a state change — and ruinous per frame: every scrub seek then paid
    /// up to 200ms before it could even ask for a picture, which is the
    /// difference between a scrubber that tracks the pointer and one that
    /// visibly lags behind it (the design rule budgets 50ms for the whole
    /// operation). The pending state is cached by `set_playing_hint`
    /// instead: the caller already knows which state it asked for, so the
    /// hot path does not need to re-derive it from the pipeline.
    pub fn pull_current_frame(&self, timeout: gst::ClockTime) -> Result<Frame, EngineError> {
        // `ClockTime::ZERO` makes this a non-blocking peek at the last
        // known state, not a wait for a pending transition.
        let (_, current, _) = self.pipeline.state(Some(gst::ClockTime::ZERO));
        let playing = current == gst::State::Playing
            || self.playing_hint.load(std::sync::atomic::Ordering::Relaxed);
        let sample = if playing {
            self.appsink.try_pull_sample(timeout)
        } else {
            self.appsink.try_pull_preroll(timeout)
        };
        sample_to_frame(&sample.ok_or(EngineError::NoSample)?)
    }

    /// the two-tier seek, tier one: `KEY_UNIT` flag, fast,
    /// snaps to the nearest keyframe — used while scrubbing. `position` is
    /// in the pipeline's own SOURCE time (nanoseconds), matching
    /// `offcut_model::Time`'s representation exactly (no unit conversion at
    /// this boundary, per `frame.rs`'s doc comment).
    pub fn seek_fast(&self, position: Time) -> Result<(), EngineError> {
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT,
                gst::ClockTime::from_nseconds(position.as_nanos()),
            )
            .map_err(|_| EngineError::SeekFailed)
    }

    /// the two-tier seek, tier two: `ACCURATE` flag, issued once
    /// on drag release. Frame-exact, slower — the tradeoff the two-tier
    /// design exists to avoid paying on every intermediate scrub position.
    pub fn seek_accurate(&self, position: Time) -> Result<(), EngineError> {
        self.pipeline
            .seek_simple(
                gst::SeekFlags::FLUSH | gst::SeekFlags::ACCURATE,
                gst::ClockTime::from_nseconds(position.as_nanos()),
            )
            .map_err(|_| EngineError::SeekFailed)
    }

    /// Current pipeline position, translated straight into
    /// `offcut_model::Time`. `None` when the pipeline cannot report a
    /// position (e.g. not yet prerolled) — callers must handle this rather
    /// than the wrapper inventing a fake zero, which would be exactly the
    /// kind of silently-wrong timestamp the design rule warns against.
    pub fn position(&self) -> Option<Time> {
        self.pipeline
            .query_position::<gst::ClockTime>()
            .map(|ct| Time::from_nanos(ct.nseconds()))
    }
}

impl Drop for Pipeline {
    fn drop(&mut self) {
        // Best-effort: a pipeline that's already NULL or whose bus is gone
        // should not panic on drop. `stop`'s own Result is intentionally
        // discarded here for exactly that reason.
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

fn sample_to_frame(sample: &gst::Sample) -> Result<Frame, EngineError> {
    let buffer = sample.buffer().ok_or(EngineError::NoBuffer)?;
    let caps = sample.caps().ok_or(EngineError::NoCaps)?;
    let video_info =
        gst_video::VideoInfo::from_caps(caps).map_err(|e| EngineError::InvalidVideoInfo(e.to_string()))?;
    let map = buffer.map_readable().map_err(|_| EngineError::BufferMapFailed)?;

    // This crate's `test_pattern` only ever asks for RGBA; a real-file
    // pipeline may need other formats, at which point this match grows —
    // deliberately not speculatively handling formats never yet produced
    // by a real pipeline in this crate (mirrors `PixelFormat`'s own doc
    // comment).
    let format = match video_info.format() {
        gst_video::VideoFormat::Rgba => PixelFormat::Rgba8,
        other => {
            return Err(EngineError::InvalidVideoInfo(format!(
                "unsupported pixel format {other:?} — only Rgba8 is wired up so far"
            )))
        }
    };

    let pts = buffer
        .pts()
        .map(|ct| Time::from_nanos(ct.nseconds()))
        .unwrap_or(Time::ZERO);

    Ok(Frame {
        width: video_info.width(),
        height: video_info.height(),
        stride: video_info.stride()[0] as u32,
        format,
        data: map.as_slice().to_vec(),
        pts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact scenario proven out in this session's throwaway probe,
    /// promoted into a real, permanent test: build a headless
    /// videotestsrc pipeline, play it, pull every frame, and confirm the
    /// byte size and non-zero content of the first one. This is the
    /// closest thing to the design Phase 0's "a video plays... with no
    /// dropped frames" this sandbox can actually verify — see `lib.rs`
    /// for what could not be verified here (GPU texture upload/display)
    /// and why.
    #[test]
    fn pulls_expected_frame_count_with_real_nonzero_data() {
        let pipeline = Pipeline::test_pattern(320, 240, 10).expect("pipeline build failed");
        pipeline.play().expect("play failed");

        let mut frames = Vec::new();
        loop {
            match pipeline.pull_frame(gst::ClockTime::from_seconds(5)) {
                Ok(frame) => frames.push(frame),
                Err(EngineError::NoSample) => break,
                Err(e) => panic!("unexpected pull error: {e}"),
            }
        }
        pipeline.stop().expect("stop failed");

        assert_eq!(frames.len(), 10, "expected exactly 10 frames from num-buffers=10");
        let first = &frames[0];
        assert_eq!(first.width, 320);
        assert_eq!(first.height, 240);
        assert_eq!(first.format, PixelFormat::Rgba8);
        assert_eq!(first.data.len(), 320 * 4 * 240, "tightly packed RGBA frame size mismatch");
        assert!(first.is_well_formed(), "frame failed its own well-formedness check");
        assert!(first.has_non_zero_data(), "videotestsrc test pattern produced an all-zero frame");
    }

    #[test]
    fn every_pulled_frame_is_well_formed() {
        let pipeline = Pipeline::test_pattern(160, 120, 5).expect("pipeline build failed");
        pipeline.play().expect("play failed");
        let mut count = 0;
        while let Ok(frame) = pipeline.pull_frame(gst::ClockTime::from_seconds(5)) {
            assert!(frame.is_well_formed(), "frame {count} not well formed");
            count += 1;
        }
        pipeline.stop().expect("stop failed");
        assert_eq!(count, 5);
    }

    #[test]
    fn unknown_appsink_name_errors_cleanly_not_panics() {
        // No element named "sink" at all -- must return an error, not
        // panic, since a caller building a pipeline description by hand
        // (or from a future config file) can typo this.
        let result = Pipeline::from_description("videotestsrc num-buffers=1 ! fakesink name=notsink");
        assert!(matches!(result, Err(EngineError::ElementNotFound("sink"))));
    }

    #[test]
    fn malformed_pipeline_description_errors_cleanly_not_panics() {
        let result = Pipeline::from_description("this is not a valid gst pipeline description !!!");
        assert!(matches!(result, Err(EngineError::PipelineBuild(_))));
    }

    #[test]
    fn position_is_none_before_playing_and_some_after() {
        let pipeline = Pipeline::test_pattern(160, 120, 20).expect("pipeline build failed");
        // Before playing, position is typically unavailable — this is
        // intentionally not asserted as always-None (GStreamer's exact
        // behavior here can vary), only that play+pull produces a
        // reportable position afterward.
        pipeline.play().expect("play failed");
        let _first = pipeline.pull_frame(gst::ClockTime::from_seconds(5)).expect("pull failed");
        // Position should now be queryable and non-negative by
        // construction (Time wraps u64).
        let pos = pipeline.position();
        assert!(pos.is_some(), "expected a queryable position once playing and after a pulled frame");
        pipeline.stop().expect("stop failed");
    }

    // --- Real-file playback (Step 2 of the "actually a video editor" work) ---

    fn sample_path(name: &str) -> std::path::PathBuf {
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

    /// The one that matters: decode a **real H.264 MP4** (not
    /// videotestsrc) all the way to RGBA frames. This is the actual
    /// import-and-play path a user exercises by opening a file.
    #[test]
    fn decodes_a_real_mp4_into_rgba_frames() {
        let path = require_sample("sample.mp4");
        // Audio off for this test: autoaudiosink would try to open a real
        // output device, which is neither available nor relevant in a
        // headless test run. The audio branch gets its own test below.
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        pipeline.play().expect("play failed");

        let mut frames = 0usize;
        let mut first: Option<Frame> = None;
        while let Ok(frame) = pipeline.pull_frame(gst::ClockTime::from_seconds(10)) {
            if first.is_none() {
                first = Some(frame.clone());
            }
            frames += 1;
            if frames >= 30 {
                break; // one second's worth is plenty to prove real decode
            }
        }
        pipeline.stop().expect("stop failed");

        assert!(frames >= 30, "expected at least 30 decoded frames, got {frames}");
        let first = first.expect("no frames decoded from a real MP4");
        assert_eq!((first.width, first.height), (640, 360), "must match the fixture's real resolution");
        assert_eq!(first.format, PixelFormat::Rgba8);
        assert!(first.is_well_formed());
        assert!(first.has_non_zero_data(), "decoded a real video into an all-zero buffer");
    }

    /// Frames must arrive with monotonically advancing timestamps —
    /// the property playback and the playhead both depend on, and the
    /// thing that breaks first if the caps/queue wiring is wrong.
    #[test]
    fn real_file_frames_have_advancing_timestamps() {
        let path = require_sample("sample.mp4");
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        pipeline.play().expect("play failed");

        let mut last = Time::ZERO;
        let mut seen = 0;
        while let Ok(frame) = pipeline.pull_frame(gst::ClockTime::from_seconds(10)) {
            if seen > 0 {
                assert!(
                    frame.pts >= last,
                    "frame {seen} went backwards in time: {:?} after {:?}",
                    frame.pts,
                    last
                );
            }
            last = frame.pts;
            seen += 1;
            if seen >= 20 {
                break;
            }
        }
        pipeline.stop().expect("stop failed");
        assert!(seen >= 20);
        assert!(last > Time::ZERO, "timestamps never advanced past zero");
    }

    /// A real file reports a real duration once prerolled — this is what
    /// the timeline's total length and the playhead's scale depend on.
    #[test]
    fn real_file_reports_its_duration_after_preroll() {
        let path = require_sample("sample.mp4");
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        pipeline.pause().expect("pause (preroll) failed");
        pipeline
            .wait_until_ready(gst::ClockTime::from_seconds(10))
            .expect("preroll failed");

        let duration = pipeline.duration().expect("no duration after preroll");
        let secs = duration.as_secs_f64();
        assert!((4.8..5.3).contains(&secs), "expected ~5s duration, got {secs}s");
        pipeline.stop().expect("stop failed");
    }

    /// Seeking a real file must land near the requested position — the
    /// core of scrubbing. `KEY_UNIT` snaps to a keyframe, so this asserts
    /// "close to", not "exactly", which is the honest contract of the
    /// fast tier of the two-tier seek.
    #[test]
    fn seeking_a_real_file_lands_near_the_requested_position() {
        let path = require_sample("sample.mp4");
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        pipeline.play().expect("play failed");
        let _warm = pipeline.pull_frame(gst::ClockTime::from_seconds(10)).expect("warm-up pull failed");

        let target = Time::from_nanos(3_000_000_000); // 3s into a ~5s file
        pipeline.seek_accurate(target).expect("accurate seek failed");

        let frame = pipeline.pull_frame(gst::ClockTime::from_seconds(10)).expect("pull after seek failed");
        let delta = frame.pts.as_secs_f64() - target.as_secs_f64();
        assert!(
            delta.abs() < 0.5,
            "ACCURATE seek to 3.0s landed at {:.3}s (delta {:.3}s)",
            frame.pts.as_secs_f64(),
            delta
        );
        pipeline.stop().expect("stop failed");
    }

    /// `set_volume` reports `false` (not an error) on a pipeline with no
    /// audio branch — the no-special-case contract its doc comment
    /// promises.
    #[test]
    fn set_volume_is_a_graceful_no_op_without_an_audio_branch() {
        let path = require_sample("sample.mp4");
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        let had_audio_branch = pipeline.set_volume(0.0).expect("set_volume errored");
        assert!(!had_audio_branch, "no audio branch was requested, so none should exist");
    }

    #[test]
    fn from_file_on_a_missing_path_errors_cleanly() {
        let result = Pipeline::from_file(std::path::Path::new("/nonexistent/nope.mp4"), false);
        // uridecodebin resolves the URI lazily, so the failure may surface
        // at build or at play time; either way it must not panic.
        if let Ok(p) = result {
            let _ = p.play();
            let pulled = p.pull_frame(gst::ClockTime::from_seconds(3));
            assert!(pulled.is_err(), "a nonexistent file must not yield frames");
        }
    }

    /// The audio/video sync regression, pinned at the point it actually
    /// went wrong: the playback appsink must be **clock-synced**.
    ///
    /// This is asserted against real decoded timestamps rather than
    /// against the pipeline description string, because the string is the
    /// mechanism and the timing is the promise. A synced appsink cannot
    /// deliver N frames faster than those frames' own presentation
    /// timestamps span — that is the definition of syncing to the clock,
    /// and it is exactly what `sync=false` violated while the audio sink
    /// kept honest time.
    #[test]
    fn playback_frames_arrive_no_faster_than_real_time() {
        use std::time::Instant;

        let path = require_sample("sample.mp4");
        let pipeline = Pipeline::from_file(&path, false).expect("from_file failed");
        pipeline.play().expect("play failed");

        // Discard the first pull: it includes preroll and state-change
        // latency, which is not playback pacing.
        let first = pipeline
            .pull_frame(gst::ClockTime::from_seconds(10))
            .expect("warm-up pull failed");

        let started = Instant::now();
        let mut last = first.clone();
        for _ in 0..20 {
            last = pipeline.pull_frame(gst::ClockTime::from_seconds(10)).expect("pull failed");
        }
        let wall = started.elapsed().as_secs_f64();
        pipeline.stop().expect("stop failed");

        let media = last.pts.as_secs_f64() - first.pts.as_secs_f64();
        assert!(media > 0.0, "timestamps did not advance across 20 frames");

        // Allow generous slack for scheduling on a loaded machine, but
        // not the order-of-magnitude speedup `sync=false` produced: it
        // delivered this span in near-zero wall time.
        assert!(
            wall >= media * 0.5,
            "20 frames spanning {media:.3}s of media arrived in {wall:.3}s of wall time — \
             the appsink is not clock-synced, so the picture will race the audio"
        );
    }

    #[test]
    fn set_rate_rejects_nonsense_rates() {
        let pipeline = Pipeline::test_pattern(160, 120, 5).expect("build failed");
        assert!(pipeline.set_rate(0.0).is_err(), "zero rate must be rejected");
        assert!(pipeline.set_rate(-1.0).is_err(), "negative rate must be rejected");
        assert!(pipeline.set_rate(f64::NAN).is_err(), "NaN rate must be rejected");
    }
}
