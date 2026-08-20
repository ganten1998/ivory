//! Where notes come from, when the app is the one choosing.
//!
//! The desktop app opens a MIDI device itself: it enumerates ports, picks one
//! at startup, and lets the user switch with a dialog. A VST3 editor does none
//! of that — the host hands it note events and there is no device list to
//! offer — which is exactly what `Caps::midi_ports` says.
//!
//! So the app holds an `Option<Box<dyn MidiPorts>>`: `Some` on the desktop,
//! `None` in a plugin. A trait rather than a `cfg`, because the alternative is
//! `midir` in the shared crate, and the whole point of `ivory-ui`'s dependency
//! list is that `midir` cannot be reached from here at all.
//!
//! Note what is NOT behind this trait: the `mpsc` channel the events arrive
//! on. That is `std`, it is host-agnostic, and a plugin uses the same one —
//! `process()` sends into it from the audio thread and the editor drains it on
//! the next frame, which is the same shape as `midir`'s callback thread.

use crate::midi_event::MidiEvent;
use std::sync::mpsc;

/// A device the user can pick from a list.
///
/// Two fields and the distinction between them is the whole point. `uid` is
/// what gets written to the settings file; `name` is what gets drawn. Storing
/// the name would break for two identical webcams — which is not a hypothetical,
/// it is what happens the moment somebody adds a second camera for a side angle
/// — and again when the OS language changes and "Built-in Microphone" comes
/// back as "Micrófono integrado".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable, opaque, platform-supplied. Never shown to the user.
    pub uid: String,
    /// What the user calls it.
    pub name: String,
    /// The one the platform would pick if nobody chose. Marked in the list
    /// rather than auto-selected, so "System Default" stays a visible choice.
    pub default: bool,
}

/// A source of capture devices — cameras, or audio inputs.
///
/// One trait for both because the app does exactly the same three things with
/// each of them: list what is there, open one by uid, ask which is open. The
/// alternative was two traits differing only in their doc comments.
///
/// `Send` for the same reason [`MidiPorts`] is: the app that owns it has to be.
pub trait CaptureDevices: Send {
    /// Everything present right now. Re-read each time a picker opens, because
    /// devices are plugged and unplugged while the app runs — and on macOS a
    /// Continuity Camera appears and disappears as the phone comes and goes.
    fn list(&self) -> Vec<DeviceInfo>;

    /// Open this device by uid, closing whatever was open. `Err` carries a
    /// message written for the user.
    ///
    /// An empty uid means "close whatever is open and select nothing", which is
    /// how the None row in the picker is expressed without a second method.
    fn open(&mut self, uid: &str) -> Result<(), String>;

    /// The uid currently open, if any.
    fn current(&self) -> Option<String>;

    /// The display name of the device currently open.
    ///
    /// Separate from `current` because the band needs to *draw* something, and
    /// looking the uid back up through `list()` every frame means enumerating
    /// hardware sixty times a second.
    fn current_name(&self) -> Option<String>;
}

/// What the audio system itself can be asked, for the Setup panel.
///
/// A third port beside [`MidiPorts`] and [`CaptureDevices`], and not a method
/// on either: a system is not a device, and the two questions here are asked
/// once when a panel opens rather than every frame — which is the whole reason
/// they are not on [`crate::recorder::AudioStatus`], the struct the host
/// pushes sixty times a second. `rates` in particular re-enumerates hardware.
pub trait AudioSetup: Send {
    /// Every audio system this build can open, in the backend's own order.
    ///
    /// Usually one. cpal compiles in exactly one host per platform unless a
    /// cargo feature asks for more, and both of the extras — JACK and ASIO —
    /// need a development library present when the release is BUILT. Listed
    /// from the real enumeration anyway, so the panel says what this build has.
    fn systems(&self) -> Vec<String>;

    /// The rates the input device now selected will actually open at.
    ///
    /// Empty when nothing is selected or the device cannot be reached, and the
    /// panel then offers only "the device's own" — which is what every build
    /// before this one did unconditionally.
    fn rates(&self) -> Vec<u32>;

    /// How many inputs the selected device has. 0 when nothing is selected.
    fn input_channels(&self) -> u16;

    /// Which input, or pair of inputs, is being recorded.
    ///
    /// `None` is the whole device. `Some((a, None))` is input `a` as mono;
    /// `Some((a, Some(b)))` is the pair — which need not be adjacent and need
    /// not be in order, because 2/3 and 3/2 are both things people patch.
    ///
    /// Zero-based, like every index. What the UI prints is one-based, because
    /// that is what is on the front of the interface.
    fn channels(&self) -> Option<(u16, Option<u16>)>;

    /// Record this input, this pair, or the whole device.
    ///
    /// **The uid grammar stays on the host's side of the firewall.** A choice
    /// is a pair of numbers here and a string there; `ivory-ui` cannot spell
    /// the string and must not learn to.
    fn set_channels(&mut self, pick: Option<(u16, Option<u16>)>);
}

/// A folder the app has asked the host to choose.
///
/// The **request pattern**, not a blocking call. `rfd`'s native panel runs a
/// nested run loop, and raising one from inside an egui frame means re-entering
/// the frame that is already on the stack. So `ivory-ui` records that it wants
/// a folder, the host drains it *after* `frame()` returns, and a plugin refuses
/// simply by never draining — the same shape as the plugin's `pending_resize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirRequest {
    /// Where the picker should open. `None` means the platform decides.
    pub start_at: Option<std::path::PathBuf>,
    /// Title for the panel.
    pub title: String,
    /// What the chosen folder is FOR.
    ///
    /// One request type with a purpose on it rather than two nearly identical
    /// ones: the host's job is the same either way — raise the native panel
    /// after the frame — and the only thing that differs is which of the app's
    /// setters gets the answer.
    pub purpose: DirPurpose,
}

/// A file the app has asked the host to choose.
///
/// The same request pattern as [`DirRequest`], and for the same reason: a
/// native panel cannot be raised from inside an egui frame. A plugin refuses
/// simply by never draining it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRequest {
    pub start_at: Option<std::path::PathBuf>,
    pub title: String,
    /// Extensions to offer, without dots, and the name of the group. Empty
    /// means every file.
    pub extensions: Vec<String>,
    pub extension_label: String,
    pub purpose: FilePurpose,
}

/// The backing track, as the band needs to draw it.
///
/// **The waveform is precomputed by the host**, because the samples are on the
/// other side of the firewall and there are a hundred million of them.
/// `ivory-ui` gets an outline it can draw and a length it can label; what a
/// peak envelope is, and how many megabytes it came from, is the recorder's
/// business.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackInfo {
    /// The file's name, or empty when nothing is loaded.
    pub name: String,
    /// How long the file is, in seconds.
    pub seconds: f64,
    /// A peak envelope, 0..=1, left to right across the whole file.
    pub wave: Vec<f32>,
    /// Why the last import did not work, if it did not.
    pub error: String,
}

impl TrackInfo {
    /// Nothing loaded, as a borrowable constant — the band's `EMPTY` view is a
    /// `const` and cannot build a `String`.
    pub const NONE: &'static TrackInfo = &TrackInfo {
        name: String::new(),
        seconds: 0.0,
        wave: Vec::new(),
        error: String::new(),
    };

    /// Whether anything is loaded at all.
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
    }
}

/// What the effects ship as, pushed in by the host.
///
/// **The DSP owns these numbers, and it is on the other side of the firewall.**
/// `ivory-ui` draws a panel of sliders and has no idea what a comb filter's
/// feedback should be; writing a second copy of the defaults here to draw the
/// sliders at the right place would be two tables to keep in step, and the
/// symptom of them drifting is a panel that reads 62% while the audio does
/// something else. So the host hands them over once, the same way it hands
/// over the device list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectDefaults {
    /// Parameter key to its default, as the settings file would store it.
    pub values: serde_json::Map<String, serde_json::Value>,
    /// How a knob's position reads out, keyed by its mix key (`hpf_mix`).
    ///
    /// Anything not named here reads as a percentage, which is what a wet/dry
    /// send is and what a filter corner is not.
    pub units: Vec<(String, KnobUnit)>,
    /// Every parameter that is a list of names rather than a number.
    ///
    /// **General, because there are three of them now.** The delay's time was
    /// the only one, and it was two dedicated fields; a filter slope is the
    /// same shape of thing — a short list a row steps through — and a second
    /// and third copy of "divisions / default_division" is how the two drift.
    pub choices: Vec<ChoiceParam>,
}

/// What a knob's number means, for the one line of text under its name.
///
/// **The host decides, because the host owns the sweep.** `ivory-ui` cannot
/// name `HPF_HZ` — it is a DSP constant on the other side of the firewall —
/// but it can be handed two ends and told the shape between them, which is a
/// display mapping and not a filter.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum KnobUnit {
    /// 0..=1 as a whole-number percentage. The default for everything.
    #[default]
    Percent,
    /// A frequency, swept **exponentially** from `low` to `high` — which is
    /// what a frequency control is, and why an octave is the same distance
    /// everywhere on the dial.
    Hertz { low: f32, high: f32 },
    /// A level, swept **linearly in decibels** from `low` to `high`, because
    /// decibels already are the logarithm and taking it twice would put the
    /// useful half of a threshold in the last eighth of the travel.
    Decibels { low: f32, high: f32 },
}

/// One parameter whose values are named rather than continuous.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChoiceParam {
    /// The settings key, e.g. `delay_division`.
    pub key: String,
    /// `(key, label)`, in the order a row steps through them.
    pub options: Vec<(String, String)>,
    /// Which option applies when the settings file says nothing.
    pub default: String,
}

/// One editable parameter of a patch.
///
/// **Untyped on purpose, like every other thing the host pushes in.**
/// `ivory-ui` cannot name a `Voice` and has no business knowing what keyboard
/// level scaling is; it draws a row with a name and a number in it and reports
/// which number changed. What that means to six operators is the DSP's problem,
/// on the other side of the firewall.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchParam {
    pub name: String,
    pub value: i32,
    pub max: i32,
    /// Names for each value, when the parameter is a CHOICE rather than a
    /// number: an LFO wave is "triangle", not 0. Empty means a plain number.
    pub choices: Vec<String>,
    /// A word after the number — "Hz", "semitones". Empty for most.
    pub unit: String,
}

/// One page of the patch editor: the six operators, and the patch itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchGroup {
    pub title: String,
    pub params: Vec<PatchParam>,
}

/// Everything the patch editor shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PatchEdit {
    /// The patch's own name, ten characters in the format.
    pub name: String,
    /// 0..=31. Shown as 1..=32, the way every chart prints it.
    pub algorithm: usize,
    /// Where each operator's output goes: 0 is the speaker, 1..=6 is the
    /// operator it modulates. Pushed in because the routing table lives with
    /// the synth, and the diagram is the one thing this editor draws.
    pub routing: [u8; 6],
    /// Which operator the feedback loop is taken from, 1-based.
    pub feedback_op: u8,
    pub groups: Vec<PatchGroup>,
    /// Where a Save would write, for the editor to say so. Empty until the
    /// host knows.
    pub bank_path: String,
}

/// What the host found in a DX7 cartridge, for the picker to show.
///
/// **Names, not voices.** `ivory-ui` cannot parse SysEx and has no business
/// knowing what an operator is; it draws a list and reports an index, and the
/// host turns that index back into a patch. That is the same firewall the
/// plugin picker keeps by holding paths rather than modules.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CartridgeInfo {
    /// What the cartridge calls itself. Empty means none is loaded.
    pub bank: String,
    /// It failed its own checksum. Loaded anyway; worth showing.
    pub bad_checksum: bool,
    /// The patch names, in cartridge order.
    pub voices: Vec<String>,
    /// Why the last attempt failed, if it did. Empty on success, and on
    /// failure everything above is left as it was.
    pub error: String,
}

/// Why a file is being asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePurpose {
    /// A DX7 cartridge to load into the built-in FM instrument.
    Cartridge,
    /// An audio file to play along to.
    BackingTrack,
}

/// Why a folder is being asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirPurpose {
    /// Where takes are written.
    RecordRoot,
    /// Another place to look for VST3 bundles.
    PluginFolder,
}

/// A source of MIDI the app picks for itself.
///
/// `Send` because the app that owns it has to be: `nih_plug_egui` requires the
/// editor's state to be `'static + Send`, and a trait object is only as `Send`
/// as its bound says.
pub trait MidiPorts: Send {
    /// Every input port that could be opened, right now. Re-read each time the
    /// dialog opens: devices are unplugged while the app is running.
    fn list(&self) -> Vec<String>;

    /// Open this port, closing whatever was open. `Err` carries a message
    /// meant for the user, so it says what failed rather than which type did.
    fn connect(&mut self, name: &str, tx: mpsc::Sender<MidiEvent>) -> Result<(), String>;

    /// The port currently open, if any.
    fn current(&self) -> Option<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recording fake, which is the other reason this is a trait: the port
    /// dialog and its two menu actions had no way to be tested at all while
    /// they went straight to a real device.
    #[derive(Default)]
    struct Fake {
        available: Vec<String>,
        open: Option<String>,
    }

    impl MidiPorts for Fake {
        fn list(&self) -> Vec<String> {
            self.available.clone()
        }
        fn connect(&mut self, name: &str, _tx: mpsc::Sender<MidiEvent>) -> Result<(), String> {
            if self.available.iter().any(|n| n == name) {
                self.open = Some(name.to_owned());
                Ok(())
            } else {
                Err(format!("no port named '{name}'"))
            }
        }
        fn current(&self) -> Option<String> {
            self.open.clone()
        }
    }

    #[test]
    fn a_port_source_can_be_faked_without_a_device() {
        let (tx, _rx) = mpsc::channel();
        let mut p = Fake {
            available: vec!["Scarlett 2i2".into(), "USB-MIDI 1".into()],
            open: None,
        };
        assert_eq!(p.current(), None);
        assert!(p.connect("USB-MIDI 1", tx.clone()).is_ok());
        assert_eq!(p.current().as_deref(), Some("USB-MIDI 1"));
        assert!(p.connect("Nothing", tx).is_err());
        assert_eq!(
            p.current().as_deref(),
            Some("USB-MIDI 1"),
            "a failed connect must not close the port that was working"
        );
    }

    /// The trait object has to be `Send`, or `IvoryApp` stops being `Send` and
    /// `create_egui_editor` refuses it with a wall of trait errors in the
    /// plugin crate rather than here.
    #[test]
    fn the_trait_object_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Box<dyn MidiPorts>>();
        assert_send::<Box<dyn CaptureDevices>>();
    }

    /// A fake camera list, for the same reason the fake MIDI port exists: the
    /// Recorder band's device rows are otherwise untestable without hardware.
    #[derive(Default)]
    struct FakeDevices {
        present: Vec<DeviceInfo>,
        open: Option<String>,
    }

    impl CaptureDevices for FakeDevices {
        fn list(&self) -> Vec<DeviceInfo> {
            self.present.clone()
        }
        fn open(&mut self, uid: &str) -> Result<(), String> {
            if uid.is_empty() {
                self.open = None;
                return Ok(());
            }
            if self.present.iter().any(|d| d.uid == uid) {
                self.open = Some(uid.to_owned());
                Ok(())
            } else {
                Err(format!("no device with uid '{uid}'"))
            }
        }
        fn current(&self) -> Option<String> {
            self.open.clone()
        }
        fn current_name(&self) -> Option<String> {
            let uid = self.open.as_ref()?;
            self.present
                .iter()
                .find(|d| &d.uid == uid)
                .map(|d| d.name.clone())
        }
    }

    /// Two cameras that call themselves the same thing is the case that decides
    /// uid-not-name, so it is the case the fake is asked about.
    #[test]
    fn two_devices_sharing_a_name_are_still_told_apart() {
        let mut d = FakeDevices {
            present: vec![
                DeviceInfo {
                    uid: "0x1400000046d0825".into(),
                    name: "HD Pro Webcam C920".into(),
                    default: true,
                },
                DeviceInfo {
                    uid: "0x1a11000046d0825".into(),
                    name: "HD Pro Webcam C920".into(),
                    default: false,
                },
            ],
            open: None,
        };
        assert!(d.open("0x1a11000046d0825").is_ok());
        assert_eq!(d.current().as_deref(), Some("0x1a11000046d0825"));
        assert_eq!(d.current_name().as_deref(), Some("HD Pro Webcam C920"));
        assert!(d.open("gone").is_err());
        assert_eq!(
            d.current().as_deref(),
            Some("0x1a11000046d0825"),
            "a failed open must not close the device that was working"
        );
    }

    /// The None row. Expressed as an empty uid rather than a second method,
    /// which is the kind of thing that is obvious for a week and then not.
    #[test]
    fn an_empty_uid_closes_the_device() {
        let mut d = FakeDevices {
            present: vec![DeviceInfo {
                uid: "a".into(),
                name: "A".into(),
                default: true,
            }],
            open: Some("a".into()),
        };
        assert!(d.open("").is_ok());
        assert_eq!(d.current(), None);
        assert_eq!(d.current_name(), None);
    }
}
