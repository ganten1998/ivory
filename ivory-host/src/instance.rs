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

use std::cell::Cell;
use std::ffi::c_void;

use vst3::Steinberg::Vst::{
    kNoParamId, BusDirections_, BusInfo, ControllerNumbers_, Event, Event_, IAudioProcessor,
    IAudioProcessorTrait, IComponent, IComponentTrait, IConnectionPoint, IConnectionPointTrait,
    IEditController, IEventList, IEventListTrait, IHostApplication, IHostApplicationTrait,
    IMidiMapping, IMidiMappingTrait, IParamValueQueue, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, LegacyMIDICCOutEvent, MediaTypes_, NoteOffEvent, NoteOnEvent, ParamID,
    ParamValue, ProcessData, ProcessModes_, ProcessSetup, SymbolicSampleSizes_,
};
use vst3::Steinberg::{
    kInvalidArgument, kResultFalse, kResultOk, tresult, IPluginBaseTrait, IPluginFactoryTrait,
    TUID,
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
/// Minimal on purpose: `getName` so a plugin's error dialogs and preset paths
/// can say who loaded it, and `createInstance` refused. `createInstance` is how
/// a plugin asks the host to make an `IMessage`/`IAttributeList` for
/// component-to-controller communication, which a recorder that never opens an
/// editor does not need. Refusing is a legal answer; returning a broken object
/// would not be.
struct HostApp;

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
        _iid: *mut TUID,
        obj: *mut *mut c_void,
    ) -> tresult {
        if !obj.is_null() {
            // Null the out-parameter before refusing. A plugin that checks the
            // pointer instead of the result code would otherwise use whatever
            // was on its stack.
            // SAFETY: `obj` is a caller-provided out-parameter.
            unsafe { *obj = std::ptr::null_mut() };
        }
        kResultFalse
    }
}

/// A live plugin instance.
///
/// Not `Send`: VST3 requires that the main-thread methods be called from one
/// thread, and `process` is the only one that may be called from another. Making
/// the whole thing `!Send` and handing the audio thread a narrower type later is
/// the safe direction to be wrong in.
pub struct Instance {
    component: ComPtr<IComponent>,
    processor: ComPtr<IAudioProcessor>,
    _host: ComWrapper<HostApp>,
    setup: Setup,
    active: bool,
    processing: bool,
    audio_out: Vec<Bus>,
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

        let host = ComWrapper::new(HostApp);
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
        // controller is created and connected while the component is
        // initialised but not yet active. It is gone again by the time this
        // returns — see `read_midi_map`.
        let midi_map = read_midi_map(module, &component, &host_unknown);

        let changes = ComWrapper::new(ParamChanges::new());
        let Some(changes_ptr) = changes.to_com_ptr::<IParameterChanges>() else {
            // SAFETY: initialize succeeded, so terminate is owed.
            unsafe { component.terminate() };
            return Err("could not build the parameter change list".to_string());
        };

        let mut me = Self {
            component,
            processor,
            _host: host,
            setup,
            active: false,
            processing: false,
            audio_out: Vec::new(),
            event_in: Vec::new(),
            midi_map,
            changes,
            changes_ptr,
        };

        me.audio_out = me.buses(MediaTypes_::kAudio as i32, BusDirections_::kOutput as i32);
        me.event_in = me.buses(MediaTypes_::kEvent as i32, BusDirections_::kInput as i32);

        me.setup_processing()?;
        me.activate_all_buses();
        me.set_active(true)?;
        Ok(me)
    }

    fn buses(&self, media: i32, dir: i32) -> Vec<Bus> {
        // SAFETY: the component is initialised and alive.
        let count = unsafe { self.component.getBusCount(media, dir) };
        let mut out = Vec::with_capacity(count.max(0) as usize);
        for i in 0..count {
            let mut info: BusInfo = unsafe { std::mem::zeroed() };
            // SAFETY: `i` is in range and `info` is a valid out-parameter.
            if unsafe { self.component.getBusInfo(media, dir, i, &mut info) } != kResultOk {
                continue;
            }
            out.push(Bus {
                name: utf16_to_string(&info.name),
                channels: info.channelCount,
                // BusTypes::kAux == 1. Named inline because the constant lives
                // in a differently-shaped module than the two above and reading
                // `1` here is worse than reading the comment.
                aux: info.busType == 1,
            });
        }
        out
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

        for ch in out.iter_mut().take(channels) {
            ch.clear();
            ch.resize(frames, 0.0);
        }
        let mut ptrs: Vec<*mut f32> = out
            .iter_mut()
            .take(channels)
            .map(|c| c.as_mut_ptr())
            .collect();

        // EVERY activated output bus needs an AudioBusBuffers, not just the one
        // we intend to read. `ProcessData::numOutputs` is the length of the
        // `outputs` array, and a plugin walks all of it.
        //
        // This is what silence looks like: Pianoteq exposes EIGHT stereo output
        // buses, all activated, and being handed `numOutputs: 1` it wrote
        // nothing at all and still returned kResultOk. Not an error, not a
        // warning — a correct-looking call that produces a silent file. Every
        // multi-output instrument (Kontakt, Omnisphere, any drum machine) has
        // the same shape.
        //
        // Buses past the first get real, separately-allocated scratch: pointing
        // several buses at one buffer invites a plugin that sums into its
        // outputs to accumulate eight times into the one we read.
        let bus_count = self.audio_out.len().max(1);
        let mut scratch: Vec<Vec<f32>> = Vec::new();
        for b in self.audio_out.iter().skip(1) {
            for _ in 0..b.channels.max(0) {
                scratch.push(vec![0.0; frames]);
            }
        }
        let mut scratch_ptrs: Vec<*mut f32> =
            scratch.iter_mut().map(|c| c.as_mut_ptr()).collect();

        let mut buses: Vec<vst3::Steinberg::Vst::AudioBusBuffers> =
            Vec::with_capacity(bus_count);
        buses.push(vst3::Steinberg::Vst::AudioBusBuffers {
            numChannels: channels as i32,
            silenceFlags: 0,
            __field0: vst3::Steinberg::Vst::AudioBusBuffers__type0 {
                channelBuffers32: ptrs.as_mut_ptr(),
            },
        });
        let mut at = 0usize;
        for b in self.audio_out.iter().skip(1) {
            let n = b.channels.max(0) as usize;
            buses.push(vst3::Steinberg::Vst::AudioBusBuffers {
                numChannels: n as i32,
                silenceFlags: 0,
                __field0: vst3::Steinberg::Vst::AudioBusBuffers__type0 {
                    channelBuffers32: unsafe { scratch_ptrs.as_mut_ptr().add(at) },
                },
            });
            at += n;
        }

        // ── the control changes ─────────────────────────────────────────────
        //
        // The pool is reused, not rebuilt: `clear` is one `Cell` write and every
        // point below is two more. This is the part of `process` that a plugin
        // calls back into while the audio deadline is running, so it is the part
        // that had to be allocation-free even though the buffers above are not
        // (that landmine is `ivory/src/instrument.rs`'s to defuse, and it says
        // so).
        let mut list_data = EventList::new(events);
        self.changes.clear();
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
                    list_data.push_legacy_cc(*c, offset);
                }
                None => {
                    // No mapping published, so the parameter path is not
                    // available at all and the legacy event is the only hope.
                    // Counted so the band can say "this instrument has no
                    // pedal" rather than the user concluding the app has none.
                    unmapped += 1;
                    list_data.push_legacy_cc(*c, offset);
                }
            }
        }

        let list = ComWrapper::new(list_data);
        let list_ptr = list
            .to_com_ptr::<IEventList>()
            .ok_or_else(|| "could not build the event list".to_string())?;

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: 0,
            numOutputs: buses.len() as i32,
            inputs: std::ptr::null_mut(),
            outputs: buses.as_mut_ptr(),
            inputParameterChanges: self.changes_ptr.as_ptr(),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: list_ptr.as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        // SAFETY: every pointer in `data` is either null or owned by this
        // function or by `self`, and all of them outlive the call. In
        // particular `inputParameterChanges` points at `self.changes`, which is
        // alive for the whole `Instance` and is only written between calls.
        let r = unsafe { self.processor.process(&mut data) };
        if r != kResultOk {
            return Err(format!("process returned {r}"));
        }
        Ok(Rendered { frames, unmapped })
    }
}

impl Drop for Instance {
    /// Teardown, in reverse. Unlike a module (which is never unloaded, see
    /// `scan::Library`), an instance genuinely must be released: a plugin left
    /// active holds its DSP allocation and, for several commercial plugins, a
    /// worker thread.
    fn drop(&mut self) {
        if self.processing {
            // SAFETY: alive processor.
            unsafe { self.processor.setProcessing(0u8) };
        }
        if self.active {
            // SAFETY: alive component.
            unsafe { self.component.setActive(0u8) };
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
}

/// An `IEventList` over a borrowed slice.
struct EventList {
    events: Vec<Event>,
}

impl EventList {
    fn new(notes: &[Note]) -> Self {
        let events = notes
            .iter()
            .map(|n| {
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
                e
            })
            .collect();
        Self { events }
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
    fn push_legacy_cc(&mut self, c: Control, offset: i32) {
        let value = if c.controller == Control::PITCH_BEND {
            (c.value >> 7) as i8
        } else {
            (c.value & 0x7F) as i8
        };
        // SAFETY: `Event` is a plain C struct of integers and a union; zeroed is
        // a valid bit pattern for it and every field that matters is written
        // below. This is the same construction `new` uses.
        let mut e: Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = offset;
        e.ppqPosition = 0.0;
        e.flags = 0;
        e.r#type = Event_::EventTypes_::kLegacyMIDICCOutEvent as u16;
        e.__field0.midiCCOut = LegacyMIDICCOutEvent {
            controlNumber: c.controller.clamp(0, 255) as u8,
            channel: c.channel.clamp(0, 15) as i8,
            value,
            value2: 0,
        };
        self.events.push(e);
    }
}

impl Class for EventList {
    type Interfaces = (IEventList,);
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> i32 {
        self.events.len() as i32
    }

    unsafe fn getEvent(&self, index: i32, e: *mut Event) -> tresult {
        let Some(src) = usize::try_from(index).ok().and_then(|i| self.events.get(i)) else {
            return kInvalidArgument;
        };
        if e.is_null() {
            return kInvalidArgument;
        }
        // SAFETY: `e` is a caller-provided out-parameter.
        unsafe { *e = *src };
        kResultOk
    }

    unsafe fn addEvent(&self, _e: *mut Event) -> tresult {
        // The host owns this list and fills it before `process`. A plugin
        // adding to its own INPUT list is not a thing that happens; output
        // events go to a different list, which this host does not supply.
        kResultFalse
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

/// Read the plugin's CC-to-parameter table, once, at instantiation.
///
/// Returns `MIDI_CHANNELS * CTRL_COUNT` entries, `kNoParamId` where the plugin
/// published nothing.
///
/// # Why this creates a second plugin object and then throws it away
///
/// `IMidiMapping` lives on the edit controller (see the module docs), so the
/// controller must exist. It does **not** have to keep existing: the table is
/// static for the life of an instance, so this creates the controller,
/// connects it, copies the table, and tears it back down before `process` is
/// ever called.
///
/// That is the smaller change, and it is also the safer one. A controller left
/// connected means the component may try to send it `IMessage`s while audio is
/// running, and building those messages is a call back into [`HostApp`], which
/// refuses — a refusal that is legal but that no plugin's error path is well
/// tested against. Nothing here is connected by the time the first block is
/// rendered.
///
/// The cost is stated plainly: a plugin that changes its CC assignment at
/// runtime announces it through `IComponentHandler::restartComponent`
/// (`kMidiCCAssignmentChanged`), which this host does not implement and could
/// not receive anyway. The table is read once. Instruments do not do this;
/// modular hosts-inside-plugins do.
fn read_midi_map(
    module: &Module,
    component: &ComPtr<IComponent>,
    host: &ComPtr<IHostApplication>,
) -> Vec<ParamID> {
    let empty = || vec![kNoParamId; MIDI_CHANNELS * CTRL_COUNT];

    // A "single component effect" merges the two halves into one object. Ask
    // the component first: when it answers, no second object is needed and none
    // of the connection dance below applies.
    if let Some(map) = component.cast::<IMidiMapping>() {
        return harvest(&map);
    }

    let mut cid: TUID = [0; 16];
    // SAFETY: `cid` is a valid out-parameter and the component is initialised.
    if unsafe { component.getControllerClassId(&mut cid) } != kResultOk {
        return empty();
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
        return empty();
    }
    // SAFETY: `createInstance` returns an object with one reference already
    // added, which `from_raw` takes ownership of.
    let Some(controller) = (unsafe { ComPtr::<IEditController>::from_raw(raw.cast()) }) else {
        return empty();
    };

    // SAFETY: freshly created controller; the host context outlives this
    // function, which terminates the controller before returning.
    if unsafe { controller.initialize(host.as_ptr().cast()) } != kResultOk {
        return empty();
    }

    // THE step. Without it Pianoteq returns kResultOk and paramId 0 for every
    // controller on every channel — see the module docs. Both directions,
    // because the SDK's own connect is symmetric and a plugin that only hears
    // one half is a plugin that has not finished setting itself up.
    let comp_point = component.cast::<IConnectionPoint>();
    let ctrl_point = controller.cast::<IConnectionPoint>();
    if let (Some(a), Some(b)) = (&comp_point, &ctrl_point) {
        // SAFETY: both are live connection points on objects this function owns
        // a reference to.
        unsafe {
            a.connect(b.as_ptr());
            b.connect(a.as_ptr());
        }
    }

    let table = match controller.cast::<IMidiMapping>() {
        Some(map) => harvest(&map),
        None => empty(),
    };

    if let (Some(a), Some(b)) = (&comp_point, &ctrl_point) {
        // SAFETY: undoing exactly the connect above, on the same live objects.
        unsafe {
            a.disconnect(b.as_ptr());
            b.disconnect(a.as_ptr());
        }
    }
    // SAFETY: initialize succeeded, so terminate is owed exactly once. The
    // controller is released when `controller` drops, after this.
    unsafe { controller.terminate() };
    table
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

fn bytes_to_tuid(cid: [u8; 16]) -> TUID {
    let mut out: TUID = [0; 16];
    for (o, b) in out.iter_mut().zip(cid.iter()) {
        *o = *b as i8;
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
        let host = HostApp;
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
        let host = HostApp;
        // SAFETY: deliberately passing null, which is what this checks.
        assert_eq!(unsafe { host.getName(std::ptr::null_mut()) }, kInvalidArgument);
    }

    #[test]
    fn create_instance_nulls_the_out_parameter_before_refusing() {
        // A plugin that checks the pointer instead of the result code would
        // otherwise use whatever was on its stack.
        let host = HostApp;
        let mut obj: *mut c_void = 0x1234_usize as *mut c_void;
        let mut cid: TUID = [0; 16];
        let mut iid: TUID = [0; 16];
        // SAFETY: all three pointers are valid.
        let r = unsafe { host.createInstance(&mut cid, &mut iid, &mut obj) };
        assert_ne!(r, kResultOk);
        assert!(obj.is_null(), "the out-parameter was left dangling");
    }

    #[test]
    fn an_event_list_reports_and_returns_its_events() {
        let notes = [
            Note { offset: 0, pitch: 60, velocity: 0.8, on: true },
            Note { offset: 128, pitch: 60, velocity: 0.5, on: false },
        ];
        let list = EventList::new(&notes);
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
        let list = EventList::new(&[]);
        let mut e: Event = unsafe { std::mem::zeroed() };
        // SAFETY: valid out-parameter, deliberately bad indices.
        unsafe {
            assert_eq!(list.getEvent(0, &mut e), kInvalidArgument);
            assert_eq!(list.getEvent(-1, &mut e), kInvalidArgument);
        }
    }

    #[test]
    fn velocity_is_a_float_not_a_midi_byte() {
        // The single easiest mistake at this boundary: VST3 velocity is
        // 0.0..=1.0, so passing 100 makes every note fortissimo and clipped.
        let n = Note { offset: 0, pitch: 60, velocity: 100.0 / 127.0, on: true };
        assert!(n.velocity < 1.0);
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
}
