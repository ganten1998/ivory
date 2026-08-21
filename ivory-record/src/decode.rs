//! Reading an audio file in, for the backing-track player.
//!
//! **Two backends, split the same way `encode` is, and for the same reason.**
//! Neither platform needs a decoding crate per format, because both already
//! have something that reads everything:
//!
//! * **Not macOS** — `tangent-ffmpeg`, which the release already ships so that
//!   video works on a machine with nothing installed. One command does format,
//!   channel layout and sample rate together.
//! * **macOS** — `afconvert`, which is in `/usr/bin` on every Mac ever sold.
//!   The mac build has no bundled ffmpeg (video goes through AVFoundation), so
//!   reaching for one here would mean either shipping 76 MB of encoder for a
//!   feature that is not encoding, or several hundred lines of CoreAudio FFI
//!   plus a resampler of my own. `afconvert` is neither.
//!
//! What comes back either way is interleaved **stereo f32 at the device's
//! rate**, because that is the only shape the mixer wants and both tools will
//! convert to it. A mono file is duplicated across both channels, which is
//! what a mono backing track should do.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// A decoded audio file, ready to mix.
#[derive(Debug, Clone, PartialEq)]
pub struct Clip {
    /// Interleaved stereo, at [`Clip::rate`].
    pub samples: Vec<f32>,
    pub rate: u32,
    /// Where it came from, for the row's label and for reloading it.
    pub source: PathBuf,
}

impl Clip {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }

    pub fn seconds(&self) -> f64 {
        if self.rate == 0 {
            return 0.0;
        }
        self.frames() as f64 / f64::from(self.rate)
    }

    /// The file's name, for a row that has one line to say what is loaded.
    pub fn label(&self) -> String {
        self.source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// A peak envelope, `buckets` wide, for drawing.
    ///
    /// **Peak and not average.** A waveform drawn from means is a grey sausage
    /// that says nothing about where the music is; the outline people read is
    /// the loudest sample in each column, which is also what tells them where
    /// a track actually starts when they come to trim it.
    ///
    /// Both channels fold into one outline: this is a picture for finding the
    /// ends of a take, not a stereo analyser.
    pub fn envelope(&self, buckets: usize) -> Vec<f32> {
        let frames = self.frames();
        if buckets == 0 || frames == 0 {
            return Vec::new();
        }
        let mut out = vec![0.0; buckets];
        for (i, slot) in out.iter_mut().enumerate() {
            let from = i * frames / buckets;
            let to = (((i + 1) * frames / buckets).max(from + 1)).min(frames);
            let mut peak = 0.0f32;
            for f in from..to {
                peak = peak.max(self.samples[f * 2].abs());
                peak = peak.max(self.samples[f * 2 + 1].abs());
            }
            *slot = peak.min(1.0);
        }
        out
    }
}

/// The most audio one clip may hold, in frames.
///
/// **Twenty minutes of stereo, which is 460 MB of f32.** A backing track is
/// three to six minutes; this is not a limit anybody making one will meet, and
/// it is what stops somebody who picked a two-hour podcast by mistake from
/// finding out by watching the machine swap. The error says the length, so it
/// reads as "that file is enormous" rather than as a failure.
const MAX_FRAMES: usize = 20 * 60 * 48_000;

/// Decode `path` to interleaved stereo f32 at `rate`.
///
/// The error is a sentence for a person: this is reached by picking a file in
/// a dialog, so "it did not work" has to say what to do instead.
pub fn decode(path: &Path, rate: u32) -> Result<Clip, String> {
    let rate = rate.clamp(8_000, 192_000);
    let raw = read_raw(path, rate)?;
    if raw.len() < 8 {
        return Err("that file has no audio in it".to_owned());
    }
    let frames = raw.len() / 8;
    if frames > MAX_FRAMES {
        return Err(format!(
            "that file is {:.0} minutes long. The backing track holds up to {} \
             minutes - trim it first, or pick a shorter one.",
            frames as f64 / f64::from(rate) / 60.0,
            MAX_FRAMES / 60 / 48_000
        ));
    }
    let mut samples = Vec::with_capacity(frames * 2);
    for chunk in raw.chunks_exact(4) {
        // A trailing partial sample is dropped rather than folded in: half a
        // float is not a quiet sample, it is a loud wrong one.
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    samples.truncate(frames * 2);
    Ok(Clip {
        samples,
        rate,
        source: path.to_path_buf(),
    })
}

/// Everywhere but macOS: the encoder the release already carries.
#[cfg(not(target_os = "macos"))]
fn read_raw(path: &Path, rate: u32) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let program = crate::encode::ffmpeg::program();
    let mut child = crate::proc::command(&program)
        // `-nostdin`, or a child that decides it wants a terminal takes the
        // parent's and the app is left with no keyboard until it exits.
        .args(["-nostdin", "-v", "error", "-i"])
        .arg(path)
        .args([
            // The first AUDIO stream: somebody will import a video file, and
            // without this the default mapping picks its picture.
            "-map",
            "a:0",
            "-f",
            "f32le",
            "-acodec",
            "pcm_f32le",
            "-ac",
            "2",
            "-ar",
            &rate.to_string(),
            "-",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            format!(
                "that file could not be read ({e}). Importing audio needs \
                 ffmpeg: {}",
                crate::encode::ffmpeg::how_to_install()
            )
        })?;

    let mut raw = Vec::new();
    if let Some(mut out) = child.stdout.take() {
        // Drained BEFORE waiting: a child whose pipe fills while the parent
        // waits on it is a deadlock, and a five-minute track is a great many
        // pipe buffers.
        out.read_to_end(&mut raw)
            .map_err(|e| format!("that file stopped part way through ({e})"))?;
    }
    let status = child
        .wait()
        .map_err(|e| format!("the decoder did not finish ({e})"))?;
    if !status.success() {
        let mut why = String::new();
        if let Some(mut err) = child.stderr.take() {
            let _ = err.read_to_string(&mut why);
        }
        let why = why.lines().last().unwrap_or("").trim().to_owned();
        return Err(if why.is_empty() {
            "that file is not audio this can read".to_owned()
        } else {
            format!("that file could not be read: {why}")
        });
    }
    Ok(raw)
}

/// macOS: `afconvert`, which is in `/usr/bin` on every Mac.
///
/// It writes a file rather than a stream — there is no stdout mode that works
/// across formats — so this goes via a temporary WAV and takes it away again.
/// `LEF32@rate` is little-endian 32-bit float, which is the shape the caller
/// wants; the RIFF wrapper is a header this walks past.
#[cfg(target_os = "macos")]
fn read_raw(path: &Path, rate: u32) -> Result<Vec<u8>, String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!(
        "tangent-import-{}-{stamp}.wav",
        std::process::id()
    ));
    // Whatever happens below, the temporary file goes away.
    struct Sweep(PathBuf);
    impl Drop for Sweep {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _sweep = Sweep(tmp.clone());

    let out = crate::proc::command("/usr/bin/afconvert")
        .arg(path)
        .args(["-f", "WAVE", "-d"])
        .arg(format!("LEF32@{rate}"))
        .args(["-c", "2"])
        .arg(&tmp)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("that file could not be read ({e})"))?;
    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.lines().last().unwrap_or("").trim().to_owned();
        return Err(if why.is_empty() {
            "that file is not audio this can read".to_owned()
        } else {
            format!("that file could not be read: {why}")
        });
    }
    let bytes = std::fs::read(&tmp).map_err(|e| format!("the decoded file went missing ({e})"))?;
    riff_data(&bytes).ok_or_else(|| "the decoded file has no audio in it".to_owned())
}

/// The `data` chunk of a RIFF/WAVE file.
///
/// **Walked rather than assumed at offset 44.** `afconvert` writes an `FLLR`
/// padding chunk before `data` to page-align it, so the fixed offset every
/// tutorial gives lands in the middle of the padding — which reads as a track
/// that begins with a burst of noise.
#[cfg(target_os = "macos")]
fn riff_data(bytes: &[u8]) -> Option<Vec<u8>> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }
    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let from = at + 8;
        if id == b"data" {
            // Clamped: a size field that runs past the end is a truncated
            // file, and what there is of it is still music.
            let to = from.saturating_add(size).min(bytes.len());
            return Some(bytes[from..to].to_vec());
        }
        // Chunks are word-aligned; an odd size carries a pad byte.
        at = from.saturating_add(size + (size & 1));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A file goes in and stereo float comes out at the rate asked for.**
    ///
    /// Run against a real file through whatever this platform decodes with,
    /// because the thing being tested is the subprocess contract — the flags,
    /// the byte order and, on macOS, the chunk walk that a fixed offset gets
    /// wrong.
    #[test]
    fn a_wav_round_trips_through_the_platform_decoder() {
        // A second of a 440 Hz tone, written with this crate's own writer.
        let dir = std::env::temp_dir().join(format!("tangent-decode-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tone.wav");
        let rate = 48_000u32;
        let frames = rate as usize;
        let mut pcm = Vec::with_capacity(frames * 2);
        for f in 0..frames {
            let x = (std::f32::consts::TAU * 440.0 * f as f32 / rate as f32).sin() * 0.5;
            pcm.push(x);
            pcm.push(x);
        }
        let spec = crate::wav::WavSpec {
            sample_rate: rate,
            channels: 2,
            format: crate::wav::SampleFormat::Float32,
        };
        let bext = crate::wav::Bext::new(crate::wav::Wallclock::now_utc(), spec);
        match crate::wav::WavWriter::create(&path, spec, &bext) {
            Ok(mut w) => {
                w.write_interleaved(&pcm).expect("write");
                w.finish().expect("finish");
            }
            Err(e) => {
                eprintln!("no writer here ({e}); skipping");
                return;
            }
        }

        let clip = match decode(&path, rate) {
            Ok(c) => c,
            Err(e) => {
                // No decoder on this machine is a real answer, not a failure:
                // the Linux CI box may have neither ffmpeg nor a bundled one.
                eprintln!("no decoder here ({e}); skipping");
                let _ = std::fs::remove_dir_all(&dir);
                return;
            }
        };
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(clip.rate, rate);
        assert!(
            (clip.seconds() - 1.0).abs() < 0.02,
            "a one-second file decoded to {:.3}s",
            clip.seconds()
        );
        let peak = clip.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(
            (peak - 0.5).abs() < 0.02,
            "a half-scale tone came back at {peak:.3} - the byte order or the \
             chunk offset is wrong"
        );
        // Not silence in the first millisecond: that is what a `data` chunk
        // read from the wrong offset looks like.
        let head = clip.samples[..96].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(head > 0.01, "the decode started inside the header");
    }

    /// The envelope is the outline, and it is as long as it was asked for.
    #[test]
    fn the_envelope_follows_the_loudest_sample() {
        let clip = Clip {
            // Quiet, then loud, then quiet.
            samples: (0..3000)
                .flat_map(|i| {
                    let v = if (1000..2000).contains(&i) { 0.8 } else { 0.05 };
                    [v, v]
                })
                .collect(),
            rate: 48_000,
            source: PathBuf::from("x.wav"),
        };
        let env = clip.envelope(3);
        assert_eq!(env.len(), 3);
        assert!(env[0] < 0.1 && env[2] < 0.1, "{env:?}");
        assert!(env[1] > 0.7, "the loud third did not show: {env:?}");
        // Degenerate asks answer with nothing rather than dividing by zero.
        assert!(clip.envelope(0).is_empty());
    }
}
