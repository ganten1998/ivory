//! The V4L2 camera backend, via `linuxvideo` — RECORDER-PLAN §4's Linux slot.
//!
//! Everything policy-shaped lives above the seam in `camera.rs` and is shared
//! with macOS: this file only enumerates devices, negotiates a format, and
//! runs the capture loop. Read the macOS backend's docs first; where the two
//! differ it is because the platforms do, and the differences are written down
//! here.
//!
//! # Identity: `bus_info`, not the device node
//!
//! `/dev/video0` is a discovery order, not a name — plug two cameras in and
//! reboot, and they can swap. `VIDIOC_QUERYCAP.bus_info`
//! (`usb-0000:00:1a.0-1.1`) names the *port the camera is plugged into*, which
//! survives reboots and distinguishes two identical webcams, exactly the job
//! [`CameraInfo::uid`](super::CameraInfo) documents. It changes if the camera
//! moves to another USB port, which AVFoundation's `uniqueID` does not — the
//! honest cost of having no serial number to read.
//!
//! # One physical camera is several device nodes
//!
//! uvcvideo registers a metadata node beside every capture node (this
//! machine's FaceTime HD is `/dev/video0` capture + `/dev/video1` metadata).
//! Enumeration filters on `device_capabilities().VIDEO_CAPTURE`, so the
//! metadata node never becomes a phantom second camera in the picker.
//!
//! # Formats: YUYV converted here, MJPG decoded, everything else declined
//!
//! Most UVC cameras offer their full frame rate uncompressed only at small
//! sizes — this machine's camera does YUYV at 30 fps up to 640x480 and needs
//! MJPG for 1280x720 at 30 — so MJPEG support is not an optimisation, it is
//! what makes 720p exist at all on Linux. When the same size-and-rate is
//! offered both ways, YUYV wins: a byte reshuffle beats a JPEG decode that
//! costs real milliseconds on the machine that is also running a synth.
//!
//! # The capture loop polls, and the poll is what makes drop prompt
//!
//! `ReadStream::dequeue` blocks with no timeout, and a stalled camera would
//! park a thread in it forever — so the loop asks `will_block()` first and
//! sleeps a few milliseconds when nothing is queued. Dropping the
//! [`VideoSource`] flips a stop flag and joins the thread, which is therefore
//! never more than one sleep away from exiting. The join matters: reopening
//! the same camera while an old stream still holds the device is `EBUSY`, and
//! "switching cameras sometimes fails" is precisely the bug that leaves.

use std::io;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use linuxvideo::format::{FrameIntervals, FrameSizes, PixFormat, PixelFormat};
use linuxvideo::{BufType, CapabilityFlags, Device, Fract};

use super::{
    pick_format, yuyv_to_rgba, CameraError, CameraInfo, CameraStats, Format, FormatWish,
    FrameSlot, PermissionStatus, VideoSource, BYTES_PER_PIXEL, FPS_EPSILON,
};
use crate::audio::{DeviceState, Timebase};

/// No TCC on Linux: whether a camera can be opened is decided by file
/// permissions on the device node, and that failure is reported by `open`,
/// where it happens, with the fix in the message.
pub(super) fn permission_status() -> PermissionStatus {
    PermissionStatus::NotApplicable
}

/// One offered (format, native encoding, exact interval) triple.
///
/// The `Fract` is kept from enumeration rather than re-derived from the f64,
/// because `VIDIOC_S_PARM` wants the *driver's own* fraction back — 29.97 fps
/// is `1001/30000`, and a reconstructed `100/2997` is a different fraction the
/// driver is free to round somewhere unhelpful.
struct Offer {
    format: Format,
    pixel: PixelFormat,
    fract: Fract,
}

/// Every capture-capable device, sorted by node path for a stable order.
fn capture_nodes() -> Result<Vec<(PathBuf, Device)>, CameraError> {
    let iter = match linuxvideo::list() {
        Ok(iter) => iter,
        // No /dev/video* at all commonly surfaces as NotFound here; that is
        // "no cameras", not an error.
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(CameraError::Enumeration(e.to_string())),
    };
    let mut nodes = Vec::new();
    let mut denied: Option<PathBuf> = None;
    for dev in iter {
        let Ok(dev) = dev else {
            // A node that cannot be opened. Remember a permission denial so
            // that "every camera exists but none is readable" can say why.
            continue;
        };
        let Ok(path) = dev.path() else { continue };
        match dev.capabilities() {
            Ok(caps)
                if caps
                    .device_capabilities()
                    .contains(CapabilityFlags::VIDEO_CAPTURE) =>
            {
                nodes.push((path, dev));
            }
            _ => {}
        }
    }
    // A second pass for the EACCES case: `linuxvideo::list` swallows the open
    // error per node, so re-open one video node directly to name the failure.
    if nodes.is_empty() {
        for i in 0..4 {
            let path = PathBuf::from(format!("/dev/video{i}"));
            if path.exists() {
                if let Err(e) = Device::open(&path) {
                    if e.kind() == io::ErrorKind::PermissionDenied {
                        denied = Some(path);
                    }
                }
            }
        }
    }
    if let Some(path) = denied {
        return Err(CameraError::Enumeration(format!(
            "no permission to read {} - add your user to the 'video' group \
             (usermod -aG video $USER, then log out and back in)",
            path.display()
        )));
    }
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(nodes)
}

/// The card name, cleaned: uvcvideo appends ": " to some names.
fn card_name(dev: &Device) -> String {
    dev.capabilities()
        .map(|c| c.card().trim().trim_end_matches(':').trim().to_owned())
        .unwrap_or_else(|_| "Camera".to_owned())
}

fn bus_uid(dev: &Device) -> Option<String> {
    dev.capabilities().ok().map(|c| c.bus_info().to_owned())
}

/// Everything this device offers that this backend can convert, deduplicated
/// with YUYV preferred over MJPG at the same size and rate.
///
/// Returns the usable offers plus the raw count of (format, size, rate)
/// triples seen, which is what [`CameraError::NoUsableFormat`] reports.
fn offers(dev: &Device) -> (Vec<Offer>, usize) {
    let mut out: Vec<Offer> = Vec::new();
    let mut seen = 0usize;
    // YUYV first, so the dedup below keeps it over MJPG.
    for pf in [PixelFormat::YUYV, PixelFormat::MJPG] {
        let advertises = dev
            .formats(BufType::VIDEO_CAPTURE)
            .filter_map(Result::ok)
            .any(|f| f.pixel_format() == pf);
        if !advertises {
            continue;
        }
        let Ok(FrameSizes::Discrete(sizes)) = dev.frame_sizes(pf) else {
            // Stepwise/continuous ranges are virtual devices (v4l2loopback);
            // supporting them means inventing a size, which pick_format cannot
            // rank honestly. Count and decline.
            seen += 1;
            continue;
        };
        for s in sizes {
            let Ok(FrameIntervals::Discrete(ivals)) = dev.frame_intervals(pf, s.width(), s.height())
            else {
                seen += 1;
                continue;
            };
            for iv in ivals {
                seen += 1;
                let f = *iv.fract();
                if f.numerator() == 0 || f.denominator() == 0 {
                    continue;
                }
                let fps = f64::from(f.denominator()) / f64::from(f.numerator());
                let format = Format {
                    width: s.width(),
                    height: s.height(),
                    fps,
                };
                let duplicate = out.iter().any(|o| {
                    o.format.width == format.width
                        && o.format.height == format.height
                        && (o.format.fps - fps).abs() < FPS_EPSILON
                });
                if !duplicate {
                    out.push(Offer {
                        format,
                        pixel: pf,
                        fract: f,
                    });
                }
            }
        }
    }
    (out, seen)
}

pub(super) fn cameras() -> Result<Vec<CameraInfo>, CameraError> {
    let mut list = Vec::new();
    for (_, dev) in capture_nodes()? {
        let Some(uid) = bus_uid(&dev) else { continue };
        // Two capture nodes on one physical device: keep the first.
        if list.iter().any(|c: &CameraInfo| c.uid == uid) {
            continue;
        }
        let (offered, _) = offers(&dev);
        list.push(CameraInfo {
            uid,
            name: card_name(&dev),
            // V4L2 has no notion of a system default camera; the first node in
            // path order is the stable, least-surprising stand-in.
            is_default: list.is_empty(),
            formats: offered.into_iter().map(|o| o.format).collect(),
        });
    }
    Ok(list)
}

pub(super) fn open(
    uid: &str,
    wish: &FormatWish,
    timebase: Timebase,
    slot: Arc<FrameSlot>,
    stats: Arc<CameraStats>,
) -> Result<Box<dyn VideoSource>, CameraError> {
    let mut found = None;
    for (path, dev) in capture_nodes()? {
        if bus_uid(&dev).as_deref() == Some(uid) {
            found = Some((path, dev));
            break;
        }
    }
    let Some((_, dev)) = found else {
        return Err(CameraError::NotFound(uid.to_owned()));
    };
    let name = card_name(&dev);

    let (offered, seen) = offers(&dev);
    let formats: Vec<Format> = offered.iter().map(|o| o.format).collect();
    let Some(pick) = pick_format(&formats, wish) else {
        return Err(CameraError::NoUsableFormat {
            camera: name,
            offered: seen,
        });
    };
    let chosen = &offered[pick];

    let cap = dev
        .video_capture(PixFormat::new(
            chosen.format.width,
            chosen.format.height,
            chosen.pixel,
        ))
        .map_err(|e| CameraError::Configure(e.to_string()))?;

    // Read back what the driver actually granted — V4L2's S_FMT negotiates
    // rather than obeys, exactly like the session preset on macOS.
    let granted = cap.format();
    let pixel = granted.pixel_format();
    if pixel != PixelFormat::YUYV && pixel != PixelFormat::MJPG {
        return Err(CameraError::Configure(format!(
            "asked for {:?}, driver substituted {:?}, which this backend cannot convert",
            chosen.pixel, pixel
        )));
    }
    let (width, height) = (granted.width(), granted.height());
    let stride_in = granted.bytes_per_line() as usize;

    // The rate is per-stream state, set after the format. Failure is not fatal:
    // the stream still runs at the driver's default, and the read-back below
    // reports whatever that is.
    let actual_fract = cap.set_frame_interval(chosen.fract).unwrap_or(chosen.fract);
    let fps = if actual_fract.numerator() > 0 {
        f64::from(actual_fract.denominator()) / f64::from(actual_fract.numerator())
    } else {
        chosen.format.fps
    };

    let format = Format { width, height, fps };
    let stop = Arc::new(AtomicBool::new(false));
    let state = Arc::new(AtomicU8::new(state_as_u8(DeviceState::Running)));

    let thread_stop = Arc::clone(&stop);
    let thread_state = Arc::clone(&state);
    let join = std::thread::Builder::new()
        .name("ivory-camera".to_owned())
        .spawn(move || {
            capture_loop(
                cap,
                pixel,
                width,
                height,
                stride_in,
                timebase,
                &slot,
                &stats,
                &thread_stop,
                &thread_state,
            );
        })
        .map_err(|e| CameraError::Open(format!("could not start the capture thread: {e}")))?;

    Ok(Box::new(LinuxCamera {
        format,
        stop,
        state,
        join: Some(join),
    }))
}

/// The capture thread: dequeue, stamp, convert, publish, until told to stop.
#[allow(clippy::too_many_arguments)]
fn capture_loop(
    cap: linuxvideo::VideoCaptureDevice,
    pixel: PixelFormat,
    width: u32,
    height: u32,
    stride_in: usize,
    timebase: Timebase,
    slot: &FrameSlot,
    stats: &CameraStats,
    stop: &AtomicBool,
    state: &AtomicU8,
) {
    // `ReadStream` holds mmap'd pointers and is not `Send`, so it is built on
    // the thread that will use it — which is also why `open` cannot surface
    // this particular failure synchronously.
    let mut stream = match cap.into_stream() {
        Ok(s) => s,
        Err(_) => {
            state.store(state_as_u8(DeviceState::Errored), Ordering::Relaxed);
            return;
        }
    };

    // **Built on the first compressed frame, not up front.** Choosing between
    // hardware and software means probing with a real frame, and a camera that
    // never delivers one should not pay for the attempt.
    let mut mjpeg: Option<MjpegDecoder> = None;
    // The compressed frame, copied out of the driver's buffer so the decode
    // does not hold it. Reused for the life of the take: one allocation, not
    // one per frame.
    let mut compressed: Vec<u8> = Vec::new();
    while !stop.load(Ordering::Relaxed) {
        match stream.will_block() {
            Ok(true) => {
                std::thread::sleep(Duration::from_millis(3));
                continue;
            }
            Ok(false) => {}
            Err(e) => {
                state.store(state_as_u8(lost_or_errored(&e)), Ordering::Relaxed);
                return;
            }
        }
        // **Copy the frame out, then let go of the buffer.**
        //
        // The decode used to run INSIDE this closure, and the closure is what
        // holds the mmap'd buffer: the driver has only two, so a decode that
        // takes a third of a core left it exactly one to fill while the other
        // was busy. Anything arriving in that window overwrites a frame nobody
        // has read, which on Linux is invisible — V4L2 gives no sequence
        // number through `linuxvideo` 0.3 and there is nothing to compare.
        //
        // (Two is the crate's `DEFAULT_BUFFER_COUNT` and `ReadStream::new` is
        // `pub(crate)`, so asking for more would mean vendoring it.)
        //
        // **YUYV stays inside, and that is not an oversight.** It is a byte
        // reshuffle, not a decode: moving it out would mean copying the whole
        // UNCOMPRESSED frame first — 1.8 MB at 720p — to save less time than
        // the copy costs. MJPEG is the opposite case: a quarter of a megabyte
        // in, and a decode that dwarfs it.
        let mut pending: Option<crate::clock::Nanos> = None;
        let r = stream.dequeue(|view| {
            // First statement, per the module's timebase contract.
            let host_ns = timebase.now();
            if view.is_error() {
                stats.note_unreadable();
                return Ok(());
            }
            // **Before any work at all, after the dequeue.** The queue has to
            // keep moving whatever the answer is, or the driver backs up and
            // starts reporting errors; the conversion is the part worth
            // skipping when nobody is looking. See `FrameSlot::want`.
            if !slot.should_convert(host_ns) {
                stats.note_skipped();
                return Ok(());
            }
            let bytes: &[u8] = &view;
            match pixel {
                PixelFormat::YUYV => {
                    let began = std::time::Instant::now();
                    let stride = if stride_in >= width as usize * 2 {
                        stride_in
                    } else {
                        width as usize * 2
                    };
                    let mut dst = slot.take_spare();
                    if yuyv_to_rgba(bytes, width, height, stride, &mut dst) {
                        slot.note_convert_cost(began.elapsed().as_nanos() as u64);
                        slot.publish(super::Frame {
                            width,
                            height,
                            stride: width as usize * BYTES_PER_PIXEL,
                            pixels: dst,
                            host_ns,
                            pts_ns: None,
                        });
                        stats.note_delivered();
                    } else {
                        stats.note_unreadable();
                    }
                }
                _ => {
                    compressed.clear();
                    compressed.extend_from_slice(bytes);
                    pending = Some(host_ns);
                }
            }
            Ok::<(), io::Error>(())
        });
        if let Err(e) = r {
            state.store(state_as_u8(lost_or_errored(&e)), Ordering::Relaxed);
            return;
        }
        // Outside the checkout: the driver has its buffer back before the
        // expensive part starts.
        let Some(host_ns) = pending else {
            continue;
        };
        // **Timed, because the preview's rate is decided from it.** This is
        // the JPEG decode the budget exists for — and it is why hardware
        // decode raises the preview's rate rather than merely lowering the
        // load: 2.3 ms a frame instead of 32 buys back most of the interval.
        let began = std::time::Instant::now();
        let decoder =
            mjpeg.get_or_insert_with(|| MjpegDecoder::new(width, height, &compressed));
        let mut dst = slot.take_spare();
        if !decoder.decode(&compressed, width, height, &mut dst) {
            stats.note_unreadable();
            continue;
        }
        slot.note_convert_cost(began.elapsed().as_nanos() as u64);
        slot.publish(super::Frame {
            width,
            height,
            stride: width as usize * BYTES_PER_PIXEL,
            pixels: dst,
            host_ns,
            // V4L2 stamps `v4l2_buffer.timestamp`, but linuxvideo 0.3 does
            // not expose it; when it does, this is where it goes.
            pts_ns: None,
        });
        stats.note_delivered();
    }
}

/// Whichever MJPEG decoder this machine turned out to have.
///
/// Hardware decode is thirteen times cheaper on the CPU at 720p — 2.3 ms a
/// frame against 32.1, measured — which on a two-core laptop that is also
/// running a synth is the difference between the camera being affordable and
/// not. But it depends on a VA-API driver with a JPEG entrypoint, which most
/// machines have and some do not, so it is strictly an ACCELERATOR: when it is
/// missing, or when it declines a particular frame, `zune-jpeg` runs and the
/// only difference is the cost.
enum MjpegDecoder {
    Hardware(super::vaapi::Decoder),
    Software,
}

impl MjpegDecoder {
    fn new(width: u32, height: u32, sample: &[u8]) -> Self {
        match super::vaapi::Decoder::new(width, height, sample) {
            Some(d) => MjpegDecoder::Hardware(d),
            None => MjpegDecoder::Software,
        }
    }

    fn decode(&mut self, bytes: &[u8], width: u32, height: u32, dst: &mut Vec<u8>) -> bool {
        match self {
            // A frame the hardware declines — a sampling it cannot express, a
            // truncated frame, a transient driver error — costs a software
            // decode rather than the frame itself.
            MjpegDecoder::Hardware(d) => {
                d.decode(bytes, width, height, dst) || decode_mjpg(bytes, width, height, dst)
            }
            MjpegDecoder::Software => decode_mjpg(bytes, width, height, dst),
        }
    }
}

/// Decode one MJPEG frame to tightly packed RGBA at exactly `width`x`height`.
///
/// A frame whose own header disagrees with the negotiated size is refused —
/// publishing it would hand the compositor a buffer whose length disagrees
/// with its claimed geometry, which is the exact failure `Frame::stride`'s
/// docs warn about.
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
    dst.resize(width as usize * height as usize * BYTES_PER_PIXEL, 0);
    decoder.decode_into(dst).is_ok()
}

fn lost_or_errored(e: &io::Error) -> DeviceState {
    // ENODEV is the unplug; everything else is a driver in trouble.
    if e.raw_os_error() == Some(19) {
        DeviceState::Lost
    } else {
        DeviceState::Errored
    }
}

// The same private encoding `camera.rs` uses for `CameraStats`; restated here
// for the same reason it is restated there.
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

struct LinuxCamera {
    format: Format,
    stop: Arc<AtomicBool>,
    state: Arc<AtomicU8>,
    join: Option<JoinHandle<()>>,
}

impl VideoSource for LinuxCamera {
    fn format(&self) -> Format {
        self.format
    }

    fn state(&self) -> DeviceState {
        state_from_u8(self.state.load(Ordering::Relaxed))
    }
}

impl Drop for LinuxCamera {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            // The loop is at most one 3 ms sleep from seeing the flag, so this
            // join is bounded in practice; skipping it would leak the device
            // open and make an immediate reopen fail with EBUSY.
            let _ = join.join();
        }
    }
}
