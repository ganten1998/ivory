//! Opening a hosted plugin's own editor, in a real window.
//!
//! `instance.rs` can make a plugin play. It could not make one *sound
//! different*, because every knob a Pianoteq owner wants is inside Pianoteq's
//! own UI and nothing here had ever drawn it. This is that UI.
//!
//! # The sequence, which is short and unforgiving
//!
//! ```text
//! IEditController::createView("editor")   the controller builds its UI object
//! isPlatformTypeSupported("NSView")       ask before assuming; refuse politely
//! getSize(&ViewRect)                      the size to make the window
//! setFrame(IPlugFrame)                    a HOST-implemented interface
//! attached(nsview, "NSView")              the plugin draws into OUR view
//!   ...
//! removed()                               BEFORE the view goes away
//! ```
//!
//! **`removed()` before the view is destroyed is the one that crashes.** The
//! plugin has put its own `NSView` inside ours and is holding a pointer to it;
//! releasing our view first hands it a dangling parent and the crash lands in
//! the plugin's drawing code, several seconds later, looking like a plugin bug.
//! [`Editor`]'s `Drop` does the whole order and is the only way the window ever
//! goes away.
//!
//! # Everything here is main-thread work
//!
//! VST3 says so for every method above, and AppKit is not negotiable about it.
//! The type system carries that: [`Editor`] holds `Retained<NSWindow>`, which is
//! `!Send`, so an editor cannot be moved off the thread that made it, and
//! [`Editor::open`] refuses without a `MainThreadMarker`.
//!
//! # Why a separate native window and not an egui panel
//!
//! An `egui` window is painted geometry inside one big GL surface. It has no
//! `NSView` to hand a plugin and cannot acquire one. So the editor gets an
//! `NSWindow` of its own, made with AppKit directly.
//!
//! **That does not need a second event loop, and the reason is worth stating:**
//! on macOS `winit`'s event loop *is* `NSApplication`'s run loop — it calls
//! `[NSApp run]` and services `NSEvent`s — so a window created by anyone in the
//! process is driven by it, gets its mouse and key events, and redraws, with no
//! cooperation from the app at all. Verified rather than assumed: an editor
//! opened from Tangent's menu is live while the egui window keeps painting, and
//! `pump` (below) exists **only** for processes that have no such loop.
//!
//! # Closing is not unloading
//!
//! The editor is a view onto a running instrument. Closing its window calls
//! `removed()` and destroys the window; the plugin keeps playing, keeps its
//! notes ringing, and can have its editor opened again. Nothing here touches
//! the audio path. The other direction is the caller's to enforce: **unloading
//! the plugin while an editor is open must drop the [`Editor`] first**, because
//! the view belongs to a controller that is about to be terminated.
//!
//! # The window the user cannot resize, and the plugin can
//!
//! The style mask deliberately omits `Resizable`. A user-dragged corner would
//! need `checkSizeConstraint`/`onSize` negotiation with the plugin on every
//! frame of the drag, and getting that wrong looks like a plugin that will not
//! lay out. The plugin's own path is implemented and is the one that matters:
//! [`IPlugFrame::resizeView`](vst3::Steinberg::IPlugFrameTrait::resizeView) is
//! how Pianoteq asks for a different size when you switch instrument layout or
//! change its zoom, and ignoring it leaves a plugin drawing outside its window.

use std::cell::Cell;
use std::time::Duration;

use vst3::ComPtr;
use vst3::Steinberg::Vst::{IEditController, IEditControllerTrait, ViewType};
use vst3::Steinberg::IPlugView;

use crate::instance::Instance;

#[cfg(target_os = "macos")]
use mac::Window as Inner;
#[cfg(not(target_os = "macos"))]
use stub::Window as Inner;

/// Why an editor could not be opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorError {
    /// This build has no window binding for plugin editors.
    ///
    /// Windows and Linux, today. The UI should grey the row rather than offer
    /// something that will fail — see [`Instance::has_editor`], which answers
    /// the same question without trying.
    Unsupported,
    /// The plugin builds no editor. Legal, and true of a few instruments.
    NoEditor,
    /// The plugin refuses the only window handle this platform has to offer.
    ///
    /// Carries the VST3 platform type that was asked for (`"NSView"`,
    /// `"HWND"`, `"X11EmbedWindowID"`). A plugin that says no here is one built
    /// for a different OS's UI toolkit and there is nothing to be done about it
    /// from this side.
    WrongPlatform(&'static str),
    /// [`Editor::open`] was called from a thread that is not the main thread.
    NotMainThread,
    /// A VST3 call in the sequence failed. Carries what and how.
    Refused(String),
}

impl std::fmt::Display for EditorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unsupported => {
                write!(f, "plugin editors are only implemented on macOS so far")
            }
            Self::NoEditor => write!(f, "this plugin has no editor"),
            Self::WrongPlatform(want) => {
                write!(f, "the plugin will not draw into a {want}")
            }
            Self::NotMainThread => {
                write!(f, "a plugin editor can only be opened on the main thread")
            }
            Self::Refused(why) => write!(f, "{why}"),
        }
    }
}

impl std::error::Error for EditorError {}

/// A main-thread reference to a loaded plugin's edit controller.
///
/// # What this is for
///
/// In the app the [`Instance`] is **inside the audio callback** by the time a
/// user asks for its editor. There is no `&Instance` to be had. A controller is
/// a reference-counted COM object, so a second reference can be taken while the
/// instance is still on this thread and kept after it leaves — which is exactly
/// what this is. See [`Instance::editor_handle`].
///
/// # `ComPtr` does NOT make this `!Send`, and that surprised this file
///
/// The `vst3` bindings carry `unsafe impl Send for I` and `unsafe impl Sync for
/// I` on **every one of their 126 interface types**, unconditionally, and
/// `ComPtr<I>` is `Send` whenever `I` is. So a bare `ComPtr<IEditController>` is
/// cheerfully `Send` and carries no thread information whatsoever.
///
/// That is worth knowing beyond this struct. [`Instance`] was `!Send` for years
/// **by accident** — not because of its `ComPtr`s, but because it happened to
/// hold a `ComWrapper<HostApp>` whose `Cell` counter made it non-`Sync`. That
/// counter became an atomic in the same change as this file (a component may
/// allocate a message from the audio thread), at which point `Instance` would
/// silently have become `Send` and the "VST3 main-thread methods are called
/// from one thread" rule the whole crate is built on would have gone
/// unenforced, with no error anywhere. It now carries an explicit marker too.
///
/// So the field below is not decoration. There is a test for each of them.
///
/// **Drop it before the [`Instance`] is dropped.** The instance's teardown calls
/// `IEditController::terminate`, and a handle that outlives that is a live
/// pointer into a terminated object.
pub struct EditorHandle {
    controller: ComPtr<IEditController>,
    /// Lazily probed, then remembered. See [`Instance::has_editor`] for why the
    /// only way to ask is to build one and throw it away.
    probed: Cell<Option<bool>>,
    /// Makes this `!Send` and `!Sync` on purpose. See above.
    not_send: std::marker::PhantomData<*const ()>,
}

impl EditorHandle {
    pub(crate) fn new(controller: ComPtr<IEditController>) -> Self {
        Self {
            controller,
            probed: Cell::new(None),
            not_send: std::marker::PhantomData,
        }
    }

    /// Whether this plugin offers an editor at all. Main thread only.
    ///
    /// The first call builds a view and releases it, because VST3 has no
    /// `hasEditor`; the answer is then remembered for the handle's life.
    pub fn has_editor(&self) -> bool {
        if let Some(known) = self.probed.get() {
            return known;
        }
        let answer = match self.create_view() {
            Some(view) => {
                drop(view);
                true
            }
            None => false,
        };
        self.probed.set(Some(answer));
        answer
    }

    /// `IEditController::createView("editor")`, owned.
    fn create_view(&self) -> Option<ComPtr<IPlugView>> {
        // SAFETY: live controller; `kEditor` is a static NUL-terminated string.
        let raw = unsafe { self.controller.createView(ViewType::kEditor) };
        // SAFETY: `createView` returns a view with one reference already added,
        // which `from_raw` takes ownership of.
        unsafe { ComPtr::<IPlugView>::from_raw(raw) }
    }
}

/// A plugin's own editor, in a window of its own.
///
/// Owns the `IPlugView` and the native window, and its `Drop` is the only
/// teardown path: `removed()`, then the frame is detached, then the window goes.
///
/// `!Send` and `!Sync`, because the window is.
pub struct Editor {
    inner: Inner,
}

impl Editor {
    /// Open `inst`'s editor in a window titled `title`. Main thread only.
    ///
    /// Only usable where the caller still owns the instance — the example and
    /// the tests. The app's engine has handed its instance to the audio thread
    /// and goes through [`Editor::open_handle`] instead.
    pub fn open(inst: &Instance, title: &str) -> Result<Self, EditorError> {
        let handle = inst.editor_handle().ok_or(EditorError::NoEditor)?;
        Self::open_handle(&handle, title)
    }

    /// Open the editor behind `handle` in a window titled `title`. Main thread
    /// only.
    pub fn open_handle(handle: &EditorHandle, title: &str) -> Result<Self, EditorError> {
        let view = handle.create_view().ok_or(EditorError::NoEditor)?;
        handle.probed.set(Some(true));
        Ok(Self {
            inner: Inner::open(view, title)?,
        })
    }

    /// The user closed the window. The caller should drop this `Editor`.
    ///
    /// Poll it once a frame. It stays `false` for a window that is merely
    /// hidden, minimised or behind something else — this is a real
    /// `windowWillClose:` and not a guess from visibility, which would report a
    /// minimised window as closed and throw the user's editor away for them.
    pub fn closed(&self) -> bool {
        self.inner.closed()
    }

    /// Bring the window to the front and give it the keyboard.
    ///
    /// What "open the editor" should do when one is already open.
    pub fn focus(&self) {
        self.inner.focus();
    }

    /// The window's current content size in points, as the plugin last asked
    /// for it.
    pub fn size(&self) -> (i32, i32) {
        self.inner.size()
    }

    /// How many times the plugin has asked the host to resize it.
    ///
    /// Diagnostic, and the only way to see from outside that `IPlugFrame` is
    /// wired up at all: Pianoteq calls it when its layout changes, so a
    /// non-zero count here is proof the host side of the contract is live.
    pub fn resizes(&self) -> u64 {
        self.inner.resizes()
    }
}

/// Service the platform's window events for up to `timeout`.
///
/// **Only for a process with no other event loop**, which means the
/// `ivory-host` examples and `--plugin-test`. Tangent itself must never call
/// this: `winit` owns `NSApplication`'s run loop there and two things pulling
/// events out of one queue is how a UI starts dropping clicks.
///
/// Returns after `timeout` at the latest, so a caller can interleave its own
/// work — playing a phrase, draining a meter — between calls.
pub fn pump(timeout: Duration) {
    #[cfg(target_os = "macos")]
    mac::pump(timeout);
    #[cfg(not(target_os = "macos"))]
    let _ = timeout;
}

// ─────────────────────────────────────────────────────────────────────────────
// macOS
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
mod mac {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    use objc2::rc::{autoreleasepool, Retained};
    use objc2::{define_class, DefinedClass, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSEventMask, NSView,
        NSWindow, NSWindowDelegate, NSWindowStyleMask,
    };
    use objc2_foundation::{
        NSDate, NSDefaultRunLoopMode, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
        NSSize, NSString,
    };
    use vst3::Steinberg::{
        kInvalidArgument, kPlatformTypeNSView, kResultFalse, kResultOk, tresult, IPlugFrame,
        IPlugFrameTrait, IPlugView, IPlugViewTrait, ViewRect,
    };
    use vst3::{Class, ComPtr, ComRef, ComWrapper};

    use super::EditorError;

    /// Fallback content size for a plugin whose `getSize` fails.
    ///
    /// Not a guess at anything: it is a window big enough to be obviously wrong
    /// and to be resized by the plugin's own `resizeView`, which is what a
    /// plugin that would not answer `getSize` is likely to do anyway.
    const FALLBACK_SIZE: (i32, i32) = (800, 600);

    /// The two facts the delegate and the frame publish and [`Window`] reads.
    ///
    /// `Rc<Cell<..>>` and not an atomic: everything that touches this is on the
    /// main thread by contract, and both writers are AppKit or VST3 callbacks
    /// that arrive there.
    #[derive(Default)]
    pub(super) struct State {
        closed: Cell<bool>,
        resizes: Cell<u64>,
        size: Cell<(i32, i32)>,
    }

    define_class!(
        // SAFETY:
        // - NSObject has no subclassing requirements.
        // - CloseWatcher implements no Drop.
        // - MainThreadOnly because NSWindowDelegate is, and because the ivar is
        //   an `Rc` that must not be released from another thread.
        #[unsafe(super(NSObject))]
        #[thread_kind = MainThreadOnly]
        // Named explicitly and distinctively. A class name is a process-global
        // symbol and this process is full of foreign code: a plugin registering
        // its own "WindowDelegate" would be a runtime collision with no error
        // message, only whichever one registered first winning.
        #[name = "TangentVst3EditorWindowDelegate"]
        #[ivars = Rc<State>]
        struct CloseWatcher;

        unsafe impl NSObjectProtocol for CloseWatcher {}

        unsafe impl NSWindowDelegate for CloseWatcher {
            /// The user hit the close button.
            ///
            /// **This, and not `isVisible`.** A minimised window is not visible;
            /// so is every window of a hidden application (Cmd-H). Polling
            /// visibility would report both as closed and would silently throw
            /// away the editor of anyone who minimised it.
            #[unsafe(method(windowWillClose:))]
            fn window_will_close(&self, _notification: &NSNotification) {
                self.ivars().closed.set(true);
            }
        }
    );

    impl CloseWatcher {
        fn new(mtm: MainThreadMarker, state: Rc<State>) -> Retained<Self> {
            let this = Self::alloc(mtm).set_ivars(state);
            // SAFETY: calling NSObject's designated initialiser on a fresh
            // allocation, which is what `define_class!` requires of us.
            unsafe { objc2::msg_send![super(this), init] }
        }
    }

    /// The host's `IPlugFrame`: how a plugin asks for a different size.
    ///
    /// Held by [`Window`] and released only after the view it belongs to,
    /// because `IPlugView::setFrame` does **not** take a reference — the SDK's
    /// own `CPluginView` stores the pointer bare and relies on the host to
    /// outlive it. Field order in [`Window`] is what makes that true.
    struct PlugFrame {
        window: Retained<NSWindow>,
        view: Retained<NSView>,
        state: Rc<State>,
    }

    impl Class for PlugFrame {
        type Interfaces = (IPlugFrame,);
    }

    impl IPlugFrameTrait for PlugFrame {
        unsafe fn resizeView(&self, view: *mut IPlugView, new_size: *mut ViewRect) -> tresult {
            if new_size.is_null() {
                return kInvalidArgument;
            }
            // SAFETY: a caller-provided ViewRect, checked non-null.
            let rect = unsafe { *new_size };
            let (w, h) = (rect.right - rect.left, rect.bottom - rect.top);
            if w < 1 || h < 1 {
                return kInvalidArgument;
            }
            // VST3 says main thread and AppKit insists. A plugin that called
            // this from a worker would be mutating the window server's idea of
            // a window from the wrong thread, which does not fail — it corrupts.
            // Refusing is survivable; the plugin draws at its old size.
            if MainThreadMarker::new().is_none() {
                return kResultFalse;
            }
            let size = NSSize::new(f64::from(w), f64::from(h));
            self.window.setContentSize(size);
            self.view.setFrameSize(size);
            self.state.size.set((w, h));
            self.state.resizes.set(self.state.resizes.get() + 1);

            // Resize, THEN tell the view it happened — the order in the SDK's
            // own frame implementation. This re-enters the plugin from inside
            // one of its own calls, which is by design and is what a plugin
            // waiting to lay itself out is expecting.
            if !view.is_null() {
                // SAFETY: a borrowed pointer the plugin owns for the duration of
                // this call; `ComRef` adds no reference and releases none.
                if let Some(v) = unsafe { ComRef::<IPlugView>::from_raw(view) } {
                    // SAFETY: live view, valid ViewRect.
                    unsafe { v.onSize(new_size) };
                }
            }
            kResultOk
        }
    }

    /// The window, the view inside it, and the plugin's view inside that.
    ///
    /// **Field order is the teardown order** and is not cosmetic. `Drop` runs
    /// the VST3 half by hand and then the fields go in the order below: the
    /// plugin's view first, then the frame it was told about, then our `NSView`,
    /// then the window.
    pub(super) struct Window {
        view: ComPtr<IPlugView>,
        /// Held, never read: the plugin has a bare pointer to it and `Drop`
        /// needs it to still be there when `setFrame(null)` runs.
        #[allow(dead_code, reason = "kept alive for the plugin's pointer, not read")]
        frame: ComWrapper<PlugFrame>,
        /// Held, never read: it is the window's content view and the plugin's
        /// parent, and releasing it before `removed()` is the crash this whole
        /// module is arranged around.
        #[allow(dead_code, reason = "kept alive for the plugin's view, not read")]
        nsview: Retained<NSView>,
        window: Retained<NSWindow>,
        /// Retained here because `NSWindow`'s delegate reference is weak. A
        /// delegate that only the window "held" would be freed immediately and
        /// the close notification would land on garbage.
        #[allow(dead_code, reason = "the window's reference to it is weak")]
        delegate: Retained<CloseWatcher>,
        state: Rc<State>,
    }

    impl Window {
        pub(super) fn open(view: ComPtr<IPlugView>, title: &str) -> Result<Self, EditorError> {
            let Some(mtm) = MainThreadMarker::new() else {
                return Err(EditorError::NotMainThread);
            };
            // A pool around every AppKit excursion, for the reason spelled out
            // in `pump`: a caller with no run loop of its own has no pool
            // either, and what AppKit autoreleases in here would leak outright.
            // Harmless and nested when the app DOES have one.
            autoreleasepool(|_| Self::open_inner(view, title, mtm))
        }

        fn open_inner(
            view: ComPtr<IPlugView>,
            title: &str,
            mtm: MainThreadMarker,
        ) -> Result<Self, EditorError> {
            // Ask before assuming. A plugin built for a different toolkit says
            // no here, and that is a clean refusal rather than a crash inside
            // `attached`.
            // SAFETY: live view; the platform type is a static C string.
            if unsafe { view.isPlatformTypeSupported(kPlatformTypeNSView) } != kResultOk {
                return Err(EditorError::WrongPlatform("NSView"));
            }

            let mut rect = ViewRect {
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
            };
            // SAFETY: live view, valid out-parameter.
            let sized = unsafe { view.getSize(&mut rect) };
            let (w, h) = if sized == kResultOk {
                (
                    (rect.right - rect.left).max(1),
                    (rect.bottom - rect.top).max(1),
                )
            } else {
                FALLBACK_SIZE
            };

            let state = Rc::new(State::default());
            state.size.set((w, h));
            let content = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(f64::from(w), f64::from(h)),
            );
            // No `Resizable`: see the module docs. The plugin resizes us, the
            // user does not.
            let style = NSWindowStyleMask::Titled
                | NSWindowStyleMask::Closable
                | NSWindowStyleMask::Miniaturizable;
            // SAFETY: AppKit's designated NSWindow initialiser, on the main
            // thread, with a fully-formed rect and style. Unsafe in objc2
            // solely because of the `releasedWhenClosed` obligation discharged
            // immediately below.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    content,
                    style,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            // **THE memory-management trap in this file.** A programmatically
            // created NSWindow defaults to `releasedWhenClosed: YES`, so the
            // close button releases the window — and we are holding the only
            // other reference, which becomes a dangling `Retained` the instant
            // the user closes it. Every "closing the plugin window crashes the
            // host" on macOS is this line missing.
            // SAFETY: the obligation objc2 marks `initWithContentRect` unsafe
            // for, discharged on the very next line after it.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(title));

            // Our own NSView rather than the window's automatic content view:
            // the plugin puts its view inside this one and `removed()` has to
            // happen before it is released, which means owning its lifetime.
            let nsview = NSView::initWithFrame(NSView::alloc(mtm), content);
            window.setContentView(Some(&nsview));

            let delegate = CloseWatcher::new(mtm, Rc::clone(&state));
            window.setDelegate(Some(objc2::runtime::ProtocolObject::from_ref(&*delegate)));

            let frame = ComWrapper::new(PlugFrame {
                window: window.clone(),
                view: nsview.clone(),
                state: Rc::clone(&state),
            });
            let Some(frame_ptr) = frame.to_com_ptr::<IPlugFrame>() else {
                return Err(EditorError::Refused(
                    "could not build the host's IPlugFrame".to_string(),
                ));
            };
            // Before `attached`, as the SDK sequences it: a plugin may ask for
            // its real size during attach and needs somewhere to ask.
            // SAFETY: live view; the frame outlives it (field order in `Window`).
            unsafe { view.setFrame(frame_ptr.as_ptr()) };

            let parent: *mut c_void = Retained::as_ptr(&nsview).cast_mut().cast();
            // SAFETY: `parent` is a live NSView owned by this struct and the
            // platform type matches it. This is the call that hands a plugin a
            // piece of our window.
            let attached = unsafe { view.attached(parent, kPlatformTypeNSView) };
            if attached != kResultOk {
                // Undo the half-built state in the right order rather than
                // leaving `Drop` to guess: nothing is attached, so `removed()`
                // is NOT owed and calling it would be a second teardown of
                // something that never started.
                // SAFETY: live view, dropping the frame it was given.
                unsafe { view.setFrame(std::ptr::null_mut()) };
                window.setDelegate(None);
                window.close();
                return Err(EditorError::Refused(format!(
                    "the plugin refused to attach to an NSView (tresult {attached})"
                )));
            }

            window.center();
            window.makeKeyAndOrderFront(None);
            // Bring the process forward too. Without this the window appears
            // behind whatever the user was looking at when they asked for it,
            // which reads as "nothing happened".
            NSApplication::sharedApplication(mtm).activate();

            Ok(Self {
                view,
                frame,
                nsview,
                window,
                delegate,
                state,
            })
        }

        pub(super) fn closed(&self) -> bool {
            self.state.closed.get()
        }

        pub(super) fn focus(&self) {
            if let Some(mtm) = MainThreadMarker::new() {
                self.window.makeKeyAndOrderFront(None);
                NSApplication::sharedApplication(mtm).activate();
            }
        }

        pub(super) fn size(&self) -> (i32, i32) {
            self.state.size.get()
        }

        pub(super) fn resizes(&self) -> u64 {
            self.state.resizes.get()
        }
    }

    impl Drop for Window {
        fn drop(&mut self) {
            autoreleasepool(|_| self.teardown());
        }
    }

    impl Window {
        /// The order that matters, in four steps. Split out of `Drop` only so
        /// an autorelease pool can wrap the whole of it.
        fn teardown(&mut self) {
            // 1. The plugin takes its view out of ours. Anything after this
            //    that touches the plugin's drawing is the plugin's own problem;
            //    anything BEFORE it that destroys our view is ours, and is a
            //    crash inside the plugin several seconds later.
            // SAFETY: live view, attached exactly once in `open`.
            unsafe { self.view.removed() };
            // 2. The plugin forgets the frame, because the frame is about to go
            //    and `setFrame` never took a reference to it.
            // SAFETY: live view.
            unsafe { self.view.setFrame(std::ptr::null_mut()) };
            // 3. The delegate goes before the window can notify it. `close()`
            //    would fire `windowWillClose:` and the flag it sets is of no
            //    further interest, but a delegate released while the window
            //    still points at it would be.
            self.window.setDelegate(None);
            self.window.close();
            // 4. Fields drop in declaration order from here: view, frame,
            //    nsview, window, delegate. The view is released before the
            //    frame it was told about, which is the whole reason for that
            //    order.
        }
    }

    /// Run `NSApplication`'s event loop by hand for up to `timeout`.
    ///
    /// `nextEventMatchingMask:untilDate:` is not merely a queue poll — it runs
    /// the CFRunLoop underneath, so timers, display refresh and the plugin's own
    /// drawing all happen inside it. Which is the difference between a window
    /// that is *on screen* and a window that is *alive*, and the reason this
    /// blocks for the whole timeout instead of returning immediately on an empty
    /// queue.
    pub(super) fn pump(timeout: Duration) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        let deadline = Instant::now() + timeout;
        loop {
            // **One pool per iteration, which is what `[NSApp run]` does.**
            // AppKit autoreleases freely while handling an event, and an object
            // autoreleased with no pool in place is not deferred — it is leaked
            // outright, permanently. Without this a hand-pumped loop grows for
            // as long as it runs, and closed windows never dealloc: an editor
            // opened and closed twenty times leaves twenty `NSWindow`s alive.
            let got_event = autoreleasepool(|_| {
                let left = deadline.saturating_duration_since(Instant::now());
                let until = NSDate::dateWithTimeIntervalSinceNow(left.as_secs_f64());
                // SAFETY: standard AppKit event pump on the main thread.
                let event = unsafe {
                    app.nextEventMatchingMask_untilDate_inMode_dequeue(
                        NSEventMask::Any,
                        Some(&until),
                        NSDefaultRunLoopMode,
                        true,
                    )
                };
                if let Some(e) = &event {
                    app.sendEvent(e);
                }
                event.is_some()
            });
            // `false` is the timeout, and the only exit.
            if !got_event || Instant::now() >= deadline {
                break;
            }
        }
        autoreleasepool(|_| app.updateWindows());
    }

    /// Make this process one that can own a window at all.
    ///
    /// A binary that is not inside a `.app` starts as a background process with
    /// no dock tile and no ability to take focus — its windows open behind
    /// everything and never become key. Bundled apps get this from their
    /// `Info.plist` and must not call it; the examples are not bundled and must.
    pub(super) fn become_foreground() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let app = NSApplication::sharedApplication(mtm);
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        // `finishLaunching` is what `[NSApp run]` does before its first event.
        // Skipping it leaves AppKit half-initialised and windows unresponsive.
        app.finishLaunching();
        app.activate();
    }
}

/// Make this process one whose windows can appear and take focus.
///
/// **For any process that will never call `[NSApp run]`** — the examples here
/// and `tangent --plugin-test`, both of which drive their own loop with
/// [`pump`]. Two different things are missing in those, and both are fixed
/// here: a bare (unbundled) executable starts as a background process whose
/// windows open behind everything and never become key, and a process that
/// never runs the event loop never gets AppKit's `finishLaunching`, without
/// which windows draw but do not respond.
///
/// Tangent's GUI must **not** call this: `winit` calls `[NSApp run]` and owns
/// both settings there.
pub fn become_foreground() {
    #[cfg(target_os = "macos")]
    mac::become_foreground();
}

// ─────────────────────────────────────────────────────────────────────────────
// Everywhere else
// ─────────────────────────────────────────────────────────────────────────────

/// The seam, on a platform whose window handle nobody has wired up yet.
///
/// An **uninhabited** `Window`, so [`Editor`] keeps exactly one definition and
/// one set of doc comments on every platform while being impossible to
/// construct here. When Windows arrives it is this module that grows a real
/// body: `kPlatformTypeHWND` and a `CreateWindowExW`, or `kPlatformTypeX11EmbedWindowID`
/// and an X11 window, into the same four methods below. Nothing in
/// [`Editor`] or in `instance.rs` has to change for either.
#[cfg(not(target_os = "macos"))]
mod stub {
    use super::EditorError;
    use vst3::ComPtr;
    use vst3::Steinberg::IPlugView;

    /// Uninhabited, like the empty enum it replaces — `void` can never be
    /// produced, so a `Window` can never exist. The phantom raw pointer is
    /// there for the tests' compile-time assertion that an `Editor` is not
    /// `Send`: an empty ENUM is vacuously `Send`, so on the platforms that use
    /// this stub the assertion held on macOS and failed to even compile here.
    /// The invariant is about the type's contract, not about whether a value
    /// can currently be made, so the stub carries the same marker the real
    /// windows do.
    pub(super) struct Window {
        void: std::convert::Infallible,
        _not_send: std::marker::PhantomData<*const ()>,
    }

    impl Window {
        pub(super) fn open(view: ComPtr<IPlugView>, title: &str) -> Result<Self, EditorError> {
            // The view is released here rather than leaked: it was created by
            // `EditorHandle::create_view` a few lines up and nothing is going to
            // attach it.
            drop(view);
            let _ = title;
            Err(EditorError::Unsupported)
        }

        pub(super) fn closed(&self) -> bool {
            match self.void {}
        }

        pub(super) fn focus(&self) {
            match self.void {}
        }

        pub(super) fn size(&self) -> (i32, i32) {
            match self.void {}
        }

        pub(super) fn resizes(&self) -> u64 {
            match self.void {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_has_a_sentence_a_user_could_read() {
        // These reach the UI verbatim, so "kResultFalse" is not an acceptable
        // answer for any of them.
        for e in [
            EditorError::Unsupported,
            EditorError::NoEditor,
            EditorError::WrongPlatform("NSView"),
            EditorError::NotMainThread,
            EditorError::Refused("the plugin refused to attach".to_string()),
        ] {
            let text = e.to_string();
            assert!(text.len() > 10, "{e:?} says only {text:?}");
            assert!(!text.contains("Error"), "{text:?} reads like a type name");
        }
    }

    /// Is `T` `Send`, as a value rather than as a bound?
    ///
    /// Stable Rust has no negative trait bound, so this leans on the one rule
    /// that stands in for one: an **inherent** associated constant wins over a
    /// trait's, and the inherent one below only exists when `T: Send`. So
    /// `Probe::<T>::IS_SEND` reads `true` for a `Send` type and falls through to
    /// the trait's `false` for anything else.
    struct Probe<T>(std::marker::PhantomData<T>);
    trait NotSend {
        const IS_SEND: bool = false;
    }
    impl<T> NotSend for Probe<T> {}
    impl<T: Send> Probe<T> {
        const IS_SEND: bool = true;
    }

    #[test]
    fn an_editor_cannot_be_moved_off_the_thread_that_made_it() {
        // Not a style preference. Every VST3 editor method and every AppKit
        // call in this file is main-thread-only, and `Editor` holding a
        // `Retained<NSWindow>` is what makes the compiler enforce it instead of
        // a comment asking nicely. If someone "fixes" that with an
        // `unsafe impl Send`, this fails rather than the window server does.
        //
        // `const` blocks, so this is a COMPILE error rather than a red test:
        // the fact is knowable at compile time and a broken invariant should
        // stop the build that broke it.
        const { assert!(!Probe::<Editor>::IS_SEND) };
        const { assert!(!Probe::<EditorHandle>::IS_SEND) };
        // And the mechanism itself works, which is worth one line: a probe that
        // always said `false` would pass the two assertions above forever.
        const { assert!(Probe::<u32>::IS_SEND) };
    }
}
