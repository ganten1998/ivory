//! Named color themes (D-UI-10).
//!
//! A theme is DATA. Nothing downstream branches on a theme id — adding one
//! means adding a single `const` to [`THEMES`] and nothing else, anywhere.
//!
//! **Split of authority, deliberately.** The five user-editable key-fill colors
//! stay in [`Settings`] and keep their existing dark-mode swap; a theme only
//! *stamps* them once, at selection. The [`Palette`] owns exactly the colors
//! that were hardcoded constants before this module existed. That boundary is
//! the safety argument: the 88-key fill path and the four color pickers behave
//! exactly as they always did, and `Classic` is pinned byte-identical to the
//! shipped look by `classic_palette_matches_the_shipped_constants`.
//!
//! **The gate cannot fail.** [`resolve`] is the only place tier is consulted.
//! An unknown or locked id degrades to Classic while the *requested* id stays
//! untouched in settings.json — install a license, or move the config to a
//! licensed machine, and the theme returns with no reconfiguration.

use crate::settings::{Rgb, Settings};
use egui::Color32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Free,
    Supporter,
}

/// Phosphorescent key glow. Alpha lives here, never in settings.json — that
/// file's color format is Python-compatible `#rrggbb` with no alpha channel.
#[derive(Clone, Copy)]
pub struct Glow {
    /// Halo color for a held key.
    pub color: Color32,
    /// Concentric bands: (outset in points at 100% size, alpha gain 0..1).
    pub bands: &'static [(f32, f32)],
    /// Constant faint charge on *idle* keys. Reserved for the idle-bloom
    /// pass; the held-key halo is what ships first.
    #[allow(dead_code)]
    pub idle_alpha: u8,
}

/// One color per role that used to be a hardcoded constant.
#[derive(Clone, Copy)]
pub struct Palette {
    // piano chrome (piano.rs)
    pub piano_bg: Color32,
    pub key_separator: Color32,
    pub black_key_outline: Color32,
    // chord strip (chord_strip.rs)
    pub chord_bg: Color32,
    pub chord_text: Color32,
    // context menu (menu.rs)
    pub menu_bg: Color32,
    pub menu_text: Color32,
    pub menu_sel: Color32,
    pub menu_sep: Color32,
    // dialogs (dialogs.rs)
    pub dialog_bg: Color32,
    pub dialog_text: Color32,
    pub dialog_button_bg: Color32,
    pub dialog_button_hover: Color32,
    pub dialog_button_border: Color32,
}

/// The five key-fill colors a theme stamps into `Settings` when selected.
/// Applying a theme is therefore visible in the color pickers, and undoable
/// with them — the theme does not own these afterwards.
#[derive(Clone, Copy)]
pub struct KeyPreset {
    pub white_idle: Rgb,
    pub black_idle: Rgb,
    pub white_active: Rgb,
    pub black_active: Rgb,
    pub sustain: Rgb,
}

pub struct Theme {
    /// Stable key for settings.json. Never rename a shipped id.
    pub id: &'static str,
    /// Human name (store copy, About line). Kept even where only `menu_label`
    /// is drawn today so a theme is never identified by its id in prose.
    #[allow(dead_code)]
    pub name: &'static str,
    /// Pre-rendered menu row (const, so no allocation in the draw path).
    pub menu_label: &'static str,
    pub tier: Tier,
    pub light: Palette,
    pub dark: Palette,
    pub keys: KeyPreset,
    /// Themes without a glow render exactly as before.
    pub glow: Option<Glow>,
    /// Themes designed for one mode only pin it, so selecting them cannot land
    /// the user in a variant that was never art-directed.
    pub force_dark: Option<bool>,
}

impl Theme {
    /// Menu row label. Supporter themes carry a marker so the roster reads as
    /// an invitation rather than a locked door.
    pub fn menu_label(&self) -> &'static str {
        self.menu_label
    }

    pub fn palette(&self, dark_mode: bool) -> &Palette {
        if dark_mode { &self.dark } else { &self.light }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    Rgb { r, g, b }
}

// ── Classic: the shipped look, reproduced exactly ───────────────────────────

const CLASSIC_DARK: Palette = Palette {
    piano_bg: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    key_separator: Color32::from_rgb(153, 153, 153),
    black_key_outline: Color32::from_rgb(204, 204, 204),
    chord_bg: Color32::BLACK,
    chord_text: Color32::from_rgb(232, 220, 192),
    menu_bg: Color32::from_rgb(0x00, 0x00, 0x00),
    menu_text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
    menu_sel: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    menu_sep: Color32::from_rgb(0xE8, 0xDC, 0xC0),
    dialog_bg: Color32::from_rgb(0x00, 0x00, 0x00),
    dialog_text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
    dialog_button_bg: Color32::from_rgb(0x1a, 0x1a, 0x1a),
    dialog_button_hover: Color32::from_rgb(0x2a, 0x2a, 0x2a),
    dialog_button_border: Color32::from_rgb(0xE8, 0xDC, 0xC0),
};

const CLASSIC_LIGHT: Palette = Palette {
    piano_bg: Color32::from_rgb(0xE8, 0xE8, 0xE8),
    key_separator: Color32::from_rgb(92, 63, 31),
    black_key_outline: Color32::from_rgb(139, 115, 85),
    chord_bg: Color32::BLACK,
    chord_text: Color32::from_rgb(232, 220, 192),
    menu_bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
    menu_text: Color32::from_rgb(0x00, 0x00, 0x00),
    menu_sel: Color32::from_rgb(0xd4, 0xc8, 0xb0),
    menu_sep: Color32::from_rgb(0x00, 0x00, 0x00),
    dialog_bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
    dialog_text: Color32::from_rgb(0x00, 0x00, 0x00),
    dialog_button_bg: Color32::from_rgb(0xd4, 0xc8, 0xb0),
    dialog_button_hover: Color32::from_rgb(0xc0, 0xb4, 0x9c),
    dialog_button_border: Color32::from_rgb(0x00, 0x00, 0x00),
};

/// Shipped defaults, duplicated here so the parity test can assert the stamp
/// is a no-op against a fresh `Settings`.
const CLASSIC_KEYS: KeyPreset = KeyPreset {
    white_idle: rgb(0xE8, 0xDC, 0xC0),
    black_idle: rgb(0x1a, 0x1a, 0x1a),
    white_active: rgb(0x6c, 0x9b, 0xd2),
    black_active: rgb(0x6c, 0x9b, 0xd2),
    sustain: rgb(0xd2, 0xa3, 0x6c),
};

pub const CLASSIC: Theme = Theme {
    id: "classic",
    menu_label: "Theme: Classic",
    name: "Classic",
    tier: Tier::Free,
    light: CLASSIC_LIGHT,
    dark: CLASSIC_DARK,
    keys: CLASSIC_KEYS,
    glow: None,
    force_dark: None,
};

// ── Graphite (free): a dark mode someone actually designed ──────────────────
// Classic's dark mode is an inversion — it swaps the idle fills and leaves the
// chrome alone, which is why its black-key rings measure CR 1.18 and vanish.
// Graphite exists so the free tier has a properly art-directed dark option.

const GRAPHITE: Palette = Palette {
    piano_bg: Color32::from_rgb(0x14, 0x15, 0x17),
    key_separator: Color32::from_rgb(0x6B, 0x70, 0x7A),
    black_key_outline: Color32::from_rgb(0x8A, 0x90, 0x9B),
    chord_bg: Color32::from_rgb(0x0D, 0x0E, 0x10),
    chord_text: Color32::from_rgb(0xD7, 0xDD, 0xE6),
    menu_bg: Color32::from_rgb(0x0D, 0x0E, 0x10),
    menu_text: Color32::from_rgb(0xD7, 0xDD, 0xE6),
    menu_sel: Color32::from_rgb(0x24, 0x27, 0x2C),
    menu_sep: Color32::from_rgb(0x4A, 0x4F, 0x58),
    dialog_bg: Color32::from_rgb(0x0D, 0x0E, 0x10),
    dialog_text: Color32::from_rgb(0xD7, 0xDD, 0xE6),
    dialog_button_bg: Color32::from_rgb(0x1C, 0x1F, 0x23),
    dialog_button_hover: Color32::from_rgb(0x2A, 0x2E, 0x34),
    dialog_button_border: Color32::from_rgb(0x6B, 0x70, 0x7A),
};

// ── Centennial (supporter): DX7II Centennial, 1987 ──────────────────────────
// Silver casing, gold-plated controls, and 76 keys that glow in the dark.
// Phosphorescence only reads against darkness, so this is a dark theme: the
// silver lives in the linework, and gold appears in exactly two places — the
// chord label, and the whole keyboard the moment the sustain pedal goes down
// (gold-plated controls; pedalling is a control).

const CENTENNIAL_P: Palette = Palette {
    piano_bg: Color32::from_rgb(0x08, 0x09, 0x0A),
    key_separator: Color32::from_rgb(0x98, 0xA4, 0x9D), // machined silver seam
    black_key_outline: Color32::from_rgb(0xB0, 0xA8, 0x94), // champagne ring
    chord_bg: Color32::from_rgb(0x0A, 0x0A, 0x0B),
    chord_text: Color32::from_rgb(0xE6, 0xCB, 0x84), // champagne gold
    menu_bg: Color32::from_rgb(0x0A, 0x0A, 0x0B),
    menu_text: Color32::from_rgb(0xE6, 0xCB, 0x84),
    menu_sel: Color32::from_rgb(0x1A, 0x1A, 0x18),
    menu_sep: Color32::from_rgb(0x8C, 0x7B, 0x49),
    dialog_bg: Color32::from_rgb(0x0A, 0x0A, 0x0B),
    dialog_text: Color32::from_rgb(0xE6, 0xCB, 0x84),
    dialog_button_bg: Color32::from_rgb(0x1A, 0x1A, 0x18),
    dialog_button_hover: Color32::from_rgb(0x26, 0x26, 0x20),
    dialog_button_border: Color32::from_rgb(0x8C, 0x7B, 0x49),
};

pub const THEMES: &[Theme] = &[
    CLASSIC,
    Theme {
        id: "graphite",
        menu_label: "Theme: Graphite",
        name: "Graphite",
        tier: Tier::Free,
        light: GRAPHITE,
        dark: GRAPHITE,
        keys: KeyPreset {
            white_idle: rgb(0xDE, 0xE3, 0xEA),
            black_idle: rgb(0x1A, 0x1C, 0x20),
            white_active: rgb(0x6C, 0x9B, 0xD2),
            black_active: rgb(0x8F, 0xB6, 0xE4),
            sustain: rgb(0xD2, 0xA3, 0x6C),
        },
        glow: None,
        // Graphite IS the dark option; its single palette is used either way,
        // but pinning avoids the idle-swap flipping its keys.
        force_dark: Some(false),
    },
    Theme {
        id: "centennial",
        menu_label: "Theme: Centennial \u{2726}",
        name: "Centennial",
        tier: Tier::Supporter,
        light: CENTENNIAL_P,
        dark: CENTENNIAL_P,
        keys: KeyPreset {
            white_idle: rgb(0x41, 0x54, 0x4C),  // charged phosphor at rest
            black_idle: rgb(0x0C, 0x0F, 0x0E),  // gloss black, a hair green
            white_active: rgb(0xB7, 0xFF, 0xDD), // phosphor flare
            black_active: rgb(0xCD, 0xFF, 0xEA), // hotter: narrow keys need it
            sustain: rgb(0xE8, 0xC4, 0x6A),      // gold plate
        },
        glow: Some(Glow {
            color: Color32::from_rgb(0x9B, 0xFF, 0xD4),
            // Bands, not filled expanded rects: a filled rect would blow the
            // key core out to white and destroy the phosphor read.
            bands: &[(2.0, 0.20), (5.0, 0.11), (10.0, 0.055)],
            idle_alpha: 18,
        }),
        force_dark: Some(false),
    },
];

/// Index into [`THEMES`] of the theme currently in force.
///
/// A global rather than a threaded parameter on purpose: this is genuinely
/// process-wide UI state read by four unrelated draw paths (piano, chord strip,
/// menu, dialogs), and threading a `&Palette` through every one of them —
/// including the detached chord viewport — would touch far more code than the
/// feature is worth. It is only ever written from the menu handler.
static ACTIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Set the theme in force. Call after resolving against the license.
pub fn set_active(theme: &'static Theme) {
    let idx = THEMES.iter().position(|t| t.id == theme.id).unwrap_or(0);
    ACTIVE.store(idx, std::sync::atomic::Ordering::Relaxed);
}

pub fn active() -> &'static Theme {
    let idx = ACTIVE.load(std::sync::atomic::Ordering::Relaxed);
    THEMES.get(idx).unwrap_or(&CLASSIC)
}

/// Palette of the active theme for the given mode. This is what the draw paths
/// call in place of the constants they used to hardcode.
pub fn palette(dark_mode: bool) -> &'static Palette {
    active().palette(dark_mode)
}

pub fn by_id(id: &str) -> Option<&'static Theme> {
    THEMES.iter().find(|t| t.id == id)
}

/// The theme actually in force. Infallible by construction: an unknown id, or
/// a supporter theme without a license, yields Classic. The caller's stored
/// preference is never modified as a side effect of this call.
pub fn resolve(requested: &str, licensed: bool) -> &'static Theme {
    match by_id(requested) {
        Some(t) if t.tier == Tier::Free || licensed => t,
        _ => &CLASSIC,
    }
}

/// Copy a theme's key preset into settings. Called only when the user picks a
/// theme, so the color pickers keep showing (and editing) real values.
pub fn stamp_keys(theme: &Theme, s: &mut Settings) {
    s.white_key_idle_color = theme.keys.white_idle;
    s.black_key_idle_color = theme.keys.black_idle;
    s.white_key_active_color = theme.keys.white_active;
    s.black_key_active_color = theme.keys.black_active;
    s.sustain_color = theme.keys.sustain;
    if let Some(dark) = theme.force_dark {
        s.dark_mode = dark;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PARITY LOCK. Every Classic role must equal the literal at the site it
    /// replaced. If this passes, the theme refactor is provably invisible to a
    /// user who never opens the picker.
    #[test]
    fn classic_palette_matches_the_shipped_constants() {
        let d = &CLASSIC.dark;
        assert_eq!(d.piano_bg, Color32::from_rgb(0x1a, 0x1a, 0x1a));
        assert_eq!(d.key_separator, Color32::from_rgb(153, 153, 153));
        assert_eq!(d.black_key_outline, Color32::from_rgb(204, 204, 204));
        assert_eq!(d.menu_bg, Color32::from_rgb(0x00, 0x00, 0x00));
        assert_eq!(d.menu_text, Color32::from_rgb(0xE8, 0xDC, 0xC0));
        assert_eq!(d.menu_sel, Color32::from_rgb(0x1a, 0x1a, 0x1a));
        assert_eq!(d.menu_sep, Color32::from_rgb(0xE8, 0xDC, 0xC0));
        assert_eq!(d.dialog_button_hover, Color32::from_rgb(0x2a, 0x2a, 0x2a));
        assert_eq!(d.dialog_button_border, Color32::from_rgb(0xE8, 0xDC, 0xC0));

        let l = &CLASSIC.light;
        assert_eq!(l.piano_bg, Color32::from_rgb(0xE8, 0xE8, 0xE8));
        assert_eq!(l.key_separator, Color32::from_rgb(92, 63, 31));
        assert_eq!(l.black_key_outline, Color32::from_rgb(139, 115, 85));
        assert_eq!(l.menu_bg, Color32::from_rgb(0xE8, 0xDC, 0xC0));
        assert_eq!(l.menu_sel, Color32::from_rgb(0xd4, 0xc8, 0xb0));
        assert_eq!(l.dialog_button_hover, Color32::from_rgb(0xc0, 0xb4, 0x9c));

        // Chord strip is mode-independent (spec §5.3).
        for p in [&CLASSIC.light, &CLASSIC.dark] {
            assert_eq!(p.chord_bg, Color32::BLACK);
            assert_eq!(p.chord_text, Color32::from_rgb(232, 220, 192));
        }
    }

    /// Selecting Classic must not change a fresh install.
    #[test]
    fn stamping_classic_is_a_no_op_on_defaults() {
        let mut s = Settings::default();
        let before = (
            s.white_key_idle_color,
            s.black_key_idle_color,
            s.white_key_active_color,
            s.black_key_active_color,
            s.sustain_color,
        );
        stamp_keys(&CLASSIC, &mut s);
        assert_eq!(
            before,
            (
                s.white_key_idle_color,
                s.black_key_idle_color,
                s.white_key_active_color,
                s.black_key_active_color,
                s.sustain_color
            )
        );
    }

    #[test]
    fn classic_is_the_default_and_is_free() {
        assert_eq!(THEMES[0].id, CLASSIC.id);
        assert_eq!(CLASSIC.tier, Tier::Free);
        assert!(CLASSIC.glow.is_none(), "Classic must render exactly as before");
    }

    #[test]
    fn supporter_themes_are_gated_but_never_error() {
        assert_eq!(resolve("centennial", false).id, "classic", "locked -> Classic");
        assert_eq!(resolve("centennial", true).id, "centennial");
        // Free themes ignore the license entirely.
        assert_eq!(resolve("graphite", false).id, "graphite");
        // Garbage and empty are not errors.
        assert_eq!(resolve("no-such-theme", true).id, "classic");
        assert_eq!(resolve("", false).id, "classic");
    }

    #[test]
    fn theme_ids_are_unique_and_stable() {
        let mut ids: Vec<&str> = THEMES.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate theme id");
        // Renaming a shipped id silently resets that user's theme.
        for expect in ["classic", "graphite", "centennial"] {
            assert!(by_id(expect).is_some(), "shipped id {expect} disappeared");
        }
    }

    /// Chord text must stay legible on the strip in EVERY theme — the one place
    /// a bad palette would make the app unusable rather than merely ugly.
    #[test]
    fn chord_text_is_legible_in_every_theme() {
        fn lum(c: Color32) -> f64 {
            let f = |v: u8| {
                let s = v as f64 / 255.0;
                if s <= 0.03928 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
            };
            0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
        }
        for t in THEMES {
            for p in [&t.light, &t.dark] {
                let (a, b) = (lum(p.chord_text), lum(p.chord_bg));
                let (hi, lo) = if a > b { (a, b) } else { (b, a) };
                let cr = (hi + 0.05) / (lo + 0.05);
                assert!(cr >= 4.5, "{}: chord text contrast {cr:.2} is too low", t.id);
            }
        }
    }
}
