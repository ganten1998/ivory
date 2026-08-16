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

use std::ffi::c_void;

use vst3::Steinberg::Vst::{
    BusDirections_, BusInfo, Event, Event_, IAudioProcessor, IAudioProcessorTrait, IComponent,
    IComponentTrait, IEventList, IEventListTrait, IHostApplication, IHostApplicationTrait,
    MediaTypes_, NoteOffEvent, NoteOnEvent, ProcessData, ProcessModes_, ProcessSetup,
    SymbolicSampleSizes_,
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

        let mut me = Self {
            component,
            processor,
            _host: host,
            setup,
            active: false,
            processing: false,
            audio_out: Vec::new(),
            event_in: Vec::new(),
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

        let list = ComWrapper::new(EventList::new(events));
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
            inputParameterChanges: std::ptr::null_mut(),
            outputParameterChanges: std::ptr::null_mut(),
            inputEvents: list_ptr.as_ptr(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        let r = unsafe { self.processor.process(&mut data) };
        if r != kResultOk {
            return Err(format!("process returned {r}"));
        }
        Ok(frames)
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
