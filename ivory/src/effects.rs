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

/// Samples the right channel's delays are lengthened by.
///
/// What makes the output stereo at all: the same input through two slightly
/// different rooms. Too small and it is mono; too large and it is an echo.
const STEREO_SPREAD: usize = 23;

/// The rate the lengths above were chosen at.
const DESIGN_RATE: f32 = 44_100.0;

/// How much of each comb's output returns to its input. Room size.
const COMB_FEEDBACK: f32 = 0.86;

/// The lowpass inside each comb. Air and soft furnishings absorb treble, so a
/// reverb whose highs last as long as its lows sounds like a swimming pool.
const COMB_DAMP: f32 = 0.28;

/// The fixed allpass coefficient from the original. Diffusion, not colour.
const ALLPASS_FEEDBACK: f32 = 0.5;

/// Scales the whole wet path. Eight combs in parallel is eight times the input
/// before anything else happens.
const REVERB_TRIM: f32 = 0.055;

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
    fn process(&mut self, input: f32) -> f32 {
        let out = self.buf[self.at];
        self.store = flush(out * (1.0 - COMB_DAMP) + self.store * COMB_DAMP);
        self.buf[self.at] = flush(input + self.store * COMB_FEEDBACK);
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
    fn process(&mut self, input: f32) -> f32 {
        // Parallel: every comb sees the same input and their outputs sum.
        let mut out = 0.0;
        for c in &mut self.combs {
            out += c.process(input);
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

/// How much of each repeat comes back for the next one.
///
/// Chosen so a repeat is clearly a repeat and the tail is gone inside a bar or
/// two. Higher is a wash that buries the next phrase; the knob is a mix
/// control, not a runaway.
const DELAY_FEEDBACK: f32 = 0.42;

/// One-pole lowpass in the feedback loop, as a coefficient on the new sample.
///
/// Each repeat darker than the last, which is what tape did and why tape delay
/// sits under a piano without competing with it.
const DELAY_DAMP: f32 = 0.62;

/// The right channel's delay, as a fraction of the left's.
///
/// Not the same length, or the repeats are mono and sit on top of the dry
/// signal. Two thirds puts the right channel on a triplet against the left,
/// which is a musical relationship rather than a smear.
const DELAY_RIGHT_RATIO: f32 = 0.667;

/// Longest delay the line can hold, in seconds. Two seconds covers a whole bar
/// at 30 bpm.
const MAX_DELAY_SECS: f32 = 2.0;

/// Beats per repeat. A dotted eighth, which is the delay setting that made
/// every record between 1983 and 1991 sound like that, and which fills space
/// without doubling the beat.
const DELAY_BEATS: f32 = 0.75;

/// Seconds per repeat when there is no sensible tempo to sync to.
const DELAY_FALLBACK_SECS: f32 = 0.375;

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
            len: [sample_rate * DELAY_FALLBACK_SECS; 2],
            sample_rate,
        }
    }

    /// Where the delay time is heading, in samples, from the tempo.
    fn target(&self, bpm: f64) -> [f32; 2] {
        let secs = if bpm.is_finite() && bpm > 1.0 {
            (60.0 / bpm) as f32 * DELAY_BEATS
        } else {
            DELAY_FALLBACK_SECS
        };
        let cap = (self.left.len() - 2) as f32;
        let l = (secs * self.sample_rate).clamp(1.0, cap);
        [l, (l * DELAY_RIGHT_RATIO).clamp(1.0, cap)]
    }

    /// Read `len` samples back, interpolating, so a slewing time glides.
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
    fn process(&mut self, input: [f32; 2], target: [f32; 2]) -> [f32; 2] {
        let mut out = [0.0_f32; 2];
        for ch in 0..2 {
            self.len[ch] += (target[ch] - self.len[ch]).clamp(
                -DELAY_SLEW * self.sample_rate,
                DELAY_SLEW * self.sample_rate,
            );
            let buf = if ch == 0 { &self.left } else { &self.right };
            let wet = Self::tap(buf, self.at, self.len[ch]);
            self.store[ch] = flush(wet * (1.0 - DELAY_DAMP) + self.store[ch] * DELAY_DAMP);
            let write = flush(input[ch] + self.store[ch] * DELAY_FEEDBACK);
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

/// Reverb and delay on the instrument bus.
pub struct Effects {
    reverb: [ReverbChannel; 2],
    delay: Delay,
    /// Where the wet mixes actually are, chasing where the knobs say.
    reverb_mix: f32,
    delay_mix: f32,
    /// Both were off last block, so the tails are already flushed and do not
    /// need flushing again.
    idle: bool,
}

impl Effects {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate.max(8_000.0);
        Self {
            reverb: [ReverbChannel::new(sr, 0), ReverbChannel::new(sr, STEREO_SPREAD)],
            delay: Delay::new(sr),
            reverb_mix: 0.0,
            delay_mix: 0.0,
            idle: true,
        }
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
        reverb: f32,
        delay: f32,
        bpm: f64,
    ) {
        let (r_want, d_want) = (reverb.clamp(0.0, 1.0), delay.clamp(0.0, 1.0));
        // Both knobs at rest AND both mixes arrived there: nothing to do, and
        // nothing ringing that would be cut off by not doing it.
        if r_want < OFF && d_want < OFF && self.reverb_mix < OFF && self.delay_mix < OFF {
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
        let target = self.delay.target(bpm);
        for f in 0..frames {
            let at = f * channels;
            let dry = [buf[at], buf[at + 1]];

            self.reverb_mix += (r_want - self.reverb_mix) * KNOB_SLEW.max(0.0);
            self.delay_mix += (d_want - self.delay_mix) * KNOB_SLEW.max(0.0);

            let wet_d = self.delay.process(dry, target);
            // The reverb hears the delay's repeats, which is the order a mixing
            // desk puts them in and the reason a delayed note sounds like it is
            // in the same room as the note that made it.
            let into_verb = [
                dry[0] + wet_d[0] * self.delay_mix,
                dry[1] + wet_d[1] * self.delay_mix,
            ];
            let wet_r = [
                self.reverb[0].process(into_verb[0]) * REVERB_TRIM,
                self.reverb[1].process(into_verb[1]) * REVERB_TRIM,
            ];
            for ch in 0..2 {
                buf[at + ch] = dry[ch] + wet_d[ch] * self.delay_mix + wet_r[ch] * self.reverb_mix;
            }
        }
    }

    /// Drop every tail. Called when both knobs reach zero, and on a stop.
    pub fn clear(&mut self) {
        for r in &mut self.reverb {
            r.clear();
        }
        self.delay.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;

    /// One impulse in, `frames` of output, at 120 bpm.
    fn strike(fx: &mut Effects, frames: usize, reverb: f32, delay: f32) -> Vec<f32> {
        strike_at(fx, frames, reverb, delay, 120.0)
    }

    /// As [`strike`], at a stated tempo. **The tempo has to be the same one the
    /// warm-up used**: the delay time slews, so passing a different bpm here
    /// starts it moving back and the repeat arrives somewhere between the two.
    fn strike_at(fx: &mut Effects, frames: usize, reverb: f32, delay: f32, bpm: f64) -> Vec<f32> {
        let mut out = vec![0.0_f32; frames * 2];
        out[0] = 1.0;
        out[1] = 1.0;
        fx.process(&mut out, frames, 2, reverb, delay, bpm);
        out
    }

    fn peak(v: &[f32]) -> f32 {
        v.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
    }

    /// **Off has to be silent, and it has to be free.** The default state of
    /// both knobs, and if a buffer comes back changed with both at zero then
    /// every user is paying for an effect they did not ask for.
    #[test]
    fn both_knobs_at_zero_do_not_touch_the_signal() {
        let mut fx = Effects::new(SR);
        let mut buf: Vec<f32> = (0..512).map(|i| (i as f32 * 0.01).sin()).collect();
        let before = buf.clone();
        fx.process(&mut buf, 256, 2, 0.0, 0.0, 120.0);
        assert_eq!(buf, before, "an effect at zero changed the signal");
    }

    /// Turning reverb up puts energy where there was none, and it decays.
    #[test]
    fn reverb_makes_a_tail_that_dies() {
        let mut fx = Effects::new(SR);
        let out = strike(&mut fx, 24_000, 1.0, 0.0);
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

    /// **The delay lands on the beat.** The whole reason the time is not a
    /// knob: at 120 bpm a dotted eighth is 375 ms, and a repeat anywhere else
    /// is a mistake somebody has to hear to find.
    #[test]
    fn the_delay_follows_the_tempo() {
        for (bpm, want_secs) in [(120.0_f64, 0.375_f32), (90.0, 0.5), (60.0, 0.75)] {
            let mut fx = Effects::new(SR);
            // Long enough for the slew to arrive before the impulse is sent.
            let mut warm = vec![0.0_f32; 2 * 48_000];
            fx.process(&mut warm, 48_000, 2, 0.0, 1.0, bpm);

            let out = strike_at(&mut fx, 48_000, 0.0, 1.0, bpm);
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
                (got - want_secs).abs() < 0.02,
                "at {bpm} bpm the first repeat landed at {got:.3}s, not {want_secs:.3}s"
            );
        }
    }

    /// Repeats get quieter, or the knob is an oscillator.
    #[test]
    fn the_delay_does_not_run_away() {
        let mut fx = Effects::new(SR);
        let out = strike(&mut fx, 48_000 * 4, 0.0, 1.0);
        let first = peak(&out[..48_000]);
        let last = peak(&out[48_000 * 6..]);
        assert!(last < first * 0.2, "the delay is not decaying: {first} -> {last}");
    }

    /// **Nothing rings on from an hour ago.** Turning a knob down flushes the
    /// tails, so turning it back up starts from silence rather than replaying
    /// whatever was in the buffers.
    #[test]
    fn turning_both_off_flushes_what_was_ringing() {
        let mut fx = Effects::new(SR);
        strike(&mut fx, 8_000, 1.0, 1.0);
        // Down, and long enough for the mixes to slew to zero.
        let mut off = vec![0.0_f32; 2 * 48_000 * 2];
        fx.process(&mut off, 48_000 * 2, 2, 0.0, 0.0, 120.0);
        // Up again, into silence.
        let mut back = vec![0.0_f32; 2 * 4_000];
        fx.process(&mut back, 4_000, 2, 1.0, 1.0, 120.0);
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
            let out = strike(&mut fx, frames, 1.0, 0.0);
            let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0].abs()).collect();
            let peak = left.iter().skip(1000).fold(0.0_f32, |m, v| m.max(*v));
            // Where it falls to a thousandth of its loudest.
            left.iter()
                .rposition(|v| *v > peak * 0.001)
                .unwrap_or(0) as f32
                / sr
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
        fx.process(&mut buf, 256, 1, 1.0, 1.0, 120.0);
        assert_eq!(buf, before, "a mono buffer was written to");
    }
}
