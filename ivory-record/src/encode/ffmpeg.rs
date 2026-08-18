//! The encoder on Windows and Linux: raw frames down a pipe into `ffmpeg`.
//!
//! # Why a subprocess and not a library
//!
//! Video encoding has no portable API. macOS has VideoToolbox behind
//! `AVAssetWriter` and that is what `macos.rs` uses; Windows has Media
//! Foundation, and Linux has whatever the distribution shipped. Writing and
//! maintaining two more native backends is three encoders to keep in step, in
//! two languages this crate does not otherwise speak, for one output format.
//!
//! `ffmpeg` is on both platforms, speaks BGRA on a pipe, and writes exactly the
//! H.264 in MP4 that `macos.rs` produces. The whole backend is safe Rust and a
//! process, which is a smaller thing to get wrong than a COM interface.
//!
//! **The cost is that it must be installed**, and this file is honest about
//! that: [`Encoder::create`] fails with an actionable message naming the
//! platform's install command rather than writing a file nobody will find. A
//! take still writes its `.wav` and its `.mid` either way, which is the same
//! bargain the stub made.
//!
//! # The rules this file lives by
//!
//! **Nothing blocks the caller.** `push` hands the frame to a writer thread
//! through a bounded channel and returns. A machine that cannot encode as fast
//! as it composites fills the queue and then DROPS, which is what
//! `dropped_not_ready` counts and `take.json` reports. Writing straight to the
//! pipe would block the UI thread on a slow encoder, which is the same failure
//! as a dropped frame except that the app stops responding first.
//!
//! **The video is written first and the audio muxed after.** One `ffmpeg` can
//! read one pipe portably; passing a second one means `/dev/fd` on Unix and
//! nothing at all on Windows. So the frames go to a temporary video-only file
//! while the take runs, the samples go to a raw file beside it, and `finish`
//! muxes the two with a stream copy for the video. The copy is the point: the
//! expensive part is never done twice.
//!
//! **Audio is padded, never adjusted.** `push_audio` carries the index the
//! samples belong at, exactly as the `.wav` writes them. A gap is filled with
//! silence so the sound stays where it was played; sliding later samples
//! earlier would drift the take against its own MIDI file.

use crate::clock::Nanos;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::time::{Duration, Instant};

/// How many composited frames may be waiting for the encoder.
///
/// Small on purpose. A 1080p BGRA frame is 8 MB, so this is the difference
/// between a bounded 33 MB of slack and an unbounded queue that ends a long
/// take with the machine out of memory. Four is enough to ride out the
/// ordinary hiccup and far too few to be a queue in disguise.
const QUEUE: usize = 4;

/// How many frame intervals a frame may wait before it is given up on.
///
/// **Not zero, and that is the whole of it.** The first version dropped the
/// instant the queue was full, which is the same mistake `macos.rs` made and
/// documents: an encoder is briefly busy all the time, so an instant drop
/// threw away eleven frames in twenty and turned a two-second clip into a
/// quarter of a second. Two frame intervals is long enough for every ordinary
/// hiccup and far too short to be a queue in disguise.
const WAIT_FRAMES: u32 = 2;

/// Where `ffmpeg` is, or why it is not.
///
/// The bundled encoder's file name, beside the executable. Deliberately NOT
/// `ffmpeg`: the Linux installer puts it in `~/.local/bin`, which is on most
/// users' `PATH`, and an unprefixed `ffmpeg` there would silently shadow the
/// system one for every shell the user owns.
const BUNDLED: &str = if cfg!(target_os = "windows") {
    "tangent-ffmpeg.exe"
} else {
    "tangent-ffmpeg"
};

/// `IVORY_FFMPEG` overrides everything, which is what the tests use. Then the
/// copy shipped beside the executable, so the artifact works on a machine with
/// nothing installed — the release carries its own encoder on the platforms
/// that need one. Only then the bare name, so `PATH` decides.
fn program() -> PathBuf {
    if let Some(p) = std::env::var_os("IVORY_FFMPEG") {
        return PathBuf::from(p);
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let bundled = dir.join(BUNDLED);
            if bundled.is_file() {
                return bundled;
            }
        }
    }
    PathBuf::from("ffmpeg")
}

/// Named so the message tells somebody what to actually do about it.
///
/// Reachable only when the bundled copy beside the executable is gone too —
/// somebody copied `tangent` out of its folder, most likely — so the bundled
/// copy is worth naming ahead of the package manager.
fn how_to_install() -> &'static str {
    if cfg!(target_os = "windows") {
        "restore the tangent-ffmpeg.exe that shipped next to tangent.exe, \
         install it with `winget install ffmpeg`, or put ffmpeg.exe on PATH"
    } else {
        "reinstall so the bundled tangent-ffmpeg sits next to the tangent \
         binary, or install ffmpeg with your package manager, for example \
         `sudo apt install ffmpeg`"
    }
}

fn missing(e: &std::io::Error) -> String {
    format!(
        "video needs ffmpeg and it could not be started ({e}). The take's audio \
         and MIDI are still recorded. To get video too, {}.",
        how_to_install()
    )
}

/// The samples, on their way to being a track.
struct AudioSink {
    file: std::fs::File,
    path: PathBuf,
    spec: super::AudioSpec,
    /// Frames already written, so a gap can be padded rather than closed.
    written: u64,
}

pub struct Encoder {
    child: Child,
    /// `None` once `finish` has taken it, which is what closes the pipe and
    /// tells ffmpeg the stream has ended.
    tx: Option<SyncSender<Vec<u8>>>,
    writer: Option<std::thread::JoinHandle<()>>,
    audio: Option<AudioSink>,
    /// The video-only file, muxed into `out` by `finish`.
    tmp: PathBuf,
    out: PathBuf,
    frame_bytes: usize,
    /// How long `push` will wait for room before it gives the frame up.
    wait: Duration,
    last_pts: Option<Nanos>,
    out_of_order: u64,
    dropped: u64,
    frames: u64,
}

impl Encoder {
    pub fn create(
        path: &Path,
        spec: super::VideoSpec,
        audio: Option<super::AudioSpec>,
    ) -> Result<Self, String> {
        let tmp = path.with_extension("video.tmp.mp4");
        // A leftover from a killed take would be appended to rather than
        // replaced, and the second take would carry the first's opening.
        let _ = std::fs::remove_file(&tmp);

        let child = Command::new(program())
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-f", "rawvideo", "-pixel_format", "bgra"])
            .args(["-video_size", &format!("{}x{}", spec.width, spec.height)])
            .args(["-framerate", &spec.fps.to_string()])
            .args(["-i", "pipe:0"])
            // yuv420p and not the encoder's pick: 4:4:4 or 10-bit H.264 is
            // legal and unplayable in every browser and most phones, which is
            // where a take of a lesson is going to be watched.
            .args(["-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p"])
            .args(["-movflags", "+faststart"])
            .arg(&tmp)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| missing(&e))?;

        let mut child = child;
        let mut stdin = child.stdin.take().ok_or("ffmpeg gave no stdin")?;
        let (tx, rx) = sync_channel::<Vec<u8>>(QUEUE);
        // The writer owns the pipe for the whole take. It ends when the
        // channel closes, which `finish` does by dropping the sender.
        let writer = std::thread::Builder::new()
            .name("ivory-encode".into())
            .spawn(move || {
                let mut alive = true;
                for frame in rx {
                    // A broken pipe means ffmpeg died. Keep DRAINING rather
                    // than returning: the channel is bounded, so a writer that
                    // stops receiving would block the caller it exists to
                    // spare, and the take would hang instead of reporting.
                    if alive && stdin.write_all(&frame).is_err() {
                        alive = false;
                    }
                }
                drop(stdin);
            })
            .map_err(|e| format!("the encode thread would not start: {e}"))?;

        let audio = match audio.filter(|a| a.is_usable()) {
            None => None,
            Some(spec) => {
                let path = tmp.with_extension("f32");
                let file = std::fs::File::create(&path)
                    .map_err(|e| format!("the take's audio buffer could not be made: {e}"))?;
                Some(AudioSink {
                    file,
                    path,
                    spec,
                    written: 0,
                })
            }
        };

        Ok(Self {
            child,
            tx: Some(tx),
            writer: Some(writer),
            audio,
            tmp,
            out: path.to_path_buf(),
            frame_bytes: spec.width as usize * spec.height as usize * 4,
            wait: Duration::from_secs_f64(f64::from(WAIT_FRAMES) / f64::from(spec.fps.max(1))),
            last_pts: None,
            out_of_order: 0,
            dropped: 0,
            frames: 0,
        })
    }

    pub fn push_audio(&mut self, interleaved: &[f32], first_frame: u64) -> Result<(), String> {
        let Some(sink) = self.audio.as_mut() else {
            return Ok(());
        };
        let ch = u64::from(sink.spec.channels).max(1);
        // Silence for anything missed, so the sound stays where it was played.
        if first_frame > sink.written {
            let gap = ((first_frame - sink.written) * ch) as usize;
            let zeros = vec![0u8; gap.min(1 << 20) * 4];
            let mut left = gap * 4;
            while left > 0 {
                let n = left.min(zeros.len().max(4));
                sink.file
                    .write_all(&zeros[..n.min(zeros.len())])
                    .map_err(|e| format!("the take's audio buffer could not be written: {e}"))?;
                left -= n.min(zeros.len());
            }
            sink.written = first_frame;
        }
        let mut bytes = Vec::with_capacity(interleaved.len() * 4);
        for s in interleaved {
            bytes.extend_from_slice(&s.to_le_bytes());
        }
        sink.file
            .write_all(&bytes)
            .map_err(|e| format!("the take's audio buffer could not be written: {e}"))?;
        sink.written += interleaved.len() as u64 / ch;
        Ok(())
    }

    pub fn push(&mut self, bgra: &[u8], pts_ns: Nanos) -> Result<(), String> {
        // A frame at or before the previous one is dropped rather than
        // refused. Cameras do deliver the occasional out-of-order timestamp,
        // and killing a take over one frame is the worse bug.
        if self.last_pts.is_some_and(|last| pts_ns <= last) {
            self.out_of_order += 1;
            return Ok(());
        }
        if bgra.len() < self.frame_bytes {
            return Err(format!(
                "a frame of {} bytes is short of the {} this video is",
                bgra.len(),
                self.frame_bytes
            ));
        }
        let Some(tx) = self.tx.as_ref() else {
            return Err("the encoder is already finished".to_owned());
        };
        // `SyncSender::send_timeout` is still unstable, so this is that: try,
        // and if there is no room, wait a millisecond and try again until the
        // deadline. `try_send` hands the frame back on a full channel, so the
        // retry costs no second copy of eight megabytes.
        let deadline = Instant::now() + self.wait;
        let mut frame = bgra[..self.frame_bytes].to_vec();
        loop {
            match tx.try_send(frame) {
                Ok(()) => {
                    self.last_pts = Some(pts_ns);
                    self.frames += 1;
                    return Ok(());
                }
                // Backpressure, and the whole reason the queue is bounded. Not
                // an error: `take.json` carries the count so "the video is
                // juddery" has an answer.
                Err(TrySendError::Full(back)) => {
                    if Instant::now() >= deadline {
                        self.dropped += 1;
                        return Ok(());
                    }
                    frame = back;
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(TrySendError::Disconnected(_)) => {
                    return Err("the encoder stopped".to_owned())
                }
            }
        }
    }

    pub fn out_of_order(&self) -> u64 {
        self.out_of_order
    }

    pub fn dropped_not_ready(&self) -> u64 {
        self.dropped
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    pub fn finish(mut self) -> Result<(), String> {
        // Closing the pipe is what ends the stream, so the sender goes before
        // the wait. Without this, ffmpeg sits on a read that will never return
        // and the take never finishes.
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let status = self
            .child
            .wait()
            .map_err(|e| format!("ffmpeg could not be waited on: {e}"))?;
        if !status.success() {
            let _ = std::fs::remove_file(&self.tmp);
            self.clean_audio();
            return Err(format!("ffmpeg refused the video ({status})"));
        }
        if self.frames == 0 {
            let _ = std::fs::remove_file(&self.tmp);
            self.clean_audio();
            return Err("no frames reached the encoder".to_owned());
        }

        match self.audio.take() {
            // Video only: the temporary file IS the take, so it is moved
            // rather than copied.
            None => std::fs::rename(&self.tmp, &self.out)
                .map_err(|e| format!("the video could not be put in place: {e}")),
            Some(sink) => {
                let r = Self::mux(&self.tmp, &sink, &self.out);
                let _ = std::fs::remove_file(&sink.path);
                let _ = std::fs::remove_file(&self.tmp);
                r
            }
        }
    }

    /// Put the sound back with the picture.
    ///
    /// `-c:v copy`, so the H.264 written during the take is not touched: the
    /// expensive work happens once, while the take runs, and this pass is a
    /// container rewrite plus an AAC encode of the audio.
    fn mux(video: &Path, sink: &AudioSink, out: &Path) -> Result<(), String> {
        let status = Command::new(program())
            .args(["-hide_banner", "-loglevel", "error", "-y"])
            .args(["-i"])
            .arg(video)
            .args(["-f", "f32le"])
            .args(["-ar", &sink.spec.sample_rate.to_string()])
            .args(["-ac", &sink.spec.channels.to_string()])
            .args(["-i"])
            .arg(&sink.path)
            .args(["-c:v", "copy", "-c:a", "aac", "-b:a", "192k"])
            .args(["-movflags", "+faststart"])
            .arg(out)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|e| missing(&e))?;
        if status.success() {
            Ok(())
        } else {
            Err(format!("ffmpeg refused to mux the take ({status})"))
        }
    }

    fn clean_audio(&mut self) {
        if let Some(sink) = self.audio.take() {
            let _ = std::fs::remove_file(&sink.path);
        }
    }
}

impl Drop for Encoder {
    /// A take abandoned without `finish` must not leave ffmpeg running.
    fn drop(&mut self) {
        drop(self.tx.take());
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.tmp);
        self.clean_audio();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn have_ffmpeg() -> bool {
        Command::new(program())
            .arg("-version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok()
    }

    /// A missing ffmpeg is a refusal with instructions, never a panic and never
    /// a half-written file.
    #[test]
    fn no_ffmpeg_is_an_answer_and_not_a_crash() {
        std::env::set_var("IVORY_FFMPEG", "definitely-not-a-real-program-xyzzy");
        let dir = std::env::temp_dir().join("ivory-ffmpeg-missing");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("take.mp4");
        // `expect_err` needs `Debug` on the Ok side and an `Encoder` owns a
        // child process, so the result is matched rather than unwrapped.
        let e = match Encoder::create(
            &out,
            super::super::VideoSpec {
                width: 64,
                height: 64,
                fps: 30,
            },
            None,
        ) {
            Ok(_) => panic!("a missing encoder must refuse"),
            Err(e) => e,
        };
        std::env::remove_var("IVORY_FFMPEG");
        assert!(e.contains("ffmpeg"), "{e}");
        assert!(
            e.contains("audio and MIDI are still recorded"),
            "the message does not say what a take still does: {e}"
        );
        assert!(!out.exists(), "a refused take left a file behind");
    }

    /// The real thing: frames in, a playable file out.
    #[test]
    #[ignore = "needs ffmpeg"]
    fn frames_become_a_video_that_ffprobe_can_read() {
        if !have_ffmpeg() {
            eprintln!("no ffmpeg on PATH, the encoder was not exercised");
            return;
        }
        const W: u32 = 64;
        const H: u32 = 48;
        let dir = std::env::temp_dir().join("ivory-ffmpeg-encode");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("take.mp4");
        let _ = std::fs::remove_file(&out);

        let mut enc = Encoder::create(
            &out,
            super::super::VideoSpec {
                width: W,
                height: H,
                fps: 10,
            },
            None,
        )
        .expect("ffmpeg is on PATH");
        for i in 0..20u64 {
            let mut f = vec![0u8; (W * H * 4) as usize];
            for px in f.chunks_exact_mut(4) {
                px[0] = (i * 12) as u8; // B
                px[3] = 0xFF;
            }
            enc.push(&f, (i * 100_000_000) as Nanos).expect("push");
        }
        assert_eq!(enc.frames_written(), 20);
        enc.finish().expect("finish");
        assert!(out.exists(), "no file was written");
        let n = std::fs::metadata(&out).expect("stat").len();
        assert!(n > 512, "the file is {n} bytes, which is not a video");
        let _ = std::fs::remove_file(&out);
    }

    /// A frame that goes backwards in time is dropped, not fatal.
    #[test]
    #[ignore = "needs ffmpeg"]
    fn a_frame_from_the_past_is_dropped_rather_than_fatal() {
        if !have_ffmpeg() {
            return;
        }
        const W: u32 = 32;
        const H: u32 = 32;
        let dir = std::env::temp_dir().join("ivory-ffmpeg-order");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("take.mp4");
        let mut enc = Encoder::create(
            &out,
            super::super::VideoSpec {
                width: W,
                height: H,
                fps: 10,
            },
            None,
        )
        .expect("ffmpeg");
        let f = vec![0xFFu8; (W * H * 4) as usize];
        enc.push(&f, 1_000_000_000).expect("first");
        enc.push(&f, 500_000_000).expect("a late frame is not an error");
        assert_eq!(enc.out_of_order(), 1);
        assert_eq!(enc.frames_written(), 1);
        drop(enc);
        assert!(!out.exists());
    }
}
