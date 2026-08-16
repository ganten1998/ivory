//! The monitor engine: one audio output stream that plays a hosted VST3
//! instrument and a metronome, and taps the instrument for the recorder.
//!
//! `ivory-host` could already load a plugin and render audio from it offline
//! (`examples/record_plugin.rs` writes a whole take that way). What it could not
//! do is make a sound, because Tangent had no audio OUTPUT anywhere — the
//! recorder's `audio.rs` opens an *input* stream and nothing else. A pianist has
//! to hear the piano while they play it, so this file exists.
//!
//! # There is exactly ONE output stream, and it is not optional
//!
//! Two cpal output streams on one device fight over it: on CoreAudio the second
//! one re-negotiates the device's buffer size out from under the first, and on
//! WASAPI shared mode they arrive at the mixer as two clients whose callbacks
//! are not phase-locked to each other. So the plugin and the click are summed
//! **in the same callback**, which is also the only way the click can be
//! sample-accurate against what the instrument is playing.
//!
//! And the stream's existence does not depend on a plugin. [`Engine::start`]
//! opens the device with nothing loaded; the metronome works immediately, and a
//! plugin is swapped in later while the stream keeps running. A metronome that
//! only works once you have found and loaded a VST3 is not a metronome.
//!
//! # The two mixes, which are not the same sum
//!
//! ```text
//!                    plugin gain
//!   plugin ──────────────►(×)──┬──────────────┬────► device mix ──► speakers
//!                              │              │        (+ click always)
//!                              │              │
//!   click ──►(×)───────────────┴──────────────┼────► recorder tap ──► the take
//!         metronome gain    (only if           │       (plugin only, by default)
//!                        metronome_in_take)    │
//! ```
//!
//! **The click reaches the speakers and NOT the take**, and that default is
//! deliberate: a click bleeding into the recording is a ruined take, and it is
//! ruined in a way you only discover on playback. [`Engine::set_metronome_in_take`]
//! turns it on for people who want a guide track. The consequence is that the
//! two sums are built separately, frame by frame, rather than the tap being a
//! branch off the finished monitor bus — tapping the final bus is how the click
//! gets into the take by accident six months from now.
//!
//! # Threading: the plugin renders IN the audio callback
//!
//! [`ivory_host::Instance`] holds `ComPtr`s and is `!Send`; cpal wants a
//! `Send + 'static` callback. Two shapes were available and this file takes the
//! first:
//!
//! (a) move the instance into the callback behind a newtype with an
//!     `unsafe impl Send`, so the plugin renders directly under the device's
//!     deadline — which is what every real host does; or
//! (b) render on a dedicated thread into a ring the callback drains, which
//!     avoids the `unsafe` and costs **one whole buffer of latency**.
//!
//! Measured on the owner's machine (Scarlett 18i20, CoreAudio): the device
//! granted the 256-frame buffer that [`Engine::start`] asks for, and the backend
//! reports **5.33 ms** between the callback and playback, which is exactly one
//! buffer. Shape (b) would double that before the interface's own converter
//! delay is counted at all. A pianist feels the difference. See [`PluginBox`]
//! for the safety argument, which is about *when* things move rather than about
//! the move itself.
//!
//! Latency in this file is reported and never assumed:
//! [`Engine::output_delay_ns`] is the backend's own number, and RECORDER-PLAN
//! §3a is explicit about why a claimed latency is worse than an absent one.
//!
//! # What the callback may not do, and how each rule is kept
//!
//! **Never allocate.** Every buffer here is sized once, in [`Engine::start`] or
//! in [`Engine::load_plugin`]: the render scratch, the device mix, the tap
//! scratch, the note list and all three rings. Pushes into `Vec`s are guarded by
//! their capacity rather than trusted. There is **one exception and it is not
//! this file's to remove**: `Instance::process` allocates internally on every
//! call — a `Vec` of channel pointers, an `AudioBusBuffers` vector, one scratch
//! buffer per channel of every output bus past the first, and a `ComWrapper`ed
//! event list. On Pianoteq, which exposes eight stereo buses, that is fourteen
//! same-sized allocations per block, every block. They are identical in size
//! every time, so a general-purpose allocator serves them from a free list and
//! it has never been heard here; it is still a landmine, and the fix is a
//! `process_into` on `Instance` that keeps its scratch between calls. Recorded
//! rather than hidden because someone will eventually hit it under memory
//! pressure and start looking in the wrong file.
//!
//! **Never lock.** Three `rtrb` SPSC rings and a handful of relaxed atomics. The
//! only `Mutex` in the file is written by cpal's *error* callback, which is a
//! different callback, with `try_lock` — exactly as `ivory-record`'s `audio.rs`
//! does it.
//!
//! **Never panic.** A panic across the cpal FFI boundary is undefined behaviour,
//! not a stack trace. Nothing in the render path indexes, unwraps, or divides by
//! a value it has not bounded first.
//!
//! # Where a note actually lands, stated plainly
//!
//! Events carry a host-timebase stamp and [`place`] converts it to a frame
//! offset inside the block. That machinery is real and tested — a stamp before
//! the block goes to offset 0, one after it is held for the next block, one
//! inside lands on its frame.
//!
//! **For live playing every event lands at offset 0, and no amount of
//! arithmetic changes that.** A block rendered at time T can only carry events
//! that were already known at T, so every one of them is by definition in the
//! past, and sub-block placement can only ever *delay* an event. Placement earns
//! its keep for events that are scheduled ahead of the clock, which is what
//! [`plugin_test`] plays and what a future MIDI-file player would.
//!
//! The real live latency is therefore not the block: it is **however often
//! [`Engine::send_midi`] is called**. Today the Recorder band drains the MIDI tap
//! once per UI frame, so a note waits up to ~16 ms before it is even queued. The
//! fix is not here — it is to call `send_midi` from `midi.rs`'s own `midir`
//! callback, which is why it takes `&self` and pushes into a lock-free ring
//! instead of taking `&mut self`. Until then the audio path's own 8 ms is the
//! smaller half of what the player feels.
//!
//! # The one thing a pianist will notice missing
//!
//! **The sustain pedal does not reach the plugin.** `Instance::process` takes
//! `&[Note]` and nothing else, so there is no door for a control change: VST3
//! delivers CC64 either as a parameter change through `IMidiMapping` or as a
//! legacy MIDI CC event, and `ivory-host` implements neither. Every pedal message
//! is counted by [`Engine::pedal_dropped`] rather than silently discarded, so the
//! band can say so instead of the user discovering it mid-phrase. This is the
//! first thing to fix in `ivory-host` after this lands.

// This module is a public API with no caller yet: the Recorder band that will
// drive it is being written alongside it, and `main.rs` does not wire
// `plugin_test` to a CLI flag (its own docs carry the four lines that do). A
// binary crate reports every unreached `pub` item as dead, so without this the
// build is fifty warnings deep and a real one could hide among them. **Delete
// this line the moment the band lands**: after that, anything it reports is a
// genuinely orphaned control.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ivory_host::{Control, Instance, Module, Note, Setup};
use ivory_record::audio::{Timebase, CLIP_LEVEL, DEFAULT_RING_SECONDS};
use ivory_record::clock::Nanos;
use rtrb::{Consumer, Producer, RingBuffer};

// ───────────────────────────────────────────────────────────────────────────
// Constants
// ───────────────────────────────────────────────────────────────────────────

/// Frames per `process` call, fixed for the life of an instance.
///
/// 512 because that is the number every measurement in RECORDER-PLAN §8 was
/// taken at, and because the plugin sizes its internal buffers from it at
/// `setupProcessing` — asking for more later is an error, not a slow path. A
/// device callback larger than this is split into several `process` calls; one
/// smaller renders a short block. Both are covered below.
const MAX_BLOCK: i32 = 512;

/// Output buffer asked of the device, in frames. 256 is 5.3 ms at 48 kHz, which
/// every CoreAudio and WASAPI device on this machine accepts, and it is the
/// dominant term in what a player feels from the audio path. Overridable with
/// `IVORY_OUT_BUFFER` for a machine that crackles at it.
const WANT_BUFFER_FRAMES: u32 = 256;

/// The largest device callback the mix scratch is sized for. Anything larger is
/// rendered in several passes rather than reallocating on the audio thread.
const MAX_CALLBACK_FRAMES: usize = 8192;

/// Note events carried into one `process` call. A burst larger than this is
/// spread over the following blocks; nothing is dropped, because a dropped
/// note-off is a note held until the app quits.
const MAX_EVENTS_PER_BLOCK: usize = 128;

/// Control changes buffered per block.
///
/// Far smaller than the note budget and deliberately so: there are three
/// pedals, and a continuous half-pedal sweep from a good keyboard is a few
/// messages per block at most. Overflow is counted, not dropped silently.
const MAX_CONTROLS_PER_BLOCK: usize = 32;

/// The recorder tap is **always stereo**, whatever the instrument renders.
///
/// It is fixed at [`Engine::start`] and never changes, which is the property
/// [`RecorderTap`] needs: the writer thread holds the tap across a plugin swap,
/// and a take whose channel count changes halfway through is not a file any tool
/// will open. A mono instrument is written to both channels; a multi-output one
/// contributes its first two. See [`map_frame`].
const TAP_CHANNELS: usize = 2;

/// Gain smoothing time constant. 10 ms is short enough to feel instant on a
/// fader and long enough that a jump from 0.0 to 1.0 is a fade rather than a
/// step discontinuity, which is a click in exactly the way the metronome is
/// supposed to have a monopoly on.
const GAIN_TAU_SECONDS: f64 = 0.010;

/// Release velocity for a note-off that carries none of its own.
///
/// 64/127, which is MIDI's own "no opinion" value. Not 0: several pianos map
/// release velocity to damper noise, and 0 there is a different sound rather
/// than a neutral one.
const DEFAULT_RELEASE_VELOCITY: f32 = 64.0 / 127.0;

/// How much faster the accented click is played back. `2^(4/12)` is a major
/// third up.
///
/// **Pitch and not level**, chosen deliberately: an accent made by playing the
/// same sample louder disappears the moment the user pulls the metronome gain
/// down or listens on anything with compression, whereas a pitch difference
/// survives both. It costs one extra resampled copy at startup and nothing at
/// all per frame.
const ACCENT_RATE_RATIO: f64 = 1.259_921_05;

/// Tempo bounds. Not taste: the beat clock counts down a frame at a time, so a
/// period below one frame would fire forever inside one callback.
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 300.0;

/// How long [`Engine::load_plugin`] waits for the audio thread to hand the old
/// instance back before giving up and letting the stream's own teardown drop it.
/// Twenty buffers at the slowest plausible size; if the callback has not run in
/// that time it is not running at all.
const RETIRE_TIMEOUT: Duration = Duration::from_millis(250);

/// Peak-hold fall rate, matching `ivory-record`'s meter so the two read the same.
const HOLD_DECAY_DB_PER_SEC: f32 = 20.0;

// ───────────────────────────────────────────────────────────────────────────
// What the caller sees
// ───────────────────────────────────────────────────────────────────────────

/// The instrument currently loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Loaded {
    pub bundle: PathBuf,
    pub class: String,
    pub vendor: String,
    /// Channels on the plugin's **main** output bus, which is the only bus this
    /// engine reads. Buses past it are rendered into scratch and discarded —
    /// they have to be rendered, see `Instance::process`.
    pub channels: u16,
    /// The rate the plugin was set up for, which is always the device's. Nothing
    /// is resampled.
    pub sample_rate: u32,
}

/// What the warm-up concluded, so the band can be honest about it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarmUp {
    /// `false` means the instrument never made a sound and was declared ready by
    /// timeout. The take may be silent and the UI should say so.
    pub heard: bool,
    pub elapsed: Duration,
    /// Loudest peak seen while warming up. A value just under the readiness
    /// threshold reads very differently from an exact zero.
    pub peak: f32,
}

/// The open output device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputInfo {
    pub device: String,
    pub channels: u16,
    pub sample_rate: u32,
    /// `None` when the device would not accept a fixed size and chose its own.
    pub buffer_frames: Option<u32>,
}

impl OutputInfo {
    /// One line for the band. Round-trip latency is not knowable from cpal 0.16
    /// (it reads neither `kAudioDevicePropertyLatency` nor the safety offset),
    /// so this reports the buffer and says nothing it cannot support.
    pub fn latency_line(&self) -> String {
        match self.buffer_frames {
            Some(f) if self.sample_rate > 0 => format!(
                "{} frames, {:.1} ms buffer at {} Hz",
                f,
                f as f32 * 1_000.0 / self.sample_rate as f32,
                self.sample_rate
            ),
            _ => format!("{} Hz, device buffer size", self.sample_rate),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// MIDI, crossing to the audio thread
// ───────────────────────────────────────────────────────────────────────────

/// One MIDI message on its way to the audio thread.
///
/// Fixed size and `Copy` on purpose. A `Vec<u8>` here would mean an allocation
/// on the sender and a deallocation **in the callback**, which is the rule this
/// whole file is arranged around. Three bytes covers every channel message; a
/// SysEx dump is not something an instrument monitor needs to forward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MidiEvent {
    stamp: Nanos,
    status: u8,
    data1: u8,
    data2: u8,
}

/// Raw MIDI bytes to a VST3 note, or `None` for anything that is not one.
///
/// Three traps, all of them cheap to get wrong and expensive to hear:
///
/// 1. **A note-on with velocity 0 IS a note-off.** Roughly half of all
///    keyboards release notes that way because it lets them stay in running
///    status, and a host that treats it as a note-on at silent velocity leaves
///    every note of the performance held down forever.
/// 2. **VST3 velocity is a float 0.0..=1.0, not a MIDI byte** (`instance.rs`
///    says so where `Note::velocity` is declared). Passing 100 makes every note
///    fortissimo and clipped, and it sounds like a broken plugin.
/// 3. **A byte below 0x80 is not a status byte.** midir delivers whole messages
///    and resolves running status itself, but a truncated or malformed buffer
///    must fall out here rather than be masked into a plausible note.
fn note_from_midi(status: u8, data1: u8, data2: u8) -> Option<Note> {
    if status < 0x80 {
        return None;
    }
    let pitch = i16::from(data1 & 0x7F);
    let velocity = data2 & 0x7F;
    match status & 0xF0 {
        0x90 if velocity != 0 => Some(Note {
            offset: 0,
            pitch,
            velocity: f32::from(velocity) / 127.0,
            on: true,
        }),
        // Trap 1.
        0x90 => Some(Note {
            offset: 0,
            pitch,
            velocity: DEFAULT_RELEASE_VELOCITY,
            on: false,
        }),
        0x80 => Some(Note {
            offset: 0,
            pitch,
            velocity: f32::from(velocity) / 127.0,
            on: false,
        }),
        _ => None,
    }
}

/// Sustain, sostenuto or soft: the three pedals, which cannot be delivered.
///
/// See the module docs. They are recognised only so they can be counted, and
/// they are counted so the gap is visible in the UI instead of being discovered
/// by a pianist in the middle of a phrase.
fn is_pedal(status: u8, data1: u8) -> bool {
    status >= 0x80 && status & 0xF0 == 0xB0 && matches!(data1, 64 | 66 | 67)
}

/// Where an event goes inside a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Placement {
    /// This frame offset within the block.
    At(i32),
    /// After this block ends. Hold it; do not drop it.
    Later,
}

/// Place a host-timebase stamp inside a block that starts at `block_start`.
///
/// An event older than the block goes to offset 0 rather than being dropped:
/// the audio for those frames has not been rendered yet, so offset 0 is the
/// earliest it can possibly sound, and a late note is a note, whereas a dropped
/// note-off is a stuck key.
fn place(stamp: Nanos, block_start: Nanos, rate: f64, frames: usize) -> Placement {
    let delta = stamp.saturating_sub(block_start);
    if delta <= 0 {
        return Placement::At(0);
    }
    if !(rate.is_finite() && rate > 0.0) {
        return Placement::At(0);
    }
    let offset = (delta as f64) * rate / 1e9;
    if offset >= frames as f64 {
        return Placement::Later;
    }
    // `as i32` on a value already bounded by `frames`, which is bounded by
    // MAX_CALLBACK_FRAMES. No saturation to worry about.
    Placement::At(offset as i32)
}

// ───────────────────────────────────────────────────────────────────────────
// Channel mapping
// ───────────────────────────────────────────────────────────────────────────

/// Map one frame of instrument output onto one frame of some destination.
///
/// The instrument decides its own channel count and the device decides its own,
/// and they are routinely different. The rules, all of them deliberate:
///
/// * **No source at all** (nothing loaded) writes silence, not stale samples.
/// * **Mono goes to every destination channel.** A mono instrument that only
///   appeared in the left speaker would be reported as a broken plugin.
/// * **Two or more sources fill the first two destination channels** and leave
///   the rest silent. Taking the first two of a multi-output instrument is the
///   right answer because bus 0 is the main mix by VST3 convention — the other
///   seven of Pianoteq's eight are stem outputs of the same performance, and
///   summing them would be the same piano eight times.
/// * **A mono destination gets the average, not the sum.** Summing is +6 dB and
///   clips a mix that was correct in stereo.
fn map_frame(src: &[f32], dst: &mut [f32]) {
    match (src.len(), dst.len()) {
        (0, _) => dst.fill(0.0),
        (1, _) => dst.fill(src[0]),
        (_, 0) => {}
        (_, 1) => dst[0] = (src[0] + src[1]) * 0.5,
        _ => {
            dst[0] = src[0];
            dst[1] = src[1];
            dst[2..].fill(0.0);
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The metronome
// ───────────────────────────────────────────────────────────────────────────

/// The click, decoded and resampled to the device rate once, at startup.
///
/// Immutable after construction and shared with the callback through an `Arc`,
/// so the audio thread only ever dereferences it.
struct Click {
    normal: Vec<f32>,
    accent: Vec<f32>,
}

impl Click {
    /// Decode `assets/click.wav` and prepare both voices at `rate`.
    ///
    /// The asset is a mono 48 kHz 24-bit file with a `JUNK` chunk in front of
    /// `fmt ` — see `ivory_record::wav::read_pcm`, which exists because of it.
    fn load(rate: f64) -> Result<Self, String> {
        let bytes = include_bytes!("../../assets/click.wav");
        let (spec, samples) = ivory_record::wav::read_pcm(bytes)?;
        if spec.channels != 1 {
            // Only mono is folded into every output channel below; a stereo
            // asset would be read as interleaved garbage at double speed.
            return Err(format!(
                "the click asset is {}-channel and the metronome needs mono",
                spec.channels
            ));
        }
        let src = f64::from(spec.sample_rate);
        Ok(Self {
            normal: resample_linear(&samples, src, rate),
            accent: resample_linear(&samples, src * ACCENT_RATE_RATIO, rate),
        })
    }
}

/// Linear interpolation, which is the right tool exactly once.
///
/// A click is a broadband transient about half a second long, played at most a
/// few times a second, and its whole job is to mark an instant. Linear
/// interpolation's error is high-frequency and the material is already
/// high-frequency noise; a polyphase resampler here would be a dependency and a
/// week for a difference nobody can hear. Anything that has to survive being
/// *recorded* would deserve better, and the click is the one signal that
/// deliberately does not reach the take.
fn resample_linear(src: &[f32], from: f64, to: f64) -> Vec<f32> {
    if src.is_empty() || !(from.is_finite() && from > 0.0) || !(to.is_finite() && to > 0.0) {
        return Vec::new();
    }
    if (from - to).abs() < f64::EPSILON {
        return src.to_vec();
    }
    let step = from / to;
    let out_len = ((src.len() as f64) / step).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    for i in 0..out_len {
        let pos = i as f64 * step;
        let a = pos as usize;
        let frac = (pos - a as f64) as f32;
        let s0 = src[a.min(src.len() - 1)];
        let s1 = src[(a + 1).min(src.len() - 1)];
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

/// One click, playing.
#[derive(Debug, Default)]
struct Voice {
    pos: usize,
    accent: bool,
    playing: bool,
}

impl Voice {
    /// Retrigger from the top. A beat that arrives while the previous click is
    /// still ringing cuts it off rather than layering, which is what a
    /// hardware metronome does and what keeps 300 bpm from turning into mush.
    fn trigger(&mut self, accent: bool) {
        self.pos = 0;
        self.accent = accent;
        self.playing = true;
    }

    fn next(&mut self, click: &Click) -> f32 {
        if !self.playing {
            return 0.0;
        }
        let buf = if self.accent {
            &click.accent
        } else {
            &click.normal
        };
        match buf.get(self.pos) {
            Some(s) => {
                self.pos += 1;
                *s
            }
            None => {
                self.playing = false;
                0.0
            }
        }
    }
}

/// What a beat is, when one fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Beat {
    accent: bool,
    /// 1-based count-in beat, or 0 for a free-running metronome beat.
    count_in: u32,
    /// This beat is the downbeat that ends a count-in: recording starts here.
    downbeat: bool,
}

/// The beat clock, counted in **frames on the audio thread**, never in
/// `Instant`.
///
/// A count-in timed off the UI clock drifts away from the click the player is
/// actually hearing, and "the countdown said 4 but the click had already gone"
/// is worse than no countdown at all. Here the beat and the click are the same
/// event by construction, because the click is triggered by this.
///
/// `to_next` is fractional and accumulates with `+=` rather than being reset, so
/// a period of 22050.7 frames does not round down 22050 every beat and lose a
/// beat every half hour.
#[derive(Debug)]
struct Beats {
    period: f64,
    to_next: f64,
    /// Beats since the phase was last restarted, for the bar accent.
    index: u64,
    count_in_left: u32,
    count_in_total: u32,
    /// The last count-in beat has sounded; the next beat is the downbeat.
    awaiting_downbeat: bool,
    /// The `count_in_req` generation this clock has acted on.
    seen_req: u64,
    /// Whether the free-running metronome was on last frame, so switching it on
    /// restarts the phase at a downbeat instead of wherever the count happened
    /// to be.
    was_on: bool,
}

impl Beats {
    fn new(rate: f64, bpm: f64) -> Self {
        Self {
            period: period_frames(rate, bpm),
            to_next: 0.0,
            index: 0,
            count_in_left: 0,
            count_in_total: 0,
            awaiting_downbeat: false,
            seen_req: 0,
            was_on: false,
        }
    }

    fn counting_in(&self) -> bool {
        self.count_in_left > 0 || self.awaiting_downbeat
    }

    /// Restart the phase so the next frame is beat 1.
    fn restart(&mut self, count_in: u32) {
        self.to_next = 0.0;
        self.index = 0;
        self.count_in_left = count_in;
        self.count_in_total = count_in;
        self.awaiting_downbeat = false;
    }

    /// Advance one frame. `Some` on the frame a beat lands.
    ///
    /// `on` is the free-running metronome switch; a count-in sounds whether it
    /// is on or not, because a count-in nobody can hear is not one.
    fn tick(&mut self, on: bool, beats_per_bar: u32) -> Option<Beat> {
        if !self.counting_in() {
            if on != self.was_on {
                self.was_on = on;
                if on {
                    self.restart(0);
                } else {
                    return None;
                }
            }
            if !on {
                return None;
            }
        }

        if self.to_next > 0.0 {
            self.to_next -= 1.0;
            return None;
        }
        // `period - 1.0`, not `period`: this frame IS the beat, so it counts
        // towards the next one. Adding the whole period here and decrementing
        // only on the frames that do not fire puts every beat one frame late,
        // which compounds — at 97 bpm it is 97 frames a minute, and it looks
        // exactly like the fractional-period drift this accumulator exists to
        // prevent.
        self.to_next += self.period - 1.0;

        let bar = beats_per_bar.max(1) as u64;
        let accent = self.index % bar == 0;
        self.index += 1;

        if self.count_in_left > 0 {
            self.count_in_left -= 1;
            let n = self.count_in_total - self.count_in_left;
            if self.count_in_left == 0 {
                self.awaiting_downbeat = true;
            }
            return Some(Beat {
                accent,
                count_in: n,
                downbeat: false,
            });
        }
        if self.awaiting_downbeat {
            self.awaiting_downbeat = false;
            // The downbeat always sounds: it is the "go" the whole count-in is
            // there to deliver. It also lands one beat period after the last
            // count-in beat, which is what "recording starts on the downbeat
            // after them" means.
            return Some(Beat {
                accent: true,
                count_in: 0,
                downbeat: true,
            });
        }
        Some(Beat {
            accent,
            count_in: 0,
            downbeat: false,
        })
    }
}

/// Frames per beat, with the tempo clamped to something the frame counter can
/// represent.
fn period_frames(rate: f64, bpm: f64) -> f64 {
    let bpm = if bpm.is_finite() {
        bpm.clamp(MIN_BPM, MAX_BPM)
    } else {
        120.0
    };
    let rate = if rate.is_finite() && rate > 0.0 {
        rate
    } else {
        48_000.0
    };
    (rate * 60.0 / bpm).max(1.0)
}

// ───────────────────────────────────────────────────────────────────────────
// The state both threads can see
// ───────────────────────────────────────────────────────────────────────────

/// Everything the UI thread and the audio thread share.
///
/// Atomics only, all `Relaxed`: nothing here orders anything else, and every
/// field is a number one side publishes and the other reads. `f32` and `f64`
/// travel as their bit patterns because there is no `AtomicF32`; the
/// conversions are exact both ways.
#[derive(Debug)]
struct Shared {
    plugin_gain: AtomicU32,
    metro_gain: AtomicU32,
    metro_on: AtomicBool,
    metro_in_take: AtomicBool,
    bpm: AtomicU64,
    beats_per_bar: AtomicU32,

    /// Bumped by `start_count_in`/`cancel_count_in`. The callback restarts its
    /// beat clock when this changes, which is how a request crosses without a
    /// lock and without a queue that could hold two.
    count_in_req: AtomicU64,
    count_in_beats: AtomicU32,
    /// 1-based beat currently sounding, or 0 for none.
    beat_now: AtomicU32,
    count_in_done: AtomicBool,
    /// Host-timebase instant the count-in downbeat will be **heard**, including
    /// the device's own output delay. `i64::MIN` for "no downbeat yet".
    downbeat_ns: AtomicI64,

    /// Peak magnitude since the UI last read it. `fetch_max` on the bit pattern
    /// is exactly max for non-negative floats, and the UI clears it with a
    /// `swap`, so a peak between two UI frames can never be missed. RMS is a
    /// plain store of the last callback's value: it is a texture, not an event.
    peak_l: AtomicU32,
    peak_r: AtomicU32,
    rms_l: AtomicU32,
    rms_r: AtomicU32,
    clipped: AtomicBool,

    /// How far ahead of the callback its audio is heard, as the backend
    /// reports it. Published every callback because it is the only latency
    /// number that is measured rather than assumed — see [`output_delay_ns`].
    delay_ns: AtomicI64,

    tap_dropped: AtomicU64,
    midi_dropped: AtomicU64,
    pedal_dropped: AtomicU64,
    /// Latched by the callback when `Instance::process` refuses a block. The
    /// message is a `&'static str` chosen by the reader, because building a
    /// `String` here would allocate on the audio thread.
    plugin_faulted: AtomicBool,
    running: AtomicBool,
    callbacks: AtomicU64,
    swaps: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            plugin_gain: AtomicU32::new(1.0f32.to_bits()),
            metro_gain: AtomicU32::new(0.7f32.to_bits()),
            metro_on: AtomicBool::new(false),
            // THE default the owner called out: the click is a monitor signal,
            // not a take signal.
            metro_in_take: AtomicBool::new(false),
            bpm: AtomicU64::new(120.0f64.to_bits()),
            beats_per_bar: AtomicU32::new(4),
            count_in_req: AtomicU64::new(0),
            count_in_beats: AtomicU32::new(0),
            beat_now: AtomicU32::new(0),
            count_in_done: AtomicBool::new(false),
            downbeat_ns: AtomicI64::new(i64::MIN),
            delay_ns: AtomicI64::new(0),
            peak_l: AtomicU32::new(0),
            peak_r: AtomicU32::new(0),
            rms_l: AtomicU32::new(0),
            rms_r: AtomicU32::new(0),
            clipped: AtomicBool::new(false),
            tap_dropped: AtomicU64::new(0),
            midi_dropped: AtomicU64::new(0),
            pedal_dropped: AtomicU64::new(0),
            plugin_faulted: AtomicBool::new(false),
            running: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
        }
    }

    fn f32_of(slot: &AtomicU32) -> f32 {
        f32::from_bits(slot.load(Ordering::Relaxed))
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The plugin, and getting it across
// ───────────────────────────────────────────────────────────────────────────

/// A loaded instrument plus the buffers rendering it needs.
///
/// **Field order is load-bearing.** Rust drops fields in declaration order, so
/// `inst` is declared before `module`: an `Instance` released after the factory
/// that made it is a call into a `ComPtr` whose owner has gone.
struct Hosted {
    inst: Instance,
    /// Held, never read: dropping the module unmaps the library and every
    /// `ComPtr` into it becomes a dangling function table. Declared AFTER
    /// `inst` so it is dropped after it.
    #[allow(dead_code, reason = "kept alive for the instance's sake, not read")]
    module: Module,
    /// One `Vec` per channel of the main output bus, each pre-grown to
    /// `MAX_BLOCK` so `Instance::process`'s internal `resize` never allocates.
    bufs: Vec<Vec<f32>>,
    channels: usize,
}

/// The `Instance` on its way into or out of the audio callback.
///
/// # SAFETY
///
/// `Hosted` contains an `ivory_host::Instance` and an `ivory_host::Module`,
/// both of which hold `ComPtr`s and are therefore `!Send`. This asserts `Send`
/// for the box that carries one across, and the assertion rests on four things:
///
/// 1. **It is moved, not shared.** The value travels through an `rtrb` SPSC
///    ring, which moves it out of the producer's slot and into the consumer's
///    hands. At no instant do two threads hold a reference: the UI thread's
///    copy is gone the moment `push` returns, and the callback's copy is gone
///    the moment it pushes it to the retire ring. There is no `Arc`, no
///    `&Hosted` stored anywhere, and no way to observe one from the other side.
/// 2. **It is moved exactly once in each direction, and the protocol enforces
///    it.** For every `PluginBox` the callback takes, it pushes exactly one
///    back; and it refuses to take one at all unless the retire ring has a free
///    slot ([`Renderer::swap_plugin`]). The UI thread waits for that return
///    before it drops anything ([`Engine::hand_off`]). So an instance is either
///    on the UI thread, in a ring, or in the callback — never in two places, and
///    never dropped twice.
/// 3. **The only VST3 call the audio thread makes is `process`.** The SDK
///    splits its API into an initialisation context and a processing context.
///    `initialize`, `setupProcessing`, `activateBus`, `setActive` and
///    `terminate` are initialisation-context calls and every one of them happens
///    on the UI thread — the first four inside `Instance::create` before the box
///    is handed over, `terminate` inside `Instance::drop` after it has been
///    handed back. `setProcessing` is the interesting case, because
///    `Instance::process` calls it itself and latches the result: the FIRST
///    block a new instance renders is the warm-up's, on the UI thread, so the
///    transition into the realtime section is already made before the handoff
///    and every later call is a no-op that never reaches the plugin. That is the
///    safer order of the two — the state transition is not racing the move — and
///    it leaves `process` as the only thing the audio thread ever calls, which
///    is exactly the call VST3 designates for it.
/// 4. **Nothing is dropped in the callback.** `Instance::drop` calls
///    `setProcessing(0)`, `setActive(0)` and `terminate`, and a commercial
///    plugin's `terminate` frees sample memory, joins worker threads and, for
///    several of them, unmaps files. That is unbounded work under a real-time
///    deadline. The callback therefore never drops a `PluginBox`; it returns it,
///    and [`Engine::hand_off`] drops it on the UI thread.
///
/// The one case the protocol cannot cover is a callback that stops running
/// while holding an instance — a device that vanished, or a stream that was
/// never started. Then [`Engine::hand_off`] times out and the instance is
/// dropped with the closure when the `cpal::Stream` drops. That happens on
/// whatever thread drops the stream, which for the macOS backend is this one
/// (`coreaudio-rs` stops and uninitialises the AudioUnit *before* freeing the
/// render callback box, so the audio thread is provably not inside it).
struct PluginBox(Option<Box<Hosted>>);

// SAFETY: see the type's documentation above. The four conditions there are the
// argument; this line is only where it is asserted.
unsafe impl Send for PluginBox {}

// ───────────────────────────────────────────────────────────────────────────
// The renderer: everything that runs on the audio thread
// ───────────────────────────────────────────────────────────────────────────

/// The audio callback's own state. Built on the UI thread, moved in once.
struct Renderer {
    shared: Arc<Shared>,
    timebase: Timebase,
    rate: f64,
    dev_channels: usize,

    /// The resident instrument, in the same newtype it travelled in. It has to
    /// be the newtype and not a bare `Option<Box<Hosted>>`: cpal requires the
    /// whole callback closure to be `Send`, so every field of this struct must
    /// be, and the `unsafe impl` on [`PluginBox`] is the one place that claim is
    /// made and argued.
    plugin: PluginBox,
    incoming: Consumer<PluginBox>,
    retiring: Producer<PluginBox>,

    midi: Consumer<MidiEvent>,
    /// One event popped but not yet due. `rtrb` has no peek, so an event that
    /// belongs to a later block lives here rather than being pushed back.
    pending: Option<MidiEvent>,
    notes: Vec<Note>,
    /// Control changes for this block — the sustain pedal, above all.
    ///
    /// Preallocated beside `notes` and for the same reason: `process` runs on
    /// the audio thread and a `Vec` that grows there allocates under a
    /// real-time deadline. A pedal is one message per press, so this never
    /// needs to be large.
    controls: Vec<Control>,

    tap: Producer<f32>,
    /// Interleaved recorder mix for one block. Sized once.
    tap_scratch: Vec<f32>,
    /// One frame of mapped instrument output. Sized once, to the larger of the
    /// device's channel count and the tap's.
    frame: Vec<f32>,

    click: Click,
    voice: Voice,
    beats: Beats,

    /// `process` calls made since the stream opened. Not a statistic: it is how
    /// the chunking test observes that a 4096-frame callback became eight
    /// blocks, which is not visible any other way without a real plugin.
    chunks: u64,
    /// Smoothed gains. One-pole per frame so a fader move is a fade, not a step.
    plugin_gain: f32,
    metro_gain: f32,
    gain_coeff: f32,
}

impl Renderer {
    /// Take a waiting plugin, if one is waiting and the old one can be returned.
    ///
    /// The order matters: the retire ring is checked for room *first*, because
    /// accepting a new instance with nowhere to put the old one would leave the
    /// callback holding two, and dropping one here is exactly what condition 4
    /// of [`PluginBox`]'s safety argument forbids.
    fn swap_plugin(&mut self) -> bool {
        if self.retiring.slots() == 0 {
            return false;
        }
        let Ok(next) = self.incoming.pop() else {
            return false;
        };
        let old = std::mem::replace(&mut self.plugin, next);
        self.shared.plugin_faulted.store(false, Ordering::Relaxed);
        self.shared.swaps.fetch_add(1, Ordering::Relaxed);
        // Cannot fail: `slots()` was checked above and this is the only
        // producer. `let _` rather than `expect` because a panic here is
        // undefined behaviour, not a bug report.
        let _ = self.retiring.push(old);
        true
    }

    /// Collect the events that belong to `[block_start, block_start + frames)`.
    ///
    /// Always drains, even with no plugin loaded: a queue left to fill up would
    /// fire a burst of stale note-ons the instant an instrument appeared.
    fn collect_notes(&mut self, block_start: Nanos, frames: usize) {
        self.notes.clear();
        self.controls.clear();
        loop {
            let event = match self.pending.take() {
                Some(e) => e,
                None => match self.midi.pop() {
                    Ok(e) => e,
                    Err(_) => break,
                },
            };
            let offset = match place(event.stamp, block_start, self.rate, frames) {
                Placement::Later => {
                    self.pending = Some(event);
                    break;
                }
                Placement::At(offset) => offset,
            };
            if self.notes.len() == self.notes.capacity() {
                // Full. Hold this one for the next block rather than pushing
                // (which reallocates on the audio thread) or dropping it (which
                // hangs a note). A hundred and twenty-eight events in one block
                // is a MIDI file being dumped, not a person playing.
                self.pending = Some(event);
                break;
            }
            if let Some(note) = note_from_midi(event.status, event.data1, event.data2) {
                self.notes.push(Note { offset, ..note });
            } else if is_pedal(event.status, event.data1) {
                // Delivered as a VST3 parameter change, which is the only door
                // VST3 leaves open for a CC — see `ivory-host`'s `Control`.
                // Whether it ARRIVES depends on the plugin publishing a
                // mapping; `Rendered::unmapped` is what counts the ones that
                // did not, so `pedal_dropped` now means "this instrument has no
                // pedal" rather than "we never tried".
                if self.controls.len() < self.controls.capacity() {
                    self.controls.push(Control {
                        offset,
                        controller: i16::from(event.data1),
                        value: u16::from(event.data2),
                        // The channel is NOT decoration: Pianoteq publishes a
                        // different parameter for each channel's CC64, so
                        // sending channel 0 for a keyboard transmitting on
                        // channel 3 moves the wrong piano.
                        channel: i16::from(event.status & 0x0F),
                    });
                } else {
                    self.shared.pedal_dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// Render `out.len() / dev_channels` frames of the device mix.
    ///
    /// `out` is interleaved at the device's channel count. `now` is the host
    /// instant the callback was entered and `heard` is the instant its first
    /// frame will actually leave the DAC.
    fn render(&mut self, out: &mut [f32], now: Nanos, heard: Nanos) {
        self.shared.callbacks.fetch_add(1, Ordering::Relaxed);
        self.shared
            .delay_ns
            .store(heard.saturating_sub(now), Ordering::Relaxed);
        self.swap_plugin();

        // Every sample inside `frames` is written below, so this is only here
        // for a buffer whose length is not a whole number of frames: its tail
        // must be silence and not whatever the driver last left there.
        out.fill(0.0);

        let dev_ch = self.dev_channels;
        if dev_ch == 0 {
            // `Engine::start` refuses a zero-channel device, so this is
            // unreachable — and it is written down anyway because the next line
            // is a division, an integer division by zero PANICS in Rust, and a
            // panic across the cpal boundary is undefined behaviour rather than
            // a stack trace.
            return;
        }
        // Clamped, not trusted: the scratch buffers below are sized for
        // MAX_CALLBACK_FRAMES and indexing past them would be the same panic in
        // a different costume. A device that hands over more than this gets its
        // tail as silence, which is audible and survivable; the caller in
        // `build_for` splits the callback so it never happens.
        let frames = (out.len() / dev_ch).min(MAX_CALLBACK_FRAMES);

        // Read the controls once per callback, not once per frame: they are
        // published by the UI at human speed and re-reading them 256 times
        // costs 256 atomic loads for a value that cannot have changed.
        let plugin_target = Shared::f32_of(&self.shared.plugin_gain).clamp(0.0, 8.0);
        let metro_target = Shared::f32_of(&self.shared.metro_gain).clamp(0.0, 8.0);
        let metro_on = self.shared.metro_on.load(Ordering::Relaxed);
        let in_take = self.shared.metro_in_take.load(Ordering::Relaxed);
        let bpb = self.shared.beats_per_bar.load(Ordering::Relaxed);
        self.beats.period = period_frames(
            self.rate,
            f64::from_bits(self.shared.bpm.load(Ordering::Relaxed)),
        );
        let req = self.shared.count_in_req.load(Ordering::Relaxed);
        if req != self.beats.seen_req {
            self.beats.seen_req = req;
            let want = self.shared.count_in_beats.load(Ordering::Relaxed);
            self.beats.restart(want);
            self.shared.count_in_done.store(want == 0, Ordering::Relaxed);
            self.shared.beat_now.store(0, Ordering::Relaxed);
            self.shared.downbeat_ns.store(i64::MIN, Ordering::Relaxed);
            self.voice.playing = false;
        }

        let mut peak_l = 0.0f32;
        let mut peak_r = 0.0f32;
        let mut sumsq_l = 0.0f64;
        let mut sumsq_r = 0.0f64;
        let mut clipped = false;
        let mut tap_frames = 0usize;

        let mut done = 0usize;
        while done < frames {
            let block = MAX_BLOCK as usize;
            let n = block.min(frames - done);
            self.chunks += 1;

            // Each chunk gets its own window in the host timeline, so an event
            // stamped inside the second half of a 1024-frame callback lands in
            // the second `process` call rather than being held for the next
            // callback entirely.
            let block_start = now + (done as f64 / self.rate * 1e9) as Nanos;
            self.collect_notes(block_start, n);

            let plugin_ch = self.render_plugin(n);

            for i in 0..n {
                let frame_index = done + i;

                if let Some(beat) = self.beats.tick(metro_on, bpb) {
                    self.voice.trigger(beat.accent);
                    self.shared
                        .beat_now
                        .store(beat.count_in, Ordering::Relaxed);
                    if beat.downbeat {
                        let at = heard + (frame_index as f64 / self.rate * 1e9) as Nanos;
                        self.shared.downbeat_ns.store(at, Ordering::Relaxed);
                        self.shared.count_in_done.store(true, Ordering::Relaxed);
                    }
                }

                self.plugin_gain += (plugin_target - self.plugin_gain) * self.gain_coeff;
                self.metro_gain += (metro_target - self.metro_gain) * self.gain_coeff;
                let click = self.voice.next(&self.click) * self.metro_gain;

                // ── the tap mix: instrument only, unless asked otherwise ──
                let pair = self.plugin_source(plugin_ch, i);
                // Sliced to the instrument's real width: handing `map_frame` a
                // two-element array for a MONO plugin would put a hard zero in
                // the right channel instead of the same signal.
                let src = &pair[..plugin_ch.min(2)];
                map_frame(src, &mut self.frame[..TAP_CHANNELS]);
                let tap_at = tap_frames * TAP_CHANNELS;
                for c in 0..TAP_CHANNELS {
                    let s = self.frame[c] * self.plugin_gain + if in_take { click } else { 0.0 };
                    self.tap_scratch[tap_at + c] = s;
                }
                tap_frames += 1;

                // ── the device mix: instrument plus click, always ──
                let at = frame_index * dev_ch;
                map_frame(src, &mut self.frame[..dev_ch]);
                for c in 0..dev_ch {
                    let s = self.frame[c] * self.plugin_gain + click;
                    out[at + c] = s;
                    let mag = s.abs();
                    if mag >= CLIP_LEVEL {
                        clipped = true;
                    }
                    if c == 0 {
                        peak_l = peak_l.max(mag);
                        sumsq_l += f64::from(s) * f64::from(s);
                    } else if c == 1 {
                        peak_r = peak_r.max(mag);
                        sumsq_r += f64::from(s) * f64::from(s);
                    }
                }
            }
            done += n;
        }

        self.push_tap(tap_frames);
        self.publish_meters(frames, peak_l, peak_r, sumsq_l, sumsq_r, clipped);
    }

    /// Render one chunk from the plugin and return how many channels it wrote.
    ///
    /// Zero means silence, and that is the normal state before an instrument is
    /// chosen. A block the plugin refuses latches the fault and returns zero
    /// rather than leaving the previous block's samples in the buffers, which
    /// would loop the last 10 ms forever at full level.
    fn render_plugin(&mut self, frames: usize) -> usize {
        if self.shared.plugin_faulted.load(Ordering::Relaxed) {
            return 0;
        }
        let notes = &self.notes;
        let controls = &self.controls;
        let Some(p) = self.plugin.0.as_mut() else {
            return 0;
        };
        match p
            .inst
            .process_with_controls(notes, controls, frames, &mut p.bufs)
        {
            Ok(rendered) => {
                // A control this instrument published no mapping for. Counted
                // rather than ignored, so the band can say "this plugin has no
                // pedal" instead of the user concluding the app has none.
                if rendered.unmapped > 0 {
                    self.shared
                        .pedal_dropped
                        .fetch_add(rendered.unmapped as u64, Ordering::Relaxed);
                }
                p.channels
            }
            Err(_) => {
                // The message is discarded on purpose: reading it means holding
                // a `String` the audio thread would have to free. `fault()`
                // turns this flag into words.
                self.shared.plugin_faulted.store(true, Ordering::Relaxed);
                0
            }
        }
    }

    /// One frame of instrument output, as a slice `map_frame` can read.
    ///
    /// A tiny fixed array rather than a `Vec`: this is called once per frame and
    /// the only sizes that matter are 0, 1 and 2.
    fn plugin_source(&self, channels: usize, i: usize) -> [f32; 2] {
        match (self.plugin.0.as_ref(), channels) {
            (Some(p), 1) => [p.bufs[0][i], 0.0],
            (Some(p), c) if c >= 2 => [p.bufs[0][i], p.bufs[1][i]],
            _ => [0.0, 0.0],
        }
    }

    /// Push the recorder's mix, whole frames only.
    ///
    /// **Only whole frames**, exactly as `ivory-record`'s capture ring insists:
    /// a ring-full event that split a frame would swap the take's channels from
    /// that point on and nothing downstream could detect it. When the ring is
    /// full the frames are counted and dropped, because a short take that says
    /// so beats a stalled audio thread.
    fn push_tap(&mut self, frames: usize) {
        if frames == 0 {
            return;
        }
        let want = frames * TAP_CHANNELS;
        let room = (self.tap.slots() / TAP_CHANNELS) * TAP_CHANNELS;
        let n = want.min(room);
        if n > 0 {
            let _ = self.tap.push_entire_slice(&self.tap_scratch[..n]);
        }
        if n < want {
            self.shared
                .tap_dropped
                .fetch_add(((want - n) / TAP_CHANNELS) as u64, Ordering::Relaxed);
        }
    }

    fn publish_meters(
        &self,
        frames: usize,
        peak_l: f32,
        peak_r: f32,
        sumsq_l: f64,
        sumsq_r: f64,
        clipped: bool,
    ) {
        let s = &self.shared;
        s.peak_l.fetch_max(peak_l.to_bits(), Ordering::Relaxed);
        s.peak_r.fetch_max(peak_r.to_bits(), Ordering::Relaxed);
        if frames > 0 {
            let n = frames as f64;
            s.rms_l
                .store(((sumsq_l / n).sqrt() as f32).to_bits(), Ordering::Relaxed);
            s.rms_r
                .store(((sumsq_r / n).sqrt() as f32).to_bits(), Ordering::Relaxed);
        }
        if clipped {
            s.clipped.store(true, Ordering::Relaxed);
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The recorder's read end
// ───────────────────────────────────────────────────────────────────────────

/// The recorder's half of the tap. `Send`, taken once, moved to the writer
/// thread.
///
/// It has to be a separate handle rather than a method on [`Engine`], because
/// the two live on different threads and always will: `Engine` owns a
/// `cpal::Stream` and a VST3 instance and is therefore pinned to the UI thread,
/// while `record.rs`'s writer runs on its own so that a file write can never
/// block a frame.
///
/// **It survives a plugin swap.** The ring belongs to the engine, not to the
/// instrument: [`RecorderTap::channels`] is fixed at [`Engine::start`] and a
/// different instrument is mapped into it (see [`map_frame`]) rather than
/// changing the width of a take that is already rolling.
pub struct RecorderTap {
    rx: Consumer<f32>,
    channels: usize,
    sample_rate: u32,
    dropped: Arc<Shared>,
}

impl RecorderTap {
    /// Append everything available. Interleaved; returns frames moved.
    pub fn drain(&mut self, out: &mut Vec<f32>) -> usize {
        let available = self.rx.slots();
        let whole = (available / self.channels) * self.channels;
        if whole == 0 {
            return 0;
        }
        let Ok(chunk) = self.rx.read_chunk(whole) else {
            return 0;
        };
        let (a, b) = chunk.as_slices();
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        chunk.commit_all();
        whole / self.channels
    }

    /// Append at most `frames` frames. Interleaved; returns frames moved.
    ///
    /// The bounded form, which is what the recorder wants: the take's length is
    /// decided by the INPUT device's timestamps, so each written block asks for
    /// exactly as many instrument frames as the block it is being summed into.
    /// Draining everything available would run the two clocks against each
    /// other and the ring would slowly empty or overflow depending on which
    /// device was faster.
    pub fn drain_frames(&mut self, frames: usize, out: &mut Vec<f32>) -> usize {
        let want = frames.saturating_mul(self.channels);
        let whole = (self.rx.slots().min(want) / self.channels) * self.channels;
        if whole == 0 {
            return 0;
        }
        let Ok(chunk) = self.rx.read_chunk(whole) else {
            return 0;
        };
        let (a, b) = chunk.as_slices();
        out.extend_from_slice(a);
        out.extend_from_slice(b);
        chunk.commit_all();
        whole / self.channels
    }

    /// Discard everything buffered and zero [`RecorderTap::dropped`].
    ///
    /// **Call this at the instant the take starts writing**, exactly as
    /// `ivory-record`'s `LevelTracker::arm` is called, and for the same reason.
    /// The ring runs from the moment the engine opens, so by the time a user has
    /// chosen an instrument, waited out its five-second warm-up and pressed
    /// Record, it holds minutes-old monitor audio and has overflowed several
    /// times over. Without this the first take of every session reports tens of
    /// thousands of dropped frames and `take.json` calls a perfectly good
    /// recording short. The probe measures exactly that: 80384 frames "lost"
    /// before a single note was played.
    ///
    /// It is deliberately not automatic. The ring keeps running between takes so
    /// that a pre-roll has something to reach back into, which is the whole
    /// reason it is four seconds deep rather than four blocks.
    pub fn arm(&mut self) {
        let waiting = self.rx.slots();
        if waiting > 0 {
            if let Ok(chunk) = self.rx.read_chunk(waiting) {
                chunk.commit_all();
            }
        }
        self.dropped.tap_dropped.store(0, Ordering::Relaxed);
    }

    /// Frames the ring could not hold. **Non-zero means the take is short**,
    /// and `take.json` has to say so — a silently short take is the worst
    /// outcome the recorder has.
    pub fn dropped(&self) -> u64 {
        self.dropped.tap_dropped.load(Ordering::Relaxed)
    }

    /// Always [`TAP_CHANNELS`], for the life of the engine.
    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

}

// ───────────────────────────────────────────────────────────────────────────
// The engine
// ───────────────────────────────────────────────────────────────────────────

/// The monitor engine: one output stream, one optional instrument, one
/// metronome.
///
/// `!Send` and `!Sync`, because `cpal::Stream` is. That is not a limitation to
/// work around — it is what makes the `RefCell` below sound and what pins the
/// VST3 initialisation-context calls to one thread, which is where VST3 wants
/// them.
pub struct Engine {
    // Declared first so it is dropped first: stopping the device before
    // anything it might still be reading goes away. Held, never read — the
    // whole value of this field is its `Drop`.
    #[allow(dead_code, reason = "held so the stream lives; dropping it stops audio")]
    stream: cpal::Stream,
    output: OutputInfo,
    shared: Arc<Shared>,
    timebase: Timebase,

    /// The sender half of the MIDI ring. `RefCell` rather than a `Mutex`: this
    /// type cannot cross threads, so there is nothing to lock against, and
    /// `send_midi` must not block. A `RefCell` borrow is a counter check with no
    /// atomics and no waiting.
    midi: RefCell<Producer<MidiEvent>>,

    to_audio: Producer<PluginBox>,
    from_audio: Consumer<PluginBox>,
    loaded: Option<Loaded>,
    warm: Option<WarmUp>,

    tap: Option<RecorderTap>,
    fault: Arc<Mutex<Option<String>>>,

    /// Meter hold, decayed on the UI thread because that is where it is read.
    hold: Cell<(f32, f32)>,
    hold_at: Cell<Instant>,
}

impl Engine {
    /// Open the default output device (or `out_device` by name) and start it.
    ///
    /// No plugin yet: the metronome works from here on, and an instrument is
    /// loaded into the running stream later.
    pub fn start(out_device: Option<&str>) -> Result<Self, String> {
        Self::start_with(out_device, Timebase::new())
    }

    /// As [`Engine::start`], sharing the recorder's timebase.
    ///
    /// Worth doing: it is what makes [`Engine::count_in_downbeat_ns`] comparable
    /// with the MIDI tap's stamps and with the take's `T0`, so a take can start
    /// on the downbeat the player actually heard rather than on the UI frame
    /// that noticed.
    pub fn start_with(out_device: Option<&str>, timebase: Timebase) -> Result<Self, String> {
        let host = cpal::default_host();
        let device = resolve_output(&host, out_device)?;
        let name = device
            .name()
            .unwrap_or_else(|_| "unnamed output device".to_string());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("{name}: no default output config ({e})"))?;

        let channels = supported.channels();
        let rate = supported.sample_rate().0;
        if channels == 0 || rate == 0 {
            return Err(format!("{name} reports {channels} channels at {rate} Hz"));
        }
        let (buffer_size, buffer_frames) = pick_buffer(supported.buffer_size());

        let shared = Arc::new(Shared::new());
        let fault = Arc::new(Mutex::new(None));
        let click = Click::load(f64::from(rate))?;

        // Every ring is allocated here and never again.
        let (midi_tx, midi_rx) = RingBuffer::<MidiEvent>::new(1024);
        let tap_frames = (DEFAULT_RING_SECONDS * rate as f32) as usize;
        let (tap_tx, tap_rx) = RingBuffer::<f32>::new(tap_frames.max(4096) * TAP_CHANNELS);
        // Two and two: at most one handoff is ever in flight, because
        // `hand_off` waits for the return before it starts another. The spare
        // slot is what lets `swap_plugin` check for room before it commits.
        let (to_audio, incoming) = RingBuffer::<PluginBox>::new(2);
        let (retiring, from_audio) = RingBuffer::<PluginBox>::new(2);

        let dev_ch = channels as usize;
        let widest = dev_ch.max(TAP_CHANNELS);
        let renderer = Renderer {
            shared: Arc::clone(&shared),
            timebase,
            rate: f64::from(rate),
            dev_channels: dev_ch,
            plugin: PluginBox(None),
            incoming,
            retiring,
            midi: midi_rx,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            controls: Vec::with_capacity(MAX_CONTROLS_PER_BLOCK),
            tap: tap_tx,
            tap_scratch: vec![0.0; MAX_CALLBACK_FRAMES * TAP_CHANNELS],
            frame: vec![0.0; widest],
            click,
            voice: Voice::default(),
            beats: Beats::new(f64::from(rate), 120.0),
            chunks: 0,
            plugin_gain: 1.0,
            metro_gain: 0.7,
            gain_coeff: gain_coefficient(f64::from(rate)),
        };

        let config = cpal::StreamConfig {
            channels,
            sample_rate: supported.sample_rate(),
            buffer_size,
        };
        let stream = build_stream(
            &device,
            &config,
            supported.sample_format(),
            renderer,
            Arc::clone(&shared),
            Arc::clone(&fault),
        )?;
        stream
            .play()
            .map_err(|e| format!("{name}: could not start the output stream ({e})"))?;
        shared.running.store(true, Ordering::Relaxed);

        Ok(Self {
            stream,
            output: OutputInfo {
                device: name,
                channels,
                sample_rate: rate,
                buffer_frames,
            },
            shared: Arc::clone(&shared),
            timebase,
            midi: RefCell::new(midi_tx),
            to_audio,
            from_audio,
            loaded: None,
            warm: None,
            tap: Some(RecorderTap {
                rx: tap_rx,
                channels: TAP_CHANNELS,
                sample_rate: rate,
                dropped: shared,
            }),
            fault,
            hold: Cell::new((0.0, 0.0)),
            hold_at: Cell::new(Instant::now()),
        })
    }


    // ── the instrument ──────────────────────────────────────────────────────

    /// Load an instrument into the running stream.
    ///
    /// **This blocks for about five seconds and must not be called from inside
    /// a frame.** Module load, instantiation and — the expensive part — the
    /// warm-up: RECORDER-PLAN §8 measured four of six instruments on this
    /// machine rendering silence if played immediately after instantiation, and
    /// all four fine five seconds later. `ivory_host::ready` is what decides
    /// that, run here through its blocking `warm_up` helper because this call is
    /// blocking by contract.
    ///
    /// The incremental alternative is available and is what the Recorder band
    /// should eventually use: own a `ready::Readiness`, step it a block at a
    /// time from this thread, and paint `status_line()` every frame so the
    /// fifteen-second worst case is a progress bar instead of a frozen window.
    /// It needs the instance to stay on this thread until the gate closes, which
    /// is exactly what happens here — the handoff is the last step, after the
    /// warm-up, not before it.
    pub fn load_plugin(
        &mut self,
        bundle: &Path,
        class_name: Option<&str>,
    ) -> Result<Loaded, String> {
        let module = Module::open(bundle)?;
        let classes = module.audio_modules();
        if classes.is_empty() {
            return Err(format!(
                "{} has no Audio Module Class, so it is an effect or a shell rather than an instrument",
                bundle.display()
            ));
        }
        let class = match class_name {
            Some(want) => classes
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(want))
                .or_else(|| {
                    let want = want.to_lowercase();
                    classes.iter().find(|c| c.name.to_lowercase().contains(&want))
                })
                .ok_or_else(|| {
                    format!(
                        "{} has no class matching {want:?}; it offers {}",
                        bundle.display(),
                        classes
                            .iter()
                            .map(|c| c.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                })?
                .clone(),
            None => classes[0].clone(),
        };

        let setup = Setup {
            sample_rate: f64::from(self.output.sample_rate),
            max_block: MAX_BLOCK,
        };
        let mut inst = Instance::create(&module, &class, setup)?;
        let channels = inst
            .audio_outputs()
            .first()
            .map(|b| b.channels.max(0) as usize)
            .unwrap_or(0);
        if channels == 0 {
            return Err(format!("{} has no audio output channels", class.name));
        }

        let gate = ivory_host::ready::warm_up(&mut inst, ivory_host::Policy::default());
        if gate.state() == ivory_host::ReadyState::Failed {
            return Err(gate
                .reason()
                .unwrap_or("the instrument failed to warm up")
                .to_string());
        }
        let warm = WarmUp {
            heard: gate.evidence() != Some(ivory_host::ready::Evidence::Timeout),
            elapsed: gate.elapsed(),
            peak: gate.peak_seen(),
        };

        let loaded = Loaded {
            bundle: bundle.to_path_buf(),
            class: class.name.clone(),
            vendor: module.vendor().to_string(),
            channels: channels as u16,
            sample_rate: self.output.sample_rate,
        };
        let hosted = Hosted {
            inst,
            module,
            // Pre-grown to a full block so `Instance::process`'s `resize` is a
            // length change and never an allocation.
            bufs: vec![vec![0.0; MAX_BLOCK as usize]; channels],
            channels,
        };

        self.hand_off(PluginBox(Some(Box::new(hosted))));
        self.loaded = Some(loaded.clone());
        self.warm = Some(warm);
        Ok(loaded)
    }

    /// Take the instrument out of the running stream and release it here.
    pub fn unload_plugin(&mut self) {
        if self.loaded.is_none() {
            return;
        }
        self.loaded = None;
        self.warm = None;
        self.hand_off(PluginBox(None));
    }

    /// Give the callback `next` and wait for whatever it was holding, so the old
    /// instance is dropped on **this** thread.
    ///
    /// Condition 4 of [`PluginBox`]'s safety argument in one function. The wait
    /// is bounded: a callback that is not running cannot return anything, and
    /// then the instance goes down with the stream instead.
    fn hand_off(&mut self, next: PluginBox) {
        // Anything already returned is dropped here, now, before another
        // handoff can fill the ring.
        while self.from_audio.pop().is_ok() {}
        if self.to_audio.push(next).is_err() {
            // Only reachable if a previous handoff was never collected, which
            // the wait below is there to prevent.
            return;
        }
        let deadline = Instant::now() + RETIRE_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(old) = self.from_audio.pop() {
                drop(old);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    pub fn plugin(&self) -> Option<&Loaded> {
        self.loaded.as_ref()
    }

    /// What the warm-up concluded. `heard: false` means the instrument never
    /// made a sound and was declared ready by timeout.
    pub fn warm_up(&self) -> Option<&WarmUp> {
        self.warm.as_ref()
    }

    pub fn output(&self) -> &OutputInfo {
        &self.output
    }

    // ── MIDI in ─────────────────────────────────────────────────────────────

    /// Queue a raw MIDI message for the audio thread.
    ///
    /// Never blocks, never allocates, never fails loudly: a full ring counts the
    /// loss in [`Engine::midi_dropped`] rather than stalling the caller. Takes
    /// `&self` so it can one day be called from `midi.rs`'s own `midir` callback
    /// instead of once per UI frame, which is where the latency actually is.
    ///
    /// A short or empty buffer is padded with zeroes and then ignored by
    /// [`note_from_midi`], which is the only place that decides what a message
    /// means.
    pub fn send_midi(&self, stamp_host_ns: Nanos, bytes: &[u8]) {
        let Some(&status) = bytes.first() else {
            return;
        };
        let event = MidiEvent {
            stamp: stamp_host_ns,
            status,
            data1: bytes.get(1).copied().unwrap_or(0),
            data2: bytes.get(2).copied().unwrap_or(0),
        };
        let Ok(mut tx) = self.midi.try_borrow_mut() else {
            self.shared.midi_dropped.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if tx.push(event).is_err() {
            self.shared.midi_dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Messages the MIDI ring could not hold.
    pub fn midi_dropped(&self) -> u64 {
        self.shared.midi_dropped.load(Ordering::Relaxed)
    }

    /// Pedal messages received and **not delivered to the instrument**, because
    /// `ivory-host` has no path for a control change. See the module docs; this
    /// is here so the band can say so rather than the user finding out.
    pub fn pedal_dropped(&self) -> u64 {
        self.shared.pedal_dropped.load(Ordering::Relaxed)
    }

    // ── the recorder's tap ──────────────────────────────────────────────────

    /// Take the recorder's read end. `None` if it has already been taken.
    pub fn take_recorder_tap(&mut self) -> Option<RecorderTap> {
        self.tap.take()
    }

    // ── metering ────────────────────────────────────────────────────────────

    /// Levels for the band's meter, measured on the **device mix** — what the
    /// player is hearing, click included.
    ///
    /// The peak is read-and-cleared, so a transient between two UI frames is
    /// never missed; the hold decays here rather than in the callback, at the
    /// same 20 dB/s `ivory-record`'s meter uses so the two read alike.
    pub fn meters(&self) -> ivory_ui::recorder::Meters {
        let s = &self.shared;
        let peak_l = f32::from_bits(s.peak_l.swap(0, Ordering::Relaxed));
        let peak_r = f32::from_bits(s.peak_r.swap(0, Ordering::Relaxed));
        let rms_l = Shared::f32_of(&s.rms_l);
        let rms_r = Shared::f32_of(&s.rms_r);

        let now = Instant::now();
        let dt = now.duration_since(self.hold_at.get()).as_secs_f32();
        self.hold_at.set(now);
        let decay = 10f32.powf(-HOLD_DECAY_DB_PER_SEC * dt / 20.0);
        let (hl, hr) = self.hold.get();
        let hold = ((hl * decay).max(peak_l), (hr * decay).max(peak_r));
        self.hold.set(hold);

        let mono = self.output.channels <= 1;
        ivory_ui::recorder::Meters {
            left: ivory_ui::recorder::Level {
                peak: peak_l,
                rms: rms_l,
                hold: hold.0,
            },
            right: ivory_ui::recorder::Level {
                peak: if mono { peak_l } else { peak_r },
                rms: if mono { rms_l } else { rms_r },
                hold: if mono { hold.0 } else { hold.1 },
            },
            mono,
            clipped: s.clipped.load(Ordering::Relaxed),
        }
    }

    /// Clear the clip latch. The *only* thing that does: a latch that clears
    /// itself is one the performer never sees, because they were looking at
    /// their hands when it happened.
    pub fn clear_clip(&self) {
        self.shared.clipped.store(false, Ordering::Relaxed);
    }

    // ── health ──────────────────────────────────────────────────────────────

    /// True while the device is open and the callback has not reported a fatal
    /// error.
    pub fn is_running(&self) -> bool {
        self.shared.running.load(Ordering::Relaxed)
    }

    /// Callbacks served. A count that stops climbing while [`Engine::is_running`]
    /// is true is a device that has gone quiet without saying so.
    pub fn callbacks(&self) -> u64 {
        self.shared.callbacks.load(Ordering::Relaxed)
    }

    /// **Measured** output latency: how far ahead of its callback the backend
    /// says a buffer will be heard.
    ///
    /// The only latency number in the product that is not an assumption. cpal
    /// 0.16 reads neither `kAudioDevicePropertyLatency` nor the safety offset
    /// (`ivory-record`'s `audio.rs` documents the dig), so the device's own
    /// converter delay is still outside it — but the difference between the
    /// callback instant and the playback instant is real, comes from the
    /// backend, and is what separates a 5 ms path from a 40 ms one. Zero means
    /// the backend declined to say.
    pub fn output_delay_ns(&self) -> Nanos {
        self.shared.delay_ns.load(Ordering::Relaxed)
    }

    /// The instrument fault first, because it is the actionable one; then
    /// whatever cpal last reported about the device.
    pub fn fault(&self) -> Option<String> {
        if self.shared.plugin_faulted.load(Ordering::Relaxed) {
            return Some(
                "the instrument stopped rendering; reload it or choose another".to_string(),
            );
        }
        self.fault.lock().ok().and_then(|g| g.clone())
    }

    // ── gains ───────────────────────────────────────────────────────────────

    /// Linear, not dB. 1.0 is unity; above that is deliberate make-up gain and
    /// is clamped at 8x in the callback so a fader dragged into a text field
    /// cannot produce a full-scale square wave.
    pub fn set_plugin_gain(&self, linear: f32) {
        self.shared
            .plugin_gain
            .store(sane_gain(linear).to_bits(), Ordering::Relaxed);
    }

    pub fn set_metronome_gain(&self, linear: f32) {
        self.shared
            .metro_gain
            .store(sane_gain(linear).to_bits(), Ordering::Relaxed);
    }



    // ── the metronome ───────────────────────────────────────────────────────

    /// Turn the free-running click on or off. Switching it on restarts the
    /// phase, so it always begins on an accented beat 1.
    pub fn set_metronome_enabled(&self, on: bool) {
        self.shared.metro_on.store(on, Ordering::Relaxed);
    }


    /// Whether the click is mixed into what the recorder captures.
    ///
    /// **False by default**, and that is the standard: a click bleeding into
    /// the take ruins it, and ruins it in a way nobody notices until playback.
    pub fn set_metronome_in_take(&self, on: bool) {
        self.shared.metro_in_take.store(on, Ordering::Relaxed);
    }


    pub fn set_tempo(&self, bpm: f64) {
        self.shared.bpm.store(bpm.to_bits(), Ordering::Relaxed);
    }


    /// Beats per bar, which decides only which beat is accented.
    pub fn set_beats_per_bar(&self, beats: u32) {
        self.shared
            .beats_per_bar
            .store(beats.max(1), Ordering::Relaxed);
    }

    /// Count `beats` in at `bpm`, then report [`Engine::count_in_done`].
    ///
    /// **Beats, not seconds** — the pre-roll is N clicks at the take's tempo and
    /// recording starts on the downbeat after them, which is one further beat
    /// period. The clicks and the countdown are the same event: both come from
    /// the audio thread's frame counter, so what the UI shows cannot drift from
    /// what the player hears.
    pub fn start_count_in(&self, beats: u32, bpm: f64) {
        let s = &self.shared;
        s.bpm.store(bpm.to_bits(), Ordering::Relaxed);
        s.count_in_beats.store(beats, Ordering::Relaxed);
        s.count_in_done.store(false, Ordering::Relaxed);
        s.beat_now.store(0, Ordering::Relaxed);
        s.downbeat_ns.store(i64::MIN, Ordering::Relaxed);
        // Published last: the callback acts on the generation, so every other
        // field is already in place when it does.
        s.count_in_req.fetch_add(1, Ordering::Relaxed);
    }

    /// Abandon a count-in. [`Engine::count_in_done`] stays false.
    pub fn cancel_count_in(&self) {
        let s = &self.shared;
        s.count_in_beats.store(0, Ordering::Relaxed);
        s.count_in_done.store(false, Ordering::Relaxed);
        s.beat_now.store(0, Ordering::Relaxed);
        s.count_in_req.fetch_add(1, Ordering::Relaxed);
    }

    /// The count-in beat sounding now, 1-based, for the big countdown. `None`
    /// once the count is over or if none is running.
    pub fn metronome_beat(&self) -> Option<u32> {
        match self.shared.beat_now.load(Ordering::Relaxed) {
            0 => None,
            n => Some(n),
        }
    }

    /// True from the downbeat that ends the count-in until the next
    /// [`Engine::start_count_in`].
    pub fn count_in_done(&self) -> bool {
        self.shared.count_in_done.load(Ordering::Relaxed)
    }

    /// When the downbeat was, or will be, **heard** — host timebase, including
    /// the device's own output delay.
    ///
    /// This is the number a take's `T0` should use. Polling
    /// [`Engine::count_in_done`] once a frame is accurate to a UI frame; this is
    /// accurate to a sample, and it is the difference between a take that starts
    /// on the beat and one that starts within 16 ms of it.
    pub fn count_in_downbeat_ns(&self) -> Option<Nanos> {
        match self.shared.downbeat_ns.load(Ordering::Relaxed) {
            i64::MIN => None,
            n => Some(n),
        }
    }

    /// The timebase this engine stamps against, so a caller can place its own
    /// events in the same one.
    pub fn timebase(&self) -> Timebase {
        self.timebase
    }
}

impl Drop for Engine {
    /// Retire the instrument **before** the stream goes, so `terminate` runs
    /// here rather than wherever the backend happens to free its callback box.
    fn drop(&mut self) {
        self.unload_plugin();
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cpal::Stream is not Debug.
        f.debug_struct("Engine")
            .field("output", &self.output)
            .field("plugin", &self.loaded)
            .field("running", &self.is_running())
            .finish()
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Opening the device
// ───────────────────────────────────────────────────────────────────────────

fn sane_gain(linear: f32) -> f32 {
    if linear.is_finite() {
        linear.clamp(0.0, 8.0)
    } else {
        0.0
    }
}

/// One-pole coefficient for [`GAIN_TAU_SECONDS`] at `rate`.
fn gain_coefficient(rate: f64) -> f32 {
    if !(rate.is_finite() && rate > 0.0) {
        return 1.0;
    }
    (1.0 - (-1.0 / (rate * GAIN_TAU_SECONDS)).exp()) as f32
}

/// Find an output device by name, exactly and then loosely.
///
/// cpal has no stable device identifier on any backend — `name()` is the only
/// identity method there is (`ivory-record`'s `audio.rs` documents the dig
/// through the backends) — so a name is what a caller can pass and a substring
/// is what a human will type.
fn resolve_output(host: &cpal::Host, want: Option<&str>) -> Result<cpal::Device, String> {
    let Some(want) = want else {
        return host
            .default_output_device()
            .ok_or_else(|| "this machine reports no default audio output".to_string());
    };
    let devices: Vec<cpal::Device> = host
        .output_devices()
        .map_err(|e| format!("could not enumerate audio outputs ({e})"))?
        .collect();
    let names: Vec<String> = devices
        .iter()
        .map(|d| d.name().unwrap_or_else(|_| "unnamed".to_string()))
        .collect();
    if let Some(i) = names.iter().position(|n| n == want) {
        return Ok(devices.into_iter().nth(i).expect("index came from names"));
    }
    let lower = want.to_lowercase();
    if let Some(i) = names.iter().position(|n| n.to_lowercase().contains(&lower)) {
        return Ok(devices.into_iter().nth(i).expect("index came from names"));
    }
    Err(format!(
        "no audio output matching {want:?}; this machine has {}",
        names.join(", ")
    ))
}

/// Ask for [`WANT_BUFFER_FRAMES`], inside whatever the device will accept.
///
/// A `Fixed` size outside the supported range is a build error rather than a
/// clamp, so the range is read first. `SupportedBufferSize::Unknown` means the
/// backend will not say, and there the only safe answer is `Default`.
fn pick_buffer(supported: &cpal::SupportedBufferSize) -> (cpal::BufferSize, Option<u32>) {
    let want = std::env::var("IVORY_OUT_BUFFER")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(WANT_BUFFER_FRAMES);
    match supported {
        cpal::SupportedBufferSize::Range { min, max } if *min <= *max => {
            let frames = want.clamp(*min, *max);
            (cpal::BufferSize::Fixed(frames), Some(frames))
        }
        _ => (cpal::BufferSize::Default, None),
    }
}

/// Build the stream for whatever sample type the device speaks.
///
/// The same shape `ivory-record`'s capture path uses, and for the same reason:
/// CoreAudio hands out `f32`, WASAPI shared mode usually does, and ALSA
/// routinely does not. A monitor that only works on two of three platforms is
/// not one.
fn build_stream(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    format: cpal::SampleFormat,
    renderer: Renderer,
    shared: Arc<Shared>,
    fault: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String> {
    macro_rules! open {
        ($t:ty) => {
            build_for::<$t>(device, config, renderer, shared, fault)
        };
    }
    match format {
        cpal::SampleFormat::F32 => open!(f32),
        cpal::SampleFormat::I32 => open!(i32),
        cpal::SampleFormat::I16 => open!(i16),
        cpal::SampleFormat::U16 => open!(u16),
        cpal::SampleFormat::I8 => open!(i8),
        cpal::SampleFormat::U8 => open!(u8),
        other => Err(format!("the output device speaks {other}, which is not supported")),
    }
}

/// The entire cpal-facing body of the monitor path.
///
/// Deliberately a dozen lines: everything that could be wrong is in
/// [`Renderer::render`], which is reachable from a test with no device.
fn build_for<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut renderer: Renderer,
    shared: Arc<Shared>,
    fault: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream, String>
where
    T: cpal::SizedSample + cpal::FromSample<f32> + Send + 'static,
{
    let dev_ch = config.channels as usize;
    let timebase = renderer.timebase;
    let rate = f64::from(config.sample_rate.0);
    // Sized once, on this thread. Everything after this point is the audio
    // thread's and allocates nothing.
    let mut scratch = vec![0.0f32; MAX_CALLBACK_FRAMES * dev_ch];

    device
        .build_output_stream::<T, _, _>(
            config,
            move |data: &mut [T], info: &cpal::OutputCallbackInfo| {
                let now = timebase.now();
                let heard = now + output_delay_ns(info);
                let mut done = 0usize;
                // A callback larger than the mix scratch is rendered in several
                // passes rather than reallocating it. Each pass is later in the
                // timeline by exactly the frames already written, or every event
                // in the second pass would be placed against the first pass's
                // window.
                while done + dev_ch <= data.len() {
                    let frames = ((data.len() - done) / dev_ch).min(MAX_CALLBACK_FRAMES);
                    let samples = frames * dev_ch;
                    let ahead = ((done / dev_ch) as f64 / rate * 1e9) as Nanos;
                    renderer.render(&mut scratch[..samples], now + ahead, heard + ahead);
                    for (d, s) in data[done..done + samples].iter_mut().zip(&scratch[..samples]) {
                        *d = T::from_sample(*s);
                    }
                    done += samples;
                }
                // A trailing partial frame is silenced rather than left as
                // whatever the driver had there.
                for d in data[done..].iter_mut() {
                    *d = T::from_sample(0.0f32);
                }
            },
            move |err| {
                // Never panic here: on macOS this fires for a bus-powered
                // interface browning out, which is a routine event. A contended
                // lock is skipped rather than waited on, and the flag is what
                // anything acts on.
                if matches!(err, cpal::StreamError::DeviceNotAvailable) {
                    shared.running.store(false, Ordering::Relaxed);
                }
                if let Ok(mut slot) = fault.try_lock() {
                    *slot = Some(err.to_string());
                }
            },
            None,
        )
        .map_err(|e| format!("could not open the output stream ({e})"))
}

/// How far ahead of the callback the audio it is writing will be heard.
///
/// cpal gives both instants in the device's own timebase and only lets them be
/// compared with each other (`StreamInstant`'s fields are private), so the
/// difference is the only thing readable — and the difference is exactly what is
/// wanted. `None` means playback is not after the callback, which is not
/// physical; zero is the honest answer there.
fn output_delay_ns(info: &cpal::OutputCallbackInfo) -> Nanos {
    let t = info.timestamp();
    t.playback
        .duration_since(&t.callback)
        .map(|d| d.as_nanos() as Nanos)
        .unwrap_or(0)
}

// ───────────────────────────────────────────────────────────────────────────
// The probe
// ───────────────────────────────────────────────────────────────────────────

/// Developer probe: load an instrument, count in, play a phrase, report.
///
/// **Not wired to the CLI from here.** `main.rs` owns argument parsing and is
/// being edited elsewhere; adding four lines beside `--record-test` turns this
/// on:
///
/// ```text
/// #[cfg(feature = "recorder")]
/// "--plugin-test" => {
///     instrument::plugin_test(args.next());
///     std::process::exit(0);
/// }
/// ```
///
/// It is the only way to exercise the whole monitor chain — device, plugin,
/// warm-up, MIDI queue, event placement, metronome, meters — against real
/// hardware, which no unit test can do. The phrase is played with stamps in the
/// *future*, so sub-block event placement is actually under test rather than
/// collapsing to offset 0 the way live playing does.
pub fn plugin_test(filter: Option<String>) {
    let filter = filter.unwrap_or_else(|| "Pianoteq".to_string());
    let Some(bundle) = ivory_host::discover().into_iter().find(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains(&filter.to_lowercase()))
            .unwrap_or(false)
    }) else {
        eprintln!("no VST3 matching {filter:?}");
        std::process::exit(1);
    };

    let mut engine = match Engine::start(None) {
        Ok(e) => e,
        Err(why) => {
            eprintln!("could not open an audio output: {why}");
            std::process::exit(1);
        }
    };
    println!("output:   {} ({})", engine.output().device, engine.output().latency_line());

    println!("loading:  {} (this blocks for the warm-up)", bundle.display());
    let loaded = match engine.load_plugin(&bundle, None) {
        Ok(l) => l,
        Err(why) => {
            eprintln!("could not load the instrument: {why}");
            std::process::exit(1);
        }
    };
    println!(
        "instrument: {} [{}], {} channels at {} Hz",
        loaded.class, loaded.vendor, loaded.channels, loaded.sample_rate
    );
    if let Some(w) = engine.warm_up() {
        println!(
            "warm-up:  {:.1} s, peak {:.4}{}",
            w.elapsed.as_secs_f32(),
            w.peak,
            if w.heard {
                ""
            } else {
                " — NEVER MADE A SOUND, declared ready by timeout"
            }
        );
    }

    engine.set_plugin_gain(0.8);
    engine.set_metronome_gain(0.6);
    engine.set_beats_per_bar(4);
    engine.set_metronome_in_take(false);

    // The recorder's read end, taken exactly as `record.rs`'s writer thread
    // will take it. Drained below on every poll: a tap nobody reads fills its
    // ring in four seconds and reports a colossal `dropped` count, which is
    // correct behaviour and a very confusing probe.
    let mut tap = engine
        .take_recorder_tap()
        .expect("the tap has not been taken");
    // Arm it: the ring has been running since the device opened, right through
    // the five-second warm-up, so it is full of monitor audio nobody asked for.
    tap.arm();
    let mut scratch: Vec<f32> = Vec::new();
    let mut tap_frames = 0usize;
    let mut tap_peak = 0.0f32;
    /// Move whatever is waiting and fold it into the running totals. A free
    /// function rather than a closure so the totals stay ordinary locals that
    /// can be read between calls.
    fn drain(
        tap: &mut RecorderTap,
        scratch: &mut Vec<f32>,
        frames: &mut usize,
        peak: &mut f32,
    ) {
        scratch.clear();
        *frames += tap.drain(scratch);
        *peak = scratch.iter().fold(*peak, |a, s| a.max(s.abs()));
    }

    println!("counting in 4 beats at 96 bpm...");
    engine.start_count_in(4, 96.0);
    let mut shown = 0;
    while !engine.count_in_done() {
        if let Some(beat) = engine.metronome_beat() {
            if beat != shown {
                shown = beat;
                println!("  {beat}");
            }
        }
        drain(&mut tap, &mut scratch, &mut tap_frames, &mut tap_peak);
        std::thread::sleep(Duration::from_millis(2));
    }
    let click_in_tap = tap_peak;
    println!("go.");
    engine.set_metronome_enabled(true);

    // The same ii-V-I `record_plugin.rs` plays, and for the same reason: the
    // chord onsets are deliberately not multiples of the block size, so every
    // event has to be placed by the clock rather than by rounding.
    let t0 = engine.timebase().now() + 200_000_000;
    let chords: [(&[u8], i64); 4] = [
        (&[50, 62, 65, 69, 72], 0),
        (&[43, 62, 65, 67, 71], 913),
        (&[48, 64, 67, 71, 74], 1_847),
        (&[36, 48, 55, 64, 67], 2_791),
    ];
    for (notes, at) in chords {
        for (i, pitch) in notes.iter().enumerate() {
            let on = t0 + (at + i as i64 * 7) * 1_000_000;
            engine.send_midi(on, &[0x90, *pitch, 72 + i as u8 * 6]);
            engine.send_midi(on + 880_000_000, &[0x80, *pitch, 64]);
        }
    }

    let until = Instant::now() + Duration::from_secs(5);
    let mut peak = 0.0f32;
    while Instant::now() < until {
        peak = peak.max(engine.meters().peak());
        drain(&mut tap, &mut scratch, &mut tap_frames, &mut tap_peak);
        std::thread::sleep(Duration::from_millis(16));
    }

    println!("\npeak heard:      {peak:.4} (device mix: instrument plus click)");
    println!(
        "tap captured:    {tap_frames} frames of {}-channel audio at {} Hz, peak {tap_peak:.4}",
        tap.channels(),
        tap.sample_rate()
    );
    println!(
        "click in tap:    peak {click_in_tap:.4} during the count-in — must be 0.0000, \
         because metronome_in_take is off"
    );
    println!(
        "output delay:    {:.2} ms (measured, converter delay not included)",
        engine.output_delay_ns() as f64 / 1e6
    );
    println!("callbacks:       {}", engine.callbacks());
    println!("midi dropped:    {}", engine.midi_dropped());
    println!("tap frames lost: {}", tap.dropped());
    println!(
        "pedal messages:  {} seen and NOT delivered — see instrument.rs",
        engine.pedal_dropped()
    );
    if let Some(why) = engine.fault() {
        println!("fault:           {why}");
    }
    if peak < 1e-4 {
        println!("\nSILENT — nothing reached the output.");
        std::process::exit(2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── raw MIDI to a VST3 note ────────────────────────────────────────────

    #[test]
    fn a_note_on_becomes_a_note_on_with_its_velocity_scaled_to_a_fraction() {
        let n = note_from_midi(0x90, 60, 100).expect("a note-on is a note");
        assert!(n.on);
        assert_eq!(n.pitch, 60);
        assert!(
            (n.velocity - 100.0 / 127.0).abs() < 1e-6,
            "velocity came out {}; VST3 wants 0.0..=1.0 and passing the MIDI byte \
             makes every note fortissimo and clipped",
            n.velocity
        );
        assert!(n.velocity <= 1.0);
    }

    #[test]
    fn a_note_on_with_velocity_zero_is_a_note_off() {
        // Half of all keyboards release notes this way, because it lets them
        // stay in running status. A host that reads it as a quiet note-on
        // leaves every note of the performance held down.
        let n = note_from_midi(0x90, 60, 0).expect("velocity 0 is still a note event");
        assert!(!n.on, "note-on velocity 0 was treated as a note-on");
        assert_eq!(n.pitch, 60);
        assert!(
            n.velocity > 0.0,
            "a release velocity of exactly 0 is a different sound on any piano \
             that maps it to damper noise"
        );
    }

    #[test]
    fn a_note_off_carries_its_release_velocity_through() {
        let n = note_from_midi(0x80, 72, 64).expect("a note-off is a note");
        assert!(!n.on);
        assert_eq!(n.pitch, 72);
        assert!((n.velocity - 64.0 / 127.0).abs() < 1e-6);
    }

    #[test]
    fn notes_are_read_from_every_midi_channel_not_only_channel_one() {
        for channel in 0..16u8 {
            let on = note_from_midi(0x90 | channel, 64, 90).expect("channel {channel}");
            assert!(on.on);
            let off = note_from_midi(0x80 | channel, 64, 0).expect("channel {channel}");
            assert!(!off.on);
        }
    }

    #[test]
    fn channel_messages_that_are_not_notes_are_ignored() {
        // Control change, program change, pitch bend, aftertouch, poly
        // aftertouch, and every system message.
        for status in [0xB0, 0xC0, 0xD0, 0xE0, 0xA0, 0xF0, 0xF8, 0xFE, 0xFF] {
            assert!(
                note_from_midi(status, 64, 100).is_none(),
                "status {status:#04x} was read as a note"
            );
        }
    }

    #[test]
    fn a_data_byte_mistaken_for_a_status_byte_is_refused_rather_than_masked_into_a_note() {
        // Running status resolved wrongly, or a truncated buffer. `0x10 & 0xF0`
        // is not 0x90, but `0x90 & 0x7F` is 0x10, and a reader that masks first
        // invents notes out of pitch bytes.
        for status in 0x00..0x80u8 {
            assert!(note_from_midi(status, 60, 100).is_none(), "{status:#04x}");
        }
    }

    #[test]
    fn a_pitch_or_velocity_with_its_high_bit_set_is_masked_rather_than_overflowing() {
        let n = note_from_midi(0x90, 0xFF, 0xFF).expect("still a note-on");
        assert_eq!(n.pitch, 127);
        assert!(n.velocity <= 1.0);
    }

    #[test]
    fn the_three_pedals_are_recognised_only_so_that_they_can_be_counted() {
        // They cannot be delivered: `Instance::process` takes notes and nothing
        // else. Counting them is what makes the gap visible instead of leaving
        // a pianist to discover it mid-phrase.
        assert!(is_pedal(0xB0, 64), "sustain");
        assert!(is_pedal(0xB5, 66), "sostenuto");
        assert!(is_pedal(0xB0, 67), "soft");
        assert!(!is_pedal(0xB0, 1), "the mod wheel is not a pedal");
        assert!(!is_pedal(0x90, 64), "a note at pitch 64 is not a pedal");
        assert!(note_from_midi(0xB0, 64, 127).is_none());
    }

    // ── event placement ────────────────────────────────────────────────────

    const RATE: f64 = 48_000.0;

    #[test]
    fn an_event_stamped_before_the_block_lands_at_offset_zero_rather_than_being_dropped() {
        // This is every live note: a block rendered at T can only carry events
        // known at T, so all of them are in the past. Offset 0 is the earliest
        // frame that has not been rendered yet.
        assert_eq!(place(0, 1_000_000, RATE, 512), Placement::At(0));
        assert_eq!(place(-5_000_000, 0, RATE, 512), Placement::At(0));
        assert_eq!(place(i64::MIN, 0, RATE, 512), Placement::At(0));
    }

    #[test]
    fn an_event_stamped_after_the_block_is_held_for_the_next_one() {
        // 512 frames at 48 kHz is 10.667 ms.
        assert_eq!(place(11_000_000, 0, RATE, 512), Placement::Later);
        assert_eq!(place(10_666_667, 0, RATE, 512), Placement::Later);
        assert_eq!(place(i64::MAX, 0, RATE, 512), Placement::Later);
    }

    #[test]
    fn an_event_inside_the_block_lands_on_its_own_frame() {
        // 1 ms into a block at 48 kHz is frame 48; 5 ms is frame 240.
        assert_eq!(place(1_000_000, 0, RATE, 512), Placement::At(48));
        assert_eq!(place(5_000_000, 0, RATE, 512), Placement::At(240));
        // The last frame of the block is inside it; one sample later is not.
        assert_eq!(place(10_666_000, 0, RATE, 512), Placement::At(511));
        // And an offset is measured from the block's own start, not from zero.
        assert_eq!(place(101_000_000, 100_000_000, RATE, 512), Placement::At(48));
    }

    #[test]
    fn a_nonsense_sample_rate_places_everything_at_zero_instead_of_dividing_by_it() {
        assert_eq!(place(5_000_000, 0, 0.0, 512), Placement::At(0));
        assert_eq!(place(5_000_000, 0, f64::NAN, 512), Placement::At(0));
    }

    // ── channel mapping ────────────────────────────────────────────────────

    #[test]
    fn a_mono_instrument_is_heard_from_both_speakers() {
        let mut dst = [0.0; 2];
        map_frame(&[0.5], &mut dst);
        assert_eq!(dst, [0.5, 0.5], "a mono plugin in one speaker reads as broken");
    }

    #[test]
    fn a_stereo_instrument_maps_straight_through() {
        let mut dst = [0.0; 2];
        map_frame(&[0.25, -0.75], &mut dst);
        assert_eq!(dst, [0.25, -0.75]);
    }

    #[test]
    fn a_multi_output_instrument_contributes_its_first_two_channels_and_no_more() {
        // Pianoteq's eight are stem outputs of the SAME performance; summing
        // them would be the same piano eight times, 18 dB up and clipped.
        let src = [0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let mut dst = [9.0; 2];
        map_frame(&src, &mut dst);
        assert_eq!(dst, [0.1, 0.2]);
    }

    #[test]
    fn a_mono_device_gets_the_average_and_not_the_sum() {
        let mut dst = [9.0; 1];
        map_frame(&[0.5, 0.5], &mut dst);
        assert_eq!(dst, [0.5], "summing to mono is +6 dB and clips a correct mix");
        map_frame(&[0.5], &mut dst);
        assert_eq!(dst, [0.5]);
    }

    #[test]
    fn a_device_with_more_channels_than_the_instrument_leaves_the_rest_silent() {
        let mut dst = [9.0; 6];
        map_frame(&[0.5, -0.5], &mut dst);
        assert_eq!(dst, [0.5, -0.5, 0.0, 0.0, 0.0, 0.0]);
        // Mono is the exception: it goes everywhere, because a mono instrument
        // has no left or right to be wrong about.
        map_frame(&[0.5], &mut dst);
        assert_eq!(dst, [0.5; 6]);
    }

    #[test]
    fn no_instrument_at_all_writes_silence_rather_than_the_previous_block() {
        let mut dst = [9.0; 2];
        map_frame(&[], &mut dst);
        assert_eq!(dst, [0.0, 0.0]);
    }

    // ── the beat clock ─────────────────────────────────────────────────────

    /// Run the beat clock for `frames` and return every beat it fired, with the
    /// frame it fired on. No device, no click, no audio.
    fn beats_over(b: &mut Beats, frames: usize, on: bool, bpb: u32) -> Vec<(usize, Beat)> {
        (0..frames)
            .filter_map(|i| b.tick(on, bpb).map(|beat| (i, beat)))
            .collect()
    }

    #[test]
    fn a_count_in_is_measured_in_beats_and_recording_starts_one_beat_after_the_last_one() {
        // 120 bpm at 48 kHz is 24000 frames per beat. Four beats of count-in
        // click at frames 0, 24000, 48000 and 72000, and the downbeat that ends
        // it is at 96000 — one further beat, which is what "the downbeat after
        // them" means.
        let mut b = Beats::new(48_000.0, 120.0);
        b.restart(4);
        let fired = beats_over(&mut b, 120_000, false, 4);
        let counted: Vec<_> = fired.iter().filter(|(_, x)| x.count_in > 0).collect();
        assert_eq!(counted.len(), 4, "expected four count-in beats");
        assert_eq!(counted[0].0, 0);
        assert_eq!(counted[1].0, 24_000);
        assert_eq!(counted[2].0, 48_000);
        assert_eq!(counted[3].0, 72_000);
        assert_eq!(
            counted.iter().map(|(_, x)| x.count_in).collect::<Vec<_>>(),
            vec![1, 2, 3, 4],
            "the countdown must be 1-based and in order"
        );
        let down: Vec<_> = fired.iter().filter(|(_, x)| x.downbeat).collect();
        assert_eq!(down.len(), 1);
        assert_eq!(down[0].0, 96_000, "recording starts a whole beat after beat 4");
    }

    #[test]
    fn a_count_in_sounds_even_when_the_metronome_is_switched_off() {
        // A count-in nobody can hear is not one. This is the whole reason the
        // clock takes `on` rather than being gated by it.
        let mut b = Beats::new(48_000.0, 120.0);
        b.restart(2);
        let fired = beats_over(&mut b, 80_000, false, 4);
        assert_eq!(fired.len(), 3, "two count-in beats and the downbeat");
        // And once it is over, a switched-off metronome is silent again.
        let after = beats_over(&mut b, 80_000, false, 4);
        assert!(after.is_empty());
    }

    #[test]
    fn the_beat_clock_does_not_drift_on_a_tempo_whose_period_is_not_a_whole_number() {
        // 44100 Hz at 97 bpm is 27278.35 frames per beat. Truncating loses a
        // beat every hour and a half, which is exactly long enough to be blamed
        // on something else.
        let mut b = Beats::new(44_100.0, 97.0);
        b.restart(0);
        b.was_on = true;
        let fired = beats_over(&mut b, 44_100 * 60, true, 4);
        let expected = (44_100.0 * 60.0 / (44_100.0 * 60.0 / 97.0)) as usize;
        assert!(
            fired.len().abs_diff(expected) <= 1,
            "fired {} beats in a minute at 97 bpm, expected about {expected}",
            fired.len()
        );
        let last = fired.last().expect("some beats").0 as f64;
        let ideal = (fired.len() - 1) as f64 * (44_100.0 * 60.0 / 97.0);
        assert!(
            (last - ideal).abs() < 2.0,
            "the last beat landed at {last} and should be at {ideal:.1}"
        );
    }

    #[test]
    fn the_metronome_accents_the_first_beat_of_every_bar() {
        let mut b = Beats::new(48_000.0, 120.0);
        b.restart(0);
        b.was_on = true;
        let fired = beats_over(&mut b, 24_000 * 9, true, 3);
        let accents: Vec<usize> = fired
            .iter()
            .enumerate()
            .filter(|(_, (_, x))| x.accent)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(accents, vec![0, 3, 6], "three beats to the bar");
    }

    #[test]
    fn switching_the_metronome_on_restarts_the_phase_so_it_begins_on_a_downbeat() {
        // Otherwise the first click after switching it on lands wherever the
        // free-running count happened to be, and the accent is on the wrong
        // beat for the rest of the session.
        let mut b = Beats::new(48_000.0, 120.0);
        let silent = beats_over(&mut b, 10_000, false, 4);
        assert!(silent.is_empty());
        let fired = beats_over(&mut b, 24_000, true, 4);
        assert_eq!(fired.first().expect("a beat").0, 0);
        assert!(fired[0].1.accent);
    }

    #[test]
    fn an_absurd_tempo_cannot_make_the_beat_clock_fire_forever_inside_one_block() {
        // A period below one frame would never let the countdown run down.
        assert!(period_frames(48_000.0, 1e9) >= 1.0);
        assert!(period_frames(48_000.0, 0.0) >= 1.0);
        assert!(period_frames(48_000.0, f64::NAN) >= 1.0);
        assert!(period_frames(0.0, 120.0) >= 1.0);
        let mut b = Beats::new(48_000.0, f64::INFINITY);
        b.restart(0);
        b.was_on = true;
        assert!(beats_over(&mut b, 1_000, true, 4).len() < 1_000);
    }

    // ── the click ──────────────────────────────────────────────────────────

    #[test]
    fn the_click_asset_decodes_and_is_prepared_at_the_devices_rate() {
        let at_48 = Click::load(48_000.0).expect("assets/click.wav must decode");
        assert!(!at_48.normal.is_empty());
        assert!(
            at_48.accent.len() < at_48.normal.len(),
            "the accent is the same sample played faster, so it must be shorter"
        );
        let at_44 = Click::load(44_100.0).expect("44.1 kHz devices exist");
        let ratio = at_44.normal.len() as f64 / at_48.normal.len() as f64;
        assert!(
            (ratio - 44_100.0 / 48_000.0).abs() < 0.01,
            "resampling to 44.1 kHz produced {ratio:.4} of the samples"
        );
    }

    #[test]
    fn resampling_preserves_the_length_in_seconds_and_never_divides_by_zero() {
        let src: Vec<f32> = (0..1000).map(|i| (i as f32 / 100.0).sin()).collect();
        let up = resample_linear(&src, 48_000.0, 96_000.0);
        assert!((up.len() as f64 / 2000.0 - 1.0).abs() < 0.01);
        let same = resample_linear(&src, 48_000.0, 48_000.0);
        assert_eq!(same, src, "no resampling should be an exact copy");
        assert!(resample_linear(&[], 48_000.0, 48_000.0).is_empty());
        assert!(resample_linear(&src, 0.0, 48_000.0).is_empty());
        assert!(resample_linear(&src, 48_000.0, f64::NAN).is_empty());
    }

    #[test]
    fn a_retriggered_click_starts_again_rather_than_layering() {
        let click = Click {
            normal: vec![1.0, 0.5, 0.25],
            accent: vec![2.0],
        };
        let mut v = Voice::default();
        assert_eq!(v.next(&click), 0.0, "an untriggered voice is silent");
        v.trigger(false);
        assert_eq!(v.next(&click), 1.0);
        assert_eq!(v.next(&click), 0.5);
        v.trigger(false);
        assert_eq!(v.next(&click), 1.0, "the retrigger must start from the top");
        assert_eq!(v.next(&click), 0.5);
        assert_eq!(v.next(&click), 0.25);
        assert_eq!(v.next(&click), 0.0, "and then stop");
        assert_eq!(v.next(&click), 0.0);
        v.trigger(true);
        assert_eq!(v.next(&click), 2.0, "the accent is a different buffer");
    }

    // ── the rings ──────────────────────────────────────────────────────────

    #[test]
    fn the_recorder_tap_can_be_moved_to_the_writer_thread() {
        fn assert_send<T: Send>() {}
        assert_send::<RecorderTap>();
        // And the handoff, which is the whole point of the newtype.
        assert_send::<PluginBox>();
        assert_send::<MidiEvent>();
    }

    /// A tap and its producer, with no device anywhere.
    fn tap_pair(frames: usize) -> (Producer<f32>, RecorderTap, Arc<Shared>) {
        let shared = Arc::new(Shared::new());
        let (tx, rx) = RingBuffer::<f32>::new(frames * TAP_CHANNELS);
        (
            tx,
            RecorderTap {
                rx,
                channels: TAP_CHANNELS,
                sample_rate: 48_000,
                dropped: Arc::clone(&shared),
            },
            shared,
        )
    }

    #[test]
    fn the_tap_moves_whole_frames_and_reports_how_many() {
        let (mut tx, mut tap, _) = tap_pair(64);
        tx.push_entire_slice(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), 3);
        assert_eq!(out, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        assert_eq!(tap.drain(&mut out), 0, "draining twice must not repeat");
        assert_eq!(tap.channels(), 2);
        assert_eq!(tap.sample_rate(), 48_000);
    }

    #[test]
    fn a_tap_holding_half_a_frame_yields_nothing_rather_than_swapping_the_channels() {
        // If a partial frame ever left the ring, every sample after it would be
        // on the wrong lane for the rest of the take, and no listening test in
        // mono would catch it.
        let (mut tx, mut tap, _) = tap_pair(64);
        tx.push_entire_slice(&[1.0]).unwrap();
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), 0);
        assert!(out.is_empty());
        tx.push_entire_slice(&[2.0]).unwrap();
        assert_eq!(tap.drain(&mut out), 1);
        assert_eq!(out, vec![1.0, 2.0]);
    }

    #[test]
    fn arming_the_tap_throws_away_the_monitor_audio_that_was_playing_before_the_take() {
        // Measured in the probe before this existed: 80384 frames "lost" during
        // a plugin's own warm-up, which would have made `take.json` call the
        // first take of every session short.
        let shared = Arc::new(Shared::new());
        let (mut tx, rx) = RingBuffer::<f32>::new(8 * TAP_CHANNELS);
        tx.push_entire_slice(&[1.0; 8]).unwrap();
        shared.tap_dropped.store(9_999, Ordering::Relaxed);
        let mut tap = RecorderTap {
            rx,
            channels: TAP_CHANNELS,
            sample_rate: 48_000,
            dropped: Arc::clone(&shared),
        };

        tap.arm();
        assert_eq!(tap.dropped(), 0, "a take must not inherit the monitor's losses");
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), 0, "stale audio was left in the ring");

        // And it keeps working afterwards: arming empties the ring, it does not
        // break it.
        tx.push_entire_slice(&[0.5, 0.5]).unwrap();
        assert_eq!(tap.drain(&mut out), 1);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn a_tap_ring_that_overflows_counts_the_loss_rather_than_dropping_it_silently() {
        // A silently short take is the worst outcome this path can produce, so
        // the number has to reach `take.json`.
        let shared = Arc::new(Shared::new());
        let (tx, rx) = RingBuffer::<f32>::new(8 * TAP_CHANNELS);
        let mut r = test_renderer(Arc::clone(&shared), tx, 2);
        for (i, s) in r.tap_scratch.iter_mut().enumerate() {
            *s = i as f32;
        }
        r.push_tap(8);
        assert_eq!(shared.tap_dropped.load(Ordering::Relaxed), 0);
        r.push_tap(8);
        assert_eq!(
            shared.tap_dropped.load(Ordering::Relaxed),
            8,
            "eight frames had nowhere to go and must be counted"
        );

        let mut tap = RecorderTap {
            rx,
            channels: TAP_CHANNELS,
            sample_rate: 48_000,
            dropped: Arc::clone(&shared),
        };
        let mut out = Vec::new();
        assert_eq!(tap.drain(&mut out), 8, "what did fit is still intact");
        assert_eq!(tap.dropped(), 8);
    }

    // ── the render loop ────────────────────────────────────────────────────

    /// A renderer with no device, no plugin and no click. Everything below
    /// drives this directly, which is why none of it needs hardware.
    fn test_renderer(shared: Arc<Shared>, tap: Producer<f32>, dev_ch: usize) -> Renderer {
        // The far halves are dropped here on purpose: `rtrb` handles an
        // abandoned peer (`pop` reports empty, `push` fills to capacity), so a
        // renderer under test needs no partner threads at all.
        let (_, incoming) = RingBuffer::<PluginBox>::new(2);
        let (retiring, _) = RingBuffer::<PluginBox>::new(2);
        let (_, midi) = RingBuffer::<MidiEvent>::new(1024);
        Renderer {
            shared,
            timebase: Timebase::new(),
            rate: RATE,
            dev_channels: dev_ch,
            plugin: PluginBox(None),
            incoming,
            retiring,
            midi,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            controls: Vec::with_capacity(MAX_CONTROLS_PER_BLOCK),
            tap,
            tap_scratch: vec![0.0; MAX_CALLBACK_FRAMES * TAP_CHANNELS],
            frame: vec![0.0; dev_ch.max(TAP_CHANNELS)],
            click: Click {
                normal: vec![1.0; 16],
                accent: vec![1.0; 8],
            },
            voice: Voice::default(),
            beats: Beats::new(RATE, 120.0),
            chunks: 0,
            plugin_gain: 1.0,
            metro_gain: 1.0,
            gain_coeff: 1.0,
        }
    }

    /// A renderer wired to a live MIDI producer, so events can be queued the way
    /// `send_midi` queues them.
    fn renderer_with_midi(dev_ch: usize) -> (Renderer, Producer<MidiEvent>, Arc<Shared>) {
        let shared = Arc::new(Shared::new());
        let (tap_tx, _tap_rx) = RingBuffer::<f32>::new(1 << 16);
        let (midi_tx, midi_rx) = RingBuffer::<MidiEvent>::new(1024);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, dev_ch);
        r.midi = midi_rx;
        (r, midi_tx, shared)
    }

    fn queue(tx: &mut Producer<MidiEvent>, stamp: Nanos, bytes: [u8; 3]) {
        tx.push(MidiEvent {
            stamp,
            status: bytes[0],
            data1: bytes[1],
            data2: bytes[2],
        })
        .expect("the test ring is large enough");
    }

    #[test]
    fn events_are_placed_at_their_own_frame_inside_the_block_they_belong_to() {
        let (mut r, mut tx, _) = renderer_with_midi(2);
        queue(&mut tx, 1_000_000, [0x90, 60, 100]); // 1 ms: frame 48
        queue(&mut tx, 5_000_000, [0x90, 64, 100]); // 5 ms: frame 240
        queue(&mut tx, 20_000_000, [0x90, 67, 100]); // past a 512-frame block
        r.collect_notes(0, 512);
        assert_eq!(r.notes.len(), 2);
        assert_eq!(r.notes[0].offset, 48);
        assert_eq!(r.notes[0].pitch, 60);
        assert_eq!(r.notes[1].offset, 240);
        assert!(
            r.pending.is_some(),
            "the third event belongs to a later block and must be held"
        );
    }

    #[test]
    fn a_callback_larger_than_one_render_block_is_split_into_blocks_and_keeps_every_event() {
        // 4096 frames against a 512-frame max_block is eight `process` calls.
        // `Instance::process` REFUSES anything larger — it returns an error
        // rather than truncating — so a host that hands the whole callback over
        // gets silence and no clue why.
        let (mut r, mut tx, _) = renderer_with_midi(2);
        let now = 1_000_000_000;
        // One note in each of the eight blocks: 512 frames is 10.667 ms.
        for k in 0..8i64 {
            queue(&mut tx, now + k * 10_666_667 + 1_000_000, [0x90, 60 + k as u8, 100]);
        }
        let mut out = vec![0.0f32; 4096 * 2];
        r.render(&mut out, now, now);
        assert_eq!(r.chunks, 8, "4096 frames must become eight 512-frame blocks");
        assert!(r.pending.is_none(), "an event was left over after 4096 frames");
        assert_eq!(
            r.midi.slots(),
            0,
            "{} events never left the queue",
            r.midi.slots()
        );
    }

    #[test]
    fn each_block_of_a_split_callback_gets_only_the_events_that_belong_to_it() {
        // The trap this catches: computing every offset against the CALLBACK's
        // start rather than the BLOCK's. Then an event 30 ms in has offset 1440,
        // which `Instance::process` places past the end of a 512-frame block —
        // or, worse, a host clamps it to 511 and every note in the second half
        // of the callback lands on the same frame.
        let (mut r, mut tx, _) = renderer_with_midi(2);
        // 15 ms is frame 720: block 1 (frames 512..1024), offset 208.
        queue(&mut tx, 15_000_000, [0x90, 60, 100]);
        let mut out = vec![0.0f32; 2048 * 2];
        r.collect_notes(0, 512);
        assert!(r.notes.is_empty(), "it does not belong to block 0");
        let block1_start = (512.0 / RATE * 1e9) as Nanos;
        r.collect_notes(block1_start, 512);
        assert_eq!(r.notes.len(), 1);
        assert_eq!(r.notes[0].offset, 208);
        assert!(r.notes[0].offset < 512, "an offset past the block is refused by process");
        let _ = &mut out;
    }

    #[test]
    fn a_callback_smaller_than_one_render_block_still_delivers_its_events() {
        let (mut r, mut tx, _) = renderer_with_midi(2);
        let now = 0;
        queue(&mut tx, now, [0x90, 60, 100]);
        let mut out = vec![0.0f32; 64 * 2];
        r.render(&mut out, now, now);
        assert!(r.midi.slots() == 0 && r.pending.is_none());
    }

    #[test]
    fn an_event_stamped_beyond_the_callback_is_held_and_not_lost() {
        let (mut r, mut tx, _) = renderer_with_midi(2);
        let now = 0;
        // 64 frames is 1.33 ms; this event is 5 ms out.
        queue(&mut tx, 5_000_000, [0x90, 60, 100]);
        let mut out = vec![0.0f32; 64 * 2];
        r.render(&mut out, now, now);
        assert!(r.pending.is_some(), "a future event must be held, not dropped");
        // And it comes back out on a later callback that covers it.
        r.render(&mut out, 5_000_000, 5_000_000);
        assert!(r.pending.is_none());
    }

    #[test]
    fn a_burst_larger_than_the_event_buffer_is_spread_over_blocks_rather_than_dropped() {
        // Pushing past the capacity would reallocate on the audio thread;
        // dropping would hang a note. It has to be neither.
        let (mut r, mut tx, _) = renderer_with_midi(2);
        for i in 0..300u32 {
            queue(&mut tx, 0, [0x90, 40 + (i % 40) as u8, 100]);
        }
        let mut out = vec![0.0f32; 512 * 2];
        let capacity = r.notes.capacity();
        r.render(&mut out, 0, 0);
        assert_eq!(r.notes.capacity(), capacity, "the note buffer reallocated");
        assert!(r.pending.is_some() || r.midi.slots() > 0, "the rest must be held");
        for _ in 0..8 {
            r.render(&mut out, 0, 0);
        }
        assert!(r.midi.slots() == 0, "the burst never drained");
    }

    /// **The sustain pedal reaches the instrument**, as a VST3 control.
    ///
    /// This test used to assert the opposite — that pedals were counted and
    /// discarded — because `ivory-host` had no way to send a CC. It does now
    /// (`Instance::process_with_controls`, via `IMidiMapping`), so the
    /// assertion is inverted: the pedal becomes a `Control` on the right
    /// channel with the right value, and `pedal_dropped` counts only what an
    /// instrument published no mapping for.
    #[test]
    fn the_sustain_pedal_becomes_a_control_for_the_instrument() {
        let (mut r, mut tx, shared) = renderer_with_midi(2);
        queue(&mut tx, 0, [0xB3, 64, 127]); // sustain DOWN, channel 3
        queue(&mut tx, 0, [0xB0, 1, 64]); // mod wheel: not a pedal
        let mut out = vec![0.0f32; 128 * 2];
        r.render(&mut out, 0, 0);

        assert_eq!(r.controls.len(), 1, "{:?}", r.controls);
        let c = r.controls[0];
        assert_eq!(c.controller, Control::SUSTAIN);
        assert_eq!(c.value, 127);
        assert_eq!(
            c.channel, 3,
            "the channel is not decoration: Pianoteq publishes a different \
             parameter for each channel's CC64"
        );
        assert_eq!(
            shared.pedal_dropped.load(Ordering::Relaxed),
            0,
            "nothing is dropped merely for being a pedal any more"
        );
    }

    /// All three pedals travel, not just sustain.
    #[test]
    fn sostenuto_and_the_soft_pedal_travel_too() {
        let (mut r, mut tx, _shared) = renderer_with_midi(2);
        queue(&mut tx, 0, [0xB0, 64, 127]);
        queue(&mut tx, 0, [0xB0, 66, 64]);
        queue(&mut tx, 0, [0xB0, 67, 10]);
        let mut out = vec![0.0f32; 128 * 2];
        r.render(&mut out, 0, 0);
        let sent: Vec<i16> = r.controls.iter().map(|c| c.controller).collect();
        assert_eq!(
            sent,
            vec![
                Control::SUSTAIN,
                Control::SOSTENUTO,
                Control::SOFT
            ]
        );
    }

    /// The control buffer is fixed-size and preallocated, like the note buffer:
    /// a `Vec` that grows on the audio thread allocates under a real-time
    /// deadline. Overflow is counted rather than silently dropped.
    #[test]
    fn a_flood_of_pedal_messages_never_reallocates_on_the_audio_thread() {
        let (mut r, mut tx, shared) = renderer_with_midi(2);
        for i in 0..(MAX_CONTROLS_PER_BLOCK * 2) {
            queue(&mut tx, 0, [0xB0, 64, (i % 128) as u8]);
        }
        let capacity = r.controls.capacity();
        let mut out = vec![0.0f32; 128 * 2];
        r.render(&mut out, 0, 0);
        assert_eq!(r.controls.capacity(), capacity, "the buffer reallocated");
        assert_eq!(r.controls.len(), MAX_CONTROLS_PER_BLOCK);
        assert!(
            shared.pedal_dropped.load(Ordering::Relaxed) > 0,
            "the overflow has to be reportable"
        );
    }

    #[test]
    fn midi_is_drained_even_with_no_instrument_loaded() {
        // A queue left to fill up would fire a burst of stale note-ons the
        // instant an instrument appeared, minutes after they were played.
        let (mut r, mut tx, _) = renderer_with_midi(2);
        for i in 0..50u8 {
            queue(&mut tx, 0, [0x90, 40 + i, 100]);
        }
        let mut out = vec![0.0f32; 512 * 2];
        assert!(r.plugin.0.is_none());
        r.render(&mut out, 0, 0);
        assert_eq!(r.midi.slots(), 0);
    }

    #[test]
    fn the_click_reaches_the_speakers_and_not_the_take() {
        // The one default the owner called out. The two mixes are built
        // separately, so this is a property of the render loop rather than of
        // a flag somebody might read in the wrong place.
        let shared = Arc::new(Shared::new());
        let (tap_tx, tap_rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        shared.metro_on.store(true, Ordering::Relaxed);
        shared.metro_gain.store(1.0f32.to_bits(), Ordering::Relaxed);
        r.beats.was_on = true;
        r.beats.restart(0);

        let mut out = vec![0.0f32; 256 * 2];
        r.render(&mut out, 0, 0);

        let heard = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(heard > 0.5, "the click never reached the output (peak {heard})");

        let mut tap = RecorderTap {
            rx: tap_rx,
            channels: TAP_CHANNELS,
            sample_rate: 48_000,
            dropped: Arc::clone(&shared),
        };
        let mut captured = Vec::new();
        assert_eq!(tap.drain(&mut captured), 256);
        let bled = captured.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert_eq!(
            bled, 0.0,
            "the click bled into the take, which is the failure nobody notices until playback"
        );
    }

    #[test]
    fn metronome_in_take_puts_the_click_into_the_tap_as_well() {
        let shared = Arc::new(Shared::new());
        let (tap_tx, tap_rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        shared.metro_on.store(true, Ordering::Relaxed);
        shared.metro_in_take.store(true, Ordering::Relaxed);
        shared.metro_gain.store(1.0f32.to_bits(), Ordering::Relaxed);
        r.beats.was_on = true;
        r.beats.restart(0);

        let mut out = vec![0.0f32; 256 * 2];
        r.render(&mut out, 0, 0);

        let mut tap = RecorderTap {
            rx: tap_rx,
            channels: TAP_CHANNELS,
            sample_rate: 48_000,
            dropped: Arc::clone(&shared),
        };
        let mut captured = Vec::new();
        tap.drain(&mut captured);
        let bled = captured.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(bled > 0.5, "asked for a guide click and got none (peak {bled})");
    }

    #[test]
    fn the_metronome_gain_reaches_zero_without_a_step_discontinuity() {
        // A gain that jumps is a click, which is the one thing the metronome is
        // supposed to have a monopoly on.
        let shared = Arc::new(Shared::new());
        let (tap_tx, _rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        r.gain_coeff = gain_coefficient(RATE);
        r.plugin_gain = 1.0;
        shared.plugin_gain.store(0.0f32.to_bits(), Ordering::Relaxed);
        let mut out = vec![0.0f32; 8 * 2];
        r.render(&mut out, 0, 0);
        assert!(
            r.plugin_gain > 0.9,
            "a 10 ms smoother must not cross most of its range in 8 frames \
             (it reached {})",
            r.plugin_gain
        );
        // 600 blocks of 8 frames is 4800 frames, 100 ms, ten time constants.
        for _ in 0..600 {
            r.render(&mut out, 0, 0);
        }
        assert!(r.plugin_gain < 0.01, "the smoother never arrived ({})", r.plugin_gain);
    }

    #[test]
    fn the_meter_reads_the_device_mix_and_never_misses_a_peak_between_ui_frames() {
        let shared = Arc::new(Shared::new());
        // A loud callback and then a quiet one. A meter that stored rather than
        // max-ed would report the quiet one.
        shared.peak_l.fetch_max(0.9f32.to_bits(), Ordering::Relaxed);
        shared.peak_l.fetch_max(0.1f32.to_bits(), Ordering::Relaxed);
        assert_eq!(f32::from_bits(shared.peak_l.load(Ordering::Relaxed)), 0.9);
        // And reading clears it, so the next window starts empty.
        assert_eq!(f32::from_bits(shared.peak_l.swap(0, Ordering::Relaxed)), 0.9);
        assert_eq!(f32::from_bits(shared.peak_l.load(Ordering::Relaxed)), 0.0);
    }

    #[test]
    fn a_count_in_publishes_its_beat_and_its_downbeat_from_the_audio_thread() {
        let shared = Arc::new(Shared::new());
        let (tap_tx, _rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        // Two beats at 240 bpm: 12000 frames apart at 48 kHz.
        shared.bpm.store(240.0f64.to_bits(), Ordering::Relaxed);
        shared.count_in_beats.store(2, Ordering::Relaxed);
        shared.count_in_req.fetch_add(1, Ordering::Relaxed);

        let mut out = vec![0.0f32; 4_096 * 2];
        r.render(&mut out, 0, 0);
        assert_eq!(shared.beat_now.load(Ordering::Relaxed), 1);
        assert!(!shared.count_in_done.load(Ordering::Relaxed));

        // 36000 frames covers beat 2 (12000) and the downbeat (24000).
        for _ in 0..8 {
            r.render(&mut out, 0, 0);
        }
        assert!(shared.count_in_done.load(Ordering::Relaxed));
        assert_ne!(shared.downbeat_ns.load(Ordering::Relaxed), i64::MIN);
        assert_eq!(
            shared.beat_now.load(Ordering::Relaxed),
            0,
            "the countdown must clear when it is over"
        );
    }

    #[test]
    fn the_gain_coefficient_is_a_real_time_constant_and_survives_a_nonsense_rate() {
        // 10 ms at 48 kHz is about 1/480 per frame.
        let c = gain_coefficient(48_000.0);
        assert!((c - 1.0 / 480.0).abs() < 1e-4, "got {c}");
        assert_eq!(gain_coefficient(0.0), 1.0);
        assert_eq!(gain_coefficient(f64::NAN), 1.0);
    }

    #[test]
    fn a_gain_set_to_nonsense_is_silence_rather_than_a_nan_in_the_take() {
        assert_eq!(sane_gain(f32::NAN), 0.0);
        assert_eq!(sane_gain(f32::INFINITY), 0.0);
        assert_eq!(sane_gain(-1.0), 0.0);
        assert_eq!(sane_gain(100.0), 8.0);
        assert_eq!(sane_gain(0.5), 0.5);
    }

    #[test]
    fn a_device_that_will_not_say_what_buffer_sizes_it_takes_gets_its_own_choice() {
        // `BufferSize::Fixed` outside the supported range is a build error, not
        // a clamp, so guessing is worse than deferring.
        let (size, frames) = pick_buffer(&cpal::SupportedBufferSize::Unknown);
        assert!(matches!(size, cpal::BufferSize::Default));
        assert_eq!(frames, None);
        let (size, frames) = pick_buffer(&cpal::SupportedBufferSize::Range { min: 512, max: 4096 });
        assert!(matches!(size, cpal::BufferSize::Fixed(512)));
        assert_eq!(frames, Some(512), "the request is clamped up to the minimum");
        let (_, frames) = pick_buffer(&cpal::SupportedBufferSize::Range { min: 16, max: 64 });
        assert_eq!(frames, Some(64), "and down to the maximum");
    }

    // ── with a real device and a real plugin ───────────────────────────────

    #[test]
    #[ignore = "needs an audio output device; run with \
                `cargo test -p ivory -- --ignored the_engine_opens`"]
    fn the_engine_opens_an_output_and_runs_the_metronome_with_no_plugin_loaded() {
        // The claim this proves is the one the whole restructure is about: the
        // click does not wait for a VST3.
        let mut engine = Engine::start(None).expect("an audio output");
        assert!(engine.is_running());
        assert!(engine.plugin().is_none());
        engine.set_metronome_enabled(true);
        engine.set_tempo(120.0);
        std::thread::sleep(Duration::from_millis(600));
        assert!(engine.callbacks() > 0, "the device never called back");
        assert!(
            engine.meters().peak() > 0.0,
            "the metronome made no sound without a plugin"
        );
        assert!(engine.take_recorder_tap().is_some());
        assert!(engine.take_recorder_tap().is_none(), "the tap is taken once");
    }

    #[test]
    #[ignore = "needs an audio output device; run with \
                `cargo test -p ivory -- --ignored the_count_in --nocapture`"]
    fn the_count_in_runs_in_beats_on_a_real_device_and_reports_its_own_latency() {
        // The numbers this prints are the only measured ones in the file: the
        // buffer the device actually granted, and the backend's own
        // callback-to-playback delta. Everything else about latency in this
        // product is an assumption, and RECORDER-PLAN §3a is explicit that the
        // difference matters.
        let engine = Engine::start(None).expect("an audio output");
        println!("device:   {}", engine.output().device);
        println!("buffer:   {}", engine.output().latency_line());

        engine.set_metronome_gain(0.5);
        engine.start_count_in(4, 120.0);
        let began = Instant::now();
        let mut seen: Vec<(u32, Duration)> = Vec::new();
        while !engine.count_in_done() && began.elapsed() < Duration::from_secs(10) {
            if let Some(b) = engine.metronome_beat() {
                if seen.last().map(|(n, _)| *n) != Some(b) {
                    seen.push((b, began.elapsed()));
                }
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        println!(
            "delay:    {:.2} ms measured callback-to-playback",
            engine.output_delay_ns() as f64 / 1e6
        );
        for (beat, at) in &seen {
            println!("  beat {beat} at {:.0} ms", at.as_secs_f64() * 1e3);
        }
        assert_eq!(
            seen.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert!(engine.count_in_done());
        // Four beats at 120 bpm plus the downbeat is 2.5 s of clicks.
        let total = began.elapsed().as_secs_f64();
        assert!(
            (2.0..3.2).contains(&total),
            "a four-beat count-in at 120 bpm took {total:.2} s"
        );
        assert!(engine.count_in_downbeat_ns().is_some());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored a_real_instrument`"]
    fn a_real_instrument_is_heard_and_can_be_swapped_while_the_stream_runs() {
        let bundles = ivory_host::discover();
        let Some(bundle) = bundles.iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains("pianoteq"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pianoteq; this test needs one installed");
        };
        let mut engine = Engine::start(None).expect("an audio output");
        let loaded = engine.load_plugin(bundle, None).expect("load");
        assert_eq!(engine.plugin(), Some(&loaded));
        assert!(engine.warm_up().is_some_and(|w| w.heard), "warmed up silent");

        let now = engine.timebase().now();
        engine.send_midi(now, &[0x90, 60, 100]);
        engine.send_midi(now, &[0x90, 64, 100]);
        engine.send_midi(now, &[0x90, 67, 100]);
        std::thread::sleep(Duration::from_millis(500));
        assert!(engine.meters().peak() > 0.001, "the instrument was not heard");
        engine.send_midi(engine.timebase().now(), &[0x80, 60, 64]);
        engine.send_midi(engine.timebase().now(), &[0x80, 64, 64]);
        engine.send_midi(engine.timebase().now(), &[0x80, 67, 64]);

        // The swap: the stream keeps running and the tap keeps its width.
        let mut tap = engine.take_recorder_tap().expect("a tap");
        assert_eq!(tap.channels(), TAP_CHANNELS);
        engine.unload_plugin();
        assert!(engine.plugin().is_none());
        engine.load_plugin(bundle, None).expect("reload");
        assert_eq!(tap.channels(), TAP_CHANNELS, "the tap changed width mid-take");
        assert!(engine.is_running());
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }
}
