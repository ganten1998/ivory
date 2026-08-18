//! The Recorder band: a whole take's worth of controls in 200 points of window.
//!
//! Two pictures of the same machine, switched by [`RecordState`], because no
//! other recorder knows that its user is a pianist:
//!
//!   * **idle**, which is about FRAMING and destination — a wide camera
//!     preview, a live meter, the monitor levels, and every control that
//!     decides what the next take will be, and
//!   * **rolling**, which is about being readable from a piano bench two metres
//!     away — the preview collapses, the timecode and the meter become huge,
//!     and everything that could change the destination mid-take goes away.
//!
//! While playing, the pianist is looking at their hands. The preview's job is
//! finished before the take starts, which is the whole reason those are two
//! layouts and not one that dims.
//!
//! # Four groups, left to right
//!
//! The band was once a single row of controls at one size, and the owner's
//! verdict on it was that nothing said what it was. So it is now grouped, and
//! the groups are ordered by how often a hand goes near them:
//!
//!   1. **preview** — framing. Looked at before the first take of a session and
//!      then never again, so it is the one thing that shrinks when the take
//!      starts.
//!   2. **transport** — record, stop, meter, timecode. The biggest things in
//!      the band, because they are the ones read from the bench.
//!   3. **destination** — folder, name, devices, count-in, tempo, export. Set
//!      once and left alone, so it is the smallest type in the band and it
//!      disappears entirely while rolling.
//!   4. **instruments and monitor** — three instrument slots, each with its own
//!      small level knob and its own button that opens that plugin's window,
//!      then the click and input faders and the click switch. Touched every
//!      take, and the only group besides the transport that survives a live
//!      take: turning the click down halfway through is exactly when you need
//!      to, and so is reaching a preset in a plugin's own window.
//!
//! The monitor is last, at the right edge, and it is **pinned there** — same
//! rectangle in both layouts, down to the point. Everything else in the band
//! moves when a take starts: the preview collapses, the timecode triples, the
//! whole destination column vanishes. A fader that survives a take and then
//! slides a third of the window sideways at the moment it becomes the only
//! thing you might still want to touch is only half a survivor, so the monitor
//! is measured from the right edge of the band rather than from anything that
//! changes. See `MONITOR_W`.
//!
//! # Where the three slots' space came from
//!
//! Three instrument rows, each carrying a name, a level and two buttons, is a
//! lot to add to a band 200 points tall. Nothing was compressed to make room;
//! three things that deserved their space less gave it up:
//!
//!   * **the monitor's INSTRUMENT fader** — deleted outright. It set the level
//!     of "the instrument" back when there was one, and the per-slot knobs say
//!     the same thing three times more precisely. That is one of the four
//!     monitor rows back.
//!   * **the destination's INSTRUMENT row** — deleted too. It was a picker, and
//!     each slot's own name box is now that picker. The destination column went
//!     from eight rows to seven, so the seven that remain are each TALLER than
//!     the eight were.
//!   * **width, from the destination and the preview**. [`MONITOR_W`] went from
//!     0.28 of the body to 0.36; the preview's width cap went from 0.26 to 0.22,
//!     which only bites in the detached window (in the band the preview is
//!     bounded by the band's height long before it reaches either cap). The
//!     destination is the group `docs/RECORDER-PLAN.md` §5 describes as set once
//!     and left alone, and the preview is looked at twice a session; the slots
//!     are touched every take.
//!
//! There is no in-band control for hiding the elapsed clock. There was, it was
//! a box marked `CLOCK` with a line through it, and no one could tell what it
//! did. The setting lives in the right-click menu, where the row reads "Hide
//! Elapsed Time" and says so in words. [`RecorderView::hide_elapsed`] still
//! suppresses the readout; only the mystery box is gone.
//!
//! Like `piano.rs`, `fretboard_panel.rs` and `theory_panel.rs`, this module is
//! dumb: it is handed a [`RecorderView`] snapshot and it paints it, and a click
//! comes back out as a [`Hit`] for somebody else to act on. It opens no device,
//! reads no disk and knows no time — see `recorder.rs` for why that firewall is
//! load-bearing rather than tidy.
//!
//! **`draw` is a pure painter and `hit_test` is a pure function of geometry.**
//! Both go through one private [`Layout`], which is the single most important
//! thing in this file: a hit test that computes its own rectangles is a hit test
//! that stops matching the picture the first time the picture moves, invisibly,
//! and the compositor in `docs/RECORDER-PLAN.md` §6 needs to render this same
//! band into an offscreen 1920x1080 surface with no `Context` anywhere near it.
//!
//! See `docs/RECORDER-PLAN.md` §5.

use crate::fonts;
use crate::recorder::{
    disk_text, gain_text, gain_to_fader, timecode, DeviceLabel, ExportSpec, Level, Meters,
    NumField, Preview, RecordState, RecorderView, SlotView, COUNT_IN_CHOICES, MAX_BPM, MIN_BPM,
    SLOTS,
};
use crate::settings::Settings;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

/// Band height for a 1300pt-wide window. At that width it gives a ~230x150
/// preview and still leaves ~1000pt for controls, which is the one dimension
/// this window has in abundance. Scaled with everything else (spec §3.2).
/// **165, down from 200.** The take's settings left the band for the menu and
/// took seven rows with them; what is left is a transport, two faders, two
/// meters and six slot rows, and none of those wanted the height the old
/// destination column did. The band was reaching a third of the way down the
/// window for controls that no longer live in it.
pub const BAND_H_AT_1300: f64 = 165.0;

/// Height of the Recorder band for a window `w` points wide.
///
/// **A pure function of width and of nothing else**, truncated like every other
/// band in the layout. Nothing about the camera may reach it: a 4:3 webcam that
/// made the window 90 points taller than a 16:9 one would resize the piano
/// because somebody unplugged a device, which is `docs/RECORDER-PLAN.md` §0's
/// named failure. The camera's aspect is dealt with entirely inside
/// [`fit_preview`], where it belongs.
pub fn band_height(w: f32) -> f32 {
    (BAND_H_AT_1300 * w as f64 / 1300.0).trunc() as f32
}

// ── palette ────────────────────────────────────────────────────────────────

/// The record red, and the only strong hue this band uses.
///
/// The chord strip's red, so the app has one red rather than two that nearly
/// match. It reads against both the cream and the near-black background, which
/// a darker red does not.
const REC_RED: Color32 = Color32::from_rgb(0xE8, 0x3A, 0x4E);

struct Palette {
    /// Also the letterbox bars. See [`fit_preview`].
    bg: Color32,
    /// Recessed fills: the preview well, the meter troughs and the fader
    /// tracks. A fader has to look like a slot with something in it, which is
    /// the same picture as a meter and gets the same colour.
    well: Color32,
    /// Control fills, so a thing you can click looks unlike a thing you read.
    field: Color32,
    ink: Color32,
    faint: Color32,
    line: Color32,
    /// The meter and the fader fills. Whatever a held key looks like on the
    /// piano — the user chose that colour once and this is the same "sound is
    /// happening" signal.
    accent: Color32,
    rec: Color32,
    /// A chosen device that is not plugged in, which is neither an error nor
    /// normal and must not read as either.
    warn: Color32,
}

/// The two inks the band can wear.
const INK_LIGHT: Color32 = Color32::from_rgb(0xE8, 0xDC, 0xC0);
const INK_DARK: Color32 = Color32::from_rgb(0x1a, 0x1a, 0x1a);

/// Relative luminance, sRGB, as WCAG defines it.
fn luminance(c: Color32) -> f32 {
    let ch = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
}

/// Contrast ratio between two colours, 1.0 (identical) to 21.0 (black/white).
fn contrast_ratio(a: Color32, b: Color32) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    let (hi, lo) = if x > y { (x, y) } else { (y, x) };
    (hi + 0.05) / (lo + 0.05)
}

/// Whether ink on this background has to be light.
///
/// **The band's ink follows its own background, not the theme.** The colour is
/// the user's to choose, so a rule that read `dark_mode` would put near-black
/// text on a dark walnut the moment somebody picked one in light mode — and the
/// band would be unreadable with nothing on screen explaining why.
///
/// It MEASURES both candidates rather than testing a brightness threshold, and
/// the difference is not academic: a mid grey sits below any sensible threshold
/// and yet takes dark ink far better than light — 5.3 against 2.7. A threshold
/// picked the ivory and produced the one genuinely unreadable band this whole
/// mechanism exists to prevent.
///
/// The worst case is a background whose luminance falls where the two curves
/// cross, at about 0.17, where the better of the two still gives 3.57 — above
/// WCAG's 3.0 for large text and below its 4.5 for body text, which is honest:
/// somebody who picks that exact colour gets a legible band and not a
/// comfortable one.
fn wants_light_ink(c: Color32) -> bool {
    contrast_ratio(c, INK_LIGHT) > contrast_ratio(c, INK_DARK)
}

fn palette(s: &Settings) -> Palette {
    // The BAND's own background, and its own choice. It used to borrow the
    // piano's, so the recorder read as another band of the same window — which
    // is right until you want to tell a set of controls apart from an
    // instrument at a glance while playing.
    let bg = s.recorder_bg_color.to_color32();
    // Everything else follows the BACKGROUND, not `dark_mode`. See
    // `wants_light_ink`.
    let dark = wants_light_ink(bg);
    Palette {
        bg,
        // Derived FROM the background rather than fixed, so a recessed well
        // still looks recessed and a control still looks raised whatever the
        // band is coloured. Multiplying keeps the hue: a well in a walnut band
        // is a darker walnut, not a grey hole in it.
        well: shade(bg, if dark { 0.55 } else { 0.88 }),
        field: shade(bg, if dark { 1.35 } else { 0.94 }),
        ink: if dark { INK_LIGHT } else { INK_DARK },
        // Secondary text carries the folder preview and the disk line, which
        // are the two things a first-time user actually learns the scheme
        // from, so it is darkened on light and lightened on dark until it
        // reads at a glance — the same correction `theory_panel` needed.
        faint: if dark {
            Color32::from_rgb(0x9a, 0x92, 0x80)
        } else {
            Color32::from_rgb(0x6b, 0x60, 0x4a)
        },
        line: if dark {
            Color32::from_rgb(0x62, 0x5c, 0x50)
        } else {
            Color32::from_rgb(0x9c, 0x8f, 0x74)
        },
        accent: s.white_key_active_color.to_color32(),
        rec: REC_RED,
        warn: if dark {
            Color32::from_rgb(0xE8, 0xC4, 0x6A)
        } else {
            Color32::from_rgb(0xA0, 0x66, 0x00)
        },
    }
}

/// The background, lightened or darkened, keeping its hue.
///
/// `gamma_multiply` would fade it towards transparent; this scales the channels
/// so a walnut band gets a darker walnut well rather than a grey one.
fn shade(c: Color32, k: f32) -> Color32 {
    let f = |v: u8| (f32::from(v) * k).clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

fn font(size: f32) -> FontId {
    FontId::new(size, fonts::courier_bold())
}

fn font_light(size: f32) -> FontId {
    FontId::new(size, fonts::courier())
}

/// Advance width of one character as a fraction of the font size, for the
/// bundled monospaced faces. Used to size text to its box without laying it
/// out first, which a pure painter cannot do.
const ADV: f32 = 0.62;

/// A size at which `text` fits across `r`, never above `nominal`.
///
/// Shrink rather than truncate. A destination path is most useful at its END —
/// `.../Movies/Tangent` — and clipping takes exactly that away, while a path
/// two points smaller than its neighbours is merely a long path.
fn fit_text(r: Rect, text: &str, nominal: f32) -> f32 {
    let n = text.chars().count().max(1) as f32;
    nominal.min(r.width() / (n * ADV)).max(0.0)
}

/// Below this a glyph is a smudge, and a smudge reads as a rendering fault
/// rather than as "your window is too small". The band draws nothing instead.
const MIN_TEXT: f32 = 5.0;

// ── the shared layout ──────────────────────────────────────────────────────

/// Every rectangle the band uses, computed once from the rect and the state.
///
/// **`draw` and `hit_test` both go through this and neither computes a
/// rectangle of its own.** That is what keeps a click landing on the thing it
/// looks like it is on: the two cannot drift, because there is only one set of
/// numbers. It is also what makes the properties that matter testable without a
/// screen — that the destination controls are gone mid-take, that the faders
/// are not, and that the clock is not drawn when the user asked for no clock.
///
/// A rectangle that is not offered is [`Rect::NOTHING`], which contains no
/// point and intersects nothing, so an absent control cannot be clicked by
/// accident and cannot collide with a present one in the overlap test.
/// `Rect::ZERO` would not do: it contains the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Layout {
    /// The take is live, in any of its three senses.
    rolling: bool,
    /// The box the camera image is fitted INTO. Fixed by the band; never by
    /// the camera.
    preview: Rect,

    // ── transport ──
    record: Rect,
    /// The steady red dot, which takes the record button's place while rolling.
    /// An indicator and not a control, so it is not a hit.
    dot: Rect,
    stop: Rect,
    meter: Rect,
    /// The big readout. [`Rect::NOTHING`] when the clock is suppressed.
    timecode: Rect,

    // ── instruments ──
    /// One row each, always all three. See [`SlotRow`].
    slots: [SlotRow; SLOTS],

    // ── monitor ──
    //
    // The whole ROW, caption and number included. The clickable part of it is
    // the track alone — see [`fader_zones`] — because a press anywhere in the
    // row would mean that clicking the word "CLICK" sets it to silence.
    metronome_row: Rect,
    input_row: Rect,
    click: Rect,

    /// The supporter heart, bottom-right of the band.
    ///
    /// **Not a hit** as far as this module is concerned: the band knows where
    /// it goes and nothing more. Cycling its colour and raising the thanks card
    /// on hover belong to whoever owns the licence and the pointer, which is
    /// the app, so it reads this rect and does both itself.
    heart: Rect,

    // ── destination ──
    /// Opens the menu the take's settings moved into. A button rather than
    /// only a right-click, because a control nobody can find is a control that
    /// was removed.
    setup: Rect,
    dest: Rect,
    /// The button that shows the folder in the file manager.
    ///
    /// Its own target rather than a second affordance inside the FOLDER box:
    /// that box already carries "Choose...", and a box that opens a picker when
    /// you click the left of it and a Finder window when you click the right is
    /// a box nobody can predict.
    reveal: Rect,
    default_tick: Rect,
    name: Rect,
    /// Live grey text under the name field. Teaches the naming scheme without
    /// a help page, and is not clickable.
    folder: Rect,
    /// How long the disk will last, beside the folder preview. Also not
    /// clickable: it is an answer, not a question.
    disk: Rect,
    camera: Rect,
    audio: Rect,
    count_in: Rect,
    tempo: Rect,
    /// The time signature, typed rather than dragged. NOT `meter` — that name
    /// is the level meter's, twenty lines up.
    time_sig: Rect,
    export: Rect,
    /// The tick beside Export: show the take's folder as soon as it is done.
    open_when_done: Rect,

    /// One line of status, and the clip warning beside it.
    status: Rect,
    clip: Rect,

    /// Hairlines in the gaps between the groups. Not controls, and not hits —
    /// they are the cheapest way to make four groups look like four groups
    /// rather than like eighteen boxes in a row.
    rules: [Rect; 2],
}

/// One instrument slot's rectangles, resolved against what is in the slot.
///
/// Its own struct rather than five more arrays on [`Layout`] because the
/// interesting thing about a slot row is that its parts are gated by DIFFERENT
/// questions, and each of those questions has a wrong answer that ships as a
/// bug:
///
///   * `pick` goes while a take is rolling. Loading a VST3 blocks the main
///     thread for seconds; offering it at 0:47 is offering to drop the take.
///   * `knob` and `open` do NOT go, for the opposite reason: balancing a layer
///     and changing a preset mid-take are both things people really do, and
///     neither one can touch what has already been written.
///   * `open` is absent when the plugin has no editor. A plugin without one is
///     legal VST3, and a button that cannot do anything is worse than no button.
///   * `knob` and `clear` are absent when the slot is empty, and `name` takes
///     the whole row instead, so an empty slot reads as one wide invitation
///     rather than as a broken row of dead controls.
#[derive(Debug, Clone, Copy, PartialEq)]
struct SlotRow {
    /// The whole row. Drawn, never a hit.
    row: Rect,
    /// The name box. DRAWN in both layouts — which instruments are loaded is
    /// worth knowing during a take — but clickable only at rest, which is what
    /// `pick` is for.
    name: Rect,
    pick: Rect,
    knob: Rect,
    /// The dB reading beside the knob. An answer, not a question.
    value: Rect,
    open: Rect,
    clear: Rect,
}

impl SlotRow {
    const NONE: SlotRow = SlotRow {
        row: Rect::NOTHING,
        name: Rect::NOTHING,
        pick: Rect::NOTHING,
        knob: Rect::NOTHING,
        value: Rect::NOTHING,
        open: Rect::NOTHING,
        clear: Rect::NOTHING,
    };

    fn new(row: Rect, v: &SlotView<'_>, rolling: bool) -> Self {
        if !row.is_positive() {
            return Self::NONE;
        }
        let (name, knob, value, open, clear) = slot_zones(row, v.filled());
        Self {
            row,
            name,
            pick: if rolling { Rect::NOTHING } else { name },
            knob,
            value,
            open: if v.has_editor { open } else { Rect::NOTHING },
            clear: if rolling { Rect::NOTHING } else { clear },
        }
    }
}

/// The five zones of a slot row: name, knob, dB reading, editor button, clear.
///
/// A function rather than more [`SlotRow`] fields for the same reason as
/// [`fader_zones`]: the painter and the hit test have to be looking at exactly
/// the same rectangles, and the test that proves an eight-character dB reading
/// still fits beside a knob at [`DETACHED_MIN`] has to be looking at them too.
///
/// An empty slot gets the whole row as its name box and nothing else. There is
/// no level to set on a slot with nothing in it, and no window to open.
fn slot_zones(row: Rect, filled: bool) -> (Rect, Rect, Rect, Rect, Rect) {
    if !row.is_positive() {
        return (Rect::NOTHING, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
    }
    if !filled {
        return (row, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
    }
    (
        slice_h(row, 0.000, 0.310),
        slice_h(row, 0.330, 0.520),
        slice_h(row, 0.535, 0.680),
        slice_h(row, 0.700, 0.890),
        slice_h(row, 0.905, 1.000),
    )
}

/// Where the three slot rows sit down the monitor column, as fractions of it.
///
/// A constant so the painter, the hit test and the test that proves the rows do
/// not touch each other all read the same numbers. The gaps are real gaps:
/// `Rect::contains` is inclusive at both edges, so two rows that merely meet
/// share a line of points and the overlap test fails there.
/// Where each slot row sits in the column, as a fraction of it.
///
/// Generated rather than written out, because there are six of them now and a
/// hand-typed table of six pairs is a table with a typo in it. Even gaps, and
/// the last row ends at the bottom.
const SLOT_ROWS: [(f32, f32); SLOTS] = {
    let mut out = [(0.0, 0.0); SLOTS];
    let mut i = 0;
    // A twelfth of the column between rows, so six rows breathe.
    let pitch = 1.0 / SLOTS as f32;
    while i < SLOTS {
        let top = i as f32 * pitch;
        out[i] = (top, top + pitch * 0.86);
        i += 1;
    }
    out
};

/// A horizontal slice of `r`, by fraction of its width.
fn slice_h(r: Rect, from: f32, to: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(r.left() + r.width() * from, r.top()),
        Pos2::new(r.left() + r.width() * to, r.bottom()),
    )
}

/// A vertical slice of `r`, by fraction of its height.
fn slice_v(r: Rect, from: f32, to: f32) -> Rect {
    Rect::from_min_max(
        Pos2::new(r.left(), r.top() + r.height() * from),
        Pos2::new(r.right(), r.top() + r.height() * to),
    )
}

/// A one-point vertical hairline down `r` at absolute `x`.
fn rule_x(r: Rect, x: f32) -> Rect {
    if !r.is_positive() {
        return Rect::NOTHING;
    }
    Rect::from_min_max(Pos2::new(x - 0.5, r.top()), Pos2::new(x + 0.5, r.bottom()))
}

/// The monitor column's width, as a fraction of the whole body.
///
/// Of the BODY, and measured from its right edge — not of whatever is left
/// after the preview, and not of whatever is left after the destination. Both
/// of those change when a take starts, and the whole point of this column is
/// that it does not. See the module docs.
///
/// **0.36, up from 0.28**, because this column now carries the three instrument
/// slots as well as the faders, and a slot row has four things in it. The extra
/// eight hundredths come off the destination, which is the group set once at the
/// start of a session — and the destination gets a row of its own height back in
/// the same change, since its INSTRUMENT picker is now each slot's name box.
const MONITOR_W: f32 = 0.36;

/// The three zones of a fader row: icon, track, value.
///
/// A function rather than nine more [`Layout`] fields, because only one of the
/// three is a hit region and all three have to be the same rectangles in the
/// painter and in the test that proves `-60.0 dB` still fits in its box at the
/// smallest band the app will draw.
///
/// **The middle one is the hit region**, and its two ends are the two ends of
/// the fader's travel. See [`along`].
///
/// **The first zone is a picture and not a word.** It was `CLICK` and `INPUT`
/// in 5pt capitals — two labels that between them ate 29% of the monitor's
/// width to say what a metronome and a microphone say at a glance from the
/// bench. The zone is a SQUARE's worth of row rather than a caption's worth,
/// and the eighth of the row it gave up is what pays for the two click
/// switches now sitting in the click's own row. See [`CLICK_SWITCHES`].
fn fader_zones(row: Rect) -> (Rect, Rect, Rect) {
    if !row.is_positive() {
        return (Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
    }
    (
        slice_h(row, 0.00, 0.07),
        slice_h(row, 0.09, 0.73),
        slice_h(row, 0.755, 1.00),
    )
}

/// Where the two click switches sit inside the CLICK fader's row: the tick that
/// says whether you hear it, and the tick that says whether it lands in the
/// file.
///
/// They used to be a row of their own under both faders, which read as two
/// switches belonging to the monitor as a whole — and one of them (`In take`)
/// then vanished when a take started while the other grew to full width, so the
/// row changed shape as well as meaning. Beside the click's own level they are
/// unambiguously the CLICK's two questions, and the row they vacated is what
/// makes both faders a third taller.
///
/// A constant rather than two literals in [`Layout::fill_monitor`] because the
/// test that proves nothing in this row overlaps has to slice the row the same
/// way the painter does.


/// Text height in a fader row, as a fraction of the row. Shared by the painter
/// and by the test that asserts the dB reading fits.
const FADER_TEXT: f32 = 0.62;

impl Layout {
    /// Nothing at all, for a rect too small to hold a band. Every field absent
    /// rather than degenerate: negative rectangles produce NaN centres, and a
    /// NaN centre is a shape drawn nowhere and a hit test that never matches.
    fn empty(rolling: bool) -> Self {
        Self {
            rolling,
            preview: Rect::NOTHING,
            heart: Rect::NOTHING,
            record: Rect::NOTHING,
            dot: Rect::NOTHING,
            stop: Rect::NOTHING,
            meter: Rect::NOTHING,
            timecode: Rect::NOTHING,
            slots: [SlotRow::NONE; SLOTS],
            metronome_row: Rect::NOTHING,
            input_row: Rect::NOTHING,
            click: Rect::NOTHING,
            setup: Rect::NOTHING,
            dest: Rect::NOTHING,
            reveal: Rect::NOTHING,
            default_tick: Rect::NOTHING,
            name: Rect::NOTHING,
            folder: Rect::NOTHING,
            disk: Rect::NOTHING,
            camera: Rect::NOTHING,
            audio: Rect::NOTHING,
            count_in: Rect::NOTHING,
            tempo: Rect::NOTHING,
            time_sig: Rect::NOTHING,
            export: Rect::NOTHING,
            open_when_done: Rect::NOTHING,
            status: Rect::NOTHING,
            clip: Rect::NOTHING,
            rules: [Rect::NOTHING; 2],
        }
    }

    fn new(rect: Rect, view: &RecorderView<'_>) -> Self {
        let rolling = view.state.is_active();
        // The SAME margin on all four sides. It used to be a fraction of the
        // height, which on a wide band is a thin top-and-bottom against a
        // generous left-and-right — and with the status line under the body
        // the bottom read as twice the top.
        let pad = (rect.height() * 0.055).clamp(1.0, 10.0);
        let gap = (rect.height() * 0.05).clamp(1.0, 10.0);
        let inner = rect.shrink(pad);
        if !inner.is_positive() {
            return Self::empty(rolling);
        }

        // The status line spans the whole band in both layouts, so "no audio
        // input selected" is in the same place before and during a take.
        let status_h = (inner.height() * 0.13).min(20.0);
        let status = Rect::from_min_max(
            Pos2::new(inner.left(), inner.bottom() - status_h),
            inner.max,
        );
        let body = Rect::from_min_max(
            inner.min,
            Pos2::new(inner.right(), status.top() - gap * 0.5),
        );
        if !body.is_positive() {
            return Self::empty(rolling);
        }
        let mut l = Self {
            // The message never runs into the clip warning, whatever either
            // says: they share the row and neither may be the reason the other
            // is unreadable.
            status: slice_h(status, 0.0, 0.68),
            clip: slice_h(status, 0.70, 1.0),
            ..Self::empty(rolling)
        };

        if rolling {
            l.fill_rolling(body, gap, view);
        } else {
            l.fill_idle(body, gap, view);
        }
        l
    }

    /// The monitor's rectangle, which is the same one in both layouts.
    fn monitor_of(body: Rect) -> Rect {
        Rect::from_min_max(
            Pos2::new(body.right() - body.width() * MONITOR_W, body.top()),
            Pos2::new(body.right(), body.bottom() - Self::heart_h(body)),
        )
    }

    /// Sit the heart at the bottom-right, inside the band's own margin.
    ///
    /// Sized off the strip it was given rather than off the band, so it stays
    /// the same weight relative to the chrome around it at every window size,
    /// and pushed hard into the corner: it is a credit, not a control, and it
    /// earns its place by staying out of the way.
    fn fill_heart(&mut self, body: Rect, monitor: Rect) {
        let h = (Self::heart_h(body) * 0.85).max(6.0);
        let w = h * crate::chord_strip::HEART_ASPECT;
        self.heart = Rect::from_min_size(
            Pos2::new(body.right() - w, body.bottom() - h),
            Vec2::new(w, h),
        );
        // A slot column too narrow for even that is one where the heart would
        // be a smudge over a fader number, so it is simply not placed.
        if w > monitor.width() * 0.5 {
            self.heart = Rect::NOTHING;
        }
    }

    /// The strip along the bottom of the instrument column that the heart sits
    /// in. Taken off the slots rather than overlaid on them, because the slots
    /// are a list that grows and anything drawn on top of a list is something
    /// the list will eventually reach.
    fn heart_h(body: Rect) -> f32 {
        (body.height() * 0.18).clamp(9.0, 24.0)
    }

    /// Idle: preview, transport, destination, monitor.
    ///
    /// In that order deliberately. The device pickers are NOT first, because
    /// after the first session they are the controls nobody ever touches again;
    /// the faders are last because that is where they are during a take, and
    /// they are the one group that may not move between the two layouts.
    fn fill_idle(&mut self, body: Rect, gap: f32, view: &RecorderView<'_>) {
        // The preview wants to be landscape, so its width follows the band's
        // own HEIGHT — never the camera's aspect, which is the whole point.
        // It gives up about a fifth of what it used to claim: three faders and
        // an instrument row moved in, and a framing view you look at twice a
        // session may not crowd the controls you use every take.
        //
        // The 0.22 cap only binds in a DETACHED window, where the band is tall
        // for its width; in the band itself the height term is smaller than
        // either bound long before the cap matters.
        // **Three groups now, not four.** The take's settings went to the
        // menu, so the band is what you look at (the preview and the name),
        // what you touch every take (transport, meters, faders), and what you
        // load into it (the slots). The middle group is more than twice the
        // width it had, which is where the bigger meters and the reachable
        // faders come from.
        let pv_w = Self::preview_w(body);
        let left = Rect::from_min_max(body.min, Pos2::new(body.left() + pv_w, body.bottom()));
        // The preview keeps a landscape box at the top; the name and the
        // Setup button take the rest of the column.
        // A little over a third to the picture, the rest to the words. The
        // preview is a framing check you glance at twice a session; the name,
        // the tempo and the way into every other take setting are read every
        // time, and at 0.56 to the picture they were four-point type.
        self.preview = slice_v(left, 0.00, 0.44);
        self.fill_setup(slice_v(left, 0.48, 1.00));

        let monitor = Self::monitor_of(body);
        self.fill_heart(body, monitor);
        self.fill_monitor(monitor, view);
        self.rules[1] = rule_x(body, monitor.left() - gap * 0.5);

        let middle = Rect::from_min_max(
            Pos2::new(left.right() + gap, body.top()),
            Pos2::new(monitor.left() - gap, body.bottom()),
        );
        if !middle.is_positive() {
            return;
        }
        self.rules[0] = rule_x(body, left.right() + gap * 0.5);
        self.fill_transport(middle);
        self.fill_faders(body, gap);
    }

    /// The transport column: the round record button, a stop beside it, the
    /// meter under both, and the clock under that.
    ///
    /// The clock now takes the whole width of the column. That is what deleting
    /// the `CLOCK` toggle bought — the timecode used to share its row with a
    /// small mystery box, and the box is what was making the number small.
    /// The middle group's rectangle AT REST, whatever the state.
    ///
    /// The faders are placed from this in both layouts, so the two controls
    /// somebody reaches for with their eyes on their hands do not move when a
    /// take starts. Everything else in the group is free to grow — the clock
    /// triples and the meters get the room the preview gives up — but a fader
    /// that slid sideways at the moment it became the only thing you might
    /// still want to touch would be half a survivor. Same argument the monitor
    /// column has always made; the faders took it with them when they moved.
    fn middle_of(body: Rect, gap: f32) -> Rect {
        let pv_w = Self::preview_w(body);
        let monitor = Self::monitor_of(body);
        Rect::from_min_max(
            Pos2::new(body.left() + pv_w + gap, body.top()),
            Pos2::new(monitor.left() - gap, body.bottom()),
        )
    }

    /// The left column's width, which is the same in both layouts because the
    /// faders are placed against it. See [`Layout::middle_of`].
    fn preview_w(body: Rect) -> f32 {
        (body.height() * 1.05).clamp(body.width() * 0.13, body.width() * 0.20)
    }

    /// The two fader rows, from the at-rest middle. See [`Layout::middle_of`].
    fn fill_faders(&mut self, body: Rect, gap: f32) {
        let m = Self::middle_of(body, gap);
        if !m.is_positive() {
            return;
        }
        // Their own column, between the transport and the meters, and centred
        // in it: two rows in the middle of a tall column rather than two rows
        // pinned to the bottom of the band, which is what made the bottom of
        // the window read as empty.
        let col = slice_h(m, 0.19, 0.63);
        // Tall rows in a narrow column. Moving the pair off the meters bought
        // width to give away and none to spare, so the legibility comes back
        // out of the HEIGHT: the two rows take nearly the whole column, which
        // was dead space above and below them, and the dB reading at the end
        // of each track stays a number rather than a smudge at the smallest
        // band this app will draw.
        self.metronome_row = slice_v(col, 0.18, 0.47);
        self.input_row = slice_v(col, 0.53, 0.82);
        self.click = fader_zones(self.metronome_row).0;
    }

    fn fill_transport(&mut self, t: Rect) {
        if !t.is_positive() {
            return;
        }
        // The whole middle group: the two buttons and the clock across the
        // top, the meters under them at a size worth reading from a bench, and
        // the click and input faders along the bottom — which used to be in
        // the monitor column stealing the room three more instrument slots
        // needed.
        // **Three columns, left to right: transport, faders, meters.** All of
        // them use the group's FULL height, which is where the meters get to
        // be meters: their faces are sized by height, so a row that was a
        // third of the group made them postage stamps however wide the box
        // was. The faders are beside them rather than under them for the same
        // reason — the height they were taking was the height the meters
        // needed.
        let top = slice_h(t, 0.00, 0.15);
        self.meter = slice_h(t, 0.65, 1.00);
        // **The same size, both of them.** Stop used to be 0.66 of record, and
        // then its glyph was shrunk another 30% inside that — so the square
        // read as less than half the circle. Two transport buttons of different
        // sizes look like one is the real control and the other is a note about
        // it.
        //
        // Capped at the width of its own slice, which is what keeps the two
        // from colliding at any aspect the window can take. 0.38 rather than
        // 0.42 because they are now the same width: the centres are 42% of the
        // row apart, so two half-widths have to come to less than that.
        // Stacked, and smaller: two round controls in a narrow column read as
        // a transport, and side by side they were claiming a third of the
        // group for two buttons pressed once a take.
        let d = (top.height() * 0.24).min(top.width() * 0.62);
        self.record = Rect::from_center_size(slice_v(top, 0.06, 0.36).center(), Vec2::splat(d));
        self.stop = Rect::from_center_size(slice_v(top, 0.40, 0.70).center(), Vec2::splat(d));
        // A quarter of the transport rather than an eighteenth. The meter is a
        // pair of VU faces now, and a dial needs HEIGHT in a way a bar never
        // did: the same 34 points that made a perfectly good bar make a face
        // with no room to print anything on it.
        // Under the buttons, in the column they share. At rest it is a
        // readout; while rolling it is the headline it has always been.
        self.timecode = slice_v(top, 0.76, 1.00);
    }

    /// The instrument slots, the two faders and the click.
    ///
    /// The same six rows in the same rectangle before and during a take — see
    /// [`Layout::monitor_of`] — because this is the group that has to still be
    /// there at 0:47, and a control you have to go and find again has not really
    /// survived. What leaves is inside the rows rather than the rows themselves:
    /// each slot's picker (see [`SlotRow`]) and the "in take" tick, which decides
    /// what goes in the FILE and is therefore a destination question wearing a
    /// monitor's clothes.
    ///
    /// There is no INSTRUMENT fader any more. There are three instruments, each
    /// with its own knob, and one fader that claimed to set the level of all of
    /// them would be a fourth control fighting the three real ones.
    fn fill_monitor(&mut self, m: Rect, view: &RecorderView<'_>) {
        if !m.is_positive() {
            return;
        }
        // **Nothing but slots now.** The two faders moved into the middle
        // group with the transport, which is where a hand goes during a take
        // anyway — and the column they vacated is exactly the room three more
        // instruments needed. Six rows where there were three, and an empty
        // one is an invitation rather than a gap: see `EMPTY_SLOT`.
        let rolling = view.state.is_active();
        for (i, (from, to)) in SLOT_ROWS.into_iter().enumerate() {
            self.slots[i] = SlotRow::new(slice_v(m, from, to), &view.slots[i], rolling);
        }
    }

    /// The destination column: seven small rows of things set once.
    ///
    /// Small on purpose. Every one of these is decided before the first take of
    /// a session and then left alone, and the band only has 200 points; giving
    /// the folder path the same weight as the record button is what made the
    /// old one read as a wall.
    ///
    /// **Seven rows, not eight.** The INSTRUMENT row that used to sit between
    /// the folder preview and the camera is gone: it was a picker for the one
    /// instrument, and there are three now, each with its own name box in the
    /// slot rows. Every remaining row is a little taller for it, which is most
    /// of what pays for the narrower column.
    /// What is left of the destination group: a button that opens the menu it
    /// moved into, and the one control in it that has to be TYPED.
    ///
    /// **Everything else went to the right-click menu.** Folder, camera, audio,
    /// count-in, signature, tempo, export, show-when-done: nine controls set
    /// once at the start of a session, sitting in the middle of the band in
    /// front of the transport, which is touched every take. Most of them
    /// already opened a picker or cycled a value, so a menu row is the same
    /// interaction with less furniture — and the space they leave is what
    /// makes room for a bigger meter and a fader you can actually hit.
    ///
    /// The take name stays because it is the only one somebody types, and a
    /// menu cannot hold a text field.
    fn fill_setup(&mut self, d: Rect) {
        if !d.is_positive() {
            return;
        }
        self.name = slice_v(d, 0.00, 0.26);
        self.folder = slice_v(d, 0.29, 0.45);
        // **The tempo stays.** It is the one thing in this group that is
        // DRAGGED, and a menu row cannot be dragged; it is also per-take
        // rather than per-session, since it sets the count-in's speed. The
        // rest of the group went to the menu because a menu could hold it.
        self.tempo = slice_v(d, 0.49, 0.67);
        // The biggest row in the column, because it is now the way to every
        // take setting there is: everything that used to be a box down here
        // lives behind it.
        self.setup = slice_v(d, 0.72, 1.00);
    }

    /// Rolling: readable from the bench. The preview collapses to a strip that
    /// is enough to see somebody has walked in front of the camera and no more,
    /// the two things worth reading from two metres away take most of the rest,
    /// and the monitor column stays exactly where it was.
    fn fill_rolling(&mut self, body: Rect, gap: f32, view: &RecorderView<'_>) {
        // Never wider than the preview at rest: this box only ever COLLAPSES
        // when a take starts, and a rolling preview that grew would run under
        // the faders, which are pinned to the at-rest geometry.
        let pv_w = (body.height() * 1.1)
            .min(body.width() * 0.15)
            .min(Self::preview_w(body));
        self.preview = Rect::from_min_max(body.min, Pos2::new(body.left() + pv_w, body.bottom()));
        let monitor = Self::monitor_of(body);
        self.fill_heart(body, monitor);
        self.fill_monitor(monitor, view);
        self.rules[1] = rule_x(body, monitor.left() - gap * 0.5);

        // **The at-rest middle, not the one the narrower preview leaves.** The
        // preview collapses when a take starts; the transport group may not
        // follow it left, or the meters slide under the faders — which are
        // pinned to the at-rest geometry and do not move — and the band
        // reshuffles at the one moment nobody can afford to look at it.
        let t = Self::middle_of(body, gap);
        if !t.is_positive() {
            return;
        }
        // The faders survive a take for the same reason they always did: the
        // click is turned down mid-performance more than any other control
        // here. They keep the bottom of the group, where they are at rest.
        self.fill_faders(body, gap);
        // **The identical three columns the band has at rest.** Rolling used
        // to rearrange itself — meters narrower, buttons up in a strip, a clock
        // across half the group — and the moment a take starts is the worst
        // moment for the band to move under the eye. The only thing that
        // changes now is that the record button becomes the steady dot, which
        // is the one difference that means something.
        let top = slice_h(t, 0.00, 0.15);
        self.meter = slice_h(t, 0.65, 1.00);

        // The dot stands exactly where the record button stood, at exactly its
        // size. There is no record button while rolling — pressing it would
        // mean nothing, and a dead control is worse than no control.
        let d = (top.height() * 0.24).min(top.width() * 0.62);
        self.dot = Rect::from_center_size(slice_v(top, 0.06, 0.36).center(), Vec2::splat(d));
        self.stop = Rect::from_center_size(slice_v(top, 0.40, 0.70).center(), Vec2::splat(d));

        // The one thing `hide_elapsed` suppresses is a CLOCK. The count-in beat
        // is the number the player is counting, and "FINISHING" is the reason
        // not to close the lid yet; hiding either would be hiding the wrong
        // thing under the name of a performance setting.
        self.timecode = if view.hide_elapsed && matches!(view.state, RecordState::Rolling) {
            Rect::NOTHING
        } else {
            slice_v(top, 0.76, 1.00)
        };
    }

    /// Every clickable region and what it means, in one place.
    ///
    /// [`hit_test`] reads this and so does the test that proves no two of them
    /// overlap, so a control that moves onto another one fails a test rather
    /// than quietly swallowing its clicks.
    fn targets(&self) -> [(Rect, Produces); 37] {
        use Produces::{Along, Fixed, SlotGain};
        let track = |row: Rect| fader_zones(row).1;
        let s = &self.slots;
        [
            (self.record, Fixed(Hit::Record)),
            (self.stop, Fixed(Hit::Stop)),
            (self.setup, Fixed(Hit::OpenSetup)),
            // Written out per slot rather than built in a loop, because a
            // fixed-size array of them is what makes forgetting one a compile
            // error, and because the index in the `Hit` has to be the index of
            // the row it came from — the one thing a clever loop could get
            // subtly wrong and nothing would notice until slot 1's knob started
            // moving slot 0's level.
            (s[0].pick, Fixed(Hit::PickSlot(0))),
            (s[0].knob, SlotGain(0)),
            (s[0].open, Fixed(Hit::OpenSlotEditor(0))),
            (s[0].clear, Fixed(Hit::ClearSlot(0))),
            (s[1].pick, Fixed(Hit::PickSlot(1))),
            (s[1].knob, SlotGain(1)),
            (s[1].open, Fixed(Hit::OpenSlotEditor(1))),
            (s[1].clear, Fixed(Hit::ClearSlot(1))),
            (s[2].pick, Fixed(Hit::PickSlot(2))),
            (s[2].knob, SlotGain(2)),
            (s[2].open, Fixed(Hit::OpenSlotEditor(2))),
            (s[2].clear, Fixed(Hit::ClearSlot(2))),
            (s[3].pick, Fixed(Hit::PickSlot(3))),
            (s[3].knob, SlotGain(3)),
            (s[3].open, Fixed(Hit::OpenSlotEditor(3))),
            (s[3].clear, Fixed(Hit::ClearSlot(3))),
            (s[4].pick, Fixed(Hit::PickSlot(4))),
            (s[4].knob, SlotGain(4)),
            (s[4].open, Fixed(Hit::OpenSlotEditor(4))),
            (s[4].clear, Fixed(Hit::ClearSlot(4))),
            (track(self.metronome_row), Along(Hit::SetMetronomeGain)),
            (track(self.input_row), Along(Hit::SetInputGain)),
            (self.click, Fixed(Hit::ToggleMetronome)),
            (self.dest, Fixed(Hit::ChooseFolder)),
            (self.reveal, Fixed(Hit::RevealFolder)),
            (self.default_tick, Fixed(Hit::ToggleDefaultDir)),
            (self.open_when_done, Fixed(Hit::ToggleOpenWhenDone)),
            (self.name, Fixed(Hit::NameField)),
            (self.camera, Fixed(Hit::PickCamera)),
            (self.audio, Fixed(Hit::PickAudio)),
            (self.count_in, Fixed(Hit::CycleCountIn)),
            (self.tempo, Along(tempo_at)),
            (self.time_sig, Fixed(Hit::EditTimeSignature)),
            (self.export, Fixed(Hit::Export)),
        ]
    }
}

// ── the preview ────────────────────────────────────────────────────────────

/// The whole texture, every time. See [`fit_preview`].
const FULL_FRAME: Rect = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0));

/// Where a camera frame of `source_px` pixels goes inside the box `into`.
///
/// **Letterboxed, not cropped**, and the destination rect shrinks rather than
/// the `uv` — `Painter::image` takes both, and using `uv` to fill the box would
/// crop the sensor frame. Cropping hides what the camera is actually going to
/// record, and framing a piano shot is precisely the job of seeing the edges.
/// So the uv stays [`FULL_FRAME`] and the bars are painted in the band's own
/// background colour.
///
/// The result is centred and always inside `into`, which is the property that
/// keeps the camera's aspect out of the layout: whatever shape the frame is,
/// the box it lands in was decided by [`band_height`] alone.
///
/// Nothing to draw — no box, or a source with no pixels — is
/// [`Rect::NOTHING`] rather than a division by zero.
pub fn fit_preview(into: Rect, source_px: Vec2) -> Rect {
    if !into.is_positive() || source_px.x <= 0.0 || source_px.y <= 0.0 {
        return Rect::NOTHING;
    }
    let source = source_px.x / source_px.y;
    let size = if source > into.width() / into.height() {
        Vec2::new(into.width(), into.width() / source)
    } else {
        Vec2::new(into.height() * source, into.height())
    };
    Rect::from_center_size(into.center(), size)
}

// ── what a click means ─────────────────────────────────────────────────────

/// What is under a click. The app turns one of these into a request and
/// performs it after the frame; nothing here does anything.
///
/// `PartialEq` but **not** `Eq`, because four of the variants carry a float.
/// Comparing two of those for equality is nearly always the wrong question
/// anyway — "is this the same control I was already dragging?" is the right
/// one, and that is [`Hit::is_same_control`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    /// Open the menu the take's settings moved into.
    OpenSetup,
    Record,
    Stop,
    ChooseFolder,
    /// Show the destination folder in the file manager.
    RevealFolder,
    ToggleDefaultDir,
    /// Show the take's folder as soon as the take is finished.
    ToggleOpenWhenDone,
    NameField,
    PickCamera,
    PickAudio,
    /// Open the VST3 instrument picker FOR THIS SLOT. The index is which of the
    /// three rows was clicked, and the app has to keep it: a picker that loads
    /// into whichever slot the app happened to think was current is how you get
    /// an instrument replaced instead of layered.
    PickSlot(usize),
    /// **The obvious button.** Show slot `n`'s own plugin window, and raise it
    /// if it is already on screen. Drawn only when the plugin has an editor.
    OpenSlotEditor(usize),
    /// Unload slot `n`.
    ClearSlot(usize),
    /// Cycle 0 / 4 / 8 beats. See [`next_count_in`].
    CycleCountIn,
    Export,
    ToggleMetronome,
    /// Whether the click is also recorded into the file.
    ToggleMetronomeInTake,
    /// Slot `n`'s knob was pressed or dragged to this 0..=1 position along its
    /// travel.
    ///
    /// The index is **part of the control's identity** and not just payload —
    /// see [`Hit::is_same_control`], which a `mem::discriminant` compare alone
    /// gets wrong in a way that ends with a drag started on slot 0 setting
    /// slot 1's level.
    SetSlotGain(usize, f32),
    /// A fader was pressed or dragged to this 0..=1 position along its travel.
    ///
    /// A POSITION and not a gain: `recorder::fader_to_gain` turns it into one,
    /// and it lives there so that the fader's travel and the audio path's
    /// scaling cannot disagree.
    SetMetronomeGain(f32),
    SetInputGain(f32),
    /// Tempo dragged, already clamped to `MIN_BPM..=MAX_BPM`.
    SetTempo(f64),
    /// Type a time signature into the band. A CLICK opens it, because unlike
    /// every other numeric cell there is nothing to drag: "6/8" is two numbers
    /// and a slash, not a point on a continuum.
    EditTimeSignature,
}

impl Hit {
    /// The value the four dragged variants carry in [`Hit::ALL`].
    ///
    /// Halfway along, which is a real point on every one of their travels and
    /// not an end that might be special. Nothing compares against it: the
    /// reachability test asks [`Hit::is_same_control`], because the whole point
    /// of a dragged control is that the value depends on where you pressed.
    const MIDWAY: f32 = 0.5;

    /// Every control, which is what the reachability test iterates. The
    /// exhaustive match in [`Hit::label`] is what makes adding a variant
    /// without adding it here a compile error rather than an untested control.
    ///
    /// The four per-slot controls appear once per slot, because "reachable" is
    /// a question about slot 2 that slot 0 cannot answer for it.
    pub const ALL: [Hit; 38] = [
        Hit::Record,
        Hit::Stop,
        Hit::OpenSetup,
        Hit::ChooseFolder,
        Hit::RevealFolder,
        Hit::ToggleDefaultDir,
        Hit::ToggleOpenWhenDone,
        Hit::NameField,
        Hit::PickCamera,
        Hit::PickAudio,
        Hit::PickSlot(0),
        Hit::PickSlot(1),
        Hit::PickSlot(2),
        Hit::PickSlot(3),
        Hit::PickSlot(4),
        Hit::OpenSlotEditor(0),
        Hit::OpenSlotEditor(1),
        Hit::OpenSlotEditor(2),
        Hit::OpenSlotEditor(3),
        Hit::OpenSlotEditor(4),
        Hit::ClearSlot(0),
        Hit::ClearSlot(1),
        Hit::ClearSlot(2),
        Hit::ClearSlot(3),
        Hit::ClearSlot(4),
        Hit::CycleCountIn,
        Hit::Export,
        Hit::ToggleMetronome,
        Hit::ToggleMetronomeInTake,
        Hit::SetSlotGain(0, Hit::MIDWAY),
        Hit::SetSlotGain(1, Hit::MIDWAY),
        Hit::SetSlotGain(2, Hit::MIDWAY),
        Hit::SetSlotGain(3, Hit::MIDWAY),
        Hit::SetSlotGain(4, Hit::MIDWAY),
        Hit::SetMetronomeGain(Hit::MIDWAY),
        Hit::SetInputGain(Hit::MIDWAY),
        Hit::SetTempo((MIN_BPM + MAX_BPM) * 0.5),
        Hit::EditTimeSignature,
    ];

    pub fn label(self) -> &'static str {
        // One string per slot, because `label` returns a `&'static str` and
        // cannot format. If `SLOTS` ever changes, these stop compiling, which
        // is the point: a fourth slot whose controls were all called
        // "Instrument 3" would be a menu nobody could read.
        const PICK: [&str; SLOTS] = [
            "Instrument 1",
            "Instrument 2",
            "Instrument 3",
            "Instrument 4",
            "Instrument 5",
        ];
        const OPEN: [&str; SLOTS] = [
            "Open instrument 1's window",
            "Open instrument 2's window",
            "Open instrument 3's window",
            "Open instrument 4's window",
            "Open instrument 5's window",
        ];
        const CLEAR: [&str; SLOTS] = [
            "Clear instrument 1",
            "Clear instrument 2",
            "Clear instrument 3",
            "Clear instrument 4",
            "Clear instrument 5",
        ];
        const GAIN: [&str; SLOTS] = [
            "Instrument 1 level",
            "Instrument 2 level",
            "Instrument 3 level",
            "Instrument 4 level",
            "Instrument 5 level",
        ];
        // A hit is only ever built from a real row, but the index arrives as a
        // `usize` and a panic in a label is a crash in a tooltip.
        let n = |i: usize| i.min(SLOTS - 1);
        match self {
            Hit::Record => "Record",
            Hit::OpenSetup => "Take settings",
            Hit::Stop => "Stop",
            Hit::ChooseFolder => "Choose folder",
            Hit::RevealFolder => "Show the folder",
            Hit::ToggleDefaultDir => "Use this folder by default",
            Hit::ToggleOpenWhenDone => "Show the take when it is finished",
            Hit::NameField => "Take name",
            Hit::PickCamera => "Camera",
            Hit::PickAudio => "Audio input",
            Hit::PickSlot(i) => PICK[n(i)],
            Hit::OpenSlotEditor(i) => OPEN[n(i)],
            Hit::ClearSlot(i) => CLEAR[n(i)],
            Hit::CycleCountIn => "Count-in",
            Hit::Export => "Export",
            // The icon IS the switch, so this label is the only text anywhere
            // that says the metronome can be clicked.
            Hit::ToggleMetronome => "Click on/off",
            Hit::ToggleMetronomeInTake => "Record the click into takes",
            Hit::SetSlotGain(i, _) => GAIN[n(i)],
            Hit::SetMetronomeGain(_) => "Click level",
            Hit::SetInputGain(_) => "Input level",
            Hit::SetTempo(_) => "Tempo",
            Hit::EditTimeSignature => "Time signature",
        }
    }

    /// Whether this control is DRAGGED rather than clicked.
    ///
    /// Exactly the value-carrying variants: they are the ones whose value is a
    /// function of where the pointer is, so they are the ones the app has to
    /// keep following while the button is held. A button has nothing to follow.
    pub fn is_draggable(&self) -> bool {
        matches!(
            self,
            Hit::SetSlotGain(_, _)
                | Hit::SetMetronomeGain(_)
                | Hit::SetInputGain(_)
                | Hit::SetTempo(_)
        )
    }

    /// Which control this is, for [`Hit::is_same_control`]: the variant, plus
    /// the slot for the four that have one.
    ///
    /// **The slot index is half the answer.** `mem::discriminant` alone says
    /// `SetSlotGain(0, _)` and `SetSlotGain(1, _)` are the same control, and a
    /// caller that believes it is still dragging the knob it grabbed would then
    /// set slot 1's level from a drag that started on slot 0's knob — silently,
    /// and only once the pointer wandered a row.
    fn control_key(self) -> (std::mem::Discriminant<Hit>, usize) {
        let slot = match self {
            Hit::PickSlot(i)
            | Hit::OpenSlotEditor(i)
            | Hit::ClearSlot(i)
            | Hit::SetSlotGain(i, _) => i,
            _ => 0,
        };
        (std::mem::discriminant(&self), slot)
    }

    /// Whether two hits are the same CONTROL, ignoring any value one carries.
    ///
    /// `SetTempo(90.0)` and `SetTempo(140.0)` are one control being dragged,
    /// which is the question a caller holding the mouse button down actually
    /// has, and the question `==` cannot answer. `SetSlotGain(0, 0.2)` and
    /// `SetSlotGain(1, 0.2)` are two different controls at the same position,
    /// which is the question `==` gets right and a discriminant gets wrong.
    pub fn is_same_control(self, other: Hit) -> bool {
        self.control_key() == other.control_key()
    }
}

/// Which typeable field a hit belongs to, if any.
///
/// Exactly the draggable hits and no others, which is the point: a control you
/// can drag to a number is a control you should be able to type the number
/// into, and there is no third kind.
pub fn num_field(hit: Hit) -> Option<NumField> {
    match hit {
        Hit::EditTimeSignature => Some(NumField::Meter),
        Hit::SetSlotGain(i, _) => Some(NumField::Slot(i)),
        Hit::SetMetronomeGain(_) => Some(NumField::Metronome),
        Hit::SetInputGain(_) => Some(NumField::Input),
        Hit::SetTempo(_) => Some(NumField::Tempo),
        _ => None,
    }
}

/// What a rectangle produces when it is pressed.
///
/// Two kinds, because four of the controls are DRAGGED: their answer depends on
/// where along the rectangle the press landed. Making that a property of the
/// region rather than of the caller is what keeps [`hit_test`] a pure function
/// of geometry — the same point always produces the same hit, so the caller can
/// simply ask again on every pointer move.
#[derive(Debug, Clone, Copy)]
enum Produces {
    /// The same hit wherever inside it you press.
    Fixed(Hit),
    /// A hit carrying how far along the rect the press landed, 0..=1.
    Along(fn(f32) -> Hit),
    /// The same, for a knob that also has to say which slot it belongs to. Its
    /// own variant rather than a closure so that [`Layout::targets`] stays a
    /// plain `Copy` array with no allocation and no lifetime.
    SlotGain(usize),
}

impl Produces {
    fn hit(self, t: f32) -> Hit {
        match self {
            Produces::Fixed(h) => h,
            Produces::Along(f) => f(t),
            Produces::SlotGain(i) => Hit::SetSlotGain(i, t.clamp(0.0, 1.0)),
        }
    }
}

/// A tempo from a position along its control, clamped to what the SMF writer
/// and every DAW's bar ruler will accept.
///
/// Linear across the whole legal range, and absolute rather than relative,
/// because a relative drag needs to remember where the drag started and this
/// function is not allowed to remember anything. The number is drawn beside the
/// control for exactly that reason.
fn tempo_at(t: f32) -> Hit {
    Hit::SetTempo(MIN_BPM + f64::from(t.clamp(0.0, 1.0)) * (MAX_BPM - MIN_BPM))
}

/// How far along `r` the point `pos` fell, 0..=1 from its left edge to its
/// right.
///
/// Clamped, so both ends of a fader's travel are reachable rather than being a
/// value the last pixel rounds off the end of. `r.left()` is exactly 0.0,
/// `r.right()` is exactly 1.0, and the middle is exactly 0.5.
fn along(r: Rect, pos: Pos2) -> f32 {
    if r.width() <= 0.0 {
        return 0.0;
    }
    ((pos.x - r.left()) / r.width()).clamp(0.0, 1.0)
}

/// What is under `pos`, if anything.
///
/// The exact inverse of [`draw`], by construction rather than by discipline:
/// both read the same [`Layout`].
///
/// # Dragging
///
/// Six of the hits carry a value taken from where in their rectangle the press
/// landed. **The caller is expected to call this again on every pointer move
/// while the button is held**, and to keep hold of which control it grabbed —
/// this function has no memory, so a pointer that wanders off the track returns
/// whatever is under it now, or `None`. Comparing the new hit to the grabbed one
/// with [`Hit::is_same_control`] is how the caller tells "still dragging the
/// click fader" from "the pointer is over the record button", and — since the
/// three slot knobs are stacked one above another — "still on slot 0's knob"
/// from "the pointer has slipped onto slot 1's".
///
/// # While a take is live
///
/// [`Hit::Stop`], the two faders, the three slot knobs, the three editor
/// buttons and [`Hit::ToggleMetronome`] survive. Every destination control is
/// gone, which is not a courtesy — the output folder, the take name and the
/// devices are all decided at `T0` and a UI that lets you change them at 0:47 is
/// a UI that promises something it cannot do. The instrument PICKERS go with
/// them, for a harder reason still: loading a VST3 blocks the main thread for
/// seconds, and doing that during a take costs the take.
///
/// The levels survive for the opposite reason: turning the click down halfway
/// through a take is precisely when you need to, and the level you hear changes
/// nothing about the file. So do the editor buttons — reaching for a different
/// preset between two passes is a real thing, and it is the plugin's own window
/// that does it. The one monitor control that DOES change the file — whether the
/// click is recorded into it — goes away with the destination.
/// Where the Setup button is, so the app can open the menu at it.
/// Where the supporter heart goes: bottom-right of the band, in its own strip.
///
/// [`Rect::NOTHING`] when the band is too small to carry it. The app draws it
/// and owns every gesture on it; this is only the geometry, in the one file
/// that knows the band's geometry.
pub fn heart_rect(rect: Rect, view: &RecorderView<'_>) -> Rect {
    Layout::new(rect, view).heart
}

pub fn setup_rect(rect: Rect, view: &RecorderView<'_>) -> Rect {
    Layout::new(rect, view).setup
}

pub fn hit_test(rect: Rect, view: &RecorderView<'_>, pos: Pos2) -> Option<Hit> {
    if !rect.contains(pos) {
        return None;
    }
    Layout::new(rect, view)
        .targets()
        .into_iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(r, k)| k.hit(along(r, pos)))
}

/// The next count-in length in the cycle, in beats.
///
/// Here rather than in the app so that the control's label and the effect of
/// clicking it cannot disagree. An unknown value — a hand-edited settings file
/// — lands on the first choice rather than sticking.
pub fn next_count_in(current: u32) -> u32 {
    let i = COUNT_IN_CHOICES
        .iter()
        .position(|&c| c == current)
        .map_or(0, |i| i + 1);
    COUNT_IN_CHOICES[i % COUNT_IN_CHOICES.len()]
}

// ── drawing ────────────────────────────────────────────────────────────────

/// Draw the band. A pure painter: no `Ui`, no `Context`, no input, no clock.
///
/// The absence of a clock is not tidiness either. §5's re-export table promises
/// that a display-only video can be rebuilt after the fact by replaying the
/// recorded MIDI through these same `draw` functions, and that promise is worth
/// exactly as much as "the display at any instant is a function of the state at
/// that instant". One `animate_bool` here and a take could never be re-rendered.
pub fn draw(painter: &Painter, rect: Rect, view: &RecorderView<'_>, s: &Settings) {
    let p = palette(s);
    painter.rect_filled(rect, 0.0, p.bg);
    // Nothing escapes the band. Text is shrunk to fit its box rather than
    // truncated, but a pathologically long path still has to stop at the edge
    // instead of being painted over the piano.
    let painter = &painter.with_clip_rect(rect);
    let l = Layout::new(rect, view);

    draw_preview(painter, &l, view, &p);
    draw_transport(painter, &l, &p);
    draw_meter(painter, l.meter, view.meters, &p);
    draw_readout(painter, &l, view, &p);
    draw_monitor(painter, &l, view, &p);
    if !l.rolling {
        draw_destination(painter, &l, view, s, &p);
        // The button that opens the menu the rest of the destination group
        // moved into. Every rect it left behind is `Rect::NOTHING`, which
        // every draw here already treats as "not on screen", so nothing above
        // needed a condition adding to it.
        if l.setup.is_positive() {
            control(painter, l.setup, &p);
            let size = fit_text(l.setup, "SETUP...", l.setup.height() * 0.5);
            if size >= MIN_TEXT {
                painter.text(
                    l.setup.center(),
                    Align2::CENTER_CENTER,
                    "SETUP...",
                    font(size),
                    p.ink,
                );
            }
        }
    }
    for r in l.rules {
        if r.is_positive() {
            painter.rect_filled(r, 0.0, p.line.gamma_multiply(0.55));
        }
    }
    draw_status(painter, &l, view, &p);

    if view.state.is_active() {
        // A STEADY border, and a steady dot up in the transport. Never
        // blinking: a blinking indicator measurably degrades performance and
        // is the most-cited psychological complaint in the piano forums, which
        // is also why this band owns no animation state at all.
        painter.rect_stroke(
            rect.shrink(1.5),
            0.0,
            Stroke::new(3.0_f32, p.rec),
            StrokeKind::Middle,
        );
    }
}

/// A control's chrome: a filled box with a hairline, so a thing you can click
/// looks unlike a thing you read.
fn control(painter: &Painter, r: Rect, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 2.0, p.field);
    painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
}

/// How far a labelled box's text stands in from its edges.
///
/// Bounded by the WIDTH as well as the height, which the first version was not:
/// in a tall narrow box — a detached window's export button — a 30%-of-height
/// inset ate the whole box and the label silently stopped being drawn.
fn label_inset(r: Rect) -> f32 {
    (r.height() * 0.30).min(r.width() * 0.06)
}

/// A labelled control: the caption in the faint ink, the value after it.
///
/// One helper because most of the destination column is this shape, and eight
/// copies would drift apart the first time one of them was adjusted.
fn labelled(painter: &Painter, r: Rect, cap: &str, value: &str, colour: Color32, p: &Palette) {
    control(painter, r, p);
    label_text(painter, r, cap, value, colour, p);
}

/// The caption and the value, with no box under them.
///
/// Split out of [`labelled`] for the one place that has to draw the same pair
/// of words WITHOUT the chrome: a slot's name while a take is rolling, when it
/// is a thing to read and not a thing to press. Same insets, same sizing, so
/// the row does not jump at the moment the take starts.
fn label_text(painter: &Painter, r: Rect, cap: &str, value: &str, colour: Color32, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    let inset = label_inset(r);
    let inner = Rect::from_min_max(
        Pos2::new(r.left() + inset, r.top()),
        Pos2::new(r.right() - inset, r.bottom()),
    );
    if !inner.is_positive() {
        return;
    }
    let joined = format!("{cap} {value}");
    let size = fit_text(inner, &joined, inner.height() * 0.52);
    if size < MIN_TEXT {
        return;
    }
    let y = inner.center().y;
    let after = painter
        .text(
            Pos2::new(inner.left(), y),
            Align2::LEFT_CENTER,
            cap,
            font_light(size),
            p.faint,
        )
        .right();
    painter.text(
        Pos2::new(after + size * ADV, y),
        Align2::LEFT_CENTER,
        value,
        font(size),
        colour,
    );
}

/// The camera, as a PANE OF THE WINDOW rather than a box in the Recorder band.
///
/// **This is the whole of the camera's presence in the app now.** It sits
/// beside the theory diagrams, it is what you frame a shot in, and — because
/// the video is the window's own bands laid into the video's frame — it is also
/// exactly what lands in the recording. There is no second model of where the
/// camera goes and therefore nothing for the two models to disagree about,
/// which is what every camera-layout bug in this app has been.
///
/// Public because it is painted from `app.rs`, from the two places that draw
/// bands: the live window and the offscreen compositor.
pub fn draw_camera_pane(
    painter: &Painter,
    rect: Rect,
    preview: Option<Preview>,
    camera: DeviceLabel<'_>,
    surround: Color32,
    s: &Settings,
) {
    // The band it SITS in, not the band it came from. The letterbox bars round
    // a 4:3 camera are the theory panel's own background, so the row reads as
    // one band with a camera in it rather than as a hole cut in the window.
    let mut p = palette(s);
    p.bg = surround;
    draw_camera(painter, rect, preview, camera, &p);
}

fn draw_preview(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    draw_camera(painter, l.preview, view.preview, view.camera, p);
}

/// One camera box, wherever it is.
fn draw_camera(
    painter: &Painter,
    r: Rect,
    preview: Option<Preview>,
    camera: DeviceLabel<'_>,
    p: &Palette,
) {
    if !r.is_positive() {
        return;
    }
    match preview {
        Some(pv) => {
            // The bars are the BAND's background, not a black mat: the box is
            // fixed by the layout and the frame is fitted into it, so whatever
            // is left over has to look like the band and not like a fault.
            painter.rect_filled(r, 0.0, p.bg);
            let dst = fit_preview(r, pv.size);
            if dst.is_positive() {
                painter.image(pv.texture, dst, FULL_FRAME, Color32::WHITE);
            }
        }
        None => {
            painter.rect_filled(r, 0.0, p.well);
            // Say what to do. An empty grey box is indistinguishable from a
            // camera pointed at a wall, and one of those is the user's problem
            // to fix while the other is not.
            let (top, hint) = match camera {
                DeviceLabel::None => ("NO CAMERA SELECTED", "choose one on the right"),
                DeviceLabel::Missing(_) => ("CAMERA NOT AVAILABLE", "it is not plugged in"),
                DeviceLabel::Open(_) => ("WAITING FOR CAMERA", "the first frame has not arrived"),
            };
            let size =
                fit_text(r, hint, r.height() * 0.11).min(fit_text(r, top, r.height() * 0.14));
            if size >= MIN_TEXT {
                painter.text(
                    Pos2::new(r.center().x, r.center().y - size),
                    Align2::CENTER_CENTER,
                    top,
                    font(size),
                    p.ink.gamma_multiply(0.8),
                );
                painter.text(
                    Pos2::new(r.center().x, r.center().y + size),
                    Align2::CENTER_CENTER,
                    hint,
                    font_light(size * 0.9),
                    p.faint,
                );
            }
        }
    }
    painter.rect_stroke(r, 0.0, Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
}

fn draw_transport(painter: &Painter, l: &Layout, p: &Palette) {
    if l.record.is_positive() {
        let c = l.record.center();
        let rad = l.record.width() * 0.5;
        painter.circle_filled(c, rad, p.rec);
        painter.circle_stroke(c, rad, Stroke::new(1.5_f32, p.line));
    }
    // The two glyphs sit in rects of the SAME size — and that alone did not
    // make them look the same size, which is the part the first attempt got
    // wrong. The circle fills its rect; the square was inset another 9%, so it
    // was 82% of the circle's diameter and read as the smaller control.
    //
    // A square of side D beside a circle of diameter D looks BIGGER, because it
    // is 27% more ink. Equal AREA is the honest match: side = D * sqrt(pi)/2 =
    // 0.886 D, which is the inset used below.
    if l.dot.is_positive() {
        painter.circle_filled(l.dot.center(), l.dot.width() * 0.5, p.rec);
    }
    if l.stop.is_positive() {
        // Present in BOTH layouts, in the same place. A stop button that only
        // appears once the take has started is one you have to find while your
        // hands are on the keys.
        //
        // Drawn like the record button and not like a menu row: a filled shape
        // with a hairline round it, at the same size. It used to sit inside a
        // `control` box, which is the chrome every LABELLED row in this band
        // wears — so the two transport buttons were not merely different sizes
        // but different KINDS of thing.
        //
        // The square is inset a little because an inscribed square is 27%
        // more ink than the circle it shares a diameter with, and equal
        // measurements are not equal weight.
        const STOP_INSET: f32 = (1.0 - 0.886) * 0.5;
        let stop = l.stop.shrink(l.stop.width() * STOP_INSET);
        painter.rect_filled(stop, 1.0, if l.rolling { p.ink } else { p.faint });
        painter.rect_stroke(
            stop,
            1.0,
            Stroke::new(1.5_f32, p.line),
            StrokeKind::Inside,
        );
    }
}

// ── the monitor ────────────────────────────────────────────────────────────

fn draw_monitor(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    for (i, slot) in view.slots.iter().enumerate() {
        let typing = typing_for(view, NumField::Slot(i));
        draw_slot(painter, &l.slots[i], i, slot, l.rolling, typing, p);
    }
    // **The metronome IS the click switch**, and its two states are the icon
    // at full strength and the icon faded into the panel. There is no tick
    // beside it any more: a picture of a metronome with a checkbox next to it
    // was two controls for one question, and the icon was already the most
    // clickable-looking thing in the row.
    let click_ink = if view.metronome_on {
        p.faint
    } else {
        toward(p.faint, p.bg, 0.68)
    };
    for (row, icon, ink, gain, field) in [
        (
            l.metronome_row,
            FaderIcon::Metronome,
            click_ink,
            view.gains.metronome,
            NumField::Metronome,
        ),
        (
            l.input_row,
            FaderIcon::Microphone,
            p.faint,
            view.gains.input,
            NumField::Input,
        ),
    ] {
        draw_fader(painter, row, icon, ink, gain, typing_for(view, field), p);
    }
    // **Whether the click ends up in the FILE has no control of its own.** It
    // is set once a year and it was sitting in the busiest row of the band
    // wearing a box and a caption, which is a lot of furniture for a question
    // nobody asks twice. It lives on a right-click of the metronome now, and
    // says so only when it is ON: a dot on the icon, because a setting that
    // puts a click track into your recording may not be invisible.
    if view.metronome_in_take && l.click.is_positive() {
        let r = l.click;
        let d = (r.height() * 0.16).clamp(2.0, 5.0);
        painter.circle_filled(
            Pos2::new(r.right() - d, r.top() + d),
            d,
            if view.metronome_on { p.ink } else { p.faint },
        );
    }
}

// ── the instrument slots ───────────────────────────────────────────────────

/// What an empty slot says, at rest and during a take.
///
/// It says something. An empty box in a row of full ones reads as a rendering
/// fault; "empty" with an instruction beside it reads as an offer, and that is
/// the whole reason all three rows are drawn whether or not anything is in them.
/// The instruction goes away during a take because it would be a lie: the picker
/// is not reachable then.
const EMPTY_SLOT: &str = "empty  (click to load)";
const EMPTY_SLOT_ROLLING: &str = "empty";

/// One slot row: which instrument, how loud, its window, and a way to unload it.
fn draw_slot(
    painter: &Painter,
    r: &SlotRow,
    i: usize,
    v: &SlotView<'_>,
    rolling: bool,
    typing: Option<&str>,
    p: &Palette,
) {
    if !r.row.is_positive() {
        return;
    }
    // The row's own number, so three slots read as slot one, two and three
    // rather than as three copies of the same control. Sized by `SLOTS`, so a
    // fourth slot has to be given a name here rather than quietly being a
    // second slot three.
    const NUMBER: [&str; SLOTS] = ["1", "2", "3", "4", "5"];
    let n = NUMBER[i.min(SLOTS - 1)];
    let (value, ink) = match (v.name, v.missing) {
        (None, _) => (
            if rolling {
                EMPTY_SLOT_ROLLING.to_owned()
            } else {
                EMPTY_SLOT.to_owned()
            },
            p.faint,
        ),
        // "did not load" and not "not connected": telling somebody their
        // instrument is not connected sends them looking for a cable.
        (Some(name), true) => (format!("{name}  (did not load)"), p.warn),
        (Some(name), false) => (name.to_owned(), p.ink),
    };
    if rolling {
        // Text, not a box. The picker is unreachable during a take, and a
        // control that looks pressable and is not is worse than no control.
        label_text(painter, r.name, n, &value, ink, p);
    } else {
        labelled(painter, r.name, n, &value, ink, p);
    }

    draw_track(painter, r.knob, v.gain, p);
    draw_gain_value(painter, r.value, v.gain, typing, p);
    draw_open_button(painter, r.open, v.editor_open, p);
    draw_clear_button(painter, r.clear, p);
}

/// The button the owner asked for in capitals.
///
/// A filled box with a border and a centred word in the bold face, which in this
/// band is what a button looks like and nothing else is: every other box carries
/// a left-aligned caption and a value. It used to be a row in a right-click
/// submenu, which is to say it used to be invisible.
///
/// Lit — a tint of the accent and a two-point border instead of one — while the
/// plugin's window is already on screen, because pressing it then RAISES that
/// window rather than doing nothing, and the button has to look like it knows
/// the difference. A TINT and not a solid fill: the accent is whatever colour
/// the user chose for a held key, and a word printed on top of an unknown colour
/// is a word that might not be readable.
fn draw_open_button(painter: &Painter, r: Rect, open: bool, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 2.0, p.field);
    if open {
        painter.rect_filled(r, 2.0, p.accent.gamma_multiply(0.28));
    }
    painter.rect_stroke(
        r,
        2.0,
        if open {
            Stroke::new(2.0_f32, p.accent)
        } else {
            Stroke::new(1.0_f32, p.line)
        },
        StrokeKind::Inside,
    );
    // "OPEN" alone could be opening a file. The longer wording is drawn
    // wherever it fits at full size, which is the main window and any detached
    // window worth detaching.
    let (text, size) = fit_label(r, &["OPEN WINDOW", "WINDOW", "OPEN"], r.height() * 0.46);
    if size >= MIN_TEXT {
        painter.text(r.center(), Align2::CENTER_CENTER, text, font(size), p.ink);
    }
}

/// A plain word in a button-shaped box.
///
/// [`draw_open_button`] with its lit state taken out, for the buttons that are
/// pressed rather than toggled. Sharing the chrome is what stops the band
/// growing a second thing that is nearly, but not quite, a button.
fn draw_word_button(painter: &Painter, r: Rect, choices: &[&str], p: &Palette) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 2.0, p.field);
    painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
    let (text, size) = fit_label(r, choices, r.height() * 0.46);
    if size >= MIN_TEXT {
        painter.text(r.center(), Align2::CENTER_CENTER, text, font(size), p.ink);
    }
}

/// The longest of `choices` that draws at `nominal` without being shrunk.
///
/// Ordered longest first. The last is the fallback and is shrunk to fit like any
/// other text, so one button can say "OPEN WINDOW" in the main window and "OPEN"
/// in a 640-point detached one without either of them being a smudge. A pure
/// function of the rectangle, so the two layouts and the offscreen compositor
/// all pick the same word.
fn fit_label<'t>(r: Rect, choices: &[&'t str], nominal: f32) -> (&'t str, f32) {
    for c in choices {
        if fit_text(r, c, nominal) >= nominal {
            return (c, nominal);
        }
    }
    match choices.last() {
        Some(c) => (c, fit_text(r, c, nominal)),
        None => ("", 0.0),
    }
}

/// Unload the slot: a cross, drawn as two segments.
///
/// Segments rather than a glyph for the same reason [`draw_tick`] draws its
/// check that way — no bundled face is guaranteed to carry one, and a tofu box
/// is indistinguishable from a control that means something else.
fn draw_clear_button(painter: &Painter, r: Rect, p: &Palette) {
    control(painter, r, p);
    if !r.is_positive() {
        return;
    }
    let arm = (r.height().min(r.width()) * 0.24).max(1.0);
    let c = r.center();
    let s = Stroke::new((arm * 0.34).max(1.0), p.faint);
    painter.line_segment(
        [
            Pos2::new(c.x - arm, c.y - arm),
            Pos2::new(c.x + arm, c.y + arm),
        ],
        s,
    );
    painter.line_segment(
        [
            Pos2::new(c.x + arm, c.y - arm),
            Pos2::new(c.x - arm, c.y + arm),
        ],
        s,
    );
}

/// Which of the two monitor faders a row is, which is the whole of what its
/// leading square says.
///
/// An enum rather than a caption string: these are the only two faders in the
/// band, each is drawn rather than typeset, and a `&str` parameter would invite
/// a third one whose picture nobody had drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FaderIcon {
    Metronome,
    Microphone,
}

/// One fader: its icon, a slotted track with a handle in it, and the level in
/// dB. All three, because "clear volume knobs" was the brief and a bare strip
/// of colour is none of them.
///
/// The dB reading is the part that is easy to leave out and the part that makes
/// the control usable: a handle two thirds along means nothing on its own, and
/// `-6.0 dB` means something to everyone who has ever used a mixer.
fn draw_fader(
    painter: &Painter,
    row: Rect,
    icon: FaderIcon,
    ink: Color32,
    gain: f32,
    typing: Option<&str>,
    p: &Palette,
) {
    if !row.is_positive() {
        return;
    }
    let (icon_r, track, val_r) = fader_zones(row);
    draw_fader_icon(painter, icon_r, icon, ink, p);
    draw_gain_value(painter, val_r, gain, typing, p);
    draw_track(painter, track, gain, p);
}

/// The square at the head of a fader row.
///
/// **Drawn, not typeset.** The same reason [`draw_tick`] draws its check mark
/// out of two line segments: no face this app bundles is guaranteed to carry a
/// metronome or a microphone, and a tofu box at the head of the click fader
/// would read as a broken control rather than as a missing glyph. These are a
/// handful of line segments and one polygon, so they are the same picture on
/// every platform and inside the offscreen compositor, which has no font
/// fallback chain at all.
///
/// Both are inscribed in the largest square the zone will hold, so the two
/// icons are the same size as each other whatever the row's aspect — a
/// metronome larger than the microphone beside it would read as the louder of
/// the two controls.
fn draw_fader_icon(painter: &Painter, r: Rect, icon: FaderIcon, ink: Color32, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    let s = r.width().min(r.height()) * 0.92;
    if s < 6.0 {
        // Below this the shapes collapse into a blot, which reads worse than
        // an empty margin. Same bargain as `MIN_TEXT`.
        return;
    }
    let b = Rect::from_center_size(Pos2::new(r.left() + s * 0.5, r.center().y), Vec2::splat(s));
    match icon {
        FaderIcon::Metronome => draw_metronome(painter, b, ink, p),
        FaderIcon::Microphone => draw_microphone(painter, b, ink),
    }
}

/// A colour faded `t` of the way into the background it sits on.
///
/// Toward the BACKGROUND rather than scaled toward black, which is what
/// [`shade`] does. Scaling would darken a faded icon on a cream band as well as
/// on a walnut one, and on the cream band darker is more prominent — the "off"
/// state would shout.
fn toward(c: Color32, bg: Color32, t: f32) -> Color32 {
    let f = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * t).round() as u8;
    Color32::from_rgb(f(c.r(), bg.r()), f(c.g(), bg.g()), f(c.b(), bg.b()))
}

/// A metronome: the tapered case, with the pendulum and its weight cut out of
/// it in the background colour.
///
/// Cut out rather than stroked on top, because at 16 points a rod drawn in a
/// second ink over a filled body is two shades of mud, while a gap in a solid
/// shape stays a gap however small it gets.
fn draw_metronome(painter: &Painter, b: Rect, ink: Color32, p: &Palette) {
    let s = b.width();
    let (cx, top, bottom) = (b.center().x, b.top() + s * 0.06, b.bottom() - s * 0.06);
    painter.add(egui::Shape::convex_polygon(
        vec![
            Pos2::new(cx - s * 0.14, top),
            Pos2::new(cx + s * 0.14, top),
            Pos2::new(cx + s * 0.38, bottom),
            Pos2::new(cx - s * 0.38, bottom),
        ],
        ink,
        Stroke::NONE,
    ));
    // The rod leans, because a metronome at rest is the one picture of a
    // metronome nobody recognises.
    let foot = Pos2::new(cx - s * 0.03, bottom - s * 0.07);
    let head = Pos2::new(cx + s * 0.19, top + s * 0.05);
    painter.line_segment([foot, head], Stroke::new((s * 0.075).max(1.0), p.bg));
    let along = |t: f32| Pos2::new(foot.x + (head.x - foot.x) * t, foot.y + (head.y - foot.y) * t);
    painter.rect_filled(
        Rect::from_center_size(along(0.36), Vec2::new(s * 0.17, s * 0.085)),
        1.0,
        p.bg,
    );
}

/// A microphone: capsule, cradle, stem, foot.
///
/// The cradle is what makes it a microphone rather than a pill — a capsule on
/// its own reads as a battery — so it is an arc of eleven points rather than
/// the three-segment bracket that would have been cheaper.
fn draw_microphone(painter: &Painter, b: Rect, ink: Color32) {
    let s = b.width();
    let cx = b.center().x;
    let top = b.top() + s * 0.04;
    let capsule = Rect::from_min_max(
        Pos2::new(cx - s * 0.17, top),
        Pos2::new(cx + s * 0.17, top + s * 0.50),
    );
    painter.rect_filled(capsule, s * 0.17, ink);
    let stroke = Stroke::new((s * 0.09).max(1.0), ink);
    let (pivot_y, radius) = (top + s * 0.36, s * 0.30);
    let arc: Vec<Pos2> = (0_u8..=10)
        .map(|i| {
            let t = std::f32::consts::PI * f32::from(i) / 10.0;
            Pos2::new(cx - radius * t.cos(), pivot_y + radius * t.sin())
        })
        .collect();
    painter.add(egui::Shape::line(arc, stroke));
    let foot_y = b.bottom() - s * 0.04;
    painter.line_segment(
        [Pos2::new(cx, pivot_y + radius), Pos2::new(cx, foot_y)],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(cx - s * 0.22, foot_y),
            Pos2::new(cx + s * 0.22, foot_y),
        ],
        stroke,
    );
}

/// What a control is showing while it is being typed into, if it is.
///
/// A borrow out of the view rather than a flag, so that the panel draws the
/// text the app is actually holding — a copy would be a second place for the
/// caret and the characters to disagree.
fn typing_for<'a>(view: &RecorderView<'a>, field: NumField) -> Option<&'a str> {
    view.editing
        .filter(|e| e.field == field)
        .map(|e| e.text.as_str())
}

/// The level in dB, right-aligned in its own box.
///
/// Shared by the two faders and by the three slot knobs, which is why it is a
/// function: a knob small enough to be called tiny needs its number MORE than a
/// full-width fader does, and two implementations of "what does this control say
/// it is set to" would drift.
fn draw_gain_value(painter: &Painter, r: Rect, gain: f32, typing: Option<&str>, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    if let Some(typed) = typing {
        draw_typed_value(painter, r, typed, p);
        return;
    }
    let reading = gain_text(gain);
    let size = fit_text(r, &reading, r.height() * FADER_TEXT);
    if size < MIN_TEXT {
        return;
    }
    painter.text(
        Pos2::new(r.right(), r.center().y),
        Align2::RIGHT_CENTER,
        &reading,
        font(size),
        // OFF is a state and not a level, and it is the one setting that
        // will make somebody swear at a silent recording, so it does not
        // get to look like a number.
        if gain <= 0.0 { p.faint } else { p.ink },
    );
}

/// What a right-aligned numeric box shows while it is being typed into: the
/// characters so far, and a caret after them.
///
/// The box keeps its size and its alignment, so the row does not move at the
/// moment somebody starts typing — a control that jumps when you click it is
/// one you click twice.
fn draw_typed_value(painter: &Painter, r: Rect, typed: &str, p: &Palette) {
    // Sized against a full-width reading rather than against `typed`, or the
    // characters would start enormous and shrink as they were entered.
    let size = fit_text(r, "-00.0 dB", r.height() * FADER_TEXT);
    if size < MIN_TEXT {
        return;
    }
    // Room for the caret is taken out of the box BEFORE the text is placed,
    // which is what keeps the caret inside a right-aligned field instead of
    // hanging off the end of it.
    let caret = size * ADV;
    let end = r.right() - caret;
    if !typed.is_empty() {
        painter.text(
            Pos2::new(end, r.center().y),
            Align2::RIGHT_CENTER,
            typed,
            font(size),
            p.ink,
        );
    }
    painter.line_segment(
        [
            Pos2::new(end + caret * 0.4, r.center().y - size * 0.5),
            Pos2::new(end + caret * 0.4, r.center().y + size * 0.5),
        ],
        Stroke::new(1.5_f32, p.ink),
    );
}

/// The caret inside a caption+value box, after `typed_chars` characters of the
/// value.
///
/// Shared by the take name and the tempo. They looked identical when they were
/// two copies, which is exactly when two copies start to drift.
fn draw_caption_caret(
    painter: &Painter,
    r: Rect,
    cap: &str,
    shown: &str,
    typed_chars: usize,
    p: &Palette,
) {
    if !r.is_positive() {
        return;
    }
    let inset = label_inset(r);
    let inner = Rect::from_min_max(
        Pos2::new(r.left() + inset, r.top()),
        Pos2::new(r.right() - inset, r.bottom()),
    );
    if !inner.is_positive() {
        return;
    }
    let size = fit_text(inner, &format!("{cap} {shown}"), inner.height() * 0.52);
    if size < MIN_TEXT {
        return;
    }
    // The caret goes after what has been TYPED, which is not what is drawn
    // when the field is empty and showing a placeholder.
    let before = cap.chars().count() + 1 + typed_chars;
    let x = inner.left() + size * ADV * before as f32;
    painter.line_segment(
        [
            Pos2::new(x, r.center().y - size * 0.5),
            Pos2::new(x, r.center().y + size * 0.5),
        ],
        Stroke::new(1.5_f32, p.ink),
    );
}

/// The slotted track, its fill, the 0 dB mark and the handle.
///
/// Extracted from [`draw_fader`] so a slot's tiny knob is the SAME control at a
/// quarter of the width rather than a second, similar-looking one: same trough,
/// same unity mark, same handle, same mapping from gain to position. A control
/// the user has already learned twenty points lower down the column.
/// How deep the channel is cut, for a row `h` tall. Shared by the channel and
/// by the cap, which has to be taller than the slot it rides in or it looks
/// like a marker printed on the panel rather than a thing sitting in it.
fn slot_height(h: f32) -> f32 {
    (h * 0.40).clamp(2.0, 14.0)
}

/// Where the cap sits on `track` at `gain`.
///
/// Its own function because the test that proves a cap never hangs off the end
/// of its own channel has to ask the same question the painter answers. A test
/// that recomputed the geometry would be a test of its own copy of it, and the
/// copy is exactly what drifts.
fn cap_rect(track: Rect, gain: f32) -> Rect {
    let h = track.height();
    // Wholly inside the channel at both ends rather than hanging half off it,
    // so the ends of the travel look like ends.
    let cw = (h * 0.30).clamp(4.0, 12.0).min(track.width());
    let x = track.left() + track.width() * gain_to_fader(gain);
    let lo = track.left() + cw * 0.5;
    let hi = (track.right() - cw * 0.5).max(lo);
    let slot_h = slot_height(h);
    Rect::from_center_size(
        Pos2::new(x.clamp(lo, hi), track.center().y),
        Vec2::new(cw, (h * 0.78).max(slot_h + 2.0)),
    )
}

/// The bone the fader caps are moulded in, and the shadow under one.
///
/// **Fixed rather than taken from the palette**, and it is the one thing in
/// this band that does not follow the background colour. A cap is a physical
/// object sitting in a slot cut into the panel: it reads as bone on walnut and
/// as bone on ivory, because it is always outlined and it always sits in a dark
/// channel. A cap that restyled itself for a light background would be a
/// painted-on rectangle, which is exactly the "strip of colour" the owner
/// rejected in the first place.
const CAP_BONE: Color32 = Color32::from_rgb(0xDC, 0xD2, 0xB6);

/// One fader, drawn like the one on a Tascam 388: a channel cut into the panel,
/// a scale of ticks either side of it, and a ribbed bone cap riding in it.
///
/// **The cap's position is the reading.** There is no coloured fill behind it
/// any more — the old track lit up from the left in the accent colour, which
/// made a fader look like a progress bar and made two faders at different
/// levels look like two different KINDS of control. A real fader says how loud
/// it is by where its cap is, and the dB figure beside it says the rest.
///
/// The scale earns its ink: with a bare slot, "a bit left of centre" is the
/// most anybody can read off a fader from two metres away, and eleven marks
/// turn that into a position you can return to.
fn draw_track(painter: &Painter, track: Rect, gain: f32, p: &Palette) {
    if !track.is_positive() {
        return;
    }
    let h = track.height();
    // The channel, cut into the panel and darker than any well in the band —
    // it is a hole, and the cap in it is the only bright thing in the row.
    let slot_h = slot_height(h);
    let slot = Rect::from_center_size(track.center(), Vec2::new(track.width(), slot_h));
    painter.rect_filled(slot, 1.0, shade(p.well, 0.55));
    painter.rect_stroke(
        slot,
        1.0,
        Stroke::new(1.0_f32, shade(p.line, 0.7)),
        StrokeKind::Inside,
    );

    // Eleven marks, above and below, and nothing at all when they would merge
    // into a grey smear — which is what they do on a knob 60 points wide.
    let arm = (h - slot_h) * 0.5;
    if track.width() >= 90.0 && arm >= 4.0 {
        // The scale is PRINTED on the panel, so it is drawn in the panel's own
        // secondary ink faded back — not in `line`, which is the colour of the
        // edges of boxes and lands within a shade of the leather it sits on.
        let stroke = Stroke::new(1.0_f32, toward(p.faint, p.bg, 0.30));
        for i in 0_u8..=10 {
            let x = track.left() + track.width() * f32::from(i) / 10.0;
            // The ends of the scale sit a hair inside, so the first and last
            // marks are marks rather than the box's own edge.
            let x = x.clamp(track.left() + 0.5, track.right() - 0.5);
            let len = if i % 5 == 0 { arm * 0.85 } else { arm * 0.5 };
            painter.line_segment(
                [
                    Pos2::new(x, slot.top() - len),
                    Pos2::new(x, slot.top() - arm * 0.15),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    Pos2::new(x, slot.bottom() + arm * 0.15),
                    Pos2::new(x, slot.bottom() + len),
                ],
                stroke,
            );
        }
    }

    // The 0 dB mark, because unity is the one position everybody looks for and
    // it is nowhere near the middle of a decibel scale. Inside the channel, so
    // it reads as a mark on the scale rather than as a twelfth tick.
    let unity = track.left() + track.width() * gain_to_fader(1.0);
    painter.line_segment(
        [
            Pos2::new(unity, slot.top() + 1.0),
            Pos2::new(unity, slot.bottom() - 1.0),
        ],
        Stroke::new(1.0_f32, p.faint),
    );

    let cap = cap_rect(track, gain);
    let cw = cap.width();
    painter.rect_filled(cap, 1.0, CAP_BONE);
    // Ribs across the travel, the way they are moulded into the real cap so a
    // thumb can feel where it is. One when there is only room for one: two
    // hairlines three points apart is a smudge, not a grip.
    let ribs: &[f32] = if cw >= 9.0 { &[0.32, 0.68] } else { &[0.5] };
    let rib = Stroke::new(1.0_f32, shade(CAP_BONE, 0.62));
    for f in ribs {
        let rx = cap.left() + cw * f;
        painter.line_segment(
            [
                Pos2::new(rx, cap.top() + cap.height() * 0.16),
                Pos2::new(rx, cap.bottom() - cap.height() * 0.16),
            ],
            rib,
        );
    }
    painter.rect_stroke(
        cap,
        1.0,
        Stroke::new(1.0_f32, shade(CAP_BONE, 0.42)),
        StrokeKind::Inside,
    );
}

/// Where the red zone starts. The last 6 dB, drawn always rather than only once
/// something has gone wrong, so the top of the scale means something before the
/// first take instead of after it.
const CLIP_ZONE: f32 = 0.501; // -6 dBFS

/// The VU face, and the ink printed on it.
///
/// Fixed, like [`CAP_BONE`] and for the same reason: a meter is an instrument
/// let into the panel, not a region of it. It is a lit cream card with black
/// printing behind glass whatever colour the panel around it is, which is what
/// makes it read as a thing rather than as a drawing.
const VU_FACE: Color32 = Color32::from_rgb(0xE3, 0xD9, 0xBD);
const VU_PRINT: Color32 = Color32::from_rgb(0x24, 0x20, 0x1A);

/// Where each printed mark sits along the arc, left to right.
///
/// **Not linear in decibels**, because a VU face is not: the bottom half of the
/// scale is squeezed into the first fifth of the sweep and the marks open out
/// towards 0. Copying the real spacing is most of what makes a drawn VU read as
/// a VU rather than as a dial with numbers on it.
///
/// `0` is aligned to -6 dBFS, which is where this band's red zone has always
/// started — so the red arc on the face means exactly what the red end of the
/// old bar meant, and a take metered to the old picture meters the same on this
/// one. See [`CLIP_ZONE`].
/// **`true` means the mark carries its NUMBER**, not merely that it is long.
///
/// The bottom of a VU scale is squeezed into the first fifth of the sweep, so
/// -20, -10 and -5 sit almost on top of each other there — printed, their
/// numbers overlapped into an unreadable smear. Three labels across the arc is
/// what a real face carries at this size: the two ends and the one number
/// anybody is actually looking for.
const VU_MARKS: [(f32, f32, bool); 9] = [
    (-20.0, 0.00, true),
    (-10.0, 0.22, false),
    (-7.0, 0.33, false),
    (-5.0, 0.42, false),
    (-3.0, 0.53, false),
    (-1.0, 0.65, false),
    (0.0, 0.72, true),
    (1.0, 0.80, false),
    (3.0, 1.00, true),
];

/// dBFS at 0 VU. The red zone's edge, unchanged from the bar meter.
const VU_ZERO_DBFS: f32 = -6.0;

/// Where a level sits along the arc, 0 at the left stop and 1 at +3.
///
/// Interpolated between the printed marks rather than computed, so the needle
/// and the scale it is read against cannot disagree — which is the one failure
/// that would make a beautiful meter lie.
fn vu_frac(level: f32) -> f32 {
    if level <= 0.0 {
        return 0.0;
    }
    let vu = 20.0 * level.log10() - VU_ZERO_DBFS;
    if vu <= VU_MARKS[0].0 {
        return 0.0;
    }
    for w in VU_MARKS.windows(2) {
        let ((a_vu, a_f, _), (b_vu, b_f, _)) = (w[0], w[1]);
        if vu <= b_vu {
            return a_f + (b_f - a_f) * (vu - a_vu) / (b_vu - a_vu);
        }
    }
    1.0
}

/// Half the needle's sweep, in radians — a shade over 90 degrees end to end,
/// which is what a real VU movement swings.
///
/// It is not a free choice: with the pivot near the bottom edge the radius can
/// be no more than the face is tall, so the sweep is the only thing left that
/// decides how much of the window the arc spans. At 35 degrees the arc covered
/// half the card and the meter read as a small dial adrift on a large blank.
const VU_SWEEP: f32 = 0.80;

/// The level meter: one analogue VU per channel, modelled on the one in a
/// Focusrite ISA One.
///
/// **A needle rather than a bar**, and the difference is not decoration. A bar
/// is read by its edge, which means reading a number off a scale you have to
/// look at; a needle is read by its ANGLE, which is a shape, and a shape is
/// something you can take in from a piano bench two metres away while your
/// hands are busy. It is also the picture every person who has stood in front
/// of a tape machine already knows how to read.
///
/// It is driven by RMS, which is what a VU is: an average-responding meter with
/// slow ballistics. Peak lives in the lamp beside the face, because peak is a
/// yes-or-no question — did anything get too close — and a needle answering two
/// questions at once answers neither.
///
/// The meter is live before arming, which is the entire point of it: "I
/// recorded silence" is a failure class that dies at the sight of a moving
/// needle. Nothing here is gated on the state.
fn draw_meter(painter: &Painter, r: Rect, m: Meters, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    let one = [m.left];
    let two = [m.left, m.right];
    let faces: &[Level] = if m.mono { &one } else { &two };
    let n = faces.len() as f32;
    let gap = (r.height() * 0.10).min(8.0);
    // **The face has an aspect, and the box it is given does not.** A VU window
    // is about half again as wide as it is tall; stretched to fill a long thin
    // cell it becomes a card with a small arc adrift in the middle of it, which
    // is the one way to make a drawn meter look like a drawing. So the face
    // takes the height it is given and only as much width as that height
    // deserves.
    //
    // And the PAIR is centred, not each face in a share of the row. A share
    // apiece is what put two 65-point meters at either end of a 600-point strip
    // while a take was rolling, which reads as two instruments on opposite
    // walls rather than as the left and right of one signal.
    let fw = ((r.width() - gap * (n - 1.0)) / n).min(r.height() * 1.35);
    if fw <= 0.0 {
        return;
    }
    let mut left = r.center().x - (fw * n + gap * (n - 1.0)) * 0.5;
    for lv in faces {
        let face = Rect::from_min_max(
            Pos2::new(left, r.top()),
            Pos2::new(left + fw, r.bottom()),
        );
        draw_vu(painter, face, *lv, m.clipped, p);
        left += fw + gap;
    }
}

/// One VU face.
fn draw_vu(painter: &Painter, face: Rect, lv: Level, clipped: bool, p: &Palette) {
    let (fw, fh) = (face.width(), face.height());
    if fw < 12.0 || fh < 10.0 {
        return;
    }
    painter.rect_filled(face, 2.0, VU_FACE);

    // The pivot sits just inside the bottom edge so its hub is visible, the way
    // it is on a real movement. The radius is whichever of the two dimensions
    // runs out first, so a wide short face and a tall narrow one both get a
    // needle that stays on the card.
    let pivot = Pos2::new(face.center().x, face.bottom() - fh * 0.13);
    let radius = (fh * 0.74).min((fw * 0.5 - 2.0) / VU_SWEEP.sin()).max(1.0);
    let at = |f: f32, rr: f32| {
        let a = (f - 0.5) * 2.0 * VU_SWEEP;
        Pos2::new(pivot.x + rr * a.sin(), pivot.y - rr * a.cos())
    };

    // The arc, with its last stretch in red. Two polylines rather than one, so
    // the red is the SCALE going red and not a band drawn over it.
    let arc = |from: f32, to: f32, colour: Color32, w: f32| {
        let pts: Vec<Pos2> = (0_u8..=16)
            .map(|k| at(from + (to - from) * f32::from(k) / 16.0, radius))
            .collect();
        painter.add(egui::Shape::line(pts, Stroke::new(w, colour)));
    };
    let hair = (fh * 0.022).max(1.0);
    let red_from = VU_MARKS[6].1;
    arc(0.0, red_from, VU_PRINT, hair);
    arc(red_from, 1.0, p.rec, hair * 1.6);

    // The printed marks. The majors are longer and, when there is room for
    // them, carry their numbers.
    let label = fh >= 46.0 && fw >= 90.0;
    for (vu, f, major) in VU_MARKS {
        // The five-in-ten marks stay long whether or not they are numbered:
        // the scale is read by its ticks and labelled at three points.
        let long = major || (vu as i32) % 5 == 0;
        let inner = radius * if long { 0.84 } else { 0.90 };
        let colour = if f >= red_from { p.rec } else { VU_PRINT };
        painter.line_segment([at(f, inner), at(f, radius)], Stroke::new(hair, colour));
        if label && major {
            let text = if vu > 0.0 {
                format!("+{vu:.0}")
            } else {
                format!("{vu:.0}")
            };
            painter.text(
                at(f, radius * 0.70),
                Align2::CENTER_CENTER,
                &text,
                font(fh * 0.15),
                colour,
            );
        }
    }
    if label {
        painter.text(
            Pos2::new(pivot.x, pivot.y - radius * 0.30),
            Align2::CENTER_CENTER,
            "VU",
            font_light(fh * 0.16),
            VU_PRINT,
        );
    }

    // The needle. RMS, and clamped to the stops rather than allowed off the
    // card: a needle that leaves the face is a rendering fault, and the lamp is
    // what says the signal went past the end.
    let n = at(vu_frac(lv.rms).clamp(0.0, 1.0), radius * 0.97);
    painter.line_segment([pivot, n], Stroke::new((fh * 0.035).max(1.0), VU_PRINT));
    painter.circle_filled(pivot, (fh * 0.075).max(1.5), VU_PRINT);

    // The peak lamp: a yes-or-no answer to a yes-or-no question. Lit while the
    // held peak is in the last six decibels, and LATCHED red once anything has
    // actually clipped, because the person it is for was looking at their hands
    // when it happened.
    let d = (fh * 0.12).clamp(3.0, 8.0);
    let hot = clipped || lv.hold >= CLIP_ZONE;
    // Bottom right, where the card is empty. Top right is where the arc's own
    // last inch is, and a lamp there sits ON the red the lamp is about.
    let at_lamp = Pos2::new(face.right() - d * 1.5, face.bottom() - d * 1.5);
    painter.circle_filled(
        at_lamp,
        d * 0.5,
        if hot {
            p.rec
        } else {
            toward(p.rec, VU_FACE, 0.86)
        },
    );
    // A ring, so an unlit lamp reads as a lamp that is not lit rather than as a
    // mark on the card.
    painter.circle_stroke(at_lamp, d * 0.5, Stroke::new(1.0_f32, shade(VU_FACE, 0.45)));

    painter.rect_stroke(
        face,
        2.0,
        Stroke::new(1.0_f32, if clipped { p.rec } else { shade(VU_FACE, 0.35) }),
        StrokeKind::Inside,
    );
}

/// What the big readout says.
///
/// Its own function so that the one thing about it that is easy to get wrong —
/// what a count-in shows — can be asserted without a screen.
///
/// During a count-in it is the BEAT, because a count-in is a musical
/// instruction and the number the player needs is the one the click is playing.
/// A countdown in seconds against a click in beats is two clocks disagreeing in
/// front of the person trying to come in on time.
fn readout_text(state: RecordState, elapsed_s: f64) -> String {
    match state {
        // Just the number. It said "3 OF 12" when the count was a running
        // total, and "3 OF 6" once it became a beat within the bar — at which
        // point the second half stopped carrying anything: the bar's length is
        // already on screen in the SIG cell, and nobody counting a band in has
        // ever said "of six". What the player needs is the digit they are
        // saying out loud, as large as the box will draw it.
        RecordState::CountIn { beat, .. } => beat.to_string(),
        RecordState::Finishing => "FINISHING".to_owned(),
        RecordState::Rolling | RecordState::Idle => timecode(elapsed_s),
    }
}

fn draw_readout(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    let r = l.timecode;
    if !r.is_positive() {
        return;
    }
    let text = readout_text(view.state, view.elapsed_s);
    let colour = match view.state {
        RecordState::CountIn { .. } => p.rec,
        RecordState::Rolling | RecordState::Finishing => p.ink,
        RecordState::Idle => p.faint,
    };
    let size = fit_text(r, &text, r.height() * (if l.rolling { 0.92 } else { 0.8 }));
    if size < MIN_TEXT {
        return;
    }
    let (at, align) = if l.rolling {
        (r.center(), Align2::CENTER_CENTER)
    } else {
        (Pos2::new(r.left(), r.center().y), Align2::LEFT_CENTER)
    };
    painter.text(at, align, &text, font(size), colour);
}

// ── the destination ────────────────────────────────────────────────────────

/// The suffix on a device that is named but absent.
///
/// `Missing` is neither `None` nor `Open` and must not read as either: it means
/// the user already chose this thing and it is not here right now, which is a
/// thing to go and fix rather than a thing to go and set up. The wording is per
/// device because a camera is unplugged and a plugin fails to load, and telling
/// a user their instrument is "not connected" sends them looking for a cable —
/// which is why the slot rows spell out "did not load" themselves.
fn device_note(d: DeviceLabel<'_>, missing: &'static str) -> &'static str {
    match d {
        DeviceLabel::Missing(_) => missing,
        DeviceLabel::None | DeviceLabel::Open(_) => "",
    }
}

fn device_ink(d: DeviceLabel<'_>, p: &Palette) -> Color32 {
    match d {
        DeviceLabel::None => p.faint,
        DeviceLabel::Open(_) => p.ink,
        DeviceLabel::Missing(_) => p.warn,
    }
}

/// What a take will actually produce, in four words.
///
/// Drawn as the Export button's own value so the dialog is somewhere you go to
/// CHANGE the answer rather than to find out what it is.
fn export_summary(spec: &ExportSpec) -> String {
    let mut parts: Vec<String> = Vec::new();
    if spec.audio {
        parts.push("wav".to_owned());
    }
    if spec.midi {
        parts.push("midi".to_owned());
    }
    match spec.encoder_count() {
        0 => {}
        1 => parts.push("1 video".to_owned()),
        n => parts.push(format!("{n} videos")),
    }
    if parts.is_empty() {
        // `ExportSpec::problem` refuses this at the moment of recording, but
        // the band still has to say what it is looking at.
        return "nothing".to_owned();
    }
    parts.join(" + ")
}

/// The count-in, in the fewest words that are still true.
///
/// Beats rather than bars, matching the menu: "8 beats" is what the click
/// plays, and the app has no time signature to turn that into bars with.
/// The count-in, in BARS — which is what a click cycles and what the menu
/// offers. It read beats, and beats stopped being the setting when the time
/// signature arrived: the cell showed a number the control no longer changed.
fn count_in_text(bars: u32) -> String {
    match bars {
        0 => "off".to_owned(),
        1 => "1 bar".to_owned(),
        n => format!("{n} bars"),
    }
}

/// A tempo without a trailing `.0` on the whole numbers, which is nearly all of
/// them, and without losing the half that 92.5 has.
fn tempo_text(bpm: f64) -> String {
    if (bpm - bpm.round()).abs() < 0.05 {
        format!("{:.0}", bpm.round())
    } else {
        format!("{bpm:.1}")
    }
}

fn draw_destination(
    painter: &Painter,
    l: &Layout,
    view: &RecorderView<'_>,
    s: &Settings,
    p: &Palette,
) {
    labelled(painter, l.dest, "FOLDER", view.dest, p.ink, p);
    // The "Choose..." affordance lives INSIDE the folder box, right-aligned,
    // because the box itself is the target: two adjacent controls that both
    // open the same picker is one control too many.
    if l.dest.is_positive() {
        let size = fit_text(l.dest, "Choose...", l.dest.height() * 0.45);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(l.dest.right() - label_inset(l.dest), l.dest.center().y),
                Align2::RIGHT_CENTER,
                "Choose...",
                font_light(size),
                p.faint,
            );
        }
    }
    // A button, in the same shape as the slots' OPEN WINDOW: a filled box with
    // a border and a centred word, which in this band is what a pressable thing
    // looks like and nothing else is. Every other box in this column carries a
    // caption and a value, and this one has no value to carry.
    //
    // It says SHOW rather than OPEN for the reason the tick beside Export does:
    // nothing is opened, a folder is shown.
    draw_word_button(painter, l.reveal, &["SHOW FOLDER", "SHOW"], p);
    draw_tick(
        painter,
        l.default_tick,
        "Default",
        s.record_dir_is_default,
        p,
    );

    // The take name is not required and does not have to be unique: the
    // timestamp guarantees that. Type "nocturne" once, press record five
    // times, and get five adjacent folders with no overwrite dialog ever.
    let empty = view.take_name.is_empty();
    let shown = if empty { "(optional)" } else { view.take_name };
    labelled(
        painter,
        l.name,
        "NAME",
        shown,
        if empty { p.faint } else { p.ink },
        p,
    );
    if view.name_focused {
        draw_caption_caret(
            painter,
            l.name,
            "NAME",
            shown,
            view.take_name.chars().count(),
            p,
        );
    }
    text_line(painter, l.folder, view.folder_preview, p.faint, false);

    // Disk as a DURATION. "214 GB free" means nothing to a pianist and "~58
    // min" means everything, which is why the view carries minutes and not
    // bytes in the first place.
    let disk = match view.disk_minutes {
        Some(m) => format!("{} left", disk_text(m)),
        None => "measuring free space".to_owned(),
    };
    text_line(painter, l.disk, &disk, p.faint, true);

    // The instruments are NOT here any more. They have three rows of their own
    // in the group that survives a take; a fourth picker in the column that
    // vanishes at `T0` would have been the same control in two places, only one
    // of which works.
    for (r, cap, dev, missing) in [
        (l.camera, "CAMERA", view.camera, "  (not connected)"),
        (l.audio, "AUDIO", view.audio, "  (not connected)"),
    ] {
        let value = format!("{}{}", dev.text(), device_note(dev, missing));
        labelled(painter, r, cap, &value, device_ink(dev, p), p);
    }

    labelled(
        painter,
        l.count_in,
        "COUNT-IN",
        &count_in_text(view.count_in_bars),
        p.ink,
        p,
    );
    // The signature, typed rather than dragged. `labelled` with a caret when it
    // is being edited, exactly like the take name.
    {
        let typing = typing_for(view, NumField::Meter);
        let shown = typing.map_or_else(|| view.time_signature.label(), str::to_owned);
        labelled(painter, l.time_sig, "SIG", &shown, p.ink, p);
        if let Some(typed) = typing {
            draw_caption_caret(painter, l.time_sig, "SIG", &shown, typed.chars().count(), p);
        }
    }
    draw_tempo(
        painter,
        l.tempo,
        view.tempo_bpm,
        typing_for(view, NumField::Tempo),
        p,
    );
    labelled(
        painter,
        l.export,
        "EXPORT",
        &export_summary(&s.record_export),
        p.ink,
        p,
    );
    // "Show when done" and not "Open": the folder is shown in the file manager,
    // nothing is opened, and somebody who reads "Open" and expects the take to
    // start playing has been told the wrong thing.
    draw_tick(
        painter,
        l.open_when_done,
        "Show when done",
        s.record_open_when_done,
        p,
    );
}

/// The tempo box, with a hairline of travel along its bottom.
///
/// It is the only thing in this column that is dragged rather than clicked, and
/// nothing else in the band is shaped like it, so it has to say so. The bar is
/// where in `MIN_BPM..=MAX_BPM` the number sits — the same mapping [`tempo_at`]
/// inverts, which is what makes the picture and the drag agree.
fn draw_tempo(painter: &Painter, r: Rect, bpm: f64, typing: Option<&str>, p: &Palette) {
    // While it is being typed into, the box shows the characters rather than
    // the stored tempo — otherwise there is nothing on screen to tell somebody
    // their keystrokes are going anywhere.
    let shown = typing.map_or_else(|| tempo_text(bpm), str::to_owned);
    labelled(painter, r, "TEMPO", &shown, p.ink, p);
    if let Some(typed) = typing {
        draw_caption_caret(painter, r, "TEMPO", &shown, typed.chars().count(), p);
    }
    if !r.is_positive() {
        return;
    }
    let t = ((bpm - MIN_BPM) / (MAX_BPM - MIN_BPM)).clamp(0.0, 1.0) as f32;
    let h = (r.height() * 0.14).max(1.0);
    let bar = Rect::from_min_max(
        Pos2::new(r.left(), r.bottom() - h),
        Pos2::new(r.left() + r.width() * t, r.bottom()),
    );
    if bar.is_positive() {
        painter.rect_filled(bar, 1.0, p.accent);
    }
}

/// A tick box and its caption: two line segments rather than a glyph, because
/// no bundled face is guaranteed to carry a check mark and a tofu box in a
/// checkbox is indistinguishable from a ticked one.
///
/// The box is bounded by the WIDTH as well as the height. In a tall narrow slot
/// — a detached window's click switch — a box sized off the height alone eats
/// the whole control and the caption disappears.
fn draw_tick(painter: &Painter, r: Rect, cap: &str, on: bool, p: &Palette) {
    control(painter, r, p);
    if !r.is_positive() {
        return;
    }
    let inset = (r.height() * 0.25).min(r.width() * 0.08);
    let side = (r.height() - inset * 2.0).min(r.width() * 0.30);
    if side <= 0.0 {
        return;
    }
    let bx = Rect::from_min_size(
        Pos2::new(r.left() + inset, r.center().y - side * 0.5),
        Vec2::splat(side),
    );
    painter.rect_stroke(bx, 1.0, Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
    if on {
        let s = Stroke::new((side * 0.16).max(1.0), p.ink);
        painter.line_segment(
            [
                Pos2::new(bx.left() + side * 0.22, bx.center().y),
                Pos2::new(bx.center().x - side * 0.02, bx.bottom() - side * 0.24),
            ],
            s,
        );
        painter.line_segment(
            [
                Pos2::new(bx.center().x - side * 0.02, bx.bottom() - side * 0.24),
                Pos2::new(bx.right() - side * 0.18, bx.top() + side * 0.24),
            ],
            s,
        );
    }
    let text = Rect::from_min_max(Pos2::new(bx.right() + inset, r.top()), r.max);
    let size = fit_text(text, cap, r.height() * 0.5);
    if size >= MIN_TEXT {
        painter.text(
            Pos2::new(text.left(), text.center().y),
            Align2::LEFT_CENTER,
            cap,
            font_light(size),
            p.ink,
        );
    }
}

/// A line of unboxed text in a row, left- or right-aligned.
fn text_line(painter: &Painter, r: Rect, text: &str, colour: Color32, right: bool) {
    if !r.is_positive() || text.is_empty() {
        return;
    }
    let size = fit_text(r, text, r.height() * 0.82);
    if size < MIN_TEXT {
        return;
    }
    let (at, align) = if right {
        (Pos2::new(r.right(), r.center().y), Align2::RIGHT_CENTER)
    } else {
        (Pos2::new(r.left(), r.center().y), Align2::LEFT_CENTER)
    };
    painter.text(at, align, text, font_light(size), colour);
}

fn draw_status(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    if let Some(m) = view.message {
        text_line(painter, l.status, m, p.ink, false);
    }
    // Latched, so it is still there after Stop. An indicator that clears itself
    // is one the performer never sees, because they were looking at their hands
    // when it happened.
    if view.meters.clipped || view.clip_warning {
        text_line(painter, l.clip, "CLIPPED", p.rec, true);
    }
}

/// One-pixel top edge so the band reads as its own, matching the fretboard's.
/// Drawn by the caller, which knows what is above it.
pub fn draw_top_edge(painter: &Painter, rect: Rect, s: &Settings) {
    let c = if s.dark_mode {
        Color32::from_rgb(60, 60, 60)
    } else {
        Color32::from_rgb(120, 100, 78)
    };
    painter.rect_stroke(
        Rect::from_min_size(rect.min, Vec2::new(rect.width(), 1.0)),
        0.0,
        Stroke::new(1.0_f32, c),
        StrokeKind::Inside,
    );
}

// ── the popped-out window ──────────────────────────────────────────────────
//
// The fourth detachable surface, and deliberately the same shape of code as the
// other three (`chord_strip`, `fretboard_panel`, `theory_panel`): same
// close-to-reattach, same right-click-anywhere menu, same borderless
// drag-anywhere, same window level. Four popouts with four sets of habits would
// be worse than none.
//
// Like the theory window and unlike the other two, this one is an INPUT: a
// click starts a take. Everything below that differs from the chord strip — the
// hit in the outcome, the drag that stands aside for it, the ctrl-click guard —
// exists for that one reason.

/// Default size for the popped-out recorder when nothing is remembered.
///
/// Not the band's proportions, for the same reason the neck's popout is not
/// (D-UI-10): in its own window it should be legible rather than a slice of the
/// main one. §5's reason for detaching at all is a big framing view on a second
/// monitor, and the preview claims about a quarter of the width, so this is
/// about as small as the window gets while the destination column still holds a
/// device name at a readable size.
pub const DETACHED_DEFAULT: Vec2 = Vec2::new(720.0, 400.0);

/// The smallest window the band still works in.
///
/// **Raised from 480x270 when the monitor column arrived**, and unchanged when
/// the three instrument slots moved into it. The binding constraint is the same
/// as it always was — the destination column, not the preview. At 640x340 that
/// column is ~170pt across with seven rows of ~36pt, which holds
/// `EXPORT wav + midi` and `AUDIO Scarlett 2i2 USB` at about 11pt: it lost width
/// to the slots and got it back in row height, because the slots also took its
/// INSTRUMENT row away. In the same window a slot row is ~39pt tall and ~220pt
/// across, which is `1 Pianoteq 8` at 9pt, a knob, a dB reading at 6.4pt, an
/// `OPEN` button at 17pt and a cross. Below that the column is a stack of boxes with smudges in
/// them, which reads as a rendering fault rather than as "too small" — the same
/// failure `theory_panel::DETACHED_MIN` puts a floor under.
pub const DETACHED_MIN: Vec2 = Vec2::new(640.0, 340.0);

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("ivory-recorder-window")
}

/// Outline for the popped-out window. The band's background is near-black in
/// dark mode and pale grey in light, so neither theme's fill separates the
/// window from the desktop behind it. One neutral grey reads against both,
/// exactly as it does for the other three popouts.
pub const BORDER_COLOR: Color32 = Color32::from_gray(0x5A);

/// What the app has to act on after showing the window.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct DetachedOutcome {
    /// The user closed the window. Close-to-reattach: the band comes back.
    pub close_requested: bool,
    /// Live inner size in points, recorded every frame. Feed it to
    /// `theory_panel::GeometryGuard::observe` rather than to settings directly.
    pub inner_size: Option<Vec2>,
    /// Live outer position in monitor coordinates, recorded every frame.
    pub outer_pos: Option<Pos2>,
    /// Right-click (or ctrl-click on macOS) happened at this monitor-space
    /// position, for the app context menu.
    pub context_menu_at: Option<Pos2>,
    /// A click landed on a control. Feed it to the same handler the band's
    /// clicks go to.
    ///
    /// Only the PRESS is reported here. A fader that is being dragged has to be
    /// re-hit-tested by the app while the button is held — see [`hit_test`].
    pub hit: Option<Hit>,
}

/// The recorder in its own window.
///
/// `builder_size` and `builder_pos` must stay constant for the lifetime of one
/// detachment, exactly as for the other three windows: egui diffs this builder
/// against the previous frame's builder rather than against the window's real
/// geometry, so a value that changed every frame would fight the user's own
/// resizes and drags.
///
/// `main_focused` decides the window LEVEL. A detached window is a piece of the
/// same app, so it rises and falls WITH the piano rather than being left
/// wherever the window stack last put it. By level rather than by raising:
/// always-on-top while we are frontmost is exactly "above our own window", and
/// dropping to Normal when we are not means it never floats over other
/// applications.
pub fn show_detached_window(
    ctx: &egui::Context,
    builder_size: Vec2,
    builder_pos: Option<Pos2>,
    borderless: bool,
    main_focused: bool,
    view: &RecorderView<'_>,
    s: &Settings,
) -> DetachedOutcome {
    let mut outcome = DetachedOutcome::default();
    let mut builder = egui::ViewportBuilder::default()
        .with_title("Tangent")
        .with_inner_size(builder_size)
        .with_min_inner_size(DETACHED_MIN)
        .with_resizable(true)
        .with_decorations(!borderless)
        .with_window_level(if main_focused {
            egui::viewport::WindowLevel::AlwaysOnTop
        } else {
            egui::viewport::WindowLevel::Normal
        });
    if let Some(pos) = builder_pos {
        builder = builder.with_position(pos);
    }

    ctx.show_viewport_immediate(viewport_id(), builder, |vp, _class| {
        crate::shell::viewport_ui(vp, |ui| {
            // THE WINDOW'S OWN RECT, used for both the drawing and the hit test
            // below. The band's rect is a completely different rectangle —
            // different width, different height, different origin — and a hit
            // test run against it would return whatever is at that point IN THE
            // MAIN WINDOW, which here means pressing Record because the pointer
            // happened to be where the record button is somewhere else. Nothing
            // in this closure may reach for a band rect, which is why none is
            // passed in.
            let rect = ui.max_rect();
            draw(ui.painter(), rect, view, s);
            painter_border(ui.painter(), rect);

            let (close, inner_rect, outer_rect, pressed, secondary, pointer, ctrl) =
                ui.input(|i| {
                    (
                        i.viewport().close_requested(),
                        i.viewport().inner_rect,
                        i.viewport().outer_rect,
                        i.pointer.primary_pressed(),
                        i.pointer.secondary_clicked(),
                        i.pointer.interact_pos(),
                        i.modifiers.ctrl,
                    )
                });

            outcome.close_requested = close;
            // The fallback matters in a host that reports no window geometry at
            // all: without it the guard sees `None` forever and the window is
            // never remembered, rather than never poisoned.
            outcome.inner_size = inner_rect.map(|r| r.size()).or(Some(rect.size()));
            outcome.outer_pos = outer_rect.map(|r| r.min);

            // Ctrl-click IS the right-click on macOS, and this window has
            // twenty-six click targets. Without the guard, ctrl-clicking to
            // open the menu over the transport starts a take at the same time.
            let ctrl_as_context = cfg!(target_os = "macos") && ctrl;
            let menu = secondary || (pressed && ctrl_as_context);
            if menu {
                if let (Some(pos), Some(inner)) = (pointer, inner_rect) {
                    outcome.context_menu_at = Some(inner.min + pos.to_vec2());
                }
            }

            if pressed && !menu {
                if let Some(p) = pointer {
                    outcome.hit = hit_test(rect, view, p);
                }
            }

            // Borderless drag-anywhere, minus the parts that are not
            // "anywhere". With no title bar the whole window is the drag
            // handle, so an unguarded StartDrag means pressing Record picks the
            // window up and carries it off instead of starting the take. The
            // press only becomes a drag when it hit nothing.
            if borderless && pressed && !menu && outcome.hit.is_none() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
        });
    });
    outcome
}

fn painter_border(painter: &Painter, rect: Rect) {
    painter.rect_stroke(
        rect.shrink(0.5),
        0.0,
        Stroke::new(1.0_f32, BORDER_COLOR),
        StrokeKind::Middle,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recorder::{fader_to_gain, Gains, Preview};

    fn band(w: f32) -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 350.0), Vec2::new(w, band_height(w)))
    }

    /// An empty rack, which is what the app opens with.
    fn idle() -> RecorderView<'static> {
        RecorderView::empty()
    }

    fn rolling() -> RecorderView<'static> {
        RecorderView {
            state: RecordState::Rolling,
            elapsed_s: 252.0,
            ..RecorderView::empty()
        }
    }

    fn loaded(name: &'static str, gain: f32, has_editor: bool) -> SlotView<'static> {
        SlotView {
            name: Some(name),
            missing: false,
            gain,
            has_editor,
            editor_open: false,
        }
    }

    /// Three instruments loaded, every one of them with an editor.
    ///
    /// The state in which a slot row has all four of its controls, and therefore
    /// the state most of the reachability tests have to be in: an EMPTY slot
    /// deliberately has no knob, no editor button and nothing to clear, because
    /// there is no level to set on a slot with nothing in it.
    fn furnished() -> RecorderView<'static> {
        RecorderView {
            slots: [
                loaded("Pianoteq 8", 1.0, true),
                loaded("Kontakt 7", 0.5, true),
                loaded("Dexed", 0.0, true),
                loaded("sfizz", 0.7, true),
                loaded("Surge XT", 0.3, true),
            ],
            ..RecorderView::empty()
        }
    }

    fn furnished_rolling() -> RecorderView<'static> {
        RecorderView {
            state: RecordState::Rolling,
            elapsed_s: 252.0,
            ..furnished()
        }
    }

    /// Every shape a rack can be in that changes the shape of a row.
    ///
    /// Empty, full, a plugin with no editor at all (legal VST3), and one named
    /// in settings that would not load. The mixed ones matter: the three rows
    /// are laid out independently, and "slot 1 is empty between two full ones"
    /// is exactly the arrangement a naive layout collapses.
    fn racks() -> [[SlotView<'static>; SLOTS]; 4] {
        [
            [SlotView::EMPTY; SLOTS],
            [
                loaded("Pianoteq 8", 1.0, true),
                loaded("Kontakt 7", 0.5, true),
                loaded("Dexed", 0.0, true),
                loaded("sfizz", 0.7, true),
                loaded("Surge XT", 0.3, true),
            ],
            [
                loaded("sfizz", 1.0, false),
                SlotView::EMPTY,
                loaded("Surge XT", 0.25, false),
                SlotView::EMPTY,
                SlotView::EMPTY,
            ],
            [
                SlotView {
                    name: Some("Pianoteq 8"),
                    missing: true,
                    ..SlotView::EMPTY
                },
                SlotView::EMPTY,
                // Its window is already on screen, which the button says.
                SlotView {
                    editor_open: true,
                    ..loaded("Dexed", 3.98, true)
                },
                SlotView::EMPTY,
                loaded("Surge XT", 0.5, true),
            ],
        ]
    }

    fn with_rack(state: RecordState, slots: [SlotView<'static>; SLOTS]) -> RecorderView<'static> {
        RecorderView {
            state,
            slots,
            ..RecorderView::empty()
        }
    }

    /// Every state a take passes through, which several tests have to sweep.
    const STATES: [RecordState; 4] = [
        RecordState::Idle,
        RecordState::CountIn { beat: 3, of: 4 },
        RecordState::Rolling,
        RecordState::Finishing,
    ];

    /// The controls that stay reachable once a take is live, with a full rack.
    ///
    /// The three editor buttons are in the list and the three PICKERS are not,
    /// which is the whole distinction: reaching a preset in a plugin's own
    /// window between two passes is a real thing, and loading a plugin blocks
    /// the main thread for seconds and would cost the take.
    const SURVIVORS: [Hit; 14] = [
        Hit::Stop,
        Hit::SetSlotGain(0, 0.0),
        Hit::SetSlotGain(1, 0.0),
        Hit::SetSlotGain(2, 0.0),
        Hit::SetSlotGain(3, 0.0),
        Hit::SetSlotGain(4, 0.0),
        Hit::OpenSlotEditor(0),
        Hit::OpenSlotEditor(1),
        Hit::OpenSlotEditor(2),
        Hit::OpenSlotEditor(3),
        Hit::OpenSlotEditor(4),
        Hit::SetMetronomeGain(0.0),
        Hit::SetInputGain(0.0),
        Hit::ToggleMetronome,
    ];

    /// The whole reason `band_height` takes one argument. A camera that could
    /// change the band's height would resize the piano when somebody plugged in
    /// a different webcam — `docs/RECORDER-PLAN.md` §0's named failure, and the
    /// one thing about this panel that cannot be fixed later without breaking
    /// every geometry test in `app.rs`.
    #[test]
    fn a_four_three_camera_does_not_make_the_band_taller() {
        for w in [400.0_f32, 650.0, 1300.0, 2600.0] {
            let want = band_height(w);
            let r = band(w);
            assert_eq!(r.height(), want);
            for src in [
                Vec2::new(640.0, 480.0),   // 4:3
                Vec2::new(1920.0, 1080.0), // 16:9
                Vec2::new(1080.0, 1920.0), // a phone held upright
                Vec2::new(1000.0, 1000.0),
            ] {
                let l = Layout::new(r, &idle());
                let dst = fit_preview(l.preview, src);
                assert!(
                    l.preview.contains_rect(dst.shrink(0.01)),
                    "a {src:?} source escaped its box at {w}pt"
                );
                assert!(
                    r.contains_rect(l.preview),
                    "the preview box escaped the band at {w}pt"
                );
                // And the height the layout was given is still the height the
                // layout function alone decided.
                assert_eq!(band_height(w), want);
            }
        }
    }

    /// State and the presence of a frame are equally powerless over the height.
    #[test]
    fn the_band_height_is_a_function_of_width_and_nothing_else() {
        assert_eq!(band_height(1300.0), 165.0);
        assert_eq!(band_height(650.0), 82.0);
        // Truncated, not rounded, like every other band: 165 * 1000/1300 is
        // 126.9, and a half-pixel band puts every row on a fractional line.
        assert_eq!(band_height(1000.0), 126.0);
        assert_eq!(band_height(0.0), 0.0);

        let pv = Preview {
            texture: egui::TextureId::User(1),
            size: Vec2::new(640.0, 480.0),
        };
        for w in [400.0_f32, 1300.0] {
            let want = band_height(w);
            for state in STATES {
                for preview in [None, Some(pv)] {
                    let v = RecorderView {
                        preview,
                        ..with_rack(state, racks()[1])
                    };
                    // Nothing in the view can reach the height: the layout is
                    // handed a rect the height already decided.
                    let l = Layout::new(band(w), &v);
                    assert_eq!(band_height(w), want, "{state:?} moved the band height");
                    for (r, k) in l.targets() {
                        assert!(
                            !r.is_positive() || band(w).contains_rect(r),
                            "{} escaped the band at {w}pt in {state:?}",
                            k.hit(0.0).label()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn fit_preview_letterboxes_whatever_shape_the_camera_is() {
        let box_ = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(320.0, 180.0));
        // Wider than the box: full width, bars top and bottom.
        let wide = fit_preview(box_, Vec2::new(1000.0, 250.0));
        assert!((wide.width() - 320.0).abs() < 0.01);
        assert!(wide.height() < box_.height());
        // Taller than the box: full height, bars left and right.
        let tall = fit_preview(box_, Vec2::new(480.0, 640.0));
        assert!((tall.height() - 180.0).abs() < 0.01);
        assert!(tall.width() < box_.width());
        // Exactly the box's aspect: it fills, and it is not a rounding error
        // away from filling.
        let exact = fit_preview(box_, Vec2::new(1920.0, 1080.0));
        assert!((exact.width() - 320.0).abs() < 0.01, "{exact:?}");
        assert!((exact.height() - 180.0).abs() < 0.01, "{exact:?}");

        for (src, got) in [
            (Vec2::new(1000.0, 250.0), wide),
            (Vec2::new(480.0, 640.0), tall),
            (Vec2::new(1920.0, 1080.0), exact),
        ] {
            assert!(
                box_.contains_rect(got.shrink(0.01)),
                "{src:?} left the box: {got:?}"
            );
            assert!(
                (got.center() - box_.center()).length() < 0.01,
                "{src:?} was not centred"
            );
            assert!(
                ((got.width() / got.height()) - (src.x / src.y)).abs() < 0.001,
                "{src:?} was stretched to {got:?}"
            );
        }
    }

    /// A camera that has not reported a size yet, and a band that has been
    /// dragged to nothing. Both are reachable and neither may divide by zero.
    #[test]
    fn fit_preview_survives_a_zero_sized_source_or_box() {
        let box_ = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 180.0));
        for src in [
            Vec2::ZERO,
            Vec2::new(0.0, 480.0),
            Vec2::new(640.0, 0.0),
            Vec2::new(-4.0, 3.0),
        ] {
            let r = fit_preview(box_, src);
            assert!(!r.is_positive(), "{src:?} produced something to draw");
        }
        for b in [
            Rect::from_min_size(Pos2::ZERO, Vec2::ZERO),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(100.0, 0.0)),
            Rect::NOTHING,
        ] {
            let r = fit_preview(b, Vec2::new(1920.0, 1080.0));
            assert!(!r.is_positive());
            assert!(!r.min.x.is_nan() && !r.min.y.is_nan(), "NaN escaped");
        }
    }

    /// Two controls that overlap means one of them silently swallows the
    /// other's clicks, and nothing about the picture says so.
    ///
    /// Swept over every rack as well as every record state, because a slot row's
    /// insides change shape with what is in it: an empty slot's name box takes
    /// the whole row, and a row whose neighbour grew into it is precisely the
    /// bug this catches.
    /// **The two transport buttons are the same size.**
    ///
    /// Stop used to be 0.66 of record and then had its glyph shrunk another
    /// 30% inside that, so the square read as less than half the circle — which
    /// makes one look like the real control and the other like a note about it.
    /// **The band stays readable whatever colour it is given.**
    ///
    /// The ink follows the BACKGROUND, not `dark_mode`. A rule that read the
    /// theme would put near-black text on a dark walnut the moment somebody
    /// picked one in light mode, and the band would be unreadable with nothing
    /// on screen explaining why.
    #[test]
    fn the_ink_follows_the_background_and_not_the_theme() {
        use crate::settings::Rgb;
        // A dark colour chosen while the app is in LIGHT mode.
        let mut s = Settings {
            dark_mode: false,
            ..Settings::default()
        };
        s.recorder_bg_color = Rgb {
            r: 0x2A,
            g: 0x20,
            b: 0x18,
        };
        let p = palette(&s);
        assert!(
            contrast_ratio(p.bg, p.ink) > 4.0,
            "dark band, light theme: ink {:?} on {:?} is unreadable",
            p.ink,
            p.bg
        );

        // And a light colour chosen while the app is in DARK mode.
        let mut s = Settings {
            dark_mode: true,
            ..Settings::default()
        };
        s.recorder_bg_color = Rgb {
            r: 0xE8,
            g: 0xDC,
            b: 0xC0,
        };
        let p = palette(&s);
        assert!(
            contrast_ratio(p.bg, p.ink) > 4.0,
            "light band, dark theme: ink {:?} on {:?} is unreadable",
            p.ink,
            p.bg
        );
    }

    /// Every colour a user could pick leaves legible ink, and a well that is
    /// still the same hue as the band it is cut into.
    #[test]
    fn any_background_gets_readable_ink_and_a_well_of_its_own_hue() {
        use crate::settings::Rgb;
        for (r, g, b) in [
            (0x00, 0x00, 0x00),
            (0xFF, 0xFF, 0xFF),
            (0x4A, 0x3B, 0x2C), // the default walnut
            (0x8B, 0x00, 0x00), // saturated red
            (0x00, 0x00, 0x8B), // saturated blue: dark despite a high channel
            (0x00, 0xFF, 0x00), // and green, which is bright despite one channel
            (0x80, 0x80, 0x80),
        ] {
            let mut s = Settings::default();
            s.recorder_bg_color = Rgb { r, g, b };
            let p = palette(&s);
            assert!(
                contrast_ratio(p.bg, p.ink) > 3.5,
                "#{r:02X}{g:02X}{b:02X}: ink {:?} on {:?}",
                p.ink,
                p.bg
            );
            // The well is a shade of the band, not a grey hole in it: the
            // channel ORDER survives.
            let order = |c: Color32| {
                let mut v = [(c.r(), 0u8), (c.g(), 1), (c.b(), 2)];
                v.sort();
                [v[0].1, v[1].1, v[2].1]
            };
            assert_eq!(
                order(p.well),
                order(p.bg),
                "#{r:02X}{g:02X}{b:02X}: the well lost the band's hue"
            );
        }
    }

    /// **And the same size DURING a take, too.**
    ///
    /// The rolling layout draws a dot where the record button stood and the
    /// stop button beside it. The dot was 0.30 of the row against stop's 1.0,
    /// so it read as a speck next to a slab — two controls of vastly different
    /// size, which is what they are not: one is the state and the other is the
    /// way out of it.
    #[test]
    fn the_rolling_transport_is_the_same_size_as_itself() {
        for w in [320.0_f32, 640.0, 1300.0, 2600.0] {
            let r = band(w);
            let v = RecorderView {
                state: RecordState::Rolling,
                ..idle()
            };
            let l = Layout::new(r, &v);
            if !l.dot.is_positive() || !l.stop.is_positive() {
                continue;
            }
            assert!(
                (l.dot.width() - l.stop.width()).abs() < 0.51,
                "at {w}: the dot is {} wide and stop is {}",
                l.dot.width(),
                l.stop.width()
            );
            assert!(!l.dot.intersects(l.stop), "at {w}: they overlap");
        }
    }

    #[test]
    fn record_and_stop_are_the_same_size() {
        for w in [320.0_f32, 640.0, 1300.0, 2600.0] {
            let r = band(w);
            let l = Layout::new(r, &idle());
            if !l.record.is_positive() || !l.stop.is_positive() {
                continue;
            }
            assert!(
                (l.record.width() - l.stop.width()).abs() < 0.51,
                "at {w}: record is {} wide and stop is {}",
                l.record.width(),
                l.stop.width()
            );
            assert!(
                (l.record.height() - l.stop.height()).abs() < 0.51,
                "at {w}: record is {} tall and stop is {}",
                l.record.height(),
                l.stop.height()
            );
            // And they still do not touch, which is the constraint that made
            // them different sizes in the first place.
            assert!(
                !l.record.intersects(l.stop),
                "at {w}: the transport buttons overlap"
            );
        }
    }

    #[test]
    fn no_two_hit_regions_overlap() {
        for w in [500.0_f32, 900.0, 1300.0, 2600.0] {
            for r in [
                band(w),
                Rect::from_min_size(Pos2::ZERO, DETACHED_DEFAULT),
                Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
            ] {
                for state in STATES {
                    for hide in [false, true] {
                        for rack in racks() {
                            let v = RecorderView {
                                hide_elapsed: hide,
                                ..with_rack(state, rack)
                            };
                            let t = Layout::new(r, &v).targets();
                            for i in 0..t.len() {
                                for j in (i + 1)..t.len() {
                                    assert!(
                                        !t[i].0.intersects(t[j].0),
                                        "{} and {} overlap at {w}pt in {state:?}: {:?} {:?}",
                                        t[i].1.hit(0.0).label(),
                                        t[j].1.hit(0.0).label(),
                                        t[i].0,
                                        t[j].0
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// The slot rows and everything drawn in them keep to themselves.
    ///
    /// The DRAWN zones and not only the clickable ones: the instrument's name
    /// and its dB reading are not hits, and a dB reading painted across the OPEN
    /// button is exactly as unreadable as two buttons on top of each other. The
    /// clickable half of this is [`no_two_hit_regions_overlap`]; this is the
    /// other half, and it also pins the rows inside the band at every width the
    /// window can take.
    #[test]
    fn the_slot_rows_and_everything_in_them_stay_out_of_each_others_way() {
        for r in [
            band(500.0),
            band(900.0),
            band(1300.0),
            band(2600.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            for state in STATES {
                for rack in racks() {
                    let v = with_rack(state, rack);
                    let l = Layout::new(r, &v);
                    let mut zones: Vec<(String, Rect)> = Vec::new();
                    for (i, s) in l.slots.iter().enumerate() {
                        assert!(s.row.is_positive(), "slot {i} has no row at {:?}", r.size());
                        assert!(
                            r.contains_rect(s.row),
                            "slot {i}'s row left the band at {:?}",
                            r.size()
                        );
                        for (what, z) in [
                            ("name", s.name),
                            ("knob", s.knob),
                            ("value", s.value),
                            ("open", s.open),
                            ("clear", s.clear),
                        ] {
                            if z.is_positive() {
                                assert!(
                                    s.row.contains_rect(z),
                                    "slot {i}'s {what} left its own row"
                                );
                                zones.push((format!("slot {i} {what}"), z));
                            }
                        }
                    }
                    // Everything else with ink on it, so a row that grew ate
                    // something visible rather than merely something clickable.
                    let (_, click_track, click_val) = fader_zones(l.metronome_row);
                    let (_, in_track, in_val) = fader_zones(l.input_row);
                    for (what, z) in [
                        ("click track", click_track),
                        ("click reading", click_val),
                        ("input track", in_track),
                        ("input reading", in_val),
                        ("click switch", l.click),
                        ("preview", l.preview),
                        ("meter", l.meter),
                        ("timecode", l.timecode),
                        ("record", l.record),
                        ("stop", l.stop),
                        ("folder", l.dest),
                        ("show button", l.reveal),
                        ("show-when-done tick", l.open_when_done),
                        ("name field", l.name),
                        ("folder preview", l.folder),
                        ("disk", l.disk),
                        ("camera", l.camera),
                        ("audio", l.audio),
                        ("count-in", l.count_in),
                        ("tempo", l.tempo),
                        ("export", l.export),
                        ("status", l.status),
                        ("clip", l.clip),
                    ] {
                        if z.is_positive() {
                            zones.push((what.to_owned(), z));
                        }
                    }
                    for i in 0..zones.len() {
                        for j in (i + 1)..zones.len() {
                            assert!(
                                !zones[i].1.intersects(zones[j].1),
                                "{} and {} overlap in {state:?} at {:?}",
                                zones[i].0,
                                zones[j].0,
                                r.size()
                            );
                        }
                    }
                }
            }
        }
    }

    /// Every control has to be reachable at the point it is drawn, or it is
    /// decoration. Driven off `Hit::ALL` so a new variant that nobody wired up
    /// fails here rather than shipping as a button that does nothing.
    ///
    /// With a full rack, because six of the twenty-six controls belong to a
    /// loaded instrument and an empty slot deliberately offers none of them.
    /// What an EMPTY slot offers is its own test.
    #[test]
    fn every_control_is_reachable_in_the_idle_layout() {
        for r in [
            band(1300.0),
            band(650.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            let v = furnished();
            let l = Layout::new(r, &v);
            for want in Hit::ALL {
                // The one control with no rectangle of its own: whether the
                // click lands in the FILE is set by right-clicking the
                // metronome, because it is set once and was taking a box and a
                // caption in the busiest row of the band. Its reachability is
                // the app's to prove, not the layout's.
                // The take's settings moved to the menu, so they have no
                // rectangle in the band any more. Their reachability is the
                // menu's to prove, not the layout's — the same bargain the
                // in-take toggle already made.
                const IN_THE_MENU: [Hit; 9] = [
                    Hit::ChooseFolder,
                    Hit::RevealFolder,
                    Hit::ToggleDefaultDir,
                    Hit::ToggleOpenWhenDone,
                    Hit::PickCamera,
                    Hit::PickAudio,
                    Hit::CycleCountIn,
                    Hit::Export,
                    Hit::EditTimeSignature,
                ];
                if IN_THE_MENU.iter().any(|m| m.is_same_control(want))
                    || want == Hit::ToggleMetronomeInTake
                {
                    assert!(
                        l.targets().into_iter().all(|(rect, k)| {
                            !k.hit(Hit::MIDWAY).is_same_control(want) || !rect.is_positive()
                        }),
                        "{} grew a rectangle in the band again",
                        want.label()
                    );
                    continue;
                }
                let (rect, _) = l
                    .targets()
                    .into_iter()
                    .find(|(_, k)| k.hit(Hit::MIDWAY).is_same_control(want))
                    .expect("every variant is in `targets`");
                assert!(
                    rect.is_positive(),
                    "{} has no rect while idle at {:?}",
                    want.label(),
                    r.size()
                );
                let got = hit_test(r, &v, rect.center());
                assert!(
                    got.is_some_and(|h| h.is_same_control(want)),
                    "{} is not clickable at its own centre: got {got:?}",
                    want.label()
                );
            }
            // And a miss is a miss: outside the band nothing is hit.
            assert_eq!(hit_test(r, &v, r.min - Vec2::splat(4.0)), None);
            assert_eq!(hit_test(r, &v, r.max + Vec2::splat(4.0)), None);
        }
    }

    /// **Which controls survive a live take**, asserted by sweeping the whole
    /// band rather than by asking the layout, because the question is what a
    /// user can hit and not what the struct says.
    ///
    /// Stop, the two faders, the three slot knobs, the three editor buttons and
    /// the click switch. Nothing else: the folder, the name and the devices are
    /// all decided at `T0`, and offering them at 0:47 would be promising
    /// something the recorder cannot do. Record is gone too — the button is a
    /// red dot while rolling, and a dead control is worse than no control.
    ///
    /// The instrument PICKERS are the interesting absence. They sit in the group
    /// that survives, inches from the knobs that do, and they still have to go:
    /// loading a VST3 blocks the main thread for seconds, so a picker offered
    /// mid-take is an offer to lose the take. Clearing a slot goes with them for
    /// the same reason.
    ///
    /// The levels and the editor buttons are the exception on purpose. Turning
    /// the click down or a layer up halfway through a take is exactly when you
    /// need to, reaching a preset in the plugin's own window between two passes
    /// is a real thing, and none of them can change what has already been
    /// written. The one that could — "in take", which decides whether the click
    /// reaches the file — leaves with the destination.
    #[test]
    fn the_levels_and_editor_buttons_survive_a_take_and_the_pickers_do_not() {
        for state in [
            RecordState::CountIn { beat: 1, of: 8 },
            RecordState::Rolling,
            RecordState::Finishing,
        ] {
            for hide in [false, true] {
                let v = RecorderView {
                    hide_elapsed: hide,
                    ..with_rack(state, racks()[1])
                };
                let r = band(1300.0);
                let mut seen: Vec<Hit> = Vec::new();
                let mut y = r.top();
                while y <= r.bottom() {
                    let mut x = r.left();
                    while x <= r.right() {
                        if let Some(h) = hit_test(r, &v, Pos2::new(x, y)) {
                            assert!(
                                SURVIVORS.iter().any(|s| s.is_same_control(h)),
                                "{} is reachable at ({x}, {y}) during {state:?}",
                                h.label()
                            );
                            if !seen.iter().any(|s| s.is_same_control(h)) {
                                seen.push(h);
                            }
                        }
                        x += 2.0;
                    }
                    y += 2.0;
                }
                for want in SURVIVORS {
                    assert!(
                        seen.iter().any(|s| s.is_same_control(want)),
                        "{} is unreachable during {state:?}",
                        want.label()
                    );
                }
                assert_eq!(seen.len(), SURVIVORS.len());
            }
        }
    }

    /// A control that survives a take and then moves is only half a survivor.
    ///
    /// Everything else in the band jumps when a take starts — that is what the
    /// two layouts are for — so the one group you might reach for at 0:47 has
    /// to be exactly where it was at 0:00, to the point, and not merely still
    /// present somewhere.
    #[test]
    fn the_monitor_is_the_same_rectangle_before_and_during_a_take() {
        for r in [
            band(500.0),
            band(1300.0),
            band(2600.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            let at_rest = Layout::new(r, &furnished());
            let live = Layout::new(r, &furnished_rolling());
            for i in 0..SLOTS {
                assert_eq!(at_rest.slots[i].row, live.slots[i].row, "slot {i} moved");
                // The knob and the button that survive have to survive WHERE
                // they were. The name box keeps its rectangle too — it stops
                // being a control and carries on being the row's label, which
                // is why the row does not reshuffle at the moment a take
                // starts.
                assert_eq!(at_rest.slots[i].knob, live.slots[i].knob, "knob {i} moved");
                assert_eq!(at_rest.slots[i].open, live.slots[i].open, "button {i} moved");
                assert_eq!(at_rest.slots[i].name, live.slots[i].name, "name {i} moved");
                assert!(at_rest.slots[i].pick.is_positive());
                assert!(!live.slots[i].pick.is_positive(), "slot {i} is still pickable");
                assert!(!live.slots[i].clear.is_positive(), "slot {i} is still clearable");
            }
            assert_eq!(at_rest.metronome_row, live.metronome_row, "click moved");
            assert_eq!(at_rest.input_row, live.input_row, "input moved");
            // The click switch is the metronome icon, and it is the SAME
            // rectangle in both layouts — not merely present in both. It is
            // reached mid-take with the performer's eyes on their hands, so it
            // may not move, and it no longer grows into the space `In take`
            // leaves behind: a control that changes size at the moment a take
            // starts is one you have to look at to be sure you hit.
            assert_eq!(at_rest.click, live.click, "the click switch moved");
            // And it is inside the metronome's own row, at the head of it.
            assert_eq!(at_rest.click, fader_zones(at_rest.metronome_row).0);
            // And the divider that marks the group off does not move either.
            assert_eq!(at_rest.rules[1], live.rules[1]);
        }
    }

    /// A fader whose ends cannot be reached is a fader that cannot be turned
    /// off and cannot be turned up, which is both ends of the only two things
    /// anybody does with one.
    /// The 0..=1 a level control reports, whichever of them it is.
    fn position(h: Option<Hit>) -> f32 {
        match h {
            Some(Hit::SetSlotGain(_, t) | Hit::SetMetronomeGain(t) | Hit::SetInputGain(t)) => t,
            other => panic!("that was not a level control: {other:?}"),
        }
    }

    #[test]
    fn a_faders_whole_travel_is_reachable_from_end_to_end() {
        for r in [
            band(1300.0),
            band(700.0),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            // Live as well as idle: the faders are the group that survives.
            for v in [idle(), rolling()] {
                let l = Layout::new(r, &v);
                for (row, want) in [
                    (l.metronome_row, Hit::SetMetronomeGain(0.0)),
                    (l.input_row, Hit::SetInputGain(0.0)),
                ] {
                    let (_, track, _) = fader_zones(row);
                    assert!(track.is_positive(), "{} has no track", want.label());
                    let at = |x: f32| hit_test(r, &v, Pos2::new(x, track.center().y));
                    assert!(
                        at(track.center().x).is_some_and(|h| h.is_same_control(want)),
                        "{} is not the control under its own track",
                        want.label()
                    );
                    assert_eq!(position(at(track.left())), 0.0, "{}", want.label());
                    assert_eq!(position(at(track.right())), 1.0, "{}", want.label());
                    let mid = position(at(track.center().x));
                    assert!((mid - 0.5).abs() < 1e-3, "{} centred at {mid}", want.label());
                    // And the ends mean what the audio path will read them as.
                    assert_eq!(fader_to_gain(position(at(track.left()))), 0.0);
                    assert!(fader_to_gain(position(at(track.right()))) > 1.0);
                }
            }
        }
    }

    /// A knob small enough to be called tiny is still a knob: both ends of its
    /// travel have to be reachable, and the bottom of it has to be SILENCE.
    ///
    /// -60 dB is not off when what it is attenuating is a layered pad under a
    /// piano — it is quiet, audible, and on the recording. Asserted per slot,
    /// because three knobs sharing one mapping is an assumption and not a fact
    /// until the third one has been asked.
    #[test]
    fn every_slots_gain_travel_reaches_silence_and_the_top_of_the_scale() {
        for r in [
            band(1300.0),
            band(700.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            // Live as well as idle: the knobs are in the group that survives.
            for v in [furnished(), furnished_rolling()] {
                let l = Layout::new(r, &v);
                for i in 0..SLOTS {
                    let knob = l.slots[i].knob;
                    assert!(knob.is_positive(), "slot {i} has no knob at {:?}", r.size());
                    let at = |x: f32| hit_test(r, &v, Pos2::new(x, knob.center().y));
                    assert_eq!(position(at(knob.left())), 0.0, "slot {i} bottom");
                    assert_eq!(position(at(knob.right())), 1.0, "slot {i} top");
                    let mid = position(at(knob.center().x));
                    assert!((mid - 0.5).abs() < 1e-3, "slot {i} centred at {mid}");
                    // And the ends mean what the audio path will read them as.
                    assert_eq!(fader_to_gain(position(at(knob.left()))), 0.0);
                    assert!(fader_to_gain(position(at(knob.right()))) > 1.0);
                }
            }
        }
    }

    /// **Three knobs stacked in a column, and a drag that starts on one of them
    /// must not end on another.**
    ///
    /// The app holds the `Hit` it grabbed and asks [`Hit::is_same_control`]
    /// whether the pointer is still on it. A `mem::discriminant` comparison —
    /// which is what every other variant in this enum needs and what this one
    /// used to get — says slot 0's knob and slot 1's knob are the same control,
    /// so a drag begun on the top row would go on setting the middle row's level
    /// after the pointer slipped twenty points down. Silently, and only for
    /// people who drag carelessly, which is everyone.
    #[test]
    fn each_slot_is_a_control_of_its_own_and_not_the_slot_above_it() {
        assert!(!Hit::SetSlotGain(0, 0.2).is_same_control(Hit::SetSlotGain(1, 0.2)));
        assert!(Hit::SetSlotGain(0, 0.2).is_same_control(Hit::SetSlotGain(0, 0.9)));
        assert!(!Hit::PickSlot(1).is_same_control(Hit::PickSlot(2)));
        assert!(!Hit::OpenSlotEditor(0).is_same_control(Hit::OpenSlotEditor(2)));
        assert!(!Hit::ClearSlot(0).is_same_control(Hit::ClearSlot(1)));
        // Different controls of the same slot are still different controls.
        assert!(!Hit::PickSlot(0).is_same_control(Hit::OpenSlotEditor(0)));
        assert!(Hit::SetSlotGain(2, 0.0).is_draggable());
        assert!(!Hit::OpenSlotEditor(2).is_draggable());

        // And the geometry agrees: every one of the twelve regions answers with
        // its own index.
        for r in [band(1300.0), Rect::from_min_size(Pos2::ZERO, DETACHED_MIN)] {
            let v = furnished();
            let l = Layout::new(r, &v);
            for i in 0..SLOTS {
                let s = &l.slots[i];
                for (what, rect, want) in [
                    ("picker", s.pick, Hit::PickSlot(i)),
                    ("knob", s.knob, Hit::SetSlotGain(i, 0.5)),
                    ("editor button", s.open, Hit::OpenSlotEditor(i)),
                    ("clear", s.clear, Hit::ClearSlot(i)),
                ] {
                    assert!(rect.is_positive(), "slot {i}'s {what} is not drawn");
                    let got = hit_test(r, &v, rect.center());
                    assert!(
                        got.is_some_and(|h| h.is_same_control(want)),
                        "slot {i}'s {what} answered {got:?}"
                    );
                }
            }
            // The rows are in the order they are drawn in, top to bottom, so
            // "the second slot" means the second one down and not whichever
            // one the array happened to be built in.
            assert!(l.slots[0].row.top() < l.slots[1].row.top());
            assert!(l.slots[1].row.top() < l.slots[2].row.top());
        }
    }

    /// An empty slot is still a slot: drawn, and clickable to fill.
    ///
    /// This is the whole reason there are three visible rows rather than one row
    /// per loaded instrument. Layering is discoverable because the second and
    /// third rows are sitting there empty inviting a click; a rack that only
    /// showed what was already in it would be a feature you have to be told
    /// about.
    #[test]
    fn an_empty_slot_is_still_drawn_and_still_invites_a_click() {
        for r in [
            band(1300.0),
            band(650.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            let v = idle();
            let l = Layout::new(r, &v);
            for i in 0..SLOTS {
                let s = &l.slots[i];
                assert!(s.row.is_positive(), "slot {i} is not drawn when empty");
                assert!(
                    s.pick.is_positive(),
                    "slot {i} cannot be clicked to fill it"
                );
                assert_eq!(
                    s.name, s.row,
                    "an empty slot's invitation is the whole row, not a corner of it"
                );
                let got = hit_test(r, &v, s.row.center());
                assert!(
                    got.is_some_and(|h| h.is_same_control(Hit::PickSlot(i))),
                    "clicking empty slot {i} answered {got:?}"
                );
                // And nothing that would be a lie: there is no level to set on
                // an empty slot, no window to open and nothing to clear.
                assert!(!s.knob.is_positive(), "an empty slot has a level control");
                assert!(!s.open.is_positive(), "an empty slot offers a window");
                assert!(!s.clear.is_positive(), "an empty slot offers to be cleared");
            }
            // The room the missing controls leave is real room: an empty row's
            // invitation is far wider than a full row's name box.
            let full = Layout::new(r, &furnished());
            assert!(l.slots[0].name.width() > full.slots[0].name.width() * 2.0);
        }
    }

    /// A plugin with no editor is legal VST3, and a button that cannot do
    /// anything is worse than no button: it is a button the user presses twice
    /// and then files a bug about.
    #[test]
    fn the_editor_button_is_there_only_when_the_plugin_has_an_editor() {
        for r in [band(1300.0), Rect::from_min_size(Pos2::ZERO, DETACHED_MIN)] {
            let with = with_rack(RecordState::Idle, [loaded("Pianoteq 8", 1.0, true); SLOTS]);
            let without = with_rack(RecordState::Idle, [loaded("sfizz", 1.0, false); SLOTS]);
            for i in 0..SLOTS {
                let open = Layout::new(r, &with).slots[i].open;
                assert!(open.is_positive(), "slot {i} has no editor button");
                assert!(
                    hit_test(r, &with, open.center())
                        .is_some_and(|h| h.is_same_control(Hit::OpenSlotEditor(i))),
                    "slot {i}'s editor button is not clickable"
                );
                let dead = &Layout::new(r, &without).slots[i];
                assert!(
                    !dead.open.is_positive(),
                    "slot {i} drew a button for a plugin with no window"
                );
                // The row is otherwise unchanged, and the space where the
                // button would have been answers to nobody rather than to its
                // neighbours.
                assert_eq!(dead.knob, Layout::new(r, &with).slots[i].knob);
                assert_eq!(hit_test(r, &without, open.center()), None);
            }
        }
    }

    /// The tempo is dragged along its box, and a drag that could reach 0.001
    /// BPM would draw a DAW a bar ruler several centuries long.
    #[test]
    fn the_tempo_drag_covers_the_legal_range_and_no_more() {
        let r = band(1300.0);
        let v = idle();
        let l = Layout::new(r, &v);
        let at = |x: f32| match hit_test(r, &v, Pos2::new(x, l.tempo.center().y)) {
            Some(Hit::SetTempo(b)) => b,
            other => panic!("that was not the tempo: {other:?}"),
        };
        assert!((at(l.tempo.left()) - MIN_BPM).abs() < 1e-9);
        assert!((at(l.tempo.right()) - MAX_BPM).abs() < 1e-9);
        let mid = at(l.tempo.center().x);
        assert!(
            (mid - (MIN_BPM + MAX_BPM) * 0.5).abs() < 1.0,
            "the middle of the box is {mid} BPM"
        );
        // Monotonic across the whole box, or a rightward drag could slow down.
        let mut last = f64::MIN;
        for i in 0..=50 {
            let bpm = at(l.tempo.left() + l.tempo.width() * i as f32 / 50.0);
            assert!(bpm >= last, "the tempo went backwards at {i}");
            assert!((MIN_BPM..=MAX_BPM).contains(&bpm), "{bpm} is out of range");
            last = bpm;
        }
    }

    /// The clock the setting hides is a CLOCK. The count-in beat is the number
    /// the player is counting on, and a performance setting that swallowed it
    /// would be hiding the wrong thing under the right name.
    #[test]
    fn the_count_in_readout_shows_the_beat_and_hide_elapsed_leaves_it_alone() {
        // Just the number the player is saying out loud. It read "3 OF 4"
        // while the count was a running total against a total; now that it is
        // a beat WITHIN THE BAR — 1 2 3 4 5 6, 1 2 3 4 5 6 in 6/8 — the second
        // half carries nothing the SIG cell does not already say.
        assert_eq!(readout_text(RecordState::CountIn { beat: 3, of: 4 }, 0.0), "3");
        assert_eq!(
            readout_text(RecordState::CountIn { beat: 5, of: 6 }, 91.0),
            "5",
            "the elapsed clock does not leak into the count"
        );
        assert_eq!(readout_text(RecordState::Finishing, 12.0), "FINISHING");
        assert_eq!(readout_text(RecordState::Rolling, 252.0), "4:12");

        let r = band(1300.0);
        for hide in [false, true] {
            let v = RecorderView {
                state: RecordState::CountIn { beat: 1, of: 4 },
                hide_elapsed: hide,
                ..RecorderView::empty()
            };
            assert!(
                Layout::new(r, &v).timecode.is_positive(),
                "the beat vanished with hide_elapsed = {hide}"
            );
        }
    }

    /// The setting no competitor offers: after a blinking light, a running
    /// timer is the most-cited performance distraction. It is reached from the
    /// menu now — there is no `CLOCK` box in the band, because the owner could
    /// not tell what one was for — but the readout still obeys it.
    #[test]
    fn the_clock_is_not_drawn_while_rolling_when_it_is_hidden() {
        let r = band(1300.0);
        let hidden = |state| {
            Layout::new(
                r,
                &RecorderView {
                    state,
                    hide_elapsed: true,
                    ..RecorderView::empty()
                },
            )
            .timecode
        };
        assert!(
            !hidden(RecordState::Rolling).is_positive(),
            "the clock is still on screen during a take"
        );
        assert!(
            hidden(RecordState::CountIn { beat: 2, of: 4 }).is_positive(),
            "the count-in is the number the user is waiting for"
        );
        assert!(
            hidden(RecordState::Finishing).is_positive(),
            "'files are still being closed' is not a clock"
        );
        assert!(
            hidden(RecordState::Idle).is_positive(),
            "the setting is about recording, not about sitting there"
        );
        // Without the setting the clock is drawn in every state, in exactly
        // the same box. **Rolling no longer rearranges the band.** The clock
        // used to swell to four times its resting size, which meant the meters
        // narrowed and the buttons moved at the one moment nobody can afford
        // to look away and find things elsewhere. The dot replacing the record
        // button is the only difference between the two layouts now.
        let shown = |state| {
            Layout::new(
                r,
                &RecorderView {
                    state,
                    ..RecorderView::empty()
                },
            )
        };
        let at_rest = shown(RecordState::Idle).timecode;
        let live = shown(RecordState::Rolling).timecode;
        assert!(at_rest.is_positive() && live.is_positive());
        assert_eq!(
            live, at_rest,
            "the clock moved when the take started: {live:?} vs {at_rest:?}"
        );
        // The meter, the stop button and the faders are still there either way.
        for state in [RecordState::Idle, RecordState::Rolling] {
            let l = Layout::new(
                r,
                &RecorderView {
                    state,
                    hide_elapsed: true,
                    ..RecorderView::empty()
                },
            );
            assert!(l.meter.is_positive(), "the meter went away in {state:?}");
            assert!(l.stop.is_positive(), "the transport went away in {state:?}");
            assert!(
                l.metronome_row.is_positive() && l.click.is_positive(),
                "the monitor went away in {state:?}"
            );
            assert!(
                l.slots.iter().all(|s| s.row.is_positive()),
                "the instrument slots went away in {state:?}"
            );
        }
    }

    /// The transport is a picture of the state before it is anything else: a
    /// round red button at rest, a steady dot while live, in the same place.
    #[test]
    fn the_record_button_becomes_the_dot_and_never_both() {
        let r = band(1300.0);
        let at_rest = Layout::new(r, &idle());
        assert!(at_rest.record.is_positive() && !at_rest.dot.is_positive());
        let live = Layout::new(r, &rolling());
        assert!(live.dot.is_positive() && !live.record.is_positive());
        assert!(
            live.preview.width() < at_rest.preview.width(),
            "the preview did not collapse for the rolling layout"
        );
        // **The meter no longer has to grow, and asserting that it does would
        // be asserting the old band.** It was tiny at rest when the transport
        // was a quarter of a narrow column, so growing was the only way it
        // could become readable. The transport is half the band now and the
        // meters are large in both layouts; what the rolling one buys is the
        // CLOCK, which takes nearly half the group. So: the meter stays
        // substantial, and the clock is what changes.
        let area = |r: Rect| r.width() * r.height();
        assert!(
            area(live.meter) > area(at_rest.meter) * 0.4,
            "the rolling meter collapsed: {:?} vs {:?}",
            live.meter,
            at_rest.meter
        );
        assert!(
            live.meter.height() >= at_rest.meter.height(),
            "the rolling meter got shorter, which is what its faces are sized by"
        );
        // And the tick that decides what goes in the FILE leaves with the rest
        // of the destination, while the switch that decides what you HEAR does
        // not.

        assert!(live.click.is_positive());
    }

    /// **The needle and the scale it is read against cannot disagree.**
    ///
    /// A drawn meter has one failure worse than being ugly: pointing somewhere
    /// the printing says is a different number. Both come out of `VU_MARKS`, so
    /// this test checks the one thing that could still drift — that a level
    /// lands at the fraction its own printed mark is drawn at.
    #[test]
    fn the_vu_needle_points_at_the_mark_the_face_prints() {
        for (vu, frac, _) in VU_MARKS {
            let level = 10_f32.powf((vu + VU_ZERO_DBFS) / 20.0);
            let got = vu_frac(level);
            assert!(
                (got - frac).abs() < 1e-3,
                "{vu} VU draws at {frac} and reads at {got}"
            );
        }
        // 0 VU is the top of the black arc and the start of the red, and it is
        // the same level the band has always drawn its red zone from.
        assert!((vu_frac(CLIP_ZONE) - VU_MARKS[6].1).abs() < 0.02);
        // Monotonic, and pinned at both stops: a needle that goes backwards or
        // walks off the card is worse than no meter.
        let mut last = -1.0;
        for i in 0..=200 {
            let f = vu_frac(i as f32 / 100.0);
            assert!(f >= last, "the needle went backwards at {i}");
            assert!((0.0..=1.0).contains(&f), "{f} is off the face");
            last = f;
        }
        assert_eq!(vu_frac(0.0), 0.0, "silence is not the left stop");
        assert_eq!(vu_frac(4.0), 1.0, "a huge level is not the right stop");
    }

    /// The cap rides IN the channel and stops at both ends of it, because the
    /// ends of a fader's travel have to look like ends — and because a cap half
    /// off the panel is the picture of a broken machine.
    #[test]
    fn a_fader_cap_stays_on_its_own_track_at_both_stops() {
        for r in [band(500.0), band(1300.0), band(2600.0)] {
            let l = Layout::new(r, &furnished());
            let mut tracks: Vec<Rect> = vec![fader_zones(l.metronome_row).1, fader_zones(l.input_row).1];
            tracks.extend(l.slots.iter().map(|s| s.knob));
            for track in tracks {
                if !track.is_positive() {
                    continue;
                }
                // Silence, unity and the top of the scale — the two stops and
                // the one position everybody actually looks for.
                for gain in [0.0_f32, 1.0, 100.0] {
                    let cap = cap_rect(track, gain);
                    assert!(
                        cap.left() >= track.left() - 0.01 && cap.right() <= track.right() + 0.01,
                        "the cap hangs off the track at {gain} in {:?}",
                        r.size()
                    );
                    assert!(
                        cap.height() > slot_height(track.height()),
                        "the cap is not taller than the channel it rides in"
                    );
                }
                // And it MOVES: a cap pinned at one end for every level would
                // pass every assertion above.
                assert!(cap_rect(track, 1.0).center().x > cap_rect(track, 0.0).center().x);
            }
        }
    }


    /// A fader with no number beside it is a strip of colour, which is what the
    /// owner was complaining about. The number has to actually FIT, and the
    /// widest thing it can say is eight characters wide.
    #[test]
    fn a_gain_reading_fits_the_box_reserved_for_it_at_the_smallest_band() {
        let widest = (0..=100)
            .map(|i| gain_text(fader_to_gain(i as f32 / 100.0)))
            .max_by_key(|t| t.chars().count())
            .expect("the sweep is not empty");
        assert_eq!(widest.chars().count(), 8, "{widest} is a new worst case");

        for r in [
            band(500.0),
            band(900.0),
            band(1300.0),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            for v in [idle(), rolling()] {
                let l = Layout::new(r, &v);
                for (which, row) in [("click", l.metronome_row), ("input", l.input_row)] {
                    let (icon, _, val) = fader_zones(row);
                    let size = fit_text(val, &widest, val.height() * FADER_TEXT);
                    assert!(
                        size >= MIN_TEXT,
                        "'{widest}' draws at {size}pt in {r:?}, which is a smudge"
                    );
                    // The icon is a picture and not a word, so what it needs is
                    // a square it can be inscribed in — see `draw_fader_icon`,
                    // which draws nothing at all below six points.
                    let s = icon.width().min(icon.height()) * 0.92;
                    assert!(
                        s >= 6.0,
                        "the {which} icon gets {s}pt of square in {r:?}, which is a blot"
                    );
                }
            }
        }
    }

    /// The tiny knob needs its number MORE than a full-width fader does: there
    /// is no caption beside it saying what it belongs to and no room for one, so
    /// `-6.0 dB` is the only thing that says how far up it is.
    ///
    /// Not at `band(500)`, where the whole band is smudges and the rule is that
    /// text below `MIN_TEXT` is not drawn at all — but at every size anybody
    /// records at, including the smallest detached window the app allows.
    #[test]
    fn a_slots_knob_carries_a_readable_number_at_every_size_worth_recording_at() {
        let widest = "-60.0 dB";
        for r in [
            band(900.0),
            band(1300.0),
            band(2600.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            for v in [furnished(), furnished_rolling()] {
                let l = Layout::new(r, &v);
                for i in 0..SLOTS {
                    let s = &l.slots[i];
                    let size = fit_text(s.value, widest, s.value.height() * FADER_TEXT);
                    assert!(
                        size >= MIN_TEXT,
                        "slot {i} reads '{widest}' at {size}pt in {:?}",
                        r.size()
                    );
                    // And the name, which is the other thing a row has to say.
                    let name = fit_text(s.name, "1 Pianoteq 8", s.name.height() * 0.52);
                    assert!(name >= MIN_TEXT, "slot {i}'s name draws at {name}pt");
                    // The button says what it does. "OPEN" is the fallback and
                    // the fuller wording is used wherever there is room; either
                    // way it is not a smudge.
                    let choices = ["OPEN WINDOW", "WINDOW", "OPEN"];
                    let (text, size) = fit_label(s.open, &choices, s.open.height() * 0.46);
                    assert!(
                        size >= MIN_TEXT,
                        "slot {i}'s button says '{text}' at {size}pt in {:?}",
                        r.size()
                    );
                }
            }
        }
        // The label picker takes the longest wording that fits at full size and
        // shrinks only the shortest, so a wide button never says "OPEN" and a
        // narrow one never says it in 4pt type.
        let choices = ["OPEN WINDOW", "OPEN"];
        let wide = Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 20.0));
        let narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(30.0, 20.0));
        let tiny = Rect::from_min_size(Pos2::ZERO, Vec2::new(12.0, 20.0));
        assert_eq!(fit_label(wide, &choices, 10.0), ("OPEN WINDOW", 10.0));
        // Room for the short wording at full size, none for the long one: the
        // short one is taken rather than the long one squeezed.
        assert_eq!(fit_label(narrow, &choices, 10.0), ("OPEN", 10.0));
        // And past that, the last choice is shrunk rather than dropped.
        let (text, size) = fit_label(tiny, &choices, 10.0);
        assert_eq!(text, "OPEN");
        assert!(size < 10.0, "the fallback was not shrunk: {size}pt");
    }

    #[test]
    fn the_count_in_control_cycles_through_every_choice_and_comes_back() {
        let mut seen = Vec::new();
        let mut v = COUNT_IN_CHOICES[0];
        for _ in 0..COUNT_IN_CHOICES.len() {
            seen.push(v);
            v = next_count_in(v);
        }
        assert_eq!(seen, COUNT_IN_CHOICES.to_vec());
        assert_eq!(v, COUNT_IN_CHOICES[0], "the cycle does not come back round");
        // A value a hand-edited settings file could hold lands somewhere real
        // rather than sticking.
        assert_eq!(next_count_in(97), COUNT_IN_CHOICES[0]);
        // And the label says BARS, because that is what the choices are now
        // that there is a time signature to count them in. It said beats, and
        // went on saying beats after the setting became bars — so the cell
        // showed a number the control no longer changed.
        assert_eq!(count_in_text(0), "off");
        assert_eq!(count_in_text(1), "1 bar");
        assert_eq!(count_in_text(2), "2 bars");
    }

    /// **Everything draggable is typeable; not everything typeable is
    /// draggable.**
    ///
    /// It used to be an equality, and that was right while every numeric cell
    /// was a fader: a draggable hit with no field would swallow a tap and do
    /// nothing. The time signature broke the symmetry honestly rather than
    /// accidentally — "6/8" is two numbers and a slash, and there is no
    /// continuum between 4/4 and 7/8 to drag along, so it is typed and only
    /// typed.
    #[test]
    fn every_control_you_can_drag_is_one_you_can_type_into() {
        for hit in Hit::ALL {
            if hit.is_draggable() {
                assert!(
                    num_field(hit).is_some(),
                    "{hit:?} can be dragged and not typed into"
                );
            }
        }
        // And the one that goes the other way is the one it is supposed to be.
        let typed_only: Vec<Hit> = Hit::ALL
            .into_iter()
            .filter(|h| num_field(*h).is_some() && !h.is_draggable())
            .collect();
        assert_eq!(typed_only, vec![Hit::EditTimeSignature]);
    }

    /// The four fields are four fields, not one used four times.
    #[test]
    fn each_control_owns_its_own_field() {
        let fields: Vec<_> = Hit::ALL.into_iter().filter_map(num_field).collect();
        let mut seen = fields.clone();
        seen.sort_by_key(|f| format!("{f:?}"));
        seen.dedup();
        assert_eq!(
            seen.len(),
            fields.len(),
            "two controls share a field, so typing into one would edit the other"
        );
        assert!(fields.contains(&NumField::Tempo));
        for i in 0..SLOTS {
            assert!(fields.contains(&NumField::Slot(i)), "slot {i} has no field");
        }
    }

    #[test]
    fn a_tempo_reads_as_a_round_number_unless_it_is_not_one() {
        assert_eq!(tempo_text(120.0), "120");
        assert_eq!(tempo_text(92.5), "92.5");
        assert_eq!(tempo_text(60.0), "60");
    }

    /// A chosen device that is not plugged in is neither "None" nor working,
    /// and it must not read as either: one is a thing to set up and the other
    /// is a thing to go and fix. An instrument that would not load is a third
    /// thing again, and "not connected" would send the user looking for a
    /// cable.
    #[test]
    fn a_missing_device_reads_differently_from_an_open_one() {
        assert_eq!(device_note(DeviceLabel::Open("FaceTime HD"), "  (gone)"), "");
        assert_eq!(device_note(DeviceLabel::None, "  (gone)"), "");
        assert!(device_note(DeviceLabel::Missing("Scarlett"), "  (not connected)")
            .contains("not connected"));
        assert!(device_note(DeviceLabel::Missing("Pianoteq"), "  (did not load)")
            .contains("did not load"));
        let s = Settings::default();
        let p = palette(&s);
        assert_eq!(device_ink(DeviceLabel::Missing("x"), &p), p.warn);
        assert_eq!(device_ink(DeviceLabel::Open("x"), &p), p.ink);
        assert_eq!(device_ink(DeviceLabel::None, &p), p.faint);
        assert_ne!(p.warn, p.ink, "a missing device is drawn in the same ink");
        assert_ne!(p.warn, p.faint);
    }

    #[test]
    fn the_export_line_says_what_a_take_will_produce() {
        use crate::recorder::VideoMode;
        assert_eq!(export_summary(&ExportSpec::default()), "wav + midi");
        assert_eq!(
            export_summary(&ExportSpec {
                video: VideoMode::Composite,
                ..Default::default()
            }),
            "wav + midi + 1 video"
        );
        assert_eq!(
            export_summary(&ExportSpec {
                audio: false,
                video: VideoMode::Both,
                ..Default::default()
            }),
            "midi + 3 videos"
        );
        // `problem()` refuses this at the moment of recording, but the band
        // still has to render it rather than showing an empty line.
        assert_eq!(
            export_summary(&ExportSpec {
                audio: false,
                midi: false,
                ..Default::default()
            }),
            "nothing"
        );
    }

    /// The painter has to survive every state at every size, with and without a
    /// frame. It draws real glyphs, so a bare `Context` with no fonts bound
    /// panics rather than failing an assertion — hence the install.
    #[test]
    fn the_band_paints_in_every_state_at_every_size() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        let pv = Preview {
            texture: egui::TextureId::User(7),
            size: Vec2::new(640.0, 480.0),
        };
        for dark in [false, true] {
            let s = Settings {
                dark_mode: dark,
                ..Settings::default()
            };
            for w in [0.0_f32, 60.0, 400.0, 1300.0, 2600.0] {
                for state in STATES {
                    for preview in [None, Some(pv)] {
                        // Every control that can be typed into, drawn mid-edit
                        // at every width — including the degenerate ones, where
                        // the caret's own rectangle is what would go negative.
                        for edit in typing_states() {
                            for rack in racks() {
                            let v = RecorderView {
                                state,
                                elapsed_s: 3725.0,
                                meters: Meters {
                                    left: Level {
                                        peak: 1.4,
                                        rms: 0.3,
                                        hold: 0.9,
                                    },
                                    right: Level::default(),
                                    mono: state == RecordState::Rolling,
                                    clipped: true,
                                },
                                dest: "~/Movies/Tangent",
                                take_name: "nocturne",
                                name_focused: true,
                                editing: edit.as_ref(),
                                folder_preview: "nocturne-2026-08-16-141203",
                                slots: rack,
                                // Every fader at a different, awkward place:
                                // silence, past unity, and the click's default.
                                gains: Gains {
                                    slots: [3.98, 0.0, 1.0, 0.5, 0.25],
                                    metronome: 0.0,
                                    input: 0.5,
                                },
                                metronome_on: true,
                                metronome_in_take: dark,
                                tempo_bpm: 92.5,
                                count_in_beats: 8,
                                count_in_bars: 2,
                                time_signature: crate::recorder::TimeSignature { beats: 6, unit: 8 },
                                camera: DeviceLabel::Missing("FaceTime HD Camera"),
                                audio: DeviceLabel::Open("Scarlett 2i2 USB"),
                                preview,
                                disk_minutes: Some(134.0),
                                hide_elapsed: dark,
                                message: Some("recorded 4:12 to nocturne-2026-08-16-141203"),
                                clip_warning: true,
                            };
                            let r = band(w);
                            let _ = ctx.run(Default::default(), |ctx| {
                                egui::CentralPanel::default().show(ctx, |ui| {
                                    draw(ui.painter(), r, &v, &s);
                                    draw_top_edge(ui.painter(), r, &s);
                                });
                            });
                            // And the hit test agrees it is looking at the same
                            // rectangle, at every size, without panicking on the
                            // degenerate ones.
                            let _ = hit_test(r, &v, r.center());
                            }
                        }
                    }
                }
            }
        }
    }

    /// No edit, plus one per typeable field, plus the two shapes a field passes
    /// through on the way to a number: empty, and a lone minus sign.
    fn typing_states() -> Vec<Option<crate::recorder::NumEdit>> {
        let mut v = vec![None];
        for field in [
            NumField::Slot(0),
            NumField::Slot(SLOTS - 1),
            NumField::Metronome,
            NumField::Input,
            NumField::Tempo,
        ] {
            for text in ["", "-", "-12.5"] {
                v.push(Some(crate::recorder::NumEdit {
                    field,
                    text: text.to_owned(),
                }));
            }
        }
        v
    }
}
