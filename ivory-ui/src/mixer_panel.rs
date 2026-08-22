//! The mixer view: every source in the app, drawn as a channel strip.
//!
//! **The band is the rack and this is the routing**, and that division is why
//! nothing in the band had to move to make room. The six effect knobs stay
//! where a hand reaches for them during a take; what lives here is the thing
//! they never had — who feeds them, how much, and what is heard.
//!
//! A pure painter, like every other module in this crate: [`draw`] takes a
//! rect and a [`MixerView`] and paints; [`hit_test`] is its exact inverse, and
//! [`Layout::targets`] is the single source of truth both are derived from. No
//! state, no `egui::Ui`, and nothing here can open a device or read a file.

use crate::recorder::{gain_text, gain_to_fader, Column, Strip, INSERTS, SLOTS};
use egui::{Color32, FontId, Painter, Pos2, Rect, Stroke, Vec2};

/// One channel, as the painter sees it.
///
/// **Levels come in as they are, not as decibels.** `gain` is linear and
/// `peak` is a magnitude, because that is what the audio produced; turning
/// them into a position and a number is this module's job and doing it twice,
/// differently, in two places is how a fader and its readout disagree.
pub struct StripView<'a> {
    /// `None` is the master, which has no send and cannot be muted or soloed.
    pub strip: Option<Strip>,
    /// What the channel is called. Empty on an unfilled slot, which is drawn
    /// as somewhere to put an instrument rather than as a channel with no name.
    pub name: &'a str,
    /// The second line under the name: the device, the file, the patch.
    pub detail: &'a str,
    /// A user effect across this strip, by name. Empty for none.
    /// What is in each of this channel's three insert slots, by short name.
    /// Empty for an empty slot, which is drawn as somewhere to put one.
    pub inserts: [&'a str; INSERTS],
    /// What feeds a general channel, for its icon. `None` on everything else
    /// and on an unused channel.
    pub kind: Option<crate::recorder::ChannelKind>,
    /// Index into [`STRIP_COLORS`]. Zero is the desk's own wood.
    pub color: usize,
    /// An instrument slot with nothing in it.
    ///
    /// **Drawn, not hidden.** Five slots that appear one at a time as they are
    /// filled is a desk that changes shape under your hands; five outlines with
    /// a plus in them is a rack with room in it, and the plus is a second way
    /// into the instrument picker for somebody who is already looking at the
    /// mixer.
    pub empty: bool,
    /// Linear, as the fader sits.
    pub gain: f32,
    /// 0..=1, how much of it reaches the effects bus.
    pub send: f32,
    /// The loudest thing it made since the last frame, left and right.
    pub peak: [f32; 2],
    /// Whether it is a stereo source. A mono one draws ONE bar, because two
    /// identical bars is a lie about the signal.
    pub stereo: bool,
    /// Decibels the limiter took off, for the master alone. Zero for none.
    pub gr_db: f32,
    pub muted: bool,
    pub soloed: bool,
}

impl StripView<'_> {
    /// The master and the effects return take no send, and neither does a slot
    /// with nothing in it to send.
    fn sends(&self) -> bool {
        self.strip.is_some_and(Strip::sends) && !self.empty
    }

    /// Whether it has controls at all. An empty slot has one: the plus.
    fn live(&self) -> bool {
        self.strip.is_some() && !self.empty
    }
}

/// Everything the mixer draws, pushed in.
pub struct MixerView<'a> {
    /// Every channel, then the master last. Fixed, because the desk is fixed:
    /// an app whose strips appear and disappear as things are loaded is a desk
    /// that changes shape under your hands.
    pub strips: [StripView<'a>; COLUMNS],
    /// True when anything at all is soloed, so the strips that are not can be
    /// drawn as what they are — silent, not merely unlit.
    pub any_solo: bool,
    pub dark_mode: bool,
    /// The band's own wood, so the two surfaces are one instrument.
    pub wood: (u8, u8, u8),
    /// The strip whose colour palette is open, if any.
    pub palette_open: Option<Column>,
    /// A decibel figure being typed, and into which strip.
    ///
    /// **A fader is a bad way to ask for exactly -6.0.** Its whole range is
    /// eighty decibels; the drag was slowed until it was usable, and this is
    /// the other half of that — click the number and say it.
    pub typing: Option<(Column, &'a str)>,
    /// The channel whose NAME is being typed, and what is in the field.
    pub naming: Option<(Column, &'a str)>,
}

impl MixerView<'_> {
    /// Whether a strip is heard right now, which is what dims it.
    pub fn heard(&self, at: usize) -> bool {
        let Some(s) = self.strips.get(at) else {
            return true;
        };
        if !s.live() {
            return true;
        }
        if self.any_solo {
            return s.soloed;
        }
        !s.muted
    }
}

/// What a press on the mixer means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The fader of the strip drawn in this column.
    Fader(Column),
    /// The send knob of the strip drawn in this column.
    Send(Column),
    Mute(Column),
    Solo(Column),
    /// **What is on this channel**: the plus on an empty slot, and the icon
    /// above the name on a filled one. Both open the same picker — the
    /// interface for an input, the instrument for a slot, the file for the
    /// backing track — because both ask the same question.
    Add(Column),
    /// One of the three insert chips: put an effect in it, or take one out.
    /// The second field is the BAY, which is not a channel of anything.
    Insert(Column, usize),
    /// A swatch in the open palette: paint this column's strip colour `c`.
    Paint(Column, usize),
    /// A right-click anywhere on a strip: open its palette.
    Palette(Column),
    /// The decibel readout: type a number into it.
    Db(Column),
    /// The channel's name: type a new one.
    ///
    /// **On an INPUT this renames the interface, not the channel.** Every
    /// input of one box shares the half of the label worth shortening, so
    /// typing "x" over "Scarlett 18i20 USB - 3" makes all of them "x - N" at
    /// once — which is the only rename that fits and the only one anybody
    /// wants to type more than once.
    Name(Column),
}

impl Hit {
    /// Which column it is on. Every one of them is on exactly one.
    pub fn column(self) -> Column {
        match self {
            Hit::Fader(i)
            | Hit::Send(i)
            | Hit::Mute(i)
            | Hit::Solo(i)
            | Hit::Add(i)
            | Hit::Insert(i, _)
            | Hit::Palette(i)
            | Hit::Db(i)
            | Hit::Name(i)
            | Hit::Paint(i, _) => i,
        }
    }

    /// Whether it travels under the hand, and along which axis.
    ///
    /// The same contract the band's controls use, and deliberately the same
    /// feel: relative, so a press never makes the handle jump, and the hand
    /// can leave the control and keep going.
    pub fn axis(self) -> Option<DragAxis> {
        match self {
            Hit::Fader(_) => Some(DragAxis::Vertical),
            Hit::Send(_) => Some(DragAxis::Vertical),
            Hit::Mute(_) | Hit::Solo(_) | Hit::Add(_) | Hit::Insert(..) | Hit::Paint(..)
            | Hit::Palette(_) | Hit::Db(_) | Hit::Name(_) => None,
        }
    }

    /// How far the pointer must move to sweep it end to end, in points.
    pub fn travel(self) -> Option<f32> {
        match self {
            Hit::Fader(_) => Some(FADER_TRAVEL),
            Hit::Send(_) => Some(SEND_TRAVEL),
            Hit::Mute(_) | Hit::Solo(_) | Hit::Add(_) | Hit::Insert(..) | Hit::Paint(..)
            | Hit::Palette(_) | Hit::Db(_) | Hit::Name(_) => None,
        }
    }
}

/// What a channel can be painted.
///
/// **A row per hue, three tones across.** Eight colours were enough to tell
/// eight channels apart and not enough to tell them apart the way somebody
/// wants to — two quiet blues for two microphones, a loud red for the one
/// thing that must not be missed. So: nine rows of three.
///
/// The first row is the neutrals, and index 0 is the band's own wood — a
/// channel nobody has coloured is not a colour choice at all. Every value is
/// dark enough to carry the face's text, because this paints the whole strip
/// and not a dot on it; the labels sit on plates of their own for the same
/// reason.
///
/// [`PALETTE_COLUMNS`] is what makes the grid read as hues rather than as
/// twenty-seven swatches: reading down a column is one tone, across a row is
/// one colour.
pub const STRIP_COLORS: [(u8, u8, u8); 27] = [
    (0x00, 0x00, 0x00), // the desk's own wood — replaced at draw time
    (0x36, 0x33, 0x30), // slate
    (0x56, 0x51, 0x4D), // stone
    (0x3F, 0x12, 0x15), (0x63, 0x17, 0x1C), (0x86, 0x27, 0x2D), // red
    (0x3F, 0x27, 0x12), (0x63, 0x39, 0x17), (0x86, 0x52, 0x27), // amber
    (0x3F, 0x3B, 0x12), (0x63, 0x5C, 0x17), (0x86, 0x7D, 0x27), // olive
    (0x21, 0x3F, 0x12), (0x30, 0x63, 0x17), (0x46, 0x86, 0x27), // green
    (0x12, 0x3A, 0x3F), (0x17, 0x5A, 0x63), (0x27, 0x7B, 0x86), // teal
    (0x12, 0x24, 0x3F), (0x17, 0x36, 0x63), (0x27, 0x4D, 0x86), // blue
    (0x29, 0x12, 0x3F), (0x3D, 0x17, 0x63), (0x57, 0x27, 0x86), // violet
    (0x3F, 0x12, 0x33), (0x63, 0x17, 0x4E), (0x86, 0x27, 0x6C), // magenta
];

/// Three, because the table is hues down and tones across.
pub const PALETTE_COLUMNS: usize = 3;

/// The master's colour on a fresh install. Red, because it is the one channel
/// you look for — the top tone of it, which is the one that reads as RED
/// rather than as maroon.
pub const MASTER_COLOR: usize = 5;

/// Where the eight-colour table's indices moved to when it became this one.
///
/// **A palette is a set of indices somebody has already saved.** Growing the
/// table from eight to twenty-seven renumbered every one of them, so a desk
/// with a teal microphone and a violet backing track came back dark red and
/// brown — colours the user never chose, on a build where the old ones were
/// still on screen a minute earlier.
///
/// Each entry is the nearest tone of the same hue. `slate` becomes `stone`,
/// which is the only one that is not an exact match and is the closest the new
/// neutrals get.
pub const PALETTE_V1_TO_V2: [usize; 8] = [0, 5, 8, 14, 16, 20, 23, 2];

/// Which way a control travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAxis {
    Vertical,
}

/// How far the hand moves to sweep a fader end to end, in points.
///
/// **Long, and longer than it looks.** The first pair were a fader's own
/// height, on the reasoning that a fader should feel one-to-one under the
/// pointer — but the range is eighty decibels and the track is a hundred
/// points, so every point was most of a decibel and nobody could land on a
/// number. A sweep wider than the screen is not a problem: the drag is
/// RELATIVE, so the hand leaves the fader and goes on pulling.
const FADER_TRAVEL: f32 = 420.0;

/// The same, for a send. A percentage is read to the nearest few, so it wants
/// the same room a fader does.
const SEND_TRAVEL: f32 = 420.0;

/// Where everything on one strip is, and where the strips are.
///
/// Fractions of the rect rather than points, so the same layout holds at every
/// window size the app offers — and so the hit test and the painter cannot
/// drift, because they read the same struct.
/// The drawn channels, plus the master.
///
/// Every instrument slot, every input of the interface, the backing track and
/// the master. The click and the effects return keep their place on the desk
/// and are not drawn — their controls are the band's.
// Eight general channels, the backing track, the click, and the master.
pub const COLUMNS: usize = crate::recorder::CHANNELS + 3;

pub struct Layout {
    pub strips: [StripStrip; COLUMNS],
}

/// One strip's rectangles.
#[derive(Debug, Clone, Copy)]
pub struct StripStrip {
    pub panel: Rect,
    pub icon: Rect,
    pub name: Rect,
    pub detail: Rect,
    pub meter: Rect,
    pub send: Rect,
    pub fader: Rect,
    pub db: Rect,
    pub mute: Rect,
    pub solo: Rect,
    /// A user effect plugin across this strip. Only the bus has one today.
    pub inserts: [Rect; INSERTS],
    /// The plus on an empty slot.
    pub add: Rect,
}

impl StripStrip {
    /// Every rectangle absent, for a strip that is not there or a rect too
    /// small to hold one. `Rect::NOTHING` rather than a derive, because
    /// `Rect` has no `Default` and a zero-sized rect at the origin would be
    /// INSIDE small windows — a target you can press by accident.
    const NONE: Self = Self {
        panel: Rect::NOTHING,
        icon: Rect::NOTHING,
        name: Rect::NOTHING,
        detail: Rect::NOTHING,
        meter: Rect::NOTHING,
        send: Rect::NOTHING,
        fader: Rect::NOTHING,
        db: Rect::NOTHING,
        mute: Rect::NOTHING,
        solo: Rect::NOTHING,
        inserts: [Rect::NOTHING; INSERTS],
        add: Rect::NOTHING,
    };
}

/// Space around the rack, and between the strips.
const PAD: f32 = 10.0;
const GAP: f32 = 6.0;
/// The master is the same width as every other channel.
///
/// **It used to be wider, "because it is the one you look at".** That bought
/// the master a legible scale and left every other channel with a smear where
/// its numbers should be — and a desk where one strip is a different size is a
/// desk with two kinds of strip on it. The width it gave back is spread across
/// all eleven, which is what makes the ticks readable everywhere.
const MASTER_EXTRA: f32 = 0.0;

impl Layout {
    pub fn new(rect: Rect, view: &MixerView<'_>) -> Self {
        let inner = Rect::from_min_max(
            Pos2::new(rect.min.x + PAD, rect.min.y + PAD),
            Pos2::new(rect.max.x - PAD, rect.max.y - PAD),
        );
        if !inner.is_positive() {
            return Self {
                strips: [StripStrip::NONE; COLUMNS],
            };
        }
        // **Every channel one unit wide, and the count never changes.** Five
        // instrument slots, the interface's inputs, the backing track and the
        // master — a fixed desk, drawn the same whether or not anything is in
        // it. A rack that grows a column when something is plugged in is a
        // rack whose controls move under a hand.
        //
        // The master is a little wider, so it reads as the end of the row
        // rather than as another channel.
        let gaps = (COLUMNS - 1) as f32;
        let units = gaps + 1.0 + MASTER_EXTRA;
        let unit = (inner.width() - GAP * gaps) / units;
        let mut x = inner.left();
        let strips = std::array::from_fn(|i| {
            let Some(v) = view.strips.get(i) else {
                return StripStrip::NONE;
            };
            let w = if i == COLUMNS - 1 {
                unit * (1.0 + MASTER_EXTRA)
            } else {
                unit
            };
            let panel = Rect::from_min_size(Pos2::new(x, inner.top()), Vec2::new(w, inner.height()));
            x += w + GAP;
            Self::one(panel, Some(v))
        });
        Self { strips }
    }

    /// The rows inside one strip, top to bottom.
    ///
    /// **Read as a sentence**: what it is, how loud it is arriving, how much
    /// goes to the effects, how loud it leaves, and whether it is heard at all.
    /// A strip with no send gives that row's height to its meter rather than
    /// leaving a hole, because a hole reads as a control that failed to draw.
    fn one(panel: Rect, view: Option<&StripView<'_>>) -> StripStrip {
        let Some(view) = view else {
            return StripStrip::NONE;
        };
        // **An empty slot is a panel and a plus, and nothing else.** Drawing a
        // dead fader on it would be five controls that do nothing, which is
        // the thing the plugin rack was already doing wrong.
        if view.empty {
            return StripStrip {
                panel,
                // **The whole column, not the plus in the middle of it.** The
                // plus is a picture of what pressing here does; a hundred
                // points of empty channel that ignores a press is a column
                // that looks dead until you find the one spot that is not.
                // Nothing else is on an empty strip, so nothing can collide.
                add: panel,
                ..StripStrip::NONE
            };
        }
        let sends = view.sends();
        let switchable = view.live();
        let cut = |a: f32, b: f32| {
            Rect::from_min_max(
                Pos2::new(panel.left() + 4.0, panel.top() + panel.height() * a),
                Pos2::new(panel.right() - 4.0, panel.top() + panel.height() * b),
            )
        };
        // **The meter stands BESIDE the fader, not above it.** That is what a
        // channel strip looks like on every desk ever built, and it is not
        // decoration: the two answer one question between them — how much is
        // arriving and how much you are letting through — and reading them
        // means looking at one place. Stacked, the meter also left a hand's
        // width of empty face above the fader that nothing could use.
        // BELOW the insert row, which every channel has now. The two shared a
        // band for one build and the guard test found it before anybody could
        // press a send and get an effect picker.
        let send = if sends { cut(0.41, 0.53) } else { Rect::NOTHING };
        // Short of the bottom edge: a switch flush against the frame reads as
        // something that has been cut off rather than as the end of the strip.
        // **Three inserts, on every channel, and the number never changes.**
        //
        // There used to be ONE, on the effects bus alone, on the argument that
        // a reverb is a send effect and per-channel inserts would be nine
        // copies of the same plugin. That argument is still true of reverb and
        // was never true of an amp sim on the input, a compressor on the piano
        // or an EQ on the backing track.
        //
        // Three because a desk you have to remember the rules of is not a desk
        // you can see, and because this started as a MIDI monitor: a rack that
        // grows until the machine gives out is a different program. Thirty-six
        // across the desk if every one is filled, which is deliberately
        // reachable — anybody who fills thirty-six finds out what their
        // hardware does, and that is worth learning from the app rather than
        // from a number the app made up.
        //
        // **Stacked, not shoulder to shoulder.** Three chips side by side in a
        // column ninety points wide are thirty points each, and thirty points
        // holds about three characters of "FabFilter Pro-R 2" — a rack you
        // have to click to find out what is in it. Down the strip each one is
        // as wide as the channel, which is where the reading room comes from;
        // the height it costs comes out of the fader's travel, which had the
        // most of anything on the strip.
        let rack = cut(0.195, 0.395);
        let inserts: [Rect; INSERTS] = std::array::from_fn(|i| {
            let h = rack.height() / INSERTS as f32;
            let y = rack.top() + h * i as f32;
            Rect::from_min_max(
                Pos2::new(rack.left() + 2.0, y + 1.0),
                Pos2::new(rack.right() - 2.0, y + h - 1.0),
            )
        });
        let switches = if switchable { cut(0.902, 0.975) } else { Rect::NOTHING };
        let (mute, solo) = if switchable {
            let mid = switches.center().x;
            (
                Rect::from_min_max(switches.min, Pos2::new(mid - 3.0, switches.max.y)),
                Rect::from_min_max(Pos2::new(mid + 3.0, switches.min.y), switches.max),
            )
        } else {
            (Rect::NOTHING, Rect::NOTHING)
        };
        // A strip with no send starts its fader where the send would have been,
        // rather than leaving a hole: a hole reads as a control that failed to
        // draw.
        // A strip with no send starts its fader where the send would have been,
        // rather than leaving a hole — unless something else took that room.
        // The effects bus has no send and DOES have an insert, and the two
        // wanted the same band; the guard test found it before anybody could
        // press one and get the other.
        // Every channel has an insert row now, so every channel's travel
        // starts below it — a send is what still moves the line.
        let travel = cut(if sends { 0.55 } else { 0.42 }, 0.82);
        // The meter takes the left third and the fader the rest, so the fader
        // still sits near the middle of the strip where a hand expects it.
        let split = travel.left() + travel.width() * 0.42;
        // **Every source channel carries one, and it is the way in.** The
        // microphone and the waveform were labels: they said what the column
        // was, which the name underneath already said. An instrument had none
        // at all, so the desk's three kinds of source read as one kind of
        // column with different words on it.
        //
        // They are now the picker — press the microphone to choose the
        // interface, the keyboard to choose the instrument, the waveform to
        // choose the file — which is why the instrument needed a glyph of its
        // own. The bus, the click and the master have no source to choose and
        // keep the room for their names.
        // Every source channel carries one — the eight generals (whose glyph
        // is their KIND, and clicking it cycles the kind), the backing track,
        // and now the click. The bus and the master have nothing to pick and
        // keep the room for their names.
        let has_icon = matches!(
            view.strip,
            Some(Strip::Channel(_) | Strip::Track | Strip::Click)
        );
        // **Under the name, not over it.** The master has no source to choose
        // and so no icon, and with the icon on top every other channel's name
        // sat a band lower than the master's — twelve labels at one height and
        // one at another, which reads as the master being a different kind of
        // object rather than the same strip with nothing to pick.
        let icon = if has_icon {
            let r = cut(0.088, 0.145);
            Rect::from_center_size(r.center(), Vec2::splat(r.height().min(r.width())))
        } else {
            Rect::NOTHING
        };
        StripStrip {
            panel,
            icon,
            // **Loosely fitting the text, not a band of face.** These were a
            // fifteenth of the strip each, which at any usable window size is
            // three times the height of what is written in them: the plate
            // behind them then reads as a panel with a word lost in it.
            // The same band on every channel now, master included.
            name: cut(0.022, 0.078),
            detail: if has_icon { cut(0.152, 0.19) } else { cut(0.086, 0.128) },
            meter: Rect::from_min_max(travel.min, Pos2::new(split, travel.max.y)),
            send,
            fader: Rect::from_min_max(Pos2::new(split, travel.min.y), travel.max),
            db: cut(0.83, 0.888),
            mute,
            solo,
            inserts,
            // **The same target as the plus on an empty one**, because it is
            // the same question: what is on this channel. An empty slot draws
            // a plus and a filled one draws its icon, and both open the picker
            // that answers it.
            add: icon,
        }
    }

    /// Every clickable region and what it means.
    ///
    /// **The one source of truth.** `hit_test` reads it and the painter draws
    /// from the same rects, so a control that moved cannot become a control
    /// you can see and not press.
    pub fn targets(&self) -> Vec<(Rect, Hit)> {
        let mut out = Vec::with_capacity(6 * 4);
        for (i, s) in self.strips.iter().enumerate() {
            let i = Column(i);
            for (r, hit) in [
                (s.fader, Hit::Fader(i)),
                (s.send, Hit::Send(i)),
                (s.mute, Hit::Mute(i)),
                (s.solo, Hit::Solo(i)),
                (s.add, Hit::Add(i)),
                (s.inserts[0], Hit::Insert(i, 0)),
                (s.inserts[1], Hit::Insert(i, 1)),
                (s.inserts[2], Hit::Insert(i, 2)),
                (s.db, Hit::Db(i)),
                (s.name, Hit::Name(i)),
            ] {
                if r.is_positive() {
                    out.push((r, hit));
                }
            }
        }
        out
    }
}

/// What a press at `pos` means, or nothing.
pub fn hit_test(rect: Rect, view: &MixerView<'_>, pos: Pos2) -> Option<Hit> {
    let l = Layout::new(rect, view);
    // **The palette first, and it swallows the strip under it.** A swatch over
    // a fader that also moved the fader would be a colour you cannot pick
    // without changing a level.
    if let Some(at) = view.palette_open {
        if let Some(s) = l.strips.get(at.0) {
            for (c, r) in palette_over(s).into_iter().enumerate() {
                if r.contains(pos) {
                    return Some(Hit::Paint(at, c));
                }
            }
            if s.panel.contains(pos) {
                // Anywhere else on the strip closes it without painting.
                return Some(Hit::Paint(at, usize::MAX));
            }
        }
    }
    l.targets()
        .into_iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| h)
}

/// Which column a point is on, for a right-click. `None` off the rack.
pub fn strip_at(rect: Rect, view: &MixerView<'_>, pos: Pos2) -> Option<Column> {
    Layout::new(rect, view)
        .strips
        .iter()
        .position(|s| s.panel.contains(pos))
        .map(Column)
}

/// How far a control travels, for the caller that is dragging it.
pub fn drag_travel(hit: Hit) -> Option<f32> {
    hit.travel()
}

// ── painting ───────────────────────────────────────────────────────────────

struct Ink {
    wood: Color32,
    face: Color32,
    engrave: Color32,
    faint: Color32,
    track: Color32,
    /// The fader's slot: a dark channel cut into the face, not a hairline.
    slot: Color32,
    /// The fader's cap: bone, the colour of the real thing.
    cap: Color32,
    /// The ridges across it, a shade under the cap so they read as cuts.
    cap_rib: Color32,
    lit: Color32,
    mute: Color32,
    solo: Color32,
    meter_lo: Color32,
    gr: Color32,
    meter_hi: Color32,
}

fn ink(view: &MixerView<'_>) -> Ink {
    let (r, g, b) = view.wood;
    let wood = Color32::from_rgb(r, g, b);
    // The strip faces are the band's own bone, lifted a little so a rack of
    // them does not read as one slab.
    let face = if view.dark_mode {
        Color32::from_rgb(0xC9, 0xC0, 0xA6)
    } else {
        Color32::from_rgb(0xE3, 0xD9, 0xBD)
    };
    Ink {
        wood,
        face,
        engrave: Color32::from_rgb(0x24, 0x20, 0x1A),
        faint: Color32::from_rgb(0x24, 0x20, 0x1A).gamma_multiply(0.55),
        // Dark enough to read as a well rather than as a lighter panel: at
        // rest a meter should look like somewhere a level would go.
        track: Color32::from_rgb(0x1A, 0x17, 0x12),
        // **Ivory on black, from the owner's photograph of the real desk** —
        // and the SAME ivory and the same black the band's faders run in, out
        // of the one place they are written down. Two views of one desk that
        // disagreed about what a fader is made of would be two desks.
        slot: crate::recorder_panel::CAP_SLOT,
        cap: crate::recorder_panel::CAP_BONE,
        cap_rib: crate::recorder_panel::CAP_RIB,
        lit: Color32::from_rgb(0x1C, 0x6F, 0xD6),
        mute: Color32::from_rgb(0xE0, 0xA8, 0x22),
        solo: Color32::from_rgb(0xE8, 0x3A, 0x4E),
        meter_lo: Color32::from_rgb(0x3D, 0xC0, 0x5A),
        gr: Color32::from_rgb(0xE8, 0x8C, 0x2A),
        meter_hi: Color32::from_rgb(0xE0, 0xA8, 0x22),
    }
}

/// Paint the whole rack into `rect`.
pub fn draw(painter: &Painter, rect: Rect, view: &MixerView<'_>) {
    let l = Layout::new(rect, view);
    let p = ink(view);
    painter.rect_filled(rect, 0.0, p.wood);

    for (i, s) in l.strips.iter().enumerate() {
        let Some(v) = view.strips.get(i) else { continue };
        if !s.panel.is_positive() {
            continue;
        }
        let heard = view.heard(i);
        let typed = view.typing.filter(|(at, _)| at.0 == i).map(|(_, b)| b);
        let naming = view.naming.filter(|(at, _)| at.0 == i).map(|(_, b)| b);
        strip(painter, s, v, &p, heard, typed, naming);
    }
    // The palette last, over whatever it belongs to.
    if let Some(at) = view.palette_open {
        if let Some(s) = l.strips.get(at.0) {
            painter.rect_filled(s.panel, 3.0, Color32::from_black_alpha(180));
            for (c, r) in palette_over(s).into_iter().enumerate() {
                let (cr, cg, cb) = STRIP_COLORS[c];
                let fill = if c == 0 {
                    p.wood
                } else {
                    Color32::from_rgb(cr, cg, cb)
                };
                painter.rect_filled(r, 2.0, fill);
                painter.rect_stroke(
                    r,
                    2.0,
                    Stroke::new(1.0, p.face.gamma_multiply(0.5)),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}

fn strip(
    painter: &Painter,
    l: &StripStrip,
    v: &StripView<'_>,
    p: &Ink,
    heard: bool,
    typed: Option<&str>,
    naming: Option<&str>,
) {
    if v.empty {
        empty_slot(painter, l, p);
        return;
    }
    // **A painted channel keeps its own face for the labels.** The whole panel
    // takes the colour, and the name and what it is carrying sit on a plate of
    // the ordinary face — so a channel can be any colour at all and its label
    // is still dark text on bone, which is the one thing that must not depend
    // on a choice somebody made for fun.
    let painted = v.color != 0;
    let face = if painted {
        let (r, g, b) = STRIP_COLORS[v.color.min(STRIP_COLORS.len() - 1)];
        Color32::from_rgb(r, g, b)
    } else if heard {
        p.face
    } else {
        // **Not merely unlit: silent.** A strip that solo has taken out of the
        // mix is not producing anything, and drawing it exactly like one that
        // is would make solo a light rather than a routing decision.
        p.face.gamma_multiply(0.62)
    };
    painter.rect_filled(l.panel, 3.0, face);
    painter.rect_stroke(
        l.panel,
        3.0,
        Stroke::new(1.0, p.engrave.gamma_multiply(0.30)),
        egui::StrokeKind::Inside,
    );

    let cap = |size: f32| FontId::new(size, crate::fonts::courier_bold());
    let plain = |size: f32| FontId::new(size, crate::fonts::courier());
    let h = l.panel.height();
    // **Everything printed ON the panel, in ink the panel can carry.**
    //
    // The name sits on a plate of the ordinary face and is always dark on
    // bone. Nothing else does: the icon, the second line, the decibel readout
    // and the fader's scale are all painted straight onto the channel — so on
    // a channel somebody painted dark red they were dark on dark, which is
    // where the keyboard icon and "+0.0 dB" went when the desk got colours.
    // Chosen from the face as composited, for the reason `flatten` gives.
    let solid = flatten(face, p.wood);
    let ink = print_on(solid, p);

    // The plate the label sits on, so it reads on any colour.
    // **The name and nothing else.** It used to run down to the second line
    // when there was one, which was a plate sized for two lines on a channel
    // with one — and now the icon sits between them, so a plate that reached
    // the second line would paint over the icon as well.
    let plate = Rect::from_min_max(
        Pos2::new(l.panel.left() + 3.0, l.name.top() - 2.0),
        Pos2::new(l.panel.right() - 3.0, l.name.bottom() + 2.0),
    );
    // **Under every name, painted or not.** It was drawn only on a coloured
    // channel, where it exists to keep the label legible on any background —
    // but a name is a FIELD now, and a field needs an edge to look like one
    // before it is clicked. The two reasons want the same rectangle.
    let _ = painted;
    if plate.is_positive() {
        painter.rect_filled(plate, 2.0, p.face);
    }
    // **The band's own icons, under the names.** The microphone and the
    // waveform are how the input and the backing track are already labelled
    // one band up; drawing them again here is what makes the three kinds of
    // source read as three kinds of thing rather than as three rows of text.
    // Under, so that every label on the desk — the master's included — sits at
    // one height.
    if let Some(buf) = naming {
        painter.rect_filled(l.name, 2.0, p.face);
        painter.rect_stroke(
            l.name,
            2.0,
            Stroke::new(1.0, p.lit),
            egui::StrokeKind::Inside,
        );
        centred(
            painter,
            l.name,
            &format!("{buf}_"),
            cap((h * 0.032).clamp(7.5, 12.0)),
            p.engrave,
        );
    }
    let named = match (v.strip, v.kind) {
        // A general channel's icon IS its kind, which is why clicking it
        // cycles: the keyboard says MIDI, the microphone says AUDIO.
        (Some(Strip::Channel(_)), Some(crate::recorder::ChannelKind::Midi)) => {
            crate::recorder_panel::draw_instrument_icon(painter, l.icon, ink);
            true
        }
        (Some(Strip::Channel(_)), Some(crate::recorder::ChannelKind::Audio)) => {
            crate::recorder_panel::draw_microphone(painter, l.icon, ink);
            true
        }
        (Some(Strip::Track), _) => {
            crate::recorder_panel::draw_waveform_icon(painter, l.icon, ink);
            true
        }
        (Some(Strip::Click), _) => {
            crate::recorder_panel::draw_metronome_on(painter, l.icon, ink, face);
            true
        }
        _ => false,
    };
    let _ = named;
    if naming.is_none() {
        centred(painter, l.name, v.name, cap((h * 0.032).clamp(7.5, 12.0)), p.engrave);
    }
    if !v.detail.is_empty() {
        centred(
            painter,
            l.detail,
            v.detail,
            plain((h * 0.024).clamp(7.0, 11.0)),
            ink.gamma_multiply(0.72),
        );
    }
    // A dimmed channel is dimmed by a veil rather than by a paler face, so
    // painting one does not cost it its "not heard" state.
    if !heard {
        painter.rect_filled(l.panel, 3.0, Color32::from_black_alpha(90));
    }

    // The master gets the full scale; a channel gets the four marks its width
    // has room for. `strip: None` IS the master — see `StripView`.
    let ticks: &[f32] = if v.strip.is_none() {
        &MASTER_TICKS
    } else {
        &METER_TICKS
    };
    // The face as it will LOOK, not as it is written down: a silent strip's is
    // translucent, and the scale has to be chosen against what is on screen.
    meter(
        painter,
        l.meter,
        v.peak,
        v.stereo,
        v.gr_db,
        heard,
        ticks,
        solid,
        p,
    );

    if l.send.is_positive() {
        send_knob(painter, l.send, v.send, p);
    }
    // **A rack: an ivory field with three strips down it.**
    //
    // On the ordinary face rather than in the channel's own colour, for the
    // reason the name plate is: a plugin's name is text to be read, and text
    // to be read is dark on bone whatever somebody painted the channel. The
    // field is drawn once behind all three so the rack reads as one object
    // with three bays in it, which is what a rack is.
    let bays: Vec<Rect> = l.inserts.iter().copied().filter(|r| r.is_positive()).collect();
    if let (Some(first), Some(last)) = (bays.first(), bays.last()) {
        let field = Rect::from_min_max(
            Pos2::new(first.left() - 2.0, first.top() - 1.0),
            Pos2::new(last.right() + 2.0, last.bottom() + 1.0),
        );
        painter.rect_filled(field, 2.0, p.face);
        painter.rect_stroke(
            field,
            2.0,
            Stroke::new(1.0, p.engrave.gamma_multiply(0.30)),
            egui::StrokeKind::Inside,
        );
    }
    for (n, cell) in l.inserts.iter().enumerate() {
        if !cell.is_positive() {
            continue;
        }
        let name = v.inserts.get(n).copied().unwrap_or("");
        let filled = !name.is_empty();
        // The hairline between bays, so three empty ones are three bays and
        // not one tall blank. Not under the last, which would be a line along
        // the bottom edge of the field.
        if n + 1 < INSERTS {
            painter.line_segment(
                [
                    Pos2::new(cell.left() + 1.0, cell.bottom() + 1.0),
                    Pos2::new(cell.right() - 1.0, cell.bottom() + 1.0),
                ],
                Stroke::new(1.0, p.engrave.gamma_multiply(0.18)),
            );
        }
        let font = FontId::new(
            (cell.height() * 0.46).clamp(6.5, 10.0),
            crate::fonts::courier_bold(),
        );
        if filled {
            // The tab at the left edge is the "loaded" light the whole chip
            // used to be. Blue on bone says the same thing and leaves the
            // width for the name, which is the point of stacking them.
            painter.rect_filled(
                Rect::from_min_max(
                    cell.min,
                    Pos2::new(cell.left() + 3.0, cell.bottom()),
                ),
                1.0,
                p.lit,
            );
            let text = Rect::from_min_max(Pos2::new(cell.left() + 6.0, cell.top()), cell.max);
            centred(
                painter,
                text,
                &shorten_plugin(painter, text.width(), name, &font),
                font.clone(),
                p.engrave,
            );
        } else {
            // Big enough to be an offer. At the chip's own text size it was a
            // speck in the middle of an empty bay, which reads as dirt on the
            // panel rather than as somewhere to put a plugin.
            let plus = FontId::new(
                (cell.height() * 0.62).clamp(9.0, 15.0),
                crate::fonts::courier_bold(),
            );
            centred(painter, *cell, "+", plus, p.engrave.gamma_multiply(0.45));
        }
    }

    fader(painter, l.fader, v.gain, solid, p);
    let db_font = plain((h * 0.026).clamp(8.0, 12.0));
    match typed {
        Some(buf) => {
            painter.rect_filled(l.db, 2.0, p.face);
            painter.rect_stroke(
                l.db,
                2.0,
                Stroke::new(1.0, p.lit),
                egui::StrokeKind::Inside,
            );
            centred(painter, l.db, &format!("{buf}_"), db_font, p.engrave);
        }
        None => centred(painter, l.db, &gain_text(v.gain), db_font, ink),
    }

    if l.mute.is_positive() {
        switch(painter, l.mute, "M", v.muted, p.mute, p);
        switch(painter, l.solo, "S", v.soloed, p.solo, p);
    }
}

/// The swatches, over the strip whose colour is being chosen.
///
/// Drawn last and hit-tested first, like every other transient thing in this
/// app: while it is up it is the only thing on that strip that can be pressed.
fn palette_over(l: &StripStrip) -> Vec<Rect> {
    if !l.panel.is_positive() {
        return Vec::new();
    }
    let n = STRIP_COLORS.len();
    let cols = PALETTE_COLUMNS;
    let rows = n.div_ceil(cols);
    // **Square, and inside the strip.** It could be wider than the channel it
    // belongs to and the swatches would be bigger — but then a press in the
    // overhang would fall through to the NEIGHBOUR's fader, and a colour
    // picker that sometimes moves the channel beside it is worse than a small
    // one. Whichever of the two axes runs out first decides the size.
    let side = ((l.panel.width() - 8.0) / cols as f32)
        .min((l.panel.height() - 8.0) / rows as f32)
        .max(1.0);
    let w = side * cols as f32;
    let h = side * rows as f32;
    let left = l.panel.center().x - w * 0.5;
    let top = l.panel.center().y - h * 0.5;
    (0..n)
        .map(|i| {
            Rect::from_min_size(
                Pos2::new(
                    left + (i % cols) as f32 * side,
                    top + (i / cols) as f32 * side,
                ),
                Vec2::splat(side),
            )
            .shrink(1.5)
        })
        .collect()
}

/// Somewhere to put an instrument: an outline and a plus.
///
/// **An outline rather than a filled panel**, because it is not a channel yet
/// and drawing it like one would be five faders that do nothing. The rack on
/// the right of the band says "empty (click to load)"; this says the same
/// thing in the shape a mixer says it in.
fn empty_slot(painter: &Painter, l: &StripStrip, p: &Ink) {
    painter.rect_stroke(
        l.panel,
        3.0,
        Stroke::new(1.0, p.face.gamma_multiply(0.34)),
        egui::StrokeKind::Inside,
    );
    let r = l.add;
    if !r.is_positive() {
        return;
    }
    // **A plus the size of a plus, on a target the size of the channel.** The
    // whole panel is pressable now — see `Layout::one` — but drawing the mark
    // at the size of the target gives a cross from corner to corner, which
    // reads as a channel that has been struck out rather than one waiting to
    // be filled.
    let arm = r.width().min(r.height()) * 0.17;
    let c = r.center();
    let ink = p.face.gamma_multiply(0.55);
    let w = (arm * 0.16).clamp(1.5, 4.0);
    painter.line_segment(
        [Pos2::new(c.x - arm, c.y), Pos2::new(c.x + arm, c.y)],
        Stroke::new(w, ink),
    );
    painter.line_segment(
        [Pos2::new(c.x, c.y - arm), Pos2::new(c.x, c.y + arm)],
        Stroke::new(w, ink),
    );
}

/// A vertical bar, filling from the bottom.
///
/// **Narrow, and centred in the room it was given.** A meter drawn the full
/// width of the strip is not a meter, it is a panel with a colour in it: at
/// rest it is the largest thing on the channel and it reads as something that
/// failed to load rather than as a level of nothing.
/// The marks down the side of a CHANNEL's meter, in decibels.
///
/// **Four, because a strip is a fifth of the width the master has.** More
/// labels in a column that narrow is a grey smear, and these are the ones
/// anybody actually reads a level against.
const METER_TICKS: [f32; 4] = [0.0, -12.0, -24.0, -48.0];

/// And the MASTER's, which has the room for a scale you can read a number off.
///
/// The standard set, thinned where the fader's own curve crowds it: the marks
/// near unity are worth having one every three decibels and the ones at the
/// bottom are not.
///
/// **Neither END of the travel gets one.** +12 and -60 are where the fader
/// stops, so a label there sits half off the meter and reads against nothing.
/// `every_meter_tick_lands_where_the_fader_would_put_it` caught +12 the first
/// time this was written.
const MASTER_TICKS: [f32; 9] = [6.0, 0.0, -3.0, -6.0, -12.0, -18.0, -24.0, -36.0, -48.0];

fn meter(
    painter: &Painter,
    r: Rect,
    peak: [f32; 2],
    stereo: bool,
    gr_db: f32,
    heard: bool,
    ticks: &[f32],
    // **The strip's own face**, because the scale is printed on it. The
    // numbers used to be a faded ivory, which is legible on a channel somebody
    // painted and completely invisible on one they did not: ivory on ivory.
    // Every unpainted channel on the desk had a meter with no scale beside it,
    // and the two kinds of strip looked like two different controls.
    face: Color32,
    p: &Ink,
) {
    if !r.is_positive() {
        return;
    }
    // **Tall and thin, with the ticks to its left.** A meter you read a number
    // off wants a scale beside it, and the scale is what decides how much of
    // the cell the bars can have.
    let scale_w = (r.width() * 0.5).clamp(14.0, 30.0);
    let bars = Rect::from_min_max(Pos2::new(r.left() + scale_w, r.top()), r.max);
    // **Legible or absent.** The floor used to be 5.5 points, which is not a
    // number anybody reads, it is a grey mark that means "there was a number
    // here". Sized from the column the numbers actually sit in.
    let font = FontId::new((scale_w * 0.46).clamp(6.5, 9.0), crate::fonts::courier());
    for &db in ticks {
        let t = gain_to_fader(10f32.powf(db / 20.0)).clamp(0.0, 1.0);
        let y = bars.bottom() - bars.height() * t;
        painter.text(
            Pos2::new(r.left() + scale_w - 3.0, y),
            egui::Align2::RIGHT_CENTER,
            format!("{db:.0}"),
            font.clone(),
            print_on(face, p).gamma_multiply(0.62),
        );
        painter.line_segment(
            [Pos2::new(bars.left(), y), Pos2::new(bars.right(), y)],
            Stroke::new(0.5, p.face.gamma_multiply(0.18)),
        );
    }

    // One bar or two, and that is a claim about the SOURCE rather than about
    // the samples: a mono microphone drawn as two identical bars says it is
    // stereo, which is the one thing a meter must not do.
    let lanes = if stereo { 2 } else { 1 };
    let gap = 2.0;
    let w = ((bars.width() - gap * (lanes as f32 - 1.0)) / lanes as f32).max(2.0);
    for lane in 0..lanes {
        let x = bars.left() + lane as f32 * (w + gap);
        let cell = Rect::from_min_max(
            Pos2::new(x, bars.top()),
            Pos2::new(x + w, bars.bottom()),
        );
        painter.rect_filled(cell, 2.0, p.track);
        // **Gain reduction hangs from the top of EVERY lane**, the way it does
        // on the band's own master, because that is the direction it means:
        // the ceiling coming down. One strip down the right-hand edge of the
        // pair was the first version and it read as the right channel being
        // limited on its own — the limiter is stereo-linked and takes the same
        // decibels off both sides, so showing it on one is a picture of
        // something that does not happen.
        //
        // Before the bar and outside the `heard` test: a limiter working on a
        // channel you have muted is still worth seeing, and a limiter working
        // on silence is not possible.
        if gr_db > 0.01 {
            let t = (gr_db / 24.0).clamp(0.0, 1.0);
            let w = (cell.width() * 0.3).clamp(2.0, 5.0);
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(cell.right() - w, cell.top()),
                    Pos2::new(cell.right(), cell.top() + cell.height() * t),
                ),
                1.0,
                p.gr,
            );
        }
        let v = if stereo { peak[lane.min(1)] } else { peak[0].max(peak[1]) };
        if !heard || v <= 0.0 {
            continue;
        }
        // The same curve the faders use, so a strip at unity reads at the same
        // height on both — a meter on a different scale from the fader beside
        // it is two rulers on one wall.
        let t = gain_to_fader(v).clamp(0.0, 1.0);
        let top = cell.bottom() - cell.height() * t;
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(cell.left(), top), cell.max),
            2.0,
            if v >= 0.9 { p.meter_hi } else { p.meter_lo },
        );
    }

}

/// A knob, drawn as an arc rather than a dial: it is a percentage, and an arc
/// says how much where a pointer says where.
fn send_knob(painter: &Painter, r: Rect, amount: f32, p: &Ink) {
    // **Smaller than the cell it sits in.** The first version filled the row
    // and read as the biggest thing on the channel, which a send that is
    // usually at zero has no business being.
    let d = r.height().min(r.width() * 0.72) * 0.78;
    if d <= 2.0 {
        return;
    }
    let c = Pos2::new(r.center().x, r.center().y);
    painter.circle_filled(c, d * 0.5, p.track);
    let t = amount.clamp(0.0, 1.0);
    // A quarter-turn short of each stop, like a real pot.
    let sweep = std::f32::consts::PI * 1.5;
    let start = std::f32::consts::PI * 0.75;
    let angle = start + sweep * t;
    let tip = Pos2::new(c.x + angle.cos() * d * 0.42, c.y + angle.sin() * d * 0.42);
    painter.circle_filled(c, d * 0.42, p.cap);
    painter.line_segment(
        [c, tip],
        Stroke::new((d * 0.09).max(1.0), if t > 0.001 { p.lit } else { p.faint }),
    );
}

/// A vertical fader: a track, and a cap on it.
/// A fader: an ivory cap, ribbed, riding in a black slot.
///
/// **Modelled on the real thing** — the owner's photograph of a period desk,
/// where the slot is a dark channel cut into the face and the cap is a bone
/// block with fine ridges across it for a thumb to find. What was here was a
/// pale line with a pale block on it, which reads as a diagram of a fader
/// rather than as one.
///
/// The ribs are drawn only when there is room for them to be ribs; below that
/// they are a smear and the cap is better plain.
fn fader(painter: &Painter, r: Rect, gain: f32, face: Color32, p: &Ink) {
    if !r.is_positive() {
        return;
    }
    let x = r.center().x;
    // The slot: dark, and wide enough to read as a cut rather than a hairline.
    let slot_w = (r.width() * 0.16).clamp(3.0, 7.0);
    let slot = Rect::from_min_max(
        Pos2::new(x - slot_w * 0.5, r.top()),
        Pos2::new(x + slot_w * 0.5, r.bottom()),
    );
    painter.rect_filled(slot, slot_w * 0.5, p.slot);
    painter.rect_stroke(
        slot,
        slot_w * 0.5,
        Stroke::new(1.0, p.engrave.gamma_multiply(0.25)),
        egui::StrokeKind::Inside,
    );

    // **The scale, printed on the panel either side of the slot.** The band's
    // faders have had one since they were drawn; the desk's had a cap in a
    // channel and no way to see where along it you were. Eleven marks, the
    // fifths longer, in the panel's own ink faded well back — a scale is
    // something you read when you look for it, not something that competes
    // with the cap.
    //
    // Nothing at all where the arms would be under three points wide: eleven
    // marks that short are a grey smudge, which is the band's rule too.
    let arm = (r.width() - slot_w) * 0.5 - 2.0;
    if arm >= 3.0 && r.height() >= 60.0 {
        let ink = print_on(face, p).gamma_multiply(0.30);
        for i in 0_u8..=10 {
            let y = r.bottom() - r.height() * f32::from(i) / 10.0;
            // A hair inside, so the first and last are marks rather than the
            // ends of the travel.
            let y = y.clamp(r.top() + 0.5, r.bottom() - 0.5);
            let len = if i % 5 == 0 { arm * 0.85 } else { arm * 0.45 };
            painter.line_segment(
                [
                    Pos2::new(slot.left() - 2.0 - len, y),
                    Pos2::new(slot.left() - 2.0, y),
                ],
                Stroke::new(1.0, ink),
            );
            painter.line_segment(
                [
                    Pos2::new(slot.right() + 2.0, y),
                    Pos2::new(slot.right() + 2.0 + len, y),
                ],
                Stroke::new(1.0, ink),
            );
        }
        // Unity, which is the one position a hand looks for and is nowhere
        // near the middle of a decibel scale. Across the slot, so it reads as
        // a mark ON the scale and not as a twelfth tick.
        let uy = r.bottom() - r.height() * gain_to_fader(1.0).clamp(0.0, 1.0);
        painter.line_segment(
            [
                Pos2::new(slot.left() - 1.0, uy),
                Pos2::new(slot.right() + 1.0, uy),
            ],
            Stroke::new(1.0, p.cap.gamma_multiply(0.5)),
        );
    }

    let t = gain_to_fader(gain).clamp(0.0, 1.0);
    let y = r.bottom() - r.height() * t;
    let w = (r.width() * 0.72).clamp(12.0, 34.0);
    let h = (w * 0.52).clamp(9.0, 20.0);
    let cap = Rect::from_center_size(Pos2::new(x, y), Vec2::new(w, h));
    // A dark seat under the cap, so it sits ON the slot rather than in it.
    painter.rect_filled(cap.expand(1.0), 3.0, p.slot);
    painter.rect_filled(cap, 2.0, p.cap);
    // The ribs, and the line through the middle that is the cap's own edge —
    // the one thing on it a hand aims at.
    let ribs = ((h / 3.0).floor() as usize).min(4);
    if ribs >= 2 {
        for i in 1..ribs {
            let ry = cap.top() + cap.height() * (i as f32 / ribs as f32);
            painter.line_segment(
                [
                    Pos2::new(cap.left() + 2.0, ry),
                    Pos2::new(cap.right() - 2.0, ry),
                ],
                Stroke::new(0.75, p.cap_rib),
            );
        }
    }
    painter.line_segment(
        [
            Pos2::new(cap.left() + 1.0, y),
            Pos2::new(cap.right() - 1.0, y),
        ],
        Stroke::new(1.25, p.engrave.gamma_multiply(0.55)),
    );
}

fn switch(painter: &Painter, r: Rect, label: &str, on: bool, lit: Color32, p: &Ink) {
    if !r.is_positive() {
        return;
    }
    painter.rect_filled(r, 2.0, if on { lit } else { p.track });
    let size = (r.height() * 0.62).clamp(8.0, 13.0);
    centred(
        painter,
        r,
        label,
        FontId::new(size, crate::fonts::courier_bold()),
        // **Light either way.** The well behind an unlit switch is nearly
        // black, so the dark ink the rest of the strip uses vanished into it —
        // two unlabelled rectangles under every channel, which is what the
        // first version of this drew.
        if on {
            p.engrave
        } else {
            p.face.gamma_multiply(0.78)
        },
    );
}

/// Centred, and **cut to the cell rather than run out of it**.
///
/// A channel's name is whatever the user typed and whatever the interface
/// calls itself, and a mixer strip is a hundred points wide: "Scarlett 18i20
/// USB - 3" drawn whole reaches across two neighbours and reads as a fault in
/// the drawing. An ellipsis says "there is more of this"; overflow says
/// nothing at all.
fn centred(painter: &Painter, r: Rect, text: &str, font: FontId, colour: Color32) {
    if !r.is_positive() || text.is_empty() {
        return;
    }
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        &cut_to_fit(painter, r.width(), text, &font),
        font,
        colour,
    );
}

/// The longest prefix of `text` that fits `width`, with an ellipsis if it was
/// cut. Measured through the painter's own fonts, because a character's width
/// is a property of the face and not of the count.
/// A plugin's name for a rack bay: the end of it, not the beginning.
///
/// **The vendor is the first word and the useless one.** "FabFilter Pro-R 2"
/// cut to fit gives "FabFilter Pro…", and a rack of those is three bays that
/// all say FabFilter — the manufacturer, which is the same on all of them,
/// crowding out the part that says which plugin it is. So leading words are
/// dropped until what is left fits, and only then is it cut.
fn shorten_plugin(painter: &Painter, width: f32, name: &str, font: &FontId) -> String {
    let fits = |t: &str| {
        painter
            .layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE)
            .rect
            .width()
            <= width
    };
    if fits(name) {
        return name.to_owned();
    }
    let words: Vec<&str> = name.split_whitespace().collect();
    for drop in 1..words.len() {
        let tail = words[drop..].join(" ");
        if fits(&tail) {
            return tail;
        }
    }
    // Nothing fits whole, so the LAST word cut is still better than the first
    // word whole: "Pro-R…" names a plugin and "FabFilt…" names a company.
    cut_to_fit(painter, width, words.last().copied().unwrap_or(name), font)
}

/// What `c` actually looks like once it is painted over `under`.
///
/// **Colours here are premultiplied and some of them are translucent.** A
/// silent strip's face is the bone at 62%, whose raw bytes read as a mid grey
/// while the eye sees a warm tan over the walnut — so any decision made from
/// the bytes is made about a colour that is never on screen.
fn flatten(c: Color32, under: Color32) -> Color32 {
    let a = f32::from(c.a()) / 255.0;
    let mix = |s: u8, d: u8| (f32::from(s) + f32::from(d) * (1.0 - a)).min(255.0) as u8;
    Color32::from_rgb(
        mix(c.r(), under.r()),
        mix(c.g(), under.g()),
        mix(c.b(), under.b()),
    )
}

/// Perceived luminance, 0..=255.
///
/// Weighted rather than averaged: at equal numbers green reads far lighter
/// than blue, and a desk with twenty-seven paints on it has several of both.
fn luma(c: Color32) -> f32 {
    0.299 * f32::from(c.r()) + 0.587 * f32::from(c.g()) + 0.114 * f32::from(c.b())
}

/// Ink that can be read on `bg`, whatever somebody painted the channel.
///
/// `bg` must be a colour that is really on screen — see [`flatten`].
fn print_on(bg: Color32, p: &Ink) -> Color32 {
    if luma(bg) > 140.0 { p.engrave } else { p.face }
}

fn cut_to_fit(painter: &Painter, width: f32, text: &str, font: &FontId) -> String {
    let measure = |t: &str| {
        painter
            .layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE)
            .rect
            .width()
    };
    if measure(text) <= width {
        return text.to_owned();
    }
    // From the end: the front of a name is the half that identifies it.
    let mut keep = text.len();
    while keep > 0 {
        keep -= 1;
        while keep > 0 && !text.is_char_boundary(keep) {
            keep -= 1;
        }
        let cut = format!("{}\u{2026}", text[..keep].trim_end());
        if measure(&cut) <= width {
            return cut;
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_strip(strip: Option<Strip>, empty: bool) -> StripView<'static> {
        StripView {
            strip,
            name: "CHANNEL",
            detail: "",
            inserts: [""; INSERTS],
            // A used general channel in the tests is MIDI unless a test says
            // otherwise; track/click/master ignore it.
            kind: (!empty && matches!(strip, Some(Strip::Channel(_))))
                .then_some(crate::recorder::ChannelKind::Midi),
            color: 0,
            stereo: true,
            gr_db: 0.0,
            empty,
            gain: 1.0,
            send: 0.0,
            peak: [0.0; 2],
            muted: false,
            soloed: false,
        }
    }

    /// Every slot filled, so the ordinary strip is what is being measured.
    fn a_view() -> MixerView<'static> {
        let channels = Strip::shown();
        MixerView {
            strips: std::array::from_fn(|i| {
                a_strip(channels.get(i).copied(), false)
            }),
            any_solo: false,
            palette_open: None,
            typing: None,
            naming: None,
            dark_mode: true,
            wood: (0x4A, 0x3B, 0x2C),
        }
    }

    /// The index of the last channel before the master.
    const MASTER: usize = COLUMNS - 1;
    /// The effects return, which is the channel before that.
    /// The click's column, which is the one before the master.
    const CLICK: usize = COLUMNS - 2;

    fn rect() -> Rect {
        Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(1300.0, 420.0))
    }

    /// **Every control can be pressed, and presses where it is drawn.**
    ///
    /// `hit_test` is the painter's inverse and both read `targets`, so this is
    /// the test that keeps them one thing: a control that moved and took its
    /// hit region with it passes, and one that moved without it does not.
    #[test]
    fn every_control_is_where_it_is_drawn() {
        let v = a_view();
        let r = rect();
        let l = Layout::new(r, &v);
        for (region, hit) in l.targets() {
            assert!(
                region.is_positive(),
                "{hit:?} has no rectangle but is offered as a target"
            );
            assert_eq!(
                hit_test(r, &v, region.center()),
                Some(hit),
                "{hit:?} could not be pressed at its own centre"
            );
            assert!(
                r.contains(region.center()),
                "{hit:?} is drawn outside the mixer"
            );
        }
    }

    /// No two controls overlap, or one of them is unreachable.
    #[test]
    fn no_two_controls_are_in_the_same_place() {
        let v = a_view();
        let all = Layout::new(rect(), &v).targets();
        for (i, (a, ha)) in all.iter().enumerate() {
            for (b, hb) in all.iter().skip(i + 1) {
                assert!(
                    !a.intersects(*b),
                    "{ha:?} and {hb:?} overlap, so one of them cannot be pressed"
                );
            }
        }
    }

    /// **The master takes no send and cannot be muted.**
    ///
    /// Muting the master is turning it down and soloing it means nothing, so a
    /// strip that could do either would be offering a control with no meaning
    /// behind it. The effects return takes no send either: a bus that can feed
    /// itself is a bus that howls.
    #[test]
    fn the_master_and_the_bus_are_not_offered_what_they_cannot_do() {
        let v = a_view();
        let l = Layout::new(rect(), &v);
        let master = &l.strips[MASTER];
        assert!(!master.send.is_positive(), "the master was given a send");
        assert!(!master.mute.is_positive(), "the master was given a mute");
        assert!(!master.solo.is_positive(), "the master was given a solo");
        assert!(master.fader.is_positive(), "the master has no fader");

        // The click, before the master: a fader and both switches but NO
        // send — a metronome feeding the reverb is not a thing, and the send
        // was deleted rather than defaulted to zero.
        let click = &l.strips[CLICK];
        assert!(!click.send.is_positive(), "the click was given a send");
        assert!(click.mute.is_positive() && click.solo.is_positive());
        assert!(click.fader.is_positive(), "the click has no fader");
        // And the backing track before it is an ordinary channel.
        let track = &l.strips[CLICK - 1];
        assert!(track.send.is_positive() && track.mute.is_positive());
    }

    /// A strip with no send gives that room to its meter rather than leaving a
    /// hole where a control would be.
    #[test]
    fn a_strip_without_a_send_has_a_taller_meter() {
        let v = a_view();
        let l = Layout::new(rect(), &v);
        assert!(
            l.strips[MASTER].meter.height() > l.strips[0].meter.height(),
            "the master left a hole where a send would have been"
        );
    }

    /// **Solo takes the others out of the mix, and the drawing says so.**
    #[test]
    fn soloing_one_strip_silences_the_rest() {
        let mut v = a_view();
        v.strips[2].soloed = true;
        v.any_solo = true;
        assert!(v.heard(2), "the soloed strip was silent");
        assert!(!v.heard(0), "an unsoloed strip was still heard");
        // And the master is never taken out by a solo, or soloing anything
        // would mute the app.
        assert!(v.heard(MASTER), "solo silenced the master");
    }

    /// **An empty slot offers one control, and it is the plus.**
    ///
    /// Not a dead fader, a dead send and two dead switches — which is exactly
    /// what the plugin rack was already doing wrong in the other direction.
    #[test]
    fn an_empty_slot_is_a_plus_and_nothing_else() {
        let mut v = a_view();
        v.strips[0].empty = true;
        let l = Layout::new(rect(), &v);
        let s = &l.strips[0];
        assert!(s.add.is_positive(), "there is no way to fill the slot");
        for (r, what) in [
            (s.fader, "fader"),
            (s.send, "send"),
            (s.mute, "mute"),
            (s.solo, "solo"),
            (s.meter, "meter"),
        ] {
            assert!(!r.is_positive(), "an empty slot was given a {what}");
        }
        assert_eq!(
            hit_test(rect(), &v, s.add.center()),
            Some(Hit::Add(Column(0))),
            "the plus does not open anything"
        );
        // And a filled one has no plus in the MIDDLE of it, where the fader
        // is. It keeps the same target on its icon — the picker is how you
        // change what is on a channel, filled or not — so what is checked is
        // that the two are not in two places: `add` is the icon exactly.
        let filled = Layout::new(rect(), &a_view());
        assert!(filled.strips[0].icon.is_positive(), "a filled slot has no icon");
        assert_eq!(
            filled.strips[0].add, filled.strips[0].icon,
            "the picker is somewhere other than the icon that opens it"
        );
        assert!(
            !filled.strips[0].add.contains(filled.strips[0].fader.center()),
            "the picker is over the fader"
        );
        assert_eq!(
            hit_test(rect(), &a_view(), filled.strips[0].icon.center()),
            Some(Hit::Add(Column(0))),
            "the icon does not open the picker"
        );
    }

    /// **An empty channel is pressable everywhere, not just on the plus.**
    ///
    /// A hundred points of empty column that ignores a press is a channel that
    /// looks dead until you find the one spot in the middle that is not.
    #[test]
    fn an_empty_channel_is_pressable_anywhere_on_it() {
        let mut v = a_view();
        v.strips[0].empty = true;
        let r = rect();
        let l = Layout::new(r, &v);
        let panel = l.strips[0].panel;
        for (what, at) in [
            ("the middle", panel.center()),
            ("the top-left", Pos2::new(panel.left() + 3.0, panel.top() + 3.0)),
            ("the bottom-right", Pos2::new(panel.right() - 3.0, panel.bottom() - 3.0)),
            ("the top edge", Pos2::new(panel.center().x, panel.top() + 1.0)),
            ("the bottom edge", Pos2::new(panel.center().x, panel.bottom() - 1.0)),
        ] {
            assert_eq!(
                hit_test(r, &v, at),
                Some(Hit::Add(Column(0))),
                "{what} of an empty channel does nothing"
            );
        }
    }

    /// **The label sits at the same height on every column, master included.**
    ///
    /// The icon used to go above the name, and the master has no source to
    /// choose and so no icon — so twelve labels sat at one height and the
    /// master's at another, which reads as the master being a different kind
    /// of object rather than the same strip with nothing to pick.
    #[test]
    fn every_label_is_at_the_same_height_as_the_masters() {
        let v = a_view();
        let l = Layout::new(rect(), &v);
        let master = l.strips[COLUMNS - 1].name;
        assert!(master.is_positive(), "the master has no name plate");
        for (i, s) in l.strips.iter().enumerate() {
            if !s.name.is_positive() {
                continue;
            }
            assert!(
                (s.name.top() - master.top()).abs() < 0.5,
                "column {i}'s label is at {} where the master's is at {}",
                s.name.top(),
                master.top()
            );
            // And where a column has an icon it is UNDER the writing.
            if s.icon.is_positive() {
                assert!(
                    s.icon.top() >= s.name.bottom(),
                    "column {i} draws its icon over its own name"
                );
            }
        }
    }

    /// **The scale beside a meter can be read on every channel.**
    ///
    /// The numbers were a faded ivory, chosen against the walnut the desk sits
    /// on — and every UNPAINTED channel has an ivory face, so on eight strips
    /// out of twelve the scale was ivory on ivory and simply absent. The
    /// owner's words were "ensure every single track has legible readout
    /// ticks", and the fault was never the size.
    ///
    /// Contrast rather than a colour: this has to hold for all twenty-seven
    /// paints as well as for the bare face, and naming the right answer for
    /// each of them is a table that would go stale the next time one changed.
    #[test]
    fn the_scale_can_be_read_on_every_face_a_channel_can_have() {
        let v = a_view();
        let p = ink(&v);
        let mut faces = vec![
            ("the bare face", p.face),
            ("a silent strip", p.face.gamma_multiply(0.62)),
        ];
        for (i, &(r, g, b)) in STRIP_COLORS.iter().enumerate() {
            faces.push((
                match i {
                    0 => "colour 0",
                    _ => "a painted strip",
                },
                Color32::from_rgb(r, g, b),
            ));
        }
        for (what, face) in faces {
            // Exactly what the panel does: the face over the desk, then the
            // ink over that.
            let face = flatten(face, p.wood);
            let printed = flatten(print_on(face, &p).gamma_multiply(0.62), face);
            let d = (luma(printed) - luma(face)).abs();
            assert!(
                d > 40.0,
                "{what} ({face:?}) prints its scale at a difference of {d:.0}, \
                 which is a mark where a number should be"
            );
        }
    }

    /// Every instrument slot is a channel, filled or not.
    #[test]
    fn the_desk_has_a_strip_for_every_slot() {
        use crate::recorder::CHANNELS;
        let channels = Strip::shown();
        for n in 0..CHANNELS {
            assert_eq!(channels[n], Strip::Channel(n), "channel {n} has no column");
        }
        // **Drawn: the eight, the track, and — new — the click.** Only the
        // effects return keeps its off-desk life: its three knobs are the
        // band's, and a soloable bus is a feedback question.
        assert_eq!(
            COLUMNS,
            crate::recorder::CHANNELS + 3,
            "a drawn channel went missing"
        );
        assert_eq!(channels.len(), crate::recorder::CHANNELS + 2);
        assert!(channels.contains(&Strip::Click), "the click lost its column");
        assert!(!channels.contains(&Strip::Fx), "the bus is drawn twice");
        // Every strip owns a distinct place in the arrays, or two of them
        // would share a send and mute together.
        let mut seen: Vec<usize> = Strip::all().iter().map(|s| s.index()).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "two channels share an index");
    }

    /// **Three inserts on every channel, and all three can be pressed.**
    ///
    /// There used to be ONE, on the effects bus alone. A desk where one
    /// channel can hold an effect and another cannot is a desk you have to
    /// remember the rules of.
    #[test]
    fn every_channel_has_three_inserts_you_can_fill() {
        let v = a_view();
        let r = rect();
        let l = Layout::new(r, &v);
        for (i, s) in l.strips.iter().enumerate() {
            if !s.panel.is_positive() || v.strips[i].empty {
                continue;
            }
            for n in 0..INSERTS {
                assert!(
                    s.inserts[n].is_positive(),
                    "channel {i} has no room for insert {n}"
                );
                assert_eq!(
                    hit_test(r, &v, s.inserts[n].center()),
                    Some(Hit::Insert(Column(i), n)),
                    "channel {i}'s insert {n} cannot be pressed"
                );
            }
            // **And each bay is as wide as the channel.** The whole reason
            // they were stacked: three across a hundred-point strip held about
            // three characters of a plugin's name, which is a rack you have to
            // click to read.
            for n in 0..INSERTS {
                assert!(
                    s.inserts[n].width() > s.panel.width() / INSERTS as f32 * 2.0,
                    "channel {i}'s insert {n} is no wider than a third of the \
                     strip, which is what stacking them was for"
                );
            }
            for n in 1..INSERTS {
                // **Down the strip now, not across it.** Each bay is as wide
                // as the channel — that is where the reading room came from —
                // so what separates them is the top edge of one against the
                // bottom of the one above.
                assert!(
                    s.inserts[n].top() >= s.inserts[n - 1].bottom(),
                    "channel {i}'s inserts {} and {n} overlap",
                    n - 1
                );
            }
        }
        // An EMPTY slot is an outline and a plus, and nothing else: three
        // effect chips on a channel with no instrument in it would be three
        // controls for something that is not there.
        let mut empty = a_view();
        empty.strips[0] = a_strip(Some(Strip::Channel(0)), true);
        let l = Layout::new(r, &empty);
        assert!(!l.strips[0].inserts[0].is_positive());
    }

    /// **A channel's name is a field, and a long one is cut.**
    ///
    /// A desk is a set of names before it is a set of faders — "BELL KEYS" is
    /// what the app knows and "Rhodes" is what the person at the desk knows —
    /// so every drawn channel has a name you can press and type into. And the
    /// name that arrives from an interface is "Scarlett 18i20 USB - 3", which
    /// has never fitted a strip a hundred points wide.
    #[test]
    fn every_channel_has_a_name_you_can_press() {
        let v = a_view();
        let r = rect();
        let l = Layout::new(r, &v);
        for (i, s) in l.strips.iter().enumerate() {
            if !s.panel.is_positive() {
                continue;
            }
            assert!(s.name.is_positive(), "channel {i} has nowhere to put a name");
            assert_eq!(
                hit_test(r, &v, s.name.center()),
                Some(Hit::Name(Column(i))),
                "channel {i}'s name cannot be pressed"
            );
        }
        // An EMPTY slot has no name to change: it is an outline and a plus,
        // and a name field on it would be a control for a channel that is not
        // there yet.
        let mut empty = a_view();
        empty.strips[0] = a_strip(Some(Strip::Channel(0)), true);
        let l = Layout::new(r, &empty);
        assert!(!l.strips[0].name.is_positive(), "an empty slot got a name field");
        assert_eq!(hit_test(r, &empty, l.strips[0].panel.center()), Some(Hit::Add(Column(0))));
    }

    /// **Every mark has to land on the scale it is drawn against.**
    ///
    /// The fader's travel is +12 down to -60. A tick outside that is clamped to
    /// an end and sits on top of its neighbour, which is a scale that says two
    /// different levels are the same height — the exact thing a scale exists
    /// to stop.
    #[test]
    fn every_meter_tick_lands_where_the_fader_would_put_it() {
        for (which, ticks) in [
            ("channel", &METER_TICKS[..]),
            ("master", &MASTER_TICKS[..]),
        ] {
            let mut seen: Vec<f32> = Vec::new();
            for &db in ticks {
                let t = gain_to_fader(10f32.powf(db / 20.0));
                assert!(
                    t > 0.0 && t < 1.0,
                    "{which}'s {db} dB mark sits at {t}, which is off the end of \
                     the travel and on top of whatever is there"
                );
                for other in &seen {
                    assert!(
                        (other - t).abs() > 0.01,
                        "{which} has two marks at {t}, so one of them is a label \
                         nobody can read"
                    );
                }
                seen.push(t);
            }
        }
        // The master column is the one you read a number off, and it is wider
        // — so it carries the scale a channel has no room for.
        assert!(
            MASTER_TICKS.len() > METER_TICKS.len(),
            "the master's scale is no fuller than a channel's"
        );
    }

    /// **A colour you cannot pick without moving the channel beside it.**
    ///
    /// The palette could be wider than the strip it belongs to and the
    /// swatches would be easier to hit — but a press in the overhang falls
    /// through to the NEIGHBOUR's fader, which is a colour picker that
    /// sometimes changes a level. So every swatch stays inside its own
    /// channel, and every one of them is reachable.
    #[test]
    fn every_swatch_is_inside_its_own_channel_and_can_be_pressed() {
        let mut v = a_view();
        v.palette_open = Some(Column(0));
        let r = rect();
        let l = Layout::new(r, &v);
        let panel = l.strips[0].panel;
        let swatches = palette_over(&l.strips[0]);
        assert_eq!(swatches.len(), STRIP_COLORS.len(), "a colour is unreachable");
        for (i, sw) in swatches.iter().enumerate() {
            assert!(
                panel.contains(sw.min) && panel.contains(sw.max),
                "swatch {i} hangs outside the channel it belongs to"
            );
            assert!(sw.is_positive(), "swatch {i} has no area");
            assert_eq!(
                hit_test(r, &v, sw.center()),
                Some(Hit::Paint(Column(0), i)),
                "swatch {i} cannot be pressed"
            );
            for (j, other) in swatches.iter().enumerate().take(i) {
                assert!(
                    !other.intersects(*sw),
                    "swatches {j} and {i} overlap, so one of them wins twice"
                );
            }
        }
        // Hues down, tones across: the grid is what makes twenty-seven
        // swatches pickable without reading every one of them.
        assert_eq!(
            (STRIP_COLORS.len() - PALETTE_COLUMNS) % PALETTE_COLUMNS,
            0,
            "the neutrals and the hues do not line up into rows"
        );
        // The master's default has to BE in the table it indexes.
        assert!(MASTER_COLOR < STRIP_COLORS.len());
    }

    /// **The decibel readout is a target, and the palette swallows the strip.**
    ///
    /// Both are things the first version had no room for and both are now the
    /// difference between a fader you fight and one you tell.
    #[test]
    fn a_number_can_be_typed_and_a_palette_covers_what_is_under_it() {
        let v = a_view();
        let r = rect();
        let l = Layout::new(r, &v);
        assert_eq!(
            hit_test(r, &v, l.strips[0].db.center()),
            Some(Hit::Db(Column(0))),
            "the number could not be clicked"
        );

        // With a palette open, everything under it is a swatch or a dismissal
        // — a swatch over a fader that also moved the fader would be a colour
        // you cannot pick without changing a level.
        let mut open = a_view();
        open.palette_open = Some(Column(0));
        let swatches = palette_over(&l.strips[0]);
        assert_eq!(swatches.len(), STRIP_COLORS.len());
        for (c, s) in swatches.iter().enumerate() {
            assert_eq!(hit_test(r, &open, s.center()), Some(Hit::Paint(Column(0), c)));
        }
        // The fader is under it and cannot be reached.
        assert!(matches!(
            hit_test(r, &open, l.strips[0].fader.center()),
            Some(Hit::Paint(Column(0), _))
        ));
        // And a strip that is NOT the open one is untouched.
        assert_eq!(
            hit_test(r, &open, l.strips[1].fader.center()),
            Some(Hit::Fader(Column(1)))
        );
    }

    /// Everything that travels does so vertically, and knows how far.
    #[test]
    fn every_travelling_control_has_an_axis_and_a_distance() {
        for hit in [Hit::Fader(Column(0)), Hit::Send(Column(1))] {
            assert_eq!(hit.axis(), Some(DragAxis::Vertical), "{hit:?}");
            assert!(hit.travel().is_some_and(|t| t > 0.0), "{hit:?}");
        }
        for hit in [Hit::Mute(Column(0)), Hit::Solo(Column(0))] {
            assert_eq!(hit.axis(), None, "{hit:?} is not a thing you drag");
            assert_eq!(hit.travel(), None, "{hit:?}");
        }
    }

    /// It draws at the smallest band the app will ever hand it, without
    /// panicking and without putting anything outside the rect.
    #[test]
    fn it_survives_a_rect_far_too_small_to_be_useful() {
        let v = a_view();
        for size in [
            Vec2::new(1300.0, 420.0),
            Vec2::new(640.0, 180.0),
            Vec2::new(120.0, 60.0),
            Vec2::new(4.0, 4.0),
            Vec2::new(0.0, 0.0),
        ] {
            let r = Rect::from_min_size(Pos2::new(7.0, 11.0), size);
            let l = Layout::new(r, &v);
            for (region, hit) in l.targets() {
                assert!(
                    r.contains(region.center()),
                    "{hit:?} escaped a {size:?} mixer"
                );
            }
        }
    }
}
