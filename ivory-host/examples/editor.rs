//! Open a plugin's own editor and leave it on screen until it is closed.
//!
//!   cargo run -p ivory-host --example editor -- Pianoteq
//!
//! The thing a human can actually look at. Everything else about hosting can be
//! measured — peak, RMS, a written file — but "the plugin's UI appeared and you
//! can use it" cannot, and a window that is on screen but not being *driven*
//! looks identical to one that is until you try to click something.
//!
//! So this reports what it can while the window is up: the plugin keeps
//! rendering blocks in the gaps between event pumps, so a frozen window shows up
//! as a stalled block count, and `resizeView` calls are counted so that changing
//! Pianoteq's layout proves the host half of `IPlugFrame` is wired in.
//!
//! **No audio device.** This renders into a buffer and throws it away; it exists
//! to prove the window works. Use
//! `dist/Tangent.app/Contents/MacOS/tangent --plugin-test Pianoteq` for the
//! version you can hear, which is also the one that runs inside a signed bundle
//! and therefore the one a plugin with a licence check will talk to.

use std::time::{Duration, Instant};

use ivory_host::{Editor, Instance, Module, Note, Setup};

/// How long each pump gets before the loop takes a turn. 16 ms is a 60 Hz
/// frame, which is what the app's own loop gives it.
const FRAME: Duration = Duration::from_millis(16);

/// Blocks of the probe note below. 40 x 512 frames is a bit under half a
/// second at 48 kHz — long enough for an attack and the start of a decay,
/// short enough that rendering it does not stall the window visibly.
const PROBE_BLOCKS: usize = 40;

/// Strike a note, render its attack, and report peak and RMS.
///
/// **This is how "changing a preset changed the sound" stops being a claim.**
/// Nothing here is listening to a device, so the only evidence available is the
/// numbers, and a Steinway and a Bösendorfer struck identically do not produce
/// the same ones. Run it before and after touching the editor.
fn probe(inst: &mut Instance, bufs: &mut [Vec<f32>], channels: usize) -> (f32, f32) {
    let on = [Note {
        offset: 0,
        pitch: 60,
        velocity: 100.0 / 127.0,
        on: true,
    }];
    let off = [Note {
        offset: 0,
        pitch: 60,
        velocity: 0.5,
        on: false,
    }];
    let mut peak = 0.0f32;
    let mut sumsq = 0.0f64;
    let mut n = 0u64;
    for b in 0..PROBE_BLOCKS {
        let events: &[Note] = if b == 0 { &on } else { &[] };
        if inst.process(events, 512, bufs).is_err() {
            break;
        }
        for ch in bufs.iter().take(channels) {
            for s in ch.iter() {
                peak = peak.max(s.abs());
                sumsq += f64::from(*s) * f64::from(*s);
                n += 1;
            }
        }
    }
    // Release it, or the probe leaves a note held down for the rest of the run
    // and the next one measures both.
    let _ = inst.process(&off, 512, bufs);
    let rms = if n > 0 {
        (sumsq / n as f64).sqrt() as f32
    } else {
        0.0
    };
    (peak, rms)
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

    println!("module:   {}", bundle.display());
    let module = Module::open(&bundle).expect("open module");
    let class = module
        .audio_modules()
        .into_iter()
        .next()
        .expect("no Audio Module Class");
    println!("class:    {} [{}]", class.name, module.vendor());

    let setup = Setup {
        sample_rate: 48_000.0,
        max_block: 512,
    };
    let mut inst = Instance::create(&module, &class, setup).expect("instantiate");

    // A bare `cargo run` binary is not inside a `.app`, so the window server
    // treats it as a background process: its windows open behind everything and
    // never take the keyboard. Tangent is a bundle and must NOT call this.
    ivory_host::editor::become_foreground();

    let t = Instant::now();
    let offered = inst.has_editor();
    println!(
        "has_editor: {offered} (asked in {:.0} ms — VST3 has no cheaper way)",
        t.elapsed().as_secs_f64() * 1e3
    );
    if !offered {
        println!("nothing to show.");
        return;
    }

    let title = format!("{} — Tangent", class.name);

    // Opened, torn down and opened again before a human ever sees it.
    //
    // **The second open is the one that crashes.** The first proves nothing
    // about `removed()`: a host that destroys the window while the plugin still
    // has an `NSView` inside it looks perfectly fine until the plugin next
    // draws, and a controller that cannot build a second view after its first
    // was released looks fine until somebody closes the window and changes
    // their mind. Both are three seconds of work to check here and a bug report
    // from a stranger otherwise.
    let opened_at = Instant::now();
    match Editor::open(&inst, &title) {
        Ok(first) => {
            drop(first);
            println!(
                "reopen:   closed and reopened cleanly ({:.0} ms for the round trip)",
                opened_at.elapsed().as_secs_f64() * 1e3
            );
        }
        Err(why) => {
            eprintln!("could not open the editor: {why}");
            std::process::exit(1);
        }
    }

    let editor = match Editor::open(&inst, &title) {
        Ok(e) => e,
        Err(why) => {
            eprintln!("could not open the editor a second time: {why}");
            std::process::exit(1);
        }
    };
    let (w, h) = editor.size();
    println!("window:   {w} x {h} points, from the plugin's own getSize()");
    println!("messages: {} allocated by the host so far", inst.messages_made());
    println!("\nclose the window to finish. Resize or re-skin the plugin to exercise resizeView.");

    let channels = inst
        .audio_outputs()
        .first()
        .map(|b| b.channels.max(0) as usize)
        .unwrap_or(2);
    let mut bufs: Vec<Vec<f32>> = vec![vec![0.0; 512]; channels.max(1)];

    let mut blocks = 0u64;
    let mut reported = Instant::now();
    let mut last_size = (w, h);
    while !editor.closed() {
        ivory_host::editor::pump(FRAME);
        // Keep the DSP running underneath the UI. Not for the sound — nothing
        // is listening — but because a plugin whose processor has stopped
        // behaves differently in its editor, and because this is where a
        // deadlock between the two halves would show up as a stalled count.
        if inst.process(&[], 512, &mut bufs).is_ok() {
            blocks += 1;
        }
        if editor.size() != last_size {
            last_size = editor.size();
            println!(
                "resizeView: the plugin asked for {} x {} (call {})",
                last_size.0,
                last_size.1,
                editor.resizes()
            );
        }
        if reported.elapsed() >= Duration::from_secs(5) {
            reported = Instant::now();
            let (edits, restarts) = inst.editor_edits();
            let (peak, rms) = probe(&mut inst, &mut bufs, channels);
            blocks += PROBE_BLOCKS as u64;
            println!(
                "alive: {blocks} blocks, {} resizes, {edits} performEdit, \
                 {restarts} restartComponent, {} messages | probe C4: \
                 peak {peak:.4} rms {rms:.4}",
                editor.resizes(),
                inst.messages_made()
            );
        }
    }

    // Dropping the editor is the teardown: `removed()`, then the window. Doing
    // it explicitly here rather than at the end of `main` is the point being
    // demonstrated — the instance below is still alive and playable afterwards.
    println!("\nclosed by the user.");
    drop(editor);
    let before = blocks;
    for _ in 0..100 {
        if inst.process(&[], 512, &mut bufs).is_ok() {
            blocks += 1;
        }
    }
    println!(
        "the instrument survived its editor: {} more blocks rendered after the window went",
        blocks - before
    );

    let (edits, restarts) = inst.editor_edits();
    println!("performEdit calls:      {edits}");
    println!("restartComponent calls: {restarts}");
    println!("host messages made:     {}", inst.messages_made());
}
