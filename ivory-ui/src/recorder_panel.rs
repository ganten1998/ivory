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
    DEFAULT_BPM,
    gain_text, gain_to_fader, timecode, DeviceLabel, ExportSpec, Level, Meters,
    NumField, Preview, RecordState, RecorderView, SlotView, COUNT_IN_CHOICES, MAX_BPM, MIN_BPM,
    SLOTS,
};
use crate::ports::KnobUnit;
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
pub const BAND_H_AT_1300: f64 = 190.0;

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

    /// The master column: the output meter, the limiter's gain reduction
    /// beside it, and the master knob under both.
    ///
    /// **A different signal from `meter`.** The VU shows what is being
    /// RECORDED — the input when there is one — and this shows what LEAVES:
    /// after the effects, after the limiter, after the master. On a machine
    /// with an interface plugged in those are two genuinely different things,
    /// and neither one answers the other's question.
    /// The dB scale, **on both sides of the ladders**. Each channel is read
    /// against the numbers next to it; a right channel three inches from the
    /// only scale is a right channel nobody reads accurately.
    master_scale: [Rect; 2],
    master_bars: Rect,
    master_knob: Rect,

    /// The six effect knobs, in [`Fx::ALL`] order, under the meters.
    ///
    /// The whole CELL, label included, and the whole cell is the drag target:
    /// a knob is thirty points across and a control you can only grab by its
    /// own diameter is a control you keep missing.
    fx: [Rect; Fx::ALL.len()],
    /// Where a typed tempo goes: a small box under the knob, drawn only while
    /// one is being typed.
    tempo_entry: Rect,

    /// The backing track's fader, under the click and the input.
    track_row: Rect,
    /// Its icon: a click imports a file, a right-click opens the waveform.
    track_icon: Rect,

    /// The microphone icon at the head of the input fader.
    ///
    /// Not a press target — a right-click on it opens the audio input's
    /// picker, the way a right-click on the metronome sets whether the click
    /// lands in the file. The device belongs to the fader it feeds, and a
    /// picker reachable only from a menu of subjects that are mostly about the
    /// piano is a picker nobody finds.
    input_icon: Rect,

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
    name: Rect,
    /// Live grey text under the name field. Teaches the naming scheme without
    /// a help page, and is not clickable.
    folder: Rect,
    /// How long the disk will last, beside the folder preview. Also not
    /// clickable: it is an answer, not a question.
    disk: Rect,
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
    // **Scaled so the last row's bottom IS the bottom.** Rows of 86% of the
    // pitch leave a gap after the last one as well as between them, and that
    // trailing gap was a row's worth of nothing under the fifth instrument.
    let k = 1.0 / (1.0 - pitch * 0.14);
    while i < SLOTS {
        let top = i as f32 * pitch;
        out[i] = (top * k, (top + pitch * 0.86) * k);
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
/// **0.30 in 5.0**, down from 0.36. The full-height camera preview took its
/// points from somewhere, and a slot row is four things on one line while the
/// VU pair is the thing anybody actually watches while a take runs.
/// The instrument column's share of what the preview leaves.
///
/// **Down from what it was, because the rows had slack and the transport had
/// none.** A slot row is a name, a slider and a number; at a third of the band
/// it was carrying an inch of empty panel after "empty (click to load)" while
/// the faders next door were being measured against whether their dB reading
/// would render at five points.
const MONITOR_W: f32 = 0.30;

/// The fewest points the instrument column may have, whatever the share says.
///
/// A share alone is wrong in a DETACHED recorder window, which is tall and
/// narrow: the preview takes its quarter off the top of a small width, and
/// thirty percent of the little that is left is a slot row whose gain reading
/// will not render. Measured against
/// `a_slots_knob_carries_a_readable_number_at_every_size_worth_recording_at`,
/// which is the test that says so.
const MONITOR_MIN_W: f32 = 195.0;

/// The most of the box that floor may claim. See [`Layout::monitor_of`].
const MONITOR_FLOOR_MAX: f32 = 0.38;

/// The camera preview's width over its height. The commonest sensor shape, and
/// the shape of the file a take writes.
const PREVIEW_ASPECT: f32 = 16.0 / 9.0;

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
/// switches now sitting in the click's own row.
pub fn fader_zones(row: Rect) -> (Rect, Rect, Rect) {
    if !row.is_positive() {
        return (Rect::NOTHING, Rect::NOTHING, Rect::NOTHING);
    }
    // The icon gets a wider share than it looks like it needs. It is drawn
    // inscribed in a SQUARE, so at seven hundredths of a narrow column it was
    // width-bound at four points however tall the row was — a metronome that
    // small is a blot, and `draw_fader_icon` refuses to draw one at all.
    (
        slice_h(row, 0.00, 0.11),
        slice_h(row, 0.13, 0.73),
        slice_h(row, 0.755, 1.00),
    )
}

/// Text height in a fader row, as a fraction of the row. Shared by the painter
/// and by the test that asserts the dB reading fits.
///
/// A sixth down from where it was. The reading is a check, not a headline: it
/// grew with the row when the faders took the words column's width, and a
/// number the size of the chord name is a number claiming to matter more than
/// the fader it belongs to.
const FADER_TEXT: f32 = 0.53;

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
            input_icon: Rect::NOTHING,
            track_row: Rect::NOTHING,
            track_icon: Rect::NOTHING,
            master_scale: [Rect::NOTHING; 2],
            master_bars: Rect::NOTHING,
            master_knob: Rect::NOTHING,
            fx: [Rect::NOTHING; Fx::ALL.len()],
            tempo_entry: Rect::NOTHING,
            setup: Rect::NOTHING,
            dest: Rect::NOTHING,
            reveal: Rect::NOTHING,
                name: Rect::NOTHING,
            folder: Rect::NOTHING,
            disk: Rect::NOTHING,
                    count_in: Rect::NOTHING,
            tempo: Rect::NOTHING,
            time_sig: Rect::NOTHING,
            export: Rect::NOTHING,
            open_when_done: Rect::NOTHING,
            status: Rect::NOTHING,
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

        // **The preview keeps the band's own margin on three sides.** It is
        // the camera inset a take will carry, so it is not a control among
        // controls: it is a picture, and a picture hung with a different gap
        // under it than beside it reads as an accident. The same `pad` above
        // it, to the left of it, and below it — which means it takes the full
        // height of `inner` and the status line starts to its right.
        let pv_w = (inner.height() * PREVIEW_ASPECT).min(inner.width() * 0.24);
        let preview = Rect::from_min_max(
            inner.min,
            Pos2::new(inner.left() + pv_w, inner.bottom()),
        );

        // Everything else lives to the right of it.
        let rest = Rect::from_min_max(
            Pos2::new(preview.right() + gap, inner.top()),
            inner.max,
        );
        if !rest.is_positive() {
            return Self::empty(rolling);
        }
        // The status line spans what the preview leaves, in both layouts, so
        // "no audio input selected" is in the same place before and during a
        // take.
        // **One line of small text, and usually empty.** It was a fifth of
        // the column reserved for a message that is not there most of the
        // time, and every section above it stopped that much short of the
        // band's bottom edge. The space it does need is still reserved — a
        // band that changed shape when a warning appeared would move the
        // controls under a hand — but it is the height of the line now.
        let status_h = (rest.height() * 0.10).min(15.0);
        let status = Rect::from_min_max(
            Pos2::new(rest.left(), rest.bottom() - status_h),
            rest.max,
        );
        let body = Rect::from_min_max(
            rest.min,
            Pos2::new(rest.right(), status.top() - gap * 0.5),
        );
        if !body.is_positive() {
            return Self::empty(rolling);
        }
        let mut l = Self {
            preview,
            // The message never runs into the clip warning, whatever either
            // says: they share the row and neither may be the reason the other
            // is unreadable.
            // **The whole row.** It shared it with a `CLIPPED` word that has
            // gone: the lamp on the VU face says that, in the place somebody
            // watching levels is already looking.
            status,
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
        // **The floor is itself capped.** A minimum in POINTS is right in a
        // detached window, which is narrow but not tiny; applied to the
        // smallest band this app draws it takes half of everything and starves
        // the faders, which is the failure the share was there to prevent in
        // the first place.
        //
        // `MONITOR_FLOOR_MAX` is where the two tests that pull against each
        // other both pass: `a_slots_knob_carries_a_readable_number_at_every_
        // size_worth_recording_at` from below and `a_gain_reading_fits_the_box_
        // reserved_for_it_at_the_smallest_band` from above. There is not much
        // room between them, and that is a fact about a five-hundred-point
        // band rather than a number anybody chose.
        let floor = MONITOR_MIN_W.min(body.width() * MONITOR_FLOOR_MAX);
        let w = (body.width() * MONITOR_W).max(floor);
        Rect::from_min_max(
            Pos2::new(body.right() - w, body.top()),
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
        // **Capped lower than it was.** The strip is a credit at the foot of
        // the instrument column, and at 24 points it stopped the fifth slot a
        // row's height above every other section's bottom edge. The heart is
        // drawn at 85% of it and is a picture of a heart; it does not need a
        // slot's worth of column.
        (body.height() * 0.13).clamp(9.0, 17.0)
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
        // The picture on the far left, full height; the words in their own
        // column beside it. They used to be stacked in one narrow column,
        // which made the preview short AND the type small — the two were
        // fighting over the same points and both lost.

        let monitor = Self::monitor_of(body);
        self.fill_heart(body, monitor);
        self.fill_monitor(monitor, view);
        self.rules[1] = rule_x(body, monitor.left() - gap * 0.5);

        // One source for the middle, shared with the rolling layout and with
        // `fill_faders`, so the faders cannot drift from the meters beside
        // them. See `Layout::middle_of`.
        let middle = Self::middle_of(body, gap);
        if !middle.is_positive() {
            return;
        }
        self.rules[0] = rule_x(body, middle.left() - gap * 0.5);
        self.fill_transport(middle);
        self.fill_faders(body, gap, false);
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
        // **There is no words column any more.** The take name moved into the
        // take-settings popup, the tempo became a small typed box at the head
        // of the faders, and the transport went to the foot of them — so the
        // fifth of the band that column was using is width the faders and the
        // meters now have.
        let monitor = Self::monitor_of(body);
        Rect::from_min_max(body.min, Pos2::new(monitor.left() - gap, body.bottom()))
    }


    /// The left column of the middle group: tempo, two faders, transport.
    ///
    /// **Four rows in one column, and they belong together.** They were three
    /// separate things — a words column with the name and the tempo in it, a
    /// fader pair, and a transport bar at the foot of the words — which is
    /// three columns' worth of margin for two faders and three buttons. What
    /// is here now is the strip a hand rests on during a take: how loud the
    /// click is, how loud the input is, how fast, and go.
    fn fill_faders(&mut self, body: Rect, gap: f32, rolling: bool) {
        let m = Self::middle_of(body, gap);
        if !m.is_positive() {
            return;
        }
        // Their own column, between the transport and the meters, and centred
        // in it: two rows in the middle of a tall column rather than two rows
        // pinned to the bottom of the band, which is what made the bottom of
        // the window read as empty.
        // **The knobs left this column**, and the faders got the width back —
        // see `fill_transport`, where the three of them are now a row under
        // the meters. What they were was a stack ten points wide, which is a
        // knob you cannot read and cannot land on.
        let col = slice_h(m, 0.00, FADER_COL);

        // Tall rows in a narrow column. Moving the pair off the meters bought
        // width to give away and none to spare, so the legibility comes back
        // out of the HEIGHT: the two rows take nearly the whole column, which
        // was dead space above and below them, and the dB reading at the end
        // of each track stays a number rather than a smudge at the smallest
        // band this app will draw.
        // **The transport row carries a knob now, so it needs a knob's
        // height.** The faders lose a little of theirs to pay for it: a fader
        // is a bar and a number and reads at any height, and a knob below a
        // certain size is three grey rings.
        // **Three rows now, and the transport keeps its knob's worth.** The
        // backing track is a level like the other two and belongs with them:
        // it is the third thing in the mix that is not an instrument.
        self.metronome_row = slice_v(col, 0.02, 0.21);
        self.input_row = slice_v(col, 0.25, 0.44);
        self.track_row = slice_v(col, 0.48, 0.67);
        self.click = fader_zones(self.metronome_row).0;
        self.input_icon = fader_zones(self.input_row).0;
        self.track_icon = fader_zones(self.track_row).0;

        // **The transport, under the faders it belongs with.** Three squares
        // of one size, one centred in each third of the row: they are one
        // group, and a stop bigger than the cog beside it reads as more
        // important than it is.
        let bar = slice_v(col, 0.70, 1.00);
        // **The tempo is a knob at the head of the transport**, and that is
        // what makes the transport one thing rather than three buttons and a
        // number that happened to be nearby.
        //
        // **The same cell as an effect knob, to the point.** They are the same
        // control and the eye reads them as a set; a tempo knob a few points
        // off the size of the three beside it looks like a mistake nobody can
        // name. Sized from the first effect cell, which `fill_transport` has
        // already placed — not from a fraction of this row, which would agree
        // with it only by luck and only at one window size.
        //
        // **And the clock takes its place while a take runs.** The tempo
        // cannot be changed mid-take — the `.mid`'s tempo map is already
        // written — so leaving a live knob there would be a control that lies.
        // What goes in its place is the one number anybody looks at from a
        // piano bench, in the row their hand is already on.
        let head = if self.fx[0].is_positive() {
            let size = Vec2::new(
                self.fx[0].width().min(bar.width() * 0.5),
                self.fx[0].height().min(bar.height()),
            );
            // **Sat on the bottom of the row, not floated in the middle of
            // it.** The tempo knob is one of eight and the other seven end at
            // the foot of their own column; centring this one left it a dozen
            // points higher than the rest, with the gap under it reading as
            // the band's bottom margin — which is what "a huge unused margin
            // below the buttons" actually was.
            Rect::from_center_size(
                Pos2::new(bar.left() + size.x * 0.5, bar.bottom() - size.y * 0.5),
                size,
            )
        } else {
            slice_h(bar, 0.00, 0.28)
        };
        if rolling {
            self.timecode = head;
        } else {
            self.tempo = head;
        }
        // **The buttons start where the knob's CELL ends**, not at a fraction
        // of the row. The knob's ticks stand outside its body but inside its
        // cell, so a gap measured from the row left a hole that looked like
        // something was missing from it.
        let buttons = Rect::from_min_max(
            Pos2::new(head.right() + bar.width() * 0.02, bar.top()),
            bar.max,
        );
        // **Sized and lined up against the KNOB, not against the row.** They
        // are one group with it, and a group whose parts are measured from
        // different things is a group that only looks like one by accident:
        // the knob's circle hangs in its own face, under a label the buttons
        // do not have, so a row-centred button sat visibly below it.
        //
        // Their side comes off the knob's body diameter — the thing you see —
        // and smaller than it, because the transport's job is Record and the
        // tempo is what you set once.
        let (line, from_knob) = knob_centre(head).map_or((buttons.center().y, None), |c| {
            (c.y, knob_face(head).map(|k| k.radius * 2.0))
        });
        let side = from_knob
            .map_or(buttons.height() * TRANSPORT_SIDE, |d| d * TRANSPORT_SIDE)
            .min(buttons.width() * 0.26);
        let icon = |i: usize| {
            let cx = buttons.left() + buttons.width() * (i as f32 * 2.0 + 1.0) / 6.0;
            Rect::from_center_size(Pos2::new(cx, line), Vec2::splat(side))
        };
        // The typing box, under the knob and only ever drawn while somebody is
        // typing into it. Allowed to hang below the column: it is an overlay
        // for as long as a number takes to enter, and the alternative is a
        // permanent gap under the transport reserved for nothing.
        self.tempo_entry = Rect::from_min_size(
            Pos2::new(self.tempo.center().x - self.tempo.width() * 0.44, self.tempo.bottom()),
            Vec2::new(self.tempo.width() * 0.88, self.tempo.height() * 0.42),
        );
        if rolling {
            // No record button while rolling — pressing it would mean nothing,
            // and a dead control is worse than no control. The steady dot
            // stands exactly where it stood. No cog either: none of what it
            // opens can be changed with an encoder running.
            self.dot = icon(0);
            self.stop = icon(1);
        } else {
            self.record = icon(0);
            self.stop = icon(1);
            self.setup = icon(2);
        }
    }

    /// The master column: scale, bars, gain reduction, knob.
    ///
    /// Laid out from the BOTTOM: the knob is a fixed share and the meter takes
    /// whatever is left, because a knob that shrinks with the window stops
    /// being grabbable long before a meter stops being readable.
    fn fill_master(&mut self, m: Rect, knob_size: Vec2) {
        if !m.is_positive() || !(knob_size.x > 0.0 && knob_size.y > 0.0) {
            return;
        }
        // The knob sits at the foot of the column, the same size as an effect
        // knob, **centred between the two ladders rather than on the column**.
        // The column carries the scale down its left-hand side, so its centre
        // is not the meters' centre — and a knob that belongs to the pair
        // above it has to line up with the pair and not with the margin.
        let bars = slice_h(m, MASTER_SCALE_W + 0.03, 1.0 - MASTER_SCALE_W - 0.03);
        let knob = Rect::from_center_size(
            Pos2::new(bars.center().x, m.bottom() - knob_size.y * 0.5),
            Vec2::new(knob_size.x.min(m.width()), knob_size.y),
        );
        if !knob_fits(knob) {
            return;
        }
        // **Straight down to the knob.** There was a readout strip in here —
        // a recess with the output level in it, which is where the reduction
        // used to be drawn. The reduction reads against the scale now, and
        // what the strip was holding open was ladder.
        let meter = Rect::from_min_max(m.min, Pos2::new(m.right(), knob.top() - 4.0));
        if !meter.is_positive() {
            return;
        }
        self.master_knob = knob;
        // Left to right: the numbers, the two bars, then the reduction.
        // **The scale is between nothing and everything** — it is read against
        // the bars, so it goes next to them and not off in a corner.
        // **One split meter, not two.** The pair used to be most of the
        // column and read as two separate bars that happened to be adjacent;
        // narrow and close together they read as one stereo meter with a seam
        // down it, which is what a master is.
        self.master_scale = [
            slice_h(meter, 0.00, MASTER_SCALE_W),
            slice_h(meter, 1.0 - MASTER_SCALE_W, 1.0),
        ];
        self.master_bars = slice_h(meter, MASTER_SCALE_W + 0.03, 1.0 - MASTER_SCALE_W - 0.03);
    }

    fn fill_transport(&mut self, t: Rect) {
        if !t.is_positive() {
            return;
        }
        // **Two columns, and the right one is stacked.** The record button,
        // the stop and the clock live at the foot of the words column; the
        // faders have the left of this group; and the right of it is the
        // meters with the three effect knobs in a row underneath.
        //
        // The knobs were a stack in a column ten points wide, squeezed between
        // the faders and the meters, and at that size the word over each one
        // did not fit and the dial itself was a smudge. Across the bottom of
        // the meter column each of them is a third of three hundred points
        // instead — the same three controls, three times the diameter.
        // **The master takes the right edge of the transport.** It is the end
        // of the signal path and it is where the eye already goes last, which
        // is the same reason it is on the right of every desk ever made.
        let right = slice_h(t, FADER_COL + 0.04, MASTER_COL);
        let master_col = slice_h(t, MASTER_COL + 0.02, 1.00);
        self.meter = slice_v(right, 0.0, METER_SHARE);
        let knobs = slice_v(right, METER_SHARE + 0.03, 1.0);
        // Side by side, one third each, with the gap inside the cell: the
        // label is centred over the dial and `draw_knob` centres the pair in
        // whatever it is handed.
        // **Two rows of three.** Three sends on top, then the three that
        // shape the whole output: a high-pass, a low-pass and the limiter that
        // is always the last thing in the chain. Six across one row would put
        // every knob below the size `knob_fits` will accept at any window this
        // app opens at.
        // **The gap between the rows is bigger than it looks like it needs to
        // be**, because a knob is bigger than its cell: the tick marks stand
        // out to 1.26 of the face radius and are drawn OUTSIDE the rectangle
        // the face was measured in (see `draw_knob`). At a four percent gap
        // the word HPF landed on the reverb knob's skirt.
        let row = |top: f32, bottom: f32| slice_v(knobs, top, bottom);
        let (top, bottom) = (row(0.00, 0.45), row(0.55, 1.00));
        let cells = [
            slice_h(top, 0.00, 0.32),
            slice_h(top, 0.34, 0.66),
            slice_h(top, 0.68, 1.00),
            slice_h(bottom, 0.00, 0.32),
            slice_h(bottom, 0.34, 0.66),
            slice_h(bottom, 0.68, 1.00),
        ];
        // **A control nobody can see is not a control.** `draw_knob` refuses a
        // cell too small to be a knob rather than drawing a smudge, so the
        // layout has to refuse it too — otherwise the band keeps a live drag
        // target over blank panel. One predicate, asked by both.
        //
        // All six or none: five knobs where there should be six is worse than
        // none, because the missing one is the one somebody goes looking for.
        if cells.iter().copied().all(knob_fits) {
            self.fx = cells;
        }
        // **After the effect knobs, because it is measured against one.** The
        // master is the same control as the six beside it and has to be the
        // same size; taking a share of its own column instead would make it
        // half again as big, which reads as a different kind of thing.
        self.fill_master(master_col, self.fx[0].size());
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


    /// Rolling: readable from the bench. The preview collapses to a strip that
    /// is enough to see somebody has walked in front of the camera and no more,
    /// the two things worth reading from two metres away take most of the rest,
    /// and the monitor column stays exactly where it was.
    fn fill_rolling(&mut self, body: Rect, gap: f32, view: &RecorderView<'_>) {
        // **The same preview, at the same size**, placed in `new` for both
        // layouts. It used to collapse to a strip when a take started, back
        // when it was a framing check. It is the camera inset the recording
        // will carry now, so the moment a take starts is the moment it matters
        // most — and the band does not rearrange itself under the eye either
        // way.
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
        // **Transport first, faders second**, the same way round as at rest.
        // The tempo knob is sized from the effect knobs, and those are placed
        // in `fill_transport` — so asking for them the other way round would
        // measure against a rectangle that is still `NOTHING`.
        self.fill_transport(t);
        self.fill_faders(body, gap, true);

        // **The identical band, with the record button become the dot.** The
        // transport sits at the foot of the faders in both layouts, so the
        // stop button is under the same pixel at 0:00 and at 4:12 — see
        // `fill_faders`, which places both. What leaves is the tempo, which
        // cannot be changed mid-take; the clock takes its box.

        // The one thing `hide_elapsed` suppresses is a CLOCK. The count-in beat
        // is the number the player is counting, and "FINISHING" is the reason
        // not to close the lid yet; hiding either would be hiding the wrong
        // thing under the name of a performance setting.
        if view.hide_elapsed && matches!(view.state, RecordState::Rolling) {
            self.timecode = Rect::NOTHING;
        }
    }

    /// Every clickable region and what it means, in one place.
    ///
    /// [`hit_test`] reads this and so does the test that proves no two of them
    /// overlap, so a control that moves onto another one fails a test rather
    /// than quietly swallowing its clicks.
    fn targets(&self) -> [(Rect, Produces); 61] {
        use Produces::{Along, AlongV, Fixed, SlotGain};
        let track = |row: Rect| fader_zones(row).1;
        // The dB at the end of a fader row, which is the box you type into.
        let reading = |row: Rect| fader_zones(row).2;
        // The word over a knob, which is the same thing for a dial.
        // **Nothing when there is no word.** Below a certain height the label
        // is not drawn at all (see `knob_face`), and a zero-height target is
        // still a target as far as an overlap check is concerned.
        // A GAP under it, the way every other pair of targets in this layout
        // is separated: two rectangles that share an edge are two rectangles a
        // press on that edge can land in either of.
        let cap = |cell: Rect| {
            knob_face(cell)
                .filter(|f| f.label_h > 0.0)
                .map_or(Rect::NOTHING, |f| {
                    Rect::from_min_max(
                        cell.min,
                        Pos2::new(cell.right(), cell.top() + f.label_h * 0.88),
                    )
                })
        };
        // **What is left of the cell once the word is taken out of it.** The
        // whole cell used to be the drag target, on the reasoning that a knob
        // you can only grab by its own diameter is one you keep missing — and
        // that is still true of the part BELOW the word. The word is a target
        // of its own now, so the two cannot both have it.
        let dial = |cell: Rect| {
            knob_face(cell).map_or(Rect::NOTHING, |f| {
                Rect::from_min_max(Pos2::new(cell.left(), cell.top() + f.label_h), cell.max)
            })
        };
        let s = &self.slots;
        // **The numbers come FIRST.** A label sits inside its knob's cell and
        // a reading beside its fader's track; both are scanned before the
        // control they belong to, so the smaller, more specific target wins.
        [
            (reading(self.metronome_row), Fixed(Hit::Type(NumField::Metronome))),
            (reading(self.input_row), Fixed(Hit::Type(NumField::Input))),
            (reading(self.track_row), Fixed(Hit::Type(NumField::Track))),
            (s[0].value, Fixed(Hit::Type(NumField::Slot(0)))),
            (s[1].value, Fixed(Hit::Type(NumField::Slot(1)))),
            (s[2].value, Fixed(Hit::Type(NumField::Slot(2)))),
            (s[3].value, Fixed(Hit::Type(NumField::Slot(3)))),
            (s[4].value, Fixed(Hit::Type(NumField::Slot(4)))),
            (cap(self.tempo), Fixed(Hit::Type(NumField::Tempo))),
            (cap(self.master_knob), Fixed(Hit::Type(NumField::Master))),
            (cap(self.fx[0]), Fixed(Hit::Type(NumField::Fx(Fx::Reverb)))),
            (cap(self.fx[1]), Fixed(Hit::Type(NumField::Fx(Fx::Delay)))),
            (cap(self.fx[2]), Fixed(Hit::Type(NumField::Fx(Fx::Chorus)))),
            (cap(self.fx[3]), Fixed(Hit::Type(NumField::Fx(Fx::Hpf)))),
            (cap(self.fx[4]), Fixed(Hit::Type(NumField::Fx(Fx::Lpf)))),
            (cap(self.fx[5]), Fixed(Hit::Type(NumField::Fx(Fx::Limiter)))),
            // The microphone is its input's picker, on the same terms as the
            // preview below: the device belongs to the control it feeds, and
            // it is the icon at the head of the fader it feeds.
            (
                if self.rolling { Rect::NOTHING } else { self.input_icon },
                Fixed(Hit::PickAudio),
            ),
            // **The picture is the picker.** Not while a take is rolling: the
            // camera cannot be changed mid-take, and the dialog it opens is
            // modal over the one gesture that matters then, which is Stop.
            (
                if self.rolling { Rect::NOTHING } else { self.preview },
                Fixed(Hit::PickCamera),
            ),
            // **The clip latch is cleared by pressing the meter it is shown
            // on.** There was a word `CLIPPED` in the status line to press
            // instead; the lamp on the VU face says the same thing in the
            // place somebody is already looking, and two indicators for one
            // fact is one of them being wrong.
            (self.meter, Fixed(Hit::DismissClip)),
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
            (track(self.track_row), Along(Hit::SetTrackGain)),
            // The icon is the import button. A left click on it opens the
            // file dialog; a right click opens the waveform, the same way the
            // microphone's icon opens the audio picker.
            //
            // **Not while a take is rolling.** Swapping the backing track
            // half way through a performance is not a thing anybody means to
            // do, and the dialog it opens is modal — it would stop the one
            // gesture that matters at that moment, which is Stop. The LEVEL
            // stays live, like every other fader.
            (
                if self.rolling { Rect::NOTHING } else { self.track_icon },
                Fixed(Hit::ImportTrack),
            ),
            // Up the cell for more, which is the direction every knob in every
            // studio turns and the direction the two faders beside them move.
            (dial(self.master_knob), AlongV(Hit::SetMaster)),
            (dial(self.fx[0]), AlongV(|v| Hit::SetFx(Fx::Reverb, v))),
            (dial(self.fx[1]), AlongV(|v| Hit::SetFx(Fx::Delay, v))),
            (dial(self.fx[2]), AlongV(|v| Hit::SetFx(Fx::Chorus, v))),
            (dial(self.fx[3]), AlongV(|v| Hit::SetFx(Fx::Hpf, v))),
            (dial(self.fx[4]), AlongV(|v| Hit::SetFx(Fx::Lpf, v))),
            (dial(self.fx[5]), AlongV(|v| Hit::SetFx(Fx::Limiter, v))),
            (self.click, Fixed(Hit::ToggleMetronome)),
            (self.dest, Fixed(Hit::ChooseFolder)),
            (self.reveal, Fixed(Hit::RevealFolder)),
            (self.open_when_done, Fixed(Hit::ToggleOpenWhenDone)),
            (self.count_in, Fixed(Hit::CycleCountIn)),
            // Turned like the sends, and typed into on a DOUBLE click — see
            // `draw_tempo`. `SetTempo` carries beats rather than 0..=1, so the
            // producer converts on the way out.
            (dial(self.tempo), AlongV(|t| Hit::SetTempo(knob_to_tempo(t)))),
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
    /// The take-settings popup's DONE button. It only ever closes the popup,
    /// which is why it is a hit here and not an action anywhere else.
    CloseSetup,
    /// Reachable only from the popup: the four rows below have no home in the
    /// band, and a control that exists in exactly one place needs no rule about
    /// which place wins.
    ShowAudioStatus,
    ToggleCountInInTake,
    ToggleHideElapsed,
    Record,
    Stop,
    ChooseFolder,
    /// Show the destination folder in the file manager.
    RevealFolder,
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
    /// The three effect sends, 0..=1.
    /// Open a field for typing, and nothing else.
    ///
    /// **A gesture of its own, on a target of its own.** Dragging, resetting
    /// and typing are three things a control has to do and a press has only so
    /// many meanings: the drag is the handle, the double click is the reset,
    /// and typing is the NUMBER — the dB at the end of a fader, the word over
    /// a knob. Pressing what the value is written on to change it is the one
    /// mapping nobody has to be told.
    Type(NumField),
    /// The backing track's level, as a fader position.
    SetTrackGain(f32),
    /// Choose an audio file to play along to.
    ImportTrack,
    /// Put every clip latch out. **The VU face is the target.**
    ///
    /// A latch that clears itself is one the performer never sees, because
    /// they were looking at their hands. A latch with no way to clear it is
    /// one that stays lit for the rest of the session and stops meaning
    /// anything — the same failure, slower. It was a word in the status line
    /// and is the meter now: one indicator, pressed where it is shown.
    DismissClip,
    /// The master, as a FADER POSITION 0..=1 — not a gain. Same curve as the
    /// four faders, because it is one; it just wears a knob.
    SetMaster(f32),
    /// One of the six effect knobs, 0..=1. **One variant, not six**: they are
    /// the same control six times over, and six parallel variants meant six
    /// parallel arms in every match that ever touches one.
    SetFx(Fx, f32),
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

    /// The same control, carrying `v` instead.
    ///
    /// For a caller that computed the value itself rather than reading it off
    /// a position — which is what a relative drag does. Anything that carries
    /// no 0..=1 value comes back unchanged.
    #[must_use]
    pub fn with_value(self, v: f32) -> Hit {
        let v = v.clamp(0.0, 1.0);
        match self {
            Hit::SetFx(fx, _) => Hit::SetFx(fx, v),
            Hit::SetMaster(_) => Hit::SetMaster(v),
            Hit::SetMetronomeGain(_) => Hit::SetMetronomeGain(v),
            Hit::SetInputGain(_) => Hit::SetInputGain(v),
            Hit::SetTrackGain(_) => Hit::SetTrackGain(v),
            Hit::SetSlotGain(i, _) => Hit::SetSlotGain(i, v),
            // **The tempo, which was missing.** Every other knob's drag goes
            // through here to turn "how far the hand has moved" into a value;
            // without an arm the tempo fell to `other => other` and every
            // frame of a drag re-applied the hit the PRESS produced — which is
            // absolute, so the knob jumped to wherever it was first touched
            // and then would not move. It has never been relatively draggable;
            // the other seven becoming consistent is what made it obvious.
            Hit::SetTempo(_) => Hit::SetTempo(knob_to_tempo(v)),
            other => other,
        }
    }

    /// Every control, which is what the reachability test iterates. The
    /// exhaustive match in [`Hit::label`] is what makes adding a variant
    /// without adding it here a compile error rather than an untested control.
    ///
    /// The four per-slot controls appear once per slot, because "reachable" is
    /// a question about slot 2 that slot 0 cannot answer for it.
    pub const ALL: [Hit; 48] = [
        Hit::Record,
        Hit::Stop,
        Hit::OpenSetup,
        Hit::ChooseFolder,
        Hit::RevealFolder,
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
        Hit::SetMaster(Hit::MIDWAY),
        Hit::SetTrackGain(Hit::MIDWAY),
        Hit::ImportTrack,
        Hit::Type(NumField::Tempo),
        Hit::DismissClip,
        Hit::SetFx(Fx::Reverb, Hit::MIDWAY),
        Hit::SetFx(Fx::Delay, Hit::MIDWAY),
        Hit::SetFx(Fx::Chorus, Hit::MIDWAY),
        Hit::SetFx(Fx::Hpf, Hit::MIDWAY),
        Hit::SetFx(Fx::Lpf, Hit::MIDWAY),
        Hit::SetFx(Fx::Limiter, Hit::MIDWAY),
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
            Hit::CloseSetup => "Done",
            Hit::ShowAudioStatus => "Setup: the audio system, devices, rate and buffer",
            Hit::ToggleCountInInTake => "Record the count-in into the take",
            Hit::ToggleHideElapsed => "Hide the elapsed time",
            Hit::Stop => "Stop",
            Hit::ChooseFolder => "Choose folder",
            Hit::RevealFolder => "Show the folder",
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
            Hit::SetFx(fx, _) => fx.describe(),
            Hit::SetMaster(_) => "Master  -  right-click to type, double-click for 0 dB",
            Hit::DismissClip => "Levels  -  click to clear the clip lamp",
            Hit::SetTrackGain(_) => "Backing track level",
            Hit::Type(_) => "Click to type a number",
            Hit::ImportTrack => "Backing track  -  click to choose a file",
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
                | Hit::SetFx(..)
                | Hit::SetMaster(_)
                | Hit::SetInputGain(_)
                | Hit::SetTrackGain(_)
                | Hit::SetTempo(_)
        )
    }

    /// Whether this is one of the eight knobs.
    ///
    /// They share a gesture set the faders do not: a right-click types a
    /// number into one, a double-click puts it back to what it ships as, and a
    /// plain tap does NOTHING — because the first click of a double-click is a
    /// tap, and a control that opened a text box under every double-click
    /// could never be reset by one.
    pub fn is_knob(self) -> bool {
        matches!(self, Hit::SetFx(..) | Hit::SetTempo(_) | Hit::SetMaster(_))
    }

    /// What a double-click puts this control back to.
    ///
    /// **Nothing applied, for everything that applies something.** All six
    /// effects go to zero, which for the filters is a corner out at the edge
    /// of hearing and for the limiter is a threshold of 0 dB — in every case
    /// the position where the knob is not doing anything.
    ///
    /// Everything that is a LEVEL goes to what it ships as instead, which is
    /// unity for the four that carry sound and -6 dB for the click. "Nothing
    /// applied" for a fader would be silence, and a reset that mutes the thing
    /// you were listening to is a reset nobody uses twice.
    pub fn reset_to(self) -> Option<Hit> {
        let ships_at = |g: f32| gain_to_fader(g);
        let d = crate::recorder::Gains::default();
        Some(match self {
            Hit::SetSlotGain(i, _) => {
                Hit::SetSlotGain(i, ships_at(d.slots.get(i).copied().unwrap_or(1.0)))
            }
            Hit::SetMetronomeGain(_) => Hit::SetMetronomeGain(ships_at(d.metronome)),
            Hit::SetInputGain(_) => Hit::SetInputGain(ships_at(d.inputs[0])),
            Hit::SetTrackGain(_) => Hit::SetTrackGain(ships_at(d.track)),
            // **The limiter is the exception and it is not an inconsistency.**
            // Its knob is a threshold, so "not applied" is fully clockwise —
            // above everything, catching nothing — where every other knob's
            // "not applied" is fully anticlockwise.
            Hit::SetFx(Fx::Limiter, _) => Hit::SetFx(Fx::Limiter, 1.0),
            Hit::SetFx(fx, _) => Hit::SetFx(fx, 0.0),
            Hit::SetTempo(_) => Hit::SetTempo(DEFAULT_BPM),
            Hit::SetMaster(_) => Hit::SetMaster(ships_at(d.master)),
            _ => return None,
        })
    }

    /// Which control this is, for [`Hit::is_same_control`]: the variant, plus
    /// the index for the ones that carry one.
    ///
    /// **The index is half the answer.** `mem::discriminant` alone says
    /// `SetSlotGain(0, _)` and `SetSlotGain(1, _)` are the same control, and a
    /// caller that believes it is still dragging the knob it grabbed would then
    /// set slot 1's level from a drag that started on slot 0's knob — silently,
    /// and only once the pointer wandered a row.
    ///
    /// `SetFx` is the same trap and it arrived the same way: six knobs that
    /// used to be six variants became one variant carrying which, and a
    /// discriminant cannot see the difference between the reverb and the
    /// limiter. Grabbing REVERB and dragging down onto HPF would have set the
    /// high-pass.
    fn control_key(self) -> (std::mem::Discriminant<Hit>, usize) {
        let index = match self {
            Hit::PickSlot(i)
            | Hit::OpenSlotEditor(i)
            | Hit::ClearSlot(i)
            | Hit::SetSlotGain(i, _) => i,
            Hit::SetFx(fx, _) => fx.index(),
            // Two `Type`s are two controls: the dB at the end of slot 1 and
            // the dB at the end of slot 2 are not the same box.
            Hit::Type(f) => num_field_key(f),
            _ => 0,
        };
        (std::mem::discriminant(&self), index)
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

/// A number for each field, so `control_key` can tell two `Type`s apart.
fn num_field_key(f: NumField) -> usize {
    match f {
        NumField::Slot(i) => i,
        NumField::Metronome => 100,
        NumField::Input => 101,
        NumField::Master => 102,
        NumField::Track => 103,
        NumField::Tempo => 104,
        NumField::Meter => 105,
        NumField::TrackIn => 106,
        NumField::TrackOut => 107,
        NumField::Fx(fx) => 200 + fx.index(),
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
        // The knob IS the box: a double click on it opens the field. There is
        // no separate control to press.
        Hit::SetTempo(_) => Some(NumField::Tempo),
        Hit::SetFx(fx, _) => Some(NumField::Fx(fx)),
        Hit::SetMaster(_) => Some(NumField::Master),
        Hit::SetInputGain(_) => Some(NumField::Input),
        Hit::SetTrackGain(_) => Some(NumField::Track),
        // **The one that opens a field IS a field.** Everything else here
        // reports which field it would be typed into if somebody asked; this
        // one is the asking.
        Hit::Type(f) => Some(f),
        // NOT `SetTempo`: that carries a committed value and has no box of
        // its own any more. `EditTempo` is the box.
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
    /// The same, measured UP the rect: 0 at the bottom, 1 at the top. For
    /// round knobs, where left-to-right would be a control that goes the wrong
    /// way half the time depending on which side of it you grabbed.
    AlongV(fn(f32) -> Hit),
    /// The same, for a knob that also has to say which slot it belongs to. Its
    /// own variant rather than a closure so that [`Layout::targets`] stays a
    /// plain `Copy` array with no allocation and no lifetime.
    SlotGain(usize),
}

impl Produces {
    /// The hit for a press at `pos` inside `r`.
    ///
    /// Takes the rect and the point rather than a fraction, because the two
    /// travelling controls measure different axes and a caller that computed
    /// the fraction would have to know which — the one thing this enum exists
    /// to keep it from knowing.
    fn hit(self, r: Rect, pos: Pos2) -> Hit {
        match self {
            Produces::Fixed(h) => h,
            Produces::Along(f) => f(along(r, pos)),
            Produces::AlongV(f) => f(up(r, pos)),
            Produces::SlotGain(i) => Hit::SetSlotGain(i, along(r, pos).clamp(0.0, 1.0)),
        }
    }

    /// A representative hit, for asking a producer WHICH CONTROL it is.
    ///
    /// The value it carries is meaningless and every caller ignores it:
    /// `label` and `is_same_control` are both about identity. A unit rect at
    /// the origin, so the answer does not depend on where anything is.
    fn control(self) -> Hit {
        self.hit(Rect::from_min_size(Pos2::ZERO, Vec2::splat(1.0)), Pos2::ZERO)
    }
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

/// Which way a control travels.
///
/// A caller dragging one has to hold the OTHER axis still — see
/// [`drag_axis`] — or a fader loses the gesture the moment the hand drifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAxis {
    Horizontal,
    Vertical,
}

/// Which way the control under `hit` travels, if it travels at all.
///
/// **Read off the same table [`hit_test`] uses**, rather than matched on the
/// `Hit` variants here. A second list would be a second thing to update, and
/// the failure it produces is a knob that cannot be dragged at all: the caller
/// pins the axis the control actually moves along, and every pointer position
/// reports the same value.
pub fn drag_axis(rect: Rect, view: &RecorderView<'_>, hit: Hit) -> Option<DragAxis> {
    Layout::new(rect, view)
        .targets()
        .into_iter()
        .find(|(r, k)| r.is_positive() && (*k).control().is_same_control(hit))
        .and_then(|(_, k)| match k {
            Produces::Along(_) | Produces::SlotGain(_) => Some(DragAxis::Horizontal),
            Produces::AlongV(_) => Some(DragAxis::Vertical),
            Produces::Fixed(_) => None,
        })
}

/// How far the pointer must move to sweep `hit` end to end, in points.
///
/// **Every control here is dragged RELATIVELY**, knobs and faders alike: the
/// value follows how far the hand has moved rather than where it ended up. A
/// fader's travel is its own track, so it still feels one-to-one under the
/// pointer; what it gains is that the hand can leave the track and keep going,
/// and that pressing it never makes the handle jump.
///
/// `None` for anything that does not travel.
pub fn drag_travel(rect: Rect, view: &RecorderView<'_>, hit: Hit) -> Option<f32> {
    let l = Layout::new(rect, view);
    let (_, k) = l
        .targets()
        .into_iter()
        .find(|(r, k)| r.is_positive() && (*k).control().is_same_control(hit))?;
    match k {
        Produces::AlongV(_) => Some(KNOB_TRAVEL),
        Produces::Along(_) | Produces::SlotGain(_) => Some(FADER_TRAVEL),
        Produces::Fixed(_) => None,
    }
}

/// How far the pointer must move to sweep a knob end to end, in points.
///
/// **Wider than the knob, by a lot.** A knob's cell is a few tens of points;
/// mapping that to the whole range is a control nobody can land on a number
/// with. The hand can leave the knob, leave the band, and go on turning.
pub const KNOB_TRAVEL: f32 = 260.0;

/// How far the pointer must move to sweep a FADER end to end, in points.
///
/// **Wider than the fader, and by more than a knob is.** A fader used to
/// travel its own track — one-to-one, which feels direct and is the reason
/// half its numbers could not be reached: seventy-two decibels across two
/// hundred points is a third of a decibel per point, and it reads to a tenth.
/// Any value a control can DISPLAY has to be reachable with the mouse alone,
/// so the travel is the number that makes a tenth of a decibel one point.
///
/// A big move is still a big move: -60 to unity is most of a screen. What
/// makes that acceptable is that neither of the two ways of getting there in a
/// hurry goes through the drag — a double click resets it, and the dB at the
/// end of the row can be typed into.
pub const FADER_TRAVEL: f32 =
    (crate::recorder::GAIN_MAX_DB - crate::recorder::GAIN_MIN_DB) * 10.0;

/// How much finer a drag is with the fine modifier held.
///
/// **Any readable value has to be reachable by hand.** A fader spans seventy-
/// two decibels and reads to a tenth of one; over its own track that is four
/// tenths of a decibel per point, so half the numbers it can display cannot be
/// landed on. Six times finer puts every one of them within reach without
/// making the ordinary gesture sluggish.
pub const FINE_DRAG: f32 = 1.0 / 6.0;

/// How far UP `r` the point `pos` fell, 0..=1 from its bottom edge to its top.
///
/// Screen y grows downward and knobs do not, so this is [`along`] inverted on
/// the other axis. Clamped for the same reason: both ends have to be reachable.
fn up(r: Rect, pos: Pos2) -> f32 {
    if r.height() <= 0.0 {
        return 0.0;
    }
    ((r.bottom() - pos.y) / r.height()).clamp(0.0, 1.0)
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

/// Where a knob is, for a caller that needs to reason about the gesture.
///
/// `None` when the band is too small to draw it — which is also when it must
/// not be draggable. Same predicate the painter uses.
pub fn knob_rect(rect: Rect, view: &RecorderView<'_>, hit: Hit) -> Option<Rect> {
    let l = Layout::new(rect, view);
    let r = match hit {
        Hit::SetFx(fx, _) => l.fx[fx.index()],
        Hit::SetMaster(_) => l.master_knob,
        Hit::SetTempo(_) => l.tempo,
        _ => return None,
    };
    r.is_positive().then_some(r)
}

/// The microphone icon's rectangle.
///
/// **Read directly rather than through `hit_test`, and that is the point.**
/// The icon's entry in `targets` is `Rect::NOTHING` while a take is rolling,
/// so that nobody swaps the input device out from under a running encoder. A
/// right-click on the same pixels toggles live MONITORING, which is not a
/// device change — it is listen-only, and checking your feed mid-take is
/// exactly when somebody wants it. So that gesture reads the rectangle here
/// and is not gated by a rule belonging to the other one.
///
/// (The doc used to say "for the right-click that opens the audio picker".
/// That became a LEFT click in 4.19.0 and the comment did not follow.)
pub fn input_icon(rect: Rect, view: &RecorderView<'_>) -> Option<Rect> {
    let r = Layout::new(rect, view).input_icon;
    r.is_positive().then_some(r)
}

/// The camera preview, for the right-click that opens the camera picker.
///
/// The device belongs to the picture it fills. It was a row in the take
/// settings, next to where the file goes and how long the count-in is, which
/// is a list of things about a TAKE — a camera is not one of them.
pub fn preview_rect(rect: Rect, view: &RecorderView<'_>) -> Option<Rect> {
    let r = Layout::new(rect, view).preview;
    r.is_positive().then_some(r)
}

/// The backing track's icon: a left click imports, a right click opens the
/// waveform.
pub fn track_icon(rect: Rect, view: &RecorderView<'_>) -> Option<Rect> {
    let r = Layout::new(rect, view).track_icon;
    r.is_positive().then_some(r)
}

/// The click fader's whole row, for a caller reasoning about the gesture.
pub fn metronome_row(rect: Rect, view: &RecorderView<'_>) -> Option<Rect> {
    let r = Layout::new(rect, view).metronome_row;
    r.is_positive().then_some(r)
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
        .map(|(r, k)| k.hit(r, pos))
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
        draw_cog(painter, l.setup, &p);
        // Over the metronome's own icon, which says what the number is for
        // better than a caption would.
        draw_tempo(
            painter,
            &l,
            view.tempo_bpm,
            typing_for(view, NumField::Tempo),
            view.turning == Some(NumField::Tempo),
            &p,
        );
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
                // "on the right" was true when the camera picker was a box in
                // the middle of the band. It is behind the cog now, and a
                // placeholder pointing at somewhere the control has not been
                // for a release is worse than no placeholder.
                // **The hint is the gesture**, because the box IS the
                // control now: clicking the preview opens the picker. It said
                // "choose one under the cog", which was a direction to
                // somewhere else and stopped being true twice.
                DeviceLabel::None => ("NO CAMERA", "Select Camera"),
                DeviceLabel::Missing(_) => ("CAMERA NOT AVAILABLE", "Select Camera"),
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
                    font_light(size * 0.72),
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
            view.gains.inputs[0],
            NumField::Input,
        ),
        (
            l.track_row,
            FaderIcon::Waveform,
            // Lit only when a file is loaded, so an empty row reads as an
            // offer rather than as a control that does nothing.
            if view.track.is_empty() { p.faint } else { p.ink },
            view.gains.track,
            NumField::Track,
        ),
    ] {
        draw_fader(painter, row, icon, ink, gain, typing_for(view, field), p);
    }
    // The master: what leaves, what the limiter is taking off, and the one
    // control that changes it.
    draw_master(painter, l, view, p);
    if l.master_knob.is_positive() {
        let field = NumField::Master;
        let g = view.gains.master;
        draw_knob(
            painter,
            l.master_knob,
            &Knob {
                value: gain_to_fader(g),
                label: "MASTER",
                typing: typing_for(view, field),
                turning: view.turning == Some(field),
                reading: gain_text(g),
                cap: MASTER_CAP,
            },
            p,
        );
    }
    // The six effect knobs, under the meters: three sends, then the three
    // that shape what leaves.
    for fx in Fx::ALL {
        let field = NumField::Fx(fx);
        let v = view.fx.get(fx);
        draw_knob(
            painter,
            l.fx[fx.index()],
            &Knob {
                value: v,
                label: fx.title(),
                typing: typing_for(view, field),
                turning: view.turning == Some(field),
                reading: knob_reading(view.fx_units[fx.index()], v),
                cap: fx.cap(),
            },
            p,
        );
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
    // **And the microphone, in record red.** Same geometry, same corner, same
    // argument one step louder: a setting that puts a click in your file may
    // not be invisible, and one that puts your microphone through your speakers
    // may not be quiet about it either. Monitoring is how a room feeds back.
    if view.input_monitor && l.input_icon.is_positive() {
        let r = l.input_icon;
        let d = (r.height() * 0.16).clamp(2.0, 5.0);
        painter.circle_filled(Pos2::new(r.right() - d, r.top() + d), d, p.rec);
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
    draw_word_button_sized(painter, r, choices, 0.46, p);
}

/// As [`draw_word_button`], with the text a chosen fraction of the box.
///
/// The fraction is a parameter because one panel wanted to be quieter than the
/// rest rather than because two sizes are a design: the backing track's panel
/// is the widest thing this file draws, so its rows are the tallest, and text
/// sized as a fraction of them came out half again as big as the same control
/// anywhere else. Sizing text off its own box is right until one box is
/// unusually large.
fn draw_word_button_sized(
    painter: &Painter,
    r: Rect,
    choices: &[&str],
    factor: f32,
    p: &Palette,
) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 2.0, p.field);
    painter.rect_stroke(r, 2.0, Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
    let (text, size) = fit_label(r, choices, r.height() * factor);
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
    /// The backing track: three bars of a waveform, which is the only thing
    /// this row could be a picture of.
    Waveform,
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
        FaderIcon::Waveform => draw_waveform_icon(painter, b, ink),
    }
}

/// A waveform: five bars about a centre line, tall in the middle.
///
/// **Bars and not a drawn curve.** At twelve points across, a curve is a
/// wobble; the bars survive being small, and they are what the row below the
/// microphone is a picture of.
pub(crate) fn draw_waveform_icon(painter: &Painter, b: Rect, ink: Color32) {
    let n = 5;
    let w = b.width() / (n as f32 * 2.0 - 1.0);
    // Tallest in the middle, so it reads as a sound and not as a bar chart.
    let heights = [0.34_f32, 0.72, 1.0, 0.58, 0.28];
    for (i, h) in heights.iter().enumerate() {
        let x = b.left() + i as f32 * w * 2.0;
        let half = b.height() * 0.5 * h;
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x, b.center().y - half),
                Pos2::new(x + w, b.center().y + half),
            ),
            0.0,
            ink,
        );
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
pub(crate) fn draw_microphone(painter: &Painter, b: Rect, ink: Color32) {
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
    let (reading, size) = fader_reading(r, gain);
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

/// The gain reading a box can actually hold, and the size to draw it at.
///
/// **The unit is the first thing to go.** "+12.0 dB" is eight characters, and
/// in the fader column of a band at the smallest window this app opens they
/// come out at three points — which is not a number, it is a grey mark where a
/// number should be. Dropping " dB" is worth 60% more height for the digits,
/// and the digits are the reading; a fader with a dB scale printed the length
/// of it does not need the unit repeated at the end. It is only dropped when
/// the full form will not do, so nothing changes at any ordinary size.
///
/// Shared with the test that proves the smallest band still reads, so the two
/// cannot disagree about what is actually drawn.
fn fader_reading(r: Rect, gain: f32) -> (String, f32) {
    let full = gain_text(gain);
    let size = fit_text(r, &full, r.height() * FADER_TEXT);
    if size >= MIN_TEXT {
        return (full, size);
    }
    let short = full.strip_suffix(" dB").map(str::to_owned).unwrap_or(full);
    let size = fit_text(r, &short, r.height() * FADER_TEXT);
    (short, size)
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
/// **Every mark carries its number**, the way the face on the desk does.
///
/// Three labels was the wrong answer to real crowding: the crowding came from
/// printing "-20" and "-10" — a minus sign in front of every number on the
/// left two thirds of a scale, eating the width the digits needed. A real VU
/// prints the numbers BARE and puts one `−` at the left end and one `+` at the
/// right, which is what `draw_vu` does, and ten of them fit with room over.
///
/// The positions are the reference face's, not a formula's: the bottom of a VU
/// scale is compressed and the top is not, and no smooth curve gets 20 and 10
/// as close as they really are while leaving 0 to +3 as open as it really is.
const VU_MARKS: [(f32, f32, bool); 10] = [
    (-20.0, 0.00, true),
    (-10.0, 0.20, true),
    (-7.0, 0.30, true),
    (-5.0, 0.38, true),
    (-3.0, 0.48, true),
    (-1.0, 0.58, true),
    (0.0, 0.64, true),
    (1.0, 0.76, true),
    (2.0, 0.87, true),
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
/// A VU window's width over its height.
///
/// Measured off the ISA One on the owner's desk: the glass is a hair under
/// five by three. That much width is not decoration — it is what puts ten
/// numbers along the arc without them touching, which is the difference
/// between a scale you read and a scale you infer. The face is fitted to this
/// and CENTRED in whatever box it was given, rather than stretched to fill it.
const VU_ASPECT: f32 = 1.65;

/// Half the needle's sweep, in radians.
///
/// **±56°, measured off the reference face**, not the ±46° this had. The two
/// end ticks sit 275 and 200 pixels out from the hub, which is 54° and 58° off
/// vertical. A narrower sweep drew the same arc into the middle of the glass
/// and left a finger of empty card down each side — the face looked oversized
/// for its own scale, which is the tell that the geometry is invented.
const VU_SWEEP: f32 = 0.97;

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
    // **Whichever of the two runs out first sets BOTH.** Capping the width at
    // the height's worth and then drawing the face down the full height is how
    // a landscape meter came out portrait and cut off: the aspect was written
    // down, the height ignored it, and the arc ran off the top of a tall card.
    let cell = (r.width() - gap * (n - 1.0)) / n;
    let fh = r.height().min(cell / VU_ASPECT);
    let fw = fh * VU_ASPECT;
    if fw <= 0.0 || fh <= 0.0 {
        return;
    }
    let top = r.center().y - fh * 0.5;
    let mut left = r.center().x - (fw * n + gap * (n - 1.0)) * 0.5;
    for lv in faces {
        let face = Rect::from_min_size(Pos2::new(left, top), Vec2::new(fw, fh));
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
    // The hub sits just inside the bottom edge, the way the chrome half-moon
    // does on the real one, and the arc is as big as the shorter of the two
    // dimensions allows. The width term leaves a card's-width margin at each
    // end rather than two points, so the scale ends inside the glass.
    // **The hub is BELOW the glass**, the way a real movement's is: what you
    // see is a needle coming up out of the bottom edge, not a pointer pinned
    // to a dot sitting on the card. Everything it draws is clipped to the
    // face, so the part below the edge simply is not there.
    let pivot = Pos2::new(face.center().x, face.bottom() + fh * 0.05);
    let radius = (fh * 0.80).min(fw * 0.46 / VU_SWEEP.sin()).max(1.0);
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
    // By VALUE, not by index: the scale gained a mark and an index into it is
    // a thing that goes quietly wrong when it does.
    let red_from = VU_MARKS
        .iter()
        .find(|(vu, ..)| *vu >= 0.0)
        .map_or(1.0, |(_, f, _)| *f);
    arc(0.0, red_from, VU_PRINT, hair);
    arc(red_from, 1.0, p.rec, hair * 1.6);

    // The printed marks, each with its number above it.
    let label = fh >= 40.0 && fw >= 76.0;
    // **The reference face's own proportion.** Its digits are 38 pixels on a
    // 485-pixel face — 7.8% — and that is not a stylistic choice, it is what
    // ten numbers along an arc of that radius will physically take without
    // touching. At 13.5% they overlapped into a smear, which is the same
    // mistake as printing minus signs: spending width the arc does not have.
    let num = fh * 0.085;
    for (vu, f, _) in VU_MARKS {
        let inner = radius * 0.86;
        let colour = if f >= red_from { p.rec } else { VU_PRINT };
        painter.line_segment([at(f, inner), at(f, radius)], Stroke::new(hair, colour));
        if label {
            // **Bare.** No sign in front of any of them — the two signs live
            // at the ends of the scale, which is where a face that has to fit
            // ten numbers along an arc puts them.
            painter.text(
                // Just inside the ticks, which start at 0.86. Further in is
                // a shorter arc and therefore less room, which is the other
                // half of why they were colliding.
                at(f, radius * 0.80),
                Align2::CENTER_CENTER,
                &format!("{:.0}", vu.abs()),
                font(num),
                colour,
            );
        }
    }
    if label {
        // The signs, outboard of the first and last marks and level with them,
        // saying which half of the scale is which. One character each: the
        // whole reason the numbers between them can be bare.
        for (f, sign, colour) in [
            (-0.09_f32, "\u{2212}", VU_PRINT),
            (1.09, "+", p.rec),
        ] {
            painter.text(
                at(f, radius * 0.90),
                Align2::CENTER_CENTER,
                sign,
                font(num * 1.15),
                colour,
            );
        }
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
    // Thin. A VU needle is a wire with a counterweight, and at 3.5% of the
    // face it was a pointer drawn by somebody who had not looked at one.
    let needle = painter.with_clip_rect(face);
    needle.line_segment([pivot, n], Stroke::new((fh * 0.018).max(1.0), VU_PRINT));
    needle.circle_filled(pivot, (fh * 0.085).max(1.5), VU_PRINT);

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


/// The settings cog: the way into the take-settings popup.
///
/// **Solid, with stubby teeth.** The first cut drew a thin ring and ran eight
/// long thin spokes from it out to the edge, which is a ship's wheel — the
/// owner said so and was right. A gear is a SOLID body whose teeth barely
/// stand proud of it: the teeth are short and wide, the hub is a hole punched
/// through the middle, and there is nothing in between for a spoke to be.
fn draw_cog(painter: &Painter, r: Rect, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    let c = r.center();
    let rad = r.width().min(r.height()) * 0.5;
    // Below this the teeth merge into the body and it is a smudge of a circle,
    // so nothing is drawn at all rather than something that reads as a fault.
    if rad < 5.0 {
        return;
    }
    // The body: most of the circle. The teeth reach from inside it to the
    // edge, so they read as bumps ON it rather than as arms coming OFF it.
    let body = rad * 0.74;
    painter.circle_filled(c, body, p.ink);
    const TEETH: u8 = 8;
    // Wide enough that eight of them nearly meet at the rim, which is what a
    // gear looks like; a thin one is a spoke however short it is.
    let tooth = Stroke::new(rad * 0.34, p.ink);
    for i in 0..TEETH {
        let a = std::f32::consts::TAU * f32::from(i) / f32::from(TEETH);
        let (sn, cs) = a.sin_cos();
        let d = Vec2::new(sn, -cs);
        painter.line_segment([c + d * (body * 0.72), c + d * rad], tooth);
    }
    // The hub, punched through to the band behind it. Big enough to read as a
    // hole at sixteen points, which is the size this is actually drawn at.
    painter.circle_filled(c, rad * 0.30, p.bg);
}

// ── the effect knobs ────────────────────────────────────────────────────────

/// The knob's sweep, in radians, centred on straight up.
///
/// 270 degrees, which is what a panel knob with an end stop turns and what
/// leaves the gap at the bottom that tells you where the ends are.
const KNOB_SWEEP: f32 = std::f32::consts::TAU * 0.75;

/// Tick marks around the skirt. Eleven on the 388, and eleven here.
const KNOB_TICKS: usize = 11;

/// The tempo knob's cap.
///
/// A darkened orange against the effects' blue. They are the same control and
/// deliberately so; the colour is what says this one moves the CLICK rather
/// than the sound.
const TEMPO_CAP: Color32 = Color32::from_rgb(0xb5, 0x5c, 0x18);

/// The blue cap on a Tascam 388's effect returns.
///
/// A real colour off the reference photograph rather than the band's accent,
/// because the whole point of these two controls is that they look like a
/// piece of hardware and not like the rest of the panel.
const KNOB_CAP: Color32 = Color32::from_rgb(0x1c, 0x6f, 0xd6);

/// One knob's number, in whatever unit it is measured in.
///
/// **Hertz rounds the way a person reads a frequency**, not the way a float
/// prints one: whole numbers below a kilohertz, one decimal above, and no
/// trailing `.0` on a round figure. "1.2 kHz" and "480 Hz" are what somebody
/// would say out loud; "1200.0000 Hz" is what the number happens to be.
pub fn knob_reading(unit: KnobUnit, t: f32) -> String {
    let t = t.clamp(0.0, 1.0);
    match unit {
        KnobUnit::Percent => format!("{:.0}%", t * 100.0),
        KnobUnit::Decibels { low, high } => format!("{:.1} dB", low + (high - low) * t),
        KnobUnit::Hertz { low, high } => {
            let hz = low * (high / low).powf(t);
            if hz >= 1_000.0 {
                let k = hz / 1_000.0;
                if (k - k.round()).abs() < 0.05 {
                    format!("{k:.0} kHz")
                } else {
                    format!("{k:.1} kHz")
                }
            } else {
                format!("{hz:.0} Hz")
            }
        }
    }
}

/// The inverse of [`knob_reading`] for a typed number.
///
/// **Only the knob's own unit is accepted.** Somebody typing into a filter is
/// typing a frequency; taking "500" as half travel there would silently set
/// 2 kHz, which is the kind of wrong that looks like the box not working.
pub fn knob_typed(unit: KnobUnit, text: &str) -> Option<f32> {
    match unit {
        KnobUnit::Percent => crate::recorder::parse_percent(text),
        KnobUnit::Decibels { low, high } => {
            let db = text
                .trim()
                .trim_end_matches(|c: char| c.is_whitespace() || c == 'b' || c == 'B')
                .trim_end_matches(['d', 'D'])
                .trim()
                .parse::<f32>()
                .ok()?;
            if !db.is_finite() {
                return None;
            }
            Some(((db - low) / (high - low)).clamp(0.0, 1.0))
        }
        KnobUnit::Hertz { low, high } => {
            let t = text.trim().trim_end_matches(|c: char| {
                c.is_whitespace() || c == 'z' || c == 'Z' || c == 'h' || c == 'H'
            });
            let (t, scale) = match t.strip_suffix(['k', 'K']) {
                Some(rest) => (rest.trim_end(), 1_000.0),
                None => (t, 1.0),
            };
            let hz = t.trim().parse::<f32>().ok()? * scale;
            if !hz.is_finite() || hz <= 0.0 {
                return None;
            }
            // Where that frequency sits on this knob, clamped to its ends: a
            // 40 Hz low-pass is a real wish and the answer is "as low as this
            // one goes", not a refusal.
            Some(((hz / low).log10() / (high / low).log10()).clamp(0.0, 1.0))
        }
    }
}

// ── the master meter ────────────────────────────────────────────────────────
//
// A segmented ladder, not a smooth bar. The segments are the point: an LED
// ladder tells you WHERE you are at a glance because the lit count is a number
// you read without reading, and a continuous fill is a length you have to
// measure against a scale. It is also what the hardware this panel is dressed
// as actually had next to its VUs.

/// The bottom of the output ladder, in dBFS. The top is 0.
const MASTER_FLOOR_DB: f32 = -60.0;

/// Where the ladder changes colour, in dBFS.
///
/// **-18 and -6, which are not arbitrary.** -18 dBFS is the digital home of
/// the old +4 dBu nominal level, so green means "where the meter was designed
/// to sit"; -6 is a hand's width from the wall and is where somebody should
/// start caring.
const MASTER_AMBER_DB: f32 = -18.0;
const MASTER_RED_DB: f32 = -6.0;

/// Segments in each ladder.
const MASTER_SEGMENTS: usize = 22;

/// The ladder's colours, lit. Unlit is these at a tenth.
const LED_GREEN: Color32 = Color32::from_rgb(0x3d, 0xc0, 0x5a);
const LED_AMBER: Color32 = Color32::from_rgb(0xe0, 0xa8, 0x22);
const LED_RED: Color32 = Color32::from_rgb(0xd6, 0x38, 0x28);

/// The strip behind the scale that says the limiter is working.
///
/// **Not green.** Reduction is not a level and there is no amount of it that
/// is "good"; a colour that said otherwise would be a claim about somebody's
/// music.
const LED_GR: Color32 = Color32::from_rgb(0xe8, 0x8c, 0x2a);

/// The recess the ladders sit in.
const METER_FACE: Color32 = Color32::from_rgb(0x14, 0x12, 0x12);

/// A linear amplitude as a fraction of the ladder, 0 at the floor and 1 at 0 dB.
///
/// Linear in DECIBELS, which is the only scale on which a meter's top half is
/// not the only half that moves.
fn master_fraction(linear: f32) -> f32 {
    if linear <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * linear.log10();
    ((db - MASTER_FLOOR_DB) / -MASTER_FLOOR_DB).clamp(0.0, 1.0)
}

/// The colour a segment takes when it is lit, by where it sits on the ladder.
fn led_colour(fraction: f32) -> Color32 {
    let db = MASTER_FLOOR_DB + fraction * -MASTER_FLOOR_DB;
    if db >= MASTER_RED_DB {
        LED_RED
    } else if db >= MASTER_AMBER_DB {
        LED_AMBER
    } else {
        LED_GREEN
    }
}

/// One segmented ladder. `filled` is 0..=1 of its length, from the bottom.
fn draw_ladder(painter: &Painter, r: Rect, filled: f32, hold: Option<f32>) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 1.0, METER_FACE);
    let inner = r.shrink(1.0);
    if !inner.is_positive() {
        return;
    }
    let seg_h = inner.height() / MASTER_SEGMENTS as f32;
    // A hairline of face colour between segments, but only while there is
    // room for one: below about a point a gap is the whole segment.
    let gap = (seg_h * 0.22).min(1.5);
    for i in 0..MASTER_SEGMENTS {
        // How far up the ladder this segment's top edge is.
        let at = (i as f32 + 1.0) / MASTER_SEGMENTS as f32;
        let y = inner.bottom() - i as f32 * seg_h;
        let (top, bottom) = (y - seg_h + gap, y);
        if bottom <= top {
            continue;
        }
        let lit = filled >= at - 0.5 / MASTER_SEGMENTS as f32;
        let base = led_colour(at);
        let colour = if lit {
            base
        } else {
            // Unlit, not absent: an LED you can see is off is what tells you
            // how much headroom is left.
            Color32::from_rgba_unmultiplied(base.r(), base.g(), base.b(), 34)
        };
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(inner.left(), top), Pos2::new(inner.right(), bottom)),
            0.0,
            colour,
        );
    }
    // The peak hold: a bright line, drawn over whatever it lands on, because a
    // hold that is a lit segment is indistinguishable from the level itself.
    if let Some(h) = hold.filter(|h| *h > 0.0) {
        let y = inner.bottom() - h.clamp(0.0, 1.0) * inner.height();
        painter.line_segment(
            [Pos2::new(inner.left(), y), Pos2::new(inner.right(), y)],
            Stroke::new(1.5_f32, Color32::from_rgb(0xff, 0xf4, 0xe0)),
        );
    }
}

/// The master column: what leaves, how hard the limiter is working, and the
/// one control that changes it.
fn draw_master(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    if !l.master_bars.is_positive() {
        return;
    }
    let m = &view.master;
    // Two bars with a hair between them, or one when the output is mono.
    let bars = l.master_bars;
    let (left, right) = if m.mono {
        (bars, Rect::NOTHING)
    } else {
        (slice_h(bars, 0.0, 0.48), slice_h(bars, 0.52, 1.0))
    };
    draw_ladder(
        painter,
        left,
        master_fraction(m.left.peak.max(m.left.rms)),
        Some(master_fraction(m.left.hold)),
    );
    if right.is_positive() {
        draw_ladder(
            painter,
            right,
            master_fraction(m.right.peak.max(m.right.rms)),
            Some(master_fraction(m.right.hold)),
        );
    }

    // **The reduction is a strip behind the scale, and the scale is its
    // readout.** It hangs from 0 and reaches down by however many decibels the
    // limiter took off, against the same numbers the ladders are read against
    // — so 6 dB of reduction reaches the -6, and it needs no number of its own.
    // It had a column, and a column for something that is usually zero is a
    // column the meters could have had.
    // **Both sides, mirrored.** The numbers on the left are read against the
    // left channel and the ones on the right against the right; one scale for
    // a stereo pair means one of the two channels is always being estimated.
    for (side, face) in l.master_scale.into_iter().enumerate() {
        if !face.is_positive() {
            continue;
        }
        // The left scale is right-aligned against the ladders and the right
        // scale left-aligned against them, so both sit next to what they
        // measure rather than out at the column's edges.
        let (align, at_x) = if side == 0 {
            (Align2::RIGHT_CENTER, face.right() - 2.0)
        } else {
            (Align2::LEFT_CENTER, face.left() + 2.0)
        };
        // Small: eight numbers have to fit up the side of the ladder without
        // touching each other, and the ladder is what is being read.
        let step = l.master_bars.height() / 9.0;
        let size = fit_text(
            Rect::from_min_size(face.min, Vec2::new(face.width() - 3.0, step)),
            "-60",
            step * 0.86,
        );
        let inner = l.master_bars.shrink(1.0);

        // **The reduction, behind the numbers and no wider than they are.**
        // It hangs from 0 and reaches down by however many decibels the
        // limiter took off, against the same scale the ladders are read
        // against — so 6 dB of reduction reaches the -6, and it needs no
        // number of its own.
        //
        // Bounded by the WIDEST LABEL rather than by the column: the column is
        // as wide as the gap it was given and the strip is meant to sit under
        // the ticks, not sweep the whole margin.
        if view.gr_db > 0.0 && size >= MIN_TEXT {
            let wide = "-60".len() as f32 * ADV * size;
            let (left, right) = if side == 0 {
                ((at_x - wide).max(face.left()), at_x)
            } else {
                (at_x, (at_x + wide).min(face.right()))
            };
            let down = (view.gr_db / -MASTER_FLOOR_DB).clamp(0.0, 1.0) * inner.height();
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(left, inner.top()),
                    Pos2::new(right, inner.top() + down),
                ),
                1.0,
                // Translucent: the numbers are drawn over it and have to stay
                // readable, which is the whole reason it lives here.
                Color32::from_rgba_unmultiplied(LED_GR.r(), LED_GR.g(), LED_GR.b(), 110),
            );
        }

        if size >= MIN_TEXT {
            for db in [0.0_f32, -6.0, -12.0, -18.0, -24.0, -36.0, -48.0, -60.0] {
                let t = (db - MASTER_FLOOR_DB) / -MASTER_FLOOR_DB;
                let y = inner.bottom() - t * inner.height();
                painter.text(
                    Pos2::new(at_x, y),
                    align,
                    {
                        let n = if db == 0.0 {
                            "0".to_owned()
                        } else {
                            format!("{db:.0}")
                        };
                        // **Padded to three on the right-hand scale.** These
                        // are left-aligned over there, so "0" and "-6" start
                        // where "-60" starts and their digits sit a character
                        // or two left of the column the rest form. The font is
                        // monospaced, so leading spaces are exactly the fix.
                        if side == 0 {
                            n
                        } else {
                            format!("{n:>3}")
                        }
                    },
                    font(size),
                    p.faint,
                );
            }
        }
    }

}

/// The filters' caps.
///
/// **Violet, and it is the one hue nothing else here uses.** They were ivory,
/// which made two of the eight knobs read as blank caps with no colour at all
/// — the odd ones out rather than a pair. Violet sits opposite the panel's
/// warm brown, so it separates cleanly without competing with the sends'
/// blue, the limiter's green or the master's red, and it takes the same white
/// slot every other cap has.
const FILTER_CAP: Color32 = Color32::from_rgb(0x7b, 0x5c, 0xc9);

/// The limiter's cap.
///
/// **Green, and it was red until the master arrived.** Red is now the master's,
/// which is the right home for it: the master is the one knob on this panel
/// that can undo everything the others did. The limiter keeps a colour of its
/// own because it is neither a send nor a filter — a deep bottle green, which
/// is what the meter lamps on the gear this panel is dressed as were, and
/// which nothing else here is.
const LIMITER_CAP: Color32 = Color32::from_rgb(0x2f, 0x8f, 0x5c);

/// The master's cap.
///
/// Red, alone. It is the last thing in the chain and the only control here
/// that can put the output past the ceiling the limiter just guaranteed.
const MASTER_CAP: Color32 = Color32::from_rgb(0xc4, 0x2f, 0x22);

/// A transport button's side, as a fraction of the tempo knob's diameter.
///
/// **Measured against the knob beside them**, which is what makes the four of
/// them one group. Smaller than it: the transport's job is Record, and three
/// buttons the size of the thing next to them read as the loudest object in
/// the band — which they are not; the meters are.
const TRANSPORT_SIDE: f32 = 0.72;

/// The tempo box's share of the fader column's height.
///
/// The faders' share of the middle group's width.
///
/// The rest, less a gap, is the meters with the knob row under them. Named
/// because three places have to agree about it and the failure when they do
/// not is a knob drawn over a meter.
const FADER_COL: f32 = 0.46;

/// The meters' share of the height of the column they share with the knobs.
///
/// **The meters give up the smaller half of what the knobs gained.** A VU face
/// is sized by whichever of its height and width runs out first and it was
/// width-limited before, so a shorter column costs it less than the number
/// suggests — and the band is taller now than it was, which pays for most of
/// it. See `BAND_H_AT_1300`.
/// **0.40, down from 0.46, which is where the dials got their size from.**
/// The VU pair is a picture of a level and reads at any height; a dial below a
/// certain diameter is three grey rings, and there are eight of them. The
/// meters gave up eight percent of the column and every knob on the panel is a
/// sixth bigger for it.
const METER_SHARE: f32 = 0.40;

/// Where the transport's right column stops and the master column starts.
const MASTER_COL: f32 = 0.78;


/// The dB scale's share of the master column's width, **at each side**. What
/// is left between the two is the ladders.
const MASTER_SCALE_W: f32 = 0.26;

/// The smallest knob face worth drawing, as a radius.
///
/// Below this the body, the cap and the slot land inside a few points of each
/// other and it is three grey rings, not a control.
const KNOB_MIN_R: f32 = 7.0;

/// The label's share of a knob cell, and its bounds in points.
///
/// The top end matters now that the knobs are a row rather than a stack: a
/// cell three times as wide can carry a word three times the size, and a knob
/// whose name is set in six points beside a thirty-point dial reads as a dial
/// with a smudge over it.
const KNOB_LABEL: (f32, f32, f32) = (0.24, 6.0, 15.0);

/// What a knob gets out of its cell: where the face goes, how big it is, and
/// how much was left for the word above it.
struct KnobFace {
    face: Rect,
    /// **The body's radius, not the skirt's.** The ticks stand outside it and
    /// are deliberately not counted: sizing the knob to fit them made the
    /// visible circle a fifth smaller than the room it had, which is a knob
    /// that looks lost in its own cell.
    radius: f32,
    /// Points reserved for the label. **Zero means there is no room for one**,
    /// and the face takes the whole cell.
    label_h: f32,
}

/// How a knob would be laid out in `cell`, or `None` if there is no room.
///
/// **The layout and the painter both ask this**, which is what keeps them from
/// disagreeing about whether a knob is there — the failure that leaves a drag
/// target on blank panel.
///
/// **The word yields before the knob does.** In a short band the label eats a
/// third of the cell to say something nobody can read at five points, and what
/// is lost is the control. So below the label's own minimum the face takes
/// everything: a knob with no caption is still a knob, still grabbable, and
/// still says what it is set to the moment a hand is on it.
fn knob_face(cell: Rect) -> Option<KnobFace> {
    if !cell.is_positive() {
        return None;
    }
    let (share, lo, hi) = KNOB_LABEL;
    let wanted = cell.height() * share;
    let label_h = if wanted >= lo { wanted.min(hi) } else { 0.0 };
    let face = Rect::from_min_max(
        Pos2::new(cell.left(), cell.top() + label_h),
        Pos2::new(cell.right(), cell.bottom()),
    );
    let radius = face.width().min(face.height()) * 0.5;
    (radius >= KNOB_MIN_R).then_some(KnobFace {
        face,
        radius,
        label_h,
    })
}

/// Where a knob's circle sits, for anything that has to line up with it.
///
/// **The centre of the FACE, both ways.** The circle used to hang from the top
/// of its face, so a cell taller than it was wide left the slack underneath —
/// and anything aligned to the cell's middle sat below the knob rather than
/// beside it.
fn knob_centre(cell: Rect) -> Option<Pos2> {
    knob_face(cell).map(|k| k.face.center())
}

fn knob_fits(cell: Rect) -> bool {
    knob_face(cell).is_some()
}

/// One knob, modelled on the Tascam 388's effect returns.
///
/// Black body, blue cap, and a pale slot across the cap that IS the pointer —
/// the 388 has no separate indicator line, the moulding is slotted and the slot
/// tells you where it is. A word over the top, and tick marks around the skirt
/// so a glance can tell a quarter turn from a half.
///
/// `value` is 0..=1 and the caller owns what that means.
/// One knob's worth of state, for [`draw_knob`].
///
/// A struct rather than seven positional arguments: two of them are strings
/// and two are bools, and a call site that got either pair the wrong way round
/// would compile.
struct Knob<'a> {
    /// 0..=1, whatever that means to the caller.
    value: f32,
    /// The word over it.
    label: &'a str,
    /// What is being typed into it, if anything.
    typing: Option<&'a str>,
    /// A hand is on it right now, so show the reading rather than the name.
    turning: bool,
    /// What the reading SAYS. A send is a percent and a tempo is a number of
    /// beats; the knob does not know which it is and should not guess.
    reading: String,
    /// The cap. The effects share one; the tempo has its own, which is what
    /// makes it read as part of the transport rather than a fourth send.
    cap: Color32,
}

fn draw_knob(painter: &Painter, cell: Rect, k: &Knob<'_>, p: &Palette) {
    let (value, label, typing, turning) = (k.value, k.label, k.typing, k.turning);
    if !cell.is_positive() {
        return;
    }
    // The label takes the top and the knob gets a square out of what is left.
    // Sized off the CELL rather than measured off the text, because a knob that
    // changed size with the length of the word over it would leave these two
    // different sizes.
    let Some(KnobFace {
        face,
        radius: rad,
        label_h,
    }) = knob_face(cell)
    else {
        return;
    };
    // **The name gives way to the number while a hand is on it.** There is
    // nowhere else on a knob to put characters, and a knob with no readout is
    // one you set by ear and then cannot repeat. Whole percent: a knob is not
    // a control anybody lands on a tenth of one, and the extra digits are
    // noise moving under a moving hand.
    let (text, ink) = match (typing, turning) {
        (Some(typed), _) => (Some(format!("{typed}_")), p.ink),
        (None, true) => (Some(k.reading.clone()), p.ink),
        // The name only when there is a strip to put it in. See `knob_face`:
        // the word is what gives way in a short band, not the control.
        (None, false) => ((label_h > 0.0).then(|| label.to_owned()), p.faint),
    };
    if let Some(text) = text {
        // A reading has to be drawn even when there is no label strip — it is
        // the whole point of the gesture — so it borrows the top of the face.
        let strip = label_h.max(KNOB_LABEL.1);
        // **Fitted to the cell, not just to the strip.** The row's cells are
        // wide and short; sizing the word off the strip alone leaves it far
        // smaller than the space it has, and "CHORUS" is six characters that
        // have somewhere to go.
        let band = Rect::from_min_max(
            Pos2::new(cell.left(), cell.top()),
            Pos2::new(cell.right(), cell.top() + strip),
        );
        let size = fit_text(band, &text, strip * 0.96);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(cell.center().x, cell.top()),
                Align2::CENTER_TOP,
                &text,
                font(size),
                ink,
            );
        }
    }
    let c = face.center();
    let t = value.clamp(0.0, 1.0);
    // Straight down is the middle of the missing quarter, so the sweep runs
    // from half of it past one side to half of it past the other.
    let angle = std::f32::consts::PI + (t - 0.5) * KNOB_SWEEP;
    let dir = |a: f32| Vec2::new(a.sin(), -a.cos());

    // **The ticks stand OUTSIDE the body, and outside the cell.** They are a
    // mark on the panel the knob is mounted through, not part of the knob, so
    // they are not what its size is measured against — see `knob_face`. Drawn
    // first, so the body covers their inner ends.
    let tick = Stroke::new((rad * 0.09).max(0.7), p.faint);
    for i in 0..KNOB_TICKS {
        let a = std::f32::consts::PI
            + (i as f32 / (KNOB_TICKS - 1) as f32 - 0.5) * KNOB_SWEEP;
        let d = dir(a);
        painter.line_segment([c + d * (rad * 1.09), c + d * (rad * 1.26)], tick);
    }
    // The body: near-black, like the moulding, filling the face it was given.
    painter.circle_filled(c, rad, Color32::from_rgb(0x14, 0x12, 0x12));
    // The cap, inset, in its own colour.
    let cap = rad * 0.64;
    painter.circle_filled(c, cap, k.cap);
    // The slot. Across the whole cap and through the centre, which is what a
    // screwdriver slot looks like and what makes the angle readable at
    // fourteen points: a short pointer at one edge is a dot at this size.
    let d = dir(angle);
    painter.line_segment(
        [c - d * cap * 0.92, c + d * cap * 0.92],
        Stroke::new((cap * 0.30).max(1.0), Color32::from_rgb(0xd8, 0xe4, 0xf2)),
    );
}

/// The tempo, as a knob at the head of the transport.
///
/// **The same knob as the three effect sends, in a different colour.** That is
/// the whole point: a number floating beside three buttons is a number that
/// happened to be nearby, and a knob among them is part of the transport. The
/// cap is a darkened orange so it is not read as a fourth send.
///
/// Turned like the others — relatively, so the pointer can leave it and go on
/// turning — and DOUBLE-clicked to type, which is the one gesture the sends do
/// not have. A single tap on a send opens its field; the tempo is turned far
/// more often than it is typed, and a tap that opened a text box every time a
/// hand brushed it would be in the way.
fn draw_tempo(
    painter: &Painter,
    l: &Layout,
    bpm: f64,
    typing: Option<&str>,
    turning: bool,
    p: &Palette,
) {
    draw_knob(
        painter,
        l.tempo,
        &Knob {
            value: tempo_to_knob(bpm),
            label: "TEMPO",
            // Never in the knob's own strip: the typed number gets a box of
            // its own below, where there is room for digits and a caret.
            typing: None,
            turning: turning || typing.is_some(),
            reading: tempo_text(bpm),
            cap: TEMPO_CAP,
        },
        p,
    );
    // The typing box, only while somebody is typing into it. A box that was
    // always there would be a second tempo readout under the first.
    let Some(typed) = typing else { return };
    let r = l.tempo_entry;
    if !r.is_positive() {
        return;
    }
    control(painter, r, p);
    let shown = format!("{typed}_");
    let size = fit_text(r.shrink(r.width() * 0.08), &shown, r.height() * 0.72);
    if size >= MIN_TEXT {
        painter.text(r.center(), Align2::CENTER_CENTER, &shown, font(size), p.ink);
    }
}

/// Where `bpm` sits on the knob, 0..=1.
///
/// Linear across the legal range, which over [`KNOB_TRAVEL`] points of travel
/// is about a beat a point — fine enough to land on a number by hand, and the
/// reading says which one.
pub fn tempo_knob_position(bpm: f64) -> f32 {
    tempo_to_knob(bpm)
}

fn tempo_to_knob(bpm: f64) -> f32 {
    ((bpm - MIN_BPM) / (MAX_BPM - MIN_BPM)).clamp(0.0, 1.0) as f32
}

/// The inverse of [`tempo_to_knob`], clamped to what the SMF writer and every
/// DAW's bar ruler will accept.
pub fn knob_to_tempo(t: f32) -> f64 {
    MIN_BPM + f64::from(t.clamp(0.0, 1.0)) * (MAX_BPM - MIN_BPM)
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

    /// **Every control in the take-settings panel can actually be clicked.**
    ///
    /// The regression this exists for, in full: 4.19.0 moved the camera and the
    /// audio input out of this panel and onto the controls they feed, and took
    /// the AUDIO STATUS row out with them. That one was not moved anywhere.
    /// `Hit::ShowAudioStatus` kept its variant, kept its tooltip, kept its arm
    /// in `app.rs` — and had no rectangle in any layout, so the panel that says
    /// what rate the two streams are running at could not be opened from
    /// anywhere in the app. Nothing failed. Nothing warned. It was simply gone,
    /// and it stayed gone for a release.
    ///
    /// A rect-by-rect walk rather than a list of Hits, so this cannot be
    /// satisfied by a target that exists in `targets()` and is covered by
    /// something drawn over it.
    #[test]
    fn every_take_settings_control_can_actually_be_clicked() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1280.0, 720.0));
        let anchor = Rect::from_min_size(Pos2::new(40.0, 480.0), Vec2::new(20.0, 20.0));
        let l = SetupLayout::new(screen, anchor);
        let v = idle();
        for (rect, want) in l.targets() {
            assert!(rect.is_positive(), "{want:?} has no rectangle at all");
            let got = setup_hit_test(screen, anchor, &v, rect.center());
            assert_eq!(got, Some(want), "clicking {want:?} landed on {got:?}");
        }
        // The two that are only reachable here, and the way in to Setup.
        for want in [
            Hit::ToggleCountInInTake,
            Hit::ToggleHideElapsed,
            Hit::ShowAudioStatus,
        ] {
            let rect = match want {
                Hit::ToggleCountInInTake => l.count_in_in_take,
                Hit::ToggleHideElapsed => l.hide_elapsed,
                _ => l.audio,
            };
            assert_eq!(
                setup_hit_test(screen, anchor, &v, rect.center()),
                Some(want),
                "{want:?} is not reachable from the take settings"
            );
        }
    }

    /// No two controls in the panel share a pixel.
    ///
    /// Seven rows where there were six: the row that was added is the row most
    /// likely to have been laid over the one above it.
    #[test]
    fn the_take_settings_rows_do_not_overlap() {
        for w in [900.0_f32, 1280.0, 1920.0] {
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w * 0.56));
            let anchor = Rect::from_min_size(Pos2::new(40.0, w * 0.3), Vec2::new(20.0, 20.0));
            let l = SetupLayout::new(screen, anchor);
            let zones: Vec<(&str, Rect)> = vec![
                ("name", l.name),
                ("dest", l.dest),
                ("reveal", l.reveal),
                ("folder", l.folder),
                ("disk", l.disk),
                ("count-in", l.count_in),
                ("time sig", l.time_sig),
                ("export", l.export),
                ("open when done", l.open_when_done),
                ("click in take", l.click_in_take),
                ("count-in in take", l.count_in_in_take),
                ("hide elapsed", l.hide_elapsed),
                ("audio setup", l.audio),
                ("close", l.close),
            ];
            for i in 0..zones.len() {
                for j in (i + 1)..zones.len() {
                    if !zones[i].1.is_positive() || !zones[j].1.is_positive() {
                        continue;
                    }
                    assert!(
                        !zones[i].1.intersects(zones[j].1),
                        "{} and {} overlap at {w}",
                        zones[i].0,
                        zones[j].0
                    );
                }
            }
            // And every one of them is inside the panel, which a seventh row
            // pushing past the bottom edge is exactly how it would not be.
            for (what, z) in &zones {
                if z.is_positive() {
                    assert!(l.panel.contains_rect(*z), "{what} is outside the panel at {w}");
                }
            }
        }
    }

    /// **The backing track's panel does not shout.**
    ///
    /// It is the widest panel this file draws — 720 points against an effect
    /// panel's 300 — and every piece of text in it was sized as a fraction of
    /// rows derived from that width. The result was a title at 24 points and
    /// trim readouts at 33, against a band whose own readouts are 11 to 14: the
    /// owner's words were "all of it smaller, it's way too big".
    ///
    /// Asserted as POINTS at a real window size rather than as the fractions,
    /// because the fractions are what went wrong — each one looked reasonable
    /// against its own box.
    #[test]
    fn the_backing_track_panel_is_sized_like_the_rest_of_the_app() {
        for w in [1280.0_f32, 1440.0, 1920.0] {
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w * 0.62));
            let anchor = Rect::from_min_size(Pos2::new(w * 0.3, w * 0.2), Vec2::new(16.0, 16.0));
            let l = TrackLayout::new(screen, anchor);
            assert!(l.panel.is_positive(), "no panel at {w}");
            // The nominal sizes `draw_track_panel` asks for, in the same order
            // the reader meets them.
            let title = l.title.height() * 0.42;
            let field = l.field_in.height() * 0.34;
            let reset = l.reset.height() * 0.30;
            for (what, pt) in [("title", title), ("trim field", field), ("reset", reset)] {
                assert!(
                    pt <= 20.0,
                    "the {what} asks for {pt:.0} points at a {w:.0}-point window"
                );
                assert!(
                    pt >= MIN_TEXT,
                    "the {what} shrank to {pt:.0} points and will not draw"
                );
            }
            // The numbers being typed into are the largest thing in the panel,
            // which is the one piece of hierarchy it has.
            assert!(field > title, "the trim readouts are not the subject");
        }
    }

    /// **The microphone icon keeps a rectangle mid-take; the PICKER does not.**
    ///
    /// The difference is the whole reason a right-click on those pixels reads
    /// `input_icon` rather than going through `hit_test`. The picker is gated
    /// off while a take rolls so that nobody swaps the input device out from
    /// under a running encoder — a good rule that does not apply to live
    /// monitoring, which is listen-only and is exactly what somebody wants to
    /// check mid-take. Sharing one gate would have disabled both.
    #[test]
    fn monitoring_stays_reachable_mid_take_and_the_picker_does_not() {
        let r = band(1280.0);
        let icon = input_icon(r, &idle()).expect("a microphone icon");
        assert_eq!(
            hit_test(r, &idle(), icon.center()),
            Some(Hit::PickAudio),
            "a left-click on the icon does not open the picker"
        );

        let rolling = rolling();
        assert_eq!(
            hit_test(r, &rolling, icon.center()),
            None,
            "the device picker is reachable mid-take"
        );
        assert_eq!(
            input_icon(r, &rolling),
            Some(icon),
            "the icon lost its rectangle mid-take, so monitoring cannot be toggled"
        );
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

    /// **A knob drags up, a fader drags across.** The caller pins the axis a
    /// control does NOT travel on, so an answer of Horizontal for the knobs
    /// would freeze the only axis they move on — the control would be dead
    /// rather than fussy, and nothing about the picture would say so.
    #[test]
    fn each_control_reports_the_axis_it_actually_travels_on() {
        let r = band(1300.0);
        // A full rack: an empty slot has no gain knob, and asking for the axis
        // of a control that is not there is correctly answered with None.
        let v = with_rack(RecordState::Idle, racks()[1]);
        for (hit, want) in [
            (Hit::SetFx(Fx::Reverb, 0.0), DragAxis::Vertical),
            (Hit::SetFx(Fx::Delay, 0.0), DragAxis::Vertical),
            (Hit::SetMetronomeGain(0.0), DragAxis::Horizontal),
            (Hit::SetInputGain(0.0), DragAxis::Horizontal),
            (Hit::SetSlotGain(0, 0.0), DragAxis::Horizontal),
        ] {
            assert_eq!(
                drag_axis(r, &v, hit),
                Some(want),
                "{} travels the other way",
                hit.label()
            );
        }
        // A button is not dragged at all, and a caller that treated one as a
        // fader would set a value every time the pointer moved over it.
        assert_eq!(drag_axis(r, &v, Hit::Record), None);
        assert_eq!(drag_axis(r, &v, Hit::Stop), None);
    }

    /// Both ends of a knob's travel are reachable, and up is more.
    /// **A filter knob reads in hertz, and a send still reads in percent.**
    /// **The tempo drags relatively, like every other knob.**
    ///
    /// It never did: `with_value` had no arm for it, so every frame of a drag
    /// re-applied the hit the PRESS produced — which is absolute. The knob
    /// jumped to wherever it was first touched and then would not move.
    #[test]
    fn the_tempo_knob_travels_with_the_hand() {
        // A drag is `control_value` plus how far the hand has moved, put
        // through `with_value`. Half way up the dial is half way up the range.
        let mid = Hit::SetTempo(0.0).with_value(0.5);
        let Hit::SetTempo(bpm) = mid else {
            panic!("the tempo lost its value: {mid:?}")
        };
        assert!(
            (bpm - (MIN_BPM + (MAX_BPM - MIN_BPM) * 0.5)).abs() < 0.01,
            "half travel is {bpm} BPM"
        );
        // Both ends, and the round trip through the position it reports.
        assert_eq!(Hit::SetTempo(0.0).with_value(0.0), Hit::SetTempo(MIN_BPM));
        assert_eq!(Hit::SetTempo(0.0).with_value(1.0), Hit::SetTempo(MAX_BPM));
        for bpm in [40.0_f64, 92.5, 120.0, 200.0] {
            let back = Hit::SetTempo(0.0).with_value(tempo_knob_position(bpm));
            let Hit::SetTempo(got) = back else { panic!() };
            assert!((got - bpm).abs() < 0.01, "{bpm} came back as {got}");
        }
        // **And every control that travels goes through it.** The tempo was
        // the one that did not, and nothing said so. Two different values, so
        // this asks whether the value is USED rather than whether it happens
        // to differ from whatever `Hit::ALL` was built with.
        for h in Hit::ALL.into_iter().filter(|h| h.is_draggable()) {
            assert_ne!(
                h.with_value(0.2),
                h.with_value(0.8),
                "{h:?} ignores the value a drag hands it"
            );
        }
    }

    /// **The clip latch is cleared by pressing the meter it is shown on.**
    ///
    /// It was a word in the status line, which is a second place to look for a
    /// fact the VU's own lamp already carries — and on a Mac with no interface
    /// selected the lamp never lit at all, so the word was the only indication
    /// and it was the one that did not work. One indicator, in the place
    /// somebody watching levels is already looking, and pressing it is what
    /// puts it out.
    #[test]
    fn the_clip_latch_is_cleared_by_pressing_the_meter() {
        let r = band(1300.0);
        let v = idle();
        let l = Layout::new(r, &v);
        assert!(l.meter.is_positive(), "no meter to press");
        assert_eq!(hit_test(r, &v, l.meter.center()), Some(Hit::DismissClip));

        // Live as well as idle: a clip during a take is exactly when somebody
        // acknowledges one.
        let rolling = rolling();
        let l = Layout::new(r, &rolling);
        assert!(l.meter.is_positive());
        assert_eq!(hit_test(r, &rolling, l.meter.center()), Some(Hit::DismissClip));
    }

    /// **Both devices are chosen from the control they feed**, on a left
    /// click, and neither while a take is rolling.
    #[test]
    fn the_microphone_is_its_own_picker() {
        let r = band(1300.0);
        let v = idle();
        let l = Layout::new(r, &v);
        assert!(l.input_icon.is_positive(), "no microphone icon");
        assert_eq!(hit_test(r, &v, l.input_icon.center()), Some(Hit::PickAudio));

        let rolling = rolling();
        assert_ne!(
            hit_test(r, &rolling, Layout::new(r, &rolling).input_icon.center()),
            Some(Hit::PickAudio),
            "the picker is reachable during a take"
        );
    }

    /// **Pressing the preview chooses the camera** — and only at rest.
    #[test]
    fn the_preview_is_the_camera_picker_until_a_take_starts() {
        let r = band(1300.0);
        let v = idle();
        let l = Layout::new(r, &v);
        assert!(l.preview.is_positive(), "no preview");
        assert_eq!(hit_test(r, &v, l.preview.center()), Some(Hit::PickCamera));

        // Not mid-take: the camera cannot be changed then, and the dialog it
        // opens would be modal over the one gesture that matters, which is
        // Stop.
        let rolling = rolling();
        assert_ne!(
            hit_test(r, &rolling, Layout::new(r, &rolling).preview.center()),
            Some(Hit::PickCamera),
            "the picker is reachable during a take"
        );
    }

    /// **Every knob resets to the position where it is doing nothing** —
    /// except the two that are not effects, which have a resting value of
    /// their own.
    #[test]
    fn a_double_click_puts_every_knob_back() {
        use crate::recorder::fader_to_gain;
        for fx in Fx::ALL {
            // The limiter's dial is a threshold: not applied is fully
            // clockwise, where it is above everything and catches nothing.
            let rest = if fx == Fx::Limiter { 1.0 } else { 0.0 };
            assert_eq!(
                Hit::SetFx(fx, 0.77).reset_to(),
                Some(Hit::SetFx(fx, rest)),
                "{} does not reset to nothing applied",
                fx.title()
            );
        }
        assert_eq!(Hit::SetTempo(180.0).reset_to(), Some(Hit::SetTempo(120.0)));
        // The master goes to UNITY, which is 0 dB, not to the bottom of its
        // travel — a master that reset to silence would be a reset nobody
        // could use.
        let Some(Hit::SetMaster(back)) = Hit::SetMaster(0.1).reset_to() else {
            panic!("the master does not reset")
        };
        assert!(
            (fader_to_gain(back) - 1.0).abs() < 1.0e-4,
            "the master resets to {} dB",
            20.0 * fader_to_gain(back).log10()
        );

        // **And the faders, which reset to what they SHIP as and not to
        // silence.** "Nothing applied" for a level would be off, and a reset
        // that mutes what you were listening to is one nobody uses twice.
        let d = Gains::default();
        for (hit, want) in [
            (Hit::SetInputGain(0.3), d.inputs[0]),
            (Hit::SetSlotGain(1, 0.3), d.slots[1]),
            (Hit::SetTrackGain(0.3), d.track),
            (Hit::SetMetronomeGain(0.9), d.metronome),
        ] {
            let back = hit.reset_to().expect("a level with no resting value");
            let Some(v) = (match back {
                Hit::SetInputGain(v)
                | Hit::SetSlotGain(_, v)
                | Hit::SetTrackGain(v)
                | Hit::SetMetronomeGain(v) => Some(v),
                _ => None,
            }) else {
                panic!("{hit:?} reset to {back:?}")
            };
            assert!(
                (fader_to_gain(v) - want).abs() < 1.0e-3,
                "{hit:?} reset to {} and ships at {want}",
                fader_to_gain(v)
            );
        }
        // A button has nothing to put back.
        assert_eq!(Hit::Record.reset_to(), None);
        assert_eq!(Hit::Type(NumField::Tempo).reset_to(), None);
    }

    /// The eight knobs are exactly the controls with the knob gesture set.
    #[test]
    fn the_knobs_are_the_ones_that_look_like_knobs() {
        let knobs: Vec<Hit> = Hit::ALL.into_iter().filter(|h| h.is_knob()).collect();
        assert_eq!(knobs.len(), Fx::ALL.len() + 2, "{knobs:?}");
        for fx in Fx::ALL {
            assert!(knobs.iter().any(|h| h.is_same_control(Hit::SetFx(fx, 0.0))));
        }
        assert!(knobs.iter().any(|h| h.is_same_control(Hit::SetTempo(0.0))));
        assert!(knobs.iter().any(|h| h.is_same_control(Hit::SetMaster(0.0))));
        // And every one of them can be typed into, which is what the
        // right-click does.
        for h in knobs {
            assert!(num_field(h).is_some(), "{h:?} cannot be typed into");
            assert!(h.reset_to().is_some(), "{h:?} cannot be reset");
        }
    }

    /// A track panel, and a track to put in it.
    fn a_track() -> crate::ports::TrackInfo {
        crate::ports::TrackInfo {
            name: "backing.mp3".to_owned(),
            seconds: 200.0,
            wave: vec![0.5; 1000],
            error: String::new(),
        }
    }

    /// **The trim reads as a fraction of the file, and zero means the end.**
    #[test]
    fn the_trim_spans_what_it_says() {
        // Untrimmed: the whole file.
        assert_eq!(trim_fractions(200.0, 0.0, 0.0), (0.0, 1.0));
        // An out-point of zero is the END, everywhere — it is what the engine
        // reads and what the settings hold.
        assert_eq!(trim_fractions(200.0, 50.0, 0.0), (0.25, 1.0));
        assert_eq!(trim_fractions(200.0, 50.0, 150.0), (0.25, 0.75));
        // Past the ends it pins rather than drawing off the edge.
        assert_eq!(trim_fractions(200.0, -9.0, 900.0), (0.0, 1.0));
        // A file of no length has no fractions to give and must not divide.
        assert_eq!(trim_fractions(0.0, 1.0, 2.0), (0.0, 1.0));
        // Crossed points come back in order rather than as a negative span.
        let (a, b) = trim_fractions(200.0, 150.0, 50.0);
        assert!(a <= b, "{a} {b}");
    }

    /// **Each part of the panel answers for itself, and the waveform's middle
    /// answers for nothing.**
    ///
    /// A press in the body of a waveform is a press on a picture. Treating it
    /// as "move whichever handle is nearest" would move half the track from
    /// under a hand that was only pointing at something.
    #[test]
    fn the_track_panel_hits_what_is_under_the_pointer() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0));
        let anchor = Rect::from_min_size(Pos2::new(420.0, 300.0), Vec2::new(18.0, 18.0));
        let l = TrackLayout::new(screen, anchor);
        assert!(l.panel.is_positive(), "no panel");
        assert!(
            screen.contains_rect(l.panel),
            "the panel hangs off the window"
        );

        let at = |p: Pos2| track_hit_test(screen, anchor, 200.0, 50.0, 150.0, p);
        assert_eq!(at(l.close.center()), Some(TrackHit::Close));
        assert_eq!(at(l.reset.center()), Some(TrackHit::ClearTrim));
        assert_eq!(at(l.field_in.center()), Some(TrackHit::TypeIn));
        assert_eq!(at(l.field_out.center()), Some(TrackHit::TypeOut));
        // Outside the panel entirely: not ours, which is what dismisses it.
        assert_eq!(at(Pos2::new(5.0, 5.0)), None);

        // The handles: 50/200 and 150/200 along the waveform.
        let x = |t: f32| l.wave.left() + t * l.wave.width();
        let y = l.wave.center().y;
        assert!(matches!(
            at(Pos2::new(x(0.25), y)),
            Some(TrackHit::DragIn(_))
        ));
        assert!(matches!(
            at(Pos2::new(x(0.75), y)),
            Some(TrackHit::DragOut(_))
        ));
        // And the middle of the kept part is neither.
        assert_eq!(at(Pos2::new(x(0.5), y)), None, "the body moved a handle");

        // The fraction a drag reports is where the pointer is, not where the
        // handle was.
        let Some(TrackHit::DragIn(t)) = at(Pos2::new(x(0.25) + 2.0, y)) else {
            panic!("not the in handle")
        };
        assert!((t - 0.25).abs() < 0.02, "it reported {t}");
    }

    /// The panel stays on screen however near an edge its icon is.
    #[test]
    fn the_track_panel_stays_inside_the_window() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 600.0));
        for corner in [
            Pos2::new(0.0, 0.0),
            Pos2::new(890.0, 0.0),
            Pos2::new(0.0, 590.0),
            Pos2::new(890.0, 590.0),
        ] {
            let anchor = Rect::from_min_size(corner, Vec2::new(14.0, 14.0));
            let l = TrackLayout::new(screen, anchor);
            assert!(
                screen.contains_rect(l.panel),
                "anchored at {corner:?} the panel is at {:?}",
                l.panel
            );
        }
    }

    /// Drawing an empty panel and a full one both work at every size.
    #[test]
    fn the_track_panel_draws_at_every_size() {
        let ctx = egui::Context::default();
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        for w in [400.0_f32, 900.0, 1600.0] {
            for track in [crate::ports::TrackInfo::default(), a_track()] {
                let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w * 0.6));
                let anchor = Rect::from_min_size(
                    Pos2::new(w * 0.3, w * 0.3),
                    Vec2::new(14.0, 14.0),
                );
                let _ = ctx.run(Default::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        draw_track_panel(
                            ui.painter(),
                            TrackPanel {
                                screen,
                                anchor,
                                track: &track,
                                from: 9.5,
                                to: 196.0,
                                typing: (None, Some("1:12")),
                            },
                            &Settings::default(),
                        );
                    });
                });
            }
        }
    }

    /// **The master ladder is linear in decibels**, which is the only scale
    /// on which the bottom half of a meter ever moves.
    #[test]
    fn the_master_ladder_is_a_decibel_scale() {
        assert!((master_fraction(1.0) - 1.0).abs() < 1.0e-4, "0 dBFS is the top");
        assert_eq!(master_fraction(0.0), 0.0, "silence is the floor");
        // -60 dBFS is the floor, and half way up is -30.
        let floor = 10f32.powf(MASTER_FLOOR_DB / 20.0);
        assert!(master_fraction(floor) < 1.0e-4);
        let half = 10f32.powf((MASTER_FLOOR_DB / 2.0) / 20.0);
        assert!((master_fraction(half) - 0.5).abs() < 0.01, "-30 dB is not the middle");
        // Past full scale it pins rather than running off the top.
        assert_eq!(master_fraction(4.0), 1.0);

        // The colours change where they say they do, and nowhere else.
        let at = |db: f32| led_colour((db - MASTER_FLOOR_DB) / -MASTER_FLOOR_DB);
        assert_eq!(at(-40.0), LED_GREEN);
        assert_eq!(at(-19.0), LED_GREEN);
        assert_eq!(at(-17.0), LED_AMBER);
        assert_eq!(at(-7.0), LED_AMBER);
        assert_eq!(at(-5.0), LED_RED);
        assert_eq!(at(0.0), LED_RED);
    }

    /// **The master column sits beside the band, not on top of it.**
    ///
    /// Four new rectangles went into a row that was already full. Every one of
    /// them has to clear the knobs to its left and the slots to its right, at
    /// every width — an overlap here is a control that cannot be pressed.
    #[test]
    fn the_master_column_clears_everything_around_it() {
        for w in [500.0_f32, 900.0, 1300.0, 1800.0] {
            let r = band(w);
            for v in [idle(), rolling()] {
                let l = Layout::new(r, &v);
                // **First: it is there at all.** Every assertion below skips a
                // rectangle that is NOTHING, so without this the whole test
                // passes on a layout that simply never placed the master.
                if w >= 900.0 {
                    assert!(l.master_bars.is_positive(), "no master ladder at {w}");
                    assert!(l.master_knob.is_positive(), "no master knob at {w}");
                }
                let master = [
                    ("scale L", l.master_scale[0]),
                    ("scale R", l.master_scale[1]),
                    ("bars", l.master_bars),
                    ("knob", l.master_knob),
                ];
                for (name, m) in master {
                    if !m.is_positive() {
                        continue;
                    }
                    assert!(
                        r.contains_rect(m),
                        "the master {name} is outside the band at {w}"
                    );
                    for (other, o) in Fx::ALL.map(|fx| (fx.title(), l.fx[fx.index()])) {
                        if o.is_positive() {
                            assert!(
                                m.left() >= o.right() - 0.5,
                                "the master {name} overlaps {other} at {w}"
                            );
                        }
                    }
                    if l.meter.is_positive() {
                        assert!(
                            m.left() >= l.meter.right() - 0.5,
                            "the master {name} overlaps the VU at {w}"
                        );
                    }
                }
                // And the knob is an effect knob's size, because it is one.
                if l.master_knob.is_positive() && l.fx[0].is_positive() {
                    assert!(
                        (l.master_knob.height() - l.fx[0].height()).abs() < 0.5,
                        "the master knob is {} high and an effect knob is {} at {w}",
                        l.master_knob.height(),
                        l.fx[0].height()
                    );
                }
            }
        }
    }

    /// The limiter and the master do not wear the same cap.
    ///
    /// They sit two inches apart and mean opposite things — one holds the
    /// output down, the other is the only control that can push it up past
    /// what the first one guaranteed.
    #[test]
    fn the_limiter_and_the_master_are_told_apart_by_colour() {
        assert_ne!(Fx::Limiter.cap(), MASTER_CAP);
        for fx in Fx::ALL {
            assert_ne!(fx.cap(), MASTER_CAP, "{} wears the master's red", fx.title());
        }
    }

    #[test]
    fn a_knob_reads_out_in_its_own_unit() {
        assert_eq!(knob_reading(KnobUnit::Percent, 0.42), "42%");
        assert_eq!(knob_reading(KnobUnit::Percent, 0.0), "0%");
        assert_eq!(knob_reading(KnobUnit::Percent, 1.0), "100%");

        // The high-pass, as the host sweeps it.
        let hp = KnobUnit::Hertz {
            low: 20.0,
            high: 1_200.0,
        };
        assert_eq!(knob_reading(hp, 0.0), "20 Hz");
        assert_eq!(knob_reading(hp, 1.0), "1.2 kHz");
        // Half way up an exponential sweep is the GEOMETRIC middle, which is
        // the whole reason a filter dial is logarithmic: sqrt(20 * 1200).
        assert_eq!(knob_reading(hp, 0.5), "155 Hz");

        // The limiter is a threshold, linear in decibels: taking the log of
        // something that already is one would put the useful half of the dial
        // in the last eighth of the travel.
        let th = KnobUnit::Decibels {
            low: 0.0,
            high: -30.0,
        };
        assert_eq!(knob_reading(th, 0.0), "0.0 dB");
        assert_eq!(knob_reading(th, 0.5), "-15.0 dB");
        assert_eq!(knob_reading(th, 1.0), "-30.0 dB");

        // The low-pass runs backwards - up is darker - and still reads right.
        let lp = KnobUnit::Hertz {
            low: 20_000.0,
            high: 200.0,
        };
        assert_eq!(knob_reading(lp, 0.0), "20 kHz");
        assert_eq!(knob_reading(lp, 1.0), "200 Hz");
        assert_eq!(knob_reading(lp, 0.5), "2 kHz");
    }

    /// Typing into a filter is typing a frequency, and it lands there.
    #[test]
    fn a_typed_frequency_lands_on_the_right_part_of_the_dial() {
        let hp = KnobUnit::Hertz {
            low: 20.0,
            high: 1_200.0,
        };
        // Round trip: whatever is typed reads back as itself.
        for text in ["20", "155", "480", "1.2k", "1200 Hz", "800hz"] {
            let t = knob_typed(hp, text).expect("{text} was refused");
            let back = knob_reading(hp, t);
            let want: f32 = text
                .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.' && c != 'k')
                .trim_end_matches('k')
                .parse()
                .unwrap();
            let want = if text.contains('k') { want * 1_000.0 } else { want };
            let got: f32 = back
                .split_whitespace()
                .next()
                .unwrap()
                .parse()
                .unwrap();
            let got = if back.contains('k') { got * 1_000.0 } else { got };
            assert!(
                (got - want).abs() / want < 0.02,
                "{text} came back as {back}"
            );
        }
        // A threshold is typed in decibels, with or without the unit.
        let th = KnobUnit::Decibels {
            low: 0.0,
            high: -30.0,
        };
        for text in ["-6", "-6 dB", "-6dB", " -6 "] {
            let t = knob_typed(th, text).unwrap_or_else(|| panic!("{text} was refused"));
            assert_eq!(knob_reading(th, t), "-6.0 dB", "{text}");
        }
        assert_eq!(knob_typed(th, "0"), Some(0.0));
        // Past either end it pins, and a positive threshold is not a thing
        // this dial has.
        assert_eq!(knob_typed(th, "-90"), Some(1.0));
        assert_eq!(knob_typed(th, "+6"), Some(0.0));
        assert_eq!(knob_typed(th, "loud"), None);

        // Off the ends, a real wish gets the nearest thing this dial has
        // rather than a refusal.
        assert_eq!(knob_typed(hp, "5"), Some(0.0));
        assert_eq!(knob_typed(hp, "9000"), Some(1.0));
        // And nonsense is refused rather than read as silence.
        assert_eq!(knob_typed(hp, "abc"), None);
        assert_eq!(knob_typed(hp, ""), None);
        assert_eq!(knob_typed(hp, "-40"), None);
        assert_eq!(knob_typed(hp, "0"), None);
    }

    #[test]
    fn a_knob_reads_zero_at_the_bottom_and_one_at_the_top() {
        let r = band(1300.0);
        let v = idle();
        // The dial, not the whole cell: the word above it is the type field.
        let cell = {
            let c = Layout::new(r, &v).fx[Fx::Reverb.index()];
            let label = knob_face(c).map_or(0.0, |f| f.label_h);
            Rect::from_min_max(Pos2::new(c.left(), c.top() + label), c.max)
        };
        assert!(cell.is_positive(), "the knob has no rectangle to drag in");
        let at = |y: f32| hit_test(r, &v, Pos2::new(cell.center().x, y));
        // Half a point in from the bottom edge, which is the last point inside
        // the rect — so this is "as low as the pointer can get", not exactly 0.
        let Some(Hit::SetFx(Fx::Reverb, low)) = at(cell.bottom() - 0.5) else {
            panic!("the bottom of the knob is not the knob")
        };
        // **A point of travel, not a fixed 2%.** The knobs are half the height
        // they were now that there are six of them in two rows, so half a point
        // is twice the fraction of the travel it used to be — an absolute
        // tolerance here fails on a layout change rather than on a bug.
        assert!(
            low <= 1.0 / cell.height(),
            "the bottom of the travel reads {low} in a cell {} high",
            cell.height()
        );
        assert_eq!(at(cell.top()), Some(Hit::SetFx(Fx::Reverb, 1.0)));
        // And the middle is the middle, not an end.
        let Some(Hit::SetFx(Fx::Reverb, mid)) = at(cell.center().y) else {
            panic!("the middle of the knob is not the knob")
        };
        assert!((mid - 0.5).abs() < 0.05, "the centre reads {mid}");
    }

    /// **The camera sits in equal margins.** It is the inset a take carries,
    /// so it is a picture rather than a control, and a picture hung with a
    /// different gap under it than beside it reads as an accident. The status
    /// line used to run underneath it, which is what made the bottom margin
    /// twice the others.
    #[test]
    fn the_preview_has_the_same_margin_above_left_and_below_it() {
        for w in [500.0_f32, 900.0, 1300.0, 1800.0] {
            let r = band(w);
            for v in [idle(), rolling()] {
                let l = Layout::new(r, &v);
                assert!(l.preview.is_positive(), "no preview at {w}pt");
                let left = l.preview.left() - r.left();
                let top = l.preview.top() - r.top();
                let bottom = r.bottom() - l.preview.bottom();
                for (name, got) in [("top", top), ("bottom", bottom)] {
                    assert!(
                        (got - left).abs() < 0.5,
                        "at {w}pt the preview has {left:.1} to the left and {got:.1} {name}"
                    );
                }
                // And it really is 16:9, which is what makes it the shape of
                // the file rather than the shape of whatever was left over.
                let aspect = l.preview.width() / l.preview.height();
                assert!(
                    (aspect - PREVIEW_ASPECT).abs() < 0.02 || l.preview.width() < r.width() * 0.24,
                    "the preview is {aspect:.2}:1 at {w}pt"
                );
            }
        }
    }

    /// Nothing else may sit in the preview's column — that is what the equal
    /// margin means. The status line ran under it before.
    #[test]
    fn nothing_shares_the_previews_column() {
        let r = band(1300.0);
        let v = idle();
        let l = Layout::new(r, &v);
        for (name, other) in [
            ("status", l.status),
            ("setup", l.setup),
            ("name", l.name),
            ("tempo", l.tempo),
            ("meter", l.meter),
            ("reverb", l.fx[Fx::Reverb.index()]),
            ("limiter", l.fx[Fx::Limiter.index()]),
        ] {
            if !other.is_positive() {
                continue;
            }
            assert!(
                other.left() >= l.preview.right(),
                "{name} overlaps the preview: it starts at {:.1} and the preview ends at {:.1}",
                other.left(),
                l.preview.right()
            );
        }
    }

    /// The controls that stay reachable once a take is live, with a full rack.
    ///
    /// The three editor buttons are in the list and the three PICKERS are not,
    /// which is the whole distinction: reaching a preset in a plugin's own
    /// window between two passes is a real thing, and loading a plugin blocks
    /// the main thread for seconds and would cost the take.
    ///
    /// The two effect knobs are in it for the same reason the faders are: they
    /// are a mix, they cost the audio thread nothing to move, and riding the
    /// reverb through a take is a thing a person does on purpose.
    const SURVIVORS: [Hit; 38] = [
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
        // The master most of all: it is the level control somebody reaches
        // for at 0:47, and a take is exactly when.
        Hit::SetMaster(0.0),
        // The backing track's level, but not its import button: see `targets`.
        Hit::SetTrackGain(0.0),
        // Pressing the VU puts its clip lamp out, and a clip during a take is
        // exactly when somebody does that.
        Hit::DismissClip,
        // **And every number that belongs to something that survives.** A
        // level you can drag mid-take is a level you can type mid-take; the
        // tempo is not here because its knob becomes the clock when a take
        // starts, so there is no word left to press.
        Hit::Type(NumField::Metronome),
        Hit::Type(NumField::Input),
        Hit::Type(NumField::Track),
        Hit::Type(NumField::Master),
        Hit::Type(NumField::Slot(0)),
        Hit::Type(NumField::Slot(1)),
        Hit::Type(NumField::Slot(2)),
        Hit::Type(NumField::Slot(3)),
        Hit::Type(NumField::Slot(4)),
        Hit::Type(NumField::Fx(Fx::Reverb)),
        Hit::Type(NumField::Fx(Fx::Delay)),
        Hit::Type(NumField::Fx(Fx::Chorus)),
        Hit::Type(NumField::Fx(Fx::Hpf)),
        Hit::Type(NumField::Fx(Fx::Lpf)),
        Hit::Type(NumField::Fx(Fx::Limiter)),
        Hit::SetFx(Fx::Reverb, 0.0),
        Hit::SetFx(Fx::Delay, 0.0),
        Hit::SetFx(Fx::Chorus, 0.0),
        // The filters and the limiter survive a take for the same reason the
        // sends do: they are the sound, and a take is when somebody wants to
        // change it.
        Hit::SetFx(Fx::Hpf, 0.0),
        Hit::SetFx(Fx::Lpf, 0.0),
        Hit::SetFx(Fx::Limiter, 0.0),
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
        assert_eq!(band_height(1300.0), 190.0);
        assert_eq!(band_height(650.0), 95.0);
        // Truncated, not rounded, like every other band: 190 * 1000/1300 is
        // 146.1, and a half-pixel band puts every row on a fractional line.
        assert_eq!(band_height(1000.0), 146.0);
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
                        input_monitor: false,
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
                            k.control().label()
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
                input_monitor: false,
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
                                input_monitor: false,
                                hide_elapsed: hide,
                                ..with_rack(state, rack)
                            };
                            let t = Layout::new(r, &v).targets();
                            for i in 0..t.len() {
                                for j in (i + 1)..t.len() {
                                    assert!(
                                        !t[i].0.intersects(t[j].0),
                                        "{} and {} overlap at {w}pt in {state:?}: {:?} {:?}",
                                        t[i].1.control().label(),
                                        t[j].1.control().label(),
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
                        ("count-in", l.count_in),
                        ("tempo", l.tempo),
                        ("export", l.export),
                        ("status", l.status),
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
                // Everything the band no longer carries itself. The take
                // NAME joined them when the words column went: it is set once
                // a session, and the band is what your hands are on while a
                // take runs. `SetTempo` is here because the tempo BOX is a
                // control and the value is not — see `Hit::EditTempo`.
                const IN_THE_MENU: [Hit; 7] = [
                    Hit::ChooseFolder,
                    Hit::RevealFolder,
                    Hit::ToggleOpenWhenDone,
                    Hit::CycleCountIn,
                    Hit::Export,
                    Hit::EditTimeSignature,
                    Hit::NameField,
                ];
                // `DismissClip` is a target only while the warning is on
                // screen, which the idle fixture is not — see
                // `the_clip_warning_can_be_dismissed_only_while_it_is_showing`,
                // which is where that half is proved.
                // A `Type` target is the WORD over a knob or the number
                // beside a fader, and below a certain band height neither is
                // drawn — see `knob_face`. Covered at a readable size by
                // `every_number_can_be_typed_into_where_it_is_written`.
                // A `Type` target is the WORD over a knob or the number
                // beside a fader, and below a certain band height neither is
                // drawn — see `knob_face`. It has a rectangle at readable
                // sizes, so it belongs in neither branch here; it is covered
                // by `every_number_can_be_typed_into_where_it_is_written`.
                if matches!(want, Hit::Type(_)) {
                    continue;
                }
                if IN_THE_MENU.iter().any(|m| m.is_same_control(want))
                    || want == Hit::ToggleMetronomeInTake
                {
                    assert!(
                        l.targets().into_iter().all(|(rect, k)| {
                            !k.control().is_same_control(want) || !rect.is_positive()
                        }),
                        "{} grew a rectangle in the band again",
                        want.label()
                    );
                    continue;
                }
                let (rect, _) = l
                    .targets()
                    .into_iter()
                    .find(|(_, k)| k.control().is_same_control(want))
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
                    input_monitor: false,
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
                input_monitor: false,
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
        // **Nothing at rest, whatever the setting says.** A clock that can
        // only ever read 0:00 is furniture: every state with a number in it —
        // the count-in, rolling, finishing — is `is_active`, and those take
        // the whole column above the transport row.
        assert!(
            !hidden(RecordState::Idle).is_positive(),
            "a clock at rest, with only one thing it could ever say"
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
        assert!(!at_rest.is_positive() && live.is_positive());

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
        // **The clock is the one thing that moves, and it grows.** At rest it
        // says 0:00 and takes what is left of the transport's row; while a
        // take runs it is the number somebody reads from a piano bench, and
        // the name and tempo boxes it replaces cannot be touched mid-take
        // anyway.
        assert!(
            live.timecode.area() > at_rest.timecode.area() * 2.0,
            "the clock did not take the column: {:?} vs {:?}",
            live.timecode,
            at_rest.timecode
        );

        // **And the preview does NOT collapse.** It used to, back when it was
        // a framing check somebody glanced at before a take. A take records
        // the window now, so this box is the camera inset the recording will
        // carry, and the moment it matters most is the moment it used to
        // shrink.
        assert_eq!(
            live.preview, at_rest.preview,
            "the preview moved when the take started"
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
        // The worst case on the whole sweep, found rather than assumed.
        let (worst, longest) = (0..=100)
            .map(|i| {
                let f = i as f32 / 100.0;
                (f, gain_text(fader_to_gain(f)))
            })
            .max_by_key(|(_, t)| t.chars().count())
            .expect("the sweep is not empty");
        assert_eq!(longest.chars().count(), 8, "{longest} is a new worst case");
        let worst_gain = fader_to_gain(worst);

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
                    // Through `fader_reading`, which is what the painter uses:
                    // the box may drop the unit rather than the number.
                    let (widest, size) = fader_reading(val, worst_gain);
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
        // What is typed and never dragged: a time signature, which has no
        // continuum between 4/4 and 7/8 to sweep along, and the `Type` targets
        // themselves — the number at the end of a fader and the word over a
        // knob, which exist to be typed into and nothing else.
        assert!(
            typed_only
                .iter()
                .all(|h| matches!(h, Hit::EditTimeSignature | Hit::Type(_))),
            "{typed_only:?}"
        );
        assert!(typed_only.contains(&Hit::EditTimeSignature));
    }

    /// The four fields are four fields, not one used four times.
    ///
    /// **`Type` is excluded, and deliberately.** A `Type` hit names the same
    /// field as the control it sits on — that is the whole point of it, and
    /// counting both would make every field look shared with itself.
    #[test]
    fn each_control_owns_its_own_field() {
        let fields: Vec<_> = Hit::ALL
            .into_iter()
            .filter(|h| !matches!(h, Hit::Type(_)))
            .filter_map(num_field)
            .collect();
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

    #[test]
    fn the_export_line_says_what_a_take_will_produce() {
        use crate::recorder::VideoMode;
        assert_eq!(
            export_summary(&ExportSpec {
                video: VideoMode::None,
                ..Default::default()
            }),
            "wav + midi"
        );
        // The shipped default is not that one. A take records the window.
        assert_eq!(export_summary(&ExportSpec::default()), "wav + midi + 1 video");
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
                video: VideoMode::None,
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
        let loaded_track = crate::ports::TrackInfo {
            name: "backing.mp3".to_owned(),
            seconds: 214.0,
            wave: (0..1000)
                .map(|i| ((i as f32 / 60.0).sin().abs() * 0.9).min(1.0))
                .collect(),
            error: String::new(),
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
                                input_monitor: false,
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
                                    inputs: [0.5; crate::recorder::INPUTS],
                                    master: if dark { 1.0 } else { 0.35 },
                                    track: 0.7,
                                    fx_return: 1.0,
                                },
                                // Both ends of each knob's travel, so a slot or
                                // a pointer that escapes at one extreme is
                                // caught rather than only being drawn mid-way.
                                fx: crate::recorder::FxSends {
                                    reverb: 0.0,
                                    delay: 1.0,
                                    chorus: 0.5,
                                    hpf: 0.35,
                                    lpf: 1.0,
                                    limiter: 0.7,
                                },
                                // The filters read in hertz, so the sweep
                                // also covers the longest string a knob can
                                // be asked to fit.
                                // Loud enough to be in the red on one pass
                                // and quiet on the other, with the limiter
                                // visibly working: the sweep has to cover a
                                // meter that is doing something.
                                master: Meters {
                                    left: Level {
                                        peak: if dark { 0.98 } else { 0.06 },
                                        rms: if dark { 0.7 } else { 0.03 },
                                        hold: if dark { 1.0 } else { 0.09 },
                                    },
                                    right: Level {
                                        peak: if dark { 0.55 } else { 0.02 },
                                        rms: if dark { 0.4 } else { 0.01 },
                                        hold: if dark { 0.6 } else { 0.04 },
                                    },
                                    mono: false,
                                    clipped: dark,
                                },
                                gr_db: if dark { 7.5 } else { 0.0 },
                                // A loaded track on one pass and none on the
                                // other, so the sweep covers both the row that
                                // is an offer and the row that is a control.
                                track: if dark { &loaded_track } else { crate::ports::TrackInfo::NONE },
                                fx_units: [
                                    KnobUnit::Percent,
                                    KnobUnit::Percent,
                                    KnobUnit::Percent,
                                    KnobUnit::Hertz { low: 20.0, high: 1_200.0 },
                                    KnobUnit::Hertz { low: 20_000.0, high: 200.0 },
                                    KnobUnit::Decibels { low: 0.0, high: -30.0 },
                                ],
                                // A knob mid-turn, so the sweep also covers a
                                // knob showing a number instead of its name.
                                turning: Some(NumField::Fx(Fx::Reverb)),
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

// ── The take-settings popup ─────────────────────────────────────────────────
//
// **The controls that left the band, back as controls.** They spent one
// release as menu rows, and a menu row is a poor home for a folder path you
// want to READ, a device that may say "(not connected)", or three ticks whose
// current state is the question. Every one of them is a box with a caption and
// a value again — the same boxes the band drew, in the same ink, laid out with
// room to breathe rather than squeezed into a column between the transport and
// the instruments.
//
// A popup and not a band, because that is what they are: set once at the start
// of a session, and in the way for the rest of it. It hangs off the cog in the
// band and it is drawn by the same painters as everything else here, so it
// works in a window, in a plugin, and in a recording.

/// How wide the popup is, as a share of the window, and the bounds on that.
/// **Small.** The first cut took a third of the window and gave every row
/// forty points of height, which `labelled` turns into type twice the size of
/// the band's own — a settings panel that shouts. It is a utility panel: it
/// wants to be readable and out of the way, at about the weight of the boxes
/// it was summoned from.
const SETUP_W: (f32, f32, f32) = (0.24, 290.0, 430.0);
/// Its height over its width.
const SETUP_ASPECT: f32 = 0.80;
/// Rows of controls under the title.
///
/// **Seven: six about the take, and Setup.** The camera and the audio input
/// left this panel because a device belongs to the control it feeds — the
/// microphone's own icon opens the audio picker and the camera preview opens
/// the camera's (see `input_icon` and `preview_rect`).
///
/// The audio PATH did not, and should not have. It went out with them in
/// 4.19.0 and landed nowhere: `Hit::ShowAudioStatus` was left with no target in
/// any layout, so the panel that says what rate the two streams are running at
/// — the one that exists because a silent 44.1-against-48 mismatch makes takes
/// drift — became unreachable from anywhere in the app. It is back, as the last
/// row, and it is the one row here that is not about a take.
const SETUP_ROWS: usize = 7;

/// Every rectangle in the popup.
struct SetupLayout {
    panel: Rect,
    title: Rect,
    close: Rect,
    /// What the next take is called. It was a box in the band, at the top of a
    /// column that no longer exists; it is set once a session and belongs with
    /// the rest of what a take is.
    name: Rect,
    dest: Rect,
    reveal: Rect,
    folder: Rect,
    disk: Rect,
    count_in: Rect,
    time_sig: Rect,
    export: Rect,
    open_when_done: Rect,
    click_in_take: Rect,
    count_in_in_take: Rect,
    hide_elapsed: Rect,
    /// The way to Setup: the audio system, the devices, the rate, the buffer.
    audio: Rect,
}

impl SetupLayout {
    const NONE: Self = Self {
        panel: Rect::NOTHING,
        title: Rect::NOTHING,
        close: Rect::NOTHING,
        name: Rect::NOTHING,
        dest: Rect::NOTHING,
        reveal: Rect::NOTHING,
        folder: Rect::NOTHING,
        disk: Rect::NOTHING,
        count_in: Rect::NOTHING,
        time_sig: Rect::NOTHING,
        export: Rect::NOTHING,
        open_when_done: Rect::NOTHING,
        hide_elapsed: Rect::NOTHING,
        click_in_take: Rect::NOTHING,
        count_in_in_take: Rect::NOTHING,
        audio: Rect::NOTHING,
    };

    /// Hung off `anchor` — the cog — and pulled back onto `screen`.
    ///
    /// Below and left-aligned with the cog when there is room, which is where a
    /// popup belongs relative to the thing that opened it. The clamp is not a
    /// nicety: the cog sits near the left of a band that can be half a screen
    /// wide, and a panel that ran off the bottom would put the ticks where
    /// nobody could reach them.
    fn new(screen: Rect, anchor: Rect) -> Self {
        if !screen.is_positive() {
            return Self::NONE;
        }
        let w = (screen.width() * SETUP_W.0).clamp(SETUP_W.1, SETUP_W.2.min(screen.width()));
        let h = (w * SETUP_ASPECT).min(screen.height() * 0.80);
        let mut panel = Rect::from_min_size(
            Pos2::new(anchor.left(), anchor.bottom() + h * 0.04),
            Vec2::new(w, h),
        );
        let dx = (screen.left() - panel.left()).max(0.0) - (panel.right() - screen.right()).max(0.0);
        let dy = (screen.top() - panel.top()).max(0.0) - (panel.bottom() - screen.bottom()).max(0.0);
        panel = panel.translate(Vec2::new(dx, dy));

        let pad = (h * 0.055).clamp(4.0, 16.0);
        let body = panel.shrink(pad);
        if !body.is_positive() {
            return Self {
                panel,
                ..Self::NONE
            };
        }
        // The title keeps a band of its own at the top, with the way out at the
        // right of it. Everything else is seven rows of equal height, because a
        // dialog whose rows are all different sizes reads as a form somebody
        // assembled rather than as a panel.
        let title_h = body.height() * 0.13;
        let title = Rect::from_min_max(
            body.min,
            Pos2::new(body.right(), body.top() + title_h),
        );
        let close_w = (title.height() * 1.1).min(body.width() * 0.16);
        let close = Rect::from_min_max(
            Pos2::new(title.right() - close_w, title.top()),
            title.max,
        );
        let rows = Rect::from_min_max(Pos2::new(body.left(), title.bottom()), body.max);
        // A real gap between rows: `Rect::contains` is inclusive at both edges,
        // so two rows that merely touch share a line of pixels and every
        // overlap test in this file fails there.
        let row = |i: usize| {
            let pitch = 1.0 / SETUP_ROWS as f32;
            let top = i as f32 * pitch;
            slice_v(rows, top + pitch * 0.10, top + pitch * 0.92)
        };

        let r0 = row(0);
        let r1 = row(1);
        let note = {
            let r = row(2);
            Rect::from_center_size(r.center(), Vec2::new(r.width(), r.height() * 0.60))
        };
        let r3 = row(3);
        let r4 = row(4);
        let r5 = row(5);
        let r6 = row(6);
        Self {
            panel,
            title,
            close,
            // What the take is called: the first thing about it, and the first
            // row here.
            name: r0,
            // Where takes go, and how to see it.
            //
            // **The folder gets the width the "Default" tick used to have.**
            // That tick set a flag nothing read: the folder was written to
            // settings and reused next launch whether it was on or off, so it
            // was a switch wired to itself, sitting on the one row that needed
            // the room. A path is long and it is the thing being read.
            dest: slice_h(r1, 0.00, 0.79),
            reveal: slice_h(r1, 0.82, 1.00),
            // What the next take will be called, and whether it will fit.
            // Answers rather than questions, so they carry no box — and a
            // HALF-HEIGHT row, because `text_line` sizes itself to whatever it
            // is given and a full row made the one line nobody clicks the
            // biggest thing in the panel.
            folder: slice_h(note, 0.00, 0.60),
            disk: slice_h(note, 0.62, 1.00),
            // How long a bar is, how many of them you get, and what comes out
            // at the end. The three that describe the take itself.
            count_in: slice_h(r3, 0.00, 0.30),
            time_sig: slice_h(r3, 0.33, 0.55),
            export: slice_h(r3, 0.58, 1.00),
            // The four questions with yes-or-no answers, in two rows of two, so
            // a tick is never the only thing on a line.
            open_when_done: slice_h(r4, 0.00, 0.49),
            click_in_take: slice_h(r4, 0.51, 1.00),
            count_in_in_take: slice_h(r5, 0.00, 0.49),
            hide_elapsed: slice_h(r5, 0.51, 1.00),
            // **The whole width, and alone on its row.** Everything above is
            // about the take that is about to be made; this is about the path
            // it will be made through, and a row to itself is the cheapest way
            // to say so in a panel with no headings.
            audio: r6,
        }
    }

    /// Every clickable region and what it means. The popup's [`Layout::targets`].
    fn targets(&self) -> [(Rect, Hit); 10] {
        [
            (self.name, Hit::NameField),
            (self.close, Hit::CloseSetup),
            (self.dest, Hit::ChooseFolder),
            (self.reveal, Hit::RevealFolder),
            (self.count_in, Hit::CycleCountIn),
            (self.time_sig, Hit::EditTimeSignature),
            (self.export, Hit::Export),
            (self.open_when_done, Hit::ToggleOpenWhenDone),
            (self.click_in_take, Hit::ToggleMetronomeInTake),
            (self.audio, Hit::ShowAudioStatus),
        ]
    }
}

// ── the effect panels ───────────────────────────────────────────────────────
//
// A right-click on a knob opens the effect behind it. The knob itself is one
// number — how much — because during a take that is the only one anybody
// reaches for; everything that shapes the sound lives here, where it can be
// read and set once and left alone.
//
// One layout for all three, because they are the same shape: a title, four
// rows, a Reset. What differs is what the rows are CALLED and, for the delay's
// first row, that it steps through named divisions rather than sliding.

/// Which effect a panel belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fx {
    Reverb,
    Delay,
    Chorus,
    Hpf,
    Lpf,
    Limiter,
}

impl Fx {
    /// **In drawn order: the top row, then the bottom row.** Three sends over
    /// three things that shape the whole output. Anything walking all six
    /// walks them in the order a hand finds them.
    pub const ALL: [Fx; 6] = [
        Fx::Reverb,
        Fx::Delay,
        Fx::Chorus,
        Fx::Hpf,
        Fx::Lpf,
        Fx::Limiter,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Fx::Reverb => "REVERB",
            Fx::Delay => "DELAY",
            Fx::Chorus => "CHORUS",
            Fx::Hpf => "HPF",
            Fx::Lpf => "LPF",
            Fx::Limiter => "LIMITER",
        }
    }

    /// A line under the title saying what the thing IS. Two of these name real
    /// hardware, because that is the fastest way to say what a sound is to
    /// somebody who has heard one.
    pub fn subtitle(self) -> &'static str {
        match self {
            Fx::Reverb => "eight combs into four allpasses",
            Fx::Delay => "in time with the tempo",
            Fx::Chorus => "true stereo, after the Boss CE-1",
            Fx::Hpf => "takes the bottom off, after everything",
            Fx::Lpf => "takes the top off - up is darker",
            Fx::Limiter => "true peak, held under the threshold",
        }
    }

    /// The knob this panel hangs off.
    pub fn hit(self) -> Hit {
        Hit::SetFx(self, 0.0)
    }

    /// Where this knob's own value lives in the settings file.
    ///
    /// The rows in [`Fx::rows`] are the effect's PARAMETERS; this is the knob
    /// itself. Named here for the same reason they are: it is the string the
    /// host answers to, so it cannot be spelled differently in two places.
    pub fn mix_key(self) -> &'static str {
        match self {
            Fx::Reverb => "reverb_mix",
            Fx::Delay => "delay_mix",
            Fx::Chorus => "chorus_mix",
            Fx::Hpf => "hpf_mix",
            Fx::Lpf => "lpf_mix",
            Fx::Limiter => "limiter_mix",
        }
    }

    /// Its position in [`Fx::ALL`], which is its cell in the layout.
    pub fn index(self) -> usize {
        Fx::ALL.iter().position(|&x| x == self).unwrap_or(0)
    }

    /// What the status line says while a hand is on it.
    pub fn describe(self) -> &'static str {
        match self {
            Fx::Reverb => "Reverb  -  shift right-click for its parameters",
            Fx::Delay => "Delay  -  shift right-click for its parameters",
            Fx::Chorus => "Chorus  -  shift right-click for its parameters",
            Fx::Hpf => "High-pass  -  shift right-click for its parameters",
            Fx::Lpf => "Low-pass  -  shift right-click for its parameters",
            Fx::Limiter => "Limiter threshold  -  shift right-click for more",
        }
    }

    /// The colour of the knob's cap.
    ///
    /// **The bottom row is not the top row.** Reverb, delay and chorus are
    /// sends — things added to a sound. The filters and the limiter change
    /// what leaves, and the limiter is the one that will alter a take whether
    /// or not anybody is listening for it, so it is the one that is red.
    pub fn cap(self) -> Color32 {
        match self {
            Fx::Reverb | Fx::Delay | Fx::Chorus => KNOB_CAP,
            Fx::Hpf | Fx::Lpf => FILTER_CAP,
            Fx::Limiter => LIMITER_CAP,
        }
    }

    /// The four rows, in order.
    ///
    /// **Keys, not indices.** They are what goes in the settings file and what
    /// the host reads back, so a row that is renamed or reordered here cannot
    /// silently start writing to a different parameter.
    pub fn rows(self) -> [FxRow; 4] {
        const fn slide(key: &'static str, label: &'static str) -> FxRow {
            FxRow { key, label, step: false }
        }
        const fn step(key: &'static str, label: &'static str) -> FxRow {
            FxRow { key, label, step: true }
        }
        match self {
            Fx::Reverb => [
                slide("reverb_size", "Size"),
                slide("reverb_damp", "Damping"),
                slide("reverb_width", "Width"),
                FxRow::NONE,
            ],
            Fx::Delay => [
                step("delay_division", "Time"),
                slide("delay_feedback", "Repeats"),
                slide("delay_tone", "Tone"),
                slide("delay_width", "Width"),
            ],
            Fx::Chorus => [
                slide("chorus_rate", "Rate"),
                slide("chorus_depth", "Depth"),
                slide("chorus_width", "Width"),
                slide("chorus_tone", "Tone"),
            ],
            Fx::Hpf => [
                step("hpf_slope", "Slope"),
                slide("hpf_resonance", "Resonance"),
                FxRow::NONE,
                FxRow::NONE,
            ],
            Fx::Lpf => [
                step("lpf_slope", "Slope"),
                slide("lpf_resonance", "Resonance"),
                FxRow::NONE,
                FxRow::NONE,
            ],
            Fx::Limiter => [
                slide("limiter_release", "Release"),
                slide("limiter_knee", "Knee"),
                FxRow::NONE,
                FxRow::NONE,
            ],
        }
    }
}

/// One row of an effect's parameter panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxRow {
    /// The settings key this row writes. Empty means the row is not there.
    pub key: &'static str,
    pub label: &'static str,
    /// Whether it steps through a named list rather than sliding along a
    /// track. There is no position along a track that means "a dotted eighth",
    /// and none that means "24 dB an octave" either.
    pub step: bool,
}

impl FxRow {
    /// A row that is not there. Panels have between two and four.
    pub const NONE: Self = Self {
        key: "",
        label: "",
        step: false,
    };
}

/// A row of an effect panel, and how it is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FxHit {
    /// Drag along the row to set `key` to 0..=1.
    Set { key: &'static str, value: f32 },
    /// A named parameter: step to the next value in its list.
    NextChoice { key: &'static str },
    /// Put this effect back to what it shipped as.
    Reset(Fx),
    Close,
}

const FX_W: (f32, f32, f32) = (0.20, 250.0, 360.0);
const FX_ASPECT: f32 = 0.78;

/// Where everything in an effect panel goes.
struct FxLayout {
    panel: Rect,
    title: Rect,
    close: Rect,
    /// One per row of [`Fx::rows`], empty ones included so the indices line up.
    rows: [Rect; 4],
    reset: Rect,
}

impl FxLayout {
    const NONE: Self = Self {
        panel: Rect::NOTHING,
        title: Rect::NOTHING,
        close: Rect::NOTHING,
        rows: [Rect::NOTHING; 4],
        reset: Rect::NOTHING,
    };

    /// Hung off the knob that opened it, and clamped onto the screen.
    ///
    /// Below the knob when there is room and above it when there is not: the
    /// knobs sit in the top band, so "below" is nearly always right, and the
    /// clamp is what stops a panel running off the bottom of a short window.
    fn new(screen: Rect, anchor: Rect) -> Self {
        if !screen.is_positive() || !anchor.is_positive() {
            return Self::NONE;
        }
        let w = (screen.width() * FX_W.0).clamp(FX_W.1, FX_W.2.min(screen.width()));
        let h = (w * FX_ASPECT).min(screen.height() * 0.80);
        // Centred on the knob rather than left-aligned to it: a knob is thirty
        // points wide and a panel is three hundred, so hanging one off the
        // left edge of the other puts it visibly off to one side.
        let mut panel = Rect::from_min_size(
            Pos2::new(anchor.center().x - w * 0.5, anchor.bottom() + h * 0.05),
            Vec2::new(w, h),
        );
        let dx = (screen.left() - panel.left()).max(0.0) - (panel.right() - screen.right()).max(0.0);
        let dy = (screen.top() - panel.top()).max(0.0) - (panel.bottom() - screen.bottom()).max(0.0);
        panel = panel.translate(Vec2::new(dx, dy));

        let pad = (h * 0.06).clamp(4.0, 14.0);
        let body = panel.shrink(pad);
        if !body.is_positive() {
            return Self {
                panel,
                ..Self::NONE
            };
        }
        let title_h = body.height() * 0.20;
        let title = Rect::from_min_max(body.min, Pos2::new(body.right(), body.top() + title_h));
        // Square, at the top right of the title band. A word would need three
        // times the width and this panel does not have it — and an X in the
        // corner is the one piece of chrome nobody has to be taught.
        let close_w = (title.height() * 0.52).min(body.width() * 0.16);
        let close = Rect::from_min_size(
            Pos2::new(title.right() - close_w, title.top()),
            Vec2::splat(close_w),
        );
        // Four rows and the Reset, evenly. Equal heights because a panel whose
        // rows are all different sizes reads as a form somebody assembled.
        let rest = Rect::from_min_max(Pos2::new(body.left(), title.bottom()), body.max);
        let pitch = rest.height() / 5.0;
        let row = |i: usize| {
            Rect::from_min_size(
                Pos2::new(rest.left(), rest.top() + pitch * i as f32),
                Vec2::new(rest.width(), pitch * 0.86),
            )
        };
        Self {
            panel,
            title,
            close,
            rows: [row(0), row(1), row(2), row(3)],
            // Right-aligned and narrow: it is the one destructive thing here
            // and it should not look like another row of settings.
            reset: {
                let r = row(4);
                Rect::from_min_max(
                    Pos2::new(r.right() - r.width() * 0.36, r.top()),
                    r.max,
                )
            },
        }
    }

    /// The draggable track inside a row: the part after the label.
    fn track(row: Rect) -> Rect {
        slice_h(row, FX_TRACK.0, FX_TRACK.1)
    }
}

/// Where a row's track starts and ends, as a share of the row.
///
/// The label takes the left and the reading takes the right, exactly as a
/// fader row does — so the two kinds of control in this app look like each
/// other rather than like two people's work.
const FX_TRACK: (f32, f32) = (0.40, 0.86);

/// Where an effect panel goes, for a caller that has to swallow presses.
pub fn fx_popup_rect(screen: Rect, anchor: Rect) -> Rect {
    FxLayout::new(screen, anchor).panel
}

/// What a press inside an effect panel means.
///
/// `None` for a press on the panel's own chrome, which the caller still
/// swallows: a panel you can click through is one that closes when you meant
/// to press something in it.
pub fn fx_hit_test(screen: Rect, anchor: Rect, fx: Fx, pos: Pos2) -> Option<FxHit> {
    let l = FxLayout::new(screen, anchor);
    if !l.panel.contains(pos) {
        return None;
    }
    if l.close.contains(pos) {
        return Some(FxHit::Close);
    }
    if l.reset.contains(pos) {
        return Some(FxHit::Reset(fx));
    }
    for (i, row) in fx.rows().into_iter().enumerate() {
        if row.key.is_empty() || !l.rows[i].contains(pos) {
            continue;
        }
        if row.step {
            return Some(FxHit::NextChoice { key: row.key });
        }
        let track = FxLayout::track(l.rows[i]);
        return Some(FxHit::Set {
            key: row.key,
            value: along(track, pos),
        });
    }
    None
}

/// Which row of `fx` a point is on, for a caller continuing a drag.
///
/// A drag inside a panel has to keep setting the row it STARTED on, even once
/// the pointer has slid onto the row above — the same reason `is_same_control`
/// exists for the band.
pub fn fx_row_at(screen: Rect, anchor: Rect, fx: Fx, pos: Pos2) -> Option<&'static str> {
    // A stepped row is a row. The question this answers is "which parameter is
    // under the pointer", and answering `None` over the Slope row would say
    // there is nothing there.
    match fx_hit_test(screen, anchor, fx, pos)? {
        FxHit::Set { key, .. } | FxHit::NextChoice { key } => Some(key),
        _ => None,
    }
}

/// The value a point along `key`'s row would set, wherever the pointer is.
///
/// Clamped into the track rather than refused, so a hand that slides off the
/// end of a row pins the value instead of dropping the gesture.
pub fn fx_value_at(screen: Rect, anchor: Rect, fx: Fx, key: &str, pos: Pos2) -> Option<f32> {
    let l = FxLayout::new(screen, anchor);
    let i = fx.rows().iter().position(|r| r.key == key)?;
    let track = FxLayout::track(l.rows[i]);
    track.is_positive().then(|| along(track, pos))
}

/// Where the take-settings popup goes, given the window and the cog.
///
/// [`Rect::NOTHING`] when there is no room for it. The app needs this to know
/// whether a press landed inside the popup or outside it, which is the whole of
/// "click away to dismiss".
pub fn setup_popup_rect(screen: Rect, anchor: Rect) -> Rect {
    SetupLayout::new(screen, anchor).panel
}

/// What a press inside the popup means, or `None` for a press on its chrome.
///
/// A press anywhere in the panel that is not a control returns `None` and is
/// still SWALLOWED by the caller — a dialog you can click through is one that
/// closes when you meant to press something in it.
pub fn setup_hit_test(
    screen: Rect,
    anchor: Rect,
    view: &RecorderView<'_>,
    pos: Pos2,
) -> Option<Hit> {
    let l = SetupLayout::new(screen, anchor);
    if !l.panel.contains(pos) {
        return None;
    }
    let rolling = view.state.is_active();
    l.targets()
        .into_iter()
        // Nothing here is set MID-TAKE. Every one of these decides how the
        // file being written right now was going to be written, and a picker
        // that swaps the camera under a running encoder is not a setting, it
        // is a crash with a caption.
        .find(|(r, _)| !rolling && r.contains(pos))
        .map(|(_, h)| h)
        .or_else(|| l.close.contains(pos).then_some(Hit::CloseSetup))
        .or({
            // Extra rows that are only reachable here.
            let extra = [
                (l.count_in_in_take, Hit::ToggleCountInInTake),
                (l.hide_elapsed, Hit::ToggleHideElapsed),
            ];
            extra
                .into_iter()
                .find(|(r, _)| r.contains(pos))
                .map(|(_, h)| h)
        })
}

/// Draw the take-settings popup over `screen`, hung off the cog at `anchor`.
/// Draw an effect's panel over `screen`, hung off the knob at `anchor`.
///
/// `values` answers what each key is set to, and `division` is the delay's
/// time. Both come from the caller because this crate does not own the
/// settings file — the same reason every other painter here takes a view.
pub fn draw_fx(
    painter: &Painter,
    screen: Rect,
    anchor: Rect,
    fx: Fx,
    values: &dyn Fn(&str) -> f32,
    // `choice`: the label a stepped row shows, for its key. See `FxRow::step`.
    choice: &dyn Fn(&str) -> String,
    s: &Settings,
) {
    let l = FxLayout::new(screen, anchor);
    if !l.panel.is_positive() {
        return;
    }
    let p = palette(s);
    // The same scrim the take settings use: it is what says the panel is in
    // FRONT of the window rather than being another band that appeared.
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(96));
    painter.rect_filled(l.panel, 4.0, p.bg);
    painter.rect_stroke(l.panel, 4.0, Stroke::new(1.0_f32, p.ink), StrokeKind::Inside);

    if l.title.is_positive() {
        let size = fit_text(l.title, fx.title(), l.title.height() * 0.42);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(l.title.left(), l.title.top() + l.title.height() * 0.30),
                Align2::LEFT_CENTER,
                fx.title(),
                font(size),
                p.ink,
            );
        }
        // What the thing is, under its name. Faint, because it is read once.
        let sub = fit_text(l.title, fx.subtitle(), l.title.height() * 0.26);
        if sub >= MIN_TEXT {
            painter.text(
                Pos2::new(l.title.left(), l.title.top() + l.title.height() * 0.74),
                Align2::LEFT_CENTER,
                fx.subtitle(),
                font(sub),
                p.faint,
            );
        }
        draw_word_button_sized(painter, l.close, &["X"], 0.34, &p);
    }

    for (i, FxRow { key, label, step }) in fx.rows().into_iter().enumerate() {
        let row = l.rows[i];
        if key.is_empty() || !row.is_positive() {
            continue;
        }
        let size = fit_text(slice_h(row, 0.0, FX_TRACK.0), label, row.height() * 0.52);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(row.left(), row.center().y),
                Align2::LEFT_CENTER,
                label,
                font(size),
                p.ink,
            );
        }
        let track = FxLayout::track(row);
        let reading = Rect::from_min_max(Pos2::new(track.right(), row.top()), row.max);
        if step {
            // A name, in a box, that steps to the next one when pressed. Drawn
            // as a button rather than as a track, because it is one: there is
            // no position along a line that means "a dotted eighth", and none
            // that means "24 dB an octave" either.
            let division = &choice(key);
            let box_r = Rect::from_min_max(
                Pos2::new(track.left(), row.top() + row.height() * 0.14),
                Pos2::new(reading.right(), row.bottom() - row.height() * 0.14),
            );
            painter.rect_filled(box_r, 2.0, p.field);
            let size = fit_text(box_r.shrink(box_r.width() * 0.06), division, box_r.height() * 0.62);
            if size >= MIN_TEXT {
                painter.text(box_r.center(), Align2::CENTER_CENTER, division, font(size), p.ink);
            }
            continue;
        }

        let v = values(key).clamp(0.0, 1.0);
        // The track: a well with the set part filled, exactly as a fader's is.
        let h = (row.height() * 0.22).max(2.0);
        let well = Rect::from_center_size(track.center(), Vec2::new(track.width(), h));
        painter.rect_filled(well, h * 0.5, p.well);
        if v > 0.0 {
            let filled = Rect::from_min_size(well.min, Vec2::new(well.width() * v, h));
            painter.rect_filled(filled, h * 0.5, p.accent);
        }
        // And the handle, so it is obviously a thing to be dragged.
        let knob_w = (row.height() * 0.20).max(3.0);
        let x = well.left() + well.width() * v;
        painter.rect_filled(
            Rect::from_center_size(
                Pos2::new(x.clamp(well.left() + knob_w * 0.5, well.right() - knob_w * 0.5), well.center().y),
                Vec2::new(knob_w, row.height() * 0.66),
            ),
            1.5,
            p.ink,
        );
        let text = format!("{:.0}%", v * 100.0);
        let size = fit_text(reading, &text, row.height() * 0.46);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(reading.right(), row.center().y),
                Align2::RIGHT_CENTER,
                &text,
                font(size),
                p.faint,
            );
        }
    }

    if l.reset.is_positive() {
        let size = fit_text(l.reset, "Reset", l.reset.height() * 0.52);
        if size >= MIN_TEXT {
            painter.rect_stroke(
                l.reset,
                2.0,
                Stroke::new(1.0_f32, p.faint),
                StrokeKind::Inside,
            );
            painter.text(l.reset.center(), Align2::CENTER_CENTER, "Reset", font(size), p.ink);
        }
    }
}

pub fn draw_setup(
    painter: &Painter,
    screen: Rect,
    anchor: Rect,
    view: &RecorderView<'_>,
    s: &Settings,
) {
    let l = SetupLayout::new(screen, anchor);
    if !l.panel.is_positive() {
        return;
    }
    let p = palette(s);

    // A scrim over the whole window. It is what says the panel is modal, and
    // it is what makes a dialog drawn in the app's own ink read as being IN
    // FRONT of the window rather than as another band that appeared.
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(96));
    painter.rect_filled(l.panel, 4.0, p.bg);
    painter.rect_stroke(l.panel, 4.0, Stroke::new(1.0_f32, p.ink), StrokeKind::Inside);

    if l.title.is_positive() {
        let size = fit_text(l.title, "TAKE SETTINGS", l.title.height() * 0.50);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(l.title.left(), l.title.center().y),
                Align2::LEFT_CENTER,
                "TAKE SETTINGS",
                font(size),
                p.ink,
            );
        }
    }
    draw_word_button(painter, l.close, &["DONE", "OK"], &p);

    // **The take name lives here now.** It was a box at the top of a column in
    // the band that no longer exists — and it belongs here anyway: it is set
    // once a session, beside everything else that describes a take, and the
    // band is what your hands are on while one is running.
    //
    // Not required and not unique: the timestamp guarantees that. Type
    // "nocturne" once, press record five times, and get five adjacent folders
    // with no overwrite dialog ever.
    control(painter, l.name, &p);
    {
        let empty = view.take_name.is_empty();
        let shown = if empty { "(optional)" } else { view.take_name };
        label_text(
            painter,
            l.name,
            "NAME",
            shown,
            if empty { p.faint } else { p.ink },
            &p,
        );
        if view.name_focused {
            draw_caption_caret(
                painter,
                l.name,
                "NAME",
                shown,
                view.take_name.chars().count(),
                &p,
            );
        }
    }

    // "Choose..." lives INSIDE the folder box, right-aligned, because the box
    // itself is the target: two adjacent controls that open the same picker is
    // one control too many.
    //
    // **Its width is taken out of the box BEFORE the path is placed.** The
    // band's folder box was wide enough that the two never met; this one is
    // half of a small panel, and the path ran straight under the word. A
    // reserved zone rather than a clip, so the path shrinks to fit the room it
    // really has instead of being cut off mid-directory.
    control(painter, l.dest, &p);
    if l.dest.is_positive() {
        let size = fit_text(l.dest, "Choose...", l.dest.height() * 0.45);
        let gap = if size >= MIN_TEXT {
            let w = painter
                .layout_no_wrap("Choose...".to_owned(), font_light(size), p.faint)
                .rect
                .width();
            painter.text(
                Pos2::new(l.dest.right() - label_inset(l.dest), l.dest.center().y),
                Align2::RIGHT_CENTER,
                "Choose...",
                font_light(size),
                p.faint,
            );
            w + label_inset(l.dest) * 2.0
        } else {
            0.0
        };
        let value = Rect::from_min_max(
            l.dest.min,
            Pos2::new(l.dest.right() - gap, l.dest.bottom()),
        );
        label_text(painter, value, "FOLDER", view.dest, p.ink, &p);
    }
    // SHOW rather than OPEN, for the reason the tick beside Export has: nothing
    // is opened, a folder is shown.
    draw_word_button(painter, l.reveal, &["SHOW FOLDER", "SHOW"], &p);

    text_line(painter, l.folder, view.folder_preview, p.faint, false);
    // Disk as a DURATION. "214 GB free" means nothing to a pianist and "~58
    // min" means everything, which is why the view carries minutes and not
    // bytes in the first place.
    let disk = match view.disk_minutes {
        Some(m) if m >= 90.0 => format!("~{:.0} h left", m / 60.0),
        Some(m) => format!("~{m:.0} min left"),
        None => "measuring free space".to_owned(),
    };
    text_line(painter, l.disk, &disk, p.faint, true);

    labelled(
        painter,
        l.count_in,
        "COUNT-IN",
        &count_in_text(view.count_in_bars),
        p.ink,
        &p,
    );
    {
        let typing = typing_for(view, NumField::Meter);
        let shown = typing.map_or_else(|| view.time_signature.label(), str::to_owned);
        labelled(painter, l.time_sig, "SIG", &shown, p.ink, &p);
        if let Some(typed) = typing {
            draw_caption_caret(painter, l.time_sig, "SIG", &shown, typed.chars().count(), &p);
        }
    }
    labelled(
        painter,
        l.export,
        "EXPORT",
        &export_summary(&s.record_export),
        p.ink,
        &p,
    );

    // The four yes-or-no questions. Each is worded as the STATE it is in, not
    // as the action that changes it, because a tick already says which way it
    // is set and a row that says both is a row you have to read twice.
    for (r, cap, on) in [
        (l.open_when_done, "Show when done", s.record_open_when_done),
        (l.click_in_take, "Click into takes", s.metronome_in_take),
        (l.count_in_in_take, "Count-in into take", s.record_count_in_in_take),
        (l.hide_elapsed, "Hide elapsed time", s.record_hide_elapsed),
    ] {
        draw_tick(painter, r, cap, on, &p);
    }

    // **Setup**, showing the input it is about. The caption is a question the
    // device name answers, so the row is a readout as well as a way in: an
    // interface that has gone missing says so here, in the panel you would open
    // to find out why nothing is being recorded.
    labelled(painter, l.audio, "AUDIO SETUP", view.audio.text(), p.ink, &p);
}



// ── the backing track's waveform ────────────────────────────────────────────
//
// A right-click on the track's icon opens this. The row itself is a level and
// an import button, because that is all a fifteen-point row can be; where the
// track STARTS and STOPS is a question about a picture, and it needs one.

/// The panel's width, as a fraction of the window and in points.
///
/// Wider than an effect panel and deliberately: this one holds a waveform, and
/// a waveform four inches wide is where a person can actually find the bar
/// they meant. The effect panels are four rows of text and want no more room
/// than the text.
const TRACK_W: (f32, f32, f32) = (0.46, 320.0, 720.0);
const TRACK_ASPECT: f32 = 0.44;

/// How wide a trim handle's grab zone is, in points.
///
/// **Fatter than the line it draws.** The line is one point because a fat line
/// hides the waveform under it; a one-point grab target is a control nobody
/// can catch. Fourteen is a comfortable thumb at any window size.
const TRIM_GRAB: f32 = 14.0;

/// The shortest a trimmed track may be, in seconds.
///
/// **Not zero.** Two handles that can meet are two handles that can cross, and
/// what that produces is a track which plays nothing — discovered by pressing
/// Record and hearing silence.
pub const MIN_TRIM: f64 = 0.05;

/// Where everything in the track panel goes.
pub struct TrackLayout {
    pub panel: Rect,
    pub title: Rect,
    pub close: Rect,
    /// The waveform itself. Trim positions are fractions ALONG this.
    pub wave: Rect,
    /// The two typed fields, and the button that clears the trim.
    pub field_in: Rect,
    pub field_out: Rect,
    pub reset: Rect,
}

impl TrackLayout {
    const NONE: Self = Self {
        panel: Rect::NOTHING,
        title: Rect::NOTHING,
        close: Rect::NOTHING,
        wave: Rect::NOTHING,
        field_in: Rect::NOTHING,
        field_out: Rect::NOTHING,
        reset: Rect::NOTHING,
    };

    pub fn new(screen: Rect, anchor: Rect) -> Self {
        if !screen.is_positive() || !anchor.is_positive() {
            return Self::NONE;
        }
        let w = (screen.width() * TRACK_W.0).clamp(TRACK_W.1, TRACK_W.2.min(screen.width()));
        let h = (w * TRACK_ASPECT).min(screen.height() * 0.80);
        let mut panel = Rect::from_min_size(
            Pos2::new(anchor.center().x - w * 0.25, anchor.bottom() + h * 0.05),
            Vec2::new(w, h),
        );
        // Nudged back inside the window, both axes, exactly as the effect
        // panels are: a panel anchored to a control near an edge otherwise
        // hangs off it.
        let dx = (screen.left() - panel.left()).max(0.0) - (panel.right() - screen.right()).max(0.0);
        let dy = (screen.top() - panel.top()).max(0.0) - (panel.bottom() - screen.bottom()).max(0.0);
        panel = panel.translate(Vec2::new(dx, dy));
        if !panel.is_positive() {
            return Self::NONE;
        }
        let body = panel.shrink(panel.width() * 0.035);
        // **Shorter rows than the panel's width would suggest.** This is the
        // widest panel in the file — 720 points against an effect panel's 300 —
        // and every row was a fraction of a height derived from that width, so
        // its text came out at twice the size of the same control in the band.
        // The rows carry two numbers and two buttons; they do not need a fifth
        // of the panel each.
        let title = slice_v(body, 0.00, 0.13);
        let wave = slice_v(body, 0.20, 0.72);
        let row = slice_v(body, 0.80, 1.00);
        Self {
            panel,
            title: slice_h(title, 0.0, 0.86),
            close: slice_h(title, 0.88, 1.0),
            wave,
            field_in: slice_h(row, 0.00, 0.30),
            field_out: slice_h(row, 0.34, 0.64),
            reset: slice_h(row, 0.74, 1.00),
        }
    }
}

/// What a press in the track panel means.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackHit {
    /// Drag the in-point to this fraction of the file.
    DragIn(f32),
    /// Drag the out-point to this fraction of the file.
    DragOut(f32),
    /// Type the in- or out-point instead.
    TypeIn,
    TypeOut,
    /// Play the whole file again.
    ClearTrim,
    Close,
}

/// Where a trim point sits along the waveform, 0..=1.
///
/// `out` of zero means the end of the file, everywhere: it is what the engine
/// reads and what the settings hold, so the drawing has to agree.
pub fn trim_fractions(seconds: f64, from: f64, to: f64) -> (f32, f32) {
    if seconds <= 0.0 {
        return (0.0, 1.0);
    }
    let a = (from / seconds).clamp(0.0, 1.0) as f32;
    let b = if to <= 0.0 {
        1.0
    } else {
        (to / seconds).clamp(0.0, 1.0) as f32
    };
    (a.min(b), b.max(a))
}

/// What a press at `pos` in the track panel means, if anything.
pub fn track_hit_test(
    screen: Rect,
    anchor: Rect,
    seconds: f64,
    from: f64,
    to: f64,
    pos: Pos2,
) -> Option<TrackHit> {
    let l = TrackLayout::new(screen, anchor);
    if !l.panel.contains(pos) {
        return None;
    }
    if l.close.contains(pos) {
        return Some(TrackHit::Close);
    }
    if l.reset.contains(pos) {
        return Some(TrackHit::ClearTrim);
    }
    if l.field_in.contains(pos) {
        return Some(TrackHit::TypeIn);
    }
    if l.field_out.contains(pos) {
        return Some(TrackHit::TypeOut);
    }
    if l.wave.is_positive() && l.wave.expand(TRIM_GRAB).contains(pos) {
        let t = ((pos.x - l.wave.left()) / l.wave.width()).clamp(0.0, 1.0);
        let (a, b) = trim_fractions(seconds, from, to);
        let (xa, xb) = (
            l.wave.left() + a * l.wave.width(),
            l.wave.left() + b * l.wave.width(),
        );
        // **Whichever handle is nearer, and only if the press is near one.**
        // A press in the middle of the waveform is not a request to move the
        // end that happens to be closest — it is a press on the picture, and
        // moving half the track under it would be the worst kind of surprise.
        let (da, db) = ((pos.x - xa).abs(), (pos.x - xb).abs());
        if da.min(db) <= TRIM_GRAB {
            return Some(if da <= db {
                TrackHit::DragIn(t)
            } else {
                TrackHit::DragOut(t)
            });
        }
    }
    None
}

/// The panel's rectangle, for a caller that has to swallow presses.
pub fn track_popup_rect(screen: Rect, anchor: Rect) -> Rect {
    TrackLayout::new(screen, anchor).panel
}

/// A time in seconds as `m:ss.t`, which is how somebody reads a trim point.
pub fn trim_text(seconds: f64) -> String {
    let s = seconds.max(0.0);
    format!("{}:{:04.1}", (s / 60.0) as u64, s % 60.0)
}

/// Everything `draw_track_panel` needs, as one argument.
///
/// A struct rather than eight parameters: six of them are `f64`, `&str` and
/// `Rect` in pairs, and the way that goes wrong is silently — a trim drawn
/// with the in and out the wrong way round looks like a panel with a bug in
/// the waveform.
pub struct TrackPanel<'a> {
    pub screen: Rect,
    pub anchor: Rect,
    pub track: &'a crate::ports::TrackInfo,
    /// The trim, in seconds. `to` of zero is the end of the file.
    pub from: f64,
    pub to: f64,
    /// What is being typed into the IN and OUT fields, if either is open.
    pub typing: (Option<&'a str>, Option<&'a str>),
}

/// Draw the waveform, the trim, and the two numbers.
pub fn draw_track_panel(painter: &Painter, at: TrackPanel<'_>, s: &Settings) {
    let TrackPanel {
        screen,
        anchor,
        track,
        from,
        to,
        typing,
    } = at;
    let l = TrackLayout::new(screen, anchor);
    if !l.panel.is_positive() {
        return;
    }
    let p = palette(s);
    painter.rect_filled(screen, 0.0, Color32::from_black_alpha(96));
    painter.rect_filled(l.panel, 4.0, p.bg);
    painter.rect_stroke(l.panel, 4.0, Stroke::new(1.0_f32, p.ink), StrokeKind::Inside);

    if l.title.is_positive() {
        let name = if track.is_empty() {
            "no backing track loaded".to_owned()
        } else {
            format!("{}   {}", track.name, trim_text(track.seconds))
        };
        // 0.42, the same fraction the effect panels' titles use, so the two
        // read as the same size of thing on screen rather than the same
        // fraction of two different boxes.
        let size = fit_text(l.title, &name, l.title.height() * 0.42);
        if size >= MIN_TEXT {
            painter.text(
                Pos2::new(l.title.left(), l.title.center().y),
                Align2::LEFT_CENTER,
                &name,
                font(size),
                p.ink,
            );
        }
        draw_word_button(painter, l.close, &["X"], &p);
    }

    // The waveform, in the same recess the meters use.
    if l.wave.is_positive() {
        painter.rect_filled(l.wave, 2.0, METER_FACE);
        let (a, b) = trim_fractions(track.seconds, from, to);
        let x_at = |t: f32| l.wave.left() + t * l.wave.width();
        let mid = l.wave.center().y;
        let half = l.wave.height() * 0.46;
        if track.wave.is_empty() {
            let msg = "click the waveform icon in the band to import a file";
            let size = fit_text(l.wave, msg, l.wave.height() * 0.11);
            if size >= MIN_TEXT {
                painter.text(l.wave.center(), Align2::CENTER_CENTER, msg, font(size), p.faint);
            }
        } else {
            // One column per point, sampled from the envelope — the envelope
            // is a thousand buckets and the panel is a few hundred points
            // wide, so this is a decimation and not a stretch.
            let cols = l.wave.width().max(1.0) as usize;
            for i in 0..cols {
                let t = i as f32 / cols as f32;
                let v = track.wave[(t * track.wave.len() as f32) as usize % track.wave.len()];
                let x = l.wave.left() + i as f32;
                // **Outside the trim is drawn and dimmed, not hidden.** What
                // was cut is how somebody knows they cut the right thing, and
                // a waveform that jumped to only the kept part every time a
                // handle moved would be impossible to aim.
                let lit = t >= a && t <= b;
                let ink = if lit {
                    Color32::from_rgb(0x5f, 0xc9, 0x8a)
                } else {
                    Color32::from_rgba_unmultiplied(0x5f, 0xc9, 0x8a, 52)
                };
                let h = (v * half).max(0.5);
                painter.rect_filled(
                    Rect::from_min_max(Pos2::new(x, mid - h), Pos2::new(x + 1.0, mid + h)),
                    0.0,
                    ink,
                );
            }
            // The handles, over the top.
            for x in [x_at(a), x_at(b)] {
                painter.line_segment(
                    [Pos2::new(x, l.wave.top()), Pos2::new(x, l.wave.bottom())],
                    Stroke::new(1.5_f32, Color32::from_rgb(0xff, 0xf4, 0xe0)),
                );
            }
        }
    }

    // The two numbers and the way back.
    let (kept_from, kept_to) = (from, if to <= 0.0 { track.seconds } else { to });
    for (r, label, value, typed) in [
        (l.field_in, "IN", kept_from, typing.0),
        (l.field_out, "OUT", kept_to, typing.1),
    ] {
        if !r.is_positive() {
            continue;
        }
        painter.rect_filled(r, 2.0, p.field);
        let text = typed.map_or_else(|| trim_text(value), |t| format!("{t}_"));
        let shown = format!("{label} {text}");
        // A little larger than the title: these are the numbers being read and
        // typed into, which is what the panel is for.
        let size = fit_text(r.shrink(r.width() * 0.06), &shown, r.height() * 0.34);
        if size >= MIN_TEXT {
            painter.text(r.center(), Align2::CENTER_CENTER, &shown, font(size), p.ink);
        }
    }
    draw_word_button_sized(painter, l.reset, &["WHOLE FILE", "WHOLE"], 0.30, &p);
}
