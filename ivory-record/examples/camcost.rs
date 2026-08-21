//! What the camera actually costs, measured through the real capture path.
//!
//! `jpegbench` times the software decoder against files. This opens the actual
//! camera through `open_camera`, runs it for a while, and reports the CPU the
//! process burned per delivered frame — so it measures the capture thread as
//! it really runs, hardware decode and all, rather than a decode in isolation.
//!
//!   cargo run --release -p ivory-record --example camcost -- 10
//!   IVORY_NO_VAAPI=1 cargo run --release -p ivory-record --example camcost -- 10
//!
//! The difference between those two runs is what `camera/vaapi.rs` is worth.

// **Linux only, because what it measures is.** `zune-jpeg` and the VA-API
// decoder are `[target.'cfg(target_os = "linux")'.dependencies]`, so this
// cannot even be COMPILED elsewhere — and `cargo test --workspace` builds
// every example on every platform. A stub main is what keeps that build green
// without pretending the measurement means anything off the machine it is
// about.
#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("camcost measures what the Linux camera costs end to end; there is nothing to measure here.");
}

#[cfg(target_os = "linux")]
mod imp {
use std::time::{Duration, Instant};

use ivory_record::audio::Timebase;
use ivory_record::camera::{cameras, default_camera, open_camera, FormatWish};

/// Process CPU seconds (utime + stime) from /proc, so the example needs no
/// libc dependency of its own.
fn cpu_seconds() -> f64 {
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    let Some(rest) = s.rsplit_once(')').map(|(_, r)| r) else {
        return 0.0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    let g = |i: usize| -> f64 { f.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.0) };
    (g(11) + g(12)) / 100.0
}

pub fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);

    let list = match cameras() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot enumerate cameras: {e}");
            std::process::exit(1);
        }
    };
    let Some(cam) = default_camera(&list) else {
        eprintln!("no cameras");
        std::process::exit(1);
    };
    println!("camera: {} ({})", cam.name, cam.uid);

    let stream = match open_camera(&cam.uid, &FormatWish::hd(), Timebase::new()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open: {e}");
            std::process::exit(1);
        }
    };
    let fmt = stream.format();
    println!(
        "format: {}x{} @ {:.2} fps   hardware decode: {}",
        fmt.width,
        fmt.height,
        fmt.fps,
        if std::env::var_os("IVORY_NO_VAAPI").is_some() { "disabled" } else { "enabled if available" }
    );

    // Let the first frames pay for decoder setup, then measure a steady state.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(2) {
        let _ = stream.latest();
        std::thread::sleep(Duration::from_millis(5));
    }

    let d0 = stream.stats().frames_delivered();
    let u0 = stream.stats().frames_unreadable();
    let c0 = cpu_seconds();
    let t0 = Instant::now();
    // Drain like the UI does, so `frames_superseded` stays meaningful.
    while t0.elapsed().as_secs_f64() < secs {
        let _ = stream.latest();
        std::thread::sleep(Duration::from_millis(5));
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - c0;
    let delivered = stream.stats().frames_delivered() - d0;
    let unreadable = stream.stats().frames_unreadable() - u0;

    println!("\n{delivered} frames in {wall:.2} s  ({:.2} fps)", delivered as f64 / wall);
    println!("  unreadable            {unreadable}");
    // The thing the app could not previously tell anyone.
    match stream.rate_limited() {
        Some(r) => println!(
            "  RATE LIMITED          delivering {:.2} fps against {:.2} negotiated \
             ({:.0}% of it)",
            r.actual_fps,
            r.negotiated_fps,
            r.ratio() * 100.0
        ),
        None => println!(
            "  rate                  keeping up with the negotiated {:.2} fps",
            stream.format().fps
        ),
    }
    println!("  process CPU           {cpu:.3} s  ({:.1}% of one core)", 100.0 * cpu / wall);
    if delivered > 0 {
        println!("  CPU per frame         {:.3} ms", cpu * 1000.0 / delivered as f64);
    }
}

}

#[cfg(target_os = "linux")]
pub fn main() {
    imp::main();
}
