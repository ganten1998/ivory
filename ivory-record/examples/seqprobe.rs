//! Does this driver actually maintain `v4l2_buffer.sequence`, and does a
//! shallow queue actually lose frames?
//!
//! `bufdepth` reports drops as gaps in the driver's sequence numbers, and a
//! driver that leaves the field at zero would report no gaps forever — the
//! same answer as a queue that loses nothing, for the opposite reason. This is
//! the control that tells the two apart. It talks to V4L2 directly, prints the
//! raw sequence numbers, and can hold each buffer for a while on purpose:
//!
//!   ./seqprobe 40 0                      # sequence should step by exactly 1
//!   IVORY_V4L2_BUFFERS=2 ./seqprobe 40 120   # a slow consumer on a shallow queue
//!   IVORY_V4L2_BUFFERS=8 ./seqprobe 40 120   # ... and on a deep one
//!
//! If the middle run shows gaps and the first does not, the instrument works
//! and the queue depth is doing something measurable.
//!
//! A sustained slow consumer is not what queue depth is for, though — no finite
//! queue survives a consumer slower than the producer, it only takes longer to
//! saturate. The case depth actually buys is the BURST: a consumer that keeps
//! up until something stalls it. Two more arguments model that — stall this
//! many milliseconds, every this many frames:
//!
//!   IVORY_V4L2_BUFFERS=2 ./seqprobe 60 0 300 15
//!   IVORY_V4L2_BUFFERS=6 ./seqprobe 60 0 300 15

#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("seqprobe talks to V4L2; there is nothing to probe here.");
}

#[cfg(target_os = "linux")]
mod imp {
use std::time::{Duration, Instant};

use linuxvideo::format::{PixFormat, PixelFormat};
use linuxvideo::Device;

pub fn main() {
    let mut a = std::env::args().skip(1);
    let want: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(40);
    let hold_ms: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let stall_ms: u64 = a.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let every: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let buffers: u32 = std::env::var("IVORY_V4L2_BUFFERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);

    let dev = Device::open(std::path::Path::new("/dev/video0")).expect("open /dev/video0");
    let cap = dev
        .video_capture(PixFormat::new(1280, 720, PixelFormat::MJPG))
        .expect("negotiate 720p MJPG");
    let granted_fmt = cap.format();
    println!(
        "negotiated {}x{} {:?}  (driver's sizeimage {} bytes)",
        granted_fmt.width(),
        granted_fmt.height(),
        granted_fmt.pixel_format(),
        granted_fmt.size_image()
    );
    let mut stream = cap
        .into_stream_with(buffers)
        .expect("start streaming");
    println!(
        "requested {buffers} buffers, driver granted {}   holding each frame {hold_ms} ms{}",
        stream.buffer_count(),
        if stall_ms > 0 && every > 0 {
            format!(", stalling {stall_ms} ms every {every}")
        } else {
            String::new()
        }
    );

    let mut seqs: Vec<u32> = Vec::with_capacity(want);
    // The mmap'd size of one buffer, read off the first frame rather than
    // computed from the format: what the queue costs in memory is
    // granted count times this, and for MJPEG it is the driver's worst-case
    // allocation rather than the size of any actual frame.
    let mut buf_bytes = 0usize;
    let t0 = Instant::now();
    while seqs.len() < want && t0.elapsed() < Duration::from_secs(60) {
        let r = stream.dequeue(|view| {
            seqs.push(view.sequence());
            buf_bytes = view.raw_buffer().len();
            if hold_ms > 0 {
                // Stand in for a consumer that does real work while holding the
                // buffer — which is what the two-buffer default cannot afford.
                std::thread::sleep(Duration::from_millis(hold_ms));
            }
            // The burst: a consumer that keeps up, until it does not. This
            // sleeps AFTER releasing nothing — the buffer is still held — which
            // is the point: a stalled capture thread is a held buffer.
            if stall_ms > 0 && every > 0 && seqs.len() % every == 0 {
                std::thread::sleep(Duration::from_millis(stall_ms));
            }
            Ok(())
        });
        if let Err(e) = r {
            eprintln!("dequeue failed: {e}");
            break;
        }
    }

    let elapsed = t0.elapsed().as_secs_f64();
    print!("sequences:");
    for (i, s) in seqs.iter().enumerate() {
        if i % 12 == 0 {
            print!("\n  ");
        }
        print!("{s:>6}");
    }
    println!();

    println!(
        "  one buffer is {:.0} KB, so this queue costs {:.1} MB",
        buf_bytes as f64 / 1024.0,
        (buf_bytes * stream.buffer_count()) as f64 / (1024.0 * 1024.0)
    );

    let mut gaps = 0u64;
    let mut steps_of_one = 0u64;
    for w in seqs.windows(2) {
        let d = w[1].checked_sub(w[0]).unwrap_or(0);
        if d == 1 {
            steps_of_one += 1;
        }
        gaps += u64::from(d.saturating_sub(1));
    }
    let all_zero = seqs.iter().all(|s| *s == 0);
    println!(
        "\n{} frames in {elapsed:.2}s ({:.2} fps seen)",
        seqs.len(),
        seqs.len() as f64 / elapsed
    );
    println!("  consecutive steps of 1   {steps_of_one} of {}", seqs.len().saturating_sub(1));
    println!("  frames lost in gaps      {gaps}");
    if all_zero {
        println!("\n  *** sequence is ALL ZERO: this driver does not maintain the field,");
        println!("      and every drop number measured through it is meaningless. ***");
    }
}

}

#[cfg(target_os = "linux")]
pub fn main() {
    imp::main();
}
