//! Does the exported audio actually contain the notes the exported MIDI claims?
//!
//! This is RECORDER-PLAN §11's "Test B" in miniature, and it is the only test in
//! the tree that answers the product's real question rather than a component's.
//! Everything else proves that a module computes what it was asked to; this
//! proves that a **take** is internally consistent — that the `.wav` has an
//! attack at every instant the `.mid` says a chord was played.
//!
//! # Why it is `#[ignore]`d
//!
//! It needs a real VST3 instrument installed, which no CI machine has and no
//! other developer is guaranteed to have. Run it deliberately:
//!
//! ```text
//! cargo test -p ivory-host --test plugin_take_sync -- --ignored --nocapture
//! ```
//!
//! It follows the repo's existing convention for expensive or hardware-bound
//! suites (`ivory-core/tests/blast_radius.rs`).
//!
//! # Why an energy ratio rather than an onset detector
//!
//! The obvious check — find onsets, compare their times to the MIDI — does not
//! work on a piano and the failure is instructive. A chord is voiced with a few
//! milliseconds of spread between fingers, sympathetic strings keep ringing, and
//! a naive rising-edge detector fires on beating between partials long after the
//! attack. Measured here, it reported six "onsets" in the first second of a
//! four-chord progression.
//!
//! So the question is asked the other way round, which is both easier and
//! stronger: **at each instant the MIDI names, did the energy jump?** A note that
//! is late, early, or missing all fail it, and a ringing chord does not.

use ivory_host::{Instance, Module, Note, Setup};
use ivory_record::clock::{Nanos, Timeline};
use ivory_record::smf::{Captured, MidiTake};
use ivory_record::take::{Take, WallTime};
use ivory_record::wav::{Bext, SampleFormat, WavSpec, WavWriter};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;
const SEC: Nanos = 1_000_000_000;

/// Chord start times, in milliseconds, deliberately not on block boundaries.
///
/// 913 ms is 43,824 frames, which is 85.59 blocks of 512. If anything in the
/// chain places events by rounding to a block instead of by the clock, these
/// land up to 5 ms out and this test is what notices.
const CHORDS_MS: [i64; 4] = [0, 913, 1_847, 2_791];

/// Which instrument to test with.
///
/// Overridable with `TANGENT_TEST_VST3=<substring>`, because "the first Audio
/// Module Class found" is a terrible default: on this machine it is `Ample Bass
/// J`, and a bass sampler asked to play a C5 chord legitimately produces very
/// little. A test that fails because it chose an unsuitable instrument teaches
/// nothing about sync.
///
/// The preference order is a piano, then anything — a piano being both the
/// product's subject and the instrument most likely to sound on any pitch.
fn instrument() -> Option<(Module, ivory_host::ClassInfo)> {
    let wanted = std::env::var("TANGENT_TEST_VST3").ok();
    let bundles = ivory_host::discover();
    let name_of = |p: &std::path::Path| {
        p.file_name().unwrap_or_default().to_string_lossy().to_lowercase()
    };

    let mut order: Vec<std::path::PathBuf> = Vec::new();
    if let Some(w) = &wanted {
        let w = w.to_lowercase();
        order.extend(bundles.iter().filter(|p| name_of(p).contains(&w)).cloned());
    } else {
        for hint in ["pianoteq", "piano", "grand", "keyscape"] {
            let hits: Vec<_> = bundles
                .iter()
                .filter(|p| name_of(p).contains(hint) && !order.contains(p))
                .cloned()
                .collect();
            order.extend(hits);
        }
        let rest: Vec<_> = bundles
            .iter()
            .filter(|p| !order.contains(p))
            .cloned()
            .collect();
        order.extend(rest);
    }

    for bundle in order {
        let Ok(module) = Module::open(&bundle) else {
            continue;
        };
        if let Some(class) = module.audio_modules().into_iter().next() {
            return Some((module, class));
        }
    }
    None
}

#[test]
#[ignore = "needs a real VST3 instrument installed; run with --ignored"]
fn the_exported_audio_has_an_attack_at_every_instant_the_exported_midi_names() {
    let Some((module, class)) = instrument() else {
        panic!("no VST3 instrument found; this test cannot run on this machine");
    };
    eprintln!("instrument: {} [{}]", class.name, module.vendor());

    let setup = Setup {
        sample_rate: RATE as f64,
        max_block: BLOCK as i32,
    };
    let mut inst = Instance::create(&module, &class, setup).expect("instantiate");
    let channels = inst
        .audio_outputs()
        .first()
        .map(|b| b.channels)
        .unwrap_or(2)
        .max(1) as usize;
    let mut bufs: Vec<Vec<f32>> = vec![Vec::new(); channels];

    // Four of six instruments on the author's machine render SILENCE if
    // recorded cold (RECORDER-PLAN §8, spike 2). Without this the test fails
    // for a reason that has nothing to do with sync.
    for _ in 0..(5.0 * RATE as f64 / BLOCK as f64) as usize {
        inst.process(&[], BLOCK, &mut bufs).expect("warm-up");
    }

    let dir = std::env::temp_dir().join("tangent-sync-test");
    let _ = std::fs::remove_dir_all(&dir);
    let at = WallTime::from_unix(1_786_804_327, 0);
    let take = Take::create(&dir, &at, Some("sync")).expect("create take");

    let t0: Nanos = 1_000_000_000;
    let t1 = t0 + 4_200 * (SEC / 1_000);
    let timeline = Timeline::synthetic(t0, t1, RATE as f64);

    // ── the performance ─────────────────────────────────────────────────────
    let mut midi = MidiTake::new();
    let mut events: Vec<(Nanos, u8, u8, bool)> = Vec::new();
    for (c, ms) in CHORDS_MS.iter().enumerate() {
        for (i, p) in [48u8 + c as u8, 60, 64, 67, 72].iter().enumerate() {
            let on = t0 + ms * (SEC / 1_000) + i as i64 * 4 * (SEC / 1_000);
            let off = on + 800 * (SEC / 1_000);
            events.push((on, *p, 90, true));
            events.push((off, *p, 64, false));
            midi.push(Captured::new(on, [0x90, *p, 90]));
            midi.push(Captured::new(off, [0x80, *p, 64]));
        }
    }
    events.sort_by_key(|e| e.0);

    // ── render ──────────────────────────────────────────────────────────────
    let spec = WavSpec {
        sample_rate: RATE,
        channels: channels as u16,
        format: SampleFormat::Int24,
    };
    let mut wav = WavWriter::create(&take.wav(), spec, &Bext::new(at, spec)).expect("wav");
    let total = (timeline.duration_seconds() * RATE as f64) as usize;
    let mut frame = 0usize;
    let mut next = 0usize;
    let mut il: Vec<f32> = Vec::new();
    let mut mono: Vec<f32> = Vec::with_capacity(total);

    while frame < total {
        let n = BLOCK.min(total - frame);
        let end = frame + n;
        let mut notes = Vec::new();
        while next < events.len() {
            let pos = timeline.file_sample(events[next].0);
            if pos < 0.0 {
                next += 1;
                continue;
            }
            let pos = pos as usize;
            if pos >= end {
                break;
            }
            let (_, pitch, vel, on) = events[next];
            notes.push(Note {
                offset: pos.saturating_sub(frame) as i32,
                pitch: i16::from(pitch),
                velocity: f32::from(vel) / 127.0,
                on,
            });
            next += 1;
        }
        inst.process(&notes, n, &mut bufs).expect("process");
        il.clear();
        for i in 0..n {
            mono.push(bufs[0][i]);
            for ch in bufs.iter().take(channels) {
                il.push(ch[i]);
            }
        }
        wav.write_interleaved(&il).expect("write");
        frame = end;
    }
    wav.finish().expect("finish");
    midi.write(&timeline, 120.0, &take.midi()).expect("write midi");

    // ── the assertion ───────────────────────────────────────────────────────
    let rms = |a: isize, b: isize| -> f64 {
        let a = a.max(0) as usize;
        let b = (b.max(0) as usize).min(mono.len());
        if b <= a {
            return 0.0;
        }
        let s: f64 = mono[a..b].iter().map(|v| (*v as f64) * (*v as f64)).sum();
        (s / (b - a) as f64).sqrt()
    };

    let mut failures = Vec::new();
    for ms in CHORDS_MS {
        let f = (ms as f64 / 1000.0 * RATE as f64) as isize;
        let win = (0.055 * RATE as f64) as isize;
        let guard = (0.005 * RATE as f64) as isize;
        let before = rms(f - win, f - guard);
        let after = rms(f + guard, f + win);

        // A chord must be audible after its own start...
        if after < 0.01 {
            failures.push(format!("{ms}ms: no audio after the note ({after:.5} RMS)"));
            continue;
        }
        // ...and louder than just before it. `before` is not silence except at
        // the very first chord: the previous chord is still ringing, which is
        // exactly why the threshold is a RATIO and not an absolute level.
        if before > 1e-6 && after / before < 2.0 {
            failures.push(format!(
                "{ms}ms: no attack — {before:.5} before, {after:.5} after \
                 (ratio {:.2}, needed 2.0)",
                after / before
            ));
        }
        eprintln!(
            "  {ms:>5}ms  before {before:.5}  after {after:.5}  ratio {:.1}",
            if before > 1e-9 { after / before } else { f64::INFINITY }
        );
    }
    assert!(
        failures.is_empty(),
        "the .wav and the .mid disagree about when notes happened:\n  {}",
        failures.join("\n  ")
    );

    let _ = std::fs::remove_dir_all(&dir);
}
