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
///
/// **And so is the take**, because the take IS the tap — see
/// `record::TakeSource`. The file used to be as wide as the INPUT, so choosing
/// a mono microphone folded a stereo piano down to mono. That was defensible
/// while a take could be the dry microphone alone; it is not defensible now
/// that a take is the desk.
pub const TAP_CHANNELS: usize = 2;

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
                "{} frames, {:.1} ms buffer at {} Hz{}",
                f,
                f as f32 * 1_000.0 / self.sample_rate as f32,
                self.sample_rate,
                // **The ring, where it is not the same as the period.** On
                // Linux the two differ by four (see `BUFFER_PERIODS`), so a
                // line that named only one of them would be answering a
                // different question than the one somebody debugging dropouts
                // is asking. Everywhere else they are the same number and
                // saying it twice would be noise.
                if BUFFER_PERIODS > 1 {
                    format!("  ({} frame ring)", f * BUFFER_PERIODS)
                } else {
                    String::new()
                }
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

/// A channel of the mixer, and the bit it owns in `muted` / `soloed`.
///
/// **The master is not here on purpose.** Muting the master is the same as
/// turning it down and soloing it means nothing, so a strip that cannot do
/// either does not get a bit that would have to be defended against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Strip {
    /// One general channel, MIDI or AUDIO by the desk's choice. See the UI's
    /// own `Strip` — the kind is not in the enum there either, and for the
    /// same reason: cycling a channel must not renumber it.
    Channel(usize),
    Track,
    Click,
    Fx,
}

/// How many general channels the desk has. Mirrors the UI's constant; the
/// `From` impl below is what keeps the two enums provably the same shape.
pub const CHANNELS: usize = ivory_ui::recorder::CHANNELS;

/// How many inputs of one interface the desk has room for.
///
/// **Asserted equal to `ivory_record::audio::MAX_PICKS`** — the capture cannot
/// keep more than that, and a desk with more strips than the capture has picks
/// would draw channels nothing can ever fill. `ivory-ui` declares its own copy
/// because it may not reach either of these; the test below is what keeps all
/// three the same number.
pub const INPUTS: usize = ivory_ui::recorder::INPUTS;

/// How many channels the desk has, master aside.
pub const STRIPS: usize = CHANNELS + 3;

impl Strip {
    /// Its place in the arrays, and the bit it owns.
    pub const fn index(self) -> usize {
        match self {
            Strip::Channel(i) => i,
            Strip::Track => CHANNELS,
            Strip::Click => CHANNELS + 1,
            Strip::Fx => CHANNELS + 2,
        }
    }

    fn bit(self) -> u32 {
        1 << self.index()
    }
}

impl From<ivory_ui::recorder::Strip> for Strip {
    /// **Exhaustive on purpose.** The UI declares its own `Strip` because it
    /// may not reach across the firewall for this one; adding a channel there
    /// and forgetting it here would silently route the new strip to whatever
    /// index happened to line up. This way it does not compile.
    fn from(ui: ivory_ui::recorder::Strip) -> Self {
        use ivory_ui::recorder::Strip as Ui;
        match ui {
            Ui::Channel(i) => Strip::Channel(i),
            Ui::Track => Strip::Track,
            Ui::Click => Strip::Click,
            Ui::Fx => Strip::Fx,
        }
    }
}

/// Whether a strip is heard, given the mute and solo masks.
///
/// **Solo is exclusive and mute loses to it.** With anything soloed, only the
/// soloed strips are heard — including a soloed strip that is also muted,
/// because pressing solo on a muted channel is unambiguously a request to hear
/// it and the alternative is a solo button that sometimes does nothing.
fn strip_is_heard(strip: Strip, muted: u32, soloed: u32) -> bool {
    if soloed != 0 {
        return soloed & strip.bit() != 0;
    }
    muted & strip.bit() == 0
}

/// Add what came back from the effects bus, at the bus's own fader.
fn add_return(
    mix: &mut [f32],
    aux: &[f32],
    frames: usize,
    gain: &mut f32,
    target: f32,
    coeff: f32,
) -> [f32; 2] {
    let mut peak = [0.0f32; 2];
    for f in 0..frames {
        *gain += (target - *gain) * coeff;
        let at = f * TAP_CHANNELS;
        for c in 0..TAP_CHANNELS {
            let (Some(v), Some(a)) = (mix.get_mut(at + c), aux.get(at + c)) else {
                return peak;
            };
            let wet = *a * *gain;
            peak[c.min(1)] = peak[c.min(1)].max(wet.abs());
            *v += wet;
        }
    }
    peak
}

impl Shared {
    /// Keep the loudest sample a strip has made since the UI last looked.
    ///
    /// `fetch_max` on the BITS, which is only correct for non-negative floats
    /// — and these are magnitudes, so they are. The same trick the limiter's
    /// gain reduction uses two fields up.
    fn note_strip_peak(&self, strip: Strip, peak: [f32; 2]) {
        let at = &self.strip_peak[strip.index()];
        for (slot, v) in at.iter().zip(peak) {
            if v > 0.0 {
                slot.fetch_max(v.to_bits(), Ordering::Relaxed);
            }
        }
    }
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
    /// One fader per general channel, whatever the channel's kind.
    channel_gains: [AtomicU32; CHANNELS],
    /// Which general channel the built-in DX7 belongs to: a DESK INDEX, or -1
    /// for none. It stopped being a slot when the slots stopped existing on
    /// the desk; the migration guarantees a desk with no instrument anywhere
    /// gets one channel seeded with the built-in, so there is no silent
    /// fallback left to need.
    builtin_strip: AtomicI64,
    /// Which general channel each capture PICK feeds: desk indices, -1 for an
    /// unbound pick. The k-th AUDIO channel takes pick k — the HOST computes
    /// the order, the callback only reads it, which is the same trust split
    /// every other mask here follows.
    pick_strip: [AtomicI64; INPUTS],
    metro_gain: AtomicU32,
    /// The backing track's level, linear.
    track_gain: AtomicU32,
    /// Whether the backing track should be rolling. Set by the transport.
    /// Where to go, and the generation that makes it an EVENT.
    ///
    /// Two fields, because a bare store cannot be told from the same value
    /// re-asserted — which is exactly how the old `set_track_playing` level
    /// made a locate inexpressible: the edges were found ON the audio thread,
    /// so a seek while rolling changed nothing and a seek while stopped was
    /// overwritten by the rewind. The host stores the payload first and the
    /// generation last, Release; the callback consumes generation-first,
    /// Acquire. See `Renderer::run_transport`.
    transport_at: AtomicU64,
    transport_req: AtomicU64,
    /// The generation the callback has APPLIED. `ack == req` means
    /// `transport_pos` answers the question the host asked.
    transport_ack: AtomicU64,
    /// The transport is moving. A LEVEL, and correctly so — "should I be
    /// moving" genuinely is a level; only the seek was ever an event wearing a
    /// level's clothes. Safe to re-assert every frame, because nothing keys
    /// off its edges any more.
    rolling: AtomicBool,
    /// The frame the callback reached at the end of the last chunk.
    transport_pos: AtomicU64,
    /// A backing track has been handed over, playing or not.
    ///
    /// Read from the UI thread, which cannot see `Renderer::track` — that lives
    /// on the audio thread. It exists so that "is there anything on the
    /// instrument bus" can be answered without one, which is the question
    /// `TakeSource::resolve` has to ask before every take.
    track_loaded: AtomicBool,
    /// **Input monitoring: hear what the microphone hears.**
    ///
    /// Never persisted, anywhere. It is off at every launch by construction —
    /// see `IvoryApp::input_monitor` — because the failure mode is a room full
    /// of feedback the moment somebody turns their speakers on after a relaunch
    /// they had forgotten was monitoring.
    monitor_on: AtomicBool,
    /// Where the track starts and stops, in frames. `out` of zero means "to
    /// the end", so a clip with no trim needs no knowledge of its own length
    /// down here.

    /// A live-input ring on its way to the renderer, or `None` taking one away.
    ///
    /// Behind a mutex and picked up with `try_lock`, exactly like the backing
    /// track: the ring's read end is `!Sync` and belongs to one thread, and the
    /// audio thread may not wait on the UI thread to hand it over.
    /// The live input's ring, its total channel count, and how those channels
    /// are shared out between the input strips.
    ///
    /// **The widths are the other end of `Picks`.** The capture laid the
    /// chosen inputs end to end in one stream — a mono input then a stereo
    /// pair is one channel then two — and without the widths this side would
    /// have a block of three channels and no idea which strip owns which.
    pending_monitor:
        std::sync::Mutex<Option<Option<(rtrb::Consumer<f32>, u16, [u8; INPUTS], u32)>>>,
    /// Frames of the monitor ring that were STALE and thrown away, so that what
    /// is heard is what is happening.
    ///
    /// Non-zero at the start of every session by design — see `mix_monitor` —
    /// and climbing afterwards means the two device clocks are pulling apart.
    monitor_slip: AtomicU64,
    /// How much audio the monitor ring is holding, in frames: the monitoring
    /// path's own latency, on top of the two device buffers, as a number.
    monitor_backlog: AtomicU32,
    /// Bit `i` set: channel `i`'s rack voices ignore the note stream.
    ///
    /// **The MIDI arm, as one mask**, exactly the shape mute and solo travel
    /// in. Zero is every channel armed, which is the desk this app has always
    /// had — an empty channel armed plays nothing, so five loaded instruments
    /// still layer, and switching a channel to AUDIO is what sets its bit.
    midi_off: AtomicU32,
    /// The master, as a LINEAR gain. The last thing on the instrument bus,
    /// after the limiter, reaching both the device mix and the take — the same
    /// rule the effects follow.
    ///
    /// **Not the click.** The click has its own fader and is added to the
    /// device mix after this, which is why the master and the click behave
    /// like two faders on a desk rather than one inside the other.
    master_gain: AtomicU32,
    /// Decibels of gain reduction the limiter has applied since the UI last
    /// looked — a positive number, zero for none. Read and reset.
    gr_db: AtomicU32,
    /// How much of each strip goes to the effects bus, 0..=1, indexed by
    /// [`Strip::index`].
    ///
    /// **Post-fader, which is the one that behaves.** Pull a channel down and
    /// its reverb comes down with it; a pre-fader send is a thing you want
    /// twice a year and explains itself badly the other three hundred days.
    ///
    /// Every instrument slot defaults to 1.0 and everything else to 0.0, which
    /// is exactly the routing this app had when the effects were an insert on
    /// the instrument bus — so an upgrade sounds identical.
    send: [AtomicU32; STRIPS],
    /// The effects bus's own fader, applied to what comes back from it.
    fx_return: AtomicU32,
    /// The loudest sample each strip produced since the UI last looked, as
    /// f32 bits. Read and RESET, like the limiter's gain reduction beside it:
    /// a peak is a transient and an average would read as almost nothing on
    /// exactly the material a meter is there for.
    strip_peak: [[AtomicU32; 2]; STRIPS],
    /// One bit per strip. See [`Strip`].
    ///
    /// **Two masks rather than a flag each**, because solo is a question about
    /// ALL the strips at once — "is anything soloed" decides what every other
    /// strip does — and reading eight atomics to answer it would be eight
    /// chances for the answer to change halfway through.
    muted: AtomicU32,
    soloed: AtomicU32,
    /// The six effect knobs, 0..=1. Three of them are sends on the effects bus
    /// and three are inserts on the master — see `effects.rs`, and see
    /// `Renderer::render` for which is which and why.
    reverb_mix: AtomicU32,
    delay_mix: AtomicU32,
    chorus_mix: AtomicU32,
    hpf_mix: AtomicU32,
    lpf_mix: AtomicU32,
    limiter_mix: AtomicU32,
    /// What each effect is set to, behind a lock.
    ///
    /// **A lock, unlike the knobs above.** There are eleven of these and they
    /// only move when somebody opens a menu, so the renderer reaches for them
    /// once a block and only when `params_dirty` says something changed. Eleven
    /// atomics would be eleven names to keep in step for a value that is read
    /// as a whole or not at all.
    params: Mutex<crate::effects::Params>,
    params_dirty: AtomicBool,
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
            channel_gains: std::array::from_fn(|_| AtomicU32::new(1.0f32.to_bits())),
            builtin_strip: AtomicI64::new(-1),
            pick_strip: std::array::from_fn(|_| AtomicI64::new(-1)),
            metro_gain: AtomicU32::new(0.7f32.to_bits()),
            // Both effects OFF by default. A first launch has to sound like the
            // instrument and not like a room, and `Effects::process` skips its
            // whole cost at zero.
            track_gain: AtomicU32::new(1.0f32.to_bits()),
            transport_at: AtomicU64::new(0),
            transport_req: AtomicU64::new(0),
            transport_ack: AtomicU64::new(0),
            rolling: AtomicBool::new(false),
            transport_pos: AtomicU64::new(0),
            track_loaded: AtomicBool::new(false),
            monitor_on: AtomicBool::new(false),
            pending_monitor: std::sync::Mutex::new(None),
            monitor_slip: AtomicU64::new(0),
            monitor_backlog: AtomicU32::new(0),
            midi_off: AtomicU32::new(0),
            master_gain: AtomicU32::new(1.0f32.to_bits()),
            // The first channel — the desk's default instrument — sends
            // everything and nothing else sends anything, which is the same
            // sound the old five-slots-send-all default made. See the UI's
            // `Desk::default`, which this mirrors.
            send: std::array::from_fn(|i| {
                AtomicU32::new(if i == 0 { 1.0f32 } else { 0.0f32 }.to_bits())
            }),
            fx_return: AtomicU32::new(1.0f32.to_bits()),
            strip_peak: std::array::from_fn(|_| [AtomicU32::new(0), AtomicU32::new(0)]),
            muted: AtomicU32::new(0),
            soloed: AtomicU32::new(0),
            gr_db: AtomicU32::new(0),
            reverb_mix: AtomicU32::new(0.0f32.to_bits()),
            hpf_mix: AtomicU32::new(0.0f32.to_bits()),
            lpf_mix: AtomicU32::new(0.0f32.to_bits()),
            // **One, not zero.** The limiter's dial is a threshold and its
            // resting position is fully clockwise; starting this at zero is a
            // -48 dB threshold on an engine nobody has touched yet.
            limiter_mix: AtomicU32::new(1.0f32.to_bits()),
            delay_mix: AtomicU32::new(0.0f32.to_bits()),
            chorus_mix: AtomicU32::new(0.0f32.to_bits()),
            params: Mutex::new(crate::effects::Params::default()),
            params_dirty: AtomicBool::new(false),
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
            pending_voice: std::sync::Mutex::new(None),
            running: AtomicBool::new(false),
            callbacks: AtomicU64::new(0),
            swaps: AtomicU64::new(0),
        }
    }

    fn f32_of(cell: &AtomicU32) -> f32 {
        f32::from_bits(cell.load(Ordering::Relaxed))
    }

    fn set_track_gain(&self, linear: f32) {
        self.track_gain
            .store(sane_gain(linear).to_bits(), Ordering::Relaxed);
    }

    /// One channel fader. An out-of-range channel is a no-op, never a panic:
    /// this is reached from UI code, where the panic hook turns a panic into
    /// a dialog and `exit(1)`.
    fn set_channel_gain(&self, ch: usize, linear: f32) {
        if let Some(cell) = self.channel_gains.get(ch) {
            cell.store(sane_gain(linear).to_bits(), Ordering::Relaxed);
        }
    }

    /// All three effect knobs at once, 0..=1.
    ///
    /// Sanitised here rather than at the point of use, for the same reason the
    /// gains are: a NaN reaching a feedback loop does not stay in one sample.
    fn set_effects(&self, sends: crate::effects::Sends) {
        let sane = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 };
        self.reverb_mix
            .store(sane(sends.reverb).to_bits(), Ordering::Relaxed);
        self.delay_mix
            .store(sane(sends.delay).to_bits(), Ordering::Relaxed);
        self.chorus_mix
            .store(sane(sends.chorus).to_bits(), Ordering::Relaxed);
        self.hpf_mix
            .store(sane(sends.hpf).to_bits(), Ordering::Relaxed);
        self.lpf_mix
            .store(sane(sends.lpf).to_bits(), Ordering::Relaxed);
        self.limiter_mix
            .store(sane(sends.limiter).to_bits(), Ordering::Relaxed);
    }

    /// What the effects are set to. Picked up by the renderer next block.
    ///
    /// **Only when it CHANGED.** This is pushed every frame with the gains, and
    /// taking a lock sixty times a second to write the same eleven numbers is a
    /// lock the audio thread can contend with for nothing.
    fn set_effect_params(&self, p: crate::effects::Params) {
        let Ok(mut g) = self.params.lock() else { return };
        if *g != p {
            *g = p;
            self.params_dirty.store(true, Ordering::Release);
        }
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
    /// **Whether this plugin GENERATES rather than transforms.**
    ///
    /// Decided once, at load time, from the class's own `subCategories` — and
    /// carried IN the box, because the audio thread may not trust an index a
    /// UI thread wrote about which bay holds what. A voice in a rack is fed
    /// the block's notes and REPLACES the channel's signal; an effect is fed
    /// the channel's signal and transforms it in place. This is the whole of
    /// the difference between an instrument and an insert, and it travels
    /// with the instance it describes.
    voice: bool,
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

/// A clip and where it begins on the timeline, in frames.
///
/// **`start` is 0 for the backing track, and it is one field and one
/// subtraction** — the whole difference between "the backing track" and "a
/// clip". It rides WITH the clip rather than in a separate atomic, so a clip
/// and its position can never disagree; the renderer positions it as
/// `transport_pos - start` and everything before the clip or past its end is
/// silence. A second clip on a lane needs no new concept and no change to the
/// handover protocol.
#[derive(Clone)]
pub struct Placed {
    pub clip: Arc<ivory_record::decode::Clip>,
    pub start: u64,
}

/// How many effect inserts a channel has room for. See the UI's own copy.
pub const INSERTS: usize = ivory_ui::recorder::INSERTS;

/// One channel's insert chain: up to [`INSERTS`] effects, in series.
///
/// **A rack per channel, and each slot its own hand-off.** The pair of rings
/// IS the routing, exactly as it is for an instrument slot — an instance
/// arriving on this consumer belongs in this slot of this channel, and no
/// index has to say so. One pair carrying `(where, PluginBox)` would need the
/// audio thread to trust a number a UI thread wrote.
///
/// An empty rack costs nothing: `run` returns before it touches a buffer, and
/// the channel folds by exactly the path it did before racks existed.
struct Rack {
    slots: [PluginBox; INSERTS],
    incoming: [Consumer<PluginBox>; INSERTS],
    retiring: [Producer<PluginBox>; INSERTS],
}

impl Rack {
    /// Take whatever the UI thread has sent, and give back what it displaces.
    ///
    /// Same shape as an instrument slot's swap and for the same reason: the
    /// callback never DROPS a plugin — dropping runs the vendor's teardown,
    /// which is not something to do between two blocks of audio — it returns
    /// it for the UI thread to drop.
    fn swap(&mut self) -> usize {
        let mut swapped = 0;
        for i in 0..INSERTS {
            // **Room to give the old one back, checked BEFORE taking the new
            // one.** This was `while let Ok(next) = incoming.pop()` followed by
            // `let _ = retiring.push(old)`, and `let _` on an rtrb push is a
            // trap: `PushError::Full(T)` carries the value, so discarding the
            // `Result` DROPS the `PluginBox` — which runs `IComponent::terminate`,
            // frees the vendor's sample memory and joins its worker threads,
            // on the audio callback, between two blocks.
            //
            // It is reachable: `hand_off_insert` gives up after `RETIRE_TIMEOUT`
            // and leaves the retired plugin sitting in the ring, so the next
            // load finds `retiring` full.
            //
            // The instrument slots have always had this right — see
            // `swap_plugins`, which tests `retiring.slots() == 0` first and
            // leaves the arrival in its ring for the next block. This is that,
            // and the two protocols are now the same protocol.
            if self.retiring[i].slots() == 0 {
                continue;
            }
            let Ok(next) = self.incoming[i].pop() else {
                continue;
            };
            let old = std::mem::replace(&mut self.slots[i], next);
            // Cannot fail: the room was checked one line above and this is the
            // only producer. Still not `unwrap` — a panic here is a panic on
            // the audio thread.
            let _ = self.retiring[i].push(old);
            swapped += 1;
        }
        swapped
    }

    /// Whether anything is loaded. The whole point of asking is to leave the
    /// common path exactly as it was.
    fn is_empty(&self) -> bool {
        self.slots.iter().all(|s| s.0.is_none())
    }
}

/// Run a rack over one channel's PLANAR audio, in place.
///
/// A free function rather than a method, because every caller holds the rack
/// and the scratch as separate fields of the renderer and the borrow checker
/// only sees that if they are named separately.
///
/// `planar[c]` must hold at least `frames`. Channels a plugin does not write
/// keep what they had, and a plugin that answers with fewer channels than it
/// was given has its last one read twice rather than leaving a silent side —
/// the same rule the bus insert has always followed.
fn run_rack(
    rack: &mut Rack,
    planar: &mut [Vec<f32>],
    frames: usize,
    out: &mut [Vec<f32>],
    notes: &[ivory_host::Note],
    controls: &[ivory_host::Control],
) {
    for slot in &mut rack.slots {
        let Some(p) = slot.0.as_mut() else {
            continue;
        };
        if p.voice {
            // **A voice renders into its OWN buffers and replaces the
            // channel.** Its own, because `fx_out` is TAP_CHANNELS wide and an
            // instrument's main bus is as wide as it likes — Pianoteq's is
            // eight — and `process_through` refuses a block whose output is
            // narrower than the bus. `bufs` was allocated at the instrument's
            // width in `load_insert` for exactly this call.
            //
            // Replaces rather than mixes: a MIDI channel's signal IS its
            // instrument, and whatever was in `planar` before bay 1 ran is
            // silence or an upstream voice being superseded.
            if p.inst.process_with_controls(notes, controls, frames, &mut p.bufs).is_err() {
                continue;
            }
        } else if p.inst.process_effect(planar, frames, out).is_err() {
            // A refused block leaves the audio ALONE rather than replacing it
            // with whatever was in the output buffers: a faulted insert is a
            // channel that stops being processed, not one that starts making
            // noise.
            continue;
        }
        let wrote = p.channels.max(1);
        let src_bufs: &[Vec<f32>] = if p.voice { &p.bufs } else { &out[..] };
        for c in 0..planar.len() {
            let Some(src) = src_bufs.get(c.min(wrote - 1)) else {
                continue;
            };
            let Some(dst) = planar.get_mut(c) else {
                continue;
            };
            dst[..frames].copy_from_slice(&src[..frames]);
        }
    }
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
    /// The backing track, and where in it the transport is.
    ///
    /// **Held by the renderer, swapped in whole.** A clip is a hundred
    /// megabytes of `Vec<f32>` and the audio thread must never allocate or
    /// free one, so it arrives behind an `Arc` through `pending_track` and the
    /// old one is dropped by whoever pushed the new one.
    track: Option<Placed>,
    /// The transport's position in frames, advanced by `run_transport` and
    /// nothing else. Project frames, not clip frames: a clip positions itself
    /// as `pos - placed.start`.
    pos: u64,
    track_gain: f32,
    /// The transport generation this callback has applied. See `run_transport`.
    seen_req: u64,
    /// Whether the last chunk rolled, for the note flush on a falling edge —
    /// renderer-local, because nothing OUTSIDE the callback keys off edges any
    /// more; this one exists so a stop can end the notes it strands.
    was_rolling: bool,
    /// Every pitch currently sounding, from the note stream this renderer
    /// itself handed out. What a stop or a jump has to end.
    sounding: u128,
    /// The clip arriving, and the one going back to be dropped where it came
    /// from. The pair IS the routing, exactly as a plugin's is.
    track_incoming: Consumer<Option<Placed>>,
    track_retiring: Producer<Option<Placed>>,
    builtin: crate::dx7::Dx7,
    /// The built-in's own output, before its slot gain.
    ///
    /// **It needs a buffer of its own for the same reason a plugin does.** The
    /// FM renders by ADDING into whatever it is handed, so rendering it
    /// straight onto the bus leaves no moment at which its contribution is
    /// separable — and therefore no moment at which a fader can be applied to
    /// it. That is exactly what happened: the DX7's slot fader moved a number
    /// that reached the settings, the engine and the meter, and never once
    /// reached the audio.
    builtin_scratch: Vec<f32>,
    /// Where each general channel's fader has slewed to. Per channel, because
    /// the eight move independently — the same treatment every fader gets.
    channel_gain: [f32; CHANNELS],
    /// Reverb, delay and chorus on the instrument sum. Free at rest.
    /// The effects BUS: reverb, delay and chorus, fed by the strips' sends.
    ///
    /// **Two instances, and the second one's reverb is never used.** An
    /// `Effects` can only be in one place in the graph, and these are two
    /// places: time effects belong on a send everything can reach, and a
    /// limiter belongs across the output it is protecting. The unused
    /// allocations cost about a megabyte of RAM that is never touched, which
    /// is a better trade than making every buffer in `effects.rs` optional to
    /// save it.
    effects: crate::effects::Effects,
    /// The master INSERT: high-pass, low-pass and the limiter, across
    /// everything on its way out.
    master_effects: crate::effects::Effects,
    /// The SPEAKERS' own master insert.
    ///
    /// **Two mixes leave this renderer, and they are not always the same
    /// one.** The take is the desk: every strip that is not muted is in it.
    /// The room is the desk minus an input nobody asked to hear — which is the
    /// ordinary rig, because monitoring is off at every launch and a
    /// microphone through speakers is how a room starts feeding back.
    ///
    /// The two diverge BEFORE the master insert, so the speakers need their
    /// own instance of it: a limiter is not a linear thing and its output for
    /// the room cannot be derived from its output for the file. It runs only
    /// when the two actually differ — see `room_live` — so a rig with no input
    /// pays nothing at all for it.
    room_effects: crate::effects::Effects,
    /// The room's mix, interleaved like `mix`, valid only while `room_live`.
    room: Vec<f32>,
    /// What the room must NOT be given: the input's own contribution to `mix`,
    /// already scaled by how much of it monitoring is currently withholding.
    ///
    /// Subtracted rather than separately summed, because everything upstream
    /// of the master insert is a plain sum and the difference between the two
    /// mixes is exactly this. One buffer, filled by the pass that made the
    /// samples in the first place.
    input_dry: Vec<f32>,
    /// Whether the room and the take differ this block.
    room_live: bool,
    /// How much of the input the room is hearing, slewed 0..1 by
    /// `Shared::monitor_on`.
    ///
    /// Slewed, and not a branch, because toggling monitoring is a hand on a
    /// control and a step of a whole microphone is a click in the speakers.
    /// It scales the send as well as the dry: a channel switched out of the
    /// room does not get to arrive back in it through the reverb.
    room_gain: f32,
    /// The click, one value per frame of the current chunk, and the same
    /// again as it should reach the FILE — zero on the frames the take is not
    /// meant to carry it. Two buffers rather than a flag, so the pass that
    /// writes the take does not re-derive a per-frame condition.
    click_out: Vec<f32>,
    click_taped: Vec<f32>,
    /// A user effect ACROSS the effects bus, if one is loaded.
    ///
    /// **On the bus rather than on every channel**, and that is a design
    /// decision rather than a shortcut. A reverb is a send effect: one
    /// instance that four channels feed at four different amounts is what a
    /// desk does and what the plugin expects, and per-channel inserts would be
    /// nine instances of the same reverb running at once for a machine this
    /// app is careful about.
    /// One rack per channel, master last — `Strip::index` and then `STRIPS`.
    ///
    /// **The effects bus is one of them.** It used to be a field of its own
    /// with its own hand-off and its own function; a bus is a channel with a
    /// send instead of a fader, and one mechanism that every channel uses is
    /// worth more than a special case that only the bus understands.
    racks: [Rack; STRIPS + 1],
    /// Deinterleaved in and out for it. Preallocated, like everything else the
    /// audio thread touches.
    fx_in: Vec<Vec<f32>>,
    fx_out: Vec<Vec<f32>>,
    /// The effects bus's own buffer, one block long, interleaved like `mix`.
    aux: Vec<f32>,
    fx_return_gain: f32,
    /// The renderer's OWN copy of the parameters.
    ///
    /// Refreshed from `Shared` only when the dirty flag says to, so the audio
    /// thread never blocks on a lock in the common case and never blocks at
    /// all: a `try_lock` that fails simply means next block.
    effect_params: crate::effects::Params,
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
    /// The live input, when one is open. Drained EVERY block whether or not
    /// anybody is listening — see `mix_monitor`.
    monitor: Option<rtrb::Consumer<f32>>,
    /// How many channels that ring carries per frame.
    monitor_channels: usize,
    /// Slewed, so switching monitoring on is a fade rather than a bang.
    monitor_gain: [f32; INPUTS],
    /// Channels per input strip, in stream order. Zero for one not open.
    monitor_widths: [usize; INPUTS],
    /// The INPUT device's own block, in frames. What the monitor ring is
    /// allowed to hold before the surplus is stale rather than in flight.
    monitor_block: usize,
    monitor_scratch: Vec<f32>,
}

impl Renderer {
    /// Run one channel's insert rack over an INTERLEAVED stereo buffer.
    ///
    /// The racks want planar — one array per channel, which is what every VST3
    /// effect asks for — and the mix and the bus are interleaved, so this is
    /// the conversion at both ends. Slots hand their audio over planar
    /// already and skip it.
    ///
    /// Returns immediately on an empty rack, so a channel nobody has put an
    /// effect on costs one branch and folds by exactly the path it always did.
    fn run_insert_chain(&mut self, at: usize, buf_is_aux: bool, frames: usize) {
        // **The block's notes, gated by the channel's own arm bit.** A voice
        // in this rack is fed the same list every slot reads — see
        // `collect_notes`, drained once per block — unless the channel is
        // switched away from MIDI, in which case it is fed nothing and holds
        // its tails. The gate is per CHANNEL and travels as one atomic mask,
        // like mute and solo.
        let armed = self.shared.midi_off.load(Ordering::Relaxed) & (1 << at.min(31)) == 0;
        let notes: &[ivory_host::Note] = if armed { &self.notes } else { &[] };
        let controls: &[ivory_host::Control] = if armed { &self.controls } else { &[] };
        let Some(rack) = self.racks.get(at) else {
            return;
        };
        if rack.is_empty() {
            return;
        }
        let n = frames * TAP_CHANNELS;
        {
            let src = if buf_is_aux { &self.aux } else { &self.mix };
            let Some(src) = src.get(..n) else {
                return;
            };
            for c in 0..TAP_CHANNELS {
                let Some(dst) = self.fx_in.get_mut(c) else {
                    return;
                };
                for (f, d) in dst.iter_mut().take(frames).enumerate() {
                    *d = src[f * TAP_CHANNELS + c];
                }
            }
        }
        let Some(rack) = self.racks.get_mut(at) else {
            return;
        };
        run_rack(rack, &mut self.fx_in, frames, &mut self.fx_out, notes, controls);
        let dst = if buf_is_aux {
            self.aux.get_mut(..n)
        } else {
            self.mix.get_mut(..n)
        };
        let Some(dst) = dst else {
            return;
        };
        for c in 0..TAP_CHANNELS {
            let Some(src) = self.fx_in.get(c) else {
                continue;
            };
            for f in 0..frames {
                if let (Some(v), Some(s)) = (dst.get_mut(f * TAP_CHANNELS + c), src.get(f)) {
                    *v = *s;
                }
            }
        }
    }

    fn swap_plugins(&mut self) -> usize {
        // Every rack, by one protocol: whatever arrives replaces what is
        // there and the old one goes back to be dropped on the thread that
        // made it.
        let mut swapped = 0;
        for rack in &mut self.racks {
            swapped += rack.swap();
        }
        if swapped > 0 {
            self.shared.swaps.fetch_add(swapped as u64, Ordering::Relaxed);
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
                // The ledger of what is sounding, kept beside the push so
                // every path that delivers a note also records it — a stop or
                // a locate reads this to know which pitches it strands.
                if let Ok(pitch) = u8::try_from(note.pitch) {
                    if pitch < 128 {
                        if note.on {
                            self.sounding |= 1 << pitch;
                        } else {
                            self.sounding &= !(1 << pitch);
                        }
                    }
                }
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
        // **And the desk's switches, which decide whether a slot is heard at
        // all.** Read here with the gains, for the same reason and at the same
        // cost: they are published at human speed.
        let muted = self.shared.muted.load(Ordering::Relaxed);
        let soloed = self.shared.soloed.load(Ordering::Relaxed);
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

            // **The bus starts empty every block, and it has to be emptied
            // BEFORE the slots write to it.** It was cleared after them once,
            // for one commit, and the symptom was a reverb knob that did
            // nothing at all: the instruments' sends were wiped a few lines
            // after they were made, so the only thing left on the bus was the
            // click.
            if let Some(aux) = self.aux.get_mut(..n * TAP_CHANNELS) {
                aux.fill(0.0);
            }
            // **And the mix itself.** `sum_slots` used to zero it on its way
            // through; with the legacy rack gone, forgetting this line leaves
            // every block summed on top of the last one — the backing track
            // a block ago is still in the buffer, at full level, for ever.
            if let Some(mix) = self.mix.get_mut(..n * TAP_CHANNELS) {
                mix.fill(0.0);
            }
            self.mix_channels(n, muted, soloed);

            // A parameter change, if one is waiting and the lock is free this
            // block. Same shape as the DX7's pending voice, and for the same
            // reason: the audio thread waits for nothing.
            if self.shared.params_dirty.load(Ordering::Acquire) {
                if let Ok(g) = self.shared.params.try_lock() {
                    self.effect_params = *g;
                    self.shared.params_dirty.store(false, Ordering::Release);
                }
            }

            // ── the desk ───────────────────────────────────────────────────
            //
            // **The effects used to be an insert on the instrument bus** and
            // everything else joined downstream of them, which is why nothing
            // but the instrument could ever be reverberated and why the
            // limiter never saw the backing track it was supposed to be
            // protecting the output from.
            //
            // They are two things now. Reverb, delay and chorus are a BUS: a
            // strip sends a percentage of itself to them and what comes back
            // is added to the mix. The high-pass, the low-pass and the limiter
            // are an INSERT on the master, across everything on its way out.
            // Only three of the six were ever wet amounts; the other three are
            // a corner frequency, a corner frequency and a threshold, and a
            // send knob into those would not be a question anybody could
            // answer.
            //
            // The defaults are the old routing exactly — instrument sends
            // everything, nothing else sends anything — so an upgrade with the
            // mixer untouched sounds the same.
            let bpm = f64::from_bits(self.shared.bpm.load(Ordering::Relaxed));
            let level = |strip: Strip, gain: &AtomicU32| {
                if strip_is_heard(strip, muted, soloed) {
                    Shared::f32_of(gain).clamp(0.0, 8.0)
                } else {
                    0.0
                }
            };
            // Read here rather than through a closure: a closure over
            // `self.shared` borrows the whole of `self`, and the mixing below
            // needs `self` mutably.
            let send_of = |sh: &Shared, strip: Strip| {
                Shared::f32_of(&sh.send[strip.index()]).clamp(0.0, 1.0)
            };

            // **The slots did their own folding.** Each instrument is a channel
            // with its own fader, its own send and its own mute — see
            // `sum_slots`, which is where the summing and the sending are one
            // pass because they are one signal read twice.
            let coeff = self.gain_coeff;

            // The BACKING TRACK and the INPUT, each adding itself to the mix
            // and to the bus in one pass. Both were downstream of the effects
            // before and reached neither.
            //
            // **The transport first, once per chunk.** A locate or a falling
            // edge strands whatever notes are sounding — the keys did not come
            // up just because the timeline moved — so the strands are ENDED:
            // the built-in by its own switch, every hosted voice by note-offs
            // appended to this block's list, which is the same list they were
            // struck from.
            let (at, rolling, jumped) = self.run_transport(n);
            if (jumped || (self.was_rolling && !rolling)) && self.sounding != 0 {
                self.builtin.all_notes_off();
                for pitch in 0..128u8 {
                    if self.sounding & (1 << pitch) != 0
                        && self.notes.len() < MAX_EVENTS_PER_BLOCK
                    {
                        self.notes.push(ivory_host::Note {
                            offset: 0,
                            pitch: i16::from(pitch),
                            velocity: 0.0,
                            on: false,
                        });
                    }
                }
                self.sounding = 0;
            }
            self.was_rolling = rolling;
            self.mix_track(at, rolling, n, muted, soloed);
            self.mix_monitor(n, muted, soloed);
            // ── the click, and what it sends ───────────────────────────
            //
            // **Its own pass, because the bus has to be finished before the
            // master is.** The click is generated per frame, in step with the
            // beat clock, and it now has a send like every other strip — so it
            // has to reach the bus BEFORE the bus is processed, while the tap
            // and the device it also feeds are written AFTER the master is.
            // One loop cannot be on both sides of that.
            // `metro_target` was read at the top of the block with the rest of
            // the metronome's state; the strip only decides whether it is heard.
            let click_level = if strip_is_heard(Strip::Click, muted, soloed) {
                metro_target
            } else {
                0.0
            };
            let click_send = send_of(&self.shared, Strip::Click);
            let mut click_peak = 0.0f32;
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

                self.metro_gain += (click_level - self.metro_gain) * self.gain_coeff;
                let click = self.voice.next(&self.click) * self.metro_gain;
                self.click_out[i] = click;
                // The click reaches the FILE when the performance is meant to
                // carry it, or while a count-in that was asked to be in the
                // take is still sounding. `counting_in` rather than a beat
                // number, because the click is a decaying sample: the sound of
                // beat four is still going during beat five and has to be
                // recorded for as long as it lasts.
                //
                // Kept as a second BUFFER rather than a flag, so the pass that
                // writes the take does not have to re-derive a per-frame
                // condition it can no longer see.
                self.click_taped[i] = if in_take || (count_in_recorded && counting_in) {
                    click
                } else {
                    0.0
                };
                click_peak = click_peak.max(click.abs());
                let at = i * TAP_CHANNELS;
                for c in 0..TAP_CHANNELS {
                    if let Some(v) = self.aux.get_mut(at + c) {
                        *v += click * click_send;
                    }
                }
            }
            // **The click's rack, run here in strip order** — the same
            // window every other channel's runs in — over the buffer that
            // both destinations read. The SUMMING stays post-master (see the
            // fold below), which is the cue-bus rule: the click is processed
            // with the channels and added after the ceiling, so it can never
            // push a take into the limiter and never be ducked by one.
            // Bit-identical for an empty rack, which is every desk until
            // somebody fills a bay.
            if !self.racks[Strip::Click.index()].is_empty() {
                for c in 0..TAP_CHANNELS {
                    if let Some(dst) = self.fx_in.get_mut(c) {
                        for (f, d) in dst.iter_mut().take(n).enumerate() {
                            *d = self.click_out[f];
                        }
                    }
                }
                {
                    let armed = self.shared.midi_off.load(Ordering::Relaxed)
                        & (1 << Strip::Click.index().min(31))
                        == 0;
                    let notes: &[ivory_host::Note] = if armed { &self.notes } else { &[] };
                    let controls: &[ivory_host::Control] =
                        if armed { &self.controls } else { &[] };
                    run_rack(
                        &mut self.racks[Strip::Click.index()],
                        &mut self.fx_in,
                        n,
                        &mut self.fx_out,
                        notes,
                        controls,
                    );
                }
                // Back into BOTH buffers, and the take's keeps its gate: a
                // frame the take was not carrying stays absent however the
                // rack coloured it.
                click_peak = 0.0;
                for i in 0..n {
                    let wet = self.fx_in.first().map_or(0.0, |b| b[i]);
                    let gate = if self.click_taped[i] != 0.0 { 1.0 } else { 0.0 };
                    self.click_out[i] = wet;
                    self.click_taped[i] = wet * gate;
                    click_peak = click_peak.max(wet.abs());
                }
            }
            // The click is one voice on both sides.
            self.shared.note_strip_peak(Strip::Click, [click_peak; 2]);

            // ── the bus, and then the master ───────────────────────────────
            let sends = crate::effects::Sends {
                reverb: Shared::f32_of(&self.shared.reverb_mix),
                delay: Shared::f32_of(&self.shared.delay_mix),
                chorus: Shared::f32_of(&self.shared.chorus_mix),
                // Not on the bus. See the comment above the desk — and note
                // that the limiter's "off" is the TOP of its travel, so this
                // is 1.0 rather than the 0.0 the others use.
                hpf: 0.0,
                lpf: 0.0,
                limiter: 1.0,
            };
            if let Some(aux) = self.aux.get_mut(..n * TAP_CHANNELS) {
                self.effects
                    .process(aux, n, TAP_CHANNELS, sends, &self.effect_params, bpm);
            }
            // **And then whatever the user put across the bus.** After the
            // built-in three, so somebody who loads a reverb and leaves the
            // knobs alone hears their reverb rather than theirs through ours.
            self.run_insert_chain(Strip::Fx.index(), true, n);
            // What comes back, at the bus's own fader.
            let return_target = level(Strip::Fx, &self.shared.fx_return);
            let fx_peak = add_return(
                &mut self.mix,
                &self.aux,
                n,
                &mut self.fx_return_gain,
                return_target,
                coeff,
            );
            self.shared.note_strip_peak(Strip::Fx, fx_peak);

            let master_sends = crate::effects::Sends {
                reverb: 0.0,
                delay: 0.0,
                chorus: 0.0,
                hpf: Shared::f32_of(&self.shared.hpf_mix),
                lpf: Shared::f32_of(&self.shared.lpf_mix),
                limiter: Shared::f32_of(&self.shared.limiter_mix),
            };
            // **The room's mix is made here, one subtraction, while the sum
            // is still a sum.** Everything upstream of the master insert adds,
            // so the difference between the file's mix and the speakers' is
            // exactly the input that monitoring is withholding — and `mix_in`
            // wrote that down frame by frame as it made it.
            //
            // After the insert it could not be done at all: a limiter is not
            // linear, and there is no arithmetic that takes one voice back out
            // of what it decided.
            let split = self.room_live;
            if split {
                let end = n * TAP_CHANNELS;
                if let (Some(room), Some(mix), Some(dry)) = (
                    self.room.get_mut(..end),
                    self.mix.get(..end),
                    self.input_dry.get(..end),
                ) {
                    for i in 0..end {
                        room[i] = mix[i] - dry[i];
                    }
                }
            }
            // **The master's own rack, before the master's built-in insert.**
            // The limiter stays last on the way out, which is what a limiter
            // is for: an effect after it would undo the ceiling it just made.
            self.run_insert_chain(STRIPS, false, n);
            let master = Shared::f32_of(&self.shared.master_gain);
            if let Some(mix) = self.mix.get_mut(..n * TAP_CHANNELS) {
                self.master_effects.process(
                    mix,
                    n,
                    TAP_CHANNELS,
                    master_sends,
                    &self.effect_params,
                    bpm,
                );
                // **The master, last on the bus and after the limiter.** A
                // master that fed the limiter would be a second drive control;
                // this one is what leaves, which is what a master fader is.
                if (master - 1.0).abs() > 1.0e-6 {
                    for v in mix.iter_mut() {
                        *v *= master;
                    }
                }
            }
            // The speakers' own insert, the same settings, its own state —
            // and only while the two mixes differ. Idle it costs nothing; a
            // rig with no input device never reaches it at all.
            if split {
                if let Some(room) = self.room.get_mut(..n * TAP_CHANNELS) {
                    self.room_effects.process(
                        room,
                        n,
                        TAP_CHANNELS,
                        master_sends,
                        &self.effect_params,
                        bpm,
                    );
                    if (master - 1.0).abs() > 1.0e-6 {
                        for v in room.iter_mut() {
                            *v *= master;
                        }
                    }
                }
            }
            // How hard the limiter worked, for the meter beside it. Kept as
            // the WORST since the UI last looked: it asks sixty times a second
            // and the moment worth showing is a few samples long.
            let gr = self.master_effects.gain_reduction_db();
            if gr > 0.0 {
                self.shared.gr_db.fetch_max(gr.to_bits(), Ordering::Relaxed);
            }

            // ── out, to the take and to the speakers ───────────────────────
            //
            // Two mixes when monitoring is withholding an input, one when it
            // is not. The click has always been two already, at two levels —
            // `click_taped` and `click_out` — for the same reason: what is
            // worth hearing and what is worth keeping are not always the same
            // signal.
            for i in 0..n {
                let frame_index = done + i;
                let at2 = i * TAP_CHANNELS;
                let take = &self.mix[at2..at2 + TAP_CHANNELS];

                map_frame(take, &mut self.frame[..TAP_CHANNELS]);
                let tap_at = tap_frames * TAP_CHANNELS;
                for c in 0..TAP_CHANNELS {
                    let s = self.frame[c] + self.click_taped[i];
                    self.tap_scratch[tap_at + c] = s;
                }
                tap_frames += 1;

                let src = if split {
                    &self.room[at2..at2 + TAP_CHANNELS]
                } else {
                    &self.mix[at2..at2 + TAP_CHANNELS]
                };
                let at = frame_index * dev_ch;
                map_frame(src, &mut self.frame[..dev_ch]);
                for c in 0..dev_ch {
                    let s = self.frame[c] + self.click_out[i];
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
    /// Add the live input to the bus.
    ///
    /// **The ring is drained every block whether or not anybody is
    /// listening.** Left alone it would fill within a second of the device
    /// opening and stay full, so switching monitoring on would play a second of
    /// stale audio before catching up — and the input's callback would be
    /// pushing into a ring that never had room, which is a drop counter ticking
    /// for no reason. Drained-and-discarded is the same discipline the take's
    /// instrument ring already follows between takes.
    ///
    /// **It reaches the take, and that is the point.** This adds the input to
    /// `self.mix`, and `self.mix` is what the tap carries — so the microphone
    /// arrives in the file with its fader, its send and the master's limiter
    /// already on it, which is what a desk is. The owner's rule, in one line:
    /// if the effects were audible while it was recorded, they are in the
    /// take.
    ///
    /// It reaches the SPEAKERS only when somebody asked to hear it, and the
    /// two used to be the same switch. See the gate below.
    fn mix_monitor(&mut self, frames: usize, muted: u32, soloed: u32) {
        // Cleared first, so every path out of here that adds nothing to the
        // mix also leaves the room equal to the take.
        self.room_live = false;
        // A ring arriving, or being taken away. `try_lock`, never `lock`.
        if let Ok(mut pending) = self.shared.pending_monitor.try_lock() {
            if let Some(next) = pending.take() {
                match next {
                    Some((ring, channels, widths, block)) => {
                        self.monitor = Some(ring);
                        self.monitor_channels = usize::from(channels).max(1);
                        self.monitor_widths = widths.map(usize::from);
                        self.monitor_block = block.max(1) as usize;
                    }
                    None => {
                        self.monitor = None;
                        self.monitor_channels = 0;
                        self.monitor_widths = [0; INPUTS];
                        self.monitor_block = 0;
                    }
                }
                // A new device fades in from silence rather than arriving at
                // whatever the last one was playing at.
                self.monitor_gain = [0.0; INPUTS];
            }
        }
        let Some(ring) = self.monitor.as_mut() else {
            return;
        };
        let ch_in = self.monitor_channels.max(1);
        let want = frames * ch_in;
        // **What is heard has to be what is HAPPENING, and it was not.**
        //
        // The consumer takes at most one output block per output callback, so
        // whatever built up in the ring stayed there for the rest of the
        // session — a one-way ratchet. And it always built up: the input
        // stream opens when the band does, the OUTPUT stream starts separately
        // and can be seconds behind it while a plugin loads, and every input
        // callback in between pushed a block nobody was draining. The ring is
        // 120 ms deep, so monitoring could sit a tenth of a second behind the
        // room for ever, at the smallest buffer the app offers, with nothing
        // to point at. The owner heard it against REAPER at the same rate and
        // buffer and was right.
        //
        // So the surplus is DROPPED rather than played. What is legitimately
        // in flight is one input block — the one the device is filling now —
        // plus one output block, the one this callback is about to take.
        // Anything beyond that is history.
        //
        // WHOLE FRAMES only. A partial frame would rotate the channels for the
        // rest of the session, which is the rule the capture ring already
        // insists on and for the same reason.
        //
        // This does not close the whole gap and cannot: two AudioUnits means
        // an output callback can only ever see input that arrived before it
        // started, which is one buffer a DUPLEX device does not pay.
        let in_flight = want + self.monitor_block * ch_in;
        let backlog = ring.slots();
        self.shared
            .monitor_backlog
            .store((backlog / ch_in) as u32, Ordering::Relaxed);
        if backlog > in_flight {
            let stale = ((backlog - in_flight) / ch_in) * ch_in;
            if stale > 0 {
                if let Ok(chunk) = ring.read_chunk(stale) {
                    chunk.commit_all();
                }
                self.shared
                    .monitor_slip
                    .fetch_add((stale / ch_in) as u64, Ordering::Relaxed);
            }
        }
        let got = ring.slots().min(want);
        // Read into scratch first: the ring's chunk borrows it, and the mix
        // below needs `self.mix` mutably at the same time.
        let take = got.min(self.monitor_scratch.len());
        let scratch = &mut self.monitor_scratch[..take];
        let got = match ring.read_chunk(scratch.len()) {
            Ok(chunk) => {
                let (a, b) = chunk.as_slices();
                scratch[..a.len()].copy_from_slice(a);
                scratch[a.len()..a.len() + b.len()].copy_from_slice(b);
                let n = a.len() + b.len();
                chunk.commit_all();
                n
            }
            Err(_) => 0,
        };

        // **Mute decides the take. Monitoring decides only the room.**
        //
        // A strip is on the bus whenever it is not muted, so the file gets what
        // the desk says it gets — and `monitor_on` narrows to the one question
        // it can answer without lying: whether the SPEAKERS get it too. It used
        // to answer both, and the take was the casualty: a microphone monitored
        // through the interface's own hardware — the ordinary way anybody
        // records one, because it is the only way with no latency — is not
        // monitored here, and produced a take with no microphone in it.
        //
        // Feedback is why the room half exists at all and why it is still off
        // at every launch. See `IvoryApp::input_monitor`.
        //
        // **One switch for every input, and one fader each.** Monitoring is a
        // control-room question and the answer is the same for the whole room;
        // how loud each source is, and whether it is in the take at all, is a
        // question per channel and has a strip per channel.
        let room_target = if self.shared.monitor_on.load(Ordering::Relaxed) {
            1.0
        } else {
            0.0
        };
        let frames_got = got / ch_in;
        let n = frames.min(frames_got);
        if let Some(dry) = self.input_dry.get_mut(..frames * TAP_CHANNELS) {
            dry.fill(0.0);
        }
        // The room's slew is the room's, not any one channel's: it moves once
        // per frame however many inputs are open, or four inputs would each
        // drag it a quarter of the way and the fade would depend on how many
        // microphones happened to be plugged in.
        let room_from = self.room_gain;
        let mut room_to = room_from;
        let mut withheld = 0.0f32;
        // **Every input of the interface, at its own offset in the block.**
        // `Picks` laid them end to end when the stream was opened — a mono
        // input then a stereo pair is one channel then two — and this is the
        // other end of that: which columns of the interleaved frame belong to
        // which strip.
        let mut offset = 0usize;
        for input in 0..INPUTS {
            let width = self.monitor_widths[input].min(ch_in.saturating_sub(offset));
            if width == 0 {
                continue;
            }
            let at_ch = offset;
            offset += width;
            // **Which CHANNEL this pick feeds**, mapped by the host: the k-th
            // AUDIO channel takes pick k. An unbound pick still drains — the
            // ring has to keep moving — but is nobody's audio.
            let bound = self.shared.pick_strip[input].load(Ordering::Relaxed);
            let Some(strip) = usize::try_from(bound).ok().filter(|b| *b < CHANNELS).map(Strip::Channel) else {
                self.monitor_gain[input] = 0.0;
                continue;
            };
            let heard = strip_is_heard(strip, muted, soloed);
            let target = if heard {
                Shared::f32_of(&self.shared.channel_gains[strip.index()]).clamp(0.0, 8.0)
            } else {
                0.0
            };
            // Silent and already faded out: the drain above was the whole job
            // for this one. The others still get their turn.
            if !heard && self.monitor_gain[input] <= 1.0e-6 {
                self.monitor_gain[input] = 0.0;
                continue;
            }
            let send = Shared::f32_of(&self.shared.send[strip.index()]).clamp(0.0, 1.0);
            let mut peak = [0.0f32; 2];
            // **This strip's own audio first, then its rack, then the fold.**
            //
            // It used to go straight into the mix a sample at a time, which is
            // cheaper and leaves nowhere for an insert to stand. The values
            // are identical — this is the same arithmetic through a buffer —
            // and what it buys is a channel that can carry an amp sim like
            // every other channel can.
            //
            // A mono input goes to both sides; a stereo one keeps its sides.
            // Anything wider is folded down by taking the first two, which is
            // what a monitor is for — hearing that something is arriving, not
            // auditioning a surround mix.
            for i in 0..n {
                self.monitor_gain[input] += (target - self.monitor_gain[input]) * self.gain_coeff;
                for c in 0..TAP_CHANNELS {
                    let lane = at_ch + c.min(width - 1);
                    let src = scratch[i * ch_in + lane] * self.monitor_gain[input];
                    if let Some(dst) = self.fx_in.get_mut(c) {
                        dst[i] = src;
                    }
                }
            }
            {
                // An input channel's rack can hold a voice too — the desk is
                // about to stop distinguishing — and the gate is the same one.
                let armed =
                    self.shared.midi_off.load(Ordering::Relaxed) & (1 << strip.index().min(31)) == 0;
                let notes: &[ivory_host::Note] = if armed { &self.notes } else { &[] };
                let controls: &[ivory_host::Control] = if armed { &self.controls } else { &[] };
                run_rack(
                    &mut self.racks[strip.index()],
                    &mut self.fx_in,
                    n,
                    &mut self.fx_out,
                    notes,
                    controls,
                );
            }
            let mut room = room_from;
            for i in 0..n {
                // The room's own slew, advanced here rather than above: it
                // moves once per frame however many inputs are open, and
                // recomputing it in this pass is exact because both passes
                // walk the same frames in the same order.
                room += (room_target - room) * self.gain_coeff;
                // How much of this frame the room is NOT being given.
                let keep_out = 1.0 - room;
                withheld = withheld.max(keep_out);
                let at = i * TAP_CHANNELS;
                for c in 0..TAP_CHANNELS {
                    let src = self.fx_in.get(c).map_or(0.0, |b| b[i]);
                    peak[c.min(1)] = peak[c.min(1)].max(src.abs());
                    if let Some(v) = self.mix.get_mut(at + c) {
                        *v += src;
                    }
                    // The room's subtrahend, ADDED to rather than written:
                    // four inputs share one buffer and the last one to run
                    // would otherwise be the only one the speakers were spared.
                    if let Some(d) = self.input_dry.get_mut(at + c) {
                        *d += src * keep_out;
                    }
                    // Post-fader, like every other send here, and closed with
                    // the room: a channel nobody can hear must not arrive back
                    // in the speakers through the reverb.
                    if let Some(a) = self.aux.get_mut(at + c) {
                        *a += src * send * room;
                    }
                }
            }
            room_to = room;
            self.shared.note_strip_peak(strip, peak);
        }
        self.room_gain = room_to;
        // A hundredth of a decibel of an input still withheld is still two
        // mixes. The threshold is what stops a fully-monitored rig from
        // running a second limiter forever over a rounding error.
        self.room_live = withheld > 1.0e-4;
    }

    /// The whole transport: where this chunk starts, and whether it moves.
    ///
    /// **The locate is consumed BEFORE the level is read, in one call**, so a
    /// seek published before a stop can never be honoured after it — the
    /// Acquire pairs with the host's Release, and a callback that sees the new
    /// generation is guaranteed to see every store that preceded it. Per
    /// CHUNK, not per callback: `mix_track` runs inside the chunk loop, and
    /// advancing once per callback would hand every chunk after the first the
    /// same clip samples.
    ///
    /// Returns `(at, rolling, jumped)`. A jump — or the level falling — is what
    /// ends the notes the move strands; see the flush at the call site.
    fn run_transport(&mut self, frames: usize) -> (u64, bool, bool) {
        let req = self.shared.transport_req.load(Ordering::Acquire);
        let jumped = req != self.seen_req;
        if jumped {
            self.seen_req = req;
            self.pos = self.shared.transport_at.load(Ordering::Relaxed);
            self.shared.transport_ack.store(req, Ordering::Release);
        }
        let rolling = self.shared.rolling.load(Ordering::Relaxed);
        let at = self.pos;
        if rolling {
            self.pos += frames as u64;
        }
        self.shared.transport_pos.store(self.pos, Ordering::Relaxed);
        (at, rolling, jumped)
    }

    /// Add the backing track to the bus, if one is loaded and the transport is
    /// rolling.
    ///
    /// The clip is stereo at the device's rate — `decode` guarantees both — so
    /// there is no resampling and no channel mapping here, which is the whole
    /// reason it is guaranteed there.
    fn mix_track(&mut self, at: u64, rolling: bool, frames: usize, muted: u32, soloed: u32) {
        // A clip arriving — and the displaced one going BACK, by the racks'
        // own protocol: no room to return it, no swap this block. Dropping it
        // here would free a hundred megabytes inside the callback.
        if self.track_retiring.slots() > 0 {
            if let Ok(next) = self.track_incoming.pop() {
                let old = std::mem::replace(&mut self.track, next);
                let _ = self.track_retiring.push(old);
            }
        }
        // **Stopped means silent, exactly as it always has.** Without this the
        // parked position is re-read every callback and the clip buzzes one
        // block of itself under a readout that says nothing is moving.
        if !rolling {
            return;
        }
        let Some(placed) = self.track.clone() else {
            return;
        };
        let clip = &placed.clip;
        let total = clip.frames() as u64;
        let target = if strip_is_heard(Strip::Track, muted, soloed) {
            Shared::f32_of(&self.shared.track_gain).clamp(0.0, 8.0)
        } else {
            0.0
        };
        let send = Shared::f32_of(&self.shared.send[Strip::Track.index()]).clamp(0.0, 1.0);
        let coeff = self.gain_coeff;
        // Both at once, and they are different fields — which is the only
        // reason the loop below can write to the mix and the bus together.
        let (Some(mix), Some(aux)) = (
            self.mix.get_mut(..frames * TAP_CHANNELS),
            self.aux.get_mut(..frames * TAP_CHANNELS),
        ) else {
            return;
        };
        let mut peak = [0.0f32; 2];
        // **The clip at its fader, then its rack, then the fold.** Same shape
        // as every other channel: the values are what they always were, and
        // what the buffer buys is somewhere for an insert to stand.
        for f in 0..frames {
            self.track_gain += (target - self.track_gain) * coeff;
            // Project frame -> clip frame. The u64 compare comes BEFORE any
            // cast, because `p` grows without bound; before the clip and past
            // its end are both silence, and both are ordinary.
            let p = at + f as u64;
            let (l, r) = match p.checked_sub(placed.start) {
                Some(k) if k < total => {
                    let i = (k as usize) * 2;
                    // Bounds-checked rather than trusted: `k < total` is the
                    // clamp, and this is the line that would panic on the
                    // audio thread if that ever stopped being true.
                    match (clip.samples.get(i), clip.samples.get(i + 1)) {
                        (Some(l), Some(r)) => (l * self.track_gain, r * self.track_gain),
                        _ => (0.0, 0.0),
                    }
                }
                _ => (0.0, 0.0),
            };
            if let Some(dst) = self.fx_in.get_mut(0) {
                dst[f] = l;
            }
            if let Some(dst) = self.fx_in.get_mut(1) {
                dst[f] = r;
            }
        }
        {
            let armed = self.shared.midi_off.load(Ordering::Relaxed)
                & (1 << Strip::Track.index().min(31))
                == 0;
            let notes: &[ivory_host::Note] = if armed { &self.notes } else { &[] };
            let controls: &[ivory_host::Control] = if armed { &self.controls } else { &[] };
            run_rack(
                &mut self.racks[Strip::Track.index()],
                &mut self.fx_in,
                frames,
                &mut self.fx_out,
                notes,
                controls,
            );
        }
        for f in 0..frames {
            let l = self.fx_in.first().map_or(0.0, |b| b[f]);
            let r = self.fx_in.get(1).map_or(0.0, |b| b[f]);
            peak[0] = peak[0].max(l.abs());
            peak[1] = peak[1].max(r.abs());
            mix[f * TAP_CHANNELS] += l;
            mix[f * TAP_CHANNELS + 1] += r;
            // Post-fader, like every other send here. A backing track arrives
            // finished and usually wants none of the room the piano is in,
            // which is why this defaults to zero.
            aux[f * TAP_CHANNELS] += l * send;
            aux[f * TAP_CHANNELS + 1] += r * send;
        }
        self.shared.note_strip_peak(Strip::Track, peak);
    }

    /// The eight general channels: the built-in, the bay voices, and their
    /// effect bays — everything except AUDIO channels, whose racks run where
    /// their input arrives (see `mix_monitor`).
    ///
    /// Replaces `render_builtin`, and generalises exactly what that function
    /// already did for one synth: render into a private buffer, run the
    /// channel's rack, fold at the channel's fader and send, note the peak.
    fn mix_channels(&mut self, frames: usize, muted: u32, soloed: u32) {
        // A patch change, if one is waiting and the lock is free this block.
        if let Ok(mut g) = self.shared.pending_voice.try_lock() {
            if let Some(v) = g.take() {
                self.builtin.set_voice(v);
            }
        }
        let wanted = self.shared.builtin_strip.load(Ordering::Relaxed);
        let builtin_at = (wanted >= 0).then_some(wanted as usize);
        let midi_off = self.shared.midi_off.load(Ordering::Relaxed);
        // The built-in's notes, fed once — not per channel, it is one synth.
        // Only while its channel is MIDI and armed: a channel cycled to AUDIO
        // has its bit set, and the keys must come up rather than hold.
        let builtin_armed =
            builtin_at.is_some_and(|ch| midi_off & (1 << ch.min(31)) == 0);
        if builtin_armed {
            for n in &self.notes {
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
            // uses, and what a half-pedalled continuous controller means.
            for c in &self.controls {
                if c.controller == 64 {
                    self.builtin.set_pedal(c.value >= 64);
                }
            }
        } else if self.builtin.active() {
            self.builtin.all_notes_off();
        }

        let coeff = self.gain_coeff;
        for ch in 0..CHANNELS {
            let is_builtin = builtin_at == Some(ch);
            let rack_live = !self.racks[ch].is_empty();
            // An AUDIO channel's bit is set and its rack runs in
            // `mix_monitor`, over its input — running it here too would fold
            // its effect tails twice.
            let armed = midi_off & (1 << ch.min(31)) == 0;
            if !armed || (!is_builtin && !rack_live) {
                // **The gain is advanced whether or not anything renders.** A
                // fader moved during a rest has to have arrived by the next
                // note, or it comes in at the old level and slides.
                let target = if strip_is_heard(Strip::Channel(ch), muted, soloed) {
                    Shared::f32_of(&self.shared.channel_gains[ch]).clamp(0.0, 8.0)
                } else {
                    0.0
                };
                for _ in 0..frames {
                    self.channel_gain[ch] += (target - self.channel_gain[ch]) * coeff;
                }
                continue;
            }
            // The channel's own signal, planar for the rack.
            if is_builtin && self.builtin.active() {
                let want = frames * TAP_CHANNELS;
                let Some(scratch) = self.builtin_scratch.get_mut(..want) else {
                    continue;
                };
                scratch.fill(0.0);
                self.builtin.render(scratch, frames, TAP_CHANNELS);
                for c in 0..TAP_CHANNELS {
                    let Some(dst) = self.fx_in.get_mut(c) else { continue };
                    for f in 0..frames {
                        dst[f] = scratch[f * TAP_CHANNELS + c];
                    }
                }
            } else {
                // Silence in: a bay-1 voice REPLACES it, an effect transforms
                // it, and a rack of effects over silence is silence.
                for c in 0..TAP_CHANNELS {
                    if let Some(dst) = self.fx_in.get_mut(c) {
                        dst[..frames].fill(0.0);
                    }
                }
                if !rack_live {
                    continue;
                }
            }
            if rack_live {
                run_rack(
                    &mut self.racks[ch],
                    &mut self.fx_in,
                    frames,
                    &mut self.fx_out,
                    &self.notes,
                    &self.controls,
                );
            }
            // The fold: fader, send, peak — the channel's own, exactly as
            // every other strip does it.
            let target = if strip_is_heard(Strip::Channel(ch), muted, soloed) {
                Shared::f32_of(&self.shared.channel_gains[ch]).clamp(0.0, 8.0)
            } else {
                0.0
            };
            let send = f32::from_bits(
                self.shared.send[Strip::Channel(ch).index()].load(Ordering::Relaxed),
            )
            .clamp(0.0, 1.0);
            let want = frames * TAP_CHANNELS;
            let (Some(mix), Some(aux)) = (
                self.mix.get_mut(..want),
                self.aux.get_mut(..want),
            ) else {
                continue;
            };
            let mut peak = [0.0f32; 2];
            for f in 0..frames {
                self.channel_gain[ch] += (target - self.channel_gain[ch]) * coeff;
                let g = self.channel_gain[ch];
                let l = self.fx_in.first().map_or(0.0, |b| b[f]) * g;
                let r = self.fx_in.get(1).map_or(0.0, |b| b[f]) * g;
                peak[0] = peak[0].max(l.abs());
                peak[1] = peak[1].max(r.abs());
                mix[f * TAP_CHANNELS] += l;
                mix[f * TAP_CHANNELS + 1] += r;
                aux[f * TAP_CHANNELS] += l * send;
                aux[f * TAP_CHANNELS + 1] += r * send;
            }
            self.shared.note_strip_peak(Strip::Channel(ch), peak);
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

    /// The UI thread's ends of every rack's hand-offs. See `Rack`.
    insert_handoff: [[Handoff; INSERTS]; STRIPS + 1],
    /// The clip's two rings, in the plugin handoff's exact shape and for its
    /// exact reason: the audio thread must never free a hundred megabytes of
    /// samples, so the displaced clip comes BACK here and is dropped by the
    /// thread that sent its replacement.
    track_to_audio: Producer<Option<Placed>>,
    track_from_audio: Consumer<Option<Placed>>,
    /// What is in each of them, for the desk to name.
    inserts_loaded: [[Option<Loaded>; INSERTS]; STRIPS + 1],

    tap: Option<RecorderTap>,
    fault: Arc<Mutex<Option<String>>>,

    /// The open editor window for each loaded insert, if the user has asked
    /// for one. **An insert is a plugin with a window like any other**, and
    /// until this existed there was no way to reach one: a reverb loaded into
    /// a channel came up, worked, and could not be adjusted — every control
    /// it has is inside a window nothing opened.
    ///
    /// The engine owns them because the engine owns the plugins, and the two
    /// have a lifetime rule: an editor must never outlive the instance whose
    /// controller built its view. Declared **before** `insert_editor_handles`
    /// so that even a plain `drop(engine)` releases every window and its
    /// `IPlugView` before the controller references that made them.
    insert_editors: [[Option<ivory_host::Editor>; INSERTS]; STRIPS + 1],
    /// Each loaded insert's edit controller, taken in [`Engine::load_insert`]
    /// **before** the instance leaves for the audio thread — the one moment
    /// there is an `&Instance` on this thread. See [`Engine::open_editor`].
    insert_editor_handles: [[Option<ivory_host::EditorHandle>; INSERTS]; STRIPS + 1],
    /// Each loaded insert's processor reference, taken beside the editor's and
    /// for the same reason: after the handoff there is no `&Instance` to ask.
    /// This is what lets a bay's settings survive a relaunch — no insert has
    /// ever had one before, so every effect's knobs and every bay instrument's
    /// preset died with the process.
    insert_state_handles: [[Option<ivory_host::StateHandle>; INSERTS]; STRIPS + 1],
    /// Why a bay's saved state was not restored, if it was not.
    ///
    /// A stale or hand-edited blob must not stop the plugin loading — the
    /// user wants their piano more than they want their preset — so the load
    /// continues with the plugin's defaults and the reason lands here, where
    /// [`Engine::insert_state_error`] can put it in front of someone.
    /// Silently ignoring it is how "it keeps forgetting my piano" becomes
    /// unreportable.
    insert_state_errors: [[Option<String>; INSERTS]; STRIPS + 1],

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

    /// As [`Engine::start`], with a buffer size and a rate the user chose.
    pub fn start_sized(
        out_device: Option<&str>,
        timebase: Timebase,
        buffer_frames: Option<u32>,
        sample_rate: Option<u32>,
    ) -> Result<Self, String> {
        Self::start_inner(out_device, timebase, buffer_frames, sample_rate)
    }

    /// As [`Engine::start`], sharing the recorder's timebase.
    ///
    /// Worth doing: it is what makes [`Engine::count_in_downbeat_ns`] comparable
    /// with the MIDI tap's stamps and with the take's `T0`, so a take can start
    /// on the downbeat the player actually heard rather than on the UI frame
    /// that noticed.
    pub fn start_with(out_device: Option<&str>, timebase: Timebase) -> Result<Self, String> {
        Self::start_inner(out_device, timebase, None, None)
    }

    fn start_inner(
        out_device: Option<&str>,
        timebase: Timebase,
        asked_buffer: Option<u32>,
        asked_rate: Option<u32>,
    ) -> Result<Self, String> {
        // The system the user chose, not whatever the platform hands out. One
        // process, one driver stack: `ivory_record::audio` owns the choice
        // because that is where the input opens, and the two sides opening
        // through different hosts is a configuration nobody asked for and
        // nothing would report.
        let host = ivory_record::audio::host();
        let device = resolve_output(&host, out_device)?;
        let name = device
            .name()
            .unwrap_or_else(|_| "unnamed output device".to_string());
        let supported = device
            .default_output_config()
            .map_err(|e| format!("{name}: no default output config ({e})"))?;

        let channels = supported.channels();
        // **The rate is asked for, not imposed.** Setup offers only what the
        // INPUT device supports, and this is the output — a different device
        // with its own ranges — so the request is honoured when this device can
        // also do it and dropped when it cannot. Dropped and not refused:
        // failing to open the engine because the monitor path disagrees about a
        // rate would take the whole app's sound away over a preference.
        //
        // Worth honouring at all because `AudioStatus::rates_disagree` is a
        // warning with no fix otherwise: the writer drains the instrument's
        // ring at the input's rate while the engine fills it at the output's,
        // and the take drifts.
        let supported = match asked_rate {
            // Narrowed through a RANGE rather than rebuilt: `with_sample_rate`
            // belongs to the range, and taking it from one the device actually
            // reported is what makes the result a configuration the device has
            // agreed to rather than one assembled from three numbers that each
            // looked right.
            Some(want) => device
                .supported_output_configs()
                .ok()
                .and_then(|mut cs| {
                    cs.find(|c| {
                        c.channels() == channels
                            && c.sample_format() == supported.sample_format()
                            && c.min_sample_rate().0 <= want
                            && want <= c.max_sample_rate().0
                    })
                })
                .map(|c| c.with_sample_rate(cpal::SampleRate(want)))
                .unwrap_or(supported),
            None => supported,
        };
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

        // **A rack per channel, master last, three hand-off pairs each.**
        // Thirty-nine small rings rather than one shared pair, because the
        // ring an instance travels on is what says where it belongs, and a
        // pair carrying an index would be the audio thread trusting a number
        // a UI thread wrote. See `PluginBox`. Two and two per bay because at
        // most one handoff per bay is ever in flight — `hand_off_insert`
        // waits for the return before it starts another — and the spare
        // element is what lets `Rack::swap` check for room before it commits.
        let mut rack_ends: Vec<[Handoff; INSERTS]> = Vec::with_capacity(STRIPS + 1);
        let racks: [Rack; STRIPS + 1] = std::array::from_fn(|_| {
            let mut ends: Vec<Handoff> = Vec::with_capacity(INSERTS);
            let mut incoming = Vec::with_capacity(INSERTS);
            let mut retiring = Vec::with_capacity(INSERTS);
            for _ in 0..INSERTS {
                let (to_audio, inc) = RingBuffer::<PluginBox>::new(2);
                let (ret, from_audio) = RingBuffer::<PluginBox>::new(2);
                ends.push(Handoff {
                    to_audio,
                    from_audio,
                });
                incoming.push(inc);
                retiring.push(ret);
            }
            let mut ends = ends.into_iter();
            rack_ends.push(std::array::from_fn(|_| {
                ends.next().expect("one Handoff per insert")
            }));
            let mut inc = incoming.into_iter();
            let mut ret = retiring.into_iter();
            Rack {
                slots: std::array::from_fn(|_| PluginBox(None)),
                incoming: std::array::from_fn(|_| inc.next().expect("one per insert")),
                retiring: std::array::from_fn(|_| ret.next().expect("one per insert")),
            }
        });
        let mut rack_ends = rack_ends.into_iter();
        let insert_handoff: [[Handoff; INSERTS]; STRIPS + 1] =
            std::array::from_fn(|_| rack_ends.next().expect("one set per channel"));
        // The effect across the bus used to have a pair of its own here. It
        // does not any more: the bus is a channel like the others and its
        // effect is `insert_handoff[Strip::Fx.index()]`, so what was left was
        // two rings built on every engine start with nothing on either end.

        let dev_ch = channels as usize;
        let widest = dev_ch.max(TAP_CHANNELS);
        // The clip's pair: capacity two, one in flight plus the spare that
        // lets the swap check for room, exactly as a slot's.
        let (track_to_audio, track_incoming) = RingBuffer::<Option<Placed>>::new(2);
        let (track_retiring, track_from_audio) = RingBuffer::<Option<Placed>>::new(2);
        let renderer = Renderer {
            shared: Arc::clone(&shared),
            timebase,
            rate: f64::from(rate),
            dev_channels: dev_ch,
            // Sized for one whole render block, here and never again — this is
            // the buffer that makes summing the desk allocation-free whether
            // its channels fill up now or in an hour.
            mix: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            midi: midi_rx,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            track: None,
            pos: 0,
            track_gain: 1.0,
            seen_req: 0,
            was_rolling: false,
            sounding: 0,
            track_incoming,
            track_retiring,
            builtin: crate::dx7::Dx7::new(rate as f32),
            builtin_scratch: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            channel_gain: [1.0; CHANNELS],
            effects: crate::effects::Effects::new_send(rate as f32),
            master_effects: crate::effects::Effects::new(rate as f32),
            room_effects: crate::effects::Effects::new(rate as f32),
            room: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            input_dry: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            room_live: false,
            room_gain: 0.0,
            aux: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            racks,
            fx_in: vec![vec![0.0; MAX_BLOCK as usize]; TAP_CHANNELS],
            fx_out: vec![vec![0.0; MAX_BLOCK as usize]; TAP_CHANNELS],
            click_out: vec![0.0; MAX_BLOCK as usize],
            click_taped: vec![0.0; MAX_BLOCK as usize],
            fx_return_gain: 1.0,
            effect_params: crate::effects::Params::default(),
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
            monitor: None,
            monitor_channels: 0,
            monitor_gain: [0.0; INPUTS],
            monitor_widths: [0; INPUTS],
            monitor_block: 0,
            monitor_scratch: vec![0.0; widest * 4096],
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
            insert_handoff,
            track_to_audio,
            track_from_audio,
            inserts_loaded: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            tap: Some(RecorderTap {
                rx: tap_rx,
                channels: TAP_CHANNELS,
                sample_rate: rate,
                dropped: shared,
            }),
            fault,
            insert_editors: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            insert_editor_handles: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            insert_state_handles: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            insert_state_errors: std::array::from_fn(|_| std::array::from_fn(|_| None)),
            hold: Cell::new((0.0, 0.0)),
            hold_at: Cell::new(Instant::now()),
        })
    }


    // ── the instruments ─────────────────────────────────────────────────────

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
    /// Put a user effect across the effects bus, or take it away with `None`.
    ///
    /// **No warm-up, unlike an instrument.** `ready::warm_up` plays a note and
    /// waits to hear something, which is the right gate for a thing that makes
    /// sound and a meaningless one for a thing that changes it: an effect
    /// handed silence correctly produces silence, so the gate would fail every
    /// reverb ever written.
    ///
    /// Blocking, like every other load here — `Module::open` runs somebody
    /// else's initialiser — so the host calls it after a frame, never inside
    /// one.
    pub fn load_insert(
        &mut self,
        strip: usize,
        slot: usize,
        bundle: Option<&Path>,
        state: Option<&[u8]>,
    ) -> Result<Option<Loaded>, String> {
        if strip > STRIPS || slot >= INSERTS {
            return Ok(None);
        }
        let Some(bundle) = bundle else {
            // Window first, then the controller and state references, then the
            // instance.
            self.close_insert_editor(strip, slot);
            if let Some(h) = self
                .insert_editor_handles
                .get_mut(strip)
                .and_then(|r| r.get_mut(slot))
            {
                *h = None;
            }
            if let Some(h) = self
                .insert_state_handles
                .get_mut(strip)
                .and_then(|r| r.get_mut(slot))
            {
                *h = None;
            }
            self.hand_off_insert(strip, slot, PluginBox(None));
            self.inserts_loaded[strip][slot] = None;
            if let Some(e) = self
                .insert_state_errors
                .get_mut(strip)
                .and_then(|r| r.get_mut(slot))
            {
                *e = None;
            }
            return Ok(None);
        };
        let module = Module::open(bundle)?;
        let classes = module.audio_modules();
        let class = classes
            .first()
            .ok_or_else(|| format!("{} has no Audio Module Class", bundle.display()))?
            .clone();
        // **Any plugin loads in any bay, and there is no refusal left to
        // word.** The rack used to refuse instruments the way slots refuse
        // effects; the desk stopped distinguishing, so a bay decides WHAT the
        // plugin is instead of whether it may exist: a voice is fed the
        // block's notes and replaces the channel, an effect is fed the channel
        // and transforms it. See `Hosted::voice` and `run_rack`.
        let voice = class.kind() == ivory_host::scan::Kind::Instrument;
        let setup = Setup {
            sample_rate: f64::from(self.output.sample_rate),
            max_block: MAX_BLOCK,
        };
        let mut inst = Instance::create(&module, &class, setup)?;
        if !voice && inst.audio_inputs().is_empty() {
            // Still a real refusal for an EFFECT: with no audio input there is
            // nothing to send it and it would answer with silence for ever. A
            // voice has no audio input by design.
            return Err(format!(
                "{} has no audio input, so there is nothing to send to it",
                class.name
            ));
        }
        let channels = inst
            .audio_outputs()
            .first()
            .map(|b| b.channels.max(0) as usize)
            .unwrap_or(0);
        if channels == 0 {
            return Err(format!("{} has no audio output channels", class.name));
        }
        // **The preset, BEFORE the warm-up**, for the reason the slot path
        // documents: a preset change makes a sampled instrument reload, and
        // state that arrives after the gate has closed means the gate waited
        // for the wrong load. `load_state` also refuses a restore once the
        // instance has rendered, so this is the only moment it can happen.
        let state_error = match state {
            Some(bytes) => inst.load_state(bytes).err(),
            None => None,
        };
        // **A voice warms up like the slot it used to be.** The gate exists so
        // a sampled piano is not handed to the callback while it still renders
        // silence; nothing about moving into a bay changed that. An effect
        // needs no warm-up — see the note on `load` — and gets none.
        if voice {
            let gate = ivory_host::ready::warm_up(&mut inst, ivory_host::Policy::default());
            if gate.state() == ivory_host::ReadyState::Failed {
                return Err(gate
                    .reason()
                    .unwrap_or("the instrument failed to warm up")
                    .to_string());
            }
        }
        // Processing on, or the plugin is active and refuses every block.
        inst.set_processing(true)?;
        // **The editor's and the state's only chance to get a reference**, for
        // exactly the reason spelled out in `load_plugin_with_state`: one line
        // below, `inst` goes into a `Hosted`, into a `PluginBox`, and across a
        // ring into the audio callback, and after that there is no `&Instance`
        // on this thread and no safe way to make one. The state handle is what
        // stops an effect's settings — or a bay instrument's preset — dying at
        // every relaunch, which until now they did.
        let editor_handle = inst.editor_handle();
        let state_handle = inst.state_handle();
        let loaded = Loaded {
            bundle: bundle.to_path_buf(),
            class: class.name.clone(),
            vendor: module.vendor().to_owned(),
            channels: u16::try_from(channels).unwrap_or(u16::MAX),
            sample_rate: self.output.sample_rate,
        };
        // Whatever was in THIS bay is going away, so its window and then its
        // controller and state references go first — the lifetime rule from
        // `editors`.
        // `hand_off_insert` is where the old instance is dropped, and dropping
        // it terminates the controller the old handle points at.
        self.close_insert_editor(strip, slot);
        if let Some(h) = self
            .insert_editor_handles
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *h = None;
        }
        if let Some(h) = self
            .insert_state_handles
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *h = None;
        }
        self.hand_off_insert(
            strip,
            slot,
            PluginBox(Some(Box::new(Hosted {
                inst,
                module,
                bufs: vec![vec![0.0; MAX_BLOCK as usize]; channels.max(TAP_CHANNELS)],
                channels,
                voice,
            }))),
        );
        if let Some(h) = self
            .insert_editor_handles
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *h = editor_handle;
        }
        if let Some(h) = self
            .insert_state_handles
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *h = Some(state_handle);
        }
        self.inserts_loaded[strip][slot] = Some(loaded.clone());
        if let Some(e) = self
            .insert_state_errors
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *e = state_error.map(|err| err.to_string());
        }
        Ok(Some(loaded))
    }

    /// Why the state handed to [`Engine::load_insert`] was not restored into
    /// this bay.
    ///
    /// `None` is the normal answer: nothing was offered, or it was restored.
    /// `Some` means the plugin came up with its defaults and the user's
    /// preset did not survive — worth saying out loud, because the symptom is
    /// "it keeps forgetting my piano" and nothing else in the app would know.
    pub fn insert_state_error(&self, strip: usize, slot: usize) -> Option<&str> {
        self.insert_state_errors
            .get(strip)?
            .get(slot)?
            .as_deref()
    }

    /// What is in one insert, if anything.
    pub fn insert(&self, strip: usize, slot: usize) -> Option<&Loaded> {
        self.inserts_loaded.get(strip)?.get(slot)?.as_ref()
    }

    /// An insert's half of [`Engine::hand_off`], and the same wait.
    fn hand_off_insert(&mut self, strip: usize, slot: usize, next: PluginBox) {
        let Some(pair) = self
            .insert_handoff
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        else {
            return;
        };
        while pair.from_audio.pop().is_ok() {}
        if pair.to_audio.push(next).is_err() {
            return;
        }
        let deadline = Instant::now() + RETIRE_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(old) = pair.from_audio.pop() {
                drop(old);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    // ── the plugins' own editors ────────────────────────────────────────────

    /// One insert's own window: open it, or raise it if it is already up.
    ///
    /// The same handle trick as [`Engine::open_editor`] — see the long comment
    /// in `load_plugin_with_state` for why a reference taken at load time is
    /// the only way to reach a plugin the audio thread owns.
    pub fn open_insert_editor(&mut self, strip: usize, slot: usize) -> Result<(), String> {
        if strip > STRIPS || slot >= INSERTS {
            return Ok(());
        }
        self.poll_editor();
        if let Some(editor) = self
            .insert_editors
            .get(strip)
            .and_then(|r| r.get(slot))
            .and_then(Option::as_ref)
        {
            editor.focus();
            return Ok(());
        }
        let Some(handle) = self
            .insert_editor_handles
            .get(strip)
            .and_then(|r| r.get(slot))
            .and_then(Option::as_ref)
        else {
            return Err(format!("nothing is loaded in insert {}", slot + 1));
        };
        // The CHANNEL and the bay, both, because the same compressor on four
        // channels gives four windows that are otherwise identical.
        let title = match self.insert(strip, slot) {
            Some(l) => format!("{} - Tangent, insert {}", l.class, slot + 1),
            None => format!("Insert {} - Tangent", slot + 1),
        };
        let editor =
            ivory_host::Editor::open_handle(handle, &title).map_err(|e| e.to_string())?;
        if let Some(e) = self
            .insert_editors
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *e = Some(editor);
        }
        Ok(())
    }

    /// Close one insert's window if it is open. Safe when it is not.
    pub fn close_insert_editor(&mut self, strip: usize, slot: usize) {
        if let Some(e) = self
            .insert_editors
            .get_mut(strip)
            .and_then(|r| r.get_mut(slot))
        {
            *e = None;
        }
    }

    /// Whether the plugin in this bay offers an editor to open.
    ///
    /// `false` for an empty bay, for one that does not exist, and for a
    /// plugin that has no UI — which is the honest thing to grey a control
    /// on. The first call after a load is not free (VST3 has no `hasEditor`,
    /// so the only way to ask is to build a view and throw it away); every
    /// call after that is a cached bool.
    pub fn insert_has_editor(&self, strip: usize, slot: usize) -> bool {
        self.insert_editor_handles
            .get(strip)
            .and_then(|r| r.get(slot))
            .and_then(Option::as_ref)
            .is_some_and(ivory_host::EditorHandle::has_editor)
    }

    /// Is this insert's window open right now?
    pub fn insert_editor_open(&self, strip: usize, slot: usize) -> bool {
        self.insert_editors
            .get(strip)
            .and_then(|r| r.get(slot))
            .is_some_and(Option::is_some)
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
        // An insert's window closed by its own close button would otherwise
        // stay "open" here for ever, and
        // the click that should reopen it would silently raise nothing.
        for rack in &mut self.insert_editors {
            for editor in rack {
                if editor.as_ref().is_some_and(ivory_host::Editor::closed) {
                    *editor = None;
                }
            }
        }
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
    /// Which general CHANNEL the built-in DX7 belongs to. See
    /// `Shared::builtin_strip`: the host computes it from bay 1's sentinel and
    /// the channel's kind; the callback only reads it.
    pub fn set_builtin_strip(&self, strip: Option<usize>) {
        self.shared
            .builtin_strip
            .store(strip.map_or(-1, |s| s as i64), Ordering::Relaxed);
    }

    /// One general channel's fader.
    pub fn set_channel_gain(&self, ch: usize, linear: f32) {
        self.shared.set_channel_gain(ch, linear);
    }

    /// Which channel each capture pick feeds: desk indices, `None` unbound.
    pub fn set_pick_strips(&self, strips: [Option<usize>; INPUTS]) {
        for (cell, strip) in self.shared.pick_strip.iter().zip(strips) {
            cell.store(strip.map_or(-1, |s| s as i64), Ordering::Relaxed);
        }
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

    /// Whether a backing track is loaded, playing or not.
    ///
    /// Loaded is the question, not playing: a take is armed before the
    /// transport rolls, and a track that starts with it would otherwise decide
    /// the take's sources one buffer too late.
    pub fn track_loaded(&self) -> bool {
        self.shared.track_loaded.load(Ordering::Relaxed)
    }

    // ── the instruments' state ──────────────────────────────────────────────

    /// Which channels' rack voices ignore the note stream, as one mask.
    ///
    /// Pushed whole, like mute and solo: the UI computes it from every
    /// channel's kind and arm switch, and the audio thread reads it per chunk.
    /// Zero is every channel armed, which is the desk as it has always been.
    pub fn set_midi_off(&self, mask: u32) {
        self.shared.midi_off.store(mask, Ordering::Relaxed);
    }

    /// One insert's current state, for persisting. `None` for an empty bay —
    /// or for a plugin whose `getState` failed, which is a preset to choose
    /// again rather than an error to stop on.
    pub fn save_insert_state(&self, strip: usize, slot: usize) -> Option<Vec<u8>> {
        self.insert_state_handles
            .get(strip)?
            .get(slot)?
            .as_ref()
            .and_then(|h| h.save().ok())
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

    /// Whatever cpal last reported about the device.
    pub fn fault(&self) -> Option<String> {
        self.fault.lock().ok().and_then(|g| g.clone())
    }

    // ── gains ───────────────────────────────────────────────────────────────

    /// The three effect knobs, 0..=1.
    pub fn set_effects(&self, sends: crate::effects::Sends) {
        self.shared.set_effects(sends);
    }

    /// What each effect is set to.
    pub fn set_effect_params(&self, params: crate::effects::Params) {
        self.shared.set_effect_params(params);
    }

    /// Hand the renderer a clip and its place on the timeline, or `None` to
    /// take the lane's clip away.
    ///
    /// **The displaced clip dies HERE, on this thread.** The push waits for the
    /// return by the racks' own bargain: whatever the callback was holding
    /// comes back on `track_from_audio` and is dropped by the caller that sent
    /// its replacement — never by the audio thread, whose `drop` of a
    /// hundred-megabyte `Vec` was a measured, reachable stall.
    pub fn set_track(&mut self, placed: Option<Placed>) {
        // Whatever came back earlier is dropped first, so the ring always has
        // room and two rapid loads cannot wedge.
        while self.track_from_audio.pop().is_ok() {}
        self.shared
            .track_loaded
            .store(placed.is_some(), Ordering::Relaxed);
        if self.track_to_audio.push(placed).is_err() {
            return;
        }
        let deadline = Instant::now() + RETIRE_TIMEOUT;
        while Instant::now() < deadline {
            if let Ok(old) = self.track_from_audio.pop() {
                drop(old);
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Put the playhead at `frames`, stamped with the host's generation.
    ///
    /// Payload first, generation last with Release — `run_transport`'s Acquire
    /// pairs with it, so a callback that sees the new generation sees the new
    /// position. The generation is the HOST's monotonic counter, stored rather
    /// than incremented here, so a freshly built engine (req 0, ack 0) never
    /// matches a host that has published anything and is re-located on its
    /// first pushed frame.
    pub fn set_transport(&self, generation: u64, frames: u64) {
        self.shared.transport_at.store(frames, Ordering::Relaxed);
        self.shared.transport_req.store(generation, Ordering::Release);
    }

    /// The transport level: rolling or stopped. Safe to re-assert every frame;
    /// nothing keys off its edges.
    pub fn set_rolling(&self, rolling: bool) {
        self.shared.rolling.store(rolling, Ordering::Relaxed);
    }

    /// Where the callback's clock actually is, in frames. Monotone while
    /// rolling, constant while stopped, and exact for a given locate once
    /// [`Engine::transport_acked`] answers true for its generation.
    pub fn transport_position(&self) -> u64 {
        self.shared.transport_pos.load(Ordering::Relaxed)
    }

    /// Whether the callback has applied the locate stamped `generation`.
    pub fn transport_acked(&self, generation: u64) -> bool {
        self.shared.transport_ack.load(Ordering::Acquire) == generation
    }

    pub fn set_track_gain(&self, linear: f32) {
        self.shared.set_track_gain(linear);
    }

    /// Hear the live input, or stop hearing it.
    ///
    /// **Never restored from a settings file.** Off at every launch, because
    /// the failure mode is somebody turning their speakers on after a relaunch
    /// they had forgotten was monitoring, and getting a room full of feedback.
    pub fn set_monitor_on(&self, on: bool) {
        self.shared.monitor_on.store(on, Ordering::Relaxed);
    }

    /// What the monitoring path is holding, in milliseconds.
    ///
    /// **Latency the panel could not see.** The estimate beside it is one
    /// buffer in plus one buffer out; this is the ring between them, which on
    /// a session where the output started late used to hold a tenth of a
    /// second and never give it back. It is bounded now — see `mix_monitor` —
    /// and this is what makes the bound visible rather than trusted.
    pub fn monitor_backlog_ms(&self) -> f64 {
        let rate = f64::from(self.output.sample_rate.max(1));
        f64::from(self.shared.monitor_backlog.load(Ordering::Relaxed)) * 1000.0 / rate
    }

    /// Hand the renderer the live input's ring. `None` takes it away.
    pub fn set_monitor(&mut self, tap: Option<(rtrb::Consumer<f32>, u16, [u8; INPUTS], u32)>) {
        if let Ok(mut g) = self.shared.pending_monitor.lock() {
            *g = Some(tap);
        }
    }

    /// The desk: what every strip sends to the effects bus, and what is heard.
    ///
    /// **Pushed whole, every frame, like the gains.** The masks are built here
    /// rather than toggled bit by bit so that "is anything soloed" can never be
    /// answered from a half-written state, and the `From` below is exhaustive —
    /// which is what makes the UI's strip order and this one provably the same
    /// rather than the same by inspection.
    pub fn set_desk(&self, desk: &ivory_ui::recorder::Desk) {
        let mut muted = 0u32;
        let mut soloed = 0u32;
        let sh = &self.shared;
        for ui in ivory_ui::recorder::Strip::all() {
            let here = Strip::from(ui);
            sh.send[here.index()].store(
                desk.send[ui.index()].clamp(0.0, 1.0).to_bits(),
                Ordering::Relaxed,
            );
            if desk.muted[ui.index()] {
                muted |= here.bit();
            }
            if desk.soloed[ui.index()] {
                soloed |= here.bit();
            }
        }
        sh.muted.store(muted, Ordering::Relaxed);
        sh.soloed.store(soloed, Ordering::Relaxed);
    }

    /// The loudest thing each strip has made since this was last called, and
    /// it CLEARS as it reads. Zero for a strip that has been silent.
    pub fn strip_peaks(&self) -> [[f32; 2]; STRIPS] {
        std::array::from_fn(|i| {
            std::array::from_fn(|c| {
                f32::from_bits(self.shared.strip_peak[i][c].swap(0, Ordering::Relaxed))
            })
        })
    }

    /// What comes back from the effects bus, at its own fader.
    pub fn set_fx_return(&self, linear: f32) {
        self.shared
            .fx_return
            .store(linear.clamp(0.0, 8.0).to_bits(), Ordering::Relaxed);
    }

    /// The master, as a linear gain. See [`Shared::master_gain`].
    pub fn set_master_gain(&self, linear: f32) {
        self.shared
            .master_gain
            .store(sane_gain(linear).to_bits(), Ordering::Relaxed);
    }

    /// Decibels the limiter has taken off since this was last asked.
    ///
    /// **Read and reset**, like the meter peaks and for the same reason: gain
    /// reduction is a transient a few samples long, and a UI that asks sixty
    /// times a second would otherwise miss the moment the meter exists for.
    pub fn gain_reduction_db(&self) -> f32 {
        f32::from_bits(self.shared.gr_db.swap(0, Ordering::Relaxed))
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
        // **Every insert too, and its window BEFORE its instance.**
        //
        // This unloaded the instrument slots and nothing else, and then the
        // fields dropped in declaration order: `stream` is first, so the
        // callback box went, then the `Renderer`, then `racks`, and every
        // insert's `Hosted` was terminated — and only after all that did
        // `insert_editors` drop and call `IPlugView::removed()` and
        // `setFrame(null)` on a view whose controller no longer existed.
        //
        // That is precisely the lifetime rule `editors` is documented with and
        // ordered for, and it was reachable by quitting with an effect's
        // window open. `load_insert(.., None)` is the whole teardown in the
        // right order: close the window, drop the controller reference, then
        // retire the instance and drop it HERE, on this thread.
        for strip in 0..=STRIPS {
            for slot in 0..INSERTS {
                let _ = self.load_insert(strip, slot, None, None);
            }
        }
    }
}

impl std::fmt::Debug for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // cpal::Stream is not Debug.
        f.debug_struct("Engine")
            .field("output", &self.output)
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

/// How many periods the requested size is asked for, per platform.
///
/// **cpal's ALSA host reads `Fixed(v)` as the whole RING, not the period**, and
/// splits it into four (`host/alsa/mod.rs`: `period = v / 4`). So asking for
/// 256 on Linux does not produce 256-frame callbacks — it produces a 256-frame
/// ring refilled every 64 frames, which is four times the callback rate the
/// number was chosen for, on a plain SCHED_OTHER thread. macOS and WASAPI read
/// the same call as the period.
///
/// Multiplying by four on Linux makes the two agree: the callback cadence is
/// the 256 frames it was always meant to be, and what deepens is the safety
/// ring behind it to the four periods ALSA assumes anyway. Measured on a 2012
/// MacBook Air under four CPU hogs: six underruns in thirty seconds at the
/// stock setting, none at all with the ring four times deeper.
///
/// **And no realtime promotion to go with it.** The obvious companion fix
/// makes it dramatically worse through pipewire-alsa on the same box — 75
/// underruns against 6, starting the moment the thread is promoted, because
/// the plugin's own data loop already runs at RT 83 and a client at FIFO 70
/// inverts priority against its non-RT IPC thread. See
/// `docs/LINUX-4.16-FINDINGS.md`.
const BUFFER_PERIODS: u32 = if cfg!(target_os = "linux") { 4 } else { 1 };

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
            // The PERIOD is what `want` means; the ring is however many
            // periods this platform's host expects to be handed. See
            // `BUFFER_PERIODS`.
            let ring = want.saturating_mul(BUFFER_PERIODS).clamp(*min, *max);
            // What is reported back is the period, because that is the number
            // the callback runs at and the one the Audio Status panel is
            // talking about.
            let period = (ring / BUFFER_PERIODS).max(1);
            (cpal::BufferSize::Fixed(ring), Some(period))
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
    /// Three layers, one per channel: the count the old three-slot rack had,
    /// kept because +6.0/+9.5 dB are the numbers the table below explains.
    const LAYERS: usize = 3;
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
        "loading:  {} into channel 1's first bay (this blocks for the warm-up)",
        bundle.display()
    );
    let loaded = match engine.load_insert(0, 0, Some(&bundle), None) {
        Ok(Some(l)) => l,
        Ok(None) => {
            eprintln!("the bay refused the path");
            std::process::exit(1);
        }
        Err(why) => {
            eprintln!("could not load the instrument: {why}");
            std::process::exit(1);
        }
    };
    println!(
        "instrument: {} [{}], {} channels at {} Hz",
        loaded.class, loaded.vendor, loaded.channels, loaded.sample_rate
    );

    // ── the state, which is what makes a preset survive a restart ───────────
    //
    // Read here rather than at the end, because the other two channels are
    // loaded WITH it: the layering measurement below rests on the three
    // instruments being identical, and "restored from channel 1's own bytes"
    // is a stronger claim than "the same plugin file".
    let preset = engine.save_insert_state(0, 0);
    match &preset {
        Some(b) => println!("state:    {} bytes from channel 1", b.len()),
        None => println!("state:    this instrument handed over none"),
    }

    for ch in 0..LAYERS {
        engine.set_channel_gain(ch, LAYER_GAIN);
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
    println!(
        "\nlayering: the same instrument on each of three channels, every fader at gain {LAYER_GAIN}"
    );
    let mut rows: Vec<(usize, f32, f32, Option<f64>)> = Vec::new();
    for slot in 0..LAYERS {
        if slot > 0 {
            println!("  loading channel {} with the same instrument...", slot + 1);
            if let Err(why) =
                engine.load_insert(slot, 0, Some(&bundle), preset.as_deref())
            {
                println!("  channel {} would not load: {why}", slot + 1);
                break;
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

    println!("\n  chans  device peak   tap peak   tap vs one   process CPU");
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
         move is a channel that never got the note. Read the TAP column for that -\n  \
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
        if engine.insert_has_editor(0, 0) {
            "yes"
        } else {
            "this plugin has none"
        }
    );
    if engine.insert_has_editor(0, 0) {
        // This process will never call `[NSApp run]`: `main.rs` exits before
        // eframe starts. Without this the window opens behind everything, never
        // takes the keyboard, and looks broken.
        ivory_host::editor::become_foreground();
        match engine.open_insert_editor(0, 0) {
            Ok(()) => println!("          channel 1 open. Close the window to end this probe."),
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
    let deadline = if engine.insert_editor_open(0, 0) {
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
        if deadline.is_none() && !engine.insert_editor_open(0, 0) {
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
    match (preset.as_deref(), engine.save_insert_state(0, 0)) {
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
    if rows.len() == LAYERS {
        let _ = engine.load_insert(1, 0, None, None);
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
            "\nchannel 2 unloaded: peak {after:.4} from the other two - unloading one \
             instrument must not silence its neighbours"
        );
        println!(
            "loaded now:      {:?}",
            (0..LAYERS)
                .map(|s| engine.insert(s, 0).map(|l| l.class.as_str()))
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
        // A rack, and the whole renderer, are `Send` because every field of
        // them is — DERIVED, not asserted. `unsafe impl Send` appears exactly
        // once in this file and a second one would mean something is being
        // shared rather than moved. cpal requires this of the closure it takes,
        // so it is checked here rather than discovered at the call site.
        assert_send::<Rack>();
        assert_send::<[Rack; STRIPS + 1]>();
        assert_send::<Renderer>();
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
    impl Renderer {
        /// Replace the track rings with fresh ones and hand back the far
        /// ends, so a test can send clips and collect what comes back —
        /// the same two-way trip the engine makes.
        fn hijack_track_rings(
            &mut self,
        ) -> (Producer<Option<Placed>>, Consumer<Option<Placed>>) {
            let (to, incoming) = RingBuffer::<Option<Placed>>::new(2);
            let (retiring, from) = RingBuffer::<Option<Placed>>::new(2);
            self.track_incoming = incoming;
            self.track_retiring = retiring;
            (to, from)
        }
    }

    fn test_renderer(shared: Arc<Shared>, tap: Producer<f32>, dev_ch: usize) -> Renderer {
        // The far halves are dropped here on purpose: `rtrb` handles an
        // abandoned peer (`pop` reports empty, `push` fills to capacity), so a
        // renderer under test needs no partner threads at all.
        let (_, midi) = RingBuffer::<MidiEvent>::new(1024);
        // Empty racks: every test here is about the mixing, and an empty rack
        // is the path every channel takes until somebody loads something.
        let racks: [Rack; STRIPS + 1] = std::array::from_fn(|_| {
            let mut inc = Vec::new();
            let mut ret = Vec::new();
            for _ in 0..INSERTS {
                let (_to, i) = RingBuffer::<PluginBox>::new(1);
                let (r, _from) = RingBuffer::<PluginBox>::new(1);
                inc.push(i);
                ret.push(r);
            }
            let mut inc = inc.into_iter();
            let mut ret = ret.into_iter();
            Rack {
                slots: std::array::from_fn(|_| PluginBox(None)),
                incoming: std::array::from_fn(|_| inc.next().expect("one per insert")),
                retiring: std::array::from_fn(|_| ret.next().expect("one per insert")),
            }
        });
        Renderer {
            monitor: None,
            monitor_channels: 0,
            monitor_gain: [0.0; INPUTS],
            monitor_widths: [0; INPUTS],
            monitor_block: 0,
            monitor_scratch: Vec::new(),
            racks,
            fx_in: vec![vec![0.0; MAX_BLOCK as usize]; TAP_CHANNELS],
            fx_out: vec![vec![0.0; MAX_BLOCK as usize]; TAP_CHANNELS],
            master_effects: crate::effects::Effects::new(RATE as f32),
            room_effects: crate::effects::Effects::new(RATE as f32),
            room: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            input_dry: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            room_live: false,
            room_gain: 0.0,
            aux: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            click_out: vec![0.0; MAX_BLOCK as usize],
            click_taped: vec![0.0; MAX_BLOCK as usize],
            fx_return_gain: 1.0,
            track: None,
            pos: 0,
            track_gain: 1.0,
            seen_req: 0,
            was_rolling: false,
            sounding: 0,
            // Abandoned peers, like the MIDI ring above: pop reports empty
            // and push fills to capacity, which is all a test needs.
            track_incoming: RingBuffer::<Option<Placed>>::new(2).1,
            track_retiring: RingBuffer::<Option<Placed>>::new(2).0,
            shared,
            timebase: Timebase::new(),
            rate: RATE,
            dev_channels: dev_ch,
            mix: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            builtin_scratch: vec![0.0; MAX_BLOCK as usize * TAP_CHANNELS],
            channel_gain: [1.0; CHANNELS],
            midi,
            pending: None,
            notes: Vec::with_capacity(MAX_EVENTS_PER_BLOCK),
            builtin: crate::dx7::Dx7::new(RATE as f32),
            effects: crate::effects::Effects::new_send(RATE as f32),
            effect_params: crate::effects::Params::default(),
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
        // **The fresh desk's own shape**: channel 0 is MIDI with the built-in
        // in bay 1, exactly what `Settings::default` produces and the host
        // pushes. The silent auto-fallback died with the slots, so a renderer
        // under test says so the way the app does — by assignment.
        shared.builtin_strip.store(0, Ordering::Relaxed);
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

    /// **A bay's settings survive: state out, state back in, no complaint.**
    ///
    /// The round trip is the whole feature — `save_insert_state` is what the
    /// quit path writes to disk and the `state` argument is what the reconcile
    /// hands back at launch. Before this existed, every effect's knobs died
    /// with the process, silently, masked only by nobody expecting better.
    ///
    ///     cargo test -p ivory a_bays_settings -- --ignored --nocapture
    #[test]
    #[ignore = "needs an audio device and FabFilter Pro-R 2; runs a vendor's initialiser"]
    fn a_bays_settings_survive_a_save_and_a_reload() {
        let Some(bundle) = ivory_host::discover().into_iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains("Pro-R"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pro-R; this test needs one installed");
        };
        let mut engine = Engine::start(None).expect("an audio output");
        let at = Strip::Track.index();
        engine
            .load_insert(at, 0, Some(&bundle), None)
            .expect("an effect must load");
        let state = engine
            .save_insert_state(at, 0)
            .expect("a loaded insert must answer for its state");
        assert!(!state.is_empty(), "a state of zero bytes is not a state");

        // The trip back: the same bytes into a fresh instance of the same
        // plugin, which must accept them without complaint.
        engine
            .load_insert(at, 1, Some(&bundle), Some(&state))
            .expect("the state a plugin saved must restore into it");
        assert!(engine.insert(at, 1).is_some());
    }

    /// **An instrument lives in a bay now: no refusal, a warm-up, notes in,
    /// sound out — and the arm mask is a real gate.**
    ///
    /// On the MASTER's rack, because it is the one that runs unconditionally —
    /// the track's runs only while the backing track plays and the inputs'
    /// only while the monitor is open. A voice on the master replaces the whole
    /// mix, which is exactly what "a voice replaces the channel" means, so the
    /// master meter moving is the assertion.
    ///
    ///     cargo test -p ivory an_instrument_in_a_bay -- --ignored --nocapture
    #[test]
    #[ignore = "needs an audio device and Pianoteq installed; runs a vendor's initialiser"]
    fn an_instrument_in_a_bay_renders_notes_and_the_arm_gate_holds() {
        let Some(bundle) = ivory_host::discover().into_iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().to_lowercase().contains("pianoteq"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pianoteq; this test needs one installed");
        };
        let mut engine = Engine::start(None).expect("an audio output");
        engine
            .load_insert(STRIPS, 0, Some(&bundle), None)
            .expect("an instrument must load in a bay - the refusal is gone");
        assert!(
            engine.insert(STRIPS, 0).is_some(),
            "the bay does not report its instrument"
        );

        let peak_over = |engine: &Engine, ms: u64| -> f32 {
            let deadline = Instant::now() + Duration::from_millis(ms);
            let mut peak = 0.0f32;
            while Instant::now() < deadline {
                let m = engine.meters();
                peak = peak.max(m.left.peak).max(m.right.peak);
                std::thread::sleep(Duration::from_millis(20));
            }
            peak
        };

        // Armed: a chord, and the master must move.
        let t = engine.timebase().now();
        for n in [48u8, 60, 64, 67] {
            engine.send_midi(t, &[0x90, n, 100]);
        }
        let heard = peak_over(&engine, 1500);
        for n in [48u8, 60, 64, 67] {
            engine.send_midi(engine.timebase().now(), &[0x80, n, 0]);
        }
        assert!(
            heard > 0.001,
            "a bay instrument fed notes made no sound (peak {heard})"
        );

        // Let the tails die, then gate the channel and play again.
        std::thread::sleep(Duration::from_millis(2500));
        engine.set_midi_off(1 << STRIPS.min(31));
        let t = engine.timebase().now();
        for n in [48u8, 60, 64, 67] {
            engine.send_midi(t, &[0x90, n, 100]);
        }
        let gated = peak_over(&engine, 1200);
        for n in [48u8, 60, 64, 67] {
            engine.send_midi(engine.timebase().now(), &[0x80, n, 0]);
        }
        assert!(
            gated < heard * 0.2,
            "the arm gate did not hold: armed peak {heard}, gated peak {gated}"
        );
    }

    /// **The engine retires its own inserts, on this thread, before anything
    /// else drops.**
    ///
    /// `Drop for Engine` unloaded the instrument slots and nothing else, and
    /// then the fields dropped in declaration order — `stream` is first, so the
    /// callback box went, then the `Renderer`, then `racks`, and every insert's
    /// instance was terminated from wherever cpal happens to free its closure.
    /// Only after all of that did `insert_editors` drop and call
    /// `IPlugView::removed()` and `setFrame(null)` on a view whose controller
    /// no longer existed — which is precisely the rule `editors` is documented
    /// with and ordered for, and it was reachable by quitting with an effect's
    /// window open.
    ///
    /// **What this test cannot do, honestly.** The window half is not reachable
    /// from a test harness: libtest runs every test on a spawned thread, even
    /// at `--test-threads=1`, and `Editor::open_handle` refuses with "a plugin
    /// editor can only be opened on the main thread" — correctly, because that
    /// is what VST3 and AppKit require. So this tests the teardown PRIMITIVE
    /// that `Drop` now calls for every bay, and the ordering above it is two
    /// lines of reading. The window path is exercised by quitting the app with
    /// an effect window open.
    ///
    /// Ignored like every test that opens somebody else's binary.
    ///
    ///     cargo test -p ivory an_insert_is_retired -- --ignored --nocapture
    #[test]
    #[ignore = "needs an audio device and FabFilter Pro-R 2; runs a vendor's initialiser"]
    fn an_insert_is_retired_by_the_call_drop_makes() {
        let Some(bundle) = ivory_host::discover().into_iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains("Pro-R"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pro-R; this test needs one installed");
        };
        let mut engine = Engine::start(None).expect("an audio output");
        let at = Strip::Track.index();
        engine
            .load_insert(at, 0, Some(&bundle), None)
            .expect("an effect must load on an insert");
        assert!(engine.insert(at, 0).is_some(), "nothing was loaded");
        assert!(
            !engine.insert_editor_open(at, 0),
            "a window opened that nobody asked for"
        );

        // The exact call `Drop for Engine` now makes for every bay: close the
        // window, drop the controller reference, retire the instance and drop
        // it HERE rather than in the callback box's teardown.
        engine
            .load_insert(at, 0, None, None)
            .expect("unloading an insert must not fail");
        assert!(engine.insert(at, 0).is_none(), "the bay kept its effect");
        assert!(!engine.insert_editor_open(at, 0), "a window survived the unload");

        // And dropping a live engine with a loaded insert completes.
        engine
            .load_insert(at, 1, Some(&bundle), None)
            .expect("an effect must load on an insert");
        drop(engine);
    }

    /// **A rack never drops a plugin, and the proof is that it declines to take
    /// the new one.**
    ///
    /// The callback may not run a vendor's teardown: `terminate` frees sample
    /// memory and joins worker threads, and doing that between two blocks of
    /// audio is a dropout at best. So the protocol is that the old instance
    /// goes BACK for the UI thread to drop, and the only way to guarantee that
    /// is to check there is room for it before taking the new one.
    ///
    /// `Rack::swap` did the opposite — pop, replace, `let _ = push(old)` — and
    /// `let _` on an rtrb push is a trap, because `PushError::Full(T)` carries
    /// the value: discarding the `Result` drops the plugin, on the audio
    /// thread, exactly where it must not happen. It is reachable, because
    /// `hand_off_insert` gives up after `RETIRE_TIMEOUT` and leaves the old one
    /// sitting in the ring, so the next load finds `retiring` full.
    ///
    /// Measured by OCCUPANCY, which needs no real plugin: with no room to give
    /// one back, the arrival must still be in its ring afterwards.
    #[test]
    fn a_full_retiring_ring_makes_a_rack_wait_rather_than_drop() {
        let mut to_audio = Vec::new();
        let mut from_audio = Vec::new();
        let mut inc = Vec::new();
        let mut ret = Vec::new();
        for _ in 0..INSERTS {
            let (t, i) = RingBuffer::<PluginBox>::new(1);
            let (r, f) = RingBuffer::<PluginBox>::new(1);
            to_audio.push(t);
            inc.push(i);
            ret.push(r);
            from_audio.push(f);
        }
        let mut inc = inc.into_iter();
        let mut ret = ret.into_iter();
        let mut rack = Rack {
            slots: std::array::from_fn(|_| PluginBox(None)),
            incoming: std::array::from_fn(|_| inc.next().expect("one per insert")),
            retiring: std::array::from_fn(|_| ret.next().expect("one per insert")),
        };

        // Bay 0's retiring ring is full — the UI thread timed out and never
        // collected what it was given.
        rack.retiring[0].push(PluginBox(None)).expect("room for one");
        // And a fresh plugin has been sent to that bay.
        to_audio[0].push(PluginBox(None)).expect("room for one");
        assert_eq!(rack.incoming[0].slots(), 1, "the arrival never reached the ring");

        rack.swap();

        assert_eq!(
            rack.incoming[0].slots(),
            1,
            "the rack took a plugin it had nowhere to put the old one - which is \
             a `terminate` on the audio thread"
        );

        // And once the UI thread collects, the very next swap goes through.
        from_audio[0].pop().expect("the retired one was there to collect");
        rack.swap();
        assert_eq!(rack.incoming[0].slots(), 0, "the rack never took the arrival");
        assert_eq!(from_audio[0].slots(), 1, "the displaced one was not handed back");
    }

    /// The other bays are untouched by a bay that is waiting.
    #[test]
    fn one_stuck_bay_does_not_stall_the_rest_of_the_rack() {
        let mut to_audio = Vec::new();
        let mut from_audio = Vec::new();
        let mut inc = Vec::new();
        let mut ret = Vec::new();
        for _ in 0..INSERTS {
            let (t, i) = RingBuffer::<PluginBox>::new(1);
            let (r, f) = RingBuffer::<PluginBox>::new(1);
            to_audio.push(t);
            inc.push(i);
            ret.push(r);
            from_audio.push(f);
        }
        let mut inc = inc.into_iter();
        let mut ret = ret.into_iter();
        let mut rack = Rack {
            slots: std::array::from_fn(|_| PluginBox(None)),
            incoming: std::array::from_fn(|_| inc.next().expect("one per insert")),
            retiring: std::array::from_fn(|_| ret.next().expect("one per insert")),
        };
        rack.retiring[0].push(PluginBox(None)).expect("room for one");
        for t in &mut to_audio {
            t.push(PluginBox(None)).expect("room for one");
        }

        rack.swap();

        assert_eq!(rack.incoming[0].slots(), 1, "bay 0 should still be waiting");
        for i in 1..INSERTS {
            assert_eq!(
                rack.incoming[i].slots(),
                0,
                "bay {i} was held up by a bay it has nothing to do with"
            );
        }
    }

    /// **Mute, solo, and the one interaction between them.**
    ///
    /// Solo is exclusive and mute loses to it: pressing solo on a channel that
    /// is muted is unambiguously a request to hear it, and the alternative is a
    /// solo button that sometimes does nothing.
    #[test]
    fn solo_beats_mute_and_silences_everything_else() {
        use Strip::*;
        let all: Vec<Strip> = ivory_ui::recorder::Strip::all()
            .into_iter()
            .map(Strip::from)
            .collect();
        let none = 0;
        // Nothing muted, nothing soloed: everybody is heard.
        for s in &all {
            assert!(strip_is_heard(*s, none, none), "{s:?} was silent for no reason");
        }
        // One muted: it alone is silent.
        let m = Channel(0).bit();
        assert!(!strip_is_heard(Channel(0), m, none));
        assert!(strip_is_heard(Track, m, none));
        // One soloed: it alone is heard, and mute does not enter into it.
        let so = Track.bit();
        assert!(strip_is_heard(Track, none, so));
        assert!(!strip_is_heard(Channel(0), none, so));
        assert!(!strip_is_heard(Channel(4), none, so));
        // Soloed AND muted: heard, because pressing solo asked for it.
        assert!(
            strip_is_heard(Track, Track.bit(), Track.bit()),
            "a soloed channel was silenced by its own mute"
        );
        // Two soloed: both, and nothing else.
        let two = Track.bit() | Fx.bit();
        assert!(strip_is_heard(Track, none, two) && strip_is_heard(Fx, none, two));
        assert!(!strip_is_heard(Click, none, two));
        // **Each channel is its own strip.** Muting one must not take its
        // neighbour with it, which is the whole reason the lumped instrument
        // strip had to go.
        let one = Channel(2).bit();
        assert!(!strip_is_heard(Channel(2), one, none));
        assert!(strip_is_heard(Channel(3), one, none), "muting one channel muted another");
        // Every strip owns a bit of its own, or two of them would mute together.
        let mut bits: Vec<u32> = all.iter().map(|s| s.bit()).collect();
        let n = bits.len();
        bits.sort_unstable();
        bits.dedup();
        assert_eq!(bits.len(), n, "two strips share a bit");
    }

    /// The bus comes back at its own fader, and adds rather than replaces.
    #[test]
    fn the_effects_bus_returns_at_its_own_level() {
        let frames = 3;
        let mut mix = vec![1.0_f32; frames * TAP_CHANNELS];
        let aux = vec![0.5_f32; frames * TAP_CHANNELS];
        let mut gain = 0.0;
        let _ = add_return(&mut mix, &aux, frames, &mut gain, 1.0, 1.0);
        for (i, v) in mix.iter().enumerate() {
            assert!(
                (v - 1.5).abs() < 1.0e-6,
                "frame {i} came back as {v} rather than dry plus wet"
            );
        }
        // At zero return the bus is inaudible and the dry is untouched.
        let mut mix = vec![1.0_f32; frames * TAP_CHANNELS];
        let mut gain = 0.0;
        let _ = add_return(&mut mix, &aux, frames, &mut gain, 0.0, 1.0);
        assert!(mix.iter().all(|v| (v - 1.0).abs() < 1.0e-6));
    }

    /// **Muting the instrument silences the app.**
    ///
    /// End to end through the real renderer, because the gating happens in
    /// three places — the fader, the send and the strip — and a unit test of
    /// any one of them would pass with the wiring wrong.
    #[test]
    fn a_muted_instrument_reaches_neither_the_speakers_nor_the_bus() {
        let (mut r, mut tx, shared) = renderer_with_midi(2);
        // Every general channel muted: the built-in is on one of them and
        // this is a test about the strip, not about which channel it landed
        // on. Assigned explicitly — the silent auto-fallback died with the
        // slots, so a renderer under test says where the DX7 lives.
        shared.builtin_strip.store(0, Ordering::Relaxed);
        let mut mask = 0;
        for i in 0..CHANNELS {
            mask |= Strip::Channel(i).bit();
        }
        shared.muted.store(mask, Ordering::Relaxed);
        // Everything the bus could carry it with, turned up.
        shared.reverb_mix.store(1.0f32.to_bits(), Ordering::Relaxed);
        for i in 0..SLOTS {
            shared.send[i].store(1.0f32.to_bits(), Ordering::Relaxed);
        }

        queue(&mut tx, 0, [0x90, 60, 100]);
        let mut out = vec![0.0_f32; 2 * 2048];
        for _ in 0..4 {
            r.render(&mut out, 0, 0);
            let peak = out.iter().fold(0.0_f32, |m, s| m.max(s.abs()));
            assert!(
                peak < 1.0e-4,
                "a muted instrument produced {peak} at the device"
            );
        }
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
    /// **The transport is the position, and the clip merely stands on it.**
    ///
    /// This replaces the trim-points test — trim is gone, the playhead is the
    /// feature — and it is the audio half of `transport.rs`'s contract:
    ///
    /// 1. stopped is silent and the position does not move;
    /// 2. a locate is an EVENT — the same generation re-asserted moves nothing;
    /// 3. a locate lands while rolling, which the old level could never say;
    /// 4. `Placed::start` positions a clip with one subtraction;
    /// 5. the callback acks the generation it applied;
    /// 6. a displaced clip comes BACK on the ring — never dropped on the
    ///    audio thread, which used to free a hundred megabytes mid-callback;
    /// 7. a stop ends the notes it strands.
    #[test]
    fn the_backing_track_follows_the_transport() {
        use std::sync::Arc;
        let frames = 1000usize;
        let clip = Arc::new(ivory_record::decode::Clip {
            samples: (0..frames).flat_map(|i| [i as f32, i as f32]).collect(),
            rate: 48_000,
            source: std::path::PathBuf::from("t.wav"),
        });

        let (mut r, mut tx, shared) = renderer_with_midi(2);
        let (mut to_audio, mut from_audio) = r.hijack_track_rings();
        to_audio
            .push(Some(Placed { clip: Arc::clone(&clip), start: 0 }))
            .expect("room");
        shared.set_track_gain(1.0);
        r.track_gain = 1.0;

        // 1. Stopped: silence, and the clock does not move.
        let mut out = vec![0.0_f32; 2 * 256];
        r.render(&mut out, 0, 0);
        assert_eq!(
            out.iter().fold(0.0f32, |m, s| m.max(s.abs())),
            0.0,
            "the track played with the transport stopped"
        );
        assert_eq!(shared.transport_pos.load(Ordering::Relaxed), 0);

        // 2 + 3. Locate to 100 and roll: playback starts at frame 100.
        shared.transport_at.store(100, Ordering::Relaxed);
        shared.transport_req.store(1, Ordering::Release);
        shared.rolling.store(true, Ordering::Relaxed);
        let mut out = vec![0.0_f32; 2 * 256];
        r.render(&mut out, 0, 0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert!(
            (left[0] - 100.0).abs() < 0.5,
            "it started at frame {} and the locate said 100",
            left[0]
        );
        // 5. And the callback has acked the generation it applied.
        assert_eq!(shared.transport_ack.load(Ordering::Acquire), 1);

        // 2. The SAME generation re-asserted is not a second locate.
        shared.transport_req.store(1, Ordering::Release);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        assert!(
            (out[0] - 356.0).abs() < 0.5,
            "re-asserting a generation re-located: got {}, wanted continuity at 356",
            out[0]
        );

        // 3. A JUMP while rolling lands mid-flight — the thing the old
        // edge-detected level made inexpressible.
        shared.transport_at.store(500, Ordering::Relaxed);
        shared.transport_req.store(2, Ordering::Release);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        assert!(
            (out[0] - 500.0).abs() < 0.5,
            "a locate while rolling did nothing: got {}, wanted 500",
            out[0]
        );

        // 1 again. Stop: silence, and the position FREEZES — returning to
        // zero is the HOST's rule, applied by a locate, not the callback's.
        shared.rolling.store(false, Ordering::Relaxed);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        let frozen = shared.transport_pos.load(Ordering::Relaxed);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        assert_eq!(
            shared.transport_pos.load(Ordering::Relaxed),
            frozen,
            "the clock moved while stopped"
        );

        // 4. A clip PLACED at 300, located to 250: fifty frames of silence,
        // then the clip from its own first sample.
        to_audio
            .push(Some(Placed { clip: Arc::clone(&clip), start: 300 }))
            .expect("room");
        shared.transport_at.store(250, Ordering::Relaxed);
        shared.transport_req.store(3, Ordering::Release);
        shared.rolling.store(true, Ordering::Relaxed);
        let mut out = vec![0.0_f32; 2 * 128];
        r.render(&mut out, 0, 0);
        let left: Vec<f32> = out.iter().step_by(2).copied().collect();
        assert_eq!(left[0], 0.0, "before the clip's start is not silence");
        assert_eq!(left[49], 0.0, "frame 299 is inside the clip somehow");
        assert!(
            (left[50] - 0.0).abs() < 0.5 && (left[60] - 10.0).abs() < 0.5,
            "the placed clip did not start at its own frame 0: {} then {}",
            left[50],
            left[60]
        );

        // 6. The displaced clip came BACK rather than dying in the callback.
        // The ring holds every displacement in order — the first swap
        // displaced the initial `None` — so drain it and the clip must be in
        // what returns.
        let mut came_back = false;
        while let Ok(returned) = from_audio.pop() {
            if returned.is_some_and(|p| Arc::ptr_eq(&p.clip, &clip)) {
                came_back = true;
            }
        }
        assert!(came_back, "the displaced clip never came back on the ring");

        // 7. A note sounding when the transport stops is ENDED, not stranded.
        queue(&mut tx, r.timebase.now(), [0x90, 60, 100]);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        assert_ne!(r.sounding & (1 << 60), 0, "the note-on was never seen");
        shared.rolling.store(false, Ordering::Relaxed);
        let mut out = vec![0.0_f32; 2 * 64];
        r.render(&mut out, 0, 0);
        assert_eq!(r.sounding, 0, "stopping left a note stranded");
    }

    /// **The built-in's slot fader moves the built-in.**
    ///
    /// It did not, and it looked exactly like a working control: the number
    /// reached the settings, the engine and the meter. What it never reached
    /// was the audio — the FM renders by ADDING into the bus, so it went
    /// straight on with no gain applied while every plugin beside it went
    /// through `mix_in` with its own. Reported as "the level slider does
    /// nothing for the DX7 but works for VSTs", which is precisely what that
    /// is.
    #[test]
    fn the_builtin_is_moved_by_its_own_slot_fader() {
        let level = |gain: f32| {
            let (mut r, mut tx, shared) = renderer_with_midi(2);
            // The built-in is CHANNEL 0's now; its fader is the channel's.
            if let Some(cell) = shared.channel_gains.first() {
                cell.store(gain.to_bits(), Ordering::Relaxed);
            }
            r.channel_gain[0] = gain;
            queue(&mut tx, 0, [0x90, 60, 100]);
            let mut out = vec![0.0_f32; 2 * 4096];
            r.render(&mut out, 0, 0);
            out.iter().fold(0.0_f32, |m, s| m.max(s.abs()))
        };

        let unity = level(1.0);
        assert!(unity > 0.01, "the built-in was silent at unity");
        let half = level(0.5);
        assert!(
            (half / unity - 0.5).abs() < 0.05,
            "half gain came out at {:.3} of unity",
            half / unity
        );
        let off = level(0.0);
        assert!(off < unity * 0.02, "the fader at zero still let {off} through");
    }

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
            r.shared.set_effects(crate::effects::Sends {
                reverb,
                ..crate::effects::Sends::default()
            });
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

    /// **Three copies of one number.**
    ///
    /// `ivory-ui` declares its own `INPUTS` because it may not reach across
    /// the firewall, and `ivory-record` declares `MAX_PICKS` because that is
    /// where the channels are actually kept. A desk with more strips than the
    /// capture has picks would draw channels nothing can ever fill; fewer, and
    /// an input the user chose would be captured and never heard.
    #[test]
    fn the_desk_has_as_many_inputs_as_the_capture_can_keep() {
        assert_eq!(INPUTS, ivory_record::audio::MAX_PICKS);
        assert_eq!(INPUTS, ivory_ui::recorder::INPUTS);
        assert_eq!(STRIPS, ivory_ui::recorder::STRIPS, "the desks disagree");
        // And every UI strip maps onto a host strip at the same index, which
        // is what makes the mute masks and the send array the same arrays.
        for ui in ivory_ui::recorder::Strip::all() {
            assert_eq!(Strip::from(ui).index(), ui.index(), "{ui:?} moved");
        }
    }

    /// **Monitoring must play what is happening, not what happened.**
    ///
    /// The consumer took at most one output block per output callback, so
    /// anything that built up in the ring stayed there for the rest of the
    /// session. And it always built up: the input stream opens with the band,
    /// the OUTPUT stream starts separately and can be seconds behind it while
    /// a plugin loads, and every input callback in between pushed a block
    /// nobody was draining. The ring is 120 ms deep, so monitoring could sit a
    /// tenth of a second behind the room for ever — at the smallest buffer the
    /// app offers, with nothing on screen to point at.
    ///
    /// This fills the ring the way a late start does and checks two things:
    /// the backlog comes down to what is legitimately in flight, and what is
    /// HEARD is the newest audio rather than the oldest.
    #[test]
    fn a_monitor_that_fell_behind_catches_up_instead_of_lagging() {
        const BLOCK: usize = 128;
        let shared = Arc::new(Shared::new());
        let (tap_tx, _tap_rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        let (mut in_tx, in_rx) = RingBuffer::<f32>::new(1 << 15);
        // A second of mono input nobody drained: the old audio is 0.25 and the
        // NEWEST block — the part that should be heard — is 0.75.
        let stale = BLOCK * 40;
        for _ in 0..stale {
            let _ = in_tx.push(0.25);
        }
        r.monitor = Some(in_rx);
        r.monitor_channels = 1;
        r.monitor_widths = [1, 0, 0, 0];
        r.monitor_block = BLOCK;
        r.monitor_scratch = vec![0.0; 8192];
        // Pick 0 feeds channel 1 — an AUDIO channel by assignment, exactly
        // what the host pushes for the first audio-kind channel.
        shared.pick_strip[0].store(1, Ordering::Relaxed);
        shared.midi_off.store(1 << 1, Ordering::Relaxed);
        shared.channel_gains[1].store(1.0f32.to_bits(), Ordering::Relaxed);
        r.channel_gain[1] = 1.0;
        shared.monitor_on.store(true, Ordering::Relaxed);
        r.room_gain = 1.0;

        // Then the room, one input block per output block, which is what a
        // device that is keeping up actually does.
        let mut out = vec![0.0f32; BLOCK * 2];
        let mut heard = 0.0f32;
        for _ in 0..4 {
            for _ in 0..BLOCK {
                let _ = in_tx.push(0.75);
            }
            out.fill(0.0);
            r.render(&mut out, 0, 0);
            heard = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        }
        // Caught up: a second of stale audio did not have to be played out
        // before the room could be heard.
        assert!(
            (heard - 0.75).abs() < 0.05,
            "after four blocks the monitor is still playing {heard} - the audio \
             from a second ago, which is a backlog nothing drains and latency \
             nobody can see"
        );
        let slipped = shared.monitor_slip.load(Ordering::Relaxed);
        assert!(
            slipped >= stale as u64 / 2,
            "only {slipped} stale frames were dropped, of {stale}"
        );
        // And what is LEFT is what is in flight — one input block and one
        // output block — rather than everything that fits.
        assert!(
            r.monitor.as_ref().is_some_and(|m| m.slots() <= BLOCK * 3),
            "the ring is still holding a backlog"
        );
    }

    /// **The take is the desk. The room is the desk minus what nobody asked
    /// to hear.**
    ///
    /// Monitoring used to decide both, and the take was the casualty: a
    /// microphone monitored through the interface's own hardware — the
    /// ordinary way anybody records one, because it is the only way with no
    /// latency — is not monitored here, and produced a file with no microphone
    /// in it. Now mute decides the file and monitoring decides the speakers,
    /// which is why this asserts on BOTH sides of the same render.
    #[test]
    fn an_unmonitored_input_is_in_the_take_and_out_of_the_room() {
        for monitored in [false, true] {
            let shared = Arc::new(Shared::new());
            let (tap_tx, tap_rx) = RingBuffer::<f32>::new(1 << 16);
            let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
            // The live input, one mono channel of a steady half.
            let (mut in_tx, in_rx) = RingBuffer::<f32>::new(1 << 14);
            for _ in 0..4096 {
                let _ = in_tx.push(0.5);
            }
            r.monitor = Some(in_rx);
            r.monitor_channels = 1;
            r.monitor_widths = [1, 0, 0, 0];
            r.monitor_scratch = vec![0.0; 8192];
            // At the fader's destination rather than on its way there: the
            // slews are per sample and this test is about routing. The pick
            // feeds channel 1, an AUDIO channel by assignment.
            shared.pick_strip[0].store(1, Ordering::Relaxed);
            shared.midi_off.store(1 << 1, Ordering::Relaxed);
            shared.channel_gains[1].store(1.0f32.to_bits(), Ordering::Relaxed);
            r.channel_gain[1] = 1.0;
            r.monitor_gain[0] = 1.0;
            shared.monitor_on.store(monitored, Ordering::Relaxed);
            r.room_gain = if monitored { 1.0 } else { 0.0 };

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
            let taped = captured.iter().fold(0.0f32, |a, s| a.max(s.abs()));
            let room = out.iter().fold(0.0f32, |a, s| a.max(s.abs()));

            assert!(
                taped > 0.4,
                "the input is missing from the take (peak {taped}) with \
                 monitoring {monitored} - a take is the desk and the strip was \
                 not muted"
            );
            if monitored {
                assert!(
                    room > 0.4,
                    "monitoring is on and the speakers got nothing (peak {room})"
                );
            } else {
                assert!(
                    room < 1.0e-3,
                    "the input reached the speakers (peak {room}) with \
                     monitoring off, which is the feedback this switch exists \
                     to prevent"
                );
            }
        }
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
    fn a_channel_fader_reaches_zero_without_a_step_discontinuity() {
        // A gain that jumps is a click, which is the one thing the metronome is
        // supposed to have a monopoly on. The smoother runs whether or not the
        // channel has anything on it, so that a channel filled a minute after
        // its fader moved does not start at the gain the fader used to have
        // and jump to the one it has now.
        let shared = Arc::new(Shared::new());
        let (tap_tx, _rx) = RingBuffer::<f32>::new(1 << 16);
        let mut r = test_renderer(Arc::clone(&shared), tap_tx, 2);
        r.gain_coeff = gain_coefficient(RATE);
        r.channel_gain = [1.0; CHANNELS];
        shared.set_channel_gain(0, 0.0);
        let mut out = vec![0.0f32; 8 * 2];
        r.render(&mut out, 0, 0);
        assert!(
            r.channel_gain[0] > 0.9,
            "a 10 ms smoother must not cross most of its range in 8 frames \
             (it reached {})",
            r.channel_gain[0]
        );
        // 600 blocks of 8 frames is 4800 frames, 100 ms, ten time constants.
        for _ in 0..600 {
            r.render(&mut out, 0, 0);
        }
        assert!(
            r.channel_gain[0] < 0.01,
            "the smoother never arrived ({})",
            r.channel_gain[0]
        );
        assert_eq!(
            (r.channel_gain[1], r.channel_gain[2]),
            (1.0, 1.0),
            "one fader moved and took its neighbours with it"
        );
    }

    #[test]
    fn a_gain_written_to_a_channel_that_does_not_exist_is_ignored_rather_than_panicking() {
        // Reached from UI code, where this app's panic hook turns a panic into a
        // dialog and `exit(1)`. A menu that got its arithmetic wrong should
        // misbehave, not end the session.
        let shared = Shared::new();
        shared.set_channel_gain(CHANNELS, 0.25);
        shared.set_channel_gain(usize::MAX, 0.25);
        for cell in &shared.channel_gains {
            assert_eq!(Shared::f32_of(cell), 1.0, "a real channel was written instead");
        }
        shared.set_channel_gain(CHANNELS - 1, 0.25);
        assert_eq!(Shared::f32_of(&shared.channel_gains[CHANNELS - 1]), 0.25);
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
        // **The RING is what is asked for and the PERIOD is what is
        // reported**, and on Linux those differ by four — see
        // `BUFFER_PERIODS`. Written in terms of the constant rather than in
        // literal frames, because a test that hard-codes 512 passes on the
        // machine it was written on and fails on the one the fix is for.
        let range = |min, max| cpal::SupportedBufferSize::Range { min, max };
        let ring_of = |b: cpal::BufferSize| match b {
            cpal::BufferSize::Fixed(n) => n,
            cpal::BufferSize::Default => 0,
        };

        // Roomy device: the ring is the wanted period times the count.
        let (size, frames) = pick_buffer(&range(16, 8192), Some(256));
        assert_eq!(ring_of(size), 256 * BUFFER_PERIODS);
        assert_eq!(frames, Some(256), "the period is not what was asked for");

        // Clamped up to the minimum, and the period follows the ring.
        let (size, frames) = pick_buffer(&range(4096, 8192), Some(256));
        assert_eq!(ring_of(size), 4096);
        assert_eq!(frames, Some(4096 / BUFFER_PERIODS));

        // And down to the maximum.
        let (size, frames) = pick_buffer(&range(16, 64), Some(256));
        assert_eq!(ring_of(size), 64);
        assert_eq!(frames, Some(64 / BUFFER_PERIODS));

        // A period is never reported as zero, whatever the device says: it is
        // divided into and displayed.
        let (_, frames) = pick_buffer(&range(1, 2), Some(1));
        assert!(frames.is_some_and(|f| f >= 1));
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
        assert!(
            (0..=STRIPS).all(|st| (0..INSERTS).all(|b| engine.insert(st, b).is_none())),
            "a fresh engine has something loaded"
        );
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
                `cargo test -p ivory -- --ignored a_bays_state --nocapture`"]
    fn a_bays_state_can_be_saved_while_it_plays_and_handed_back_to_the_next_load() {
        // The engine's half of the persistence story: the instance is inside
        // the audio callback the whole time, so this exercises the second
        // reference to its processor rather than any path that stops the audio.
        // That the RESTORED SOUND is right is proved in `ivory-host`'s
        // `instance.rs`, where a rendered probe can be measured; here the claim
        // is that the bytes can be got at all while the stream runs, and that a
        // bad blob costs the preset rather than the piano.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_insert(0, 0, Some(&bundle), None).expect("load");
        assert!(engine.insert_state_error(0, 0).is_none());

        let now = engine.timebase().now();
        engine.send_midi(now, &[0x90, 60, 100]);
        let bytes = engine
            .save_insert_state(0, 0)
            .expect("a loaded bay has state to save");
        engine.send_midi(engine.timebase().now(), &[0x80, 60, 64]);
        println!("bay 0 state: {} bytes", bytes.len());
        assert!(bytes.len() > 18, "only the container came back");
        assert!(engine.is_running(), "saving state stopped the stream");
        assert!(engine.fault().is_none(), "{:?}", engine.fault());

        // An empty bay has nothing to save, and neither has one that does not
        // exist. Both are `None` rather than an error, because the caller's
        // only move either way is to write nothing.
        assert!(engine.save_insert_state(0, 1).is_none());
        assert!(engine.save_insert_state(STRIPS + 1, 0).is_none());

        engine
            .load_insert(0, 0, Some(&bundle), Some(&bytes))
            .expect("reload with state");
        assert!(
            engine.insert_state_error(0, 0).is_none(),
            "a blob this engine just wrote was refused: {:?}",
            engine.insert_state_error(0, 0)
        );

        // And the case that will actually happen: a settings file that has been
        // truncated, edited, or written by a different plugin. The instrument
        // must still load.
        let mut broken = bytes.clone();
        broken.truncate(broken.len() / 2);
        engine
            .load_insert(0, 0, Some(&bundle), Some(&broken))
            .expect("a corrupt blob must not stop the instrument loading");
        assert!(
            engine.insert_state_error(0, 0).is_some(),
            "a truncated blob was silently accepted, which is how 'it keeps \
             forgetting my piano' becomes unreportable"
        );
        assert!(engine.insert(0, 0).is_some(), "the instrument did not load");
        // ...and the error does not outlive the bay.
        let _ = engine.load_insert(0, 0, None, None);
        assert!(engine.insert_state_error(0, 0).is_none());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored a_real_instrument`"]
    fn a_real_instrument_is_heard_and_can_be_swapped_while_the_stream_runs() {
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        let loaded = engine
            .load_insert(0, 0, Some(&bundle), None)
            .expect("load")
            .expect("a real bundle in a real bay");
        assert_eq!(engine.insert(0, 0), Some(&loaded));

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
        let _ = engine.load_insert(0, 0, None, None);
        assert!(engine.insert(0, 0).is_none());
        engine.load_insert(0, 0, Some(&bundle), None).expect("reload");
        assert_eq!(tap.channels(), TAP_CHANNELS, "the tap changed width mid-take");
        assert!(engine.is_running());
        let mut out = Vec::new();
        tap.drain(&mut out);
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored two_channels --nocapture`"]
    fn two_channels_holding_the_same_instrument_are_twice_as_loud_as_one() {
        // The claim of the whole feature, measured rather than asserted: the
        // same instrument playing the same notes twice sums coherently, so the
        // peak must double. It is the only way to tell "both channels got the
        // notes" from "one got them and the other is silent", which every
        // structural test in this file would happily pass.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_insert(0, 0, Some(&bundle), None).expect("channel 1");
        for ch in 0..CHANNELS {
            engine.set_channel_gain(ch, 0.4);
        }
        let one = chord_peak(&engine);
        engine.load_insert(1, 0, Some(&bundle), None).expect("channel 2");
        let two = chord_peak(&engine);
        println!("one channel {one:.4}, two {two:.4}, ratio {:.2}x", two / one);
        assert!(one > 0.001, "the first instrument was never heard ({one})");
        // Wide bounds on purpose: a real piano's peak is a transient and the
        // two instances are separately seeded. 1.5x rules out "the second
        // channel is silent" (1.0x) and 2.5x rules out "something is summing
        // twice".
        assert!(
            (1.5..2.5).contains(&(two / one)),
            "two channels measured {:.2}x one, which is neither a layered pair \
             (2.0x) nor a channel that heard nothing (1.0x)",
            two / one
        );
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored unloading_one_channel`"]
    fn unloading_one_channel_leaves_the_others_playing() {
        // The per-bay handoff in one test: the rings channel 2's bay uses are
        // not the rings channel 1's uses, so retiring one instrument cannot
        // take the other with it.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        engine.load_insert(0, 0, Some(&bundle), None).expect("channel 1");
        engine.load_insert(1, 0, Some(&bundle), None).expect("channel 2");
        for ch in 0..CHANNELS {
            engine.set_channel_gain(ch, 0.4);
        }
        assert!(chord_peak(&engine) > 0.001);

        let _ = engine.load_insert(1, 0, None, None);
        assert!(engine.insert(1, 0).is_none());
        assert!(engine.insert(0, 0).is_some(), "the wrong bay was retired");
        let alone = chord_peak(&engine);
        assert!(
            alone > 0.001,
            "unloading channel 2 silenced channel 1 as well ({alone})"
        );
        assert!(engine.is_running());
        assert!(engine.fault().is_none(), "{:?}", engine.fault());
    }

    #[test]
    #[ignore = "needs a real VST3 instrument installed; run with \
                `cargo test -p ivory -- --ignored a_bay_that_does_not_exist`"]
    fn a_bay_that_does_not_exist_is_a_no_op_and_never_a_panic() {
        // Every one of these is reachable from a menu, and a panic here goes
        // through the app's hook to a dialog and `exit(1)`.
        let bundle = pianoteq();
        let mut engine = Engine::start(None).expect("an audio output");
        assert!(matches!(
            engine.load_insert(STRIPS + 1, 0, Some(&bundle), None),
            Ok(None)
        ));
        assert!(matches!(
            engine.load_insert(0, INSERTS, Some(&bundle), None),
            Ok(None)
        ));
        assert!(engine.open_insert_editor(STRIPS + 1, 0).is_ok());
        let _ = engine.load_insert(STRIPS + 1, 0, None, None);
        engine.close_insert_editor(STRIPS + 1, 0);
        engine.set_channel_gain(CHANNELS, 0.5);
        assert!(!engine.insert_has_editor(STRIPS + 1, 0));
        assert!(!engine.insert_editor_open(STRIPS + 1, 0));
        assert!(engine.insert(STRIPS + 1, 0).is_none());
        assert!(engine.is_running(), "a bad bay number stopped the stream");
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
