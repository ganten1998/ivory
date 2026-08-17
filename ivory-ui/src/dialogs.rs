//! Dialogs (spec §7): MIDI picker (default styling), About (themed, D-UI-6),
//! color modals (egui picker, themed, D-UI-7), MIDI info/error boxes, the
//! custom tuning editor for the guitar view (D-UI-15), and the recorder's
//! Export dialog (RECORDER-PLAN §5).
//! Each dialog is its own decorated immediate viewport, like Qt's top-level
//! dialogs (the fixed-size main window is too small to host them) — or, in a
//! plugin, a layer in the canvas. `show_dialog_viewport` is the funnel that
//! makes that the same code, and nothing here may open a window by itself.

use std::path::{Path, PathBuf};

use crate::fonts;
use crate::menu::ColorTarget;
// `Layout` is taken by egui's own, and the recorder's is the composite's
// camera-over-display arrangement. Aliased rather than qualified at every use,
// because `unused_qualifications` is a workspace lint and the two would
// otherwise have to be spelled out in full on alternate lines.
use crate::recorder::{
    DisplayShows, ExportSpec, Layout as VideoLayout, Resolution, VideoMode, FPS_CHOICES, MAX_BPM,
    MIN_BPM,
};
use egui::{
    Align, Button, Color32, CornerRadius, FontId, Layout, Pos2, Rect, RichText, Stroke, Vec2,
};
use ivory_core::fretboard::{self, Tuning, TuningError, MAX_STRINGS};
use ivory_core::patterns::{NOTE_NAMES, NOTE_NAMES_FLAT};
use ivory_core::OverrideInfo;

pub enum Dialog {
    MidiPicker {
        ports: Vec<String>,
        selected: Option<usize>,
        current: Option<String>,
    },
    /// Choose a camera or an audio input for the recorder.
    ///
    /// One dialog for both, because the app does exactly the same thing with
    /// each — the same reason `ports::CaptureDevices` is one trait. Separate
    /// from [`Dialog::MidiPicker`] despite looking identical, because the
    /// values are different: a MIDI port is chosen and reported by NAME, and a
    /// capture device by stable UID. Merging them would mean the merged dialog
    /// carried both and each caller ignored one.
    DevicePicker {
        kind: DeviceKind,
        devices: Vec<crate::ports::DeviceInfo>,
        /// `None` is the "None" row, which is a real choice: closing the camera
        /// is how you record audio-only without unplugging anything.
        selected: Option<usize>,
        /// The uid currently open.
        current: Option<String>,
    },
    /// Choose the VST3 instrument Tangent plays through.
    ///
    /// **The list is a directory listing and nothing more** — see the "plugin
    /// picker" section comment for why this dialog opens no modules, and
    /// [`Dialog::plugin_picker`] for how the caller builds it.
    PluginPicker {
        /// One row per installed VST3 bundle, already sorted for humans.
        plugins: Vec<PluginEntry>,
        /// An index into `plugins`, NEVER into the filtered list. The two are
        /// only ever reconciled in [`plugin_choice`], which is the one place a
        /// selection becomes a value.
        selected: Option<usize>,
        /// The bundle path currently loaded, if any.
        current: Option<String>,
        /// Free-text filter. There are 112 VST3s on the owner's machine and
        /// scrolling a list that long to find "Pianoteq" is the difference
        /// between a usable picker and a bad one, so this is the primary
        /// interaction rather than a nicety.
        filter: String,
        /// Take keyboard focus on the first frame, like the other text fields
        /// do. It is also what scrolls the loaded plugin into view once, which
        /// is the only thing that makes marking it worth anything ninety rows
        /// down.
        focus: bool,
    },
    NoMidiInput,
    MidiError {
        message: String,
    },
    About,
    /// Shown at startup until dismissed with its checkbox. Tangent is free; this
    /// asks for support without gating anything, so it must never feel like a
    /// paywall prompt.
    Welcome {
        dont_show_again: bool,
    },
    /// Paste a supporter key. Stays open on failure so the message can explain
    /// what went wrong without the user losing what they pasted.
    SupporterKey {
        input: String,
        /// Feedback from the last attempt, if any.
        message: Option<String>,
        /// Name on the currently installed licence, when there is one.
        installed_as: Option<String>,
        /// As above: focus and select-all on the first frame.
        focus: bool,
    },
    ColorPick {
        target: ColorTarget,
        color: Color32,
    },
    // D-UI-5 teach dialogs.
    TeachChord {
        /// Snapshot of the held MIDI notes (used to key the override on save).
        notes: Vec<u8>,
        /// Held notes pre-rendered as names under the active preference.
        note_names: String,
        /// The current detected label (or a placeholder when none).
        current_label: String,
        /// Editable name, prefilled with the current label.
        input: String,
        apply_all_keys: bool,
        /// Take keyboard focus and select the whole field on the first frame.
        /// Cleared once done, so a later frame does not fight the user's own
        /// cursor.
        focus: bool,
    },
    ManageTaught {
        rows: Vec<OverrideInfo>,
        /// D-UI-9 footer: learned-re-ranker state, refreshed on open.
        learning: LearningStatus,
    },
    // D-UI-9 learned re-ranker.
    /// Pick a different reading for the held voicing and train toward it.
    CorrectChord {
        notes: Vec<u8>,
        note_names: String,
        current_label: String,
        /// The names the re-ranker can actually be trained toward, best first.
        candidates: Vec<(String, f64)>,
        selected: Option<usize>,
    },
    /// Plain themed report of what a correction achieved.
    LearnResult {
        title: &'static str,
        message: String,
    },
    /// Build a tuning by hand (the "Custom..." entry under Tuning).
    ///
    /// Opened with [`Dialog::custom_tuning`], which seeds it from whatever
    /// tuning is live — almost every custom tuning is "standard but…", and
    /// starting from an empty grid would make the common case the slow one.
    CustomTuning {
        /// The tuning's name. Free text, and the one field the user must
        /// actually think about: see [`Problem::NameClash`] for what happens
        /// when it collides with a preset.
        name: String,
        /// One row per string, index 0 the LOWEST — the same order as
        /// `Tuning::open`, so no reversal happens anywhere except in the
        /// drawing loop, which paints them highest-first the way tab and every
        /// chord chart do.
        strings: Vec<TuningString>,
        /// Sharps or flats, mirrored from `Settings::prefer_flats` when the
        /// dialog is opened. Held here rather than read from settings because
        /// this crate's dialogs are handed their state, and because a tuning
        /// spelled Eb2 in the editor and D#2 on the piano is the one thing a
        /// note-name field must never do.
        prefer_flats: bool,
        /// Focus and select the name on the first frame, as the other dialogs
        /// with a prefilled field do.
        focus: bool,
    },
    /// What one take writes (RECORDER-PLAN §5).
    ///
    /// Opened with [`Dialog::export`], which is also where the one rule that
    /// carries real information is applied — see [`Rederivable`].
    Export {
        /// The spec being edited.
        ///
        /// **`tempo_bpm` in here is the value the dialog OPENED with, not the
        /// live one.** `tempo_text` is what the user is looking at and
        /// [`export_ready`] is the single place the two are reconciled; a
        /// keystroke never writes this field, which is what stops "9" on the
        /// way to "92" from being stored as 20.
        spec: ExportSpec,
        /// Tempo as text, because a partially-typed number is not a number.
        /// "9" on the way to "92" must not be clamped to 20 under the user's
        /// cursor.
        tempo_text: String,
        /// "Use these settings for every take" — the same one-click
        /// persistence affordance as the output folder's "Default" tick, and
        /// it writes to the same place.
        remember: bool,
        /// Opened from the result strip after a take rather than before one.
        /// Camera controls are greyed and the reason is stated.
        post_take: bool,
        /// What the finished take actually captured, when `post_take`. Used to
        /// decide which controls can still do anything.
        had_camera: bool,
    },
}

/// Snapshot of learned-re-ranker state for display.
#[derive(Clone, Default)]
pub struct LearningStatus {
    pub on: bool,
    pub corrections: u32,
    pub has_learned: bool,
    /// One row per perceptron feature: (label, weight).
    pub weights: Vec<(&'static str, f64)>,
}

pub enum DialogAction {
    ConnectPort(String),
    /// Persist the welcome dialog's "don't show again" choice.
    SetShowWelcome(bool),
    /// Try to install a pasted supporter key. The app reports the outcome by
    /// updating or closing the dialog.
    InstallLicense {
        key: String,
    },
    ApplyColor(ColorTarget, Color32),
    /// Teach `name` for the held `notes`, transposition-invariant when
    /// `apply_all_keys` and the name begins with a note name.
    TeachSave {
        notes: Vec<u8>,
        name: String,
        apply_all_keys: bool,
    },
    /// Delete the override with this interval-set-from-bass key.
    DeleteOverride {
        intervals: Vec<u8>,
    },
    /// D-UI-9: train the re-ranker to read `notes` as `name`.
    TrainCorrection {
        notes: Vec<u8>,
        name: String,
    },
    /// D-UI-9: discard all learned weights.
    ForgetLearning,
    /// Adopt a tuning the user built by hand.
    ///
    /// It is already valid: the only path to this action is through
    /// [`confirm`], which builds it with `Tuning::custom`, so the solver's
    /// monotone invariant holds and the app does not have to check again.
    ///
    /// **It cannot be stored the way a preset is.** `Settings` keeps
    /// `fretboard_tuning` as a NAME, and `Settings::fretboard_spec` resolves
    /// that name through `Tuning::by_name`, which knows only the preset table:
    /// a custom tuning saved as a bare name comes back as Standard on the next
    /// launch, silently. The pitches have to be persisted next to the name.
    SetCustomTuning(Tuning),
    /// Use this spec for the rest of this session, without persisting it.
    ///
    /// Already valid: the only path to either of these is [`export_ready`], so
    /// `problem()` is None and the tempo mark is inside `MIN_BPM..=MAX_BPM`.
    /// The app does not have to check again.
    /// A capture device was chosen. `None` means the None row: close whatever
    /// is open and select nothing.
    ChooseDevice {
        kind: DeviceKind,
        uid: Option<String>,
    },
    /// Load a VST3 instrument, or unload the one that is loaded.
    ///
    /// The value is the BUNDLE PATH and not a name, because the path is the
    /// identity and the settings key: two vendors ship a "Piano", and a plugin
    /// renamed between versions would come back from the settings file as
    /// nothing at all.
    ///
    /// `None` is the picker's None row: unload, and play nothing.
    ///
    /// **This is the one action in the file that runs somebody else's code
    /// inside Tangent.** It is why a Full build carries
    /// `com.apple.security.cs.disable-library-validation`, and why the dialog
    /// says so before the button is clicked rather than after.
    LoadPlugin {
        path: Option<String>,
    },
    SetExport(ExportSpec),
    /// As above, and write it to the settings file so every later take starts
    /// from it. This is the "Use these settings for every take" tick, and it
    /// goes to the same place the output folder's "Default" tick does.
    SetExportAndRemember(ExportSpec),
}

// ── the custom tuning editor ────────────────────────────────────────────────
//
// A tuning is six-to-twelve numbers, so the temptation is a row of numeric
// fields and a Save button. Two things make it more than that.
//
// The first is that a guitarist thinks in note names. "The low string is E2"
// is a sentence; "the low string is 40" is a lookup. Both are offered on every
// row and they stay in step, and the SPELLING comes from `ivory_core::patterns`
// rather than a table written here, so the editor cannot start calling a note
// Eb while the piano above it calls the same note D#.
//
// The second is that one of the rules being enforced is load-bearing rather
// than cosmetic. `Tuning::custom` refuses a tuning whose strings do not ascend,
// because the voicing solver's search IS the assumption that they do; a tuning
// that dips does not produce a wrong shape, it produces a search that quietly
// stops covering real candidates. An editor that answered that with "invalid"
// would be hiding the only interesting thing it knows, so every refusal here is
// reported as a reason.

/// The editor as a window. Wide enough that "Remove low string" and "Strings:"
/// share a line, tall enough that a six-string tuning needs no scrolling.
const TUNING_DIALOG_W: f32 = 480.0;
const TUNING_DIALOG_H: f32 = 500.0;
/// The `Frame` margin on every side. Named rather than inline because the width
/// the message wraps into is the dialog's less two of these, and the test that
/// proves the message fits has to measure at exactly that width.
const TUNING_MARGIN: i8 = 12;
/// Point size of the message under the string list.
const TUNING_FOOTER_PT: f32 = 10.0;
/// Height reserved for that message, whether or not there is one.
///
/// Reserved rather than measured per frame, and deliberately generous. A footer
/// that grew as you typed would walk the button row up and down under the
/// cursor; a footer reserved too SMALL pushes the confirm button off the bottom
/// edge, which is the bug the D-UI-9 result box already had once and which only
/// Esc could get you out of. `the_longest_refusal_fits_the_space_reserved_for_it`
/// measures the real messages against this rather than trusting the arithmetic.
const TUNING_FOOTER_H: f32 = 58.0;

/// One editable string in the tuning editor.
///
/// The two fields are the same value in two notations, and the invariant is
/// that `text` is what the user last committed to: it is what [`confirm`] reads
/// and it is what is on screen. `pitch` is the last value `text` successfully
/// parsed to, which is what the number spinner drags, and the two can only
/// disagree while a half-typed name (`"E"`, `"A#"`) sits in the field. Editing
/// either one writes the other, which is the whole reason both can be offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TuningString {
    /// A note name (`E2`, `A#3`, `Db4`) or a bare MIDI number.
    pub text: String,
    /// The MIDI pitch of the open string.
    pub pitch: u8,
}

impl TuningString {
    /// A row showing `pitch`, spelled under the caller's accidental
    /// preference.
    pub fn new(pitch: u8, prefer_flats: bool) -> Self {
        Self {
            text: pitch_name(pitch, prefer_flats),
            pitch,
        }
    }
}

/// A new low string is tuned a fourth below the one above it.
///
/// Not a guess: every extended-range instrument in `TUNINGS` grows downward in
/// fourths. 7-String is Standard with a low B at 35 (a fourth under the low E
/// at 40), 8-String is that with a low F# at 30, Bass (5) is Bass (4) with a
/// low B at 23 under its low E at 28. The test
/// `adding_a_string_grows_the_low_end_like_a_seven_string` asserts the result
/// is those presets note for note.
const ADDED_STRING_DROP: u8 = 5;

/// What a first string is tuned to when there is none to reason from.
///
/// Unreachable through the menu — the editor is always seeded from a live
/// tuning — but `strings` is a `Vec` and a caller may hand over an empty one.
/// A low E is the string every guitarist would expect to appear.
const DEFAULT_OPEN_PITCH: u8 = 40;

impl Dialog {
    /// Open the tuning editor on a copy of `from`.
    ///
    /// The name is prefilled with something that is NOT the preset's own name,
    /// because saving a custom tuning called "Standard" is the trap described
    /// on [`DialogAction::SetCustomTuning`] — the app would resolve that name
    /// back to the preset and the user's pitches would be gone. Editing a
    /// tuning that is already custom keeps its name, which is what "edit"
    /// means.
    pub fn custom_tuning(from: &Tuning, prefer_flats: bool) -> Dialog {
        Dialog::CustomTuning {
            name: if from.is_preset() {
                format!("{} (custom)", from.name)
            } else {
                from.name.to_string()
            },
            strings: rows_for(&from.open, prefer_flats),
            prefer_flats,
            focus: true,
        }
    }
}

/// Spell a MIDI pitch the way the rest of the app spells it, with an octave.
///
/// The pitch-class table is `ivory_core::patterns`, the same one the chord
/// strip, the teach layer and the theory panel render from — a second table
/// here would be a second spelling convention, and the day it disagreed the
/// fretboard would name a string Eb2 while the piano lit D#.
///
/// The octave is scientific pitch notation, MIDI 60 = C4, which is what makes
/// `TUNINGS`'s Standard read back as the E2 A2 D3 G3 B3 E4 written on every
/// guitar chart (asserted in the tests, because an off-by-one octave here would
/// be invisible until a guitarist read it).
pub fn pitch_name(pitch: u8, prefer_flats: bool) -> String {
    let names = if prefer_flats {
        NOTE_NAMES_FLAT
    } else {
        NOTE_NAMES
    };
    format!("{}{}", names[(pitch % 12) as usize], pitch as i32 / 12 - 1)
}

/// Read a pitch the way a guitarist writes one.
///
/// `Err` carries the hint shown under the field: a parser that can only say
/// "no" makes the user guess which of six things was wrong, and the six things
/// are cheap to tell apart here and impossible to tell apart later.
///
/// Deliberately more forgiving than [`pitch_name`] is expressive. Sharps and
/// flats are both accepted whichever the app prefers, case does not matter, and
/// a bare MIDI number is taken as one — nobody typing a tuning off a forum post
/// should have to notice which notation it was written in. One thing it does
/// NOT do is spell by convention across an octave boundary: `Cb2` is read as
/// "the pitch class of Cb, in octave 2" rather than as B1. No instrument is
/// tuned to Cb, and the flat table has no Cb in it at all, so inventing a rule
/// for it here would only create a spelling the rest of the app cannot produce.
pub fn parse_pitch(text: &str) -> Result<u8, &'static str> {
    let s = text.trim();
    if s.is_empty() {
        return Err("Type a note name like E2, or a MIDI number.");
    }
    // A bare number is a MIDI note number. Unambiguous, because no note name is
    // all digits.
    if s.bytes().all(|b| b.is_ascii_digit()) {
        return match s.parse::<u32>() {
            Ok(n) if n <= 127 => Ok(n as u8),
            _ => Err("A MIDI note number runs from 0 to 127."),
        };
    }
    // Uppercase the LETTER, one character, not the string. `parse_root` matches
    // 'A'..'G' in upper case only, so "e2" would be thrown out — but
    // `to_uppercase()` on the whole thing turns "Bb2" into "BB2", which parses
    // as a B with no accidental and then chokes on the octave. That is the
    // entire reason this is done by index.
    let head: String = s
        .char_indices()
        .map(|(i, c)| if i == 0 { c.to_ascii_uppercase() } else { c })
        .collect();
    // The letter and accidental come from the teach layer's own parser, so this
    // field accepts exactly the note names "Teach Chord Name" accepts.
    let Some((pc, rest)) = ivory_core::overrides::parse_root(&head) else {
        return Err("A pitch starts with a note letter A to G, like E2.");
    };
    if rest.is_empty() {
        return Err("A pitch needs an octave: E2, not E.");
    }
    if matches!(rest.as_bytes().first(), Some(b'#' | b'b')) {
        return Err("One accidental only: E#2 or Eb2.");
    }
    let Ok(octave) = rest.parse::<i32>() else {
        return Err("The octave has to be a number: E2, or E-1 at the very bottom.");
    };
    // The inverse of `pitch_name`'s octave, and tested as a round trip in both
    // directions over the whole MIDI range.
    let midi = (octave + 1) * 12 + pc as i32;
    if !(0..=127).contains(&midi) {
        return Err("That pitch is outside MIDI's 0 to 127.");
    }
    Ok(midi as u8)
}

/// The highest pitch core will accept as an open string.
///
/// ASKED, not copied. The ceiling is `MAX_OPEN_PITCH`, a private constant in
/// `fretboard.rs` that exists because a 24-fret neck adds two octaves and there
/// has to be MIDI left above the nut for them. A second copy of that number
/// here would be a number that stops agreeing the day the fret count changes,
/// and it would stop agreeing silently — the spinner would offer a pitch the
/// validator refuses. Probing costs 128 calls, once, behind a `OnceLock`.
fn highest_open_pitch() -> u8 {
    static CEILING: std::sync::OnceLock<u8> = std::sync::OnceLock::new();
    *CEILING.get_or_init(|| {
        (0..=127u8)
            .rev()
            .find(|p| Tuning::custom("probe", vec![*p]).is_ok())
            .unwrap_or(0)
    })
}

/// Add a string BELOW the lowest one, tuned a fourth down.
///
/// Which end grows is the difference between "add a string" doing the obvious
/// thing and the annoying one: a 7-string guitar is a 6-string with a low B
/// added *below*, not a seventh string stapled above the top E. See
/// [`ADDED_STRING_DROP`] for the evidence that this is the instruments' rule
/// and not a preference.
fn add_low_string(strings: &mut Vec<TuningString>, prefer_flats: bool) {
    if strings.len() >= MAX_STRINGS {
        // `Tuning::custom` would refuse it, and the solver's leaf count is why
        // the cap exists at all. The button is disabled here too; this is the
        // guard that makes the function safe to call anyway.
        return;
    }
    let pitch = strings
        .first()
        // Saturating, so the new string can never be tuned ABOVE the one it is
        // added under — that would be the monotone break the solver cannot
        // survive, introduced by the button meant to be safe.
        .map_or(DEFAULT_OPEN_PITCH, |low| {
            low.pitch.saturating_sub(ADDED_STRING_DROP)
        });
    strings.insert(0, TuningString::new(pitch, prefer_flats));
}

/// Take the lowest string away again: the exact inverse of [`add_low_string`].
///
/// It has to be the same end. Press Add then Remove and you must get back what
/// you had, and the presets shed from that end too — Bass (5) without its low B
/// is Bass (4), note for note, and 8-String without its low F# is 7-String.
fn remove_low_string(strings: &mut Vec<TuningString>) {
    // One string is a legal tuning (a diddley bow, or a work in progress);
    // none is not, and `Tuning::custom` refuses it.
    if strings.len() > 1 {
        strings.remove(0);
    }
}

/// A whole set of rows for a tuning's open strings, spelled and in order.
fn rows_for(open: &[u8], prefer_flats: bool) -> Vec<TuningString> {
    open.iter()
        .map(|&p| TuningString::new(p, prefer_flats))
        .collect()
}

/// Why the editor will not build a tuning yet, in a form that can be said out
/// loud.
///
/// `Debug` for the tests' sake: a failing assertion has to be able to print the
/// refusal it did not expect.
#[derive(Debug)]
enum Problem {
    /// A row's text is not a pitch. Carries the string's number counted 1-based
    /// from the LOW end — the numbering `TuningError` itself uses, so a message
    /// from here and a message from core point at the same row.
    Unreadable {
        string: usize,
        text: String,
        hint: &'static str,
    },
    /// The name is a preset's, with different pitches behind it.
    NameClash { preset: String },
    /// `Tuning::custom` refused it.
    Refused(TuningError),
}

impl Problem {
    /// One sentence of what is wrong, then why it matters.
    ///
    /// `NotAscending` and `PitchRange` are re-worded rather than deferred to
    /// `TuningError`'s own `Display`, for one reason each: core quotes raw MIDI
    /// numbers ("string 3 is 45"), which is the wrong language to answer
    /// somebody who is looking at a field that says A2 — and `NotAscending` is
    /// the one refusal whose reason is worth more than the fact. `StringCount`
    /// and `EmptyName` already say the whole thing and mention no pitches, so
    /// they are passed through as core writes them.
    fn message(&self, prefer_flats: bool) -> String {
        match self {
            Problem::Unreadable { string, text, hint } => {
                format!("String {string}: \"{text}\" is not a pitch. {hint}")
            }
            Problem::NameClash { preset } => format!(
                "\"{preset}\" is already a preset. Tangent saves a tuning by name and looks \
                 that name back up in the preset table when it starts, so this one would \
                 return as the preset and your pitches would be gone. Give it a name of its \
                 own."
            ),
            Problem::Refused(TuningError::NotAscending {
                string,
                previous,
                found,
            }) => format!(
                "String {} ({}) sounds below string {} ({}). Strings must run low to high, \
                 and that is not tidiness: the voicing solver searches by assuming each \
                 string is higher than the last, so a tuning that dips makes it stop finding \
                 shapes your instrument can really play.",
                string + 1,
                pitch_name(*found, prefer_flats),
                string,
                pitch_name(*previous, prefer_flats),
            ),
            Problem::Refused(TuningError::PitchRange(p)) => format!(
                "{} is too high for an open string: a 24-fret neck reaches two octaves above \
                 it and there has to be MIDI left for them. {} is as high as an open string \
                 goes.",
                pitch_name(*p, prefer_flats),
                pitch_name(highest_open_pitch(), prefer_flats),
            ),
            Problem::Refused(e) => e.to_string(),
        }
    }
}

/// Build the tuning the editor is currently describing, or say why not.
///
/// **The single gate.** The confirm button is enabled on `is_ok()` and the
/// action is built from the `Ok` value, so "you cannot confirm an invalid
/// tuning" is one function rather than two conditions that can drift apart.
///
/// It reads `text`, never `pitch`: the field is what the user is looking at,
/// and a tuning built from a number they can no longer see would be a tuning
/// they did not ask for.
fn confirm(name: &str, strings: &[TuningString]) -> Result<Tuning, Problem> {
    let mut open = Vec::with_capacity(strings.len());
    for (i, row) in strings.iter().enumerate() {
        match parse_pitch(&row.text) {
            Ok(p) => open.push(p),
            Err(hint) => {
                return Err(Problem::Unreadable {
                    string: i + 1,
                    text: row.text.clone(),
                    hint,
                })
            }
        }
    }
    // Checked here rather than in core, because it is not a fact about tunings:
    // it is a fact about how THIS app stores one. See
    // `DialogAction::SetCustomTuning`. Identical pitches under a preset's name
    // are harmless — that is simply the preset — so only a real collision is
    // refused.
    if let Some(preset) = Tuning::by_name(name.trim()) {
        if preset.open.as_ref() != open.as_slice() {
            return Err(Problem::NameClash {
                preset: preset.name.to_string(),
            });
        }
    }
    Tuning::custom(name, open).map_err(Problem::Refused)
}

// ── the export dialog ───────────────────────────────────────────────────────
//
// The owner asked for "a dialog menu with a selector that can include: Camera +
// Synced sound + Tangent's Display all in one video ALONG with midi in the
// export", with the other combinations available too. So it is a real selector
// over a real spec (`recorder::ExportSpec`), and every rule about what is a
// legal spec lives THERE and is already tested. This file renders it and
// refuses to commit an invalid one; it does not get a second opinion about
// what "valid" means.
//
// It is reached from two places and they mean different things. From the
// Recorder band before a take, it chooses what that take will produce. From the
// result strip after a take, it re-runs the subset that is genuinely
// re-derivable — and that subset is much smaller than it looks, which is what
// `Rederivable` is for.

/// Whether the video encoder exists yet.
///
/// **It does, on macOS.** The compositor renders an offscreen egui pass into a
/// wgpu texture and AVAssetWriter turns it into H.264 with AAC beside it; the
/// whole path is covered by `the_whole_pipeline_writes_a_vertical_video_with_
/// sound`, which produces a file and has `ffprobe` check it.
///
/// It was a constant so that turning the section on would be ONE edit rather
/// than eleven scattered conditions, and that is how it turned out — this line
/// is the edit.
///
/// Windows and Linux have no encoder (`ivory_record::encode::stub`) and no
/// compositor, and they refuse at the moment a take starts with a message
/// saying so. That refusal is not the same as this flag: this decides whether
/// the DIALOG offers video at all, and on those platforms it still does,
/// because the alternative is a dialog that silently loses the settings a user
/// carried over from a Mac. A take says what it cannot do; a dialog should not
/// forget what was asked for.
#[cfg(target_os = "macos")]
pub const VIDEO_EXPORT_READY: bool = true;
#[cfg(not(target_os = "macos"))]
pub const VIDEO_EXPORT_READY: bool = false;

/// The dialog as a window. Wide enough for the left column and the longest
/// radio label on one line, tall enough that the §5 mock needs no scrolling
/// before a take. After one it does, because the reason the camera controls are
/// dead is worth several lines and the body scrolls rather than the buttons
/// moving.
const EXPORT_DIALOG_W: f32 = 560.0;
const EXPORT_DIALOG_H: f32 = 620.0;
/// The `Frame` margin on every side; see `TUNING_MARGIN` for why it is named.
const EXPORT_MARGIN: i8 = 12;
/// The left column. Every control in the dialog starts at the same x whichever
/// section it is in, which is what makes the §5 mock read as one form rather
/// than four stacked ones.
const EXPORT_LABEL_W: f32 = 148.0;
/// How far a sub-section's label ("Layout", "Display shows") sits in from a
/// section's ("Video"). The mock's indentation, in points.
const EXPORT_SUB_INDENT: f32 = 12.0;
/// Point size of the footer — the estimate and the refusal.
const EXPORT_FOOTER_PT: f32 = 10.0;
/// Height reserved for that footer, whether or not there is a refusal in it.
///
/// Reserved rather than measured, for the reason spelled out on
/// `TUNING_FOOTER_H`: a band that grows as you click walks the button row up
/// and down under the cursor, and a band reserved too small pushes Export off
/// the bottom edge where only Esc can reach it.
/// `the_longest_export_refusal_fits_the_space_reserved_for_it` measures the
/// real messages against this rather than trusting the arithmetic.
const EXPORT_FOOTER_H: f32 = 46.0;

/// What a take can still be turned into after it has been recorded.
///
/// **This is the one rule in the dialog that carries real information.** Camera
/// frames are composited and encoded *while the take runs* — §0 rules out
/// keeping them (one 20-minute 1080p30 take is 112 GB as raw NV12) and there is
/// no decoder anywhere in this design, so nothing can read the finished file
/// back. Therefore:
///
/// | after Stop | re-derivable? | why |
/// |---|---|---|
/// | the SMF tempo mark | yes | rewriting the tempo meta is a file edit |
/// | a display-only video, any panel selection | yes | replay the recorded `.mid` offline through the compositor; the four `draw` functions are pure painters with no time dependence |
/// | anything containing the camera | **no** | those frames were composited and encoded live |
///
/// It is a value rather than a handful of `if post_take &&` scattered through
/// the rendering code because getting it wrong offers the user an option that
/// cannot work, which is worse than not offering it at all — and because a rule
/// with one home can be tested, which is what
/// `a_take_recorded_without_a_camera_can_never_gain_one` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rederivable {
    /// The camera tick, the video modes that imply a camera, and "Match
    /// camera" — the resolution of a camera that is no longer running.
    pub camera: bool,
    /// The composite layout. Fixed after a take even when the take HAD a
    /// camera: those frames were composited in that arrangement already, so
    /// "Camera full frame" can never become "Side by side".
    pub layout: bool,
    /// The display layer and which panels are in it. Always true: this is the
    /// half that genuinely survives, because it is repainted from the MIDI.
    pub display: bool,
    /// The SMF tempo mark. Always true: it is a file edit.
    pub tempo: bool,
    /// Why something is greyed, in words the dialog prints verbatim. `None`
    /// before a take, when nothing is greyed and there is nothing to explain.
    pub why: Option<&'static str>,
}

impl Rederivable {
    /// Before a take, nothing has happened yet, so nothing is fixed.
    pub const EVERYTHING: Rederivable = Rederivable {
        camera: true,
        layout: true,
        display: true,
        tempo: true,
        why: None,
    };

    /// Bring a spec down to what this take can actually still produce.
    ///
    /// Applied once, when the dialog opens, so it can never OPEN showing a
    /// camera video it cannot make — the seed comes from the settings file or
    /// from the last take, and neither of them knows what this take caught.
    /// After that the user's own ticks stand: a spec they break themselves is
    /// refused with a reason, not silently re-ticked.
    pub fn clamp(self, mut spec: ExportSpec) -> ExportSpec {
        if !self.camera {
            spec.composite.camera = false;
            if spec.video.wants_video() {
                // With the camera layer gone, every multi-file mode collapses
                // to the one video that is still re-derivable: the display,
                // replayed offline from the recorded `.mid`. And if the
                // display is not in it either, there is no video left to make
                // at all — offering an empty composite would be offering the
                // option that cannot work.
                spec.video = if spec.composite.display && spec.composite.shows.any() {
                    VideoMode::Composite
                } else {
                    VideoMode::None
                };
            }
        }
        spec
    }
}

/// The rule, in one place. See [`Rederivable`].
pub fn rederivable(post_take: bool, had_camera: bool) -> Rederivable {
    if !post_take {
        return Rederivable::EVERYTHING;
    }
    Rederivable {
        // False either way, and the reason differs: a take with no camera has
        // no frames to composite, and a take WITH one has frames that were
        // already composited and encoded. Neither can be re-made.
        camera: false,
        layout: false,
        display: true,
        tempo: true,
        why: Some(if had_camera {
            "This take's camera was composited and encoded as it ran, in the layout chosen \
             then, and the frames were never kept — Tangent has no decoder to read them back. \
             So the camera and the layout are settled now. The tempo mark, and a display-only \
             video in any panel selection, can still be re-made from the recorded MIDI."
        } else {
            "This take was recorded without the camera, and it can never gain one: camera \
             frames are composited and encoded while the take runs and nothing keeps them. \
             The tempo mark, and a display-only video in any panel selection, can still be \
             re-made from the recorded MIDI."
        }),
    }
}

/// Seed "Display shows" from the panels the user actually has on screen.
///
/// The video's panel selection then **overrides the live settings, for the
/// video only** — that is the whole reason `DisplayShows` exists as its own set
/// of flags rather than the dialog reading `Settings` at render time. Recording
/// a clean piano-and-chord video while keeping the fretboard on screen for your
/// own use is the case it is for, and because the compositor drives the same
/// `draw` functions with its own `Settings` copy, it costs a struct field
/// rather than a code path.
pub fn shows_from_settings(s: &crate::settings::Settings) -> DisplayShows {
    DisplayShows {
        // No flags to read: the piano and the chord readout are never off.
        // They are the app.
        piano: true,
        chord: true,
        fretboard: s.show_fretboard,
        // One tick for three flags, because the theory band is one panel on
        // screen however many diagrams are inside it. Seeding from
        // `theory_circle` alone would silently drop somebody who has only the
        // Tonnetz showing — hence `Views::any`, which is the same question the
        // band itself asks.
        theory: s.theory_views().any(),
    }
}

/// Read a tempo mark the way somebody types one.
///
/// Refuses out of range rather than clamping, and that is the point:
/// `MIN_BPM..=MAX_BPM` exists because a DAW will happily import 0.001 BPM and
/// draw a bar ruler several centuries long, and a field that quietly turned a
/// typed 400 into 300 would be a field the user did not notice lying to them.
fn parse_bpm(text: &str) -> Result<f64, String> {
    let s = text.trim();
    if s.is_empty() {
        return Err("The tempo mark is a number of beats per minute, like 120.".to_owned());
    }
    let Ok(v) = s.parse::<f64>() else {
        return Err(format!("\"{s}\" is not a tempo. Type a number, like 120."));
    };
    if !v.is_finite() || !(MIN_BPM..=MAX_BPM).contains(&v) {
        return Err(format!(
            "A tempo mark runs from {MIN_BPM:.0} to {MAX_BPM:.0} BPM. It does not move a single \
             note — it decides where a DAW's bar lines land."
        ));
    }
    Ok(v)
}

/// A tempo for the field: `120`, not `120.0`, and `92.5` when it really is.
fn bpm_text(bpm: f64) -> String {
    if bpm.fract() == 0.0 {
        format!("{bpm:.0}")
    } else {
        format!("{bpm}")
    }
}

/// Build the spec the dialog is currently describing, or say why not.
///
/// **The single gate**, exactly as `confirm` is for the tuning editor: the
/// Export button is enabled on `is_ok()` and the action carries the `Ok` value,
/// so "you cannot export an invalid spec" is one function rather than two
/// conditions that drift apart.
///
/// The tempo comes from the TEXT, never from `spec.tempo_bpm`, because the text
/// is what the user is looking at. `problem()` is asked of the spec that would
/// actually be written, and its own words are used verbatim — a second wording
/// here would be a second answer to the same question.
fn export_ready(spec: &ExportSpec, tempo_text: &str) -> Result<ExportSpec, String> {
    // A tempo mark is written into the SMF and nowhere else, so with no MIDI
    // there is nothing to validate. Checking anyway would grey Export out over
    // a stale value in a field that is itself greyed, which is a dead end with
    // no way for the user to see what it wants.
    let tempo_bpm = if spec.midi {
        parse_bpm(tempo_text)?
    } else {
        spec.tempo_bpm
    };
    let out = ExportSpec { tempo_bpm, ..*spec };
    match out.problem() {
        Some(p) => Err(p.message().to_owned()),
        None => Ok(out),
    }
}

/// Which action a commit is. One function, so the tick and the consequence
/// cannot drift apart.
fn export_action(spec: ExportSpec, remember: bool) -> DialogAction {
    if remember {
        DialogAction::SetExportAndRemember(spec)
    } else {
        DialogAction::SetExport(spec)
    }
}

/// The footer's first line: what this take costs, in disk and in CPU.
///
/// A rate and an encoder count rather than a byte total, because the two
/// questions somebody actually has are "will this fill my disk" and "will this
/// stutter while I play". `megabytes_per_minute` and `encoder_count` are the
/// recorder's own arithmetic; nothing is recomputed here.
fn estimate_line(spec: &ExportSpec) -> String {
    let mb = spec.megabytes_per_minute();
    let encoders = match spec.encoder_count() {
        0 => "no video encoder".to_owned(),
        1 => "1 video encoder".to_owned(),
        n => format!("{n} video encoders"),
    };
    format!(
        "~{mb:.0} MB a minute, about {:.1} GB an hour, {encoders}.",
        mb * 60.0 / 1000.0
    )
}

/// What those encoders cost on the machine this is running on.
///
/// Three simultaneous 1080p30 encodes is nothing for VideoToolbox and Media
/// Foundation and is genuinely expensive for a software encoder, and somebody
/// about to run three of them deserves to know which one they have *before*
/// they find out during a take.
///
/// `cfg!` and not a probe: this asks about the TARGET, which is where the
/// dialog will be read, so a Windows build cross-compiled on this Mac still
/// says Media Foundation.
fn encoder_note() -> &'static str {
    if cfg!(target_os = "macos") {
        "Video encoding here runs on VideoToolbox, so three 1080p30 streams cost the CPU almost \
         nothing."
    } else if cfg!(target_os = "windows") {
        "Video encoding here runs on Media Foundation, so three 1080p30 streams cost the CPU \
         almost nothing."
    } else {
        "Video encoding here is a software encoder: every stream is real CPU, and three 1080p30 \
         streams will be felt while you play."
    }
}

/// Said out loud under the video modes while [`VIDEO_EXPORT_READY`] is false.
const VIDEO_PENDING_NOTE: &str =
    "Video export is not written yet — it arrives with the encoder. This take will write the \
     audio and the MIDI. The controls below are the finished design and are greyed until then.";

/// The dialog's left column: a label of a fixed width.
///
/// The space is ALLOCATED and the text painted into it, rather than laid out
/// from the label's own size, because egui advances a sub-layout by what the
/// child actually used — so "Video" would pull its row's controls left and the
/// straight column the mock draws would bend.
fn export_label(ui: &mut egui::Ui, t: &DialogTheme, text: &str, sub: bool) {
    let h = ui.spacing().interact_size.y;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(EXPORT_LABEL_W, h), egui::Sense::hover());
    if text.is_empty() {
        return;
    }
    let indent = if sub { EXPORT_SUB_INDENT } else { 0.0 };
    ui.painter().text(
        rect.left_center() + Vec2::new(indent, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        FontId::new(if sub { 10.0 } else { 11.0 }, fonts::courier_bold()),
        if sub {
            t.text.gamma_multiply(0.72)
        } else {
            t.text
        },
    );
}

/// One themed checkbox that may be greyed.
///
/// `add_enabled` fades the whole widget through the painter, which is what
/// makes a disabled control LOOK disabled rather than merely ignore the click —
/// and a control that ignores clicks silently is how a user concludes the app
/// is broken.
fn export_check(ui: &mut egui::Ui, t: &DialogTheme, enabled: bool, on: &mut bool, label: &str) {
    let text = RichText::new(label)
        .font(FontId::new(11.0, fonts::courier_bold()))
        .color(t.text);
    ui.add_enabled(enabled, egui::Checkbox::new(on, text));
}

/// One themed radio button that may be greyed. Sets `current` when chosen.
fn export_radio<T: PartialEq>(
    ui: &mut egui::Ui,
    t: &DialogTheme,
    enabled: bool,
    current: &mut T,
    value: T,
    label: &str,
) {
    let text = RichText::new(label)
        .font(FontId::new(11.0, fonts::courier_bold()))
        .color(t.text);
    if ui
        .add_enabled(enabled, egui::RadioButton::new(*current == value, text))
        .clicked()
    {
        *current = value;
    }
}

/// A wrapped explanatory paragraph, full width. Full width and not in the label
/// column, because a horizontal layout has infinite width and a long line in
/// one would run off the edge of the dialog instead of wrapping.
fn export_para(ui: &mut egui::Ui, t: &DialogTheme, text: &str) {
    ui.add_space(3.0);
    ui.label(
        RichText::new(text)
            .font(FontId::new(9.0, fonts::courier_bold()))
            .color(t.text.gamma_multiply(0.72)),
    );
    ui.add_space(3.0);
}

impl Dialog {
    /// Open the Export dialog on `spec`.
    ///
    /// `post_take` is the difference between the dialog's two entry points and
    /// they mean different things: from the Recorder band's `Export...` button
    /// it chooses what the NEXT take produces, and from the result strip after
    /// a take it re-runs what is genuinely re-derivable from the one just
    /// finished. `had_camera` is what that finished take actually caught.
    ///
    /// The spec is [`Rederivable::clamp`]ed on the way in, so a dialog opened
    /// after a take can never OPEN promising a camera video that cannot exist —
    /// the seed comes from the settings file or the last take, and neither of
    /// them knows what this take caught.
    ///
    /// The app opens it like this:
    ///
    /// ```ignore
    /// let mut spec = /* the remembered ExportSpec, or ExportSpec::default() */;
    /// spec.composite.shows = dialogs::shows_from_settings(&self.settings);
    /// self.dialog = Some(Dialog::export(spec, false, false));   // before a take
    /// self.dialog = Some(Dialog::export(spec, true, take.had_camera)); // after one
    /// ```
    ///
    /// and drains `DialogAction::SetExport` / `SetExportAndRemember`, the
    /// second of which also writes the spec to settings.
    pub fn export(spec: ExportSpec, post_take: bool, had_camera: bool) -> Dialog {
        let mut spec = rederivable(post_take, had_camera).clamp(spec);
        if !VIDEO_EXPORT_READY {
            // Honesty, in the same edit as the greying: while there is no
            // encoder, a remembered `video` from the settings file would
            // otherwise be committed as a promise of a file that will never
            // appear. Delete these three lines when the constant flips.
            spec.video = VideoMode::None;
        }
        Dialog::Export {
            tempo_text: bpm_text(spec.tempo_bpm),
            spec,
            remember: false,
            post_take,
            had_camera,
        }
    }
}

/// Which kind of device a [`Dialog::DevicePicker`] is choosing.
///
/// Carries its own copy through the action so the app does not have to remember
/// which picker it opened — the answer arrives saying what question it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceKind {
    Camera,
    AudioInput,
}

impl DeviceKind {
    pub fn title(self) -> &'static str {
        match self {
            DeviceKind::Camera => "Select Camera",
            DeviceKind::AudioInput => "Select Audio Input",
        }
    }

    fn prompt(self) -> &'static str {
        match self {
            DeviceKind::Camera => "Camera to record and preview:",
            DeviceKind::AudioInput => "Audio input to record:",
        }
    }

    /// What the None row says. Not just "None": what choosing it DOES is the
    /// part worth spelling out, because a recorder with no camera is a normal
    /// thing to want and a recorder with no audio input used to be a mistake.
    ///
    /// It no longer is, and this row used to say so — "record MIDI only" was
    /// true when the input device was the only thing that could drive a writer,
    /// so a loaded instrument went unrecorded and the user was told to go and
    /// choose a microphone in order to record something the microphone was not
    /// going to record. A take with no input now records the instrument on the
    /// output device's own clock.
    fn none_row(self) -> &'static str {
        match self {
            DeviceKind::Camera => "None  —  record without video",
            DeviceKind::AudioInput => "None  —  record the instrument, not an input",
        }
    }

    fn nothing_found(self) -> &'static str {
        match self {
            DeviceKind::Camera => {
                "No cameras found. If one is connected, Tangent may not have \
                 been granted camera access yet — check System Settings > \
                 Privacy & Security."
            }
            DeviceKind::AudioInput => {
                "No audio inputs found. If an interface is connected, Tangent \
                 may not have been granted microphone access yet — check \
                 System Settings > Privacy & Security."
            }
        }
    }
}

// ── the plugin picker ───────────────────────────────────────────────────────
//
// There are 112 VST3 bundles installed on the owner's machine, and the one
// decision this dialog makes is that it will not open a single one of them.
//
// A picker showing each plugin's REAL name has to ask each plugin, and asking
// means `Module::open` — which on macOS runs the bundle's `bundleEntry`, where
// plugins locate sample libraries, read preset folders and spawn threads (see
// the header of `ivory-host/src/scan.rs`). A hundred of those in a row is
// seconds of a frozen window, and it is precisely where a broken plugin takes
// the process down: before the user has chosen anything at all, from a dialog
// they opened to look. So the rows come from `ivory_host::discover()`, which is
// a `read_dir` and nothing more — paths and file names.
//
// What that costs is honest and small: the vendor column is usually empty, and
// the dialog says why rather than leaving it looking like missing data. One
// module is opened, at the moment one is loaded.
//
// Nothing here takes a rescan, a refresh or a "read them all properly" flag. An
// API that invited scanning would eventually be taken up.

/// The picker as a window. As wide as the Export dialog so the two notes at the
/// bottom wrap the same way, and tall enough that a filtered list is worth
/// looking at.
const PLUGIN_DIALOG_W: f32 = 560.0;
const PLUGIN_DIALOG_H: f32 = 460.0;
/// The `Frame` margin on every side; see `TUNING_MARGIN` for why it is named.
const PLUGIN_MARGIN: i8 = 12;
/// Point size of the two notes under the list.
const PLUGIN_NOTE_PT: f32 = 9.0;
/// Height reserved for those notes, whether or not the list is empty.
///
/// Reserved rather than measured, for the reason spelled out on
/// `TUNING_FOOTER_H`: a band that changed size would walk the button row under
/// the cursor, and a band reserved too small pushes Load off the bottom edge
/// where only Esc can reach it. The test
/// `the_notes_under_the_list_fit_the_space_reserved_for_them` measures the real
/// strings against this rather than trusting the arithmetic.
const PLUGIN_FOOTER_H: f32 = 60.0;

/// What the None row says. Not just "None": what choosing it DOES is the part
/// worth spelling out, exactly as on [`DeviceKind::none_row`].
const PLUGIN_NONE_ROW: &str = "None  —  no instrument; unloads the one that is loaded";

/// Said under the list, every time, in full-strength text rather than the faint
/// grey the other note gets.
///
/// A plugin is not a document Tangent reads: it is code, in this process, with
/// this process's file access and this process's crash. Somebody deserves to
/// know that before they click Load rather than after their first bad plugin,
/// and the sentence is short enough that they will actually read it.
const PLUGIN_TRUST_NOTE: &str =
    "A plugin is somebody else's code, running inside Tangent. It shares this process, so one that \
     crashes takes Tangent with it — which is also why this build ships with macOS library \
     validation disabled. Load ones you trust.";

/// Said under that: why the vendor column is mostly blank.
///
/// Without it an empty vendor reads as a bug. With it, it reads as the
/// deliberate choice described in this section's header.
const PLUGIN_SCAN_NOTE: &str =
    "Names come from the files on disk. Tangent opens a plugin, and asks it what it really is, only \
     when it loads it — asking all of them would mean starting all of them.";

/// One installed VST3, as a directory listing knows it.
///
/// Public because the fields are the whole contract with the app: the caller
/// builds these from `ivory_host::discover()`, and a caller that already knows
/// a vendor (because it has loaded that bundle before) may fill it in. What it
/// must never do is go and find out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginEntry {
    /// Absolute path to the `.vst3` bundle. This is the identity and the
    /// settings key — a display name is not unique and changes with the
    /// vendor's whims.
    pub path: String,
    /// What to show: the bundle's file stem, cleaned up.
    pub name: String,
    /// Vendor, when known. Usually empty, and that is not a gap to be filled:
    /// filling it requires OPENING the module. See the section header.
    pub vendor: String,
}

impl PluginEntry {
    /// Read a row off a bundle path, opening nothing.
    ///
    /// The stem, because a `.vst3` is a DIRECTORY and its name is the only
    /// thing on disk that reads like a product ("Pianoteq 8.vst3"). A bundle
    /// whose path has no stem at all — a trailing `..`, a caller's mistake —
    /// falls back to the whole path rather than an unclickable-looking blank
    /// row.
    pub fn from_bundle(path: &Path) -> PluginEntry {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().trim().to_owned())
            .filter(|s| !s.is_empty());
        let full = path.to_string_lossy().into_owned();
        PluginEntry {
            name: stem.unwrap_or_else(|| full.clone()),
            path: full,
            vendor: String::new(),
        }
    }
}

/// Whether a row survives the filter box.
///
/// Case-insensitive, and on the vendor as well as the name, because "Arturia"
/// is how you look for the eleven of them at once and "arturia" is how it gets
/// typed. An empty filter matches everything, so the list needs no special case
/// for the state it opens in.
fn plugin_matches(entry: &PluginEntry, filter: &str) -> bool {
    let needle = filter.trim().to_lowercase();
    needle.is_empty()
        || entry.name.to_lowercase().contains(&needle)
        || entry.vendor.to_lowercase().contains(&needle)
}

/// The rows the filter leaves on screen, as indices into `plugins`.
///
/// Indices and not references, because `selected` is an index and the two have
/// to be comparable. Both the list and [`plugin_choice`] are built from this
/// one function, so what is on screen and what OK commits cannot disagree —
/// which is the whole of the index-into-a-filtered-list bug.
fn visible_plugins(plugins: &[PluginEntry], filter: &str) -> Vec<usize> {
    plugins
        .iter()
        .enumerate()
        .filter(|(_, p)| plugin_matches(p, filter))
        .map(|(i, _)| i)
        .collect()
}

/// What Load would do, when it would do anything.
#[derive(Debug, PartialEq, Eq)]
enum PluginChoice<'a> {
    Load(&'a PluginEntry),
    /// The None row: unload whatever is loaded and play nothing.
    Unload,
}

impl PluginChoice<'_> {
    /// The action, built from the entry — so the PATH travels and the name
    /// stays on screen where it belongs.
    fn action(&self) -> DialogAction {
        DialogAction::LoadPlugin {
            path: match self {
                PluginChoice::Load(e) => Some(e.path.clone()),
                PluginChoice::Unload => None,
            },
        }
    }
}

/// What the picker is currently offering to do, or nothing.
///
/// **The single gate**, exactly as `confirm` is for the tuning editor and
/// `export_ready` is for Export: the Load button is enabled on `is_some()` and
/// the action is built from the same value, so "you cannot load a row you
/// cannot see" is one function rather than two conditions that drift apart.
///
/// Three rules, in this order:
///
/// 1. The highlighted entry, **if the filter still shows it**. Select Pianoteq,
///    then type "serum", and Pianoteq is not what the user is looking at any
///    more; committing it would be the classic filtered-list bug wearing a
///    different hat.
/// 2. Otherwise, the only entry the filter left — the reason typing four
///    letters and pressing Enter works, which with 112 plugins is how the
///    dialog is actually used. It needs a NON-EMPTY filter: on a machine with
///    exactly one plugin installed, an empty filter also leaves exactly one
///    entry, and that rule would make the None row unreachable.
/// 3. Otherwise the None row (`selected` is nothing), and only when there is
///    something to unload. Unloading nothing is not an action, which is what
///    greys Load out on a picker opened with no instrument running.
fn plugin_choice<'a>(
    plugins: &'a [PluginEntry],
    selected: Option<usize>,
    current: Option<&str>,
    filter: &str,
) -> Option<PluginChoice<'a>> {
    let visible = visible_plugins(plugins, filter);
    if let Some(i) = selected {
        if visible.contains(&i) {
            // Resolved through `get` rather than indexed. The list is rebuilt
            // from disk every time the dialog opens, and a stale index must
            // select nothing rather than panic.
            return plugins.get(i).map(PluginChoice::Load);
        }
    }
    if !filter.trim().is_empty() {
        if let [only] = visible[..] {
            return plugins.get(only).map(PluginChoice::Load);
        }
    }
    if selected.is_none() && current.is_some() {
        return Some(PluginChoice::Unload);
    }
    None
}

/// One row's text: a mark for the plugin that is loaded, the name, and the
/// vendor when there is one.
///
/// The mark LEADS. It trails on [`Dialog::DevicePicker`], where there are four
/// devices and you can see them all; ninety rows down a list of 112 a trailing
/// bullet is invisible, and the dialog's font is monospace, so a one-character
/// gutter lines every row up into a column you can run an eye down.
fn plugin_row(entry: &PluginEntry, current: Option<&str>) -> String {
    let mark = if current == Some(entry.path.as_str()) {
        '\u{2022}'
    } else {
        ' '
    };
    if entry.vendor.trim().is_empty() {
        format!("{mark} {}", entry.name)
    } else {
        format!("{mark} {}   ({})", entry.name, entry.vendor)
    }
}

/// Where VST3s live on this platform, said out loud when none were found.
///
/// An empty box teaches nothing, and an empty list has an ordinary cause on
/// every platform: the installer offered AU or VST2 and the user took it. So
/// the dialog names the directories it looked in.
///
/// `cfg!` and not a probe, exactly as [`encoder_note`] does it: this asks about
/// the TARGET, which is the machine the dialog will be read on, so a Windows
/// build cross-compiled on this Mac still names the Windows folder.
fn vst3_locations() -> &'static str {
    if cfg!(target_os = "macos") {
        "No VST3 instruments found. macOS keeps them in ~/Library/Audio/Plug-Ins/VST3 for one user \
         and /Library/Audio/Plug-Ins/VST3 for everybody. An installer that offered you AU or VST2 \
         and took the default put nothing in either."
    } else if cfg!(target_os = "windows") {
        "No VST3 instruments found. Windows keeps them in %COMMONPROGRAMFILES%\\VST3 — usually \
         C:\\Program Files\\Common Files\\VST3. An installer that offered you VST2 and took the \
         default put nothing there."
    } else {
        "No VST3 instruments found. Linux keeps them in ~/.vst3 for one user and /usr/lib/vst3 for \
         everybody."
    }
}

impl Dialog {
    /// Open the picker on the bundles the caller found.
    ///
    /// `bundles` is `ivory_host::discover()`'s output verbatim — a directory
    /// listing, which is the whole point (see the section header above). This
    /// crate cannot depend on `ivory-host` and does not want to: paths in,
    /// a path out.
    ///
    /// The app opens it like this:
    ///
    /// ```ignore
    /// self.dialog = Some(Dialog::plugin_picker(
    ///     &ivory_host::discover(),
    ///     self.settings.plugin_path.clone(),   // what is loaded now, or None
    /// ));
    /// ```
    ///
    /// and drains `DialogAction::LoadPlugin { path }`, where `None` means
    /// unload.
    ///
    /// Two things happen on the way in. The rows are sorted the way a person
    /// reads a list rather than the way `discover` returns them — it sorts
    /// `PathBuf`s, so the system folder lands before the user's and "Zebra"
    /// before "arturia", which in a list of 112 is indistinguishable from
    /// unsorted. And the loaded plugin is preselected, so the dialog opens on
    /// the answer to "what am I playing through", not on row one.
    pub fn plugin_picker(bundles: &[PathBuf], current: Option<String>) -> Dialog {
        let mut plugins: Vec<PluginEntry> = bundles
            .iter()
            .map(|p| PluginEntry::from_bundle(p))
            .collect();
        plugins.sort_by(|a, b| {
            a.name
                .to_lowercase()
                .cmp(&b.name.to_lowercase())
                // Two bundles can share a name — the user's copy and the system
                // one — and a comparator that called them equal would order
                // them differently on different runs.
                .then_with(|| a.path.cmp(&b.path))
        });
        let selected = current
            .as_deref()
            .and_then(|c| plugins.iter().position(|p| p.path == c));
        Dialog::PluginPicker {
            plugins,
            selected,
            current,
            filter: String::new(),
            focus: true,
        }
    }
}

/// Stable surface identity: a viewport id on the desktop, an `Area` id in a
/// plugin. `dialog: Option<Dialog>` means there is never more than one open, so
/// one id serves them all and the window keeps its place between dialogs.
const DIALOG_ID: &str = "ivory-dialog";

pub fn color_pick_title(target: ColorTarget) -> &'static str {
    match target {
        ColorTarget::WhiteIdle => "Choose White Key Color",
        ColorTarget::BlackIdle => "Choose Black Key Color",
        ColorTarget::Active => "Choose Active Key Color",
        ColorTarget::Sustain => "Choose Sustain Pedal Color",
        ColorTarget::ChordText => "Choose Chord Color",
    }
}

/// Theme palette shared by About and the color modals (spec §7.3 / D-UI-7).
struct DialogTheme {
    bg: Color32,
    text: Color32,
    button_bg: Color32,
    button_hover: Color32,
    button_border: Color32,
}

fn theme(dark: bool) -> DialogTheme {
    if dark {
        DialogTheme {
            bg: Color32::from_rgb(0x00, 0x00, 0x00),
            text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            button_bg: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            button_hover: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            button_border: Color32::from_rgb(0xE8, 0xDC, 0xC0),
        }
    } else {
        DialogTheme {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            text: Color32::from_rgb(0x00, 0x00, 0x00),
            button_bg: Color32::from_rgb(0xd4, 0xc8, 0xb0),
            button_hover: Color32::from_rgb(0xc0, 0xb4, 0x9c),
            button_border: Color32::from_rgb(0x00, 0x00, 0x00),
        }
    }
}

fn apply_theme(style: &mut egui::Style, t: &DialogTheme) {
    style.spacing.button_padding = egui::vec2(12.0, 4.0); // Qt: padding 4px 12px
    let set = |wv: &mut egui::style::WidgetVisuals, bg: Color32| {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0_f32, t.button_border);
        wv.fg_stroke = Stroke::new(1.0_f32, t.text);
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
    };
    set(&mut style.visuals.widgets.inactive, t.button_bg);
    set(&mut style.visuals.widgets.hovered, t.button_hover);
    set(&mut style.visuals.widgets.active, t.button_hover);
    set(&mut style.visuals.widgets.open, t.button_hover);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, t.text);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, t.text);
    style.visuals.override_text_color = None;
    style.visuals.window_fill = t.bg;
    style.visuals.selection.bg_fill = t.button_hover;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, t.text);
    style.visuals.hyperlink_color = t.text;
}

/// Stock egui styling for the MIDI dialogs, which Qt showed with the platform
/// default look, NOT the app theme (spec §7.1).
fn stock_style(ctx: &egui::Context) -> egui::Style {
    egui::Style {
        visuals: match ctx.input(|i| i.raw.system_theme) {
            Some(egui::Theme::Dark) => egui::Visuals::dark(),
            _ => egui::Visuals::light(),
        },
        ..Default::default()
    }
}

struct VpResult {
    close: bool,
}

/// Focus a text field and select everything in it, once.
///
/// The point of the shortcut that opens these dialogs is that you can start
/// typing. Without this you land on a dialog whose field is not focused, so
/// the first thing you do is click it — and even then the caret sits at one
/// end and the prefilled name has to be cleared by hand. Selecting the whole
/// value means the first keystroke replaces it, which is what every rename
/// field on every platform does.
fn focus_and_select_all(ui: &egui::Ui, resp: &egui::Response, text: &str) {
    resp.request_focus();
    if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), resp.id) {
        let all = egui::text_selection::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(text.chars().count()),
        );
        state.cursor.set_char_range(Some(all));
        state.store(ui.ctx(), resp.id);
    }
}

/// Where child windows open. A viewport can only see its own geometry, so the
/// main window's rect has to be handed in from the app rather than read here.
#[derive(Clone, Copy, Default)]
pub struct Placement {
    /// The main window's inner rect, in monitor coordinates.
    pub parent: Option<Rect>,
    pub monitor: Option<Vec2>,
    /// What the host allows. `child_windows` decides whether a dialog is an OS
    /// window or is drawn in the canvas.
    pub caps: crate::host::Caps,
}

impl Placement {
    /// Centred on the parent, kept fully on the monitor. None when the parent
    /// geometry is not known yet, in which case the OS places the window, which
    /// on Windows means the top-left corner of the screen.
    ///
    /// "Not known yet" has to mean exactly that. A viewport cannot see its own
    /// position on the first frames, so the app used to hand over a rect at
    /// (0, 0) rather than nothing — and the welcome dialog, which opens on the
    /// very first frame, centred itself on the top-left of the SCREEN. It was
    /// centred, on the wrong thing.
    fn position_for(self, size: Vec2) -> Option<Pos2> {
        let parent = self.parent?;
        Some(crate::settings::clamp_to_monitor(
            parent.center() - size * 0.5,
            size,
            self.monitor,
        ))
    }
}

fn show_dialog_viewport(
    ctx: &egui::Context,
    placement: Placement,
    title: &str,
    size: Vec2,
    min_size: Vec2,
    content: impl FnMut(&mut egui::Ui, &mut VpResult),
) -> VpResult {
    let mut content = content;
    let mut result = VpResult { close: false };

    // Window or layer — `shell::surface` decides, and it is the only thing in
    // this crate that does. Everything below is what a dialog adds on top:
    // rounded fill on the inline path, because a layer has no title bar and no
    // window edge to tell it apart from the app behind it.
    let inline = !placement.caps.child_windows;
    let spec = crate::shell::SurfaceSpec {
        id: DIALOG_ID,
        title,
        size,
        min_size,
        // These dialogs are MODAL — `handle_main_interaction` drops every event
        // while one is open — so a dialog that ends up behind the main window
        // is not merely untidy, it is an app that has stopped responding with
        // no visible reason why. The welcome note and the supporter key are the
        // two that open on top of a window the user is already looking at.
        pos: placement.position_for(size),
        decorated: true,
        resizable: true,
        takes_focus: true,
        modal: true,
        ..Default::default()
    };
    let report = crate::shell::surface(ctx, placement.caps, &spec, &mut |ui, want_close| {
        if inline {
            ui.painter()
                .rect_filled(ui.max_rect(), 4.0, ui.style().visuals.window_fill);
        }
        let mut r = VpResult { close: false };
        content(ui, &mut r);
        if r.close {
            *want_close = true;
        }
    });
    result.close = report.close;
    result
}

/// Render the active dialog. Returns an action for the app to apply;
/// `dialog_opt` becomes None when the dialog closes.
pub fn show(
    ctx: &egui::Context,
    dialog_opt: &mut Option<Dialog>,
    dark_mode: bool,
    placement: Placement,
) -> Option<DialogAction> {
    let dialog = dialog_opt.as_mut()?;
    let mut action = None;

    let result = match dialog {
        Dialog::DevicePicker {
            kind,
            devices,
            selected,
            current,
        } => {
            let kind = *kind;
            let stock = stock_style(ctx);
            show_dialog_viewport(
                ctx,
                placement,
                kind.title(),
                Vec2::new(420.0, 320.0),
                Vec2::new(360.0, 200.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label(kind.prompt());
                            if devices.is_empty() {
                                // Not an empty list with an OK button. On macOS
                                // an empty world is what a missing entitlement
                                // looks like, and on every platform it is what
                                // an unplugged interface looks like — either
                                // way a silent empty box teaches nothing.
                                ui.add_space(8.0);
                                ui.label(kind.nothing_found());
                            }
                            ui.add_space(4.0);
                            let bottom_h = 34.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    // The None row first, and always present.
                                    if ui
                                        .selectable_label(selected.is_none(), kind.none_row())
                                        .clicked()
                                    {
                                        *selected = None;
                                    }
                                    for (i, d) in devices.iter().enumerate() {
                                        // The default is MARKED, not
                                        // preselected: "System Default" has to
                                        // stay a visible choice, or somebody
                                        // with two interfaces cannot tell which
                                        // one they are about to get.
                                        let label = if d.default {
                                            format!("{}   (system default)", d.name)
                                        } else {
                                            d.name.clone()
                                        };
                                        let live = current.as_deref() == Some(d.uid.as_str());
                                        let label = if live {
                                            format!("{label}  \u{2022}")
                                        } else {
                                            label
                                        };
                                        if ui.selectable_label(*selected == Some(i), label).clicked()
                                        {
                                            *selected = Some(i);
                                        }
                                    }
                                });
                            ui.add_space(6.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    // An out-of-range index cannot happen from
                                    // the UI, but the list is rebuilt from live
                                    // hardware every time this opens — so it is
                                    // resolved through `get` rather than
                                    // indexed, and a device unplugged while the
                                    // dialog was open selects nothing instead
                                    // of panicking.
                                    let uid = selected
                                        .and_then(|i| devices.get(i))
                                        .map(|d| d.uid.clone());
                                    action = Some(DialogAction::ChooseDevice { kind, uid });
                                    result.close = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::PluginPicker {
            plugins,
            selected,
            current,
            filter,
            focus,
        } => {
            // Themed, unlike the two pickers above it. Those are stock because
            // Qt showed the MIDI dialogs that way (spec §7.1) and the recorder's
            // device picker is their twin; this one has a live text field and
            // two paragraphs of the app's own voice in it, and reads as part of
            // Tangent rather than as a system sheet.
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                placement,
                "Load Instrument",
                Vec2::new(PLUGIN_DIALOG_W, PLUGIN_DIALOG_H),
                Vec2::new(460.0, 300.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    // The filter field paints its interior on `extreme_bg_color`
                    // — stock near-black otherwise, a hole punched through a
                    // themed dialog. Same trap as the tuning editor.
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    let faint = t.text.gamma_multiply(0.72);
                    // Taken before the field consumes it: the same first frame
                    // that focuses the filter scrolls the loaded plugin into
                    // view, which is what makes marking it worth anything when
                    // it is ninety rows down.
                    let first_frame = *focus;
                    // Read here, not from `ctx`: on the desktop this dialog is
                    // its own viewport with its own input, and the parent
                    // context never sees the key at all. A singleline `TextEdit`
                    // surrenders focus on Enter but does not consume the event
                    // (`filtered_events` copies), so one read serves the field
                    // and the list both.
                    //
                    // Never on the opening frame. Inline there is one context
                    // and one event queue, so the keystroke that opened the
                    // picker is still in it — and the dialog would confirm
                    // itself before it was ever seen.
                    let enter = !first_frame && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let mut confirm_now = false;
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(PLUGIN_MARGIN))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Filter:").font(bold(11.0)).color(t.text));
                                let resp = ui.add(
                                    egui::TextEdit::singleline(filter)
                                        .font(bold(12.0))
                                        .text_color(t.text)
                                        .desired_width(f32::INFINITY),
                                );
                                if *focus {
                                    *focus = false;
                                    focus_and_select_all(ui, &resp, filter);
                                }
                            });

                            let visible = visible_plugins(plugins, filter);
                            if plugins.is_empty() {
                                // Not an empty box with a greyed button: an
                                // empty list cannot be told apart from a picker
                                // that failed, and the ordinary cause is an
                                // installer that offered AU or VST2 and was
                                // taken up on it.
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(vst3_locations())
                                        .font(bold(11.0))
                                        .color(t.text),
                                );
                                ui.add_space(4.0);
                            } else {
                                ui.label(
                                    RichText::new(if filter.trim().is_empty() {
                                        format!("{} installed", plugins.len())
                                    } else {
                                        format!("{} of {}", visible.len(), plugins.len())
                                    })
                                    .font(bold(PLUGIN_NOTE_PT))
                                    .color(faint),
                                );
                                ui.add_space(2.0);
                            }
                            // The two notes, the button row, and the space
                            // between them. See `PLUGIN_FOOTER_H` for why the
                            // band is a reservation and not a measurement.
                            let bottom_h = PLUGIN_FOOTER_H + 34.0 + 8.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    // The None row first, always present, never
                                    // filtered, and there even when nothing is
                                    // installed: it is not an entry, and
                                    // unloading has to stay reachable from a
                                    // picker whose plugin folder has moved since
                                    // the plugin was loaded.
                                    if ui
                                        .selectable_label(
                                            selected.is_none(),
                                            RichText::new(format!("  {PLUGIN_NONE_ROW}"))
                                                .font(bold(12.0))
                                                .color(t.text),
                                        )
                                        .clicked()
                                    {
                                        *selected = None;
                                    }
                                    for &i in &visible {
                                        let Some(entry) = plugins.get(i) else {
                                            continue;
                                        };
                                        let resp = ui.selectable_label(
                                            *selected == Some(i),
                                            RichText::new(plugin_row(entry, current.as_deref()))
                                                .font(bold(12.0))
                                                .color(t.text),
                                        );
                                        // `i`, the index into `plugins` — never
                                        // the position in `visible`, which
                                        // changes under the user with every
                                        // keystroke.
                                        if resp.clicked() {
                                            *selected = Some(i);
                                        }
                                        if resp.double_clicked() {
                                            *selected = Some(i);
                                            confirm_now = true;
                                        }
                                        if first_frame && *selected == Some(i) {
                                            // The mark is worth nothing ninety
                                            // rows down a list nobody has
                                            // scrolled.
                                            resp.scroll_to_me(Some(Align::Center));
                                        }
                                    }
                                    if visible.is_empty() && !plugins.is_empty() {
                                        ui.add_space(6.0);
                                        ui.label(
                                            RichText::new(
                                                "Nothing matches that. Clear the filter to see \
                                                 them all.",
                                            )
                                            .font(bold(11.0))
                                            .color(t.text),
                                        );
                                    }
                                });

                            // ── what this costs you ─────────────────────────
                            //
                            // Full strength for the warning and faint for the
                            // explanation, because one of them is a thing the
                            // user has to decide about and the other is a thing
                            // they were about to wonder.
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(PLUGIN_TRUST_NOTE)
                                    .font(bold(PLUGIN_NOTE_PT))
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(PLUGIN_SCAN_NOTE)
                                    .font(bold(PLUGIN_NOTE_PT))
                                    .color(faint),
                            );

                            // Computed AFTER the list, so it answers this
                            // frame's click rather than the one before it.
                            let choice =
                                plugin_choice(plugins, *selected, current.as_deref(), filter);
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let load = ui.add_enabled(
                                    choice.is_some(),
                                    Button::new(RichText::new("Load").color(t.text)),
                                );
                                let cancel =
                                    ui.add(Button::new(RichText::new("Cancel").color(t.text)));
                                // Cancel is asked FIRST, and that is not tidiness:
                                // egui activates a focused button with Enter, so
                                // a user who tabbed to Cancel and pressed it
                                // would otherwise trip the global Enter below and
                                // load a plugin by pressing the button that says
                                // don't.
                                if cancel.clicked() {
                                    result.close = true;
                                } else if load.clicked() || confirm_now || enter {
                                    // Enter, a double-click and the button all
                                    // take the value the gate offered, which is
                                    // the point of there being one gate: they
                                    // cannot come to mean different things.
                                    // Escape is the funnel's job.
                                    if let Some(c) = &choice {
                                        action = Some(c.action());
                                        result.close = true;
                                    }
                                }
                            });
                        });
                },
            )
        }

        Dialog::MidiPicker {
            ports,
            selected,
            current,
        } => {
            let stock = stock_style(ctx);
            show_dialog_viewport(
                ctx,
                placement,
                "Select MIDI Input",
                Vec2::new(400.0, 300.0),
                Vec2::new(400.0, 200.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label("Select MIDI input port:");
                            if let Some(current) = current.as_deref() {
                                ui.label(format!("Current: {current}"));
                            }
                            ui.add_space(4.0);
                            let bottom_h = 34.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for (i, name) in ports.iter().enumerate() {
                                        if ui.selectable_label(*selected == Some(i), name).clicked()
                                        {
                                            *selected = Some(i);
                                        }
                                    }
                                });
                            ui.add_space(6.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    if let Some(i) = *selected {
                                        if let Some(name) = ports.get(i) {
                                            action = Some(DialogAction::ConnectPort(name.clone()));
                                        }
                                    }
                                    result.close = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::NoMidiInput => {
            let stock = stock_style(ctx);
            show_dialog_viewport(
                ctx,
                placement,
                "No MIDI Input",
                Vec2::new(420.0, 170.0),
                Vec2::new(320.0, 140.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                "No MIDI input ports found!\n\nYou can still use keytoggle mode by enabling it in the context menu.",
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::MidiError { message } => {
            let stock = stock_style(ctx);
            let text =
                format!("Error opening MIDI port:\n{message}\n\nYou can still use keytoggle mode.");
            show_dialog_viewport(
                ctx,
                placement,
                "MIDI Error",
                Vec2::new(420.0, 170.0),
                Vec2::new(320.0, 140.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(text.as_str());
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::About => {
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                placement,
                "About Tangent",
                Vec2::new(400.0, 190.0),
                Vec2::new(400.0, 150.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(9))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Tangent").font(bold(16.0)).color(t.text),
                                );
                                ui.label(
                                    RichText::new(
                                        "Simple MIDI Keyboard Monitor with Advanced Chord Detection",
                                    )
                                    .font(bold(10.0))
                                    .color(t.text),
                                );
                                // The author's own site. It was still the old
                                // shambhaline address, carried through the port
                                // from the Python app's spec (docs/spec/
                                // ui-spec.md 244) and never revisited.
                                ui.hyperlink_to(
                                    RichText::new("ganten.neocities.org")
                                        .font(bold(10.0))
                                        .color(t.text)
                                        .underline(),
                                    "https://ganten.neocities.org",
                                );
                            });
                            // Stretch, then bottom block.
                            let bottom_h = 62.0;
                            ui.add_space((ui.available_height() - bottom_h).max(0.0));
                            ui.label(
                                RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION")))
                                    .font(bold(8.0))
                                    .color(t.text),
                            );
                            // D-UI-6: one extra 8pt credit line under the version.
                            ui.label(
                                RichText::new(
                                    "Courier Prime © The Courier Prime Project Authors, SIL OFL 1.1",
                                )
                                .font(bold(8.0))
                                .color(t.text),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::ColorPick { target, color } => {
            let t = theme(dark_mode);
            let target = *target;
            show_dialog_viewport(
                ctx,
                placement,
                color_pick_title(target),
                Vec2::new(320.0, 400.0),
                Vec2::new(280.0, 320.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(9))
                        .show(ui, |ui| {
                            egui::color_picker::color_picker_color32(
                                ui,
                                color,
                                egui::color_picker::Alpha::Opaque,
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    // OK applies + saves; Cancel/close is a no-op (D-UI-7).
                                    action = Some(DialogAction::ApplyColor(target, *color));
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::Welcome { dont_show_again } => {
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                placement,
                "Welcome to Tangent",
                Vec2::new(470.0, 300.0),
                Vec2::new(410.0, 260.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            for line in [
                                "Tangent is free, and it stays free.",
                                "",
                                "It is made for the love of it. There are no",
                                "locked features, no trial, no account, and",
                                "nothing is sent anywhere.",
                                "",
                                "If it earns a place in your setup, a donation",
                                "is welcome and genuinely helps — but it buys",
                                "you nothing you do not already have.",
                                "",
                                "Everything lives in the right-click menu.",
                            ] {
                                ui.label(RichText::new(line).font(bold(12.0)).color(t.text));
                            }
                            ui.add_space(8.0);
                            let mut hide = *dont_show_again;
                            if ui
                                .checkbox(
                                    &mut hide,
                                    RichText::new("Don't show this again")
                                        .font(bold(11.0))
                                        .color(t.text),
                                )
                                .changed()
                            {
                                *dont_show_again = hide;
                                action = Some(DialogAction::SetShowWelcome(!hide));
                            }
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("Start playing").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::SupporterKey {
            input,
            message,
            installed_as,
            focus,
        } => {
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                placement,
                "Supporter Key",
                Vec2::new(460.0, 290.0),
                Vec2::new(400.0, 250.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            if let Some(name) = installed_as.as_deref() {
                                ui.label(
                                    RichText::new(format!("Supporter: {name}"))
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Tangent is free, and it stays free.")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                                ui.label(
                                    RichText::new("A key is a thank-you, nothing more.")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            }
                            ui.add_space(8.0);
                            ui.label(
                                RichText::new("Paste your key:")
                                    .font(bold(11.0))
                                    .color(t.text),
                            );
                            let edit = egui::TextEdit::multiline(input)
                                .font(bold(11.0))
                                .text_color(t.text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(5);
                            let resp = ui.add(edit);
                            if *focus {
                                *focus = false;
                                focus_and_select_all(ui, &resp, input);
                            }
                            if let Some(msg) = message.as_deref() {
                                ui.add_space(4.0);
                                ui.label(RichText::new(msg).font(bold(11.0)).color(t.text));
                            }
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ok = !input.trim().is_empty();
                                if ui
                                    .add_enabled(
                                        ok,
                                        Button::new(RichText::new("Activate").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    action =
                                        Some(DialogAction::InstallLicense { key: input.clone() });
                                    // NOT closed here: the app decides, so a bad
                                    // key can report why without losing the paste.
                                }
                                if ui
                                    .add(Button::new(RichText::new("Close").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::TeachChord {
            notes,
            note_names,
            current_label,
            input,
            apply_all_keys,
            focus,
        } => {
            let t = theme(dark_mode);
            let notes = notes.clone();
            show_dialog_viewport(
                ctx,
                placement,
                "Teach Chord Name",
                Vec2::new(420.0, 240.0),
                Vec2::new(360.0, 200.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Notes: {note_names}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(format!("Detected: {current_label}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space(6.0);
                            ui.label(RichText::new("Name:").font(bold(11.0)).color(t.text));
                            let edit = egui::TextEdit::singleline(input)
                                .font(bold(12.0))
                                .text_color(t.text)
                                .desired_width(f32::INFINITY);
                            let resp = ui.add(edit);
                            if *focus {
                                *focus = false;
                                focus_and_select_all(ui, &resp, input);
                            }
                            ui.add_space(6.0);
                            let mut apply = *apply_all_keys;
                            if ui
                                .checkbox(
                                    &mut apply,
                                    RichText::new("Apply in all keys")
                                        .font(bold(11.0))
                                        .color(t.text),
                                )
                                .changed()
                            {
                                *apply_all_keys = apply;
                            }
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ok_enabled = !input.trim().is_empty();
                                if ui
                                    .add_enabled(
                                        ok_enabled,
                                        Button::new(RichText::new("OK").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    action = Some(DialogAction::TeachSave {
                                        notes: notes.clone(),
                                        name: input.trim().to_owned(),
                                        apply_all_keys: *apply_all_keys,
                                    });
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::CorrectChord {
            notes,
            note_names,
            current_label,
            candidates,
            selected,
        } => {
            let t = theme(dark_mode);
            let notes = notes.clone();
            show_dialog_viewport(
                ctx,
                placement,
                "Correct Chord Name",
                Vec2::new(470.0, 370.0),
                Vec2::new(400.0, 290.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Notes: {note_names}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(format!("Now reads: {current_label}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Which reading would you rather see?")
                                    .font(bold(11.0))
                                    .color(t.text),
                            );
                            ui.add_space(2.0);
                            // Explain the list's limits up front — these are the
                            // only names the re-ranker can be trained toward.
                            // 4 explanatory lines at 9pt + spacing + buttons.
                            let bottom_h = 90.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for (i, (name, score)) in candidates.iter().enumerate() {
                                        let is_current = current_label == name
                                            || current_label
                                                .strip_prefix(name.as_str())
                                                .is_some_and(|r| r.starts_with('/'));
                                        let text = if is_current {
                                            format!("{name}   (current)")
                                        } else {
                                            name.clone()
                                        };
                                        ui.horizontal(|ui| {
                                            if ui
                                                .selectable_label(
                                                    *selected == Some(i),
                                                    RichText::new(text)
                                                        .font(bold(12.0))
                                                        .color(t.text),
                                                )
                                                .clicked()
                                            {
                                                *selected = Some(i);
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(format!("{score:.0}"))
                                                            .font(bold(10.0))
                                                            .color(t.text.gamma_multiply(0.55)),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                });
                            ui.add_space(4.0);
                            // Measured, not hand-waved: see
                            // ivory-core/tests/blast_radius.rs.
                            ui.label(
                                RichText::new(
                                    "Tangent learns a general leaning, not this one chord.\n\
                                     One correction changes about 1 chord in 10 overall,\n\
                                     often in unrelated keys. \"Forget Learning\" in Manage\n\
                                     Taught Chords undoes all of it exactly.",
                                )
                                .font(bold(9.0))
                                .color(t.text.gamma_multiply(0.75)),
                            );
                            ui.add_space(4.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let pick = selected.and_then(|i| candidates.get(i));
                                if ui
                                    .add_enabled(
                                        pick.is_some(),
                                        Button::new(RichText::new("Learn").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    if let Some((name, _)) = pick {
                                        action = Some(DialogAction::TrainCorrection {
                                            notes: notes.clone(),
                                            name: name.clone(),
                                        });
                                    }
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::LearnResult { title, message } => {
            let t = theme(dark_mode);
            let title = *title;
            let message = message.clone();
            // Size from the content. These messages range from one line to ten,
            // and a fixed height pushed the OK button off the bottom edge of the
            // longest one — where only Esc could still dismiss it. 12pt Courier
            // Prime rows are ~13.5pt; the authored lines are under 60 chars, so
            // the 446pt content width does not wrap them.
            let lines = message.lines().count().max(1) as f32;
            let height = (lines * 13.5 + 24.0 + 30.0 + 16.0).clamp(150.0, 460.0);
            show_dialog_viewport(
                ctx,
                placement,
                title,
                Vec2::new(470.0, height),
                Vec2::new(400.0, height.min(200.0)),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(message.as_str())
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::ManageTaught { rows, learning } => {
            let t = theme(dark_mode);
            let mut to_delete: Option<Vec<u8>> = None;
            let mut forget = false;
            // Snapshot for the closure; the live binding is updated afterwards.
            let learning_view = learning.clone();
            let r = show_dialog_viewport(
                ctx,
                placement,
                "Manage Taught Chords",
                Vec2::new(460.0, 460.0),
                Vec2::new(360.0, 320.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new("No taught chords yet.")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            }
                            // List, then the D-UI-9 learning footer, then the
                            // buttons. Reserve the footer's REAL height (it grows
                            // one 9pt line per non-zero weight, up to seven) —
                            // a fixed guess clips the buttons off the bottom
                            // once training fills every feature in.
                            const FOOTER_LINE_H: f32 = 13.0;
                            let weight_rows = learning_view
                                .weights
                                .iter()
                                .filter(|(_, w)| *w != 0.0)
                                .count() as f32;
                            let footer_h = 8.0 // separator
                                + 16.0 // status line
                                + if learning_view.has_learned {
                                    FOOTER_LINE_H * (1.0 + weight_rows)
                                } else {
                                    FOOTER_LINE_H * 2.0 // the two-line hint
                                };
                            let bottom_h = 34.0 + footer_h + 6.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for row in rows.iter() {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add(Button::new(
                                                    RichText::new("Delete").color(t.text),
                                                ))
                                                .clicked()
                                            {
                                                to_delete = Some(row.intervals.clone());
                                            }
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}   [{}]",
                                                    row.display_name, row.voicing
                                                ))
                                                .font(bold(12.0))
                                                .color(t.text),
                                            );
                                        });
                                    }
                                });
                            ui.add_space(6.0);
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "Chord learning: {}  ({} correction{})",
                                    if learning_view.on { "ON" } else { "off" },
                                    learning_view.corrections,
                                    if learning_view.corrections == 1 {
                                        ""
                                    } else {
                                        "s"
                                    },
                                ))
                                .font(bold(11.0))
                                .color(t.text),
                            );
                            if learning_view.has_learned {
                                ui.label(
                                    RichText::new("Learned leanings:")
                                        .font(bold(9.0))
                                        .color(t.text.gamma_multiply(0.75)),
                                );
                                for (label, w) in learning_view.weights.iter() {
                                    if *w == 0.0 {
                                        continue;
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "   {label}: {}{:.0}",
                                            if *w > 0.0 { "+" } else { "" },
                                            w
                                        ))
                                        .font(bold(9.0))
                                        .color(t.text.gamma_multiply(0.75)),
                                    );
                                }
                            } else {
                                // Forget leaves the switch armed but the weights
                                // empty. Say outright that readings are stock, or
                                // "ON (0 corrections)" reads as a failed undo.
                                ui.label(
                                    RichText::new(if learning_view.on {
                                        "Nothing learned — chord names are stock.\n\
                                         Use \"Correct Chord Name...\" to teach a leaning."
                                    } else {
                                        "Nothing learned yet. Play a chord, then use\n\
                                         \"Correct Chord Name...\" in the menu."
                                    })
                                    .font(bold(9.0))
                                    .color(t.text.gamma_multiply(0.75)),
                                );
                            }
                            ui.add_space(6.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("Close").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                                if ui
                                    .add_enabled(
                                        learning_view.has_learned,
                                        Button::new(RichText::new("Forget Learning").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    forget = true;
                                }
                            });
                        });
                },
            );
            if let Some(intervals) = to_delete {
                // Reflect the deletion locally so the list updates immediately,
                // and tell the app to persist it.
                rows.retain(|r| r.intervals != intervals);
                action = Some(DialogAction::DeleteOverride { intervals });
            }
            if forget {
                // Same immediate-feedback pattern as Delete: clear the footer
                // locally so the dialog updates now, and let the app persist.
                learning.has_learned = false;
                learning.corrections = 0;
                learning.weights.clear();
                action = Some(DialogAction::ForgetLearning);
            }
            r
        }

        Dialog::CustomTuning {
            name,
            strings,
            prefer_flats,
            focus,
        } => {
            let t = theme(dark_mode);
            let flats = *prefer_flats;
            // Asked once, outside the loop: see `highest_open_pitch`. The
            // spinner is clamped to it, so the one refusal a user could hit by
            // dragging is not reachable by dragging at all.
            let ceiling = highest_open_pitch();
            show_dialog_viewport(
                ctx,
                placement,
                "Custom Tuning",
                Vec2::new(TUNING_DIALOG_W, TUNING_DIALOG_H),
                Vec2::new(440.0, 320.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    // Text fields and number spinners paint their interiors on
                    // `extreme_bg_color`, which is stock near-black otherwise:
                    // a row of holes punched through a themed dialog.
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    let faint = t.text.gamma_multiply(0.72);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(TUNING_MARGIN))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("Name:").font(bold(11.0)).color(t.text));
                                let resp = ui.add(
                                    egui::TextEdit::singleline(name)
                                        .font(bold(12.0))
                                        .text_color(t.text)
                                        .desired_width(f32::INFINITY),
                                );
                                if *focus {
                                    *focus = false;
                                    focus_and_select_all(ui, &resp, name);
                                }
                            });

                            // ── start from a preset ─────────────────────────
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Start from:").font(bold(10.0)).color(faint),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for preset in fretboard::TUNINGS {
                                    // Marked while the rows still hold exactly
                                    // this preset's pitches, so "standard
                                    // but…" stops claiming to be standard the
                                    // moment it stops being standard.
                                    let untouched = strings.len() == preset.open.len()
                                        && strings
                                            .iter()
                                            .zip(preset.open.iter())
                                            .all(|(row, p)| row.pitch == *p);
                                    if ui
                                        .selectable_label(
                                            untouched,
                                            RichText::new(preset.name.as_ref())
                                                .font(bold(10.0))
                                                .color(t.text),
                                        )
                                        .clicked()
                                    {
                                        // Pitches ONLY. The name belongs to
                                        // the user: it is typed once while the
                                        // presets are clicked several times
                                        // trying them on, so overwriting it
                                        // would throw away typing that cannot
                                        // be got back.
                                        *strings = rows_for(&preset.open, flats);
                                    }
                                }
                            });

                            // ── how many strings ────────────────────────────
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!("Strings: {}", strings.len()))
                                        .font(bold(11.0))
                                        .color(t.text),
                                );
                                ui.add_space(6.0);
                                // Buttons rather than a count field, and the
                                // word "low" in both labels, because the count
                                // alone cannot say WHICH end changes — and
                                // which end changes is the whole difference
                                // between the obvious behaviour and the
                                // annoying one (see `add_low_string`).
                                if ui
                                    .add_enabled(
                                        strings.len() < MAX_STRINGS,
                                        Button::new(
                                            RichText::new("Add low string").color(t.text),
                                        ),
                                    )
                                    .clicked()
                                {
                                    add_low_string(strings, flats);
                                }
                                if ui
                                    .add_enabled(
                                        strings.len() > 1,
                                        Button::new(
                                            RichText::new("Remove low string").color(t.text),
                                        ),
                                    )
                                    .clicked()
                                {
                                    remove_low_string(strings);
                                }
                            });

                            // ── the strings themselves ──────────────────────
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(
                                    "Highest string at the top. They are numbered from the \
                                     LOWEST, the way the message below counts them.",
                                )
                                .font(bold(9.0))
                                .color(faint),
                            );
                            // The interval up from the string below. A player
                            // reads a tuning by its gaps — standard is
                            // 5 5 5 4 5 — and a NEGATIVE one is the monotone
                            // break made visible before Create is even
                            // reached. Snapshotted before the rows are drawn
                            // because a row cannot see its neighbour through
                            // an `iter_mut`; one frame of lag on a decoration.
                            let gaps: Vec<Option<i16>> = strings
                                .iter()
                                .enumerate()
                                .map(|(i, row)| {
                                    i.checked_sub(1).map(|below| {
                                        row.pitch as i16 - strings[below].pitch as i16
                                    })
                                })
                                .collect();
                            // The message band, the button row, and the space
                            // between them. See `TUNING_FOOTER_H` for why the
                            // band is a reservation rather than a measurement.
                            let bottom_h = TUNING_FOOTER_H + 34.0 + 8.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    // Drawn highest-first, which is how tab and
                                    // every chord chart are read, while the
                                    // numbering still counts from the lowest
                                    // string so it agrees with `TuningError`.
                                    for (i, row) in strings.iter_mut().enumerate().rev() {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(format!("{:>2}", i + 1))
                                                    .font(bold(11.0))
                                                    .color(faint),
                                            );
                                            let typed = ui.add(
                                                egui::TextEdit::singleline(&mut row.text)
                                                    .font(bold(12.0))
                                                    .text_color(t.text)
                                                    .desired_width(58.0),
                                            );
                                            if typed.changed() {
                                                // A row whose text does not
                                                // parse keeps the last pitch
                                                // that did, so the spinner
                                                // never shows a number nobody
                                                // chose while a name is
                                                // half-typed.
                                                if let Ok(p) = parse_pitch(&row.text) {
                                                    row.pitch = p;
                                                }
                                            }
                                            let dragged = ui.add(
                                                egui::DragValue::new(&mut row.pitch)
                                                    .range(0..=ceiling),
                                            );
                                            if dragged.changed() {
                                                // `confirm` reads the FIELD, so
                                                // a spinner that moved the
                                                // number without rewriting the
                                                // text would build a tuning out
                                                // of a value the user can no
                                                // longer see.
                                                row.text = pitch_name(row.pitch, flats);
                                            }
                                            if let Some(Some(gap)) = gaps.get(i) {
                                                ui.label(
                                                    RichText::new(format!("{gap:+}"))
                                                        .font(bold(10.0))
                                                        // Full strength when it
                                                        // descends: the theme
                                                        // has no error colour,
                                                        // and weight is the one
                                                        // emphasis that works
                                                        // in both themes.
                                                        .color(if *gap < 0 {
                                                            t.text
                                                        } else {
                                                            faint
                                                        }),
                                                );
                                            }
                                        });
                                    }
                                });

                            // ── what is wrong, or what will be made ─────────
                            //
                            // Computed AFTER the rows are drawn, so the message
                            // answers this frame's keystroke rather than the
                            // one before it, and always occupies the same band
                            // whether it is a complaint or a confirmation.
                            let ready = confirm(name, strings);
                            let footer = match &ready {
                                Ok(built) => format!(
                                    "{} strings: {}",
                                    built.strings(),
                                    built
                                        .open
                                        .iter()
                                        .map(|&p| pitch_name(p, flats))
                                        .collect::<Vec<_>>()
                                        .join(" ")
                                ),
                                Err(problem) => problem.message(flats),
                            };
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(footer)
                                    .font(bold(TUNING_FOOTER_PT))
                                    .color(if ready.is_ok() { faint } else { t.text }),
                            );

                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(
                                        ready.is_ok(),
                                        Button::new(RichText::new("Use Tuning").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    // Closes only by producing a tuning. A
                                    // disabled button cannot be clicked, so
                                    // this is belt and braces — but the belt is
                                    // that there is exactly one place a
                                    // `Tuning` is built, and it is the same
                                    // value that enabled the button.
                                    if let Ok(built) = &ready {
                                        action = Some(DialogAction::SetCustomTuning(
                                            built.clone(),
                                        ));
                                        result.close = true;
                                    }
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::Export {
            spec,
            tempo_text,
            remember,
            post_take,
            had_camera,
        } => {
            let t = theme(dark_mode);
            let post_take = *post_take;
            // Asked once, outside the drawing, because every greyed control in
            // the dialog is greyed by this one value. See `Rederivable`.
            let can = rederivable(post_take, *had_camera);
            show_dialog_viewport(
                ctx,
                placement,
                // The two entry points mean different things and the title bar
                // is the cheapest place to say which one this is.
                if post_take {
                    "Export This Take"
                } else {
                    "Export"
                },
                Vec2::new(EXPORT_DIALOG_W, EXPORT_DIALOG_H),
                Vec2::new(470.0, 340.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    // The tempo field paints its interior on `extreme_bg_color`
                    // — stock near-black otherwise, a hole punched through a
                    // themed dialog. Same trap as the tuning editor.
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    let faint = t.text.gamma_multiply(0.72);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(EXPORT_MARGIN))
                        .show(ui, |ui| {
                            // The footer band and the button row are reserved
                            // first; everything else scrolls. Inline in a
                            // plugin the canvas can be shorter than the
                            // dialog, and Export has to stay reachable there
                            // too — that is the whole reason the funnel exists.
                            let bottom_h = EXPORT_FOOTER_H + 34.0 + 8.0;
                            let body_h = (ui.available_height() - bottom_h).max(60.0);
                            egui::ScrollArea::vertical()
                                .max_height(body_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    // ── always written ──────────────────────
                                    //
                                    // No filenames here, unlike the §5 mock:
                                    // the take's name is built by
                                    // `ivory_record::take` and this crate is
                                    // never handed it (see `recorder.rs` on why
                                    // the band is given strings rather than
                                    // computing them). What each file IS is the
                                    // part the dialog can say truthfully.
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "Always written", false);
                                        export_check(ui, &t, true, &mut spec.audio, "Audio");
                                        ui.label(
                                            RichText::new("48 kHz 24-bit stereo .wav")
                                                .font(bold(9.0))
                                                .color(faint),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "", false);
                                        export_check(ui, &t, true, &mut spec.midi, "MIDI");
                                        ui.label(
                                            RichText::new("SMF 1, 960 ppq, tick 0 at take start")
                                                .font(bold(9.0))
                                                .color(faint),
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "", false);
                                        // Live after a take: rewriting the
                                        // tempo meta is a file edit, which is
                                        // the one thing on this dialog that is
                                        // re-derivable without question. Dead
                                        // with no MIDI, because there would be
                                        // no file to write it into.
                                        let tempo_on = can.tempo && spec.midi;
                                        ui.label(
                                            RichText::new("Tempo mark")
                                                .font(bold(11.0))
                                                .color(t.text),
                                        );
                                        ui.add_enabled(
                                            tempo_on,
                                            egui::TextEdit::singleline(tempo_text)
                                                .font(bold(12.0))
                                                .text_color(t.text)
                                                .desired_width(52.0),
                                        );
                                        ui.label(
                                            RichText::new("BPM").font(bold(11.0)).color(t.text),
                                        );
                                    });

                                    // ── video ───────────────────────────────
                                    ui.add_space(6.0);
                                    for (i, m) in VideoMode::ALL.into_iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            export_label(
                                                ui,
                                                &t,
                                                if i == 0 { "Video" } else { "" },
                                                false,
                                            );
                                            export_radio(
                                                ui,
                                                &t,
                                                m == VideoMode::None || VIDEO_EXPORT_READY,
                                                &mut spec.video,
                                                m,
                                                m.label(),
                                            );
                                        });
                                    }
                                    if !VIDEO_EXPORT_READY {
                                        export_para(ui, &t, VIDEO_PENDING_NOTE);
                                    }

                                    // ── what is in the composite ────────────
                                    //
                                    // The camera and display ticks are read by
                                    // the per-source modes too (they are what
                                    // `encoder_count` counts), so they follow
                                    // "is there a video at all" rather than
                                    // "is there a composite". The layout is the
                                    // only control that is about the composite
                                    // specifically.
                                    let video_on = spec.video.wants_video();
                                    let composite_on = spec.video.has_composite();
                                    ui.add_space(6.0);
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "Composite contains", true);
                                        export_check(
                                            ui,
                                            &t,
                                            video_on && can.camera,
                                            &mut spec.composite.camera,
                                            "Camera",
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "", true);
                                        export_check(
                                            ui,
                                            &t,
                                            video_on,
                                            &mut spec.composite.display,
                                            "Tangent's display",
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "", true);
                                        export_check(
                                            ui,
                                            &t,
                                            video_on,
                                            &mut spec.composite.audio,
                                            "Performance audio",
                                        );
                                    });
                                    for (i, l) in VideoLayout::ALL.into_iter().enumerate() {
                                        ui.horizontal(|ui| {
                                            export_label(
                                                ui,
                                                &t,
                                                if i == 0 { "Layout" } else { "" },
                                                true,
                                            );
                                            export_radio(
                                                ui,
                                                &t,
                                                composite_on && can.layout,
                                                &mut spec.composite.layout,
                                                l,
                                                l.label(),
                                            );
                                        });
                                    }
                                    // Said here rather than at the bottom,
                                    // because it is the answer to "why can I
                                    // not click these".
                                    if let Some(why) = can.why {
                                        export_para(ui, &t, why);
                                    }

                                    // ── which panels the display draws ──────
                                    //
                                    // The app's own panel names, not a second
                                    // vocabulary invented for the video. These
                                    // override the live settings FOR THE VIDEO
                                    // ONLY — recording a clean piano-and-chord
                                    // video while keeping the fretboard on
                                    // screen for your own use is the case this
                                    // exists for. `shows_from_settings` seeds
                                    // them.
                                    let shows_on =
                                        video_on && spec.composite.display && can.display;
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "Display shows", true);
                                        export_check(
                                            ui,
                                            &t,
                                            shows_on,
                                            &mut spec.composite.shows.piano,
                                            "Piano",
                                        );
                                        export_check(
                                            ui,
                                            &t,
                                            shows_on,
                                            &mut spec.composite.shows.chord,
                                            "Chord name",
                                        );
                                    });
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "", true);
                                        export_check(
                                            ui,
                                            &t,
                                            shows_on,
                                            &mut spec.composite.shows.fretboard,
                                            "Fretboard",
                                        );
                                        export_check(
                                            ui,
                                            &t,
                                            shows_on,
                                            &mut spec.composite.shows.theory,
                                            "Theory",
                                        );
                                    });

                                    // ── frame size and rate ─────────────────
                                    ui.add_space(6.0);
                                    for (i, chunk) in Resolution::ALL.chunks(2).enumerate() {
                                        ui.horizontal(|ui| {
                                            export_label(
                                                ui,
                                                &t,
                                                if i == 0 { "Resolution" } else { "" },
                                                true,
                                            );
                                            for &r in chunk {
                                                // "Match camera" after a take
                                                // would be matching a camera
                                                // that is not running and whose
                                                // frames were never kept.
                                                let on = video_on
                                                    && (r != Resolution::MatchCamera || can.camera);
                                                export_radio(
                                                    ui,
                                                    &t,
                                                    on,
                                                    &mut spec.resolution,
                                                    r,
                                                    r.label(),
                                                );
                                            }
                                        });
                                    }
                                    ui.horizontal(|ui| {
                                        export_label(ui, &t, "Frame rate", true);
                                        for f in FPS_CHOICES {
                                            export_radio(
                                                ui,
                                                &t,
                                                video_on,
                                                &mut spec.fps,
                                                f,
                                                &format!("{f} fps"),
                                            );
                                        }
                                    });
                                    export_para(ui, &t, encoder_note());
                                });

                            // ── what it costs, or what is wrong ─────────────
                            //
                            // Computed AFTER the body, so the footer answers
                            // this frame's click rather than the one before it,
                            // and occupying the same band either way so the
                            // buttons do not walk under the cursor.
                            let built = export_ready(spec, tempo_text);
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(estimate_line(spec))
                                    .font(bold(EXPORT_FOOTER_PT))
                                    .color(faint),
                            );
                            if let Err(why) = &built {
                                ui.label(
                                    RichText::new(why.as_str())
                                        .font(bold(EXPORT_FOOTER_PT))
                                        .color(t.text),
                                );
                            }

                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.horizontal(|ui| {
                                let mut keep = *remember;
                                if ui
                                    .checkbox(
                                        &mut keep,
                                        RichText::new("Use these settings for every take")
                                            .font(bold(11.0))
                                            .color(t.text),
                                    )
                                    .changed()
                                {
                                    *remember = keep;
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if ui
                                        .add_enabled(
                                            built.is_ok(),
                                            Button::new(RichText::new("Export").color(t.text)),
                                        )
                                        .clicked()
                                    {
                                        // Closes only by producing a spec, and
                                        // it is the same value that enabled the
                                        // button — `export_ready` is the one
                                        // gate, so nothing invalid can reach
                                        // the app even if the button were
                                        // somehow clicked while greyed.
                                        if let Ok(s) = &built {
                                            action = Some(export_action(*s, *remember));
                                            result.close = true;
                                        }
                                    }
                                    if ui
                                        .add(Button::new(RichText::new("Cancel").color(t.text)))
                                        .clicked()
                                    {
                                        result.close = true;
                                    }
                                });
                            });
                        });
                },
            )
        }
    };

    if result.close {
        *dialog_opt = None;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dialog must centre on the WINDOW, and must refuse to guess when the
    /// window's position is not known yet. The welcome note opens on the first
    /// frame, before the platform has reported the main window's rect; handing
    /// it a rect at (0, 0) made it centre on the corner of the screen, which
    /// looks like a placement bug and is really a "we did not know" bug.
    #[test]
    fn a_dialog_centres_on_the_window_or_declines_to_place_itself() {
        let size = Vec2::new(400.0, 300.0);
        let monitor = Some(Vec2::new(2560.0, 1440.0));

        // Parent unknown: no position, so the platform places it.
        assert_eq!(
            Placement {
                parent: None,
                monitor,
                caps: crate::host::Caps::DESKTOP
            }
            .position_for(size),
            None
        );

        // Parent known: dead centre of the parent.
        let parent = Rect::from_min_size(Pos2::new(600.0, 400.0), Vec2::new(1300.0, 200.0));
        let p = Placement {
            parent: Some(parent),
            monitor,
            caps: crate::host::Caps::DESKTOP,
        }
        .position_for(size)
        .expect("a known parent must yield a position");
        let dialog_centre = p + size * 0.5;
        assert!(
            (dialog_centre.x - parent.center().x).abs() < 0.5,
            "off-centre in x"
        );
        assert!(
            (dialog_centre.y - parent.center().y).abs() < 0.5,
            "off-centre in y"
        );

        // A window near an edge still puts the whole dialog on the monitor.
        let edge = Rect::from_min_size(Pos2::new(2400.0, 1380.0), Vec2::new(1300.0, 200.0));
        let p = Placement {
            parent: Some(edge),
            monitor,
            caps: crate::host::Caps::DESKTOP,
        }
        .position_for(size)
        .unwrap();
        assert!(p.x >= 0.0 && p.y >= 0.0, "pushed off the top-left: {p:?}");
        assert!(p.x + size.x <= 2560.0 + 0.5, "hangs off the right: {p:?}");
        assert!(p.y + size.y <= 1440.0 + 0.5, "hangs off the bottom: {p:?}");
    }

    // ── the custom tuning editor ────────────────────────────────────────────

    /// Rows built from what somebody would actually type.
    fn rows(texts: &[&str]) -> Vec<TuningString> {
        texts
            .iter()
            .map(|s| TuningString {
                text: (*s).to_owned(),
                pitch: parse_pitch(s).unwrap_or(0),
            })
            .collect()
    }

    fn pitches(rows: &[TuningString]) -> Vec<u8> {
        rows.iter().map(|r| r.pitch).collect()
    }

    fn preset(name: &str) -> Vec<u8> {
        Tuning::by_name(name)
            .unwrap_or_else(|| panic!("{name} is not in TUNINGS any more"))
            .open
            .to_vec()
    }

    /// A note name has to survive the trip out to the field and back, in the
    /// spelling the app prefers — otherwise the editor would rewrite a string
    /// the user typed into a different one the moment they touched the spinner.
    #[test]
    fn note_names_and_midi_numbers_round_trip_in_the_spelling_the_app_prefers() {
        for prefer_flats in [true, false] {
            for pitch in 0..=127u8 {
                let name = pitch_name(pitch, prefer_flats);
                assert_eq!(
                    parse_pitch(&name),
                    Ok(pitch),
                    "{name} did not come back as {pitch}"
                );
            }
        }
        // The app prefers flats (`ChordDetector::new`, `Settings::default`),
        // and a string has to be spelled the way the piano above it spells the
        // same note. This is the assertion that would fail if the editor grew
        // its own note table.
        assert_eq!(pitch_name(61, true), "Db4");
        assert_eq!(pitch_name(61, false), "C#4");
        // Both spellings are READ whichever is preferred, and case is not a
        // spelling: nobody copying a tuning off a forum post holds shift for it.
        for text in ["Db4", "C#4", "db4", "c#4", " Db4 ", "61"] {
            assert_eq!(parse_pitch(text), Ok(61), "{text:?} was not read as Db4");
        }
        // Standard must read back as the E A D G B E written on every guitar
        // chart, in the octaves a guitarist would write. An off-by-one octave
        // is invisible until somebody who plays reads it.
        let standard: Vec<String> = Tuning::standard()
            .open
            .iter()
            .map(|&p| pitch_name(p, true))
            .collect();
        assert_eq!(standard, ["E2", "A2", "D3", "G3", "B3", "E4"]);
        // Both ends of MIDI, where the octave arithmetic goes negative.
        assert_eq!(pitch_name(0, true), "C-1");
        assert_eq!(parse_pitch("C-1"), Ok(0));
        assert_eq!(parse_pitch("G9"), Ok(127));

        // A parser that can only say "no" makes the user guess which of six
        // things was wrong, so each refusal names its own.
        for (bad, must_mention) in [
            ("", "note name"),
            ("   ", "note name"),
            ("H2", "A to G"),
            ("E", "octave"),
            ("E#b2", "accidental"),
            ("Ex", "octave"),
            ("C-3", "MIDI"),
            ("300", "0 to 127"),
        ] {
            let hint = parse_pitch(bad).expect_err(&format!("{bad:?} was read as a pitch"));
            assert!(
                hint.contains(must_mention),
                "{bad:?} was refused with {hint:?}, which never mentions {must_mention:?}"
            );
        }
    }

    /// Every way a tuning can be refused has to arrive as a REASON. The one
    /// that matters is `NotAscending`: it is not a style rule, it is the
    /// voicing solver's search invariant, and "invalid" would be hiding the
    /// only interesting thing the editor knows.
    #[test]
    fn every_reason_a_tuning_can_be_refused_reaches_the_user_as_a_reason() {
        // Listed one by one on purpose: a variant added to `TuningError` later
        // must be given words here too.
        for e in [
            TuningError::StringCount(0),
            TuningError::StringCount(MAX_STRINGS + 1),
            TuningError::NotAscending {
                string: 2,
                previous: 50,
                found: 45,
            },
            TuningError::PitchRange(120),
            TuningError::EmptyName,
        ] {
            let msg = Problem::Refused(e.clone()).message(true);
            assert!(msg.len() > 20, "{e:?} was answered with {msg:?}");
            assert!(
                !msg.to_lowercase().contains("invalid"),
                "{e:?} was answered with \"invalid\", which is not a reason: {msg:?}"
            );
        }

        let ascending = Problem::Refused(TuningError::NotAscending {
            string: 2,
            previous: 50,
            found: 45,
        })
        .message(true);
        // Note names, not MIDI numbers: the user is looking at a field that
        // says A2. This is the whole reason core's `Display` is not reused for
        // this variant.
        assert!(
            ascending.contains("A2") && ascending.contains("D3"),
            "not in the notation the fields use: {ascending}"
        );
        assert!(
            !ascending.contains("45") && !ascending.contains("50"),
            "raw MIDI numbers leaked into a note-name editor: {ascending}"
        );
        // 1-based from the LOW end, the way `TuningError` counts, so the
        // message and the row numbers on screen point at the same string.
        assert!(
            ascending.contains("String 3") && ascending.contains("string 2"),
            "the wrong string is accused: {ascending}"
        );
        assert!(
            ascending.contains("solver") && ascending.contains("shapes"),
            "the reason is missing, so this reads as a taste rule: {ascending}"
        );

        let too_high = Problem::Refused(TuningError::PitchRange(120)).message(true);
        assert!(too_high.contains("C9"), "which pitch? {too_high}");
        assert!(
            too_high.contains(&pitch_name(highest_open_pitch(), true)),
            "refused without saying what would be accepted: {too_high}"
        );

        let unreadable = Problem::Unreadable {
            string: 3,
            text: "Q9".to_owned(),
            hint: parse_pitch("Q9").expect_err("Q is not a note"),
        }
        .message(true);
        assert!(
            unreadable.contains("String 3") && unreadable.contains("Q9"),
            "{unreadable}"
        );

        let clash = Problem::NameClash {
            preset: "Standard".to_owned(),
        }
        .message(true);
        assert!(clash.contains("Standard"), "{clash}");
        assert!(
            clash.contains("name"),
            "the fix is to rename it, and the message has to say so: {clash}"
        );
    }

    /// A seventh string is a low B added BELOW the low E, not a string stapled
    /// above the top one, and the shipped presets settle it rather than taste:
    /// this asserts the result is `TUNINGS`'s own 7- and 8-string tunings, note
    /// for note.
    #[test]
    fn adding_a_string_grows_the_low_end_like_a_seven_string() {
        let mut r: Vec<TuningString> = Tuning::standard()
            .open
            .iter()
            .map(|&p| TuningString::new(p, true))
            .collect();

        add_low_string(&mut r, true);
        assert_eq!(
            pitches(&r),
            preset("7-String"),
            "the added string is not the low B a 7-string guitar actually has"
        );
        // And it arrives spelled, not as an empty box waiting to be filled in.
        assert_eq!(r[0].text, "B1");
        add_low_string(&mut r, true);
        assert_eq!(pitches(&r), preset("8-String"));

        remove_low_string(&mut r);
        remove_low_string(&mut r);
        assert_eq!(
            pitches(&r),
            preset("Standard"),
            "Add then Remove has to be a round trip, which it only is if both \
             touch the same end"
        );

        // The presets shed from that end too, which is the other half of the
        // argument for it.
        let mut bass: Vec<TuningString> = preset("Bass (5)")
            .iter()
            .map(|&p| TuningString::new(p, true))
            .collect();
        remove_low_string(&mut bass);
        assert_eq!(pitches(&bass), preset("Bass (4)"));

        // The bounds hold even when a caller ignores the disabled buttons, and
        // nothing added below can ever sound ABOVE what it was added under —
        // an "add a string" button that broke the solver's invariant would be
        // the worst possible source of it.
        let mut wide = r.clone();
        for _ in 0..20 {
            add_low_string(&mut wide, true);
        }
        assert_eq!(wide.len(), MAX_STRINGS);
        for pair in wide.windows(2) {
            assert!(
                pair[0].pitch <= pair[1].pitch,
                "adding strings broke the ascending order: {:?}",
                pitches(&wide)
            );
        }
        let mut one = vec![TuningString::new(40, true)];
        remove_low_string(&mut one);
        assert_eq!(one.len(), 1, "a tuning with no strings is not a tuning");
    }

    /// Confirm is one function, and it is the same value that greys the button
    /// out and that builds the `Tuning` — so nothing invalid can reach the app
    /// even if the button were somehow clicked.
    #[test]
    fn confirm_refuses_everything_the_solver_could_not_survive() {
        let good = confirm("My Tuning", &rows(&["E2", "A2", "D3", "G3", "B3", "E4"]))
            .expect("standard tuning under a new name is the plainest custom tuning there is");
        assert_eq!(good.name, "My Tuning");
        assert_eq!(good.open.as_ref(), Tuning::standard().open.as_ref());
        // Whitespace is the user's, not the tuning's.
        assert_eq!(
            confirm("  Padded  ", &rows(&["E2"]))
                .expect("a padded name is a name")
                .name,
            "Padded"
        );
        // A copy of a preset under its own name IS that preset, so it is
        // harmless and must not be refused.
        assert!(
            confirm("Standard", &rows(&["E2", "A2", "D3", "G3", "B3", "E4"])).is_ok(),
            "an unedited copy of a preset was refused"
        );

        let many: Vec<&str> = vec!["E2"; MAX_STRINGS + 1];
        for (why, name, texts) in [
            ("a half-typed pitch", "My Tuning", vec!["E2", "A2", "D3", "E"]),
            ("a name that is only whitespace", "   ", vec!["E2", "A2"]),
            (
                "a preset's name over different pitches",
                "Drop D",
                vec!["E2", "A2"],
            ),
            (
                "a preset's name in lower case, since lookup ignores it",
                "drop d",
                vec!["E2", "A2"],
            ),
            ("strings that do not ascend", "My Tuning", vec!["E2", "D2"]),
            (
                "an open string with no room to fret above it",
                "My Tuning",
                vec!["C9"],
            ),
            ("no strings at all", "My Tuning", vec![]),
            ("more strings than the solver will enumerate", "My Tuning", many),
        ] {
            let problem = confirm(name, &rows(&texts))
                .err()
                .unwrap_or_else(|| panic!("{why} was accepted"));
            let msg = problem.message(true);
            assert!(msg.len() > 20, "{why} was refused with {msg:?}");
            assert!(
                !msg.to_lowercase().contains("invalid"),
                "{why}: \"invalid\" is not a reason: {msg}"
            );
        }
    }

    /// The message under the strings gets a fixed band of the dialog so that
    /// the buttons do not walk up and down under the cursor as you type. That
    /// only works if the LONGEST thing it can say fits inside the band — and
    /// "it should come to about four lines" is exactly the reasoning that once
    /// pushed the D-UI-9 result box's OK button off its bottom edge. So this
    /// measures every message the editor can produce, in the face it uses, at
    /// the size it uses, wrapped to the width it has.
    #[test]
    fn the_longest_refusal_fits_the_space_reserved_for_it() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);

        let mut messages = vec![
            Problem::NameClash {
                preset: "Half Step Down".to_owned(),
            }
            .message(true),
            Problem::Unreadable {
                string: 12,
                text: "not a note at all".to_owned(),
                hint: parse_pitch("Q").expect_err("Q is not a note"),
            }
            .message(true),
        ];
        for e in [
            TuningError::StringCount(MAX_STRINGS + 1),
            TuningError::NotAscending {
                string: 11,
                previous: 103,
                found: 0,
            },
            TuningError::PitchRange(127),
            TuningError::EmptyName,
        ] {
            messages.push(Problem::Refused(e).message(true));
        }
        // The band holds the confirmation as well as the complaints, and the
        // longest of those is a full twelve strings spelled out.
        let widest = Tuning::custom("wide", (0..MAX_STRINGS as u8).map(|i| i * 8).collect())
            .expect("twelve ascending strings inside the range is a legal tuning");
        messages.push(format!(
            "{} strings: {}",
            widest.strings(),
            widest
                .open
                .iter()
                .map(|&p| pitch_name(p, false))
                .collect::<Vec<_>>()
                .join(" ")
        ));

        // Exactly the width the label is given: the dialog's, less the
        // `Frame` margin on each side.
        let content_w = TUNING_DIALOG_W - 2.0 * f32::from(TUNING_MARGIN);
        let mut worst = (0.0_f32, String::new());
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(TUNING_DIALOG_W, TUNING_DIALOG_H),
                )),
                ..Default::default()
            },
            |ctx| {
                crate::shell::viewport_ui(ctx, |ui| {
                    for m in &messages {
                        let galley = ui.painter().layout(
                            m.clone(),
                            FontId::new(TUNING_FOOTER_PT, fonts::courier_bold()),
                            Color32::WHITE,
                            content_w,
                        );
                        if galley.size().y > worst.0 {
                            worst = (galley.size().y, m.clone());
                        }
                    }
                });
            },
        );
        assert!(
            worst.0 <= TUNING_FOOTER_H,
            "{TUNING_FOOTER_H}pt is reserved for the message and the longest one \
             needs {}pt, which would push the buttons off the bottom edge: {:?}",
            worst.0,
            worst.1
        );
    }

    /// The editor has to draw where there is no window manager at all: in a
    /// plugin it is an `Area` in the DAW's canvas, and `show` is the only
    /// funnel that knows the difference. This runs a whole frame of it inline —
    /// text fields, a wrapped row of preset buttons, a scroll area and twelve
    /// spinners sharing one `Area` id — and asserts the editor does nothing on
    /// its own until it is confirmed.
    #[test]
    fn the_editor_draws_inline_in_a_plugin_and_says_nothing_until_it_is_confirmed() {
        let ctx = egui::Context::default();
        // The dialogs name Courier Prime explicitly and epaint PANICS on a
        // font family that is not bound, so the fonts have to be installed
        // exactly as the app installs them.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);

        let mut dialog = Some(Dialog::custom_tuning(&Tuning::standard(), true));
        let placement = Placement {
            parent: None,
            monitor: None,
            caps: crate::host::Caps::PLUGIN,
        };
        let mut action = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 400.0))),
                ..Default::default()
            },
            |ctx| {
                action = show(ctx, &mut dialog, true, placement);
            },
        );
        assert!(action.is_none(), "the editor acted without being asked to");

        match dialog {
            Some(Dialog::CustomTuning { name, strings, .. }) => {
                assert_eq!(strings.len(), 6, "it did not open on the live tuning");
                // Opened on a name the app will NOT resolve back to the preset,
                // which is the trap `DialogAction::SetCustomTuning` describes.
                assert_ne!(name, "Standard");
                assert!(
                    confirm(&name, &strings).is_ok(),
                    "the editor opened on a tuning it would refuse to accept: {}",
                    confirm(&name, &strings)
                        .err()
                        .map(|p| p.message(true))
                        .unwrap_or_default()
                );
            }
            _ => panic!("the editor closed itself, or changed into another dialog"),
        }
    }

    // ── the export dialog ───────────────────────────────────────────────────

    use crate::recorder::{Composite, SpecProblem};

    /// The four specs the validator has an opinion about, plus the one it does
    /// not. Built by hand rather than through `Dialog::export`, which clamps.
    fn export_specs() -> [ExportSpec; 4] {
        [
            ExportSpec::default(),
            ExportSpec {
                audio: false,
                midi: false,
                ..Default::default()
            },
            ExportSpec {
                video: VideoMode::Composite,
                composite: Composite {
                    camera: false,
                    display: false,
                    ..Default::default()
                },
                ..Default::default()
            },
            ExportSpec {
                video: VideoMode::Composite,
                composite: Composite {
                    camera: false,
                    display: true,
                    shows: DisplayShows {
                        piano: false,
                        chord: false,
                        fretboard: false,
                        theory: false,
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
        ]
    }

    /// One function greys the button and builds the value, so the two cannot
    /// drift apart — and the words shown are the recorder's own, because a
    /// second wording here would be a second answer to the same question.
    #[test]
    fn the_export_button_is_greyed_exactly_when_the_spec_has_a_problem() {
        for s in export_specs() {
            let built = export_ready(&s, "120");
            assert_eq!(
                built.is_err(),
                s.problem().is_some(),
                "the button and the validator disagree about {s:?}"
            );
            match (s.problem(), built) {
                (Some(p), Err(msg)) => assert_eq!(msg, p.message(), "invented its own words"),
                (None, Ok(out)) => assert_eq!(out, s, "an accepted spec was altered on the way"),
                (p, b) => panic!("{p:?} against {b:?}"),
            }
        }
        // Audio and MIDI default ON and cannot both be off: the owner's brief
        // is that a take produces the audio, the MIDI and the video, and a
        // directory with none of the first two is not a take. Refused, rather
        // than an empty folder appearing on the user's disk.
        let none = ExportSpec {
            audio: false,
            midi: false,
            ..Default::default()
        };
        assert!(ExportSpec::default().audio && ExportSpec::default().midi);
        assert_eq!(
            export_ready(&none, "120").unwrap_err(),
            SpecProblem::NothingToWrite.message()
        );
    }

    /// A finished take can never gain the camera. Those frames are composited
    /// and encoded as the take runs and nothing keeps them (§0: raw retention
    /// is 112 GB per take), and there is no decoder anywhere in this design to
    /// read the finished file back. Offering the tick anyway would be offering
    /// an option that cannot work, which is worse than not offering it.
    #[test]
    fn a_take_recorded_without_a_camera_can_never_gain_one() {
        let can = rederivable(true, false);
        assert!(!can.camera, "the camera controls were left live");
        assert!(!can.layout, "a layout for a camera layer that cannot exist");
        assert!(
            can.tempo && can.display,
            "the two things that ARE re-derivable were greyed with them"
        );
        let why = can.why.expect("greyed with no reason given");
        assert!(
            why.to_lowercase().contains("camera"),
            "the reason never mentions the camera: {why}"
        );

        // And the seed is clamped, so the dialog cannot even OPEN promising a
        // camera video: the spec comes from the settings file or the last take
        // and neither of them knows what this take caught.
        let clamped = can.clamp(ExportSpec {
            video: VideoMode::Both,
            ..Default::default()
        });
        assert!(!clamped.composite.camera);
        assert_eq!(
            clamped.video,
            VideoMode::Composite,
            "a display-only video IS re-derivable — replayed from the recorded .mid"
        );
        assert!(clamped.is_valid());

        // With the display out of it too there is no video left to make, and
        // an empty composite is not a video.
        let nothing_left = can.clamp(ExportSpec {
            video: VideoMode::Composite,
            composite: Composite {
                camera: true,
                display: false,
                ..Default::default()
            },
            ..Default::default()
        });
        assert_eq!(nothing_left.video, VideoMode::None);
        assert!(
            nothing_left.is_valid(),
            "clamping produced a spec the dialog would then refuse: {nothing_left:?}"
        );
    }

    /// The camera half is settled even when there WAS a camera: those frames
    /// were composited in that arrangement as the take ran, so "Camera full
    /// frame, display overlaid" can never become "Side by side".
    #[test]
    fn a_finished_take_cannot_be_given_a_different_layout_even_when_it_had_a_camera() {
        let can = rederivable(true, true);
        assert!(!can.layout, "the layout was left live after the take");
        assert!(!can.camera);
        assert!(can.tempo && can.display);
        let why = can.why.expect("greyed with no reason given");
        assert!(
            why.to_lowercase().contains("layout"),
            "the layout is greyed and nothing says why: {why}"
        );
        // A take that HAD a camera and one that did not are greyed for
        // different reasons, and they are worth saying differently.
        assert_ne!(can.why, rederivable(true, false).why);

        // Before a take nothing has happened yet, so nothing is fixed and
        // there is nothing to explain.
        let before = rederivable(false, false);
        assert_eq!(before, Rederivable::EVERYTHING);
        assert!(before.why.is_none());
        let untouched = ExportSpec {
            video: VideoMode::Both,
            ..Default::default()
        };
        assert_eq!(before.clamp(untouched), untouched);
    }

    /// The field holds TEXT for exactly this reason: a partially-typed number
    /// is not a number, and "9" on the way to "92" must not be rounded up to
    /// `MIN_BPM` under the cursor. And a tempo that is finished and out of
    /// range is REFUSED, not quietly clamped — a field that turned a typed 400
    /// into 300 would be a field lying to somebody who was not watching.
    #[test]
    fn a_half_typed_tempo_never_reaches_the_spec_and_an_impossible_one_is_refused_not_clamped() {
        let spec = ExportSpec {
            tempo_bpm: 120.0,
            ..Default::default()
        };
        for half in ["9", "", " ", "-", "1.", "9e"] {
            assert!(
                export_ready(&spec, half).is_err(),
                "{half:?} was accepted as a tempo"
            );
            assert_eq!(spec.tempo_bpm, 120.0, "{half:?} reached the stored spec");
        }
        for bad in ["19", "19.9", "301", "0", "-40", "1e9", "NaN", "inf"] {
            let e = export_ready(&spec, bad).expect_err("{bad:?} was accepted");
            assert!(e.len() > 20, "{bad:?} was refused with {e:?}");
        }
        // In range is taken exactly, at both ends and with a fraction.
        for (text, want) in [
            ("120", 120.0),
            ("92.5", 92.5),
            (" 88 ", 88.0),
            ("20", MIN_BPM),
            ("300", MAX_BPM),
        ] {
            assert_eq!(
                export_ready(&spec, text).expect("a legal tempo").tempo_bpm,
                want,
                "{text:?}"
            );
        }
        // With no MIDI there is no file to write a tempo mark into, so a stale
        // value in a greyed field must not be able to grey Export out as well
        // — that is a dead end with nothing on screen the user can fix.
        let no_midi = ExportSpec {
            midi: false,
            ..spec
        };
        assert_eq!(
            export_ready(&no_midi, "not a tempo at all")
                .expect("a greyed field refused an export it has no say in")
                .tempo_bpm,
            120.0
        );
    }

    /// Open it, change nothing, commit: the spec has to come back exactly as it
    /// went in. The seeded text is the only copy of the tempo while the dialog
    /// is open, so a constructor that seeded it badly would silently rewrite
    /// the value on the way out.
    #[test]
    fn the_export_dialog_opens_on_the_spec_it_was_given_and_commits_it_unchanged() {
        let spec = ExportSpec {
            tempo_bpm: 92.5,
            ..Default::default()
        };
        match Dialog::export(spec, false, true) {
            Dialog::Export {
                spec: seeded,
                tempo_text,
                remember,
                post_take,
                had_camera,
            } => {
                assert_eq!(seeded, spec, "the spec was altered on the way in");
                assert_eq!(tempo_text, "92.5");
                assert!(!remember, "it must not persist without being asked to");
                assert!(!post_take && had_camera);
                assert_eq!(
                    export_ready(&seeded, &tempo_text).expect("opened on a spec it refuses"),
                    spec,
                    "a round trip through the field changed the tempo"
                );
            }
            _ => panic!("Dialog::export built some other dialog"),
        }
        // A whole number reads as a whole number: a field opening on "120.0"
        // is one every user retypes.
        match Dialog::export(ExportSpec::default(), false, false) {
            Dialog::Export { tempo_text, .. } => assert_eq!(tempo_text, "120"),
            _ => panic!("Dialog::export built some other dialog"),
        }
    }

    /// The tick is the whole difference between "this take" and "every take",
    /// so it decides the action rather than being reported alongside it.
    #[test]
    fn the_remember_tick_is_what_chooses_the_action_that_persists() {
        let spec = ExportSpec {
            fps: 60,
            ..Default::default()
        };
        match export_action(spec, false) {
            DialogAction::SetExport(s) => assert_eq!(s, spec),
            _ => panic!("an unticked box persisted the spec anyway"),
        }
        match export_action(spec, true) {
            DialogAction::SetExportAndRemember(s) => assert_eq!(s, spec),
            _ => panic!("the tick did not reach the action"),
        }
    }

    /// The video panels seed from what is on screen, and the theory band is
    /// one panel however many diagrams are inside it.
    #[test]
    fn the_video_panel_ticks_seed_from_the_panels_that_are_on_screen() {
        let mut s = crate::settings::Settings::default();
        let shows = shows_from_settings(&s);
        assert!(shows.piano && shows.chord, "the panels that are never off");
        assert!(!shows.fretboard && !shows.theory);
        s.show_fretboard = true;
        // Only the Tonnetz. Seeding from `theory_circle` alone would drop this
        // user's band out of their video without saying anything.
        s.theory_tonnetz = true;
        let shows = shows_from_settings(&s);
        assert!(shows.fretboard && shows.theory);
    }

    /// Disk AND CPU, because "will it fill my disk" and "will it stutter while
    /// I play" are two questions and a byte count answers only one of them.
    #[test]
    fn the_estimate_names_a_rate_and_an_encoder_count() {
        let quiet = estimate_line(&ExportSpec::default());
        assert!(quiet.contains("MB a minute"), "{quiet}");
        assert!(
            quiet.contains("no video encoder"),
            "audio and MIDI run no encoder at all: {quiet}"
        );
        let busy = estimate_line(&ExportSpec {
            video: VideoMode::Both,
            ..Default::default()
        });
        assert!(busy.contains("3 video encoders"), "{busy}");
        assert!(
            encoder_note().contains("1080p30"),
            "the note never says what a stream costs: {}",
            encoder_note()
        );
    }

    /// The footer gets a fixed band so the buttons do not walk up and down
    /// under the cursor as you click. That only works if the longest thing it
    /// can say fits — and "it should come to about two lines" is exactly the
    /// reasoning that once pushed a dialog's OK button off its own bottom edge.
    #[test]
    fn the_longest_export_refusal_fits_the_space_reserved_for_it() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);

        // Every refusal the footer can hold, plus the fattest estimate line —
        // the two are painted together, so it is their SUM that has to fit.
        let mut refusals: Vec<String> = export_specs()
            .into_iter()
            .filter_map(|s| export_ready(&s, "120").err())
            .collect();
        for bad in ["", "not a tempo at all, not even close", "400"] {
            refusals.push(
                export_ready(&ExportSpec::default(), bad).expect_err("a bad tempo was accepted"),
            );
        }
        let estimate = estimate_line(&ExportSpec {
            video: VideoMode::Both,
            ..Default::default()
        });

        // Exactly the width the labels are given: the dialog's, less the
        // `Frame` margin on each side.
        let content_w = EXPORT_DIALOG_W - 2.0 * f32::from(EXPORT_MARGIN);
        let mut worst = (0.0_f32, String::new());
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(EXPORT_DIALOG_W, EXPORT_DIALOG_H),
                )),
                ..Default::default()
            },
            |ctx| {
                crate::shell::viewport_ui(ctx, |ui| {
                    let height = |m: &str| {
                        ui.painter()
                            .layout(
                                m.to_owned(),
                                FontId::new(EXPORT_FOOTER_PT, fonts::courier_bold()),
                                Color32::WHITE,
                                content_w,
                            )
                            .size()
                            .y
                    };
                    let base = height(&estimate);
                    for m in &refusals {
                        let total = base + height(m) + ui.spacing().item_spacing.y;
                        if total > worst.0 {
                            worst = (total, m.clone());
                        }
                    }
                });
            },
        );
        assert!(
            worst.0 <= EXPORT_FOOTER_H,
            "{EXPORT_FOOTER_H}pt is reserved for the footer and the estimate plus the \
             longest refusal needs {}pt, which would push Export off the bottom edge: {:?}",
            worst.0,
            worst.1
        );
    }

    /// Like the tuning editor, this dialog has to draw where there is no window
    /// manager at all — in a plugin it is an `Area` in the DAW's canvas, and
    /// `show` is the only funnel that knows the difference. This runs a whole
    /// frame of it inline (a text field, four radio groups, eleven checkboxes
    /// and a scroll area sharing one `Area` id) and asserts it does nothing on
    /// its own until it is confirmed.
    #[test]
    fn the_export_dialog_draws_inline_in_a_plugin_and_says_nothing_until_it_is_confirmed() {
        let ctx = egui::Context::default();
        // The dialogs name Courier Prime explicitly and epaint PANICS on a
        // font family that is not bound.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);

        // The post-take case, which is the one with greyed controls in it.
        let mut dialog = Some(Dialog::export(ExportSpec::default(), true, false));
        let placement = Placement {
            parent: None,
            monitor: None,
            caps: crate::host::Caps::PLUGIN,
        };
        let mut action = None;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 420.0))),
                ..Default::default()
            },
            |ctx| {
                action = show(ctx, &mut dialog, true, placement);
            },
        );
        assert!(action.is_none(), "the dialog exported without being asked");

        match dialog {
            Some(Dialog::Export {
                spec, tempo_text, ..
            }) => assert!(
                export_ready(&spec, &tempo_text).is_ok(),
                "it opened on a spec it would refuse to export: {:?}",
                export_ready(&spec, &tempo_text).err()
            ),
            _ => panic!("the dialog closed itself, or changed into another one"),
        }
    }

    // ── the plugin picker ───────────────────────────────────────────────────

    fn entry(path: &str, name: &str, vendor: &str) -> PluginEntry {
        PluginEntry {
            path: path.to_owned(),
            name: name.to_owned(),
            vendor: vendor.to_owned(),
        }
    }

    /// A believable shelf. Two vendors are known because the app has loaded
    /// those bundles before; the other two are blank, which is the ordinary
    /// case and the whole point of the picker not scanning.
    fn installed() -> Vec<PluginEntry> {
        vec![
            entry(
                "/Library/Audio/Plug-Ins/VST3/Analog Lab V.vst3",
                "Analog Lab V",
                "Arturia",
            ),
            entry(
                "/Users/x/Library/Audio/Plug-Ins/VST3/Pianoteq 8.vst3",
                "Pianoteq 8",
                "Modartt",
            ),
            entry("/Library/Audio/Plug-Ins/VST3/Serum2.vst3", "Serum2", ""),
            entry("/Library/Audio/Plug-Ins/VST3/Vital.vst3", "Vital", ""),
        ]
    }

    /// Every string a frame actually painted.
    ///
    /// "The dialog says where VST3s live" is a claim about what is on screen and
    /// there is no other way to reach it: the help is a label, not a value. So
    /// the frame's shapes are walked and their galleys read.
    fn painted_text(output: &egui::FullOutput) -> String {
        fn walk(shape: &egui::epaint::Shape, out: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    out.push_str(t.galley.text());
                    out.push('\n');
                }
                egui::epaint::Shape::Vec(v) => {
                    for s in v {
                        walk(s, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = String::new();
        for c in &output.shapes {
            walk(&c.shape, &mut out);
        }
        out
    }

    /// The picker drawn inline, the way a plugin editor draws it: what ended up
    /// on the screen, and anything it asked the app to do.
    ///
    /// Three frames, not one. An `egui::Area` whose size is not known yet runs a
    /// SIZING PASS — it lays the whole dialog out and paints none of it
    /// (`Area::content_ui` builds that `Ui` with `.invisible()`), and areas
    /// fade in over the frames after. A test that ran one frame and read the
    /// shapes would find a blank screen and conclude the dialog draws nothing,
    /// which is the wrong answer to the right question.
    fn run_picker(dialog: &mut Option<Dialog>) -> (String, Option<DialogAction>) {
        let ctx = egui::Context::default();
        // The themed dialogs name Courier Prime explicitly and epaint PANICS on
        // a font family that is not bound.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);
        let placement = Placement {
            parent: None,
            monitor: None,
            caps: crate::host::Caps::PLUGIN,
        };
        let mut action = None;
        let mut painted = String::new();
        for _ in 0..3 {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 520.0))),
                    ..Default::default()
                },
                |ctx| {
                    if let Some(a) = show(ctx, dialog, true, placement) {
                        action = Some(a);
                    }
                },
            );
            painted = painted_text(&out);
        }
        (painted, action)
    }

    /// With 112 plugins installed the filter is the dialog, so it has to find
    /// things the way somebody actually types: any case, and by the maker as
    /// well as the product — "arturia" is how you ask for the eleven of them at
    /// once, and the vendor is not in the name.
    #[test]
    fn the_filter_matches_a_name_or_a_vendor_in_any_case_and_an_empty_one_shows_everything() {
        let plugins = installed();
        let names = |f: &str| {
            visible_plugins(&plugins, f)
                .into_iter()
                .map(|i| plugins[i].name.as_str())
                .collect::<Vec<_>>()
        };

        assert_eq!(names(""), ["Analog Lab V", "Pianoteq 8", "Serum2", "Vital"]);
        assert_eq!(names("   "), names(""), "whitespace is not a filter");
        assert_eq!(names("piano"), ["Pianoteq 8"]);
        assert_eq!(names("PIANO"), ["Pianoteq 8"], "the caps lock case");
        assert_eq!(names(" piano "), ["Pianoteq 8"], "a pasted name has spaces");
        assert_eq!(names("arturia"), ["Analog Lab V"], "not matched on vendor");
        assert_eq!(names("MODARTT"), ["Pianoteq 8"]);
        // A blank vendor must not turn into a wildcard. `contains("")` is true
        // of every string, so the empty needle is ruled out first and the two
        // vendorless rows answer for their names alone.
        assert_eq!(names("ital"), ["Vital"]);
        assert!(names("zzz").is_empty());
    }

    /// The bug this dialog is most likely to grow: a row is highlighted, the
    /// user types, the list becomes a different list, and OK commits either the
    /// row that scrolled out of sight or whatever now sits at that position.
    /// The committed value is an ENTRY, resolved against what is on screen.
    #[test]
    fn filtering_after_a_selection_never_commits_a_row_the_user_can_no_longer_see() {
        let plugins = installed();
        // Vital is highlighted; "an" leaves Analog Lab V and Pianoteq 8, and
        // Vital is not among them.
        let stale = plugin_choice(&plugins, Some(3), None, "an");
        assert_eq!(
            stale, None,
            "Load stayed live over a selection the filter had hidden: {stale:?}"
        );
        // And in particular it is not the filtered list's row 3, nor its row 0
        // — the two shapes the index bug takes.
        assert_ne!(stale, Some(PluginChoice::Load(&plugins[0])));

        // Narrowed to one, the user types four letters and presses Enter. That
        // is how a list of 112 is used, so the single match wins over a stale
        // highlight rather than greying the button out.
        assert_eq!(
            plugin_choice(&plugins, Some(3), None, "piano"),
            Some(PluginChoice::Load(&plugins[1]))
        );
        // A selection the filter still SHOWS is untouched by any of this.
        assert_eq!(
            plugin_choice(&plugins, Some(3), None, "ital"),
            Some(PluginChoice::Load(&plugins[3]))
        );
        // The list is rebuilt from disk on every open, so an index left over
        // from a bundle that has since been deleted selects nothing rather than
        // panicking.
        assert_eq!(plugin_choice(&plugins, Some(99), None, ""), None);
    }

    /// The None row is a real choice — it is how you get the sound back off a
    /// plugin without quitting — so it carries a value of its own rather than
    /// being "cancel with extra steps".
    #[test]
    fn the_none_row_unloads_the_instrument_that_is_loaded() {
        let plugins = installed();
        let loaded = plugins[1].path.clone();
        let choice = plugin_choice(&plugins, None, Some(&loaded), "")
            .expect("the None row is a choice, not an absence of one");
        assert_eq!(choice, PluginChoice::Unload);
        match choice.action() {
            DialogAction::LoadPlugin { path } => {
                assert_eq!(path, None, "the None row loaded something");
            }
            _ => panic!("the None row produced some other action"),
        }
        // Nothing loaded, nothing typed, nothing highlighted: Load is greyed,
        // because unloading nothing is not an action.
        assert_eq!(plugin_choice(&plugins, None, None, ""), None);
        // One plugin installed and an empty filter also leaves "exactly one
        // match" — and if that rule fired here, the None row could never be
        // reached on a machine with a single instrument.
        let one = vec![plugins[1].clone()];
        assert_eq!(
            plugin_choice(&one, None, Some(&loaded), ""),
            Some(PluginChoice::Unload)
        );
    }

    /// An empty list has an ordinary cause — the installer offered AU or VST2
    /// and the user took the default — and an empty box cannot be told apart
    /// from a picker that failed. So the dialog names the folders it looked in,
    /// and this reads what actually reached the screen.
    #[test]
    fn an_empty_plugin_list_says_where_vst3s_live_instead_of_showing_an_empty_box() {
        // The directories are the platform's, decided by the TARGET, so a
        // cross-built Windows binary still names the Windows folder.
        #[cfg(target_os = "macos")]
        assert!(
            vst3_locations().contains("~/Library/Audio/Plug-Ins/VST3")
                && vst3_locations().contains(" /Library/Audio/Plug-Ins/VST3"),
            "{}",
            vst3_locations()
        );
        #[cfg(target_os = "windows")]
        assert!(
            vst3_locations().contains("%COMMONPROGRAMFILES%\\VST3"),
            "{}",
            vst3_locations()
        );
        #[cfg(target_os = "linux")]
        assert!(
            vst3_locations().contains("~/.vst3") && vst3_locations().contains("/usr/lib/vst3"),
            "{}",
            vst3_locations()
        );

        let mut dialog = Some(Dialog::plugin_picker(&[], None));
        let (painted, action) = run_picker(&mut dialog);
        assert!(action.is_none(), "an empty picker loaded something");
        assert!(
            painted.contains(vst3_locations()),
            "the help never reached the screen: {painted}"
        );
        // The warning is not conditional on there being a list. It is the same
        // dialog and it will be read again the moment a plugin is installed.
        assert!(painted.contains(PLUGIN_TRUST_NOTE), "{painted}");
        // And the None row survives an empty folder, because the plugin that is
        // loaded may be the one whose folder has moved — unloading it is the
        // one thing this picker can still do.
        assert!(painted.contains(PLUGIN_NONE_ROW), "{painted}");
        assert!(dialog.is_some(), "the picker closed itself");
    }

    /// Opening on row one, with nothing marked, makes the user answer "what am
    /// I playing through" by reading 112 rows. The loaded bundle is preselected
    /// and marked, and — because it may be ninety rows down — the mark is only
    /// worth anything if the list opens on it, which is what the `focus` flag
    /// also does. This runs the frame inline, the way a plugin editor draws it.
    #[test]
    fn the_loaded_plugin_opens_preselected_and_marked() {
        let bundles: Vec<PathBuf> = [
            "/Library/Audio/Plug-Ins/VST3/Vital.vst3",
            // Lower case on purpose: `discover` sorts paths, so a byte sort
            // would file this after "Vital" and a user would call the list
            // unsorted.
            "/Library/Audio/Plug-Ins/VST3/analog lab v.vst3",
            "/Users/x/Library/Audio/Plug-Ins/VST3/Pianoteq 8.vst3",
        ]
        .iter()
        .map(PathBuf::from)
        .collect();
        let loaded = "/Users/x/Library/Audio/Plug-Ins/VST3/Pianoteq 8.vst3".to_owned();

        let opened = Dialog::plugin_picker(&bundles, Some(loaded.clone()));
        match &opened {
            Dialog::PluginPicker {
                plugins,
                selected,
                current,
                filter,
                focus,
            } => {
                assert_eq!(
                    plugins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
                    ["analog lab v", "Pianoteq 8", "Vital"],
                    "sorted the way a byte comparison sorts, not the way a person reads"
                );
                // The name is the stem: nobody wants to read ".vst3" 112 times.
                assert!(plugins.iter().all(|p| !p.name.contains(".vst3")));
                // Nothing was opened, so nothing knows a vendor. This is the
                // assertion that fails the day the picker starts scanning.
                assert!(plugins.iter().all(|p| p.vendor.is_empty()));
                assert_eq!(*selected, Some(1), "it did not open on what is loaded");
                assert_eq!(current.as_deref(), Some(loaded.as_str()));
                assert!(filter.is_empty() && *focus);
                assert!(
                    plugin_row(&plugins[1], Some(&loaded)).starts_with('\u{2022}'),
                    "the loaded plugin is not marked"
                );
                assert!(
                    plugin_row(&plugins[0], Some(&loaded)).starts_with(' '),
                    "a plugin that is not loaded was marked as if it were"
                );
                // A blank vendor leaves no empty brackets behind; a known one
                // is shown, which is the only reason the field exists.
                assert_eq!(plugin_row(&plugins[2], None), "  Vital");
                assert_eq!(
                    plugin_row(&entry("/x/A.vst3", "A", "Arturia"), None),
                    "  A   (Arturia)"
                );
            }
            _ => panic!("plugin_picker built some other dialog"),
        }

        let mut dialog = Some(opened);
        let (painted, action) = run_picker(&mut dialog);
        assert!(action.is_none(), "the picker loaded a plugin unasked");
        assert!(
            painted.contains("\u{2022} Pianoteq 8"),
            "the mark never reached the screen: {painted}"
        );
        assert!(
            painted.contains(PLUGIN_NONE_ROW) && painted.contains(PLUGIN_SCAN_NOTE),
            "{painted}"
        );
    }

    /// Typing four letters and pressing Enter is how a list of 112 is used, so
    /// the key has to go all the way through the funnel and come out as the
    /// action. What it must NOT do is answer the keystroke that opened the
    /// dialog: inline there is one context and one event queue, and a picker
    /// that confirmed itself before it was read would be a dialog nobody could
    /// see.
    #[test]
    fn enter_loads_the_highlighted_plugin_but_never_on_the_frame_that_opened_the_dialog() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);
        let placement = Placement {
            parent: None,
            monitor: None,
            caps: crate::host::Caps::PLUGIN,
        };
        let held_enter = || egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(700.0, 520.0))),
            events: vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };

        let loaded = "/Users/x/Library/Audio/Plug-Ins/VST3/Pianoteq 8.vst3".to_owned();
        let mut dialog = Some(Dialog::plugin_picker(
            &[
                PathBuf::from("/Library/Audio/Plug-Ins/VST3/Vital.vst3"),
                PathBuf::from(&loaded),
            ],
            Some(loaded.clone()),
        ));

        let mut action = None;
        let _ = ctx.run(held_enter(), |ctx| {
            action = show(ctx, &mut dialog, true, placement);
        });
        assert!(
            action.is_none() && dialog.is_some(),
            "the picker answered the keystroke that opened it"
        );

        let _ = ctx.run(held_enter(), |ctx| {
            action = show(ctx, &mut dialog, true, placement);
        });
        match action {
            Some(DialogAction::LoadPlugin { path }) => {
                assert_eq!(path.as_deref(), Some(loaded.as_str()), "it loaded another");
            }
            _ => panic!("Enter did not load the highlighted plugin"),
        }
        assert!(dialog.is_none(), "it loaded a plugin and stayed open");
    }

    /// The name is what is on screen and the path is what is stored. Two
    /// vendors ship a "Piano", plugins are renamed between versions, and a
    /// settings file holding a display name comes back as nothing at all.
    #[test]
    fn confirming_commits_the_bundle_path_and_never_the_display_name() {
        let plugins = installed();
        let choice =
            plugin_choice(&plugins, Some(1), None, "").expect("a highlighted row is a choice");
        assert_eq!(choice, PluginChoice::Load(&plugins[1]));
        match choice.action() {
            DialogAction::LoadPlugin { path } => {
                assert_eq!(path.as_deref(), Some(plugins[1].path.as_str()));
                assert_ne!(path.as_deref(), Some(plugins[1].name.as_str()));
                assert!(
                    path.as_deref().is_some_and(|p| p.ends_with(".vst3")),
                    "what travelled was not a bundle path: {path:?}"
                );
            }
            _ => panic!("Load produced some other action"),
        }
    }

    /// The two notes get a fixed band so the buttons do not walk under the
    /// cursor as the list is filtered. That only works if what they say fits —
    /// and "it comes to about four lines" is exactly the reasoning that once
    /// pushed a dialog's OK button off its own bottom edge.
    #[test]
    fn the_notes_under_the_list_fit_the_space_reserved_for_them() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        fonts::apply_text_styles(&ctx);

        // Exactly the width the labels are given: the dialog's, less the
        // `Frame` margin on each side.
        let content_w = PLUGIN_DIALOG_W - 2.0 * f32::from(PLUGIN_MARGIN);
        let mut total = 0.0_f32;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(PLUGIN_DIALOG_W, PLUGIN_DIALOG_H),
                )),
                ..Default::default()
            },
            |ctx| {
                crate::shell::viewport_ui(ctx, |ui| {
                    let height = |m: &str| {
                        ui.painter()
                            .layout(
                                m.to_owned(),
                                FontId::new(PLUGIN_NOTE_PT, fonts::courier_bold()),
                                Color32::WHITE,
                                content_w,
                            )
                            .size()
                            .y
                    };
                    // Both are painted, every frame, one under the other.
                    total = height(PLUGIN_TRUST_NOTE)
                        + height(PLUGIN_SCAN_NOTE)
                        + ui.spacing().item_spacing.y;
                });
            },
        );
        assert!(
            total <= PLUGIN_FOOTER_H,
            "{PLUGIN_FOOTER_H}pt is reserved for the notes and they need {total}pt, \
             which would push Load off the bottom edge"
        );
    }
}
