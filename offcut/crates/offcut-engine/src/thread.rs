//! The engine thread: the middle box, made real.
//!
//! ```text
//!  UI thread  --EngineCommand (mpsc)-->  Engine thread (owns the pipeline)
//!             <--EngineEvent  (mpsc)---
//! ```
//!
//! The UI never touches a `gst::Pipeline` directly. It sends commands and
//! receives events, which is what keeps `Pipeline`'s blocking calls
//! (`pull_frame`, state changes, seeks) off the render loop entirely.
//!
//! **Why this is a plain OS thread and not an async task:** every
//! interesting GStreamer call here blocks — `pull_frame` waits on the
//! appsink, `set_state` waits for a state transition, a flushing seek
//! waits for the pipeline to settle. Running those on an async executor
//! starves whatever else shares that executor; this project already hit
//! exactly that bug once (a blocking `recv()` inside an async subscription
//! task silently froze frame delivery — see `offcut-app`'s
//! `video_frame_stream` doc comment). A dedicated thread is the correct
//! shape for a blocking producer.

use crate::error::EngineError;
use crate::frame::Frame;
use crate::pipeline::Pipeline;
use gstreamer as gst;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use offcut_model::Time;

/// Commands the UI sends to the engine thread.
#[derive(Debug, Clone)]
pub enum EngineCommand {
    /// Load a media file and preroll it (paused on the first frame).
    Open(PathBuf),
    Play,
    Pause,
    /// The design rule tier one: fast, keyframe-snapping — while dragging.
    SeekFast(Time),
    /// The design rule tier two: frame-accurate — on drag release.
    SeekAccurate(Time),
    /// The design rule: playback rate for the speed feature.
    SetRate(f64),
    /// The design rule: 0.0 = muted, 1.0 = full.
    SetVolume(f64),
    /// Stop playback and release the current file.
    Close,
    /// Ask the thread to exit; it will drop its pipeline cleanly.
    Shutdown,
}

/// Events the engine thread sends back to the UI.
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A file was opened successfully; carries its real, probed properties.
    Opened { path: PathBuf, info: crate::probe::MediaInfo },
    /// A newly decoded frame, ready for upload. `Arc` because the UI may
    /// hold it across several redraws without re-decoding.
    Frame(Arc<Frame>),
    /// Playback position advanced (emitted alongside frames).
    Position(Time),
    /// Playback state actually changed (as opposed to being requested).
    PlayingChanged(bool),
    /// End of stream reached.
    Eos,
    /// Something failed. The UI surfaces this rather than the engine
    /// panicking a thread the UI can't see (the design rule Phase 7's "error
    /// surfaces").
    Error(String),
}

/// Handle to a running engine thread. Dropping it shuts the thread down.
pub struct EngineHandle {
    commands: Sender<EngineCommand>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl EngineHandle {
    /// Spawn the engine thread. Returns the handle plus the receiving end
    /// of the event channel, which the UI drains (in this app, from an
    /// `iced::Subscription`).
    pub fn spawn() -> (EngineHandle, Receiver<EngineEvent>) {
        let (cmd_tx, cmd_rx) = channel::<EngineCommand>();
        let (evt_tx, evt_rx) = channel::<EngineEvent>();

        let join = std::thread::Builder::new()
            .name("offcut-engine".to_string())
            .spawn(move || run(cmd_rx, evt_tx))
            .expect("failed to spawn engine thread");

        (EngineHandle { commands: cmd_tx, join: Some(join) }, evt_rx)
    }

    /// Send a command. `Err` only when the engine thread is gone.
    pub fn send(&self, command: EngineCommand) -> Result<(), EngineError> {
        self.commands
            .send(command)
            .map_err(|_| EngineError::ProbeFailed("engine thread is not running".into()))
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(EngineCommand::Shutdown);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Engine-thread state. Kept in one struct so `run`'s loop stays readable.
struct EngineState {
    pipeline: Option<Pipeline>,
    playing: bool,
    /// Frames are only pulled while playing OR when a seek asked for one
    /// preview frame; this flag is that one-shot request.
    want_one_frame: bool,
    /// How many times the current paused preview request has come back
    /// empty. A flushing seek needs a few pull attempts before the
    /// decoder has a frame ready, so the request is retried rather than
    /// abandoned -- but only up to `MAX_PREVIEW_ATTEMPTS`, so a request
    /// with genuinely no frame behind it cannot spin this thread.
    preview_attempts: u32,
}

/// How many empty pulls a paused preview request tolerates before giving
/// up. At the 60ms pull timeout this is a little under a second, which is
/// generous for a keyframe seek in a long file and still bounded.
const MAX_PREVIEW_ATTEMPTS: u32 = 12;

fn run(commands: Receiver<EngineCommand>, events: Sender<EngineEvent>) {
    let mut state = EngineState { pipeline: None, playing: false, want_one_frame: false, preview_attempts: 0 };

    loop {
        // 1. Drain every pending command first, so a burst of scrub seeks
        //    doesn't queue up behind frame pulls.
        //
        //    Fast seeks are *coalesced*: a drag emits one per pointer
        //    move, and the pointer moves far faster than a decoder can
        //    seek. Executing all of them means the picture chases the
        //    pointer several hundred milliseconds behind, and every
        //    intermediate seek is work whose result is thrown away. Only
        //    the newest one is worth doing — it is where the pointer
        //    actually is. Accurate seeks are never coalesced away: those
        //    come one per gesture, on release, and each is a commitment.
        let mut pending_fast_seek: Option<Time> = None;
        loop {
            match commands.try_recv() {
                Ok(EngineCommand::Shutdown) => return,
                Ok(EngineCommand::SeekFast(t)) => pending_fast_seek = Some(t),
                Ok(cmd) => {
                    // A command that changes what a seek would even mean
                    // must not be reordered behind one.
                    if let Some(t) = pending_fast_seek.take()
                        && let Err(e) = handle(EngineCommand::SeekFast(t), &mut state, &events)
                    {
                        let _ = events.send(EngineEvent::Error(e.to_string()));
                    }
                    if let Err(e) = handle(cmd, &mut state, &events) {
                        let _ = events.send(EngineEvent::Error(e.to_string()));
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some(t) = pending_fast_seek
            && let Err(e) = handle(EngineCommand::SeekFast(t), &mut state, &events)
        {
            let _ = events.send(EngineEvent::Error(e.to_string()));
        }

        // 2. Pull a frame if there's a reason to.
        let should_pull = state.pipeline.is_some() && (state.playing || state.want_one_frame);
        if !should_pull {
            // Nothing to do: sleep briefly rather than spinning a core.
            // 4ms is well under a 60fps frame budget, so a Play command
            // is acted on effectively immediately.
            std::thread::sleep(std::time::Duration::from_millis(2));
            continue;
        }

        let pipeline = state.pipeline.as_ref().expect("checked is_some above");
        // `pull_current_frame`, not `pull_frame`: when we are paused and
        // servicing a one-shot `want_one_frame` (file just opened, or a
        // scrub seek landed), the only frame that exists is the preroll
        // sample. See `Pipeline::pull_current_frame`'s doc comment for the
        // three bugs that single distinction caused.
        // 60ms, not 250ms: while scrubbing, this timeout is the upper
        // bound on how long the engine sits blocked on one frame instead
        // of noticing the newer seek the user has already made. The design
        // §4.1's whole scrub budget is 50ms.
        match pipeline.pull_current_frame(gst::ClockTime::from_mseconds(60)) {
            Ok(frame) => {
                state.want_one_frame = false;
                state.preview_attempts = 0;
                let pts = frame.pts;
                if events.send(EngineEvent::Frame(Arc::new(frame))).is_err() {
                    return; // UI is gone
                }
                let _ = events.send(EngineEvent::Position(pts));
            }
            Err(EngineError::NoSample) => {
                // `NoSample` means one of two opposite things: the stream
                // is genuinely over, or this pull simply timed out before
                // the next frame was ready. Ask the appsink which.
                //
                // Getting this wrong stops playback: with a clock-synced
                // appsink a frame legitimately takes up to a frame
                // interval to arrive, so pulls *do* time out during
                // healthy playback. Treating that as EOS halted a
                // just-started file after two frames.
                let ended = pipeline.is_eos();
                if state.playing && ended {
                    state.playing = false;
                    let _ = events.send(EngineEvent::PlayingChanged(false));
                    let _ = events.send(EngineEvent::Eos);
                }
                // # A paused preview request must be retried, not dropped
                //
                // This used to clear `want_one_frame` on any paused
                // timeout, abandoning the request. But a flushing seek
                // takes time to settle: ask for the frame 60ms later and
                // it is simply not ready yet, so the *one* pull that was
                // ever going to happen found nothing and the preview
                // silently kept showing the previous frame. That is the
                // "stepping frames does not move the preview" defect —
                // the seek was issued and honoured, but nobody collected
                // the result.
                //
                // Retrying is bounded rather than open-ended: a request
                // that genuinely has no frame behind it (seek past EOS)
                // must not spin the thread forever.
                if !state.playing {
                    state.preview_attempts = state.preview_attempts.saturating_add(1);
                    if ended || state.preview_attempts >= MAX_PREVIEW_ATTEMPTS {
                        state.want_one_frame = false;
                        state.preview_attempts = 0;
                    }
                }
            }
            Err(e) => {
                state.want_one_frame = false;
                let _ = events.send(EngineEvent::Error(e.to_string()));
            }
        }
    }
}

fn handle(
    command: EngineCommand,
    state: &mut EngineState,
    events: &Sender<EngineEvent>,
) -> Result<(), EngineError> {
    match command {
        EngineCommand::Shutdown => {}

        EngineCommand::Open(path) => {
            // Probe first: this both validates the file and gives the UI
            // real metadata (duration/fps/resolution) to build its model
            // from, rather than the UI guessing.
            let info = crate::probe::probe_file(&path, gst::ClockTime::from_seconds(15))?;

            let pipeline = Pipeline::from_file(&path, info.has_audio)?;
            // Preroll so duration/seek work immediately, and so the first
            // frame is on screen before the user presses play.
            pipeline.pause()?;
            pipeline.wait_until_ready(gst::ClockTime::from_seconds(15))?;

            state.pipeline = Some(pipeline);
            state.playing = false;
            state.want_one_frame = true; // show frame 1 immediately

            let _ = events.send(EngineEvent::Opened { path, info });
            let _ = events.send(EngineEvent::PlayingChanged(false));
        }

        EngineCommand::Play => {
            if let Some(p) = &state.pipeline {
                p.play()?;
                state.playing = true;
                let _ = events.send(EngineEvent::PlayingChanged(true));
            }
        }

        EngineCommand::Pause => {
            if let Some(p) = &state.pipeline {
                p.pause()?;
                state.playing = false;
                let _ = events.send(EngineEvent::PlayingChanged(false));
            }
        }

        EngineCommand::SeekFast(t) => {
            if let Some(p) = &state.pipeline {
                p.seek_fast(t)?;
                state.preview_attempts = 0;
                // Even paused, produce one frame so scrubbing shows the
                // frame under the playhead -- the "live preview while
                // dragging" behavior the design calls the biggest
                // usability win.
                state.want_one_frame = true;
            }
        }

        EngineCommand::SeekAccurate(t) => {
            if let Some(p) = &state.pipeline {
                p.seek_accurate(t)?;
                state.preview_attempts = 0;
                state.want_one_frame = true;
            }
        }

        EngineCommand::SetRate(rate) => {
            if let Some(p) = &state.pipeline {
                p.set_rate(rate)?;
                state.want_one_frame = true;
            }
        }

        EngineCommand::SetVolume(v) => {
            if let Some(p) = &state.pipeline {
                let _ = p.set_volume(v)?;
            }
        }

        EngineCommand::Close => {
            if let Some(p) = state.pipeline.take() {
                let _ = p.stop();
            }
            state.playing = false;
            state.want_one_frame = false;
            let _ = events.send(EngineEvent::PlayingChanged(false));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn require_sample(name: &str) -> PathBuf {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../media")
            .join(name);
        assert!(
            p.exists(),
            "missing test fixture {}\nRun `offcut/tools/make-sample.sh` to generate it.",
            p.display()
        );
        p
    }

    /// Collect events until `predicate` matches one, or the deadline
    /// passes. Returns everything seen, so failures can show what
    /// actually arrived instead of just "timed out".
    fn wait_for(
        events: &Receiver<EngineEvent>,
        timeout: Duration,
        mut predicate: impl FnMut(&EngineEvent) -> bool,
    ) -> (bool, Vec<EngineEvent>) {
        let deadline = Instant::now() + timeout;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(100)) {
                Ok(evt) => {
                    let hit = predicate(&evt);
                    seen.push(evt);
                    if hit {
                        return (true, seen);
                    }
                }
                Err(_) => continue,
            }
        }
        (false, seen)
    }

    #[test]
    fn opening_a_real_file_reports_its_real_properties() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();

        let (found, seen) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Opened { .. }));
        assert!(found, "never got Opened; saw: {seen:?}");

        let opened = seen.iter().find_map(|e| match e {
            EngineEvent::Opened { info, .. } => Some(info.clone()),
            _ => None,
        });
        let info = opened.expect("Opened event carried no info");
        assert_eq!(info.resolution, (640, 360));
        assert!(info.has_audio);
    }

    /// The whole point of the engine thread: opening a file delivers real
    /// decoded frames to the UI side without the UI ever blocking.
    #[test]
    fn opening_a_file_delivers_a_first_frame_without_pressing_play() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();

        let (found, seen) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Frame(_)));
        assert!(found, "no preview frame after Open; saw: {seen:?}");

        let frame = seen.iter().find_map(|e| match e {
            EngineEvent::Frame(f) => Some(f.clone()),
            _ => None,
        });
        let frame = frame.expect("no frame");
        assert_eq!((frame.width, frame.height), (640, 360));
        assert!(frame.has_non_zero_data(), "preview frame was blank");
    }

    #[test]
    fn play_produces_a_stream_of_frames_with_advancing_positions() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();
        let (opened, _) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Opened { .. }));
        assert!(opened, "file never opened");

        engine.send(EngineCommand::Play).unwrap();

        // Gather several positions and confirm they advance.
        let mut positions: Vec<Time> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && positions.len() < 10 {
            if let Ok(EngineEvent::Position(t)) = events.recv_timeout(Duration::from_millis(200)) {
                positions.push(t);
            }
        }
        assert!(positions.len() >= 10, "only got {} positions", positions.len());
        assert!(
            positions.last().unwrap() > positions.first().unwrap(),
            "playback position never advanced: {:?} -> {:?}",
            positions.first(),
            positions.last()
        );
    }

    /// Playback must survive a pull that simply timed out.
    ///
    /// With a clock-synced appsink, frames arrive on the clock's schedule
    /// rather than as fast as they decode, so an individual pull *can*
    /// exceed its timeout during entirely healthy playback. The engine
    /// used to read that as end-of-stream and stop — this asserts that
    /// playback keeps running well past the point where the old code
    /// gave up (it managed two frames).
    #[test]
    fn a_pull_timeout_during_playback_is_not_mistaken_for_end_of_stream() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();
        let (opened, _) = wait_for(&events, Duration::from_secs(20), |e| {
            matches!(e, EngineEvent::Opened { .. })
        });
        assert!(opened, "file never opened");

        engine.send(EngineCommand::Play).unwrap();

        // The fixture is ~5s long, so a couple of seconds of playback is
        // well short of a real EOS. Any Eos seen here is spurious.
        let mut frames = 0usize;
        let mut spurious_eos = false;
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(EngineEvent::Frame(_)) => frames += 1,
                Ok(EngineEvent::Eos) => {
                    spurious_eos = true;
                    break;
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }

        assert!(!spurious_eos, "playback reported EOS {frames} frames into a 5s file");
        assert!(
            frames > 20,
            "only {frames} frames in 3s — playback stalled instead of streaming"
        );
    }

    /// Stepping frames while paused must actually deliver a **new
    /// picture**, not just move a number.
    ///
    /// The defect this pins: a paused preview request was abandoned if
    /// the pull timed out, and a flushing seek routinely needs longer
    /// than one 60ms pull to settle. So the seek was issued and honoured
    /// and nobody collected the result -- the preview kept showing the
    /// previous frame while the playhead and timecode both moved.
    #[test]
    fn a_paused_seek_delivers_a_frame_at_the_new_position() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();
        let (opened, _) = wait_for(&events, Duration::from_secs(20), |e| {
            matches!(e, EngineEvent::Opened { .. })
        });
        assert!(opened, "file never opened");

        // Drain the initial preview frame.
        let _ = wait_for(&events, Duration::from_secs(10), |e| matches!(e, EngineEvent::Frame(_)));

        // Now seek, paused, to several distinct positions. Each must
        // produce a frame whose timestamp is near where we asked.
        for target_secs in [3.0f64, 1.0, 4.0] {
            let target = Time::from_nanos((target_secs * 1e9) as u64);
            engine.send(EngineCommand::SeekAccurate(target)).unwrap();

            let (got, seen) = wait_for(&events, Duration::from_secs(10), |e| {
                matches!(e, EngineEvent::Frame(_))
            });
            assert!(got, "no frame after a paused seek to {target_secs}s; saw {seen:?}");

            let pts = seen
                .iter()
                .rev()
                .find_map(|e| match e {
                    EngineEvent::Frame(f) => Some(f.pts.as_secs_f64()),
                    _ => None,
                })
                .expect("a frame was reported found");
            assert!(
                (pts - target_secs).abs() < 0.5,
                "paused seek to {target_secs}s previewed {pts:.3}s -- the picture \
                 does not match the playhead"
            );
        }
    }

    #[test]
    fn seek_moves_the_reported_position() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();
        let (opened, _) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Opened { .. }));
        assert!(opened);

        engine.send(EngineCommand::SeekAccurate(Time::from_nanos(3_000_000_000))).unwrap();

        let (found, seen) = wait_for(&events, Duration::from_secs(20), |e| {
            matches!(e, EngineEvent::Position(t) if t.as_secs_f64() > 2.5)
        });
        assert!(found, "position never reached the sought point; saw: {seen:?}");
    }

    #[test]
    fn opening_a_nonexistent_file_reports_an_error_instead_of_dying() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(PathBuf::from("/nonexistent/nope.mp4"))).unwrap();

        let (found, seen) = wait_for(&events, Duration::from_secs(25), |e| matches!(e, EngineEvent::Error(_)));
        assert!(found, "expected an Error event; saw: {seen:?}");

        // And critically, the thread is still alive and usable afterward.
        engine
            .send(EngineCommand::Open(require_sample("sample.mp4")))
            .expect("engine thread died after an error");
        let (recovered, _) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Opened { .. }));
        assert!(recovered, "engine did not recover from a bad file");
    }

    #[test]
    fn dropping_the_handle_shuts_the_thread_down() {
        let (engine, events) = EngineHandle::spawn();
        engine.send(EngineCommand::Open(require_sample("sample.mp4"))).unwrap();
        let (opened, _) = wait_for(&events, Duration::from_secs(20), |e| matches!(e, EngineEvent::Opened { .. }));
        assert!(opened);

        drop(engine); // joins the thread

        // The event channel's sender is gone, so draining it terminates
        // rather than hanging -- proof the thread actually exited.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match events.recv_timeout(Duration::from_millis(200)) {
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    assert!(Instant::now() < deadline, "engine thread never exited");
                }
            }
        }
    }
}
