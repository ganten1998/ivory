//! The desktop half of the app: an eframe window, and a real MIDI device.
//!
//! Everything that draws lives in `ivory_ui::app::IvoryApp`, which the VST3
//! build also uses. What is left here is exactly the three things a standalone
//! has and a plugin editor does not — an `eframe::App` impl, a `midir`
//! connection, and a command line — plus the `Caps` value that says so.
//!
//! The wrapper struct is not ceremony. `impl eframe::App for IvoryApp` cannot
//! be written from this crate: the orphan rule forbids implementing a foreign
//! trait for a foreign type. That is a feature here rather than an obstacle —
//! it is the compiler stating that eframe is this crate's business and not the
//! shared crate's.

use crate::midi;
use ivory_ui::app::IvoryApp;
use ivory_ui::host::Caps;
use ivory_ui::midi_event::MidiEvent;
use ivory_ui::ports::MidiPorts;
use ivory_ui::settings::Settings;
use std::sync::mpsc;

/// A real MIDI input, opened with `midir`.
///
/// Holds the egui context so the callback thread can wake the UI on every
/// event: repaints are event-driven rather than busy-looped (D-UI-3). A plugin
/// needs no equivalent — the host calls `process()` and then the editor.
pub struct DeviceMidi {
    ctx: egui::Context,
    conn: Option<midi::MidiConnection>,
}

impl DeviceMidi {
    pub fn new(ctx: egui::Context) -> Self {
        Self { ctx, conn: None }
    }

    /// The startup priority chain (spec §10). Silent on failure by design: the
    /// app runs without MIDI rather than opening a dialog nobody asked for.
    pub fn auto_connect(&mut self, tx: mpsc::Sender<MidiEvent>) {
        self.conn = midi::auto_connect(tx, self.ctx.clone());
    }
}

impl MidiPorts for DeviceMidi {
    fn list(&self) -> Vec<String> {
        midi::list_port_names()
    }

    fn connect(&mut self, name: &str, tx: mpsc::Sender<MidiEvent>) -> Result<(), String> {
        // Close the old port FIRST (parity): some drivers refuse a second open
        // of the same device, so holding both across the switch fails on the
        // machines that matter and works on the ones that do not.
        self.conn = None;
        self.conn = Some(midi::connect_by_name(name, tx, self.ctx.clone())?);
        Ok(())
    }

    fn current(&self) -> Option<String> {
        self.conn.as_ref().map(|c| c.port_name.clone())
    }
}

/// The standalone app: `IvoryApp`, plus the eframe trait impl it cannot carry.
pub struct DesktopApp(IvoryApp);

impl DesktopApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        settings: Settings,
        cli_port: Option<String>,
    ) -> Self {
        // Dev switch: run the DESKTOP binary through the PLUGIN's capability
        // set, so the in-canvas menu and dialogs can be looked at, screenshot
        // and debugged in a window — without a DAW, a rebuild, an installer
        // and a plugin rescan between each attempt.
        //
        //   IVORY_INLINE=1 /Applications/Tangent.app/Contents/MacOS/tangent
        //
        // Environment-gated, so a normal launch cannot reach it. It is a
        // faithful test of the path: the same `Caps::PLUGIN` the plugin uses,
        // which also means the window will not resize itself.
        //   IVORY_INLINE=menu also opens the menu on the first frame.
        let caps = if matches!(
            std::env::var("IVORY_INLINE").as_deref(),
            Ok("1") | Ok("menu")
        ) {
            eprintln!("IVORY_INLINE=1: running with the plugin's capabilities");
            Caps::PLUGIN
        } else {
            Caps::DESKTOP
        };
        let mut app = IvoryApp::new(&cc.egui_ctx, settings, caps);
        let mut device = DeviceMidi::new(cc.egui_ctx.clone());
        // `-p NAME` beats the priority chain, and a bad name is not fatal: the
        // app opens with no MIDI, which is the same outcome as no device.
        match cli_port {
            Some(name) => {
                if let Err(e) = device.connect(&name, app.midi_sender()) {
                    eprintln!("could not open MIDI port {name:?}: {e}");
                }
            }
            None => device.auto_connect(app.midi_sender()),
        }
        app.set_ports(Some(Box::new(device)));
        Self(app)
    }
}

impl eframe::App for DesktopApp {
    /// eframe hands over a Context; everything below wants a Ui, and
    /// `IvoryApp::frame` is the bridge all three hosts share.
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.0.frame(ctx);
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        IvoryApp::CLEAR_COLOR
    }
}
