//! What the thing hosting us can actually do.
//!
//! Tangent runs in two places that differ less than they look. The desktop app
//! owns a window, opens MIDI devices, and can spawn more windows. A VST3 editor
//! owns none of that: it is handed one child window by the host, receives its
//! notes from the host, and is resized by the host.
//!
//! `Caps` is how the shared GUI asks, instead of assuming. It is deliberately
//! plain data rather than a trait: every field is a fact about the host that
//! the UI needs at a branch point, and a struct of bools can be built in a test
//! for a host that does not exist yet.
//!
//! The rule for adding a field: it must name a CAPABILITY, not a host. Code
//! that says `if caps.child_windows` still reads correctly the day someone
//! writes a CLAP or AUv3 build; code that says `if is_plugin` has to be
//! revisited every time.

/// What this host permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Caps {
    /// The UI may open additional OS windows.
    ///
    /// **False does not mean "the calls fail".** In a plugin,
    /// `egui::Context::show_viewport_immediate` still RUNS: the context is
    /// created with `embed_viewports: true`, so it invokes the closure inline
    /// and opens a second `CentralPanel` under the same id as the first,
    /// painting garbage over the piano. A hard error would have been kinder.
    /// Anything that would have been a window has to be drawn in the canvas
    /// instead, and this is the flag that decides.
    pub child_windows: bool,
    /// The chord readout and the fretboard may be popped into their own
    /// windows. Requires `child_windows`; a plugin has neither.
    pub detachable: bool,
    /// The UI may set its own size by sending viewport commands. A plugin
    /// editor may NOT: `egui-baseview` honours `ViewportCommand::InnerSize`,
    /// so an ungated one reaches into the DAW and resizes the editor behind
    /// the host's back. This also gates the borderless toggle, the geometry
    /// write-back and the offscreen rescue — everything that treats the window
    /// as ours.
    pub window_sizing: bool,
    /// A size percentage is a meaningful thing to offer.
    ///
    /// True for BOTH hosts, and the distinction from `window_sizing` is the
    /// point. A plugin cannot set its own size, but it can ASK: VST3 has
    /// `IPlugFrame::resizeView` and nih-plug wires it to
    /// `GuiContext::request_resize`. So the Size submenu belongs in a plugin,
    /// it just takes a different road to the same place — the host is told,
    /// and the host decides.
    pub size_presets: bool,
    /// The UI chooses its own MIDI input. A plugin is given notes by the host
    /// and has no device list to offer.
    pub midi_ports: bool,
    /// Settings changes may be written to `~/.config/ivory/`. A plugin shares
    /// that file with the standalone and with every other instance, so the
    /// last window closed would otherwise decide everyone's colours.
    pub persist_global_settings: bool,
    /// The UI may open a camera or an audio input.
    ///
    /// False in a plugin, which has no business opening a device behind the
    /// DAW's back, and false in a Minimal build, where the code to do it is not
    /// linked at all. **The Recorder band's height must be exactly zero when
    /// this is false** — `fit_bands` shrinks the piano to make room for any
    /// band with a non-zero height, including one that will never be painted.
    pub capture_devices: bool,
    /// The UI may create take directories and write media outside the config
    /// directory.
    ///
    /// Deliberately separate from `persist_global_settings`, which is about one
    /// specific shared file. Conflating "shares settings.json" with "may write
    /// a 2 GB video into the user's Movies folder" is exactly the mistake this
    /// struct's own rule warns against. False in a plugin for a second reason
    /// too: multiple instances and offline bounce make plugin-side file writing
    /// actively dangerous.
    pub write_user_files: bool,
    /// A "Choose folder..." button is a meaningful thing to offer.
    ///
    /// False in a plugin, where a native modal raised inside a DAW is a hang
    /// report waiting to happen — and where, with `persist_global_settings`
    /// false, there would be nowhere to put the answer anyway.
    pub native_file_dialogs: bool,
}

impl Caps {
    /// The standalone app: it can do everything.
    pub const DESKTOP: Caps = Caps {
        child_windows: true,
        detachable: true,
        window_sizing: true,
        size_presets: true,
        midi_ports: true,
        persist_global_settings: true,
        capture_devices: true,
        write_user_files: true,
        native_file_dialogs: true,
    };

    /// A plugin editor: one host-owned window, host-supplied notes, host-driven
    /// size, and shared config it must not fight over.
    pub const PLUGIN: Caps = Caps {
        child_windows: false,
        detachable: false,
        window_sizing: false,
        size_presets: true,
        midi_ports: false,
        persist_global_settings: false,
        capture_devices: false,
        write_user_files: false,
        native_file_dialogs: false,
    };

    /// The standalone, built **without** the recorder (`--no-default-features`).
    ///
    /// Everything the chord display needs and nothing that opens a device or
    /// writes a file the user did not ask for. This is a real shipped variant,
    /// not a debug mode — see `docs/RECORDER-PLAN.md` §12a.
    ///
    /// The point of it is **not** binary size. Measured, the whole recorder and
    /// plugin-host dependency set is about 50 KB. The point is that a Full build
    /// must carry `com.apple.security.cs.disable-library-validation` and
    /// `allow-jit` in order to host plugins at all, plus camera and microphone
    /// usage strings — and someone who wants a chord display should not have to
    /// run a binary that can load arbitrary third-party code. A Minimal build
    /// ships **no entitlements**, exactly as 3.0.0 does.
    ///
    /// It differs from `DESKTOP` in exactly the two capabilities whose code is
    /// absent, and in nothing else: it still owns a real window, still opens its
    /// own MIDI device, still detaches, still raises native dialogs.
    pub const MINIMAL: Caps = Caps {
        capture_devices: false,
        write_user_files: false,
        ..Caps::DESKTOP
    };
}

impl Default for Caps {
    /// Desktop, because that is the host every existing caller has and a
    /// default that silently disabled things would be a bug generator.
    fn default() -> Self {
        Self::DESKTOP
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detaching opens a window, so it cannot outlive `child_windows`. Stated
    /// as a test because the two are easy to set independently and the failure
    /// is a plugin trying to open an OS window, which does not fail loudly.
    #[test]
    fn detaching_requires_being_able_to_open_a_window() {
        for c in [Caps::DESKTOP, Caps::PLUGIN, Caps::default()] {
            assert!(
                !c.detachable || c.child_windows,
                "{c:?} claims it can detach without being able to open a window"
            );
        }
    }

    #[test]
    fn the_plugin_gives_up_everything_the_desktop_owns() {
        assert_eq!(Caps::default(), Caps::DESKTOP);
        let p = Caps::PLUGIN;
        assert!(!p.child_windows && !p.detachable && !p.window_sizing);
        assert!(
            p.size_presets,
            "a plugin can still ASK the host for a size, and must be able to"
        );
        assert!(!p.midi_ports, "a plugin is handed its notes");
        assert!(!p.persist_global_settings, "a plugin shares that file");
        assert!(!p.capture_devices, "a plugin must not open a camera");
        assert!(
            !p.write_user_files,
            "multiple instances and offline bounce make plugin-side file \
             writing actively dangerous"
        );
        assert!(
            !p.native_file_dialogs,
            "a native modal inside a DAW is a hang report waiting to happen"
        );
    }

    /// A Minimal build is the desktop MINUS the two things whose code is not
    /// linked, and nothing else. Stated as a test because the tempting mistake
    /// is to make Minimal a cut-down app — it is not; it is the same app without
    /// a recorder.
    #[test]
    fn minimal_gives_up_the_recorder_and_nothing_else() {
        let m = Caps::MINIMAL;
        assert!(!m.capture_devices && !m.write_user_files);
        assert!(
            m.child_windows && m.detachable && m.window_sizing && m.midi_ports,
            "Minimal still owns a real window and a real MIDI device"
        );
        assert!(
            m.persist_global_settings && m.native_file_dialogs && m.size_presets,
            "nothing else is taken away"
        );
    }

    /// Writing a take is opening a device's worth of trust. A build that may
    /// capture but may not write has nowhere to put a recording; a build that
    /// may write but not capture has nothing to put there. Neither is a
    /// configuration anyone wants, and a `..Caps::DESKTOP` spread makes it easy
    /// to create one by editing a single field.
    #[test]
    fn capture_and_writing_files_travel_together() {
        for c in [Caps::DESKTOP, Caps::PLUGIN, Caps::MINIMAL, Caps::default()] {
            assert_eq!(
                c.capture_devices, c.write_user_files,
                "{c:?} can do one half of recording and not the other"
            );
        }
    }
}
