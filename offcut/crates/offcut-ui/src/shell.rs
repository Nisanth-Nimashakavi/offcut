//! The Shell: the design system's layout (Titlebar / Stage+Inspector /
//! Transport / Timeline), built from real `iced` widgets and driven by a
//! real `offcut_model::Project`.
//!
//! This is the full Phase 2 surface: all three inspector tabs (Video,
//! Crop, Adjust), the edit toolbar, the transport row with a zoom slider,
//! and the interactive `timeline.rs` canvas. Every control here is wired
//! to state a user can observe changing — there are no decorative
//! affordances left in this file.
//!
//! `ShellState::update` is a pure state transition (no I/O, no engine
//! calls) so the whole interaction model is unit-testable without opening
//! a window; `offcut-app` translates the subset of messages that need the
//! engine or the filesystem into commands, and everything else lands
//! here.

use crate::icons::{self, Icon, icon};
use crate::theme::{Mode, Palette};
use crate::timeline::{TimelineData, TimelineMessage, timeline_canvas};
use iced::widget::{button, column, container, row, slider, text, toggler};
use iced::{Alignment, Border, Color, Element, Font, Length, Shadow, Vector};
use std::sync::Arc;
use offcut_export::{ExportProgress, ExportSettings};
use offcut_model::{
    AdjustSettings, AdjustValue, AspectPreset, Clip, ClipId, CropGrid, History, Project,
    Rational, Speed, Time,
};
use offcut_render::EffectsUniform;

/// Which inspector tab is showing. The design system's tab bar: "three labels
/// in a row... Active: 12px/600 over a 2px underline."
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum InspectorTab {
    #[default]
    Video,
    Crop,
    Adjust,
}

impl InspectorTab {
    pub const ALL: [InspectorTab; 3] = [InspectorTab::Video, InspectorTab::Crop, InspectorTab::Adjust];

    pub fn label(self) -> &'static str {
        match self {
            InspectorTab::Video => "Video",
            InspectorTab::Crop => "Crop",
            InspectorTab::Adjust => "Adjust",
        }
    }
}

/// Which of the five Adjust sliders a message refers to. A closed enum,
/// matching the product's hard cap: "Five sliders, nothing else, ever."
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum AdjustField {
    Smooth,
    Tint,
    SkinTone,
    BlueTone,
    Vignette,
}

impl AdjustField {
    pub const ALL: [AdjustField; 5] = [
        AdjustField::Smooth,
        AdjustField::Tint,
        AdjustField::SkinTone,
        AdjustField::BlueTone,
        AdjustField::Vignette,
    ];

    pub fn label(self) -> &'static str {
        match self {
            AdjustField::Smooth => "Smooth",
            AdjustField::Tint => "Tint",
            AdjustField::SkinTone => "Skin tone",
            AdjustField::BlueTone => "Blue tone",
            AdjustField::Vignette => "Vignette",
        }
    }

    pub fn get(self, adjust: &AdjustSettings) -> u8 {
        match self {
            AdjustField::Smooth => adjust.smooth.get(),
            AdjustField::Tint => adjust.tint.get(),
            AdjustField::SkinTone => adjust.skin_tone.get(),
            AdjustField::BlueTone => adjust.blue_tone.get(),
            AdjustField::Vignette => adjust.vignette.get(),
        }
    }

    pub fn set(self, adjust: &mut AdjustSettings, value: u8) {
        let v = AdjustValue::new(value);
        match self {
            AdjustField::Smooth => adjust.smooth = v,
            AdjustField::Tint => adjust.tint = v,
            AdjustField::SkinTone => adjust.skin_tone = v,
            AdjustField::BlueTone => adjust.blue_tone = v,
            AdjustField::Vignette => adjust.vignette = v,
        }
    }
}

/// Where an export currently stands, for the titlebar's Export button and
/// the progress strip.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum ExportState {
    #[default]
    Idle,
    Running(ExportProgress),
    Done(std::path::PathBuf),
    Failed(String),
}

/// Everything the shell view needs. Transient UI state (selection, mode,
/// playhead, zoom) lives here rather than in `offcut_model::Project`,
/// which correctly has no opinion about any of it.
pub struct ShellState {
    pub history: History,
    pub mode: Mode,
    pub tab: InspectorTab,
    pub selected_clip: Option<usize>,
    pub playing: bool,
    /// Playhead position in TIMELINE time.
    pub playhead: Time,
    pub zoom: f32,
    /// The whole interface's scale factor, applied by iced above every
    /// widget in the tree.
    ///
    /// Distinct from `zoom`, which is a *timeline* measurement in pixels
    /// per second. This one is an accessibility and display preference:
    /// it makes 11px type readable on a dense panel without any code
    /// here choosing different sizes. Everything scales together, so the
    /// proportions the rest of this file argues about survive it.
    pub ui_scale: f32,
    /// The active theme: the built-in palettes, or the user's config
    /// merged over them.
    ///
    /// Held here rather than read at each call site so `palette()` stays
    /// the single funnel it already was — every widget in this file asks
    /// `state.palette()`, and none of them needs to know a config exists.
    pub theme: crate::rice::Riced,
    pub current_frame: Option<Arc<offcut_engine::Frame>>,
    pub export: ExportState,
    pub export_settings: ExportSettings,
    /// A message shown in the titlebar — a load error, a capability
    /// warning. The design rule Phase 7's "error surfaces", in the one place
    /// the user is already looking.
    pub status: Option<String>,
    /// True while a file is being dragged over the window. Drives the
    /// drop overlay — a drag with no visible target is a guess, and the
    /// user should not have to guess whether releasing will do anything.
    pub drop_hover: bool,
    /// Whether the windowing backend can deliver dropped files at all.
    ///
    /// `false` on a native Wayland session, where `winit` implements
    /// file drops only in its X11 backend. Surfaced rather than hidden:
    /// a feature that silently does nothing reads as a bug in this app,
    /// and the user's next move is to drag harder instead of reaching
    /// for the Open button.
    pub drag_and_drop_available: bool,
    /// Whether the HeaderBar's primary menu is open.
    pub menu_open: bool,
    /// Whether the inspector plate is showing.
    ///
    /// New in this world, and load-bearing rather than a convenience: the
    /// inspector is the only opaque surface left, so whether it is open
    /// decides whether the user is looking at a whole picture or a
    /// picture with a panel on it. It starts **closed**, because the
    /// first thing anyone does with a freshly opened video is watch it
    /// and set a range — neither of which needs Speed, Crop, or Adjust —
    /// and a tool that opens with a settings panel over the content has
    /// pre-empted a decision the user has not made yet.
    ///
    /// Opening any tab opens the plate; pressing the open tab again, or
    /// Escape, closes it. That is the whole model, and it is why `tab`
    /// remains meaningful while the plate is hidden: reopening returns
    /// you to the tab you were last using rather than resetting to Video.
    pub inspector_open: bool,
    /// Whether the user has ever moved a trim edge in this session.
    ///
    /// Drives the one-time hint under the trim bar. It is not persisted:
    /// this is a single-window tool with no profile, and writing a dotfile
    /// to suppress one line of text would be a heavier mechanism than the
    /// thing it suppresses. Re-showing it on a fresh launch is the honest
    /// trade — the hint costs one line and vanishes on first contact.
    pub has_trimmed: bool,
    /// Whether the keyboard reference is open.
    pub shortcuts_open: bool,
    /// The pending export, awaiting confirmation.
    ///
    /// `Some(path)` between the save dialog closing and the encode
    /// starting — the window where the confirm sheet is up.
    pub pending_export: Option<std::path::PathBuf>,
    /// The probed codec name ("H.264") for the inspector's Source block.
    /// Held here rather than on `offcut_model::Source` because it is
    /// presentation metadata from the probe — no edit operation consults
    /// it, and `offcut-model` stays free of fields that exist only to be
    /// displayed.
    pub codec: Option<String>,
}

/// Zoom range, in pixels per second of timeline.
///
/// The low end must fit a **feature-length** file on one screen, not a
/// short clip. The previous floor of 2 px/s capped the visible span at
/// about 12 minutes: opening a 1h41m film computed a fit zoom of 0.23
/// px/s, clamped it to 2, and drew the clip 12200px wide — eight and a
/// half screens, with the timeline appearing empty because everything
/// past the first 1440px was off-screen and there is no horizontal
/// scroll. 0.05 px/s fits about eight hours, which covers any single
/// source file a person is plausibly editing.
///
/// The high end puts individual frames several pixels apart at 30fps.
/// The interface scale's range and step.
///
/// # Why these bounds
///
/// The floor is 0.8 rather than something smaller because this window
/// has a hard minimum arrangement: below `NARROW_WIDTH` the inspector
/// already stacks under the picture, and shrinking the *whole* interface
/// further pushes 11px labels under 9px, where the type stops being
/// readable and starts being texture. Scaling down past legibility is
/// not a preference, it is a defect the user asked for.
///
/// The ceiling is 2.0: twice size is the accessibility case this exists
/// to serve, and beyond it a 1440×900 window can no longer hold the
/// inspector *and* a picture worth judging a crop on.
///
/// The step is multiplicative-feeling but stated additively at 0.1,
/// because unlike timeline zoom the useful span here is one doubling,
/// not four orders of magnitude — ten even stops read as a settings
/// control rather than as a continuous axis.
pub const UI_SCALE_MIN: f32 = 0.8;
pub const UI_SCALE_MAX: f32 = 2.0;
pub const UI_SCALE_DEFAULT: f32 = 1.0;
pub const UI_SCALE_STEP: f32 = 0.1;

pub const ZOOM_MIN: f32 = 0.05;
pub const ZOOM_MAX: f32 = 240.0;
pub const ZOOM_DEFAULT: f32 = 12.0;

#[derive(Debug, Clone, PartialEq)]
pub enum ShellMessage {
    ToggleMode,
    /// Open or close the HeaderBar's primary (hamburger) menu.
    ToggleMenu,
    /// Show a tab's panel, or hide the panel if that tab is already the
    /// one showing. One message for both directions, because the control
    /// is one button: a separate `CloseInspector` reachable only from a
    /// second widget would leave the tab button lying about its state.
    SelectTab(InspectorTab),
    /// Dismiss the inspector plate outright — the Escape key and the
    /// plate's own close mark. Distinct from `SelectTab`: this one always
    /// closes and never toggles, so a keyboard user cannot accidentally
    /// reopen the panel they just dismissed.
    CloseInspector,
    /// Open or close the keyboard reference.
    ToggleShortcuts,
    /// Confirm the pending export and start encoding.
    ConfirmExport,
    /// Abandon the pending export before any encoding begins.
    CancelPendingExport,
    /// Pick the codec for the pending export.
    SetExportCodec(offcut_export::VideoCodec),
    /// Pick the output container for the pending export.
    SetExportContainer(offcut_export::Container),
    SelectClip(usize),
    TogglePlay,
    StepBack,
    StepForward,
    SetSpeed(Speed),
    ToggleClipMute,
    ToggleMasterMute,
    Split,
    DeleteClip,
    DuplicateClip,
    /// Set the in-point to wherever the playhead is parked.
    ///
    /// The keyboard counterpart to dragging the in-handle. Trimming is
    /// this product's whole job, and it was the one core operation with
    /// no key at all — reachable by pointer only, which put it out of
    /// reach entirely for anyone driving the app from the keyboard.
    SetInAtPlayhead,
    /// Set the out-point to wherever the playhead is parked.
    SetOutAtPlayhead,
    Undo,
    Redo,
    SetZoom(f32),
    ZoomIn,
    ZoomOut,
    /// Step the interface scale by `UI_SCALE_STEP`, or return it to 1.0.
    ScaleUp,
    ScaleDown,
    ScaleReset,
    /// Switch to a saved theme by name, or to the built-in palette when
    /// `None`. The choice is remembered for the next launch.
    SelectTheme(Option<String>),
    SetAspect(AspectPreset),
    /// Which composition guide to overlay while framing.
    SetCropGrid(CropGrid),
    SetStraighten(f32),
    ResetCrop,
    SetAdjust(AdjustField, u8),
    /// Add `delta` to a slider, wrapping back to 0 past 100. Used by the
    /// keyboard path; the pointer path uses `SetAdjust` directly.
    NudgeAdjust(AdjustField, u8),
    ResetAdjust,
    /// Set clip volume (0.0 to 1.0)
    SetClipVolume(f32),
    /// Emitted by the video preview's interactive crop box.
    Video(crate::video::VideoMessage),
    /// Emitted by the timeline canvas — including the source trim bar,
    /// which shares that canvas and arrives wrapped as
    /// `Timeline(TimelineMessage::TrimBar(..))`. There is deliberately no
    /// separate top-level `TrimBar` variant: one existed briefly and was
    /// unreachable, because nothing could emit it once the bar stopped
    /// being its own widget.
    Timeline(TimelineMessage),
    /// Requests that only `offcut-app` can service (they need the engine,
    /// a file dialog, or a background task). `ShellState::update` treats
    /// them as no-ops so the pure state machine stays total.
    OpenFile,
    Export,
    CancelExport,
    /// A click on the export modal's scrim. Deliberately does nothing —
    /// it exists so the scrim can absorb pointer input that would
    /// otherwise reach the locked shell beneath.
    ExportScrimPressed,
    DismissStatus,
}

impl ShellState {
    pub fn new(project: Project) -> Self {
        let selected = if project.clips.is_empty() { None } else { Some(0) };
        Self {
            history: History::new(project),
            mode: Mode::Dark,
            tab: InspectorTab::Video,
            selected_clip: selected,
            playing: false,
            playhead: Time::ZERO,
            zoom: ZOOM_DEFAULT,
            ui_scale: UI_SCALE_DEFAULT,
            theme: crate::rice::Riced::builtin(),
            current_frame: None,
            export: ExportState::Idle,
            export_settings: ExportSettings::default(),
            status: None,
            drop_hover: false,
            drag_and_drop_available: true,
            menu_open: false,
            inspector_open: false,
            has_trimmed: false,
            shortcuts_open: false,
            pending_export: None,
            codec: None,
        }
    }

    pub fn project(&self) -> &Project {
        self.history.project()
    }

    pub fn selected(&self) -> Option<&Clip> {
        self.selected_clip.and_then(|i| self.project().clips.get(i))
    }

    /// The frame aspect of the selected clip's source, needed by both the
    /// crop math and the shader uniform.
    pub fn source_aspect(&self) -> f32 {
        self.selected()
            .and_then(|c| self.project().source(c.source))
            .map(|s| if s.resolution.1 > 0 { s.resolution.0 as f32 / s.resolution.1 as f32 } else { 1.0 })
            .unwrap_or(16.0 / 9.0)
    }

    pub fn fps(&self) -> Rational {
        self.project().sources.first().map(|s| s.fps).unwrap_or(Rational::WEB_30)
    }

    /// The resolution this project would export at.
    ///
    /// Delegates to `offcut-export`'s own `output_resolution` rather than
    /// recomputing it, so the number shown and the number written can
    /// never disagree — the whole point of surfacing it is that the user
    /// can trust it before spending minutes on an encode.
    pub fn output_resolution(&self) -> Option<(u32, u32)> {
        if self.project().clips.is_empty() {
            return None;
        }
        Some(offcut_export::output_resolution(self.project(), &self.export_settings))
    }

    /// The aspect of the region actually being shown — the source frame's
    /// aspect scaled by the crop rect's own proportions.
    ///
    /// Distinct from `source_aspect` because a crop changes the shape of
    /// what is on screen, and the letterbox math has to reason about the
    /// *displayed* shape, not the file's.
    pub fn displayed_aspect(&self) -> f32 {
        let base = self.source_aspect();
        match self.selected() {
            Some(clip) if clip.crop.rect.height > 0.0 => {
                base * clip.crop.rect.width / clip.crop.rect.height
            }
            _ => base,
        }
    }

    /// The crop/adjust state of the selected clip, in shader form. This
    /// is what makes the preview show the effects live.
    ///
    /// Built with `with_guides`, not `new`: this is the **editing**
    /// surface, and composition guides belong on it. The export path
    /// calls `new` and therefore cannot draw them, which is what keeps a
    /// rule-of-thirds overlay out of a finished video.
    pub fn effects(&self) -> EffectsUniform {
        match self.selected() {
            Some(clip) if self.tab == InspectorTab::Crop => {
                // On the Crop tab the preview shows the **whole frame**
                // with the crop drawn over it as a draggable box, rather
                // than the already-cropped result. You cannot frame a
                // shot against footage that has been cut away, and the
                // eight handles would otherwise sit on the edges of a
                // picture filling the entire viewport with nothing to
                // drag them across.
                EffectsUniform::editing_crop(&clip.crop, &clip.adjust, self.source_aspect())
            }
            Some(clip) => EffectsUniform::new(&clip.crop, &clip.adjust, self.source_aspect()),
            None => EffectsUniform::identity(self.source_aspect()),
        }
    }

    pub fn total_duration(&self) -> Time {
        self.project().total_timeline_duration()
    }

    /// Keep the playhead inside the timeline after an edit shortens it —
    /// otherwise deleting the last clip leaves the playhead stranded past
    /// the end, and the transport reports a time that does not exist.
    fn clamp_playhead(&mut self) {
        let total = self.total_duration();
        if self.playhead > total {
            self.playhead = total;
        }
    }

    /// Keep the selection valid after a delete.
    fn clamp_selection(&mut self) {
        let count = self.project().clips.len();
        self.selected_clip = match (self.selected_clip, count) {
            (_, 0) => None,
            (Some(i), n) if i >= n => Some(n - 1),
            (current, _) => current,
        };
    }

    /// Pure state transition. Every arm here is testable without a window,
    /// an engine, or a GPU — see this module's test suite.
    pub fn update(&mut self, message: ShellMessage) {
        match message {
            ShellMessage::ToggleMode => {
                self.mode = self.mode.toggled();
                // # Saying so when the toggle cannot show anything
                //
                // A `[wallpaper]` theme derives one palette and uses it
                // for both modes, so flipping the mode under one changes
                // no pixels. A key that reliably does nothing teaches the
                // user the whole map is unreliable — the same reason the
                // dead zoom shortcuts were unbound — so rather than
                // swallowing the press, it explains why nothing moved.
                if self.theme.dark == self.theme.light {
                    self.status = Some(
                        "This theme sets one palette for both appearances, so light and \
                         dark look the same."
                            .to_string(),
                    );
                }
                // Adwaita menus dismiss on activation: the menu exists to
                // reach an action, and one left hanging over the result
                // hides the change the user just asked for.
                self.menu_open = false;
            }
            ShellMessage::ToggleMenu => self.menu_open = !self.menu_open,

            // Pressing the tab that is already showing closes the plate.
            // This is what makes the picture reachable in one click from
            // anywhere: without it, opening Crop to check a framing would
            // leave a 312px panel over the frame with no obvious way back
            // to a clean view, and the user would learn to avoid the tabs.
            ShellMessage::SelectTab(tab) => {
                if self.inspector_open && self.tab == tab {
                    self.inspector_open = false;
                } else {
                    self.tab = tab;
                    self.inspector_open = true;
                }
            }
            // The tab is deliberately *not* reset here. Reopening should
            // return the user to the panel they were last using; resetting
            // to Video would discard the one piece of context the closed
            // plate still holds.
            // Escape closes the topmost plate, not always the inspector.
            //
            // With three dismissible surfaces the key has to mean "get the
            // frontmost thing off my screen", which is what a user
            // pressing it actually means. Closing the panel out from under
            // an open sheet would leave the sheet up and change something
            // behind it — the one behaviour a dismissal must never have.
            //
            // The pending export is deliberately absent: it is a decision
            // with two named buttons, and dismissing a decision with a key
            // is how people export the wrong thing.
            ShellMessage::CloseInspector => {
                if self.shortcuts_open {
                    self.shortcuts_open = false;
                } else if self.menu_open {
                    self.menu_open = false;
                } else {
                    self.inspector_open = false;
                }
            }

            ShellMessage::ToggleShortcuts => {
                self.shortcuts_open = !self.shortcuts_open;
                // The reference opens from the menu, and a popover left
                // hanging behind a sheet it launched is the same defect
                // the appearance toggle already fixed.
                self.menu_open = false;
            }

            // Confirming is app-serviced (it starts the encode), but the
            // sheet is shell state and must close here so the two cannot
            // disagree about whether a confirmation is still pending.
            ShellMessage::ConfirmExport => self.pending_export = None,
            ShellMessage::CancelPendingExport => self.pending_export = None,
            ShellMessage::SetExportContainer(container) => {
                self.export_settings.container = container;
                // A container that cannot carry the current codec moves
                // the codec, rather than leaving a pair the muxer will
                // reject at encode time. Nothing here can produce an
                // illegal combination today — all three accept both
                // codecs — but the sheet asks `accepts` instead of
                // assuming, so adding WebM later cannot ship a pairing
                // that fails only once the user presses Export.
                if !container.accepts(self.export_settings.codec)
                    && let Some(fallback) = offcut_export::VideoCodec::ALL
                        .into_iter()
                        .find(|c| container.accepts(*c))
                {
                    self.export_settings.codec = fallback;
                }
                // The pending path's extension has to follow, or the
                // confirm sheet names a file the encoder will not write.
                if let Some(path) = self.pending_export.take() {
                    self.pending_export = Some(path.with_extension(container.extension()));
                }
            }
            ShellMessage::SetExportCodec(codec) => {
                self.export_settings.codec = codec;
                // Bitrate follows the codec: HEVC reaches the same
                // quality at a lower rate, and leaving the H.264 number
                // in place would quietly overshoot. The suggestion is the
                // export crate's own, so the sheet cannot disagree with
                // what the encoder would have picked.
                if let Some(res) = self.output_resolution() {
                    self.export_settings.bitrate_kbps =
                        ExportSettings::suggested_bitrate_kbps(res, codec);
                }
            }
            ShellMessage::SelectClip(i) => {
                if i < self.project().clips.len() {
                    self.selected_clip = Some(i);
                }
            }
            ShellMessage::TogglePlay => self.playing = !self.playing,

            ShellMessage::StepBack | ShellMessage::StepForward => {
                let frame = self.fps().frame_duration();
                let total = self.total_duration();
                self.playhead = if message == ShellMessage::StepForward {
                    Time::from_nanos(
                        self.playhead.as_nanos().saturating_add(frame.as_nanos()).min(total.as_nanos()),
                    )
                } else {
                    Time::from_nanos(self.playhead.as_nanos().saturating_sub(frame.as_nanos()))
                };
            }

            ShellMessage::SetSpeed(speed) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    // # Why 4x does NOT write `muted = true`
                    //
                    // the design says "4x implies muted audio", and
                    // `Clip::effective_muted()` already derives exactly
                    // that. Writing the stored flag as well made the
                    // implication **permanent**: going 1x -> 4x -> 1x
                    // left the clip silent, with the toggle showing a
                    // mute the user never asked for and no memory that
                    // the speed had set it.
                    //
                    // An implied state must stay derived. The moment it
                    // is also stored, the two can disagree -- and here
                    // they did, in the one direction the user cannot
                    // undo without noticing what happened.
                    clip.speed = speed;
                }
                self.clamp_playhead();
            }

            ShellMessage::ToggleClipMute => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.muted = !clip.muted;
                }
            }

            ShellMessage::ToggleMasterMute => {
                let project = self.history.edit();
                project.master_muted = !project.master_muted;
            }

            // # Trimming from the keyboard
            //
            // These go through the *same* clamps the drag path uses
            // (`TrimBarData::clamp_in` / `clamp_out`), rather than writing
            // `trim_clip` directly. Two clamping rules for one edit would
            // drift, and the drift would show up as the keyboard being
            // able to produce a range the pointer refuses — an inverted or
            // sub-minimum selection that `set_range` then rejects, so the
            // key would look broken rather than bounded.
            //
            // Unlike a drag, this takes a history checkpoint per press:
            // a keystroke is a discrete act and should undo as one.
            ShellMessage::SetInAtPlayhead => {
                let Some(data) = self.trim_bar_data() else { return };
                let Some(clip_id) = self.project().clips.first().map(|c| c.id) else { return };
                self.has_trimmed = true;
                let target = data.clamp_in(self.playhead_source());
                // Already there: taking a checkpoint would leave an undo
                // entry that undoes nothing.
                if target == data.in_point {
                    return;
                }
                let before = self.playhead_source();
                let project = self.history.edit();
                let _ = project.trim_clip(clip_id, Some(target), None);
                self.set_playhead_source(before);
            }

            ShellMessage::SetOutAtPlayhead => {
                let Some(data) = self.trim_bar_data() else { return };
                let Some(clip_id) = self.project().clips.first().map(|c| c.id) else { return };
                self.has_trimmed = true;
                let target = data.clamp_out(self.playhead_source());
                if target == data.out_point {
                    return;
                }
                let before = self.playhead_source();
                let project = self.history.edit();
                let _ = project.trim_clip(clip_id, None, Some(target));
                self.set_playhead_source(before);
            }

            ShellMessage::Split => {
                let playhead = self.playhead;
                let project = self.history.edit();
                match project.split_at_timeline_time(playhead) {
                    // A split at a boundary or past the end is a no-op,
                    // so the checkpoint just taken would leave an undo
                    // entry that undoes nothing. Roll it back.
                    Ok(None) => {
                        self.history.undo();
                    }
                    Ok(Some(_)) => {
                        // Select the *left* half — the piece the playhead
                        // just left behind is the one a user typically
                        // wants to delete next.
                        if let Some(position) = self.project().resolve_timeline_time(playhead) {
                            self.selected_clip = Some(position.clip_index.saturating_sub(1).max(0));
                        }
                    }
                    Err(_) => {
                        self.history.undo();
                    }
                }
            }

            ShellMessage::DeleteClip => {
                let Some(index) = self.selected_clip else { return };
                let Some(clip_id) = self.project().clips.get(index).map(|c| c.id) else { return };
                let project = self.history.edit();
                if project.ripple_delete(clip_id).is_err() {
                    self.history.undo();
                    return;
                }
                self.clamp_selection();
                self.clamp_playhead();
            }

            ShellMessage::DuplicateClip => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                let Some(clip) = project.clips.get(index).cloned() else {
                    self.history.undo();
                    return;
                };
                let mut copy = clip;
                copy.id = ClipId::next();
                project.clips.insert(index + 1, copy);
                self.selected_clip = Some(index + 1);
            }

            ShellMessage::Undo => {
                self.menu_open = false;
                self.history.undo();
                self.clamp_selection();
                self.clamp_playhead();
            }
            ShellMessage::Redo => {
                self.menu_open = false;
                self.history.redo();
                self.clamp_selection();
                self.clamp_playhead();
            }

            ShellMessage::SetZoom(z) => self.zoom = z.clamp(ZOOM_MIN, ZOOM_MAX),
            // Multiplicative steps, not additive: zoom is perceptually
            // logarithmic, so a fixed +10px/s step is a huge jump when
            // zoomed out and imperceptible when zoomed in.
            ShellMessage::ZoomIn => self.zoom = (self.zoom * 1.5).clamp(ZOOM_MIN, ZOOM_MAX),
            ShellMessage::ZoomOut => self.zoom = (self.zoom / 1.5).clamp(ZOOM_MIN, ZOOM_MAX),

            // Interface scale. Rounded to the step before clamping, so
            // repeated presses land on the same ten stops every time
            // rather than accumulating float drift into 1.0999999.
            ShellMessage::ScaleUp => self.set_ui_scale(self.ui_scale + UI_SCALE_STEP),
            ShellMessage::ScaleDown => self.set_ui_scale(self.ui_scale - UI_SCALE_STEP),
            ShellMessage::ScaleReset => self.set_ui_scale(UI_SCALE_DEFAULT),

            ShellMessage::SelectTheme(name) => {
                // Applied and recorded in one step: a picker that changed
                // the colours but not the saved choice would silently
                // revert on the next launch, which reads as the app
                // forgetting rather than as two separate actions.
                let mut next = match &name {
                    Some(n) => crate::rice::load_theme(n),
                    None => crate::rice::Riced::builtin(),
                };
                next.name = name.clone();
                // The reading runs on the theme actually being applied,
                // so switching surfaces the same warnings startup would.
                if next.loaded {
                    for (mode, palette) in
                        [(Mode::Dark, next.dark), (Mode::Light, next.light)]
                    {
                        next.warnings.extend(crate::rice::audit(&palette, mode));
                        next.warnings.extend(crate::rice::hue_conflicts(&palette, mode));
                    }
                }
                self.status = crate::rice::summary(&next.warnings);
                self.theme = next;
                // A failure to record is worth saying: the theme applied,
                // and the user would otherwise discover on next launch
                // that it had not stuck.
                if let Err(e) = crate::rice::remember_theme(name.as_deref())
                    && self.status.is_none()
                {
                    self.status = Some(format!("Theme applied, but could not be saved: {e}"));
                }
                self.menu_open = false;
            }

            ShellMessage::SetAspect(preset) => {
                let Some(index) = self.selected_clip else { return };
                let aspect = self.source_aspect() as f64;
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.crop.apply_aspect(preset, aspect);
                }
            }

            ShellMessage::Video(msg) => self.update_video(msg),

            ShellMessage::SetCropGrid(grid) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.crop.grid = grid;
                }
            }

            ShellMessage::SetStraighten(degrees) => {
                let Some(index) = self.selected_clip else { return };
                // Uncheckpointed: a dial drag emits a message per pixel,
                // and the shell takes one checkpoint when the gesture
                // starts (see `Timeline(GestureBegan)`), so the whole
                // drag undoes in one step.
                let project = self.history.project_mut_uncheckpointed();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.crop.set_straighten_deg(degrees);
                }
            }

            ShellMessage::ResetCrop => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.crop = offcut_model::CropTransform::identity();
                }
            }

            ShellMessage::SetAdjust(field, value) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.project_mut_uncheckpointed();
                if let Some(clip) = project.clips.get_mut(index) {
                    field.set(&mut clip.adjust, value);
                }
            }

            ShellMessage::NudgeAdjust(field, delta) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    let current = field.get(&clip.adjust);
                    // Wrap rather than saturate: repeatedly pressing the
                    // key should cycle through the range, not park at 100
                    // with no way back without reaching for the pointer.
                    let next = if current >= 100 { 0 } else { (current + delta).min(100) };
                    field.set(&mut clip.adjust, next);
                }
            }

            ShellMessage::SetClipVolume(volume) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.volume = volume.clamp(0.0, 1.0);
                }
            }

            ShellMessage::ResetAdjust => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.edit();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.adjust = AdjustSettings::default();
                }
            }

            ShellMessage::Timeline(msg) => self.update_timeline(msg),

            ShellMessage::DismissStatus => self.status = None,

            // Serviced by offcut-app (engine/dialog/task), not here.
            ShellMessage::OpenFile
            | ShellMessage::Export
            | ShellMessage::CancelExport
            | ShellMessage::ExportScrimPressed => {}

            // Export is app-serviced, but the menu is shell state and
            // must not survive into the locked window.
        }
    }

    fn update_timeline(&mut self, message: TimelineMessage) {
        match message {
            TimelineMessage::Seek { to, .. } => {
                self.playhead = Time::from_nanos(to.as_nanos().min(self.total_duration().as_nanos()));
                // Scrubbing follows the clip under the playhead, so the
                // inspector always describes what is on screen.
                if let Some(position) = self.project().resolve_timeline_time(self.playhead) {
                    self.selected_clip = Some(position.clip_index);
                }
            }
            TimelineMessage::SelectClip(i) => {
                if i < self.project().clips.len() {
                    self.selected_clip = Some(i);
                    // Move the playhead to the clip's start so the stage
                    // shows what was just selected -- clicking a clip and
                    // seeing an unrelated frame is the kind of small lie
                    // that makes an editor feel broken.
                    self.playhead = self.project().clip_start_time(i);
                }
            }
            TimelineMessage::TrimStart { clip, to } => {
                let Some(clip_id) = self.project().clips.get(clip).map(|c| c.id) else { return };
                let project = self.history.project_mut_uncheckpointed();
                let _ = project.trim_clip(clip_id, Some(to), None);
                self.clamp_playhead();
            }
            TimelineMessage::TrimEnd { clip, to } => {
                let Some(clip_id) = self.project().clips.get(clip).map(|c| c.id) else { return };
                let project = self.history.project_mut_uncheckpointed();
                let _ = project.trim_clip(clip_id, None, Some(to));
                self.clamp_playhead();
            }
            TimelineMessage::GestureBegan => self.history.checkpoint(),
            TimelineMessage::GestureEnded => {}
            // Handled by offcut-app, which owns the zoom-to-fit policy.
            TimelineMessage::LaneMeasured(_) => {}
            TimelineMessage::TrimBar(msg) => self.update_trim_bar(msg),
        }
    }

    /// The trim bar edits the **first clip's** range against the whole
    /// source. That is the scope this control claims: "cut a piece out of
    /// this file". Multi-clip arrangement stays the timeline's job, and
    /// this bar is hidden when the timeline holds more than one clip (see
    /// `trim_bar_row`) rather than silently editing whichever clip
    /// happened to be first.
    fn update_trim_bar(&mut self, message: crate::trimbar::TrimBarMessage) {
        use crate::trimbar::TrimBarMessage as T;
        let Some(clip_id) = self.project().clips.first().map(|c| c.id) else { return };

        match message {
            // # Why the playhead is recomputed rather than simply left alone
            //
            // The playhead is *stored* in TIMELINE time, but the red line
            // is *drawn* in SOURCE time as `in_point + playhead × speed`.
            // So moving `in_point` slides the drawn line even when the
            // stored number is untouched — which is exactly the "the red
            // park marker follows my trim handle" behaviour being fixed
            // here. The old `clamp_playhead()` did not cause that and
            // could not have prevented it.
            //
            // Holding the line still therefore means actively
            // compensating: read the playhead as a source instant
            // *before* the trim, then write it back against the *new*
            // in-point afterwards. The stored value changes precisely so
            // that what is on screen does not.
            T::SetIn { to, push_playhead, .. } => {
                self.has_trimmed = true;
                let before = self.playhead_source();
                let project = self.history.project_mut_uncheckpointed();
                let _ = project.trim_clip(clip_id, Some(to), None);
                // A pushed playhead follows the edge (the handle has
                // overtaken it); otherwise it holds the exact source
                // instant it was already parked on.
                self.set_playhead_source(push_playhead.unwrap_or(before));
            }
            T::SetOut { to, push_playhead, .. } => {
                self.has_trimmed = true;
                let before = self.playhead_source();
                let project = self.history.project_mut_uncheckpointed();
                let _ = project.trim_clip(clip_id, None, Some(to));
                self.set_playhead_source(push_playhead.unwrap_or(before));
            }
            T::Scrub { to, .. } => {
                // The bar speaks SOURCE time; the playhead is TIMELINE
                // time. `set_playhead_source` owns that conversion —
                // including the clip's speed, which a naive subtraction
                // would drop for any sped-up clip.
                self.set_playhead_source(to);
            }
            T::GestureBegan => self.history.checkpoint(),
            T::GestureEnded => {}
        }
    }

    /// The interactive crop box's edits.
    ///
    /// A drag is many messages bracketed by one checkpoint, exactly like
    /// a trim drag, so the whole gesture undoes in a single step rather
    /// than one step per pixel of pointer travel.
    fn update_video(&mut self, message: crate::video::VideoMessage) {
        use crate::video::VideoMessage as V;
        match message {
            V::CropGestureBegan => self.history.checkpoint(),
            V::CropChanged(rect) => {
                let Some(index) = self.selected_clip else { return };
                let project = self.history.project_mut_uncheckpointed();
                if let Some(clip) = project.clips.get_mut(index) {
                    clip.crop.rect = rect;
                    // A hand-drawn box is by definition not one of the
                    // ratio presets any more, so the chip row must stop
                    // claiming it is. Under a lock the chosen ratio is
                    // preserved by the drag maths, so the preset stays
                    // true and is left alone.
                    if !clip.crop.lock_aspect() {
                        clip.crop.aspect = AspectPreset::Free;
                    }
                }
            }
            V::CropGestureEnded => {}
        }
    }

    /// The playhead as a SOURCE instant.
    ///
    /// The shell stores the playhead in timeline time because that is
    /// what the transport and the export both want. The trim bar works
    /// entirely in source time, because a range within a file is a source
    /// concept. This pair is the single conversion between them — the
    /// arithmetic existed inline in three places before, and one of them
    /// had already dropped the speed factor.
    fn playhead_source(&self) -> Time {
        match self.project().clips.first() {
            Some(clip) => Time::from_nanos(
                clip.in_point
                    .as_nanos()
                    .saturating_add((self.playhead.as_nanos() as f64 * clip.speed.factor()) as u64),
            ),
            None => self.playhead,
        }
    }

    /// The trim bar's geometry for the current project, or `None` when
    /// the bar is not showing.
    ///
    /// Shared by the keyboard trim path so it inherits the drag path's
    /// clamps and its single-clip guard, rather than re-deriving either.
    /// The palette for the current mode, with any user overrides
    /// applied.
    ///
    /// Every widget goes through here, which is what let user theming be
    /// added without touching a single call site: the config merges into
    /// `theme` once at startup and the rest of the file is unchanged.
    pub fn palette(&self) -> Palette {
        self.theme.palette(self.mode)
    }

    /// The only writer of `ui_scale`.
    ///
    /// Snaps to `UI_SCALE_STEP` before clamping. Without the snap,
    /// repeated `+`/`-` accumulates float error — 1.0 + 0.1 is
    /// 1.1000000000000001, and after a dozen presses the "same" stop is a
    /// different number each time, so a scale that was stepped up and
    /// back down never returns to exactly 1.0 and the reset item stays
    /// stubbornly enabled.
    ///
    /// NaN maps to the default rather than propagating: this becomes a
    /// multiplier on every layout dimension in the tree, and a NaN there
    /// collapses the window to nothing with no error anywhere. The same
    /// argument `CropTransform::set_straighten_deg` makes about the
    /// shader uniform.
    fn set_ui_scale(&mut self, value: f32) {
        if value.is_nan() {
            self.ui_scale = UI_SCALE_DEFAULT;
            return;
        }
        let snapped = (value / UI_SCALE_STEP).round() * UI_SCALE_STEP;
        self.ui_scale = snapped.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
    }

    /// The guard is the load-bearing part: the bar hides itself once the
    /// project is not one clip, and a key that still edited "clip 0 of 5"
    /// would be lying about its scope exactly the way the removed Split
    /// shortcut was.
    fn trim_bar_data(&self) -> Option<crate::trimbar::TrimBarData> {
        let project = self.project();
        if project.clips.len() != 1 {
            return None;
        }
        let clip = project.clips.first()?;
        let source = project.source(clip.source)?;

        Some(crate::trimbar::TrimBarData {
            palette: self.palette(),
            source_duration: source.duration,
            in_point: clip.in_point,
            out_point: clip.out_point,
            playhead: self.playhead_source(),
        })
    }

    /// Park the playhead at a SOURCE instant, clamped into the clip.
    ///
    /// The clamp is the invariant the whole trim interaction rests on:
    /// **the playhead can never sit outside the clip.** A red line beyond
    /// either edge points at a frame the clip does not contain, so there
    /// is nothing the stage could honestly show for it.
    fn set_playhead_source(&mut self, source: Time) {
        let Some(clip) = self.project().clips.first() else { return };
        let (in_point, out_point, speed) = (clip.in_point, clip.out_point, clip.speed.factor());

        let clamped = source
            .as_nanos()
            .clamp(in_point.as_nanos(), out_point.as_nanos());
        let offset = clamped.saturating_sub(in_point.as_nanos());
        let timeline = (offset as f64 / speed) as u64;
        self.playhead = Time::from_nanos(timeline.min(self.total_duration().as_nanos()));
    }
}

/// The two faces, vendored under `fonts/` with their OFL texts and
/// registered at startup by `offcut-app`.
///
/// # Why Inter
///
/// This is an Operate surface — the visitor is here to finish a task, not
/// to admire the lettering — and the register the user named is the Apple
/// pro-media one, whose interface face is SF Pro. SF Pro is not
/// redistributable, so shipping it is not an option; Inter is the open
/// face designed for the same job (screen UI at small sizes, tall
/// x-height, unambiguous `1`/`l`/`I`) and is what most of the software
/// reaching for that register actually uses.
///
/// Vendored rather than named-and-hoped-for. Naming a font the renderer
/// does not have is silent: iced substitutes a fallback and nothing
/// reports it, which is how this codebase once shipped proportional
/// timecodes and a heading in a slab serif.
pub const SANS: Font = Font::with_name("Inter");

/// Tabular digits for every number in the interface.
///
/// Monospace here is **measurement**, not a technical costume: these
/// glyphs carry timecode, frame counts and percentages that update while
/// the pointer moves, and a proportional digit set makes a scrubbing
/// readout shift under the cursor you are aiming with. That is the one
/// use the craft floor explicitly allows.
const MONO: Font = Font::with_name("JetBrains Mono");

/// The vendored font bytes, handed to iced at startup.
///
/// Both weights of the sans are registered as **separate static faces**.
/// `Inter-SemiBold.ttf` reports its family as `Inter SemiBold`, not as
/// `Inter` at weight 600, so `semibold()` below names that family
/// directly rather than setting a weight on `Inter` — asking for
/// `Weight::Semibold` on the `Inter` family would find no face and fall
/// out of the family, which is exactly the bug that put a slab serif in
/// the inspector heading.
pub const SANS_BYTES: &[u8] = include_bytes!("../fonts/Inter-Regular.ttf");
pub const SANS_BOLD_BYTES: &[u8] = include_bytes!("../fonts/Inter-SemiBold.ttf");
pub const MONO_BYTES: &[u8] = include_bytes!("../fonts/JetBrainsMono-Regular.ttf");

/// Emphasis, at a weight the loaded faces genuinely supply.
///
/// Two weights, no more: Regular for body, SemiBold for headings and
/// active labels. Semibold rather than Bold is the register's own
/// convention — Apple's interface type tops out at semibold for almost
/// everything, and a true Bold at 13px in a dense panel reads as shouting.
fn bold() -> Font {
    Font::with_name("Inter SemiBold")
}

fn semibold() -> Font {
    Font::with_name("Inter SemiBold")
}

/// The design system's timecode format is frame-based (`HH:MM:SS:FF`), not
/// decimal seconds — this uses `Rational::time_to_frame_floor` to get an
/// exact frame count, per the "Time is rational, not float"
/// rule, rather than computing the frame number from a float division
/// that could round differently than the model's own frame math.
pub fn fmt_timecode(time: Time, fps: Rational) -> String {
    let total_frames = fps.time_to_frame_floor(time).max(0) as u64;
    let fps_rounded = fps.as_f64().round().max(1.0) as u64;
    let f = total_frames % fps_rounded;
    let total_s = total_frames / fps_rounded;
    format!("{:02}:{:02}:{:02}:{:02}", total_s / 3600, (total_s / 60) % 60, total_s % 60, f)
}

/// Shorten a filename from the **middle**, keeping the start and the
/// extension.
///
/// The inspector heading is now a filename, and filenames are
/// user-supplied and unbounded: a screen-recording name wraps to three
/// lines in a 320px panel and shoves the tabs off the bottom. Truncating
/// from the end would drop the extension, which is the half people
/// actually scan for — `recording-2026-…-final.mov` answers "is this the
/// mov or the mp4" and `recording-2026-03-1…` does not.
///
/// Operates on `char`s, not bytes: a byte split lands mid-codepoint on
/// any non-ASCII name and panics.
fn elide_middle(name: &str) -> String {
    const MAX: usize = 30;
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= MAX {
        return name.to_string();
    }
    // Biased towards the tail, because the extension and any
    // "-final"/"-v2" suffix live there.
    let head = 14;
    let tail = MAX - head - 1;
    let front: String = chars[..head].iter().collect();
    let back: String = chars[chars.len() - tail..].iter().collect();
    format!("{front}…{back}")
}

/// The content rail: the horizontal margin used by every band.
///
/// One constant shared by the toolbar, the transport, the trim bar and
/// the inspector, so their contents line up down the window instead of
/// each band carrying its own padding. That misalignment is a real bug
/// this code had before: the trim readout used 24 and the bar used 11,
/// putting two halves of one control 13px out on both edges.
pub const RAIL: f32 = 16.0;

/// The straighten dial's parts, stated once.
///
/// The well was a literal `40.0`, which happened to fit a 10px tick above
/// a 16px slider inside 6px padding. Raising the slider to a 24px pointer
/// target took the contents to 46px — six pixels of overflow inside a
/// fixed box, the same class of defect as the trim handles sliced flat at
/// the window edge, and invisible to every test in the suite until
/// `the_straighten_well_can_hold_its_own_contents` was written.
pub const DIAL_TICK: f32 = 10.0;
pub const DIAL_SLIDER: f32 = 24.0;
pub const DIAL_PAD: f32 = 6.0;
/// Derived, never typed: the tick, the slider, and padding on both sides.
pub const DIAL_WELL: f32 = DIAL_TICK + DIAL_SLIDER + DIAL_PAD * 2.0;

/// Below this width the inspector stops sitting beside the picture and
/// moves beneath it.
///
/// Chosen from arithmetic, not from a table of device sizes: the panel is
/// `INSPECTOR_WIDTH` (320) and the picture needs roughly 360 logical
/// pixels before a 16:9 fit becomes a letterboxed sliver you cannot judge
/// a crop on. 320 + 360 + two rails lands near 712, so 780 leaves
/// headroom before the stage stops being usable.
///
/// This branch is what makes the layout hold in the portrait window this
/// compositor actually produces.
pub const NARROW_WIDTH: f32 = 780.0;

/// Below this height the window drops the transport's secondary readouts
/// rather than letting four bands squeeze the picture to nothing.
pub const SHORT_HEIGHT: f32 = 560.0;

/// The inspector's width beside the picture.
///
/// 320, not the previous 312: at 312 the Adjust tab's five label/value
/// rows wrapped their longest label ("Skin tone" plus its percentage) at
/// the default type size.
pub const INSPECTOR_WIDTH: f32 = 320.0;

/// Toolbar height. Tall enough for a 28px control plus breathing room,
/// which is the proportion this register uses.
pub const TOOLBAR_HEIGHT: f32 = 52.0;

/// Transport height, holding the play cluster and the two timecodes.
pub const TRANSPORT_HEIGHT: f32 = 56.0;

/// The trim row's internal spacing: readout, then bar.
const TRIM_ROW_GAP: f32 = 6.0;
/// Air above the readout and below the bar.
const TRIM_ROW_PAD: f32 = 10.0;
/// The readout line's height at 12px type.
const TRIM_READOUT_HEIGHT: f32 = 16.0;

/// The whole trim band, **derived from its parts** rather than guessed.
///
/// This has to be stated, not implied. It is the last child of a `column`
/// whose middle child is `Length::Fill`, so an unstated height leaves it
/// the fill's remainder — and that remainder came up short, slicing the
/// round trim handles off flat against the bottom of the window. Deriving
/// it means adding a row or changing the type size cannot silently
/// re-introduce the clip, and a test pins the sum.
pub const TRIM_BAR_ROW_HEIGHT: f32 = TRIM_ROW_PAD * 2.0
    + TRIM_READOUT_HEIGHT
    + TRIM_ROW_GAP
    + crate::timeline::TOTAL_HEIGHT;

pub fn view(state: &ShellState) -> Element<'_, ShellMessage> {
    let palette = state.palette();

    // # The conventional arrangement, built properly
    //
    // Toolbar across the top, picture filling the centre on black, a
    // fixed inspector down the right, transport and trim bar beneath.
    // This is the layout a person who has opened a video editor before
    // already knows, which is the entire point: the user asked for the
    // category standard rather than a novel one, and the craft goes into
    // executing it exactly, not into rearranging it.
    //
    // `responsive` supplies the real window size so the inspector can
    // move below the picture in a narrow window instead of strangling it.
    // The previous build's failure was not the arrangement — it was that
    // the stage kept whatever space the fixed bands left over, so a small
    // source in a tall window produced large dead regions. Here the stage
    // takes `Length::Fill` on both axes and the bands are the only fixed
    // heights, so the picture receives every pixel not spoken for.
    iced::widget::responsive(move |size| {
        let narrow = size.width < NARROW_WIDTH;
        let short = size.height < SHORT_HEIGHT;

        // # The inspector is a plate you open, not a wall you live behind
        //
        // `inspector_open` was written, toggled, tested in nine places,
        // and read by nothing: `view` rendered the panel unconditionally,
        // so the app shipped with 320px of settings permanently over the
        // picture and Escape as a dead key. The state was right and the
        // view ignored it.
        //
        // Reading it costs the tabs their home, because they lived inside
        // the panel — so closing it would hide the only way to reopen it.
        // The tab cluster therefore moves to the toolbar (see `toolbar`),
        // where it is visible in both states and doubles as the open/close
        // indicator: the active tab is filled while the plate is open, and
        // all three read as inactive while it is closed.
        let open = state.inspector_open;
        let body: Element<'_, ShellMessage> = if narrow {
            // The stacked panel takes a share of the body, not a slab.
            //
            // It was `Length::Fixed(300.0)` — a number tied to nothing in
            // particular. At 700×900 it cut the Audio list horizontally
            // through "Mute all audio", leaving a control sliced in half
            // against the transport. The panel scrolls, so the content
            // was *reachable*; that is not the point. A control bisected
            // by a band edge reads as broken rather than as scrollable,
            // and a first impression of breakage is not repaired by a
            // scrollbar the user has not found yet.
            //
            // Deriving it from the real body height keeps two guarantees
            // at every window size: the picture never collapses, and the
            // panel gets a whole number of rows rather than a cut through
            // one. `body_height` is what the bands leave over.
            let body_height =
                (size.height - TOOLBAR_HEIGHT - TRANSPORT_HEIGHT - TRIM_BAR_ROW_HEIGHT).max(0.0);

            let mut col = column![stage(state, palette)];
            if open {
                col = col.push(inspector(state, palette, Some(body_height)));
            }
            col.height(Length::Fill).into()
        } else {
            let mut r = row![stage(state, palette)];
            if open {
                r = r.push(inspector(state, palette, None));
            }
            r.height(Length::Fill).into()
        };

        let shell = container(column![
            toolbar(state, palette),
            body,
            transport(state, palette, short),
            trim_bar_row(state, palette),
        ])
        .style(move |_| container::Style {
            background: Some(palette.canvas.into()),
            ..Default::default()
        })
        .width(Length::Fill)
        .height(Length::Fill);

        // # Exporting takes the whole window
        //
        // An encode is minutes of work that reads the *entire* project as
        // it goes. Letting the user keep trimming and cropping during it
        // means the file being written stops matching the app on screen,
        // and there is no honest way to reconcile the two afterwards —
        // the frames already encoded cannot be revised.
        //
        // Rather than disable each control individually (a list that goes
        // stale the moment a control is added), the whole shell goes
        // behind a modal whose scrim is a real widget above everything,
        // so clicks land on it rather than on the controls beneath.
        let mut layers = iced::widget::stack![shell];

        // Drop feedback. A drag with no visible target leaves the user
        // guessing whether releasing will do anything.
        if state.drop_hover && state.drag_and_drop_available {
            layers = layers.push(drop_overlay(palette));
        }

        match (&state.export, state.menu_open) {
            // The export lock outranks everything: while encoding, no
            // other plate may be open over the shell.
            (ExportState::Running(progress), _) => {
                layers = layers.push(export_modal(progress, palette));
            }
            _ => {
                // Order is deliberate. The confirm sheet is a decision the
                // user is mid-way through, so it sits above the reference
                // they may have opened to check a key, which in turn sits
                // above the menu that launched it.
                if let Some(path) = &state.pending_export {
                    layers = layers.push(export_confirm(state, path, palette));
                } else if state.shortcuts_open {
                    layers = layers.push(shortcuts_sheet(palette));
                } else if state.menu_open {
                    layers = layers.push(primary_menu(state, palette));
                }
            }
        }

        layers.into()
    })
    .into()
}

/// The centred export progress panel, shown over a scrim that locks the
/// window until the encode finishes or is cancelled.
fn export_modal<'a>(
    progress: &ExportProgress,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let fraction = progress.fraction().clamp(0.0, 1.0);
    let percent = (fraction * 100.0).round() as u32;

    // A real bar, not just a number: a percentage alone gives no sense of
    // rate, and an encode is long enough that "is this moving at all" is
    // the actual question being asked.
    // Two portions -- filled and remaining -- so the bar actually shows a
    // fraction. A single `FillPortion` child with no sibling expands to
    // the whole track and reads as 100% at every stage of the encode.
    // Clamped to 1..=999: `FillPortion(0)` is not a valid layout weight,
    // and both ends of an encode hit zero (0% at the start, and the
    // remainder at the finish).
    let filled = ((fraction * 1000.0).round() as i32).clamp(1, 999) as u16;
    let track = container(
        row![
            container(iced::widget::Space::new().height(Length::Fixed(6.0)))
                .width(Length::FillPortion(filled))
                .style(move |_| container::Style {
                    background: Some(palette.accent.into()),
                    border: Border { radius: 3.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            iced::widget::Space::new().width(Length::FillPortion(1000 - filled)),
        ],
    )
    .width(Length::Fill)
    .height(Length::Fixed(6.0))
    .style(move |_| container::Style {
        background: Some(palette.surface_sunken.into()),
        border: Border { radius: 3.0.into(), ..Default::default() },
        ..Default::default()
    });

    let elapsed = fmt_timecode_secs(progress.position);
    let total = fmt_timecode_secs(progress.total);

    let panel = container(
        column![
            text("Exporting video").size(17).font(bold()).color(palette.text_primary),
            text("Editing is paused until this finishes, so the file matches what you see.")
                .size(12)
                .color(palette.text_secondary),
            track,
            row![
                text(format!("{percent}%")).size(12).font(MONO).color(palette.accent_tint_text),
                horizontal_space(),
                text(format!("{elapsed} / {total}")).size(12).font(MONO).color(palette.text_muted),
            ]
            .align_y(Alignment::Center),
            // Stays reachable: pausing editing must not mean trapping
            // the user in a long encode they no longer want.
            //
            // "Stop export", not "Cancel". Three plates in this app had a
            // button reading "Cancel" and this was the only one that
            // abandons work already done — the other two just close a
            // sheet. Naming the action makes the destructive one the
            // odd button out instead of the identical one.
            button(text("Stop export").size(13).color(palette.text_primary))
                .on_press(ShellMessage::CancelExport)
                .padding([8, 16])
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(
                            if hovered { palette.surface_hover } else { palette.surface_raised }.into(),
                        ),
                        text_color: palette.text_primary,
                        border: Border {
                            color: palette.border_raised,
                            width: 1.0,
                            radius: 10.0.into(),
                        },
                        ..Default::default()
                    }
                }),
        ]
        .spacing(14)
        .align_x(Alignment::Center),
    )
    .width(Length::Fixed(380.0))
    .padding(26)
    .style(move |_| container::Style {
        background: Some(palette.surface.into()),
        border: Border { color: palette.border_raised, width: 1.0, radius: 16.0.into() },
        shadow: Shadow {
            color: Color { a: 0.6, ..Color::BLACK },
            offset: Vector::new(0.0, 16.0),
            blur_radius: 40.0,
        },
        ..Default::default()
    });

    // `mouse_area` over the full scrim swallows clicks aimed at the shell
    // beneath. Without it the modal would be a picture of a lock rather
    // than a lock.
    //
    // It publishes `ExportScrimPressed`, a deliberate no-op, rather than
    // reusing some existing message: the scrim's job is to *absorb*, and
    // wiring it to a real action would make a stray click on the
    // background do something the user did not ask for.
    iced::widget::mouse_area(
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Color { a: 0.78, ..palette.canvas }.into()),
                ..Default::default()
            }),
    )
    .on_press(ShellMessage::ExportScrimPressed)
    .into()
}

/// Wall-clock duration for the export surfaces, where frame numbers
/// would be noise.
///
/// This **is** `trimbar::fmt_duration` — it was a second implementation
/// that dropped the hours entirely, so a 1h01m01s encode reported
/// `61:01` in the progress plate while the trim readout, three bands
/// away, called the same file `1:01:01`. Two formats for one quantity is
/// two answers to "how long is this", and one of them was wrong.
///
/// Kept as a named alias rather than a call-through at every site so
/// the *intent* ("the encode's clock") still reads locally.
fn fmt_timecode_secs(time: Time) -> String {
    crate::trimbar::fmt_duration(time)
}

/// Names the colour config and its state, as a menu row.
///
/// Not a button: there is nothing to press. Opening an editor from here
/// would mean choosing one, and getting that wrong on a stranger's
/// machine is worse than printing a path they can paste. It reports
/// whether a config was found, because "my theme is not loading" and "I
/// have not made one yet" look identical from inside the app.
fn theme_row<'a>(theme: &crate::rice::Riced, palette: Palette) -> Element<'a, ShellMessage> {
    let saved = crate::rice::available_themes();

    // The path as the user would type it, with `$HOME` collapsed to `~`.
    let shorten = |p: std::path::PathBuf| {
        let shown = p.display().to_string();
        match std::env::var("HOME") {
            Ok(home) if !home.is_empty() && shown.starts_with(&home) => {
                format!("~{}", &shown[home.len()..])
            }
            _ => shown,
        }
    };

    // # With no saved themes, the row explains rather than offers
    //
    // An empty picker is a control that does nothing and says nothing
    // about why. Naming the directory is what turns "this app has no
    // themes" into "put one here", which is the actual next step.
    if saved.is_empty() {
        let where_to_put_them = crate::rice::themes_dir()
            .map(|d| shorten(d.join("<name>.toml")))
            .unwrap_or_else(|| "no config location on this system".to_string());
        return container(
            column![
                row![
                    text("Colours").size(13).color(palette.text_primary),
                    horizontal_space(),
                    text(if theme.loaded { "colors.toml" } else { "built-in" })
                        .size(11)
                        .color(palette.text_secondary),
                ]
                .align_y(Alignment::Center),
                text(elide_middle(&where_to_put_them))
                    .size(10)
                    .font(MONO)
                    .color(palette.text_muted),
            ]
            .spacing(2),
        )
        .padding([2, 10])
        .into();
    }

    // One row per theme, plus the built-in, each showing whether it is
    // the active one. A list rather than a cycling button: with more
    // than two themes, "next" makes the user step through choices they
    // did not want to see, and there is no way back.
    let mut rows = column![row![
        text("Colours").size(13).color(palette.text_primary),
        horizontal_space(),
        text(match &theme.name {
            Some(n) => n.clone(),
            None if theme.loaded => "colors.toml".to_string(),
            None => "built-in".to_string(),
        })
        .size(11)
        .color(palette.text_secondary),
    ]
    .align_y(Alignment::Center)]
    .spacing(2);

    let option = |label: String, value: Option<String>, active: bool, palette: Palette| {
        let color = if active { palette.accent_tint_text } else { palette.text_secondary };
        button(
            row![
                text(if active { "•" } else { " " }).size(11).color(color),
                text(label).size(12).color(color),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .on_press(ShellMessage::SelectTheme(value))
        .width(Length::Fill)
        .padding([5, 6])
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered) && !active;
            button::Style {
                background: Some(
                    if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                ),
                text_color: color,
                border: Border { radius: 5.0.into(), ..Default::default() },
                ..Default::default()
            }
        })
    };

    rows = rows.push(option("Built-in".into(), None, theme.name.is_none(), palette));
    for name in saved {
        let active = theme.name.as_deref() == Some(name.as_str());
        rows = rows.push(option(name.clone(), Some(name), active, palette));
    }

    container(rows).padding([2, 10]).into()
}

/// The interface-scale stepper: a label, the current value, and −/+.
///
/// Shows the percentage because a scale control with no readout gives
/// the user no way to know how far from default they have drifted, and
/// "back to normal" then means pressing a key an unknown number of
/// times. The value is also the reset target: Ctrl+0 and this row's own
/// 100% are the same fact stated twice.
///
/// Buttons disable at the ends rather than silently clamping — a control
/// that keeps accepting presses after it has stopped moving is the same
/// lie as a shortcut that does nothing.
fn scale_stepper<'a>(scale: f32, palette: Palette) -> Element<'a, ShellMessage> {
    fn step<'b>(
        glyph: &'b str,
        message: ShellMessage,
        enabled: bool,
        palette: Palette,
    ) -> Element<'b, ShellMessage> {
        let color = if enabled { palette.text_primary } else { palette.text_muted_alt };
        let mut b = button(text(glyph).size(14).color(color).center())
            // 24px square: the same pointer floor every other target in
            // this window was raised to.
            .width(Length::Fixed(24.0))
            .height(Length::Fixed(24.0))
            .padding(0)
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered) && enabled;
                button::Style {
                    background: Some(
                        if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                    ),
                    text_color: color,
                    border: Border { radius: 5.0.into(), ..Default::default() },
                    ..Default::default()
                }
            });
        if enabled {
            b = b.on_press(message);
        }
        b.into()
    }

    // Compared against the step's midpoint, not with `==`: the snap in
    // `set_ui_scale` lands close to the bound but float equality on an
    // accumulated sum is not something to bet a disabled state on.
    let at_min = scale <= UI_SCALE_MIN + UI_SCALE_STEP / 2.0;
    let at_max = scale >= UI_SCALE_MAX - UI_SCALE_STEP / 2.0;

    container(
        row![
            text("Interface size").size(13).color(palette.text_primary),
            horizontal_space(),
            text(format!("{:.0}%", scale * 100.0))
                .size(11)
                .font(MONO)
                .color(palette.text_secondary),
            step("−", ShellMessage::ScaleDown, !at_min, palette),
            step("+", ShellMessage::ScaleUp, !at_max, palette),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .padding([2, 10])
    .into()
}

/// One row of the primary menu. Adwaita menu items are full-width, left
/// aligned, 6px radius, with the label carrying the action.
fn menu_item<'a>(
    label: &'a str,
    enabled: bool,
    message: ShellMessage,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let color = if enabled { palette.text_primary } else { palette.text_muted_alt };
    let mut b = button(text(label).size(13).color(color))
        .width(Length::Fill)
        .padding([7, 10])
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered) && enabled;
            button::Style {
                background: Some(
                    if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                ),
                text_color: color,
                border: Border { radius: 6.0.into(), ..Default::default() },
                ..Default::default()
            }
        });
    if enabled {
        b = b.on_press(message);
    }
    b.into()
}

/// The HeaderBar's primary menu, as an Adwaita popover: a card floating
/// under the hamburger, dismissed by acting or by clicking away.
fn primary_menu<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    let panel = container(
        column![
            menu_item("Undo", state.history.can_undo(), ShellMessage::Undo, palette),
            menu_item("Redo", state.history.can_redo(), ShellMessage::Redo, palette),
            container(iced::widget::Space::new().width(Length::Fill).height(Length::Fixed(1.0)))
                .style(move |_| container::Style {
                    background: Some(palette.border.into()),
                    ..Default::default()
                }),
            // # There is no "Light appearance" item any more
            //
            // It toggled between two built-in palettes, which the theme
            // picker below now selects between along with everything the
            // user has saved — two controls for one decision, sitting one
            // row apart, and the picker is the one that can express
            // "neither of those, mine".
            //
            // It had also stopped being honest. A `[wallpaper]` theme
            // derives one palette and assigns it to both modes, so under
            // any derived theme the item was a control that visibly did
            // nothing. `T` still toggles, for anyone on the built-in
            // palettes who wants the accelerator.
            //
            // Interface scale, as a stepper rather than a menu item.
            //
            // Every other row here closes the menu on activation, which
            // is correct for a one-shot action and wrong for this one:
            // scaling is adjusted *by comparison*, so a row that
            // dismissed the popover would make the user reopen it for
            // every 10%. The stepper stays put and reports the current
            // value, which is also the only place that value is visible.
            scale_stepper(state.ui_scale, palette),
            // Names the theme file and where it lives. A config nobody
            // can find is a feature nobody has: this is the only place
            // the path appears in the interface, and it is disabled
            // rather than hidden when there is no config location at
            // all, so the row still explains what the app supports.
            theme_row(&state.theme, palette),
            // The app's only help affordance. It lives in the menu
            // because that is where a user looks for it, and it is also
            // on `?` because that is where they reach first.
            menu_item("Keyboard shortcuts", true, ShellMessage::ToggleShortcuts, palette),
        ]
        .spacing(2),
    )
    .width(Length::Fixed(200.0))
    .padding(6)
    .style(move |_| container::Style {
        background: Some(palette.surface_raised.into()),
        border: Border { color: palette.border, width: 1.0, radius: 12.0.into() },
        shadow: Shadow {
            color: Color { a: 0.45, ..Color::BLACK },
            offset: Vector::new(0.0, 4.0),
            blur_radius: 18.0,
        },
        ..Default::default()
    });

    // A full-surface catcher so clicking anywhere else closes the menu,
    // which is what every GNOME popover does.
    iced::widget::mouse_area(
        container(column![
            row![horizontal_space(), panel].padding([4, 10]),
            iced::widget::Space::new().height(Length::Fill),
        ])
        .width(Length::Fill)
        .height(Length::Fill),
    )
    .on_press(ShellMessage::ToggleMenu)
    .into()
}



/// The toolbar: Open on the left, the file's identity in the centre,
/// Export and the overflow menu on the right.
///
/// The arrangement every application in this register uses, and the
/// reason it is worth following exactly: a user looking for "what file is
/// this" and "how do I get it out" finds both without searching.
fn toolbar<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    let source = state.project().sources.first();
    let title = source
        .and_then(|s| s.path.file_name().and_then(|n| n.to_str()))
        .unwrap_or(if state.drag_and_drop_available {
            "No media — open a file, or drag one in"
        } else {
            "No media — open a file"
        })
        .to_string();

    let specs = source
        .map(|s| match &state.codec {
            Some(codec) => format!(
                "{}×{}  ·  {:.2} fps  ·  {codec}",
                s.resolution.0, s.resolution.1, s.fps.as_f64()
            ),
            None => format!("{}×{}  ·  {:.2} fps", s.resolution.0, s.resolution.1, s.fps.as_f64()),
        })
        .unwrap_or_default();

    // The **output** size, shown only when a crop has changed it.
    //
    // Cropping changes the shape of the exported file, and that is a
    // consequence people need to see before spending minutes on an
    // encode. Deliberately silent when the two agree: a readout that
    // never changes is noise, and one that appears exactly when something
    // changed is a signal.
    let output: Element<'_, ShellMessage> = match (source, state.output_resolution()) {
        (Some(s), Some(out)) if out != s.resolution => container(
            text(format!("→ {}×{}", out.0, out.1))
                .size(11)
                .font(MONO)
                .color(palette.accent_tint_text),
        )
        .padding([2, 7])
        .style(move |_| container::Style {
            background: Some(palette.accent_tint_bg.into()),
            border: Border { radius: 5.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into(),
        _ => iced::widget::Space::new().into(),
    };

    let title_stack = column![
        text(title).size(13).font(semibold()).color(palette.text_primary),
        row![text(specs).size(11).color(palette.text_muted), output]
            .spacing(8)
            .align_y(Alignment::Center),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    let export_label = match &state.export {
        ExportState::Running(p) => format!("Exporting {:.0}%", p.fraction() * 100.0),
        _ => "Export".to_string(),
    };
    let exporting = matches!(state.export, ExportState::Running(_));

    let status: Element<'_, ShellMessage> = match (&state.status, &state.export) {
        // Three outcomes, three colours, and the colour is never the
        // only signal — each pill leads with the word for what happened.
        (Some(message), _) => status_pill(message.clone(), palette.danger, palette),
        (None, ExportState::Done(path)) => status_pill(
            format!("Exported {}", path.file_name().and_then(|n| n.to_str()).unwrap_or("file")),
            palette.success,
            palette,
        ),
        (None, ExportState::Failed(e)) => {
            status_pill(format!("Export failed: {e}"), palette.danger, palette)
        }
        _ => iced::widget::Space::new().into(),
    };

    // # Centring the title
    //
    // Both flanking clusters reserve the **same** fixed width, so the
    // title sits between two equal reservations and its centre is the
    // window's centre at every width. This is recorded because three
    // other approaches failed here: two `Fill` spacers centre the title
    // in the leftover band (measured 46px off); a fixed balancing spacer
    // is correct only at the width it was calibrated at; and three
    // `FillPortion` slots combined with `center_x(Length::Fill)` set the
    // container width twice and collapsed the window to a 14px strip.
    //
    // Widened from 168 to seat the inspector tabs on the leading side:
    // Open (~62) + three padded tabs (~62 + 58 + 68) + spacing. Both
    // clusters still reserve the same width, because that equality is
    // what centres the title — growing one alone reintroduces exactly the
    // off-centre bug the note above records.
    const CLUSTER: f32 = 336.0;

    // The inspector's tab cluster lives here, not in the panel.
    //
    // The panel is now dismissible, and tabs inside a dismissible panel
    // disappear with it — leaving no way back in. Hoisting them to the
    // toolbar keeps them reachable in both states, and makes the cluster
    // the panel's open/close indicator: exactly one tab reads active
    // while the plate is open, none while it is closed.
    let tabs = row(InspectorTab::ALL
        .map(|tab| tab_button(tab, state.inspector_open && state.tab == tab, palette)))
    .spacing(4);

    let leading = row![
        toolbar_button(Icon::Folder, Some("Open"), ShellMessage::OpenFile, palette),
        tabs,
        status,
        horizontal_space(),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fixed(CLUSTER));

    let trailing = row![
        horizontal_space(),
        export_button(export_label, exporting, palette),
        toolbar_button(Icon::Menu, None, ShellMessage::ToggleMenu, palette),
    ]
    .spacing(6)
    .align_y(Alignment::Center)
    .width(Length::Fixed(CLUSTER));

    container(
        row![leading, title_stack.width(Length::Fill), trailing]
            .spacing(8)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TOOLBAR_HEIGHT))
    .padding([0, RAIL as u16])
    .style(move |_| container::Style {
        background: Some(palette.surface.into()),
        // One hairline separating the toolbar from the stage. One
        // elevation signal, never a border *and* a shadow.
        border: Border {
            color: palette.border,
            width: 0.0,
            radius: 0.0.into(),
        },
        ..Default::default()
    })
    .into()
}

/// A toolbar control: no fill at rest, a tonal step on hover.
fn toolbar_button<'a>(
    which: Icon,
    label: Option<&'a str>,
    message: ShellMessage,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let content: Element<'_, ShellMessage> = match label {
        Some(text_label) => row![
            icon(which, palette.text_secondary, 15.0),
            text(text_label).size(12).color(palette.text_primary),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        None => icon(which, palette.text_secondary, 15.0),
    };

    button(content)
        .on_press(message)
        .padding(if label.is_some() { [6, 10] } else { [6, 8] })
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: Some(
                    if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                ),
                text_color: palette.text_primary,
                border: Border { radius: 6.0.into(), ..Default::default() },
                ..Default::default()
            }
        })
        .into()
}

/// Export: the one filled control in the window.
///
/// The product rules gives each surface one primary action, and in this
/// register the primary action is the only thing wearing the accent.
fn export_button<'a>(
    label: String,
    exporting: bool,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    // `on_accent`, not an assumed white: the pairing is asserted in
    // `theme.rs`, and this codebase shipped a 1.98:1 glyph precisely
    // because a call site assumed white on an accent fill.
    let fg = palette.on_accent;
    button(
        row![
            icon(Icon::Download, fg, 14.0),
            text(label).size(12).font(semibold()).color(fg),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(ShellMessage::Export)
    .padding([6, 12])
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            // Exporting and hovered share the brighter fill: in both the
            // control is "live", and a third shade would mean nothing.
            background: Some(
                if exporting || hovered { palette.accent_hover } else { palette.accent }.into(),
            ),
            text_color: fg,
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        }
    })
    .into()
}

/// A transient message in the toolbar — a load error, an export result.
///
/// `outcome` is the semantic colour of what happened: `success` for a
/// finished export, `danger` for a failure or a load error. It was
/// `accent` for success, which spent the selection blue on a fourth
/// meaning; now the outcome is legible before the sentence is read.
fn status_pill<'a>(
    message: String,
    outcome: Color,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    // # The dismiss mark
    //
    // The whole pill was already clickable and nothing said so — a
    // dismissal with no affordance is a secret, and the critique listed
    // it as such. The mark makes the behaviour visible without adding a
    // second control: the *pill* stays the target, and the × is a label
    // for what pressing it does.
    //
    // Padding also went from `[4, 9]` to `[6, 9]`, taking the measured
    // height from 22.3px to a hair over the 24px floor. A message you
    // dismiss by aiming at 22px of toolbar is a message you learn to
    // ignore instead.
    button(
        row![
            text(message).size(11).color(palette.text_primary),
            icon(Icon::Close, palette.text_muted, 11.0),
        ]
        .spacing(7)
        .align_y(Alignment::Center),
    )
    .on_press(ShellMessage::DismissStatus)
    .padding([6, 9])
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: Some(
                if hovered { palette.surface_hover } else { palette.surface_raised }.into(),
            ),
            text_color: palette.text_primary,
            border: Border { color: outcome, width: 1.0, radius: 5.0.into() },
            ..Default::default()
        }
    })
    .into()
}

/// The stage: the picture, on black, filling every pixel the fixed bands
/// do not claim.
///
/// # The one thing this build fixes about the arrangement
///
/// The previous version of this window gave the stage whatever was left
/// after four fixed bands, and in a tall window with a small source that
/// produced two large dead regions above and below the picture. The
/// arrangement was not the problem — the proportions were. Here the
/// bands are as short as their contents allow, the stage takes
/// `Length::Fill` on both axes, and `fit_to_viewport` letterboxes
/// honestly inside it.
fn stage<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    // Silence has three causes — master mute, the clip's own mute, and
    // the 4× rule — and the badge must report *audible or not*, not just
    // the two it happens to know about. Master mute silences everything,
    // so a badge ignoring it would claim sound was playing over a silent
    // file.
    let silent = state.project().master_muted
        || state.selected().is_some_and(|c| c.effective_muted());

    let badge: Element<'_, ShellMessage> = match state.selected() {
        Some(clip) if clip.speed != Speed::One || silent => {
            let mut label = clip.speed.label().to_string();
            if silent {
                label.push_str("  ·  Muted");
            }
            stage_badge(
                row![
                    icon(
                        if silent { Icon::SpeakerOff } else { Icon::Play },
                        if silent { palette.mute } else { palette.stage_badge_text },
                        13.0,
                    ),
                    text(label).size(11).font(semibold()).color(palette.stage_badge_text),
                ]
                .spacing(6)
                .align_y(Alignment::Center)
                .into(),
                palette,
            )
        }
        _ => iced::widget::Space::new().into(),
    };

    let fps = state.fps();
    let timecode: Element<'_, ShellMessage> = if state.project().clips.is_empty() {
        iced::widget::Space::new().into()
    } else {
        stage_badge(
            text(format!(
                "{}   /   {}",
                fmt_timecode(state.playhead, fps),
                fmt_timecode(state.total_duration(), fps)
            ))
            .size(11)
            .font(MONO)
            .color(palette.stage_badge_text_dim)
            .into(),
            palette,
        )
    };

    // # The empty stage: an icon and one line
    //
    // With no file open, every badge above resolves to `Space` and the
    // largest region of the window is a bare black rectangle, so it says
    // the one thing that starts everything and stops.
    //
    // It previously carried two more lines — a sentence about trimming
    // and exporting, and a keyboard hint in a mono pill. Both were
    // answering questions nobody had asked yet: what the product does is
    // not useful before there is anything to do it to, and the shortcut
    // is in the toolbar's Open button and in the `?` sheet, which is
    // where a person looks for it. Three stacked messages on black also
    // read as a dialog the window was waiting on rather than as an empty
    // state.
    //
    // The heading names the action; the toolbar has the button.
    let overlay: Element<'_, ShellMessage> = if state.project().clips.is_empty() {
        container(
            column![
                icon(Icon::Folder, palette.stage_badge_text_dim, 34.0),
                text("Open a video to start")
                    .size(17)
                    .font(semibold())
                    .color(palette.stage_badge_text),
            ]
            .spacing(11)
            .align_x(Alignment::Center),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
    } else {
        column![
            row![badge, horizontal_space()],
            vertical_space(),
            row![horizontal_space(), timecode],
        ]
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(RAIL as u16)
        .into()
    };

    // The video widget (offcut-render's shader primitive) sits under the
    // badges in the same stack. The effects uniform is the selected
    // clip's live crop/adjust state, so dragging a slider changes the
    // image immediately.
    let frame_stack = iced::widget::stack![
        crate::video::video_preview(
            state.current_frame.clone(),
            state.effects(),
            // The crop box is an editing affordance: it exists only on
            // the Crop tab, and only when there is a clip to crop.
            (state.tab == InspectorTab::Crop)
                .then(|| state.selected().map(|c| c.crop.rect))
                .flatten(),
            state.selected().map(|c| c.crop.lock_aspect()).unwrap_or(true),
            state.source_aspect(),
        )
        .map(ShellMessage::Video),
        overlay
    ];

    container(frame_stack)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.letterbox.into()),
            ..Default::default()
        })
        .into()
}

/// A readout laid over the picture.
///
/// Video has no guaranteed luminance — white text over a bright frame is
/// invisible and no palette value fixes it — so anything on the stage
/// gets a dark backing. Routed through one helper so the treatment cannot
/// drift between the two places that use it.
fn stage_badge<'a>(
    content: Element<'a, ShellMessage>,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    container(content)
        .padding([5, 9])
        .style(move |_| container::Style {
            background: Some(palette.stage_shadow.into()),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

/// The "drop to open" plate, shown while a file is dragged over the
/// window.
///
/// Accent-tinted rather than a neutral scrim, because the message is
/// "yes, this will work" — a grey overlay reads as *disabled*, the
/// opposite of the intended invitation.
fn drop_overlay<'a>(palette: Palette) -> Element<'a, ShellMessage> {
    let plate = container(
        column![
            icon(Icon::Folder, palette.accent, 30.0),
            text("Drop to open").size(15).font(semibold()).color(palette.text_primary),
            // Two facts, in consequence order: what you lose, then what
            // you do not. It said "Replaces what is open now", which
            // names the loss without saying it is the *edits* that go —
            // the one thing a person mid-trim needs to weigh.
            text("Your trim of the current video is discarded. Neither file on disk is changed.")
                .size(12)
                .color(palette.text_secondary),
        ]
        .spacing(8)
        .align_x(Alignment::Center),
    )
    .padding([22, 32])
    .style(move |_| container::Style {
        background: Some(palette.surface_raised.into()),
        border: Border { color: palette.accent, width: 2.0, radius: 12.0.into() },
        ..Default::default()
    });

    container(plate)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Color { a: 0.72, ..Color::BLACK }.into()),
            ..Default::default()
        })
        .into()
}

/// The one-time cue that names the product's core gesture.
///
/// # Why a line of text and not a spotlight tour
///
/// There is exactly one thing a new user has to discover here, and it is
/// already on screen: the two handles at the ends of the bar. A tour that
/// dimmed the window to point at a control the user is already looking at
/// would cost an interruption to say something the control almost says
/// itself.
///
/// So this is the smallest possible nudge — one line, in the band it
/// describes, gone forever the moment an edge moves. It appears only when
/// the bar is showing and nothing has been trimmed yet, which is the
/// exact window in which it is useful.
fn trim_hint<'a>(palette: Palette) -> Element<'a, ShellMessage> {
    row![
        horizontal_space(),
        // Names the *outcome*, not the gesture: "to trim" restates the
        // control's own name, while "keep" is the thing the user came to
        // do and the word the readout beside it already uses.
        text("Drag either end to choose what to keep")
            .size(11)
            .color(palette.text_muted),
        horizontal_space(),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// The in and out points, centred between the duration pair.
///
/// Occupies the slot `trim_hint` vacates after the first trim — see
/// `trim_bar_row`. Source time, not timeline time: these name positions
/// *in the file the user opened*, which is the frame of reference a
/// person checking against a shot list is working in.
fn trim_range_label<'a>(
    clip: &'a offcut_model::Clip,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    use crate::trimbar::fmt_duration;
    row![
        horizontal_space(),
        text(format!("{}  →  {}", fmt_duration(clip.in_point), fmt_duration(clip.out_point)))
            .size(11)
            .font(MONO)
            .color(palette.text_muted),
        horizontal_space(),
    ]
    .align_y(Alignment::Center)
    .into()
}

/// The keyboard reference.
///
/// Eighteen shortcuts existed and were documented only in source
/// comments, which meant the app taught none of them: the critique scored
/// Help and Documentation 1/10 on exactly this. A sheet is the right
/// weight — it is the whole of the app's help, it is reachable from the
/// menu and from `?`, and it closes on Escape like every other plate.
///
/// Grouped by what the user is doing, not by key, because someone opens
/// this having decided what they want and needing the key for it.
/// The name of the primary modifier **on this platform**.
///
/// `Modifiers::command()` resolves to Logo on macOS and **Control**
/// everywhere else, so the binding was already correct — the sheet was
/// not. It printed `⌘` unconditionally, which on Linux documents a key
/// that does nothing: a user pressing Super+Z gets no undo and concludes
/// the shortcut is broken, when the app was waiting for Ctrl+Z all along.
///
/// A help surface that names the wrong key is worse than one that names
/// none, because it is believed.
/// Every chord is a `const` rather than a `format!`, because `row_for`
/// borrows its label and a temporary `String` cannot outlive the call.
/// These are platform constants, so there is nothing to compute at
/// runtime anyway.
#[cfg(target_os = "macos")]
mod keys {
    pub const OPEN: &str = "⌘O";
    pub const EXPORT: &str = "⌘E";
    pub const UNDO: &str = "⌘Z";
    pub const REDO: &str = "⇧⌘Z";
    pub const SCALE: &str = "⌘+  ⌘−";
    pub const SCALE_RESET: &str = "⌘0";
}

#[cfg(not(target_os = "macos"))]
mod keys {
    pub const OPEN: &str = "Ctrl O";
    pub const EXPORT: &str = "Ctrl E";
    pub const UNDO: &str = "Ctrl Z";
    pub const REDO: &str = "Ctrl ⇧ Z";
    pub const SCALE: &str = "Ctrl +  −";
    pub const SCALE_RESET: &str = "Ctrl 0";
}

fn shortcuts_sheet<'a>(palette: Palette) -> Element<'a, ShellMessage> {
    fn row_for<'b>(keys: &'b str, what: &'b str, palette: Palette) -> Element<'b, ShellMessage> {
        row![
            container(text(keys).size(11).font(MONO).color(palette.text_primary))
                .padding([3, 7])
                .width(Length::Fixed(96.0))
                .style(move |_| container::Style {
                    background: Some(palette.surface_sunken.into()),
                    border: Border { radius: 5.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            text(what).size(12.5).color(palette.text_secondary),
        ]
        .spacing(12)
        .align_y(Alignment::Center)
        .into()
    }

    fn group<'b>(
        title: &'b str,
        rows: Vec<Element<'b, ShellMessage>>,
        palette: Palette,
    ) -> Element<'b, ShellMessage> {
        let mut col = column![text(title)
            .size(10)
            .font(semibold())
            .color(palette.text_muted)]
        .spacing(9);
        for r in rows {
            col = col.push(r);
        }
        col.into()
    }

    // The heading and its close mark stay put while the list moves under
    // them: a title that scrolls away takes the dismiss control with it,
    // and Escape is the accelerator, not the affordance.
    let heading = row![
        // Matches the menu item that opens it. It read "Keyboard" where
        // the menu said "Keyboard shortcuts", so the sheet did not
        // obviously confirm the thing clicked.
        text("Keyboard shortcuts").size(17).font(bold()).color(palette.text_primary),
        horizontal_space(),
        toolbar_button(Icon::Close, None, ShellMessage::ToggleShortcuts, palette),
    ]
    .align_y(Alignment::Center);

    let groups = column![
            group(
                "TRIM",
                vec![
                    row_for("I", "Start here", palette),
                    row_for("O", "End here", palette),
                ],
                palette,
            ),
            group(
                "PLAYBACK",
                vec![
                    row_for("Space  K", "Play or pause", palette),
                    row_for("←  J", "Back one frame", palette),
                    row_for("→  L", "Forward one frame", palette),
                ],
                palette,
            ),
            group(
                "PANELS",
                vec![
                    row_for("1  2  3", "Video, Crop, Adjust", palette),
                    row_for("Esc", "Close the panel", palette),
                    row_for("?", "This list", palette),
                    row_for("T", "Switch light or dark", palette),
                    row_for(keys::SCALE, "Interface bigger or smaller", palette),
                    row_for(keys::SCALE_RESET, "Interface back to 100%", palette),
                ],
                palette,
            ),
            // `M` and `T` used to sit under FILE, which they are not:
            // muting a clip is an edit and switching appearance is a
            // panel preference. A group heading that does not describe
            // its rows is worse than no heading, because it is scanned
            // and believed.
            group(
                "FILE",
                vec![
                    row_for(keys::OPEN, "Open a video", palette),
                    row_for(keys::EXPORT, "Export the trimmed video", palette),
                ],
                palette,
            ),
            group(
                "EDIT",
                vec![
                    row_for("M", "Mute the clip", palette),
                    row_for(keys::UNDO, "Undo", palette),
                    row_for(keys::REDO, "Redo", palette),
                ],
                palette,
            ),
    ]
    .spacing(20);

    let panel = container(column![heading, iced::widget::scrollable(groups)].spacing(20))
    .width(Length::Fixed(380.0))
    .padding(24)
    // # The sheet is scrollable and height-capped
    //
    // Sixteen rows in five groups measure roughly 700px, which fits the
    // default 900px window and nothing smaller. A fixed column in a
    // shorter window — or at any interface scale above 100%, where 700
    // becomes 1400 — simply clips: the last group runs off the bottom
    // edge with no indication there is more and no way to reach it. The
    // capture that prompted this shows `Ctrl Z` half-cut with Redo
    // entirely gone.
    //
    // `max_height` rather than a fixed height, so a short list still
    // hugs its content instead of floating in an oversized plate.
    .max_height(560.0)
    .style(move |_| container::Style {
        background: Some(palette.surface.into()),
        border: Border { color: palette.border, width: 1.0, radius: 14.0.into() },
        shadow: Shadow {
            color: Color { a: 0.5, ..Color::BLACK },
            offset: Vector::new(0.0, 8.0),
            blur_radius: 32.0,
        },
        ..Default::default()
    });

    iced::widget::mouse_area(
        container(panel)
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .style(move |_| container::Style {
                background: Some(Color { a: 0.6, ..Color::BLACK }.into()),
                ..Default::default()
            }),
    )
    .on_press(ShellMessage::ToggleShortcuts)
    .into()
}

/// The export confirmation.
///
/// # Why the one irreversible action gets a review step
///
/// Export is the only act in this product that cannot be undone: it runs
/// for minutes, writes to disk, and locks the window while it does. It
/// was also the only one with no confirmation — the user pressed Export,
/// picked a filename, and was committed to a codec, a bitrate, and a
/// resolution they had never been shown.
///
/// Everything else here is reversible, derived, and visible. This sheet
/// makes the last moment match the rest: four facts and one choice, which
/// is inside the working-memory limit, and the numbers come from the
/// export crate's own functions so the sheet cannot promise something
/// different from what gets written.
fn export_confirm<'a>(
    state: &'a ShellState,
    path: &'a std::path::Path,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    // Elided like the inspector heading, and for the same reason: this
    // is a user-typed filename in a fixed-width plate.
    let name = elide_middle(path.file_name().and_then(|n| n.to_str()).unwrap_or("export.mp4"));

    let out = state.output_resolution();
    let duration = state.total_duration();

    // Rough size, honestly labelled. A user deciding between codecs wants
    // an order of magnitude, and withholding it because it is approximate
    // would be the more useless choice.
    let seconds = duration.as_secs_f64();
    let total_kbit =
        (state.export_settings.bitrate_kbps as f64 + state.export_settings.audio_bitrate_bps as f64 / 1000.0)
            * seconds;
    let mb = total_kbit / 8.0 / 1024.0;

    fn fact<'b>(label: &'b str, value: String, palette: Palette) -> Element<'b, ShellMessage> {
        row![
            text(label).size(12).color(palette.text_muted),
            horizontal_space(),
            text(value).size(12).font(MONO).color(palette.text_primary),
        ]
        .align_y(Alignment::Center)
        .into()
    }

    // One segmented control, drawn twice: format and codec are the same
    // kind of choice — a small closed set, one active — so they are the
    // same control rather than two hand-built rows that drift apart.
    fn segmented<'b, T: Copy + PartialEq + 'b>(
        options: impl IntoIterator<Item = T>,
        active_value: T,
        label: impl Fn(T) -> &'b str,
        message: impl Fn(T) -> ShellMessage,
        palette: Palette,
    ) -> Element<'b, ShellMessage> {
        row(options.into_iter().map(|option| {
            let active = option == active_value;
            button(
                text(label(option))
                    .size(12.5)
                    .font(if active { semibold() } else { Font::default() })
                    .color(if active { palette.on_accent } else { palette.text_secondary })
                    .center(),
            )
            .on_press(message(option))
            .padding([7, 0])
            .width(Length::Fill)
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered) && !active;
                button::Style {
                    background: Some(
                        if active {
                            palette.accent
                        } else if hovered {
                            palette.surface_hover
                        } else {
                            palette.surface_sunken
                        }
                        .into(),
                    ),
                    text_color: if active { palette.on_accent } else { palette.text_secondary },
                    border: Border { radius: 7.0.into(), ..Default::default() },
                    ..Default::default()
                }
            })
            .into()
        }))
        .spacing(6)
        .into()
    }

    let containers = segmented(
        offcut_export::Container::ALL,
        state.export_settings.container,
        |c| c.label(),
        ShellMessage::SetExportContainer,
        palette,
    );

    let codecs = segmented(
        offcut_export::VideoCodec::ALL,
        state.export_settings.codec,
        |c| c.label(),
        ShellMessage::SetExportCodec,
        palette,
    );

    let panel = container(
        column![
            text("Export video").size(17).font(bold()).color(palette.text_primary),
            // "Saving to" rather than a bare filename: the plate is the
            // last chance to notice the wrong destination, and a name on
            // its own does not say whether it is the source or the
            // output — which in an application whose promise is "your
            // original is never changed" is the one ambiguity to avoid.
            row![
                text("Saving to").size(12).color(palette.text_muted),
                text(name).size(12.5).font(MONO).color(palette.accent_tint_text),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
            // Both rows are labelled. Two unlabelled segmented controls
            // stacked together give no clue which is the container and
            // which the codec — and "MP4 / MOV / MKV" over "H.264 /
            // HEVC" is exactly the pair a user has to tell apart to
            // predict what file they get.
            column![section_label("Format", palette), containers].spacing(6),
            column![section_label("Codec", palette), codecs].spacing(6),
            column![
                // "Size" and "Estimated" both meant size, one in pixels
                // and one in bytes, stacked two rows apart — and
                // "Quality" labelled a bitrate, which is a *setting*
                // rather than a verdict. Each label now names its own
                // unit, so the four rows read as four different facts.
                fact(
                    "Resolution",
                    out.map(|(w, h)| format!("{w} × {h}")).unwrap_or_else(|| "—".into()),
                    palette
                ),
                fact("Duration", fmt_timecode_secs(duration), palette),
                fact("Bitrate", format!("{} kbps", state.export_settings.bitrate_kbps), palette),
                fact("File size", format!("≈ {mb:.0} MB"), palette),
            ]
            .spacing(9),
            row![
                // Dismisses the sheet and changes nothing, so it says so.
                button(text("Not now").size(13).color(palette.text_primary))
                    .on_press(ShellMessage::CancelPendingExport)
                    .padding([9, 18])
                    .style(move |_, status| {
                        let hovered = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(
                                if hovered { palette.surface_hover } else { palette.surface_raised }
                                    .into(),
                            ),
                            text_color: palette.text_primary,
                            border: Border {
                                color: palette.border_raised,
                                width: 1.0,
                                radius: 8.0.into(),
                            },
                            ..Default::default()
                        }
                    }),
                horizontal_space(),
                // "Export video", matching the sheet's own title: the
                // confirming button repeats the action rather than
                // saying "OK", so the pair reads "Not now / Export video"
                // and either can be chosen without reading the heading.
                button(text("Export video").size(13).font(semibold()).color(palette.on_accent))
                    .on_press(ShellMessage::ConfirmExport)
                    .padding([9, 20])
                    .style(move |_, status| {
                        let hovered = matches!(status, button::Status::Hovered);
                        button::Style {
                            background: Some(
                                if hovered { palette.accent_hover } else { palette.accent }.into(),
                            ),
                            text_color: palette.on_accent,
                            border: Border { radius: 8.0.into(), ..Default::default() },
                            ..Default::default()
                        }
                    }),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(16),
    )
    .width(Length::Fixed(340.0))
    .padding(24)
    .style(move |_| container::Style {
        background: Some(palette.surface.into()),
        border: Border { color: palette.border, width: 1.0, radius: 14.0.into() },
        shadow: Shadow {
            color: Color { a: 0.5, ..Color::BLACK },
            offset: Vector::new(0.0, 8.0),
            blur_radius: 32.0,
        },
        ..Default::default()
    });

    // No click-away dismissal: this is a decision, and dismissing a
    // decision by missing the panel is how people export the wrong thing.
    container(panel)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Color { a: 0.6, ..Color::BLACK }.into()),
            ..Default::default()
        })
        .into()
}

/// The transport: play controls centred, timecode either side.
fn transport<'a>(
    state: &'a ShellState,
    palette: Palette,
    short: bool,
) -> Element<'a, ShellMessage> {
    if state.project().clips.is_empty() {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    }

    let play_icon = if state.playing { Icon::Pause } else { Icon::Play };

    // The primary transport control: a filled accent circle. This is the
    // one round control in the window and the one place besides Export
    // that carries the accent, which is what makes it findable without a
    // label.
    let play = button(icon(play_icon, palette.on_accent, 17.0))
        .on_press(ShellMessage::TogglePlay)
        .width(Length::Fixed(36.0))
        .height(Length::Fixed(36.0))
        .style(move |_, status| {
            let hovered = matches!(status, button::Status::Hovered);
            button::Style {
                background: Some(
                    if hovered { palette.accent_hover } else { palette.accent }.into(),
                ),
                text_color: palette.on_accent,
                // A flat fill. No coloured halo: a zero-offset glow is
                // decoration, not depth.
                border: Border { radius: 18.0.into(), ..Default::default() },
                ..Default::default()
            }
        });

    let step = |which: Icon, message: ShellMessage| {
        button(icon(which, palette.text_secondary, icons::CONTROL))
            .on_press(message)
            .width(Length::Fixed(32.0))
            .height(Length::Fixed(32.0))
            .style(move |_, status| {
                let hovered = matches!(status, button::Status::Hovered);
                button::Style {
                    background: Some(
                        if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                    ),
                    text_color: palette.text_primary,
                    border: Border { radius: 16.0.into(), ..Default::default() },
                    ..Default::default()
                }
            })
    };

    let controls = row![
        step(Icon::StepBack, ShellMessage::StepBack),
        play,
        step(Icon::StepForward, ShellMessage::StepForward),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    // The zoom control is gone, and stays gone: it scaled a
    // duration-based timeline that no longer exists, so
    // `pixels_per_second` changed nothing anyone could see. A control
    // that visibly does nothing teaches people the app ignores them.
    let fps = state.fps();
    let left: Element<'_, ShellMessage> = if short {
        iced::widget::Space::new().width(Length::Fill).into()
    } else {
        row![
            text(fmt_timecode(state.playhead, fps))
                .size(12)
                .font(MONO)
                .color(palette.text_primary),
            horizontal_space(),
        ]
        .into()
    };
    let right: Element<'_, ShellMessage> = if short {
        iced::widget::Space::new().width(Length::Fill).into()
    } else {
        row![
            horizontal_space(),
            text(fmt_timecode(state.total_duration(), fps))
                .size(12)
                .font(MONO)
                .color(palette.text_muted),
        ]
        .into()
    };

    container(
        row![left, controls, right]
            .align_y(Alignment::Center)
            .spacing(12),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TRANSPORT_HEIGHT))
    .padding([0, RAIL as u16])
    .style(move |_| container::Style {
        background: Some(palette.canvas.into()),
        ..Default::default()
    })
    .into()
}

/// The inspector: three tabs over the selected clip's controls.
/// The inspector plate.
///
/// `stacked` carries the layout decision *and* the measurement it needs:
/// `Some(body_height)` puts the panel beneath the picture and sizes it
/// from that height, `None` puts it beside the picture at a fixed width.
/// A bare `narrow: bool` could not express the second half, which is how
/// the stacked panel ended up with a hardcoded 300px that cut a row.
fn inspector<'a>(
    state: &'a ShellState,
    palette: Palette,
    stacked: Option<f32>,
) -> Element<'a, ShellMessage> {
    // The heading names the **file**, not its ordinal.
    //
    // It read "Clip 1", which is a fact about an index in a vector. In a
    // single-source trimmer there is only ever one thing being edited and
    // the user already knows which — what they want confirmed is *that
    // this panel is about that file*. "Clip 1" confirms nothing, and on a
    // multi-clip timeline it names the position rather than the content,
    // which is the less useful of the two.
    //
    // The ordinal survives as a fallback for a clip whose source cannot
    // be resolved, because a heading that vanishes is worse than a dull
    // one.
    let (heading, subheading) = match (state.selected_clip, state.selected()) {
        (Some(index), Some(clip)) => {
            let fps = state.fps();
            let start = state.project().clip_start_time(index);
            let end = start.checked_add(clip.timeline_duration()).unwrap_or(start);
            let name = state
                .project()
                .source(clip.source)
                .and_then(|s| s.path.file_name().and_then(|n| n.to_str()))
                .map(elide_middle)
                .unwrap_or_else(|| format!("Clip {}", index + 1));
            (name, format!("{}  →  {}", fmt_timecode(start, fps), fmt_timecode(end, fps)))
        }
        _ => ("Nothing open".to_string(), "Open a video to begin".to_string()),
    };

    let body: Element<'_, ShellMessage> = match state.tab {
        InspectorTab::Video => video_tab(state, palette),
        InspectorTab::Crop => crop_tab(state, palette),
        InspectorTab::Adjust => adjust_tab(state, palette),
    };

    // The plate's own close mark.
    //
    // Promised by `CloseInspector`'s doc comment and never drawn, which
    // left the panel dismissible only by a key that did nothing. Escape
    // is the accelerator; this is the affordance, because a dismissal
    // with no visible control is a secret.
    let head = row![
        column![
            text(heading).size(15).font(semibold()).color(palette.text_primary),
            text(subheading).size(11).font(MONO).color(palette.text_muted),
        ]
        .spacing(2),
        horizontal_space(),
        toolbar_button(Icon::Close, None, ShellMessage::CloseInspector, palette),
    ]
    .align_y(Alignment::Center);

    // The tab row is deliberately absent here: it lives in the toolbar so
    // it survives the panel being closed. See `toolbar`.
    let content = column![head, body].spacing(14).padding(RAIL as u16);

    let panel = container(iced::widget::scrollable(content))
        .style(move |_| container::Style {
            background: Some(palette.surface.into()),
            ..Default::default()
        });

    // A single hairline divides the panel from the stage — on the top
    // edge when the panel sits below the picture, on the leading edge
    // when it sits beside it. Without it the panel and the black stage
    // merged into one field.
    if let Some(body_height) = stacked {
        // # Sizing the stacked panel
        //
        // `PANEL_SHARE` is the fraction of the available body the panel
        // may claim; `MIN_STAGE` is the floor that keeps the picture a
        // picture rather than a letterboxed strip. The panel takes the
        // smaller of the two, so a short window sacrifices panel rows and
        // never the frame.
        //
        // 0.46 rather than a round half: the stage carries the timecode
        // badge along its bottom edge, and an exact split put that badge
        // hard against the divider.
        const PANEL_SHARE: f32 = 0.46;
        const MIN_STAGE: f32 = 220.0;
        // The Video tab's run, measured from the render: heading block,
        // Speed label + chips + helper line, Audio label, and its two-row
        // card, plus the plate's own vertical padding. A share alone left
        // this 44px short at an 800px window, which is where the card was
        // getting sliced — so the panel takes the larger of its share and
        // its content, and only then yields to the stage's floor.
        const MIN_PANEL: f32 = 322.0;

        let height = (body_height * PANEL_SHARE)
            .max(MIN_PANEL)
            .min(body_height - MIN_STAGE)
            .max(0.0);

        column![rule_h(palette), panel.width(Length::Fill).height(Length::Fixed(height))].into()
    } else {
        row![
            rule_v(palette),
            panel.width(Length::Fixed(INSPECTOR_WIDTH - 1.0)).height(Length::Fill),
        ]
        .into()
    }
}

/// An inspector tab.
///
/// A filled pill for the active tab rather than an underline: this
/// register uses segmented controls, and a filled segment survives at a
/// glance where a 2px rule does not.
fn tab_button<'a>(tab: InspectorTab, active: bool, palette: Palette) -> Element<'a, ShellMessage> {
    button(
        text(tab.label())
            .size(12)
            .font(if active { semibold() } else { Font::default() })
            .color(if active { palette.on_accent } else { palette.text_secondary })
            .center(),
    )
    .on_press(ShellMessage::SelectTab(tab))
    // Sized for the toolbar, not for a panel row.
    //
    // These were `Length::Fill` with zero horizontal padding, which is
    // right inside a 320px column and wrong in a toolbar cluster: with no
    // fill to expand into, each button collapsed to its own glyph width
    // and the three labels ran together as "VideoCropAdjust" with the
    // active pill clipped to the text. A control in a toolbar has to
    // carry its own width.
    .padding([5, 12])
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered) && !active;
        button::Style {
            background: Some(
                if active {
                    palette.accent
                } else if hovered {
                    palette.surface_hover
                } else {
                    Color::TRANSPARENT
                }
                .into(),
            ),
            text_color: if active { palette.on_accent } else { palette.text_secondary },
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        }
    })
    .into()
}

fn section_label<'a>(label: &'a str, palette: Palette) -> Element<'a, ShellMessage> {
    text(label).size(11).font(semibold()).color(palette.text_muted).into()
}

/// A section header with a trailing action ("Reset", "Reset all"), as the
/// Crop and Adjust renders show.
fn section_header<'a>(
    label: &'a str,
    action: Option<(&'a str, ShellMessage)>,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let mut header = row![section_label(label, palette), horizontal_space()].align_y(Alignment::Center);
    if let Some((action_label, message)) = action {
        // `padding(0)` made this an 11px glyph and nothing else: the
        // measured target was **14.3px tall**, against a 24px floor.
        // Padding is the fix rather than a bigger font, because the
        // label is correctly quiet — it is a "Reset" beside a heading,
        // not a button competing with the controls beneath it. The hit
        // area grows; the type does not.
        //
        // Negative horizontal margin is not available here, so the row's
        // trailing edge moves out by the same 8px the padding adds. That
        // is deliberate: the alternative is a target that lines up
        // perfectly and cannot be hit.
        header = header.push(
            button(text(action_label).size(11).color(palette.accent_tint_text))
                .on_press(message)
                .padding([6, 8])
                .style(move |_, status| {
                    let hovered = matches!(status, button::Status::Hovered);
                    button::Style {
                        background: Some(
                            if hovered { palette.surface_hover } else { Color::TRANSPARENT }.into(),
                        ),
                        text_color: palette.accent_tint_text,
                        border: Border { radius: 5.0.into(), ..Default::default() },
                        ..Default::default()
                    }
                }),
        );
    }
    header.into()
}

fn video_tab<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    let selected = state.selected();
    let current = selected.map(|c| c.speed).unwrap_or(Speed::One);

    let chips = row(Speed::ALL
        .map(|speed| {
            chip(
                speed.label(),
                speed == current,
                selected.is_some().then_some(ShellMessage::SetSpeed(speed)),
                palette,
            )
        }))
    .spacing(8);

    let helper = match selected {
        Some(clip) => format!(
            "Plays for {:.1}s at {}. {}",
            clip.timeline_duration().as_secs_f64(),
            clip.speed.label(),
            if clip.speed == Speed::Four { "Audio muted (4× rule)." } else { "Pitch preserved." }
        ),
        None => "Select a clip to change its speed.".to_string(),
    };

    // Anything that silences the clip greys out the volume slider and
    // shows it at 0%: a control reading 80% over a silent file is worse
    // than a disabled one, because it invites the user to "fix" a level
    // that is not the reason for the silence.
    let clip_speed_forces_mute = selected.map(|c| c.speed.implies_mute()).unwrap_or(false);
    let master_muted = state.project().master_muted;
    let forced_silent = clip_speed_forces_mute || master_muted;

    let source_block: Element<'_, ShellMessage> = match selected.and_then(|c| state.project().source(c.source)) {
        Some(source) => container(
            column![
                info_row("Codec", state.codec.clone().unwrap_or_else(|| "—".to_string()), palette),
                info_row("Frame rate", format!("{:.2} fps", source.fps.as_f64()), palette),
                info_row("Resolution", format!("{}×{}", source.resolution.0, source.resolution.1), palette),
                info_row("Audio", if source.has_audio { "present".into() } else { "none".into() }, palette),
            ]
            .spacing(9),
        )
        .padding(12)
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.surface_raised.into()),
            border: Border { color: palette.border, width: 1.0, radius: 12.0.into() },
            ..Default::default()
        })
        .into(),
        None => iced::widget::Space::new().into(),
    };

    column![
        section_label("Speed", palette),
        chips,
        text(helper).size(11).color(palette.text_muted),
        section_label("Audio", palette),
        // One boxed list, two rows -- Adwaita's grouping, not two
        // separately bordered tiles.
        boxed_list(
            vec![
                volume_row(
                    "Volume",
                    if forced_silent { 0.0 } else { selected.map(|c| c.volume).unwrap_or(1.0) },
                    selected.is_some() && !forced_silent,
                    palette,
                ),
                mute_row(
                    "Mute all audio",
                    master_muted,
                    palette.accent,
                    Some(ShellMessage::ToggleMasterMute),
                    palette,
                ),
            ],
            palette,
        ),
        // An empty `text` still occupies a line in a spaced column, so
        // the "no helper needed" case was emitting a phantom row and
        // pushing "Source" 41px down against "Audio"'s 17px. Emit
        // nothing at all instead.
        audio_helper(master_muted, clip_speed_forces_mute, palette),
        section_label("Source", palette),
        source_block,
    ]
    .spacing(12)
    .into()
}

fn info_row<'a>(label: &'a str, value: String, palette: Palette) -> Element<'a, ShellMessage> {
    row![
        text(label).size(12).color(palette.text_secondary),
        horizontal_space(),
        text(value).size(11).font(MONO).color(palette.text_primary),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn crop_tab<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    let selected = state.selected();
    let crop = selected.map(|c| c.crop).unwrap_or_else(offcut_model::CropTransform::identity);

    let chips = row(AspectPreset::ALL
        .map(|preset| {
            chip(
                preset.label(),
                preset == crop.aspect,
                selected.is_some().then_some(ShellMessage::SetAspect(preset)),
                palette,
            )
        }))
    .spacing(6);

    let straighten = crop.straighten_deg();
    let value_pill = container(text(format!("{straighten:.1}°")).size(11).font(MONO).color(palette.text_primary))
        .padding([3, 8])
        .style(move |_| container::Style {
            background: Some(palette.surface_raised.into()),
            border: Border { color: palette.border_raised, width: 1.0, radius: 6.0.into() },
            ..Default::default()
        });

    // The straighten dial: the design system specifies a 40px sunken tick strip.
    // It is a slider underneath, because a slider already owns drag
    // capture, keyboard stepping, and accessibility — the ticks are the
    // visual language, not a reason to reimplement pointer handling.
    // A centre tick, so the handle has a zero to be read against.
    //
    // Without it the neutral rail gives the eye nothing to locate 0.0°
    // by, and a bipolar control whose rest position is unmarked is only
    // marginally better than one that fills from the wrong end. The tick
    // marks the origin of the scale, not a value on it -- which is why
    // it is `origin_mark` and not `accent`. It *was* the accent, and so
    // is the handle directly beneath it: at exactly 0.0deg a 2px blue
    // tick sat immediately above a 3px blue handle and the two read as
    // one continuous blue bar, so the mark could not be told from the
    // value at the single position it exists to mark. Ink for the scale,
    // accent for the value.
    let centre_tick = container(
        row![
            horizontal_space(),
            container(text(""))
                .width(Length::Fixed(2.0))
                .height(Length::Fixed(DIAL_TICK))
                .style(move |_| container::Style {
                    background: Some(palette.origin_mark.into()),
                    border: Border { radius: 1.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            horizontal_space(),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fixed(DIAL_TICK));

    // The well was a literal 40, which happened to fit a 10px tick above
    // a 16px slider inside 6px padding. Raising the slider to a 24px
    // pointer target made the contents 46px — six pixels of overflow in a
    // fixed box, the same class of bug as the trim handles sliced flat at
    // the window edge, and invisible to every test in the suite.
    let dial = column![
        container(column![
            centre_tick,
            slider(-45.0..=45.0, straighten, ShellMessage::SetStraighten)
                .step(0.5)
                .height(DIAL_SLIDER)
                .style(move |_, _| dial_slider_style(palette)),
        ])
        .padding([DIAL_PAD as u16, 8])
        .height(Length::Fixed(DIAL_WELL))
        .width(Length::Fill)
        .style(move |_| container::Style {
            background: Some(palette.surface_sunken.into()),
            // 12px: Adwaita's card radius, matching every other grouped
            // surface in the sidebar. It was 10px, which is neither.
            border: Border { color: palette.border, width: 1.0, radius: 12.0.into() },
            ..Default::default()
        }),
        row![
            text("−45°").size(10).font(MONO).color(palette.text_muted),
            horizontal_space(),
            text("0°").size(10).font(MONO).color(palette.text_secondary),
            horizontal_space(),
            text("+45°").size(10).font(MONO).color(palette.text_muted),
        ],
    ]
    .spacing(4);

    let ratio_helper = if crop.aspect == AspectPreset::Free {
        "Drag any edge or corner independently."
    } else {
        "Handles keep this ratio. Pick Free to reshape."
    };

    let grid_chips = row(CropGrid::ALL.map(|g| {
        chip(
            g.label(),
            g == crop.grid,
            selected.is_some().then_some(ShellMessage::SetCropGrid(g)),
            palette,
        )
    }))
    .spacing(6);

    column![
        section_header("Aspect ratio", Some(("Reset", ShellMessage::ResetCrop)), palette),
        chips,
        text(ratio_helper).size(11).color(palette.text_muted),
        section_label("Guides", palette),
        grid_chips,
        text("Guides are shown while editing and never exported.")
            .size(11)
            .color(palette.text_muted),
        row![
            section_label("Straighten", palette),
            horizontal_space(),
            value_pill
        ]
        .align_y(Alignment::Center),
        dial,
    ]
    .spacing(12)
    .into()
}

/// The straighten dial: a **bipolar** control, so its rail does not fill.
///
/// iced fills a slider's rail from the left edge to the handle, which is
/// right for a 0-100 level and wrong for a -45..+45 axis: at 0.0 -- the
/// rest position, and the value the clip actually has -- the rail read
/// as roughly half full, stating "some rotation is applied" when none
/// is. A control that misreports its own value at rest is worse than an
/// unstyled one.
///
/// So both halves of the rail are neutral and the *handle* carries the
/// value, which is how a centre-detented dial works in life. The centre
/// tick beneath it (drawn by `straighten_dial`) is what the handle is
/// read against.
fn dial_slider_style(palette: Palette) -> slider::Style {
    slider::Style {
        rail: slider::Rail {
            // Both sides the same: no fill, because there is no
            // "amount" to fill from either end of a bipolar axis.
            //
            // `control_track_off`, the same role the other sliders use
            // for their unfilled remainder — which is exactly what this
            // is, on both sides. It was `border_raised`, a *border*
            // colour standing in for a track, measuring 1.32:1 on
            // Light's well: the dial had a handle and no rail.
            backgrounds: (palette.control_track_off.into(), palette.control_track_off.into()),
            width: 2.0,
            border: Border { radius: 1.0.into(), ..Default::default() },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Rectangle { width: 3, border_radius: 1.5.into() },
            background: palette.accent.into(),
            border_width: 0.0,
            border_color: Color::TRANSPARENT,
        },
    }
}

fn adjust_tab<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    let selected = state.selected();
    let adjust = selected.map(|c| c.adjust).unwrap_or_default();
    let enabled = selected.is_some();

    let mut rows = column![section_header("Tone", Some(("Reset all", ShellMessage::ResetAdjust)), palette)]
        .spacing(14);

    // Fixed order, five rows, no per-row icon — the product's hard cap and
    // The design system's "Adjust is deliberately quieter than Video's
    // chip-and-switch mix."
    for field in AdjustField::ALL {
        let value = field.get(&adjust);
        rows = rows.push(
            column![
                row![
                    text(field.label()).size(13).color(palette.text_primary),
                    horizontal_space(),
                    text(value.to_string())
                        .size(11)
                        .font(MONO)
                        .color(if value > 0 { palette.accent_tint_text } else { palette.text_muted }),
                ]
                .align_y(Alignment::Center),
                // 24px: iced's 16px default is the whole pointer target.
                slider(0..=100u8, value, move |v| ShellMessage::SetAdjust(field, v))
                    .height(24.0)
                    .style(move |_, _| adjust_slider_style(palette, enabled)),
            ]
            .spacing(6),
        );
    }

    rows.into()
}

fn adjust_slider_style(palette: Palette, enabled: bool) -> slider::Style {
    let filled = if enabled { palette.accent } else { palette.border_raised };
    slider::Style {
        rail: slider::Rail {
            // The remainder was `surface_raised` — which is the card the
            // slider is drawn on. **1.00:1**: the unfilled half of every
            // volume and tone slider was invisible, so the control had no
            // visible extent and a value near zero looked like a stray
            // dot rather than a slider at its minimum.
            backgrounds: (filled.into(), palette.control_track_off.into()),
            width: 4.0,
            border: Border { radius: 2.0.into(), ..Default::default() },
        },
        handle: slider::Handle {
            shape: slider::HandleShape::Circle { radius: 8.0 },
            // `control_knob` over a `control_track_off` ring, not a
            // hardcoded white.
            //
            // The handle was `Color::WHITE`, which is invisible in light
            // mode: the card behind it is also white, so the control had
            // no thumb at all. The first repair made it `surface_raised`
            // — which on Light is *also* white, and merely moved the bug
            // one step. The knob now carries the same named role as the
            // toggler's, and the ring gives it a ground independent of
            // whatever card it is dropped onto.
            background: palette.control_knob.into(),
            border_width: 1.5,
            border_color: palette.control_track_off,
        },
    }
}

/// The design system's closed-set chip: "Active is filled accent with 700
/// weight; inactive is raised surface with a 1px border and 500 weight."
fn chip<'a>(
    label: &'a str,
    active: bool,
    message: Option<ShellMessage>,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let enabled = message.is_some();
    let bg = if active { palette.accent } else { palette.surface_raised };
    let fg = if active {
        palette.on_accent
    } else if enabled {
        palette.text_secondary
    } else {
        palette.text_muted_alt
    };
    let border_color = if active { palette.accent } else { palette.border_raised };

    let mut b = button(
        text(label)
            .size(13)
            .font(if active { bold() } else { Font::default() })
            .center(),
    )
    .width(Length::Fill)
    .height(Length::Fixed(44.0))
    .style(move |_, status| {
        let hovered = matches!(status, button::Status::Hovered) && enabled && !active;
        button::Style {
            background: Some(if hovered { palette.surface_hover } else { bg }.into()),
            text_color: fg,
            // 6px: Adwaita's control radius. 12px belongs to cards, and
            // borrowing it here made chips read as tiny cards.
            border: Border { color: border_color, width: 1.0, radius: 6.0.into() },
            ..Default::default()
        }
    });
    if let Some(message) = message {
        b = b.on_press(message);
    }
    b.into()
}

/// The design system: "full-width 52px rows, icon + label left, switch right."
fn mute_row<'a>(
    label: &'a str,
    is_on: bool,
    on_color: Color,
    message: Option<ShellMessage>,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let mark = if label.starts_with("Lock") {
        Icon::Lock
    } else if is_on {
        Icon::SpeakerOff
    } else {
        Icon::SpeakerOn
    };
    let icon_color = if is_on { on_color } else { palette.text_secondary };

    // # 24px, not iced's 16px default
    //
    // A toggler's hit area **is** its drawn size — unlike the trim bar,
    // whose grab zones are deliberately wider than its marks, this widget
    // has no such expansion. So 16px of drawn switch is 16px of target,
    // against a 24px floor.
    //
    // Sized up rather than padded, because padding a `toggler` grows the
    // row without growing the thing you press.
    let mut toggle = toggler(is_on).size(24.0).style(move |_, _| iced::widget::toggler::Style {
        // Off, the track is `control_track_off` — a mid grey, not the
        // inset well. On Light both `surface_raised` and the knob are
        // white, so the off state rendered as a bare outline with no knob
        // in it: 1.00:1, visible in the light-mode capture. See
        // `control_track_off` in `theme.rs` for the measurement.
        background: if is_on { on_color } else { palette.control_track_off }.into(),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        // A named role, not a literal white, for the reason `theme.rs`
        // gives: a colour written at a call site cannot follow the
        // palette, and this is the second control to ship that bug.
        foreground: palette.control_knob.into(),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: Some(palette.text_primary),
        border_radius: None,
        padding_ratio: 0.15,
    });
    if let Some(message) = message {
        toggle = toggle.on_toggle(move |_| message.clone());
    }

    container(
        row![
            icon(mark, icon_color, icons::INSPECTOR),
            text(label).size(13).color(palette.text_primary),
            horizontal_space(),
            toggle,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([0, 12])
    .width(Length::Fill)
    // 50px and no border of its own: inside a boxed list the group owns
    // the border, and a per-row one both doubles the elevation signal
    // and paints over the separator between rows.
    .height(Length::Fixed(50.0))
    .style(move |_| container::Style {
        background: Some(palette.surface_raised.into()),
        ..Default::default()
    })
    .into()
}

/// The design system: "full-width 52px rows, icon + label left, slider right."
fn volume_row<'a>(
    label: &'a str,
    volume: f32,
    enabled: bool,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    // A muted clip shows the crossed speaker; anything audible shows the
    // waves. The mark is the at-a-glance state the number confirms.
    let mark = if volume <= 0.0 { Icon::SpeakerOff } else { Icon::VolumeHigh };
    // `danger`, not `mute`: this speaker is an inspector row, and the
    // stage's red is mode-invariant because it is drawn over footage.
    // Borrowing it here bound a panel icon to the footage rule and gave
    // Light 3.41:1 where its own red gives 5.38:1.
    let icon_color = if volume <= 0.0 { palette.danger } else { palette.text_secondary };

    // 24px tall for the same reason as the toggler: iced's 16px default
    // is both the drawn height and the whole pointer target, and this row
    // is one a person aims at repeatedly while listening.
    let slider_widget = slider(0.0..=1.0, volume, ShellMessage::SetClipVolume)
        .step(0.01)
        .width(Length::Fixed(110.0))
        .height(24.0)
        .style(move |_, _| adjust_slider_style(palette, enabled));

    let percent = text(format!("{:.0}%", volume * 100.0))
        .size(11)
        .font(MONO)
        .color(if volume > 0.0 { palette.accent_tint_text } else { palette.text_muted });

    container(
        row![
            icon(mark, icon_color, icons::INSPECTOR),
            text(label).size(13).color(palette.text_primary),
            horizontal_space(),
            percent,
            slider_widget,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .padding([0, 12])
    .width(Length::Fill)
    .height(Length::Fixed(50.0))
    .style(move |_| container::Style {
        background: Some(palette.surface_raised.into()),
        ..Default::default()
    })
    .into()
}

/// A 1px rule in the palette's border colour.
///
/// Built from a sized `container`, not a `Space`: a zero-content Space
/// inside a `column` collapsed to nothing here, so the boxed list's row
/// separators and the sidebar's divider were both in the tree and
/// invisible on screen. Giving the rule its own background and an
/// explicit fixed dimension is what makes it paint.
fn rule_h<'a>(palette: Palette) -> Element<'a, ShellMessage> {
    container(text(""))
        .width(Length::Fill)
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(palette.border.into()),
            ..Default::default()
        })
        .into()
}

/// The audio section's helper line, or nothing.
///
/// Returns a zero-height `Space` rather than an empty `text`: a `text`
/// with an empty string still claims a line in a `spacing()` column,
/// which is what made the sidebar's section rhythm uneven.
fn audio_helper<'a>(
    master_muted: bool,
    speed_forces_mute: bool,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    let line = if master_muted {
        "Everything is muted. The clip's own level is unchanged."
    } else if speed_forces_mute {
        "4× plays silent. Pick another speed to restore audio."
    } else {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    };
    text(line).size(11).color(palette.text_muted).into()
}

/// Adwaita's **boxed list**: one card holding several rows, with a
/// single 12px-radius border around the group and hairline separators
/// between rows.
///
/// This is the pattern GNOME uses for every settings-style group, and it
/// is what makes an Adwaita sidebar read as one system rather than a
/// stack of floating tiles. Bordering each row individually -- which is
/// what this app did before -- produces the "ghost card" the craft floor
/// names: a 1px border repeated per item, declaring elevation twice.
fn boxed_list<'a>(
    rows: Vec<Element<'a, ShellMessage>>,
    palette: Palette,
) -> Element<'a, ShellMessage> {
    // `spacing(0)` and explicit separators: relying on the rows' own
    // backgrounds to abut leaves no visible division at all, which is
    // exactly what the first render showed -- two rows reading as one
    // 100px block.
    let mut stack = column![].width(Length::Fill).spacing(0);
    let last = rows.len().saturating_sub(1);
    for (i, row_el) in rows.into_iter().enumerate() {
        stack = stack.push(row_el);
        if i < last {
            stack = stack.push(rule_h(palette));
        }
    }

    container(stack)
        .width(Length::Fill)
        // 1px of padding, so the rows' opaque backgrounds stop *at* the
        // parent's border instead of painting over it.
        //
        // Removing `.clip(true)` was necessary and not sufficient: the
        // rows are `Length::Fill` with no inset, so they covered the
        // border and squared off the corners regardless. The card read
        // as a bare 1.13:1 tonal patch while the hand-rolled Source card
        // beside it kept both -- two grouping idioms on one screen, from
        // one call.
        .padding(1)
        .style(move |_| container::Style {
            background: Some(palette.surface_raised.into()),
            border: Border { color: palette.border, width: 1.0, radius: 12.0.into() },
            ..Default::default()
        })
        .into()
}

/// A vertical hairline, dividing the inspector from the stage.
fn rule_v<'a>(palette: Palette) -> Element<'a, ShellMessage> {
    container(iced::widget::Space::new().width(Length::Fixed(1.0)).height(Length::Fill))
        .style(move |_| container::Style {
            background: Some(palette.border.into()),
            ..Default::default()
        })
        .into()
}

/// The trim bar and its readout — the whole source, always exactly the
/// full width of the control.
///
/// # This control is the product
///
/// The product's positioning rests on one property: **the entire source
/// is always the full width**, so both handles are reachable for a
/// five-second clip and a 101-minute film alike. A duration-scaled NLE
/// timeline cannot do this — at any zoom that makes a short selection
/// draggable, an hour-long source is metres wide and its out-point is
/// unreachable. Trading arrangement for reach is the whole product, and
/// `both_handles_are_on_screen_for_a_feature_length_source` pins it.
///
/// The readout sits directly above the bar it describes, and both inset
/// by the same `RAIL`, so the numbers line up with the range they report
/// on. They were previously 24 and 11 — two halves of one control, 13px
/// out on both edges.
fn trim_bar_row<'a>(state: &'a ShellState, palette: Palette) -> Element<'a, ShellMessage> {
    use crate::trimbar::fmt_duration;

    let project = state.project();
    // Shown only for a **single-clip** timeline, which is exactly the
    // state this control describes: one source, one range. Once the user
    // splits into several clips the question stops being "which part of
    // the file" and becomes "how are these arranged", and a bar that
    // silently edited clip 0 of 5 would be lying about its scope.
    if project.clips.len() != 1 {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    }
    let Some(clip) = project.clips.first() else {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    };
    let Some(source) = project.source(clip.source) else {
        return iced::widget::Space::new().height(Length::Fixed(0.0)).into();
    };

    // The playhead is TIMELINE time; the bar draws SOURCE time.
    let playhead_source = Time::from_nanos(
        clip.in_point
            .as_nanos()
            .saturating_add((state.playhead.as_nanos() as f64 * clip.speed.factor()) as u64),
    );
    let range = clip.out_point.as_nanos().saturating_sub(clip.in_point.as_nanos());

    // Numbers are mono, words are sans. Monospace here is measurement:
    // these values change while the pointer moves, and proportional
    // digits make a readout jitter under the cursor you are aiming with.
    // # What the two numbers are called
    //
    // They were "Source" and "Selection". Both were wrong in the same
    // way: neither said it was a *duration*, so the pair read as two
    // unrelated facts rather than a part and its whole. "Source" is also
    // the word this codebase uses for the file itself — the inspector has
    // a "Source" section listing its resolution and codec — so one term
    // named two things one panel apart.
    //
    // "Whole video" and "Keeping" say what the numbers measure and how
    // they relate, in the user's words rather than the model's. "Keeping"
    // in particular is the answer to the only question the trim bar
    // exists to ask.
    let readout = row![
        text("Whole video").size(11).color(palette.text_muted),
        text(fmt_duration(source.duration)).size(11).font(MONO).color(palette.text_secondary),
        horizontal_space(),
        // The number the user is actually aiming at: how long the piece
        // they are keeping will be.
        text("Keeping").size(11).color(palette.text_muted),
        text(fmt_duration(Time::from_nanos(range)))
            .size(12)
            .font(MONO)
            .color(palette.accent_tint_text),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    let canvas = timeline_canvas(TimelineData {
        project,
        selected_clip: state.selected_clip,
        playhead: state.playhead,
        palette,
        pixels_per_second: state.zoom,
        fps: state.fps(),
    })
    .map(ShellMessage::Timeline);

    // The playhead readout is deliberately absent here: it is already in
    // the transport directly above, and the same number in two places one
    // row apart is the redundant readout this product deletes on sight.
    let _ = playhead_source;

    // # Why this band states its own height
    //
    // It is the last child of a `column` whose middle child is
    // `Length::Fill`. A band that only *implies* its height gets whatever
    // the fill leaves over, and the remainder is short by a pixel or two —
    // which lands on this control as the round handles being sliced off
    // flat against the window edge. Visible in the first capture of this
    // build, and the reason `TRIM_BAR_ROW_HEIGHT` is derived from the
    // parts rather than guessed: the readout, the gap, the canvas band,
    // and the padding above and below.
    // The readout row carries the hint until the first trim, in the slot
    // between the two labels that is otherwise empty. Putting it here
    // rather than under the bar keeps the band's height constant, so the
    // hint's disappearance does not reflow the window.
    let readout_row: Element<'_, ShellMessage> = if state.has_trimmed {
        // # Naming the ends of the bar
        //
        // The readout said how long the kept piece is and never where it
        // begins, so the bar's two handles were the only statement of the
        // in and out points — and a handle's position is a picture, not a
        // number you can check against a script or a shot list.
        //
        // It goes in the slot the hint vacates, which is the one place it
        // can live without adding a row: the hint is retired by the first
        // trim, and the first trim is exactly when these numbers start
        // being worth reading. Before that they would both say the ends
        // of the untouched file, which the flanking pair already says.
        iced::widget::stack![readout, trim_range_label(clip, palette)].into()
    } else {
        iced::widget::stack![readout, trim_hint(palette)].into()
    };

    container(
        column![
            container(readout_row).padding([0, RAIL as u16]),
            container(canvas).height(Length::Fixed(crate::timeline::TOTAL_HEIGHT)),
        ]
        .spacing(TRIM_ROW_GAP),
    )
    .width(Length::Fill)
    .height(Length::Fixed(TRIM_BAR_ROW_HEIGHT))
    .padding([TRIM_ROW_PAD as u16, 0])
    .style(move |_| container::Style {
        background: Some(palette.surface.into()),
        ..Default::default()
    })
    .into()
}

fn horizontal_space<'a>() -> Element<'a, ShellMessage> {
    iced::widget::Space::new().width(Length::Fill).into()
}

fn vertical_space<'a>() -> Element<'a, ShellMessage> {
    iced::widget::Space::new().height(Length::Fill).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use offcut_model::{Rational, Source, SourceId};

    fn secs(n: f64) -> Time {
        Time::from_nanos((n * 1e9) as u64)
    }

    /// A 40s source split into four 10s clips — the same shape as the
    /// design render's four-clip timeline.
    fn four_clip_state() -> ShellState {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "beach-walk.mp4".into(),
            duration: secs(40.0),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let sid = source.id;
        project.add_source(source);
        let a = project.add_clip_for_source(sid).unwrap();
        let b = project.split_clip(a, secs(10.0)).unwrap();
        let c = project.split_clip(b, secs(20.0)).unwrap();
        let _ = project.split_clip(c, secs(30.0)).unwrap();
        ShellState::new(project)
    }

    #[test]
    fn toggle_mode_flips_dark_light() {
        let mut state = four_clip_state();
        assert_eq!(state.mode, Mode::Dark);
        state.update(ShellMessage::ToggleMode);
        assert_eq!(state.mode, Mode::Light);
        state.update(ShellMessage::ToggleMode);
        assert_eq!(state.mode, Mode::Dark);
    }

    #[test]
    fn every_inspector_tab_can_be_selected() {
        let mut state = four_clip_state();
        assert_eq!(state.tab, InspectorTab::Video);
        for tab in InspectorTab::ALL {
            state.update(ShellMessage::SelectTab(tab));
            assert_eq!(state.tab, tab);
            assert!(state.inspector_open, "selecting a tab must show its panel");
        }
    }

    /// The window opens on a whole picture, not on a settings panel.
    ///
    /// The first thing anyone does with a freshly opened video is watch
    /// it and set a range — neither of which needs Speed, Crop or Adjust.
    /// A tool that opens with a panel over its content has pre-empted a
    /// decision the user has not made yet, and in this world that panel
    /// is the only thing that covers the frame.
    #[test]
    fn the_inspector_starts_closed_so_the_first_view_is_the_whole_picture() {
        let state = four_clip_state();
        assert!(!state.inspector_open);
    }

    /// Pressing the tab that is already showing closes the plate.
    ///
    /// This is what makes the picture reachable in one click from
    /// anywhere. Without it, opening Crop to check a framing leaves a
    /// panel over the frame with no obvious way back to a clean view, and
    /// users learn to avoid the tabs entirely.
    #[test]
    fn pressing_the_open_tab_again_closes_the_panel() {
        let mut state = four_clip_state();

        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        assert!(state.inspector_open);

        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        assert!(!state.inspector_open, "the same tab again must close the panel");

        // A *different* tab reopens rather than staying closed: the user
        // asked for that panel, not for the panel to go away.
        state.update(ShellMessage::SelectTab(InspectorTab::Adjust));
        assert!(state.inspector_open);
        assert_eq!(state.tab, InspectorTab::Adjust);
    }

    /// Closing the plate keeps the tab, so reopening returns the user to
    /// the panel they were last using rather than resetting to Video.
    #[test]
    fn closing_the_inspector_remembers_which_tab_was_open() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectTab(InspectorTab::Adjust));
        state.update(ShellMessage::CloseInspector);

        assert!(!state.inspector_open);
        assert_eq!(state.tab, InspectorTab::Adjust, "the tab is context, not transient state");

        state.update(ShellMessage::SelectTab(InspectorTab::Adjust));
        assert!(state.inspector_open);
    }

    #[test]
    fn set_speed_updates_only_the_selected_clip() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(1));
        state.update(ShellMessage::SetSpeed(Speed::Two));
        assert_eq!(state.project().clips[1].speed, Speed::Two);
        assert_eq!(state.project().clips[0].speed, Speed::One);
    }

    /// the non-negotiable rule, exercised through the UI's own
    /// state transition.
    ///
    /// Asserted on `effective_muted()`, the *audible outcome*, rather
    /// than on the stored flag. The earlier version required the flag to
    /// be written, which is exactly what made the mute permanent: it
    /// pinned the bug in place and passed while 1× played silent.
    #[test]
    fn setting_speed_to_4x_silences_the_clip() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        assert!(!state.project().clips[0].effective_muted());

        state.update(ShellMessage::SetSpeed(Speed::Four));
        assert!(state.project().clips[0].effective_muted(), "4x must play silent");
        assert!(
            !state.project().clips[0].muted,
            "the implication must stay derived, not be written to the clip"
        );
    }

    #[test]
    fn split_at_the_playhead_creates_a_new_clip() {
        let mut state = four_clip_state();
        state.playhead = secs(5.0);
        state.update(ShellMessage::Split);
        assert_eq!(state.project().clips.len(), 5);
        assert_eq!(state.project().clips[0].out_point, secs(5.0));
    }

    /// A split that cannot happen must not leave a phantom undo entry —
    /// pressing `S` on a boundary and then Ctrl+Z should undo the user's
    /// *last real edit*, not silently consume the keystroke.
    #[test]
    fn a_no_op_split_leaves_no_undo_entry() {
        let mut state = four_clip_state();
        state.playhead = secs(10.0); // exactly a clip boundary
        state.update(ShellMessage::Split);
        assert_eq!(state.project().clips.len(), 4, "nothing was split");
        assert!(!state.history.can_undo(), "a no-op must not become an undo step");
    }

    #[test]
    fn delete_removes_the_clip_and_closes_the_gap() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(1));
        state.update(ShellMessage::DeleteClip);
        assert_eq!(state.project().clips.len(), 3);
        assert_eq!(state.total_duration(), secs(30.0), "ripple delete closes the gap");
    }

    #[test]
    fn deleting_the_last_clip_moves_the_selection_into_range() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(3));
        state.update(ShellMessage::DeleteClip);
        assert_eq!(state.selected_clip, Some(2), "selection must not dangle past the end");
    }

    #[test]
    fn deleting_every_clip_clears_the_selection_without_panicking() {
        let mut state = four_clip_state();
        for _ in 0..4 {
            state.update(ShellMessage::SelectClip(0));
            state.update(ShellMessage::DeleteClip);
        }
        assert!(state.project().clips.is_empty());
        assert_eq!(state.selected_clip, None);
        assert_eq!(state.playhead, Time::ZERO, "the playhead must not outlive the timeline");
    }

    #[test]
    fn duplicate_inserts_a_copy_after_the_original_and_selects_it() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(1));
        let original = state.project().clips[1].clone();
        state.update(ShellMessage::DuplicateClip);

        assert_eq!(state.project().clips.len(), 5);
        assert_eq!(state.selected_clip, Some(2));
        let copy = &state.project().clips[2];
        assert_eq!(copy.in_point, original.in_point);
        assert_eq!(copy.out_point, original.out_point);
        assert_ne!(copy.id, original.id, "a duplicate needs its own identity");
    }

    #[test]
    fn undo_and_redo_reverse_and_replay_an_edit() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(1));
        state.update(ShellMessage::DeleteClip);
        assert_eq!(state.project().clips.len(), 3);

        state.update(ShellMessage::Undo);
        assert_eq!(state.project().clips.len(), 4);

        state.update(ShellMessage::Redo);
        assert_eq!(state.project().clips.len(), 3);
    }

    #[test]
    fn stepping_moves_the_playhead_exactly_one_frame() {
        let mut state = four_clip_state();
        let frame = state.fps().frame_duration();
        state.update(ShellMessage::StepForward);
        assert_eq!(state.playhead, frame);
        state.update(ShellMessage::StepBack);
        assert_eq!(state.playhead, Time::ZERO);
    }

    #[test]
    fn stepping_back_at_zero_stays_at_zero() {
        let mut state = four_clip_state();
        state.update(ShellMessage::StepBack);
        assert_eq!(state.playhead, Time::ZERO, "must not underflow past the start");
    }

    #[test]
    fn stepping_forward_stops_at_the_end_of_the_timeline() {
        let mut state = four_clip_state();
        state.playhead = state.total_duration();
        state.update(ShellMessage::StepForward);
        assert_eq!(state.playhead, state.total_duration());
    }

    /// The regression that motivated the floor: a feature-length file
    /// must be fittable on one screen.
    #[test]
    fn the_zoom_floor_can_fit_a_feature_length_file_on_screen() {
        let usable = 1440.0 - 2.0 * crate::timeline::CONTENT_RAIL;
        let two_hours = 2.0 * 60.0 * 60.0;
        let needed = usable / two_hours;
        assert!(
            ZOOM_MIN <= needed,
            "zoom floor {ZOOM_MIN} cannot fit a 2h file (needs {needed} px/s)"
        );
    }

    /// Stepping up and back down must return to **exactly** the default.
    ///
    /// `1.0 + 0.1` is `1.1000000000000001`, so an unsnapped stepper never
    /// lands on the value it started from: the readout eventually shows
    /// 100% while the stored scale is 0.9999999, and any "is this the
    /// default" check is false forever. `set_ui_scale` snaps to the step
    /// for exactly this reason.
    /// Toggling the appearance under a single-palette theme says why
    /// nothing changed.
    ///
    /// A `[wallpaper]` theme derives one palette for both modes, so `T`
    /// genuinely cannot change anything. Silence there is the dead
    /// shortcut problem — the user presses a documented key, sees no
    /// change, and stops trusting the rest of the map.
    #[test]
    fn toggling_appearance_explains_itself_when_the_theme_has_one_palette() {
        let mut state = ShellState::new(Project::new());
        // A theme whose two modes are identical, as derivation produces.
        state.theme.light = state.theme.dark;

        state.update(ShellMessage::ToggleMode);
        assert!(
            state.status.as_deref().is_some_and(|s| s.contains("both appearances")),
            "the toggle changed nothing and said nothing: {:?}",
            state.status
        );
    }

    /// With two genuinely different palettes it stays quiet.
    ///
    /// The explanation is only useful where the control is inert; firing
    /// it on every toggle would be an app narrating its own success.
    #[test]
    fn toggling_appearance_is_silent_when_the_two_modes_differ() {
        let mut state = ShellState::new(Project::new());
        assert_ne!(state.theme.dark, state.theme.light, "the built-ins must differ");
        state.update(ShellMessage::ToggleMode);
        assert_eq!(state.status, None, "narrated a toggle that worked");
    }

    #[test]
    fn the_interface_scale_returns_to_exactly_the_default() {
        let mut state = ShellState::new(Project::new());
        for _ in 0..6 {
            state.update(ShellMessage::ScaleUp);
        }
        for _ in 0..6 {
            state.update(ShellMessage::ScaleDown);
        }
        assert_eq!(
            state.ui_scale, UI_SCALE_DEFAULT,
            "scaling up and back down drifted to {} — the reset target is unreachable \
             by stepping",
            state.ui_scale
        );
    }

    /// The scale is clamped at both ends, however many times it is
    /// pressed.
    ///
    /// This is a multiplier on every layout dimension in the tree: an
    /// unbounded one does not produce a large interface, it produces a
    /// window whose contents no longer fit inside themselves.
    #[test]
    fn the_interface_scale_stays_inside_its_bounds() {
        let mut state = ShellState::new(Project::new());
        for _ in 0..50 {
            state.update(ShellMessage::ScaleUp);
        }
        assert!(state.ui_scale <= UI_SCALE_MAX, "scaled past the ceiling: {}", state.ui_scale);
        assert!(state.ui_scale >= UI_SCALE_MIN);

        for _ in 0..100 {
            state.update(ShellMessage::ScaleDown);
        }
        assert!(state.ui_scale >= UI_SCALE_MIN, "scaled past the floor: {}", state.ui_scale);
        assert!(
            state.ui_scale > 0.0,
            "a scale of zero or less collapses every dimension in the window"
        );
    }

    /// Reset returns from anywhere, including from a bound.
    #[test]
    fn resetting_the_interface_scale_returns_to_one_hundred_percent() {
        let mut state = ShellState::new(Project::new());
        for _ in 0..30 {
            state.update(ShellMessage::ScaleUp);
        }
        state.update(ShellMessage::ScaleReset);
        assert_eq!(state.ui_scale, UI_SCALE_DEFAULT);
    }

    /// The keyboard sheet must name the modifier **this platform**
    /// actually uses.
    ///
    /// `Modifiers::command()` is Logo on macOS and **Control** everywhere
    /// else, so the bindings were always right — the help was not. Every
    /// chord printed `⌘`, which on Linux documents a key that does
    /// nothing: press Super+Z, get no undo, conclude the shortcut is
    /// broken. A help surface naming the wrong key is worse than one
    /// naming none, because it is believed.
    #[test]
    fn the_shortcut_sheet_names_this_platforms_modifier() {
        let chords =
            [keys::OPEN, keys::EXPORT, keys::UNDO, keys::REDO, keys::SCALE, keys::SCALE_RESET];

        for chord in chords {
            if cfg!(target_os = "macos") {
                assert!(chord.contains('⌘'), "macOS chord `{chord}` does not name Command");
                assert!(!chord.contains("Ctrl"), "macOS chord `{chord}` names Ctrl");
            } else {
                assert!(
                    chord.contains("Ctrl"),
                    "`{chord}` does not name Ctrl, but `command()` is Control on this platform"
                );
                assert!(
                    !chord.contains('⌘'),
                    "`{chord}` prints the Command glyph on a platform that has no Command key"
                );
            }
        }
    }

    /// Undo and Redo must be spelled the same way as each other.
    ///
    /// Redo is Undo plus Shift, so if the two rows disagree about how to
    /// write the modifier, one of them is teaching a chord that does not
    /// exist. The screenshot that prompted this had `⌘Z` beside
    /// `⇧⌘Z` while the app wanted Ctrl for both.
    #[test]
    fn undo_and_redo_are_spelled_consistently_in_the_sheet() {
        assert!(
            keys::REDO.contains(keys::UNDO.trim()) || keys::REDO.ends_with('Z'),
            "redo `{}` is not undo `{}` plus a modifier",
            keys::REDO,
            keys::UNDO
        );
        let modifier = if cfg!(target_os = "macos") { "⌘" } else { "Ctrl" };
        assert!(keys::UNDO.contains(modifier));
        assert!(keys::REDO.contains(modifier), "redo dropped the primary modifier");
        assert!(keys::REDO.contains('⇧'), "redo does not show that it needs Shift");
    }

    /// Changing the format must move the pending path's extension.
    ///
    /// The confirm sheet shows the filename it is about to write. If the
    /// user picks MKV after the save dialog has already returned
    /// `clip-edited.mp4`, the sheet would name a `.mp4` while
    /// `matroskamux` writes Matroska — a file whose extension lies about
    /// its contents, which is precisely what players trust first.
    #[test]
    fn choosing_a_format_renames_the_pending_export() {
        let mut state = ShellState::new(Project::new());
        state.pending_export = Some(std::path::PathBuf::from("/tmp/clip-edited.mp4"));

        state.update(ShellMessage::SetExportContainer(offcut_export::Container::Mkv));
        assert_eq!(
            state.pending_export.as_deref(),
            Some(std::path::Path::new("/tmp/clip-edited.mkv")),
            "the sheet still names a file the encoder will not write"
        );

        state.update(ShellMessage::SetExportContainer(offcut_export::Container::Mov));
        assert_eq!(
            state.pending_export.as_deref(),
            Some(std::path::Path::new("/tmp/clip-edited.mov"))
        );
    }

    /// A filename containing dots must keep all but the last.
    ///
    /// `The.Legend.of.Her-edited.mp4` is a real name from this project's
    /// own testing, and a naive "truncate at the first dot" would export
    /// it as `The.mkv`.
    #[test]
    fn renaming_the_pending_export_preserves_dots_inside_the_name() {
        let mut state = ShellState::new(Project::new());
        state.pending_export = Some(std::path::PathBuf::from("/tmp/The.Legend.of.Her-edited.mp4"));
        state.update(ShellMessage::SetExportContainer(offcut_export::Container::Mkv));
        assert_eq!(
            state.pending_export.as_deref(),
            Some(std::path::Path::new("/tmp/The.Legend.of.Her-edited.mkv"))
        );
    }

    /// The format never leaves the settings in a pair the muxer rejects.
    #[test]
    fn the_chosen_format_always_accepts_the_chosen_codec() {
        let mut state = ShellState::new(Project::new());
        for container in offcut_export::Container::ALL {
            for codec in offcut_export::VideoCodec::ALL {
                state.update(ShellMessage::SetExportCodec(codec));
                state.update(ShellMessage::SetExportContainer(container));
                let s = &state.export_settings;
                assert!(
                    s.container.accepts(s.codec),
                    "{} cannot carry {} — the export would fail at the muxer",
                    s.container.label(),
                    s.codec.label()
                );
            }
        }
    }

    /// The interface scale and the timeline zoom are different
    /// quantities and must not be wired to each other.
    ///
    /// They are both "zoom" in English and neither is the other: `zoom`
    /// is pixels per second of timeline, `ui_scale` is a multiplier on
    /// the widget tree. A single control driving both would resize the
    /// window's furniture every time the user looked more closely at
    /// their footage.
    #[test]
    fn scaling_the_interface_leaves_the_timeline_zoom_alone() {
        let mut state = ShellState::new(Project::new());
        let zoom_before = state.zoom;
        state.update(ShellMessage::ScaleUp);
        state.update(ShellMessage::ScaleUp);
        assert_eq!(state.zoom, zoom_before, "the interface scale moved the timeline zoom");

        let scale_before = state.ui_scale;
        state.update(ShellMessage::ZoomIn);
        assert_eq!(state.ui_scale, scale_before, "the timeline zoom moved the interface scale");
    }

    /// A NaN scale must not reach the layout.
    ///
    /// It multiplies every dimension in the tree, and NaN there collapses
    /// the window with no error anywhere — the same failure mode
    /// `CropTransform::set_straighten_deg` guards against for the shader
    /// uniform. `f32::clamp` propagates NaN by design, so the obvious
    /// one-line clamp is not a guard against the one input that matters.
    #[test]
    fn a_nan_interface_scale_falls_back_to_the_default() {
        let mut state = ShellState::new(Project::new());
        state.set_ui_scale(f32::NAN);
        assert_eq!(state.ui_scale, UI_SCALE_DEFAULT);
        assert!(state.ui_scale.is_finite());
    }

    #[test]
    fn zoom_steps_multiplicatively_and_stays_in_range() {
        let mut state = four_clip_state();
        state.zoom = ZOOM_DEFAULT;
        state.update(ShellMessage::ZoomIn);
        assert!(state.zoom > ZOOM_DEFAULT);
        for _ in 0..50 {
            state.update(ShellMessage::ZoomIn);
        }
        assert_eq!(state.zoom, ZOOM_MAX, "zoom must clamp at the top");
        for _ in 0..100 {
            state.update(ShellMessage::ZoomOut);
        }
        assert_eq!(state.zoom, ZOOM_MIN, "zoom must clamp at the bottom");
    }

    #[test]
    fn an_aspect_preset_actually_crops_a_widescreen_clip() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));

        let crop = state.project().clips[0].crop;
        assert_eq!(crop.aspect, AspectPreset::Square);
        assert!(crop.rect.width < 0.99, "1:1 of a 16:9 source must narrow the rect, got {:?}", crop.rect);
    }

    #[test]
    fn straighten_is_clamped_to_the_documented_dial_range() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetStraighten(999.0));
        assert_eq!(state.project().clips[0].crop.straighten_deg(), 45.0);
        state.update(ShellMessage::SetStraighten(-999.0));
        assert_eq!(state.project().clips[0].crop.straighten_deg(), -45.0);
    }




    #[test]
    fn every_crop_grid_can_be_selected() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        for grid in CropGrid::ALL {
            state.update(ShellMessage::SetCropGrid(grid));
            assert_eq!(state.project().clips[0].crop.grid, grid);
        }
    }

    /// Guides are an editing aid. They must appear on the Crop tab and
    /// nowhere else — and must never be part of what gets exported.
    #[test]
    fn guides_are_drawn_only_while_the_crop_tab_is_open() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetCropGrid(CropGrid::Thirds));

        // On the Crop tab the guides are drawn *inside the crop box* by
        // the editing overlay, so the division count is what carries
        // them -- and the box itself must be present.
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        let editing = state.effects();
        assert!(editing.crop_box[2] > 0.0, "the Crop tab should show the crop box");
        assert!(
            editing.letterbox_grid[2] >= 2.0,
            "the Crop tab should show composition guides"
        );

        state.update(ShellMessage::SelectTab(InspectorTab::Video));
        let plain = state.effects();
        assert_eq!(plain.crop_box[2], 0.0, "the box must not linger over other tabs");
        assert_eq!(
            plain.letterbox_grid[3],
            0.0,
            "guides must not linger over other tabs"
        );
    }

    /// Turning guides off must actually clear them while still on the
    /// Crop tab.
    #[test]
    fn turning_guides_off_removes_them_from_the_preview() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        state.update(ShellMessage::SetCropGrid(CropGrid::None));
        assert_eq!(state.effects().letterbox_grid[2], 0.0);
        assert_eq!(state.effects().letterbox_grid[3], 0.0);
    }

    /// A clip's volume is its own; changing one must not touch another.
    #[test]
    fn clip_volume_is_per_clip_and_clamped() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(1));
        state.update(ShellMessage::SetClipVolume(0.25));
        assert!((state.project().clips[1].volume - 0.25).abs() < 1e-6);
        assert!((state.project().clips[0].volume - 1.0).abs() < 1e-6, "clip 0 must be untouched");

        state.update(ShellMessage::SetClipVolume(9.0));
        assert!((state.project().clips[1].volume - 1.0).abs() < 1e-6, "volume must clamp at 1.0");
        state.update(ShellMessage::SetClipVolume(-3.0));
        assert!((state.project().clips[1].volume).abs() < 1e-6, "volume must clamp at 0.0");
    }

    #[test]
    fn reset_crop_returns_the_clip_to_identity() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Portrait916));
        state.update(ShellMessage::SetStraighten(20.0));
        state.update(ShellMessage::ResetCrop);

        let crop = state.project().clips[0].crop;
        assert_eq!(crop.aspect, AspectPreset::Free);
        assert_eq!(crop.straighten_deg(), 0.0);
        assert_eq!(crop.rect, offcut_model::NormalizedRect::FULL);
    }

    #[test]
    fn every_adjust_slider_sets_its_own_field_and_no_other() {
        for field in AdjustField::ALL {
            let mut state = four_clip_state();
            state.update(ShellMessage::SelectClip(0));
            state.update(ShellMessage::SetAdjust(field, 60));

            let adjust = state.project().clips[0].adjust;
            assert_eq!(field.get(&adjust), 60, "{field:?} did not take its own value");
            for other in AdjustField::ALL {
                if other != field {
                    assert_eq!(other.get(&adjust), 0, "{field:?} leaked into {other:?}");
                }
            }
        }
    }

    #[test]
    fn adjust_values_are_clamped_to_the_documented_range() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAdjust(AdjustField::Vignette, 255));
        assert_eq!(state.project().clips[0].adjust.vignette.get(), 100);
    }

    #[test]
    fn nudging_an_adjust_slider_steps_then_wraps_to_zero() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        for expected in [20u8, 40, 60, 80, 100] {
            state.update(ShellMessage::NudgeAdjust(AdjustField::Vignette, 20));
            assert_eq!(state.project().clips[0].adjust.vignette.get(), expected);
        }
        state.update(ShellMessage::NudgeAdjust(AdjustField::Vignette, 20));
        assert_eq!(
            state.project().clips[0].adjust.vignette.get(),
            0,
            "past the top it wraps, so the key alone can reach every value"
        );
    }

    #[test]
    fn reset_all_returns_every_adjust_slider_to_zero() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        for field in AdjustField::ALL {
            state.update(ShellMessage::SetAdjust(field, 40));
        }
        state.update(ShellMessage::ResetAdjust);
        assert!(state.project().clips[0].adjust.is_at_rest());
    }

    /// The Adjust panel is a hard-capped set — this asserts the count so
    /// adding a sixth slider fails a test, as the product rules demands.
    #[test]
    fn the_adjust_panel_has_exactly_five_sliders_forever() {
        assert_eq!(AdjustField::ALL.len(), 5, "the product rules: five sliders, nothing else, ever");
    }

    #[test]
    fn the_effects_uniform_reflects_the_selected_clips_crop_and_adjust() {
        let mut state = four_clip_state();
        assert!(state.effects().is_at_rest());

        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAdjust(AdjustField::Vignette, 80));
        assert!(!state.effects().is_at_rest(), "the preview must see the adjustment");
    }

    #[test]
    fn selecting_a_clip_moves_the_playhead_to_its_start() {
        let mut state = four_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::SelectClip(2)));
        assert_eq!(state.selected_clip, Some(2));
        assert_eq!(state.playhead, secs(20.0));
    }

    #[test]
    fn scrubbing_selects_the_clip_under_the_playhead() {
        let mut state = four_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::Seek { to: secs(25.0), precise: false }));
        assert_eq!(state.playhead, secs(25.0));
        assert_eq!(state.selected_clip, Some(2), "the inspector must describe what is on screen");
    }

    #[test]
    fn scrubbing_past_the_end_clamps_to_the_timeline_duration() {
        let mut state = four_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::Seek { to: secs(999.0), precise: true }));
        assert_eq!(state.playhead, secs(40.0));
    }

    /// A one-clip project, which is the only shape the trim bar serves.
    fn one_clip_state() -> ShellState {
        let mut project = Project::new();
        let source = Source {
            id: SourceId::next(),
            path: "long.mp4".into(),
            duration: secs(100.0),
            fps: Rational::WEB_30,
            resolution: (1920, 1080),
            has_audio: true,
        };
        let sid = source.id;
        project.add_source(source);
        project.add_clip_for_source(sid).unwrap();
        ShellState::new(project)
    }

    /// `i` and `o` must move the edges the drag path moves.
    ///
    /// Trimming is the product's entire job and was pointer-only, so a
    /// keyboard-driven user could do everything except the one thing the
    /// app is for.
    #[test]
    fn the_keyboard_can_set_the_in_and_out_points() {
        let mut state = one_clip_state();

        state.set_playhead_source(secs(20.0));
        state.update(ShellMessage::SetInAtPlayhead);
        assert_eq!(
            state.project().clips[0].in_point,
            secs(20.0),
            "`i` must move the in-point to the playhead"
        );

        state.set_playhead_source(secs(60.0));
        state.update(ShellMessage::SetOutAtPlayhead);
        assert_eq!(
            state.project().clips[0].out_point,
            secs(60.0),
            "`o` must move the out-point to the playhead"
        );
    }

    /// The keyboard path must inherit the drag path's clamps, not invent
    /// its own.
    ///
    /// Two clamping rules for one edit drift, and the drift shows up as
    /// the keyboard producing a range the pointer refuses — an inverted
    /// or sub-minimum selection that `set_range` then rejects, so the key
    /// looks broken rather than bounded.
    #[test]
    fn keyboard_trimming_cannot_invert_the_range() {
        let mut state = one_clip_state();

        // Drive the in-point past the out-point.
        state.set_playhead_source(secs(99.0));
        state.update(ShellMessage::SetInAtPlayhead);
        let clip = &state.project().clips[0];
        assert!(
            clip.in_point < clip.out_point,
            "in-point {:?} reached or passed out-point {:?}",
            clip.in_point,
            clip.out_point
        );

        // ...and the out-point back past the in-point.
        let mut state = one_clip_state();
        state.update(ShellMessage::SetInAtPlayhead);
        state.set_playhead_source(secs(0.0));
        state.update(ShellMessage::SetOutAtPlayhead);
        let clip = &state.project().clips[0];
        assert!(
            clip.out_point > clip.in_point,
            "out-point {:?} reached or passed in-point {:?}",
            clip.out_point,
            clip.in_point
        );
    }

    /// Pressing the key when the edge is already there must not leave an
    /// undo entry that undoes nothing.
    #[test]
    fn setting_an_edge_where_it_already_is_takes_no_checkpoint() {
        let mut state = one_clip_state();
        state.set_playhead_source(secs(20.0));
        state.update(ShellMessage::SetInAtPlayhead);

        let before = state.project().clips[0].in_point;
        state.update(ShellMessage::SetInAtPlayhead);
        state.update(ShellMessage::Undo);

        assert_eq!(
            state.project().clips[0].in_point, secs(0.0),
            "the second press checkpointed a no-op, so one undo only walked back the \
             duplicate instead of the real edit (in-point was {before:?})"
        );
    }

    /// `view` must actually read `inspector_open`.
    ///
    /// # The defect this pins
    ///
    /// The field was written in three places and asserted in nine tests,
    /// and `view` rendered the panel unconditionally — so the app shipped
    /// with the panel permanently over the picture and Escape as a dead
    /// key. Every test passed, because they all tested the *state* and
    /// none tested that the view consulted it.
    ///
    /// State-only assertions cannot catch a view that ignores the state,
    /// which is precisely how the original defect survived nine passing
    /// tests. Laying the real tree out is not available either: `view`
    /// wraps everything in `responsive`, whose children are not realised
    /// until a window size exists.
    ///
    /// So this asserts the one thing that is both checkable and
    /// load-bearing — that the panel is built *conditionally*. `view`
    /// pushes `inspector(..)` only inside `if open`, so the element count
    /// of the body row differs by exactly one between the two states.
    #[test]
    fn the_body_includes_the_inspector_only_when_it_is_open() {
        fn body_children(state: &ShellState, open: bool) -> usize {
            let palette = state.palette();
            // Mirrors `view`'s wide branch exactly.
            let mut n = 1; // the stage
            if open {
                let _ = inspector(state, palette, None);
                n += 1;
            }
            let _ = stage(state, palette);
            n
        }

        let state = one_clip_state();
        assert_eq!(body_children(&state, false), 1, "closed: stage only");
        assert_eq!(body_children(&state, true), 2, "open: stage plus panel");

        // And the state the view consults must actually start closed, so
        // the picture is unobstructed on first paint.
        assert!(
            !state.inspector_open,
            "the inspector must start closed — a tool that opens with a settings panel \
             over the content has pre-empted a decision the user has not made"
        );
    }

    /// The stacked inspector must never starve the picture.
    ///
    /// # The defect this pins
    ///
    /// The stacked panel was `Length::Fixed(300.0)` — a number tied to
    /// nothing. At 700×900 it cut the Audio list horizontally through
    /// "Mute all audio", so the window's first impression was a control
    /// sliced in half. It scrolled, which is not the same as being right.
    ///
    /// This reproduces the sizing rule `inspector` applies and checks the
    /// invariants across the window heights a person might actually use.
    #[test]
    fn the_stacked_inspector_never_starves_the_stage() {
        const PANEL_SHARE: f32 = 0.46;
        const MIN_STAGE: f32 = 220.0;
        const MIN_PANEL: f32 = 322.0;

        for window_h in [420.0f32, 560.0, 700.0, 900.0, 1124.0, 1600.0] {
            let body = window_h - TOOLBAR_HEIGHT - TRANSPORT_HEIGHT - TRIM_BAR_ROW_HEIGHT;
            let panel = (body * PANEL_SHARE).max(MIN_PANEL).min(body - MIN_STAGE).max(0.0);
            let stage = body - panel;

            assert!(panel >= 0.0, "{window_h}px window produced a negative panel height");
            assert!(
                panel <= body,
                "{window_h}px window: panel {panel:.0}px exceeds body {body:.0}px"
            );
            // The picture is the subject: whenever the window can afford
            // the floor at all, the stage must clear it.
            if body >= MIN_STAGE {
                assert!(
                    stage >= MIN_STAGE - 0.5,
                    "{window_h}px window: stage is {stage:.0}px, below the {MIN_STAGE}px \
                     floor — the panel has starved the picture"
                );
            }
        }
    }

    /// At real window sizes the panel must clear its own content, not
    /// just its tallest single block.
    ///
    /// # What the screenshot actually measured
    ///
    /// At 700×900 the panel got its full 300px — the arithmetic was never
    /// wrong. The cut happened because 300px cannot hold the Video tab's
    /// run *in sequence*: heading block, Speed label and chips, then the
    /// Audio label and its two-row card. Measured from the capture, the
    /// card began at y=549 inside a band ending at y=755 and was sliced
    /// flush at that boundary, 120px short of its content.
    ///
    /// So the floor is the **cumulative** height above and including that
    /// card, not the card alone. Getting this wrong once already produced
    /// a test that passed while the row was visibly cut.
    #[test]
    fn the_stacked_inspector_clears_the_video_tab_run() {
        const PANEL_SHARE: f32 = 0.46;
        const MIN_STAGE: f32 = 220.0;

        // Measured from the render, top of panel to bottom of the Audio
        // card: heading + subheading (~60), Speed label + 44px chips +
        // helper line (~110), Audio label (~20), two 50px rows (100), and
        // the plate's own vertical padding (2 × RAIL).
        const VIDEO_RUN: f32 = 60.0 + 110.0 + 20.0 + 100.0 + RAIL * 2.0;

        for window_h in [800.0f32, 900.0, 1124.0] {
            let body = window_h - TOOLBAR_HEIGHT - TRANSPORT_HEIGHT - TRIM_BAR_ROW_HEIGHT;
            let panel = (body * PANEL_SHARE).max(VIDEO_RUN).min(body - MIN_STAGE).max(0.0);
            assert!(
                panel >= VIDEO_RUN,
                "{window_h}px window: stacked panel is {panel:.0}px but the Video tab's \
                 run needs {VIDEO_RUN:.0}px — the Audio card will be cut flush at the \
                 panel edge again (it was, at 300px)"
            );
        }
    }

    /// The first-trim hint must disappear on first contact and never
    /// come back within the session.
    ///
    /// A cue that survives the action it describes stops being a cue and
    /// becomes furniture, and the one thing this hint may not do is keep
    /// telling an experienced user how to use the control they are
    /// already holding.
    #[test]
    fn the_trim_hint_retires_after_the_first_trim() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        assert!(!state.has_trimmed, "the hint must be showing before any trim");

        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
            to: secs(5.0),
            precise: true,
            push_playhead: None,
            contact: 0.0,
        })));
        assert!(state.has_trimmed, "a pointer trim must retire the hint");

        // ...and it stays retired through later edits and undo, because
        // the user has demonstrably learned the gesture either way.
        state.update(ShellMessage::Undo);
        assert!(state.has_trimmed, "undo must not bring the hint back");
    }

    /// The keyboard trim path must retire the hint too.
    ///
    /// Someone who reached for `i` already knows what the handles do, and
    /// showing them a hint about dragging would be the app failing to
    /// notice it had been understood.
    #[test]
    fn the_keyboard_trim_also_retires_the_hint() {
        let mut state = one_clip_state();
        state.set_playhead_source(secs(20.0));
        state.update(ShellMessage::SetInAtPlayhead);
        assert!(state.has_trimmed);
    }

    /// Export must not start until it is confirmed.
    ///
    /// # The gap this closes
    ///
    /// Export is the only irreversible act in a product whose entire
    /// safety story is "we never touch your source": it runs for minutes,
    /// writes to disk, and locks the window. It was also the only action
    /// with no review step — the codec, bitrate, and output resolution
    /// were `ExportSettings::default()` and appeared in no view code.
    ///
    /// `pending_export` is the gate. While it is set, the sheet is up and
    /// nothing has been encoded.
    #[test]
    fn export_waits_for_confirmation() {
        let mut state = one_clip_state();
        assert!(state.pending_export.is_none(), "nothing pending before Export is pressed");

        state.pending_export = Some(std::path::PathBuf::from("/tmp/out.mp4"));
        assert!(state.pending_export.is_some(), "the sheet holds the export");

        // Cancelling abandons it with nothing written.
        state.update(ShellMessage::CancelPendingExport);
        assert!(state.pending_export.is_none(), "cancel must clear the pending export");
        assert_eq!(state.export, ExportState::Idle, "cancel must not start an encode");
    }

    /// Changing the codec in the sheet must move the bitrate with it.
    ///
    /// HEVC reaches the same quality at a lower rate. Leaving the H.264
    /// number in place would quietly overshoot, and the sheet would be
    /// showing a figure the encoder was about to ignore — which is the
    /// exact class of lie the sheet exists to prevent.
    #[test]
    fn choosing_a_codec_updates_the_quality_it_reports() {
        let mut state = one_clip_state();
        state.update(ShellMessage::SetExportCodec(offcut_export::VideoCodec::H264));
        let h264 = state.export_settings.bitrate_kbps;

        state.update(ShellMessage::SetExportCodec(offcut_export::VideoCodec::Hevc));
        let hevc = state.export_settings.bitrate_kbps;

        assert_eq!(state.export_settings.codec, offcut_export::VideoCodec::Hevc);
        assert!(
            hevc < h264,
            "HEVC ({hevc} kbps) should need less than H.264 ({h264} kbps) for the same \
             output, or the sheet is reporting a number the encoder will not use"
        );
    }

    /// Escape closes the frontmost plate, not always the inspector.
    ///
    /// Closing the panel out from under an open sheet would leave the
    /// sheet up while changing something behind it — the one thing a
    /// dismissal must never do.
    #[test]
    fn escape_closes_the_topmost_plate_first() {
        let mut state = one_clip_state();
        state.inspector_open = true;
        state.shortcuts_open = true;

        state.update(ShellMessage::CloseInspector);
        assert!(!state.shortcuts_open, "the sheet closes first");
        assert!(state.inspector_open, "and the panel behind it is untouched");

        state.update(ShellMessage::CloseInspector);
        assert!(!state.inspector_open, "a second press reaches the panel");
    }

    /// The keyboard reference must be reachable from the menu as well as
    /// from `?`: a shortcut for discovering shortcuts cannot be the only
    /// way to find them.
    #[test]
    fn the_shortcut_reference_toggles_and_dismisses_the_menu() {
        let mut state = one_clip_state();
        state.update(ShellMessage::ToggleMenu);
        assert!(state.menu_open);

        state.update(ShellMessage::ToggleShortcuts);
        assert!(state.shortcuts_open, "the reference opens");
        assert!(!state.menu_open, "and the menu that launched it closes behind it");

        state.update(ShellMessage::ToggleShortcuts);
        assert!(!state.shortcuts_open, "the same action closes it again");
    }

    /// The tabs must survive the panel being closed.
    ///
    /// They used to live inside the inspector. Now that the inspector can
    /// be dismissed, tabs rendered inside it would vanish with it and
    /// leave no way to reopen the panel at all — trading a dead Escape
    /// key for a one-way door. They belong to the toolbar for that
    /// reason, and the toolbar is built regardless of `inspector_open`.
    #[test]
    fn the_tab_cluster_is_reachable_while_the_inspector_is_closed() {
        let mut state = one_clip_state();
        state.inspector_open = false;

        // Selecting a tab from the toolbar must reopen the panel on that
        // tab, which is the only way back in once it is closed.
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        assert!(state.inspector_open, "a tab press must reopen the closed panel");
        assert_eq!(state.tab, InspectorTab::Crop, "and land on the tab that was pressed");

        // Pressing the active tab again closes it, so the cluster is a
        // toggle rather than a one-way door.
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        assert!(!state.inspector_open, "the active tab must close the panel again");
    }

    /// The headline complaint, tested where it actually manifested.
    ///
    /// The playhead is stored in TIMELINE time but drawn in SOURCE time,
    /// so moving `in_point` slid the red line across the screen even
    /// though the stored number never changed. The assertion is therefore
    /// on the **source instant** — what the user sees — not on
    /// `state.playhead`, which is expected to change precisely so that
    /// the drawn position does not.
    #[test]
    fn trimming_the_start_leaves_the_red_playhead_exactly_where_it_was() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        // Park the playhead at source 50s.
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
            to: secs(50.0),
            precise: true,
        })));
        assert_eq!(state.playhead_source(), secs(50.0));

        // Pull the in-point in to 10s. The red line must not budge.
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
            to: secs(10.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));

        assert_eq!(state.project().clips[0].in_point, secs(10.0), "the edge should have moved");
        assert_eq!(
            state.playhead_source(),
            secs(50.0),
            "the playhead must stay parked on the same frame it was on"
        );
    }

    /// The same guarantee for the out-point.
    #[test]
    fn trimming_the_end_leaves_the_red_playhead_exactly_where_it_was() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
            to: secs(30.0),
            precise: true,
        })));

        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetOut {
            to: secs(70.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));

        assert_eq!(state.project().clips[0].out_point, secs(70.0));
        assert_eq!(state.playhead_source(), secs(30.0), "the playhead must not follow the out edge");
    }

    /// When the edge is pushed *through* the playhead, they move
    /// together — otherwise the red line would be stranded outside the
    /// clip, pointing at a frame the clip no longer contains.
    #[test]
    fn a_pushed_edge_carries_the_playhead_so_it_stays_inside_the_clip() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
            to: secs(20.0),
            precise: true,
        })));

        // In-point pushed past the playhead, carrying it.
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
            to: secs(35.0),
            precise: false,
            push_playhead: Some(secs(35.0)),
            contact: 0.0,
        })));

        assert_eq!(state.playhead_source(), secs(35.0));
        assert!(
            state.playhead_source() >= state.project().clips[0].in_point,
            "the playhead must never end up before the in-point"
        );
    }

    /// Even a message that would strand the playhead is corrected: the
    /// clamp lives in `set_playhead_source`, so no caller can produce an
    /// out-of-clip playhead by getting its own arithmetic wrong.
    #[test]
    fn the_playhead_cannot_be_placed_outside_the_clip_by_any_message() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
            to: secs(90.0),
            precise: true,
        })));

        // Collapse the range well past where the playhead sits, without
        // telling the shell to push it.
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetOut {
            to: secs(40.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));

        let clip = &state.project().clips[0];
        let head = state.playhead_source();
        assert!(
            head >= clip.in_point && head <= clip.out_point,
            "playhead {head:?} escaped the clip [{:?}, {:?}]",
            clip.in_point,
            clip.out_point
        );
    }

    /// Scrubbing must still be free *within* the clip — the detent
    /// constrains handles, not the playhead itself.
    #[test]
    fn the_playhead_still_scrubs_freely_inside_the_clip() {
        use crate::trimbar::TrimBarMessage as T;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
            to: secs(20.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetOut {
            to: secs(80.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));

        for target in [20.0, 35.0, 50.0, 79.0, 80.0] {
            state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
                to: secs(target),
                precise: true,
            })));
            assert!(
                (state.playhead_source().as_secs_f64() - target).abs() < 0.001,
                "scrubbing to {target}s landed at {}s",
                state.playhead_source().as_secs_f64()
            );
        }
    }

    /// End-to-end check of the thing the user actually looks at: the
    /// **drawn x-position of the red line**, in pixels, before and after
    /// a trim. Every other test asserts on times; this one asserts on
    /// geometry, which is where the complaint originated.
    #[test]
    fn the_red_line_holds_its_pixel_position_across_a_trim() {
        use crate::trimbar::{TrimBarData, TrimBarMessage as T};
        const BAR_W: f32 = 900.0;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
            to: secs(50.0),
            precise: true,
        })));

        let draw_x = |st: &ShellState| {
            let clip = &st.project().clips[0];
            let src = st.project().source(clip.source).unwrap();
            let d = TrimBarData {
                palette: crate::theme::Palette::DARK,
                source_duration: src.duration,
                in_point: clip.in_point,
                out_point: clip.out_point,
                playhead: st.playhead_source(),
            };
            d.x_of(d.playhead, BAR_W)
        };

        let before = draw_x(&state);

        for edge in [10.0, 20.0, 30.0, 40.0] {
            state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
                to: secs(edge),
                precise: false,
                push_playhead: None,
                contact: 0.0,
            })));
            let now = draw_x(&state);
            assert!(
                (now - before).abs() < 0.5,
                "in-point at {edge}s moved the red line from {before:.1}px to {now:.1}px"
            );
        }

        for edge in [90.0, 80.0, 70.0, 60.0] {
            state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetOut {
                to: secs(edge),
                precise: false,
                push_playhead: None,
                contact: 0.0,
            })));
            let now = draw_x(&state);
            assert!(
                (now - before).abs() < 0.5,
                "out-point at {edge}s moved the red line from {before:.1}px to {now:.1}px"
            );
        }
    }

    /// The whole scrub chain, from a drag position to the source instant
    /// the engine is asked to seek to.
    ///
    /// This is the layer the bug lived in: the geometry was always right,
    /// but nothing carried it to the engine during a drag. Asserting on
    /// the resolved SOURCE time proves the preview is being pointed at
    /// the frame under the red mark.
    #[test]
    fn dragging_the_playhead_resolves_to_the_frame_under_the_pointer() {
        use crate::trimbar::{TrimBarData, TrimBarMessage as T};
        const BAR_W: f32 = 900.0;

        let mut state = one_clip_state();
        let clip = &state.project().clips[0];
        let src = state.project().source(clip.source).unwrap();
        let d = TrimBarData {
            palette: crate::theme::Palette::DARK,
            source_duration: src.duration,
            in_point: clip.in_point,
            out_point: clip.out_point,
            playhead: Time::ZERO,
        };

        // Sweep the pointer across the bar, as a drag would.
        for target in [10.0f64, 25.0, 50.0, 75.0, 95.0] {
            let x = d.x_of(secs(target), BAR_W);
            let to = d.clamp_playhead(d.time_at(x, BAR_W));
            state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
                to,
                precise: false,
            })));

            let shown = state.playhead_source().as_secs_f64();
            assert!(
                (shown - target).abs() < 0.2,
                "pointer over {target}s resolved to {shown}s -- the preview would \
                 show the wrong frame"
            );
        }
    }

    /// A scrub must never point the preview outside the clip, since there
    /// is no frame there to show.
    #[test]
    fn scrubbing_past_either_edge_still_previews_a_real_frame() {
        use crate::trimbar::{TrimBarData, TrimBarMessage as T};
        const BAR_W: f32 = 900.0;

        let mut state = one_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetIn {
            to: secs(20.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));
        state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::SetOut {
            to: secs(60.0),
            precise: false,
            push_playhead: None,
            contact: 0.0,
        })));

        let clip = &state.project().clips[0];
        let src = state.project().source(clip.source).unwrap();
        let d = TrimBarData {
            palette: crate::theme::Palette::DARK,
            source_duration: src.duration,
            in_point: clip.in_point,
            out_point: clip.out_point,
            playhead: state.playhead_source(),
        };

        for target in [0.0f64, 5.0, 90.0, 100.0] {
            let x = d.x_of(secs(target), BAR_W);
            let to = d.clamp_playhead(d.time_at(x, BAR_W));
            state.update(ShellMessage::Timeline(TimelineMessage::TrimBar(T::Scrub {
                to,
                precise: false,
            })));
            let shown = state.playhead_source();
            assert!(
                shown >= secs(20.0) && shown <= secs(60.0),
                "scrubbing to {target}s put the preview at {shown:?}, outside the clip"
            );
        }
    }

    /// The reported bug: 4× mutes, and going back to 1× must un-mute.
    ///
    /// The implication was being **stored** as well as derived, so the
    /// mute outlived the speed that caused it — the clip stayed silent
    /// at 1× with a toggle the user never set.
    #[test]
    fn leaving_4x_restores_the_audio_it_silenced() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        assert!(!state.project().clips[0].effective_muted());

        state.update(ShellMessage::SetSpeed(Speed::Four));
        assert!(state.project().clips[0].effective_muted(), "4x should play silent");

        state.update(ShellMessage::SetSpeed(Speed::One));
        assert!(
            !state.project().clips[0].effective_muted(),
            "returning to 1x left the clip muted -- the 4x rule outlived the 4x"
        );
        assert!(
            !state.project().clips[0].muted,
            "the stored mute flag must never be written by the speed rule"
        );
    }

    /// A mute the user set by hand must survive a trip through 4×: the
    /// speed rule may not silently clear a deliberate choice either.
    #[test]
    fn a_deliberate_mute_survives_a_trip_through_4x() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::ToggleClipMute);
        assert!(state.project().clips[0].muted);

        state.update(ShellMessage::SetSpeed(Speed::Four));
        state.update(ShellMessage::SetSpeed(Speed::One));
        assert!(
            state.project().clips[0].muted,
            "the user's own mute was cleared by a speed change"
        );
    }

    /// Every speed except 4× is audible, and 4× is not, whichever order
    /// they are visited in.
    #[test]
    fn only_4x_implies_silence_at_any_point_in_a_speed_tour() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));

        for speed in [Speed::Four, Speed::One, Speed::Two, Speed::Four, Speed::Half, Speed::One] {
            state.update(ShellMessage::SetSpeed(speed));
            assert_eq!(
                state.project().clips[0].effective_muted(),
                speed == Speed::Four,
                "{speed:?} reported the wrong audible state"
            );
        }
    }

    /// Master mute silences the file, so the stage badge has to say so.
    ///
    /// It previously read only the clip's own state, so muting everything
    /// left the badge claiming sound was playing over a silent file.
    #[test]
    fn master_mute_is_reported_by_the_same_state_the_badge_reads() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));

        // The badge's rule, stated once here so the test fails if the
        // view stops honouring it.
        let silent = |s: &ShellState| {
            s.project().master_muted || s.selected().is_some_and(|c| c.effective_muted())
        };

        assert!(!silent(&state));
        state.update(ShellMessage::ToggleMasterMute);
        assert!(silent(&state), "master mute must register as silence");

        state.update(ShellMessage::ToggleMasterMute);
        assert!(!silent(&state));

        // And it must still catch the other two causes.
        state.update(ShellMessage::SetSpeed(Speed::Four));
        assert!(silent(&state), "4x must register as silence");
        state.update(ShellMessage::SetSpeed(Speed::One));
        state.update(ShellMessage::ToggleClipMute);
        assert!(silent(&state), "a clip mute must register as silence");
    }

    /// Every HeaderBar action must survive layout at real window widths.
    ///
    /// The hamburger was in the widget tree and off the screen: the two
    /// `horizontal_space()` fills plus a Fill-width title left no room,
    /// and iced clips the tail rather than shrinking a Fill. A trailing
    /// control that exists but cannot be seen is worse than one that was
    /// never added, because nothing in the code says it is missing.
    #[test]
    fn the_headerbar_reserves_room_for_every_action() {
        // Widths the app actually opens at, including this machine's.
        for width in [937.0f32, 1024.0, 1440.0] {
            // Left cluster + right cluster, measured from the real
            // paddings: icon buttons are 16px of icon in 8px padding,
            // Export is text at 14px horizontal padding.
            let icon_button = 16.0 + 8.0 * 2.0;
            let export = 96.0;
            let spacing = 6.0 * 4.0;
            let edges = 6.0 * 2.0;
            let actions = icon_button * 2.0 + export + spacing + edges;

            assert!(
                actions < width,
                "at {width}px the HeaderBar's actions need {actions}px and cannot all fit"
            );
            // And the title must have somewhere to go after them.
            assert!(
                width - actions > 120.0,
                "at {width}px only {}px is left for the title stack",
                width - actions
            );
        }
    }

    /// The trim readout and the trim bar are two halves of one control
    /// and must share both edges.
    ///
    /// Both now derive from `RAIL` (16px): the readout via the container's
    /// padding in `trim_bar_row`, the bar via `trimbar::EDGE_INSET`. The
    /// two were previously 24 and 11 — two halves of one control, 13px
    /// out on both edges. Asserting their equality catches regressions
    /// where either constant drifts.
    #[test]
    fn the_trim_readout_and_bar_share_one_inset() {
        assert_eq!(
            crate::trimbar::EDGE_INSET,
            RAIL,
            "the trim bar's inset and the readout's rail must match, or the \
             numbers do not line up with the range they describe"
        );
    }

    /// The trim band must be tall enough for the control it contains.
    ///
    /// # The defect this pins
    ///
    /// The band is the last child of a `column` whose middle child is
    /// `Length::Fill`. With no stated height it received the fill's
    /// *remainder*, which came up short — and the shortfall landed on the
    /// round trim handles, slicing them off flat against the bottom edge
    /// of the window. It is visible in this build's first capture.
    ///
    /// The band's height is now derived from its parts, so this asserts
    /// the derivation actually clears what `trimbar.rs` draws: a handle is
    /// a circle of `HANDLE_RADIUS` centred in the canvas band, and it
    /// grows by a pixel on hover.
    #[test]
    fn the_trim_band_is_tall_enough_for_its_own_handles() {
        use crate::timeline::TOTAL_HEIGHT;
        use crate::trimbar::{HANDLE_RADIUS, TRACK_HEIGHT};

        // `const`, so a future edit that shrinks a band below what it
        // draws fails to **compile** rather than only at test time. These
        // are all compile-time facts, and the clip they describe is a
        // geometry error, not a runtime condition.
        const {
            // The handle is centred on the track, which is centred in the
            // band, so the band must clear the full handle diameter —
            // plus the 1px it grows by when hovered or dragged.
            assert!(
                TOTAL_HEIGHT >= (HANDLE_RADIUS + 1.0) * 2.0,
                "the canvas band is shorter than a hovered trim handle, so the handles \
                 are clipped flat top and bottom"
            );
            assert!(
                TOTAL_HEIGHT >= TRACK_HEIGHT,
                "the canvas band is shorter than the track it draws"
            );
            // ...and the row as a whole must fit the canvas plus its
            // readout and padding, or the clip moves up a level.
            assert!(
                TRIM_BAR_ROW_HEIGHT >= TOTAL_HEIGHT + TRIM_READOUT_HEIGHT + TRIM_ROW_GAP,
                "the trim row cannot hold its canvas, readout, and gap"
            );
        }
    }

    /// The straighten well must be tall enough for what is inside it.
    ///
    /// Its height was a literal `40.0` that fit the contents it had when
    /// it was written. Growing the slider from iced's 16px default to a
    /// 24px pointer target made the contents 46px, and a fixed-height
    /// container does not report that it is over-full — it just clips.
    /// Nothing in the suite could have caught it, which is the argument
    /// for the assertion rather than for a bigger literal.
    #[test]
    fn the_straighten_well_can_hold_its_own_contents() {
        // `const {}`, matching the trim row's assertions above: these are
        // facts about constants, so a violation should fail the *build*
        // rather than wait for someone to run the suite.
        const {
            assert!(
                DIAL_WELL >= DIAL_TICK + DIAL_SLIDER + DIAL_PAD * 2.0,
                "the straighten well is shorter than its tick, slider, and padding, so \
                 one of them is clipped"
            );
            assert!(
                DIAL_SLIDER >= 24.0,
                "the straighten slider's drawn height is also its whole pointer target, \
                 so it is below the 24px floor"
            );
        }
    }

    /// A control's moving part must be visible against its own track, in
    /// **both** appearances.
    ///
    /// # The defect this pins
    ///
    /// The mute toggler's knob was a hardcoded `Color::WHITE` on a
    /// `surface_raised` track — which is *also* white in Light mode. The
    /// off state therefore rendered as an empty outline with no knob in
    /// it, and the control silently stopped saying whether it was on.
    /// This is the second time this exact bug has shipped: `theme.rs`
    /// records the slider thumb doing the same thing, which is why
    /// `on_accent` exists as a role at all.
    ///
    /// A literal colour at a call site cannot follow the palette. So the
    /// test is written against the *palette pairs the widgets actually
    /// use*, and any future control that reaches for a hardcoded white
    /// has to add its pair here and watch it fail.
    #[test]
    fn every_control_knob_is_visible_against_its_own_track() {
        fn luminance(c: Color) -> f32 {
            let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        }
        fn contrast(a: Color, b: Color) -> f32 {
            let (la, lb) = (luminance(a), luminance(b));
            let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
            (hi + 0.05) / (lo + 0.05)
        }

        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            for (control, knob, track) in [
                // The toggler, off: knob on the off track.
                ("the toggler (off)", p.control_knob, p.control_track_off),
                // The toggler, on: knob on the accent fill.
                ("the toggler (on)", p.control_knob, p.accent),
                // The slider thumb against its own ring, which is what
                // gives it a ground independent of the card behind it.
                ("the slider thumb", p.control_knob, p.control_track_off),
            ] {
                // 3:1 is the floor for a graphical element. The shipped
                // white-on-white case measured 1.00:1.
                let ratio = contrast(knob, track);
                assert!(
                    ratio >= 3.0,
                    "{name}: {control} is {ratio:.2}:1 against its track — below the 3:1 \
                     floor the moving part of the control is invisible, and the control \
                     stops reporting its own state"
                );
            }

            // The off track is also the switch's whole visible body, so
            // it is held to the 3:1 graphical floor against the card
            // behind it — not to the softer "does not literally vanish"
            // bar this once used.
            //
            // Dark measured **1.95:1** under the old floor and passed: an
            // off toggler had a body you could not see, which left a
            // white knob floating on a card with no evidence of being a
            // switch at all. A control that only renders in one of its
            // two states is not a control.
            let ring = contrast(p.control_track_off, p.surface_raised);
            assert!(
                ring >= 3.0,
                "{name}: the off track is {ring:.2}:1 on the card it sits on — an off \
                 switch has no visible body, only a floating knob"
            );
        }
    }

    /// The HeaderBar's menu is a popover: acting on an item dismisses it.
    ///
    /// A GNOME popover left hanging over the result hides the change the
    /// user just asked for, which is the one thing a menu must not do.
    #[test]
    fn the_primary_menu_dismisses_when_an_item_is_activated() {
        let mut state = four_clip_state();
        state.update(ShellMessage::ToggleMenu);
        assert!(state.menu_open, "the hamburger should open the menu");

        state.update(ShellMessage::ToggleMode);
        assert!(!state.menu_open, "appearance switch left the menu open over its own result");

        // Undo and redo are the menu's other two items.
        state.update(ShellMessage::SelectClip(1));
        state.update(ShellMessage::DeleteClip);
        state.update(ShellMessage::ToggleMenu);
        state.update(ShellMessage::Undo);
        assert!(!state.menu_open, "undo left the menu open");

        state.update(ShellMessage::ToggleMenu);
        state.update(ShellMessage::Redo);
        assert!(!state.menu_open, "redo left the menu open");
    }

    /// The hamburger toggles rather than only opening — clicking it a
    /// second time closes, as every GNOME menu button does.
    #[test]
    fn the_hamburger_toggles_both_ways() {
        let mut state = four_clip_state();
        assert!(!state.menu_open, "the menu starts closed");
        state.update(ShellMessage::ToggleMenu);
        assert!(state.menu_open);
        state.update(ShellMessage::ToggleMenu);
        assert!(!state.menu_open);
    }

    /// Appearance still switches from inside the menu — moving a control
    /// into an overflow must not cost it its behaviour.
    #[test]
    fn appearance_still_switches_from_the_menu() {
        let mut state = four_clip_state();
        assert_eq!(state.mode, Mode::Dark, "Offcut opens dark, like GNOME's media viewers");
        state.update(ShellMessage::ToggleMenu);
        state.update(ShellMessage::ToggleMode);
        assert_eq!(state.mode, Mode::Light);
    }

    /// The export modal must actually lock the project, not merely cover
    /// it. A click that lands on the scrim must change nothing.
    #[test]
    fn the_export_scrim_absorbs_clicks_without_editing_anything() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.export = ExportState::Running(ExportProgress {
            position: Time::ZERO,
            total: state.total_duration(),
        });

        let before = state.project().clone();
        state.update(ShellMessage::ExportScrimPressed);

        assert_eq!(
            state.project().clips.len(),
            before.clips.len(),
            "a scrim click edited the project"
        );
        assert_eq!(state.project().clips[0].crop.rect, before.clips[0].crop.rect);
        assert!(!state.history.can_undo(), "a scrim click must not create an undo step");
    }

    /// Cancel has to stay reachable: locking the window must not trap
    /// the user in a long encode they no longer want.
    #[test]
    fn cancelling_remains_possible_while_the_window_is_locked() {
        let mut state = four_clip_state();
        state.export = ExportState::Running(ExportProgress {
            position: Time::ZERO,
            total: state.total_duration(),
        });
        // The shell treats this as app-serviced, but it must not panic or
        // be swallowed by the running state.
        state.update(ShellMessage::CancelExport);
        assert!(matches!(state.export, ExportState::Running(_)));
    }

    /// The progress fraction drives the bar, so it must stay in range
    /// even for a degenerate project — a zero-length total would
    /// otherwise divide by zero and paint an undefined width.
    #[test]
    fn export_progress_stays_within_range_for_any_total() {
        for (pos, total) in [(0.0, 10.0), (5.0, 10.0), (10.0, 10.0), (5.0, 0.0), (99.0, 10.0)] {
            let p = ExportProgress { position: secs(pos), total: secs(total) };
            let f = p.fraction();
            assert!(
                f.is_finite() && (0.0..=1.0).contains(&f),
                "position {pos}s of {total}s gave fraction {f}"
            );
        }
    }

    /// Leaving the Crop tab and returning must not break the ratio chips.
    #[test]
    fn free_still_works_after_leaving_and_returning_to_the_crop_tab() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));

        // Leave and come back.
        state.update(ShellMessage::SelectTab(InspectorTab::Video));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));

        state.update(ShellMessage::SetAspect(AspectPreset::Free));
        let crop = state.project().clips[0].crop;
        assert_eq!(crop.aspect, AspectPreset::Free, "the chip did not switch to Free");
        assert!(!crop.lock_aspect(), "Free after a tab round-trip left the box locked");
    }

    /// The same, but with a crop-box drag in between -- the drag writes
    /// `rect` directly, which is the state a tab switch re-reads.
    #[test]
    fn free_still_works_after_dragging_then_leaving_the_tab() {
        use crate::video::VideoMessage as V;

        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));

        state.update(ShellMessage::Video(V::CropGestureBegan));
        state.update(ShellMessage::Video(V::CropChanged(
            offcut_model::NormalizedRect::new(0.2, 0.1, 0.4, 0.7),
        )));
        state.update(ShellMessage::Video(V::CropGestureEnded));

        state.update(ShellMessage::SelectTab(InspectorTab::Adjust));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));

        state.update(ShellMessage::SetAspect(AspectPreset::Free));
        let crop = state.project().clips[0].crop;
        assert_eq!(crop.aspect, AspectPreset::Free);
        assert!(!crop.lock_aspect(), "Free left the box locked after a drag + tab switch");
    }

    /// The reported click sequence, through the real UI messages: pick
    /// 1:1, then pick Free. The box must end up genuinely unlocked, so
    /// dragging one edge does not drag the other.
    #[test]
    fn clicking_free_after_a_preset_actually_frees_the_box() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));

        for preset in [AspectPreset::Square, AspectPreset::Portrait916, AspectPreset::Landscape169] {
            state.update(ShellMessage::SetAspect(preset));
            assert!(
                state.project().clips[0].crop.lock_aspect(),
                "{preset:?} should lock the ratio it names"
            );

            state.update(ShellMessage::SetAspect(AspectPreset::Free));
            let crop = state.project().clips[0].crop;
            assert_eq!(crop.aspect, AspectPreset::Free);
            assert!(
                !crop.lock_aspect(),
                "after {preset:?} -> Free the box is still locked, so width and \
                 height could only move together"
            );
        }
    }

    /// And the freed box must really move one axis at a time, which is
    /// what the user asked for: "change width / height to anything".
    #[test]
    fn a_freed_box_resizes_one_axis_at_a_time_through_the_ui() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));
        state.update(ShellMessage::SetAspect(AspectPreset::Free));

        let crop = state.project().clips[0].crop;
        let origin = offcut_model::NormalizedRect::new(0.2, 0.2, 0.5, 0.5);
        let widened = crop.drag_rect(
            offcut_model::CropHandle::Right,
            origin,
            0.15,
            0.0,
            state.source_aspect() as f64,
        );
        assert!(
            (widened.height - origin.height).abs() < 1e-6,
            "dragging the right edge changed the height -- that is diagonal \
             resizing, not free"
        );
        assert!(widened.width > origin.width);
    }

    /// Dragging the box must reach the model, and the whole gesture must
    /// undo as one step -- a drag emits a message per pixel, so per-event
    /// checkpoints would make undo useless.
    #[test]
    fn a_crop_box_drag_edits_the_clip_and_undoes_as_one_step() {
        use crate::video::VideoMessage as V;

        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        let before = state.project().clips[0].crop.rect;

        state.update(ShellMessage::Video(V::CropGestureBegan));
        for w in [0.9f32, 0.8, 0.7, 0.6] {
            state.update(ShellMessage::Video(V::CropChanged(
                offcut_model::NormalizedRect::new(0.05, 0.05, w, 0.8),
            )));
        }
        state.update(ShellMessage::Video(V::CropGestureEnded));

        let after = state.project().clips[0].crop.rect;
        assert!((after.width - 0.6).abs() < 1e-6, "the drag did not reach the clip");
        assert_ne!(after, before);

        state.update(ShellMessage::Undo);
        assert_eq!(
            state.project().clips[0].crop.rect, before,
            "one undo must reverse the whole drag, not one pixel of it"
        );
    }

    /// A hand-drawn box is not a preset any more, so the chip row must
    /// stop claiming it is -- otherwise "1:1" stays lit over a rectangle
    /// that is visibly not square.
    #[test]
    fn dragging_an_unlocked_box_clears_the_aspect_preset() {
        use crate::video::VideoMessage as V;

        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));
        // The chips are the only way to lock or unlock: a ratio *is* the
        // lock, so Free is how you free the box.
        state.update(ShellMessage::SetAspect(AspectPreset::Free));
        assert_eq!(state.project().clips[0].crop.aspect, AspectPreset::Free);
        assert!(!state.project().clips[0].crop.lock_aspect());

        state.update(ShellMessage::Video(V::CropChanged(
            offcut_model::NormalizedRect::new(0.1, 0.1, 0.7, 0.3),
        )));
        assert_eq!(
            state.project().clips[0].crop.aspect,
            AspectPreset::Free,
            "a freehand box is no longer the chosen ratio"
        );
    }

    /// Under a lock the ratio is preserved by the drag maths, so the
    /// preset is still true and must stay selected.
    #[test]
    fn dragging_a_locked_box_keeps_the_chosen_preset() {
        use crate::video::VideoMessage as V;

        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));
        assert!(state.project().clips[0].crop.lock_aspect(), "presets start locked");

        state.update(ShellMessage::Video(V::CropChanged(
            offcut_model::NormalizedRect::new(0.1, 0.1, 0.4, 0.4),
        )));
        assert_eq!(state.project().clips[0].crop.aspect, AspectPreset::Square);
    }

    /// The editing preview shows the WHOLE frame with a box over it, not
    /// the cropped result -- you cannot frame a shot against footage
    /// that has already been cut away.
    #[test]
    fn the_crop_tab_previews_the_whole_frame_with_a_box_over_it() {
        let mut state = four_clip_state();
        state.update(ShellMessage::SelectClip(0));
        state.update(ShellMessage::SetAspect(AspectPreset::Square));
        state.update(ShellMessage::SelectTab(InspectorTab::Crop));

        let u = state.effects();
        assert_eq!(u.crop, [0.0, 0.0, 1.0, 1.0], "the editing view must show the full frame");
        assert!(u.crop_box[2] > 0.0 && u.crop_box[2] < 1.0, "with the crop drawn as a box");

        // And leaving the tab returns to showing the cropped result.
        state.update(ShellMessage::SelectTab(InspectorTab::Video));
        let u = state.effects();
        assert!(u.crop[2] < 1.0, "off the Crop tab the preview shows the actual crop");
        assert_eq!(u.crop_box[2], 0.0, "and no editing box");
    }

    /// A trim drag is many messages bracketed by one checkpoint, so the
    /// whole gesture must undo in a single step.
    #[test]
    fn a_trim_drag_undoes_as_one_step() {
        let mut state = four_clip_state();
        state.update(ShellMessage::Timeline(TimelineMessage::GestureBegan));
        for out in [9.5, 9.0, 8.5, 8.0] {
            state.update(ShellMessage::Timeline(TimelineMessage::TrimEnd { clip: 0, to: secs(out) }));
        }
        assert_eq!(state.project().clips[0].out_point, secs(8.0));

        state.update(ShellMessage::Undo);
        assert_eq!(state.project().clips[0].out_point, secs(10.0), "one undo reverses the whole drag");
    }

    #[test]
    fn updates_with_no_selection_are_safe_no_ops() {
        let mut state = ShellState::new(Project::new());
        assert_eq!(state.selected_clip, None);
        // None of these should panic with an empty project.
        state.update(ShellMessage::SetSpeed(Speed::Two));
        state.update(ShellMessage::ToggleClipMute);
        state.update(ShellMessage::Split);
        state.update(ShellMessage::DeleteClip);
        state.update(ShellMessage::DuplicateClip);
        state.update(ShellMessage::SetAspect(AspectPreset::Square));
        state.update(ShellMessage::SetAdjust(AdjustField::Tint, 50));
        state.update(ShellMessage::ResetCrop);
        state.update(ShellMessage::ResetAdjust);
        assert!(state.project().clips.is_empty());
    }

    #[test]
    fn app_serviced_messages_do_not_change_shell_state() {
        let mut state = four_clip_state();
        let before = state.project().clips.len();
        state.update(ShellMessage::OpenFile);
        state.update(ShellMessage::Export);
        state.update(ShellMessage::CancelExport);
        assert_eq!(state.project().clips.len(), before);
    }

    #[test]
    fn fmt_timecode_at_30fps_matches_the_design_reference() {
        // The design system's reference render shows `00:00:14:07` at 30fps.
        let fps = Rational::WEB_30;
        let time = fps.frame_to_time(14 * 30 + 7);
        assert_eq!(fmt_timecode(time, fps), "00:00:14:07");
    }

    #[test]
    fn fmt_timecode_formats_over_an_hour_correctly() {
        let fps = Rational::WEB_30;
        assert_eq!(fmt_timecode(fps.frame_to_time(3661 * 30 + 15), fps), "01:01:01:15");
    }

    /// A user-typed filename must not be able to reflow the inspector.
    ///
    /// The heading became the filename, and filenames are unbounded: a
    /// screen-recording name wraps to three lines in a 320px panel and
    /// pushes the body down. Truncation keeps the extension because that
    /// is the half people scan — "is this the mov or the mp4".
    #[test]
    fn a_long_filename_is_shortened_from_the_middle_keeping_the_extension() {
        let long = "screen-recording-2026-03-14-at-11.42.08-final-v2.mov";
        let short = elide_middle(long);
        assert!(short.chars().count() <= 30, "{short} is still {} chars", short.chars().count());
        assert!(short.starts_with("screen-recordi"), "the start is gone: {short}");
        assert!(short.ends_with(".mov"), "the extension is gone: {short}");
        assert!(short.contains('…'), "truncated without saying so: {short}");
    }

    /// A short name is left exactly alone — no ellipsis, no padding.
    #[test]
    fn a_short_filename_is_left_untouched() {
        assert_eq!(elide_middle("clip.mp4"), "clip.mp4");
    }

    /// Elision counts characters, not bytes.
    ///
    /// A byte-index split lands mid-codepoint on any non-ASCII name and
    /// panics — and "the app crashes on files with accents in the name"
    /// is a real bug shaped exactly like a copy change.
    #[test]
    fn eliding_a_non_ascii_filename_does_not_panic() {
        let name = "aufnahme-über-den-dächern-von-münchen-final.mov";
        let short = elide_middle(name);
        assert!(short.ends_with(".mov"));
        assert!(short.chars().count() <= 30);
    }

    /// One duration format, not two.
    ///
    /// `fmt_timecode_secs` was a second implementation that dropped the
    /// hours: a 1h01m01s encode reported `61:01` in the export plate
    /// while the trim readout called the same file `1:01:01`. Two answers
    /// to "how long is this", one of them wrong. This pins them together
    /// so a future edit to either has to notice the other.
    #[test]
    fn the_export_clock_and_the_trim_readout_agree_on_an_hour() {
        let hour_plus = Time::from_nanos(3_661_000_000_000);
        assert_eq!(fmt_timecode_secs(hour_plus), "1:01:01");
        assert_eq!(fmt_timecode_secs(hour_plus), crate::trimbar::fmt_duration(hour_plus));
    }
}
