//! An OpenGL context of our own, for compositing on hardware that has no
//! Vulkan driver.
//!
//! # Why this exists at all
//!
//! [`Compositor::standalone`](super::Compositor::standalone) asks wgpu for an
//! adapter and, on a machine with no hardware Vulkan, gets mesa's lavapipe —
//! a CPU rasteriser. Every composited frame is then drawn by the same two
//! cores that are running the synth. Measured on a 2012 HD 4000 at 720p:
//! **217 ms of CPU per composited frame on lavapipe against 13.8 ms on the
//! GPU**, which is 326% of one core against 21%.
//!
//! That GPU has a perfectly good OpenGL driver. The app's own *window* is
//! already using it. Two separate things stopped the compositor from doing the
//! same, and this module exists because of the second one.
//!
//! # 1. wgpu cannot open a GL context on this driver, and that is a wgpu bug
//!
//! `wgpu-hal` 27 walks a ladder of context-robustness levels — core robust
//! access, then the EXT form, then none — and retries only when a rung fails
//! with `BAD_ATTRIBUTE`. Mesa's crocus answers the core attribute with
//! `BAD_ATTRIBUTE` but the EXT one with `BAD_CONFIG`, so wgpu stops one rung
//! above the level that works and reports no adapter at all. A plain context
//! with no robustness attribute succeeds on the same display, every time.
//!
//! Rather than carry a patched wgpu, this module creates the context itself —
//! at which point the ladder is not involved. (The bug is still worth fixing
//! upstream: it costs every driver that answers the EXT attribute with
//! anything other than `BAD_ATTRIBUTE` its hardware GL backend.)
//!
//! # 2. wgpu's own EGL handling would unbind the window's context
//!
//! This is the one that made the refusal in `standalone` correct rather than
//! merely cautious. wgpu-hal brackets its GL work like this:
//!
//! ```text
//! AdapterContext::lock()  -> eglMakeCurrent(display, pbuffer, pbuffer, ctx)
//! AdapterContextLock::drop -> eglMakeCurrent(display, NONE, NONE, NONE)
//! ```
//!
//! It does not *restore* what was current; it unbinds. The compositor runs on
//! the UI thread by design (it paints the app, and moving it would cost 250 MB
//! a second of frames crossing a channel), and EGL allows one current context
//! per thread — so after one composited frame the window's context is current
//! nowhere, and eframe's next `swapBuffers` fails on a surface bound to
//! nothing. `EglContext::make_current` unwraps, so that surfaces as a panic
//! mid-take.
//!
//! [`Adapter::new_external`][ext] sidesteps this completely: an adapter built
//! that way carries `egl: None`, so wgpu performs **no** currency management
//! and never touches the window's context. The bargain is that making the
//! context current becomes our job, which is what [`Gl::enter`] is for — and
//! it *restores* the previous context rather than unbinding, which is the
//! whole point.
//!
//! [ext]: wgpu::hal::gles::Adapter::new_external
//!
//! # Why `dlopen` rather than linking
//!
//! The same rule the camera's VA-API decoder follows: Tangent ships as a
//! tarball to machines whose graphics stack is unknown, and a binary that
//! refuses to start where `libEGL` is absent would be a poor trade for an
//! optimisation. Nothing here is linked; absence is [`Gl::create`] returning
//! `None` and the compositor taking the wgpu path it always took.

#![allow(non_camel_case_types, non_snake_case)]

use std::ffi::{c_char, c_int, c_uint, c_void};

// ---------------------------------------------------------------------------
// The slice of EGL this needs, transcribed by hand.
// ---------------------------------------------------------------------------

type EGLDisplay = *mut c_void;
type EGLConfig = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;
type EGLBoolean = c_uint;
type EGLenum = c_uint;
type EGLint = i32;

type MakeCurrentFn =
    unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean;
type GetErrorFn = unsafe extern "C" fn() -> EGLint;

const EGL_FALSE: EGLBoolean = 0;
const EGL_NO_CONTEXT: EGLContext = std::ptr::null_mut();
const EGL_NO_SURFACE: EGLSurface = std::ptr::null_mut();
const EGL_NO_DISPLAY: EGLDisplay = std::ptr::null_mut();
const EGL_DEFAULT_DISPLAY: *mut c_void = std::ptr::null_mut();

const EGL_NONE: EGLint = 0x3038;

const EGL_SURFACE_TYPE: EGLint = 0x3033;
const EGL_PBUFFER_BIT: EGLint = 0x0001;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_OPENGL_BIT: EGLint = 0x0008;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_GREEN_SIZE: EGLint = 0x3023;
const EGL_BLUE_SIZE: EGLint = 0x3022;
const EGL_ALPHA_SIZE: EGLint = 0x3021;
const EGL_WIDTH: EGLint = 0x3057;
const EGL_HEIGHT: EGLint = 0x3056;

const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;

const EGL_OPENGL_API: EGLenum = 0x30A2;
const EGL_OPENGL_ES_API: EGLenum = 0x30A0;

const EGL_EXTENSIONS: EGLint = 0x3055;
const EGL_DRAW: EGLint = 0x3059;
const EGL_READ: EGLint = 0x305A;

// GLX, because the window is very probably using it.
//
// **You cannot hold a GLX context and an EGL context current on the same
// thread.** Mesa answers `eglMakeCurrent` with `EGL_BAD_ACCESS` (0x3002) while
// a GLX context is current, and eframe's glutin picks GLX on X11 by default —
// `config: Glx(Config { .. })` in its own debug output. That is the real
// reason the compositor could not have a GL context of its own, and it is why
// wgpu's attempt produced `EGL_BAD_ACCESS` too.
//
// So the window's GLX context is released before ours is bound, and put back
// afterwards. On a display where the window uses EGL there is nothing current
// in GLX terms and all of this is a few cheap no-ops.
type GLXDrawable = std::ffi::c_ulong;
type GLXContext = *mut c_void;
type XDisplay = *mut c_void;

const RTLD_NOW: c_int = 0x2;

unsafe extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

/// The GLX entry points needed to step around the window's context.
#[derive(Clone, Copy)]
struct Glx {
    GetCurrentDisplay: unsafe extern "C" fn() -> XDisplay,
    GetCurrentContext: unsafe extern "C" fn() -> GLXContext,
    GetCurrentDrawable: unsafe extern "C" fn() -> GLXDrawable,
    GetCurrentReadDrawable: unsafe extern "C" fn() -> GLXDrawable,
    MakeContextCurrent:
        unsafe extern "C" fn(XDisplay, GLXDrawable, GLXDrawable, GLXContext) -> c_int,
}

impl Glx {
    /// `None` is ordinary: a machine with no GLX, or a window on EGL.
    fn load() -> Option<Self> {
        let lib = unsafe { dlopen(c"libGL.so.1".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            return None;
        }
        macro_rules! g {
            ($n:literal) => {{
                let p = unsafe { dlsym(lib, concat!($n, "\0").as_ptr().cast()) };
                if p.is_null() {
                    return None;
                }
                unsafe { std::mem::transmute(p) }
            }};
        }
        Some(Glx {
            GetCurrentDisplay: g!("glXGetCurrentDisplay"),
            GetCurrentContext: g!("glXGetCurrentContext"),
            GetCurrentDrawable: g!("glXGetCurrentDrawable"),
            GetCurrentReadDrawable: g!("glXGetCurrentReadDrawable"),
            MakeContextCurrent: g!("glXMakeContextCurrent"),
        })
    }

    /// What GLX has current on this thread right now.
    fn current(&self) -> GlxPrevious {
        unsafe {
            GlxPrevious {
                display: (self.GetCurrentDisplay)(),
                context: (self.GetCurrentContext)(),
                draw: (self.GetCurrentDrawable)(),
                read: (self.GetCurrentReadDrawable)(),
            }
        }
    }

    /// Let go of whatever is current, so EGL can bind on this thread.
    fn release(&self, prev: &GlxPrevious) {
        if !prev.context.is_null() && !prev.display.is_null() {
            unsafe { (self.MakeContextCurrent)(prev.display, 0, 0, std::ptr::null_mut()) };
        }
    }

    /// Put the window's context back exactly as it was.
    fn restore(&self, prev: &GlxPrevious) {
        if !prev.context.is_null() && !prev.display.is_null() {
            let ok = unsafe {
                (self.MakeContextCurrent)(prev.display, prev.draw, prev.read, prev.context)
            };
            if ok == 0 {
                log::error!(
                    "compositor GL: could not restore the window's GLX context - \
                     the window may stop drawing"
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
struct GlxPrevious {
    display: XDisplay,
    context: GLXContext,
    draw: GLXDrawable,
    read: GLXDrawable,
}

/// Every EGL entry point this module calls, resolved once.
///
/// `Copy` because it is nothing but function pointers and a library handle,
/// and the candidate loop in [`Gl::create_from`] needs it again after moving
/// one into a `Gl` that turned out not to work.
#[derive(Clone, Copy)]
struct Egl {
    _lib: *mut c_void,
    GetDisplay: unsafe extern "C" fn(*mut c_void) -> EGLDisplay,
    Initialize: unsafe extern "C" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean,
    Terminate: unsafe extern "C" fn(EGLDisplay) -> EGLBoolean,
    QueryString: unsafe extern "C" fn(EGLDisplay, EGLint) -> *const c_char,
    ChooseConfig: unsafe extern "C" fn(
        EGLDisplay,
        *const EGLint,
        *mut EGLConfig,
        EGLint,
        *mut EGLint,
    ) -> EGLBoolean,
    BindAPI: unsafe extern "C" fn(EGLenum) -> EGLBoolean,
    CreateContext:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext,
    DestroyContext: unsafe extern "C" fn(EGLDisplay, EGLContext) -> EGLBoolean,
    CreatePbufferSurface:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, *const EGLint) -> EGLSurface,
    DestroySurface: unsafe extern "C" fn(EGLDisplay, EGLSurface) -> EGLBoolean,
    MakeCurrent: MakeCurrentFn,
    GetCurrentContext: unsafe extern "C" fn() -> EGLContext,
    GetCurrentDisplay: unsafe extern "C" fn() -> EGLDisplay,
    GetCurrentSurface: unsafe extern "C" fn(EGLint) -> EGLSurface,
    GetProcAddress: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    GetError: GetErrorFn,
}

macro_rules! sym {
    ($handle:expr, $name:literal) => {{
        let p = unsafe { dlsym($handle, concat!($name, "\0").as_ptr().cast()) };
        if p.is_null() {
            log::debug!("compositor GL: libEGL has no {}", $name);
            return None;
        }
        unsafe { std::mem::transmute(p) }
    }};
}

impl Egl {
    fn load() -> Option<Self> {
        // SONAME, not the `.so` symlink: the latter belongs to a -devel
        // package a user running a binary release will not have.
        let lib = unsafe { dlopen(c"libEGL.so.1".as_ptr(), RTLD_NOW) };
        if lib.is_null() {
            log::debug!("compositor GL: libEGL.so.1 not present");
            return None;
        }
        Some(Egl {
            _lib: lib,
            GetDisplay: sym!(lib, "eglGetDisplay"),
            Initialize: sym!(lib, "eglInitialize"),
            Terminate: sym!(lib, "eglTerminate"),
            QueryString: sym!(lib, "eglQueryString"),
            ChooseConfig: sym!(lib, "eglChooseConfig"),
            BindAPI: sym!(lib, "eglBindAPI"),
            CreateContext: sym!(lib, "eglCreateContext"),
            DestroyContext: sym!(lib, "eglDestroyContext"),
            CreatePbufferSurface: sym!(lib, "eglCreatePbufferSurface"),
            DestroySurface: sym!(lib, "eglDestroySurface"),
            MakeCurrent: sym!(lib, "eglMakeCurrent"),
            GetCurrentContext: sym!(lib, "eglGetCurrentContext"),
            GetCurrentDisplay: sym!(lib, "eglGetCurrentDisplay"),
            GetCurrentSurface: sym!(lib, "eglGetCurrentSurface"),
            GetProcAddress: sym!(lib, "eglGetProcAddress"),
            GetError: sym!(lib, "eglGetError"),
        })
    }
}

// ---------------------------------------------------------------------------

/// Which client API the context ended up being.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Api {
    /// Desktop OpenGL. Preferred: it is what old Intel parts are best at, and
    /// it does not cap at the GLES feature set.
    OpenGl,
    /// OpenGL ES. What Gen7 tops out at (3.0 — 3.1 needs compute shaders it
    /// does not have) and what many ARM parts offer exclusively.
    Gles,
}

/// A context we own, on the current thread.
pub(super) struct Gl {
    egl: Egl,
    display: EGLDisplay,
    context: EGLContext,
    /// Only when the display cannot do surfaceless contexts.
    pbuffer: EGLSurface,
    /// Present when this machine has GLX, which is where the window's context
    /// most likely lives. See the note beside [`Glx`].
    glx: Option<Glx>,
    pub(super) api: Api,
    /// What to restore when this context is torn down, set by
    /// [`make_current_sticky`](Self::make_current_sticky).
    sticky: std::cell::Cell<Option<Previous>>,
    /// **Pins this to one thread.** An EGL context is current to at most one
    /// thread at a time, and every wgpu object built on it has the same rule,
    /// so a `Gl` that could be sent would be a bug the compiler could not see.
    /// A raw pointer is the ordinary way to say `!Send + !Sync` on stable.
    _not_send: std::marker::PhantomData<*const ()>,
}

impl Gl {
    /// Build a context, or `None` if this machine cannot give us one.
    ///
    /// `None` is not an error path — it is every machine without EGL, and
    /// every machine whose driver declines. The caller keeps wgpu.
    pub(super) fn create() -> Option<Self> {
        Self::create_from(0).map(|(gl, _)| gl)
    }

    /// The same, skipping the first `skip` candidates, and reporting which one
    /// it used.
    ///
/// The EGL display for a context that renders offscreen and owns itself.
///
/// **`eglGetDisplay(EGL_DEFAULT_DISPLAY)` asks mesa to GUESS**, from the
/// environment, which window system to attach to. Under X11 it sees `DISPLAY`
/// and guesses right. Under Wayland, with no `DISPLAY` set, it guesses wrong
/// and hands back nothing — and the compositor then falls all the way back to
/// wgpu's own adapter, which on this hardware is llvmpipe.
///
/// Measured on the 2013 Air, 60 frames at 1280x720, same binary minutes apart:
///
/// | session | platform          | path              | CPU/frame | at 15 fps |
/// |---------|-------------------|-------------------|-----------|-----------|
/// | X11     | guessed           | owned hardware GL |   8.17 ms |       12% |
/// | Wayland | guessed           | llvmpipe          | 205-337ms |  308-506% |
/// | Wayland | surfaceless       | owned hardware GL |   9.17 ms |       14% |
/// | X11     | surfaceless       | owned hardware GL |   6.33 ms |       10% |
///
/// So this was never a Wayland limitation: hardware GL is available there and
/// is if anything cheaper. It was one guess, made silently, costing every
/// Wayland user a factor of twenty-five.
///
/// **Surfaceless is what this context actually wants.** It renders to a texture
/// and has no business connecting to a compositor at all, so asking for a
/// platform with no window system is not a workaround — it is the honest
/// request, and it happens to be the fastest on both. The guess is kept as the
/// last resort, for a libEGL too old to offer the platform extension.
fn open_display(egl: &Egl) -> EGLDisplay {
    // EGL_MESA_platform_surfaceless. Also accepted by the EGL 1.5 core entry
    // point, which is why the EXT one is enough to ask for.
    const EGL_PLATFORM_SURFACELESS_MESA: EGLenum = 0x31DD;
    type GetPlatformDisplayExt =
        unsafe extern "C" fn(EGLenum, *mut c_void, *const EGLint) -> EGLDisplay;

    // A CLIENT extension: queried with no display, which is the one thing
    // `eglGetProcAddress` is allowed to do before `eglInitialize`.
    let p = unsafe { (egl.GetProcAddress)(c"eglGetPlatformDisplayEXT".as_ptr()) };
    if !p.is_null() {
        // SAFETY: the EGL spec fixes this signature for this name, and the
        // pointer came from `eglGetProcAddress` asked for exactly that name.
        let f: GetPlatformDisplayExt = unsafe { std::mem::transmute(p) };
        let d = unsafe {
            f(
                EGL_PLATFORM_SURFACELESS_MESA,
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        };
        if d != EGL_NO_DISPLAY {
            log::debug!("compositor GL: surfaceless EGL display");
            return d;
        }
        log::debug!("compositor GL: no surfaceless platform, falling back to the default");
    }
    unsafe { (egl.GetDisplay)(EGL_DEFAULT_DISPLAY) }
}

    /// **Creating a context is not the last thing that can fail.** wgpu opens a
    /// device on it afterwards, and that can be refused for reasons only
    /// visible then — a desktop GL 3.3 context is fine until wgpu tries to
    /// build its indirect-validation compute shader and finds GLSL 330 too old.
    /// The caller walks the candidates so one refusal is not the end of it.
    pub(super) fn create_from(skip: usize) -> Option<(Self, usize)> {
        if std::env::var_os("IVORY_NO_GL_COMPOSITE").is_some() {
            log::debug!("compositor GL: disabled by IVORY_NO_GL_COMPOSITE");
            return None;
        }
        let egl = Egl::load()?;
        let glx = Glx::load();

        let display = Self::open_display(&egl);
        if display == EGL_NO_DISPLAY {
            log::debug!("compositor GL: no EGL display");
            return None;
        }
        let (mut major, mut minor) = (0, 0);
        if unsafe { (egl.Initialize)(display, &mut major, &mut minor) } == EGL_FALSE {
            log::debug!("compositor GL: eglInitialize failed");
            return None;
        }

        let exts = unsafe { (egl.QueryString)(display, EGL_EXTENSIONS) };
        let exts = if exts.is_null() {
            String::new()
        } else {
            unsafe { std::ffi::CStr::from_ptr(exts) }
                .to_string_lossy()
                .into_owned()
        };
        // A context with no surface needs either EGL 1.5 or the KHR extension;
        // otherwise a 1x1 pbuffer stands in, which is what wgpu does too.
        let surfaceless =
            (major, minor) >= (1, 5) || exts.contains("EGL_KHR_surfaceless_context");

        // **GLES before desktop GL, and that ordering is load-bearing.**
        //
        // The obvious preference is desktop GL: it is what old Intel parts are
        // best at, and it does not cap at the GLES feature set. It also does
        // not work. wgpu-core builds an indirect-validation COMPUTE pipeline
        // when it opens a device, and mesa advertises `GL_ARB_compute_shader`
        // on this hardware — so wgpu tries, generates a shader that needs
        // GLSL 430, gets GLSL 330 from a 3.3 context, and the device creation
        // fails as `Device(Lost)`:
        //
        //   indirect-validation error: ComputePipeline(Internal(
        //       "The selected version doesn't support Features(DYNAMIC_ARRAY_SIZE)"))
        //
        // A GLES 3.0 context advertises no compute at all, so wgpu skips the
        // whole thing. It is also the context wgpu creates for itself on this
        // backend, which makes it the better-trodden path — and GLES is what
        // most of the old ARM hardware this is meant to help offers anyway.
        //
        // Desktop GL is still tried, after, for anything new enough to satisfy
        // the compute path and old enough to lack Vulkan.
        //
        // Every attempt omits any robustness attribute, which is the other
        // reason this module can get a context where wgpu cannot.
        let attempts: [(Api, EGLenum, EGLint, i32, i32); 4] = [
            (Api::Gles, EGL_OPENGL_ES_API, EGL_OPENGL_ES2_BIT, 3, 0),
            (Api::OpenGl, EGL_OPENGL_API, EGL_OPENGL_BIT, 4, 3),
            (Api::OpenGl, EGL_OPENGL_API, EGL_OPENGL_BIT, 3, 3),
            (Api::Gles, EGL_OPENGL_ES_API, EGL_OPENGL_ES2_BIT, 2, 0),
        ];

        for (idx, &(api, egl_api, renderable, maj, min)) in
            attempts.iter().enumerate().skip(skip)
        {
            let cfg_attrs = [
                EGL_SURFACE_TYPE,
                EGL_PBUFFER_BIT,
                EGL_RENDERABLE_TYPE,
                renderable,
                EGL_RED_SIZE,
                8,
                EGL_GREEN_SIZE,
                8,
                EGL_BLUE_SIZE,
                8,
                EGL_ALPHA_SIZE,
                8,
                EGL_NONE,
            ];
            let mut config: EGLConfig = std::ptr::null_mut();
            let mut n: EGLint = 0;
            let ok = unsafe {
                (egl.ChooseConfig)(display, cfg_attrs.as_ptr(), &mut config, 1, &mut n)
            };
            if ok == EGL_FALSE || n < 1 {
                continue;
            }
            if unsafe { (egl.BindAPI)(egl_api) } == EGL_FALSE {
                continue;
            }
            let ctx_attrs = [
                EGL_CONTEXT_MAJOR_VERSION,
                maj,
                EGL_CONTEXT_MINOR_VERSION,
                min,
                EGL_NONE,
            ];
            let context =
                unsafe { (egl.CreateContext)(display, config, EGL_NO_CONTEXT, ctx_attrs.as_ptr()) };
            if context == EGL_NO_CONTEXT {
                log::debug!(
                    "compositor GL: {api:?} {maj}.{min} declined (EGL 0x{:X})",
                    unsafe { (egl.GetError)() }
                );
                continue;
            }

            let pbuffer = if surfaceless {
                EGL_NO_SURFACE
            } else {
                let pb = [EGL_WIDTH, 1, EGL_HEIGHT, 1, EGL_NONE];
                let s = unsafe { (egl.CreatePbufferSurface)(display, config, pb.as_ptr()) };
                if s == EGL_NO_SURFACE {
                    unsafe { (egl.DestroyContext)(display, context) };
                    continue;
                }
                s
            };

            let gl = Gl {
                egl,
                display,
                context,
                pbuffer,
                api,
                glx,
                sticky: std::cell::Cell::new(None),
                _not_send: std::marker::PhantomData,
            };
            // Prove it can actually be made current before promising it. A
            // context that creates and will not bind is worse than none: the
            // failure would land on the first frame of a take.
            if gl.enter().is_none() {
                log::debug!("compositor GL: {api:?} {maj}.{min} would not bind");
                continue;
            }
            log::debug!("compositor GL: {api:?} {maj}.{min} context created");
            return Some((gl, idx));
        }
        log::debug!("compositor GL: no context on this display");
        None
    }

    /// This context's raw handle, for tests that need to prove which context
    /// is current.
    #[cfg(test)]
    pub(super) fn raw(&self) -> *mut c_void {
        self.context
    }

    /// Whatever context is current on this thread right now.
    #[cfg(test)]
    pub(super) fn current_raw(&self) -> *mut c_void {
        unsafe { (self.egl.GetCurrentContext)() }
    }

    /// The GL renderer string, read from this context.
    ///
    /// Enough to answer "is this hardware?" without building anything on top
    /// of the context — which matters, because standing a wgpu device up and
    /// tearing it down again at startup is what turned the window black, while
    /// creating and binding the bare context is demonstrably harmless.
    pub(super) fn renderer(&self) -> Option<String> {
        const GL_RENDERER: c_uint = 0x1F01;
        let p = self.proc_address("glGetString");
        if p.is_null() {
            return None;
        }
        // SAFETY: `glGetString` has this signature, and the caller holds the
        // context current through a `Guard`.
        let f: unsafe extern "C" fn(c_uint) -> *const c_char =
            unsafe { std::mem::transmute(p) };
        let s = unsafe { f(GL_RENDERER) };
        if s.is_null() {
            return None;
        }
        Some(unsafe { std::ffi::CStr::from_ptr(s) }.to_string_lossy().into_owned())
    }

    /// The address of a GL entry point, for glow's loader.
    pub(super) fn proc_address(&self, name: &str) -> *const c_void {
        let mut buf = Vec::with_capacity(name.len() + 1);
        buf.extend_from_slice(name.as_bytes());
        buf.push(0);
        unsafe { (self.egl.GetProcAddress)(buf.as_ptr().cast()) as *const c_void }
    }

    /// Make this context current, and restore whatever was current when the
    /// returned guard is dropped.
    ///
    /// **The restore is the reason this module exists.** wgpu's own EGL path
    /// unbinds instead, which leaves the window's context current on no thread
    /// and takes the app down on the next `swapBuffers`.
    pub(super) fn enter(&self) -> Option<Guard> {
        let e = &self.egl;
        // What to put back. A display of NO_DISPLAY means nothing was current,
        // and the restore is then an unbind, which is correct.
        let mut prev = Previous {
            display: unsafe { (e.GetCurrentDisplay)() },
            context: unsafe { (e.GetCurrentContext)() },
            draw: unsafe { (e.GetCurrentSurface)(EGL_DRAW) },
            read: unsafe { (e.GetCurrentSurface)(EGL_READ) },
            glx: None,
        };
        // **Let go of the window's GLX context first, or EGL will refuse.**
        // One thread cannot hold both; mesa answers EGL_BAD_ACCESS while a GLX
        // context is current, and glutin picks GLX on X11.
        if let Some(glx) = self.glx {
            let g = glx.current();
            if !g.context.is_null() {
                glx.release(&g);
                prev.glx = Some((glx, g));
            }
        }
        let ok = unsafe {
            (e.MakeCurrent)(self.display, self.pbuffer, self.pbuffer, self.context)
        };
        if ok == EGL_FALSE {
            log::debug!("compositor GL: eglMakeCurrent failed (0x{:X})", unsafe {
                (e.GetError)()
            });
            // Do not walk away holding the window's context hostage.
            if let Some((glx, g)) = prev.glx {
                glx.restore(&g);
            }
            return None;
        }
        Some(Guard {
            make_current: e.MakeCurrent,
            get_error: e.GetError,
            own_display: self.display,
            prev,
        })
    }

    /// Make this context current and leave it that way, remembering what to
    /// put back.
    ///
    /// For [`Drop`] only. A `Drop` impl runs *before* its struct's fields are
    /// dropped, so a guard taken there would be gone by the time the wgpu
    /// objects release their GPU resources — and those need the context. This
    /// stashes the restore instead, and [`Gl`]'s own `Drop`, which runs last,
    /// performs it.
    pub(super) fn make_current_sticky(&self) {
        let e = &self.egl;
        let mut prev = Previous {
            display: unsafe { (e.GetCurrentDisplay)() },
            context: unsafe { (e.GetCurrentContext)() },
            draw: unsafe { (e.GetCurrentSurface)(EGL_DRAW) },
            read: unsafe { (e.GetCurrentSurface)(EGL_READ) },
            glx: None,
        };
        if let Some(glx) = self.glx {
            let g = glx.current();
            if !g.context.is_null() {
                glx.release(&g);
                prev.glx = Some((glx, g));
            }
        }
        if unsafe { (e.MakeCurrent)(self.display, self.pbuffer, self.pbuffer, self.context) }
            != EGL_FALSE
        {
            self.sticky.set(Some(prev));
        } else if let Some((glx, g)) = prev.glx {
            glx.restore(&g);
        }
    }
}

impl Drop for Gl {
    fn drop(&mut self) {
        let e = &self.egl;
        unsafe {
            // **Only take the thread's context away if it is OURS.**
            //
            // `eglMakeCurrent(NONE)` clears whatever is current on this
            // thread, not merely our own binding — so unbinding
            // unconditionally destroys the window's context binding that a
            // `Guard` has just carefully put back. That is not theoretical: it
            // turned the app's window black at startup, permanently, because
            // `renders_on_the_cpu` builds a `Gl`, probes with it, and drops it
            // while the window's GLX context is current again.
            let sticky = self.sticky.get();
            // What GLX holds right now, so it can be bound again once our
            // context is gone. See the re-bind at the end of this block.
            let glx_now = self.glx.map(|g| (g, g.current()));
            if (e.GetCurrentContext)() == self.context {
                (e.MakeCurrent)(self.display, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            }
            if self.pbuffer != EGL_NO_SURFACE {
                (e.DestroySurface)(self.display, self.pbuffer);
            }
            (e.DestroyContext)(self.display, self.context);
            // Whatever was current when the compositor started tearing down.
            // Restored here, last of all, because the wgpu objects above had
            // to release their GPU resources while OUR context was current.
            if let Some(p) = sticky {
                p.restore(e.MakeCurrent, e.GetError, self.display);
            } else if let Some((g, prev)) = glx_now {
                // **Bind the window's GLX context again, even though it never
                // left.**
                //
                // Destroying an EGL context on a display GLX is also using
                // leaves the GLX drawable needing re-validation, and nothing
                // else will do it: glutin believes its context is current, so
                // it never calls makeCurrent again, and the window renders
                // into a drawable the driver has quietly stopped presenting.
                // The window goes black and stays black.
                //
                // This is measured, and from an asymmetry that took a real
                // take to find: the compositor's own context is destroyed the
                // same way at the end of every take and the window survives —
                // because `sticky` sends it through the branch above, which
                // re-binds. The startup probe had no sticky state, took this
                // branch, and did not.
                g.restore(&prev);
            }
            // Deliberately NOT eglTerminate: the display is process-wide and
            // shared with whatever is drawing the window. Terminating it would
            // pull the rug from under the very context this module exists to
            // protect.
            let _ = e.Terminate;
        }
    }
}

#[derive(Clone, Copy)]
struct Previous {
    display: EGLDisplay,
    context: EGLContext,
    draw: EGLSurface,
    read: EGLSurface,
    /// The window's GLX context, if it had one. Released while ours is bound.
    glx: Option<(Glx, GlxPrevious)>,
}

/// Holds our context current, and puts back the previous one on drop.
///
/// **Deliberately not borrowing [`Gl`].** The compositor needs `&mut self` for
/// the body of `frame()` while this is alive, so a guard that borrowed the
/// context would make the two mutually exclusive. Everything it needs is a
/// pointer or a plain function pointer, so it carries copies instead — and the
/// raw pointers keep it `!Send`, which is the property that actually matters.
pub(super) struct Guard {
    make_current: MakeCurrentFn,
    get_error: GetErrorFn,
    own_display: EGLDisplay,
    prev: Previous,
}

impl Previous {
    /// Put the thread back exactly as it was found: EGL first, because our
    /// context has to be off this thread before GLX will take it back.
    fn restore(&self, make_current: MakeCurrentFn, get_error: GetErrorFn, own: EGLDisplay) {
        unsafe {
            if self.display != EGL_NO_DISPLAY && self.context != EGL_NO_CONTEXT {
                if (make_current)(self.display, self.draw, self.read, self.context) == EGL_FALSE {
                    log::error!(
                        "compositor GL: could not restore the previous EGL context (0x{:X})",
                        (get_error)()
                    );
                }
            } else {
                (make_current)(own, EGL_NO_SURFACE, EGL_NO_SURFACE, EGL_NO_CONTEXT);
            }
        }
        if let Some((glx, g)) = self.glx {
            glx.restore(&g);
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.prev
            .restore(self.make_current, self.get_error, self.own_display);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GLX context on a pbuffer, standing in for the window's.
    ///
    /// **This is the shape of the bug the unit tests originally missed.**
    /// Everything here bound cleanly against an EGL stand-in, and then failed
    /// in the real app with `EGL_BAD_ACCESS` — because eframe's glutin picks
    /// GLX on X11, and one thread cannot hold a GLX context and an EGL context
    /// at the same time. A test with no GLX in it could never have found that.
    mod glxstub {
        use super::super::*;

        pub struct GlxCtx {
            pub display: XDisplay,
            pub context: GLXContext,
            pub drawable: GLXDrawable,
            make_current:
                unsafe extern "C" fn(XDisplay, GLXDrawable, GLXDrawable, GLXContext) -> c_int,
            get_current: unsafe extern "C" fn() -> GLXContext,
        }

        const GLX_DRAWABLE_TYPE: c_int = 0x8010;
        const GLX_PBUFFER_BIT: c_int = 0x0000_0004;
        const GLX_RENDER_TYPE: c_int = 0x8011;
        const GLX_RGBA_BIT: c_int = 0x0000_0001;
        const GLX_RGBA_TYPE: c_int = 0x8014;
        const GLX_PBUFFER_WIDTH: c_int = 0x8041;
        const GLX_PBUFFER_HEIGHT: c_int = 0x8040;

        /// `None` wherever GLX or X is unavailable, which is not a failure.
        pub fn create() -> Option<GlxCtx> {
            unsafe {
                let x = dlopen(c"libX11.so.6".as_ptr(), RTLD_NOW);
                let g = dlopen(c"libGL.so.1".as_ptr(), RTLD_NOW);
                if x.is_null() || g.is_null() {
                    return None;
                }
                macro_rules! sy {
                    ($h:expr, $n:literal) => {{
                        let p = dlsym($h, concat!($n, "\0").as_ptr().cast());
                        if p.is_null() {
                            return None;
                        }
                        std::mem::transmute(p)
                    }};
                }
                let open_display: unsafe extern "C" fn(*const c_char) -> XDisplay =
                    sy!(x, "XOpenDisplay");
                let default_screen: unsafe extern "C" fn(XDisplay) -> c_int =
                    sy!(x, "XDefaultScreen");
                let choose: unsafe extern "C" fn(
                    XDisplay,
                    c_int,
                    *const c_int,
                    *mut c_int,
                ) -> *mut *mut c_void = sy!(g, "glXChooseFBConfig");
                let create_pbuffer: unsafe extern "C" fn(
                    XDisplay,
                    *mut c_void,
                    *const c_int,
                ) -> GLXDrawable = sy!(g, "glXCreatePbuffer");
                let create_ctx: unsafe extern "C" fn(
                    XDisplay,
                    *mut c_void,
                    c_int,
                    GLXContext,
                    c_int,
                ) -> GLXContext = sy!(g, "glXCreateNewContext");
                let make_current: unsafe extern "C" fn(
                    XDisplay,
                    GLXDrawable,
                    GLXDrawable,
                    GLXContext,
                ) -> c_int = sy!(g, "glXMakeContextCurrent");
                let get_current: unsafe extern "C" fn() -> GLXContext =
                    sy!(g, "glXGetCurrentContext");

                let display = open_display(std::ptr::null());
                if display.is_null() {
                    return None;
                }
                let screen = default_screen(display);
                let attrs = [
                    GLX_DRAWABLE_TYPE,
                    GLX_PBUFFER_BIT,
                    GLX_RENDER_TYPE,
                    GLX_RGBA_BIT,
                    0,
                ];
                let mut n: c_int = 0;
                let configs = choose(display, screen, attrs.as_ptr(), &mut n);
                if configs.is_null() || n < 1 {
                    return None;
                }
                let cfg = *configs;
                let pb = [GLX_PBUFFER_WIDTH, 4, GLX_PBUFFER_HEIGHT, 4, 0];
                let drawable = create_pbuffer(display, cfg, pb.as_ptr());
                if drawable == 0 {
                    return None;
                }
                let context = create_ctx(display, cfg, GLX_RGBA_TYPE, std::ptr::null_mut(), 1);
                if context.is_null() {
                    return None;
                }
                if make_current(display, drawable, drawable, context) == 0 {
                    return None;
                }
                Some(GlxCtx { display, context, drawable, make_current, get_current })
            }
        }

        impl GlxCtx {
            pub fn is_current(&self) -> bool {
                unsafe { (self.get_current)() == self.context }
            }
            pub fn rebind(&self) -> bool {
                unsafe {
                    (self.make_current)(self.display, self.drawable, self.drawable, self.context)
                        != 0
                }
            }
        }
    }

    /// **The window is on GLX, and the compositor must work anyway.**
    ///
    /// With a GLX context current on this thread, `eglMakeCurrent` fails with
    /// `EGL_BAD_ACCESS` unless the GLX one is released first. This asserts both
    /// halves: that our context binds at all, and that the GLX context is back
    /// exactly as it was afterwards.
    #[test]
    fn a_context_binds_and_restores_around_a_live_glx_context() {
        let Some(glx) = glxstub::create() else {
            eprintln!("no GLX here; nothing to step around");
            return;
        };
        assert!(glx.is_current(), "the stand-in GLX context did not become current");

        let Some(gl) = Gl::create() else {
            eprintln!("no EGL here");
            return;
        };
        // Creating it probes a bind, which is where the real app failed.
        assert!(
            glx.is_current(),
            "creating the EGL context left the GLX context unbound"
        );

        {
            let _guard = gl.enter().expect(
                "could not bind an EGL context while GLX held the thread - \
                 this is the EGL_BAD_ACCESS the real app hit",
            );
            assert!(
                !glx.is_current(),
                "GLX was still current while ours was bound, which EGL forbids"
            );
        }
        assert!(
            glx.is_current(),
            "the window's GLX context was not restored - it would stop drawing"
        );
        assert!(glx.rebind(), "the GLX context is no longer usable");

        // **And dropping ours must not take the window's binding with it.**
        // `eglMakeCurrent(NONE)` clears whatever is current on the thread, not
        // just our own context — so a `Drop` that unbinds unconditionally
        // wipes the GLX binding a `Guard` has just restored. That is what
        // turned the real app's window black at startup, and nothing in this
        // file caught it until this assertion existed.
        drop(gl);
        assert!(
            glx.is_current(),
            "dropping the compositor's GL context cleared the window's GLX \
             context - the window would go black"
        );
    }

    /// Whatever was current before must still be current after, because the
    /// compositor shares a thread with the window that is drawing.
    #[test]
    fn entering_and_leaving_restores_the_previous_context() {
        let Some(gl) = Gl::create() else {
            eprintln!("no EGL here; nothing to check");
            return;
        };
        // Stand in for the window: bind our own context and call it "theirs",
        // then check a nested enter/leave puts it back.
        let outer = gl.enter().expect("bind once");
        let before = unsafe { (gl.egl.GetCurrentContext)() };
        assert_ne!(before, EGL_NO_CONTEXT, "nothing became current");
        {
            let _inner = gl.enter().expect("bind again");
        }
        let after = unsafe { (gl.egl.GetCurrentContext)() };
        assert_eq!(before, after, "the previous context was not restored");
        drop(outer);
    }

    #[test]
    fn a_context_can_resolve_gl_entry_points() {
        let Some(gl) = Gl::create() else {
            eprintln!("no EGL here; nothing to check");
            return;
        };
        let _guard = gl.enter().expect("bind");
        assert!(
            !gl.proc_address("glGetString").is_null(),
            "glGetString did not resolve, so glow would get an empty context"
        );
    }
}
