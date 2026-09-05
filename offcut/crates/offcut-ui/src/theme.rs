//! Colour tokens for Offcut's interface.
//!
//! Roles split into two families by what they are painted on.
//!
//! **Chrome roles** sit on the app's own surfaces: the toolbar, the
//! inspector, the modal. Each is asserted against the surface it is
//! actually drawn on, and they invert between Dark and Light.
//!
//! **Stage roles** (`trim_*`, `playhead`, `stage_badge_*`) are drawn on
//! or beside the picture, which is black in both modes. They are
//! mode-invariant and asserted against black. Recolouring them for Light
//! mode would change marks floating over video that did not change.

use iced::Color;

/// `Color:from_rgb8` is `const`, so every literal below reads directly
/// as the six-digit hex a designer would write, with no manual `/255.0`
/// division to get wrong.
const fn hex(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb8(r, g, b)
}

/// A named color role.
///
/// A named colour role.
///
/// The roles split into two families, and the split is the design:
///
/// **Chrome roles** (`surface*`, `text_*`, `border*`, `accent*`) sit on
/// the application's own surfaces, which have a known background. Each is
/// asserted against the surface it is genuinely painted on, and they
/// invert between Dark and Light like ordinary interface colour.
///
/// **Stage roles** (`trim_out*`, `trim_track`, `trim_range_*`,
/// `playhead`, `mute`, `stage_badge_*`, `letterbox`, `stage_shadow`) are
/// drawn on the picture or in the dark well below it, neither of which
/// changes with the appearance setting. They are **mode-invariant** and
/// asserted against those dark grounds: recolouring a mark that floats
/// over footage, because the user switched the *panels* to light, changes
/// legibility on video that did not change at all. The same argument
/// covers the trim well: it is a viewing instrument, not a panel, and
/// Final Cut and Premiere both keep theirs dark in every appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// The window background, behind the toolbar, transport and trim bar.
    pub canvas: Color,
    /// A panel surface: the inspector, the toolbar.
    pub surface: Color,
    /// A card or control raised above its panel.
    pub surface_raised: Color,
    /// An inset well: a slider track, a text field, the trim bar track.
    pub surface_sunken: Color,
    pub surface_sunken_alt: Color,
    /// The video ground. Black in both modes: a grey surround competes
    /// with the picture.
    pub letterbox: Color,
    /// Hairline separators. Rows are divided by rules, not boxes.
    pub border: Color,
    /// The heavier border on a raised control, so a button edge is
    /// visible against the card it sits on.
    pub border_raised: Color,
    /// The one saturated colour in the chrome: selection, active tab,
    /// primary action.
    ///
    /// A fill role, asserted at the 3:1 graphical floor. Accent-coloured
    /// text uses `accent_tint_text`, which has a different floor.
    pub accent: Color,
    /// Foreground for content drawn on the accent fill.
    ///
    /// A role rather than a literal white at each call site: white on a
    /// previous mint accent measured 1.98:1 and the play glyph was
    /// unreadable. Asserted at 4.5:1 in both modes, which is why Dark's
    /// accent is `#0a6fd8` and not `#0a84ff` (white on that is 3.65:1).
    pub on_accent: Color,
    /// The moving part of a control: a toggler's knob, a slider's thumb.
    ///
    /// White in both appearances. Legibility comes from
    /// `control_track_off` rather than from tinting the knob.
    pub control_knob: Color,
    /// The unfilled part of any control track: an off toggler, a
    /// slider's remainder, the bipolar dial's rail.
    ///
    /// One role for all three. The sliders once used `surface_raised`,
    /// the card they sit on, at 1.00:1: the unfilled half was invisible.
    ///
    /// Held to 3:1 against both grounds it is painted on (raised card and
    /// sunken well) and dark enough to carry the white knob. Not
    /// `surface_sunken`: a white knob on Light's well is 1.29:1.
    pub control_track_off: Color,
    /// A completed export.
    ///
    /// Separate from `accent`, which previously carried this too and so
    /// meant four things at once: primary action, selection, progress,
    /// and result. Never the only signal: the pill says "Exported".
    pub success: Color,
    /// A failure, or a destructive action. Chrome's red.
    ///
    /// Same hue as `mute` but a separate field: `mute` is a stage role
    /// and mode-invariant, while this is measured against a light panel
    /// the stage never sees.
    pub danger: Color,
    /// The origin mark on a bipolar control.
    ///
    /// Ink, not `accent`: the straighten dial's handle is the accent, so
    /// an accent tick vanished under it at exactly 0.0°.
    pub origin_mark: Color,
    /// A tinted background for an accent-coloured callout.
    pub accent_tint_bg: Color,
    /// Accent-coloured text, at the 4.5:1 floor. A fill and a label have
    /// different requirements, so they are separate roles.
    pub accent_tint_text: Color,

    // ---- Stage roles: the picture and the well below it, mode-invariant ----
    /// The trim bar's track: the well the whole source lies in.
    ///
    /// Dark in both appearances. It carries the amber out-point and red
    /// playhead, which measure 1.11:1 and 1.66:1 on a light grey.
    pub trim_track: Color,
    /// Fill for the trim bar's selected range.
    ///
    /// Solid, not a wash: a 16% white wash on this track measured 1.62:1
    /// and the bar read as empty.
    pub trim_range_fill: Color,
    /// The in-point handle and the kept range's bounding rules.
    pub trim_range_edge: Color,
    /// The unselected remainder of the source: the footage the export
    /// will discard. Must read as *excluded* without vanishing.
    pub trim_range_excluded: Color,
    /// The playhead. Red, so it is distinct from the selection blue and
    /// the out-point amber.
    pub playhead: Color,
    /// Muted audio. Shares the playhead's red; both mean attention.
    pub mute: Color,
    /// The out-point handle, paired with `trim_range_edge` (the
    /// in-point). Hue tells which edge a drag is moving; position and
    /// shape carry it too.
    pub trim_out: Color,
    pub trim_out_hover: Color,
    /// Fixed pair for badges drawn over the picture.
    pub stage_badge_text: Color,
    pub stage_badge_text_dim: Color,
    /// The shadow under a badge or readout laid over the picture.
    ///
    /// Video has no guaranteed luminance: white text over a white sky is
    /// invisible and no palette value fixes it: so anything drawn on the
    /// stage carries a soft dark backing. Kept as a role rather than a
    /// literal so the treatment is consistent wherever it appears.
    pub stage_shadow: Color,

    // ---- Chrome text ----
    pub text_primary: Color,
    pub text_secondary: Color,
    pub text_secondary_alt: Color,
    pub text_muted: Color,
    /// **Disabled** text: a menu item that cannot act, a chip with no
    /// clip to apply to.
    ///
    /// The one text role deliberately below 4.5:1: 3.49:1 Dark, 3.33:1
    /// Light. WCAG 1.4.3 exempts disabled controls, and that exemption is
    /// the point rather than a loophole: "unavailable" is communicated by
    /// being *harder to read* than live text, so raising it to the floor
    /// would delete the signal.
    ///
    /// Held to 3:1 instead, which is a real floor and one it must not
    /// slip under: asserted in `disabled_text_is_dimmer_but_still_legible`.
    /// It is excluded from the 4.5:1 list by name, not by omission.
    pub text_muted_alt: Color,
    /// Hover states: one tonal step from their base surface.
    pub surface_hover: Color,
    pub accent_hover: Color,
    /// The audio level meter's bars.
    pub waveform: Color,
    /// The 1px border around a muted audio span.
    pub mute_border: Color,
}

/// Builds a `Palette` from its **chrome** fields alone, filling every
/// **stage** field from one shared list.
///
/// This macro is the enforcement mechanism for the mode-invariance rule.
/// Both palettes below are written through it, so the stage roles are
/// physically written once and a per-mode stage value is not something
/// you can express here without editing the shared list: which is the
/// point. A test then asserts the two palettes agree on every stage
/// field, so even editing the list cannot ship a mode-dependent stage.
///
/// (A macro expanding to a bare field list would be simpler, but Rust
/// parses struct literals before macro expansion, so `Palette {
/// stage_roles!(), .. }` is a syntax error. The macro owns the whole
/// literal instead.)
macro_rules! studio_palette {
    ($($chrome_field:ident: $chrome_value:expr),* $(,)?) => {
        Palette {
            $($chrome_field: $chrome_value,)*

        // System red. 4.50:1 on the dark panel.
        playhead: hex(0xFF, 0x45, 0x3A),
        mute: hex(0xFF, 0x45, 0x3A),
        // Amber, 14.7:1 on the black track. Means the out-point, only.
        trim_out: hex(0xFF, 0xD4, 0x26),
        trim_out_hover: hex(0xFF, 0xE0, 0x66),

        // The trim well. Dark in both modes: its marks are the amber
        // out-point and red playhead, both below 1.2:1 on a light grey.
        trim_track: hex(0x15, 0x15, 0x15),
        // The kept range: 3.72:1 on the track. A 16% white wash here
        // read 1.62:1 and the bar looked empty.
        trim_range_fill: hex(0x0A, 0x6F, 0xD8),
        // The in-point handle and the rules bounding the kept range.
        // White: 21:1 on the track and 4.91:1 on the blue fill it sits on.
        trim_range_edge: hex(0xFF, 0xFF, 0xFF),
        // The discarded head and tail. Reads as excluded, not absent.
        trim_range_excluded: Color::from_rgba(1.0, 1.0, 1.0, 0.06),

        stage_badge_text: hex(0xFF, 0xFF, 0xFF),
        stage_badge_text_dim: hex(0xC8, 0xC8, 0xC8),
        // Video has no guaranteed luminance, so anything laid over the
        // picture carries a soft dark backing.
        stage_shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.55),
        }
    };
}

impl Palette {
    /// **Dark**: the default, and the one a media application is used
    /// in: you judge an image against a dark surround, which is why every
    /// editor and viewer in this category ships dark first.
    ///
    /// The ramp is deliberately **neutral**. A blue-tinted grey is the
    /// tell of a theme that was picked rather than measured, and next to
    /// a true-black stage any cast in the surrounding chrome is visible
    /// as a colour error in the picture.
    pub const DARK: Palette = studio_palette! {
        canvas: hex(0x1A, 0x1A, 0x1A),
        surface: hex(0x25, 0x25, 0x25),
        surface_raised: hex(0x2F, 0x2F, 0x2F),
        surface_sunken: hex(0x15, 0x15, 0x15),
        surface_sunken_alt: hex(0x0D, 0x0D, 0x0D),
        // True black in both modes: the picture never gets a bright
        // frame, whatever the chrome is doing.
        letterbox: hex(0x00, 0x00, 0x00),
        border: hex(0x38, 0x38, 0x38),
        border_raised: hex(0x4A, 0x4A, 0x4A),
        // Not the brighter `#0a84ff`: this fill carries white labels,
        // and white on that is 3.65:1. This clears 4.91:1.
        accent: hex(0x0A, 0x6F, 0xD8),
        on_accent: hex(0xFF, 0xFF, 0xFF),
        control_knob: hex(0xFF, 0xFF, 0xFF),
        // Two floors at once: 4.27:1 under the white knob, 3.14:1
        // against the card. The darker `#5A5A5F` met only the first, so
        // an off switch had no visible body.
        control_track_off: hex(0x7A, 0x7A, 0x80),
        // 7.58:1 on the panel, 76 degrees of hue from the accent.
        success: hex(0x30, 0xD1, 0x58),
        danger: hex(0xFF, 0x45, 0x3A),
        // 9.24:1 on the well, 4.49:1 on the rail, 2.48:1 against the
        // handle that parks on it at 0.0 degrees.
        origin_mark: hex(0xB8, 0xB8, 0xBD),
        accent_tint_bg: hex(0x14, 0x2A, 0x42),
        // Accent text needs 4.5:1, which the fill above misses. 7.16:1.
        accent_tint_text: hex(0x6F, 0xB6, 0xFF),

        text_primary: hex(0xFF, 0xFF, 0xFF),
        text_secondary: hex(0xA8, 0xA8, 0xAD),
        text_secondary_alt: hex(0xC0, 0xC0, 0xC5),
        text_muted: hex(0x90, 0x90, 0x95),
        text_muted_alt: hex(0x78, 0x78, 0x7D),
        surface_hover: hex(0x3A, 0x3A, 0x3A),
        accent_hover: hex(0x2B, 0x85, 0xEA),
        waveform: hex(0x5E, 0x5E, 0x63),
        mute_border: hex(0x5E, 0x2A, 0x25),
    };

    /// **Light**: the same arrangement on light surfaces, for a user
    /// working in daylight.
    ///
    /// The stage stays black: `studio_palette!` fills every stage role
    /// from one shared list, so the picture and its marks are identical
    /// in both modes. Only the chrome inverts.
    pub const LIGHT: Palette = studio_palette! {
        canvas: hex(0xEC, 0xEC, 0xEE),
        surface: hex(0xF5, 0xF5, 0xF7),
        // A card differs from its panel by tone, not border alone, so a
        // border bug degrades rather than deletes. Asserted by a test.
        surface_raised: hex(0xFF, 0xFF, 0xFF),
        surface_sunken: hex(0xE2, 0xE2, 0xE5),
        surface_sunken_alt: hex(0xD8, 0xD8, 0xDB),
        letterbox: hex(0x00, 0x00, 0x00),
        border: hex(0xD6, 0xD6, 0xD9),
        border_raised: hex(0xC6, 0xC6, 0xC8),
        accent: hex(0x00, 0x71, 0xE3),
        on_accent: hex(0xFF, 0xFF, 0xFF),
        control_knob: hex(0xFF, 0xFF, 0xFF),
        // Clears three floors: 4.14:1 on the card, 3.20:1 on the well,
        // 4.14:1 under the knob.
        control_track_off: hex(0x7C, 0x7C, 0x84),
        // A step darker than Dark's: the system green is 1.7:1 on a
        // white card. 4.04:1 here.
        success: hex(0x24, 0x8A, 0x3D),
        danger: hex(0xD7, 0x00, 0x15),
        // Ink, not grey: `text_muted` is 1.08:1 against Light's handle.
        origin_mark: hex(0x2C, 0x2C, 0x2E),
        accent_tint_bg: hex(0xE3, 0xEF, 0xFC),
        accent_tint_text: hex(0x00, 0x58, 0xB0),

        text_primary: hex(0x1D, 0x1D, 0x1F),
        text_secondary: hex(0x5E, 0x5E, 0x63),
        text_secondary_alt: hex(0x48, 0x48, 0x4D),
        text_muted: hex(0x6E, 0x6E, 0x73),
        text_muted_alt: hex(0x86, 0x86, 0x8B),
        surface_hover: hex(0xE8, 0xE8, 0xEB),
        accent_hover: hex(0x00, 0x5F, 0xC4),
        waveform: hex(0xB4, 0xB4, 0xB8),
        mute_border: hex(0xF0, 0xC6, 0xC9),
    };
}

/// App-wide UI mode. `Default` is `Dark`, matching the design system: "Dark is
/// the default, chosen from the use scene."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

impl Palette {
    /// The built-in palette for `mode`, before any user overrides.
    pub fn builtin(mode: Mode) -> Palette {
        match mode {
            Mode::Dark => Palette::DARK,
            Mode::Light => Palette::LIGHT,
        }
    }

    /// Set one role by its config name. Returns `false` for an unknown
    /// name, so the loader can report a typo instead of ignoring it.
    ///
    /// The match is exhaustive over the struct's fields on purpose: add a
    /// role and this stops compiling until it is named here, which is
    /// what keeps the config vocabulary from silently falling behind the
    /// palette.
    pub fn set_role(&mut self, role: &str, color: Color) -> bool {
        match role {
            "canvas" => self.canvas = color,
            "surface" => self.surface = color,
            "surface_raised" => self.surface_raised = color,
            "surface_sunken" => self.surface_sunken = color,
            "surface_sunken_alt" => self.surface_sunken_alt = color,
            "letterbox" => self.letterbox = color,
            "border" => self.border = color,
            "border_raised" => self.border_raised = color,
            "accent" => self.accent = color,
            "on_accent" => self.on_accent = color,
            "control_knob" => self.control_knob = color,
            "control_track_off" => self.control_track_off = color,
            "success" => self.success = color,
            "danger" => self.danger = color,
            "origin_mark" => self.origin_mark = color,
            "accent_tint_bg" => self.accent_tint_bg = color,
            "accent_tint_text" => self.accent_tint_text = color,
            "trim_track" => self.trim_track = color,
            "trim_range_fill" => self.trim_range_fill = color,
            "trim_range_edge" => self.trim_range_edge = color,
            "trim_range_excluded" => self.trim_range_excluded = color,
            "playhead" => self.playhead = color,
            "mute" => self.mute = color,
            "trim_out" => self.trim_out = color,
            "trim_out_hover" => self.trim_out_hover = color,
            "stage_badge_text" => self.stage_badge_text = color,
            "stage_badge_text_dim" => self.stage_badge_text_dim = color,
            "stage_shadow" => self.stage_shadow = color,
            "text_primary" => self.text_primary = color,
            "text_secondary" => self.text_secondary = color,
            "text_secondary_alt" => self.text_secondary_alt = color,
            "text_muted" => self.text_muted = color,
            "text_muted_alt" => self.text_muted_alt = color,
            "surface_hover" => self.surface_hover = color,
            "accent_hover" => self.accent_hover = color,
            "waveform" => self.waveform = color,
            "mute_border" => self.mute_border = color,
            _ => return false,
        }
        true
    }

    /// Every role name a config may set, in declaration order.
    ///
    /// Public because the config loader suggests near-misses from it and
    /// a test asserts `set_role` accepts every entry: the two would
    /// otherwise drift, and a role present in one and missing from the
    /// other is a customisation that silently does nothing.
    pub const ROLE_NAMES: [&'static str; 37] = [
        "canvas",
        "surface",
        "surface_raised",
        "surface_sunken",
        "surface_sunken_alt",
        "letterbox",
        "border",
        "border_raised",
        "accent",
        "on_accent",
        "control_knob",
        "control_track_off",
        "success",
        "danger",
        "origin_mark",
        "accent_tint_bg",
        "accent_tint_text",
        "trim_track",
        "trim_range_fill",
        "trim_range_edge",
        "trim_range_excluded",
        "playhead",
        "mute",
        "trim_out",
        "trim_out_hover",
        "stage_badge_text",
        "stage_badge_text_dim",
        "stage_shadow",
        "text_primary",
        "text_secondary",
        "text_secondary_alt",
        "text_muted",
        "text_muted_alt",
        "surface_hover",
        "accent_hover",
        "waveform",
        "mute_border",
    ];
}

impl Mode {
    pub fn palette(self) -> Palette {
        Palette::builtin(self)
    }

    pub fn toggled(self) -> Mode {
        match self {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A card must differ from its surface by tone, not border alone.
    ///
    /// Both were `#ffffff` in light mode once, so a group defined only by
    /// its 1px border vanished when that border failed to render.
    #[test]
    fn cards_are_separable_from_their_surface_without_a_border() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            assert_ne!(
                p.surface_raised, p.surface,
                "{name}: a card and its background are the same colour, so the group \
                 exists only as long as its border renders"
            );
        }
    }

    #[test]
    fn dark_and_light_accent_are_distinct_but_both_valid_colors() {
        assert_ne!(Palette::DARK.accent, Palette::LIGHT.accent);
    }

    /// Every stage role is byte-identical across the two modes.
    ///
    /// Stage roles are drawn over the picture, which is black in both
    /// modes. Recolouring them because the panels changed would alter
    /// legibility on footage that did not.
    ///
    /// The list is manual: adding a stage role means adding it here, so a
    /// role deliberately moved to the chrome family is not covered by
    /// accident.
    #[test]
    fn every_stage_role_is_mode_invariant() {
        let (d, l) = (Palette::DARK, Palette::LIGHT);
        for (role, a, b) in [
            ("letterbox", d.letterbox, l.letterbox),
            ("playhead", d.playhead, l.playhead),
            ("mute", d.mute, l.mute),
            ("trim_out", d.trim_out, l.trim_out),
            ("trim_out_hover", d.trim_out_hover, l.trim_out_hover),
            ("trim_track", d.trim_track, l.trim_track),
            ("trim_range_edge", d.trim_range_edge, l.trim_range_edge),
            ("trim_range_fill", d.trim_range_fill, l.trim_range_fill),
            ("trim_range_excluded", d.trim_range_excluded, l.trim_range_excluded),
            ("stage_badge_text", d.stage_badge_text, l.stage_badge_text),
            ("stage_badge_text_dim", d.stage_badge_text_dim, l.stage_badge_text_dim),
            ("stage_shadow", d.stage_shadow, l.stage_shadow),
        ] {
            assert_eq!(
                a, b,
                "{role} differs between Dark and Light, but it is drawn on the picture — \
                 the footage did not change when the panels did"
            );
        }
    }

    /// The chrome family, by contrast, **must** invert. A palette whose
    /// panels are identical in both modes has a Light mode in name only.
    #[test]
    fn the_chrome_family_actually_inverts() {
        let (d, l) = (Palette::DARK, Palette::LIGHT);
        assert_ne!(d.surface, l.surface, "the panel is the same colour in both modes");
        assert_ne!(d.text_primary, l.text_primary);
        assert!(
            luminance(l.surface) > luminance(d.surface),
            "the light panel is not lighter than the dark one"
        );
    }

    /// WCAG 2.1 relative luminance.
    fn luminance(c: Color) -> f32 {
        let ch = |v: f32| if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
        0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    /// Hue in degrees, for asserting that two roles are different
    /// *colours* and not merely different values of one.
    fn hue_deg(c: Color) -> f32 {
        let (r, g, b) = (c.r, c.g, c.b);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        if d.abs() < 1e-6 {
            return 0.0;
        }
        let h = if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        if h < 0.0 { h + 360.0 } else { h }
    }

    /// The shorter way around the hue circle, so 350° and 10° read as
    /// 20° apart rather than 340°.
    fn hue_separation(a: Color, b: Color) -> f32 {
        let d = (hue_deg(a) - hue_deg(b)).abs();
        d.min(360.0 - d)
    }

    /// White on a previous mint accent measured 1.98:1, and nothing
    /// complained because the play icon was a hardcoded `Color:WHITE`.
    /// `on_accent` carries that per mode; this asserts the floor.
    ///
    /// Also why Dark's accent is `#0a6fd8`: `#0a84ff` is 3.65:1 here.
    #[test]
    fn on_accent_is_readable_against_the_accent_in_both_modes() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let ratio = contrast(p.on_accent, p.accent);
            assert!(
                ratio >= 4.5,
                "{name}: on_accent against accent is {ratio:.2}:1, below the 4.5:1 floor"
            );
        }
    }

    /// Chrome text is checked against the panel it is genuinely painted
    /// on.
    #[test]
    fn chrome_text_clears_the_floor_on_its_own_surface() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            for (role, color) in [
                ("text_primary", p.text_primary),
                ("text_secondary", p.text_secondary),
                ("text_secondary_alt", p.text_secondary_alt),
                ("text_muted", p.text_muted),
                ("accent_tint_text", p.accent_tint_text),
            ] {
                let ratio = contrast(color, p.surface);
                assert!(
                    ratio >= 4.5,
                    "{name}/{role}: {ratio:.2}:1 on the panel — below the 4.5:1 text floor"
                );
            }
        }
    }

    /// The unfilled track must be visible on every surface it is drawn
    /// on: raised cards in the inspector, and the dial's sunken well.
    ///
    /// A value tuned against one disappears on the other. Both shipped:
    /// the sliders' remainder was `surface_raised` (1.00:1) and the
    /// dial's rail was `border_raised` (1.32:1 on Light's well).
    #[test]
    fn an_unfilled_track_is_visible_on_both_grounds_it_is_painted_on() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            for (ground, bg) in
                [("a raised card", p.surface_raised), ("the sunken well", p.surface_sunken)]
            {
                let ratio = contrast(p.control_track_off, bg);
                assert!(
                    ratio >= 3.0,
                    "{name}: the unfilled track is {ratio:.2}:1 on {ground} — the control \
                     has no visible extent there, only a floating handle"
                );
            }
        }
    }

    /// Disabled text sits below the body floor on purpose, so it gets
    /// its own floor rather than none.
    ///
    /// WCAG 1.4.3 exempts disabled controls, and dimness is the disabled
    /// signal. Asserted at 3:1 and asserted to stay dimmer than the live
    /// text beside it.
    #[test]
    fn disabled_text_is_dimmer_but_still_legible() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let ratio = contrast(p.text_muted_alt, p.surface);
            assert!(
                ratio >= 3.0,
                "{name}: disabled text is {ratio:.2}:1 on the panel — dim is the signal, \
                 illegible is a bug"
            );
            assert!(
                ratio < contrast(p.text_secondary, p.surface),
                "{name}: disabled text reads as strongly as live secondary text, so \
                 'unavailable' has no visual cue at all"
            );
        }
    }

    /// Every saturated role is checked against the surface it actually
    /// sits on, which differs by role.
    ///
    /// `accent` and `mute` are chrome, checked against `surface`.
    /// `trim_*` are drawn in the dark well; checking those against a
    /// light panel would demand an amber invisible where it is painted.
    #[test]
    fn saturated_roles_clear_the_contrast_floor_on_their_surface() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            for (role, color, background, bg_name) in [
                ("accent", p.accent, p.surface, "the panel"),
                ("mute", p.mute, p.surface, "the panel"),
                // The outcome pills are painted as a 1px border on a
                // raised card, so the card is the ground that matters: 
                // not the panel behind it.
                ("success", p.success, p.surface_raised, "the pill's card"),
                ("danger", p.danger, p.surface_raised, "the pill's card"),
                ("origin_mark", p.origin_mark, p.surface_sunken, "the dial's well"),
                ("trim_range_fill", p.trim_range_fill, p.trim_track, "the trim well"),
                ("trim_range_edge", p.trim_range_edge, p.trim_track, "the trim well"),
                ("trim_out", p.trim_out, p.trim_track, "the trim well"),
                ("playhead", p.playhead, p.trim_track, "the trim well"),
                ("stage_badge_text", p.stage_badge_text, p.letterbox, "the stage"),
            ] {
                let ratio = contrast(color, background);
                assert!(
                    ratio >= 3.0,
                    "{name}/{role}: {ratio:.2}:1 against {bg_name} — below the 3:1 floor \
                     for a large/graphical element"
                );
            }
        }
    }

    /// Each saturated colour owns exactly one meaning, so state can be
    /// read at a glance. Amber (out-point) and red (playhead/mute) are the
    /// pair most at risk of collapsing into "some warm colour"; blue and
    /// amber must stay obviously different as the in/out pair.
    #[test]
    fn each_saturated_role_stays_hue_distinct_from_the_others() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let separation = hue_separation;
            assert!(
                separation(p.trim_out, p.mute) > 20.0,
                "{name}: the out-point amber and the attention red are too close in hue \
                 to tell apart at a glance"
            );
            assert!(
                separation(p.accent, p.trim_out) > 60.0,
                "{name}: the selection blue and the out-point amber must be obviously \
                 different hues"
            );
            assert!(
                separation(p.accent, p.playhead) > 60.0,
                "{name}: the selection blue and the playhead red must be obviously \
                 different hues"
            );
        }
    }

    /// A finished export and a failed one must not be the same colour.
    ///
    /// They are close in luminance: green and red are 1.22:1 apart in
    /// Light, the pair a deuteranope cannot separate. So `status_pill`
    /// leads with the word, and colour is the fast path, not the only one.
    #[test]
    fn the_outcome_colours_are_not_the_same_event() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            assert_ne!(p.success, p.danger, "{name}: succeeded and failed render alike");
            assert!(
                hue_separation(p.success, p.danger) > 60.0,
                "{name}: the success and failure hues are too close to tell apart"
            );
        }
    }

    /// The accent must not also mean "finished".
    ///
    /// A completed export once wore `accent`, the same blue as the
    /// primary action, the selection, and the progress bar.
    #[test]
    fn a_finished_export_does_not_wear_the_selection_colour() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            assert_ne!(
                p.success, p.accent,
                "{name}: 'exported' and 'selected' are the same colour again"
            );
            assert!(
                hue_separation(p.success, p.accent) > 45.0,
                "{name}: the success green has drifted back towards the selection blue"
            );
        }
    }

    /// The origin mark must be readable against the handle that parks
    /// on it.
    ///
    /// The tick was `accent`, and so is the handle over it: at 0.0° a 2px
    /// blue tick under a 3px blue handle merged into one bar.
    #[test]
    fn the_origin_mark_survives_the_handle_parking_on_it() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            assert_ne!(
                p.origin_mark, p.accent,
                "{name}: the dial's zero tick and its handle are the same colour, so the \
                 origin disappears at 0.0°"
            );
            let ratio = contrast(p.origin_mark, p.accent);
            assert!(
                ratio >= 2.0,
                "{name}: the origin tick is {ratio:.2}:1 against the handle that covers it"
            );
        }
    }

    /// Chrome red and stage red are separate fields.
    ///
    /// `mute` is mode-invariant; `danger` is measured against a light
    /// panel the stage never sees. This asserts Light's danger is tuned
    /// for its panel rather than copied.
    #[test]
    fn chrome_red_is_tuned_separately_from_the_red_over_footage() {
        assert_ne!(
            Palette::LIGHT.danger,
            Palette::LIGHT.mute,
            "the light panel's red is a copy of the stage's, so it was never measured here"
        );
        let ratio = contrast(Palette::LIGHT.danger, Palette::LIGHT.surface_raised);
        assert!(ratio >= 4.5, "light danger is {ratio:.2}:1 on its card");
    }

    /// The in-point and the out-point must be distinguishable *without*
    /// relying on hue, because a red-green colourblind user reads amber
    /// and white as the same warm-ish mark at 13px.
    #[test]
    fn the_in_and_out_handles_differ_by_luminance_not_only_hue() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            assert_ne!(
                p.trim_range_edge, p.trim_out,
                "{name}: in and out are not interchangeable and must not render alike"
            );
            let separable = contrast(p.trim_range_edge, p.trim_out);
            assert!(
                separable >= 1.2,
                "{name}: the in and out handles are {separable:.2}:1 apart in luminance — \
                 a colourblind user reads them as the same mark"
            );
        }
    }

    /// Control handles must be visible against the surface they sit on,
    /// in both modes.
    ///
    /// The slider thumb was once a hardcoded `Color:WHITE`, which
    /// vanished on light mode's white cards: the control rendered with
    /// no thumb at all. A literal colour at a call site cannot follow the
    /// palette, which is exactly why `on_accent` was made a role.
    #[test]
    fn control_handles_are_visible_against_their_own_surface() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            let ratio = contrast(p.border_raised, p.surface_raised);
            assert!(
                ratio >= 1.25,
                "{name}: a handle drawn in surface_raised with a border_raised outline \
                 is {ratio:.2}:1 against its own card — not a visible control"
            );
        }
    }

    /// The neutral ramp must stay **neutral**.
    ///
    /// A blue- or warm-tinted grey is the tell of a theme that was picked
    /// rather than measured, and next to a true-black stage any cast in
    /// the surrounding chrome reads as a colour error in the picture
    /// itself: which is the one thing a media application cannot afford.
    #[test]
    fn the_neutral_ramp_carries_no_colour_cast() {
        for (name, p) in [("dark", Palette::DARK), ("light", Palette::LIGHT)] {
            for (role, c) in [
                ("canvas", p.canvas),
                ("surface", p.surface),
                ("surface_raised", p.surface_raised),
                ("surface_sunken", p.surface_sunken),
                ("border", p.border),
                ("border_raised", p.border_raised),
            ] {
                let max = c.r.max(c.g).max(c.b);
                let min = c.r.min(c.g).min(c.b);
                assert!(
                    max - min <= 0.02,
                    "{name}/{role}: channel spread is {:.3} — this grey has a colour cast, \
                     which reads as a tint in the picture beside it",
                    max - min
                );
            }
        }
    }

    /// The stage is black in both modes, and that is a decision rather
    /// than an oversight in the Light palette.
    #[test]
    fn the_stage_is_black_in_both_modes() {
        assert_eq!(Palette::DARK.letterbox, Palette::LIGHT.letterbox);
        assert_eq!(Palette::DARK.letterbox, Color::from_rgb8(0, 0, 0));
    }

    #[test]
    fn mode_default_is_dark() {
        assert_eq!(Mode::default(), Mode::Dark);
    }

    #[test]
    fn mode_toggles_both_ways() {
        assert_eq!(Mode::Dark.toggled(), Mode::Light);
        assert_eq!(Mode::Light.toggled(), Mode::Dark);
    }
}
