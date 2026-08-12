//! Settings persistence — Python-compatible `~/.config/ivory/settings.json`.
//!
//! Parity rules (spec §8):
//! - Literal `Path.home()/.config/ivory/settings.json` on ALL platforms
//!   (do NOT "improve" to dirs::config_dir(); Python hard-codes this).
//! - 13 keys, lowercase `#rrggbb` colors, JSON indent=2.
//! - Any read/parse error => all defaults. Per-key type mismatch => that key's default.
//! - Unknown keys are preserved across load/save (D-UI-5: additive `custom_font_path`).
//! - Write errors silently ignored.

use serde_json::{Map, Value};
use std::path::PathBuf;

/// A solid RGB color stored as in the Python settings file (`#rrggbb`, lowercase).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    /// Parse `#rrggbb` (case-insensitive) or `#rgb`. Returns None on anything else.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let hex = s.strip_prefix('#')?;
        match hex.len() {
            6 => {
                let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
                let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
                let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
                Some(Self { r, g, b })
            }
            3 => {
                let d = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok();
                let (r, g, b) = (d(0)?, d(1)?, d(2)?);
                Some(Self {
                    r: r * 17,
                    g: g * 17,
                    b: b * 17,
                })
            }
            _ => None,
        }
    }

    /// Lowercase `#rrggbb`, exactly like `QColor.name()`.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }

    pub fn to_color32(self) -> egui::Color32 {
        egui::Color32::from_rgb(self.r, self.g, self.b)
    }

    pub fn from_color32(c: egui::Color32) -> Self {
        Self::new(c.r(), c.g(), c.b())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub dark_mode: bool,
    pub white_key_idle_color: Rgb,
    pub black_key_idle_color: Rgb,
    pub white_key_active_color: Rgb,
    pub black_key_active_color: Rgb,
    pub sustain_color: Rgb,
    pub prefer_flats: bool,
    pub chord_detection_enabled: bool,
    pub window_size_percent: i64,
    pub borderless_mode: bool,
    pub chord_window_detached: bool,
    pub detached_chord_height: i64,
    pub keytoggle_enabled: bool,
    /// Additive key (D-UI-5): optional path to a user font loaded at top
    /// priority in both font families. Absent from the file when None.
    pub custom_font_path: Option<String>,
    /// Built-in UI typeface key (see fonts::FontChoice). Additive key; unknown
    /// values fall back to Courier Prime.
    pub font_choice: String,
    /// Supporter extra: halo under held keys. Additive key; ignored without a
    /// license, so a config moved to an unlicensed machine simply draws no glow.
    pub glow_enabled: bool,
    /// Supporter extra: bloom the chord label.
    pub chord_glow: bool,
    /// Chord label colour. Defaults to a segmented-display green.
    pub chord_text_color: Rgb,
    /// Unknown keys from the file, preserved verbatim on save (file order).
    pub extra: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_mode: false,
            white_key_idle_color: Rgb::new(0xE8, 0xDC, 0xC0),
            black_key_idle_color: Rgb::new(0x1a, 0x1a, 0x1a),
            white_key_active_color: Rgb::new(0x6C, 0x9B, 0xD2),
            black_key_active_color: Rgb::new(0x6C, 0x9B, 0xD2),
            sustain_color: Rgb::new(0xD2, 0xA3, 0x6C),
            prefer_flats: true,
            chord_detection_enabled: true,
            window_size_percent: 100,
            borderless_mode: false,
            chord_window_detached: false,
            detached_chord_height: 50,
            keytoggle_enabled: false,
            custom_font_path: None,
            font_choice: crate::fonts::FontChoice::default().key().to_owned(),
            glow_enabled: false,
            chord_glow: false,
            chord_text_color: Rgb { r: 0x2F, g: 0xE8, b: 0x6B },
            extra: Map::new(),
        }
    }
}

impl Settings {
    /// Literal `~/.config/ivory/settings.json` on every platform (parity).
    pub fn path() -> PathBuf {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        home.join(".config").join("ivory").join("settings.json")
    }

    pub fn load() -> Self {
        Self::load_from(&Self::path())
    }

    fn load_from(path: &std::path::Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        Self::from_map(map)
    }

    fn from_map(mut map: Map<String, Value>) -> Self {
        let mut s = Self::default();

        let take_bool = |map: &mut Map<String, Value>, key: &str, dst: &mut bool| {
            if let Some(v) = map.remove(key) {
                if let Some(b) = v.as_bool() {
                    *dst = b;
                }
            }
        };
        take_bool(&mut map, "dark_mode", &mut s.dark_mode);
        take_bool(&mut map, "prefer_flats", &mut s.prefer_flats);
        take_bool(
            &mut map,
            "chord_detection_enabled",
            &mut s.chord_detection_enabled,
        );
        take_bool(&mut map, "borderless_mode", &mut s.borderless_mode);
        take_bool(&mut map, "chord_window_detached", &mut s.chord_window_detached);
        take_bool(&mut map, "keytoggle_enabled", &mut s.keytoggle_enabled);

        let take_color = |map: &mut Map<String, Value>, key: &str, dst: &mut Rgb| {
            if let Some(v) = map.remove(key) {
                if let Some(c) = v.as_str().and_then(Rgb::parse) {
                    *dst = c;
                }
            }
        };
        take_color(&mut map, "white_key_idle_color", &mut s.white_key_idle_color);
        take_color(&mut map, "black_key_idle_color", &mut s.black_key_idle_color);
        take_color(
            &mut map,
            "white_key_active_color",
            &mut s.white_key_active_color,
        );
        take_color(
            &mut map,
            "black_key_active_color",
            &mut s.black_key_active_color,
        );
        take_color(&mut map, "sustain_color", &mut s.sustain_color);

        if let Some(v) = map.remove("window_size_percent") {
            if let Some(n) = v.as_i64() {
                if n > 0 {
                    s.window_size_percent = n;
                }
            }
        }
        if let Some(v) = map.remove("detached_chord_height") {
            if let Some(n) = v.as_i64() {
                // D-UI-1: honored (unlike Python, which overwrote it with 50 on
                // init); values <= 0 fall back to 50 at the point of use.
                s.detached_chord_height = n;
            }
        }
        if let Some(v) = map.remove("custom_font_path") {
            if let Some(p) = v.as_str() {
                s.custom_font_path = Some(p.to_owned());
            }
        }
        if let Some(v) = map.remove("font_choice") {
            if let Some(f) = v.as_str() {
                // Stored verbatim; an unknown key resolves to Courier at use.
                s.font_choice = f.to_owned();
            }
        }
        if let Some(v) = map.remove("glow_enabled") {
            if let Some(b) = v.as_bool() {
                s.glow_enabled = b;
            }
        }
        if let Some(v) = map.remove("chord_glow") {
            if let Some(b) = v.as_bool() {
                s.chord_glow = b;
            }
        }
        if let Some(v) = map.remove("chord_text_color") {
            if let Some(c) = v.as_str().and_then(Rgb::parse) {
                s.chord_text_color = c;
            }
        }

        s.extra = map; // whatever is left, preserved in file order
        s
    }

    fn to_map(&self) -> Map<String, Value> {
        // Python writes its dict in insertion order; replicate that key order,
        // then the additive key, then any preserved unknown keys.
        let mut map = Map::new();
        map.insert("dark_mode".into(), Value::Bool(self.dark_mode));
        map.insert(
            "white_key_idle_color".into(),
            Value::String(self.white_key_idle_color.to_hex()),
        );
        map.insert(
            "black_key_idle_color".into(),
            Value::String(self.black_key_idle_color.to_hex()),
        );
        map.insert(
            "white_key_active_color".into(),
            Value::String(self.white_key_active_color.to_hex()),
        );
        map.insert(
            "black_key_active_color".into(),
            Value::String(self.black_key_active_color.to_hex()),
        );
        map.insert(
            "sustain_color".into(),
            Value::String(self.sustain_color.to_hex()),
        );
        map.insert("prefer_flats".into(), Value::Bool(self.prefer_flats));
        map.insert(
            "chord_detection_enabled".into(),
            Value::Bool(self.chord_detection_enabled),
        );
        map.insert(
            "window_size_percent".into(),
            Value::Number(self.window_size_percent.into()),
        );
        map.insert("borderless_mode".into(), Value::Bool(self.borderless_mode));
        map.insert(
            "chord_window_detached".into(),
            Value::Bool(self.chord_window_detached),
        );
        map.insert(
            "detached_chord_height".into(),
            Value::Number(self.detached_chord_height.into()),
        );
        map.insert("keytoggle_enabled".into(), Value::Bool(self.keytoggle_enabled));
        if let Some(ref p) = self.custom_font_path {
            map.insert("custom_font_path".into(), Value::String(p.clone()));
        }
        map.insert("font_choice".into(), Value::String(self.font_choice.clone()));
        map.insert("glow_enabled".into(), Value::Bool(self.glow_enabled));
        map.insert("chord_glow".into(), Value::Bool(self.chord_glow));
        map.insert("chord_text_color".into(), Value::String(self.chord_text_color.to_hex()));
        for (k, v) in &self.extra {
            map.insert(k.clone(), v.clone());
        }
        map
    }

    /// Saved after every mutation. Write errors are silently ignored (parity).
    pub fn save(&self) {
        self.save_to(&Self::path());
    }

    fn save_to(&self, path: &std::path::Path) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // serde_json pretty-printing indents with 2 spaces, like Python's indent=2.
        if let Ok(text) = serde_json::to_string_pretty(&Value::Object(self.to_map())) {
            let _ = std::fs::write(path, text);
        }
    }

    /// "Reset Settings to Default" (D-UI-8): resets the 13 parity keys and
    /// `custom_font_path`, keeps unknown keys.
    pub fn reset_to_defaults(&mut self) {
        let extra = std::mem::take(&mut self.extra);
        *self = Self::default();
        self.extra = extra;
    }

    /// Height for the detached chord window (D-UI-1: <= 0 falls back to 50).
    pub fn detached_height_for_use(&self) -> f32 {
        if self.detached_chord_height > 0 {
            self.detached_chord_height as f32
        } else {
            50.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec_table() {
        let s = Settings::default();
        assert!(!s.dark_mode);
        assert_eq!(s.white_key_idle_color.to_hex(), "#e8dcc0");
        assert_eq!(s.black_key_idle_color.to_hex(), "#1a1a1a");
        assert_eq!(s.white_key_active_color.to_hex(), "#6c9bd2");
        assert_eq!(s.black_key_active_color.to_hex(), "#6c9bd2");
        assert_eq!(s.sustain_color.to_hex(), "#d2a36c");
        assert!(s.prefer_flats);
        assert!(s.chord_detection_enabled);
        assert_eq!(s.window_size_percent, 100);
        assert!(!s.borderless_mode);
        assert!(!s.chord_window_detached);
        assert_eq!(s.detached_chord_height, 50);
        assert!(!s.keytoggle_enabled);
        assert!(s.custom_font_path.is_none());
    }

    #[test]
    fn unknown_keys_preserved_and_key_order_stable() {
        let json = r##"{
            "future_key": {"nested": [1, 2]},
            "dark_mode": true,
            "white_key_idle_color": "#ABCDEF"
        }"##;
        let map = match serde_json::from_str::<Value>(json).unwrap() {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        let s = Settings::from_map(map);
        assert!(s.dark_mode);
        // case-insensitive parse, lowercase serialization
        assert_eq!(s.white_key_idle_color.to_hex(), "#abcdef");
        assert!(s.extra.contains_key("future_key"));

        let out = s.to_map();
        let keys: Vec<&str> = out.keys().map(|k| k.as_str()).collect();
        assert_eq!(keys[0], "dark_mode");
        assert_eq!(keys[12], "keytoggle_enabled");
        assert_eq!(*keys.last().unwrap(), "future_key");
        // custom_font_path is absent when None
        assert!(!out.contains_key("custom_font_path"));
    }

    #[test]
    fn garbage_file_yields_defaults() {
        let dir = std::env::temp_dir().join("ivory-settings-test");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&path), Settings::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn wrong_typed_keys_fall_back_per_key() {
        let json = r#"{"dark_mode": "yes", "window_size_percent": 150, "detached_chord_height": -3}"#;
        let map = match serde_json::from_str::<Value>(json).unwrap() {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        let s = Settings::from_map(map);
        assert!(!s.dark_mode); // wrong type => default
        assert_eq!(s.window_size_percent, 150);
        assert_eq!(s.detached_chord_height, -3);
        assert_eq!(s.detached_height_for_use(), 50.0); // D-UI-1 fallback
    }
}
