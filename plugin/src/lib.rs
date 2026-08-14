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
use nih_plug::prelude::*;
use nih_plug_egui::{create_egui_editor, EguiState};
use parking_lot::Mutex;
use std::sync::Arc;

/// How many note events can be waiting when the GUI is between frames.
///
/// At a 50ms repaint cadence and ten fingers this is roughly two seconds of
/// the fastest playing anyone manages. It is a fixed allocation made once, so
/// the only cost of being generous is memory; the only cost of being stingy is
/// a dropped note-off, which sticks a key on screen until it is played again.
const NOTE_QUEUE: usize = 1024;

/// The editor size the host is told about before it has been told otherwise.
///
/// The standalone's natural width is 1300 with a 150-point piano. A DAW rack
/// is narrower than a desktop, so this opens at half that and lets the host
/// resize: the layout is proportional, so it stays correct at any width.
const EDITOR_W: u32 = 900;
const EDITOR_H: u32 = 260;

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
        Self {
            editor_state: EguiState::from_size(EDITOR_W, EDITOR_H),
            // Seeded from the user's own settings, so a first insert looks
            // like the app they already know. Read once, never written back.
            settings: Mutex::new(Settings::load().to_json()),
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
    const VENDOR: &'static str = "ganten";
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
            move |ctx, _setter, _queue, _| {
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
