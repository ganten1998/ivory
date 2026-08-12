//! Dialogs (spec §7): MIDI picker (default styling), About (themed, D-UI-6),
//! color modals (egui picker, themed, D-UI-7), MIDI info/error boxes.
//! Each dialog is its own decorated immediate viewport, like Qt's top-level
//! dialogs (the fixed-size main window is too small to host them).

use crate::fonts;
use crate::menu::ColorTarget;
use egui::{
    Align, Button, Color32, CornerRadius, FontId, Layout, RichText, Stroke, Vec2, ViewportBuilder,
    ViewportId,
};
use ivory_core::OverrideInfo;

pub enum Dialog {
    MidiPicker {
        ports: Vec<String>,
        selected: Option<usize>,
        current: Option<String>,
    },
    NoMidiInput,
    MidiError {
        message: String,
    },
    About,
    /// Paste a supporter key. Stays open on failure so the message can explain
    /// what went wrong without the user losing what they pasted.
    SupporterKey {
        input: String,
        /// Feedback from the last attempt, if any.
        message: Option<String>,
        /// Name on the currently installed licence, when there is one.
        installed_as: Option<String>,
    },
    ColorPick {
        target: ColorTarget,
        color: Color32,
    },
    // D-UI-5 teach dialogs.
    TeachChord {
        /// Snapshot of the held MIDI notes (used to key the override on save).
        notes: Vec<u8>,
        /// Held notes pre-rendered as names under the active preference.
        note_names: String,
        /// The current detected label (or a placeholder when none).
        current_label: String,
        /// Editable name, prefilled with the current label.
        input: String,
        apply_all_keys: bool,
    },
    ManageTaught {
        rows: Vec<OverrideInfo>,
        /// D-UI-9 footer: learned-re-ranker state, refreshed on open.
        learning: LearningStatus,
    },
    // D-UI-9 learned re-ranker.
    /// Pick a different reading for the held voicing and train toward it.
    CorrectChord {
        notes: Vec<u8>,
        note_names: String,
        current_label: String,
        /// The names the re-ranker can actually be trained toward, best first.
        candidates: Vec<(String, f64)>,
        selected: Option<usize>,
    },
    /// Plain themed report of what a correction achieved.
    LearnResult {
        title: &'static str,
        message: String,
    },
}

/// Snapshot of learned-re-ranker state for display.
#[derive(Clone, Default)]
pub struct LearningStatus {
    pub on: bool,
    pub corrections: u32,
    pub has_learned: bool,
    /// One row per perceptron feature: (label, weight).
    pub weights: Vec<(&'static str, f64)>,
}

pub enum DialogAction {
    ConnectPort(String),
    /// Try to install a pasted supporter key. The app reports the outcome by
    /// updating or closing the dialog.
    InstallLicense {
        key: String,
    },
    ApplyColor(ColorTarget, Color32),
    /// Teach `name` for the held `notes`, transposition-invariant when
    /// `apply_all_keys` and the name begins with a note name.
    TeachSave {
        notes: Vec<u8>,
        name: String,
        apply_all_keys: bool,
    },
    /// Delete the override with this interval-set-from-bass key.
    DeleteOverride {
        intervals: Vec<u8>,
    },
    /// D-UI-9: train the re-ranker to read `notes` as `name`.
    TrainCorrection {
        notes: Vec<u8>,
        name: String,
    },
    /// D-UI-9: discard all learned weights.
    ForgetLearning,
}

fn dialog_vp_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-dialog")
}

pub fn color_pick_title(target: ColorTarget) -> &'static str {
    match target {
        ColorTarget::WhiteIdle => "Choose White Key Color",
        ColorTarget::BlackIdle => "Choose Black Key Color",
        ColorTarget::Active => "Choose Active Key Color",
        ColorTarget::Sustain => "Choose Sustain Pedal Color",
        ColorTarget::ChordText => "Choose Chord Color",
    }
}

/// Theme palette shared by About and the color modals (spec §7.3 / D-UI-7).
struct DialogTheme {
    bg: Color32,
    text: Color32,
    button_bg: Color32,
    button_hover: Color32,
    button_border: Color32,
}

fn theme(dark: bool) -> DialogTheme {
    if dark {
        DialogTheme {
            bg: Color32::from_rgb(0x00, 0x00, 0x00),
            text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            button_bg: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            button_hover: Color32::from_rgb(0x2a, 0x2a, 0x2a),
            button_border: Color32::from_rgb(0xE8, 0xDC, 0xC0),
        }
    } else {
        DialogTheme {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            text: Color32::from_rgb(0x00, 0x00, 0x00),
            button_bg: Color32::from_rgb(0xd4, 0xc8, 0xb0),
            button_hover: Color32::from_rgb(0xc0, 0xb4, 0x9c),
            button_border: Color32::from_rgb(0x00, 0x00, 0x00),
        }
    }
}

fn apply_theme(style: &mut egui::Style, t: &DialogTheme) {
    style.spacing.button_padding = egui::vec2(12.0, 4.0); // Qt: padding 4px 12px
    let set = |wv: &mut egui::style::WidgetVisuals, bg: Color32| {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::new(1.0, t.button_border);
        wv.fg_stroke = Stroke::new(1.0, t.text);
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
    };
    set(&mut style.visuals.widgets.inactive, t.button_bg);
    set(&mut style.visuals.widgets.hovered, t.button_hover);
    set(&mut style.visuals.widgets.active, t.button_hover);
    set(&mut style.visuals.widgets.open, t.button_hover);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.text);
    style.visuals.override_text_color = None;
    style.visuals.window_fill = t.bg;
    style.visuals.selection.bg_fill = t.button_hover;
    style.visuals.selection.stroke = Stroke::new(1.0, t.text);
    style.visuals.hyperlink_color = t.text;
}

/// Stock egui styling for the MIDI dialogs, which Qt showed with the platform
/// default look, NOT the app theme (spec §7.1).
fn stock_style(ctx: &egui::Context) -> egui::Style {
    egui::Style {
        visuals: match ctx.input(|i| i.raw.system_theme) {
            Some(egui::Theme::Dark) => egui::Visuals::dark(),
            _ => egui::Visuals::light(),
        },
        ..Default::default()
    }
}

struct VpResult {
    close: bool,
}

fn show_dialog_viewport(
    ctx: &egui::Context,
    title: &str,
    size: Vec2,
    min_size: Vec2,
    content: impl FnMut(&mut egui::Ui, &mut VpResult),
) -> VpResult {
    let mut content = content;
    let mut result = VpResult { close: false };
    let builder = ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size(min_size)
        .with_resizable(true)
        .with_decorations(true);
    ctx.show_viewport_immediate(dialog_vp_id(), builder, |ui, _class| {
        let (close_req, esc) = ui.input(|i| {
            (
                i.viewport().close_requested(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if close_req || esc {
            result.close = true; // window close / Esc == Cancel (strict no-op)
        }
        content(ui, &mut result);
    });
    result
}

/// Render the active dialog. Returns an action for the app to apply;
/// `dialog_opt` becomes None when the dialog closes.
pub fn show(
    ctx: &egui::Context,
    dialog_opt: &mut Option<Dialog>,
    dark_mode: bool,
) -> Option<DialogAction> {
    let dialog = dialog_opt.as_mut()?;
    let mut action = None;

    let result = match dialog {
        Dialog::MidiPicker {
            ports,
            selected,
            current,
        } => {
            let stock = stock_style(ctx);
            show_dialog_viewport(
                ctx,
                "Select MIDI Input",
                Vec2::new(400.0, 300.0),
                Vec2::new(400.0, 200.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.label("Select MIDI input port:");
                            if let Some(current) = current.as_deref() {
                                ui.label(format!("Current: {current}"));
                            }
                            ui.add_space(4.0);
                            let bottom_h = 34.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for (i, name) in ports.iter().enumerate() {
                                        if ui
                                            .selectable_label(*selected == Some(i), name)
                                            .clicked()
                                        {
                                            *selected = Some(i);
                                        }
                                    }
                                });
                            ui.add_space(6.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    if let Some(i) = *selected {
                                        if let Some(name) = ports.get(i) {
                                            action =
                                                Some(DialogAction::ConnectPort(name.clone()));
                                        }
                                    }
                                    result.close = true;
                                }
                                if ui.button("Cancel").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::NoMidiInput => {
            let stock = stock_style(ctx);
            show_dialog_viewport(
                ctx,
                "No MIDI Input",
                Vec2::new(420.0, 170.0),
                Vec2::new(320.0, 140.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                "No MIDI input ports found!\n\nYou can still use keytoggle mode by enabling it in the context menu.",
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::MidiError { message } => {
            let stock = stock_style(ctx);
            let text = format!(
                "Error opening MIDI port:\n{message}\n\nYou can still use keytoggle mode."
            );
            show_dialog_viewport(
                ctx,
                "MIDI Error",
                Vec2::new(420.0, 170.0),
                Vec2::new(320.0, 140.0),
                |ui, result| {
                    *ui.style_mut() = stock.clone();
                    let bg = ui.style().visuals.window_fill;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(text.as_str());
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("OK").clicked() {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::About => {
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                "About Ivory",
                Vec2::new(400.0, 190.0),
                Vec2::new(400.0, 150.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(9))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("Ivory").font(bold(16.0)).color(t.text),
                                );
                                ui.label(
                                    RichText::new(
                                        "Simple MIDI Keyboard Monitor with Advanced Chord Detection",
                                    )
                                    .font(bold(10.0))
                                    .color(t.text),
                                );
                                ui.hyperlink_to(
                                    RichText::new("shambhaline@neocities.org")
                                        .font(bold(10.0))
                                        .color(t.text)
                                        .underline(),
                                    "https://shambhaline.neocities.org",
                                );
                            });
                            // Stretch, then bottom block.
                            let bottom_h = 62.0;
                            ui.add_space((ui.available_height() - bottom_h).max(0.0));
                            ui.label(
                                RichText::new(concat!("Version ", env!("CARGO_PKG_VERSION")))
                                    .font(bold(8.0))
                                    .color(t.text),
                            );
                            // D-UI-6: one extra 8pt credit line under the version.
                            ui.label(
                                RichText::new(
                                    "Courier Prime © The Courier Prime Project Authors, SIL OFL 1.1",
                                )
                                .font(bold(8.0))
                                .color(t.text),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::ColorPick { target, color } => {
            let t = theme(dark_mode);
            let target = *target;
            show_dialog_viewport(
                ctx,
                color_pick_title(target),
                Vec2::new(320.0, 400.0),
                Vec2::new(280.0, 320.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(9))
                        .show(ui, |ui| {
                            egui::color_picker::color_picker_color32(
                                ui,
                                color,
                                egui::color_picker::Alpha::Opaque,
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    // OK applies + saves; Cancel/close is a no-op (D-UI-7).
                                    action = Some(DialogAction::ApplyColor(target, *color));
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::SupporterKey {
            input,
            message,
            installed_as,
        } => {
            let t = theme(dark_mode);
            show_dialog_viewport(
                ctx,
                "Supporter Key",
                Vec2::new(460.0, 290.0),
                Vec2::new(400.0, 250.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            if let Some(name) = installed_as.as_deref() {
                                ui.label(
                                    RichText::new(format!("Supporter: {name}"))
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            } else {
                                ui.label(
                                    RichText::new("Ivory is free. A supporter key")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                                ui.label(
                                    RichText::new("unlocks the extras — nothing else.")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            }
                            ui.add_space(8.0);
                            ui.label(RichText::new("Paste your key:").font(bold(11.0)).color(t.text));
                            let edit = egui::TextEdit::multiline(input)
                                .font(bold(11.0))
                                .text_color(t.text)
                                .desired_width(f32::INFINITY)
                                .desired_rows(5);
                            ui.add(edit);
                            if let Some(msg) = message.as_deref() {
                                ui.add_space(4.0);
                                ui.label(RichText::new(msg).font(bold(11.0)).color(t.text));
                            }
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ok = !input.trim().is_empty();
                                if ui
                                    .add_enabled(
                                        ok,
                                        Button::new(RichText::new("Activate").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    action = Some(DialogAction::InstallLicense {
                                        key: input.clone(),
                                    });
                                    // NOT closed here: the app decides, so a bad
                                    // key can report why without losing the paste.
                                }
                                if ui
                                    .add(Button::new(RichText::new("Close").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::TeachChord {
            notes,
            note_names,
            current_label,
            input,
            apply_all_keys,
        } => {
            let t = theme(dark_mode);
            let notes = notes.clone();
            show_dialog_viewport(
                ctx,
                "Teach Chord Name",
                Vec2::new(420.0, 240.0),
                Vec2::new(360.0, 200.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.visuals_mut().extreme_bg_color = t.bg;
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Notes: {note_names}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(format!("Detected: {current_label}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space(6.0);
                            ui.label(RichText::new("Name:").font(bold(11.0)).color(t.text));
                            let edit = egui::TextEdit::singleline(input)
                                .font(bold(12.0))
                                .text_color(t.text)
                                .desired_width(f32::INFINITY);
                            ui.add(edit);
                            ui.add_space(6.0);
                            let mut apply = *apply_all_keys;
                            if ui
                                .checkbox(
                                    &mut apply,
                                    RichText::new("Apply in all keys")
                                        .font(bold(11.0))
                                        .color(t.text),
                                )
                                .changed()
                            {
                                *apply_all_keys = apply;
                            }
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let ok_enabled = !input.trim().is_empty();
                                if ui
                                    .add_enabled(
                                        ok_enabled,
                                        Button::new(RichText::new("OK").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    action = Some(DialogAction::TeachSave {
                                        notes: notes.clone(),
                                        name: input.trim().to_owned(),
                                        apply_all_keys: *apply_all_keys,
                                    });
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::CorrectChord {
            notes,
            note_names,
            current_label,
            candidates,
            selected,
        } => {
            let t = theme(dark_mode);
            let notes = notes.clone();
            show_dialog_viewport(
                ctx,
                "Correct Chord Name",
                Vec2::new(470.0, 370.0),
                Vec2::new(400.0, 290.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Notes: {note_names}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.label(
                                RichText::new(format!("Now reads: {current_label}"))
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space(6.0);
                            ui.label(
                                RichText::new("Which reading would you rather see?")
                                    .font(bold(11.0))
                                    .color(t.text),
                            );
                            ui.add_space(2.0);
                            // Explain the list's limits up front — these are the
                            // only names the re-ranker can be trained toward.
                            // 4 explanatory lines at 9pt + spacing + buttons.
                            let bottom_h = 90.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for (i, (name, score)) in candidates.iter().enumerate() {
                                        let is_current = current_label == name
                                            || current_label
                                                .strip_prefix(name.as_str())
                                                .is_some_and(|r| r.starts_with('/'));
                                        let text = if is_current {
                                            format!("{name}   (current)")
                                        } else {
                                            name.clone()
                                        };
                                        ui.horizontal(|ui| {
                                            if ui
                                                .selectable_label(
                                                    *selected == Some(i),
                                                    RichText::new(text)
                                                        .font(bold(12.0))
                                                        .color(t.text),
                                                )
                                                .clicked()
                                            {
                                                *selected = Some(i);
                                            }
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(format!("{score:.0}"))
                                                            .font(bold(10.0))
                                                            .color(t.text.gamma_multiply(0.55)),
                                                    );
                                                },
                                            );
                                        });
                                    }
                                });
                            ui.add_space(4.0);
                            // Measured, not hand-waved: see
                            // ivory-core/tests/blast_radius.rs.
                            ui.label(
                                RichText::new(
                                    "Ivory learns a general leaning, not this one chord.\n\
                                     One correction changes about 1 chord in 10 overall,\n\
                                     often in unrelated keys. \"Forget Learning\" in Manage\n\
                                     Taught Chords undoes all of it exactly.",
                                )
                                .font(bold(9.0))
                                .color(t.text.gamma_multiply(0.75)),
                            );
                            ui.add_space(4.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let pick = selected.and_then(|i| candidates.get(i));
                                if ui
                                    .add_enabled(
                                        pick.is_some(),
                                        Button::new(RichText::new("Learn").color(t.text)),
                                    )
                                    .clicked()
                                {
                                    if let Some((name, _)) = pick {
                                        action = Some(DialogAction::TrainCorrection {
                                            notes: notes.clone(),
                                            name: name.clone(),
                                        });
                                    }
                                    result.close = true;
                                }
                                if ui
                                    .add(Button::new(RichText::new("Cancel").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::LearnResult { title, message } => {
            let t = theme(dark_mode);
            let title = *title;
            let message = message.clone();
            // Size from the content. These messages range from one line to ten,
            // and a fixed height pushed the OK button off the bottom edge of the
            // longest one — where only Esc could still dismiss it. 12pt Courier
            // Prime rows are ~13.5pt; the authored lines are under 60 chars, so
            // the 446pt content width does not wrap them.
            let lines = message.lines().count().max(1) as f32;
            let height = (lines * 13.5 + 24.0 + 30.0 + 16.0).clamp(150.0, 460.0);
            show_dialog_viewport(
                ctx,
                title,
                Vec2::new(470.0, height),
                Vec2::new(400.0, height.min(200.0)),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(message.as_str())
                                    .font(bold(12.0))
                                    .color(t.text),
                            );
                            ui.add_space((ui.available_height() - 30.0).max(0.0));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("OK").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                            });
                        });
                },
            )
        }

        Dialog::ManageTaught { rows, learning } => {
            let t = theme(dark_mode);
            let mut to_delete: Option<Vec<u8>> = None;
            let mut forget = false;
            // Snapshot for the closure; the live binding is updated afterwards.
            let learning_view = learning.clone();
            let r = show_dialog_viewport(
                ctx,
                "Manage Taught Chords",
                Vec2::new(460.0, 460.0),
                Vec2::new(360.0, 320.0),
                |ui, result| {
                    apply_theme(ui.style_mut(), &t);
                    ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg);
                    let bold = |size: f32| FontId::new(size, fonts::courier_bold());
                    egui::Frame::NONE
                        .inner_margin(egui::Margin::same(12))
                        .show(ui, |ui| {
                            if rows.is_empty() {
                                ui.label(
                                    RichText::new("No taught chords yet.")
                                        .font(bold(12.0))
                                        .color(t.text),
                                );
                            }
                            // List, then the D-UI-9 learning footer, then the
                            // buttons. Reserve the footer's REAL height (it grows
                            // one 9pt line per non-zero weight, up to seven) —
                            // a fixed guess clips the buttons off the bottom
                            // once training fills every feature in.
                            const FOOTER_LINE_H: f32 = 13.0;
                            let weight_rows = learning_view
                                .weights
                                .iter()
                                .filter(|(_, w)| *w != 0.0)
                                .count() as f32;
                            let footer_h = 8.0 // separator
                                + 16.0 // status line
                                + if learning_view.has_learned {
                                    FOOTER_LINE_H * (1.0 + weight_rows)
                                } else {
                                    FOOTER_LINE_H * 2.0 // the two-line hint
                                };
                            let bottom_h = 34.0 + footer_h + 6.0;
                            let list_h = (ui.available_height() - bottom_h).max(40.0);
                            egui::ScrollArea::vertical()
                                .max_height(list_h)
                                .auto_shrink([false, false])
                                .show(ui, |ui| {
                                    for row in rows.iter() {
                                        ui.horizontal(|ui| {
                                            if ui
                                                .add(Button::new(
                                                    RichText::new("Delete").color(t.text),
                                                ))
                                                .clicked()
                                            {
                                                to_delete = Some(row.intervals.clone());
                                            }
                                            ui.label(
                                                RichText::new(format!(
                                                    "{}   [{}]",
                                                    row.display_name, row.voicing
                                                ))
                                                .font(bold(12.0))
                                                .color(t.text),
                                            );
                                        });
                                    }
                                });
                            ui.add_space(6.0);
                            ui.separator();
                            ui.label(
                                RichText::new(format!(
                                    "Chord learning: {}  ({} correction{})",
                                    if learning_view.on { "ON" } else { "off" },
                                    learning_view.corrections,
                                    if learning_view.corrections == 1 { "" } else { "s" },
                                ))
                                .font(bold(11.0))
                                .color(t.text),
                            );
                            if learning_view.has_learned {
                                ui.label(
                                    RichText::new("Learned leanings:")
                                        .font(bold(9.0))
                                        .color(t.text.gamma_multiply(0.75)),
                                );
                                for (label, w) in learning_view.weights.iter() {
                                    if *w == 0.0 {
                                        continue;
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "   {label}: {}{:.0}",
                                            if *w > 0.0 { "+" } else { "" },
                                            w
                                        ))
                                        .font(bold(9.0))
                                        .color(t.text.gamma_multiply(0.75)),
                                    );
                                }
                            } else {
                                // Forget leaves the switch armed but the weights
                                // empty. Say outright that readings are stock, or
                                // "ON (0 corrections)" reads as a failed undo.
                                ui.label(
                                    RichText::new(if learning_view.on {
                                        "Nothing learned — chord names are stock.\n\
                                         Use \"Correct Chord Name...\" to teach a leaning."
                                    } else {
                                        "Nothing learned yet. Play a chord, then use\n\
                                         \"Correct Chord Name...\" in the menu."
                                    })
                                    .font(bold(9.0))
                                    .color(t.text.gamma_multiply(0.75)),
                                );
                            }
                            ui.add_space(6.0);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add(Button::new(RichText::new("Close").color(t.text)))
                                    .clicked()
                                {
                                    result.close = true;
                                }
                                if ui
                                    .add_enabled(
                                        learning_view.has_learned,
                                        Button::new(
                                            RichText::new("Forget Learning").color(t.text),
                                        ),
                                    )
                                    .clicked()
                                {
                                    forget = true;
                                }
                            });
                        });
                },
            );
            if let Some(intervals) = to_delete {
                // Reflect the deletion locally so the list updates immediately,
                // and tell the app to persist it.
                rows.retain(|r| r.intervals != intervals);
                action = Some(DialogAction::DeleteOverride { intervals });
            }
            if forget {
                // Same immediate-feedback pattern as Delete: clear the footer
                // locally so the dialog updates now, and let the app persist.
                learning.has_learned = false;
                learning.corrections = 0;
                learning.weights.clear();
                action = Some(DialogAction::ForgetLearning);
            }
            r
        }
    };

    if result.close {
        *dialog_opt = None;
    }
    action
}
