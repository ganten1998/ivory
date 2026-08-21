//! Camera capture: enumeration, the frame stream, and the slot the preview reads.
//!
//! This is `audio.rs`'s opposite number, and it is deliberately shaped like it —
//! same error style, same "pure policy split out from the platform call" split,
//! same insistence that the only place `std::time::Instant` is read is
//! [`Timebase`]. Read `docs/RECORDER-PLAN.md` §3a and §4's Camera subsection
//! before changing anything here; both are the design authority and both name
//! failures that are invisible until somebody watches the export.
//!
//! # The one thing that must not drift: the timebase
//!
//! Every frame carries [`Frame::host_ns`], a reading of the **same**
//! [`Timebase`] the audio callback reads, taken as the first statement of the
//! delivery callback. That is the entire audio/video sync contract. A camera
//! stamped against its own epoch, or against a second `Instant::now()` epoch
//! created for the camera, cannot be lined up with the WAV at all — not
//! approximately, not with a slider, not ever — because nothing in the take
//! records how far apart the two epochs were.
//!
//! [`Frame::pts_ns`] carries AVFoundation's own presentation stamp beside it.
//! It is not used yet: it is the input a [`SourceClock`](crate::clock::SourceClock)
//! needs to anchor and rate-fit the camera the way [`ClockTap`](crate::audio::ClockTap)
//! does for audio, and capturing it now costs nothing while adding it later
//! would mean re-touching the callback.
//!
//! # Newest-wins, which is the opposite of what the audio ring does
//!
//! Audio uses a four-second `rtrb` ring because **every sample matters**: a
//! dropped one is a hole in the file. Video preview is the exact opposite. A
//! preview that queues frames it cannot draw does not fall behind by a constant;
//! it falls behind by a *growing* amount, and the user watches their hands lag
//! further and further behind the picture until the machine catches up. So
//! [`FrameSlot`] holds exactly one frame and a new one overwrites it, read or
//! not. Frames lost that way are counted ([`CameraStats::frames_superseded`])
//! rather than hidden, because "the preview is dropping half the frames" is a
//! thing a user should be able to find out.
//!
//! This is a preview path, not the recording path. When the encoder lands it
//! gets its own queue with the audio ring's discipline, not this one.
//!
//! # Stride, which is the bug this whole module is arranged to avoid
//!
//! `CVPixelBufferGetBytesPerRow` is **not** `width * 4`. CoreVideo pads rows to
//! a hardware-friendly alignment, so a 1918-pixel-wide capture routinely arrives
//! with a 7680-byte stride. Reading it as `width * 4` shifts every row by a
//! constant and produces a picture that shears diagonally — the classic
//! symptom, and precisely nokhwa's macOS corruption bug, which RECORDER-PLAN §4
//! calls out by name. It matters twice over downstream: `egui_glow` sets
//! `UNPACK_ALIGNMENT 1` and never sets `UNPACK_ROW_LENGTH`, so a strided buffer
//! handed to a texture upload shears there too.
//!
//! [`bgra_to_rgba`] therefore takes `stride` as a separate argument from
//! `width`, always emits a tightly packed `width * 4` result, and has a test
//! with padded rows that fails loudly if anyone "simplifies" it.
//!
//! # What the camera's latency is: nothing, and that is on purpose
//!
//! [`CameraStream::latency_ns`] is **zero and is a placeholder**. §3a puts a UVC
//! webcam's sensor → ISP → USB path at **20-150 ms**, AVFoundation exposes no
//! property that reports it, and inventing a plausible-looking constant would be
//! worse than zero because zero is visibly wrong and 60 ms is invisibly wrong.
//! [`CameraStream::latency_source`] says [`LatencySource::AssumedZero`] until
//! [`CameraStream::set_latency_ns`] is called with a figure from the §3a(2)
//! sharp-attack calibration, at which point it says
//! [`LatencySource::Measured`]. `take.json` must report whichever it is.
//!
//! This is the term that matters most in this feature and the one nothing
//! compensates: §6 composites the camera and the MIDI-driven key display into
//! **one frame**, so uncompensated, the keys light before the filmed fingers
//! land. The eye compares two things inside one image, which is a far lower
//! detection threshold than lip-sync.
//!
//! # Platforms
//!
//! macOS (AVFoundation) and Linux (`linuxvideo`/V4L2) are implemented, each
//! behind [`VideoSource`] and the two `backend::` entry points; everything
//! above that seam — the types, the format policy, the conversions, the slot —
//! is shared and tested. Windows (`IMFSourceReader`) is RECORDER-PLAN §4 work
//! and slots in the same way; until then it gets a stub that enumerates
//! nothing and returns [`CameraError::Unsupported`], so the crate builds
//! everywhere from day one and the UI degrades by saying so rather than by not
//! compiling.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use crate::audio::{DeviceState, Timebase};
use crate::clock::Nanos;
use crate::take::LatencySource;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as backend;
/// Hardware MJPEG decode, when the machine has it. See the module's own docs:
/// it is strictly an accelerator, absent it `zune-jpeg` runs unchanged, and
/// nothing links against `libva` — the library is opened at runtime.
#[cfg(target_os = "linux")]
pub mod vaapi;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod stub;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use stub as backend;

/// Bytes per pixel in both BGRA and RGBA. Named because a bare `4` in a stride
/// computation is indistinguishable from a bare `4` in a channel index.
pub const BYTES_PER_PIXEL: usize = 4;

/// Above this pixel count a format is treated as "bigger than we asked for".
///
/// Not a hard limit — a machine with only a 4K camera still gets a picture. It
/// is a tie-break, and it exists because a Continuity Camera happily offers
/// 3840x2160 and the honest answer for a recorder that will *encode* that
/// stream on the same machine that is running a piano synth is "no, thank you".
pub const PREFERRED_MAX_PIXELS: u32 = 1920 * 1080;

/// Frame-rate comparisons are made to this tolerance.
///
/// Webcams advertise 29.97 and 30 as different formats and some advertise only
/// the former. An exact `>= 30.0` test rejects a 29.97 camera outright and the
/// user is told their camera offers nothing usable, which is a lie.
pub const FPS_EPSILON: f64 = 0.05;

// ───────────────────────────────────────────────────────────────────────────
// Device identity, formats, and what the caller wants
// ───────────────────────────────────────────────────────────────────────────

/// One offered capture format.
///
/// `fps` is the **maximum** rate of the underlying format's frame-rate range.
/// A device format is a range, not a point, and the top of the range is the
/// only number a picker can usefully show.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Format {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

impl Format {
    pub fn pixels(&self) -> u32 {
        self.width.saturating_mul(self.height)
    }

    /// Bytes one RGBA frame of this size occupies, tightly packed.
    pub fn rgba_len(&self) -> usize {
        self.width as usize * self.height as usize * BYTES_PER_PIXEL
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{} @ {:.2} fps", self.width, self.height, self.fps)
    }
}

/// What the caller would like, all of it optional.
///
/// The same shape as [`ConfigWish`](crate::audio::ConfigWish) and for the same
/// reason: the policy that turns a wish plus a list of offers into a choice is
/// [`pick_format`], a pure function, so it can be tested without a camera.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FormatWish {
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// A **floor**, not a target: a format is a candidate if it reaches this,
    /// and among those that do, the closest one wins. Asking for 30 on a camera
    /// that offers 30 and 60 gets 30, because the extra 30 frames a second are
    /// bytes to encode rather than picture to see.
    pub fps: Option<f64>,
}

impl FormatWish {
    /// 1280x720 at 30 fps: the default a piano recorder should ask for.
    ///
    /// 720p is enough to see finger position across a keyboard at any sane
    /// playback size, and it is a quarter of 1080p's pixels to convert on every
    /// frame while a synth is running on the same machine.
    pub fn hd() -> Self {
        Self {
            width: Some(1280),
            height: Some(720),
            fps: Some(30.0),
        }
    }

    /// 1920x1080 at 30 fps, for when the export is the point.
    pub fn full_hd() -> Self {
        Self {
            width: Some(1920),
            height: Some(1080),
            fps: Some(30.0),
        }
    }
}

/// One row of the camera picker.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraInfo {
    /// AVFoundation's `uniqueID`, and **this is the settings key**, not `name`.
    ///
    /// `audio.rs` could not do this: cpal 0.16 exposes no stable device
    /// identifier at all, so [`DeviceKey`](crate::audio::DeviceKey) has to make
    /// do with name-plus-occurrence and documents what that costs. The camera
    /// side has a real UID, so it gets used, and every failure that key was
    /// working around simply does not arise here: two identical webcams have
    /// different UIDs, and a UID does not change when the OS language changes
    /// while `localizedName` — the clue is in the name — does.
    pub uid: String,
    /// For display only. Never match on it.
    pub name: String,
    /// True for the device `AVCaptureDevice.defaultDeviceWithMediaType(video)`
    /// returns. Compared **by UID**, so unlike the audio equivalent this flag
    /// is a selection and not merely a label.
    pub is_default: bool,
    /// Deduplicated: a device that offers 1920x1080 at 30 under three different
    /// native pixel encodings is one row here, because `videoSettings` forces
    /// BGRA and the three are then indistinguishable to us.
    pub formats: Vec<Format>,
}

/// Whether this process may open a camera.
///
/// RECORDER-PLAN §0's point about an empty enumeration being what a missing
/// entitlement looks like applies with more force here than for audio: on macOS
/// a denied camera does not error, it delivers **black frames forever**, and
/// "the camera is broken" is what the user reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionStatus {
    /// Never asked. Opening a camera raises the TCC prompt.
    NotDetermined,
    /// The user said no. Only System Settings can undo it; the prompt will
    /// never appear again.
    Denied,
    /// Screen Time, MDM or parental controls. The user *cannot* grant it, so
    /// telling them to open System Settings is bad advice.
    Restricted,
    Granted,
    /// The platform has no camera permission model, or no camera support.
    NotApplicable,
}

impl PermissionStatus {
    /// True when opening a camera has a chance of producing a picture. Includes
    /// [`NotDetermined`](Self::NotDetermined), because that is exactly the state
    /// in which asking is the right move.
    pub fn may_open(self) -> bool {
        matches!(self, Self::NotDetermined | Self::Granted | Self::NotApplicable)
    }
}

impl fmt::Display for PermissionStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotDetermined => "not yet requested",
            Self::Denied => "denied (System Settings > Privacy & Security > Camera)",
            Self::Restricted => "restricted by a system policy",
            Self::Granted => "granted",
            Self::NotApplicable => "not applicable on this platform",
        })
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Errors
// ───────────────────────────────────────────────────────────────────────────

/// Everything that can go wrong before frames are arriving.
///
/// Device loss *after* that is not here, for the reason
/// [`AudioError`](crate::audio::AudioError) gives: it is not an error anyone can
/// be handed, it is a state the session reacts to mid-take. See
/// [`CameraStream::state`].
#[derive(Debug)]
pub enum CameraError {
    /// Camera access is denied or restricted. **Kept separate from every other
    /// failure on purpose**: it is the only one with a fix the user can carry
    /// out, and it is the one that otherwise presents as "no cameras found".
    PermissionDenied(PermissionStatus),
    /// The platform refused to enumerate.
    Enumeration(String),
    /// No connected camera has this UID. Carries the UID rather than a name
    /// because a saved setting holds a UID and that is what needs to appear in
    /// the log.
    NotFound(String),
    /// The camera exists but offers nothing this module can use.
    NoUsableFormat { camera: String, offered: usize },
    /// `lockForConfiguration` failed, or the format could not be applied.
    /// Usually another application holds the camera.
    Configure(String),
    /// The session could not be built or started.
    Open(String),
    /// No camera backend on this platform yet.
    Unsupported,
}

impl fmt::Display for CameraError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PermissionDenied(s) => write!(f, "camera access is {s}"),
            Self::Enumeration(e) => write!(f, "could not list cameras: {e}"),
            Self::NotFound(uid) => write!(f, "camera \"{uid}\" is not connected"),
            Self::NoUsableFormat { camera, offered } => write!(
                f,
                "\"{camera}\" offers no usable video format ({offered} offered)"
            ),
            Self::Configure(e) => write!(f, "could not configure the camera: {e}"),
            Self::Open(e) => write!(f, "could not start the camera: {e}"),
            Self::Unsupported => {
                f.write_str("camera capture is not implemented on this platform yet")
            }
        }
    }
}

impl Error for CameraError {}

// ───────────────────────────────────────────────────────────────────────────
// Choosing a format — pure policy, testable with no camera attached
// ───────────────────────────────────────────────────────────────────────────

/// Ranking key for one candidate format. Bigger is better, lexicographically.
///
/// A named struct rather than a bare tuple because the *order of the fields is
/// the policy*, and a tuple makes reordering them look like a formatting change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct FormatRank {
    /// Reaches the wanted frame rate (within [`FPS_EPSILON`]).
    ///
    /// **First, ahead of the size**, exactly as
    /// [`pick_input_config`](crate::audio::pick_input_config) puts the sample
    /// rate ahead of the channel count. It only ever decides anything when the
    /// wanted size cannot reach the wanted rate, and in that case the answer is
    /// a different size: a 15 fps recording of hands on a keyboard is juddery
    /// past use, while 1080p instead of 720p is invisible after the export
    /// downscales it.
    fps_hit: bool,
    /// Every dimension the caller named matches exactly.
    size_hit: bool,
    /// At or under [`PREFERRED_MAX_PIXELS`].
    within_cap: bool,
    /// Under the cap, larger wins; over the cap, smaller wins. Only ever
    /// compared against another candidate in the same `within_cap` class, which
    /// is what lets one field carry both senses.
    area_pref: i64,
    /// When the rate was reached: closeness to what was asked (negated excess).
    /// Otherwise: the rate itself, so an unreachable wish still takes the
    /// fastest thing on offer. Again, only ever compared within one `fps_hit`
    /// class.
    fps_pref: i64,
}

fn rank_format(f: &Format, wish: &FormatWish) -> FormatRank {
    let size_hit = match (wish.width, wish.height) {
        // Nothing was asked for, so nothing can hit; the lower tie-breaks decide.
        (None, None) => false,
        (w, h) => w.is_none_or(|x| x == f.width) && h.is_none_or(|y| y == f.height),
    };
    let fps_hit = wish.fps.is_some_and(|want| f.fps + FPS_EPSILON >= want);
    let within_cap = f.pixels() <= PREFERRED_MAX_PIXELS;
    let area = i64::from(f.pixels());
    // Milli-fps because `f64` is not `Ord` and a NaN in a sort key is a panic
    // waiting for the one camera that reports a broken frame-rate range.
    let millifps = (f.fps * 1000.0) as i64;
    let fps_pref = match wish.fps {
        Some(want) if fps_hit => -(((f.fps - want) * 1000.0) as i64),
        _ => millifps,
    };
    FormatRank {
        fps_hit,
        size_hit,
        within_cap,
        area_pref: if within_cap { area } else { -area },
        fps_pref,
    }
}

/// Pick the best offered format, as an **index** into `offered`.
///
/// An index rather than the [`Format`] itself because the caller has to get back
/// to the platform object that produced it — on macOS an `AVCaptureDeviceFormat`
/// plus the `AVFrameRateRange` whose `minFrameDuration` locks the rate — and
/// matching a returned `Format` back by value would tie on a device that offers
/// the same geometry twice.
///
/// Priority, in order, mirroring [`pick_input_config`](crate::audio::pick_input_config):
/// the frame rate is reached; then every named dimension matches; then the size
/// is inside [`PREFERRED_MAX_PIXELS`]; then the largest size inside that cap (or
/// the smallest outside it); then the frame rate closest to what was asked.
///
/// `None` only when `offered` is empty.
pub fn pick_format(offered: &[Format], wish: &FormatWish) -> Option<usize> {
    let mut best: Option<(FormatRank, usize)> = None;
    for (index, format) in offered.iter().enumerate() {
        let rank = rank_format(format, wish);
        if best.is_none_or(|(current, _)| rank > current) {
            best = Some((rank, index));
        }
    }
    best.map(|(_, index)| index)
}

/// Find a camera by UID.
///
/// A one-liner, and it is a named function anyway because it is the *entire*
/// answer to the failure `audio.rs` spends three paragraphs apologising for.
/// Two identical webcams share a `name` and differ in `uid`; a saved setting
/// therefore keeps pointing at the same physical camera after the other one is
/// unplugged, which [`DeviceKey`](crate::audio::DeviceKey) cannot promise.
pub fn select_by_uid<'a>(cameras: &'a [CameraInfo], uid: &str) -> Option<&'a CameraInfo> {
    cameras.iter().find(|c| c.uid == uid)
}

/// The camera to use when nothing is saved: the system default, else the first.
pub fn default_camera(cameras: &[CameraInfo]) -> Option<&CameraInfo> {
    cameras
        .iter()
        .find(|c| c.is_default)
        .or_else(|| cameras.first())
}

// ───────────────────────────────────────────────────────────────────────────
// BGRA → RGBA
// ───────────────────────────────────────────────────────────────────────────

/// Convert one BGRA8 frame to tightly packed RGBA8.
///
/// AVFoundation is asked for `kCVPixelFormatType_32BGRA` because that makes its
/// own converter do the YUV→BGRA step inside the capture pipeline and there is
/// no planar handling anywhere in this crate (RECORDER-PLAN §4). egui wants
/// RGBA. So exactly one byte swap stands between them, and this is it.
///
/// **`stride` is the source's `bytesPerRow` and is routinely larger than
/// `width * BYTES_PER_PIXEL`.** The output never is: `dst` comes back with
/// exactly `width * height * 4` bytes and no padding, which is what a texture
/// upload with `UNPACK_ALIGNMENT 1` and no `UNPACK_ROW_LENGTH` requires.
///
/// Returns `false` — leaving `dst` **untouched**, so a reused scratch buffer
/// survives — when `stride` is narrower than a row or `src` is too short to
/// hold `height` rows. It is a `bool` rather than a panic because this runs on
/// AVFoundation's dispatch queue: unwinding a Rust panic through an Objective-C
/// frame is undefined behaviour, not a crash report, so the callback counts the
/// failure ([`CameraStats::frames_unreadable`]) and drops the frame.
///
/// A zero-width or zero-height frame converts successfully to nothing. That is
/// not a degenerate case worth an error: a camera that has been started but has
/// not focused yet can legitimately deliver one.
pub fn bgra_to_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut Vec<u8>,
) -> bool {
    let row_bytes = width as usize * BYTES_PER_PIXEL;
    let rows = height as usize;

    if width == 0 || rows == 0 {
        dst.clear();
        return true;
    }
    if stride < row_bytes {
        return false;
    }
    // The last row only needs `row_bytes`, not a full `stride` — CoreVideo
    // allocates `stride * height` in practice, but requiring that would reject a
    // valid tightly-packed final row from some other backend for no reason.
    let needed = stride * (rows - 1) + row_bytes;
    if src.len() < needed {
        return false;
    }

    // Not `clear()` first: resizing an already-correct buffer to its own length
    // is free, and every byte written below is overwritten anyway, so a reused
    // scratch buffer costs no allocation and no memset in the steady state.
    dst.resize(row_bytes * rows, 0);

    for y in 0..rows {
        let s = &src[y * stride..y * stride + row_bytes];
        let d = &mut dst[y * row_bytes..(y + 1) * row_bytes];
        for (px_in, px_out) in s
            .chunks_exact(BYTES_PER_PIXEL)
            .zip(d.chunks_exact_mut(BYTES_PER_PIXEL))
        {
            // Memory order is B,G,R,A in and R,G,B,A out: swap 0 and 2, leave
            // alpha alone. AVFoundation sets alpha to 0xFF for an opaque camera
            // frame, so it is passed through rather than forced — a device that
            // ever does deliver transparency should show it, not be lied about.
            px_out[0] = px_in[2];
            px_out[1] = px_in[1];
            px_out[2] = px_in[0];
            px_out[3] = px_in[3];
        }
    }
    true
}

// ───────────────────────────────────────────────────────────────────────────
// YUYV → RGBA
// ───────────────────────────────────────────────────────────────────────────

/// Convert one YUYV (YUY2, 4:2:2) frame to tightly packed RGBA8.
///
/// The Linux path's [`bgra_to_rgba`]: V4L2 webcams deliver their uncompressed
/// formats almost universally as YUYV, and this is the one byte-reshuffle that
/// stands between that and egui. The contract is the same as `bgra_to_rgba`'s,
/// on purpose — `stride` is the driver's `bytes_per_line` and may exceed
/// `width * 2`, the output is always tight `width * 4`, and a bad
/// stride/length combination returns `false` leaving `dst` untouched, because
/// this too runs on a capture thread where a panic is not an option.
///
/// The matrix is BT.601 limited range, which is what UVC hardware emits.
/// Fixed-point (×256) integer math, saturated to 0..=255: a fast path that is
/// exact enough for a preview and an encode alike, and it cannot NaN.
///
/// `width` must be even — YUYV carries two pixels per four bytes, and every
/// V4L2 discrete frame size is. An odd width is refused rather than rounded so
/// a driver bug is counted ([`CameraStats::frames_unreadable`]) instead of
/// silently cropped.
pub fn yuyv_to_rgba(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    dst: &mut Vec<u8>,
) -> bool {
    let rows = height as usize;
    let row_bytes_in = width as usize * 2;
    let row_bytes_out = width as usize * BYTES_PER_PIXEL;

    if width == 0 || rows == 0 {
        dst.clear();
        return true;
    }
    if width % 2 != 0 || stride < row_bytes_in {
        return false;
    }
    let needed = stride * (rows - 1) + row_bytes_in;
    if src.len() < needed {
        return false;
    }

    dst.resize(row_bytes_out * rows, 0);

    for y in 0..rows {
        let s = &src[y * stride..y * stride + row_bytes_in];
        let d = &mut dst[y * row_bytes_out..(y + 1) * row_bytes_out];
        for (px_in, px_out) in s.chunks_exact(4).zip(d.chunks_exact_mut(8)) {
            // Y0 U Y1 V — two pixels sharing one chroma sample.
            let u = i32::from(px_in[1]) - 128;
            let v = i32::from(px_in[3]) - 128;
            // BT.601 limited range, ×256: 1.164, 1.596, 0.813, 0.391, 2.018.
            let r_off = 409 * v;
            let g_off = -100 * u - 208 * v;
            let b_off = 516 * u;
            for (i, &y_raw) in [px_in[0], px_in[2]].iter().enumerate() {
                let yy = 298 * (i32::from(y_raw) - 16);
                let at = i * 4;
                px_out[at] = ((yy + r_off + 128) >> 8).clamp(0, 255) as u8;
                px_out[at + 1] = ((yy + g_off + 128) >> 8).clamp(0, 255) as u8;
                px_out[at + 2] = ((yy + b_off + 128) >> 8).clamp(0, 255) as u8;
                px_out[at + 3] = 0xFF;
            }
        }
    }
    true
}

// ───────────────────────────────────────────────────────────────────────────
// Frames and the newest-wins slot
// ───────────────────────────────────────────────────────────────────────────

/// One converted frame, RGBA8, tightly packed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// Bytes per row of [`pixels`](Self::pixels). Always `width * 4` for a frame
    /// this module produced — [`bgra_to_rgba`] guarantees it and a test asserts
    /// it — and it is in the struct anyway because RECORDER-PLAN §4 puts stride
    /// in the `VideoSource` trait, so that every consumer is written against it
    /// from the first day rather than the day a backend stops copying.
    pub stride: usize,
    pub pixels: Vec<u8>,
    /// [`Timebase`] nanoseconds, read as the first statement of the delivery
    /// callback. **The audio/video sync contract lives in this field.**
    pub host_ns: Nanos,
    /// The source's own presentation stamp, in nanoseconds, when it had a valid
    /// one. On macOS this is `CMSampleBufferGetPresentationTimeStamp`, which
    /// AVFoundation places on `CMClockGetHostTimeClock()` — the same domain as
    /// `Instant`, though not the same origin (clock.rs's opening section).
    ///
    /// Nothing consumes it yet. It is here because it is the stamp a
    /// [`SourceClock`](crate::clock::SourceClock) anchors on, and because
    /// retrofitting it would mean re-opening the one callback in this crate
    /// that cannot be unit-tested.
    pub pts_ns: Option<Nanos>,
}

impl Frame {
    /// A frame with no pixels, for tests and for a fake source.
    pub fn empty(host_ns: Nanos) -> Self {
        Self {
            width: 0,
            height: 0,
            stride: 0,
            pixels: Vec::new(),
            host_ns,
            pts_ns: None,
        }
    }
}

#[derive(Debug, Default)]
struct SlotInner {
    newest: Option<Frame>,
    /// The pixel buffer of whichever frame was displaced last, handed back to
    /// the producer so the steady state allocates nothing.
    spare: Vec<u8>,
}

/// The single-slot, newest-wins handoff from the capture callback to the UI.
///
/// **Not a queue, and the difference is the whole point.** See the module docs:
/// a preview that queues frames it cannot draw accumulates a delay that grows
/// without bound, which is far worse than dropping frames.
///
/// One producer only. The macOS backend guarantees that by handing AVFoundation
/// a *serial* dispatch queue; a concurrent queue would let two callbacks
/// interleave [`take_spare`](Self::take_spare) and [`publish`](Self::publish)
/// and lose the buffer recycling (it would still be safe, just pointlessly
/// allocating).
///
/// The `Mutex` is held only across a `Vec` move, never across the conversion, so
/// the capture callback is never blocked on the UI thread's repaint.
#[derive(Debug)]
pub struct FrameSlot {
    inner: Mutex<SlotInner>,
    stats: Arc<CameraStats>,
    /// Who is looking, which is what decides whether a frame is worth paying
    /// for. A [`FrameWant`] as a `u8`.
    ///
    /// **The conversion is the cost, not the capture.** A dequeue is a handoff
    /// of an mmap'd pointer; turning MJPEG into RGBA is a JPEG decode of a
    /// 720p image, and the capture thread was doing one thirty times a second
    /// for as long as the band was open — 35.6% of a core on a 2013 MacBook
    /// Air, measured, with no take rolling and the camera pane hidden.
    want: AtomicU8,
    /// What one conversion costs on THIS machine, in nanoseconds, smoothed.
    ///
    /// **Measured, because the first attempt at this guessed.** A preview was
    /// pinned to ten a second on every host — a number picked from that
    /// MacBook Air. But the cost it was paying for is a JPEG decode, and only
    /// the V4L2 path does one: on macOS the "conversion" is a BGRA-to-RGBA
    /// copy that costs a fraction of a millisecond, so the cap bought nothing
    /// and spent two thirds of the preview's smoothness on it. The owner's
    /// report was "camera in general is low framerate — very low", and it was
    /// right.
    ///
    /// Zero means nothing has been timed yet, and the first frame is taken.
    convert_ns: AtomicU64,
    /// Timebase reading of the last frame that WAS converted.
    converted_at_ns: AtomicU64,
}

/// How badly the frames are wanted.
///
/// Three states rather than a raw interval, because the interval a PREVIEW
/// should use is not the host's business — it depends on what a conversion
/// costs here, which only the thread doing them can know.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum FrameWant {
    /// A camera that is open because opening takes seconds, with nothing on
    /// screen showing it. No conversions at all.
    None = 0,
    /// Somebody is watching a preview box. As smooth as this machine can
    /// afford — see [`FrameSlot::affordable_interval`].
    Preview = 1,
    /// A take with video. Every frame, whatever it costs: a dropped frame here
    /// is a hole in a file somebody cannot re-record.
    Every = 2,
}

/// The share of one core a PREVIEW may spend converting frames.
///
/// **Scaled by how many cores there are, floored at what the potato was
/// already doing.** One thread at a third of a core is nothing on an
/// eight-core desktop and a third of the whole machine on a two-core laptop,
/// so a single percentage cannot be right for both.
///
/// The floor is where the number comes from and it is not arbitrary: the 2013
/// MacBook Air that started all this spent 35.6% of a core decoding thirty
/// frames a second, so one decode there costs about 11.9 ms — and 11.9 ms at
/// twelve percent is a 99 ms interval, which is the ten a second that machine
/// was hard-coded to. So the smallest machines keep exactly the behaviour that
/// was measured for them, and every larger one is allowed to be smooth.
///
/// The ceiling is a refusal to let a preview eat a third of a core no matter
/// how many there are; past that the honest answer is a smaller preview, not
/// more CPU.
fn preview_budget_percent() -> u64 {
    static ANSWER: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *ANSWER.get_or_init(|| {
        let cores = std::thread::available_parallelism().map_or(2, |n| n.get()) as u64;
        (4 * cores).clamp(12, 33)
    })
}

/// The slowest a preview is ever allowed to get.
///
/// A guard on the arithmetic above, not a policy: a machine where one
/// conversion costs a tenth of a second would otherwise be asked for one frame
/// every second, and a preview that updates once a second is not a preview.
/// Better to blow the budget than to show a slideshow.
const PREVIEW_SLOWEST_NS: u64 = 200_000_000;

impl FrameSlot {
    pub fn new(stats: Arc<CameraStats>) -> Self {
        Self {
            inner: Mutex::new(SlotInner::default()),
            stats,
            // Every frame until somebody says otherwise, which is what every
            // build before this one did.
            want: AtomicU8::new(FrameWant::Every as u8),
            convert_ns: AtomicU64::new(0),
            converted_at_ns: AtomicU64::new(Self::NEVER),
        }
    }

    /// No frame has been converted yet. See [`should_convert`](Self::should_convert).
    const NEVER: u64 = u64::MAX;

    /// Say who is looking. See [`FrameWant`].
    pub fn want(&self, want: FrameWant) {
        self.want.store(want as u8, Ordering::Relaxed);
    }

    /// What one conversion just cost, for the budget to spend.
    ///
    /// **Slow up, fast down, and the asymmetry is the point.** A symmetric
    /// average treats "this frame was slow" and "this frame was quick" as
    /// equally good news, which gets the risk backwards: a preview that dips
    /// for a moment because something else on the machine woke up costs
    /// nothing, and a preview STUCK slow long after the machine went quiet is
    /// the bug. So a rise moves an eighth of the way — a scheduler hiccup
    /// barely registers, a machine that genuinely got busier is followed
    /// within a second — and a fall moves half, which is back to full rate in
    /// a handful of frames.
    pub fn note_convert_cost(&self, ns: u64) {
        let old = self.convert_ns.load(Ordering::Relaxed);
        let next = match old {
            0 => ns,
            old if ns > old => old + (ns - old) / 8,
            old => old - (old - ns) / 2,
        };
        self.convert_ns.store(next, Ordering::Relaxed);
    }

    /// How far apart a preview's frames have to be for the conversions to fit
    /// in [`PREVIEW_BUDGET_PERCENT`] of one core.
    ///
    /// Zero — every frame — until something has been timed, so a preview comes
    /// up at full rate and slows down if it turns out it has to, rather than
    /// starting slow and never finding out that it need not be.
    fn affordable_interval(&self) -> u64 {
        let cost = self.convert_ns.load(Ordering::Relaxed);
        if cost == 0 {
            return 0;
        }
        cost.saturating_mul(100)
            .saturating_div(preview_budget_percent())
            .min(PREVIEW_SLOWEST_NS)
    }

    /// **Should the capture thread pay for this frame?**
    ///
    /// Called with the frame's own timebase stamp, before the conversion and
    /// after the dequeue — the queue has to keep moving whatever the answer is,
    /// or the driver backs up and starts reporting errors.
    ///
    /// Records the time on a yes, so the caller cannot forget to. That is the
    /// whole reason this takes `&self` and is not a pure function: the two
    /// halves being separable is how a decimator ends up letting every frame
    /// through on some path nobody tested.
    pub fn should_convert(&self, host_ns: Nanos) -> bool {
        let every = match self.want.load(Ordering::Relaxed) {
            x if x == FrameWant::None as u8 => return false,
            x if x == FrameWant::Every as u8 => 0,
            // A preview: whatever this machine can afford.
            _ => self.affordable_interval(),
        };
        let now = host_ns.max(0) as u64;
        let last = self.converted_at_ns.load(Ordering::Relaxed);
        // **The first frame of a stream always converts.** `NEVER` rather than
        // zero, because a zero would mean "converted at time zero" — and a
        // timebase whose stamps start near zero would then skip its own first
        // frames, so a preview came up blank for a moment on one platform and
        // not the other. It cost a test to notice.
        //
        // A stamp that went BACKWARDS also converts, once, rather than locking
        // the camera out until the clock catches up. It should not happen; if
        // it does, the failure would be a preview that silently stopped.
        // **A frame that is nearly due counts as due**, and the slack is not a
        // fudge. Frames arrive on the camera's own grid — 33.3 ms at 30 fps —
        // so a strict "at least 100 ms since the last one" is never satisfied
        // by the frame at 99.999 ms and waits for the one at 133, which turns a
        // request for ten a second into seven and a half. What is being asked
        // for is a RATE, not a minimum spacing.
        let due = every.saturating_sub(every / 8);
        if every > 0 && last != Self::NEVER && now >= last && now - last < due {
            return false;
        }
        self.converted_at_ns.store(now, Ordering::Relaxed);
        true
    }

    /// Borrow the recycled pixel buffer to convert into.
    ///
    /// Returns an empty `Vec` the first time and after any poisoning; callers
    /// must treat the contents as garbage and the capacity as a gift.
    pub fn take_spare(&self) -> Vec<u8> {
        match self.inner.lock() {
            Ok(mut g) => std::mem::take(&mut g.spare),
            Err(_) => Vec::new(),
        }
    }

    /// Hand a finished frame's buffer back to be filled again.
    ///
    /// **The steady state used to be the losing one.** `spare` was refilled
    /// only when `publish` DISPLACED a frame — that is, only while the preview
    /// was dropping frames — so a camera keeping up allocated a fresh
    /// full-size buffer for every single frame, and one falling behind did
    /// not. That is exactly the wrong way round.
    ///
    /// The consumer owns the pixels once it has taken them, so the consumer is
    /// what gives them back. Whichever buffer is larger is kept: the capture
    /// resizes what it is handed, and starting from the bigger one is one
    /// fewer growth.
    pub fn recycle(&self, pixels: Vec<u8>) {
        if pixels.capacity() == 0 {
            return;
        }
        if let Ok(mut g) = self.inner.lock() {
            if pixels.capacity() > g.spare.capacity() {
                g.spare = pixels;
            }
        }
    }

    /// Install a frame, displacing whatever was there.
    ///
    /// The displaced frame's buffer becomes the next [`take_spare`](Self::take_spare).
    pub fn publish(&self, frame: Frame) {
        let Ok(mut g) = self.inner.lock() else {
            // A poisoned slot means the UI thread panicked mid-read. Dropping
            // the frame is right: there is nobody left to draw it, and the
            // capture callback must not panic.
            self.stats.frames_superseded.fetch_add(1, Ordering::Relaxed);
            return;
        };
        if let Some(old) = g.newest.replace(frame) {
            self.stats.frames_superseded.fetch_add(1, Ordering::Relaxed);
            g.spare = old.pixels;
        }
    }

    /// Take the newest frame, if one has arrived since the last call.
    ///
    /// **Takes rather than clones.** A clone would memcpy 8 MB per repaint at
    /// 1080p for a picture that has not changed, and `None` is exactly the
    /// signal a texture uploader wants: nothing new, keep the texture you have.
    pub fn latest(&self) -> Option<Frame> {
        self.inner.lock().ok().and_then(|mut g| g.newest.take())
    }

    /// Whether a frame is waiting, without consuming it.
    pub fn has_frame(&self) -> bool {
        self.inner.lock().is_ok_and(|g| g.newest.is_some())
    }
}

/// A `Send` handle onto the frames of a [`CameraStream`].
///
/// [`CameraStream`] itself is `!Send` because it owns platform objects (see its
/// docs). The frames are not, so a worker thread that wants them gets this.
#[derive(Debug, Clone)]
pub struct FrameReader {
    slot: Arc<FrameSlot>,
}

impl FrameReader {
    /// Newest-wins, exactly as [`FrameSlot::latest`].
    pub fn latest(&self) -> Option<Frame> {
        self.slot.latest()
    }

    /// See [`FrameSlot::recycle`].
    pub fn recycle(&self, pixels: Vec<u8>) {
        self.slot.recycle(pixels);
    }

    pub fn has_frame(&self) -> bool {
        self.slot.has_frame()
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Counters
// ───────────────────────────────────────────────────────────────────────────

/// Counters the delivery callback writes and everyone else reads.
///
/// All `Relaxed`, for [`CaptureStats`](crate::audio::CaptureStats)'s reason:
/// these are statistics, not synchronisation. The frames themselves travel
/// through [`FrameSlot`]'s `Mutex`, which does its own acquire/release.
#[derive(Debug, Default)]
pub struct CameraStats {
    frames_delivered: AtomicU64,
    frames_superseded: AtomicU64,
    frames_dropped_late: AtomicU64,
    frames_unreadable: AtomicU64,
    /// Frames the capture thread deliberately did not convert. See
    /// [`FrameSlot::want`].
    frames_skipped: AtomicU64,
    device_state: AtomicU8,
    /// Monotonic nanoseconds at the first and most recent delivery, for
    /// [`delivered_fps`](Self::delivered_fps). Zero until a frame arrives.
    ///
    /// Two stamps rather than a start time and a count, because the answer
    /// wanted is the rate *while running* — a camera opened long before the
    /// first frame would otherwise read as slow for the rest of the session.
    first_delivery_ns: AtomicU64,
    last_delivery_ns: AtomicU64,
}

/// Monotonic nanoseconds from a process-wide origin.
///
/// Not [`Timebase`], which belongs to a take: this outlives takes and is only
/// ever used for differences.
fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// A camera delivering materially fewer frames than the format it agreed to.
///
/// **This is a real condition with a real fix, and it is invisible today.** A
/// UVC webcam integrates for as long as the light needs and cannot produce
/// frames faster than that, so in a dim room it silently halves its rate:
/// measured on a 2012 FaceTime HD, `fps = min(30, 1/exposure)` exactly —
/// 66.6 ms of exposure gives 15 fps, 30 ms gives 30. Nothing in the app says
/// so, and the frames that never arrive are indistinguishable from frames the
/// app lost, which sends anyone looking in exactly the wrong place.
///
/// The fix belongs to the user — more light, or a shorter exposure and a
/// darker picture — which is precisely why it is worth saying out loud.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RateLimited {
    /// What the driver agreed to when the format was negotiated.
    pub negotiated_fps: f64,
    /// What is actually arriving.
    pub actual_fps: f64,
}

impl RateLimited {
    /// Delivered rate as a fraction of the negotiated one.
    pub fn ratio(&self) -> f64 {
        if self.negotiated_fps <= 0.0 {
            return 1.0;
        }
        self.actual_fps / self.negotiated_fps
    }
}

impl CameraStats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Frames converted and published. The rate this divided by elapsed time
    /// gives is the *real* frame rate, which is frequently not the one the
    /// format advertises — a webcam in a dim room silently halves it.
    pub fn frames_delivered(&self) -> u64 {
        self.frames_delivered.load(Ordering::Relaxed)
    }

    /// Frames overwritten in the slot before anyone read them: the price of
    /// newest-wins, and the number that says the preview cannot keep up.
    pub fn frames_superseded(&self) -> u64 {
        self.frames_superseded.load(Ordering::Relaxed)
    }

    /// Frames the platform dropped before we saw them — on macOS,
    /// `captureOutput:didDropSampleBuffer:`. Distinct from
    /// [`frames_superseded`](Self::frames_superseded) because this one means
    /// *our callback* was too slow and held the capture pool hostage, which is a
    /// different bug with a different fix.
    pub fn frames_dropped_late(&self) -> u64 {
        self.frames_dropped_late.load(Ordering::Relaxed)
    }

    /// Frames that arrived but could not be converted: no image buffer, a pixel
    /// format that was not BGRA despite `videoSettings` asking for it, an
    /// unlockable base address, or a stride/size combination that failed
    /// [`bgra_to_rgba`]'s bounds check. Non-zero here means the picture is
    /// wrong, not merely late.
    pub fn frames_unreadable(&self) -> u64 {
        self.frames_unreadable.load(Ordering::Relaxed)
    }

    pub fn device_state(&self) -> DeviceState {
        state_from_u8(self.device_state.load(Ordering::Relaxed))
    }

    pub fn set_device_state(&self, state: DeviceState) {
        self.device_state.store(state_as_u8(state), Ordering::Relaxed);
    }

    pub fn note_delivered(&self) {
        let n = self.frames_delivered.fetch_add(1, Ordering::Relaxed);
        // Two relaxed stores on the capture thread, which is already doing
        // atomics here; the reader tolerates a torn pair by returning `None`.
        let now = monotonic_ns();
        if n == 0 {
            self.first_delivery_ns.store(now, Ordering::Relaxed);
        }
        self.last_delivery_ns.store(now, Ordering::Relaxed);
    }

    /// The rate the camera is **actually** delivering, or `None` before there
    /// is enough to say.
    ///
    /// Measured between the first and most recent delivery, so it is the rate
    /// while running rather than an average that includes the time before the
    /// stream produced anything.
    pub fn delivered_fps(&self) -> Option<f64> {
        /// Enough arrivals that a slow start cannot masquerade as a slow
        /// camera. At 15 fps this is a couple of seconds.
        const ENOUGH: u64 = 30;
        let n = self.frames_delivered();
        if n < ENOUGH {
            return None;
        }
        let first = self.first_delivery_ns.load(Ordering::Relaxed);
        let last = self.last_delivery_ns.load(Ordering::Relaxed);
        let span = last.checked_sub(first).filter(|s| *s > 0)?;
        // n frames span n-1 intervals.
        Some((n - 1) as f64 * 1_000_000_000.0 / span as f64)
    }

    pub fn note_dropped_late(&self) {
        self.frames_dropped_late.fetch_add(1, Ordering::Relaxed);
    }

    pub fn note_unreadable(&self) {
        self.frames_unreadable.fetch_add(1, Ordering::Relaxed);
    }

    /// A frame that arrived and was deliberately not converted.
    ///
    /// Counted separately from every other loss, because it is not one: these
    /// are frames nobody asked for. A skip landing in `frames_dropped_late`
    /// would make an idle camera look like a failing one in `take.json`.
    pub fn note_skipped(&self) {
        self.frames_skipped.fetch_add(1, Ordering::Relaxed);
    }

    pub fn frames_skipped(&self) -> u64 {
        self.frames_skipped.load(Ordering::Relaxed)
    }

    /// Clear the counters for a new take. Never during one.
    pub fn reset(&self) {
        self.frames_delivered.store(0, Ordering::Relaxed);
        self.frames_superseded.store(0, Ordering::Relaxed);
        self.frames_dropped_late.store(0, Ordering::Relaxed);
        self.frames_unreadable.store(0, Ordering::Relaxed);
    }
}

/// `DeviceState` is `audio.rs`'s, deliberately: a session that has to react to
/// "the interface vanished" and "the camera vanished" should not be matching on
/// two enums with the same three variants. Its `as_u8`/`from_u8` are private to
/// that module, so the atomic encoding is restated here rather than by widening
/// `audio.rs`'s API for one caller.
fn state_as_u8(state: DeviceState) -> u8 {
    match state {
        DeviceState::Running => 0,
        DeviceState::Lost => 1,
        DeviceState::Errored => 2,
    }
}

fn state_from_u8(v: u8) -> DeviceState {
    match v {
        1 => DeviceState::Lost,
        2 => DeviceState::Errored,
        _ => DeviceState::Running,
    }
}

// ───────────────────────────────────────────────────────────────────────────
// The backend seam
// ───────────────────────────────────────────────────────────────────────────

/// The per-platform half of a running camera: RECORDER-PLAN §4's `VideoSource`.
///
/// Everything above this line — the types, [`pick_format`], [`bgra_to_rgba`],
/// [`FrameSlot`] — is shared and already tested, so adding Windows or Linux is
/// this trait plus two `backend::` functions and nothing else.
///
/// Frames do not travel through the trait; they go straight into the
/// [`FrameSlot`] the backend was handed. That keeps the delivery path free of a
/// virtual call per frame and, more usefully, means a backend can deliver from
/// whatever thread its platform insists on without the trait having to describe
/// it.
///
/// Dropping the implementor stops the camera.
trait VideoSource {
    /// The format the device was **actually** opened with, which is not always
    /// the one that was asked for.
    fn format(&self) -> Format;

    /// Polled, not pushed. macOS answers it from `AVCaptureSession.isRunning`
    /// and `AVCaptureDevice.isConnected` rather than by subscribing to
    /// `AVCaptureSessionRuntimeErrorNotification`, because a poll the UI already
    /// makes every repaint is cheaper and less to go wrong than a notification
    /// observer whose lifetime has to be managed by hand.
    fn state(&self) -> DeviceState;
}

// ───────────────────────────────────────────────────────────────────────────
// Public entry points
// ───────────────────────────────────────────────────────────────────────────

/// This process's camera permission.
///
/// Never blocks and never prompts. The prompt happens inside [`open_camera`],
/// which is the one moment the user has just asked for a camera and a dialog is
/// not a non-sequitur.
pub fn permission_status() -> PermissionStatus {
    backend::permission_status()
}

/// Every camera the system can see, in the platform's own order.
///
/// Not cached, for [`input_devices`](crate::audio::input_devices)'s reason: a
/// camera plugged in while the Recorder band is open must appear without a
/// restart.
///
/// Returns [`CameraError::PermissionDenied`] rather than an empty list when
/// access is denied, because an empty list is indistinguishable from "this
/// machine has no camera" and sends the user looking for the wrong problem.
pub fn cameras() -> Result<Vec<CameraInfo>, CameraError> {
    backend::cameras()
}

/// Open a camera by UID and start it.
///
/// Per RECORDER-PLAN §3 this is called when the Recorder band *opens*, not when
/// Record is pressed: `AVCaptureSession.startRunning` is **300-800 ms on a
/// built-in camera and can exceed 2 s for a Continuity Camera**, and that is
/// warm-up the take must not contain.
///
/// **Measured, on the machine this was written on**: 63 ms for the built-in
/// MacBook Pro camera, and **1.9-3.9 s for a Logitech MX Brio**. The Logitech is
/// not slow hardware; that is the price of making the requested format stick,
/// because `startRunning` re-imposes the session preset over `activeFormat` on a
/// UVC device and undoing it means reconfiguring a *running* session. The macOS
/// backend pays it only when the read-back says it has to — the built-in camera
/// honours the format first time and never gets there — but a user with an
/// external webcam waits those seconds, once, when the Recorder band opens.
///
/// **It blocks for that whole time, on the calling thread.** Do not call it from
/// a repaint. Moving it off-thread is real future work rather than an oversight:
/// `AVCaptureSession` is documented thread-safe, so the start could be
/// dispatched, but the returned [`CameraStream`] is `!Send` and the drop-order
/// against a queued start needs designing rather than asserting.
///
/// If permission is [`PermissionStatus::NotDetermined`], AVFoundation raises the
/// TCC prompt during this call and returns immediately; frames are black until
/// the user answers. That is AVFoundation's behaviour, not a policy chosen here,
/// and it is why [`CameraStats::frames_delivered`] climbing while the picture
/// stays black is a permission symptom rather than a driver one.
pub fn open_camera(
    uid: &str,
    wish: &FormatWish,
    timebase: Timebase,
) -> Result<CameraStream, CameraError> {
    let stats = Arc::new(CameraStats::new());
    let slot = Arc::new(FrameSlot::new(Arc::clone(&stats)));
    let source = backend::open(
        uid,
        wish,
        timebase,
        Arc::clone(&slot),
        Arc::clone(&stats),
    )?;
    Ok(CameraStream {
        source,
        slot,
        stats,
        latency_ns: AtomicI64::new(0),
        latency_measured: AtomicU8::new(0),
    })
}

/// A running camera.
///
/// **`!Send`, and that is the platform's doing rather than ours** — the same
/// situation [`InputStream`](crate::audio::InputStream) documents. The macOS
/// backend holds `Retained<AVCaptureSession>` and friends, which objc2 does not
/// mark `Send`. Build it, hold it and drop it on one thread, in practice the UI
/// thread; use [`reader`](Self::reader) to get frames to a worker.
///
/// Dropping this stops the camera.
pub struct CameraStream {
    source: Box<dyn VideoSource>,
    slot: Arc<FrameSlot>,
    stats: Arc<CameraStats>,
    latency_ns: AtomicI64,
    /// 0 = assumed, 1 = measured. An atomic beside the value rather than a
    /// `Mutex<(Nanos, LatencySource)>` because both are read on the repaint path
    /// and neither is ever read as a pair that has to be consistent.
    latency_measured: AtomicU8,
}

impl fmt::Debug for CameraStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CameraStream")
            .field("format", &self.source.format())
            .field("state", &self.source.state())
            .field("latency_ns", &self.latency_ns())
            .finish()
    }
}

impl CameraStream {
    /// The newest frame, or `None` if none has arrived since the last call.
    ///
    /// Newest-wins: see [`FrameSlot`]. Three frames delivered between two calls
    /// yields the third and counts the other two as
    /// [`CameraStats::frames_superseded`].
    pub fn latest(&self) -> Option<Frame> {
        self.slot.latest()
    }

    /// See [`FrameSlot::recycle`].
    pub fn recycle(&self, pixels: Vec<u8>) {
        self.slot.recycle(pixels);
    }

    /// Tell the capture thread who is looking. See [`FrameWant`].
    pub fn want(&self, want: FrameWant) {
        self.slot.want(want);
    }

    /// A `Send` handle onto the same slot, for a thread that is not this one.
    pub fn reader(&self) -> FrameReader {
        FrameReader {
            slot: Arc::clone(&self.slot),
        }
    }

    /// What the device is doing. [`DeviceState::Lost`] carries
    /// RECORDER-PLAN §4's policy: stop the take, finalise every file that has
    /// bytes, mark it incomplete, and **never discard what was captured**.
    pub fn state(&self) -> DeviceState {
        let state = self.source.state();
        self.stats.set_device_state(state);
        state
    }

    /// The format actually opened, which may not be the one wished for.
    ///
    /// **Read back from the device, never remembered from the request.** On
    /// macOS the session preset can and does override `activeFormat`, and a
    /// reported format that disagrees with the delivered pixels breaks every
    /// downstream size assumption silently. Compare it against a [`Frame`]'s own
    /// `width`/`height` if you want to be sure; they should always agree.
    pub fn format(&self) -> Format {
        self.source.format()
    }

    /// Whether the camera is delivering materially fewer frames than the
    /// format it agreed to, and by how much.
    ///
    /// `None` means either that it is keeping up, or that too few frames have
    /// arrived to say — never "no idea". It is a statement about the *camera*,
    /// not about the app losing frames, and that distinction is the whole
    /// point. See [`RateLimited`].
    pub fn rate_limited(&self) -> Option<RateLimited> {
        /// Below this fraction of the negotiated rate, say so. A camera that
        /// halves itself lands at 0.5; 25 against 30 is 0.83 and not worth
        /// interrupting anyone about.
        const SHORTFALL: f64 = 0.8;
        let negotiated_fps = self.source.format().fps;
        if negotiated_fps <= 0.0 {
            return None;
        }
        let actual_fps = self.stats.delivered_fps()?;
        let limited = RateLimited { negotiated_fps, actual_fps };
        (limited.ratio() < SHORTFALL).then_some(limited)
    }

    pub fn stats(&self) -> &Arc<CameraStats> {
        &self.stats
    }

    /// This camera's capture latency, to be **subtracted** from every frame's
    /// timestamp per §3a's `T = a·D + b − latency`.
    ///
    /// **Zero, and zero is a placeholder, not a measurement.** No macOS API
    /// reports it. §3a puts the true figure at 20-150 ms for a UVC webcam and
    /// gives the only honest way to obtain it: the sharp-attack calibration in
    /// §3a(2), cross-correlating an audio onset against the frame with the
    /// largest inter-frame pixel delta, stored per device UID because it is a
    /// property of the camera and not of the take.
    ///
    /// Until that runs, [`latency_source`](Self::latency_source) says
    /// [`LatencySource::AssumedZero`] and `take.json` must say so too.
    pub fn latency_ns(&self) -> Nanos {
        self.latency_ns.load(Ordering::Relaxed)
    }

    /// Install a measured latency from the §3a(2) calibration.
    ///
    /// Marks the figure [`LatencySource::Measured`]. There is deliberately no
    /// way to install one and still call it assumed: a number in this field that
    /// nobody can trace is exactly the failure §3a's reporting requirement
    /// exists to prevent.
    pub fn set_latency_ns(&self, latency_ns: Nanos) {
        self.latency_ns.store(latency_ns, Ordering::Relaxed);
        self.latency_measured.store(1, Ordering::Relaxed);
    }

    /// Where [`latency_ns`](Self::latency_ns) came from, for `take.json`.
    ///
    /// Never [`LatencySource::OsReported`]: AVFoundation guarantees the
    /// timebase, not the origin, and exposes no capture-latency property at all.
    pub fn latency_source(&self) -> LatencySource {
        if self.latency_measured.load(Ordering::Relaxed) == 0 {
            LatencySource::AssumedZero
        } else {
            LatencySource::Measured
        }
    }
}

#[cfg(test)]
mod tests {

    /// **A camera that halves itself must be reported, and one that keeps up
    /// must not.**
    ///
    /// The condition is real and invisible: a UVC webcam integrates for as long
    /// as the light needs, so in a dim room it delivers half the rate it
    /// negotiated. Measured on a 2012 FaceTime HD, `fps = min(30, 1/exposure)`
    /// exactly. Today those missing frames look exactly like frames the app
    /// lost, which sends anyone looking in the wrong place.
    #[test]
    fn a_rate_limited_camera_is_distinguishable_from_a_healthy_one() {
        let stats = CameraStats::new();
        assert_eq!(stats.delivered_fps(), None, "answered before it could know");
        for _ in 0..60 {
            stats.note_delivered();
        }
        let fps = stats
            .delivered_fps()
            .expect("60 frames is enough to have an opinion");
        assert!(fps > 0.0, "a rate of {fps} is not a rate");

        let halved = RateLimited { negotiated_fps: 30.0, actual_fps: 15.0 };
        assert!((halved.ratio() - 0.5).abs() < 1e-9);
        let fine = RateLimited { negotiated_fps: 30.0, actual_fps: 29.97 };
        assert!(fine.ratio() > 0.99, "a healthy camera read as limited");
        let bad = RateLimited { negotiated_fps: 0.0, actual_fps: 0.0 };
        assert_eq!(bad.ratio(), 1.0, "an unknown rate must not read as a fault");
    }

    /// A slow start must not read as a slow camera.
    #[test]
    fn the_rate_needs_enough_frames_before_it_will_answer() {
        let stats = CameraStats::new();
        for _ in 0..29 {
            stats.note_delivered();
        }
        assert_eq!(stats.delivered_fps(), None, "answered on 29 frames");
        stats.note_delivered();
        assert!(stats.delivered_fps().is_some(), "still silent at 30 frames");
    }

    /// **The camera pays for a frame only when somebody is looking.**
    ///
    /// The conversion is a 720p JPEG decode, and the capture thread was doing
    /// thirty a second for as long as the band was open — 35.6% of a core on a
    /// 2013 MacBook Air, measured with the pane hidden and no take rolling, on
    /// the same machine that then dropped over half the frames of an actual
    /// take. The dequeue still happens either way; the queue has to keep moving
    /// or the driver backs up.
    #[test]
    fn frames_are_converted_at_the_rate_somebody_asked_for() {
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));

        // The default, and what every build before this one did: all of them.
        let ns = 1_000_000_000 / 30;
        for i in 0..30 {
            assert!(
                slot.should_convert(i * ns),
                "frame {i} was skipped with no rate asked for"
            );
        }

        // None at all, which is a camera that is open because opening takes
        // seconds, with nothing on screen showing it.
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        slot.want(FrameWant::None);
        assert!(!(0..30).any(|i| slot.should_convert(i * ns)));
    }

    /// **What a preview costs is measured, not assumed — and this is the test
    /// that says the two machines get different answers from one rule.**
    ///
    /// The version before this one asked every host for ten frames a second,
    /// from a number measured on a 2013 MacBook Air's JPEG decode. Only the
    /// V4L2 path decodes JPEG; on macOS a conversion is a BGRA-to-RGBA copy
    /// costing half a millisecond, so the cap threw away two thirds of the
    /// preview for nothing. The owner's report was "camera in general is low
    /// framerate — very low".
    #[test]
    fn a_preview_is_as_smooth_as_the_machine_can_afford() {
        let ns = 1_000_000_000 / 30;

        // A machine where a conversion is half a millisecond: every frame.
        let quick = FrameSlot::new(Arc::new(CameraStats::new()));
        quick.want(FrameWant::Preview);
        for i in 0..30 {
            quick.note_convert_cost(500_000);
            assert!(
                quick.should_convert(i * ns),
                "frame {i} was skipped on a machine that could afford it"
            );
        }

        // A conversion costing 30 ms — a big frame on a slow box. That is 90%
        // of a core at thirty a second, past any budget on any machine.
        let slow = FrameSlot::new(Arc::new(CameraStats::new()));
        slow.want(FrameWant::Preview);
        for _ in 0..16 {
            slow.note_convert_cost(30_000_000);
        }
        // The rule, stated directly, so this does not depend on how many cores
        // the machine running the test happens to have.
        let want = slow.affordable_interval();
        assert_eq!(
            want,
            (30_000_000 * 100 / preview_budget_percent()).min(PREVIEW_SLOWEST_NS)
        );
        assert!(want > ns as u64, "a 30 ms conversion was called affordable at 30 fps");

        // Some frames, and never all of them.
        let taken = (0..30)
            .filter(|i| {
                let yes = slow.should_convert(i * ns);
                if yes {
                    slow.note_convert_cost(30_000_000);
                }
                yes
            })
            .count();
        assert!(
            (1..30).contains(&taken),
            "a 30 ms conversion took {taken} frames a second out of 30"
        );

        // The first frame is always taken, whatever the budget: a preview that
        // came up blank while it worked out how fast it could go would be a
        // camera that looks broken for its first second.
        let fresh = FrameSlot::new(Arc::new(CameraStats::new()));
        fresh.want(FrameWant::Preview);
        assert!(fresh.should_convert(0), "the first frame was skipped");

        // And a take is not subject to any of it. A dropped frame there is a
        // hole in a file nobody can re-record.
        let take = FrameSlot::new(Arc::new(CameraStats::new()));
        take.want(FrameWant::Every);
        for _ in 0..16 {
            take.note_convert_cost(50_000_000);
        }
        assert!(
            (0..30).all(|i| take.should_convert(i * ns)),
            "a take was decimated by the preview's budget"
        );
    }

    /// One slow frame must not cost the preview anything it does not get back.
    ///
    /// The risk is asymmetric and so is the smoothing: a preview that dips for
    /// a moment is nothing, a preview stuck slow after the machine went quiet
    /// again is the whole bug this replaced.
    #[test]
    fn a_single_slow_frame_does_not_stick() {
        let frame_ns = 1_000_000_000 / 30;
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        for _ in 0..32 {
            slot.note_convert_cost(500_000);
        }
        let settled = slot.affordable_interval();
        assert!(settled < frame_ns, "half a millisecond could not afford 30 fps");

        // Ten times as long, once — something else on the machine woke up.
        slot.note_convert_cost(5_000_000);
        assert!(
            slot.affordable_interval() < frame_ns,
            "one slow frame dropped a preview that can afford every frame"
        );

        // And it is gone within a handful of frames, not a session.
        for _ in 0..6 {
            slot.note_convert_cost(500_000);
        }
        let back = slot.affordable_interval();
        assert!(
            back <= settled + settled / 5,
            "the preview took too long to recover: {back} against a settled {settled}"
        );
    }

    /// A timebase that goes backwards must not lock the camera out forever.
    ///
    /// It should not happen and it is not worth a panic if it does: the failure
    /// would be a preview that stopped updating and never recovered, with
    /// nothing anywhere saying why.
    #[test]
    fn a_backwards_stamp_does_not_freeze_the_preview() {
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        slot.want(FrameWant::Preview);
        for _ in 0..16 {
            slot.note_convert_cost(12_000_000);
        }
        assert!(slot.should_convert(10_000_000_000));
        // Far in the past, once.
        assert!(
            slot.should_convert(5_000_000_000),
            "a stamp that went backwards was refused"
        );
        // And the rate still holds from there.
        assert!(!slot.should_convert(5_010_000_000));
    }
    use super::*;

    const FPS: f64 = 30.0;

    fn fmt(width: u32, height: u32, fps: f64) -> Format {
        Format { width, height, fps }
    }

    /// A BGRA source buffer whose every pixel encodes its own coordinates, so a
    /// row picked up at the wrong offset is *visible* rather than merely
    /// suspected. Padding bytes are 0xEE, a value no encoded pixel can produce.
    fn padded_bgra(width: u32, height: u32, stride: usize) -> Vec<u8> {
        let mut buf = vec![0xEE; stride * height as usize];
        for y in 0..height as usize {
            for x in 0..width as usize {
                let at = y * stride + x * BYTES_PER_PIXEL;
                buf[at] = 0x10 + x as u8; // B
                buf[at + 1] = 0x20 + y as u8; // G
                buf[at + 2] = 0x30 + x as u8; // R
                buf[at + 3] = 0xFF; // A
            }
        }
        buf
    }

    fn frame_of(width: u32, height: u32, fill: u8, host_ns: Nanos) -> Frame {
        Frame {
            width,
            height,
            stride: width as usize * BYTES_PER_PIXEL,
            pixels: vec![fill; width as usize * height as usize * BYTES_PER_PIXEL],
            host_ns,
            pts_ns: None,
        }
    }

    // ── BGRA → RGBA ─────────────────────────────────────────────────────────

    #[test]
    fn a_row_stride_wider_than_the_image_does_not_shear_the_picture() {
        // 5 px wide is 20 bytes; CoreVideo would pad that to 32 or 64. This is
        // the exact shape of nokhwa's macOS corruption bug.
        let (width, height, stride) = (5u32, 4u32, 64usize);
        let src = padded_bgra(width, height, stride);
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, width, height, stride, &mut dst));

        let row_bytes = width as usize * BYTES_PER_PIXEL;
        assert_eq!(dst.len(), row_bytes * height as usize, "output must be tight");
        for y in 0..height as usize {
            for x in 0..width as usize {
                let at = y * row_bytes + x * BYTES_PER_PIXEL;
                assert_eq!(dst[at], 0x30 + x as u8, "R at ({x},{y}) came from the wrong row");
                assert_eq!(dst[at + 1], 0x20 + y as u8, "G at ({x},{y})");
                assert_eq!(dst[at + 2], 0x10 + x as u8, "B at ({x},{y})");
                assert_eq!(dst[at + 3], 0xFF, "A at ({x},{y})");
            }
        }
    }

    #[test]
    fn no_padding_byte_from_a_strided_source_reaches_the_output() {
        let (width, height, stride) = (3u32, 3u32, 40usize);
        let src = padded_bgra(width, height, stride);
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, width, height, stride, &mut dst));
        assert!(
            !dst.contains(&0xEE),
            "0xEE is the padding filler; finding one in the output means \
             stride was treated as image data"
        );
    }

    #[test]
    fn the_blue_and_red_channels_are_swapped_and_alpha_is_left_alone() {
        let src = [0x01u8, 0x02, 0x03, 0x77];
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, 1, 1, 4, &mut dst));
        assert_eq!(dst, vec![0x03, 0x02, 0x01, 0x77]);
    }

    #[test]
    fn a_one_by_one_frame_converts_to_exactly_four_bytes() {
        let src = padded_bgra(1, 1, 4);
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, 1, 1, 4, &mut dst));
        assert_eq!(dst.len(), 4);
    }

    #[test]
    fn a_zero_sized_frame_converts_to_nothing_rather_than_failing() {
        let mut dst = vec![9u8; 16];
        assert!(bgra_to_rgba(&[], 0, 0, 0, &mut dst));
        assert!(dst.is_empty());

        let mut dst = vec![9u8; 16];
        assert!(bgra_to_rgba(&[], 640, 0, 2560, &mut dst));
        assert!(dst.is_empty(), "zero height is a valid empty frame");

        let mut dst = vec![9u8; 16];
        assert!(bgra_to_rgba(&[], 0, 480, 0, &mut dst));
        assert!(dst.is_empty(), "zero width is a valid empty frame");
    }

    #[test]
    fn a_stride_narrower_than_one_row_is_refused_without_touching_the_buffer() {
        let mut dst = vec![7u8; 8];
        assert!(!bgra_to_rgba(&[0; 64], 5, 4, 16, &mut dst));
        assert_eq!(dst, vec![7u8; 8], "a refused frame must not clobber the scratch");
    }

    #[test]
    fn a_source_too_short_for_its_own_geometry_is_refused_rather_than_read_past() {
        let mut dst = Vec::new();
        // 4 rows of stride 64 needs 3*64 + 16 = 208 bytes; give it 100.
        assert!(!bgra_to_rgba(&[0; 100], 4, 4, 64, &mut dst));
    }

    #[test]
    fn the_last_row_only_needs_its_pixels_and_not_a_full_stride_of_padding() {
        // A backend that packs the final row tightly is still correct, and
        // demanding stride*height would reject it.
        let (width, height, stride) = (4u32, 3u32, 32usize);
        let row_bytes = width as usize * BYTES_PER_PIXEL;
        let src = vec![0u8; stride * (height as usize - 1) + row_bytes];
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, width, height, stride, &mut dst));
        assert_eq!(dst.len(), row_bytes * height as usize);
    }

    #[test]
    fn a_reused_scratch_buffer_is_resized_rather_than_appended_to() {
        let mut dst = vec![0xAA; 4096];
        let src = padded_bgra(2, 2, 8);
        assert!(bgra_to_rgba(&src, 2, 2, 8, &mut dst));
        assert_eq!(dst.len(), 16, "a bigger previous frame must not leave a tail");
    }

    #[test]
    fn a_converted_frames_stride_is_exactly_its_width_in_pixels() {
        // The old version of this test wrote `stride: width * 4` into the
        // `Frame` by hand and then asserted that field equalled `width * 4` —
        // a tautology about a literal it had just written, which would have
        // gone on passing if the conversion started emitting padded rows.
        //
        // What has to be proven is that the CONVERSION produces a tight buffer
        // from a padded source, so the assertion is against `dst.len()` and the
        // pixel data, and the stride is DERIVED rather than declared.
        let (width, height, stride) = (7u32, 3u32, 96usize);
        let src = padded_bgra(width, height, stride);
        let mut dst = Vec::new();
        assert!(bgra_to_rgba(&src, width, height, stride, &mut dst));

        let tight = width as usize * BYTES_PER_PIXEL;
        assert_eq!(
            dst.len(),
            tight * height as usize,
            "a padded {stride}-byte source row must convert to a tight \
             {tight}-byte one, or every consumer that trusts Frame::stride \
             draws a sheared picture"
        );
        // And row N really is row N: with a padded source, an off-by-one in the
        // row arithmetic still produces a buffer of the right LENGTH.
        for row in 0..height as usize {
            for col in 0..width as usize {
                let s = row * stride + col * BYTES_PER_PIXEL;
                let d = row * tight + col * BYTES_PER_PIXEL;
                assert_eq!(
                    (dst[d], dst[d + 1], dst[d + 2]),
                    (src[s + 2], src[s + 1], src[s]),
                    "row {row} col {col} came from the wrong place (BGRA->RGBA)"
                );
            }
        }
        let frame = Frame {
            width,
            height,
            stride: dst.len() / height as usize,
            pixels: dst,
            host_ns: 0,
            pts_ns: None,
        };
        assert_eq!(frame.stride, tight, "Frame::stride's documented claim");
        assert_eq!(frame.pixels.len(), frame.stride * frame.height as usize);
    }

    // ── YUYV → RGBA ─────────────────────────────────────────────────────────

    /// One YUYV macropixel (4 bytes, 2 pixels) with both Y samples equal.
    fn yuyv_px(y: u8, u: u8, v: u8) -> [u8; 4] {
        [y, u, y, v]
    }

    #[test]
    fn yuyv_nominal_black_white_and_grey_land_where_bt601_says() {
        // Limited-range anchors: Y=16 is black, Y=235 is white, neutral chroma.
        let mut src = Vec::new();
        src.extend_from_slice(&yuyv_px(16, 128, 128));
        src.extend_from_slice(&yuyv_px(235, 128, 128));
        let mut dst = Vec::new();
        assert!(yuyv_to_rgba(&src, 4, 1, 8, &mut dst));
        assert_eq!(&dst[0..4], &[0, 0, 0, 0xFF], "Y=16 must be full black");
        assert_eq!(
            &dst[8..12],
            &[255, 255, 255, 0xFF],
            "Y=235 must be full white"
        );
        for px in dst.chunks_exact(4) {
            assert_eq!(px[0], px[1], "neutral chroma may not tint");
            assert_eq!(px[1], px[2], "neutral chroma may not tint");
        }
    }

    #[test]
    fn yuyv_a_pure_red_sample_comes_out_red() {
        // BT.601 pure red: Y=81, U=90, V=240.
        let src = yuyv_px(81, 90, 240);
        let mut dst = Vec::new();
        assert!(yuyv_to_rgba(&src, 2, 1, 4, &mut dst));
        let (r, g, b) = (dst[0], dst[1], dst[2]);
        assert!(r >= 250, "red channel is {r}, expected ~255");
        assert!(g <= 5 && b <= 5, "green/blue are ({g},{b}), expected ~0");
    }

    #[test]
    fn yuyv_a_padded_stride_does_not_shear_and_rows_stay_rows() {
        // Row y carries Y = 20*y + 30 uniformly; if any output row's pixels
        // disagree with each other, a row was picked up at the wrong offset.
        let (width, height, stride) = (4u32, 3u32, 32usize);
        let mut src = vec![0xEE; stride * height as usize];
        for y in 0..height as usize {
            for m in 0..(width as usize / 2) {
                let at = y * stride + m * 4;
                src[at..at + 4].copy_from_slice(&yuyv_px(30 + 20 * y as u8, 128, 128));
            }
        }
        let mut dst = Vec::new();
        assert!(yuyv_to_rgba(&src, width, height, stride, &mut dst));
        let row_bytes = width as usize * BYTES_PER_PIXEL;
        assert_eq!(dst.len(), row_bytes * height as usize, "output must be tight");
        for y in 0..height as usize {
            let row = &dst[y * row_bytes..(y + 1) * row_bytes];
            let first = row[0];
            for px in row.chunks_exact(4) {
                assert_eq!(px[0], first, "row {y} mixes rows: stride was mis-read");
                assert_eq!(px[3], 0xFF);
            }
        }
        // And the rows are distinct, so "all equal" is not vacuously true.
        assert_ne!(dst[0], dst[row_bytes], "test rows must differ to prove anything");
    }

    #[test]
    fn yuyv_refuses_an_odd_width_rather_than_rounding_it() {
        let mut dst = vec![7u8; 8];
        assert!(!yuyv_to_rgba(&[0; 64], 3, 2, 8, &mut dst));
        assert_eq!(dst, vec![7u8; 8], "a refused frame must not clobber the scratch");
    }

    #[test]
    fn yuyv_refuses_a_source_too_short_for_its_own_geometry() {
        let mut dst = Vec::new();
        // 3 rows of stride 16 needs 2*16 + 8 = 40 bytes; give it 30.
        assert!(!yuyv_to_rgba(&[0; 30], 4, 3, 16, &mut dst));
    }

    #[test]
    fn yuyv_a_zero_sized_frame_converts_to_nothing_rather_than_failing() {
        let mut dst = vec![9u8; 16];
        assert!(yuyv_to_rgba(&[], 0, 0, 0, &mut dst));
        assert!(dst.is_empty());
    }

    // ── format selection ────────────────────────────────────────────────────

    #[test]
    fn an_exact_size_match_outranks_a_larger_one_that_also_reaches_the_rate() {
        let offered = [fmt(1920, 1080, FPS), fmt(1280, 720, FPS), fmt(640, 480, FPS)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(offered[pick], fmt(1280, 720, FPS));
    }

    #[test]
    fn reaching_the_requested_rate_outranks_matching_the_requested_size() {
        // The size is available only at 15 fps. A 15 fps take of a piano is
        // unwatchable, so the rate wins and the size gives way.
        let offered = [fmt(1280, 720, 15.0), fmt(1920, 1080, 30.0)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(offered[pick], fmt(1920, 1080, 30.0));
    }

    #[test]
    fn a_camera_that_only_advertises_twenty_nine_ninety_seven_still_satisfies_thirty() {
        // The trap FPS_EPSILON exists for: an exact `>= 30.0` tells the user
        // their perfectly ordinary webcam offers nothing usable.
        let offered = [fmt(1280, 720, 29.97), fmt(640, 480, 60.0)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(offered[pick], fmt(1280, 720, 29.97));
    }

    #[test]
    fn asking_for_thirty_gets_thirty_and_not_the_sixty_beside_it() {
        let offered = [fmt(1280, 720, 60.0), fmt(1280, 720, 30.0)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(
            offered[pick],
            fmt(1280, 720, 30.0),
            "fps is a floor with a closest-match tie-break, not a maximise"
        );
    }

    #[test]
    fn an_unreachable_frame_rate_falls_back_to_the_fastest_on_offer() {
        let offered = [fmt(1280, 720, 10.0), fmt(1280, 720, 24.0)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(offered[pick], fmt(1280, 720, 24.0));
    }

    #[test]
    fn a_four_k_continuity_camera_is_not_chosen_over_a_ten_eighty_p_one() {
        let offered = [fmt(3840, 2160, 30.0), fmt(1920, 1080, 30.0)];
        let pick = pick_format(&offered, &FormatWish::default()).expect("a pick");
        assert_eq!(
            offered[pick],
            fmt(1920, 1080, 30.0),
            "PREFERRED_MAX_PIXELS keeps the encoder off a 4K stream by default"
        );
    }

    #[test]
    fn a_camera_that_only_offers_four_k_still_gets_opened_at_its_smallest() {
        let offered = [fmt(7680, 4320, 30.0), fmt(3840, 2160, 30.0)];
        let pick = pick_format(&offered, &FormatWish::default()).expect("a pick");
        assert_eq!(
            offered[pick],
            fmt(3840, 2160, 30.0),
            "over the cap the preference inverts: smallest, not largest"
        );
    }

    #[test]
    fn with_no_wish_at_all_the_largest_format_under_the_cap_wins() {
        let offered = [fmt(640, 480, 30.0), fmt(1920, 1080, 30.0), fmt(320, 240, 60.0)];
        let pick = pick_format(&offered, &FormatWish::default()).expect("a pick");
        assert_eq!(offered[pick], fmt(1920, 1080, 30.0));
    }

    #[test]
    fn an_empty_offer_list_yields_no_pick_rather_than_a_bad_one() {
        assert_eq!(pick_format(&[], &FormatWish::hd()), None);
    }

    #[test]
    fn a_width_only_wish_matches_on_width_alone() {
        let offered = [fmt(1280, 960, 30.0), fmt(1920, 1080, 30.0)];
        let wish = FormatWish {
            width: Some(1280),
            height: None,
            fps: None,
        };
        let pick = pick_format(&offered, &wish).expect("a pick");
        assert_eq!(offered[pick], fmt(1280, 960, 30.0));
    }

    #[test]
    fn the_pick_is_an_index_so_two_identical_geometries_stay_distinguishable() {
        // The device offers the same 1280x720@30 twice (two native encodings
        // that both become BGRA). Returning a Format by value would make these
        // indistinguishable to a caller that has to get back to the platform
        // object behind one of them.
        let offered = [fmt(1280, 720, 30.0), fmt(1280, 720, 30.0)];
        let pick = pick_format(&offered, &FormatWish::hd()).expect("a pick");
        assert_eq!(pick, 0, "ties resolve to the first offer, deterministically");
    }

    // ── UID selection ───────────────────────────────────────────────────────

    fn two_of_the_same_webcam() -> Vec<CameraInfo> {
        // Two identical Logitech C920s. `audio.rs` cannot tell these apart and
        // says so at length; here it is one field lookup.
        vec![
            CameraInfo {
                uid: "0x1400000046d082d".into(),
                name: "HD Pro Webcam C920".into(),
                is_default: true,
                formats: vec![fmt(1280, 720, 30.0)],
            },
            CameraInfo {
                uid: "0x1410000046d082d".into(),
                name: "HD Pro Webcam C920".into(),
                is_default: false,
                formats: vec![fmt(1920, 1080, 30.0)],
            },
        ]
    }

    #[test]
    fn two_cameras_sharing_a_name_are_still_told_apart_by_uid() {
        let cams = two_of_the_same_webcam();
        let second = select_by_uid(&cams, "0x1410000046d082d").expect("the second C920");
        assert_eq!(second.formats, vec![fmt(1920, 1080, 30.0)]);
        let first = select_by_uid(&cams, "0x1400000046d082d").expect("the first C920");
        assert_eq!(first.formats, vec![fmt(1280, 720, 30.0)]);
        assert_eq!(first.name, second.name, "the names really are identical");
    }

    #[test]
    fn a_uid_that_is_no_longer_connected_selects_nothing_rather_than_a_neighbour() {
        // The failure DeviceKey cannot avoid: unplugging the first of two
        // identical devices promotes the second into its slot. A UID lookup
        // simply misses, and the caller falls back to the default explicitly.
        let cams = two_of_the_same_webcam();
        assert!(select_by_uid(&cams, "0x1420000046d082d").is_none());
    }

    #[test]
    fn the_default_camera_is_the_flagged_one_and_the_first_one_otherwise() {
        let mut cams = two_of_the_same_webcam();
        assert_eq!(default_camera(&cams).map(|c| c.uid.as_str()), Some("0x1400000046d082d"));
        cams[0].is_default = false;
        cams[1].is_default = true;
        assert_eq!(default_camera(&cams).map(|c| c.uid.as_str()), Some("0x1410000046d082d"));
        for c in &mut cams {
            c.is_default = false;
        }
        assert_eq!(default_camera(&cams).map(|c| c.uid.as_str()), Some("0x1400000046d082d"));
        assert!(default_camera(&[]).is_none());
    }

    // ── the newest-wins slot ────────────────────────────────────────────────

    #[test]
    fn three_frames_pushed_and_one_read_yields_the_third_and_not_the_first() {
        let stats = Arc::new(CameraStats::new());
        let slot = FrameSlot::new(Arc::clone(&stats));
        slot.publish(frame_of(2, 2, 1, 100));
        slot.publish(frame_of(2, 2, 2, 200));
        slot.publish(frame_of(2, 2, 3, 300));

        let got = slot.latest().expect("a frame");
        assert_eq!(got.host_ns, 300, "newest-wins, so the third frame");
        assert_eq!(got.pixels[0], 3);
        assert_eq!(
            stats.frames_superseded(),
            2,
            "the two frames nobody saw must be counted, not hidden"
        );
    }

    #[test]
    fn reading_twice_without_a_new_frame_yields_none_the_second_time() {
        // The signal a texture uploader wants: None means "nothing new, keep
        // the texture you have", which is why latest() takes rather than clones.
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        slot.publish(frame_of(1, 1, 7, 10));
        assert!(slot.latest().is_some());
        assert!(slot.latest().is_none());
    }

    #[test]
    fn a_displaced_frames_buffer_comes_back_as_the_next_scratch() {
        // This is what makes the steady state allocation-free: the capture
        // callback converts into the buffer of the frame it is about to replace.
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        assert!(slot.take_spare().is_empty(), "nothing to recycle yet");
        slot.publish(frame_of(4, 4, 1, 10));
        slot.publish(frame_of(4, 4, 2, 20));
        let spare = slot.take_spare();
        assert_eq!(
            spare.capacity(),
            4 * 4 * BYTES_PER_PIXEL,
            "the displaced frame's allocation must be handed back"
        );
        assert!(slot.take_spare().is_empty(), "and only handed back once");
    }

    #[test]
    fn a_slot_that_has_never_been_written_reports_no_frame() {
        let slot = FrameSlot::new(Arc::new(CameraStats::new()));
        assert!(!slot.has_frame());
        assert!(slot.latest().is_none());
        slot.publish(Frame::empty(0));
        assert!(slot.has_frame(), "has_frame must not consume");
        assert!(slot.has_frame());
        assert!(slot.latest().is_some());
    }

    #[test]
    fn a_frame_reader_sees_the_same_slot_as_the_stream_that_made_it() {
        let stats = Arc::new(CameraStats::new());
        let slot = Arc::new(FrameSlot::new(stats));
        let reader = FrameReader {
            slot: Arc::clone(&slot),
        };
        slot.publish(frame_of(1, 1, 5, 42));
        assert_eq!(reader.latest().map(|f| f.host_ns), Some(42));
        assert!(slot.latest().is_none(), "the reader consumed it");
    }

    #[test]
    fn a_frame_reader_is_send_so_a_worker_thread_can_hold_one() {
        fn assert_send<T: Send>() {}
        assert_send::<FrameReader>();
        assert_send::<Frame>();
        assert_send::<Arc<FrameSlot>>();
    }

    // ── counters and state ──────────────────────────────────────────────────

    #[test]
    fn the_three_ways_a_frame_can_be_lost_are_counted_separately() {
        // Superseded (the preview was slow), dropped-late (our callback was
        // slow and held the capture pool), unreadable (the picture was wrong).
        // One counter for all three would make every camera bug look the same.
        let stats = CameraStats::new();
        stats.note_dropped_late();
        stats.note_unreadable();
        stats.note_unreadable();
        assert_eq!(stats.frames_dropped_late(), 1);
        assert_eq!(stats.frames_unreadable(), 2);
        assert_eq!(stats.frames_superseded(), 0);
    }

    #[test]
    fn resetting_the_counters_does_not_clear_a_lost_device() {
        let stats = CameraStats::new();
        stats.note_delivered();
        stats.set_device_state(DeviceState::Lost);
        stats.reset();
        assert_eq!(stats.frames_delivered(), 0);
        assert_eq!(
            stats.device_state(),
            DeviceState::Lost,
            "a between-takes reset must not un-unplug the camera"
        );
    }

    #[test]
    fn every_device_state_survives_the_trip_through_the_atomic() {
        for state in [DeviceState::Running, DeviceState::Lost, DeviceState::Errored] {
            assert_eq!(state_from_u8(state_as_u8(state)), state);
        }
        assert_eq!(state_from_u8(200), DeviceState::Running, "garbage reads as running");
    }

    #[test]
    fn a_fresh_stats_block_starts_running_rather_than_errored() {
        assert_eq!(CameraStats::new().device_state(), DeviceState::Running);
    }

    // ── permission ──────────────────────────────────────────────────────────

    #[test]
    fn only_the_two_states_the_user_cannot_immediately_fix_block_an_open() {
        assert!(PermissionStatus::Granted.may_open());
        assert!(
            PermissionStatus::NotDetermined.may_open(),
            "not-determined is exactly when asking is right"
        );
        assert!(PermissionStatus::NotApplicable.may_open());
        assert!(!PermissionStatus::Denied.may_open());
        assert!(!PermissionStatus::Restricted.may_open());
    }

    #[test]
    fn denied_and_restricted_read_differently_because_the_advice_differs() {
        // Telling someone under MDM to open System Settings wastes their time.
        assert!(PermissionStatus::Denied.to_string().contains("System Settings"));
        assert!(!PermissionStatus::Restricted.to_string().contains("System Settings"));
    }

    #[test]
    fn a_permission_error_reads_as_a_permission_problem_and_names_no_device() {
        let e = CameraError::PermissionDenied(PermissionStatus::Denied);
        let text = e.to_string();
        assert!(text.contains("camera access"), "got {text:?}");
        assert!(text.contains("System Settings"), "got {text:?}");
    }

    #[test]
    fn a_missing_camera_error_names_the_uid_because_that_is_what_settings_hold() {
        let e = CameraError::NotFound("0x1400000046d082d".into());
        assert!(e.to_string().contains("0x1400000046d082d"));
    }

    // ── the latency term ────────────────────────────────────────────────────

    #[test]
    fn an_uncalibrated_camera_reports_zero_latency_and_admits_it_is_assumed() {
        // The §3a contract: the number and its provenance travel together, and
        // "assumed_zero" is what take.json must say until a calibration runs.
        assert_eq!(LatencySource::default(), LatencySource::AssumedZero);
    }

    // ── hardware ────────────────────────────────────────────────────────────

    /// Ignored: enumerates real cameras and raises a TCC prompt on macOS.
    ///
    /// Run with `cargo test -p ivory-record --ignored camera` on a machine with
    /// a camera attached, and never in CI: a build box has no camera, and the
    /// permission prompt blocks the run forever. Under a plain `cargo test`
    /// binary macOS may deny outright — real access wants the signed bundle
    /// with `com.apple.security.device.camera`, which is what
    /// `examples/lscam.rs` and the app itself provide.
    #[test]
    #[ignore = "enumerates real cameras"]
    fn every_enumerated_camera_reports_a_uid_and_at_least_one_format() {
        let cams = cameras().expect("enumeration");
        for cam in &cams {
            assert!(!cam.uid.is_empty(), "{} has no UID", cam.name);
            assert!(!cam.formats.is_empty(), "{} offers no format", cam.name);
            assert!(
                select_by_uid(&cams, &cam.uid).is_some(),
                "a UID straight out of enumeration must select"
            );
        }
        assert!(
            cams.iter().filter(|c| c.is_default).count() <= 1,
            "at most one camera can be the system default"
        );
    }
    /// **The steady state was the losing one.**
    ///
    /// `spare` was refilled only when a frame was DISPLACED — only while the
    /// preview was dropping frames — so a camera that was keeping up
    /// allocated a fresh full-size buffer for every single frame and one that
    /// was falling behind did not. The consumer owns the pixels once it has
    /// taken them, so the consumer gives them back.
    #[test]
    fn a_consumed_frame_gives_its_buffer_back() {
        let slot = FrameSlot::new(Arc::new(CameraStats::default()));
        // Nothing to recycle yet, so the first frame has to allocate.
        assert_eq!(slot.take_spare().capacity(), 0);

        let mut px = Vec::with_capacity(4096);
        px.extend(std::iter::repeat_n(7u8, 4096));
        slot.publish(Frame {
            width: 32,
            height: 32,
            stride: 128,
            pixels: px,
            host_ns: 1,
            pts_ns: None,
        });
        // The consumer takes it, uses it, and hands the buffer back.
        let got = slot.latest().expect("a frame was published");
        assert_eq!(slot.take_spare().capacity(), 0, "nothing was displaced");
        slot.recycle(got.pixels);
        assert!(
            slot.take_spare().capacity() >= 4096,
            "the buffer was not kept, so every frame allocates while the \
             camera is keeping up"
        );

        // An empty one is not worth keeping, and a smaller one does not
        // replace a bigger one: the capture resizes what it is handed.
        slot.recycle(Vec::new());
        assert_eq!(slot.take_spare().capacity(), 0);
        slot.recycle(Vec::with_capacity(8192));
        slot.recycle(Vec::with_capacity(16));
        assert!(slot.take_spare().capacity() >= 8192, "the bigger one lost");
    }


    /// Ignored: opens a real camera.
    ///
    /// See the note above. This is the test that would catch a stride bug on
    /// real hardware, and it is also the one no CI can run — which is why the
    /// stride cases above are built from synthetic padded buffers instead.
    #[test]
    #[ignore = "opens a real camera"]
    fn a_real_camera_delivers_tightly_packed_frames_in_the_host_timebase() {
        let timebase = Timebase::new();
        let cams = cameras().expect("enumeration");
        let cam = default_camera(&cams).expect("a camera");
        let stream =
            open_camera(&cam.uid, &FormatWish::hd(), timebase).expect("the default camera");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        let mut seen = 0;
        let mut last_ns: Option<Nanos> = None;
        while std::time::Instant::now() < deadline && seen < 10 {
            if let Some(frame) = stream.latest() {
                assert_eq!(
                    frame.stride,
                    frame.width as usize * BYTES_PER_PIXEL,
                    "a delivered frame must be tightly packed"
                );
                assert_eq!(frame.pixels.len(), frame.stride * frame.height as usize);
                assert!(frame.host_ns > 0, "a frame must be stamped in the timebase");
                if let Some(prev) = last_ns {
                    assert!(frame.host_ns > prev, "frame stamps must advance");
                }
                last_ns = Some(frame.host_ns);
                seen += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(seen >= 10, "only {seen} frames in three seconds");
        assert_eq!(stream.state(), DeviceState::Running);
        assert_eq!(
            stream.stats().frames_unreadable(),
            0,
            "an unreadable frame means the pixel format or the stride is wrong"
        );
    }
}
