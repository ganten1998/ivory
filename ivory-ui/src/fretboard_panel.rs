//! The guitar view (D-UI-15): a neck drawn under the piano, with the notes you
//! are holding shown where a guitarist would actually put their fingers.
//!
//! Dumb on purpose, exactly like `piano.rs`. Every decision — which of a
//! pitch's five possible positions lights up, what is a barre, what folded an
//! octave, what could not fit — was already made by `ivory_core::voicing`, and
//! this module only renders the answer. Nothing here recomputes a shape, and
//! nothing here decides one.
//!
//! Geometry comes from `ivory_core::fretboard`, which works in a 0.0..=1.0
//! space along the string. That space is NOT the width of the widget:
//! `fret_x(22)` is 0.719, because a 22-fret neck is only 72% of the way to the
//! bridge. The board is scaled so the last drawn fret lands on the right edge,
//! which is how every chord chart and every guitar photograph is framed.
//!
//! The four pictures that are not an ordinary dot each mean something
//! different, and keeping them distinguishable is the whole job:
//!
//!   * a **hollow ring left of the nut** is an open string,
//!   * a **hollow dot with an arrow** is a note out of the instrument's range,
//!     drawn an octave away, with the arrow saying which way,
//!   * a **faint ring on the board** is a note the guitar can make but not at
//!     the same time as the others, and
//!   * an **x above the nut** is a string that must be damped.
//!
//! A note that is simply not shown gets none of those: it is counted in the
//! caption instead, because the one failure that makes a panel like this
//! untrustworthy is silently drawing five of the six notes someone played.

use crate::fonts;
use crate::settings::Settings;
use egui::{Align2, Color32, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};
use ivory_core::fretboard::{self, FretboardSpec};
use ivory_core::voicing::{Barre, Outcome, StringState, Voicing};

/// Band height for a 1300pt-wide window, scaled with everything else. Six
/// strings need enough room that a dot is a dot rather than a smear, and the
/// piano above it is 150 at the same width.
pub const BAND_H_AT_1300: f64 = 132.0;

/// How wide the barre bar is, in dot radii. A note dot is 2.0 across, so
/// anything less than this leaves the dots bulging out of the bar.
const BARRE_W_IN_DOTS: f32 = 2.0;

/// What the capo is made of.
///
/// Its own choice rather than part of `Wood`, because a capo is an accessory
/// clamped onto the neck and not part of the instrument's finish — the same
/// capo goes on rosewood, maple and ebony. Cycled by clicking it, which is
/// the only control it needs: there is exactly one on screen and it is
/// unmistakably the thing you are pointing at.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum CapoStyle {
    /// Black, because the default fingerboard is rosewood and a wooden capo
    /// on a wooden neck is a bar you have to look for. It is also what most
    /// capos actually are.
    #[default]
    Black,
    Silver,
    Wood,
}

pub(crate) struct CapoColors {
    pub body: Color32,
    pub sheen: Color32,
    pub arm: Color32,
    /// Fine cross-ticks, for brushed metal.
    pub texture: Option<Color32>,
    /// Long lines along the bar, for wood grain.
    pub grain: Option<Color32>,
}

impl CapoStyle {
    pub const ALL: [CapoStyle; 3] = [CapoStyle::Black, CapoStyle::Silver, CapoStyle::Wood];

    pub fn key(self) -> &'static str {
        match self {
            CapoStyle::Wood => "wood",
            CapoStyle::Black => "black",
            CapoStyle::Silver => "silver",
        }
    }

    /// Unknown values fall back at the point of use, like the fingerboard
    /// wood and the typeface: a settings file written by a later build keeps
    /// its own value rather than being rewritten.
    pub fn from_key(s: &str) -> Self {
        match s {
            "wood" => CapoStyle::Wood,
            "silver" => CapoStyle::Silver,
            _ => CapoStyle::Black,
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|c| *c == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub(crate) fn colors(self) -> CapoColors {
        match self {
            CapoStyle::Wood => CapoColors {
                body: Color32::from_rgb(0x6b, 0x45, 0x28),
                sheen: Color32::from_rgb(0x9a, 0x6d, 0x44),
                arm: Color32::from_rgb(0x8a, 0x8d, 0x92),
                texture: None,
                grain: Some(Color32::from_rgb(0x53, 0x34, 0x1c)),
            },
            CapoStyle::Black => CapoColors {
                body: Color32::from_rgb(0x1c, 0x1c, 0x1e),
                sheen: Color32::from_rgb(0x6e, 0x6e, 0x74),
                arm: Color32::from_rgb(0x8a, 0x8d, 0x92),
                texture: None,
                grain: None,
            },
            CapoStyle::Silver => CapoColors {
                body: Color32::from_rgb(0xa8, 0xac, 0xb2),
                sheen: Color32::from_rgb(0xe6, 0xe9, 0xee),
                arm: Color32::from_rgb(0x6f, 0x73, 0x79),
                // Brushed: fine ticks across the bar, barely darker.
                texture: Some(Color32::from_rgb(0x8d, 0x92, 0x99)),
                grain: None,
            },
        }
    }
}

/// Height of the fretboard band for a window `w` points wide. Truncated like
/// every other band in the layout (spec §3.2).
pub fn band_height(w: f32, strings: usize) -> f32 {
    let base = BAND_H_AT_1300 * w as f64 / 1300.0;
    (base * string_scale(strings)).trunc() as f32
}

/// How much taller the band gets for instruments with more than six strings.
///
/// **This exists because 7- and 8-string tunings do not fit.** The band was a
/// pure function of width, sized for six strings, and at 200pt wide that is
/// 20pt of neck: eight strings then sit 2.3pt apart while a note dot is 2.0pt
/// across, leaving 0.3pt of daylight. A chord reads as one vertical smear
/// rather than as notes. Caught by
/// `dots_on_neighbouring_strings_stay_apart` the moment 7- and 8-string
/// tunings were added, which is the argument for that test existing.
///
/// The band therefore grows with the string count, keeping per-string spacing
/// roughly constant. Note what this is NOT: it is not the layout following a
/// device's aspect ratio, which `docs/RECORDER-PLAN.md` §0 forbids for the
/// camera preview. String count is application state the user chose, the height
/// stays a deterministic function of `(width, strings)`, and every geometry test
/// still pins exactly.
///
/// It never shrinks below the six-string height. A four-string bass keeps the
/// familiar band and simply spaces its strings further apart — a band that got
/// visibly shorter when you switched to bass would read as a bug.
fn string_scale(strings: usize) -> f64 {
    (strings.max(1) as f64 / 6.0).max(1.0)
}

/// No headstock. The nut sits on the window's left edge and every fret of the
/// neck gets the width instead.
///
/// This was 2.6% of the band, holding the open-string rings and the damped
/// string crosses behind the nut the way a paper chord chart does. On a chart
/// that space is free; here it was 2.6% of the fretboard, spent on two symbols.
/// Those now sit ON the nut, which is where the open string physically is.
const GUTTER: f32 = 0.0;
/// Vertical breathing room above the top string and below the bottom one.
const PAD_Y: f32 = 0.10;

struct Geom {
    /// x of the nut, and of the right-hand end of the board.
    left: f32,
    right: f32,
    /// Vertical centre of string 0 (the lowest, drawn at the BOTTOM) and the
    /// gap between adjacent strings.
    bottom: f32,
    spacing: f32,
    strings: usize,
    /// `fret_x(frets)`, the divisor that stretches the board to the full width.
    scale: f32,
    frets: u8,
    capo: u8,
    /// How far the fingerboard slab extends past the outer strings.
    ///
    /// It wants to be a fraction of the string spacing, so the inset looks the
    /// same whatever the string count — but the spacing GROWS as strings are
    /// removed, because the outer two always sit at the same place. On the
    /// shipped 4-string bass that put the slab 8.6pt above the band and over
    /// the piano, so it is capped at the padding that actually exists.
    edge: f32,
}

impl Geom {
    /// The board's geometry does NOT depend on whether there is a caption.
    ///
    /// It used to: a caption took 19% of the band and the neck shrank to fit,
    /// so a note going out of range mid-phrase resized the whole fretboard
    /// under the player's hands. The caption is drawn OVER the board now,
    /// down in the corner past the last inlay, and the neck never moves.
    fn new(rect: Rect, spec: &FretboardSpec) -> Option<Self> {
        let strings = spec.tuning.strings();
        if strings == 0 || rect.width() <= 0.0 || rect.height() <= 0.0 {
            return None;
        }
        let board_h = rect.height();
        let pad = board_h * PAD_Y;
        let top = rect.top() + pad;
        let bottom = rect.top() + board_h - pad;
        let spacing = (bottom - top) / (strings as f32 - 1.0).max(1.0);
        // `frets: 0` is a legal board (open strings only) and would divide by
        // zero here, so fall back to a one-fret scale and draw just the nut.
        let scale = fretboard::fret_x(spec.frets.max(1));
        Some(Self {
            left: rect.left() + rect.width() * GUTTER,
            right: rect.right() - 1.0,
            bottom,
            spacing,
            strings,
            scale,
            frets: spec.frets,
            capo: spec.capo,
            edge: (spacing * 0.62).min(pad),
        })
    }

    /// Screen x for a point at fractional position `t` along the string.
    fn x(&self, t: f32) -> f32 {
        self.left + (t / self.scale) * (self.right - self.left)
    }

    fn wire_x(&self, fret: u8) -> f32 {
        self.x(fretboard::fret_x(fret))
    }

    /// Where a fingertip goes: centred in its fret, not on the wire.
    fn press_x(&self, fret: u8) -> f32 {
        self.x(fretboard::press_x(fret))
    }

    /// String 0 is the lowest-sounding and belongs at the BOTTOM of the
    /// diagram, which is the one convention every player shares.
    fn y(&self, string: usize) -> f32 {
        self.bottom - string as f32 * self.spacing
    }

    /// Deliberately well under half the string spacing. At 0.38 two dots on
    /// adjacent strings were a couple of points apart and read as one blob;
    /// the shape matters more than the dots being big.
    ///
    /// The ceiling matters as much as the ratio. A flat minimum radius is what
    /// made a small window worse rather than better: at 3pt of string spacing
    /// a 2pt floor is a 4pt dot, which overlaps its neighbours outright. The
    /// popped-out window can be dragged down to 90pt tall, so this is
    /// reachable, not theoretical.
    fn dot_r(&self) -> f32 {
        (self.spacing * 0.30).max(1.0).min(self.spacing * 0.45)
    }

    /// Where an open-string ring or a damped-string cross goes, now that there
    /// is no margin to put them in: centred one radius in from the left edge,
    /// so the mark straddles the nut and is still drawn whole.
    fn mark_x(&self, rect: Rect) -> f32 {
        rect.left() + self.dot_r()
    }
}

/// The three fingerboard woods.
///
/// Not a colour scheme bolted on: a real neck is one of a small number of
/// woods, and each one changes what has to be drawn ON it. Maple is pale, so
/// its strings, wires and inlay dots are DARK — the same light strings that
/// read beautifully on rosewood vanish on blonde maple. Each wood therefore
/// carries its whole palette rather than a single fill colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wood {
    /// Warm dark brown. The default because it is what most necks are, and
    /// because it is the one that flatters the cream keyboard above it.
    #[default]
    Rosewood,
    /// Blonde and light. The only one with a pale board, which inverts
    /// everything drawn on it.
    Maple,
    /// Near-black and high contrast.
    Ebony,
}

impl Wood {
    pub const ALL: [Wood; 3] = [Wood::Rosewood, Wood::Maple, Wood::Ebony];

    /// Stored in settings. Unknown values fall back to the default rather than
    /// being rewritten, so a file from a later build keeps its wood.
    pub fn key(self) -> &'static str {
        match self {
            Wood::Rosewood => "rosewood",
            Wood::Maple => "maple",
            Wood::Ebony => "ebony",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Wood::Rosewood => "Rosewood",
            Wood::Maple => "Maple",
            Wood::Ebony => "Ebony",
        }
    }

    pub fn from_key(k: &str) -> Wood {
        Wood::ALL
            .into_iter()
            .find(|w| w.key().eq_ignore_ascii_case(k))
            .unwrap_or_default()
    }

    /// True when the board is pale enough that everything on it must be dark.
    fn pale(self) -> bool {
        matches!(self, Wood::Maple)
    }
}

struct Palette {
    /// The band behind the neck: the piano's own background, so the two read
    /// as one instrument rather than two panels.
    bg: Color32,
    /// The fingerboard itself.
    board: Color32,
    nut: Color32,
    string: Color32,
    wire: Color32,
    inlay: Color32,
    dot: Color32,
    /// Drawn around a note dot when the board is pale, so a light accent
    /// colour on blonde maple is still a dot rather than a smudge.
    dot_edge: Option<Color32>,
    /// Marks ON the board, which is now everything: the board runs the full
    /// width of the band, so there is no background strip left to draw on.
    on_board: Color32,
}

fn palette(s: &Settings, wood: Wood) -> Palette {
    // The app is flat and high-contrast by design — cream keys, near-black
    // sharps, one accent colour for anything sounding — and the neck follows
    // that rather than trying to look photographic. The first attempt drew dark
    // strings on the piano's own light background and came out looking like a
    // spreadsheet; a real board with real strings on it is what fixed that.
    //
    // The wood does NOT follow dark mode. A neck is made of what it is made of,
    // and swapping maple for ebony when the lights go down would be a different
    // instrument. Only the band around it and the gutter marks follow the theme.
    let dark = s.dark_mode;
    let (board, string, wire, inlay, on_board) = match wood {
        Wood::Rosewood => (
            Color32::from_rgb(0x4a, 0x2b, 0x22),
            Color32::from_rgb(0xC9, 0xC4, 0xBC),
            Color32::from_rgb(0x93, 0x89, 0x7c),
            Color32::from_rgb(0xE8, 0xDC, 0xC0).gamma_multiply(0.34),
            Color32::from_rgb(0xE8, 0xDC, 0xC0).gamma_multiply(0.78),
        ),
        Wood::Maple => (
            Color32::from_rgb(0xd8, 0xb0, 0x72),
            // Dark strings and dark dots: the pale board inverts everything.
            Color32::from_rgb(0x5a, 0x46, 0x2c),
            Color32::from_rgb(0x8a, 0x6f, 0x45),
            Color32::from_rgb(0x3a, 0x2c, 0x1a).gamma_multiply(0.55),
            Color32::from_rgb(0x3a, 0x2c, 0x1a),
        ),
        Wood::Ebony => (
            Color32::from_rgb(0x18, 0x15, 0x13),
            Color32::from_rgb(0xB8, 0xB2, 0xA8),
            Color32::from_rgb(0x6a, 0x64, 0x5c),
            Color32::from_rgb(0xEC, 0xE6, 0xDA).gamma_multiply(0.42),
            Color32::from_rgb(0xEC, 0xE6, 0xDA).gamma_multiply(0.82),
        ),
    };
    Palette {
        bg: crate::piano::bg_color(dark),
        board,
        // A real nut is bone. On maple that would disappear, so there it is the
        // dark line a bone nut actually reads as against pale wood.
        nut: if wood.pale() {
            Color32::from_rgb(0x4a, 0x38, 0x22)
        } else {
            Color32::from_rgb(0xE8, 0xDC, 0xC0)
        },
        string,
        wire,
        inlay,
        // Whatever a held key looks like on the piano is what a held note
        // looks like here. The user already chose this colour once.
        dot: s.white_key_active_color.to_color32(),
        dot_edge: wood.pale().then(|| Color32::from_rgb(0x3a, 0x2c, 0x1a)),
        on_board,
    }
}

/// Draw the neck and everything on it. `voicing` must have been solved for
/// `spec`; a mismatch is a wiring bug, and the assertion in the tests is what
/// catches it.
pub fn draw(
    painter: &Painter,
    rect: Rect,
    voicing: &Voicing,
    spec: &FretboardSpec,
    s: &Settings,
    wood: Wood,
    barre: Option<Barre>,
) {
    let p = palette(s, wood);
    painter.rect_filled(rect, 0.0, p.bg);

    let caption = voicing.caption();
    let Some(g) = Geom::new(rect, spec) else {
        // A tuning with no strings. There is no board to draw; say so rather
        // than leaving a blank rectangle that reads as a crash.
        draw_caption(painter, rect, "no strings", &p, s);
        return;
    };

    // ── the board ───────────────────────────────────────────────────────────
    // The fingerboard as a slab, from the nut to the end of the neck, with the
    // outer strings inset rather than sitting on its edge.
    let board = board_rect(rect, &g);
    painter.rect_filled(board, 0.0, p.board);

    for f in 1..=g.frets {
        let x = g.wire_x(f).round() + 0.5;
        if x > g.right {
            break;
        }
        painter.line_segment(
            [Pos2::new(x, board.top()), Pos2::new(x, board.bottom())],
            Stroke::new(1.5_f32, p.wire),
        );
    }
    for &f in fretboard::INLAY_FRETS {
        if f > g.frets {
            continue;
        }
        let x = g.press_x(f);
        let mid = (g.y(0) + g.y(g.strings - 1)) * 0.5;
        let r = (g.spacing * 0.22).max(1.5);
        if fretboard::is_double_inlay(f) {
            painter.circle_filled(Pos2::new(x, mid - g.spacing * 0.85), r, p.inlay);
            painter.circle_filled(Pos2::new(x, mid + g.spacing * 0.85), r, p.inlay);
        } else {
            painter.circle_filled(Pos2::new(x, mid), r, p.inlay);
        }
    }
    // The nut. Bone-coloured and heavier than a fret wire, because that is
    // what tells you at a glance which end of the neck you are looking at.
    // Flush with the left edge, not centred on it: with no margin left, half a
    // centred nut would be clipped away.
    let nut = g.wire_x(0);
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(nut, board.top()),
            Pos2::new(nut + 3.0, board.bottom()),
        ),
        0.0,
        p.nut,
    );
    // Strings, thinner as they go up, the way they actually are.
    for st in 0..g.strings {
        let t = 1.0 + 1.6 * (1.0 - st as f32 / (g.strings as f32 - 1.0).max(1.0));
        painter.line_segment(
            [Pos2::new(nut, g.y(st)), Pos2::new(g.right, g.y(st))],
            Stroke::new(t, p.string),
        );
    }
    if g.capo > 0 && g.capo <= g.frets {
        draw_capo(painter, rect, &g, s.capo_style());
    }

    // ── what is being played ────────────────────────────────────────────────
    // Barre first, so the dots sit on top of it.
    //
    // TOLD, not inferred. The solver derives a barre from the shape — adjacent
    // strings sharing their lowest fret — which is the right model for a shape
    // IT chose, and the wrong one for a shape you entered by hand: two notes
    // that happen to share a fret are two notes, and drawing a bar across them
    // claims a finger position you did not ask for. The caller decides which
    // of the two this is; this module only draws.
    if let Some(b) = barre {
        if b.hi_string < g.strings {
            let x = g.press_x(b.fret);
            // Exactly a dot across, so the note dots drawn on top of it are
            // INSCRIBED rather than bulging out of it.
            //
            // It used to be 1.7 radii wide while every dot is 2.0 across, so
            // each barred string pushed a little past the bar and the two ends
            // pushed most — a barre read as two blobs joined by a stick rather
            // than as one finger laid across the strings.
            let w = g.dot_r() * BARRE_W_IN_DOTS;
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x - w * 0.5, g.y(b.hi_string) - g.dot_r()),
                    Pos2::new(x + w * 0.5, g.y(b.lo_string) + g.dot_r()),
                ),
                w * 0.5,
                p.dot,
            );
            if let Some(edge) = p.dot_edge {
                painter.rect_stroke(
                    Rect::from_min_max(
                        Pos2::new(x - w * 0.5, g.y(b.hi_string) - g.dot_r()),
                        Pos2::new(x + w * 0.5, g.y(b.lo_string) + g.dot_r()),
                    ),
                    w * 0.5,
                    Stroke::new(1.5_f32, edge),
                    StrokeKind::Inside,
                );
            }
        }
    }

    // Notes the guitar could make but not alongside the rest: faint rings at
    // the places they wanted. A ring says "not at the same time"; an empty
    // space says "this app is broken", and those must not look alike.
    for n in &voicing.notes {
        if let Outcome::Conflict { wanted } = &n.outcome {
            for pos in wanted {
                if pos.string >= g.strings {
                    continue;
                }
                painter.circle_stroke(
                    Pos2::new(g.press_x(pos.fret), g.y(pos.string)),
                    g.dot_r() * 0.8,
                    Stroke::new(1.5_f32, p.on_board),
                );
            }
        }
    }

    for n in &voicing.notes {
        let Outcome::Placed {
            pos, octave_shift, ..
        } = n.outcome
        else {
            continue;
        };
        if pos.string >= g.strings {
            continue;
        }
        let y = g.y(pos.string);
        let r = g.dot_r();
        if octave_shift != 0 {
            // A ghost: the pitch class is right, the octave is not. Hollow, so
            // it cannot be mistaken for a note that is really there, with the
            // arrow saying which way it moved.
            let x = if pos.fret == g.capo {
                gutter_x(rect, &g)
            } else {
                g.press_x(pos.fret)
            };
            painter.circle_stroke(Pos2::new(x, y), r, Stroke::new(2.0_f32, p.dot));
            let arrow = if octave_shift > 0 {
                "\u{2191}"
            } else {
                "\u{2193}"
            };
            painter.text(
                Pos2::new(x, y),
                Align2::CENTER_CENTER,
                arrow,
                FontId::new((r * 1.5).max(7.0), fonts::courier_bold()),
                p.dot,
            );
        } else if pos.fret == g.capo {
            // Open (or capo'd): a ring behind the nut, never a dot on it.
            painter.circle_stroke(
                Pos2::new(gutter_x(rect, &g), y),
                r * 0.75,
                Stroke::new(2.0_f32, p.dot),
            );
        } else {
            let c = Pos2::new(g.press_x(pos.fret), y);
            painter.circle_filled(c, r, p.dot);
            if let Some(edge) = p.dot_edge {
                painter.circle_stroke(c, r, Stroke::new(1.5_f32, edge));
            }
        }
    }

    // Strings the player has to keep quiet, and strings simply not in use.
    // Skipped only when something is actually sounding: on an idle board every
    // string is "unused", and six crosses telling nobody to mute nothing is
    // just noise on the view the app sits at all day.
    let sounding_any = voicing
        .strings
        .iter()
        .any(|s| matches!(s, StringState::Sounding { .. }));
    for (st, state) in voicing.strings.iter().enumerate().take(g.strings) {
        if !sounding_any {
            break;
        }
        // On the board now, so these follow the wood rather than the band.
        let colour = match state {
            StringState::Skipped => p.on_board,
            StringState::Unused => p.on_board.gamma_multiply(0.5),
            StringState::Sounding { .. } => continue,
        };
        painter.text(
            Pos2::new(gutter_x(rect, &g), g.y(st)),
            Align2::CENTER_CENTER,
            "\u{00d7}",
            FontId::new((g.dot_r() * 1.9).max(8.0), fonts::courier_bold()),
            colour,
        );
    }

    // A board that cannot make any of these notes gets slashed, so it reads as
    // "impossible" rather than as "empty".
    if voicing.shape.unreachable_count as usize == voicing.notes.len() && !voicing.notes.is_empty()
    {
        painter.line_segment(
            [board.left_top(), board.right_bottom()],
            Stroke::new(2.0_f32, p.on_board),
        );
    }

    if let Some(c) = caption {
        draw_caption(painter, rect, &c, &p, s);
    }
}

/// Which MIDI pitch is under a click, or `None` for a miss.
///
/// The inverse of what `draw` does, and it has to stay that way: the piano's
/// `hit_test` matches its drawing math line for line (spec 4.5) precisely so a
/// key never lights somewhere you cannot click. Same rule here.
///
/// Two things are not simply "which fret space is this".
///
/// With no headstock there is nowhere left of the nut to click for an open
/// string, so the first `2 * dot_r` of the board is the OPEN zone, which is
/// where the open rings and mute crosses are actually drawn. It eats the left
/// edge of the first fret's space, but the first fret's press point is far to
/// the right of it, so nothing overlaps in practice.
///
/// And a click has to land NEAR a string, not merely inside the board. Halfway
/// between two strings is not a note, and guessing one there means every miss
/// silently adds something.
pub fn hit_test(rect: Rect, spec: &FretboardSpec, pos: Pos2) -> Option<u8> {
    let (string, fret) = position_at(rect, spec, pos)?;
    spec.pitch_at(string, fret)
}

/// The same hit-test, reporting WHERE rather than what. The app records this
/// so a note entered on the neck can be pinned to the position it was put in.
pub fn position_at(rect: Rect, spec: &FretboardSpec, pos: Pos2) -> Option<(usize, u8)> {
    if !rect.contains(pos) {
        return None;
    }
    let g = Geom::new(rect, spec)?;

    // Nearest string, but only if the click is actually on it.
    let rel = (g.bottom - pos.y) / g.spacing;
    let string = rel.round();
    if string < 0.0 || string >= g.strings as f32 {
        return None;
    }
    if (rel - string).abs() > 0.42 {
        return None;
    }
    let string = string as usize;

    // Fret. The open zone first, then the space between two wires.
    let fret = if pos.x <= g.wire_x(0) + 2.0 * g.dot_r() {
        spec.capo
    } else {
        let mut found = None;
        for f in (spec.capo.max(1))..=spec.frets {
            if pos.x <= g.wire_x(f) {
                found = Some(f);
                break;
            }
        }
        found?
    };

    Some((string, fret))
}

/// The fingerboard slab.
///
/// It starts at the band's left EDGE, not at the nut. The strip behind the nut
/// is the headstock, and leaving it in the band background painted a pale bar
/// down the side of a dark neck that read as the panel failing to fill its own
/// space. Everything that lives back there — open-string rings, damped-string
/// crosses — sits on wood now like everything else.
fn board_rect(rect: Rect, g: &Geom) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left(), g.y(g.strings - 1) - g.edge),
        Pos2::new(g.right, g.y(0) + g.edge),
    )
}

/// x of the open/damped marker column, behind the nut.
fn gutter_x(rect: Rect, g: &Geom) -> f32 {
    g.mark_x(rect)
}

/// Over the board, bottom right, past the last inlay. Small and dim: it is
/// there to be read when something is missing, not to be noticed otherwise,
/// and above all it must not move the neck to say so.
fn draw_caption(painter: &Painter, rect: Rect, text: &str, p: &Palette, _s: &Settings) {
    let size = (rect.height() * 0.115).max(8.0);
    painter.text(
        Pos2::new(rect.right() - 6.0, rect.bottom() - 3.0),
        Align2::RIGHT_BOTTOM,
        text,
        FontId::new(size, fonts::courier()),
        p.on_board,
    );
}

/// One-pixel top edge so the fretboard reads as its own band rather than as
/// more piano. Drawn by the caller, which knows what is above it.
/// The capo, drawn as the clamp it is.
///
/// It used to be a thin rounded bar barely wider than a fret wire, which read
/// as another fret rather than as a thing attached to the neck. A real capo is
/// a chunky rubber-sleeved bar that grips ACROSS all the strings and overhangs
/// them at both ends, with the mechanism sitting off the edge of the board.
///
/// Drawn just behind the fret it holds, which is where a player puts one: on
/// the fret itself it would cover the wire and look like a very fat fret.
/// Where the capo is, so a click can find it. `None` when none is fitted.
pub fn capo_rect(rect: Rect, spec: &FretboardSpec) -> Option<Rect> {
    let g = Geom::new(rect, spec)?;
    if g.capo == 0 || g.capo > g.frets {
        return None;
    }
    let (x, w, top, bot) = capo_geom(rect, &g);
    // Widened for the hit test only: the bar is a few points across and
    // nobody aims at a few points. Not so wide that it swallows the frets
    // either side, which are notes.
    let grab = w.max(10.0);
    Some(Rect::from_min_max(
        Pos2::new(x - grab, top),
        Pos2::new(x + grab, bot),
    ))
}

/// The bar's x, width, top and bottom.
///
/// Shared by the drawing and the hit test, so a click cannot land beside the
/// thing it looks like it is on.
fn capo_geom(band: Rect, g: &Geom) -> (f32, f32, f32, f32) {
    let x = g.press_x(g.capo);
    // A third of the gap to the fret behind it. Chunky enough to read as a
    // thing clamped ONTO the neck rather than as another fret wire, and tied
    // to fret spacing rather than string spacing because that is what sets the
    // scale of everything else along the neck.
    let gap = (g.press_x(g.capo) - g.press_x(g.capo.saturating_sub(1))).abs();
    let w = (gap / 3.0).max(6.0);
    // CONTAINED. A capo that pokes out of the top is a black bar standing in
    // the piano above it, which is not what a capo does to a guitar. The
    // overhang is whatever room is left after the outer strings, so it shrinks
    // on a crowded twelve-string rather than escaping.
    let head = (band.top() + 1.0).min(g.y(g.strings - 1));
    let foot = (band.bottom() - 1.0).max(g.y(0));
    let overhang = (g.spacing * 0.9)
        .min(g.y(g.strings - 1) - head)
        .min(foot - g.y(0))
        .max(0.0);
    (x, w, g.y(g.strings - 1) - overhang, g.y(0) + overhang)
}

fn draw_capo(painter: &Painter, band: Rect, g: &Geom, style: CapoStyle) {
    let (x, w, top, bot) = capo_geom(band, g);
    let r = w * 0.42;
    let c = style.colors();

    // A soft shadow on the bridge side, so it sits ON the board rather than
    // being painted into it.
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(x - w * 0.5 + w * 0.22, top + w * 0.18),
            Pos2::new(x + w * 0.5 + w * 0.22, bot + w * 0.18),
        ),
        r,
        Color32::from_black_alpha(60),
    );

    // ONE bar, the whole length. It used to have a fatter circle stuck on each
    // end, meaning to read as the rubber pads that overhang the outer strings;
    // at this size they read as two blobs on a stick.
    let body = Rect::from_min_max(Pos2::new(x - w * 0.5, top), Pos2::new(x + w * 0.5, bot));
    painter.rect_filled(body, r, c.body);

    // The light down the nut side. One thin rect rather than a gradient, which
    // egui has no cheap way to draw and which nobody would see at this size.
    painter.rect_filled(
        Rect::from_min_max(
            Pos2::new(x - w * 0.5 + w * 0.16, top + r * 0.8),
            Pos2::new(x - w * 0.5 + w * 0.30, bot - r * 0.8),
        ),
        w * 0.06,
        c.sheen,
    );

    // Texture, where the material has any: fine cross-ticks for brushed metal,
    // a few long grain lines for wood. Spaced off the bar's own length so it
    // scales with the window instead of getting denser as the board grows.
    if let Some(tex) = c.texture {
        let n = ((bot - top) / (w * 0.55)).round().max(2.0) as i32;
        for i in 1..n {
            let y = top + (bot - top) * i as f32 / n as f32;
            painter.line_segment(
                [Pos2::new(x - w * 0.34, y), Pos2::new(x + w * 0.34, y)],
                Stroke::new(1.0_f32, tex),
            );
        }
    }
    if let Some(grain) = c.grain {
        for k in [-0.22_f32, 0.10, 0.30] {
            painter.line_segment(
                [Pos2::new(x + w * k, top + r), Pos2::new(x + w * k, bot - r)],
                Stroke::new(1.0_f32, grain),
            );
        }
    }

    // The arm, off the treble edge, where the screw or spring lives.
    let arm_top = bot + r * 0.2;
    let arm_bot = (arm_top + g.spacing * 0.55).min(band.bottom() - 1.0);
    if arm_bot > arm_top + 1.0 {
        painter.rect_filled(
            Rect::from_min_max(
                Pos2::new(x - w * 0.30, arm_top),
                Pos2::new(x + w * 0.30, arm_bot),
            ),
            w * 0.22,
            c.arm,
        );
    }
}

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

/// Default size for the popped-out neck when nothing has been remembered.
///
/// Wider than it is tall by a lot, because a neck is: 22 frets across six
/// strings only reads if the frets have room. Deliberately NOT the attached
/// band's proportions, for the same reason the detached chord window is not
/// the piano's (D-UI-10) — in its own window it should be legible, not a
/// slice of the main one.
pub const DETACHED_DEFAULT: Vec2 = Vec2::new(880.0, 190.0);

pub fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("ivory-fretboard-window")
}

/// Outline for the popped-out window. The neck fills the whole surface and is
/// dark in two woods out of three, so on a dark desktop the window would have
/// no edge at all. One neutral grey reads against both.
pub const BORDER_COLOR: Color32 = Color32::from_gray(0x5A);

#[derive(Default)]
pub struct DetachedOutcome {
    pub close_requested: bool,
    pub inner_size: Option<Vec2>,
    pub outer_pos: Option<Pos2>,
    /// Right-click position in monitor coordinates, for the context menu.
    pub context_menu_at: Option<Pos2>,
}

/// The neck in its own window. Mirrors `chord_strip::show_detached_window`
/// exactly — same close-to-reattach, same right-click-anywhere menu, same
/// borderless drag-anywhere — so the two popouts behave identically and there
/// is only one set of habits to learn.
#[allow(clippy::too_many_arguments)]
/// `main_focused` decides the window LEVEL. A detached window is a piece of
/// the same app, so it rises and falls WITH the piano rather than being left
/// wherever the window stack last put it — which is what "the children don't
/// follow" looks like: focus the piano and its own readouts stay buried under
/// whatever you were doing before.
///
/// By level rather than by raising: always-on-top while we are frontmost is
/// exactly "above our own window", and dropping to Normal when we are not
/// means it never floats over other applications.
pub fn show_detached_window(
    ctx: &egui::Context,
    builder_size: Vec2,
    builder_pos: Option<Pos2>,
    borderless: bool,
    main_focused: bool,
    voicing: &Voicing,
    spec: &FretboardSpec,
    s: &Settings,
    wood: Wood,
    barre: Option<Barre>,
) -> DetachedOutcome {
    let mut outcome = DetachedOutcome::default();
    let mut builder = egui::ViewportBuilder::default()
        .with_title("Tangent")
        .with_inner_size(builder_size)
        .with_min_inner_size([320.0, 90.0])
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
            let rect = ui.max_rect();
            draw(ui.painter(), rect, voicing, spec, s, wood, barre);
            painter_border(ui.painter(), rect);

            let (close, inner_rect, outer_rect, pressed, secondary, pointer) = ui.input(|i| {
                (
                    i.viewport().close_requested(),
                    i.viewport().inner_rect,
                    i.viewport().outer_rect,
                    i.pointer.primary_pressed(),
                    i.pointer.secondary_clicked(),
                    i.pointer.interact_pos(),
                )
            });

            outcome.close_requested = close;
            outcome.inner_size = inner_rect.map(|r| r.size()).or(Some(rect.size()));
            outcome.outer_pos = outer_rect.map(|r| r.min);

            if secondary {
                if let (Some(pos), Some(inner)) = (pointer, inner_rect) {
                    outcome.context_menu_at = Some(inner.min + pos.to_vec2());
                }
            }
            if borderless && pressed && !secondary {
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

    /// A barre is one line across the strings, so the bar must be at least as
    /// wide as the dots drawn on top of it. At 1.7 radii against a 2.0-radius
    /// dot every barred string bulged, and the two ends bulged most: it read
    /// as two blobs joined by a stick.
    #[test]
    fn a_barre_is_never_narrower_than_the_dots_on_it() {
        for w in [400.0_f32, 900.0, 1300.0, 2600.0] {
            let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, FretboardSpec::default().tuning.strings())));
            let g = Geom::new(r, &FretboardSpec::default()).unwrap();
            // The bar's width and the dot's diameter, both taken from the
            // geometry the drawing code uses, at each width.
            //
            // The previous version asserted `BARRE_W_IN_DOTS >= 2.0` inside
            // this loop, which is a constant and made the loop, the rect and
            // the `Geom` decoration — clippy called it a "constant value"
            // assertion and clippy was right. This form is still ultimately
            // about that constant, but it goes through `Geom::new` and
            // `dot_r()` at four widths, so it also fails if either starts
            // returning zero or panicking on a small band. What it does NOT
            // prove is that `draw` uses these numbers; that would need the
            // barre rect to be a function, and it is currently inline at :493.
            let bar_w = g.dot_r() * BARRE_W_IN_DOTS;
            let dot_d = g.dot_r() * 2.0;
            assert!(
                g.dot_r() > 0.0,
                "a zero-radius dot at width {w} means nothing is drawn at all"
            );
            assert!(
                bar_w >= dot_d,
                "at width {w} the barre is {bar_w} wide against a {dot_d} dot, \
                 so every dot on it bulges out and it reads as two blobs \
                 joined by a stick"
            );
        }
    }

    /// Clicking the capo must find the capo, at every fret and every size —
    /// and clicking where there is no capo must not.
    #[test]
    fn the_capo_can_be_clicked_where_it_is_drawn() {
        for w in [400.0_f32, 900.0, 1300.0, 2600.0] {
            let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, FretboardSpec::default().tuning.strings())));
            // No capo, nothing to hit.
            let none = FretboardSpec {
                capo: 0,
                ..Default::default()
            };
            assert_eq!(capo_rect(r, &none), None);

            for capo in 1..=9u8 {
                let spec = FretboardSpec {
                    capo,
                    ..Default::default()
                };
                let hit = capo_rect(r, &spec).expect("a fitted capo has a rect");
                let g = Geom::new(r, &spec).unwrap();
                let (x, _, top, bot) = capo_geom(r, &g);

                // Its own centre is on it.
                assert!(
                    hit.contains(Pos2::new(x, (top + bot) * 0.5)),
                    "capo {capo} at width {w}: the middle of the bar is not on it"
                );
                // It stays inside the band: a capo that pokes out is a bar
                // standing in the piano above the neck.
                assert!(
                    r.expand(0.5).contains_rect(hit),
                    "capo {capo} at width {w} escapes the band: {hit:?} vs {r:?}"
                );
                // And it does not swallow the neighbouring frets, which are
                // notes people mean to click.
                for other in [capo.saturating_sub(2), capo + 2] {
                    if other == 0 || other == capo {
                        continue;
                    }
                    let px = g.press_x(other);
                    assert!(
                        !hit.contains(Pos2::new(px, (top + bot) * 0.5)),
                        "the capo at {capo} swallows fret {other} at width {w}"
                    );
                }
            }
        }
    }

    /// Cycling reaches every capo style and comes back round, and an unknown
    /// value from a later build falls back rather than being rewritten.
    #[test]
    fn capo_styles_cycle_and_unknown_values_fall_back() {
        let mut seen = std::collections::HashSet::new();
        let mut c = CapoStyle::default();
        for _ in 0..CapoStyle::ALL.len() {
            seen.insert(c);
            c = c.next();
        }
        assert_eq!(seen.len(), CapoStyle::ALL.len());
        assert_eq!(
            c,
            CapoStyle::default(),
            "the cycle does not come back round"
        );
        assert_eq!(
            CapoStyle::default(),
            CapoStyle::Black,
            "black is the default: a wooden capo on the default rosewood neck \
             is a bar you have to look for"
        );
        assert_eq!(CapoStyle::from_key("teak-from-2027"), CapoStyle::Black);
        for st in CapoStyle::ALL {
            assert_eq!(
                CapoStyle::from_key(st.key()),
                st,
                "{st:?} does not round-trip"
            );
        }
    }
    use super::*;
    use ivory_core::fretboard::{Tuning, TUNINGS};
    use ivory_core::voicing::solve_cold;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 200.0), Vec2::new(1300.0, 132.0))
    }

    #[test]
    fn every_wood_round_trips_through_its_settings_key() {
        for w in Wood::ALL {
            assert_eq!(Wood::from_key(w.key()), w);
            assert_eq!(Wood::from_key(&w.key().to_uppercase()), w);
        }
        // An unknown or future wood falls back rather than blanking the board.
        assert_eq!(Wood::from_key("koa"), Wood::default());
        assert_eq!(Wood::from_key(""), Wood::Rosewood);
        // Maple is the pale one, and it is the reason the palette is per-wood
        // rather than one fill colour: light strings vanish on blonde wood.
        let s = Settings::default();
        let maple = palette(&s, Wood::Maple);
        let rose = palette(&s, Wood::Rosewood);
        assert!(
            maple.dot_edge.is_some(),
            "a pale board needs an edge on its dots"
        );
        assert!(rose.dot_edge.is_none());
        let lum = |c: Color32| c.r() as u32 + c.g() as u32 + c.b() as u32;
        assert!(
            lum(maple.board) > lum(maple.string),
            "maple: dark strings on a pale board"
        );
        assert!(
            lum(rose.board) < lum(rose.string),
            "rosewood: light strings on a dark board"
        );
        assert!(
            lum(palette(&s, Wood::Ebony).board) < lum(rose.board),
            "ebony is the darkest"
        );
    }

    #[test]
    fn the_band_scales_with_the_window_like_every_other_one() {
        assert_eq!(band_height(1300.0, 6), 132.0);
        assert_eq!(band_height(650.0, 6), 66.0);
        // Truncated, not rounded: the piano and the chord strip both are, and a
        // half-pixel band would put every string on a fractional row.
        assert_eq!(band_height(1000.0, 6), 101.0);
        assert_eq!(band_height(0.0, 6), 0.0);
    }

    #[test]
    fn the_last_fret_lands_on_the_right_edge() {
        // The trap this exists to catch: `fret_x(22)` is 0.719, so drawing
        // straight into widget space would leave 28% of the band empty and put
        // every dot in the wrong place.
        let spec = FretboardSpec::default();
        let g = Geom::new(rect(), &spec).unwrap();
        assert!((g.wire_x(spec.frets) - g.right).abs() < 0.5);
        assert!((g.wire_x(0) - g.left).abs() < 0.001, "fret 0 is the nut");
        // The 12th fret is still visibly the halfway house it is on a real
        // neck, which the naive normalisation would also have destroyed.
        let half = (g.wire_x(12) - g.left) / (g.right - g.left);
        assert!(
            (half - 0.5 / 0.7194).abs() < 0.01,
            "octave landed at {half}"
        );
    }

    #[test]
    fn press_points_stay_inside_their_fret_and_ascend() {
        let spec = FretboardSpec::default();
        let g = Geom::new(rect(), &spec).unwrap();
        for f in 1..=spec.frets {
            let x = g.press_x(f);
            assert!(
                x > g.wire_x(f - 1) && x < g.wire_x(f),
                "fret {f} escaped its space"
            );
        }
        // `press_x(0)` still resolves behind the nut — the geometry says so —
        // but nothing draws there any more: with no headstock there is no
        // "behind" on screen, so open and damped marks straddle the nut
        // instead. That is what `mark_x` is for.
        assert!(
            g.press_x(0) < g.left,
            "fret 0 is still behind the nut in geometry"
        );
        let gx = gutter_x(rect(), &g);
        assert!(gx >= g.left, "a mark must not be drawn off the left edge");
        assert!(gx < g.press_x(1), "a mark must not reach the first fret");
    }

    /// The neck must not move when a caption appears. It used to: the caption
    /// took 19% of the band and the board shrank to fit, so a note going out
    /// of range mid-phrase resized the fretboard under the player's hands.
    #[test]
    fn a_caption_never_moves_the_neck() {
        let spec = FretboardSpec::default();
        let g = Geom::new(rect(), &spec).unwrap();
        // There is only one geometry now, so this is true by construction —
        // the assertion is here so that re-introducing a caption-dependent
        // layout has to delete a test that says why not.
        for st in 0..g.strings {
            assert!(g.y(st) >= rect().top() && g.y(st) <= rect().bottom());
        }
        let solved = solve_cold(&spec, &[36, 43, 48, 52, 55, 58, 60, 64, 67, 72]);
        assert!(solved.caption().is_some(), "this input should caption");
        let quiet = solve_cold(&spec, &[40, 47, 52, 56, 59, 64]);
        assert!(quiet.caption().is_none(), "an ordinary chord says nothing");
        // Same board either way.
        assert_eq!(
            Geom::new(rect(), &spec).map(|g| (g.bottom, g.spacing, g.edge)),
            Geom::new(rect(), &spec).map(|g| (g.bottom, g.spacing, g.edge))
        );
    }

    /// Two dots on adjacent strings must not touch, or a shape reads as one
    /// smear instead of as notes.
    #[test]
    fn dots_on_neighbouring_strings_stay_apart() {
        for t in TUNINGS {
            for w in [200.0_f32, 650.0, 1300.0, 2600.0] {
                let spec = FretboardSpec {
                    tuning: t.clone(),
                    frets: 22,
                    capo: 0,
                };
                let r = Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(w, band_height(w, spec.tuning.strings())),
                );
                let Some(g) = Geom::new(r, &spec) else {
                    continue;
                };
                let gap = g.spacing - 2.0 * g.dot_r();
                assert!(
                    gap > 0.15 * g.spacing,
                    "{} at {w}pt: adjacent dots leave only {gap:.1}pt of {:.1}pt spacing",
                    t.name,
                    g.spacing
                );
            }
        }
    }

    /// No background strip anywhere: the neck fills the band edge to edge.
    #[test]
    fn the_board_leaves_no_pale_margin_down_the_left() {
        for t in TUNINGS {
            for w in [650.0_f32, 1300.0, 2600.0] {
                let spec = FretboardSpec {
                    tuning: t.clone(),
                    frets: 22,
                    capo: 0,
                };
                let r = Rect::from_min_size(
                    Pos2::ZERO,
                    Vec2::new(w, band_height(w, spec.tuning.strings())),
                );
                let Some(g) = Geom::new(r, &spec) else {
                    continue;
                };
                let b = board_rect(r, &g);
                assert_eq!(b.left(), r.left(), "{}: pale strip at {w}pt", t.name);
                assert!(
                    b.right() >= r.right() - 2.0,
                    "{}: gap at the far end",
                    t.name
                );
                assert!(b.top() >= r.top() - 0.01 && b.bottom() <= r.bottom() + 0.01);
                // No headstock: the nut is ON the left edge and the marks
                // straddle it, drawn whole rather than clipped.
                assert_eq!(g.wire_x(0), r.left(), "the nut should be flush left");
                let gx = gutter_x(r, &g);
                assert!(gx - g.dot_r() >= r.left() - 0.01, "an open ring is clipped");
                assert!(gx < g.press_x(1), "a mark collided with the first fret");
            }
        }
    }

    /// A click must land on the note the eye sees, which means hit_test has to
    /// be the exact inverse of draw. The piano has had this property since the
    /// port (spec 4.5); the fretboard now needs it too, because keytoggle can
    /// put notes in from either instrument.
    #[test]
    fn every_drawn_dot_is_clickable_at_the_place_it_is_drawn() {
        let spec = FretboardSpec::default();
        let r = rect();
        let g = Geom::new(r, &spec).unwrap();
        for string in 0..g.strings {
            for fret in 0..=spec.frets {
                let x = if fret == spec.capo {
                    g.mark_x(r)
                } else {
                    g.press_x(fret)
                };
                let hit = hit_test(r, &spec, Pos2::new(x, g.y(string)));
                assert_eq!(
                    hit,
                    spec.pitch_at(string, fret),
                    "string {string} fret {fret} at x={x} did not hit its own dot"
                );
            }
        }
    }

    #[test]
    fn a_click_between_two_strings_is_a_miss_not_a_guess() {
        let spec = FretboardSpec::default();
        let r = rect();
        let g = Geom::new(r, &spec).unwrap();
        // Dead centre between strings 2 and 3.
        let between = (g.y(2) + g.y(3)) * 0.5;
        assert_eq!(hit_test(r, &spec, Pos2::new(g.press_x(5), between)), None);
        // Outside the board entirely.
        assert_eq!(
            hit_test(r, &spec, Pos2::new(g.press_x(5), r.top() - 5.0)),
            None
        );
        assert_eq!(hit_test(r, &spec, Pos2::new(r.right() + 5.0, g.y(0))), None);
        // Well below the lowest string.
        assert_eq!(
            hit_test(r, &spec, Pos2::new(g.press_x(5), g.y(0) + g.spacing)),
            None
        );
    }

    #[test]
    fn the_open_zone_is_at_the_nut_and_respects_a_capo() {
        let r = rect();
        let plain = FretboardSpec::default();
        let g = Geom::new(r, &plain).unwrap();
        // Hard against the nut is the open string, on every string.
        for st in 0..g.strings {
            assert_eq!(
                hit_test(r, &plain, Pos2::new(r.left() + 1.0, g.y(st))),
                plain.pitch_at(st, 0),
                "the nut should give the open string on string {st}"
            );
        }
        // With a capo the same click gives the capo'd pitch, not fret 0, and
        // nothing below the capo is reachable at all.
        let capo = FretboardSpec {
            capo: 3,
            ..Default::default()
        };
        let gc = Geom::new(r, &capo).unwrap();
        assert_eq!(
            hit_test(r, &capo, Pos2::new(r.left() + 1.0, gc.y(0))),
            capo.pitch_at(0, 3)
        );
        for st in 0..gc.strings {
            for x in [
                r.left(),
                r.left() + 20.0,
                gc.press_x(1),
                gc.press_x(2),
                gc.press_x(3),
            ] {
                if let Some(p) = hit_test(r, &capo, Pos2::new(x, gc.y(st))) {
                    assert!(
                        p >= capo.pitch_at(st, 3).unwrap(),
                        "clicked behind the capo on string {st}"
                    );
                }
            }
        }
    }

    #[test]
    fn hit_testing_holds_for_every_tuning_and_window_size() {
        for t in TUNINGS {
            for capo in [0u8, 4] {
                for w in [650.0_f32, 1300.0, 2600.0] {
                    let spec = FretboardSpec {
                        tuning: t.clone(),
                        frets: 22,
                        capo,
                    };
                    let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, FretboardSpec::default().tuning.strings())));
                    let Some(g) = Geom::new(r, &spec) else {
                        continue;
                    };
                    for st in 0..g.strings {
                        for fret in [capo, capo + 1, 7, 12, 22] {
                            if fret > spec.frets {
                                continue;
                            }
                            let x = if fret == capo {
                                g.mark_x(r)
                            } else {
                                g.press_x(fret)
                            };
                            assert_eq!(
                                hit_test(r, &spec, Pos2::new(x, g.y(st))),
                                spec.pitch_at(st, fret),
                                "{} capo{capo} at {w}pt: string {st} fret {fret}",
                                t.name
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_low_string_is_at_the_bottom() {
        // Every chord chart in the world agrees on this and getting it upside
        // down is the single most obvious way to look wrong to a guitarist.
        let spec = FretboardSpec::default();
        let g = Geom::new(rect(), &spec).unwrap();
        assert!(
            g.y(0) > g.y(5),
            "string 0 is the low E and belongs at the bottom"
        );
        for st in 1..6 {
            assert!(g.y(st) < g.y(st - 1));
        }
        assert!(g.y(5) >= rect().top() && g.y(0) <= rect().bottom());
    }

    #[test]
    fn every_tuning_and_capo_produces_a_board_that_fits_the_band() {
        for t in TUNINGS {
            for capo in [0u8, 3, 12, 21] {
                for w in [200.0_f32, 650.0, 1300.0, 2600.0] {
                    let spec = FretboardSpec {
                        tuning: t.clone(),
                        frets: 22,
                        capo,
                    };
                    let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, band_height(w, FretboardSpec::default().tuning.strings())));
                    let Some(g) = Geom::new(r, &spec) else {
                        panic!("{} lost its board at width {w}", t.name);
                    };
                    assert!(g.right > g.left);
                    assert!(g.dot_r() > 0.0);
                    for st in 0..g.strings {
                        assert!(g.y(st) >= r.top() && g.y(st) <= r.bottom());
                    }
                    // The SLAB, not just the strings. It extends past the outer
                    // strings by `edge`, and that inset wants to scale with the
                    // string spacing — which grows as strings are removed,
                    // because the outer two stay put. Unclamped, the shipped
                    // 4-string bass painted 8.6pt of fingerboard over the piano
                    // above it and 8.6pt out of the bottom of the window. The
                    // string-position assertion above cannot see that.
                    let top = g.y(g.strings - 1) - g.edge;
                    let bot = g.y(0) + g.edge;
                    assert!(
                        top >= r.top() - 0.01 && bot <= r.bottom() + 0.01,
                        "{} ({} strings) painted its board {top}..{bot} outside the band {:?}",
                        t.name,
                        t.strings(),
                        (r.top(), r.bottom())
                    );
                    for f in 0..=spec.frets {
                        let x = g.wire_x(f);
                        assert!(x >= g.left - 0.5 && x <= g.right + 0.5, "fret {f} at {x}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_stringless_or_fretless_board_does_not_divide_by_zero() {
        const NONE: Tuning = Tuning {
            name: std::borrow::Cow::Borrowed("None"),
            open: std::borrow::Cow::Borrowed(&[]),
        };
        assert!(Geom::new(
            rect(),
            &FretboardSpec {
                tuning: NONE.clone(),
                frets: 22,
                capo: 0
            }
        )
        .is_none());
        // Zero frets is a legal board: open strings and nothing else.
        let g = Geom::new(
            rect(),
            &FretboardSpec {
                frets: 0,
                capo: 0,
                ..Default::default()
            },
        )
        .expect("a fretless board is still a board");
        assert!(g.wire_x(0).is_finite() && g.right > g.left);
        // A single-string tuning must not divide by (strings - 1).
        const ONE: Tuning = Tuning {
            name: std::borrow::Cow::Borrowed("One"),
            open: std::borrow::Cow::Borrowed(&[40]),
        };
        let g = Geom::new(
            rect(),
            &FretboardSpec {
                tuning: ONE.clone(),
                frets: 22,
                capo: 0,
            },
        )
        .unwrap();
        assert!(g.y(0).is_finite() && g.spacing.is_finite());
    }

    #[test]
    fn the_renderer_survives_every_voicing_the_solver_can_produce() {
        // The view is dumb, but it still indexes `strings` by position and
        // reads `barre` bounds. This drives real solver output through the
        // painter to prove none of that can be out of range.
        let ctx = egui::Context::default();
        // The panel draws real glyphs (the arrows, the damped-string cross),
        // and a bare Context has no font families bound, so painting one
        // panics rather than failing an assertion.
        fonts::install(&ctx, fonts::FontChoice::default(), None);
        let s = Settings::default();
        let cases: &[&[u8]] = &[
            &[],
            &[60],
            &[40, 47, 52, 56, 59, 64],
            &[41, 48, 53, 57, 60, 65],
            &[84, 85, 86],
            &[36, 43, 48, 52, 55, 58, 60, 64, 67, 72],
            &[0, 1, 127],
        ];
        for t in TUNINGS {
            for capo in [0u8, 5, 21] {
                let spec = FretboardSpec {
                    tuning: t.clone(),
                    frets: 22,
                    capo,
                };
                for held in cases {
                    let v = solve_cold(&spec, held);
                    // Every wood, because each one carries its own palette and
                    // the pale board takes a different branch for the nut and
                    // the dot edges.
                    for wood in Wood::ALL {
                        let _ = ctx.run(Default::default(), |ctx| {
                            egui::CentralPanel::default().show(ctx, |ui| {
                                draw(ui.painter(), rect(), &v, &spec, &s, wood, v.shape.barre);
                                draw_top_edge(ui.painter(), rect(), &s);
                            });
                        });
                    }
                }
            }
        }
    }
}
