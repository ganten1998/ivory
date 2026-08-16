//! The camera spike: what does this machine have, and does it deliver frames?
//!
//!   cargo run -p ivory-record --example lscam
//!   cargo run -p ivory-record --example lscam -- --frames 30
//!   cargo run -p ivory-record --example lscam -- --uid 0x1400000046d082d
//!   cargo run -p ivory-record --example lscam -- --list
//!
//! Lists every camera with every format it offers, opens one, and grabs ten
//! frames — reporting size, stride, pixel format sanity, the host timestamp of
//! each frame and the gap to the previous one.
//!
//! **This is how a real camera gets verified, because `cargo test` cannot.**
//! Every stride case in `camera.rs` is a synthetic padded buffer, which proves
//! the arithmetic and proves nothing about what a Logitech actually sends. The
//! numbers to look at:
//!
//! - **stride** is printed as a ratio to `width * 4`. Anything but `1.00x` means
//!   the camera pads its rows, which is exactly the case the conversion exists
//!   for; if the picture is fine at `1.00x` and shears at `1.33x`, the bug is in
//!   `bgra_to_rgba` and not in the camera.
//! - **Δ** is the gap between consecutive host timestamps. It should sit near
//!   `1000/fps` ms. A number that is consistently double means the camera has
//!   silently halved its rate for exposure, which is a dim room and not a bug.
//! - **unreadable** must be zero. Any other number means the pixel format is not
//!   the BGRA that `videoSettings` asked for, and the preview is showing
//!   nothing.
//! - **superseded** is expected to be large here: this loop polls slowly on
//!   purpose, and newest-wins is doing its job by throwing away what it cannot
//!   show. It is *not* a bug and its absence would be one.
//!
//! On macOS an unsigned `cargo run` binary may be denied camera access outright
//! (or prompt once and be forgotten): the entitlement that makes this reliable
//! is `com.apple.security.device.camera` in a signed bundle. If the frame count
//! is zero and the permission line says "granted", that is the difference.

use std::time::{Duration, Instant};

use ivory_record::audio::Timebase;
use ivory_record::camera::{
    cameras, default_camera, open_camera, permission_status, select_by_uid, CameraInfo, Format,
    FormatWish, BYTES_PER_PIXEL,
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().any(|a| a == name);
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    println!("camera permission: {}", permission_status());

    let found = match cameras() {
        Ok(found) => found,
        Err(e) => {
            println!("\nenumeration failed: {e}");
            std::process::exit(1);
        }
    };

    println!("\n{} camera(s) found", found.len());
    for cam in &found {
        describe(cam);
    }
    if found.is_empty() || flag("--list") {
        return;
    }

    let wanted = value("--uid");
    let chosen = match &wanted {
        Some(uid) => match select_by_uid(&found, uid) {
            Some(cam) => cam,
            None => {
                println!("\nno camera with UID {uid}");
                std::process::exit(1);
            }
        },
        None => default_camera(&found).expect("the list is not empty"),
    };

    let frames: usize = value("--frames")
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    grab(chosen, frames);
}

fn describe(cam: &CameraInfo) {
    let flag = if cam.is_default { " [default]" } else { "" };
    println!("\n  {}{}", cam.name, flag);
    println!("    uid    {}", cam.uid);
    println!("    formats ({}):", cam.formats.len());
    // Largest first, which is the order a human scans for "what is the best this
    // thing can do".
    let mut sorted: Vec<Format> = cam.formats.clone();
    sorted.sort_by(|a, b| {
        b.pixels()
            .cmp(&a.pixels())
            .then(b.fps.total_cmp(&a.fps))
    });
    for f in sorted.iter().take(12) {
        println!("      {f}");
    }
    if sorted.len() > 12 {
        println!("      ... and {} more", sorted.len() - 12);
    }
}

fn grab(cam: &CameraInfo, wanted: usize) {
    let wish = FormatWish::hd();
    println!("\nopening \"{}\" ({})", cam.name, cam.uid);
    println!(
        "  asking for {}x{} @ {} fps",
        wish.width.unwrap_or(0),
        wish.height.unwrap_or(0),
        wish.fps.unwrap_or(0.0)
    );

    // The clock every source in a take shares. Created here, before the open, so
    // the printed timestamps are measured from the same zero the recorder would
    // use — including the session warm-up, which is the point of the next line.
    let timebase = Timebase::new();
    let opening = Instant::now();
    let stream = match open_camera(&cam.uid, &wish, timebase) {
        Ok(stream) => stream,
        Err(e) => {
            println!("  failed: {e}");
            std::process::exit(1);
        }
    };
    // RECORDER-PLAN §3 budgets 300-800 ms here on a built-in camera and over 2 s
    // for a Continuity Camera. This number is why the recorder opens the camera
    // when its band opens rather than when Record is pressed.
    println!("  startRunning took {} ms", opening.elapsed().as_millis());
    println!("  opened at {}", stream.format());
    println!(
        "  latency   {} ns ({:?}) — see RECORDER-PLAN §3a, this is a placeholder",
        stream.latency_ns(),
        stream.latency_source()
    );

    println!("\n  {:>3}  {:>11}  {:>8}  {:>9}  {:>8}", "#", "host_ns", "Δ ms", "size", "stride");

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut seen = 0usize;
    let mut previous: Option<i64> = None;
    let mut gaps: Vec<f64> = Vec::new();
    let mut first_ns: Option<i64> = None;

    while seen < wanted && Instant::now() < deadline {
        let Some(frame) = stream.latest() else {
            std::thread::sleep(Duration::from_millis(2));
            continue;
        };
        seen += 1;
        first_ns.get_or_insert(frame.host_ns);
        let gap_ms = previous.map(|p| (frame.host_ns - p) as f64 / 1e6);
        if let Some(gap) = gap_ms {
            gaps.push(gap);
        }
        previous = Some(frame.host_ns);

        let tight = frame.width as usize * BYTES_PER_PIXEL;
        let ratio = if tight == 0 {
            0.0
        } else {
            frame.stride as f64 / tight as f64
        };
        println!(
            "  {:>3}  {:>11}  {:>8}  {:>9}  {:>7.2}x",
            seen,
            frame.host_ns,
            gap_ms.map_or_else(|| "-".to_string(), |g| format!("{g:.2}")),
            format!("{}x{}", frame.width, frame.height),
            ratio
        );

        // A frame whose buffer does not match its own geometry would sail past
        // every check above and produce a garbled picture downstream.
        assert_eq!(
            frame.pixels.len(),
            frame.stride * frame.height as usize,
            "frame {seen} claims {}x{} at stride {} but carries {} bytes",
            frame.width,
            frame.height,
            frame.stride,
            frame.pixels.len()
        );
    }

    let stats = stream.stats();
    println!("\n  state       {:?}", stream.state());
    println!("  delivered   {}", stats.frames_delivered());
    println!(
        "  superseded  {}  (dropped by newest-wins; a large number here is correct)",
        stats.frames_superseded()
    );
    println!(
        "  late-drop   {}  (AVFoundation dropped these; non-zero means our callback is slow)",
        stats.frames_dropped_late()
    );
    println!(
        "  unreadable  {}  (MUST be zero; non-zero means the pixel format is not BGRA)",
        stats.frames_unreadable()
    );

    if gaps.is_empty() {
        println!("\n  no frames in 15 s — check the permission line at the top");
        std::process::exit(1);
    }
    let mean = gaps.iter().sum::<f64>() / gaps.len() as f64;
    let min = gaps.iter().copied().fold(f64::INFINITY, f64::min);
    let max = gaps.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    println!(
        "\n  frame gap   mean {mean:.2} ms, min {min:.2}, max {max:.2}  \
         (expected {:.2} ms at {:.2} fps)",
        1000.0 / stream.format().fps,
        stream.format().fps
    );
    // Measured across the frames this loop actually collected, so it is a real
    // delivered rate rather than the advertised one. A camera that has halved
    // its rate for exposure shows up right here.
    if let (Some(first), Some(last)) = (first_ns, previous) {
        if seen > 1 && last > first {
            let elapsed_s = (last - first) as f64 / 1e9;
            println!(
                "  measured    {:.2} fps over {:.2} s of wall clock",
                (seen - 1) as f64 / elapsed_s,
                elapsed_s
            );
        }
    }
}
