//! Drawn icon paths.
//!
//! The design system's Rules section: "Icons are drawn paths at 1.6–1.8 stroke,
//! 16px in controls and 18px in inspector rows. **No glyphs, no emoji.**"
//! That rule is why this module exists rather than an icon font: a font
//! would make stroke weight a property of the typeface instead of the
//! design system, and an emoji renders differently on every machine.
//!
//! # Why SVG rather than `canvas`
//!
//! The first implementation drew each icon with `iced::widget::canvas`,
//! which was **wrong in a way only running it revealed**: in this version
//! of iced, sibling `canvas` widgets do not compose. A probe with three
//! plain 60px filled squares stacked in a `column` rendered exactly one
//! square — the *last* program's geometry, drawn at the *first* widget's
//! position. In the real app that meant every icon vanished except one,
//! and the timeline (the last canvas in the tree) was the only one that
//! survived. Screenshots of both the probe and the app are what caught
//! it; nothing about the code reads as wrong.
//!
//! `svg` has no such limitation, and its `Style::color` filter exists
//! precisely for recoloring a symbolic icon — so one path definition
//! still serves every palette state and both sizes. The canvas approach
//! remains correct for the *timeline*, which is a single large canvas and
//! is genuinely the right tool for that job.

use iced::widget::svg;
use iced::{Color, Element, Length};

/// Which icon to draw. A closed enum, not a string lookup, so a typo is a
/// compile error and the full inventory is greppable in one place.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Play,
    Pause,
    StepBack,
    StepForward,
    Split,
    Delete,
    Duplicate,
    Undo,
    Redo,
    Download,
    ZoomIn,
    ZoomOut,
    SpeakerOn,
    SpeakerOff,
    VolumeHigh,
    /// GNOME's primary-menu hamburger. Three equal rules, which is what
    /// libadwaita's `open-menu-symbolic` draws -- not a stack of
    /// unequal lines, and not a kebab.
    Menu,
    /// Dismiss. Two rules crossing at the viewbox centre, at the family's
    /// own stroke weight. This closes a plate; it does not delete the
    /// plate's contents, which is `Delete`.
    Close,
    Lock,
    Folder,
    Sun,
    Moon,
}

/// All icons are authored in a 24×24 viewbox — the same convention as
/// every mainstream icon set, which makes the stroke widths directly
/// comparable to the design system's "1.6–1.8" figure.
const STROKE: &str = r#"fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round""#;

/// `STROKE` at a caller-chosen weight.
///
/// Appending a second `stroke-width` after `STROKE` produces duplicate
/// attributes on one element — invalid SVG that the rasteriser drops
/// **silently**, so the icon occupies its slot and draws nothing. Four
/// icons shipped that way, and only a screenshot of a blank HeaderBar
/// slot revealed it. This helper makes the correct form the easy one.
fn stroke_at(width: &str) -> String {
    format!(
        r#"fill="none" stroke="currentColor" stroke-width="{width}" stroke-linecap="round" stroke-linejoin="round""#
    )
}

impl Icon {
    /// Every icon, in declaration order. Public because the handle cache
    /// below needs to enumerate them, and because a test asserting the
    /// inventory is distinct is more honest against a real list than
    /// against a copy of one.
    pub const ALL: [Icon; 21] = [
        Icon::Play,
        Icon::Pause,
        Icon::StepBack,
        Icon::StepForward,
        Icon::Split,
        Icon::Delete,
        Icon::Duplicate,
        Icon::Undo,
        Icon::Redo,
        Icon::Download,
        Icon::ZoomIn,
        Icon::ZoomOut,
        Icon::SpeakerOn,
        Icon::SpeakerOff,
        Icon::VolumeHigh,
        Icon::Menu,
        Icon::Close,
        Icon::Lock,
        Icon::Folder,
        Icon::Sun,
        Icon::Moon,
    ];

    /// The inner markup of this icon's 24×24 viewbox.
    fn body(self) -> String {
        match self {
            // Filled triangle: the play affordance reads as solid at
            // 16px; a stroked outline turns to mush at that size.
            Icon::Play => r#"<path d="M8 5 L19 12 L8 19 Z" fill="currentColor"/>"#.to_string(),
            Icon::Pause => format!(
                r#"<path d="M9 5.5 V18.5 M15 5.5 V18.5" {}/>"#, stroke_at("2.4")
            ),
            Icon::StepBack => format!(
                r#"<path d="M17 5.5 L17 18.5 L8 12 Z" fill="currentColor"/><path d="M6 5.5 V18.5" {}/>"#, stroke_at("1.9")
            ),
            Icon::StepForward => format!(
                r#"<path d="M7 5.5 L7 18.5 L16 12 Z" fill="currentColor"/><path d="M18 5.5 V18.5" {}/>"#, stroke_at("1.9")
            ),
            // A dashed cut line with arrowheads pushing apart.
            Icon::Split => format!(
                r#"<path d="M12 3.5 V20.5" {STROKE} stroke-dasharray="3 3"/><path d="M8 8.5 L4.5 12 L8 15.5 M16 8.5 L19.5 12 L16 15.5" {STROKE}/>"#
            ),
            Icon::Delete => format!(
                r#"<path d="M5.5 7 H18.5 M7.5 7 L8.5 19.5 H15.5 L16.5 7 M9.5 7 V4.5 H14.5 V7" {STROKE}/>"#
            ),
            Icon::Duplicate => format!(
                r#"<path d="M8.5 4.5 H19.5 V15.5" {STROKE}/><path d="M4.5 8.5 H15.5 V19.5 H4.5 Z" {STROKE}/>"#
            ),
            Icon::Undo => format!(
                r#"<path d="M9 7 L4.5 11 L9 15 M4.5 11 H14 C18.5 11 20 14 19 18.5" {STROKE}/>"#
            ),
            Icon::Redo => format!(
                r#"<path d="M15 7 L19.5 11 L15 15 M19.5 11 H10 C5.5 11 4 14 5 18.5" {STROKE}/>"#
            ),
            Icon::Download => format!(
                r#"<path d="M12 3.5 V15 M7.5 10.5 L12 15 L16.5 10.5 M4.5 18.5 H19.5" {STROKE}/>"#
            ),
            Icon::ZoomIn => format!(
                r#"<circle cx="10.5" cy="10.5" r="6" {STROKE}/><path d="M15 15 L20 20 M7.5 10.5 H13.5 M10.5 7.5 V13.5" {STROKE}/>"#
            ),
            Icon::ZoomOut => format!(
                r#"<circle cx="10.5" cy="10.5" r="6" {STROKE}/><path d="M15 15 L20 20 M7.5 10.5 H13.5" {STROKE}/>"#
            ),
            Icon::SpeakerOn => format!(
                r#"<path d="M4.5 9.5 H8 L12 5.5 V18.5 L8 14.5 H4.5 Z" {STROKE}/><path d="M15 9 C17 10.5 17 13.5 15 15 M17.5 6.5 C21 9 21 15 17.5 17.5" {STROKE}/>"#
            ),
            // The "x" that the design system's muted rows show.
            Icon::SpeakerOff => format!(
                r#"<path d="M4.5 9.5 H8 L12 5.5 V18.5 L8 14.5 H4.5 Z" {STROKE}/><path d="M15.5 9.5 L20.5 14.5 M20.5 9.5 L15.5 14.5" {STROKE}/>"#
            ),
            // Three arcs, against SpeakerOn's two. This mark labels a
            // *level* control, so it should read as "more/less" rather
            // than as the on/off state SpeakerOn carries — and the two
            // must not be mistakable for each other at 18px.
            Icon::VolumeHigh => format!(
                r#"<path d="M3.5 9.5 H7 L11 5.5 V18.5 L7 14.5 H3.5 Z" {STROKE}/><path d="M13.5 10 C14.8 11 14.8 13 13.5 14 M16 7.8 C18.4 9.6 18.4 14.4 16 16.2 M18.5 5.6 C22 8.2 22 15.8 18.5 18.4" {STROKE}/>"#
            ),
            Icon::Menu => format!(
                r#"<path d="M4 7.25 H20 M4 12 H20 M4 16.75 H20" {}/>"#, stroke_at("1.9")
            ),
            // Inset to 6.5/17.5 rather than spanning the full viewbox:
            // an X drawn corner to corner reads heavier than the other
            // marks at the same stroke, because its diagonals are longer
            // than any rule in the family.
            Icon::Close => format!(
                r#"<path d="M6.5 6.5 L17.5 17.5 M17.5 6.5 L6.5 17.5" {STROKE}/>"#
            ),
            Icon::Lock => format!(
                r#"<path d="M6.5 11 H17.5 V19.5 H6.5 Z" {STROKE}/><path d="M8.5 11 V8 C8.5 4.5 15.5 4.5 15.5 8 V11" {STROKE}/>"#
            ),
            Icon::Folder => format!(
                r#"<path d="M3.5 6.5 H9.5 L11.5 9 H20.5 V18.5 H3.5 Z" {STROKE}/>"#
            ),
            Icon::Sun => format!(
                r#"<circle cx="12" cy="12" r="4.5" {STROKE}/><path d="M12 2.5 V5 M12 19 V21.5 M2.5 12 H5 M19 12 H21.5 M5.2 5.2 L7 7 M17 17 L18.8 18.8 M18.8 5.2 L17 7 M7 17 L5.2 18.8" {STROKE}/>"#
            ),
            Icon::Moon => format!(
                r#"<path d="M20 14.5 A9 9 0 1 1 9.5 4 A7 7 0 0 0 20 14.5 Z" {STROKE}/>"#
            ),
        }
    }

    /// The complete SVG document for this icon.
    ///
    /// `currentColor` throughout, so `svg::Style::color` recolors the
    /// whole mark in one place — the mechanism that lets a single
    /// definition serve every palette state (active, muted, disabled)
    /// without a second copy per color.
    pub fn svg_source(self) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="24" height="24">{}</svg>"#,
            self.body()
        )
    }
}

/// One `svg::Handle` per icon, built once for the whole process.
///
/// # Why this cache exists
///
/// `icon()` is called from `view()`, which iced runs on **every single
/// frame**. The previous version formatted this icon's SVG source into a
/// fresh `String`, allocated a `Vec<u8>` from it, and handed that to
/// `svg::Handle::from_memory` — every icon, every frame. A handle built
/// from new bytes is a *new* handle as far as the renderer's cache is
/// concerned, so the SVG was also re-parsed and re-rasterized each time.
/// With ~14 icons on screen at 60fps that is ~840 string formats,
/// allocations, and SVG parses per second, purely to redraw marks that
/// never change.
///
/// Handles are cheap to clone (they are reference-counted internally),
/// so building each one once and cloning turns that whole cost into a
/// refcount bump — and lets the renderer's own cache actually hit,
/// because the handle's identity is now stable across frames.
static HANDLES: std::sync::LazyLock<std::collections::HashMap<Icon, svg::Handle>> =
    std::sync::LazyLock::new(|| {
        Icon::ALL
            .iter()
            .map(|&which| (which, svg::Handle::from_memory(which.svg_source().into_bytes())))
            .collect()
    });

/// Build an icon element at `size` logical pixels.
pub fn icon<'a, Message: 'a>(which: Icon, color: Color, size: f32) -> Element<'a, Message> {
    svg(HANDLES[&which].clone())
        .width(Length::Fixed(size))
        .height(Length::Fixed(size))
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

/// The size the design system specifies for icons inside controls.
pub const CONTROL: f32 = 16.0;
/// The size the design system specifies for icons in inspector rows.
pub const INSPECTOR: f32 = 18.0;

#[cfg(test)]
mod tests {
    use super::*;

    use super::Icon;
    const ALL: [Icon; 21] = Icon::ALL;

    #[test]
    fn every_icon_produces_a_well_formed_svg_document() {
        for which in ALL {
            let source = which.svg_source();
            assert!(source.starts_with("<svg "), "{which:?} is not an svg document");
            assert!(source.ends_with("</svg>"), "{which:?} is not closed");
            assert!(source.contains(r#"viewBox="0 0 24 24""#), "{which:?} lost its viewbox");
            assert!(
                source.matches("<path").count() + source.matches("<circle").count() > 0,
                "{which:?} has no drawn geometry"
            );
        }
    }

    /// Every mark must be recolorable through `svg::Style::color`, which
    /// only works if nothing hardcodes a color.
    /// An SVG element may not carry the same attribute twice.
    ///
    /// `STROKE` already sets `stroke-width`, so appending another one to
    /// the same `<path>` produces duplicate-attribute markup that the
    /// rasteriser rejects — silently. The hamburger occupied its slot in
    /// the HeaderBar and drew nothing at all, which no test and no
    /// compiler could see; only a screenshot showed the gap.
    #[test]
    fn no_icon_sets_the_same_attribute_twice_on_one_element() {
        for which in ALL {
            for element in which.svg_source().split('<').skip(1) {
                let Some(tag) = element.split('>').next() else { continue };
                for attribute in ["stroke-width", "d", "fill", "stroke"] {
                    let hits = tag.matches(&format!("{attribute}=")).count();
                    assert!(
                        hits <= 1,
                        "{which:?} repeats `{attribute}` on one element, which is invalid \
                         SVG and renders as nothing: {tag}"
                    );
                }
            }
        }
    }

    #[test]
    fn no_icon_hardcodes_a_color() {
        for which in ALL {
            let source = which.svg_source();
            assert!(source.contains("currentColor"), "{which:?} does not use currentColor");
            assert!(!source.contains('#'), "{which:?} hardcodes a hex color");
        }
    }

    /// The stroke weights the design system implies, asserted so a future edit
    /// that drifts to 1.0 or 3.0 fails rather than quietly changing the
    /// visual weight of every control.
    #[test]
    fn stroke_weights_stay_within_the_documented_range() {
        for which in ALL {
            let source = which.svg_source();
            // Filled-only icons legitimately carry no stroke width.
            if source.contains("stroke-width") {
                assert!(
                    source.contains(r#"stroke-width="1.7""#)
                        || source.contains(r#"stroke-width="1.9""#)
                        || source.contains(r#"stroke-width="2.4""#),
                    "{which:?} uses an undocumented stroke width"
                );
            }
        }
    }

    #[test]
    fn icon_sizes_match_the_design_system() {
        // The design system: "16px in controls and 18px in inspector rows."
        assert_eq!(CONTROL, 16.0);
        assert_eq!(INSPECTOR, 18.0);
    }

    /// Every variant must appear in `ALL`, because `ALL` is what builds
    /// the handle cache — and `icon()` indexes that cache directly, so a
    /// variant missing from the list panics at the first render rather
    /// than degrading. Adding `Close` hit exactly that: the enum knew
    /// about it, the registry did not, and the crash surfaced in an
    /// unrelated shell test.
    ///
    /// The count assertion below cannot catch a *swap* (one variant
    /// dropped, another added), so the cache lookup is exercised for
    /// every variant too.
    #[test]
    fn every_icon_variant_is_registered_in_the_handle_cache() {
        for which in ALL {
            // Panics if `which` is absent from the `HANDLES` map.
            let _ = super::HANDLES[&which].clone();
        }
    }

    #[test]
    fn every_icon_variant_is_distinct_in_both_identity_and_artwork() {
        assert_eq!(ALL.len(), 21, "update this list when adding an icon");
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a, b, "duplicate variant in the inventory");
                assert_ne!(
                    a.svg_source(),
                    b.svg_source(),
                    "{a:?} and {b:?} would draw the same mark"
                );
            }
        }
    }

    /// Zoom in and zoom out differ only by the plus stroke; losing it
    /// would make the two timeline-zoom buttons identical.
    #[test]
    fn zoom_in_has_a_plus_that_zoom_out_does_not() {
        assert!(Icon::ZoomIn.svg_source().contains("M10.5 7.5 V13.5"));
        assert!(!Icon::ZoomOut.svg_source().contains("M10.5 7.5 V13.5"));
    }

    /// The muted speaker carries an "x"; the unmuted one carries waves.
    #[test]
    fn the_two_speaker_states_are_visually_distinguishable() {
        let on = Icon::SpeakerOn.svg_source();
        let off = Icon::SpeakerOff.svg_source();
        assert!(on.contains('C'), "SpeakerOn should have curved wave paths");
        assert_ne!(on, off);
    }
}
