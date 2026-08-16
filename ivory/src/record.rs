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
use std::sync::atomic::{AtomicBool, Ordering};
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
    Quit,
}

struct StartArgs {
    path: PathBuf,
    spec: WavSpec,
    bext: Bext,
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

    /// Forgiving, and it resolves the one case the UI cannot: "plugin" with no
    /// plugin loaded records the input rather than recording silence, because a
    /// take of nothing is never what anybody meant.
    pub fn resolve(setting: &str, plugin_loaded: bool, input_open: bool) -> Self {
        let want = match setting {
            "plugin" => TakeSource::Plugin,
            "both" => TakeSource::Both,
            _ => TakeSource::Input,
        };
        match (want, plugin_loaded, input_open) {
            (TakeSource::Plugin, false, _) => TakeSource::Input,
            (TakeSource::Both, false, _) => TakeSource::Input,
            (TakeSource::Both, true, false) => TakeSource::Plugin,
            (TakeSource::Input, _, false) if plugin_loaded => TakeSource::Plugin,
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

/// The writer thread's own state.
struct Writer {
    sink: audio::CaptureSink,
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
                // The instrument, summed into the same block.
                //
                // The INPUT is the rate master here and the plugin follows it:
                // the file's length is decided by the device whose timestamps
                // built the timeline, and the plugin is fitted to it. When the
                // two are the same interface — the ordinary one-box piano rig —
                // they share a crystal and this is exact. On two separate
                // devices they drift, which is why `take.json` records which
                // source was the master.
                if self.source.uses_plugin() {
                    let frames = self.buf.len() / self.tracker.channels().max(1);
                    self.mix_plugin(frames);
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

    fn write_samples(&mut self, samples: &[f32]) {
        let Some(wav) = self.wav.as_mut() else {
            return;
        };
        if let Err(e) = wav.write_interleaved(samples) {
            if self.error.is_none() {
                self.error = Some(format!("could not write the audio file: {e}"));
            }
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
            frames_dropped: self
                .sink
                .stats()
                .frames_dropped()
                .saturating_sub(self.dropped_at_arm)
                + self.short_frames
                + self
                    .plugin
                    .as_ref()
                    .map_or(0, |t| t.dropped().saturating_sub(self.plugin_dropped_at_arm)),
            first_frame_ns: self.first_frame_ns,
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
    cmds: mpsc::Sender<Cmd>,
    reports: mpsc::Receiver<AudioReport>,
    meters: Arc<Mutex<AudioMeters>>,
    running: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Audio {
    fn open(selection: &InputSelection, timebase: Timebase) -> Result<Self, String> {
        let wish = audio::ConfigWish::default();
        // Three seconds of ring. The writer thread wakes every 4 ms, so this is
        // absurd headroom — and that is the point: the one thing that must
        // never happen is the ring filling because the machine hiccuped while
        // somebody was recording a take they cannot play again.
        let (stream, sink) =
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
        let meters = Arc::new(Mutex::new(AudioMeters::new(channels)));
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (report_tx, report_rx) = mpsc::channel();
        let running = Arc::new(AtomicBool::new(true));

        let mut writer = Writer {
            sink,
            tracker: LevelTracker::new(channels, f64::from(config.sample_rate)),
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
                            let _ = report_tx.send(report);
                        }
                        Ok(Cmd::Plugin(tap)) => writer.plugin = tap.map(|t| *t),
                        Ok(Cmd::Source(mode)) => writer.source = mode,
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
            })
            .map_err(|e| format!("could not start the recording thread: {e}"))?;

        Ok(Self {
            _stream: stream,
            device_name: config.device.clone(),
            channels: config.channels,
            sample_rate: config.sample_rate,
            cmds: cmd_tx,
            reports: report_rx,
            meters,
            running,
            thread: Some(thread),
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
        let Ok(m) = self.meters.lock() else {
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
    pub fn message(&self) -> String {
        if let Some(p) = &self.problem {
            // The folder still comes first when there IS one. A disk that
            // filled at 6:00 of a 10:00 take leaves six perfectly good minutes
            // on disk, and reporting only "No space left on device" sends the
            // user looking for a take they think was lost.
            return if self.folder.is_empty() {
                p.clone()
            } else {
                format!("{p}  —  what was recorded is in {}", self.folder)
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
            s.push_str("  —  WARNING: the audio is silent");
        } else if self.clipped {
            s.push_str("  —  the audio clipped");
        }
        if let Some(n) = &self.note {
            s.push_str("  —  ");
            s.push_str(n);
        }
        s
    }
}

/// The recorder, as the app holds it.
pub struct Session {
    timebase: Timebase,
    tap: Arc<RawMidiTap>,
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
}

impl Session {
    /// A session with no device open. Opening one is [`open_input`], which the
    /// app calls when the band appears rather than when Record is pressed.
    ///
    /// [`open_input`]: Session::open_input
    pub fn new(tap: Arc<RawMidiTap>, timebase: Timebase) -> Self {
        Self {
            timebase,
            tap,
            audio: None,
            audio_error: None,
            camera: None,
            camera_error: None,
            midi_clock: SourceClock::new(MIDIR_SCALE_NS, MIDI_SETTLE_NS),
            midi: MidiTake::new(),
            state: RecordState::Idle,
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

    pub fn clipped(&self) -> bool {
        self.clipped
    }

    pub fn audio_device_name(&self) -> Option<&str> {
        self.audio.as_ref().map(|a| a.device_name.as_str())
    }

    pub fn audio_error(&self) -> Option<&str> {
        self.audio_error.as_deref()
    }

    pub fn meters(&self) -> UiMeters {
        self.audio
            .as_ref()
            .map_or(UiMeters::SILENT, |a| a.levels())
    }

    /// Seconds since the take started writing.
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
    pub fn open_input(&mut self, selection: &InputSelection) {
        if self.state.is_active() {
            return; // never swap the device out from under a running take
        }
        self.audio = None; // close the old one FIRST; some drivers refuse two
        match Audio::open(selection, self.timebase) {
            Ok(a) => {
                self.audio = Some(a);
                self.audio_error = None;
            }
            Err(e) => self.audio_error = Some(e),
        }
    }

    /// Hand the monitor engine's recorder tap to the writer thread.
    ///
    /// Taken once and moved, because it is the read end of a lock-free ring and
    /// belongs to exactly one thread. `None` removes it.
    pub fn set_plugin_tap(&mut self, tap: Option<crate::instrument::RecorderTap>) {
        if let Some(audio) = &self.audio {
            let _ = audio.cmds.send(Cmd::Plugin(tap.map(Box::new)));
        }
    }

    /// Which sources the next take is made of. Ignored mid-take: a take that
    /// changed what it was recording halfway through would produce a file
    /// matching neither answer.
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

    /// How long one beat lasts at the take's tempo.
    fn beat(&self) -> Duration {
        Duration::from_secs_f64(60.0 / self.spec.tempo_bpm.clamp(20.0, 300.0))
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
        // 1-based: the first beat of a four-beat count-in is "1", not "0". The
        // number shown is the one the player is counting out loud.
        let now = RecordState::CountIn {
            beat: (done as u32) + 1,
            of: self.count_in_of,
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

    fn begin(&mut self, root: &std::path::Path, name: Option<&str>) {
        self.clipped = false;
        self.last = None;
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
        if self.spec.audio && self.audio.is_none() && self.source.uses_plugin() {
            self.pending_note = Some(
                "no audio input is open, so there is no clock to record the                  instrument against — the take is MIDI only. Choose an input                  in the Recorder band to record its sound."
                    .to_owned(),
            );
        }
        if let (Some(audio), true) = (&self.audio, self.spec.audio) {
            let spec = audio.spec();
            let args = StartArgs {
                path: take.wav(),
                spec,
                bext: Bext::new(at, spec),
            };
            let _ = audio.cmds.send(Cmd::Start(Box::new(args)));
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
        let timeline = match &report {
            Some(r) => Timeline::from_fit(t0, t1, nominal, &r.fit),
            None => Timeline::synthetic(t0, t1, nominal),
        };

        // ── MIDI ────────────────────────────────────────────────────────────
        let mut wrote_midi = false;
        if spec.midi && !self.midi.is_empty() {
            match self.midi.write(&timeline, spec.tempo_bpm, &take.midi()) {
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
                 microphone entitlement — run the probe from the signed .app:\n\
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
    session.open_input(&InputSelection::Default);
    match (session.audio_device_name(), session.audio_error()) {
        (Some(name), _) => println!("\nopen: {name}"),
        (None, Some(e)) => {
            eprintln!("could not open the default input: {e}");
            return;
        }
        (None, None) => {
            eprintln!("no input opened and no error reported — that is a bug");
            return;
        }
    }

    // Meter for a moment BEFORE arming, which is the behaviour that kills the
    // "I recorded silence" failure class — so it is the behaviour the probe
    // shows you rather than one it takes on trust.
    println!("\nlevel check (2s) — play something:");
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
        eprintln!("the take produced no summary — that is a bug");
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
    fn writer_with_ring(channels: usize) -> (audio::CaptureSource, Writer) {
        let stats = Arc::new(audio::CaptureStats::new());
        let (source, sink) = audio::capture_channel(channels, 48_000, 512, Arc::clone(&stats));
        let writer = Writer {
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
            "the file must hold exactly the frames the device produced — \
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
            "nothing may be created until the countdown finishes — a cancelled \
             count-in must leave no empty folder behind"
        );
        s.toggle(&dir, None, 3, ExportSpec::default());
        assert_eq!(s.state(), RecordState::Idle, "pressing again cancels");
        assert!(!dir.exists());
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
mod source_tests {
    use super::TakeSource;

    /// "Record the plugin" with no plugin loaded must not record silence. A
    /// take of nothing is never what anybody meant, and it is a setting that
    /// survives from a session where a plugin WAS loaded.
    #[test]
    fn asking_for_a_plugin_that_is_not_loaded_records_the_input_instead() {
        assert_eq!(
            TakeSource::resolve("plugin", false, true),
            TakeSource::Input
        );
        assert_eq!(TakeSource::resolve("both", false, true), TakeSource::Input);
    }

    /// And the mirror: with a plugin loaded and no input device open, the
    /// plugin is the only thing there is to record.
    #[test]
    fn with_no_input_open_a_loaded_plugin_is_the_take() {
        assert_eq!(
            TakeSource::resolve("input", true, false),
            TakeSource::Plugin
        );
        assert_eq!(TakeSource::resolve("both", true, false), TakeSource::Plugin);
    }

    #[test]
    fn an_ordinary_setup_gets_what_it_asked_for() {
        assert_eq!(TakeSource::resolve("input", true, true), TakeSource::Input);
        assert_eq!(TakeSource::resolve("plugin", true, true), TakeSource::Plugin);
        assert_eq!(TakeSource::resolve("both", true, true), TakeSource::Both);
        assert_eq!(
            TakeSource::resolve("nonsense from a later build", false, true),
            TakeSource::Input
        );
    }

    #[test]
    fn the_setting_round_trips() {
        for s in ["input", "plugin", "both"] {
            assert_eq!(TakeSource::resolve(s, true, true).to_setting(), s);
        }
    }
}
