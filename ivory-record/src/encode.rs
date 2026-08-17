//! Turning frames into a video file.
//!
//! The **live** half of the video pipeline. Camera frames cannot be kept and
//! encoded afterwards — RECORDER-PLAN §0 measured one 20-minute 1080p30 take at
//! 112 GB as raw NV12 — so every frame is encoded as it arrives, into a
//! video-only file. The audio is muxed in at Stop by [`mux`], from the `.wav`
//! that was being written anyway.
//!
//! # Why the audio is not encoded live too
//!
//! The first design (RECORDER-PLAN §7) appended audio to the same writer as it
//! ran, which means building a `CMSampleBuffer` around PCM on the writer thread
//! and getting its timestamp right sixty times a second. Muxing at the end is
//! simpler and *more* accurate, for a reason worth stating plainly: the `.wav`
//! is already sample-accurate against the take clock, because
//! `FrameCursor`/`WritePlan` pad dropouts so that file sample N is device frame
//! N. Anything derived from it inherits that. A live audio path would be a
//! second, independent chance to get the same alignment wrong.
//!
//! So sync reduces to ONE number — how far the first video frame is from the
//! first audio sample — and that number is known exactly, because camera frames
//! and audio callbacks both carry `host_ns` off the same [`crate::clock`]
//! timebase. See [`Mux::video_offset_ns`].
//!
//! # Platforms
//!
//! macOS only for now (AVFoundation: VideoToolbox H.264, AAC, `.mp4`). Windows
//! and Linux get [`Encoder::create`] returning `Err`, which is what the Export
//! dialog's `VIDEO_EXPORT_READY` gate reads — a build that cannot encode says
//! so rather than writing a file nobody will find.

use crate::clock::Nanos;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(not(target_os = "macos"))]
mod stub;
#[cfg(not(target_os = "macos"))]
use stub as sys;

/// What the video track will be.
///
/// `width` and `height` are the FINAL frame size, already decided by the export
/// spec's `Resolution` — the encoder does not scale, because the compositor has
/// to know the size it is painting into anyway and two places deciding it is
/// two places to disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoSpec {
    pub width: u32,
    pub height: u32,
    /// Nominal rate, written into the track. The real timing comes from each
    /// frame's presentation time, so a camera that delivers 29.97 or drops to
    /// 24 in low light still lands correctly — this is what a player shows in
    /// its info panel, not what it uses to place frames.
    pub fps: u32,
}

impl VideoSpec {
    /// Both dimensions even, which H.264 requires in 4:2:0.
    ///
    /// Rounded DOWN rather than up: growing the frame would leave a strip the
    /// compositor never painted, and a green or black edge on every video is
    /// the kind of thing that is noticed once and never forgiven.
    pub fn even(self) -> Self {
        Self {
            width: self.width & !1,
            height: self.height & !1,
            ..self
        }
    }

    pub fn is_usable(self) -> bool {
        let e = self.even();
        e.width >= 16 && e.height >= 16 && self.fps > 0
    }
}

/// A live video-only encode.
///
/// Frames go in with a presentation time in nanoseconds **relative to the first
/// frame of the take**, not absolute host time: a file whose first frame is at
/// 1.4 seconds starts with 1.4 seconds of nothing in every player.
pub struct Encoder(sys::Encoder);

impl Encoder {
    /// Create a video-only file at `path`.
    ///
    /// The extension is the caller's, and it must match what the platform
    /// writes — `.mp4` on macOS. Not chosen here, because the take's file
    /// layout is `take.rs`'s business.
    pub fn create(path: &std::path::Path, spec: VideoSpec) -> Result<Self, String> {
        if !spec.is_usable() {
            return Err(format!(
                "{}x{} at {} fps is not a video",
                spec.width, spec.height, spec.fps
            ));
        }
        sys::Encoder::create(path, spec.even()).map(Encoder)
    }

    /// Append one frame, as tightly-packed BGRA8.
    ///
    /// `pts_ns` must not go backwards. A frame at or before the previous one is
    /// DROPPED rather than refused: cameras do deliver the occasional
    /// out-of-order timestamp, and killing a take over one frame would be a
    /// worse bug than the one it reports.
    pub fn push(&mut self, bgra: &[u8], pts_ns: Nanos) -> Result<(), String> {
        self.0.push(bgra, pts_ns)
    }

    /// How many frames were dropped for going backwards in time.
    pub fn out_of_order(&self) -> u64 {
        self.0.out_of_order()
    }

    /// Frames the encoder refused because it was not ready for more.
    ///
    /// Not an error and not silent: VideoToolbox applies backpressure through
    /// `isReadyForMoreMediaData`, and a machine that cannot keep up drops
    /// frames rather than growing an unbounded queue. `take.json` carries this
    /// so that "the video is juddery" can be answered.
    pub fn dropped_not_ready(&self) -> u64 {
        self.0.dropped_not_ready()
    }

    pub fn frames_written(&self) -> u64 {
        self.0.frames_written()
    }

    /// Close the file. **Must** be called, or the container has no index and
    /// nothing will play it.
    pub fn finish(self) -> Result<(), String> {
        self.0.finish()
    }
}

/// Combine a video-only file and a `.wav` into one playable file.
///
/// The video track is copied through **without re-encoding** — it is already
/// H.264 and re-compressing it would cost quality and minutes for nothing. Only
/// the audio is encoded, from LPCM to AAC.
pub struct Mux {
    /// The video-only file the take wrote.
    pub video: std::path::PathBuf,
    /// The take's `.wav`.
    pub audio: std::path::PathBuf,
    pub out: std::path::PathBuf,
    /// **The sync number.** How far the first video frame is behind the first
    /// audio sample, in nanoseconds, on the take's own clock.
    ///
    /// Positive means the camera started late, which is the normal case: an
    /// audio device is running before the take begins and a camera takes
    /// between 63 ms and four seconds to produce its first frame. The video
    /// track is offset by exactly this much so that what was simultaneous
    /// stays simultaneous.
    ///
    /// Getting the SIGN wrong here doubles the error rather than removing it,
    /// which is why `a_late_camera_is_pushed_later_not_earlier` exists.
    pub video_offset_ns: Nanos,
}

impl Mux {
    pub fn run(&self) -> Result<(), String> {
        sys::mux(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_size_is_made_even_by_shrinking_not_growing() {
        // Growing would leave a strip the compositor never painted, which is a
        // green or black edge on every video ever exported.
        let odd = VideoSpec {
            width: 1921,
            height: 1081,
            fps: 30,
        };
        assert_eq!(odd.even().width, 1920);
        assert_eq!(odd.even().height, 1080);
        // And an even one is left exactly alone.
        let even = VideoSpec {
            width: 1920,
            height: 1080,
            fps: 30,
        };
        assert_eq!(even.even(), even);
    }

    #[test]
    fn a_frame_nobody_could_encode_is_refused_before_the_platform_sees_it() {
        for bad in [
            VideoSpec {
                width: 0,
                height: 1080,
                fps: 30,
            },
            VideoSpec {
                width: 1920,
                height: 1080,
                fps: 0,
            },
            // Measured AFTER `even()`, which is the ordering that matters: 15
            // becomes 14, and a check against the raw width would let a frame
            // through that the encoder is then handed one column short.
            VideoSpec {
                width: 15,
                height: 240,
                fps: 30,
            },
        ] {
            assert!(!bad.is_usable(), "{bad:?} should not be encodable");
            let dir = std::env::temp_dir().join("tangent-encode-refused");
            let _ = std::fs::create_dir_all(&dir);
            assert!(
                Encoder::create(&dir.join("x.mp4"), bad).is_err(),
                "{bad:?} was accepted"
            );
        }
    }

    /// **A real file, written by the real encoder, checked by something that is
    /// not this code.**
    ///
    /// `#[ignore]` because it needs VideoToolbox and writes to disk, which is
    /// the same reason the camera tests are ignored. Run with:
    ///
    /// ```text
    /// cargo test -p ivory-record --  --ignored writes_a_real
    /// ```
    ///
    /// The assertions that matter are the ones made by `ffprobe`, not by us: a
    /// file this code both wrote and validated would prove only that it is
    /// self-consistent.
    #[test]
    #[ignore = "writes a real video file with the platform encoder"]
    fn writes_a_real_playable_file_at_the_times_it_was_given() {
        let dir = std::env::temp_dir().join("tangent-encode-real");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("v.mp4");

        const W: u32 = 320;
        const H: u32 = 240;
        const FPS: u32 = 30;
        const N: u64 = 90; // three seconds
        let spec = VideoSpec {
            width: W,
            height: H,
            fps: FPS,
        };
        let mut enc = Encoder::create(&path, spec).expect("create");
        let mut frame = vec![0u8; (W * H * 4) as usize];
        for i in 0..N {
            // A moving band, so a file that is all one colour is visibly wrong
            // rather than merely small.
            let row = (i as usize * 2) % H as usize;
            frame.fill(0);
            for x in 0..W as usize {
                let p = (row * W as usize + x) * 4;
                frame[p] = 0xFF; // B
                frame[p + 3] = 0xFF; // A
            }
            // Exactly on the nominal grid, so the duration is arithmetic and
            // not a measurement.
            let pts = (i as i64 * 1_000_000_000) / i64::from(FPS);
            enc.push(&frame, pts).expect("push");
        }
        assert_eq!(enc.out_of_order(), 0, "the test fed them in order");
        let written = enc.frames_written();
        let refused = enc.dropped_not_ready();
        enc.finish().expect("finish");

        assert!(path.is_file(), "no file was written");
        let size = std::fs::metadata(&path).expect("stat").len();
        assert!(size > 1024, "a {size}-byte mp4 is not a video");
        assert_eq!(
            written + refused,
            N,
            "every frame was either written or refused"
        );
        // **And nearly all of them were WRITTEN.** This is the assertion that
        // matters and the one the first version of this test did not make: with
        // an instant drop on "not ready", 83 of these 90 frames were thrown
        // away and a three-second clip came out 0.23 seconds long — while
        // `written + refused == N` sailed through, because the frames were
        // faithfully counted on their way into the bin.
        assert!(
            written >= N - 2,
            "only {written} of {N} frames were encoded; {refused} were refused"
        );

        // And now something that is not this code says whether it is real.
        let Ok(out) = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_packets",
                "-show_entries",
                "stream=codec_name,width,height,nb_read_packets",
                "-of",
                "csv=p=0",
            ])
            .arg(&path)
            .output()
        else {
            // No ffprobe on this machine: the file was still written and its
            // size checked. Saying so is better than passing silently.
            eprintln!("ffprobe is not installed — the container was not verified");
            return;
        };
        let probe = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        assert!(
            probe.starts_with("h264,320,240,"),
            "ffprobe read this back as {probe:?}"
        );
        let packets: u64 = probe
            .rsplit(',')
            .next()
            .and_then(|n| n.parse().ok())
            .expect("a packet count");
        assert_eq!(packets, written, "the file holds a different number of frames");
    }
}
