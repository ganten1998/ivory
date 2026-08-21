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

use crate::recorder::{gain_text, gain_to_fader, Strip, STRIPS};
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
    /// The loudest thing it made since the last frame, as a magnitude.
    pub peak: f32,
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
    /// The fader of strip `n`, where `n` indexes [`MixerView::strips`].
    Fader(usize),
    /// The send knob of strip `n`.
    Send(usize),
    Mute(usize),
    Solo(usize),
    /// The plus on an empty slot: put an instrument here.
    Add(usize),
}

impl Hit {
    /// Whether it travels under the hand, and along which axis.
    ///
    /// The same contract the band's controls use, and deliberately the same
    /// feel: relative, so a press never makes the handle jump, and the hand
    /// can leave the control and keep going.
    pub fn axis(self) -> Option<DragAxis> {
        match self {
            Hit::Fader(_) => Some(DragAxis::Vertical),
            Hit::Send(_) => Some(DragAxis::Vertical),
            Hit::Mute(_) | Hit::Solo(_) | Hit::Add(_) => None,
        }
    }

    /// How far the pointer must move to sweep it end to end, in points.
    pub fn travel(self) -> Option<f32> {
        match self {
            Hit::Fader(_) => Some(FADER_TRAVEL),
            Hit::Send(_) => Some(SEND_TRAVEL),
            Hit::Mute(_) | Hit::Solo(_) | Hit::Add(_) => None,
        }
    }
}

/// Which way a control travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragAxis {
    Vertical,
}

/// **Up is louder.** A vertical fader dragged downward turning up would be
/// wrong in a way nobody would report as a bug and everybody would feel.
const FADER_TRAVEL: f32 = 160.0;

/// Wider than the knob by a lot, for the reason the band's knobs are: a knob's
/// cell is a few tens of points and mapping that to the whole range is a
/// control nobody can land on a number with.
const SEND_TRAVEL: f32 = 200.0;

/// Where everything on one strip is, and where the strips are.
///
/// Fractions of the rect rather than points, so the same layout holds at every
/// window size the app offers — and so the hit test and the painter cannot
/// drift, because they read the same struct.
/// Channels plus the master.
pub const COLUMNS: usize = STRIPS + 1;

pub struct Layout {
    pub strips: [StripStrip; COLUMNS],
}

/// One strip's rectangles.
#[derive(Debug, Clone, Copy)]
pub struct StripStrip {
    pub panel: Rect,
    pub name: Rect,
    pub detail: Rect,
    pub meter: Rect,
    pub send: Rect,
    pub fader: Rect,
    pub db: Rect,
    pub mute: Rect,
    pub solo: Rect,
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
        name: Rect::NOTHING,
        detail: Rect::NOTHING,
        meter: Rect::NOTHING,
        send: Rect::NOTHING,
        fader: Rect::NOTHING,
        db: Rect::NOTHING,
        mute: Rect::NOTHING,
        solo: Rect::NOTHING,
        add: Rect::NOTHING,
    };
}

/// Space around the rack, and between the strips.
const PAD: f32 = 10.0;
const GAP: f32 = 6.0;
/// The master is wider, because it is the one you look at.
const MASTER_EXTRA: f32 = 0.35;

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
        // Every channel one unit wide, and the master a little more so it
        // reads as the end of the row rather than another channel.
        let gaps = (COLUMNS - 1) as f32;
        let units = gaps + 1.0 + MASTER_EXTRA;
        let unit = (inner.width() - GAP * gaps) / units;
        let mut x = inner.left();
        let strips = std::array::from_fn(|i| {
            let w = if i == COLUMNS - 1 {
                unit * (1.0 + MASTER_EXTRA)
            } else {
                unit
            };
            let panel = Rect::from_min_size(Pos2::new(x, inner.top()), Vec2::new(w, inner.height()));
            x += w + GAP;
            Self::one(panel, view.strips.get(i))
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
            let plus = Rect::from_center_size(
                panel.center(),
                Vec2::splat(panel.width().min(panel.height()) * 0.34),
            );
            return StripStrip {
                panel,
                add: plus,
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
        let send = if sends { cut(0.19, 0.37) } else { Rect::NOTHING };
        // Short of the bottom edge: a switch flush against the frame reads as
        // something that has been cut off rather than as the end of the strip.
        let switches = if switchable { cut(0.90, 0.975) } else { Rect::NOTHING };
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
        let travel = cut(if sends { 0.41 } else { 0.24 }, 0.82);
        // The meter takes the left third and the fader the rest, so the fader
        // still sits near the middle of the strip where a hand expects it.
        let split = travel.left() + travel.width() * 0.34;
        StripStrip {
            panel,
            name: cut(0.02, 0.10),
            detail: cut(0.10, 0.16),
            meter: Rect::from_min_max(travel.min, Pos2::new(split, travel.max.y)),
            send,
            fader: Rect::from_min_max(Pos2::new(split, travel.min.y), travel.max),
            db: cut(0.83, 0.90),
            mute,
            solo,
            add: Rect::NOTHING,
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
            for (r, hit) in [
                (s.fader, Hit::Fader(i)),
                (s.send, Hit::Send(i)),
                (s.mute, Hit::Mute(i)),
                (s.solo, Hit::Solo(i)),
                (s.add, Hit::Add(i)),
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
    Layout::new(rect, view)
        .targets()
        .into_iter()
        .find(|(r, _)| r.contains(pos))
        .map(|(_, h)| h)
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
    cap: Color32,
    lit: Color32,
    mute: Color32,
    solo: Color32,
    meter_lo: Color32,
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
        cap: Color32::from_rgb(0x4A, 0x42, 0x38),
        lit: Color32::from_rgb(0x1C, 0x6F, 0xD6),
        mute: Color32::from_rgb(0xE0, 0xA8, 0x22),
        solo: Color32::from_rgb(0xE8, 0x3A, 0x4E),
        meter_lo: Color32::from_rgb(0x3D, 0xC0, 0x5A),
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
        strip(painter, s, v, &p, heard);
    }
}

fn strip(painter: &Painter, l: &StripStrip, v: &StripView<'_>, p: &Ink, heard: bool) {
    if v.empty {
        empty_slot(painter, l, p);
        return;
    }
    let face = if heard {
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

    centred(painter, l.name, v.name, cap((h * 0.032).clamp(7.5, 12.0)), p.engrave);
    if !v.detail.is_empty() {
        centred(
            painter,
            l.detail,
            v.detail,
            plain((h * 0.024).clamp(7.0, 11.0)),
            p.faint,
        );
    }

    meter(painter, l.meter, v.peak, heard, p);

    if l.send.is_positive() {
        send_knob(painter, l.send, v.send, p);
    }

    fader(painter, l.fader, v.gain, p);
    centred(
        painter,
        l.db,
        &gain_text(v.gain),
        plain((h * 0.026).clamp(8.0, 12.0)),
        p.engrave,
    );

    if l.mute.is_positive() {
        switch(painter, l.mute, "M", v.muted, p.mute, p);
        switch(painter, l.solo, "S", v.soloed, p.solo, p);
    }
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
    let arm = r.width().min(r.height()) * 0.5;
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
fn meter(painter: &Painter, r: Rect, peak: f32, heard: bool, p: &Ink) {
    if !r.is_positive() {
        return;
    }
    let w = (r.width() * 0.52).clamp(6.0, 18.0);
    let r = Rect::from_center_size(r.center(), Vec2::new(w, r.height()));
    painter.rect_filled(r, 2.0, p.track);
    if !heard || peak <= 0.0 {
        return;
    }
    // The same curve the faders use, so a strip at unity reads at the same
    // height on both — a meter on a different scale from the fader beside it
    // is two rulers on one wall.
    let t = gain_to_fader(peak).clamp(0.0, 1.0);
    let top = r.bottom() - r.height() * t;
    let filled = Rect::from_min_max(Pos2::new(r.left(), top), r.max);
    let colour = if peak >= 0.9 { p.meter_hi } else { p.meter_lo };
    painter.rect_filled(filled, 2.0, colour);
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
fn fader(painter: &Painter, r: Rect, gain: f32, p: &Ink) {
    if !r.is_positive() {
        return;
    }
    let x = r.center().x;
    let track = Rect::from_min_max(
        Pos2::new(x - 2.0, r.top()),
        Pos2::new(x + 2.0, r.bottom()),
    );
    painter.rect_filled(track, 2.0, p.track);
    let t = gain_to_fader(gain).clamp(0.0, 1.0);
    let y = r.bottom() - r.height() * t;
    let w = (r.width() * 0.62).clamp(10.0, 30.0);
    let cap = Rect::from_center_size(Pos2::new(x, y), Vec2::new(w, 8.0));
    painter.rect_filled(cap, 2.0, p.cap);
    painter.line_segment(
        [
            Pos2::new(cap.left() + 2.0, y),
            Pos2::new(cap.right() - 2.0, y),
        ],
        Stroke::new(1.0, p.face),
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

fn centred(painter: &Painter, r: Rect, text: &str, font: FontId, colour: Color32) {
    if !r.is_positive() {
        return;
    }
    painter.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        text,
        font,
        colour,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_strip(strip: Option<Strip>, empty: bool) -> StripView<'static> {
        StripView {
            strip,
            name: "CHANNEL",
            detail: "",
            empty,
            gain: 1.0,
            send: 0.0,
            peak: 0.0,
            muted: false,
            soloed: false,
        }
    }

    /// Every slot filled, so the ordinary strip is what is being measured.
    fn a_view() -> MixerView<'static> {
        let channels = Strip::all();
        MixerView {
            strips: std::array::from_fn(|i| {
                a_strip(channels.get(i).copied(), false)
            }),
            any_solo: false,
            dark_mode: true,
            wood: (0x4A, 0x3B, 0x2C),
        }
    }

    /// The index of the last channel before the master.
    const MASTER: usize = COLUMNS - 1;
    /// The effects return, which is the channel before that.
    const FX: usize = COLUMNS - 2;

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

        let fx = &l.strips[FX];
        assert!(!fx.send.is_positive(), "the effects bus can feed itself");
        assert!(fx.mute.is_positive(), "the effects return cannot be muted");
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
            Some(Hit::Add(0)),
            "the plus does not open anything"
        );
        // And a filled one has no plus, or there would be two ways to mean two
        // different things in one place.
        let filled = Layout::new(rect(), &a_view());
        assert!(!filled.strips[0].add.is_positive());
    }

    /// Every instrument slot is a channel, filled or not.
    #[test]
    fn the_desk_has_a_strip_for_every_slot() {
        use crate::recorder::SLOTS;
        let channels = Strip::all();
        for n in 0..SLOTS {
            assert_eq!(channels[n], Strip::Slot(n), "slot {n} is not a channel");
        }
        assert_eq!(COLUMNS, SLOTS + 5, "a channel went missing");
        // Every strip owns a distinct place in the arrays, or two of them
        // would share a send and mute together.
        let mut seen: Vec<usize> = channels.iter().map(|s| s.index()).collect();
        let n = seen.len();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), n, "two channels share an index");
    }

    /// Everything that travels does so vertically, and knows how far.
    #[test]
    fn every_travelling_control_has_an_axis_and_a_distance() {
        for hit in [Hit::Fader(0), Hit::Send(1)] {
            assert_eq!(hit.axis(), Some(DragAxis::Vertical), "{hit:?}");
            assert!(hit.travel().is_some_and(|t| t > 0.0), "{hit:?}");
        }
        for hit in [Hit::Mute(0), Hit::Solo(0)] {
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
