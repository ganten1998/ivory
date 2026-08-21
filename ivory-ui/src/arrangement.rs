//! The arrangement: the DOCUMENT half of the app, in its own file.
//!
//! # Why this is not in `settings.json`, and the decision is deliberate
//!
//! Settings are PREFERENCES: fixed-size arrays keyed by the desk's shape,
//! migrated by `settings_version`, one set per user for ever. An arrangement
//! is a DOCUMENT: an unbounded list of clips whose positions change with the
//! work, one per song, and eventually a thing you hand to somebody. Folding
//! the second into the first would mean exactly one arrangement for ever
//! (starting a second song destroys the first), every schema change running a
//! migration across everyone's colours and window geometry, and the
//! `LEGACY_MAX_STRIPS` caps losing their meaning the moment an unbounded
//! document shares the file. The owner weighed this and chose the split.
//!
//! Deliberately NOT a "project" feature: no New/Open/Save-As, no dirty flag,
//! no recent-files list. One file at a well-known path, auto-loaded and
//! auto-saved, exactly like the settings — and because it is a PATH, multiple
//! songs become possible later without any of this being rework.
//!
//! # The schema, and what is deliberately absent
//!
//! Version 1 is the smallest honest document: clips on lanes, each a file
//! path and a start in seconds. Seconds, because seconds survive a device
//! that changes rate — the engine converts at its own rate, the same rule the
//! transport follows. Unknown keys ride in `extra` and survive a round trip,
//! so a newer build's arrangement opened by this one is not stripped.
//!
//! No tempo map, no clip trims, no per-clip gain, no MIDI clips YET — each of
//! those is a real subsystem, and a schema that names them before they exist
//! is a promise this build cannot keep. `version` is what lets them arrive.

use serde_json::{Map, Value};
use std::path::PathBuf;

/// The document's schema version. Bump WITH a migration arm in
/// [`Arrangement::from_map`], never without one.
pub const ARRANGEMENT_VERSION: u64 = 1;

/// One piece of audio standing on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrClip {
    /// Which lane it stands on. Lane 0 is the backing track's.
    pub lane: usize,
    /// The file it came from, absolute.
    pub path: String,
    /// Where it begins on the timeline, in seconds from 0:00.
    pub start_s: f64,
}

/// The whole document.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arrangement {
    pub clips: Vec<ArrClip>,
    /// Keys this build does not know, kept so a newer build's document
    /// survives being opened here — the settings file's own bargain.
    pub extra: Map<String, Value>,
}

impl Arrangement {
    /// Where the document lives. Overridable for tests in EVERY build, for
    /// the same reason `Settings::path` is: a binary-crate test links an
    /// ordinary `ivory-ui`, and one that ran a frame against the real home
    /// directory would edit the user's own song.
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("IVORY_ARRANGEMENT_PATH") {
            return PathBuf::from(p);
        }
        Settings_dir().join("arrangement.json")
    }

    pub fn load() -> Self {
        match std::fs::read_to_string(Self::path()) {
            Ok(text) => Self::from_json(&text),
            Err(_) => Self::default(),
        }
    }

    /// Write-then-rename, the settings file's own shape: a crash or a full
    /// disk mid-write must not leave half a song where a whole one was.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, self.to_json()).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    pub fn from_json(text: &str) -> Self {
        let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        Self::from_map(map)
    }

    fn from_map(mut map: Map<String, Value>) -> Self {
        // Read and discarded for now — version 1 is the first version, so
        // there is nothing to migrate FROM. The arm structure below is the
        // settings file's: consume what is known, keep the rest.
        let _version = map
            .shift_remove("arrangement_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut clips = Vec::new();
        if let Some(Value::Array(list)) = map.shift_remove("clips") {
            for item in list {
                let Value::Object(c) = item else { continue };
                let lane = c.get("lane").and_then(Value::as_u64).unwrap_or(0) as usize;
                let Some(path) = c.get("path").and_then(Value::as_str) else {
                    continue;
                };
                let start_s = c
                    .get("start_s")
                    .and_then(Value::as_f64)
                    .filter(|s| s.is_finite() && *s >= 0.0)
                    .unwrap_or(0.0);
                clips.push(ArrClip {
                    lane,
                    path: path.to_owned(),
                    start_s,
                });
            }
        }
        Self { clips, extra: map }
    }

    pub fn to_json(&self) -> String {
        let mut map = Map::new();
        map.insert(
            "arrangement_version".into(),
            Value::Number(ARRANGEMENT_VERSION.into()),
        );
        map.insert(
            "clips".into(),
            Value::Array(
                self.clips
                    .iter()
                    .map(|c| {
                        let mut m = Map::new();
                        m.insert("lane".into(), Value::Number((c.lane as u64).into()));
                        m.insert("path".into(), Value::String(c.path.clone()));
                        if let Some(n) = serde_json::Number::from_f64(c.start_s) {
                            m.insert("start_s".into(), Value::Number(n));
                        }
                        Value::Object(m)
                    })
                    .collect(),
            ),
        );
        // Unknown keys LAST, like the settings file: a newer build's keys win
        // over nothing, and they survive.
        for (k, v) in &self.extra {
            map.entry(k.clone()).or_insert_with(|| v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(map)).unwrap_or_else(|_| "{}".into())
    }
}

/// The settings directory, shared with `Settings::path` by construction.
#[allow(non_snake_case)]
fn Settings_dir() -> PathBuf {
    crate::settings::Settings::path()
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document survives its own round trip, clips, order and all.
    #[test]
    fn an_arrangement_round_trips() {
        let a = Arrangement {
            clips: vec![
                ArrClip { lane: 0, path: "/x/backing.mp3".into(), start_s: 0.0 },
                ArrClip { lane: 3, path: "/x/solo.wav".into(), start_s: 84.5 },
            ],
            extra: Map::new(),
        };
        let back = Arrangement::from_json(&a.to_json());
        assert_eq!(back, a, "the document did not survive its own round trip");
    }

    /// Keys this build does not know come back out unchanged — a newer
    /// build's song opened here is not stripped.
    #[test]
    fn a_newer_builds_keys_survive() {
        let text = r#"{"arrangement_version": 9, "clips": [],
                       "tempo_map": [{"at": 0, "bpm": 121}]}"#;
        let a = Arrangement::from_json(text);
        let out = a.to_json();
        assert!(
            out.contains("tempo_map") && out.contains("121"),
            "a newer build's tempo map was stripped: {out}"
        );
    }

    /// Garbage is an empty document, never a panic and never a half-read.
    #[test]
    fn garbage_is_an_empty_arrangement() {
        for text in ["", "not json", "[1,2,3]", r#"{"clips": "no"}"#] {
            let a = Arrangement::from_json(text);
            assert!(a.clips.is_empty(), "{text:?} produced clips");
        }
        // A clip missing its path is skipped; a negative start is zeroed.
        let a = Arrangement::from_json(
            r#"{"clips": [{"lane": 1}, {"path": "/x.wav", "start_s": -4.0}]}"#,
        );
        assert_eq!(a.clips.len(), 1);
        assert_eq!(a.clips[0].start_s, 0.0);
    }
}
