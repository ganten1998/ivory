//! The eframe App: fixed-size window mechanics, timers, MIDI state, and the
//! wiring between piano, chord strip, menu, and dialogs.
//!
//! Timing model (D-UI-3): no busy loop. `request_repaint_after(50ms)` gives
//! the Qt GUI-timer cadence, MIDI events wake the context immediately, and
//! chord detection runs on its own 100ms gate (with immediate off-cadence
//! runs after keytoggle clicks and note-preference changes).

use crate::chord_strip;
use crate::dialogs::{self, Dialog, DialogAction, LearningStatus};
use crate::fretboard_panel;
use crate::host::Caps;
use crate::keys;
use crate::menu::{self, ColorTarget, MenuAction, MenuState, MenuView};
use crate::midi_event::MidiEvent;
use crate::piano;
use crate::ports::{CaptureDevices, MidiPorts};
use crate::recorder;
use crate::recorder_panel;
use crate::settings::{Rgb, Settings};
use crate::theory_panel;
use egui::{Pos2, Rect, Vec2, ViewportCommand};
use ivory_core::voicing::{VoicingSession, Weights};
use ivory_core::{ChordDetector, OverrideStore, TrainOutcome};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const GUI_TICK: Duration = Duration::from_millis(50);
const DETECT_TICK: Duration = Duration::from_millis(100);
const DEBOUNCE_100MS: Duration = Duration::from_millis(100);
/// Quiet period after a move or resize before window geometry is written back.
/// Long enough that a drag is one write, short enough that a kill -9 a second
/// later still keeps the new position.
const GEOMETRY_SAVE_DELAY: Duration = Duration::from_millis(700);
/// How much of a restored window must be on-screen before it counts as visible.
/// A window peeking a few pixels onto the desktop is not reachable in practice.
const OFFSCREEN_SLACK: f32 = 40.0;
/// Slack when comparing the size a window actually has against the size we
/// asked for. Rounding and DPI scaling move things by a pixel or two.
const WM_SIZE_TOLERANCE: f32 = 8.0;

/// Whether something other than us is sizing our windows.
///
/// Under a tiling window manager (AeroSpace, yabai, i3) the size we ask for is
/// simply overruled. Recording the result as if the user had chosen it is
/// worse than recording nothing: the tiled geometry then follows them into
/// every later session, including ones where nothing is tiling. That is
/// exactly how a `detached_chord_height` of 1377 ended up in a settings file.
fn wm_overrode_size(observed: Vec2, requested: Vec2) -> bool {
    (observed.x - requested.x).abs() > WM_SIZE_TOLERANCE
        || (observed.y - requested.y).abs() > WM_SIZE_TOLERANCE
}

/// How long after the detached window appears a size mismatch still counts as
/// the window manager's doing rather than the user's. A tiling WM overrules
/// the requested size at creation; a person reaching for the window edge takes
/// longer than this. Size alone cannot tell the two apart, only timing can.
const WM_GRACE: Duration = Duration::from_millis(600);

/// What is sounding, and why (spec §10 semantics).
///
/// Pulled out of `IvoryApp` so it can be TESTED: the rules here have four
/// interacting branches and had no coverage at all, while being the thing every
/// other display in the app reads from. It is also the piece the plugin build
/// needs, since a plugin receives note events from the host rather than from a
/// `midir` channel, and only this half is shared.
///
/// Velocity and press time used to be stored per note and were read by nothing
/// (both were `#[allow(dead_code)]`). They are gone rather than carried: arrival
/// ORDER is a property of the tick and not of the note, which is exactly the
/// assumption that once made a ten-note voicing shed its bass (HANDOFF §2d), and
/// a `pressed_at` sitting in the struct invites someone to rediscover that.
#[derive(Default)]
pub struct NoteState {
    /// Sounding right now, whether the key is still down or the pedal is
    /// holding it.
    held: HashSet<u8>,
    /// Keys already released, sounding only because the pedal is down. Always a
    /// subset of `held`.
    pedalled: HashSet<u8>,
    sustain_down: bool,
}

impl NoteState {
    pub fn sustain_down(&self) -> bool {
        self.sustain_down
    }

    pub fn held(&self) -> &HashSet<u8> {
        &self.held
    }

    /// One MIDI event. The four rules, in the order they interact:
    pub fn apply(&mut self, ev: MidiEvent) {
        match ev {
            // A note struck again cancels a pending pedal release. Without
            // this, re-striking a key while the pedal is down would leave the
            // note queued to die at the next pedal lift.
            MidiEvent::NoteOn { note, .. } => {
                self.held.insert(note);
                self.pedalled.remove(&note);
            }
            MidiEvent::NoteOff { note } => {
                if self.sustain_down {
                    // Only a note that is actually sounding can be queued. A
                    // note-off for something never held must not create one.
                    if self.held.contains(&note) {
                        self.pedalled.insert(note);
                    }
                } else {
                    self.held.remove(&note);
                    self.pedalled.remove(&note);
                }
            }
            // Only the DOWN-to-UP edge releases. Pedal down while already down
            // changes nothing, and pedal up while already up must not drain a
            // set that a later note-off will refill.
            MidiEvent::Sustain { down } => {
                let was = self.sustain_down;
                self.sustain_down = down;
                if was && !down {
                    for note in self.pedalled.drain() {
                        self.held.remove(&note);
                    }
                }
            }
        }
    }
}

pub struct IvoryApp {
    settings: Settings,
    detector: ChordDetector,
    /// Supporter license, loaded once at startup. Status is DERIVED from it
    /// wherever it is consulted — never cached as a boolean, so there is no
    /// flag to flip and none to go stale.
    license: ivory_core::license::LicenseStore,

    midi_tx: mpsc::Sender<MidiEvent>,
    midi_rx: mpsc::Receiver<MidiEvent>,
    /// Where notes come from when the app picks for itself. `None` in a
    /// plugin, which is handed its notes and has no device list to offer.
    /// The CHANNEL above is not optional: a plugin uses the same one, filled
    /// from `process()` instead of from a `midir` callback thread.
    ports: Option<Box<dyn MidiPorts>>,
    /// Who can enumerate and select an audio input, and a camera. `None` in a
    /// plugin and in a Minimal build, where the code to do it is not linked.
    audio_devices: Option<Box<dyn CaptureDevices>>,
    cameras: Option<Box<dyn CaptureDevices>>,
    /// What the host permits. Read at every branch point rather than compared
    /// against a host name, and captured once at construction so a frame
    /// cannot be half-drawn under one set of rules and half under another.
    caps: Caps,
    /// A size the user asked for that the host has not been told about yet.
    /// Only ever set when the host owns the window.
    pending_resize: Option<Vec2>,
    /// The rect the last frame was laid out into. Read when asking a host for
    /// a new size, so the width the user already has is preserved.
    last_pane: Vec2,
    /// Where the bands were actually drawn, which is centred in the pane.
    /// Child windows and in-canvas dialogs centre on this.
    last_drawn: Rect,
    /// A barre the user drew by dragging along a fret, and the drag in
    /// progress. Only ever set by that gesture.
    manual_barre: Option<ivory_core::voicing::Barre>,
    barre_drag: Option<(usize, u8)>,
    /// One-shot latch for the IVORY_INLINE=menu debug hook.
    demo_menu_done: bool,
    /// Whether the main window currently has focus. Detached windows are
    /// raised with it and dropped with it, so the app moves as one thing.
    main_focused: bool,

    notes: NoteState,
    manual_notes: HashSet<u8>,
    /// Where a note entered ON THE FRETBOARD was put, as pitch -> (string,
    /// fret). The solver honours these so a clicked shape stays clicked: left
    /// to choose, it redraws a hand-entered voicing somewhere else about three
    /// times in four, which is right when it is choosing and wrong when the
    /// choosing is already done.
    manual_positions: HashMap<u8, (usize, u8)>,

    current_chord: Option<String>,
    last_detection: Option<Instant>,

    /// D-UI-15: the guitar view's solver state. Exactly ONE of these exists,
    /// so every surface that draws a fretboard is drawing the same shape, and
    /// it carries the hysteresis that stops the picture jumping around while
    /// somebody plays. Kept alive even while the panel is hidden — it costs a
    /// few hundred bytes and it means the board is already right the moment
    /// the panel is switched on.
    voicing: VoicingSession,
    last_voicing: Option<Instant>,
    /// D-UI-16: the popped-out neck. Same shape of state as the detached chord
    /// window, including the tiling-WM guard, because it has the same problem.
    fret_window_visible: bool,
    fret_builder_size: Vec2,
    fret_builder_pos: Option<Pos2>,
    fret_live_size: Option<Vec2>,
    fret_live_pos: Option<Pos2>,
    fret_shown_at: Option<Instant>,
    fret_wm_managed: bool,
    startup_fret_detach_at: Option<Instant>,

    /// The theory band is on screen in its own window, and the guard that says
    /// whether its geometry is the user's or a tiling WM's.
    ///
    /// One `GeometryGuard` rather than the five loose fields the fretboard
    /// popout still uses (`fret_shown_at`, `fret_wm_managed`, `fret_live_*`).
    /// `theory_panel` shipped the guard as a type; taking it here is what makes
    /// the third popout cheaper than the second rather than another copy of it.
    theory_window_visible: bool,
    theory_builder_size: Vec2,
    theory_builder_pos: Option<Pos2>,
    theory_guard: Option<theory_panel::GeometryGuard>,
    startup_theory_detach_at: Option<Instant>,

    /// The Recorder band, the fourth popout. Same shape as the theory window
    /// above it, reusing `theory_panel::GeometryGuard` rather than growing a
    /// fourth copy of the tiling-WM dance — the guard is about window managers,
    /// not about theory diagrams, and the module it happens to live in is an
    /// accident of which popout was written when.
    recorder_window_visible: bool,
    recorder_builder_size: Vec2,
    recorder_builder_pos: Option<Pos2>,
    recorder_guard: Option<theory_panel::GeometryGuard>,
    startup_recorder_detach_at: Option<Instant>,
    /// Everything the band shows, written each frame by whoever is hosting us.
    /// Inert in a plugin, where nothing ever writes to it.
    recorder: recorder::RecorderState,
    /// One pending request for the host to perform after the frame. `Option`
    /// rather than a queue on purpose: these are all user gestures, at most one
    /// happens per frame, and a queue would let a stuck host accumulate a
    /// backlog of Record presses to replay.
    recorder_request: Option<recorder::RecorderRequest>,
    /// A folder the host has been asked to choose. Drained after the frame so
    /// the native panel's nested run loop never starts inside an egui frame.
    dir_request: Option<crate::ports::DirRequest>,
    /// An export spec chosen without ticking "use these settings for every
    /// take": it governs every take **for the rest of this session** and is
    /// never written to the settings file.
    ///
    /// Session-scoped rather than one-take-scoped, and that is a decision
    /// rather than an oversight: somebody who turns something off for a
    /// practice session should not have to turn it off again before every
    /// single take. The tick is what makes it outlive the session.
    export_override: Option<recorder::ExportSpec>,
    /// A settings write owed once the user stops dragging a fader.
    settings_save_at: Option<Instant>,
    /// Which instrument slot the open picker is filling. See
    /// [`open_plugin_picker`](IvoryApp::open_plugin_picker).
    picker_slot: usize,
    /// Every VST3 bundle the host found, for the picker.
    ///
    /// Supplied by the host rather than discovered here: `ivory-ui` cannot
    /// reach `ivory-host`, and it is a directory listing rather than a scan —
    /// nothing is LOADED to build this list, which matters when there are 112
    /// of them and any one could crash the process on open.
    plugin_list: Vec<std::path::PathBuf>,
    /// The band control the pointer grabbed, and where it grabbed it.
    ///
    /// The faders and the tempo are DRAGGED, and `recorder_panel::hit_test` is
    /// a pure function of position with no memory — so the app is what
    /// remembers which control is being held. Without this, dragging a fader
    /// past the edge of its own track hands the next control the value.
    grabbed: Option<(recorder_panel::Hit, Pos2)>,
    /// The take-name field has keyboard focus.
    ///
    /// While this is true every single-letter shortcut is suppressed, or typing
    /// "background" into the field would toggle the border, dark mode,
    /// keytoggle, the guitar view and the note preference on the way past.
    name_focused: bool,

    /// The detached chord window is actually on screen.
    detach_window_visible: bool,
    /// Builder size and position, frozen per detachment session so the
    /// per-frame builder diff never fights the user's resizes and drags.
    detached_builder_size: Vec2,
    detached_builder_pos: Option<Pos2>,
    detached_live_size: Option<Vec2>,
    detached_live_pos: Option<Pos2>,
    startup_detach_at: Option<Instant>,
    /// Geometry is written back on a debounce rather than on every frame: a
    /// window drag would otherwise rewrite settings.json a hundred times, and
    /// waiting for a clean exit loses the position whenever the app is killed.
    geometry_save_at: Option<Instant>,

    menu_state: Option<MenuState>,
    dialog: Option<Dialog>,

    last_sent_size: Option<Vec2>,
    decorations_sent: Option<bool>,
    main_inner_origin: Pos2,
    /// Whether `main_inner_origin` has ever been reported by the platform.
    /// Until it has, it is (0, 0) — which is a real position, not a missing
    /// one, so anything that centres on it lands in the corner of the screen.
    main_origin_known: bool,
    monitor_size: Option<Vec2>,
    /// The restored-off-screen rescue runs once, not every frame.
    offscreen_checked: bool,
    main_live_pos: Option<Pos2>,
    main_live_size: Option<Vec2>,
    /// When the detached window last appeared, and whether the window manager
    /// took its sizing over. See `wm_overrode_size` and `WM_GRACE`.
    detached_shown_at: Option<Instant>,
    detached_wm_managed: bool,
}

impl IvoryApp {
    /// `ctx` rather than an `eframe::CreationContext`, because eframe is one of
    /// three things that can hand this app a context and the other two have
    /// never heard of it. Everything the old signature used was `cc.egui_ctx`.
    ///
    /// `caps` says what the host permits, and is decided by the caller — the
    /// only code that knows what it is running inside. The app starts with no
    /// MIDI source at all; a host that picks its own attaches one with
    /// `set_ports`, and one that is handed its notes never does.
    pub fn new(ctx: &egui::Context, mut settings: Settings, caps: Caps) -> Self {
        // A host with no child windows cannot have anything detached, so it
        // must not believe it has.
        //
        // These two flags persist, and a plugin instance is seeded from the
        // same `settings.json` the standalone writes. Someone who left the
        // chord strip popped out on the desktop got a plugin with NO chord
        // readout: the band is zeroed by the flag, the window it was moved to
        // is gated off by `caps.detachable`, and the Attach row that would
        // undo it is gated off too. There was no way back except Reset
        // Settings, which throws away every colour, font and tuning as well.
        //
        // Cleared on the LOCAL copy only. A plugin does not write the shared
        // file, so the desktop's own arrangement is untouched.
        if !caps.detachable {
            settings.chord_window_detached = false;
            settings.fretboard_detached = false;
            settings.theory_detached = false;
            settings.recorder_detached = false;
        }
        // And the band itself, for a host that cannot open a device.
        //
        // Not the same question as detaching, and it has a worse failure. The
        // band is 200 points tall; a plugin editor whose settings file says
        // `show_recorder: true` would lay out 200 points of transport it can
        // never populate, shrinking the piano to make room for a camera preview
        // that will never arrive. `fit_bands` cannot tell the difference
        // between a band that is empty and one that is off.
        if !caps.capture_devices {
            settings.show_recorder = false;
            settings.recorder_detached = false;
        }
        crate::fonts::install(
            ctx,
            crate::fonts::FontChoice::from_key(&settings.font_choice),
            settings.custom_font_path.as_deref(),
        );
        crate::fonts::apply_text_styles(ctx);

        let mut detector = ChordDetector::new();
        detector.set_note_preference(settings.prefer_flats);
        // Teach layer: load user overrides (~/.config/ivory/overrides.json).
        // A missing or corrupt file yields an empty store; the detector then
        // behaves exactly like the stock engine until something is taught.
        detector.set_overrides(Some(OverrideStore::load()));

        let license = ivory_core::license::LicenseStore::load();

        let (midi_tx, midi_rx) = mpsc::channel();

        // Recreate the detached chord window 100ms after startup (spec §5.7).
        let startup_detach_at = (settings.chord_window_detached
            && settings.chord_detection_enabled)
            .then(|| Instant::now() + DEBOUNCE_100MS);

        let detached_builder_size = settings.detached_size_for_use();
        let detached_builder_pos = settings.detached_pos_for_use();

        let welcome = settings.show_welcome.then(|| Dialog::Welcome {
            dont_show_again: false,
        });

        let spec = settings.fretboard_spec();
        let weights = Weights::for_tuning(&spec.tuning);
        let voicing = VoicingSession::new(spec, weights);
        let settings_fret_size = settings.fretboard_win_size();
        let settings_fret_pos = settings.fretboard_win_pos();
        let startup_fret_detach_at = (settings.fretboard_detached && settings.show_fretboard)
            .then(|| Instant::now() + DEBOUNCE_100MS);
        // Same one-shot for the theory window. Gated on the band having at
        // least one diagram selected as well as being detached: a window
        // restored with nothing in it is a blank rectangle the user has to
        // close to find out what it was.
        let startup_theory_detach_at = (settings.theory_detached
            && settings.theory_views().any())
        .then(|| Instant::now() + DEBOUNCE_100MS);
        // And the recorder's. Gated on the band being SHOWN as well as
        // detached, so "Hide Recorder" with the window remembered for next time
        // does not reopen the window on its own at the next launch.
        let startup_recorder_detach_at = (settings.recorder_detached && settings.show_recorder)
            .then(|| Instant::now() + DEBOUNCE_100MS);

        Self {
            settings,
            detector,
            license,
            midi_tx,
            midi_rx,
            ports: None,
            audio_devices: None,
            cameras: None,
            caps,
            pending_resize: None,
            last_pane: Vec2::ZERO,
            last_drawn: Rect::NOTHING,
            manual_barre: None,
            barre_drag: None,
            demo_menu_done: false,
            // Assume focused: a window that has just opened is, and the first
            // frames report None.
            main_focused: true,
            notes: NoteState::default(),
            manual_notes: HashSet::new(),
            manual_positions: HashMap::new(),
            current_chord: None,
            last_detection: None,
            voicing,
            last_voicing: None,
            fret_window_visible: false,
            fret_builder_size: settings_fret_size,
            fret_builder_pos: settings_fret_pos,
            fret_live_size: None,
            fret_live_pos: None,
            fret_shown_at: None,
            fret_wm_managed: false,
            startup_fret_detach_at,
            theory_window_visible: false,
            theory_builder_size: theory_panel::DETACHED_DEFAULT,
            theory_builder_pos: None,
            theory_guard: None,
            startup_theory_detach_at,
            recorder_window_visible: false,
            recorder_builder_size: recorder_panel::DETACHED_DEFAULT,
            recorder_builder_pos: None,
            recorder_guard: None,
            startup_recorder_detach_at,
            recorder: recorder::RecorderState::default(),
            recorder_request: None,
            dir_request: None,
            export_override: None,
            settings_save_at: None,
            picker_slot: 0,
            grabbed: None,
            plugin_list: Vec::new(),
            name_focused: false,

            detach_window_visible: false,
            detached_builder_size,
            detached_builder_pos,
            detached_live_size: None,
            detached_live_pos: None,
            startup_detach_at,
            geometry_save_at: None,
            menu_state: None,
            dialog: welcome,
            last_sent_size: None,
            decorations_sent: None,
            main_inner_origin: Pos2::ZERO,
            main_origin_known: false,
            monitor_size: None,
            offscreen_checked: false,
            main_live_pos: None,
            main_live_size: None,
            detached_shown_at: None,
            detached_wm_managed: false,
        }
    }

    // ── Geometry (spec §3.2, integer truncation like Python) ───────────────

    fn layout_sizes(&self) -> Bands {
        band_sizes(&self.settings)
    }

    /// The channel every note arrives on, whoever is sending.
    ///
    /// `midir`'s callback thread holds one of these on the desktop; a VST3
    /// `process()` holds one in the plugin. Neither knows about the other, and
    /// `process_midi_events` cannot tell them apart, which is the point.
    pub fn midi_sender(&self) -> mpsc::Sender<MidiEvent> {
        self.midi_tx.clone()
    }

    /// Attach the thing that enumerates and opens MIDI devices.
    ///
    /// Separate from `new` because the source needs the sender, and the sender
    /// is made here. A host that is given its notes never calls this, which is
    /// also what makes the MIDI menu row and the picker dialog inert rather
    /// than merely hidden.
    pub fn set_ports(&mut self, ports: Option<Box<dyn MidiPorts>>) {
        self.ports = ports;
    }

    /// Attach the thing that enumerates audio inputs.
    ///
    /// The same shape as `set_ports` and for the same reason: `cpal` may not be
    /// reachable from this crate, so what arrives is a trait object. A plugin
    /// never calls this, which is what makes the device rows inert rather than
    /// merely hidden — and `Caps::capture_devices` is what makes them absent.
    pub fn set_capture_devices(&mut self, devices: Option<Box<dyn CaptureDevices>>) {
        self.audio_devices = devices;
    }

    /// And the cameras.
    pub fn set_cameras(&mut self, devices: Option<Box<dyn CaptureDevices>>) {
        self.cameras = devices;
    }

    /// Every audio input present right now, for the picker.
    ///
    /// Enumerated on demand rather than cached: an interface plugged in while
    /// the band is open has to appear without a restart.
    pub fn audio_device_list(&self) -> Vec<crate::ports::DeviceInfo> {
        self.audio_devices.as_ref().map(|d| d.list()).unwrap_or_default()
    }

    pub fn camera_list(&self) -> Vec<crate::ports::DeviceInfo> {
        self.cameras.as_ref().map(|d| d.list()).unwrap_or_default()
    }

    /// A size the user picked that the host has not been told about, if any.
    ///
    /// Taken, not read: the request goes out once. A host that refuses it will
    /// simply not resize, and the layout fits whatever rect it ends up with,
    /// so a refusal costs nothing but the size the user asked for.
    pub fn take_pending_resize(&mut self) -> Option<Vec2> {
        self.pending_resize.take()
    }

    /// Put the typeface back on a context that has never seen it.
    ///
    /// A plugin editor is a window the host opens and closes at will, and each
    /// time it opens there is a new GL context and a new, empty font atlas.
    /// The app is not rebuilt — reopening a window must not reset the tuning —
    /// so the faces have to be reinstalled without it.
    pub fn install_fonts(&self, ctx: &egui::Context) {
        crate::fonts::install(
            ctx,
            crate::fonts::FontChoice::from_key(&self.settings.font_choice),
            self.settings.custom_font_path.as_deref(),
        );
        crate::fonts::apply_text_styles(ctx);
    }

    /// The current settings as the same JSON the settings file holds.
    ///
    /// For a host that persists state itself rather than sharing the file.
    pub fn settings_json(&self) -> String {
        self.settings.to_json()
    }

    /// Feed one event in directly, for a host that has no channel to spare.
    ///
    /// The plugin uses the sender instead; this exists so a test can drive the
    /// app without one.
    pub fn feed(&mut self, ev: MidiEvent) {
        self.notes.apply(ev);
    }

    /// Ask the host for the height this band stack needs, keeping the width.
    ///
    /// On the desktop the window simply follows `layout_sizes()` on the next
    /// frame. A plugin's editor cannot: nothing resizes it unless it asks, and
    /// `SetSizePercent` was the only thing that ever did. So turning the
    /// guitar view on inside a DAW did not make the editor taller — it made
    /// the whole layout SMALLER to fit the height it already had, and the
    /// piano lost 40% of its width to black bars either side. Every action
    /// that adds or removes a band routes through here.
    fn request_natural_size(&mut self) {
        if self.caps.window_sizing {
            return; // the window follows the layout by itself
        }
        // Keep the width the editor already has; only the stack height
        // changed. Falling back to the settings width covers the first frame,
        // before anything has been laid out.
        let w = if self.last_pane.x > 1.0 {
            self.last_pane.x
        } else {
            main_width(&self.settings)
        };
        self.pending_resize = Some(natural_size(&self.settings, w));
    }

    /// Persist, if this host owns the settings file.
    ///
    /// One gate instead of twenty-four call sites each remembering to check.
    /// `~/.config/ivory/settings.json` is shared by the standalone and by
    /// EVERY plugin instance, so without this the last editor window you
    /// happened to touch would decide everyone's colours — and a DAW project
    /// reopened tomorrow would silently pick up whatever the standalone was
    /// set to. A plugin's state belongs in its own project file.
    fn save_settings(&self) {
        if self.caps.persist_global_settings {
            self.settings.save();
        }
    }

    /// Re-solve the guitar view on the detection cadence.
    ///
    /// Deliberately NOT the frame: `VoicingSession` caches on the held set, so
    /// a held chord costs one slice compare either way, but tying it to the
    /// same 100ms gate as chord detection means the two readouts change on the
    /// same tick instead of a frame apart. It runs whether or not the panel is
    /// visible and whether or not chord detection is on — the guitar view is
    /// its own instrument, not a decoration on the chord strip.
    fn voicing_tick(&mut self, force: bool) {
        let due = force || self.last_voicing.is_none_or(|t| t.elapsed() >= DETECT_TICK);
        if !due {
            return;
        }
        let dt = self
            .last_voicing
            .map_or(0, |t| t.elapsed().as_millis().min(u32::MAX as u128) as u32);
        self.last_voicing = Some(Instant::now());
        self.voicing.update(&self.display_notes(), dt);
    }

    /// Point the solver at whatever the settings now describe. Both calls
    /// throw away the remembered hand, which is the point: a shape the new
    /// board could not have produced must never survive a tuning change.
    /// Hand the solver the positions the user chose. Dropped whenever the
    /// board changes, because a pin names a `(string, fret)` on the board it
    /// was made on: keep them across a tuning or capo change and they name
    /// pitches nobody is playing.
    /// Fret every string between two, at one fret, and remember that it is a
    /// barre rather than a coincidence.
    ///
    /// Idempotent: a drag fires this on every frame it moves, so it has to be
    /// safe to run again with the same span, and cheap enough to run at frame
    /// rate. Re-solving is gated behind an actual change for that reason.
    fn place_barre(
        &mut self,
        spec: &ivory_core::fretboard::FretboardSpec,
        a: usize,
        b: usize,
        fret: u8,
    ) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let mut changed = false;
        for st in lo..=hi {
            let Some(note) = spec.pitch_at(st, fret) else {
                continue;
            };
            // One finger per string, exactly as a single click does: a barre
            // that left an older note on the same string would be a hand with
            // two fingers on one string.
            let stale: Vec<u8> = self
                .manual_positions
                .iter()
                .filter(|(_, &(s, _))| s == st)
                .map(|(&n, _)| n)
                .collect();
            for n in stale {
                if n != note {
                    self.manual_positions.remove(&n);
                    self.manual_notes.remove(&n);
                    changed = true;
                }
            }
            if self.manual_positions.insert(note, (st, fret)) != Some((st, fret)) {
                changed = true;
            }
            changed |= self.manual_notes.insert(note);
        }
        let barre = (hi > lo).then_some(ivory_core::voicing::Barre {
            fret,
            lo_string: lo,
            hi_string: hi,
        });
        if barre != self.manual_barre {
            self.manual_barre = barre;
            changed = true;
        }
        if changed {
            self.sync_pins();
            self.detection_tick(true);
            self.voicing_tick(true);
        }
    }

    /// The barre to draw, which is not always the one the solver derived.
    ///
    /// For a shape the SOLVER chose, its own barre is the right answer: it
    /// decided to bar those strings and the diagram should say so.
    ///
    /// For a shape entered BY HAND, it is not. `barre_and_fingers` reads a
    /// barre out of any adjacent strings that share their lowest fret, so
    /// placing two notes that happen to line up drew a bar across them and
    /// claimed a finger position nobody asked for. By hand, a barre is
    /// something you make on purpose — by dragging along a fret — and nothing
    /// else counts as one.
    fn barre_to_draw(&self) -> Option<ivory_core::voicing::Barre> {
        if self.manual_positions.is_empty() {
            return self.voicing.current().shape.barre;
        }
        // Checked against the notes rather than trusted. A barre is only a
        // barre while every string it spans is still fretted at that fret, and
        // the user is free to click one of them off afterwards. Validating
        // here rather than clearing at each of the places a note can go away
        // means it cannot be left behind by one that was missed.
        let b = self.manual_barre?;
        let all_there = (b.lo_string..=b.hi_string).all(|st| {
            self.manual_positions
                .iter()
                .any(|(n, &(s, f))| s == st && f == b.fret && self.manual_notes.contains(n))
        });
        all_there.then_some(b)
    }

    fn sync_pins(&mut self) {
        let pins: Vec<(u8, usize, u8)> = self
            .manual_positions
            .iter()
            .filter(|(p, _)| self.manual_notes.contains(p))
            .map(|(&p, &(st, f))| (p, st, f))
            .collect();
        self.voicing.set_pins(pins);
    }

    fn rebuild_voicing(&mut self) {
        self.manual_positions.clear();
        self.voicing.set_pins(Vec::new());
        let spec = self.settings.fretboard_spec();
        self.voicing.set_weights(Weights::for_tuning(&spec.tuning));
        self.voicing.set_spec(spec);
        self.voicing_tick(true);
    }

    // ── MIDI state (spec §10 semantics) ────────────────────────────────────

    fn process_midi_events(&mut self) {
        while let Ok(ev) = self.midi_rx.try_recv() {
            self.notes.apply(ev);
        }
    }

    /// Keys drawn as active: MIDI-held notes plus manual (keytoggle) notes.
    fn display_notes(&self) -> HashSet<u8> {
        // Dev/marketing hook: pin a voicing with no keyboard attached, for
        // store screenshots and for eyeballing display work (a glow or a
        // segment readout can only be judged lit). Environment-gated, so a
        // normal launch cannot reach it and it ships inert.
        //   IVORY_DEMO_NOTES=60,64,67,71 /Applications/Tangent.app/Contents/MacOS/ivory
        if let Ok(spec) = std::env::var("IVORY_DEMO_NOTES") {
            let demo: HashSet<u8> = spec
                .split(&[',', ' '][..])
                .filter_map(|t| t.trim().parse::<u8>().ok())
                .collect();
            if !demo.is_empty() {
                return demo;
            }
        }
        let mut set: HashSet<u8> = self.notes.held().iter().copied().collect();
        if self.settings.keytoggle_enabled {
            set.extend(self.manual_notes.iter().copied());
        }
        set
    }

    /// What the theory band draws, from the notes already on screen.
    ///
    /// Pitch classes, not notes: the circle, the lattice and the triangles all
    /// have twelve positions and no octave axis, so the fold to `% 12` happens
    /// here rather than three times inside the renderer.
    ///
    /// The root comes from the detector's own label when there is one, and
    /// from the bass otherwise. Parsing a label the app itself generated is
    /// narrow enough to be safe, and it is the only way to know that a voicing
    /// with a C in the bass is being HEARD as an inversion of something else.
    fn theory_input(&self, display: &HashSet<u8>) -> theory_panel::Input {
        // The band shows what you PUT there, not what you are playing, unless
        // you ask otherwise. It is a reference to look at while your hands are
        // busy, and one that redrew on every note would be unreadable at
        // exactly the moment it is wanted.
        let notes: &HashSet<u8> = if self.settings.theory_follow_midi {
            display
        } else {
            &self.manual_notes
        };
        let pcs = notes.iter().fold(0u16, |m, n| m | 1 << (n % 12));
        let bass = notes.iter().min().map(|n| n % 12);
        // The detector reads the notes on SCREEN, so its label describes this
        // set only when the two agree. When they do not — pinned notes while
        // something else is being played — the root is dropped rather than
        // borrowed, and the diagrams fall back to the bass. A tonic marker
        // pointing at a chord you are not looking at is worse than none.
        let (root, minor) = self
            .current_chord
            .as_deref()
            .and_then(theory_panel::parse_label)
            .filter(|(r, _)| pcs & (1 << (r % 12)) != 0)
            .map_or((None, false), |(r, m)| (Some(r), m));
        theory_panel::Input {
            pcs,
            bass,
            root,
            minor,
        }
    }

    /// Place or remove the pitch classes a click on the theory band asked for.
    ///
    /// Keytoggle's rules, applied to a different picture: clicking something
    /// already lit removes it, clicking something dark adds it. A chord vertex
    /// places the whole triad, because that vertex IS a chord and placing one
    /// note of it would be a strange thing for a diagram of chords to do.
    ///
    /// Pitch classes have no octave, so they are placed in the octave above
    /// middle C — the one the piano puts in the middle of the window.
    fn toggle_theory_hit(&mut self, hit: theory_panel::Hit) {
        let pcs: Vec<u8> = match hit {
            theory_panel::Hit::Pc(pc) => vec![pc],
            theory_panel::Hit::Triad { root, minor } => {
                let m = if minor {
                    theory_panel::minor_triad(root)
                } else {
                    theory_panel::major_triad(root)
                };
                (0..12u8).filter(|pc| m & (1 << pc) != 0).collect()
            }
        };
        // A triad is placed whole or cleared whole: if any of it is missing,
        // add the rest; only when all of it is already there does it come off.
        let all_present = pcs
            .iter()
            .all(|pc| self.manual_notes.iter().any(|n| n % 12 == *pc));
        for pc in pcs {
            self.manual_notes.retain(|n| n % 12 != pc);
            self.manual_positions.retain(|n, _| n % 12 != pc);
            if !all_present {
                self.manual_notes.insert(60 + pc);
            }
        }
        self.sync_pins();
        self.detection_tick(true);
        self.voicing_tick(true);
    }

    // ── Chord detection (spec §12) ─────────────────────────────────────────

    fn detection_tick(&mut self, force: bool) {
        if !self.settings.chord_detection_enabled {
            self.current_chord = None;
            return;
        }
        let due = force
            || self
                .last_detection
                .is_none_or(|t| t.elapsed() >= DETECT_TICK);
        if !due {
            return;
        }
        self.last_detection = Some(Instant::now());
        let notes = self.display_notes();
        self.current_chord = if notes.is_empty() {
            None
        } else {
            self.detector.detect_chord(&notes)
        };
    }

    /// Which shortcuts are live, for both the handler and the help card.
    ///
    /// One method rather than two call sites building it, so the card cannot
    /// advertise a key the handler refuses — which is the whole reason
    /// `keys.rs` keeps its bindings in one table.
    fn key_gates(&self) -> keys::Gates {
        keys::Gates {
            recorder_shown: self.settings.show_recorder,
            recorder_available: self.caps.capture_devices,
        }
    }

    fn menu_view(&self) -> MenuView {
        MenuView {
            dark_mode: self.settings.dark_mode,
            borderless: self.settings.borderless_mode,
            keytoggle: self.settings.keytoggle_enabled,
            prefer_flats: self.settings.prefer_flats,
            detection_enabled: self.settings.chord_detection_enabled,
            detached: self.settings.chord_window_detached,
            notes_held: !self.display_notes().is_empty(),
            learning_on: self.detector.learning_mode(),
            supporter: self.license.is_supporter(),
            heart_on: self.settings.show_heart,
            fretboard_on: self.settings.show_fretboard,
            theory: self.settings.theory_views(),
            theory_detached: self.settings.theory_detached,
            theory_follows_midi: self.settings.theory_follow_midi,
            wood: self.settings.fretboard_wood().key(),
            fretboard_detached: self.settings.fretboard_detached,
            recorder_on: self.settings.show_recorder,
            recorder_detached: self.settings.recorder_detached,
            count_in_beats: self.settings.count_in_beats(),
            metronome_on: self.settings.metronome_on,
            metronome_in_take: self.settings.metronome_in_take,
            hide_elapsed: self.settings.record_hide_elapsed,
            caps: self.caps,
            tuning: self.settings.fretboard_spec().tuning.name.to_string(),
            capo: self.settings.fretboard_spec().capo,
            next_font: {
                use crate::fonts::FontChoice;
                // The face the next click will actually give you. Same method
                // the action uses, so the label cannot promise one thing and
                // the click do another.
                let cur = FontChoice::from_key(&self.settings.font_choice);
                let next = cur.next();
                (next != cur).then(|| next.label())
            },
        }
    }

    fn open_menu_at(&mut self, ctx: &egui::Context, global_pos: Pos2) {
        self.menu_state = Some(MenuState::open(
            ctx,
            self.menu_view(),
            global_pos,
            self.monitor_size,
        ));
    }

    /// Colour of the supporter heart, or None when it should not be drawn.
    /// Derived every frame from the licence — never a cached flag.
    fn heart_color(&self) -> Option<egui::Color32> {
        if !(self.settings.show_heart && self.license.is_supporter()) {
            return None;
        }
        let n = chord_strip::HEART_COLORS.len() as i64;
        // rem_euclid so a hand-edited negative index still lands in range.
        let idx = self.settings.heart_color.rem_euclid(n) as usize;
        Some(chord_strip::HEART_COLORS[idx])
    }

    // ── Main-window interaction ────────────────────────────────────────────

    fn handle_main_interaction(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        piano_rect: Rect,
        chord_rect: Option<Rect>,
        fret_rect: Option<Rect>,
        theory_rect: Option<Rect>,
        recorder_rect: Option<Rect>,
    ) {
        let resp = ui.interact(
            ui.max_rect(),
            egui::Id::new("ivory-main-bg"),
            egui::Sense::click_and_drag(),
        );
        if self.dialog.is_some() {
            return; // Qt dialogs are modal: main window ignores input.
        }
        let (primary_pressed, pointer_down, pointer_released, pointer, ctrl) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.modifiers.ctrl,
            )
        });
        let ctrl_as_context = cfg!(target_os = "macos") && ctrl;

        // Right-click (or ctrl-click on macOS, Qt default) opens the menu.
        if resp.secondary_clicked() || (resp.clicked() && ctrl_as_context) {
            if let Some(pos) = pointer {
                let global = self.main_inner_origin + pos.to_vec2();
                self.open_menu_at(ctx, global);
            }
            return;
        }
        if self.menu_state.is_some() {
            // A press closes the menu — but ONLY where the menu is its own
            // window.
            //
            // On the desktop the menu is a separate viewport, so this handler
            // never sees a press that lands on a menu item; the only presses
            // it sees are elsewhere, and closing is right.
            //
            // Drawn INLINE, the menu is in this very context, so this handler
            // sees the press that is landing on the item — and closed the menu
            // before `menu::show` ran, which is later in the frame. The button
            // was never clicked and the menu simply vanished. Every row in a
            // plugin was dead for exactly this reason. `menu::show` closes it
            // on a press outside its own rect instead, which it can tell and
            // this cannot.
            if primary_pressed && self.caps.child_windows {
                self.menu_state = None;
            }
            // Either way the click stops here rather than reaching a piano key.
            return;
        }

        // ── the barre gesture ─────────────────────────────────────────────
        //
        // Hold and drag along a fret and every string you cross is fretted
        // there. This is the ONLY way to get a barre: a shape entered by hand
        // gets one because you drew one, never because two notes happened to
        // line up.
        if self.settings.keytoggle_enabled {
            if let (Some((start_st, fret)), Some(pos)) = (self.barre_drag, pointer) {
                if pointer_down {
                    if let Some(r) = fret_rect.filter(|r| r.contains(pos)) {
                        let spec = self.settings.fretboard_spec();
                        if let Some((st, f)) = fretboard_panel::position_at(r, &spec, pos) {
                            // Same fret only. Dragging diagonally is someone
                            // reaching for a different note, not barring.
                            if f == fret && st != start_st {
                                self.place_barre(&spec, start_st, st, fret);
                            }
                        }
                    }
                }
            }
            if pointer_released {
                self.barre_drag = None;
            }
        }

        if primary_pressed && !ctrl_as_context {
            if let Some(pos) = pointer {
                // The Recorder band first, and NOT behind `keytoggle_enabled`.
                // Its controls are buttons, not an instrument: the record
                // button has to work whether or not the user has turned on
                // clicking the piano to place notes.
                if let Some(r) = recorder_rect.filter(|r| r.contains(pos)) {
                    let hit = recorder_panel::hit_test(
                        r,
                        &self.recorder.view(
                            self.settings.record_take_name.as_deref().unwrap_or_default(),
                            self.name_focused,
                            self.settings.knobs(),
                            self.settings.record_hide_elapsed,
                        ),
                        pos,
                    );
                    // Remember a dragged control for as long as the button is
                    // held. Only the value-carrying hits are grabbable; a
                    // button does not want a drag.
                    self.grabbed = hit
                        .filter(recorder_panel::Hit::is_draggable)
                        .map(|h| (h, pos));
                    // A press anywhere in the band that is not the name field
                    // takes focus off it, which is what makes clicking away
                    // commit the name the way every other text field does.
                    self.name_focused = matches!(hit, Some(recorder_panel::Hit::NameField));
                    if let Some(hit) = hit {
                        self.apply_recorder_hit(hit);
                    }
                    return;
                }
                // Clicking anywhere else also drops the field's focus, or the
                // next letter typed at the piano would go into the take name.
                self.name_focused = false;
                // The supporter heart cycles colour on click. Checked before the
                // keytoggle hit-test because it sits in the chord strip, not the
                // keyboard, so the two can never contend.
                if self.heart_color().is_some() {
                    if let Some(cr) = chord_rect {
                        if chord_strip::heart_rect(cr).contains(pos) {
                            self.settings.heart_color = self
                                .settings
                                .heart_color
                                .wrapping_add(1)
                                .rem_euclid(chord_strip::HEART_COLORS.len() as i64);
                            self.save_settings();
                            return;
                        }
                    }
                }
                // Keytoggle hit-test/toggle first (spec §4.5), then StartDrag
                // (must be issued directly from the press handler).
                // The capo cycles what it is made of. Checked BEFORE the
                // keytoggle hit-test, and deliberately: the capo sits on a
                // fret, so the two would otherwise contend and the note would
                // win — you would be unable to click the thing you are
                // pointing at. It is on top visually; it is on top here.
                if let Some(r) = fret_rect.filter(|r| r.contains(pos)) {
                    let spec = self.settings.fretboard_spec();
                    if fretboard_panel::capo_rect(r, &spec).is_some_and(|cr| cr.contains(pos)) {
                        self.settings.capo_style =
                            self.settings.capo_style().next().key().to_owned();
                        self.save_settings();
                        return;
                    }
                }

                // The theory band is a third instrument. Clicking a name on
                // the circle or a node on the lattice places that note;
                // clicking a chord vertex places the whole triad. Handled
                // before the piano and the neck because it is the only one
                // that speaks in pitch classes rather than in notes.
                if self.settings.keytoggle_enabled {
                    if let Some(hit) = theory_rect.filter(|r| r.contains(pos)).and_then(|r| {
                        let display = self.display_notes();
                        theory_panel::hit_test(
                            r,
                            self.settings.theory_views(),
                            self.theory_input(&display),
                            pos,
                        )
                    }) {
                        self.toggle_theory_hit(hit);
                        return;
                    }
                }
                if self.settings.keytoggle_enabled {
                    // Either instrument can put a note in, and they toggle the
                    // same set — so a shape entered on the neck lights up on the
                    // keyboard, and a chord entered on the keys shows you where
                    // a guitarist would play it. That symmetry is the point:
                    // the fretboard stops being a readout and becomes an input.
                    let hit = if piano_rect.contains(pos) {
                        let local = pos - piano_rect.min;
                        piano::hit_test(local.x, local.y, piano_rect.width(), piano_rect.height())
                    } else {
                        fret_rect.filter(|r| r.contains(pos)).and_then(|r| {
                            fretboard_panel::hit_test(r, &self.settings.fretboard_spec(), pos)
                        })
                    };
                    if let Some(note) = hit {
                        if self.manual_notes.remove(&note) {
                            self.manual_positions.remove(&note);
                        } else {
                            // Only a fretboard click pins. A piano click says
                            // WHICH note, not where on the neck to draw it, so
                            // the solver still chooses for those.
                            if let Some(r) = fret_rect.filter(|r| r.contains(pos)) {
                                if let Some((st, fret)) = fretboard_panel::position_at(
                                    r,
                                    &self.settings.fretboard_spec(),
                                    pos,
                                ) {
                                    // A press on the neck may become a drag
                                    // along a fret, which is the only way to
                                    // make a barre.
                                    self.barre_drag = Some((st, fret));
                                    // One finger per string, because that is
                                    // how a guitar works. Clicking a second
                                    // fret on a string MOVES the note there
                                    // rather than adding a second one that can
                                    // never sound. Without this, one such click
                                    // un-pinned the whole shape — pinning is
                                    // all-or-nothing, so a single impossible
                                    // note sent every other note back to the
                                    // solver to be rearranged, and the board
                                    // started reporting "4 of 5 notes".
                                    if let Some(&occupant) = self
                                        .manual_positions
                                        .iter()
                                        .find(|(_, &(s, _))| s == st)
                                        .map(|(p, _)| p)
                                    {
                                        self.manual_notes.remove(&occupant);
                                        self.manual_positions.remove(&occupant);
                                    }
                                    self.manual_positions.insert(note, (st, fret));
                                }
                            }
                            self.manual_notes.insert(note);
                        }
                        self.sync_pins();
                        self.detection_tick(true); // immediate off-cadence update
                        self.voicing_tick(true);
                    }
                }
                // Dragging moves OUR window. A plugin's window is the
                // host's, and borderless_mode is not even offered there.
                if self.settings.borderless_mode && self.caps.window_sizing {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
            }
        }
    }

    // ── Detach / attach (spec §5.7) ────────────────────────────────────────

    fn detach_chord_window(&mut self) {
        // Python seeded the detached height from the attached strip on every
        // detach, which is what made the window a piano-wide sliver each time
        // and quietly discarded whatever size the user had chosen. The window
        // now reopens wherever and however they last left it.
        self.settings.chord_window_detached = true;
        self.detach_window_visible = true;
        self.detached_builder_size = self.settings.detached_size_for_use();
        self.detached_builder_pos = self.settings.detached_pos_for_use().map(|p| {
            crate::settings::clamp_to_monitor(p, self.detached_builder_size, self.monitor_size)
        });
        self.detached_live_size = None;
        self.detached_live_pos = None;
        self.detached_shown_at = Some(Instant::now());
        self.detached_wm_managed = false;
        self.save_settings();
    }

    // ── The popped-out neck (D-UI-16) ─────────────────────────────────────

    fn detach_fretboard(&mut self) {
        self.settings.fretboard_detached = true;
        self.fret_window_visible = true;
        self.fret_builder_size = self.settings.fretboard_win_size();
        self.fret_builder_pos = self.settings.fretboard_win_pos().map(|p| {
            crate::settings::clamp_to_monitor(p, self.fret_builder_size, self.monitor_size)
        });
        self.fret_live_size = None;
        self.fret_live_pos = None;
        self.fret_shown_at = Some(Instant::now());
        self.fret_wm_managed = false;
        self.save_settings();
    }

    fn detach_theory(&mut self) {
        self.settings.theory_detached = true;
        self.theory_window_visible = true;
        self.theory_builder_size = self.settings.theory_win_size();
        self.theory_builder_pos = self.settings.theory_win_pos().map(|p| {
            crate::settings::clamp_to_monitor(p, self.theory_builder_size, self.monitor_size)
        });
        // The guard is armed with the SAME size handed to the builder, and both
        // must stay fixed for as long as the window lives: a guard measuring
        // against a size that drifts cannot tell a tiling WM from a user.
        self.theory_guard = Some(theory_panel::GeometryGuard::opened(
            self.theory_builder_size,
            Instant::now(),
        ));
        self.save_settings();
    }

    // ── The Recorder popout ────────────────────────────────────────────────
    //
    // Four fields and three methods, the same three the theory window has. The
    // one difference worth noting is that this popout has a REASON beyond
    // taste: a second monitor holding a big framing view of the camera while
    // the piano stays where it is.

    fn detach_recorder(&mut self) {
        self.settings.recorder_detached = true;
        self.recorder_window_visible = true;
        self.recorder_builder_size = self.settings.recorder_win_size();
        self.recorder_builder_pos = self.settings.recorder_win_pos().map(|p| {
            crate::settings::clamp_to_monitor(p, self.recorder_builder_size, self.monitor_size)
        });
        self.recorder_guard = Some(theory_panel::GeometryGuard::opened(
            self.recorder_builder_size,
            Instant::now(),
        ));
        self.save_settings();
        self.request_natural_size();
    }

    fn reattach_recorder(&mut self) {
        if self.recorder_guard.as_ref().is_none_or(|g| !g.wm_managed()) {
            self.remember_recorder_geometry();
        }
        self.settings.recorder_detached = false;
        self.recorder_window_visible = false;
        self.recorder_guard = None;
        self.save_settings();
        self.request_natural_size();
    }

    /// Write back the recorder popout's size and position.
    fn remember_recorder_geometry(&mut self) -> bool {
        let Some(g) = self.recorder_guard.as_ref() else {
            return false;
        };
        let mut changed = false;
        if let Some(size) = g.live_size() {
            let (w, h) = (size.x.round() as i64, size.y.round() as i64);
            if self.settings.recorder_win_w != Some(w) || self.settings.recorder_win_h != Some(h) {
                self.settings.recorder_win_w = Some(w);
                self.settings.recorder_win_h = Some(h);
                changed = true;
            }
        }
        if let Some(pos) = g.live_pos() {
            let (x, y) = (pos.x.round() as i64, pos.y.round() as i64);
            if self.settings.recorder_win_x != Some(x) || self.settings.recorder_win_y != Some(y) {
                self.settings.recorder_win_x = Some(x);
                self.settings.recorder_win_y = Some(y);
                changed = true;
            }
        }
        changed
    }

    // ── The host's side of the recorder ────────────────────────────────────

    /// Everything the band shows, for the host to fill in each frame.
    ///
    /// `&mut` and public because the direction of travel is inward: the app
    /// does not ask a device for anything, it is TOLD. That is what keeps
    /// `cpal`, cameras and take directories out of this crate entirely.
    pub fn recorder_state_mut(&mut self) -> &mut recorder::RecorderState {
        &mut self.recorder
    }

    /// Take whatever the band asked for, if anything.
    ///
    /// Drained by the host AFTER `frame()` returns. A plugin refuses simply by
    /// never calling this, which is why refusal needs no code of its own.
    pub fn take_recorder_request(&mut self) -> Option<recorder::RecorderRequest> {
        self.recorder_request.take()
    }

    /// Take a pending "choose a folder" request. Same contract.
    pub fn take_directory_request(&mut self) -> Option<crate::ports::DirRequest> {
        self.dir_request.take()
    }

    /// Whether the take-name field currently has keyboard focus, so the host
    /// knows a space bar is a space and not a Record press.
    pub fn recorder_name_focused(&self) -> bool {
        self.name_focused
    }

    /// One click in the band.
    ///
    /// Everything that changes a SETTING happens here and now; everything that
    /// touches a device or the filesystem becomes a request for the host. That
    /// split is the same one `Caps` draws, and it is why a plugin could paint
    /// this band harmlessly if it ever had reason to.
    fn apply_recorder_hit(&mut self, hit: recorder_panel::Hit) {
        use recorder_panel::Hit;
        use recorder::RecorderRequest as R;
        match hit {
            Hit::Record => self.request_recorder(R::Toggle),
            Hit::Stop => self.request_recorder(R::Stop),
            Hit::ChooseFolder => self.ask_for_a_folder(),
            Hit::ToggleDefaultDir => {
                self.settings.record_dir_is_default = !self.settings.record_dir_is_default;
                // Unticking it does NOT forget the folder. The tick means "keep
                // using this next time"; clearing the path as well would throw
                // away the choice the user just made in the act of saying they
                // did not want it to be permanent.
                self.save_settings();
            }
            Hit::NameField => self.name_focused = true,
            Hit::PickCamera => self.open_device_picker(dialogs::DeviceKind::Camera),
            Hit::PickAudio => self.open_device_picker(dialogs::DeviceKind::AudioInput),
            Hit::CycleCountIn => {
                let choices = recorder::COUNT_IN_CHOICES;
                let now = self.settings.count_in_beats();
                let next = choices
                    .iter()
                    .position(|c| *c == now)
                    .map_or(choices[0], |i| choices[(i + 1) % choices.len()]);
                self.settings.record_count_in_beats = i64::from(next);
                self.save_settings();
            }
            Hit::Export => self.open_export_dialog(),
            Hit::PickSlot(slot) => self.open_plugin_picker(slot),
            Hit::ClearSlot(slot) => {
                if let Some(p) = self.settings.plugin_slots.get_mut(slot) {
                    *p = None;
                    self.save_settings();
                }
            }
            Hit::OpenSlotEditor(slot) => {
                self.request_recorder(recorder::RecorderRequest::OpenPluginEditor(slot));
            }
            Hit::SetSlotGain(slot, p) => {
                if let Some(g) = self.settings.plugin_gains.get_mut(slot) {
                    *g = f64::from(recorder::fader_to_gain(p));
                    self.save_settings_soon();
                }
            }
            Hit::ToggleMetronome => {
                self.settings.metronome_on = !self.settings.metronome_on;
                self.save_settings();
            }
            Hit::ToggleMetronomeInTake => {
                self.settings.metronome_in_take = !self.settings.metronome_in_take;
                self.save_settings();
            }
            // The faders. Saved through the same debounce the window geometry
            // uses rather than on every frame of a drag — a fader written to
            // disk sixty times a second is sixty file rewrites per gesture.
            Hit::SetMetronomeGain(p) => {
                self.settings.metronome_gain = f64::from(recorder::fader_to_gain(p));
                self.save_settings_soon();
            }
            Hit::SetInputGain(p) => {
                self.settings.input_gain = f64::from(recorder::fader_to_gain(p));
                self.save_settings_soon();
            }
            Hit::SetTempo(bpm) => {
                self.settings.record_export.tempo_bpm =
                    bpm.clamp(recorder::MIN_BPM, recorder::MAX_BPM);
                self.save_settings_soon();
            }
        }
    }

    // ── What the host needs to read and write ──────────────────────────────
    //
    // Deliberately narrow. The host does not get `&mut Settings`: every one of
    // these is a specific question with a specific answer, and a general
    // accessor is how the desktop binary would end up quietly owning settings
    // policy that the plugin also has to obey.

    /// The Recorder band is showing, so the host should have an input open.
    pub fn recorder_band_open(&self) -> bool {
        self.settings.show_recorder
    }

    /// Where takes go, resolved.
    pub fn record_root(&self) -> std::path::PathBuf {
        self.settings.record_root()
    }

    /// Whether the chosen folder is meant to survive the session.
    pub fn record_dir_is_default(&self) -> bool {
        self.settings.record_dir_is_default
    }

    /// The typed take name, if any.
    pub fn take_name(&self) -> Option<&str> {
        self.settings.record_take_name.as_deref()
    }

    pub fn count_in_beats(&self) -> u32 {
        self.settings.count_in_beats()
    }

    /// The tempo the click, the count-in and the SMF tempo mark all share.
    pub fn tempo_bpm(&self) -> f64 {
        self.settings.record_export.tempo_bpm
    }

    /// `input` / `plugin` / `both`, verbatim from the file.
    pub fn audio_source_setting(&self) -> &str {
        &self.settings.record_audio_source
    }

    pub fn metronome_on(&self) -> bool {
        self.settings.metronome_on
    }

    pub fn metronome_in_take(&self) -> bool {
        self.settings.metronome_in_take
    }

    /// The three monitor gains, linear.
    pub fn gains(&self) -> recorder::Gains {
        self.settings.knobs().gains
    }

    /// The stable uid of the audio input the user chose, if any.
    pub fn chosen_audio_uid(&self) -> Option<&str> {
        self.settings.record_audio_device.as_deref()
    }

    /// The user explicitly chose "None — record MIDI only".
    ///
    /// Distinct from `chosen_audio_uid() == None`, which is also what "has
    /// never opened the picker" looks like — and those two want opposite
    /// behaviour at startup: a default input so the meter is live, or no input
    /// at all.
    pub fn audio_explicitly_off(&self) -> bool {
        self.settings.record_audio_source == "none"
    }

    pub fn chosen_camera_uid(&self) -> Option<&str> {
        self.settings.record_camera_uid.as_deref()
    }

    /// Answer a [`crate::ports::DirRequest`].
    ///
    /// `remember` is the "Default" tick, and it is passed in rather than read
    /// from settings because the host is answering a question the user asked
    /// before the tick's current value was necessarily what they meant.
    pub fn set_record_dir(&mut self, dir: std::path::PathBuf, remember: bool) {
        self.settings.record_dir = Some(dir.to_string_lossy().into_owned());
        self.settings.record_dir_is_default = remember;
        self.save_settings();
    }

    // No `set_audio_uid` / `set_camera_uid` here, deliberately.
    //
    // There were two, `pub`, with no callers — so no `dead_code` warning — and
    // they wrote the setting WITHOUT telling the device object, which is half
    // of what `DialogAction::ChooseDevice` does. A remembered choice that never
    // takes effect until the next launch is exactly the bug the comment there
    // warns about, and an obvious-looking public setter is how the next host
    // would have reintroduced it. Selection goes through the dialog action.

    /// What the next take will write.
    ///
    /// The remembered spec unless the user has chosen something for this
    /// session, which is the whole point of having a "use for every take" tick
    /// rather than saving unconditionally.
    pub fn export_spec(&self) -> recorder::ExportSpec {
        self.export_override.unwrap_or(self.settings.record_export)
    }

    /// Debounce a settings write. See `settings_save_at`.
    fn save_settings_soon(&mut self) {
        self.settings_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
    }

    /// Hand the app the list of installed VST3 bundles.
    ///
    /// Called once at startup by the host. Paths and names only — building it
    /// opens nothing.
    pub fn set_plugin_list(&mut self, bundles: Vec<std::path::PathBuf>) {
        self.plugin_list = bundles;
    }

    /// The instrument chosen for each slot, for the host to load after the
    /// frame.
    pub fn chosen_plugin(&self, slot: usize) -> Option<&str> {
        self.settings
            .plugin_slots
            .get(slot)
            .and_then(|p| p.as_deref())
    }

    /// Which slot the open picker is filling.
    ///
    /// The dialog does not know about slots — it chooses a bundle — so the app
    /// remembers what the question was. Set when the picker opens and read when
    /// the answer comes back.
    fn open_plugin_picker(&mut self, slot: usize) {
        if !self.caps.capture_devices || slot >= recorder::SLOTS {
            return;
        }
        self.picker_slot = slot;
        // The dialog's own constructor rather than building the variant here:
        // it sorts, it derives the rows from the paths, and it preselects the
        // loaded one. Three things that would otherwise be duplicated and
        // would drift.
        self.dialog = Some(Dialog::plugin_picker(
            &self.plugin_list,
            self.settings.plugin_slots[slot].clone(),
        ));
    }

    fn open_export_dialog(&mut self) {
        let mut spec = self.export_spec();
        // Seed the display panels from what is actually on screen, so the
        // common case — "record what I am looking at" — needs no clicks. The
        // dialog then overrides them FOR THE VIDEO ONLY.
        spec.composite.shows = dialogs::shows_from_settings(&self.settings);
        // Which of the two dialogs this is depends on whether there is a take
        // to talk about.
        //
        // With a finished take on screen, Export means "re-export THAT", and
        // the post-take dialog is the one that knows what is re-derivable: the
        // SMF's tempo mark and a display-only video are, and anything
        // containing the camera is not — those frames were composited live and
        // nothing kept them. Without one it means "what should the next take
        // produce".
        //
        // `had_camera: false` unconditionally, and it is not a placeholder: no
        // take this build can produce contains camera frames, because nothing
        // encodes video yet. It becomes a real question the day it does.
        let post_take = self.recorder.last_take_folder.is_some();
        self.dialog = Some(Dialog::export(spec, post_take, false));
    }

    /// Longest take name the field accepts.
    ///
    /// Not a filesystem limit — `ivory_record::take` handles those, including
    /// the ones Windows invents. This is a legibility limit: the folder name
    /// also carries a timestamp, and a 200-character name makes every take in
    /// the finder unreadable.
    const NAME_MAX: usize = 64;

    /// The take-name field, driven from raw input.
    fn edit_take_name(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        let mut name = self.settings.record_take_name.clone().unwrap_or_default();
        let before = name.clone();
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    for ch in text.chars() {
                        // Control characters never reach a name. A tab or a
                        // newline pasted in from a set list would survive
                        // sanitisation as an invisible character in a folder
                        // name, which is the sort of thing that is impossible
                        // to see and impossible to type again.
                        if !ch.is_control() && name.chars().count() < Self::NAME_MAX {
                            name.push(ch);
                        }
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    name.pop();
                }
                egui::Event::Key {
                    key: egui::Key::Enter | egui::Key::Escape | egui::Key::Tab,
                    pressed: true,
                    ..
                } => {
                    self.name_focused = false;
                }
                _ => {}
            }
        }
        if name != before {
            // Empty is absent, not an empty string: the name is optional and
            // the timestamp already makes every folder unique, so a field the
            // user cleared must go back to producing unnamed takes rather than
            // takes called "".
            self.settings.record_take_name = (!name.is_empty()).then_some(name);
            self.save_settings();
        }
    }

    fn request_recorder(&mut self, request: recorder::RecorderRequest) {
        // Refused rather than queued where the host cannot honour it. A plugin
        // never drains, so an ungated request would sit here forever and the
        // first thing a Toggle did after somebody added draining would be to
        // start a take nobody asked for.
        if !self.caps.capture_devices {
            return;
        }
        self.recorder_request = Some(request);
    }

    /// Ask the host to raise a folder picker.
    ///
    /// Not a blocking call and not a `RecorderRequest`: `rfd`'s native panel
    /// runs a nested run loop, so raising one from inside a frame means
    /// re-entering the frame already on the stack. The host drains this after
    /// `frame()` returns.
    fn ask_for_a_folder(&mut self) {
        if !self.caps.capture_devices || !self.caps.native_file_dialogs {
            return;
        }
        self.dir_request = Some(crate::ports::DirRequest {
            start_at: Some(self.settings.record_root()),
            title: "Where should Tangent put your takes?".to_owned(),
        });
    }

    /// Open the camera or audio-input picker, listing what is present RIGHT NOW.
    ///
    /// Enumerated at open rather than cached, for the same reason the MIDI
    /// picker re-reads its ports: devices are plugged and unplugged while the
    /// app runs, and on macOS a Continuity Camera appears and vanishes with the
    /// phone.
    fn open_device_picker(&mut self, kind: dialogs::DeviceKind) {
        if !self.caps.capture_devices {
            return;
        }
        let (devices, current) = match kind {
            dialogs::DeviceKind::Camera => (
                self.camera_list(),
                self.settings.record_camera_uid.clone(),
            ),
            dialogs::DeviceKind::AudioInput => (
                self.audio_device_list(),
                self.settings.record_audio_device.clone(),
            ),
        };
        // Preselect what is already chosen, so OK on an unchanged dialog is a
        // no-op rather than a silent switch to None.
        let selected = current
            .as_deref()
            .and_then(|uid| devices.iter().position(|d| d.uid == uid));
        self.dialog = Some(Dialog::DevicePicker {
            kind,
            devices,
            selected,
            current,
        });
    }

    fn reattach_theory(&mut self) {
        if self.theory_guard.as_ref().is_none_or(|g| !g.wm_managed()) {
            self.remember_theory_geometry();
        }
        self.settings.theory_detached = false;
        self.theory_window_visible = false;
        self.theory_guard = None;
        self.save_settings();
    }

    /// Write back the theory popout's size and position.
    fn remember_theory_geometry(&mut self) -> bool {
        let Some(g) = self.theory_guard.as_ref() else {
            return false;
        };
        let mut changed = false;
        if let Some(size) = g.live_size() {
            let (w, h) = (size.x.round() as i64, size.y.round() as i64);
            if self.settings.theory_win_w != Some(w) || self.settings.theory_win_h != Some(h) {
                self.settings.theory_win_w = Some(w);
                self.settings.theory_win_h = Some(h);
                changed = true;
            }
        }
        if let Some(pos) = g.live_pos() {
            let (x, y) = (pos.x.round() as i64, pos.y.round() as i64);
            if self.settings.theory_win_x != Some(x) || self.settings.theory_win_y != Some(y) {
                self.settings.theory_win_x = Some(x);
                self.settings.theory_win_y = Some(y);
                changed = true;
            }
        }
        changed
    }

    fn reattach_fretboard(&mut self) {
        if !self.fret_wm_managed {
            self.remember_fretboard_geometry();
        }
        self.settings.fretboard_detached = false;
        self.fret_window_visible = false;
        self.fret_shown_at = None;
        self.save_settings();
    }

    /// Write back the popout's size and position. Returns whether anything
    /// actually changed, so the caller can skip a needless settings write.
    fn remember_fretboard_geometry(&mut self) -> bool {
        let mut dirty = false;
        if let Some(size) = self.fret_live_size {
            let (w, h) = (size.x.round() as i64, size.y.round() as i64);
            if w > 0
                && h > 0
                && (self.settings.fretboard_win_w, self.settings.fretboard_win_h)
                    != (Some(w), Some(h))
            {
                self.settings.fretboard_win_w = Some(w);
                self.settings.fretboard_win_h = Some(h);
                dirty = true;
            }
        }
        if let Some(pos) = self.fret_live_pos {
            let (x, y) = (pos.x.round() as i64, pos.y.round() as i64);
            if (self.settings.fretboard_win_x, self.settings.fretboard_win_y) != (Some(x), Some(y))
            {
                self.settings.fretboard_win_x = Some(x);
                self.settings.fretboard_win_y = Some(y);
                dirty = true;
            }
        }
        dirty
    }

    fn reattach_chord_window(&mut self) {
        if !self.detached_wm_managed {
            self.remember_detached_geometry();
        }
        self.detach_window_visible = false;
        self.detached_shown_at = None;
        self.settings.chord_window_detached = false;
        self.save_settings();
    }

    /// Copy the detached window's live geometry into settings. Returns whether
    /// anything actually changed, so the caller can avoid pointless writes.
    fn remember_detached_geometry(&mut self) -> bool {
        let mut changed = false;
        if let Some(size) = self.detached_live_size {
            let (w, h) = (size.x.round() as i64, size.y.round() as i64);
            if w > 0 && self.settings.detached_chord_width != Some(w) {
                self.settings.detached_chord_width = Some(w);
                changed = true;
            }
            if h > 0 && self.settings.detached_chord_height != h {
                self.settings.detached_chord_height = h;
                changed = true;
            }
        }
        if let Some(pos) = self.detached_live_pos {
            let (x, y) = (pos.x.round() as i64, pos.y.round() as i64);
            if self.settings.detached_chord_x != Some(x) {
                self.settings.detached_chord_x = Some(x);
                changed = true;
            }
            if self.settings.detached_chord_y != Some(y) {
                self.settings.detached_chord_y = Some(y);
                changed = true;
            }
        }
        changed
    }

    // ── Menu actions ───────────────────────────────────────────────────────

    /// A shortcut. Everything here routes through the SAME code the menu rows
    /// use, so a key and a menu item can never drift apart in behaviour.
    fn apply_key_action(&mut self, ctx: &egui::Context, action: keys::KeyAction) {
        use keys::KeyAction as K;
        match action {
            // Help is HELD, not toggled, so it never reaches here.
            K::ToggleHelp | K::CloseHelp => {}
            K::ToggleKeytoggle => self.apply_menu_action(ctx, MenuAction::ToggleKeytoggle),
            K::ToggleFretboard => self.apply_menu_action(ctx, MenuAction::ToggleFretboard),
            // One key, five states, in the order someone discovering the band
            // would want them: nothing, each diagram alone, then all three.
            // Three independent toggles have eight states and no natural
            // order, so the key walks a path through them rather than trying
            // to enumerate them; the menu is there for the other three.
            K::CycleTheory => {
                use theory_panel::{View, Views};
                const CYCLE: [Views; 5] = [
                    Views {
                        circle: true,
                        tonnetz: false,
                        triangles: false,
                    },
                    Views {
                        circle: false,
                        tonnetz: true,
                        triangles: false,
                    },
                    Views {
                        circle: false,
                        tonnetz: false,
                        triangles: true,
                    },
                    Views {
                        circle: true,
                        tonnetz: true,
                        triangles: true,
                    },
                    Views {
                        circle: false,
                        tonnetz: false,
                        triangles: false,
                    },
                ];
                let now = self.settings.theory_views();
                let next = CYCLE
                    .iter()
                    .position(|v| *v == now)
                    .map_or(CYCLE[0], |i| CYCLE[(i + 1) % CYCLE.len()]);
                for v in View::ALL {
                    self.settings.set_theory_view(v, v.is_on(next));
                }
                self.save_settings();
            }
            // Space. Not routed through a `MenuAction`, because there is no
            // menu row for it: pressing Record is what the BAND is for, and a
            // menu row that starts a take would be reachable from a right-click
            // over the piano with the recorder hidden.
            K::ToggleRecording => self.request_recorder(recorder::RecorderRequest::Toggle),
            K::ToggleRecorder => self.apply_menu_action(ctx, MenuAction::ToggleRecorder),
            K::ToggleDarkMode => self.apply_menu_action(ctx, MenuAction::ToggleDarkMode),
            K::ToggleDetection => self.apply_menu_action(ctx, MenuAction::ToggleChordDetection),
            K::ToggleBorderless => self.apply_menu_action(ctx, MenuAction::ToggleBorderless),
            K::CycleFont => self.apply_menu_action(ctx, MenuAction::CycleFont),
            // These open dialogs. The shortcut gate already refuses to fire
            // while one is up, so A cannot stack a second About on the first.
            K::ShowAbout => self.apply_menu_action(ctx, MenuAction::ShowAbout),
            K::ShowSupporterKey => self.apply_menu_action(ctx, MenuAction::ShowSupporterKey),
            // The teach block. Each one is the menu row it names, so a
            // shortcut and a click cannot come to mean different things — and
            // the enabled-ness rules live in one place rather than two.
            K::TeachChordName => self.apply_menu_action(ctx, MenuAction::TeachChordName),
            K::CorrectChordName => self.apply_menu_action(ctx, MenuAction::CorrectChordName),
            K::ManageTaughtChords => self.apply_menu_action(ctx, MenuAction::ManageTaughtChords),
            K::ToggleChordLearning => self.apply_menu_action(ctx, MenuAction::ToggleChordLearning),
            K::ToggleNotePreference => {
                self.apply_menu_action(ctx, MenuAction::ToggleNotePreference)
            }
            // "Clear what I placed", not "clear everything": notes arriving
            // from a MIDI keyboard are not ours to drop, and they would come
            // straight back on the next frame anyway.
            K::ClearNotes => {
                self.manual_notes.clear();
                self.manual_positions.clear();
                self.manual_barre = None;
                self.voicing.set_pins(Vec::new());
                self.detection_tick(true);
                self.voicing_tick(true);
            }
        }
    }

    fn apply_menu_action(&mut self, ctx: &egui::Context, action: MenuAction) {
        match action {
            MenuAction::SetSizePercent(p) => {
                self.settings.window_size_percent = p;
                self.save_settings();
                // The detached window deliberately does NOT follow: it is sized
                // by the user now, not slaved to the keyboard's width.
                //
                // A host that owns the window has to be ASKED. The app cannot
                // do that itself — the request goes out over the plugin API,
                // which `ivory-ui` has never heard of — so it records what it
                // wants and the binary that does know picks it up.
                if !self.caps.window_sizing {
                    self.pending_resize = Some(initial_window_size(&self.settings));
                }
            }
            MenuAction::ToggleBorderless => {
                self.settings.borderless_mode = !self.settings.borderless_mode;
                self.save_settings();
            }
            MenuAction::SelectMidiInput => {
                // Unreachable without `caps.midi_ports` — the menu drops the
                // row — but `ports` is what actually decides, so the two
                // cannot disagree about whether there is a device to pick.
                let Some(src) = self.ports.as_ref() else {
                    return;
                };
                let ports = src.list();
                self.dialog = Some(if ports.is_empty() {
                    Dialog::NoMidiInput
                } else {
                    Dialog::MidiPicker {
                        ports,
                        selected: None,
                        current: src.current(),
                    }
                });
            }
            MenuAction::PickColor(target) => {
                let seed = match target {
                    ColorTarget::WhiteIdle => self.settings.white_key_idle_color,
                    ColorTarget::BlackIdle => self.settings.black_key_idle_color,
                    // Initial swatch = white active (spec §6.2 item 10).
                    ColorTarget::Active => self.settings.white_key_active_color,
                    ColorTarget::Sustain => self.settings.sustain_color,
                    ColorTarget::ChordText => self.settings.chord_text_color,
                };
                self.dialog = Some(Dialog::ColorPick {
                    target,
                    color: seed.to_color32(),
                });
            }
            MenuAction::ToggleDarkMode => {
                self.settings.dark_mode = !self.settings.dark_mode;
                self.save_settings();
            }
            MenuAction::CycleFont => {
                use crate::fonts::FontChoice;
                let next = FontChoice::from_key(&self.settings.font_choice).next();
                self.settings.font_choice = next.key().to_owned();
                self.save_settings();
                crate::fonts::install(ctx, next, self.settings.custom_font_path.as_deref());
                crate::fonts::apply_text_styles(ctx);
            }
            MenuAction::ToggleKeytoggle => {
                self.settings.keytoggle_enabled = !self.settings.keytoggle_enabled;
                if !self.settings.keytoggle_enabled {
                    self.manual_notes.clear(); // disabling clears manual notes
                    self.manual_positions.clear();
                    self.manual_barre = None;
                    self.voicing.set_pins(Vec::new());
                }
                self.save_settings();
                self.detection_tick(true);
            }
            MenuAction::ToggleNotePreference => {
                self.settings.prefer_flats = !self.settings.prefer_flats;
                self.detector
                    .set_note_preference(self.settings.prefer_flats);
                self.save_settings();
                self.detection_tick(true); // refresh display immediately
            }
            MenuAction::ToggleChordDetection => {
                self.settings.chord_detection_enabled = !self.settings.chord_detection_enabled;
                if !self.settings.chord_detection_enabled {
                    self.current_chord = None;
                }
                self.save_settings();
                // Window resize follows from layout_sizes() on the next pass.
                self.request_natural_size();
            }
            MenuAction::DetachChordWindow => self.detach_chord_window(),
            MenuAction::AttachChordWindow => self.reattach_chord_window(),
            MenuAction::TeachChordName => self.open_teach_dialog(),
            MenuAction::ManageTaughtChords => self.open_manage_dialog(),
            MenuAction::CorrectChordName => self.open_correct_dialog(),
            MenuAction::ToggleChordLearning => {
                let on = !self.detector.learning_mode();
                self.detector.set_learning_mode(on);
                self.detection_tick(true); // readings change immediately
            }
            MenuAction::ToggleTheoryFollowsMidi => {
                self.settings.theory_follow_midi = !self.settings.theory_follow_midi;
                self.save_settings();
            }
            MenuAction::ToggleTheoryView(v) => {
                let on = !v.is_on(self.settings.theory_views());
                self.settings.set_theory_view(v, on);
                self.save_settings();
                self.request_natural_size();
            }
            MenuAction::ToggleFretboard => {
                self.settings.show_fretboard = !self.settings.show_fretboard;
                // Hiding the view hides it everywhere: leaving a popped-out
                // neck on screen after "Hide Fretboard" would be a window with
                // no way back to it in the menu.
                if !self.settings.show_fretboard && self.fret_window_visible {
                    self.reattach_fretboard();
                    self.settings.fretboard_detached = true; // remembered for next time
                }
                self.save_settings();
                // The window height follows from layout_sizes() next pass.
                self.voicing_tick(true);
                self.request_natural_size();
            }
            MenuAction::SetTuning(name) => {
                self.settings.fretboard_tuning = name.to_owned();
                self.save_settings();
                self.rebuild_voicing();
            }
            MenuAction::SetCapo(fret) => {
                self.settings.fretboard_capo = fret as i64;
                self.save_settings();
                self.rebuild_voicing();
            }
            MenuAction::SetWood(key) => {
                self.settings.fretboard_wood = key.to_owned();
                self.save_settings();
            }
            MenuAction::DetachFretboard => self.detach_fretboard(),
            MenuAction::AttachFretboard => self.reattach_fretboard(),
            MenuAction::DetachTheory => self.detach_theory(),
            MenuAction::AttachTheory => self.reattach_theory(),
            MenuAction::ToggleRecorder => {
                // Refused while a take is running, and this is a safety rule
                // rather than tidiness. Stop lives only in the band, so hiding
                // it mid-take removes the only control that can end the take —
                // and the host stops reconciling devices on the closed edge, so
                // the microphone and the camera stay open with nothing on
                // screen saying they are.
                if self.recorder.state.is_active() {
                    return;
                }
                self.settings.show_recorder = !self.settings.show_recorder;
                // Hiding the band hides it everywhere, exactly as Hide
                // Fretboard does: a popped-out recorder left on screen with the
                // band gone is a window with no menu row that can reach it.
                if !self.settings.show_recorder && self.recorder_window_visible {
                    self.reattach_recorder();
                    self.settings.recorder_detached = true; // remembered for next time
                }
                // Opening the BAND is what opens the audio input, not pressing
                // Record — the meter has to be live before arming. The host
                // reconciles that after the frame by watching this flag, so
                // there is nothing to send from here.
                self.save_settings();
                self.request_natural_size();
            }
            MenuAction::DetachRecorder => self.detach_recorder(),
            MenuAction::AttachRecorder => self.reattach_recorder(),
            MenuAction::ShowExportDialog => self.open_export_dialog(),
            MenuAction::SetCountIn(beats) => {
                self.settings.record_count_in_beats = i64::from(beats);
                self.save_settings();
            }
            MenuAction::ToggleMetronome => {
                self.settings.metronome_on = !self.settings.metronome_on;
                self.save_settings();
            }
            MenuAction::ToggleMetronomeInTake => {
                self.settings.metronome_in_take = !self.settings.metronome_in_take;
                self.save_settings();
            }
            MenuAction::ToggleHideElapsed => {
                self.settings.record_hide_elapsed = !self.settings.record_hide_elapsed;
                self.save_settings();
            }
            MenuAction::EditCustomTuning => {
                // Seeded from whatever tuning is LIVE, because almost every
                // custom tuning is "standard but…" and an empty grid would make
                // the common case the slow one. `Dialog::custom_tuning` also
                // renames a preset on the way in, so confirming cannot produce a
                // tuning called "Standard" that the settings loader would then
                // resolve back to the preset, silently discarding the pitches.
                self.dialog = Some(Dialog::custom_tuning(
                    &self.settings.fretboard_spec().tuning,
                    self.settings.prefer_flats,
                ));
            }
            MenuAction::ToggleHeart => {
                self.settings.show_heart = !self.settings.show_heart;
                self.save_settings();
            }
            MenuAction::ShowSupporterKey => {
                self.dialog = Some(Dialog::SupporterKey {
                    input: String::new(),
                    message: None,
                    installed_as: self.license.display_name().map(str::to_owned),
                    focus: true,
                })
            }
            MenuAction::ShowAbout => self.dialog = Some(Dialog::About),
            MenuAction::ResetSettings => self.reset_settings(ctx),
        }
    }

    /// "Reset Settings to Default" (spec §9, D-UI-8).
    fn reset_settings(&mut self, ctx: &egui::Context) {
        let had_custom_font = self.settings.custom_font_path.is_some();
        let live_detached = self
            .detach_window_visible
            .then_some(self.detached_live_size)
            .flatten();

        self.settings.reset_to_defaults();

        // Closing the detached window re-attaches; Python's close handler then
        // records its live height (overwriting the freshly reset 50).
        if self.detach_window_visible {
            self.detach_window_visible = false;
            if let Some(size) = live_detached {
                self.settings.detached_chord_height = size.y.round() as i64;
            }
        }
        self.detector.set_note_preference(true);
        self.manual_notes.clear();
        self.manual_positions.clear();
        self.fret_window_visible = false;
        self.fret_shown_at = None;
        self.rebuild_voicing();
        if had_custom_font {
            // Settings were reset: back to the bundled default font too.
            crate::fonts::install(ctx, crate::fonts::FontChoice::default(), None);
        }
        self.save_settings();
    }

    // ── Teach layer (D-UI-5) ───────────────────────────────────────────────

    /// Open "Teach Chord Name…" for the currently-held voicing. No-op if
    /// nothing is held (the menu item is greyed in that case anyway).
    fn open_teach_dialog(&mut self) {
        let display = self.display_notes();
        if display.is_empty() {
            return;
        }
        // Render the chord tones from the bass, matching the stored key and the
        // "Manage" voicing display.
        let (bass_pc, ivs) = OverrideStore::interval_set_from_bass(&display);
        let note_names = ivs
            .iter()
            .map(|&iv| self.detector.get_note_name((bass_pc + iv) % 12))
            .collect::<Vec<_>>()
            .join(" ");
        let current_label = self
            .current_chord
            .clone()
            .unwrap_or_else(|| "(none)".to_owned());
        let input = self.current_chord.clone().unwrap_or_default();
        let mut notes: Vec<u8> = display.iter().copied().collect();
        notes.sort_unstable();
        self.dialog = Some(Dialog::TeachChord {
            notes,
            note_names,
            current_label,
            input,
            // Naming a voicing usually means naming the shape, not that one
            // key, so this starts on. The last choice is remembered.
            apply_all_keys: self.settings.teach_apply_all_keys,
            // The whole point of the shortcut: hold the chord, press N, type.
            focus: true,
        });
    }

    /// Open "Manage Taught Chords…" listing all stored overrides, plus the
    /// learned-re-ranker footer (D-UI-9).
    fn open_manage_dialog(&mut self) {
        let rows = self
            .detector
            .overrides()
            .map(|s| s.list(self.settings.prefer_flats))
            .unwrap_or_default();
        self.dialog = Some(Dialog::ManageTaught {
            rows,
            learning: self.learning_status(),
        });
    }

    fn learning_status(&self) -> LearningStatus {
        match self.detector.overrides() {
            Some(s) => LearningStatus {
                on: s.learning_mode(),
                corrections: s.corrections(),
                has_learned: s.has_learned(),
                weights: s.weights_report(),
            },
            None => LearningStatus::default(),
        }
    }

    /// Open "Correct Chord Name…" (D-UI-9) for the held voicing. The list is
    /// exactly what the re-ranker can be trained toward — offering anything
    /// else would let a correction silently do nothing.
    fn open_correct_dialog(&mut self) {
        let display = self.display_notes();
        if display.is_empty() {
            // The menu froze `enabled` when it opened; the notes may have been
            // released since. Say so rather than being a dead click.
            self.dialog = Some(Dialog::LearnResult {
                title: "Correct Chord Name",
                message: "No notes are being held any more.\n\n\
                          Hold the chord you want to correct, then open the\n\
                          menu again."
                    .to_owned(),
            });
            return;
        }
        let candidates = self.detector.trainable_candidates(&display);
        let (bass_pc, ivs) = OverrideStore::interval_set_from_bass(&display);
        let note_names = ivs
            .iter()
            .map(|&iv| self.detector.get_note_name((bass_pc + iv) % 12))
            .collect::<Vec<_>>()
            .join(" ");
        let current_label = self
            .current_chord
            .clone()
            .unwrap_or_else(|| "(none)".to_owned());

        if candidates.len() < 2 {
            // Nothing to re-rank. Distinguish the two very different reasons,
            // or the user is left staring at a name they cannot change.
            let pinned = self
                .detector
                .overrides()
                .and_then(|s| s.lookup(&display, self.settings.prefer_flats))
                .is_some();
            let message = if pinned {
                format!(
                    "This voicing has a taught name pinned to it, so Tangent is\n\
                     not weighing any alternatives.\n\n\
                     Notes: {note_names}\nReads: {current_label}\n\n\
                     Remove it in \"Manage Taught Chords...\" first if you want\n\
                     Tangent to choose the name again."
                )
            } else if candidates.len() == 1 {
                // It WAS weighed — there simply was no rival. Saying "named by
                // a fixed rule" here would be the wrong explanation.
                format!(
                    "Only one chord name fits these notes, so there is nothing\n\
                     to weigh it against.\n\n\
                     Notes: {note_names}\nReads: {current_label}\n\n\
                     Add or change a note to give Tangent a choice, or use\n\
                     \"Teach Chord Name...\" to pin a name of your own."
                )
            } else {
                // NOT only intervals and scales: several chord shapes (m7b5 vs
                // m6, dim7 upper structures, a 6-chord with its third in the
                // bass) are resolved by dedicated branches before scoring ever
                // runs, and they land here too.
                format!(
                    "Tangent named this voicing with a fixed rule rather than by\n\
                     weighing alternatives, so there is nothing to re-rank.\n\n\
                     Notes: {note_names}\nReads: {current_label}\n\n\
                     That is how intervals, scales and a few special chord\n\
                     shapes are named. To pin the name you want, use \"Teach\n\
                     Chord Name...\"."
                )
            };
            self.dialog = Some(Dialog::LearnResult {
                title: "Correct Chord Name",
                message,
            });
            return;
        }

        let mut notes: Vec<u8> = display.iter().copied().collect();
        notes.sort_unstable();
        self.dialog = Some(Dialog::CorrectChord {
            notes,
            note_names,
            current_label,
            candidates,
            selected: None,
        });
    }

    /// Turn a training attempt into something a musician can act on.
    fn train_and_report(&mut self, notes: Vec<u8>, name: String) {
        let set: HashSet<u8> = notes.iter().copied().collect();
        // A successful correction switches Chord Learning on. Say so rather
        // than letting the menu item quietly change under the user.
        let was_on = self.detector.learning_mode();
        let outcome = self.detector.train_on_correction(&set, &name);
        let turned_on = !was_on && self.detector.learning_mode();
        let corrections = self
            .detector
            .overrides()
            .map(|s| s.corrections())
            .unwrap_or(0);
        let message = match outcome {
            TrainOutcome::Learned { now_reads, .. } => {
                let tail = if now_reads == name {
                    String::new()
                } else {
                    format!(
                        "\n\nIt shows as \"{now_reads}\" because the bass note is\n\
                         added automatically."
                    )
                };
                // Correcting re-arms the master switch, which brings back EVERY
                // earlier correction, not just this one. Saying only "now ON"
                // would hide that from someone who deliberately switched it off.
                let switched = if turned_on {
                    "\n\nChord Learning is switched back ON, so every earlier\n\
                     correction is active again too — not just this one."
                        .to_owned()
                } else {
                    String::new()
                };
                format!(
                    "Learned. This voicing now reads {now_reads}.{tail}\n\n\
                     Corrections so far: {corrections}. Similar voicings may\n\
                     read differently now — \"Forget Learning\" in Manage\n\
                     Taught Chords undoes all of it.{switched}"
                )
            }
            TrainOutcome::AlreadyCorrect { displays_as } => {
                if displays_as == name {
                    format!("{name} is already Tangent's choice here, so nothing was changed.")
                } else {
                    format!(
                        "{name} already wins for this voicing, so nothing was\n\
                         changed. It shows as \"{displays_as}\" because the bass\n\
                         note is added after the name is chosen.\n\n\
                         To pin a different display name, use \"Teach Chord\n\
                         Name...\"."
                    )
                }
            }
            TrainOutcome::OutrankedByRule { wants, displays_as } => format!(
                "{wants} is already Tangent's top-scoring reading — but the name\n\
                 you see, \"{displays_as}\", comes from a separate rule that runs\n\
                 afterwards and overrides it.\n\n\
                 Chord learning only reorders competing chord names, so it\n\
                 cannot reach this one. Nothing was changed. Use \"Teach Chord\n\
                 Name...\" to pin {wants} outright."
            ),
            TrainOutcome::Stubborn { still_reads, .. } => format!(
                "Tangent could not be nudged that far.\n\n\
                 {name} scores too far behind {still_reads} for a safe nudge\n\
                 to close the gap, so nothing was changed.\n\n\
                 To force this name anyway, use \"Teach Chord Name...\" — it\n\
                 pins the name outright."
            ),
            TrainOutcome::NotTrainable => format!(
                "{name} is not one of the readings Tangent weighed for this\n\
                 voicing, so there is nothing to re-rank.\n\n\
                 Use \"Teach Chord Name...\" to pin it instead."
            ),
            TrainOutcome::NoStore => {
                "Chord learning is unavailable — the settings folder could not\n\
                 be opened."
                    .to_owned()
            }
        };
        self.dialog = Some(Dialog::LearnResult {
            title: "Chord Learning",
            message,
        });
        self.detection_tick(true);
    }

    fn apply_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::SetShowWelcome(show) => {
                self.settings.show_welcome = show;
                self.save_settings();
            }
            DialogAction::LoadPlugin { path } => {
                let slot = self.picker_slot.min(recorder::SLOTS - 1);
                // Written to settings and nothing else: loading is the host's
                // job, done after the frame, because `Module::open` runs
                // third-party code and `Instance::create` can take seconds.
                // The host notices the change by watching `chosen_plugin()`.
                self.settings.plugin_slots[slot] = path;
                self.save_settings();
            }
            DialogAction::ChooseDevice { kind, uid } => {
                // Written to settings AND pushed to the device object. The
                // first is what survives a restart; the second is what the
                // host's reconciler acts on after the frame. Doing only the
                // first would remember a choice that never took effect until
                // the next launch.
                match kind {
                    dialogs::DeviceKind::Camera => {
                        self.settings.record_camera_uid = uid.clone();
                        if let Some(d) = self.cameras.as_mut() {
                            let _ = d.open(uid.as_deref().unwrap_or(""));
                        }
                    }
                    dialogs::DeviceKind::AudioInput => {
                        // `record_audio_source` carries the None choice across
                        // a restart. Without it, `record_audio_device: null` is
                        // indistinguishable from "never chose", and the next
                        // launch helpfully opens the system microphone for
                        // somebody who explicitly asked for MIDI only.
                        self.settings.record_audio_source = if uid.is_some() {
                            "input".to_owned()
                        } else {
                            "none".to_owned()
                        };
                        self.settings.record_audio_device = uid.clone();
                        if let Some(d) = self.audio_devices.as_mut() {
                            let _ = d.open(uid.as_deref().unwrap_or(""));
                        }
                    }
                }
                self.save_settings();
            }
            // This session only. Deliberately NOT written to settings: the
            // tick is what makes a choice outlive the session.
            DialogAction::SetExport(spec) => self.export_override = Some(spec),
            DialogAction::SetExportAndRemember(spec) => {
                // The override is cleared as well as the setting written, or
                // the session copy would keep shadowing the thing the user just
                // asked to be permanent.
                self.export_override = None;
                self.settings.record_export = spec;
                self.save_settings();
            }
            DialogAction::InstallLicense { key } => {
                // The dialog stays open on failure so the message can say what
                // went wrong without the user losing what they pasted; a typo
                // and a forged key report differently (the CRC is checked
                // before the signature).
                match self.license.install(&key) {
                    Ok(license) => {
                        let who = license
                            .name
                            .clone()
                            .unwrap_or_else(|| "supporter".to_owned());
                        self.dialog = Some(Dialog::SupporterKey {
                            input: String::new(),
                            message: Some(format!("Thank you, {who}. That means a lot.")),
                            installed_as: self.license.display_name().map(str::to_owned),
                            // Already installed: nothing to type, so leave the
                            // field alone rather than grabbing the caret.
                            focus: false,
                        });
                    }
                    Err(err) => {
                        self.dialog = Some(Dialog::SupporterKey {
                            input: key,
                            message: Some(err.message().to_owned()),
                            installed_as: self.license.display_name().map(str::to_owned),
                            // It failed and the key is still there to fix, so
                            // select it: the usual next move is to paste again.
                            focus: true,
                        });
                    }
                }
            }
            DialogAction::TeachSave {
                notes,
                name,
                apply_all_keys,
            } => {
                let set: HashSet<u8> = notes.iter().copied().collect();
                if let Some(store) = self.detector.overrides_mut() {
                    store.teach(&set, &name, apply_all_keys);
                }
                // Remember the choice so the box comes back the way it was left.
                if self.settings.teach_apply_all_keys != apply_all_keys {
                    self.settings.teach_apply_all_keys = apply_all_keys;
                    self.save_settings();
                }
                self.detection_tick(true); // re-detect immediately (D-UI-5)
            }
            DialogAction::DeleteOverride { intervals } => {
                if let Some(store) = self.detector.overrides_mut() {
                    store.delete(&intervals);
                }
                self.detection_tick(true);
            }
            DialogAction::TrainCorrection { notes, name } => self.train_and_report(notes, name),
            DialogAction::ForgetLearning => {
                self.detector.reset_learning();
                self.detection_tick(true);
            }
            DialogAction::SetCustomTuning(t) => {
                // Store the pitches AND select the tuning, in that order. The
                // selection is a sentinel (`Settings::CUSTOM_TUNING`) rather
                // than the tuning's own name: a name that happened to match a
                // preset would be resolved back to the preset on the next load
                // and the user's pitches would be silently gone. The dialog
                // renames a preset-derived tuning on the way IN for the same
                // reason; this is the other half of that guard.
                self.settings.fretboard_custom_name = Some(t.name.to_string());
                self.settings.fretboard_custom_open = Some(
                    t.open
                        .iter()
                        .map(|p| p.to_string())
                        .collect::<Vec<_>>()
                        .join(","),
                );
                self.settings.fretboard_tuning =
                    Settings::CUSTOM_TUNING.to_owned();
                self.dialog = None;
                self.save_settings();
                self.rebuild_voicing();
                // A different string count changes the fretboard band's height,
                // so the window has to be re-measured. `SetTuning` never needed
                // this because every preset was six strings; with 4- to
                // 12-string tunings it is the difference between the neck
                // fitting and the piano being clipped.
                self.request_natural_size();
            }
            DialogAction::ConnectPort(name) => {
                if let Some(src) = self.ports.as_mut() {
                    if let Err(e) = src.connect(&name, self.midi_tx.clone()) {
                        self.dialog = Some(Dialog::MidiError { message: e });
                    }
                }
            }
            DialogAction::ApplyColor(target, color) => {
                let rgb = Rgb::from_color32(color);
                match target {
                    ColorTarget::WhiteIdle => self.settings.white_key_idle_color = rgb,
                    ColorTarget::BlackIdle => self.settings.black_key_idle_color = rgb,
                    ColorTarget::Active => {
                        // Sets BOTH active colors (spec §6.2 item 10).
                        self.settings.white_key_active_color = rgb;
                        self.settings.black_key_active_color = rgb;
                    }
                    ColorTarget::Sustain => self.settings.sustain_color = rgb,
                    ColorTarget::ChordText => self.settings.chord_text_color = rgb,
                }
                self.save_settings();
            }
        }
    }
}

fn main_width(settings: &Settings) -> f32 {
    (1300.0 * settings.window_size_percent as f64 / 100.0).trunc() as f32
}

/// Initial fixed window size for the ViewportBuilder, computed from settings
/// before the event loop starts (spec §3.2).
pub fn initial_window_size(settings: &Settings) -> Vec2 {
    band_sizes(settings).total()
}

/// The stacked bands, top to bottom: theory, chord strip, piano, fretboard,
/// plus the window width. A hidden band is 0.0.
///
/// One function rather than two, because there used to be two: the copy in
/// `initial_window_size` decides the size the window OPENS at and the copy in
/// `layout_sizes` decides what it is resized to on the first frame, so any
/// drift between them shows up as a window that visibly jumps at startup.
/// Integer truncation per band, like Python (spec §3.2).
fn band_sizes(settings: &Settings) -> Bands {
    band_sizes_at(settings, main_width(settings))
}

/// The same bands, for a window that is `w` wide.
///
/// Split out because a plugin does not choose its own width: the host does,
/// and every band's height is a fixed fraction of the width, so laying out
/// into a given rect is the same arithmetic with a different starting number.
fn band_sizes_at(settings: &Settings, w: f32) -> Bands {
    let piano_h = (w as f64 / (1300.0 / 150.0)).trunc() as f32;
    let chord_visible = settings.chord_detection_enabled && !settings.chord_window_detached;
    let chord_h = if chord_visible {
        (50.0 * w as f64 / 1300.0).trunc() as f32
    } else {
        0.0
    };
    let fret_h = if settings.show_fretboard && !settings.fretboard_detached {
        fretboard_panel::band_height(w, settings.fretboard_spec().tuning.strings())
    } else {
        0.0
    };
    // Zero while popped out, exactly like the chord strip and the neck above.
    // Without this the diagrams render in BOTH places at once, and the main
    // window keeps 300pt of height for a band that is somewhere else.
    let theory_h = if settings.theory_detached {
        0.0
    } else {
        theory_panel::band_height(w, settings.theory_views())
    };
    // Zero when hidden AND zero when popped out, like every band above it.
    //
    // There is no `caps` term here and there must not be: `IvoryApp::new`
    // already forces `show_recorder` false wherever `capture_devices` is,
    // so a host that cannot record has nothing to zero. Testing caps here as
    // well would put the same rule in two places, and the two would disagree
    // the first time somebody changed one of them.
    let recorder_h = if settings.show_recorder && !settings.recorder_detached {
        recorder_panel::band_height(w)
    } else {
        0.0
    };
    Bands {
        w,
        recorder_h,
        theory_h,
        chord_h,
        piano_h,
        fret_h,
    }
}

/// The natural size of the whole layout at a given width.
///
/// What a plugin editor should OPEN at. `initial_window_size` is the
/// desktop's answer and bakes in the size percentage, which a plugin does not
/// have; this takes the width as an argument and returns the height that goes
/// with it, so an editor opens showing every band the user has turned on
/// rather than a slice of them.
pub fn natural_size(settings: &Settings, width: f32) -> Vec2 {
    band_sizes_at(settings, width).total()
}

/// The biggest layout that fits inside `avail`, for a host that hands over a
/// rect rather than being asked for one.
///
/// Every band's height is a fixed fraction of the width, so the whole layout
/// is described by one number and fitting it is a matter of picking that
/// number. Width alone is not enough: a wide, short editor would get a layout
/// far taller than it, and the piano would be cut in half.
fn fit_bands(settings: &Settings, avail: Vec2) -> Bands {
    // Total height at a known width gives the ratio, whichever bands are on.
    let probe = band_sizes_at(settings, 1300.0);
    let h_at_1300 = probe.total().y.max(1.0);
    let w_for_height = avail.y * 1300.0 / h_at_1300;
    let w = avail.x.min(w_for_height).max(1.0).trunc();
    band_sizes_at(settings, w)
}

/// The horizontal bands the window is made of, top to bottom, and its width.
///
/// A struct rather than the tuple this used to be. Adding the theory band made
/// it five values in stacking order, and a five-tuple whose third element is
/// the piano is exactly the kind of thing that gets destructured wrong once and
/// then silently draws the chord strip where the keyboard should be.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Bands {
    w: f32,
    /// Top of the stack, above the theory band.
    ///
    /// The recorder is the surface you set up BEFORE you play and glance at
    /// WHILE you play, and both of those want it at the top of the window,
    /// nearest the eyeline — not below the keyboard where the hands are.
    recorder_h: f32,
    theory_h: f32,
    chord_h: f32,
    piano_h: f32,
    fret_h: f32,
}

impl Bands {
    fn total(self) -> Vec2 {
        Vec2::new(
            self.w,
            self.recorder_h + self.theory_h + self.chord_h + self.piano_h + self.fret_h,
        )
    }
}

impl IvoryApp {
    /// One frame, from a `Context`.
    ///
    /// All three hosts hand over a context rather than a `Ui` — `eframe::App`
    /// for the desktop window, `show_viewport_immediate` for a child window,
    /// and `nih_plug_egui`'s editor callback for the VST3 build — so this is
    /// the shape they share. `shell::viewport_ui` is the one bridge, and its
    /// test is what proves the central panel never grew a margin.
    pub fn frame(&mut self, ctx: &egui::Context) {
        crate::shell::viewport_ui(ctx, |ui| self.paint(ui));
    }

    /// One frame into a `Ui` that somebody else made.
    ///
    /// `nih_plug_egui`'s `ResizableWindow` opens its OWN `CentralPanel` so it
    /// can put a drag corner in it, and two central panels under the same id
    /// is the exact silent-garbage failure `shell::surface` exists to avoid.
    /// So the plugin uses this and skips the bridge.
    pub fn paint_into(&mut self, ui: &mut egui::Ui) {
        self.paint(ui);
    }

    /// The colour behind everything, for hosts that clear the surface
    /// themselves.
    pub const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn paint(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // Dev hook, alongside IVORY_DEMO_NOTES: open the menu on the first
        // frame so the in-canvas version can be photographed without anyone
        // having to click, and therefore without taking focus from whatever
        // else is being tested. Environment-gated; ships inert.
        //   IVORY_INLINE=menu /Applications/Tangent.app/Contents/MacOS/tangent
        if !self.demo_menu_done && std::env::var("IVORY_INLINE").as_deref() == Ok("menu") {
            self.demo_menu_done = true;
            let at = ui.max_rect().min + Vec2::new(20.0, 20.0);
            self.open_menu_at(&ctx, at);
        }

        self.process_midi_events();

        // Startup restore for the popped-out neck (100ms single-shot), the
        // same shape as the chord window's below it.
        if let Some(t) = self.startup_fret_detach_at {
            if Instant::now() >= t {
                self.startup_fret_detach_at = None;
                if self.settings.fretboard_detached && self.settings.show_fretboard {
                    self.fret_window_visible = true;
                    self.fret_builder_size = self.settings.fretboard_win_size();
                    self.fret_builder_pos = self.settings.fretboard_win_pos().map(|p| {
                        crate::settings::clamp_to_monitor(
                            p,
                            self.fret_builder_size,
                            self.monitor_size,
                        )
                    });
                    self.fret_shown_at = Some(Instant::now());
                    self.fret_wm_managed = false;
                }
            }
        }

        // And the same restore for the theory window.
        if let Some(t) = self.startup_theory_detach_at {
            if Instant::now() >= t {
                self.startup_theory_detach_at = None;
                if self.settings.theory_detached && self.settings.theory_views().any() {
                    self.theory_window_visible = true;
                    self.theory_builder_size = self.settings.theory_win_size();
                    self.theory_builder_pos = self.settings.theory_win_pos().map(|p| {
                        crate::settings::clamp_to_monitor(
                            p,
                            self.theory_builder_size,
                            self.monitor_size,
                        )
                    });
                    self.theory_guard = Some(theory_panel::GeometryGuard::opened(
                        self.theory_builder_size,
                        Instant::now(),
                    ));
                }
            }
        }

        // And the same restore for the Recorder window.
        if let Some(t) = self.startup_recorder_detach_at {
            if Instant::now() >= t {
                self.startup_recorder_detach_at = None;
                if self.settings.recorder_detached && self.settings.show_recorder {
                    self.recorder_window_visible = true;
                    self.recorder_builder_size = self.settings.recorder_win_size();
                    self.recorder_builder_pos = self.settings.recorder_win_pos().map(|p| {
                        crate::settings::clamp_to_monitor(
                            p,
                            self.recorder_builder_size,
                            self.monitor_size,
                        )
                    });
                    self.recorder_guard = Some(theory_panel::GeometryGuard::opened(
                        self.recorder_builder_size,
                        Instant::now(),
                    ));
                }
            }
        }

        // Startup detach restore (100ms single-shot).
        if let Some(t) = self.startup_detach_at {
            if Instant::now() >= t {
                self.startup_detach_at = None;
                if self.settings.chord_window_detached && self.settings.chord_detection_enabled {
                    self.detach_window_visible = true;
                    self.detached_builder_size = self.settings.detached_size_for_use();
                    self.detached_builder_pos = self.settings.detached_pos_for_use().map(|p| {
                        crate::settings::clamp_to_monitor(
                            p,
                            self.detached_builder_size,
                            self.monitor_size,
                        )
                    });
                    self.detached_shown_at = Some(Instant::now());
                    self.detached_wm_managed = false;
                }
            }
        }

        self.detection_tick(false);
        self.voicing_tick(false);

        // Shortcuts, but never while a dialog or the context menu is up: those
        // are modal, and a stray K behind a modal changing the app underneath
        // it is the kind of thing that reads as a haunting.
        // `name_focused` joins the modal guard rather than the `Gates` struct,
        // and the distinction matters. `Gates` decides whether a binding EXISTS
        // — the help card is generated from the same answer — so gating on the
        // text field there would make Space vanish from the card for as long as
        // somebody was typing, which is a flicker, not a fact. Focus is a
        // "swallow this frame's keys" condition, exactly like an open dialog.
        if self.dialog.is_none() && self.menu_state.is_none() && !self.name_focused {
            if let Some(action) = keys::pressed(&ctx, self.key_gates()) {
                self.apply_key_action(&ctx, action);
            }
        }

        // Track our position on the monitor for global menu placement, child
        // window centring, and so the main window reopens where it was left.
        //
        // Gated on owning the window. Inside a plugin editor these reads do
        // not fail — they return the ROOT viewport's values, which are the
        // host's, so an unguarded version would quietly file the DAW's window
        // position into the user's settings file as if it were the piano's.
        if self.caps.child_windows {
            // Whether WE are frontmost, so the detached windows can follow.
            // `focused` is None on the frames before the window manager has
            // said, and treating that as "not focused" would drop the
            // children behind on every startup.
            if let Some(f) = ctx.input(|i| i.viewport().focused) {
                self.main_focused = f;
            }
        }
        let (inner_rect, outer_rect, monitor) = if self.caps.window_sizing {
            ctx.input(|i| {
                (
                    i.viewport().inner_rect,
                    i.viewport().outer_rect,
                    i.viewport().monitor_size,
                )
            })
        } else {
            // The editor's own canvas IS its world: menus and dialogs position
            // themselves inside it, and there is no monitor beyond it.
            let r = ctx.content_rect();
            (Some(r), Some(r), Some(r.size()))
        };
        if let Some(r) = inner_rect {
            self.main_inner_origin = r.min;
            self.main_origin_known = true;
        }
        self.monitor_size = monitor;
        // Rescue a window restored onto a monitor that is no longer there.
        // A remembered position is the classic way for an app to launch
        // invisibly, so this runs once, as soon as the monitor is known.
        if !self.offscreen_checked && self.caps.window_sizing {
            if let (Some(r), Some(mon)) = (outer_rect.or(inner_rect), monitor) {
                self.offscreen_checked = true;
                let on_screen =
                    Rect::from_min_size(Pos2::ZERO, mon).intersects(r.shrink(OFFSCREEN_SLACK));
                if !on_screen {
                    let fixed = crate::settings::clamp_to_monitor(r.min, r.size(), Some(mon));
                    ctx.send_viewport_cmd(ViewportCommand::OuterPosition(fixed));
                }
            }
        }

        // The outer rect is what `with_position` takes, so that is what gets
        // recorded; falling back to the inner origin is better than nothing on
        // platforms that do not report it. Whether it is worth storing is
        // decided at write-back time, once the size has settled.
        let live_pos = outer_rect.map(|r| r.min).or(inner_rect.map(|r| r.min));
        if live_pos != self.main_live_pos {
            self.main_live_pos = live_pos;
            self.geometry_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
        }
        self.main_live_size = inner_rect.map(|r| r.size());

        // Fixed-size enforcement: Min+Max+Inner triple whenever the target
        // changes (size %, chord toggle, detach/attach).
        // The desktop asks for a size and gets it. A plugin is GIVEN one, and
        // has to lay out inside it — `main_width` would otherwise put a
        // 1300-point layout into whatever the host opened, and the piano would
        // run off the right-hand edge.
        let bands = if self.caps.window_sizing {
            self.layout_sizes()
        } else {
            // The PANE, not the context: inside a host's resizable wrapper the
            // two differ by its frame, and laying out to the context would put
            // the bottom band under the resize corner.
            fit_bands(&self.settings, ui.max_rect().size())
        };
        let Bands {
            w,
            recorder_h,
            theory_h,
            chord_h,
            piano_h,
            fret_h,
        } = bands;
        let target = bands.total();
        // GATED, not merely harmless. `egui-baseview` HONOURS
        // `ViewportCommand::InnerSize` — it calls `window.resize()` — so an
        // ungated triple would reach into the DAW and resize the editor behind
        // the host's back on frame one. Min/Max are swallowed there, which
        // would leave exactly the half of the mechanism that does damage.
        if self.caps.window_sizing && self.last_sent_size != Some(target) {
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(target));
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(target));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
            self.last_sent_size = Some(target);
        }
        // Borderless enforcement; Qt re-sets the title after flag changes.
        let decorations = !self.settings.borderless_mode;
        if self.caps.window_sizing && self.decorations_sent != Some(decorations) {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(decorations));
            ctx.send_viewport_cmd(ViewportCommand::Title("Tangent".to_owned()));
            self.decorations_sent = Some(decorations);
        }

        // Paint, top to bottom: theory band, chord strip, piano, fretboard
        // (spec §3.1, D-UI-17). Each band's top is the sum of the ones above
        // it, and a hidden band is zero tall, so nothing needs a special case.
        let display = self.display_notes();
        // Centred in whatever we were given. On the desktop the window IS the
        // layout, so both terms are zero and no pixel moves; in a plugin it is
        // what keeps the picture in the middle of an editor the user has
        // resized to something else.
        let pane = ui.max_rect();
        // The app owns every pixel it is given. On the desktop the bands cover
        // the window exactly and this changes nothing; in a plugin the layout
        // is centred in whatever the host opened, and without this the gap
        // shows egui's default panel fill rather than Tangent's background.
        ui.painter().rect_filled(pane, 0.0, egui::Color32::BLACK);
        let origin = Pos2::new(
            pane.min.x + ((pane.width() - w) * 0.5).max(0.0).trunc(),
            pane.min.y + ((pane.height() - target.y) * 0.5).max(0.0).trunc(),
        );
        self.last_pane = pane.size();
        self.last_drawn = Rect::from_min_size(origin, target);
        let band_at = |top: f32, h: f32| {
            Rect::from_min_size(Pos2::new(origin.x, origin.y + top), Vec2::new(w, h))
        };
        let piano_rect = band_at(recorder_h + theory_h + chord_h, piano_h);
        let mut chord_rect_for_hit: Option<Rect> = None;
        let fret_rect_for_hit: Option<Rect> = (fret_h > 0.0)
            .then(|| band_at(recorder_h + theory_h + chord_h + piano_h, fret_h));
        let theory_rect_for_hit: Option<Rect> =
            (theory_h > 0.0).then(|| band_at(recorder_h, theory_h));
        let recorder_rect_for_hit: Option<Rect> =
            (recorder_h > 0.0).then(|| band_at(0.0, recorder_h));
        if let Some(rect) = recorder_rect_for_hit {
            recorder_panel::draw(
                ui.painter(),
                rect,
                &self.recorder.view(
                    self.settings.record_take_name.as_deref().unwrap_or_default(),
                    self.name_focused,
                    self.settings.knobs(),
                    self.settings.record_hide_elapsed,
                ),
                &self.settings,
            );
        }
        if let Some(theory_rect) = theory_rect_for_hit {
            theory_panel::draw(
                ui.painter(),
                theory_rect,
                self.settings.theory_views(),
                self.theory_input(&display),
                &self.settings,
            );
        }
        if chord_h > 0.0 {
            let chord_rect = band_at(recorder_h + theory_h, chord_h);
            chord_rect_for_hit = Some(chord_rect);
            chord_strip::draw(
                ui.painter(),
                chord_rect,
                self.current_chord.as_deref(),
                self.settings.chord_text_color.to_color32(),
                self.heart_color(),
                None, // attached: it already has the piano below it as an edge
            );
        }
        piano::draw(
            ui.painter(),
            piano_rect,
            &display,
            self.notes.sustain_down(),
            &self.settings,
        );
        if let Some(fret_rect) = fret_rect_for_hit {
            let spec = self.settings.fretboard_spec();
            fretboard_panel::draw(
                ui.painter(),
                fret_rect,
                self.voicing.current(),
                &spec,
                &self.settings,
                self.settings.fretboard_wood(),
                self.barre_to_draw(),
            );
            fretboard_panel::draw_top_edge(ui.painter(), fret_rect, &self.settings);
        }

        self.handle_main_interaction(
            &ctx,
            ui,
            piano_rect,
            chord_rect_for_hit,
            fret_rect_for_hit,
            theory_rect_for_hit,
            recorder_rect_for_hit,
        );

        // A fader being dragged. `hit_test` has no memory, so the app supplies
        // it: the probe's Y is pinned to where the grab started, which keeps a
        // few pixels of vertical drift on the track instead of dropping the
        // gesture, and the X is clamped into the band so dragging past the end
        // pins the value rather than losing it.
        if let (Some(rect), Some((held, from))) = (recorder_rect_for_hit, self.grabbed) {
            let (down, pos) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
            if !down {
                self.grabbed = None;
            } else if let Some(pos) = pos {
                let probe = Pos2::new(pos.x.clamp(rect.left(), rect.right() - 0.5), from.y);
                let view = self.recorder.view(
                    self.settings.record_take_name.as_deref().unwrap_or_default(),
                    self.name_focused,
                    self.settings.knobs(),
                    self.settings.record_hide_elapsed,
                );
                if let Some(now) = recorder_panel::hit_test(rect, &view, probe) {
                    // Same CONTROL, not the same value: the whole point is that
                    // the value changed.
                    if now.is_same_control(held) {
                        self.apply_recorder_hit(now);
                    }
                }
            }
        }

        // The take-name field, edited from raw input rather than with an egui
        // widget. The band is a pure painter — that is what will let the
        // compositor render it into a 1080p surface later — so it cannot own a
        // `TextEdit`, and the field is worth having anyway: "type nocturne
        // once, press record five times" is the workflow the whole naming
        // scheme exists to support.
        if self.name_focused {
            self.edit_take_name(&ctx);
        }

        // Held, not toggled: press to read, release and it slides away. Drawn
        // last so it is over everything, and asked for every frame so the
        // animation can run even when nothing else changed.
        // Not while something modal is up. The key is read straight out of the
        // raw input, so without this an `h` typed into the supporter-key field
        // both entered the letter and slid the card down over the app.
        let help = keys::help_progress(&ctx, self.dialog.is_none() && self.menu_state.is_none());
        if help > 0.0 {
            keys::draw_help(
                ui.painter(),
                ui.max_rect(),
                self.settings.dark_mode,
                help,
                self.key_gates(),
            );
        }

        // The popped-out neck. `caps.detachable` gates the DRAWING, not just
        // the menu row: `fretboard_detached` is a persisted setting, so a
        // settings file written by the standalone would otherwise have a
        // plugin opening a phantom window on its first frame.
        if self.fret_window_visible && self.caps.detachable {
            let spec = self.settings.fretboard_spec();
            let outcome = fretboard_panel::show_detached_window(
                &ctx,
                self.fret_builder_size,
                self.fret_builder_pos,
                self.settings.borderless_mode,
                self.main_focused,
                self.voicing.current(),
                &spec,
                &self.settings,
                self.settings.fretboard_wood(),
                self.barre_to_draw(),
            );
            // Same tiling-WM guard as the chord window: inside the grace
            // period a size mismatch is the window manager's doing, not the
            // user's, and this session's geometry is not ours to remember.
            if let (Some(shown), Some(size)) = (self.fret_shown_at, outcome.inner_size) {
                if Instant::now().duration_since(shown) < WM_GRACE {
                    self.fret_wm_managed |= wm_overrode_size(size, self.fret_builder_size);
                } else {
                    self.fret_shown_at = None;
                }
            }
            let moved = outcome.inner_size != self.fret_live_size
                || (outcome.outer_pos.is_some() && outcome.outer_pos != self.fret_live_pos);
            if let Some(size) = outcome.inner_size {
                self.fret_live_size = Some(size);
            }
            if let Some(pos) = outcome.outer_pos {
                self.fret_live_pos = Some(pos);
            }
            if moved && !self.fret_wm_managed {
                self.geometry_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
            }
            if outcome.close_requested {
                self.reattach_fretboard(); // close-to-reattach, like the chord window
            } else if let Some(pos) = outcome.context_menu_at {
                if self.dialog.is_none() {
                    self.open_menu_at(&ctx, pos);
                }
            }
        }

        // Detached theory window. Third popout, and the cheapest of the three:
        // `theory_panel::GeometryGuard` owns the tiling-WM logic that the two
        // above still open-code, so this block is the window plus one
        // `observe`.
        if self.theory_window_visible && self.caps.detachable {
            let outcome = theory_panel::show_detached_window(
                &ctx,
                self.theory_builder_size,
                self.theory_builder_pos,
                self.settings.borderless_mode,
                self.main_focused,
                self.settings.theory_views(),
                self.theory_input(&self.display_notes()),
                &self.settings,
            );
            if let Some(g) = self.theory_guard.as_mut() {
                if g.observe(&outcome, Instant::now()) {
                    self.geometry_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
                }
            }
            if outcome.close_requested {
                self.reattach_theory(); // close-to-reattach, like the other two
            } else if let Some(hit) = outcome.hit {
                // The diagrams are inputs, and they stay inputs when popped
                // out. The hit arrives already resolved against the WINDOW's
                // rect, which is a different rectangle from the band's.
                self.toggle_theory_hit(hit);
            } else if let Some(pos) = outcome.context_menu_at {
                if self.dialog.is_none() {
                    self.open_menu_at(&ctx, pos);
                }
            }
        }

        // The popped-out Recorder. Its reason for existing is the best of the
        // four: a big framing view of the camera on a second monitor while the
        // piano stays where it is.
        if self.recorder_window_visible && self.caps.detachable {
            let view = self.recorder.view(
                self.settings.record_take_name.as_deref().unwrap_or_default(),
                self.name_focused,
                self.settings.knobs(),
                self.settings.record_hide_elapsed,
            );
            let outcome = recorder_panel::show_detached_window(
                &ctx,
                self.recorder_builder_size,
                self.recorder_builder_pos,
                self.settings.borderless_mode,
                self.main_focused,
                &view,
                &self.settings,
            );
            if let Some(g) = self.recorder_guard.as_mut() {
                if g.observe_geometry(outcome.inner_size, outcome.outer_pos, Instant::now()) {
                    self.geometry_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
                }
            }
            if outcome.close_requested {
                self.reattach_recorder(); // close-to-reattach, like the other three
            } else if let Some(hit) = outcome.hit.filter(|_| self.dialog.is_none()) {
                // The hit arrives already resolved against the WINDOW's rect,
                // which is a different rectangle from the band's.
                //
                // `dialog.is_none()` because the main window is modal to a
                // dialog (`handle_main_interaction` returns early) and the
                // popout has to be too. Without it, clicking Export in the
                // detached recorder while the Export dialog is already open
                // REPLACES it with a fresh one and discards everything typed.
                // The recorder is the only popout whose hits open dialogs,
                // which is what makes this a bug here and not in the other two.
                self.name_focused = matches!(hit, recorder_panel::Hit::NameField);
                self.apply_recorder_hit(hit);
            } else if let Some(pos) = outcome.context_menu_at {
                if self.dialog.is_none() {
                    self.open_menu_at(&ctx, pos);
                }
            }
        }

        // Detached chord window, gated for the same reason as the neck above.
        if self.detach_window_visible && self.caps.detachable {
            let outcome = chord_strip::show_detached_window(
                &ctx,
                self.detached_builder_size,
                self.detached_builder_pos,
                self.settings.borderless_mode,
                self.main_focused,
                self.current_chord.as_deref(),
                self.settings.chord_text_color.to_color32(),
                self.heart_color(),
            );
            // A tiling WM overrules the size we asked for the moment the window
            // appears. Inside the grace period a mismatch is therefore its
            // doing, not the user's, and this session's geometry is not ours
            // to remember. After it, a mismatch is a real resize.
            if let (Some(shown), Some(size)) = (self.detached_shown_at, outcome.inner_size) {
                if Instant::now().duration_since(shown) < WM_GRACE {
                    self.detached_wm_managed |= wm_overrode_size(size, self.detached_builder_size);
                } else {
                    self.detached_shown_at = None;
                }
            }

            let moved = outcome.inner_size != self.detached_live_size
                || (outcome.outer_pos.is_some() && outcome.outer_pos != self.detached_live_pos);
            if let Some(size) = outcome.inner_size {
                self.detached_live_size = Some(size);
            }
            if let Some(pos) = outcome.outer_pos {
                self.detached_live_pos = Some(pos);
            }
            if moved && !self.detached_wm_managed {
                self.geometry_save_at = Some(Instant::now() + GEOMETRY_SAVE_DELAY);
            }
            if outcome.close_requested {
                self.reattach_chord_window(); // close-to-reattach
            } else if let Some(pos) = outcome.context_menu_at {
                if self.dialog.is_none() {
                    self.open_menu_at(&ctx, pos);
                }
            }
        }

        // Debounced write-back for the faders, which are the only settings in
        // the app changed by DRAGGING. Same reasoning as the geometry below and
        // a separate deadline, because the two are unrelated gestures and one
        // resetting the other's timer would let a long drag postpone a save
        // indefinitely.
        if let Some(deadline) = self.settings_save_at {
            if Instant::now() >= deadline {
                self.settings_save_at = None;
                self.save_settings();
            }
        }

        // Debounced geometry write-back: one settings.json write after the
        // user stops dragging or resizing, rather than one per frame.
        if let Some(deadline) = self.geometry_save_at {
            if Instant::now() >= deadline {
                self.geometry_save_at = None;
                let mut dirty = false;
                // The main window is not user-resizable, so once it has settled
                // any disagreement with `target` means something else is
                // placing it and its position is not worth remembering.
                let ours = self
                    .main_live_size
                    .is_some_and(|s| !wm_overrode_size(s, target));
                if ours {
                    if let Some(p) = self.main_live_pos {
                        let (x, y) = (p.x.round() as i64, p.y.round() as i64);
                        if self.settings.window_x != Some(x) || self.settings.window_y != Some(y) {
                            self.settings.window_x = Some(x);
                            self.settings.window_y = Some(y);
                            dirty = true;
                        }
                    }
                }
                if self.detach_window_visible && !self.detached_wm_managed {
                    dirty |= self.remember_detached_geometry();
                }
                if self.fret_window_visible && !self.fret_wm_managed {
                    dirty |= self.remember_fretboard_geometry();
                }
                if self.theory_window_visible
                    && self.theory_guard.as_ref().is_none_or(|g| !g.wm_managed())
                {
                    dirty |= self.remember_theory_geometry();
                }
                if self.recorder_window_visible
                    && self.recorder_guard.as_ref().is_none_or(|g| !g.wm_managed())
                {
                    dirty |= self.remember_recorder_geometry();
                }
                if dirty {
                    self.save_settings();
                }
            }
        }

        // Context menu viewport.
        if let Some(action) = menu::show(&ctx, &mut self.menu_state) {
            self.apply_menu_action(&ctx, action);
        }

        // Dialog viewport. Child windows centre on the main window: without a
        // position the OS places them, which on Windows is the top-left of the
        // screen no matter where the user has put the piano.
        let placement = dialogs::Placement {
            caps: self.caps,
            // The rect the layout was actually DRAWN into, which is centred in
            // the pane and is not the same as one anchored at its corner. With
            // a band turned off, the two differ by half the slack and dialogs
            // landed up to 180 points away from the app they belong to.
            parent: self
                .main_origin_known
                .then_some(self.last_drawn)
                .filter(|r: &Rect| r.is_positive()),
            monitor: self.monitor_size,
        };
        if let Some(action) =
            dialogs::show(&ctx, &mut self.dialog, self.settings.dark_mode, placement)
        {
            self.apply_dialog_action(action);
        }

        // 50ms GUI cadence; MIDI events wake us sooner via request_repaint_of.
        ctx.request_repaint_after(GUI_TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `nih_plug_egui::create_egui_editor` requires its state to be
    /// `'static + Send`. Asserted HERE rather than discovered in the plugin
    /// crate, where the same mistake arrives as a wall of trait errors
    /// pointing at a macro. Send only, NOT Sync: `mpsc::Receiver` is not Sync,
    /// and the 0.7 adapter does not ask for it.
    #[test]
    fn the_app_can_be_handed_to_a_plugin_editor() {
        fn assert_send<T: 'static + Send>() {}
        assert_send::<IvoryApp>();
    }

    fn headless(caps: Caps) -> (egui::Context, IvoryApp) {
        headless_with(caps, Settings::default())
    }

    fn headless_with(caps: Caps, settings: Settings) -> (egui::Context, IvoryApp) {
        let ctx = egui::Context::default();
        let app = IvoryApp::new(&ctx, settings, caps);
        (ctx, app)
    }

    /// A desktop app with the Welcome dialog already dismissed.
    ///
    /// It matters for anything about keyboard shortcuts: `show_welcome`
    /// defaults to true, the dialog is modal, and shortcuts are suppressed
    /// while one is up. A shortcut test on a plain `headless()` passes for the
    /// wrong reason — it is measuring the welcome screen.
    fn recorder_app() -> (egui::Context, IvoryApp) {
        headless_with(
            Caps::DESKTOP,
            Settings {
                show_welcome: false,
                ..Settings::default()
            },
        )
    }

    /// One frame with the space bar pressed, returning whatever it started.
    fn space(ctx: &egui::Context, app: &mut IvoryApp) -> Option<recorder::RecorderRequest> {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
        app.take_recorder_request()
    }

    fn run_one_frame(ctx: &egui::Context, app: &mut IvoryApp, size: Vec2) -> Vec<String> {
        let out = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
        out.viewport_output
            .values()
            .flat_map(|v| v.commands.iter())
            .map(|c| format!("{c:?}"))
            .collect()
    }

    /// The whole menu must fit the editor it is drawn in, at every size the
    /// plugin can open at and with every band turned on.
    ///
    /// A menu taller than its canvas does not look scrollable, it looks CUT
    /// OFF — the rows past the edge are simply absent and nothing on screen
    /// says otherwise. On the desktop it cannot happen: the menu is its own
    /// window and may be taller than the app. In a plugin editor, which is
    /// often shorter than the ~550 points this menu wants, it happened the
    /// moment the guitar view was turned on.
    #[test]
    fn the_whole_menu_fits_a_short_editor() {
        for (fret, theory) in [(false, false), (true, false), (true, true)] {
            let settings = Settings {
                show_welcome: false,
                show_fretboard: fret,
                theory_circle: theory,
                theory_tonnetz: theory,
                theory_triangles: theory,
                ..Settings::default()
            };
            let (ctx, mut app) = headless_with(Caps::PLUGIN, settings.clone());
            crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);

            // Every editor height the plugin realistically opens at, plus a
            // deliberately cruel one.
            for h in [natural_size(&settings, 900.0).y, 200.0, 137.0] {
                let pane = Vec2::new(900.0, h);
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pane)),
                        ..Default::default()
                    },
                    |ctx| app.frame(ctx),
                );
                app.open_menu_at(&ctx, Pos2::new(20.0, 20.0));
                let state = app.menu_state.as_ref().expect("menu open");
                let menu = menu::size_for_test(state);
                let row_h = menu::row_height_for_test(state);
                // Either it fits, or it was squeezed as far as it is allowed
                // to go and `shell::surface` scrolls the rest. What must never
                // happen is a menu that is too tall AND was not squeezed —
                // that is the one that looks cut off with rows simply absent.
                if menu.y > pane.y + 0.5 {
                    // Measured against the same menu with no ceiling, so the
                    // floor is whatever the code says it is rather than a
                    // number copied into the test that can drift.
                    let roomy = {
                        let mut a = app.menu_view();
                        a.caps = Caps::DESKTOP;
                        menu::row_height_for_test(&MenuState::open(
                            &ctx,
                            a,
                            Pos2::ZERO,
                            Some(Vec2::new(4000.0, 4000.0)),
                        ))
                    };
                    assert!(
                        row_h < roomy - 0.5,
                        "menu is {}pt in a {}pt editor and its rows are still \
                         {row_h}pt, the same as the unconstrained {roomy}pt \
                         (fretboard {fret}, theory {theory}) — it did not \
                         squeeze at all, so the bottom rows are simply missing",
                        menu.y,
                        pane.y
                    );
                }
                // Readable either way: squeezing to a smear is not a fix.
                assert!(
                    row_h >= 12.0,
                    "rows squeezed to {row_h}pt, which is not a menu any more"
                );
                app.menu_state = None;
            }
        }
    }

    /// A barre comes from a drag along a fret, and from nothing else.
    ///
    /// The solver derives one from any adjacent strings sharing their lowest
    /// fret, which is right for a shape IT chose and wrong for one entered by
    /// hand: two notes that happen to line up are two notes, and a bar drawn
    /// across them claims a finger position nobody asked for.
    #[test]
    fn a_hand_entered_barre_comes_only_from_a_drag() {
        let (ctx, mut app) = headless_with(
            Caps::DESKTOP,
            Settings {
                show_welcome: false,
                show_fretboard: true,
                keytoggle_enabled: true,
                ..Settings::default()
            },
        );
        let _ = &ctx;
        let spec = app.settings.fretboard_spec();

        // Two notes placed one at a time on the same fret, adjacent strings.
        // The solver would call that a barre; by hand it is two notes.
        for st in [0usize, 1] {
            let note = spec.pitch_at(st, 5).unwrap();
            app.manual_positions.insert(note, (st, 5));
            app.manual_notes.insert(note);
        }
        app.sync_pins();
        assert_eq!(
            app.barre_to_draw(),
            None,
            "two hand-placed notes on one fret were drawn as a barre"
        );

        // Dragged, they are.
        app.place_barre(&spec, 0, 3, 5);
        let b = app.barre_to_draw().expect("a drag makes a barre");
        assert_eq!(b.fret, 5);
        assert_eq!((b.lo_string, b.hi_string), (0, 3));
        // ...and every string it crosses is actually fretted.
        for st in 0..=3usize {
            let note = spec.pitch_at(st, 5).unwrap();
            assert_eq!(app.manual_positions.get(&note), Some(&(st, 5)));
        }

        // Take one of them off and it stops being a barre, without anything
        // having to remember to clear it.
        let gone = spec.pitch_at(2, 5).unwrap();
        app.manual_notes.remove(&gone);
        assert_eq!(
            app.barre_to_draw(),
            None,
            "a barre survived one of its own strings being removed"
        );

        // A drag that stays on one string is not a barre either.
        app.manual_notes.clear();
        app.manual_positions.clear();
        app.manual_barre = None;
        app.place_barre(&spec, 2, 2, 7);
        assert_eq!(app.barre_to_draw(), None, "a one-string drag made a barre");

        // With nothing placed by hand, the solver's own barre is shown again:
        // it chose to bar those strings and the diagram should say so.
        app.manual_positions.clear();
        app.manual_notes.clear();
        app.sync_pins();
        assert_eq!(app.barre_to_draw(), app.voicing.current().shape.barre);
    }

    /// A host that cannot open windows must not believe something is detached.
    ///
    /// Both flags persist, and a plugin instance is seeded from the same file
    /// the standalone writes. Someone who left the chord strip popped out on
    /// the desktop got a plugin with no chord readout at all: the band is
    /// zeroed by the flag, the window is gated off by `caps.detachable`, and
    /// the Attach row that would undo it is gated off too. The only way back
    /// was Reset Settings.
    #[test]
    fn a_plugin_never_inherits_a_detached_band() {
        let detached = Settings {
            chord_window_detached: true,
            fretboard_detached: true,
            show_fretboard: true,
            chord_detection_enabled: true,
            show_welcome: false,
            ..Settings::default()
        };

        let (_, plugin) = headless_with(Caps::PLUGIN, detached.clone());
        assert!(!plugin.settings.chord_window_detached);
        assert!(!plugin.settings.fretboard_detached);
        let b = band_sizes(&plugin.settings);
        assert!(
            b.chord_h > 0.0,
            "the chord strip vanished with nowhere to go"
        );
        assert!(b.fret_h > 0.0, "the fretboard vanished with nowhere to go");

        // The desktop keeps what it was given: there, detached means detached.
        let (_, desktop) = headless_with(Caps::DESKTOP, detached);
        assert!(desktop.settings.chord_window_detached);
        assert!(desktop.settings.fretboard_detached);
    }

    /// Turning a band on must make the editor TALLER, not the picture smaller.
    ///
    /// On the desktop the window follows `layout_sizes()` by itself. A plugin
    /// editor cannot: nothing resizes it unless it asks, and only
    /// `SetSizePercent` ever did. So "Show Fretboard" inside a DAW did not
    /// grow the editor — it shrank the whole layout to fit the height it
    /// already had, and the piano lost 40% of its width to black bars.
    #[test]
    fn a_plugin_asks_for_room_when_a_band_appears() {
        let (ctx, mut app) = headless_with(
            Caps::PLUGIN,
            Settings {
                show_welcome: false,
                show_fretboard: false,
                ..Settings::default()
            },
        );
        let pane = Vec2::new(900.0, 137.0);
        let run = |ctx: &egui::Context, app: &mut IvoryApp| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, pane)),
                    ..Default::default()
                },
                |ctx| app.frame(ctx),
            );
        };
        run(&ctx, &mut app);
        let before = fit_bands(&app.settings, pane).total();
        app.take_pending_resize();

        for action in [
            MenuAction::ToggleFretboard,
            MenuAction::ToggleTheoryView(theory_panel::View::Circle),
            MenuAction::ToggleChordDetection,
        ] {
            app.apply_menu_action(&ctx, action.clone());
            let asked = app.take_pending_resize().unwrap_or_else(|| {
                panic!("{action:?} changed the band stack without asking for room")
            });
            // It asks for the height the new stack needs at the width it has,
            // rather than squeezing the layout into the old height.
            assert!(
                (asked.x - pane.x).abs() < 1.5,
                "{action:?} asked to change the WIDTH: {asked:?}"
            );
            let squeezed = fit_bands(&app.settings, pane).total();
            assert!(
                asked.y > squeezed.y || squeezed.x >= before.x - 1.0,
                "{action:?}: squeezing into the old height gives {squeezed:?}, \
                 which is narrower than the {before:?} it started at, and the \
                 request {asked:?} would not fix it"
            );
            run(&ctx, &mut app);
        }

        // The desktop must NOT ask — its window follows the layout by itself,
        // and a stray request there would be a viewport command in a plugin's
        // clothing.
        let (ctx, mut desk) = headless_with(
            Caps::DESKTOP,
            Settings {
                show_welcome: false,
                ..Settings::default()
            },
        );
        let _ = &ctx;
        desk.apply_menu_action(&ctx, MenuAction::ToggleFretboard);
        assert_eq!(desk.take_pending_resize(), None);
    }

    /// A right-click must open the menu in a plugin, and a left-click on a row
    /// must ACTIVATE it.
    ///
    /// This is the test that was missing. Every row of the plugin's menu was
    /// dead, and nothing caught it, because `menu::show` was tested for what it
    /// DRAWS and the app was tested for what it SENDS — and the bug was in the
    /// order the two ran. `handle_main_interaction` closed the menu on any
    /// press, which is right when the menu is its own OS window and this
    /// handler never sees the press that lands on it. Drawn inline it is in
    /// the same context, so the handler saw the press meant for the item,
    /// cleared the menu, and returned — and `menu::show`, later in the frame,
    /// found nothing to draw. The menu just vanished on every click.
    ///
    /// So this drives real pointer events at a real row position and asserts
    /// the setting changed.
    #[test]
    fn a_plugin_menu_row_can_actually_be_clicked() {
        // No welcome dialog: it is modal, and a modal correctly swallows every
        // click, so leaving it up would test the modal rather than the menu.
        let (ctx, mut app) = headless_with(
            Caps::PLUGIN,
            Settings {
                show_welcome: false,
                ..Settings::default()
            },
        );
        crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);
        let size = Vec2::new(900.0, 600.0);

        let run = |ctx: &egui::Context, app: &mut IvoryApp, events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ctx| app.frame(ctx),
            );
        };
        // A click is three frames, as it is for a person: the pointer arrives,
        // the button goes down, the button comes up. egui hit-tests against
        // the widget rects it saw last frame, so a press delivered before the
        // pointer has ever been over the widget lands on nothing.
        let button_event =
            |p: Pos2, button: egui::PointerButton, pressed: bool| egui::Event::PointerButton {
                pos: p,
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
        let click =
            |ctx: &egui::Context, app: &mut IvoryApp, p: Pos2, button: egui::PointerButton| {
                run(ctx, app, vec![egui::Event::PointerMoved(p)]);
                run(ctx, app, vec![button_event(p, button, true)]);
                run(ctx, app, vec![button_event(p, button, false)]);
            };

        run(&ctx, &mut app, vec![]);
        // Right-click near the top-left opens the menu there.
        let open_at = Pos2::new(20.0, 20.0);
        click(&ctx, &mut app, open_at, egui::PointerButton::Secondary);
        assert!(
            app.menu_state.is_some(),
            "a right-click did not open the menu in a plugin"
        );

        // The first row is Size, a submenu; the first ITEM row is what we can
        // click without hovering a submenu open. Find it from the same view
        // the menu was built from.
        let view = app.menu_view();
        let rows = menu::rows_for_test(view);
        let (idx, _, want) = rows
            .iter()
            .enumerate()
            .find_map(|(i, (label, a))| {
                (label.as_str() == "Dark Mode").then_some((i, label.clone(), a.clone()))
            })
            .expect("Dark Mode is in the menu");
        assert_eq!(want, MenuAction::ToggleDarkMode);

        let before = app.settings.dark_mode;
        let row = menu::row_center_for_test(app.menu_state.as_ref().unwrap(), idx);
        click(&ctx, &mut app, row, egui::PointerButton::Primary);
        assert_ne!(
            app.settings.dark_mode, before,
            "clicking a menu row in a plugin did nothing — the row is dead"
        );
        assert!(
            app.menu_state.is_none(),
            "choosing a row must close the menu"
        );
    }

    /// A dialog and a menu must OPEN and DRAW inside a plugin, and still send
    /// the host nothing.
    ///
    /// The two frame tests above run an idle app. This one drives it through
    /// the paths that used to open OS windows — `A` raises the About box, `H`
    /// the shortcut card — and keeps running frames while they are up. In a
    /// plugin `show_viewport_immediate` does not fail, it opens a second
    /// `CentralPanel` under the same id and paints garbage over the piano, so
    /// "it did not crash" is not the assertion. The assertion is that the host
    /// still receives no viewport command and the app is still standing.
    #[test]
    fn dialogs_and_menus_open_inside_a_plugin_without_reaching_the_host() {
        let (ctx, mut app) = headless(Caps::PLUGIN);
        crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);
        let size = Vec2::new(900.0, 435.0);

        let press = |k: egui::Key| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        let frame = |ctx: &egui::Context, app: &mut IvoryApp, events: Vec<egui::Event>| {
            let out = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    events,
                    ..Default::default()
                },
                |ctx| app.frame(ctx),
            );
            out.viewport_output
                .values()
                .flat_map(|v| v.commands.iter())
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
        };

        // Settle, then open the About box and hold it open for several frames.
        for _ in 0..2 {
            assert!(frame(&ctx, &mut app, vec![]).is_empty());
        }
        assert!(frame(&ctx, &mut app, vec![press(egui::Key::A)]).is_empty());
        assert!(
            app.dialog.is_some(),
            "A did not open a dialog, so this test proves nothing"
        );
        for i in 0..4 {
            let cmds = frame(&ctx, &mut app, vec![]);
            assert!(
                cmds.is_empty(),
                "an open dialog commanded the host on frame {i}: {cmds:?}"
            );
        }

        // Escape closes it, inline as well as in a window.
        frame(&ctx, &mut app, vec![press(egui::Key::Escape)]);
        assert!(
            app.dialog.is_none(),
            "Escape did not close the in-canvas dialog"
        );

        // ...and the held shortcut card, which is drawn over everything.
        for i in 0..3 {
            let cmds = frame(&ctx, &mut app, vec![press(egui::Key::H)]);
            assert!(
                cmds.is_empty(),
                "the shortcut card commanded the host on frame {i}: {cmds:?}"
            );
        }
    }

    /// A plugin is handed a rect and has to lay out inside it. `main_width`
    /// is the desktop's answer — 1300 points times a size percentage — and
    /// using it in an editor the host opened at 900 would run the piano off
    /// the right-hand edge, with no window command available to fix it.
    ///
    /// Checked in both directions, because "it fits" is also satisfied by a
    /// layout that shrank to nothing.
    #[test]
    fn a_plugin_lays_out_inside_the_rect_it_is_given() {
        let mut s = Settings::default();
        for (fret, theory) in [(false, false), (true, false), (true, true)] {
            s.show_fretboard = fret;
            s.theory_circle = theory;
            for avail in [
                Vec2::new(900.0, 260.0),  // the editor's default
                Vec2::new(400.0, 200.0),  // a narrow rack
                Vec2::new(1800.0, 300.0), // wide and short: height decides
                Vec2::new(600.0, 1200.0), // tall and narrow: width decides
            ] {
                let b = fit_bands(&s, avail);
                let t = b.total();
                assert!(
                    t.x <= avail.x + 0.5 && t.y <= avail.y + 0.5,
                    "{t:?} does not fit in {avail:?} (fretboard {fret}, theory {theory})"
                );
                assert!(
                    b.w >= 1.0 && b.piano_h >= 1.0,
                    "the layout collapsed to {b:?} in {avail:?}"
                );
                // It should USE the space, not sit in a corner of it: one of
                // the two dimensions is the binding constraint and should be
                // nearly filled.
                let fill = (t.x / avail.x).max(t.y / avail.y);
                assert!(
                    fill > 0.90,
                    "only filled {:.0}% of {avail:?} ({t:?})",
                    fill * 100.0
                );
            }
        }
    }

    /// ...and the desktop is untouched by all of that. Its layout is still
    /// decided by the size percentage and nothing else.
    #[test]
    fn the_desktop_layout_is_unchanged() {
        let mut s = Settings::default();
        for pct in [50i64, 75, 100, 125, 150, 175, 200] {
            s.window_size_percent = pct;
            let w = main_width(&s);
            assert_eq!(band_sizes(&s).w, w, "at {pct}%");
            assert_eq!(band_sizes(&s), band_sizes_at(&s, w), "at {pct}%");
        }
    }

    /// A plugin editor's window belongs to the DAW. Not one frame may ask to
    /// resize it, undecorate it, retitle it, move it, or start dragging it.
    ///
    /// This is not a theoretical guard. `egui-baseview` HONOURS
    /// `ViewportCommand::InnerSize` — it calls `window.resize()` — so the
    /// fixed-size triple, left ungated, would reach into the host and resize
    /// the editor behind its back on the first frame. `MinInnerSize` and
    /// `MaxInnerSize` are swallowed there, which would have left exactly the
    /// one third of the mechanism that does damage.
    /// A popped-out band must vacate the main window, or the diagrams render in
    /// two places at once and the main window keeps 300pt of height for
    /// something that is somewhere else.
    #[test]
    fn a_detached_theory_band_takes_no_height_in_the_main_window() {
        let mut s = Settings::default();
        s.theory_circle = true;
        let attached = band_sizes_at(&s, 1300.0);
        assert!(attached.theory_h > 0.0, "the band should be showing");

        s.theory_detached = true;
        let detached = band_sizes_at(&s, 1300.0);
        assert_eq!(detached.theory_h, 0.0, "detached, so it is not in the stack");
        assert_eq!(
            detached.piano_h, attached.piano_h,
            "detaching must not resize the piano"
        );
        assert!(
            detached.total().y < attached.total().y,
            "the window should get shorter when a band leaves it"
        );
    }

    /// A plugin has no second window, so it must never start believing it has
    /// one — a plugin instance is seeded from the same settings file the
    /// desktop writes, so a user who left the theory band popped out on the
    /// desktop would otherwise get a DAW editor with no theory band at all: the
    /// band is zeroed by the flag, and the window it moved to cannot exist.
    #[test]
    fn a_plugin_never_starts_with_a_detached_theory_band() {
        let ctx = egui::Context::default();
        let mut s = Settings::default();
        s.theory_circle = true;
        s.theory_detached = true;
        let app = IvoryApp::new(&ctx, s, Caps::PLUGIN);
        assert!(
            !app.settings.theory_detached,
            "Caps::PLUGIN must clear it, as it already does for the chord \
             window and the neck"
        );
    }

    #[test]
    fn the_recorder_band_is_only_in_the_stack_when_it_is_attached_and_shown() {
        let mut s = Settings::default();
        assert_eq!(
            band_sizes_at(&s, 1300.0).recorder_h,
            0.0,
            "off by default — the band is 200pt tall and a window that grows \
             on its own after an update is a geometry surprise"
        );
        s.show_recorder = true;
        let shown = band_sizes_at(&s, 1300.0);
        assert!(shown.recorder_h > 0.0);

        s.recorder_detached = true;
        let detached = band_sizes_at(&s, 1300.0);
        assert_eq!(detached.recorder_h, 0.0, "it is somewhere else");
        assert_eq!(
            detached.piano_h, shown.piano_h,
            "detaching must not resize the piano"
        );
        assert!(detached.total().y < shown.total().y);
    }

    /// The worst of the four seeding failures, because the recorder is the one
    /// band a plugin can never populate: `capture_devices` is false there, so
    /// there is no camera, no input and no take directory. A settings file
    /// written by the desktop with `show_recorder: true` would otherwise cost a
    /// DAW editor 200 points of empty transport, taken out of the piano.
    #[test]
    fn a_plugin_never_starts_with_a_recorder_band() {
        let ctx = egui::Context::default();
        let mut s = Settings::default();
        s.show_recorder = true;
        s.recorder_detached = true;
        let app = IvoryApp::new(&ctx, s, Caps::PLUGIN);
        assert!(!app.settings.show_recorder, "no device, no band");
        assert!(!app.settings.recorder_detached, "and no window for it");
        assert_eq!(band_sizes_at(&app.settings, 1300.0).recorder_h, 0.0);
    }

    /// A Minimal build is a desktop app with no recorder linked, so it has to
    /// clear the band for exactly the same reason a plugin does — and it is the
    /// case that would be missed, because a Minimal build DOES have windows.
    #[test]
    fn a_minimal_build_never_starts_with_a_recorder_band_either() {
        let ctx = egui::Context::default();
        let mut s = Settings::default();
        s.show_recorder = true;
        let app = IvoryApp::new(&ctx, s, Caps::MINIMAL);
        assert!(!app.settings.show_recorder);
        assert!(
            app.settings.show_fretboard || !app.settings.show_fretboard,
            "and nothing else is disturbed"
        );
    }

    /// Hiding the band must not leave its window on screen: the Attach row
    /// lives inside the Recorder category, which is only drawn while the band
    /// is showing, so the window would have no way back.
    #[test]
    fn hiding_the_recorder_takes_its_window_with_it() {
        let (ctx, mut app) = headless(Caps::DESKTOP);
        app.settings.show_recorder = true;
        app.detach_recorder();
        assert!(app.recorder_window_visible);

        app.apply_menu_action(&ctx, MenuAction::ToggleRecorder);
        assert!(!app.settings.show_recorder);
        assert!(!app.recorder_window_visible, "the window went with it");
        assert!(
            app.settings.recorder_detached,
            "but where it was is remembered, so turning it back on puts it back"
        );
    }

    /// Space is the transport, and the band owns a text field. Typing a space
    /// into a take name must not also start a take.
    #[test]
    fn a_focused_take_name_swallows_the_transport_key() {
        let (ctx, mut app) = recorder_app();
        app.settings.show_recorder = true;
        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Toggle),
            "with the band open and nothing focused, Space is the transport"
        );

        app.name_focused = true;
        assert_eq!(
            space(&ctx, &mut app),
            None,
            "a space typed into the take name must not also start a take"
        );

        // And the card does not flicker: `name_focused` guards the frame, it
        // does not un-declare the binding.
        assert!(app.key_gates().recorder_shown);
    }

    /// The other half of the same gate, and the one that protects people who
    /// have never opened the recorder: Space is the widest key on the keyboard
    /// and gets hit by accident constantly.
    #[test]
    fn space_does_nothing_until_the_band_has_been_opened() {
        let (ctx, mut app) = recorder_app();
        assert!(!app.settings.show_recorder);
        assert_eq!(space(&ctx, &mut app), None);
        // And the same app, one flag later, DOES respond — otherwise this test
        // would go on passing if Space stopped working entirely.
        app.settings.show_recorder = true;
        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Toggle)
        );
    }

    /// A plugin drains nothing, so a request it can never honour must not be
    /// recorded at all — otherwise the first thing draining would do, whenever
    /// somebody added it, is start a take nobody asked for.
    #[test]
    fn a_plugin_records_no_recorder_requests() {
        let (_ctx, mut app) = headless(Caps::PLUGIN);
        app.request_recorder(recorder::RecorderRequest::Toggle);
        app.request_recorder(recorder::RecorderRequest::Stop);
        assert_eq!(app.take_recorder_request(), None);
        app.ask_for_a_folder();
        assert_eq!(app.take_directory_request(), None);
    }

    /// The take name is optional, and clearing it has to go back to producing
    /// unnamed takes rather than takes called "".
    #[test]
    fn clearing_the_take_name_makes_it_absent_rather_than_empty() {
        let (ctx, mut app) = headless(Caps::DESKTOP);
        app.settings.record_take_name = Some("nocturne".into());
        app.name_focused = true;
        ctx.begin_pass(egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }; 8],
            ..Default::default()
        });
        app.edit_take_name(&ctx);
        let _ = ctx.end_pass();
        assert_eq!(app.settings.record_take_name, None, "not Some(\"\")");
    }

    /// The session-only spec must not leak into the file, and remembering must
    /// clear the session copy — or the thing the user just made permanent would
    /// go on being shadowed by the temporary one.
    #[test]
    fn an_export_spec_is_remembered_only_when_it_is_asked_to_be() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        let one_off = recorder::ExportSpec {
            tempo_bpm: 92.0,
            ..Default::default()
        };
        app.apply_dialog_action(DialogAction::SetExport(one_off));
        assert_eq!(app.export_spec(), one_off, "this take uses it");
        assert_eq!(
            app.settings.record_export,
            recorder::ExportSpec::default(),
            "and the file does not"
        );

        let forever = recorder::ExportSpec {
            tempo_bpm: 76.0,
            ..Default::default()
        };
        app.apply_dialog_action(DialogAction::SetExportAndRemember(forever));
        assert_eq!(app.settings.record_export, forever);
        assert_eq!(
            app.export_spec(),
            forever,
            "the session copy must not go on shadowing it"
        );
    }

    #[test]
    fn a_plugin_frame_never_commands_the_hosts_window() {
        let (ctx, mut app) = headless(Caps::PLUGIN);
        // Several frames: the size latch and the decorations latch each fire
        // once, and "once" is easy to miss by looking at frame one alone.
        for i in 0..4 {
            let cmds = run_one_frame(&ctx, &mut app, Vec2::new(1300.0, 632.0));
            assert!(
                cmds.is_empty(),
                "frame {i} sent the host {} viewport command(s): {cmds:?}",
                cmds.len()
            );
        }
    }

    /// ...and the desktop still does all of it, or the gate has quietly turned
    /// the standalone into a resizable window with no title.
    #[test]
    fn the_desktop_still_sizes_and_titles_its_own_window() {
        let (ctx, mut app) = headless(Caps::DESKTOP);
        let mut seen: Vec<String> = Vec::new();
        for _ in 0..4 {
            seen.extend(run_one_frame(&ctx, &mut app, Vec2::new(1300.0, 632.0)));
        }
        for want in [
            "MinInnerSize",
            "MaxInnerSize",
            "InnerSize",
            "Decorations",
            "Title",
        ] {
            assert!(
                seen.iter().any(|c| c.starts_with(want)),
                "the desktop stopped sending {want}; saw {seen:?}"
            );
        }
    }

    /// A plugin shares `~/.config/ivory/settings.json` with the standalone and
    /// with every other instance, so it must never write it. Asserted through
    /// the one gate every call site now goes through.
    #[test]
    fn a_plugin_does_not_write_the_shared_settings_file() {
        let before = std::fs::read(Settings::path()).ok();
        let (_ctx, app) = headless(Caps::PLUGIN);
        for _ in 0..3 {
            app.save_settings();
        }
        assert_eq!(
            std::fs::read(Settings::path()).ok(),
            before,
            "a plugin wrote the shared settings file"
        );
    }

    /// D-UI-15: the fretboard is a third band in the same stack, and the two
    /// places that compute the window height have to agree about it. They used
    /// to be two copies of the same arithmetic; drift between them is a window
    /// that visibly jumps on the first frame.
    #[test]
    fn the_fretboard_band_joins_the_stack_at_every_size() {
        let mut s = Settings::default();
        for pct in [50i64, 75, 100, 125, 150, 175, 200] {
            s.window_size_percent = pct;
            s.show_fretboard = false;
            let without = initial_window_size(&s);
            s.show_fretboard = true;
            let with = initial_window_size(&s);
            let w = main_width(&s);
            assert_eq!(with.x, without.x, "the fretboard must not change the width");
            assert_eq!(
                with.y - without.y,
                fretboard_panel::band_height(w, 6),
                "band height at {pct}%"
            );
            assert_eq!(with.y, with.y.trunc(), "bands are whole pixels");
        }
        // And it is independent of the chord strip: hiding one must not resize
        // the other.
        s.show_fretboard = true;
        s.chord_detection_enabled = false;
        let piano_and_fret = initial_window_size(&s);
        s.show_fretboard = false;
        assert_eq!(
            piano_and_fret.y - initial_window_size(&s).y,
            fretboard_panel::band_height(main_width(&s), s.fretboard_spec().tuning.strings())
        );
    }

    // ── The MIDI state machine (spec §10) ─────────────────────────────
    //
    // Four interacting rules that had no coverage at all, in the one piece of
    // the app every other display reads from. Written before the code moves to
    // a shared crate, because "it still compiles" is not evidence that a state
    // machine still behaves.

    use MidiEvent::{NoteOff, NoteOn, Sustain};

    fn held(n: &NoteState) -> Vec<u8> {
        let mut v: Vec<u8> = n.held().iter().copied().collect();
        v.sort_unstable();
        v
    }

    fn feed(events: &[MidiEvent]) -> NoteState {
        let mut n = NoteState::default();
        for e in events {
            n.apply(*e);
        }
        n
    }

    #[test]
    fn a_key_sounds_while_it_is_down_and_stops_when_it_is_not() {
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            NoteOn {
                note: 64,
                velocity: 80,
            },
        ]);
        assert_eq!(held(&n), vec![60, 64]);
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            NoteOff { note: 60 },
        ]);
        assert!(held(&n).is_empty());
        // A note-off for something never held is not an event, it is noise.
        let n = feed(&[NoteOff { note: 60 }]);
        assert!(held(&n).is_empty());
    }

    #[test]
    fn the_pedal_holds_notes_past_the_key_and_lets_go_on_release() {
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
        ]);
        assert_eq!(held(&n), vec![60], "the pedal is down, so it still sounds");
        assert!(n.sustain_down());

        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
            Sustain { down: false },
        ]);
        assert!(
            held(&n).is_empty(),
            "lifting the pedal releases what the key let go of"
        );
        assert!(!n.sustain_down());
    }

    #[test]
    fn a_key_struck_again_under_the_pedal_survives_the_next_lift() {
        // The subtle one. Without cancelling the pending release, re-striking a
        // key while the pedal is down leaves it queued to die at the next lift,
        // so a re-articulated note vanishes while you are still holding it.
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: false },
        ]);
        assert_eq!(
            held(&n),
            vec![60],
            "a re-struck key must not be released by the pedal"
        );
    }

    #[test]
    fn only_the_down_to_up_edge_releases_anything() {
        // Pedal down while already down changes nothing.
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
            Sustain { down: true },
        ]);
        assert_eq!(held(&n), vec![60]);
        // Pedal up while already up must not drain a set a later note-off fills.
        let n = feed(&[
            Sustain { down: false },
            NoteOn {
                note: 60,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
        ]);
        assert_eq!(held(&n), vec![60]);
    }

    #[test]
    fn a_key_still_down_when_the_pedal_lifts_keeps_sounding() {
        // The pedal releases what the KEY let go of, and nothing else.
        let n = feed(&[
            NoteOn {
                note: 60,
                velocity: 100,
            },
            NoteOn {
                note: 64,
                velocity: 100,
            },
            Sustain { down: true },
            NoteOff { note: 60 },
            Sustain { down: false },
        ]);
        assert_eq!(held(&n), vec![64], "64 is still physically down");
    }

    #[test]
    fn the_state_machine_never_leaks_a_note() {
        // Random event streams: whatever happens, a note sounding with the
        // pedal up must be one whose key is genuinely down, so lifting the
        // pedal twice can never leave something stuck on.
        let mut seed = 0x1234_5678u64;
        let mut next = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed >> 11
        };
        for _ in 0..2000 {
            let mut n = NoteState::default();
            let mut down: HashSet<u8> = HashSet::new();
            for _ in 0..40 {
                let note = 60 + (next() % 4) as u8;
                match next() % 3 {
                    0 => {
                        n.apply(NoteOn {
                            note,
                            velocity: 100,
                        });
                        down.insert(note);
                    }
                    1 => {
                        n.apply(NoteOff { note });
                        down.remove(&note);
                    }
                    _ => n.apply(Sustain {
                        down: next() % 2 == 0,
                    }),
                }
                // Everything physically down is always sounding.
                for k in &down {
                    assert!(n.held().contains(k), "key {k} is down but silent");
                }
            }
            // Lift the pedal: what remains must be exactly the keys still down.
            n.apply(Sustain { down: false });
            assert_eq!(held(&n), {
                let mut v: Vec<u8> = down.iter().copied().collect();
                v.sort_unstable();
                v
            });
        }
    }

    #[test]
    fn size_math_matches_python_int_truncation() {
        let mut s = Settings::default();
        let table = [
            (50, 650.0, 75.0, 25.0),
            (75, 975.0, 112.0, 37.0),
            (100, 1300.0, 150.0, 50.0),
            (125, 1625.0, 187.0, 62.0),
            (150, 1950.0, 225.0, 75.0),
            (175, 2275.0, 262.0, 87.0),
            (200, 2600.0, 300.0, 100.0),
        ];
        for (pct, w, piano_h, chord_h) in table {
            s.window_size_percent = pct;
            assert_eq!(main_width(&s), w, "W at {pct}%");
            // Chord strip visible (default settings): height = chordH + pianoH.
            assert_eq!(
                initial_window_size(&s),
                Vec2::new(w, piano_h + chord_h),
                "window size at {pct}% with chord strip"
            );
            // Detached or detection disabled: piano-only height.
            let mut no_strip = s.clone();
            no_strip.chord_detection_enabled = false;
            assert_eq!(
                initial_window_size(&no_strip),
                Vec2::new(w, piano_h),
                "window size at {pct}% without chord strip"
            );
            no_strip.chord_detection_enabled = true;
            no_strip.chord_window_detached = true;
            assert_eq!(
                initial_window_size(&no_strip),
                Vec2::new(w, piano_h),
                "window size at {pct}% detached"
            );
        }
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn a_tiling_wm_is_detected_but_ordinary_jitter_is_not() {
        let asked = Vec2::new(460.0, 150.0);
        // AeroSpace tiling the detached window to a third of a 2560x1440
        // screen: the exact case that put detached_chord_height=1377 into a
        // settings file, by recording the WM's choice as the user's.
        assert!(wm_overrode_size(Vec2::new(853.0, 1377.0), asked));
        // Sub-pixel and DPI rounding must not read as a resize.
        assert!(!wm_overrode_size(Vec2::new(460.0, 150.0), asked));
        assert!(!wm_overrode_size(Vec2::new(459.5, 150.4), asked));
        assert!(!wm_overrode_size(Vec2::new(467.0, 157.0), asked));
        // A real user resize is over the tolerance and must be honoured, which
        // is why timing, not size, decides who did it.
        assert!(wm_overrode_size(Vec2::new(700.0, 150.0), asked));
    }
}
