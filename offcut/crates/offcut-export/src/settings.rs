//! Export presets: what the export sheet offers, as values rather than
//! as strings scattered through pipeline-description formatting.
//!
//! The design rule: "Presets: *Same as source*, 1080p, 720p, 480p ×
//! {H.264, HEVC}." The closed-set discipline `Speed` and `AspectPreset`
//! already follow applies here for the same reason — a free-text bitrate
//! field is an invitation to produce a file no player will open.

use offcut_model::{Rational, Time};

/// Output resolution. `SameAsSource` is deliberately the default: the
/// least surprising export of a 1080p clip is a 1080p file.
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum ResolutionPreset {
    #[default]
    SameAsSource,
    P1080,
    P720,
    P480,
}

impl ResolutionPreset {
    pub const ALL: [ResolutionPreset; 4] = [
        ResolutionPreset::SameAsSource,
        ResolutionPreset::P1080,
        ResolutionPreset::P720,
        ResolutionPreset::P480,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ResolutionPreset::SameAsSource => "Same as source",
            ResolutionPreset::P1080 => "1080p",
            ResolutionPreset::P720 => "720p",
            ResolutionPreset::P480 => "480p",
        }
    }

    /// Target height, or `None` for "keep the source's".
    pub fn height(self) -> Option<u32> {
        match self {
            ResolutionPreset::SameAsSource => None,
            ResolutionPreset::P1080 => Some(1080),
            ResolutionPreset::P720 => Some(720),
            ResolutionPreset::P480 => Some(480),
        }
    }

    /// Resolve against a real source resolution, preserving aspect ratio.
    ///
    /// Both dimensions are rounded to **even** numbers. This is not
    /// fussiness: H.264 4:2:0 chroma subsampling requires even dimensions,
    /// and `x264enc` fed an odd width fails to negotiate with an error
    /// that names caps rather than the actual problem. A 1920×1039 source
    /// scaled to 720p is 1330.7×720 → 1330×720 here, which encodes;
    /// 1331×720 does not.
    pub fn resolve(self, source: (u32, u32)) -> (u32, u32) {
        let (sw, sh) = (source.0.max(2), source.1.max(2));
        let even = |v: u32| (v / 2 * 2).max(2);
        match self.height() {
            None => (even(sw), even(sh)),
            Some(target_h) => {
                if sh <= target_h {
                    // Never upscale: exporting a 480p clip at "1080p"
                    // produces a bigger file with no more detail.
                    return (even(sw), even(sh));
                }
                let width = (sw as f64 * target_h as f64 / sh as f64).round() as u32;
                (even(width), even(target_h))
            }
        }
    }
}

/// Output container.
///
/// # Why this is a small, closed set and not "every muxer GStreamer has"
///
/// A container is only meaningful in combination with a codec, and most
/// combinations are either illegal or a trap. WebM may carry VP8/VP9 and
/// Opus and nothing else; putting H.264 in it produces a file the spec
/// forbids and browsers refuse. Offcut encodes H.264 and HEVC, so the
/// containers offered are the ones those two codecs legitimately live
/// in — MP4, QuickTime, and Matroska.
///
/// MKV is included because it accepts both codecs without caveat and is
/// the honest answer for "I want a container that will not argue".
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum Container {
    #[default]
    Mp4,
    Mov,
    Mkv,
}

impl Container {
    pub const ALL: [Container; 3] = [Container::Mp4, Container::Mov, Container::Mkv];

    pub fn label(self) -> &'static str {
        match self {
            Container::Mp4 => "MP4",
            Container::Mov => "MOV",
            Container::Mkv => "MKV",
        }
    }

    /// The filename extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Mkv => "mkv",
        }
    }

    /// The GStreamer muxer element.
    ///
    /// `qtmux` rather than `mp4mux` for MOV: they are siblings in the
    /// same plugin, and `mp4mux` writes an ISO-BMFF brand that some
    /// QuickTime-era tools reject in a file named `.mov`. A container
    /// choice that produces the wrong brand is worse than not offering
    /// the choice.
    pub fn muxer_element(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4mux",
            Container::Mov => "qtmux",
            Container::Mkv => "matroskamux",
        }
    }

    /// Whether the AAC stream needs `aacparse` before the muxer.
    ///
    /// The ISO-BMFF muxers want stream metadata the parser supplies;
    /// Matroska accepts the encoder's output directly and does not need
    /// an element inserted to satisfy it.
    pub fn needs_aac_parser(self) -> bool {
        match self {
            Container::Mp4 | Container::Mov => true,
            Container::Mkv => false,
        }
    }

    /// Whether this container can legally carry `codec`.
    ///
    /// All three currently accept both codecs, so this is uniformly
    /// true — but it is a **function, not a comment**, because the moment
    /// a VP9 or WebM option is added the honest answer stops being
    /// uniform, and a UI that offers an illegal pair produces a file no
    /// player opens. The export sheet asks this rather than assuming.
    pub fn accepts(self, codec: VideoCodec) -> bool {
        match self {
            // MP4 and MOV are both ISO-BMFF derivatives: H.264 and HEVC
            // are their native cases.
            Container::Mp4 | Container::Mov => {
                matches!(codec, VideoCodec::H264 | VideoCodec::Hevc)
            }
            // Matroska is codec-agnostic by design.
            Container::Mkv => true,
        }
    }
}

/// Video codec for the output. Both are MP4-muxable, per the product's
/// "Export: MP4/H.264 and MP4/HEVC".
#[derive(Copy, Clone, PartialEq, Eq, Debug, Default)]
pub enum VideoCodec {
    #[default]
    H264,
    Hevc,
}

impl VideoCodec {
    pub const ALL: [VideoCodec; 2] = [VideoCodec::H264, VideoCodec::Hevc];

    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
        }
    }

    /// The **software** encoder element name. The design is explicit
    /// that software is the shipping default and hardware is a
    /// runtime-probed upgrade, "never silently" — `hardware_element`
    /// below is the upgrade, and `Encoder::choose` is where the probe
    /// happens.
    pub fn software_element(self) -> &'static str {
        match self {
            VideoCodec::H264 => "x264enc",
            VideoCodec::Hevc => "x265enc",
        }
    }

    /// The VA-API encoder element, if this machine has it.
    pub fn hardware_element(self) -> &'static str {
        match self {
            VideoCodec::H264 => "vah264enc",
            VideoCodec::Hevc => "vah265enc",
        }
    }

    /// The parser between encoder and muxer. `mp4mux` needs
    /// AVC/HVC1-formatted stream metadata, which the parser supplies;
    /// omitting it produces an MP4 that some players open and others
    /// reject, which is worse than failing.
    pub fn parser_element(self) -> &'static str {
        match self {
            VideoCodec::H264 => "h264parse",
            VideoCodec::Hevc => "h265parse",
        }
    }
}

/// The AAC encoder to mux audio with, in preference order.
///
/// There is no user-facing choice here and deliberately so: the product rules
/// keeps the export surface small, and every one of these produces AAC in
/// an MP4. Which element is used depends only on what this machine has,
/// which is why the list is ordered rather than configurable — the first
/// present element wins.
///
/// `avenc_aac` (gst-libav) leads because it is the one most reliably
/// installed alongside the H.264 decoder this app already requires, so a
/// machine that can *open* an MP4 can almost always *write* its audio too.
pub const AAC_ENCODERS: &[&str] = &["avenc_aac", "fdkaacenc", "faac", "voaacenc"];

/// Everything the export sheet collects, in one value.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportSettings {
    pub resolution: ResolutionPreset,
    /// The output container. Paired with `codec` by `Container::accepts`,
    /// which the export sheet consults rather than assuming.
    pub container: Container,
    pub codec: VideoCodec,
    /// Target video bitrate in kbit/s.
    pub bitrate_kbps: u32,
    /// Target audio bitrate in bit/s (AAC). 128 kbps stereo is
    /// transparent enough for the screen recordings and phone footage
    /// the product targets, and is the near-universal default.
    pub audio_bitrate_bps: u32,
    /// Honor the project's master mute / per-clip mutes by writing
    /// silence. The design rule: "export writes silence for muted spans so
    /// the audio stream stays continuous and players do not glitch at
    /// clip boundaries."
    pub include_audio: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            resolution: ResolutionPreset::default(),
            container: Container::default(),
            codec: VideoCodec::default(),
            bitrate_kbps: 8_000,
            audio_bitrate_bps: 128_000,
            include_audio: true,
        }
    }
}

impl ExportSettings {
    /// A bitrate that suits the resolved output size, used when the user
    /// picks a resolution preset rather than typing a number. These are
    /// deliberately generous — a visibly soft export of a screen
    /// recording is a worse failure than a file a few MB larger.
    pub fn suggested_bitrate_kbps(resolution: (u32, u32), codec: VideoCodec) -> u32 {
        let pixels = resolution.0 as u64 * resolution.1 as u64;
        let h264 = match pixels {
            p if p >= 1920 * 1080 => 12_000,
            p if p >= 1280 * 720 => 6_000,
            p if p >= 854 * 480 => 3_000,
            _ => 1_500,
        };
        match codec {
            VideoCodec::H264 => h264,
            // HEVC reaches comparable quality at roughly 60% the bitrate.
            VideoCodec::Hevc => (h264 * 3) / 5,
        }
    }
}

/// Progress reported while an export runs.
#[derive(Copy, Clone, PartialEq, Debug)]
pub struct ExportProgress {
    /// Timeline position written so far.
    pub position: Time,
    pub total: Time,
}

impl ExportProgress {
    /// `0.0..=1.0`. Guaranteed finite even for an empty timeline, because
    /// a progress bar fed a NaN renders as a full bar, which reads as
    /// "done" at the exact moment nothing has happened.
    pub fn fraction(self) -> f32 {
        if self.total.as_nanos() == 0 {
            return 0.0;
        }
        (self.position.as_nanos() as f64 / self.total.as_nanos() as f64).clamp(0.0, 1.0) as f32
    }

    /// Seconds remaining, estimated from elapsed wall time. `None` before
    /// enough progress exists to extrapolate from — showing "ETA 0:00" at
    /// 0% is a lie the UI should not have to launder.
    pub fn eta_secs(self, elapsed_secs: f64) -> Option<f64> {
        let fraction = self.fraction() as f64;
        if fraction <= 0.01 || elapsed_secs <= 0.0 {
            return None;
        }
        Some((elapsed_secs / fraction - elapsed_secs).max(0.0))
    }
}

/// Output frame rate for the export: the first source's rate, or 30 for
/// an empty project. Mixed-rate timelines are normalized to the first
/// clip's rate by `videorate`, which is the honest v1 behavior —
/// The design rule flags variable/mixed frame rate as a real hazard and this
/// is the simple, predictable answer rather than a silent per-segment
/// rate change the muxer would have to reconcile.
pub fn output_framerate(project: &offcut_model::Project) -> Rational {
    project.sources.first().map(|s| s.fps).unwrap_or(Rational::WEB_30)
}

/// The first AAC encoder from [`AAC_ENCODERS`] this machine actually has,
/// or `None` if it has none.
///
/// `None` is not an export failure. It downgrades the export to
/// video-only, because a silent file the user can play beats an error
/// telling them to install a plugin they cannot install — the same
/// runtime-probe-and-degrade posture `caps.rs` takes for hardware encode.
pub fn available_aac_encoder(caps: &offcut_engine::Capabilities) -> Option<&'static str> {
    AAC_ENCODERS.iter().copied().find(|e| caps.has(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The extension, the muxer, and the label must describe one format.
    ///
    /// A `.mov` file written by `mp4mux`, or an MKV named `.mp4`, is a
    /// file whose name lies about its contents — and players trust the
    /// name before they trust the bytes.
    #[test]
    fn every_container_agrees_with_itself() {
        for c in Container::ALL {
            let (ext, mux, label) = (c.extension(), c.muxer_element(), c.label());
            assert!(!ext.starts_with('.'), "{label}: extension carries its own dot: {ext}");
            assert_eq!(ext, label.to_ascii_lowercase(), "{label}: extension and label disagree");
            assert!(!mux.is_empty());
        }
        // MOV uses qtmux, not mp4mux: they are siblings, and mp4mux
        // writes an ISO-BMFF brand some QuickTime-era tools reject in a
        // file named .mov.
        assert_eq!(Container::Mov.muxer_element(), "qtmux");
        assert_eq!(Container::Mp4.muxer_element(), "mp4mux");
    }

    /// No two containers may share a muxer or an extension.
    #[test]
    fn the_containers_are_actually_distinct_formats() {
        for (i, a) in Container::ALL.iter().enumerate() {
            for b in &Container::ALL[i + 1..] {
                assert_ne!(a.extension(), b.extension());
                assert_ne!(a.muxer_element(), b.muxer_element());
            }
        }
    }

    /// Every container must accept at least one codec Offcut can encode.
    ///
    /// A format offered in the sheet that can carry nothing this app
    /// produces is a chip that guarantees a failed export — the class of
    /// defect `accepts` exists to make impossible rather than to
    /// document.
    #[test]
    fn every_container_can_carry_something_this_app_encodes() {
        for c in Container::ALL {
            assert!(
                VideoCodec::ALL.into_iter().any(|codec| c.accepts(codec)),
                "{} accepts neither codec Offcut encodes, so choosing it cannot succeed",
                c.label()
            );
        }
    }

    /// Only the ISO-BMFF containers ask for `aacparse`.
    #[test]
    fn the_aac_parser_is_requested_only_where_it_is_needed() {
        assert!(Container::Mp4.needs_aac_parser());
        assert!(Container::Mov.needs_aac_parser());
        assert!(!Container::Mkv.needs_aac_parser(), "matroskamux takes raw AAC");
    }

    #[test]
    fn same_as_source_keeps_the_source_resolution() {
        assert_eq!(ResolutionPreset::SameAsSource.resolve((1920, 1080)), (1920, 1080));
        assert_eq!(ResolutionPreset::SameAsSource.resolve((640, 360)), (640, 360));
    }

    #[test]
    fn presets_preserve_aspect_ratio() {
        assert_eq!(ResolutionPreset::P720.resolve((1920, 1080)), (1280, 720));
        // 852, not the "854" commonly quoted for 480p 16:9: 1920*480/1080
        // is 853.33, which rounds to 853 and then evens *down* to 852.
        // 854 is what you get by rounding 853.33 up to 854 to stay even,
        // and it is very slightly wider than 16:9. Rounding down keeps
        // the output no wider than the source's true aspect, which is the
        // safer direction — it letterboxes by at most one pixel rather
        // than stretching.
        assert_eq!(ResolutionPreset::P480.resolve((1920, 1080)), (852, 480));
        // 4:3 source
        assert_eq!(ResolutionPreset::P720.resolve((1440, 1080)), (960, 720));
    }

    /// The specific encoder failure this rounding exists to prevent.
    #[test]
    fn resolved_dimensions_are_always_even_for_420_chroma() {
        for source in [(1920, 1039), (1921, 1081), (999, 777), (3, 3)] {
            for preset in ResolutionPreset::ALL {
                let (w, h) = preset.resolve(source);
                assert_eq!(w % 2, 0, "{preset:?} of {source:?} gave odd width {w}");
                assert_eq!(h % 2, 0, "{preset:?} of {source:?} gave odd height {h}");
                assert!(w >= 2 && h >= 2);
            }
        }
    }

    #[test]
    fn presets_never_upscale() {
        // A 480p source exported "at 1080p" stays 480p.
        assert_eq!(ResolutionPreset::P1080.resolve((854, 480)), (854, 480));
        assert_eq!(ResolutionPreset::P720.resolve((640, 360)), (640, 360));
    }

    #[test]
    fn hevc_is_suggested_a_lower_bitrate_than_h264_at_the_same_size() {
        let h264 = ExportSettings::suggested_bitrate_kbps((1920, 1080), VideoCodec::H264);
        let hevc = ExportSettings::suggested_bitrate_kbps((1920, 1080), VideoCodec::Hevc);
        assert!(hevc < h264, "HEVC should need less bitrate: {hevc} vs {h264}");
    }

    #[test]
    fn suggested_bitrate_scales_down_with_resolution() {
        let big = ExportSettings::suggested_bitrate_kbps((1920, 1080), VideoCodec::H264);
        let mid = ExportSettings::suggested_bitrate_kbps((1280, 720), VideoCodec::H264);
        let small = ExportSettings::suggested_bitrate_kbps((640, 360), VideoCodec::H264);
        assert!(big > mid && mid > small);
    }

    #[test]
    fn progress_fraction_is_finite_and_clamped_even_for_a_zero_length_timeline() {
        let p = ExportProgress { position: Time::from_nanos(5), total: Time::ZERO };
        assert_eq!(p.fraction(), 0.0, "must not be NaN — a NaN renders as a full bar");

        let p = ExportProgress { position: Time::from_nanos(50), total: Time::from_nanos(10) };
        assert_eq!(p.fraction(), 1.0, "overshoot must clamp, not exceed 1");
    }

    #[test]
    fn progress_fraction_is_proportional() {
        let p = ExportProgress { position: Time::from_nanos(25), total: Time::from_nanos(100) };
        assert!((p.fraction() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn eta_is_none_until_there_is_enough_progress_to_extrapolate() {
        let p = ExportProgress { position: Time::ZERO, total: Time::from_nanos(100) };
        assert_eq!(p.eta_secs(3.0), None, "an ETA at 0% would be invented");
    }

    #[test]
    fn eta_extrapolates_linearly_from_elapsed_time() {
        // 25% done after 10s => ~30s remaining.
        let p = ExportProgress { position: Time::from_nanos(25), total: Time::from_nanos(100) };
        let eta = p.eta_secs(10.0).expect("should have an ETA at 25%");
        assert!((eta - 30.0).abs() < 0.5, "expected ~30s, got {eta}");
    }

    #[test]
    fn default_settings_are_the_least_surprising_ones() {
        let s = ExportSettings::default();
        assert_eq!(s.resolution, ResolutionPreset::SameAsSource);
        assert_eq!(s.codec, VideoCodec::H264);
        assert!(s.include_audio);
    }
}
