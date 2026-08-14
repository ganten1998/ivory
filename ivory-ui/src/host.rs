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
    }
}
