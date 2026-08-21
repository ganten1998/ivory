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
use std::process::{Child, Stdio};
use std::collections::VecDeque;
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};


/// How many composited frames may be waiting for the encoder.
///
/// Small on purpose. A 1080p BGRA frame is 8 MB, so this is the difference
/// between a bounded 33 MB of slack and an unbounded queue that ends a long
/// take with the machine out of memory. Four is enough to ride out the
/// ordinary hiccup and far too few to be a queue in disguise.
const QUEUE: usize = 10;

/// The longest gap one picture may be held across, in frames.
///
/// Two seconds at any rate this app offers. A compositor that stops for longer
/// than that has not stalled, it has stopped — the camera was unplugged, the
/// machine went to sleep — and painting the last picture over the whole of it
/// would be inventing a recording of something that did not happen.
const MAX_PAD: u32 = 60;



/// How many bytes of composited frames may wait on the UI thread for room.
///
/// **This replaced a sleep, and the sleep was a feedback loop.** `push` runs on
/// the window's thread. When the queue filled it slept in one-millisecond
/// steps — up to 12 ms in steady state and up to 250 ms while ffmpeg was still
/// launching — and every one of those milliseconds was a repaint that did not
/// happen, which is a camera frame that arrived with nobody to convert it,
/// which fills the queue further. Blocking the window to save a frame cost
/// more frames than it saved.
///
/// So the wait became memory instead of time. A frame that finds no room is
/// held here and offered again on the next push, and the UI thread never
/// sleeps. The budget is in BYTES rather than frames because that is what the
/// machine actually has: 48 MB is about thirteen frames at 720p and eight at
/// 1080p, comfortably more slack than the 250 ms of blocking it replaces, and
/// still bounded — a long take on a slow disk cannot end with the machine out
/// of memory.
///
/// Beyond it, frames are shed and counted. That costs nothing in timing: the
/// slot each picture belongs to is computed from its own timestamp, so the gap
/// a shed frame leaves is filled by the next one's padding, automatically.
const OVERFLOW_BYTES: usize = 48 * 1024 * 1024;

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
pub fn program() -> PathBuf {
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
pub fn how_to_install() -> &'static str {
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

/// One picture, and how many of the video's slots it fills.
///
/// **`times` is what makes the video honest about time.** The stream on the
/// pipe is constant-rate — ffmpeg stamps it by arrival order and nothing else —
/// so a compositor that falls behind produces a video that is SHORT and FAST
/// unless the gaps it left are filled in. Filling them here, from the frame's
/// real timestamp, is what turns a stall into a brief freeze instead of a
/// speed-up.
struct Frame {
    bgra: Vec<u8>,
    /// Slots to fill with the PREVIOUS picture before writing this one.
    ///
    /// The previous one, because that is what was on screen while the
    /// compositor was busy making this one. Padding with the new picture
    /// instead would pull every stall's worth of motion earlier — the same
    /// error as sending no timestamps at all, just smaller.
    pad: u32,
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
    tx: Option<SyncSender<Frame>>,
    writer: Option<std::thread::JoinHandle<()>>,
    audio: Option<AudioSink>,
    /// The video-only file, muxed into `out` by `finish`.
    tmp: PathBuf,
    out: PathBuf,
    frame_bytes: usize,
    /// Frames that found no room, waiting to be offered again. See
    /// [`OVERFLOW_BYTES`].
    overflow: VecDeque<Frame>,
    /// The bytes `overflow` is holding, tracked rather than recomputed: it is
    /// consulted on every frame.
    overflow_bytes: usize,
    last_pts: Option<Nanos>,
    /// When the first frame of the take happened, which is where the video's
    /// own timeline starts.
    first_pts: Option<Nanos>,
    /// Frame slots already written, INCLUDING the ones written as padding.
    ///
    /// The video is constant-rate: slot `n` is `n / fps` seconds in, and this
    /// is how many slots have gone down the pipe. `frames` counts real frames
    /// and is what the take report means by a frame.
    slots: u64,
    /// The take's frame rate, for turning a timestamp into a slot number.
    fps: f64,
    /// Frames written to fill a gap the compositor could not keep up with.
    repeated: u64,
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

        let child = crate::proc::command(program())
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
        let (tx, rx) = sync_channel::<Frame>(QUEUE);
        // The writer owns the pipe for the whole take. It ends when the
        // channel closes, which `finish` does by dropping the sender.
        let writer = std::thread::Builder::new()
            .name("ivory-encode".into())
            .spawn(move || {
                let mut alive = true;
                // The last picture written, kept so a gap can be filled with
                // it. One buffer, reused: the padding costs no allocation and
                // does not go through the channel, so a twenty-slot stall is
                // twenty writes rather than twenty frames of backpressure.
                let mut last: Option<Vec<u8>> = None;
                for frame in rx {
                    if let Some(prev) = last.as_ref() {
                        for _ in 0..frame.pad {
                            // A broken pipe means ffmpeg died. Keep DRAINING
                            // rather than returning: the channel is bounded, so
                            // a writer that stops receiving would block the
                            // caller it exists to spare, and the take would
                            // hang instead of reporting.
                            if !alive {
                                break;
                            }
                            if stdin.write_all(prev).is_err() {
                                alive = false;
                            }
                        }
                    }
                    if alive && stdin.write_all(&frame.bgra).is_err() {
                        alive = false;
                    }
                    last = Some(frame.bgra);
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
            overflow: VecDeque::new(),
            overflow_bytes: 0,
            last_pts: None,
            first_pts: None,
            slots: 0,
            fps: f64::from(spec.fps.max(1)),
            repeated: 0,
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
        // **Where this picture belongs on the video's own clock.**
        //
        // The timestamp used to be read, compared and thrown away: every frame
        // that arrived went down the pipe as the next slot, whenever it turned
        // up. On a machine that cannot composite at the target rate — which is
        // the case this whole path exists to survive — half the slots never
        // arrived, so the take came out HALF LENGTH and playing at twice
        // speed, out of sync with its own audio. Measured on the owner's Linux
        // box across six takes: video length over audio length, 0.50 every
        // time.
        let first = *self.first_pts.get_or_insert(pts_ns);
        let slot = (((pts_ns - first) as f64 / 1.0e9) * self.fps).round().max(0.0) as u64;
        // Two pictures for one slot: the second is the one nobody would see,
        // so it is the one dropped. This is what a constant-rate resampler
        // does with frames that arrive faster than real time.
        if self.slots > 0 && slot < self.slots {
            self.out_of_order += 1;
            return Ok(());
        }
        // The slots between the last picture and this one. They were time the
        // LAST picture was on screen for, so that is what fills them — a
        // freeze, which is what actually happened, rather than the next
        // picture arriving early.
        let gap = slot.saturating_sub(self.slots);
        if bgra.len() < self.frame_bytes {
            return Err(format!(
                "a frame of {} bytes is short of the {} this video is",
                bgra.len(),
                self.frame_bytes
            ));
        }
        // **The padding rides with the picture it precedes**, not with the one
        // it repeats. The writer holds no state, and a gap can therefore never
        // be written for a frame that was then dropped for want of room.
        let frame = Frame {
            bgra: bgra[..self.frame_bytes].to_vec(),
            // Capped, because a stall of several seconds should not write
            // several seconds of one picture into a file the user is going to
            // watch. Beyond this the video runs short, which is at least
            // honest about there having been nothing to show.
            pad: gap.min(u64::from(MAX_PAD)) as u32,
        };
        // Anything held from an earlier push goes first, or the video would be
        // out of order. Non-blocking: whatever does not fit stays held.
        if !self.drain_overflow()? {
            // The channel is still full, so this frame joins the queue behind
            // the ones already waiting rather than jumping them.
            return Ok(self.hold(frame, pts_ns, slot, gap));
        }
        let Some(tx) = self.tx.as_ref() else {
            return Err("the encoder is already finished".to_owned());
        };
        match tx.try_send(frame) {
            Ok(()) => {
                self.accept(pts_ns, slot, gap);
                Ok(())
            }
            // Backpressure, and the whole reason the queue is bounded. Not an
            // error: `take.json` carries the count so "the video is juddery"
            // has an answer.
            Err(TrySendError::Full(back)) => Ok(self.hold(back, pts_ns, slot, gap)),
            Err(TrySendError::Disconnected(_)) => Err("the encoder stopped".to_owned()),
        }
    }

    /// Book a frame in as written: it is either in the channel or held.
    fn accept(&mut self, pts_ns: Nanos, slot: u64, gap: u64) {
        self.last_pts = Some(pts_ns);
        self.frames += 1;
        self.slots = slot + 1;
        self.repeated += gap;
    }

    /// Hold a frame for the next push, or shed it if the budget is spent.
    ///
    /// Shedding costs nothing in timing. `slot` comes from the frame's own
    /// timestamp and `self.slots` only advances for a frame that was kept, so
    /// the next picture's `gap` covers whatever was shed, automatically.
    fn hold(&mut self, frame: Frame, pts_ns: Nanos, slot: u64, gap: u64) {
        if self.overflow_bytes + frame.bgra.len() > OVERFLOW_BYTES {
            self.dropped += 1;
            return;
        }
        self.overflow_bytes += frame.bgra.len();
        self.overflow.push_back(frame);
        self.accept(pts_ns, slot, gap);
    }

    /// Offer held frames to the channel without waiting.
    ///
    /// `Ok(true)` means the overflow is empty and the channel has room for one
    /// more; `Ok(false)` means it is still backed up.
    fn drain_overflow(&mut self) -> Result<bool, String> {
        let Some(tx) = self.tx.as_ref() else {
            return Err("the encoder is already finished".to_owned());
        };
        while let Some(frame) = self.overflow.pop_front() {
            let bytes = frame.bgra.len();
            match tx.try_send(frame) {
                Ok(()) => self.overflow_bytes -= bytes,
                Err(TrySendError::Full(back)) => {
                    self.overflow.push_front(back);
                    return Ok(false);
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.overflow_bytes -= bytes;
                    return Err("the encoder stopped".to_owned());
                }
            }
        }
        Ok(true)
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

    pub fn frames_repeated(&self) -> u64 {
        self.repeated
    }

    pub fn finish(mut self) -> Result<(), String> {
        // **Anything still held goes down the pipe first.** `push` never waits
        // for room — it holds the frame and offers it again next time — so at
        // the end of a take there may be frames that have been counted as
        // written and are still here. Blocking is right this once: the take is
        // over, there is no window to keep responsive, and the alternative is
        // losing the last second of the video.
        if self.tx.is_some() {
            let held: Vec<Frame> = self.overflow.drain(..).collect();
            self.overflow_bytes = 0;
            for frame in held {
                let Some(tx) = self.tx.as_ref() else { break };
                if tx.send(frame).is_err() {
                    // The writer is gone; the count already says so.
                    break;
                }
            }
        }
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
        let status = crate::proc::command(program())
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
    use std::process::Command;

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

    /// **The take must be as long as it took, whatever the machine managed.**
    ///
    /// The bug this exists for was in every take on a machine that could not
    /// composite at the target rate: the timestamp was read, compared and
    /// thrown away, so every frame that arrived went down the pipe as the next
    /// slot regardless of when it happened. Half the frames arriving meant a
    /// video half as long, playing twice as fast, out of sync with its own
    /// audio. Measured across six of the owner's takes: video length over
    /// audio length, 0.50 every time.
    ///
    /// Ten frames over two seconds at a declared 10 fps has to be twenty
    /// slots, not ten.
    #[test]
    #[ignore = "needs ffmpeg"]
    fn a_compositor_that_falls_behind_still_writes_a_full_length_take() {
        if !have_ffmpeg() {
            eprintln!("no ffmpeg on PATH, the encoder was not exercised");
            return;
        }
        const W: u32 = 64;
        const H: u32 = 48;
        let dir = std::env::temp_dir().join("ivory-ffmpeg-slots");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("slow.mp4");
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
        let f = vec![0x40u8; (W * H * 4) as usize];
        // Half rate: one frame every 200 ms against a 100 ms slot.
        for i in 0..10u64 {
            enc.push(&f, (i * 200_000_000) as Nanos).expect("push");
        }
        assert_eq!(enc.frames_written(), 10, "ten real frames went in");
        assert_eq!(
            enc.frames_repeated(),
            9,
            "the gaps between them were not filled, so the video is half length"
        );
        enc.finish().expect("finish");

        // And the file itself says so. Nineteen slots at 10 fps is 1.9 s; ten
        // would be 1.0, which is the failure.
        let probe = Command::new("ffprobe")
            .args(["-v", "error", "-select_streams", "v:0"])
            .args(["-count_frames", "-show_entries", "stream=nb_read_frames"])
            .args(["-of", "csv=p=0"])
            .arg(&out)
            .output()
            .expect("ffprobe");
        let text = String::from_utf8_lossy(&probe.stdout);
        let n: u64 = text.trim().trim_end_matches(',').parse().unwrap_or(0);
        assert!(
            n >= 18,
            "the file has {n} frames where the take was 19 slots long - the \
             video is short and fast, which is the bug"
        );
        let _ = std::fs::remove_file(&out);
    }

    /// **`push` must not sleep, however full the queue is.**
    ///
    /// It runs on the window's thread. It used to wait in one-millisecond
    /// steps for room — up to 12 ms in steady state and up to 250 ms while
    /// ffmpeg was still launching — and every one of those milliseconds was a
    /// repaint that did not happen, which is a camera frame that arrived with
    /// nobody to convert it, which fills the queue further. Blocking the
    /// window to save a frame cost more frames than it saved.
    ///
    /// So: push a burst several times the queue depth, faster than any encoder
    /// could drain it, and hold the whole burst to a budget that the old code
    /// would have blown by two orders of magnitude.
    #[test]
    #[ignore = "needs ffmpeg"]
    fn a_burst_that_fills_the_queue_never_blocks_the_window() {
        if !have_ffmpeg() {
            eprintln!("no ffmpeg on PATH, the encoder was not exercised");
            return;
        }
        const W: u32 = 64;
        const H: u32 = 48;
        let dir = std::env::temp_dir().join("ivory-ffmpeg-noblock");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("noblock.mp4");
        let _ = std::fs::remove_file(&out);
        let mut enc = Encoder::create(
            &out,
            super::super::VideoSpec { width: W, height: H, fps: 30 },
            None,
        )
        .expect("ffmpeg is on PATH");
        let f = vec![0x40u8; (W * H * 4) as usize];

        // Several times QUEUE, pushed as fast as the loop can go — which is
        // exactly the launch case, when ffmpeg has not read a byte yet.
        let burst = (QUEUE * 6) as u64;
        let t0 = std::time::Instant::now();
        for i in 0..burst {
            enc.push(&f, (i * 33_333_333) as Nanos).expect("push");
        }
        let elapsed = t0.elapsed();

        // The old code slept up to 250 ms PER FRAME during warm-up. Sixty
        // frames of that is fifteen seconds; this budget is generous for
        // sixty memcpys and impossible for sixty sleeps.
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "a burst of {burst} frames took {elapsed:?} - push is blocking the \
             window again"
        );
        // And nothing was silently lost: the budget is large enough to hold a
        // burst this size, so every frame is either queued or held.
        assert_eq!(
            enc.frames_written(),
            burst,
            "frames went missing that the overflow had room for"
        );
        assert_eq!(enc.dropped_not_ready(), 0, "shed a frame with budget to spare");
        enc.finish().expect("finish");
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
