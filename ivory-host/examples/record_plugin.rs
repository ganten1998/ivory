//! The first complete take: a performance played into a hosted instrument,
//! recorded to a real take directory.
//!
//! ```text
//! cargo run -p ivory-host --example record_plugin -- Pianoteq
//! ```
//!
//! This is the moment the two lanes stop being separate work. It exercises the
//! whole chain end to end — plugin hosting, the timebase, the WAV writer with
//! its BWF chunk, the SMF writer with its stop sequence, the take directory and
//! the manifest — and produces a folder a DAW can open.
//!
//! The performance is synthetic (no keyboard required) but it is *timed* like a
//! real one: events carry host-clock timestamps and are converted to sample
//! offsets through the same `Timeline` the exported files use, rather than being
//! placed at convenient block boundaries. That is the part worth proving. A demo
//! that fires every note at offset 0 of some block proves nothing about sync,
//! and sync is the whole feature.

use ivory_host::{Instance, Module, Note, Setup};
use ivory_record::clock::{Nanos, Timeline};
use ivory_record::smf::{Captured, MidiTake};
use ivory_record::take::{Manifest, Take, WallTime};
use ivory_record::wav::{Bext, SampleFormat, WavSpec, WavWriter};

const RATE: u32 = 48_000;
const BLOCK: usize = 512;
const SEC: Nanos = 1_000_000_000;

/// One note of the synthetic performance: pitch, when it starts, how long.
struct Played {
    pitch: u8,
    at_ms: i64,
    len_ms: i64,
    velocity: u8,
}

/// A ii-V-I in C, voiced in both hands, played with human-ish timing — the
/// offsets are deliberately not multiples of the 512-frame block, so every
/// event has to be placed by the clock rather than by rounding.
fn performance() -> Vec<Played> {
    let mut out = Vec::new();
    let chords: [(&[u8], i64); 4] = [
        (&[50, 62, 65, 69, 72], 0),      // Dm9
        (&[43, 62, 65, 67, 71], 913),    // G13
        (&[48, 64, 67, 71, 74], 1_847),  // Cmaj9
        (&[36, 48, 55, 64, 67], 2_791),  // C, wide
    ];
    for (notes, at) in chords {
        for (i, p) in notes.iter().enumerate() {
            out.push(Played {
                pitch: *p,
                // Spread each chord slightly, as hands do.
                at_ms: at + i as i64 * 7,
                len_ms: 880,
                velocity: 72 + (i as u8 * 6),
            });
        }
    }
    out
}

fn main() {
    let filter = std::env::args().nth(1).unwrap_or_else(|| "Pianoteq".into());

    let Some(bundle) = ivory_host::discover().into_iter().find(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase().contains(&filter.to_lowercase()))
            .unwrap_or(false)
    }) else {
        eprintln!("no VST3 matching {filter:?}");
        std::process::exit(1);
    };

    let module = Module::open(&bundle).expect("open module");
    let class = module.audio_modules().into_iter().next().expect("no instrument");
    println!("instrument: {} [{}]", class.name, module.vendor());

    let setup = Setup { sample_rate: RATE as f64, max_block: BLOCK as i32 };
    let mut inst = Instance::create(&module, &class, setup).expect("instantiate");
    let channels = inst.audio_outputs().first().map(|b| b.channels).unwrap_or(2).max(1) as usize;
    let mut bufs: Vec<Vec<f32>> = vec![Vec::new(); channels];

    // ── warm-up ─────────────────────────────────────────────────────────────
    // Four of six instruments on this machine render SILENCE if recorded cold
    // (RECORDER-PLAN §8, spike 2). This is the crude version of what
    // `ready.rs` will do properly; without it this example produces a silent
    // take from most of the library and looks like a hosting bug.
    let warm_blocks = (5.0 * RATE as f64 / BLOCK as f64) as usize;
    for _ in 0..warm_blocks {
        inst.process(&[], BLOCK, &mut bufs).expect("warm-up");
    }
    println!("warmed up 5s before arming");

    // ── the take ────────────────────────────────────────────────────────────
    let root = std::env::temp_dir().join("Tangent");
    let at = WallTime::from_unix(ivory_record::take::unix_now_seconds(), 0);
    let take = Take::create(&root, &at, Some(&format!("{} demo", class.name)))
        .expect("create take");
    println!("take:       {}", take.dir().display());

    // T0 is the arm instant. Everything below is expressed against it, and the
    // exported files agree because they all go through this one `Timeline`.
    let t0: Nanos = 1_000_000_000;
    let take_ms = 4_200i64;
    let t1 = t0 + take_ms * (SEC / 1_000);
    // Synthetic: this render is not clocked by a real device, so there is no
    // crystal to have drifted and epsilon is zero BY DEFINITION rather than by
    // assumption. `Timeline::synthetic` is the honest constructor for that, and
    // the manifest will say `"clock": "synthetic"` rather than reporting a fit
    // that was never made.
    let timeline = Timeline::synthetic(t0, t1, RATE as f64);

    // ── flatten the performance into timestamped MIDI ───────────────────────
    let mut midi = MidiTake::new();
    let mut events: Vec<(Nanos, u8, u8, bool)> = Vec::new();
    for p in performance() {
        let on = t0 + p.at_ms * (SEC / 1_000);
        let off = on + p.len_ms * (SEC / 1_000);
        events.push((on, p.pitch, p.velocity, true));
        events.push((off, p.pitch, 64, false));
        midi.push(Captured::new(on, [0x90, p.pitch, p.velocity]));
        midi.push(Captured::new(off, [0x80, p.pitch, 64]));
    }
    events.sort_by_key(|e| e.0);

    // ── render, block by block, placing events by the clock ─────────────────
    let spec = WavSpec { sample_rate: RATE, channels: channels as u16, format: SampleFormat::Int24 };
    let mut wav = WavWriter::create(&take.wav(), spec, &Bext::new(at, spec)).expect("wav");

    let total_frames = (timeline.duration_seconds() * RATE as f64) as usize;
    let mut frame = 0usize;
    let mut next = 0usize;
    let mut interleaved: Vec<f32> = Vec::with_capacity(BLOCK * channels);
    let mut peak = 0.0f32;

    while frame < total_frames {
        let n = BLOCK.min(total_frames - frame);
        let block_end = frame + n;

        // Every event whose sample position falls inside this block, placed at
        // its exact offset. `file_sample` is the same function that decides
        // where the event lands in the .mid, so the audio and the MIDI cannot
        // disagree about when a note happened.
        let mut notes: Vec<Note> = Vec::new();
        while next < events.len() {
            let pos = timeline.file_sample(events[next].0);
            if pos < 0.0 {
                next += 1;
                continue;
            }
            let pos = pos as usize;
            if pos >= block_end {
                break;
            }
            let (_, pitch, vel, on) = events[next];
            notes.push(Note {
                offset: pos.saturating_sub(frame) as i32,
                pitch: i16::from(pitch),
                // VST3 velocity is a float 0..=1, not a MIDI byte. Passing 72
                // here would be fortissimo and clipped.
                velocity: f32::from(vel) / 127.0,
                on,
            });
            next += 1;
        }

        inst.process(&notes, n, &mut bufs).expect("process");

        interleaved.clear();
        for i in 0..n {
            for ch in bufs.iter().take(channels) {
                let s = ch[i];
                peak = peak.max(s.abs());
                interleaved.push(s);
            }
        }
        wav.write_interleaved(&interleaved).expect("write audio");
        frame = block_end;
    }
    let frames_written = wav.frames();
    wav.finish().expect("finish wav");

    // ── the other two deliverables ──────────────────────────────────────────
    midi.write(&timeline, 120.0, &take.midi()).expect("write midi");

    let mut manifest = Manifest::starting(take.name(), at, t0);
    manifest.apply_timeline(&timeline);
    manifest.finish();
    manifest.write(take.dir()).expect("write manifest");

    // ── report ──────────────────────────────────────────────────────────────
    println!("\n{}", take.name());
    for entry in std::fs::read_dir(take.dir()).expect("read take dir") {
        let e = entry.expect("entry");
        let size = e.metadata().map(|m| m.len()).unwrap_or(0);
        println!("  {:<48} {:>10} bytes", e.file_name().to_string_lossy(), size);
    }
    println!(
        "\naudio: {frames_written} frames ({:.2}s), peak {peak:.4}",
        frames_written as f64 / RATE as f64
    );
    if peak < 1e-6 {
        println!("SILENT TAKE — the instrument was not ready.");
        std::process::exit(2);
    }
    println!("play it:  afplay {}", take.wav().display());
}
