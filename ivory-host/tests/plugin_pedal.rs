//! Does the sustain pedal actually reach the instrument?
//!
//! "`process` returned `kResultOk`" proves nothing here, and `instance.rs` has
//! the scar to prove it: Pianoteq was handed one `AudioBusBuffers` for its eight
//! output buses, wrote nothing at all, and returned `kResultOk`. A control
//! change is worse, because there is no return value to ignore in the first
//! place — `IParameterChanges` is a list the plugin may simply not read.
//!
//! So the pedal is measured the way a pianist would check it: **hold a note,
//! let go of the key with the pedal down, and listen for whether it is still
//! ringing.** The same phrase is played twice, once with CC64 and once without,
//! and the energy in a window after the note-off is compared. A pedal that did
//! not arrive makes the two runs identical.
//!
//! # Why it is `#[ignore]`d
//!
//! It needs a real VST3 piano installed, which no CI machine has. Run it
//! deliberately, following the same convention as `plugin_take_sync.rs`:
//!
//! ```text
//! cargo test -p ivory-host --test plugin_pedal -- --ignored --nocapture
//! ```

use ivory_host::{ClassInfo, Control, Instance, Module, Note, Setup};

const RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

/// Seconds into the take that each thing happens.
const NOTE_ON: f64 = 0.00;
const PEDAL_DOWN: f64 = 0.05;
const NOTE_OFF: f64 = 0.60;
/// The window the whole test rests on: entirely after the key was released,
/// with a guard so the note-off transient itself is not measured.
const TAIL_FROM: f64 = 0.80;
const TAIL_TO: f64 = 1.40;
const END: f64 = 1.50;

/// Warm-up, in seconds. RECORDER-PLAN §8: four of six instruments on this
/// machine render silence if played cold, and a silent run would "prove" the
/// pedal works by making both cases zero.
const WARM_UP: f64 = 5.0;

fn frame(seconds: f64) -> usize {
    (seconds * RATE) as usize
}

/// A piano, by preference. Copied in spirit from `plugin_take_sync.rs`: "the
/// first Audio Module Class found" is a terrible default, and a pedal test on a
/// bass sampler measures nothing at all.
fn instrument() -> Option<(Module, ClassInfo)> {
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
        let rest: Vec<_> = bundles.iter().filter(|p| !order.contains(p)).cloned().collect();
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

/// What one run of the phrase produced.
struct Run {
    /// RMS while the key is down, which is the reference the tail is judged
    /// against. A run whose note never sounded cannot say anything about
    /// sustain, and this is what notices.
    held: f64,
    /// RMS in the window after the key came up.
    tail: f64,
    /// Controls the plugin published no mapping for.
    unmapped: usize,
}

/// Play the phrase once, with or without the pedal, on a fresh instance.
///
/// Fresh on purpose: a pedal left down, a damper still moving or a reverb tail
/// from the previous run would all show up as sustain that CC64 did not cause.
fn play(module: &Module, class: &ClassInfo, pedal: bool) -> Run {
    let setup = Setup { sample_rate: RATE, max_block: BLOCK as i32 };
    let mut inst = Instance::create(module, class, setup).expect("instantiate");
    let channels = inst
        .audio_outputs()
        .first()
        .map(|b| b.channels)
        .unwrap_or(2)
        .max(1) as usize;
    let mut bufs: Vec<Vec<f32>> = vec![vec![0.0; BLOCK]; channels];

    for _ in 0..(WARM_UP * RATE / BLOCK as f64) as usize {
        inst.process(&[], BLOCK, &mut bufs).expect("warm-up");
    }

    let mut mono: Vec<f32> = Vec::with_capacity(frame(END));
    let mut unmapped = 0usize;
    let mut at = 0usize;
    while at < frame(END) {
        let n = BLOCK.min(frame(END) - at);
        let end = at + n;

        // Everything that falls inside this block, placed on its own frame.
        let mut notes: Vec<Note> = Vec::new();
        let mut controls: Vec<Control> = Vec::new();
        let place = |t: f64| -> Option<i32> {
            let f = frame(t);
            (f >= at && f < end).then(|| (f - at) as i32)
        };
        if let Some(o) = place(NOTE_ON) {
            notes.push(Note { offset: o, pitch: 60, velocity: 100.0 / 127.0, on: true });
        }
        if let Some(o) = place(NOTE_OFF) {
            notes.push(Note { offset: o, pitch: 60, velocity: 64.0 / 127.0, on: false });
        }
        if pedal {
            if let Some(o) = place(PEDAL_DOWN) {
                controls.push(Control::cc(o, 0, Control::SUSTAIN, 127));
            }
        }

        let r = inst
            .process_with_controls(&notes, &controls, n, &mut bufs)
            .expect("process");
        unmapped += r.unmapped;
        for i in 0..n {
            mono.push(bufs[0][i]);
        }
        at = end;
    }

    let rms = |from: f64, to: f64| -> f64 {
        let a = frame(from).min(mono.len());
        let b = frame(to).min(mono.len());
        if b <= a {
            return 0.0;
        }
        let sum: f64 = mono[a..b].iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
        (sum / (b - a) as f64).sqrt()
    };

    Run {
        held: rms(0.20, 0.55),
        tail: rms(TAIL_FROM, TAIL_TO),
        unmapped,
    }
}

#[test]
#[ignore = "needs a real VST3 instrument installed; run with --ignored"]
fn a_note_released_with_the_pedal_down_is_still_ringing_afterwards() {
    let Some((module, class)) = instrument() else {
        panic!("no VST3 instrument found; this test cannot run on this machine");
    };
    eprintln!("instrument: {} [{}]", class.name, module.vendor());

    // The mapping itself, reported before anything is rendered: it is the one
    // number that says WHY, when the energy figures disagree.
    {
        let setup = Setup { sample_rate: RATE, max_block: BLOCK as i32 };
        let inst = Instance::create(&module, &class, setup).expect("instantiate");
        eprintln!(
            "  CC64 -> {:?}   CC66 -> {:?}   CC67 -> {:?}   maps anything: {}",
            inst.control_param(0, Control::SUSTAIN).map(|id| format!("{id:#x}")),
            inst.control_param(0, Control::SOSTENUTO).map(|id| format!("{id:#x}")),
            inst.control_param(0, Control::SOFT).map(|id| format!("{id:#x}")),
            inst.maps_controls()
        );
    }

    let dry = play(&module, &class, false);
    let wet = play(&module, &class, true);

    eprintln!("  no pedal:   held {:.6}  tail {:.6}", dry.held, dry.tail);
    eprintln!("  pedal down: held {:.6}  tail {:.6}", wet.held, wet.tail);
    eprintln!(
        "  tail ratio: {:.1}x   (unmapped controls: {} dry, {} wet)",
        if dry.tail > 1e-9 { wet.tail / dry.tail } else { f64::INFINITY },
        dry.unmapped,
        wet.unmapped
    );

    assert!(
        wet.held > 1e-4 && dry.held > 1e-4,
        "the note never sounded ({:.6} / {:.6} RMS while held), so this run says \
         nothing about the pedal — the instrument is probably still warming up",
        dry.held,
        wet.held
    );
    assert_eq!(
        wet.unmapped, 0,
        "this instrument publishes no IMidiMapping for CC64, so the pedal could \
         not be sent as a parameter change at all"
    );
    assert!(
        dry.tail < dry.held * 0.5,
        "without the pedal the note is still as loud after the key came up \
         ({:.6} tail vs {:.6} held) — the note-off is not arriving either, and \
         this test cannot tell the two failures apart",
        dry.tail,
        dry.held
    );
    // The real assertion. A pedal that never arrived makes these two identical.
    assert!(
        wet.tail > dry.tail * 3.0,
        "the pedal did not arrive: {:.6} RMS after the key came up with CC64 \
         down, {:.6} without it (ratio {:.2}, needed 3.0)",
        wet.tail,
        dry.tail,
        wet.tail / dry.tail.max(1e-12)
    );
}
