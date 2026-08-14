//! Tangent as a VST3 plugin.
//!
//! This crate is a shell. Everything you see when it opens is
//! `ivory_ui::app::IvoryApp` — the same code the standalone runs, not a
//! lookalike — and everything this file does is answer the three questions a
//! DAW asks that a desktop window never does:
//!
//!   1. **Where do the notes come from?** The host, through `process()`, on
//!      the audio thread. They cross to the GUI through a lock-free queue,
//!      because the editor holds its state behind a mutex for a whole frame
//!      and the audio thread must never wait on that.
//!   2. **Where does the state live?** In the DAW project, not in
//!      `~/.config/ivory/settings.json`. That file is shared by the standalone
//!      and by every other instance; a plugin that wrote it would let the last
//!      window you touched decide everyone's colours.
//!   3. **Who owns the window?** The host. `Caps::PLUGIN` says so, and
//!      `ivory-ui` reads it at every branch point — menus and dialogs draw in
//!      the canvas instead of opening windows, and not one
//!      `ViewportCommand` is sent.
//!
//! There is no audio. `AUDIO_IO_LAYOUTS` is empty and `process()` returns
//! without touching the buffer: Tangent reads what you play, it does not make
//! a sound. In most hosts that means loading it on a MIDI or instrument track
//! and routing your keyboard to it.

use crossbeam::queue::ArrayQueue;
use ivory_ui::app::IvoryApp;
use ivory_ui::host::Caps;
use ivory_ui::midi_event::MidiEvent;
use ivory_ui::settings::Settings;
use nih_plug::params::persist::PersistentField;
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, EguiState};
use baseview::PhySize;
use parking_lot::Mutex;
use std::sync::Arc;

/// How many note events can be waiting when the GUI is between frames.
///
/// At a 50ms repaint cadence and ten fingers this is roughly two seconds of
/// the fastest playing anyone manages. It is a fixed allocation made once, so
/// the only cost of being generous is memory; the only cost of being stingy is
/// a dropped note-off, which sticks a key on screen until it is played again.
const NOTE_QUEUE: usize = 1024;

/// The width a new instance opens at.
///
/// The standalone is 1300 at 100%. A DAW rack is narrower than a desktop, so
/// this opens at about two thirds and lets the host resize from there — the
/// layout is proportional, so it stays correct at any width.
///
/// The HEIGHT is not a constant, because it cannot be: it depends on how many
/// bands the user has turned on, and the theory band alone is 300 points at
/// full width. A fixed height would open a first instance showing a slice of
/// the app, with nothing on screen to explain why.
const EDITOR_W: u32 = 900;

/// Bounds on what the Size menu may ask the host for. Below the minimum the
/// piano stops being a piano; above the maximum it stops fitting on a screen.
const MIN_W: u32 = 420;
const MIN_H: u32 = 90;
const MAX_W: u32 = 4000;
const MAX_H: u32 = 2400;

/// Everything the plugin keeps between the audio thread and the editor.
struct Tangent {
    params: Arc<TangentParams>,
    /// Audio thread pushes, GUI thread pops. Never locked, never allocated in.
    notes: Arc<ArrayQueue<MidiEvent>>,
    /// The running app, kept ALIVE across editor open and close.
    ///
    /// A DAW opens and closes a plugin window freely, and `create_egui_editor`
    /// takes its state by value — so if the app were built there, closing the
    /// window would silently reset the tuning, the theory selection and every
    /// note you had placed by hand. Holding it here instead means the window
    /// is a lid, not a lifetime.
    editor: Arc<Mutex<Option<IvoryApp>>>,
}

#[derive(Params)]
struct TangentParams {
    /// The editor's size, remembered per instance by the host.
    #[persist = "editor-state"]
    editor_state: Arc<EguiState>,

    /// Everything else, as the same JSON the settings file holds.
    ///
    /// A blob rather than a parameter each, because none of these is a
    /// parameter: a DAW would offer to automate "dark mode" and to write
    /// envelopes for the fretboard tuning. `Settings` already round-trips
    /// through this exact text, unknown keys and all, so a project saved by
    /// the plugin and a settings file written by the app say the same thing.
    #[persist = "settings"]
    settings: Mutex<String>,
}

impl Default for TangentParams {
    fn default() -> Self {
        let settings = Settings::load();
        Self {
            // Seeded from the user's own settings, so a first insert looks
            // like the app they already know. Read once, never written back.
            editor_state: EguiState::from_size(EDITOR_W, {
                let h = ivory_ui::app::natural_size(&settings, EDITOR_W as f32).y;
                h.round().clamp(80.0, 2000.0) as u32
            }),
            settings: Mutex::new(settings.to_json()),
        }
    }
}

impl Default for Tangent {
    fn default() -> Self {
        Self {
            params: Arc::new(TangentParams::default()),
            notes: Arc::new(ArrayQueue::new(NOTE_QUEUE)),
            editor: Arc::new(Mutex::new(None)),
        }
    }
}

impl Plugin for Tangent {
    const NAME: &'static str = "Tangent";
    const VENDOR: &'static str = "Ganten";
    const URL: &'static str = "https://ganten.neocities.org";
    const EMAIL: &'static str = "keys@ivorymidi.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    /// No audio at all. Tangent reads what you play; it does not make a sound,
    /// and claiming a stereo pair it would only pass through invites a host to
    /// route audio into a dead end.
    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[];

    const MIDI_INPUT: MidiConfig = MidiConfig::MidiCCs;
    const MIDI_OUTPUT: MidiConfig = MidiConfig::None;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        let home = self.editor.clone();
        let notes = self.notes.clone();
        let params = self.params.clone();

        create_egui_editor(
            self.params.editor_state.clone(),
            (),
            Default::default(),
            {
                let home = home.clone();
                let params = params.clone();
                move |ctx, _queue, _| {
                    // Built once per window, but only constructed once per
                    // instance: reopening finds the app already there.
                    let mut slot = home.lock();
                    if slot.is_none() {
                        let settings = Settings::from_json(&params.settings.lock());
                        *slot = Some(IvoryApp::new(ctx, settings, Caps::PLUGIN));
                    } else {
                        // A new window means a new GL context and a new font
                        // atlas, so the faces have to go back on.
                        if let Some(app) = slot.as_ref() {
                            app.install_fonts(ctx);
                        }
                    }
                }
            },
            move |ctx, setter, queue, _| {
                let mut slot = home.lock();
                let Some(app) = slot.as_mut() else {
                    return;
                };
                // Drain what the audio thread has played since the last frame.
                // Bounded by the queue, so a stalled GUI cannot grow it.
                while let Some(ev) = notes.pop() {
                    app.feed(ev);
                }
                app.frame(ctx);

                // ── Size ───────────────────────────────────────────────────
                //
                // A plugin cannot resize its own window; it has to ASK, and
                // the host decides. `nih_plug_egui` ships a drag corner for
                // this, but its private `set_requested_size` is the only way
                // in and the corner drew invisibly over a black background —
                // so the editor opened at one size and stayed there forever.
                //
                // The Size submenu is the same control the standalone has, and
                // the handshake below is exactly what the adapter's own corner
                // performs, done with public API: tell `EguiState` the new
                // size FIRST (the host reads it back through `Editor::size()`),
                // then ask, and only touch the window once the host agrees.
                if let Some(want) = app.take_pending_resize() {
                    let w = (want.x.round() as u32).clamp(MIN_W, MAX_W);
                    let h = (want.y.round() as u32).clamp(MIN_H, MAX_H);
                    // `Arc::try_unwrap` rather than serde: `from_size` hands
                    // back a fresh `Arc` with a count of one, so this always
                    // succeeds, and `PersistentField::set` is the public way
                    // to move a size into the state the host will read.
                    if let Ok(v) = Arc::try_unwrap(EguiState::from_size(w, h)) {
                        params.editor_state.set(v);
                    }
                    if setter.raw_context.request_resize() {
                        let scale = ctx.pixels_per_point();
                        queue.resize(PhySize::new(
                            (w as f32 * scale).round() as u32,
                            (h as f32 * scale).round() as u32,
                        ));
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                            egui::Vec2::new(w as f32, h as f32),
                        ));
                    }
                }
                // Write the settings back into the project every frame. It is
                // a string compare and a lock the audio thread never takes,
                // and the alternative is remembering to do it at each of the
                // twenty-four places a setting can change.
                let now = app.settings_json();
                let mut stored = params.settings.lock();
                if *stored != now {
                    *stored = now;
                }
            },
        )
    }

    /// Notes in, nothing out.
    ///
    /// Allocation-free and lock-free by construction: `ArrayQueue::push` on a
    /// pre-allocated queue, and nothing else. A full queue drops the oldest
    /// event rather than blocking, because blocking here is an audio dropout
    /// and a dropped note is a key that looks stuck.
    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        while let Some(event) = context.next_event() {
            let translated = match event {
                // A note-on with velocity 0 is a note-off. The host has
                // usually already normalised this, but the rule is the app's
                // (spec §10) and it costs one branch to be sure.
                NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => {
                    Some(MidiEvent::NoteOn {
                        note,
                        velocity: (velocity * 127.0).round().clamp(1.0, 127.0) as u8,
                    })
                }
                NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } => {
                    Some(MidiEvent::NoteOff { note })
                }
                // CC64 is the damper pedal, and the only controller the app
                // reads. `value` is normalised, so the >= 64 threshold of the
                // MIDI spec is >= 0.5 here.
                NoteEvent::MidiCC { cc, value, .. } if cc == control_change::DAMPER_PEDAL => {
                    Some(MidiEvent::Sustain { down: value >= 0.5 })
                }
                _ => None,
            };
            if let Some(ev) = translated {
                // `force_push` overwrites the oldest rather than failing, so a
                // GUI that has not run for a while loses the start of a phrase
                // instead of the end of it — and the end is what is sounding.
                self.notes.force_push(ev);
            }
        }

        // Keep the editor repainting on the GUI's own cadence, not on ours.
        ProcessStatus::Normal
    }
}

impl Vst3Plugin for Tangent {
    /// Sixteen bytes, fixed forever: a host identifies the plugin by this, so
    /// changing it orphans every project that has one loaded.
    const VST3_CLASS_ID: [u8; 16] = *b"TangentChordView";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Instrument,
        Vst3SubCategory::Analyzer,
        Vst3SubCategory::Tools,
    ];
}

nih_export_vst3!(Tangent);
