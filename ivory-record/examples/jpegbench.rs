//! What one MJPEG frame costs on the CPU, measured the way the camera backend
//! actually does it.
//!
//! `camera/linux.rs::decode_mjpg` builds a fresh `JpegDecoder` per frame and
//! decodes straight to RGBA. That is reproduced exactly here so the number is
//! the real one, not a flattering variant — the MJPEG stream shares its
//! Huffman and quantisation tables across every frame, and rebuilding the
//! decoder re-parses them 30 times a second.
//!
//! Run against frames dumped from a real capture:
//!
//!   ffmpeg -f v4l2 -input_format mjpeg -video_size 1280x720 -i /dev/video0 \
//!          -t 6 -c copy sample.mkv
//!   ffmpeg -i sample.mkv -c copy -f image2 'frames/f%04d.jpg'
//!   cargo run --release -p ivory-record --example jpegbench -- frames/*.jpg

// **Linux only, because what it measures is.** `zune-jpeg` and the VA-API
// decoder are `[target.'cfg(target_os = "linux")'.dependencies]`, so this
// cannot even be COMPILED elsewhere — and `cargo test --workspace` builds
// every example on every platform. A stub main is what keeps that build green
// without pretending the measurement means anything off the machine it is
// about.
#[cfg(not(target_os = "linux"))]
pub fn main() {
    eprintln!("jpegbench measures the Linux camera backend's JPEG decode; there is nothing to measure here.");
}

#[cfg(target_os = "linux")]
mod imp {
use std::io;
use std::time::Instant;

fn cpu_seconds() -> f64 {
    // getrusage(RUSAGE_SELF), via /proc so the example needs no libc dep.
    let s = std::fs::read_to_string("/proc/self/stat").unwrap_or_default();
    // Fields after the (possibly space-containing) comm are counted from the
    // last ')', which is why this does not just split on whitespace.
    let Some(rest) = s.rsplit_once(')').map(|(_, r)| r) else {
        return 0.0;
    };
    let f: Vec<&str> = rest.split_whitespace().collect();
    // utime = field 14, stime = field 15 (1-based, whole line); after the
    // comm split they are offsets 11 and 12.
    let tick = 100.0; // CLK_TCK on every Linux this runs on
    let utime: f64 = f.get(11).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let stime: f64 = f.get(12).and_then(|v| v.parse().ok()).unwrap_or(0.0);
    (utime + stime) / tick
}

/// Byte-for-byte the body of `camera/linux.rs::decode_mjpg`.
fn decode_mjpg(bytes: &[u8], width: u32, height: u32, dst: &mut Vec<u8>) -> bool {
    use zune_jpeg::zune_core::colorspace::ColorSpace;
    use zune_jpeg::zune_core::options::DecoderOptions;

    let options = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut decoder = zune_jpeg::JpegDecoder::new_with_options(io::Cursor::new(bytes), options);
    if decoder.decode_headers().is_err() {
        return false;
    }
    let Some((w, h)) = decoder.dimensions() else {
        return false;
    };
    if (w as u32, h as u32) != (width, height) {
        return false;
    }
    dst.resize(width as usize * height as usize * 4, 0);
    decoder.decode_into(dst).is_ok()
}

pub fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: jpegbench frame.jpg...");
        std::process::exit(2);
    }
    let blobs: Vec<Vec<u8>> = paths
        .iter()
        .filter_map(|p| std::fs::read(p).ok())
        .collect();
    assert!(!blobs.is_empty(), "no readable frames");

    // Geometry from the first frame, the way the backend gets it from V4L2.
    let (w, h) = {
        use zune_jpeg::zune_core::options::DecoderOptions;
        let mut d = zune_jpeg::JpegDecoder::new_with_options(
            io::Cursor::new(&blobs[0]),
            DecoderOptions::default(),
        );
        d.decode_headers().expect("headers");
        let (w, h) = d.dimensions().expect("dimensions");
        (w as u32, h as u32)
    };

    let mut dst = Vec::new();
    // Warm up: first decode pays for the allocation the rest reuse.
    for b in blobs.iter().take(3) {
        decode_mjpg(b, w, h, &mut dst);
    }

    let reps = 4;
    let c0 = cpu_seconds();
    let t0 = Instant::now();
    let mut n = 0usize;
    for _ in 0..reps {
        for b in &blobs {
            if decode_mjpg(b, w, h, &mut dst) {
                n += 1;
            }
        }
    }
    let wall = t0.elapsed().as_secs_f64();
    let cpu = cpu_seconds() - c0;

    println!("zune-jpeg {n} frames, {w}x{h} -> RGBA");
    println!("  CPU   {:.3} s  ({:.3} ms/frame)", cpu, cpu * 1000.0 / n as f64);
    println!(
        "  wall  {:.3} s  ({:.3} ms/frame, {:.1} fps ceiling)",
        wall,
        wall * 1000.0 / n as f64,
        n as f64 / wall
    );
}

}

#[cfg(target_os = "linux")]
pub fn main() {
    imp::main();
}
