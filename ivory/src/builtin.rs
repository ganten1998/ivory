//! The instrument Tangent has when it has no instrument.
//!
//! # Why this exists
//!
//! Out of the box the app drew a piano and made no sound. Every note it could
//! name, engrave and place on a fretboard was silent until the user found a
//! VST3, installed it, and loaded it into a slot — which is a reasonable ask of
//! somebody building a rig and an unreasonable one of somebody who has just
//! opened a free app to see what it does.
//!
//! # Why synthesis and not samples
//!
//! **Because it is free, in both senses.** A sampled piano worth having is tens
//! of megabytes at best, and the good ones are licensed. Rendering samples out
//! of a commercial plugin and shipping them is a licence violation dressed as
//! an engineering decision. A CC-licensed library is a legitimate route and
//! still costs megabytes and an attribution page.
//!
//! This is a few hundred lines and no bytes at all. It ships on every platform,
//! needs no download, cannot be missing, and raises no question about what may
//! be redistributed.
//!
//! # What it sounds like, and what it does not pretend to be
//!
//! A tine electric piano: two-operator FM, which is the cheapest structure that
//! sounds like a real instrument rather than like a test tone. A bright,
//! fast-decaying partial at fourteen times the fundamental gives the strike; a
//! sine at the fundamental gives the body; the strike's envelope is much
//! shorter than the body's, which is exactly what a Rhodes tine does.
//!
//! It is NOT an acoustic piano and does not try to be. A bad piano imitation is
//! worse than a good electric one, and this is the sound somebody can play for
//! an hour without wincing. Anybody who wants a Steinway loads a Steinway; the
//! point of this is that the app works before they do.
//!
//! # The rules this file lives by
//!
//! **It runs on the audio thread.** No allocation, no locks, no panics, no
//! `f64` transcendentals in the inner loop beyond what a sine costs. The voice
//! array is fixed and voices are stolen, never grown.
//!
//! **Silence costs nothing.** With no voice sounding, `render` returns without
//! touching the buffer, so the built-in is free when a real plugin is loaded.

/// How many notes can sound at once.
///
/// Sixteen, which is two hands and a pedal's worth of overlap. A stolen voice
/// is a real artefact and this is well past where a player would notice.
const VOICES: usize = 16;

/// The strike partial's ratio to the fundamental.
///
/// Fourteen is the tine. Lower ratios sound like an organ, higher ones like a
/// bell; this is the one that reads as a struck metal bar.
const TINE_RATIO: f32 = 14.0;

/// Seconds for the strike to fall to a thousandth. Short: it is a transient,
/// and a slow one turns the instrument into a bell.
const TINE_DECAY: f32 = 0.16;

/// The body's decay at middle C, in seconds. Scaled by pitch, because a low
/// string rings far longer than a high one on every real instrument.
const BODY_DECAY_C4: f32 = 2.6;

/// Seconds to fall silent after a note-off. Not instant: an instant cut is a
/// click, and the click is the only thing anybody would hear.
const RELEASE: f32 = 0.28;

/// Per-voice level, before the limiter.
///
/// Chosen so one note is healthy rather than so sixteen cannot clip: the
/// limiter below is what makes the ceiling a guarantee, and picking a gain low
/// enough for the pathological case would make every ordinary note quiet.
const GAIN: f32 = 0.20;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Idle,
    Held,
    Released,
}

#[derive(Clone, Copy)]
struct Voice {
    stage: Stage,
    pitch: i16,
    /// Radians per frame for the fundamental, and for the tine.
    step: f32,
    tine_step: f32,
    phase: f32,
    tine_phase: f32,
    /// Multiplied in per frame; an exponential decay without a `powf`.
    body_k: f32,
    tine_k: f32,
    release_k: f32,
    body_env: f32,
    tine_env: f32,
    release_env: f32,
    /// How hard it was struck: sets both loudness and how much tine is heard,
    /// which is what makes a real one bark when you dig in.
    velocity: f32,
    /// Oldest-first ordering for voice stealing.
    age: u64,
}

impl Voice {
    const SILENT: Self = Self {
        stage: Stage::Idle,
        pitch: 0,
        step: 0.0,
        tine_step: 0.0,
        phase: 0.0,
        tine_phase: 0.0,
        body_k: 0.0,
        tine_k: 0.0,
        release_k: 0.0,
        body_env: 0.0,
        tine_env: 0.0,
        release_env: 0.0,
        velocity: 0.0,
        age: 0,
    };
}

pub struct Builtin {
    voices: [Voice; VOICES],
    sample_rate: f32,
    /// Monotonic, so the oldest voice is always the smallest. Wrapping is
    /// unreachable at any real sample rate but costs nothing to be right about.
    clock: u64,
    /// Sustain: notes released while it is down keep ringing until it lifts.
    pedal: bool,
    sounding: usize,
}

impl Builtin {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            voices: [Voice::SILENT; VOICES],
            sample_rate: sample_rate.max(8_000.0),
            clock: 0,
            pedal: false,
            sounding: 0,
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: f32) {
        let sr = sample_rate.max(8_000.0);
        if (sr - self.sample_rate).abs() > 0.5 {
            self.sample_rate = sr;
            self.all_notes_off();
        }
    }

    /// Whether anything is sounding. `render` is free when this is false, and
    /// the engine uses it to decide whether the built-in is worth mixing.
    pub fn active(&self) -> bool {
        self.sounding > 0
    }

    pub fn note_on(&mut self, pitch: i16, velocity: f32) {
        if !(0..=127).contains(&pitch) {
            return;
        }
        // A repeated note takes its own voice back rather than adding a second
        // one: a trill on one key must not eat the polyphony.
        let slot = self
            .voices
            .iter()
            .position(|v| v.stage != Stage::Idle && v.pitch == pitch)
            .or_else(|| self.voices.iter().position(|v| v.stage == Stage::Idle))
            .unwrap_or_else(|| {
                // Steal the oldest. Not the quietest: finding the quietest
                // means a pass over sixteen envelopes on the audio thread, and
                // oldest is what a player expects anyway.
                let mut best = 0;
                for (i, v) in self.voices.iter().enumerate() {
                    if v.age < self.voices[best].age {
                        best = i;
                    }
                }
                let _ = best;
                best
            });

        let hz = 440.0 * 2.0_f32.powf((f32::from(pitch) - 69.0) / 12.0);
        let tau = std::f32::consts::TAU;
        let vel = velocity.clamp(0.05, 1.0);
        // Higher notes decay faster, and the relationship is roughly one octave
        // per halving. `pitch` rather than `hz` so this is one multiply.
        let octaves_above_c4 = (f32::from(pitch) - 60.0) / 12.0;
        let body_secs = (BODY_DECAY_C4 * 0.5_f32.powf(octaves_above_c4 * 0.7)).clamp(0.25, 6.0);

        self.clock = self.clock.wrapping_add(1);
        let was_idle = self.voices[slot].stage == Stage::Idle;
        self.voices[slot] = Voice {
            stage: Stage::Held,
            pitch,
            step: tau * hz / self.sample_rate,
            tine_step: tau * hz * TINE_RATIO / self.sample_rate,
            // **Not zero.** Every voice starting at phase zero makes their
            // attacks sum coherently, and a sixteen-note cluster peaked at
            // 2.3 before this: sixteen sines all crossing zero together are
            // sixteen times one sine. A real instrument has no such alignment.
            // Spread by slot, deterministically, so a take renders identically
            // twice.
            phase: std::f32::consts::TAU * (slot as f32 * 0.618_034) % std::f32::consts::TAU,
            tine_phase: std::f32::consts::TAU * (slot as f32 * 0.377_19) % std::f32::consts::TAU,
            body_k: decay_k(body_secs, self.sample_rate),
            tine_k: decay_k(TINE_DECAY, self.sample_rate),
            release_k: decay_k(RELEASE, self.sample_rate),
            body_env: 1.0,
            tine_env: 1.0,
            release_env: 1.0,
            velocity: vel,
            age: self.clock,
        };
        if was_idle {
            self.sounding += 1;
        }
    }

    pub fn note_off(&mut self, pitch: i16) {
        if self.pedal {
            // Held by the pedal. The voice stays in `Held` and is released when
            // the pedal lifts, which is what a damper does.
            for v in &mut self.voices {
                if v.stage == Stage::Held && v.pitch == pitch {
                    v.pitch = -pitch - 1; // marked: released by the player, held by the pedal
                }
            }
            return;
        }
        for v in &mut self.voices {
            if v.stage == Stage::Held && v.pitch == pitch {
                v.stage = Stage::Released;
            }
        }
    }

    pub fn set_pedal(&mut self, down: bool) {
        let lifting = self.pedal && !down;
        self.pedal = down;
        if lifting {
            for v in &mut self.voices {
                if v.stage == Stage::Held && v.pitch < 0 {
                    v.stage = Stage::Released;
                }
            }
        }
    }

    pub fn all_notes_off(&mut self) {
        self.voices = [Voice::SILENT; VOICES];
        self.sounding = 0;
        self.pedal = false;
    }

    /// Add `frames` of interleaved audio into `out`, which has `channels` of it.
    ///
    /// **Adds rather than writes**, so the caller can mix the built-in over
    /// whatever else is on the bus without a second buffer.
    pub fn render(&mut self, out: &mut [f32], frames: usize, channels: usize) {
        if self.sounding == 0 || channels == 0 {
            return;
        }
        let n = frames.min(out.len() / channels);
        // **Frame-major, so the voices can be summed before they are limited.**
        // Voice-major would need somewhere to put the sum, and there is nowhere
        // to put anything on this thread.
        for f in 0..n {
            let mut sum = 0.0_f32;
            for v in &mut self.voices {
                if v.stage == Stage::Idle {
                    continue;
                }
                // Two operators: the tine modulates the body's phase. The
                // modulation index falls with the tine's own envelope, which
                // turns a bright strike into a warm sustain with no filter
                // anywhere.
                let index = v.tine_env * (0.9 + 2.6 * v.velocity);
                let body = fast_sin(v.phase + index * fast_sin(v.tine_phase));
                sum += body * v.body_env * v.release_env * v.velocity * GAIN;

                v.phase += v.step;
                v.tine_phase += v.tine_step;
                if v.phase >= std::f32::consts::TAU {
                    v.phase -= std::f32::consts::TAU;
                }
                if v.tine_phase >= std::f32::consts::TAU {
                    v.tine_phase -= std::f32::consts::TAU;
                }
                v.body_env *= v.body_k;
                v.tine_env *= v.tine_k;
                if v.stage == Stage::Released {
                    v.release_env *= v.release_k;
                }
            }
            let s = soft_clip(sum);
            let base = f * channels;
            for c in 0..channels {
                out[base + c] += s;
            }
        }
        // Retire what can no longer be heard, once per block rather than once
        // per frame. The threshold is below the least significant bit of
        // sixteen-bit audio.
        for v in &mut self.voices {
            if v.stage != Stage::Idle && v.body_env * v.release_env < 1.0e-4 {
                *v = Voice::SILENT;
                self.sounding = self.sounding.saturating_sub(1);
            }
        }
    }
}

/// A ceiling that cannot be exceeded, and does not sound like a wall.
///
/// The alternative to a limiter is a per-voice gain low enough that sixteen
/// notes cannot sum past one, which makes a single note four per cent of full
/// scale. A real electric piano goes through a preamp that does exactly this,
/// so a big chord compressing slightly is the instrument behaving, not the
/// software failing.
///
/// Linear below the knee, so ordinary playing is untouched.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.6;
    let a = x.abs();
    if a <= KNEE {
        return x;
    }
    let over = a - KNEE;
    // Asymptotes at 1.0: KNEE plus a saturating remainder.
    let shaped = KNEE + (1.0 - KNEE) * (over / (over + (1.0 - KNEE)));
    if x < 0.0 {
        -shaped
    } else {
        shaped
    }
}

/// The per-frame multiplier that falls to a thousandth in `secs`.
fn decay_k(secs: f32, sample_rate: f32) -> f32 {
    let frames = (secs * sample_rate).max(1.0);
    // 0.001^(1/frames), without a `powf` per frame at render time.
    (-6.907_755_4 / frames).exp()
}

/// A sine, cheaply.
///
/// `f32::sin` is a library call per operator per frame per voice; at sixteen
/// voices and two operators that is thirty-two of them. This is a Bhaskara-style
/// approximation with a peak error around a thousandth, which is inaudible in a
/// waveform and free.
fn fast_sin(x: f32) -> f32 {
    const TAU: f32 = std::f32::consts::TAU;
    const PI: f32 = std::f32::consts::PI;
    // Fold into -PI..PI.
    let mut x = x % TAU;
    if x > PI {
        x -= TAU;
    } else if x < -PI {
        x += TAU;
    }
    // 4x(PI - |x|) / (5PI^2/4 - |x|(PI - |x|)), signed.
    let ax = x.abs();
    let num = 16.0 * x * (PI - ax);
    let den = 5.0 * PI * PI - 4.0 * ax * (PI - ax);
    num / den
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(buf: &[f32]) -> f32 {
        if buf.is_empty() {
            return 0.0;
        }
        (buf.iter().map(|s| s * s).sum::<f32>() / buf.len() as f32).sqrt()
    }

    /// The approximation has to actually be a sine, or every note is subtly the
    /// wrong instrument.
    #[test]
    fn the_cheap_sine_is_a_sine() {
        let mut worst = 0.0_f32;
        for i in 0..2000 {
            let x = -8.0 + 16.0 * i as f32 / 2000.0;
            worst = worst.max((fast_sin(x) - x.sin()).abs());
        }
        assert!(worst < 0.002, "peak error {worst}");
    }

    /// Silence costs nothing, which is what makes the built-in free while a
    /// real plugin is loaded.
    #[test]
    fn an_idle_instrument_does_not_touch_the_buffer() {
        let mut b = Builtin::new(48_000.0);
        let mut out = vec![7.0_f32; 256];
        b.render(&mut out, 128, 2);
        assert!(out.iter().all(|s| *s == 7.0), "silence wrote to the bus");
        assert!(!b.active());
    }

    /// A note makes a sound, and stops making one.
    #[test]
    fn a_note_sounds_and_then_stops() {
        let sr = 48_000.0;
        let mut b = Builtin::new(sr);
        b.note_on(60, 0.8);
        assert!(b.active());

        let mut out = vec![0.0_f32; 2 * 4096];
        b.render(&mut out, 4096, 2);
        assert!(rms(&out) > 0.01, "a struck note was inaudible: {}", rms(&out));

        // Long past every decay constant in the file.
        for _ in 0..200 {
            let mut block = vec![0.0_f32; 2 * 4096];
            b.render(&mut block, 4096, 2);
        }
        assert!(!b.active(), "the note never stopped");
    }

    /// **It must not clip on its own.** Sixteen voices at once is a pedalled
    /// cluster, not a pathological case, and a built-in that distorts is worse
    /// than one that is quiet.
    #[test]
    fn a_full_chord_stays_inside_the_rails() {
        let sr = 48_000.0;
        let mut b = Builtin::new(sr);
        for i in 0..VOICES {
            b.note_on(48 + i as i16 * 3, 1.0);
        }
        let mut peak = 0.0_f32;
        for _ in 0..8 {
            let mut out = vec![0.0_f32; 2 * 1024];
            b.render(&mut out, 1024, 2);
            for s in out {
                peak = peak.max(s.abs());
            }
        }
        assert!(peak <= 1.0, "the built-in clipped at {peak}");
        assert!(peak > 0.05, "sixteen voices came out at {peak}");
    }

    /// A repeated note takes its own voice back, so a trill cannot eat the
    /// polyphony.
    #[test]
    fn a_repeated_note_reuses_its_voice() {
        let mut b = Builtin::new(48_000.0);
        for _ in 0..VOICES * 3 {
            b.note_on(64, 0.7);
        }
        let live = b.voices.iter().filter(|v| v.stage != Stage::Idle).count();
        assert_eq!(live, 1, "one key took {live} voices");
    }

    /// Render a phrase to a `.wav` so a human can hear it.
    ///
    /// Numbers prove it is not silent and does not clip. Only an ear proves it
    /// is worth shipping as the sound of a fresh install.
    ///
    ///   cargo test -p ivory --bins builtin_demo -- --ignored
    #[test]
    #[ignore = "writes a file for a person to listen to"]
    fn builtin_demo_wav() {
        const SR: u32 = 48_000;
        let mut b = Builtin::new(SR as f32);
        let mut left = Vec::<f32>::new();

        // A ii-V-I in C, voiced the way a pianist would, then a single low note
        // to hear the decay on its own.
        let phrase: [(&[i16], f32); 4] = [
            (&[50, 57, 60, 65, 69], 1.4),  // Dm9
            (&[43, 53, 57, 62, 64], 1.4),  // G13
            (&[48, 55, 59, 64, 67], 2.2),  // Cmaj9
            (&[36], 3.0),                  // low C, alone
        ];
        for (notes, secs) in phrase {
            for n in notes {
                b.note_on(*n, 0.85);
            }
            let frames = (secs * SR as f32) as usize;
            let mut block = vec![0.0_f32; frames * 2];
            b.render(&mut block, frames, 2);
            left.extend(block.chunks_exact(2).map(|f| f[0]));
            for n in notes {
                b.note_off(*n);
            }
        }

        let path = std::env::temp_dir().join("tangent-builtin.wav");
        let mut wav = Vec::<u8>::new();
        let data_len = (left.len() * 2) as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&SR.to_le_bytes());
        wav.extend_from_slice(&(SR * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in &left {
            wav.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
        }
        std::fs::write(&path, wav).expect("write the demo");
        println!("wrote {}", path.display());
    }

    /// The pedal holds what the player let go of, and lifting it lets them go.
    #[test]
    fn the_pedal_holds_notes_until_it_lifts() {
        let mut b = Builtin::new(48_000.0);
        b.set_pedal(true);
        b.note_on(60, 0.8);
        b.note_off(60);
        assert!(
            b.voices.iter().any(|v| v.stage == Stage::Held),
            "the pedal did not hold the note"
        );
        b.set_pedal(false);
        assert!(
            b.voices.iter().any(|v| v.stage == Stage::Released),
            "lifting the pedal did not release it"
        );
    }
}
