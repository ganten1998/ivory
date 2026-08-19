//! Reverb and delay, on everything the app plays.
//!
//! Two knobs on the recorder, modelled on the Tascam 388's effect returns, and
//! one number each: how much. Everything else is chosen here, because the
//! person turning them is playing the piano with their other hand.
//!
//! # Where this sits
//!
//! On the instrument sum, after the slots and the built-in and before the
//! metronome and the input monitor. So:
//!
//! - a VST3 and the built-in FM get the same treatment, which is the point;
//! - the click stays dry, because a reverb tail on a count-in is a worse
//!   count-in and nobody has ever wanted one;
//! - what is heard is what is recorded, because the take is tapped downstream.
//!
//! # Why these algorithms
//!
//! The reverb is Schroeder-Moorer (the "Freeverb" arrangement): eight comb
//! filters in parallel into four allpasses in series, per channel, with the
//! right channel's delays offset so the two decorrelate into something wide.
//! It is thirty years old, it is public domain, it is about a hundred lines,
//! and it costs a few hundred multiply-adds a sample. A convolution reverb
//! would sound better and would mean shipping impulse responses, a partitioned
//! FFT, and a latency budget, for a control that is off by default.
//!
//! The delay is a delay line with feedback and a lowpass in the loop, and its
//! **time comes from the session tempo**. That is the one real opinion in this
//! file: a musician with one knob wants repeats that land on the beat, and
//! Tangent is the rare effect that already knows the tempo because there is a
//! metronome next to it.
//!
//! # Denormals
//!
//! Every feedback path here decays towards zero and never reaches it, and a
//! float that small costs a hundred times a normal one on some hardware — an
//! audio thread that was fine can start dropping buffers a minute after the
//! last note. [`flush`] is the guard, applied where each loop closes.

// ── what a person can change ────────────────────────────────────────────────

/// Where a delay's repeats land, in beats.
///
/// **Named divisions, not a time in milliseconds.** The knob is next to a
/// metronome, and a delay that drifts off the beat every time the tempo
/// changes is a delay somebody has to keep re-setting. Free time is the one
/// entry that does not follow the tempo, for anybody who wants a slapback that
/// stays put.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Division {
    Quarter,
    #[default]
    DottedEighth,
    Eighth,
    TripletEighth,
    Sixteenth,
    /// A fixed 375ms, whatever the tempo is doing.
    Free,
}

impl Division {
    /// Every value, in the order a menu should offer them: longest first.
    pub const ALL: [Division; 6] = [
        Division::Quarter,
        Division::DottedEighth,
        Division::Eighth,
        Division::TripletEighth,
        Division::Sixteenth,
        Division::Free,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Division::Quarter => "1/4",
            Division::DottedEighth => "1/8 dotted",
            Division::Eighth => "1/8",
            Division::TripletEighth => "1/8 triplet",
            Division::Sixteenth => "1/16",
            Division::Free => "free",
        }
    }

    /// How it is written to the settings file, and read back.
    pub fn key(self) -> &'static str {
        match self {
            Division::Quarter => "quarter",
            Division::DottedEighth => "dotted-eighth",
            Division::Eighth => "eighth",
            Division::TripletEighth => "triplet-eighth",
            Division::Sixteenth => "sixteenth",
            Division::Free => "free",
        }
    }

    pub fn from_key(key: &str) -> Option<Division> {
        Division::ALL.into_iter().find(|d| d.key() == key)
    }

    /// Beats per repeat. `Free` has none: see [`FREE_SECS`].
    fn beats(self) -> Option<f32> {
        Some(match self {
            Division::Quarter => 1.0,
            Division::DottedEighth => 0.75,
            Division::Eighth => 0.5,
            Division::TripletEighth => 1.0 / 3.0,
            Division::Sixteenth => 0.25,
            Division::Free => return None,
        })
    }
}

/// Seconds per repeat with [`Division::Free`], and the fallback when there is
/// no sensible tempo to sync to.
const FREE_SECS: f32 = 0.375;

/// Everything about the three effects a person can change.
///
/// **One struct, sent whole.** The alternative is an atomic per field, and
/// there are eleven of them — eleven names to keep in step across the settings
/// file, the menu, the shared state and the renderer. These change when
/// somebody opens a menu, which is never, in audio terms; so they cross to the
/// audio thread as one value behind a lock the renderer only reaches for when
/// a flag says something moved.
///
/// Every field is 0..=1 unless it says otherwise, because that is what a
/// control is, and the mapping to whatever the DSP actually wants lives next to
/// the DSP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    /// How long the room rings. Maps to the comb feedback.
    pub reverb_size: f32,
    /// How fast the top comes off the tail.
    pub reverb_damp: f32,
    /// How far apart the two channels' rooms are. 0 is mono.
    pub reverb_width: f32,

    pub delay_division: Division,
    /// How much of each repeat comes back for the next.
    pub delay_feedback: f32,
    /// How much darker each repeat is than the one before it.
    pub delay_tone: f32,
    /// How far the right channel's time is from the left's. 0 is mono.
    pub delay_width: f32,

    /// Sweep speed, mapped across [`CHORUS_RATE_HZ`].
    pub chorus_rate: f32,
    /// How far the sweep moves the delay.
    pub chorus_depth: f32,
    /// How much of the wet is inverted into the right channel — the CE-1's
    /// own trick. See [`Chorus`].
    pub chorus_width: f32,
    /// The bucket brigade's bandwidth. Down is darker and more like the
    /// hardware; up is a cleaner chorus than ever existed in 1976.
    pub chorus_tone: f32,
}

impl Default for Params {
    /// **These are the sound**, and they are what every knob was voiced
    /// against before there was a menu to change them. A reset goes here.
    fn default() -> Self {
        Self {
            reverb_size: 0.62,
            reverb_damp: 0.35,
            reverb_width: 0.70,
            delay_division: Division::DottedEighth,
            delay_feedback: 0.42,
            delay_tone: 0.55,
            delay_width: 0.60,
            chorus_rate: 0.28,
            chorus_depth: 0.55,
            chorus_width: 0.85,
            chorus_tone: 0.45,
        }
    }
}

impl Params {
    /// Clamp everything into range. Called on the way in from the settings
    /// file, which a person can edit and a later build can have written.
    pub fn sane(mut self) -> Self {
        for v in [
            &mut self.reverb_size,
            &mut self.reverb_damp,
            &mut self.reverb_width,
            &mut self.delay_feedback,
            &mut self.delay_tone,
            &mut self.delay_width,
            &mut self.chorus_rate,
            &mut self.chorus_depth,
            &mut self.chorus_width,
            &mut self.chorus_tone,
        ] {
            *v = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 };
        }
        self
    }
}

/// Below this, a sample is on its way to a denormal and is worth nothing.
///
/// 1e-20 in amplitude is 400 dB down. Nothing musical lives here.
const TINY: f32 = 1.0e-20;

/// Zero anything too small to hear, before it becomes expensive to add.
#[inline]
fn flush(x: f32) -> f32 {
    if x.abs() < TINY {
        0.0
    } else {
        x
    }
}

// ── reverb ──────────────────────────────────────────────────────────────────

/// Comb delay lengths in samples at 44.1 kHz, from the original.
///
/// Mutually prime on purpose: shared factors make the combs agree about which
/// frequencies to reinforce, and a reverb that agrees with itself is a ringing
/// metal box.
const COMB_LEN: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];

/// Allpass lengths, same source and the same reasoning.
const ALLPASS_LEN: [usize; 4] = [556, 441, 341, 225];

/// Samples the right channel's delays are lengthened by, at full width.
///
/// What makes the output stereo at all: the same input through two slightly
/// different rooms. Too small and it is mono; too large and it is an echo.
///
/// Fixed at construction rather than swept, because changing a delay LENGTH
/// resizes a buffer and cannot be done per sample. The width control scales
/// this when the reverb is built, and a change to it rebuilds the pair — which
/// happens when somebody moves a menu slider, not in the audio callback.
const STEREO_SPREAD: usize = 23;

/// The rate the lengths above were chosen at.
const DESIGN_RATE: f32 = 44_100.0;

/// How much of each comb's output returns to its input, at each end of the
/// size control.
///
/// The top is deliberately short of 1.0: a comb at unity never decays, and a
/// control that can be turned to "for ever" is a control somebody turns to for
/// ever by accident.
const COMB_FEEDBACK: (f32, f32) = (0.68, 0.92);

/// The lowpass inside each comb, at each end of the damping control. Air and
/// soft furnishings absorb treble, so a reverb whose highs last as long as its
/// lows sounds like a swimming pool.
const COMB_DAMP: (f32, f32) = (0.06, 0.55);

/// The fixed allpass coefficient from the original. Diffusion, not colour.
const ALLPASS_FEEDBACK: f32 = 0.5;

/// Scales the whole wet path, at [`REVERB_TRIM_AT`]. Eight combs in parallel is
/// eight times the input before anything else happens.
const REVERB_TRIM: f32 = 0.055;

/// The size this trim was voiced at.
///
/// **Size must not be a volume control.** A comb's steady-state gain is
/// `1/(1 - feedback)`, so turning the room up from small to large multiplies
/// the wet by more than two on its own — somebody reaching for a longer tail
/// gets a louder one and turns the send back down to compensate, and now they
/// have two controls fighting. The trim is scaled by `1 - feedback` against
/// this point, which holds the wet level steady across the whole sweep and
/// leaves the DEFAULT sounding exactly as it did before there was a control.
const REVERB_TRIM_AT: f32 = 0.62;

/// A comb filter with a one-pole lowpass in its feedback path.
struct Comb {
    buf: Vec<f32>,
    at: usize,
    /// The lowpass's state: the running average that damps the loop.
    store: f32,
}

impl Comb {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            at: 0,
            store: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        let out = self.buf[self.at];
        self.store = flush(out * (1.0 - damp) + self.store * damp);
        self.buf[self.at] = flush(input + self.store * feedback);
        self.at = (self.at + 1) % self.buf.len();
        out
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
        self.store = 0.0;
    }
}

/// An allpass: passes every frequency at the same level and scrambles phase.
///
/// What turns eight discrete echoes into something that sounds continuous.
struct Allpass {
    buf: Vec<f32>,
    at: usize,
}

impl Allpass {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            at: 0,
        }
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        let buffered = self.buf[self.at];
        self.buf[self.at] = flush(input + buffered * ALLPASS_FEEDBACK);
        self.at = (self.at + 1) % self.buf.len();
        buffered - input
    }

    fn clear(&mut self) {
        self.buf.fill(0.0);
    }
}

/// One channel of reverb: eight combs into four allpasses.
struct ReverbChannel {
    combs: [Comb; 8],
    allpasses: [Allpass; 4],
}

impl ReverbChannel {
    /// `offset` is [`STEREO_SPREAD`] for the right channel and 0 for the left.
    fn new(sample_rate: f32, offset: usize) -> Self {
        // The lengths are in samples at 44.1k, so at 96k the same room needs
        // twice as many. A reverb that ignored this would be half as long and
        // an octave brighter on half the interfaces people own.
        let scale = |len: usize| ((len + offset) as f32 * sample_rate / DESIGN_RATE) as usize;
        Self {
            combs: std::array::from_fn(|i| Comb::new(scale(COMB_LEN[i]))),
            allpasses: std::array::from_fn(|i| Allpass::new(scale(ALLPASS_LEN[i]))),
        }
    }

    #[inline]
    fn process(&mut self, input: f32, feedback: f32, damp: f32) -> f32 {
        // Parallel: every comb sees the same input and their outputs sum.
        let mut out = 0.0;
        for c in &mut self.combs {
            out += c.process(input, feedback, damp);
        }
        // Series: each allpass sees what the one before it made.
        for a in &mut self.allpasses {
            out = a.process(out);
        }
        out
    }

    fn clear(&mut self) {
        for c in &mut self.combs {
            c.clear();
        }
        for a in &mut self.allpasses {
            a.clear();
        }
    }
}

// ── delay ───────────────────────────────────────────────────────────────────

/// How much of each repeat comes back for the next one, at each end of the
/// feedback control.
///
/// The top stops well short of 1.0 on purpose: this is a mix knob on a piano
/// app, not an oscillator, and a delay that can be set to build for ever is a
/// delay that will be, once, loudly.
const DELAY_FEEDBACK: (f32, f32) = (0.0, 0.72);

/// One-pole lowpass in the feedback loop, at each end of the tone control, as
/// a coefficient on the new sample.
///
/// Each repeat darker than the last, which is what tape did and why tape delay
/// sits under a piano without competing with it. The control is INVERTED
/// against this: turning tone up means less damping.
const DELAY_DAMP: (f32, f32) = (0.0, 0.82);

/// The right channel's delay, as a fraction of the left's, at full width.
///
/// Not the same length, or the repeats are mono and sit on top of the dry
/// signal. Two thirds puts the right channel on a triplet against the left,
/// which is a musical relationship rather than a smear.
const DELAY_RIGHT_RATIO: f32 = 0.667;

/// Longest delay the line can hold, in seconds. Two seconds covers a whole bar
/// at 30 bpm.
const MAX_DELAY_SECS: f32 = 2.0;

/// A stereo delay whose time follows the session tempo.
struct Delay {
    left: Vec<f32>,
    right: Vec<f32>,
    at: usize,
    /// Feedback lowpass state, one per channel.
    store: [f32; 2],
    /// Current delay in samples, smoothed towards the target.
    len: [f32; 2],
    sample_rate: f32,
}

/// How fast the delay time slews towards a new tempo, per sample.
///
/// A tempo change while notes are ringing must not be a click. Slow enough to
/// be a tape machine spinning up, fast enough to arrive within a bar.
const DELAY_SLEW: f32 = 0.00002;

impl Delay {
    fn new(sample_rate: f32) -> Self {
        let cap = (sample_rate * MAX_DELAY_SECS) as usize + 2;
        Self {
            left: vec![0.0; cap],
            right: vec![0.0; cap],
            at: 0,
            store: [0.0; 2],
            len: [sample_rate * FREE_SECS; 2],
            sample_rate,
        }
    }

    /// Where the delay time is heading, in samples, from the tempo.
    fn target(&self, bpm: f64, p: &Params) -> [f32; 2] {
        let secs = match p.delay_division.beats() {
            Some(beats) if bpm.is_finite() && bpm > 1.0 => (60.0 / bpm) as f32 * beats,
            _ => FREE_SECS,
        };
        let cap = (self.left.len() - 2) as f32;
        let l = (secs * self.sample_rate).clamp(1.0, cap);
        // Width closes the two channels together rather than opening them
        // apart: at 0 they are the same length, which is a mono delay.
        let ratio = 1.0 - (1.0 - DELAY_RIGHT_RATIO) * p.delay_width;
        [l, (l * ratio).clamp(1.0, cap)]
    }

    /// Read `len` samples back, interpolating, so a slewing time glides.
    ///
    /// Associated rather than a method: the chorus reads its own buffer the
    /// same way, and two implementations of "read backwards with a fraction"
    /// is two chances to get the wraparound wrong.
    #[inline]
    fn tap(buf: &[f32], at: usize, len: f32) -> f32 {
        let n = buf.len();
        let whole = len as usize;
        let frac = len - whole as f32;
        // `+ n` before the subtraction: `at` is smaller than `whole` for the
        // first pass through the buffer, and this is unsigned.
        let a = (at + n - whole.min(n - 1)) % n;
        let b = (a + n - 1) % n;
        buf[a] * (1.0 - frac) + buf[b] * frac
    }

    #[inline]
    fn process(&mut self, input: [f32; 2], target: [f32; 2], p: &Params) -> [f32; 2] {
        let feedback = DELAY_FEEDBACK.0 + (DELAY_FEEDBACK.1 - DELAY_FEEDBACK.0) * p.delay_feedback;
        // Inverted: tone UP is less damping, which is what a tone control does.
        let damp = DELAY_DAMP.1 - (DELAY_DAMP.1 - DELAY_DAMP.0) * p.delay_tone;
        let mut out = [0.0_f32; 2];
        for ch in 0..2 {
            self.len[ch] += (target[ch] - self.len[ch]).clamp(
                -DELAY_SLEW * self.sample_rate,
                DELAY_SLEW * self.sample_rate,
            );
            let buf = if ch == 0 { &self.left } else { &self.right };
            let wet = Self::tap(buf, self.at, self.len[ch]);
            self.store[ch] = flush(wet * (1.0 - damp) + self.store[ch] * damp);
            let write = flush(input[ch] + self.store[ch] * feedback);
            if ch == 0 {
                self.left[self.at] = write;
            } else {
                self.right[self.at] = write;
            }
            out[ch] = wet;
        }
        self.at = (self.at + 1) % self.left.len();
        out
    }

    fn clear(&mut self) {
        self.left.fill(0.0);
        self.right.fill(0.0);
        self.store = [0.0; 2];
    }
}

// ── chorus ──────────────────────────────────────────────────────────────────

/// The sweep's slowest and fastest, in Hz.
///
/// The CE-1's rate control covers roughly this. The bottom is a swell you feel
/// rather than hear; the top is where chorus becomes vibrato and then becomes
/// seasick.
const CHORUS_RATE_HZ: (f32, f32) = (0.12, 3.2);

/// The delay the sweep moves through, in milliseconds.
///
/// A bucket brigade at these clock rates is a few milliseconds long. Under
/// about 4ms it is a flanger; over about 12ms the pitch wobble becomes an
/// audible detune rather than a shimmer.
const CHORUS_MS: (f32, f32) = (4.5, 11.5);

/// The bucket brigade's own bandwidth, in Hz, at each end of the tone control.
///
/// **A BBD is dark, and that is most of what a CE-1 sounds like.** The chips
/// clock at a few tens of kHz and every stage loses treble; a chorus that
/// returns the top end intact sounds like a plugin, not like the pedal.
const CHORUS_LP_HZ: (f32, f32) = (2_600.0, 9_000.0);

/// A stereo chorus after the Boss CE-1.
///
/// # The stereo
///
/// **One delay line, two outputs, and the right one is inverted.** That is the
/// CE-1's actual circuit and the reason it is remembered: the wet signal is
/// added to one output and subtracted from the other, which puts the modulated
/// copy in opposite polarity across the pair. The result is enormously wide
/// from a single voice — much wider than two independent LFOs get you.
///
/// It also means the wet CANCELS if somebody sums the two channels to mono,
/// which is exactly what the hardware does and is the reason `chorus_width` is
/// a control rather than a constant: at 0 both channels get the wet in phase,
/// which is narrow and mono-safe, and at 1 it is the pedal.
///
/// # The sweep
///
/// Triangle, not sine. The CE-1's LFO is a charging capacitor and the sweep
/// is close to linear between its turning points, which is why the pitch shift
/// sits at a steady value and then reverses rather than easing continuously.
struct Chorus {
    buf: [Vec<f32>; 2],
    at: usize,
    /// The LFO's position, 0..1, wrapping.
    phase: f32,
    /// One-pole lowpass state for the BBD bandwidth, per channel.
    lp: [f32; 2],
    sample_rate: f32,
}

impl Chorus {
    fn new(sample_rate: f32) -> Self {
        // Room for the longest delay the depth can reach, and a little over.
        let cap = (sample_rate * CHORUS_MS.1 * 2.0 / 1000.0) as usize + 4;
        Self {
            buf: [vec![0.0; cap], vec![0.0; cap]],
            at: 0,
            phase: 0.0,
            lp: [0.0; 2],
            sample_rate,
        }
    }

    /// The triangle, as -1..=1.
    #[inline]
    fn triangle(phase: f32) -> f32 {
        // Up for the first half, down for the second.
        if phase < 0.5 {
            phase * 4.0 - 1.0
        } else {
            3.0 - phase * 4.0
        }
    }

    #[inline]
    fn process(&mut self, input: [f32; 2], p: &Params) -> [f32; 2] {
        let rate = CHORUS_RATE_HZ.0 + (CHORUS_RATE_HZ.1 - CHORUS_RATE_HZ.0) * p.chorus_rate;
        self.phase = (self.phase + rate / self.sample_rate).fract();

        // The sweep, in samples. Centred in the range so depth opens outward
        // from the middle rather than from the short end — a depth of zero is
        // then a fixed short delay, which is a comb filter and not a wobble.
        let (lo, hi) = (
            CHORUS_MS.0 * self.sample_rate / 1000.0,
            CHORUS_MS.1 * self.sample_rate / 1000.0,
        );
        let centre = (lo + hi) * 0.5;
        let swing = (hi - lo) * 0.5 * p.chorus_depth;
        let len = centre + Self::triangle(self.phase) * swing;

        // The BBD's bandwidth, as a one-pole coefficient.
        let cutoff = CHORUS_LP_HZ.0 + (CHORUS_LP_HZ.1 - CHORUS_LP_HZ.0) * p.chorus_tone;
        let a = (-std::f32::consts::TAU * cutoff / self.sample_rate).exp();

        let mut wet = [0.0_f32; 2];
        for ch in 0..2 {
            let tapped = Delay::tap(&self.buf[ch], self.at, len);
            self.lp[ch] = flush(tapped * (1.0 - a) + self.lp[ch] * a);
            wet[ch] = self.lp[ch];
            self.buf[ch][self.at] = flush(input[ch]);
        }
        self.at = (self.at + 1) % self.buf[0].len();

        // **The inversion.** At width 1 the right channel gets the wet in
        // opposite polarity, which is the CE-1. At 0 both get it the same way,
        // which is narrow and survives a mono fold.
        let right_sign = 1.0 - 2.0 * p.chorus_width;
        [wet[0], wet[1] * right_sign]
    }

    fn clear(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.lp = [0.0; 2];
    }
}

// ── the pair ────────────────────────────────────────────────────────────────

/// Below this a knob is off, and off means the whole effect is skipped.
///
/// **Not just multiplied by zero.** A reverb at zero mix still runs eight combs
/// per channel per sample, and both are off by default: the common case has to
/// cost nothing, or every user pays for a feature most of them never turn on.
const OFF: f32 = 0.0005;

/// How fast a knob's value reaches the audio, per sample.
///
/// The knob is read once a block and applied per sample through this, because
/// stepping a mix coefficient at a block boundary is a click, and dragging a
/// knob is a few hundred block boundaries.
const KNOB_SLEW: f32 = 0.0004;

/// How much of each effect, straight off the three knobs.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sends {
    pub reverb: f32,
    pub delay: f32,
    pub chorus: f32,
}

/// The three effects on the instrument bus.
pub struct Effects {
    reverb: [ReverbChannel; 2],
    delay: Delay,
    chorus: Chorus,
    /// Where the wet mixes actually are, chasing where the knobs say.
    mix: Sends,
    /// The width the reverb's BUFFER LENGTHS were built from.
    ///
    /// The one parameter that cannot be swept: it is a delay length, so
    /// changing it means new buffers. Compared each block so a menu change
    /// takes effect, against a value only a person can move.
    built_width: f32,
    sample_rate: f32,
    /// Everything was off last block, so the tails are already flushed and do
    /// not need flushing again.
    idle: bool,
}

impl Effects {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8_000.0);
        let p = Params::default();
        Self {
            reverb: Self::build_reverb(sr, p.reverb_width),
            delay: Delay::new(sr),
            chorus: Chorus::new(sr),
            mix: Sends::default(),
            built_width: p.reverb_width,
            sample_rate: sr,
            idle: true,
        }
    }

    /// The two reverb channels, the right one offset by `width`.
    fn build_reverb(sample_rate: f32, width: f32) -> [ReverbChannel; 2] {
        let spread = (STEREO_SPREAD as f32 * width.clamp(0.0, 1.0)) as usize;
        [
            ReverbChannel::new(sample_rate, 0),
            ReverbChannel::new(sample_rate, spread),
        ]
    }

    /// Apply both, in place, to an interleaved buffer.
    ///
    /// `reverb` and `delay` are 0..=1 straight off the knobs. `bpm` sets the
    /// delay time. `channels` is the buffer's stride; anything past the first
    /// two is left alone, because these are stereo effects and a third channel
    /// is not a thing to guess about.
    pub fn process(
        &mut self,
        buf: &mut [f32],
        frames: usize,
        channels: usize,
        sends: Sends,
        p: &Params,
        bpm: f64,
    ) {
        let want = Sends {
            reverb: sends.reverb.clamp(0.0, 1.0),
            delay: sends.delay.clamp(0.0, 1.0),
            chorus: sends.chorus.clamp(0.0, 1.0),
        };
        // Every knob at rest AND every mix arrived there: nothing to do, and
        // nothing ringing that would be cut off by not doing it.
        let quiet = |m: &Sends| m.reverb < OFF && m.delay < OFF && m.chorus < OFF;
        if quiet(&want) && quiet(&self.mix) {
            if !self.idle {
                // Once, on the way down: whatever was still ringing when the
                // knob reached zero must not be waiting inside the buffers to
                // be heard when it is turned up again half an hour later.
                self.clear();
                self.idle = true;
            }
            return;
        }
        self.idle = false;
        if channels < 2 {
            return;
        }
        // The one parameter that is a buffer length rather than a coefficient.
        // Rebuilt when a person moves it, which is never in audio terms.
        if (p.reverb_width - self.built_width).abs() > 1.0e-4 {
            self.reverb = Self::build_reverb(self.sample_rate, p.reverb_width);
            self.built_width = p.reverb_width;
        }
        let target = self.delay.target(bpm, p);
        let comb_fb = COMB_FEEDBACK.0 + (COMB_FEEDBACK.1 - COMB_FEEDBACK.0) * p.reverb_size;
        let comb_damp = COMB_DAMP.0 + (COMB_DAMP.1 - COMB_DAMP.0) * p.reverb_damp;
        // See `REVERB_TRIM_AT`: size changes how long, not how loud.
        //
        // **The square root, and it matters which.** A comb's impulse response
        // is `fb^n`, so its ENERGY is `1/(1 - fb²)` and its amplitude goes as
        // the root of that. Normalising by `1 - fb` instead — the steady-state
        // gain — overshoots badly: it made the smallest room four times louder
        // than the largest, because a small room's level is its early
        // reflections and those barely depend on the feedback at all.
        let voiced_fb = COMB_FEEDBACK.0 + (COMB_FEEDBACK.1 - COMB_FEEDBACK.0) * REVERB_TRIM_AT;
        let energy = |fb: f32| (1.0 - fb * fb).max(1.0e-6).sqrt();
        let trim = REVERB_TRIM * energy(comb_fb) / energy(voiced_fb);

        for f in 0..frames {
            let at = f * channels;
            let dry = [buf[at], buf[at + 1]];

            self.mix.reverb += (want.reverb - self.mix.reverb) * KNOB_SLEW;
            self.mix.delay += (want.delay - self.mix.delay) * KNOB_SLEW;
            self.mix.chorus += (want.chorus - self.mix.chorus) * KNOB_SLEW;

            // **Chorus, then delay, then reverb** — the order a pedalboard
            // runs and a desk's sends imply. A chorus AFTER the reverb smears
            // the tail into mush; the delay repeating an already chorused note
            // is what makes the repeats shimmer.
            let wet_c = self.chorus.process(dry, p);
            let chorused = [
                dry[0] + wet_c[0] * self.mix.chorus,
                dry[1] + wet_c[1] * self.mix.chorus,
            ];
            let wet_d = self.delay.process(chorused, target, p);
            let into_verb = [
                chorused[0] + wet_d[0] * self.mix.delay,
                chorused[1] + wet_d[1] * self.mix.delay,
            ];
            let wet_r = [
                self.reverb[0].process(into_verb[0], comb_fb, comb_damp) * trim,
                self.reverb[1].process(into_verb[1], comb_fb, comb_damp) * trim,
            ];
            for ch in 0..2 {
                buf[at + ch] =
                    chorused[ch] + wet_d[ch] * self.mix.delay + wet_r[ch] * self.mix.reverb;
            }
        }
    }

    /// Drop every tail. Called when every knob reaches zero, and on a stop.
    pub fn clear(&mut self) {
        for r in &mut self.reverb {
            r.clear();
        }
        self.delay.clear();
        self.chorus.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// The three sends, as one.
    fn sends(reverb: f32, delay: f32, chorus: f32) -> Sends {
        Sends {
            reverb,
            delay,
            chorus,
        }
    }

    /// One impulse in, `frames` of output, at 120 bpm with default settings.
    fn strike(fx: &mut Effects, frames: usize, s: Sends) -> Vec<f32> {
        strike_with(fx, frames, s, &Params::default(), 120.0)
    }

    /// As [`strike`], at a stated tempo and settings. **The tempo has to be the
    /// same one the warm-up used**: the delay time slews, so a different bpm
    /// here starts it moving back and the repeat arrives somewhere between.
    fn strike_with(
        fx: &mut Effects,
        frames: usize,
        s: Sends,
        p: &Params,
        bpm: f64,
    ) -> Vec<f32> {
        let mut out = vec![0.0_f32; frames * 2];
        out[0] = 1.0;
        out[1] = 1.0;
        fx.process(&mut out, frames, 2, s, p, bpm);
        out
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0_f32, |m, x| m.max(x.abs()))
    }

    /// **Off has to be silent, and it has to be free.** The default state of
    /// all three knobs, and if a buffer comes back changed with every one at
    /// zero then every user is paying for effects they did not ask for.
    #[test]
    fn every_knob_at_zero_does_not_touch_the_signal() {
        let mut fx = Effects::new(SR);
        let mut buf: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let before = buf.clone();
        fx.process(&mut buf, 256, 2, Sends::default(), &Params::default(), 120.0);
        assert_eq!(buf, before, "an effect at zero changed the signal");
    }

    /// Turning reverb up puts energy where there was none, and it decays.
    #[test]
    fn reverb_makes_a_tail_that_dies() {
        let mut fx = Effects::new(SR);
        let out = strike(&mut fx, 24_000, sends(1.0, 0.0, 0.0));
        // Well past the impulse, so this is the room and not the input.
        let early = peak(&out[2_000 * 2..6_000 * 2]);
        let late = peak(&out[18_000 * 2..]);
        assert!(early > 1.0e-4, "there was no tail at all: {early}");
        assert!(late < early, "the tail is not decaying: {early} -> {late}");

        // Stereo: the two channels must not be the same signal, or the spread
        // is doing nothing and the reverb is mono.
        let (l, r): (Vec<f32>, Vec<f32>) = out
            .chunks_exact(2)
            .skip(3_000)
            .map(|f| (f[0], f[1]))
            .unzip();
        let diff = l
            .iter()
            .zip(&r)
            .fold(0.0_f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff > 1.0e-5, "both channels are identical: the reverb is mono");
    }

    /// **Size is a real control.** A bigger room rings longer, or the menu is
    /// a row of numbers that do nothing.
    #[test]
    fn a_bigger_room_rings_longer() {
        let tail = |size: f32| {
            let mut fx = Effects::new(SR);
            let p = Params {
                reverb_size: size,
                ..Params::default()
            };
            let out = strike_with(&mut fx, 48_000, sends(1.0, 0.0, 0.0), &p, 120.0);
            peak(&out[36_000 * 2..])
        };
        let (small, large) = (tail(0.0), tail(1.0));
        assert!(
            large > small * 4.0,
            "a room at full size rings {large:e} against {small:e} at none"
        );
    }

    /// **Size changes how long, not how loud.** A comb's gain rises with its
    /// feedback, so an unnormalised size control is also a volume control —
    /// and somebody reaching for a longer tail turns the send back down to
    /// compensate, leaving two controls fighting over one thing.
    #[test]
    fn size_does_not_change_how_loud_the_reverb_is() {
        // **Energy over the whole tail**, not the peak just after the strike.
        // A bigger hall spreads the same energy over more time, so its
        // moment-to-moment level is genuinely lower and a peak measurement
        // would be asking for the wrong thing to be equal.
        let energy = |size: f32| {
            let mut fx = Effects::new(SR);
            let p = Params {
                reverb_size: size,
                ..Params::default()
            };
            let out = strike_with(&mut fx, 96_000, sends(1.0, 0.0, 0.0), &p, 120.0);
            (out.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>()).sqrt()
        };
        let (small, large) = (energy(0.0), energy(1.0));
        assert!(
            large < small * 2.0 && small < large * 2.0,
            "the smallest room carries {small:e} and the largest {large:e}: \
             size is a volume control"
        );
    }

    /// **The delay lands on the beat**, and the division says where. The whole
    /// reason the time is not a knob: at 120 bpm a dotted eighth is 375 ms, and
    /// a repeat anywhere else is a mistake somebody has to hear to find.
    #[test]
    fn the_delay_follows_the_tempo_and_the_division() {
        let cases = [
            (120.0_f64, Division::DottedEighth, 0.375_f32),
            (90.0, Division::DottedEighth, 0.5),
            (120.0, Division::Quarter, 0.5),
            (120.0, Division::Eighth, 0.25),
            (120.0, Division::Sixteenth, 0.125),
            // Free ignores the tempo entirely, which is the point of it.
            (200.0, Division::Free, FREE_SECS),
        ];
        for (bpm, division, want) in cases {
            let p = Params {
                delay_division: division,
                ..Params::default()
            };
            let mut fx = Effects::new(SR);
            // Long enough for the slew to arrive before the impulse is sent.
            let mut warm = vec![0.0_f32; 2 * 96_000];
            fx.process(&mut warm, 96_000, 2, sends(0.0, 1.0, 0.0), &p, bpm);

            let out = strike_with(&mut fx, 48_000, sends(0.0, 1.0, 0.0), &p, bpm);
            // The loudest thing after the impulse itself is the first repeat.
            let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0].abs()).collect();
            let (at, _) = left
                .iter()
                .enumerate()
                .skip(64)
                .fold((0usize, 0.0_f32), |(bi, bv), (i, v)| {
                    if *v > bv {
                        (i, *v)
                    } else {
                        (bi, bv)
                    }
                });
            let got = at as f32 / SR;
            assert!(
                (got - want).abs() < 0.02,
                "{} at {bpm} bpm landed at {got:.3}s, not {want:.3}s",
                division.label()
            );
        }
    }

    /// Repeats get quieter, or the knob is an oscillator — at every feedback
    /// setting the menu can reach.
    #[test]
    fn the_delay_does_not_run_away_at_any_setting() {
        for feedback in [0.0_f32, 0.5, 1.0] {
            let p = Params {
                delay_feedback: feedback,
                ..Params::default()
            };
            let mut fx = Effects::new(SR);
            let out = strike_with(&mut fx, 48_000 * 6, sends(0.0, 1.0, 0.0), &p, 120.0);
            let first = peak(&out[..48_000]);
            let last = peak(&out[48_000 * 10..]);
            assert!(
                last < first * 0.35,
                "feedback {feedback} went {first:e} -> {last:e} after five seconds"
            );
        }
    }

    /// **The chorus is wide, and the width control is what makes it wide.**
    /// The CE-1's trick is one delay line inverted into the right channel; at
    /// full width the two outputs must be opposite, and at zero they must not.
    #[test]
    fn the_chorus_inverts_one_channel_at_full_width() {
        let difference = |width: f32| {
            let mut fx = Effects::new(SR);
            let p = Params {
                chorus_width: width,
                ..Params::default()
            };
            // A tone rather than an impulse: a chorus is heard on sustained
            // sound and an impulse leaves the delay line almost empty.
            let frames = 24_000;
            let mut buf = vec![0.0_f32; frames * 2];
            for (i, f) in buf.chunks_exact_mut(2).enumerate() {
                let v = (i as f32 * 220.0 * std::f32::consts::TAU / SR).sin() * 0.5;
                f[0] = v;
                f[1] = v;
            }
            fx.process(&mut buf, frames, 2, sends(0.0, 0.0, 1.0), &p, 120.0);
            // How far the two channels have been pushed apart. The input was
            // identical in both, so all of this is the effect.
            buf.chunks_exact(2)
                .skip(12_000)
                .fold(0.0_f32, |m, f| m.max((f[0] - f[1]).abs()))
        };
        let (narrow, wide) = (difference(0.0), difference(1.0));
        assert!(
            narrow < 1.0e-6,
            "at zero width the channels differ by {narrow}, so it is not mono-safe"
        );
        assert!(
            wide > 0.05,
            "at full width the channels differ by only {wide}, so the inversion is not happening"
        );
    }

    /// A chorus has to MOVE. A fixed delay is a comb filter, which is a tone
    /// change and not a chorus, and it is what a broken LFO sounds like.
    #[test]
    fn the_chorus_sweeps() {
        let mut fx = Effects::new(SR);
        let p = Params {
            chorus_rate: 1.0,
            chorus_depth: 1.0,
            ..Params::default()
        };
        let frames = 48_000;
        let mut buf = vec![0.0_f32; frames * 2];
        for (i, f) in buf.chunks_exact_mut(2).enumerate() {
            let v = (i as f32 * 440.0 * std::f32::consts::TAU / SR).sin() * 0.5;
            f[0] = v;
            f[1] = v;
        }
        fx.process(&mut buf, frames, 2, sends(0.0, 0.0, 1.0), &p, 120.0);
        let out: Vec<f32> = buf.chunks_exact(2).map(|f| f[0]).collect();

        // **The OUTPUT's envelope**, not the wet signal's amplitude. The wet is
        // a delayed copy of the same sine and its amplitude never changes,
        // whatever the sweep is doing; what moves is where dry and wet cancel.
        // A stuck sweep is a fixed comb, and a fixed comb has a fixed envelope.
        //
        // **Short windows.** Three periods of the tone, not thirty: at full
        // rate the sweep carries the comb through most of a cycle inside a long
        // window, so every long window finds a constructive moment and the
        // envelope looks flat when it is anything but.
        let mut lo = f32::MAX;
        let mut hi = 0.0_f32;
        for w in (8_000..44_000).step_by(300) {
            let d = peak(&out[w..w + 300]);
            lo = lo.min(d);
            hi = hi.max(d);
        }
        assert!(
            hi > lo * 1.4,
            "the sweep is not moving: the envelope sat between {lo:e} and {hi:e}"
        );
    }

    /// **Nothing rings on from an hour ago.** Turning the knobs down flushes
    /// the tails, so turning them back up starts from silence rather than
    /// replaying whatever was in the buffers.
    #[test]
    fn turning_everything_off_flushes_what_was_ringing() {
        let mut fx = Effects::new(SR);
        strike(&mut fx, 8_000, sends(1.0, 1.0, 1.0));
        // Down, and long enough for the mixes to slew to zero.
        let mut off = vec![0.0_f32; 2 * 48_000 * 2];
        fx.process(
            &mut off,
            48_000 * 2,
            2,
            Sends::default(),
            &Params::default(),
            120.0,
        );
        // Up again, into silence.
        let mut back = vec![0.0_f32; 2 * 4_000];
        fx.process(
            &mut back,
            4_000,
            2,
            sends(1.0, 1.0, 1.0),
            &Params::default(),
            120.0,
        );
        assert!(
            peak(&back) < 1.0e-6,
            "an old tail came back when the knob did: {}",
            peak(&back)
        );
    }

    /// A room at 96 kHz is the same room, not one half as long.
    #[test]
    fn the_room_is_the_same_size_at_every_sample_rate() {
        let tail_secs = |sr: f32| {
            let mut fx = Effects::new(sr);
            let frames = (sr * 2.0) as usize;
            let out = strike(&mut fx, frames, sends(1.0, 0.0, 0.0));
            let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0].abs()).collect();
            let peak = left.iter().skip(1000).fold(0.0_f32, |m, v| m.max(*v));
            // Where it falls to a thousandth of its loudest.
            left.iter().rposition(|v| *v > peak * 0.001).unwrap_or(0) as f32 / sr
        };
        let (a, b) = (tail_secs(44_100.0), tail_secs(96_000.0));
        assert!(
            (a - b).abs() < a * 0.2,
            "the tail is {a:.2}s at 44.1k and {b:.2}s at 96k"
        );
    }

    /// Mono and odd channel counts are left alone rather than half-processed.
    #[test]
    fn a_mono_buffer_is_not_mangled() {
        let mut fx = Effects::new(SR);
        let mut buf: Vec<f32> = (0..256).map(|i| (i as f32 * 0.02).sin()).collect();
        let before = buf.clone();
        fx.process(
            &mut buf,
            256,
            1,
            sends(1.0, 1.0, 1.0),
            &Params::default(),
            120.0,
        );
        assert_eq!(buf, before, "a mono buffer was written to");
    }

    /// **Nothing a menu can reach may blow up.** Every parameter at both
    /// extremes, all three sends open, against a signal loud enough to be
    /// unkind: the output has to stay finite and bounded.
    #[test]
    fn no_setting_a_menu_can_reach_makes_it_explode() {
        let extremes = [0.0_f32, 1.0];
        for a in extremes {
            for b in extremes {
                for division in Division::ALL {
                    let p = Params {
                        reverb_size: a,
                        reverb_damp: b,
                        reverb_width: a,
                        delay_division: division,
                        delay_feedback: a,
                        delay_tone: b,
                        delay_width: b,
                        chorus_rate: a,
                        chorus_depth: b,
                        chorus_width: a,
                        chorus_tone: b,
                    };
                    let mut fx = Effects::new(SR);
                    let frames = 24_000;
                    // **Fresh input each block**, as the renderer hands it
                    // over. Re-processing the same buffer would be a feedback
                    // loop of the test's own making, and would fail for a
                    // reason that says nothing about the effects.
                    let mut phase = 0.0_f32;
                    for _ in 0..8 {
                        let mut buf = vec![0.0_f32; frames * 2];
                        for f in buf.chunks_exact_mut(2) {
                            phase += 110.0 * std::f32::consts::TAU / SR;
                            let v = phase.sin();
                            f[0] = v;
                            f[1] = -v;
                        }
                        fx.process(&mut buf, frames, 2, sends(1.0, 1.0, 1.0), &p, 120.0);
                        assert!(
                            buf.iter().all(|s| s.is_finite()),
                            "{} produced a non-finite sample",
                            division.label()
                        );
                        // Generous, because everything open on a full-scale
                        // input is not a mix anybody would use. What this
                        // catches is a runaway, not a hot setting.
                        assert!(
                            peak(&buf) < 12.0,
                            "{} reached {} with everything open",
                            division.label(),
                            peak(&buf)
                        );
                    }
                }
            }
        }
    }

    /// Rubbish in the settings file does not become rubbish in a feedback loop.
    #[test]
    fn parameters_are_clamped_on_the_way_in() {
        let p = Params {
            reverb_size: f32::NAN,
            delay_feedback: 40.0,
            chorus_depth: -3.0,
            ..Params::default()
        }
        .sane();
        assert!(p.reverb_size.is_finite());
        assert!((0.0..=1.0).contains(&p.delay_feedback));
        assert!((0.0..=1.0).contains(&p.chorus_depth));
    }

    /// Every division survives a trip through the settings file.
    #[test]
    fn a_division_round_trips_through_its_key() {
        for d in Division::ALL {
            assert_eq!(Division::from_key(d.key()), Some(d), "{}", d.label());
            assert!(!d.label().is_empty());
        }
        assert_eq!(Division::from_key("nonsense"), None);
    }
}
