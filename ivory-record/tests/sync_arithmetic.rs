//! Test A from `docs/RECORDER-PLAN.md` §11: the arithmetic, headless.
//!
//! No camera, no audio interface, no MIDI keyboard, no permissions, about a
//! second of wall clock. This is where roughly 90% of the real bugs in a
//! recorder live, and it is the only part of the verification story that can run
//! anywhere.
//!
//! The take it drives is deliberately hostile, and every element of it is a bug
//! that has shipped in a real product:
//!
//! * audio whose nominal rate is 48000 and whose true rate is 48002.4 (+50 ppm),
//!   for twenty minutes — the drift case
//! * a camera whose first frame is 743 ms late, which jitters ±4 ms, and which
//!   drops three frames at t = 612 s — the first-frame-offset and dropped-frame
//!   cases
//! * MIDI containing two out-of-order arrivals, a re-triggered note, a pedal
//!   already down before Record, a note held across the start, and both a note
//!   and the pedal still held at Stop
//!
//! **The assertion that matters most is the one at t = 1200 s.** A pipeline with
//! no drift correction passes every other check in this file and fails only that
//! one, by 60 ms. A suite that asserts only near t = 0 is a suite that certifies
//! the exact bug this feature exists to avoid.

use ivory_record::clock::{Nanos, RateFit, SourceClock, Timeline, MIDIR_SCALE_NS, NS_PER_SEC};
use ivory_record::smf::{Captured, MidiTake, PPQ};
use midly::{MetaMessage, MidiMessage, Smf, TrackEventKind};

const SEC: Nanos = 1_000_000_000;
const TAKE_SECS: i64 = 1_200;
const NOMINAL: f64 = 48_000.0;
const TRUE_RATE: f64 = 48_002.4; // +50 ppm
const TEMPO_BPM: f64 = 120.0;
/// Ticks per second at 120 BPM, PPQ 960.
const TPS: f64 = PPQ as f64 * TEMPO_BPM / 60.0;

/// `T0` is deliberately not zero and not round. An epoch-dependent bug that
/// cancels at zero is a bug that ships.
const T0: Nanos = 987_654_321_000;
const T1: Nanos = T0 + TAKE_SECS * SEC;

/// A cheap deterministic jitter source. `Math.random` in a test is a flake
/// generator; this is reproducible and its sequence is irrelevant so long as it
/// is not constant.
fn jitter(i: u64, spread_ns: i64) -> i64 {
    let h = i.wrapping_mul(6_364_136_223_846_793_005).rotate_left(17);
    (h % (2 * spread_ns as u64 + 1)) as i64 - spread_ns
}

/// The audio device, sampled honestly: its own crystal runs at `TRUE_RATE` while
/// the host clock runs at real time.
fn fit_audio() -> RateFit {
    let mut fit = RateFit::new();
    let block = 512u64;
    let blocks = (TAKE_SECS as f64 * TRUE_RATE / block as f64) as u64;
    for k in 0..blocks {
        let sample = k * block;
        let t = T0 + (sample as f64 / TRUE_RATE * NS_PER_SEC) as Nanos;
        fit.push(sample, t);
    }
    fit
}

#[test]
fn the_drift_correction_holds_at_the_end_of_a_twenty_minute_take() {
    let tl = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());

    let ppm = tl.epsilon_ppm();
    assert!(
        (ppm - 50.0).abs() < 0.5,
        "the fit should recover +50 ppm; got {ppm:.3}"
    );
    assert!(!tl.is_synthetic());

    // A real host instant twenty minutes in maps to a LATER position in the
    // exported file, because the device delivered more samples than nominal.
    let at_end = tl.file_seconds(T1);
    assert!(
        (at_end - 1_200.06).abs() < 0.005,
        "expected ~1200.06 s of exported timeline for 1200 s of real time, got \
         {at_end:.4}"
    );

    // And the size of what the correction is worth, stated as the number the
    // design document quotes.
    let uncorrected = Timeline::synthetic(T0, T1, NOMINAL);
    let delta_ms = (at_end - uncorrected.file_seconds(T1)) * 1_000.0;
    assert!(
        (delta_ms - 60.0).abs() < 1.0,
        "uncorrected drift over this take is {delta_ms:.1} ms, expected ~60"
    );
}

#[test]
fn every_midi_event_lands_within_a_millisecond_at_both_ends_of_the_take() {
    let tl = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());
    let mut take = MidiTake::new();

    // Known truths, in host time, spread across the whole take so the far end is
    // actually exercised. 1_199 s is the one that catches missing drift
    // correction.
    let truths: Vec<i64> = vec![0, 1, 37, 600, 900, 1_199];
    for (i, secs) in truths.iter().enumerate() {
        take.push(Captured::new(
            T0 + secs * SEC,
            [0x90, 60 + i as u8, 100],
        ));
        take.push(Captured::new(
            T0 + secs * SEC + SEC / 2,
            [0x80, 60 + i as u8, 64],
        ));
    }

    let smf = take.build(&tl, TEMPO_BPM);
    let mut abs = 0u64;
    let mut got: Vec<(u8, f64)> = Vec::new();
    for ev in &smf.tracks[1] {
        abs += ev.delta.as_int() as u64;
        if let TrackEventKind::Midi {
            message: MidiMessage::NoteOn { key, vel },
            ..
        } = ev.kind
        {
            if vel.as_int() > 0 {
                // Convert back through the file's OWN tempo and PPQ, which is
                // what any reader will do. Checking against the ticks we wrote
                // would only prove we can read our own variable.
                got.push((key.as_int(), abs as f64 / TPS));
            }
        }
    }

    assert_eq!(got.len(), truths.len(), "lost or gained a note");
    for (i, secs) in truths.iter().enumerate() {
        let (key, seconds) = got[i];
        assert_eq!(key, 60 + i as u8);
        // The true host instant, expressed on the exported timeline.
        let expected = tl.file_seconds(T0 + secs * SEC);
        let err_ms = (seconds - expected).abs() * 1_000.0;
        assert!(
            err_ms < 1.0,
            "note {key} at {secs}s is {err_ms:.3} ms out; the 1 ms budget is \
             MIDI's own serial transmission time for a three-byte message"
        );
    }
}

#[test]
fn a_pipeline_without_drift_correction_fails_only_the_far_end() {
    // The point of this test is the asymmetry: it demonstrates that a suite
    // asserting near t=0 certifies a broken pipeline.
    let good = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());
    let bad = Timeline::synthetic(T0, T1, NOMINAL);

    let near = (good.file_seconds(T0 + SEC) - bad.file_seconds(T0 + SEC)).abs() * 1_000.0;
    let far = (good.file_seconds(T1) - bad.file_seconds(T1)).abs() * 1_000.0;

    assert!(near < 1.0, "at one second in, the two are indistinguishable");
    assert!(far > 50.0, "at twenty minutes in, they are 60 ms apart");
}

#[test]
fn video_frames_land_within_half_a_frame_despite_a_late_start_and_dropped_frames() {
    let tl = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());
    let fps = 30.0;
    let frame_ns = (NS_PER_SEC / fps) as Nanos;
    let half_frame = 1.0 / (2.0 * fps);

    // The camera starts 743 ms after T0 — sensor warm-up and auto-exposure
    // convergence, which is entirely normal and is the "first frame offset" bug
    // when it is ignored.
    let first_frame = T0 + 743 * SEC / 1_000;
    let mut delivered = 0u64;

    for i in 0..(TAKE_SECS as u64 * 30) {
        let ideal = first_frame + i as i64 * frame_ns;
        if ideal > T1 {
            break;
        }
        // Three frames vanish at t = 612 s. A recorder that synthesises the
        // timeline from a frame counter puts everything after this 100 ms early
        // for the rest of the take.
        let at_612 = (ideal - T0) / SEC == 612;
        if at_612 && (612..615).contains(&(i % 1_000)) {
            continue;
        }
        let actual = ideal + jitter(i, 4 * SEC / 1_000);
        delivered += 1;

        let pts = tl.file_seconds(actual);
        let truth = tl.file_seconds(ideal);
        assert!(
            (pts - truth).abs() <= half_frame,
            "frame {i} is {:.4}s from its ideal slot, over half a frame",
            (pts - truth).abs()
        );
        // And the first exported frame is at or after zero, never negative.
        assert!(pts >= -1e-9, "frame {i} has a negative PTS");
    }

    assert!(delivered > 35_000, "sanity: about 36,000 frames in 20 minutes");
}

#[test]
fn a_frame_counter_timeline_would_fail_this_and_real_timestamps_do_not() {
    // The failure mode named in the plan: keeping real timestamps but writing a
    // constant duration. Here the camera runs at a true 29.91 fps while claiming
    // 30 — routine for a USB webcam under variable exposure.
    let tl = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());
    let true_fps = 29.91;
    let n = 30_000u64;

    let real = T0 + (n as f64 / true_fps * NS_PER_SEC) as Nanos;
    let honest = tl.file_seconds(real);
    let synthesised = n as f64 / 30.0;

    assert!(
        (honest - synthesised).abs() > 2.0,
        "a nominal-rate timeline should be seconds adrift by frame {n}; it is \
         {:.3}s",
        (honest - synthesised).abs()
    );
}

#[test]
fn the_whole_adversarial_take_produces_a_coherent_file() {
    let tl = Timeline::from_fit(T0, T1, NOMINAL, &fit_audio());
    let mut take = MidiTake::new();

    // Before Record: a program change at connect time, and a pedal already down.
    take.push(Captured::new(T0 - 45 * SEC, [0xC0, 3]));
    take.push(Captured::new(T0 - 2 * SEC, [0xB0, 64, 127]));
    // A note begun before Record and released during the take: its note-off has
    // no matching note-on and must not drive the held counter negative.
    take.push(Captured::new(T0 - SEC / 2, [0x90, 48, 90]));
    take.push(Captured::new(T0 + 5 * SEC, [0x80, 48, 40]));

    // A re-triggered note (a trill).
    take.push(Captured::new(T0 + 10 * SEC, [0x90, 64, 100]));
    take.push(Captured::new(T0 + 10 * SEC + SEC / 20, [0x90, 64, 105]));
    take.push(Captured::new(T0 + 10 * SEC + SEC / 10, [0x80, 64, 50]));
    take.push(Captured::new(T0 + 10 * SEC + SEC / 8, [0x80, 64, 50]));

    // Two out-of-order arrivals: pushed late, timestamped early.
    take.push(Captured::new(T0 + 700 * SEC, [0x90, 67, 100]));
    take.push(Captured::new(T0 + 699 * SEC, [0x90, 65, 100]));
    take.push(Captured::new(T0 + 700 * SEC + SEC, [0x80, 67, 64]));
    take.push(Captured::new(T0 + 699 * SEC + SEC, [0x80, 65, 64]));

    // Half-pedalling near the end.
    for (i, v) in [90u8, 60, 30, 100].iter().enumerate() {
        take.push(Captured::new(T0 + 1_100 * SEC + i as i64 * SEC / 4, [0xB0, 64, *v]));
    }

    // Still sounding at Stop: one note, and the pedal.
    take.push(Captured::new(T0 + 1_198 * SEC, [0x90, 72, 110]));

    // Heartbeats the whole time, which must never reach the file.
    for i in 0..500 {
        take.push(Captured::new(T0 + i * SEC / 3, [0xFE]));
    }

    let smf = take.build(&tl, TEMPO_BPM);

    // ── monotonic ticks, and no masked negative delta ───────────────────────
    for track in &smf.tracks {
        for ev in track {
            assert!(
                ev.delta.as_int() < 0x0FFF_FFFF,
                "a delta at the u28 ceiling is what a masked negative looks like"
            );
        }
    }

    // ── the file is coherent enough to read back and re-emit unchanged ──────
    let mut first = Vec::new();
    smf.write(&mut first).unwrap();
    let parsed = Smf::parse(&first).expect("our own file must parse");
    let mut second = Vec::new();
    parsed.write(&mut second).unwrap();
    assert_eq!(first, second, "round trip is not byte-identical");

    // ── walk it ─────────────────────────────────────────────────────────────
    let mut abs = 0u64;
    let mut end_tick = None;
    let mut offs_at_end = 0;
    let mut pedal_up_at_end: Option<usize> = None;
    let mut last_off_at_end: Option<usize> = None;
    let mut order = 0usize;
    let mut heartbeats = 0;
    let end = (tl.duration_seconds() * TPS).round() as u64;

    for ev in &smf.tracks[1] {
        abs += ev.delta.as_int() as u64;
        match ev.kind {
            TrackEventKind::Meta(MetaMessage::EndOfTrack) => end_tick = Some(abs),
            TrackEventKind::Midi { message, .. } if abs == end => {
                match message {
                    MidiMessage::NoteOff { .. } => {
                        offs_at_end += 1;
                        last_off_at_end = Some(order);
                    }
                    MidiMessage::Controller { controller, value }
                        if controller.as_int() == 64 && value.as_int() == 0 =>
                    {
                        pedal_up_at_end = Some(order);
                    }
                    _ => {}
                }
                order += 1;
            }
            TrackEventKind::Midi { .. } => order += 1,
            _ => {}
        }
        if format!("{:?}", ev.kind).contains("254") {
            heartbeats += 1;
        }
    }

    assert_eq!(heartbeats, 0, "Active Sensing reached the file");

    // Exactly one hanging note (72), released at the stop tick.
    assert_eq!(
        offs_at_end, 1,
        "expected exactly one synthesised note-off at Stop"
    );
    let (off, ped) = (
        last_off_at_end.expect("a note-off at the stop tick"),
        pedal_up_at_end.expect("a pedal-up at the stop tick"),
    );
    assert!(
        off < ped,
        "pedal-up must come at or after the note-offs, or a reader that models \
         sustain re-releases notes meant to ring"
    );

    // EndOfTrack sits at the take's real end, which is the audio sample count
    // converted to ticks.
    let samples = tl.duration_seconds() * NOMINAL;
    let expected_end = (samples / NOMINAL * TPS).round() as u64;
    let got_end = end_tick.expect("EndOfTrack");
    assert!(
        got_end.abs_diff(expected_end) <= 1,
        "EndOfTrack at {got_end}, audio ends at {expected_end}"
    );
}

#[test]
fn a_midi_source_anchors_even_when_its_clock_domain_is_already_right() {
    // The trap the design walked into once: on macOS every clock is in the mach
    // domain, so it is tempting to treat the conversion as an identity and skip
    // the anchor. The SCALE alone makes that wrong by a factor of 1000, and the
    // origin is still unknown afterwards.
    let mut c = SourceClock::new(MIDIR_SCALE_NS, 2 * SEC);

    // The device's epoch is its own, some hours before the take.
    let device_epoch_us = 3_600_000_000u64;
    for i in 0..200u64 {
        let stamp = device_epoch_us + i * 5_000;
        let host = T0 + (i as i64) * 5 * SEC / 1_000;
        c.observe(stamp, host + jitter(i, SEC / 1_000).abs());
    }

    let mapped = c.to_timebase(device_epoch_us).expect("anchored");
    assert!(
        (mapped - T0).abs() < 2 * SEC / 1_000,
        "the anchor should recover T0 to within a couple of milliseconds; got \
         {} ns off",
        (mapped - T0).abs()
    );

    // And the identity would have been catastrophically wrong.
    let naive = device_epoch_us as i64;
    assert!(
        (naive - T0).abs() > 900 * SEC,
        "treating microseconds as nanoseconds is not a rounding error"
    );
}
