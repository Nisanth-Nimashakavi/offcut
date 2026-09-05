//! Runtime capability probing: which GStreamer elements this machine
//! actually has.
//!
//! **This module exists because of a real, checked environment surprise**
//! (2026-08-28) that contradicted the design rule: that section listed
//! `avdec_h264` as "confirmed present" via `gst-libav`, and `x264enc` via
//! `gst-plugins-ugly`. Only the second was true. This machine has
//! `gst-plugins-{base,bad,ugly}` but **not** `gst-plugins-good` and
//! **not** `gst-libav` — which means no `qtdemux`, no `mp4mux`, and no
//! `avdec_h264`: an editor that cannot open or write an MP4 at all. The
//! failure mode without this module is terrible: `gst::parse::launch`
//! returns a generic "no element" string mid-pipeline, or `uridecodebin`
//! silently produces no pads and the app just hangs on a black frame.
//!
//! So: probe explicitly, fail loudly, and say exactly which elements are
//! missing and how to fix it. `offcut/tools/setup-gst-plugins.sh` installs
//! the missing plugins into a user-local prefix without root.

use crate::error::EngineError;
use gstreamer as gst;

/// Elements required to *open and decode* common video files.
pub const REQUIRED_FOR_PLAYBACK: &[&str] = &[
    "uridecodebin",  // the demux+decode autoplugger
    "videoconvert",  // to RGBA for the texture upload path
    "appsink",       // frame delivery into offcut-render
    "volume",        // per-clip / master mute
];

/// Elements required to *export* an edited timeline.
pub const REQUIRED_FOR_EXPORT: &[&str] = &[
    "x264enc",       // The design rule: software H.264 is the shipping default
    "mp4mux",        // the container
    "videorate",     // speed changes
    "audioconvert",
    "audioresample",
];

/// Elements that meaningfully improve things but are not fatal if absent.
pub const OPTIONAL: &[&str] = &[
    "qtdemux",       // present via gst-plugins-good; uridecodebin needs it for MP4
    // The alternate output containers. `mp4mux` is required above; these
    // two are optional because their absence should disable a format
    // rather than block export entirely.
    "qtmux",         // MOV
    "matroskamux",   // MKV
    "avdec_h264",    // gst-libav H.264 decode
    // The AAC encoders offcut-export tries, in order. All four are listed
    // here rather than just the preferred one because `Capabilities::has`
    // reports `true` for any element it was never asked to probe — so an
    // element the export intends to *choose between* must appear in one
    // of these lists or the choice is made on stale optimism.
    "avenc_aac",     // gst-libav AAC encode
    "fdkaacenc",
    "faac",
    "voaacenc",
    "aacparse",      // between the AAC encoder and mp4mux
    "pitch",         // pitch-preserving tempo for speed
    // The HEVC export path, end to end. `x265enc` was listed and its two
    // companions were not, which is the exact failure `has` warns about
    // below: an unprobed element reports **present**, so `build` chose
    // `vah265enc` on a machine without it and `gst::parse::launch` failed
    // with "no element" — surfaced to the user as "Export failed: failed
    // to build the encode pipeline". H.264 was unaffected because all
    // three of its elements were listed.
    "x265enc",       // HEVC software encode
    "vah265enc",     // VA-API HEVC encode
    "h265parse",     // between the HEVC encoder and mp4mux
    "h264parse",     // likewise for H.264
    "vah264dec",     // VA-API hardware decode, if the user installed it
    "vah264enc",     // VA-API hardware encode
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capabilities {
    pub missing_required_playback: Vec<String>,
    pub missing_required_export: Vec<String>,
    pub missing_optional: Vec<String>,
}

impl Capabilities {
    pub fn can_play(&self) -> bool {
        self.missing_required_playback.is_empty()
    }

    pub fn can_export(&self) -> bool {
        self.missing_required_playback.is_empty() && self.missing_required_export.is_empty()
    }

    pub fn has(&self, element: &str) -> bool {
        !self.missing_required_playback.iter().any(|m| m == element)
            && !self.missing_required_export.iter().any(|m| m == element)
            && !self.missing_optional.iter().any(|m| m == element)
    }

    /// A human-readable explanation suitable for showing a user, naming
    /// the fix rather than just the failure — the "error surfaces" quality
    /// the error-surface quality this build asks for, applied at the one place it's most
    /// likely to bite a fresh install.
    pub fn diagnosis(&self) -> String {
        if self.can_export() {
            return "All required GStreamer elements are present.".to_string();
        }
        let mut s = String::from("Missing required GStreamer elements:\n");
        for m in self.missing_required_playback.iter().chain(self.missing_required_export.iter()) {
            s.push_str(&format!("  - {m}\n"));
        }
        s.push_str(
            "\nOn Arch-family systems these live in `gst-plugins-good` and `gst-libav`.\n\
             Install system-wide with:  sudo pacman -S gst-plugins-good gst-libav\n\
             Or, without root, run this repo's `offcut/tools/setup-gst-plugins.sh`,\n\
             which extracts them into a user-local prefix and prints the\n\
             GST_PLUGIN_PATH to export.\n",
        );
        s
    }

    /// The same finding as `diagnosis`, in **one line**, for a status
    /// pill that has no room for the full text.
    ///
    /// The pill previously showed `first_line(&diagnosis())`, which is
    /// the string `"Missing required GStreamer elements:"` — a sentence
    /// ending in a colon that introduces a list the user never sees. It
    /// named neither what was missing nor what to do, so the one message
    /// standing between a fresh install and a black window said only
    /// that something was wrong.
    ///
    /// This names the package to install, because that is the whole of
    /// the recovery and it fits.
    pub fn short_diagnosis(&self) -> String {
        if self.can_export() {
            return "All required GStreamer elements are present.".to_string();
        }
        let missing: Vec<&str> = self
            .missing_required_playback
            .iter()
            .chain(self.missing_required_export.iter())
            .map(String::as_str)
            .collect();
        let what = match missing.as_slice() {
            [] => "GStreamer plugins are".to_string(),
            [one] => format!("The GStreamer plugin `{one}` is"),
            [a, b] => format!("The GStreamer plugins `{a}` and `{b}` are"),
            [a, b, rest @ ..] => format!(
                "`{a}`, `{b}` and {} more GStreamer plugins are",
                rest.len()
            ),
        };
        format!("{what} missing, so video cannot play or export. Install gst-plugins-good and gst-libav.")
    }
}

fn element_exists(name: &str) -> bool {
    gst::ElementFactory::find(name).is_some()
}

/// Probe the current GStreamer registry. Requires `gst::init()` to have
/// run; call `crate::pipeline::ensure_gst_init` first (or just use
/// `probe()`, which does it for you).
pub fn probe() -> Result<Capabilities, EngineError> {
    crate::pipeline::ensure_gst_init()?;

    let collect = |list: &[&str]| -> Vec<String> {
        list.iter().filter(|e| !element_exists(e)).map(|e| e.to_string()).collect()
    };

    Ok(Capabilities {
        missing_required_playback: collect(REQUIRED_FOR_PLAYBACK),
        missing_required_export: collect(REQUIRED_FOR_EXPORT),
        missing_optional: collect(OPTIONAL),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This is an environment assertion as much as a code test: it fails
    /// loudly if the machine running the suite cannot actually play video,
    /// which is far more useful than every downstream pipeline test
    /// failing with a confusing parse error.
    #[test]
    fn required_playback_elements_are_present() {
        let caps = probe().expect("probe failed");
        assert!(
            caps.can_play(),
            "this machine cannot decode video with GStreamer.\n{}",
            caps.diagnosis()
        );
    }

    #[test]
    fn required_export_elements_are_present() {
        let caps = probe().expect("probe failed");
        assert!(
            caps.can_export(),
            "this machine cannot export video with GStreamer.\n{}",
            caps.diagnosis()
        );
    }

    /// **Every element the export pipeline names must be probed.**
    ///
    /// # The bug this exists to prevent, stated exactly
    ///
    /// `has` returns `true` for any element it was never asked about —
    /// it answers "is this in a missing list", and an unprobed element is
    /// in none of them. `x265enc` was listed; `vah265enc` and `h265parse`
    /// were not. So on a machine with no VA-API, `has("vah265enc")` said
    /// **true**, the encoder chose the hardware element that did not
    /// exist, and `gst::parse::launch` failed with a generic error the
    /// user saw as "Export failed: failed to build the encode pipeline".
    ///
    /// H.264 was unaffected purely because all three of its elements
    /// happened to be listed — so the defect looked like "HEVC is
    /// broken" rather than like a hole in this list.
    ///
    /// This asserts the list covers the pipeline rather than trusting
    /// that it does. Adding a codec or container means adding its
    /// elements here, and this test is what says so.
    #[test]
    fn every_element_the_export_pipeline_can_name_is_probed() {
        let probed: Vec<&str> = REQUIRED_FOR_PLAYBACK
            .iter()
            .chain(REQUIRED_FOR_EXPORT.iter())
            .chain(OPTIONAL.iter())
            .copied()
            .collect();

        // Encoders, parsers, and muxers the export crate can put in a
        // pipeline description. Spelled out rather than imported so this
        // crate keeps no dependency on offcut-export.
        for element in [
            "x264enc", "x265enc", "vah264enc", "vah265enc",
            "h264parse", "h265parse",
            "mp4mux", "qtmux", "matroskamux",
            "aacparse",
        ] {
            assert!(
                probed.contains(&element),
                "`{element}` can appear in an export pipeline but is in no probe list, so \
                 `has(\"{element}\")` returns true on a machine without it and the export \
                 fails at launch with a generic error"
            );
        }
    }

    #[test]
    fn diagnosis_is_reassuring_when_nothing_is_missing() {
        let caps = Capabilities {
            missing_required_playback: vec![],
            missing_required_export: vec![],
            missing_optional: vec!["vah264dec".into()],
        };
        assert!(caps.can_play() && caps.can_export());
        assert!(caps.diagnosis().contains("All required"));
    }

    /// The pill has one line, and that line has to carry the recovery.
    ///
    /// It used to show `first_line(&diagnosis())` — literally "Missing
    /// required GStreamer elements:", a colon introducing a list that was
    /// then thrown away. The user learned that something was missing and
    /// nothing else.
    #[test]
    fn the_one_line_diagnosis_names_both_the_cause_and_the_fix() {
        let caps = Capabilities {
            missing_required_playback: vec!["uridecodebin".into()],
            missing_required_export: vec!["x264enc".into()],
            missing_optional: vec![],
        };
        let short = caps.short_diagnosis();
        assert!(!short.contains('\n'), "a status pill cannot show a second line: {short}");
        assert!(short.contains("uridecodebin"), "does not say what is missing: {short}");
        assert!(short.contains("gst-plugins-good"), "does not say how to fix it: {short}");
        assert!(!short.ends_with(':'), "ends mid-sentence: {short}");
    }

    /// Many missing plugins must not produce a pill longer than the
    /// toolbar. The count carries the rest.
    #[test]
    fn a_long_missing_list_is_summarised_rather_than_enumerated() {
        let caps = Capabilities {
            missing_required_playback: vec!["a".into(), "b".into(), "c".into(), "d".into()],
            missing_required_export: vec![],
            missing_optional: vec![],
        };
        let short = caps.short_diagnosis();
        assert!(short.contains("2 more"), "{short}");
        assert!(short.len() < 160, "too long for a status pill: {short}");
    }

    #[test]
    fn diagnosis_names_the_fix_when_something_is_missing() {
        let caps = Capabilities {
            missing_required_playback: vec!["uridecodebin".into()],
            missing_required_export: vec![],
            missing_optional: vec![],
        };
        assert!(!caps.can_play());
        let d = caps.diagnosis();
        assert!(d.contains("uridecodebin"), "diagnosis should name the missing element");
        assert!(d.contains("setup-gst-plugins.sh"), "diagnosis should name the no-root fix");
    }

    #[test]
    fn has_reports_per_element_availability() {
        let caps = Capabilities {
            missing_required_playback: vec![],
            missing_required_export: vec![],
            missing_optional: vec!["vah264enc".into()],
        };
        assert!(caps.has("x264enc"));
        assert!(!caps.has("vah264enc"));
    }
}
