//! The macOS camera backend: `AVCaptureSession` → `AVCaptureVideoDataOutput`.
//!
//! RECORDER-PLAN §4's shape, exactly: one session, one device input, one video
//! data output forced to `kCVPixelFormatType_32BGRA` in `videoSettings` so that
//! AVFoundation's own converter does YUV→BGRA inside the capture pipeline and
//! there is no planar handling anywhere in this crate. Delivery is a push onto a
//! serial `dispatch_queue`, and the rule there is the audio callback's rule with
//! the volume turned down: no file I/O, no locks held across work, and above all
//! **no panics**, because unwinding a Rust panic through an Objective-C frame is
//! undefined behaviour rather than a crash report.
//!
//! # The two landmines the plan told us to handle before writing this
//!
//! **1. Continuity Camera needs two things and every library gets it wrong.**
//! An iPhone used as a webcam is only discovered when
//! `NSCameraUseContinuityCameraDeviceType` is in the Info.plist *and*
//! `AVCaptureDeviceTypeContinuityCamera` is in the discovery session's device
//! type array. The plist half is `scripts/build-macos.sh`; this file is the
//! other half. Out of the box neither nokhwa nor `cameras` finds the user's
//! iPhone, and for a piano app the best camera in the room usually *is* an
//! iPhone on a mount.
//!
//! **2. The deployment-target trap, which breaks launch and not merely the
//! camera.** `LSMinimumSystemVersion` is 11.0. objc2 emits
//! `AVCaptureDeviceTypeContinuityCamera` and `AVCaptureDeviceTypeExternal` (both
//! macOS 14+, verified against the SDK's own `API_AVAILABLE` annotations) as
//! plain **non-weak** `extern "C"` statics — checked in
//! `objc2-av-foundation-0.3.2/src/generated/AVCaptureDevice.rs`, they carry no
//! weak-linking attribute of any kind. A non-weak data symbol is bound eagerly
//! by dyld, so merely *referencing* one makes the app fail to start on macOS
//! 11/12/13. So [`video_device_types`] links only the 10.15+ symbols and builds
//! the newer ones as `NSString` literals at runtime, behind an OS-version gate
//! so an unrecognised type string is never handed to AVFoundation either.
//!
//! The literal values were verified against the running system rather than
//! assumed, by compiling and running a three-line Objective-C program that
//! `NSLog`s each constant. They are equal to their own symbol names — with one
//! exception worth knowing about: on macOS 14+ the *deprecated*
//! `AVCaptureDeviceTypeExternalUnknown` symbol's **value** is the string
//! `"AVCaptureDeviceTypeExternal"`. That is why this file links that symbol
//! rather than hard-coding either spelling: the symbol exists on every macOS we
//! support and its value tracks whatever the running OS considers correct.

use std::sync::Arc;

use dispatch2::{DispatchQueue, DispatchRetained};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass};
use objc2_av_foundation::{
    AVAuthorizationStatus, AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceDiscoverySession,
    AVCaptureDeviceFormat, AVCaptureDeviceInput, AVCaptureDevicePosition, AVCaptureDeviceType,
    AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureOutput, AVCaptureSession,
    AVCaptureVideoDataOutput, AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaType,
    AVMediaTypeVideo,
};
// Deprecated since macOS 14 and imported anyway; see `video_device_types` for
// why the deprecation is the reason to use it rather than a reason not to. The
// allow has to sit on the `use` as well as on the call site, because naming a
// deprecated item is itself the warning.
#[allow(deprecated)]
use objc2_av_foundation::AVCaptureDeviceTypeExternalUnknown;
use objc2_core_media::{
    CMSampleBuffer, CMTime, CMTimeFlags, CMVideoFormatDescriptionGetDimensions,
};
use objc2_core_video::{
    kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetDataSize, CVPixelBufferGetHeight,
    CVPixelBufferGetPixelFormatType, CVPixelBufferGetWidth, CVPixelBufferIsPlanar,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{
    ns_string, NSArray, NSDictionary, NSNumber, NSObject, NSObjectProtocol, NSProcessInfo, NSString,
};

use super::{
    bgra_to_rgba, pick_format, CameraError, CameraInfo, CameraStats, Format, FormatWish, Frame,
    FrameSlot, PermissionStatus, VideoSource,
};
use crate::audio::{DeviceState, Timebase};
use crate::clock::Nanos;

/// `CVReturn` success. CoreVideo has no named constant in these bindings and a
/// bare `0` in a lock check reads like a null pointer.
const CV_RETURN_SUCCESS: i32 = 0;

/// The label the camera's serial queue carries in Instruments and in a spindump.
///
/// Worth naming rather than leaving anonymous: when a user reports a beachball,
/// "was the camera queue the one that was stuck" is the first question, and an
/// unlabelled queue answers it with a hex address.
const QUEUE_LABEL: &str = "org.ganten.tangent.camera";

// ───────────────────────────────────────────────────────────────────────────
// Permission
// ───────────────────────────────────────────────────────────────────────────

/// `AVMediaTypeVideo`, which is nullable in these bindings.
///
/// It has existed since macOS 10.7, so `None` here means the framework did not
/// load at all — in which case every call below would fail anyway, and the
/// callers turn it into an enumeration error rather than unwrapping.
fn video_media_type() -> Option<&'static AVMediaType> {
    // SAFETY: reading an `extern "C"` static from a framework that is linked
    // into this binary. The binding is `Option<&'static ...>`, so a null symbol
    // is a `None` rather than a dangling reference.
    unsafe { AVMediaTypeVideo }
}

pub(super) fn permission_status() -> PermissionStatus {
    let Some(media) = video_media_type() else {
        return PermissionStatus::NotApplicable;
    };
    // SAFETY: `authorizationStatusForMediaType:` raises an NSInvalidArgumentException
    // for any media type other than video or audio; this is video. It is
    // otherwise a pure query with no threading requirement.
    let status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(media) };
    match status {
        AVAuthorizationStatus::Authorized => PermissionStatus::Granted,
        AVAuthorizationStatus::Denied => PermissionStatus::Denied,
        AVAuthorizationStatus::Restricted => PermissionStatus::Restricted,
        _ => PermissionStatus::NotDetermined,
    }
}

fn permission_gate() -> Result<(), CameraError> {
    let status = permission_status();
    if status.may_open() {
        Ok(())
    } else {
        Err(CameraError::PermissionDenied(status))
    }
}

// ───────────────────────────────────────────────────────────────────────────
// Discovery
// ───────────────────────────────────────────────────────────────────────────

/// The major component of the running macOS version.
///
/// `NSProcessInfo` reports Big Sur and later as major 11+, and everything
/// earlier as major 10 with the interesting number in `minorVersion`. Every gate
/// in this file is `>= 13`, so comparing the major alone is correct and needs no
/// 10.x special case.
fn macos_major() -> i64 {
    NSProcessInfo::processInfo().operatingSystemVersion().majorVersion as i64
}

/// The device types the discovery session searches.
///
/// Order matters: AVFoundation documents that results come back sorted to match
/// this array, so the built-in camera stays first and the user's iPhone does not
/// silently become the default when it wanders into Bluetooth range.
///
/// See the module docs for why exactly two of these are linked symbols and the
/// rest are string literals.
fn video_device_types() -> Vec<&'static AVCaptureDeviceType> {
    let mut types: Vec<&'static AVCaptureDeviceType> = Vec::with_capacity(4);

    // SAFETY: both statics are `AVF_EXPORT` from macOS 10.15, below this app's
    // 11.0 deployment target, so dyld can always bind them. Reading an
    // `extern "C"` static is the only unsafety and the value is a `&'static
    // NSString` owned by the framework.
    unsafe {
        types.push(AVCaptureDeviceTypeBuiltInWideAngleCamera);
        // Deprecated in favour of `AVCaptureDeviceTypeExternal`, and used here
        // ON PURPOSE: the replacement is a macOS 14 symbol that would break
        // launch on 11-13, while this one exists on every version we support
        // and, from 14 onward, has the replacement's value anyway.
        #[allow(deprecated)]
        types.push(AVCaptureDeviceTypeExternalUnknown);
    }

    let major = macos_major();
    if major >= 13 {
        // An iPhone's ultra-wide, perspective-corrected to look like an overhead
        // shot of the desk — which, for a piano, is an overhead shot of the
        // keyboard. Free to offer and the user simply will not see the row if
        // they have no iPhone.
        types.push(ns_string!("AVCaptureDeviceTypeDeskViewCamera"));
    }
    if major >= 14 {
        types.push(ns_string!("AVCaptureDeviceTypeContinuityCamera"));
    }
    types
}

/// Every camera, as AVFoundation objects.
///
/// Deliberately **not** `AVCaptureDevice.devices()`: that is deprecated and, more
/// to the point, it does not surface Continuity Cameras at all. Apple documents
/// that devices of the newer types "may only be discovered using an
/// AVCaptureDeviceDiscoverySession", which is also why [`find_device`] searches
/// this list rather than calling `+deviceWithUniqueID:`.
fn discover() -> Result<Vec<Retained<AVCaptureDevice>>, CameraError> {
    let Some(media) = video_media_type() else {
        return Err(CameraError::Enumeration(
            "AVFoundation did not export AVMediaTypeVideo".into(),
        ));
    };
    let types = NSArray::from_slice(&video_device_types());
    // SAFETY: `deviceTypes` is a non-null array of genuine AVCaptureDeviceType
    // strings (see `video_device_types` for why every element is valid on the
    // running OS), the media type is video, and `Unspecified` is the documented
    // "any position" value. The call has no threading requirement.
    let session = unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &types,
            Some(media),
            AVCaptureDevicePosition::Unspecified,
        )
    };
    // SAFETY: a plain property read on the session we just built.
    let devices = unsafe { session.devices() };
    Ok(devices.iter().collect())
}

fn unique_id(device: &AVCaptureDevice) -> String {
    // SAFETY: `uniqueID` is a non-null property on every AVCaptureDevice.
    unsafe { device.uniqueID() }.to_string()
}

fn find_device(uid: &str) -> Result<Retained<AVCaptureDevice>, CameraError> {
    discover()?
        .into_iter()
        .find(|d| unique_id(d) == uid)
        .ok_or_else(|| CameraError::NotFound(uid.to_string()))
}

// ───────────────────────────────────────────────────────────────────────────
// Formats
// ───────────────────────────────────────────────────────────────────────────

/// One row of the format picker, plus the platform objects behind it.
struct Offer {
    format: Format,
    device_format: Retained<AVCaptureDeviceFormat>,
    /// The `CMTime` that pins the rate, taken from the frame-rate range itself
    /// rather than computed as `1/fps`.
    ///
    /// **Not a rounding nicety.** 1/29.97 has no exact `CMTime`, and
    /// `setActiveVideoMinFrameDuration:` rejects — silently, by clamping — a
    /// duration that is not inside the active format's supported range. The
    /// range's own `minFrameDuration` is by construction exactly at the boundary.
    min_frame_duration: CMTime,
}

/// Every distinct geometry-and-rate a device offers.
///
/// **Deduplicated, and the duplicates are real.** A Logitech C920 lists
/// 1920x1080 at 30 three times, once per native encoding (`420v`, `420f`,
/// `yuvs`). Because `videoSettings` forces BGRA, all three produce identical
/// output and identical work, so showing three identical rows in a picker is
/// noise that makes the user think they have to understand something.
fn offers_for(device: &AVCaptureDevice) -> Vec<Offer> {
    let mut offers: Vec<Offer> = Vec::new();
    // SAFETY: `formats` is a non-null array property; each element is an
    // `AVCaptureDeviceFormat` whose `formatDescription` is non-null for a video
    // device, and `CMVideoFormatDescriptionGetDimensions` reads it by reference.
    let formats = unsafe { device.formats() };
    for device_format in formats.iter() {
        let dims = unsafe {
            let desc = device_format.formatDescription();
            CMVideoFormatDescriptionGetDimensions(&desc)
        };
        let (Ok(width), Ok(height)) = (u32::try_from(dims.width), u32::try_from(dims.height))
        else {
            continue;
        };
        // SAFETY: another non-null array property; `maxFrameRate` and
        // `minFrameDuration` are plain scalar reads on its elements.
        let ranges = unsafe { device_format.videoSupportedFrameRateRanges() };
        for range in ranges.iter() {
            let fps = unsafe { range.maxFrameRate() };
            let format = Format {
                width,
                height,
                fps,
            };
            if offers.iter().any(|o| o.format == format) {
                continue;
            }
            offers.push(Offer {
                format,
                device_format: device_format.clone(),
                min_frame_duration: unsafe { range.minFrameDuration() },
            });
        }
    }
    offers
}

// ───────────────────────────────────────────────────────────────────────────
// Enumeration
// ───────────────────────────────────────────────────────────────────────────

pub(super) fn cameras() -> Result<Vec<CameraInfo>, CameraError> {
    permission_gate()?;
    let devices = discover()?;

    // The default is matched **by UID**, not by name. `audio.rs` has to match
    // its default by name and documents that this puts the flag on the wrong one
    // of two identical interfaces; with a real UID that whole caveat evaporates.
    let default_uid = video_media_type()
        // SAFETY: media type is video, as the method requires.
        .and_then(|m| unsafe { AVCaptureDevice::defaultDeviceWithMediaType(m) })
        .map(|d| unique_id(&d));

    Ok(devices
        .iter()
        .map(|device| {
            let uid = unique_id(device);
            CameraInfo {
                is_default: Some(&uid) == default_uid.as_ref(),
                // SAFETY: a non-null property. Localised, hence display-only.
                name: unsafe { device.localizedName() }.to_string(),
                formats: offers_for(device).into_iter().map(|o| o.format).collect(),
                uid,
            }
        })
        .collect())
}

// ───────────────────────────────────────────────────────────────────────────
// The delegate
// ───────────────────────────────────────────────────────────────────────────

struct DelegateIvars {
    slot: Arc<FrameSlot>,
    stats: Arc<CameraStats>,
    timebase: Timebase,
}

define_class!(
    // SAFETY:
    // - NSObject has no subclassing requirements.
    // - The class does not implement `Drop`.
    #[unsafe(super(NSObject))]
    // Deliberately NOT `#[name = "..."]`. objc2's default derives the runtime
    // name from the module path *and the crate version*, and registering a
    // fixed class name twice in one process is a hard runtime failure — which is
    // a live risk here rather than a theoretical one, because Tangent ships both
    // a standalone app and a VST3 and a host can load a plugin into a process
    // that already contains this crate.
    #[ivars = DelegateIvars]
    struct FrameDelegate;

    unsafe impl NSObjectProtocol for FrameDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for FrameDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn did_output_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            self.accept(sample_buffer);
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        fn did_drop_sample_buffer(
            &self,
            _output: &AVCaptureOutput,
            _sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            // The dropped buffer carries metadata and no pixels. Counting it is
            // the whole job: a rising count means THIS callback is too slow and
            // is holding the capture pool's memory hostage, which is a different
            // problem from the preview being slow to read.
            self.ivars().stats.note_dropped_late();
        }
    }
);

impl FrameDelegate {
    fn new(ivars: DelegateIvars) -> Retained<Self> {
        let this = Self::alloc().set_ivars(ivars);
        // SAFETY: `init` on a freshly allocated NSObject subclass whose ivars
        // have been set, which is exactly the initialiser contract.
        unsafe { msg_send![super(this), init] }
    }

    /// The entire capture-facing body. Everything it can get wrong lives in
    /// [`bgra_to_rgba`], which is tested against synthetic padded buffers.
    ///
    /// Runs on the serial dispatch queue. It must not panic, must not block on
    /// the UI thread, and must not hold onto the sample buffer: AVFoundation's
    /// own documentation warns that retaining sample buffers starves the capture
    /// pool and makes the device start dropping frames.
    /// The delivery callback, with a panic barrier around it.
    ///
    /// objc2 0.6 emits method impls as `extern "C-unwind"`, so a panic here
    /// unwinds INTO AVFoundation's libdispatch frames. Nothing up that stack
    /// handles it, and any frame compiled `-fno-exceptions` makes it genuinely
    /// undefined behaviour rather than a clean abort.
    ///
    /// `accept_inner` is panic-free by inspection today — every fallible step
    /// is a `let ... else` and `bgra_to_rgba` bounds-checks — but nothing
    /// *enforces* that, and the obvious future edits (an `unwrap`, a slice
    /// index) would reintroduce it silently. This makes the guarantee
    /// structural: a panic becomes a dropped frame and a counter.
    fn accept(&self, sample_buffer: &CMSampleBuffer) {
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.accept_inner(sample_buffer);
        }));
        if caught.is_err() {
            self.ivars()
                .stats
                .frames_unreadable
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn accept_inner(&self, sample_buffer: &CMSampleBuffer) {
        let ivars = self.ivars();

        // FIRST statement, before any work at all. This reading and the audio
        // callback's are the same clock, and the gap between them is the entire
        // audio/video sync error budget; anything done before it is added to
        // that budget permanently.
        let host_ns = ivars.timebase.now();

        // SAFETY: reading the sample's presentation stamp, a by-value scalar
        // read on a buffer AVFoundation owns for the duration of this call.
        let pts_ns = cmtime_nanos(unsafe { sample_buffer.presentation_time_stamp() });

        // SAFETY: `CMSampleBufferGetImageBuffer` returns a borrowed reference
        // that objc2 retains for us; `None` for a buffer carrying no image,
        // which is what a metadata-only sample looks like.
        let Some(image) = (unsafe { sample_buffer.image_buffer() }) else {
            ivars.stats.note_unreadable();
            return;
        };
        // `CVPixelBuffer` is a type alias for `CVImageBuffer` (CoreVideo's own
        // headers do the same), so this is a rename and not a cast.
        let pixels: &CVPixelBuffer = &image;

        let format = CVPixelBufferGetPixelFormatType(pixels);
        if format != kCVPixelFormatType_32BGRA || CVPixelBufferIsPlanar(pixels) {
            // `videoSettings` asked for BGRA, so this cannot happen — and if it
            // ever does, producing no picture is enormously better than
            // producing a garbled one that gets blamed on the camera.
            ivars.stats.note_unreadable();
            return;
        }

        let Some(_lock) = PixelLock::read_only(pixels) else {
            ivars.stats.note_unreadable();
            return;
        };

        let base = CVPixelBufferGetBaseAddress(pixels);
        if base.is_null() {
            ivars.stats.note_unreadable();
            return;
        }
        let stride = CVPixelBufferGetBytesPerRow(pixels);
        let len = CVPixelBufferGetDataSize(pixels);
        let (Ok(width), Ok(height)) = (
            u32::try_from(CVPixelBufferGetWidth(pixels)),
            u32::try_from(CVPixelBufferGetHeight(pixels)),
        ) else {
            ivars.stats.note_unreadable();
            return;
        };

        // SAFETY: the buffer is locked for reading for the lifetime of `_lock`,
        // which outlives this slice; `base` is non-null and CoreVideo documents
        // `CVPixelBufferGetDataSize` as the size of that allocation, so `len`
        // bytes from `base` are readable and no other thread may write them
        // while the read lock is held. `bgra_to_rgba` bounds-checks `len`
        // against the stride and height rather than trusting them to agree.
        let src = unsafe { std::slice::from_raw_parts(base.cast::<u8>(), len) };

        let mut buffer = ivars.slot.take_spare();
        if !bgra_to_rgba(src, width, height, stride, &mut buffer) {
            ivars.stats.note_unreadable();
            return;
        }

        ivars.slot.publish(Frame {
            width,
            height,
            // The OUTPUT stride, which is always tight. The input's `stride` is
            // the padded one and must not leak downstream: that is the shear.
            stride: width as usize * super::BYTES_PER_PIXEL,
            pixels: buffer,
            host_ns,
            pts_ns,
        });
        ivars.stats.note_delivered();
    }
}

/// A `CVPixelBuffer` read lock that unlocks on the way out of every path.
///
/// A hand-written unlock at each `return` is how a locked pixel buffer escapes
/// into the capture pool: the device then blocks forever waiting for memory it
/// will never get back, which presents as the camera freezing after a few
/// seconds rather than as an error.
struct PixelLock<'a> {
    buffer: &'a CVPixelBuffer,
}

impl<'a> PixelLock<'a> {
    fn read_only(buffer: &'a CVPixelBuffer) -> Option<Self> {
        // SAFETY: CoreVideo's documented pairing. `ReadOnly` promises we will
        // not write, which we do not; the matching unlock with the same flags is
        // in `Drop` and is unconditional.
        let rc = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        (rc == CV_RETURN_SUCCESS).then_some(Self { buffer })
    }
}

impl Drop for PixelLock<'_> {
    fn drop(&mut self) {
        // SAFETY: paired with the successful lock in `read_only`, same flags,
        // same buffer, and reached exactly once because `read_only` is the only
        // constructor.
        unsafe {
            CVPixelBufferUnlockBaseAddress(self.buffer, CVPixelBufferLockFlags::ReadOnly);
        }
    }
}

/// A `CMTime` as nanoseconds, or `None` if it is not a finite valid time.
///
/// Two traps in five lines. `CMTime` is `#[repr(C, packed(4))]`, so every field
/// is copied into a local before it is used — taking a reference to a packed
/// field (which `flags.contains(..)` would do implicitly) is an error, and the
/// "fix" of adding `&` is unsound. And the multiply is done in `i128`: a
/// `mach_absolute_time`-derived value at a nanosecond timescale is ~1e12, and
/// `1e12 * 1e9` overflows `i64` by three orders of magnitude.
fn cmtime_nanos(t: CMTime) -> Option<Nanos> {
    let flags = t.flags;
    let value = t.value;
    let timescale = t.timescale;
    if !flags.contains(CMTimeFlags::Valid) || timescale <= 0 {
        return None;
    }
    let ns = i128::from(value) * 1_000_000_000i128 / i128::from(timescale);
    i64::try_from(ns).ok()
}

// ───────────────────────────────────────────────────────────────────────────
// Opening
// ───────────────────────────────────────────────────────────────────────────

/// A running `AVCaptureSession` and everything that must outlive it.
///
/// `output` and `delegate` are held because the output owns the delegate only
/// weakly-ish (it retains it, but we need both to detach cleanly), and `queue`
/// because dropping the last reference to a dispatch queue with work in flight
/// is not something to find out about empirically.
struct MacCamera {
    session: Retained<AVCaptureSession>,
    device: Retained<AVCaptureDevice>,
    output: Retained<AVCaptureVideoDataOutput>,
    _delegate: Retained<FrameDelegate>,
    _queue: DispatchRetained<DispatchQueue>,
    format: Format,
}

impl VideoSource for MacCamera {
    fn format(&self) -> Format {
        self.format
    }

    fn state(&self) -> DeviceState {
        // SAFETY: two plain property reads. AVCaptureSession is documented
        // thread-safe and AVCaptureDevice's `connected` is KVO-observable, i.e.
        // readable from any thread.
        let connected = unsafe { self.device.isConnected() };
        let running = unsafe { self.session.isRunning() };
        match (connected, running) {
            // Unplugged, browned out, or the hub re-enumerated. RECORDER-PLAN §4:
            // stop the take, finalise every file that has bytes, mark it
            // incomplete, never discard what was captured.
            (false, _) => DeviceState::Lost,
            (true, true) => DeviceState::Running,
            // Still attached but the session stopped on its own: a runtime error
            // AVFoundation posted a notification about that we do not subscribe
            // to. Treated as fatal to the take, because a take with an
            // unexplained hole is worse than a short one.
            (true, false) => DeviceState::Errored,
        }
    }
}

impl Drop for MacCamera {
    fn drop(&mut self) {
        // SAFETY: both are documented, thread-safe teardown calls.
        // `stopRunning` blocks until the session has stopped, so no frame can
        // arrive after the delegate is detached. The order matters the other way
        // too: detaching first would leave a running session pushing frames at
        // nobody, which AVFoundation tolerates but Instruments does not forgive.
        unsafe {
            self.session.stopRunning();
            self.output.setSampleBufferDelegate_queue(None, None);
        }
    }
}

/// The fallible half of the configuration bracket, split out so that
/// `commitConfiguration` is unconditional at the call site rather than repeated
/// on every error path — which is how one of them ends up missing it.
///
/// # Safety
///
/// The caller must have called `beginConfiguration` on `session` and must call
/// `commitConfiguration` whatever this returns.
unsafe fn build_session(
    session: &AVCaptureSession,
    device: &AVCaptureDevice,
    chosen: &Offer,
    name: &str,
) -> Result<Retained<AVCaptureVideoDataOutput>, CameraError> {
    // SAFETY: the caller holds the configuration bracket, per this function's
    // own contract; every `add` is guarded by its `can`, which is
    // AVCaptureSession's documented protocol.
    unsafe {
        let input = AVCaptureDeviceInput::deviceInputWithDevice_error(device)
            // This is also where the TCC prompt appears for a NotDetermined
            // permission, and where "another app has the camera" surfaces.
            .map_err(|e| CameraError::Open(e.localizedDescription().to_string()))?;
        if !session.canAddInput(&input) {
            return Err(CameraError::Open(format!(
                "\"{name}\" cannot be added to a session"
            )));
        }
        session.addInput(&input);

        // The format is applied HERE — after the input is attached and inside
        // the configuration bracket — and the ordering was found the hard way.
        //
        // Setting `activeFormat` on a *detached* device and then adding it to a
        // session loses silently: the session reconfigures its inputs to suit
        // its preset (`High` by default), so the camera runs at the preset's
        // format while `format()` cheerfully reports what was asked for. Doing
        // it here is what a built-in camera honours; a UVC device needs the
        // second, post-`startRunning` application too, and `open` explains why.
        configure(device, chosen)?;

        let output = AVCaptureVideoDataOutput::new();
        output.setVideoSettings(Some(&bgra_video_settings()));
        // Newest-wins begins here, one layer below `FrameSlot`: with this off,
        // AVFoundation queues frames our callback has not collected and the
        // preview's latency grows without bound instead of frames being lost.
        output.setAlwaysDiscardsLateVideoFrames(true);
        if !session.canAddOutput(&output) {
            return Err(CameraError::Open("the video output was refused".into()));
        }
        session.addOutput(&output);
        Ok(output)
    }
}

pub(super) fn open(
    uid: &str,
    wish: &FormatWish,
    timebase: Timebase,
    slot: Arc<FrameSlot>,
    stats: Arc<CameraStats>,
) -> Result<Box<dyn VideoSource>, CameraError> {
    permission_gate()?;
    let device = find_device(uid)?;
    // SAFETY: a non-null property read, used only for error messages.
    let name = unsafe { device.localizedName() }.to_string();

    let offers = offers_for(&device);
    let geometry: Vec<Format> = offers.iter().map(|o| o.format).collect();
    let chosen = pick_format(&geometry, wish).ok_or(CameraError::NoUsableFormat {
        camera: name.clone(),
        offered: geometry.len(),
    })?;
    let chosen = &offers[chosen];

    // SAFETY: `new` is `unsafe` across objc2's generated bindings because a
    // class's designated initialiser may have requirements; AVCaptureSession's
    // has none and `+new` is the documented way to make one.
    let session = unsafe { AVCaptureSession::new() };
    // SAFETY: the whole block is AVCaptureSession's documented configuration
    // protocol — every mutation between `beginConfiguration` and
    // `commitConfiguration`, every `add` guarded by its `can` — performed on one
    // thread with no other reference to the session in existence yet.
    let (output, queue, delegate) = unsafe {
        session.beginConfiguration();

        // Every failure inside the bracket still commits before it propagates.
        // An `AVCaptureSession` abandoned between `beginConfiguration` and
        // `commitConfiguration` leaves a half-applied configuration on a
        // *device* that outlives the session, and the next open inherits it.
        let built = build_session(&session, &device, chosen, &name);
        session.commitConfiguration();
        let output = built?;

        // A SERIAL queue. `setSampleBufferDelegate:queue:` documents that frame
        // ordering is only guaranteed on one, and out-of-order frames would make
        // `host_ns` non-monotonic — which nothing downstream checks for, because
        // nothing downstream should have to.
        let queue = DispatchQueue::new(QUEUE_LABEL, None);
        let delegate = FrameDelegate::new(DelegateIvars {
            slot,
            stats,
            timebase,
        });
        output.setSampleBufferDelegate_queue(
            Some(ProtocolObject::from_ref(&*delegate)),
            Some(&queue),
        );

        // Blocks. 300-800 ms on a built-in camera, over 2 s for a Continuity
        // Camera, and the first ever run is the slowest. RECORDER-PLAN §3's
        // reason for opening the camera when the Recorder band opens rather than
        // when Record is pressed is exactly this number.
        session.startRunning();

        (output, queue, delegate)
    };

    // **`startRunning` re-imposes the session preset over `activeFormat`, and
    // re-applying it afterwards is the only thing that survives.** Measured on
    // an MX Brio, by reading `activeFormat` back at each step: it is still the
    // requested 1280x720 immediately after `commitConfiguration`, and it is
    // 1920x1080@24 the instant `startRunning` returns. The clobber is at START,
    // not at commit, which is why no amount of care inside the configuration
    // bracket fixes it. macOS has no escape hatch here —
    // `AVCaptureSessionPresetInputPriority` is iOS-only, `API_UNAVAILABLE(macos)`
    // — and setting a matching named preset (`AVCaptureSessionPreset1280x720`)
    // is accepted and then ignored by the device's DAL plugin. Only this works.
    //
    // Guarded by a read-back because reconfiguring a *running* session costs
    // ~1.5 s on that same camera, and a device that already honoured the format
    // must not pay it.
    if active_format(&device).is_none_or(|actual| !matches(&actual, &chosen.format)) {
        // Deliberately not `?`. By this point there IS a running camera; failing
        // the open would throw away a working picture over a frame rate. The
        // read-back below reports whatever it actually settled on, which is the
        // property that matters and the one nothing downstream can recover from
        // being lied to about.
        let _ = configure(&device, chosen);
    }

    // **Read back rather than remember.** `format()` must describe the frames
    // that actually arrive, and the paragraph above is the proof that what was
    // asked for and what the session settled on are two different things. A
    // reported format that disagrees with the delivered pixels is worse than no
    // report at all: every downstream size assumption — texture allocation,
    // encoder setup, the composite's layout — is built on it.
    let format = active_format(&device).unwrap_or(chosen.format);

    Ok(Box::new(MacCamera {
        session,
        device,
        output,
        _delegate: delegate,
        _queue: queue,
        format,
    }))
}

/// `{ kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_32BGRA }`.
///
/// The key is written as the literal `"PixelFormatType"` rather than as
/// CoreVideo's `kCVPixelBufferPixelFormatTypeKey`, because that constant is a
/// `CFString` and the dictionary wants an `NSString`; bridging it would mean an
/// unsafe toll-free cast to save nothing. The literal was verified against the
/// running framework the same way the device-type strings were.
fn bgra_video_settings() -> Retained<NSDictionary<NSString, AnyObject>> {
    let value = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
    NSDictionary::from_slices(&[ns_string!("PixelFormatType")], &[&*value as &AnyObject])
}

/// What the device is *actually* set to, right now.
///
/// The frame rate comes from `activeVideoMinFrameDuration` when the device
/// reports a usable one — that is the cap [`configure`] just installed — and
/// falls back to the top of the active format's own range, which is what the
/// device runs at when nothing has capped it.
fn active_format(device: &AVCaptureDevice) -> Option<Format> {
    // SAFETY: three non-null property reads on a device that is attached to a
    // committed session, plus a by-reference read of its format description.
    unsafe {
        let device_format = device.activeFormat();
        let dims = CMVideoFormatDescriptionGetDimensions(&device_format.formatDescription());
        let (Ok(width), Ok(height)) = (u32::try_from(dims.width), u32::try_from(dims.height))
        else {
            return None;
        };
        let duration = device.activeVideoMinFrameDuration();
        let capped = cmtime_nanos(duration).filter(|ns| *ns > 0).map(|ns| 1e9 / ns as f64);
        let fps = capped.unwrap_or_else(|| {
            device_format
                .videoSupportedFrameRateRanges()
                .iter()
                .map(|r| r.maxFrameRate())
                .fold(0.0, f64::max)
        });
        Some(Format {
            width,
            height,
            fps,
        })
    }
}

/// Apply the chosen format and pin the frame rate.
///
/// **The order is load-bearing.** `setActiveFormat:` resets
/// `activeVideoMinFrameDuration` to the new format's default, so setting the
/// duration first silently accomplishes nothing and the camera runs at whatever
/// rate it likes.
///
/// Only the *minimum* duration is set, which caps the rate from above. Setting
/// `activeVideoMaxFrameDuration` too would lock the rate exactly, at the price
/// of forbidding the camera from lengthening its exposure in a dim room — a
/// constant frame rate bought with a black picture, which is the wrong trade for
/// somebody filming their hands under a piano lamp.
fn configure(device: &AVCaptureDevice, offer: &Offer) -> Result<(), CameraError> {
    // SAFETY: `lockForConfiguration` is AVCaptureDevice's documented exclusion
    // protocol; every mutation below happens under it and the unlock is on both
    // paths out. It fails when another process holds the device, which is a
    // `Result` and not a panic.
    unsafe {
        device
            .lockForConfiguration()
            .map_err(|e| CameraError::Configure(e.localizedDescription().to_string()))?;
        device.setActiveFormat(&offer.device_format);
        // **Read the format back before setting the duration, and this guard is
        // the difference between a wrong frame rate and a dead process.**
        //
        // `setActiveVideoMinFrameDuration:` raises an uncaught
        // NSInvalidArgumentException when the duration is not *exactly* one the
        // ACTIVE format supports, and an Objective-C exception crossing a Rust
        // frame aborts — there is no catching it and no crash report worth
        // reading. Reproduced on a Logitech MX Brio, whose "30 fps" range is
        // actually `1000000/30000030`: a computed `CMTimeMake(1, 30)` is
        // rejected outright, which is why `Offer` carries the range's own
        // `minFrameDuration` verbatim. This checks that the format we just
        // asked for is the one that took, because if it silently did not, the
        // duration we hold belongs to some other format and is a live grenade.
        let active = device.activeFormat();
        if frame_duration_supported(&active, offer.min_frame_duration) {
            device.setActiveVideoMinFrameDuration(offer.min_frame_duration);
        }
        device.unlockForConfiguration();
    }
    Ok(())
}

/// Whether `duration` is one this format will accept.
///
/// Exact equality against each range's own endpoints rather than an ordering
/// comparison: `CMTime` is a rational and `1/30` and `1000000/30000030` are
/// different values that both print as "30 fps", so only a duration taken
/// verbatim from a range is safe to hand back to AVFoundation.
fn frame_duration_supported(device_format: &AVCaptureDeviceFormat, duration: CMTime) -> bool {
    // SAFETY: a non-null array property and two scalar reads per element.
    unsafe {
        device_format
            .videoSupportedFrameRateRanges()
            .iter()
            .any(|r| r.minFrameDuration() == duration || r.maxFrameDuration() == duration)
    }
}

/// Whether the device is running the geometry and rate we asked for.
fn matches(actual: &Format, wanted: &Format) -> bool {
    actual.width == wanted.width
        && actual.height == wanted.height
        && (actual.fps - wanted.fps).abs() <= super::FPS_EPSILON
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_invalid_cmtime_yields_no_timestamp_rather_than_a_zero() {
        // Zero is a legitimate presentation stamp; conflating it with "there was
        // no stamp" would anchor a clock on a value that never existed.
        let invalid = CMTime {
            value: 5,
            timescale: 1_000_000_000,
            flags: CMTimeFlags::empty(),
            epoch: 0,
        };
        assert_eq!(cmtime_nanos(invalid), None);
    }

    #[test]
    fn a_zero_timescale_cmtime_does_not_divide_by_zero() {
        let broken = CMTime {
            value: 1,
            timescale: 0,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(cmtime_nanos(broken), None);
    }

    #[test]
    fn a_cmtime_is_converted_through_its_own_timescale_and_not_assumed_nanos() {
        // 90 kHz is a real media timescale; treating the raw value as nanos
        // would be off by a factor of 11111.
        let t = CMTime {
            value: 90_000,
            timescale: 90_000,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(cmtime_nanos(t), Some(1_000_000_000));
    }

    #[test]
    fn a_host_clock_sized_cmtime_does_not_overflow_on_the_way_to_nanos() {
        // Two days of uptime at a nanosecond timescale. In `i64`, the
        // intermediate `value * 1e9` is ~1.7e23 and wraps; in `i128` it does not.
        let t = CMTime {
            value: 172_800_000_000_000,
            timescale: 1_000_000_000,
            flags: CMTimeFlags::Valid,
            epoch: 0,
        };
        assert_eq!(cmtime_nanos(t), Some(172_800_000_000_000));
    }

    #[test]
    fn the_discovery_list_always_contains_the_two_symbols_old_macos_can_bind() {
        // If this ever returns fewer than two, a linked symbol has been swapped
        // for a runtime string and the built-in camera has stopped being found.
        let types = video_device_types();
        assert!(types.len() >= 2);
        assert_eq!(
            types[0].to_string(),
            "AVCaptureDeviceTypeBuiltInWideAngleCamera",
            "the built-in camera must sort first, or an iPhone in the room \
             silently becomes the default"
        );
    }

    #[test]
    fn continuity_camera_is_offered_exactly_when_the_os_can_bind_it() {
        // The deployment-target trap, asserted rather than trusted: the type is
        // present from macOS 14 and absent before it, and the string is the one
        // verified against the running framework.
        let types: Vec<String> = video_device_types().iter().map(|t| t.to_string()).collect();
        let has = types.iter().any(|t| t == "AVCaptureDeviceTypeContinuityCamera");
        assert_eq!(has, macos_major() >= 14);
    }

    #[test]
    fn no_device_type_string_is_empty_or_duplicated() {
        let types: Vec<String> = video_device_types().iter().map(|t| t.to_string()).collect();
        assert!(types.iter().all(|t| !t.is_empty()));
        let mut sorted = types.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), types.len(), "a duplicate type would double every device");
    }

    #[test]
    fn the_video_settings_dictionary_asks_for_bgra_and_nothing_else() {
        // If this key or value is wrong AVFoundation delivers its native format
        // — usually planar YUV — and every frame is counted unreadable.
        let settings = bgra_video_settings();
        assert_eq!(settings.count(), 1);
        let value = settings
            .objectForKey(ns_string!("PixelFormatType"))
            .expect("the pixel format key");
        let number: &NSNumber = value.downcast_ref().expect("an NSNumber");
        assert_eq!(number.as_u32(), kCVPixelFormatType_32BGRA);
    }
}
