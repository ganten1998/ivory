//! The right-click context menu (spec §6) — the entire UI surface.
//!
//! Qt shows menus as their own top-level popup windows; the fixed 200px-tall
//! main window cannot host a ~460px menu, so the menu is rendered in its own
//! borderless immediate viewport at the global cursor position. An open
//! submenu is a second sibling viewport to its right (Qt-like placement) —
//! one viewport, reused, because only one submenu is ever open.
//!
//! Chrome parity (spec §6.1): bold Courier Prime, per-mode colors, item
//! padding 4px 20px, no rounding, 1px separators, toggle items rename
//! themselves (no checkmarks anywhere).
//!
//! The rows are grouped into CATEGORIES: the top level names a subject
//! (Window, Colors, Keyboard, Chords, Theory, Fretboard) and the hover carries
//! the verbs. It was twenty-six near-flat rows, which is a list to read rather
//! than a menu to aim at. **Two levels, and only two** — see `Entry::Submenu`
//! and `build_entries`, which is written around that limit rather than against
//! it.

use crate::fonts;
use crate::fretboard_panel;
use crate::host::Caps;
use egui::{Button, Color32, CornerRadius, FontId, Margin, Pos2, Rect, Stroke, Vec2};
use ivory_core::fretboard;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorTarget {
    WhiteIdle,
    BlackIdle,
    Active,
    Sustain,
    /// The chord label. Free to change; the bloom around it is the extra.
    ChordText,
    /// The Recorder band's own background.
    ///
    /// The band used to borrow the piano's, so it read as another band of the
    /// same window — which is right until you want to tell them apart at a
    /// glance while playing. Its own colour, and the band's ink follows the
    /// colour's brightness rather than the theme, so any choice stays readable.
    RecorderBg,
}

/// `Clone` but not `Copy`: `SetTuning` carries an owned name, because a custom
/// tuning's name is user input.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MenuAction {
    SetSizePercent(i64),
    ToggleBorderless,
    SelectMidiInput,
    PickColor(ColorTarget),
    ToggleDarkMode,
    ToggleKeytoggle,
    ToggleNotePreference,
    ToggleChordDetection,
    /// Cycle the built-in UI typeface (Courier Prime <-> Terminess).
    CycleFont,
    DetachChordWindow,
    AttachChordWindow,
    TeachChordName,
    ManageTaughtChords,
    /// D-UI-9: correct the current reading, training the learned re-ranker.
    CorrectChordName,
    /// D-UI-9: master switch for the learned re-ranker (weights are kept).
    ToggleChordLearning,
    /// Open the supporter-key dialog.
    ShowSupporterKey,
    /// Supporter decoration: show/hide the pixel heart.
    ToggleHeart,
    /// D-UI-15: the guitar view.
    ToggleFretboard,
    /// Name from `fretboard::TUNINGS`.
    /// `String`, not `&'static str`: a custom tuning's name is user input and
    /// has no static lifetime.
    SetTuning(String),
    /// Open the custom-tuning editor.
    ///
    /// A dialog, not a window, which is why it survives in a plugin for the
    /// same reason "Teach Chord Name..." does: it needs no second OS window, no
    /// device and no size of its own. It is offered only while the fretboard is
    /// showing — an editor for something the user cannot see is a control with
    /// no visible effect.
    EditCustomTuning,
    SetCapo(u8),
    /// Fingerboard wood key (see `fretboard_panel::Wood`).
    SetWood(&'static str),
    /// D-UI-17: turn one theory diagram on or off. Independent, because any
    /// combination of the three may be showing at once.
    ToggleTheoryView(crate::theory_panel::View),
    /// D-UI-17: whether the theory band follows live playing or stays put.
    ToggleTheoryFollowsMidi,
    /// D-UI-16: pop the guitar view into its own window, and put it back.
    DetachFretboard,
    AttachFretboard,
    /// D-UI-17: pop the theory band into its own window, and put it back.
    ///
    /// The third detachable surface, and it follows the other two exactly so
    /// there is one set of habits rather than three — including that
    /// `Caps::detachable` gates it, because a plugin has no window to pop it
    /// into.
    DetachTheory,
    AttachTheory,
    /// §5: show and hide the Recorder band.
    ///
    /// This row is the ONLY way the feature is reachable. `show_recorder`
    /// defaults to false and is deliberately kept out of `first_launch()`, and
    /// this menu is the entire chrome — there is no menu bar and no preferences
    /// dialog — so an earlier draft of the recorder shipped completely
    /// invisible. Every row below exists because of that.
    ///
    /// Hiding also CLOSES the detached window rather than orphaning it, which
    /// is what `ToggleFretboard` already means; the app does the closing, so
    /// the contract lives in the action name.
    ToggleRecorder,
    /// §5: pop the Recorder band into its own window, and put it back.
    ///
    /// The fourth detachable surface and the one with the best reason to be on
    /// a second monitor — a big framing view of the camera while the piano
    /// stays where it was. Gated on `caps.detachable` like the other three.
    DetachRecorder,
    AttachRecorder,
    /// §5: open the Export dialog and its composition selector.
    ///
    /// A dialog rather than a window, so on its own it would survive a plugin
    /// for the same reason "Teach Chord Name..." does. It does not, because it
    /// configures a take that a plugin may not capture or write in the first
    /// place — the whole category goes, not this row.
    ShowExportDialog,
    /// What the audio path is doing: both rates, both buffers, the round trip.
    ShowAudioStatus,
    /// §5: pre-roll countdown in seconds; one of `recorder::PREROLL_CHOICES`.
    /// Count-in length in beats (0 / 4 / 8).
    SetCountIn(u32),
    /// The take's time signature. Drives the click's accent, how long a bar of
    /// count-in is, and the `.mid`'s meta event.
    SetTimeSignature(crate::recorder::TimeSignature),
    /// Write the count-in into the take rather than before it.
    ToggleCountInInTake,
    /// What a take is made of: `auto` / `input` / `plugin` / `both`.
    SetRecordSources(&'static str),
    /// The click.
    ToggleMetronome,
    /// Whether the click is mixed into the FILE as well as the monitors.
    ToggleMetronomeInTake,
    /// §5: hide the running clock while recording.
    ///
    /// After a blinking light, a running timer is the most-cited performance
    /// distraction in the piano forums, and no competitor offers the switch.
    ToggleHideElapsed,
    ShowAbout,
    ResetSettings,
}

/// Everything the menu needs to know to render its labels.
/// `Clone` but no longer `Copy`: `tuning` is a `String` since custom tunings
/// exist. Every call site takes it by value already, so the only cost is one
/// explicit `.clone()` where a view is reused.
#[derive(Clone)]
pub struct MenuView {
    pub dark_mode: bool,
    pub borderless: bool,
    pub keytoggle: bool,
    pub prefer_flats: bool,
    pub detection_enabled: bool,
    pub detached: bool,
    /// "Teach Chord Name..." and "Correct Chord Name..." are greyed when no
    /// notes are held — both act on the voicing you are playing.
    pub notes_held: bool,
    /// D-UI-9: whether the learned re-ranker is currently influencing readings.
    pub learning_on: bool,
    /// Label of the typeface that will be active after the next Font click.
    /// `None` hides the row entirely (only Courier Prime is available here).
    pub next_font: Option<&'static str>,
    /// A valid supporter license is installed. Gates the extras block.
    pub supporter: bool,
    pub heart_on: bool,
    /// D-UI-15: the guitar view is showing. Its Tuning and Capo submenus are
    /// hidden while it is off, rather than offered and inert.
    pub fretboard_on: bool,
    /// The tuning currently in use, by name.
    ///
    /// `String` rather than `&'static str` since custom tunings exist: a
    /// user-entered name has no static lifetime, and the alternative — leaking
    /// it to fake one — would leak a little more on every edit.
    pub tuning: String,
    pub capo: u8,
    pub wood: &'static str,
    /// D-UI-16: the guitar view is in its own window.
    pub fretboard_detached: bool,
    /// D-UI-17: which theory diagrams are showing.
    pub theory: crate::theory_panel::Views,
    /// D-UI-17: whether the theory band follows live playing.
    pub theory_follows_midi: bool,
    /// D-UI-17: the theory band is in its own window.
    ///
    /// Needed for the same reason `fretboard_detached` is: the Detach row
    /// renames itself to Attach, and there are no checkmarks in this menu, so
    /// the label IS the state readout. Without this the row could only ever say
    /// one of the two things, and half of it would be a lie.
    pub theory_detached: bool,
    /// §5: the Recorder band is showing.
    ///
    /// Named to match `fretboard_on` rather than the `show_recorder` setting it
    /// comes from: the two rows behave identically, and this is the label's
    /// input, not the file's key.
    pub recorder_on: bool,
    /// §5: the Recorder band is in its own window.
    ///
    /// Needed for the same reason `theory_detached` is: the Detach row renames
    /// itself to Attach, so the label IS the state readout.
    pub recorder_detached: bool,
    /// §5: the take's time signature, so the count-in submenu can say what a
    /// bar is worth.
    pub time_signature: crate::recorder::TimeSignature,
    /// §5: the count-in length in BARS.
    pub count_in_bars: u32,
    /// §5: whether the count-in is written into the take.
    pub count_in_in_take: bool,
    /// §5: the live count-in, in beats. Comes from `Settings::count_in_beats()`,
    /// which has already clamped a stray value from a later build's file to
    /// something this menu can mark.
    pub count_in_beats: u32,
    /// What a take is made of, verbatim from the settings.
    pub record_sources: String,
    /// The click is running.
    pub metronome_on: bool,
    /// The click is mixed into the recording as well as the monitors.
    pub metronome_in_take: bool,
    /// §5: the running clock is hidden while recording.
    pub hide_elapsed: bool,
    /// What the host allows. Rows whose action needs a window, a device list,
    /// or control of its own size are not shown where they cannot work — an
    /// inert row is worse than an absent one, because the user cannot tell
    /// whether they mis-clicked or the app is broken.
    pub caps: Caps,
}

#[derive(Clone, Copy)]
pub struct MenuColors {
    pub bg: Color32,
    pub text: Color32,
    pub sel: Color32,
    pub sep: Color32,
}

pub fn colors(dark_mode: bool) -> MenuColors {
    if dark_mode {
        MenuColors {
            bg: Color32::from_rgb(0x00, 0x00, 0x00),
            text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            sel: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            sep: Color32::from_rgb(0xE8, 0xDC, 0xC0),
        }
    } else {
        MenuColors {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            text: Color32::from_rgb(0x00, 0x00, 0x00),
            sel: Color32::from_rgb(0xd4, 0xc8, 0xb0),
            sep: Color32::from_rgb(0x00, 0x00, 0x00),
        }
    }
}

const MENU_FONT_SIZE: f32 = 13.0;
const PAD_X: f32 = 20.0; // Qt item padding 4px 20px
const PAD_Y: f32 = 4.0;
const SEP_H: f32 = 3.0; // 1px line + 1px margin above/below
const SIZE_PERCENTS: [i64; 7] = [50, 75, 100, 125, 150, 175, 200];
/// Highest capo offered. Past this it stops being a capo and starts being a
/// different instrument, and the list has to end somewhere.
const CAPO_MAX: u8 = 9;
const ARROW: &str = "\u{23F5}"; // ⏵ submenu indicator
/// The most a menu may be squeezed to fit a short editor before it stops being
/// worth reading and the scroll fallback takes over instead.
const MIN_SQUEEZE: f32 = 0.62;
const MIN_ROW_H: f32 = 13.0;
/// Layers in the inline menu's drop shadow.
const SHADOW_STEPS: u32 = 5;

/// Total height of a menu whose rows are `row_h` tall.
///
/// Separate from the loop that builds `subs` because the squeeze has to know
/// the answer BEFORE choosing `row_h`, and computing it two different ways is
/// how a menu ends up one row taller than the space it was measured for.
fn measured_height(entries: &[Entry], row_h: f32) -> f32 {
    entries
        .iter()
        .map(|e| match e {
            Entry::Separator => SEP_H,
            _ => row_h,
        })
        .sum()
}

/// Pull an open submenu back onto the monitor.
///
/// Slide up rather than off the bottom, and flip to the menu's LEFT rather than
/// off the right edge, which is what a native menu does. `menu_left` is the
/// parent menu's left edge, so the flipped submenu lands against it.
///
/// A function rather than four lines inside `show` because the test used to
/// re-implement it, and a re-implemented clamp passes while the real one is
/// wrong. Grouping the menu into categories made that a much bigger risk:
/// Chords, Theory, Fretboard, Wood, Tuning and Capo all open from rows below
/// the halfway mark, so nearly every hover in the menu now depends on this.
fn clamp_submenu(pos: Pos2, size: Vec2, menu_left: f32, mon: Vec2) -> Pos2 {
    let mut p = pos;
    if p.y + size.y > mon.y {
        p.y = (mon.y - size.y).max(0.0);
    }
    if p.x + size.x > mon.x {
        p.x = (menu_left - size.x).max(0.0);
    }
    p
}

/// One row inside a submenu.
///
/// A `(String, MenuAction)` pair until the menu was grouped into categories.
/// It carries `enabled` for the same reason the top-level `Item` does: "Teach
/// Chord Name..." and "Correct Chord Name..." act on the voicing you are
/// holding and grey out without one, and grouping moved them a level down. A
/// submenu item that could not be greyed would have silently re-enabled them,
/// and the teach dialog would open with nothing to teach.
struct SubItem {
    label: String,
    action: MenuAction,
    enabled: bool,
}

enum Entry {
    Separator,
    Item {
        label: String,
        action: MenuAction,
        enabled: bool,
    },
    /// A parent row that opens a sibling viewport to its right. There used to
    /// be exactly one of these (Size), hard-coded from the entry list all the
    /// way down to the viewport; the guitar view needs two more, so the whole
    /// path is now driven by the list.
    ///
    /// **Items, not entries: this is the last level.** A submenu holds rows and
    /// nothing else, and `show` draws exactly one sibling popup, so there is no
    /// third level to nest into. Every category below is designed around that.
    Submenu {
        label: String,
        items: Vec<SubItem>,
    },
}

/// Push a category, as a hover when there is something worth hovering for.
///
/// The shape follows the content, which is what keeps `Caps` honest: under
/// `Caps::PLUGIN` the Window category loses Borderless, Keyboard loses its
/// device row and Fretboard loses Detach, so a category can arrive here with
/// one item or none. An empty hover is a dead end the user cannot tell from a
/// bug, and a one-item hover charges a hover to reach a single row. So nothing
/// vanishes, one item is drawn as an ordinary row, and two or more open.
///
/// It also gives the fretboard block its shape for free: with the guitar view
/// off there is only "Show Fretboard" to say, so the category IS that row.
fn push_category(entries: &mut Vec<Entry>, label: &str, mut items: Vec<SubItem>) {
    match items.len() {
        0 => {}
        1 => {
            let only = items.remove(0);
            entries.push(Entry::Item {
                label: only.label,
                action: only.action,
                enabled: only.enabled,
            });
        }
        _ => entries.push(Entry::Submenu {
            label: label.to_owned(),
            items,
        }),
    }
}

/// Where an open submenu goes and how big it is. Measured at open time with
/// the rest of the menu, because a Qt menu is static once it is showing.
struct SubGeom {
    row_top: f32,
    size: Vec2,
}

pub struct MenuState {
    pos: Pos2, // global (monitor points), top-left
    size: Vec2,
    entries: Vec<Entry>,
    row_h: f32, // uniform item height; buttons are forced to it so the
    // stacked rows exactly fill the computed viewport size
    /// Geometry per submenu, in entry order.
    subs: Vec<SubGeom>,
    /// Which submenu is showing, as an index into `subs`. At most one, which
    /// is why they can all share a single viewport id.
    submenu_open: Option<usize>,
    /// A submenu the pointer is on but which has not been switched to yet.
    ///
    /// **The diagonal-travel fix.** Submenus open to the RIGHT, so reaching one
    /// means moving right and usually down or up — and every row crossed on the
    /// way is a row that hovers. Without a dwell, the submenu under the pointer
    /// changes on the way to the one that was wanted, and what opens is
    /// whichever row the path happened to end on. The owner's report was that
    /// hovering anything gave the TOP category about eight times in ten.
    ///
    /// So a switch to a DIFFERENT submenu waits until the pointer has stayed on
    /// the new row. Opening the first one is instant, because there is nothing
    /// being travelled away from.
    pending_sub: Option<(usize, Instant)>,
    /// Where the pointer first was, in the menu's own coordinates.
    ///
    /// **The menu opens AT the cursor**, so on the frame it appears the pointer
    /// is already sitting on its first row — and the first row is a submenu.
    /// That is the whole of "it always opens the top one": nobody pointed at
    /// Window, the menu was placed under a pointer that happened to be there.
    first_pointer: Option<Pos2>,
    /// Whether the pointer has moved far enough since then to count as
    /// pointing at something. Sticky once true.
    armed: bool,
    /// Kept so a submenu can be clamped too, not just the menu. Tuning and
    /// Capo sit near the BOTTOM of a long menu and Capo is ten rows deep, so
    /// unclamped they run off the screen and their lower rows cannot be
    /// clicked. Size never hit this: it is the first row and seven rows tall.
    monitor: Option<Vec2>,
    dark_mode: bool,
    opened_at: Instant,
    saw_focus: bool,
    /// Text size multiplier, below 1.0 only when the menu had to be squeezed
    /// to fit a short editor. Applied to the row font so the labels shrink
    /// with the rows rather than overflowing them.
    font_scale: f32,
    /// Captured at open time, not read from the app each frame, so a menu can
    /// never be half-drawn as a window and half as a layer.
    caps: Caps,
}

/// Stable surface identities. On the desktop these are viewport ids; in a
/// plugin they are `Area` ids. One string each, so the two paths cannot drift.
/// Decide which submenu is open, given what the pointer is on.
///
/// Its own function because it is the whole of the bug the owner reported and
/// none of it is about drawing: hovering a submenu opened the TOP category
/// about eight times in ten, because reaching a panel that opens to the RIGHT
/// means crossing the rows in between, and each one hovered.
///
/// Returns whether a switch is still pending, so the caller can ask for the
/// frame that will complete it.
fn settle_submenu(
    open: &mut Option<usize>,
    pending: &mut Option<(usize, Instant)>,
    hovered: Option<usize>,
    hovered_plain_row: bool,
    armed: bool,
    still: bool,
    now: Instant,
) -> bool {
    // **Nothing at all until the pointer has moved.** The menu is placed under
    // the cursor, so the row it appears beneath was never chosen — acting on it
    // is how every single opening produced the top submenu.
    if !armed {
        return false;
    }
    match hovered {
        // Already showing: nothing to decide, and any pending switch is stale.
        Some(i) if *open == Some(i) => {
            *pending = None;
            false
        }
        // Nothing open yet, so there is no journey to protect. Instant, because
        // a delay on the FIRST submenu would just feel like lag.
        Some(i) if open.is_none() => {
            *open = Some(i);
            *pending = None;
            false
        }
        // A different one, with a submenu already open. The case that was
        // firing by accident.
        // A pointer that has STOPPED on a row has arrived; one still moving is
        // in transit across it. Stillness rather than elapsed time is what
        // makes this work at all: a stationary pointer produces no events, so a
        // switch waiting only on a timer can sit unfinished on the row you
        // meant to leave.
        Some(i) if still => {
            *open = Some(i);
            *pending = None;
            false
        }
        Some(i) => match *pending {
            Some((j, at)) if j == i && now.duration_since(at) >= SUB_SWITCH_DWELL => {
                *open = Some(i);
                *pending = None;
                false
            }
            Some((j, _)) if j == i => true,
            _ => {
                *pending = Some((i, now));
                true
            }
        },
        None => {
            *pending = None;
            // A plain row — one with no submenu — closes whatever is open. The
            // pointer being nowhere at all does not: that is what passing over
            // the gap between the menu and the panel looks like.
            if hovered_plain_row {
                *open = None;
            }
            false
        }
    }
}

/// How long the pointer must stay on a different submenu row before it opens.
///
/// Short enough to feel immediate when it is where you meant to go, long enough
/// to survive crossing two or three rows on the way to the one that is already
/// open. Apple's own menus use a shape (a triangle toward the open panel)
/// rather than a delay; a delay is a fraction of the code and covers the same
/// gesture, and unlike the triangle it also survives a submenu that opened to
/// the LEFT because it was clamped against the screen edge.
const SUB_SWITCH_DWELL: std::time::Duration = std::time::Duration::from_millis(140);

/// How far the pointer must move before the menu will act on what it is over.
///
/// Small: the point is only to tell "the menu appeared under my cursor" from
/// "I moved onto a row". Half a row is plenty and a whole row would make the
/// first deliberate hover feel dead.
const ARM_SLOP: f32 = 6.0;

const MENU_ID: &str = "ivory-menu";
const SUBMENU_ID: &str = "ivory-menu-sub";

/// The menu, as categories.
///
/// It was twenty-six rows deep and almost entirely flat, which is a list to
/// read rather than a menu to aim at. Everything that belongs together is now
/// one hover: the top level names the SUBJECT, the hover carries the verbs.
///
/// Two rules shape what follows, and both are the type's, not a preference:
/// a submenu holds items and not more submenus (`Entry::Submenu`), and a
/// category with nothing in it must not draw as an empty hover
/// (`push_category`). Where those two collide with the tidy grouping, the
/// comment at the collision says which one won.
fn build_entries(view: MenuView) -> Vec<Entry> {
    let item = |label: &str, action: MenuAction| Entry::Item {
        label: label.to_owned(),
        action,
        enabled: true,
    };
    let row = |label: &str, action: MenuAction| SubItem {
        label: label.to_owned(),
        action,
        enabled: true,
    };
    let mut e = Vec::new();

    // ── Window ─────────────────────────────────────────────────────────────
    // Size was a submenu of its own, and it CANNOT become a child of Window:
    // `Entry::Submenu` holds items, not submenus, and `show` draws exactly one
    // sibling popup — there is no third level, and inventing one would rewrite
    // the placement, clamping and hover-tracking path all at once. So the
    // percents are Window's own items. Same seven choices, same actions, still
    // one hover from the top, and Borderless joins them because it is the only
    // other thing there is to say about the window.
    let mut window: Vec<SubItem> = Vec::new();
    if view.caps.size_presets {
        // Offered wherever a size can be CHOSEN, which includes a plugin: it
        // cannot set its own window, but it can ask, and VST3 has a path for
        // exactly that. Leaving it out was the difference between an editor you
        // can read and one you cannot.
        window.extend(SIZE_PERCENTS.iter().map(|&p| SubItem {
            label: format!("{p}%"),
            action: MenuAction::SetSizePercent(p),
            enabled: true,
        }));
    }
    // Borderless is a different question with a different answer: window
    // chrome belongs to whoever owns the window, and in a plugin that is the
    // host. Label shows what you would switch TO.
    if view.caps.window_sizing {
        window.push(row(
            if view.borderless {
                "Bordered"
            } else {
                "Borderless"
            },
            MenuAction::ToggleBorderless,
        ));
    }
    push_category(&mut e, "Window", window);

    // Dark Mode is deliberately NOT filed under a category, and it is the one
    // row that isn't.
    //
    // It is the most-flipped item in the menu, so a hover to reach it is a toll
    // paid all day; and it is the row `app.rs`'s plugin test clicks BY LABEL
    // through `rows_for_test` to prove menu rows are alive at all in an editor,
    // which only works while it is a top-level item. Moving it into a hover
    // means updating that test in the same change, or the only end-to-end proof
    // that the plugin menu works at all goes red.
    e.push(item(
        if view.dark_mode {
            "Light Mode"
        } else {
            "Dark Mode"
        },
        MenuAction::ToggleDarkMode,
    ));
    // Only offered when a second typeface is actually installed, matching how
    // Detach appears conditionally rather than showing a dead row. One row, so
    // no category: a "Text" hover holding a single typeface name would cost a
    // hover and save nothing.
    if let Some(next) = view.next_font {
        e.push(item(next, MenuAction::CycleFont));
    }

    // ── Colors ─────────────────────────────────────────────────────────────
    // Five flat rows and a separator, now one hover. Spelled the American way
    // because every label inside it already is ("Set White Key Color...") and a
    // "Colours" hover full of "Color..." items reads like a bug.
    push_category(
        &mut e,
        "Colors",
        vec![
            row(
                "Set White Key Color...",
                MenuAction::PickColor(ColorTarget::WhiteIdle),
            ),
            row(
                "Set Black Key Color...",
                MenuAction::PickColor(ColorTarget::BlackIdle),
            ),
            row(
                "Set Active Key Color...",
                MenuAction::PickColor(ColorTarget::Active),
            ),
            row(
                "Set Sustain Color...",
                MenuAction::PickColor(ColorTarget::Sustain),
            ),
            row(
                "Set Chord Color...",
                MenuAction::PickColor(ColorTarget::ChordText),
            ),
            row(
                "Set Recorder Color...",
                MenuAction::PickColor(ColorTarget::RecorderBg),
            ),
        ],
    );
    e.push(Entry::Separator);

    // ── Keyboard ───────────────────────────────────────────────────────────
    let mut keyboard = Vec::new();
    // A plugin is handed its notes by the host and has no device to choose.
    if view.caps.midi_ports {
        keyboard.push(row("Select MIDI Input...", MenuAction::SelectMidiInput));
    }
    keyboard.push(row(
        if view.keytoggle {
            "Disable Keytoggle"
        } else {
            "Enable Keytoggle"
        },
        MenuAction::ToggleKeytoggle,
    ));
    // Note spelling sits with the keys rather than with the chords. It respells
    // every name in the app, not just chord names, and the keyboard is where
    // the user meets it first.
    keyboard.push(row(
        if view.prefer_flats {
            "Use Sharps (A#)"
        } else {
            "Use Flats (Bb)"
        },
        MenuAction::ToggleNotePreference,
    ));
    push_category(&mut e, "Keyboard", keyboard);

    // ── Chords ─────────────────────────────────────────────────────────────
    // The detector, its window, and everything that teaches it — six rows and
    // three separators' worth of top level, all of it about one subject.
    let mut chords = Vec::new();
    if view.detached && view.caps.detachable {
        chords.push(row("Attach Chord Window", MenuAction::AttachChordWindow));
    } else {
        chords.push(row(
            if view.detection_enabled {
                "Disable Chord Detection"
            } else {
                "Enable Chord Detection"
            },
            MenuAction::ToggleChordDetection,
        ));
        if view.detection_enabled && view.caps.detachable {
            chords.push(row("Detach Chord Window", MenuAction::DetachChordWindow));
        }
    }
    // D-UI-5: "Teach Chord Name..." is greyed only when no notes are held;
    // "Manage Taught Chords..." is always available.
    chords.push(SubItem {
        label: "Teach Chord Name...".to_owned(),
        action: MenuAction::TeachChordName,
        enabled: view.notes_held,
    });
    chords.push(row(
        "Manage Taught Chords...",
        MenuAction::ManageTaughtChords,
    ));
    // D-UI-9: the learned re-ranker. Forgetting what was learned lives in
    // "Manage Taught Chords...", one step away from the button that trains.
    chords.push(SubItem {
        label: "Correct Chord Name...".to_owned(),
        action: MenuAction::CorrectChordName,
        // Needs both a voicing AND a visible reading: with detection off,
        // detection_tick() nulls current_chord, so the dialog would show
        // "Now reads: (none)" and the result would land somewhere invisible.
        enabled: view.notes_held && view.detection_enabled,
    });
    // The toggle renames itself like every other toggle here (Qt parity — no
    // checkmarks anywhere).
    chords.push(row(
        if view.learning_on {
            "Disable Chord Learning"
        } else {
            "Enable Chord Learning"
        },
        MenuAction::ToggleChordLearning,
    ));
    push_category(&mut e, "Chords", chords);

    // ── Theory ─────────────────────────────────────────────────────────────
    // D-UI-17: the theory band. Each row renames itself the way every other
    // toggle here does, so the hover says what is showing without a checkmark
    // column.
    let mut theory: Vec<SubItem> = crate::theory_panel::View::ALL
        .iter()
        .map(|v| SubItem {
            label: if v.is_on(view.theory) {
                format!("Hide {}", v.label())
            } else {
                format!("Show {}", v.label())
            },
            action: MenuAction::ToggleTheoryView(*v),
            enabled: true,
        })
        .collect();
    // Whether the band tracks your playing sits with the diagrams rather than
    // in the keyboard block, because it is a property of this display and of
    // nothing else.
    theory.push(row(
        if view.theory_follows_midi {
            "Stop Following MIDI"
        } else {
            "Follow MIDI"
        },
        MenuAction::ToggleTheoryFollowsMidi,
    ));
    // D-UI-17: the third detachable surface. Last in the hover, not second like
    // the fretboard's, because the diagram toggles above it are what the band
    // IS — where it lives is the afterthought.
    if view.caps.detachable {
        theory.push(row(
            if view.theory_detached {
                "Attach Theory"
            } else {
                "Detach Theory"
            },
            if view.theory_detached {
                MenuAction::AttachTheory
            } else {
                MenuAction::DetachTheory
            },
        ));
    }
    push_category(&mut e, "Theory", theory);

    // ── Fretboard ──────────────────────────────────────────────────────────
    // D-UI-15: the guitar view. Its own subject, because it is a second
    // instrument rather than another chord-display option. While it is off the
    // category collapses to the one row there is to show — a Custom Tuning
    // item on a hidden fretboard is a control for something you cannot see.
    let mut fretboard = vec![row(
        if view.fretboard_on {
            "Hide Fretboard"
        } else {
            "Show Fretboard"
        },
        MenuAction::ToggleFretboard,
    )];
    if view.fretboard_on {
        // Mirrors the chord window's Detach/Attach exactly, so there is one
        // set of habits rather than two.
        if view.caps.detachable {
            fretboard.push(row(
                if view.fretboard_detached {
                    "Attach Fretboard"
                } else {
                    "Detach Fretboard"
                },
                if view.fretboard_detached {
                    MenuAction::AttachFretboard
                } else {
                    MenuAction::DetachFretboard
                },
            ));
        }
        fretboard.push(row("Custom Tuning...", MenuAction::EditCustomTuning));
    }
    push_category(&mut e, "Fretboard", fretboard);
    if view.fretboard_on {
        // Wood, Tuning and Capo stay TOP-LEVEL hovers, immediately under the
        // Fretboard row and inside the same separator block so they still read
        // as its business.
        //
        // They are lists of choices — three woods, every shipped tuning, ten
        // frets — so each is already a submenu, and a submenu cannot hold
        // another one (see `Entry::Submenu`). The alternatives were both worse
        // than a sibling: flattening ~20 choices into the Fretboard hover, or
        // inventing a third menu level for three rows. Siblings it is.
        push_category(
            &mut e,
            "Wood",
            fretboard_panel::Wood::ALL
                .iter()
                .map(|w| SubItem {
                    label: if w.key() == view.wood {
                        format!("{}  \u{2022}", w.label())
                    } else {
                        w.label().to_owned()
                    },
                    action: MenuAction::SetWood(w.key()),
                    enabled: true,
                })
                .collect(),
        );
        push_category(
            &mut e,
            "Tuning",
            fretboard::TUNINGS
                .iter()
                .map(|t| SubItem {
                    // The current one is marked rather than hidden: a submenu
                    // that never says what is selected makes you close it again
                    // to find out.
                    label: if t.name == view.tuning {
                        format!("{}  \u{2022}", t.name)
                    } else {
                        t.name.to_string()
                    },
                    action: MenuAction::SetTuning(t.name.to_string()),
                    enabled: true,
                })
                .collect(),
        );
        push_category(
            &mut e,
            "Capo",
            (0..=CAPO_MAX)
                .map(|f| {
                    let label = if f == 0 {
                        "No Capo".to_owned()
                    } else {
                        format!("Fret {f}")
                    };
                    SubItem {
                        label: if f == view.capo {
                            format!("{label}  \u{2022}")
                        } else {
                            label
                        },
                        action: MenuAction::SetCapo(f),
                        enabled: true,
                    }
                })
                .collect(),
        );
    }

    // ── Recorder ───────────────────────────────────────────────────────────
    // §5. LAST of the subjects, after the whole display block, and the position
    // is the argument rather than an accident.
    //
    // Window, Keyboard, Chords, Theory and Fretboard are all answers to "what
    // is on screen" — asked and re-asked all day, which is why they are near
    // the top. The recorder is a MODE the user enters: it opens a camera,
    // claims 200 points of the window and ends in files on disk. You go to it
    // deliberately, once, and then you are in it; putting it among the display
    // toggles would make it one more thing to scan past every time somebody
    // wants dark mode. The plan's own control-order thinking is the same shape:
    // the things you touch constantly come first, and the things you set up
    // once come after.
    //
    // It also physically cannot go between Fretboard and Wood/Tuning/Capo,
    // which are siblings that have to stay adjacent to the row they belong to.
    //
    // The whole category is gated on `caps.capture_devices` rather than row by
    // row: a plugin has no business opening a camera behind the DAW's back, and
    // a Minimal build has not linked the code to. Absent, not
    // offered-and-greyed — an inert row is worse than a missing one, because
    // the user cannot tell it from a bug.
    if view.caps.capture_devices {
        // Renames itself rather than carrying a checkmark, per the chrome rule
        // at the top of this file. (Wood/Tuning/Capo use a `•` marker instead,
        // and that is not an exception to it: those are lists of CHOICES, where
        // the question is which one, not whether.)
        let mut recorder = vec![row(
            if view.recorder_on {
                // "Hide" closes the detached window rather than orphaning it,
                // exactly as "Hide Fretboard" does. The app does the closing;
                // what matters here is that both states are one action, so
                // there is never a hidden band with a live window beside it.
                "Hide Recorder"
            } else {
                "Show Recorder"
            },
            MenuAction::ToggleRecorder,
        )];
        if view.recorder_on {
            // Detach needs `caps.detachable` ON TOP of `capture_devices`: a
            // host may be able to open a camera and still have no second window
            // to put the framing view in. Same gate the fretboard and theory
            // detach rows use, so there is one set of habits rather than four.
            if view.caps.detachable {
                recorder.push(row(
                    if view.recorder_detached {
                        "Attach Recorder"
                    } else {
                        "Detach Recorder"
                    },
                    if view.recorder_detached {
                        MenuAction::AttachRecorder
                    } else {
                        MenuAction::DetachRecorder
                    },
                ));
            }
            // No instrument rows here. They live in the band — three visible
            // slots, each with its own volume and its own button to open that
            // plugin's window — because that is where they are usable and
            // because a right-click submenu is where the owner could not find
            // them. The band is also the only place they COULD live: the
            // monitor engine's life is tied to the band being open, so there is
            // no state in which a menu row could load an instrument and the
            // band could not show it.
            recorder.push(row(
                if view.metronome_on {
                    "Stop the Click"
                } else {
                    "Start the Click"
                },
                MenuAction::ToggleMetronome,
            ));
            recorder.push(row(
                // Worded as what it DOES rather than as a state, because the
                // consequence is the whole point: a click in the file is a
                // ruined take and the default is off for that reason.
                if view.metronome_in_take {
                    "Keep the Click Out of Recordings"
                } else {
                    "Record the Click Into Takes"
                },
                MenuAction::ToggleMetronomeInTake,
            ));
            recorder.push(row(
                // Worded as what it does, like the click row above it. The
                // consequence is the whole point: with it on, Record starts
                // writing IMMEDIATELY and the count is at the head of the file.
                if view.count_in_in_take {
                    "Count In Before the Take Starts"
                } else {
                    "Record the Count-in Into the Take"
                },
                MenuAction::ToggleCountInInTake,
            ));
            recorder.push(row("Audio Status...", MenuAction::ShowAudioStatus));
            recorder.push(row("Export...", MenuAction::ShowExportDialog));
            recorder.push(row(
                if view.hide_elapsed {
                    "Show Elapsed Time"
                } else {
                    "Hide Elapsed Time"
                },
                MenuAction::ToggleHideElapsed,
            ));
        }
        push_category(&mut e, "Recorder", recorder);
        if view.recorder_on {
            // What a take is made of. A sibling hover for the same reason the
            // count-in is one: it is a list of choices.
            //
            // It exists at all because its absence was a bug people hit: with
            // no control, the stored default decided, and the default recorded
            // the microphone and left the instrument you could plainly hear out
            // of the file.
            //
            // "Sources" and not "Record": this sits directly under "Recorder",
            // and two rows reading "Recorder" and "Record" are one glance apart
            // from being the same word.
            push_category(
                &mut e,
                "Sources",
                [
                    ("auto", "Everything there is"),
                    ("plugin", "Instruments only"),
                    ("input", "Audio input only"),
                    ("both", "Instruments + input"),
                ]
                .into_iter()
                .map(|(key, label)| SubItem {
                    label: if key == view.record_sources {
                        format!("{label}  \u{2022}")
                    } else {
                        label.to_owned()
                    },
                    action: MenuAction::SetRecordSources(key),
                    enabled: true,
                })
                .collect(),
            );
            // The signature the click, the count-in and the `.mid` all share.
            // A sibling hover for the same reason the count-in is one.
            push_category(
                &mut e,
                "Time signature",
                crate::recorder::TIME_SIGNATURES
                    .into_iter()
                    .map(|sig| SubItem {
                        label: if sig == view.time_signature {
                            format!("{}  \u{2022}", sig.label())
                        } else {
                            sig.label()
                        },
                        action: MenuAction::SetTimeSignature(sig),
                        enabled: true,
                    })
                    .collect(),
            );
            // A SIBLING hover, for exactly the reason Wood/Tuning/Capo are:
            // the count-in is a list of choices, so it is already a submenu, and a
            // submenu cannot hold another one (see `Entry::Submenu`). Offered
            // only while the band is showing — a countdown length for a
            // recorder you cannot see is a control with no visible effect.
            push_category(
                &mut e,
                "Count-in",
                crate::recorder::COUNT_IN_CHOICES
                    .into_iter()
                    .map(|bars| {
                        // Bars, with the beat count in brackets — the signature
                        // decides it now, so "2 bars" alone leaves you doing
                        // the multiplication and "12 beats" alone leaves you
                        // doing the division.
                        let label = if bars == 0 {
                            "No count-in".to_owned()
                        } else {
                            let beats = view.time_signature.beats_in(bars);
                            let word = if bars == 1 { "bar" } else { "bars" };
                            format!(
                                "{bars} {word}  ({beats} beats of {})",
                                view.time_signature.label()
                            )
                        };
                        SubItem {
                            // Marked rather than hidden, like Tuning and Capo: a
                            // submenu that never says what is selected makes you
                            // close it again to find out.
                            label: if bars == view.count_in_bars {
                                format!("{label}  \u{2022}")
                            } else {
                                label
                            },
                            action: MenuAction::SetCountIn(bars),
                            enabled: true,
                        }
                    })
                    .collect(),
            );
        }
    }

    // ── The rows that are not a category ───────────────────────────────────
    // Three one-shot actions and the supporter pair. None of them groups with
    // anything: burying "About" under a "Help" hover would be a hover invented
    // to hold one row.
    e.push(Entry::Separator);
    e.push(item(
        if view.supporter {
            "Supporter Key..."
        } else {
            "Support Tangent..."
        },
        MenuAction::ShowSupporterKey,
    ));
    if view.supporter {
        // Stays beside the supporter row rather than under Colors: it is a
        // visibility toggle for a decoration, not a colour, and it only exists
        // for the person the row above it is addressed to.
        e.push(item(
            if view.heart_on {
                "Hide Heart"
            } else {
                "Show Heart"
            },
            MenuAction::ToggleHeart,
        ));
    }
    e.push(Entry::Separator);
    e.push(item("About", MenuAction::ShowAbout));
    e.push(item("Reset Settings to Default", MenuAction::ResetSettings));
    e
}

impl MenuState {
    /// Snapshot labels and measure geometry at open time (Qt menus are static
    /// while shown). `global_pos` is monitor-space points.
    pub fn open(
        ctx: &egui::Context,
        view: MenuView,
        global_pos: Pos2,
        monitor_size: Option<Vec2>,
    ) -> Self {
        let entries = build_entries(view.clone());
        let font = FontId::new(MENU_FONT_SIZE, fonts::courier_bold());

        let measure = |ctx: &egui::Context, text: &str| -> Vec2 {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                    .size()
            })
        };

        let mut text_h: f32 = 0.0;
        let mut max_w: f32 = 0.0;
        let arrow_w = measure(ctx, ARROW).x;
        for entry in &entries {
            match entry {
                Entry::Separator => {}
                Entry::Item { label, .. } => {
                    let sz = measure(ctx, label);
                    text_h = text_h.max(sz.y);
                    max_w = max_w.max(sz.x);
                }
                Entry::Submenu { label, .. } => {
                    let sz = measure(ctx, label);
                    text_h = text_h.max(sz.y);
                    // text + gap + arrow
                    max_w = max_w.max(sz.x + 12.0 + arrow_w);
                }
            }
        }
        let mut row_h = (text_h + 2.0 * PAD_Y).ceil();
        let width = (max_w + 2.0 * PAD_X).ceil();

        // Squeeze to fit, when there is a hard ceiling.
        //
        // A menu that scrolls is a menu that LOOKS cut off — the rows past the
        // edge are simply absent, and nothing on screen says to scroll. On a
        // desktop it never comes up: the menu is its own window and may be
        // taller than the app. In a plugin editor, which can easily be shorter
        // than the ~550 points this menu wants, it came up immediately.
        //
        // Menus are static once shown, so the fix is to measure first and
        // shrink the rows until the whole thing fits. Down to a floor: below
        // about nine points it stops being readable, and at that point the
        // scroll fallback in `shell::surface` takes over rather than shrinking
        // into illegibility.
        let mut font_scale = 1.0_f32;
        if let Some(mon) = monitor_size {
            if measured_height(&entries, row_h) > mon.y {
                // Solved, not approximated. Separators are a fixed 3 points
                // and do not shrink, so scaling the whole height by a ratio
                // leaves the menu a few points too tall — which is the one
                // outcome this is trying to avoid.
                let n_sep = entries
                    .iter()
                    .filter(|e| matches!(e, Entry::Separator))
                    .count() as f32;
                let n_row = entries.len() as f32 - n_sep;
                if n_row >= 1.0 {
                    let room = mon.y - n_sep * SEP_H;
                    let fitted = (room / n_row).floor();
                    let floor = (row_h * MIN_SQUEEZE).max(MIN_ROW_H);
                    let chosen = fitted.max(floor).min(row_h);
                    font_scale = (chosen / row_h).clamp(MIN_SQUEEZE, 1.0);
                    row_h = chosen;
                }
            }
        }

        let mut height = 0.0;
        let mut subs: Vec<SubGeom> = Vec::new();
        for entry in &entries {
            match entry {
                Entry::Separator => height += SEP_H,
                Entry::Item { .. } => height += row_h,
                Entry::Submenu { items, .. } => {
                    let w = items
                        .iter()
                        .fold(0.0_f32, |acc, it| acc.max(measure(ctx, &it.label).x));
                    subs.push(SubGeom {
                        row_top: height,
                        size: Vec2::new((w + 2.0 * PAD_X).ceil(), items.len() as f32 * row_h),
                    });
                    height += row_h;
                }
            }
        }

        // Best-effort clamp to the monitor.
        let mut pos = global_pos;
        if let Some(mon) = monitor_size {
            if pos.x + width > mon.x {
                pos.x = (mon.x - width).max(0.0);
            }
            if pos.y + height > mon.y {
                pos.y = (mon.y - height).max(0.0);
            }
        }

        Self {
            pos,
            size: Vec2::new(width, height),
            entries,
            row_h,
            font_scale,
            subs,
            submenu_open: None,
            pending_sub: None,
            first_pointer: None,
            armed: false,
            monitor: monitor_size,
            dark_mode: view.dark_mode,
            opened_at: Instant::now(),
            saw_focus: false,
            caps: view.caps,
        }
    }
}

/// The clickable rows, in draw order, for a test that wants to click one.
///
/// Separators and submenu parents are excluded: a separator cannot be clicked
/// and a submenu parent opens rather than acts.
/// The menu's natural size, for a test that needs to know whether it is taller
/// than the editor it is being drawn into.
#[doc(hidden)]
pub fn size_for_test(state: &MenuState) -> Vec2 {
    state.size
}

#[doc(hidden)]
pub fn row_height_for_test(state: &MenuState) -> f32 {
    state.row_h
}

#[doc(hidden)]
pub fn rows_for_test(view: MenuView) -> Vec<(String, MenuAction)> {
    build_entries(view)
        .into_iter()
        .filter_map(|e| match e {
            Entry::Item { label, action, .. } => Some((label, action)),
            _ => None,
        })
        .collect()
}

/// The centre of clickable row `idx`, in the same coordinates `show` draws in.
///
/// Walks the entry list exactly as the row loop does — separators are `SEP_H`
/// tall, everything else is `row_h` — so a test clicks where the row actually
/// is rather than where it is assumed to be.
#[doc(hidden)]
pub fn row_center_for_test(state: &MenuState, idx: usize) -> Pos2 {
    let mut y = state.pos.y;
    let mut seen = 0usize;
    for entry in &state.entries {
        match entry {
            Entry::Separator => y += SEP_H,
            Entry::Item { .. } => {
                if seen == idx {
                    return Pos2::new(state.pos.x + state.size.x * 0.5, y + state.row_h * 0.5);
                }
                seen += 1;
                y += state.row_h;
            }
            Entry::Submenu { .. } => y += state.row_h,
        }
    }
    Pos2::new(state.pos.x + state.size.x * 0.5, y)
}

/// The menu's own background, and the edge it needs to look like an object.
///
/// On the desktop this is deliberately borderless: the menu is its own OS
/// window, so the window edge and its drop shadow already separate it from
/// whatever is behind, and a drawn border on top of that looks heavy.
///
/// Drawn INLINE there is no window and no shadow — and the menu's background
/// is the same cream as the app's. The result was a menu with no visible
/// extent at all: the rows read as text lying across the theory band and the
/// separators as stray lines through it, which is what "the menu is cut off"
/// looks like when nothing is actually missing. So inline it gets a real
/// border and a shadow, and becomes a thing sitting on top of the app.
fn draw_menu_backdrop(painter: &egui::Painter, rect: Rect, c: MenuColors, caps: Caps) {
    if !caps.child_windows {
        // A soft shadow, down and to the right, built from a few translucent
        // rects. Cheaper than a blur and enough to lift the menu off the page.
        for i in (1..=SHADOW_STEPS).rev() {
            let k = i as f32;
            painter.rect_filled(
                rect.translate(Vec2::splat(k * 0.9)).expand(k * 0.4),
                2.0,
                Color32::from_black_alpha((14.0 / k) as u8),
            );
        }
    }
    painter.rect_filled(rect, 0.0, c.bg);
    painter.rect_stroke(
        rect.shrink(0.5),
        0.0,
        // Borderless on the desktop (the window edge does the work), a real
        // 1px edge inline.
        Stroke::new(1.0_f32, if caps.child_windows { c.bg } else { c.sep }),
        egui::StrokeKind::Middle,
    );
}

/// Shrink the row text along with the rows, when the menu had to be squeezed.
///
/// Without this the rows get shorter and the labels do not, so they overflow
/// their own row and the menu looks broken rather than compact. Padding comes
/// down with it, or the text has no room left inside the row at all.
fn scale_menu_font(style: &mut egui::Style, scale: f32) {
    if scale >= 0.999 {
        return;
    }
    let size = (MENU_FONT_SIZE * scale).max(8.0);
    style.text_styles.insert(
        egui::TextStyle::Button,
        FontId::new(size, fonts::courier_bold()),
    );
    style.spacing.button_padding = egui::vec2(PAD_X * scale, (PAD_Y * scale).max(1.0));
}

fn apply_menu_style(style: &mut egui::Style, c: MenuColors) {
    style.spacing.button_padding = egui::vec2(PAD_X, PAD_Y);
    style.spacing.item_spacing = egui::vec2(0.0, 0.0);
    style.spacing.menu_margin = Margin::ZERO;
    style.spacing.window_margin = Margin::ZERO;
    style.spacing.interact_size = egui::vec2(0.0, 0.0);

    let set = |wv: &mut egui::style::WidgetVisuals, bg: Color32, fg: Color32| {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::NONE;
        wv.fg_stroke = Stroke::new(1.0_f32, fg);
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
    };
    set(
        &mut style.visuals.widgets.inactive,
        Color32::TRANSPARENT,
        c.text,
    );
    set(&mut style.visuals.widgets.hovered, c.sel, c.text);
    set(&mut style.visuals.widgets.active, c.sel, c.text);
    set(&mut style.visuals.widgets.open, c.sel, c.text);
    set(
        &mut style.visuals.widgets.noninteractive,
        Color32::TRANSPARENT,
        c.text.gamma_multiply(0.4),
    );
    // Separators draw with noninteractive.bg_stroke.
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, c.sep);
    style.visuals.selection.bg_fill = c.sel;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, c.text);
    style.visuals.window_fill = c.bg;
    style.visuals.window_stroke = Stroke::new(1.0_f32, c.bg);
    style.visuals.popup_shadow = egui::Shadow::NONE;
    style.visuals.window_shadow = egui::Shadow::NONE;
    style.visuals.menu_corner_radius = CornerRadius::ZERO;
    style.visuals.override_text_color = None;
}

fn menu_button(ui: &mut egui::Ui, label: &str, enabled: bool, row_h: f32) -> egui::Response {
    ui.add_enabled(
        enabled,
        Button::new(label)
            .wrap_mode(egui::TextWrapMode::Extend)
            .min_size(egui::vec2(0.0, row_h)),
    )
}

/// Render the open menu (and submenu). Returns a chosen action, if any;
/// `state_opt` is set to None when the menu closes for any reason.
pub fn show(ctx: &egui::Context, state_opt: &mut Option<MenuState>) -> Option<MenuAction> {
    let state = state_opt.as_mut()?;
    let c = colors(state.dark_mode);
    let caps = state.caps;
    let row_h = state.row_h;
    let submenu_open = state.submenu_open;
    let mut action: Option<MenuAction> = None;
    let mut close = false;

    let mut hover_close_submenu = false;
    let mut hover_open_submenu: Option<usize> = None;
    // Where each submenu's parent row actually landed, in whatever coordinates
    // it was drawn in. Inline the menu may be inside a `ScrollArea`, so the
    // measured-at-open `row_top` is not where the row IS.
    let mut sub_row_rects: Vec<Rect> = Vec::new();

    // ── Main menu ──────────────────────────────────────────────────────────
    let menu_spec = crate::shell::SurfaceSpec {
        id: MENU_ID,
        size: state.size,
        min_size: state.size,
        pos: Some(state.pos),
        order: egui::Order::Foreground,
        ..Default::default()
    };
    let font_scale = state.font_scale;
    // Read INSIDE the menu's own surface. On the desktop the menu is a separate
    // viewport with its own input, so the parent context's pointer is not over
    // it and would report whatever the main window last saw.
    let mut pointer: Option<Pos2> = None;
    let mut moving = false;
    let menu_report = crate::shell::surface(ctx, caps, &menu_spec, &mut |ui, want_close| {
        pointer = ui.ctx().input(|i| i.pointer.latest_pos());
        // Points per second. A pointer crossing rows on its way somewhere is
        // moving; one that has arrived is not. `40` is slow enough that a
        // deliberate slow drag still counts as moving and fast enough that the
        // last twitch before stopping does not.
        moving = ui.ctx().input(|i| i.pointer.velocity().length()) > 40.0;
        apply_menu_style(ui.style_mut(), c);
        scale_menu_font(ui.style_mut(), font_scale);
        let rect = ui.max_rect();
        draw_menu_backdrop(ui.painter(), rect, c, caps);

        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
            let mut sub_idx = 0usize;
            for entry in &state.entries {
                match entry {
                    Entry::Separator => {
                        ui.add(egui::Separator::default().spacing(SEP_H));
                    }
                    Entry::Item {
                        label,
                        action: a,
                        enabled,
                    } => {
                        let r = menu_button(ui, label, *enabled, row_h);
                        if r.hovered() {
                            hover_close_submenu = true;
                        }
                        if r.clicked() {
                            action = Some(a.clone());
                            *want_close = true;
                        }
                    }
                    Entry::Submenu { label, .. } => {
                        let r = ui.add(
                            Button::new(label.as_str())
                                .right_text(ARROW)
                                .selected(submenu_open == Some(sub_idx))
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .min_size(egui::vec2(0.0, row_h)),
                        );
                        if r.hovered() || r.clicked() {
                            hover_open_submenu = Some(sub_idx);
                        }
                        sub_row_rects.push(r.rect);
                        sub_idx += 1;
                    }
                }
            }
        });
    });
    close |= menu_report.close;

    // Armed once the pointer has moved away from where the menu appeared under
    // it. Sticky, and measured from the FIRST position seen inside the menu
    // rather than from the click, so a menu clamped against a screen edge is
    // judged by where the pointer actually is.
    if let Some(p) = pointer {
        match state.first_pointer {
            None => state.first_pointer = Some(p),
            Some(first) => {
                if (p - first).length() > ARM_SLOP {
                    state.armed = true;
                }
            }
        }
    }
    if settle_submenu(
        &mut state.submenu_open,
        &mut state.pending_sub,
        hover_open_submenu,
        hover_close_submenu,
        state.armed,
        !moving,
        Instant::now(),
    ) {
        // Still waiting out the dwell. Ask for the frame that will end it, or a
        // pointer that has stopped moving would sit there forever.
        ctx.request_repaint_after(SUB_SWITCH_DWELL);
    }

    // ── Submenu (sibling, Qt-style to the right) ──────────────────────────
    // Only one submenu can be open at a time, so they all share one surface id
    // and it simply moves and resizes as the pointer travels down the menu.
    let open_sub = state
        .submenu_open
        .filter(|_| !close)
        .and_then(|i| state.subs.get(i).map(|g| (i, g.row_top, g.size)));
    let mut submenu_report = None;
    if let Some((sub_i, row_top, sub_size)) = open_sub {
        // Beside the row, and the two hosts measure that differently.
        //
        // On the desktop the menu is its own window: rows are drawn at
        // window-local coordinates and `row_top` — measured at open time — is
        // exact, so the submenu goes at the menu window's origin plus it.
        //
        // Inline there is no window, the row rect is already in canvas
        // coordinates, and the menu may be scrolled. Using `row_top` there put
        // the submenu beside a DIFFERENT row: reaching for it crossed other
        // rows, which re-points `submenu_open`, and the click then landed in
        // whichever submenu had taken its place — silently setting a capo
        // nobody asked for. The row's own rect is the only thing that knows
        // where the row ended up.
        // **Beside the row where it actually IS**, on both hosts.
        //
        // The desktop used to place it from `row_top`, measured when the menu
        // opened. That is only right while the measurement and the layout agree
        // about row heights, separators and font scale — and when they drift,
        // the submenu appears beside a different row, so reaching for it drags
        // the pointer across the rows in between. The inline host already had
        // this fix and the comment explaining it; the desktop had the bug.
        //
        // `x` still comes from the menu's own width rather than the row rect:
        // a justified button stops short of the frame's padding, and hanging
        // the submenu off THAT leaves it overlapping the menu by a few points.
        let mut sub_pos = match sub_row_rects.get(sub_i) {
            Some(r) if !caps.child_windows => Pos2::new(r.max.x, r.min.y),
            Some(r) => Pos2::new(state.pos.x + state.size.x, state.pos.y + r.min.y),
            None => Pos2::new(state.pos.x + state.size.x, state.pos.y + row_top),
        };
        if let Some(mon) = state.monitor {
            sub_pos = clamp_submenu(sub_pos, sub_size, state.pos.x, mon);
        }

        let sub_spec = crate::shell::SurfaceSpec {
            id: SUBMENU_ID,
            size: sub_size,
            min_size: sub_size,
            pos: Some(sub_pos),
            // Don't steal key focus from the menu — and inline, sit above it.
            takes_focus: false,
            order: egui::Order::Tooltip,
            ..Default::default()
        };
        let report = crate::shell::surface(ctx, caps, &sub_spec, &mut |ui, want_close| {
            apply_menu_style(ui.style_mut(), c);
            scale_menu_font(ui.style_mut(), font_scale);
            let rect = ui.max_rect();
            draw_menu_backdrop(ui.painter(), rect, c, caps);
            let items = state
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Submenu { items, .. } => Some(items),
                    _ => None,
                })
                .nth(sub_i);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                for it in items.into_iter().flatten() {
                    // `it.enabled`, not `true`: "Teach Chord Name..." lives in
                    // a submenu now, and hard-coding this is how a dialog opens
                    // on a voicing that is not being held.
                    if menu_button(ui, &it.label, it.enabled, row_h).clicked() {
                        action = Some(it.action.clone());
                        *want_close = true;
                    }
                }
            });
        });
        close |= report.close;
        submenu_report = Some(report);
    }

    // ── Closing when the user goes elsewhere ───────────────────────────────
    //
    // Two different signals for the same intent, because the two hosts offer
    // different evidence. A window knows it lost focus; a layer in someone
    // else's window has no focus to lose and has to watch the pointer instead.
    if caps.child_windows {
        if menu_report.focused == Some(true)
            || submenu_report.is_some_and(|r| r.focused == Some(true))
        {
            state.saw_focus = true;
        }
        let grace = state.opened_at.elapsed() > std::time::Duration::from_millis(250);
        let all_unfocused = menu_report.focused == Some(false)
            && submenu_report.is_none_or(|r| r.focused != Some(true));
        if state.saw_focus && grace && all_unfocused {
            close = true;
        }
    } else if menu_report.pressed_outside && submenu_report.is_none_or(|r| r.pressed_outside) {
        // The press that OPENED the menu is always inside it — the menu is
        // positioned at the cursor — so this needs no opening grace.
        close = true;
    }

    if close {
        *state_opt = None;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> MenuView {
        MenuView {
            dark_mode: false,
            borderless: false,
            keytoggle: false,
            prefer_flats: true,
            detection_enabled: true,
            detached: false,
            notes_held: false,
            learning_on: false,
            next_font: None,
            supporter: false,
            heart_on: true,
            fretboard_on: false,
            tuning: "Standard".to_string(),
            capo: 0,
            wood: "rosewood",
            fretboard_detached: false,
            theory: crate::theory_panel::Views::default(),
            theory_follows_midi: false,
            theory_detached: false,
            // Off, like the setting it mirrors: the band is 200 points tall and
            // a window that grows on its own is the geometry surprise this app
            // has already been bitten by twice.
            recorder_on: false,
            recorder_detached: false,
            count_in_beats: 4,
            time_signature: crate::recorder::TimeSignature::default(),
            count_in_bars: 1,
            count_in_in_take: false,
            record_sources: "auto".to_owned(),
            metronome_on: false,
            metronome_in_take: false,
            hide_elapsed: false,
            caps: Caps::DESKTOP,
        }
    }

    /// Everything on at once, for the tests that need the biggest menu there
    /// is. A helper rather than four copies of the same literal, because the
    /// copies drifted the moment the recorder arrived and the "fullest" menu
    /// silently stopped being the fullest one.
    fn fullest() -> MenuView {
        MenuView {
            fretboard_on: true,
            recorder_on: true,
            supporter: true,
            next_font: Some("Terminess"),
            ..view()
        }
    }

    #[test]
    #[ignore = "structure dump, not an assertion"]
    fn dump_menu_structure() {
        // `fullest()`, not `view()`: a dump that leaves out the fretboard and
        // recorder blocks is a dump of two thirds of the menu, which is not
        // what anyone runs this for.
        for e in build_entries(fullest()) {
            match e {
                Entry::Submenu { label, items } => {
                    println!("[{label}]");
                    for it in items {
                        // Greyed items are marked: the whole point of `SubItem`
                        // carrying `enabled` is that a hover can hold one.
                        println!(
                            "    {}{}",
                            it.label,
                            if it.enabled { "" } else { "  (greyed)" }
                        );
                    }
                }
                other => println!("{}", label_of(&other)),
            }
        }
    }

    fn label_of(e: &Entry) -> String {
        match e {
            Entry::Item { label, .. } => label.clone(),
            Entry::Separator => "-----".into(),
            Entry::Submenu { label, .. } => format!("[{label}]"),
        }
    }

    /// Submenu rows, in menu order: (parent label, item labels, item actions).
    fn submenus(v: MenuView) -> Vec<(String, Vec<String>, Vec<MenuAction>)> {
        build_entries(v)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Submenu { label, items } => Some((
                    label,
                    items.iter().map(|it| it.label.clone()).collect(),
                    items.into_iter().map(|it| it.action).collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// One submenu by name. Positional indexing broke every time a submenu
    /// was added above another, which is a test failing for the wrong reason.
    fn sub(v: MenuView, name: &str) -> (String, Vec<String>, Vec<MenuAction>) {
        submenus(v.clone())
            .into_iter()
            .find(|(n, ..)| n == name)
            .unwrap_or_else(|| panic!("no {name} submenu"))
    }

    /// The hover labels, in menu order — what the top level READS as.
    fn category_names(v: MenuView) -> Vec<String> {
        submenus(v).into_iter().map(|(n, ..)| n).collect()
    }

    /// TOP-LEVEL clickable rows only.
    fn rows(v: MenuView) -> Vec<(String, MenuAction, bool)> {
        build_entries(v)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item {
                    label,
                    action,
                    enabled,
                } => Some((label, action, enabled)),
                _ => None,
            })
            .collect()
    }

    /// Every clickable row in the whole menu: the top level and the inside of
    /// every hover, in the order the user meets them.
    ///
    /// Most of what these tests assert is "this row exists, with this label,
    /// greyed exactly when it cannot act". Grouping the menu into categories
    /// moved a dozen rows one level down without changing any of that, so the
    /// tests ask the whole menu rather than the top level of it. The plugin
    /// test in particular MUST use this: a row that needs a window is just as
    /// dead one level down.
    fn all_rows(v: MenuView) -> Vec<(String, MenuAction, bool)> {
        build_entries(v)
            .into_iter()
            .flat_map(|e| match e {
                Entry::Item {
                    label,
                    action,
                    enabled,
                } => vec![(label, action, enabled)],
                Entry::Submenu { items, .. } => items
                    .into_iter()
                    .map(|it| (it.label, it.action, it.enabled))
                    .collect(),
                Entry::Separator => Vec::new(),
            })
            .collect()
    }

    fn find(v: MenuView, action: MenuAction) -> Option<(String, bool)> {
        all_rows(v)
            .into_iter()
            .find(|(_, a, _)| *a == action)
            .map(|(l, _, e)| (l, e))
    }

    /// D-UI-9: correcting acts on the held voicing AND needs a visible reading,
    /// so it greys out with no notes down or with detection off.
    ///
    /// Unchanged in intent by the regrouping; it now proves it about a row
    /// inside the Chords hover, which is the reason `SubItem` has an `enabled`
    /// field at all.
    #[test]
    fn correct_item_needs_notes_and_detection() {
        let mut v = view();
        let label = "Correct Chord Name...".to_owned();
        assert_eq!(
            find(v.clone(), MenuAction::CorrectChordName),
            Some((label.clone(), false))
        );
        v.notes_held = true;
        assert_eq!(
            find(v.clone(), MenuAction::CorrectChordName),
            Some((label.clone(), true))
        );
        // Detection off nulls current_chord — nothing to correct against.
        v.detection_enabled = false;
        assert_eq!(
            find(v.clone(), MenuAction::CorrectChordName),
            Some((label, false))
        );
        // Teaching still works with detection off: it pins a name outright.
        assert_eq!(
            find(v.clone(), MenuAction::TeachChordName).map(|(_, e)| e),
            Some(true)
        );
    }

    /// Qt parity: toggles rename themselves rather than showing a checkmark.
    #[test]
    fn learning_toggle_renames_itself() {
        let mut v = view();
        assert_eq!(
            find(v.clone(), MenuAction::ToggleChordLearning),
            Some(("Enable Chord Learning".to_owned(), true))
        );
        v.learning_on = true;
        assert_eq!(
            find(v.clone(), MenuAction::ToggleChordLearning),
            Some(("Disable Chord Learning".to_owned(), true))
        );
    }

    /// The size choices are the ones that existed before submenus were plural,
    /// and they must come out of the generalised path unchanged.
    ///
    /// Same intent as when this test was called `size_is_still_the_first_
    /// submenu...`, asserted against the new shape: "Size" could not become a
    /// child of "Window" (a submenu holds items, not submenus), so the percents
    /// ARE Window's items. Still the first hover, still seven, still 100% third.
    #[test]
    fn the_size_percents_are_still_the_first_hover_and_still_read_the_same() {
        let subs = submenus(view());
        assert_eq!(subs[0].0, "Window");
        assert_eq!(
            subs[0].1[..7],
            ["50%", "75%", "100%", "125%", "150%", "175%", "200%"]
        );
        assert_eq!(subs[0].2[2], MenuAction::SetSizePercent(100));
        // Borderless joined them rather than sitting loose at the top level.
        assert_eq!(subs[0].1[7], "Borderless");
    }

    /// Dark Mode is the one row that is deliberately NOT in a category.
    ///
    /// `app.rs`'s plugin test finds it by label through `rows_for_test` and
    /// clicks it to prove menu rows are alive at all in an editor — and
    /// `rows_for_test` returns top-level items only. Filing it under a hover
    /// breaks that test in another file, which is the sort of failure nobody
    /// reads the second time. It is also the most-flipped row in the menu, so
    /// the hover would be a toll paid all day.
    #[test]
    fn dark_mode_stays_a_top_level_row_because_another_file_clicks_it_by_label() {
        for caps in [Caps::DESKTOP, Caps::PLUGIN] {
            let v = MenuView {
                caps,
                next_font: Some("Terminess"),
                ..view()
            };
            assert!(
                rows(v)
                    .iter()
                    .any(|(l, a, _)| l == "Dark Mode" && *a == MenuAction::ToggleDarkMode),
                "no top-level Dark Mode row under {caps:?}"
            );
        }
    }

    /// D-UI-15: the guitar view renames itself like every other toggle here,
    /// and its choice lists exist only while it is on. A Tuning row on a
    /// hidden fretboard is a control for something you cannot see.
    #[test]
    fn the_fretboard_toggle_brings_its_submenus_with_it() {
        let mut v = view();
        assert_eq!(
            find(v.clone(), MenuAction::ToggleFretboard),
            Some(("Show Fretboard".to_owned(), true))
        );
        // With the view off there is exactly one thing to say about it, so the
        // Fretboard category is that row rather than a hover onto one item.
        assert_eq!(
            category_names(v.clone()),
            vec!["Window", "Colors", "Keyboard", "Chords", "Theory"],
            "no fretboard hovers while the fretboard is off"
        );
        assert!(rows(v.clone()).iter().any(|(l, ..)| l == "Show Fretboard"));

        v.fretboard_on = true;
        assert_eq!(
            find(v.clone(), MenuAction::ToggleFretboard),
            Some(("Hide Fretboard".to_owned(), true))
        );
        // Asserted as the whole list, in order: an inserted submenu that
        // silently shifts Wood/Tuning/Capo is exactly what this catches.
        // Wood, Tuning and Capo are SIBLINGS of Fretboard rather than children
        // of it because a submenu cannot hold a submenu — see `build_entries`.
        assert_eq!(
            category_names(v.clone()),
            vec![
                "Window",
                "Colors",
                "Keyboard",
                "Chords",
                "Theory",
                "Fretboard",
                "Wood",
                "Tuning",
                "Capo"
            ]
        );
        let wood = sub(v.clone(), "Wood");
        assert_eq!(wood.1.len(), 3, "three woods");
        assert!(
            wood.1[0].starts_with("Rosewood"),
            "rosewood is the default and comes first"
        );
        assert!(wood.1[0].ends_with('\u{2022}'));
        assert_eq!(wood.2[0], MenuAction::SetWood("rosewood"));
        // Detach mirrors the chord window's toggle, renaming itself.
        assert_eq!(
            find(v.clone(), MenuAction::DetachFretboard).map(|(l, _)| l),
            Some("Detach Fretboard".to_owned())
        );
        let d = MenuView {
            fretboard_detached: true,
            ..v.clone()
        };
        assert_eq!(
            find(d, MenuAction::AttachFretboard).map(|(l, _)| l),
            Some("Attach Fretboard".to_owned())
        );
        // Every shipped tuning is offered, and the live one is marked rather
        // than hidden: a submenu that never says what is selected makes you
        // close it again to find out.
        let tuning = sub(v.clone(), "Tuning");
        assert_eq!(tuning.1.len(), fretboard::TUNINGS.len());
        assert!(tuning.1[0].starts_with("Standard"));
        assert!(
            tuning.1[0].ends_with('\u{2022}'),
            "the current tuning is marked"
        );
        assert!(!tuning.1[1].ends_with('\u{2022}'));
        assert_eq!(tuning.2[0], MenuAction::SetTuning("Standard".to_string()));
        assert!(
            tuning.2.iter().all(|a| matches!(a, MenuAction::SetTuning(n)
                if fretboard::Tuning::by_name(n).is_some())),
            "every offered tuning must resolve"
        );

        let capo = sub(v.clone(), "Capo");
        assert_eq!(capo.1[0], "No Capo  \u{2022}");
        assert_eq!(capo.1[1], "Fret 1");
        assert_eq!(capo.2[0], MenuAction::SetCapo(0));
        assert_eq!(capo.2.len() as u8, CAPO_MAX + 1);
    }

    #[test]
    fn the_marked_row_follows_the_settings() {
        let v = MenuView {
            fretboard_on: true,
            tuning: "DADGAD".to_string(),
            capo: 3,
            ..view()
        };
        let tuning = sub(v.clone(), "Tuning");
        let marked: Vec<&String> = tuning
            .1
            .iter()
            .filter(|l| l.ends_with('\u{2022}'))
            .collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].starts_with("DADGAD"));
        assert_eq!(sub(v.clone(), "Capo").1[3], "Fret 3  \u{2022}");
    }

    /// A submenu low in the menu must slide up rather than run off the bottom
    /// of the screen, and flip left rather than off the right edge.
    ///
    /// Same intent as before, but asserted about EVERY hover and against the
    /// real `clamp_submenu` rather than a copy of it — the copy could pass
    /// while `show` was wrong. It matters more now: grouping the menu into
    /// categories means most hovers open from rows below the halfway mark,
    /// where only Capo used to be.
    #[test]
    fn every_submenu_is_pulled_back_onto_the_screen_near_a_corner() {
        let mon = Vec2::new(1440.0, 900.0);
        let row_h = 21.0; // a typical measured row; the clamp is height-blind
        let width = 260.0;
        // The fullest menu there is: fretboard on, so Wood/Tuning/Capo exist,
        // and recorder on, so Pre-roll does — the newest hover and the lowest,
        // which makes it the one most likely to run off the bottom.
        let v = fullest();
        for (name, items, _) in submenus(v.clone()) {
            let size = Vec2::new(width, items.len() as f32 * row_h);
            // Opened from the bottom-right, the corner where a menu that was
            // right-clicked low on a small screen puts its rows.
            let p = clamp_submenu(Pos2::new(1380.0, 860.0), size, 1120.0, mon);
            assert!(
                p.y >= 0.0 && p.y + size.y <= mon.y,
                "[{name}] runs off the bottom at {p:?}"
            );
            assert!(
                p.x >= 0.0 && p.x + size.x <= mon.x,
                "[{name}] runs off the right at {p:?}"
            );
            assert!(
                p.x < 1380.0,
                "[{name}] should flip to the menu's left, not just shrink back"
            );
        }
        // Comfortably inside: untouched.
        let inside = Pos2::new(300.0, 200.0);
        assert_eq!(
            clamp_submenu(inside, Vec2::new(120.0, 260.0), 200.0, mon),
            inside
        );
    }

    /// A category must be worth the hover it costs.
    ///
    /// `Caps` can hollow one out — Window loses Borderless in a plugin,
    /// Keyboard loses its device row, Fretboard loses Detach — and an empty
    /// hover is a dead end the user cannot tell from a bug, while a one-item
    /// hover charges a hover for a single row.
    #[test]
    fn no_category_opens_onto_nothing_or_onto_a_single_row() {
        for caps in [Caps::DESKTOP, Caps::PLUGIN, Caps::MINIMAL] {
            for on in [false, true] {
                let v = MenuView {
                    caps,
                    fretboard_on: on,
                    detached: on,
                    fretboard_detached: on,
                    theory_detached: on,
                    // Both states matter under both `Caps`: a Recorder hover
                    // holding only "Detach Recorder" is exactly what a
                    // `capture_devices`-true / `detachable`-false host would
                    // produce if the show/hide row were ever gated too.
                    recorder_on: on,
                    recorder_detached: on,
                    supporter: on,
                    next_font: on.then_some("Terminess"),
                    ..view()
                };
                for (name, items, _) in submenus(v) {
                    assert!(
                        items.len() >= 2,
                        "[{name}] is a hover over {} row(s) under {caps:?}",
                        items.len()
                    );
                }
            }
        }
    }

    /// The point of the whole exercise: a top level of subjects, not a
    /// screenful of verbs. It was twenty-six near-flat rows, which is a list
    /// you read rather than a menu you aim at.
    #[test]
    fn the_top_level_is_a_short_list_of_subjects() {
        let count = |v: MenuView| {
            build_entries(v)
                .iter()
                .filter(|e| !matches!(e, Entry::Separator))
                .count()
        };
        // Was 16, then 18. The recorder costs FOUR, and each is structural
        // rather than sprawl: one subject (Recorder) plus three choice-list
        // siblings — Sources, Time signature and Count-in — which is the exact
        // shape the fretboard already has and the only shape `Entry::Submenu`
        // allows, since a submenu cannot hold another one. Raise this only for
        // a reason that can be written down in the same breath.
        assert!(
            count(fullest()) <= 20,
            "the fullest menu is back to {} top-level rows",
            count(fullest())
        );
        assert!(
            count(view()) <= 11,
            "the everyday menu is {} top-level rows",
            count(view())
        );
    }

    /// D-UI-17: the theory band is the third detachable surface and its row
    /// renames itself like the other two. It is absent where there is no window
    /// to put it in, which is the same rule the other two follow.
    #[test]
    fn the_theory_detach_row_renames_itself_and_is_absent_without_a_window() {
        assert_eq!(
            find(view(), MenuAction::DetachTheory).map(|(l, _)| l),
            Some("Detach Theory".to_owned())
        );
        let d = MenuView {
            theory_detached: true,
            ..view()
        };
        assert_eq!(
            find(d.clone(), MenuAction::AttachTheory).map(|(l, _)| l),
            Some("Attach Theory".to_owned())
        );
        assert_eq!(
            find(d, MenuAction::DetachTheory),
            None,
            "one row with two names, never both at once"
        );
        let p = MenuView {
            caps: Caps::PLUGIN,
            ..view()
        };
        assert_eq!(find(p.clone(), MenuAction::DetachTheory), None);
        assert_eq!(
            sub(p, "Theory").1.len(),
            crate::theory_panel::View::ALL.len() + 1,
            "the diagrams and Follow MIDI, and nothing that needs a window"
        );
    }

    /// The custom-tuning editor is a fretboard control, so it appears with the
    /// fretboard and not before. It is a dialog rather than a window, so unlike
    /// Detach it survives a plugin — the same reasoning that keeps "Teach Chord
    /// Name..." there.
    #[test]
    fn custom_tuning_is_offered_only_while_the_fretboard_is_showing() {
        assert_eq!(
            find(view(), MenuAction::EditCustomTuning),
            None,
            "an editor for a view the user cannot see"
        );
        let on = MenuView {
            fretboard_on: true,
            ..view()
        };
        assert_eq!(
            find(on.clone(), MenuAction::EditCustomTuning),
            Some(("Custom Tuning...".to_owned(), true))
        );
        let plugin = MenuView {
            caps: Caps::PLUGIN,
            ..on.clone()
        };
        assert!(find(plugin, MenuAction::EditCustomTuning).is_some());
        // It sits in the Fretboard hover, NOT in Tuning, where every item must
        // resolve to a shipped tuning.
        assert!(sub(on.clone(), "Fretboard")
            .2
            .contains(&MenuAction::EditCustomTuning));
        assert!(sub(on, "Tuning")
            .2
            .iter()
            .all(|a| matches!(a, MenuAction::SetTuning(_))));
    }

    /// The desktop menu must still offer everything it offered under
    /// `Caps::DESKTOP`. Regrouping is only safe if nothing WENT AWAY; where a
    /// row lives is what changed, so this asks the whole menu rather than the
    /// top level of it.
    #[test]
    fn desktop_caps_change_nothing() {
        let mut v = view();
        v.fretboard_on = true;
        v.detection_enabled = true;
        let with = all_rows(v.clone());
        for want in [
            MenuAction::ToggleBorderless,
            MenuAction::SelectMidiInput,
            MenuAction::DetachChordWindow,
            MenuAction::DetachFretboard,
            MenuAction::DetachTheory,
        ] {
            assert!(
                with.iter().any(|(_, a, _)| *a == want),
                "{want:?} went missing on the desktop"
            );
        }
        assert_eq!(submenus(v.clone())[0].0, "Window");
        assert_eq!(
            rows(v.clone()).last().map(|(_, a, _)| a.clone()),
            Some(MenuAction::ResetSettings)
        );
    }

    /// In a plugin, every row that survives must be one the host can actually
    /// honour. An inert row is worse than an absent one: the user cannot tell
    /// whether they mis-clicked or the app is broken.
    #[test]
    fn no_surviving_plugin_row_needs_a_window_or_a_device() {
        let v = MenuView {
            caps: Caps::PLUGIN,
            fretboard_on: true,
            detection_enabled: true,
            detached: true,
            fretboard_detached: true,
            theory_detached: true,
            // Turned ON deliberately. The settings file is shared with the
            // standalone, so a plugin WILL be handed `show_recorder: true` by
            // somebody's config; the category has to be absent because of
            // `caps`, not because the flag happened to be false.
            recorder_on: true,
            recorder_detached: true,
            // And an instrument LOADED, for the same reason: the settings file
            // remembers a plugin path, so a plugin instance will be handed one.
            // The Instrument Window and Unload rows have to be absent because
            // of `caps`, not because nothing happened to be loaded.
            metronome_on: true,
            ..view()
        };
        // Detach/Attach Theory joins the list for exactly the reason the other
        // two detach pairs are on it: it needs a second OS window, and a plugin
        // editor is handed one window by the host and gets no more.
        //
        // `EditCustomTuning` is deliberately NOT here. It raises a dialog, and
        // dialogs are drawn in the canvas when `caps.child_windows` is false —
        // the same road "Teach Chord Name..." takes, which this test asserts
        // survives a few lines down.
        //
        // EVERY recorder action is here, including the two that raise no window
        // and open no device by themselves (`ShowExportDialog`, `SetPreRoll`,
        // `ToggleHideElapsed`). They are forbidden by what they are ABOUT: each
        // one configures a capture a plugin may not perform, so a row offering
        // it is a promise the editor cannot keep. Listing only the obvious
        // three is precisely how a plugin row for a camera slips through.
        let mut forbidden = vec![
            MenuAction::SelectMidiInput,
            MenuAction::ToggleBorderless,
            MenuAction::DetachChordWindow,
            MenuAction::AttachChordWindow,
            MenuAction::DetachFretboard,
            MenuAction::AttachFretboard,
            MenuAction::DetachTheory,
            MenuAction::AttachTheory,
            MenuAction::ToggleRecorder,
            MenuAction::DetachRecorder,
            MenuAction::AttachRecorder,
            MenuAction::ShowExportDialog,
            MenuAction::ToggleHideElapsed,
            // The instrument rows. Loading one runs third-party code inside
            // the process and opens a device; the editor opens a native window;
            // and the click writes to an output stream. None of the three is
            // anything a VST3 editor may do inside its host.
            MenuAction::ToggleMetronome,
            MenuAction::ToggleMetronomeInTake,
        ];
        // Built from the table rather than spelled out, so a fourth count-in
        // choice cannot be added to `COUNT_IN_CHOICES` and quietly arrive in a
        // plugin unlisted here.
        forbidden.extend(crate::recorder::COUNT_IN_CHOICES.map(MenuAction::SetCountIn));
        forbidden.extend(["auto", "plugin", "input", "both"].map(MenuAction::SetRecordSources));
        for (label, action, _) in all_rows(v.clone()) {
            assert!(
                !forbidden.contains(&action),
                "{label} needs something a plugin editor does not have"
            );
        }
        // Size IS offered, and this assertion used to say the opposite —
        // which is how a plugin shipped with no way to change its size at all.
        // A plugin cannot SET its geometry, but it can ask the host for one,
        // and refusing to offer the choice is not the same as being unable to
        // make it. Borderless is the row that genuinely cannot survive: window
        // chrome belongs to whoever owns the window.
        assert!(
            all_rows(v.clone())
                .iter()
                .any(|(_, a, _)| *a == MenuAction::SetSizePercent(100)),
            "a plugin with no size choices has no way to be made readable"
        );
        assert!(
            !all_rows(v.clone())
                .iter()
                .any(|(_, a, _)| *a == MenuAction::ToggleBorderless),
            "window chrome is the host's in a plugin"
        );
        // And what SHOULD survive still does: the whole point is a plugin that
        // can still teach a chord, change tuning and pick a colour.
        let kept = all_rows(v.clone());
        for want in [
            MenuAction::ToggleDarkMode,
            MenuAction::ToggleKeytoggle,
            MenuAction::TeachChordName,
            MenuAction::ManageTaughtChords,
            MenuAction::ToggleFretboard,
            MenuAction::EditCustomTuning,
            MenuAction::ShowAbout,
        ] {
            assert!(
                kept.iter().any(|(_, a, _)| *a == want),
                "{want:?} went missing"
            );
        }
        // Every category is still there, and still in the same order: none of
        // them is emptied out by `Caps::PLUGIN`.
        assert_eq!(
            category_names(v.clone()),
            vec![
                "Window",
                "Colors",
                "Keyboard",
                "Chords",
                "Theory",
                "Fretboard",
                "Wood",
                "Tuning",
                "Capo"
            ]
        );
    }

    /// Each theory row renames itself the way every other toggle in this menu
    /// does, and all three are independent — the request was explicitly to be
    /// able to show more than one at once, so turning one on must not turn
    /// another off.
    #[test]
    fn the_theory_rows_rename_themselves_and_stay_independent() {
        use crate::theory_panel::{View, Views};
        let mut v = view();
        assert_eq!(
            sub(v.clone(), "Theory").1,
            vec![
                "Show Circle of Fifths",
                "Show Tonnetz",
                "Show Harmonic Triangles",
                "Follow MIDI",
                // Where the band LIVES comes last: the toggles above it are
                // what the band is.
                "Detach Theory",
            ]
        );
        let mut want: Vec<MenuAction> = View::ALL
            .iter()
            .map(|x| MenuAction::ToggleTheoryView(*x))
            .collect();
        want.push(MenuAction::ToggleTheoryFollowsMidi);
        want.push(MenuAction::DetachTheory);
        assert_eq!(sub(v.clone(), "Theory").2, want);

        // The follow row renames itself like every other toggle here, and it
        // is OFF by default: the band is something to look at while playing,
        // and one that redrew on every note could not be read while playing.
        assert!(!view().theory_follows_midi);
        let following = MenuView {
            theory_follows_midi: true,
            ..view()
        };
        assert_eq!(
            sub(following, "Theory").1[View::ALL.len()],
            "Stop Following MIDI"
        );

        v.theory = Views {
            circle: true,
            tonnetz: false,
            triangles: true,
        };
        assert_eq!(
            sub(v.clone(), "Theory").1[..3],
            [
                "Hide Circle of Fifths",
                "Show Tonnetz",
                "Hide Harmonic Triangles"
            ],
            "the rows do not each follow their own flag"
        );
    }

    /// The learning block sits with the teach block, after it, and the menu
    /// still ends with About / Reset.
    ///
    /// Same intent, new shape: those four rows are inside the Chords hover
    /// now, so the order is asserted there instead of across the top level.
    #[test]
    fn learning_block_sits_after_the_teach_block() {
        let chords = sub(view(), "Chords").2;
        let pos = |a: MenuAction| {
            chords
                .iter()
                .position(|x| *x == a)
                .unwrap_or_else(|| panic!("{a:?} is not in the Chords hover"))
        };
        assert!(
            pos(MenuAction::ToggleChordDetection) < pos(MenuAction::TeachChordName),
            "the detector's own switch comes before the things that train it"
        );
        assert!(pos(MenuAction::TeachChordName) < pos(MenuAction::ManageTaughtChords));
        assert!(pos(MenuAction::ManageTaughtChords) < pos(MenuAction::CorrectChordName));
        assert!(pos(MenuAction::CorrectChordName) < pos(MenuAction::ToggleChordLearning));

        let top = rows(view());
        assert_eq!(
            top.last().map(|(_, a, _)| a.clone()),
            Some(MenuAction::ResetSettings)
        );
        assert_eq!(
            top.iter().position(|(_, a, _)| *a == MenuAction::ShowAbout),
            Some(top.len() - 2),
            "About and Reset are the last two rows"
        );
    }

    /// §5: this row is the entire affordance. `show_recorder` is off by default
    /// and out of `first_launch()`, this menu is the whole chrome, and an
    /// earlier draft of the recorder shipped with no way to switch it on at
    /// all. So: it exists, it renames itself rather than growing a checkmark,
    /// and the band's other controls arrive with it.
    /// **Crossing rows on the way to a submenu must not change which one is
    /// open.**
    ///
    /// The owner's report: hovering any submenu gave the TOP category about
    /// eight times in ten. Submenus open to the RIGHT, so reaching one means
    /// travelling across the rows between here and there, and every one of them
    /// hovered — what opened was whichever row the path happened to end on.
    #[test]
    fn travelling_across_rows_does_not_switch_the_open_submenu() {
        let t0 = Instant::now();
        let mut open = None;
        let mut pending = None;

        // The first one opens at once. A delay here would just feel like lag.
        assert!(!settle_submenu(&mut open, &mut pending, Some(3), false, true, false, t0));
        assert_eq!(open, Some(3));

        // Now the pointer crosses rows 2, 1 and 0 on its way to the panel. None
        // of them may take over, because none was dwelt on.
        let mut t = t0;
        for row in [2usize, 1, 0] {
            t += std::time::Duration::from_millis(20);
            assert!(
                settle_submenu(&mut open, &mut pending, Some(row), false, true, false, t),
                "row {row} should have started a wait, not a switch"
            );
            assert_eq!(open, Some(3), "row {row} stole the submenu in transit");
        }

        // Arriving and STAYING is a different thing, and it does switch.
        let arrive = t + std::time::Duration::from_millis(20);
        assert!(settle_submenu(&mut open, &mut pending, Some(0), false, true, false, arrive));
        let settled = arrive + SUB_SWITCH_DWELL;
        assert!(!settle_submenu(&mut open, &mut pending, Some(0), false, true, false, settled));
        assert_eq!(open, Some(0), "a deliberate hover must still work");
    }

    /// **A menu that appears under the cursor must not act on what it landed
    /// on.**
    ///
    /// The menu's top-left is placed AT the click, so on the frame it opens the
    /// pointer is already inside its first row — and the first row is a
    /// submenu. Every single opening therefore opened Window, which is exactly
    /// what the owner reported twice: "most areas flash then default to the top
    /// submenu". Nobody pointed at it.
    #[test]
    fn a_menu_opening_under_the_cursor_opens_nothing() {
        let t = Instant::now();
        let mut open = None;
        let mut pending = None;

        // Not armed: the pointer has not moved since the menu appeared.
        assert!(!settle_submenu(
            &mut open, &mut pending, Some(0), false, false, true, t
        ));
        assert_eq!(open, None, "the top submenu opened without being pointed at");

        // Still not, however long it sits there.
        let later = t + std::time::Duration::from_secs(2);
        assert!(!settle_submenu(
            &mut open, &mut pending, Some(0), false, false, true, later
        ));
        assert_eq!(open, None);

        // Move, and the same hover works immediately.
        assert!(!settle_submenu(
            &mut open, &mut pending, Some(0), false, true, true, later
        ));
        assert_eq!(open, Some(0));
    }

    /// **A pointer that has STOPPED on a row has arrived.**
    ///
    /// Stillness rather than elapsed time is what makes the switch work at
    /// all: a stationary pointer produces no events, so a dwell waiting only on
    /// a timer can sit unfinished on the row you meant to leave — which is how
    /// the first fix left it still showing the wrong submenu.
    #[test]
    fn a_pointer_that_has_stopped_switches_at_once() {
        let t = Instant::now();
        let mut open = Some(4);
        let mut pending = None;
        // Moving across row 1: no switch.
        assert!(settle_submenu(
            &mut open, &mut pending, Some(1), false, true, false, t
        ));
        assert_eq!(open, Some(4));
        // Stopped on it: switch, on this very frame.
        assert!(!settle_submenu(
            &mut open, &mut pending, Some(1), false, true, true, t
        ));
        assert_eq!(open, Some(1));
    }

    /// Hovering the row that is ALREADY open changes nothing and cancels any
    /// half-finished switch — which is what moving back to it means.
    #[test]
    fn returning_to_the_open_row_cancels_a_pending_switch() {
        let t = Instant::now();
        let mut open = Some(2);
        let mut pending = Some((5, t));
        assert!(!settle_submenu(&mut open, &mut pending, Some(2), false, true, false, t));
        assert_eq!(open, Some(2));
        assert!(pending.is_none(), "the abandoned switch was still armed");
    }

    /// A plain row closes the submenu; being over NOTHING does not.
    ///
    /// The gap between the menu and its panel is "nothing", and closing there
    /// would make the panel impossible to reach on any host that does not
    /// place them flush.
    #[test]
    fn a_plain_row_closes_the_submenu_but_empty_space_does_not() {
        let t = Instant::now();
        let mut open = Some(1);
        let mut pending = None;
        assert!(!settle_submenu(&mut open, &mut pending, None, false, true, false, t));
        assert_eq!(open, Some(1), "the gap between menu and panel closed it");

        assert!(!settle_submenu(&mut open, &mut pending, None, true, true, false, t));
        assert_eq!(open, None, "a plain row should have closed it");
    }

    #[test]
    fn the_recorder_row_renames_itself_and_brings_its_controls_with_it() {
        let mut v = view();
        assert_eq!(
            find(v.clone(), MenuAction::ToggleRecorder),
            Some(("Show Recorder".to_owned(), true))
        );
        // Off, the category IS that one row — the same collapse the fretboard
        // gets, and for the same reason: there is one thing to say.
        assert!(!category_names(v.clone()).iter().any(|n| n == "Recorder"));
        assert!(rows(v.clone()).iter().any(|(l, ..)| l == "Show Recorder"));
        for absent in [
            MenuAction::ShowExportDialog,
            MenuAction::ToggleHideElapsed,
            MenuAction::DetachRecorder,
        ] {
            assert_eq!(
                find(v.clone(), absent.clone()),
                None,
                "{absent:?} is a control for a band the user cannot see"
            );
        }
        assert!(!category_names(v.clone()).iter().any(|n| n == "Count-in"));

        v.recorder_on = true;
        assert_eq!(
            find(v.clone(), MenuAction::ToggleRecorder),
            Some(("Hide Recorder".to_owned(), true)),
            "the label is the state readout; there are no checkmarks here"
        );
        // Detach mirrors the other three detachable surfaces, renaming itself.
        assert_eq!(
            find(v.clone(), MenuAction::DetachRecorder).map(|(l, _)| l),
            Some("Detach Recorder".to_owned())
        );
        let d = MenuView {
            recorder_detached: true,
            ..v.clone()
        };
        assert_eq!(
            find(d.clone(), MenuAction::AttachRecorder).map(|(l, _)| l),
            Some("Attach Recorder".to_owned())
        );
        assert_eq!(
            find(d, MenuAction::DetachRecorder),
            None,
            "one row with two names, never both at once"
        );
        assert_eq!(
            sub(v.clone(), "Recorder").1,
            vec![
                "Hide Recorder",
                "Detach Recorder",
                "Start the Click",
                "Record the Click Into Takes",
                "Record the Count-in Into the Take",
                "Audio Status...",
                "Export...",
                "Hide Elapsed Time",
            ]
        );
        // The elapsed-time switch renames itself too.
        assert_eq!(
            find(
                MenuView {
                    hide_elapsed: true,
                    ..v.clone()
                },
                MenuAction::ToggleHideElapsed
            )
            .map(|(l, _)| l),
            Some("Show Elapsed Time".to_owned())
        );
        // Recorder is LAST of the subjects, after the whole display block, and
        // Pre-roll is its sibling for the reason Wood/Tuning/Capo are the
        // fretboard's. Asserted as the whole list, in order: an inserted
        // category that shifts these is exactly what this catches.
        assert_eq!(
            category_names(fullest()),
            vec![
                "Window",
                "Colors",
                "Keyboard",
                "Chords",
                "Theory",
                "Fretboard",
                "Wood",
                "Tuning",
                "Capo",
                "Recorder",
                "Sources",
                "Time signature",
                "Count-in",
            ]
        );
    }

    /// The subject is absent, not greyed, where no device may be opened. A
    /// plugin must not reach for a camera behind the DAW's back, and a Minimal
    /// build has not linked the code that would.
    ///
    /// `Caps::PLUGIN` and `Caps::MINIMAL` together, because they arrive at the
    /// same answer from opposite directions and only one of them is obvious.
    #[test]
    fn the_recorder_category_is_absent_entirely_without_capture_devices() {
        for caps in [Caps::PLUGIN, Caps::MINIMAL] {
            let v = MenuView {
                caps,
                recorder_on: true,
                recorder_detached: true,
                ..view()
            };
            assert!(
                !category_names(v.clone())
                    .iter()
                    .any(|n| n == "Recorder" || n == "Count-in"),
                "a Recorder hover survived {caps:?}"
            );
            for gone in [
                MenuAction::ToggleRecorder,
                MenuAction::DetachRecorder,
                MenuAction::AttachRecorder,
                MenuAction::ShowExportDialog,
                MenuAction::ToggleHideElapsed,
                MenuAction::SetCountIn(3),
            ] {
                assert_eq!(
                    find(v.clone(), gone.clone()),
                    None,
                    "{gone:?} survived {caps:?}"
                );
            }
        }
    }

    /// Capturing and opening a second window are separate permissions, and a
    /// host can hold one without the other. The recorder must still be fully
    /// usable there — only the row that needs a window goes.
    ///
    /// A configuration nobody ships today, which is why it is worth a test: the
    /// gate is `capture_devices && detachable`, and writing it as one `if` is
    /// the mistake that would take the whole band away from such a host.
    #[test]
    fn only_the_detach_row_goes_when_a_capturing_host_has_no_second_window() {
        let v = MenuView {
            caps: Caps {
                detachable: false,
                child_windows: false,
                ..Caps::DESKTOP
            },
            recorder_on: true,
            ..view()
        };
        assert_eq!(find(v.clone(), MenuAction::DetachRecorder), None);
        assert_eq!(find(v.clone(), MenuAction::AttachRecorder), None);
        assert_eq!(
            sub(v.clone(), "Recorder").1,
            vec![
                "Hide Recorder",
                "Start the Click",
                "Record the Click Into Takes",
                "Record the Count-in Into the Take",
                "Audio Status...",
                "Export...",
                "Hide Elapsed Time"
            ],
            "everything that does not need a window stays"
        );
        assert!(category_names(v).iter().any(|n| n == "Count-in"));
    }

    /// The count-in hover says what it is set to rather than making you close
    /// it again to find out — and says it exactly once. Two marks is a menu
    /// that has lost track of its own state; none is a menu that never had it.
    #[test]
    fn the_count_in_hover_marks_exactly_one_choice() {
        for &want in &crate::recorder::COUNT_IN_CHOICES {
            let v = MenuView {
                recorder_on: true,
                count_in_bars: want,
                ..view()
            };
            let (labels, actions) = {
                let s = sub(v.clone(), "Count-in");
                (s.1, s.2)
            };
            assert_eq!(labels.len(), crate::recorder::COUNT_IN_CHOICES.len());
            let marked: Vec<&String> =
                labels.iter().filter(|l| l.ends_with('\u{2022}')).collect();
            assert_eq!(marked.len(), 1, "{want} bars marked {marked:?}");
            // Bars, with the beat count in brackets — the signature decides
            // it, so "2 bars" alone leaves the reader multiplying and
            // "12 beats" alone leaves them dividing.
            let expect = match want {
                0 => "No count-in".to_owned(),
                1 => "1 bar".to_owned(),
                n => format!("{n} bars"),
            };
            assert!(marked[0].starts_with(&expect), "{marked:?} is not {expect}");
            assert_eq!(
                actions,
                crate::recorder::COUNT_IN_CHOICES
                    .map(MenuAction::SetCountIn)
                    .to_vec()
            );
        }
        // The wording the plan asks for, in order.
        let v = MenuView {
            recorder_on: true,
            ..view()
        };
        assert_eq!(
            sub(v.clone(), "Count-in").1,
            vec![
                "No count-in",
                "1 bar  (4 beats of 4/4)  \u{2022}",
                "2 bars  (8 beats of 4/4)",
                "4 bars  (16 beats of 4/4)",
            ]
        );

        // **And the same list in 6/8 counts twelve, not eight.** This is the
        // whole reason the count-in is bars: no number of beats is the right
        // answer at every signature, and the old label said "of 4" because
        // there was nothing else it could say.
        let six_eight = MenuView {
            recorder_on: true,
            time_signature: crate::recorder::TimeSignature { beats: 6, unit: 8 },
            count_in_bars: 2,
            ..view()
        };
        assert_eq!(
            sub(six_eight, "Count-in").1,
            vec![
                "No count-in",
                "1 bar  (6 beats of 6/8)",
                "2 bars  (12 beats of 6/8)  \u{2022}",
                "4 bars  (24 beats of 6/8)",
            ]
        );
    }
}
