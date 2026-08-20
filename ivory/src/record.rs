//! The recording session: what happens between pressing Record and having a
//! folder full of files.
//!
//! `ivory-record` owns every piece of machinery this uses — the timebase, the
//! ring, the rate fit, the WAV writer, the SMF writer, the take directory — and
//! deliberately owns no policy. This module is the policy: when to arm, what a
//! pre-roll does, which clock a take is built on when there is no audio device
//! at all, and what the band is told while it is happening.
//!
//! It lives in the binary rather than in `ivory-record` for two reasons. It
//! needs [`crate::midi::RawMidiTap`], which is a `midir` thing and therefore a
//! desktop thing. And `cpal::Stream` is `!Send` on every platform
//! (`cpal-0.16.0/src/platform/mod.rs:755`), so the stream has to be built, held
//! and dropped on the UI thread — which makes "the thing that owns the stream"
//! the same object as "the thing the GUI talks to", and that object belongs
//! here.
//!
//! # The two threads, and why the split is where it is
//!
//! The **UI thread** owns the `cpal` stream, the MIDI tap drain, the session
//! state machine and the take directory. It touches no audio samples.
//!
//! The **writer thread** owns the ring's read end, the rate fit, the level
//! tracker and the WAV file. It runs whenever the Recorder band is open, not
//! only during a take, because **the meter has to be live before arming** —
//! that is what kills the "I recorded silence" failure class, and it is the
//! single most valuable thing in the band (RECORDER-PLAN §5).
//!
//! Nothing crosses between them except a command channel, a report channel, and
//! one `Mutex<Meters>` the UI reads at frame rate. The mutex is right and a
//! channel would be wrong: a meter wants the LATEST value, and a channel makes
//! it lag by however far behind the UI has fallen.
//!
//! # What this does not do yet
//!
//! No video. The camera preview is live but nothing encodes it, so a take
//! writes `.wav` and `.mid` and says so. The Export dialog's video rows are
//! disabled for the same reason. See `docs/RECORDER-PLAN.md` §12 steps 6-7.

use crate::midi::RawMidiTap;
use ivory_record::audio::{
    self, ClockTap, FrameCursor, InputSelection, InputStream, LevelTracker, Meters as AudioMeters,
    Timebase,
};
use ivory_record::camera;
use ivory_record::clock::{Nanos, RateFit, SourceClock, Timeline, MIDIR_SCALE_NS};
use ivory_record::smf::{Captured, MidiTake};
use ivory_record::take::{self, Manifest, Take, WallTime};
use ivory_record::wav::{Bext, SampleFormat, WavSpec, WavWriter};
use ivory_ui::recorder::{ExportSpec, Level, Meters as UiMeters, RecordState};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the MIDI clock is given to settle before its offset is trusted.
///
/// Two seconds, matching the audio path. A keyboard that has been sitting idle
/// contributes nothing until it is played, so in practice this settles on the
/// first few notes of the take rather than on the first two seconds of it.
const MIDI_SETTLE_NS: Nanos = 2_000_000_000;

/// How much MIDI history is kept behind the present.
///
/// Not for the take itself — the take starts at T0 — but for the tick-0 state
/// restatement a correct `.mid` needs: the sustain pedal that went down before
/// Record was pressed is still down, and a file that does not say so plays back
/// dry (RECORDER-PLAN §7 rule 8). Sixty seconds is far more than that needs and
/// costs a few hundred kilobytes.
const MIDI_HISTORY_NS: Nanos = 60_000_000_000;

/// The writer thread's poll interval.
///
/// Short enough that a 60 Hz meter never sees the same window twice, long
/// enough that the thread is asleep essentially all the time. The ring is sized
/// in seconds, so this is nowhere near the deadline that matters.
const POLL: Duration = Duration::from_millis(4);

/// Sample format for the take's WAV.
///
/// 24-bit fixed, not 32-bit float. It is what every DAW and every collaborator
/// expects, it is two thirds of the size, and the headroom argument for float
/// does not apply to a file that is written once from a live input and never
/// summed into anything.
const TAKE_FORMAT: SampleFormat = SampleFormat::Int24;

// ───────────────────────────────────────────────────────────────────────────
// The writer thread
// ───────────────────────────────────────────────────────────────────────────

/// What the UI thread asks the writer to do.
enum Cmd {
    /// Begin writing to this file. The writer replies with nothing; a failure
    /// to create the file arrives in the report at Stop, because there is
    /// nothing useful the UI could do about it mid-take that it will not also
    /// do at the end.
    Start(Box<StartArgs>),
    /// Close the file and send a report.
    Stop,
    /// Install or remove the monitor engine's recorder tap.
    ///
    /// Boxed and sent rather than shared: the tap is the read end of a
    /// lock-free ring and belongs to exactly one thread, which is this one.
    Plugin(Option<Box<crate::instrument::RecorderTap>>),
    /// Which sources the next take is made of.
    Source(TakeSource),
    /// Put out the clip latch on the writer's OWN tracker.
    ///
    /// **The half of "clear the clip" that was missing.** `Session::clear_clip`
    /// cleared the published `AudioMeters` — the copy the UI reads — and the
    /// writer's `pump()` copied its own still-latched `clipped[]` straight back
    /// over it on the very next cycle, roughly every 4 ms. So the lamp could
    /// not be cleared while an input was open, and not for one frame: a 60 fps
    /// capture of two clicks caught zero dark frames.
    ///
    /// A command and not a shared flag because the channel is already here and
    /// the tracker lives on THIS thread, not in the audio callback — there is
    /// nothing to make lock-free.
    ClearClip,
    Quit,
}

struct StartArgs {
    path: PathBuf,
    spec: WavSpec,
    bext: Bext,
    /// Where to copy every sample that reaches the `.wav`, for the video's
    /// audio track. `None` for a take with no video in it.
    ///
    /// **This is the sync guarantee, and it is a copy rather than a second
    /// read of the device on purpose.** Anything that re-derived the samples —
    /// a second tap, a second cursor, a re-read of the file — would be a second
    /// chance to disagree with the `.wav` about which frame is which. Sending
    /// what was just written, with the index it was written at, cannot.
    audio_tx: Option<mpsc::Sender<AudioChunk>>,
}

/// Samples on their way to the video's audio track.
///
/// The `allow(dead_code)` off macOS is not a shrug: the encoder and the
/// compositor are macOS-only, so on Windows and Linux this really is written
/// and never read. Keeping the plumbing compiled on every platform is what
/// stops it rotting between now and the day those platforms get an encoder —
/// the alternative is a second `cfg` maze through the writer.
///
///
/// Sent to the UI thread, where the encoder lives. That direction is
/// deliberate and it is a 600-fold difference in traffic: audio is 384 kB a
/// second, and composited 1080p frames are 250 MB a second — so the audio
/// crosses the thread boundary and the video never has to.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) struct AudioChunk {
    /// Index of the first frame here, counted from the start of the take —
    /// which is exactly its index in the `.wav`.
    pub first_frame: u64,
    pub samples: Vec<f32>,
}

/// Everything the UI needs from the writer once a take has stopped.
///
/// The `RateFit` is in here rather than the `Timeline` because the timeline
/// needs T0 and T1, which are the UI thread's to decide — the writer does not
/// know when the user pressed the button, only what the device did.
struct AudioReport {
    frames: u64,
    fit: RateFit,
    /// Marks that arrived with no usable device timestamp. Non-zero means the
    /// backend is not giving us a clock, and the take's report has to say so
    /// rather than presenting a fit made from a handful of points.
    unstamped: u64,
    /// Frames the ring could not hold. **The meter never saw these**, so a
    /// clean clip latch is only as trustworthy as this number is zero — which
    /// is precisely why the two are reported side by side.
    frames_dropped: u64,
    clipped_samples: u64,
    take_peak: f32,
    channels: u16,
    /// When file sample 0 happened, in the timebase. The take's real T0.
    first_frame_ns: Option<Nanos>,
    /// Whether the device was still running when the take stopped.
    running: bool,
    /// Read from the source clock, not rebuilt: the anchor, the jitter and the
    /// latency term all belong to the object that measured them.
    source: take::SourceReport,
    /// Whatever went wrong, in words meant for the user.
    error: Option<String>,
}

/// Which sources a take is made of.
///
/// Not `graph::SourceMode` (yet). That type carries the full mixer with its
/// per-source gains and delay compensation, and this is the subset that is
/// actually wired: the recorder writes ONE rate master and optionally sums a
/// second source into it. When the graph is made real this collapses into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeSource {
    /// The audio input device. What a microphone take is.
    Input,
    /// The hosted instrument. What a piano take is, and the default whenever a
    /// plugin is loaded — the plugin IS the sound the user is playing.
    Plugin,
    /// Both, summed.
    ///
    /// **The two run on independent device clocks**, so this is only exactly
    /// right when the input and the output are the same interface — which is
    /// the ordinary setup for a piano rig with one box, and the reason it is
    /// offered at all. On two separate devices the sum drifts by their relative
    /// crystal error, and `take.json` says which source was the rate master so
    /// the drift is at least attributable.
    Both,
}

impl TakeSource {
    pub fn to_setting(self) -> &'static str {
        match self {
            TakeSource::Input => "input",
            TakeSource::Plugin => "plugin",
            TakeSource::Both => "both",
        }
    }

    /// Forgiving, and it resolves the cases the setting cannot know about.
    ///
    /// **An absent setting means "record whatever there is".** That is the fix
    /// for the first thing anybody hit: load a piano, press record, and get a
    /// file with only the microphone in it — the instrument you could plainly
    /// hear was monitored but never recorded, because the stored setting said
    /// `input` and nothing had ever offered to change it. A loaded instrument
    /// is one the user went and chose; leaving it out of the take is never what
    /// they meant.
    ///
    /// **A backing track is the same argument and it took a second bug to
    /// notice.** `TakeSource::Plugin` is not really "the plugin": it is the
    /// instrument BUS, and the bus carries the instruments, the click and the
    /// backing track. This function knew about plugins and inputs only, so
    /// somebody with a microphone selected, no instrument loaded and a backing
    /// track playing got `Input` — a take of themselves playing along to
    /// something that is not in the file. The track was audible in the room, so
    /// the recording was not silent; it just had the bleed instead of the
    /// track, which is the version of this failure that survives a listen.
    ///
    /// Loaded, not playing: a take is armed before the transport rolls, and a
    /// track that starts with it would decide the sources one buffer too late.
    ///
    /// An EXPLICIT setting is still obeyed, which is what makes
    /// instrument-only and microphone-only reachable.
    pub fn resolve(
        setting: &str,
        plugin_loaded: bool,
        track_loaded: bool,
        input_open: bool,
    ) -> Self {
        // Everything downstream asks "is there anything on the instrument bus",
        // never "is there a plugin". Those were the same question until the
        // backing track arrived, and every arm below was written when they were.
        let bus = plugin_loaded || track_loaded;
        let want = match setting {
            "plugin" => TakeSource::Plugin,
            "both" => TakeSource::Both,
            "input" => TakeSource::Input,
            // Anything else, including the absent default, is "everything
            // there is".
            _ => {
                if bus {
                    TakeSource::Both
                } else {
                    TakeSource::Input
                }
            }
        };
        match (want, bus, input_open) {
            (TakeSource::Plugin, false, _) => TakeSource::Input,
            (TakeSource::Both, false, _) => TakeSource::Input,
            (TakeSource::Both, true, false) => TakeSource::Plugin,
            (TakeSource::Input, _, false) if bus => TakeSource::Plugin,
            (other, ..) => other,
        }
    }

    fn uses_plugin(self) -> bool {
        matches!(self, TakeSource::Plugin | TakeSource::Both)
    }

    fn uses_input(self) -> bool {
        matches!(self, TakeSource::Input | TakeSource::Both)
    }
}

/// Scale an interleaved block towards `target`, and say where the gain got to.
///
/// **Per FRAME, not per sample.** Stepping the pole once per sample across an
/// interleaved buffer applies a slightly different gain to the left and the
/// right of the same frame — which is a stereo image that swings while the
/// fader moves. Every channel of a frame gets the same number.
///
/// Pulled out of `Writer::apply_input_gain` so the two things that are easy to
/// get wrong — that, and stepping rather than sliding — can be asserted without
/// a capture device in the room.
fn walk_gain(buf: &mut [f32], channels: usize, target: f32, mut now: f32, coeff: f32) -> f32 {
    let ch = channels.max(1);
    for frame in buf.chunks_exact_mut(ch) {
        now += (target - now) * coeff;
        for s in frame.iter_mut() {
            *s *= now;
        }
    }
    now
}

/// How fast a fader move reaches the samples: one pole, 10 ms.
///
/// The same shape and time constant `instrument.rs` uses for its own gains, so
/// the microphone fader and the instrument faders feel like the same control.
/// Long enough to be inaudible, short enough that nobody notices a lag.
fn gain_slew_coefficient(rate: f64) -> f32 {
    if !(rate.is_finite() && rate > 0.0) {
        return 1.0;
    }
    (1.0 - (-1.0 / (rate * 0.010)).exp()) as f32
}

/// The writer thread's own state.
struct Writer {
    sink: audio::CaptureSink,
    /// The fader's target, written by the UI thread. See `Session::input_gain`.
    input_gain: Arc<AtomicU32>,
    /// Where the gain has actually got to. **Slewed, not stepped**: this thread
    /// works in blocks of a few milliseconds, and applying a fader's new value
    /// to a whole block is a step discontinuity — a click on every frame of a
    /// drag. One pole per sample, the same shape and time constant the engine's
    /// own gains use.
    input_gain_now: f32,
    input_gain_coeff: f32,
    tracker: LevelTracker,
    clock: ClockTap,
    cursor: FrameCursor,
    meters: Arc<Mutex<AudioMeters>>,
    wav: Option<WavWriter>,
    error: Option<String>,
    buf: Vec<f32>,
    /// Scratch for the plugin's audio, kept so the steady state allocates
    /// nothing.
    plugin_buf: Vec<f32>,
    plugin: Option<crate::instrument::RecorderTap>,
    source: TakeSource,
    silence: Vec<f32>,
    /// Timebase instant of the first frame written to the current file.
    /// `None` until the take's first mark arrives. See `pump`.
    first_frame_ns: Option<Nanos>,
    /// Frames the ring could not supply against a mark that promised them.
    /// Padded, and counted here so the padding is never silent.
    short_frames: u64,
    /// Counter readings at the moment this take was armed.
    ///
    /// `CaptureStats` counts since the DEVICE opened, not since the take
    /// started — the band opens the input, and a user may record ten takes
    /// through one stream. Reporting the raw counter makes take two inherit
    /// take one's dropouts, and print "48 frames were lost" for a take that
    /// lost nothing. Subtracting a baseline is preferred to `stats().reset()`
    /// because the callback writes those counters concurrently and the device
    /// state must survive being re-armed.
    dropped_at_arm: u64,
    unstamped_at_arm: u64,
    /// The instrument ring's own loss counter at arm. Same reasoning as
    /// `dropped_at_arm`, and it needs its own baseline because the two rings
    /// fill for completely different reasons.
    plugin_dropped_at_arm: u64,
    /// See [`StartArgs::audio_tx`]. Cleared at Stop, so a take with no video
    /// does not go on copying samples to a receiver nobody is draining.
    audio_tx: Option<mpsc::Sender<AudioChunk>>,
}

impl Writer {
    /// One pass: absorb whatever the device has produced since the last one.
    ///
    /// The idle and recording paths differ, and the difference is not an
    /// optimisation. Idle drains everything available and only reads marks for
    /// the clock anchor. Recording drives the drain FROM the marks through
    /// [`FrameCursor`], so that a dropout becomes padded silence at the right
    /// place in the file rather than a gap that slides every note after it
    /// earlier — which is the exact drift this whole feature exists to prevent.
    fn pump(&mut self) {
        if self.wav.is_some() {
            while let Some(mark) = self.sink.next_mark() {
                self.clock.observe(&mark);
                // WHEN file sample 0 actually happened, recorded from the first
                // mark of the take rather than assumed to be `T_arm`.
                //
                // RECORDER-PLAN §3: `T0 = max(T_audio_sample_0, T_arm)`. The
                // first draft used `T_arm` alone, which is wrong by up to a
                // buffer period in whichever direction the poll happened to
                // land — a fixed, per-take MIDI-versus-audio offset of several
                // milliseconds, which is exactly the error this feature exists
                // to remove.
                if self.first_frame_ns.is_none() {
                    self.first_frame_ns = Some(
                        mark.device_ns
                            .and_then(|s| self.clock.source().to_timebase(s))
                            .unwrap_or(mark.host_ns),
                    );
                }
                let plan = self.cursor.plan(&mark);
                self.buf.clear();
                let got = if plan.frames > 0 {
                    self.sink.pop_frames(plan.frames as usize, &mut self.buf)
                } else {
                    0
                };
                // Recording the instrument and not the room: the input's
                // samples are still POPPED (the ring has to keep moving) and
                // its marks still drive the clock and the rate fit, but its
                // audio does not reach the file.
                //
                // **The input device stays the clock even when it is not the
                // content.** It is the only source with device timestamps, so
                // it is the only one that can measure the crystal — and a take
                // of a plugin is still a take that has to not drift.
                if !self.source.uses_input() {
                    self.buf.iter_mut().for_each(|s| *s = 0.0);
                }
                // **The fader, before anything else touches the block.**
                //
                // Here and not later, because everything downstream has to see
                // the same samples: the meter the user is watching, the file
                // being written, and the video's audio track. A gain applied
                // after the tracker would be a fader that changes the recording
                // and not the meter, which is the confusing half of having no
                // fader at all.
                //
                // After the zeroing above rather than before it: with the input
                // not part of the take there is nothing to scale, and slewing
                // towards a target through silence would leave the gain
                // somewhere arbitrary when the input came back.
                self.apply_input_gain();
                // The instrument, summed into the same block.
                //
                // The INPUT is the rate master here and the plugin follows it:
                // the file's length is decided by the device whose timestamps
                // built the timeline, and the plugin is fitted to it. When the
                // two are the same interface — the ordinary one-box piano rig —
                // they share a crystal and this is exact. On two separate
                // devices they drift, which is why `take.json` records which
                // source was the master.
                let frames = self.buf.len() / self.tracker.channels().max(1);
                if self.source.uses_plugin() {
                    self.mix_plugin(frames);
                } else {
                    // Drained and thrown away, NOT left alone.
                    //
                    // Leaving it filled a ring that nothing was reading, and
                    // every frame it then refused counted as a take loss: a
                    // 37-second take reported "1,608,192 frames were lost to
                    // the system and padded with silence" — 33 seconds of a
                    // 37-second recording — when in truth nothing was lost at
                    // all. The instrument simply was not part of the take.
                    self.discard_plugin(frames);
                }
                self.tracker.absorb(&self.buf);
                // The ring can hold fewer frames than the mark promised — the
                // idle path drains marks and samples in two statements, so a
                // callback landing between them leaves an orphan mark whose
                // samples are already gone. Discarding the shortfall would
                // break `file sample N == device frame N` permanently and
                // silently; padding it keeps the invariant and counts the loss.
                let short = plan.frames as u64 - got as u64;
                self.short_frames += short;
                self.write_plan(plan.silence_before, plan.silence_after + short as u32);
            }
        } else {
            while let Some(mark) = self.sink.next_mark() {
                self.clock.observe(&mark);
            }
            self.buf.clear();
            self.sink.drain_samples(&mut self.buf);
            // The same gain between takes, so the meter reads what the fader
            // says while nothing is recording — which is when anybody actually
            // sets it.
            self.apply_input_gain();
            self.tracker.absorb(&self.buf);
            // Keep the instrument's ring moving while idle, and throw the
            // audio away.
            //
            // Nothing drained it between takes, so it filled within a few
            // seconds of the engine starting and then counted every frame as
            // dropped for the rest of the session — 440,832 of them in one
            // probe run. The next take would then report "440832 frames were
            // lost to the system", which is both alarming and false.
            //
            // Draining also means a take starts with the audio being played
            // NOW rather than with whatever was in the ring when it filled up.
            if let Some(tap) = self.plugin.as_mut() {
                self.plugin_buf.clear();
                tap.drain(&mut self.plugin_buf);
                self.plugin_buf.clear();
            }
        }
        if let Ok(mut m) = self.meters.lock() {
            self.tracker.publish(&mut m);
        }
    }

    /// Write silence, then the samples, then more silence.
    ///
    /// The first error is kept and the rest are swallowed: a disk that filled
    /// up produces one error a second for the rest of the take otherwise, and
    /// the first one is the only one that says anything.
    /// Written out plainly as three steps rather than as a loop over
    /// `[before, after]` with the samples emitted inside it.
    ///
    /// That loop was the first version and it wrote the buffer **twice**
    /// whenever `before == after`, which is every ordinary block, because both
    /// are zero and `if pad == before` is then true on both iterations. The
    /// result was a file exactly twice as long as the take, with every sample
    /// duplicated — and nothing in the unit tests noticed, because they all
    /// exercise the dropout path where the two differ. It was caught by
    /// `--record-test` against a real device: 292,864 frames reported for a
    /// 3.03-second take that should hold 145,632.
    fn write_plan(&mut self, before: u64, after: u32) {
        let channels = self.tracker.channels() as u64;
        self.write_silence(before.saturating_mul(channels));
        let samples = std::mem::take(&mut self.buf);
        self.write_samples(&samples);
        self.buf = samples;
        self.write_silence(u64::from(after).saturating_mul(channels));
    }

    /// Throw away everything the ring is holding, and the marks that describe
    /// it, without writing any of it.
    fn discard_ring(&mut self) {
        while let Some(mark) = self.sink.next_mark() {
            self.clock.observe(&mark);
        }
        self.buf.clear();
        self.sink.drain_samples(&mut self.buf);
        self.buf.clear();
    }

    /// Sum `frames` of instrument audio into `self.buf`, in place.
    ///
    /// Channel counts are reconciled here rather than upstream because they can
    /// differ and both are outside our control: a mono interface with a stereo
    /// piano is an ordinary setup. A mono source is sent to every output
    /// channel; a wider source is folded down to the channels there are, which
    /// keeps a stereo piano audible on a mono take instead of silently dropping
    /// its right hand.
    /// Walk the block applying the microphone fader, one pole per sample.
    ///
    /// Interleaved, so the coefficient is applied per FRAME and every channel
    /// of that frame gets the same gain — stepping per sample across an
    /// interleaved buffer would pan the signal while the fader moved.
    fn apply_input_gain(&mut self) {
        let target = f32::from_bits(self.input_gain.load(Ordering::Relaxed));
        let target = if target.is_finite() && target >= 0.0 {
            target
        } else {
            1.0
        };
        // Nothing to do, and worth checking: unity is where this sits for
        // everybody who has never touched the fader, and the walk below is per
        // sample on the writer thread.
        if (target - 1.0).abs() < 1.0e-6 && (self.input_gain_now - 1.0).abs() < 1.0e-6 {
            return;
        }
        let ch = self.tracker.channels().max(1);
        self.input_gain_now = walk_gain(
            &mut self.buf,
            ch,
            target,
            self.input_gain_now,
            self.input_gain_coeff,
        );
    }

    fn mix_plugin(&mut self, frames: usize) {
        let Some(tap) = self.plugin.as_mut() else {
            return;
        };
        let out_ch = self.tracker.channels().max(1);
        let in_ch = tap.channels().max(1);
        self.plugin_buf.clear();
        let got = tap.drain_frames(frames, &mut self.plugin_buf);
        // Short is not an error: the output device is free-running and may not
        // have produced this block yet. The missing frames stay silent rather
        // than shifting everything after them, which is the same rule the input
        // path's dropout padding follows.
        for f in 0..got.min(frames) {
            for c in 0..out_ch {
                let src = if in_ch == 1 {
                    self.plugin_buf[f * in_ch]
                } else if c < in_ch {
                    self.plugin_buf[f * in_ch + c]
                } else {
                    // More output channels than the plugin has: repeat the last
                    // one rather than leaving silence in it.
                    self.plugin_buf[f * in_ch + in_ch - 1]
                };
                let dst = f * out_ch + c;
                if dst < self.buf.len() {
                    self.buf[dst] += src;
                }
            }
        }
    }

    /// Move `frames` of instrument audio out of the ring and drop it.
    ///
    /// The same amount `mix_plugin` would have taken, so the ring keeps pace
    /// with the take instead of backing up.
    fn discard_plugin(&mut self, frames: usize) {
        let Some(tap) = self.plugin.as_mut() else {
            return;
        };
        self.plugin_buf.clear();
        tap.drain_frames(frames, &mut self.plugin_buf);
        self.plugin_buf.clear();
    }

    fn write_samples(&mut self, samples: &[f32]) {
        let Some(wav) = self.wav.as_mut() else {
            return;
        };
        // The index BEFORE the write, which is the index of the first frame in
        // `samples`. Read here rather than tracked separately so that the
        // encoder's idea of "frame N" is the wav's own, not a parallel count
        // that could drift from it.
        let first_frame = wav.frames();
        if let Err(e) = wav.write_interleaved(samples) {
            if self.error.is_none() {
                self.error = Some(format!("could not write the audio file: {e}"));
            }
        }
        // **Every sample the file gets, the video gets, at the same index** —
        // including the silence a dropout is padded with, because this is the
        // single funnel both go through. That is the entire reason the video's
        // audio cannot drift from the `.wav`: there is no second path for it to
        // drift along.
        if let Some(tx) = self.audio_tx.as_ref() {
            // A fresh Vec per block, and measured rather than assumed: a block
            // is a poll interval of audio, so this is one small allocation
            // every few milliseconds on a thread that is already writing to a
            // file. Recycling buffers through a return channel was written and
            // then removed — it was more machinery than the thing it saved.
            let buf = samples.to_vec();
            // A send that fails means the UI thread has stopped listening,
            // which happens at Stop and on the frame a video take is abandoned.
            // Not an error: the `.wav` is unaffected and it is the master.
            let _ = tx.send(AudioChunk {
                first_frame,
                samples: buf,
            });
        }
    }

    /// `count` SAMPLES of silence, in chunks of the reusable zero buffer.
    ///
    /// Chunked rather than allocated: a dropout can be seconds long, and
    /// allocating a pad that size would allocate megabytes on the writer thread
    /// at the exact moment the machine is already struggling.
    fn write_silence(&mut self, mut count: u64) {
        // A zero-length scratch would make `count.min(0)` zero forever: an
        // unbreakable loop on the writer thread, which would then make
        // `Drop for Audio`'s join() hang the app on quit. Only reachable via a
        // device claiming zero channels, which `Audio::open` now refuses — this
        // is the second lock on the same door.
        if self.silence.is_empty() {
            return;
        }
        while count > 0 {
            let n = count.min(self.silence.len() as u64) as usize;
            let chunk = std::mem::take(&mut self.silence);
            self.write_samples(&chunk[..n]);
            self.silence = chunk;
            count -= n as u64;
        }
    }

    fn stop(&mut self) -> AudioReport {
        // One last pass, so the frames the device produced between the user
        // pressing Stop and this thread noticing are in the file rather than
        // discarded. Without it every take is short by up to a poll interval.
        self.pump();
        let mut frames = 0;
        if let Some(wav) = self.wav.take() {
            frames = wav.frames();
            if let Err(e) = wav.finish() {
                if self.error.is_none() {
                    self.error = Some(format!("could not close the audio file: {e}"));
                }
            }
        }
        let mut m = AudioMeters::new(self.tracker.channels());
        self.tracker.publish(&mut m);
        AudioReport {
            frames,
            fit: self.clock.fit().clone(),
            // Per-take, by subtracting the reading taken at arm. The fit itself
            // is deliberately NOT reset: the stream is continuous and the
            // crystal is the same one, so a longer fit is strictly a better
            // measurement of it. It is the counters that would lie, not the fit.
            unstamped: self.clock.unstamped().saturating_sub(self.unstamped_at_arm),
            // The instrument ring's losses count ONLY when the instrument is
            // part of the take. Counting them regardless is what produced the
            // false "1,608,192 frames were lost" on a take that lost nothing:
            // the ring was not being read because the instrument was not being
            // recorded, which is not a dropout, it is a decision.
            frames_dropped: self
                .sink
                .stats()
                .frames_dropped()
                .saturating_sub(self.dropped_at_arm)
                + self.short_frames
                + if self.source.uses_plugin() {
                    let armed = self.plugin_dropped_at_arm;
                    self.plugin.as_ref().map_or(0, |t| t.dropped().saturating_sub(armed))
                } else {
                    // An instrument that is not in the take cannot cost the take
                    // anything. Its ring is drained and discarded, so whatever it
                    // "dropped" is audio nobody asked to keep.
                    0
                },
            // Taken, not copied. It is reset on `Cmd::Start`, and a take that
            // writes no `.wav` at all never sends one — so without this the
            // NEXT report would carry the previous take's first frame. Today
            // `t0.max(first)` makes a stale value harmless, because a stale one
            // is always in the past and always loses; that is a property of the
            // consumer, not of this, and it is not one to rely on.
            first_frame_ns: self.first_frame_ns.take(),
            running: self.sink.stats().device_state().is_running(),
            clipped_samples: m.clipped_samples,
            take_peak: m.loudest_take_peak(),
            channels: self.tracker.channels() as u16,
            source: take::SourceReport::from_clock("audio input", self.clock.source())
                .with_fit(self.clock.fit()),
            error: self.error.take(),
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The audio half, as the session sees it
// ───────────────────────────────────────────────────────────────────────────

/// An open input device plus the thread draining it.
struct Audio {
    /// `!Send`; this is why the whole struct lives on the UI thread.
    _stream: InputStream,
    device_name: String,
    channels: u16,
    sample_rate: u32,
    /// Frames per callback, when the device accepted a fixed size. `None` is
    /// cpal's `BufferSize::Default`: the device chose and did not say what.
    buffer_frames: Option<u32>,
    cmds: mpsc::Sender<Cmd>,
    reports: mpsc::Receiver<AudioReport>,
    meters: Arc<Mutex<AudioMeters>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// The live tap, until the engine takes it. See `Session::take_monitor`.
    monitor: Option<(rtrb::Consumer<f32>, u16)>,
}

impl Audio {
    fn open(
        selection: &InputSelection,
        channel: Option<audio::ChannelPick>,
        timebase: Timebase,
        tap_home: mpsc::Sender<Box<crate::instrument::RecorderTap>>,
        buffer_frames: Option<u32>,
        sample_rate: Option<u32>,
        input_gain: Arc<AtomicU32>,
    ) -> Result<Self, String> {
        // The user's buffer choice, or the device's own. Both streams get the
        // same one: they are two halves of one path, and a round-trip figure
        // made of a 64-frame input and a 1024-frame output is a number nobody
        // can act on. The rate is the same bargain for the same reason — two
        // devices at two rates make a take that drifts.
        let wish = audio::ConfigWish {
            buffer_frames,
            sample_rate,
            // NOT `channels`. Asking for one channel would take the FIRST one;
            // this opens everything the device has and keeps the one that was
            // asked for. See `ConfigWish::pick_channel`.
            pick_channel: channel,
            ..audio::ConfigWish::default()
        };
        // Three seconds of ring. The writer thread wakes every 4 ms, so this is
        // absurd headroom — and that is the point: the one thing that must
        // never happen is the ring filling because the machine hiccuped while
        // somebody was recording a take they cannot play again.
        let (stream, sink, monitor) =
            audio::open_input(selection, &wish, 3.0, timebase).map_err(|e| e.to_string())?;
        let config = stream.config().clone();
        let channels = config.channels as usize;
        // `capture_channel` asserts this, and an assert on the UI thread goes
        // through the panic hook to a dialog and exit(1) — a device claiming
        // zero channels would kill the app the moment the band opened.
        if channels == 0 {
            return Err(format!(
                "the input '{}' reports zero channels and cannot be recorded",
                config.device
            ));
        }
        // **What the device actually gave us, in one line.**
        //
        // What was ASKED for is nothing like what arrives: on Linux `default`
        // is plug-routed, so PipeWire will happily hand back a 2-channel
        // 44.1 kHz stream for a genuinely mono 48 kHz source and neither the
        // band nor the log said so. A field investigation into a clip lamp
        // spent half its length establishing this line by inspecting the graph
        // from outside the app. It costs one `debug!` and it pays for itself in
        // the first bug report that mentions channels or rate.
        log::debug!(
            "input open: {} - {} ch, {} Hz, {:?}, buffer {}",
            config.device,
            config.channels,
            config.sample_rate,
            config.sample_format,
            match config.buffer_size {
                cpal::BufferSize::Fixed(f) => f.to_string(),
                cpal::BufferSize::Default => "device default".to_owned(),
            }
        );
        let meters = Arc::new(Mutex::new(AudioMeters::new(channels)));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));

        // The clip latch is held off for the first few milliseconds, because a
        // stream that has just opened has a converter chain warming up under
        // it and one garbage buffer is enough to latch a clip on a source that
        // was silent. See `audio::CLIP_WARMUP_MS`.
        let mut tracker = LevelTracker::new(channels, f64::from(config.sample_rate));
        tracker.warm_up(audio::CLIP_WARMUP_MS);
        let mut writer = Writer {
            sink,
            input_gain: Arc::clone(&input_gain),
            // Starts AT the target rather than at unity: the slew exists to
            // stop a moving fader clicking, and a device opening at whatever
            // the fader already says is not a move.
            input_gain_now: f32::from_bits(input_gain.load(Ordering::Relaxed)),
            input_gain_coeff: gain_slew_coefficient(f64::from(config.sample_rate)),
            tracker,
            clock: ClockTap::new(2_000_000_000),
            cursor: FrameCursor::new(),
            meters: Arc::clone(&meters),
            wav: None,
            error: None,
            plugin_buf: Vec::new(),
            plugin: None,
            source: TakeSource::Input,
            first_frame_ns: None,
            short_frames: 0,
            dropped_at_arm: 0,
            unstamped_at_arm: 0,
            plugin_dropped_at_arm: 0,
            audio_tx: None,
            buf: Vec::with_capacity(channels * 4096),
            // One block of zeros, reused. Allocating a pad the size of the
            // dropout would allocate megabytes on the worst possible thread at
            // the worst possible moment.
            silence: vec![0.0; channels * 1024],
        };
        let alive = Arc::clone(&running);
        let thread = std::thread::Builder::new()
            .name("tangent-writer".into())
            .spawn(move || {
                while alive.load(Ordering::Relaxed) {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::Start(args)) => {
                            // Drain FIRST. The ring holds everything captured
                            // since the last idle poll — up to a poll interval
                            // of audio from BEFORE Record was pressed — and
                            // `FrameCursor::new()` deliberately never pads for
                            // a stream that started earlier, so without this
                            // the file opens with pre-T0 audio and every note
                            // in the .mid lands late against it.
                            writer.discard_ring();
                            writer.cursor = FrameCursor::new();
                            writer.error = None;
                            writer.first_frame_ns = None;
                            writer.short_frames = 0;
                            writer.dropped_at_arm = writer.sink.stats().frames_dropped();
                            // `arm()`, not just a baseline: the instrument's
                            // ring has been filling since the device opened —
                            // right through the five-second plugin warm-up —
                            // so it starts a take holding audio from before
                            // anybody played anything. Arming discards that AND
                            // resets the counter, so the take begins with the
                            // note being played now.
                            if let Some(t) = writer.plugin.as_mut() {
                                t.arm();
                            }
                            writer.plugin_dropped_at_arm =
                                writer.plugin.as_ref().map_or(0, |t| t.dropped());
                            writer.unstamped_at_arm = writer.clock.unstamped();
                            writer.tracker.arm();
                            writer.audio_tx = args.audio_tx;
                            match WavWriter::create(&args.path, args.spec, &args.bext) {
                                Ok(w) => writer.wav = Some(w),
                                Err(e) => {
                                    writer.error =
                                        Some(format!("could not create the audio file: {e}"));
                                }
                            }
                        }
                        Ok(Cmd::Stop) => {
                            let report = writer.stop();
                            // Dropped AFTER the final pump, so the last block —
                            // the one `stop` flushes — reaches the video too.
                            // Dropping it before would leave the video's audio
                            // a poll interval short of the `.wav`'s.
                            writer.audio_tx = None;
                            let _ = report_tx.send(report);
                        }
                        Ok(Cmd::Plugin(tap)) => writer.plugin = tap.map(|t| *t),
                        Ok(Cmd::Source(mode)) => writer.source = mode,
                        // `clear_clip`, never `arm`: arming also forgets the
                        // take peak and the frame count, which are the take's
                        // history. Acknowledging a red light must not erase the
                        // evidence that the take was silent.
                        Ok(Cmd::ClearClip) => writer.tracker.clear_clip(),
                        Ok(Cmd::Quit) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    writer.pump();
                    std::thread::sleep(POLL);
                }
                // A take still open when the app quits is finished rather than
                // abandoned: the header is patched, the file plays, and the
                // recording someone left running survives being closed.
                if writer.wav.is_some() {
                    let _ = writer.stop();
                }
                // Hand the instrument tap back on the way out, so it can be
                // given to whichever writer runs next. See `Session::tap_tx`.
                if let Some(t) = writer.plugin.take() {
                    let _ = tap_home.send(Box::new(t));
                }
            })
            .map_err(|e| format!("could not start the recording thread: {e}"))?;

        Ok(Self {
            _stream: stream,
            device_name: config.device.clone(),
            channels: config.channels,
            sample_rate: config.sample_rate,
            buffer_frames: match config.buffer_size {
                cpal::BufferSize::Fixed(n) => Some(n),
                cpal::BufferSize::Default => None,
            },
            cmds: cmd_tx,
            reports: report_rx,
            meters,
            running,
            thread: Some(thread),
            monitor: monitor.map(|m| (m, config.channels)),
        })
    }

    fn spec(&self) -> WavSpec {
        WavSpec {
            sample_rate: self.sample_rate,
            channels: self.channels,
            format: TAKE_FORMAT,
        }
    }

    fn levels(&self) -> UiMeters {
        levels_of(&self.meters)
    }
}

/// One meter reading, shared by both writers.
///
/// A function rather than a method on each, so the band cannot end up showing
/// the instrument's level in a different SHAPE from the input's — mono/stereo,
/// hold, and the clip latch are all decisions, and two copies of them would
/// drift.
fn levels_of(meters: &Arc<Mutex<AudioMeters>>) -> UiMeters {
    let Ok(m) = meters.lock() else {
        return UiMeters::SILENT;
    };
    let at = |c: usize| Level {
        peak: m.peak.get(c).copied().unwrap_or(0.0),
        rms: m.rms.get(c).copied().unwrap_or(0.0),
        hold: m.hold.get(c).copied().unwrap_or(0.0),
    };
    UiMeters {
        left: at(0),
        right: at(if m.channels > 1 { 1 } else { 0 }),
        mono: m.channels <= 1,
        clipped: m.any_clipped(),
    }
}

impl Drop for Audio {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.cmds.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            // Joined, not detached. The thread may be holding a half-written
            // WAV whose header has not been patched, and a detached thread
            // racing process exit is how that file stays broken.
            let _ = t.join();
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The session
// ───────────────────────────────────────────────────────────────────────────

/// What a finished take produced, for the message under the transport.
pub struct Summary {
    pub folder: String,
    pub seconds: f64,
    pub wrote_audio: bool,
    pub wrote_midi: bool,
    pub clipped: bool,
    pub silent: bool,
    /// The take did not happen, or did not finish. Replaces the whole message.
    pub problem: Option<String>,
    /// The take worked, but something is worth knowing. Appended to it.
    ///
    /// Separate from `problem` because dropped frames are not a failure — the
    /// file is complete and sample-accurate — and reporting them as one would
    /// both hide the folder name and, in the manifest, send the next launch
    /// into crash recovery for a take that is fine.
    pub note: Option<String>,
}

impl Summary {
    /// One line, and it has to survive being the only thing the user reads.
    /// Whether this take hit a real problem, as opposed to simply finishing.
    ///
    /// What makes the report unsuppressable: "the disk filled" is not a
    /// message anybody meant to stop seeing. A silent take counts — it is the
    /// failure the meter exists to prevent and the one nobody notices until
    /// they open the file.
    pub fn is_problem(&self) -> bool {
        self.problem.is_some() || self.silent
    }

    pub fn message(&self) -> String {
        if let Some(p) = &self.problem {
            // The folder still comes first when there IS one. A disk that
            // filled at 6:00 of a 10:00 take leaves six perfectly good minutes
            // on disk, and reporting only "No space left on device" sends the
            // user looking for a take they think was lost.
            return if self.folder.is_empty() {
                p.clone()
            } else {
                format!("{p}  -  what was recorded is in {}", self.folder)
            };
        }
        let mins = ivory_ui::recorder::timecode(self.seconds);
        let what = match (self.wrote_audio, self.wrote_midi) {
            (true, true) => "audio + MIDI",
            (true, false) => "audio",
            (false, true) => "MIDI",
            (false, false) => "nothing",
        };
        let mut s = format!("Recorded {mins} of {what} to {}", self.folder);
        if self.silent {
            s.push_str("  -  WARNING: the audio is silent");
        } else if self.clipped {
            s.push_str("  -  the audio clipped");
        }
        if let Some(n) = &self.note {
            s.push_str("  -  ");
            s.push_str(n);
        }
        s
    }
}

/// The recorder, as the app holds it.
pub struct Session {
    timebase: Timebase,
    tap: Arc<RawMidiTap>,
    /// **The microphone fader.** `f32` bits, shared with whatever writer thread
    /// is currently draining the input.
    ///
    /// It lives on the Session rather than on [`Audio`] because it outlives the
    /// device: changing the input rebuilds `Audio`, and a fader that reset to
    /// unity every time somebody picked a different microphone would be a
    /// setting that quietly stops being true.
    ///
    /// It had no consumer at all until 4.20.0. The fader was drawn, dragged and
    /// persisted, and nothing in `ivory/src` ever read it — the owner reported
    /// it as "moving it changes neither the VUs nor the recorded level", which
    /// was exactly right.
    input_gain: Arc<AtomicU32>,
    audio: Option<Audio>,
    /// Why the audio device is not open, if it is not.
    audio_error: Option<String>,
    /// The open camera. `!Send`, like the audio stream and for the same
    /// reason — `AVCaptureSession` and its delegate are tied to the thread
    /// that built them — which is what keeps this whole struct on the UI
    /// thread.
    camera: Option<camera::CameraStream>,
    camera_error: Option<String>,
    midi_clock: SourceClock,
    midi: MidiTake,
    state: RecordState,
    /// Timebase instant of the moment the take started writing.
    t0: Nanos,
    /// Wall-clock at the same moment, for the folder name and the BWF chunk.
    started_at: WallTime,
    started_instant: Option<Instant>,
    /// When the count-in started, and how many beats it runs for.
    count_in_from: Option<Instant>,
    count_in_of: u32,
    take: Option<Take>,
    /// The spec the RUNNING take was started with.
    ///
    /// Frozen at T0 rather than read at Stop, because the Export dialog can be
    /// opened mid-take and a take that changed what it was writing halfway
    /// through would produce a directory matching neither answer.
    spec: ExportSpec,
    /// What the take is made of. Mirrored to the writer thread by
    /// [`set_source`](Session::set_source).
    source: TakeSource,
    /// Something noticed at `begin` that the summary at `stop` should say.
    pending_note: Option<String>,
    /// A T0 supplied from outside; see [`arm_at`](Session::arm_at).
    arm_override: Option<Nanos>,
    last: Option<Summary>,
    /// Latched across the take, because the meter's own latch is reset when the
    /// next one is armed and the user may not have looked yet.
    clipped: bool,
    /// The take's time signature, for the `.mid`'s tempo map.
    meter: ivory_ui::recorder::TimeSignature,
    /// Where a writer thread puts the instrument tap when it shuts down.
    ///
    /// **The tap is a single object that has to move between two writers.** It
    /// is the read end of a lock-free ring, so exactly one thread may hold it,
    /// and which thread that is depends on whether an input device is open —
    /// switching the input to "None" mid-session has to carry it from one
    /// writer to the other. Without this the tap simply died with the thread
    /// that owned it, and the instrument became unrecordable until the plugin
    /// was reloaded.
    tap_tx: mpsc::Sender<Box<crate::instrument::RecorderTap>>,
    tap_rx: mpsc::Receiver<Box<crate::instrument::RecorderTap>>,
    /// The instrument's own writer, for takes with no input device.
    ///
    /// Mutually exclusive with `audio` in practice: when an input is open it
    /// drives the take and mixes the instrument in, and when there is none this
    /// records the instrument alone off the output device's clock.
    plugin_audio: Option<PluginAudio>,
    /// The take's audio on its way to the video encoder. See
    /// [`StartArgs::audio_tx`] for why it goes THIS way across the threads.
    audio_for_video: Option<mpsc::Receiver<AudioChunk>>,
    /// The rate and channel count the writer is actually using, which the
    /// encoder needs to describe its input and which the UI thread cannot
    /// assume — the device decides it, not the settings.
    video_audio_spec: Option<(u32, u16)>,
    /// The last take's manifest, kept in memory after `stop` wrote it.
    ///
    /// The video is the one report that cannot be in the manifest at Stop:
    /// the encoder is the app's, not the session's, and it finishes AFTER
    /// `stop` returns. Keeping the written manifest is what lets
    /// [`record_video`](Session::record_video) fold the video in without
    /// parsing `take.json` back off disk — this crate writes JSON and
    /// deliberately does not read it.
    finished: Option<(std::path::PathBuf, Manifest)>,
}

impl Session {
    /// A session with no device open. Opening one is [`open_input`], which the
    /// app calls when the band appears rather than when Record is pressed.
    ///
    /// [`open_input`]: Session::open_input
    pub fn new(tap: Arc<RawMidiTap>, timebase: Timebase) -> Self {
        let (tap_tx, tap_rx) = mpsc::channel();
        Self {
            input_gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            timebase,
            tap,
            audio: None,
            audio_error: None,
            camera: None,
            camera_error: None,
            midi_clock: SourceClock::new(MIDIR_SCALE_NS, MIDI_SETTLE_NS),
            midi: MidiTake::new(),
            state: RecordState::Idle,
            meter: ivory_ui::recorder::TimeSignature::default(),
            tap_tx,
            tap_rx,
            plugin_audio: None,
            audio_for_video: None,
            video_audio_spec: None,
            t0: 0,
            started_at: WallTime::now_utc(),
            started_instant: None,
            count_in_from: None,
            count_in_of: 0,
            take: None,
            spec: ExportSpec::default(),
            source: TakeSource::Input,
            pending_note: None,
            arm_override: None,
            last: None,
            clipped: false,
            finished: None,
        }
    }

    /// The app's single time origin, shared with the monitor engine.
    ///
    /// One `Timebase` across the MIDI tap, the input stream and the output
    /// stream. Two would put the click, the notes and the recording in
    /// different worlds and every take would carry an offset nobody could
    /// account for.
    pub fn timebase(&self) -> Timebase {
        self.timebase
    }

    pub fn state(&self) -> RecordState {
        self.state
    }

    pub fn is_recording(&self) -> bool {
        self.state.is_active()
    }

    pub fn last_summary(&self) -> Option<&Summary> {
        self.last.as_ref()
    }

    /// Put out every clip latch the band can show, because the user just said
    /// they have seen it.
    ///
    /// **All of them, or the light does not go out.** The indicator is an OR
    /// across latches that arrive by different paths — the live input tracker,
    /// the instrument bus's own, and the take summary — and clearing all but
    /// one is a dismiss button that appears not to work.
    /// The engine's is cleared by the caller, which is the only one holding it.
    ///
    /// **Each of them is TWO latches, and that is the bug this comment used to
    /// describe wrongly.** The `AudioMeters` behind these mutexes are a
    /// published COPY. The original lives in the `LevelTracker` on the writer
    /// thread, and `Writer::pump` copies it over the published one every cycle
    /// — so clearing the copies alone was undone within about 4 ms, forever,
    /// for as long as the stream stayed open. The symptom was precise and
    /// bizarre: the lamp could be cleared only while NO input was selected,
    /// because unselecting one destroys the thread that owns the latch.
    ///
    /// So the copies are cleared here — that is what makes the lamp go dark on
    /// this frame rather than the next poll — and the command clears the
    /// sources, so it stays dark.
    /// Hand the live-monitor tap to whoever will play it, once.
    ///
    /// Taken rather than borrowed: the ring's read end belongs to exactly one
    /// consumer, and that is the engine's render thread.
    pub fn take_monitor(&mut self) -> Option<(rtrb::Consumer<f32>, u16)> {
        self.audio.as_mut().and_then(|a| a.monitor.take())
    }

    /// The microphone fader, as a linear gain.
    ///
    /// Applied on the writer thread to the input block, before the meter reads
    /// it and before the file is written — so the number the fader shows is the
    /// number the take gets, and moving it visibly moves the VUs. See
    /// [`Session::input_gain`] for how long it was connected to nothing.
    pub fn set_input_gain(&self, linear: f32) {
        let sane = if linear.is_finite() && linear >= 0.0 {
            linear.min(16.0)
        } else {
            1.0
        };
        self.input_gain.store(sane.to_bits(), Ordering::Relaxed);
    }

    pub fn clear_clip(&mut self) {
        self.clipped = false;
        for (m, cmds) in [
            self.audio.as_ref().map(|a| (&a.meters, &a.cmds)),
            self.plugin_audio.as_ref().map(|p| (&p.meters, &p.cmds)),
        ]
        .into_iter()
        .flatten()
        {
            if let Ok(mut m) = m.lock() {
                m.clear_clip();
            }
            // A dead writer thread is not an error here: the send fails, the
            // meters are already cleared, and there is nothing left to
            // republish over them.
            let _ = cmds.send(Cmd::ClearClip);
        }
    }

    pub fn clipped(&self) -> bool {
        self.clipped
    }

    /// What the INPUT stream is running at, for the status panel.
    pub fn input_stats(&self) -> Option<(String, ivory_ui::recorder::StreamStats)> {
        let a = self.audio.as_ref()?;
        Some((
            a.device_name.clone(),
            ivory_ui::recorder::StreamStats {
                sample_rate: a.sample_rate,
                channels: a.channels,
                buffer_frames: a.buffer_frames,
            },
        ))
    }

    /// Whether anything is feeding the band's VU at all.
    ///
    /// **False is the ordinary case, not an error.** With no audio interface
    /// selected and the built-in instrument playing, neither of the two
    /// sources this type meters exists — so `meters()` answers SILENT and the
    /// VU sits still and never latches a clip, however hard the FM is driven.
    /// That is a Mac with a piano plugged into it, which is most of them; the
    /// caller falls back to the engine's own device-mix meters when this is
    /// false. See `DesktopApp::push_state`.
    pub fn has_meter_source(&self) -> bool {
        self.audio.is_some() || self.plugin_audio.is_some()
    }

    pub fn audio_device_name(&self) -> Option<&str> {
        self.audio.as_ref().map(|a| a.device_name.as_str())
    }

    pub fn audio_error(&self) -> Option<&str> {
        self.audio_error.as_deref()
    }

    /// What the band's meter shows.
    ///
    /// The input's level when there is one, and otherwise the INSTRUMENT's —
    /// because with no input open the instrument is what is being recorded, and
    /// a dead meter over a take that is capturing audio is the single most
    /// alarming thing this band can display.
    pub fn meters(&self) -> UiMeters {
        match (&self.audio, &self.plugin_audio) {
            (Some(a), _) => a.levels(),
            (None, Some(p)) => levels_of(&p.meters),
            (None, None) => UiMeters::SILENT,
        }
    }

    /// Seconds since the take started writing.
    /// Where the take being recorded is being written, while one is.
    ///
    /// The video file goes beside the `.wav` and the `.mid`, so the compositor
    /// needs the folder — and it has to be the folder of the take that is
    /// ACTUALLY running, not one recomputed from the settings, or a take
    /// started before somebody changed the destination would write its video
    /// somewhere else.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn take_dir(&self) -> Option<&std::path::Path> {
        self.take.as_ref().map(|t| t.dir())
    }

    pub fn elapsed(&self) -> f64 {
        match (self.state.is_writing(), self.started_instant) {
            (true, Some(at)) => at.elapsed().as_secs_f64(),
            _ => 0.0,
        }
    }

    /// Open (or re-open) the audio input.
    ///
    /// Called when the Recorder band opens and whenever the device selection
    /// changes — never at Record. RECORDER-PLAN §3: a device opened at Record
    /// costs warm-up time inside the take, and a meter that only comes alive
    /// once recording has begun cannot prevent the mistake it exists to prevent.
    pub fn open_input(
        &mut self,
        selection: &InputSelection,
        channel: Option<audio::ChannelPick>,
        buffer_frames: Option<u32>,
        sample_rate: Option<u32>,
    ) {
        if self.state.is_active() {
            return; // never swap the device out from under a running take
        }
        self.audio = None; // close the old one FIRST; some drivers refuse two
        // And stop the instrument's own writer, which was only running because
        // there was no input. Both drops join their threads, and each hands the
        // instrument tap back on the way out — so after this line the tap is in
        // the channel rather than lost with the thread.
        self.plugin_audio = None;
        let tap = self.recover_tap();
        match Audio::open(
            selection,
            channel,
            self.timebase,
            self.tap_tx.clone(),
            buffer_frames,
            sample_rate,
            Arc::clone(&self.input_gain),
        ) {
            Ok(a) => {
                if let Some(t) = tap {
                    let _ = a.cmds.send(Cmd::Plugin(Some(Box::new(t))));
                }
                self.audio = Some(a);
                self.audio_error = None;
            }
            Err(e) => {
                // The device would not open, so the instrument goes back to
                // recording itself rather than being silently dropped along
                // with the microphone nobody got.
                if let Some(t) = tap {
                    self.plugin_audio =
                        Some(PluginAudio::start(t, self.timebase, self.tap_tx.clone()));
                }
                self.audio_error = Some(e);
            }
        }
    }

    /// Collect the instrument tap from whichever writer has just released it.
    ///
    /// Only meaningful immediately after dropping one: `Drop` joins the thread,
    /// and the thread sends the tap as its last act, so by the time the drop
    /// returns the channel is holding it.
    fn recover_tap(&mut self) -> Option<crate::instrument::RecorderTap> {
        let mut last = None;
        // Drained rather than read once: a session that has opened and closed
        // several devices can have more than one in flight, and the NEWEST is
        // the one that matches the engine that is running now.
        while let Ok(t) = self.tap_rx.try_recv() {
            last = Some(*t);
        }
        last
    }

    /// Hand the monitor engine's recorder tap to the writer thread.
    ///
    /// Taken once and moved, because it is the read end of a lock-free ring and
    /// belongs to exactly one thread. `None` removes it.
    pub fn set_plugin_tap(&mut self, tap: Option<crate::instrument::RecorderTap>) {
        if let Some(audio) = &self.audio {
            // An input is open, so IT drives the take and mixes the instrument
            // in. Its writer owns the tap.
            self.plugin_audio = None;
            let _ = audio.cmds.send(Cmd::Plugin(tap.map(Box::new)));
            return;
        }
        // No input. The instrument records itself, on the output device's own
        // clock — see `PluginAudio`.
        match tap {
            Some(t) => match self.plugin_audio.as_ref() {
                // Already running: hand the new tap over rather than tearing
                // down a thread that is mid-take.
                Some(p) => {
                    let _ = p.cmds.send(Cmd::Plugin(Some(Box::new(t))));
                }
                None => {
                    self.plugin_audio =
                        Some(PluginAudio::start(t, self.timebase, self.tap_tx.clone()))
                }
            },
            None => self.plugin_audio = None,
        }
    }

    /// Whether a take can be recorded at all — with an input, or with an
    /// instrument on its own.
    pub fn can_record_audio(&self) -> bool {
        self.audio.is_some() || self.plugin_audio.is_some()
    }

    /// Which sources the next take is made of. Ignored mid-take: a take that
    /// changed what it was recording halfway through would produce a file
    /// matching neither answer.
    /// The take's time signature. Ignored mid-take, like the source: a `.mid`
    /// whose bar lines changed halfway through is a file nobody can edit.
    pub fn set_meter(&mut self, meter: ivory_ui::recorder::TimeSignature) {
        if !self.state.is_active() {
            self.meter = meter;
        }
    }

    pub fn set_source(&mut self, source: TakeSource) {
        if self.state.is_active() {
            return;
        }
        self.source = source;
        if let Some(audio) = &self.audio {
            let _ = audio.cmds.send(Cmd::Source(source));
        }
    }

    pub fn source(&self) -> TakeSource {
        self.source
    }

    pub fn close_input(&mut self) {
        if !self.state.is_active() {
            self.audio = None;
            self.audio_error = None;
            // The input's writer has just handed the instrument tap back. Give
            // it a writer of its own, so choosing "None" as the input leaves a
            // loaded instrument recordable rather than mute — which is the
            // whole point of the None row saying what it now says.
            if let Some(t) = self.recover_tap() {
                self.plugin_audio =
                    Some(PluginAudio::start(t, self.timebase, self.tap_tx.clone()));
            }
        }
    }

    // ── The camera ─────────────────────────────────────────────────────────

    /// Open a camera by uid, or close whatever is open when `uid` is `None`.
    ///
    /// **This blocks for 300-800 ms**, and over two seconds for a Continuity
    /// Camera: `AVCaptureSession.startRunning` is synchronous. That is exactly
    /// why it is called from the host's after-frame drain and never from a
    /// repaint — and exactly why it happens when the band OPENS rather than
    /// when Record is pressed, because it is warm-up the take must not contain.
    pub fn open_camera(&mut self, uid: Option<&str>) {
        if self.state.is_active() {
            return;
        }
        self.camera = None;
        self.camera_error = None;
        let Some(uid) = uid.filter(|u| !u.is_empty()) else {
            return;
        };
        // `hd()`, NOT `default()`. An all-`None` wish makes every candidate
        // miss on both size and rate, so the pick collapses to "largest under
        // the cap, then fastest" — 1080p60 on any modern webcam. That is 4.5x
        // the pixels to convert on the capture queue and 4.5x the texture to
        // upload, every frame, while a synth is running on the same machine,
        // for a preview whose job is framing.
        match camera::open_camera(uid, &camera::FormatWish::hd(), self.timebase) {
            Ok(stream) => self.camera = Some(stream),
            Err(e) => self.camera_error = Some(e.to_string()),
        }
    }

    pub fn close_camera(&mut self) {
        if !self.state.is_active() {
            self.camera = None;
            self.camera_error = None;
        }
    }

    /// The take's audio, on its way to the video encoder.
    ///
    /// Drained by the UI thread every frame while a video take is rolling. See
    /// [`StartArgs::audio_tx`] for why the samples travel in this direction.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn video_audio(&self) -> Option<&mpsc::Receiver<AudioChunk>> {
        self.audio_for_video.as_ref()
    }

    /// The rate and channel count the input device settled on.
    ///
    /// The encoder needs this to describe what it is being handed, and it
    /// cannot be taken from the settings: the device decides it. A take at
    /// 44.1 kHz described as 48 would play back a semitone and a bit sharp.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn video_audio_spec(&self) -> Option<(u32, u16)> {
        self.video_audio_spec
    }

    pub fn camera_error(&self) -> Option<&str> {
        self.camera_error.as_deref()
    }

    pub fn camera_format(&self) -> Option<camera::Format> {
        self.camera.as_ref().map(|c| c.format())
    }

    /// The camera is open and running but has never delivered a frame.
    ///
    /// The failure this exists for is specific and otherwise undiagnosable: a
    /// camera that is denied, or whose pixel format did not take, **does not
    /// error** — it produces a running session that delivers nothing, forever.
    /// What the user reports is "the camera is broken". Without this the band
    /// shows an empty preview box and says nothing at all.
    pub fn camera_silent(&self) -> bool {
        self.camera
            .as_ref()
            .is_some_and(|c| c.state().is_running() && c.stats().frames_delivered() == 0)
    }

    /// Whether the camera is open AND still answering.
    ///
    /// Distinct from `camera_format().is_some()`, which stays true for a
    /// device that has been unplugged — the format is cached at open. Anything
    /// deciding whether to keep showing a picture has to ask this one.
    pub fn camera_running(&self) -> bool {
        self.camera
            .as_ref()
            .is_some_and(|c| c.state().is_running())
    }

    /// The newest frame, if one has arrived since the last call.
    ///
    /// **Consuming, and newest-wins.** `None` means "nothing new", not "no
    /// camera" — the caller keeps showing the last texture it uploaded, which
    /// is what makes a 30 fps camera look right in a 60 fps window instead of
    /// flickering black on every other frame.
    pub fn next_frame(&self) -> Option<camera::Frame> {
        self.camera.as_ref()?.latest()
    }

    /// Camera frames delivered since the camera OPENED — a counter, not a
    /// per-take figure. The camera outlives the take (it opens with the band),
    /// so a per-take number is this read at Stop minus the same read at start;
    /// `TakeVideo` keeps the baseline.
    /// How often the camera's frames are worth converting, in nanoseconds.
    ///
    /// The host decides: a take with video wants every frame, a preview box
    /// wants a handful a second, and a camera open with nothing on screen
    /// showing it wants none. See `camera::FrameSlot::want_every`.
    pub fn set_camera_rate(&self, every_ns: u64) {
        if let Some(c) = self.camera.as_ref() {
            c.want_every(every_ns);
        }
    }

    pub fn camera_frames_delivered(&self) -> u64 {
        self.camera
            .as_ref()
            .map_or(0, |c| c.stats().frames_delivered())
    }

    /// Fold the finished video into the last take's manifest.
    ///
    /// `stop` wrote `take.json` without a video section, because the encoder
    /// belongs to the app and was still finishing the file. This sets the
    /// report, appends the file name, and rewrites the manifest atomically.
    /// A no-op when there is no finished take to amend — a video that
    /// finalises after the NEXT take has started must not write into it,
    /// which is why `begin` clears the retained manifest.
    pub fn record_video(&mut self, report: take::VideoReport, file_name: &str) {
        let Some((dir, manifest)) = self.finished.as_mut() else {
            return;
        };
        manifest.video = Some(report);
        manifest.files.push(file_name.to_owned());
        let _ = manifest.write(dir);
    }

    /// Drain the always-on MIDI tap into the take buffer.
    ///
    /// Called every frame, recording or not. The tap is always on because midir
    /// seals a connection's callback the moment the port opens, so there is no
    /// way to arm it later (see `midi.rs`) — and that turns out to be what a
    /// correct `.mid` needs anyway.
    /// `on_event` sees every message as it is drained, in the timebase.
    ///
    /// A fan-out rather than a second drain, because the tap is a QUEUE: two
    /// consumers means one of them gets each message and the other gets none.
    /// The monitor engine needs the same notes the take does — that is the
    /// whole point of hosting an instrument — so they come from one drain.
    pub fn pump_midi(&mut self, mut on_event: impl FnMut(Nanos, &[u8])) {
        let now = self.timebase.now();
        for (stamp, arrived, bytes) in self.tap.drain() {
            // Pair the device stamp with the arrival reading taken in the midir
            // callback, not with `now`: this loop runs at frame rate, and a
            // uniformly late reading is exactly what a running-minimum anchor
            // cannot see through.
            self.midi_clock
                .observe(stamp, self.timebase.at(arrived));
            let t = self
                .midi_clock
                .to_timebase(stamp)
                .unwrap_or_else(|| self.timebase.at(arrived));
            on_event(t, &bytes);
            self.midi.push(Captured::new(t, bytes));
        }
        // A rolling window BEHIND the present, and — critically — behind T0 as
        // well once a take is running.
        //
        // The first version pruned to `self.t0` during a take. That deletes
        // exactly the events `MidiTake::build` needs: its tick-0 snapshot pass
        // is `filter(|e| e.t < t0)`, so pruning at T0 made it empty on every
        // take and the whole rule-8 machinery unreachable. The symptom is a
        // `.mid` that never restates the sustain pedal or the program change
        // that were already true when Record was pressed — hold the pedal,
        // press Record, and the file plays dry.
        let horizon = if self.state.is_active() { self.t0 } else { now };
        self.midi.prune(horizon - MIDI_HISTORY_NS);
    }

    /// Capture a MIDI event this APP made, so a take carries it too.
    ///
    /// **The audio always had it and the `.mid` never did.** Anything the app
    /// sounds — a clicked key, a chord on the fretboard, a note placed on the
    /// lattice, the Space strike, the re-strike when a transpose moves a
    /// latched chord — goes to the instrument through `Engine::send_midi`, so
    /// it reaches the mix, the monitor and the recorded AUDIO. None of it went
    /// anywhere near [`pump_midi`], which drains the input device, so the
    /// `.mid` beside the audio was missing every note that was not played on a
    /// keyboard.
    ///
    /// Same stamp and same bytes as the engine was given, so the two cannot
    /// drift: what is written is what was heard, to the sample.
    pub fn capture_app_midi(&mut self, at: Nanos, bytes: [u8; 3]) {
        self.midi.push(Captured::new(at, bytes));
    }

    /// Press the one button. Starts a pre-roll, starts a take, or stops one.
    ///
    /// One button rather than arm-then-record, per RECORDER-PLAN §5: the second
    /// state exists in DAWs because a DAW has tracks to arm, and this does not.
    pub fn toggle(
        &mut self,
        root: &std::path::Path,
        name: Option<&str>,
        count_in_beats: u32,
        spec: ExportSpec,
    ) {
        match self.state {
            RecordState::Idle => {
                self.spec = spec;
                if count_in_beats == 0 {
                    self.begin(root, name);
                } else {
                    self.count_in_from = Some(Instant::now());
                    self.count_in_of = count_in_beats;
                    self.state = RecordState::CountIn {
                        beat: 1,
                        of: count_in_beats,
                    };
                }
            }
            // Pressing it during the count-in cancels, which is what anyone who
            // has just realised they came in wrong will try.
            RecordState::CountIn { .. } => {
                self.count_in_from = None;
                self.state = RecordState::Idle;
            }
            RecordState::Rolling => self.stop(),
            RecordState::Finishing => {}
        }
    }

    /// How long one beat of the take's SIGNATURE lasts, at its tempo.
    ///
    /// **Of the signature.** This returned `60 / bpm` — a quarter note, always
    /// — while the click the player actually hears comes from
    /// `instrument::period_frames`, which multiplies by `4 / unit`. In 6/8 the
    /// two disagreed by a factor of two: the clicks came at eighths and the
    /// number on screen advanced at quarters, so a twelve-beat count-in was
    /// still showing "6" as the last click sounded.
    ///
    /// One function owns the answer now, and it is
    /// [`TimeSignature::beat_seconds`] — the same one the engine's period is
    /// derived from.
    fn beat(&self) -> Duration {
        Duration::from_secs_f64(self.meter.beat_seconds(self.spec.tempo_bpm).max(1e-6))
    }

    /// Advance the count-in. Called once a frame.
    ///
    /// Returns true when something changed, so the caller knows to repaint —
    /// the count is the one part of this that animates without any input.
    ///
    /// **The beat number comes from the wall clock, not from the audio
    /// thread's sample count**, and the click the user hears comes from the
    /// audio thread. They are two clocks, so in principle they can disagree —
    /// in practice a count-in is at most eight beats, four seconds at 120, and
    /// the two differ by well under a millisecond over that. Worth knowing
    /// about; not worth a lock-free beat channel to fix.
    pub fn tick(&mut self, root: &std::path::Path, name: Option<&str>) -> bool {
        let Some(from) = self.count_in_from else {
            return false;
        };
        let beat = self.beat();
        let elapsed = Instant::now().saturating_duration_since(from);
        let done = elapsed.as_secs_f64() / beat.as_secs_f64();
        if done >= f64::from(self.count_in_of) {
            self.count_in_from = None;
            self.begin(root, name);
            return true;
        }
        // **The beat WITHIN THE BAR, and the bar's own length.**
        //
        // Two bars of 6/8 is "1 2 3 4 5 6, 1 2 3 4 5 6" — never "1 of 12". A
        // beat in a measure is a beat in a measure, and nobody counting a band
        // in has ever said "seven". This used to show the running total against
        // the total, which is a progress bar wearing a musician's clothes.
        //
        // The engine accents the same beat, because it takes the modulo of the
        // same number — so the "1" on screen is the click that sounds different.
        let per_bar = u32::from(self.meter.beats.max(1));
        let now = RecordState::CountIn {
            beat: (done as u32) % per_bar + 1,
            of: per_bar,
        };
        let changed = self.state != now;
        self.state = now;
        changed
    }

    /// T0 for the next `begin`, when something knows better than "now".
    ///
    /// The count-in sets it to the instant the downbeat is HEARD, which the
    /// audio thread knows exactly and the UI thread can only estimate — a frame
    /// late, plus the output device's own delay. Recording from the beat the
    /// player heard is the difference between a take that starts on the bar and
    /// one that starts a few milliseconds after it.
    pub fn arm_at(&mut self, t0: Nanos) {
        self.arm_override = Some(t0);
    }

    /// Forget a T0 that was offered and never used.
    ///
    /// Belt and braces beside the caller's own edge-detection: an override that
    /// outlives the count-in it came from is a take timestamped from a downbeat
    /// that happened minutes ago, and the failure is invisible until somebody
    /// opens the `.mid` beside the `.wav`.
    fn forget_arm(&mut self) {
        self.arm_override = None;
    }

    fn begin(&mut self, root: &std::path::Path, name: Option<&str>) {
        self.clipped = false;
        self.last = None;
        // The previous take's manifest must not be amendable once a new take
        // exists, or a slow encoder could write take N's video into take N+1.
        self.finished = None;
        self.pending_note = None;
        let at = WallTime::now_utc();
        let slug = name.and_then(take::sanitise_slug);
        if let Err(e) = take::prepare_root(root) {
            self.fail(format!("could not use the output folder: {e}"));
            return;
        }
        let take = match Take::create(root, &at, slug.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                self.fail(format!("could not create the take folder: {e}"));
                return;
            }
        };

        // T0 is read AFTER the directory exists. Creating a folder can block on
        // a slow or networked volume, and a T0 taken before it would put that
        // delay at the head of the file as silence the MIDI does not know about.
        self.t0 = self.arm_override.take().unwrap_or_else(|| self.timebase.now());
        self.started_at = at;
        self.started_instant = Some(Instant::now());
        // Keep the minute BEFORE T0: that is where the pedal-down and the
        // program change the .mid has to restate at tick 0 live.
        self.midi.prune(self.t0 - MIDI_HISTORY_NS);

        // `self.spec.audio` is the Export dialog's "write the .wav" tick, and
        // it has to be honoured HERE. Sending Start regardless meant unticking
        // audio still produced a microphone recording — and the summary still
        // said "audio + MIDI", so nothing revealed it.
        // No input device means no writer thread, and the writer thread is
        // what writes the WAV — even for a plugin-only take, because the input
        // is the only source with a device clock to measure. Somebody who
        // turned the input off and loaded a piano would otherwise get a take
        // with a .mid and no audio, and nothing saying why.
        // A take that can write no audio at all, and why. This used to fire
        // whenever there was no input device, because the input was the only
        // thing that could drive a writer — so somebody with a piano loaded and
        // "None" chosen was told to go and pick a microphone in order to record
        // an instrument the microphone would not be recording. Now the only
        // case left is the real one: nothing to record from at all.
        if self.spec.audio && !self.can_record_audio() {
            self.pending_note = Some(
                "no audio input is open and no instrument is loaded, so the                  take is MIDI only. Choose an input, or load an instrument, in                  the Recorder band."
                    .to_owned(),
            );
        }
        if let (Some(audio), true) = (&self.audio, self.spec.audio) {
            let spec = audio.spec();
            // The video's audio track, when the take has video. A channel per
            // take rather than one for the session's life: a receiver left over
            // from the last take would deliver its tail into this one.
            let (audio_tx, audio_rx) = if self.spec.video.wants_video() {
                let (tx, rx) = mpsc::channel();
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };
            self.audio_for_video = audio_rx;
            self.video_audio_spec = Some((spec.sample_rate, spec.channels));
            let args = StartArgs {
                path: take.wav(),
                spec,
                bext: Bext::new(at, spec),
                audio_tx,
            };
            let _ = audio.cmds.send(Cmd::Start(Box::new(args)));
        } else if let (Some(p), true) = (&self.plugin_audio, self.spec.audio) {
            // No input device: the instrument records itself. Same file, same
            // folder, same manifest — only the clock is different.
            let spec = p.spec();
            let (audio_tx, audio_rx) = if self.spec.video.wants_video() {
                let (tx, rx) = mpsc::channel();
                (Some(tx), Some(rx))
            } else {
                (None, None)
            };
            self.audio_for_video = audio_rx;
            self.video_audio_spec = Some((spec.sample_rate, spec.channels));
            let args = StartArgs {
                path: take.wav(),
                spec,
                bext: Bext::new(at, spec),
                audio_tx,
            };
            let _ = p.cmds.send(Cmd::Start(Box::new(args)));
        }
        self.take = Some(take);
        self.state = RecordState::Rolling;
    }

    fn fail(&mut self, problem: String) {
        self.state = RecordState::Idle;
        self.count_in_from = None;
        self.last = Some(Summary {
            folder: String::new(),
            seconds: 0.0,
            wrote_audio: false,
            wrote_midi: false,
            clipped: false,
            silent: false,
            problem: Some(problem),
            note: None,
        });
    }

    /// Stop, finish every file, and write the manifest.
    ///
    /// Synchronous: it blocks the UI thread for as long as the writer takes to
    /// patch a header and close a file, which is milliseconds. The alternative
    /// — returning to Idle while a 2 GB file is still flushing — is how a user
    /// learns that quitting immediately after Stop loses the take.
    pub fn stop(&mut self) {
        if !self.state.is_writing() {
            return;
        }
        self.state = RecordState::Finishing;
        let t1 = self.timebase.now();
        // The last few milliseconds of playing. `after_frame` drains the tap at
        // the top and handles Stop at the bottom, so without this the release
        // of the final chord is still in the tap — and it IS in the .wav, which
        // makes it an audio-versus-MIDI asymmetry rather than a rounding error.
        // `MidiTake::build` already filters to `<= t1`, so nothing past the end
        // can leak in.
        self.pump_midi(|_, _| {});
        let Some(take) = self.take.take() else {
            self.state = RecordState::Idle;
            return;
        };

        let spec = self.spec;
        let mut problem: Option<String> = None;
        // Something worth saying about a take that nonetheless worked.
        let mut note: Option<String> = self.pending_note.take();

        // ── audio ───────────────────────────────────────────────────────────
        let mut report: Option<AudioReport> = None;
        // Whichever writer is running this take. `audio` wins when both exist,
        // which matches `begin`: an open input drives the take.
        if let Some(p) = self.plugin_audio.as_ref().filter(|_| self.audio.is_none()) {
            while p.reports.try_recv().is_ok() {}
            let _ = p.cmds.send(Cmd::Stop);
            match p.reports.recv_timeout(Duration::from_secs(2)) {
                Ok(r) => report = Some(r),
                Err(_) => {
                    problem = Some(
                        "the instrument's recording thread did not finish in                          time; the audio file may be incomplete"
                            .to_owned(),
                    );
                }
            }
        }
        if let Some(audio) = &self.audio {
            // Drain anything the channel is still holding BEFORE asking for a
            // new report. A previous Stop that timed out at two seconds leaves
            // its report queued; without this the next take reads it and every
            // take from then on is attributed the one before it — frames, clip
            // latch, rate fit and all.
            while audio.reports.try_recv().is_ok() {}
            let _ = audio.cmds.send(Cmd::Stop);
            // Bounded, because a wedged writer thread must not hang the app
            // forever. Two seconds is far beyond a header patch and far short
            // of anything a user would read as a freeze.
            match audio.reports.recv_timeout(Duration::from_secs(2)) {
                Ok(r) => report = Some(r),
                Err(_) => {
                    problem = Some(
                        "the recording thread did not finish in time; \
                         the audio file may be incomplete"
                            .to_owned(),
                    );
                }
            }
        }
        if let Some(r) = &report {
            if let Some(e) = &r.error {
                problem.get_or_insert_with(|| e.clone());
            }
            if r.clipped_samples > 0 {
                self.clipped = true;
            }
        }

        // ── the timeline every file agrees on ───────────────────────────────
        //
        // A measured fit when there was a device to measure, and `synthetic`
        // when there was not. The distinction is the whole reason `synthetic`
        // exists: 0 ppm measured and 0 ppm assumed must not look alike in
        // `take.json`.
        let nominal = self
            .audio
            .as_ref()
            .map_or(48_000.0, |a| f64::from(a.sample_rate));
        // **T0 is where the audio actually starts, not where the button was
        // pressed.** RECORDER-PLAN §3: `T0 = max(T_audio_sample_0, T_arm)`.
        //
        // The writer reports the timebase instant of the frame it wrote first;
        // the ring was discarded at Start, so that frame is the first one
        // captured after arming — up to one buffer period after `T_arm`. Using
        // `T_arm` here instead would place every MIDI event that many
        // milliseconds early against the audio, identically for the whole take,
        // which is precisely the fixed offset this feature exists to remove.
        let t0 = report
            .as_ref()
            .and_then(|r| r.first_frame_ns)
            .map_or(self.t0, |first| self.t0.max(first));
        // And T1 stretches to cover the audio that is actually in the file.
        //
        // The writer does one last pass after Stop, so the `.wav` runs a poll
        // interval or so past the instant the button was pressed — measured at
        // about 11 ms. Left alone, `duration_seconds` and the SMF's end-of-track
        // describe a take slightly shorter than the file beside them, and the
        // three deliverables disagree about where the take ends.
        //
        // **Extended, never shortened.** `max` is the whole point: audio that
        // ran LONG is the writer's tail and the timeline should cover it, while
        // audio that ran SHORT is a device that stopped — and pulling T1 back to
        // meet it would silently truncate the MIDI to match a fault instead of
        // reporting one. The short case is already reported through `running`
        // and the frame count.
        let t1 = match &report {
            Some(r) if r.frames > 0 => {
                let secs = r.frames as f64 / nominal;
                t1.max(t0 + (secs * 1_000_000_000.0) as Nanos)
            }
            _ => t1,
        };
        let timeline = match &report {
            Some(r) => Timeline::from_fit(t0, t1, nominal, &r.fit),
            None => Timeline::synthetic(t0, t1, nominal),
        };

        // ── MIDI ────────────────────────────────────────────────────────────
        let mut wrote_midi = false;
        if spec.midi && !self.midi.is_empty() {
            // With the take's signature, so a DAW's bar lines land where the
            // click did. `Meter` carries the unit as a POWER of two, because
            // that is what the file stores and converting in two places is how
            // the two come to disagree.
            let meter = ivory_record::smf::Meter {
                beats: self.meter.beats,
                unit_power: self.meter.unit_power(),
            };
            match self
                .midi
                .write_with_meter(&timeline, spec.tempo_bpm, &take.midi(), meter)
            {
                Ok(()) => wrote_midi = true,
                Err(e) => {
                    problem.get_or_insert_with(|| format!("could not write the MIDI file: {e}"));
                }
            }
        }

        // ── the manifest ────────────────────────────────────────────────────
        //
        // Filled rather than left at its defaults, because `take.json` is the
        // only place the numbers that explain a bad take exist: how many frames
        // the ring lost, how many marks arrived with no device timestamp, what
        // the crystal actually measured. A manifest of defaults looks exactly
        // like a perfect take.
        let mut manifest = Manifest::starting(take.name(), self.started_at, t0);
        manifest.apply_timeline(&timeline);
        let frames = report.as_ref().map_or(0, |r| r.frames);
        if let Some(r) = &report {
            // Fields set INDIVIDUALLY rather than by replacing the struct.
            // `apply_timeline` is documented as the only thing allowed to set
            // `clock` and `epsilon_ppm`, so that a synthetic clock can never be
            // reported alongside a non-zero ppm; a struct literal here would
            // quietly take that guarantee back and leave the two agreeing only
            // by coincidence.
            manifest.audio.nominal_rate = nominal;
            manifest.audio.true_rate = r.fit.true_rate();
            manifest.audio.channels = r.channels;
            manifest.audio.bits = TAKE_FORMAT.bits();
            manifest.audio.frames = r.frames;
            manifest.audio.peak_dbfs = f64::from(audio::dbfs(r.take_peak));
            manifest.audio.clipped = r.clipped_samples > 0;
            manifest.sources.push(r.source.clone());
            // The device stopped answering. RECORDER-PLAN §4's policy is to
            // finalise what exists and mark it incomplete — a `.wav` of eight
            // minutes beside a `.mid` of twenty, with a manifest claiming
            // `complete: true`, is the worst possible outcome because nothing
            // anywhere says the two disagree.
            if !r.running {
                problem.get_or_insert_with(|| {
                    "the audio device stopped during the take; the recording \
                     ends where it stopped"
                        .to_owned()
                });
            }
            // A NOTE, not a problem, and the difference matters twice over.
            //
            // A dropout is padded with silence so the file stays
            // sample-accurate, which means it is inaudible as a gap — the only
            // trace it leaves is this line. But the take is complete, so
            // calling `abort` here would mark the manifest incomplete and send
            // the next launch into the crash-recovery path for a file that is
            // perfectly fine.
            if r.frames_dropped > 0 {
                note = Some(format!(
                    "{} frames were lost to the system and padded with silence",
                    r.frames_dropped
                ));
            } else if r.unstamped > 0 && r.fit.observations() == 0 {
                // The backend handed us no device timestamps at all, so there
                // is no fit and the take is on the nominal rate. Worth saying:
                // it is the difference between drift that was measured and
                // corrected and drift that was assumed away.
                note = Some(
                    "the audio device reported no timestamps, so the take is \
                     on its nominal rate rather than a measured one"
                        .to_owned(),
                );
            }
        }
        // Which sources this take was made of, so a file that sounds like a
        // piano and a file that sounds like a room can be told apart later.
        manifest
            .sources
            .push(take::SourceReport::from_clock("take_source", &self.midi_clock));
        if let Some(last) = manifest.sources.last_mut() {
            last.name = format!("sources: {}", self.source.to_setting());
        }
        manifest.midi = take::MidiReport {
            tempo_bpm: spec.tempo_bpm,
            ppq: ivory_record::smf::PPQ,
            events: self.midi.len(),
        };
        manifest
            .sources
            .push(take::SourceReport::from_clock("midi", &self.midi_clock));
        if frames > 0 {
            manifest.files.push(
                take.wav()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        if wrote_midi {
            manifest.files.push(
                take.midi()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            );
        }
        if let Some(p) = &problem {
            manifest.abort(p.clone());
        } else {
            manifest.finish();
        }
        let _ = manifest.write(take.dir());
        // Retained for `record_video`: the encoder finishes after this returns
        // and the manifest is the only place its report can live.
        self.finished = Some((take.dir().to_path_buf(), manifest.clone()));

        let silent = report
            .as_ref()
            .is_some_and(|r| r.frames > 0 && r.take_peak < 1e-5);
        self.last = Some(Summary {
            folder: take.name().to_owned(),
            seconds: timeline.duration_seconds(),
            wrote_audio: frames > 0,
            wrote_midi,
            clipped: self.clipped,
            silent,
            problem,
            note,
        });
        self.started_instant = None;
        self.forget_arm();
        self.state = RecordState::Idle;
    }

}

// ───────────────────────────────────────────────────────────────────────────
// Free space
// ───────────────────────────────────────────────────────────────────────────

/// Bytes available to this user on the volume holding `path`.
///
/// **Available, not free.** On every Unix filesystem some blocks are reserved
/// for root (5% is the ext4 default), so `f_bfree` is a number the user cannot
/// actually write into and `f_bavail` is. Reporting the first is how a
/// recorder promises eight more minutes and stops after six.
///
/// Walks up to the nearest existing ancestor, because the take root usually
/// does not exist yet the first time this is asked — that is the whole point of
/// showing the estimate before the first take.
pub fn available_bytes(path: &std::path::Path) -> Option<u64> {
    let mut probe = path;
    loop {
        if probe.exists() {
            break;
        }
        probe = probe.parent()?;
    }
    platform_available_bytes(probe)
}

#[cfg(unix)]
fn platform_available_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: `stat` is a plain POD struct with no invariants, `zeroed` is a
    // valid bit pattern for it, and `statvfs` either fills it and returns 0 or
    // leaves it alone and returns -1 — which is the branch below. `c_path` is a
    // NUL-terminated C string that outlives the call.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    // `f_frsize` is the fragment size, which is what the block counts are in.
    // `f_bsize` is the preferred I/O size and is NOT the same number on macOS.
    let unit = if stat.f_frsize > 0 {
        stat.f_frsize as u64
    } else {
        stat.f_bsize as u64
    };
    Some((stat.f_bavail as u64).saturating_mul(unit))
}

#[cfg(windows)]
fn platform_available_bytes(path: &std::path::Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free_to_caller: u64 = 0;
    // SAFETY: `wide` is NUL-terminated and outlives the call; the two output
    // pointers are valid for writes of `u64` and the API accepts null for the
    // other two out-parameters.
    let ok = unsafe {
        windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (ok != 0).then_some(free_to_caller)
}

// ───────────────────────────────────────────────────────────────────────────
// The developer probe
// ───────────────────────────────────────────────────────────────────────────

/// Record one take from the real default input, with no GUI, and report it.
///
/// ```text
/// tangent --record-test 5
/// ```
///
/// RECORDER-PLAN §12 step 4 asks for this, and it earns its place: it is the
/// only way to exercise the whole capture chain — device, ring, rate fit,
/// meters, WAV writer, BWF chunk, take directory, manifest — **against real
/// hardware**, which no unit test can do. It is also how the entitlements get
/// proven: run it from the signed `.app` bundle and a missing microphone
/// entitlement shows up as an empty device list rather than as a mystery
/// months later.
///
/// Undocumented in `--help`, like `--dump-midi`, because it is a probe rather
/// than a feature.
pub fn record_test(seconds: Option<String>) {
    let seconds: f64 = seconds
        .and_then(|s| s.parse().ok())
        .unwrap_or(5.0_f64)
        .clamp(0.5, 600.0);

    match audio::input_devices() {
        Ok(devices) if devices.is_empty() => {
            eprintln!(
                "no audio inputs found.\n\
                 If an interface is connected, this build may be missing the\n\
                 microphone entitlement - run the probe from the signed .app:\n\
                 dist/Tangent.app/Contents/MacOS/tangent --record-test"
            );
            return;
        }
        Ok(devices) => {
            println!("inputs:");
            for d in &devices {
                println!(
                    "  {:<40} {} ch  {} Hz{}",
                    d.key.to_string(),
                    d.channels.map_or_else(|| "?".into(), |c| c.to_string()),
                    d.sample_rate.map_or_else(|| "?".into(), |r| r.to_string()),
                    if d.is_default { "  (default)" } else { "" },
                );
            }
        }
        Err(e) => {
            eprintln!("could not enumerate audio inputs: {e}");
            return;
        }
    }

    let timebase = Timebase::new();
    let tap = Arc::new(RawMidiTap::new(60_000));
    let mut session = Session::new(Arc::clone(&tap), timebase);
    session.open_input(&InputSelection::Default, None, None, None);
    match (session.audio_device_name(), session.audio_error()) {
        (Some(name), _) => println!("\nopen: {name}"),
        (None, Some(e)) => {
            eprintln!("could not open the default input: {e}");
            return;
        }
        (None, None) => {
            eprintln!("no input opened and no error reported - that is a bug");
            return;
        }
    }

    // Meter for a moment BEFORE arming, which is the behaviour that kills the
    // "I recorded silence" failure class — so it is the behaviour the probe
    // shows you rather than one it takes on trust.
    println!("\nlevel check (2s) - play something:");
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        let m = session.meters();
        let bar = |v: f32| "#".repeat((v.clamp(0.0, 1.0) * 40.0) as usize);
        println!(
            "  L {:<40} R {:<40}",
            bar(m.left.peak),
            bar(m.right.peak)
        );
    }

    let root = std::env::temp_dir().join("Tangent");
    println!("\nrecording {seconds:.1}s to {}", root.display());
    session.toggle(&root, Some("record test"), 0, ExportSpec::default());
    let until = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(50));
        session.pump_midi(|_, _| {});
    }
    session.stop();

    let Some(summary) = session.last_summary() else {
        eprintln!("the take produced no summary - that is a bug");
        return;
    };
    println!("\n{}", summary.message());
    if let Some(p) = &summary.problem {
        eprintln!("PROBLEM: {p}");
    }
    let dir = root.join(&summary.folder);
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for e in entries.flatten() {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                println!("  {:<40} {size:>12} bytes", e.file_name().to_string_lossy());
            }
            // The wav is named after the take, not "audio.wav" — printing a
            // hardcoded name gives a command that fails.
            if let Some(wav) = std::fs::read_dir(&dir).ok().and_then(|mut e| {
                e.find_map(|x| {
                    let p = x.ok()?.path();
                    (p.extension()? == "wav").then_some(p)
                })
            }) {
                println!("\nplay it:  afplay {}", wav.display());
            }
            println!("read it:  cat {}", dir.join("take.json").display());
        }
        Err(e) => eprintln!("could not read {}: {e}", dir.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `Writer` wired to a synthetic ring, with no device anywhere.
    ///
    /// This exists because the writer is the one part of the recorder that
    /// cannot be checked by inspection: it turns a stream of `TimingMark`s into
    /// bytes, and the only question that matters — "does the file contain
    /// exactly the frames the device produced?" — is invisible until you count
    /// them.
    /// A `PluginWriter` over a ring the test owns the other end of.
    fn plugin_writer(channels: usize, slots: usize) -> (PluginWriter, rtrb::Producer<f32>, impl Fn(u64)) {
        let (tap, tx, note) = crate::instrument::RecorderTap::for_test(slots, channels);
        let w = PluginWriter {
            tap: Some(tap),
            timebase: Timebase::new(),
            tracker: LevelTracker::new(channels, 48_000.0),
            meters: Arc::new(Mutex::new(AudioMeters::new(channels))),
            wav: None,
            buf: Vec::new(),
            frames: 0,
            fit: RateFit::new(),
            first_frame_ns: None,
            dropped_at_arm: 0,
            error: None,
            audio_tx: None,
            channels: channels as u16,
            sample_rate: 48_000,
        };
        (w, tx, note)
    }

    /// **A take with no input device records the instrument.**
    ///
    /// The whole point of `PluginAudio`: before it, this take produced a `.mid`
    /// and nothing else, and the band told the user to go and choose a
    /// microphone in order to record an instrument the microphone would not
    /// have been recording.
    #[test]
    fn an_instrument_records_itself_with_no_input_device() {
        let dir = std::env::temp_dir().join("tangent-plugin-only");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("t.wav");

        const CH: usize = 2;
        const FRAMES: usize = 4096;
        let (mut w, mut tx, _note) = plugin_writer(CH, FRAMES * CH * 2);
        let spec = WavSpec {
            sample_rate: 48_000,
            channels: CH as u16,
            format: TAKE_FORMAT,
        };
        w.begin(StartArgs {
            path: path.clone(),
            spec,
            bext: Bext::new(WallTime::from_unix(1_786_804_327, 0), spec),
            audio_tx: None,
        });

        for i in 0..FRAMES {
            let v = (i as f32 / FRAMES as f32) * 0.5;
            for _ in 0..CH {
                let _ = tx.push(v);
            }
            // Pump periodically, as the thread does, so the ring never fills.
            if i % 512 == 511 {
                w.pump();
            }
        }
        let report = w.finish();

        assert_eq!(
            report.frames, FRAMES as u64,
            "every rendered frame should have reached the file"
        );
        assert_eq!(report.channels, CH as u16);
        assert_eq!(
            report.frames_dropped, 0,
            "nothing was lost, so nothing may be reported as lost"
        );
        assert!(report.error.is_none(), "{:?}", report.error);
        assert!(
            report.first_frame_ns.is_some(),
            "a take that wrote audio knows when its first frame was"
        );
        assert!(report.take_peak > 0.0, "the file is silent");
        // And the file is real.
        let size = std::fs::metadata(&path).expect("stat").len();
        // 24-bit stereo: 3 bytes a sample, plus a header.
        assert!(
            size >= (FRAMES * CH * 3) as u64,
            "a {size}-byte file cannot hold {FRAMES} frames"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **Rendered audio has no dropouts, so nothing is ever padded.**
    ///
    /// The input path pads a gap with silence because the device produced
    /// frames nobody read. Nothing produces frames here except the engine, and
    /// the engine's frames all arrive — so a loss can only be a ring overflow,
    /// which is a fault to report rather than a hole to fill.
    #[test]
    fn an_overflowing_ring_is_reported_and_not_padded_over() {
        let dir = std::env::temp_dir().join("tangent-plugin-overflow");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        const CH: usize = 2;
        // Deliberately tiny, and never pumped, so it fills.
        let (mut w, mut tx, note) = plugin_writer(CH, 256);
        let spec = WavSpec {
            sample_rate: 48_000,
            channels: CH as u16,
            format: TAKE_FORMAT,
        };
        w.begin(StartArgs {
            path: dir.join("t.wav"),
            spec,
            bext: Bext::new(WallTime::from_unix(1_786_804_327, 0), spec),
            audio_tx: None,
        });
        for _ in 0..4096 {
            if tx.push(0.25).is_err() {
                note(1);
            }
        }
        let report = w.finish();
        assert!(
            report.frames_dropped > 0,
            "the ring overflowed and the take said nothing about it"
        );
        // What DID arrive is still in the file — an overflow loses the frames
        // it refused, not the ones it took.
        assert!(report.frames > 0, "the file should hold what did fit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ring is drained even when no take is running, for the reason the
    /// input path had to learn: a ring nobody reads fills up, and then every
    /// frame it refuses is counted against the next take that IS recording.
    #[test]
    fn the_instrument_ring_is_drained_between_takes() {
        const CH: usize = 2;
        let (mut w, mut tx, note) = plugin_writer(CH, 1024);
        for _ in 0..40 {
            for _ in 0..512 {
                if tx.push(0.5).is_err() {
                    note(1);
                }
            }
            // No wav: this is the idle path.
            w.pump();
        }
        let overflowed = w.tap.as_ref().expect("tap").dropped();
        assert_eq!(
            overflowed, 0,
            "the ring overflowed {overflowed} times while idle - it is not \
             being drained, so the next take inherits the loss"
        );
    }

    fn writer_with_ring(channels: usize) -> (audio::CaptureSource, Writer) {
        let stats = Arc::new(audio::CaptureStats::new());
        let (source, sink) = audio::capture_channel(channels, 48_000, 512, Arc::clone(&stats));
        let writer = Writer {
            input_gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            input_gain_now: 1.0,
            input_gain_coeff: 1.0,
            sink,
            tracker: LevelTracker::new(channels, 48_000.0),
            clock: ClockTap::new(2_000_000_000),
            cursor: FrameCursor::new(),
            meters: Arc::new(Mutex::new(AudioMeters::new(channels))),
            wav: None,
            error: None,
            plugin_buf: Vec::new(),
            plugin: None,
            source: TakeSource::Input,
            first_frame_ns: None,
            short_frames: 0,
            dropped_at_arm: 0,
            unstamped_at_arm: 0,
            plugin_dropped_at_arm: 0,
            audio_tx: None,
            buf: Vec::new(),
            silence: vec![0.0; channels * 1024],
        };
        (source, writer)
    }

    /// **The regression test for the doubling bug.**
    ///
    /// The first `write_plan` looped over `[silence_before, silence_after]` and
    /// emitted the samples inside the loop under `if pad == before`. On an
    /// ordinary block both pads are zero, so that condition held on BOTH
    /// iterations and every sample was written twice — a file exactly twice as
    /// long as the take, with the performance duplicated end to end.
    ///
    /// Every unit test at the time passed, because they all drove the dropout
    /// path where the two pads differ. Real hardware found it in one run.
    #[test]
    fn the_file_holds_exactly_the_frames_the_device_produced() {
        let dir = std::env::temp_dir().join("tangent-writer-frames");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("t.wav");

        const CH: usize = 2;
        const BLOCK: usize = 256;
        const BLOCKS: usize = 40;
        let (mut source, mut writer) = writer_with_ring(CH);
        let spec = WavSpec {
            sample_rate: 48_000,
            channels: CH as u16,
            format: TAKE_FORMAT,
        };
        let at = WallTime::from_unix(1_786_804_327, 0);
        writer.wav = Some(WavWriter::create(&path, spec, &Bext::new(at, spec)).expect("create"));

        // A ramp, so a duplicated block is visible as a value repeating rather
        // than only as a count — a test that counted frames alone would pass on
        // a writer that wrote half the blocks twice and dropped the others.
        let mut host_ns: Nanos = 1_000_000;
        for b in 0..BLOCKS {
            let block: Vec<f32> = (0..BLOCK * CH)
                .map(|i| (b * BLOCK * CH + i) as f32 / 1e6)
                .collect();
            source.accept(&block, Some(host_ns), host_ns);
            host_ns += (BLOCK as Nanos * 1_000_000_000) / 48_000;
            writer.pump();
        }
        let report = writer.stop();

        assert_eq!(
            report.frames,
            (BLOCK * BLOCKS) as u64,
            "the file must hold exactly the frames the device produced - \
             double this number is the write_plan doubling bug"
        );
        assert!(report.error.is_none(), "{:?}", report.error);

        // And confirm against the bytes on disk, not just the writer's own
        // count: a writer that miscounted in the same direction as it wrote
        // would agree with itself.
        let size = std::fs::metadata(&path).expect("stat").len();
        let audio_bytes = u64::from(TAKE_FORMAT.bytes_per_sample()) * CH as u64 * (BLOCK * BLOCKS) as u64;
        assert!(
            size > audio_bytes && size < audio_bytes + 4096,
            "file is {size} bytes; expected {audio_bytes} of audio plus a header"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **The instrument's ring must be drained even when it is not recorded.**
    ///
    /// It was not, so it filled, and every frame it then refused was counted as
    /// a take loss: a 37-second take reported "1,608,192 frames were lost to
    /// the system and padded with silence" — 33 seconds of a 37-second
    /// recording — when nothing had been lost at all. The instrument simply was
    /// not part of that take.
    #[test]
    fn an_instrument_left_out_of_the_take_reports_no_losses() {
        let dir = std::env::temp_dir().join("tangent-writer-nodrain");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");

        const CH: usize = 2;
        const BLOCK: usize = 256;
        let (mut source, mut writer) = writer_with_ring(CH);
        writer.source = TakeSource::Input;
        // A REAL tap, deliberately tiny, and one the test keeps filling. The
        // first version of this test had `plugin: None` and passed with the fix
        // removed, which is no test at all: with nothing to drain there is
        // nothing to leave undrained.
        let (tap, mut tap_tx, note_overflow) = crate::instrument::RecorderTap::for_test(1024, 2);
        writer.plugin = Some(tap);
        let spec = WavSpec {
            sample_rate: 48_000,
            channels: CH as u16,
            format: TAKE_FORMAT,
        };
        let at = WallTime::from_unix(1_786_804_327, 0);
        writer.wav = Some(
            WavWriter::create(&dir.join("t.wav"), spec, &Bext::new(at, spec)).expect("create"),
        );

        let block = vec![0.25f32; BLOCK * CH];
        let mut host_ns: Nanos = 1_000_000;
        for _ in 0..40 {
            source.accept(&block, Some(host_ns), host_ns);
            host_ns += (BLOCK as Nanos * 1_000_000_000) / 48_000;
            // Push more instrument audio than a 1024-slot ring can hold, every
            // block, so a writer that does not drain it WILL overflow and count
            // losses.
            for _ in 0..(BLOCK * 2) {
                if tap_tx.push(0.5).is_err() {
                    note_overflow(1);
                }
            }
            writer.pump();
        }
        let report = writer.stop();
        assert_eq!(
            report.frames_dropped, 0,
            "an instrument that is not being recorded cannot lose a take any              frames"
        );
        assert_eq!(report.frames, (BLOCK * 40) as u64);

        // And the other half: the ring was actually kept MOVING. Reporting zero
        // losses is easy to get right by accident — the conditional above does
        // it — while still leaving the ring to fill up and stay full, which is
        // the state the user's 1,608,192 phantom losses came from.
        //
        // Measured through the tap's own overflow counter rather than by
        // draining what is left over. Draining was the first thing tried and it
        // proves nothing: an empty result reads the same whether the ring was
        // kept empty or merely could not be read.
        let overflowed = writer
            .plugin
            .as_ref()
            .expect("the tap is still installed")
            .dropped();
        assert_eq!(
            overflowed, 0,
            "the instrument's ring overflowed {overflowed} times during a take \
             that does not record it - it is not being drained, so it backs up \
             and every later take inherits the loss"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The dropout path, which is what the doubling bug was hiding behind:
    /// when the two pads DIFFER the old code was accidentally correct.
    #[test]
    fn a_dropout_is_padded_so_file_frames_still_equal_device_frames() {
        let dir = std::env::temp_dir().join("tangent-writer-dropout");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("t.wav");

        const CH: usize = 2;
        // A ring far too small to hold what is pushed, so the source is forced
        // to drop and the marks carry a real `frames_dropped`.
        let stats = Arc::new(audio::CaptureStats::new());
        let (mut source, sink) = audio::capture_channel(CH, 1024, 64, Arc::clone(&stats));
        let mut writer = Writer {
            input_gain: Arc::new(AtomicU32::new(1.0_f32.to_bits())),
            input_gain_now: 1.0,
            input_gain_coeff: 1.0,
            sink,
            tracker: LevelTracker::new(CH, 48_000.0),
            clock: ClockTap::new(2_000_000_000),
            cursor: FrameCursor::new(),
            meters: Arc::new(Mutex::new(AudioMeters::new(CH))),
            wav: None,
            error: None,
            plugin_buf: Vec::new(),
            plugin: None,
            source: TakeSource::Input,
            first_frame_ns: None,
            short_frames: 0,
            dropped_at_arm: 0,
            unstamped_at_arm: 0,
            plugin_dropped_at_arm: 0,
            audio_tx: None,
            buf: Vec::new(),
            silence: vec![0.0; CH * 1024],
        };
        let spec = WavSpec {
            sample_rate: 48_000,
            channels: CH as u16,
            format: TAKE_FORMAT,
        };
        let at = WallTime::from_unix(1_786_804_327, 0);
        writer.wav = Some(WavWriter::create(&path, spec, &Bext::new(at, spec)).expect("create"));

        // Push several blocks WITHOUT pumping, so the ring overflows.
        let block = vec![0.5f32; 512 * CH];
        let mut host_ns: Nanos = 1_000_000;
        for _ in 0..8 {
            source.accept(&block, Some(host_ns), host_ns);
            host_ns += (512 * 1_000_000_000) / 48_000;
        }
        writer.pump();
        let report = writer.stop();

        assert!(
            report.frames_dropped > 0,
            "the ring was meant to overflow; without a dropout this test proves \
             nothing"
        );
        assert_eq!(
            report.frames,
            8 * 512,
            "a dropout must be PADDED, not closed: the file stays sample-\
             accurate and the loss is audible as silence rather than sliding \
             every note after it earlier"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn session() -> Session {
        Session::new(Arc::new(RawMidiTap::new(1024)), Timebase::new())
    }

    /// The state machine has to be right without a device, because that is the
    /// configuration every test machine and every first launch is in.
    /// **The instrument tap survives the writer that was holding it.**
    ///
    /// It is the read end of a lock-free ring, so exactly one thread may own
    /// it, and which thread that is depends on whether an input device is open.
    /// Switching the input to "None" tears down one writer and starts another,
    /// and without the hand-back the tap died with the thread — leaving the
    /// instrument unrecordable until the plugin was reloaded, which is a bug
    /// nobody would ever connect to the device they had just changed.
    #[test]
    fn the_instrument_tap_survives_its_writer_being_torn_down() {
        let (tap, _tx, _note) = crate::instrument::RecorderTap::for_test(1024, 2);
        let (tap_tx, tap_rx) = mpsc::channel();
        let writer = PluginAudio::start(tap, Timebase::new(), tap_tx);
        assert_eq!(writer.channels, 2);
        // Dropping joins the thread, and the thread's last act is to send the
        // tap back — so by the time this returns it is in the channel.
        drop(writer);
        assert!(
            tap_rx.try_recv().is_ok(),
            "the tap was lost with the thread that held it"
        );
    }

    /// A session with no input device gives the instrument a writer of its own,
    /// and a take then records audio rather than only MIDI.
    #[test]
    fn a_session_with_no_input_still_records_a_loaded_instrument() {
        let dir = std::env::temp_dir().join("tangent-session-plugin-only");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        assert!(!s.can_record_audio(), "nothing is loaded yet");

        let (tap, mut tx, _note) = crate::instrument::RecorderTap::for_test(1 << 16, 2);
        s.set_plugin_tap(Some(tap));
        assert!(
            s.can_record_audio(),
            "an instrument with no input device should still be recordable"
        );

        s.toggle(&dir, Some("plugin"), 0, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Rolling);
        // Wait for the writer thread to have PROCESSED Start before pushing.
        // `begin` arms the tap, and arming discards the ring — so audio pushed
        // in the window between the button press and the thread noticing is
        // thrown away, which is correct (it is audio from before T0) and which
        // made the first version of this test record nothing at all.
        std::thread::sleep(Duration::from_millis(40));
        for _ in 0..4096 {
            let _ = tx.push(0.25);
            let _ = tx.push(0.25);
        }
        std::thread::sleep(Duration::from_millis(60));
        s.stop();

        let summary = s.last_summary().expect("a take produces a summary");
        assert!(summary.problem.is_none(), "{:?}", summary.problem);
        assert!(
            summary.wrote_audio,
            "the instrument was loaded and its audio should be in the take"
        );
        // And the old advice is gone: nothing should be telling the user to go
        // and choose a microphone.
        let msg = summary.message();
        assert!(
            !msg.contains("MIDI only"),
            "the take still claims to be MIDI only: {msg}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **During a count-in there is no take folder, and nothing may assume
    /// there is.**
    ///
    /// This pair is what the video lifecycle keys off, and getting it wrong
    /// cost every user with a count-in their video, silently: the host asked
    /// `is_recording()`, which is TRUE through the count-in, went looking for a
    /// folder to write an `.mp4` into, found none, and set the flag that stops
    /// it trying again. By the time the take actually started there was a
    /// folder and nothing left that would look at it.
    ///
    /// `is_writing()` is the honest question, and it is also right on its own
    /// terms: the bars before the downbeat are deliberately not in the audio,
    /// so they have no business being in the video.
    #[test]
    fn a_count_in_is_not_a_take_and_has_no_folder_yet() {
        let dir = std::env::temp_dir().join("tangent-countin-nofolder");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        s.toggle(&dir, Some("counted"), 4, ExportSpec::default());

        assert!(
            matches!(s.state(), RecordState::CountIn { .. }),
            "a count-in was asked for and not entered"
        );
        assert!(
            s.is_recording(),
            "a count-in IS active - this is the half that misleads"
        );
        assert!(
            !s.state().is_writing(),
            "a count-in is not writing, and that is the half to ask"
        );
        assert!(
            s.take_dir().is_none(),
            "there is no folder until the downbeat, so nothing may look for one"
        );
        s.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **`take.json` records the video, and the video's own file.**
    ///
    /// The regression this exists for ran the whole life of the recorder and
    /// was never noticed on a Mac, because a Mac encodes fine and nobody reads
    /// the manifest when the `.mp4` is sitting right there. `stop` writes the
    /// manifest before the encoder has finished the file, so the video section
    /// was always `null` and `take.mp4` was never in `files`. The session now
    /// keeps what it wrote and folds the report in afterwards.
    ///
    /// Asserted through the FILE rather than the in-memory manifest: what is
    /// on disk is the only thing anybody ever reads, and an amend that updated
    /// the struct and forgot to rewrite would pass any weaker test.
    #[test]
    fn the_manifest_gains_the_video_after_the_encoder_finishes() {
        let dir = std::env::temp_dir().join("tangent-session-test-video");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        s.toggle(&dir, Some("filmed"), 0, ExportSpec::default());
        s.stop();
        let folder = s.last_summary().expect("a take produces a summary").folder.clone();
        let manifest = dir.join(&folder).join("take.json");

        // At Stop the encoder is still writing, so there is no video yet.
        let before: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest).expect("take.json"))
                .expect("valid json");
        assert!(before["video"].is_null(), "a video before the encoder finished");

        s.record_video(
            ivory_record::take::VideoReport {
                container: "mp4".into(),
                video_codec: "h264".into(),
                audio_codec: "aac".into(),
                width: 1920,
                height: 1080,
                fps: 30.0,
                frames_expected: 300,
                frames_received: 297,
            },
            "take.mp4",
        );

        let after: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&manifest).expect("take.json"))
                .expect("valid json");
        assert_eq!(after["video"]["video_codec"], "h264", "{after:#}");
        assert_eq!(after["video"]["width"], 1920);
        assert_eq!(after["video"]["frames_received"], 297);
        assert!(
            after["files"]
                .as_array()
                .expect("files is a list")
                .iter()
                .any(|f| f == "take.mp4"),
            "the video is not in files: {after:#}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A video that finalises after the NEXT take has started must not write
    /// into it. `begin` clears the retained manifest, and this is that rule.
    #[test]
    fn a_late_video_does_not_land_in_the_following_take() {
        let dir = std::env::temp_dir().join("tangent-session-test-late");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        s.toggle(&dir, Some("first"), 0, ExportSpec::default());
        s.stop();
        let first = s.last_summary().expect("summary").folder.clone();
        s.toggle(&dir, Some("second"), 0, ExportSpec::default());
        s.record_video(
            ivory_record::take::VideoReport {
                container: "mp4".into(),
                video_codec: "h264".into(),
                audio_codec: "aac".into(),
                width: 640,
                height: 360,
                fps: 30.0,
                frames_expected: 10,
                frames_received: 10,
            },
            "take.mp4",
        );
        s.stop();
        let second = s.last_summary().expect("summary").folder.clone();
        assert_ne!(first, second);
        for folder in [first, second] {
            let m: serde_json::Value = serde_json::from_str(
                &std::fs::read_to_string(dir.join(&folder).join("take.json")).expect("take.json"),
            )
            .expect("valid json");
            assert!(m["video"].is_null(), "{folder} was amended by a late video");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **It had no `#[test]`.** It has never run, in any release, since it was
    /// written. Third of its kind found in this file's neighbourhood; the two
    /// before it were loops whose variable was unused.
    #[test]
    fn a_session_with_no_device_still_starts_and_stops_a_take() {
        let dir = std::env::temp_dir().join("tangent-session-test-basic");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        assert_eq!(s.state(), RecordState::Idle);
        s.toggle(&dir, Some("test"), 0, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Rolling);
        s.stop();
        assert_eq!(s.state(), RecordState::Idle);

        let summary = s.last_summary().expect("a take produces a summary");
        assert!(summary.problem.is_none(), "{:?}", summary.problem);
        assert!(!summary.wrote_audio, "there was no audio device");
        // The take directory exists and holds a manifest even with no media,
        // because a folder that appears and stays empty is indistinguishable
        // from a crash.
        let take_dir = dir.join(&summary.folder);
        assert!(take_dir.is_dir(), "{} was not created", take_dir.display());
        assert!(take_dir.join("take.json").is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_count_in_delays_the_take_and_can_be_cancelled() {
        let dir = std::env::temp_dir().join("tangent-session-test-countin");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        s.toggle(&dir, None, 3, ExportSpec::default());
        assert!(
            matches!(s.state(), RecordState::CountIn { .. }),
            "a count-in must not start writing immediately"
        );
        assert!(
            !dir.exists(),
            "nothing may be created until the countdown finishes - a cancelled \
             count-in must leave no empty folder behind"
        );
        s.toggle(&dir, None, 3, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Idle, "pressing again cancels");
        assert!(!dir.exists());
    }

    /// **The number on screen advances at the rate of the clicks.**
    ///
    /// The countdown is driven by `Session::beat` and the audible click by
    /// `instrument::period_frames`. They are two clocks and they must agree
    /// about how long a beat is — and they did not: `beat` returned a QUARTER
    /// note whatever the signature said, so in 6/8 the clicks came at eighths
    /// while the screen counted at half that. A twelve-beat count-in was still
    /// showing "6" as the last click sounded.
    #[test]
    fn the_countdown_advances_at_the_same_rate_as_the_click() {
        use ivory_ui::recorder::TimeSignature;
        for (sig, bpm, want_secs) in [
            (TimeSignature { beats: 4, unit: 4 }, 120.0, 0.5),
            // The one that was wrong: an eighth at 120 is a quarter of a second.
            (TimeSignature { beats: 6, unit: 8 }, 120.0, 0.25),
            (TimeSignature { beats: 7, unit: 8 }, 90.0, 60.0 / 90.0 * 0.5),
            // A half-note beat gets two quarters.
            (TimeSignature { beats: 2, unit: 2 }, 120.0, 1.0),
        ] {
            let mut s = session();
            s.set_meter(sig);
            s.spec = ExportSpec {
                tempo_bpm: bpm,
                ..ExportSpec::default()
            };
            // Through a `Duration`, which is quantised to NANOSECONDS — so the
            // tolerances below are a nanosecond and not an epsilon. 7/8 at 90
            // is 0.3333... seconds, which no exact comparison will ever match
            // after that round trip.
            let shown = s.beat().as_secs_f64();
            assert!(
                (shown - want_secs).abs() < 1e-9,
                "{} at {bpm}: the screen counts a beat as {shown}s, the click plays {want_secs}s",
                sig.label()
            );
            // And it IS the engine's own answer, not a second derivation that
            // happens to match today.
            let engine = sig.beat_seconds(bpm);
            assert!(
                (shown - engine).abs() < 1e-9,
                "{}: the screen and the engine derive a beat differently",
                sig.label()
            );
        }
    }

    /// **A two-bar count-in in 6/8 counts 1 2 3 4 5 6, 1 2 3 4 5 6.**
    ///
    /// Never "1 of 12". A beat in a measure is a beat in a measure, and nobody
    /// counting a band in has ever said "seven". It used to show the running
    /// total against the total, which is a progress bar wearing a musician's
    /// clothes.
    #[test]
    fn the_count_in_counts_beats_within_the_bar() {
        use ivory_ui::recorder::TimeSignature;
        let dir = std::env::temp_dir().join("tangent-countin-in-bar");
        let _ = std::fs::remove_dir_all(&dir);

        let sig = TimeSignature { beats: 6, unit: 8 };
        let mut s = session();
        s.set_meter(sig);
        // Two bars of 6/8 = twelve beats, at 120 bpm = 0.25s each.
        s.toggle(&dir, None, sig.beats_in(2), ExportSpec::default());

        let beat_len = sig.beat_seconds(120.0);
        let mut seen = Vec::new();
        for n in 0..12 {
            // Park the start so that exactly `n` beats have elapsed, plus a
            // sliver so we are inside beat n+1 rather than on its edge.
            let elapsed = Duration::from_secs_f64(beat_len * (n as f64 + 0.5));
            s.count_in_from = Some(Instant::now() - elapsed);
            s.tick(&dir, None);
            match s.state() {
                RecordState::CountIn { beat, of } => {
                    assert_eq!(of, 6, "the bar is six beats long, not twelve");
                    seen.push(beat);
                }
                other => panic!("beat {n} left the count-in: {other:?}"),
            }
        }
        assert_eq!(seen, vec![1, 2, 3, 4, 5, 6, 1, 2, 3, 4, 5, 6]);

        // And it still ENDS after all twelve, which is the half that must not
        // regress while fixing the numbers.
        s.count_in_from = Some(Instant::now() - Duration::from_secs(60));
        assert!(s.tick(&dir, None), "an expiring count-in is a change");
        assert_eq!(s.state(), RecordState::Rolling);
        s.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A signature cannot change under a take that is already running.**
    ///
    /// The session refuses it, and the ENGINE has to refuse it too — that pair
    /// is the point. The engine's copy was pushed every frame with no take
    /// check, so a right-click during a take moved the click and the accent
    /// while the countdown and the `.mid` kept the old meter: one setting with
    /// two live values, and a file whose bar lines do not match what was heard.
    #[test]
    fn the_signature_is_fixed_once_a_take_is_under_way() {
        use ivory_ui::recorder::TimeSignature;
        let dir = std::env::temp_dir().join("tangent-meter-locked");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();

        let four = TimeSignature { beats: 4, unit: 4 };
        let six = TimeSignature { beats: 6, unit: 8 };
        s.set_meter(four);
        s.toggle(&dir, Some("locked"), 0, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Rolling);

        s.set_meter(six);
        assert_eq!(
            s.meter, four,
            "the signature moved under a rolling take"
        );
        // The beat length the countdown and the file use is the OLD one too.
        assert!((s.beat().as_secs_f64() - four.beat_seconds(120.0)).abs() < 1e-9);

        s.stop();
        // And once it is over, it takes the new one.
        s.set_meter(six);
        assert_eq!(s.meter, six, "a stopped session should accept a change");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A T0 offered by one take must not be used by the next.**
    ///
    /// `arm_at` is how the engine hands over the instant the downbeat was
    /// HEARD, which is more accurate than the UI frame that noticed it. But
    /// `count_in_done` is a latch — it stays true until the next count-in
    /// starts — so the caller offered the same instant on every frame for the
    /// rest of the session, and a take that began without a count-in took it.
    /// With "record the count-in into the take" on, every take starts with a
    /// count-in length of zero, so every take after the first would have been
    /// stamped from the first one's downbeat.
    #[test]
    fn a_stale_downbeat_cannot_time_stamp_the_next_take() {
        let dir = std::env::temp_dir().join("tangent-stale-arm");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();

        // A take that WAS armed from a downbeat.
        let downbeat = s.timebase.now();
        s.arm_at(downbeat);
        s.toggle(&dir, Some("first"), 0, ExportSpec::default());
        assert_eq!(s.t0, downbeat, "the armed instant is what a take starts at");
        s.stop();

        // The offer is not renewed, and the next take must not inherit it.
        let before = s.timebase.now();
        s.toggle(&dir, Some("second"), 0, ExportSpec::default());
        assert!(
            s.t0 >= before,
            "the second take was stamped from the first take's downbeat"
        );
        s.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The countdown has to actually count. A `tick` that never reaches zero is
    /// a record button that does nothing, and it would pass any test that only
    /// checked the state right after pressing it.
    #[test]
    fn a_count_in_that_elapses_starts_the_take() {
        let dir = std::env::temp_dir().join("tangent-session-test-elapse");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        s.toggle(&dir, None, 0, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Rolling);
        s.stop();

        // And through the countdown path, forced to expire.
        s.toggle(&dir, None, 3, ExportSpec::default());
        s.count_in_from = Some(Instant::now() - Duration::from_secs(60));
        assert!(s.tick(&dir, None), "an expiring count-in is a change");
        assert_eq!(s.state(), RecordState::Rolling);
        s.stop();
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two takes in a row must not collide, because "type a name once and press
    /// record five times" is the workflow the naming scheme exists to support.
    #[test]
    fn five_takes_with_the_same_name_make_five_folders() {
        let dir = std::env::temp_dir().join("tangent-session-test-five");
        let _ = std::fs::remove_dir_all(&dir);
        let mut s = session();
        let mut names = std::collections::HashSet::new();
        for _ in 0..5 {
            s.toggle(&dir, Some("nocturne"), 0, ExportSpec::default());
            s.stop();
            let summary = s.last_summary().expect("summary");
            assert!(summary.problem.is_none(), "{:?}", summary.problem);
            names.insert(summary.folder.clone());
        }
        assert_eq!(names.len(), 5, "a take overwrote another: {names:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A destination that cannot be written to has to say so rather than
    /// silently returning to Idle, which reads as "the button is broken".
    #[test]
    fn an_unusable_output_folder_is_reported_and_not_swallowed() {
        let mut s = session();
        // A path under a regular FILE, which cannot be a directory.
        let file = std::env::temp_dir().join("tangent-session-test-file");
        std::fs::write(&file, b"not a directory").expect("write");
        let root = file.join("takes");
        s.toggle(&root, None, 0, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Idle);
        let summary = s.last_summary().expect("a failure is still a summary");
        assert!(
            summary.problem.is_some(),
            "a take that could not start must say why"
        );
        assert!(!summary.message().is_empty());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn stopping_when_nothing_is_running_does_nothing() {
        let mut s = session();
        s.stop();
        assert_eq!(s.state(), RecordState::Idle);
        assert!(s.last_summary().is_none());
    }

    /// The summary is the only thing most users read, so every branch of it has
    /// to be a sentence rather than a debug print.
    #[test]
    fn a_summary_reads_as_a_sentence_in_every_state() {
        // A builder rather than `..base`: `Summary` owns two `String`s and a
        // struct-update expression moves them, so the second variant would not
        // compile against the first.
        let make = |clipped, silent, problem: Option<&str>, note: Option<&str>| Summary {
            folder: "nocturne-2026-08-16-141203".into(),
            seconds: 252.0,
            wrote_audio: true,
            wrote_midi: true,
            clipped,
            silent,
            problem: problem.map(str::to_owned),
            note: note.map(str::to_owned),
        };
        assert_eq!(
            make(false, false, None, None).message(),
            "Recorded 4:12 of audio + MIDI to nocturne-2026-08-16-141203"
        );
        assert!(make(true, false, None, None).message().contains("clipped"));
        let silent = make(true, true, None, None).message();
        assert!(
            silent.contains("silent") && !silent.contains("clipped"),
            "silence is the worse news and must not be buried under the clip note"
        );
        // A take that failed BEFORE a folder existed reports only the problem.
        let never_started = Summary {
            folder: String::new(),
            seconds: 0.0,
            wrote_audio: false,
            wrote_midi: false,
            clipped: false,
            silent: false,
            problem: Some("the disk is full".into()),
            note: None,
        };
        assert_eq!(
            never_started.message(),
            "the disk is full",
            "a take that did not happen must not also claim to have recorded 4:12"
        );
        // But a take that failed PART WAY has files on disk, and the folder
        // name is the one thing the user needs in order to find them. A disk
        // that filled at 6:00 of a 10:00 take leaves six good minutes; a
        // message of "No space left on device" alone sends the user looking
        // for a take they think was lost.
        let partial = make(false, false, Some("the disk is full"), None).message();
        assert!(
            partial.starts_with("the disk is full") && partial.contains("nocturne-"),
            "{partial}"
        );
        // A note is an addition, not a replacement: the folder name is the one
        // thing the user needs from this line and it must survive every warning.
        let noted = make(false, false, None, Some("48 frames were lost")).message();
        assert!(
            noted.contains("nocturne-2026-08-16-141203") && noted.contains("48 frames were lost"),
            "{noted}"
        );
    }

    /// The estimate is shown before the first take, when the take root does
    /// not exist yet — so a function that only answered for existing paths
    /// would return nothing at exactly the moment it is wanted.
    #[test]
    fn free_space_is_reported_for_a_folder_that_does_not_exist_yet() {
        let root = std::env::temp_dir().join("tangent-not-created/deeper/still");
        let bytes = available_bytes(&root).expect("the temp volume has a size");
        assert!(bytes > 0, "a writable volume with zero bytes free is not one");
        assert!(
            available_bytes(&std::env::temp_dir()).is_some(),
            "and an existing folder answers too"
        );
    }

    /// **The pedal that went down before Record was pressed must survive into
    /// the take**, because that is the entire reason the MIDI tap is always on.
    ///
    /// `MidiTake::build` writes its tick-0 state restatement from
    /// `filter(|e| e.t < t0)`. Pruning to T0 — which the first version did, in
    /// both `begin` and `pump_midi` — makes that set empty on every take, so
    /// the `.mid` never restates the sustain pedal or the program change and
    /// plays back dry. The bug was locked in by the earlier version of THIS
    /// test, which asserted the pre-T0 event was correctly dropped.
    #[test]
    fn the_pedal_pressed_before_record_survives_into_the_take() {
        let mut s = session();
        // A pedal-down a second before the take, and a note inside it.
        let pedal_at = 5_000_000_000 - 1_000_000_000;
        s.midi.push(Captured::new(pedal_at, [0xB0, 64, 127]));
        s.state = RecordState::Rolling;
        s.t0 = 5_000_000_000;
        s.midi.push(Captured::new(6_000_000_000, [0x90, 60, 90]));
        s.pump_midi(|_, _| {});
        assert_eq!(
            s.midi.len(),
            2,
            "the pre-T0 pedal must still be there for the tick-0 snapshot"
        );
    }

    /// The window is bounded, though — an always-on tap that never forgot
    /// anything would grow for as long as the app is open.
    #[test]
    fn midi_history_older_than_the_window_is_still_dropped() {
        let mut s = session();
        s.state = RecordState::Rolling;
        s.t0 = MIDI_HISTORY_NS * 3;
        // Two minutes before T0, against a one-minute window.
        s.midi
            .push(Captured::new(s.t0 - MIDI_HISTORY_NS * 2, [0xB0, 64, 127]));
        // And one second before it, which is inside the window.
        s.midi
            .push(Captured::new(s.t0 - 1_000_000_000, [0xB0, 64, 127]));
        s.pump_midi(|_, _| {});
        assert_eq!(
            s.midi.len(),
            1,
            "the window is bounded behind T0, not unbounded"
        );
    }
}

#[cfg(test)]
mod input_gain_tests {
    use super::{gain_slew_coefficient, walk_gain};

    /// **The fader reaches the samples, and slides rather than steps.**
    ///
    /// It reached nothing at all until 4.20.0: `gains.input` was packaged by
    /// the settings, drawn on the fader, written by the drag, and read by
    /// nobody in `ivory/src`. The owner's report was "moving the mic fader
    /// changes neither the VUs nor the recorded level nor the master", which
    /// was the whole truth.
    #[test]
    fn the_fader_scales_the_block_and_gets_there_smoothly() {
        let coeff = gain_slew_coefficient(48_000.0);
        // Half a second of stereo ones, which is far longer than the 10 ms the
        // pole needs.
        let mut buf = vec![1.0_f32; 2 * 24_000];
        let end = walk_gain(&mut buf, 2, 0.5, 1.0, coeff);
        assert!(
            (end - 0.5).abs() < 1.0e-3,
            "the gain settled at {end}, not at the fader"
        );
        // The tail is at the target...
        assert!((buf[buf.len() - 1] - 0.5).abs() < 1.0e-3);
        // ...and the head is NOT, which is the whole point: a block that
        // arrived already at 0.5 would be the step this exists to avoid.
        assert!(
            buf[0] > 0.99,
            "the gain stepped instead of sliding: first sample {}",
            buf[0]
        );

        // **A drag is inaudible.** No neighbouring pair may differ by enough to
        // click; at 48 kHz and a 10 ms pole the biggest step is tiny.
        let worst = buf
            .chunks_exact(2)
            .zip(buf.chunks_exact(2).skip(1))
            .map(|(a, b)| (a[0] - b[0]).abs())
            .fold(0.0_f32, f32::max);
        assert!(worst < 0.01, "the biggest sample-to-sample jump is {worst}");
    }

    /// Both channels of a frame get the same gain, so a moving fader does not
    /// swing the stereo image.
    #[test]
    fn a_moving_fader_does_not_pan() {
        let coeff = gain_slew_coefficient(48_000.0);
        let mut buf = vec![1.0_f32; 2 * 512];
        walk_gain(&mut buf, 2, 0.0, 1.0, coeff);
        for (i, frame) in buf.chunks_exact(2).enumerate() {
            assert_eq!(frame[0], frame[1], "frame {i} was panned by the fader");
        }
    }

    /// Unity leaves the samples exactly alone — not nearly alone.
    #[test]
    fn unity_is_a_no_op() {
        let coeff = gain_slew_coefficient(44_100.0);
        let mut buf: Vec<f32> = (0..64).map(|i| i as f32 * 0.01 - 0.3).collect();
        let before = buf.clone();
        let end = walk_gain(&mut buf, 2, 1.0, 1.0, coeff);
        assert_eq!(buf, before, "unity changed the samples");
        assert_eq!(end, 1.0);
    }

    /// A partial frame at the end of a block is left alone rather than scaled
    /// as if it were whole — `chunks_exact_mut` drops it, which is the same
    /// rule `CaptureSource::accept` follows for the same reason.
    #[test]
    fn a_trailing_partial_frame_is_not_scaled() {
        let mut buf = vec![1.0_f32; 5];
        walk_gain(&mut buf, 2, 0.0, 0.0, 1.0);
        assert_eq!(buf[4], 1.0, "half a frame was scaled");
    }
}

#[cfg(test)]
mod source_tests {
    use super::TakeSource;

    /// **Everything on the instrument bus counts, not just a VST3.**
    ///
    /// The bug this exists for cost the owner twelve takes across three
    /// releases and was invisible from inside the app: the monitor played the
    /// built-in DX7, the meters moved, the `.mid` captured every note, and the
    /// `.wav` had the microphone and nothing else. `resolve` was asking "is a
    /// PLUGIN loaded", the built-in is not one, so `auto` answered `Input` and
    /// the whole bus was left out of the file.
    ///
    /// The backing track is the same mistake found a second way: loaded, in the
    /// mix, audible, and not in the take — which reads as "the backing track
    /// does not record", because the bleed into the microphone is still there.
    ///
    /// So the parameter is `track_loaded` beside `plugin_loaded` and the caller
    /// passes `any_instrument_loaded()`, which counts the built-in. Every arm
    /// below is a case that produced a wrong file.
    #[test]
    fn anything_on_the_instrument_bus_is_worth_recording() {
        // A backing track and a microphone: the take must have both.
        assert_eq!(
            TakeSource::resolve("auto", false, true, true),
            TakeSource::Both,
            "a take made while a backing track plays left the track out"
        );
        // A backing track and no microphone: the bus is the whole take.
        assert_eq!(
            TakeSource::resolve("auto", false, true, false),
            TakeSource::Plugin
        );
        // The same for an instrument, which is what the caller now passes for
        // the built-in as well as for a VST3.
        assert_eq!(
            TakeSource::resolve("auto", true, false, true),
            TakeSource::Both
        );
        // And with neither, the input is still the only thing there is.
        assert_eq!(
            TakeSource::resolve("auto", false, false, true),
            TakeSource::Input
        );

        // **An explicit choice is still obeyed**, including the one that says
        // the bus only. "Record the instruments" with a backing track loaded
        // and no plugin is a real request, and it used to fall through to the
        // microphone.
        assert_eq!(
            TakeSource::resolve("plugin", false, true, true),
            TakeSource::Plugin,
            "instruments-only with a track loaded recorded the microphone"
        );
        assert_eq!(
            TakeSource::resolve("both", false, true, true),
            TakeSource::Both
        );
        // Microphone-only stays reachable, which is what makes the setting
        // worth having at all.
        assert_eq!(
            TakeSource::resolve("input", true, true, true),
            TakeSource::Input
        );
    }

    /// "Record the plugin" with no plugin loaded must not record silence. A
    /// take of nothing is never what anybody meant, and it is a setting that
    /// survives from a session where a plugin WAS loaded.
    #[test]
    fn asking_for_a_plugin_that_is_not_loaded_records_the_input_instead() {
        assert_eq!(
            TakeSource::resolve("plugin", false, false, true),
            TakeSource::Input
        );
        assert_eq!(TakeSource::resolve("both", false, false, true), TakeSource::Input);
    }

    /// And the mirror: with a plugin loaded and no input device open, the
    /// plugin is the only thing there is to record.
    #[test]
    fn with_no_input_open_a_loaded_plugin_is_the_take() {
        assert_eq!(
            TakeSource::resolve("input", true, false, false),
            TakeSource::Plugin
        );
        assert_eq!(TakeSource::resolve("both", true, false, false), TakeSource::Plugin);
    }

    /// **The bug a user hit within minutes.** Load a piano, press record, and
    /// the file had only the microphone in it — the instrument was monitored,
    /// plainly audible, and absent from the take, because the stored default
    /// said `input` and no control had ever existed to say otherwise.
    #[test]
    fn a_loaded_instrument_is_in_the_take_unless_somebody_says_otherwise() {
        assert_eq!(
            TakeSource::resolve("auto", true, false, true),
            TakeSource::Both,
            "an instrument you went and loaded belongs in the recording"
        );
        assert_eq!(
            TakeSource::resolve("auto", false, false, true),
            TakeSource::Input,
            "and with none loaded there is nothing extra to add"
        );
        // An EXPLICIT choice still wins, which is what makes the other three
        // menu rows mean anything.
        assert_eq!(TakeSource::resolve("input", true, false, true), TakeSource::Input);
        assert_eq!(
            TakeSource::resolve("plugin", true, false, true),
            TakeSource::Plugin
        );
    }

    #[test]
    fn an_ordinary_setup_gets_what_it_asked_for() {
        assert_eq!(TakeSource::resolve("input", true, false, true), TakeSource::Input);
        assert_eq!(TakeSource::resolve("plugin", true, false, true), TakeSource::Plugin);
        assert_eq!(TakeSource::resolve("both", true, false, true), TakeSource::Both);
        assert_eq!(
            TakeSource::resolve("nonsense from a later build", false, false, true),
            TakeSource::Input
        );
    }

    #[test]
    fn the_setting_round_trips() {
        for s in ["input", "plugin", "both"] {
            assert_eq!(TakeSource::resolve(s, true, false, true).to_setting(), s);
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// A take with no input device
// ───────────────────────────────────────────────────────────────────────────

/// The instrument recorded on its OWN clock, with no audio input open.
///
/// **Why this exists.** The writer above is driven by an input device's marks,
/// so a take needed one even when the only thing being recorded was a hosted
/// instrument — the microphone was there purely to supply a clock. That is
/// backwards: somebody who has loaded a piano and chosen "None" as their input
/// wants the piano in the file, and being told "record MIDI only" is being told
/// the app cannot do the obvious thing.
///
/// **Why it is simpler than the input path, rather than a copy of it.** Captured
/// audio can be LOST — the device produces frames whether or not anyone is
/// reading, so a dropout is real and has to become padded silence at exactly
/// the right place, which is what [`FrameCursor`] and [`WritePlan`] are for.
/// Rendered audio cannot be lost that way. The engine's callback produces every
/// frame of it, in order, and the only way to lose one is to let the ring
/// overflow — which is a fault to report, not a hole to pad. So there is no
/// cursor here, no plan, and no padding: drain the ring, write what it gave.
///
/// The clock is the output device's, measured the same way the input's is: a
/// [`RateFit`] over (frames written, timebase instant) pairs. It is a real
/// measurement of a real crystal, not the nominal rate.
struct PluginAudio {
    channels: u16,
    sample_rate: u32,
    cmds: mpsc::Sender<Cmd>,
    reports: mpsc::Receiver<AudioReport>,
    meters: Arc<Mutex<AudioMeters>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl PluginAudio {
    /// Take ownership of the tap and start a writer thread around it.
    fn start(
        tap: crate::instrument::RecorderTap,
        timebase: Timebase,
        tap_home: mpsc::Sender<Box<crate::instrument::RecorderTap>>,
    ) -> Self {
        let channels = tap.channels().max(1);
        let sample_rate = tap.sample_rate().max(1);
        let meters = Arc::new(Mutex::new(AudioMeters::new(channels)));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));
        let alive = Arc::clone(&running);
        let meters_for_thread = Arc::clone(&meters);

        let thread = std::thread::Builder::new()
            .name("tangent-instrument-writer".into())
            .spawn(move || {
                let mut w = PluginWriter {
                    tap: Some(tap),
                    timebase,
                    tracker: LevelTracker::new(channels, f64::from(sample_rate)),
                    meters: meters_for_thread,
                    wav: None,
                    buf: Vec::with_capacity(channels * 4096),
                    frames: 0,
                    fit: RateFit::new(),
                    first_frame_ns: None,
                    dropped_at_arm: 0,
                    error: None,
                    audio_tx: None,
                    channels: channels as u16,
                    sample_rate,
                };
                while alive.load(Ordering::Relaxed) {
                    match cmd_rx.try_recv() {
                        Ok(Cmd::Start(args)) => w.begin(*args),
                        Ok(Cmd::Stop) => {
                            let report = w.finish();
                            w.audio_tx = None;
                            let _ = report_tx.send(report);
                        }
                        // The tap can be replaced — a plugin swap does not
                        // rebuild the engine, but losing the engine does.
                        Ok(Cmd::Plugin(t)) => w.tap = t.map(|t| *t),
                        // Meaningless here: with no input there is only one
                        // thing this can be recording.
                        Ok(Cmd::Source(_)) => {}
                        // Meaningful here for exactly the same reason it is
                        // meaningful on the input writer: this thread's `pump`
                        // republishes its own latch every cycle, so clearing
                        // the shared meters alone would be undone in 4 ms.
                        Ok(Cmd::ClearClip) => w.tracker.clear_clip(),
                        Ok(Cmd::Quit) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {}
                    }
                    w.pump();
                    std::thread::sleep(POLL);
                }
                // A take still open when the app quits is finished rather than
                // abandoned, exactly as the input writer does it.
                if w.wav.is_some() {
                    let _ = w.finish();
                }
                // Same hand-back as the input writer's: the tap outlives the
                // thread that was holding it.
                if let Some(t) = w.tap.take() {
                    let _ = tap_home.send(Box::new(t));
                }
            })
            .ok();

        Self {
            channels: channels as u16,
            sample_rate,
            cmds: cmd_tx,
            reports: report_rx,
            meters,
            running,
            thread,
        }
    }

    fn spec(&self) -> WavSpec {
        WavSpec {
            sample_rate: self.sample_rate,
            channels: self.channels,
            format: TAKE_FORMAT,
        }
    }
}

impl Drop for PluginAudio {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        let _ = self.cmds.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// The writer thread's state, for a take with no input device.
struct PluginWriter {
    tap: Option<crate::instrument::RecorderTap>,
    timebase: Timebase,
    tracker: LevelTracker,
    meters: Arc<Mutex<AudioMeters>>,
    wav: Option<WavWriter>,
    buf: Vec<f32>,
    frames: u64,
    fit: RateFit,
    first_frame_ns: Option<Nanos>,
    dropped_at_arm: u64,
    error: Option<String>,
    audio_tx: Option<mpsc::Sender<AudioChunk>>,
    channels: u16,
    sample_rate: u32,
}

impl PluginWriter {
    fn begin(&mut self, args: StartArgs) {
        // Arm FIRST. The ring has been filling since the engine started —
        // right through the five-second plugin warm-up — so it holds audio from
        // before anybody played anything. Arming discards that AND resets the
        // loss counter, so the take begins with the note being played now.
        if let Some(tap) = self.tap.as_mut() {
            tap.arm();
        }
        self.dropped_at_arm = self.tap.as_ref().map_or(0, |t| t.dropped());
        self.tracker.arm();
        self.frames = 0;
        self.fit = RateFit::new();
        self.first_frame_ns = None;
        self.error = None;
        self.audio_tx = args.audio_tx;
        match WavWriter::create(&args.path, args.spec, &args.bext) {
            Ok(w) => self.wav = Some(w),
            Err(e) => self.error = Some(format!("could not create the audio file: {e}")),
        }
    }

    /// Drain the ring; write what came out if a take is running.
    ///
    /// **Drained either way**, which is the same rule the input path learned
    /// the hard way: a ring nobody reads fills up, and every frame it then
    /// refuses is counted as a loss forever after.
    fn pump(&mut self) {
        let Some(tap) = self.tap.as_mut() else {
            return;
        };
        self.buf.clear();
        tap.drain(&mut self.buf);
        if !self.buf.is_empty() {
            self.tracker.absorb(&self.buf);
            if self.wav.is_some() {
                // The instant the FIRST frame of the file happened. Read here
                // rather than at Start, because Start is when the button was
                // pressed and this is when audio actually began.
                if self.first_frame_ns.is_none() {
                    self.first_frame_ns = Some(self.timebase.now());
                }
                let frames = (self.buf.len() / usize::from(self.channels).max(1)) as u64;
                let first_frame = self.frames;
                // Taken out and put back so `write_samples` can borrow `self`
                // mutably while the samples are read.
                let block = std::mem::take(&mut self.buf);
                self.write_samples(&block, first_frame);
                self.buf = block;
                self.frames += frames;
                // One point per poll, which is 250 a second — the same order
                // the input's fit gets, and enough to measure a crystal.
                self.fit.push(self.frames, self.timebase.now());
            }
        }
        if let Ok(mut m) = self.meters.lock() {
            self.tracker.publish(&mut m);
        }
    }

    fn write_samples(&mut self, samples: &[f32], first_frame: u64) {
        let Some(wav) = self.wav.as_mut() else {
            return;
        };
        if let Err(e) = wav.write_interleaved(samples) {
            if self.error.is_none() {
                self.error = Some(format!("could not write the audio file: {e}"));
            }
        }
        // Same funnel, same guarantee as the input path: what the file gets,
        // the video gets, at the index the file wrote it at.
        if let Some(tx) = self.audio_tx.as_ref() {
            let _ = tx.send(AudioChunk {
                first_frame,
                samples: samples.to_vec(),
            });
        }
    }

    fn finish(&mut self) -> AudioReport {
        // One last pass, so the frames the engine rendered between the user
        // pressing Stop and this thread noticing are in the file.
        self.pump();
        if let Some(w) = self.wav.take() {
            if let Err(e) = w.finish() {
                if self.error.is_none() {
                    self.error = Some(format!("could not finish the audio file: {e}"));
                }
            }
        }
        let (clipped_samples, take_peak) = match self.meters.lock() {
            Ok(m) => (m.clipped_samples, m.loudest_take_peak()),
            Err(_) => (0, 0.0),
        };
        AudioReport {
            frames: self.frames,
            fit: std::mem::replace(&mut self.fit, RateFit::new()),
            // There is no such thing here: every frame carries its position by
            // construction, because this thread counts them as it writes them.
            unstamped: 0,
            // ONLY ring overflow. Rendered audio has no dropouts — the engine
            // produces every frame of it — so a non-zero number here means the
            // ring filled, which is a fault worth reporting and not a hole that
            // padding could have hidden.
            frames_dropped: self
                .tap
                .as_ref()
                .map_or(0, |t| t.dropped().saturating_sub(self.dropped_at_arm)),
            clipped_samples,
            take_peak,
            channels: self.channels,
            first_frame_ns: self.first_frame_ns,
            running: self.tap.is_some(),
            source: take::SourceReport {
                name: "instrument".to_owned(),
                anchor_ns: self.first_frame_ns,
                fitted_rate: self.fit.true_rate(),
                latency_ns: 0,
                latency_source: take::LatencySource::AssumedZero,
                observations: self.fit.observations(),
                jitter_ns: None,
            },
            error: self.error.take(),
        }
    }
}
