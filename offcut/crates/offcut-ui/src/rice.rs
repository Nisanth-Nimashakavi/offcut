//! User theming: an optional colour config read once at startup.
//!
//! # What this is and is not
//!
//! It overrides **colours**, by role name, and nothing else. Geometry,
//! typography, and layout stay fixed, because those are the numbers
//! The design system derives from measurements — the trim handle's radius is
//! tied to its grab zone, the content rail to the readout's alignment —
//! and exposing them invites a config that renders a control unusable in
//! a way the user cannot diagnose. A palette is the part of a visual
//! world that is genuinely a matter of taste.
//!
//! # Why it warns instead of enforcing
//!
//! `theme.rs` asserts contrast floors that caught real defects: a white
//! knob on a white card, a trim selection at 1.62:1, a toggler that only
//! rendered in one of its two states. Those tests protect the *shipped*
//! palette and they should keep doing so.
//!
//! A user's palette is their business. But the failures above are ones
//! that look like nothing at all — a control that does not appear to be
//! there reads as a bug in the app, not as a consequence of a colour
//! choice — so a config that lands in that state is told, once, with the
//! role named and the ratio measured. Applied either way: the check is a
//! reading, not a veto.

use crate::theme::{Mode, Palette};
use iced::Color;
use std::path::{Path, PathBuf};

/// Where the config lives, following the XDG spec with a sensible
/// fallback. Returns `None` when neither variable nor home is set, which
/// is a machine with no user config location rather than an error.
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir).join("offcut"));
    }
    let home = std::env::var_os("HOME").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(home).join(".config").join("offcut"))
}

pub fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("colors.toml"))
}

/// Where saved themes live: `<config>/themes/<name>.toml`.
///
/// A directory rather than one file, because a person who rices has more
/// than one theme and switching should not mean editing. The active
/// choice is a *name* recorded in `selected`, so the theme files
/// themselves stay untouched by the app — they are the user's documents,
/// not its storage.
pub fn themes_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("themes"))
}

/// The file recording which theme was last chosen.
///
/// Deliberately a bare name and not a path: a moved config directory
/// should keep working, and an absolute path recorded here would break
/// the moment someone moved their dotfiles.
fn selection_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("selected"))
}

/// Every theme in the themes directory, by name, sorted.
///
/// Missing directory yields an empty list rather than an error: nobody
/// has saved a theme yet is the normal first case.
pub fn available_themes() -> Vec<String> {
    let Some(dir) = themes_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|x| x.to_str()) != Some("toml") {
                return None;
            }
            path.file_stem().and_then(|n| n.to_str()).map(str::to_string)
        })
        .collect();
    names.sort();
    names
}

/// The theme chosen last time, if it still exists.
///
/// A recorded name whose file has since been deleted returns `None`
/// rather than an error: the theme is simply gone, and starting on the
/// built-in palette is the right recovery.
pub fn selected_theme() -> Option<String> {
    let name = std::fs::read_to_string(selection_path()?).ok()?.trim().to_string();
    if name.is_empty() {
        return None;
    }
    available_themes().contains(&name).then_some(name)
}

/// Record the chosen theme so the next launch starts in it.
///
/// `None` clears the selection, which is how "use the built-in theme"
/// is expressed — writing the built-in's name would make it a theme
/// that could be shadowed by a user file of the same name.
pub fn remember_theme(name: Option<&str>) -> std::io::Result<()> {
    let Some(path) = selection_path() else {
        return Ok(());
    };
    match name {
        Some(n) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, n)
        }
        None => match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    }
}

/// Load a theme by name from the themes directory.
pub fn load_theme(name: &str) -> Riced {
    let Some(dir) = themes_dir() else {
        return Riced::builtin();
    };
    load_from(&dir.join(format!("{name}.toml")))
}

/// The outcome of loading a config: the palettes to use, plus anything
/// worth telling the user.
#[derive(Debug, Clone)]
pub struct Riced {
    pub dark: Palette,
    pub light: Palette,
    /// Problems found while loading. Empty on a clean load *and* on no
    /// config at all — absence of a file is not a problem.
    pub warnings: Vec<String>,
    /// Whether a config file was actually read.
    pub loaded: bool,
    /// The saved theme this came from, if it came from one. `None` means
    /// the built-in palette or the plain `colors.toml`.
    pub name: Option<String>,
}

impl Riced {
    /// The built-in theme, untouched.
    pub fn builtin() -> Self {
        Self {
            dark: Palette::DARK,
            light: Palette::LIGHT,
            warnings: Vec::new(),
            loaded: false,
            name: None,
        }
    }

    pub fn palette(&self, mode: Mode) -> Palette {
        match mode {
            Mode::Dark => self.dark,
            Mode::Light => self.light,
        }
    }
}

/// Load the config at the standard path, or return the built-in theme.
/// Load whatever this machine should start in.
///
/// The saved selection wins, then a plain `colors.toml`, then the
/// built-in palette. That order lets someone keep a hand-written
/// `colors.toml` and still switch to a named theme without deleting it —
/// picking "Built-in" clears the selection and the `colors.toml` comes
/// back, which is what a person who wrote that file expects.
/// Where a package drops its example themes.
///
/// A package installs as root into a staging directory with no user and
/// no `HOME`, so it cannot write anyone's `~/.config`. What it can do is
/// put files under a shared prefix; the app copies them the first time
/// it runs, as the user, with that user's permissions.
///
/// The env var is for Flatpak and AppImage, whose prefixes are not
/// `/usr`.
fn shipped_themes_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("OFFCUT_SHIPPED_THEMES").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    for prefix in ["/app/share/offcut/themes", "/usr/share/offcut/themes",
                   "/usr/local/share/offcut/themes"] {
        let p = PathBuf::from(prefix);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

/// Copy the shipped example themes into the user's themes directory,
/// once.
///
/// # What this deliberately does not do
///
/// It does not write a `colors.toml`, and it does not select a theme.
/// The app's default appearance is its built-in palette, and a fresh
/// install that silently starts in someone else's colour scheme is a
/// worse first run than one that starts plain. These are examples placed
/// where the picker can see them, nothing more.
///
/// It never overwrites. A user who edited `wall.toml` keeps their
/// version; a user who deleted it keeps it deleted, because the marker
/// below records that the seeding already happened.
fn seed_example_themes() {
    let (Some(dest), Some(src)) = (themes_dir(), shipped_themes_dir()) else {
        return;
    };
    // One marker rather than per-file existence checks: deleting an
    // example is a decision, and restoring it on the next launch would
    // override that decision every time.
    let marker = match config_dir() {
        Some(d) => d.join(".examples-installed"),
        None => return,
    };
    if marker.exists() {
        return;
    }
    if std::fs::create_dir_all(&dest).is_err() {
        return;
    }
    if let Ok(entries) = std::fs::read_dir(&src) {
        for e in entries.filter_map(Result::ok) {
            let from = e.path();
            if from.extension().and_then(|x| x.to_str()) != Some("toml") {
                continue;
            }
            if let Some(name) = from.file_name() {
                let to = dest.join(name);
                if !to.exists() {
                    let _ = std::fs::copy(&from, &to);
                }
            }
        }
    }
    let _ = std::fs::write(&marker, "");
}

pub fn load() -> Riced {
    // First run: put the shipped examples where the picker can find
    // them. Silent and best-effort — a read-only or absent config
    // location is not a reason to fail to start.
    seed_example_themes();

    if let Some(name) = selected_theme() {
        let mut r = load_theme(&name);
        if r.loaded {
            r.name = Some(name);
            return r;
        }
    }
    match config_path() {
        Some(path) => load_from(&path),
        None => Riced::builtin(),
    }
}

/// Load a specific file. A missing file is not an error — it is the
/// normal case for a user who has not customised anything.
pub fn load_from(path: &Path) -> Riced {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Riced::builtin(),
        Err(e) => {
            let mut r = Riced::builtin();
            r.warnings.push(format!("Could not read {}: {e}", path.display()));
            return r;
        }
    };
    parse(&text)
}

/// Parse config text into palettes.
///
/// The format is deliberately the smallest thing that works: two
/// optional sections, `[dark]` and `[light]`, each mapping a role name to
/// a hex colour. No nesting, no includes, no expressions — a theme file
/// should be readable and diffable at a glance.
///
/// ```text
/// [dark]
/// accent = "#89b4fa"
/// canvas = "#1e1e2e"
///
/// [light]
/// accent = "#1e66f5"
/// ```
pub fn parse(text: &str) -> Riced {
    let mut out = Riced::builtin();
    out.loaded = true;
    out.name = None;
    let mut section: Option<Mode> = None;
    // Seeds accumulate across a `[wallpaper]` section and are resolved
    // into a full palette after the whole file is read, so the order of
    // lines inside that section does not matter.
    let mut wall: Vec<(usize, String, Color)> = Vec::new();
    let mut in_wallpaper = false;
    let mut explicit: Vec<(Mode, String, Color)> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        // `#` starts a comment only where a value cannot begin, because
        // every colour in this file also starts with `#`. Splitting on
        // the first `#` would turn `accent = "#89b4fa"` into `accent = "`.
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }

        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_wallpaper = false;
            section = match name.trim() {
                "dark" => Some(Mode::Dark),
                "light" => Some(Mode::Light),
                "wallpaper" => {
                    in_wallpaper = true;
                    None
                }
                other => {
                    out.warnings.push(format!(
                        "line {line_no}: unknown section [{other}] — expected [wallpaper], \
                         [dark], or [light]"
                    ));
                    None
                }
            };
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            out.warnings.push(format!("line {line_no}: expected `role = \"#rrggbb\"`"));
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"').trim();

        if in_wallpaper {
            match parse_color(value) {
                Some(c) => wall.push((line_no, key.to_string(), c)),
                None => out.warnings.push(format!(
                    "line {line_no}: `{value}` is not a colour — use #rgb, #rrggbb, or #rrggbbaa"
                )),
            }
            continue;
        }

        let Some(mode) = section else {
            out.warnings
                .push(format!("line {line_no}: `{key}` is before any [dark] or [light] section"));
            continue;
        };

        let color = match parse_color(value) {
            Some(c) => c,
            None => {
                out.warnings.push(format!(
                    "line {line_no}: `{value}` is not a colour — use #rgb, #rrggbb, or #rrggbbaa"
                ));
                continue;
            }
        };

        let target = match mode {
            Mode::Dark => &mut out.dark,
            Mode::Light => &mut out.light,
        };
        if target.set_role(key, color) {
            // Remembered so a `[wallpaper]` derivation cannot overwrite
            // a value the user stated outright.
            explicit.push((mode, key.to_string(), color));
        } else {
            // A typo silently ignored is a customisation that appears not
            // to work, so the nearest real role is offered.
            let hint = nearest_role(key)
                .map(|r| format!(" — did you mean `{r}`?"))
                .unwrap_or_default();
            out.warnings.push(format!("line {line_no}: unknown colour role `{key}`{hint}"));
        }
    }

    // # Wallpaper seeds are resolved first, then overridden
    //
    // The derived palette is a *base*, not a result: a `[dark]` section
    // in the same file still wins, role by role. That ordering is the
    // useful one — derive the 37 relationships from a wallpaper, then
    // correct the two you disagree with, rather than choosing between
    // "generated" and "hand-written".
    if !wall.is_empty()
        && let Some(seeds) = seeds_from(&wall, &mut out.warnings)
    {
        {
            let derived = derive(&seeds);
            // The derivation is the base; explicit roles win. Both modes
            // get it, because a wallpaper does not have a light variant —
            // someone who wants one writes a `[light]` section, which
            // still overrides this.
            out.dark = derived;
            out.light = derived;
            // Re-apply everything the file set by hand, so an explicit
            // role beats the derived one regardless of whether it
            // appeared above or below `[wallpaper]`.
            for (mode, role, color) in &explicit {
                let target = match mode {
                    Mode::Dark => &mut out.dark,
                    Mode::Light => &mut out.light,
                };
                target.set_role(role, *color);
            }
        }
    }

    out
}

/// Turn the `[wallpaper]` lines into seeds.
///
/// The vocabulary is deliberately tiny and stated in the user's terms
/// rather than the palette's: someone reading colours off an image can
/// say "this is what it mostly is" and "this is what stands out", and
/// cannot reasonably be asked which of 37 roles a swatch belongs to.
///
/// `background` and `accent` are required; the rest refine the stage
/// marks and fall back to hue rotations off the accent.
fn seeds_from(
    lines: &[(usize, String, Color)],
    warnings: &mut Vec<String>,
) -> Option<Seeds> {
    let find = |name: &str| lines.iter().rev().find(|(_, k, _)| k == name).map(|(_, _, c)| *c);

    for (line_no, key, _) in lines {
        const KNOWN: [&str; 6] =
            ["background", "accent", "danger", "warning", "success", "text"];
        if !KNOWN.contains(&key.as_str()) {
            warnings.push(format!(
                "line {line_no}: `{key}` is not a wallpaper seed — use one of {}",
                KNOWN.join(", ")
            ));
        }
    }

    let (Some(background), Some(accent)) = (find("background"), find("accent")) else {
        warnings.push(
            "[wallpaper] needs `background` (the colour your wallpaper mostly is) and \
             `accent` (the one that stands out of it)"
                .to_string(),
        );
        return None;
    };

    Some(Seeds {
        background,
        accent,
        danger: find("danger"),
        warning: find("warning"),
        success: find("success"),
        text: find("text"),
    })
}

/// Strip a trailing `#` comment without eating a `#rrggbb` value.
///
/// A comment marker only counts outside quotes and at a position where a
/// value cannot already have started. Naively cutting at the first `#`
/// breaks every line in the file, since colours begin with one.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => {
                // A `#` directly after `=` is a bare (unquoted) colour,
                // which this format accepts: `accent = #89b4fa`.
                let before = line[..i].trim_end();
                if before.ends_with('=') {
                    continue;
                }
                return &line[..i];
            }
            _ => {}
        }
    }
    line
}

/// `#rgb`, `#rrggbb`, or `#rrggbbaa`. The leading `#` is optional so a
/// value pasted from a palette tool works either way.
pub fn parse_color(value: &str) -> Option<Color> {
    let hex = value.trim().trim_start_matches('#');
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |s: &str| u8::from_str_radix(s, 16).ok();
    let (r, g, b, a) = match hex.len() {
        // Shorthand: each digit doubled, so `#abc` is `#aabbcc`.
        3 => {
            let d: Vec<u8> = hex
                .chars()
                .map(|c| u8::from_str_radix(&c.to_string(), 16).unwrap_or(0))
                .collect();
            (d[0] * 17, d[1] * 17, d[2] * 17, 255)
        }
        6 => (byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?, 255),
        8 => (byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?, byte(&hex[6..8])?),
        _ => return None,
    };
    Some(Color::from_rgba8(r, g, b, a as f32 / 255.0))
}

/// The closest role name within a small edit distance, for "did you
/// mean". Returns `None` when nothing is close, because a confident
/// wrong suggestion is worse than none.
fn nearest_role(input: &str) -> Option<&'static str> {
    let lower = input.to_ascii_lowercase();
    let mut best: Option<(usize, &'static str)> = None;
    for role in Palette::ROLE_NAMES {
        let d = edit_distance(&lower, role);
        // Scale the threshold with length: two edits on a short name is a
        // different word, on a long one it is a typo.
        let limit = (role.len() / 4).clamp(1, 3);
        if d <= limit && best.is_none_or(|(bd, _)| d < bd) {
            best = Some((d, role));
        }
    }
    best.map(|(_, r)| r)
}

/// Levenshtein distance, two rows rather than a full matrix.
fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.is_empty() {
        return b.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

// ---------------------------------------------------------------------
// The contrast reading
// ---------------------------------------------------------------------

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

/// Check a user palette against the floors `theme.rs` asserts, and
/// describe what fails.
///
/// # Which pairs, and why only these
///
/// Each role is measured against **the surface it is genuinely painted
/// on**, which is the whole discipline of `theme.rs`'s own tests: the
/// out-point amber lives in the dark trim well, not on a panel, and
/// checking it against a light surface would demand a colour that is
/// then invisible where it actually appears.
///
/// The list is the subset where failure is *invisible* rather than ugly.
/// A garish theme is a choice; a slider with no thumb, or a trim
/// selection indistinguishable from its track, reads as the application
/// being broken. Those are the ones worth a sentence.
pub fn audit(palette: &Palette, mode: Mode) -> Vec<String> {
    let mode_name = match mode {
        Mode::Dark => "dark",
        Mode::Light => "light",
    };
    let p = palette;
    let mut out = Vec::new();

    // (role, colour, ground, ground name, floor, what breaks)
    let checks: [(&str, Color, Color, &str, f32, &str); 9] = [
        ("text_primary", p.text_primary, p.surface, "the panel", 4.5, "body text"),
        ("text_secondary", p.text_secondary, p.surface, "the panel", 4.5, "secondary text"),
        ("on_accent", p.on_accent, p.accent, "the accent fill", 4.5, "button labels"),
        ("accent", p.accent, p.surface, "the panel", 3.0, "the primary action"),
        (
            "control_track_off",
            p.control_track_off,
            p.surface_raised,
            "its card",
            3.0,
            "an off switch's body",
        ),
        (
            "control_knob",
            p.control_knob,
            p.control_track_off,
            "its track",
            3.0,
            "a slider thumb",
        ),
        (
            "trim_range_fill",
            p.trim_range_fill,
            p.trim_track,
            "the trim well",
            3.0,
            "the kept range",
        ),
        ("trim_out", p.trim_out, p.trim_track, "the trim well", 3.0, "the out-point handle"),
        ("playhead", p.playhead, p.trim_track, "the trim well", 3.0, "the playhead"),
    ];

    for (role, color, ground, ground_name, floor, breaks) in checks {
        let ratio = contrast(color, ground);
        if ratio < floor {
            out.push(format!(
                "{mode_name}: `{role}` is {ratio:.2}:1 against {ground_name} (needs {floor:.1}) \
                 — {breaks} will be hard to see"
            ));
        }
    }

    out
}

/// Hues that are too close to tell apart at the size they are drawn.
///
/// # Why this is separate from the contrast audit
///
/// Contrast cannot see it. A terracotta selection, a red playhead and an
/// amber out-point all clear their floors against the dark well
/// comfortably — each one is perfectly legible *on its own*. What fails
/// is telling them apart from each other, and that is a hue distance,
/// not a luminance ratio.
///
/// It matters most for exactly the palettes people want: a warm
/// monochrome wallpaper produces an accent sitting on top of the two
/// marks that must never be confused with it. The app cannot resolve
/// that itself — moving the marks breaks conventions the user did not
/// choose, moving the accent stops it being their colour — so it is
/// reported and left to them.
pub fn hue_conflicts(palette: &Palette, mode: Mode) -> Vec<String> {
    fn hue(c: Color) -> f32 {
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
    fn gap(a: Color, b: Color) -> f32 {
        let d = (hue(a) - hue(b)).abs();
        d.min(360.0 - d)
    }
    // A near-grey has no meaningful hue, so comparing it is noise.
    fn chromatic(c: Color) -> bool {
        let max = c.r.max(c.g).max(c.b);
        let min = c.r.min(c.g).min(c.b);
        max - min > 0.12
    }

    let mode_name = match mode {
        Mode::Dark => "dark",
        Mode::Light => "light",
    };
    let p = palette;
    let mut out = Vec::new();

    for (a_name, a, b_name, b, floor, why) in [
        (
            "trim_range_fill",
            p.trim_range_fill,
            "playhead",
            p.playhead,
            60.0,
            "the kept range and the current frame",
        ),
        (
            "trim_range_fill",
            p.trim_range_fill,
            "trim_out",
            p.trim_out,
            60.0,
            "the kept range and the out-point handle",
        ),
        ("trim_out", p.trim_out, "playhead", p.playhead, 20.0, "the out-point and the playhead"),
        ("success", p.success, "danger", p.danger, 60.0, "a finished export and a failed one"),
    ] {
        if !chromatic(a) || !chromatic(b) {
            continue;
        }
        let d = gap(a, b);
        if d < floor {
            out.push(format!(
                "{mode_name}: `{a_name}` and `{b_name}` are {d:.0}° apart in hue (want \
                 {floor:.0}°) — {why} will look like the same colour"
            ));
        }
    }
    out
}

/// A single line for the status pill, summarising everything found.
///
/// The pill has one line, so a config with nine problems cannot list
/// them. It names the count and the first, which is enough to know
/// something is wrong and where to start; the full list goes to stderr,
/// where someone editing a theme file is already looking.
pub fn summary(warnings: &[String]) -> Option<String> {
    match warnings.len() {
        0 => None,
        1 => Some(format!("Theme: {}", warnings[0])),
        n => Some(format!("Theme: {} (and {} more issue{})", warnings[0], n - 1, if n == 2 { "" } else { "s" })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_config_is_the_builtin_theme_and_not_a_warning() {
        let r = load_from(Path::new("/nonexistent/offcut/colors.toml"));
        assert!(!r.loaded);
        assert!(r.warnings.is_empty(), "a missing config is the normal case, not a problem");
        assert_eq!(r.dark, Palette::DARK);
        assert_eq!(r.light, Palette::LIGHT);
    }

    #[test]
    fn an_override_replaces_only_the_named_role() {
        let r = parse("[dark]\naccent = \"#ff0000\"\n");
        assert_eq!(r.dark.accent, Color::from_rgb8(0xFF, 0, 0));
        // Everything else is untouched, including the other mode.
        assert_eq!(r.dark.canvas, Palette::DARK.canvas);
        assert_eq!(r.light, Palette::LIGHT);
    }

    /// **A `#` starts a comment; a colour also starts with `#`.**
    ///
    /// Cutting the line at the first `#` turns `accent = "#89b4fa"` into
    /// `accent = "`, which breaks every line in a colour config — the
    /// one format where the naive comment rule is guaranteed to fail.
    #[test]
    fn a_comment_marker_does_not_eat_the_colour_it_precedes() {
        let r = parse(
            "# a leading comment\n\
             [dark]\n\
             accent = \"#89b4fa\"  # trailing comment\n\
             canvas = #1e1e2e\n",
        );
        assert_eq!(r.dark.accent, Color::from_rgb8(0x89, 0xB4, 0xFA));
        assert_eq!(r.dark.canvas, Color::from_rgb8(0x1E, 0x1E, 0x2E), "bare colours parse too");
        assert!(r.warnings.is_empty(), "clean config warned: {:?}", r.warnings);
    }

    /// A typo must be reported, not ignored.
    ///
    /// Silently dropping an unknown key produces a customisation that
    /// appears not to work, which sends the user looking for the bug in
    /// the app rather than in their file.
    #[test]
    fn an_unknown_role_is_reported_with_a_suggestion() {
        let r = parse("[dark]\ntext_secondry = \"#ffffff\"\n");
        assert_eq!(r.warnings.len(), 1);
        let w = &r.warnings[0];
        assert!(w.contains("text_secondry"), "{w}");
        assert!(w.contains("text_secondary"), "no suggestion offered: {w}");
    }

    /// Nonsense gets no confident suggestion.
    #[test]
    fn a_wildly_wrong_role_is_not_given_a_misleading_suggestion() {
        let r = parse("[dark]\nbackground_colour_of_everything = \"#fff\"\n");
        assert_eq!(r.warnings.len(), 1);
        assert!(
            !r.warnings[0].contains("did you mean"),
            "guessed at an unrelated name: {}",
            r.warnings[0]
        );
    }

    #[test]
    fn every_documented_role_name_is_actually_settable() {
        let mut p = Palette::DARK;
        for role in Palette::ROLE_NAMES {
            assert!(
                p.set_role(role, Color::BLACK),
                "`{role}` is advertised in ROLE_NAMES but set_role rejects it"
            );
        }
    }

    /// The config vocabulary must cover the whole palette.
    ///
    /// A role missing from `ROLE_NAMES` cannot be themed and gives no
    /// error when named — the user's line is simply reported as unknown,
    /// which reads as a typo in a name they copied correctly.
    #[test]
    fn the_role_vocabulary_covers_every_palette_field() {
        // Set every advertised role to a sentinel; nothing may be left
        // at its built-in value.
        let mut p = Palette::DARK;
        for role in Palette::ROLE_NAMES {
            p.set_role(role, Color::from_rgba(0.123, 0.456, 0.789, 1.0));
        }
        let sentinel = Color::from_rgba(0.123, 0.456, 0.789, 1.0);
        // A field left untouched would still equal DARK's value, and DARK
        // has no field equal to the sentinel.
        assert_eq!(
            p,
            Palette {
                canvas: sentinel,
                surface: sentinel,
                surface_raised: sentinel,
                surface_sunken: sentinel,
                surface_sunken_alt: sentinel,
                letterbox: sentinel,
                border: sentinel,
                border_raised: sentinel,
                accent: sentinel,
                on_accent: sentinel,
                control_knob: sentinel,
                control_track_off: sentinel,
                success: sentinel,
                danger: sentinel,
                origin_mark: sentinel,
                accent_tint_bg: sentinel,
                accent_tint_text: sentinel,
                trim_track: sentinel,
                trim_range_fill: sentinel,
                trim_range_edge: sentinel,
                trim_range_excluded: sentinel,
                playhead: sentinel,
                mute: sentinel,
                trim_out: sentinel,
                trim_out_hover: sentinel,
                stage_badge_text: sentinel,
                stage_badge_text_dim: sentinel,
                stage_shadow: sentinel,
                text_primary: sentinel,
                text_secondary: sentinel,
                text_secondary_alt: sentinel,
                text_muted: sentinel,
                text_muted_alt: sentinel,
                surface_hover: sentinel,
                accent_hover: sentinel,
                waveform: sentinel,
                mute_border: sentinel,
            },
            "a palette field is not reachable from ROLE_NAMES, so it cannot be themed"
        );
    }

    #[test]
    fn colours_parse_in_every_accepted_form() {
        assert_eq!(parse_color("#abc"), Some(Color::from_rgb8(0xAA, 0xBB, 0xCC)));
        assert_eq!(parse_color("#89b4fa"), Some(Color::from_rgb8(0x89, 0xB4, 0xFA)));
        assert_eq!(parse_color("89b4fa"), Some(Color::from_rgb8(0x89, 0xB4, 0xFA)));
        let with_alpha = parse_color("#00000080").expect("8-digit hex");
        assert!((with_alpha.a - 0.502).abs() < 0.01, "alpha was {}", with_alpha.a);

        for bad in ["", "#12345", "#gggggg", "not-a-colour", "#1234567"] {
            assert_eq!(parse_color(bad), None, "`{bad}` parsed as a colour");
        }
    }

    /// **The built-in palettes must pass their own audit.**
    ///
    /// This is the load-bearing test of the checker: if the shipped theme
    /// trips it, the thresholds are wrong and every user with a valid
    /// config gets a warning about colours they did not choose. It also
    /// keeps `audit` honest against `theme.rs`'s own assertions — the two
    /// measure the same pairs and must agree.
    #[test]
    fn the_shipped_theme_passes_the_check_users_are_held_to() {
        for (mode, p) in
            [(Mode::Dark, Palette::DARK), (Mode::Light, Palette::LIGHT)]
        {
            let found = audit(&p, mode);
            assert!(found.is_empty(), "the built-in theme fails its own audit: {found:?}");
        }
    }

    /// An invisible control is named, with the role and the measurement.
    #[test]
    fn an_unreadable_override_is_reported_against_the_surface_it_sits_on() {
        // A trim fill nearly identical to the well it is drawn in: the
        // exact defect (1.62:1) that shipped once in the real palette.
        let r = parse("[dark]\ntrim_range_fill = \"#161616\"\n");
        let found = audit(&r.dark, Mode::Dark);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("trim_range_fill"), "{}", found[0]);
        assert!(found[0].contains(":1"), "no measurement given: {}", found[0]);
    }

    /// The theme applies even when it fails the audit.
    ///
    /// "Warn but apply" is the contract: the reading is information, not
    /// a veto. A checker that silently substituted its own colours would
    /// make a user's config appear to be ignored.
    #[test]
    fn a_failing_theme_is_still_applied() {
        let r = parse("[dark]\ntrim_range_fill = \"#161616\"\n");
        assert_eq!(
            r.dark.trim_range_fill,
            Color::from_rgb8(0x16, 0x16, 0x16),
            "the audit overrode the user's choice instead of reporting it"
        );
    }

    #[test]
    fn a_role_outside_any_section_is_reported_rather_than_guessed() {
        let r = parse("accent = \"#ff0000\"\n");
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("before any"), "{}", r.warnings[0]);
        assert_eq!(r.dark, Palette::DARK, "applied to a mode it was never assigned to");
    }

    fn wall(extra: &str) -> Riced {
        parse(&format!(
            "[wallpaper]\nbackground = \"#111016\"\naccent = \"#d3685b\"\n{extra}"
        ))
    }

    #[test]
    fn two_seeds_are_enough_to_produce_a_whole_palette() {
        let r = wall("");
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        // Nothing was left at the built-in value.
        assert_ne!(r.dark.surface, Palette::DARK.surface);
        assert_ne!(r.dark.accent, Palette::DARK.accent);
        assert_eq!(r.dark.accent, Color::from_rgb8(0xD3, 0x68, 0x5B), "the accent is the seed");
    }

    #[test]
    fn a_wallpaper_section_without_its_two_required_seeds_says_so() {
        let r = parse("[wallpaper]\naccent = \"#d3685b\"\n");
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("background"), "{}", r.warnings[0]);
        assert_eq!(r.dark, Palette::DARK, "a partial seed set must not half-derive a palette");
    }

    #[test]
    fn an_unknown_seed_name_is_reported_with_the_list_of_real_ones() {
        let r = wall("foreground = \"#ffffff\"\n");
        assert!(r.warnings.iter().any(|w| w.contains("foreground") && w.contains("text")));
    }

    /// **An explicit role beats the derivation, wherever it appears.**
    ///
    /// The two features have to compose: derive the 37 relationships from
    /// an image, then correct the one or two you disagree with. If the
    /// derivation won, the only way to fix a single role would be to
    /// abandon the whole thing and hand-write all 37.
    #[test]
    fn an_explicit_role_overrides_the_derived_one_in_either_order() {
        let after = parse(
            "[wallpaper]\nbackground = \"#111016\"\naccent = \"#d3685b\"\n\
             [dark]\nplayhead = \"#6fc7e0\"\n",
        );
        assert_eq!(after.dark.playhead, Color::from_rgb8(0x6F, 0xC7, 0xE0));

        // And stated *before* the wallpaper section, which is the order a
        // person is more likely to get wrong.
        let before = parse(
            "[dark]\nplayhead = \"#6fc7e0\"\n\
             [wallpaper]\nbackground = \"#111016\"\naccent = \"#d3685b\"\n",
        );
        assert_eq!(
            before.dark.playhead,
            Color::from_rgb8(0x6F, 0xC7, 0xE0),
            "the derivation overwrote a role the user stated outright"
        );
    }

    /// **A monochrome wallpaper must not produce four identical marks.**
    ///
    /// This is the defect that made the feature worth testing rather than
    /// eyeballing. A warm image offers `danger`, `success` and `accent`
    /// within 25 degrees of each other, and honouring each seed at face
    /// value produced a green-labelled role that was the same red as the
    /// accent. Every one of them passed the contrast audit — they are
    /// individually legible, and collectively meaningless.
    #[test]
    fn a_warm_monochrome_wallpaper_still_yields_distinguishable_semantics() {
        let r = wall("danger = \"#90363b\"\nsuccess = \"#654847\"\n");
        let (s, d) = (r.dark.success, r.dark.danger);
        // Green stays green even though the seed offered a brown.
        assert!(
            s.g > s.r && s.g > s.b,
            "success is not green: {:?} — a brown seed was taken at face value",
            s
        );
        // Red stays red.
        assert!(d.r > d.g && d.r > d.b, "danger is not red: {d:?}");
    }

    /// A seed that *is* the conventional colour keeps its exact shade.
    ///
    /// The anchoring must not flatten every theme to the same three
    /// hues — a wallpaper's particular green is what ties the palette to
    /// the image, and it is honoured whenever it is recognisably green.
    #[test]
    fn a_seed_near_its_convention_is_used_rather_than_the_anchor() {
        let with_green = wall("success = \"#39a85a\"\n");
        let without = wall("");
        assert_ne!(
            with_green.dark.success, without.dark.success,
            "a genuinely green `success` seed was ignored in favour of the anchor"
        );
    }

    /// The derived palette must pass the contrast audit it will be held
    /// to, for a range of seeds rather than the one that was tuned.
    #[test]
    fn derived_palettes_clear_the_contrast_audit() {
        for (bg, accent) in [
            ("#111016", "#d3685b"), // the warm wallpaper
            ("#1e1e2e", "#89b4fa"), // a cool one
            ("#f5f5f7", "#0071e3"), // a light one
            ("#000000", "#ffffff"), // degenerate: pure black and white
            ("#404040", "#808080"), // fully desaturated
        ] {
            let r = parse(&format!(
                "[wallpaper]\nbackground = \"{bg}\"\naccent = \"{accent}\"\n"
            ));
            let found = audit(&r.dark, Mode::Dark);
            assert!(found.is_empty(), "seeds {bg}/{accent} derived a failing palette: {found:?}");
        }
    }

    /// **Hue collisions are reported, not silently resolved.**
    ///
    /// A warm accent lands on top of the red playhead and amber
    /// out-point, and every one of those colours passes its contrast
    /// floor — the failure is that they cannot be told from each other.
    /// The app cannot fix it alone: moving the marks breaks conventions
    /// the user did not choose, and moving the accent stops it being
    /// their colour. So it says so.
    #[test]
    fn a_warm_accent_colliding_with_the_stage_marks_is_reported() {
        let r = wall("");
        let found = hue_conflicts(&r.dark, Mode::Dark);
        assert!(
            found.iter().any(|w| w.contains("playhead")),
            "a terracotta selection beside a red playhead went unreported: {found:?}"
        );
        assert!(found[0].contains('°'), "no measurement given: {}", found[0]);
    }

    /// The built-in theme has no hue collisions, or every user would be
    /// warned about colours they did not choose.
    #[test]
    fn the_shipped_theme_has_no_hue_collisions() {
        for (mode, p) in [(Mode::Dark, Palette::DARK), (Mode::Light, Palette::LIGHT)] {
            assert!(hue_conflicts(&p, mode).is_empty(), "the built-in palette collides with itself");
        }
    }

    /// A grey theme must not be reported as a hue collision.
    ///
    /// Two near-greys are 0° apart by arithmetic and identical by
    /// intent — warning about them would make a deliberately monochrome
    /// theme unusable.
    #[test]
    fn a_greyscale_theme_is_not_warned_about_hue() {
        let r = parse(
            "[dark]\ntrim_range_fill = \"#888888\"\nplayhead = \"#cccccc\"\n\
             trim_out = \"#aaaaaa\"\n",
        );
        assert!(
            hue_conflicts(&r.dark, Mode::Dark).is_empty(),
            "a greyscale theme was warned about hue distance"
        );
    }

    /// A recorded theme whose file has since been deleted must not
    /// resurrect as an error.
    ///
    /// `selected_theme` filters against what actually exists, so a
    /// deleted theme falls back to the built-in palette rather than
    /// leaving the app pointing at a file that is not there.
    #[test]
    fn a_selection_naming_a_missing_theme_resolves_to_nothing() {
        // `available_themes` reads the real config dir, which in a test
        // environment has no themes — so any name is "missing".
        assert!(
            !available_themes().contains(&"definitely-not-a-real-theme".to_string()),
            "test assumption broken: that theme exists"
        );
    }

    /// The themes directory sits under the config directory, and the
    /// selection beside it.
    ///
    /// Pinned because the three paths have to agree: a picker reading one
    /// directory while the loader reads another is a switch that appears
    /// to do nothing.
    /// The seeding marker must sit beside the themes it records, not
    /// inside them.
    ///
    /// A marker file inside `themes/` would be listed by
    /// `available_themes` as a theme, or filtered by extension and then
    /// silently reseeded forever. It belongs one level up.
    #[test]
    fn the_seed_marker_is_not_mistaken_for_a_theme() {
        let Some(dir) = config_dir() else { return };
        let marker = dir.join(".examples-installed");
        assert_eq!(marker.parent(), Some(dir.as_path()));
        assert!(
            !marker.to_string_lossy().contains("/themes/"),
            "the marker lives inside the themes directory and would be scanned as one"
        );
        // And it is not a .toml, so even a stray copy could not be listed.
        assert_ne!(marker.extension().and_then(|e| e.to_str()), Some("toml"));
    }

    #[test]
    fn the_theme_paths_are_all_under_one_config_directory() {
        // Only meaningful when a config location exists at all.
        let (Some(dir), Some(themes), Some(colors)) =
            (config_dir(), themes_dir(), config_path())
        else {
            return;
        };
        assert!(themes.starts_with(&dir), "themes dir is outside the config dir");
        assert!(colors.starts_with(&dir), "colors.toml is outside the config dir");
        assert_eq!(themes.file_name().unwrap(), "themes");
        assert_eq!(colors.file_name().unwrap(), "colors.toml");
    }

    #[test]
    fn the_pill_summary_names_a_count_it_cannot_list() {
        assert_eq!(summary(&[]), None);
        let one = summary(&["a".into()]).unwrap();
        assert!(!one.contains("more"));
        let three = summary(&["a".into(), "b".into(), "c".into()]).unwrap();
        assert!(three.contains("2 more issues"), "{three}");
    }
}

// ---------------------------------------------------------------------
// Deriving a whole palette from a handful of wallpaper colours
// ---------------------------------------------------------------------

/// HSL, as the working space for deriving a ramp.
///
/// Not OKLCH, despite it being the better space for this: it would mean
/// a colour-science dependency for one screen's worth of theming, and
/// the operations here are "hold this hue, walk the lightness" — which
/// HSL expresses directly and predictably enough at these amplitudes.
#[derive(Copy, Clone, Debug)]
struct Hsl {
    h: f32,
    s: f32,
    l: f32,
}

fn to_hsl(c: Color) -> Hsl {
    let (r, g, b) = (c.r, c.g, c.b);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d.abs() < 1e-6 {
        return Hsl { h: 0.0, s: 0.0, l };
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let h = if max == r {
        60.0 * (((g - b) / d) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsl { h: if h < 0.0 { h + 360.0 } else { h }, s, l }
}

fn from_hsl(v: Hsl) -> Color {
    let (h, s, l) = (v.h.rem_euclid(360.0), v.s.clamp(0.0, 1.0), v.l.clamp(0.0, 1.0));
    if s < 1e-6 {
        return Color::from_rgb(l, l, l);
    }
    let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
    let p = 2.0 * l - q;
    let f = |mut t: f32| {
        if t < 0.0 {
            t += 1.0;
        }
        if t > 1.0 {
            t -= 1.0;
        }
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    };
    let hk = h / 360.0;
    Color::from_rgb(f(hk + 1.0 / 3.0), f(hk), f(hk - 1.0 / 3.0))
}

/// WCAG relative luminance of an HSL lightness step, used to solve for a
/// lightness that hits a contrast target.
fn lightness_for_contrast(hue: f32, sat: f32, ground: Color, target: f32, lighter: bool) -> Color {
    // Binary search on L. Monotonic in luminance, so 24 halvings lands
    // well inside a single 8-bit step.
    let (mut lo, mut hi) = if lighter { (0.5f32, 1.0f32) } else { (0.0f32, 0.5f32) };
    let mut best = from_hsl(Hsl { h: hue, s: sat, l: if lighter { 1.0 } else { 0.0 } });
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        let c = from_hsl(Hsl { h: hue, s: sat, l: mid });
        if contrast(c, ground) >= target {
            best = c;
            // Move toward the ground: the *least* extreme colour that
            // still clears the floor keeps the theme's character.
            if lighter {
                hi = mid;
            } else {
                lo = mid;
            }
        } else if lighter {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    best
}

/// The seed colours a user pulls out of their wallpaper.
///
/// Only `background` and `accent` are required, because those are the
/// two a person can actually name by looking at an image: the field it
/// mostly is, and the thing that stands out of it. Everything else is
/// optional and falls back to a hue rotation of the accent.
#[derive(Debug, Clone)]
pub struct Seeds {
    pub background: Color,
    pub accent: Color,
    pub danger: Option<Color>,
    pub warning: Option<Color>,
    pub success: Option<Color>,
    pub text: Option<Color>,
}

/// Build a full 37-role palette from a few wallpaper colours.
///
/// # What this does and does not decide
///
/// It derives **relationships**, not taste. The seeds set the hue and
/// character; this fixes the things that are not a matter of preference:
/// that a card is a step above its panel, that a fill carries readable
/// text, that the three stage marks stay far enough apart in hue to be
/// told apart at 13px.
///
/// Every role that carries text or must be seen is solved for its
/// contrast target against the surface it is genuinely painted on,
/// rather than being a fixed offset that happens to work for one seed.
/// That is the whole reason this is worth generating instead of asking
/// the user to fill in 37 values: the failures are in the pairs, and the
/// pairs are exactly what a person cannot eyeball.
pub fn derive(seeds: &Seeds) -> Palette {
    let bg = to_hsl(seeds.background);
    let acc = to_hsl(seeds.accent);

    // The window ground keeps the wallpaper's hue but is pinned to a
    // usable lightness: a stage surround has to be dark enough to judge
    // an image against, and a wallpaper's dominant colour is often not.
    // Saturation is capped hard — a strongly tinted panel next to
    // footage reads as a colour cast in the footage.
    let tint = bg.s.min(0.18);
    let dark = bg.l < 0.5;

    let step = |l: f32| from_hsl(Hsl { h: bg.h, s: tint, l: l.clamp(0.0, 1.0) });

    // The surface ramp, as five steps around the seed's own lightness.
    // Dark themes step *up* from the ground, light themes step down, so
    // "raised" is always the one that catches more light.
    let (canvas, surface, raised, sunken, sunken_alt) = if dark {
        (step(0.09), step(0.14), step(0.22), step(0.07), step(0.05))
    } else {
        (step(0.92), step(0.96), step(1.0), step(0.88), step(0.83))
    };

    // Text is solved against the panel, not offset from it.
    let text_primary = seeds
        .text
        .unwrap_or_else(|| lightness_for_contrast(bg.h, tint * 0.5, surface, 12.0, dark));
    let text_secondary = lightness_for_contrast(bg.h, tint * 0.6, surface, 4.6, dark);
    let text_secondary_alt = lightness_for_contrast(bg.h, tint * 0.6, surface, 7.0, dark);
    let text_muted = text_secondary;
    // Disabled text: deliberately below the body floor but above 3:1,
    // because dimness *is* the disabled signal.
    let text_muted_alt = lightness_for_contrast(bg.h, tint * 0.6, surface, 3.2, dark);

    // The accent keeps the seed's hue and saturation, but its lightness
    // is solved so the fill clears 3:1 on the panel. A wallpaper accent
    // is chosen for looking good in an image, not for carrying a button.
    let accent = if contrast(seeds.accent, surface) >= 3.2 {
        seeds.accent
    } else {
        lightness_for_contrast(acc.h, acc.s.max(0.35), surface, 3.2, dark)
    };
    // Whichever of near-black or near-white reads better on the fill.
    let on_accent = if contrast(Color::WHITE, accent) >= contrast(step(0.06), accent) {
        Color::WHITE
    } else {
        step(0.06)
    };
    let accent_hover = {
        let a = to_hsl(accent);
        from_hsl(Hsl { l: (a.l + if dark { 0.08 } else { -0.08 }).clamp(0.0, 1.0), ..a })
    };
    let accent_tint_text = lightness_for_contrast(acc.h, acc.s.max(0.35), surface, 4.6, dark);
    let accent_tint_bg = {
        let a = to_hsl(accent);
        from_hsl(Hsl { h: a.h, s: a.s * 0.45, l: if dark { 0.18 } else { 0.9 } })
    };

    // # The stage marks
    //
    // These are the three that must never be confused: the kept range,
    // the out-point, and the playhead. Hues are taken from the seeds
    // where given and otherwise rotated off the accent by fixed amounts
    // that guarantee the separations `theme.rs` asserts.
    let well = step(if dark { 0.06 } else { 0.08 });

    // # A seed is only honoured if it is actually a different colour
    //
    // A wallpaper is not a palette. A warm monochrome image — a
    // terracotta photograph, a sepia print — yields five swatches within
    // 25 degrees of each other, and taking each one at face value
    // produces an accent, a success, a danger and a playhead that are
    // all the same red. The picker was working; the image simply does
    // not contain four distinguishable hues.
    //
    // So each mark's hue is the seed's *if* it is far enough from the
    // accent to be told apart, and a rotation otherwise. The contrast
    // audit cannot catch this — every one of those reds passes its
    // floor against the well — which is exactly why the separation is
    // enforced at derivation instead of being left to the reading.
    // The semantic hues do **not** rotate off the accent, because they
    // are not free: red means stop, amber means caution and green means
    // done in every editor a person has used, and a "danger" colour
    // rotated 150 degrees off a red accent is green — which is worse
    // than a collision, because it is confidently wrong.
    //
    // So each keeps its conventional anchor, and only follows its seed
    // when the seed is recognisably that colour. A wallpaper's warm
    // brown offered as `success` is not a green, and honouring it would
    // produce three identical reds.
    const RED: f32 = 0.0;
    const AMBER: f32 = 40.0;
    const GREEN: f32 = 140.0;
    let near = |seed: Option<Color>, anchor: f32| -> f32 {
        match seed.map(|c| to_hsl(c).h) {
            Some(h) => {
                let d = (h - anchor).abs();
                // Within 45 degrees of the convention, the seed is that
                // colour and its exact shade is worth keeping — it is
                // what ties the theme to the image.
                if d.min(360.0 - d) <= 45.0 { h } else { anchor }
            }
            None => anchor,
        }
    };
    let danger_h = near(seeds.danger, RED);
    let warning_h = near(seeds.warning, AMBER);
    let success_h = near(seeds.success, GREEN);



    // Marks live on the dark well in both modes, so they are solved
    // against it and are always the lighter of the two options.
    let mark = |h: f32, target: f32| lightness_for_contrast(h, 0.65, well, target, true);
    let playhead = mark(danger_h, 4.5);
    let trim_out = mark(warning_h, 4.5);

    Palette {
        canvas,
        surface,
        surface_raised: raised,
        surface_sunken: sunken,
        surface_sunken_alt: sunken_alt,
        // The picture's surround is black in every theme, derived or not.
        letterbox: Color::BLACK,
        border: step(if dark { 0.24 } else { 0.84 }),
        border_raised: step(if dark { 0.32 } else { 0.78 }),
        accent,
        on_accent,
        control_knob: Color::WHITE,
        // # The off track, which cannot simply be "lighter than the card"
        //
        // It has to clear 3:1 against the card *and* carry the white
        // knob, and on a light theme the card is already near-white:
        // searching upward returns white, giving 1.00:1 against both.
        // That is the white-knob-on-white-card defect this palette has
        // shipped twice, arrived at a third way.
        //
        // So the direction is chosen from the card rather than from the
        // mode: go darker when the card is light, lighter when it is
        // dark, which is the only direction with room in either case.
        control_track_off: {
            let card_is_light = luminance(raised) > 0.4;
            let track =
                lightness_for_contrast(bg.h, tint * 0.5, raised, 3.1, !card_is_light);
            // The knob is white, so a track that fails to carry it is
            // pushed down until it does — the card floor is already met
            // and darkening only increases that margin.
            if contrast(Color::WHITE, track) >= 3.0 {
                track
            } else {
                lightness_for_contrast(bg.h, tint * 0.5, Color::WHITE, 3.0, false)
            }
        },
        success: lightness_for_contrast(success_h, 0.55, surface, 4.6, dark),
        danger: lightness_for_contrast(danger_h, 0.6, surface, 4.6, dark),
        origin_mark: text_secondary_alt,
        accent_tint_bg,
        accent_tint_text,
        trim_track: well,
        // The selection is **always** the accent. It is the one thing the
        // accent means, and a trim range in a different colour from the
        // active tab and the primary button would be a second accent
        // with no stated meaning. Where a warm accent crowds the red
        // playhead or amber out-point, that is reported rather than
        // silently resolved — see `hue_conflicts`.
        trim_range_fill: accent,
        trim_range_edge: Color::WHITE,
        trim_range_excluded: Color::from_rgba(1.0, 1.0, 1.0, 0.06),
        playhead,
        mute: playhead,
        trim_out,
        trim_out_hover: {
            let t = to_hsl(trim_out);
            from_hsl(Hsl { l: (t.l + 0.1).min(1.0), ..t })
        },
        stage_badge_text: Color::WHITE,
        stage_badge_text_dim: Color::from_rgb(0.78, 0.78, 0.78),
        stage_shadow: Color::from_rgba(0.0, 0.0, 0.0, 0.55),
        text_primary,
        text_secondary,
        text_secondary_alt,
        text_muted,
        text_muted_alt,
        surface_hover: step(if dark { 0.19 } else { 0.91 }),
        accent_hover,
        waveform: step(if dark { 0.34 } else { 0.72 }),
        mute_border: {
            let d = to_hsl(playhead);
            from_hsl(Hsl { h: d.h, s: 0.35, l: if dark { 0.26 } else { 0.82 } })
        },
    }
}
