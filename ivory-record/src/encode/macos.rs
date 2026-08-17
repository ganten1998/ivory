//! The macOS encoder: `AVAssetWriter` → H.264 through VideoToolbox.
//!
//! One writer, one video input, one pixel-buffer adaptor. Frames arrive as
//! tightly-packed BGRA and are copied into a `CVPixelBuffer` from the adaptor's
//! own POOL — allocating one per frame is 250 MB a second of malloc at 1080p30,
//! and the pool exists precisely so nobody does that.
//!
//! # The rules this file lives by
//!
//! **Nothing here may panic.** It runs on the writer thread, and the same rule
//! that governs `camera/macos.rs` applies for the same reason: unwinding a Rust
//! panic through an Objective-C frame is undefined behaviour rather than a
//! crash report. Every fallible step returns `Err`.
//!
//! **`isReadyForMoreMediaData` is backpressure and must be obeyed — but not
//! instantly.** Appending when the input is not ready is how an encoder grows
//! an unbounded queue and the app runs out of memory twenty minutes into a
//! take. Dropping the moment it says "not ready" is the opposite mistake, and
//! it is the one this file made first: VideoToolbox goes not-ready constantly
//! and very briefly, so an instant drop threw away 83 of 90 frames and turned
//! a three-second clip into 0.23 seconds. A frame now waits up to two frame
//! intervals and is dropped only if the encoder is still behind — long enough
//! for every ordinary hiccup, far too short to be a queue in disguise.
//!
//! **Presentation times are the only timing.** The `fps` in the spec goes into
//! the track's nominal rate and nowhere else. A camera delivering 29.97, or
//! dropping to 15 in low light, still lands correctly because each frame says
//! when it is.

use crate::clock::Nanos;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_av_foundation::{
    AVAssetWriter, AVAssetWriterInput, AVAssetWriterInputPixelBufferAdaptor, AVFileTypeMPEG4, AVMediaTypeVideo, AVVideoCodecKey, AVVideoCodecTypeH264, AVVideoHeightKey,
    AVVideoWidthKey,
};
use objc2_core_video::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
    CVPixelBufferPool, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// The timescale every presentation time is expressed in.
///
/// Nanoseconds, so that a `Nanos` from the take clock converts with no division
/// and no rounding at all. A `CMTime` holds an `i64` value, so at this scale it
/// runs for 292 years before overflowing — the alternative, 600 or 90000, would
/// mean rounding every single frame's timestamp to something the clock did not
/// say.
const TIMESCALE: i32 = 1_000_000_000;

pub struct Encoder {
    writer: Retained<AVAssetWriter>,
    input: Retained<AVAssetWriterInput>,
    adaptor: Retained<AVAssetWriterInputPixelBufferAdaptor>,
    width: usize,
    height: usize,
    /// Nominal rate, kept only to size the backpressure wait.
    fps: u32,
    /// The last presentation time appended, so a frame that goes backwards can
    /// be spotted. `None` until the first frame lands.
    last_pts: Option<Nanos>,
    out_of_order: u64,
    dropped_not_ready: u64,
    frames: u64,
}

impl Encoder {
    pub fn create(path: &std::path::Path, spec: super::VideoSpec) -> Result<Self, String> {
        // AVAssetWriter REFUSES to write where a file already exists, and says
        // so with an error that reads like a permissions problem. The take
        // directory is fresh, but a retried export into the same folder is not,
        // and "cannot write" is a bad way to learn that.
        let _ = std::fs::remove_file(path);

        let url = unsafe { NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy())) };
        let file_type = unsafe { AVFileTypeMPEG4 }.ok_or("AVFileTypeMPEG4 is missing")?;
        let writer = unsafe { AVAssetWriter::assetWriterWithURL_fileType_error(&url, file_type) }
            .map_err(|e| format!("could not create the video file: {e}"))?;

        let settings = video_settings(spec)?;
        let media_type = unsafe { AVMediaTypeVideo }.ok_or("AVMediaTypeVideo is missing")?;
        let input = unsafe {
            AVAssetWriterInput::assetWriterInputWithMediaType_outputSettings(
                media_type,
                Some(&settings),
            )
        };
        // The take is running while this encodes, so the writer is told the
        // data is arriving in real time. Without it AVFoundation is free to
        // assume a file-to-file transcode and buffer far more aggressively than
        // a live capture can afford.
        unsafe { input.setExpectsMediaDataInRealTime(true) };

        let attributes = pixel_attributes(spec)?;
        let adaptor = unsafe {
            AVAssetWriterInputPixelBufferAdaptor::
                assetWriterInputPixelBufferAdaptorWithAssetWriterInput_sourcePixelBufferAttributes(
                    &input,
                    Some(&attributes),
                )
        };

        if !unsafe { writer.canAddInput(&input) } {
            return Err("the video encoder refused its own settings".to_owned());
        }
        unsafe { writer.addInput(&input) };

        if !unsafe { writer.startWriting() } {
            return Err(status_error(&writer, "could not start writing"));
        }
        // Session zero, because presentation times are already relative to the
        // first frame of the take. See `Encoder::push`.
        unsafe { writer.startSessionAtSourceTime(cm_time(0)) };

        Ok(Self {
            writer,
            input,
            adaptor,
            width: spec.width as usize,
            height: spec.height as usize,
            fps: spec.fps,
            last_pts: None,
            out_of_order: 0,
            dropped_not_ready: 0,
            frames: 0,
        })
    }

    pub fn push(&mut self, bgra: &[u8], pts_ns: Nanos) -> Result<(), String> {
        let want = self.width * self.height * 4;
        if bgra.len() < want {
            return Err(format!(
                "a frame of {} bytes is short of the {want} a {}x{} BGRA frame needs",
                bgra.len(),
                self.width,
                self.height
            ));
        }
        // Backwards, or a repeat. Dropped and counted rather than refused:
        // cameras do deliver the occasional out-of-order timestamp and ending a
        // take over one frame would be worse than the fault it reports.
        if self.last_pts.is_some_and(|last| pts_ns <= last) {
            self.out_of_order += 1;
            return Ok(());
        }
        // Backpressure: WAITED ON briefly, then obeyed.
        //
        // The first version dropped the instant the input said "not ready", and
        // that threw away 83 frames out of 90 in the encoder's own test — a
        // three-second clip came out 0.23 seconds long. VideoToolbox goes
        // not-ready constantly and for a very short time; treating that as a
        // dropped frame is treating normal operation as an emergency.
        //
        // So: wait up to two frame intervals, which is long enough to cover
        // every ordinary hiccup and short enough that it cannot become an
        // unbounded queue in disguise. A machine that is genuinely too slow
        // still drops rather than growing memory, which is the property the
        // original check was there to protect.
        if !self.wait_until_ready() {
            self.dropped_not_ready += 1;
            return Ok(());
        }

        let pool = unsafe { self.adaptor.pixelBufferPool() }
            .ok_or("the encoder has no pixel buffer pool")?;
        let buffer = pool_buffer(&pool)?;

        // Copy in, ROW BY ROW. The pool's rows are padded to whatever alignment
        // VideoToolbox wants — 1920 BGRA is 7680 bytes and the pool hands back
        // a stride of 7680 today, but on other widths and other machines it
        // does not, and a straight `copy_from_slice` of the whole plane would
        // shear the picture diagonally.
        unsafe {
            let rc = CVPixelBufferLockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
            if rc != 0 {
                return Err(format!("could not lock a pixel buffer ({rc})"));
            }
            let base = CVPixelBufferGetBaseAddress(&buffer).cast::<u8>();
            if base.is_null() {
                CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
                return Err("a pixel buffer had no memory".to_owned());
            }
            let stride = CVPixelBufferGetBytesPerRow(&buffer);
            let src_stride = self.width * 4;
            for y in 0..self.height {
                std::ptr::copy_nonoverlapping(
                    bgra.as_ptr().add(y * src_stride),
                    base.add(y * stride),
                    src_stride,
                );
            }
            CVPixelBufferUnlockBaseAddress(&buffer, CVPixelBufferLockFlags::empty());
        }

        let ok = unsafe {
            self.adaptor
                .appendPixelBuffer_withPresentationTime(&buffer, cm_time(pts_ns))
        };
        if !ok {
            return Err(status_error(&self.writer, "a frame could not be written"));
        }
        self.last_pts = Some(pts_ns);
        self.frames += 1;
        Ok(())
    }

    /// Spin briefly for the encoder to accept more, and say whether it did.
    ///
    /// A sleep and not a busy loop: this runs on the writer thread, and burning
    /// a core to shave a millisecond off a video frame would steal time from
    /// the take's own audio, which is the thing that must not glitch.
    fn wait_until_ready(&self) -> bool {
        let budget = std::time::Duration::from_nanos(2_000_000_000 / u64::from(self.fps.max(1)));
        let deadline = std::time::Instant::now() + budget;
        loop {
            if unsafe { self.input.isReadyForMoreMediaData() } {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_micros(250));
        }
    }

    pub fn out_of_order(&self) -> u64 {
        self.out_of_order
    }

    pub fn dropped_not_ready(&self) -> u64 {
        self.dropped_not_ready
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    pub fn finish(self) -> Result<(), String> {
        unsafe { self.input.markAsFinished() };
        // The BLOCKING finish, deliberately. The asynchronous one would need a
        // block and a completion handler, and the caller is the writer thread
        // at Stop — which has nothing else to do and must not return until the
        // file has its index, or the take is unplayable.
        if !unsafe { self.writer.finishWriting() } {
            return Err(status_error(&self.writer, "the video file could not be closed"));
        }
        Ok(())
    }
}

/// `{AVVideoCodecKey: h264, AVVideoWidthKey: w, AVVideoHeightKey: h}`.
fn video_settings(spec: super::VideoSpec) -> Result<Retained<NSDictionary<NSString, AnyObject>>, String> {
    let codec_key = unsafe { AVVideoCodecKey }.ok_or("AVVideoCodecKey is missing")?;
    let width_key = unsafe { AVVideoWidthKey }.ok_or("AVVideoWidthKey is missing")?;
    let height_key = unsafe { AVVideoHeightKey }.ok_or("AVVideoHeightKey is missing")?;
    let h264 = unsafe { AVVideoCodecTypeH264 }.ok_or("AVVideoCodecTypeH264 is missing")?;
    let w = NSNumber::new_u32(spec.width);
    let h = NSNumber::new_u32(spec.height);
    // H.264 and not HEVC: it plays everywhere, including in the browser and in
    // every editor a pianist is likely to open the file in. HEVC would halve
    // the file and cost the user an afternoon finding out why Premiere will not
    // import it.
    Ok(unsafe {
        NSDictionary::from_slices::<NSString>(
            &[codec_key, width_key, height_key],
            &[
                &*(h264 as &NSString as &AnyObject as *const AnyObject as *const AnyObject),
                &*(&*w as &AnyObject as *const AnyObject),
                &*(&*h as &AnyObject as *const AnyObject),
            ],
        )
    })
}

/// What the adaptor's pool should allocate: BGRA at the frame's own size.
fn pixel_attributes(
    spec: super::VideoSpec,
) -> Result<Retained<NSDictionary<NSString, AnyObject>>, String> {
    let fmt_key = unsafe { kCVPixelBufferPixelFormatTypeKey };
    let w_key = unsafe { kCVPixelBufferWidthKey };
    let h_key = unsafe { kCVPixelBufferHeightKey };
    let fmt = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
    let w = NSNumber::new_u32(spec.width);
    let h = NSNumber::new_u32(spec.height);
    Ok(unsafe {
        NSDictionary::from_slices::<NSString>(
            &[
                &*(fmt_key as *const _ as *const NSString),
                &*(w_key as *const _ as *const NSString),
                &*(h_key as *const _ as *const NSString),
            ],
            &[
                &*(&*fmt as &AnyObject as *const AnyObject),
                &*(&*w as &AnyObject as *const AnyObject),
                &*(&*h as &AnyObject as *const AnyObject),
            ],
        )
    })
}

fn pool_buffer(pool: &CVPixelBufferPool) -> Result<Retained<CVPixelBuffer>, String> {
    let mut out: *mut CVPixelBuffer = std::ptr::null_mut();
    let rc = unsafe {
        CVPixelBufferPool::create_pixel_buffer(
            None,
            pool,
            std::ptr::NonNull::from(&mut out),
        )
    };
    if rc != 0 || out.is_null() {
        return Err(format!("the pixel buffer pool is empty ({rc})"));
    }
    // `create_pixel_buffer` follows the CREATE rule, so the reference is
    // already owned and must not be retained again.
    unsafe { Retained::from_raw(out) }.ok_or_else(|| "a pixel buffer vanished".to_owned())
}

fn cm_time(ns: Nanos) -> objc2_core_media::CMTime {
    objc2_core_media::CMTime {
        value: ns,
        timescale: TIMESCALE,
        flags: objc2_core_media::CMTimeFlags::Valid,
        epoch: 0,
    }
}

/// The writer's own error, if it has one, rather than a generic failure.
///
/// Worth the code: `startWriting` returning false with no explanation is the
/// single most common way this API wastes an afternoon, and the real reason is
/// always sitting in `writer.error`.
fn status_error(writer: &AVAssetWriter, what: &str) -> String {
    let status = unsafe { writer.status() };
    match unsafe { writer.error() } {
        Some(e) => format!("{what}: {e}"),
        None => format!("{what} (status {})", status.0),
    }
}

pub fn mux(_m: &super::Mux) -> Result<(), String> {
    Err("muxing is not written yet".to_owned())
}
