//! Writing the take's `.wav`: a streaming RIFF writer with a Broadcast Wave
//! `bext` chunk.
//!
//! Called from the writer thread that drains the audio ring, **never from the
//! audio callback**. Everything here allocates, seeks and writes files; a
//! callback that does any of those produces the dropout the ring exists to
//! prevent. The API takes `&[f32]` interleaved because that is what cpal hands
//! the callback and what the §4a mixer produces, and the conversion to the
//! file's word size happens here, once, where the clipping can be counted.
//!
//! # Why this is hand-rolled rather than `hound`
//!
//! `hound` writes `"fmt "` and `data` and offers no hook for a third chunk, so
//! it cannot write `bext` at all. That is the entire reason this file exists;
//! everything else about a WAV writer is a hundred lines of byte pushing.
//!
//! # Why `bext` is the highest-leverage 600 bytes in the whole feature
//!
//! `TimeReference` is a 64-bit sample count: samples since local midnight, at
//! **this file's** sample rate. REAPER, Pro Tools, Nuendo and Pyramix all read
//! it and drop the file at its source position, so a take lands in a timeline
//! at the absolute time it was played with nobody dragging anything. Nobody in
//! the piano-tool space writes it. It costs ~600 bytes and one `u64`.
//!
//! It is also the reason `Wallclock` is passed in rather than read here: this
//! crate has no timezone database (no `chrono`, no `time`, and `std` cannot do
//! local civil time), and the `bext` date must be the *same* instant that named
//! the take directory in §9. Two independent conversions is how a folder called
//! `2026-08-15_143207` ends up holding a file that a DAW places at 21:32.
//!
//! # The traps, each with a test below
//!
//! 1. **A WAV's RIFF and `data` sizes are normally patched at close, and a file
//!    whose header says 0 plays as empty.** Kill the process mid-take and the
//!    whole performance is on disk and unreachable. So both fields are patched
//!    every couple of seconds *during* the take and the worst case is a couple
//!    of seconds of tail, not the take.
//! 2. **Patching moves the append cursor.** Seek to offset 4, write, forget to
//!    seek back, and the next block of audio overwrites the header. The cursor
//!    is restored from the writer's own byte count every time.
//! 3. **A buffered writer plus a patched size is a header that claims bytes
//!    which are still in userspace.** After a crash those bytes never existed
//!    and the tail is garbage. There is deliberately no `BufWriter` here: one
//!    `write_all` per block, so everything the header claims is already the
//!    kernel's problem and survives the process dying.
//! 4. **`bext` fields are fixed-width slots, not strings.** A 300-character
//!    description, or a single `é`, shifts every following field and the
//!    `TimeReference` a DAW reads is then whatever landed in those four bytes.
//!    Non-ASCII is stripped before truncation, so truncation can never split a
//!    character, and every slot is NUL-padded to its exact size.
//! 5. **An odd-length chunk needs a RIFF pad byte** that is *not* counted in the
//!    chunk's size. Skip it and every following chunk starts on an odd offset;
//!    strict parsers reject the file and lenient ones read `"fmt "` as garbage.
//! 6. **24-bit is three little-endian bytes**, not a 32-bit word with a spare.
//! 7. **A sample above full scale must be clamped before it is truncated.**
//!    Rust's `as` casts saturate, so `2.0 * 8388608.0` survives as `16777216`
//!    in an `i32` — and taking its low three bytes gives `0x000000`, a
//!    polarity-flipped zero crossing. A momentary over becomes a click that is
//!    far louder than the clipping it replaced.
//! 8. **Clamping a 32-bit float file destroys the only reason to choose one.**
//!    Float WAV is defined past ±1.0 and a mix that overs can be pulled back
//!    down losslessly, so the float path never clamps — it only counts.
//! 9. **A non-finite sample** from a misbehaving plugin is written as silence
//!    rather than as a NaN bit pattern, because every downstream tool treats a
//!    NaN in a WAV differently and several of them treat it as full scale.
//! 10. **The RIFF size fields are `u32`.** 4 GiB is 4.1 hours at 48 kHz/24-bit
//!     stereo, which a practice session can genuinely reach. Past 75% of the
//!     ceiling [`WavWriter::capacity`] reports [`Capacity::Warning`] — a state
//!     the band can render, not a `println!` nobody sees — and the write that
//!     would actually wrap the fields is refused rather than silently
//!     corrupting a three-hour take.
//! 11. **A float WAV needs `fmt ` with a `cbSize` and a `fact` chunk**, and
//!     `fact`'s frame count is a *third* field that has to be patched during the
//!     take. Miss it and a crashed float take says it holds zero frames.
//! 12. **A block whose length is not a whole number of frames** desynchronises
//!     the channels for the rest of the file — left and right swap and never
//!     swap back. It is refused.

use std::fs::File;
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use crate::clock::{Nanos, Timeline};

/// The largest total file size the `u32` RIFF size field can describe.
///
/// The field holds *file length minus 8*, so this is conservative by those
/// eight bytes. Being conservative here costs 8 bytes of a 4 GiB file and
/// removes any need to reason about the boundary twice.
pub const RIFF_CEILING: u64 = u32::MAX as u64;

/// Fraction of [`RIFF_CEILING`] at which [`Capacity::Warning`] begins.
///
/// At the default 48 kHz/24-bit stereo that is 3 h 6 min, which is the "warn
/// past three hours" of RECORDER-PLAN §9 expressed as a property of the format
/// rather than as a hardcoded number that would be wrong at 16-bit mono.
pub const WARN_FRACTION: f64 = 0.75;

/// The fixed part of a `bext` chunk, before the variable coding history.
///
/// 256 + 32 + 32 + 10 + 8 + 4 + 4 + 2 + 64 + (5 × 2) + 180. Named because the
/// tests assert against it and because a `bext` that is not this long is the
/// one bug in this file no reader will diagnose for you — it will simply place
/// your take at the wrong time.
pub const BEXT_FIXED_LEN: usize = 602;

/// The `Version` field written into `bext`.
///
/// **1, deliberately, not 2.** The byte layout is identical either way: version
/// 2 names the ten bytes at offset 412 as five loudness measurements, version 1
/// calls the same ten bytes reserved-and-zero. Writing version 2 asserts that
/// the loudness fields are meaningful, and Tangent does not measure EBU R128.
/// Claiming a measurement of 0.0 LUFS is worse than claiming none.
pub const BWF_VERSION: u16 = 1;

/// How much audio may go unaccounted for in the header between size patches.
///
/// Two seconds: the patch is three four-byte writes at fixed offsets, so making
/// it rarer buys nothing measurable and costs the tail of a crashed take.
pub const DEFAULT_PATCH_SECONDS: f64 = 2.0;

const WAVE_FORMAT_PCM: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
/// Read but never written: see trap 5 in [`read_pcm`] for why a reader has to
/// know this tag, and the `Int24` doc above for why a writer should not use it.
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

/// The RIFF size field is always at offset 4. It is the one offset in the file
/// that never depends on what was written.
const RIFF_SIZE_OFFSET: u64 = 4;

/// What one sample looks like on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    Int16,
    /// The default, and what every field recorder writes: 24-bit integer with
    /// `WAVE_FORMAT_PCM`. Microsoft's own guidance says anything above 16 bits
    /// should use `WAVE_FORMAT_EXTENSIBLE`; in practice extensible is what
    /// older tools choke on and plain tag-1 24-bit is what Sound Devices, Zoom,
    /// REAPER and Pro Tools all produce and consume.
    Int24,
    /// 32-bit integer. Nothing here writes it by choice — an `f32` source has
    /// 24 bits of mantissa, so a 32-bit integer take carries not one bit more
    /// information than [`SampleFormat::Int24`] and costs a third more disk.
    /// It exists because [`read_pcm`] meets files in this format and the
    /// reader's vocabulary and the writer's must be the same one.
    Int32,
    /// 32-bit IEEE float. Needs a `fact` chunk and a `cbSize` in `fmt ` — see
    /// trap 11.
    Float32,
}

impl SampleFormat {
    pub fn bits(self) -> u16 {
        match self {
            SampleFormat::Int16 => 16,
            SampleFormat::Int24 => 24,
            SampleFormat::Int32 | SampleFormat::Float32 => 32,
        }
    }

    pub fn bytes_per_sample(self) -> u16 {
        self.bits() / 8
    }

    pub fn is_float(self) -> bool {
        matches!(self, SampleFormat::Float32)
    }

    fn tag(self) -> u16 {
        if self.is_float() {
            WAVE_FORMAT_IEEE_FLOAT
        } else {
            WAVE_FORMAT_PCM
        }
    }

    /// The coding-history `A=` code (EBU R98).
    ///
    /// R98 registers no code for floating point, so `A=FLOAT` is a convention
    /// rather than a citation. It is harmless: `fmt ` is authoritative for the
    /// sample format and nothing reads coding history to decide how to decode.
    fn coding_algorithm(self) -> &'static str {
        if self.is_float() {
            "FLOAT"
        } else {
            "PCM"
        }
    }
}

/// Rate, channel count and word size. 48 kHz/24-bit stereo by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavSpec {
    pub sample_rate: u32,
    pub channels: u16,
    pub format: SampleFormat,
}

impl Default for WavSpec {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            format: SampleFormat::Int24,
        }
    }
}

impl WavSpec {
    pub fn bytes_per_frame(&self) -> u64 {
        self.channels as u64 * self.format.bytes_per_sample() as u64
    }

    pub fn bytes_per_second(&self) -> u64 {
        self.sample_rate as u64 * self.bytes_per_frame()
    }

    /// Seconds of audio that fit under the RIFF ceiling, ignoring the ~700-byte
    /// header. 4.14 hours at the default format, which is the number
    /// RECORDER-PLAN §9 quotes.
    pub fn ceiling_seconds(&self) -> f64 {
        let per_sec = self.bytes_per_second();
        if per_sec == 0 {
            return 0.0;
        }
        RIFF_CEILING as f64 / per_sec as f64
    }

    /// When [`Capacity::Warning`] starts, in seconds of audio.
    pub fn warn_seconds(&self) -> f64 {
        self.ceiling_seconds() * WARN_FRACTION
    }
}

/// Local civil time, supplied by the caller.
///
/// **The caller's job, not this module's, and that is the point.** `std` has no
/// timezone database and this crate deliberately has no date dependency, so the
/// only way to be sure the `bext` timestamp agrees with the take directory's
/// `YYYY-MM-DD_HHMMSS` name is for both to come from one value. [`from_unix`]
/// converts an epoch count to civil time with no timezone knowledge at all —
/// feed it `unix_seconds + utc_offset_seconds` and it yields local time; feed it
/// the raw epoch and it yields UTC, which is a legal but worse `bext`.
///
//// Civil time, shared with `take.rs`.
///
/// **Deliberately NOT a second type.** An earlier draft of this file defined its
/// own `Wallclock` with its own Howard-Hinnant conversion, in parallel with
/// `take::WallTime`. Two independent civil-time conversions is precisely the bug
/// this file's own docs warn about: the BWF `OriginationDate`/`Time` and the
/// take DIRECTORY NAME would be derived separately, and any disagreement — a UTC
/// offset applied on one side and not the other, a different rounding of the
/// seconds — puts the file at the wrong place in a REAPER timeline while the
/// folder says otherwise. One instant, one conversion, one type.
pub use crate::take::WallTime as Wallclock;

/// The Broadcast Wave extension chunk.
///
/// Field widths are the spec's and are not negotiable; [`Bext::to_bytes`]
/// enforces every one of them. The fields are public because they are all
/// free-text metadata whose only invariant is "fits in its slot after ASCII
/// sanitisation", and that invariant is applied at serialisation rather than at
/// assignment so a caller can build one incrementally.
#[derive(Debug, Clone)]
pub struct Bext {
    /// Free text, 256 bytes. A take's folder name is a good value.
    pub description: String,
    /// 32 bytes. Who made the file.
    pub originator: String,
    /// 32 bytes. EBU R99 defines a unique-source syntax for this; empty is
    /// legal and is what we write, because a fabricated reference is worse than
    /// none.
    pub originator_reference: String,
    /// The instant sample 0 was captured, in local civil time.
    pub origination: Wallclock,
    /// Samples since midnight at the file's rate. See
    /// [`Wallclock::samples_since_midnight`].
    pub time_reference: u64,
    /// EBU R98 coding history. CRLF-separated, CRLF-terminated.
    pub coding_history: String,
}

impl Bext {
    /// A `bext` describing a take that started at `origination` in `spec`'s
    /// format, with `time_reference` and the coding history derived from both so
    /// they cannot disagree with each other.
    pub fn new(origination: Wallclock, spec: WavSpec) -> Self {
        let mode = match spec.channels {
            1 => "mono",
            2 => "stereo",
            _ => "multi",
        };
        Self {
            description: String::new(),
            originator: concat!("Tangent ", env!("CARGO_PKG_VERSION")).to_string(),
            originator_reference: String::new(),
            origination,
            time_reference: origination.samples_since_midnight(spec.sample_rate),
            coding_history: format!(
                "A={},F={},W={},M={},T={}\r\n",
                spec.format.coding_algorithm(),
                spec.sample_rate,
                spec.format.bits(),
                mode,
                concat!("Tangent ", env!("CARGO_PKG_VERSION")),
            ),
        }
    }

    /// The chunk payload: [`BEXT_FIXED_LEN`] bytes then the coding history.
    ///
    /// Every slot is written by [`ascii_slot`], which strips non-ASCII *before*
    /// truncating. That ordering is trap 4: truncating UTF-8 at byte 256 can
    /// leave half a character, and a `bext` reader does not resynchronise — it
    /// reads the next 32 bytes as the originator whatever they contain, and the
    /// four bytes it later calls `TimeReferenceLow` are somebody's surname.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(BEXT_FIXED_LEN + self.coding_history.len());
        ascii_slot(&mut b, &self.description, 256);
        ascii_slot(&mut b, &self.originator, 32);
        ascii_slot(&mut b, &self.originator_reference, 32);
        ascii_slot(&mut b, &self.origination.date_string(), 10);
        ascii_slot(&mut b, &self.origination.time_string(), 8);
        // Two DWORDs, low then high — which on a little-endian host is the same
        // eight bytes a `u64` would produce, and is not the same eight bytes a
        // big-endian `u64` would produce. RIFF is little-endian everywhere.
        b.extend_from_slice(&(self.time_reference as u32).to_le_bytes());
        b.extend_from_slice(&((self.time_reference >> 32) as u32).to_le_bytes());
        b.extend_from_slice(&BWF_VERSION.to_le_bytes());
        b.extend_from_slice(&[0u8; 64]); // UMID: all-zero means "none"
        b.extend_from_slice(&[0u8; 10]); // the five loudness fields; see BWF_VERSION
        b.extend_from_slice(&[0u8; 180]); // Reserved
        debug_assert_eq!(b.len(), BEXT_FIXED_LEN);

        // The history is the one variable-length field, so it is the one place
        // a stray control character could shift nothing at all — but CR and LF
        // are load-bearing here and must survive the sanitiser.
        for byte in self.coding_history.bytes() {
            if byte == b'\r' || byte == b'\n' || (0x20..0x7F).contains(&byte) {
                b.push(byte);
            }
        }
        if !b.ends_with(b"\r\n") {
            b.extend_from_slice(b"\r\n");
        }
        b
    }
}

/// Copy `s` into exactly `len` bytes, ASCII only, NUL-padded.
fn ascii_slot(out: &mut Vec<u8>, s: &str, len: usize) {
    let start = out.len();
    for byte in s.bytes() {
        if out.len() - start == len {
            break;
        }
        // Anything outside printable ASCII is dropped rather than replaced.
        // The continuation bytes of a multi-byte character are all >= 0x80, so
        // a whole character disappears together and no partial one survives.
        if (0x20..0x7F).contains(&byte) {
            out.push(byte);
        }
    }
    out.resize(start + len, 0);
}

/// How much of the 4 GiB RIFF ceiling is left. A state to render, not a warning
/// to print.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capacity {
    Fine,
    /// Past [`WARN_FRACTION`] of the ceiling. **The take is not stopped for
    /// this** — §9 says warn, do not fail — but the band should say so while
    /// there is still an hour left to say it in.
    Warning,
    /// The next block would push a size field past `u32`. Further writes are
    /// refused so that what is already on disk stays a valid file.
    Full,
}

/// A streaming WAV writer whose file is valid at (almost) every instant.
///
/// "Almost" is honest: between the last size patch and now, up to
/// [`DEFAULT_PATCH_SECONDS`] of audio is on disk but not yet described by the
/// header. Everything before that point is playable even if the process is
/// killed with `SIGKILL` in the middle of a block.
#[derive(Debug)]
pub struct WavWriter {
    file: File,
    spec: WavSpec,
    /// Where the audio starts, and therefore where the append cursor lives.
    header_len: u64,
    data_size_offset: u64,
    /// `Some` only for float files, which carry a `fact` chunk (trap 11).
    fact_offset: Option<u64>,
    data_bytes: u64,
    /// The RIFF pad byte after an odd-length `data`, written by [`finish`].
    ///
    /// [`finish`]: WavWriter::finish
    pad_bytes: u64,
    frames: u64,
    frames_at_last_patch: u64,
    patch_interval_frames: u64,
    clipped: u64,
    ceiling: u64,
    finished: bool,
    /// Reused so that a block of audio costs no allocation on the writer
    /// thread. It is not the audio callback, but it runs once per buffer for
    /// the whole take and there is no reason for it to churn the allocator.
    scratch: Vec<u8>,
}

impl WavWriter {
    /// Create the file and write its header. The file is a valid, empty WAV
    /// before a single sample arrives.
    pub fn create(path: &Path, spec: WavSpec, bext: &Bext) -> io::Result<Self> {
        if spec.channels == 0 || spec.sample_rate == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a WAV needs at least one channel and a non-zero sample rate",
            ));
        }

        let payload = bext.to_bytes();
        let bext_size = u32::try_from(payload.len()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "bext chunk exceeds 4 GiB")
        })?;

        let mut header: Vec<u8> = Vec::with_capacity(payload.len() + 64);
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&0u32.to_le_bytes()); // patched
        header.extend_from_slice(b"WAVE");

        // EBU Tech 3285's own file-structure diagram puts <bext> first, before
        // <fmt >. Readers that only look at the first chunk are broken for any
        // BWF regardless of the order chosen here.
        header.extend_from_slice(b"bext");
        header.extend_from_slice(&bext_size.to_le_bytes());
        header.extend_from_slice(&payload);
        if payload.len() % 2 == 1 {
            header.push(0); // trap 5: pad, and it is NOT in bext_size
        }

        let block_align = spec.bytes_per_frame() as u16;
        // 16 for PCM; 18 for float, because a non-PCM `fmt ` must carry a
        // `cbSize` even when it is zero.
        let fmt_size: u32 = if spec.format.is_float() { 18 } else { 16 };
        header.extend_from_slice(b"fmt ");
        header.extend_from_slice(&fmt_size.to_le_bytes());
        header.extend_from_slice(&spec.format.tag().to_le_bytes());
        header.extend_from_slice(&spec.channels.to_le_bytes());
        header.extend_from_slice(&spec.sample_rate.to_le_bytes());
        header.extend_from_slice(&(spec.bytes_per_second() as u32).to_le_bytes());
        header.extend_from_slice(&block_align.to_le_bytes());
        header.extend_from_slice(&spec.format.bits().to_le_bytes());
        if spec.format.is_float() {
            header.extend_from_slice(&0u16.to_le_bytes()); // cbSize
        }

        let fact_offset = spec.format.is_float().then(|| {
            header.extend_from_slice(b"fact");
            header.extend_from_slice(&4u32.to_le_bytes());
            let at = header.len() as u64;
            header.extend_from_slice(&0u32.to_le_bytes()); // patched
            at
        });

        header.extend_from_slice(b"data");
        let data_size_offset = header.len() as u64;
        header.extend_from_slice(&0u32.to_le_bytes()); // patched

        let mut file = File::create(path)?;
        file.write_all(&header)?;

        let mut w = Self {
            file,
            spec,
            header_len: header.len() as u64,
            data_size_offset,
            fact_offset,
            data_bytes: 0,
            pad_bytes: 0,
            frames: 0,
            frames_at_last_patch: 0,
            patch_interval_frames: (DEFAULT_PATCH_SECONDS * spec.sample_rate as f64) as u64,
            clipped: 0,
            ceiling: RIFF_CEILING,
            finished: false,
            scratch: Vec::new(),
        };
        // An empty take is still a take: patch now so a file that never sees a
        // sample is a valid zero-length WAV rather than one whose RIFF size is
        // the placeholder zero.
        w.patch_sizes()?;
        Ok(w)
    }

    /// Append interleaved frames, converting to the file's word size.
    ///
    /// Also patches the size fields when enough audio has gone by, because a
    /// crash-safety mechanism the caller has to remember to call is a
    /// crash-safety mechanism that is not there.
    pub fn write_interleaved(&mut self, samples: &[f32]) -> io::Result<()> {
        let channels = self.spec.channels as usize;
        if samples.len() % channels != 0 {
            // Trap 12. Writing the partial frame would rotate every subsequent
            // sample by one channel for the rest of the take: the file plays,
            // the meters look right, and left and right are swapped from that
            // point on.
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "{} samples is not a whole number of {}-channel frames",
                    samples.len(),
                    channels
                ),
            ));
        }

        let frames = (samples.len() / channels) as u64;
        let block = frames * self.spec.bytes_per_frame();
        if self.total_bytes() + block > self.ceiling {
            // Trap 10. Refuse the block whole: a partially written block is a
            // partially written frame, and the file that is already on disk is
            // worth more than these few milliseconds.
            return Err(io::Error::new(
                io::ErrorKind::FileTooLarge,
                "a RIFF file cannot exceed 4 GiB; stop the take and start another",
            ));
        }

        self.scratch.clear();
        self.scratch.reserve(block as usize);
        match self.spec.format {
            SampleFormat::Int16 => {
                for &s in samples {
                    let (v, clipped) = quantise(s, 16);
                    self.clipped += u64::from(clipped);
                    self.scratch.extend_from_slice(&(v as i16).to_le_bytes());
                }
            }
            SampleFormat::Int24 => {
                for &s in samples {
                    let (v, clipped) = quantise(s, 24);
                    self.clipped += u64::from(clipped);
                    // Trap 6: the low three bytes, little end first. For a
                    // negative value the arithmetic shift keeps the sign bits
                    // where two's complement wants them: -8388608 is 00 00 80.
                    self.scratch.push(v as u8);
                    self.scratch.push((v >> 8) as u8);
                    self.scratch.push((v >> 16) as u8);
                }
            }
            SampleFormat::Int32 => {
                for &s in samples {
                    let (v, clipped) = quantise(s, 32);
                    self.clipped += u64::from(clipped);
                    self.scratch.extend_from_slice(&v.to_le_bytes());
                }
            }
            SampleFormat::Float32 => {
                for &s in samples {
                    // Trap 8: counted, never clamped. Trap 9: a non-finite
                    // sample is the one value that cannot be preserved.
                    let broken = !s.is_finite();
                    let v = if broken { 0.0 } else { s };
                    self.clipped += u64::from(broken || is_at_full_scale(s));
                    self.scratch.extend_from_slice(&v.to_le_bytes());
                }
            }
        }

        self.file.write_all(&self.scratch)?;
        self.data_bytes += block;
        self.frames += frames;

        if self.frames - self.frames_at_last_patch >= self.patch_interval_frames {
            self.patch_sizes()?;
        }
        Ok(())
    }

    /// Rewrite the RIFF size, the `data` size and (for float) the `fact` frame
    /// count, then put the append cursor back exactly where it was.
    ///
    /// Trap 2 lives here. The cursor is restored from this writer's own byte
    /// count rather than left wherever the last patch ended, because the next
    /// thing to touch this handle is a block of audio and a cursor at offset 8
    /// turns it into a new file header.
    pub fn patch_sizes(&mut self) -> io::Result<()> {
        let total = self.total_bytes();
        let riff = (total - 8) as u32;
        let data = self.data_bytes as u32;

        self.patch_u32(RIFF_SIZE_OFFSET, riff)?;
        self.patch_u32(self.data_size_offset, data)?;
        if let Some(at) = self.fact_offset {
            self.patch_u32(at, self.frames as u32)?;
        }
        self.file.seek(SeekFrom::Start(total))?;
        self.frames_at_last_patch = self.frames;
        Ok(())
    }

    fn patch_u32(&mut self, at: u64, value: u32) -> io::Result<()> {
        self.file.seek(SeekFrom::Start(at))?;
        self.file.write_all(&value.to_le_bytes())
    }

    /// Close the take: pad, patch, and get the bytes onto the platter.
    ///
    /// `sync_all` is worth its few milliseconds exactly once. It is *not* worth
    /// it per patch: an ordinary `write` already survives the process dying,
    /// which is the failure this file is defending against, and only power loss
    /// needs the barrier.
    pub fn finish(mut self) -> io::Result<()> {
        if self.data_bytes % 2 == 1 {
            // Trap 5 again, for `data` this time: an odd-length chunk is padded
            // to even. Nothing follows `data`, so a crashed take that never got
            // here is still readable — this is tidiness for the spec's sake,
            // not a recovery concern.
            self.file.seek(SeekFrom::Start(self.header_len + self.data_bytes))?;
            self.file.write_all(&[0])?;
            self.pad_bytes = 1;
        }
        self.patch_sizes()?;
        self.file.sync_all()?;
        self.finished = true;
        Ok(())
    }

    /// Bytes on disk, header included.
    pub fn total_bytes(&self) -> u64 {
        self.header_len + self.data_bytes + self.pad_bytes
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }

    pub fn spec(&self) -> WavSpec {
        self.spec
    }

    /// How much audio is in the file, on the file's own timeline.
    ///
    /// Integer arithmetic in `u128` rather than `frames as f64 / rate`, because
    /// this is the number the take's manifest reports and a rounding error here
    /// is indistinguishable from a real drift measurement.
    pub fn duration_ns(&self) -> Nanos {
        (self.frames as u128 * 1_000_000_000 / self.spec.sample_rate as u128) as Nanos
    }

    /// Samples the take expected to write minus samples actually written.
    ///
    /// Positive means audio was lost — the ring overran, or the writer thread
    /// was starved. **That loss is completely silent in the file**: a WAV that
    /// is 200 ms short simply plays 200 ms short, and everything after it in a
    /// timeline is early by 200 ms with no artefact to point at. §9 wants
    /// frames-expected versus frames-received in `take.json` for video; this is
    /// the same number for audio, and it is the only place a dropout leaves a
    /// trace.
    pub fn deficit_frames(&self, timeline: &Timeline) -> f64 {
        timeline.file_sample(timeline.t1()) - self.frames as f64
    }

    /// Samples written at or beyond full scale, plus any non-finite sample.
    ///
    /// A latch for the "I recorded silence / I recorded distortion" failure
    /// class (§5): it is reported after Stop, when the user can still play the
    /// take again. Exactly ±1.0 counts, because a converter that reached the
    /// rail is what the meter is asking about even though ±1.0 itself quantises
    /// without distortion.
    pub fn clipped_samples(&self) -> u64 {
        self.clipped
    }

    pub fn capacity(&self) -> Capacity {
        let used = self.total_bytes();
        if used + self.spec.bytes_per_frame() > self.ceiling {
            Capacity::Full
        } else if used as f64 >= self.ceiling as f64 * WARN_FRACTION {
            Capacity::Warning
        } else {
            Capacity::Fine
        }
    }

    /// Recording time left before [`Capacity::Full`], for the band's
    /// "~58 min at current settings" readout.
    pub fn seconds_remaining(&self) -> f64 {
        let per_sec = self.spec.bytes_per_second();
        if per_sec == 0 {
            return 0.0;
        }
        self.ceiling.saturating_sub(self.total_bytes()) as f64 / per_sec as f64
    }

    /// How much audio may go unaccounted for between size patches.
    pub fn set_patch_seconds(&mut self, seconds: f64) {
        self.patch_interval_frames =
            (seconds.max(0.0) * self.spec.sample_rate as f64) as u64;
    }

    /// Pretend the RIFF ceiling is `bytes`.
    ///
    /// A test seam, and the only honest way to cover trap 10: proving that the
    /// ceiling is refused rather than wrapped otherwise costs 4 GiB of disk and
    /// four hours of writes per run.
    #[cfg(test)]
    fn shrink_ceiling(&mut self, bytes: u64) {
        self.ceiling = bytes;
    }
}

impl Drop for WavWriter {
    /// A last patch on the way out, so that a writer thread which panics —
    /// or a `?` that returns early out of the session — still leaves a header
    /// describing every byte that reached the file. Errors are unreportable
    /// here and the periodic patch has already bounded the damage.
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.patch_sizes();
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Reading
// ───────────────────────────────────────────────────────────────────────────

/// Decode a whole PCM or float WAV held in memory.
///
/// The counterpart to [`WavWriter`], and deliberately the smaller half: this
/// exists so the app can `include_bytes!` a fixed asset — the metronome click —
/// and get samples out of it without a decoder dependency. It reads a file that
/// is already in memory and returns every sample interleaved as `f32`, which is
/// what everything downstream of here speaks.
///
/// # The traps, each with a test below
///
/// 1. **A WAV is a chunk LIST and `fmt ` is not guaranteed to be first.** The
///    metronome asset this was written for opens `RIFF/WAVE` and then a
///    **`JUNK` chunk of 28 bytes** before `fmt ` — a padding chunk written by
///    Pro Tools and by half the hardware recorders on the market, so that a
///    later `ds64` can be dropped in without moving the audio. A reader that
///    assumes "header, then `fmt `, then `data`" reads the JUNK payload as a
///    format block: zeroes, which is a zero channel count and a zero sample
///    rate, and every division after that is by zero. So the chunk list is
///    walked, by id, to the end.
/// 2. **An odd-length chunk carries a pad byte that is not in its size.** Miss
///    it and every following chunk id is read one byte early, which is the
///    same failure as trap 1 but harder to see.
/// 3. **A chunk size may be a lie.** Files truncated by a crashed recorder
///    routinely claim a `data` size larger than the bytes that follow, and a
///    streaming writer legitimately writes `0xFFFFFFFF`. Every read is bounded
///    by the slice, never by the declared size, and a short `data` yields the
///    audio that is really there rather than an error.
/// 4. **24-bit is three bytes and must be sign-extended by hand.** Assembling
///    them into an `i32` and forgetting the shift makes every negative sample a
///    very large positive one: full-scale noise on the bottom half of the wave.
/// 5. **`WAVE_FORMAT_EXTENSIBLE` (0xFFFE) hides the real format in a GUID.**
///    Anything above 16 bits from a modern DAW is likely to be tagged this way;
///    reading tag 0xFFFE as "unknown" rejects perfectly ordinary files. The
///    first two bytes of the subformat GUID are the tag it stands for.
/// 6. **8-bit WAV is UNSIGNED**, with 0x80 as zero, and it is the one common
///    format whose silence is not a run of zero bytes. Rather than carry a
///    format the writer cannot write, it is refused by name — a clear error
///    beats a plausible one that plays as a square wave.
///
/// The returned [`WavSpec`] describes the FILE, not the returned samples: its
/// `format` is what was on disk even though what comes back is always `f32`.
pub fn read_pcm(bytes: &[u8]) -> Result<(WavSpec, Vec<f32>), String> {
    if bytes.len() < 12 {
        return Err(format!("not a WAV: {} bytes is shorter than a RIFF header", bytes.len()));
    }
    if &bytes[0..4] != b"RIFF" {
        // RF64 is the >4 GiB variant and is a different container with a `ds64`
        // chunk; naming it is more useful than "bad magic".
        let what = if &bytes[0..4] == b"RF64" { "an RF64 file, not a RIFF WAV" } else { "not a RIFF file" };
        return Err(format!("{what} (magic {:?})", String::from_utf8_lossy(&bytes[0..4])));
    }
    if &bytes[8..12] != b"WAVE" {
        return Err(format!(
            "RIFF form is {:?}, not WAVE",
            String::from_utf8_lossy(&bytes[8..12])
        ));
    }

    let mut fmt: Option<&[u8]> = None;
    let mut data: Option<&[u8]> = None;
    let mut at = 12usize;
    // Trap 1: walk the list. `at + 8` rather than `at < len` so a trailing
    // fragment too short to hold a chunk header ends the walk instead of
    // indexing past it.
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let declared = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]]) as usize;
        let body = at + 8;
        // Trap 3: the slice is the authority, not the size field.
        let end = body.saturating_add(declared).min(bytes.len());
        match id {
            b"fmt " => fmt = Some(&bytes[body..end]),
            b"data" => data = Some(&bytes[body..end]),
            _ => {}
        }
        // Trap 2: the pad byte is not counted in the chunk's size. Checked,
        // because a `0xFFFFFFFF` size from a streaming writer would otherwise
        // wrap the cursor back to the start of the file and loop forever.
        match body.checked_add(declared).and_then(|n| n.checked_add(declared & 1)) {
            Some(next) => at = next,
            None => break,
        }
    }

    let fmt = fmt.ok_or_else(|| "no fmt chunk".to_string())?;
    if fmt.len() < 16 {
        return Err(format!("fmt chunk is {} bytes, and the minimum is 16", fmt.len()));
    }
    let mut tag = u16::from_le_bytes([fmt[0], fmt[1]]);
    let channels = u16::from_le_bytes([fmt[2], fmt[3]]);
    let sample_rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
    let bits = u16::from_le_bytes([fmt[14], fmt[15]]);

    // Trap 5. The extension is `cbSize`(2) + validBits(2) + channelMask(4) +
    // a 16-byte GUID whose first two bytes are the tag it stands for.
    if tag == WAVE_FORMAT_EXTENSIBLE {
        if fmt.len() < 26 {
            return Err("fmt says WAVE_FORMAT_EXTENSIBLE but carries no subformat GUID".to_string());
        }
        tag = u16::from_le_bytes([fmt[24], fmt[25]]);
    }

    if channels == 0 {
        return Err("fmt declares zero channels".to_string());
    }
    if sample_rate == 0 {
        return Err("fmt declares a zero sample rate".to_string());
    }

    let format = match (tag, bits) {
        (WAVE_FORMAT_PCM, 16) => SampleFormat::Int16,
        (WAVE_FORMAT_PCM, 24) => SampleFormat::Int24,
        (WAVE_FORMAT_PCM, 32) => SampleFormat::Int32,
        (WAVE_FORMAT_IEEE_FLOAT, 32) => SampleFormat::Float32,
        // Trap 6.
        (WAVE_FORMAT_PCM, 8) => {
            return Err("8-bit WAV is unsigned and is not supported; convert it to 16-bit".to_string())
        }
        (WAVE_FORMAT_IEEE_FLOAT, 64) => {
            return Err("64-bit float WAV is not supported; convert it to 32-bit float".to_string())
        }
        (WAVE_FORMAT_PCM, other) => return Err(format!("{other}-bit PCM is not supported")),
        (other, bits) => return Err(format!("format tag {other} at {bits} bits is not supported")),
    };

    let spec = WavSpec {
        sample_rate,
        channels,
        format,
    };
    let Some(data) = data else {
        return Err("no data chunk".to_string());
    };

    let stride = format.bytes_per_sample() as usize;
    // Trap 3 again, and trap 12's reading half: a `data` chunk that ends
    // mid-frame yields whole frames and drops the fragment. Keeping it would
    // rotate the channels of everything that came before it relative to
    // whatever is appended next.
    let frames = data.len() / (stride * channels as usize);
    let samples = frames * channels as usize;
    let mut out = Vec::with_capacity(samples);
    for i in 0..samples {
        let s = &data[i * stride..i * stride + stride];
        out.push(match format {
            SampleFormat::Int16 => {
                i32::from(i16::from_le_bytes([s[0], s[1]])) as f32 / full_scale(16)
            }
            // Trap 4: build the three bytes into the TOP of an i32 and shift
            // back down arithmetically. `(s[2] as i32) << 16` alone leaves
            // every negative sample as a large positive one.
            SampleFormat::Int24 => {
                let raw = (i32::from(s[0]) << 8) | (i32::from(s[1]) << 16) | (i32::from(s[2]) << 24);
                (raw >> 8) as f32 / full_scale(24)
            }
            SampleFormat::Int32 => {
                i32::from_le_bytes([s[0], s[1], s[2], s[3]]) as f32 / full_scale(32)
            }
            SampleFormat::Float32 => f32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        });
    }
    Ok((spec, out))
}

/// `2^(bits-1)`: the scale [`quantise`] multiplies by and [`read_pcm`] divides
/// by.
///
/// One function rather than two constants on purpose. If the reader and the
/// writer ever disagreed about full scale, a file would come back a fraction of
/// a dB from where it went in and nothing would say so.
fn full_scale(bits: u32) -> f32 {
    (1u32 << (bits - 1)) as f32
}

/// One float sample to an integer of `bits` bits, and whether it was at or past
/// full scale.
///
/// The scale is `2^(bits-1)`, not `2^(bits-1) - 1`. Using the smaller number is
/// the "symmetric" choice and it is wrong twice over: -1.0 then fails to reach
/// the minimum code, and every full-scale signal reads 0.1 dB low against every
/// other tool. The clamp is trap 7 and is not optional — `as` saturates at
/// `i32`, not at 24 bits, so an unclamped over survives the cast and then wraps
/// when its low three bytes are taken.
fn quantise(x: f32, bits: u32) -> (i32, bool) {
    if !x.is_finite() {
        return (0, true);
    }
    let scale = full_scale(bits);
    // At 32 bits `scale - 1.0` is `scale` again — an f32 has 24 bits of
    // mantissa and cannot represent 2^31 - 1 — so the clamp does not bound the
    // cast on its own. The `as` cast saturates at `i32::MAX`, which is the same
    // answer, and that is why this is safe rather than lucky.
    let max = scale - 1.0;
    let min = -scale;
    ((x * scale).round().clamp(min, max) as i32, is_at_full_scale(x))
}

/// Written as two comparisons rather than as `!(-1.0..1.0).contains(&x)`,
/// because a `Range` is half-open: that expression silently exempts exactly
/// -1.0, which is the one full-scale value a DC-offset or a hard-limited signal
/// is most likely to sit on. A NaN is false here — non-finite samples are
/// counted by their own arm, so nothing is ever counted twice.
fn is_at_full_scale(x: f32) -> bool {
    x >= 1.0 || x <= -1.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A temp path that cleans itself up. No `tempfile` dependency: this crate
    /// has three dependencies and none of them is for tests.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        static N: AtomicU32 = AtomicU32::new(0);
        let mut p = std::env::temp_dir();
        p.push(format!(
            "ivory-wav-{}-{}-{}.wav",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed),
            name
        ));
        let _ = std::fs::remove_file(&p);
        Scratch(p)
    }

    /// 2026-08-15T14:32:07Z.
    ///
    /// Built from an instant rather than by hand, which is the point of sharing
    /// one type with `take.rs`: a literal could assert a calendar date that the
    /// shared conversion would never produce, and then this file's tests would
    /// pass while a real take's folder name and BWF stamp disagreed.
    fn wallclock() -> Wallclock {
        let w = Wallclock::from_unix(1_786_804_327, 0);
        debug_assert_eq!((w.year, w.month, w.day), (2026, 8, 15));
        debug_assert_eq!((w.hour, w.minute, w.second), (14, 32, 7));
        w
    }

    /// A chunk walker written from the RIFF rules rather than from this file's
    /// opinion of them, so that a bug in the writer cannot be a matching bug in
    /// the reader. Returns `(id, payload offset, payload length)`.
    fn chunks(bytes: &[u8]) -> Vec<(String, usize, usize)> {
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let mut out = Vec::new();
        let mut at = 12usize;
        while at + 8 <= bytes.len() {
            let id = String::from_utf8_lossy(&bytes[at..at + 4]).to_string();
            let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
            out.push((id, at + 8, size));
            at += 8 + size + (size % 2); // the pad byte is not in `size`
        }
        out
    }

    fn find<'a>(cs: &'a [(String, usize, usize)], id: &str) -> &'a (String, usize, usize) {
        cs.iter().find(|c| c.0 == id).unwrap_or_else(|| panic!("no {id} chunk"))
    }

    fn le32(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn le16(bytes: &[u8], at: usize) -> u16 {
        u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
    }

    #[test]
    fn a_fresh_file_is_a_valid_empty_wav_before_a_single_sample_arrives() {
        let p = scratch("empty");
        let spec = WavSpec::default();
        let w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        let bytes = std::fs::read(&p.0).unwrap();
        drop(w);

        assert_eq!(
            le32(&bytes, 4) as usize,
            bytes.len() - 8,
            "the RIFF size must describe the file even before any audio, or an \
             arm-then-abort leaves a header full of placeholder zeros"
        );
        let cs = chunks(&bytes);
        assert_eq!(find(&cs, "data").2, 0);
    }

    #[test]
    fn the_default_is_forty_eight_kilohertz_twenty_four_bit_stereo() {
        let p = scratch("default");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[0.0; 8]).unwrap();
        w.finish().unwrap();

        let bytes = std::fs::read(&p.0).unwrap();
        let cs = chunks(&bytes);
        let (_, fmt, size) = *find(&cs, "fmt ");
        assert_eq!(size, 16, "a PCM fmt chunk is 16 bytes with no cbSize");
        assert_eq!(le16(&bytes, fmt), 1, "WAVE_FORMAT_PCM");
        assert_eq!(le16(&bytes, fmt + 2), 2, "stereo");
        assert_eq!(le32(&bytes, fmt + 4), 48_000);
        assert_eq!(le32(&bytes, fmt + 8), 288_000, "byte rate");
        assert_eq!(le16(&bytes, fmt + 12), 6, "block align");
        assert_eq!(le16(&bytes, fmt + 14), 24);
    }

    #[test]
    fn an_interrupted_take_is_a_short_but_valid_file() {
        // The whole point of the module. Nothing here calls finish(), and the
        // writer is still alive and holding the handle when the file is read —
        // exactly the state a SIGKILL leaves behind.
        let p = scratch("interrupted");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.set_patch_seconds(0.0); // patch after every block, so the test is deterministic
        for _ in 0..100 {
            w.write_interleaved(&[0.25, -0.25]).unwrap();
        }

        let bytes = std::fs::read(&p.0).unwrap();
        assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8, "RIFF size");
        let cs = chunks(&bytes);
        let (_, data, size) = *find(&cs, "data");
        assert_eq!(size, 100 * 6, "100 stereo 24-bit frames");
        assert_eq!(data + size, bytes.len(), "data must run to the end of file");
        // And the audio is really there, not just claimed.
        let first = i32::from_le_bytes([0, bytes[data], bytes[data + 1], bytes[data + 2]]) >> 8;
        assert_eq!(first, (0.25 * 8_388_608.0) as i32);
    }

    #[test]
    fn patching_does_not_disturb_the_append_cursor() {
        // Trap 2. Without the restoring seek, the second block lands at offset
        // 8 and eats the header.
        let p = scratch("cursor");
        let spec = WavSpec {
            format: SampleFormat::Int16,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.set_patch_seconds(0.0);
        for k in 0..64i32 {
            let v = k as f32 / 1024.0;
            w.write_interleaved(&[v, -v]).unwrap();
        }
        w.finish().unwrap();

        let bytes = std::fs::read(&p.0).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF", "the header was overwritten");
        let cs = chunks(&bytes);
        let (_, data, size) = *find(&cs, "data");
        assert_eq!(size, 64 * 4);
        for k in 0..64i32 {
            let at = data + k as usize * 4;
            let l = i16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
            assert_eq!(
                l,
                ((k as f32 / 1024.0) * 32_768.0).round() as i16,
                "frame {k} is not the frame that was written"
            );
        }
    }

    #[test]
    fn the_bext_chunk_has_every_field_at_the_offset_the_spec_says() {
        // Offsets hardcoded from EBU Tech 3285 rather than derived from the
        // writer, because a test that asks the code where its fields are can
        // only ever agree with it.
        let p = scratch("bext");
        let spec = WavSpec::default();
        let mut b = Bext::new(wallclock(), spec);
        b.description = "nocturne".to_string();
        let w = WavWriter::create(&p.0, spec, &b).unwrap();
        let bytes = std::fs::read(&p.0).unwrap();
        drop(w);

        let cs = chunks(&bytes);
        let (_, at, size) = *find(&cs, "bext");
        assert!(size >= BEXT_FIXED_LEN, "bext is {size} bytes, short of 602");
        let x = &bytes[at..at + size];

        assert_eq!(&x[0..8], b"nocturne");
        assert!(x[8..256].iter().all(|b| *b == 0), "Description must NUL-pad");
        assert_eq!(&x[256..263], b"Tangent");
        assert!(x[288..320].iter().all(|b| *b == 0), "OriginatorReference");
        assert_eq!(&x[320..330], b"2026-08-15");
        assert_eq!(&x[330..338], b"14:32:07");
        let low = le32(x, 338) as u64;
        let high = le32(x, 342) as u64;
        assert_eq!(low | (high << 32), 52_327 * 48_000, "TimeReference");
        assert_eq!(le16(x, 346), BWF_VERSION);
        assert!(x[348..412].iter().all(|b| *b == 0), "UMID is 64 bytes");
        assert!(x[412..422].iter().all(|b| *b == 0), "5 loudness fields");
        assert!(x[422..602].iter().all(|b| *b == 0), "Reserved is 180 bytes");
        assert!(
            x[602..].ends_with(b"\r\n"),
            "coding history must be CRLF-terminated: {:?}",
            String::from_utf8_lossy(&x[602..])
        );
        assert!(String::from_utf8_lossy(&x[602..]).contains("F=48000"));
    }

    #[test]
    fn time_reference_is_samples_since_midnight_at_the_files_own_rate() {
        // The number a DAW turns into a timeline position. At 44.1k the same
        // instant is a different count, which is the trap in using a constant
        // 48000 anywhere in this calculation.
        let noon = Wallclock {
            hour: 12,
            minute: 0,
            second: 0,
            nanos: 500_000_000,
            ..wallclock()
        };
        assert_eq!(
            noon.samples_since_midnight(48_000),
            43_200 * 48_000 + 24_000,
            "half a second of sub-second precision must survive"
        );
        assert_eq!(noon.samples_since_midnight(44_100), 43_200 * 44_100 + 22_050);
    }

    #[test]
    fn a_time_reference_past_midday_still_fits_when_it_crosses_thirty_two_bits() {
        // 2^32 samples at 48 kHz is 24 h 51 m, so a single day never overflows —
        // but at 192 kHz it is 6 h 12 m, and a take at 18:00 needs the high
        // DWORD to be more than decoration.
        let evening = Wallclock {
            hour: 18,
            ..wallclock()
        };
        let tr = evening.samples_since_midnight(192_000);
        assert!(tr > u32::MAX as u64, "this case must exercise the high word");
        let spec = WavSpec {
            sample_rate: 192_000,
            ..WavSpec::default()
        };
        let x = Bext::new(evening, spec).to_bytes();
        let low = le32(&x, 338) as u64;
        let high = le32(&x, 342) as u64;
        assert_eq!(low | (high << 32), tr);
    }

    #[test]
    fn an_over_long_or_non_ascii_description_never_shifts_the_next_field() {
        // Trap 4: this is the bug that silently moves TimeReference.
        let spec = WavSpec::default();
        let mut b = Bext::new(wallclock(), spec);
        b.description = "é".repeat(400);
        b.originator = "Tangent".to_string();
        let x = b.to_bytes();
        assert!(x.len() >= BEXT_FIXED_LEN);
        assert!(
            x[0..256].iter().all(|c| *c == 0),
            "a non-ASCII description must vanish rather than land as mojibake"
        );
        assert_eq!(&x[256..263], b"Tangent", "Originator must still start at 256");

        b.description = "x".repeat(400);
        let x = b.to_bytes();
        assert_eq!(&x[0..256], "x".repeat(256).as_bytes());
        assert_eq!(&x[256..263], b"Tangent");
    }

    #[test]
    fn an_odd_length_coding_history_leaves_the_next_chunk_word_aligned() {
        // Trap 5. Without the pad byte `fmt ` starts on an odd offset and a
        // strict parser rejects the file outright.
        let p = scratch("pad");
        let spec = WavSpec::default();
        let mut b = Bext::new(wallclock(), spec);
        // Force an odd payload: 602 is even, so an odd history is what is needed.
        b.coding_history = "A=PCM\r\n".to_string(); // 7 bytes
        assert_eq!((BEXT_FIXED_LEN + b.coding_history.len()) % 2, 1);
        let w = WavWriter::create(&p.0, spec, &b).unwrap();
        let bytes = std::fs::read(&p.0).unwrap();
        drop(w);

        let cs = chunks(&bytes);
        let (_, fmt, _) = *find(&cs, "fmt ");
        assert_eq!(fmt % 2, 0, "fmt payload must be word aligned");
        assert_eq!(&bytes[fmt - 8..fmt - 4], b"fmt ", "the walker found garbage");
        assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8, "pad counts in RIFF");
    }

    #[test]
    fn twenty_four_bit_samples_are_three_little_endian_bytes() {
        let p = scratch("i24");
        let spec = WavSpec {
            channels: 1,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[-1.0]).unwrap();
        w.finish().unwrap();

        let bytes = std::fs::read(&p.0).unwrap();
        let cs = chunks(&bytes);
        let (_, data, size) = *find(&cs, "data");
        assert_eq!(size, 3, "one mono 24-bit frame is three bytes, not four");
        assert_eq!(
            &bytes[data..data + 3],
            &[0x00, 0x00, 0x80],
            "-1.0 is the minimum code -8388608, little end first"
        );
    }

    #[test]
    fn an_odd_length_data_chunk_is_padded_at_finish() {
        let p = scratch("odddata");
        let spec = WavSpec {
            channels: 1,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[0.0; 3]).unwrap(); // 9 bytes
        w.finish().unwrap();

        let bytes = std::fs::read(&p.0).unwrap();
        let cs = chunks(&bytes);
        assert_eq!(find(&cs, "data").2, 9);
        assert_eq!(bytes.len() % 2, 0, "the file must end on an even offset");
        assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8);
    }

    #[test]
    fn an_over_is_clamped_to_the_rail_rather_than_wrapping_to_the_other_one() {
        // Trap 7, and the reason it matters: an unclamped 2.0 keeps its bit 24
        // through the i32 cast and then loses it to the three-byte truncation,
        // so a moment of clipping becomes a full-scale polarity flip — a click
        // that is louder and uglier than the distortion it stood in for.
        assert_eq!(quantise(2.0, 24).0, 8_388_607);
        assert_eq!(quantise(-2.0, 24).0, -8_388_608);
        assert_eq!(quantise(1.0, 16).0, 32_767);
        assert_eq!(quantise(-1.0, 16).0, -32_768, "-1.0 must reach the minimum code");
        assert_eq!(quantise(0.5, 16).0, 16_384);
    }

    #[test]
    fn clipping_is_latched_and_counted_so_stop_can_report_it() {
        let p = scratch("clip");
        let spec = WavSpec {
            channels: 1,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[0.5, 0.999, -0.999]).unwrap();
        assert_eq!(w.clipped_samples(), 0, "nothing here reached the rail");
        w.write_interleaved(&[1.0, -1.0, 2.0, -2.0]).unwrap();
        assert_eq!(w.clipped_samples(), 4);
        w.finish().unwrap();
    }

    #[test]
    fn a_non_finite_sample_becomes_silence_rather_than_a_full_scale_click() {
        // Trap 9. A NaN reaches this module only from a broken plugin or an
        // uninitialised buffer, and every reader downstream interprets one
        // differently; silence is the only interpretation that cannot be loud.
        assert_eq!(quantise(f32::NAN, 24), (0, true));
        assert_eq!(quantise(f32::INFINITY, 24), (0, true));

        let p = scratch("nan");
        let spec = WavSpec {
            channels: 1,
            format: SampleFormat::Float32,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[f32::NAN]).unwrap();
        w.finish().unwrap();
        let bytes = std::fs::read(&p.0).unwrap();
        let cs = chunks(&bytes);
        let (_, data, _) = *find(&cs, "data");
        let v = f32::from_le_bytes(bytes[data..data + 4].try_into().unwrap());
        assert_eq!(v, 0.0, "a NaN must not reach the file even in float");
    }

    #[test]
    fn a_float_file_is_never_clamped_because_that_is_why_float_was_chosen() {
        // Trap 8. Clamping here would throw away headroom the user explicitly
        // asked to keep, and the loss is not recoverable afterwards.
        let p = scratch("f32");
        let spec = WavSpec {
            channels: 1,
            format: SampleFormat::Float32,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.write_interleaved(&[2.5, -2.5]).unwrap();
        assert_eq!(w.clipped_samples(), 2, "still counted, just not altered");
        w.finish().unwrap();

        let bytes = std::fs::read(&p.0).unwrap();
        let cs = chunks(&bytes);
        let (_, fmt, size) = *find(&cs, "fmt ");
        assert_eq!(size, 18, "a non-PCM fmt chunk carries a cbSize");
        assert_eq!(le16(&bytes, fmt), 3, "WAVE_FORMAT_IEEE_FLOAT");
        let (_, data, _) = *find(&cs, "data");
        assert_eq!(
            f32::from_le_bytes(bytes[data..data + 4].try_into().unwrap()),
            2.5
        );
    }

    #[test]
    fn a_crashed_float_take_still_says_how_many_frames_it_holds() {
        // Trap 11: `fact` is the third field that has to be patched during the
        // take, and it is the one nobody remembers because PCM files do not
        // have it.
        let p = scratch("fact");
        let spec = WavSpec {
            format: SampleFormat::Float32,
            ..WavSpec::default()
        };
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.set_patch_seconds(0.0);
        w.write_interleaved(&[0.0; 20]).unwrap(); // 10 stereo frames
        let bytes = std::fs::read(&p.0).unwrap();
        drop(w);

        let cs = chunks(&bytes);
        let (_, fact, size) = *find(&cs, "fact");
        assert_eq!(size, 4);
        assert_eq!(le32(&bytes, fact), 10, "sample frames, not samples or bytes");
    }

    #[test]
    fn a_partial_frame_is_refused_rather_than_swapping_the_channels_forever() {
        let p = scratch("partial");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        let e = w.write_interleaved(&[0.0, 0.0, 0.0]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(w.frames(), 0, "nothing may have been written");
    }

    #[test]
    fn a_spec_with_no_channels_is_refused_instead_of_dividing_by_zero() {
        let p = scratch("zero");
        let spec = WavSpec {
            channels: 0,
            ..WavSpec::default()
        };
        let e = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn the_riff_ceiling_is_refused_rather_than_wrapped() {
        // Trap 10 with a 4 GiB ceiling shrunk to a few hundred bytes, because
        // the honest version of this test writes for four hours.
        let p = scratch("ceiling");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        w.set_patch_seconds(0.0);
        let ceiling = w.total_bytes() + 60;
        w.shrink_ceiling(ceiling);
        for _ in 0..10 {
            w.write_interleaved(&[0.1, 0.1]).unwrap(); // 6 bytes each
        }
        assert_eq!(w.capacity(), Capacity::Full);
        let e = w.write_interleaved(&[0.1, 0.1]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::FileTooLarge);
        assert_eq!(w.frames(), 10, "the refused block must not be counted");

        // And what is on disk is still a file.
        let bytes = std::fs::read(&p.0).unwrap();
        drop(w);
        assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(find(&chunks(&bytes), "data").2, 60);
    }

    #[test]
    fn the_warning_arrives_with_over_an_hour_of_the_ceiling_left() {
        let spec = WavSpec::default();
        // The number RECORDER-PLAN §9 quotes, checked rather than asserted.
        let hours = spec.ceiling_seconds() / 3_600.0;
        assert!(
            (hours - 4.14).abs() < 0.02,
            "4 GiB at 48k/24/stereo is 4.1 hours, got {hours:.3}"
        );
        let warn = spec.warn_seconds() / 3_600.0;
        assert!(
            (3.0..3.25).contains(&warn),
            "the warning must land just past three hours, got {warn:.3}"
        );
        assert!(
            hours - warn > 1.0,
            "warning with less than an hour left is a warning nobody can act on"
        );
    }

    #[test]
    fn capacity_and_time_remaining_track_what_has_been_written() {
        let p = scratch("capacity");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        assert_eq!(w.capacity(), Capacity::Fine);
        let before = w.seconds_remaining();
        w.write_interleaved(&[0.0; 96_000]).unwrap(); // one second, stereo
        let after = w.seconds_remaining();
        assert!(
            (before - after - 1.0).abs() < 0.01,
            "a second of audio must cost a second of headroom: {before} -> {after}"
        );
        assert_eq!(w.duration_ns(), 1_000_000_000);
        w.finish().unwrap();
    }

    #[test]
    fn a_starved_writer_shows_up_as_a_frame_deficit_against_the_timeline() {
        // The only trace a dropout leaves: the file is simply short, and every
        // event after it in a timeline is early by the missing duration.
        let p = scratch("deficit");
        let spec = WavSpec::default();
        let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
        let timeline = Timeline::synthetic(0, 1_000_000_000, 48_000.0);
        w.write_interleaved(&[0.0; 90_000]).unwrap(); // 45_000 frames of 48_000
        assert!(
            (w.deficit_frames(&timeline) - 3_000.0).abs() < 1.0,
            "3000 frames were lost and the file cannot say so any other way"
        );
        w.finish().unwrap();
    }

    #[test]
    fn a_unix_instant_becomes_the_civil_date_a_folder_name_would_use() {
        // 2026-08-15T14:32:07Z, checked against `date -u -r 1786804327`.
        let w = Wallclock::from_unix(1_786_804_327, 0);
        assert_eq!(w.date_string(), "2026-08-15");
        assert_eq!(w.time_string(), "14:32:07");
        // A leap day, because February is where a hand-rolled civil calendar
        // goes wrong, and 2100 is not a leap year even though 4 divides it.
        assert_eq!(Wallclock::from_unix(1_709_164_800, 0).date_string(), "2024-02-29");
        assert_eq!(Wallclock::from_unix(4_107_542_400, 0).date_string(), "2100-03-01");
        // Before the epoch: div_euclid floors, plain division would truncate
        // towards zero and land a day late.
        assert_eq!(Wallclock::from_unix(-1, 0).date_string(), "1969-12-31");
        assert_eq!(Wallclock::from_unix(-1, 0).time_string(), "23:59:59");
    }

    #[test]
    fn dropping_the_writer_patches_the_header_one_last_time() {
        // The writer thread panicking is a real path: a panic unwinds through
        // the session and the file must still describe every byte it holds.
        let p = scratch("drop");
        let spec = WavSpec::default();
        {
            let mut w = WavWriter::create(&p.0, spec, &Bext::new(wallclock(), spec)).unwrap();
            w.set_patch_seconds(3_600.0); // no periodic patch will fire
            w.write_interleaved(&[0.0; 512]).unwrap();
        }
        let bytes = std::fs::read(&p.0).unwrap();
        assert_eq!(find(&chunks(&bytes), "data").2, 256 * 6);
        assert_eq!(le32(&bytes, 4) as usize, bytes.len() - 8);
    }

    /// Needs a real audio device, so it is not part of `cargo test`. RECORDER-PLAN
    /// §12 step 4 drives this from a `--record-test` CLI flag inside a signed
    /// bundle, because on macOS microphone access is attributed to the
    /// responsible ancestor process and `cargo test` is not one.
    #[test]
    #[ignore = "requires an audio input device and a signed bundle"]
    fn a_real_capture_lands_in_a_file_a_daw_will_open() {}

    // ── reading ────────────────────────────────────────────────────────────

    /// Build a WAV by hand, optionally with a `JUNK` chunk in front of `fmt `.
    ///
    /// Hand-rolled rather than round-tripped through [`WavWriter`], because the
    /// whole point of these tests is files this crate did NOT write.
    fn build_wav(tag: u16, bits: u16, channels: u16, rate: u32, body: &[u8], junk: bool) -> Vec<u8> {
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&tag.to_le_bytes());
        fmt.extend_from_slice(&channels.to_le_bytes());
        fmt.extend_from_slice(&rate.to_le_bytes());
        let block_align = channels * bits / 8;
        fmt.extend_from_slice(&(rate * u32::from(block_align)).to_le_bytes());
        fmt.extend_from_slice(&block_align.to_le_bytes());
        fmt.extend_from_slice(&bits.to_le_bytes());

        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&0u32.to_le_bytes()); // patched below
        out.extend_from_slice(b"WAVE");
        if junk {
            out.extend_from_slice(b"JUNK");
            out.extend_from_slice(&28u32.to_le_bytes());
            out.extend_from_slice(&[0u8; 28]);
        }
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        out.extend_from_slice(&fmt);
        out.extend_from_slice(b"data");
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
        let riff = (out.len() - 8) as u32;
        out[4..8].copy_from_slice(&riff.to_le_bytes());
        out
    }

    #[test]
    fn a_junk_chunk_before_fmt_does_not_become_the_format_block() {
        // This is the shape of `assets/click.wav` and of anything Pro Tools
        // ever wrote. A reader that assumes fmt-then-data reads 28 zero bytes
        // as the format and then divides by a zero channel count.
        let body: Vec<u8> = (0..8u8).flat_map(|i| [i, 0]).collect();
        let bytes = build_wav(WAVE_FORMAT_PCM, 16, 1, 48_000, &body, true);
        let (spec, samples) = read_pcm(&bytes).expect("a JUNK chunk is not an error");
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.format, SampleFormat::Int16);
        assert_eq!(samples.len(), 8);
    }

    #[test]
    fn an_odd_length_chunk_is_followed_by_a_pad_byte_that_is_not_in_its_size() {
        // A 3-byte JUNK chunk. Skip the pad and every id afterwards is read one
        // byte early, so `fmt ` is never found.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0, 0, 0, 0]); // three bytes plus the pad
        let tail = build_wav(WAVE_FORMAT_PCM, 16, 2, 44_100, &[0, 0, 0, 0], false);
        bytes.extend_from_slice(&tail[12..]);
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());

        let (spec, samples) = read_pcm(&bytes).expect("the pad byte must be skipped");
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate, 44_100);
        assert_eq!(samples.len(), 2);
    }

    #[test]
    fn twenty_four_bit_samples_are_sign_extended_rather_than_read_as_large_positives() {
        // -1, -8388608 (full-scale negative) and +8388607, little end first.
        let body = [0xFF, 0xFF, 0xFF, 0x00, 0x00, 0x80, 0xFF, 0xFF, 0x7F];
        let bytes = build_wav(WAVE_FORMAT_PCM, 24, 1, 48_000, &body, false);
        let (spec, s) = read_pcm(&bytes).unwrap();
        assert_eq!(spec.format, SampleFormat::Int24);
        assert!(s[0] < 0.0, "-1 read as {} — the shift is missing", s[0]);
        assert!((s[1] + 1.0).abs() < 1e-6, "full-scale negative read as {}", s[1]);
        assert!((s[2] - 1.0).abs() < 1e-4, "full-scale positive read as {}", s[2]);
    }

    #[test]
    fn every_supported_word_size_lands_at_the_same_amplitude() {
        // Half scale in each format. They must agree, or a click read from a
        // 16-bit file is 48 dB quieter than the same click read from 24-bit.
        let i16b = build_wav(WAVE_FORMAT_PCM, 16, 1, 48_000, &16_384i16.to_le_bytes(), false);
        let i24b = build_wav(WAVE_FORMAT_PCM, 24, 1, 48_000, &[0x00, 0x00, 0x40], false);
        let i32b = build_wav(WAVE_FORMAT_PCM, 32, 1, 48_000, &1_073_741_824i32.to_le_bytes(), false);
        let f32b = build_wav(WAVE_FORMAT_IEEE_FLOAT, 32, 1, 48_000, &0.5f32.to_le_bytes(), false);
        for bytes in [i16b, i24b, i32b, f32b] {
            let (spec, s) = read_pcm(&bytes).unwrap();
            assert!(
                (s[0] - 0.5).abs() < 1e-4,
                "{:?} read half scale as {}",
                spec.format,
                s[0]
            );
        }
    }

    #[test]
    fn an_extensible_format_tag_is_followed_into_its_subformat_guid() {
        // Anything above 16 bits out of a modern DAW is likely to be tagged
        // 0xFFFE, and rejecting that rejects ordinary files.
        let mut fmt = Vec::new();
        fmt.extend_from_slice(&WAVE_FORMAT_EXTENSIBLE.to_le_bytes());
        fmt.extend_from_slice(&1u16.to_le_bytes());
        fmt.extend_from_slice(&48_000u32.to_le_bytes());
        fmt.extend_from_slice(&144_000u32.to_le_bytes());
        fmt.extend_from_slice(&3u16.to_le_bytes());
        fmt.extend_from_slice(&24u16.to_le_bytes());
        fmt.extend_from_slice(&22u16.to_le_bytes()); // cbSize
        fmt.extend_from_slice(&24u16.to_le_bytes()); // valid bits
        fmt.extend_from_slice(&4u32.to_le_bytes()); // channel mask
        fmt.extend_from_slice(&WAVE_FORMAT_PCM.to_le_bytes()); // the GUID's head
        fmt.extend_from_slice(&[0u8; 14]);

        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&(fmt.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&fmt);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&3u32.to_le_bytes());
        bytes.extend_from_slice(&[0x00, 0x00, 0x40]);
        let riff = (bytes.len() - 8) as u32;
        bytes[4..8].copy_from_slice(&riff.to_le_bytes());

        let (spec, s) = read_pcm(&bytes).unwrap();
        assert_eq!(spec.format, SampleFormat::Int24);
        assert!((s[0] - 0.5).abs() < 1e-4);
    }

    #[test]
    fn a_data_chunk_that_claims_more_than_it_holds_yields_what_is_really_there() {
        // A recorder killed mid-take leaves exactly this, and so does a
        // streaming writer that never patched its header.
        let mut bytes = build_wav(WAVE_FORMAT_PCM, 16, 2, 48_000, &[0u8; 8], false);
        let n = bytes.len();
        bytes[n - 12..n - 8].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        let (_, s) = read_pcm(&bytes).expect("a truncated take is readable, not an error");
        assert_eq!(s.len(), 4, "read past the end of the buffer");
    }

    #[test]
    fn a_data_chunk_ending_mid_frame_drops_the_fragment_rather_than_swapping_the_channels() {
        // Three samples in a stereo file. Keeping the odd one puts the right
        // channel on the left meter for everything appended afterwards.
        let bytes = build_wav(WAVE_FORMAT_PCM, 16, 2, 48_000, &[1, 0, 2, 0, 3, 0], false);
        let (_, s) = read_pcm(&bytes).unwrap();
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_file_that_is_not_a_wav_is_refused_by_name_rather_than_indexed_into() {
        assert!(read_pcm(&[]).is_err());
        assert!(read_pcm(b"RIF").is_err());
        assert!(read_pcm(&[0u8; 64]).unwrap_err().contains("not a RIFF"));
        let mut rf64 = vec![0u8; 64];
        rf64[0..4].copy_from_slice(b"RF64");
        assert!(read_pcm(&rf64).unwrap_err().contains("RF64"));
        let mut avi = vec![0u8; 64];
        avi[0..4].copy_from_slice(b"RIFF");
        avi[8..12].copy_from_slice(b"AVI ");
        assert!(read_pcm(&avi).unwrap_err().contains("not WAVE"));
        // Eight-bit is unsigned, so silence is 0x80 and reading it as signed is
        // a full-scale DC offset. Refused by name.
        let eight = build_wav(WAVE_FORMAT_PCM, 8, 1, 48_000, &[0x80, 0x80], false);
        assert!(read_pcm(&eight).unwrap_err().contains("unsigned"));
    }

    #[test]
    fn a_chunk_size_that_would_wrap_the_cursor_ends_the_walk_instead_of_looping() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 16]);
        // No fmt is reachable past a chunk that claims the rest of the address
        // space, and the loop must terminate rather than hang the app.
        assert!(read_pcm(&bytes).is_err());
    }

    #[test]
    fn the_metronome_asset_decodes_to_half_a_second_of_mono_forty_eight_kilohertz_audio() {
        // The real file, compiled in. Its measured shape (RECORDER-PLAN §4a's
        // monitor path): mono, 48 kHz, 24-bit, 0.53 s, with a JUNK chunk in
        // front of `fmt `. If someone replaces the asset with a stereo or
        // 44.1 kHz file this test says so before the metronome does.
        let (spec, samples) = read_pcm(include_bytes!("../../assets/click.wav"))
            .expect("assets/click.wav must decode");
        assert_eq!(spec.channels, 1, "the click is mixed into every output channel");
        assert_eq!(spec.sample_rate, 48_000);
        assert_eq!(spec.format, SampleFormat::Int24);
        let seconds = samples.len() as f32 / spec.sample_rate as f32;
        assert!(
            (0.4..0.7).contains(&seconds),
            "the click is {seconds:.3} s, which is not a click"
        );
        let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.1, "the click decoded to near-silence (peak {peak})");
        assert!(
            samples.iter().all(|s| s.is_finite()),
            "a non-finite sample in the click would poison every block it is mixed into"
        );
    }
}
