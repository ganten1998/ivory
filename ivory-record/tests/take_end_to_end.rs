//! The first proof that the pieces compose: one take, on disk, no devices.
//!
//! `clock`, `smf`, `wav` and `take` were built separately — three of them
//! concurrently — against a shared design rather than against each other. Unit
//! tests prove each is correct in isolation; nothing until now has proven they
//! agree. This does, and it asserts the one contract the whole feature rests on:
//!
//! > **WAV sample 0, MIDI tick 0 and the take's start are the same instant.**
//!
//! Still no camera, no audio interface and no MIDI keyboard: the timeline is
//! synthesised, so this runs anywhere in milliseconds.

use ivory_record::clock::{Nanos, RateFit, Timeline, NS_PER_SEC};
use ivory_record::smf::{Captured, MidiTake};
use ivory_record::take::{Manifest, Take, WallTime};
use ivory_record::wav::{Bext, SampleFormat, WavSpec, WavWriter};

const SEC: Nanos = 1_000_000_000;
const RATE: u32 = 48_000;

/// A scratch directory that removes itself, named after the test so a failure
/// leaves one identifiable tree rather than a pile of anonymous ones.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("tangent-e2e-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("scratch root");
        Self(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An audio device running 50 ppm fast, which is an ordinary crystal.
fn drifting_fit(t0: Nanos, seconds: i64) -> RateFit {
    let true_rate = RATE as f64 * 1.000_05;
    let mut fit = RateFit::new();
    let block = 512u64;
    for k in 0..(seconds as f64 * true_rate / block as f64) as u64 {
        let sample = k * block;
        fit.push(sample, t0 + (sample as f64 / true_rate * NS_PER_SEC) as Nanos);
    }
    fit
}

#[test]
fn one_take_produces_a_directory_a_daw_can_open() {
    let scratch = Scratch::new("compose");
    let at = WallTime::from_unix(1_786_804_327, 0); // 2026-08-15T14:32:07Z

    // ── the take directory ──────────────────────────────────────────────────
    let take = Take::create(&scratch.0, &at, Some("nocturne in c#m")).expect("create take");
    assert!(
        take.name().starts_with("2026-08-15_143207"),
        "the folder must sort chronologically: {}",
        take.name()
    );
    assert!(
        !take.name().contains(':') && !take.name().contains(' '),
        "no colons (illegal on Windows) and no spaces (these get pasted into \
         shells): {}",
        take.name()
    );
    assert!(
        take.name().contains("nocturne"),
        "the typed name should survive sanitisation: {}",
        take.name()
    );

    // ── the timeline ────────────────────────────────────────────────────────
    let t0 = 987_654_321_000;
    let take_secs = 8;
    let t1 = t0 + take_secs * SEC;
    let timeline = Timeline::from_fit(t0, t1, RATE as f64, &drifting_fit(t0, take_secs));
    assert!(!timeline.is_synthetic(), "the fit should have been usable");
    assert!(
        (timeline.epsilon_ppm() - 50.0).abs() < 1.0,
        "the take should have measured its device at ~50 ppm fast, got {:.2}",
        timeline.epsilon_ppm()
    );

    // ── audio ───────────────────────────────────────────────────────────────
    let spec = WavSpec {
        sample_rate: RATE,
        channels: 2,
        format: SampleFormat::Int24,
    };
    let bext = Bext::new(at, spec);
    let mut wav = WavWriter::create(&take.wav(), spec, &bext).expect("create wav");
    // A quiet 220 Hz tone, so the file is not all zeros and a peak is meaningful.
    let frames = (take_secs as u32 * RATE) as usize;
    let mut block = Vec::with_capacity(2048);
    let mut written = 0usize;
    while written < frames {
        block.clear();
        let n = 1024.min(frames - written);
        for i in 0..n {
            let t = (written + i) as f64 / RATE as f64;
            let s = (t * 220.0 * std::f64::consts::TAU).sin() as f32 * 0.25;
            block.push(s);
            block.push(s);
        }
        wav.write_interleaved(&block).expect("write audio");
        written += n;
    }
    let audio_frames = wav.frames();
    wav.finish().expect("finish wav");

    // ── MIDI ────────────────────────────────────────────────────────────────
    let mut midi = MidiTake::new();
    // A pedal already down before Record, and a program change from connect
    // time: both must be restated at tick 0 or the file has no instrument and a
    // pedal release looks spontaneous.
    midi.push(Captured::new(t0 - 30 * SEC, [0xC0, 0]));
    midi.push(Captured::new(t0 - 2 * SEC, [0xB0, 64, 127]));
    for i in 0..4i64 {
        midi.push(Captured::new(t0 + i * SEC, [0x90, 60 + i as u8 * 4, 96]));
        midi.push(Captured::new(t0 + i * SEC + SEC / 2, [0x80, 60 + i as u8 * 4, 40]));
    }
    // Still held at Stop, so the stop sequence has to do something.
    midi.push(Captured::new(t0 + 7 * SEC, [0x90, 72, 100]));
    midi.write(&timeline, 120.0, &take.midi()).expect("write midi");

    // ── the manifest ────────────────────────────────────────────────────────
    let mut manifest = Manifest::starting(take.name(), at, t0);
    manifest.apply_timeline(&timeline);
    manifest.finish();
    manifest.write(take.dir()).expect("write manifest");

    // ── everything is on disk and non-trivial ───────────────────────────────
    for path in [take.wav(), take.midi(), take.manifest_path()] {
        let meta = std::fs::metadata(&path)
            .unwrap_or_else(|e| panic!("{} missing: {e}", path.display()));
        assert!(meta.len() > 0, "{} is empty", path.display());
    }

    // ── the WAV is a real WAV, with a bext ──────────────────────────────────
    let bytes = std::fs::read(take.wav()).expect("read wav");
    assert_eq!(&bytes[0..4], b"RIFF");
    assert_eq!(&bytes[8..12], b"WAVE");
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64;
    assert_eq!(
        declared + 8,
        bytes.len() as u64,
        "the RIFF size field must describe the file that is actually there — \
         this is the field that reads 0 on a crashed take"
    );
    let bext_at = bytes
        .windows(4)
        .position(|w| w == b"bext")
        .expect("no bext chunk: the whole point of not using hound");
    // TimeReference sits 338 bytes into the chunk body, which starts 8 after
    // the fourcc: Description 256 + Originator 32 + OriginatorReference 32 +
    // OriginationDate 10 + OriginationTime 8. Written out rather than left as a
    // magic number, because getting it wrong is how you read four bytes of
    // somebody else's field and place the take in 1970.
    let tr_off = bext_at + 8 + 256 + 32 + 32 + 10 + 8;
    let lo = u32::from_le_bytes(bytes[tr_off..tr_off + 4].try_into().unwrap()) as u64;
    let hi = u32::from_le_bytes(bytes[tr_off + 4..tr_off + 8].try_into().unwrap()) as u64;
    assert_eq!(
        (hi << 32) | lo,
        at.samples_since_midnight(RATE),
        "TimeReference is what makes a DAW place the take at the time it was \
         played; a wrong one is silently wrong"
    );

    // ── the MIDI is a real SMF, and agrees with the WAV about time ──────────
    let midi_bytes = std::fs::read(take.midi()).expect("read midi");
    assert_eq!(&midi_bytes[0..4], b"MThd");
    let smf = midly::Smf::parse(&midi_bytes).expect("our own SMF must parse");
    assert_eq!(smf.tracks.len(), 2, "format 1: tempo map, then performance");

    // THE CONTRACT: the last event sits at the take's end, and the take's end is
    // the audio's length. If these disagree, the .mid and the .wav disagree
    // about how long the performance was.
    let end_tick: u64 = smf.tracks[1].iter().map(|e| e.delta.as_int() as u64).sum();
    let ticks_per_sec = ivory_record::smf::PPQ as f64 * 2.0; // 120 BPM
    let midi_end_s = end_tick as f64 / ticks_per_sec;
    let audio_end_s = audio_frames as f64 / RATE as f64;
    assert!(
        (midi_end_s - audio_end_s).abs() < 0.005,
        "MIDI ends at {midi_end_s:.4}s and audio at {audio_end_s:.4}s — the two \
         deliverables must agree about the take's duration"
    );

    // The pedal that was down before Record was restated at tick 0.
    let mut abs = 0u64;
    let mut zero_events = Vec::new();
    for ev in &smf.tracks[1] {
        abs += ev.delta.as_int() as u64;
        if abs == 0 {
            zero_events.push(format!("{:?}", ev.kind));
        }
    }
    let at_zero = zero_events.join("|");
    assert!(
        at_zero.contains("ProgramChange"),
        "no instrument at tick 0: {at_zero}"
    );
    assert!(
        at_zero.contains("Controller"),
        "the pedal that was already down was not restated: {at_zero}"
    );

    // ── the manifest says the take completed ────────────────────────────────
    let manifest_text = std::fs::read_to_string(take.manifest_path()).expect("read manifest");
    assert!(
        manifest_text.contains("\"complete\""),
        "the crash detector must be present"
    );
    assert!(
        ivory_record::take::is_complete(take.dir()),
        "a cleanly finished take must not read as crashed"
    );
}

#[test]
fn a_take_interrupted_before_anything_is_finished_is_still_readable() {
    // The scenario the whole crash-safety design exists for: the process dies
    // mid-take. Nothing is finished, nothing is closed, no destructor is
    // trusted to have run. What is on disk must still be openable.
    let scratch = Scratch::new("crash");
    let at = WallTime::from_unix(1_786_804_327, 0);
    let take = Take::create(&scratch.0, &at, None).expect("create take");

    let spec = WavSpec::default();
    let mut wav =
        WavWriter::create(&take.wav(), spec, &Bext::new(at, spec)).expect("create wav");
    let block = vec![0.1f32; 2 * 48_000];
    wav.write_interleaved(&block).expect("write");
    wav.patch_sizes().expect("patch");

    // Read it back WITHOUT finishing, and with the writer still holding the
    // handle — which is exactly the state a SIGKILL leaves.
    let bytes = std::fs::read(take.wav()).expect("read wav");
    let declared = u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as u64;
    assert!(
        declared > 1000,
        "an unfinished take must not claim size 0; that is the file that plays \
         as empty and loses the performance"
    );
    assert!(
        declared + 8 <= bytes.len() as u64,
        "the header must never claim more bytes than reached the disk"
    );

    // And the take is detectably incomplete, so a later launch can offer it.
    let manifest = Manifest::starting(take.name(), at, 0);
    manifest.write(take.dir()).expect("write manifest");
    assert!(
        !ivory_record::take::is_complete(take.dir()),
        "an unfinished take must read as crashed — that flag is the recovery \
         path's only input"
    );
}
