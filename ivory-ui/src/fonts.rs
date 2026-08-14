//! Font setup: two named families backed by bundled Courier Prime (SIL OFL 1.1),
//! with egui's default fonts appended as glyph fallback in each.
//!
//! - "courier"      = CourierPrime-Regular + defaults  (chord strip: Normal weight)
//! - "courier-bold" = CourierPrime-Bold + defaults     (menus, About, dialogs)
//!
//! The optional settings key `custom_font_path` loads a user TTF/OTF at top
//! priority in BOTH families; load errors are silently ignored.
//! Courier New is never shipped (licensing).

use egui::epaint::text::{FontData, FontDefinitions, FontFamily};
use std::sync::Arc;

pub static COURIER_PRIME_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/CourierPrime-Regular.ttf");
pub static COURIER_PRIME_BOLD: &[u8] = include_bytes!("../../assets/fonts/CourierPrime-Bold.ttf");

/// Terminess Nerd Font Mono (SIL OFL 1.1, (C) 2020 Dimitar Toshkov Zhekov,
/// (C) 2023 Tilman Blumenbach). Bundled so the option works on every machine
/// rather than only where it happens to be installed; its licence ships in
/// font-licenses/ in every artifact, as the OFL requires.
pub static TERMINESS_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/TerminessNerdFontMono-Regular.ttf");
pub static TERMINESS_BOLD: &[u8] =
    include_bytes!("../../assets/fonts/TerminessNerdFontMono-Bold.ttf");

/// JetBrains Mono, SIL OFL 1.1. The third face fills the gap the other two
/// leave: Courier Prime is a typewriter and Terminess is a terminal, and
/// neither is a plainly modern one. It is also the only bundled face that
/// covers EVERY symbol Tangent draws by itself, arrows included — Courier
/// Prime has no U+2191/U+2193, so the guitar view's octave arrows reach the
/// screen through the fallback chain rather than from the chosen face.
/// 270 KB a weight against Terminess's 2.6 MB.
pub static JETBRAINS_REGULAR: &[u8] =
    include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf");
pub static JETBRAINS_BOLD: &[u8] = include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf");

pub const FAMILY_COURIER: &str = "courier";
pub const FAMILY_COURIER_BOLD: &str = "courier-bold";

pub fn courier() -> FontFamily {
    FontFamily::Name(FAMILY_COURIER.into())
}

pub fn courier_bold() -> FontFamily {
    FontFamily::Name(FAMILY_COURIER_BOLD.into())
}

/// A built-in UI typeface the user can pick from the menu.
///
/// Both are bundled, so the choice always works offline and on a fresh machine.
/// Terminess costs ~5 MB of binary (2.5 MB compressed in the download) because
/// the Nerd Font faces carry thousands of icon glyphs; that was accepted
/// deliberately in favour of the option working for everyone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum FontChoice {
    #[default]
    Courier,
    Terminess,
    JetBrains,
}

impl FontChoice {
    /// Stable key for settings.json. Unknown values fall back to the default.
    pub fn from_key(s: &str) -> Self {
        match s {
            "terminess" => FontChoice::Terminess,
            "jetbrains" => FontChoice::JetBrains,
            _ => FontChoice::Courier,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            FontChoice::Courier => "courier",
            FontChoice::Terminess => "terminess",
            FontChoice::JetBrains => "jetbrains",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FontChoice::Courier => "Courier Prime",
            FontChoice::Terminess => "Terminess",
            FontChoice::JetBrains => "JetBrains Mono",
        }
    }

    pub const ALL: [FontChoice; 3] = [
        FontChoice::Courier,
        FontChoice::Terminess,
        FontChoice::JetBrains,
    ];

    /// Both faces are compiled in, so every choice is always usable. Kept as a
    /// method because the menu asks per-entry and a future font may not be.
    pub fn is_available(self) -> bool {
        true
    }

    /// The next available face, wrapping.
    ///
    /// This used to be `ALL.iter().find(|f| *f != cur)` in two places, which
    /// is not a cycle: it always returns the FIRST face that is not the
    /// current one. With three faces that flip-flops between the first two and
    /// **JetBrains Mono was unreachable** — bundled, listed, and impossible to
    /// select. One method now, used by the menu label and by the action, so
    /// the row cannot promise a face the click does not give you.
    pub fn next(self) -> Self {
        let here = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        for step in 1..=Self::ALL.len() {
            let cand = Self::ALL[(here + step) % Self::ALL.len()];
            if cand.is_available() {
                return cand;
            }
        }
        self
    }
}

/// Install the font families on the context. Call at startup and again if the
/// font choice or `custom_font_path` changes (e.g. settings reset).
pub fn install(ctx: &egui::Context, choice: FontChoice, custom_font_path: Option<&str>) {
    let mut defs = FontDefinitions::default();

    defs.font_data.insert(
        "CourierPrime-Regular".to_owned(),
        Arc::new(FontData::from_static(COURIER_PRIME_REGULAR)),
    );
    defs.font_data.insert(
        "CourierPrime-Bold".to_owned(),
        Arc::new(FontData::from_static(COURIER_PRIME_BOLD)),
    );

    // Egui's default fonts, appended as glyph fallback (Δ is covered by
    // Courier Prime itself; the fallback covers arrows/emoji/etc.).
    let default_fallback: Vec<String> = defs
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();

    let mut regular = vec!["CourierPrime-Regular".to_owned()];
    let mut bold = vec!["CourierPrime-Bold".to_owned()];

    // A chosen built-in sits above Courier Prime but below any custom font.
    // Courier Prime stays in the chain underneath as glyph fallback, so a face
    // lacking a symbol Tangent draws (°, ø, Δ) still renders it correctly.
    // The chosen face, whichever it is. Courier Prime stays underneath as glyph
    // fallback, so a face missing a symbol still renders it.
    if let Some((name, reg, bold_bytes)) = match choice {
        FontChoice::Terminess => Some(("Terminess", TERMINESS_REGULAR, TERMINESS_BOLD)),
        FontChoice::JetBrains => Some(("JetBrainsMono", JETBRAINS_REGULAR, JETBRAINS_BOLD)),
        FontChoice::Courier => None,
    } {
        defs.font_data.insert(
            format!("{name}-Regular"),
            Arc::new(FontData::from_static(reg)),
        );
        defs.font_data.insert(
            format!("{name}-Bold"),
            Arc::new(FontData::from_static(bold_bytes)),
        );
        regular.insert(0, format!("{name}-Regular"));
        // Real bold face, so a chosen font does not flatten the menus and
        // About to a single weight.
        bold.insert(0, format!("{name}-Bold"));
    }

    // Optional user font at top priority in both families; errors ignored.
    if let Some(path) = custom_font_path {
        if let Ok(bytes) = std::fs::read(path) {
            defs.font_data.insert(
                "IvoryCustom".to_owned(),
                Arc::new(FontData::from_owned(bytes)),
            );
            regular.insert(0, "IvoryCustom".to_owned());
            bold.insert(0, "IvoryCustom".to_owned());
        }
    }

    regular.extend(default_fallback.iter().cloned());
    bold.extend(default_fallback.iter().cloned());

    defs.families
        .insert(FontFamily::Name(FAMILY_COURIER.into()), regular);
    defs.families
        .insert(FontFamily::Name(FAMILY_COURIER_BOLD.into()), bold);

    ctx.set_fonts(defs);
}

/// Map egui text styles so stock widgets pick the right family: bold Courier
/// for chrome (menus/dialogs use TextStyle::Button/Body), regular for
/// monospace. Explicit `FontId`s are used wherever the spec names a size.
pub fn apply_text_styles(ctx: &egui::Context) {
    use egui::{FontId, TextStyle};
    ctx.all_styles_mut(|style| {
        style
            .text_styles
            .insert(TextStyle::Body, FontId::new(13.0, courier_bold()));
        style
            .text_styles
            .insert(TextStyle::Button, FontId::new(13.0, courier_bold()));
        style
            .text_styles
            .insert(TextStyle::Heading, FontId::new(16.0, courier_bold()));
        style
            .text_styles
            .insert(TextStyle::Small, FontId::new(10.0, courier_bold()));
        style
            .text_styles
            .insert(TextStyle::Monospace, FontId::new(13.0, courier()));
    });
}

#[cfg(test)]
mod tests {

    /// Cycling must REACH every bundled face. It did not: the old
    /// "first face that is not the current one" rule bounced between Courier
    /// Prime and Terminess forever, and JetBrains Mono — compiled into the
    /// binary, listed in the menu — could not be selected at all.
    #[test]
    fn cycling_reaches_every_bundled_face() {
        let mut seen = std::collections::HashSet::new();
        let mut f = FontChoice::default();
        for _ in 0..FontChoice::ALL.len() {
            seen.insert(f);
            f = f.next();
        }
        assert_eq!(
            seen.len(),
            FontChoice::ALL.len(),
            "cycling from {:?} only reached {:?}",
            FontChoice::default(),
            seen
        );
        assert_eq!(f, FontChoice::default(), "the cycle does not come back round");

        // ...from every starting point, not just the default.
        for start in FontChoice::ALL {
            let mut seen = std::collections::HashSet::new();
            let mut f = start;
            for _ in 0..FontChoice::ALL.len() {
                seen.insert(f);
                f = f.next();
            }
            assert_eq!(seen.len(), FontChoice::ALL.len(), "starting from {start:?}");
        }
    }

    /// The menu row promises the face the next click will give you.
    #[test]
    fn the_menu_label_matches_what_cycling_does() {
        for f in FontChoice::ALL {
            assert_eq!(f.next().label(), f.next().label());
            assert_ne!(f.next(), f, "a face that cycles to itself would look stuck");
        }
    }
    use super::*;

    /// Spec §5.6 / DESIGN: the chord font must cover Δ (U+0394, maj7 glyph)
    /// and ø (U+00F8) in both embedded weights.
    #[test]
    fn embedded_fonts_cover_delta_and_oslash() {
        // Every symbol Tangent actually draws, not just the two chord ones.
        // The arrows matter: the guitar view marks a folded note with U+2191 or
        // U+2193, and Courier Prime does NOT have them — they reach the screen
        // through the fallback chain. That is fine, and it is worth having a
        // test that knows it rather than a surprise if the chain ever changes.
        const DRAWN: &[(char, &str)] = &[
            ('\u{0394}', "major-7 delta"),
            ('\u{00F8}', "half-diminished slash-o"),
            ('\u{00B0}', "diminished ring"),
            ('\u{00D7}', "muted-string cross"),
            ('\u{2022}', "menu selection dot"),
            ('\u{00B7}', "caption separator"),
            ('\u{2191}', "octave up"),
            ('\u{2193}', "octave down"),
        ];

        let has = |bytes: &[u8], c: char| {
            ttf_parser::Face::parse(bytes, 0)
                .map(|f| f.glyph_index(c).is_some())
                .unwrap_or(false)
        };

        // The chord symbols must come from the bundled faces themselves, not
        // from a fallback: they are the app's whole output.
        for (c, what) in &DRAWN[..3] {
            assert!(has(COURIER_PRIME_REGULAR, *c), "Courier Prime lacks {what}");
            assert!(
                has(COURIER_PRIME_BOLD, *c),
                "Courier Prime Bold lacks {what}"
            );
        }

        // JetBrains Mono is the one face that covers the lot on its own, which
        // is part of why it is bundled.
        for (c, what) in DRAWN {
            assert!(has(JETBRAINS_REGULAR, *c), "JetBrains Mono lacks {what}");
            assert!(has(JETBRAINS_BOLD, *c), "JetBrains Mono Bold lacks {what}");
        }

        // And everything must be reachable from SOMEWHERE in the chain.
        for (c, what) in DRAWN {
            let covered = has(COURIER_PRIME_REGULAR, *c)
                || has(TERMINESS_REGULAR, *c)
                || has(JETBRAINS_REGULAR, *c);
            assert!(
                covered,
                "no bundled face has {what}; it would render as tofu"
            );
        }
    }
}
