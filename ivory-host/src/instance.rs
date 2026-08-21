//! Instantiating a plugin and pulling audio out of it.
//!
//! `scan.rs` finds modules and reads what they claim to be. This turns one of
//! those claims into a running instrument: create the component, initialise it,
//! negotiate buses, activate, and call `process`.
//!
//! # The sequence is not optional and not reorderable
//!
//! VST3 has a state machine, and plugins enforce it. Every step below returns a
//! `tresult` and every one of them is checked, because the failure mode of
//! skipping a step is not an error — it is a plugin that returns silence, or one
//! that crashes on the third `process` call for reasons that look unrelated.
//!
//! ```text
//! createInstance(cid, IComponent)      the factory hands back the processing half
//! IComponent::initialize(host)         the plugin may allocate; needs a host context
//! queryInterface(IAudioProcessor)      the same object, asked for its other face
//! setupProcessing(rate, block, 32-bit) BEFORE setActive, always
//! activateBus(...) for each bus        a bus nobody activated produces nothing
//! setActive(true)                      the plugin allocates its DSP state here
//! setProcessing(true)                  entering the realtime section
//! process(ProcessData)                 finally
//! ```
//!
//! Teardown runs it backwards, and `Drop` does that so a `?` in the middle of
//! setup cannot leave a plugin active with no owner.
//!
//! # Why a host context is required rather than optional
//!
//! `initialize` takes an `FUnknown*`. Passing null is legal by the letter of the
//! ABI and many plugins accept it — and some do not, because they query it for
//! `IHostApplication` during initialisation and treat its absence as a fatal
//! configuration error. Supplying a real one costs about forty lines and removes
//! a whole class of "works with plugin A, silently fails with plugin B".
//!
//! # The sustain pedal, which is not an event
//!
//! VST3 deleted MIDI control changes from the event stream on purpose. There is
//! no "send CC64" call anywhere in the API. A pedal reaches an instrument like
//! this, and only like this:
//!
//! ```text
//! IComponent::getControllerClassId       the processor names its OTHER half
//! factory.createInstance(that, IEditController)
//! IEditController::initialize(host)
//! IConnectionPoint::connect  BOTH WAYS   <- the step everybody skips
//! IMidiMapping::getMidiControllerAssignment(bus, channel, 64, &paramId)
//! ProcessData::inputParameterChanges      a HOST-implemented IParameterChanges
//!   └ IParamValueQueue for paramId        carrying (sampleOffset, 0.0..=1.0)
//! ```
//!
//! Two traps live in that sequence and both of them look like success:
//!
//! 1. **`IMidiMapping` is not on the component.** It is on the edit controller,
//!    which is a separate class the factory exports and which nothing else in
//!    this host needs. Pianoteq 9: `component.cast::<IMidiMapping>()` is `None`.
//! 2. **An unconnected controller answers, and answers wrongly.** Pianoteq's
//!    `getMidiControllerAssignment` returns `kResultOk` for every controller on
//!    every channel *before* `IConnectionPoint::connect` — with `paramId 0` each
//!    time. Zero is a legal parameter id. A host that trusts the result code
//!    would push the sustain pedal into whatever parameter 0 happens to be, on
//!    every plugin, forever, and the symptom would be "the pedal does something
//!    strange" rather than an error. After connecting, the same call returns
//!    `0x6d636d40` for CC64 on channel 0 and a different id per channel. See
//!    [`read_midi_map`], which keeps the connection open for exactly as long as
//!    it takes to read the table and no longer.
//!
//! **And a third trap, found by measurement after both of those were fixed:**
//! Pianoteq publishes a mapping for CC64, accepts the parameter change, returns
//! `kResultOk`, and ignores it — the rendered audio is identical to six decimal
//! places. It responds only to the LEGACY MIDI CC event. So both are sent, on
//! every control, always; a CC is a value rather than a delta, so a plugin that
//! honours both sets the same parameter twice and nothing is harmed. See the
//! comment in `process_with_controls` for the measured numbers.
//!
//! The values then travel in `inputParameterChanges`, which is an interface the
//! **host** implements and the plugin CALLS during `process` — so
//! [`ParamChanges`] and [`ParamQueue`] run on the audio thread and are built to
//! that standard: allocated once in [`Instance::create`], reused every block,
//! `Cell` rather than `RefCell` so there is no borrow flag to panic on.
//!
//! # The controller is now KEPT, and that changes what the host owes
//!
//! Reading the CC table used to be the controller's whole reason to exist: it
//! was created, connected, harvested and torn down inside one function. It is
//! now created once and held for the instance's whole life, because
//! `IEditController::createView` is the only door to the plugin's own UI and a
//! released controller has no door. See [`editor`](crate::editor).
//!
//! That is not a free change. A controller that stays connected is a controller
//! the component will actually talk to and a controller a user will actually
//! touch, and both of those cost the host something it previously got away with
//! not having:
//!
//! **Fourth trap: `IHostApplication::createInstance` cannot go on refusing.**
//! The component-to-controller channel carries `IMessage` objects, and the
//! plugin does not allocate them — it asks the HOST to, through
//! `IHostApplication::createInstance(IMessage)`. This host used to return
//! `kResultFalse` there and say so in a comment ("a recorder that never opens an
//! editor does not need it"), which was true right up until it opened an editor.
//! A plugin whose halves cannot exchange messages does not fail loudly; it opens
//! a UI whose knobs move and whose sound does not change, because the news never
//! reaches the processor. [`HostMessage`] and [`HostAttributes`] are here for
//! exactly that, and [`HostApp::messages_made`] counts how often a plugin took
//! the offer — Pianoteq 9 does, during `initialize`, before a single block is
//! rendered.
//!
//! **Fifth trap: three objects now have to be torn down in one order.** An
//! `Instance` released after the factory that made it is already a call into a
//! dangling function table (see `ivory/src/instrument.rs`'s `Hosted`). The
//! controller makes it three deep, and `Drop` runs strictly:
//!
//! ```text
//! setProcessing(false) / setActive(false)   leave the realtime section
//! IConnectionPoint::disconnect  BOTH WAYS   before either end can go
//! IEditController::terminate                owed iff initialize succeeded
//! release the controller                    `editing` is the FIRST field
//! IComponent::terminate                     owed iff initialize succeeded
//! release the component                     then the host context
//! ...and only then may the Module drop      caller's ordering, not ours
//! ```
//!
//! Field order in [`Instance`] is what enforces the middle of that, so moving a
//! declaration is a semantic change and not a formatting one.
//!
//! **Sixth trap, and the one that decides whether the editor is worth having:
//! a knob the user moves does not reach the processor unless the host carries
//! it.** Measured on Pianoteq 9 with its own UI open: dragging the volume
//! slider to minimum changed the rendered audio by nothing at all — a probe C4
//! measured 0.0871 / 0.0817 / 0.0898 RMS across the drag, the same three
//! numbers as a run where nothing was touched. `IComponentHandler::performEdit`
//! had fired; the host had swallowed it. The two halves of a VST3 plugin are
//! separate objects, the controller knows what the user did and the processor
//! does not, and closing that gap is the host's job: `(id, value)` into the
//! next block's `inputParameterChanges`, through the same door the pedal uses.
//! [`ComponentHandler`] and [`Instance::drain_editor_edits`] are that. What
//! makes it easy to miss is that PRESET changes take a different path and
//! arrive without help, so an unforwarding host looks like it works right up
//! until someone touches a control.
//!
//! **Seventh trap: everything the user just did is thrown away on quit unless
//! the host asks for it.** A plugin's state does not persist itself and there
//! is no "save" anywhere in the VST3 API — there is `IComponent::getState`,
//! which writes into a stream the HOST implements, and a host that never calls
//! it hands its user the default piano every launch. The whole of that lives in
//! [`state`](crate::state), including the rewind between `setState` and
//! `setComponentState` that decides whether the plugin's UI agrees with its own
//! sound; [`Instance::save_state`] and [`Instance::load_state`] are the doors.
//!
//! # `process` allocates nothing, and that is a recent sentence
//!
//! Every block used to allocate a `Vec` of channel pointers, an
//! `AudioBusBuffers` vector, one scratch buffer per channel of every output bus
//! past the first (fourteen on Pianoteq), and a `ComWrapper`ed event list —
//! four allocation sites, identical in size every time, under a realtime
//! deadline. `ivory/src/instrument.rs` recorded it as a landmine it could not
//! defuse from where it stood, and with three layered instruments it was three
//! times the size.
//!
//! It is [`Scratch`] now: sized once in [`Instance::create`] from
//! [`Setup::max_block`] and the bus layout, rebound per block. The rule the
//! whole file already lived by ([`ParamChanges`], [`ComponentHandler`]) simply
//! extends to the buffers. See [`Scratch::bind`] for the two things that stay
//! per-block on purpose — the output pointers, because the caller's `Vec`s may
//! have moved, and the zero-fill of the aux buses, because a plugin that sums
//! into its outputs would otherwise accumulate into last block's numbers.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};

use vst3::Steinberg::Vst::IAttributeList_::AttrID;
use vst3::Steinberg::Vst::{
    kNoParamId, AudioBusBuffers, AudioBusBuffers__type0, BusDirections_, BusInfo,
    ControllerNumbers_, Event, Event_, IAttributeList,
    IAttributeListTrait, IAttributeList_iid, IAudioProcessor, IAudioProcessorTrait, IComponent,
    IComponentHandler, IComponentHandlerTrait, IComponentTrait, IConnectionPoint,
    IConnectionPointTrait, IEditController, IEditControllerTrait, IEventList, IEventListTrait,
    IHostApplication, IHostApplicationTrait, IMessage, IMessageTrait, IMessage_iid, IMidiMapping,
    IMidiMappingTrait, IParamValueQueue, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, LegacyMIDICCOutEvent, MediaTypes_, NoteOffEvent, NoteOnEvent, ParamID,
    ParamValue, ProcessData, ProcessModes_, ProcessSetup, SymbolicSampleSizes_, TChar, ViewType,
};
use vst3::Steinberg::{
    int32, int64, kInvalidArgument, kResultFalse, kResultOk, tresult, uint32, FIDString,
    IPluginBaseTrait, IPluginFactoryTrait, IPlugView, TUID,
};
use vst3::{Class, ComPtr, ComWrapper, Interface};

use crate::scan::{ClassInfo, Module};

/// Audio format the instance is set up for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Setup {
    pub sample_rate: f64,
    /// Maximum frames per `process` call. The plugin sizes its internal buffers
    /// from this, so asking for more later is undefined rather than slow.
    pub max_block: i32,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            sample_rate: 48_000.0,
            max_block: 512,
        }
    }
}

/// What a plugin says about one of its buses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bus {
    pub name: String,
    pub channels: i32,
    /// A bus the plugin considers optional. Activating one anyway is harmless;
    /// leaving a *required* one inactive is what produces silence.
    pub aux: bool,
}

/// The host context handed to `initialize`.
///
/// Two jobs: `getName`, so a plugin's error dialogs and preset paths can say who
/// loaded it, and `createInstance`, which is how a plugin asks the host to
/// allocate the `IMessage`/`IAttributeList` objects its two halves talk over.
///
/// **`createInstance` used to refuse, and refusing used to be defensible.** The
/// controller was created for a few microseconds to read the CC table and torn
/// down again, so nothing was ever going to send a message. Now the controller
/// lives as long as the instance so that its editor can be opened, the channel
/// is permanently open, and a host that will not allocate a message is a host
/// whose plugin's UI is decorative. See the module docs, fourth trap.
struct HostApp {
    /// Messages and attribute lists handed out. Not a statistic: it is the only
    /// evidence that a given plugin uses this channel at all, and the answer
    /// differs per plugin. Read by [`HostApp::messages_made`].
    ///
    /// An atomic and not a `Cell`, because **a component is allowed to ask for a
    /// message from the audio thread** while its controller asks from the UI
    /// thread. Neither the counter nor the allocation below is something to want
    /// under a realtime deadline, and that hazard is named in
    /// [`attach_controller`]; a torn counter on top of it would just make the
    /// evidence untrustworthy too.
    made: AtomicU64,
}

impl HostApp {
    fn new() -> Self {
        Self {
            made: AtomicU64::new(0),
        }
    }

    /// How many `IMessage`/`IAttributeList` objects this plugin has asked for.
    ///
    /// Zero from a plugin whose editor works means that plugin keeps its two
    /// halves in sync some other way (most commercial instruments share one
    /// engine object in-process and never send a message at all). Non-zero
    /// means the old refusing implementation would have broken it silently.
    fn messages_made(&self) -> u64 {
        self.made.load(Ordering::Relaxed)
    }
}

impl Class for HostApp {
    type Interfaces = (IHostApplication,);
}

impl IHostApplicationTrait for HostApp {
    unsafe fn getName(&self, name: *mut vst3::Steinberg::Vst::String128) -> tresult {
        if name.is_null() {
            return kInvalidArgument;
        }
        // String128 is [u16; 128] of UTF-16, NUL-terminated. Written by hand
        // rather than with a helper because getting the terminator wrong gives
        // the plugin 128 characters of stack garbage as the host's name, which
        // then appears in its error dialogs.
        let text = "Tangent";
        // SAFETY: `name` is a caller-provided array of 128 TChar.
        let out = unsafe { &mut *name };
        out.fill(0);
        for (i, unit) in text.encode_utf16().take(127).enumerate() {
            out[i] = unit;
        }
        kResultOk
    }

    unsafe fn createInstance(
        &self,
        _cid: *mut TUID,
        iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if obj.is_null() {
            return kInvalidArgument;
        }
        // Null the out-parameter FIRST, on every path. A plugin that checks the
        // pointer instead of the result code would otherwise use whatever was
        // on its stack.
        // SAFETY: `obj` is a caller-provided out-parameter, checked non-null.
        unsafe { *obj = std::ptr::null_mut() };
        if iid.is_null() {
            return kInvalidArgument;
        }
        // The SDK passes the same value as both `cid` and `iid` here, and every
        // host implementation matches on the iid. Only two classes are ever
        // asked for.
        // SAFETY: `iid` is a caller-provided TUID, checked non-null.
        let want = unsafe { *iid };

        if want == IMessage_iid {
            let msg = ComWrapper::new(HostMessage::new());
            let Some(ptr) = msg.to_com_ptr::<IMessage>() else {
                return kResultFalse;
            };
            self.made.fetch_add(1, Ordering::Relaxed);
            // `into_raw` hands over the reference `to_com_ptr` added rather
            // than dropping it: the plugin owns this object now and will
            // release it. Anything else here is a leak or a double free.
            // SAFETY: `obj` was checked non-null above.
            unsafe { *obj = ptr.into_raw().cast() };
            return kResultOk;
        }
        if want == IAttributeList_iid {
            let attrs = ComWrapper::new(HostAttributes::new());
            let Some(ptr) = attrs.to_com_ptr::<IAttributeList>() else {
                return kResultFalse;
            };
            self.made.fetch_add(1, Ordering::Relaxed);
            // SAFETY: `obj` was checked non-null above.
            unsafe { *obj = ptr.into_raw().cast() };
            return kResultOk;
        }
        // Anything else is a class this host genuinely does not implement, and
        // `kResultFalse` with a nulled out-parameter is the honest answer.
        kResultFalse
    }
}

// ── The component-to-controller channel, which the host has to furnish ───────

/// An `IMessage`, allocated by the host at the plugin's request.
///
/// A plugin's processor and controller are two objects that may not touch each
/// other's memory, so everything one tells the other travels as a message: a
/// name and a bag of typed attributes. **The plugin allocates neither.** It
/// calls `IHostApplication::createInstance`, gets one of these, fills it in, and
/// hands it to `IConnectionPoint::notify` on the far end — which is a direct
/// call into the other half, on whatever thread the sender happened to be on.
///
/// `RefCell` and not `Cell`: the values are `Vec`s. The borrows below are all
/// leaf borrows that call nothing, so there is no path on which one is held
/// while the plugin re-enters — which matters because a `RefCell` panic across
/// this ABI is undefined behaviour, not a backtrace.
struct HostMessage {
    /// NUL-terminated, because `getMessageID` returns a `const char*` into it.
    /// The pointer must stay valid until the id is replaced or the message
    /// dies, which is why this is owned storage and not a borrowed `&str`.
    id: RefCell<Vec<u8>>,
    attrs: ComWrapper<HostAttributes>,
}

impl HostMessage {
    fn new() -> Self {
        Self {
            id: RefCell::new(vec![0]),
            attrs: ComWrapper::new(HostAttributes::new()),
        }
    }
}

impl Class for HostMessage {
    type Interfaces = (IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> FIDString {
        // A pointer into the `Vec`'s heap allocation, which outlives the borrow
        // guard and is only invalidated by `setMessageID`. This is precisely
        // what the SDK's own `HostMessage` does.
        self.id.borrow().as_ptr().cast()
    }

    unsafe fn setMessageID(&self, id: FIDString) {
        let mut slot = self.id.borrow_mut();
        slot.clear();
        if !id.is_null() {
            // SAFETY: the plugin passes a NUL-terminated C string, which is the
            // only thing an FIDString can be.
            let bytes = unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes();
            slot.extend_from_slice(bytes);
        }
        slot.push(0);
    }

    unsafe fn getAttributes(&self) -> *mut IAttributeList {
        // BORROWED, not owned: `getAttributes` does not add a reference and the
        // plugin must not release what it gets back. `as_com_ref` is the only
        // accessor that touches no refcount.
        self.attrs
            .as_com_ref::<IAttributeList>()
            .map(|r| r.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }
}

/// One value in a [`HostAttributes`].
enum Attr {
    Int(int64),
    Float(f64),
    /// UTF-16, NOT NUL-terminated here — the terminator is added on the way out
    /// in `getString`, where the caller's buffer size is known.
    Str(Vec<TChar>),
    Bin(Vec<u8>),
}

/// The `IAttributeList` inside a [`HostMessage`]: a small typed map keyed by a C
/// string.
///
/// A `Vec` of pairs rather than a `HashMap`. A message carries a handful of
/// attributes — the SDK's own examples carry one — and a linear scan over four
/// entries beats hashing, with the far more useful property that no key is ever
/// hashed with a pointer's *address* by accident.
struct HostAttributes {
    entries: RefCell<Vec<(Vec<u8>, Attr)>>,
}

impl HostAttributes {
    fn new() -> Self {
        Self {
            entries: RefCell::new(Vec::new()),
        }
    }

    /// Copy an `AttrID` out of the plugin's memory. `None` for a null key,
    /// which is the only thing that can go wrong here.
    ///
    /// # Safety
    ///
    /// `id` must be null or a NUL-terminated C string.
    unsafe fn key(id: AttrID) -> Option<Vec<u8>> {
        if id.is_null() {
            return None;
        }
        // SAFETY: the caller guarantees a NUL-terminated string.
        Some(unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes().to_vec())
    }

    /// Insert or replace.
    fn put(&self, key: Vec<u8>, value: Attr) {
        let mut entries = self.entries.borrow_mut();
        match entries.iter_mut().find(|(k, _)| *k == key) {
            Some(slot) => slot.1 = value,
            None => entries.push((key, value)),
        }
    }
}

impl Class for HostAttributes {
    type Interfaces = (IAttributeList,);
}

impl IAttributeListTrait for HostAttributes {
    unsafe fn setInt(&self, id: AttrID, value: int64) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID, which is a C string.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        self.put(key, Attr::Int(value));
        kResultOk
    }

    unsafe fn getInt(&self, id: AttrID, value: *mut int64) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        if value.is_null() {
            return kInvalidArgument;
        }
        let entries = self.entries.borrow();
        let Some((_, Attr::Int(v))) = entries.iter().find(|(k, _)| *k == key) else {
            return kResultFalse;
        };
        // SAFETY: caller-provided out-parameter, checked non-null.
        unsafe { *value = *v };
        kResultOk
    }

    unsafe fn setFloat(&self, id: AttrID, value: f64) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        self.put(key, Attr::Float(value));
        kResultOk
    }

    unsafe fn getFloat(&self, id: AttrID, value: *mut f64) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        if value.is_null() {
            return kInvalidArgument;
        }
        let entries = self.entries.borrow();
        let Some((_, Attr::Float(v))) = entries.iter().find(|(k, _)| *k == key) else {
            return kResultFalse;
        };
        // SAFETY: caller-provided out-parameter, checked non-null.
        unsafe { *value = *v };
        kResultOk
    }

    unsafe fn setString(&self, id: AttrID, string: *const TChar) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        if string.is_null() {
            return kInvalidArgument;
        }
        // TChar is UTF-16 and NUL-terminated; there is no length to be had any
        // other way.
        let mut out: Vec<TChar> = Vec::new();
        let mut at = 0usize;
        loop {
            // SAFETY: walking a NUL-terminated UTF-16 string the plugin owns.
            let unit = unsafe { *string.add(at) };
            if unit == 0 {
                break;
            }
            out.push(unit);
            at += 1;
        }
        self.put(key, Attr::Str(out));
        kResultOk
    }

    unsafe fn getString(&self, id: AttrID, string: *mut TChar, size_in_bytes: uint32) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        // `sizeInBytes`, not `sizeInCharacters` — the single easiest way to
        // write one unit past the end of a plugin's buffer.
        let units = (size_in_bytes as usize) / size_of::<TChar>();
        if string.is_null() || units == 0 {
            return kInvalidArgument;
        }
        let entries = self.entries.borrow();
        let Some((_, Attr::Str(v))) = entries.iter().find(|(k, _)| *k == key) else {
            return kResultFalse;
        };
        let n = v.len().min(units - 1);
        for (i, unit) in v.iter().take(n).enumerate() {
            // SAFETY: `i < n <= units - 1`, and the buffer holds `units`.
            unsafe { *string.add(i) = *unit };
        }
        // SAFETY: `n <= units - 1`, so this is the last legal slot at worst.
        unsafe { *string.add(n) = 0 };
        kResultOk
    }

    unsafe fn setBinary(&self, id: AttrID, data: *const c_void, size_in_bytes: uint32) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        if data.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: the plugin says `data` is readable for `size_in_bytes`.
        let bytes =
            unsafe { std::slice::from_raw_parts(data.cast::<u8>(), size_in_bytes as usize) };
        self.put(key, Attr::Bin(bytes.to_vec()));
        kResultOk
    }

    unsafe fn getBinary(
        &self,
        id: AttrID,
        data: *mut *const c_void,
        size_in_bytes: *mut uint32,
    ) -> tresult {
        // SAFETY: `id` is a plugin-provided AttrID.
        let Some(key) = (unsafe { Self::key(id) }) else {
            return kInvalidArgument;
        };
        if data.is_null() || size_in_bytes.is_null() {
            return kInvalidArgument;
        }
        let entries = self.entries.borrow();
        let Some((_, Attr::Bin(v))) = entries.iter().find(|(k, _)| *k == key) else {
            return kResultFalse;
        };
        // A pointer INTO this list's own storage, exactly as the SDK's
        // implementation does: the contract is that it stays valid until the
        // attribute is replaced or the list dies. Copying instead would need an
        // owner for the copy and there is nowhere to put one.
        // SAFETY: both are caller-provided out-parameters, checked non-null.
        unsafe {
            *data = v.as_ptr().cast();
            *size_in_bytes = v.len() as uint32;
        }
        kResultOk
    }
}

/// Parameters the editor has moved, allocated once and never grown.
///
/// Sixty-four distinct parameters touched in one session of knob-twiddling is
/// already generous — a human moves one at a time — and the cost of being wrong
/// is that the sixty-fifth control in a plugin's UI stops working, which is
/// visible. The cost of growing this instead would be an allocation on whichever
/// thread got there first.
const MAX_EDIT_SLOTS: usize = 64;

/// One parameter's latest value, as the editor last set it.
///
/// **Latest-wins and not a queue.** A knob dragged across a block produces
/// dozens of `performEdit` calls and the processor only needs where the knob
/// ended up: nobody is recording sample-accurate automation from a mouse. That
/// turns a ring buffer with all its ordering into three atomics, and makes an
/// edit that arrives while the audio thread is mid-block simply be the value
/// delivered next block rather than a lost one.
struct EditSlot {
    /// `kNoParamId` until claimed. **Written once**, by the thread that owns the
    /// editor, and never changed — which is what lets the audio thread read it
    /// without any ordering beyond the `used` counter's release.
    id: AtomicU32,
    /// The normalised value, as its bit pattern. There is no `AtomicF64`.
    value: AtomicU64,
    /// Bumped by every edit. The audio thread remembers the last one it
    /// delivered; equal means nothing new, which is the common case and costs
    /// one relaxed load per slot per block.
    seq: AtomicU64,
}

/// The `IComponentHandler` the edit controller is given.
///
/// Two jobs, and the second one is why this file's editor is usable at all.
///
/// **A controller with no handler may refuse to build an editor**, and several
/// plugins do. Being non-null is most of what this is for.
///
/// **`performEdit` is the host's cue to carry a GUI parameter move to the
/// PROCESSOR**, and this was the sixth trap, found by measurement:
///
/// > Pianoteq 9's editor was opened, its volume slider dragged to minimum, and
/// > the rendered audio did not change **at all** — a probe C4 measured
/// > 0.0871 / 0.0817 / 0.0898 RMS across the drag, byte-identical to the same
/// > probes in a run where nothing was touched. `performEdit` had fired. The
/// > host had swallowed it.
///
/// The two halves of a VST3 plugin are separate objects that may not touch each
/// other's memory. When the user moves a knob, the *controller* knows and the
/// *processor* does not, and closing that gap is the host's job and nobody
/// else's: take the `(id, value)` and put it in the next block's
/// `inputParameterChanges`, which is the same door the sustain pedal comes
/// through. A host that skips it opens an editor whose knobs move and whose
/// sound never changes, which is a worse outcome than having no editor, because
/// it looks like it works.
///
/// Preset changes are a separate path and do arrive without help — Pianoteq
/// announces them with `restartComponent` and both halves reload — which is
/// exactly why this was so easy to miss: change a preset, hear it change,
/// conclude the editor is wired up, ship it.
///
/// The table below is read from the audio thread inside
/// [`Instance::process_with_controls`], so it allocates nothing, locks nothing
/// and cannot fail.
struct ComponentHandler {
    /// Allocated once, in [`ComponentHandler::new`]. Never resized.
    slots: Vec<EditSlot>,
    /// How many of `slots` are claimed. Published with `Release` after the id
    /// inside the slot is written, read with `Acquire`, so a slot the audio
    /// thread can see is a slot whose id it can trust.
    used: AtomicUsize,
    edits: AtomicU64,
    restarts: AtomicU64,
    /// Edits that arrived after every slot was claimed by a different
    /// parameter. Counted rather than dropped silently, because the symptom is
    /// "one control in this plugin does nothing".
    overflow: AtomicU64,
}

impl ComponentHandler {
    fn new() -> Self {
        Self {
            slots: (0..MAX_EDIT_SLOTS)
                .map(|_| EditSlot {
                    id: AtomicU32::new(kNoParamId),
                    value: AtomicU64::new(0),
                    seq: AtomicU64::new(0),
                })
                .collect(),
            used: AtomicUsize::new(0),
            edits: AtomicU64::new(0),
            restarts: AtomicU64::new(0),
            overflow: AtomicU64::new(0),
        }
    }
}

impl Class for ComponentHandler {
    type Interfaces = (IComponentHandler,);
}

#[cfg(test)]
impl ComponentHandler {
    fn editor_overflow_for_test(&self) -> u64 {
        self.overflow.load(Ordering::Relaxed)
    }
}

impl IComponentHandlerTrait for ComponentHandler {
    unsafe fn beginEdit(&self, _id: ParamID) -> tresult {
        // The host's cue to open an automation region. Tangent records audio,
        // not automation, so there is nothing to open — and the value still
        // arrives, because `performEdit` does not depend on this.
        kResultOk
    }

    unsafe fn performEdit(&self, id: ParamID, value: ParamValue) -> tresult {
        self.edits.fetch_add(1, Ordering::Relaxed);
        let used = self.used.load(Ordering::Acquire);
        // Already claimed? Then this is the common case — a knob being dragged
        // — and it is two stores.
        for slot in self.slots.iter().take(used) {
            if slot.id.load(Ordering::Relaxed) == id {
                slot.value.store(value.to_bits(), Ordering::Relaxed);
                // Release, and after the value: the audio thread reads this
                // first and everything written before it is then visible.
                slot.seq.fetch_add(1, Ordering::Release);
                return kResultOk;
            }
        }
        let Some(slot) = self.slots.get(used) else {
            self.overflow.fetch_add(1, Ordering::Relaxed);
            return kResultFalse;
        };
        slot.id.store(id, Ordering::Relaxed);
        slot.value.store(value.to_bits(), Ordering::Relaxed);
        slot.seq.store(1, Ordering::Relaxed);
        // Publishing the count LAST is what makes the three stores above safe
        // to read without any further ordering.
        self.used.store(used + 1, Ordering::Release);
        kResultOk
    }

    unsafe fn endEdit(&self, _id: ParamID) -> tresult {
        kResultOk
    }

    unsafe fn restartComponent(&self, _flags: int32) -> tresult {
        // `kResultOk` and not `kNotImplemented`: a plugin that is told its
        // restart request failed can and does take that as a fatal condition,
        // and every flag it could ask for here (parameter values changed,
        // latency changed, the CC map changed) is a refresh this host performs
        // by reading fresh values on the next block anyway.
        //
        // This is also the path a PRESET change takes. It needs nothing from
        // the host, which is precisely how a host with no `performEdit`
        // forwarding can look like it works.
        self.restarts.fetch_add(1, Ordering::Relaxed);
        kResultOk
    }
}

/// A live plugin instance.
///
/// Not `Send`: VST3 requires that the main-thread methods be called from one
/// thread, and `process` is the only one that may be called from another. Making
/// the whole thing `!Send` and handing the audio thread a narrower type later is
/// the safe direction to be wrong in.
///
/// **And `ComPtr` is not what makes it `!Send`.** The `vst3` bindings put an
/// unconditional `unsafe impl Send`/`Sync` on every one of their interface
/// types, so every `ComPtr` in this struct is `Send`. Until [`NOT_SEND`] was
/// added this type was `!Send` purely because a `Cell` inside `HostApp` made
/// `ComWrapper<HostApp>` non-`Sync` — an accident, one refactor deep, holding up
/// the whole threading argument in `ivory/src/instrument.rs`. The refactor that
/// would have broken it arrived in the same change as the editor, because a
/// component may allocate a message from the audio thread and that `Cell` had to
/// become an atomic. See the test.
pub struct Instance {
    /// The edit controller and its connection to the component.
    ///
    /// **Declared FIRST, and that is the teardown order rather than a
    /// preference**: the controller must be released before the component, and
    /// Rust drops fields in declaration order. `None` for a plugin that exports
    /// no controller class at all, which is legal and means no editor and no CC
    /// map.
    editing: Option<Editing>,
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    /// The `IComponentHandler` the controller was given. Held so it outlives
    /// the controller that is holding a raw pointer to it, which is why it is
    /// declared after `editing`.
    handler: ComWrapper<ComponentHandler>,
    _host: ComWrapper<HostApp>,
    setup: Setup,
    active: bool,
    processing: bool,
    audio_out: Vec<Bus>,
    /// The audio the plugin wants FED to it. Empty on an instrument.
    audio_in: Vec<Bus>,
    event_in: Vec<Bus>,
    /// CC-to-parameter, `channel * CTRL_COUNT + controller`, read once at
    /// creation. `kNoParamId` means this plugin published nothing for it.
    midi_map: Vec<ParamID>,
    /// The host-side `IParameterChanges`, and the interface pointer handed to
    /// the plugin. Both are held so that `process` neither allocates nor
    /// touches a refcount: `to_com_ptr` clones an `Arc`, which is an atomic
    /// increment on the audio thread for a pointer that never changes.
    changes: ComWrapper<ParamChanges>,
    changes_ptr: ComPtr<IParameterChanges>,
    /// The buffers, pointer arrays and event list `process` needs, allocated
    /// once. Unlike `editing` its position in this struct is not load-bearing:
    /// it owns nothing the plugin has to be told about at teardown.
    scratch: Scratch,
    /// Whether the plugin builds an editor, answered lazily and remembered.
    ///
    /// Lazily because the only way to ask is to build one and throw it away —
    /// there is no `hasEditor` anywhere in VST3 — and building Pianoteq's UI
    /// costs real milliseconds that a load which nobody will ever open an editor
    /// for should not pay. Remembered because the answer cannot change.
    has_editor: Cell<Option<bool>>,
    /// The `seq` this instance has already delivered for each of the handler's
    /// edit slots. Sized once, in [`Instance::create`], so the drain in
    /// `process_with_controls` reads and writes an array that was allocated
    /// before the audio thread ever saw this object.
    edit_seen: Vec<u64>,
    /// Says out loud what this type has always relied on. See the struct docs.
    not_send: NotSend,
}

/// `PhantomData<*const ()>`, named, because an unexplained one in a struct reads
/// like leftovers.
type NotSend = std::marker::PhantomData<*const ()>;

/// The value of a [`NotSend`], so `Instance::create` reads as prose.
const NOT_SEND: NotSend = std::marker::PhantomData;

/// The edit controller half, alive for the instance's whole life.
///
/// It used to live for the length of one function (`read_midi_map`), because the
/// CC table was all anybody wanted from it. `IEditController::createView` is the
/// only way to reach a plugin's own UI, so it stays.
struct Editing {
    controller: ComPtr<IEditController>,
    /// The two ends of the connection, held so `disconnect` can undo exactly
    /// what `connect` did rather than re-deriving the pointers at teardown.
    /// `None` for a single-component plugin, where there is one object and
    /// nothing to connect.
    points: Option<(ComPtr<IConnectionPoint>, ComPtr<IConnectionPoint>)>,
    /// `IEditController::initialize` succeeded, so `terminate` is owed exactly
    /// once. False for a single-component plugin: there the controller IS the
    /// component and the component's own `terminate` covers it — calling it
    /// twice is what "the plugin crashed on unload" looks like.
    initialised: bool,
}

impl Instance {
    /// Create and fully prepare an instance of `class` from `module`.
    ///
    /// On any failure the partially-built instance is torn down in the right
    /// order rather than leaked, which matters because a plugin left `active`
    /// with no owner keeps its DSP allocation and, for some plugins, a thread.
    pub fn create(module: &Module, class: &ClassInfo, setup: Setup) -> Result<Self, String> {
        if !class.is_audio_module() {
            return Err(format!(
                "{} is a {}, not an Audio Module Class",
                class.name, class.category
            ));
        }
        // **An effect refused HERE, with a reason.**
        //
        // **The "is it an instrument" check is NOT here.** It was, and it was
        // in the wrong place: `create` is how this crate instantiates any
        // VST3, and once the insert racks existed it was refusing every effect
        // in the app — a Pro-R dropped on an insert answered "Pro-R 2 is an
        // effect, not an instrument", which is a correct sentence about the
        // wrong question.
        //
        // The rule belongs to the caller that has a rule: an instrument slot
        // is fed MIDI and refuses an effect, an insert is fed audio and
        // refuses an instrument. Both refusals now sit beside the loader they
        // are about — see `Engine::load` and `Engine::load_insert` — and
        // `class.kind()` is public so both can ask.

        let host = ComWrapper::new(HostApp::new());
        let host_unknown = host
            .to_com_ptr::<IHostApplication>()
            .ok_or_else(|| "could not build the host context".to_string())?;

        let mut raw: *mut c_void = std::ptr::null_mut();
        let mut cid: TUID = bytes_to_tuid(class.cid);
        // SAFETY: `cid` and the IID are valid for the call; `raw` is a valid
        // out-parameter. The factory is alive for `module`'s lifetime.
        let result = unsafe {
            module.factory().createInstance(
                cid.as_mut_ptr(),
                IComponent::IID.as_ptr() as *const _ as *mut _,
                &mut raw,
            )
        };
        if result != kResultOk || raw.is_null() {
            return Err(format!(
                "{} refused to instantiate (tresult {result})",
                class.name
            ));
        }
        // SAFETY: `createInstance` returns an object with one reference already
        // added, which `from_raw` takes ownership of.
        let component = unsafe { ComPtr::<IComponent>::from_raw(raw.cast()) }
            .ok_or_else(|| "instantiated a null component".to_string())?;

        // SAFETY: freshly created component; the host context outlives it,
        // because `Instance` owns both and drops the component first.
        let result = unsafe { component.initialize(host_unknown.as_ptr().cast()) };
        if result != kResultOk {
            return Err(format!("{}: initialize failed ({result})", class.name));
        }

        let Some(processor) = component.cast::<IAudioProcessor>() else {
            // SAFETY: initialize succeeded, so terminate is owed.
            unsafe { component.terminate() };
            return Err(format!(
                "{} is an Audio Module Class but has no IAudioProcessor",
                class.name
            ));
        };

        // Before setupProcessing, matching the SDK's own host sequence: the
        // controller is created, given a handler and connected while the
        // component is initialised but not yet active. Unlike every version of
        // this file before the editor landed, it is still here when this
        // returns.
        let handler = ComWrapper::new(ComponentHandler::new());
        let editing = attach_controller(module, &component, &host_unknown, &handler);
        let midi_map = read_midi_map(&component, editing.as_ref());

        let changes = ComWrapper::new(ParamChanges::new());
        let Some(changes_ptr) = changes.to_com_ptr::<IParameterChanges>() else {
            // The controller goes down first and by hand: `editing` is a local
            // here, not yet a field, so nothing is arranging its teardown.
            release_controller(editing);
            // SAFETY: initialize succeeded, so terminate is owed.
            unsafe { component.terminate() };
            return Err("could not build the parameter change list".to_string());
        };

        // The bus layout is read BEFORE the instance exists, because the
        // scratch below is sized from it and there is no honest placeholder for
        // a scratch: an `Instance` whose buffers do not match its buses is one
        // `process` call away from handing a plugin a short array.
        let audio_out = read_buses(
            &component,
            MediaTypes_::kAudio as i32,
            BusDirections_::kOutput as i32,
        );
        let event_in = read_buses(
            &component,
            MediaTypes_::kEvent as i32,
            BusDirections_::kInput as i32,
        );
        // **And the audio INPUTS, which an instrument does not have and an
        // effect is nothing without.** Read here with the rest of the layout
        // so the scratch can be sized for them; a plugin handed `numInputs: 0`
        // processes silence and returns `kResultOk`, which is a reverb that
        // appears to load and does nothing.
        let audio_in = read_buses(
            &component,
            MediaTypes_::kAudio as i32,
            BusDirections_::kInput as i32,
        );
        // **The last allocation this instance makes.** Everything `process`
        // needs is sized here, from the layout the plugin has just described and
        // from `setup.max_block`; see [`Scratch`].
        let scratch = match Scratch::new(setup.max_block, &audio_out, &audio_in) {
            Ok(s) => s,
            Err(why) => {
                release_controller(editing);
                // SAFETY: initialize succeeded, so terminate is owed.
                unsafe { component.terminate() };
                return Err(why);
            }
        };

        let mut me = Self {
            editing,
            component,
            processor,
            handler,
            _host: host,
            setup,
            active: false,
            processing: false,
            audio_out,
            audio_in,
            event_in,
            midi_map,
            changes,
            changes_ptr,
            scratch,
            has_editor: Cell::new(None),
            edit_seen: vec![0; MAX_EDIT_SLOTS],
            not_send: NOT_SEND,
        };

        me.setup_processing()?;
        me.activate_all_buses();
        me.set_active(true)?;
        Ok(me)
    }

    fn setup_processing(&mut self) -> Result<(), String> {
        let mut s = ProcessSetup {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            maxSamplesPerBlock: self.setup.max_block,
            sampleRate: self.setup.sample_rate,
        };
        // SAFETY: `s` is fully initialised. This MUST precede setActive: a
        // plugin allocates its buffers from maxSamplesPerBlock when it goes
        // active, so setting it afterwards is ignored at best.
        let r = unsafe { self.processor.setupProcessing(&mut s) };
        if r != kResultOk {
            return Err(format!(
                "setupProcessing refused {} Hz / {} frames ({r})",
                self.setup.sample_rate, self.setup.max_block
            ));
        }
        Ok(())
    }

    /// Activate every bus the plugin declares.
    ///
    /// Deliberately not selective. An inactive bus is not an error and produces
    /// no audio, so a host that activates only what it thinks it needs gets
    /// silence from any plugin whose main output is not bus 0 — and diagnoses it
    /// as a broken plugin.
    fn activate_all_buses(&self) {
        for (media, dir, buses) in [
            (MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32, &self.audio_out),
            (MediaTypes_::kEvent as i32, BusDirections_::kInput as i32, &self.event_in),
        ] {
            for i in 0..buses.len() as i32 {
                // SAFETY: index is in range by construction.
                unsafe { self.component.activateBus(media, dir, i, 1) };
            }
        }
        // Audio INPUTS are activated too, when present: an instrument usually
        // has none, but an effect refuses to process without them.
        // SAFETY: reading a count, then activating in range.
        let ins = unsafe {
            self.component
                .getBusCount(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32)
        };
        for i in 0..ins {
            unsafe {
                self.component
                    .activateBus(MediaTypes_::kAudio as i32, BusDirections_::kInput as i32, i, 1)
            };
        }
    }

    fn set_active(&mut self, on: bool) -> Result<(), String> {
        if self.active == on {
            return Ok(());
        }
        // SAFETY: the component is initialised.
        let r = unsafe { self.component.setActive(u8::from(on)) };
        if r != kResultOk {
            return Err(format!("setActive({on}) failed ({r})"));
        }
        self.active = on;
        Ok(())
    }

    /// Enter or leave the realtime section.
    pub fn set_processing(&mut self, on: bool) -> Result<(), String> {
        if self.processing == on {
            return Ok(());
        }
        // SAFETY: active component.
        let r = unsafe { self.processor.setProcessing(u8::from(on)) };
        // Some plugins return kNotImplemented here and process perfectly well;
        // the SDK treats the call as advisory. Only a hard failure is an error.
        if r != kResultOk && r != kResultFalse {
            self.processing = on;
            return Ok(());
        }
        self.processing = on;
        Ok(())
    }

    /// The audio buses this plugin wants fed. Empty on an instrument, which is
    /// what tells the two apart at the point it matters.
    pub fn audio_inputs(&self) -> &[Bus] {
        &self.audio_in
    }

    /// Run a block THROUGH the plugin: audio in, audio out.
    ///
    /// **The effect path, and the only one that fills `ProcessData::inputs`.**
    /// [`process`](Instance::process) leaves them null because an instrument
    /// makes its sound from notes; an effect handed nothing processes silence
    /// and reports success, which is a reverb that loads and does nothing.
    ///
    /// `input` is one `Vec` per channel, at least `frames` long. A plugin
    /// wanting more channels than it is given has the last one repeated —
    /// mono into a stereo reverb is the ordinary case and refusing it would be
    /// a refusal nobody could act on.
    pub fn process_effect(
        &mut self,
        input: &[Vec<f32>],
        frames: usize,
        out: &mut [Vec<f32>],
    ) -> Result<usize, String> {
        self.process_through(&[], &[], input, frames, out)
            .map(|r| r.frames)
    }

    pub fn audio_outputs(&self) -> &[Bus] {
        &self.audio_out
    }

    pub fn event_inputs(&self) -> &[Bus] {
        &self.event_in
    }

    pub fn setup(&self) -> Setup {
        self.setup
    }

    /// The parameter this plugin has wired `controller` on `channel` to, if it
    /// published one.
    ///
    /// An array lookup, safe to call from the audio thread — the table was read
    /// at creation precisely so that nothing here calls into the plugin.
    pub fn control_param(&self, channel: i16, controller: i16) -> Option<ParamID> {
        let (Ok(ch), Ok(cc)) = (usize::try_from(channel), usize::try_from(controller)) else {
            return None;
        };
        if ch >= MIDI_CHANNELS || cc >= CTRL_COUNT {
            return None;
        }
        match self.midi_map.get(ch * CTRL_COUNT + cc) {
            Some(&id) if id != kNoParamId => Some(id),
            _ => None,
        }
    }

    /// Whether this plugin published any control mapping at all.
    ///
    /// `false` means every [`Control`] handed to [`Instance::process_with_controls`]
    /// will be counted in [`Rendered::unmapped`], and that the pedal almost
    /// certainly does nothing on this instrument.
    pub fn maps_controls(&self) -> bool {
        self.midi_map.iter().any(|id| *id != kNoParamId)
    }

    // ── the editor ──────────────────────────────────────────────────────────

    /// Whether this plugin offers an editor at all. Some do not.
    ///
    /// **Main thread only**, and the first call is not free: VST3 has no
    /// `hasEditor`, so the only honest way to answer is to ask the controller
    /// for a view and release it again. The answer is remembered, so the cost
    /// is paid once per instance and only if somebody asks.
    ///
    /// `false` is the right thing to grey a menu row on. It is also what a
    /// plugin with no controller class returns, without calling into the plugin
    /// at all.
    pub fn has_editor(&self) -> bool {
        if let Some(known) = self.has_editor.get() {
            return known;
        }
        let answer = match &self.editing {
            Some(e) => probe_editor(&e.controller),
            None => false,
        };
        self.has_editor.set(Some(answer));
        answer
    }

    /// A main-thread reference to the edit controller, for [`crate::Editor`].
    ///
    /// # Why this exists rather than `Editor::open(&self)` being enough
    ///
    /// In the app the `Instance` is *inside the audio callback* by the time a
    /// user asks for its editor — it was moved there, behind an `unsafe impl
    /// Send`, and there is no `&Instance` to be had from the UI thread at all.
    /// Taking a reference to the controller is what closes that gap: it is a COM
    /// object with a reference count, so a second reference costs an atomic
    /// increment, can be taken while the instance is still on this thread, and
    /// stays behind when the instance leaves.
    ///
    /// Nothing about that is shared mutable state. The audio thread calls
    /// exactly one method on the whole plugin (`IAudioProcessor::process`) and
    /// none of them on the controller; every controller call in this crate
    /// happens on the thread that owns the window. See `PluginBox` in
    /// `ivory/src/instrument.rs`, condition 3.
    ///
    /// The handle must be dropped before the `Instance` is: the instance's
    /// `Drop` calls `IEditController::terminate`, and a handle outliving that
    /// is a live pointer to a terminated object.
    pub fn editor_handle(&self) -> Option<crate::editor::EditorHandle> {
        let e = self.editing.as_ref()?;
        Some(crate::editor::EditorHandle::new(e.controller.clone()))
    }

    // ── the state ───────────────────────────────────────────────────────────

    /// The processor's state and the controller's, as opaque bytes.
    ///
    /// **Main thread only.** Everything the user did in the plugin's own editor
    /// is in here — the preset, the piano, every knob — and nothing else in this
    /// crate can reproduce it: there is no way to enumerate a plugin's state and
    /// no way to rebuild it from parameters, because most of it is not
    /// parameters. See [`crate::state`] for the container the two blobs travel
    /// in and for why they travel in one at all.
    ///
    /// A few kilobytes for an instrument that names a preset; a sampler that
    /// embeds its content would be larger, and [`crate::MAX_STATE_BYTES`] is the
    /// ceiling either way. Measure before putting the result in a JSON file.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        crate::state::save(&self.component, self.editing.as_ref().map(|e| &e.controller))
    }

    /// Restore what [`Instance::save_state`] returned.
    ///
    /// **Before the instrument has rendered anything, and this is enforced.**
    /// The order is create, restore, warm up, and it is not a preference:
    ///
    /// * a plugin reloads samples when its preset changes, so state that
    ///   arrives after the warm-up starts a NEW load and the gate in
    ///   [`crate::ready`] has just finished waiting for the wrong one — the
    ///   instrument is declared ready and then goes quiet, which is the exact
    ///   failure that module exists to prevent;
    /// * the warm-up renders and probes, so state that arrives after it has
    ///   already been heard: the first thing out of the speakers is the default
    ///   piano.
    ///
    /// `process` latches the plugin into its realtime section, so "has rendered"
    /// is a thing this type already knows, and restoring after it is refused
    /// rather than half-applied.
    ///
    /// **Wrong or corrupt bytes are an `Err` and never reach the plugin.** They
    /// come off disk and out of a settings file a user can edit; `setState` with
    /// somebody else's bytes is undefined behaviour wearing a `tresult`.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), String> {
        if self.processing {
            return Err(
                "the instrument has already started rendering; state must be restored \
                 between creating it and warming it up"
                    .to_string(),
            );
        }
        crate::state::load(
            &self.component,
            self.editing.as_ref().map(|e| &e.controller),
            bytes,
        )
    }

    /// A main-thread reference to both halves, for saving state after this
    /// instance has been handed to the audio thread.
    ///
    /// The same trick as [`Instance::editor_handle`] and for the same reason:
    /// the app has no `&Instance` on this thread. See [`crate::StateHandle`],
    /// which also argues why `getState` is safe while `process` is running.
    ///
    /// The handle must be dropped before the `Instance` is.
    pub fn state_handle(&self) -> crate::state::StateHandle {
        crate::state::StateHandle::new(
            self.component.clone(),
            self.editing.as_ref().map(|e| e.controller.clone()),
        )
    }

    /// `IMessage` objects this plugin has asked the host to allocate.
    ///
    /// Diagnostic. Non-zero means the plugin genuinely uses the
    /// component-to-controller channel, and therefore that the host's old
    /// refusing `createInstance` would have broken it. See the module docs,
    /// fourth trap.
    pub fn messages_made(&self) -> u64 {
        self._host.messages_made()
    }

    /// `IComponentHandler::performEdit` calls the editor has made, and
    /// `restartComponent` calls.
    ///
    /// Diagnostic. A rising first number with an unchanging sound is the exact
    /// symptom [`Instance::drain_editor_edits`] exists to cure, so it is worth
    /// being able to see.
    pub fn editor_edits(&self) -> (u64, u64) {
        (
            self.handler.edits.load(Ordering::Relaxed),
            self.handler.restarts.load(Ordering::Relaxed),
        )
    }

    /// Parameter moves that arrived with every edit slot claimed by some other
    /// parameter.
    ///
    /// Non-zero means a plugin with more than [`MAX_EDIT_SLOTS`] distinct
    /// controls has had one of them stop responding, which is a thing a user
    /// would report as "this knob does nothing".
    pub fn editor_overflow(&self) -> u64 {
        self.handler.overflow.load(Ordering::Relaxed)
    }

    /// Render `frames` of audio, delivering `events` at their sample offsets.
    ///
    /// `out` is filled per channel of the FIRST audio output bus, which is the
    /// main output by VST3 convention. Returns the number of frames written.
    pub fn process(
        &mut self,
        events: &[Note],
        frames: usize,
        out: &mut [Vec<f32>],
    ) -> Result<usize, String> {
        self.process_with_controls(events, &[], frames, out)
            .map(|r| r.frames)
    }

    /// As [`Instance::process`], and deliver `controls` too.
    ///
    /// This is the only path a sustain pedal has. Each control is looked up in
    /// the table read at creation and pushed into the preallocated
    /// [`ParamChanges`] as a `(sampleOffset, 0.0..=1.0)` point; one the plugin
    /// published no mapping for is counted in [`Rendered::unmapped`] and offered
    /// as a legacy MIDI CC event instead, which is the only other door VST3
    /// leaves open and which most instruments ignore.
    pub fn process_with_controls(
        &mut self,
        events: &[Note],
        controls: &[Control],
        frames: usize,
        out: &mut [Vec<f32>],
    ) -> Result<Rendered, String> {
        self.process_through(events, controls, &[], frames, out)
    }

    /// The one `process` call. Notes, controls, audio in, audio out.
    ///
    /// **Both paths go through here** so that an instrument and an effect
    /// cannot drift apart in how they set up a block — the parameter changes,
    /// the event list and the output binding are the same work, and the only
    /// difference is whether `ProcessData::inputs` is filled.
    fn process_through(
        &mut self,
        events: &[Note],
        controls: &[Control],
        input: &[Vec<f32>],
        frames: usize,
        out: &mut [Vec<f32>],
    ) -> Result<Rendered, String> {
        if frames > self.setup.max_block as usize {
            return Err(format!(
                "asked for {frames} frames but the plugin was set up for {}",
                self.setup.max_block
            ));
        }
        let Some(main) = self.audio_out.first() else {
            return Err("plugin has no audio output bus".into());
        };
        let channels = main.channels.max(0) as usize;
        if out.len() < channels {
            return Err(format!("need {channels} output channels, got {}", out.len()));
        }
        self.set_processing(true)?;

        // The caller's buffers, not ours: `resize` allocates only if this `Vec`
        // has never held a block, which is why `ivory/src/instrument.rs`
        // pre-grows them to `MAX_BLOCK` at load time. Everything below this line
        // is allocation-free unconditionally; this line is allocation-free if
        // the caller did its half.
        for ch in out.iter_mut().take(channels) {
            ch.clear();
            ch.resize(frames, 0.0);
        }

        // ── the events ──────────────────────────────────────────────────────
        //
        // Reused, not rebuilt. The list is a COM object the plugin calls into
        // during `process`, and it used to be `ComWrapper::new`d — an `Arc`
        // allocation and a refcounted free, per block, on the audio thread.
        self.scratch.events.clear();
        for n in events {
            self.scratch.events.push_note(n);
        }

        // ── the control changes ─────────────────────────────────────────────
        //
        // The pool is reused, not rebuilt: `clear` is one `Cell` write and every
        // point below is two more. This is the part of `process` that a plugin
        // calls back into while the audio deadline is running.
        self.changes.clear();
        self.drain_editor_edits();
        let last = frames.saturating_sub(1) as i32;
        let mut unmapped = 0usize;
        for c in controls {
            // A sampleOffset outside the block is out of contract, and the
            // plugin is entitled to index its buffers with it.
            let offset = c.offset.clamp(0, last);
            match self.control_param(c.channel, c.controller) {
                Some(id) => {
                    // BOTH doors, always — and this is measured, not defensive.
                    //
                    // The parameter change is the specified path: `IMidiMapping`
                    // names a parameter, the host pushes a point into
                    // `inputParameterChanges`, and the processor applies it.
                    // Pianoteq 9 publishes the mapping (CC64 -> 0x6d636d40),
                    // accepts the queue without complaint, returns `kResultOk`
                    // — **and does nothing at all**. Measured on a held C4
                    // released with the pedal down: 0.001452 RMS of tail with
                    // the parameter change, against 0.001452 without it. To six
                    // decimal places, byte for byte, the same rendering.
                    //
                    // Adding the legacy MIDI CC event beside it, changing
                    // nothing else, takes that tail to 0.012151 — **8.4x**, and
                    // the held portion changes too because the dampers are off
                    // the strings. That is the pedal arriving.
                    //
                    // So both are sent. A CC is a VALUE and not a delta, so a
                    // plugin that honours both simply sets the same parameter
                    // twice; there is no double-application to fear. Sending
                    // only the specified one is correct by the letter of VST3
                    // and silent on the instrument this app exists to play.
                    if let Some(q) = self.changes.queue_for(id) {
                        q.push(offset, c.normalised());
                    }
                    self.scratch.events.push_legacy_cc(*c, offset);
                }
                None => {
                    // No mapping published, so the parameter path is not
                    // available at all and the legacy event is the only hope.
                    // Counted so the band can say "this instrument has no
                    // pedal" rather than the user concluding the app has none.
                    unmapped += 1;
                    self.scratch.events.push_legacy_cc(*c, offset);
                }
            }
        }

        // ── the buffers ─────────────────────────────────────────────────────
        //
        // Last, deliberately: `bind` hands back a raw pointer into the scratch
        // and nothing may take `&mut self` between here and `process`.
        let (outputs, bus_count) = self.scratch.bind(channels, frames, out);
        // **Bound after the outputs**, and both point into the same scratch:
        // nothing may take `&mut self` between here and `process`.
        let (inputs, in_count) = self.scratch.bind_inputs(input, frames);

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: in_count as i32,
            numOutputs: bus_count as i32,
            inputs,
            outputs,
            inputParameterChanges: self.changes_ptr.as_ptr(),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: self.scratch.events_ptr.as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        // SAFETY: every pointer in `data` is either null or owned by `self`,
        // and all of them outlive the call. `inputParameterChanges` points at
        // `self.changes` and `inputEvents` at `self.scratch.events`, both alive
        // for the whole `Instance` and both only written between calls;
        // `outputs` is `self.scratch.buses`, rebound five lines up to the
        // channel pointers this block is rendering into.
        let r = unsafe { self.processor.process(&mut data) };
        if r != kResultOk {
            return Err(format!("process returned {r}"));
        }
        Ok(Rendered {
            frames,
            unmapped,
            dropped_events: self.scratch.events.dropped(),
        })
    }

    /// Move whatever the editor has changed into this block's parameter list.
    ///
    /// **This is the line between an editor that works and one that lies.** See
    /// [`ComponentHandler`], sixth trap: without it Pianoteq's volume slider
    /// moves on screen and the rendered audio does not change by one bit.
    ///
    /// Runs on the audio thread, called from `process_with_controls` between
    /// `changes.clear()` and the pedal loop, and does none of the three
    /// forbidden things:
    ///
    /// * **No allocation.** `slots` and `edit_seen` were both sized in
    ///   [`Instance::create`]; this only reads and writes into them.
    /// * **No lock.** Three relaxed loads and one acquire per claimed slot, and
    ///   the overwhelmingly common answer is "nothing changed", which costs the
    ///   acquire alone.
    /// * **No panic.** Every index goes through `get`, including `edit_seen`,
    ///   even though `used` cannot exceed [`MAX_EDIT_SLOTS`] — a panic across
    ///   this ABI is undefined behaviour rather than a backtrace, so the bound
    ///   is checked rather than reasoned about.
    ///
    /// A slot whose value could not be placed (the queue pool is full because
    /// the pedal got there first) is deliberately **not** marked as seen, so it
    /// is retried on the next block instead of being lost. A knob that moved
    /// stays where the user put it either way.
    fn drain_editor_edits(&mut self) {
        let used = self.handler.used.load(Ordering::Acquire);
        for i in 0..used {
            let Some(slot) = self.handler.slots.get(i) else {
                break;
            };
            // Acquire, and read first: everything the editor wrote before
            // bumping this is visible once this load has seen the bump.
            let seq = slot.seq.load(Ordering::Acquire);
            let Some(seen) = self.edit_seen.get_mut(i) else {
                break;
            };
            if seq == *seen {
                continue;
            }
            let id = slot.id.load(Ordering::Relaxed);
            if id == kNoParamId {
                continue;
            }
            // Reading the value after the seq can pick up an even newer one
            // than the seq promised, which is the correct outcome for a control
            // whose only interesting property is where it ended up.
            let value = f64::from_bits(slot.value.load(Ordering::Relaxed));
            let Some(q) = self.changes.queue_for(id) else {
                // Pool exhausted this block. Leave `seen` alone and try again
                // next block rather than silently dropping the move.
                continue;
            };
            // Offset 0: the value applies from the first frame of the block.
            // A mouse has no opinion finer than that.
            q.push(0, value);
            *seen = seq;
        }
    }
}

impl Drop for Instance {
    /// Teardown, in reverse. Unlike a module (which is never unloaded, see
    /// `scan::Library`), an instance genuinely must be released: a plugin left
    /// active holds its DSP allocation and, for several commercial plugins, a
    /// worker thread.
    ///
    /// **Three objects now, in one order** — see the module docs, fifth trap.
    /// The controller is disconnected and terminated here; releasing it is the
    /// job of the `editing` field, which is declared first so it happens before
    /// the component is released.
    fn drop(&mut self) {
        if self.processing {
            // SAFETY: alive processor.
            unsafe { self.processor.setProcessing(0u8) };
        }
        if self.active {
            // SAFETY: alive component.
            unsafe { self.component.setActive(0u8) };
        }
        // Disconnect BEFORE either end is terminated. A component that is still
        // connected to a terminated controller will call into it — that is what
        // the channel is for — and the call lands in an object that has freed
        // what it needs to answer.
        if let Some(e) = &self.editing {
            if let Some((a, b)) = &e.points {
                // SAFETY: undoing exactly the connect in `attach_controller`, on
                // the same two live objects.
                unsafe {
                    a.disconnect(b.as_ptr());
                    b.disconnect(a.as_ptr());
                }
            }
            // The handler goes away with this Instance, so take it off the
            // controller first rather than leaving the controller holding a
            // pointer into a struct that is mid-drop.
            // SAFETY: alive controller.
            unsafe { e.controller.setComponentHandler(std::ptr::null_mut()) };
            if e.initialised {
                // SAFETY: `initialize` succeeded on a controller of its own, so
                // `terminate` is owed exactly once. A single-component plugin
                // sets `initialised: false` and is terminated once, below.
                unsafe { e.controller.terminate() };
            }
        }
        // SAFETY: initialize succeeded, so terminate is owed exactly once.
        unsafe { self.component.terminate() };
    }
}

/// One note event to deliver during a `process` call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Note {
    /// Frames from the start of this block.
    pub offset: i32,
    pub pitch: i16,
    /// 0.0..=1.0. VST3 velocity is a float, not a 0-127 integer — dividing by
    /// 127 is the caller's job and forgetting it makes every note fortissimo.
    pub velocity: f32,
    pub on: bool,
}

/// One control change to deliver during a `process` call. The sustain pedal is
/// this.
///
/// **`value` is the raw MIDI number and is NOT normalised**, which is
/// deliberately the opposite of [`Note::velocity`]. The reason is that the
/// divisor is a property of the controller rather than of the caller: a 7-bit
/// CC is out of 127, pitch bend is out of 16383, and a caller who has to
/// remember which is which will eventually send a pedal at 0.5 % of its value.
/// [`Control::normalised`] owns that decision and is unit-tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Control {
    /// Frames from the start of this block. Clamped into the block by
    /// [`Instance::process_with_controls`] — a `sampleOffset` past `numSamples`
    /// is out of contract and some plugins index with it.
    pub offset: i32,
    /// A VST3 controller number: a MIDI CC 0..=127, or one of the two synthetic
    /// ones VST3 adds ([`Control::AFTERTOUCH`], [`Control::PITCH_BEND`]).
    pub controller: i16,
    /// 0..=127 for a CC or channel pressure; 0..=16383 for pitch bend.
    pub value: u16,
    /// 0..=15. **Not decoration**: Pianoteq publishes a different parameter for
    /// each channel's CC64, so a host that queries channel 0 and sends channel 3
    /// moves the wrong piano.
    pub channel: i16,
}

impl Control {
    pub const SUSTAIN: i16 = ControllerNumbers_::kCtrlSustainOnOff as i16;
    pub const SOSTENUTO: i16 = ControllerNumbers_::kCtrlSustenutoOnOff as i16;
    pub const SOFT: i16 = ControllerNumbers_::kCtrlSoftPedalOnOff as i16;
    pub const AFTERTOUCH: i16 = ControllerNumbers_::kAfterTouch as i16;
    pub const PITCH_BEND: i16 = ControllerNumbers_::kPitchBend as i16;

    /// A 7-bit control change, which is every CC including all three pedals.
    pub fn cc(offset: i32, channel: i16, controller: i16, value: u8) -> Self {
        Self {
            offset,
            controller,
            value: u16::from(value & 0x7F),
            channel,
        }
    }

    /// Pitch bend from its two MIDI data bytes, LSB first as the wire has them.
    pub fn pitch_bend(offset: i32, channel: i16, lsb: u8, msb: u8) -> Self {
        Self {
            offset,
            controller: Self::PITCH_BEND,
            value: u16::from(lsb & 0x7F) | (u16::from(msb & 0x7F) << 7),
            channel,
        }
    }

    /// The value VST3 wants: 0.0..=1.0.
    ///
    /// Pitch bend is the whole reason this is a function. It is 14-bit, so its
    /// full scale is 16383 and its *centre* is 8192 — dividing it by 127 gives
    /// 64.5, which a plugin clamps to 1.0, and the instrument sits a full tone
    /// sharp for the rest of the session.
    pub fn normalised(&self) -> ParamValue {
        let full = if self.controller == Self::PITCH_BEND {
            16_383.0
        } else {
            127.0
        };
        (ParamValue::from(self.value) / full).clamp(0.0, 1.0)
    }
}

/// What one `process` call did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rendered {
    pub frames: usize,
    /// Controls this plugin publishes no `IMidiMapping` for.
    ///
    /// They were still offered as a legacy MIDI CC event, which most instruments
    /// ignore, so read a non-zero count as **"the pedal probably did not
    /// arrive"** rather than as a hard failure. It is not "we never tried",
    /// which is what it used to mean upstream.
    pub unmapped: usize,
    /// Notes and control events that did not fit in this block's event list.
    ///
    /// Always zero from `ivory/src/instrument.rs`, which caps itself well under
    /// [`MAX_EVENTS`] and counts its own drops. Non-zero means some other caller
    /// handed one `process` call more events than a block can carry, and the
    /// symptom would otherwise be a note that silently never sounded.
    pub dropped_events: usize,
}

/// Events this list can carry in one block.
///
/// The app's renderer caps itself at 128 notes and 32 controls per block
/// (`MAX_EVENTS_PER_BLOCK` and `MAX_CONTROLS_PER_BLOCK` in
/// `ivory/src/instrument.rs`), and each control can add one legacy CC event, so
/// 160 is the most that can ever arrive from there and 256 is comfortably past
/// it. The 257th event of a block is **dropped and counted** in
/// [`Rendered::dropped_events`] rather than allocated for: this list is filled
/// on the audio thread.
const MAX_EVENTS: usize = 256;

/// The `IEventList` the plugin reads this block's notes out of.
///
/// **Allocated once, in [`Instance::create`], and refilled every block.** It
/// used to be built per call — a `Vec<Event>` and a `ComWrapper` (which is an
/// `Arc`) for every block of every slot — which is two allocations and two frees
/// under a realtime deadline, three times over with three instruments layered.
///
/// `Cell` and not `RefCell`, for the same reason [`ParamQueue`] is: a `RefCell`
/// would put a borrow flag on the audio thread's path, and a double-borrow
/// panic across the VST3 ABI is undefined behaviour rather than a backtrace.
/// `Event` is a plain C struct and `Copy`, so a `Cell` of one costs nothing.
struct EventList {
    /// Fixed length [`MAX_EVENTS`]. Never grown, never shrunk; `len` says how
    /// much of it this block is using.
    events: Vec<Cell<Event>>,
    len: Cell<usize>,
    /// Events that did not fit. Counted rather than dropped silently, because
    /// the symptom is "a note went missing" and nothing else would say so.
    dropped: Cell<usize>,
}

impl EventList {
    fn new() -> Self {
        Self {
            events: (0..MAX_EVENTS)
                .map(|_| {
                    // SAFETY: `Event` is a plain C struct of integers and a
                    // union; a zeroed one is a valid value of it. Nothing reads
                    // a slot before `push` has written it (`len` bounds every
                    // read), so these are placeholders and not data.
                    Cell::new(unsafe { std::mem::zeroed() })
                })
                .collect(),
            len: Cell::new(0),
            dropped: Cell::new(0),
        }
    }

    /// Forget last block's events.
    fn clear(&self) {
        self.len.set(0);
        self.dropped.set(0);
    }

    fn dropped(&self) -> usize {
        self.dropped.get()
    }

    /// Append, or count it as dropped. `bool` for the caller that cares.
    fn push(&self, e: Event) -> bool {
        let n = self.len.get();
        let Some(slot) = self.events.get(n) else {
            self.dropped.set(self.dropped.get().saturating_add(1));
            return false;
        };
        slot.set(e);
        self.len.set(n + 1);
        true
    }

    /// Append a note-on or note-off.
    fn push_note(&self, n: &Note) -> bool {
        // SAFETY: as in `new` — a zeroed `Event` is a valid value, and every
        // field that matters is written below.
        let mut e: Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = n.offset;
        e.ppqPosition = 0.0;
        e.flags = 0;
        if n.on {
            e.r#type = Event_::EventTypes_::kNoteOnEvent as u16;
            e.__field0.noteOn = NoteOnEvent {
                channel: 0,
                pitch: n.pitch,
                tuning: 0.0,
                velocity: n.velocity,
                length: 0,
                noteId: -1,
            };
        } else {
            e.r#type = Event_::EventTypes_::kNoteOffEvent as u16;
            e.__field0.noteOff = NoteOffEvent {
                channel: 0,
                pitch: n.pitch,
                velocity: n.velocity,
                noteId: -1,
                tuning: 0.0,
            };
        }
        self.push(e)
    }

    /// Append a control change as a legacy MIDI CC event.
    ///
    /// The fallback for a plugin that publishes no `IMidiMapping`, and a weak
    /// one: `kLegacyMIDICCOutEvent` is named for the direction the SDK actually
    /// specifies — plugin to host — and instruments are under no obligation to
    /// read it coming the other way. Measured on Pianoteq 9, which does have a
    /// mapping and so never sees this path, and on a mapping-less plugin, where
    /// it changed nothing. It is sent anyway because it costs one struct and it
    /// is the only other door VST3 leaves open; the caller is told through
    /// [`Rendered::unmapped`] that this is what happened, rather than being
    /// allowed to believe the pedal arrived.
    ///
    /// The two 14-bit controllers are folded to 7 bits here. A legacy CC event
    /// has one `value` byte and nowhere to put the rest.
    fn push_legacy_cc(&self, c: Control, offset: i32) -> bool {
        // `c_char`, NOT `i8`. The VST3 headers declare these fields as `char`,
        // and `char` is SIGNED on x86 and UNSIGNED on aarch64 — so hard-coding
        // `i8` compiles on an Intel Mac and on x86_64 Linux, and fails on every
        // ARM Linux target there is. It builds on Apple Silicon only because
        // the Darwin ABI keeps `char` signed where Linux's ARM ABI does not,
        // which is exactly the sort of difference that is invisible until a
        // Raspberry Pi build fails.
        let value = if c.controller == Control::PITCH_BEND {
            (c.value >> 7) as std::os::raw::c_char
        } else {
            (c.value & 0x7F) as std::os::raw::c_char
        };
        // SAFETY: `Event` is a plain C struct of integers and a union; zeroed is
        // a valid bit pattern for it and every field that matters is written
        // below. This is the same construction `push_note` uses.
        let mut e: Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = offset;
        e.ppqPosition = 0.0;
        e.flags = 0;
        e.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
        e.__field0.midiCCOut = LegacyMIDICCOutEvent {
            controlNumber: c.controller.clamp(0, 255) as u8,
            channel: c.channel.clamp(0, 15) as std::os::raw::c_char,
            value,
            value2: 0,
        };
        self.push(e)
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> i32 {
        // `len`, not `events.len()`: the array is [`MAX_EVENTS`] long for the
        // instance's whole life and only the first `len` of it belongs to this
        // block. Reporting the capacity would hand the plugin 256 events, most
        // of them last block's.
        self.len.get() as i32
    }

    unsafe fn getEvent(&self, index: i32, e: *mut Event) -> tresult {
        let Some(src) = usize::try_from(index)
            .ok()
            .filter(|i| *i < self.len.get())
            .and_then(|i| self.events.get(i))
        else {
            return kInvalidArgument;
        };
        if e.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: `e` is a caller-provided out-parameter.
        unsafe { *e = src.get() };
        kResultOk
    }

    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        // The host owns this list and fills it before `process`. A plugin
        // adding to its own INPUT list is not a thing that happens; output
        // events go to a different list, which this host does not supply.
        kResultFalse
    }
}

// ── The buffers `process` hands over, allocated once ─────────────────────────

/// Everything one `process` call gives the plugin that is neither the plugin's
/// nor the caller's: pointer arrays, the discarded buses' storage, and the
/// event list.
///
/// **Sized once, in [`Instance::create`], from [`Setup::max_block`] and the bus
/// layout.** Every field here used to be built per call. Measured on Pianoteq's
/// layout by reproducing the old code under a counting allocator: **42
/// allocator calls for one block** — 21 allocations and 21 frees — which is 126
/// with three slots layered, about 12,000 a second at 512 frames and 48 kHz.
/// They were always the same size, so a general-purpose allocator served them
/// off a free list and nobody ever heard it; an allocation under a realtime
/// deadline is a dropout waiting for a busy moment, and the free list is
/// exactly what a busy moment takes away.
struct Scratch {
    /// Channel pointers for the MAIN output bus — the one the caller reads.
    ///
    /// **Refreshed every block**, and that is not paranoia: `out` belongs to
    /// the caller, and a `Vec` that grew since the last call is a `Vec` whose
    /// buffer moved.
    ptrs: Vec<*mut f32>,
    /// One block of samples per channel of every output bus PAST the first,
    /// flat: channel `i` starts at `i * max_block`.
    ///
    /// Those buses are rendered and thrown away and they still need REAL,
    /// SEPARATE storage — pointing several buses at one buffer invites a plugin
    /// that sums into its outputs to accumulate eight times into the one we
    /// read. Flat rather than a `Vec<Vec<f32>>` because it is one allocation
    /// instead of fifteen (Pianoteq: seven stereo buses past the main one).
    aux: Vec<f32>,
    /// Channel pointers into [`Scratch::aux`], in bus order. The plugin is
    /// handed a pointer INTO this array, so it is one contiguous array and not
    /// one per bus.
    aux_ptrs: Vec<*mut f32>,
    /// One per output bus, in bus order. `numChannels` is fixed at creation and
    /// is where [`Scratch::bind`] reads the layout back from; only the pointer
    /// and the silence flags are rewritten per block.
    buses: Vec<AudioBusBuffers>,
    /// One block for every input channel of every input bus, contiguous.
    in_samples: Vec<f32>,
    in_ptrs: Vec<*mut f32>,
    in_buses: Vec<AudioBusBuffers>,
    /// The event list and the interface pointer handed to the plugin, held for
    /// the same reason `changes`/`changes_ptr` are: `to_com_ptr` clones an
    /// `Arc`, which is an atomic increment on the audio thread for a pointer
    /// that never changes.
    events: ComWrapper<EventList>,
    events_ptr: ComPtr<IEventList>,
    /// Frames one channel of `aux` holds.
    max_block: usize,
}

impl Scratch {
    fn new(max_block: i32, audio_out: &[Bus], audio_in: &[Bus]) -> Result<Self, String> {
        let max_block = max_block.max(0) as usize;
        let channels = |b: &Bus| b.channels.max(0) as usize;
        let main = audio_out.first().map(&channels).unwrap_or(0);
        let aux_channels: usize = audio_out.iter().skip(1).map(&channels).sum();

        let events = ComWrapper::new(EventList::new());
        let events_ptr = events
            .to_com_ptr::<IEventList>()
            .ok_or_else(|| "could not build the event list".to_string())?;

        // One `AudioBusBuffers` per bus even though only the first is read.
        // `ProcessData::numOutputs` is the length of this array and a plugin
        // walks all of it: Pianoteq exposes EIGHT stereo output buses, all
        // activated, and handed `numOutputs: 1` it wrote nothing at all and
        // still returned kResultOk. Not an error, not a warning — a
        // correct-looking call that produces a silent file. Every multi-output
        // instrument (Kontakt, Omnisphere, any drum machine) has that shape.
        let mut buses = Vec::with_capacity(audio_out.len().max(1));
        let mut push = |n: usize| {
            buses.push(AudioBusBuffers {
                numChannels: n as i32,
                silenceFlags: 0,
                // Bound per block; never handed to a plugin like this.
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: std::ptr::null_mut(),
                },
            });
        };
        push(main);
        for b in audio_out.iter().skip(1) {
            push(channels(b));
        }

        // ── the input side ──────────────────────────────────────────────
        //
        // Sized the same way and for the same reason. An instrument has no
        // input buses at all, so all of this is empty and costs nothing; an
        // effect usually has exactly one stereo bus.
        //
        // **Every input channel gets scratch of its own**, unlike the outputs,
        // which borrow the caller's. The caller hands over as many channels as
        // it has and the plugin may want more — mono into a stereo reverb is
        // the ordinary case — so the samples are COPIED into buffers that are
        // always the right width. A plugin handed a short array walks off the
        // end of it.
        let in_channels: usize = audio_in.iter().map(&channels).sum();
        let mut in_buses = Vec::with_capacity(audio_in.len());
        for b in audio_in {
            in_buses.push(AudioBusBuffers {
                numChannels: channels(b) as i32,
                silenceFlags: 0,
                __field0: AudioBusBuffers__type0 {
                    channelBuffers32: std::ptr::null_mut(),
                },
            });
        }

        Ok(Self {
            ptrs: vec![std::ptr::null_mut(); main],
            aux: vec![0.0; aux_channels * max_block],
            aux_ptrs: vec![std::ptr::null_mut(); aux_channels],
            buses,
            in_samples: vec![0.0; in_channels * max_block],
            in_ptrs: vec![std::ptr::null_mut(); in_channels],
            in_buses,
            events,
            events_ptr,
            max_block,
        })
    }

    /// Point this block's `AudioBusBuffers` at this block's samples, and hand
    /// back the array and its length for `ProcessData`.
    ///
    /// **Allocates nothing and cannot panic**: every index goes through `get`,
    /// and the two loops write into arrays that were sized before the audio
    /// thread ever saw this object.
    ///
    /// Two things stay per-block on purpose:
    ///
    /// * the main bus's channel pointers, because the caller's `Vec`s may have
    ///   moved since the last call;
    /// * the zero-fill of the aux channels, because the buffers used to be
    ///   freshly allocated (and therefore zeroed) every block, and a plugin
    ///   that sums into its outputs rather than overwriting them would
    ///   otherwise accumulate into last block's numbers until it overflowed.
    /// Copy this block's input into the scratch and point the input buses at
    /// it. Returns the array and its length for `ProcessData`.
    ///
    /// **Null and zero when there is nothing to feed**, which is every
    /// instrument: a plugin with no input buses must be handed `numInputs: 0`,
    /// and one WITH them that we have no audio for is better fed silence than
    /// a dangling pointer.
    ///
    /// Allocates nothing and cannot panic: every index goes through `get`, and
    /// the scratch was sized from the bus layout before the audio thread ever
    /// saw this instance.
    fn bind_inputs(&mut self, input: &[Vec<f32>], frames: usize) -> (*mut AudioBusBuffers, usize) {
        if self.in_buses.is_empty() {
            return (std::ptr::null_mut(), 0);
        }
        let frames = frames.min(self.max_block);
        let mut at = 0usize;
        for (bus, buf) in self.in_buses.iter_mut().enumerate() {
            let want = buf.numChannels.max(0) as usize;
            for c in 0..want {
                let start = (at + c) * self.max_block;
                let Some(dst) = self.in_samples.get_mut(start..start + frames) else {
                    continue;
                };
                // **The last channel repeats.** Mono into a stereo reverb is
                // the ordinary case; the alternative is silence down one side,
                // which sounds like a broken plugin rather than a mono source.
                // Only the FIRST bus is fed at all — the others are side-chains
                // and this app has nothing to put in one.
                let src = (bus == 0)
                    .then(|| input.get(c).or_else(|| input.last()))
                    .flatten();
                match src {
                    Some(ch) => {
                        for (d, s) in dst.iter_mut().zip(ch.iter()) {
                            *d = *s;
                        }
                        // A short channel leaves the tail of the block silent
                        // rather than holding the last sample.
                        for d in dst.iter_mut().skip(ch.len()) {
                            *d = 0.0;
                        }
                    }
                    None => dst.fill(0.0),
                }
                if let Some(p) = self.in_ptrs.get_mut(at + c) {
                    *p = self.in_samples[start..].as_mut_ptr();
                }
            }
            buf.__field0.channelBuffers32 = match self.in_ptrs.get_mut(at) {
                Some(p) => p as *mut *mut f32,
                None => std::ptr::null_mut(),
            };
            at += want;
        }
        (self.in_buses.as_mut_ptr(), self.in_buses.len())
    }

    fn bind(
        &mut self,
        channels: usize,
        frames: usize,
        out: &mut [Vec<f32>],
    ) -> (*mut AudioBusBuffers, usize) {
        for (p, ch) in self.ptrs.iter_mut().zip(out.iter_mut().take(channels)) {
            *p = ch.as_mut_ptr();
        }

        let frames = frames.min(self.max_block);
        for (i, p) in self.aux_ptrs.iter_mut().enumerate() {
            let start = i * self.max_block;
            let Some(chan) = self
                .aux
                .get_mut(start..)
                .and_then(|rest| rest.get_mut(..frames))
            else {
                continue;
            };
            chan.fill(0.0);
            *p = chan.as_mut_ptr();
        }

        let mut at = 0usize;
        for (k, bus) in self.buses.iter_mut().enumerate() {
            // Reset every block: `silenceFlags` on an OUTPUT bus is written by
            // the plugin, and a stale flag left from last block is a claim that
            // this block is silent.
            bus.silenceFlags = 0;
            if k == 0 {
                bus.__field0.channelBuffers32 = self.ptrs.as_mut_ptr();
                continue;
            }
            // `get_mut(at..)` rather than pointer arithmetic: the sum of these
            // channel counts is exactly `aux_ptrs.len()` by construction, and
            // the day that stops being true this hands the plugin a null it can
            // check instead of a pointer it cannot.
            bus.__field0.channelBuffers32 = self
                .aux_ptrs
                .get_mut(at..)
                .map_or(std::ptr::null_mut(), <[*mut f32]>::as_mut_ptr);
            at += bus.numChannels.max(0) as usize;
        }

        (self.buses.as_mut_ptr(), self.buses.len())
    }
}

// ── Control changes: the map, and the queues that carry the values ───────────

/// Controller numbers `IMidiMapping` covers: 0..=127 are the MIDI CCs, 128 is
/// aftertouch and 129 is pitch bend. `kCountCtrlNumber` is 130 and is the SDK's
/// own name for exactly this bound.
const CTRL_COUNT: usize = ControllerNumbers_::kCountCtrlNumber as usize;

/// MIDI channels. The whole table is read per channel because plugins really do
/// publish a different parameter per channel — Pianoteq's CC64 ids run
/// `0x6d636d40`, `0x6d636dc2`, ... one `CTRL_COUNT` step apart.
const MIDI_CHANNELS: usize = 16;

/// Distinct parameters that can change in one block. Five pedals and wheels is
/// the realistic number; 32 is room for a plugin whose channels each map
/// separately, and it costs 32 preallocated COM objects once.
const MAX_PARAM_QUEUES: usize = 32;

/// Points one parameter can take in one block. A pedal moved by a human cannot
/// produce 32 values in 10 ms; a continuous pedal swept by a sequencer can, and
/// the 33rd is dropped rather than allocated for.
const MAX_POINTS: usize = 32;

/// Create the plugin's edit controller, connect it to the component, and keep
/// it.
///
/// `None` means this plugin has no controller: no CC map and no editor. That is
/// legal and rare.
///
/// # The connect is the step everybody skips
///
/// Without it Pianoteq returns `kResultOk` and `paramId 0` for every controller
/// on every channel — see the module docs, second trap. Both directions, because
/// the SDK's own connect is symmetric and a plugin that only hears one half is a
/// plugin that has not finished setting itself up.
///
/// # This used to disconnect again, and the reason it stopped
///
/// Every version of this file before the editor landed created the controller,
/// harvested the CC table and tore it down inside one function, on the argument
/// that a connected controller means the component may send it messages while
/// audio is running and that building those messages is a call back into
/// [`HostApp`], which refused.
///
/// The refusal is what changed, not the risk. `IEditController::createView` is
/// the only door to a plugin's own UI, so the controller has to live; and once
/// it lives, [`HostApp::createInstance`] has to actually allocate. It does.
///
/// What is genuinely accepted here is that `IConnectionPoint::notify` may be
/// called on the audio thread by a component that has something to tell its
/// controller mid-block — the SDK's answer is a "connection proxy" that defers
/// every message to the UI thread, which this host does not have. Measured on
/// Pianoteq 9: the messages all arrive during `initialize`, before a block is
/// rendered. A plugin that chatters during `process` would be allocating on the
/// audio thread through this path, and the proxy is the fix.
fn attach_controller(
    module: &Module,
    component: &ComPtr<IComponent>,
    host: &ComPtr<IHostApplication>,
    handler: &ComWrapper<ComponentHandler>,
) -> Option<Editing> {
    let handler_ptr = handler.to_com_ptr::<IComponentHandler>();
    let set_handler = |controller: &ComPtr<IEditController>| {
        if let Some(h) = &handler_ptr {
            // Before `connect`, matching the SDK's own host sequence. A
            // controller with no handler is entitled to refuse to build an
            // editor.
            // SAFETY: live controller, live handler owned by the caller.
            unsafe { controller.setComponentHandler(h.as_ptr()) };
        }
    };

    // A "single component effect" merges the two halves into one object. Ask
    // the component first: when it answers, no second object is needed, none of
    // the connection dance applies, and `terminate` is owed only once — on the
    // component — which is why `initialised` is false here.
    if let Some(controller) = component.cast::<IEditController>() {
        set_handler(&controller);
        return Some(Editing {
            controller,
            points: None,
            initialised: false,
        });
    }

    let mut cid: TUID = [0; 16];
    // SAFETY: `cid` is a valid out-parameter and the component is initialised.
    if unsafe { component.getControllerClassId(&mut cid) } != kResultOk {
        return None;
    }

    let mut raw: *mut c_void = std::ptr::null_mut();
    // SAFETY: `cid` names a class the component itself just gave us; `raw` is a
    // valid out-parameter and the factory outlives this call.
    let created = unsafe {
        module.factory().createInstance(
            cid.as_mut_ptr(),
            IEditController::IID.as_ptr() as *const _ as *mut _,
            &mut raw,
        )
    };
    if created != kResultOk || raw.is_null() {
        return None;
    }
    // SAFETY: `createInstance` returns an object with one reference already
    // added, which `from_raw` takes ownership of.
    let controller = unsafe { ComPtr::<IEditController>::from_raw(raw.cast()) }?;

    // SAFETY: freshly created controller; the host context outlives it, because
    // `Instance` owns both and terminates the controller in `Drop`.
    if unsafe { controller.initialize(host.as_ptr().cast()) } != kResultOk {
        // No terminate: initialize did not succeed, so none is owed. The
        // reference goes with `controller`.
        return None;
    }
    set_handler(&controller);

    // THE step.
    let points = match (
        component.cast::<IConnectionPoint>(),
        controller.cast::<IConnectionPoint>(),
    ) {
        (Some(a), Some(b)) => {
            // SAFETY: both are live connection points on objects this function
            // owns a reference to, and `Instance::drop` disconnects them.
            unsafe {
                a.connect(b.as_ptr());
                b.connect(a.as_ptr());
            }
            Some((a, b))
        }
        _ => None,
    };

    // The SDK's host sequence has one more step here that this does not:
    // `IEditController::setComponentState` with the component's own state, so
    // the controller starts out agreeing with the processor. It is skipped
    // **because a freshly created instance has nothing to disagree about** —
    // both halves are at their defaults and neither has been told otherwise.
    // The moment that stops being true is a restore, and that is exactly where
    // the call lives: [`crate::state::load`], with the rewind between the two
    // that decides whether the plugin's UI matches its own sound.
    Some(Editing {
        controller,
        points,
        initialised: true,
    })
}

/// Undo [`attach_controller`] for an `Editing` that never became a field of an
/// `Instance`.
///
/// Only the failure path in [`Instance::create`] needs this: once `editing` is a
/// field, `Instance::drop` owns the ordering. Written out rather than made a
/// `Drop` impl on `Editing` **because a `Drop` impl would run at the wrong
/// time** — it would disconnect and terminate the controller as the field is
/// released, which is after `Instance::drop` has already terminated the
/// component, and the SDK's order is the other way round.
fn release_controller(editing: Option<Editing>) {
    let Some(e) = editing else {
        return;
    };
    if let Some((a, b)) = &e.points {
        // SAFETY: undoing exactly the connect in `attach_controller`.
        unsafe {
            a.disconnect(b.as_ptr());
            b.disconnect(a.as_ptr());
        }
    }
    // SAFETY: live controller.
    unsafe { e.controller.setComponentHandler(std::ptr::null_mut()) };
    if e.initialised {
        // SAFETY: initialize succeeded, so terminate is owed exactly once.
        unsafe { e.controller.terminate() };
    }
}

/// Does this controller build an editor?
///
/// The only way to ask. VST3 has no `hasEditor`: a host either creates the view
/// or does not know. Created and released immediately, which is the same pair of
/// calls a user closing the window makes, so a plugin that cannot survive it
/// cannot survive being used either.
fn probe_editor(controller: &ComPtr<IEditController>) -> bool {
    // SAFETY: live controller; `kEditor` is a static NUL-terminated string.
    let raw = unsafe { controller.createView(ViewType::kEditor) };
    if raw.is_null() {
        return false;
    }
    // SAFETY: `createView` returns a view with one reference already added,
    // which `from_raw` takes ownership of and releases here.
    drop(unsafe { ComPtr::<IPlugView>::from_raw(raw) });
    true
}

/// Read the plugin's CC-to-parameter table, once, at instantiation.
///
/// Returns `MIDI_CHANNELS * CTRL_COUNT` entries, `kNoParamId` where the plugin
/// published nothing.
///
/// `IMidiMapping` lives on the edit controller (see the module docs), so this
/// runs after [`attach_controller`] and reads through it. The table is copied
/// rather than queried per block so that nothing on the audio thread calls into
/// the plugin to answer "where does CC64 go".
///
/// The cost is stated plainly: a plugin that changes its CC assignment at
/// runtime announces it through `IComponentHandler::restartComponent`
/// (`kMidiCCAssignmentChanged`), which [`ComponentHandler`] receives, counts and
/// ignores. The table is read once. Instruments do not do this; modular
/// hosts-inside-plugins do.
fn read_midi_map(component: &ComPtr<IComponent>, editing: Option<&Editing>) -> Vec<ParamID> {
    let empty = || vec![kNoParamId; MIDI_CHANNELS * CTRL_COUNT];

    // The component first, for a single-component plugin where the mapping is
    // on the same object.
    if let Some(map) = component.cast::<IMidiMapping>() {
        return harvest(&map);
    }
    match editing.and_then(|e| e.controller.cast::<IMidiMapping>()) {
        Some(map) => harvest(&map),
        None => empty(),
    }
}

/// Pull the whole table out of a live `IMidiMapping`.
fn harvest(map: &ComPtr<IMidiMapping>) -> Vec<ParamID> {
    let mut table = vec![kNoParamId; MIDI_CHANNELS * CTRL_COUNT];
    for channel in 0..MIDI_CHANNELS {
        for ctrl in 0..CTRL_COUNT {
            let mut id: ParamID = kNoParamId;
            // Bus 0, because bus 0 is the event bus every note in this file is
            // sent on. Asking about a bus you do not play is a different
            // question with a different answer.
            // SAFETY: `id` is a valid out-parameter; the mapping is alive.
            let r = unsafe {
                map.getMidiControllerAssignment(0, channel as i16, ctrl as i16, &mut id)
            };
            if r == kResultOk {
                table[channel * CTRL_COUNT + ctrl] = id;
            }
        }
    }
    if looks_unconnected(&table) {
        table.fill(kNoParamId);
    }
    table
}

/// Is this table the answer a controller gives when it has not been connected?
///
/// The signature is *several different controller numbers sharing one parameter
/// id*, which is what Pianoteq returns (everything is 0) before
/// `IConnectionPoint::connect`. Checked on channel 0 alone and deliberately so:
/// one id for one controller across all sixteen channels is a perfectly normal
/// channel-agnostic mapping, and rejecting that would throw away a working
/// pedal. One id for the mod wheel *and* the sustain pedal is not a mapping.
///
/// Refusing a suspect table means no pedal, which is the status quo. Trusting
/// one means writing pedal values into an arbitrary parameter of a piano.
fn looks_unconnected(table: &[ParamID]) -> bool {
    let mut mapped = 0usize;
    let mut first: Option<ParamID> = None;
    let mut uniform = true;
    for id in table.iter().take(CTRL_COUNT) {
        if *id == kNoParamId {
            continue;
        }
        mapped += 1;
        match first {
            None => first = Some(*id),
            Some(f) => uniform &= f == *id,
        }
    }
    mapped >= 2 && uniform
}

/// One parameter's points for one block. A COM object the plugin reads during
/// `process`.
///
/// `Cell`, not `RefCell`: a `RefCell` would put a borrow flag on the audio
/// thread's path and a double borrow is a panic, and a panic across the VST3
/// ABI is undefined behaviour rather than a stack trace. Nothing here can fail.
struct ParamQueue {
    id: Cell<ParamID>,
    len: Cell<usize>,
    points: [Cell<(i32, ParamValue)>; MAX_POINTS],
}

impl ParamQueue {
    fn new() -> Self {
        Self {
            id: Cell::new(kNoParamId),
            len: Cell::new(0),
            points: std::array::from_fn(|_| Cell::new((0, 0.0))),
        }
    }

    /// Claim this queue for `id` and forget last block's points.
    fn reset(&self, id: ParamID) {
        self.id.set(id);
        self.len.set(0);
    }

    /// Append a point. `false` when the queue is full, which is dropped rather
    /// than grown: this runs on the audio thread.
    fn push(&self, offset: i32, value: ParamValue) -> bool {
        let n = self.len.get();
        if n >= MAX_POINTS {
            return false;
        }
        self.points[n].set((offset, value));
        self.len.set(n + 1);
        true
    }
}

impl Class for ParamQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        self.id.get()
    }

    unsafe fn getPointCount(&self) -> i32 {
        self.len.get() as i32
    }

    unsafe fn getPoint(
        &self,
        index: i32,
        sample_offset: *mut i32,
        value: *mut ParamValue,
    ) -> tresult {
        let Some(point) = usize::try_from(index)
            .ok()
            .filter(|i| *i < self.len.get())
            .map(|i| self.points[i].get())
        else {
            return kInvalidArgument;
        };
        if sample_offset.is_null() || value.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: both are caller-provided out-parameters, checked non-null.
        unsafe {
            *sample_offset = point.0;
            *value = point.1;
        }
        kResultOk
    }

    unsafe fn addPoint(&self, sample_offset: i32, value: ParamValue, index: *mut i32) -> tresult {
        // The host fills this list before `process`; a plugin writing into its
        // own INPUT queue is not a thing that happens. Honoured anyway rather
        // than refused, because it is the same three lines and a refusal here
        // would be a surprise to any plugin that tried.
        if !self.push(sample_offset, value) {
            return kResultFalse;
        }
        if !index.is_null() {
            // SAFETY: caller-provided out-parameter, checked non-null.
            unsafe { *index = self.len.get() as i32 - 1 };
        }
        kResultOk
    }
}

/// `ProcessData::inputParameterChanges`: the host-side object the plugin calls
/// into to read this block's parameter moves.
///
/// Allocated once, in [`Instance::create`]. Every block after that is
/// `clear()` plus a handful of `Cell` writes, so nothing on the audio thread
/// allocates, locks or can fail.
struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    /// Queues carrying data this block. VST3 wants `getParameterCount` to be the
    /// number of parameters that actually changed, not the size of the pool.
    used: Cell<usize>,
}

impl ParamChanges {
    fn new() -> Self {
        Self {
            queues: (0..MAX_PARAM_QUEUES).map(|_| ComWrapper::new(ParamQueue::new())).collect(),
            used: Cell::new(0),
        }
    }

    fn clear(&self) {
        self.used.set(0);
    }

    /// The queue for `id`, claiming a fresh one if this is its first point in
    /// this block. `None` when the pool is exhausted.
    ///
    /// Linear search over at most [`MAX_PARAM_QUEUES`] entries, which is faster
    /// than any map at this size and, more to the point, is a search over an
    /// array that was allocated five seconds ago rather than now.
    fn queue_for(&self, id: ParamID) -> Option<&ParamQueue> {
        let used = self.used.get();
        for q in self.queues.iter().take(used) {
            if q.id.get() == id {
                return Some(q);
            }
        }
        let q = self.queues.get(used)?;
        q.reset(id);
        self.used.set(used + 1);
        Some(q)
    }
}

impl Class for ParamChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> i32 {
        self.used.get() as i32
    }

    unsafe fn getParameterData(&self, index: i32) -> *mut IParamValueQueue {
        let Some(q) = usize::try_from(index)
            .ok()
            .filter(|i| *i < self.used.get())
            .and_then(|i| self.queues.get(i))
        else {
            return std::ptr::null_mut();
        };
        // A borrowed pointer, not an owned one: `getParameterData` does not
        // add a reference and the plugin must not release what it gets back.
        // `as_com_ref` is the only accessor here that touches no refcount.
        q.as_com_ref::<IParamValueQueue>()
            .map(|r| r.as_ptr())
            .unwrap_or(std::ptr::null_mut())
    }

    unsafe fn addParameterData(&self, _id: *const ParamID, _index: *mut i32) -> *mut IParamValueQueue {
        // Same argument as `EventList::addEvent`: this is the INPUT list, the
        // host owns it and fills it before `process`, and a plugin adding to it
        // would be writing somewhere nobody reads. Null is the ABI's "could not
        // create one", which is true.
        std::ptr::null_mut()
    }
}

/// What one media type and direction of a component's buses look like.
///
/// A free function and not a method, because [`Instance::create`] needs the
/// answer before there is an `Instance` to ask: the scratch is sized from it.
fn read_buses(component: &ComPtr<IComponent>, media: i32, dir: i32) -> Vec<Bus> {
    // SAFETY: the component is initialised and alive.
    let count = unsafe { component.getBusCount(media, dir) };
    let mut out = Vec::with_capacity(count.max(0) as usize);
    for i in 0..count {
        // SAFETY: `BusInfo` is a plain C struct of integers and a character
        // array, so a zeroed one is a valid value; `getBusInfo` overwrites it.
        let mut info: BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: `i` is in range and `info` is a valid out-parameter.
        if unsafe { component.getBusInfo(media, dir, i, &mut info) } != kResultOk {
            continue;
        }
        out.push(Bus {
            name: utf16_to_string(&info.name),
            channels: info.channelCount,
            // BusTypes::kAux == 1. Named inline because the constant lives in a
            // differently-shaped module than the two above and reading `1` here
            // is worse than reading the comment.
            aux: info.busType == 1,
        });
    }
    out
}

/// A class id, as the SDK wants it.
///
/// `TUID` is `[c_char; 16]`, and `c_char` is signed on x86 and unsigned on
/// aarch64 — so the cast has to be spelled `c_char` rather than `i8` or this
/// function compiles on exactly half the targets. The BYTES are identical
/// either way; it is only the type that differs.
fn bytes_to_tuid(cid: [u8; 16]) -> TUID {
    let mut out: TUID = [0; 16];
    for (o, b) in out.iter_mut().zip(cid.iter()) {
        *o = *b as std::os::raw::c_char;
    }
    out
}

fn utf16_to_string(raw: &[u16]) -> String {
    let units: Vec<u16> = raw.iter().take_while(|c| **c != 0).copied().collect();
    String::from_utf16_lossy(&units).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_reports_a_name_a_plugin_can_read() {
        let host = HostApp::new();
        let mut buf: vst3::Steinberg::Vst::String128 = [0x41u16; 128];
        // SAFETY: `buf` is a valid String128.
        let r = unsafe { host.getName(&mut buf) };
        assert_eq!(r, kResultOk);
        assert_eq!(utf16_to_string(&buf), "Tangent");
        assert_eq!(
            buf[7], 0,
            "the string must be NUL-terminated, or the plugin reads 128 \
             characters of whatever was there before"
        );
    }

    #[test]
    fn a_null_name_buffer_is_refused_rather_than_written_through() {
        let host = HostApp::new();
        // SAFETY: deliberately passing null, which is what this checks.
        assert_eq!(unsafe { host.getName(std::ptr::null_mut()) }, kInvalidArgument);
    }

    #[test]
    fn create_instance_nulls_the_out_parameter_before_refusing() {
        // A plugin that checks the pointer instead of the result code would
        // otherwise use whatever was on its stack.
        let host = HostApp::new();
        let mut obj: *mut c_void = 0x1234_usize as *mut c_void;
        let mut cid: TUID = [0; 16];
        let mut iid: TUID = [0; 16];
        // SAFETY: all three pointers are valid.
        let r = unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) };
        assert_ne!(r, kResultOk);
        assert!(obj.is_null(), "the out-parameter was left dangling");
        assert_eq!(host.messages_made(), 0);
    }

    #[test]
    fn the_host_really_allocates_the_message_a_plugin_asks_for() {
        // The fourth trap in the module docs: this used to return kResultFalse,
        // which was survivable only while the controller was thrown away after
        // reading the CC table. A connected controller that cannot be sent a
        // message is an editor whose knobs do nothing.
        let host = HostApp::new();
        let mut obj: *mut c_void = std::ptr::null_mut();
        let mut cid: TUID = IMessage_iid;
        let mut iid: TUID = IMessage_iid;
        // SAFETY: all three pointers are valid.
        let r = unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) };
        assert_eq!(r, kResultOk);
        assert!(!obj.is_null());
        assert_eq!(host.messages_made(), 1);
        // The plugin owns it now, so this test has to release it exactly as a
        // plugin would.
        // SAFETY: one reference was handed over; this takes and drops it.
        drop(unsafe { ComPtr::<IMessage>::from_raw(obj.cast()) });
    }

    #[test]
    fn a_message_carries_its_id_and_its_attributes_back_out() {
        let msg = HostMessage::new();
        // SAFETY: trait methods on a live message; the id is NUL-terminated.
        unsafe {
            msg.setMessageID(c"heartbeat".as_ptr());
            let back = std::ffi::CStr::from_ptr(msg.getMessageID());
            assert_eq!(back.to_bytes(), b"heartbeat");
            assert!(!msg.getAttributes().is_null());
        }
    }

    #[test]
    fn attributes_round_trip_every_type_the_sdk_defines() {
        let attrs = HostAttributes::new();
        // SAFETY: trait methods on a live list, with valid out-parameters.
        unsafe {
            assert_eq!(attrs.setInt(c"n".as_ptr(), -7), kResultOk);
            assert_eq!(attrs.setFloat(c"x".as_ptr(), 0.25), kResultOk);
            let text: Vec<TChar> = "hi".encode_utf16().chain(std::iter::once(0)).collect();
            assert_eq!(attrs.setString(c"s".as_ptr(), text.as_ptr()), kResultOk);
            assert_eq!(attrs.setBinary(c"b".as_ptr(), [1u8, 2, 3].as_ptr().cast(), 3), kResultOk);

            let mut n: int64 = 0;
            assert_eq!(attrs.getInt(c"n".as_ptr(), &mut n), kResultOk);
            assert_eq!(n, -7);

            let mut x = 0.0f64;
            assert_eq!(attrs.getFloat(c"x".as_ptr(), &mut x), kResultOk);
            assert!((x - 0.25).abs() < f64::EPSILON);

            let mut buf = [0x41u16; 8];
            let size = (buf.len() * size_of::<TChar>()) as uint32;
            assert_eq!(attrs.getString(c"s".as_ptr(), buf.as_mut_ptr(), size), kResultOk);
            assert_eq!(utf16_to_string(&buf), "hi");

            let mut data: *const c_void = std::ptr::null();
            let mut len: uint32 = 0;
            assert_eq!(attrs.getBinary(c"b".as_ptr(), &mut data, &mut len), kResultOk);
            assert_eq!(len, 3);
            assert_eq!(std::slice::from_raw_parts(data.cast::<u8>(), 3), &[1, 2, 3]);

            // A key nobody set is kResultFalse, not a wrong answer.
            assert_ne!(attrs.getInt(c"missing".as_ptr(), &mut n), kResultOk);
            // And the wrong type for a key that exists is refused too.
            assert_ne!(attrs.getFloat(c"n".as_ptr(), &mut x), kResultOk);
        }
    }

    #[test]
    fn a_string_longer_than_the_plugins_buffer_is_truncated_and_still_terminated() {
        // `sizeInBytes`, not `sizeInCharacters`: getting that wrong writes one
        // UTF-16 unit past the end of a plugin's buffer, every time, forever.
        let attrs = HostAttributes::new();
        // SAFETY: trait methods on a live list.
        unsafe {
            let text: Vec<TChar> = "abcdef".encode_utf16().chain(std::iter::once(0)).collect();
            assert_eq!(attrs.setString(c"s".as_ptr(), text.as_ptr()), kResultOk);
            let mut buf = [0x41u16; 8];
            // Room for THREE units, so two characters and a terminator.
            assert_eq!(attrs.getString(c"s".as_ptr(), buf.as_mut_ptr(), 6), kResultOk);
            assert_eq!(utf16_to_string(&buf), "ab");
            assert_eq!(buf[2], 0);
            assert_eq!(buf[3], 0x41, "wrote past the buffer the caller gave");
        }
    }

    #[test]
    fn an_event_list_reports_and_returns_its_events() {
        let notes = [
            Note { offset: 0, pitch: 60, velocity: 0.8, on: true },
            Note { offset: 128, pitch: 60, velocity: 0.5, on: false },
        ];
        let list = EventList::new();
        for n in &notes {
            assert!(list.push_note(n));
        }
        // SAFETY: trait methods on a live list.
        unsafe {
            assert_eq!(list.getEventCount(), 2);
            let mut e: Event = std::mem::zeroed();
            assert_eq!(list.getEvent(0, &mut e), kResultOk);
            assert_eq!(e.sampleOffset, 0);
            assert_eq!(e.__field0.noteOn.pitch, 60);
            assert!((e.__field0.noteOn.velocity - 0.8).abs() < 1e-6);
            assert_eq!(list.getEvent(1, &mut e), kResultOk);
            assert_eq!(e.sampleOffset, 128);
            assert_eq!(e.__field0.noteOff.pitch, 60);
        }
    }

    #[test]
    fn an_out_of_range_event_index_is_refused_not_indexed() {
        let list = EventList::new();
        let mut e: Event = unsafe { std::mem::zeroed() };
        // SAFETY: valid out-parameter, deliberately bad indices.
        unsafe {
            assert_eq!(list.getEvent(0, &mut e), kInvalidArgument);
            assert_eq!(list.getEvent(-1, &mut e), kInvalidArgument);
        }
    }

    #[test]
    fn a_reused_event_list_never_shows_the_plugin_last_blocks_events() {
        // The list is [`MAX_EVENTS`] long for the instance's whole life now,
        // and only `len` of it belongs to this block. Reporting the capacity —
        // or forgetting to clear — hands the plugin a note it already played,
        // which is a stuck note or a double attack and not a compile error.
        let list = EventList::new();
        list.push_note(&Note { offset: 0, pitch: 60, velocity: 0.8, on: true });
        list.push_note(&Note { offset: 1, pitch: 64, velocity: 0.8, on: true });
        // SAFETY: trait method on a live list.
        assert_eq!(unsafe { list.getEventCount() }, 2);

        list.clear();
        // SAFETY: trait method on a live list.
        assert_eq!(unsafe { list.getEventCount() }, 0);
        let mut e: Event = unsafe { std::mem::zeroed() };
        // SAFETY: valid out-parameter; index 0 is now past the end.
        assert_eq!(unsafe { list.getEvent(0, &mut e) }, kInvalidArgument);

        list.push_note(&Note { offset: 7, pitch: 67, velocity: 0.5, on: true });
        // SAFETY: trait methods on a live list.
        unsafe {
            assert_eq!(list.getEventCount(), 1);
            assert_eq!(list.getEvent(0, &mut e), kResultOk);
            assert_eq!(e.__field0.noteOn.pitch, 67, "slot 0 still held the old note");
            assert_eq!(list.getEvent(1, &mut e), kInvalidArgument);
        }
    }

    #[test]
    fn more_events_than_a_block_can_carry_are_counted_rather_than_allocated_for() {
        // The list is fixed-size because it is filled on the audio thread. The
        // 257th event of a block is a note that will not sound, and the only
        // thing worse than dropping it is dropping it silently.
        let list = EventList::new();
        for i in 0..MAX_EVENTS {
            assert!(
                list.push_note(&Note { offset: 0, pitch: 60, velocity: 0.5, on: true }),
                "event {i} was refused inside the capacity"
            );
        }
        assert!(!list.push_note(&Note { offset: 0, pitch: 60, velocity: 0.5, on: true }));
        assert!(!list.push_legacy_cc(Control::cc(0, 0, Control::SUSTAIN, 127), 0));
        assert_eq!(list.dropped(), 2);
        // SAFETY: trait method on a live list.
        assert_eq!(unsafe { list.getEventCount() }, MAX_EVENTS as i32);
        list.clear();
        assert_eq!(list.dropped(), 0);
    }

    #[test]
    fn velocity_is_a_float_not_a_midi_byte() {
        // The single easiest mistake at this boundary: VST3 velocity is
        // 0.0..=1.0, so passing 100 makes every note fortissimo and clipped.
        let n = Note { offset: 0, pitch: 60, velocity: 100.0 / 127.0, on: true };
        assert!(n.velocity < 1.0);
    }

    #[test]
    fn a_knob_dragged_across_a_block_arrives_as_where_it_ended_up() {
        // Latest-wins, one slot. A drag is dozens of performEdits and the
        // processor needs the last one, not a queue of the ones in between.
        let h = ComponentHandler::new();
        // SAFETY: trait methods on a live handler.
        unsafe {
            for step in 1..=50u32 {
                assert_eq!(h.performEdit(0x6d63_6d40, f64::from(step) / 50.0), kResultOk);
            }
        }
        assert_eq!(h.used.load(Ordering::Acquire), 1, "one parameter, one slot");
        assert_eq!(h.edits.load(Ordering::Relaxed), 50);
        let slot = &h.slots[0];
        assert_eq!(slot.id.load(Ordering::Relaxed), 0x6d63_6d40);
        assert!((f64::from_bits(slot.value.load(Ordering::Relaxed)) - 1.0).abs() < 1e-12);
        // The sequence moved, which is the only thing the audio thread looks
        // at to decide whether there is anything to do.
        assert_eq!(slot.seq.load(Ordering::Acquire), 50);
    }

    #[test]
    fn each_parameter_gets_its_own_slot_and_keeps_it() {
        let h = ComponentHandler::new();
        // SAFETY: trait methods on a live handler.
        unsafe {
            h.performEdit(10, 0.1);
            h.performEdit(20, 0.2);
            h.performEdit(10, 0.9);
        }
        assert_eq!(h.used.load(Ordering::Acquire), 2);
        assert_eq!(h.slots[0].id.load(Ordering::Relaxed), 10);
        assert_eq!(h.slots[1].id.load(Ordering::Relaxed), 20);
        assert!((f64::from_bits(h.slots[0].value.load(Ordering::Relaxed)) - 0.9).abs() < 1e-12);
        assert_eq!(h.editor_overflow_for_test(), 0);
    }

    #[test]
    fn a_plugin_with_more_controls_than_slots_counts_what_it_lost() {
        // Silently dropping these would present as "one knob in this plugin
        // does nothing", which is unreportable. Counting them makes it a
        // number someone can read.
        let h = ComponentHandler::new();
        // SAFETY: trait methods on a live handler.
        unsafe {
            for id in 0..MAX_EDIT_SLOTS as u32 {
                assert_eq!(h.performEdit(id, 0.5), kResultOk);
            }
            assert_ne!(h.performEdit(9_999, 0.5), kResultOk);
            // ...but a parameter that already has a slot still works.
            assert_eq!(h.performEdit(0, 0.75), kResultOk);
        }
        assert_eq!(h.editor_overflow_for_test(), 1);
        assert!((f64::from_bits(h.slots[0].value.load(Ordering::Relaxed)) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn an_instance_still_cannot_be_moved_between_threads_by_accident() {
        // `ivory/src/instrument.rs` moves one across a ring behind an
        // `unsafe impl Send` it argues for in forty lines. Every one of those
        // lines assumes the compiler is refusing the *unargued* move — and the
        // compiler was only refusing it because a `Cell` in `HostApp` happened
        // to be there. See the struct docs.
        //
        // The inherent-const-beats-trait-const trick, because stable Rust has
        // no way to write `T: !Send`.
        struct Probe<T>(std::marker::PhantomData<T>);
        trait NotSendProbe {
            const IS_SEND: bool = false;
        }
        impl<T> NotSendProbe for Probe<T> {}
        impl<T: Send> Probe<T> {
            const IS_SEND: bool = true;
        }
        // `const`, so an `Instance` that became `Send` fails the BUILD rather
        // than one test somebody might not be running.
        const { assert!(!Probe::<Instance>::IS_SEND) };
        const { assert!(Probe::<u32>::IS_SEND) };
    }

    // ── the allocation counter ──────────────────────────────────────────────

    /// A global allocator that counts, on **one thread at a time**.
    ///
    /// `cargo test` runs tests in parallel in one process, so a global counter
    /// would be measuring the whole suite and the answer would depend on which
    /// tests happened to be running. The counter is therefore thread-local:
    /// only the thread inside [`while_counting`] is counted, and every other
    /// test in this binary pays one relaxed thread-local read per allocation
    /// and is otherwise unaffected.
    ///
    /// **Frees are counted too.** "Never allocate on the audio thread" is
    /// shorthand: the hazard is the allocator's lock and its slow paths, and
    /// `free` takes the same lock. A block that allocates nothing and frees a
    /// `ComWrapper` has not solved the problem.
    ///
    /// It can only see allocations made through **Rust's** allocator. A VST3
    /// plugin is C++ and calls `malloc` directly, so a plugin that allocates
    /// inside `process` is invisible here and always will be — that is the
    /// plugin's contract to keep, not this host's.
    struct Counting;

    thread_local! {
        /// `None` when this thread is not counting.
        static TOUCHES: Cell<Option<usize>> = const { Cell::new(None) };
    }

    fn touch() {
        // `try_with`, not `with`: this runs inside the allocator, and a panic
        // there during thread teardown would be a very confusing crash.
        let _ = TOUCHES.try_with(|c| {
            if let Some(n) = c.get() {
                c.set(Some(n + 1));
            }
        });
    }

    // SAFETY: every method forwards to `System`, unchanged, with the same
    // arguments. The counter is a thread-local `Cell<Option<usize>>` whose
    // access allocates nothing through this allocator (macOS allocates the TLS
    // block through libSystem's `malloc`, not Rust's), so there is no
    // re-entrancy.
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
            touch();
            // SAFETY: forwarding the caller's own layout.
            unsafe { std::alloc::System.alloc(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
            touch();
            // SAFETY: forwarding the caller's own pointer and layout.
            unsafe { std::alloc::System.dealloc(ptr, layout) }
        }
        unsafe fn realloc(
            &self,
            ptr: *mut u8,
            layout: std::alloc::Layout,
            new_size: usize,
        ) -> *mut u8 {
            touch();
            // SAFETY: forwarding the caller's own pointer, layout and size.
            unsafe { std::alloc::System.realloc(ptr, layout, new_size) }
        }
        unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
            touch();
            // SAFETY: forwarding the caller's own layout.
            unsafe { std::alloc::System.alloc_zeroed(layout) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;

    /// Run `f` and say how many times it went to the allocator.
    fn while_counting<T>(f: impl FnOnce() -> T) -> (T, usize) {
        // Touch the thread-local first, so the platform's own lazy setup for it
        // is not the thing being measured.
        TOUCHES.with(|c| c.set(None));
        TOUCHES.with(|c| c.set(Some(0)));
        let out = f();
        let n = TOUCHES.with(Cell::take).unwrap_or(0);
        (out, n)
    }

    #[test]
    fn the_allocation_counter_can_see_an_allocation_at_all() {
        // A counter that is silently broken would make every assertion below
        // pass, which is the worst possible failure mode for this kind of test.
        let (_, touches) = while_counting(|| drop(vec![0u8; 4096]));
        assert_eq!(touches, 2, "one allocation and one free");
        let (_, quiet) = while_counting(|| ());
        assert_eq!(quiet, 0);
    }

    /// **An effect is fed audio, and a plugin that wants more channels than
    /// it is handed gets the last one repeated.**
    ///
    /// The binding is the whole of what makes an effect possible: every
    /// `process` call before this one set `numInputs: 0` and `inputs: null`,
    /// which a plugin answers by processing silence and reporting success — a
    /// reverb that loads and does nothing.
    ///
    /// Checked through the scratch rather than through a plugin, because the
    /// thing that can be wrong here is pointer arithmetic and there is no
    /// third-party reverb on a build machine.
    #[test]
    fn an_effects_input_is_bound_channel_by_channel() {
        let stereo_in = vec![Bus {
            name: "in".into(),
            channels: 2,
            aux: false,
        }];
        let stereo_out = vec![Bus {
            name: "out".into(),
            channels: 2,
            aux: false,
        }];
        let mut scratch =
            Scratch::new(64, &stereo_out, &stereo_in).expect("build the scratch");

        // MONO in, stereo wanted: the one channel is copied to both sides.
        let mono = vec![vec![0.25_f32; 8]];
        let (ptr, count) = scratch.bind_inputs(&mono, 8);
        assert!(!ptr.is_null(), "an effect was handed no input at all");
        assert_eq!(count, 1, "one input bus");
        // SAFETY: the pointer is into `scratch`, which outlives this block, and
        // `count` is the length `bind_inputs` just reported.
        let bus = unsafe { &*ptr };
        assert_eq!(bus.numChannels, 2);
        for c in 0..2usize {
            // SAFETY: two channels, as the bus says, each `max_block` long.
            let ch = unsafe { *bus.__field0.channelBuffers32.add(c) };
            let samples = unsafe { std::slice::from_raw_parts(ch, 8) };
            assert!(
                samples.iter().all(|v| (v - 0.25).abs() < 1e-9),
                "channel {c} was not fed: {samples:?}"
            );
        }

        // A SHORT channel leaves the tail silent rather than holding the last
        // sample, which would be a click repeated for the rest of the block.
        let short = vec![vec![1.0_f32; 3], vec![1.0_f32; 3]];
        let (ptr, _) = scratch.bind_inputs(&short, 8);
        let bus = unsafe { &*ptr };
        let ch = unsafe { *bus.__field0.channelBuffers32 };
        let samples = unsafe { std::slice::from_raw_parts(ch, 8) };
        assert_eq!(&samples[..3], &[1.0, 1.0, 1.0]);
        assert!(samples[3..].iter().all(|v| *v == 0.0), "{samples:?}");

        // **An instrument is handed nothing at all.** No input buses means
        // `numInputs: 0` and a null array, which is what every VST3 host does
        // and what this one did unconditionally until now.
        let mut instrument = Scratch::new(64, &stereo_out, &[]).expect("scratch");
        let (ptr, count) = instrument.bind_inputs(&mono, 8);
        assert!(ptr.is_null() && count == 0, "an instrument was fed audio");
    }

    #[test]
    fn a_block_of_process_scratch_is_bound_without_touching_the_allocator() {
        // Everything `process` does on the host's side of the plugin call,
        // driven a hundred times: the event list, the parameter queues and the
        // bus binding. Each of those was an allocation per block until the
        // scratch moved onto the `Instance`.
        //
        // The bus layout is Pianoteq's, measured: eight stereo outputs, so
        // fourteen channels of discarded scratch and fifteen channel pointers.
        let buses: Vec<Bus> = (0..8)
            .map(|i| Bus {
                name: format!("out {i}"),
                channels: 2,
                aux: i > 0,
            })
            .collect();
        let mut scratch = Scratch::new(512, &buses, &[]).expect("build the scratch");
        let changes = ParamChanges::new();
        let mut out: Vec<Vec<f32>> = vec![vec![0.0; 512]; 2];
        let notes = [
            Note { offset: 0, pitch: 60, velocity: 0.8, on: true },
            Note { offset: 96, pitch: 64, velocity: 0.8, on: true },
            Note { offset: 400, pitch: 60, velocity: 0.4, on: false },
        ];
        let controls = [
            Control::cc(0, 0, Control::SUSTAIN, 127),
            Control::pitch_bend(64, 0, 0x00, 0x50),
        ];

        let (_, touches) = while_counting(|| {
            for _ in 0..100 {
                scratch.events.clear();
                for n in &notes {
                    assert!(scratch.events.push_note(n));
                }
                changes.clear();
                for c in &controls {
                    if let Some(q) = changes.queue_for(0x6d63_6d40) {
                        q.push(c.offset, c.normalised());
                    }
                    assert!(scratch.events.push_legacy_cc(*c, c.offset));
                }
                let (buses, count) = scratch.bind(2, 512, &mut out);
                assert!(!buses.is_null());
                assert_eq!(count, 8);
            }
        });
        assert_eq!(
            touches, 0,
            "a hundred blocks went to the allocator {touches} times; the whole \
             point of the scratch is that this is zero"
        );
    }

    // ── the two that need a real instrument ─────────────────────────────────

    /// The instrument every measurement in this repo is taken against.
    ///
    /// Not a fixture that can be faked: the point of the two tests below is
    /// that a commercial plugin's behaviour is not deducible from the API, and
    /// both of them exist because something the API said yes to turned out to
    /// be silent (the eight output buses) or ignored (CC64).
    fn pianoteq() -> (Module, ClassInfo) {
        let bundle = crate::scan::discover()
            .into_iter()
            .find(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().to_lowercase().contains("pianoteq"))
                    .unwrap_or(false)
            })
            .expect("no VST3 matching Pianoteq; this test needs one installed");
        let module = Module::open(&bundle).expect("open module");
        let class = module
            .audio_modules()
            .into_iter()
            .next()
            .expect("no Audio Module Class");
        (module, class)
    }

    /// Play one note, render half a second, and return the RMS of the main bus.
    ///
    /// Then drain the tail, so the next probe starts from silence rather than
    /// from the last one still ringing.
    fn probe_rms(inst: &mut Instance) -> f32 {
        const PITCH: i16 = 84;
        const BLOCKS: usize = 48;
        const RELEASE_AT: usize = 40;
        const DRAIN: usize = 200;

        let channels = inst
            .audio_outputs()
            .first()
            .map(|b| b.channels.max(0) as usize)
            .unwrap_or(0);
        let frames = inst.setup().max_block.max(0) as usize;
        let mut bufs: Vec<Vec<f32>> = vec![vec![0.0; frames]; channels];
        let on = [Note { offset: 0, pitch: PITCH, velocity: 0.8, on: true }];
        let off = [Note { offset: 0, pitch: PITCH, velocity: 0.5, on: false }];

        let mut sum = 0.0f64;
        let mut n = 0usize;
        for block in 0..BLOCKS {
            let events: &[Note] = match block {
                0 => &on,
                RELEASE_AT => &off,
                _ => &[],
            };
            inst.process(events, frames, &mut bufs).expect("process");
            for ch in bufs.iter().take(channels) {
                for s in ch.iter().take(frames) {
                    sum += f64::from(*s) * f64::from(*s);
                    n += 1;
                }
            }
        }
        for _ in 0..DRAIN {
            inst.process(&[], frames, &mut bufs).expect("drain");
        }
        (sum / n.max(1) as f64).sqrt() as f32
    }

    #[test]
    #[ignore = "needs Pianoteq 9 installed; loads it twice, warms both up and \
                renders about a minute of audio, so it takes about twenty-five \
                seconds. Run with --test-threads=1."]
    fn a_restored_instrument_renders_the_saved_sound_and_not_the_default_one() {
        // **A round trip of bytes proves nothing here.** `save_state` returning
        // what `load_state` accepts is a property of this file's own container;
        // the question is whether the PLUGIN heard it, and the only instrument
        // that can answer that is the rendered audio. This file already records
        // two cases where the API said yes and the audio said no: eight output
        // buses (`kResultOk`, silence) and CC64 as a parameter change
        // (`kResultOk`, byte-identical rendering).
        let (module, class) = pianoteq();
        let setup = Setup { sample_rate: 48_000.0, max_block: 512 };

        let mut a = Instance::create(&module, &class, setup).expect("instantiate");
        let gate = crate::ready::warm_up(&mut a, crate::ready::Policy::default());
        assert_eq!(gate.state(), crate::ready::State::Ready, "{}", gate.status_line());

        let default_rms = probe_rms(&mut a);
        assert!(default_rms > 1e-3, "the default piano made no sound ({default_rms})");

        // Move the volume the way the plugin's own editor does: `performEdit`
        // on the host's handler, which `drain_editor_edits` carries into the
        // next block's parameter changes. That is the sixth trap's path, so
        // this test also fails if editor forwarding ever regresses.
        let volume = {
            let e = a.editing.as_ref().expect("Pianoteq exports an edit controller");
            // SAFETY: a live controller on the thread that owns it.
            let count = unsafe { e.controller.getParameterCount() };
            let mut found = None;
            for i in 0..count {
                // SAFETY: `info` is a valid out-parameter and `i` is in range.
                let mut info: vst3::Steinberg::Vst::ParameterInfo =
                    unsafe { std::mem::zeroed() };
                // SAFETY: as above.
                if unsafe { e.controller.getParameterInfo(i, &mut info) } == kResultOk
                    && utf16_to_string(&info.title) == "Volume"
                {
                    found = Some(info.id);
                    break;
                }
            }
            found.expect("Pianoteq publishes a parameter called Volume")
        };
        // Measured, not chosen: Pianoteq's volume curve is nearly flat from
        // 0.5 to its 0.727 default (a probe reads 0.0662 against 0.0685, a 3%
        // difference this test would not be able to tell from run-to-run
        // wobble) and then falls away fast. 0.3 renders 0.0228, a third of the
        // default, and 0.0 renders 0.0030 — quiet enough that "restored" and
        // "the instrument is broken" would look the same. 0.3 is the value that
        // is unmistakably not the default and unmistakably not silence.
        const QUIETER: f64 = 0.3;
        // SAFETY: trait method on the live handler this instance owns.
        unsafe { a.handler.performEdit(volume, QUIETER) };
        // **Four seconds of settling, and it is not superstition.** Pianoteq
        // ramps its volume rather than stepping it, and the ramp is slow enough
        // that a probe played eight blocks after the move measures the ramp and
        // not the value: the first attempt at this test read 0.0660 for a
        // setting whose steady state is 0.0078, and every value looked like it
        // had arrived one probe late.
        let mut settle: Vec<Vec<f32>> = vec![vec![0.0; 512]; 2];
        for _ in 0..400 {
            a.process(&[], 512, &mut settle).expect("settle");
        }
        let changed_rms = probe_rms(&mut a);
        assert!(
            changed_rms < default_rms * 0.6,
            "moving the volume did not change the sound: {default_rms} -> {changed_rms}. \
             Either the parameter did not reach the processor (see the sixth trap) or \
             this test is measuring the wrong control"
        );

        let bytes = a.save_state().expect("save the state");
        // Read back through the container's own documented layout — magic,
        // version, two lengths — so the measurement below is a fact about this
        // plugin and not about this test's arithmetic.
        assert_eq!(&bytes[..4], b"TGST");
        let blob = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes"));
        println!(
            "Pianoteq 9 state: {} bytes total = {} processor + {} controller + 18 header",
            bytes.len(),
            blob(6),
            blob(10)
        );
        drop(a);

        let mut b = Instance::create(&module, &class, setup).expect("instantiate again");
        // **Create, restore, warm up.** The other order renders the default
        // sound during the warm-up and, worse, leaves the gate having waited
        // for a load the state is about to replace. See `Instance::load_state`.
        b.load_state(&bytes).expect("restore the state");
        let gate = crate::ready::warm_up(&mut b, crate::ready::Policy::default());
        assert_eq!(gate.state(), crate::ready::State::Ready, "{}", gate.status_line());
        let restored_rms = probe_rms(&mut b);

        println!(
            "default {default_rms:.6} -> changed {changed_rms:.6} -> restored {restored_rms:.6}"
        );
        assert!(
            restored_rms > 1e-4,
            "the restored instrument is silent, which is not the same as restored"
        );
        let off_by = (restored_rms - changed_rms).abs() / changed_rms.max(f32::EPSILON);
        assert!(
            off_by < 0.2,
            "a fresh instance restored from the saved bytes rendered {restored_rms}, \
             which is {:.0}% away from the {changed_rms} that was saved; the default \
             is {default_rms}",
            off_by * 100.0
        );
    }

    #[test]
    #[ignore = "needs Pianoteq 9 installed; takes about ten seconds. Run with \
                --test-threads=1."]
    fn a_hundred_blocks_of_a_real_instrument_touch_the_allocator_zero_times() {
        // The counter cannot see the plugin's own `malloc`s — it is C++ and
        // does not use Rust's allocator — so this measures exactly what this
        // crate is responsible for, and nothing else. That is the honest scope:
        // a host cannot make a plugin realtime-safe, it can only stop being the
        // reason it is not.
        let (module, class) = pianoteq();
        let setup = Setup { sample_rate: 48_000.0, max_block: 512 };
        let mut inst = Instance::create(&module, &class, setup).expect("instantiate");

        let channels = inst
            .audio_outputs()
            .first()
            .map(|b| b.channels.max(0) as usize)
            .unwrap_or(0);
        assert!(channels > 0);
        // Pre-grown, exactly as `ivory/src/instrument.rs` does at load time.
        // `process` resizes these to the block length, and a `Vec` that has
        // never held a block would grow — the caller's allocation, on the
        // caller's conscience, and it is why the engine pre-grows them.
        let mut bufs: Vec<Vec<f32>> = vec![vec![0.0; 512]; channels];
        let notes = [
            Note { offset: 0, pitch: 60, velocity: 0.8, on: true },
            Note { offset: 128, pitch: 64, velocity: 0.8, on: true },
        ];
        let controls = [Control::cc(0, 0, Control::SUSTAIN, 127)];

        // The first block outside the count: it is where `set_processing(true)`
        // reaches the plugin and where the caller's buffers take their length.
        inst.process_with_controls(&notes, &controls, 512, &mut bufs)
            .expect("first block");

        let (frames, touches) = while_counting(|| {
            let mut total = 0usize;
            for block in 0..100 {
                let events: &[Note] = if block == 0 { &notes } else { &[] };
                let r = inst
                    .process_with_controls(events, &controls, 512, &mut bufs)
                    .expect("process");
                total += r.frames;
                assert_eq!(r.dropped_events, 0);
            }
            total
        });
        assert_eq!(frames, 100 * 512);
        assert_eq!(
            touches, 0,
            "a hundred real blocks went to Rust's allocator {touches} times"
        );
    }

    #[test]
    fn a_cid_round_trips_through_the_sign_change() {
        // TUID is [i8; 16] and ClassInfo carries [u8; 16]. A byte above 127
        // must survive the conversion, or every class id with a high byte
        // fails to instantiate with an unhelpful "no such class".
        let cid = [0xFFu8, 0x80, 0x7F, 0x00, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let tuid = bytes_to_tuid(cid);
        let back: Vec<u8> = tuid.iter().map(|b| *b as u8).collect();
        assert_eq!(back.as_slice(), &cid);
    }
    /// **An effect instantiates.** The refusal that used to live in `create`
    /// made this impossible, and with it every insert in the app: the owner
    /// dropped a Pro-R on an input and was told it was an effect, which it is.
    ///
    /// Ignored like every other test that opens somebody else's binary — it
    /// needs the plugin installed and it runs their initialiser.
    ///
    ///     cargo test -p ivory-host an_effect -- --ignored --nocapture
    #[test]
    #[ignore = "needs FabFilter Pro-R 2 installed; opens a third-party bundle"]
    fn an_effect_instantiates_because_inserts_are_made_of_them() {
        let Some(bundle) = crate::scan::discover().into_iter().find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().contains("Pro-R"))
                .unwrap_or(false)
        }) else {
            panic!("no VST3 matching Pro-R; this test needs one installed");
        };
        let module = crate::scan::Module::open(&bundle).expect("open module");
        let class = module
            .audio_modules()
            .into_iter()
            .next()
            .expect("no Audio Module Class");
        assert_eq!(
            class.kind(),
            crate::scan::Kind::Effect,
            "Pro-R is not being read as an effect, so this proves nothing"
        );
        let inst = Instance::create(&module, &class, Setup::default())
            .expect("an effect must instantiate: it is what an insert is for");
        assert!(
            !inst.audio_inputs().is_empty(),
            "an effect with no audio input has nothing to insert into"
        );
    }

}



