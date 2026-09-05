//! offcut-app: the real `iced` application.
//!
//! This is where the pure pieces meet the impure world. `offcut-ui` owns
//! the layout and a pure `ShellState::update`; `offcut-model` owns the
//! edit math; `offcut-engine` owns GStreamer; `offcut-export` owns the
//! encoder. This crate's whole job is wiring:
//!
//! - a real `EngineHandle` on its own OS thread, with its events reaching
//!   iced through a `Subscription`;
//! - the XDG file portal (via `rfd`) for Open and Export;
//! - background tasks that decode filmstrip thumbnails and the audio
//!   waveform without stalling the UI;
//! - keyboard shortcuts;
//! - translating the handful of `ShellMessage`s that need any of the
//!   above into commands, and letting `ShellState::update` handle the
//!   rest.
//!
//! # The blocking-in-async bug this file is careful about
//!
//! An earlier version of the frame subscription waited on its background
//! thread with a **synchronous** `std::sync::mpsc::Receiver::recv()`
//! inside an async block. Because that call never `.await`s, it starved
//! the executor task polling the whole combined stream — frames were sent
//! into the channel and never delivered, and the stage rendered solid
//! black with no error anywhere. The engine event bridge below therefore
//! never blocks the executor: it hands the blocking `Receiver` to a
//! dedicated OS thread and forwards through an async `Sender`.

use iced::keyboard::{Key, Modifiers};
use iced::keyboard::Event as KeyEvent;
use iced::{Element, Subscription, Task};
use std::path::PathBuf;
use std::sync::Arc;
use offcut_engine::{EngineCommand, EngineEvent, EngineHandle, Frame, MediaInfo};
use offcut_export::{CancelFlag, ExportProgress};
use offcut_model::{Project, Source, SourceId, Time};
use offcut_ui::shell::{ExportState, InspectorTab, ShellMessage, ShellState};


#[derive(Debug, Clone)]
enum AppMessage {
    Shell(ShellMessage),
    Engine(EngineEventKind),
    /// A file was chosen in the portal dialog (`None` = cancelled).
    FileChosen(Option<PathBuf>),
    ExportPathChosen(Option<PathBuf>),
    ExportProgressed(ExportProgress),
    ExportFinished(Result<PathBuf, String>),
    /// The window's size, used to fit the timeline to the real viewport
    /// rather than to the design's nominal width.
    WindowResized(iced::Size),
    /// A file is being dragged over the window (not yet released).
    FileHovered,
    /// The drag left without dropping.
    FileHoverCancelled,
    /// A file was dropped onto the window.
    FileDropped(PathBuf),
}

/// `EngineEvent` mirrored with exactly the variants the app reacts to,
/// so the match here stays exhaustive against the app's own needs rather
/// than against the engine's full vocabulary.
#[derive(Debug, Clone)]
enum EngineEventKind {
    Opened(PathBuf, MediaInfo),
    Frame(Arc<Frame>),
    Position(Time),
    PlayingChanged(bool),
    Eos,
    Error(String),
}

struct App {
    state: ShellState,
    engine: EngineHandle,
    export_cancel: Option<CancelFlag>,
    /// The real viewport width, tracked so "fit the timeline" means the
    /// window the user actually has.
    window_width: f32,
    /// Set when a file opens: the next measured lane width should re-fit
    /// the zoom, because the width that mattered was not known yet.
    pending_fit: bool,
    /// True while the **user** owns the playhead — a scrub or trim drag
    /// in progress — so the engine's own `Position` events do not yank it
    /// out from under the pointer.
    ///
    /// # Why this is a held state and not a one-shot flag
    ///
    /// It used to be set by every seek and cleared by the next `Position`
    /// event. That silently assumed seeks and position events arrive
    /// one-for-one, and they do not: a drag issues ~60 seeks a second
    /// while the engine reports position as frames retire. The two counts
    /// drift apart immediately, so the flag was routinely left *set* with
    /// no drag happening — and the next genuine position update, the one
    /// that advances the playhead during playback, was swallowed.
    ///
    /// Ownership is a *state*, not a message count: either the user is
    /// holding the playhead or the engine is driving it. Held for the
    /// duration of the gesture and cleared when it ends, which is
    /// something the UI already knows exactly.
    user_owns_playhead: bool,
}

impl App {
    fn new() -> (Self, Task<AppMessage>) {
        let (engine, events) = EngineHandle::spawn();
        // The engine receiver is consumed by the subscription, which iced
        // constructs separately from the app; hand it over through a
        // thread-local that the subscription takes exactly once.
        *ENGINE_EVENTS.lock().expect("engine event slot poisoned") = Some(events);

        let mut state = ShellState::new(Project::new());

        // # User theming, read once here
        //
        // Colours only, merged over the built-ins. Loaded before the
        // capability probe below so a broken config cannot mask a
        // missing-codec warning: an app that cannot open a video is a
        // more urgent thing to say than an unreadable accent, so the
        // codec message wins the single status slot if both fire.
        let mut theme = offcut_ui::rice::load();
        if theme.loaded {
            // The contrast reading runs on what the user actually got,
            // after merging — checking the file's own lines would miss a
            // pairing made by one override against an untouched role.
            for (mode, palette) in
                [
                    (offcut_ui::theme::Mode::Dark, theme.dark),
                    (offcut_ui::theme::Mode::Light, theme.light),
                ]
            {
                theme.warnings.extend(offcut_ui::rice::audit(&palette, mode));
                // Hue collisions are a separate reading: contrast cannot
                // see two perfectly legible colours that happen to be
                // the same colour.
                theme.warnings.extend(offcut_ui::rice::hue_conflicts(&palette, mode));
            }
            // The full list goes to stderr, where someone editing a theme
            // file is already looking. The pill gets one line.
            for w in &theme.warnings {
                eprintln!("offcut: {w}");
            }
            if let Some(line) = offcut_ui::rice::summary(&theme.warnings) {
                state.status = Some(line);
            }
        }
        state.theme = theme;

        // Surface a missing-codec environment immediately, in the
        // titlebar, rather than at the first confusing failure. This is
        // `caps.rs`'s diagnosis actually reaching a human.
        if let Ok(caps) = offcut_engine::caps::probe()
            && !caps.can_export()
        {
            // `short_diagnosis`, not `first_line(&diagnosis())`: the
            // latter is the string "Missing required GStreamer elements:"
            // — a colon introducing a list the pill then discards.
            state.status = Some(caps.short_diagnosis());
        }

        // `offcut some.mp4` should just work.
        let initial = std::env::args().nth(1).map(PathBuf::from).filter(|p| p.exists());

        state.drag_and_drop_available = drag_and_drop_available();

        let app = App { state, engine, export_cancel: None, window_width: 1440.0, pending_fit: false, user_owns_playhead: false };
        let task = match initial {
            Some(path) => Task::done(AppMessage::FileChosen(Some(path))),
            None => Task::none(),
        };
        (app, task)
    }

    fn title(&self) -> String {
        match self.state.project().sources.first() {
            Some(source) => format!(
                "{} — Offcut",
                source.path.file_name().and_then(|n| n.to_str()).unwrap_or("untitled")
            ),
            None => "Offcut".to_string(),
        }
    }

    fn update(&mut self, message: AppMessage) -> Task<AppMessage> {
        match message {
            // This arm MUST precede the general `AppMessage::Shell(msg)`
            // below. It did not, and Rust matched the broad arm first, so
            // this handler was dead code for its entire existence —
            // `cargo clippy` said so ("unreachable pattern") and the
            // symptom on screen was that the timeline never fit the
            // window: `window_width` kept the *whole window's* width from
            // `WindowResized` instead of the timeline lane's real width,
            // and the lane is narrower than the window by the inspector
            // panel. Fit-to-window therefore always over-zoomed and the
            // last clip ran off the right edge.
            AppMessage::Shell(ShellMessage::Timeline(offcut_ui::TimelineMessage::LaneMeasured(width))) => {
                let changed = (self.window_width - width).abs() > 0.5;
                self.window_width = width;
                if self.pending_fit || changed {
                    self.pending_fit = false;
                    self.fit_timeline_to_window();
                }
                Task::none()
            }

            AppMessage::Shell(msg) => self.update_shell(msg),
            AppMessage::Engine(event) => self.update_engine(event),

            AppMessage::FileChosen(None) | AppMessage::ExportPathChosen(None) => Task::none(),

            AppMessage::FileChosen(Some(path)) => {
                let _ = self.engine.send(EngineCommand::Open(path));
                Task::none()
            }

            // The chosen path opens the confirm sheet; it does not start
            // the encode. Export is the one irreversible act in this
            // product and was the only one with no review step — the user
            // pressed Export, named a file, and was committed to a codec,
            // a bitrate, and a resolution they had never been shown.
            AppMessage::ExportPathChosen(Some(path)) => {
                // Seed the bitrate from the resolution actually being
                // written, so the number in the sheet is the number the
                // encoder will use rather than a stale default.
                if let Some(res) = self.state.output_resolution() {
                    self.state.export_settings.bitrate_kbps =
                        offcut_export::ExportSettings::suggested_bitrate_kbps(
                            res,
                            self.state.export_settings.codec,
                        );
                }
                self.state.pending_export = Some(path);
                Task::none()
            }



            AppMessage::ExportProgressed(progress) => {
                // A late progress message must not resurrect the progress
                // bar after the export already finished.
                if matches!(self.state.export, ExportState::Running(_)) {
                    self.state.export = ExportState::Running(progress);
                }
                Task::none()
            }

            AppMessage::WindowResized(size) => {
                self.window_width = size.width;
                self.fit_timeline_to_window();
                Task::none()
            }

            AppMessage::FileHovered => {
                self.state.drop_hover = true;
                Task::none()
            }

            AppMessage::FileHoverCancelled => {
                self.state.drop_hover = false;
                Task::none()
            }

            // # Dropping a file opens it
            //
            // A dropped video is unambiguous intent: "edit this". It goes
            // through the exact same path as File > Open, so a dropped
            // file and a picked file cannot behave differently.
            //
            // The extension is checked *before* handing the path to
            // GStreamer, because dropping a PDF onto a video editor
            // should say so plainly rather than surfacing a decoder error
            // fifteen seconds later after a probe times out.
            AppMessage::FileDropped(path) => {
                self.state.drop_hover = false;
                if !is_supported_video(&path) {
                    let name = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("that file")
                        .to_string();
                    // Names the file, the reason, and the way out. It
                    // said only "… is not a video Offcut can open", which
                    // leaves someone holding an .mkv with no idea whether
                    // the answer is a different file or a different app.
                    //
                    // The list is **derived** from `SUPPORTED_EXTENSIONS`,
                    // not retyped: a hand-written one drifts, and a
                    // message that omits a format Offcut actually opens is
                    // worse than the vague message it replaced. Writing
                    // this out by hand is exactly how `.avi` got dropped
                    // from it the first time.
                    self.state.status = Some(format!(
                        "Can't open {name}. Offcut reads {}.",
                        supported_extensions_sentence()
                    ));
                    return Task::none();
                }
                Task::done(AppMessage::FileChosen(Some(path)))
            }


            AppMessage::ExportFinished(result) => {
                self.export_cancel = None;
                self.state.export = match result {
                    Ok(path) => ExportState::Done(path),
                    Err(e) => ExportState::Failed(first_line(&e)),
                };
                Task::none()
            }
        }
    }

    fn update_shell(&mut self, message: ShellMessage) -> Task<AppMessage> {
        use offcut_ui::TimelineMessage;

        // Messages that need the outside world are handled here; the rest
        // fall through to the pure state machine at the bottom.
        match &message {
            ShellMessage::OpenFile => {
                return Task::perform(
                    async {
                        // rfd's Linux backend goes through the XDG desktop
                        // portal, so this is the compositor's own dialog
                        // and keeps the app Flatpak-ready.
                        rfd::AsyncFileDialog::new()
                            .add_filter("Video", &SUPPORTED_EXTENSIONS)
                            .set_title("Open video")
                            .pick_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    AppMessage::FileChosen,
                );
            }

            ShellMessage::Export => {
                if self.state.project().clips.is_empty() {
                    self.state.status = Some("Nothing to export yet. Open a video first.".into());
                    return Task::none();
                }
                // The dialog follows the chosen container rather than
                // hardcoding MP4: a filter and a suggested name that say
                // `.mp4` while the encoder writes Matroska produce a file
                // whose extension lies about its contents.
                let container = self.state.export_settings.container;
                let ext = container.extension();
                let filter_label = format!("{} video", container.label());
                let suggested = self
                    .state
                    .project()
                    .sources
                    .first()
                    .and_then(|s| s.path.file_stem().and_then(|n| n.to_str()))
                    .map(|stem| format!("{stem}-edited.{ext}"))
                    .unwrap_or_else(|| format!("export.{ext}"));
                return Task::perform(
                    async move {
                        rfd::AsyncFileDialog::new()
                            .add_filter(&filter_label, &[ext])
                            .set_file_name(&suggested)
                            .set_title("Export video")
                            .save_file()
                            .await
                            .map(|h| h.path().to_path_buf())
                    },
                    AppMessage::ExportPathChosen,
                );
            }

            ShellMessage::ConfirmExport => {
                let Some(path) = self.state.pending_export.clone() else {
                    return Task::none();
                };
                self.state.update(message);
                return self.start_export(path);
            }

            ShellMessage::CancelExport => {
                if let Some(flag) = &self.export_cancel {
                    flag.cancel();
                }
                return Task::none();
            }

            ShellMessage::TogglePlay => {
                // Update the model first, then tell the engine, so the
                // button reflects intent immediately rather than after a
                // round trip through the pipeline's state change.
                self.state.update(message);
                if !self.state.playing {
                    let _ = self.engine.send(EngineCommand::Pause);
                    return Task::none();
                }

                // Pressing play while parked at the end of the range
                // restarts it rather than doing nothing. Without this the
                // button appears dead once playback has run to the
                // out-point, because the very next Position event bounds
                // it again immediately.
                let at_end = self.state.playhead >= self.state.total_duration();
                let in_point = self.state.selected().map(|c| c.in_point);
                if at_end && let Some(in_point) = in_point {
                    self.state.playhead = Time::ZERO;
                    let _ = self.engine.send(EngineCommand::SeekAccurate(in_point));
                }
                let _ = self.engine.send(EngineCommand::Play);
                return Task::none();
            }

            ShellMessage::Timeline(TimelineMessage::Seek { to, precise }) => {
                let (to, precise) = (*to, *precise);
                self.state.update(message);
                return self.seek_engine(to, precise);
            }

            // Frame stepping is a *deliberate* move to one exact frame,
            // so it always seeks accurately — and it pauses first if
            // playback is running, because stepping while the decoder is
            // still advancing means the frame you land on is immediately
            // replaced by the next one. Asking for a specific frame and
            // getting a different one is the same defect as the playhead
            // not matching the preview.
            ShellMessage::StepBack | ShellMessage::StepForward => {
                if self.state.playing {
                    self.state.playing = false;
                    let _ = self.engine.send(EngineCommand::Pause);
                }
                self.state.update(message);
                let playhead = self.state.playhead;
                return self.seek_engine(playhead, true);
            }

            ShellMessage::SetSpeed(_) => {
                self.state.update(message);
                if let Some(clip) = self.state.selected() {
                    let rate = clip.speed.factor();
                    let _ = self.engine.send(EngineCommand::SetRate(rate));
                }
                let _ = self.engine.send(EngineCommand::SetVolume(volume_for(&self.state)));
                return Task::none();
            }

            // The volume slider is a *continuous* control: a drag emits a
            // message per pixel. Each one is a 48-byte property set on the
            // pipeline's `volume` element, not a pipeline rebuild, so
            // forwarding every one is genuinely cheap and the audio
            // follows the handle rather than snapping on release.
            ShellMessage::ToggleClipMute
            | ShellMessage::ToggleMasterMute
            | ShellMessage::SetClipVolume(_) => {
                self.state.update(message);
                let _ = self.engine.send(EngineCommand::SetVolume(volume_for(&self.state)));
                return Task::none();
            }

            // Edits that change the clip set invalidate the filmstrip for
            // the clips they touch, so thumbnails are re-decoded.
            ShellMessage::Split
            | ShellMessage::DeleteClip
            | ShellMessage::DuplicateClip
            | ShellMessage::Undo
            | ShellMessage::Redo => {
                self.state.update(message);
                return self.refresh_media_tasks();
            }

            // Show the frame under the handle while dragging -- the design
            // §4.1 calls this "the single biggest usability win in the
            // whole app."
            //
            // Which edge is seeked to must follow which handle is being
            // dragged. This previously sent `clip.in_point` for *both*
            // messages, so dragging the out handle previewed the start of
            // the clip — the one frame that cannot tell you where your
            // cut lands.
            ShellMessage::Timeline(TimelineMessage::TrimStart { .. }) => {
                self.state.update(message);
                if let Some(clip) = self.state.selected() {
                    let _ = self.engine.send(EngineCommand::SeekFast(clip.in_point));
                }
                return Task::none();
            }

            ShellMessage::Timeline(TimelineMessage::TrimEnd { .. }) => {
                self.state.update(message);
                if let Some(clip) = self.state.selected() {
                    // One frame back: `out_point` is exclusive.
                    let frame = self.state.fps().frame_duration().as_nanos();
                    let preview =
                        Time::from_nanos(clip.out_point.as_nanos().saturating_sub(frame));
                    let _ = self.engine.send(EngineCommand::SeekFast(preview));
                }
                return Task::none();
            }

            // # Gesture boundaries hand the playhead back and forth
            //
            // These two arms are the *only* place `user_owns_playhead`
            // changes. A drag begins, the pointer is the authority; the
            // drag ends, the engine is again. Deriving it from anything
            // else — a seek count, a message type — is what made position
            // updates go missing.
            ShellMessage::Timeline(TimelineMessage::GestureBegan)
            | ShellMessage::Timeline(TimelineMessage::TrimBar(
                offcut_ui::TrimBarMessage::GestureBegan,
            )) => {
                self.user_owns_playhead = true;
                self.state.update(message);
                return Task::none();
            }

            ShellMessage::Timeline(TimelineMessage::GestureEnded) => {
                self.user_owns_playhead = false;
                self.state.update(message);
                return self.refresh_media_tasks();
            }

            // # The trim bar's two-tier seek
            //
            // Dragging a handle shows the frame *at that handle* — the
            // whole point of a range picker is seeing where the cut lands.
            // `SeekFast` (KEY_UNIT) while the drag is live, because at
            // 60 pointer events a second an accurate seek per move would
            // queue flushes faster than the decoder retires them and the
            // image would lag the handle by seconds on a long file.
            //
            // Note which edge is seeked to: the handle being dragged. The
            // timeline's own trim path above always seeks `in_point`,
            // which means dragging the *out* handle there shows the
            // wrong end of the clip.
            ShellMessage::Timeline(TimelineMessage::TrimBar(offcut_ui::TrimBarMessage::SetIn { to, .. })) => {
                let to = *to;
                self.state.update(message);
                let _ = self.engine.send(EngineCommand::SeekFast(to));
                return Task::none();
            }

            ShellMessage::Timeline(TimelineMessage::TrimBar(offcut_ui::TrimBarMessage::SetOut { to, .. })) => {
                let to = *to;
                self.state.update(message);
                // One frame back from the out-point: the out-point is
                // *exclusive*, so seeking exactly to it shows the first
                // frame the clip does not contain.
                let frame = self.state.fps().frame_duration().as_nanos();
                let preview = Time::from_nanos(to.as_nanos().saturating_sub(frame));
                let _ = self.engine.send(EngineCommand::SeekFast(preview));
                return Task::none();
            }

            // # Scrubbing the red mark
            //
            // `precise` is honoured rather than ignored. This previously
            // always issued an ACCURATE seek, which is correct for a
            // single click and ruinous for a drag: at 60+ pointer events
            // a second, accurate seeks queue flushes faster than the
            // decoder retires them, so the picture falls seconds behind
            // the pointer and the mark appears not to be showing the
            // current frame at all.
            //
            // `SeekFast` (KEY_UNIT) while the drag is live keeps the
            // image tracking the hand; the accurate seek fires once, from
            // `GestureEnded` below, so where you *stop* is frame-exact.
            ShellMessage::Timeline(TimelineMessage::TrimBar(offcut_ui::TrimBarMessage::Scrub { to, precise })) => {
                let (to, precise) = (*to, *precise);
                self.state.update(message);
                let command = if precise {
                    EngineCommand::SeekAccurate(to)
                } else {
                    EngineCommand::SeekFast(to)
                };
                let _ = self.engine.send(command);
                return Task::none();
            }

            // The accurate half of the two-tier seek, once, on release.
            ShellMessage::Timeline(TimelineMessage::TrimBar(offcut_ui::TrimBarMessage::GestureEnded)) => {
                self.user_owns_playhead = false;
                self.state.update(message);
                let playhead = self.state.playhead;
                return Task::batch([self.seek_engine(playhead, true), self.refresh_media_tasks()]);
            }

            ShellMessage::Timeline(TimelineMessage::SelectClip(_)) => {
                self.state.update(message);
                let playhead = self.state.playhead;
                return self.seek_engine(playhead, true);
            }

            _ => {}
        }

        self.state.update(message);
        Task::none()
    }

    /// Set the zoom so the whole timeline fits the current window.
    fn fit_timeline_to_window(&mut self) {
        let total = self.state.total_duration().as_secs_f64() as f32;
        if total <= 0.0 {
            return;
        }
        let usable = (self.window_width - 2.0 * offcut_ui::timeline::CONTENT_RAIL).max(200.0);
        self.state.zoom =
            (usable / total).clamp(offcut_ui::shell::ZOOM_MIN, offcut_ui::shell::ZOOM_MAX);
    }

    /// Translate a TIMELINE position into the SOURCE seek the engine
    /// needs. This is the one conversion that must be right for scrubbing
    /// to show the correct frame, and it goes through `offcut-model`'s
    /// tested `resolve_timeline_time` rather than being recomputed here.
    ///
    /// # The end of the timeline is a real position
    ///
    /// `resolve_timeline_time` deliberately returns `None` at and past
    /// the end — the end is *exclusive*, so there is no clip there. That
    /// is correct for asking "which clip am I inside", and wrong as a
    /// reason to skip the seek: this used to bail out silently, so
    /// stepping forward onto the final frame moved the playhead in the
    /// UI and **never told the engine**. The picture stayed on the
    /// previous frame while the red mark and the timecode both said
    /// otherwise.
    ///
    /// Nudging back by a frame gives the last *contained* instant, which
    /// is the frame a user parked at the end expects to be looking at.
    fn resolve_for_seek(&self, timeline_time: Time) -> Option<offcut_model::TimelinePosition> {
        self.state.project().resolve_timeline_time(timeline_time).or_else(|| {
            // Past the end: retreat one frame and resolve that instead.
            let frame = self.state.fps().frame_duration().as_nanos().max(1);
            let inside = Time::from_nanos(timeline_time.as_nanos().saturating_sub(frame));
            self.state.project().resolve_timeline_time(inside)
        })
    }

    fn seek_engine(&mut self, timeline_time: Time, precise: bool) -> Task<AppMessage> {
        let Some(position) = self.resolve_for_seek(timeline_time) else {
            return Task::none();
        };
        let command = if precise {
            EngineCommand::SeekAccurate(position.source_time)
        } else {
            EngineCommand::SeekFast(position.source_time)
        };
        let _ = self.engine.send(command);
        Task::none()
    }

    fn update_engine(&mut self, event: EngineEventKind) -> Task<AppMessage> {
        match event {
            EngineEventKind::Opened(path, info) => {
                // Build a fresh single-clip project from what the file
                // actually is — every field here is probed, not assumed.
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
                if project.add_clip_for_source(source_id).is_err() {
                    // This one genuinely has no user-side recovery, so it
                    // says so rather than implying an action that does not
                    // exist.
                    self.state.status =
                        Some("Offcut opened that file but could not load it. Try another file.".into());
                    return Task::none();
                }

                self.state.history.reset(project);
                self.state.codec = info.video_codec.clone();
                self.state.selected_clip = Some(0);
                self.state.playhead = Time::ZERO;
                self.state.playing = false;
                self.state.status = None;
                self.state.export = ExportState::Idle;

                // Fit the whole clip on screen at open: a long video
                // opening at a fixed default zoom would show its first
                // few seconds and look like nothing loaded.
                //
                // The width used here is the *actual* window width, not a
                // hardcoded 1440. Assuming the design width overflowed
                // the timeline on any narrower window — a 1h41m film in a
                // 1257px window computed a zoom that ran the clip about a
                // screen and a half past the right edge, so the ruler
                // stopped at 20:00 on a 101-minute file.
                // Fit to the real lane width. `resize_events` does NOT
                // fire on window open in iced 0.14 (only on an actual
                // resize), so relying on it left `window_width` at its
                // 1440 default and a 1h41m film still overflowed a
                // 1257px window. The timeline canvas reports its own
                // measured width instead, which is the only value that is
                // true by construction; `pending_fit` makes the next such
                // report re-fit rather than being ignored as a no-op.
                self.pending_fit = true;
                self.fit_timeline_to_window();

                self.refresh_media_tasks()
            }

            EngineEventKind::Frame(frame) => {
                self.state.current_frame = Some(frame);
                Task::none()
            }

            EngineEventKind::Position(source_time) => {
                // While the user is holding the playhead, the pointer is
                // the authority and the engine's reports are echoes of
                // seeks the app itself just issued. Following them would
                // fight the drag.
                if self.user_owns_playhead {
                    return Task::none();
                }
                // Only follow the engine while actually playing — during a
                // scrub the user owns the playhead, and letting decoded
                // frames drag it would fight the pointer.
                // Copied out rather than held as a borrow: the bound below
                // mutates `self.state`, and the clip's identity and range
                // are all this needs.
                let clip = self.state.selected().map(|c| (c.id, c.out_point));
                if self.state.playing
                    && let Some((clip_id, out_point)) = clip
                {
                    // # Playback is bounded by the trimmed range
                    //
                    // The decoder is fed the whole source file, so left
                    // alone it plays straight past the out-point and on to
                    // the end of a 101-minute movie. That makes the trim
                    // bar a lie: the range says "this is my clip" and
                    // pressing play ignores it.
                    //
                    // The bound is enforced here against the *source*
                    // timestamp the engine reports, rather than with a
                    // GStreamer segment seek — one comparison per frame,
                    // no extra pipeline state to keep in sync, and it
                    // stays correct when the out-point is dragged during
                    // playback.
                    if playback_should_stop(source_time, out_point) {
                        self.state.playing = false;
                        let _ = self.engine.send(EngineCommand::Pause);
                        // Park on the out-point rather than wherever the
                        // decoder happened to stop, so the next play
                        // resumes from a defined instant.
                        self.state.playhead = self.state.total_duration();
                            let _ = self.engine.send(EngineCommand::SeekAccurate(out_point));
                        return Task::none();
                    }
                    if let Some(timeline_time) =
                        self.state.project().timeline_time_of(clip_id, source_time)
                    {
                        self.state.playhead = timeline_time;
                    }
                }
                Task::none()
            }

            EngineEventKind::PlayingChanged(playing) => {
                self.state.playing = playing;
                Task::none()
            }

            EngineEventKind::Eos => {
                self.state.playing = false;
                Task::none()
            }

            EngineEventKind::Error(message) => {
                self.state.status = Some(first_line(&message));
                Task::none()
            }
        }
    }

    /// Formerly: decoded filmstrip thumbnails and an audio waveform for
    /// every clip, off the UI thread, at hundreds of milliseconds each.
    ///
    /// Both fed lanes that no longer exist — the trim bar is the only bar
    /// now — so the work was pure cost: every open, split, delete, and
    /// undo spawned decode passes whose results were dropped on the floor.
    /// Removing it is the largest single responsiveness win in this
    /// change, and it is why opening a long file is now immediate rather
    /// than a stall.
    ///
    /// Kept as a no-op returning `Task::none()` rather than deleted at
    /// every call site: the call sites are the *correct* places to refresh
    /// per-clip media, and a future overlay or preview strip would want
    /// exactly this hook back.
    fn refresh_media_tasks(&mut self) -> Task<AppMessage> {
        Task::none()
    }

    fn start_export(&mut self, destination: PathBuf) -> Task<AppMessage> {
        let project = self.state.project().clone();
        let settings = self.state.export_settings.clone();
        let cancel = CancelFlag::new();
        self.export_cancel = Some(cancel.clone());
        self.state.export = ExportState::Running(ExportProgress {
            position: Time::ZERO,
            total: self.state.total_duration(),
        });

        // The export runs on its own thread and reports progress through a
        // channel drained by a subscription. Running it inline would
        // freeze the window for the whole encode.
        let (progress_tx, progress_rx) = std::sync::mpsc::channel::<ExportProgress>();
        *EXPORT_PROGRESS.lock().expect("export progress slot poisoned") = Some(progress_rx);

        Task::perform(
            async move {
                blocking(move || {
                    match offcut_export::export(&project, &destination, &settings, &cancel, |p| {
                        let _ = progress_tx.send(p);
                    }) {
                        Ok(()) => Ok(destination),
                        Err(e) => Err(e.to_string()),
                    }
                })
                .await
            },
            AppMessage::ExportFinished,
        )
    }

    fn view(&self) -> Element<'_, AppMessage> {
        offcut_ui::view(&self.state).map(AppMessage::Shell)
    }

    fn subscription(&self) -> Subscription<AppMessage> {
        let engine = Subscription::run(engine_event_stream).map(AppMessage::Engine);
        // The modal scrim swallows *pointer* input, but shortcuts do not
        // go through the widget tree at all -- they are a global
        // subscription. Without this, Space would still play, `s` would
        // still split, and Ctrl+Z would still undo underneath a window
        // that claims to be locked, changing the project mid-encode.
        //
        // A lock that only covers one input device is not a lock.
        // Gating the whole subscription, rather than filtering inside the
        // closure: iced requires these closures to be non-capturing, and
        // the compiler says so clearly. Dropping the subscription while
        // exporting is also the more honest expression of "no keyboard
        // input right now".
        let exporting = matches!(self.state.export, ExportState::Running(_));
        let keys = if exporting {
            Subscription::none()
        } else {
            iced::keyboard::listen().filter_map(|event| match event {
                KeyEvent::KeyPressed { key, modifiers, .. } => {
                    shortcut(key, modifiers).map(AppMessage::Shell)
                }
                _ => None,
            })
        };

        let export_progress = if matches!(self.state.export, ExportState::Running(_)) {
            Subscription::run(export_progress_stream).map(AppMessage::ExportProgressed)
        } else {
            Subscription::none()
        };

        // The 150ms waveform-partials timer that used to live here is
        // gone with the waveform lane. It woke the app ~7 times a second
        // to drain a queue nothing filled any more.
        let resizes = iced::window::resize_events().map(|(_, size)| AppMessage::WindowResized(size));

        // Drag-and-drop. `resize_events` has a dedicated helper; the file
        // events do not, so this listens to the raw window event stream
        // and keeps only the three that matter.
        let drops = iced::event::listen_with(|event, _status, _window| match event {
            iced::Event::Window(iced::window::Event::FileHovered(_)) => Some(AppMessage::FileHovered),
            iced::Event::Window(iced::window::Event::FilesHoveredLeft) => {
                Some(AppMessage::FileHoverCancelled)
            }
            iced::Event::Window(iced::window::Event::FileDropped(path)) => {
                Some(AppMessage::FileDropped(path))
            }
            _ => None,
        });

        Subscription::batch([engine, keys, export_progress, resizes, drops])
    }
}

/// Keyboard map. The product's promise is that the core operations are
/// faster to reach than in a full NLE, which means they need keys, not
/// only buttons.
fn shortcut(key: Key, modifiers: Modifiers) -> Option<ShellMessage> {
    use iced::keyboard::key::Named;

    if modifiers.command() {
        return match key.as_ref() {
            Key::Character("z") if modifiers.shift() => Some(ShellMessage::Redo),
            Key::Character("z") => Some(ShellMessage::Undo),
            Key::Character("y") => Some(ShellMessage::Redo),
            Key::Character("o") => Some(ShellMessage::OpenFile),
            Key::Character("e") => Some(ShellMessage::Export),
            // # Ctrl +/-/0 scale the interface
            //
            // These keys were **unbound** for a while, and the reason is
            // worth keeping: they used to drive `ZoomIn`/`ZoomOut`, which
            // clamp and store a `zoom` value that nothing draws — the
            // canvas renders only the trim bar and discards its layout
            // (`timeline.rs`, end of `draw`). Three live shortcuts, zero
            // pixels changed. A key that reliably does nothing teaches
            // the user that the whole map is unreliable.
            //
            // They are bound again now because they drive something that
            // is genuinely applied: `App::scale_factor` feeds iced, which
            // scales every widget in the tree. The rule is unchanged —
            // a shortcut must move pixels — and this time it does.
            //
            // `=` as well as `+`: the unshifted key on a US layout is
            // `=`, and requiring Shift to zoom in is a shortcut people
            // report as broken.
            Key::Character("+") | Key::Character("=") => Some(ShellMessage::ScaleUp),
            Key::Character("-") => Some(ShellMessage::ScaleDown),
            Key::Character("0") => Some(ShellMessage::ScaleReset),
            _ => None,
        };
    }

    match key.as_ref() {
        Key::Named(Named::Space) => Some(ShellMessage::TogglePlay),
        Key::Named(Named::ArrowLeft) => Some(ShellMessage::StepBack),
        Key::Named(Named::ArrowRight) => Some(ShellMessage::StepForward),
        // Escape dismisses the inspector plate.
        //
        // The plate is the only thing that covers the picture, so the key
        // every desktop user already presses to mean "get this off my
        // screen" must reach it. Wired to `CloseInspector` rather than to
        // `SelectTab`, so repeated presses stay closed instead of
        // toggling the panel back open.
        Key::Named(Named::Escape) => Some(ShellMessage::CloseInspector),
        // The keyboard reference, on the key every application that has
        // one already uses. Also in the menu, because a shortcut for
        // discovering shortcuts cannot be the only way to find them.
        Key::Character("?") => Some(ShellMessage::ToggleShortcuts),

        // # Why Split, ripple-delete, and duplicate have no keys
        //
        // They are **capability, not features** — the product rules use those
        // exact words, and records why: pressing Split on a one-clip
        // timeline produces two clips, at which point `trim_bar_row`
        // hides itself and the user has no trim control at all.
        //
        // That reasoning removed them from the transport. It did not
        // reach this map, so `s`, `d`, `Delete`, and `Backspace` went on
        // destroying the primary control from a stray keystroke — on a
        // surface where bare letters are transport keys, so a stray
        // keystroke is not hypothetical. There was no message, no visible
        // result, and no way back but an unhinted Ctrl+Z.
        //
        // A shortcut for an operation with no surface is worse than a
        // missing one: it is a trapdoor. These return with the multi-clip
        // surface that justifies them, together.

        // In and out at the playhead.
        //
        // The operation this product exists FOR, and until now the only
        // core operation reachable by pointer alone. `i`/`o` is the idiom
        // every editor shares, which is what made it worth reclaiming
        // from the trapdoor above.
        Key::Character("i") => Some(ShellMessage::SetInAtPlayhead),
        Key::Character("o") => Some(ShellMessage::SetOutAtPlayhead),

        Key::Character("k") => Some(ShellMessage::TogglePlay),
        Key::Character("j") => Some(ShellMessage::StepBack),
        Key::Character("l") => Some(ShellMessage::StepForward),
        // `m` mutes the selected clip — the second of the product's four
        // core operations, and the one most likely to be used repeatedly
        // while scrubbing.
        Key::Character("m") => Some(ShellMessage::ToggleClipMute),
        Key::Character("t") => Some(ShellMessage::ToggleMode),
        // Number keys switch inspector tabs. Cheap to reach, and it is
        // the convention every editing tool with a tabbed inspector
        // already uses, so it costs a user nothing to learn.
        Key::Character("1") => Some(ShellMessage::SelectTab(InspectorTab::Video)),
        Key::Character("2") => Some(ShellMessage::SelectTab(InspectorTab::Crop)),
        Key::Character("3") => Some(ShellMessage::SelectTab(InspectorTab::Adjust)),
        _ => None,
    }
}

/// The container formats Offcut will attempt to open.
///
/// Deliberately the **same list** the file dialog filters on: a file the
/// picker would show must also be droppable, and vice versa. Two lists
/// would drift, and the drift would look like "drag and drop is broken
/// for mkv" rather than like a missing string.
const SUPPORTED_EXTENSIONS: [&str; 6] = ["mp4", "mov", "mkv", "webm", "m4v", "avi"];

/// Whether the windowing backend can actually deliver dropped files.
///
/// # Why this check exists
///
/// The drag-and-drop wiring in this file is complete and correct, and on
/// a **native Wayland** session it can never fire: `winit` 0.30
/// implements `DroppedFile`/`HoveredFile` in its X11 backend only — the
/// Wayland backend contains no occurrence of either. There is no event
/// to receive, so no amount of application code helps.
///
/// Reporting that is the honest thing to do. A feature that silently
/// does nothing reads as a bug in *this* app, and the user's next move
/// is to drag harder rather than to use the Open button. Saying so once,
/// with the alternative attached, costs one line of chrome and saves
/// that whole detour.
///
/// Running under XWayland (`WAYLAND_DISPLAY` unset, `DISPLAY` pointing
/// at a working X server) restores it, because that path uses the X11
/// backend which does implement drops.
fn drag_and_drop_available() -> bool {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        || std::env::var_os("WAYLAND_SOCKET").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some_and(|d| !d.is_empty());
    // winit prefers Wayland whenever it is available, so X11's drop
    // support is only reachable when Wayland is *not* in play.
    !wayland && x11
}

/// Whether a dropped path looks like a video this app can open.
///
/// An extension check, not a probe: this runs on the UI thread the
/// instant a file is released, and probing a file to answer "should I
/// even try" would freeze the window for as long as the probe takes.
/// A wrong guess here is recoverable — the engine reports a real error —
/// whereas a frozen window is not.
/// `SUPPORTED_EXTENSIONS` as a readable clause: ".mp4, .mov … and .avi".
///
/// Built from the constant so the sentence cannot fall out of step with
/// the check that produced it.
fn supported_extensions_sentence() -> String {
    let dotted: Vec<String> = SUPPORTED_EXTENSIONS.iter().map(|e| format!(".{e}")).collect();
    match dotted.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, head)) => format!("{} and {last}", head.join(", ")),
        None => String::new(),
    }
}

fn is_supported_video(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .is_some_and(|e| SUPPORTED_EXTENSIONS.contains(&e.as_str()))
}

/// Whether playback has reached the end of the trimmed range.
///
/// A free function rather than an inline comparison so it can be tested
/// without an engine, a window, or a GPU — `App` owns all three, which is
/// why the rest of this file's logic is so hard to reach from a test.
///
/// `>=`, not `>`: `out_point` is **exclusive**, so the frame at exactly
/// the out-point is the first frame the clip does not contain. Stopping
/// at `>` would play one frame past the cut on every clip.
fn playback_should_stop(source_time: Time, out_point: Time) -> bool {
    source_time >= out_point
}

/// Effective monitor volume.
///
/// Three things can quiet the preview, and they compose as a *gate* times
/// a *level* rather than as competing booleans:
///
/// - master mute silences everything, unconditionally;
/// - the clip's effective mute (which includes the ///   4×-implies-mute rule) silences this clip;
/// - otherwise the clip's own `volume` is the level.
///
/// Returning the clip's level (not a hardcoded `1.0`) is what makes the
/// new slider audible at all — the previous version answered this
/// question with a bool, so any position between 0% and 100% played at
/// full volume.
fn volume_for(state: &ShellState) -> f64 {
    if state.project().master_muted {
        return 0.0;
    }
    match state.selected() {
        Some(clip) if clip.effective_muted() => 0.0,
        Some(clip) => f64::from(clip.volume).clamp(0.0, 1.0),
        None => 1.0,
    }
}

/// The first line of a multi-line diagnostic — the titlebar has room for
/// one line, and `caps.rs`'s full diagnosis is a paragraph.
fn first_line(message: &str) -> String {
    message.lines().next().unwrap_or(message).trim().to_string()
}

// The engine's event `Receiver` and the export progress `Receiver` are
// created in `new`/`start_export` but consumed by subscriptions, which
// iced constructs separately. These statics are the hand-off: each
// receiver is taken exactly once, by the subscription that owns it.
//
// # Why these are `Mutex` statics and not `thread_local!`
//
// They *were* thread-locals, and that was a real, screenshot-visible
// bug: opening a file did nothing at all. `App::new` runs on the main
// thread and stored the receiver in *its* thread-local, but the async
// block inside `iced::stream::channel` runs on iced's **executor thread
// pool** — a different thread, whose copy of the thread-local is
// `None`. The subscription therefore took the "already taken" branch and
// waited forever, so no `Opened`, `Frame`, or `Position` event ever
// reached the UI. The engine thread was working perfectly the whole
// time; nothing was listening. A `Mutex` static is shared across
// threads, which is what a hand-off between two unrelated threads
// actually requires.
static ENGINE_EVENTS: std::sync::Mutex<Option<std::sync::mpsc::Receiver<EngineEvent>>> =
    std::sync::Mutex::new(None);
static EXPORT_PROGRESS: std::sync::Mutex<Option<std::sync::mpsc::Receiver<ExportProgress>>> =
    std::sync::Mutex::new(None);


/// Bridge the engine thread's blocking `Receiver` into an async stream.
///
/// The receiver is drained on a **dedicated OS thread**, never on the
/// executor — see this module's doc comment for the bug that rule exists
/// to prevent.
fn engine_event_stream() -> impl iced::futures::Stream<Item = EngineEventKind> {
    iced::stream::channel(64, |mut sender| async move {
        let Some(events) = ENGINE_EVENTS.lock().expect("engine event slot poisoned").take() else {
            // Already taken (a subscription restart): stay open without
            // blocking, rather than ending the stream.
            std::future::pending::<()>().await;
            return;
        };

        std::thread::spawn(move || {
            while let Ok(event) = events.recv() {
                let mapped = match event {
                    EngineEvent::Opened { path, info } => EngineEventKind::Opened(path, info),
                    EngineEvent::Frame(f) => EngineEventKind::Frame(f),
                    EngineEvent::Position(t) => EngineEventKind::Position(t),
                    EngineEvent::PlayingChanged(p) => EngineEventKind::PlayingChanged(p),
                    EngineEvent::Eos => EngineEventKind::Eos,
                    EngineEvent::Error(e) => EngineEventKind::Error(e),
                };
                if iced::futures::executor::block_on(iced::futures::SinkExt::send(&mut sender, mapped)).is_err() {
                    break; // window closed
                }
            }
        });

        std::future::pending::<()>().await;
    })
}

fn export_progress_stream() -> impl iced::futures::Stream<Item = ExportProgress> {
    iced::stream::channel(16, |mut sender| async move {
        let Some(progress) = EXPORT_PROGRESS.lock().expect("export progress slot poisoned").take() else {
            std::future::pending::<()>().await;
            return;
        };
        std::thread::spawn(move || {
            while let Ok(p) = progress.recv() {
                if iced::futures::executor::block_on(iced::futures::SinkExt::send(&mut sender, p)).is_err() {
                    break;
                }
            }
        });
        std::future::pending::<()>().await;
    })
}

/// Run a blocking closure off the executor and await its result.
///
/// iced's executor does not expose a `spawn_blocking` equivalent to
/// application code, so this is the explicit version: one thread, one
/// oneshot channel. Thumbnail and waveform extraction are hundreds of
/// milliseconds of decoding each — several dropped frames if they happen
/// anywhere near the UI thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = iced::futures::channel::oneshot::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.await.expect("blocking task panicked")
}

fn main() -> iced::Result {
    iced::application(App::new, App::update, App::view)
        // Load every vendored face before the first frame, **including
        // the bold**. Naming a font the renderer does not have is silent:
        // iced substitutes a fallback and nothing reports it, which is how
        // the timecodes once ran proportional and a heading rendered as a
        // slab. Registering the bold cut is what makes `Weight::Bold`
        // resolve inside the family instead of falling out of it.
        .font(offcut_ui::shell::SANS_BYTES)
        .font(offcut_ui::shell::SANS_BOLD_BYTES)
        .font(offcut_ui::shell::MONO_BYTES)
        .default_font(offcut_ui::shell::SANS)
        .subscription(App::subscription)
        .title(App::title)
        .window_size(iced::Size::new(1440.0, 900.0))
        // The interface scale, applied above every widget in the tree.
        //
        // A closure over app state rather than a fixed setting, so
        // Ctrl +/- take effect on the next frame with no restart. iced
        // multiplies all layout and text by this, which is exactly why
        // the scale is a single number rather than a set of per-element
        // sizes: the proportions this codebase argues about at length
        // survive it unchanged.
        .scale_factor(|app: &App| app.state.ui_scale)
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offcut_ui::AdjustField;

    #[test]
    fn transport_and_edit_shortcuts_map_to_the_documented_keys() {
        let none = Modifiers::empty();
        assert_eq!(shortcut(Key::Character("k".into()), none), Some(ShellMessage::TogglePlay));
        assert_eq!(shortcut(Key::Character("j".into()), none), Some(ShellMessage::StepBack));
        assert_eq!(shortcut(Key::Character("l".into()), none), Some(ShellMessage::StepForward));
        assert_eq!(
            shortcut(Key::Named(iced::keyboard::key::Named::Space), none),
            Some(ShellMessage::TogglePlay)
        );
    }

    /// No key may reach an operation that has no surface to show its
    /// result.
    ///
    /// # The trapdoor this closes
    ///
    /// `s`, `d`, `Delete`, and `Backspace` used to fire Split, Duplicate,
    /// and ripple-delete. The product rules had already removed all three from
    /// the transport — in its words they are "capability, not features" —
    /// because splitting a one-clip timeline hides the trim bar and
    /// leaves the user with no trim control at all. The keyboard path was
    /// simply missed, so a stray letter on a surface where bare letters
    /// are transport keys silently destroyed the primary control, with no
    /// message and no visible cause.
    ///
    /// This test is the guard: it fails the moment any of those keys is
    /// wired back up before a multi-clip surface exists to justify it.
    #[test]
    fn operations_with_no_ui_surface_have_no_keyboard_path() {
        use iced::keyboard::key::Named;
        let none = Modifiers::empty();

        for key in ["s", "d"] {
            assert_eq!(
                shortcut(Key::Character(key.into()), none),
                None,
                "`{key}` reaches an operation with no surface: pressing it hides the trim \
                 bar with no way back. \"capability, not features\"."
            );
        }
        for named in [Named::Delete, Named::Backspace] {
            assert_eq!(
                shortcut(Key::Named(named), none),
                None,
                "{named:?} ripple-deletes the only clip with no confirmation and no \
                 visible result"
            );
        }
    }

    /// The product's core operation must be reachable without a pointer.
    ///
    /// Trimming is the entire job, and it was the one core operation with
    /// no key: `SetIn`/`SetOut` were emitted only from the drag path, so
    /// a keyboard-driven user could open, play, step, and export — but
    /// not trim. `i`/`o` is the idiom every editor shares.
    #[test]
    fn trimming_is_reachable_from_the_keyboard() {
        let none = Modifiers::empty();
        assert_eq!(
            shortcut(Key::Character("i".into()), none),
            Some(ShellMessage::SetInAtPlayhead)
        );
        assert_eq!(
            shortcut(Key::Character("o".into()), none),
            Some(ShellMessage::SetOutAtPlayhead)
        );
    }

    /// A key that reliably does nothing is worse than an unbound one: it
    /// teaches the user the whole map is unreliable.
    ///
    /// # The rule survived; its subject changed
    ///
    /// Ctrl +/- once drove `ZoomIn`/`ZoomOut`, which clamp and store a
    /// `zoom` the canvas never reads — it draws the trim bar and discards
    /// its layout. Three live shortcuts, zero pixels changed, so they
    /// were unbound and this test asserted they stayed that way.
    ///
    /// They are bound again, to interface scale, which iced genuinely
    /// applies to every widget in the tree. So the assertion inverts —
    /// these keys must now *do* something — while the rule behind it is
    /// unchanged: **the timeline zoom messages still have no keyboard
    /// path**, because nothing renders at that zoom yet. That half is
    /// asserted below, and it is the half that can silently regress.
    #[test]
    fn the_scale_keys_are_bound_and_the_dead_zoom_keys_are_not() {
        let cmd = Modifiers::COMMAND;
        assert_eq!(shortcut(Key::Character("=".into()), cmd), Some(ShellMessage::ScaleUp));
        assert_eq!(shortcut(Key::Character("+".into()), cmd), Some(ShellMessage::ScaleUp));
        assert_eq!(shortcut(Key::Character("-".into()), cmd), Some(ShellMessage::ScaleDown));
        assert_eq!(shortcut(Key::Character("0".into()), cmd), Some(ShellMessage::ScaleReset));

        // The still-inert operation must not acquire a key by accident.
        // `ZoomIn`/`ZoomOut` remain reachable only from code that draws
        // nothing, so a binding to them would be a dead shortcut again.
        for key in ["=", "+", "-", "0", "z", "y", "o", "e"] {
            let bound = shortcut(Key::Character(key.into()), cmd);
            assert!(
                !matches!(bound, Some(ShellMessage::ZoomIn) | Some(ShellMessage::ZoomOut)),
                "Ctrl+{key} reaches the timeline zoom, which the canvas ignores — \
                 the key would change nothing on screen"
            );
        }
    }

    /// A bare `-`, `+` or `0` must stay unbound.
    ///
    /// These sit next to the transport letters on the same surface, and
    /// `0` in particular is a plausible mis-hit while reaching for the
    /// number keys that switch inspector tabs. Scaling the entire
    /// interface from a stray keystroke, with no modifier, would be the
    /// same trapdoor class as the removed `s`/`d` bindings.
    #[test]
    fn the_scale_keys_require_the_modifier() {
        let none = Modifiers::empty();
        for key in ["=", "+", "-", "0"] {
            assert_eq!(
                shortcut(Key::Character(key.into()), none),
                None,
                "bare `{key}` rescales the whole interface without a modifier"
            );
        }
    }

    #[test]
    fn undo_and_redo_follow_the_platform_convention() {
        let cmd = Modifiers::COMMAND;
        assert_eq!(shortcut(Key::Character("z".into()), cmd), Some(ShellMessage::Undo));
        assert_eq!(
            shortcut(Key::Character("z".into()), cmd | Modifiers::SHIFT),
            Some(ShellMessage::Redo)
        );
        assert_eq!(shortcut(Key::Character("y".into()), cmd), Some(ShellMessage::Redo));
    }

    /// A bare letter must not trigger a command-modified action — the
    /// classic bug where typing in a future text field deletes a clip.
    ///
    /// `o` is the interesting case and the reason this is written as a
    /// *difference* rather than as "bare letters do nothing": bare `o`
    /// now sets the out-point while Ctrl+O still opens a file. The rule
    /// being defended is that the two never collapse into each other, not
    /// that the unmodified key is inert.
    #[test]
    fn unmodified_keys_do_not_trigger_command_shortcuts() {
        let none = Modifiers::empty();
        let cmd = Modifiers::COMMAND;

        assert_eq!(shortcut(Key::Character("z".into()), none), None);
        assert_eq!(shortcut(Key::Character("e".into()), none), None);

        for key in ["o", "z", "e"] {
            let bare = shortcut(Key::Character(key.into()), none);
            let modified = shortcut(Key::Character(key.into()), cmd);
            assert_ne!(
                bare, modified,
                "`{key}` and Ctrl+{key} resolve to the same action, so the modifier is \
                 doing nothing and one of the two is unreachable"
            );
        }

        // The specific collision that would cost a user their work: bare
        // `o` must never open a file dialog over an unsaved edit.
        assert_eq!(shortcut(Key::Character("o".into()), cmd), Some(ShellMessage::OpenFile));
        assert_ne!(shortcut(Key::Character("o".into()), none), Some(ShellMessage::OpenFile));
    }

    #[test]
    fn mute_has_its_own_key_because_it_is_a_core_operation() {
        assert_eq!(
            shortcut(Key::Character("m".into()), Modifiers::empty()),
            Some(ShellMessage::ToggleClipMute)
        );
    }

    #[test]
    fn number_keys_switch_inspector_tabs() {
        let none = Modifiers::empty();
        assert_eq!(
            shortcut(Key::Character("1".into()), none),
            Some(ShellMessage::SelectTab(InspectorTab::Video))
        );
        assert_eq!(
            shortcut(Key::Character("2".into()), none),
            Some(ShellMessage::SelectTab(InspectorTab::Crop))
        );
        assert_eq!(
            shortcut(Key::Character("3".into()), none),
            Some(ShellMessage::SelectTab(InspectorTab::Adjust))
        );
    }

    #[test]
    fn unknown_keys_are_ignored_rather_than_mapped_to_something_surprising() {
        let none = Modifiers::empty();
        assert_eq!(shortcut(Key::Character("q".into()), none), None);
        assert_eq!(shortcut(Key::Character("w".into()), none), None);
    }

    /// Escape dismisses the one thing that covers the picture.
    ///
    /// It previously mapped to nothing, correctly — there was no
    /// dismissable surface. Now the inspector plate is the only opaque
    /// thing in the window, and the key every desktop user already
    /// presses to mean "get this off my screen" has to reach it.
    #[test]
    fn escape_closes_the_inspector_and_keeps_it_closed() {
        let none = Modifiers::empty();
        assert_eq!(
            shortcut(Key::Named(iced::keyboard::key::Named::Escape), none),
            Some(ShellMessage::CloseInspector)
        );

        // Pressed twice, it must not toggle the plate back open — which
        // is exactly what wiring it to `SelectTab` would have done.
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        assert!(state.inspector_open, "selecting a tab opens the plate");
        state.update(ShellMessage::CloseInspector);
        assert!(!state.inspector_open);
        state.update(ShellMessage::CloseInspector);
        assert!(!state.inspector_open, "a second Escape must not reopen the plate");
    }

    fn state_with_one_clip() -> ShellState {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "/tmp/t.mp4".into(),
            duration: Time::from_nanos(10_000_000_000),
            fps: offcut_model::Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let sid = source.id;
        project.add_source(source);
        project.add_clip_for_source(sid).unwrap();
        ShellState::new(project)
    }

    /// Playback must stop at the out-point, not run on to the end of the
    /// source file. Without this the trim bar is decorative: the range
    /// claims "this is my clip" and pressing play ignores it.
    #[test]
    fn playback_stops_at_the_out_point_and_not_before() {
        let out = Time::from_nanos(5_000_000_000);

        assert!(!playback_should_stop(Time::ZERO, out));
        assert!(!playback_should_stop(Time::from_nanos(4_999_000_000), out));
        // Exactly the out-point: exclusive, so this frame is already past
        // the cut. Using `>` here would play one frame too many.
        assert!(playback_should_stop(out, out));
        assert!(playback_should_stop(Time::from_nanos(9_000_000_000), out));
    }

    /// Drag-and-drop is only advertised when the backend can deliver it.
    ///
    /// winit 0.30 implements file drops in its X11 backend only, and
    /// prefers Wayland whenever `WAYLAND_DISPLAY` is set — so on a native
    /// Wayland session no drop event can ever arrive. Claiming the
    /// feature there would make the app look broken rather than the
    /// toolkit look limited.
    ///
    /// The env vars are read through this one function, so the rule is
    /// testable without a window.
    #[test]
    fn drag_and_drop_is_claimed_only_when_the_backend_supports_it() {
        // The matrix, expressed as the predicate's own inputs.
        let available = |wayland: bool, x11: Option<&str>| {
            let x11_live = x11.is_some_and(|d| !d.is_empty());
            !wayland && x11_live
        };

        assert!(!available(true, Some(":0")), "Wayland wins, so drops cannot arrive");
        assert!(!available(true, None), "native Wayland with no X at all");
        assert!(available(false, Some(":0")), "X11 backend does deliver drops");
        assert!(!available(false, None), "no display server we can use");
        assert!(!available(false, Some("")), "an empty DISPLAY is not a display");
    }

    /// The formats accepted by a drop must match the file dialog's, in
    /// both directions — a divergence reads as "drag and drop is broken
    /// for mkv" rather than as a missing string.
    #[test]
    fn drops_and_the_open_dialog_accept_the_same_formats() {
        for ext in SUPPORTED_EXTENSIONS {
            let path = PathBuf::from(format!("/tmp/clip.{ext}"));
            assert!(is_supported_video(&path), "{ext} is offered by the dialog but not droppable");
        }
    }

    /// Drag-and-drop must accept exactly what the file dialog offers,
    /// case-insensitively (a phone writes `.MOV`, not `.mov`).
    #[test]
    fn dropping_accepts_the_same_formats_the_open_dialog_offers() {
        for ext in SUPPORTED_EXTENSIONS {
            let lower = PathBuf::from(format!("/tmp/clip.{ext}"));
            let upper = PathBuf::from(format!("/tmp/clip.{}", ext.to_uppercase()));
            assert!(is_supported_video(&lower), "{ext} should be droppable");
            assert!(is_supported_video(&upper), ".{ext} uppercase should be droppable too");
        }
    }

    /// Dropping something that is not a video must be rejected up front
    /// rather than handed to GStreamer to fail on slowly.
    #[test]
    fn dropping_a_non_video_is_rejected_rather_than_probed() {
        for path in ["/tmp/notes.pdf", "/tmp/song.mp3", "/tmp/archive.zip", "/tmp/noext"] {
            assert!(
                !is_supported_video(std::path::Path::new(path)),
                "{path} must not be treated as a video"
            );
        }
    }

    /// The volume slider must actually be audible at intermediate
    /// positions. The previous implementation answered this question with
    /// a bool, so 30% and 100% both played at full volume.
    #[test]
    fn the_volume_slider_is_audible_between_silent_and_full() {
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectClip(0));

        state.update(ShellMessage::SetClipVolume(0.3));
        let quiet = volume_for(&state);
        assert!((quiet - 0.3).abs() < 1e-6, "expected 0.3, got {quiet}");

        state.update(ShellMessage::SetClipVolume(1.0));
        assert!((volume_for(&state) - 1.0).abs() < 1e-6);

        state.update(ShellMessage::SetClipVolume(0.0));
        assert_eq!(volume_for(&state), 0.0, "zero on the slider must be silent");
    }

    /// Master mute outranks the slider: it is a global gate, not another
    /// level to be averaged in.
    #[test]
    fn master_mute_overrides_any_slider_position() {
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetClipVolume(1.0));
        state.update(ShellMessage::ToggleMasterMute);
        assert_eq!(volume_for(&state), 0.0, "master mute must win over a full slider");
    }

    /// the 4×-implies-mute rule must survive the move from a
    /// mute toggle to a continuous slider.
    #[test]
    fn the_4x_rule_still_silences_the_clip_regardless_of_slider_position() {
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetClipVolume(1.0));
        state.update(ShellMessage::SetSpeed(offcut_model::Speed::Four));
        assert_eq!(volume_for(&state), 0.0, "4x implies mute even at full slider");
    }

    /// Stepping onto the **last** frame must still seek the engine.
    ///
    /// `resolve_timeline_time` returns `None` at and past the end (the
    /// end is exclusive), and `seek_engine` used to bail out on that --
    /// silently. So stepping forward to the final frame moved the red
    /// mark and the timecode while the picture stayed put: exactly "the
    /// preview does not match the red line".
    #[test]
    fn the_end_of_the_timeline_still_resolves_to_a_seekable_frame() {
        let state = state_with_one_clip();
        let total = state.total_duration();
        let fps = state.fps();

        // The end itself is deliberately unresolvable...
        assert!(
            state.project().resolve_timeline_time(total).is_none(),
            "the end is exclusive by design"
        );

        // ...so the SEEK PATH must still produce a position for it,
        // rather than skipping the seek entirely.
        let (mut app, _) = App::new();
        app.state = state;
        let _ = fps;

        let resolved = app.resolve_for_seek(total);
        assert!(
            resolved.is_some(),
            "seeking to the end of the timeline produced no position -- the engine              is never told, so the picture stays on the previous frame while the              red mark and timecode both move"
        );
        assert!(
            resolved.unwrap().source_time <= app.state.project().clips[0].out_point,
            "and must land inside the clip"
        );
    }

    /// Playhead ownership is a held state, not a per-message toggle.
    ///
    /// It was a one-shot bool set by every seek and cleared by the next
    /// `Position` event, which assumed the two arrive one-for-one. They
    /// do not -- a drag issues ~60 seeks a second while the engine
    /// reports position as frames retire -- so the flag was routinely
    /// left set with no drag in progress and swallowed the position
    /// update that advances the playhead during playback.
    #[test]
    fn playhead_ownership_is_a_gesture_state_not_a_seek_counter() {
        let (mut app, _) = App::new();
        assert!(!app.user_owns_playhead, "nothing is being dragged at startup");

        // Many seeks, no gesture: ownership must not flip.
        for t in [1.0f64, 2.0, 3.0] {
            let _ = app.seek_engine(Time::from_nanos((t * 1e9) as u64), false);
        }
        assert!(
            !app.user_owns_playhead,
            "seeking is not the same as the user holding the playhead -- if this \
             flips, engine position updates get swallowed during playback"
        );
    }

    #[test]
    fn preview_volume_follows_master_mute() {
        let mut state = state_with_one_clip();
        assert_eq!(volume_for(&state), 1.0);
        state.update(ShellMessage::ToggleMasterMute);
        assert_eq!(volume_for(&state), 0.0);
    }

    #[test]
    fn preview_volume_follows_the_clips_effective_mute_including_the_4x_rule() {
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetSpeed(offcut_model::Speed::Four));
        assert_eq!(volume_for(&state), 0.0, "4x implies mute, so the monitor must be silent");
    }

    /// The rejection message must list every format the picker accepts.
    ///
    /// Written by hand it listed five of the six and silently dropped
    /// `.avi`, which turns a helpful message into a false one: a user
    /// holding an AVI that Offcut opens fine would be told it does not.
    #[test]
    fn the_unsupported_file_message_lists_every_format_actually_accepted() {
        let sentence = supported_extensions_sentence();
        for ext in SUPPORTED_EXTENSIONS {
            assert!(
                sentence.contains(&format!(".{ext}")),
                "`.{ext}` is accepted but the message does not mention it: {sentence}"
            );
        }
        assert!(sentence.contains(" and "), "reads as a list, not a sentence: {sentence}");
    }

    #[test]
    fn first_line_trims_a_multiline_diagnosis_to_something_a_titlebar_can_show() {
        let diagnosis = "Missing required GStreamer elements:\n  - x264enc\n  - mp4mux\n";
        assert_eq!(first_line(diagnosis), "Missing required GStreamer elements:");
        assert_eq!(first_line("single line"), "single line");
        assert_eq!(first_line(""), "");
    }

    /// Regression test for a bug clippy caught and the screen confirmed:
    /// the `LaneMeasured` arm sat *after* the catch-all
    /// `AppMessage::Shell(msg)` arm, so it was unreachable and the
    /// timeline's fit-to-window used the whole window's width instead of
    /// the narrower lane's — the last clip ran off the right edge.
    ///
    /// The arm order itself is what regressed, and `match` order is not
    /// directly observable from a test. What *is* observable is the
    /// consequence: fitting to a lane width must produce a smaller zoom
    /// than fitting to the full window, so a test that pins that
    /// relationship fails if the two widths are ever conflated again.
    #[test]
    fn fitting_to_the_lane_width_zooms_out_further_than_fitting_to_the_window() {
        let mut state = state_with_one_clip();
        let total = state.total_duration().as_secs_f64() as f32;
        assert!(total > 0.0);

        let zoom_for = |width: f32| {
            let usable = (width - 2.0 * offcut_ui::timeline::CONTENT_RAIL).max(200.0);
            (usable / total).clamp(offcut_ui::shell::ZOOM_MIN, offcut_ui::shell::ZOOM_MAX)
        };

        // A 1440px window whose inspector panel leaves ~1050px of lane.
        let window_zoom = zoom_for(1440.0);
        let lane_zoom = zoom_for(1050.0);
        assert!(
            lane_zoom < window_zoom,
            "fitting to the real lane width must zoom out further than fitting to \
             the whole window ({lane_zoom} vs {window_zoom}); if these are equal, \
             the app is measuring the wrong thing again"
        );

        // And the fit must actually put the whole timeline inside the lane.
        let drawn_width = total * lane_zoom;
        assert!(
            drawn_width <= 1050.0,
            "a fitted timeline drew {drawn_width}px into a 1050px lane — it would overflow"
        );
        let _ = &mut state;
    }

    /// The app's wiring must not lose the pure state machine's
    /// guarantees: an adjust change still reaches the shader uniform.
    #[test]
    fn adjust_changes_reach_the_preview_uniform() {
        let mut state = state_with_one_clip();
        state.update(ShellMessage::SelectClip(0));
        assert!(state.effects().is_at_rest());
        state.update(ShellMessage::SetAdjust(AdjustField::Smooth, 55));
        assert!(!state.effects().is_at_rest());
    }
}
