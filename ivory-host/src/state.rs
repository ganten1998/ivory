//! Saving and restoring what a plugin sounds like.
//!
//! A user picks a Pianoteq preset, quits, and comes back to the default piano.
//! Nothing about a hosted instrument survived the process before this file: the
//! host could create an instance and open its editor, and every knob the user
//! touched died with the instance.
//!
//! # The three calls, and the one that matters
//!
//! ```text
//! IComponent::getState(IBStream)              the PROCESSOR's state
//! IEditController::getState(IBStream)         the controller's VIEW state
//!
//! IComponent::setState(same bytes)            restore, in this order
//! IEditController::setComponentState(SAME STREAM, REWOUND)
//! IEditController::setState(the view bytes)
//! ```
//!
//! **`IComponent::getState` is the one a DAW saves.** It is the patch, the
//! preset, the sample set, every parameter the processor holds. The
//! controller's own `getState` is scroll position and which tab is open — worth
//! keeping, never worth failing a restore over, and this file treats it that
//! way.
//!
//! # `IBStream` is the HOST's to implement, and it is not a `Vec<u8>` with a
//! cursor bolted on
//!
//! There is no stream type in VST3. The plugin is handed an interface and it
//! reads and writes through it however it likes, which for a commercial
//! instrument means a nested chunk format with sizes patched in afterwards:
//! **`seek` is used in all three modes and `tell` is used to find out where a
//! length field needs to go.** Measured on Pianoteq 9, which writes a header,
//! writes the body, seeks back with `kIBSeekSet` and patches the length in.
//!
//! So [`MemoryStream`] implements all four methods properly, including the
//! parts a "we only ever write once" stream would get away with omitting:
//!
//! * a `seek` past the end is legal, and the `write` that follows zero-fills
//!   the gap rather than refusing (the SDK's own `MemoryStream` does this, and
//!   a plugin that reserves space and comes back to it depends on it);
//! * a `read` at the end returns `kResultOk` with **zero bytes read** rather
//!   than an error, because a plugin that loops until it gets a short read
//!   would otherwise treat "the state ended" as "the state is corrupt";
//! * every out-parameter is written on every path, including the failing ones,
//!   because a plugin that checks `numBytesRead` instead of the result code is
//!   entitled to.
//!
//! # The container, and why the bytes are not handed over bare
//!
//! What comes out of `getState` is opaque and plugin-specific: it is not
//! self-describing, it carries no length, and feeding a plugin somebody else's
//! bytes is undefined behaviour with a good chance of looking like a crash in
//! the plugin. These bytes are going into a settings file a user can open in a
//! text editor, so they come back wrong eventually — truncated by a half-written
//! save, pasted from another machine's settings, or left behind after the user
//! switched instruments.
//!
//! ```text
//! offset  size  field
//! 0       4     magic, b"TGST"
//! 4       2     version, u16 little-endian, currently 1
//! 6       4     processor blob length, u32 little-endian
//! 10      4     controller blob length, u32 little-endian
//! 14      4     FNV-1a-32 of the two blobs, in order
//! 18      n     the processor's bytes
//! 18+n    m     the controller's bytes
//! ```
//!
//! Little-endian throughout and a fixed header, so a blob written on one
//! machine restores on another. Eighteen bytes of overhead against Pianoteq's
//! kilobytes is not worth economising, and each field earns its place: the
//! magic rejects a foreign blob, the version rejects a future one, the two
//! lengths must add up to exactly the buffer's size (which rejects truncation
//! **and** trailing junk), and the hash rejects an edit inside the payload that
//! the lengths cannot see. Every one of those is an `Err` and none of them
//! reaches the plugin.
//!
//! # What this file will not do
//!
//! It will not interpret a plugin's bytes, and it will not migrate them. A
//! plugin that changes its own format between versions is the plugin's problem
//! to detect — most refuse politely, and a refusal arrives here as a `tresult`
//! and leaves as an `Err`.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;

use vst3::Steinberg::Vst::{IComponent, IComponentTrait, IEditController, IEditControllerTrait};
use vst3::Steinberg::IBStream_::IStreamSeekMode_;
#[cfg(test)]
use vst3::Steinberg::IBStream_::IStreamSeekMode;
use vst3::Steinberg::{
    int32, int64, kInternalError, kInvalidArgument, kOutOfMemory, kResultOk, tresult, IBStream,
    IBStreamTrait,
};
use vst3::{Class, ComPtr, ComWrapper};

/// Header bytes: magic, version, two lengths, hash.
const HEADER: usize = 18;

/// `b"TGST"` — Tangent STate. Four bytes of "these are not your bytes".
const MAGIC: [u8; 4] = *b"TGST";

/// Bumped only when the container itself changes shape. **Not** when a plugin
/// changes its own format: that is the plugin's business and it is why the
/// blobs are opaque here.
const VERSION: u16 = 1;

/// The most a single instrument's state may be, in either direction.
///
/// Sixteen megabytes is far past anything an instrument has been measured at
/// (Pianoteq 9 is kilobytes) and far short of a number that can exhaust a
/// machine. It exists because both directions take a length from somewhere
/// untrusted: a plugin writing through [`MemoryStream`] decides how much to
/// write, and a settings file decides how much to claim. A sampler that
/// genuinely embeds its samples would hit this, and the honest failure is an
/// error naming the cap rather than a machine that swaps.
pub const MAX_STATE_BYTES: usize = 16 * 1024 * 1024;

// ── the container ────────────────────────────────────────────────────────────

/// FNV-1a, 32-bit. Eight lines, no table, no dependency.
///
/// Not a cryptographic anything and not claimed to be: the threat is a
/// half-written file and a user with a text editor, not an adversary. It costs
/// one pass over a few kilobytes at save and at load.
fn hash32(parts: [&[u8]; 2]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for part in parts {
        for b in part {
            h ^= u32::from(*b);
            h = h.wrapping_mul(0x0100_0193);
        }
    }
    h
}

/// Wrap the two blobs in the container documented at the top of this file.
fn encode(processor: &[u8], controller: &[u8]) -> Result<Vec<u8>, String> {
    if processor.len() > MAX_STATE_BYTES || controller.len() > MAX_STATE_BYTES {
        return Err(format!(
            "the instrument's state is {} bytes, past the {MAX_STATE_BYTES} byte ceiling",
            processor.len().max(controller.len())
        ));
    }
    let mut out = Vec::with_capacity(HEADER + processor.len() + controller.len());
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    out.extend_from_slice(&(processor.len() as u32).to_le_bytes());
    out.extend_from_slice(&(controller.len() as u32).to_le_bytes());
    out.extend_from_slice(&hash32([processor, controller]).to_le_bytes());
    out.extend_from_slice(processor);
    out.extend_from_slice(controller);
    Ok(out)
}

/// Unwrap a container, or say what is wrong with it.
///
/// **Every failure here is a failure that never reaches the plugin**, which is
/// the entire point: `IComponent::setState` with somebody else's bytes is
/// undefined behaviour wearing a `tresult`.
fn decode(bytes: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let Some(head) = bytes.get(..HEADER) else {
        return Err(format!(
            "not instrument state: {} bytes is shorter than the {HEADER} byte header",
            bytes.len()
        ));
    };
    // Every slice below is inside `head`, whose length was just checked, so the
    // `try_into`s cannot fail — and they are still written as fallible, because
    // the day somebody changes HEADER is the day an `expect` here becomes a
    // panic on a user's settings file.
    let field = |at: usize, n: usize| -> Result<&[u8], String> {
        head.get(at..at + n)
            .ok_or_else(|| "the state header is malformed".to_string())
    };
    if field(0, 4)? != MAGIC {
        return Err("not instrument state: the magic number does not match".to_string());
    }
    let version = u16::from_le_bytes(
        field(4, 2)?
            .try_into()
            .map_err(|_| "the state header is malformed".to_string())?,
    );
    if version != VERSION {
        return Err(format!(
            "instrument state version {version}, but this build understands {VERSION}"
        ));
    }
    let len = |at: usize| -> Result<usize, String> {
        let raw = u32::from_le_bytes(
            field(at, 4)?
                .try_into()
                .map_err(|_| "the state header is malformed".to_string())?,
        );
        Ok(raw as usize)
    };
    let proc_len = len(6)?;
    let ctrl_len = len(10)?;
    let want = u32::from_le_bytes(
        field(14, 4)?
            .try_into()
            .map_err(|_| "the state header is malformed".to_string())?,
    );

    // Summed as u64 so a pair of lengths that overflow `usize` on a 32-bit
    // build is caught here rather than wrapping into a plausible small number.
    let total = HEADER as u64 + proc_len as u64 + ctrl_len as u64;
    if total != bytes.len() as u64 {
        return Err(format!(
            "instrument state is {} bytes but its header describes {total}; it is \
             truncated or has something appended",
            bytes.len()
        ));
    }
    let (Some(processor), Some(controller)) = (
        bytes.get(HEADER..HEADER + proc_len),
        bytes.get(HEADER + proc_len..HEADER + proc_len + ctrl_len),
    ) else {
        return Err("the state header is malformed".to_string());
    };
    if hash32([processor, controller]) != want {
        return Err(
            "instrument state failed its checksum; it has been edited or corrupted".to_string(),
        );
    }
    Ok((processor, controller))
}

// ── the stream ───────────────────────────────────────────────────────────────

/// An `IBStream` over a `Vec<u8>`, implemented by the host because VST3 has no
/// stream of its own.
///
/// `RefCell` for the bytes and `Cell` for the cursor, and the same argument as
/// `HostMessage` in `instance.rs`: every borrow below is a leaf borrow that
/// calls nothing, so there is no path on which one is held while the plugin
/// re-enters. A `RefCell` panic across this ABI is undefined behaviour rather
/// than a backtrace.
///
/// **The cursor may legally sit past the end.** That is not a bug to clamp
/// away: a plugin that writes a placeholder length, writes a body, and seeks
/// back to patch the length needs the position to be meaningful wherever it
/// puts it.
pub(crate) struct MemoryStream {
    bytes: RefCell<Vec<u8>>,
    pos: Cell<usize>,
}

impl MemoryStream {
    fn empty() -> Self {
        Self {
            bytes: RefCell::new(Vec::new()),
            pos: Cell::new(0),
        }
    }

    fn over(bytes: Vec<u8>) -> Self {
        Self {
            bytes: RefCell::new(bytes),
            pos: Cell::new(0),
        }
    }

    /// A copy of everything written, from the start, whatever the cursor is
    /// doing. Copied and not moved out, because the plugin still owns a
    /// reference to this object.
    fn contents(&self) -> Vec<u8> {
        self.bytes.borrow().clone()
    }

    /// Put the cursor back at the start.
    ///
    /// **The step between `setState` and `setComponentState`.** See [`load`].
    fn rewind(&self) {
        self.pos.set(0);
    }
}

impl Class for MemoryStream {
    type Interfaces = (IBStream,);
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_read: *mut int32,
    ) -> tresult {
        // Written FIRST and on every path: a plugin that trusts this instead of
        // the result code would otherwise read whatever was on its stack and
        // then copy that many bytes out of `buffer`.
        if !num_bytes_read.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *num_bytes_read = 0 };
        }
        let Ok(want) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if want == 0 {
            return kResultOk;
        }
        if buffer.is_null() {
            return kInvalidArgument;
        }
        let bytes = self.bytes.borrow();
        let at = self.pos.get();
        // A cursor past the end reads nothing and stays where it is. `kResultOk`
        // with zero bytes and NOT an error: a plugin that reads until it gets a
        // short read is doing the normal thing, and answering it with a failure
        // turns "the state ended" into "the state is broken".
        let Some(rest) = bytes.get(at..) else {
            return kResultOk;
        };
        let take = want.min(rest.len());
        if take > 0 {
            // SAFETY: `rest` is readable for `take` bytes by construction, and
            // the plugin says `buffer` is writable for `num_bytes >= take`. The
            // two cannot overlap: this Vec is the host's and was never handed
            // out as a pointer.
            unsafe { std::ptr::copy_nonoverlapping(rest.as_ptr(), buffer.cast::<u8>(), take) };
            self.pos.set(at + take);
        }
        if !num_bytes_read.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *num_bytes_read = take as int32 };
        }
        kResultOk
    }

    unsafe fn write(
        &self,
        buffer: *mut c_void,
        num_bytes: int32,
        num_bytes_written: *mut int32,
    ) -> tresult {
        if !num_bytes_written.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *num_bytes_written = 0 };
        }
        let Ok(n) = usize::try_from(num_bytes) else {
            return kInvalidArgument;
        };
        if n == 0 {
            return kResultOk;
        }
        if buffer.is_null() {
            return kInvalidArgument;
        }
        let at = self.pos.get();
        let Some(end) = at.checked_add(n).filter(|e| *e <= MAX_STATE_BYTES) else {
            // A plugin that would take the state past the ceiling is refused
            // here rather than allowed to fill memory. See [`MAX_STATE_BYTES`].
            return kOutOfMemory;
        };
        let mut bytes = self.bytes.borrow_mut();
        if end > bytes.len() {
            // Grows, and zero-fills any gap a seek-past-the-end left behind.
            // The SDK's own MemoryStream does exactly this and plugins that
            // reserve space to come back to depend on it.
            bytes.resize(end, 0);
        }
        let Some(slot) = bytes.get_mut(at..end) else {
            // Unreachable after the resize, and checked anyway: the alternative
            // is a panic across the VST3 ABI, which is undefined behaviour.
            return kInternalError;
        };
        // SAFETY: the plugin says `buffer` is readable for `num_bytes`, and
        // `slot` is exactly that long and belongs to this host.
        unsafe { std::ptr::copy_nonoverlapping(buffer.cast::<u8>(), slot.as_mut_ptr(), n) };
        self.pos.set(end);
        if !num_bytes_written.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *num_bytes_written = n as int32 };
        }
        kResultOk
    }

    unsafe fn seek(&self, pos: int64, mode: int32, result: *mut int64) -> tresult {
        // All three modes, because plugins genuinely use all three: `kIBSeekSet`
        // to patch a length field, `kIBSeekCur` to skip a chunk it does not
        // recognise, `kIBSeekEnd` to measure what it has written.
        let base = if mode == IStreamSeekMode_::kIBSeekSet as int32 {
            0
        } else if mode == IStreamSeekMode_::kIBSeekCur as int32 {
            self.pos.get() as int64
        } else if mode == IStreamSeekMode_::kIBSeekEnd as int32 {
            self.bytes.borrow().len() as int64
        } else {
            return kInvalidArgument;
        };
        let Some(target) = base.checked_add(pos) else {
            return kInvalidArgument;
        };
        // Past the end is legal (a following write zero-fills the gap); before
        // the start is not, and neither is past the ceiling.
        let Ok(at) = usize::try_from(target) else {
            return kInvalidArgument;
        };
        if at > MAX_STATE_BYTES {
            return kInvalidArgument;
        }
        self.pos.set(at);
        if !result.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *result = target };
        }
        kResultOk
    }

    unsafe fn tell(&self, pos: *mut int64) -> tresult {
        if pos.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: caller-provided out-parameter, checked non-null.
        unsafe { *pos = self.pos.get() as int64 };
        kResultOk
    }
}

// ── save and restore ─────────────────────────────────────────────────────────

/// Run `ask` with a fresh empty stream and return what it wrote.
fn drain(what: &str, ask: impl FnOnce(*mut IBStream) -> tresult) -> Result<Vec<u8>, String> {
    let stream = ComWrapper::new(MemoryStream::empty());
    let ptr = stream
        .to_com_ptr::<IBStream>()
        .ok_or_else(|| "could not build the state stream".to_string())?;
    let r = ask(ptr.as_ptr());
    if r != kResultOk {
        return Err(format!("the plugin refused to hand over its {what} (tresult {r})"));
    }
    Ok(stream.contents())
}

/// Both halves' state, wrapped in the container documented above.
///
/// The controller's view state is **best effort**: a plugin that has none, or
/// refuses to give it, costs an empty second blob and not a failed save. The
/// processor's is not — a save that silently produced no patch is worse than no
/// save at all, because it looks like it worked until the next launch.
pub(crate) fn save(
    component: &ComPtr<IComponent>,
    controller: Option<&ComPtr<IEditController>>,
) -> Result<Vec<u8>, String> {
    // SAFETY: a live, initialised component. `getState` is a main-thread call
    // and this is the main thread; see `Instance::save_state`.
    let processor = drain("state", |s| unsafe { component.getState(s) })?;
    let view = match controller {
        // SAFETY: a live, initialised controller.
        Some(c) => drain("view state", |s| unsafe { c.getState(s) }).unwrap_or_default(),
        None => Vec::new(),
    };
    encode(&processor, &view)
}

/// Restore what [`save`] produced.
///
/// # The rewind between the two calls is the trap
///
/// `IComponent::setState` reads the stream to its end. `setComponentState` is
/// then handed **the same bytes** so the controller can show what the processor
/// is doing — and a host that passes the same stream object without seeking
/// back to zero hands it an empty read. Nothing fails: the processor has the
/// preset, the sound is right, and the plugin's UI shows the defaults. The user
/// reports it as "the editor is out of sync", months later.
///
/// A fresh stream over the same bytes would work as well and is what a careless
/// reading of the SDK suggests. It is written as one stream and an explicit
/// rewind because that is the shape of the mistake, and a rewind that is there
/// on purpose is harder to delete than one that was never needed.
pub(crate) fn load(
    component: &ComPtr<IComponent>,
    controller: Option<&ComPtr<IEditController>>,
    bytes: &[u8],
) -> Result<(), String> {
    let (processor, view) = decode(bytes)?;

    let stream = ComWrapper::new(MemoryStream::over(processor.to_vec()));
    let ptr = stream
        .to_com_ptr::<IBStream>()
        .ok_or_else(|| "could not build the state stream".to_string())?;
    // SAFETY: a live, initialised component, and a stream this function owns
    // for longer than the call.
    let r = unsafe { component.setState(ptr.as_ptr()) };
    if r != kResultOk {
        return Err(format!(
            "the instrument refused the saved state (tresult {r}); it may have been \
             written by a different version of the plugin"
        ));
    }

    if let Some(c) = controller {
        stream.rewind();
        // SAFETY: a live, initialised controller, and the same stream.
        //
        // Deliberately not fatal. Plenty of controllers return kNotImplemented
        // here and keep themselves in step some other way, and the processor —
        // which is what the user hears — already has the state. The cost of
        // being wrong is a UI showing defaults, which is visible; the cost of
        // failing the whole restore over it is a piano that will not load.
        let _ = unsafe { c.setComponentState(ptr.as_ptr()) };

        if !view.is_empty() {
            let view_stream = ComWrapper::new(MemoryStream::over(view.to_vec()));
            if let Some(vptr) = view_stream.to_com_ptr::<IBStream>() {
                // SAFETY: a live controller and a stream owned for the call.
                let _ = unsafe { c.setState(vptr.as_ptr()) };
            }
        }
    }
    Ok(())
}

/// A main-thread reference to a loaded plugin's two halves, for saving its
/// state after the instance itself has left for the audio callback.
///
/// # Why this exists rather than `Instance::save_state` being enough
///
/// The same reason [`crate::EditorHandle`] exists, and the argument is worth
/// re-reading there in full: in the app the `Instance` is *inside the audio
/// callback* by the time anyone asks it anything, and there is no `&Instance`
/// to be had on this thread at all. `IComponent` is a reference-counted COM
/// object, so a second reference can be taken while the instance is still here
/// and kept after it leaves.
///
/// # Is `getState` safe while the audio thread is calling `process`?
///
/// It is the arrangement VST3 specifies and the one every DAW relies on: the
/// SDK splits its API into a processing context (`IAudioProcessor::process`,
/// and nothing else) and everything else, which is the UI thread's. Saving a
/// project while the transport rolls is exactly this call on exactly this
/// object, and a plugin that cannot survive it cannot be used in Cubase.
///
/// What is genuinely accepted is a plugin that does **not** synchronise its own
/// `getState` against its own `process` — out of spec, and the symptom would be
/// a torn blob rather than an error. The alternative was retiring the instance
/// across the handoff to borrow it back, which costs a block of silence and any
/// note in flight every time the settings file is written; see
/// `Engine::save_slot_state`.
///
/// **Drop it before the `Instance` is dropped.** The instance's teardown calls
/// `IComponent::terminate`, and a handle outliving that is a live pointer into
/// a terminated object.
pub struct StateHandle {
    component: ComPtr<IComponent>,
    controller: Option<ComPtr<IEditController>>,
    /// Makes this `!Send` and `!Sync` on purpose: every call it makes is a
    /// main-thread VST3 call. The bindings put an unconditional `unsafe impl
    /// Send` on every interface type, so `ComPtr` carries no thread information
    /// whatsoever and this marker is the only thing enforcing it. See
    /// [`crate::EditorHandle`], which learned it the same way.
    not_send: std::marker::PhantomData<*const ()>,
}

impl StateHandle {
    pub(crate) fn new(
        component: ComPtr<IComponent>,
        controller: Option<ComPtr<IEditController>>,
    ) -> Self {
        Self {
            component,
            controller,
            not_send: std::marker::PhantomData,
        }
    }

    /// The instrument's state, as [`crate::Instance::save_state`] would return
    /// it. Main thread only.
    pub fn save(&self) -> Result<Vec<u8>, String> {
        save(&self.component, self.controller.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read through the COM interface, exactly as a plugin would, so the tests
    /// exercise the vtable rather than the Rust methods behind it.
    fn read(s: &MemoryStream, n: usize) -> (tresult, Vec<u8>) {
        let mut buf = vec![0xAAu8; n];
        let mut got: int32 = -1;
        // SAFETY: `buf` is writable for `n` bytes and `got` is a valid
        // out-parameter.
        let r = unsafe { s.read(buf.as_mut_ptr().cast(), n as int32, &mut got) };
        let got = got.max(0) as usize;
        buf.truncate(got.min(n));
        (r, buf)
    }

    fn write(s: &MemoryStream, data: &[u8]) -> (tresult, int32) {
        let mut put: int32 = -1;
        // SAFETY: `data` is readable for its length; `put` is a valid
        // out-parameter. `write` does not mutate the buffer.
        let r = unsafe {
            s.write(
                data.as_ptr().cast::<c_void>().cast_mut(),
                data.len() as int32,
                &mut put,
            )
        };
        (r, put)
    }

    fn seek(s: &MemoryStream, pos: int64, mode: IStreamSeekMode) -> (tresult, int64) {
        let mut at: int64 = -1;
        // SAFETY: valid out-parameter.
        let r = unsafe { s.seek(pos, mode as int32, &mut at) };
        (r, at)
    }

    fn tell(s: &MemoryStream) -> int64 {
        let mut at: int64 = -1;
        // SAFETY: valid out-parameter.
        unsafe { s.tell(&mut at) };
        at
    }

    #[test]
    fn a_stream_reads_back_exactly_what_was_written_to_it() {
        let s = MemoryStream::empty();
        assert_eq!(write(&s, b"hello world"), (kResultOk, 11));
        assert_eq!(tell(&s), 11);
        s.rewind();
        let (r, got) = read(&s, 11);
        assert_eq!(r, kResultOk);
        assert_eq!(got, b"hello world");
    }

    #[test]
    fn reading_at_the_end_returns_ok_with_nothing_rather_than_an_error() {
        // A plugin that reads until it gets a short read is doing the normal
        // thing; answering it with a failure turns "the state ended" into "the
        // state is broken".
        let s = MemoryStream::empty();
        write(&s, b"abc");
        s.rewind();
        assert_eq!(read(&s, 8), (kResultOk, b"abc".to_vec()));
        assert_eq!(read(&s, 8), (kResultOk, Vec::new()));
        assert_eq!(tell(&s), 3, "a read that got nothing must not move the cursor");
    }

    #[test]
    fn all_three_seek_modes_land_where_the_sdk_says_they_do() {
        // Plugins use all three: set to patch a length, cur to skip a chunk,
        // end to measure. A host that implements one of them works with the
        // plugin it was tested against and no other.
        let s = MemoryStream::empty();
        write(&s, b"0123456789");
        assert_eq!(seek(&s, 3, IStreamSeekMode_::kIBSeekSet), (kResultOk, 3));
        assert_eq!(tell(&s), 3);
        assert_eq!(seek(&s, 2, IStreamSeekMode_::kIBSeekCur), (kResultOk, 5));
        assert_eq!(seek(&s, -1, IStreamSeekMode_::kIBSeekCur), (kResultOk, 4));
        assert_eq!(seek(&s, 0, IStreamSeekMode_::kIBSeekEnd), (kResultOk, 10));
        assert_eq!(seek(&s, -4, IStreamSeekMode_::kIBSeekEnd), (kResultOk, 6));
        let (_, got) = read(&s, 4);
        assert_eq!(got, b"6789");
    }

    #[test]
    fn seeking_before_the_start_is_refused_rather_than_wrapping_round() {
        let s = MemoryStream::empty();
        write(&s, b"abcd");
        assert_eq!(seek(&s, -1, IStreamSeekMode_::kIBSeekSet).0, kInvalidArgument);
        assert_eq!(seek(&s, i64::MIN, IStreamSeekMode_::kIBSeekCur).0, kInvalidArgument);
        assert_eq!(tell(&s), 4, "a refused seek must not move the cursor");
        assert_eq!(
            seek(&s, 1, 99).0,
            kInvalidArgument,
            "an unknown seek mode is refused, not treated as kIBSeekSet"
        );
    }

    #[test]
    fn a_plugin_that_reserves_space_and_comes_back_to_patch_it_gets_what_it_wrote() {
        // The measured shape of a real instrument's state: write a placeholder
        // length, write the body, seek back, patch the length in.
        let s = MemoryStream::empty();
        write(&s, &[0u8; 4]);
        write(&s, b"body");
        seek(&s, 0, IStreamSeekMode_::kIBSeekSet);
        write(&s, &4u32.to_le_bytes());
        s.rewind();
        let (_, got) = read(&s, 8);
        assert_eq!(got, [4, 0, 0, 0, b'b', b'o', b'd', b'y']);
    }

    #[test]
    fn a_write_after_seeking_past_the_end_zero_fills_the_gap() {
        let s = MemoryStream::empty();
        write(&s, b"ab");
        seek(&s, 5, IStreamSeekMode_::kIBSeekSet);
        write(&s, b"z");
        s.rewind();
        let (_, got) = read(&s, 16);
        assert_eq!(got, [b'a', b'b', 0, 0, 0, b'z']);
    }

    #[test]
    fn a_null_buffer_is_refused_rather_than_written_through() {
        let s = MemoryStream::empty();
        // SAFETY: deliberately passing null, which is what this checks.
        unsafe {
            assert_eq!(s.read(std::ptr::null_mut(), 4, std::ptr::null_mut()), kInvalidArgument);
            assert_eq!(s.write(std::ptr::null_mut(), 4, std::ptr::null_mut()), kInvalidArgument);
            assert_eq!(s.tell(std::ptr::null_mut()), kInvalidArgument);
            // Zero bytes is not an error even with a null buffer: there is
            // nothing to dereference.
            assert_eq!(s.read(std::ptr::null_mut(), 0, std::ptr::null_mut()), kResultOk);
            // And a negative count is refused rather than cast into a huge one.
            assert_eq!(s.read(std::ptr::null_mut(), -1, std::ptr::null_mut()), kInvalidArgument);
            assert_eq!(s.write(std::ptr::null_mut(), -1, std::ptr::null_mut()), kInvalidArgument);
        }
    }

    #[test]
    fn the_read_count_is_written_before_every_refusal_too() {
        // A plugin that checks numBytesRead instead of the result code would
        // otherwise copy whatever was on its stack out of its own buffer.
        let s = MemoryStream::empty();
        let mut got: int32 = 12_345;
        // SAFETY: valid out-parameter, deliberately bad count.
        let r = unsafe { s.read(std::ptr::null_mut(), -1, &mut got) };
        assert_eq!(r, kInvalidArgument);
        assert_eq!(got, 0);
    }

    #[test]
    fn a_write_that_would_pass_the_ceiling_is_refused_rather_than_filling_memory() {
        let s = MemoryStream::empty();
        seek(&s, MAX_STATE_BYTES as int64, IStreamSeekMode_::kIBSeekSet);
        assert_eq!(write(&s, b"x").0, kOutOfMemory);
        assert_eq!(
            seek(&s, MAX_STATE_BYTES as int64 + 1, IStreamSeekMode_::kIBSeekSet).0,
            kInvalidArgument
        );
    }

    #[test]
    fn a_container_round_trips_both_blobs() {
        let wrapped = encode(b"processor bytes", b"view bytes").expect("encode");
        let (a, b) = decode(&wrapped).expect("decode");
        assert_eq!(a, b"processor bytes");
        assert_eq!(b, b"view bytes");
        assert_eq!(wrapped.len(), HEADER + 15 + 10);
    }

    #[test]
    fn an_empty_controller_blob_is_a_legal_container() {
        // The common case for a plugin with no controller state at all.
        let wrapped = encode(b"p", b"").expect("encode");
        let (a, b) = decode(&wrapped).expect("decode");
        assert_eq!(a, b"p");
        assert!(b.is_empty());
    }

    #[test]
    fn a_foreign_or_truncated_or_edited_blob_is_an_error_and_never_reaches_the_plugin() {
        // Every one of these is a thing that will actually happen to a settings
        // file: an empty string, somebody else's data, a half-written save, a
        // hand-edited byte.
        assert!(decode(b"").is_err());
        assert!(decode(b"{\"volume\": 0.5}").is_err());

        let good = encode(b"processor", b"view").expect("encode");
        for cut in 0..good.len() {
            assert!(
                decode(&good[..cut]).is_err(),
                "a blob truncated to {cut} bytes was accepted"
            );
        }
        let mut extra = good.clone();
        extra.push(0);
        assert!(decode(&extra).is_err(), "trailing junk was accepted");

        let mut edited = good.clone();
        let last = edited.len() - 1;
        edited[last] ^= 0x20;
        assert!(
            decode(&edited).is_err(),
            "an edit inside the payload was accepted; the lengths cannot see it, \
             which is what the checksum is for"
        );

        let mut wrong_magic = good.clone();
        wrong_magic[0] = b'X';
        assert!(decode(&wrong_magic).is_err());

        let mut wrong_version = good.clone();
        wrong_version[4] = 99;
        assert!(decode(&wrong_version).is_err());
    }

    #[test]
    fn a_length_field_a_user_could_type_does_not_index_out_of_the_buffer() {
        // The interesting corruption is not a short buffer, it is a header that
        // claims a gigabyte. It must be an error rather than a panic or a read
        // past the end.
        let good = encode(b"processor", b"view").expect("encode");
        for at in [6usize, 10] {
            let mut lying = good.clone();
            lying[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
            assert!(decode(&lying).is_err(), "a u32::MAX length at {at} was accepted");
        }
        // And two lengths that only overflow when added together.
        let mut both = good.clone();
        both[6..10].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        both[10..14].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        assert!(decode(&both).is_err());
    }

    #[test]
    fn a_state_handle_cannot_be_moved_between_threads_by_accident() {
        // Every call it makes is a main-thread VST3 call, and `ComPtr` is
        // unconditionally `Send` in these bindings — so the marker is the only
        // thing enforcing it. `const`, so a regression fails the build.
        struct Probe<T>(std::marker::PhantomData<T>);
        trait NotSendProbe {
            const IS_SEND: bool = false;
        }
        impl<T> NotSendProbe for Probe<T> {}
        impl<T: Send> Probe<T> {
            const IS_SEND: bool = true;
        }
        const { assert!(!Probe::<StateHandle>::IS_SEND) };
        // And the reason the marker has to be there at all, as a fact rather
        // than a claim: the `ComPtr` inside it is `Send` on its own, so nothing
        // else was ever going to stop this type crossing a thread.
        const { assert!(Probe::<ComPtr<IComponent>>::IS_SEND) };
    }
}
