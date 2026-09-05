//! The export pipeline: an edited timeline out to a real, playable MP4.
//!
//! # Shape
//!
//! ```text
//!   for each clip, in timeline order:
//!     decode source [in_point, out_point)   (a offcut-engine Pipeline)
//!       -> bake crop/adjust on the GPU      (offcut-render EffectsRenderer)
//!         -> push into appsrc               (this module)
//!            -> videoconvert -> x264enc -> h264parse -> mp4mux -> filesink
//! ```
//!
//! One encoder, one muxer, one output file, for the whole timeline —
//! the "Pipeline per clip segment, concatenated... encoded
//! once." Concatenation happens by *timestamping*: each frame pushed into
//! `appsrc` carries its position on the **output** timeline, so the
//! encoder sees one continuous stream and never learns there were clips.
//!
//! # Why `appsrc` and not `concat` + `gnlcomposition`
//!
//! The design rule rejected GStreamer Editing Services because "our timeline
//! is smaller than its API surface and we would fight its assumptions on
//! speed changes." The same argument applies to building a pure-GStreamer
//! concat graph here: the moment crop/adjust must be applied per clip,
//! from a wgpu shader, on the CPU-visible frames, the graph would need a
//! custom element anyway. Pushing baked frames into `appsrc` keeps every
//! per-clip decision (speed, crop, adjust, mute) in ordinary Rust, where
//! it is testable, and leaves GStreamer doing what it is unambiguously
//! best at: encoding and muxing.
//!
//! # Speed
//!
//! The design says the export's video branch gets "videorate +
//! adjusted PTS." Here it is purely adjusted PTS: a 2× clip's frames are
//! emitted at output timestamps half as far apart as their source
//! spacing, which *is* the speed change. `videorate` would then be a
//! no-op, so it is not in the graph — the output timestamps are already
//! at a constant rate by construction, because this module generates them
//! from the output frame index rather than passing source PTS through.
//!
//! # Audio
//!
//! Muxed, as of this revision — the gap the previous version of this
//! comment described is closed. A second `appsrc` carries F32 PCM into
//! `audioconvert ! <aac encoder> ! aacparse ! mp4mux`, joining the video
//! branch at the same muxer so one file comes out with both streams.
//!
//! Three decisions make this hold together:
//!
//! - **Length is computed, never observed.** `offcut-engine::audio`
//!   returns exactly `round(span / speed * 48000)` sample frames per
//!   clip, padding or truncating to get there. Video frames self-correct
//!   from their own timestamps; audio cannot, so a segment that decoded
//!   3 ms short would shift every later clip and the drift would
//!   accumulate. Per-segment exactness is what keeps A/V sync from being
//!   a thing to measure at the end.
//! - **Muted spans are written as silence, not skipped.** the design rule:
//!   "export writes silence for muted spans so the audio stream stays
//!   continuous and players do not glitch at clip boundaries." A muted
//!   clip still contributes its full sample count.
//! - **Speed is applied by `pitch tempo=`** inside the decode graph, so
//!   0.5× and 2× keep their original pitch. At 4×,
//!   `Speed::implies_mute` is true and the segment is silence by rule.
//!
//! If the machine has no AAC encoder at all, the export degrades to
//! video-only rather than failing: see `available_aac_encoder`.

use crate::error::ExportError;
use crate::settings::{ExportProgress, ExportSettings, output_framerate};
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use offcut_engine::Pipeline as DecodePipeline;
use offcut_model::{Project, Time};
use offcut_render::{EffectsRenderer, EffectsUniform, RenderContext};

/// A cancellation flag the UI can flip from another thread.
/// The design rule: exports are "Cancellable."
#[derive(Clone, Default, Debug)]
pub struct CancelFlag(Arc<AtomicBool>);

impl CancelFlag {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Resolve the output size for this project under these settings.
///
/// # The crop decides the shape, not the source
///
/// This used to resolve against the **source** resolution alone, which
/// silently discarded the crop: a 1:1 crop of a 16:9 file exported as
/// 16:9, with the square region stretched back across a widescreen
/// frame. The picture was right and the container was wrong, which is
/// the worst combination — it looks like a rendering bug in whatever
/// plays it.
///
/// Choosing "1:1" is a statement about the *output*, so the output has
/// to be square. The crop rect's proportions are applied to the source
/// resolution to get the real pixel dimensions being kept, and the
/// resolution preset then scales that — preserving the cropped shape
/// rather than the original one.
pub fn output_resolution(project: &Project, settings: &ExportSettings) -> (u32, u32) {
    let source = project.sources.first().map(|s| s.resolution).unwrap_or((1920, 1080));

    // The first clip's crop is what the timeline is showing. (Multi-clip
    // projects with differing crops would need a project-level output
    // shape; this app's job is one range out of one file.)
    let cropped = project
        .clips
        .first()
        .map(|clip| {
            let rect = clip.crop.rect;
            let w = (source.0 as f32 * rect.width).round().max(2.0) as u32;
            let h = (source.1 as f32 * rect.height).round().max(2.0) as u32;
            (w, h)
        })
        .unwrap_or(source);

    settings.resolution.resolve(cropped)
}

/// Export `project` to `destination`, calling `on_progress` as it goes.
///
/// # Atomic write
///
/// The design rule: "Writes to a temp file beside the target, renames on
/// success. A cancelled or crashed export never leaves a truncated file
/// at the user's chosen path." That is implemented literally here — the
/// muxer writes to `<destination>.offcut-partial`, and only a fully
/// successful, properly finalized encode gets renamed into place. The
/// temp file lives *beside* the target rather than in `/tmp` for a
/// specific reason: a rename across filesystems is not atomic (and on
/// Linux fails outright with `EXDEV`), and the user's output directory is
/// frequently on a different mount than `/tmp`.
pub fn export(
    project: &Project,
    destination: &Path,
    settings: &ExportSettings,
    cancel: &CancelFlag,
    mut on_progress: impl FnMut(ExportProgress),
) -> Result<(), ExportError> {
    if project.clips.is_empty() {
        return Err(ExportError::EmptyTimeline);
    }

    let caps = offcut_engine::caps::probe()?;
    if !caps.can_export() {
        return Err(ExportError::MissingElements(caps.diagnosis()));
    }

    let (out_width, out_height) = output_resolution(project, settings);
    let fps = output_framerate(project);
    let total = project.total_timeline_duration();

    let partial = partial_path(destination);
    // A leftover partial from a previous crash must not be appended to.
    let _ = std::fs::remove_file(&partial);

    let encoder = EncodePipeline::build(&partial, settings, out_width, out_height, fps, &caps)?;
    encoder.start()?;

    let ctx = pollster::block_on(RenderContext::new_headless())
        .map_err(|e| ExportError::Render(format!("could not open a GPU device for the export bake: {e}")))?;
    let mut effects_renderer = EffectsRenderer::new(&ctx.device);

    let frame_duration = fps.frame_duration();
    let mut output_frame_index: i64 = 0;

    let result = (|| -> Result<(), ExportError> {
        for clip in &project.clips {
            if cancel.is_cancelled() {
                return Err(ExportError::Cancelled);
            }
            let Some(source) = project.source(clip.source) else { continue };

            let aspect = if source.resolution.1 > 0 {
                source.resolution.0 as f32 / source.resolution.1 as f32
            } else {
                1.0
            };
            let effects = EffectsUniform::new(&clip.crop, &clip.adjust, aspect);

            // # Audio for this clip, pushed before its video
            //
            // Pushed first, and whole, so the generously-sized audio queue
            // is holding this segment while the video frames stream in —
            // mp4mux can then interleave the two without either branch
            // waiting on the single thread that feeds both.
            //
            // A muted clip contributes silence of exactly the same length
            // rather than nothing, which is what keeps the
            // audio stream continuous and every later clip in sync.
            if encoder.has_audio() {
                let muted = clip.effective_muted() || project.master_muted;
                let span = clip.source_span();
                let speed_factor = clip.speed.factor();
                let block = if muted || !source.has_audio {
                    offcut_engine::AudioBlock::silence(offcut_engine::frames_for_span(span, speed_factor))
                } else {
                    offcut_engine::decode_span(&source.path, clip.in_point, clip.out_point, speed_factor)?
                };
                let pts = fps.frame_to_time(output_frame_index);
                encoder.push_audio(&block, pts)?;
            }

            // Decode this clip's source span. Audio is not opened: this
            // path muxes video only (see the module doc comment), and
            // opening an audio branch we never read would have the
            // decoder block on a full queue.
            let decode = DecodePipeline::from_file(&source.path, false)?;
            decode.pause()?;
            decode.wait_until_ready(gst::ClockTime::from_seconds(15))?;
            // ACCURATE, not KEY_UNIT: an export must start exactly at the
            // trimmed in-point. The two-tier seek of the design rule is a
            // *scrubbing* optimization; paying the accurate-seek cost once
            // per clip in an export is invisible next to the encode.
            decode.seek_accurate(clip.in_point)?;
            decode.play()?;

            // Where this clip begins on the output timeline, in source
            // time terms: the frame index we started this clip at.
            let clip_first_output_frame = output_frame_index;
            let speed = clip.speed.factor();

            loop {
                if cancel.is_cancelled() {
                    let _ = decode.stop();
                    return Err(ExportError::Cancelled);
                }
                let frame = match decode.pull_current_frame(gst::ClockTime::from_seconds(5)) {
                    Ok(f) => f,
                    Err(_) => break, // EOS or timeout: this clip is done.
                };
                // Stop at the trim out-point. Source PTS is the right
                // comparison here because the decoder is playing the
                // source's own timeline.
                if frame.pts >= clip.out_point {
                    break;
                }
                if frame.pts < clip.in_point {
                    continue; // pre-roll frames before the seek settled
                }

                // # Speed, done by frame selection rather than by PTS alone
                //
                // Emitting every decoded frame and merely re-timestamping
                // them does NOT change the output duration: N frames at a
                // fixed output rate is always N/fps seconds long, whatever
                // timestamps they carry. (Measured: a 2× export of 4s of
                // source came out 3.97s, i.e. unchanged.) A speed change
                // is a *resampling* of the source: at 2× exactly half the
                // source frames appear, at 0.5× each appears twice.
                //
                // So the output frame index is derived from where this
                // source frame lands on the output timeline, and frames
                // that map to an index already written are dropped
                // (speed-up) while a gap is filled by repeating the frame
                // (slow-down). Deriving the index from the timestamp,
                // rather than counting emitted frames, is also what keeps
                // a dropped/duplicated decode from accumulating drift.
                let source_offset = frame.pts.as_nanos().saturating_sub(clip.in_point.as_nanos());
                let output_offset_nanos = (source_offset as f64 / speed) as u64;
                let target_index = clip_first_output_frame
                    + (output_offset_nanos as f64 / frame_duration.as_nanos() as f64).round() as i64;

                if target_index < output_frame_index {
                    continue; // 2×/4×: this source frame falls between output frames.
                }

                let baked = effects_renderer
                    .bake_frame(&ctx.device, &ctx.queue, &frame, &effects, out_width, out_height)
                    .map_err(|e| ExportError::Render(e.to_string()))?;

                // 0.5×: one source frame covers several output frames, so
                // it is pushed until the output catches up to it.
                while output_frame_index <= target_index {
                    let pts = fps.frame_to_time(output_frame_index);
                    encoder.push_frame(&baked, pts, frame_duration)?;
                    output_frame_index += 1;

                    if output_frame_index % 10 == 0 {
                        on_progress(ExportProgress { position: pts, total });
                    }
                }
            }
            let _ = decode.stop();
        }
        Ok(())
    })();

    match result {
        Ok(()) => {
            encoder.finish()?;
            on_progress(ExportProgress { position: total, total });
            // Only now, with a finalized and closed MP4, does the file
            // appear at the path the user chose.
            std::fs::rename(&partial, destination).map_err(|source| ExportError::Io {
                path: destination.to_path_buf(),
                source,
            })?;
            Ok(())
        }
        Err(e) => {
            let _ = encoder.abort();
            let _ = std::fs::remove_file(&partial);
            Err(e)
        }
    }
}

fn partial_path(destination: &Path) -> PathBuf {
    let mut name = destination.as_os_str().to_os_string();
    name.push(".offcut-partial");
    PathBuf::from(name)
}

/// The GStreamer half: `appsrc ! videoconvert ! encoder ! parser ! mp4mux
/// ! filesink`, plus the bus watch that turns an encoder error into a
/// `Result` instead of a silent truncated file.
struct EncodePipeline {
    pipeline: gst::Pipeline,
    appsrc: gst_app::AppSrc,
    /// `None` when audio was not requested, or was requested on a machine
    /// with no AAC encoder.
    audio_appsrc: Option<gst_app::AppSrc>,
}

impl EncodePipeline {
    fn build(
        partial: &Path,
        settings: &ExportSettings,
        width: u32,
        height: u32,
        fps: offcut_model::Rational,
        caps: &offcut_engine::Capabilities,
    ) -> Result<Self, ExportError> {
        // The design rule: "software x264enc first (confirmed installed, no
        // setup needed) with a runtime capability probe for vah264enc; if
        // the VA-API encode elements and driver are present, prefer them
        // and say so." The probe is `caps.has`, and the choice is logged
        // rather than silent.
        let hardware = settings.codec.hardware_element();
        let software = settings.codec.software_element();
        let encoder_name = if caps.has(hardware) { hardware } else { software };
        if !caps.has(encoder_name) {
            return Err(ExportError::MissingElements(format!(
                "neither {hardware} (hardware) nor {software} (software) is available for {}",
                settings.codec.label()
            )));
        }

        // # Check the muxer and parser too, before building the graph
        //
        // Only the encoder was checked here, so a missing muxer or parser
        // reached `gst::parse::launch` and came back as a generic
        // pipeline-build failure — which is what the user saw as "Export
        // failed: failed to build the encode pipeline", a message naming
        // neither the element nor the fix.
        //
        // Every element the description mentions is verified first, so
        // the error can say which one is absent.
        let muxer_name = settings.container.muxer_element();
        let parser_name = settings.codec.parser_element();
        for (element, role) in
            [(muxer_name, settings.container.label()), (parser_name, settings.codec.label())]
        {
            if !caps.has(element) {
                return Err(ExportError::MissingElements(format!(
                    "{role} export needs the GStreamer element `{element}`, which this \
                     machine does not have"
                )));
            }
        }

        let bitrate = settings.bitrate_kbps;
        // x264enc takes `bitrate` in kbit/s; vah264enc also uses kbps.
        // `key-int-max` bounds the GOP so seeking in the exported file is
        // responsive -- a 10-second GOP makes an otherwise fine export
        // feel broken in a player's scrubber.
        let encoder_config = if encoder_name.starts_with("x26") {
            format!("{encoder_name} bitrate={bitrate} speed-preset=medium key-int-max={}", fps.as_f64().round() as u32 * 2)
        } else {
            format!("{encoder_name} bitrate={bitrate}")
        };

        // Audio is opt-in via settings AND conditional on this machine
        // having an encoder. A missing AAC encoder degrades to video-only
        // rather than failing the whole export.
        let audio_encoder = if settings.include_audio {
            crate::settings::available_aac_encoder(caps)
        } else {
            None
        };

        let location = partial.to_string_lossy();
        let muxer = settings.container.muxer_element();
        // The muxer is named so the audio branch can request a second pad
        // on the *same* muxer. Two instances would be two files.
        let mut description = format!(
            "{muxer} name=mux ! filesink location=\"{location}\" \
             appsrc name=src format=time is-live=false \
             caps=video/x-raw,format=RGBA,width={width},height={height},framerate={}/{} ! \
             queue ! videoconvert ! {encoder_config} ! {} ! mux.",
            fps.num,
            fps.den,
            settings.codec.parser_element(),
        );

        if let Some(aac) = audio_encoder {
            // `aacparse` supplies the stream metadata the ISO-BMFF
            // muxers require, and is only inserted when it is both
            // present and wanted: Matroska takes the encoder's output
            // directly, and adding a parser it does not need is a
            // needless element that can fail to negotiate.
            let parse = if settings.container.needs_aac_parser() && caps.has("aacparse") {
                " aacparse ! "
            } else {
                " "
            };
            let rate = offcut_engine::EXPORT_SAMPLE_RATE;
            let channels = offcut_engine::EXPORT_CHANNELS;
            let bitrate = settings.audio_bitrate_bps;
            // `queue` on this branch is load-bearing, not decoration: both
            // appsrcs are pushed from this one thread, and mp4mux will not
            // accept a video buffer until it has audio covering the same
            // running time (and vice versa). Without a queue to absorb the
            // lead, the single pusher blocks in one branch waiting for the
            // muxer, which is waiting for the other branch, which only
            // this same blocked thread can feed: a deadlock. The queue is
            // sized generously for the same reason.
            description.push_str(&format!(
                " appsrc name=asrc format=time is-live=false \
                 caps=audio/x-raw,format=F32LE,rate={rate},channels={channels},layout=interleaved ! \
                 queue max-size-buffers=0 max-size-bytes=0 max-size-time=0 ! \
                 audioconvert ! audioresample ! {aac} bitrate={bitrate} !{parse} mux."
            ));
        }

        let element = gst::parse::launch(&description)
            .map_err(|e| ExportError::PipelineBuild(format!("{e} (pipeline: {description})")))?;
        let pipeline = element
            .downcast::<gst::Pipeline>()
            .map_err(|_| ExportError::PipelineBuild("export graph is not a Pipeline".into()))?;
        let appsrc = pipeline
            .by_name("src")
            .ok_or_else(|| ExportError::PipelineBuild("appsrc 'src' vanished from the graph".into()))?
            .downcast::<gst_app::AppSrc>()
            .map_err(|_| ExportError::PipelineBuild("'src' was not an appsrc".into()))?;

        let audio_appsrc = match audio_encoder {
            Some(_) => Some(
                pipeline
                    .by_name("asrc")
                    .ok_or_else(|| ExportError::PipelineBuild("audio appsrc 'asrc' vanished".into()))?
                    .downcast::<gst_app::AppSrc>()
                    .map_err(|_| ExportError::PipelineBuild("'asrc' was not an appsrc".into()))?,
            ),
            None => None,
        };

        Ok(Self { pipeline, appsrc, audio_appsrc })
    }

    /// Whether this graph actually has an audio branch. Distinct from
    /// `settings.include_audio`: the user can ask for audio on a machine
    /// that cannot encode it, and the caller needs to know which happened
    /// so it can skip the decode work rather than throw samples away.
    fn has_audio(&self) -> bool {
        self.audio_appsrc.is_some()
    }

    fn start(&self) -> Result<(), ExportError> {
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| ExportError::Encode(format!("could not start the encoder: {e}")))?;
        Ok(())
    }

    fn push_frame(
        &self,
        frame: &offcut_engine::Frame,
        pts: Time,
        duration: Time,
    ) -> Result<(), ExportError> {
        let mut buffer = gst::Buffer::with_size(frame.data.len())
            .map_err(|e| ExportError::Encode(format!("could not allocate a {}-byte buffer: {e}", frame.data.len())))?;
        {
            let buffer = buffer.get_mut().expect("freshly allocated buffer is uniquely owned");
            buffer.set_pts(gst::ClockTime::from_nseconds(pts.as_nanos()));
            buffer.set_duration(gst::ClockTime::from_nseconds(duration.as_nanos()));
            let mut map = buffer
                .map_writable()
                .map_err(|e| ExportError::Encode(format!("could not map the buffer writable: {e}")))?;
            map.as_mut_slice().copy_from_slice(&frame.data);
        }

        self.appsrc
            .push_buffer(buffer)
            .map_err(|flow| ExportError::Encode(format!("appsrc rejected a frame: {flow:?}")))?;
        Ok(())
    }

    /// Push one clip's worth of PCM at `pts` on the output timeline.
    ///
    /// The buffer carries an explicit PTS and duration derived from the
    /// sample count rather than being left for the muxer to infer. mp4mux
    /// interleaves by running time, and an audio buffer with no timestamp
    /// is placed wherever the encoder's own counter happens to be — which
    /// is the classic "audio drifts later and later" export bug.
    fn push_audio(&self, block: &offcut_engine::AudioBlock, pts: Time) -> Result<(), ExportError> {
        let Some(asrc) = &self.audio_appsrc else { return Ok(()) };
        if block.samples.is_empty() {
            return Ok(());
        }

        let bytes: Vec<u8> = block.samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut buffer = gst::Buffer::with_size(bytes.len())
            .map_err(|e| ExportError::Encode(format!("could not allocate an audio buffer: {e}")))?;
        {
            let buffer = buffer.get_mut().expect("freshly allocated buffer is uniquely owned");
            buffer.set_pts(gst::ClockTime::from_nseconds(pts.as_nanos()));
            buffer.set_duration(gst::ClockTime::from_nseconds(block.duration().as_nanos()));
            let mut map = buffer
                .map_writable()
                .map_err(|e| ExportError::Encode(format!("could not map the audio buffer: {e}")))?;
            map.as_mut_slice().copy_from_slice(&bytes);
        }

        asrc.push_buffer(buffer)
            .map_err(|flow| ExportError::Encode(format!("audio appsrc rejected a block: {flow:?}")))?;
        Ok(())
    }

    /// End the stream and wait for the muxer to write its index and close
    /// the file. **Skipping this is what produces an unplayable MP4**: the
    /// `moov` atom is written at EOS, so a file whose pipeline was merely
    /// set to NULL has all its frames and no way for a player to find
    /// them.
    fn finish(&self) -> Result<(), ExportError> {
        self.appsrc
            .end_of_stream()
            .map_err(|e| ExportError::Encode(format!("could not signal end of stream: {e}")))?;
        // Both branches must reach EOS or mp4mux never finalizes: it waits
        // for every sink pad it has. Ending only the video branch is a
        // 120-second hang followed by a file with no `moov` atom.
        if let Some(asrc) = &self.audio_appsrc {
            asrc.end_of_stream()
                .map_err(|e| ExportError::Encode(format!("could not end the audio stream: {e}")))?;
        }

        let bus = self
            .pipeline
            .bus()
            .ok_or_else(|| ExportError::Encode("the encode pipeline has no bus".into()))?;

        // Wait for EOS to travel all the way to filesink, or for an error.
        // Without this wait the rename below would move a file the muxer
        // has not finished writing.
        let timeout = gst::ClockTime::from_seconds(120);
        let mut outcome = Ok(());
        for message in bus.iter_timed(timeout) {
            match message.view() {
                gst::MessageView::Eos(_) => break,
                gst::MessageView::Error(err) => {
                    outcome = Err(ExportError::Encode(format!(
                        "{} ({})",
                        err.error(),
                        err.debug().unwrap_or_else(|| "no debug info".into())
                    )));
                    break;
                }
                _ => {}
            }
        }

        let _ = self.pipeline.set_state(gst::State::Null);
        outcome
    }

    fn abort(&self) -> Result<(), ExportError> {
        let _ = self.pipeline.set_state(gst::State::Null);
        Ok(())
    }
}

impl Drop for EncodePipeline {
    fn drop(&mut self) {
        let _ = self.pipeline.set_state(gst::State::Null);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// End-to-end: a 1:1 crop must write a **square file**, verified by
    /// probing the encoded output rather than by trusting the settings.
    ///
    /// `--ignored` because it runs a real encode.
    #[test]
    #[ignore]
    fn a_square_crop_writes_a_square_file_on_disk() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../media/sample.mp4");
        assert!(fixture.exists(), "missing fixture {}", fixture.display());

        let info = offcut_engine::probe::probe_file(&fixture, gstreamer::ClockTime::from_seconds(15))
            .expect("probe fixture");

        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: fixture.clone(),
            duration: info.duration,
            fps: info.fps,
            resolution: info.resolution,
            has_audio: false,
        };
        let sid = source.id;
        project.add_source(source);
        project.add_clip_for_source(sid).unwrap();
        project.clips[0].out_point = Time::from_nanos(1_000_000_000);

        let aspect = info.resolution.0 as f64 / info.resolution.1 as f64;
        project.clips[0].crop.apply_aspect(offcut_model::AspectPreset::Square, aspect);

        let out = std::env::temp_dir().join("offcut-square-crop.mp4");
        let _ = std::fs::remove_file(&out);
        export(
            &project,
            &out,
            &ExportSettings::default(),
            &CancelFlag::new(),
            |_| {},
        )
        .expect("export failed");

        let probed = offcut_engine::probe::probe_file(&out, gstreamer::ClockTime::from_seconds(15))
            .expect("probe output");
        let (w, h) = probed.resolution;
        let ratio = w as f64 / h as f64;
        eprintln!("exported {w}x{h} (ratio {ratio:.3}) from a {:?} source", info.resolution);
        assert!(
            (ratio - 1.0).abs() < 0.02,
            "a 1:1 crop wrote a {w}x{h} file (ratio {ratio:.3}) -- not square"
        );
        let _ = std::fs::remove_file(&out);
    }

    /// Choosing a ratio must change the **exported frame size**, not
    /// just the picture inside an unchanged frame.
    ///
    /// The defect: `output_resolution` resolved against the source only,
    /// so a 1:1 crop of a 16:9 file exported 16:9 with the square region
    /// stretched across it. The pixels were right and the container was
    /// wrong, which reads as a bug in whatever plays the file.
    #[test]
    fn a_square_crop_exports_a_square_file() {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/wide.mp4".into(),
            duration: Time::from_nanos(5_000_000_000),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: false,
        };
        let sid = source.id;
        project.add_source(source);
        project.add_clip_for_source(sid).unwrap();

        // Uncropped, the output matches the source.
        let settings = ExportSettings::default();
        assert_eq!(output_resolution(&project, &settings), (1920, 1080));

        // Now crop to 1:1 and the output must become square.
        project.clips[0].crop.apply_aspect(offcut_model::AspectPreset::Square, 16.0 / 9.0);
        let (w, h) = output_resolution(&project, &settings);
        let ratio = w as f64 / h as f64;
        assert!(
            (ratio - 1.0).abs() < 0.02,
            "a 1:1 crop exported {w}x{h} (ratio {ratio:.3}) -- the file is not square, \
             so the square picture will be stretched to fill it"
        );
    }

    /// Every preset must produce a file of that shape, not just 1:1.
    #[test]
    fn every_aspect_preset_exports_a_file_of_that_shape() {
        for preset in offcut_model::AspectPreset::ALL {
            let Some(want) = preset.ratio() else { continue };

            let mut project = Project::new();
            let source = Source {
                id: SourceId::next(),
                path: "/tmp/wide.mp4".into(),
                duration: Time::from_nanos(5_000_000_000),
                fps: Rational::WEB_30,
                resolution: (1920, 1080),
                has_audio: false,
            };
            let sid = source.id;
            project.add_source(source);
            project.add_clip_for_source(sid).unwrap();
            project.clips[0].crop.apply_aspect(preset, 16.0 / 9.0);

            let (w, h) = output_resolution(&project, &ExportSettings::default());
            let got = w as f64 / h as f64;
            assert!(
                (got - want).abs() < 0.03,
                "{preset:?} exported {w}x{h} (ratio {got:.3}), wanted {want:.3}"
            );
            assert_eq!(w % 2, 0, "{preset:?} gave an odd width {w}, which x264enc rejects");
            assert_eq!(h % 2, 0, "{preset:?} gave an odd height {h}");
        }
    }

    /// A resolution preset must scale the *cropped* shape, not restore
    /// the source's.
    #[test]
    fn a_resolution_preset_preserves_the_cropped_shape() {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/wide.mp4".into(),
            duration: Time::from_nanos(5_000_000_000),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: false,
        };
        let sid = source.id;
        project.add_source(source);
        project.add_clip_for_source(sid).unwrap();
        project.clips[0].crop.apply_aspect(offcut_model::AspectPreset::Square, 16.0 / 9.0);

        let settings = ExportSettings {
            resolution: crate::settings::ResolutionPreset::P720,
            ..Default::default()
        };
        let (w, h) = output_resolution(&project, &settings);
        assert_eq!(h, 720, "the preset should set the height");
        assert!(
            (w as f64 / h as f64 - 1.0).abs() < 0.02,
            "720p of a square crop gave {w}x{h}, which is not square"
        );
    }

    use crate::settings::{ResolutionPreset, VideoCodec};
    use offcut_model::{AdjustSettings, AdjustValue, Rational, Source, SourceId, Speed};

    fn fixture(name: &str) -> PathBuf {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../media").join(name);
        assert!(p.exists(), "missing fixture {} — run offcut/tools/make-sample.sh", p.display());
        p
    }

    /// A project of one clip covering `secs` of the real sample file.
    fn project_from_fixture(name: &str, in_secs: f64, out_secs: f64) -> Project {
        let path = fixture(name);
        let info = offcut_engine::probe_file(&path, gst::ClockTime::from_seconds(15)).expect("probe failed");
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path,
            duration: info.duration,
            fps: info.fps,
            resolution: info.resolution,
            has_audio: info.has_audio,
        };
        let source_id = source.id;
        project.add_source(source);
        let clip_id = project.add_clip_for_source(source_id).expect("add clip");
        project
            .trim_clip(
                clip_id,
                Some(Time::from_nanos((in_secs * 1e9) as u64)),
                Some(Time::from_nanos((out_secs * 1e9) as u64)),
            )
            .expect("trim");
        project
    }

    fn out_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("offcut-export-tests");
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let path = dir.join(name);
        let _ = std::fs::remove_file(&path);
        path
    }

    /// The headline claim of Phase 6: a real, playable MP4 comes out.
    /// Verified by re-probing the written file with GStreamer's own
    /// discoverer — if the `moov` atom were missing or the stream
    /// malformed, this probe fails exactly as a player would.
    #[test]
    fn exports_a_real_playable_mp4_that_probes_correctly() {
        let project = project_from_fixture("sample.mp4", 1.0, 3.0);
        let destination = out_path("basic.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };

        let mut last = None;
        export(&project, &destination, &settings, &CancelFlag::new(), |p| last = Some(p))
            .expect("export failed");

        assert!(destination.exists(), "no output file was written");
        let size = std::fs::metadata(&destination).unwrap().len();
        assert!(size > 1000, "output file is implausibly small ({size} bytes)");

        // The real test: GStreamer can open what we wrote.
        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15))
            .expect("the exported file is not a readable video");
        assert_eq!(info.resolution, (640, 360), "should match the source resolution");
        assert_eq!(info.video_codec.as_deref(), Some("H.264"));

        // ~2 seconds of content (1.0s -> 3.0s), allowing GOP rounding.
        let secs = info.duration.as_secs_f64();
        assert!((1.5..2.6).contains(&secs), "expected ~2s of output, got {secs}s");

        let last = last.expect("progress was never reported");
        assert_eq!(last.fraction(), 1.0, "the final progress report should be 100%");
    }

    /// The atomic-write guarantee: nothing is left at the user's chosen
    /// path when an export fails or is cancelled, and no `.offcut-partial`
    /// litter survives either.
    #[test]
    fn a_cancelled_export_leaves_no_file_at_the_destination() {
        let project = project_from_fixture("sample.mp4", 0.0, 5.0);
        let destination = out_path("cancelled.mp4");

        let cancel = CancelFlag::new();
        cancel.cancel(); // cancelled before it starts

        let result = export(&project, &destination, &ExportSettings::default(), &cancel, |_| {});
        assert!(matches!(result, Err(ExportError::Cancelled)), "got {result:?}");
        assert!(!destination.exists(), "a cancelled export must not leave a file at the target path");
        assert!(!partial_path(&destination).exists(), "the partial file must be cleaned up too");
    }

    /// Cancellation mid-encode (not just before it starts) must also be
    /// clean — this is the case a user actually hits, by clicking Cancel
    /// while the progress bar is moving.
    #[test]
    fn cancelling_partway_through_still_leaves_nothing_behind() {
        let project = project_from_fixture("sample.mp4", 0.0, 5.0);
        let destination = out_path("cancelled-mid.mp4");
        let cancel = CancelFlag::new();

        let result = export(&project, &destination, &ExportSettings::default(), &cancel, |p| {
            if p.fraction() > 0.2 {
                cancel.cancel();
            }
        });
        assert!(matches!(result, Err(ExportError::Cancelled)), "got {result:?}");
        assert!(!destination.exists());
        assert!(!partial_path(&destination).exists());
    }

    #[test]
    fn an_empty_project_is_refused_rather_than_writing_a_zero_frame_file() {
        let destination = out_path("empty.mp4");
        let result = export(&Project::new(), &destination, &ExportSettings::default(), &CancelFlag::new(), |_| {});
        assert!(matches!(result, Err(ExportError::EmptyTimeline)));
        assert!(!destination.exists());
    }

    /// A resolution preset must actually change the encoded file's
    /// dimensions — proving the export honors the sheet rather than
    /// always writing source size.
    #[test]
    fn a_resolution_preset_changes_the_encoded_files_dimensions() {
        let project = project_from_fixture("sample.mp4", 0.5, 1.5);
        let destination = out_path("scaled.mp4");
        let settings = ExportSettings {
            resolution: ResolutionPreset::P480,
            bitrate_kbps: 1500,
            ..Default::default()
        };
        // Source is 640x360, which is below 480p, so "never upscale"
        // applies and the output stays 640x360. Assert the resolved
        // decision directly, then that the file matches it.
        let expected = output_resolution(&project, &settings);
        assert_eq!(expected, (640, 360), "480p of a 360p source must not upscale");

        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");
        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert_eq!(info.resolution, expected);
    }

    /// Two clips must concatenate into one continuous stream whose
    /// duration is the sum — the "encoded once, not once per clip"
    /// property.
    #[test]
    fn a_multi_clip_timeline_exports_as_one_continuous_file() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 4.0);
        let clip_id = project.clips[0].id;
        project.split_clip(clip_id, Time::from_nanos(2_000_000_000)).expect("split");
        assert_eq!(project.clips.len(), 2);

        let destination = out_path("two-clips.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        let secs = info.duration.as_secs_f64();
        assert!((3.4..4.6).contains(&secs), "two 2s clips should total ~4s, got {secs}s");
    }

    /// Speed must actually shorten the output. A 2× clip of 4 seconds of
    /// source must produce ~2 seconds of file — this is the check that
    /// would fail if the output PTS were passed through from the source
    /// instead of generated from the output frame index.
    #[test]
    fn a_2x_clip_exports_to_half_the_duration() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 4.0);
        project.clips[0].speed = Speed::Two;
        assert!((project.total_timeline_duration().as_secs_f64() - 2.0).abs() < 0.1);

        let destination = out_path("speed-2x.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        let secs = info.duration.as_secs_f64();
        assert!((1.5..2.6).contains(&secs), "a 2x export of 4s of source should be ~2s, got {secs}s");
    }

    /// Crop and adjust must be *baked* into the exported pixels, not
    /// dropped. Comparing byte sizes would be flaky; instead this exports
    /// the same span twice — once clean, once with a full vignette — and
    /// asserts the encoder produced measurably different output. A heavy
    /// vignette darkens most of the frame, which any real encoder
    /// compresses differently.
    #[test]
    fn crop_and_adjust_are_baked_into_the_exported_pixels() {
        let clean_project = project_from_fixture("sample.mp4", 0.0, 2.0);
        let mut adjusted_project = project_from_fixture("sample.mp4", 0.0, 2.0);
        adjusted_project.clips[0].adjust = AdjustSettings {
            vignette: AdjustValue::new(100),
            ..Default::default()
        };
        adjusted_project.clips[0].crop.apply_aspect(offcut_model::AspectPreset::Square, 16.0 / 9.0);

        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        let clean_path = out_path("bake-clean.mp4");
        let adjusted_path = out_path("bake-adjusted.mp4");

        export(&clean_project, &clean_path, &settings, &CancelFlag::new(), |_| {}).expect("clean export");
        export(&adjusted_project, &adjusted_path, &settings, &CancelFlag::new(), |_| {}).expect("adjusted export");

        let clean_size = std::fs::metadata(&clean_path).unwrap().len();
        let adjusted_size = std::fs::metadata(&adjusted_path).unwrap().len();
        assert_ne!(
            clean_size, adjusted_size,
            "a full vignette + 1:1 crop produced a byte-identical file — the effects were not baked in"
        );
    }

    /// The headline claim of the audio branch: the exported file has an
    /// audio track at all. Asserted by re-probing with GStreamer's own
    /// discoverer — the same check a player makes — rather than by
    /// inspecting our own intent.
    #[test]
    fn the_exported_file_actually_contains_an_audio_track() {
        let project = project_from_fixture("sample.mp4", 0.0, 2.0);
        let destination = out_path("with-audio.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        assert!(settings.include_audio, "audio is on by default");

        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(
            info.has_audio,
            "the exported MP4 has no audio track — the audio branch did not reach the muxer"
        );
    }

    /// Turning audio off must actually produce a silent file, not merely
    /// be recorded as a preference. This is the test that fails if
    /// `include_audio` goes back to being "honored by being reported".
    #[test]
    fn include_audio_false_produces_a_file_with_no_audio_track() {
        let project = project_from_fixture("sample.mp4", 0.0, 2.0);
        let destination = out_path("no-audio.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, include_audio: false, ..Default::default() };

        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(!info.has_audio, "include_audio=false still muxed an audio track");
    }

    /// A/V sync, asserted as the property that actually matters: the
    /// audio stream's duration matches the video's. A drifting export is
    /// one where these diverge, and the divergence grows with clip count
    /// — so this uses a three-clip timeline rather than one.
    #[test]
    fn audio_and_video_durations_agree_across_a_multi_clip_timeline() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 4.0);
        let clip_id = project.clips[0].id;
        project.split_clip(clip_id, Time::from_nanos(1_500_000_000)).expect("split");
        let second = project.clips[1].id;
        project.split_clip(second, Time::from_nanos(3_000_000_000)).expect("split again");
        assert_eq!(project.clips.len(), 3);

        let destination = out_path("sync-3clip.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(info.has_audio, "no audio track to check sync against");

        // The container duration is the longer of the two streams. If the
        // audio ran short or long, the file's duration would drift away
        // from the timeline's own computed total.
        let expected = project.total_timeline_duration().as_secs_f64();
        let actual = info.duration.as_secs_f64();
        assert!(
            (actual - expected).abs() < 0.35,
            "exported duration {actual}s drifted from the timeline's {expected}s — \
             an audio segment was not the length its clip required"
        );
    }

    /// The design rule: a muted clip must still contribute *silence* to a
    /// continuous stream, not a hole. The file must therefore still have
    /// an audio track, and still be the right length.
    #[test]
    fn a_muted_clip_still_produces_a_continuous_audio_track() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 2.0);
        project.clips[0].muted = true;

        let destination = out_path("muted.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(
            info.has_audio,
            "a muted clip must write silence, not drop the audio track entirely"
        );
        let secs = info.duration.as_secs_f64();
        assert!((1.5..2.6).contains(&secs), "muted export should still be ~2s, got {secs}s");
    }

    /// Master mute is the project-wide switch and must reach the export
    /// the same way per-clip mute does.
    #[test]
    fn master_mute_silences_the_export_without_removing_the_track() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 2.0);
        project.master_muted = true;

        let destination = out_path("master-muted.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(info.has_audio, "master mute must write silence, not drop the track");
    }

    /// A 2× clip's audio must be half as long as its source span, or it
    /// would run past the end of its own picture. Asserted through the
    /// exported file's total duration.
    #[test]
    fn a_2x_clip_exports_audio_that_ends_with_its_video() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 4.0);
        project.clips[0].speed = Speed::Two;

        let destination = out_path("speed-2x-audio.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(info.has_audio);
        let secs = info.duration.as_secs_f64();
        // If the audio were pushed at 1x length, the container would be
        // ~4s (audio) instead of ~2s: the failure this test names.
        assert!(
            (1.5..2.6).contains(&secs),
            "a 2x export should be ~2s including its audio, got {secs}s — \
             the audio segment was not time-stretched to match the video"
        );
    }

    /// `Speed::implies_mute` at 4× must be honored by the export, not
    /// only by the UI and the monitor volume.
    #[test]
    fn a_4x_clip_is_muted_in_the_export_by_rule() {
        let mut project = project_from_fixture("sample.mp4", 0.0, 4.0);
        project.clips[0].speed = Speed::Four;
        assert!(project.clips[0].effective_muted(), "4x implies mute in the model");

        let destination = out_path("speed-4x.mp4");
        let settings = ExportSettings { bitrate_kbps: 2000, ..Default::default() };
        export(&project, &destination, &settings, &CancelFlag::new(), |_| {}).expect("export failed");

        let info = offcut_engine::probe_file(&destination, gst::ClockTime::from_seconds(15)).expect("probe");
        assert!(info.has_audio, "4x writes silence, which still needs a track");
    }

    #[test]
    fn output_resolution_falls_back_sensibly_for_a_project_with_no_sources() {
        let project = Project::new();
        let (w, h) = output_resolution(&project, &ExportSettings::default());
        assert!(w > 0 && h > 0 && w % 2 == 0 && h % 2 == 0);
    }

    #[test]
    fn output_framerate_uses_the_sources_real_rate() {
        let project = project_from_fixture("sample.mp4", 0.0, 1.0);
        assert_eq!(output_framerate(&project), Rational::new(30, 1));
    }

    #[test]
    fn hevc_selects_the_h265_encoder_and_parser_names() {
        assert_eq!(VideoCodec::Hevc.software_element(), "x265enc");
        assert_eq!(VideoCodec::Hevc.parser_element(), "h265parse");
    }
}
