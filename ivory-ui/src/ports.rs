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
