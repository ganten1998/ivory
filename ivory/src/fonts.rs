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
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FontChoice {
    #[default]
    Courier,
    Terminess,
}

impl FontChoice {
    /// Stable key for settings.json. Unknown values fall back to the default.
    pub fn from_key(s: &str) -> Self {
        match s {
            "terminess" => FontChoice::Terminess,
            _ => FontChoice::Courier,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            FontChoice::Courier => "courier",
            FontChoice::Terminess => "terminess",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FontChoice::Courier => "Courier Prime",
            FontChoice::Terminess => "Terminess",
        }
    }

    pub const ALL: [FontChoice; 2] = [FontChoice::Courier, FontChoice::Terminess];

    /// Both faces are compiled in, so every choice is always usable. Kept as a
    /// method because the menu asks per-entry and a future font may not be.
    pub fn is_available(self) -> bool {
        true
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
    // lacking a symbol Ivory draws (°, ø, Δ) still renders it correctly.
    if choice == FontChoice::Terminess {
        defs.font_data.insert(
            "Terminess-Regular".to_owned(),
            Arc::new(FontData::from_static(TERMINESS_REGULAR)),
        );
        defs.font_data.insert(
            "Terminess-Bold".to_owned(),
            Arc::new(FontData::from_static(TERMINESS_BOLD)),
        );
        regular.insert(0, "Terminess-Regular".to_owned());
        // Real bold face, so picking Terminess does not flatten the menus and
        // About to a single weight.
        bold.insert(0, "Terminess-Bold".to_owned());
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
        style.text_styles.insert(TextStyle::Body, FontId::new(13.0, courier_bold()));
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
    use super::*;

    /// Spec §5.6 / DESIGN: the chord font must cover Δ (U+0394, maj7 glyph)
    /// and ø (U+00F8) in both embedded weights.
    #[test]
    fn embedded_fonts_cover_delta_and_oslash() {
        for (name, bytes) in [
            ("CourierPrime-Regular", COURIER_PRIME_REGULAR),
            ("CourierPrime-Bold", COURIER_PRIME_BOLD),
        ] {
            let face = ttf_parser::Face::parse(bytes, 0)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e}"));
            for ch in ['\u{0394}', '\u{00F8}'] {
                assert!(
                    face.glyph_index(ch).is_some(),
                    "{name} lacks a glyph for {ch:?}"
                );
            }
        }
    }
}
