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
use std::path::PathBuf;
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

/// A built-in UI typeface the user can pick from the menu.
///
/// Courier Prime is bundled and always available. Terminess is loaded from the
/// user's installed fonts rather than bundled: the Nerd Font faces are ~2.6 MB
/// EACH (they carry thousands of icon glyphs Ivory never draws), which would
/// roughly double the binary for a second font. If it is not installed the app
/// silently stays on Courier Prime — a missing optional font is never an error.
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

    /// Is this choice actually usable on this machine right now?
    pub fn is_available(self) -> bool {
        match self {
            FontChoice::Courier => true,
            FontChoice::Terminess => terminess_faces().is_some(),
        }
    }
}

/// Locate installed Terminess regular+bold, in the usual per-platform font
/// directories. Returns `None` unless BOTH faces are found, so picking it can
/// never silently cost the bold weight the menus and About rely on.
fn terminess_faces() -> Option<(PathBuf, PathBuf)> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = dirs_home() {
        dirs.push(home.join("Library").join("Fonts")); // macOS
        dirs.push(home.join(".local").join("share").join("fonts")); // Linux
        dirs.push(home.join(".fonts")); // Linux (legacy)
    }
    dirs.push(PathBuf::from("/Library/Fonts"));
    dirs.push(PathBuf::from("/usr/share/fonts"));
    dirs.push(PathBuf::from("/usr/local/share/fonts"));
    dirs.push(PathBuf::from("C:\\Windows\\Fonts"));

    // Mono first: it is the fixed-advance variant, which is what a piano app
    // with a chord strip wants. "Propo"/proportional variants are not offered.
    let stems = ["TerminessNerdFontMono", "TerminessNerdFont", "Terminess"];
    for dir in &dirs {
        for stem in &stems {
            let reg = dir.join(format!("{stem}-Regular.ttf"));
            let bold = dir.join(format!("{stem}-Bold.ttf"));
            if reg.is_file() && bold.is_file() {
                return Some((reg, bold));
            }
        }
    }
    None
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
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
        if let Some((reg_path, bold_path)) = terminess_faces() {
            if let (Ok(r), Ok(b)) = (std::fs::read(&reg_path), std::fs::read(&bold_path)) {
                defs.font_data.insert(
                    "Terminess-Regular".to_owned(),
                    Arc::new(FontData::from_owned(r)),
                );
                defs.font_data
                    .insert("Terminess-Bold".to_owned(), Arc::new(FontData::from_owned(b)));
                regular.insert(0, "Terminess-Regular".to_owned());
                // Real bold face, so picking Terminess does not flatten the
                // menus and About to a single weight.
                bold.insert(0, "Terminess-Bold".to_owned());
            }
        }
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
