//! Tangent's user interface, shared by the desktop app and the VST3 plugin.
//!
//! Everything here draws with `egui` and nothing here knows what is hosting it.
//! See `Cargo.toml` for why the absent dependencies matter more than the
//! present ones.

pub mod chord_strip;
pub mod dialogs;
pub mod fonts;
pub mod fretboard_panel;
pub mod host;
pub mod keys;
pub mod menu;
pub mod piano;
pub mod settings;
pub mod shell;

/// The version the About box reports.
///
/// Named rather than incidental: `env!("CARGO_PKG_VERSION")` inside `dialogs`
/// now reads THIS crate's version, not the binary's. They are the same string
/// because both take `version.workspace = true`, and this constant is where
/// that coupling is written down so it cannot quietly stop being true.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
