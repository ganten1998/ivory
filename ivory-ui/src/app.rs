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
use crate::ports::MidiPorts;
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
    /// What the host permits. Read at every branch point rather than compared
    /// against a host name, and captured once at construction so a frame
    /// cannot be half-drawn under one set of rules and half under another.
    caps: Caps,

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
    pub fn new(ctx: &egui::Context, settings: Settings, caps: Caps) -> Self {
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
        let weights = Weights::for_tuning(spec.tuning);
        let voicing = VoicingSession::new(spec, weights);
        let settings_fret_size = settings.fretboard_win_size();
        let settings_fret_pos = settings.fretboard_win_pos();
        let startup_fret_detach_at = (settings.fretboard_detached && settings.show_fretboard)
            .then(|| Instant::now() + DEBOUNCE_100MS);

        Self {
            settings,
            detector,
            license,
            midi_tx,
            midi_rx,
            ports: None,
            caps,
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
        self.voicing.set_weights(Weights::for_tuning(spec.tuning));
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
            theory_follows_midi: self.settings.theory_follow_midi,
            wood: self.settings.fretboard_wood().key(),
            fretboard_detached: self.settings.fretboard_detached,
            caps: self.caps,
            tuning: self.settings.fretboard_spec().tuning.name,
            capo: self.settings.fretboard_spec().capo,
            next_font: {
                use crate::fonts::FontChoice;
                let cur = FontChoice::from_key(&self.settings.font_choice);
                // Show the row only if some OTHER installed face can be reached.
                FontChoice::ALL
                    .iter()
                    .copied()
                    .find(|f| *f != cur && f.is_available())
                    .map(|f| f.label())
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
    ) {
        let resp = ui.interact(
            ui.max_rect(),
            egui::Id::new("ivory-main-bg"),
            egui::Sense::click_and_drag(),
        );
        if self.dialog.is_some() {
            return; // Qt dialogs are modal: main window ignores input.
        }
        let (primary_pressed, pointer, ctrl) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
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
            if primary_pressed {
                self.menu_state = None; // click in main window closes the menu
            }
            return;
        }

        if primary_pressed && !ctrl_as_context {
            if let Some(pos) = pointer {
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
            K::ToggleDarkMode => self.apply_menu_action(ctx, MenuAction::ToggleDarkMode),
            K::ToggleDetection => self.apply_menu_action(ctx, MenuAction::ToggleChordDetection),
            K::ToggleBorderless => self.apply_menu_action(ctx, MenuAction::ToggleBorderless),
            K::CycleFont => self.apply_menu_action(ctx, MenuAction::CycleFont),
            // These open dialogs. The shortcut gate already refuses to fire
            // while one is up, so A cannot stack a second About on the first.
            K::ShowAbout => self.apply_menu_action(ctx, MenuAction::ShowAbout),
            K::ShowSupporterKey => self.apply_menu_action(ctx, MenuAction::ShowSupporterKey),
            // "Clear what I placed", not "clear everything": notes arriving
            // from a MIDI keyboard are not ours to drop, and they would come
            // straight back on the next frame anyway.
            K::ClearNotes => {
                self.manual_notes.clear();
                self.manual_positions.clear();
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
                let cur = FontChoice::from_key(&self.settings.font_choice);
                if let Some(next) = FontChoice::ALL
                    .iter()
                    .copied()
                    .find(|f| *f != cur && f.is_available())
                {
                    self.settings.font_choice = next.key().to_owned();
                    self.save_settings();
                    crate::fonts::install(ctx, next, self.settings.custom_font_path.as_deref());
                    crate::fonts::apply_text_styles(ctx);
                }
            }
            MenuAction::ToggleKeytoggle => {
                self.settings.keytoggle_enabled = !self.settings.keytoggle_enabled;
                if !self.settings.keytoggle_enabled {
                    self.manual_notes.clear(); // disabling clears manual notes
                    self.manual_positions.clear();
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
            MenuAction::ToggleHeart => {
                self.settings.show_heart = !self.settings.show_heart;
                self.save_settings();
            }
            MenuAction::ShowSupporterKey => {
                self.dialog = Some(Dialog::SupporterKey {
                    input: String::new(),
                    message: None,
                    installed_as: self.license.display_name().map(str::to_owned),
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
                        });
                    }
                    Err(err) => {
                        self.dialog = Some(Dialog::SupporterKey {
                            input: key,
                            message: Some(err.message().to_owned()),
                            installed_as: self.license.display_name().map(str::to_owned),
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
        fretboard_panel::band_height(w)
    } else {
        0.0
    };
    let theory_h = theory_panel::band_height(w, settings.theory_views());
    Bands {
        w,
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
    theory_h: f32,
    chord_h: f32,
    piano_h: f32,
    fret_h: f32,
}

impl Bands {
    fn total(self) -> Vec2 {
        Vec2::new(
            self.w,
            self.theory_h + self.chord_h + self.piano_h + self.fret_h,
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
        if self.dialog.is_none() && self.menu_state.is_none() {
            if let Some(action) = keys::pressed(&ctx) {
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
        let band_at = |top: f32, h: f32| {
            Rect::from_min_size(Pos2::new(origin.x, origin.y + top), Vec2::new(w, h))
        };
        let piano_rect = band_at(theory_h + chord_h, piano_h);
        let mut chord_rect_for_hit: Option<Rect> = None;
        let fret_rect_for_hit: Option<Rect> =
            (fret_h > 0.0).then(|| band_at(theory_h + chord_h + piano_h, fret_h));
        let theory_rect_for_hit: Option<Rect> = (theory_h > 0.0).then(|| band_at(0.0, theory_h));
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
            let chord_rect = band_at(theory_h, chord_h);
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
        );

        // Held, not toggled: press to read, release and it slides away. Drawn
        // last so it is over everything, and asked for every frame so the
        // animation can run even when nothing else changed.
        let help = keys::help_progress(&ctx);
        if help > 0.0 {
            keys::draw_help(ui.painter(), ui.max_rect(), self.settings.dark_mode, help);
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
                self.voicing.current(),
                &spec,
                &self.settings,
                self.settings.fretboard_wood(),
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

        // Detached chord window, gated for the same reason as the neck above.
        if self.detach_window_visible && self.caps.detachable {
            let outcome = chord_strip::show_detached_window(
                &ctx,
                self.detached_builder_size,
                self.detached_builder_pos,
                self.settings.borderless_mode,
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
            parent: self
                .main_origin_known
                .then(|| Rect::from_min_size(self.main_inner_origin, target)),
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
        let ctx = egui::Context::default();
        let app = IvoryApp::new(&ctx, Settings::default(), caps);
        (ctx, app)
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
                fretboard_panel::band_height(w),
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
            fretboard_panel::band_height(main_width(&s))
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
