//! Hardware MJPEG decode on Linux, via VA-API — the capture thread's half of
//! RECORDER-PLAN §4's "the camera must not cost more than the synth".
//!
//! # Why this exists
//!
//! Most UVC webcams offer their full frame rate only in MJPEG (the 2012
//! FaceTime HD this was written against does YUYV at 30 fps only up to
//! 640x480), so on Linux a 720p30 camera means a JPEG decode thirty times a
//! second, forever, on the same CPU that is running a synth. Measured on that
//! machine, 1280x720:
//!
//! | path                                   | CPU per frame |
//! |----------------------------------------|---------------|
//! | `zune-jpeg` (the software fallback)    | 32.07 ms      |
//! | this module                            |  2.32 ms      |
//!
//! Thirteen times less, and the machine has two cores at 1.8 GHz. At 30 fps
//! the software path alone is 96% of one core.
//!
//! # Three findings that shaped the design, all measured rather than assumed
//!
//! **1. The readback dominates, not the decode.** The GPU decode itself costs
//! 0.22 ms of CPU. Getting the pixels back costs everything else. A derived
//! VA image is *write-combined* memory: reading it with ordinary loads runs at
//! about 100 MB/s, so a naive `memcpy` of one 720p frame costs 35 ms and makes
//! the whole exercise *slower* than software. [`copy_wc`] reads it with
//! `MOVNTDQA` instead, which is the instruction that exists for this case, and
//! the same copy costs about 0.8 ms.
//!
//! **2. This driver's VPP cannot be told the JPEG is full range.** The obvious
//! design — let VA-API's video post-processor convert YUV to RGBA on the GPU —
//! produces a systematically dark picture, because i965 converts with
//! hardcoded *limited* range coefficients while JFIF JPEG is *full* range. It
//! honours neither the modern `input_color_properties` nor the legacy
//! `VA_SRC_*` filter flags (its VPP advertises only NoiseReduction and
//! Deinterlacing), so there is no way to ask. Correcting afterwards does not
//! work either: the wrong conversion clamps, and clamping is not invertible.
//! Measured against a reference decode, VPP output was 20-29 dB PSNR; doing
//! the conversion here is 49-54 dB, which is the chroma-siting difference and
//! nothing else.
//!
//! So this module does **not** use VPP. It takes the decoder's own YCbCr
//! surface and converts it here, where the coefficients are ours.
//!
//! **3. The conversion has to be SIMD or it eats the win.** Scalar, it cost
//! 9.7 ms a frame — a third of what it was replacing. See
//! [`yuv_to_rgba_sse2`].
//!
//! # Why `dlopen` rather than linking
//!
//! `ivory-record`'s Cargo.toml commits the Linux backend to adding "no C
//! toolchain requirement to the build", and Tangent ships as a tarball to
//! machines whose VA-API situation is unknown. Linking `libva` would mean a
//! binary that refuses to start where it is absent — for a feature that is an
//! optimisation. So the library is opened at runtime, every symbol is looked
//! up by hand, and absence is simply [`Decoder::new`] returning `None` and the
//! caller keeping `zune-jpeg`. No bindgen, no libclang, no link-time
//! dependency, and the struct layouts below are pinned by `const` assertions
//! against the sizes the real headers produce.

// The FFI names below are transcribed from <va/va.h> verbatim, and stay that
// way on purpose: a constant called `VAProfileJPEGBaseline` can be checked
// against the header by eye, where `VA_PROFILE_JPEG_BASELINE` has to be
// translated first. Rust's casing lints disagree, and here they are wrong.
#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::ffi::{c_char, c_int, c_uint, c_void};

// ---------------------------------------------------------------------------
// The slice of VA-API this needs, transcribed by hand.
//
// Every struct here is `#[repr(C)]` and carries a `const` assertion against
// the size the C header produces on x86-64. A silent layout drift is the one
// failure mode hand-written FFI has that bindgen does not, and an assertion
// that fails the build is much better than a driver reading the wrong bytes.
// ---------------------------------------------------------------------------

type VAStatus = c_int;
type VADisplay = *mut c_void;
type VAGenericID = c_uint;
type VAConfigID = VAGenericID;
type VAContextID = VAGenericID;
type VASurfaceID = VAGenericID;
type VABufferID = VAGenericID;
type VAImageID = VAGenericID;

const VA_STATUS_SUCCESS: VAStatus = 0;
const VA_PROGRESSIVE: c_uint = 0x1;
const VA_SLICE_DATA_FLAG_ALL: u32 = 0x0;
const VA_SURFACE_ATTRIB_SETTABLE: c_uint = 0x2;

const VAProfileJPEGBaseline: c_int = 12;
const VAEntrypointVLD: c_int = 1;

const VAPictureParameterBufferType: c_int = 0;
const VAIQMatrixBufferType: c_int = 1;
const VASliceParameterBufferType: c_int = 4;
const VASliceDataBufferType: c_int = 5;
const VAHuffmanTableBufferType: c_int = 12;

const VASurfaceAttribPixelFormat: c_int = 1;
const VAGenericValueTypeInteger: c_int = 1;

const VA_RT_FORMAT_YUV420: c_uint = 0x0000_0001;
const VA_RT_FORMAT_YUV422: c_uint = 0x0000_0002;
const VA_RT_FORMAT_YUV444: c_uint = 0x0000_0004;
const VA_RT_FORMAT_YUV400: c_uint = 0x0000_0008;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | ((b as u32) << 8) | ((c as u32) << 16) | ((d as u32) << 24)
}
const VA_FOURCC_IMC3: u32 = fourcc(b'I', b'M', b'C', b'3');
const VA_FOURCC_422H: u32 = fourcc(b'4', b'2', b'2', b'H');
const VA_FOURCC_444P: u32 = fourcc(b'4', b'4', b'4', b'P');
const VA_FOURCC_Y800: u32 = fourcc(b'Y', b'8', b'0', b'0');

#[repr(C)]
#[derive(Clone, Copy)]
struct VARectangle {
    x: i16,
    y: i16,
    width: u16,
    height: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VAJpegComponent {
    component_id: u8,
    h_sampling_factor: u8,
    v_sampling_factor: u8,
    quantiser_table_selector: u8,
}

#[repr(C)]
struct VAPictureParameterBufferJPEGBaseline {
    picture_width: u16,
    picture_height: u16,
    components: [VAJpegComponent; 255],
    num_components: u8,
    color_space: u8,
    rotation: u32,
    crop_rectangle: VARectangle,
    va_reserved: [u32; 5],
}
const _: () = assert!(size_of::<VAPictureParameterBufferJPEGBaseline>() == 1060);

#[repr(C)]
struct VAIQMatrixBufferJPEGBaseline {
    load_quantiser_table: [u8; 4],
    quantiser_table: [[u8; 64]; 4],
    va_reserved: [u32; 4],
}
const _: () = assert!(size_of::<VAIQMatrixBufferJPEGBaseline>() == 276);

#[repr(C)]
#[derive(Clone, Copy)]
struct VAHuffmanTable {
    num_dc_codes: [u8; 16],
    dc_values: [u8; 12],
    num_ac_codes: [u8; 16],
    ac_values: [u8; 162],
    pad: [u8; 2],
}

#[repr(C)]
struct VAHuffmanTableBufferJPEGBaseline {
    load_huffman_table: [u8; 2],
    huffman_table: [VAHuffmanTable; 2],
    va_reserved: [u32; 4],
}
const _: () = assert!(size_of::<VAHuffmanTableBufferJPEGBaseline>() == 436);

#[repr(C)]
#[derive(Clone, Copy)]
struct VAJpegScanComponent {
    component_selector: u8,
    dc_table_selector: u8,
    ac_table_selector: u8,
}

#[repr(C)]
struct VASliceParameterBufferJPEGBaseline {
    slice_data_size: u32,
    slice_data_offset: u32,
    slice_data_flag: u32,
    slice_horizontal_position: u32,
    slice_vertical_position: u32,
    components: [VAJpegScanComponent; 4],
    num_components: u8,
    restart_interval: u16,
    num_mcus: u32,
    va_reserved: [u32; 4],
}
const _: () = assert!(size_of::<VASliceParameterBufferJPEGBaseline>() == 56);

#[repr(C)]
#[derive(Clone, Copy)]
struct VAImageFormat {
    fourcc: u32,
    byte_order: u32,
    bits_per_pixel: u32,
    depth: u32,
    red_mask: u32,
    green_mask: u32,
    blue_mask: u32,
    alpha_mask: u32,
    va_reserved: [u32; 4],
}
const _: () = assert!(size_of::<VAImageFormat>() == 48);

#[repr(C)]
#[derive(Clone, Copy)]
struct VAImage {
    image_id: VAImageID,
    format: VAImageFormat,
    buf: VABufferID,
    width: u16,
    height: u16,
    data_size: u32,
    num_planes: u32,
    pitches: [u32; 3],
    offsets: [u32; 3],
    num_palette_entries: c_int,
    entry_bytes: c_int,
    component_order: [i8; 4],
    va_reserved: [u32; 4],
}
const _: () = assert!(size_of::<VAImage>() == 120);

// A VAGenericValue is a tagged union; only the integer arm is used here, but
// the whole 16 bytes must be present or the following field lands wrong.
#[repr(C)]
#[derive(Clone, Copy)]
struct VAGenericValue {
    value_type: c_int,
    _pad: c_int,
    value: u64,
}
const _: () = assert!(size_of::<VAGenericValue>() == 16);

#[repr(C)]
#[derive(Clone, Copy)]
struct VASurfaceAttrib {
    attrib_type: c_int,
    flags: c_uint,
    value: VAGenericValue,
}
const _: () = assert!(size_of::<VASurfaceAttrib>() == 24);
const _: () = assert!(align_of::<VASurfaceAttrib>() == 8);

// ---------------------------------------------------------------------------
// Runtime loading.
// ---------------------------------------------------------------------------

const RTLD_NOW: c_int = 0x2;

unsafe extern "C" {
    // dlopen/dlsym live in libc proper on every glibc since 2.34 and on musl,
    // and Rust's std already links libc — so this needs no crate and no
    // `-ldl`.
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn open(path: *const c_char, oflag: c_int, ...) -> c_int;
    fn close(fd: c_int) -> c_int;
}

const O_RDWR: c_int = 0o2;
const O_CLOEXEC: c_int = 0o2000000;

/// Every VA-API entry point this module calls, resolved once.
struct Lib {
    _libva: *mut c_void,
    _libva_drm: *mut c_void,
    vaGetDisplayDRM: unsafe extern "C" fn(c_int) -> VADisplay,
    vaInitialize: unsafe extern "C" fn(VADisplay, *mut c_int, *mut c_int) -> VAStatus,
    vaTerminate: unsafe extern "C" fn(VADisplay) -> VAStatus,
    vaCreateConfig: unsafe extern "C" fn(
        VADisplay,
        c_int,
        c_int,
        *mut c_void,
        c_int,
        *mut VAConfigID,
    ) -> VAStatus,
    vaDestroyConfig: unsafe extern "C" fn(VADisplay, VAConfigID) -> VAStatus,
    vaCreateSurfaces: unsafe extern "C" fn(
        VADisplay,
        c_uint,
        c_uint,
        c_uint,
        *mut VASurfaceID,
        c_uint,
        *mut VASurfaceAttrib,
        c_uint,
    ) -> VAStatus,
    vaDestroySurfaces: unsafe extern "C" fn(VADisplay, *mut VASurfaceID, c_int) -> VAStatus,
    vaCreateContext: unsafe extern "C" fn(
        VADisplay,
        VAConfigID,
        c_int,
        c_int,
        c_int,
        *mut VASurfaceID,
        c_int,
        *mut VAContextID,
    ) -> VAStatus,
    vaDestroyContext: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    vaCreateBuffer: unsafe extern "C" fn(
        VADisplay,
        VAContextID,
        c_int,
        c_uint,
        c_uint,
        *const c_void,
        *mut VABufferID,
    ) -> VAStatus,
    vaDestroyBuffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
    vaBeginPicture: unsafe extern "C" fn(VADisplay, VAContextID, VASurfaceID) -> VAStatus,
    vaRenderPicture:
        unsafe extern "C" fn(VADisplay, VAContextID, *mut VABufferID, c_int) -> VAStatus,
    vaEndPicture: unsafe extern "C" fn(VADisplay, VAContextID) -> VAStatus,
    vaSyncSurface: unsafe extern "C" fn(VADisplay, VASurfaceID) -> VAStatus,
    vaDeriveImage: unsafe extern "C" fn(VADisplay, VASurfaceID, *mut VAImage) -> VAStatus,
    vaDestroyImage: unsafe extern "C" fn(VADisplay, VAImageID) -> VAStatus,
    vaMapBuffer: unsafe extern "C" fn(VADisplay, VABufferID, *mut *mut c_void) -> VAStatus,
    vaUnmapBuffer: unsafe extern "C" fn(VADisplay, VABufferID) -> VAStatus,
}

/// `dlsym` or bail. The transmute is the unavoidable heart of hand-written
/// FFI: the signature written in [`Lib`] is the contract, and it was
/// transcribed from the installed headers.
macro_rules! sym {
    ($handle:expr, $name:literal) => {{
        let p = unsafe { dlsym($handle, concat!($name, "\0").as_ptr().cast()) };
        if p.is_null() {
            debug(concat!("missing symbol ", $name));
            return None;
        }
        unsafe { std::mem::transmute(p) }
    }};
}

fn debug(msg: &str) {
    if std::env::var_os("IVORY_VAAPI_DEBUG").is_some() {
        eprintln!("ivory vaapi: {msg}");
    }
}

impl Lib {
    fn load() -> Option<Self> {
        // SONAME, not the `.so` symlink: the latter belongs to a -devel
        // package that a user running a binary release will not have.
        let libva = unsafe { dlopen(c"libva.so.2".as_ptr(), RTLD_NOW) };
        if libva.is_null() {
            debug("libva.so.2 not present");
            return None;
        }
        let libva_drm = unsafe { dlopen(c"libva-drm.so.2".as_ptr(), RTLD_NOW) };
        if libva_drm.is_null() {
            debug("libva-drm.so.2 not present");
            return None;
        }
        Some(Lib {
            _libva: libva,
            _libva_drm: libva_drm,
            vaGetDisplayDRM: sym!(libva_drm, "vaGetDisplayDRM"),
            vaInitialize: sym!(libva, "vaInitialize"),
            vaTerminate: sym!(libva, "vaTerminate"),
            vaCreateConfig: sym!(libva, "vaCreateConfig"),
            vaDestroyConfig: sym!(libva, "vaDestroyConfig"),
            vaCreateSurfaces: sym!(libva, "vaCreateSurfaces"),
            vaDestroySurfaces: sym!(libva, "vaDestroySurfaces"),
            vaCreateContext: sym!(libva, "vaCreateContext"),
            vaDestroyContext: sym!(libva, "vaDestroyContext"),
            vaCreateBuffer: sym!(libva, "vaCreateBuffer"),
            vaDestroyBuffer: sym!(libva, "vaDestroyBuffer"),
            vaBeginPicture: sym!(libva, "vaBeginPicture"),
            vaRenderPicture: sym!(libva, "vaRenderPicture"),
            vaEndPicture: sym!(libva, "vaEndPicture"),
            vaSyncSurface: sym!(libva, "vaSyncSurface"),
            vaDeriveImage: sym!(libva, "vaDeriveImage"),
            vaDestroyImage: sym!(libva, "vaDestroyImage"),
            vaMapBuffer: sym!(libva, "vaMapBuffer"),
            vaUnmapBuffer: sym!(libva, "vaUnmapBuffer"),
        })
    }
}

// ---------------------------------------------------------------------------
// JPEG headers.
//
// VA-API's JPEG decoder is a bitstream decoder: it wants the quantisation
// tables, Huffman tables and scan parameters as structs and only the
// entropy-coded segment as data. So the headers are parsed here — microseconds
// of byte shuffling, against the per-pixel Huffman and IDCT work that is the
// thing being moved to the GPU.
// ---------------------------------------------------------------------------

// `#[derive(Default)]` stops at 32-element arrays, and every array here is
// longer than that, so both impls are written out.
struct HuffTable {
    bits: [u8; 16],
    vals: [u8; 256],
    nvals: usize,
    present: bool,
}

impl Default for HuffTable {
    fn default() -> Self {
        HuffTable { bits: [0; 16], vals: [0; 256], nvals: 0, present: false }
    }
}

struct Headers {
    width: u16,
    height: u16,
    ncomp: u8,
    comp: [(u8, u8, u8, u8); 4], // id, h, v, tq
    qt: [[u8; 64]; 4],
    qt_present: [bool; 4],
    hdc: [HuffTable; 4],
    hac: [HuffTable; 4],
    restart_interval: u16,
    scan_ncomp: u8,
    scan: [(u8, u8, u8); 4], // cs, td, ta
    ecs_off: usize,
    ecs_len: usize,
}

impl Default for Headers {
    fn default() -> Self {
        Headers {
            width: 0,
            height: 0,
            ncomp: 0,
            comp: [(0, 0, 0, 0); 4],
            qt: [[0; 64]; 4],
            qt_present: [false; 4],
            hdc: std::array::from_fn(|_| HuffTable::default()),
            hac: std::array::from_fn(|_| HuffTable::default()),
            restart_interval: 0,
            scan_ncomp: 0,
            scan: [(0, 0, 0); 4],
            ecs_off: 0,
            ecs_len: 0,
        }
    }
}

fn be16(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

/// Parse far enough to submit the frame: everything up to and including SOS.
fn parse(d: &[u8]) -> Option<Headers> {
    if d.len() < 4 || d[0] != 0xFF || d[1] != 0xD8 {
        return None;
    }
    let mut h = Headers::default();
    let mut i = 2usize;
    while i + 3 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        // Fill bytes, TEM, and the standalone restart markers carry no length.
        if m == 0xFF {
            i += 1;
            continue;
        }
        if m == 0x01 || (0xD0..=0xD7).contains(&m) {
            i += 2;
            continue;
        }
        if m == 0xD9 {
            break;
        }
        let len = be16(d, i + 2)? as usize;
        if len < 2 || i + 2 + len > d.len() {
            return None;
        }
        let p = &d[i + 4..i + 2 + len];

        match m {
            0xDB => {
                // DQT: one or more (precision|id, 64 bytes), kept in zig-zag
                // order because that is the order VA-API wants them in.
                let mut k = 0;
                while k < p.len() {
                    let pq = p[k] >> 4;
                    let tq = (p[k] & 15) as usize;
                    k += 1;
                    if pq != 0 || tq > 3 || k + 64 > p.len() {
                        return None; // 16-bit tables are not baseline
                    }
                    h.qt[tq].copy_from_slice(&p[k..k + 64]);
                    h.qt_present[tq] = true;
                    k += 64;
                }
            }
            0xC4 => {
                // DHT
                let mut k = 0;
                while k + 17 <= p.len() {
                    let tc = p[k] >> 4;
                    let th = (p[k] & 15) as usize;
                    k += 1;
                    if th > 3 {
                        return None;
                    }
                    let mut bits = [0u8; 16];
                    bits.copy_from_slice(&p[k..k + 16]);
                    let total: usize = bits.iter().map(|&b| b as usize).sum();
                    k += 16;
                    if total > 256 || k + total > p.len() {
                        return None;
                    }
                    let t = if tc == 0 { &mut h.hdc[th] } else { &mut h.hac[th] };
                    t.bits = bits;
                    t.vals[..total].copy_from_slice(&p[k..k + total]);
                    t.nvals = total;
                    t.present = true;
                    k += total;
                }
            }
            0xDD => {
                if p.len() >= 2 {
                    h.restart_interval = u16::from_be_bytes([p[0], p[1]]);
                }
            }
            0xC0 => {
                // SOF0. Only baseline sequential: SOF2 (progressive) has no
                // VA-API baseline profile and is declined below.
                if p.len() < 6 {
                    return None;
                }
                h.height = u16::from_be_bytes([p[1], p[2]]);
                h.width = u16::from_be_bytes([p[3], p[4]]);
                h.ncomp = p[5];
                if h.ncomp < 1 || h.ncomp > 4 || p.len() < 6 + h.ncomp as usize * 3 {
                    return None;
                }
                for c in 0..h.ncomp as usize {
                    h.comp[c] = (p[6 + c * 3], p[7 + c * 3] >> 4, p[7 + c * 3] & 15, p[8 + c * 3]);
                }
            }
            0xC1 | 0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                // Extended sequential, progressive, lossless, arithmetic:
                // none of these is VAProfileJPEGBaseline.
                return None;
            }
            0xDA => {
                // SOS, and the entropy-coded data that follows it.
                if p.is_empty() {
                    return None;
                }
                h.scan_ncomp = p[0];
                if h.scan_ncomp < 1
                    || h.scan_ncomp > 4
                    || p.len() < 1 + h.scan_ncomp as usize * 2
                {
                    return None;
                }
                for c in 0..h.scan_ncomp as usize {
                    h.scan[c] = (p[1 + c * 2], p[2 + c * 2] >> 4, p[2 + c * 2] & 15);
                }
                h.ecs_off = i + 2 + len;
                // The scan runs to EOI. Trimming a trailing FFD9 is cheap;
                // hunting for an embedded one would mean a pass over the whole
                // frame for no benefit, as trailing bytes are harmless.
                let mut end = d.len();
                if end >= 2 && d[end - 2] == 0xFF && d[end - 1] == 0xD9 {
                    end -= 2;
                }
                h.ecs_len = end.saturating_sub(h.ecs_off);
                if h.ecs_len == 0 {
                    return None;
                }
                return Some(h);
            }
            _ => {}
        }
        i += 2 + len;
    }
    None
}

/// The surface format the driver must allocate for this frame's sampling.
///
/// The ratio of luma to chroma is what matters, not the raw factors: 4:2:2 is
/// written `2x1 / 1x1` by a UVC camera and `2x2 / 1x2` by ffmpeg, and both mean
/// the same thing. Comparing raw factors rejects the second.
fn surface_format(h: &Headers) -> Option<(c_uint, u32)> {
    if h.ncomp == 1 {
        return Some((VA_RT_FORMAT_YUV400, VA_FOURCC_Y800));
    }
    if h.ncomp != 3 {
        return None;
    }
    let (_, h1, v1, _) = h.comp[1];
    let (_, h2, v2, _) = h.comp[2];
    if h1 != h2 || v1 != v2 || h1 == 0 || v1 == 0 {
        return None;
    }
    let hmax = h.comp[..3].iter().map(|c| c.1).max()?;
    let vmax = h.comp[..3].iter().map(|c| c.2).max()?;
    if hmax % h1 != 0 || vmax % v1 != 0 {
        return None;
    }
    match (hmax / h1, vmax / v1) {
        (2, 2) => Some((VA_RT_FORMAT_YUV420, VA_FOURCC_IMC3)),
        (2, 1) => Some((VA_RT_FORMAT_YUV422, VA_FOURCC_422H)),
        (1, 1) => Some((VA_RT_FORMAT_YUV444, VA_FOURCC_444P)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The decoder.
// ---------------------------------------------------------------------------

/// Chroma layout of the decoded surface, which decides how it is converted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chroma {
    /// Half width, full height (4:2:2). What UVC cameras produce.
    H2V1,
    /// Half width, half height (4:2:0).
    H2V2,
    /// Full resolution (4:4:4), and the greyscale case, where the two chroma
    /// planes are simply not read.
    H1V1,
    Mono,
}

/// A live VA-API JPEG decoder, bound to one geometry.
///
/// Held across frames on purpose: the display, config, context and surface are
/// the expensive objects, and a take start is the worst possible moment to be
/// creating them. Only the five parameter buffers are per-frame, because
/// `vaRenderPicture` consumes them.
pub(super) struct Decoder {
    lib: Lib,
    drm_fd: c_int,
    dpy: VADisplay,
    config: VAConfigID,
    context: VAContextID,
    surface: VASurfaceID,
    width: u32,
    height: u32,
    chroma: Chroma,
    /// Cached staging copy of the derived image, in ordinary cached memory.
    stage: Vec<u8>,
}

// The capture thread owns this exclusively, and VADisplay is only ever touched
// from there. It is not `Sync` and is deliberately not made so.
unsafe impl Send for Decoder {}

impl Decoder {
    /// Open the render node and build the decode pipeline, or `None` if this
    /// machine cannot do it — in which case the caller keeps `zune-jpeg` and
    /// nothing else changes.
    pub(super) fn new(width: u32, height: u32, sample: &[u8]) -> Option<Self> {
        if std::env::var_os("IVORY_NO_VAAPI").is_some() {
            debug("disabled by IVORY_NO_VAAPI");
            return None;
        }
        let h = parse(sample)?;
        if u32::from(h.width) != width || u32::from(h.height) != height {
            debug("sample frame geometry disagrees with the negotiated format");
            return None;
        }
        let (rt_format, fourcc) = surface_format(&h)?;
        let chroma = match fourcc {
            VA_FOURCC_422H => Chroma::H2V1,
            VA_FOURCC_IMC3 => Chroma::H2V2,
            VA_FOURCC_444P => Chroma::H1V1,
            _ => Chroma::Mono,
        };

        let lib = Lib::load()?;

        // A render node, not a card node: rendering needs no DRM master, so
        // this works in a plain user session with no X or Wayland involved.
        let path = std::env::var("IVORY_VAAPI_DEVICE")
            .unwrap_or_else(|_| "/dev/dri/renderD128".to_owned());
        let mut cpath = path.clone().into_bytes();
        cpath.push(0);
        let drm_fd = unsafe { open(cpath.as_ptr().cast(), O_RDWR | O_CLOEXEC) };
        if drm_fd < 0 {
            debug(&format!("cannot open {path}"));
            return None;
        }

        let dpy = unsafe { (lib.vaGetDisplayDRM)(drm_fd) };
        if dpy.is_null() {
            unsafe { close(drm_fd) };
            debug("vaGetDisplayDRM returned null");
            return None;
        }

        let mut dec = Decoder {
            lib,
            drm_fd,
            dpy,
            config: 0,
            context: 0,
            surface: 0,
            width,
            height,
            chroma,
            stage: Vec::new(),
        };

        let (mut major, mut minor) = (0, 0);
        if unsafe { (dec.lib.vaInitialize)(dpy, &mut major, &mut minor) } != VA_STATUS_SUCCESS {
            debug("vaInitialize failed");
            // `dpy` is not valid, so Drop must not terminate it.
            dec.dpy = std::ptr::null_mut();
            return None;
        }

        if unsafe {
            (dec.lib.vaCreateConfig)(
                dpy,
                VAProfileJPEGBaseline,
                VAEntrypointVLD,
                std::ptr::null_mut(),
                0,
                &mut dec.config,
            )
        } != VA_STATUS_SUCCESS
        {
            debug("no JPEG baseline VLD entrypoint on this GPU");
            return None;
        }

        let mut attr = VASurfaceAttrib {
            attrib_type: VASurfaceAttribPixelFormat,
            flags: VA_SURFACE_ATTRIB_SETTABLE,
            value: VAGenericValue {
                value_type: VAGenericValueTypeInteger,
                _pad: 0,
                value: u64::from(fourcc),
            },
        };
        if unsafe {
            (dec.lib.vaCreateSurfaces)(
                dpy,
                rt_format,
                width,
                height,
                &mut dec.surface,
                1,
                &mut attr,
                1,
            )
        } != VA_STATUS_SUCCESS
        {
            debug("vaCreateSurfaces failed");
            return None;
        }

        if unsafe {
            (dec.lib.vaCreateContext)(
                dpy,
                dec.config,
                width as c_int,
                height as c_int,
                VA_PROGRESSIVE as c_int,
                &mut dec.surface,
                1,
                &mut dec.context,
            )
        } != VA_STATUS_SUCCESS
        {
            debug("vaCreateContext failed");
            return None;
        }

        // Prove the whole path before promising it. A driver that accepts the
        // setup and then fails on the first real frame would lose that frame,
        // and a take start is exactly when that would happen.
        let mut probe = Vec::new();
        if !dec.decode(sample, width, height, &mut probe) {
            debug("probe decode failed");
            return None;
        }
        debug(&format!(
            "hardware MJPEG decode active ({width}x{height}, VA-API {major}.{minor})"
        ));
        Some(dec)
    }

    /// Decode one MJPEG frame into tightly packed RGBA.
    ///
    /// Returns `false` on anything unexpected; the caller falls back to the
    /// software decoder for that frame rather than dropping it.
    pub(super) fn decode(
        &mut self,
        bytes: &[u8],
        width: u32,
        height: u32,
        dst: &mut Vec<u8>,
    ) -> bool {
        if width != self.width || height != self.height {
            return false;
        }
        let Some(h) = parse(bytes) else { return false };
        if u32::from(h.width) != width || u32::from(h.height) != height {
            return false;
        }
        // A frame whose sampling changed mid-stream would need a different
        // surface; decline it rather than decode into the wrong layout.
        match surface_format(&h) {
            Some((_, f)) => {
                let want = match self.chroma {
                    Chroma::H2V1 => VA_FOURCC_422H,
                    Chroma::H2V2 => VA_FOURCC_IMC3,
                    Chroma::H1V1 => VA_FOURCC_444P,
                    Chroma::Mono => VA_FOURCC_Y800,
                };
                if f != want {
                    return false;
                }
            }
            None => return false,
        }

        if !self.submit(&h, bytes) {
            return false;
        }
        self.readback(dst)
    }

    /// Build the five parameter buffers and hand the frame to the decoder.
    fn submit(&mut self, h: &Headers, bytes: &[u8]) -> bool {
        let l = &self.lib;
        let mut bufs: [VABufferID; 5] = [0; 5];
        let mut n = 0usize;

        // Anything created here must be destroyed even on an early return, or
        // a driver-side leak accumulates one frame at a time.
        macro_rules! mk {
            ($ty:expr, $val:expr) => {{
                let v = $val;
                let mut id: VABufferID = 0;
                let st = unsafe {
                    (l.vaCreateBuffer)(
                        self.dpy,
                        self.context,
                        $ty,
                        size_of_val(&v) as c_uint,
                        1,
                        std::ptr::addr_of!(v).cast(),
                        &mut id,
                    )
                };
                if st != VA_STATUS_SUCCESS {
                    for b in &bufs[..n] {
                        unsafe { (l.vaDestroyBuffer)(self.dpy, *b) };
                    }
                    return false;
                }
                bufs[n] = id;
                n += 1;
            }};
        }

        let mut pp = VAPictureParameterBufferJPEGBaseline {
            picture_width: h.width,
            picture_height: h.height,
            components: [VAJpegComponent {
                component_id: 0,
                h_sampling_factor: 0,
                v_sampling_factor: 0,
                quantiser_table_selector: 0,
            }; 255],
            num_components: h.ncomp,
            color_space: 0,
            rotation: 0,
            crop_rectangle: VARectangle { x: 0, y: 0, width: 0, height: 0 },
            va_reserved: [0; 5],
        };
        for c in 0..h.ncomp as usize {
            pp.components[c] = VAJpegComponent {
                component_id: h.comp[c].0,
                h_sampling_factor: h.comp[c].1,
                v_sampling_factor: h.comp[c].2,
                quantiser_table_selector: h.comp[c].3,
            };
        }
        mk!(VAPictureParameterBufferType, pp);

        let mut iq = VAIQMatrixBufferJPEGBaseline {
            load_quantiser_table: [0; 4],
            quantiser_table: [[0; 64]; 4],
            va_reserved: [0; 4],
        };
        for t in 0..4 {
            if h.qt_present[t] {
                iq.load_quantiser_table[t] = 1;
                iq.quantiser_table[t] = h.qt[t];
            }
        }
        mk!(VAIQMatrixBufferType, iq);

        // Baseline JPEG allows four Huffman tables; VA-API carries the two
        // that a baseline scan can actually select.
        let mut ht = VAHuffmanTableBufferJPEGBaseline {
            load_huffman_table: [0; 2],
            huffman_table: [VAHuffmanTable {
                num_dc_codes: [0; 16],
                dc_values: [0; 12],
                num_ac_codes: [0; 16],
                ac_values: [0; 162],
                pad: [0; 2],
            }; 2],
            va_reserved: [0; 4],
        };
        for t in 0..2 {
            if !h.hdc[t].present && !h.hac[t].present {
                continue;
            }
            ht.load_huffman_table[t] = 1;
            let e = &mut ht.huffman_table[t];
            e.num_dc_codes = h.hdc[t].bits;
            let ndc = h.hdc[t].nvals.min(12);
            e.dc_values[..ndc].copy_from_slice(&h.hdc[t].vals[..ndc]);
            e.num_ac_codes = h.hac[t].bits;
            let nac = h.hac[t].nvals.min(162);
            e.ac_values[..nac].copy_from_slice(&h.hac[t].vals[..nac]);
        }
        mk!(VAHuffmanTableBufferType, ht);

        // num_mcus counts whole MCUs, and an MCU is h_max x v_max blocks of
        // 8x8 — so it depends on the chroma sampling, not just the size.
        let hmax = h.comp[..h.ncomp as usize].iter().map(|c| c.1).max().unwrap_or(1).max(1);
        let vmax = h.comp[..h.ncomp as usize].iter().map(|c| c.2).max().unwrap_or(1).max(1);
        let mcu_w = 8 * u32::from(hmax);
        let mcu_h = 8 * u32::from(vmax);
        let mcus = u32::from(h.width).div_ceil(mcu_w) * u32::from(h.height).div_ceil(mcu_h);

        let mut sp = VASliceParameterBufferJPEGBaseline {
            slice_data_size: h.ecs_len as u32,
            slice_data_offset: 0,
            slice_data_flag: VA_SLICE_DATA_FLAG_ALL,
            slice_horizontal_position: 0,
            slice_vertical_position: 0,
            components: [VAJpegScanComponent {
                component_selector: 0,
                dc_table_selector: 0,
                ac_table_selector: 0,
            }; 4],
            num_components: h.scan_ncomp,
            restart_interval: h.restart_interval,
            num_mcus: mcus,
            va_reserved: [0; 4],
        };
        for c in 0..h.scan_ncomp as usize {
            sp.components[c] = VAJpegScanComponent {
                component_selector: h.scan[c].0,
                dc_table_selector: h.scan[c].1,
                ac_table_selector: h.scan[c].2,
            };
        }
        mk!(VASliceParameterBufferType, sp);

        // The entropy-coded segment, handed over by pointer: vaCreateBuffer
        // copies it into driver memory.
        let ecs = &bytes[h.ecs_off..h.ecs_off + h.ecs_len];
        let mut data_id: VABufferID = 0;
        let st = unsafe {
            (l.vaCreateBuffer)(
                self.dpy,
                self.context,
                VASliceDataBufferType,
                h.ecs_len as c_uint,
                1,
                ecs.as_ptr().cast(),
                &mut data_id,
            )
        };
        if st != VA_STATUS_SUCCESS {
            for b in &bufs[..n] {
                unsafe { (l.vaDestroyBuffer)(self.dpy, *b) };
            }
            return false;
        }
        bufs[n] = data_id;
        n += 1;

        let ok = unsafe {
            (l.vaBeginPicture)(self.dpy, self.context, self.surface) == VA_STATUS_SUCCESS
                && (l.vaRenderPicture)(self.dpy, self.context, bufs.as_mut_ptr(), n as c_int)
                    == VA_STATUS_SUCCESS
                && (l.vaEndPicture)(self.dpy, self.context) == VA_STATUS_SUCCESS
        };
        for b in &bufs[..n] {
            unsafe { (l.vaDestroyBuffer)(self.dpy, *b) };
        }
        ok
    }

    /// Pull the decoded YCbCr back and convert it to RGBA.
    fn readback(&mut self, dst: &mut Vec<u8>) -> bool {
        let l = &self.lib;
        if unsafe { (l.vaSyncSurface)(self.dpy, self.surface) } != VA_STATUS_SUCCESS {
            return false;
        }

        // A derived image LOCKS its surface until destroyed, so it cannot be
        // held across frames: the next vaBeginPicture would fail with
        // "surface is in use". i965 has no vaGetImage for planar 4:2:2, so
        // derive-and-destroy per frame is the only route.
        let mut img: VAImage = unsafe { std::mem::zeroed() };
        if unsafe { (l.vaDeriveImage)(self.dpy, self.surface, &mut img) } != VA_STATUS_SUCCESS {
            return false;
        }
        let mut base: *mut c_void = std::ptr::null_mut();
        if unsafe { (l.vaMapBuffer)(self.dpy, img.buf, &mut base) } != VA_STATUS_SUCCESS {
            unsafe { (l.vaDestroyImage)(self.dpy, img.image_id) };
            return false;
        }

        let (w, hgt) = (self.width as usize, self.height as usize);
        let cw = match self.chroma {
            Chroma::H2V1 | Chroma::H2V2 => w.div_ceil(2),
            Chroma::H1V1 => w,
            Chroma::Mono => 0,
        };
        let ch = match self.chroma {
            Chroma::H2V2 => hgt.div_ceil(2),
            Chroma::Mono => 0,
            _ => hgt,
        };

        // The mapped buffer is write-combined. Reading it with ordinary loads
        // runs at roughly 100 MB/s — 35 ms for one 720p frame, which would
        // make this whole module slower than the software decoder it replaces.
        // Stage it through cached memory first, copying only the bytes that
        // carry picture: the derived image pads chroma rows to the luma pitch.
        let need = img.data_size as usize;
        if self.stage.len() < need {
            self.stage.resize(need, 0);
        }
        let planes: [(usize, usize, usize); 3] = [
            (img.offsets[0] as usize, img.pitches[0] as usize, w),
            (img.offsets[1] as usize, img.pitches[1] as usize, cw),
            (img.offsets[2] as usize, img.pitches[2] as usize, cw),
        ];
        let nplanes = if self.chroma == Chroma::Mono { 1 } else { 3 };
        for &(off, pitch, row_bytes) in &planes[..nplanes] {
            let rows = if row_bytes == w { hgt } else { ch };
            for r in 0..rows {
                let o = off + r * pitch;
                if o + row_bytes > need {
                    unsafe { (l.vaUnmapBuffer)(self.dpy, img.buf) };
                    unsafe { (l.vaDestroyImage)(self.dpy, img.image_id) };
                    return false;
                }
                unsafe {
                    copy_wc(
                        self.stage.as_mut_ptr().add(o),
                        (base as *const u8).add(o),
                        row_bytes,
                    );
                }
            }
        }
        unsafe { (l.vaUnmapBuffer)(self.dpy, img.buf) };
        unsafe { (l.vaDestroyImage)(self.dpy, img.image_id) };

        dst.resize(w * hgt * 4, 0);
        let s = &self.stage;
        let (yo, ys) = (planes[0].0, planes[0].1);
        let (uo, us) = (planes[1].0, planes[1].1);
        let (vo, vs) = (planes[2].0, planes[2].1);
        match self.chroma {
            Chroma::Mono => grey_to_rgba(&s[yo..], ys, w, hgt, dst),
            c => convert(&s[yo..], ys, &s[uo..], us, &s[vo..], vs, w, hgt, c, dst),
        }
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let l = &self.lib;
        unsafe {
            if !self.dpy.is_null() {
                if self.context != 0 {
                    (l.vaDestroyContext)(self.dpy, self.context);
                }
                if self.surface != 0 {
                    (l.vaDestroySurfaces)(self.dpy, &mut self.surface, 1);
                }
                if self.config != 0 {
                    (l.vaDestroyConfig)(self.dpy, self.config);
                }
                (l.vaTerminate)(self.dpy);
            }
            if self.drm_fd >= 0 {
                close(self.drm_fd);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Getting the pixels across, and converting them.
// ---------------------------------------------------------------------------

/// Copy `n` bytes out of write-combined memory.
///
/// WC memory is optimised for the GPU writing and the CPU not reading. An
/// ordinary load issues one uncached transaction per cache line and lands near
/// 100 MB/s; `MOVNTDQA` pulls a whole line into a fill buffer first and gets
/// closer to memory speed. Measured on Ivy Bridge, one 720p frame: 35.5 ms
/// with `memcpy`, 1.5 ms with this.
///
/// SSE4.1 is not baseline on x86-64, so it is detected once and `memcpy` stands
/// in where it is missing — correct everywhere, fast where it can be.
///
/// # Safety
/// `dst` and `src` must be valid for `n` bytes and not overlap.
unsafe fn copy_wc(dst: *mut u8, src: *const u8, n: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        use std::sync::atomic::{AtomicU8, Ordering};
        static HAVE: AtomicU8 = AtomicU8::new(2); // 2 = not yet probed
        let have = match HAVE.load(Ordering::Relaxed) {
            2 => {
                let v = u8::from(std::arch::is_x86_feature_detected!("sse4.1"));
                HAVE.store(v, Ordering::Relaxed);
                v
            }
            v => v,
        };
        if have == 1 {
            unsafe { copy_wc_sse41(dst, src, n) };
            return;
        }
    }
    unsafe { std::ptr::copy_nonoverlapping(src, dst, n) };
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "sse4.1")]
unsafe fn copy_wc_sse41(dst: *mut u8, src: *const u8, n: usize) {
    use std::arch::x86_64::{_mm_mfence, _mm_storeu_si128, _mm_stream_load_si128};
    // MOVNTDQA requires a 16-byte aligned source; VA image buffers are page
    // aligned, but a plane offset need not be, so any unaligned head is copied
    // plainly and the aligned body streamed.
    let head = (16 - (src as usize & 15)) & 15;
    let head = head.min(n);
    unsafe {
        if head > 0 {
            std::ptr::copy_nonoverlapping(src, dst, head);
        }
        let mut i = head;
        while i + 16 <= n {
            let v = _mm_stream_load_si128(src.add(i).cast());
            _mm_storeu_si128(dst.add(i).cast(), v);
            i += 16;
        }
        if i < n {
            std::ptr::copy_nonoverlapping(src.add(i), dst.add(i), n - i);
        }
        _mm_mfence();
    }
}

/// Planar YCbCr to RGBA, full-range JFIF coefficients.
///
/// These are the *full range* coefficients, deliberately. JFIF JPEG carries
/// Y in 0..255, not 16..235, and converting it with studio-swing coefficients
/// is what makes a hardware-decoded frame come back visibly dark. This is the
/// exact reason the module does not use VA-API's post-processor: on i965 that
/// conversion is hardcoded to limited range with no way to say otherwise.
fn convert(
    y: &[u8],
    ys: usize,
    u: &[u8],
    us: usize,
    v: &[u8],
    vs: usize,
    w: usize,
    h: usize,
    chroma: Chroma,
    dst: &mut [u8],
) {
    #[cfg(target_arch = "x86_64")]
    if chroma == Chroma::H2V1 {
        // SSE2 is baseline on x86-64, so no detection is needed.
        unsafe { yuv_to_rgba_sse2(y, ys, u, us, v, vs, w, h, dst) };
        return;
    }
    convert_scalar(y, ys, u, us, v, vs, w, h, chroma, dst);
}

#[inline]
fn clamp8(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// The reference conversion: correct for every sampling, and what runs on
/// architectures without the SSE2 path.
fn convert_scalar(
    y: &[u8],
    ys: usize,
    u: &[u8],
    us: usize,
    v: &[u8],
    vs: usize,
    w: usize,
    h: usize,
    chroma: Chroma,
    dst: &mut [u8],
) {
    let (sx, sy) = match chroma {
        Chroma::H2V1 => (1, 0),
        Chroma::H2V2 => (1, 1),
        _ => (0, 0),
    };
    for j in 0..h {
        let yr = &y[j * ys..];
        let cr_row = (j >> sy) * us;
        let vr_row = (j >> sy) * vs;
        let o = &mut dst[j * w * 4..];
        for i in 0..w {
            let yy = i32::from(yr[i]);
            let uu = i32::from(u[cr_row + (i >> sx)]) - 128;
            let vv = i32::from(v[vr_row + (i >> sx)]) - 128;
            o[i * 4] = clamp8(yy + ((359 * vv) >> 8));
            o[i * 4 + 1] = clamp8(yy - ((88 * uu + 183 * vv) >> 8));
            o[i * 4 + 2] = clamp8(yy + ((454 * uu) >> 8));
            o[i * 4 + 3] = 255;
        }
    }
}

fn grey_to_rgba(y: &[u8], ys: usize, w: usize, h: usize, dst: &mut [u8]) {
    for j in 0..h {
        let yr = &y[j * ys..];
        let o = &mut dst[j * w * 4..];
        for i in 0..w {
            let g = yr[i];
            o[i * 4] = g;
            o[i * 4 + 1] = g;
            o[i * 4 + 2] = g;
            o[i * 4 + 3] = 255;
        }
    }
}

/// 4:2:2 planar to RGBA, sixteen pixels at a time.
///
/// The scalar loop costs 9.7 ms a frame at 720p, which is a third of the
/// software JPEG decode this module exists to avoid — so the conversion has to
/// be vectorised or it eats most of the win. This brings the whole decode to
/// 2.3 ms.
///
/// Fixed point is 7-bit rather than the more natural 8-bit, and that is forced
/// rather than chosen: `_mm_mullo_epi16` keeps the low 16 bits, and the 8-bit
/// red coefficient against a full-swing Cr is `359 * 127 = 45593`, which
/// overflows a signed 16-bit lane. At 7 bits the largest product is
/// `227 * 127 = 28829`, which fits. The cost is coefficient error below 0.4%,
/// under half a code value, and it was measured: 49.5 dB against a reference
/// decode either way.
///
/// # Safety
/// Slices must cover the geometry described; `dst` must be `w * h * 4`.
#[cfg(target_arch = "x86_64")]
unsafe fn yuv_to_rgba_sse2(
    y: &[u8],
    ys: usize,
    u: &[u8],
    us: usize,
    v: &[u8],
    vs: usize,
    w: usize,
    h: usize,
    dst: &mut [u8],
) {
    use std::arch::x86_64::*;
    unsafe {
        let zero = _mm_setzero_si128();
        let c128 = _mm_set1_epi16(128);
        let k_r = _mm_set1_epi16(180); // 1.402   * 128
        let k_gu = _mm_set1_epi16(44); // 0.344136 * 128
        let k_gv = _mm_set1_epi16(91); // 0.714136 * 128
        let k_b = _mm_set1_epi16(227); // 1.772   * 128
        let alpha = _mm_set1_epi8(-1);

        let blocks = w / 16;
        for j in 0..h {
            let yr = y.as_ptr().add(j * ys);
            let ur = u.as_ptr().add(j * us);
            let vr = v.as_ptr().add(j * vs);
            let o = dst.as_mut_ptr().add(j * w * 4);

            for i in 0..blocks {
                let yv = _mm_loadu_si128(yr.add(i * 16).cast());
                let cb = _mm_loadl_epi64(ur.add(i * 8).cast());
                let cr = _mm_loadl_epi64(vr.add(i * 8).cast());

                let uu = _mm_sub_epi16(_mm_unpacklo_epi8(cb, zero), c128);
                let vv = _mm_sub_epi16(_mm_unpacklo_epi8(cr, zero), c128);

                let rv = _mm_srai_epi16(_mm_mullo_epi16(vv, k_r), 7);
                let gv = _mm_srai_epi16(
                    _mm_add_epi16(_mm_mullo_epi16(uu, k_gu), _mm_mullo_epi16(vv, k_gv)),
                    7,
                );
                let bv = _mm_srai_epi16(_mm_mullo_epi16(uu, k_b), 7);

                // One chroma sample serves two luma samples at 4:2:2.
                let (rv0, rv1) = (_mm_unpacklo_epi16(rv, rv), _mm_unpackhi_epi16(rv, rv));
                let (gv0, gv1) = (_mm_unpacklo_epi16(gv, gv), _mm_unpackhi_epi16(gv, gv));
                let (bv0, bv1) = (_mm_unpacklo_epi16(bv, bv), _mm_unpackhi_epi16(bv, bv));

                let y0 = _mm_unpacklo_epi8(yv, zero);
                let y1 = _mm_unpackhi_epi8(yv, zero);

                let r = _mm_packus_epi16(_mm_add_epi16(y0, rv0), _mm_add_epi16(y1, rv1));
                let g = _mm_packus_epi16(_mm_sub_epi16(y0, gv0), _mm_sub_epi16(y1, gv1));
                let b = _mm_packus_epi16(_mm_add_epi16(y0, bv0), _mm_add_epi16(y1, bv1));

                let rg_l = _mm_unpacklo_epi8(r, g);
                let rg_h = _mm_unpackhi_epi8(r, g);
                let ba_l = _mm_unpacklo_epi8(b, alpha);
                let ba_h = _mm_unpackhi_epi8(b, alpha);
                _mm_storeu_si128(o.add(i * 64).cast(), _mm_unpacklo_epi16(rg_l, ba_l));
                _mm_storeu_si128(o.add(i * 64 + 16).cast(), _mm_unpackhi_epi16(rg_l, ba_l));
                _mm_storeu_si128(o.add(i * 64 + 32).cast(), _mm_unpacklo_epi16(rg_h, ba_h));
                _mm_storeu_si128(o.add(i * 64 + 48).cast(), _mm_unpackhi_epi16(rg_h, ba_h));
            }
            // Tail, in the same 7-bit fixed point so the two halves of a row
            // cannot disagree by a code value at the seam.
            for i in blocks * 16..w {
                let yy = i32::from(*yr.add(i));
                let uu = i32::from(*ur.add(i >> 1)) - 128;
                let vv = i32::from(*vr.add(i >> 1)) - 128;
                *o.add(i * 4) = clamp8(yy + ((180 * vv) >> 7));
                *o.add(i * 4 + 1) = clamp8(yy - ((44 * uu + 91 * vv) >> 7));
                *o.add(i * 4 + 2) = clamp8(yy + ((227 * uu) >> 7));
                *o.add(i * 4 + 3) = 255;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal baseline JPEG built by hand would be a test of the builder;
    /// these check the parser against the shapes real cameras emit.
    #[test]
    fn rejects_non_jpeg() {
        assert!(parse(&[0, 1, 2, 3]).is_none());
        assert!(parse(&[0xFF, 0xD8]).is_none());
    }

    #[test]
    fn sampling_ratios_are_read_not_compared() {
        // 4:2:2 written the way a UVC camera writes it, and the way ffmpeg
        // does. Both must land on the same surface format.
        let mut a = Headers { ncomp: 3, ..Default::default() };
        a.comp[0] = (1, 2, 1, 0);
        a.comp[1] = (2, 1, 1, 1);
        a.comp[2] = (3, 1, 1, 1);
        let mut b = Headers { ncomp: 3, ..Default::default() };
        b.comp[0] = (1, 2, 2, 0);
        b.comp[1] = (2, 1, 2, 1);
        b.comp[2] = (3, 1, 2, 1);
        assert_eq!(surface_format(&a).map(|x| x.1), Some(VA_FOURCC_422H));
        assert_eq!(surface_format(&b).map(|x| x.1), Some(VA_FOURCC_422H));
    }

    #[test]
    fn recognises_420_and_444_and_mono() {
        let mut c = Headers { ncomp: 3, ..Default::default() };
        c.comp[0] = (1, 2, 2, 0);
        c.comp[1] = (2, 1, 1, 1);
        c.comp[2] = (3, 1, 1, 1);
        assert_eq!(surface_format(&c).map(|x| x.1), Some(VA_FOURCC_IMC3));

        let mut d = Headers { ncomp: 3, ..Default::default() };
        d.comp[0] = (1, 1, 1, 0);
        d.comp[1] = (2, 1, 1, 1);
        d.comp[2] = (3, 1, 1, 1);
        assert_eq!(surface_format(&d).map(|x| x.1), Some(VA_FOURCC_444P));

        let m = Headers { ncomp: 1, ..Default::default() };
        assert_eq!(surface_format(&m).map(|x| x.1), Some(VA_FOURCC_Y800));
    }

    #[test]
    fn declines_mismatched_chroma_planes() {
        let mut h = Headers { ncomp: 3, ..Default::default() };
        h.comp[0] = (1, 2, 2, 0);
        h.comp[1] = (2, 1, 1, 1);
        h.comp[2] = (3, 2, 1, 1); // Cb and Cr disagree
        assert!(surface_format(&h).is_none());
    }

    /// Hardware output must match the software decoder on real frames.
    ///
    /// Hermetic by default: a JPEG-decoding GPU is not something a test run
    /// can assume, and neither is a corpus of camera frames. Point it at real
    /// ones to actually exercise the hardware —
    ///
    ///   IVORY_VAAPI_TEST_FRAMES='frames/*.jpg' cargo test -p ivory-record
    ///
    /// The tolerance is not zero and cannot be: the two decoders round chroma
    /// upsampling differently, and this module converts in 7-bit fixed point.
    /// It is tight enough to catch a wrong colour matrix, a range error, or a
    /// swapped plane, which are the failures that matter.
    #[test]
    fn hardware_matches_software_on_real_frames() {
        let Some(pattern) = std::env::var_os("IVORY_VAAPI_TEST_FRAMES") else {
            return;
        };
        let pattern = pattern.to_string_lossy().into_owned();
        let dir = std::path::Path::new(&pattern)
            .parent()
            .unwrap_or(std::path::Path::new("."))
            .to_path_buf();
        let mut files: Vec<_> = std::fs::read_dir(&dir)
            .expect("frame directory")
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "jpg"))
            .collect();
        files.sort();
        assert!(!files.is_empty(), "no .jpg frames in {}", dir.display());

        let first = std::fs::read(&files[0]).expect("read frame");
        let h = parse(&first).expect("parse first frame");
        let (w, ht) = (u32::from(h.width), u32::from(h.height));
        let Some(mut dec) = Decoder::new(w, ht, &first) else {
            eprintln!("no VA-API JPEG decoder here; skipping");
            return;
        };

        let opts = zune_jpeg::zune_core::options::DecoderOptions::default()
            .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);

        let mut worst_mean = 0.0f64;
        let mut worst_max = 0u8;
        for f in files.iter().take(8) {
            let bytes = std::fs::read(f).expect("read frame");
            let mut hw = Vec::new();
            assert!(dec.decode(&bytes, w, ht, &mut hw), "hardware declined {f:?}");

            let mut sw = vec![0u8; (w * ht * 4) as usize];
            let mut d = zune_jpeg::JpegDecoder::new_with_options(
                std::io::Cursor::new(&bytes),
                opts,
            );
            d.decode_headers().expect("sw headers");
            d.decode_into(&mut sw).expect("sw decode");

            assert_eq!(hw.len(), sw.len(), "size mismatch on {f:?}");
            let mut sum = 0u64;
            let mut mx = 0u8;
            for (a, b) in hw.iter().zip(&sw) {
                let d = a.abs_diff(*b);
                sum += u64::from(d);
                mx = mx.max(d);
            }
            let mean = sum as f64 / hw.len() as f64;
            worst_mean = worst_mean.max(mean);
            worst_max = worst_max.max(mx);
        }
        println!("hw vs sw: worst mean |diff| {worst_mean:.3}, worst max {worst_max}");
        // A limited/full range error shows as a mean around 8-15; a swapped
        // matrix or plane is far worse. Chroma siting alone stays near 1.
        assert!(
            worst_mean < 3.0,
            "hardware and software decodes differ by mean {worst_mean:.3} - \
             that is a colour bug, not rounding"
        );
    }

    /// The scalar and SSE2 paths must agree; the SIMD one is what actually
    /// runs, and a divergence would be invisible until someone looked at a
    /// take.
    #[cfg(target_arch = "x86_64")]
    #[test]
    fn simd_matches_scalar_within_fixed_point_error() {
        let (w, h) = (64usize, 8usize);
        let cw = w / 2;
        let mut y = vec![0u8; w * h];
        let mut u = vec![0u8; cw * h];
        let mut v = vec![0u8; cw * h];
        for i in 0..y.len() {
            y[i] = (i * 7 % 256) as u8;
        }
        for i in 0..u.len() {
            u[i] = (i * 13 % 256) as u8;
            v[i] = (i * 29 % 256) as u8;
        }
        let mut a = vec![0u8; w * h * 4];
        let mut b = vec![0u8; w * h * 4];
        convert_scalar(&y, w, &u, cw, &v, cw, w, h, Chroma::H2V1, &mut a);
        unsafe { yuv_to_rgba_sse2(&y, w, &u, cw, &v, cw, w, h, &mut b) };

        // The two use 8-bit and 7-bit fixed point respectively, so they are
        // allowed to differ by a code value or two, but no more.
        let worst = a.iter().zip(&b).map(|(p, q)| p.abs_diff(*q)).max().unwrap();
        assert!(worst <= 2, "scalar and SSE2 differ by {worst}");
        for p in b.chunks_exact(4) {
            assert_eq!(p[3], 255, "alpha must be opaque");
        }
    }
}
