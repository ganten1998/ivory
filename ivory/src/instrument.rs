//! The monitor engine: one audio output stream that plays up to
//! [`SLOTS`] hosted VST3 instruments at once and a metronome, and taps the
//! instruments for the recorder.
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
//! are not phase-locked to each other. So the instruments and the click are
//! summed **in the same callback**, which is also the only way the click can be
//! sample-accurate against what they are playing — and the only way three
//! instruments can be sample-accurate against each other.
//!
//! And the stream's existence does not depend on a plugin. [`Engine::start`]
//! opens the device with nothing loaded; the metronome works immediately, and a
//! plugin is swapped in later while the stream keeps running. A metronome that
//! only works once you have found and loaded a VST3 is not a metronome.
//!
//! # Three instruments, one keyboard
//!
//! A pianist wants a pad under the piano and a bell on top, so there are
//! [`SLOTS`] of them and they all play **the same notes at the same time**,
//! each with its own gain. The count comes from `ivory_ui::recorder::SLOTS` and
//! is not redefined here: the band draws exactly as many faders as this file
//! renders, and a private constant is how those two quietly disagree.
//!
//! The three properties that make layering work rather than merely compile:
//!
//! 1. **One queue, drained once.** [`Renderer::collect_notes`] fills
//!    `notes`/`controls` once per block and every slot renders *that* list. A
//!    per-slot drain would give each event to whichever slot popped first and
//!    the other two would hear silence — the MIDI ring is a queue, not a
//!    broadcast.
//! 2. **The sum is a stereo bus.** Each slot's own width is resolved where it
//!    is still known (a mono instrument goes to both sides, an eight-out
//!    instrument contributes its first two — see [`stereo_of`]), and the sum of
//!    the three is what [`map_frame`] then maps onto the device. Once three
//!    instruments are added together there is no single source width left to
//!    ask about, so the question has to be settled per slot, before the add.
//! 3. **Each slot swaps on its own.** Loading a pad into slot 2 must not
//!    interrupt the piano in slot 1 by so much as a block, which is why the
//!    handoff below is per slot rather than one shared channel.
//!
//! # The two mixes, which are not the same sum
//!
//! ```text
//!                   slot gain
//!   instrument 1 ──►(×)─┐
//!   instrument 2 ──►(×)─┼─► stereo sum ─┬──────────────► device mix ──► speakers
//!   instrument 3 ──►(×)─┘               │                 (+ click, always)
//!                                       │
//!                                       └──────────────► recorder tap ──► the take
//!   click ──►(×)───────────────────────────────────────►  (+ click ONLY if
//!         metronome gain                                    metronome_in_take)
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
//! # Threading: the instruments render IN the audio callback
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
//! # The editor, and how it reaches a plugin the audio thread owns
//!
//! Shape (a) has a consequence nobody had to pay until the plugin needed a UI:
//! **there is no `&Instance` on this thread**. `IEditController::createView` is
//! a main-thread call on an object that left for the audio callback five
//! seconds ago.
//!
//! The answer is that the controller is a *separate, reference-counted COM
//! object*, so [`Engine::load_plugin`] takes a second reference to it — an
//! [`ivory_host::EditorHandle`] — in the last moment before `inst` is moved
//! into a [`PluginBox`], and keeps it here. Opening an editor later touches
//! that handle and never the instance. Nothing stops, nothing is retired, and
//! not one block of audio is missed by opening or closing a window.
//!
//! It also does not weaken [`PluginBox`]'s safety argument, because condition 3
//! is unchanged: the audio thread calls `process` and nothing else, and every
//! call this file makes on the controller is on this thread — which is the
//! split VST3 specifies in the first place.
//!
//! There is one handle and one window **per slot**, and they are indexed by the
//! same number everywhere: `open_editor(1)` opens the window belonging to the
//! instrument `load_plugin(1, …)` loaded. Three windows can be on screen at
//! once, which is the point — balancing a layer means hearing it against the
//! other two while you change it.
//!
//! The lifetime rule that comes with it is one line and it is enforced in
//! [`Engine::unload_plugin`]: **that slot's editor closes before that slot's
//! instrument is released**, because the window holds an `IPlugView` belonging
//! to a controller that `Instance::drop` is about to terminate.
//!
//! # What the callback may not do, and how each rule is kept
//!
//! **Never allocate.** Every buffer here is sized once, in [`Engine::start`] or
//! in [`Engine::load_plugin`]: the stereo sum bus, the device mix scratch, the
//! tap scratch, the note list and every ring. Pushes into `Vec`s are guarded by
//! their capacity rather than trusted. The only per-load allocation is a slot's
//! own channel buffers, whose width is the plugin's and therefore unknowable at
//! `start` — and it happens on the UI thread, in the same call that spends five
//! seconds warming the instrument up.
//!
//! There used to be **one exception, and it was not this file's to remove**:
//! `Instance::process` allocated internally on every call — a `Vec` of channel
//! pointers, an `AudioBusBuffers` vector, one scratch buffer per channel of
//! every output bus past the first, and a `ComWrapper`ed event list. Measured
//! on Pianoteq's eight-stereo-bus layout by putting the old code under a
//! counting allocator: **42 allocator calls for one block**, 21 allocations and
//! 21 frees, and **three loaded slots was three times that** — 126 a block,
//! about 12,000 a second. It is the one place layering made an existing
//! landmine bigger rather than adding a new one. They were identical in size
//! every time, so a general-purpose allocator served them from a free list and
//! it was never heard here, which is exactly why it survived so long.
//!
//! **It is fixed, in `ivory-host`'s `instance.rs`, where it always had to be**:
//! the scratch now lives on the `Instance`, sized once in `Instance::create`
//! from `Setup::max_block` and the bus layout. Nothing changed on this side —
//! the call is still `process_with_controls` — but the pre-grown `bufs` below
//! are now load-bearing rather than merely tidy: `process` resizes them to the
//! block length, and a `Vec` that has never held a block would grow **here**,
//! on the audio thread, which is the one allocation the host cannot make go
//! away on the caller's behalf. Measured: a hundred blocks of Pianoteq through
//! this path now touch Rust's allocator zero times.
//!
//! **Never lock.** `rtrb` SPSC rings — one for MIDI, one for the tap, and a
//! pair per slot for the handoff — and a handful of relaxed atomics. The only
//! `Mutex` in the file is written by cpal's *error* callback, which is a
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
//! # The sustain pedal, and what it cost
//!
//! It works — and this section used to say it did not, which is why it is still
//! here. VST3 has no "send a CC": a control change reaches an instrument only as
//! a parameter change found through `IMidiMapping` on the EDIT CONTROLLER, a
//! second object the host must create and connect both ways. `ivory-host` now
//! does all of that, and `Instance::process_with_controls` is the door.
//!
//! **It sends the parameter change AND the legacy MIDI CC event, on every
//! control, always.** Measured on Pianoteq 9, which publishes a mapping for
//! CC64, accepts the parameter change, returns `kResultOk` and ignores it: a
//! held C4 released with the pedal down rang at 0.001452 RMS with the parameter
//! change alone — identical to six decimal places to the same phrase with no
//! pedal at all. Adding the legacy event took that to 0.012151, which is 8.4x.
//! A CC is a value rather than a delta, so a plugin honouring both sets the same
//! parameter twice and nothing is harmed.
//!
//! [`Engine::pedal_dropped`] therefore counts what an instrument published no
//! mapping for — "this plugin has no pedal" — rather than "we never tried". It
//! reports the MINIMUM across rendering slots, not the sum: one pedal press
//! refused by three plugins is one refusal, and summing would make the counter a
//! function of how many slots happen to be full.

// This module is a public API with no caller yet: the Recorder band that will
// drive it is being written alongside it, and `main.rs` does not wire
// `plugin_test` to a CLI flag (its own docs carry the four lines that do). A
// binary crate reports every unreached `pub` item as dead, so without this the
// build is fifty warnings deep and a real one could hide among them. **Delete
// this line the moment the band lands**: after that, anything it reports is a
// genuinely orphaned control.

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ivory_host::{Control, Instance, Module, Note, Setup};
use ivory_record::audio::{Timebase, CLIP_LEVEL, DEFAULT_RING_SECONDS};
use ivory_record::clock::Nanos;
// The number of instrument slots is the GUI's, not this file's. Defining a
// second one here is how the band ends up drawing three faders for four
// renderers, or four for three, and neither side would fail to compile.
use ivory_ui::recorder::SLOTS;
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

/// The two sides one slot contributes to the stereo sum.
///
/// **This is where an instrument's own width is resolved**, and it has to happen
/// here rather than after the sum: once three instruments are added together
/// there is no "the source is mono" left to ask about, and a mono pad added to a
/// stereo piano would either vanish from one side or force the whole bus to
/// mono. The rules are [`map_frame`]'s, moved up to the one place the answer is
/// still knowable:
///
/// * **Mono goes to both sides.** A mono instrument heard only on the left is
///   reported as a broken plugin.
/// * **Two or more contributes its first two channels.** Bus 0 is the main mix
///   by VST3 convention — Pianoteq's other seven are stem outputs of the same
///   performance, and summing them would be the same piano eight times.
/// * **Anything short of `frames` contributes nothing**, rather than a slice of
///   whatever the last block left there. A refused block leaves stale samples in
///   the buffers, and `frames` is the only thing that tells them apart.
fn stereo_of(bufs: &[Vec<f32>], channels: usize, frames: usize) -> (&[f32], &[f32]) {
    match channels {
        0 => (&[], &[]),
        1 => match bufs.first() {
            Some(b) if b.len() >= frames => (&b[..frames], &b[..frames]),
            _ => (&[], &[]),
        },
        _ => match (bufs.first(), bufs.get(1)) {
            (Some(l), Some(r)) if l.len() >= frames && r.len() >= frames => {
                (&l[..frames], &r[..frames])
            }
            _ => (&[], &[]),
        },
    }
}

/// Add one slot into the stereo sum, advancing its gain a frame at a time.
///
/// The gain is smoothed **inside** this loop rather than applied per block: a
/// per-block gain is a staircase, and a staircase at 48 kHz is a buzz on every
/// fader move. It also runs when the slot contributes nothing, so that a slot
/// whose instrument is unloaded and reloaded does not resume from a gain the
/// user moved half a minute ago and jump.
fn mix_in(mix: &mut [f32], left: &[f32], right: &[f32], gain: &mut f32, target: f32, coeff: f32) {
    let frames = mix.len() / TAP_CHANNELS;
    if left.len() < frames || right.len() < frames {
        for _ in 0..frames {
            *gain += (target - *gain) * coeff;
        }
        return;
    }
    for i in 0..frames {
        *gain += (target - *gain) * coeff;
        let g = *gain;
        // Bounded by construction: `frames` is `mix.len() / 2` and both sides
        // were just measured against it. Written as an add-assign because this
        // is a SUM — the bus already holds whatever the slots before it put
        // there, and an assignment here would make the last slot the only one
        // anybody hears.
        mix[i * TAP_CHANNELS] += left[i] * g;
        mix[i * TAP_CHANNELS + 1] += right[i] * g;
    }
}

/// Map one frame of the stereo sum onto one frame of some destination.
///
/// The sum is stereo and the device decides its own width, and they are
/// routinely different. The rules, all of them deliberate:
///
/// * **No source at all** (nothing loaded) writes silence, not stale samples.
/// * **Mono goes to every destination channel.** A mono source that only
///   appeared in the left speaker would be reported as a broken plugin. The
///   render path resolves mono at the slot instead (see [`stereo_of`]), so this
///   arm now serves callers that hand it a one-element source directly.
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
            period: period_frames(rate, bpm, 4),
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
///
/// `unit` is the time signature's bottom number, and it is what makes 6/8 count
/// eighths rather than quarters. **The tempo always counts QUARTER notes** —
/// forced by the file format, since an SMF tempo meta event is microseconds per
/// quarter and nothing else — so a beat of `1/unit` lasts `4/unit` quarters.
/// At 120 in 6/8 that is a quarter of a second, and a bar of six is a second
/// and a half.
fn period_frames(rate: f64, bpm: f64, unit: u32) -> f64 {
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
    // Clamped to the note values that exist. A zero here would divide by
    // nothing on the audio thread, which is the one place that must not happen.
    let unit = match unit {
        1 | 2 | 4 | 8 | 16 | 32 => unit,
        _ => 4,
    };
    (rate * 60.0 / bpm * (4.0 / f64::from(unit))).max(1.0)
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
    /// One per instrument slot. An array rather than three fields because every
    /// reader iterates them, and because a fourth slot should be a change to
    /// one constant rather than a fourth field in four structs.
    slot_gains: [AtomicU32; SLOTS],
    metro_gain: AtomicU32,
    /// The two effect knobs, 0..=1. On the instrument bus only — see
    /// `effects.rs` for what that includes and what it deliberately does not.
    reverb_mix: AtomicU32,
    delay_mix: AtomicU32,
    metro_on: AtomicBool,
    metro_in_take: AtomicBool,
    /// The COUNT-IN's clicks belong in the take, whatever `metro_in_take` says.
    ///
    /// Two flags rather than one because they answer different questions.
    /// `metro_in_take` is about the performance: a click bleeding through a
    /// take is a ruined take, which is why it is off by default. This one is
    /// about the count that comes first — and if it were not recorded, "record
    /// the count-in into the take" would produce a silence at the head of the
    /// file with nothing to line anything up against, which is the whole
    /// reason somebody asked for it.
    count_in_in_take: AtomicBool,
    bpm: AtomicU64,
    beats_per_bar: AtomicU32,
    /// The time signature's bottom number: what gets the beat.
    beat_unit: AtomicU32,

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
    /// Latched by the callback when `Instance::process` refuses a block, **per
    /// slot**: one instrument that stops rendering must not silence the two
    /// beside it, and the band has to be able to say which one to reload. The
    /// words are chosen by the reader on the UI thread, because building a
    /// `String` here would allocate on the audio thread.
    slot_faulted: [AtomicBool; SLOTS],
    /// Which slot holds the built-in instrument, or -1 for none.
    ///
    /// An `i32` because it is read on the audio thread every block and an
    /// `Option<usize>` is not atomic. -1 does not mean "silent": with no slot
    /// claiming it, the built-in still plays when nothing else does, which is
    /// what makes a fresh install audible.
    builtin_slot: AtomicI32,
    /// A patch waiting to be picked up by the renderer.
    ///
    /// A mutex, and the audio thread only ever `try_lock`s it: a patch change
    /// that has to wait one block is imperceptible, and a block that has to
    /// wait for a patch change is a dropout.
    pending_voice: std::sync::Mutex<Option<crate::dx7::Voice>>,
    running: AtomicBool,
    callbacks: AtomicU64,
    swaps: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Self {
            slot_gains: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
            metro_gain: AtomicU32::new(0.7f32.to_bits()),
            // Both effects OFF by default. A first launch has to sound like the
            // instrument and not like a room, and `Effects::process` skips its
            // whole cost at zero.
            reverb_mix: AtomicU32::new(0.0f32.to_bits()),
            delay_mix: AtomicU32::new(0.0f32.to_bits()),
            metro_on: AtomicBool::new(false),
            // THE default the owner called out: the click is a monitor signal,
            // not a take signal.
            metro_in_take: AtomicBool::new(false),
            count_in_in_take: AtomicBool::new(false),
            bpm: AtomicU64::new(120.0f64.to_bits()),
            beats_per_bar: AtomicU32::new(4),
            beat_unit: AtomicU32::new(4),
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
            slot_faulted: std::array::from_fn(|_| AtomicBool::new(false)),
            builtin_slot: AtomicI32::new(-1),
            pending_voice: std::sync::Mutex::new(None),
            running: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
        }
    }

    fn f32_of(cell: &AtomicU32) -> f32 {
        f32::from_bits(cell.load(Ordering::Relaxed))
    }

    /// Publish one slot's gain. **A slot that does not exist is ignored**, not
    /// an index: this is reached from UI code, and the app's panic hook turns a
    /// panic into a dialog and `exit(1)`.
    fn set_slot_gain(&self, slot: usize, linear: f32) {
        if let Some(cell) = self.slot_gains.get(slot) {
            cell.store(sane_gain(linear).to_bits(), Ordering::Relaxed);
        }
    }

    /// Both effect knobs at once, 0..=1.
    ///
    /// Sanitised here rather than at the point of use, for the same reason the
    /// gains are: a NaN reaching a feedback loop does not stay in one sample.
    fn set_effects(&self, reverb: f32, delay: f32) {
        let sane = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        self.reverb_mix.store(sane(reverb).to_bits(), Ordering::Relaxed);
        self.delay_mix.store(sane(delay).to_bits(), Ordering::Relaxed);
    }

    /// The instrument fault line, naming the slots so the other two are not
    /// blamed for one instrument that stopped.
    ///
    /// Built here, on the UI thread, from bits the callback latched — the
    /// callback cannot make a `String` and this cannot be a `&'static str` once
    /// it has to say *which*.
    fn instrument_fault(&self) -> Option<String> {
        let mut names = String::new();
        for (i, f) in self.slot_faulted.iter().enumerate() {
            if f.load(Ordering::Relaxed) {
                if !names.is_empty() {
                    names.push_str(", ");
                }
                // 1-based: the band's three rows are 1, 2 and 3 to everyone who
                // is not reading this file.
                names.push_str(&(i + 1).to_string());
            }
        }
        if names.is_empty() {
            return None;
        }
        Some(format!(
            "instrument {names} stopped rendering; reload it or choose another"
        ))
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
/// for the box that carries one across, and the assertion rests on four things.
/// There are [`SLOTS`] of these in flight now rather than one, and **every
/// condition is per instance**, so three of them is three separate arguments of
/// the same shape rather than one argument that has to be re-made:
///
/// 1. **It is moved, not shared.** The value travels through an `rtrb` SPSC
///    ring, which moves it out of the producer's slot and into the consumer's
///    hands. At no instant do two threads hold a reference: the UI thread's
///    copy is gone the moment `push` returns, and the callback's copy is gone
///    the moment it pushes it to the retire ring. There is no `Arc`, no
///    `&Hosted` stored anywhere, and no way to observe one from the other side.
///    With three slots this is the condition that decided the ring layout:
///    **one ring pair per slot**, so the ring an instance arrives on *is* which
///    slot it belongs to. The alternative — one pair carrying `(slot, PluginBox)`
///    — would have the callback route on an index chosen by another thread, and
///    an index that is wrong is either a panic (undefined behaviour across the
///    cpal boundary) or two instruments in one slot and none in the other. Six
///    rings of two elements, allocated once at [`Engine::start`], buys the
///    routing back from arithmetic and puts it in the type.
/// 2. **It is moved exactly once in each direction, and the protocol enforces
///    it.** For every `PluginBox` the callback takes, it pushes exactly one
///    back; and it refuses to take one at all unless *that slot's* retire ring
///    has a free slot ([`Renderer::swap_plugins`]). The UI thread waits for that
///    return before it drops anything ([`Engine::hand_off`]). So an instance is
///    either on the UI thread, in one of its own two rings, or in the callback —
///    never in two places, and never dropped twice. Nothing is shared between
///    slots for this to be true of: slot 1's handoff cannot stall, delay or
///    misroute slot 0's, because they have no ring in common.
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
///    Three loaded slots are three instances rendered one after another in the
///    same callback, each with its own buffers, and VST3 instances share no
///    mutable state with each other — two copies of the same plugin are the
///    same code and two separate objects, which is exactly what a DAW with the
///    same synth on two tracks already is.
/// 4. **Nothing is dropped in the callback.** `Instance::drop` calls
///    `setProcessing(0)`, `setActive(0)` and `terminate`, and a commercial
///    plugin's `terminate` frees sample memory, joins worker threads and, for
///    several of them, unmaps files. That is unbounded work under a real-time
///    deadline. The callback therefore never drops a `PluginBox`; it returns it,
///    and [`Engine::hand_off`] drops it on the UI thread — once per slot, on the
///    slot's own ring.
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
// argument; this line is only where it is asserted. It stays ONE line for
// [`SLOTS`] instruments: `Slot` and `[Slot; SLOTS]` are `Send` because every
// field of them is, which the compiler works out for itself — the array of
// slots is derived-`Send`, not a second assertion. If a future change makes
// this file need a second `unsafe impl Send`, that is the signal that something
// is being shared rather than moved.
unsafe impl Send for PluginBox {}

/// One instrument slot, as the audio thread sees it.
///
/// The whole per-slot world in one struct so that [`Renderer`] holds
/// `[Slot; SLOTS]` and every loop over the three is a loop over one array
/// instead of three parallel ones that can fall out of step.
struct Slot {
    /// The resident instrument, in the same newtype it travelled in. It has to
    /// be the newtype and not a bare `Option<Box<Hosted>>`: cpal requires the
    /// whole callback closure to be `Send`, so every field of this struct must
    /// be, and the `unsafe impl` on [`PluginBox`] is the one place that claim is
    /// made and argued.
    plugin: PluginBox,
    /// This slot's half of the handoff. The pair is the routing: an instance
    /// arriving here belongs *here*, and no index says so.
    incoming: Consumer<PluginBox>,
    retiring: Producer<PluginBox>,
    /// Smoothed gain, advanced one frame at a time so a fader move is a fade
    /// and not a step. Per slot, because the three faders move independently.
    gain: f32,
}

// ───────────────────────────────────────────────────────────────────────────
// The renderer: everything that runs on the audio thread
// ───────────────────────────────────────────────────────────────────────────

/// The audio callback's own state. Built on the UI thread, moved in once.
struct Renderer {
    shared: Arc<Shared>,
    timebase: Timebase,
    rate: f64,
    dev_channels: usize,

    /// The three slots, all rendered every block from the same note list.
    slots: [Slot; SLOTS],
    /// The instruments summed, interleaved stereo, one chunk long.
    ///
    /// **Sized at [`Engine::start`] and never again**, which is what makes the
    /// sum allocation-free no matter how many slots fill up later. Stereo
    /// because the tap is stereo for the life of the engine ([`TAP_CHANNELS`])
    /// and because a common bus has to have one width: the slots' own widths
    /// are resolved into it by [`stereo_of`], on the way in.
    mix: Vec<f32>,

    midi: Consumer<MidiEvent>,
    /// One event popped but not yet due. `rtrb` has no peek, so an event that
    /// belongs to a later block lives here rather than being pushed back.
    pending: Option<MidiEvent>,
    notes: Vec<Note>,
    /// The instrument this app ships with.
    ///
    /// Rendered when a slot has asked for it, and when NO slot has anything at
    /// all — so a fresh install makes a sound, and choosing "Tangent DX7" in
    /// the picker keeps making one after a real plugin has been tried and
    /// removed. See `dx7/` for the synth and `builtin.rs` for what it grew out
    /// of.
    builtin: crate::dx7::Dx7,
    /// Reverb and delay on the instrument sum. Costs nothing at rest.
    effects: crate::effects::Effects,
    /// Whether a slot has asked for the built-in by name. When none has, it
    /// still plays if nothing else does; when one has, it plays regardless.
    builtin_slot: Option<usize>,
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
    /// Frames left during which a count-in click is still SOUNDING.
    ///
    /// `Beats::counting_in` goes false the moment the last count-in beat is
    /// consumed, and the click is a decaying sample — so a take that recorded
    /// the count-in by that flag alone would cut the last click off mid-decay,
    /// and the downbeat, which is the one everybody listens for, not at all.
    /// Reset to a beat's worth of frames every time a count-in beat or the
    /// downbeat fires.
    count_in_tail: u64,

    /// `process` calls made since the stream opened. Not a statistic: it is how
    /// the chunking test observes that a 4096-frame callback became eight
    /// blocks, which is not visible any other way without a real plugin.
    chunks: u64,
    /// Smoothed click gain. One-pole per frame, exactly like each slot's.
    metro_gain: f32,
    gain_coeff: f32,
}

impl Renderer {
    /// Take each slot's waiting plugin, if one is waiting and the old one can be
    /// returned.
    ///
    /// The order matters: the retire ring is checked for room *first*, because
    /// accepting a new instance with nowhere to put the old one would leave the
    /// callback holding two, and dropping one here is exactly what condition 4
    /// of [`PluginBox`]'s safety argument forbids.
    ///
    /// Every slot is offered a swap on every callback and the cost of a slot
    /// with nothing waiting is one relaxed load of a ring's head index. Zipped
    /// rather than indexed so that the fault flag beside a slot is *that* slot's
    /// by construction — an index into a second array is a way to clear the
    /// wrong one, and the audio thread does not get to `assert!`.
    fn swap_plugins(&mut self) -> usize {
        let mut swapped = 0;
        for (slot, faulted) in self.slots.iter_mut().zip(&self.shared.slot_faulted) {
            if slot.retiring.slots() == 0 {
                continue;
            }
            let Ok(next) = slot.incoming.pop() else {
                continue;
            };
            let old = std::mem::replace(&mut slot.plugin, next);
            // Only this slot's fault is cleared. A new pad does not vouch for
            // the piano that stopped rendering an hour ago.
            faulted.store(false, Ordering::Relaxed);
            self.shared.swaps.fetch_add(1, Ordering::Relaxed);
            // Cannot fail: `slots()` was checked above and this is the only
            // producer. `let _` rather than `expect` because a panic here is
            // undefined behaviour, not a bug report.
            let _ = slot.retiring.push(old);
            swapped += 1;
        }
        swapped
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
        self.swap_plugins();

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

        // How many frames one pass through the instruments may cover. `MAX_BLOCK`
        // is the plugins' limit — `Instance::process` REFUSES more rather than
        // truncating — and the sum bus is the other half of the same bound. It
        // is sized at `Engine::start` and never resized, so this is a
        // compile-time truth written down as a runtime one: a zero here would
        // make `n` zero and the loop below would never finish, which is a hung
        // audio thread rather than a wrong sample.
        let chunk = (MAX_BLOCK as usize).min(self.mix.len() / 2);
        if chunk == 0 {
            return;
        }

        // Read the controls once per callback, not once per frame: they are
        // published by the UI at human speed and re-reading them 256 times
        // costs 256 atomic loads for a value that cannot have changed. Three
        // slots is three more loads per callback, not three more per frame.
        let slot_targets: [f32; SLOTS] =
            std::array::from_fn(|i| Shared::f32_of(&self.shared.slot_gains[i]).clamp(0.0, 8.0));
        let metro_target = Shared::f32_of(&self.shared.metro_gain).clamp(0.0, 8.0);
        let metro_on = self.shared.metro_on.load(Ordering::Relaxed);
        let in_take = self.shared.metro_in_take.load(Ordering::Relaxed);
        let count_in_recorded = self.shared.count_in_in_take.load(Ordering::Relaxed);
        let bpb = self.shared.beats_per_bar.load(Ordering::Relaxed);
        self.beats.period = period_frames(
            self.rate,
            f64::from_bits(self.shared.bpm.load(Ordering::Relaxed)),
            self.shared.beat_unit.load(Ordering::Relaxed),
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
            let n = chunk.min(frames - done);
            self.chunks += 1;

            // Each chunk gets its own window in the host timeline, so an event
            // stamped inside the second half of a 1024-frame callback lands in
            // the second `process` call rather than being held for the next
            // callback entirely.
            let block_start = now + (done as f64 / self.rate * 1e9) as Nanos;
            // ONCE, for all three slots. Draining per slot would give each event
            // to whichever slot ran first — see the module docs.
            self.collect_notes(block_start, n);

            let widths = self.render_slots(n);
            self.sum_slots(n, &widths, &slot_targets);
            self.render_builtin(n, &widths);

            // **Here, and this position is the feature.** Downstream of every
            // instrument, so a VST3 and the built-in FM are treated alike;
            // upstream of the tap, so what is recorded is what was heard;
            // upstream of the click and the input monitor, so neither of those
            // ends up in a room they were never in.
            if let Some(mix) = self.mix.get_mut(..n * TAP_CHANNELS) {
                self.effects.process(
                    mix,
                    n,
                    TAP_CHANNELS,
                    Shared::f32_of(&self.shared.reverb_mix),
                    Shared::f32_of(&self.shared.delay_mix),
                    f64::from_bits(self.shared.bpm.load(Ordering::Relaxed)),
                );
            }

            for i in 0..n {
                let frame_index = done + i;

                let counting_in = self.count_in_tail > 0;
                self.count_in_tail = self.count_in_tail.saturating_sub(1);
                if let Some(beat) = self.beats.tick(metro_on, bpb) {
                    self.voice.trigger(beat.accent);
                    if beat.count_in > 0 || beat.downbeat {
                        // A whole beat, so the sample has room to decay, and
                        // reset on the downbeat too — that click is the "go"
                        // and the one anybody trimming to the count will look
                        // for.
                        self.count_in_tail = self.beats.period.max(1.0) as u64;
                    }
                    self.shared
                        .beat_now
                        .store(beat.count_in, Ordering::Relaxed);
                    if beat.downbeat {
                        let at = heard + (frame_index as f64 / self.rate * 1e9) as Nanos;
                        self.shared.downbeat_ns.store(at, Ordering::Relaxed);
                        self.shared.count_in_done.store(true, Ordering::Relaxed);
                    }
                }

                self.metro_gain += (metro_target - self.metro_gain) * self.gain_coeff;
                let click = self.voice.next(&self.click) * self.metro_gain;

                // The three instruments, already summed and already at their own
                // gains: `sum_slots` did that in one pass per slot, which is one
                // pass over a contiguous buffer rather than three interleaved
                // reads per frame.
                let at2 = i * TAP_CHANNELS;
                let src = &self.mix[at2..at2 + TAP_CHANNELS];

                // ── the tap mix: instruments only, unless asked otherwise ──
                map_frame(src, &mut self.frame[..TAP_CHANNELS]);
                let tap_at = tap_frames * TAP_CHANNELS;
                // The click reaches the FILE when the performance is meant to
                // carry it, or while a count-in that was asked to be in the
                // take is still sounding. `counting_in` rather than a beat
                // number, because the click is a decaying sample: the sound of
                // beat four is still going during beat five and has to be
                // recorded for as long as it lasts.
                let click_recorded = in_take || (count_in_recorded && counting_in);
                for c in 0..TAP_CHANNELS {
                    let s = self.frame[c] + if click_recorded { click } else { 0.0 };
                    self.tap_scratch[tap_at + c] = s;
                }
                tap_frames += 1;

                // ── the device mix: instruments plus click, always ──
                let at = frame_index * dev_ch;
                map_frame(src, &mut self.frame[..dev_ch]);
                for c in 0..dev_ch {
                    let s = self.frame[c] + click;
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

    /// Render one chunk from **every** loaded slot and return how many channels
    /// each one wrote.
    ///
    /// Zero means silence, and that is the normal state of a slot nobody has
    /// filled. A block a plugin refuses latches that slot's fault and returns
    /// zero for it rather than leaving the previous block's samples in the
    /// buffers, which would loop the last 10 ms forever at full level — and
    /// leaves the other two rendering, because one instrument giving up is not a
    /// reason to stop the piano.
    ///
    /// Every slot is handed the *same* `&self.notes` and `&self.controls`. That
    /// is the layering invariant in one line, and it only holds because the
    /// queue was drained into those two lists before this was called.
    /// The built-in instrument, over the bus, when nothing else is playing it.
    ///
    /// **Only when every slot is empty.** It is a fallback, not a layer: a
    /// piano underneath somebody's own instrument would be a bug they could
    /// not switch off, and the moment they load one the app has the sound they
    /// chose. `widths` is what actually rendered this block, which is the
    /// honest test — a slot holding a faulted plugin is an empty slot as far as
    /// the bus is concerned.
    fn render_builtin(&mut self, frames: usize, widths: &[usize; SLOTS]) {
        // Asked for by name, or nothing else is playing. The second is what
        // makes a fresh install audible; the first is what makes the picker's
        // top entry mean something once a plugin has been loaded elsewhere.
        // A patch change, if one is waiting and the lock is free this block.
        if let Ok(mut g) = self.shared.pending_voice.try_lock() {
            if let Some(v) = g.take() {
                self.builtin.set_voice(v);
            }
        }
        let wanted = self.shared.builtin_slot.load(Ordering::Relaxed);
        self.builtin_slot = (wanted >= 0).then_some(wanted as usize);
        if self.builtin_slot.is_none() && widths.iter().any(|w| *w > 0) {
            // Something real is playing. Keep the built-in silent AND clear, so
            // unloading a plugin mid-note does not resurrect a stale one.
            if self.builtin.active() {
                self.builtin.all_notes_off();
            }
            return;
        }
        for n in &self.notes {
            // `Note::pitch` is the VST3 shape, an `i16`; anything outside a
            // MIDI key is not a note this instrument can sound.
            let Ok(pitch) = u8::try_from(n.pitch) else {
                continue;
            };
            if n.on {
                self.builtin.note_on(pitch, n.velocity);
            } else {
                self.builtin.note_off(pitch);
            }
        }
        // CC 64 is the damper. Half scale is the switch point every synth
        // uses, and it is what a half-pedalled continuous controller means.
        for c in &self.controls {
            if c.controller == 64 {
                self.builtin.set_pedal(c.value >= 64);
            }
        }
        if !self.builtin.active() {
            return;
        }
        let Some(mix) = self.mix.get_mut(..frames * TAP_CHANNELS) else {
            return;
        };
        self.builtin.render(mix, frames, TAP_CHANNELS);
    }

    fn render_slots(&mut self, frames: usize) -> [usize; SLOTS] {
        let notes = &self.notes;
        let controls = &self.controls;
        let shared = &self.shared;
        let mut widths = [0usize; SLOTS];
        // The controls this block found no home in. **The minimum across the
        // slots that rendered**, not the sum: one pedal press refused by three
        // instruments is one message that did not arrive, and summing would
        // report the same press three times and make `pedal_dropped` a function
        // of how many slots are full.
        let mut unmapped = usize::MAX;
        for ((width, slot), faulted) in widths
            .iter_mut()
            .zip(self.slots.iter_mut())
            .zip(&shared.slot_faulted)
        {
            if faulted.load(Ordering::Relaxed) {
                continue;
            }
            let Some(p) = slot.plugin.0.as_mut() else {
                continue;
            };
            match p
                .inst
                .process_with_controls(notes, controls, frames, &mut p.bufs)
            {
                Ok(rendered) => {
                    unmapped = unmapped.min(rendered.unmapped);
                    *width = p.channels;
                }
                Err(_) => {
                    // The message is discarded on purpose: reading it means
                    // holding a `String` the audio thread would have to free.
                    // `fault()` turns this flag into words, and into a number.
                    faulted.store(true, Ordering::Relaxed);
                }
            }
        }
        // A control no instrument published a mapping for. Counted rather than
        // ignored, so the band can say "these instruments have no pedal" instead
        // of the user concluding the app has none. `usize::MAX` means no slot
        // rendered at all, which is not the pedal's fault.
        if unmapped != usize::MAX && unmapped > 0 {
            shared
                .pedal_dropped
                .fetch_add(unmapped as u64, Ordering::Relaxed);
        }
        widths
    }

    /// Sum the slots into the stereo bus, at their own smoothed gains.
    ///
    /// One pass per slot over a contiguous buffer, which is the cheap way round:
    /// the alternative reads three plugins' buffers per frame and touches three
    /// cache lines to write one.
    fn sum_slots(&mut self, frames: usize, widths: &[usize; SLOTS], targets: &[f32; SLOTS]) {
        // `get_mut` and not a slice expression: `render` bounds `frames` to what
        // the bus holds, and this is the line that would panic — on the audio
        // thread, across an FFI boundary — if that bound were ever loosened.
        let Some(mix) = self.mix.get_mut(..frames * TAP_CHANNELS) else {
            return;
        };
        mix.fill(0.0);
        let coeff = self.gain_coeff;
        for ((slot, width), target) in self.slots.iter_mut().zip(widths).zip(targets) {
            // Destructured so the gain can be advanced while the buffers are
            // read: they are separate fields and the borrow checker knows it,
            // but only if the two are named separately.
            let Slot { plugin, gain, .. } = slot;
            let (left, right) = match plugin.0.as_ref() {
                Some(p) => stereo_of(&p.bufs, *width, frames),
                None => (&[][..], &[][..]),
            };
            mix_in(mix, left, right, gain, *target, coeff);
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
    /// Build a tap over a ring the caller owns the other end of.
    ///
    /// For tests in sibling modules — `record.rs`'s writer holds a `RecorderTap`
    /// and its behaviour around one (draining it even when the instrument is
    /// not being recorded, counting its losses only when it is) cannot be
    /// tested against a `None`. Ask me how I know: the first version of that
    /// test passed with the fix removed, because there was no tap in it.
    /// Returns the tap, the write end, and a closure that counts a frame the
    /// ring could not take — the three things the engine wires together.
    ///
    /// The closure rather than the counter itself, so `Shared` stays private:
    /// a test in a sibling module needs to BUMP the count, not to see the type
    /// that holds it.
    #[cfg(test)]
    pub(crate) fn for_test(slots: usize, channels: usize) -> (Self, Producer<f32>, impl Fn(u64)) {
        let shared = Arc::new(Shared::new());
        let counter = Arc::clone(&shared);
        let (tx, rx) = RingBuffer::<f32>::new(slots);
        (
            Self {
                rx,
                channels,
                sample_rate: 48_000,
                dropped: shared,
            },
            tx,
            move |frames: u64| {
                counter.tap_dropped.fetch_add(frames, Ordering::Relaxed);
            },
        )
    }
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

/// The UI thread's end of one slot's plugin handoff.
///
/// Kept as a pair because the pair is the protocol: nothing may go in until
/// whatever is in there has come out, and holding the two halves together is
/// what makes that one function ([`Engine::hand_off`]) instead of a convention.
struct Handoff {
    to_audio: Producer<PluginBox>,
    from_audio: Consumer<PluginBox>,
}

/// The monitor engine: one output stream, [`SLOTS`] optional instruments layered
/// on top of each other, one metronome.
///
/// Everything instrument-shaped is indexed by slot and **an out-of-range slot is
/// never a panic**: it is a no-op or an `Err`, because these are called straight
/// from UI code and this app's panic hook turns a panic into a dialog and
/// `exit(1)`. A menu that got its arithmetic wrong should misbehave, not take
/// the session with it.
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

    /// This thread's end of each slot's handoff, one pair per slot.
    handoff: [Handoff; SLOTS],
    loaded: [Option<Loaded>; SLOTS],
    warm: [Option<WarmUp>; SLOTS],

    tap: Option<RecorderTap>,
    fault: Arc<Mutex<Option<String>>>,

    /// The open editor window for each slot, if the user has asked for one.
    ///
    /// The engine owns them because the engine owns the plugins, and the two
    /// have a lifetime rule: an editor must never outlive the instance whose
    /// controller built its view. Everything that unloads goes through
    /// [`Engine::close_editor`] first, and this is declared **before**
    /// `editor_handles` so that even a plain `drop(engine)` releases every
    /// window and its `IPlugView` before the controller references that made
    /// them.
    editors: [Option<ivory_host::Editor>; SLOTS],
    /// A reference to each loaded plugin's edit controller, taken in
    /// [`Engine::load_plugin`] **before** the instance is handed to the audio
    /// thread — see [`Engine::open_editor`], where that is the whole trick.
    /// `None` for a slot with nothing loaded.
    editor_handles: [Option<ivory_host::EditorHandle>; SLOTS],
    /// A reference to each loaded plugin's **processor**, taken in the same
    /// moment and for the same reason, so that [`Engine::save_slot_state`] can
    /// ask an instrument what it sounds like after it has left for the audio
    /// callback. `None` for a slot with nothing loaded.
    ///
    /// Cleared before the slot's instance is retired, exactly like
    /// `editor_handles`: a handle that outlives the instance is a live pointer
    /// into a terminated object.
    state_handles: [Option<ivory_host::StateHandle>; SLOTS],
    /// Why a slot's saved state was not restored, if it was not.
    ///
    /// A stale or hand-edited blob in the settings file must not stop the
    /// instrument loading — the user wants their piano more than they want
    /// their preset — so the load continues with the plugin's defaults and the
    /// reason lands here, where [`Engine::state_error`] can put it in front of
    /// someone. Silently ignoring it is how "my preset keeps resetting" becomes
    /// unreportable.
    state_errors: [Option<String>; SLOTS],

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

    /// As [`Engine::start`], with a buffer size the user chose.
    pub fn start_sized(
        out_device: Option<&str>,
        timebase: Timebase,
        buffer_frames: Option<u32>,
    ) -> Result<Self, String> {
        Self::start_inner(out_device, timebase, buffer_frames)
    }

    /// As [`Engine::start`], sharing the recorder's timebase.
    ///
    /// Worth doing: it is what makes [`Engine::count_in_downbeat_ns`] comparable
    /// with the MIDI tap's stamps and with the take's `T0`, so a take can start
    /// on the downbeat the player actually heard rather than on the UI frame
    /// that noticed.
    pub fn start_with(out_device: Option<&str>, timebase: Timebase) -> Result<Self, String> {
        Self::start_inner(out_device, timebase, None)
    }

    fn start_inner(
        out_device: Option<&str>,
        timebase: Timebase,
        asked_buffer: Option<u32>,
    ) -> Result<Self, String> {
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
        let (buffer_size, buffer_frames) = pick_buffer(supported.buffer_size(), asked_buffer);

        let shared = Arc::new(Shared::new());
        let fault = Arc::new(Mutex::new(None));
        let click = Click::load(f64::from(rate))?;

        // Every ring is allocated here and never again.
        let (midi_tx, midi_rx) = RingBuffer::<MidiEvent>::new(1024);
        let tap_frames = (DEFAULT_RING_SECONDS * rate as f32) as usize;
        let (tap_tx, tap_rx) = RingBuffer::<f32>::new(tap_frames.max(4096) * TAP_CHANNELS);

        // Two rings of two per slot. Two and two because at most one handoff per
        // slot is ever in flight — `hand_off` waits for the return before it
        // starts another — and the spare element is what lets `swap_plugins`
        // check for room before it commits. Six small rings rather than one
        // shared pair, because the ring an instance travels on is what says
        // which slot it belongs to; see [`PluginBox`], condition 1.
        let mut ends: Vec<Handoff> = Vec::with_capacity(SLOTS);
        let slots: [Slot; SLOTS] = std::array::from_fn(|_| {
            let (to_audio, incoming) = RingBuffer::<PluginBox>::new(2);
            let (retiring, from_audio) = RingBuffer::<PluginBox>::new(2);
            ends.push(Handoff {
                to_audio,
                from_audio,
            });
            Slot {
                plugin: PluginBox(None),
                incoming,
                retiring,
                gain: 1.0,
            }
        });
        let mut ends = ends.into_iter();
        let handoff: [Handoff; SLOTS] =
            std::array::from_fn(|_| ends.next().expect("one Handoff was pushed per slot"));

        let dev_ch = channels as usize;
        let widest = dev_ch.max(TAP_CHANNELS);
        let renderer = Renderer {
            shared: Arc::clone(&shared),
            timebase,
            rate: f64::from(rate),
            dev_channels: dev_ch,
            slots,
            // Sized for one whole render block, here and never again — this is
            // the buffer that makes summing three instruments allocation-free
            // whether they arrive now or in an hour.
            mix: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            midi: midi_rx,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            builtin: crate::dx7::Dx7::new(rate as f32),
            effects: crate::effects::Effects::new(rate as f32),
            builtin_slot: None,
            controls: Vec::with_capacity(MAX_CONTROLS_PER_BLOCK),
            tap: tap_tx,
            tap_scratch: vec![0.0; MAX_CALLBACK_FRAMES * TAP_CHANNELS],
            frame: vec![0.0; widest],
            click,
            voice: Voice::default(),
            beats: Beats::new(f64::from(rate), 120.0),
            count_in_tail: 0,
            chunks: 0,
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
            handoff,
            loaded: std::array::from_fn(|_| None),
            warm: std::array::from_fn(|_| None),
            tap: Some(RecorderTap {
                rx: tap_rx,
                channels: TAP_CHANNELS,
                sample_rate: rate,
                dropped: shared,
            }),
            fault,
            editors: std::array::from_fn(|_| None),
            editor_handles: std::array::from_fn(|_| None),
            state_handles: std::array::from_fn(|_| None),
            state_errors: std::array::from_fn(|_| None),
            hold: Cell::new((0.0, 0.0)),
            hold_at: Cell::new(Instant::now()),
        })
    }


    // ── the instruments ─────────────────────────────────────────────────────

    /// Load an instrument into `slot` of the running stream.
    ///
    /// The other slots are untouched: their instances never leave the callback,
    /// their rings are not involved, and the piano in slot 0 does not miss a
    /// block while a pad is loaded into slot 1. **What it does cost the other
    /// slots is this thread**, for as long as the warm-up below takes.
    ///
    /// **This blocks for about five seconds and must not be called from inside
    /// a frame.** Module load, instantiation and — the expensive part — the
    /// warm-up: RECORDER-PLAN §8 measured four of six instruments on this
    /// machine rendering silence if played immediately after instantiation, and
    /// all four fine five seconds later. `ivory_host::ready` is what decides
    /// that, run here through its blocking `warm_up` helper because this call is
    /// blocking by contract. Three slots means a session that fills all three
    /// waits three times, one after another, which is the strongest argument yet
    /// for the incremental form below.
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
        slot: usize,
        bundle: &Path,
        class_name: Option<&str>,
    ) -> Result<Loaded, String> {
        self.load_plugin_with_state(slot, bundle, class_name, None)
    }

    /// As [`Engine::load_plugin`], and restore `state` into the instrument
    /// before it is warmed up or handed over.
    ///
    /// `state` is whatever [`Engine::save_slot_state`] returned in an earlier
    /// session. Anything else — a truncated file, another plugin's blob, a
    /// hand-edited one — is refused by `ivory_host::state` before it can reach
    /// the plugin, and refusing it is **not** a failed load: the instrument
    /// comes up with its defaults and [`Engine::state_error`] says why.
    ///
    /// # Create, restore, warm up, and the order is the whole point
    ///
    /// The restore happens between `Instance::create` and the warm-up, and both
    /// of those neighbours matter:
    ///
    /// * **After create**, because there is nothing to restore into before it;
    /// * **Before the warm-up**, because a preset change makes an instrument
    ///   reload its samples. State that arrives afterwards starts a NEW load
    ///   immediately after `ivory_host::ready` finished waiting for the old
    ///   one, so the gate says Ready and the instrument then goes quiet — the
    ///   exact failure that module exists to prevent, reintroduced by ordering.
    ///   It also means the first thing the warm-up probe renders is the default
    ///   piano rather than the user's.
    ///
    /// `Instance::load_state` refuses a restore after the instance has rendered
    /// anything, so this ordering is enforced by the host rather than by
    /// convention.
    pub fn load_plugin_with_state(
        &mut self,
        slot: usize,
        bundle: &Path,
        class_name: Option<&str>,
        state: Option<&[u8]>,
    ) -> Result<Loaded, String> {
        // Checked FIRST, before five seconds of work that would have nowhere to
        // go, and returned as an error rather than an index because this is
        // called from the UI.
        if slot >= SLOTS {
            return Err(format!(
                "there is no instrument slot {slot}; there are {SLOTS}, numbered from 0"
            ));
        }
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

        // BEFORE the warm-up. See this function's docs: a preset change makes a
        // sampled instrument reload, so state that arrives after the gate has
        // closed means the gate waited for the wrong load.
        let state_error = match state {
            Some(bytes) => inst.load_state(bytes).err(),
            None => None,
        };

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

        // ── the editor's only chance to get a reference ─────────────────────
        //
        // **Taken here and nowhere else.** One line below, `inst` is moved into
        // a `Hosted`, into a `PluginBox`, and across a ring into the audio
        // callback, and from that moment there is no `&Instance` on this thread
        // and no safe way to make one. `IEditController` is a reference-counted
        // COM object, so a second reference costs an atomic increment and keeps
        // working after the instance has gone; it is `!Send`, so it can only
        // ever be used from here.
        //
        // The alternatives were considered and are worse:
        //
        // * **Retire the plugin to borrow it back.** `hand_off(PluginBox(None))`
        //   would return the instance, but the callback renders silence for the
        //   whole round trip and any note in the MIDI ring during it is dropped
        //   — a click and a hole in the middle of playing, every time somebody
        //   opens a window.
        // * **Create the view at load time.** Builds Pianoteq's entire UI on
        //   every load whether or not anyone wants it, and leaves a view alive
        //   with nowhere to be attached.
        //
        // The audio thread's side of this is unchanged: it calls `process` and
        // nothing else, which is condition 3 of `PluginBox`'s safety argument
        // and is exactly the split VST3 specifies — the controller is a
        // main-thread object and every call this file makes on it is on this
        // thread.
        let editor_handle = inst.editor_handle();
        // The same trick, one line later and for the same reason: after the
        // handoff there is no `&Instance` here to ask for its state either.
        // `IComponent` is reference-counted, the handle is `!Send`, and
        // `getState` is a main-thread call — which is this thread.
        let state_handle = inst.state_handle();

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

        // Whatever was in THIS slot is going away, so its editor and then its
        // controller reference must go first — see `unload_plugin`. `hand_off`
        // is where the old instance is dropped, and dropping it terminates the
        // controller the old handle points at. The other slots' editors and
        // handles are not touched, because their instruments are not.
        self.close_editor(slot);
        if let Some(h) = self.editor_handles.get_mut(slot) {
            *h = None;
        }
        if let Some(h) = self.state_handles.get_mut(slot) {
            *h = None;
        }
        self.hand_off(slot, PluginBox(Some(Box::new(hosted))));
        // Written through `get_mut` rather than indexed even though `slot` was
        // bounded at the top: the bound and the write are forty lines apart, and
        // the next edit to this function is the one that separates them further.
        if let Some(h) = self.editor_handles.get_mut(slot) {
            *h = editor_handle;
        }
        if let Some(h) = self.state_handles.get_mut(slot) {
            *h = Some(state_handle);
        }
        if let Some(e) = self.state_errors.get_mut(slot) {
            *e = state_error;
        }
        if let Some(l) = self.loaded.get_mut(slot) {
            *l = Some(loaded.clone());
        }
        if let Some(w) = self.warm.get_mut(slot) {
            *w = Some(warm);
        }
        Ok(loaded)
    }

    /// Take one slot's instrument out of the running stream and release it here.
    ///
    /// A no-op for an empty slot and for a slot that does not exist. The other
    /// two keep playing, which is the whole reason the handoff is per slot.
    ///
    /// **That slot's editor closes first, and that ordering is not optional.**
    /// The window holds an `IPlugView` the plugin's edit controller made, and
    /// the instance's `Drop` calls `IEditController::terminate`. Unloading with
    /// a window still open would leave a view — and a plugin's `NSView` inside
    /// our `NSView` — belonging to a terminated object, and the crash would land
    /// wherever AppKit next asked it to draw.
    pub fn unload_plugin(&mut self, slot: usize) {
        if slot >= SLOTS {
            return;
        }
        self.close_editor(slot);
        if let Some(h) = self.editor_handles.get_mut(slot) {
            *h = None;
        }
        // Before the instance goes, and for the same reason the editor handle
        // does: `Instance::drop` calls `IComponent::terminate`.
        if let Some(h) = self.state_handles.get_mut(slot) {
            *h = None;
        }
        if let Some(e) = self.state_errors.get_mut(slot) {
            *e = None;
        }
        if self.loaded.get(slot).is_none_or(Option::is_none) {
            return;
        }
        if let Some(l) = self.loaded.get_mut(slot) {
            *l = None;
        }
        if let Some(w) = self.warm.get_mut(slot) {
            *w = None;
        }
        self.hand_off(slot, PluginBox(None));
    }

    // ── the plugins' own editors ────────────────────────────────────────────

    /// Whether the instrument in `slot` offers an editor to open.
    ///
    /// `false` for an empty slot, for a slot that does not exist, and for a
    /// plugin that has no UI — which is the honest thing to grey a menu row on.
    /// The first call after a load is not free (VST3 has no `hasEditor`, so the
    /// only way to ask is to build a view and throw it away); every call after
    /// that is a cached bool.
    pub fn has_editor(&self, slot: usize) -> bool {
        self.editor_handles
            .get(slot)
            .and_then(Option::as_ref)
            .is_some_and(ivory_host::EditorHandle::has_editor)
    }

    /// Open one slot's editor, or bring it to the front if it is already open.
    ///
    /// **Main thread, and not from inside the audio callback** — but that is
    /// already true of every `&mut self` method here, because `Engine` is
    /// `!Send`.
    ///
    /// The window is independent of the audio: opening it does not interrupt a
    /// single block, and closing it does not stop the instrument. Call
    /// [`Engine::poll_editor`] once a frame so a window the user closes is
    /// noticed and released. Three windows can be open at once and they do not
    /// know about each other.
    pub fn open_editor(&mut self, slot: usize) -> Result<(), String> {
        if slot >= SLOTS {
            return Err(format!(
                "there is no instrument slot {slot}; there are {SLOTS}, numbered from 0"
            ));
        }
        // A window the user closed one frame ago is still `Some` here until
        // something notices. Noticing first is what stops "Open" reviving a
        // window that then vanishes again on the next `poll_editor`.
        self.poll_editor();
        if let Some(editor) = self.editors.get(slot).and_then(Option::as_ref) {
            // Already open. Raising it is what "open the editor" means the
            // second time somebody clicks the row.
            editor.focus();
            return Ok(());
        }
        let Some(handle) = self.editor_handles.get(slot).and_then(Option::as_ref) else {
            return Err(format!("no instrument is loaded in slot {}", slot + 1));
        };
        // The slot is IN the title, and it is not decoration: the same piano
        // loaded twice for two different mic positions gives two windows that
        // are otherwise character-for-character identical, and the user has to
        // be able to tell which fader belongs to the one they are editing.
        let title = match self.loaded.get(slot).and_then(Option::as_ref) {
            Some(l) => format!("{} - Tangent, instrument {}", l.class, slot + 1),
            None => format!("Instrument {} - Tangent", slot + 1),
        };
        let editor =
            ivory_host::Editor::open_handle(handle, &title).map_err(|e| e.to_string())?;
        if let Some(e) = self.editors.get_mut(slot) {
            *e = Some(editor);
        }
        Ok(())
    }

    /// Close one slot's editor window if it is open. Safe to call when it is
    /// not, and for a slot that does not exist.
    ///
    /// Dropping the [`ivory_host::Editor`] is the teardown: `IPlugView::removed`
    /// and then the window, in that order.
    pub fn close_editor(&mut self, slot: usize) {
        if let Some(e) = self.editors.get_mut(slot) {
            *e = None;
        }
    }

    /// Is this slot's editor window open right now?
    pub fn editor_open(&self, slot: usize) -> bool {
        self.editors.get(slot).is_some_and(Option::is_some)
    }

    /// Notice windows the user closed and let go of them. Call once a frame.
    ///
    /// **All three at once**, because the caller has one frame loop and should
    /// not have to know how many slots there are to keep it honest. Cheap: one
    /// bool read per open window, nothing at all for a slot with none. Without
    /// it a closed editor stays "open" as far as [`Engine::editor_open`] is
    /// concerned, and the menu row keeps saying Close for a window that is not
    /// there.
    pub fn poll_editor(&mut self) {
        for editor in &mut self.editors {
            if editor.as_ref().is_some_and(ivory_host::Editor::closed) {
                *editor = None;
            }
        }
    }

    /// Give one slot's callback `next` and wait for whatever it was holding, so
    /// the old instance is dropped on **this** thread.
    ///
    /// Condition 4 of [`PluginBox`]'s safety argument in one function. The wait
    /// is bounded: a callback that is not running cannot return anything, and
    /// then the instance goes down with the stream instead.
    ///
    /// It waits for *this slot's* ring and no other, so a slot whose handoff
    /// timed out (a device that vanished) costs the next load one timeout in
    /// that slot rather than blocking every slot behind it.
    fn hand_off(&mut self, slot: usize, next: PluginBox) {
        let Some(h) = self.handoff.get_mut(slot) else {
            // Nowhere to send it, so it is dropped here — on this thread, which
            // is the only place `Instance::drop` is allowed to run anyway.
            return;
        };
        // Anything already returned is dropped here, now, before another
        // handoff can fill the ring.
        while h.from_audio.pop().is_ok() {}
        if h.to_audio.push(next).is_err() {
            // Only reachable if a previous handoff was never collected, which
            // the wait below is there to prevent.
            return;
        }
        let deadline = Instant::now() + RETIRE_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(old) = h.from_audio.pop() {
                drop(old);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// What is loaded in `slot`, or `None` for an empty or absent one.
    pub fn plugin(&self, slot: usize) -> Option<&Loaded> {
        self.loaded.get(slot).and_then(Option::as_ref)
    }

    /// Is anything loaded at all?
    ///
    /// The take-source decision: a take records the instruments when there is an
    /// instrument to record, and one full slot out of three is enough. Asking
    /// per slot would make "record the plugin" mean "record slot 0".
    /// Put the built-in instrument in a slot, or take it out.
    ///
    /// A slot holding it is not a plugin: nothing is opened, nothing can fail,
    /// and it is playing on the next block.
    pub fn set_builtin_slot(&mut self, slot: Option<usize>) {
        self.shared
            .builtin_slot
            .store(slot.map_or(-1, |s| s as i32), Ordering::Relaxed);
    }

    /// Load a patch into the built-in.
    ///
    /// Sent as a message rather than written through, because the renderer owns
    /// it and it is on the audio thread. A voice is 155 small numbers and this
    /// happens when somebody clicks a name in a list, so the cost is nothing.
    pub fn set_builtin_voice(&mut self, voice: crate::dx7::Voice) {
        if let Ok(mut g) = self.shared.pending_voice.lock() {
            *g = Some(voice);
        }
    }

    pub fn any_plugin_loaded(&self) -> bool {
        self.loaded.iter().any(Option::is_some)
    }

    /// What one slot's warm-up concluded. `heard: false` means the instrument
    /// never made a sound and was declared ready by timeout.
    pub fn warm_up(&self, slot: usize) -> Option<&WarmUp> {
        self.warm.get(slot).and_then(Option::as_ref)
    }

    // ── the instruments' state ──────────────────────────────────────────────

    /// What the instrument in `slot` sounds like right now, as opaque bytes to
    /// be written somewhere and handed back to
    /// [`Engine::load_plugin_with_state`] next session.
    ///
    /// `None` for an empty slot, for a slot that does not exist, and for a
    /// plugin that refused — the three are deliberately not distinguished,
    /// because the only thing a caller can do with any of them is write nothing
    /// for that slot.
    ///
    /// **Main thread, and it does not interrupt the audio.** The instance is
    /// inside the callback; this goes through a second reference to its
    /// processor taken at load time, exactly as the editor does. `getState`
    /// while `process` runs on another thread is the arrangement VST3 specifies
    /// and the one every DAW uses to save a project mid-playback — the
    /// alternative, retiring the instrument across the handoff to borrow it
    /// back, would cost a block of silence and every note in flight each time
    /// the settings file is written. See `ivory_host::StateHandle`.
    ///
    /// **Size**: measured on Pianoteq 9, one slot is about 40 KB (41,233 bytes,
    /// of which 41,215 are the processor's and none the controller's). Three
    /// full slots is around 120 KB, or 165 KB base64'd into a JSON settings
    /// file — large for a settings file and small for a disk. A sampler that
    /// embeds its content could be far larger; `ivory_host::MAX_STATE_BYTES` is
    /// the ceiling either way.
    pub fn save_slot_state(&self, slot: usize) -> Option<Vec<u8>> {
        self.state_handles
            .get(slot)
            .and_then(Option::as_ref)
            .and_then(|h| h.save().ok())
    }

    /// Why the state handed to [`Engine::load_plugin_with_state`] was not
    /// restored into `slot`.
    ///
    /// `None` is the normal answer: nothing was offered, or it was restored.
    /// `Some` means the instrument came up with its defaults and the user's
    /// preset did not survive — worth saying out loud, because the symptom is
    /// "it keeps forgetting my piano" and nothing else in the app would know.
    pub fn state_error(&self, slot: usize) -> Option<&str> {
        self.state_errors
            .get(slot)
            .and_then(Option::as_ref)
            .map(String::as_str)
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
    ///
    /// It names the slot, because with three of them "the instrument stopped
    /// rendering" is a sentence the user cannot act on.
    pub fn fault(&self) -> Option<String> {
        if let Some(why) = self.shared.instrument_fault() {
            return Some(why);
        }
        self.fault.lock().ok().and_then(|g| g.clone())
    }

    // ── gains ───────────────────────────────────────────────────────────────

    /// One slot's gain. Linear, not dB: 1.0 is unity; above that is deliberate
    /// make-up gain and is clamped at 8x in the callback so a fader dragged into
    /// a text field cannot produce a full-scale square wave.
    ///
    /// **Three instruments at unity is up to three times the level of one**, and
    /// nothing here stops that: it is the user's mix, the clip indicator is
    /// honest about it, and quietly scaling the sum by the number of loaded
    /// slots would make loading a pad change the volume of the piano.
    ///
    /// A slot that does not exist is ignored rather than indexed — this comes
    /// from the UI, and a panic here reaches the user as a dialog and `exit(1)`.
    pub fn set_slot_gain(&self, slot: usize, linear: f32) {
        self.shared.set_slot_gain(slot, linear);
    }

    /// The two effect knobs, 0..=1.
    pub fn set_effects(&self, reverb: f32, delay: f32) {
        self.shared.set_effects(reverb, delay);
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

    /// Whether the COUNT-IN's clicks go into the take. See
    /// [`Shared::count_in_in_take`] for why this is not the same switch.
    pub fn set_count_in_in_take(&self, on: bool) {
        self.shared.count_in_in_take.store(on, Ordering::Relaxed);
    }


    pub fn set_tempo(&self, bpm: f64) {
        self.shared.bpm.store(bpm.to_bits(), Ordering::Relaxed);
    }


    /// Beats per bar, which decides which beat is accented.
    pub fn set_beats_per_bar(&self, beats: u32) {
        self.shared
            .beats_per_bar
            .store(beats.max(1), Ordering::Relaxed);
    }

    /// The time signature, both halves at once.
    ///
    /// One call rather than two setters, because the two are read on the audio
    /// thread on different frames: setting 6 then 8 leaves one callback seeing
    /// 6/4, which is a bar and a half of clicks at the wrong speed. They are
    /// still two atomics — a callback can still catch them mid-change — but the
    /// window is one store apart rather than a whole UI frame.
    pub fn set_meter(&self, beats: u32, unit: u32) {
        self.shared.beat_unit.store(
            match unit {
                1 | 2 | 4 | 8 | 16 | 32 => unit,
                _ => 4,
            },
            Ordering::Relaxed,
        );
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
    /// Retire **every** instrument before the stream goes, so `terminate` runs
    /// here rather than wherever the backend happens to free its callback box.
    ///
    /// One at a time and each with its own bounded wait: three instances that
    /// all have to come back is three round trips, and the alternative — pushing
    /// all three and then waiting — would have the UI thread holding a deadline
    /// for a callback that may already have stopped.
    fn drop(&mut self) {
        for slot in 0..SLOTS {
            self.unload_plugin(slot);
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cpal::Stream is not Debug.
        f.debug_struct("Engine")
            .field("output", &self.output)
            .field("plugins", &self.loaded)
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
fn pick_buffer(
    supported: &cpal::SupportedBufferSize,
    asked: Option<u32>,
) -> (cpal::BufferSize, Option<u32>) {
    // The user's choice first, then the dev override, then the default. The
    // env var stays ahead of the built-in so a debugging session does not have
    // to go through the settings file, and behind the SETTING so that what the
    // Audio Status panel says is what the panel was told.
    let want = asked
        .or_else(|| {
            std::env::var("IVORY_OUT_BUFFER")
                .ok()
                .and_then(|v| v.parse::<u32>().ok())
        })
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

/// Process CPU time so far, in seconds, or `None` where it cannot be asked.
///
/// [`plugin_test`] and nowhere else. "What does a third instrument cost" has to
/// be answered with a measurement, and the cheap honest place to take one is the
/// whole process across a fixed window: in the probe the audio thread is the
/// only thing doing real work, and everything else is a 16 ms pump.
///
/// Deliberately **not** a timer inside the audio callback. A DSP-load meter is a
/// good feature and it is not this change's to add: two clock reads per block is
/// a change to the hot path that nothing in the product asks for yet.
#[cfg(unix)]
fn cpu_seconds() -> Option<f64> {
    // SAFETY: `rusage` is a plain C struct of integers, so an all-zero bit
    // pattern is a valid value of it, and `getrusage` overwrites the whole thing
    // before anything below reads it. Zeroed rather than `MaybeUninit` because a
    // failed call still has to leave something defined behind.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: the pointer is to a live, correctly-typed local that outlives the
    // call, and `RUSAGE_SELF` is the constant the same crate declares for this
    // parameter. `getrusage` writes only through that pointer.
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } == 0;
    if !ok {
        return None;
    }
    let secs = |t: libc::timeval| t.tv_sec as f64 + t.tv_usec as f64 / 1e6;
    Some(secs(usage.ru_utime) + secs(usage.ru_stime))
}

/// Windows has `GetProcessTimes` and no `libc`; the probe says "not measured"
/// rather than growing a second platform path for a developer tool.
#[cfg(not(unix))]
fn cpu_seconds() -> Option<f64> {
    None
}

/// Developer probe: load an instrument into every slot, count in, play a phrase,
/// report.
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
/// It is the only way to exercise the whole monitor chain — device, plugins,
/// warm-up, MIDI queue, event placement, layering, metronome, meters — against
/// real hardware, which no unit test can do. The phrase is played with stamps in
/// the *future*, so sub-block event placement is actually under test rather than
/// collapsing to offset 0 the way live playing does.
///
/// **Layering is measured, not asserted.** The same instrument is loaded into
/// each slot in turn and the same phrase played to all of them, so the claim
/// "every loaded slot gets every note" becomes a number: identical instruments
/// playing identical notes sum coherently, so two slots must be +6 dB on one and
/// three must be +9.5 dB. A peak that does not move is a slot that never heard
/// the note; a peak that moves by less is a sum that is not coherent, which
/// means the slots are not being given the same events.
pub fn plugin_test(filter: Option<String>) {
    /// Every slot at the same gain, so the peaks below are comparable, and low
    /// enough that three of them summed still fit under full scale.
    const LAYER_GAIN: f32 = 0.5;
    /// Long enough to cover the phrase and its release tail.
    const MEASURE_SECONDS: f64 = 3.5;

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

    println!(
        "loading:  {} into slot 1 (this blocks for the warm-up)",
        bundle.display()
    );
    let loaded = match engine.load_plugin(0, &bundle, None) {
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
    if let Some(w) = engine.warm_up(0) {
        println!(
            "warm-up:  {:.1} s, peak {:.4}{}",
            w.elapsed.as_secs_f32(),
            w.peak,
            if w.heard {
                ""
            } else {
                " - NEVER MADE A SOUND, declared ready by timeout"
            }
        );
    }

    // ── the state, which is what makes a preset survive a restart ───────────
    //
    // Read here rather than at the end, because the other two slots are loaded
    // WITH it: the layering measurement below rests on the three instruments
    // being identical, and "restored from slot 1's own bytes" is a stronger
    // claim than "the same plugin file".
    let preset = engine.save_slot_state(0);
    match &preset {
        Some(b) => println!("state:    {} bytes from slot 1", b.len()),
        None => println!("state:    this instrument handed over none"),
    }

    for slot in 0..SLOTS {
        engine.set_slot_gain(slot, LAYER_GAIN);
    }
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

    /// Move whatever is waiting into the totals and report the loudest sample
    /// in it. A free function rather than a closure so the totals stay ordinary
    /// locals that can be read between calls.
    fn drain(tap: &mut RecorderTap, scratch: &mut Vec<f32>, frames: &mut usize) -> f32 {
        scratch.clear();
        *frames += tap.drain(scratch);
        scratch.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }

    /// The same ii-V-I `record_plugin.rs` plays, and for the same reason: the
    /// chord onsets are deliberately not multiples of the block size, so every
    /// event has to be placed by the clock rather than by rounding.
    ///
    /// A function rather than a straight line of code because it is played over
    /// and over: once per layering measurement, and then again for as long as
    /// somebody has an editor open and needs something to hear.
    fn play_phrase(engine: &Engine) {
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
    }

    /// Play the phrase once and report what it did: the loudest sample in the
    /// device mix, the loudest in the tap, and the process's CPU seconds per
    /// second of wall clock while it rang.
    ///
    /// **Layering is judged on the TAP peak and not the device peak**, because
    /// the tap is instruments-only by construction and the device mix is not.
    /// Measured before this was: the count-in's last click was still ringing
    /// when the first window opened and it is louder than the piano, so the
    /// device read 0.5841 against a tap of 0.2807 for the same phrase, and the
    /// one-slot reference was the metronome. Hence the settle below as well.
    fn measure(
        engine: &Engine,
        tap: &mut RecorderTap,
        scratch: &mut Vec<f32>,
        frames: &mut usize,
        seconds: f64,
    ) -> (f32, f32, Option<f64>) {
        // Let the previous phrase's release and any click finish sounding. A
        // peak is a maximum over the window, so anything still ringing when the
        // window opens is this measurement's answer.
        let settle = Instant::now() + Duration::from_millis(900);
        while Instant::now() < settle {
            let _ = drain(tap, scratch, frames);
            ivory_host::editor::pump(Duration::from_millis(8));
        }
        // Whatever is left over is not this phrase's. Reading the meter clears
        // its peak (it is read-and-clear by design), and the tap is drained to
        // the same end.
        engine.meters();
        let _ = drain(tap, scratch, frames);
        let cpu0 = cpu_seconds();
        let began = Instant::now();
        play_phrase(engine);
        let (mut peak, mut heard_in_tap) = (0.0f32, 0.0f32);
        while began.elapsed().as_secs_f64() < seconds {
            peak = peak.max(engine.meters().peak());
            heard_in_tap = heard_in_tap.max(drain(tap, scratch, frames));
            // `pump` and not `sleep`: with a plugin editor on screen this is
            // the only thing servicing its window.
            ivory_host::editor::pump(Duration::from_millis(8));
        }
        let elapsed = began.elapsed().as_secs_f64();
        let cores = match (cpu0, cpu_seconds()) {
            (Some(a), Some(b)) if elapsed > 0.0 => Some((b - a) / elapsed),
            _ => None,
        };
        (peak, heard_in_tap, cores)
    }

    println!("counting in 4 beats at 96 bpm...");
    engine.start_count_in(4, 96.0);
    let mut shown = 0;
    let mut click_in_tap = 0.0f32;
    while !engine.count_in_done() {
        if let Some(beat) = engine.metronome_beat() {
            if beat != shown {
                shown = beat;
                println!("  {beat}");
            }
        }
        click_in_tap = click_in_tap.max(drain(&mut tap, &mut scratch, &mut tap_frames));
        // `pump` and not `sleep`, for the same reason as in `measure`.
        ivory_host::editor::pump(Duration::from_millis(2));
    }
    println!("go.");

    // ── layering, measured ──────────────────────────────────────────────────
    //
    // The metronome stays OFF through this: the peaks below are supposed to be
    // the instruments and nothing else, and a click landing inside the window
    // would be counted as part of the sum.
    println!("\nlayering: the same instrument in each slot, every slot at gain {LAYER_GAIN}");
    let mut rows: Vec<(usize, f32, f32, Option<f64>)> = Vec::new();
    for slot in 0..SLOTS {
        if slot > 0 {
            println!("  loading slot {} with the same instrument...", slot + 1);
            if let Err(why) =
                engine.load_plugin_with_state(slot, &bundle, None, preset.as_deref())
            {
                println!("  slot {} would not load: {why}", slot + 1);
                break;
            }
            if let Some(why) = engine.state_error(slot) {
                // Not fatal — the slot is loaded, on its defaults — but it
                // means the three instruments are no longer known to match,
                // which is exactly what the dB column below assumes.
                println!("  slot {}: the saved state was refused: {why}", slot + 1);
            }
        }
        let (peak, in_tap, cores) = measure(
            &engine,
            &mut tap,
            &mut scratch,
            &mut tap_frames,
            MEASURE_SECONDS,
        );
        tap_peak = tap_peak.max(in_tap);
        rows.push((slot + 1, peak, in_tap, cores));
    }

    println!("\n  slots  device peak   tap peak   tap vs one   process CPU");
    let one = rows.first().map(|r| r.2).unwrap_or(0.0);
    for (n, peak, in_tap, cores) in &rows {
        let db = if one > 0.0 && *in_tap > 0.0 {
            format!("{:+.1} dB", 20.0 * (in_tap / one).log10())
        } else {
            "-".to_string()
        };
        let cpu = match cores {
            Some(c) => format!("{c:.2} cores"),
            None => "not measured".to_string(),
        };
        println!("  {n:>5}  {peak:>11.4}  {in_tap:>9.4}  {db:>11}   {cpu}");
    }
    println!(
        "  two slots must be about +6.0 dB on one and three about +9.5 dB: the same\n  \
         instrument playing the same notes sums coherently, so a peak that did not\n  \
         move is a slot that never got the note. Read the TAP column for that -\n  \
         the device column is what you hear, which includes the click."
    );

    // ── the plugins' own editors ────────────────────────────────────────────
    //
    // Opened here because this probe is the only place the whole chain exists
    // at once: a signed bundle (which is what a plugin with a licence check
    // will talk to), a real output device, and a window. A human can open
    // Pianoteq's UI on slot 1, change the instrument, and hear it change against
    // the other two — which is the one claim about this feature that no test can
    // make.
    println!(
        "\neditor:   {}",
        if engine.has_editor(0) {
            "yes"
        } else {
            "this plugin has none"
        }
    );
    if engine.has_editor(0) {
        // This process will never call `[NSApp run]`: `main.rs` exits before
        // eframe starts. Without this the window opens behind everything, never
        // takes the keyboard, and looks broken.
        ivory_host::editor::become_foreground();
        match engine.open_editor(0) {
            Ok(()) => println!("          slot 1 open. Close the window to end this probe."),
            Err(why) => println!("          could not open it: {why}"),
        }
    }

    // The click joins in from here: everything that had to be measured has
    // been, and what is left is a human listening to three instruments.
    engine.set_metronome_enabled(true);
    play_phrase(&engine);
    // Five seconds when there is nothing to look at, which is what this probe
    // always did. With an editor open it runs until the window is closed, and
    // the phrase repeats so there is always something playing while the
    // instrument is being changed underneath it.
    let deadline = if engine.editor_open(0) {
        None
    } else {
        Some(Instant::now() + Duration::from_secs(5))
    };
    let mut next_phrase = Instant::now() + Duration::from_secs(6);
    let mut peak = rows.iter().fold(0.0f32, |a, r| a.max(r.1));
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            break;
        }
        // 16 ms, so this loop runs at the rate the app's own frame loop would.
        ivory_host::editor::pump(Duration::from_millis(16));
        engine.poll_editor();
        if deadline.is_none() && !engine.editor_open(0) {
            println!("\neditor closed.");
            break;
        }
        peak = peak.max(engine.meters().peak());
        tap_peak = tap_peak.max(drain(&mut tap, &mut scratch, &mut tap_frames));
        if Instant::now() >= next_phrase {
            next_phrase = Instant::now() + Duration::from_secs(6);
            play_phrase(&engine);
        }
    }
    // Closing the window must not have stopped anything. The instruments are
    // still loaded and still rendering, and the next two seconds prove it.
    if deadline.is_none() {
        let check = Instant::now() + Duration::from_secs(2);
        let before = engine.callbacks();
        play_phrase(&engine);
        let mut after_peak = 0.0f32;
        while Instant::now() < check {
            after_peak = after_peak.max(engine.meters().peak());
            tap_peak = tap_peak.max(drain(&mut tap, &mut scratch, &mut tap_frames));
            std::thread::sleep(Duration::from_millis(16));
        }
        println!(
            "after the editor: {} more callbacks, peak {after_peak:.4} - the instruments \
             are running instruments, not windows",
            engine.callbacks() - before
        );
        peak = peak.max(after_peak);
    }

    // Did anything the user did in the editor reach the bytes? This is the one
    // end of the persistence feature no test can check: pick a different piano
    // in Pianoteq's own UI, close the window, and watch the state change.
    match (preset.as_deref(), engine.save_slot_state(0)) {
        (Some(before), Some(after)) if before == after.as_slice() => {
            println!("state:           unchanged, {} bytes", after.len());
        }
        (Some(before), Some(after)) => println!(
            "state:           CHANGED, {} bytes -> {} bytes. That is the preset the \
             next launch would restore.",
            before.len(),
            after.len()
        ),
        (_, after) => println!("state:           {:?} bytes now", after.map(|b| b.len())),
    }

    // ── unloading, one slot at a time ───────────────────────────────────────
    //
    // The other slots must keep playing while one is retired, which is the
    // property the per-slot handoff exists for. Slot 2 goes and slots 1 and 3
    // are asked to make a sound afterwards.
    if rows.len() == SLOTS {
        engine.unload_plugin(1);
        engine.meters();
        play_phrase(&engine);
        let until = Instant::now() + Duration::from_secs(2);
        let mut after = 0.0f32;
        while Instant::now() < until {
            after = after.max(engine.meters().peak());
            tap_peak = tap_peak.max(drain(&mut tap, &mut scratch, &mut tap_frames));
            ivory_host::editor::pump(Duration::from_millis(16));
        }
        println!(
            "\nslot 2 unloaded: peak {after:.4} from the other two - unloading one \
             instrument must not silence its neighbours"
        );
        println!(
            "loaded now:      {:?}",
            (0..SLOTS)
                .map(|s| engine.plugin(s).map(|l| l.class.as_str()))
                .collect::<Vec<_>>()
        );
        peak = peak.max(after);
    }

    println!("\npeak heard:      {peak:.4} (device mix: instruments plus click)");
    println!(
        "tap captured:    {tap_frames} frames of {}-channel audio at {} Hz, peak {tap_peak:.4}",
        tap.channels(),
        tap.sample_rate()
    );
    println!(
        "click in tap:    peak {click_in_tap:.4} during the count-in - must be 0.0000, \
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
        "pedal messages:  {} seen and NOT delivered - see instrument.rs",
        engine.pedal_dropped()
    );
    if let Some(why) = engine.fault() {
        println!("fault:           {why}");
    }
    if peak < 1e-4 {
        println!("\nSILENT - nothing reached the output.");
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

    // ── layering: what each slot contributes, and how they add ─────────────

    #[test]
    fn a_mono_slot_is_summed_into_both_sides_of_the_stereo_bus() {
        // Resolved HERE and not after the sum: once a mono pad has been added to
        // a stereo piano there is nothing left that knows the pad was mono, and
        // it would end up in the left speaker only.
        let bufs = vec![vec![0.5, -0.25]];
        let (l, r) = stereo_of(&bufs, 1, 2);
        assert_eq!(l, [0.5, -0.25]);
        assert_eq!(r, [0.5, -0.25], "a mono slot heard on one side reads as broken");
    }

    #[test]
    fn a_multi_output_slot_contributes_only_its_first_two_channels() {
        // Pianoteq's eight are stem outputs of the SAME performance; summing
        // them would be the same piano eight times, 18 dB up and clipped.
        let bufs: Vec<Vec<f32>> = (0..8).map(|c| vec![c as f32; 4]).collect();
        let (l, r) = stereo_of(&bufs, 8, 4);
        assert_eq!(l, [0.0; 4]);
        assert_eq!(r, [1.0; 4]);
    }

    #[test]
    fn a_slot_that_rendered_nothing_contributes_nothing_rather_than_stale_samples() {
        // Zero channels is an empty slot or one whose `process` was refused, and
        // its buffers still hold the last block it did render. Replaying those
        // is the last 10 ms looping forever at full level.
        let bufs = vec![vec![0.9; 8], vec![0.9; 8]];
        assert_eq!(stereo_of(&bufs, 0, 8), (&[][..], &[][..]));
        // And a buffer shorter than the block is refused for the same reason:
        // the tail of it would be whatever was there before.
        assert_eq!(stereo_of(&bufs, 2, 9), (&[][..], &[][..]));
        assert_eq!(stereo_of(&[], 2, 4), (&[][..], &[][..]));
    }

    #[test]
    fn three_slots_at_the_same_gain_are_three_times_one_of_them() {
        // The whole feature in one assertion: the same signal in three slots
        // sums coherently. A sum that assigned rather than accumulated would
        // read 1.0x here, which is exactly what "only the last instrument is
        // audible" looks like from the outside.
        let src = vec![vec![0.1, 0.2], vec![-0.1, -0.2]];
        let mut mix = vec![0.0f32; 2 * TAP_CHANNELS];
        for _ in 0..3 {
            let (l, r) = stereo_of(&src, 2, 2);
            let mut gain = 1.0;
            mix_in(&mut mix, l, r, &mut gain, 1.0, 1.0);
        }
        assert!((mix[0] - 0.3).abs() < 1e-6, "{mix:?}");
        assert!((mix[1] + 0.3).abs() < 1e-6, "{mix:?}");
        assert!((mix[2] - 0.6).abs() < 1e-6, "{mix:?}");
        assert!((mix[3] + 0.6).abs() < 1e-6, "{mix:?}");
    }

    #[test]
    fn a_slot_at_zero_gain_is_not_in_the_sum_at_all() {
        let src = vec![vec![1.0; 2], vec![1.0; 2]];
        let (l, r) = stereo_of(&src, 2, 2);
        let mut mix = vec![0.0f32; 2 * TAP_CHANNELS];
        let mut gain = 0.0;
        mix_in(&mut mix, l, r, &mut gain, 0.0, 1.0);
        assert_eq!(mix, vec![0.0; 4], "a fader at zero still let something through");
    }

    #[test]
    fn a_slot_that_contributes_nothing_still_advances_its_own_gain() {
        // Otherwise a slot loaded a minute after its fader moved starts at the
        // gain the fader used to have and jumps to the one it has now, which is
        // a click on the first note of the new instrument.
        let mut mix = vec![0.0f32; 64 * TAP_CHANNELS];
        let mut gain = 1.0;
        mix_in(&mut mix, &[], &[], &mut gain, 0.0, 0.1);
        assert!(gain < 0.01, "the smoother stopped with the signal ({gain})");
        assert_eq!(mix, vec![0.0; 64 * TAP_CHANNELS]);
    }

    #[test]
    fn a_gain_change_arrives_as_a_ramp_across_the_block_and_not_as_a_step() {
        // The first sample must still be near the OLD gain and the last near
        // the new one. A per-block gain is a staircase, and a staircase at
        // 48 kHz is a buzz on every fader move.
        let src = vec![vec![1.0; 512], vec![1.0; 512]];
        let (l, r) = stereo_of(&src, 2, 512);
        let mut mix = vec![0.0f32; 512 * TAP_CHANNELS];
        let mut gain = 1.0;
        mix_in(&mut mix, l, r, &mut gain, 0.0, gain_coefficient(RATE));
        assert!(mix[0] > 0.99, "the ramp began somewhere other than the old gain");
        assert!(mix[1022] < 0.4, "10 ms of a 10 ms smoother went nowhere");
        assert!(mix[1022] > 0.2, "it arrived far too fast to be one time constant");
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
        assert!(period_frames(48_000.0, 1e9, 4) >= 1.0);
        assert!(period_frames(48_000.0, 0.0, 4) >= 1.0);
        assert!(period_frames(48_000.0, f64::NAN, 4) >= 1.0);
        assert!(period_frames(0.0, 120.0, 4) >= 1.0);
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
        // A slot, and the whole renderer, are `Send` because every field of
        // them is — DERIVED, not asserted. `unsafe impl Send` appears exactly
        // once in this file and a second one would mean something is being
        // shared rather than moved. cpal requires this of the closure it takes,
        // so it is checked here rather than discovered at the call site.
        assert_send::<Slot>();
        assert_send::<[Slot; SLOTS]>();
        assert_send::<Renderer>();
    }

    /// A renderer whose slots have live handoff rings at both ends, so the swap
    /// protocol can be driven with no plugin anywhere: `PluginBox(None)` is a
    /// legal value and an empty slot travels the same rings a full one does.
    fn renderer_with_handoffs(shared: Arc<Shared>) -> (Renderer, [Handoff; SLOTS]) {
        let (tap_tx, _rx) = RingBuffer::<f32>::new(1 << 12);
        let mut r = test_renderer(shared, tap_tx, 2);
        let mut ends: Vec<Handoff> = Vec::with_capacity(SLOTS);
        for slot in &mut r.slots {
            let (to_audio, incoming) = RingBuffer::<PluginBox>::new(2);
            let (retiring, from_audio) = RingBuffer::<PluginBox>::new(2);
            slot.incoming = incoming;
            slot.retiring = retiring;
            ends.push(Handoff {
                to_audio,
                from_audio,
            });
        }
        let mut ends = ends.into_iter();
        (
            r,
            std::array::from_fn(|_| ends.next().expect("one Handoff per slot")),
        )
    }

    #[test]
    fn a_swap_hands_the_old_instance_back_instead_of_dropping_it_in_the_callback() {
        // Condition 4 of `PluginBox`'s safety argument: `Instance::drop` calls
        // `terminate`, which frees sample memory and joins worker threads, and
        // that is unbounded work under a real-time deadline.
        let shared = Arc::new(Shared::new());
        let (mut r, mut ends) = renderer_with_handoffs(Arc::clone(&shared));
        ends[1].to_audio.push(PluginBox(None)).expect("room to send");
        assert_eq!(r.swap_plugins(), 1);
        assert!(
            ends[1].from_audio.pop().is_ok(),
            "the old instance never came back, so the callback dropped it"
        );
        assert_eq!(shared.swaps.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn a_slot_refuses_a_new_instrument_when_it_has_nowhere_to_put_the_old_one() {
        // Accepting one with a full retire ring leaves the callback holding two
        // instances and no way to give one back, and the only way out of that is
        // to drop one — on the audio thread, which is the thing that must never
        // happen. The room is checked BEFORE the pop for exactly this reason.
        let shared = Arc::new(Shared::new());
        let (mut r, mut ends) = renderer_with_handoffs(Arc::clone(&shared));
        for _ in 0..2 {
            ends[0].to_audio.push(PluginBox(None)).expect("room to send");
        }
        assert_eq!(r.swap_plugins(), 1);
        assert_eq!(r.swap_plugins(), 1, "the retire ring holds two");
        // Nothing has collected them, so the retire ring is now full.
        ends[0].to_audio.push(PluginBox(None)).expect("room to send");
        assert_eq!(
            r.swap_plugins(),
            0,
            "a new instrument was taken with nowhere to return the old one"
        );
        // And once the UI thread collects, it works again — which is what
        // `hand_off` does before every push.
        while ends[0].from_audio.pop().is_ok() {}
        assert_eq!(r.swap_plugins(), 1);
    }

    #[test]
    fn a_handoff_to_one_slot_leaves_the_other_slots_alone() {
        // The reason there are six rings rather than one pair carrying a slot
        // index: the ring an instance arrives on IS which slot it belongs to,
        // so there is no number to get wrong and no shared queue for one slot's
        // load to sit in front of another's.
        let shared = Arc::new(Shared::new());
        let (mut r, mut ends) = renderer_with_handoffs(Arc::clone(&shared));
        for f in &shared.slot_faulted {
            f.store(true, Ordering::Relaxed);
        }
        ends[2].to_audio.push(PluginBox(None)).expect("room to send");
        assert_eq!(r.swap_plugins(), 1);
        assert!(ends[0].from_audio.pop().is_err(), "slot 1 was retired too");
        assert!(ends[1].from_audio.pop().is_err(), "slot 2 was retired too");
        assert!(ends[2].from_audio.pop().is_ok());
        assert!(
            !shared.slot_faulted[2].load(Ordering::Relaxed),
            "the new instrument's slot must start unfaulted"
        );
        assert!(
            shared.slot_faulted[0].load(Ordering::Relaxed)
                && shared.slot_faulted[1].load(Ordering::Relaxed),
            "loading one instrument cleared another slot's fault, so the band \
             would stop reporting a plugin that is still broken"
        );
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
        let (_, midi) = RingBuffer::<MidiEvent>::new(1024);
        Renderer {
            shared,
            timebase: Timebase::new(),
            rate: RATE,
            dev_channels: dev_ch,
            slots: std::array::from_fn(|_| {
                let (_, incoming) = RingBuffer::<PluginBox>::new(2);
                let (retiring, _) = RingBuffer::<PluginBox>::new(2);
                Slot {
                    plugin: PluginBox(None),
                    incoming,
                    retiring,
                    gain: 1.0,
                }
            }),
            mix: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            midi,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            builtin: crate::dx7::Dx7::new(RATE as f32),
            effects: crate::effects::Effects::new(RATE as f32),
            builtin_slot: None,
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
            count_in_tail: 0,
            chunks: 0,
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

    /// **A fresh install makes a sound.**
    ///
    /// The whole point of the built-in: no plugin loaded, a note arrives, and
    /// audio comes out of the device. Asserted through the real renderer and
    /// not the synth's own unit tests, because the wiring is what was missing
    /// for every release before this one, not the DSP.
    #[test]
    fn a_note_sounds_with_no_plugin_loaded_at_all() {
        let (mut r, mut tx, _) = renderer_with_midi(2);
        let mut out = vec![0.0_f32; 2 * 2048];

        // Silence before anything is played, which is what makes the assertion
        // below mean something.
        r.render(&mut out, 0, 0);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "the device was not silent before a note"
        );

        queue(&mut tx, 0, [0x90, 60, 100]);
        let mut heard = vec![0.0_f32; 2 * 2048];
        r.render(&mut heard, 0, 0);
        let peak = heard.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(
            peak > 0.01,
            "a note with no plugin loaded produced {peak}, which is silence"
        );
        assert!(peak <= 1.0, "the built-in clipped the device at {peak}");
    }

    /// **A patch you pick has to be the patch you hear.** The picker is a list
    /// of names and a list of names is easy to get right while the voice behind
    /// it never reaches the audio thread — the whole chain is a `Mutex` the
    /// renderer checks once a block, and a chain that silently does nothing
    /// looks exactly like a cartridge full of similar patches.
    ///
    /// Two patches chosen to be unmistakably different: an organ (algorithm 32,
    /// six carriers, no modulation) against the tine piano.
    #[test]
    fn choosing_a_patch_changes_what_comes_out() {
        use crate::dx7::{Op, Voice};

        let sound = |r: &mut Renderer, tx: &mut rtrb::Producer<MidiEvent>| {
            queue(tx, 0, [0x90, 60, 100]);
            let mut out = vec![0.0_f32; 2 * 4096];
            r.render(&mut out, 0, 0);
            queue(tx, 0, [0x80, 60, 0]);
            let mut quiet = vec![0.0_f32; 2 * 4096];
            r.render(&mut quiet, 0, 0);
            out
        };

        let (mut r, mut tx, _) = renderer_with_midi(2);
        let piano = sound(&mut r, &mut tx);

        // An organ: every operator a carrier, so it holds where the piano
        // decays and its spectrum is nothing like one.
        let mut organ = Voice::default();
        organ.algorithm = 31;
        organ.feedback = 0;
        organ.ops = [Op {
            rate: [99, 99, 99, 80],
            level: [99, 99, 99, 0],
            output_level: 80,
            coarse: 1,
            detune: 7,
            ..Op::default()
        }; 6];
        // The path a click takes: through `Shared`, picked up by the renderer.
        if let Ok(mut g) = r.shared.pending_voice.lock() {
            *g = Some(organ);
        }
        let after = sound(&mut r, &mut tx);

        let peak = |v: &[f32]| v.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
        assert!(peak(&piano) > 0.01 && peak(&after) > 0.01, "one of them was silent");
        // Not "the buffers differ" — two renders of the SAME patch differ, on
        // free-running phase alone. The organ sustains and the piano does not,
        // so the end of the block is where they cannot be confused.
        let tail = |v: &[f32]| peak(&v[v.len() * 3 / 4..]);
        assert!(
            tail(&after) > tail(&piano) * 2.0,
            "the patch never reached the audio thread: the tine piano ended at \
             {:.4} and the organ at {:.4}, and an organ does not decay",
            tail(&piano),
            tail(&after)
        );
    }

    /// **The knobs reach the audio, and they reach the FILE.**
    ///
    /// The effects sit on the instrument sum, upstream of the tap, so a take
    /// carries what was heard. Asserted through the real renderer because the
    /// position in the chain is the whole feature: `effects.rs` proves the DSP
    /// against an impulse, and none of that says whether it is wired in.
    #[test]
    fn the_effect_knobs_reach_both_the_device_and_the_take() {
        let tail = |reverb: f32| {
            let (mut r, mut tx, _) = renderer_with_midi(2);
            r.shared.set_effects(reverb, 0.0);
            queue(&mut tx, 0, [0x90, 60, 100]);
            let mut out = vec![0.0_f32; 2 * 4096];
            r.render(&mut out, 0, 0);
            queue(&mut tx, 0, [0x80, 60, 0]);
            // Long after the note is released and the built-in has decayed:
            // anything here is a room, because nothing else is left to make it.
            for _ in 0..6 {
                let mut quiet = vec![0.0_f32; 2 * 4096];
                r.render(&mut quiet, 0, 0);
            }
            let mut last = vec![0.0_f32; 2 * 4096];
            r.render(&mut last, 0, 0);
            last.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
        };
        let dry = tail(0.0);
        let wet = tail(1.0);
        assert!(
            wet > dry * 4.0 && wet > 1.0e-5,
            "the reverb knob is not wired in: dry tail {dry:e}, wet tail {wet:e}"
        );
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
        assert!(r.slots.iter().all(|s| s.plugin.0.is_none()));
        r.render(&mut out, 0, 0);
        assert_eq!(r.midi.slots(), 0);
    }

    #[test]
    fn the_queue_is_drained_once_a_block_and_every_slot_reads_the_same_list() {
        // The trap this exists for: draining the MIDI ring inside the per-slot
        // render loop. It compiles, it looks symmetrical, and it gives each
        // event to whichever slot happened to pop first — the piano plays the
        // C, the pad plays the E, and nobody can work out why a chord came out
        // as an arpeggio across three instruments.
        //
        // With no plugin loaded there is nothing to render, so what is asserted
        // is the shape: after a block, the events are sitting in ONE list that
        // every slot was handed by reference, and the queue behind it is empty.
        let (mut r, mut tx, _) = renderer_with_midi(2);
        queue(&mut tx, 0, [0x90, 60, 100]);
        queue(&mut tx, 0, [0x90, 64, 100]);
        queue(&mut tx, 0, [0x90, 67, 100]);
        let mut out = vec![0.0f32; 512 * 2];
        r.render(&mut out, 0, 0);
        assert_eq!(
            r.notes.len(),
            3,
            "all three notes must survive in one shared list, not be shared out"
        );
        assert_eq!(r.midi.slots(), 0, "and the queue must be empty afterwards");
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
    fn a_slot_gain_reaches_zero_without_a_step_discontinuity() {
        // A gain that jumps is a click, which is the one thing the metronome is
        // supposed to have a monopoly on. The smoother runs whether or not the
        // slot has an instrument in it, so that a slot filled a minute after the
        // fader moved does not start at the gain the fader used to have and jump
        // to the one it has now.
        let shared = Arc::new(Shared::new());
        let (tap_tx, _rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        r.gain_coeff = gain_coefficient(RATE);
        for slot in &mut r.slots {
            slot.gain = 1.0;
        }
        shared.set_slot_gain(0, 0.0);
        let mut out = vec![0.0f32; 8 * 2];
        r.render(&mut out, 0, 0);
        assert!(
            r.slots[0].gain > 0.9,
            "a 10 ms smoother must not cross most of its range in 8 frames \
             (it reached {})",
            r.slots[0].gain
        );
        // 600 blocks of 8 frames is 4800 frames, 100 ms, ten time constants.
        for _ in 0..600 {
            r.render(&mut out, 0, 0);
        }
        assert!(
            r.slots[0].gain < 0.01,
            "the smoother never arrived ({})",
            r.slots[0].gain
        );
        assert_eq!(
            (r.slots[1].gain, r.slots[2].gain),
            (1.0, 1.0),
            "one fader moved and took the other two with it"
        );
    }

    #[test]
    fn a_gain_written_to_a_slot_that_does_not_exist_is_ignored_rather_than_panicking() {
        // Reached from UI code, where this app's panic hook turns a panic into a
        // dialog and `exit(1)`. A menu that got its arithmetic wrong should
        // misbehave, not end the session.
        let shared = Shared::new();
        shared.set_slot_gain(SLOTS, 0.25);
        shared.set_slot_gain(usize::MAX, 0.25);
        for cell in &shared.slot_gains {
            assert_eq!(Shared::f32_of(cell), 1.0, "a real slot was written instead");
        }
        shared.set_slot_gain(SLOTS - 1, 0.25);
        assert_eq!(Shared::f32_of(&shared.slot_gains[SLOTS - 1]), 0.25);
    }

    #[test]
    fn a_faulted_slot_is_named_so_that_the_other_two_are_not_blamed_for_it() {
        // "The instrument stopped rendering" is a sentence a user with three of
        // them cannot act on.
        let shared = Shared::new();
        assert!(shared.instrument_fault().is_none());
        shared.slot_faulted[1].store(true, Ordering::Relaxed);
        let why = shared.instrument_fault().expect("a fault was latched");
        assert!(why.contains("instrument 2"), "{why}");
        shared.slot_faulted[2].store(true, Ordering::Relaxed);
        let why = shared.instrument_fault().expect("two faults were latched");
        assert!(why.contains("2, 3"), "both faulted slots must be named: {why}");
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
        let (size, frames) = pick_buffer(&cpal::SupportedBufferSize::Unknown, None);
        assert!(matches!(size, cpal::BufferSize::Default));
        assert_eq!(frames, None);
        let (size, frames) = pick_buffer(&cpal::SupportedBufferSize::Range { min: 512, max: 4096 }, None);
        assert!(matches!(size, cpal::BufferSize::Fixed(512)));
        assert_eq!(frames, Some(512), "the request is clamped up to the minimum");
        let (_, frames) = pick_buffer(&cpal::SupportedBufferSize::Range { min: 16, max: 64 }, None);
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
        assert!(!engine.any_plugin_loaded());
        assert!((0..SLOTS).all(|s| engine.plugin(s).is_none()));
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
    #[ignore = "needs an audio output device and a real VST3 instrument; run with \
                `cargo test -p ivory -- --ignored a_slots_state --nocapture`"]
    fn a_slots_state_can_be_saved_while_it_plays_and_handed_back_to_the_next_load() {
        // The engine's half of the persistence story: the instance is inside
        // the audio callback the whole time, so this exercises the second
        // reference to its processor rather than any path that stops the audio.
        // That the RESTORED SOUND is right is proved in `ivory-host`'s
        // `instance.rs`, where a rendered probe can be measured; here the claim
        // is that the bytes can be got at all while the stream runs, and that a
        // bad blob costs the preset rather than the piano.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_plugin(0, &bundle, None).expect("load");
        assert!(engine.state_error(0).is_none());

        let now = engine.timebase().now();
        engine.send_midi(now, &[0x90, 60, 100]);
        let bytes = engine
            .save_slot_state(0)
            .expect("a loaded slot has state to save");
        engine.send_midi(engine.timebase().now(), &[0x80, 60, 64]);
        println!("slot 0 state: {} bytes", bytes.len());
        assert!(bytes.len() > 18, "only the container came back");
        assert!(engine.is_running(), "saving state stopped the stream");
        assert!(engine.fault().is_none(), "{:?}", engine.fault());

        // An empty slot has nothing to save, and neither has a slot that does
        // not exist. Both are `None` rather than an error, because the caller's
        // only move either way is to write nothing.
        assert!(engine.save_slot_state(1).is_none());
        assert!(engine.save_slot_state(SLOTS).is_none());

        engine
            .load_plugin_with_state(0, &bundle, None, Some(&bytes))
            .expect("reload with state");
        assert!(
            engine.state_error(0).is_none(),
            "a blob this engine just wrote was refused: {:?}",
            engine.state_error(0)
        );

        // And the case that will actually happen: a settings file that has been
        // truncated, edited, or written by a different plugin. The instrument
        // must still load.
        let mut broken = bytes.clone();
        broken.truncate(broken.len() / 2);
        engine
            .load_plugin_with_state(0, &bundle, None, Some(&broken))
            .expect("a corrupt blob must not stop the instrument loading");
        assert!(
            engine.state_error(0).is_some(),
            "a truncated blob was silently accepted, which is how 'it keeps \
             forgetting my piano' becomes unreportable"
        );
        assert!(engine.plugin(0).is_some(), "the instrument did not load");
        // ...and the error does not outlive the slot.
        engine.unload_plugin(0);
        assert!(engine.state_error(0).is_none());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored a_real_instrument`"]
    fn a_real_instrument_is_heard_and_can_be_swapped_while_the_stream_runs() {
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        let loaded = engine.load_plugin(0, &bundle, None).expect("load");
        assert_eq!(engine.plugin(0), Some(&loaded));
        assert!(engine.any_plugin_loaded());
        assert!(engine.warm_up(0).is_some_and(|w| w.heard), "warmed up silent");

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
        engine.unload_plugin(0);
        assert!(engine.plugin(0).is_none());
        assert!(!engine.any_plugin_loaded());
        engine.load_plugin(0, &bundle, None).expect("reload");
        assert_eq!(tap.channels(), TAP_CHANNELS, "the tap changed width mid-take");
        assert!(engine.is_running());
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored two_slots --nocapture`"]
    fn two_slots_holding_the_same_instrument_are_twice_as_loud_as_one() {
        // The claim of the whole feature, measured rather than asserted: the
        // same instrument playing the same notes twice sums coherently, so the
        // peak must double. It is the only way to tell "both slots got the
        // notes" from "one slot got them and the other is silent", which every
        // structural test in this file would happily pass.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_plugin(0, &bundle, None).expect("slot 1");
        for slot in 0..SLOTS {
            engine.set_slot_gain(slot, 0.4);
        }
        let one = chord_peak(&engine);
        engine.load_plugin(1, &bundle, None).expect("slot 2");
        let two = chord_peak(&engine);
        println!("one slot {one:.4}, two slots {two:.4}, ratio {:.2}x", two / one);
        assert!(one > 0.001, "the first instrument was never heard ({one})");
        // Wide bounds on purpose: a real piano's peak is a transient and the
        // two instances are separately seeded. 1.5x rules out "the second slot
        // is silent" (1.0x) and 2.5x rules out "something is summing twice".
        assert!(
            (1.5..2.5).contains(&(two / one)),
            "two slots measured {:.2}x one, which is neither a layered pair \
             (2.0x) nor a slot that heard nothing (1.0x)",
            two / one
        );
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored unloading_one_slot`"]
    fn unloading_one_slot_leaves_the_others_playing() {
        // The per-slot handoff in one test: the rings slot 1 uses are not the
        // rings slot 0 uses, so retiring one instrument cannot take the other
        // with it.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_plugin(0, &bundle, None).expect("slot 1");
        engine.load_plugin(1, &bundle, None).expect("slot 2");
        for slot in 0..SLOTS {
            engine.set_slot_gain(slot, 0.4);
        }
        assert!(chord_peak(&engine) > 0.001);

        engine.unload_plugin(1);
        assert!(engine.plugin(1).is_none());
        assert!(engine.plugin(0).is_some(), "the wrong slot was retired");
        assert!(engine.any_plugin_loaded());
        let alone = chord_peak(&engine);
        assert!(
            alone > 0.001,
            "unloading slot 2 silenced slot 1 as well ({alone})"
        );
        assert!(engine.is_running());
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored a_slot_that_does_not_exist`"]
    fn a_slot_that_does_not_exist_is_an_error_and_never_a_panic() {
        // Every one of these is reachable from a menu, and a panic here goes
        // through the app's hook to a dialog and `exit(1)`.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        assert!(engine.load_plugin(SLOTS, &bundle, None).is_err());
        assert!(engine.open_editor(SLOTS).is_err());
        engine.unload_plugin(SLOTS);
        engine.close_editor(SLOTS);
        engine.set_slot_gain(SLOTS, 0.5);
        assert!(!engine.has_editor(SLOTS));
        assert!(!engine.editor_open(SLOTS));
        assert!(engine.plugin(SLOTS).is_none());
        assert!(engine.warm_up(SLOTS).is_none());
        assert!(engine.is_running(), "a bad slot number stopped the stream");
    }

    /// The one instrument every one of these tests needs, found the way the
    /// probe finds it.
    fn pianoteq() -> PathBuf {
        let bundles = ivory_host::discover();
        let Some(bundle) = bundles.into_iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains("pianoteq"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pianoteq; this test needs one installed");
        };
        bundle
    }

    /// Play a chord, hold it for half a second, and report the loudest thing the
    /// device mix saw. Releases the notes afterwards, so the next call measures
    /// its own chord rather than the last one still ringing.
    fn chord_peak(engine: &Engine) -> f32 {
        // Read once to clear the peak: it is read-and-clear by design, and a
        // stale peak from the previous chord would be this one's answer.
        engine.meters();
        let now = engine.timebase().now();
        for pitch in [60u8, 64, 67] {
            engine.send_midi(now, &[0x90, pitch, 100]);
        }
        let mut peak = 0.0f32;
        let until = Instant::now() + Duration::from_millis(700);
        while Instant::now() < until {
            peak = peak.max(engine.meters().peak());
            std::thread::sleep(Duration::from_millis(8));
        }
        for pitch in [60u8, 64, 67] {
            engine.send_midi(engine.timebase().now(), &[0x80, pitch, 64]);
        }
        // Let the release finish, or the next chord starts on top of this one's
        // tail and measures both.
        std::thread::sleep(Duration::from_millis(800));
        peak
    }
}
