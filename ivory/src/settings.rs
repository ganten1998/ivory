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
    /// Chord label colour.
    pub chord_text_color: Rgb,
    /// Show the welcome/support note at startup. Cleared by its own checkbox.
    pub show_welcome: bool,
    /// Supporter decoration: the pixel heart on the chord view.
    pub show_heart: bool,
    /// Index into chord_strip::HEART_COLORS. Wraps, so any stored value is safe.
    pub heart_color: i64,
    /// Remembered detached-window width. Its presence is also the marker that
    /// the window has been placed under the current geometry model at least
    /// once: while it is None the stored `detached_chord_height` is ignored,
    /// because pre-2.3 builds overwrote that key with the attached strip's
    /// height on every detach, so a stored 50 is not a size anyone chose.
    pub detached_chord_width: Option<i64>,
    /// Remembered detached-window position, in monitor coordinates.
    pub detached_chord_x: Option<i64>,
    pub detached_chord_y: Option<i64>,
    /// Remembered main-window position, in monitor coordinates.
    pub window_x: Option<i64>,
    pub window_y: Option<i64>,
    /// Initial state of "Apply in all keys" in Teach Chord Name. Remembers the
    /// last choice; starts on, because naming one voicing usually means naming
    /// the shape.
    pub teach_apply_all_keys: bool,
    /// D-UI-15: the guitar view. OFF by default, and deliberately so — turning
    /// it on makes the window taller, and a window that grows on its own after
    /// an update is exactly the kind of geometry surprise the 2.2.0 tester
    /// report was about. One line here is all it takes to change that mind.
    pub show_fretboard: bool,
    /// Name from `fretboard::TUNINGS`. Stored verbatim; an unknown name falls
    /// back to Standard at the point of use rather than being rewritten, so a
    /// settings file shared with a later build keeps its tuning.
    pub fretboard_tuning: String,
    /// Capo fret. 0 is none. Clamped at use, never on load.
    pub fretboard_capo: i64,
    /// Unknown keys from the file, preserved verbatim on save (file order).
    pub extra: Map<String, Value>,
}

/// Default detached-window size when nothing has been remembered yet.
/// Deliberately NOT the piano's 8.6667:1 strip: a chord readout in its own
/// window wants to be legible, not to mirror the keyboard's proportions.
pub const DETACHED_DEFAULT: egui::Vec2 = egui::Vec2::new(460.0, 150.0);

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
            chord_text_color: Rgb { r: 0xE8, g: 0xDC, b: 0xC0 },
            show_welcome: true,
            show_heart: true,
            heart_color: 0,
            detached_chord_width: None,
            detached_chord_x: None,
            detached_chord_y: None,
            window_x: None,
            window_y: None,
            teach_apply_all_keys: true,
            show_fretboard: false,
            fretboard_tuning: "Standard".to_owned(),
            fretboard_capo: 0,
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
        if let Some(v) = map.remove("chord_text_color") {
            if let Some(c) = v.as_str().and_then(Rgb::parse) {
                s.chord_text_color = c;
            }
        }
        if let Some(v) = map.remove("show_welcome") {
            if let Some(b) = v.as_bool() {
                s.show_welcome = b;
            }
        }
        if let Some(v) = map.remove("show_heart") {
            if let Some(b) = v.as_bool() {
                s.show_heart = b;
            }
        }
        if let Some(v) = map.remove("heart_color") {
            if let Some(n) = v.as_i64() {
                s.heart_color = n;
            }
        }

        // Geometry keys are optional and stay absent until something is placed,
        // so a hand-written file without them still gets computed defaults
        // rather than a stored zero. Negative coordinates are legitimate on
        // multi-monitor setups, so no sign check here; placement is clamped to
        // the monitor at the point of use instead.
        let take_opt_i64 = |map: &mut Map<String, Value>, key: &str, dst: &mut Option<i64>| {
            if let Some(v) = map.remove(key) {
                if let Some(n) = v.as_i64() {
                    *dst = Some(n);
                }
            }
        };
        take_opt_i64(&mut map, "detached_chord_width", &mut s.detached_chord_width);
        take_opt_i64(&mut map, "detached_chord_x", &mut s.detached_chord_x);
        take_opt_i64(&mut map, "detached_chord_y", &mut s.detached_chord_y);
        take_opt_i64(&mut map, "window_x", &mut s.window_x);
        take_opt_i64(&mut map, "window_y", &mut s.window_y);
        take_bool(&mut map, "teach_apply_all_keys", &mut s.teach_apply_all_keys);
        take_bool(&mut map, "show_fretboard", &mut s.show_fretboard);
        if let Some(v) = map.remove("fretboard_tuning") {
            if let Some(t) = v.as_str() {
                s.fretboard_tuning = t.to_owned();
            }
        }
        if let Some(v) = map.remove("fretboard_capo") {
            if let Some(n) = v.as_i64() {
                s.fretboard_capo = n;
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
        map.insert("chord_text_color".into(), Value::String(self.chord_text_color.to_hex()));
        map.insert("show_welcome".into(), Value::Bool(self.show_welcome));
        map.insert("show_heart".into(), Value::Bool(self.show_heart));
        map.insert("heart_color".into(), Value::Number(self.heart_color.into()));
        let mut put_opt = |key: &str, v: Option<i64>| {
            if let Some(n) = v {
                map.insert(key.into(), Value::Number(n.into()));
            }
        };
        put_opt("detached_chord_width", self.detached_chord_width);
        put_opt("detached_chord_x", self.detached_chord_x);
        put_opt("detached_chord_y", self.detached_chord_y);
        put_opt("window_x", self.window_x);
        put_opt("window_y", self.window_y);
        map.insert(
            "teach_apply_all_keys".into(),
            Value::Bool(self.teach_apply_all_keys),
        );
        map.insert("show_fretboard".into(), Value::Bool(self.show_fretboard));
        map.insert(
            "fretboard_tuning".into(),
            Value::String(self.fretboard_tuning.clone()),
        );
        map.insert(
            "fretboard_capo".into(),
            Value::Number(self.fretboard_capo.into()),
        );
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

    /// Size to open the detached chord window at: whatever the user last left
    /// it, or `DETACHED_DEFAULT` if they have never sized it. A remembered
    /// width is what distinguishes the two, see `detached_chord_width`.
    pub fn detached_size_for_use(&self) -> egui::Vec2 {
        match self.detached_chord_width {
            Some(w) if w > 0 => egui::Vec2::new(w as f32, self.detached_height_for_use()),
            _ => DETACHED_DEFAULT,
        }
    }

    /// Remembered detached-window position, if both coordinates are stored.
    pub fn detached_pos_for_use(&self) -> Option<egui::Pos2> {
        match (self.detached_chord_x, self.detached_chord_y) {
            (Some(x), Some(y)) => Some(egui::Pos2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    /// Remembered main-window position, if both coordinates are stored.
    pub fn window_pos_for_use(&self) -> Option<egui::Pos2> {
        match (self.window_x, self.window_y) {
            (Some(x), Some(y)) => Some(egui::Pos2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    /// The board the fretboard view draws, from whatever is in the file.
    ///
    /// Every value is sanitised HERE rather than on load, so a settings file
    /// written by a later build (a tuning this one has never heard of, a capo
    /// of 40) still opens, still draws something sensible, and still keeps its
    /// own values when it goes back to the build that understands them.
    pub fn fretboard_spec(&self) -> ivory_core::fretboard::FretboardSpec {
        use ivory_core::fretboard::{FretboardSpec, Tuning};
        let tuning = Tuning::by_name(&self.fretboard_tuning).unwrap_or_else(Tuning::standard);
        let frets = FretboardSpec::default().frets;
        FretboardSpec {
            tuning,
            frets,
            // A capo at or past the last fret is a board with nothing on it.
            // The solver handles that honestly, but nobody means it, so the
            // stored value is clamped to something playable instead.
            capo: self.fretboard_capo.clamp(0, frets as i64 - 1) as u8,
        }
    }
}

/// Keep a window fully on the monitor it is being placed on. A remembered
/// position is worthless if the monitor it referred to is gone, which is the
/// normal state of affairs for anyone who ever undocks a laptop.
pub fn clamp_to_monitor(pos: egui::Pos2, size: egui::Vec2, monitor: Option<egui::Vec2>) -> egui::Pos2 {
    let Some(m) = monitor else { return pos };
    if m.x <= 0.0 || m.y <= 0.0 {
        return pos;
    }
    egui::Pos2::new(
        pos.x.clamp(0.0, (m.x - size.x).max(0.0)).round(),
        pos.y.clamp(0.0, (m.y - size.y).max(0.0)).round(),
    )
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
        // D-UI-15: the guitar view is opt-in. Turning it on makes the window
        // taller, and a window that grows on its own after an update is the
        // geometry surprise the 2.2.0 tester report was about.
        assert!(!s.show_fretboard);
        assert_eq!(s.fretboard_tuning, "Standard");
        assert_eq!(s.fretboard_capo, 0);
    }

    #[test]
    fn a_fretboard_setting_from_the_future_still_opens_and_is_kept() {
        // Sanitising at USE rather than at LOAD is what lets a settings file
        // travel between builds: an older Tangent draws Standard, writes the
        // unknown name back untouched, and the newer one still finds its
        // tuning. The same file with a nonsense capo must not produce a board
        // with nothing on it either.
        let json = r##"{
            "fretboard_tuning": "Nashville High Strung",
            "fretboard_capo": 40,
            "show_fretboard": true
        }"##;
        let map = match serde_json::from_str::<Value>(json).unwrap() {
            Value::Object(m) => m,
            _ => unreachable!(),
        };
        let s = Settings::from_map(map);
        assert!(s.show_fretboard);
        assert_eq!(s.fretboard_tuning, "Nashville High Strung");
        assert_eq!(s.fretboard_capo, 40, "stored verbatim");
        let spec = s.fretboard_spec();
        assert_eq!(spec.tuning.name, "Standard", "unknown tuning draws as standard");
        assert!(spec.capo < spec.frets, "a capo past the last fret is nobody's intent");
        // And the round trip does not eat the value it could not understand.
        let out = serde_json::to_string(&Value::Object(s.to_map())).unwrap();
        assert!(out.contains("Nashville High Strung"));
        assert!(out.contains("\"fretboard_capo\":40"));
    }

    #[test]
    fn the_shipped_tunings_all_survive_a_round_trip() {
        for t in ivory_core::fretboard::TUNINGS {
            let mut s = Settings::default();
            s.fretboard_tuning = t.name.to_owned();
            s.fretboard_capo = 5;
            let back = Settings::from_map(s.to_map());
            assert_eq!(back.fretboard_spec().tuning.name, t.name);
            assert_eq!(back.fretboard_spec().capo, 5);
        }
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

    fn map_of(json: &str) -> Map<String, Value> {
        match serde_json::from_str::<Value>(json).unwrap() {
            Value::Object(m) => m,
            _ => unreachable!(),
        }
    }

    #[test]
    fn window_geometry_keys_are_absent_until_something_is_placed() {
        // A fresh install must not gain a stored position of (0, 0), which
        // would pin the window to the top-left corner forever.
        let out = Settings::default().to_map();
        for key in [
            "window_x",
            "window_y",
            "detached_chord_x",
            "detached_chord_y",
            "detached_chord_width",
        ] {
            assert!(!out.contains_key(key), "{key} should be absent by default");
        }
        assert_eq!(out["teach_apply_all_keys"], Value::Bool(true));
    }

    #[test]
    fn window_geometry_round_trips_including_negative_coordinates() {
        // Negative coordinates are ordinary on a monitor left of the primary,
        // so they must survive rather than being treated as invalid.
        let s = Settings::from_map(map_of(
            r#"{"window_x": -1920, "window_y": -40,
                "detached_chord_x": 300, "detached_chord_y": 220,
                "detached_chord_width": 640, "detached_chord_height": 180,
                "teach_apply_all_keys": false}"#,
        ));
        assert_eq!(s.window_pos_for_use(), Some(egui::Pos2::new(-1920.0, -40.0)));
        assert_eq!(s.detached_pos_for_use(), Some(egui::Pos2::new(300.0, 220.0)));
        assert_eq!(s.detached_size_for_use(), egui::Vec2::new(640.0, 180.0));
        assert!(!s.teach_apply_all_keys);

        let back = Settings::from_map(s.to_map());
        assert_eq!(back.window_pos_for_use(), s.window_pos_for_use());
        assert_eq!(back.detached_size_for_use(), s.detached_size_for_use());
        assert!(!back.teach_apply_all_keys);
    }

    #[test]
    fn detached_size_ignores_a_legacy_height_with_no_remembered_width() {
        // Pre-2.3 builds rewrote detached_chord_height to the attached strip's
        // height on every detach, so a file carrying only that key describes a
        // size nobody chose. It must not produce a 50px-tall sliver.
        let s = Settings::from_map(map_of(r#"{"detached_chord_height": 50}"#));
        assert_eq!(s.detached_chord_width, None);
        assert_eq!(s.detached_size_for_use(), DETACHED_DEFAULT);
        assert_eq!(s.detached_pos_for_use(), None);
    }

    #[test]
    fn half_written_position_is_ignored_rather_than_half_applied() {
        let s = Settings::from_map(map_of(r#"{"window_x": 100}"#));
        assert_eq!(s.window_pos_for_use(), None);
    }

    #[test]
    fn clamping_keeps_a_window_reachable_and_tolerates_no_monitor() {
        let size = egui::Vec2::new(460.0, 150.0);
        let mon = Some(egui::Vec2::new(1920.0, 1080.0));
        // Off the right/bottom edge: pulled fully back on screen.
        assert_eq!(
            clamp_to_monitor(egui::Pos2::new(5000.0, 5000.0), size, mon),
            egui::Pos2::new(1460.0, 930.0)
        );
        // Negative: pulled to the origin.
        assert_eq!(
            clamp_to_monitor(egui::Pos2::new(-800.0, -600.0), size, mon),
            egui::Pos2::ZERO
        );
        // Already on screen: untouched.
        assert_eq!(
            clamp_to_monitor(egui::Pos2::new(100.0, 80.0), size, mon),
            egui::Pos2::new(100.0, 80.0)
        );
        // Unknown monitor: never clamp to a guess.
        assert_eq!(
            clamp_to_monitor(egui::Pos2::new(-800.0, -600.0), size, None),
            egui::Pos2::new(-800.0, -600.0)
        );
        // A window larger than the monitor still shows its top-left corner.
        assert_eq!(
            clamp_to_monitor(egui::Pos2::new(50.0, 50.0), egui::Vec2::new(4000.0, 4000.0), mon),
            egui::Pos2::ZERO
        );
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
