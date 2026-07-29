//! The eframe App: fixed-size window mechanics, timers, MIDI state, and the
//! wiring between piano, chord strip, menu, and dialogs.
//!
//! Timing model (D-UI-3): no busy loop. `request_repaint_after(50ms)` gives
//! the Qt GUI-timer cadence, MIDI events wake the context immediately, and
//! chord detection runs on its own 100ms gate (with immediate off-cadence
//! runs after keytoggle clicks and note-preference changes).

use crate::chord_strip;
use crate::dialogs::{self, Dialog, DialogAction};
use crate::menu::{self, ColorTarget, MenuAction, MenuState, MenuView};
use crate::midi;
use crate::piano;
use crate::settings::{Rgb, Settings};
use egui::{Pos2, Rect, Vec2, ViewportCommand};
use ivory_core::{ChordDetector, OverrideStore};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const GUI_TICK: Duration = Duration::from_millis(50);
const DETECT_TICK: Duration = Duration::from_millis(100);
const DEBOUNCE_100MS: Duration = Duration::from_millis(100);

/// Per-note data (spec §4.3.5): velocity is stored but never affects
/// rendering; kept for parity and for the future teach layer.
#[allow(dead_code)]
struct NoteData {
    velocity: u8,
    pressed_at: Instant,
}

pub struct IvoryApp {
    settings: Settings,
    detector: ChordDetector,

    midi_tx: mpsc::Sender<midi::MidiEvent>,
    midi_rx: mpsc::Receiver<midi::MidiEvent>,
    midi_conn: Option<midi::MidiConnection>,

    active_notes: HashMap<u8, NoteData>,
    notes_to_release: HashSet<u8>,
    sustain_down: bool,
    manual_notes: HashSet<u8>,

    current_chord: Option<String>,
    last_detection: Option<Instant>,

    /// The detached chord window is actually on screen.
    detach_window_visible: bool,
    /// Builder size frozen per detachment session so the per-frame builder
    /// diff never fights user resizes (explicit syncs use ViewportCommand).
    detached_builder_size: Vec2,
    detached_live_size: Option<Vec2>,
    width_sync_deadline: Option<Instant>,
    startup_detach_at: Option<Instant>,

    menu_state: Option<MenuState>,
    dialog: Option<Dialog>,

    last_sent_size: Option<Vec2>,
    decorations_sent: Option<bool>,
    main_inner_origin: Pos2,
    monitor_size: Option<Vec2>,
}

impl IvoryApp {
    pub fn new(cc: &eframe::CreationContext<'_>, settings: Settings, cli_port: Option<String>) -> Self {
        crate::fonts::install(&cc.egui_ctx, settings.custom_font_path.as_deref());
        crate::fonts::apply_text_styles(&cc.egui_ctx);

        let mut detector = ChordDetector::new();
        detector.set_note_preference(settings.prefer_flats);
        // Teach layer: load user overrides (~/.config/ivory/overrides.json).
        // A missing or corrupt file yields an empty store; the detector then
        // behaves exactly like the stock engine until something is taught.
        detector.set_overrides(Some(OverrideStore::load()));

        let (midi_tx, midi_rx) = mpsc::channel();
        // Startup connection (spec §10): explicit -p port, else auto-connect
        // priority chain. Any failure => run without MIDI, no dialog.
        let midi_conn = match cli_port {
            Some(name) => midi::connect_by_name(&name, midi_tx.clone(), cc.egui_ctx.clone()).ok(),
            None => midi::auto_connect(midi_tx.clone(), cc.egui_ctx.clone()),
        };

        // Recreate the detached chord window 100ms after startup (spec §5.7).
        let startup_detach_at = (settings.chord_window_detached
            && settings.chord_detection_enabled)
            .then(|| Instant::now() + DEBOUNCE_100MS);

        let detached_builder_size = Vec2::new(
            main_width(&settings),
            settings.detached_height_for_use(),
        );

        Self {
            settings,
            detector,
            midi_tx,
            midi_rx,
            midi_conn,
            active_notes: HashMap::new(),
            notes_to_release: HashSet::new(),
            sustain_down: false,
            manual_notes: HashSet::new(),
            current_chord: None,
            last_detection: None,
            detach_window_visible: false,
            detached_builder_size,
            detached_live_size: None,
            width_sync_deadline: None,
            startup_detach_at,
            menu_state: None,
            dialog: None,
            last_sent_size: None,
            decorations_sent: None,
            main_inner_origin: Pos2::ZERO,
            monitor_size: None,
        }
    }

    // ── Geometry (spec §3.2, integer truncation like Python) ───────────────

    fn layout_sizes(&self) -> (f32, f32, f32) {
        let w = main_width(&self.settings);
        let piano_h = (w as f64 / (1300.0 / 150.0)).trunc() as f32;
        let chord_visible =
            self.settings.chord_detection_enabled && !self.settings.chord_window_detached;
        let chord_h = if chord_visible {
            (50.0 * w as f64 / 1300.0).trunc() as f32
        } else {
            0.0
        };
        (w, piano_h, chord_h)
    }

    // ── MIDI state (spec §10 semantics) ────────────────────────────────────

    fn process_midi_events(&mut self) {
        while let Ok(ev) = self.midi_rx.try_recv() {
            match ev {
                midi::MidiEvent::NoteOn { note, velocity } => {
                    self.active_notes.insert(
                        note,
                        NoteData {
                            velocity,
                            pressed_at: Instant::now(),
                        },
                    );
                    self.notes_to_release.remove(&note);
                }
                midi::MidiEvent::NoteOff { note } => {
                    if self.sustain_down {
                        if self.active_notes.contains_key(&note) {
                            self.notes_to_release.insert(note);
                        }
                    } else {
                        self.active_notes.remove(&note);
                        self.notes_to_release.remove(&note);
                    }
                }
                midi::MidiEvent::Sustain { down } => {
                    let was = self.sustain_down;
                    self.sustain_down = down;
                    if was && !down {
                        for note in self.notes_to_release.drain() {
                            self.active_notes.remove(&note);
                        }
                    }
                }
            }
        }
    }

    /// Keys drawn as active: MIDI-held notes plus manual (keytoggle) notes.
    fn display_notes(&self) -> HashSet<u8> {
        let mut set: HashSet<u8> = self.active_notes.keys().copied().collect();
        if self.settings.keytoggle_enabled {
            set.extend(self.manual_notes.iter().copied());
        }
        set
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

    // ── Main-window interaction ────────────────────────────────────────────

    fn handle_main_interaction(&mut self, ctx: &egui::Context, ui: &mut egui::Ui, piano_rect: Rect) {
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
                // Keytoggle hit-test/toggle first (spec §4.5), then StartDrag
                // (must be issued directly from the press handler).
                if self.settings.keytoggle_enabled && piano_rect.contains(pos) {
                    let local = pos - piano_rect.min;
                    if let Some(note) = piano::hit_test(
                        local.x,
                        local.y,
                        piano_rect.width(),
                        piano_rect.height(),
                    ) {
                        if !self.manual_notes.remove(&note) {
                            self.manual_notes.insert(note);
                        }
                        self.detection_tick(true); // immediate off-cadence update
                    }
                }
                if self.settings.borderless_mode {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
            }
        }
    }

    // ── Detach / attach (spec §5.7) ────────────────────────────────────────

    fn detach_chord_window(&mut self) {
        let (w, _, chord_h) = self.layout_sizes();
        // Python saves the current attached label height first.
        if chord_h > 0.0 {
            self.settings.detached_chord_height = chord_h as i64;
        }
        self.settings.chord_window_detached = true;
        self.detach_window_visible = true;
        self.detached_builder_size = Vec2::new(w, self.settings.detached_height_for_use());
        self.detached_live_size = None;
        self.settings.save();
    }

    fn reattach_chord_window(&mut self) {
        if let Some(size) = self.detached_live_size {
            self.settings.detached_chord_height = size.y.round() as i64;
        }
        self.detach_window_visible = false;
        self.settings.chord_window_detached = false;
        self.settings.save();
    }

    // ── Menu actions ───────────────────────────────────────────────────────

    fn apply_menu_action(&mut self, ctx: &egui::Context, action: MenuAction) {
        match action {
            MenuAction::SetSizePercent(p) => {
                self.settings.window_size_percent = p;
                self.settings.save();
                if self.detach_window_visible {
                    // 100ms debounce, restarted on changes (spec §5.7).
                    self.width_sync_deadline = Some(Instant::now() + DEBOUNCE_100MS);
                }
            }
            MenuAction::ToggleBorderless => {
                self.settings.borderless_mode = !self.settings.borderless_mode;
                self.settings.save();
            }
            MenuAction::SelectMidiInput => {
                let ports = midi::list_port_names();
                self.dialog = Some(if ports.is_empty() {
                    Dialog::NoMidiInput
                } else {
                    Dialog::MidiPicker {
                        ports,
                        selected: None,
                        current: self.midi_conn.as_ref().map(|c| c.port_name.clone()),
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
                };
                self.dialog = Some(Dialog::ColorPick {
                    target,
                    color: seed.to_color32(),
                });
            }
            MenuAction::ToggleDarkMode => {
                self.settings.dark_mode = !self.settings.dark_mode;
                self.settings.save();
            }
            MenuAction::ToggleKeytoggle => {
                self.settings.keytoggle_enabled = !self.settings.keytoggle_enabled;
                if !self.settings.keytoggle_enabled {
                    self.manual_notes.clear(); // disabling clears manual notes
                }
                self.settings.save();
                self.detection_tick(true);
            }
            MenuAction::ToggleNotePreference => {
                self.settings.prefer_flats = !self.settings.prefer_flats;
                self.detector.set_note_preference(self.settings.prefer_flats);
                self.settings.save();
                self.detection_tick(true); // refresh display immediately
            }
            MenuAction::ToggleChordDetection => {
                self.settings.chord_detection_enabled = !self.settings.chord_detection_enabled;
                if !self.settings.chord_detection_enabled {
                    self.current_chord = None;
                }
                self.settings.save();
                // Window resize follows from layout_sizes() on the next pass.
            }
            MenuAction::DetachChordWindow => self.detach_chord_window(),
            MenuAction::AttachChordWindow => self.reattach_chord_window(),
            MenuAction::TeachChordName => self.open_teach_dialog(),
            MenuAction::ManageTaughtChords => self.open_manage_dialog(),
            MenuAction::ShowAbout => self.dialog = Some(Dialog::About),
            MenuAction::ResetSettings => self.reset_settings(ctx),
        }
    }

    /// "Reset Settings to Default" (spec §9, D-UI-8).
    fn reset_settings(&mut self, ctx: &egui::Context) {
        let had_custom_font = self.settings.custom_font_path.is_some();
        let live_detached = self.detach_window_visible.then_some(self.detached_live_size).flatten();

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
        if had_custom_font {
            crate::fonts::install(ctx, None);
        }
        self.settings.save();
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
            apply_all_keys: false,
        });
    }

    /// Open "Manage Taught Chords…" listing all stored overrides.
    fn open_manage_dialog(&mut self) {
        let rows = self
            .detector
            .overrides()
            .map(|s| s.list(self.settings.prefer_flats))
            .unwrap_or_default();
        self.dialog = Some(Dialog::ManageTaught { rows });
    }

    fn apply_dialog_action(&mut self, ctx: &egui::Context, action: DialogAction) {
        match action {
            DialogAction::TeachSave {
                notes,
                name,
                apply_all_keys,
            } => {
                let set: HashSet<u8> = notes.iter().copied().collect();
                if let Some(store) = self.detector.overrides_mut() {
                    store.teach(&set, &name, apply_all_keys);
                }
                self.detection_tick(true); // re-detect immediately (D-UI-5)
            }
            DialogAction::DeleteOverride { intervals } => {
                if let Some(store) = self.detector.overrides_mut() {
                    store.delete(&intervals);
                }
                self.detection_tick(true);
            }
            DialogAction::ConnectPort(name) => {
                // Close the old port first (parity), then open the new one.
                self.midi_conn = None;
                match midi::connect_by_name(&name, self.midi_tx.clone(), ctx.clone()) {
                    Ok(conn) => self.midi_conn = Some(conn),
                    Err(e) => {
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
                }
                self.settings.save();
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
    let w = main_width(settings);
    let piano_h = (w as f64 / (1300.0 / 150.0)).trunc() as f32;
    let chord_visible = settings.chord_detection_enabled && !settings.chord_window_detached;
    let chord_h = if chord_visible {
        (50.0 * w as f64 / 1300.0).trunc() as f32
    } else {
        0.0
    };
    Vec2::new(w, piano_h + chord_h)
}

impl eframe::App for IvoryApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        self.process_midi_events();

        // Startup detach restore (100ms single-shot).
        if let Some(t) = self.startup_detach_at {
            if Instant::now() >= t {
                self.startup_detach_at = None;
                if self.settings.chord_window_detached && self.settings.chord_detection_enabled {
                    self.detach_window_visible = true;
                    self.detached_builder_size = Vec2::new(
                        main_width(&self.settings),
                        self.settings.detached_height_for_use(),
                    );
                }
            }
        }

        self.detection_tick(false);

        // Track our position on the monitor for global menu placement.
        let (inner_rect, monitor) =
            ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
        if let Some(r) = inner_rect {
            self.main_inner_origin = r.min;
        }
        self.monitor_size = monitor;

        // Fixed-size enforcement: Min+Max+Inner triple whenever the target
        // changes (size %, chord toggle, detach/attach).
        let (w, piano_h, chord_h) = self.layout_sizes();
        let target = Vec2::new(w, piano_h + chord_h);
        if self.last_sent_size != Some(target) {
            ctx.send_viewport_cmd(ViewportCommand::MinInnerSize(target));
            ctx.send_viewport_cmd(ViewportCommand::MaxInnerSize(target));
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(target));
            self.last_sent_size = Some(target);
        }
        // Borderless enforcement; Qt re-sets the title after flag changes.
        let decorations = !self.settings.borderless_mode;
        if self.decorations_sent != Some(decorations) {
            ctx.send_viewport_cmd(ViewportCommand::Decorations(decorations));
            ctx.send_viewport_cmd(ViewportCommand::Title("Ivory".to_owned()));
            self.decorations_sent = Some(decorations);
        }

        // Paint: chord strip on top, piano below (spec §3.1).
        let origin = ui.max_rect().min;
        let piano_rect = Rect::from_min_size(
            Pos2::new(origin.x, origin.y + chord_h),
            Vec2::new(w, piano_h),
        );
        if chord_h > 0.0 {
            let chord_rect = Rect::from_min_size(origin, Vec2::new(w, chord_h));
            chord_strip::draw(ui.painter(), chord_rect, self.current_chord.as_deref());
        }
        let display = self.display_notes();
        piano::draw(
            ui.painter(),
            piano_rect,
            &display,
            self.sustain_down,
            &self.settings,
        );

        self.handle_main_interaction(&ctx, ui, piano_rect);

        // Detached chord window.
        if self.detach_window_visible {
            let outcome = chord_strip::show_detached_window(
                &ctx,
                self.detached_builder_size,
                self.settings.borderless_mode,
                self.current_chord.as_deref(),
            );
            if let Some(size) = outcome.inner_size {
                self.detached_live_size = Some(size);
            }
            if outcome.close_requested {
                self.reattach_chord_window(); // close-to-reattach
            } else if let Some(pos) = outcome.context_menu_at {
                if self.dialog.is_none() {
                    self.open_menu_at(&ctx, pos);
                }
            }
        }

        // Debounced detached-window width sync.
        if let Some(deadline) = self.width_sync_deadline {
            if Instant::now() >= deadline {
                self.width_sync_deadline = None;
                if self.detach_window_visible {
                    if let Some(live) = self.detached_live_size {
                        chord_strip::sync_width(&ctx, w, live);
                    }
                }
            }
        }

        // Context menu viewport.
        if let Some(action) = menu::show(&ctx, &mut self.menu_state) {
            self.apply_menu_action(&ctx, action);
        }

        // Dialog viewport.
        if let Some(action) = dialogs::show(&ctx, &mut self.dialog, self.settings.dark_mode) {
            self.apply_dialog_action(&ctx, action);
        }

        // 50ms GUI cadence; MIDI events wake us sooner via request_repaint_of.
        ctx.request_repaint_after(GUI_TICK);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
