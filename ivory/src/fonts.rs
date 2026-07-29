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

pub const FAMILY_COURIER: &str = "courier";
pub const FAMILY_COURIER_BOLD: &str = "courier-bold";

pub fn courier() -> FontFamily {
    FontFamily::Name(FAMILY_COURIER.into())
}

pub fn courier_bold() -> FontFamily {
    FontFamily::Name(FAMILY_COURIER_BOLD.into())
}

/// Install the font families on the context. Call at startup and again if
/// `custom_font_path` changes (e.g. settings reset).
pub fn install(ctx: &egui::Context, custom_font_path: Option<&str>) {
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
