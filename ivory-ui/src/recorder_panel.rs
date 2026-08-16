//! The Recorder band: a whole take's worth of controls in 200 points of window.
//!
//! Two pictures of the same machine, switched by [`RecordState`], because no
//! other recorder knows that its user is a pianist:
//!
//!   * **idle**, which is about FRAMING and destination — a wide camera
//!     preview, a live meter, and every control that decides what the next take
//!     will be, and
//!   * **rolling**, which is about being readable from a piano bench two metres
//!     away — the preview collapses, the timecode and the meter become huge,
//!     and everything that could change the destination mid-take goes away.
//!
//! While playing, the pianist is looking at their hands. The preview's job is
//! finished before the take starts, which is the whole reason those are two
//! layouts and not one that dims.
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
    disk_text, timecode, DeviceLabel, ExportSpec, Level, Meters, RecordState, RecorderView,
    PREROLL_CHOICES,
};
use crate::settings::Settings;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

/// Band height for a 1300pt-wide window. At that width it gives a ~280x150
/// preview and still leaves ~950pt for controls, which is the one dimension
/// this window has in abundance. Scaled with everything else (spec §3.2).
pub const BAND_H_AT_1300: f64 = 200.0;

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
    /// Recessed fills: the preview well and the meter troughs.
    well: Color32,
    /// Control fills, so a thing you can click looks unlike a thing you read.
    field: Color32,
    ink: Color32,
    faint: Color32,
    line: Color32,
    /// The meter. Whatever a held key looks like on the piano — the user chose
    /// that colour once and this is the same "sound is happening" signal.
    accent: Color32,
    rec: Color32,
    /// A chosen device that is not plugged in, which is neither an error nor
    /// normal and must not read as either.
    warn: Color32,
}

fn palette(s: &Settings) -> Palette {
    let dark = s.dark_mode;
    Palette {
        // The piano's own background, so the recorder reads as another band of
        // the same window rather than as a dialog that landed in it.
        bg: crate::piano::bg_color(dark),
        well: if dark {
            Color32::from_rgb(0x0a, 0x0a, 0x0a)
        } else {
            Color32::from_rgb(0xCF, 0xCF, 0xCF)
        },
        field: if dark {
            Color32::from_rgb(0x26, 0x26, 0x26)
        } else {
            Color32::from_rgb(0xDC, 0xDC, 0xDC)
        },
        ink: if dark {
            Color32::from_rgb(0xE8, 0xDC, 0xC0)
        } else {
            Color32::from_rgb(0x1a, 0x1a, 0x1a)
        },
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
/// numbers. It is also what makes the two properties that matter testable
/// without a screen — that the destination controls are gone mid-take, and that
/// the clock is not drawn when the user asked for no clock.
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
    record: Rect,
    /// The steady red dot, which takes the record button's place while rolling.
    /// An indicator and not a control, so it is not a hit.
    dot: Rect,
    stop: Rect,
    meter: Rect,
    /// The big readout. [`Rect::NOTHING`] when the clock is suppressed.
    timecode: Rect,
    clock: Rect,
    /// One line of status, and the clip warning beside it.
    status: Rect,
    clip: Rect,
    dest: Rect,
    default_tick: Rect,
    name: Rect,
    /// Live grey text under the name field. Teaches the naming scheme without
    /// a help page, and is not clickable.
    folder: Rect,
    camera: Rect,
    audio: Rect,
    preroll: Rect,
    export: Rect,
    disk: Rect,
}

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

impl Layout {
    /// Nothing at all, for a rect too small to hold a band. Every field absent
    /// rather than degenerate: negative rectangles produce NaN centres, and a
    /// NaN centre is a shape drawn nowhere and a hit test that never matches.
    fn empty(rolling: bool) -> Self {
        Self {
            rolling,
            preview: Rect::NOTHING,
            record: Rect::NOTHING,
            dot: Rect::NOTHING,
            stop: Rect::NOTHING,
            meter: Rect::NOTHING,
            timecode: Rect::NOTHING,
            clock: Rect::NOTHING,
            status: Rect::NOTHING,
            clip: Rect::NOTHING,
            dest: Rect::NOTHING,
            default_tick: Rect::NOTHING,
            name: Rect::NOTHING,
            folder: Rect::NOTHING,
            camera: Rect::NOTHING,
            audio: Rect::NOTHING,
            preroll: Rect::NOTHING,
            export: Rect::NOTHING,
            disk: Rect::NOTHING,
        }
    }

    fn new(rect: Rect, view: &RecorderView<'_>) -> Self {
        let rolling = view.state.is_active();
        let pad = (rect.height() * 0.07).clamp(1.0, 14.0);
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
            l.fill_idle(body, gap);
        }
        l
    }

    /// Idle: preview, then transport, then everything that decides where the
    /// take goes. Left to right in that order deliberately — the device
    /// pickers are NOT first, because after the first session they are the
    /// controls nobody ever touches again.
    fn fill_idle(&mut self, body: Rect, gap: f32) {
        // The preview wants to be landscape, so its width follows the band's
        // own HEIGHT — never the camera's aspect, which is the whole point —
        // and it never takes more than a little over 40% of the width, or the
        // destination block stops holding a device name.
        let pv_w = (body.height() * 1.85).clamp(body.width() * 0.18, body.width() * 0.42);
        self.preview = Rect::from_min_max(body.min, Pos2::new(body.left() + pv_w, body.bottom()));
        let rest = Rect::from_min_max(Pos2::new(self.preview.right() + gap, body.top()), body.max);
        if !rest.is_positive() {
            return;
        }

        let t = slice_h(rest, 0.0, 0.33);
        let d = Rect::from_min_max(Pos2::new(t.right() + gap, rest.top()), rest.max);
        self.fill_transport(t);
        if !d.is_positive() {
            return;
        }

        // Seven rows with real gaps between them. The gaps are not decoration:
        // `Rect::contains` is inclusive at both edges, so two rows that merely
        // touch share a line of pixels and the overlap test fails there.
        let folder_row = slice_v(d, 0.00, 0.13);
        self.dest = slice_h(folder_row, 0.00, 0.70);
        self.default_tick = slice_h(folder_row, 0.74, 1.00);
        self.name = slice_v(d, 0.17, 0.30);
        self.folder = slice_v(d, 0.31, 0.42);
        self.camera = slice_v(d, 0.45, 0.58);
        self.audio = slice_v(d, 0.60, 0.73);
        let export_row = slice_v(d, 0.76, 0.89);
        self.preroll = slice_h(export_row, 0.00, 0.46);
        self.export = slice_h(export_row, 0.52, 1.00);
        self.disk = slice_v(d, 0.90, 1.00);
    }

    /// The transport column: the round record button, a stop beside it, the
    /// meter under both, and the clock with its own switch.
    fn fill_transport(&mut self, t: Rect) {
        if !t.is_positive() {
            return;
        }
        let top = slice_v(t, 0.0, 0.52);
        // Each button is capped at the width of its own slice, which is what
        // keeps the two from colliding at any aspect ratio the window can take.
        let rec_d = top.height().min(top.width() * 0.42);
        self.record = Rect::from_center_size(slice_h(top, 0.0, 0.42).center(), Vec2::splat(rec_d));
        self.stop =
            Rect::from_center_size(slice_h(top, 0.46, 0.80).center(), Vec2::splat(rec_d * 0.66));
        self.meter = slice_v(t, 0.56, 0.74);
        let time = slice_v(t, 0.78, 1.0);
        self.timecode = slice_h(time, 0.0, 0.62);
        self.clock = slice_h(time, 0.66, 1.0);
    }

    /// Rolling: readable from the bench. The preview collapses to a strip that
    /// is enough to see somebody has walked in front of the camera and no more,
    /// and the two things worth reading from two metres away take the rest.
    fn fill_rolling(&mut self, body: Rect, gap: f32, view: &RecorderView<'_>) {
        let pv_w = (body.height() * 1.1).min(body.width() * 0.18);
        self.preview = Rect::from_min_max(body.min, Pos2::new(body.left() + pv_w, body.bottom()));
        let rest = Rect::from_min_max(Pos2::new(self.preview.right() + gap, body.top()), body.max);
        if !rest.is_positive() {
            return;
        }
        self.meter = slice_v(rest, 0.74, 1.0);
        let top = slice_v(rest, 0.0, 0.68);

        // The dot stands exactly where the record button stood, so the band
        // does not reshuffle under the eye at the moment the take starts.
        // There is no record button while rolling — pressing it would mean
        // nothing, and a dead control is worse than no control.
        let dot_d = (top.height() * 0.30).min(top.width() * 0.06);
        self.dot = Rect::from_center_size(slice_h(top, 0.0, 0.10).center(), Vec2::splat(dot_d));
        let stop_d = (top.height() * 0.62).min(top.width() * 0.12);
        self.stop = Rect::from_center_size(slice_h(top, 0.12, 0.28).center(), Vec2::splat(stop_d));
        self.clock = Rect::from_min_max(
            Pos2::new(top.left() + top.width() * 0.90, top.top()),
            Pos2::new(top.right(), top.top() + top.height() * 0.30),
        );

        // The one thing `hide_elapsed` suppresses is a CLOCK. A pre-roll
        // countdown is the number the user is waiting for, and "FINISHING" is
        // the reason not to close the lid yet; hiding either would be hiding
        // the wrong thing under the name of a performance setting.
        self.timecode = if view.hide_elapsed && matches!(view.state, RecordState::Rolling) {
            Rect::NOTHING
        } else {
            slice_h(top, 0.30, 0.88)
        };
    }

    /// Every clickable region and what it means, in one place.
    ///
    /// [`hit_test`] reads this and so does the test that proves no two of them
    /// overlap, so a control that moves onto another one fails a test rather
    /// than quietly swallowing its clicks.
    fn targets(&self) -> [(Rect, Hit); 10] {
        [
            (self.record, Hit::Record),
            (self.stop, Hit::Stop),
            (self.dest, Hit::ChooseFolder),
            (self.default_tick, Hit::ToggleDefaultDir),
            (self.name, Hit::NameField),
            (self.camera, Hit::PickCamera),
            (self.audio, Hit::PickAudio),
            (self.preroll, Hit::CyclePreRoll),
            (self.export, Hit::Export),
            (self.clock, Hit::ToggleHideElapsed),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    Record,
    Stop,
    ChooseFolder,
    ToggleDefaultDir,
    NameField,
    PickCamera,
    PickAudio,
    CyclePreRoll,
    Export,
    ToggleHideElapsed,
}

impl Hit {
    /// Every variant, which is what the reachability test iterates. The
    /// exhaustive match in [`Hit::label`] is what makes adding a variant
    /// without adding it here a compile error rather than an untested control.
    pub const ALL: [Hit; 10] = [
        Hit::Record,
        Hit::Stop,
        Hit::ChooseFolder,
        Hit::ToggleDefaultDir,
        Hit::NameField,
        Hit::PickCamera,
        Hit::PickAudio,
        Hit::CyclePreRoll,
        Hit::Export,
        Hit::ToggleHideElapsed,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Hit::Record => "Record",
            Hit::Stop => "Stop",
            Hit::ChooseFolder => "Choose folder",
            Hit::ToggleDefaultDir => "Use this folder by default",
            Hit::NameField => "Take name",
            Hit::PickCamera => "Camera",
            Hit::PickAudio => "Audio input",
            Hit::CyclePreRoll => "Pre-roll",
            Hit::Export => "Export",
            Hit::ToggleHideElapsed => "Hide elapsed time",
        }
    }
}

/// What is under `pos`, if anything.
///
/// The exact inverse of [`draw`], by construction rather than by discipline:
/// both read the same [`Layout`].
///
/// **While a take is live only [`Hit::Stop`] and [`Hit::ToggleHideElapsed`]
/// survive.** Every destination control is gone, which is not a courtesy — the
/// output folder, the take name and the devices are all decided at `T0` and a
/// UI that lets you change them at 0:47 is a UI that promises something it
/// cannot do. The clock switch survives because the moment a running timer
/// becomes a distraction is the moment it is running.
pub fn hit_test(rect: Rect, view: &RecorderView<'_>, pos: Pos2) -> Option<Hit> {
    if !rect.contains(pos) {
        return None;
    }
    Layout::new(rect, view)
        .targets()
        .into_iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| h)
}

/// The next pre-roll in the cycle.
///
/// Here rather than in the app so that the control's label and the effect of
/// clicking it cannot disagree. An unknown value — a hand-edited settings file
/// — lands on the first choice rather than sticking.
pub fn next_preroll(current: u8) -> u8 {
    let i = PREROLL_CHOICES
        .iter()
        .position(|&c| c == current)
        .map_or(0, |i| i + 1);
    PREROLL_CHOICES[i % PREROLL_CHOICES.len()]
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
    draw_transport(painter, &l, view, &p);
    draw_meter(painter, l.meter, view.meters, &p);
    draw_readout(painter, &l, view, &p);
    if !l.rolling {
        draw_destination(painter, &l, view, s, &p);
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

/// A labelled control: the caption in the faint ink, the value after it.
///
/// One helper because six rows of the destination block are this shape, and six
/// copies would drift apart the first time one of them was adjusted.
fn labelled(painter: &Painter, r: Rect, cap: &str, value: &str, colour: Color32, p: &Palette) {
    control(painter, r, p);
    if !r.is_positive() {
        return;
    }
    let inset = r.height() * 0.30;
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

fn draw_preview(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    let r = l.preview;
    if !r.is_positive() {
        return;
    }
    match view.preview {
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
            let (top, hint) = match view.camera {
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

fn draw_transport(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    if l.record.is_positive() {
        let c = l.record.center();
        let rad = l.record.width() * 0.5;
        painter.circle_filled(c, rad, p.rec);
        painter.circle_stroke(c, rad, Stroke::new(1.5_f32, p.line));
    }
    if l.dot.is_positive() {
        painter.circle_filled(l.dot.center(), l.dot.width() * 0.5, p.rec);
    }
    if l.stop.is_positive() {
        // Present in BOTH layouts, in the same place. A stop button that only
        // appears once the take has started is one you have to find while your
        // hands are on the keys.
        control(painter, l.stop, p);
        painter.rect_filled(
            l.stop.shrink(l.stop.width() * 0.30),
            1.0,
            if l.rolling { p.ink } else { p.faint },
        );
    }
    if l.clock.is_positive() {
        control(painter, l.clock, p);
        let size = fit_text(l.clock.shrink(2.0), "CLOCK", l.clock.height() * 0.5);
        if size >= MIN_TEXT {
            painter.text(
                l.clock.center(),
                Align2::CENTER_CENTER,
                "CLOCK",
                font(size),
                if view.hide_elapsed { p.faint } else { p.ink },
            );
        }
        if view.hide_elapsed {
            painter.line_segment(
                [
                    Pos2::new(l.clock.left() + 2.0, l.clock.center().y),
                    Pos2::new(l.clock.right() - 2.0, l.clock.center().y),
                ],
                Stroke::new(1.5_f32, p.faint),
            );
        }
    }
}

/// Where a linear level sits along the meter, 0..=1.
///
/// Linear amplitude on a linear bar is why naive meters look broken: a
/// perfectly healthy take at -20 dBFS fills a tenth of the bar and the user
/// turns the gain up until it clips. A 60 dB scale puts half the bar at -30 dB
/// and gives the last tenth to the last 6 dB, which is where clipping lives.
fn meter_x(level: f32) -> f32 {
    const FLOOR_DB: f32 = -60.0;
    if level <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * level.log10();
    ((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0)
}

/// Where the red zone starts. The last 6 dB, drawn always rather than only once
/// something has gone wrong, so the top of the scale means something before the
/// first take instead of after it.
const CLIP_ZONE: f32 = 0.501; // -6 dBFS

fn draw_meter(painter: &Painter, r: Rect, m: Meters, p: &Palette) {
    if !r.is_positive() {
        return;
    }
    // The meter is live before arming, which is the entire point of it: "I
    // recorded silence" is a failure class that dies at the sight of a moving
    // bar. Nothing here is gated on the state.
    let one = [m.left];
    let two = [m.left, m.right];
    let bars: &[Level] = if m.mono { &one } else { &two };
    let gap = (r.height() * 0.12).min(3.0);
    let bh = (r.height() - gap * (bars.len() as f32 - 1.0)) / bars.len() as f32;
    if bh <= 0.0 {
        return;
    }
    for (i, lv) in bars.iter().enumerate() {
        let top = r.top() + i as f32 * (bh + gap);
        let bar = Rect::from_min_max(Pos2::new(r.left(), top), Pos2::new(r.right(), top + bh));
        painter.rect_filled(bar, 1.0, p.well);
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(bar.left() + bar.width() * meter_x(CLIP_ZONE), bar.top()),
                bar.max,
            ),
            1.0,
            p.rec.gamma_multiply(0.22),
        );
        // Rms as the fill and peak as a brighter tip: they answer different
        // questions, and a meter that shows only one of them is how people
        // record either silence or distortion.
        let x = |v: f32| bar.left() + bar.width() * meter_x(v);
        painter.rect_filled(
            Rect::from_min_max(bar.min, Pos2::new(x(lv.rms), bar.bottom())),
            1.0,
            p.accent,
        );
        let (pk, rms) = (x(lv.peak), x(lv.rms));
        if pk > rms {
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(rms, bar.top()), Pos2::new(pk, bar.bottom())),
                1.0,
                p.accent.gamma_multiply(0.45),
            );
        }
        if lv.hold > 0.0 {
            let h = x(lv.hold);
            painter.line_segment(
                [Pos2::new(h, bar.top()), Pos2::new(h, bar.bottom())],
                Stroke::new(2.0_f32, p.ink),
            );
        }
        painter.rect_stroke(
            bar,
            1.0,
            Stroke::new(1.0_f32, if m.clipped { p.rec } else { p.line }),
            StrokeKind::Inside,
        );
    }
}

fn draw_readout(painter: &Painter, l: &Layout, view: &RecorderView<'_>, p: &Palette) {
    let r = l.timecode;
    if !r.is_positive() {
        return;
    }
    let (text, colour) = match view.state {
        // The countdown, very large, in place of the clock: it is the only
        // number that matters while the user is walking back to the bench, and
        // it is the reason they can look up from three metres away.
        RecordState::PreRoll { remaining_s } => {
            (format!("{}", remaining_s.max(0.0).ceil() as u32), p.rec)
        }
        RecordState::Finishing => ("FINISHING".to_owned(), p.ink),
        RecordState::Rolling => (timecode(view.elapsed_s), p.ink),
        RecordState::Idle => (timecode(view.elapsed_s), p.faint),
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

/// A device row. `Missing` is neither `None` nor `Open` and must not read as
/// either: it means the user already chose this device and it is not here right
/// now, which is a thing to go and fix rather than a thing to go and set up.
fn device_note(d: DeviceLabel<'_>) -> &'static str {
    match d {
        DeviceLabel::Missing(_) => "  (not connected)",
        DeviceLabel::None | DeviceLabel::Open(_) => "",
    }
}

/// What a take will actually produce, in four words.
///
/// Drawn beside the Export button so the dialog is somewhere you go to CHANGE
/// the answer rather than to find out what it is.
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
                Pos2::new(l.dest.right() - l.dest.height() * 0.3, l.dest.center().y),
                Align2::RIGHT_CENTER,
                "Choose...",
                font_light(size),
                p.faint,
            );
        }
    }
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
    labelled(
        painter,
        l.name,
        "NAME",
        if empty { "(optional)" } else { view.take_name },
        if empty { p.faint } else { p.ink },
        p,
    );
    if view.name_focused && l.name.is_positive() {
        let size = (l.name.height() * 0.52).max(1.0);
        let inset = l.name.height() * 0.30;
        let x = l.name.left()
            + inset
            + size * ADV * (5.0 + view.take_name.chars().count() as f32 + 1.0);
        painter.line_segment(
            [
                Pos2::new(x, l.name.center().y - size * 0.5),
                Pos2::new(x, l.name.center().y + size * 0.5),
            ],
            Stroke::new(1.5_f32, p.ink),
        );
    }
    text_line(painter, l.folder, view.folder_preview, p.faint, false);

    for (r, cap, dev) in [
        (l.camera, "CAMERA", view.camera),
        (l.audio, "AUDIO", view.audio),
    ] {
        let colour = match dev {
            DeviceLabel::None => p.faint,
            DeviceLabel::Open(_) => p.ink,
            DeviceLabel::Missing(_) => p.warn,
        };
        let value = format!("{}{}", dev.text(), device_note(dev));
        labelled(painter, r, cap, &value, colour, p);
    }

    let pre = if view.preroll_s == 0 {
        "off".to_owned()
    } else {
        format!("{} s", view.preroll_s)
    };
    labelled(painter, l.preroll, "PRE-ROLL", &pre, p.ink, p);

    control(painter, l.export, p);
    if l.export.is_positive() {
        let size = fit_text(l.export, "Export...", l.export.height() * 0.52);
        if size >= MIN_TEXT {
            painter.text(
                l.export.center(),
                Align2::CENTER_CENTER,
                "Export...",
                font(size),
                p.ink,
            );
        }
    }

    // Disk as a DURATION. "214 GB free" means nothing to a pianist and "~58
    // min" means everything, which is why the view carries minutes and not
    // bytes in the first place.
    let disk = match view.disk_minutes {
        Some(m) => format!("{} left", disk_text(m)),
        None => "measuring free space".to_owned(),
    };
    text_line(painter, l.disk, &disk, p.faint, false);
    text_line(
        painter,
        l.disk,
        &export_summary(&s.record_export),
        p.faint,
        true,
    );
}

/// A tick box and its caption: two line segments rather than a glyph, because
/// no bundled face is guaranteed to carry a check mark and a tofu box in a
/// checkbox is indistinguishable from a ticked one.
fn draw_tick(painter: &Painter, r: Rect, cap: &str, on: bool, p: &Palette) {
    control(painter, r, p);
    if !r.is_positive() {
        return;
    }
    let inset = r.height() * 0.25;
    let side = r.height() - inset * 2.0;
    let bx = Rect::from_min_size(
        Pos2::new(r.left() + inset, r.top() + inset),
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
/// monitor, and the preview claims a little over 40% of the width, so this is
/// about as small as the window gets while the destination column still holds a
/// device name at a readable size.
pub const DETACHED_DEFAULT: Vec2 = Vec2::new(720.0, 400.0);

/// The smallest window the band still works in.
///
/// The binding constraint is the destination column, not the preview: seven
/// rows share the body's height and the column takes what is left after the
/// preview and the transport. At 480x270 those rows are ~28pt and the column is
/// ~170pt, which holds "CAMERA FaceTime HD" at about 8pt. Below that the band
/// is a row of boxes with smudges in them, which reads as a rendering fault
/// rather than as "too small" — the same failure `theory_panel::DETACHED_MIN`
/// puts a floor under.
pub const DETACHED_MIN: Vec2 = Vec2::new(480.0, 270.0);

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

            // Ctrl-click IS the right-click on macOS, and this window has ten
            // click targets. Without the guard, ctrl-clicking to open the menu
            // over the transport starts a take at the same time.
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
    use crate::recorder::Preview;

    fn band(w: f32) -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 350.0), Vec2::new(w, band_height(w)))
    }

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
        assert_eq!(band_height(1300.0), 200.0);
        assert_eq!(band_height(650.0), 100.0);
        // Truncated, not rounded, like every other band: 200 * 1000/1300 is
        // 153.8, and a half-pixel band puts every row on a fractional line.
        assert_eq!(band_height(1000.0), 153.0);
        assert_eq!(band_height(0.0), 0.0);

        let pv = Preview {
            texture: egui::TextureId::User(1),
            size: Vec2::new(640.0, 480.0),
        };
        for w in [400.0_f32, 1300.0] {
            let want = band_height(w);
            for state in [
                RecordState::Idle,
                RecordState::PreRoll { remaining_s: 2.0 },
                RecordState::Rolling,
                RecordState::Finishing,
            ] {
                for preview in [None, Some(pv)] {
                    let v = RecorderView {
                        state,
                        preview,
                        ..RecorderView::empty()
                    };
                    // Nothing in the view can reach the height: the layout is
                    // handed a rect the height already decided.
                    let l = Layout::new(band(w), &v);
                    assert_eq!(band_height(w), want, "{state:?} moved the band height");
                    for (r, hit) in l.targets() {
                        assert!(
                            !r.is_positive() || band(w).contains_rect(r),
                            "{hit:?} escaped the band at {w}pt in {state:?}"
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
    #[test]
    fn no_two_hit_regions_overlap() {
        let states = [
            RecordState::Idle,
            RecordState::PreRoll { remaining_s: 3.0 },
            RecordState::Rolling,
            RecordState::Finishing,
        ];
        for w in [500.0_f32, 900.0, 1300.0, 2600.0] {
            for r in [band(w), Rect::from_min_size(Pos2::ZERO, DETACHED_DEFAULT)] {
                for state in states {
                    for hide in [false, true] {
                        let v = RecorderView {
                            state,
                            hide_elapsed: hide,
                            ..RecorderView::empty()
                        };
                        let t = Layout::new(r, &v).targets();
                        for i in 0..t.len() {
                            for j in (i + 1)..t.len() {
                                assert!(
                                    !t[i].0.intersects(t[j].0),
                                    "{:?} and {:?} overlap at {w}pt in {state:?}: {:?} {:?}",
                                    t[i].1,
                                    t[j].1,
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

    /// Every control has to be reachable at the point it is drawn, or it is
    /// decoration. Driven off `Hit::ALL` so a new variant that nobody wired up
    /// fails here rather than shipping as a button that does nothing.
    #[test]
    fn every_control_is_reachable_in_the_idle_layout() {
        for r in [
            band(1300.0),
            band(650.0),
            Rect::from_min_size(Pos2::new(40.0, 90.0), DETACHED_DEFAULT),
            Rect::from_min_size(Pos2::ZERO, DETACHED_MIN),
        ] {
            let v = idle();
            let l = Layout::new(r, &v);
            for want in Hit::ALL {
                let (rect, _) = l
                    .targets()
                    .into_iter()
                    .find(|(_, h)| *h == want)
                    .expect("every variant is in `targets`");
                assert!(
                    rect.is_positive(),
                    "{} has no rect while idle at {:?}",
                    want.label(),
                    r.size()
                );
                assert_eq!(
                    hit_test(r, &v, rect.center()),
                    Some(want),
                    "{} is not clickable at its own centre",
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
    /// Stop, and the clock switch. Nothing else: the folder, the name and the
    /// devices are all decided at `T0`, and offering them at 0:47 would be
    /// promising something the recorder cannot do. Record is gone too — the
    /// button is a red dot while rolling, and a dead control is worse than no
    /// control.
    #[test]
    fn the_rolling_layout_offers_no_destination_hits() {
        for state in [
            RecordState::PreRoll { remaining_s: 1.5 },
            RecordState::Rolling,
            RecordState::Finishing,
        ] {
            for hide in [false, true] {
                let v = RecorderView {
                    state,
                    hide_elapsed: hide,
                    ..RecorderView::empty()
                };
                let r = band(1300.0);
                let mut seen = Vec::new();
                let mut y = r.top();
                while y <= r.bottom() {
                    let mut x = r.left();
                    while x <= r.right() {
                        if let Some(h) = hit_test(r, &v, Pos2::new(x, y)) {
                            assert!(
                                matches!(h, Hit::Stop | Hit::ToggleHideElapsed),
                                "{h:?} is reachable at ({x}, {y}) during {state:?}"
                            );
                            if !seen.contains(&h) {
                                seen.push(h);
                            }
                        }
                        x += 2.0;
                    }
                    y += 2.0;
                }
                seen.sort_by_key(|h| format!("{h:?}"));
                assert_eq!(
                    seen,
                    vec![Hit::Stop, Hit::ToggleHideElapsed],
                    "the two survivors are not both reachable during {state:?}"
                );
            }
        }
    }

    /// The setting no competitor offers: after a blinking light, a running
    /// timer is the most-cited performance distraction. What it hides is a
    /// CLOCK, though — not the pre-roll countdown the user is waiting for, and
    /// not the notice that files are still being written.
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
            hidden(RecordState::PreRoll { remaining_s: 2.0 }).is_positive(),
            "the countdown is the number the user is waiting for"
        );
        assert!(
            hidden(RecordState::Finishing).is_positive(),
            "'files are still being closed' is not a clock"
        );
        assert!(
            hidden(RecordState::Idle).is_positive(),
            "the setting is about recording, not about sitting there"
        );
        // Without the setting the clock is drawn in every state, and while
        // rolling it is far bigger than it is at rest — that is the whole
        // rolling layout: readable from a bench two metres away.
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
        assert!(
            live.height() > at_rest.height() * 2.0,
            "the rolling timecode is not bigger: {live:?} vs {at_rest:?}"
        );
        // The record button and the meter are still there either way.
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
            assert!(
                l.stop.is_positive() && l.clock.is_positive(),
                "the transport went away in {state:?}"
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
        assert!(
            live.meter.height() > at_rest.meter.height(),
            "the meter did not grow for the rolling layout"
        );
    }

    /// Linear amplitude on a linear bar is why naive meters look broken. Half
    /// the bar has to be a long way below full scale or every user turns the
    /// gain up until they clip.
    #[test]
    fn the_meter_scale_puts_a_usable_signal_in_the_middle() {
        assert_eq!(meter_x(0.0), 0.0);
        assert_eq!(meter_x(-1.0), 0.0, "a negative sample is not a level");
        assert!((meter_x(1.0) - 1.0).abs() < 1e-6);
        assert!(meter_x(2.0) <= 1.0, "over full scale still fits the bar");
        // -30 dBFS lands near the middle, not near the floor.
        let mid = meter_x(0.0316);
        assert!((0.45..0.55).contains(&mid), "-30 dB drew at {mid}");
        // Monotonic, or a rising signal could draw a shrinking bar.
        let mut last = -1.0;
        for i in 0..=100 {
            let x = meter_x(i as f32 / 100.0);
            assert!(x >= last, "the meter went backwards at {i}");
            last = x;
        }
        assert!(meter_x(CLIP_ZONE) > 0.85, "the red zone is not at the top");
    }

    #[test]
    fn the_preroll_control_cycles_through_every_choice_and_comes_back() {
        let mut seen = Vec::new();
        let mut v = PREROLL_CHOICES[0];
        for _ in 0..PREROLL_CHOICES.len() {
            seen.push(v);
            v = next_preroll(v);
        }
        assert_eq!(seen, PREROLL_CHOICES.to_vec());
        assert_eq!(v, PREROLL_CHOICES[0], "the cycle does not come back round");
        // A value a hand-edited settings file could hold lands somewhere real
        // rather than sticking.
        assert_eq!(next_preroll(97), PREROLL_CHOICES[0]);
    }

    /// A chosen device that is not plugged in is neither "None" nor working,
    /// and it must not read as either: one is a thing to set up and the other
    /// is a thing to go and fix.
    #[test]
    fn a_missing_device_reads_differently_from_an_open_one() {
        assert_eq!(device_note(DeviceLabel::Open("FaceTime HD")), "");
        assert_eq!(device_note(DeviceLabel::None), "");
        assert!(device_note(DeviceLabel::Missing("Scarlett")).contains("not connected"));
        let s = Settings::default();
        let p = palette(&s);
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
        let states = [
            RecordState::Idle,
            RecordState::PreRoll { remaining_s: 2.4 },
            RecordState::Rolling,
            RecordState::Finishing,
        ];
        for dark in [false, true] {
            let s = Settings {
                dark_mode: dark,
                ..Settings::default()
            };
            for w in [0.0_f32, 60.0, 400.0, 1300.0, 2600.0] {
                for state in states {
                    for preview in [None, Some(pv)] {
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
                            folder_preview: "nocturne-2026-08-16-141203",
                            camera: DeviceLabel::Missing("FaceTime HD Camera"),
                            audio: DeviceLabel::Open("Scarlett 2i2 USB"),
                            preview,
                            disk_minutes: Some(134.0),
                            hide_elapsed: dark,
                            message: Some("recorded 4:12 to nocturne-2026-08-16-141203"),
                            clip_warning: true,
                            ..RecorderView::empty()
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
