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
    }
}
