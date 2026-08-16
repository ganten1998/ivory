//! The audio graph: how a live input, a hosted instrument, or **both** become
//! the one stream a take is written from.
//!
//! ```text
//!             ┌────────────┐
//!  MIDI  ─────►  plugin    ├──► plugin_buf ──┐
//!             └────────────┘                 │   ┌───────┐
//!                                            ├──►│ mixer ├──┬──► WAV writer
//!             ┌────────────┐                 │   └───────┘  ├──► video sinks' audio
//!  input  ────►  cpal in   ├──► input_buf ───┘   gains,     └──► monitor out
//!             └────────────┘                     per source
//! ```
//!
//! RECORDER-PLAN §4a. **The mixer runs on the writer thread**, never on either
//! callback: it is the only place both buffers exist and it is already the
//! thread that owns the WAV writer.
//!
//! # This file does not know what a plugin or a device is
//!
//! There is no `cpal`, no `vst3`, no platform `cfg` and no `unsafe` below this
//! line, and there never should be. Everything enters through [`AudioSource`].
//! [`SourceRole`] names the words `input` and `plugin` only because §4a's
//! three-way mode is a **user-facing setting** (`record_audio_source`) that has
//! to be routed somewhere; the graph never learns what either one *is*.
//!
//! # The traps, each with a test below
//!
//! 1. **A live input and a plugin have opposite push/pull character.** The input
//!    already happened — its samples are sitting in a ring that a callback filled
//!    and it can only ever hand over what has arrived. The plugin has not
//!    happened yet — it renders exactly what it is asked for, when it is asked.
//!    [`AudioSource`] is therefore **pull-shaped with a caller-owned buffer**, so
//!    the block size is the mixer's decision (§4a decision 4 makes it a contract
//!    fixed for the take) and neither side allocates per block. A push-shaped
//!    trait would let each source pick its own block length, and two sources with
//!    different block lengths cannot be summed without a second layer of
//!    buffering that is exactly this module again.
//! 2. **A source that comes up short must not shorten the block.** A ring that is
//!    momentarily empty yields 40 frames when 512 were asked for. Returning a
//!    512-frame block containing 40 frames of audio and 472 of silence keeps the
//!    WAV, the video sinks and the timeline in step and costs one dropout;
//!    returning a 40-frame block slides *everything downstream* 472 frames early
//!    and never puts it back. Silence is the recoverable failure.
//! 3. **PLUGIN DELAY COMPENSATION IS NOT A REFINEMENT.** In `Both` mode against
//!    the same instrument — a digital piano's line-out *and* a plugin rendering
//!    the same MIDI — a misalignment of a few milliseconds is not heard as a
//!    delay. It is **comb filtering**: the two copies cancel at every frequency
//!    whose period is twice the offset, and the result is a tonal change. Users
//!    report it as "the plugin sounds thin" and go looking for an EQ. So every
//!    source declares its latency in frames and the graph delays the *earlier*
//!    ones to match the latest.
//! 4. **A reported latency changes.** A plugin may revise it (a sampler that has
//!    finished streaming, a linear-phase stage switching mode), and a step change
//!    in a delay line is a click. The change is crossfaded between the old and
//!    the new tap over [`PDC_FADE_SECONDS`]; a brief flange is the cheapest
//!    artefact available, and it is the only one that is not a click.
//! 5. **A muted source is still pulled.** The mode selector does not skip
//!    anybody: an input ring that stops being drained overflows and loses the
//!    samples that were in it, and a plugin that stops being rendered loses its
//!    release tails and its voice state — so switching back mid-take would fade
//!    in from nothing. Mode and gain are **level** decisions, never routing ones.
//! 6. **A gain step is a click**, whether it came from a fader or from a mode
//!    change. Every gain move is ramped across the block it lands in.
//! 7. **"Which one is clipping" is the question a user actually has.** Per-source
//!    metering is **pre-gain**, so a source that arrived over full scale is named
//!    even when its fader is down and the bus is clean — the damage was done at
//!    the converter and no fader undoes it. The bus is metered post-sum. When the
//!    bus overs and no source does, the answer is [`ClipBlame::Sum`]: turn a gain
//!    down, do not touch the interface.
//! 8. **The mix bus is never clamped.** `wav.rs` clamps on the way to an integer
//!    file and deliberately does not for float, because float WAV is defined past
//!    ±1.0 and an over can be pulled back down losslessly. Clamping here would
//!    destroy that before the writer ever sees it, and would do it in the one
//!    place that has no idea what the file format is.
//! 9. **One `NaN` from one plugin silences the whole mix**, because `NaN + x` is
//!    `NaN` and every comparison against it is false — so it would not even raise
//!    the clip latch. Non-finite samples are zeroed **at the door**, before the
//!    delay line, because a `NaN` written into a delay line comes back out again
//!    for as long as the delay is deep.
//! 10. **A source with more channels than the take is truncated, not folded
//!     down.** Summing a stereo instrument whose two channels are near-identical
//!     is +6 dB, which clips a take that metered fine; summing the eight outputs
//!     of a multi-out sampler is worse. Truncation cannot change the level, and
//!     the count of discarded channels is reported so the UI can say so.

use std::cmp::Ordering;
use std::fmt;

use crate::audio::{LevelTracker, Meters};
use crate::clock::{NS_PER_SEC, Nanos};

/// How long a plugin-delay-compensation change takes to cross over.
///
/// 5 ms is long enough that the seam is a flange rather than a click and short
/// enough that nobody hears a five-millisecond amplitude dip. It is not longer
/// because the two taps are decorrelated by construction — that is the entire
/// point of the change — so the crossfade region is the artefact and shortening
/// it is what makes it cheap.
pub const PDC_FADE_SECONDS: f64 = 0.005;

/// How much delay-line history is kept ready, in seconds.
///
/// **This is what decides whether a latency increase stutters or gouges a hole.**
/// When a plugin revises its latency upward by `d`, the other sources must now
/// emit audio from `d` frames ago — audio they have already emitted. A line that
/// kept that history re-emits it: a short stutter under the crossfade, and the
/// signal stays continuous. A line that did not can only emit silence, and then
/// steps back into the signal `d` frames later at whatever amplitude it happens
/// to have reached. Quarter of a second covers every instrument plugin latency
/// worth the name; past it the line grows and the honest fallback is the hole.
pub const PDC_HISTORY_SECONDS: f64 = 0.25;

/// The largest reported latency that is believed, in seconds.
///
/// A plugin that reports its latency in milliseconds where samples were asked
/// for is off by a factor of 48, and a plugin that reports a garbage `u32` would
/// otherwise size a delay line in gigabytes on the writer thread. Two seconds is
/// past any real instrument and past any look-ahead limiter; the clamp is
/// counted in [`SourceStats::latency_clamped`] so a plugin that trips it can be
/// named rather than silently mis-aligned.
pub const MAX_PDC_SECONDS: f64 = 2.0;

// ───────────────────────────────────────────────────────────────────────────
// What a source is
// ───────────────────────────────────────────────────────────────────────────

/// A block-oriented producer of interleaved `f32` audio.
///
/// **Pull-shaped on purpose**, and the shape is the design (module trap 1): the
/// mixer owns the block size and hands over the buffer, so a live input that can
/// only give what a callback already delivered and a plugin that renders on
/// demand implement the same three methods, and neither allocates per block.
///
/// # This is called from the writer thread and must never block
///
/// Not from an audio callback — but the writer thread is feeding the WAV writer
/// and the video sinks' audio inputs, so an implementation that waits on a
/// device stalls the whole take rather than just itself. A source with nothing
/// ready returns fewer frames; that is what [`fill`](Self::fill)'s return value
/// is for and the mixer pads the rest with silence.
///
/// Implementors are also the ones that know their own latency, which is why
/// [`latency_frames`](Self::latency_frames) is here rather than being told to
/// the mixer by a third party: two places holding one number is how a plugin
/// that revises its latency ends up compensated against a stale copy.
pub trait AudioSource {
    /// Channels this source writes per frame. §4a decision 4 fixes it for the
    /// take; the graph survives a change anyway (it rebuilds the source's delay
    /// line and meters and counts it) because the alternative is interleaving at
    /// the wrong stride for the rest of the recording.
    fn channels(&self) -> usize;

    /// Write interleaved frames into `out` and return **how many frames** were
    /// written.
    ///
    /// `out` is exactly `frames * channels()` long. Writing fewer frames than
    /// asked for is legal and expected — it is what an empty ring looks like —
    /// and the caller pads the remainder with silence rather than shortening the
    /// block. Whatever is left beyond the returned frame count is ignored, so
    /// there is no need to zero it.
    fn fill(&mut self, out: &mut [f32]) -> usize;

    /// How many frames late this source's output is, relative to the moment the
    /// sound actually happened.
    ///
    /// A plugin reports the delay it adds; a capture path reports the delay the
    /// device and the buffer add (`cpal` 0.16 reports none, per RECORDER-PLAN
    /// §3a, which is why the take's manifest calls the input's latency *assumed*
    /// rather than *reported*). It is read every block, so revising it is
    /// supported and does not need to be announced.
    fn latency_frames(&self) -> usize {
        0
    }
}

/// Which of §1's three audio sources a given input is, for routing the
/// `record_audio_source` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceRole {
    /// An audio input device: the line-out of a digital piano, a mic, an
    /// interface.
    Input,
    /// A hosted instrument rendering the incoming MIDI.
    Plugin,
}

/// The `record_audio_source` setting, as the mixer sees it.
///
/// This selects **levels, not routing** — see module trap 5. Every source is
/// pulled every block whatever the mode says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SourceMode {
    #[default]
    Input,
    Plugin,
    Both,
}

impl SourceMode {
    pub fn includes(self, role: SourceRole) -> bool {
        matches!(
            (self, role),
            (SourceMode::Both, _)
                | (SourceMode::Input, SourceRole::Input)
                | (SourceMode::Plugin, SourceRole::Plugin)
        )
    }

    /// The string written into `~/.config/ivory/settings.json`.
    pub fn to_setting(self) -> &'static str {
        match self {
            SourceMode::Input => "input",
            SourceMode::Plugin => "plugin",
            SourceMode::Both => "both",
        }
    }

    /// Absent-means-default, and **unknown-means-default too**: a settings file
    /// written by a build that had a fourth mode must not leave a returning user
    /// with no audio at all. `Input` is the source that needs no configuration.
    pub fn from_setting(s: &str) -> Self {
        match s {
            "plugin" => SourceMode::Plugin,
            "both" => SourceMode::Both,
            _ => SourceMode::Input,
        }
    }
}

/// Identifies a source inside one [`Mixer`]. Not stable across mixers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SourceId(usize);

impl SourceId {
    pub fn index(self) -> usize {
        self.0
    }
}

/// Rate and channel count of the mix bus: what the WAV, the video sinks and the
/// monitor all receive.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MixSpec {
    pub sample_rate: f64,
    pub channels: usize,
}

impl Default for MixSpec {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            channels: 2,
        }
    }
}

impl MixSpec {
    /// Zero channels or a non-positive rate are clamped rather than refused:
    /// this is arrived at from a device query, and the failure mode of a zero
    /// here is a division by zero on the writer thread mid-take.
    pub fn new(sample_rate: f64, channels: usize) -> Self {
        Self {
            sample_rate: if sample_rate > 0.0 {
                sample_rate
            } else {
                Self::default().sample_rate
            },
            channels: channels.max(1),
        }
    }

    fn frames(&self, seconds: f64) -> usize {
        (self.sample_rate * seconds).round().max(1.0) as usize
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Per-source accounting
// ───────────────────────────────────────────────────────────────────────────

/// Everything that went wrong, or nearly wrong, on one source.
///
/// All of it is reported rather than logged, because every field here is a
/// question a user asks after the fact and `take.json` is where the answer has
/// to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceStats {
    /// What the source last said [`AudioSource::channels`] was.
    pub channels: usize,
    /// Channels beyond the take's, discarded rather than folded down (trap 10).
    pub channels_dropped: usize,
    /// Frames the source actually produced.
    pub frames_yielded: u64,
    /// Frames of silence the mixer invented because the source came up short.
    /// **This is a dropout count**, and it is the number that makes a clean clip
    /// latch trustworthy or not.
    pub frames_padded: u64,
    /// Blocks in which the source came up short at all.
    pub short_blocks: u64,
    /// Samples that were not finite and were zeroed at the door (trap 9).
    pub nonfinite_samples: u64,
    /// Blocks in which the source claimed to have written more frames than it
    /// was given room for. Ignored, and counted, because believing it is a
    /// buffer overrun.
    pub over_yields: u64,
    /// The latency the source currently reports, after clamping.
    pub latency_frames: usize,
    /// The compensating delay currently applied to this source.
    pub delay_frames: usize,
    /// Times the reported latency changed after the source was added.
    pub latency_revisions: u64,
    /// Times a reported latency was refused for being past [`MAX_PDC_SECONDS`].
    pub latency_clamped: u64,
    /// Times the source changed its channel count mid-take, which §4a decision 4
    /// says cannot happen.
    pub channel_changes: u64,
}

/// One source's published state: what it is, how loud it is, and what it did.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceMeters {
    pub id: SourceId,
    pub role: SourceRole,
    /// The fader position, not the mode. A source the mode excludes still shows
    /// its own gain, because that is what the fader is drawn at.
    pub gain: f32,
    /// **Pre-gain** levels: what arrived, not what was mixed (trap 7).
    pub meters: Meters,
    pub stats: SourceStats,
}

/// Where a clip happened, phrased as the answer rather than as the data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ClipBlame {
    #[default]
    Nothing,
    /// This source arrived over full scale. Its own fader will not fix it; the
    /// thing feeding it has to come down.
    Source(SourceId),
    /// Every source is clean and the sum is not. A gain is too high.
    Sum,
}

/// The snapshot the Recorder band reads, once per frame, under a `Mutex`.
///
/// One struct rather than a meter per source held separately, so that the band
/// can never draw a bus level from one instant beside a source level from
/// another — which is exactly how a user ends up told that nothing is clipping
/// while the clip light is on.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MixMeters {
    /// Post-sum, post-gain: the signal the WAV writer receives.
    pub bus: Meters,
    pub sources: Vec<SourceMeters>,
    pub blame: ClipBlame,
    /// Frames of delay the graph is adding to align the sources (see
    /// [`Mixer::alignment_frames`]).
    pub alignment_frames: usize,
}

// ───────────────────────────────────────────────────────────────────────────
// The delay line that does the compensating
// ───────────────────────────────────────────────────────────────────────────

/// A whole-frame delay with a crossfading tap.
///
/// Processes **in place**: the incoming frame is written into the ring before
/// the delayed frame is read out over it, so the caller's block is both input
/// and output and no second buffer exists to be sized, zeroed or forgotten.
/// That ordering is also what makes a delay of zero the identity rather than a
/// one-frame delay.
struct DelayLine {
    channels: usize,
    buf: Vec<f32>,
    cap_frames: usize,
    write: usize,
    delay: usize,
    fade_from: usize,
    fade_left: usize,
    fade_len: usize,
    /// At most one change waits behind an in-flight crossfade. A third tap
    /// would be needed to fade out of a fade, and latency revisions are a
    /// load-time event, not a stream of them.
    pending: Option<(usize, usize)>,
}

impl DelayLine {
    fn new(channels: usize, history_frames: usize) -> Self {
        let cap_frames = history_frames.max(1);
        Self {
            channels,
            buf: vec![0.0; cap_frames * channels],
            cap_frames,
            write: 0,
            delay: 0,
            fade_from: 0,
            fade_left: 0,
            fade_len: 0,
            pending: None,
        }
    }

    /// Grow the ring, preserving history in chronological order.
    ///
    /// The new space is silence, and that is the honest answer: it is the region
    /// the line was never asked to remember. See [`PDC_HISTORY_SECONDS`] for why
    /// it should almost never come to this.
    fn ensure_cap(&mut self, frames: usize) {
        if frames <= self.cap_frames {
            return;
        }
        let ch = self.channels;
        let old = self.cap_frames;
        let mut next = vec![0.0; frames * ch];
        for i in 0..old {
            let from = ((self.write + i) % old) * ch;
            next[i * ch..i * ch + ch].copy_from_slice(&self.buf[from..from + ch]);
        }
        self.buf = next;
        self.cap_frames = frames;
        self.write = old;
    }

    /// `fade_len` of 0 changes the tap instantly, which clicks. It exists so a
    /// test can prove the crossfade is load-bearing.
    fn set_delay(&mut self, delay: usize, fade_len: usize) {
        let target = self.pending.map_or(self.delay, |(d, _)| d);
        if delay == target {
            return;
        }
        if self.fade_left > 0 {
            self.pending = Some((delay, fade_len));
            return;
        }
        self.begin(delay, fade_len);
    }

    fn begin(&mut self, delay: usize, fade_len: usize) {
        self.ensure_cap(delay.max(self.delay) + 1);
        if fade_len == 0 {
            self.delay = delay;
            self.fade_left = 0;
            return;
        }
        self.fade_from = self.delay;
        self.delay = delay;
        self.fade_len = fade_len;
        self.fade_left = fade_len;
    }

    fn tap(&self, delay: usize) -> usize {
        (self.write + self.cap_frames - delay) % self.cap_frames
    }

    fn process(&mut self, block: &mut [f32]) {
        let ch = self.channels;
        if ch == 0 {
            return;
        }
        for frame in block.chunks_exact_mut(ch) {
            let w = self.write * ch;
            self.buf[w..w + ch].copy_from_slice(frame);

            let new = self.tap(self.delay) * ch;
            if self.fade_left > 0 {
                let old = self.tap(self.fade_from) * ch;
                let a = (self.fade_len - self.fade_left + 1) as f32 / self.fade_len as f32;
                for (c, s) in frame.iter_mut().enumerate() {
                    *s = self.buf[old + c] * (1.0 - a) + self.buf[new + c] * a;
                }
                self.fade_left -= 1;
            } else {
                frame.copy_from_slice(&self.buf[new..new + ch]);
            }

            self.write = (self.write + 1) % self.cap_frames;

            if self.fade_left == 0 {
                if let Some((d, f)) = self.pending.take() {
                    self.begin(d, f);
                }
            }
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The mixer
// ───────────────────────────────────────────────────────────────────────────

struct Entry {
    id: SourceId,
    role: SourceRole,
    source: Box<dyn AudioSource + Send>,
    gain: f32,
    /// Where the gain ramp currently is. Starts at `gain`, so adding a source
    /// mid-session does not fade it in from nothing, and only *changes* ramp.
    applied_gain: f32,
    channels: usize,
    scratch: Vec<f32>,
    delay: DelayLine,
    levels: LevelTracker,
    stats: SourceStats,
    history_frames: usize,
    sample_rate: f64,
}

impl Entry {
    fn reshape(&mut self, channels: usize, out_channels: usize, initial: bool) {
        self.channels = channels;
        self.scratch = Vec::new();
        self.delay = DelayLine::new(channels, self.history_frames);
        self.levels = LevelTracker::new(channels, self.sample_rate);
        self.stats.channels = channels;
        self.stats.channels_dropped = channels.saturating_sub(out_channels);
        if !initial {
            self.stats.channel_changes += 1;
        }
    }

    /// Pull one block and add it to `out`. `out` has already been zeroed.
    fn pull(&mut self, frames: usize, mode: SourceMode, out_ch: usize, out: &mut [f32]) {
        if self.source.channels() != self.channels {
            self.reshape(self.source.channels(), out_ch, false);
        }
        let ch = self.channels;
        if ch == 0 {
            return;
        }

        let need = frames * ch;
        if self.scratch.len() < need {
            // The block size is fixed for a take (§4a decision 4), so this
            // allocates on the first block and never again. It is on the writer
            // thread, which is allowed to allocate; a callback is not.
            self.scratch.resize(need, 0.0);
        }
        let target = if mode.includes(self.role) {
            self.gain
        } else {
            0.0
        };
        let g0 = self.applied_gain;
        let dg = target - g0;

        let block = &mut self.scratch[..need];
        let raw = self.source.fill(block);
        if raw > frames {
            self.stats.over_yields += 1;
        }
        let n = raw.min(frames);

        // Trap 9: at the door, before the delay line, before the meters. A
        // non-finite sample that reaches the sum takes the other source with it.
        let mut bad = 0u64;
        for s in block[..n * ch].iter_mut() {
            if !s.is_finite() {
                *s = 0.0;
                bad += 1;
            }
        }
        // Trap 2: the short yield is padded, never shortened. The pad is written
        // rather than left alone because this scratch still holds the previous
        // block, and re-emitting it would be a stutter that met every test for
        // "the right number of frames".
        block[n * ch..].fill(0.0);

        self.stats.nonfinite_samples += bad;
        self.stats.frames_yielded += n as u64;
        self.stats.frames_padded += (frames - n) as u64;
        self.stats.short_blocks += u64::from(n < frames);

        // Pre-gain (trap 7), and pre-delay: the delay changes when a transient
        // is heard, never how loud it was.
        self.levels.absorb(&block[..n * ch]);
        self.delay.process(block);

        for (f, frame) in block.chunks_exact(ch).enumerate() {
            // Trap 6: ramped across the block, landing exactly on the target at
            // its last frame, so a fader move or a mode change is a slew and not
            // a step.
            let g = if dg == 0.0 {
                g0
            } else {
                g0 + dg * ((f + 1) as f32 / frames as f32)
            };
            let dst = f * out_ch;
            for c in 0..out_ch {
                // Trap 10: mono fans out at unity (a peak-preserving copy, not a
                // -3 dB pan law, because a recorder's job is to hand back what it
                // was given); extra channels are dropped, never summed.
                let s = if ch == 1 {
                    frame[0]
                } else if c < ch {
                    frame[c]
                } else {
                    break;
                };
                out[dst + c] += s * g;
            }
        }
        self.applied_gain = target;
    }
}

/// Sums N sources into one interleaved stream, with per-source gain, plugin
/// delay compensation and level accounting on every source and on the bus.
pub struct Mixer {
    spec: MixSpec,
    mode: SourceMode,
    sources: Vec<Entry>,
    bus_levels: LevelTracker,
    alignment: usize,
    fade_frames: usize,
    history_frames: usize,
    max_latency_frames: usize,
    frames_rendered: u64,
}

impl fmt::Debug for Mixer {
    /// Hand-written because [`AudioSource`] deliberately does not require
    /// `Debug`: a plugin handle has nothing useful to print and demanding it
    /// would put a bound on the trait for the sake of a log line.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Mixer")
            .field("spec", &self.spec)
            .field("mode", &self.mode)
            .field("sources", &self.sources.len())
            .field("alignment_frames", &self.alignment)
            .field("frames_rendered", &self.frames_rendered)
            .finish()
    }
}

impl Mixer {
    pub fn new(spec: MixSpec) -> Self {
        Self {
            mode: SourceMode::default(),
            sources: Vec::new(),
            bus_levels: LevelTracker::new(spec.channels, spec.sample_rate),
            alignment: 0,
            fade_frames: spec.frames(PDC_FADE_SECONDS),
            history_frames: spec.frames(PDC_HISTORY_SECONDS),
            max_latency_frames: spec.frames(MAX_PDC_SECONDS),
            frames_rendered: 0,
            spec,
        }
    }

    /// Add a source at `gain`, at the head of its own delay line.
    ///
    /// `Send` is required **here** rather than on the trait, deliberately. The
    /// mixer lives on the writer thread and has to be moved there, so somebody
    /// has to promise that a plugin handle can cross a thread boundary — and
    /// that promise belongs to the adapter that knows the plugin's threading
    /// rules, not to this file, which would otherwise be quietly licensing it.
    pub fn add(
        &mut self,
        role: SourceRole,
        gain: f32,
        source: Box<dyn AudioSource + Send>,
    ) -> SourceId {
        let id = SourceId(self.sources.len());
        let mut entry = Entry {
            id,
            role,
            gain,
            applied_gain: gain,
            channels: 0,
            scratch: Vec::new(),
            delay: DelayLine::new(0, self.history_frames),
            levels: LevelTracker::new(0, self.spec.sample_rate),
            stats: SourceStats::default(),
            history_frames: self.history_frames,
            sample_rate: self.spec.sample_rate,
            source,
        };
        let channels = entry.source.channels();
        entry.reshape(channels, self.spec.channels, true);
        entry.stats.latency_frames = entry.source.latency_frames().min(self.max_latency_frames);
        self.sources.push(entry);
        self.realign();
        id
    }

    pub fn spec(&self) -> MixSpec {
        self.spec
    }

    pub fn mode(&self) -> SourceMode {
        self.mode
    }

    /// Changing the mode is a gain move, and is ramped like one (trap 6).
    pub fn set_mode(&mut self, mode: SourceMode) {
        self.mode = mode;
    }

    pub fn set_gain(&mut self, id: SourceId, gain: f32) {
        if let Some(e) = self.sources.get_mut(id.0) {
            e.gain = gain;
        }
    }

    pub fn gain(&self, id: SourceId) -> f32 {
        self.sources.get(id.0).map_or(0.0, |e| e.gain)
    }

    pub fn source_count(&self) -> usize {
        self.sources.len()
    }

    pub fn frames_rendered(&self) -> u64 {
        self.frames_rendered
    }

    /// Frames of delay the graph is adding to align the sources: the largest
    /// latency any source reports.
    ///
    /// **This is a term in the take's timeline, not an internal detail.** The mix
    /// bus carries sound that happened `alignment_frames` earlier than the frame
    /// it is being written into, so a take that ignores it puts the WAV that many
    /// frames behind the MIDI — which is the same class of error §3a is about,
    /// arriving from inside the app instead of from a device.
    pub fn alignment_frames(&self) -> usize {
        self.alignment
    }

    /// The same term in the timebase, for the take's sync report.
    pub fn alignment_nanos(&self) -> Nanos {
        (self.alignment as f64 / self.spec.sample_rate * NS_PER_SEC) as Nanos
    }

    /// Length of the plugin-delay-compensation crossfade, in frames. Setting it
    /// to zero makes latency changes click; it is a test hook, not a setting.
    pub fn set_fade_frames(&mut self, frames: usize) {
        self.fade_frames = frames;
    }

    /// Clear every clip latch and take peak for a new take.
    pub fn arm(&mut self) {
        self.bus_levels.arm();
        for e in &mut self.sources {
            e.levels.arm();
            let keep = e.stats;
            e.stats = SourceStats {
                channels: keep.channels,
                channels_dropped: keep.channels_dropped,
                latency_frames: keep.latency_frames,
                delay_frames: keep.delay_frames,
                ..SourceStats::default()
            };
        }
    }

    /// Re-read every source's latency and re-derive the delays.
    ///
    /// Polled rather than pushed: a plugin that revises its latency has no
    /// obligation to tell anybody, and one virtual call per source per block is
    /// nothing next to a block of audio.
    fn realign(&mut self) {
        let cap = self.max_latency_frames;
        let mut max = 0usize;
        for e in &mut self.sources {
            let reported = e.source.latency_frames();
            let latency = reported.min(cap);
            if reported > cap {
                e.stats.latency_clamped += 1;
            }
            if latency != e.stats.latency_frames {
                e.stats.latency_frames = latency;
                e.stats.latency_revisions += 1;
            }
            max = max.max(latency);
        }
        self.alignment = max;
        // Nothing has flowed yet on the first pass, so there is nothing to fade
        // from and a crossfade would only fade in from silence.
        let fade = if self.frames_rendered == 0 {
            0
        } else {
            self.fade_frames
        };
        for e in &mut self.sources {
            // Align by delaying the EARLIER sources up to the latest one. The
            // latest source is never delayed, so compensation never costs more
            // latency than the plugin already imposed.
            let delay = max - e.stats.latency_frames;
            e.stats.delay_frames = delay;
            e.delay.set_delay(delay, fade);
        }
    }

    /// Sum one block into `out` and return the number of frames written.
    ///
    /// `out` is fully zeroed first — including any trailing partial frame, which
    /// is never half-filled (`wav.rs` refuses to write one for the same reason:
    /// a half frame swaps left and right for the rest of the file). A caller
    /// reusing one buffer therefore cannot leak the previous block into a
    /// dropout, which is the bug that looks like a stutter and reads like a
    /// working mixer.
    pub fn render(&mut self, out: &mut [f32]) -> usize {
        let ch = self.spec.channels;
        out.fill(0.0);
        let frames = out.len() / ch;
        if frames == 0 {
            return 0;
        }
        self.realign();
        let mode = self.mode;
        for e in &mut self.sources {
            e.pull(frames, mode, ch, out);
        }
        // The bus is metered post-sum and post-gain and is NOT clamped (trap 8).
        self.bus_levels.absorb(&out[..frames * ch]);
        self.frames_rendered += frames as u64;
        frames
    }

    /// Copy the current levels and counters into `dst`, in place.
    ///
    /// In place and not returned, matching `audio.rs`: the UI holds one of these
    /// behind a `Mutex` and wants the latest value, not a history, and a fresh
    /// allocation per repaint on the writer thread is work done under no
    /// deadline for a number that is about to be overwritten.
    pub fn publish(&mut self, dst: &mut MixMeters) {
        self.bus_levels.publish(&mut dst.bus);
        if dst.sources.len() != self.sources.len() {
            dst.sources = self
                .sources
                .iter()
                .map(|e| SourceMeters {
                    id: e.id,
                    role: e.role,
                    gain: e.gain,
                    meters: Meters::new(e.channels),
                    stats: e.stats,
                })
                .collect();
        }
        for (e, s) in self.sources.iter_mut().zip(dst.sources.iter_mut()) {
            s.id = e.id;
            s.role = e.role;
            s.gain = e.gain;
            s.stats = e.stats;
            e.levels.publish(&mut s.meters);
        }
        dst.alignment_frames = self.alignment;
        dst.blame = blame(dst);
    }
}

/// A source that arrived clipped is always the answer, even when the bus is
/// clean, because a fader cannot un-clip a converter. Only when every source is
/// intact is a clipping bus the mix's own fault.
fn blame(m: &MixMeters) -> ClipBlame {
    let worst = m
        .sources
        .iter()
        .filter(|s| s.meters.any_clipped())
        .max_by(|a, b| {
            a.meters
                .loudest_take_peak()
                .partial_cmp(&b.meters.loudest_take_peak())
                .unwrap_or(Ordering::Equal)
        });
    match (worst, m.bus.any_clipped()) {
        (Some(s), _) => ClipBlame::Source(s.id),
        (None, true) => ClipBlame::Sum,
        (None, false) => ClipBlame::Nothing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    // No devices and no plugins below this line, and none is needed: every
    // property this module has is a property of arithmetic over blocks. A test
    // that needed hardware would be a test of somebody else's code.

    /// Emits the same value on every channel forever, and never comes up short.
    struct Constant {
        channels: usize,
        value: f32,
        latency: usize,
        pulls: Arc<AtomicU64>,
    }

    impl Constant {
        fn new(channels: usize, value: f32) -> Self {
            Self {
                channels,
                value,
                latency: 0,
                pulls: Arc::new(AtomicU64::new(0)),
            }
        }
    }

    impl AudioSource for Constant {
        fn channels(&self) -> usize {
            self.channels
        }
        fn latency_frames(&self) -> usize {
            self.latency
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            self.pulls.fetch_add(1, AtomicOrdering::Relaxed);
            out.fill(self.value);
            out.len() / self.channels
        }
    }

    /// Yields `yields` frames per block whatever it was asked for: an input ring
    /// that has not been refilled yet.
    struct ShortYield {
        channels: usize,
        yields: usize,
    }

    impl AudioSource for ShortYield {
        fn channels(&self) -> usize {
            self.channels
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            let frames = (out.len() / self.channels).min(self.yields);
            out[..frames * self.channels].fill(1.0);
            frames
        }
    }

    /// One impulse, `emit_at` frames into its own output, reporting `reports`
    /// frames of latency. Lying about the two independently is what lets a test
    /// prove that compensation is doing the work.
    struct Click {
        channels: usize,
        emit_at: usize,
        reports: usize,
        n: usize,
    }

    impl AudioSource for Click {
        fn channels(&self) -> usize {
            self.channels
        }
        fn latency_frames(&self) -> usize {
            self.reports
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            let ch = self.channels;
            let frames = out.len() / ch;
            out.fill(0.0);
            for f in 0..frames {
                if self.n + f == self.emit_at {
                    out[f * ch..f * ch + ch].fill(1.0);
                }
            }
            self.n += frames;
            frames
        }
    }

    /// A steady tone, so that a discontinuity is visible as a step.
    struct Sine {
        channels: usize,
        n: usize,
        cycle: usize,
    }

    impl AudioSource for Sine {
        fn channels(&self) -> usize {
            self.channels
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            let ch = self.channels;
            let frames = out.len() / ch;
            for f in 0..frames {
                let phase =
                    ((self.n + f) % self.cycle) as f32 / self.cycle as f32 * std::f32::consts::TAU;
                out[f * ch..f * ch + ch].fill(phase.sin());
            }
            self.n += frames;
            frames
        }
    }

    /// Silent, and revises its reported latency partway through the stream —
    /// which is a plugin that has finished loading its samples.
    struct Revising {
        channels: usize,
        n: usize,
        at: usize,
        before: usize,
        after: usize,
    }

    impl AudioSource for Revising {
        fn channels(&self) -> usize {
            self.channels
        }
        fn latency_frames(&self) -> usize {
            if self.n >= self.at {
                self.after
            } else {
                self.before
            }
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            out.fill(0.0);
            let frames = out.len() / self.channels;
            self.n += frames;
            frames
        }
    }

    /// A plugin that has gone unstable, which is a thing plugins do.
    struct NotANumber {
        channels: usize,
    }

    impl AudioSource for NotANumber {
        fn channels(&self) -> usize {
            self.channels
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            out.fill(f32::NAN);
            out.len() / self.channels
        }
    }

    /// Claims to have written more than it was given room for.
    struct Liar {
        channels: usize,
    }

    impl AudioSource for Liar {
        fn channels(&self) -> usize {
            self.channels
        }
        fn fill(&mut self, out: &mut [f32]) -> usize {
            out.fill(0.25);
            out.len() / self.channels + 64
        }
    }

    fn stereo() -> Mixer {
        Mixer::new(MixSpec::default())
    }

    /// Render `blocks` blocks of `frames` and return the whole stream.
    fn run(mixer: &mut Mixer, frames: usize, blocks: usize) -> Vec<f32> {
        let ch = mixer.spec().channels;
        let mut out = vec![0.0f32; frames * ch];
        let mut all = Vec::with_capacity(frames * ch * blocks);
        for _ in 0..blocks {
            mixer.render(&mut out);
            all.extend_from_slice(&out);
        }
        all
    }

    /// Take one channel out of an interleaved stream.
    fn chan(stream: &[f32], c: usize, ch: usize) -> Vec<f32> {
        stream.iter().skip(c).step_by(ch).copied().collect()
    }

    /// The lag at which `a` best matches `b`, positive when `a` is later.
    /// Written from the definition rather than from the mixer's opinion of it.
    fn best_lag(a: &[f32], b: &[f32], max_lag: isize) -> isize {
        let mut best = (f32::NEG_INFINITY, 0isize);
        for lag in -max_lag..=max_lag {
            let mut sum = 0.0f32;
            for (i, x) in a.iter().enumerate() {
                let j = i as isize - lag;
                if j >= 0 && (j as usize) < b.len() {
                    sum += x * b[j as usize];
                }
            }
            if sum > best.0 {
                best = (sum, lag);
            }
        }
        best.1
    }

    fn max_step(x: &[f32]) -> f32 {
        x.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max)
    }

    #[test]
    fn a_source_that_comes_up_short_is_padded_rather_than_shortening_the_block() {
        let mut m = stereo();
        m.add(
            SourceRole::Input,
            1.0,
            Box::new(ShortYield {
                channels: 2,
                yields: 40,
            }),
        );
        let mut out = vec![0.0f32; 512 * 2];
        let frames = m.render(&mut out);

        assert_eq!(frames, 512, "the block length is the mixer's, not the ring's");
        assert!(
            out[..40 * 2].iter().all(|s| *s == 1.0),
            "the frames that did arrive must be present"
        );
        assert!(
            out[40 * 2..].iter().all(|s| *s == 0.0),
            "the rest must be silence; shortening the block would slide the WAV, \
             the video sinks and the timeline 472 frames early and never put \
             them back"
        );

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.sources[0].stats.frames_padded, 472);
        assert_eq!(meters.sources[0].stats.short_blocks, 1);
    }

    #[test]
    fn a_padded_block_cannot_re_emit_the_previous_blocks_audio() {
        // The scratch buffer is reused, so the pad has to be written and not
        // merely left alone. A stutter of the last 472 frames would pass every
        // length assertion above.
        let mut m = stereo();
        m.add(
            SourceRole::Input,
            1.0,
            Box::new(ShortYield {
                channels: 2,
                yields: 40,
            }),
        );
        let mut out = vec![0.0f32; 512 * 2];
        m.render(&mut out);
        m.render(&mut out);
        assert!(
            out[40 * 2..].iter().all(|s| *s == 0.0),
            "the second block's pad must still be silence"
        );
    }

    #[test]
    fn a_mono_source_reaches_both_channels_of_a_stereo_take_at_its_own_level() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(1, 0.5)));
        let mut out = vec![0.0f32; 8 * 2];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "a peak-preserving copy: a -3 dB pan law would hand back a quieter \
             recording than the one that was made, which is not a recorder's job"
        );
    }

    #[test]
    fn a_source_with_more_channels_than_the_take_loses_the_extra_ones_rather_than_summing_them() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(8, 0.5)));
        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "folding eight outputs of a multi-out sampler down would be +18 dB \
             and would clip a take that metered fine"
        );

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(
            meters.sources[0].stats.channels_dropped, 6,
            "and the count has to be reported, or the UI cannot say why the \
             take is missing the plugin's other buses"
        );
    }

    #[test]
    fn a_take_with_fewer_channels_than_the_source_is_not_a_fold_down_either() {
        let mut m = Mixer::new(MixSpec::new(48_000.0, 1));
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.6)));
        let mut out = vec![0.0f32; 4];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.6).abs() < 1e-6),
            "summing a stereo piano whose channels are near-identical is +6 dB"
        );
    }

    #[test]
    fn plugin_delay_compensation_puts_the_same_click_at_lag_zero() {
        // The piano is struck at frame 500. The line-out delivers it at 500. The
        // plugin rendering the same MIDI delivers it at 500+120 and says so.
        let strike = 500usize;
        let latency = 120usize;

        let build = |input_gain: f32, plugin_gain: f32, reports: usize| {
            let mut m = stereo();
            m.add(
                SourceRole::Input,
                input_gain,
                Box::new(Click {
                    channels: 2,
                    emit_at: strike,
                    reports: 0,
                    n: 0,
                }),
            );
            m.add(
                SourceRole::Plugin,
                plugin_gain,
                Box::new(Click {
                    channels: 2,
                    emit_at: strike + latency,
                    reports,
                    n: 0,
                }),
            );
            m.set_mode(SourceMode::Both);
            m
        };

        // One mixer per source so the two contributions can be correlated
        // against each other; both mixers still hold both sources, so the
        // alignment each computes is the one the real graph computes.
        let mut only_input = build(1.0, 0.0, latency);
        let mut only_plugin = build(0.0, 1.0, latency);
        assert_eq!(only_input.alignment_frames(), latency);

        let a = chan(&run(&mut only_input, 256, 8), 0, 2);
        let b = chan(&run(&mut only_plugin, 256, 8), 0, 2);

        assert_eq!(
            best_lag(&a, &b, 400),
            0,
            "compensated, the two copies of one transient must coincide; every \
             frame of offset between them is comb filtering, which is heard as a \
             tonal change and misdiagnosed as a thin-sounding plugin"
        );
        assert_eq!(
            a.iter().position(|s| *s > 0.5),
            Some(strike + latency),
            "the input is delayed up to the plugin, never the other way round"
        );
    }

    #[test]
    fn without_a_reported_latency_the_same_click_arrives_twice() {
        // The control for the test above: identical, except the plugin does not
        // declare its latency. If this passed at lag 0 too, the previous test
        // would be measuring nothing.
        let strike = 500usize;
        let latency = 120usize;

        let build = |input_gain: f32, plugin_gain: f32| {
            let mut m = stereo();
            m.add(
                SourceRole::Input,
                input_gain,
                Box::new(Click {
                    channels: 2,
                    emit_at: strike,
                    reports: 0,
                    n: 0,
                }),
            );
            m.add(
                SourceRole::Plugin,
                plugin_gain,
                Box::new(Click {
                    channels: 2,
                    emit_at: strike + latency,
                    reports: 0,
                    n: 0,
                }),
            );
            m.set_mode(SourceMode::Both);
            m
        };

        let a = chan(&run(&mut build(1.0, 0.0), 256, 8), 0, 2);
        let b = chan(&run(&mut build(0.0, 1.0), 256, 8), 0, 2);
        assert_eq!(best_lag(&a, &b, 400), -(latency as isize));
    }

    #[test]
    fn in_both_mode_one_transient_sums_to_one_impulse_and_not_to_two() {
        // The same thing stated the way a listener meets it: two copies of one
        // attack, one impulse of twice the height, no second impulse anywhere.
        let strike = 300usize;
        let latency = 64usize;
        let mut m = stereo();
        m.add(
            SourceRole::Input,
            1.0,
            Box::new(Click {
                channels: 2,
                emit_at: strike,
                reports: 0,
                n: 0,
            }),
        );
        m.add(
            SourceRole::Plugin,
            1.0,
            Box::new(Click {
                channels: 2,
                emit_at: strike + latency,
                reports: latency,
                n: 0,
            }),
        );
        m.set_mode(SourceMode::Both);

        let left = chan(&run(&mut m, 256, 4), 0, 2);
        let hits: Vec<usize> = left
            .iter()
            .enumerate()
            .filter(|(_, s)| **s > 0.5)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(hits, vec![strike + latency]);
        assert!((left[strike + latency] - 2.0).abs() < 1e-6);
    }

    #[test]
    fn a_latency_revision_mid_stream_crossfades_instead_of_clicking() {
        // 96 frames is half a cycle of the tone, so the old and new taps are in
        // antiphase and a jump between them is the largest step this signal can
        // make. That is the worst case on purpose: a delay change lands wherever
        // it lands, and the one that is inaudible proves nothing.
        let build = || {
            let mut m = stereo();
            m.add(
                SourceRole::Input,
                1.0,
                Box::new(Sine {
                    channels: 2,
                    n: 0,
                    cycle: 192,
                }),
            );
            m.add(
                SourceRole::Plugin,
                1.0,
                Box::new(Revising {
                    channels: 2,
                    n: 0,
                    at: 2_000,
                    before: 0,
                    after: 96,
                }),
            );
            m.set_mode(SourceMode::Both);
            m
        };

        let mut faded = build();
        let smooth = chan(&run(&mut faded, 256, 16), 0, 2);
        assert_eq!(faded.alignment_frames(), 96, "the revision must be seen");

        let mut abrupt = build();
        abrupt.set_fade_frames(0);
        let jump = chan(&run(&mut abrupt, 256, 16), 0, 2);

        let natural = std::f32::consts::TAU / 192.0;
        assert!(
            max_step(&smooth) < natural * 2.0,
            "a crossfaded tap change must stay near the tone's own slope; got a \
             step of {} against a natural {natural}",
            max_step(&smooth)
        );
        assert!(
            max_step(&jump) > 1.0,
            "and the same change without the crossfade must be the click this \
             test exists to prevent; got {}",
            max_step(&jump)
        );
    }

    #[test]
    fn a_source_the_mode_excludes_is_still_pulled() {
        let src = Constant::new(2, 1.0);
        let pulls = Arc::clone(&src.pulls);
        let mut m = stereo();
        m.add(SourceRole::Plugin, 1.0, Box::new(src));
        m.set_mode(SourceMode::Input);

        run(&mut m, 128, 5);
        assert_eq!(
            pulls.load(AtomicOrdering::Relaxed),
            5,
            "an input ring that stops being drained overflows and a plugin that \
             stops being rendered loses its voice state; the mode is a level \
             decision, never a routing one"
        );
    }

    #[test]
    fn a_mode_change_is_ramped_rather_than_stepped() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 1.0)));
        m.set_mode(SourceMode::Input);
        run(&mut m, 512, 2);

        m.set_mode(SourceMode::Plugin);
        let after = chan(&run(&mut m, 512, 2), 0, 2);
        assert!(
            max_step(&after) < 0.01,
            "muting a source with a step is a click; got {}",
            max_step(&after)
        );
        assert_eq!(
            after[after.len() - 1],
            0.0,
            "and the ramp must actually arrive, inside one block"
        );
    }

    #[test]
    fn both_mode_sums_the_two_sources_at_their_own_gains() {
        let mut m = stereo();
        let input = m.add(SourceRole::Input, 0.5, Box::new(Constant::new(2, 1.0)));
        let plugin = m.add(SourceRole::Plugin, 0.25, Box::new(Constant::new(2, 1.0)));
        m.set_mode(SourceMode::Both);

        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.75).abs() < 1e-6));

        m.set_mode(SourceMode::Plugin);
        run(&mut m, 512, 1);
        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.25).abs() < 1e-6),
            "and the excluded source contributes nothing while keeping its fader"
        );
        assert_eq!(m.gain(input), 0.5);
        assert_eq!(m.gain(plugin), 0.25);
    }

    #[test]
    fn a_clipping_source_is_named_even_when_its_fader_has_hidden_it() {
        let mut m = stereo();
        let hot = m.add(SourceRole::Input, 0.1, Box::new(Constant::new(2, 1.0)));
        m.arm();
        run(&mut m, 256, 2);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert!(!meters.bus.any_clipped(), "the fader keeps the bus clean");
        assert_eq!(
            meters.blame,
            ClipBlame::Source(hot),
            "the converter clipped before the fader ever saw it, and no fader \
             move undoes that"
        );
        assert!(meters.sources[0].meters.any_clipped());
    }

    #[test]
    fn a_clipping_sum_of_clean_sources_is_blamed_on_the_sum() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.6)));
        m.add(SourceRole::Plugin, 1.0, Box::new(Constant::new(2, 0.6)));
        m.set_mode(SourceMode::Both);
        m.arm();
        run(&mut m, 256, 2);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert!(meters.bus.any_clipped());
        assert_eq!(
            meters.blame,
            ClipBlame::Sum,
            "neither source is over on its own, so the answer is 'turn a gain \
             down', not 'your interface is too hot'"
        );
    }

    #[test]
    fn the_mix_bus_is_not_clamped() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.9)));
        m.add(SourceRole::Plugin, 1.0, Box::new(Constant::new(2, 0.9)));
        m.set_mode(SourceMode::Both);
        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 1.8).abs() < 1e-6),
            "float WAV is defined past full scale and an over can be pulled back \
             down losslessly; clamping here would destroy that before wav.rs, \
             which is the only place that knows the word size, ever sees it"
        );
    }

    #[test]
    fn one_plugin_emitting_nan_does_not_silence_the_other_source() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.4)));
        m.add(SourceRole::Plugin, 1.0, Box::new(NotANumber { channels: 2 }));
        m.set_mode(SourceMode::Both);

        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(
            out.iter().all(|s| (*s - 0.4).abs() < 1e-6),
            "NaN + x is NaN, so an unstable plugin would take the piano's own \
             line-out down with it and would not even raise the clip latch"
        );

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.sources[1].stats.nonfinite_samples, 4 * 2);
    }

    #[test]
    fn a_nan_is_zeroed_before_the_delay_line_and_not_after_it() {
        // A NaN written into a delay line comes back out again for as long as
        // the delay is deep, so sanitising on the way out of the mix would leave
        // the line itself poisoned. The other source reports the latency here,
        // which is what puts the unstable one behind a 64-frame delay.
        let mut m = stereo();
        let mut late = Constant::new(2, 0.4);
        late.latency = 64;
        m.add(SourceRole::Input, 1.0, Box::new(late));
        m.add(SourceRole::Plugin, 1.0, Box::new(NotANumber { channels: 2 }));
        m.set_mode(SourceMode::Both);

        let stream = run(&mut m, 128, 4);
        assert_eq!(m.alignment_frames(), 64, "the NaN source must be delayed");
        assert!(stream.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_source_claiming_more_frames_than_it_was_given_is_not_believed() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Liar { channels: 2 }));
        let mut out = vec![0.0f32; 32 * 2];
        assert_eq!(m.render(&mut out), 32);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.sources[0].stats.over_yields, 1);
        assert_eq!(
            meters.sources[0].stats.frames_yielded,
            32,
            "believing the claim would read past the buffer that was handed over"
        );
    }

    #[test]
    fn an_absurd_reported_latency_is_clamped_rather_than_sized_into() {
        let mut m = stereo();
        let mut src = Constant::new(2, 0.0);
        src.latency = usize::MAX / 2;
        m.add(SourceRole::Input, 1.0, Box::new(src));
        run(&mut m, 64, 1);

        assert_eq!(
            m.alignment_frames(),
            96_000,
            "two seconds is past any real instrument; a plugin reporting \
             milliseconds where samples were asked for must not size a delay \
             line in gigabytes on the writer thread"
        );
        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert!(meters.sources[0].stats.latency_clamped > 0);
    }

    #[test]
    fn the_alignment_delay_is_reported_because_the_take_has_to_account_for_it() {
        let mut m = stereo();
        let mut src = Constant::new(2, 0.0);
        src.latency = 480;
        m.add(SourceRole::Plugin, 1.0, Box::new(src));
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.0)));
        run(&mut m, 64, 1);

        assert_eq!(m.alignment_frames(), 480);
        assert_eq!(
            m.alignment_nanos(),
            10_000_000,
            "10 ms of graph latency is 10 ms the WAV sits behind the MIDI if the \
             take does not subtract it"
        );
    }

    #[test]
    fn the_output_buffer_is_zeroed_including_a_trailing_partial_frame() {
        let mut m = stereo();
        let mut out = vec![7.0f32; 5];
        assert_eq!(m.render(&mut out), 2, "two whole stereo frames of five");
        assert!(
            out.iter().all(|s| *s == 0.0),
            "a half frame swaps left and right for the rest of the file, and a \
             reused buffer would otherwise leak the previous block into a dropout"
        );
    }

    #[test]
    fn a_mixer_with_no_sources_produces_silence_rather_than_a_panic() {
        let mut m = stereo();
        let mut out = vec![1.0f32; 16];
        assert_eq!(m.render(&mut out), 8);
        assert!(out.iter().all(|s| *s == 0.0));
        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.blame, ClipBlame::Nothing);
        assert!(meters.sources.is_empty());
    }

    #[test]
    fn a_silent_source_is_reported_as_silence_and_not_as_an_absence() {
        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.0)));
        m.arm();
        run(&mut m, 512, 4);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.sources[0].stats.frames_yielded, 2_048);
        assert_eq!(meters.sources[0].stats.frames_padded, 0);
        assert!(
            meters.sources[0].meters.recorded_silence(),
            "the whole 'I recorded silence' failure class dies on this being \
             visible per source rather than only on the bus"
        );
        assert!(meters.bus.recorded_silence());
    }

    #[test]
    fn arming_clears_the_latch_without_losing_what_the_graph_is_configured_as() {
        let mut m = stereo();
        let mut src = Constant::new(2, 1.0);
        src.latency = 240;
        m.add(SourceRole::Input, 1.0, Box::new(src));
        run(&mut m, 256, 2);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert!(meters.sources[0].meters.any_clipped());

        m.arm();
        m.publish(&mut meters);
        assert!(!meters.sources[0].meters.any_clipped());
        assert_eq!(meters.sources[0].stats.frames_yielded, 0);
        assert_eq!(
            meters.sources[0].stats.latency_frames, 240,
            "a new take does not un-learn the plugin's latency"
        );
    }

    #[test]
    fn a_source_that_changes_its_channel_count_is_survived_and_counted() {
        struct Widening {
            n: usize,
        }
        impl AudioSource for Widening {
            fn channels(&self) -> usize {
                if self.n >= 8 {
                    2
                } else {
                    1
                }
            }
            fn fill(&mut self, out: &mut [f32]) -> usize {
                out.fill(0.5);
                let frames = out.len() / self.channels();
                self.n += frames;
                frames
            }
        }

        let mut m = stereo();
        m.add(SourceRole::Input, 1.0, Box::new(Widening { n: 0 }));
        run(&mut m, 8, 3);

        let mut meters = MixMeters::default();
        m.publish(&mut meters);
        assert_eq!(meters.sources[0].stats.channels, 2);
        assert_eq!(
            meters.sources[0].stats.channel_changes, 1,
            "§4a decision 4 says this cannot happen; interleaving at the wrong \
             stride for the rest of the take is what happens if it does and \
             nobody notices"
        );
    }

    #[test]
    fn a_source_reporting_no_channels_is_skipped_rather_than_dividing_by_zero() {
        let mut m = stereo();
        m.add(SourceRole::Plugin, 1.0, Box::new(Constant::new(0, 1.0)));
        m.add(SourceRole::Input, 1.0, Box::new(Constant::new(2, 0.3)));
        m.set_mode(SourceMode::Both);
        let mut out = vec![0.0f32; 4 * 2];
        m.render(&mut out);
        assert!(out.iter().all(|s| (*s - 0.3).abs() < 1e-6));
    }

    #[test]
    fn the_settings_string_round_trips_and_an_unknown_one_is_not_silence() {
        for mode in [SourceMode::Input, SourceMode::Plugin, SourceMode::Both] {
            assert_eq!(SourceMode::from_setting(mode.to_setting()), mode);
        }
        assert_eq!(
            SourceMode::from_setting("sidechain"),
            SourceMode::Input,
            "a settings file from a build with a fourth mode must not leave a \
             returning user with no audio at all"
        );
    }

    #[test]
    fn the_mode_decides_which_roles_are_audible() {
        assert!(SourceMode::Input.includes(SourceRole::Input));
        assert!(!SourceMode::Input.includes(SourceRole::Plugin));
        assert!(SourceMode::Plugin.includes(SourceRole::Plugin));
        assert!(!SourceMode::Plugin.includes(SourceRole::Input));
        assert!(SourceMode::Both.includes(SourceRole::Input));
        assert!(SourceMode::Both.includes(SourceRole::Plugin));
    }

    #[test]
    fn a_delay_of_zero_is_the_identity_and_not_a_one_frame_delay() {
        let mut line = DelayLine::new(2, 128);
        let mut block = vec![1.0, 2.0, 3.0, 4.0];
        line.process(&mut block);
        assert_eq!(block, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_delay_line_holds_its_history_across_blocks() {
        let mut line = DelayLine::new(1, 128);
        line.set_delay(3, 0);
        let mut a = vec![1.0, 2.0, 3.0, 4.0];
        line.process(&mut a);
        assert_eq!(a, vec![0.0, 0.0, 0.0, 1.0]);
        let mut b = vec![5.0, 6.0, 7.0, 8.0];
        line.process(&mut b);
        assert_eq!(
            b,
            vec![2.0, 3.0, 4.0, 5.0],
            "the tap has to survive the block boundary or every block edge is a \
             gap the length of the delay"
        );
    }

    #[test]
    fn a_delay_line_growing_past_its_history_keeps_what_it_had() {
        let mut line = DelayLine::new(1, 4);
        let mut a = vec![1.0, 2.0, 3.0, 4.0];
        line.process(&mut a);
        line.set_delay(6, 0);
        let mut b = vec![5.0, 6.0, 7.0, 8.0];
        line.process(&mut b);
        assert_eq!(
            b,
            vec![0.0, 0.0, 1.0, 2.0],
            "the two frames past the old history are the hole PDC_HISTORY_SECONDS \
             exists to avoid; the frames that were kept must still be in order"
        );
    }
}
