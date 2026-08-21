//! Does the capture path actually allocate per frame?
//!
//! `FrameSlot` keeps ONE spare pixel buffer. The capture thread takes it to
//! convert into, and the consumer hands the buffer of the frame it has finished
//! with back. If the consumer forgets, the spare is never refilled and every
//! single frame allocates a fresh full-size `Vec` — 3.7 MB at 720p — on the
//! capture thread.
//!
//! `spare` used to be refilled only when `publish` DISPLACED a frame, so the
//! allocation-free steady state held only while the preview was LOSING frames.
//! `FrameSlot::recycle` fixed that, and this measures whether it worked, by
//! counting every allocation the process makes with and without the consumer
//! playing its part:
//!
//!   cargo run --release -p ivory-record --example alloccost -- 10
//!
//! The `recycling` run is what the app does (`desktop.rs` hands the displaced
//! `camera_rgba` back through `Session::recycle_frame`). The `dropping` run is
//! the same consumer with that one call removed, and it is there to prove the
//! measurement can tell the difference at all.

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("alloccost measures the Linux capture path; there is nothing to measure here.");
}

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};

/// Counts every allocation, and separately those big enough to be a frame.
struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BIG: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Anything at least this big is a picture, not bookkeeping. 720p RGBA is
/// 3,686,400 bytes; a half-megabyte floor cannot catch anything smaller by
/// accident and cannot miss a frame.
const BIG_ENOUGH: usize = 512 * 1024;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
        if l.size() >= BIG_ENOUGH {
            BIG.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    /// **Counted too, and it has to be.** Growing a `Vec` is a `realloc`, and a
    /// capture thread handed an undersized buffer grows it — which is an
    /// allocation of a full frame by another name.
    unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(new as u64, Ordering::Relaxed);
        if new >= BIG_ENOUGH {
            BIG.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, new) }
    }
}

#[global_allocator]
static A: Counting = Counting;

#[cfg(target_os = "linux")]
mod imp {
use std::time::{Duration, Instant};

use ivory_record::audio::Timebase;
use ivory_record::camera::{cameras, default_camera, open_camera, FormatWish};

use super::{ALLOCS, BIG, BYTES};

fn snapshot() -> (u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    (ALLOCS.load(Relaxed), BIG.load(Relaxed), BYTES.load(Relaxed))
}

/// One measured run. `recycling` is the only difference between them.
fn run(secs: f64, recycling: bool) {
    let list = cameras().expect("enumerate cameras");
    let cam = default_camera(&list).expect("a camera");
    let stream =
        open_camera(&cam.uid, &FormatWish::hd(), Timebase::new()).expect("open the camera");

    // Warm up: the first frames build the decoder and grow the spare to full
    // size, and neither is a steady-state cost.
    let warm = Instant::now();
    while warm.elapsed() < Duration::from_secs(2) {
        if let Some(f) = stream.latest() {
            if recycling {
                stream.recycle(f.pixels);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let d0 = stream.stats().frames_delivered();
    let (a0, b0, y0) = snapshot();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        // Exactly what `desktop.rs` does: take the frame, and give the buffer
        // it displaces back.
        if let Some(f) = stream.latest() {
            if recycling {
                stream.recycle(f.pixels);
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let wall = t0.elapsed().as_secs_f64();
    let (a1, b1, y1) = snapshot();
    let delivered = stream.stats().frames_delivered() - d0;
    let (allocs, big, bytes) = (a1 - a0, b1 - b0, y1 - y0);

    println!(
        "{:<10}  {delivered:>4} frames in {wall:.1}s   allocations {allocs:>6}  \
         frame-sized {big:>5}  {:>7.1} MB",
        if recycling { "recycling" } else { "dropping" },
        bytes as f64 / (1024.0 * 1024.0)
    );
    if delivered > 0 {
        println!(
            "            per frame: {:.2} allocations, {:.2} frame-sized, {:.0} KB",
            allocs as f64 / delivered as f64,
            big as f64 / delivered as f64,
            bytes as f64 / delivered as f64 / 1024.0
        );
    }
}

pub fn main() {
    let secs: f64 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    println!("a frame at 720p RGBA is 3,686,400 bytes; 'frame-sized' counts allocations >= 512 KB\n");
    run(secs, true);
    run(secs, false);
}

}

#[cfg(target_os = "linux")]
pub fn main() {
    imp::main();
}
