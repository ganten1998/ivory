use std::collections::{HashMap, HashSet};
use std::sync::mpsc;
use eframe::egui;
use egui::{Color32, FontId, RichText, Vec2};
use ivory_core::ChordDetector;

use crate::midi::{MidiEvent, MidiThread};
use crate::piano_widget;
use crate::settings::Settings;

pub struct IvoryApp {
    // MIDI state
    active_notes: HashMap<u8, ()>,   // note → ()
    sustain_active: bool,
    sustained_notes: HashSet<u8>,    // notes held by pedal after key-up
    keytoggle_notes: HashSet<u8>,    // manually toggled notes

    // MIDI channel
    midi_rx: mpsc::Receiver<MidiEvent>,
    _midi_thread: Option<MidiThread>,

    // Chord detection
    chord_detector: ChordDetector,
    current_chord: Option<String>,
    debug_candidates: Vec<(String, f64)>,

    // Settings
    pub settings: Settings,

    // UI state
    show_settings: bool,
    show_debug: bool,
    midi_port_list: Vec<String>,
    status_msg: String,
}

impl IvoryApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // Set up fonts that support the Unicode symbols Ivory uses (Δ, ø, etc.)
        let fonts = egui::FontDefinitions::default();
        cc.egui_ctx.set_fonts(fonts);

        let settings = Settings::load();
        let mut chord_detector = ChordDetector::new();
        chord_detector.set_note_preference(settings.prefer_flats);

        let (tx, rx) = mpsc::channel::<MidiEvent>();
        let midi_thread = MidiThread::connect(tx);
        let port_list = MidiThread::list_ports();
        let status = if midi_thread.is_some() {
            "MIDI connected".to_string()
        } else {
            "No MIDI device found".to_string()
        };

        Self {
            active_notes: HashMap::new(),
            sustain_active: false,
            sustained_notes: HashSet::new(),
            keytoggle_notes: HashSet::new(),
            midi_rx: rx,
            _midi_thread: midi_thread,
            chord_detector,
            current_chord: None,
            debug_candidates: Vec::new(),
            settings,
            show_settings: false,
            show_debug: false,
            midi_port_list: port_list,
            status_msg: status,
        }
    }

    // ── MIDI processing ───────────────────────────────────────────────────────

    fn process_midi(&mut self) {
        while let Ok(event) = self.midi_rx.try_recv() {
            match event {
                MidiEvent::NoteOn { note, .. } => {
                    self.active_notes.insert(note, ());
                    self.sustained_notes.remove(&note);
                }
                MidiEvent::NoteOff { note } => {
                    if self.sustain_active {
                        // Keep note visually but mark it as sustained
                        self.sustained_notes.insert(note);
                    } else {
                        self.active_notes.remove(&note);
                    }
                }
                MidiEvent::SustainPedal { active } => {
                    self.sustain_active = active;
                    if !active {
                        // Release all sustained notes
                        for note in self.sustained_notes.drain() {
                            self.active_notes.remove(&note);
                        }
                    }
                }
            }
        }
    }

    // ── Chord detection ───────────────────────────────────────────────────────

    fn update_chord(&mut self) {
        if !self.settings.chord_detection_enabled {
            self.current_chord = None;
            self.debug_candidates.clear();
            return;
        }

        // Combine MIDI notes and keytoggle notes
        let mut notes: HashSet<u8> = self.active_notes.keys().copied().collect();
        notes.extend(self.keytoggle_notes.iter().copied());

        if notes.len() < 2 {
            self.current_chord = None;
            self.debug_candidates.clear();
            return;
        }

        if self.show_debug {
            let (chord, candidates) = self.chord_detector.detect_chord_debug(&notes, 5);
            self.current_chord = chord;
            self.debug_candidates = candidates;
        } else {
            self.current_chord = self.chord_detector.detect_chord(&notes);
            self.debug_candidates.clear();
        }
    }

    // ── Settings window ───────────────────────────────────────────────────────

    fn show_settings_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_settings;
        egui::Window::new("Settings")
            .open(&mut open)
            .resizable(false)
            .show(ctx, |ui| {
                egui::Grid::new("settings_grid")
                    .num_columns(2)
                    .spacing([12.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("Dark mode");
                        ui.checkbox(&mut self.settings.dark_mode, "");
                        ui.end_row();

                        ui.label("Prefer flats");
                        if ui.checkbox(&mut self.settings.prefer_flats, "").changed() {
                            self.chord_detector.set_note_preference(self.settings.prefer_flats);
                        }
                        ui.end_row();

                        ui.label("Chord detection");
                        ui.checkbox(&mut self.settings.chord_detection_enabled, "");
                        ui.end_row();

                        ui.label("Keytoggle mode");
                        ui.checkbox(&mut self.settings.keytoggle_enabled, "");
                        ui.end_row();

                        ui.label("Debug panel");
                        ui.checkbox(&mut self.show_debug, "");
                        ui.end_row();
                    });

                ui.separator();
                if ui.button("Save").clicked() {
                    self.settings.save();
                }
            });
        self.show_settings = open;
    }
}

impl eframe::App for IvoryApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll MIDI
        self.process_midi();

        // Chord detection (every frame, cheap)
        self.update_chord();

        // Settings window
        if self.show_settings {
            self.show_settings_window(ctx);
        }

        // Set background
        let bg = if self.settings.dark_mode {
            Color32::from_rgb(30, 30, 30)
        } else {
            Color32::from_rgb(232, 232, 232)
        };

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(bg))
            .show(ctx, |ui| {
                // ── Context menu ─────────────────────────────────────────────
                ui.interact(ui.max_rect(), egui::Id::new("bg"), egui::Sense::click())
                    .context_menu(|ui| {
                        if ui.button("Settings…").clicked() {
                            self.show_settings = !self.show_settings;
                            ui.close_menu();
                        }
                        ui.checkbox(&mut self.show_debug, "Debug mode");
                        if ui.button("Clear notes").clicked() {
                            self.active_notes.clear();
                            self.keytoggle_notes.clear();
                            self.sustained_notes.clear();
                            ui.close_menu();
                        }
                        ui.separator();
                        ui.label(format!("MIDI: {}", self.status_msg));
                    });

                ui.vertical(|ui| {
                    // ── Chord label ───────────────────────────────────────────
                    let chord_h = 60.0;
                    let (chord_rect, _) =
                        ui.allocate_exact_size(Vec2::new(ui.available_width(), chord_h), egui::Sense::hover());
                    let painter = ui.painter_at(chord_rect);
                    painter.rect_filled(chord_rect, 0.0, Color32::BLACK);

                    if let Some(ref chord) = self.current_chord {
                        let text_col = Color32::from_rgb(232, 220, 192);
                        let font_size = (chord_h * 0.6).max(12.0);
                        painter.text(
                            chord_rect.center(),
                            egui::Align2::CENTER_CENTER,
                            chord,
                            FontId::monospace(font_size),
                            text_col,
                        );
                    }

                    // ── Debug panel ───────────────────────────────────────────
                    if self.show_debug && !self.debug_candidates.is_empty() {
                        let debug_h = 120.0;
                        let (drect, _) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), debug_h),
                            egui::Sense::hover(),
                        );
                        let painter = ui.painter_at(drect);
                        painter.rect_filled(drect, 0.0, Color32::from_rgb(20, 20, 40));
                        let mut y = drect.top() + 4.0;
                        for (name, score) in &self.debug_candidates {
                            painter.text(
                                egui::pos2(drect.left() + 8.0, y),
                                egui::Align2::LEFT_TOP,
                                format!("{:>7.1}  {}", score, name),
                                FontId::monospace(14.0),
                                Color32::from_rgb(180, 220, 180),
                            );
                            y += 18.0;
                        }
                    }

                    // ── Piano ─────────────────────────────────────────────────
                    let available_w = ui.available_width();
                    let piano_h = piano_widget::height_for_width(available_w);
                    let (piano_rect, piano_response) = ui.allocate_exact_size(
                        Vec2::new(available_w, piano_h),
                        egui::Sense::click(),
                    );

                    // Draw
                    {
                        let painter = ui.painter_at(piano_rect);
                        piano_widget::draw_piano(
                            &painter,
                            piano_rect,
                            &self.active_notes,
                            self.sustain_active,
                            &self.settings,
                            self.settings.keytoggle_enabled,
                        );
                    }

                    // Keytoggle click handling
                    if self.settings.keytoggle_enabled {
                        if piano_response.clicked() {
                            if let Some(pos) = piano_response.interact_pointer_pos() {
                                if let Some(note) = piano_widget::hit_test(pos, piano_rect) {
                                    if self.keytoggle_notes.contains(&note) {
                                        self.keytoggle_notes.remove(&note);
                                    } else {
                                        self.keytoggle_notes.insert(note);
                                    }
                                }
                            }
                        }
                    }

                    // ── Status bar ────────────────────────────────────────────
                    ui.add_space(2.0);
                    ui.horizontal(|ui| {
                        let col = Color32::from_gray(120);
                        ui.label(RichText::new(&self.status_msg).color(col).small());
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let note_count = self.active_notes.len() + self.keytoggle_notes.len();
                            ui.label(
                                RichText::new(format!("{} notes", note_count))
                                    .color(col)
                                    .small(),
                            );
                        });
                    });
                });
            });

        // Request repaint continually so MIDI is responsive
        ctx.request_repaint();
    }
}
