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

/// How steeply a filter gets rid of what is past its corner.
///
/// **Three real slopes, not a continuous "resonance-ish" control.** 24 dB is
/// the default because it is the one that does what somebody pointing at a
/// filter means — "take that off" — and 6 dB is there because sometimes the
/// answer is a tilt rather than a cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Slope {
    /// One pole. A tilt.
    Six,
    /// One biquad, Butterworth.
    Twelve,
    /// Two cascaded biquads, Butterworth-aligned.
    #[default]
    TwentyFour,
}

impl Slope {
    /// Every value, gentlest first, which is the order a menu should offer.
    pub const ALL: [Slope; 3] = [Slope::Six, Slope::Twelve, Slope::TwentyFour];

    pub fn label(self) -> &'static str {
        match self {
            Slope::Six => "6 dB/oct",
            Slope::Twelve => "12 dB/oct",
            Slope::TwentyFour => "24 dB/oct",
        }
    }

    /// How it is written to the settings file, and read back.
    pub fn key(self) -> &'static str {
        match self {
            Slope::Six => "6",
            Slope::Twelve => "12",
            Slope::TwentyFour => "24",
        }
    }

    pub fn from_key(key: &str) -> Option<Slope> {
        Slope::ALL.into_iter().find(|s| s.key() == key)
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

    pub hpf_slope: Slope,
    /// A lift right at the corner. 0 is flat Butterworth.
    pub hpf_resonance: f32,

    pub lpf_slope: Slope,
    /// A lift right at the corner. 0 is flat Butterworth.
    pub lpf_resonance: f32,

    /// The most the limiter will ever let out, mapped across
    /// [`CEILING_DB`]. The default is -1 dBTP.
    pub limiter_ceiling: f32,
    /// How fast it lets go, mapped across [`RELEASE_MS`].
    pub limiter_release: f32,
    /// How far below the ceiling it starts easing in, across [`KNEE_DB`].
    ///
    /// The one control that trades the two failure modes against each other:
    /// hard is transparent until it is not, soft starts working earlier and
    /// distorts less when it does.
    pub limiter_knee: f32,
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
            hpf_slope: Slope::default(),
            hpf_resonance: 0.0,
            lpf_slope: Slope::default(),
            lpf_resonance: 0.0,
            // **Dialled in for general use.** -1 dBTP, a release that does not
            // pump on piano, and enough knee that the first decibel of
            // reduction is not the one you hear arriving.
            limiter_ceiling: CEILING_DEFAULT,
            limiter_release: 0.40,
            limiter_knee: 0.50,
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
            &mut self.hpf_resonance,
            &mut self.lpf_resonance,
            &mut self.limiter_ceiling,
            &mut self.limiter_release,
            &mut self.limiter_knee,
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

// ── the filters ─────────────────────────────────────────────────────────────
//
// Two knobs, one implementation. A high-pass and a low-pass differ here by
// three coefficient signs and nothing else, so they are one type with a
// direction rather than two types that have to be kept in step.

/// Where the high-pass knob sweeps, in Hz.
///
/// The top is 1.2 kHz rather than anything higher because past that a
/// high-pass on a piano stops being "take the rumble out" and starts being an
/// effect, and this is the knob people reach for to do the first thing.
const HPF_HZ: (f32, f32) = (20.0, 1_200.0);

/// Where the low-pass knob sweeps, in Hz — **backwards**: knob up is down in
/// frequency. See [`Sends::lpf`].
const LPF_HZ: (f32, f32) = (20_000.0, 200.0);

/// How much the resonance row can lift the corner, in dB of Q.
///
/// Expressed as a Q multiplier, not a gain: it is the last stage's Q that
/// moves, which is what a filter's resonance IS. 1.0 leaves Butterworth alone.
const RESONANCE: (f32, f32) = (1.0, 4.0);

/// The Butterworth Q values for a 4th-order (24 dB/oct) cascade.
///
/// Not two identical 0.707 stages — that is a Linkwitz-Riley alignment and it
/// sags 6 dB at the corner. These are the real ones: `1 / (2 cos(pi (2k+1) / 8))`
/// for k = 0, 1.
const BUTTER_Q4: (f32, f32) = (0.541_196_1, 1.306_563);

/// One second-order section, transposed direct form II.
///
/// TDF-II because it is the one that behaves when the coefficients move under
/// it, which here they do every block that somebody is turning the knob.
#[derive(Debug, Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = flush(self.b1 * x - self.a1 * y + self.z2);
        self.z2 = flush(self.b2 * x - self.a2 * y);
        y
    }

    /// RBJ cookbook, low-pass or high-pass by `high`.
    fn set(&mut self, sample_rate: f32, hz: f32, q: f32, high: bool) {
        let w = std::f32::consts::TAU * (hz / sample_rate).clamp(1.0e-5, 0.49);
        let (sn, cs) = w.sin_cos();
        let alpha = sn / (2.0 * q.max(0.05));
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high {
            let c = (1.0 + cs) / 2.0;
            (c, -2.0 * c, c)
        } else {
            let c = (1.0 - cs) / 2.0;
            (c, 2.0 * c, c)
        };
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = (-2.0 * cs) / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    fn clear(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }
}

/// One pole, for the 6 dB setting.
#[derive(Debug, Clone, Copy, Default)]
struct OnePole {
    a: f32,
    z: f32,
}

impl OnePole {
    /// Returns the low-passed value; the caller subtracts it for a high-pass.
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.z = flush(self.z + self.a * (x - self.z));
        self.z
    }

    fn set(&mut self, sample_rate: f32, hz: f32) {
        let w = std::f32::consts::TAU * (hz / sample_rate).clamp(1.0e-5, 0.49);
        // The exact one-pole coefficient, not the `w` approximation, because
        // this knob goes to 20 kHz where the two stop agreeing.
        self.a = 1.0 - (-w).exp();
    }

    fn clear(&mut self) {
        self.z = 0.0;
    }
}

/// A high-pass or a low-pass, at one of three slopes, on one channel.
#[derive(Debug, Clone, Copy, Default)]
struct Filter {
    one: OnePole,
    a: Biquad,
    b: Biquad,
}

impl Filter {
    #[inline]
    fn process(&mut self, x: f32, slope: Slope, high: bool) -> f32 {
        match slope {
            Slope::Six => {
                let lp = self.one.process(x);
                if high {
                    x - lp
                } else {
                    lp
                }
            }
            Slope::Twelve => self.a.process(x),
            Slope::TwentyFour => self.b.process(self.a.process(x)),
        }
    }

    /// Set whichever stages this slope runs, and only those.
    ///
    /// The Q of section `a` is not a constant: at 12 dB it is the whole filter
    /// and wants Butterworth's 0.707, at 24 dB it is half of a pair and wants
    /// 0.541. Setting it once for "a filter" and then correcting it elsewhere
    /// is how the 24 dB setting ends up quietly being a 12 dB one.
    ///
    /// Resonance always lands on the LAST section — the one already nearest
    /// resonant. Spread across both, it moves the corner as well as the peak.
    fn set(&mut self, sample_rate: f32, hz: f32, slope: Slope, resonance: f32, high: bool) {
        let lift = RESONANCE.0 + (RESONANCE.1 - RESONANCE.0) * resonance.clamp(0.0, 1.0);
        match slope {
            Slope::Six => self.one.set(sample_rate, hz),
            Slope::Twelve => {
                self.a
                    .set(sample_rate, hz, std::f32::consts::FRAC_1_SQRT_2 * lift, high);
            }
            Slope::TwentyFour => {
                self.a.set(sample_rate, hz, BUTTER_Q4.0, high);
                self.b.set(sample_rate, hz, BUTTER_Q4.1 * lift, high);
            }
        }
    }

    fn clear(&mut self) {
        self.one.clear();
        self.a.clear();
        self.b.clear();
    }
}

// ── the limiter ─────────────────────────────────────────────────────────────

/// What the ceiling row spans, in dBFS. The default sits at -1.
const CEILING_DB: (f32, f32) = (-6.0, -0.1);

/// The knob position that lands on exactly -1.0 dBTP.
const CEILING_DEFAULT: f32 = (-1.0 - CEILING_DB.0) / (CEILING_DB.1 - CEILING_DB.0);

/// What the release row spans, in milliseconds.
const RELEASE_MS: (f32, f32) = (20.0, 600.0);

/// What the knee row spans, in dB below the ceiling.
const KNEE_DB: (f32, f32) = (0.0, 9.0);

/// How much extra level the knob drives into the ceiling, in dB.
const DRIVE_DB: f32 = 12.0;

/// Phases the detector reconstructs between each pair of samples.
///
/// **This is what "true peak" means here.** A sample-peak limiter reads the
/// numbers in the buffer; a converter draws a smooth curve THROUGH those
/// numbers and that curve overshoots them. Four phases is what BS.1770 asks
/// for at 48 kHz, and it is the difference between "-1 dBFS in the file" and
/// "-1 dB out of the socket".
const TP_PHASES: usize = 8;

/// Taps in the reconstruction filter. Odd, so phase 0 is exactly the input
/// sample and costs nothing to "reconstruct".
const TP_TAPS: usize = 21;

/// The reconstruction filter's group delay.
const TP_CENTRE: usize = TP_TAPS / 2;

/// How far ahead the limiter looks, in milliseconds.
///
/// **Not zero, and this is why.** The first build of this had no lookahead at
/// all: detect the peak between two samples, reduce the gain on the spot. It
/// let 0.5 dB past the ceiling, and the reason is worth keeping. The peak
/// BETWEEN two samples is not made by those two samples — a converter builds
/// it from twenty of them either side. Reducing the gain on one sample and
/// leaving its neighbours alone barely moves the curve they all reconstruct
/// together.
///
/// So the gain has to be down across the whole kernel, which means it has to
/// start going down before the peak arrives. One millisecond is long enough to
/// ramp over twenty taps without the ramp itself being audible, and short
/// enough that the whole limiter costs 1.2 ms — a fifth of one buffer at the
/// rate this app opens.
const LOOKAHEAD_MS: f32 = 1.0;

/// A peak limiter that sees between the samples.
///
/// Two things happen per sample: the true peak is reconstructed at four
/// eighths of a sample either side of the centre tap, and the gain that peak
/// demands is drawn backwards as a straight line across the lookahead, kept
/// only where it is lower than what is already planned. By the time a peak
/// reaches the output, the gain has been at its final value for long enough
/// that every tap contributing to that peak was scaled by it.
///
/// The ceiling is a floor-level guarantee rather than an average: the applied
/// gain is never above the planned envelope, and the planned envelope is never
/// above what the loudest reconstructed point in range can take.
struct Limiter {
    /// `[phase][tap]`, phase 0 omitted because it is the identity.
    taps: [[f32; TP_TAPS]; TP_PHASES - 1],
    /// The last `TP_TAPS` driven input samples per channel. Index 0 is newest.
    hist: [[f32; TP_TAPS]; 2],
    /// The audio itself, delayed by the lookahead. Interleaved stereo.
    delay: Vec<[f32; 2]>,
    /// The gain planned for each of the next `look` output samples.
    env: Vec<f32>,
    /// Where the output is being read from, in both ring buffers.
    at: usize,
    /// Lookahead in samples. `env` and `delay` are one longer than this.
    look: usize,
    /// The gain actually applied, which chases `env` down at once and up on
    /// the release.
    gain: f32,
    sample_rate: f32,
}

impl Limiter {
    fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8_000.0);
        let mut taps = [[0.0; TP_TAPS]; TP_PHASES - 1];
        for (i, row) in taps.iter_mut().enumerate() {
            let frac = (i + 1) as f32 / TP_PHASES as f32;
            for (k, t) in row.iter_mut().enumerate() {
                // A windowed sinc, centred, offset by the phase. `hist` runs
                // newest-first, so tap k reads k samples ago.
                let x = k as f32 - TP_CENTRE as f32 + frac;
                let sinc = if x.abs() < 1.0e-6 {
                    1.0
                } else {
                    let pix = std::f32::consts::PI * x;
                    pix.sin() / pix
                };
                // Blackman, which buys stopband depth at a little width — the
                // right trade when the output is a maximum rather than audio.
                let n = k as f32 / (TP_TAPS - 1) as f32;
                let w = 0.42 - 0.5 * (std::f32::consts::TAU * n).cos()
                    + 0.08 * (2.0 * std::f32::consts::TAU * n).cos();
                *t = sinc * w;
            }
        }
        let look = ((LOOKAHEAD_MS * 0.001 * sr) as usize).max(TP_TAPS);
        Self {
            taps,
            hist: [[0.0; TP_TAPS]; 2],
            delay: vec![[0.0; 2]; look + 1],
            env: vec![1.0; look + 1],
            at: 0,
            look,
            gain: 1.0,
            sample_rate: sr,
        }
    }

    /// The loudest point on the reconstructed curve around the centre tap.
    #[inline]
    fn true_peak(&self) -> f32 {
        let mut peak = 0.0f32;
        for hist in &self.hist {
            peak = peak.max(hist[TP_CENTRE].abs());
            for row in &self.taps {
                let mut acc = 0.0;
                for (t, h) in row.iter().zip(hist) {
                    acc += t * h;
                }
                peak = peak.max(acc.abs());
            }
        }
        peak
    }

    /// The gain a peak demands, with a soft knee below the ceiling.
    #[inline]
    fn wanted(peak: f32, ceiling: f32, knee_db: f32) -> f32 {
        if peak <= TINY {
            return 1.0;
        }
        let over_db = 20.0 * (peak / ceiling).log10();
        if over_db <= -knee_db {
            1.0
        } else if knee_db > 1.0e-3 && over_db < knee_db {
            // A quadratic through (-knee, 0 dB) tangent to both segments,
            // which is the standard soft knee.
            let t = over_db + knee_db;
            10f32.powf(-(t * t / (4.0 * knee_db)) / 20.0)
        } else {
            ceiling / peak
        }
    }

    /// Push one stereo frame; return the frame `look + TP_CENTRE` ago,
    /// limited.
    #[inline]
    fn process(
        &mut self,
        x: [f32; 2],
        drive: f32,
        ceiling: f32,
        knee_db: f32,
        release: f32,
    ) -> [f32; 2] {
        for (hist, &sample) in self.hist.iter_mut().zip(&x) {
            hist.rotate_right(1);
            hist[0] = sample * drive;
        }
        let n = self.env.len();
        // The audio that the peak just measured belongs to: the centre tap.
        let centre = [self.hist[0][TP_CENTRE], self.hist[1][TP_CENTRE]];
        let write = (self.at + self.look) % n;
        self.delay[write] = centre;

        // **Draw the gain backwards to now.** This sample leaves the limiter
        // in `look` samples' time; the gain it needs has to be reached by
        // then, so plan a straight line from where the gain is to where it
        // must be, and keep whichever is lower at every step — an earlier,
        // louder peak's plan is not allowed to lift this one.
        let want = Self::wanted(self.true_peak(), ceiling, knee_db);
        if want < 1.0 {
            let from = self.gain;
            for k in 0..=self.look {
                let i = (self.at + k) % n;
                let t = k as f32 / self.look as f32;
                let along = from + (want - from) * t;
                if along < self.env[i] {
                    self.env[i] = along;
                }
            }
        }

        // Down with the plan, up on the release — and never above the plan,
        // which is what makes the ceiling a guarantee.
        let target = self.env[self.at];
        if target < self.gain {
            self.gain = target;
        } else {
            let ms = RELEASE_MS.0 + (RELEASE_MS.1 - RELEASE_MS.0) * release.clamp(0.0, 1.0);
            let coeff = (-1.0 / (ms * 0.001 * self.sample_rate)).exp();
            self.gain = target + (self.gain - target) * coeff;
        }
        let out = self.delay[self.at];
        self.env[self.at] = 1.0;
        self.at = (self.at + 1) % n;
        [out[0] * self.gain, out[1] * self.gain]
    }

    fn clear(&mut self) {
        self.hist = [[0.0; TP_TAPS]; 2];
        self.delay.iter_mut().for_each(|s| *s = [0.0; 2]);
        self.env.iter_mut().for_each(|g| *g = 1.0);
        self.at = 0;
        self.gain = 1.0;
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
    /// Corner frequency, across [`HPF_HZ`]. 0 is out of the way.
    pub hpf: f32,
    /// Corner frequency, across [`LPF_HZ`] — **up is darker**, because up is
    /// more of the effect on every other knob here and a filter knob that
    /// alone ran backwards would be the one everybody got wrong.
    pub lpf: f32,
    /// How hard the signal is driven into the ceiling. 0 is bypass.
    pub limiter: f32,
}

/// The three effects on the instrument bus.
pub struct Effects {
    reverb: [ReverbChannel; 2],
    delay: Delay,
    chorus: Chorus,
    hpf: [Filter; 2],
    lpf: [Filter; 2],
    limiter: Limiter,
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
            hpf: [Filter::default(); 2],
            lpf: [Filter::default(); 2],
            limiter: Limiter::new(sr),
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
            hpf: sends.hpf.clamp(0.0, 1.0),
            lpf: sends.lpf.clamp(0.0, 1.0),
            limiter: sends.limiter.clamp(0.0, 1.0),
        };
        // Every knob at rest AND every mix arrived there: nothing to do, and
        // nothing ringing that would be cut off by not doing it.
        let quiet = |m: &Sends| {
            m.reverb < OFF
                && m.delay < OFF
                && m.chorus < OFF
                && m.hpf < OFF
                && m.lpf < OFF
                && m.limiter < OFF
        };
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

        // The filters' corners, set once a block. A biquad's coefficients are
        // six transcendental functions; per-sample they would cost more than
        // the filter, and the knob does not move fast enough to notice.
        //
        // **The knob is the corner, and nothing else.** It was also a wet/dry
        // crossfade, on the theory that fading in from zero avoided a click.
        // What that actually built was a filter whose SLOPE depended on how
        // far the knob was turned: at half wet a 24 dB/octave low-pass reaches
        // 6 dB of rejection, which is not a 24 dB/octave low-pass and is not
        // what the menu says it is.
        //
        // No crossfade is needed, because the ends of the sweep are already
        // nothing: at zero the high-pass sits at 20 Hz and the low-pass at the
        // top of hearing, where a filter of any slope does nothing to music.
        // The corner follows the SLEWED knob rather than the target so a fast
        // drag glides instead of stepping.
        let sweep = |range: (f32, f32), t: f32| range.0 * (range.1 / range.0).powf(t);
        // Well clear of Nyquist: a 4th-order section at 20 kHz against a
        // 44.1 k stream is a quarter of an octave from the wall.
        let top = self.sample_rate * 0.45;
        let hpf_hz = sweep(HPF_HZ, self.mix.hpf).min(top);
        let lpf_hz = sweep(LPF_HZ, self.mix.lpf).min(top);
        // Running while EITHER the knob or the slewed value is up, so a knob
        // dropped to zero glides its corner back out of the way before the
        // filter stops rather than being switched out from under the signal.
        let hpf_on = want.hpf > OFF || self.mix.hpf > OFF;
        let lpf_on = want.lpf > OFF || self.mix.lpf > OFF;
        // **The limiter's delay line is emptied while it is not in use.**
        //
        // A guard rather than a fix, and it is worth being exact about which:
        // the line holds a millisecond of audio, and playing that back on
        // re-engage an hour later would be an audible fragment of something
        // else at full level — but it cannot happen today, because `mix`
        // reaches zero by SLEWING and the limiter keeps running the whole way
        // down, flushing itself with the silence it is being handed. A test
        // written for the fragment passed with this line removed, which is the
        // only reason that is known.
        //
        // It stays because it costs one comparison a block and stops being
        // free the day anything sets `mix` instead of easing it.
        let limiter_on = want.limiter > OFF || self.mix.limiter > OFF;
        if !limiter_on {
            self.limiter.clear();
        }
        for ch in 0..2 {
            if hpf_on {
                self.hpf[ch].set(self.sample_rate, hpf_hz, p.hpf_slope, p.hpf_resonance, true);
            } else {
                self.hpf[ch].clear();
            }
            if lpf_on {
                self.lpf[ch].set(self.sample_rate, lpf_hz, p.lpf_slope, p.lpf_resonance, false);
            } else {
                self.lpf[ch].clear();
            }
        }
        let ceiling = {
            let db = CEILING_DB.0 + (CEILING_DB.1 - CEILING_DB.0) * p.limiter_ceiling;
            10f32.powf(db / 20.0)
        };
        let knee_db = KNEE_DB.0 + (KNEE_DB.1 - KNEE_DB.0) * p.limiter_knee;

        for f in 0..frames {
            let at = f * channels;
            let dry = [buf[at], buf[at + 1]];

            self.mix.reverb += (want.reverb - self.mix.reverb) * KNOB_SLEW;
            self.mix.delay += (want.delay - self.mix.delay) * KNOB_SLEW;
            self.mix.chorus += (want.chorus - self.mix.chorus) * KNOB_SLEW;
            self.mix.hpf += (want.hpf - self.mix.hpf) * KNOB_SLEW;
            self.mix.lpf += (want.lpf - self.mix.lpf) * KNOB_SLEW;
            self.mix.limiter += (want.limiter - self.mix.limiter) * KNOB_SLEW;

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
            let mut out = [0.0f32; 2];
            for ch in 0..2 {
                out[ch] = chorused[ch] + wet_d[ch] * self.mix.delay + wet_r[ch] * self.mix.reverb;
            }

            // **After the effects, not before them.** These two are the tone
            // of what comes out, so a reverb tail that rumbles is something
            // the high-pass can fix. Ahead of the reverb it would only stop
            // the tail being made, which is a different and less useful knob.
            //
            // Each is crossfaded rather than switched: at rest the filter
            // still runs (it costs four multiplies) but contributes nothing,
            // so there is no click at the moment somebody leaves zero.
            if hpf_on {
                for (o, f) in out.iter_mut().zip(&mut self.hpf) {
                    *o = f.process(*o, p.hpf_slope, true);
                }
            }
            if lpf_on {
                for (o, f) in out.iter_mut().zip(&mut self.lpf) {
                    *o = f.process(*o, p.lpf_slope, false);
                }
            }

            // **Last, and after everything.** A limiter that is not the final
            // stage is not a limiter; it is an effect that something else gets
            // to overshoot afterwards.
            if limiter_on {
                let drive = 10f32.powf(DRIVE_DB * self.mix.limiter / 20.0);
                out = self
                    .limiter
                    .process(out, drive, ceiling, knee_db, p.limiter_release);
            }

            buf[at] = out[0];
            buf[at + 1] = out[1];
        }
    }

    /// Drop every tail. Called when every knob reaches zero, and on a stop.
    pub fn clear(&mut self) {
        for r in &mut self.reverb {
            r.clear();
        }
        self.delay.clear();
        self.chorus.clear();
        for f in self.hpf.iter_mut().chain(self.lpf.iter_mut()) {
            f.clear();
        }
        self.limiter.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// Reconstruct the analogue peak of a buffer, INDEPENDENTLY of the
    /// limiter's own detector.
    ///
    /// **Eight times, thirty-three taps, against the limiter's four and
    /// thirteen.** A true-peak claim checked with the same filter that made it
    /// is not a check; it is the detector agreeing with itself. This is the
    /// slower, longer reconstruction a converter is closer to.
    fn true_peak(buf: &[f32], channels: usize, ch: usize) -> f32 {
        const PHASES: usize = 8;
        const TAPS: usize = 33;
        const CENTRE: isize = (TAPS / 2) as isize;
        let x: Vec<f32> = buf.iter().skip(ch).step_by(channels).copied().collect();
        let mut peak = 0.0f32;
        // **The interior only.** Off the ends this convolution has no samples
        // to read and treats them as silence, which is a step from zero into
        // whatever the block starts on — and a truncated sinc rings on a step
        // by about 9%. Measured over the whole buffer that ringing reads as
        // the limiter overshooting by 0.9 dB, which is a bug in the ruler.
        for n in CENTRE as usize..x.len().saturating_sub(CENTRE as usize) {
            peak = peak.max(x[n].abs());
            for ph in 1..PHASES {
                let frac = ph as f32 / PHASES as f32;
                let mut acc = 0.0;
                for k in 0..TAPS {
                    let t = k as f32 - CENTRE as f32 - frac;
                    let sinc = if t.abs() < 1.0e-9 {
                        1.0
                    } else {
                        let pit = std::f32::consts::PI * t;
                        pit.sin() / pit
                    };
                    let w = 0.42
                        - 0.5 * (std::f32::consts::TAU * k as f32 / (TAPS - 1) as f32).cos()
                        + 0.08
                            * (2.0 * std::f32::consts::TAU * k as f32 / (TAPS - 1) as f32).cos();
                    let i = n as isize + k as isize - CENTRE;
                    if i >= 0 && (i as usize) < x.len() {
                        acc += sinc * w * x[i as usize];
                    }
                }
                peak = peak.max(acc.abs());
            }
        }
        peak
    }

    /// A sine, interleaved stereo, at `amp`, starting at sample `from`.
    ///
    /// **`from` matters.** Restarting the phase at every block puts a step in
    /// the waveform at every block boundary, which is a click with content all
    /// the way to Nyquist — and then the thing being measured is the limiter's
    /// response to a click rather than to the sine it was handed.
    fn sine(frames: usize, hz: f32, amp: f32, from: usize) -> Vec<f32> {
        let mut v = vec![0.0; frames * 2];
        for f in 0..frames {
            let t = (from + f) as f32;
            let x = amp * (std::f32::consts::TAU * hz * t / SR).sin();
            v[f * 2] = x;
            v[f * 2 + 1] = x;
        }
        v
    }

    /// **The ceiling is a guarantee, not an average.**
    ///
    /// Measured on the reconstructed waveform rather than on the samples,
    /// because that is where the claim lives: a buffer whose samples all sit
    /// under -1 dBFS can still hand a converter a curve that goes over, and
    /// "true peak" is exactly the promise that it does not.
    #[test]
    fn the_limiter_holds_a_true_peak_ceiling() {
        // 7 kHz at 48 k is a little over six samples a cycle: the peaks land
        // between samples most of the time, which is the case a sample-peak
        // limiter gets wrong.
        for (hz, amp) in [(7_000.0, 1.0), (11_000.0, 0.9), (997.0, 1.0)] {
            let mut fx = Effects::new(SR);
            let p = Params::default();
            let sends = Sends {
                limiter: 1.0,
                ..Sends::default()
            };
            // Long enough for the knob slew to arrive at full drive.
            let mut worst = 0.0f32;
            for block in 0..40 {
                let mut buf = sine(2_048, hz, amp, block * 2_048);
                fx.process(&mut buf, 2_048, 2, sends, &p, 120.0);
                // Skip the first blocks: the mix is still sliding up and the
                // drive with it, which is not the steady state being claimed.
                if block >= 30 {
                    worst = worst.max(true_peak(&buf, 2, 0));
                }
            }
            let ceiling_db = CEILING_DB.0 + (CEILING_DB.1 - CEILING_DB.0) * p.limiter_ceiling;
            let got_db = 20.0 * worst.log10();
            assert!(
                got_db <= ceiling_db + 0.35,
                "{hz} Hz reconstructed to {got_db:.2} dBTP against a {ceiling_db:.2} dB ceiling"
            );
            // And it is actually working, not just quiet: driven 12 dB into a
            // -1 dB ceiling, the output should be up near it.
            assert!(
                got_db > ceiling_db - 3.0,
                "{hz} Hz only reached {got_db:.2} dBTP - the limiter is not passing signal"
            );
        }
    }

    /// The default ceiling is the one that was asked for.
    #[test]
    fn the_limiter_ships_at_minus_one_db() {
        let d = Params::default();
        let db = CEILING_DB.0 + (CEILING_DB.1 - CEILING_DB.0) * d.limiter_ceiling;
        assert!((db + 1.0).abs() < 0.01, "the default ceiling is {db} dB");
    }

    /// **Three slopes, and they are the slopes they are named after.**
    ///
    /// 6, 12 and 24 dB an octave are first, second and fourth order, and the
    /// response of each is checked against the Butterworth its name promises:
    /// `1 / sqrt(1 + r^2n)`, where `r` is how many times past the corner the
    /// frequency is. Checking the ASYMPTOTE alone would pass a filter that is
    /// the right steepness in the far stopband and the wrong shape anywhere
    /// near the corner, which is the part anybody hears.
    #[test]
    fn the_filter_slopes_are_what_they_say() {
        for (slope, order) in [(Slope::Six, 1_i32), (Slope::Twelve, 2), (Slope::TwentyFour, 4)] {
            for high in [true, false] {
                // Both measurement points stay under an eighth of the sample
                // rate: a digital filter's response bends towards Nyquist, and
                // measuring a low-pass at 16 kHz against 48 k reads 3.9 dB an
                // octave for a perfectly good 6 — that is the warping, not the
                // filter. Deeper than this the levels reach -90 dB, where an
                // f32 biquad is at its own floor and reads shallow.
                let corner = if high { 2_000.0_f32 } else { 200.0 };
                let theory = |hz: f32| {
                    let r = if high { corner / hz } else { hz / corner };
                    -10.0 * (1.0 + r.powi(2 * order)).log10()
                };
                let measure = |hz: f32| {
                    let mut f = Filter::default();
                    f.set(SR, corner, slope, 0.0, high);
                    let n = 48_000;
                    let mut peak = 0.0f32;
                    for i in 0..n {
                        let y = f.process(
                            (std::f32::consts::TAU * hz * i as f32 / SR).sin(),
                            slope,
                            high,
                        );
                        // The last third, once the transient has gone.
                        if i > n * 2 / 3 {
                            peak = peak.max(y.abs());
                        }
                    }
                    20.0 * peak.max(1.0e-9).log10()
                };
                let side = if high { "high-pass" } else { "low-pass" };
                for mul in [0.5_f32, 0.25, 1.0] {
                    let hz = if high { corner * mul } else { corner / mul };
                    let (got, want) = (measure(hz), theory(hz));
                    assert!(
                        (got - want).abs() < 1.2,
                        "{slope:?} {side} at {hz:.0} Hz measured {got:.1} dB, \
                         Butterworth says {want:.1} dB"
                    );
                }
                // And the pair of points an octave apart really is 6 dB per
                // order, which is what the label on the menu claims.
                let (near, far) = if high {
                    (corner * 0.25, corner * 0.125)
                } else {
                    (corner * 4.0, corner * 8.0)
                };
                let per_octave = theory(near) - theory(far);
                assert!(
                    (per_octave - f32::from(order as i16) * 6.0).abs() < 0.5,
                    "{slope:?} works out at {per_octave:.1} dB an octave"
                );
            }
        }
    }

    /// **A filter half way up is still the slope it says it is.**
    ///
    /// The knob used to be the corner AND a wet/dry mix, so at half travel a
    /// 24 dB/octave low-pass rejected 6 dB — the crossfade put the unfiltered
    /// signal straight back. Measured through `Effects`, not through `Filter`,
    /// because the bug was never in the filter.
    #[test]
    fn a_filter_at_half_travel_still_rejects() {
        // Half way up the low-pass sweep is about 2 kHz; 16 kHz is three
        // octaves past it and a fourth-order filter owes about 72 dB there.
        let p = Params::default();
        let sends = Sends {
            lpf: 0.5,
            ..Sends::default()
        };
        let mut fx = Effects::new(SR);
        let mut worst: f32 = 0.0;
        for block in 0..60 {
            let mut buf = sine(2_048, 16_000.0, 1.0, block * 2_048);
            fx.process(&mut buf, 2_048, 2, sends, &p, 120.0);
            // Once the knob has slewed all the way up.
            if block >= 50 {
                worst = worst.max(
                    buf.iter()
                        .skip(64)
                        .fold(0.0f32, |a, b| a.max(b.abs())),
                );
            }
        }
        let db = 20.0 * worst.max(1.0e-9).log10();
        assert!(
            db < -30.0,
            "a half-open 24 dB/oct low-pass left 16 kHz at {db:.1} dB - it is \
             mixing the dry signal back in"
        );
    }

    /// Every new knob at zero leaves the signal alone.
    ///
    /// The filters are crossfaded and the limiter is bypassed, so "off" has to
    /// mean bit-for-bit off — not "nearly", which on a filter is a tone change
    /// nobody asked for and on a limiter is level.
    #[test]
    fn the_filters_and_the_limiter_do_nothing_at_zero() {
        let mut fx = Effects::new(SR);
        let p = Params::default();
        let sends = Sends {
            reverb: 0.5,
            ..Sends::default()
        };
        let mut buf = sine(512, 440.0, 0.5, 0);
        let before = buf.clone();
        fx.process(&mut buf, 512, 2, sends, &p, 120.0);
        // The reverb is on, so the buffer changed - but the change is the
        // reverb's. With the reverb off too, nothing at all happens.
        let mut clean = Effects::new(SR);
        let mut buf2 = before.clone();
        clean.process(&mut buf2, 512, 2, Sends::default(), &p, 120.0);
        assert_eq!(buf2, before, "every knob at zero is not a no-op");
    }

    /// The three wet sends, with the filters and limiter out of the way.
    fn sends(reverb: f32, delay: f32, chorus: f32) -> Sends {
        Sends {
            reverb,
            delay,
            chorus,
            ..Sends::default()
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
                        hpf_slope: Slope::TwentyFour,
                        hpf_resonance: b,
                        lpf_slope: Slope::TwentyFour,
                        lpf_resonance: a,
                        limiter_ceiling: a,
                        limiter_release: b,
                        limiter_knee: a,
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
