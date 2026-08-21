//! What the V4L2 queue depth is actually worth, measured against a real camera.
//!
//! `linuxvideo` 0.3.5 asks the driver for two buffers and offers no way to ask
//! for more, and `dequeue` holds one for as long as its callback runs — so the
//! driver has exactly one to fill while we look at a frame, and anything that
//! arrives in that window overwrites a frame nobody read. That loss used to be
//! unobservable: V4L2 counts every frame it produces in `v4l2_buffer.sequence`,
//! but 0.3.5 never surfaces the field. `vendor/linuxvideo` surfaces it, and a
//! gap between consecutive dequeues is the count of frames lost this way.
//!
//! Run the same capture at several depths and compare:
//!
//!   for n in 2 4 6 8; do
//!     IVORY_V4L2_BUFFERS=$n cargo run --release -p ivory-record --example bufdepth -- 20
//!   done
//!
//! The honest question is whether depth still buys anything now that the
//! decode has moved out of the dequeue closure and VA-API took it from 32 ms
//! to 2.3. If the drop counts come back all zeros, that is the answer, and
//! the deeper queue is insurance rather than a fix.

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("bufdepth measures a V4L2 queue; there is nothing to measure here.");
}

#[cfg(target_os = "linux")]
mod imp {
use std::time::{Duration, Instant};

use ivory_record::audio::Timebase;
use ivory_record::camera::{cameras, default_camera, open_camera, FormatWish};

/// Process CPU seconds (utime + stime) from /proc, so the example needs no
/// libc dependency of its own. Same reader as `camcost`.
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
        .unwrap_or(20.0);
    let requested = std::env::var("IVORY_V4L2_BUFFERS").unwrap_or_else(|_| "(default)".into());

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

    let stream = match open_camera(&cam.uid, &FormatWish::hd(), Timebase::new()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open: {e}");
            std::process::exit(1);
        }
    };
    let fmt = stream.format();

    // Let the first frames pay for decoder setup, then measure a steady state.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(2) {
        let _ = stream.latest();
        std::thread::sleep(Duration::from_millis(5));
    }

    let st = stream.stats();
    let (d0, l0, s0, u0, k0) = (
        st.frames_delivered(),
        st.frames_dropped_late(),
        st.frames_superseded(),
        st.frames_unreadable(),
        st.frames_skipped(),
    );
    let c0 = cpu_seconds();
    let t0 = Instant::now();
    // Drain like the UI does, so `frames_superseded` stays meaningful and the
    // conversion actually runs — an unobserved camera skips the expensive part,
    // which is the part that holds the buffer.
    while t0.elapsed().as_secs_f64() < secs {
        let _ = stream.latest();
        std::thread::sleep(Duration::from_millis(5));
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - c0;
    let delivered = st.frames_delivered() - d0;
    let dropped = st.frames_dropped_late() - l0;
    let superseded = st.frames_superseded() - s0;
    let unreadable = st.frames_unreadable() - u0;
    let skipped = st.frames_skipped() - k0;
    let granted = st.buffers_allocated();
    // Every frame the hardware produced during the window: the ones we saw,
    // plus the ones the driver told us about only as a gap.
    let produced = delivered + skipped + unreadable + dropped;

    println!(
        "buffers requested {requested}, granted {granted}   {}x{} @ {:.2} fps negotiated   {:.0}s",
        fmt.width, fmt.height, fmt.fps, wall
    );
    println!(
        "  produced by hardware  {produced}  ({:.2} fps)",
        produced as f64 / wall
    );
    println!(
        "  delivered             {delivered}  ({:.2} fps)",
        delivered as f64 / wall
    );
    println!(
        "  DROPPED (seq gaps)    {dropped}  ({:.2}% of produced)",
        if produced > 0 { 100.0 * dropped as f64 / produced as f64 } else { 0.0 }
    );
    println!("  skipped (not wanted)  {skipped}");
    println!("  superseded (unread)   {superseded}");
    println!("  unreadable            {unreadable}");
    println!(
        "  process CPU           {cpu:.3} s  ({:.1}% of one core)",
        100.0 * cpu / wall
    );
}

}

#[cfg(target_os = "linux")]
pub fn main() {
    imp::main();
}
