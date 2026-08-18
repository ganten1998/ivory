//! Turning frames into a video file.
//!
//! The **live** half of the video pipeline. Camera frames cannot be kept and
//! encoded afterwards — RECORDER-PLAN §0 measured one 20-minute 1080p30 take at
//! 112 GB as raw NV12 — so every frame is encoded as it arrives, into a
//! video-only file. The audio is muxed in at Stop by [`mux`], from the `.wav`
//! that was being written anyway.
//!
//! # How sync is guaranteed, rather than measured
//!
//! **Both tracks are indexed from the take's own start, so there is no offset
//! to get wrong.**
//!
//! *Audio*: the samples pushed here are the SAME ones written to the `.wav`,
//! in the same order, and the presentation time is their index divided by the
//! sample rate. `FrameCursor`/`WritePlan` already pad dropouts so that file
//! sample N is device frame N, so the muxed audio inherits that exactly. Not
//! "aligned to" the wav — the same bytes at the same index.
//!
//! *Video*: composited frames are produced on a fixed tick from take start,
//! whatever the camera is doing. A camera takes between 63 ms and four seconds
//! to deliver its first frame; during that time the frame is still composited
//! and still encoded, with the camera's part of it dark. So the video track
//! also starts at zero, and the late camera is a dark opening rather than a
//! timing problem.
//!
//! That is what removes the whole class of bug this design was most likely to
//! have. An earlier draft carried a `video_offset_ns` — the gap between the
//! first audio sample and the first camera frame — to be applied at mux time.
//! It would have worked, and getting its SIGN wrong would have doubled the
//! error instead of removing it. Ticking from take start means the number does
//! not exist.
//!
//! The one residual error is bounded and stated: a tick uses the newest camera
//! frame available at that instant, so a frame can be shown up to one tick
//! interval (33 ms at 30 fps) after the moment it was captured. That is below
//! the ~45 ms where audio leading video becomes perceptible, and it is the same
//! bound every live compositor carries.
//!
//! # Platforms
//!
//! **macOS** encodes natively: AVFoundation driving VideoToolbox, H.264 and AAC
//! in an `.mp4`, with nothing to install.
//!
//! **Windows and Linux** pipe raw frames into `ffmpeg`, which writes the same
//! H.264 in the same container. See `ffmpeg.rs` for why a subprocess rather
//! than two more native backends. The cost is that ffmpeg has to be findable,
//! and when it is not, [`Encoder::create`] fails with a message naming the
//! install command rather than writing a file nobody will find. A take still
//! writes its `.wav` and its `.mid` either way.

use crate::clock::Nanos;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as sys;

#[cfg(not(target_os = "macos"))]
mod ffmpeg;
#[cfg(not(target_os = "macos"))]
use ffmpeg as sys;

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

/// What the file IS, for `take.json`'s video report.
///
/// Constants of this module rather than queried from a backend, because both
/// backends are ASKED for exactly this — H.264 and AAC in an `.mp4` — and a
/// backend producing something else is a bug in `create`, not a configuration
/// for a manifest to discover.
pub const CONTAINER: &str = "mp4";
pub const VIDEO_CODEC: &str = "h264";
pub const AUDIO_CODEC: &str = "aac";

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
    pub fn create(
        path: &std::path::Path,
        spec: VideoSpec,
        audio: Option<AudioSpec>,
    ) -> Result<Self, String> {
        if !spec.is_usable() {
            return Err(format!(
                "{}x{} at {} fps is not a video",
                spec.width, spec.height, spec.fps
            ));
        }
        sys::Encoder::create(path, spec.even(), audio).map(Encoder)
    }

    /// Append interleaved `f32` audio, indexed from the start of the take.
    ///
    /// `first_frame` is the index of the first frame in `interleaved` — the
    /// same index the `.wav` writes it at, which is what makes the two agree by
    /// construction rather than by adjustment. A silent video ignores this.
    pub fn push_audio(&mut self, interleaved: &[f32], first_frame: u64) -> Result<(), String> {
        self.0.push_audio(interleaved, first_frame)
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

/// The audio track, if the file is to have one.
///
/// Interleaved `f32` at `sample_rate`, which is what the recorder already has
/// in hand: the samples handed to [`Encoder::push_audio`] are the SAME ones
/// written to the `.wav`, in the same order, indexed the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioSpec {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioSpec {
    pub fn is_usable(self) -> bool {
        self.sample_rate >= 8_000 && (1..=2).contains(&self.channels)
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
                Encoder::create(&dir.join("x.mp4"), bad, None).is_err(),
                "{bad:?} was accepted"
            );
        }
    }

    /// **Both tracks, in one file, of the same length.**
    ///
    /// The sync test. It writes three seconds of video ticked from take start
    /// and three seconds of audio indexed from take start, and then asks
    /// `ffprobe` whether the two tracks agree — which is the only question that
    /// matters and the one this code cannot answer about itself.
    #[test]
    #[ignore = "writes a real video file with the platform encoder"]
    fn video_and_audio_start_together_and_end_together() {
        let dir = std::env::temp_dir().join("tangent-encode-av");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("av.mp4");

        const W: u32 = 320;
        const H: u32 = 240;
        const FPS: u32 = 30;
        const RATE: u32 = 48_000;
        const CH: usize = 2;
        const SECONDS: u64 = 3;

        let mut enc = Encoder::create(
            &path,
            VideoSpec {
                width: W,
                height: H,
                fps: FPS,
            },
            Some(AudioSpec {
                sample_rate: RATE,
                channels: CH as u16,
            }),
        )
        .expect("create");

        // Interleaved: one video frame, then the audio for that frame's worth
        // of time, exactly as the recorder will do it.
        let frame = vec![0x40u8; (W * H * 4) as usize];
        let per_tick = (RATE / FPS) as usize;
        let mut audio = vec![0.0f32; per_tick * CH];
        let mut written_frames: u64 = 0;
        for i in 0..(SECONDS * u64::from(FPS)) {
            enc.push(&frame, (i as i64 * 1_000_000_000) / i64::from(FPS))
                .expect("push video");
            // A 440 Hz tone, so the file has something in it that a person can
            // check by ear if a number ever looks wrong.
            for (n, s) in audio.chunks_exact_mut(CH).enumerate() {
                let t = (written_frames + n as u64) as f64 / f64::from(RATE);
                let v = (t * 440.0 * std::f64::consts::TAU).sin() as f32 * 0.2;
                s.fill(v);
            }
            enc.push_audio(&audio, written_frames).expect("push audio");
            written_frames += per_tick as u64;
        }
        enc.finish().expect("finish");

        let Ok(out) = std::process::Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "stream=codec_type,start_time,duration",
                // `key=value`, one per line, and NOT csv. The first version of
                // this test asked for csv and indexed the columns, which put
                // start_time where duration was expected and reported a
                // perfectly good three-second video as zero seconds long.
                // ffprobe emits fields in its OWN order, not the order they
                // were asked for.
                "-of",
                "default=nw=1",
            ])
            .arg(&path)
            .output()
        else {
            eprintln!("ffprobe is not installed - the tracks were not verified");
            return;
        };
        let text = String::from_utf8_lossy(&out.stdout);
        let mut tracks: Vec<(String, f64, f64)> = Vec::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "codec_type" => tracks.push((v.to_owned(), f64::NAN, f64::NAN)),
                "start_time" => {
                    if let Some(t) = tracks.last_mut() {
                        t.1 = v.parse().unwrap_or(f64::NAN);
                    }
                }
                "duration" => {
                    if let Some(t) = tracks.last_mut() {
                        t.2 = v.parse().unwrap_or(f64::NAN);
                    }
                }
                _ => {}
            }
        }
        let find = |kind: &str| {
            tracks
                .iter()
                .find(|t| t.0 == kind)
                .map(|t| (t.1, t.2))
                .unwrap_or_else(|| panic!("no {kind} track in {text:?}"))
        };
        let (v_start, v_dur) = find("video");
        let (a_start, a_dur) = find("audio");

        assert!(
            (v_dur - SECONDS as f64).abs() < 0.06,
            "the video is {v_dur}s, not {SECONDS}"
        );
        assert!(
            (a_dur - SECONDS as f64).abs() < 0.06,
            "the audio is {a_dur}s, not {SECONDS}"
        );
        // **The assertions this test exists for.** Two tracks that begin at
        // different times, or run for different lengths, are two tracks out of
        // sync. Ticking video from take start and indexing audio from take
        // start is what makes both hold with no correction anywhere.
        assert!(
            v_start.abs() < 0.001 && a_start.abs() < 0.001,
            "the tracks do not both start at zero: video {v_start}, audio {a_start}"
        );
        assert!(
            (v_dur - a_dur).abs() < 0.06,
            "the tracks are {:.3}s apart in length - video {v_dur}, audio {a_dur}",
            (v_dur - a_dur).abs()
        );
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
        let mut enc = Encoder::create(&path, spec, None).expect("create");
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
            eprintln!("ffprobe is not installed - the container was not verified");
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
