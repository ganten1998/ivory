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
use crate::staff;
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
    /// Where the theory band was last drawn, or `NOTHING` when it was not.
    ///
    /// Recorded rather than recomputed, for the same reason `last_band` is: a
    /// caller reasoning about a gesture must not restate a layout.
    last_theory: Rect,
    /// Where the recorder band was last drawn, or `NOTHING` when it was not.
    ///
    /// Recorded rather than recomputed: the band's position depends on which
    /// other bands are showing and on the 16:9 fit, and a caller that worked
    /// it out again would be a second layout to keep in step with the first.
    last_band: Rect,
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
    /// The detector's next-best distinct names, best first, for the staff
    /// panel's readout. Empty whenever the staff is not on screen — the ranked
    /// pass costs a string per scored pattern, and nothing else reads it.
    chord_alternates: Vec<String>,
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
    /// What the band has asked the host for, oldest first.
    ///
    /// **A queue and not a slot.** One gesture can be two requests: latching a
    /// chord on the keyboard while another is already latched sends the
    /// note-offs and the note-ons in the same frame, and a slot would silently
    /// drop whichever came first. The host already drained in a loop.
    recorder_request: std::collections::VecDeque<recorder::RecorderRequest>,
    /// A folder the host has been asked to choose. Drained after the frame so
    /// the native panel's nested run loop never starts inside an egui frame.
    dir_request: Option<crate::ports::DirRequest>,
    file_request: Option<crate::ports::FileRequest>,
    /// Patch names of the cartridge the host has loaded, for the picker. The
    /// app never reads a `.syx`: this is pushed in after the host parses one.
    cartridge: crate::ports::CartridgeInfo,
    /// The host has been asked to look for plugins again.
    plugin_rescan: bool,
    /// A folder the app has asked the host to show in the file manager.
    ///
    /// The request pattern again, and for a sharper reason than the picker's:
    /// showing a folder means starting another process, which a plugin inside
    /// somebody's DAW has no business doing. A host refuses by never draining
    /// it, and `Caps` decides whether it is ever set in the first place.
    reveal_request: Option<std::path::PathBuf>,
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
    grabbed: Option<Grab>,
    /// Whether the window has been told it may be fullscreen.
    ///
    /// The size pin is Min == Max, and a window whose minimum equals its
    /// maximum cannot become the size of the screen — so the constraints have
    /// to be lifted on the way in and re-applied on the way out, once each
    /// rather than every frame.
    fullscreen_sent: Option<bool>,
    /// What both audio streams are doing, filled by the host each frame.
    ///
    /// Kept on the app rather than fetched when the panel opens, because
    /// `ivory-ui` cannot see a device and the panel has to stay live while it
    /// is up — a rate that changed under you is exactly what it is for.
    audio_status: recorder::AudioStatus,
    /// The last right-click landed on the Recorder band, so its category leads
    /// the menu. Read once when the menu is built and then irrelevant.
    menu_over_recorder: bool,
    /// The last right-click landed on the sheet music panel.
    menu_over_staff: bool,
    /// The Setup button was pressed; the menu opens after the frame, where the
    /// window origin needed to place it is known.
    setup_open: bool,
    /// The effect panel a right-click on a knob opened, if any.
    ///
    /// One at a time, like `dialog`: three panels at once over a band this
    /// small is three panels covering each other.
    fx_open: Option<recorder_panel::Fx>,
    /// The backing track's waveform panel, and which handle a hand is on.
    track_open: bool,
    track_drag: Option<bool>,
    /// The row of that panel a drag is on, once one has started. `&'static
    /// str` because it is the settings KEY — the same thing the panel reports
    /// and the host reads back, so a drag cannot end up writing to a row it
    /// did not start on.
    fx_drag: Option<&'static str>,
    /// What the effects ship as. Empty until the host says; see the type.
    fx_defaults: crate::ports::EffectDefaults,
    /// Notes sounding because a gesture is holding them: the audition key, or
    /// a mouse button on a key or a fret.
    ///
    /// **They are highlighted while they sound**, which is what makes clicking
    /// a key on a picture of a piano behave like pressing one. And they are
    /// released when the gesture ends, including when the window loses focus:
    /// a note that outlives the gesture that started it rings forever, and the
    /// only way to stop it is to quit the app.
    /// What the app is currently making the instrument sound, in DISPLAY
    /// pitches. Reconciled once a frame against [`IvoryApp::wanted_sound`].
    ///
    /// **One set for three gestures**, and that is what keeps them from
    /// fighting: a note held down with the mouse, a chord latched by
    /// keytoggle, and the Space audition can all want the same pitch, and only
    /// the union's EDGES become note-ons and note-offs. Sending them
    /// separately double-triggers the overlap and leaves the instrument
    /// holding a note the app believes it stopped.
    sounding: std::collections::BTreeSet<u8>,
    /// The note under a mouse button that is still down, with keytoggle off.
    /// One at a time: a press replaces whatever the last one was.
    clicked: std::collections::BTreeSet<u8>,
    /// Set on the frame Space goes down: sound everything afresh.
    ///
    /// **Space is a STRIKE, not a request for the note to exist.** Without
    /// this, pressing it with a chord already latched changes nothing — the
    /// notes are wanted, they are sounding, and the diff is empty — so the key
    /// appears to have stopped working. What a pianist wants from it is the
    /// chord again, from the top, which on a decaying instrument is the whole
    /// point of pressing it twice.
    restrike: bool,
    /// Whether the audition key was down last frame, so a hold is one note-on
    /// rather than one per frame. Key auto-repeat retriggered the chord dozens
    /// of times a second before this.
    audition_held: bool,
    /// A numeric field being typed into, if any.
    ///
    /// Mutually exclusive with `name_focused` in practice, because a press
    /// anywhere clears whichever one it is not opening — two focused text
    /// fields would both eat the same keystroke.
    num_edit: Option<recorder::NumEdit>,
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

    /// The Welcome card, held back until the launch splash has gone.
    ///
    /// **It is its own OS window, and the splash is a LAYER.** The splash is
    /// painted on the foreground layer of the main window, which covers every
    /// band and every inline panel — and nothing at all of a separate
    /// viewport. So the card opened on the first frame and sat on top of the
    /// wordmark for the whole of the launch wait.
    /// The backing track, as the host decoded it. Empty when none is loaded.
    track: crate::ports::TrackInfo,
    pending_welcome: Option<dialogs::Dialog>,
    /// Whether the host says its launch splash is still up. Always false in
    /// hosts that have no splash, which is every host but the desktop app.
    splash_up: bool,
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

/// A band control the pointer is holding.
///
/// `moved` is what separates a DRAG from a TAP, and it latches: once a gesture
/// has moved it is a drag for the rest of its life, so dragging a fader back to
/// where it started does not turn into a tap on release and open a text field
/// over the value you just set.
#[derive(Clone, Copy)]
struct Grab {
    hit: recorder_panel::Hit,
    from: Pos2,
    moved: bool,
    /// What the control read when it was grabbed, 0..=1.
    ///
    /// Only a knob uses it. See [`KNOB_TRAVEL`]: a knob is dragged RELATIVELY,
    /// so the value it lands on is this plus how far the hand has moved, and
    /// something has to remember where it started.
    from_value: f32,
}

/// Points of travel for a knob's whole sweep. See the panel's own constant.
use crate::recorder_panel::KNOB_TRAVEL;

/// The pitches a theory-panel hit means, in the octave above middle C.
///
/// The same octave `toggle_theory_hit` places into, so a triad clicked with
/// the toggle off sounds where the same click would have put it with the
/// toggle on. One rule, so the two modes are the same instrument.
fn theory_pitches(hit: theory_panel::Hit) -> Vec<u8> {
    match hit {
        theory_panel::Hit::Pc(pc) => vec![60 + pc],
        theory_panel::Hit::Triad { root, minor } => {
            let m = if minor {
                theory_panel::minor_triad(root)
            } else {
                theory_panel::major_triad(root)
            };
            (0..12u8)
                .filter(|pc| m & (1 << pc) != 0)
                .map(|pc| 60 + pc)
                .collect()
        }
    }
}

/// How far the pointer has to travel before a press counts as a drag.
///
/// Generous rather than tight. A tap is meant to be easy: on a trackpad a
/// deliberate click drifts a pixel or two, and a "tap" that needed the pointer
/// to be perfectly still would open the text field about half the time.
const TAP_SLOP: f32 = 4.0;

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
            // And the camera pane, for exactly the same reason and with the
            // same failure: it is laid out from this flag alone with no caps
            // term, so a plugin editor would give a third of its theory row to
            // a camera it can never open — and the only menu row that turns it
            // off is itself inside the capture-devices gate, so there would be
            // no way back.
            settings.show_camera_pane = false;
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
        //
        // **And NOT gated on there being a diagram in it.** That gate produced
        // a state with no way out from the keyboard: a session saved detached
        // and empty came back with no window and no band, and every 1/2/3/4/T
        // press afterwards edited settings that nothing on screen was reading.
        // The empty window says what it is and takes a number key; a window
        // that never opens says nothing at all.
        let startup_theory_detach_at = settings
            .theory_detached
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
            last_band: Rect::NOTHING,
            last_theory: Rect::NOTHING,
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
            chord_alternates: Vec::new(),
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
            recorder_request: std::collections::VecDeque::new(),
            dir_request: None,
            file_request: None,
            cartridge: crate::ports::CartridgeInfo::default(),
            plugin_rescan: false,
            reveal_request: None,
            export_override: None,
            settings_save_at: None,
            picker_slot: 0,
            grabbed: None,
            num_edit: None,
            track: crate::ports::TrackInfo::default(),
            menu_over_recorder: false,
            menu_over_staff: false,
            setup_open: false,
            fx_open: None,
            track_open: false,
            track_drag: None,
            fx_drag: None,
            fx_defaults: crate::ports::EffectDefaults::default(),
            sounding: std::collections::BTreeSet::new(),
            clicked: std::collections::BTreeSet::new(),
            restrike: false,
            audition_held: false,
            audio_status: recorder::AudioStatus::default(),
            fullscreen_sent: None,
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
            dialog: None,
            pending_welcome: welcome,
            splash_up: false,
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

    /// Everything drawn as active: what is lit, plus whatever is sounding.
    fn display_notes(&self) -> HashSet<u8> {
        let mut set = self.lit_notes();
        // A note being sounded is a note being played, so it lights up like
        // one. Added AFTER the transpose rather than before: `sounding` is
        // already in display pitches, and transposing it a second time lit
        // phantom keys a whole interval above the ones being played.
        set.extend(self.sounding.iter().copied());
        set
    }

    /// Keys lit by the keyboard and by keytoggle: the picture before any
    /// gesture of this app's own sounds anything.
    fn lit_notes(&self) -> HashSet<u8> {
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
        // **Only what arrived over MIDI is transposed.**
        //
        // The transpose exists because a player is in one key and reading in
        // another: a note comes off the keyboard in concert pitch and the
        // picture shows it where they want to read it. That applies to notes
        // the KEYBOARD sent and to nothing else.
        //
        // `manual_notes` are already in that space. They came from a click on
        // the drawn keyboard, the drawn neck, the drawn staff — `piano::
        // hit_test` answers with the note that key is DRAWN as. Transposing
        // them again moves the highlight off the key that was clicked: with a
        // transpose of -11, clicking middle C lit C sharp a major seventh
        // below it. The same double application had `sounding` lighting
        // phantom keys until it was moved after this line.
        let held: HashSet<u8> = self.notes.held().iter().copied().collect();
        let mut set = transposed(&held, self.settings.transpose);
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
    /// The pitch under `pos` on the sheet-music panel, if it is showing.
    ///
    /// The staff is one of the theory band's cells, so this asks the theory
    /// panel where that cell is rather than working it out again.
    fn staff_note_at(&self, band: Rect, pos: Pos2) -> Option<u8> {
        let (_, cell) = theory_panel::cells(band, &self.settings.theory_views())
            .into_iter()
            .find(|(v, _)| *v == theory_panel::View::Staff)?;
        // The same shrink the panel applies before handing the cell to the
        // staff, and the same readout flag — otherwise the pointer is measured
        // against a staff a few points from the one on screen.
        staff::hit_test(
            theory_panel::staff_body(cell),
            self.settings.chord_detection_enabled,
            &self.settings,
            pos,
        )
    }

    /// Sound `note` while the button is down, and place it if keytoggle is on.
    ///
    /// **Both, not one or the other.** The sound follows the CLICK in either
    /// mode — that is what makes every surface here an instrument — and the
    /// toggle decides only whether the note is still lit after the button
    /// comes up. See [`wanted_sound`](IvoryApp::wanted_sound) for why the
    /// latch does not hold the note down.
    fn place_or_play(&mut self, note: u8) {
        self.sound_while_held([note]);
        if !self.settings.keytoggle_enabled {
            return;
        }
        if self.manual_notes.remove(&note) {
            self.manual_positions.remove(&note);
        } else {
            self.manual_notes.insert(note);
        }
        self.sync_pins();
        self.detection_tick(true);
        self.voicing_tick(true);
    }

    /// Sound these until the mouse button comes up, replacing whatever the
    /// last press sounded.
    ///
    /// One press at a time: dragging across a keyboard is not a glissando
    /// here, and a chord placed from the lattice arrives as a chord.
    fn sound_while_held(&mut self, notes: impl IntoIterator<Item = u8>) {
        self.clicked.clear();
        self.clicked.extend(notes);
    }

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

        // **A vertex REPLACES what is showing, rather than adding to it.**
        //
        // The harmonic triangles are the one view where the thing you point at
        // is a whole chord, and the thing anybody does with them is compare:
        // press C, then F, then G. Adding each to the last builds a nine-note
        // cluster and calls it a chord — so a second vertex clears the first.
        //
        // Pressing the SAME one again still takes it off, which is what makes
        // it a toggle rather than a radio button you can never switch off.
        // And this is deliberately not done for a pitch CLASS: the circle and
        // the lattice are how a chord is built up one note at a time, which is
        // the opposite gesture.
        if matches!(hit, theory_panel::Hit::Triad { .. }) && !all_present {
            self.manual_notes.clear();
            self.manual_positions.clear();
        }
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
            // The alternates go with it. They are unreachable while detection
            // is off — the readout draws nothing without a winner — but state
            // that outlives the thing it describes is a bug waiting for a
            // reader.
            self.chord_alternates.clear();
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
        if notes.is_empty() {
            self.current_chord = None;
            self.chord_alternates.clear();
            return;
        }
        // The ranked pass, but only while the staff panel is on screen to
        // show it. It is the SAME detection — `detect_chord_debug` calls
        // `detect_chord` and collects what the scorer already computed — so
        // the winner cannot disagree between the two paths; the only cost is
        // a string per scored candidate, which nothing else would read.
        if self
            .settings
            .theory_views()
            .contains(theory_panel::View::Staff)
        {
            let (winner, ranked) = self.detector.detect_chord_debug(&notes, 8);
            self.chord_alternates = ranked
                .into_iter()
                .map(|(name, _)| name)
                .filter(|name| Some(name.as_str()) != winner.as_deref())
                .take(2)
                .collect();
            self.current_chord = winner;
        } else {
            self.current_chord = self.detector.detect_chord(&notes);
            self.chord_alternates.clear();
        }
    }

    /// The staff panel's chord readout, from whichever settings copy is
    /// painting — the live ones, or the composite's override. `None` when
    /// detection is off in that copy, which is what lets the staff take the
    /// readout strip's height back.
    /// Everything the band draws and hit-tests from.
    ///
    /// One builder for all of it. The arguments were identical at seven call
    /// sites, which is seven places to forget a new one — and the one being
    /// forgotten silently is a band that draws from stale state.
    fn recorder_layout_view(&self) -> recorder::RecorderView<'_> {
        recorder::RecorderView {
            fx_units: self.fx_units(),
            track: &self.track,
            ..self.recorder.view(
            self.settings.record_take_name.as_deref().unwrap_or_default(),
            self.name_focused,
            self.num_edit.as_ref(),
                self.settings.knobs(),
                self.settings.record_hide_elapsed,
                // Which control is being turned right now, so a knob can show
                // its number while a hand is on it. `moved`, not merely
                // grabbed: a press that has not travelled yet is on its way to
                // being a tap.
                self.grabbed
                    .filter(|g| g.moved)
                    .and_then(|g| recorder_panel::num_field(g.hit)),
            )
        }
    }

    fn staff_readout(&self, s: &Settings) -> Option<staff::Readout<'_>> {
        s.chord_detection_enabled.then_some(staff::Readout {
            chord: self.current_chord.as_deref(),
            alternates: &self.chord_alternates,
        })
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
            window_sizing: self.caps.window_sizing,
        }
    }

    /// Whether the transpose control is offered at all.
    fn transpose_view(&self) -> Option<i32> {
        self.settings
            .show_transpose
            .then_some(self.settings.transpose as i32)
    }

    /// Move the transpose by `step` semitones, if every note can come along.
    ///
    /// **All or nothing.** A chord whose top note would leave MIDI's range is
    /// not transposed at all, rather than transposed with that note dropped —
    /// dropping one note of a voicing silently changes the chord, and the whole
    /// point of this control is to ask what the chord becomes.
    fn transpose_by(&mut self, step: i64) {
        let want = (self.settings.transpose + step)
            .clamp(-crate::settings::TRANSPOSE_MAX, crate::settings::TRANSPOSE_MAX);
        if want == self.settings.transpose {
            return;
        }
        let mut held: HashSet<u8> = self.notes.held().iter().copied().collect();
        if self.settings.keytoggle_enabled {
            held.extend(self.manual_notes.iter().copied());
        }
        if transposed(&held, want).len() != held.len() {
            return;
        }
        let was = self.settings.transpose;
        self.settings.transpose = want;
        // **Placed notes are moved, not re-projected.**
        //
        // They live in the space they were CLICKED in — see `lit_notes` — so
        // the display transform does not reach them and they have to be
        // carried across by hand. Doing it here is what keeps both halves
        // true at once: an arrow key transposes a chord you built by clicking,
        // and a click lands on the key you clicked.
        //
        // The range check above already covered them, so this cannot leave
        // MIDI's range.
        let step = want - was;
        let shift = |n: &u8| u8::try_from(i64::from(*n) + step).unwrap_or(*n);
        self.manual_notes = self.manual_notes.iter().map(shift).collect();
        self.manual_positions = self
            .manual_positions
            .iter()
            .map(|(n, p)| (shift(n), *p))
            .collect();
        self.save_settings();
        // The neck and the theory band read the same notes, so they have to be
        // rebuilt from the new ones rather than left showing the old shape.
        self.rebuild_voicing();
        self.detection_tick(true);
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
            camera_pane_on: self.settings.show_camera_pane,
            extra_plugin_folders: self.settings.plugin_paths.len(),
            record_dir_is_default: self.settings.record_dir_is_default,
            open_when_done: self.settings.record_open_when_done,
            staff_on: self.settings.theory_views().contains(theory_panel::View::Staff),
            staff_note_names: self.settings.staff_note_names,
            staff_set: self.settings.staff_set.clone(),
            // Offered only when there IS one — a "Custom" row that does nothing
            // is worse than no row, and the way to make one is the dialog.
            staff_custom_label: self
                .settings
                .custom_staff_set
                .as_ref()
                .map(|k| staff::StaffSet::from_key(k).label()),
            staff_clefs: self
                .settings
                .staff_set()
                .clefs()
                .iter()
                .map(|c| c.key().to_owned())
                .collect(),
            recorder_detached: self.settings.recorder_detached,
            count_in_beats: self.settings.count_in_beats(),
            time_signature: self.settings.time_signature(),
            count_in_bars: self.settings.count_in_bars(),
            count_in_in_take: self.settings.record_count_in_in_take,
            chord_strip: self.settings.show_chord_strip,
            key_note_names: self.settings.show_piano_note_names,
            fret_note_names: self.settings.show_fret_note_names,
            recorder_first: self.menu_over_recorder && self.settings.show_recorder,
            staff_first: self.menu_over_staff,
            staff_key: self.settings.staff_key,
            record_sources: self.settings.record_sources.clone(),
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
    /// The heart's colour IF the chord strip is the one drawing it.
    ///
    /// `None` whenever the recorder band is in the window, because that is
    /// where the heart lives now — the strip is the fallback for the windows
    /// that have no band, which is a recorder turned off and a recorder torn
    /// off into its own window alike. The condition is `recorder_h > 0.0`'s,
    /// and it is written out rather than derived so the two cannot drift.
    fn strip_heart_color(&self) -> Option<egui::Color32> {
        let band_in_window = self.settings.show_recorder && !self.settings.recorder_detached;
        (!band_in_window).then(|| self.heart_color()).flatten()
    }

    fn heart_color(&self) -> Option<egui::Color32> {
        if !self.settings.show_heart {
            return None;
        }
        // **Drawn for everybody, in the colour they chose.** The heart is the
        // way into the thanks card, and the people on that card are being
        // thanked for the app existing rather than for anybody's purchase, so
        // gating the credit behind a key would be a strange thing to do to
        // them. It used to be dim for non-supporters and the licence check has
        // been dropped outright: there is nothing behind a key at all, which
        // is the point until 5.0 leaves beta.
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
        let (primary_pressed, pointer_down, pointer_released, pointer, ctrl, shift) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos(),
                i.modifiers.ctrl,
                i.modifiers.shift,
            )
        });
        let ctrl_as_context = cfg!(target_os = "macos") && ctrl;

        // Right-click (or ctrl-click on macOS, Qt default) opens the menu.
        if resp.secondary_clicked() || (resp.clicked() && ctrl_as_context) {
            if let Some(pos) = pointer {
                let global = self.main_inner_origin + pos.to_vec2();
                // A right-click ON the Recorder band opens the menu with the
                // Recorder at the top. The band is where its own settings
                // belong: it is a surface with fifteen controls on it, and
                // reaching the sixteenth meant a right-click anywhere else
                // followed by a hunt down a list of subjects that are mostly
                // about the piano.
                // **Right-clicking a knob types a number into it**, on every
                // one of the eight, which is the gesture a knob has on every
                // desk-shaped thing anybody has used. Checked before the
                // band's own menu, which is what a right-click anywhere else
                // means.
                //
                // **Shift holds the effect's parameters open instead.** They
                // used to be the plain right-click and there is no third
                // button to give them; between the two, typing a value is the
                // one somebody does mid-take and the panel is the one they
                // open once and leave alone. The status line under a knob says
                // so while a hand is on it.
                if let Some(hit) = self.knob_under(recorder_rect, pos) {
                    if shift {
                        if let Some(fx) = self.fx_under(recorder_rect, pos) {
                            self.fx_open = Some(fx);
                            self.fx_drag = None;
                        }
                        return;
                    }
                    if let Some(field) = recorder_panel::num_field(hit) {
                        self.num_edit = Some(recorder::NumEdit::new(field));
                        self.name_focused = false;
                    }
                    return;
                }
                // **Right-clicking the metronome sets whether the click goes
                // into the FILE**, and opens no menu. It is the one control in
                // the band with no box of its own: it is set once and it was
                // taking a caption and a tick in the busiest row there is.
                // Everywhere else on the band, a right-click opens the menu.
                // **And right-clicking the microphone opens the audio
                // input's picker.** Same idea, one row down: the device
                // belongs to the fader it feeds, and it was reachable only
                // from a menu led by subjects that are mostly about the piano.
                if let Some(r) = recorder_rect.filter(|r| r.contains(pos)) {
                    let view = self.recorder_layout_view();
                    if recorder_panel::input_icon(r, &view).is_some_and(|i| i.contains(pos)) {
                        self.apply_recorder_hit(recorder_panel::Hit::PickAudio);
                        return;
                    }
                    // **And right-clicking the waveform icon opens the
                    // waveform.** A left click imports; where the file starts
                    // and stops is a question about a picture, and the row is
                    // fifteen points tall.
                    if recorder_panel::track_icon(r, &view).is_some_and(|i| i.contains(pos)) {
                        self.track_open = true;
                        self.track_drag = None;
                        return;
                    }
                    if recorder_panel::hit_test(r, &view, pos)
                        == Some(recorder_panel::Hit::ToggleMetronome)
                    {
                        self.settings.metronome_in_take = !self.settings.metronome_in_take;
                        self.save_settings();
                        return;
                    }
                }
                self.menu_over_recorder = recorder_rect.is_some_and(|r| r.contains(pos));
                // And the same for the sheet music: right-clicking the staff
                // leads with its own controls, which is where the key signature
                // lives. Sixteen key signatures are worth reaching from the
                // thing they are printed on rather than from a list of subjects
                // that are mostly about the piano.
                self.menu_over_staff = theory_rect.is_some_and(|r| {
                    theory_panel::cells(r, &self.settings.theory_views())
                        .into_iter()
                        .any(|(v, cell)| v == theory_panel::View::Staff && cell.contains(pos))
                });
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
        // Letting go of the mouse stops a clicked note, wherever the pointer
        // ended up. Outside the band, over another window, anywhere: the
        // release is what ends the gesture, not where it happened.
        if pointer_released {
            self.clicked.clear();
        }

        // The backing track's panel, on the same terms as the effect panels.
        if self.track_open {
            if primary_pressed || (pointer_down && self.track_drag.is_some()) {
                if let Some(pos) = pointer {
                    self.press_in_track_panel(ui.max_rect(), pos, primary_pressed);
                }
                return;
            }
            if pointer_released {
                self.track_drag = None;
            }
        }

        // The effect panel, on the same terms as the take settings below it.
        if let Some(fx) = self.fx_open {
            if primary_pressed || (pointer_down && self.fx_drag.is_some()) {
                if let Some(pos) = pointer {
                    self.press_in_fx_panel(ui.max_rect(), fx, pos, primary_pressed);
                }
                return;
            }
            if pointer_released {
                self.fx_drag = None;
            }
        }

        // **The popup eats the press, wherever it lands.** Inside, it is a
        // control or it is the panel's own chrome; outside, it is a dismissal
        // and nothing else — a modal you can click THROUGH is one that closes
        // when you meant to press something behind it, which is every mis-click
        // in a dialog anybody has ever sworn at.
        if self.setup_open && primary_pressed {
            if let Some(pos) = pointer {
                let band = recorder_rect.unwrap_or(Rect::NOTHING);
                let view = self.recorder_layout_view();
                let anchor = recorder_panel::setup_rect(band, &view);
                let panel = recorder_panel::setup_popup_rect(ui.max_rect(), anchor);
                match recorder_panel::setup_hit_test(ui.max_rect(), anchor, &view, pos) {
                    Some(hit) => self.apply_recorder_hit(hit),
                    None if !panel.contains(pos) => self.setup_open = false,
                    None => {}
                }
            }
            return;
        }

        if primary_pressed && !ctrl_as_context {
            if let Some(pos) = pointer {
                // The Recorder band first, and NOT behind `keytoggle_enabled`.
                // Its controls are buttons, not an instrument: the record
                // button has to work whether or not the user has turned on
                // clicking the piano to place notes.
                if let Some(r) = recorder_rect.filter(|r| r.contains(pos)) {
                    // The heart takes the press before the band's own hit
                    // test looks at it. It sits in a strip carved off the
                    // instrument column, so the two can never contend — but
                    // checking first is what keeps that true if the column
                    // ever grows back into it.
                    if self.heart_color().is_some()
                        && recorder_panel::heart_rect(r, &self.recorder_layout_view())
                            .contains(pos)
                    {
                        self.settings.heart_color = self.settings.heart_color.wrapping_add(1);
                        self.save_settings();
                        return;
                    }
                    let hit = recorder_panel::hit_test(
                        r,
                        &self.recorder_layout_view(),
                        pos,
                    );
                    // Remember a dragged control for as long as the button is
                    // held. Only the value-carrying hits are grabbable; a
                    // button does not want a drag.
                    self.grabbed = hit.filter(recorder_panel::Hit::is_draggable).map(|h| Grab {
                        hit: h,
                        from: pos,
                        moved: false,
                        from_value: self.control_value(h),
                    });
                    // A press anywhere in the band that is not the name field
                    // takes focus off it, which is what makes clicking away
                    // commit the name the way every other text field does.
                    self.name_focused = matches!(hit, Some(recorder_panel::Hit::NameField));
                    // The same for a number being typed: pressing anything else
                    // commits it. Committing rather than discarding, because a
                    // half-typed tempo that vanishes when you look away is a
                    // field people learn not to trust.
                    self.commit_number_unless(hit);
                    if let Some(hit) = hit {
                        // A value control is NOT applied on press. It is applied
                        // once the gesture has moved far enough to be a drag —
                        // see the drag block — which is what leaves the tap free
                        // to mean "type into this". Nothing was lost: all a tap
                        // used to do was jump the value to wherever the pointer
                        // happened to land, which is the least precise thing
                        // either gesture can do.
                        if !recorder_panel::Hit::is_draggable(&hit) {
                            self.apply_recorder_hit(hit);
                        }
                    }
                    // **Setup opens the menu, here, on the press that asked
                    // for it.** The hit handler cannot: opening a menu needs
                    // the context and an anchor, and it has neither, so it
                    // sets a flag — and a flag nobody reads is a button that
                    // does nothing, which is exactly how this one behaved.
                    // Anchored on the button rather than the pointer so the
                    // list always hangs off the same corner.
                    return;
                }
                // Clicking anywhere else also drops the field's focus, or the
                // next letter typed at the piano would go into the take name.
                self.name_focused = false;
                self.commit_number_unless(None);
                // The supporter heart cycles colour on click. Checked before the
                // keytoggle hit-test because it sits in the chord strip, not the
                // keyboard, so the two can never contend.
                // The transpose arrows, top-left of the chord view. Checked
                // before the keytoggle hit-test for the same reason the heart
                // and the capo are: they sit on top, so they take the click.
                if self.settings.show_transpose {
                    if let Some(cr) = chord_rect {
                        let (up, down) = chord_strip::transpose_rects(cr);
                        if up.contains(pos) {
                            self.transpose_by(1);
                            return;
                        }
                        if down.contains(pos) {
                            self.transpose_by(-1);
                            return;
                        }
                    }
                }
                if self.strip_heart_color().is_some() {
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

                // **The theory band is an instrument too, in both modes.** A
                // name on the circle, a node on the lattice, a vertex of the
                // triangles, a line or a space on the STAFF: each is a place a
                // musician points at meaning a note, and every one of them was
                // silent unless keytoggle happened to be on.
                //
                // Handled before the piano and the neck because most of it
                // speaks in pitch classes rather than in notes.
                if let Some(r) = theory_rect.filter(|r| r.contains(pos)) {
                    // The staff first: it is the one that answers with a real
                    // PITCH rather than a pitch class, so it must not be
                    // rounded off to one by whatever the lattice would say.
                    if let Some(note) = self.staff_note_at(r, pos) {
                        self.place_or_play(note);
                        return;
                    }
                    let display = self.display_notes();
                    if let Some(hit) = theory_panel::hit_test(
                        r,
                        &self.settings.theory_views(),
                        self.theory_input(&display),
                        pos,
                    ) {
                        // Sounded either way — a triad as a triad, not as its
                        // lowest note — and placed as well when the toggle is
                        // on. See `place_or_play`.
                        self.sound_while_held(theory_pitches(hit));
                        if self.settings.keytoggle_enabled {
                            self.toggle_theory_hit(hit);
                        }
                        return;
                    }
                }
                // **Keytoggle off: a click SOUNDS the note and lets it go.**
                // The keyboard and the neck are instruments either way; the
                // switch decides whether a click leaves the note behind. With
                // it off, clicking a key used to do nothing at all, which is
                // the wrong answer for a picture of a piano.
                if !self.settings.keytoggle_enabled {
                    let hit = if piano_rect.contains(pos) {
                        let local = pos - piano_rect.min;
                        piano::hit_test(local.x, local.y, piano_rect.width(), piano_rect.height())
                    } else {
                        fret_rect.filter(|r| r.contains(pos)).and_then(|r| {
                            fretboard_panel::hit_test(r, &self.settings.fretboard_spec(), pos)
                        })
                    };
                    if let Some(note) = hit {
                        self.sound_while_held([note]);
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
                        // **Sounded on the way past, in this mode too.** The
                        // toggle decides whether the note stays LIT, not
                        // whether pressing a key makes a sound.
                        self.sound_while_held([note]);
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
    /// Start and stop the held audition, once per frame.
    ///
    /// **A hold and not a press.** Key auto-repeat fires `key_pressed` many
    /// times a second, which retriggered the whole chord at the repeat rate;
    /// and a note that stops on a timer stops in the middle of a phrase
    /// somebody is still holding. Down means sounding, up means silent, and
    /// the edges are the only events.
    ///
    /// Losing focus counts as letting go. The release for a key the OS stopped
    /// telling us about never arrives, and the chord would ring until the app
    /// was quit.
    fn audition_tick(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.focused);
        let down = focused
            && !self.name_focused
            && self.num_edit.is_none()
            && self.dialog.is_none()
            && !self.recorder.state.is_active()
            && ctx.input(|i| !i.modifiers.any() && i.key_down(egui::Key::Space));

        if down == self.audition_held {
            return;
        }
        self.audition_held = down;
        // The DOWN edge only. Holding the key is one strike, not sixty a
        // second — that is what `audition_held` being an edge is for.
        self.restrike |= down;
    }

    /// Every note a GESTURE of this app's own is asking to sound, right now.
    ///
    /// Three sources, unioned rather than sent separately:
    ///
    /// - **keytoggle's latch**, which outlives the click that made it. The
    ///   switch decides whether a click LEAVES the note behind, not whether it
    ///   makes a sound: a picture of a piano whose keys light up silently is
    ///   the wrong answer either way.
    /// - **a mouse button still down** with keytoggle off, which sounds until
    ///   it is let go.
    /// - **the Space audition**, which is the whole picture, held.
    ///
    /// MIDI notes are not in here. They reach the instrument through the MIDI
    /// path already, and sounding them again from the app would be a second
    /// note-on for a key somebody is holding down.
    fn wanted_sound(&self) -> std::collections::BTreeSet<u8> {
        let mut want = std::collections::BTreeSet::new();
        // **What keytoggle latches is VISUAL, and only visual.**
        //
        // The switch decides whether a click leaves the note BEHIND on screen.
        // It does not hold the sound: a note-on with no note-off is a patch
        // ringing for ever, and on an organ or a pad that is exactly what it
        // sounds like. So the latched set is not in here.
        //
        // What sounds is a gesture somebody is making — a button still down,
        // or Space — and both of those end. Space is how a latched chord is
        // heard again, which is the whole reason it is a strike.
        want.extend(self.clicked.iter().copied());
        if self.audition_held {
            want.extend(self.lit_notes());
        }
        want
    }

    /// Make the instrument sound exactly what [`wanted_sound`] asks for.
    ///
    /// **Once a frame, from a diff**, rather than at each mutation — so every
    /// path that can change the set is covered by construction instead of by
    /// remembering to call something: a key, a fret, a chord vertex on the
    /// lattice, Clear, Space, letting go of the mouse, switching keytoggle
    /// off, or starting a take.
    ///
    /// [`wanted_sound`]: IvoryApp::wanted_sound
    fn reconcile_sound(&mut self) {
        let want = self.wanted_sound();
        // **A strike starts from nothing.** Diffing against an empty set makes
        // every wanted note a note-on, including the ones already sounding, so
        // a latched chord is struck again rather than left alone.
        let held = if std::mem::take(&mut self.restrike) {
            std::collections::BTreeSet::new()
        } else {
            if want == self.sounding {
                return;
            }
            std::mem::take(&mut self.sounding)
        };
        let off: Vec<u8> = if held.is_empty() {
            // Everything, because everything is about to be re-struck. Not
            // `held`, which is empty by construction on a strike.
            self.sounding.iter().copied().collect()
        } else {
            held.difference(&want).copied().collect()
        };
        let on: Vec<u8> = want.difference(&held).copied().collect();
        self.sounding = want;
        // **Off before on.** Releasing a pitch and re-taking it in the same
        // frame has to arrive in that order, or the instrument ends up holding
        // a note this app believes it stopped. The two are separate requests
        // because `Audition` carries one direction, which is why the queue is
        // a queue.
        if !off.is_empty() {
            self.request_recorder(recorder::RecorderRequest::Audition {
                notes: off,
                on: false,
            });
        }
        if !on.is_empty() {
            self.request_recorder(recorder::RecorderRequest::Audition { notes: on, on: true });
        }
    }

    pub fn take_recorder_request(&mut self) -> Option<recorder::RecorderRequest> {
        self.recorder_request.pop_front()
    }

    /// Add a folder to the list of places VST3 bundles are looked for.
    ///
    /// The scan itself belongs to the host — this crate may not read a disk —
    /// so the app records the folder and asks to be rescanned; see
    /// `wants_plugin_rescan`.
    pub fn add_plugin_folder(&mut self, dir: std::path::PathBuf) {
        let text = dir.to_string_lossy().into_owned();
        if text.trim().is_empty() || self.settings.plugin_paths.contains(&text) {
            return;
        }
        self.settings.plugin_paths.push(text);
        self.save_settings();
        self.plugin_rescan = true;
    }

    /// The folders the user has added, for the host's scanner.
    pub fn plugin_folders(&self) -> Vec<std::path::PathBuf> {
        self.settings
            .plugin_paths
            .iter()
            .map(std::path::PathBuf::from)
            .collect()
    }

    /// Whether the host should scan for plugins again and hand the list back.
    ///
    /// Latched and drained, exactly like every other request this crate makes:
    /// the UI records that it wants one, the host does it after the frame, and
    /// nothing between those two points opens a directory.
    pub fn take_plugin_rescan(&mut self) -> bool {
        std::mem::take(&mut self.plugin_rescan)
    }

    /// Take a pending "choose a folder" request. Same contract.
    pub fn take_directory_request(&mut self) -> Option<crate::ports::DirRequest> {
        self.dir_request.take()
    }

    /// Which effect's knob is under `pos`, if any.
    /// Which knob is under `pos`, if any. See [`recorder_panel::Hit::is_knob`].
    fn knob_under(&self, band: Option<Rect>, pos: Pos2) -> Option<recorder_panel::Hit> {
        let r = band.filter(|r| r.contains(pos))?;
        let view = self.recorder_layout_view();
        recorder_panel::hit_test(r, &view, pos).filter(|h| h.is_knob())
    }

    fn fx_under(&self, band: Option<Rect>, pos: Pos2) -> Option<recorder_panel::Fx> {
        let band = band.filter(|r| r.contains(pos))?;
        let view = self.recorder_layout_view();
        recorder_panel::Fx::ALL.into_iter().find(|fx| {
            recorder_panel::knob_rect(band, &view, fx.hit()).is_some_and(|r| r.contains(pos))
        })
    }

    /// Where the open effect panel hangs from: its own knob.
    fn fx_anchor(&self, fx: recorder_panel::Fx) -> Rect {
        let view = self.recorder_layout_view();
        recorder_panel::knob_rect(self.last_band, &view, fx.hit()).unwrap_or(Rect::NOTHING)
    }

    /// Where the track panel hangs from: the waveform icon in the band.
    fn track_anchor(&self) -> Rect {
        let view = self.recorder_layout_view();
        recorder_panel::track_icon(self.last_band, &view).unwrap_or(Rect::NOTHING)
    }

    /// A press or a drag inside the backing track's panel.
    ///
    /// **A drag stays on the handle it started on**, the same rule the effect
    /// panels' rows follow: the two handles meet when a track is trimmed to
    /// nothing, and a hand that crossed over would start dragging the other
    /// one from under itself.
    fn press_in_track_panel(&mut self, screen: Rect, pos: Pos2, pressed: bool) {
        let anchor = self.track_anchor();
        let seconds = self.track.seconds;
        if let Some(is_in) = self.track_drag.filter(|_| !pressed) {
            let l = recorder_panel::TrackLayout::new(screen, anchor);
            if l.wave.is_positive() {
                let t = ((pos.x - l.wave.left()) / l.wave.width()).clamp(0.0, 1.0);
                self.set_trim(is_in, f64::from(t) * seconds);
            }
            return;
        }
        if !pressed {
            return;
        }
        let hit = recorder_panel::track_hit_test(
            screen,
            anchor,
            seconds,
            self.settings.track_in,
            self.settings.track_out,
            pos,
        );
        match hit {
            Some(recorder_panel::TrackHit::Close) => {
                self.track_open = false;
                self.track_drag = None;
            }
            Some(recorder_panel::TrackHit::ClearTrim) => {
                self.settings.track_in = 0.0;
                self.settings.track_out = 0.0;
                self.save_settings();
            }
            Some(recorder_panel::TrackHit::TypeIn) => {
                self.num_edit = Some(recorder::NumEdit::new(recorder::NumField::TrackIn));
            }
            Some(recorder_panel::TrackHit::TypeOut) => {
                self.num_edit = Some(recorder::NumEdit::new(recorder::NumField::TrackOut));
            }
            Some(recorder_panel::TrackHit::DragIn(t)) => {
                self.track_drag = Some(true);
                self.set_trim(true, f64::from(t) * seconds);
            }
            Some(recorder_panel::TrackHit::DragOut(t)) => {
                self.track_drag = Some(false);
                self.set_trim(false, f64::from(t) * seconds);
            }
            // Inside the panel and on nothing: swallowed. Outside: dismissed.
            None => {
                if !recorder_panel::track_popup_rect(screen, anchor).contains(pos) {
                    self.track_open = false;
                    self.track_drag = None;
                }
            }
        }
    }

    /// Move one trim point, keeping the two in order.
    ///
    /// **They may not cross.** An out-point before the in-point is a track
    /// that plays nothing, and the way a person discovers it is by pressing
    /// Record and hearing silence.
    fn set_trim(&mut self, is_in: bool, seconds: f64) {
        let len = self.track.seconds;
        let want = seconds.clamp(0.0, len);
        if is_in {
            let end = if self.settings.track_out <= 0.0 {
                len
            } else {
                self.settings.track_out
            };
            self.settings.track_in = want.min((end - recorder_panel::MIN_TRIM).max(0.0));
        } else {
            // Landing on the very end means "to the end", which is the zero
            // the engine and the settings both read as "no out-point" — so a
            // track dragged back to full length stops carrying an out-point
            // that would have to be updated if the file were ever replaced.
            self.settings.track_out = if want >= len - recorder_panel::MIN_TRIM {
                0.0
            } else {
                want.max(self.settings.track_in + recorder_panel::MIN_TRIM)
            };
        }
        self.save_settings_soon();
    }

    /// A press or a drag inside the open effect panel.
    ///
    /// **A drag stays on the row it started on.** Once `fx_drag` names a key,
    /// every later position sets THAT key, wherever the pointer has slid to —
    /// the same rule the band's own faders follow, and for the same reason: a
    /// hand that drifts up a row must not start setting the row above.
    fn press_in_fx_panel(
        &mut self,
        screen: Rect,
        fx: recorder_panel::Fx,
        pos: Pos2,
        pressed: bool,
    ) {
        let anchor = self.fx_anchor(fx);
        if let Some(key) = self.fx_drag {
            if let Some(v) = recorder_panel::fx_value_at(screen, anchor, fx, key, pos) {
                self.set_effect_param(key, v);
            }
            return;
        }
        if !pressed {
            return;
        }
        match recorder_panel::fx_hit_test(screen, anchor, fx, pos) {
            Some(recorder_panel::FxHit::Set { key, value }) => {
                self.fx_drag = Some(key);
                self.set_effect_param(key, value);
            }
            Some(recorder_panel::FxHit::NextChoice { key }) => self.next_choice(key),
            Some(recorder_panel::FxHit::Reset(fx)) => self.reset_effect(fx),
            Some(recorder_panel::FxHit::Close) => self.fx_open = None,
            // A press on the panel's own chrome is swallowed; one outside it
            // closes the panel and does nothing else.
            None => {
                if !recorder_panel::fx_popup_rect(screen, anchor).contains(pos) {
                    self.fx_open = None;
                }
            }
        }
    }

    /// Write one effect parameter, 0..=1.
    fn set_effect_param(&mut self, key: &str, value: f32) {
        let v = f64::from(value.clamp(0.0, 1.0));
        self.settings
            .effect_params
            .insert(key.to_owned(), serde_json::Value::from(v));
        self.save_settings_soon();
    }

    /// Load a made-up backing track and open its panel, for the screenshot
    /// hook — which has no file to import and no dialog to import it with.
    pub fn set_track_for_shot(&mut self, info: crate::ports::TrackInfo, open: bool) {
        self.track = info;
        self.track_open = open;
    }

    /// Put a level on the master meter and a reduction on the limiter, so the
    /// screenshot hook can show a meter that is doing something. A silent
    /// picture of a meter says nothing about whether it reads.
    pub fn set_master_for_shot(&mut self, master: recorder::Meters, gr_db: f32) {
        self.recorder.master = master;
        self.recorder.gr_db = gr_db;
    }

    /// Open the patch editor with a patch in it. For the screenshot hook,
    /// which has no host to fill it in.
    pub fn open_patch_editor_for_shot(&mut self) {
        self.dialog = Some(dialogs::Dialog::PatchEditor {
            slot: 0,
            patch: crate::ports::PatchEdit {
                name: "E.PIANO 1".to_owned(),
                algorithm: 4,
                routing: [0, 1, 0, 3, 0, 5],
                feedback_op: 6,
                groups: vec![crate::ports::PatchGroup {
                    title: "OP1".to_owned(),
                    params: (0..8)
                        .map(|i| crate::ports::PatchParam {
                            name: format!("Rate {}", i + 1),
                            value: 60 + i,
                            max: 99,
                            choices: Vec::new(),
                            unit: String::new(),
                        })
                        .collect(),
                }],
                bank_path: "~/.config/ivory/my-patches.syx".to_owned(),
            },
            group: 0,
            name: "E.PIANO 1".to_owned(),
            note: String::new(),
        });
    }

    /// Open an effect's panel. For the host's screenshot hook, which drives
    /// the same state a right-click sets.
    pub fn open_effect_panel(&mut self, fx: recorder_panel::Fx) {
        self.fx_open = Some(fx);
    }

    /// Ask the host for an audio file to play along to.
    ///
    /// **Every extension ffmpeg and CoreAudio read**, plus an "All files"
    /// filter behind it, for the same reason the cartridge picker has one: a
    /// filter dims non-matching files on macOS and HIDES them on Windows, and
    /// a folder that looks empty is a dialog somebody closes again.
    fn ask_for_track(&mut self) {
        self.file_request = Some(crate::ports::FileRequest {
            start_at: (!self.settings.track_path.is_empty())
                .then(|| std::path::PathBuf::from(&self.settings.track_path))
                .and_then(|p| p.parent().map(std::path::Path::to_path_buf)),
            title: "Choose a backing track".to_owned(),
            extensions: [
                "wav", "aiff", "aif", "mp3", "m4a", "aac", "flac", "ogg", "opus", "caf", "wma",
                "mp4", "mov",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            extension_label: "Audio".to_owned(),
            purpose: crate::ports::FilePurpose::BackingTrack,
        });
    }

    /// The backing track, as the host decoded it.
    pub fn set_track_info(&mut self, info: crate::ports::TrackInfo) {
        self.track = info;
    }

    /// Remember which file the track came from, so it loads again next launch.
    pub fn set_track_path(&mut self, path: String) {
        self.settings.track_path = path;
        // A fresh file starts untrimmed: the in and out points belonged to the
        // one before it, and a track that opens already cut to somebody else's
        // length is a track that looks broken.
        self.settings.track_in = 0.0;
        self.settings.track_out = 0.0;
        self.save_settings();
    }

    /// The backing track's file and trim, for the host to reload it at launch.
    pub fn track_settings(&self) -> (String, f64, f64) {
        (
            self.settings.track_path.clone(),
            self.settings.track_in,
            self.settings.track_out,
        )
    }

    /// Put the trim back after a reload. See `load_track_at_launch`.
    pub fn set_track_trim(&mut self, from: f64, to: f64) {
        self.settings.track_in = from.max(0.0);
        self.settings.track_out = to.max(0.0);
    }

    /// The backing track's level, trim and length, for the host to push at the
    /// engine. Seconds, because that is what the settings hold.
    pub fn track_playback(&self) -> (f32, f64, f64) {
        (
            self.settings.track_gain as f32,
            self.settings.track_in,
            self.settings.track_out,
        )
    }

    /// What the effects ship as. Told by the host; see [`EffectDefaults`].
    ///
    /// [`EffectDefaults`]: crate::ports::EffectDefaults
    pub fn set_effect_defaults(&mut self, defaults: crate::ports::EffectDefaults) {
        self.fx_defaults = defaults;
    }

    /// Step a named parameter to the next value in its list, and wrap.
    fn next_choice(&mut self, key: &str) {
        let Some(c) = self.fx_defaults.choices.iter().find(|c| c.key == key) else {
            return;
        };
        if c.options.is_empty() {
            return;
        }
        let now = self.choice_key(key);
        let i = c.options.iter().position(|(k, _)| *k == now).unwrap_or(0);
        let next = c.options[(i + 1) % c.options.len()].0.clone();
        self.settings
            .effect_params
            .insert(key.to_owned(), serde_json::Value::from(next));
        self.save_settings();
    }

    /// A named parameter's current key, as stored or as it ships.
    ///
    /// A stored value the host does not offer is ignored rather than kept: it
    /// is what a settings file written by a later build looks like, and the
    /// answer to "24 dB isn't a slope I have" is the default, not an empty row.
    fn choice_key(&self, key: &str) -> String {
        let Some(c) = self.fx_defaults.choices.iter().find(|c| c.key == key) else {
            return String::new();
        };
        self.settings
            .effect_params
            .get(key)
            .and_then(serde_json::Value::as_str)
            .filter(|k| c.options.iter().any(|(o, _)| o == k))
            .map_or_else(|| c.default.clone(), str::to_owned)
    }

    /// What a named parameter's row shows.
    fn choice_label(&self, key: &str) -> String {
        let now = self.choice_key(key);
        self.fx_defaults
            .choices
            .iter()
            .find(|c| c.key == key)
            .and_then(|c| c.options.iter().find(|(k, _)| *k == now))
            .map_or(now, |(_, label)| label.clone())
    }

    /// Put one effect back to what it shipped as.
    ///
    /// By REMOVING the keys rather than writing the defaults into the file: a
    /// parameter that is not in the settings is a parameter at its default,
    /// which is the same rule that makes an old file load without a migration.
    fn reset_effect(&mut self, fx: recorder_panel::Fx) {
        for recorder_panel::FxRow { key, .. } in fx.rows() {
            if !key.is_empty() {
                self.settings.effect_params.shift_remove(key);
            }
        }
        self.save_settings();
    }

    /// What one effect parameter reads, for the panel to draw.
    fn effect_param(&self, key: &str) -> f32 {
        self.settings
            .effect_params
            .get(key)
            .or_else(|| self.fx_defaults.values.get(key))
            .and_then(serde_json::Value::as_f64)
            // Half, for a key nothing has an opinion about. Only reachable
            // before the host has said anything, which on the desktop is never
            // and in a plugin is always — and a plugin has no effects panel.
            .map_or(0.5, |v| v as f32)
    }

    /// What a value-carrying control reads right now, 0..=1.
    ///
    /// What each of the six knobs' numbers mean, in `Fx::ALL` order.
    ///
    /// Percent unless the host said otherwise, which is the right answer for
    /// a build talking to an older host and for the four that really are
    /// percentages.
    fn fx_units(&self) -> [crate::ports::KnobUnit; 6] {
        let mut out = [crate::ports::KnobUnit::Percent; 6];
        for (fx, slot) in recorder_panel::Fx::ALL.into_iter().zip(&mut out) {
            if let Some((_, u)) = self
                .fx_defaults
                .units
                .iter()
                .find(|(k, _)| k == fx.mix_key())
            {
                *slot = *u;
            }
        }
        out
    }

    /// Where one effect knob's value lives in the settings.
    ///
    /// **One place, reached by both the reader and the writer.** Six knobs
    /// times two directions is twelve chances to wire a knob to the wrong
    /// field, and a knob reading one number while writing another looks
    /// exactly like a knob that does not work.
    fn fx_mix(&mut self, fx: recorder_panel::Fx) -> &mut f64 {
        use recorder_panel::Fx;
        match fx {
            Fx::Reverb => &mut self.settings.reverb_mix,
            Fx::Delay => &mut self.settings.delay_mix,
            Fx::Chorus => &mut self.settings.chorus_mix,
            Fx::Hpf => &mut self.settings.hpf_mix,
            Fx::Lpf => &mut self.settings.lpf_mix,
            Fx::Limiter => &mut self.settings.limiter_mix,
        }
    }

    /// Only the knobs answer, because only the knobs are dragged relatively
    /// and need somewhere to start from. Everything else returns zero and does
    /// not use it.
    /// One effect knob's value, read-only. The mirror of [`Self::fx_mix`].
    fn fx_value(&self, fx: recorder_panel::Fx) -> f32 {
        use recorder_panel::Fx;
        (match fx {
            Fx::Reverb => self.settings.reverb_mix,
            Fx::Delay => self.settings.delay_mix,
            Fx::Chorus => self.settings.chorus_mix,
            Fx::Hpf => self.settings.hpf_mix,
            Fx::Lpf => self.settings.lpf_mix,
            Fx::Limiter => self.settings.limiter_mix,
        }) as f32
    }

    fn control_value(&self, hit: recorder_panel::Hit) -> f32 {
        use recorder_panel::Hit as H;
        let fader = |g: f64| recorder::gain_to_fader(g as f32);
        match hit {
            H::SetFx(fx, _) => self.fx_value(fx),
            H::SetMetronomeGain(_) => fader(self.settings.metronome_gain),
            H::SetInputGain(_) => fader(self.settings.input_gain),
            H::SetMaster(_) => fader(self.settings.master_gain),
            H::SetTrackGain(_) => fader(self.settings.track_gain),
            H::SetSlotGain(i, _) => self
                .settings
                .plugin_gains
                .get(i)
                .copied()
                .map_or(0.0, fader),
            H::SetTempo(_) => recorder_panel::tempo_knob_position(
                self.settings.record_export.tempo_bpm,
            ),
            // Nothing else is dragged, so nothing else needs somewhere to
            // start from.
            _ => 0.0,
        }
    }

    /// The dialog on screen, if any. The host reads it to keep a slot row in
    /// step with the picker that belongs to it.
    pub fn open_dialog(&self) -> Option<&dialogs::Dialog> {
        self.dialog.as_ref()
    }

    /// The three effect sends, 0..=1, for the host to hand the audio thread.
    pub fn effect_sends(&self) -> [f32; 6] {
        [
            self.settings.reverb_mix as f32,
            self.settings.delay_mix as f32,
            self.settings.chorus_mix as f32,
            self.settings.hpf_mix as f32,
            self.settings.lpf_mix as f32,
            self.settings.limiter_mix as f32,
        ]
    }

    /// What each effect is set to, as the flat map the settings file holds.
    ///
    /// **Untyped on purpose.** `ivory-ui` cannot name an `effects::Params` —
    /// that type lives in the binary, behind the firewall, with the DSP it
    /// belongs to. The UI's job is to hold numbers a person moved and write
    /// them down; deciding what "reverb size 0.62" means to eight comb filters
    /// is not its business.
    pub fn effect_params(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.settings.effect_params
    }

    /// The cartridge path and patch the host should restore at launch.
    pub fn dx7_choice(&self) -> (&str, usize) {
        (&self.settings.dx7_cartridge, self.settings.dx7_patch)
    }

    /// Remember which cartridge is loaded, so the next launch opens it.
    pub fn set_dx7_cartridge(&mut self, path: String) {
        self.settings.dx7_cartridge = path;
        // The patch index belonged to the cartridge that was there before.
        self.settings.dx7_patch = 0;
        self.save_settings();
    }

    /// Take a pending "choose a file" request. Same contract again.
    pub fn take_file_request(&mut self) -> Option<crate::ports::FileRequest> {
        self.file_request.take()
    }

    /// The host has read a cartridge (or failed to). Show it.
    ///
    /// **Pushed in, because the UI cannot open a file.** `ivory-ui` has no
    /// filesystem and no SysEx parser; it has thirty-two strings and a name.
    /// Updates the open picker in place rather than reopening it, so the
    /// scroll position and the filter survive loading a bank.
    pub fn set_cartridge(&mut self, info: crate::ports::CartridgeInfo) {
        self.cartridge = info;
        let Some(dialogs::Dialog::PatchPicker {
            bank,
            bad_checksum,
            voices,
            selected,
            filter,
            error,
            ..
        }) = self.dialog.as_mut()
        else {
            return;
        };
        // A failed load leaves the cartridge that was working right where it
        // was. Somebody who picks the wrong file should not lose their sound.
        if !self.cartridge.error.is_empty() {
            error.clone_from(&self.cartridge.error);
            return;
        }
        error.clear();
        bank.clone_from(&self.cartridge.bank);
        *bad_checksum = self.cartridge.bad_checksum;
        voices.clone_from(&self.cartridge.voices);
        // A new bank means the old selection is a different patch. Nothing is
        // selected until somebody picks, and until then the built-in plays.
        *selected = None;
        filter.clear();
    }

    /// Show what is in `slot`: a VST3's own editor, or the built-in's patches.
    ///
    /// **The built-in has no window to open.** Same gesture either way, and it
    /// has to be: "click the slot to see the instrument" is one thing a user
    /// learns, not two. A VST3 shows its editor; the built-in shows the patch
    /// picker, which IS its editor.
    ///
    /// Public because the host's `IVORY_OPEN_EDITOR` hook drives it too, and a
    /// dev hook that took a different path would exercise a path no user has.
    pub fn open_slot_editor(&mut self, slot: usize) {
        if self.chosen_plugin(slot) == Some(dialogs::BUILTIN_PATH) {
            self.open_patch_picker(slot);
        } else {
            self.request_recorder(recorder::RecorderRequest::OpenPluginEditor(slot));
        }
    }

    /// The host has read the patch being edited. Show it.
    ///
    /// **Pushed in, like everything else that crosses the firewall.** The UI
    /// draws rows with numbers in them; what a number means to six operators
    /// is the synth's business. See `dx7::edit`.
    ///
    /// The open PAGE and the caret in the name field survive, because this
    /// arrives after every keystroke and a page that reset itself on each one
    /// would make the editor unusable.
    pub fn set_patch_edit(&mut self, edit: crate::ports::PatchEdit, note: Option<String>) {
        let Some(dialogs::Dialog::PatchEditor {
            patch, name, note: n, ..
        }) = self.dialog.as_mut()
        else {
            return;
        };
        // The name is what the user is typing, not what the patch says: the
        // format trims to ten characters and pads with spaces, and echoing
        // that back mid-word would fight the keyboard.
        if name.is_empty() && !edit.name.is_empty() {
            name.clone_from(&edit.name);
        }
        *patch = edit;
        if let Some(said) = note {
            *n = said;
        }
    }

    /// Open the patch picker for the built-in in `slot`.
    fn open_patch_picker(&mut self, slot: usize) {
        let selected = (!self.cartridge.voices.is_empty()
            && self.settings.dx7_patch < self.cartridge.voices.len())
            .then_some(self.settings.dx7_patch);
        self.dialog = Some(dialogs::Dialog::patch_picker(
            slot,
            self.cartridge.bank.clone(),
            self.cartridge.bad_checksum,
            self.cartridge.voices.clone(),
            selected,
        ));
    }

    /// What the audio path is doing, from the host that owns the devices.
    ///
    /// Pushed every frame rather than pulled, for the same reason the recorder
    /// view is: this crate must not be able to ask a device anything.
    /// Tell the app whether the host's launch splash is still on screen.
    ///
    /// Pushed every frame, like the recorder's state, and for the same reason:
    /// the splash belongs to the host and this crate must not be able to ask
    /// it anything. Hosts with no splash never call it and it stays false.
    pub fn set_splash_up(&mut self, up: bool) {
        self.splash_up = up;
    }

    pub fn set_audio_status(&mut self, status: recorder::AudioStatus) {
        // Also refreshed into an OPEN panel, or it would show whatever was true
        // at the moment it was opened and never change.
        if let Some(dialogs::Dialog::AudioStatus { status: live, .. }) = self.dialog.as_mut() {
            live.clone_from(&status);
        }
        self.audio_status = status;
    }

    /// Frames per audio callback the user has chosen, or `None` for the
    /// device's own default. The host reopens its streams to match.
    pub fn buffer_frames(&self) -> Option<u32> {
        self.settings.buffer_frames()
    }

    /// Take a pending "show me this folder" request. Same contract.
    pub fn take_reveal_request(&mut self) -> Option<std::path::PathBuf> {
        self.reveal_request.take()
    }

    /// Whether a finished take should show itself.
    pub fn record_open_when_done(&self) -> bool {
        self.settings.record_open_when_done
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
            Hit::DismissClip => self.request_recorder(R::DismissClip),
            // Opens the menu, led by the Recorder's own categories, at the
            // button. The take's settings live there now, and a button that
            // opens the place they went is what keeps them findable by
            // somebody who never thinks to right-click a band.
            // **A panel, not a menu.** These are boxes with captions and
            // values — a folder path you want to READ, a device that may say
            // "(not connected)", four ticks whose current state is the whole
            // question — and a menu row can show none of that.
            Hit::OpenSetup => self.setup_open = true,
            Hit::CloseSetup => self.setup_open = false,
            Hit::ShowAudioStatus => {
                self.dialog = Some(dialogs::Dialog::AudioStatus {
                    status: self.audio_status.clone(),
                    buffer: self.settings.buffer_frames(),
                });
            }
            Hit::ToggleCountInInTake => {
                self.settings.record_count_in_in_take = !self.settings.record_count_in_in_take;
                self.save_settings();
            }
            Hit::ToggleHideElapsed => {
                self.settings.record_hide_elapsed = !self.settings.record_hide_elapsed;
                self.save_settings();
            }
            Hit::ChooseFolder => self.ask_for_a_folder(),
            Hit::ToggleDefaultDir => {
                self.settings.record_dir_is_default = !self.settings.record_dir_is_default;
                // Unticking it does NOT forget the folder. The tick means "keep
                // using this next time"; clearing the path as well would throw
                // away the choice the user just made in the act of saying they
                // did not want it to be permanent.
                self.save_settings();
            }
            Hit::RevealFolder => self.reveal_record_folder(None),
            Hit::ToggleOpenWhenDone => {
                self.settings.record_open_when_done = !self.settings.record_open_when_done;
                self.save_settings();
            }
            Hit::NameField => self.name_focused = true,
            Hit::PickCamera => self.open_device_picker(dialogs::DeviceKind::Camera),
            Hit::PickAudio => self.open_device_picker(dialogs::DeviceKind::AudioInput),
            // A click OPENS it for typing — there is nothing to drag, so the
            // tap-versus-drag gesture the faders use does not apply.
            Hit::EditTimeSignature => {
                self.num_edit = Some(recorder::NumEdit::new(recorder::NumField::Meter));
                self.name_focused = false;
            }
            Hit::CycleCountIn => {
                // **BARS.** `COUNT_IN_CHOICES` became bars when the time
                // signature arrived, and this went on comparing them against
                // `count_in_beats()` and writing the legacy beats key — which
                // the band no longer reads. So the control cycled a number
                // nothing displayed, and the band appeared not to react to
                // clicks at all.
                let choices = recorder::COUNT_IN_CHOICES;
                let now = self.settings.count_in_bars();
                let next = choices
                    .iter()
                    .position(|c| *c == now)
                    .map_or(choices[0], |i| choices[(i + 1) % choices.len()]);
                self.set_count_in_bars(next);
            }
            Hit::Export => self.open_export_dialog(),
            Hit::PickSlot(slot) => self.open_plugin_picker(slot),
            Hit::ClearSlot(slot) => {
                if let Some(p) = self.settings.plugin_slots.get_mut(slot) {
                    *p = None;
                    self.save_settings();
                }
            }
            Hit::OpenSlotEditor(slot) => self.open_slot_editor(slot),
            // Both knobs write to settings and are pushed to the engine after
            // the frame, the same shape as a fader: `save_settings_soon`
            // because a drag is a hundred of these and each one is a file.
            Hit::SetFx(fx, v) => {
                *self.fx_mix(fx) = f64::from(v.clamp(0.0, 1.0));
                self.save_settings_soon();
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
            Hit::SetMaster(p) => {
                self.settings.master_gain = f64::from(recorder::fader_to_gain(p));
                self.save_settings_soon();
            }
            Hit::SetTrackGain(p) => {
                self.settings.track_gain = f64::from(recorder::fader_to_gain(p));
                self.save_settings_soon();
            }
            Hit::ImportTrack => self.ask_for_track(),
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
        // **The take's tempo, override and all.** This read the SETTINGS while
        // `export_spec` reads the session-only override, so a tempo set for one
        // take in the Export dialog moved the `.mid` and the on-screen count
        // and left the CLICK playing the old one. One number, one source: a
        // click at 90 against a file that says 120 is the exact failure the
        // "one tempo" rule in `ExportSpec` exists to prevent.
        self.export_spec().tempo_bpm
    }

    /// The take's time signature: what the click accents and how long a bar of
    /// count-in is.
    pub fn time_signature(&self) -> recorder::TimeSignature {
        self.settings.time_signature()
    }

    /// Count-in length in bars.
    pub fn count_in_bars(&self) -> u32 {
        self.settings.count_in_bars()
    }

    /// Whether the count-in belongs INSIDE the take.
    pub fn count_in_in_take(&self) -> bool {
        self.settings.record_count_in_in_take
    }

    /// `auto` / `input` / `plugin` / `both`, verbatim from the file.
    pub fn audio_source_setting(&self) -> &str {
        &self.settings.record_sources
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

    /// The user explicitly chose "None - record MIDI only".
    ///
    /// Distinct from `chosen_audio_uid() == None`, which is also what "has
    /// never opened the picker" looks like — and those two want opposite
    /// behaviour at startup: a default input so the meter is live, or no input
    /// at all.
    pub fn audio_explicitly_off(&self) -> bool {
        self.settings.record_input_off
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
        let mut spec = self.export_override.unwrap_or(self.settings.record_export);
        // **The camera goes in once.** With the pane on, the camera is part of
        // the window's own layout and therefore already inside the display
        // layer; a second camera layer over the top would paint the same
        // picture twice, once in the corner it belongs in and once wherever the
        // export layout felt like.
        //
        // Enforced here rather than by editing the stored spec, so turning the
        // pane off gives the old composite back without the user having to go
        // and re-tick anything.
        if self.settings.show_camera_pane {
            spec.composite.camera = false;
            spec.composite.display = true;
        }
        spec
    }

    /// Put the sheet music in the band if it is not there.
    ///
    /// Every control that changes what the staff SHOWS calls this: turning the
    /// clef on a panel that is not on screen is the one thing those keys could
    /// do that would look broken, and "nothing happened" is indistinguishable
    /// from "the key does not work".
    fn show_the_staff(&mut self) {
        use crate::theory_panel::View;
        if !self.settings.theory_views().contains(View::Staff) {
            self.settings.toggle_theory_view(View::Staff);
        }
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
        // **Always the NEXT take.**
        //
        // This used to open the post-take dialog whenever a finished take was
        // on screen, on the reading that Export then means "re-export THAT".
        // `last_take_folder` stays set for the rest of the session, so after
        // one take the layout and the camera were greyed for ever — and the
        // dialog is the only place either can be chosen, so the one control
        // that decides what a video LOOKS like became unreachable after the
        // first recording.
        //
        // It bought nothing, because nothing re-exports a finished take:
        // there is no offline pass that repaints a display-only video from a
        // recorded `.mid`, and the camera frames were encoded live and never
        // kept. Every setting in this dialog applies to the NEXT take whichever
        // mode it opened in, so greying half of them only stopped people
        // configuring the thing the dialog is for.
        //
        // `Rederivable` and the post-take wording stay — they are correct, and
        // they are what the day-one version of a real re-export will need. This
        // is about which dialog the Export ROW opens, not about deleting the
        // rule.
        self.dialog = Some(Dialog::export(spec, false, false));
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

    /// Commit whatever number is being typed, unless the press that triggered
    /// this landed on the very field being typed into.
    ///
    /// The exception is what lets somebody click back into a field they are
    /// already editing — to fix a typo — without the click committing it out
    /// from under them.
    fn commit_number_unless(&mut self, hit: Option<recorder_panel::Hit>) {
        let Some(edit) = self.num_edit.as_ref() else {
            return;
        };
        if hit.and_then(recorder_panel::num_field) == Some(edit.field) {
            return;
        }
        self.commit_number();
    }

    /// Apply what has been typed, and close the field.
    ///
    /// Text that is not a number simply closes without changing anything.
    /// Refusing to close would trap the user in a field they cannot satisfy,
    /// and there is nothing to warn about: the value they were looking at is
    /// still the value they have.
    fn commit_number(&mut self) {
        let Some(edit) = self.num_edit.take() else {
            return;
        };
        use recorder::NumField as F;
        use recorder_panel::Hit;
        // The signature is not a `Hit` — there is no value to carry along a
        // control — so it is committed here and returns nothing to apply.
        if edit.field == F::Meter {
            if let Some(sig) = recorder::TimeSignature::parse(&edit.text) {
                self.set_time_signature(sig);
            }
            return;
        }
        // The trim points are not `Hit`s either: they belong to a panel rather
        // than to a control in the band, and they are the only two numbers
        // here measured in time.
        if matches!(edit.field, F::TrackIn | F::TrackOut) {
            if let Some(t) = recorder::parse_time(&edit.text) {
                self.set_trim(edit.field == F::TrackIn, t);
                self.save_settings();
            }
            return;
        }
        let hit = match edit.field {
            F::Meter | F::TrackIn | F::TrackOut => None,
            F::Tempo => recorder::parse_bpm(&edit.text).map(Hit::SetTempo),
            // The setters take a FADER POSITION, not a gain, so a typed dB has
            // to go back through the same curve the drag uses. Doing it here
            // rather than teaching the setters a second unit keeps one
            // definition of what a fader position means.
            F::Slot(i) => recorder::parse_gain(&edit.text)
                .map(|g| Hit::SetSlotGain(i, recorder::gain_to_fader(g))),
            F::Metronome => recorder::parse_gain(&edit.text)
                .map(|g| Hit::SetMetronomeGain(recorder::gain_to_fader(g))),
            F::Input => recorder::parse_gain(&edit.text)
                .map(|g| Hit::SetInputGain(recorder::gain_to_fader(g))),
            // Decibels, like the faders it shares a curve with.
            F::Master => recorder::parse_gain(&edit.text)
                .map(|g| Hit::SetMaster(recorder::gain_to_fader(g))),
            F::Track => recorder::parse_gain(&edit.text)
                .map(|g| Hit::SetTrackGain(recorder::gain_to_fader(g))),
            // A PERCENT, not a gain: these are not faders and there is no dB
            // curve to invert. "40" is four tenths wet.
            // In the knob's OWN unit: a filter is typed in hertz.
            F::Fx(fx) => recorder_panel::knob_typed(self.fx_units()[fx.index()], &edit.text)
                .map(|v| Hit::SetFx(fx, v)),
        };
        if let Some(hit) = hit {
            self.apply_recorder_hit(hit);
            // Committed values are written NOW rather than through the drag
            // debounce. A drag is followed by more drag; pressing Enter is
            // somebody saying they are finished.
            self.save_settings();
        }
    }

    /// The numeric field, driven from raw input, exactly as the name field is
    /// and for the same reason: the band is a pure painter and cannot own a
    /// `TextEdit`.
    fn edit_number(&mut self, ctx: &egui::Context) {
        let events = ctx.input(|i| i.events.clone());
        for event in events {
            match event {
                egui::Event::Text(text) => {
                    if let Some(edit) = self.num_edit.as_mut() {
                        for ch in text.chars() {
                            edit.push(ch);
                        }
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Backspace,
                    pressed: true,
                    ..
                } => {
                    if let Some(edit) = self.num_edit.as_mut() {
                        edit.pop();
                    }
                }
                egui::Event::Key {
                    key: egui::Key::Enter | egui::Key::Tab,
                    pressed: true,
                    ..
                } => self.commit_number(),
                // Escape ABANDONS, which is the one thing that must not go
                // through `commit_number`.
                egui::Event::Key {
                    key: egui::Key::Escape,
                    pressed: true,
                    ..
                } => {
                    // Escape backs out of ONE thing at a time, innermost
                    // first: a number being typed into the popup belongs to
                    // the popup, and closing both on one press would throw
                    // away the panel somebody was still working in.
                    if self.num_edit.is_some() {
                        self.num_edit = None;
                    } else {
                        self.setup_open = false;
                    }
                }
                _ => {}
            }
        }
    }

    /// Set the count-in, in bars, from wherever asked.
    ///
    /// One place, because there are two — the band's cell and the menu — and
    /// they disagreed: the band cycled the legacy BEATS key while the display
    /// read bars, so clicking it changed a number nothing showed.
    /// Set the time signature from wherever asked, keeping the count-in's
    /// beat count in step: a count-in is a number of BARS, so changing the
    /// signature changes how many beats that is.
    fn set_time_signature(&mut self, sig: recorder::TimeSignature) {
        self.settings.record_time_signature = sig.label();
        self.settings.record_count_in_beats =
            i64::from(sig.beats_in(self.settings.count_in_bars()));
        self.save_settings();
    }

    fn set_count_in_bars(&mut self, bars: u32) {
        self.settings.record_count_in_bars = i64::from(bars);
        // The legacy key is kept in step so a downgrade to a build that reads
        // beats still counts in for roughly as long.
        self.settings.record_count_in_beats =
            i64::from(self.settings.time_signature().beats_in(bars));
        self.save_settings();
    }

    fn request_recorder(&mut self, request: recorder::RecorderRequest) {
        // Refused rather than queued where the host cannot honour it. A plugin
        // never drains, so an ungated request would sit here forever and the
        // first thing a Toggle did after somebody added draining would be to
        // start a take nobody asked for.
        if !self.caps.capture_devices {
            return;
        }
        self.recorder_request.push_back(request);
    }

    /// Ask the host to raise a folder picker.
    ///
    /// Not a blocking call and not a `RecorderRequest`: `rfd`'s native panel
    /// runs a nested run loop, so raising one from inside a frame means
    /// re-entering the frame already on the stack. The host drains this after
    /// `frame()` returns.
    /// Ask the host to show a folder in the file manager.
    ///
    /// `None` means the destination root — the folder the button beside it
    /// chooses. A take's own folder is passed explicitly, because a take that
    /// has finished is a more useful thing to be shown than the place takes go.
    fn reveal_record_folder(&mut self, path: Option<std::path::PathBuf>) {
        if !self.caps.capture_devices || !self.caps.native_file_dialogs {
            return;
        }
        self.reveal_request = Some(path.unwrap_or_else(|| self.settings.record_root()));
    }

    fn ask_for_a_folder(&mut self) {
        if !self.caps.capture_devices || !self.caps.native_file_dialogs {
            return;
        }
        self.dir_request = Some(crate::ports::DirRequest {
            start_at: Some(self.settings.record_root()),
            title: "Where should Tangent put your takes?".to_owned(),
            purpose: crate::ports::DirPurpose::RecordRoot,
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
            // Read from the viewport rather than tracked in a field, so the
            // toggle cannot disagree with the window: somebody who leaves
            // fullscreen with the green button, or with the OS's own gesture,
            // has changed the state without this app hearing about it, and a
            // remembered bool would then need pressing twice to do anything.
            K::ToggleFullscreen => {
                let now = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!now));
            }
            K::ToggleFretboard => self.apply_menu_action(ctx, MenuAction::ToggleFretboard),
            K::ToggleCameraPane => self.apply_menu_action(ctx, MenuAction::ToggleCameraPane),
            K::CycleClef => self.apply_menu_action(ctx, MenuAction::CycleClef),
            // **All three surfaces, one key.** `U` is "show me the letters",
            // and having it mean the staff alone while the keyboard and the
            // neck each needed a menu row was three switches for one question.
            // The staff's own default differs and stays different; what this
            // key does is turn them all on together and all off together, from
            // whatever they were.
            K::ToggleNoteNames => {
                let any = self.settings.staff_note_names
                    || self.settings.show_piano_note_names
                    || self.settings.show_fret_note_names;
                let want = !any;
                self.settings.staff_note_names = want;
                self.settings.show_piano_note_names = want;
                self.settings.show_fret_note_names = want;
                self.save_settings();
            }
            // **Every element back, in the numbered order.** It used to walk a
            // five-state cycle through combinations of three diagrams, which
            // existed because three independent flags have eight states and no
            // natural order. The number keys are that control now, and they do
            // it better — so this key is the way OUT of any arrangement,
            // including the empty one you cannot press a number to escape from
            // without knowing which number.
            K::CycleTheory => self.apply_menu_action(ctx, MenuAction::ShowAllTheory),
            K::ToggleTheoryElement(n) => {
                if let Some(v) = theory_panel::View::from_number(n) {
                    self.apply_menu_action(ctx, MenuAction::ToggleTheoryView(v));
                }
            }
            // Space. Not routed through a `MenuAction`, because there is no
            // menu row for it: pressing Record is what the BAND is for, and a
            // menu row that starts a take would be reachable from a right-click
            // over the piano with the recorder hidden.
            // Toggle, not "start": pressing Enter during a count-in has to
            // cancel it, which is what anybody who has just realised they came
            // in wrong will try — and `Toggle` is where that already lives.
            // Pressing it while a take is ROLLING does nothing, because Space
            // is the key that ends a performance.
            K::StartRecording => {
                if !self.recorder.state.is_writing() {
                    self.request_recorder(recorder::RecorderRequest::Toggle);
                }
            }
            // **One key, whichever meaning is live.** Space stops a take while
            // one is running, which is what every transport the user owns is
            // bound to. It is the obvious key for "let me hear that" and the
            // rest of the time "stop" means nothing, so the rest of the time it
            // sounds whatever is lit.
            // Stop only. The audition half of this key is a HOLD and is
            // driven per frame by `audition_tick`, because a press repeats and
            // a held chord must not.
            K::StopRecording => {
                if self.recorder.state.is_active() {
                    self.request_recorder(recorder::RecorderRequest::Stop);
                }
            }
            K::ToggleRecorder => self.apply_menu_action(ctx, MenuAction::ToggleRecorder),
            K::TransposeUp => self.transpose_by(1),
            K::TransposeDown => self.transpose_by(-1),
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
                    ColorTarget::RecorderBg => self.settings.recorder_bg_color,
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
                self.settings.toggle_theory_view(v);
                self.save_settings();
                // The band's HEIGHT does not change with the count — the cells
                // divide its width — but it goes to zero when the last element
                // leaves and comes back when the first one returns, so the
                // window still has to be re-measured.
                self.request_natural_size();
            }
            MenuAction::ShowAllTheory => {
                self.settings
                    .set_theory_views(&theory_panel::Views::all());
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
            MenuAction::ToggleCameraPane => {
                self.settings.show_camera_pane = !self.settings.show_camera_pane;
                self.save_settings();
                // The theory band's width changes with it, so its height does
                // too — the window has to be asked for a new size or the
                // diagrams draw into a row that is the wrong shape for them.
                self.request_natural_size();
            }
            MenuAction::CycleClef => {
                // Turning the clef on a band you cannot see is the one thing
                // this key could do that would look broken, so it opens it.
                let next = self.settings.staff_set().next();
                self.settings.set_staff_set(&next);
                self.show_the_staff();
                self.save_settings();
                // The staff count changes with the set — one staff or two — so
                // the window has to be asked for a new height.
                self.request_natural_size();
            }
            MenuAction::SetStaffSet(key) => {
                self.settings.staff_set = if key == "__custom__" {
                    // The stack the user built, brought back out of where it is
                    // kept. See `Settings::custom_staff_set`.
                    self.settings
                        .custom_staff_set
                        .clone()
                        .unwrap_or_else(|| "grand".to_owned())
                } else {
                    key.to_owned()
                };
                self.show_the_staff();
                self.save_settings();
                self.request_natural_size();
            }
            MenuAction::ToggleCustomClef(key) => {
                if let Some(clef) = staff::Clef::from_key(key) {
                    // Toggling from a PRESET starts the custom stack off from
                    // what is on screen, so ticking "Alto" while the grand
                    // staff is showing gives you treble, alto and bass — not
                    // alto alone, which is what starting from nothing would do
                    // and is never what anybody means.
                    match self.settings.staff_set().with_clef_toggled(clef) {
                        Some(set) => {
                            self.settings.set_staff_set(&set);
                            self.settings.custom_staff_set = Some(set.key());
                        }
                        // The last staff was just removed. A band with nothing
                        // in it is not a view.
                        None => {
                            self.settings.staff_set = "grand".to_owned();
                            self.settings.custom_staff_set = None;
                        }
                    }
                    self.show_the_staff();
                    self.save_settings();
                    self.request_natural_size();
                }
            }
            MenuAction::SetStaffKey(k) => {
                self.settings.staff_key = k.clamp(-staff::MAX_KEY, staff::MAX_KEY);
                self.show_the_staff();
                self.save_settings();
                self.request_natural_size();
            }
            MenuAction::ToggleNoteNames => {
                self.settings.staff_note_names = !self.settings.staff_note_names;
                // Like every other control that changes what the staff shows.
                // Without this, `U` on a window with no sheet music in it
                // silently flips a setting nothing is drawing.
                self.show_the_staff();
                self.save_settings();
                self.request_natural_size();
            }
            // The take's settings that moved out of the band. Each one is the
            // SAME action the band's own hit produced, so the two can never
            // drift: one handler, two ways in.
            MenuAction::ToggleKeyNoteNames => {
                self.settings.show_piano_note_names = !self.settings.show_piano_note_names;
                self.save_settings();
            }
            MenuAction::ToggleFretNoteNames => {
                self.settings.show_fret_note_names = !self.settings.show_fret_note_names;
                self.save_settings();
            }
            MenuAction::ToggleChordStrip => {
                self.settings.show_chord_strip = !self.settings.show_chord_strip;
                self.save_settings();
            }
            MenuAction::ChooseFolder => self.apply_recorder_hit(recorder_panel::Hit::ChooseFolder),
            MenuAction::RevealFolder => self.apply_recorder_hit(recorder_panel::Hit::RevealFolder),
            MenuAction::ToggleDefaultDir => {
                self.apply_recorder_hit(recorder_panel::Hit::ToggleDefaultDir);
            }
            MenuAction::ToggleOpenWhenDone => {
                self.apply_recorder_hit(recorder_panel::Hit::ToggleOpenWhenDone);
            }
            MenuAction::PickCamera => self.apply_recorder_hit(recorder_panel::Hit::PickCamera),
            MenuAction::PickAudio => self.apply_recorder_hit(recorder_panel::Hit::PickAudio),
            MenuAction::RescanPlugins => {
                self.plugin_rescan = true;
            }
            MenuAction::AddPluginFolder => {
                if self.caps.native_file_dialogs {
                    self.dir_request = Some(crate::ports::DirRequest {
                        start_at: None,
                        title: "Where else should Tangent look for VST3 plugins?".to_owned(),
                        purpose: crate::ports::DirPurpose::PluginFolder,
                    });
                }
            }
            MenuAction::ClearPluginFolders => {
                self.settings.plugin_paths.clear();
                self.save_settings();
                self.plugin_rescan = true;
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
            MenuAction::ShowAudioStatus => {
                self.dialog = Some(dialogs::Dialog::AudioStatus {
                    // A SNAPSHOT, taken when the panel opens. It is refreshed
                    // each frame by the host — see `set_audio_status` — because
                    // a status panel that froze the moment it opened would be
                    // the least useful version of itself.
                    status: self.audio_status.clone(),
                    buffer: self.settings.buffer_frames(),
                });
            }
            MenuAction::SetRecordSources(kind) => {
                self.settings.record_sources = kind.to_owned();
                self.save_settings();
            }
            MenuAction::SetCountIn(bars) => self.set_count_in_bars(bars),
            MenuAction::SetTimeSignature(sig) => self.set_time_signature(sig),
            MenuAction::ToggleCountInInTake => {
                self.settings.record_count_in_in_take = !self.settings.record_count_in_in_take;
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
                     correction is active again too - not just this one."
                        .to_owned()
                } else {
                    String::new()
                };
                format!(
                    "Learned. This voicing now reads {now_reads}.{tail}\n\n\
                     Corrections so far: {corrections}. Similar voicings may\n\
                     read differently now - \"Forget Learning\" in Manage\n\
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
                "{wants} is already Tangent's top-scoring reading - but the name\n\
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
                 To force this name anyway, use \"Teach Chord Name...\" - it\n\
                 pins the name outright."
            ),
            TrainOutcome::NotTrainable => format!(
                "{name} is not one of the readings Tangent weighed for this\n\
                 voicing, so there is nothing to re-rank.\n\n\
                 Use \"Teach Chord Name...\" to pin it instead."
            ),
            TrainOutcome::NoStore => {
                "Chord learning is unavailable - the settings folder could not\n\
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

    /// Apply what a dialog asked for.
    ///
    /// Public so the host's `IVORY_OPEN_EDITOR` hook can drive it: the patch
    /// editor is two clicks in and there is no other way to reach it from a
    /// script.
    pub fn apply_dialog_action(&mut self, action: DialogAction) {
        match action {
            DialogAction::SetBufferFrames(frames) => {
                self.settings.record_buffer_frames = i64::from(frames.unwrap_or(0));
                self.save_settings();
                // The host reopens both streams when it notices; it cannot be
                // done from here, and it must not be done mid-take.
            }
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
            DialogAction::ChoosePatch { slot, index } => {
                // `usize::MAX` is the built-in row: no cartridge patch, the one
                // compiled in. A sentinel rather than an `Option` because it
                // rides the same request the real indices do, and the host has
                // one arm to read instead of two.
                self.settings.dx7_patch = if index == usize::MAX { 0 } else { index };
                self.save_settings_soon();
                self.request_recorder(recorder::RecorderRequest::ChoosePatch { slot, index });
            }
            DialogAction::EditPatch { slot } => {
                // Opened empty; the host fills it in on the next frame, the
                // same way it fills the cartridge. See `set_patch_edit`.
                self.dialog = Some(dialogs::Dialog::PatchEditor {
                    slot,
                    patch: crate::ports::PatchEdit::default(),
                    group: 0,
                    name: String::new(),
                    note: String::new(),
                });
                self.request_recorder(recorder::RecorderRequest::EditPatch { slot });
            }
            DialogAction::ShowPatches { slot } => self.open_patch_picker(slot),
            DialogAction::SetPatchParam {
                group,
                index,
                value,
            } => {
                self.request_recorder(recorder::RecorderRequest::SetPatchParam {
                    group,
                    index,
                    value,
                });
            }
            DialogAction::SetPatchName(name) => {
                self.request_recorder(recorder::RecorderRequest::SetPatchName(name));
            }
            DialogAction::SavePatch => {
                self.request_recorder(recorder::RecorderRequest::SavePatch);
            }
            DialogAction::LoadCartridge => {
                if !self.caps.native_file_dialogs {
                    return;
                }
                self.file_request = Some(crate::ports::FileRequest {
                    // Where the last one came from, which is where the next one
                    // almost certainly is: people keep cartridges in one folder
                    // ten thousand files deep.
                    start_at: std::path::Path::new(&self.settings.dx7_cartridge)
                        .parent()
                        .filter(|p| p.is_dir())
                        .map(std::path::Path::to_path_buf),
                    title: "Choose a DX7 cartridge".to_owned(),
                    extensions: vec!["syx".to_owned(), "SYX".to_owned()],
                    extension_label: "DX7 cartridge".to_owned(),
                    purpose: crate::ports::FilePurpose::Cartridge,
                });
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
                        // **Choosing a camera turns video on.**
                        //
                        // It is the one act in this app that has no other
                        // purpose: opening a camera makes a light come on, and
                        // nobody does it to look at a preview. Before this, a
                        // user could pick their camera, watch the preview, hit
                        // Record, and get a `.wav` — with the band quietly
                        // saying "wav + midi" the whole time.
                        //
                        // ONLY when video is off, so an explicit choice of
                        // "separate file per source" is never overwritten by
                        // changing camera. And composite rather than per-source
                        // because one file with the keyboard under the hands is
                        // what somebody filming themselves play actually wants;
                        // separate files are an editing decision, made later by
                        // someone who knows they want it.
                        if uid.is_some()
                            && self.settings.record_export.video == recorder::VideoMode::None
                        {
                            self.settings.record_export.video = recorder::VideoMode::Composite;
                        }
                        if let Some(d) = self.cameras.as_mut() {
                            let _ = d.open(uid.as_deref().unwrap_or(""));
                        }
                    }
                    dialogs::DeviceKind::AudioInput => {
                        // This flag carries the None choice across a restart.
                        // Without it, `record_audio_device: null` is
                        // indistinguishable from "never chose", and the next
                        // launch helpfully opens the system microphone for
                        // somebody who explicitly asked for MIDI only.
                        self.settings.record_input_off = uid.is_none();
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
                    ColorTarget::RecorderBg => self.settings.recorder_bg_color = rgb,
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
    let piano_h = if settings.show_piano {
        (w as f64 / (1300.0 / 150.0)).trunc() as f32
    } else {
        0.0
    };
    // **The strip yields to the staff.** While the staff element is anywhere
    // on screen it carries the chord name itself — winner and runners-up — and
    // one name in two places is two places for them to disagree. The detached
    // chord window is untouched: that one is an explicit choice.
    // **The strip is a choice, not a consequence.** It used to switch itself
    // off whenever the staff was in the theory band, on the argument that two
    // places to read the chord name is one too many. That is true of the
    // DEFAULT and false as a rule: piano plus strip and nothing else is the
    // shape this app had for years, and some people want exactly that. So it
    // is a setting, off by default, and the staff no longer overrules it.
    let chord_visible = settings.chord_detection_enabled
        && settings.show_chord_strip
        && !settings.chord_window_detached;
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
    // **The camera pane's width, and the theory band's in what is left.**
    //
    // Solved rather than picked, because the two constrain each other: the pane
    // wants to be `CAMERA_PANE_ASPECT` times as wide as the row is tall, and
    // the row is as tall as the theory band wants to be at whatever width the
    // pane leaves it. One equation —
    //
    //     cw = a * k * (w - cw)   =>   cw = w * (a*k) / (1 + a*k)
    //
    // — where `k` is the theory band's height per point of width. Picking a
    // flat fraction instead would make the pane a different shape at every
    // window size, and a preview whose aspect wanders is one you cannot frame
    // a shot in.
    // **Always zero: the pane beside the diagrams is retired.** The camera has
    // one home, the full-height preview at the top-left of the recorder band,
    // which is the inset a take carries. Nothing in the UI can turn this on —
    // no menu row, no `W`, and settings version 7 migrates a stale `true` to
    // false — and leaving the arithmetic live meant a dead feature was still
    // deciding how much height the theory band was allowed to have.
    let _ = settings.show_camera_pane;
    let camera_w = 0.0_f32;
    // Zero while popped out, exactly like the chord strip and the neck above.
    // Without this the diagrams render in BOTH places at once, and the main
    // window keeps 300pt of height for a band that is somewhere else.
    //
    // **Measured at the width the camera pane leaves**, which is what makes any
    // configuration of the theory window fit beside it: `band_height` is linear
    // in width and `cells` divides whatever rect it is handed, so three
    // diagrams in 70% of the window are three narrower diagrams rather than
    // three clipped ones.
    let theory_h = if settings.theory_detached {
        0.0
    } else {
        theory_panel::band_height(w - camera_w, &settings.theory_views())
    };
    // The ROW, which is the taller of the two things in it. With the theory
    // band off or empty the pane still needs its row, or turning the diagrams
    // off would take the camera with them.
    let theory_h = theory_h.max(if camera_w > 0.0 {
        (camera_w / CAMERA_PANE_ASPECT).trunc()
    } else {
        0.0
    });
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
    // **Sixteen by nine, paid for out of the theory band.**
    //
    // Every band's height is a fixed fraction of the width, and the sum of
    // those fractions came to 1.740 — close enough to 16:9 to look like an
    // accident and far enough to letterbox a video. A take is the window now,
    // so the window's shape IS the video's shape, and a standard one costs
    // almost nothing: at 100% the theory band gives up sixteen of its three
    // hundred points, five per cent of the one band whose contents are
    // diagrams that scale to whatever they are given.
    //
    // **Clamped, and the clamp is the point.** It holds while the usual set of
    // bands is up and lets go the moment somebody turns one off: a window with
    // no fretboard would need a theory band half again as tall to stay 16:9,
    // and that is a stretched diagram rather than a standard shape. Then the
    // window is simply what its bands come to, as it always was.
    let theory_h = if theory_h > 0.0 {
        let rest = recorder_h + chord_h + piano_h + fret_h;
        let want = (w * 9.0 / 16.0).trunc() - rest;
        want.clamp(theory_h * 0.75, theory_h * 1.25).trunc()
    } else {
        theory_h
    };
    Bands {
        w,
        recorder_h,
        theory_h,
        camera_w,
        chord_h,
        piano_h,
        fret_h,
    }
}

/// The shape of the camera pane, fixed and not the sensor's.
///
/// **Deliberately not the camera's own aspect.** Every band's height in this
/// app is a pure function of the window's width and of settings, and it has to
/// stay that way: a 4:3 webcam that made the theory row taller than a 16:9 one
/// would resize the window because somebody unplugged a device, which is
/// `docs/RECORDER-PLAN.md` §0's named failure. The frame is letterboxed inside
/// the pane instead — see `fit_preview` — so nothing is cropped away either.
pub const CAMERA_PANE_ASPECT: f32 = 16.0 / 9.0;

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

/// The layout for a screen the app has been given ALL of.
///
/// **Both edges, always.** `fit_bands` preserves every band's natural aspect,
/// which means one dimension runs out first and the other gets bars — correct
/// in a window the user sized, and useless in fullscreen, where bars down both
/// sides are the whole screen you asked for and did not get.
///
/// So: take the full width, then scale every band's height by one factor to
/// make the stack exactly as tall as the screen. ONE factor for all of them, so
/// the bands keep their proportions relative to each other and only their own
/// aspect changes — a slightly taller or shorter keyboard, which is a keyboard,
/// rather than a keyboard next to a fretboard that grew at a different rate.
fn fill_bands(settings: &Settings, avail: Vec2) -> Bands {
    let w = avail.x.max(1.0).trunc();
    let natural = band_sizes_at(settings, w);
    let total = natural.total().y;
    if total <= 0.0 || avail.y <= 0.0 {
        return natural;
    }
    let k = avail.y / total;
    Bands {
        w,
        recorder_h: natural.recorder_h * k,
        theory_h: natural.theory_h * k,
        // Scaled by `k` as well, and it has to be. `k` stretches every band's
        // height to fill the screen, so the theory row ends up `h*k` tall — and
        // a pane whose width did NOT follow stops being 16:9 and starts putting
        // bars round the camera. Caught by rendering a real composited frame:
        // the pane came out 1.32:1 in a 16:9 video.
        //
        // Capped, because `k` is unbounded from above: a window far taller than
        // the layout's natural aspect would otherwise walk the camera across
        // the picture.
        camera_w: (natural.camera_w * k).min(w * 0.45),
        chord_h: natural.chord_h * k,
        piano_h: natural.piano_h * k,
        fret_h: natural.fret_h * k,
    }
}

/// Every note moved by `semitones`, dropping any that leave MIDI's range.
///
/// A free function because it is pure arithmetic on a set and both the display
/// path and the "may we transpose at all?" check need exactly the same answer —
/// the second asks whether the result is the same SIZE as the input, which only
/// works if it is the same function.
fn transposed(notes: &HashSet<u8>, semitones: i64) -> HashSet<u8> {
    if semitones == 0 {
        return notes.clone();
    }
    notes
        .iter()
        .filter_map(|n| u8::try_from(i64::from(*n) + semitones).ok().filter(|m| *m <= 127))
        .collect()
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
    /// The theory ROW: the diagrams and, beside them, the camera pane. The
    /// taller of the two, so switching the diagrams off does not take the
    /// camera with them.
    theory_h: f32,
    /// How much of that row the camera pane takes. Zero when it is off.
    camera_w: f32,
    chord_h: f32,
    piano_h: f32,
    fret_h: f32,
}

impl Bands {
    /// The theory row cut in two: what the diagrams get, and what the camera
    /// gets.
    ///
    /// One function so that the live window, the offscreen compositor and the
    /// hit test all cut it in the same place. Three copies of this arithmetic
    /// is how a click on the camera lands on the circle of fifths behind it.
    fn theory_row(self, row: Rect, left: bool) -> (Rect, Rect) {
        if self.camera_w <= 0.0 {
            return (row, Rect::NOTHING);
        }
        let cut = if left {
            row.left() + self.camera_w
        } else {
            row.right() - self.camera_w
        };
        let a = Rect::from_min_max(row.min, Pos2::new(cut, row.bottom()));
        let b = Rect::from_min_max(Pos2::new(cut, row.top()), row.max);
        let (theory, column) = if left { (b, a) } else { (a, b) };
        // **The pane is 16:9 inside its column, not the column itself.** At the
        // natural size the two are the same rectangle — the width was solved
        // from the row's height precisely so that they would be. They come
        // apart when the layout is stretched to fill a screen, or when the
        // pane's width hits its cap, and there the choice is between a pane
        // that is the wrong shape and a pane that is the right shape with the
        // panel showing above and below it. The panel wins: a preview whose
        // aspect wanders is one you cannot frame a shot in.
        let h = (column.width() / CAMERA_PANE_ASPECT).min(column.height());
        let camera = Rect::from_center_size(
            column.center(),
            Vec2::new(h * CAMERA_PANE_ASPECT, h),
        );
        (theory, camera)
    }

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

    /// Paint the display bands into `rect`, for the video compositor.
    ///
    /// **`&self`, and that is the whole safety argument.** It must not be
    /// [`Self::paint`] called a second time: `paint` drains the MIDI channel,
    /// runs the voicing and detection ticks, and overwrites `last_pane` and
    /// `last_drawn` — which dialog placement and `request_natural_size` read.
    /// A composite pass that did any of that would move the app's own dialogs
    /// to wherever the video frame is, and would consume MIDI events the window
    /// then never saw.
    ///
    /// So this paints what is ALREADY decided. Whatever the last real frame
    /// worked out is what goes in the video, which is also what makes the two
    /// agree: the video shows what the player was looking at.
    ///
    /// The Recorder band is deliberately absent. It is the surface you set up a
    /// take WITH; a recorder's own transport inside the recording is a picture
    /// of the tool rather than of the performance.
    /// The width:height ratio of the display bands the video will contain.
    ///
    /// The compositor needs this BEFORE it lays out the frame, so the band can
    /// be given the height its content actually wants — see `Layout::split`.
    /// Computed from the same `band_sizes_at` the painting uses, so the two
    /// cannot disagree about how tall a keyboard is.
    pub fn composite_aspect(&self, shows: recorder::DisplayShows) -> f32 {
        let s = self.composite_settings(shows);
        // At a nominal width, because every band's height is a fixed fraction
        // of the width — so the RATIO is the same at every size and this needs
        // no knowledge of the frame.
        const NOMINAL: f32 = 1300.0;
        let h = band_sizes_at(&s, NOMINAL).total().y;
        if h <= 0.0 {
            return 0.0;
        }
        NOMINAL / h
    }

    /// The app's settings with the export's panel selection applied.
    ///
    /// One function because the layout and the painting both need it, and a
    /// second copy of this would be the two of them drawing different videos.
    fn composite_settings(&self, shows: recorder::DisplayShows) -> Settings {
        let mut s = self.settings.clone();
        // **A take is the window.** Every band appears in the video exactly
        // when it appears on screen, in the order it appears there, and the
        // ticks in the Export dialog can only take one AWAY. What they cannot
        // do is add one back: a band torn off into its own window is not in
        // the window a take records, and one that is switched off is not there
        // to record.
        //
        // The recorder used to be forced off here, back when the video was an
        // arrangement of its own with the camera floated over it as an inset.
        // It is the band the camera lives in, so leaving it out was leaving
        // the performer out — and putting the camera back by a second route
        // gave two arrangements of one picture to disagree about.
        s.show_recorder = self.settings.show_recorder && !self.settings.recorder_detached;
        s.recorder_detached = false;
        s.chord_detection_enabled = shows.chord && self.settings.chord_detection_enabled;
        s.show_chord_strip =
            self.settings.show_chord_strip && !self.settings.chord_window_detached;
        s.chord_window_detached = false;
        s.show_fretboard =
            shows.fretboard && self.settings.show_fretboard && !self.settings.fretboard_detached;
        s.fretboard_detached = false;
        // **Unticking the piano has to remove its HEIGHT, not just its ink.**
        // Every other flag here maps onto a setting that zeroes a band; the
        // piano had none, so an unticked piano left a piano-sized hole in the
        // middle of the video.
        s.show_piano = shows.piano;
        s.theory_detached = false;
        // The theory band has no "show theory" bool — it exists exactly when
        // something is in it — so the override has to work in both directions
        // by hand.
        // It only ever takes away: a band torn off into its own window is not
        // in the window a take records, and one the number keys collapsed is
        // not there to record either. The old code filled an EMPTY band with
        // all four diagrams when the tick asked for theory, which was the
        // video inventing a layout nobody had on screen.
        if !shows.theory || self.settings.theory_detached {
            s.theory_order = String::new();
        }
        s
    }

    pub fn paint_composite(
        &self,
        painter: &egui::Painter,
        rect: Rect,
        shows: recorder::DisplayShows,
        camera: Option<recorder::Preview>,
    ) {
        if !rect.is_positive() {
            return;
        }
        // The export's OWN panel selection, applied to a copy. This is how the
        // dialog's "Display shows" ticks override the live window without
        // touching it: record a clean piano-and-chord video while keeping the
        // fretboard on screen for yourself.
        let s = self.composite_settings(shows);

        // **FILLED, not fitted.** `fit_bands` preserves the stack's aspect and
        // centres what is left, which is right for a window somebody sized and
        // wrong here: in the default layout the pane IS the whole video frame,
        // so fitting left the app as a strip through the middle with black
        // above and below — the letterbox that made the old default put the
        // camera first in the first place.
        //
        // For the stacked layouts this changes nothing: their pane is already
        // sized to the content by `Layout::band_height`, so the scale factor
        // comes out at one.
        let bands = fill_bands(&s, rect.size());
        let total = bands.total();
        let origin = Pos2::new(
            rect.min.x + ((rect.width() - total.x) * 0.5).max(0.0).trunc(),
            rect.min.y + ((rect.height() - total.y) * 0.5).max(0.0).trunc(),
        );
        let w = bands.w;
        let band_at =
            |top: f32, h: f32| Rect::from_min_size(Pos2::new(origin.x, origin.y + top), Vec2::new(w, h));

        let display = self.display_notes();
        // **The recorder band, first, exactly as the window stacks it.** It
        // was never painted here at all: `composite_settings` forced it off
        // and every offset below counted from the theory band. So a take was
        // the window minus its top band — and the camera, whose only home is
        // that band's preview, had to be composited in by a second route that
        // could put it somewhere the window never does.
        if bands.recorder_h > 0.0 {
            recorder_panel::draw(
                painter,
                band_at(0.0, bands.recorder_h),
                &recorder::RecorderView {
                    // **The compositor's texture, not the window's.** They are
                    // different `egui::Context`s with different atlases, so the
                    // handle the live preview holds means nothing here: it
                    // would draw whatever else carried that id, or nothing.
                    preview: camera,
                    // Nothing is focused and nothing is being typed into a
                    // video: a caret blinking in a recording is a control the
                    // viewer will try to click.
                    name_focused: false,
                    editing: None,
                    fx_units: self.fx_units(),
                    track: &self.track,
                    // The composite's own copy: nothing is focused, nothing
                    // is being typed into and no hand is on a knob, because
                    // there is no pointer in a video frame.
                    ..self.recorder.view(
                        s.record_take_name.as_deref().unwrap_or_default(),
                        false,
                        None,
                        s.knobs(),
                        s.record_hide_elapsed,
                        None,
                    )
                },
                &s,
            );
        }
        if bands.theory_h > 0.0 {
            // The same cut as the live window, from the same function — this
            // is the point of the whole change. What the video shows is what
            // the window shows, camera included, because there is only one
            // arrangement and both paths read it.
            let (theory, pane) = bands.theory_row(band_at(bands.recorder_h, bands.theory_h), s.camera_pane_left);
            if s.theory_views().count() > 0 && !s.theory_detached {
                theory_panel::draw(
                    painter,
                    theory,
                    &s.theory_views(),
                    self.theory_input(&display),
                    &display,
                    self.staff_readout(&s),
                    &s,
                );
            }
            if pane.is_positive() {
                recorder_panel::draw_camera_pane(
                    painter,
                    pane,
                    // **The compositor's texture, not the window's.** They are
                    // different `egui::Context`s with different atlases, so the
                    // handle the live preview holds means nothing in the
                    // offscreen one: it would draw whatever else happened to
                    // carry that id, or nothing at all.
                    camera,
                    self.recorder.camera_label(),
                    theory_panel::band_bg(&s),
                    &s,
                );
            }
        }
        if bands.chord_h > 0.0 && shows.chord {
            chord_strip::draw(
                painter,
                band_at(bands.recorder_h + bands.theory_h, bands.chord_h),
                self.current_chord.as_deref(),
                s.chord_text_color.to_color32(),
                // No heart and no transpose arrows in the video. Both are
                // controls — one is a licence badge and the other is a pair of
                // buttons — and a control painted into a recording is a thing
                // the viewer will try to click.
                None,
                None,
                None,
            );
        }
        if shows.piano {
            piano::draw(
                painter,
                band_at(bands.recorder_h + bands.theory_h + bands.chord_h, bands.piano_h),
                &display,
                self.notes.sustain_down(),
                &s,
            );
        }
        if bands.fret_h > 0.0 {
            let spec = s.fretboard_spec();
            let r = band_at(
                bands.recorder_h + bands.theory_h + bands.chord_h + bands.piano_h,
                bands.fret_h,
            );
            fretboard_panel::draw(
                painter,
                r,
                self.voicing.current(),
                &spec,
                &s,
                s.fretboard_wood(),
                self.barre_to_draw(),
            );
            fretboard_panel::draw_top_edge(painter, r, &s);
        }
    }

    /// The colour behind everything, for hosts that clear the surface
    /// themselves.
    pub const CLEAR_COLOR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn paint(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();

        // **The Welcome card waits for the splash.** It is a separate OS
        // window and the splash is a layer inside this one, so the layer
        // cannot cover it: the card opened on the first frame and sat on the
        // wordmark for the whole launch wait. Promoted the moment the host
        // stops reporting a splash, and only into an empty dialog slot so it
        // can never displace something the user opened first.
        if !self.splash_up && self.dialog.is_none() {
            self.dialog = self.pending_welcome.take();
        }

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
        // The same hook for the take-settings popup, which cannot be
        // photographed any other way: it is dismissed by a press anywhere
        // outside it, and taking a screenshot is a press somewhere outside it.
        //   IVORY_INLINE=setup /Applications/Tangent.app/Contents/MacOS/tangent
        if !self.demo_menu_done && std::env::var("IVORY_INLINE").as_deref() == Ok("setup") {
            self.demo_menu_done = true;
            self.setup_open = true;
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
        // A numeric field joins it for the same reason and one of its own: the
        // transpose arrows are bound to Up and Down, and typing a tempo with
        // those keys live would transpose the chord behind the field.
        if self.dialog.is_none()
            && self.menu_state.is_none()
            && !self.name_focused
            && self.num_edit.is_none()
        {
            if let Some(action) = keys::pressed(&ctx, self.key_gates()) {
                self.apply_key_action(&ctx, action);
            }
        }
        // Every frame, gated or not: the audition is a HOLD, and its release
        // has to run even when the press that started it would no longer be
        // accepted — a dialog opening mid-chord must stop the chord, not
        // abandon it.
        self.audition_tick(&ctx);
        // And everything a gesture wants sounding, for the same reason: every
        // path that can change that set has run by now, and this is the one
        // place that has to notice.
        self.reconcile_sound();

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
        // Fullscreen is its own layout, and it has to be: `layout_sizes` asks
        // for a WINDOW SIZE and gets it, which is meaningless when the window
        // is already the screen. Filling is the point — see `fill_bands`.
        let fullscreen = self.caps.window_sizing
            && ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
        let bands = if fullscreen {
            fill_bands(&self.settings, ui.max_rect().size())
        } else if self.caps.window_sizing {
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
            camera_w: _,
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
        //
        // AND NOT WHILE FULLSCREEN, which is the other half. This app pins the
        // window by setting Min and Max to the SAME size — that is what makes
        // it un-resizable — and a window whose minimum equals its maximum
        // cannot be made the size of the screen. Fullscreen was fighting the
        // window manager every frame; it is not a mode you can bolt onto a
        // fixed-size window without saying so here.
        if self.caps.window_sizing && fullscreen {
            if self.fullscreen_sent != Some(true) {
                // Let it grow. Sent once on the way in rather than every frame,
                // because these are the constraints the OS is trying to honour
                // while it animates into fullscreen.
                ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(Vec2::new(1.0, 1.0)));
                ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(Vec2::new(
                    f32::INFINITY,
                    f32::INFINITY,
                )));
                self.fullscreen_sent = Some(true);
                // So the pin is re-applied on the way back out.
                self.last_sent_size = None;
            }
        } else if self.caps.window_sizing {
            if self.fullscreen_sent == Some(true) {
                self.fullscreen_sent = Some(false);
                self.last_sent_size = None;
            }
            if self.last_sent_size != Some(target) {
                ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(target));
                ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(target));
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
                self.last_sent_size = Some(target);
            }
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
        // Where the thanks card hangs from, once the heart has been drawn.
        let mut heart_rect_for_card: Option<Rect> = None;
        let fret_rect_for_hit: Option<Rect> = (fret_h > 0.0)
            .then(|| band_at(recorder_h + theory_h + chord_h + piano_h, fret_h));
        // The row, then the two halves of it. The camera pane is not a hit
        // target — there is nothing to click on a picture of yourself — so only
        // the diagrams' half goes to the hit test.
        let theory_row = (theory_h > 0.0).then(|| band_at(recorder_h, theory_h));
        let (theory_rect_for_hit, camera_pane_rect) = match theory_row {
            Some(row) => {
                let (t, c) = bands.theory_row(row, self.settings.camera_pane_left);
                (
                    (self.settings.theory_views().count() > 0
                        && !self.settings.theory_detached)
                        .then_some(t),
                    c.is_positive().then_some(c),
                )
            }
            None => (None, None),
        };
        let recorder_rect_for_hit: Option<Rect> =
            (recorder_h > 0.0).then(|| band_at(0.0, recorder_h));
        self.last_band = recorder_rect_for_hit.unwrap_or(Rect::NOTHING);
        if recorder_rect_for_hit.is_none() {
            // A band that is not on screen cannot hold a focused field. Without
            // this, hiding the Recorder with R mid-edit leaves the field open
            // and invisible, still swallowing every single-key shortcut — an
            // app that has apparently stopped responding, with nothing on
            // screen to explain why or to click on to get out of it.
            self.num_edit = None;
            self.name_focused = false;
            self.grabbed = None;
        }
        if let Some(rect) = recorder_rect_for_hit {
            recorder_panel::draw(
                ui.painter(),
                rect,
                &self.recorder_layout_view(),
                &self.settings,
            );
            // **The heart.** It moved here from the chord strip, which is off
            // by default in 5.0 — a heart that hides itself thanks nobody.
            // Drawn by the app rather than by the band because the colour is
            // the app's to know; the band only says where.
            //
            // The CARD it raises is not drawn here. It is taller than the band
            // and hangs down over whatever is below, so it is painted after
            // every other band — see the end of this function.
            if let Some(c) = self.heart_color() {
                let hr = recorder_panel::heart_rect(rect, &self.recorder_layout_view());
                if hr.is_positive() {
                    chord_strip::draw_heart(ui.painter(), hr, c);
                    heart_rect_for_card = Some(hr);
                }
            }
        }
        // The row's own background FIRST, whatever is in it. The camera pane
        // can hold this row open with the diagrams' half empty or detached, and
        // in that case nothing else fills it — leaving the app's black backdrop
        // showing through a band-shaped hole.
        if let Some(row) = theory_row {
            ui.painter()
                .rect_filled(row, 0.0, theory_panel::band_bg(&self.settings));
        }
        self.last_theory = theory_rect_for_hit.unwrap_or(Rect::NOTHING);
        if let Some(theory_rect) = theory_rect_for_hit {
            theory_panel::draw(
                ui.painter(),
                theory_rect,
                &self.settings.theory_views(),
                self.theory_input(&display),
                &display,
                self.staff_readout(&self.settings),
                &self.settings,
            );
        }
        if let Some(pane) = camera_pane_rect {
            recorder_panel::draw_camera_pane(
                ui.painter(),
                pane,
                self.recorder.preview,
                self.recorder.camera_label(),
                theory_panel::band_bg(&self.settings),
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
                // **One heart, wherever it can be.** The recorder band is its
                // home; the strip only carries it when there is no band, which
                // is the piano-and-strip window somebody chose on purpose.
                // Two hearts in one window is two things to click and one
                // colour setting behind them both.
                self.strip_heart_color(),
                None, // attached: it already has the piano below it as an edge
                self.transpose_view(),
            );
            if let Some(hr) = self.strip_heart_color().map(|_| chord_strip::heart_rect(chord_rect))
            {
                heart_rect_for_card = Some(hr);
            }
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

        // **The thanks card, last of everything.** It hangs out of the recorder
        // band and down across the theory band, so painted with its own band it
        // went UNDER the diagrams — egui draws in call order, and the card was
        // being called first. Nothing else in this window overlaps another
        // band, which is why nothing else has to be here.
        //
        // Only in the live window: the compositor draws these same bands into
        // a video, where there is no pointer and a hover card would be a stray
        // note in somebody's recording.
        // **The take-settings popup, in front of everything.** It is modal:
        // it paints a scrim over the whole window, so anything drawn after it
        // would float on top of its own dimming. Only the thanks card comes
        // later, and only because a card raised from a heart you can still see
        // is a card that belongs in front.
        if self.track_open {
            let anchor = self.track_anchor();
            if anchor.is_positive() {
                let typing = |f: recorder::NumField| {
                    self.num_edit
                        .as_ref()
                        .filter(|e| e.field == f)
                        .map(|e| e.text.as_str())
                };
                recorder_panel::draw_track_panel(
                    ui.painter(),
                    recorder_panel::TrackPanel {
                        screen: ui.max_rect(),
                        anchor,
                        track: &self.track,
                        from: self.settings.track_in,
                        to: self.settings.track_out,
                        typing: (
                            typing(recorder::NumField::TrackIn),
                            typing(recorder::NumField::TrackOut),
                        ),
                    },
                    &self.settings,
                );
            } else {
                self.track_open = false;
            }
        }
        // The effect panels, on the same terms and for the same reasons.
        if let Some(fx) = self.fx_open {
            let anchor = self.fx_anchor(fx);
            if anchor.is_positive() {
                recorder_panel::draw_fx(
                    ui.painter(),
                    ui.max_rect(),
                    anchor,
                    fx,
                    &|key| self.effect_param(key),
                    &|key| self.choice_label(key),
                    &self.settings,
                );
            } else {
                // No knob, no panel. The band can be hidden or too small to
                // draw knobs in, and a panel left latched over nothing is a
                // panel with no way back to the control it belongs to.
                self.fx_open = None;
            }
        }
        if self.setup_open {
            if let Some(rect) = recorder_rect_for_hit {
                let view = self.recorder_layout_view();
                recorder_panel::draw_setup(
                    ui.painter(),
                    ui.max_rect(),
                    recorder_panel::setup_rect(rect, &view),
                    &view,
                    &self.settings,
                );
            } else {
                // No band, no cog, no popup. Nothing can have opened it, and
                // leaving it latched would put a panel on screen with no way
                // back to the thing it belongs to.
                self.setup_open = false;
            }
        }
        if let Some(hr) = heart_rect_for_card {
            if ctx.pointer_latest_pos().is_some_and(|p| hr.contains(p)) {
                chord_strip::draw_thanks(ui.painter(), hr, ui.max_rect(), self.settings.dark_mode);
            }
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
        if let (Some(rect), Some(grab)) = (recorder_rect_for_hit, self.grabbed) {
            let (down, pos) = ctx.input(|i| (i.pointer.primary_down(), i.pointer.interact_pos()));
            if !down {
                self.grabbed = None;
                // Released without ever moving: a TAP, which opens the control
                // for typing rather than setting it to wherever the pointer is.
                if !grab.moved {
                    let double = ctx.input(|i| {
                        i.pointer.button_double_clicked(egui::PointerButton::Primary)
                    });
                    if grab.hit.is_knob() {
                        // **A knob resets on a double click and does nothing
                        // on a single one.** Nothing, because the first click
                        // of a double click is a single one: a knob that
                        // opened a text box under every tap could never be
                        // reset by one. Typing into a knob is the right-click.
                        if double {
                            if let Some(reset) = grab.hit.reset_to() {
                                self.apply_recorder_hit(reset);
                                self.num_edit = None;
                                self.save_settings();
                            }
                        }
                    } else if let Some(field) = recorder_panel::num_field(grab.hit) {
                        // Everything else still opens for typing on a tap. A
                        // fader is a long thin thing you can put a pointer on
                        // exactly, and it has no second gesture to protect.
                        self.num_edit = Some(recorder::NumEdit::new(field));
                        self.name_focused = false;
                    }
                }
            } else if let Some(pos) = pos {
                if (pos - grab.from).length() > TAP_SLOP {
                    // Latched: this gesture is a drag now and stays one.
                    if let Some(g) = self.grabbed.as_mut() {
                        g.moved = true;
                    }
                    // A drag beats a half-typed number in the same control —
                    // you cannot be typing into a fader you are hauling.
                    self.num_edit = None;
                }
                // Only once it IS a drag. A press that has not moved yet sets
                // nothing, so that letting go of it can mean something else.
                if self.grabbed.is_some_and(|g| g.moved) {
                    let view = self.recorder_layout_view();
                    // **Every control travels by how far the hand has MOVED**,
                    // not by where it ended up. The pointer is free to leave
                    // the control, leave the band, and go on turning it — and
                    // a press never makes a handle jump to meet it.
                    //
                    // A fader's travel is its own track, so it still feels
                    // one-to-one; a knob's is `KNOB_TRAVEL`, which is far wider
                    // than the knob. Holding the fine modifier makes either six
                    // times slower, which is what puts every readable decibel
                    // within reach of a hand.
                    let axis = recorder_panel::drag_axis(rect, &view, grab.hit);
                    if let Some(axis) = axis {
                        let travel = recorder_panel::drag_travel(rect, &view, grab.hit)
                            .unwrap_or(KNOB_TRAVEL);
                        let moved = match axis {
                            recorder_panel::DragAxis::Vertical => grab.from.y - pos.y,
                            recorder_panel::DragAxis::Horizontal => pos.x - grab.from.x,
                        };
                        let fine = if ctx.input(|i| i.modifiers.shift) {
                            recorder_panel::FINE_DRAG
                        } else {
                            1.0
                        };
                        let v = (grab.from_value + moved / travel * fine).clamp(0.0, 1.0);
                        self.apply_recorder_hit(grab.hit.with_value(v));
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
        if self.num_edit.is_some() {
            self.edit_number(&ctx);
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
            let held = self.display_notes();
            let outcome = theory_panel::show_detached_window(
                &ctx,
                self.theory_builder_size,
                self.theory_builder_pos,
                self.settings.borderless_mode,
                self.main_focused,
                &self.settings.theory_views(),
                self.theory_input(&held),
                &held,
                self.staff_readout(&self.settings),
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
            let view = self.recorder_layout_view();
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
                self.transpose_view(),
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
            //
            // **Translated into MONITOR coordinates**, which is what
            // `Placement::parent` is documented to be and what it was not.
            // `last_drawn` comes from `ui.max_rect()`, so in a viewport it
            // starts at the window's own origin — near (0, 0) — and handing it
            // over raw centred every dialog on a rectangle sitting in the
            // top-left of the SCREEN. On this machine that put the welcome card
            // at x = (1300 - 470) / 2 = 415, which looks deliberate and is a
            // window-sized coincidence.
            parent: self
                .main_origin_known
                .then(|| self.last_drawn.translate(self.main_inner_origin.to_vec2()))
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


    /// **One chord name on screen, not two.**
    ///
    /// The staff panel carries the chord readout, so while it is anywhere in
    /// the theory band the strip's height goes to zero — and comes back the
    /// moment the staff leaves, because then the strip is the only place the
    /// name lives. The detached chord window is a third place and an explicit
    /// choice; it wins over both.
    #[test]
    fn the_chord_strip_is_a_choice_the_staff_does_not_overrule() {
        // `first_launch`, not `default`: bare defaults leave the band empty
        // and visibility is exactly what a first launch decides.
        let mut s = Settings::first_launch();
        assert!(
            s.theory_views().contains(theory_panel::View::Staff),
            "a first launch no longer includes the staff; this test needs rethinking"
        );
        // Off out of the box, because the staff prints the chord name itself
        // and two places to read it is one too many BY DEFAULT.
        assert_eq!(
            band_sizes_at(&s, 1300.0).chord_h,
            0.0,
            "the strip is up on a first launch"
        );
        // Asked for, it comes up — WITH the staff still there. It used to
        // suppress itself in that case, which made piano-plus-strip, the shape
        // this app had for years, unreachable without dismantling the theory
        // band.
        s.show_chord_strip = true;
        assert!(
            band_sizes_at(&s, 1300.0).chord_h > 0.0,
            "the strip was asked for and the staff overruled it"
        );
        // And it is still there once the staff goes, which is the case that
        // matters most: then the strip is the only place the name lives.
        s.toggle_theory_view(theory_panel::View::Staff);
        assert!(
            band_sizes_at(&s, 1300.0).chord_h > 0.0,
            "the staff left and took the strip with it"
        );
        // Detection off beats everything: no detector, no name anywhere.
        s.chord_detection_enabled = false;
        assert_eq!(band_sizes_at(&s, 1300.0).chord_h, 0.0);
    }

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

    // ── the built-in's patch picker ─────────────────────────────────────────

    /// **One gesture, two instruments.** Clicking a slot opens the VST3's own
    /// editor; the built-in has no window, so the same click has to open the
    /// patch picker instead. If this ever sends `OpenPluginEditor` for the
    /// built-in, the click does nothing at all and the built-in becomes an
    /// instrument with no way to change its sound.
    #[test]
    fn opening_the_builtin_shows_patches_and_a_vst_shows_its_own_editor() {
        let (_, mut app) = headless(Caps::DESKTOP);
        app.settings.plugin_slots[0] = Some(dialogs::BUILTIN_PATH.to_owned());
        app.settings.plugin_slots[1] = Some("/x/Pianoteq 8.vst3".to_owned());

        app.apply_recorder_hit(recorder_panel::Hit::OpenSlotEditor(0));
        assert!(
            matches!(app.dialog, Some(dialogs::Dialog::PatchPicker { slot: 0, .. })),
            "the built-in did not open its patches"
        );
        // And nothing was asked of the host: there is no window to open.
        assert!(app.take_recorder_request().is_none());

        app.dialog = None;
        app.apply_recorder_hit(recorder_panel::Hit::OpenSlotEditor(1));
        assert!(app.dialog.is_none(), "a VST3 does not use the patch picker");
        assert_eq!(
            app.take_recorder_request(),
            Some(recorder::RecorderRequest::OpenPluginEditor(1))
        );
    }

    /// A cartridge arriving updates the OPEN picker rather than replacing it,
    /// and clears a selection that belonged to the bank before it.
    #[test]
    fn loading_a_cartridge_refills_the_open_picker() {
        let (_, mut app) = headless(Caps::DESKTOP);
        app.settings.plugin_slots[0] = Some(dialogs::BUILTIN_PATH.to_owned());
        app.apply_recorder_hit(recorder_panel::Hit::OpenSlotEditor(0));

        app.set_cartridge(crate::ports::CartridgeInfo {
            bank: "ROM1A".to_owned(),
            bad_checksum: true,
            voices: (0..32).map(|i| format!("PATCH {i}")).collect(),
            error: String::new(),
        });
        let Some(dialogs::Dialog::PatchPicker {
            bank,
            bad_checksum,
            voices,
            selected,
            error,
            ..
        }) = &app.dialog
        else {
            panic!("the picker closed when a cartridge loaded")
        };
        assert_eq!(bank, "ROM1A");
        assert!(*bad_checksum, "a bad checksum has to be shown, not hidden");
        assert_eq!(voices.len(), 32);
        assert!(error.is_empty());
        // Nothing is selected: patch 12 of the old bank is a different sound.
        assert_eq!(*selected, None);
    }

    /// **A file that will not parse must not cost you the one that did.** The
    /// error goes into the dialog and the working cartridge stays loaded —
    /// somebody browsing a folder of ten thousand `.syx` will hit a bad one.
    #[test]
    fn a_cartridge_that_fails_leaves_the_good_one_alone() {
        let (_, mut app) = headless(Caps::DESKTOP);
        app.settings.plugin_slots[0] = Some(dialogs::BUILTIN_PATH.to_owned());
        app.apply_recorder_hit(recorder_panel::Hit::OpenSlotEditor(0));
        app.set_cartridge(crate::ports::CartridgeInfo {
            bank: "ROM1A".to_owned(),
            voices: (0..32).map(|i| format!("PATCH {i}")).collect(),
            ..Default::default()
        });
        app.set_cartridge(crate::ports::CartridgeInfo {
            error: "this is a single-voice dump, not a 32-voice cartridge".to_owned(),
            ..Default::default()
        });
        let Some(dialogs::Dialog::PatchPicker {
            bank, voices, error, ..
        }) = &app.dialog
        else {
            panic!("the picker closed")
        };
        assert_eq!(bank, "ROM1A", "the working cartridge was thrown away");
        assert_eq!(voices.len(), 32);
        assert!(error.contains("single-voice"), "the reason was not shown");
    }

    /// Picking a patch asks the host for it and remembers it, and the built-in
    /// row asks for the patch compiled in.
    #[test]
    fn picking_a_patch_plays_it_and_is_remembered() {
        let (_, mut app) = headless(Caps::DESKTOP);
        app.apply_dialog_action(dialogs::DialogAction::ChoosePatch { slot: 1, index: 12 });
        assert_eq!(
            app.take_recorder_request(),
            Some(recorder::RecorderRequest::ChoosePatch { slot: 1, index: 12 })
        );
        assert_eq!(app.settings.dx7_patch, 12);

        // The built-in row. `usize::MAX` is not an index into anything, so it
        // must not be written to the settings as one.
        app.apply_dialog_action(dialogs::DialogAction::ChoosePatch {
            slot: 1,
            index: usize::MAX,
        });
        assert_eq!(
            app.take_recorder_request(),
            Some(recorder::RecorderRequest::ChoosePatch {
                slot: 1,
                index: usize::MAX
            })
        );
        assert_eq!(app.settings.dx7_patch, 0);
    }

    /// A plugin host never raises a file panel, so it never asks for one.
    #[test]
    fn a_plugin_does_not_ask_for_a_cartridge_file() {
        let (_, mut app) = headless(Caps::PLUGIN);
        app.apply_dialog_action(dialogs::DialogAction::LoadCartridge);
        assert!(app.take_file_request().is_none());

        let (_, mut app) = headless(Caps::DESKTOP);
        app.apply_dialog_action(dialogs::DialogAction::LoadCartridge);
        let r = app.take_file_request().expect("the desktop asks");
        assert_eq!(r.purpose, crate::ports::FilePurpose::Cartridge);
        assert!(r.extensions.iter().any(|e| e.eq_ignore_ascii_case("syx")));
    }

    /// The cog opens the take-settings popup, and the popup can be dismissed.
    ///
    /// **The regression this exists for**: `Hit::OpenSetup` set a flag, and
    /// nothing anywhere read the flag. The button was the only way to reach
    /// the settings that left the band in 5.0, and pressing it did nothing —
    /// which is exactly what "unresponsive" looks like from the outside.
    #[test]
    fn the_cog_opens_the_take_settings_popup_and_it_can_be_closed() {
        let (_, mut app) = headless_with(Caps::DESKTOP, Settings::first_launch());
        let w = main_width(&app.settings);
        let band = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, recorder_panel::band_height(w)));
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w * 0.7));

        // The cog is really there, and pressing it is really a Setup hit.
        let cog = recorder_panel::setup_rect(band, &app.recorder_layout_view());
        assert!(cog.is_positive(), "the band has no cog");
        assert_eq!(
            recorder_panel::hit_test(band, &app.recorder_layout_view(), cog.center()),
            Some(recorder_panel::Hit::OpenSetup),
            "the cog's own centre is not a Setup hit"
        );

        assert!(!app.setup_open);
        app.apply_recorder_hit(recorder_panel::Hit::OpenSetup);
        assert!(app.setup_open, "the cog did not open the popup");

        // The panel is on screen and has room for its controls.
        let panel = recorder_panel::setup_popup_rect(screen, cog);
        assert!(panel.is_positive(), "the popup has no rectangle");
        assert!(screen.contains_rect(panel), "the popup hangs off the window");

        // Every control in it is reachable, and each is a DIFFERENT control:
        // a panel where two boxes answer to the same press is a panel where
        // one of them is dead.
        let view = app.recorder_layout_view();
        let mut seen: Vec<recorder_panel::Hit> = Vec::new();
        let (mut y, step) = (panel.top(), 2.0_f32);
        while y <= panel.bottom() {
            let mut x = panel.left();
            while x <= panel.right() {
                if let Some(h) =
                    recorder_panel::setup_hit_test(screen, cog, &view, Pos2::new(x, y))
                {
                    if !seen.contains(&h) {
                        seen.push(h);
                    }
                }
                x += step;
            }
            y += step;
        }
        assert!(
            seen.len() >= 12,
            "only {} controls are reachable in the popup: {:?}",
            seen.len(),
            seen.iter().map(|h| h.label()).collect::<Vec<_>>()
        );
        assert!(seen.contains(&recorder_panel::Hit::CloseSetup), "no way out");
        assert!(seen.contains(&recorder_panel::Hit::ChooseFolder));
        assert!(seen.contains(&recorder_panel::Hit::ToggleHideElapsed));

        // DONE closes it, and so does the Setup hit coming round again.
        app.apply_recorder_hit(recorder_panel::Hit::CloseSetup);
        assert!(!app.setup_open, "DONE did not close the popup");
    }

    /// The heart is drawn for everybody, in the band, out of everything's way.
    ///
    /// It used to live in the chord strip and to be grey unless a key had been
    /// bought. The strip is off by default in 5.0, which left it nowhere at
    /// all — and nothing in this app is behind a key while 5.0 is in beta.
    #[test]
    fn the_heart_is_visible_to_everyone_and_clear_of_the_slots() {
        let (_, app) = headless_with(Caps::DESKTOP, Settings::first_launch());
        // **Whatever the licence says.** This runs on developer machines that
        // hold a key and on CI machines that do not, and the answer has to be
        // the same on both: there is nothing behind a key at all.
        assert!(
            app.heart_color().is_some(),
            "the heart is hidden (supporter: {})",
            app.license.is_supporter()
        );

        for pct in [50_i64, 100, 200] {
            let mut s = Settings::first_launch();
            s.window_size_percent = pct;
            let w = main_width(&s);
            let band =
                Rect::from_min_size(Pos2::ZERO, Vec2::new(w, recorder_panel::band_height(w)));
            let view = app.recorder_layout_view();
            let heart = recorder_panel::heart_rect(band, &view);
            assert!(heart.is_positive(), "no heart at {pct}%");
            assert!(band.contains_rect(heart), "the heart escaped the band at {pct}%");
            // And it takes no click that belongs to a control.
            assert_eq!(
                recorder_panel::hit_test(band, &view, heart.center()),
                None,
                "the heart is sitting on a control at {pct}%"
            );
        }
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

    /// The release edge for anything Space was holding.
    ///
    /// An explicit release EVENT, not merely a frame with no events: egui keeps
    /// a key down until it is told otherwise, which is right for real input and
    /// means a quiet frame is not a let-go.
    fn key_up(ctx: &egui::Context, app: &mut IvoryApp) -> Option<recorder::RecorderRequest> {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0))),
                events: vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed: false,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
        app.take_recorder_request()
    }

    /// **A vertex of the harmonic triangles SWITCHES chords.**
    ///
    /// What anybody does with that view is compare: press C, then F, then G.
    /// Adding each to the last builds a nine-note cluster and calls it a
    /// chord. Pressing the same one again still takes it off, which is what
    /// keeps it a toggle rather than a radio button with no way out.
    #[test]
    fn a_chord_vertex_replaces_the_chord_before_it() {
        let (_, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        let pcs = |app: &IvoryApp| {
            let mut v: Vec<u8> = app.manual_notes.iter().map(|n| n % 12).collect();
            v.sort_unstable();
            v.dedup();
            v
        };

        app.toggle_theory_hit(theory_panel::Hit::Triad {
            root: 0,
            minor: false,
        });
        assert_eq!(pcs(&app), vec![0, 4, 7], "C major did not go in");

        app.toggle_theory_hit(theory_panel::Hit::Triad {
            root: 5,
            minor: false,
        });
        assert_eq!(pcs(&app), vec![0, 5, 9], "F major did not replace C major");

        // The same one again comes off, and leaves nothing behind.
        app.toggle_theory_hit(theory_panel::Hit::Triad {
            root: 5,
            minor: false,
        });
        assert!(pcs(&app).is_empty(), "pressing F again did not clear it");
    }

    /// **And a pitch class does not**, because that is the opposite gesture:
    /// the circle and the lattice are how a chord is built one note at a time.
    #[test]
    fn a_lattice_node_adds_rather_than_replacing() {
        let (_, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        for pc in [0, 4, 7] {
            app.toggle_theory_hit(theory_panel::Hit::Pc(pc));
        }
        let mut got: Vec<u8> = app.manual_notes.iter().map(|n| n % 12).collect();
        got.sort_unstable();
        assert_eq!(got, vec![0, 4, 7], "the notes replaced each other");
    }

    /// **A click lands on the key that was clicked, whatever the transpose.**
    ///
    /// The regression this exists for: with a transpose of -11 set, clicking
    /// middle C lit C sharp a major seventh below it. Placed notes live in the
    /// space they were clicked in, and the display transform was reaching them
    /// a second time.
    #[test]
    fn a_clicked_key_lights_the_key_that_was_clicked() {
        for transpose in [-11_i64, 0, 5] {
            let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
            app.settings.keytoggle_enabled = true;
            app.settings.transpose = transpose;

            app.place_or_play(60);
            app.clicked.clear();
            let _ = run_frame(&ctx, &mut app);
            assert!(
                app.display_notes().contains(&60),
                "transpose {transpose}: clicking middle C lit {:?}",
                app.display_notes()
            );
            assert_eq!(app.display_notes().len(), 1, "transpose {transpose}");
        }
    }

    /// **And the arrow keys still move what was placed.** The two halves pull
    /// against each other: a click must not be re-projected, and a transpose
    /// must still carry a chord somebody built by clicking.
    #[test]
    fn the_arrow_keys_carry_a_placed_chord_along() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.place_or_play(60);
        app.place_or_play(64);
        // Let go: a note still under the button is sounding, and a sounding
        // note lights whatever pitch it was struck at until it stops.
        app.clicked.clear();
        let _ = run_frame(&ctx, &mut app);
        assert_eq!(
            app.display_notes(),
            [60, 64].into_iter().collect::<HashSet<u8>>()
        );

        app.transpose_by(2);
        assert_eq!(app.settings.transpose, 2);
        assert_eq!(
            app.display_notes(),
            [62, 66].into_iter().collect::<HashSet<u8>>(),
            "the placed chord did not come along"
        );
        // And back again, exactly.
        app.transpose_by(-2);
        assert_eq!(
            app.display_notes(),
            [60, 64].into_iter().collect::<HashSet<u8>>(),
            "the chord did not come back where it started"
        );
    }

    /// **The staff is an instrument now.** It was a readout, and said so in a
    /// comment: a note on it is a note you are already holding. That stopped
    /// being true when every other view became playable — a pianist reading a
    /// chord off the staff should be able to put one back.
    #[test]
    fn a_click_on_the_staff_sounds_the_note_that_is_drawn_there() {
        for keytoggle in [false, true] {
            let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
            app.settings.keytoggle_enabled = keytoggle;
            app.settings.theory_order = theory_panel::View::Staff.key().to_owned();
            let _ = run_frame(&ctx, &mut app);

            // Middle line of the treble staff, which is B4.
            let band = app.last_theory;
            assert!(band.is_positive(), "the theory band is not showing");
            let (_, cell) = theory_panel::cells(band, &app.settings.theory_views())
                .into_iter()
                .find(|(v, _)| *v == theory_panel::View::Staff)
                .expect("the staff has a cell");
            let body = theory_panel::staff_body(cell);
            let want = staff::hit_test(
                body,
                app.settings.chord_detection_enabled,
                &app.settings,
                body.center(),
            );
            let Some(want) = want else {
                panic!("nothing is drawn at the centre of the staff")
            };
            assert_eq!(
                app.staff_note_at(band, body.center()),
                Some(want),
                "the app asked a different staff than the panel drew"
            );

            app.place_or_play(want);
            assert_eq!(
                run_frame(&ctx, &mut app),
                Some(recorder::RecorderRequest::Audition {
                    notes: vec![want],
                    on: true
                }),
                "keytoggle {keytoggle}: clicking the staff made no sound"
            );
        }
    }

    /// A triad from the harmonic triangles arrives as a TRIAD, in both modes.
    /// Clicking a chord and hearing its root alone is the wrong instrument.
    #[test]
    fn a_chord_from_the_theory_band_sounds_whole() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.sound_while_held(theory_pitches(theory_panel::Hit::Triad {
            root: 0,
            minor: false,
        }));
        assert_eq!(
            run_frame(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition {
                notes: vec![60, 64, 67],
                on: true
            }),
            "a C major vertex did not sound as C major"
        );
    }

    /// **What keytoggle latches is visual, and only visual.**
    ///
    /// A note-on with no note-off is a patch ringing for ever, and on an organ
    /// or a pad that is exactly what it sounds like. The owner asked for this
    /// after hearing it: the toggle decides whether the note stays LIT, not
    /// whether the app holds it down. What sounds is a gesture that ends — a
    /// button still down, or Space.
    #[test]
    fn a_latched_note_is_lit_but_does_not_ring() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;

        app.manual_notes.insert(64);
        assert!(
            run_frame(&ctx, &mut app).is_none(),
            "a latched note held the instrument down"
        );
        // It IS on screen, which is the half of it the toggle is for.
        assert!(app.display_notes().contains(&64));
        // Frame after frame, still nothing: no note-on, and nothing to release.
        assert!(run_frame(&ctx, &mut app).is_none());
        app.manual_notes.clear();
        assert!(run_frame(&ctx, &mut app).is_none());
    }

    /// **A click sounds in both modes, and stops when the button comes up.**
    /// The toggle decides what is left on screen, not whether pressing a key
    /// makes a sound.
    #[test]
    fn a_click_sounds_while_the_button_is_down_in_either_mode() {
        for keytoggle in [false, true] {
            let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
            app.settings.keytoggle_enabled = keytoggle;

            app.place_or_play(67);
            assert_eq!(
                run_frame(&ctx, &mut app),
                Some(recorder::RecorderRequest::Audition {
                    notes: vec![67],
                    on: true
                }),
                "keytoggle {keytoggle}: the click made no sound"
            );
            // Letting go stops it, wherever the pointer ended up.
            app.clicked.clear();
            assert_eq!(
                run_frame(&ctx, &mut app),
                Some(recorder::RecorderRequest::Audition {
                    notes: vec![67],
                    on: false
                }),
                "keytoggle {keytoggle}: the note went on ringing"
            );
            // And only with the toggle ON is it still lit afterwards.
            assert_eq!(
                app.display_notes().contains(&67),
                keytoggle,
                "keytoggle {keytoggle}: the wrong thing was left on screen"
            );
        }
    }

    /// Switching keytoggle off takes the notes off the screen. Nothing is
    /// sounding to release — see `a_latched_note_is_lit_but_does_not_ring`.
    #[test]
    fn turning_keytoggle_off_clears_what_it_was_showing() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.manual_notes.extend([60, 64]);
        let _ = run_frame(&ctx, &mut app);
        assert!(app.display_notes().contains(&60));

        app.settings.keytoggle_enabled = false;
        assert!(run_frame(&ctx, &mut app).is_none());
        assert!(!app.display_notes().contains(&60), "the notes stayed lit");
    }

    /// **A take records a performance, and placing a voicing is part of one.**
    /// Nothing is silenced when Record is pressed, because the latch was never
    /// making a sound — and a click during a take sounds like any other.
    #[test]
    fn a_rolling_take_does_not_silence_a_click() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.recorder.state = recorder::RecordState::Rolling;
        app.place_or_play(60);
        assert_eq!(
            run_frame(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition {
                notes: vec![60],
                on: true
            }),
            "a note clicked during a take made no sound"
        );
    }

    /// **Space is what plays a latched chord**, and it is a strike.
    ///
    /// The latch is silent by design, so Space is not a nicety here: it is the
    /// only way to hear a voicing built up by clicking. Down sounds the whole
    /// lit set, up releases it, and pressing it again strikes it again — which
    /// on a decaying instrument is the entire point.
    #[test]
    fn space_strikes_the_chord_that_keytoggle_is_showing() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.manual_notes.extend([60, 64]);
        assert!(
            run_frame(&ctx, &mut app).is_none(),
            "placing the chord rang it"
        );

        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition {
                notes: vec![60, 64],
                on: true
            }),
            "Space did not sound what was lit"
        );
        assert_eq!(
            key_up(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition {
                notes: vec![60, 64],
                on: false
            }),
            "letting go of Space did not release the chord"
        );
        // Again, from the top.
        assert!(matches!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition { on: true, .. })
        ));
    }


    /// Holding Space is ONE strike. Key auto-repeat fires many times a second,
    /// and a chord re-attacked at the repeat rate is a machine gun.
    #[test]
    fn holding_space_strikes_once() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.manual_notes.insert(64);
        let _ = run_frame(&ctx, &mut app);
        let _ = space(&ctx, &mut app);
        // Drain the strike's second half.
        let _ = app.take_recorder_request();
        // Now hold it, with no new key events at all.
        for _ in 0..5 {
            assert!(
                run_frame(&ctx, &mut app).is_none(),
                "holding Space struck the chord again"
            );
        }
    }

    /// A transposed note sounds and lights at the transposed pitch, once.
    ///
    /// The regression: `sounding` held display pitches and was then transposed
    /// a SECOND time on its way into `display_notes`, lighting phantom keys an
    /// interval above the ones being played.
    #[test]
    fn transpose_is_applied_once_to_what_is_sounding() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.keytoggle_enabled = true;
        app.settings.transpose = 2;
        // **Placed where it was clicked, and it stays there.** A transpose
        // that is already set does not move a note somebody then places: the
        // key they pressed is the key that lights. The arrow keys move placed
        // notes explicitly — see `transpose_by`.
        app.manual_notes.insert(60);
        let _ = run_frame(&ctx, &mut app);
        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Audition {
                notes: vec![60],
                on: true
            }),
            "a placed note did not sound where it was placed"
        );
        let lit = app.display_notes();
        assert_eq!(
            lit,
            [60].into_iter().collect::<HashSet<u8>>(),
            "a phantom key lit an interval away from the one playing"
        );
    }

    /// **A knob turns by distance, not by position.** Its own cell is thirty
    /// points tall; mapping that to the whole range is three percent a pixel,
    /// which is the control the owner could not land on a number. The pointer
    /// has to be able to leave the knob, leave the band, and go on turning.
    ///
    /// Driven through real pointer events rather than by building a `Grab` by
    /// hand, because the half of this that broke first was the gesture and not
    /// the arithmetic.
    #[test]
    fn a_knob_turns_by_how_far_the_hand_moved_and_not_by_where_it_is() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.reverb_mix = 0.5;

        let cell = knob_cell(&app, recorder_panel::Hit::SetFx(recorder_panel::Fx::Reverb, 0.0));
        let from = cell.center();
        press(&ctx, &mut app, from);
        assert!(
            app.grabbed
                .is_some_and(|g| g.hit.is_same_control(recorder_panel::Hit::SetFx(recorder_panel::Fx::Reverb, 0.0))),
            "pressing the knob did not grab it"
        );

        // A quarter of the travel UP is a quarter more, wherever that lands —
        // and it lands far outside the knob, which is the point.
        let to = Pos2::new(from.x, from.y - KNOB_TRAVEL * 0.25);
        assert!(
            !cell.contains(to),
            "the test exercises nothing: {KNOB_TRAVEL} fits inside the knob"
        );
        move_to(&ctx, &mut app, to);
        assert!(
            (app.settings.reverb_mix - 0.75).abs() < 0.01,
            "a quarter of the travel gave {}",
            app.settings.reverb_mix
        );

        // And the knob says so while the hand is on it: a number, not a name.
        assert_eq!(
            app.recorder_layout_view().turning,
            Some(recorder::NumField::Fx(recorder_panel::Fx::Reverb)),
            "a knob being turned does not show its reading"
        );
    }

    /// **Every readable decibel is reachable by hand.** A fader spans seventy-
    /// two decibels and reads to a tenth of one; over its own track that is
    /// four tenths of a decibel per point, so half the numbers it can display
    /// could not be landed on. The fine modifier is what closes that.
    #[test]
    fn a_fader_can_be_landed_on_any_tenth_of_a_decibel() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        let from = {
            let band = app.last_band;
            let v = app.recorder_layout_view();
            let (_, track, _) = recorder_panel::fader_zones(
                recorder_panel::metronome_row(band, &v).expect("the click fader is there"),
            );
            track.center()
        };
        press(&ctx, &mut app, from);
        let travel = {
            let v = app.recorder_layout_view();
            recorder_panel::drag_travel(
                app.last_band,
                &v,
                recorder_panel::Hit::SetMetronomeGain(0.0),
            )
            .expect("the fader travels")
        };

        // Past the tap slop first — under it a press is still on its way to
        // being a tap and sets nothing, which is the point of the slop.
        move_to_fine(&ctx, &mut app, Pos2::new(from.x + TAP_SLOP + 6.0, from.y));
        // Now one more point, held fine. The step it produces has to be smaller
        // than the tenth of a decibel the reading shows, or there are values on
        // screen no hand can reach.
        let before = app.settings.metronome_gain;
        move_to_fine(&ctx, &mut app, Pos2::new(from.x + TAP_SLOP + 7.0, from.y));
        let db = |g: f64| 20.0 * (g as f32).max(1e-9).log10();
        let step = (db(app.settings.metronome_gain) - db(before)).abs();
        assert!(
            step > 0.0 && step < 0.1,
            "one fine point moved the fader {step:.3} dB, and it reads to 0.1"
        );

        // And the ordinary gesture still crosses the whole track in a track's
        // width, or a fader has stopped feeling like a fader.
        let (_, mut app) = (0, app);
        app.settings.metronome_gain = 0.0;
        app.grabbed = Some(Grab {
            hit: recorder_panel::Hit::SetMetronomeGain(0.0),
            from,
            moved: true,
            from_value: 0.0,
        });
        move_to(&ctx, &mut app, Pos2::new(from.x + travel, from.y));
        assert!(
            app.settings.metronome_gain > 3.9,
            "a full track's drag reached only {}",
            app.settings.metronome_gain
        );
    }

    /// The tempo is a knob now, turned like the sends, and typed into on a
    /// double click rather than a tap — it is turned far more often than it is
    /// typed.
    #[test]
    fn the_tempo_knob_turns_and_opens_on_a_double_click() {
        let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
        app.settings.record_export.tempo_bpm = 120.0;
        let from = knob_cell(&app, recorder_panel::Hit::SetTempo(0.0)).center();

        press(&ctx, &mut app, from);
        assert!(
            app.grabbed
                .is_some_and(|g| g.hit.is_same_control(recorder_panel::Hit::SetTempo(0.0))),
            "pressing the tempo knob did not grab it"
        );
        // A single tap does NOT open the field.
        release(&ctx, &mut app, from);
        assert!(app.num_edit.is_none(), "a tap opened the tempo box");

        // Turning it up a tenth of the travel raises the tempo.
        press(&ctx, &mut app, from);
        move_to(&ctx, &mut app, Pos2::new(from.x, from.y - KNOB_TRAVEL * 0.1));
        assert!(
            app.settings.record_export.tempo_bpm > 140.0,
            "a tenth of the travel gave {}",
            app.settings.record_export.tempo_bpm
        );
    }

    /// One frame with the pointer released at `pos`.
    fn release(ctx: &egui::Context, app: &mut IvoryApp, pos: Pos2) {
        pointer_frame(
            ctx,
            app,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    }

    /// A move with the fine modifier held.
    fn move_to_fine(ctx: &egui::Context, app: &mut IvoryApp, pos: Pos2) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0))),
                modifiers: egui::Modifiers::SHIFT,
                events: vec![egui::Event::PointerMoved(pos)],
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
    }

    /// Both ends are reachable, and neither wraps.
    #[test]
    fn a_knob_stops_at_both_ends_of_its_travel() {
        for (start, push, want) in [(0.5_f64, -3.0_f32, 1.0_f64), (0.5, 3.0, 0.0)] {
            let (ctx, mut app) = headless_with_band(Caps::DESKTOP);
            app.settings.delay_mix = start;
            let from = knob_cell(&app, recorder_panel::Hit::SetFx(recorder_panel::Fx::Delay, 0.0)).center();
            press(&ctx, &mut app, from);
            move_to(
                &ctx,
                &mut app,
                Pos2::new(from.x, from.y + KNOB_TRAVEL * push),
            );
            assert!(
                (app.settings.delay_mix - want).abs() < 1.0e-6,
                "three sweeps landed on {}, not {want}",
                app.settings.delay_mix
            );
        }
    }

    /// A desktop app with the band up and nothing modal in front of it.
    ///
    /// The Welcome card is a dialog, and a dialog swallows every press in the
    /// main window — so a pointer test on a plain `headless()` passes the press
    /// to nothing at all and asserts about a gesture that never happened.
    fn headless_with_band(caps: Caps) -> (egui::Context, IvoryApp) {
        let mut s = Settings::default();
        s.show_welcome = false;
        s.show_recorder = true;
        let (ctx, mut app) = headless_with(caps, s);
        app.dialog = None;
        // **One frame before anybody points at anything.** egui learns a
        // widget's rectangle by drawing it, so interaction on the very first
        // frame is interaction with a surface it has never heard of.
        pointer_frame(&ctx, &mut app, Vec::new());
        app.dialog = None;
        (ctx, app)
    }

    /// Where a knob is in the band this app last drew.
    ///
    /// `last_band` and not a rectangle assumed to be at the top of the window:
    /// where the band lands depends on which other bands are showing, and a
    /// test that guessed was pressing empty canvas three hundred points above
    /// the control it meant to grab.
    fn knob_cell(app: &IvoryApp, hit: recorder_panel::Hit) -> Rect {
        let band = app.last_band;
        assert!(band.is_positive(), "the band was never drawn");
        let v = app.recorder_layout_view();
        recorder_panel::knob_rect(band, &v, hit).expect("the knob has a rectangle")
    }

    fn pointer_frame(ctx: &egui::Context, app: &mut IvoryApp, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0))),
                events,
                ..Default::default()
            },
            |ctx| app.frame(ctx),
        );
    }

    /// Press and HOLD at `pos`. The button is never released, so the frames
    /// after this one still see it down — which is what a drag is.
    fn press(ctx: &egui::Context, app: &mut IvoryApp, pos: Pos2) {
        pointer_frame(
            ctx,
            app,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }

    fn move_to(ctx: &egui::Context, app: &mut IvoryApp, pos: Pos2) {
        pointer_frame(ctx, app, vec![egui::Event::PointerMoved(pos)]);
    }

    // ── the effect panels ───────────────────────────────────────────────────

    /// An app with the band up and the effect defaults the host would push.
    /// One filter's slope, as the host offers it. See `desktop::slope_choice`
    /// — this is a hand copy of it, which is exactly why the binary has a test
    /// asserting the host supplies a choice for every stepped row.
    fn slope_choice(key: &str) -> crate::ports::ChoiceParam {
        crate::ports::ChoiceParam {
            key: key.to_owned(),
            options: [("6", "6 dB/oct"), ("12", "12 dB/oct"), ("24", "24 dB/oct")]
                .into_iter()
                .map(|(k, l)| (k.to_owned(), l.to_owned()))
                .collect(),
            default: "24".to_owned(),
        }
    }

    fn headless_with_fx(caps: Caps) -> (egui::Context, IvoryApp) {
        let (ctx, mut app) = headless_with_band(caps);
        app.set_effect_defaults(crate::ports::EffectDefaults {
            units: vec![
                (
                    "hpf_mix".to_owned(),
                    crate::ports::KnobUnit::Hertz {
                        low: 20.0,
                        high: 1_200.0,
                    },
                ),
                (
                    "lpf_mix".to_owned(),
                    crate::ports::KnobUnit::Hertz {
                        low: 20_000.0,
                        high: 200.0,
                    },
                ),
            ],
            values: [
                ("reverb_size", 0.62),
                ("reverb_damp", 0.35),
                ("reverb_width", 0.70),
                ("delay_feedback", 0.42),
                ("delay_tone", 0.55),
                ("delay_width", 0.60),
                ("chorus_rate", 0.28),
                ("chorus_depth", 0.55),
                ("chorus_width", 0.85),
                ("chorus_tone", 0.45),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_owned(), serde_json::Value::from(v)))
            .collect(),
            choices: vec![
                crate::ports::ChoiceParam {
                    key: "delay_division".to_owned(),
                    options: [
                        ("quarter", "1/4"),
                        ("dotted-eighth", "1/8 dotted"),
                        ("eighth", "1/8"),
                    ]
                    .into_iter()
                    .map(|(k, l)| (k.to_owned(), l.to_owned()))
                    .collect(),
                    default: "dotted-eighth".to_owned(),
                },
                slope_choice("hpf_slope"),
                slope_choice("lpf_slope"),
            ],
        });
        (ctx, app)
    }

    /// **The two trim handles may not cross.**
    ///
    /// An out-point before the in-point is a track that plays nothing, and the
    /// way somebody finds out is by pressing Record and hearing silence.
    #[test]
    fn the_trim_handles_stay_in_order() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        app.set_track_for_shot(
            crate::ports::TrackInfo {
                name: "b.mp3".to_owned(),
                seconds: 200.0,
                wave: vec![0.5; 100],
                error: String::new(),
            },
            false,
        );

        app.set_trim(true, 50.0);
        app.set_trim(false, 150.0);
        assert!((app.settings.track_in - 50.0).abs() < 1.0e-6);
        assert!((app.settings.track_out - 150.0).abs() < 1.0e-6);

        // Dragging the in-point past the out-point stops at it.
        app.set_trim(true, 180.0);
        assert!(
            app.settings.track_in < app.settings.track_out,
            "in {} is not before out {}",
            app.settings.track_in,
            app.settings.track_out
        );
        // And the other way.
        app.set_trim(false, 1.0);
        assert!(app.settings.track_out > app.settings.track_in);

        // Dragged back to the very end, the out-point becomes "no out-point",
        // which is the zero the engine and the settings both read as the end.
        app.set_trim(true, 0.0);
        app.set_trim(false, 200.0);
        assert_eq!(app.settings.track_out, 0.0);

        // Past either end it pins inside the file.
        app.set_trim(true, 9_000.0);
        assert!(app.settings.track_in <= 200.0);
        app.set_trim(true, -5.0);
        assert_eq!(app.settings.track_in, 0.0);
    }

    /// A trim typed as a time lands where a player's display would say.
    #[test]
    fn a_typed_trim_reads_both_ways_of_writing_a_time() {
        assert_eq!(recorder::parse_time("12.5"), Some(12.5));
        assert_eq!(recorder::parse_time("1:12.5"), Some(72.5));
        assert_eq!(recorder::parse_time(" 2:00 "), Some(120.0));
        assert_eq!(recorder::parse_time("0"), Some(0.0));
        // And nonsense is refused rather than read as the top of the file.
        assert_eq!(recorder::parse_time(""), None);
        assert_eq!(recorder::parse_time("soon"), None);
        assert_eq!(recorder::parse_time("-4"), None);
        assert_eq!(recorder::parse_time("1:-4"), None);
        // What the panel PRINTS is what the field ACCEPTS, which is the whole
        // reason the minutes form is parsed at all.
        for t in [0.0_f64, 9.5, 72.5, 196.0] {
            let shown = recorder_panel::trim_text(t);
            let back = recorder::parse_time(&shown)
                .unwrap_or_else(|| panic!("{shown} is printed and not accepted"));
            assert!((back - t).abs() < 0.06, "{shown} came back as {back}");
        }
    }

    /// **A double click puts a knob back, through the app, into settings.**
    ///
    /// The unit test beside `reset_to` proves the VALUES; this proves the
    /// wiring — that the hit a double click produces reaches the settings the
    /// host reads, for every one of the eight.
    #[test]
    fn resetting_a_knob_lands_in_the_settings() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        // Put every knob somewhere it does not belong.
        for fx in recorder_panel::Fx::ALL {
            app.apply_recorder_hit(recorder_panel::Hit::SetFx(fx, 0.63));
            assert!((app.fx_value(fx) - 0.63).abs() < 1.0e-4);
        }
        app.apply_recorder_hit(recorder_panel::Hit::SetTempo(184.0));
        app.apply_recorder_hit(recorder_panel::Hit::SetMaster(0.2));

        // Now reset each one the way a double click would.
        for hit in recorder_panel::Hit::ALL.into_iter().filter(|h| h.is_knob()) {
            let reset = hit.reset_to().expect("a knob with no resting value");
            app.apply_recorder_hit(reset);
        }
        for fx in recorder_panel::Fx::ALL {
            let rest = if fx == recorder_panel::Fx::Limiter { 1.0 } else { 0.0 };
            assert_eq!(app.fx_value(fx), rest, "{} did not go back", fx.title());
        }
        assert!((app.settings.record_export.tempo_bpm - 120.0).abs() < 1.0e-9);
        assert!(
            (app.settings.master_gain - 1.0).abs() < 1.0e-4,
            "the master came back at {}",
            app.settings.master_gain
        );
    }

    /// **A right-click on a knob opens it for typing** — all eight — and
    /// shift keeps the effect's parameters where they were.
    #[test]
    fn right_clicking_a_knob_opens_it_for_typing() {
        let (_, app) = headless_with_fx(Caps::DESKTOP);
        let band = app.last_band;
        for hit in recorder_panel::Hit::ALL.into_iter().filter(|h| h.is_knob()) {
            let at = knob_cell(&app, hit).center();
            let found = app
                .knob_under(Some(band), at)
                .unwrap_or_else(|| panic!("{hit:?} is not under its own cell"));
            assert!(
                found.is_same_control(hit),
                "{at:?} is over {found:?}, not {hit:?}"
            );
            assert!(recorder_panel::num_field(found).is_some());
        }
        // And nothing is a knob between them.
        assert_eq!(app.knob_under(Some(band), band.left_bottom()), None);
        assert_eq!(app.knob_under(None, band.center()), None);
    }

    /// **The master knob is a fader wearing a knob**, and it lands in the
    /// settings as a linear gain like every other level in this band.
    #[test]
    fn the_master_knob_sets_the_master_gain() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        assert!(
            (app.settings.master_gain - 1.0).abs() < 1.0e-9,
            "the master does not ship at unity"
        );
        // Unity is where a knob at unity reads, so the round trip has to hold.
        assert!(
            (app.control_value(recorder_panel::Hit::SetMaster(0.0))
                - recorder::gain_to_fader(1.0))
            .abs()
                < 1.0e-6
        );
        app.apply_recorder_hit(recorder_panel::Hit::SetMaster(recorder::gain_to_fader(0.5)));
        assert!(
            (app.settings.master_gain - 0.5).abs() < 1.0e-3,
            "the master came out at {}",
            app.settings.master_gain
        );
        // And it reaches the band, which is what the host reads to push it at
        // the engine.
        assert!((app.settings.knobs().gains.master - 0.5).abs() < 1.0e-3);
        // All the way down is silence, not a floor.
        app.apply_recorder_hit(recorder_panel::Hit::SetMaster(0.0));
        assert_eq!(app.settings.master_gain, 0.0);
    }

    /// **Right-clicking a knob opens the effect behind it**, and the right one.
    ///
    /// All six, and in a grid rather than a row: the knobs are two rows of
    /// three now, so a panel that opened the wrong effect could be wrong in
    /// two directions. It would look like the panel simply not working.
    #[test]
    fn right_clicking_a_knob_opens_that_effect() {
        let (_, app) = headless_with_fx(Caps::DESKTOP);
        for fx in recorder_panel::Fx::ALL {
            let at = knob_cell(&app, recorder_panel::Hit::SetFx(fx, 0.0)).center();
            assert_eq!(
                app.fx_under(Some(app.last_band), at),
                Some(fx),
                "{} is not under its own knob",
                fx.title()
            );
        }
        // And nothing opens from the panel between them.
        let band = app.last_band;
        assert_eq!(app.fx_under(Some(band), band.center()), None);
        assert_eq!(app.fx_under(None, band.center()), None);
    }

    /// Every row sets its own parameter, and the value follows the position.
    #[test]
    fn each_row_of_a_panel_sets_its_own_parameter() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        let fx = recorder_panel::Fx::Chorus;
        app.fx_open = Some(fx);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0));
        let anchor = app.fx_anchor(fx);
        assert!(anchor.is_positive(), "the panel has nothing to hang off");

        for recorder_panel::FxRow { key, step, .. } in fx.rows() {
            assert!(!step, "this test only knows how to drag a sliding row");
            // The row is found by ASKING the panel, not by guessing: the
            // layout owns where the rows are and this test must not restate
            // it, or it would pass while the panel drew them somewhere else.
            let at = row_probe(screen, anchor, fx, key, 1.0);
            app.press_in_fx_panel(screen, fx, at, true);
            app.fx_drag = None;
            assert!(
                (app.effect_param(key) - 1.0).abs() < 0.02,
                "{key} came out at {} after a press at the top of its row",
                app.effect_param(key)
            );

            let at = row_probe(screen, anchor, fx, key, 0.0);
            app.press_in_fx_panel(screen, fx, at, true);
            app.fx_drag = None;
            assert!(
                app.effect_param(key) < 0.02,
                "{key} came out at {} at the bottom",
                app.effect_param(key)
            );
        }
    }

    /// **Every panel, every row, hits the parameter it is labelled with.**
    ///
    /// The one above drags one panel in detail; this one walks all six and
    /// asks a cheaper question of each row — does pressing it reach THIS key
    /// and no other. A filter's Slope row and the delay's Time row step rather
    /// than slide, so they are checked by stepping.
    #[test]
    fn every_panel_row_reaches_its_own_parameter() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0));
        for fx in recorder_panel::Fx::ALL {
            app.fx_open = Some(fx);
            let anchor = app.fx_anchor(fx);
            assert!(
                anchor.is_positive(),
                "{} has no knob to hang off",
                fx.title()
            );
            for recorder_panel::FxRow { key, step, .. } in fx.rows() {
                if key.is_empty() {
                    continue;
                }
                if step {
                    // A stepped row moves to a different named value, and the
                    // host is what decides which values exist.
                    let before = app.choice_key(key);
                    let at = row_probe(screen, anchor, fx, key, 0.5);
                    app.press_in_fx_panel(screen, fx, at, true);
                    app.fx_drag = None;
                    assert_ne!(
                        app.choice_key(key),
                        before,
                        "{key} did not step when its row was pressed"
                    );
                    continue;
                }
                let at = row_probe(screen, anchor, fx, key, 1.0);
                app.press_in_fx_panel(screen, fx, at, true);
                app.fx_drag = None;
                assert!(
                    (app.effect_param(key) - 1.0).abs() < 0.02,
                    "{} row {key} came out at {}",
                    fx.title(),
                    app.effect_param(key)
                );
            }
        }
    }

    /// **A drag stays on the row it started on.** The rows are twenty points
    /// apart, and a hand that drifts up while dragging must not start setting
    /// the parameter above — the same rule the band's faders follow.
    #[test]
    fn a_drag_inside_a_panel_does_not_wander_onto_the_row_above() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        let fx = recorder_panel::Fx::Chorus;
        app.fx_open = Some(fx);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0));
        let anchor = app.fx_anchor(fx);

        // Grab the DEPTH row (the second) at its middle.
        let start = row_probe(screen, anchor, fx, "chorus_depth", 0.5);
        app.press_in_fx_panel(screen, fx, start, true);
        assert_eq!(app.fx_drag, Some("chorus_depth"));
        let rate_before = app.effect_param("chorus_rate");

        // Now slide up onto the RATE row and along it.
        let wandered = row_probe(screen, anchor, fx, "chorus_rate", 0.95);
        app.press_in_fx_panel(screen, fx, wandered, false);
        assert!(
            (app.effect_param("chorus_rate") - rate_before).abs() < 1.0e-6,
            "the drag wandered onto Rate and set it"
        );
        assert!(
            app.effect_param("chorus_depth") > 0.8,
            "the drag stopped following the row it started on"
        );
    }

    /// Reset puts an effect back and leaves the others alone.
    #[test]
    fn reset_restores_one_effect_and_only_that_one() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        app.set_effect_param("chorus_depth", 0.05);
        app.set_effect_param("reverb_size", 0.05);
        app.reset_effect(recorder_panel::Fx::Chorus);
        assert!(
            (app.effect_param("chorus_depth") - 0.55).abs() < 1.0e-6,
            "chorus depth did not go back to what it ships as"
        );
        assert!(
            (app.effect_param("reverb_size") - 0.05).abs() < 1.0e-6,
            "resetting the chorus reached into the reverb"
        );
        // By REMOVING the key, so an old file and a reset file read alike.
        assert!(!app.settings.effect_params.contains_key("chorus_depth"));
    }

    /// A named parameter steps through its list and wraps.
    ///
    /// Both of them, because "the delay's time" and "a filter's slope" are the
    /// same mechanism now and the second one is the reason it is a mechanism
    /// rather than a special case.
    #[test]
    fn a_named_parameter_steps_through_its_choices() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        assert_eq!(app.choice_label("delay_division"), "1/8 dotted");
        app.next_choice("delay_division");
        assert_eq!(app.choice_label("delay_division"), "1/8");
        app.next_choice("delay_division");
        assert_eq!(app.choice_label("delay_division"), "1/4", "it did not wrap");

        // The filter slope, which ships at the steepest and wraps to the
        // gentlest.
        assert_eq!(app.choice_label("hpf_slope"), "24 dB/oct");
        app.next_choice("hpf_slope");
        assert_eq!(app.choice_label("hpf_slope"), "6 dB/oct", "it did not wrap");
        app.next_choice("hpf_slope");
        assert_eq!(app.choice_label("hpf_slope"), "12 dB/oct");
        // Stepping one did not move the other.
        assert_eq!(app.choice_label("delay_division"), "1/4");

        // A value a later build wrote, which this one does not know, reads as
        // the default rather than as an empty box.
        app.settings.effect_params.insert(
            "delay_division".to_owned(),
            serde_json::Value::from("some-future-division"),
        );
        assert_eq!(app.choice_label("delay_division"), "1/8 dotted");

        // A key with no choice at all answers with nothing rather than
        // panicking: the host decides what is a choice, and an older host is
        // allowed not to offer this one.
        assert_eq!(app.choice_label("no_such_param"), "");
        app.next_choice("no_such_param");
    }

    /// A press outside the panel closes it and does nothing else.
    #[test]
    fn a_press_outside_the_panel_closes_it() {
        let (_, mut app) = headless_with_fx(Caps::DESKTOP);
        let fx = recorder_panel::Fx::Reverb;
        app.fx_open = Some(fx);
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0));
        let anchor = app.fx_anchor(fx);
        let before = app.effect_param("reverb_size");

        let panel = recorder_panel::fx_popup_rect(screen, anchor);
        let outside = Pos2::new(panel.right() + 40.0, panel.bottom() + 40.0);
        app.press_in_fx_panel(screen, fx, outside, true);
        assert!(app.fx_open.is_none(), "the panel stayed open");
        assert!((app.effect_param("reverb_size") - before).abs() < 1.0e-6);
    }

    /// A point `t` of the way along `key`'s track, found by asking the panel.
    fn row_probe(
        screen: Rect,
        anchor: Rect,
        fx: recorder_panel::Fx,
        key: &str,
        t: f32,
    ) -> Pos2 {
        // Walk the panel until the row reports itself, then take the point at
        // `t` along it. Asking rather than restating the layout is the whole
        // point: a test that recomputed the rows would pass while the panel
        // drew them somewhere else.
        let panel = recorder_panel::fx_popup_rect(screen, anchor);
        let steps = 400;
        for i in 0..=steps {
            let y = panel.top() + panel.height() * i as f32 / steps as f32;
            let probe = Pos2::new(panel.center().x, y);
            if recorder_panel::fx_row_at(screen, anchor, fx, probe) == Some(key) {
                // Now sweep across to find the ends of the track.
                let mut lo = f32::MAX;
                let mut hi = f32::MIN;
                for j in 0..=steps {
                    let x = panel.left() + panel.width() * j as f32 / steps as f32;
                    let at = Pos2::new(x, y);
                    if let Some(v) = recorder_panel::fx_value_at(screen, anchor, fx, key, at) {
                        if recorder_panel::fx_row_at(screen, anchor, fx, at) == Some(key) {
                            if v <= 0.001 {
                                lo = lo.min(x);
                            }
                            if v >= 0.999 {
                                hi = hi.max(x);
                            }
                        }
                    }
                }
                assert!(lo < hi, "{key} has no track to drag along");
                return Pos2::new(lo + (hi - lo) * t, y);
            }
        }
        panic!("{key} has no row in the {} panel", fx.title())
    }

    /// One frame with no input at all, and whatever it asked for.
    fn run_frame(ctx: &egui::Context, app: &mut IvoryApp) -> Option<recorder::RecorderRequest> {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1300.0, 900.0))),
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
                theory_order: if theory {
                    theory_panel::Views::all().keys()
                } else {
                    String::new()
                },
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
                         (fretboard {fret}, theory {theory}) - it did not \
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
            // Explicitly, since 5.0: the strip is off by default and this test
            // is about a DETACHED band having nowhere to go in a plugin, which
            // needs a strip to be attached in the first place.
            show_chord_strip: true,
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
            "clicking a menu row in a plugin did nothing - the row is dead"
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
            s.theory_order = if theory { crate::theory_panel::Views::all().keys() } else { String::new() };
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
        s.theory_order = if true { crate::theory_panel::Views::all().keys() } else { String::new() };
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
        s.theory_order = if true { crate::theory_panel::Views::all().keys() } else { String::new() };
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
            "off by default - the band is 200pt tall and a window that grows \
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

    /// The transport keys are Enter and Space, and the band owns a text field.
    /// Typing into a take name must not also drive the transport — and Enter
    /// is now one of them, which makes this sharper than it was: Enter is also
    /// how you finish typing a name.
    #[test]
    fn a_focused_take_name_swallows_the_transport_key() {
        let (ctx, mut app) = recorder_app();
        app.settings.show_recorder = true;
        // **Space means whichever of its two jobs is live.** Rolling, it
        // stops. Idle, "stop" means nothing, so it sounds what is lit.
        app.recorder.state = recorder::RecordState::Rolling;
        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Stop),
            "with a take rolling, Space stops it"
        );
        app.recorder.state = recorder::RecordState::Idle;
        // Keytoggle on, because that is what "toggle notes on the neck" means
        // and it is the only way a placed note is one of the notes on screen.
        app.settings.keytoggle_enabled = true;
        app.manual_notes.insert(60);
        // Idle, Space strikes what is lit. The latch is silent by itself —
        // see `a_latched_note_is_lit_but_does_not_ring` — so this request is
        // Space's own.
        assert!(
            matches!(
                space(&ctx, &mut app),
                Some(recorder::RecorderRequest::Audition { on: true, .. })
            ),
            "with nothing rolling, Space sounds what is lit"
        );
        // And it is a HOLD: letting go releases it. Asserted rather than
        // merely drained, because a chord that never lets go is the failure
        // this whole model exists to prevent.
        assert!(
            matches!(
                key_up(&ctx, &mut app),
                Some(recorder::RecorderRequest::Audition { on: false, .. })
            ),
            "letting go of the key did not release the chord"
        );
        app.manual_notes.clear();

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
        app.recorder.state = recorder::RecordState::Rolling;
        assert_eq!(
            space(&ctx, &mut app),
            Some(recorder::RecorderRequest::Stop)
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

    /// **Fullscreen uses both edges, or it is a nuisance.**
    ///
    /// `fit_bands` preserves each band's natural aspect, so one dimension runs
    /// out first and the other gets bars. That is right in a window somebody
    /// sized and useless when they asked for the whole screen — bars down both
    /// sides are the real estate they asked for and did not get.
    #[test]
    fn filling_the_screen_uses_all_of_it() {
        let s = crate::settings::Settings::first_launch();
        for screen in [
            Vec2::new(1920.0, 1080.0),
            Vec2::new(3440.0, 1440.0),
            Vec2::new(2560.0, 1600.0),
            // Portrait, and a near-square, because a monitor on its side is a
            // real thing and the arithmetic must not care.
            Vec2::new(1080.0, 1920.0),
            Vec2::new(1400.0, 1400.0),
        ] {
            let filled = fill_bands(&s, screen);
            let total = filled.total();
            assert!(
                (total.x - screen.x).abs() < 1.5,
                "{screen:?}: filled {total:?}, leaving bars down the sides"
            );
            assert!(
                (total.y - screen.y).abs() < 1.5,
                "{screen:?}: filled {total:?}, leaving bars top and bottom"
            );
            // Every band the settings asked for is still there. Filling must
            // not be a way to lose one. The chord strip is legitimately absent
            // here: the default theory band includes the staff, and the staff
            // carries the chord name itself — see `chord_visible`.
            assert!(filled.piano_h > 0.0, "{screen:?}: no piano");
            assert_eq!(
                filled.chord_h, 0.0,
                "{screen:?}: the strip is up while the staff already shows the chord"
            );
            assert!(filled.fret_h > 0.0, "{screen:?}: no fretboard");
            assert!(filled.theory_h > 0.0, "{screen:?}: no theory band");
        }
    }

    /// The bands keep their proportions relative to EACH OTHER.
    ///
    /// One scale factor for all of them, so filling changes each band's own
    /// aspect a little and never the balance between them — a keyboard that
    /// grew while the fretboard beside it did not would read as a bug.
    #[test]
    fn filling_scales_every_band_by_the_same_amount() {
        let s = crate::settings::Settings::first_launch();
        let screen = Vec2::new(2560.0, 1440.0);
        let natural = band_sizes_at(&s, screen.x);
        let filled = fill_bands(&s, screen);
        let k = filled.total().y / natural.total().y;
        for (name, a, b) in [
            ("piano", natural.piano_h, filled.piano_h),
            ("chord", natural.chord_h, filled.chord_h),
            ("fretboard", natural.fret_h, filled.fret_h),
            ("theory", natural.theory_h, filled.theory_h),
            ("recorder", natural.recorder_h, filled.recorder_h),
        ] {
            if a <= 0.0 {
                continue;
            }
            assert!(
                (b / a - k).abs() < 1e-3,
                "{name} scaled by {} while the stack scaled by {k}",
                b / a
            );
        }
    }

    /// A reset must land where a fresh install lands.
    ///
    /// They were two different states, and the gap was a trap: a new install
    /// shows every band and "Reset Settings to Default" showed two. Somebody
    /// pressing it to get back to how the app came got LESS, which reads as
    /// settings that are not being saved.
    #[test]
    fn resetting_lands_where_a_fresh_install_lands() {
        let mut s = crate::settings::Settings::default();
        s.show_fretboard = false;
        s.theory_order = if false { crate::theory_panel::Views::all().keys() } else { String::new() };
        s.dark_mode = true;
        s.reset_to_defaults();

        let fresh = crate::settings::Settings::first_launch();
        assert_eq!(s.show_fretboard, fresh.show_fretboard);
        assert_eq!(s.theory_views().count(), fresh.theory_views().count());
        assert_eq!(s.show_recorder, fresh.show_recorder);
        assert!(s.show_fretboard, "a reset hid the guitar view");
        assert_eq!(
            s.theory_views(),
            theory_panel::Views::all(),
            "a reset did not restore every theory element"
        );
        // **And an instrument in the rack.** It sounds either way, because the
        // renderer plays the built-in when nothing else has — but a reset that
        // left five empty rows told somebody the app had no instrument while
        // it was playing one, and the patch picker is reached by clicking the
        // slot it is in.
        assert_eq!(
            s.plugin_slots[0].as_deref(),
            Some(dialogs::BUILTIN_PATH),
            "a reset left the rack empty"
        );
    }

    /// A fresh install lands in the same place, which is the whole point of
    /// `first_launch` being what a reset resets to.
    #[test]
    fn a_fresh_install_has_the_built_in_loaded() {
        let s = crate::settings::Settings::first_launch();
        assert_eq!(s.plugin_slots[0].as_deref(), Some(dialogs::BUILTIN_PATH));
        // And nothing else, so the rack is not four rows of something.
        assert!(s.plugin_slots[1..].iter().all(Option::is_none));
        // No cartridge chosen means the one that ships. See `dx7::factory`.
        assert!(s.dx7_cartridge.is_empty());
    }

    /// **One tempo, one source.**
    ///
    /// `export_spec` honours a session-only override and `tempo_bpm` read the
    /// settings, so a tempo set for ONE take in the Export dialog moved the
    /// `.mid` and the on-screen count and left the CLICK playing the old one.
    /// A click at 90 against a file that says 120 is the exact failure the "one
    /// tempo" rule exists to prevent, and it survived for the whole session
    /// because nothing clears the override at a take boundary.
    #[test]
    fn a_one_off_export_tempo_moves_the_click_too() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        app.settings.record_export.tempo_bpm = 120.0;
        assert!((app.tempo_bpm() - 120.0).abs() < 1e-9);

        // Session-only, exactly as the dialog's untick leaves it.
        app.apply_dialog_action(DialogAction::SetExport(recorder::ExportSpec {
            tempo_bpm: 90.0,
            ..app.settings.record_export
        }));
        assert!(
            (app.tempo_bpm() - 90.0).abs() < 1e-9,
            "the click is still at {} while the take is at 90",
            app.tempo_bpm()
        );
        assert!((app.export_spec().tempo_bpm - 90.0).abs() < 1e-9);
        // The stored setting is untouched, which is what "session only" means.
        assert!((app.settings.record_export.tempo_bpm - 120.0).abs() < 1e-9);
    }

    /// **A typed level lands where the fader would put it.**
    ///
    /// The setters take a FADER POSITION and the field accepts dB, so a commit
    /// has to go back through the same curve the drag uses. Getting this wrong
    /// is silent: "-6" would be accepted, stored as a position of -6, clamped
    /// to zero, and the instrument would go quiet.
    #[test]
    fn a_typed_level_is_the_level_that_was_typed() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        for (field, read) in [
            (
                recorder::NumField::Slot(1),
                (|a: &IvoryApp| a.settings.plugin_gains[1]) as fn(&IvoryApp) -> f64,
            ),
            (recorder::NumField::Metronome, |a| a.settings.metronome_gain),
            (recorder::NumField::Input, |a| a.settings.input_gain),
        ] {
            app.num_edit = Some(recorder::NumEdit {
                field,
                text: "-6".to_owned(),
            });
            app.commit_number();
            assert!(app.num_edit.is_none(), "{field:?} stayed open");
            let db = 20.0 * (read(&app) as f32).log10();
            assert!(
                (db + 6.0).abs() < 0.2,
                "{field:?} was set to {db:+.2} dB, not -6"
            );
        }
    }

    #[test]
    fn a_typed_tempo_is_the_tempo_that_was_typed() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        app.num_edit = Some(recorder::NumEdit {
            field: recorder::NumField::Tempo,
            text: "132".to_owned(),
        });
        app.commit_number();
        assert!((app.settings.record_export.tempo_bpm - 132.0).abs() < 1e-9);
    }

    /// Committing junk closes the field and changes NOTHING.
    ///
    /// Refusing to close would trap somebody in a field they cannot satisfy,
    /// and there is nothing to warn about — the value they were looking at is
    /// still the value they have.
    #[test]
    fn committing_nonsense_leaves_the_value_alone() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        let before = app.settings.record_export.tempo_bpm;
        for junk in ["", "-", "."] {
            app.num_edit = Some(recorder::NumEdit {
                field: recorder::NumField::Tempo,
                text: junk.to_owned(),
            });
            app.commit_number();
            assert!(app.num_edit.is_none(), "{junk:?} left the field open");
            assert_eq!(
                app.settings.record_export.tempo_bpm, before,
                "{junk:?} moved the tempo"
            );
        }
    }

    /// A press elsewhere commits, a press back into the SAME field does not.
    ///
    /// The exception is what lets somebody click into a field they are already
    /// editing to fix a typo, without the click committing it out from under
    /// them.
    #[test]
    fn clicking_away_commits_and_clicking_back_does_not() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        let typed = || recorder::NumEdit {
            field: recorder::NumField::Tempo,
            text: "144".to_owned(),
        };

        app.num_edit = Some(typed());
        // `EditTempo`, because that is the box now: `SetTempo` carries a
        // committed value and is not a thing on screen to press.
        app.commit_number_unless(Some(recorder_panel::Hit::SetTempo(90.0)));
        assert!(app.num_edit.is_some(), "clicking the same field committed it");
        assert!(
            (app.settings.record_export.tempo_bpm - 144.0).abs() > 1e-9,
            "and it must not have applied either"
        );

        app.commit_number_unless(Some(recorder_panel::Hit::Record));
        assert!(app.num_edit.is_none(), "clicking away left it open");
        assert!((app.settings.record_export.tempo_bpm - 144.0).abs() < 1e-9);
    }

    /// **Choosing "None" as the audio input has to survive a restart.**
    ///
    /// `record_audio_device: null` is also what "never opened the picker" looks
    /// like, and the two want opposite behaviour at startup — no input at all,
    /// or the system default so the meter is live. `record_input_off` is what
    /// tells them apart, and if it does not round-trip then every launch
    /// helpfully opens the system microphone for somebody who said not to.
    #[test]
    fn choosing_no_audio_input_survives_a_restart() {
        let (_ctx, mut app) = headless(Caps::DESKTOP);
        app.apply_dialog_action(DialogAction::ChooseDevice {
            kind: dialogs::DeviceKind::AudioInput,
            uid: None,
        });
        assert!(
            app.audio_explicitly_off(),
            "picking None did not record that it was picked"
        );
        assert_eq!(app.chosen_audio_uid(), None);

        // And through the FILE, which is the half that actually restarts.
        // `Settings::path()` is redirected per-thread under `cfg(test)`, so
        // this writes and reads a real file without touching the user's.
        app.settings.save();
        let reloaded = crate::settings::Settings::load();
        assert!(
            reloaded.record_input_off,
            "the None choice did not survive being written and read back"
        );
        assert_eq!(reloaded.record_audio_device, None);

        // Picking a real device clears it again, or the flag would outlive the
        // choice it describes.
        app.apply_dialog_action(DialogAction::ChooseDevice {
            kind: dialogs::DeviceKind::AudioInput,
            uid: Some("Scarlett#0".to_owned()),
        });
        assert!(!app.audio_explicitly_off());
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

    /// Transposing moves the notes and the chord name with them.
    #[test]
    fn transposing_moves_every_held_note_by_a_semitone() {
        let c_major: HashSet<u8> = [60, 64, 67].into_iter().collect();
        assert_eq!(
            transposed(&c_major, 1),
            [61, 65, 68].into_iter().collect::<HashSet<u8>>()
        );
        assert_eq!(
            transposed(&c_major, -2),
            [58, 62, 65].into_iter().collect::<HashSet<u8>>()
        );
        assert_eq!(transposed(&c_major, 0), c_major, "zero is the identity");
    }

    /// **All or nothing.** A chord whose top note would leave MIDI's range is
    /// not transposed at all — transposing it with that note dropped silently
    /// changes the chord, and asking what the chord becomes is the whole point
    /// of the control.
    #[test]
    fn a_chord_that_cannot_all_fit_does_not_move_at_all() {
        let (_ctx, mut app) = recorder_app();
        app.manual_notes = [120, 124, 127].into_iter().collect();
        app.settings.keytoggle_enabled = true;
        app.transpose_by(1);
        assert_eq!(
            app.settings.transpose, 0,
            "127 cannot go up, so nothing did"
        );
        // ...and down is still fine.
        app.transpose_by(-1);
        assert_eq!(app.settings.transpose, -1);
        assert_eq!(
            app.display_notes(),
            [119, 123, 126].into_iter().collect::<HashSet<u8>>()
        );
    }

    /// The offset is bounded, or a held-down arrow key walks it somewhere a
    /// chord can never come back from.
    #[test]
    fn the_transpose_is_bounded_in_both_directions() {
        let (_ctx, mut app) = recorder_app();
        for _ in 0..100 {
            app.transpose_by(1);
        }
        assert_eq!(app.settings.transpose, crate::settings::TRANSPOSE_MAX);
        for _ in 0..200 {
            app.transpose_by(-1);
        }
        assert_eq!(app.settings.transpose, -crate::settings::TRANSPOSE_MAX);
    }

    /// The arrows are drawn top-LEFT and the heart top-RIGHT, so one can never
    /// be clicked while aiming at the other.
    #[test]
    fn the_transpose_arrows_never_overlap_the_heart() {
        for w in [400.0_f32, 900.0, 1300.0, 2600.0] {
            let r = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, (w / 26.0).max(20.0)));
            let (up, down) = chord_strip::transpose_rects(r);
            let heart = chord_strip::heart_rect(r);
            assert!(!up.intersects(heart) && !down.intersects(heart), "at {w}");
            assert!(!up.intersects(down), "the two arrows overlap at {w}");
            for a in [up, down, heart] {
                assert!(r.contains_rect(a), "{a:?} escapes the strip at {w}");
            }
        }
    }

    /// It survives a restart: a transpose is a mode you are in, and one that
    /// reset on relaunch would silently change what the chord name means
    /// between sessions.
    #[test]
    fn the_transpose_is_remembered() {
        let mut s = Settings::default();
        assert_eq!(s.transpose, 0, "and starts at nothing");
        assert!(s.show_transpose, "the arrows are on by default");
        s.transpose = -5;
        s.show_transpose = false;
        let back = Settings::from_json(&s.to_json());
        assert_eq!(back.transpose, -5);
        assert!(!back.show_transpose);
        // A hand-edited file cannot put the app somewhere the buttons cannot.
        let wild = Settings::from_json(r#"{"transpose": 9999}"#);
        assert_eq!(wild.transpose, crate::settings::TRANSPOSE_MAX);
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
        // The piano and the chord strip alone: this test is about the size
        // ARITHMETIC — the truncation Python did — and every other band is a
        // separate term added to the same total. Leaving the theory band on
        // would be testing that it exists, which its own tests do, while making
        // a failure here unreadable.
        let mut s = Settings::default();
        s.theory_order = String::new();
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
            s.show_chord_strip = true;
            assert_eq!(main_width(&s), w, "W at {pct}%");
            // Chord strip visible: height = chordH + pianoH. Asked for by hand
            // now — 5.0 leaves it off, since the staff carries the name.
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



