use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self { Self { r, g, b } }
    pub fn to_egui(&self) -> egui::Color32 {
        egui::Color32::from_rgb(self.r, self.g, self.b)
    }
    fn from_hex(s: &str) -> Self {
        let s = s.trim_start_matches('#');
        if s.len() >= 6 {
            let r = u8::from_str_radix(&s[0..2], 16).unwrap_or(128);
            let g = u8::from_str_radix(&s[2..4], 16).unwrap_or(128);
            let b = u8::from_str_radix(&s[4..6], 16).unwrap_or(128);
            Self { r, g, b }
        } else {
            Self::rgb(128, 128, 128)
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub dark_mode: bool,
    pub white_key_idle_color: Color,
    pub black_key_idle_color: Color,
    pub white_key_active_color: Color,
    pub black_key_active_color: Color,
    pub sustain_color: Color,
    pub prefer_flats: bool,
    pub chord_detection_enabled: bool,
    pub window_size_percent: u32,
    pub borderless_mode: bool,
    pub keytoggle_enabled: bool,
    pub detached_chord_height: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_mode: false,
            white_key_idle_color: Color::from_hex("#E8DCC0"),
            black_key_idle_color: Color::from_hex("#1a1a1a"),
            white_key_active_color: Color::from_hex("#6C9BD2"),
            black_key_active_color: Color::from_hex("#6C9BD2"),
            sustain_color: Color::from_hex("#D2A36C"),
            prefer_flats: true,
            chord_detection_enabled: true,
            window_size_percent: 100,
            borderless_mode: false,
            keytoggle_enabled: false,
            detached_chord_height: 50,
        }
    }
}

impl Settings {
    pub fn config_path() -> PathBuf {
        let mut p = dirs_next::home_dir().unwrap_or_else(|| PathBuf::from("."));
        p.push(".config");
        p.push("ivory");
        p.push("settings.json");
        p
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            if let Ok(s) = std::fs::read_to_string(&path) {
                if let Ok(cfg) = serde_json::from_str::<Settings>(&s) {
                    return cfg;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(path, s);
        }
    }
}
