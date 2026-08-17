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

/// What the settings file's shape is, so that a default this app changes can
/// reach somebody who already has the app.
///
/// **The problem this solves is not versioning, it is that a default is a
/// one-shot.** The file is written whole: every key the app knows goes into it
/// on every save, so the first save any user ever makes pins the day's default
/// for every key they never touched, and no later default can ever reach them.
/// Three changes to this app shipped into that hole — two corrections to the
/// recorder's brown and the change that made the app rather than the camera the
/// subject of a recording. The owner asked for the last one four times, was
/// told four times that it was done, and it had never once been true for them.
///
/// A version stamp fixes it properly where a list of superseded values did not:
/// the migration runs ONCE against a file written before the change, and after
/// that the same value chosen deliberately is never touched again. A file with
/// no stamp is version 0 — every file every previous build wrote.
const SETTINGS_VERSION: u64 = 1;

/// Recorder backgrounds this app shipped as defaults before [`SETTINGS_VERSION`]
/// 1, and which are therefore not evidence that anybody chose them.
const V0_RECORDER_BG: [&str; 2] = ["#4a3b2c", "#33271b"];

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
    /// The Recorder band's background.
    ///
    /// A brown by default, and deliberately not the piano's grey: the band is a
    /// different KIND of surface — a set of controls rather than an instrument
    /// — and telling them apart at a glance while playing is worth a colour.
    /// Warm rather than neutral because everything else in this app already is,
    /// from the ivory ink to the rosewood fretboard.
    pub recorder_bg_color: Rgb,
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
    /// A user-built tuning, as `name` and comma-separated open MIDI pitches.
    ///
    /// Two keys rather than one, and BOTH absent unless a custom tuning has
    /// actually been made. `fretboard_tuning` selects it by holding
    /// [`CUSTOM_TUNING`]; the pitches live here so that switching to a preset
    /// and back does not lose the tuning the user spent time entering.
    ///
    /// Stored as text rather than an array because the whole settings file is
    /// hand-serialised scalars (see `load_from`/`save_to`), and a nested array
    /// would be the only one — a shape a future reader of that code would have
    /// to special-case for no gain.
    pub fretboard_custom_name: Option<String>,
    pub fretboard_custom_open: Option<String>,
    /// Capo fret. 0 is none. Clamped at use, never on load.
    pub fretboard_capo: i64,
    /// Fingerboard wood: "rosewood" (default), "maple", "ebony". Stored
    /// verbatim; an unknown value falls back at use, like `font_choice`.
    pub fretboard_wood: String,
    /// What the capo is made of: "wood" (default), "black", "silver". Stored
    /// verbatim; an unknown value falls back at use, like `fretboard_wood`.
    /// Cycled by clicking the capo itself.
    pub capo_style: String,
    /// D-UI-16: the guitar view is in its own window.
    pub fretboard_detached: bool,
    /// The theory band is popped into its own window.
    ///
    /// The third detachable surface, after the chord strip and the fretboard,
    /// and it follows their rules exactly — including that `Caps::detachable`
    /// forces it off at construction, so a plugin seeded from a desktop
    /// settings file cannot start with a band it has no window to put.
    pub theory_detached: bool,
    /// Remembered popout geometry. Absent until the window has been placed,
    /// exactly like the detached chord window's (D-UI-11).
    pub fretboard_win_w: Option<i64>,
    pub fretboard_win_h: Option<i64>,
    pub fretboard_win_x: Option<i64>,
    pub fretboard_win_y: Option<i64>,
    /// The theory window's remembered geometry. Same shape and same rule as the
    /// fretboard's: absent until the window has actually been placed, so a
    /// tiling WM's idea of the size is never mistaken for the user's (D-UI-11).
    pub theory_win_w: Option<i64>,
    pub theory_win_h: Option<i64>,
    pub theory_win_x: Option<i64>,
    pub theory_win_y: Option<i64>,
    /// D-UI-17: which theory diagrams are showing. Three independent flags,
    /// because any combination is allowed and an enum would need a variant per
    /// combination to say so. All off by default, and deliberately so: the
    /// band is 300 points tall at full width, and a window that grows on its
    /// own after an update is the geometry surprise this app has been bitten
    /// by before (see `show_fretboard`).
    pub theory_circle: bool,
    pub theory_tonnetz: bool,
    pub theory_triangles: bool,
    /// Whether the theory band follows what you are PLAYING.
    ///
    /// Off by default, and that is the point of it. The band is something you
    /// look at while you play — a key you are working in, a chord you are
    /// trying to place — and a diagram that redraws on every note you touch is
    /// one you cannot read while touching notes. With this off it shows only
    /// what you put there, by clicking the piano, the neck or the diagrams
    /// themselves, and stays put until you change it.
    pub theory_follow_midi: bool,
    /// The Recorder band is showing.
    ///
    /// Off by default and deliberately NOT part of `first_launch()`, for the
    /// same reason `show_fretboard` is off for existing users: the band is 200
    /// points tall, and a window that grows on its own is the geometry surprise
    /// this app has already been bitten by twice. Someone who wants to record
    /// turns it on once; someone who wants a chord display never sees it.
    pub show_recorder: bool,
    /// The Recorder band is in its own window — the fourth detachable surface,
    /// and the one with the best reason to be on a second monitor: a big
    /// framing view of the camera while the piano stays where it was.
    pub recorder_detached: bool,
    pub recorder_win_w: Option<i64>,
    pub recorder_win_h: Option<i64>,
    pub recorder_win_x: Option<i64>,
    pub recorder_win_y: Option<i64>,
    /// Where takes are written. Absent means the platform's videos folder with
    /// a `Tangent` subfolder — resolved at the point of use by [`record_root`],
    /// never stored as a resolved string, so moving a home directory does not
    /// strand it.
    ///
    /// [`record_root`]: Settings::record_root
    pub record_dir: Option<String>,
    /// The "set as default" tick next to the folder. False means the folder was
    /// chosen for this session and the picker will open there next time without
    /// the choice being permanent.
    pub record_dir_is_default: bool,
    /// Whether the "a chosen camera means video" default has already been
    /// applied to this settings file.
    ///
    /// A one-off migration, and it needs a marker rather than being re-derived
    /// each launch: without one, somebody who keeps their camera and turns
    /// video off would have it switched back on every time they started the
    /// app. Video only became possible in 3.2.0, so nobody can have chosen to
    /// disable it before this key existed — which is exactly why the migration
    /// is safe to run once and unsafe to run twice.
    pub video_default_applied: bool,
    /// Whether the "the video shows every panel" default has been applied.
    ///
    /// Its OWN key rather than reusing `video_default_applied`: that one is
    /// already true for everybody who has launched 3.4.0, so folding a second
    /// migration into it would silently skip them — which is exactly what
    /// happened on the first attempt at this one.
    pub shows_default_applied: bool,
    /// Show the take's folder in the file manager as soon as it is finished.
    ///
    /// Off by default. Somebody recording take after take does not want a
    /// Finder window in front of the piano every ninety seconds, and the one
    /// who does want it is the one who will go and tick the box.
    pub record_open_when_done: bool,
    /// The take name, kept across takes.
    ///
    /// It persists because the timestamp already guarantees uniqueness, so the
    /// typed name never has to be unique — type "nocturne" once, press record
    /// five times, get five adjacent folders and no overwrite dialog ever.
    pub record_take_name: Option<String>,
    /// The camera's stable UID, not its name. Two identical webcams share a
    /// name, and a name changes with the OS language.
    pub record_camera_uid: Option<String>,
    /// The audio input's stable UID. Same reasoning.
    pub record_audio_device: Option<String>,
    /// What a take is made of: `auto`, `input`, `plugin` or `both`.
    ///
    /// **A NEW key, and the old `record_audio_source` is deliberately left
    /// unread in `extra`.** That key defaulted to `"input"` and no control ever
    /// existed to change it, so every value it holds was written by the app
    /// rather than chosen by anybody — and the result was the first thing a
    /// user hit: load a piano, press record, get a file with only the
    /// microphone in it. Migrating it would mean guessing which `"input"`s were
    /// decisions; a new key has no such history.
    ///
    /// `auto` means "record whatever there is", which with an instrument loaded
    /// is the instrument and the input together.
    pub record_sources: String,
    /// The user picked "None — record MIDI only" in the audio-input picker.
    ///
    /// Its own key, because `record_audio_source` used to carry BOTH this and
    /// what a take is made of, and the two are unrelated questions: "no
    /// microphone" and "record the instrument too" can each be true or false
    /// independently. Overloading them is how choosing no input and choosing
    /// what to record could not both be expressed.
    pub record_input_off: bool,
    /// Count-in length in BEATS, at the take's tempo. Clamped to
    /// [`recorder::COUNT_IN_CHOICES`](crate::recorder::COUNT_IN_CHOICES) on
    /// read.
    ///
    /// Beats rather than seconds because a count-in is a musical instruction
    /// and the click is playing beats — a countdown in seconds beside a click
    /// in beats is two clocks disagreeing in front of the person trying to come
    /// in on time. (The old `record_preroll_s` key is simply left in `extra`,
    /// preserved but unread: three seconds does not convert into a number of
    /// beats without knowing a tempo it was never measured against.)
    pub record_count_in_beats: i64,
    /// Count-in length in BARS, at the take's time signature.
    ///
    /// **Bars, not beats.** A count-in is "two bars" to everybody who has ever
    /// been counted in, and in 6/8 two bars is twelve clicks — a number nobody
    /// wants to set by hand and which changes the moment the signature does.
    /// The old `record_count_in_beats` is migrated once and then left alone.
    pub record_count_in_bars: i64,
    /// The take's time signature, as `"6/8"`.
    ///
    /// Text rather than two numbers, because that is how it is written, how it
    /// is read back, and how a file from a later build with `13/16` in it
    /// survives being loaded by a build that only offers eight of them.
    pub record_time_signature: String,
    /// Start writing at the button press, with the count-in inside the take.
    ///
    /// Off means the count-in happens BEFORE the file starts, which is the
    /// usual thing to want: the bars before the downbeat are not part of the
    /// performance. On means the recording starts instantly and the count is at
    /// the head of the file, to trim to or to keep.
    pub record_count_in_in_take: bool,
    /// Frames per audio callback, or 0 for the device's own default.
    ///
    /// Applies to BOTH streams: they are two halves of one path, and a
    /// round-trip figure made of a 64-frame input and a 1024-frame output is a
    /// number nobody can act on.
    pub record_buffer_frames: i64,
    /// The VST3 bundle in each instrument slot. `None` is an empty slot.
    ///
    /// Paths and not names: a plugin's display name is not unique, changes
    /// between versions, and cannot be turned back into a file.
    ///
    /// Three of them because three instruments can be layered. Stored as a JSON
    /// ARRAY under `plugin_slots`, and a file written by the single-instrument
    /// build is migrated on read — its `plugin_path` becomes slot 0, so nobody
    /// loses the instrument they had chosen.
    pub plugin_slots: [Option<String>; crate::recorder::SLOTS],
    /// Linear gain per slot. Linear because that is what the audio path
    /// multiplies by; the band converts to dB to draw.
    pub plugin_gains: [f64; crate::recorder::SLOTS],
    pub metronome_gain: f64,
    pub input_gain: f64,
    /// Transpose, in semitones, applied to every note the display and the
    /// chord detector see.
    ///
    /// Persisted, because it is a mode you are in rather than a keypress: a
    /// transpose that reset on relaunch would silently change what the chord
    /// name means between sessions.
    pub transpose: i64,
    /// The transpose arrows are drawn in the chord view. On by default.
    pub show_transpose: bool,
    /// The click is running.
    pub metronome_on: bool,
    /// The click is mixed into the RECORDING as well as the monitors.
    ///
    /// **False by default and that is the important one.** A click bleeding
    /// into the take is a ruined take, and it is the mistake nobody notices
    /// until they open the file a week later.
    pub metronome_in_take: bool,
    /// Hide the running clock while recording. §5: after a blinking light, a
    /// running timer is the most-cited performance distraction, and no
    /// competitor offers the checkbox.
    pub record_hide_elapsed: bool,
    /// The Export dialog's whole state, when "use these settings for every
    /// take" is ticked.
    ///
    /// One nested object rather than eleven flat keys — see
    /// `ExportSpec::to_value` for why. A default spec is still written, so the
    /// file always shows what a take will produce.
    pub record_export: crate::recorder::ExportSpec,
    /// Unknown keys from the file, preserved verbatim on save (file order).
    pub extra: Map<String, Value>,
}

/// Where `Settings::path()` points during tests.
///
/// One file per THREAD, inside the OS temp dir.
///
/// Per thread and not per process, and that distinction is the whole point:
/// cargo runs tests in parallel threads, so a single per-process file still let
/// one test's `save_settings` land between another's before-and-after read.
/// That is exactly how `a_plugin_does_not_write_the_shared_settings_file` kept
/// failing — not because a plugin wrote anything, but because a DESKTOP test on
/// another thread did. Per thread, no two tests can see each other's file at
/// all.
#[cfg(test)]
fn test_path() -> PathBuf {
    thread_local! {
        static PATH: PathBuf = {
            let dir = std::env::temp_dir().join(format!(
                "tangent-test-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::create_dir_all(&dir);
            dir.join("settings.json")
        };
    }
    PATH.with(Clone::clone)
}

/// How far the transpose control will go, in semitones.
///
/// Two octaves each way. Further is arithmetically fine and musically pointless
/// — and the limit is what stops a held-down arrow key from walking the offset
/// somewhere a chord can never come back from.
pub const TRANSPOSE_MAX: i64 = 24;

/// The most a stored gain may be, as a linear multiplier.
///
/// `GAIN_MAX_DB` is +12 dB, which is about 4.0. The ceiling here is generous
/// rather than exact: it exists to stop a hand-edited or corrupt file putting
/// 1e9 into a monitor path, not to second-guess a future build's fader range.
const MAX_GAIN: f64 = 16.0;

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
            // The leather trim on a Tascam 388, sampled from a photograph of
            // one: the median of the hide rather than its mean, because the
            // photograph is lit and the mean carries the lamp. The ivory ink is
            // chosen against it at better than 6:1, so the band is comfortable
            // and not merely legible. See `SUPERSEDED_RECORDER_BG`.
            recorder_bg_color: Rgb {
                r: 0x62,
                g: 0x3A,
                b: 0x38,
            },
            chord_text_color: Rgb {
                r: 0xE8,
                g: 0xDC,
                b: 0xC0,
            },
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
            fretboard_custom_name: None,
            fretboard_custom_open: None,
            fretboard_capo: 0,
            fretboard_wood: crate::fretboard_panel::Wood::default().key().to_owned(),
            capo_style: crate::fretboard_panel::CapoStyle::default()
                .key()
                .to_owned(),
            fretboard_detached: false,
            theory_detached: false,
            theory_circle: false,
            theory_tonnetz: false,
            theory_triangles: false,
            theory_follow_midi: false,
            fretboard_win_w: None,
            fretboard_win_h: None,
            fretboard_win_x: None,
            fretboard_win_y: None,
            theory_win_w: None,
            theory_win_h: None,
            theory_win_x: None,
            theory_win_y: None,
            show_recorder: false,
            recorder_detached: false,
            recorder_win_w: None,
            recorder_win_h: None,
            recorder_win_x: None,
            recorder_win_y: None,
            record_dir: None,
            record_dir_is_default: false,
            video_default_applied: false,
            shows_default_applied: false,
            record_open_when_done: false,
            record_take_name: None,
            record_camera_uid: None,
            record_audio_device: None,
            record_sources: "auto".to_owned(),
            record_input_off: false,
            record_count_in_beats: 4,
            record_count_in_bars: 1,
            record_time_signature: "4/4".to_owned(),
            record_count_in_in_take: false,
            record_buffer_frames: 0,
            plugin_slots: [None, None, None],
            plugin_gains: [1.0; crate::recorder::SLOTS],
            metronome_gain: 0.5,
            input_gain: 1.0,
            transpose: 0,
            show_transpose: true,
            metronome_on: false,
            metronome_in_take: false,
            record_hide_elapsed: false,
            record_export: crate::recorder::ExportSpec::default(),
            extra: Map::new(),
        }
    }
}

impl Settings {
    /// Literal `~/.config/ivory/settings.json` on every platform (parity).
    ///
    /// **Under `cargo test` this is redirected to a temporary directory.** It
    /// has to be: several tests exercise code paths that save, and they were
    /// therefore rewriting the developer's real settings — colours, tunings,
    /// window positions and all — every time the suite ran. It also made
    /// `a_plugin_does_not_write_the_shared_settings_file` flaky, because it
    /// reads the file before and after and another test running in parallel
    /// would change it in between. A test that mutates the machine it runs on
    /// is a test nobody can trust twice.
    pub fn path() -> PathBuf {
        #[cfg(test)]
        {
            return test_path();
        }
        #[cfg(not(test))]
        {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
            home.join(".config").join("ivory").join("settings.json")
        }
    }

    pub fn load() -> Self {
        // A settings file that has never existed means nobody has arranged
        // anything yet, so show the app off rather than hiding two thirds of
        // it behind a menu nobody has opened.
        //
        // This is NOT the same question as the one `show_fretboard: false`
        // answers. That default exists because a window that grows on its own
        // AFTER AN UPDATE is a geometry surprise, and that was a real tester
        // complaint in 2.2.0. It is about people who already have a layout.
        // Someone opening Tangent for the first time has no layout to
        // surprise, and every band off means the guitar view and the theory
        // band are invisible features.
        //
        // The two cases really are distinguishable: an absent file, not an
        // unreadable one. Garbage still yields plain defaults, because a
        // corrupt file is not a first launch and quietly rearranging someone's
        // window because their settings failed to parse would be worse.
        let path = Self::path();
        if !path.exists() {
            // A first launch has no camera chosen, so there is nothing for the
            // migration to do — but marking it done stops it looking at an
            // empty file on the second launch.
            let mut s = Self::first_launch();
            s.video_default_applied = true;
            s.shows_default_applied = true;
            return s;
        }
        let mut s = Self::load_from(&path);
        s.apply_video_default();
        s.apply_shows_default();
        s
    }

    /// A camera that was chosen before video existed still means video.
    ///
    /// Choosing a camera is the one act in this app with no other purpose —
    /// opening one makes a light come on, and nobody does it to look at a
    /// preview. Everybody who picked one before 3.2.0 did so for a video the
    /// app could not yet produce, so this turns it on for them once.
    fn apply_video_default(&mut self) {
        if self.video_default_applied {
            return;
        }
        self.video_default_applied = true;
        if self.record_camera_uid.is_some() && self.record_export.video == crate::recorder::VideoMode::None {
            self.record_export.video = crate::recorder::VideoMode::Composite;
        }
    }

    /// A spec saved before the panel default changed still says
    /// piano-and-chord, so somebody with every band on would go on getting two
    /// of them.
    fn apply_shows_default(&mut self) {
        if self.shows_default_applied {
            return;
        }
        self.shows_default_applied = true;
        self.record_export.composite.shows = crate::recorder::DisplayShows::default();
    }

    /// What a brand-new install opens with: everything on.
    ///
    /// The Recorder is in here too, and it carries one consequence worth
    /// knowing: the band opens an audio input so the meter is live, which means
    /// macOS raises its microphone prompt on first launch rather than at the
    /// moment somebody asks to record. That is the price of the recorder being
    /// visible rather than hidden behind a menu nobody opens, and it is the
    /// trade the owner chose.
    pub fn first_launch() -> Self {
        Self {
            show_fretboard: true,
            theory_circle: true,
            theory_tonnetz: true,
            theory_triangles: true,
            show_recorder: true,
            ..Self::default()
        }
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

    /// Round-trip through the same JSON the settings file holds.
    ///
    /// A plugin instance does NOT own `~/.config/ivory/settings.json` — it is
    /// shared with the standalone and with every other instance — so its
    /// settings live in the DAW project instead, and this is how they get
    /// there and back. Deliberately the same text as the file, so a project
    /// saved by the plugin and a settings file written by the app say exactly
    /// the same thing, unknown keys and all.
    pub fn to_json(&self) -> String {
        Value::Object(self.to_map()).to_string()
    }

    /// The inverse, forgiving in the same way the file loader is: anything
    /// unreadable yields defaults rather than an error, because a plugin that
    /// refuses to open a project over a settings blob is worse than one that
    /// opens it looking wrong.
    pub fn from_json(text: &str) -> Self {
        match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(map)) => Self::from_map(map),
            _ => Self::default(),
        }
    }

    fn from_map(mut map: Map<String, Value>) -> Self {
        let mut s = Self::default();
        // Absent means version 0 — every file every build before this one
        // wrote. Removed from the map like every other known key, so it does
        // not also survive in `extra` and get written twice.
        let was = map
            .remove("settings_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

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
        take_bool(
            &mut map,
            "chord_window_detached",
            &mut s.chord_window_detached,
        );
        take_bool(&mut map, "keytoggle_enabled", &mut s.keytoggle_enabled);

        let take_color = |map: &mut Map<String, Value>, key: &str, dst: &mut Rgb| {
            if let Some(v) = map.remove(key) {
                if let Some(c) = v.as_str().and_then(Rgb::parse) {
                    *dst = c;
                }
            }
        };
        take_color(
            &mut map,
            "white_key_idle_color",
            &mut s.white_key_idle_color,
        );
        take_color(
            &mut map,
            "black_key_idle_color",
            &mut s.black_key_idle_color,
        );
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
        if let Some(v) = map.remove("recorder_bg_color") {
            if let Some(c) = v.as_str().and_then(Rgb::parse) {
                s.recorder_bg_color = c;
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
        take_opt_i64(
            &mut map,
            "detached_chord_width",
            &mut s.detached_chord_width,
        );
        take_opt_i64(&mut map, "detached_chord_x", &mut s.detached_chord_x);
        take_opt_i64(&mut map, "detached_chord_y", &mut s.detached_chord_y);
        take_opt_i64(&mut map, "window_x", &mut s.window_x);
        take_opt_i64(&mut map, "window_y", &mut s.window_y);
        take_bool(
            &mut map,
            "teach_apply_all_keys",
            &mut s.teach_apply_all_keys,
        );
        take_bool(&mut map, "show_fretboard", &mut s.show_fretboard);
        take_bool(&mut map, "theory_circle", &mut s.theory_circle);
        take_bool(&mut map, "theory_tonnetz", &mut s.theory_tonnetz);
        take_bool(&mut map, "theory_triangles", &mut s.theory_triangles);
        take_bool(&mut map, "theory_follow_midi", &mut s.theory_follow_midi);
        if let Some(v) = map.remove("capo_style") {
            if let Some(t) = v.as_str() {
                s.capo_style = t.to_owned();
            }
        }
        if let Some(Value::String(v)) = map.remove("fretboard_custom_name") {
            s.fretboard_custom_name = Some(v);
        }
        if let Some(Value::String(v)) = map.remove("fretboard_custom_open") {
            s.fretboard_custom_open = Some(v);
        }
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
        if let Some(v) = map.remove("fretboard_wood") {
            if let Some(t) = v.as_str() {
                s.fretboard_wood = t.to_owned();
            }
        }
        take_bool(&mut map, "fretboard_detached", &mut s.fretboard_detached);
        take_bool(&mut map, "theory_detached", &mut s.theory_detached);
        take_opt_i64(&mut map, "fretboard_win_w", &mut s.fretboard_win_w);
        take_opt_i64(&mut map, "fretboard_win_h", &mut s.fretboard_win_h);
        take_opt_i64(&mut map, "fretboard_win_x", &mut s.fretboard_win_x);
        take_opt_i64(&mut map, "fretboard_win_y", &mut s.fretboard_win_y);
        take_opt_i64(&mut map, "theory_win_w", &mut s.theory_win_w);
        take_opt_i64(&mut map, "theory_win_h", &mut s.theory_win_h);
        take_opt_i64(&mut map, "theory_win_x", &mut s.theory_win_x);
        take_opt_i64(&mut map, "theory_win_y", &mut s.theory_win_y);

        // ── the recorder ────────────────────────────────────────────────────
        take_bool(&mut map, "show_recorder", &mut s.show_recorder);
        take_bool(&mut map, "recorder_detached", &mut s.recorder_detached);
        take_opt_i64(&mut map, "recorder_win_w", &mut s.recorder_win_w);
        take_opt_i64(&mut map, "recorder_win_h", &mut s.recorder_win_h);
        take_opt_i64(&mut map, "recorder_win_x", &mut s.recorder_win_x);
        take_opt_i64(&mut map, "recorder_win_y", &mut s.recorder_win_y);
        let take_opt_str = |map: &mut Map<String, Value>, key: &str, dst: &mut Option<String>| {
            if let Some(Value::String(v)) = map.remove(key) {
                // An empty string is not a path, a uid or a name. Written by
                // hand or by an earlier build clearing a field, it would
                // otherwise be "chosen" and resolve to the process's working
                // directory the first time a take was written.
                if !v.trim().is_empty() {
                    *dst = Some(v);
                }
            }
        };
        take_opt_str(&mut map, "record_dir", &mut s.record_dir);
        take_bool(
            &mut map,
            "record_dir_is_default",
            &mut s.record_dir_is_default,
        );
        take_bool(
            &mut map,
            "record_open_when_done",
            &mut s.record_open_when_done,
        );
        take_bool(
            &mut map,
            "video_default_applied",
            &mut s.video_default_applied,
        );
        take_bool(
            &mut map,
            "shows_default_applied",
            &mut s.shows_default_applied,
        );
        take_opt_str(&mut map, "record_take_name", &mut s.record_take_name);
        take_opt_str(&mut map, "record_camera_uid", &mut s.record_camera_uid);
        take_opt_str(&mut map, "record_audio_device", &mut s.record_audio_device);
        take_bool(&mut map, "record_input_off", &mut s.record_input_off);
        if let Some(v) = map.remove("record_sources") {
            if let Some(t) = v.as_str() {
                s.record_sources = t.to_owned();
            }
        }
        // Whether the FILE carried a bars key. A local rather than a field on
        // `Settings`, because it is a fact about the file and not about the
        // user: a field would have to round-trip, and two settings that differ
        // only in how they were loaded are not different settings.
        let mut saw_bars = false;
        if let Some(v) = map.remove("record_count_in_bars") {
            if let Some(n) = v.as_i64() {
                s.record_count_in_bars = n;
                saw_bars = true;
            }
        }
        if let Some(Value::String(t)) = map.remove("record_time_signature") {
            s.record_time_signature = t;
        }
        take_bool(
            &mut map,
            "record_count_in_in_take",
            &mut s.record_count_in_in_take,
        );
        if let Some(v) = map.remove("record_buffer_frames") {
            if let Some(n) = v.as_i64() {
                s.record_buffer_frames = n;
            }
        }
        if let Some(v) = map.remove("record_count_in_beats") {
            if let Some(n) = v.as_i64() {
                s.record_count_in_beats = n;
            }
        }
        // Slots, with migration. The single-instrument build wrote
        // `plugin_path`/`plugin_gain`; a file from it must not silently lose
        // the instrument somebody chose, so those become slot 0 when the array
        // form is absent. Both old keys are then consumed rather than left in
        // `extra`, or they would be written back forever and a later build
        // would migrate them a second time over whatever slot 0 had become.
        let mut legacy_path: Option<String> = None;
        take_opt_str(&mut map, "plugin_path", &mut legacy_path);
        let legacy_gain = map.remove("plugin_gain").and_then(|v| v.as_f64());
        match map.remove("plugin_slots") {
            Some(Value::Array(items)) => {
                for (slot, item) in s.plugin_slots.iter_mut().zip(items) {
                    if let Value::String(path) = item {
                        if !path.trim().is_empty() {
                            *slot = Some(path);
                        }
                    }
                }
            }
            _ => s.plugin_slots[0] = legacy_path,
        }
        if let Some(Value::Array(items)) = map.remove("plugin_gains") {
            for (gain, item) in s.plugin_gains.iter_mut().zip(items) {
                if let Some(g) = item.as_f64() {
                    if g.is_finite() && (0.0..=MAX_GAIN).contains(&g) {
                        *gain = g;
                    }
                }
            }
        } else if let Some(g) = legacy_gain {
            if g.is_finite() && (0.0..=MAX_GAIN).contains(&g) {
                s.plugin_gains[0] = g;
            }
        }
        // Gains are sanitised HERE rather than at the point of use, unlike the
        // tuning and the capo, because a NaN or a negative multiplied into an
        // audio buffer is not a display glitch — it is a burst of noise into
        // somebody's monitors at whatever level the interface is set to.
        let take_gain = |map: &mut Map<String, Value>, key: &str, dst: &mut f64| {
            if let Some(g) = map.remove(key).and_then(|v| v.as_f64()) {
                if g.is_finite() && (0.0..=MAX_GAIN).contains(&g) {
                    *dst = g;
                }
            }
        };
        take_gain(&mut map, "metronome_gain", &mut s.metronome_gain);
        take_gain(&mut map, "input_gain", &mut s.input_gain);
        if let Some(v) = map.remove("transpose") {
            if let Some(n) = v.as_i64() {
                s.transpose = n.clamp(-TRANSPOSE_MAX, TRANSPOSE_MAX);
            }
        }
        take_bool(&mut map, "show_transpose", &mut s.show_transpose);
        take_bool(&mut map, "metronome_on", &mut s.metronome_on);
        take_bool(&mut map, "metronome_in_take", &mut s.metronome_in_take);
        take_bool(&mut map, "record_hide_elapsed", &mut s.record_hide_elapsed);
        if let Some(v) = map.remove("record_export") {
            s.record_export = crate::recorder::ExportSpec::from_value(&v);
        }

        s.extra = map; // whatever is left, preserved in file order
        if !saw_bars {
            s.convert_count_in_to_bars();
        }
        s.migrate_from(was);
        s
    }

    /// Move a file written before [`SETTINGS_VERSION`] onto the defaults it
    /// never got a chance to see.
    ///
    /// **Only values this app itself shipped as a default are touched**, and
    /// only once. A user who deliberately picked the old walnut, or who really
    /// does want the camera above the keyboard, loses that choice one time and
    /// makes it again — and it sticks for ever after, because the file is
    /// stamped on the next save. That is the cost, and it is worth paying: the
    /// alternative is what this app did until now, which is that a default,
    /// once shipped, could never be corrected for anybody who already had it.
    fn migrate_from(&mut self, was: u64) {
        if was >= SETTINGS_VERSION {
            return;
        }
        if was < 1 {
            if V0_RECORDER_BG.contains(&self.recorder_bg_color.to_hex().as_str()) {
                self.recorder_bg_color = Self::default().recorder_bg_color;
            }
            // The camera was the subject of a recording and the app was a band
            // underneath it. Since 3.10.0 it is the other way round — the app
            // draws the thing worth watching and the camera is the context —
            // and nobody who had already run the app ever saw that change.
            if self.record_export.composite.layout == crate::recorder::Layout::CameraAbove {
                self.record_export.composite.layout = crate::recorder::Layout::default();
            }
        }
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
        map.insert(
            "keytoggle_enabled".into(),
            Value::Bool(self.keytoggle_enabled),
        );
        if let Some(ref p) = self.custom_font_path {
            map.insert("custom_font_path".into(), Value::String(p.clone()));
        }
        map.insert(
            "font_choice".into(),
            Value::String(self.font_choice.clone()),
        );
        map.insert(
            "chord_text_color".into(),
            Value::String(self.chord_text_color.to_hex()),
        );
        map.insert(
            "recorder_bg_color".into(),
            Value::String(self.recorder_bg_color.to_hex()),
        );
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
        put_opt("fretboard_win_w", self.fretboard_win_w);
        put_opt("fretboard_win_h", self.fretboard_win_h);
        put_opt("fretboard_win_x", self.fretboard_win_x);
        put_opt("fretboard_win_y", self.fretboard_win_y);
        put_opt("theory_win_w", self.theory_win_w);
        put_opt("theory_win_h", self.theory_win_h);
        put_opt("theory_win_x", self.theory_win_x);
        put_opt("theory_win_y", self.theory_win_y);
        map.insert(
            "teach_apply_all_keys".into(),
            Value::Bool(self.teach_apply_all_keys),
        );
        map.insert("show_fretboard".into(), Value::Bool(self.show_fretboard));
        map.insert("theory_circle".into(), Value::Bool(self.theory_circle));
        map.insert("theory_tonnetz".into(), Value::Bool(self.theory_tonnetz));
        map.insert(
            "theory_triangles".into(),
            Value::Bool(self.theory_triangles),
        );
        map.insert(
            "theory_follow_midi".into(),
            Value::Bool(self.theory_follow_midi),
        );
        map.insert("capo_style".into(), Value::String(self.capo_style.clone()));
        map.insert(
            "fretboard_tuning".into(),
            Value::String(self.fretboard_tuning.clone()),
        );
        map.insert(
            "fretboard_capo".into(),
            Value::Number(self.fretboard_capo.into()),
        );
        // Additive and ABSENT when unset, like `custom_font_path`. A key that is
        // written as `null` is a key an older build has to know to ignore; a key
        // that is not there at all is one it never sees.
        if let Some(n) = &self.fretboard_custom_name {
            map.insert("fretboard_custom_name".into(), Value::String(n.clone()));
        }
        if let Some(o) = &self.fretboard_custom_open {
            map.insert("fretboard_custom_open".into(), Value::String(o.clone()));
        }
        map.insert(
            "fretboard_wood".into(),
            Value::String(self.fretboard_wood.clone()),
        );
        map.insert(
            "fretboard_detached".into(),
            Value::Bool(self.fretboard_detached),
        );
        map.insert(
            "theory_detached".into(),
            Value::Bool(self.theory_detached),
        );
        // ── the recorder ────────────────────────────────────────────────────
        map.insert("show_recorder".into(), Value::Bool(self.show_recorder));
        map.insert(
            "recorder_detached".into(),
            Value::Bool(self.recorder_detached),
        );
        // Written out longhand rather than through `put_opt` above: that
        // closure holds a mutable borrow of `map`, and reusing it here — after
        // the plain `map.insert` calls in between — extends the borrow across
        // them and does not compile.
        for (key, value) in [
            ("recorder_win_w", self.recorder_win_w),
            ("recorder_win_h", self.recorder_win_h),
            ("recorder_win_x", self.recorder_win_x),
            ("recorder_win_y", self.recorder_win_y),
        ] {
            if let Some(n) = value {
                map.insert(key.into(), Value::Number(n.into()));
            }
        }
        // Absent when unset, like `custom_font_path` — an absent output folder
        // means "the platform's default", which is a live decision made at the
        // point of use, not a string to be frozen into the file.
        for (key, value) in [
            ("record_dir", &self.record_dir),
            ("record_take_name", &self.record_take_name),
            ("record_camera_uid", &self.record_camera_uid),
            ("record_audio_device", &self.record_audio_device),
        ] {
            if let Some(v) = value {
                map.insert(key.into(), Value::String(v.clone()));
            }
        }
        map.insert(
            "record_dir_is_default".into(),
            Value::Bool(self.record_dir_is_default),
        );
        map.insert(
            "record_open_when_done".into(),
            Value::Bool(self.record_open_when_done),
        );
        map.insert(
            "video_default_applied".into(),
            Value::Bool(self.video_default_applied),
        );
        map.insert(
            "shows_default_applied".into(),
            Value::Bool(self.shows_default_applied),
        );
        map.insert(
            "record_sources".into(),
            Value::String(self.record_sources.clone()),
        );
        map.insert(
            "record_input_off".into(),
            Value::Bool(self.record_input_off),
        );
        map.insert(
            "record_count_in_beats".into(),
            // The DERIVED value, not the stored one. This key is legacy — bars
            // and the signature are the setting now — and writing it back
            // unchanged let the two disagree: the owner's file said 0 beats
            // beside 1 bar of 4/4, which is 4. A build old enough to read this
            // key would have counted them in for no bars at all.
            Value::Number(i64::from(self.count_in_beats()).into()),
        );
        map.insert(
            "record_count_in_bars".into(),
            Value::Number(self.record_count_in_bars.into()),
        );
        map.insert(
            "record_time_signature".into(),
            Value::String(self.record_time_signature.clone()),
        );
        map.insert(
            "record_count_in_in_take".into(),
            Value::Bool(self.record_count_in_in_take),
        );
        map.insert(
            "record_buffer_frames".into(),
            Value::Number(self.record_buffer_frames.into()),
        );
        // Written as full-length arrays including the empty slots, so slot 2
        // stays slot 2 when slot 1 is empty. A compacted list would silently
        // promote an instrument into a slot the user did not put it in.
        map.insert(
            "plugin_slots".into(),
            Value::Array(
                self.plugin_slots
                    .iter()
                    .map(|p| match p {
                        Some(path) => Value::String(path.clone()),
                        None => Value::Null,
                    })
                    .collect(),
            ),
        );
        map.insert(
            "plugin_gains".into(),
            Value::Array(
                self.plugin_gains
                    .iter()
                    .filter_map(|g| serde_json::Number::from_f64(*g))
                    .map(Value::Number)
                    .collect(),
            ),
        );
        for (key, gain) in [
            ("metronome_gain", self.metronome_gain),
            ("input_gain", self.input_gain),
        ] {
            if let Some(n) = serde_json::Number::from_f64(gain) {
                map.insert(key.into(), Value::Number(n));
            }
        }
        map.insert("transpose".into(), Value::Number(self.transpose.into()));
        map.insert("show_transpose".into(), Value::Bool(self.show_transpose));
        map.insert("metronome_on".into(), Value::Bool(self.metronome_on));
        map.insert(
            "metronome_in_take".into(),
            Value::Bool(self.metronome_in_take),
        );
        map.insert(
            "record_hide_elapsed".into(),
            Value::Bool(self.record_hide_elapsed),
        );
        map.insert("record_export".into(), self.record_export.to_value());
        // Last of the known keys, and written unconditionally: a file this
        // build saves has seen every migration up to here, so the next build's
        // migration must not run against it. See `SETTINGS_VERSION`.
        map.insert(
            "settings_version".into(),
            Value::Number(SETTINGS_VERSION.into()),
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
            // Write-then-rename, the same shape `OverrideStore::save` already
            // uses. A bare `fs::write` truncates first, so a crash or a full
            // disk mid-write leaves a half-written file — and `load_from`
            // answers a parse error with ALL DEFAULTS, which it then saves over
            // the wreckage. One torn write costs every colour, size and
            // remembered position the user ever chose.
            //
            // Rare with one writer. The plugin makes it routine: several
            // instances plus the standalone can all be saving this file, and
            // this is what stops one of them catching another mid-write.
            //
            // The temp file sits beside the target so the rename stays within
            // one filesystem, which is what makes it atomic. Errors remain
            // silent (parity), but a failure can no longer destroy the previous
            // good file, and it must not leave debris behind either.
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                if std::fs::rename(&tmp, path).is_err() {
                    let _ = std::fs::remove_file(&tmp);
                }
            } else {
                let _ = std::fs::remove_file(&tmp);
            }
        }
    }

    /// "Reset Settings to Default" (D-UI-8): resets the 13 parity keys and
    /// `custom_font_path`, keeps unknown keys.
    /// **Resets to what a FRESH INSTALL gets, not to `Settings::default()`.**
    ///
    /// Those were two different things, and the gap was a real trap: a new
    /// install opens with every band showing, and "Reset Settings to Default"
    /// took you to keyboard-and-chord. So the app's own reset produced a state
    /// the app never starts in — and somebody who pressed it expecting to get
    /// back to how it came, got less, three launches running, and reasonably
    /// concluded their settings were not being saved.
    ///
    /// One meaning of "default" now. `Settings::default()` remains the
    /// mechanical one — every field's zero value, and what a corrupt file falls
    /// back to, where rearranging somebody's window on top of a parse failure
    /// would be the wrong kindness.
    pub fn reset_to_defaults(&mut self) {
        let extra = std::mem::take(&mut self.extra);
        *self = Self::first_launch();
        self.extra = extra;
        // A reset is not a first launch: the migrations have already run, and
        // re-running them would put video back on for somebody who has just
        // asked for everything to go back to how it was.
        self.video_default_applied = true;
        self.shows_default_applied = true;
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

    /// The fingerboard wood, falling back rather than rewriting the file.
    pub fn fretboard_wood(&self) -> crate::fretboard_panel::Wood {
        crate::fretboard_panel::Wood::from_key(&self.fretboard_wood)
    }

    /// Size to open the popped-out neck at: whatever the user last left it, or
    /// the default. A remembered width is what distinguishes the two, the same
    /// marker the detached chord window uses.
    pub fn fretboard_win_size(&self) -> egui::Vec2 {
        match (self.fretboard_win_w, self.fretboard_win_h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => egui::Vec2::new(w as f32, h as f32),
            _ => crate::fretboard_panel::DETACHED_DEFAULT,
        }
    }

    pub fn theory_win_size(&self) -> egui::Vec2 {
        match (self.theory_win_w, self.theory_win_h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => egui::Vec2::new(w as f32, h as f32),
            _ => crate::theory_panel::DETACHED_DEFAULT,
        }
    }

    pub fn theory_win_pos(&self) -> Option<egui::Pos2> {
        match (self.theory_win_x, self.theory_win_y) {
            (Some(x), Some(y)) => Some(egui::Pos2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    pub fn fretboard_win_pos(&self) -> Option<egui::Pos2> {
        match (self.fretboard_win_x, self.fretboard_win_y) {
            (Some(x), Some(y)) => Some(egui::Pos2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    pub fn recorder_win_size(&self) -> egui::Vec2 {
        match (self.recorder_win_w, self.recorder_win_h) {
            (Some(w), Some(h)) if w > 0 && h > 0 => egui::Vec2::new(w as f32, h as f32),
            _ => crate::recorder_panel::DETACHED_DEFAULT,
        }
    }

    pub fn recorder_win_pos(&self) -> Option<egui::Pos2> {
        match (self.recorder_win_x, self.recorder_win_y) {
            (Some(x), Some(y)) => Some(egui::Pos2::new(x as f32, y as f32)),
            _ => None,
        }
    }

    /// Where takes go.
    ///
    /// Resolved here rather than stored, so an absent `record_dir` follows the
    /// machine rather than whatever the machine looked like when the file was
    /// written. `dirs::video_dir()` is the right call on all three platforms —
    /// it reads `FOLDERID_Videos` on Windows and `$XDG_VIDEOS_DIR` on Linux
    /// rather than assuming an English folder name, which is the bug in every
    /// hardcoded `~/Videos`.
    ///
    /// **Never the Desktop and never the bundle directory.** The fallback when
    /// the platform has no videos folder at all is the home directory, because
    /// a take written next to the executable is one nobody finds again and one
    /// that fails outright inside a signed `.app`.
    pub fn record_root(&self) -> PathBuf {
        if let Some(d) = &self.record_dir {
            let p = PathBuf::from(d);
            if !p.as_os_str().is_empty() {
                return p;
            }
        }
        let base = dirs::video_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("Tangent")
    }

    /// Count-in length, clamped to what the UI can actually offer.
    ///
    /// Sanitised at the point of use, like the tuning and the capo: a file from
    /// a later build with a 12-beat count-in keeps its 12 in the file and
    /// counts in with the nearest thing this build has.
    pub fn count_in_beats(&self) -> u32 {
        self.time_signature().beats_in(self.count_in_bars())
    }

    /// Count-in length in BARS.
    pub fn count_in_bars(&self) -> u32 {
        self.record_count_in_bars.clamp(0, 8) as u32
    }

    /// Frames per callback, or `None` for the device's own default.
    ///
    /// Sanitised at the point of use like every other stored choice: a file
    /// with 999 in it keeps its 999 and runs at the nearest size that exists.
    pub fn buffer_frames(&self) -> Option<u32> {
        let want = self.record_buffer_frames;
        if want <= 0 {
            return None;
        }
        let want = want.clamp(0, i64::from(u32::MAX)) as u32;
        crate::recorder::BUFFER_CHOICES
            .into_iter()
            .min_by_key(|c| c.abs_diff(want))
    }

    /// The take's time signature, sanitised at the point of use.
    ///
    /// Like the tuning and the capo: a file from a later build with `13/16` in
    /// it keeps its 13/16 in the file and plays in the nearest thing this build
    /// understands — which, since anything valid is understood, is itself.
    pub fn time_signature(&self) -> crate::recorder::TimeSignature {
        crate::recorder::TimeSignature::parse(&self.record_time_signature).unwrap_or_default()
    }

    /// Convert a pre-bars count-in once.
    ///
    /// The old key was BEATS and the offered values were 0, 4 and 8 — which in
    /// 4/4, the only signature that existed, are 0, 1 and 2 bars. Anything else
    /// divides and rounds, and a count-in somebody had set to a non-zero number
    /// of beats never becomes zero bars: silently removing somebody's count-in
    /// is worse than rounding it up.
    fn convert_count_in_to_bars(&mut self) {
        let beats = self.record_count_in_beats.max(0);
        let per_bar = i64::from(self.time_signature().beats.max(1));
        self.record_count_in_bars = if beats == 0 {
            0
        } else {
            (beats / per_bar).max(1)
        };
    }

    /// Everything the band renders but does not own, in one value.
    pub fn knobs(&self) -> crate::recorder::Knobs {
        crate::recorder::Knobs {
            gains: crate::recorder::Gains {
                slots: std::array::from_fn(|i| self.plugin_gains[i] as f32),
                metronome: self.metronome_gain as f32,
                input: self.input_gain as f32,
            },
            metronome_on: self.metronome_on,
            metronome_in_take: self.metronome_in_take,
            // ONE tempo, shared with the SMF's tempo mark. A click at 90
            // against a file that says 120 is a take nobody can edit later.
            tempo_bpm: self.record_export.tempo_bpm,
            count_in_beats: self.count_in_beats(),
            count_in_bars: self.count_in_bars(),
            time_signature: self.time_signature(),
        }
    }

    /// The board the fretboard view draws, from whatever is in the file.
    ///
    /// Every value is sanitised HERE rather than on load, so a settings file
    /// written by a later build (a tuning this one has never heard of, a capo
    /// of 40) still opens, still draws something sensible, and still keeps its
    /// own values when it goes back to the build that understands them.
    /// What the capo is made of. Unknown values fall back to wood here rather
    /// than on load, so a file from a later build keeps its own value.
    pub fn capo_style(&self) -> crate::fretboard_panel::CapoStyle {
        crate::fretboard_panel::CapoStyle::from_key(&self.capo_style)
    }

    /// D-UI-17: the selected theory diagrams, as one value.
    pub fn theory_views(&self) -> crate::theory_panel::Views {
        crate::theory_panel::Views {
            circle: self.theory_circle,
            tonnetz: self.theory_tonnetz,
            triangles: self.theory_triangles,
        }
    }

    /// Turn one diagram on or off. Keeping the mapping in one place means the
    /// menu, the keyboard shortcut and the settings file cannot disagree about
    /// which flag is which.
    pub fn set_theory_view(&mut self, v: crate::theory_panel::View, on: bool) {
        use crate::theory_panel::View;
        match v {
            View::Circle => self.theory_circle = on,
            View::Tonnetz => self.theory_tonnetz = on,
            View::Triangles => self.theory_triangles = on,
        }
    }

    /// The value `fretboard_tuning` holds when the custom tuning is selected.
    ///
    /// A sentinel rather than a separate boolean, so there is exactly one field
    /// answering "which tuning is in use" and no way for two of them to
    /// disagree. It cannot collide with a preset: `Tuning::by_name` is checked
    /// first and no preset is called this.
    pub const CUSTOM_TUNING: &str = "Custom";

    /// The user's custom tuning, if one has been built and is still valid.
    ///
    /// Re-validated on every read rather than trusted. The file is editable by
    /// hand and is shared with older and newer builds, and an invalid tuning —
    /// one whose strings do not ascend — would break the voicing solver's
    /// monotone search invariant rather than merely looking wrong.
    pub fn custom_tuning(&self) -> Option<ivory_core::fretboard::Tuning> {
        use ivory_core::fretboard::Tuning;
        let name = self.fretboard_custom_name.as_ref()?;
        let open = self.fretboard_custom_open.as_ref()?;
        let pitches: Option<Vec<u8>> = open
            .split(',')
            .map(|p| p.trim().parse::<u8>().ok())
            .collect();
        Tuning::custom(name.clone(), pitches?).ok()
    }

    pub fn fretboard_spec(&self) -> ivory_core::fretboard::FretboardSpec {
        use ivory_core::fretboard::{FretboardSpec, Tuning};
        // A preset wins on name; then the custom tuning; then Standard. The
        // fallback is deliberate and silent: a settings file naming a tuning
        // this build does not have (an older build, or a hand edit) opens on
        // Standard rather than refusing to start, and the stored name is left
        // alone so a newer build still finds it.
        let tuning = Tuning::by_name(&self.fretboard_tuning)
            .cloned()
            .or_else(|| {
                (self.fretboard_tuning == Self::CUSTOM_TUNING)
                    .then(|| self.custom_tuning())
                    .flatten()
            })
            .unwrap_or_else(Tuning::standard);
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
pub fn clamp_to_monitor(
    pos: egui::Pos2,
    size: egui::Vec2,
    monitor: Option<egui::Vec2>,
) -> egui::Pos2 {
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

    /// A first launch shows the whole app; a corrupt file does not get
    /// rearranged behind the user's back. Both matter, and they are one line
    /// apart in `load()`.
    /// **A default that has never reached anybody is not a default.**
    ///
    /// The file is written whole, so the first save any user makes pins the
    /// day's default for every key they never touched. Three shipped changes
    /// fell into that hole — two corrections to the recorder's brown and the
    /// one that made the app rather than the camera the subject of a recording,
    /// which the owner asked for four times and never once received. A file
    /// written before the stamp gets those defaults; a stamped file is left
    /// alone for ever after.
    #[test]
    fn an_unstamped_file_gets_the_defaults_it_never_had_a_chance_to_see() {
        let now = Settings::default();
        for fossil in V0_RECORDER_BG {
            let json = format!("{{\"recorder_bg_color\": \"{fossil}\"}}");
            assert_eq!(
                Settings::from_json(&json).recorder_bg_color,
                now.recorder_bg_color,
                "{fossil} is a superseded default and should have moved"
            );
        }
        // The one the owner has been recording with since 3.10.0 said it had
        // changed: camera on top, the app in a band underneath.
        let json = r#"{"record_export": {"layout": "camera_above"}}"#;
        assert_eq!(
            Settings::from_json(json).record_export.composite.layout,
            crate::recorder::Layout::default(),
            "the pre-3.10 export layout survived the migration"
        );

        // A colour nobody ever shipped is somebody's own, and survives.
        let mine = "#1b3a62";
        assert!(!V0_RECORDER_BG.contains(&mine), "pick an unshipped colour");
        let json = format!("{{\"recorder_bg_color\": \"{mine}\"}}");
        assert_eq!(
            Settings::from_json(&json).recorder_bg_color.to_hex(),
            mine,
            "a chosen colour was overwritten by the migration"
        );
        // And the current defaults are not themselves fossils, or the next
        // change would migrate to itself and silently do nothing.
        assert!(!V0_RECORDER_BG.contains(&now.recorder_bg_color.to_hex().as_str()));
        assert_ne!(now.record_export.composite.layout, crate::recorder::Layout::CameraAbove);
    }

    /// **It runs ONCE.** Somebody who really does want the camera above the
    /// keyboard, or the old walnut, chooses it again and keeps it — because the
    /// file they save is stamped, and a stamped file is not migrated. Without
    /// this the migration is not a migration, it is a setting the user is not
    /// allowed to have.
    #[test]
    fn a_stamped_file_keeps_a_choice_that_matches_an_old_default() {
        let mut s = Settings::default();
        s.recorder_bg_color = Rgb::parse(V0_RECORDER_BG[0]).expect("hex");
        s.record_export.composite.layout = crate::recorder::Layout::CameraAbove;

        let json = s.to_json();
        assert!(json.contains("settings_version"), "the save is not stamped");
        let back = Settings::from_json(&json);
        assert_eq!(
            back.recorder_bg_color.to_hex(),
            V0_RECORDER_BG[0],
            "a deliberate re-pick was migrated away again"
        );
        assert_eq!(
            back.record_export.composite.layout,
            crate::recorder::Layout::CameraAbove
        );
        // And the stamp does not leak into `extra`, which would write it twice.
        assert_eq!(json.matches("settings_version").count(), 1, "{json}");
    }

    /// A custom tuning survives a save/load round trip and is usable.
    #[test]
    fn a_custom_tuning_round_trips_through_the_settings_file() {
        let mut s = Settings::default();
        s.fretboard_tuning = Settings::CUSTOM_TUNING.to_owned();
        s.fretboard_custom_name = Some("Nashville".to_owned());
        s.fretboard_custom_open = Some("40,45,50,55,59,64".to_owned());

        let json = s.to_json();
        let back = Settings::from_json(&json);
        assert_eq!(back.fretboard_custom_name.as_deref(), Some("Nashville"));
        let spec = back.fretboard_spec();
        assert_eq!(spec.tuning.name, "Nashville");
        assert_eq!(spec.tuning.strings(), 6);
    }

    /// The two keys are absent, not null, when no custom tuning exists — the
    /// same rule `custom_font_path` follows, so an older build never meets them.
    #[test]
    fn an_unused_custom_tuning_writes_no_keys_at_all() {
        let json = Settings::default().to_json();
        assert!(!json.contains("fretboard_custom_name"), "{json}");
        assert!(!json.contains("fretboard_custom_open"), "{json}");
    }

    /// A hand-edited or corrupt custom tuning falls back rather than reaching
    /// the solver. Descending strings would break its monotone search
    /// invariant, which is a silent wrong answer rather than a crash.
    #[test]
    fn an_invalid_custom_tuning_falls_back_to_standard() {
        for bad in [
            "64,59,55,50,45,40",  // descending: violates the solver's invariant
            "40,45,notanumber",   // unparseable
            "",                   // empty
            "40,45,50,55,59,64,64,64,64,64,64,64,64", // 13 strings, over MAX
        ] {
            let mut s = Settings::default();
            s.fretboard_tuning = Settings::CUSTOM_TUNING.to_owned();
            s.fretboard_custom_name = Some("Broken".to_owned());
            s.fretboard_custom_open = Some(bad.to_owned());
            assert!(s.custom_tuning().is_none(), "{bad:?} should be refused");
            assert_eq!(
                s.fretboard_spec().tuning.name,
                "Standard",
                "{bad:?} should fall back rather than reach the solver"
            );
        }
    }

    /// Selecting a preset does not discard the custom tuning, so switching away
    /// and back does not lose work.
    #[test]
    fn choosing_a_preset_keeps_the_custom_tuning_on_disk() {
        let mut s = Settings::default();
        s.fretboard_custom_name = Some("Mine".to_owned());
        s.fretboard_custom_open = Some("38,45,50,55,59,64".to_owned());
        s.fretboard_tuning = "DADGAD".to_owned();
        let back = Settings::from_json(&s.to_json());
        assert_eq!(back.fretboard_spec().tuning.name, "DADGAD");
        assert!(back.custom_tuning().is_some(), "the custom tuning was lost");
    }

    /// **The legacy beats key cannot disagree with the bars that produced it.**
    ///
    /// Bars and the signature are the setting; `record_count_in_beats` is left
    /// in the file so a downgrade still counts something in. Writing it back
    /// unchanged let the two drift — the owner's file said 0 beats beside 1 bar
    /// of 4/4, which is 4 — so an older build would have counted them in for no
    /// bars at all. It is derived on the way out.
    #[test]
    fn the_legacy_count_in_key_is_derived_and_not_remembered() {
        let mut s = Settings::default();
        s.record_time_signature = "6/8".to_owned();
        s.record_count_in_bars = 2;
        // Stale, exactly as the broken cycle left it.
        s.record_count_in_beats = 0;

        let back = Settings::from_map(s.to_map());
        assert_eq!(back.record_count_in_bars, 2);
        assert_eq!(back.record_time_signature, "6/8");
        assert_eq!(
            back.record_count_in_beats, 12,
            "2 bars of 6/8 is 12 beats, whatever the stale key said"
        );
        // And the live accessor agrees with the file, which is the point.
        assert_eq!(back.count_in_beats(), 12);
    }

    #[test]
    fn a_first_launch_shows_everything_and_a_broken_file_does_not() {
        let first = Settings::first_launch();
        assert!(
            first.show_fretboard,
            "the guitar view is invisible on a fresh install"
        );
        assert!(
            first.theory_views().count() == 3,
            "the theory band is invisible on a fresh install"
        );
        assert!(
            first.show_recorder,
            "the recorder is invisible on a fresh install"
        );
        // Nothing else moves: a first launch is the DEFAULTS plus visibility,
        // not a second set of preferences to keep in step. Every flag listed
        // here is one somebody can see; anything else appearing in this list
        // would mean a first launch had grown a PREFERENCE of its own.
        let same = Settings {
            show_fretboard: false,
            theory_circle: false,
            theory_tonnetz: false,
            theory_triangles: false,
            show_recorder: false,
            ..first.clone()
        };
        assert_eq!(same, Settings::default());

        // An unreadable file is not a first launch.
        let dir = std::env::temp_dir().join(format!("tangent-firstrun-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let broken = dir.join("settings.json");
        std::fs::write(&broken, b"{ not json").unwrap();
        assert_eq!(
            Settings::load_from(&broken),
            Settings::default(),
            "a corrupt settings file rearranged the window"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The settings a plugin carries in its project file are the same text the
    /// standalone writes, unknown keys included. Anything else and a project
    /// saved by the plugin would quietly drop whatever a newer build knew
    /// about — which is the failure `extra` exists to prevent in the file.
    #[test]
    fn json_round_trips_through_a_project_file() {
        let mut s = Settings::default();
        s.dark_mode = true;
        s.window_size_percent = 150;
        s.theory_tonnetz = true;
        s.fretboard_tuning = "DADGAD".to_owned();
        s.extra.insert(
            "a_key_from_a_later_build".into(),
            Value::String("kept".into()),
        );

        let back = Settings::from_json(&s.to_json());
        assert_eq!(back, s, "a project round trip changed the settings");
        assert_eq!(
            back.extra.get("a_key_from_a_later_build"),
            Some(&Value::String("kept".into())),
            "the plugin dropped a key it did not understand"
        );
    }

    /// Garbage in a project file yields defaults, not a panic and not a
    /// refusal to open. Same rule as the settings file (spec §8).
    #[test]
    fn unreadable_project_settings_fall_back_to_defaults() {
        for junk in ["", "null", "[]", "{", "not json at all", "42"] {
            assert_eq!(
                Settings::from_json(junk),
                Settings::default(),
                "{junk:?} did not fall back"
            );
        }
    }
    use super::*;

    /// A torn write costs the user every setting they ever chose, because
    /// `load_from` answers a parse error with all-defaults and then saves those
    /// over the wreckage. The plugin turns "rare" into "routine".
    #[test]
    fn saving_is_atomic_and_leaves_no_debris() {
        let dir = std::env::temp_dir().join(format!("tangent-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("settings.json");

        let mut s = Settings::default();
        s.window_size_percent = 175;
        s.save_to(&path);

        // It landed, it parses, and it is the file we meant.
        let back = Settings::load_from(&path);
        assert_eq!(back.window_size_percent, 175);

        // No temp file left beside it, on success.
        let debris: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect();
        assert!(debris.is_empty(), "left temp files behind: {debris:?}");

        // Overwriting an existing file works and does not corrupt it.
        s.window_size_percent = 50;
        s.save_to(&path);
        assert_eq!(Settings::load_from(&path).window_size_percent, 50);
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "exactly one file"
        );

        // The rename must not have reordered the keys: the file order is
        // Python's insertion order and unknown keys keep their place.
        let text = std::fs::read_to_string(&path).unwrap();
        let first = text.find("dark_mode").unwrap();
        let later = text.find("window_size_percent").unwrap();
        assert!(first < later, "key order did not survive the rename");

        let _ = std::fs::remove_dir_all(&dir);
    }

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
        assert_eq!(
            spec.tuning.name, "Standard",
            "unknown tuning draws as standard"
        );
        assert!(
            spec.capo < spec.frets,
            "a capo past the last fret is nobody's intent"
        );
        // And the round trip does not eat the value it could not understand.
        let out = serde_json::to_string(&Value::Object(s.to_map())).unwrap();
        assert!(out.contains("Nashville High Strung"));
        assert!(out.contains("\"fretboard_capo\":40"));
    }

    #[test]
    fn the_shipped_tunings_all_survive_a_round_trip() {
        for t in ivory_core::fretboard::TUNINGS {
            let mut s = Settings::default();
            s.fretboard_tuning = t.name.to_string();
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
        assert_eq!(
            s.window_pos_for_use(),
            Some(egui::Pos2::new(-1920.0, -40.0))
        );
        assert_eq!(
            s.detached_pos_for_use(),
            Some(egui::Pos2::new(300.0, 220.0))
        );
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
            clamp_to_monitor(
                egui::Pos2::new(50.0, 50.0),
                egui::Vec2::new(4000.0, 4000.0),
                mon
            ),
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
        let json =
            r#"{"dark_mode": "yes", "window_size_percent": 150, "detached_chord_height": -3}"#;
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
