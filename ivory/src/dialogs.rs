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
    ColorPick {
        target: ColorTarget,
        color: Color32,
    },
    // TEACH-HOOK(D-UI-5): TeachChord { .. } and ManageTaught { .. } dialogs
    // are added here by a later agent (notes + current label + name input +
    // "apply in all keys"; list + delete).
}

pub enum DialogAction {
    ConnectPort(String),
    ApplyColor(ColorTarget, Color32),
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
    let mut style = egui::Style::default();
    style.visuals = match ctx.input(|i| i.raw.system_theme) {
        Some(egui::Theme::Dark) => egui::Visuals::dark(),
        _ => egui::Visuals::light(),
    };
    style
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
    let Some(dialog) = dialog_opt.as_mut() else {
        return None;
    };
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
                                RichText::new("Version 2.0.0").font(bold(8.0)).color(t.text),
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
    };

    if result.close {
        *dialog_opt = None;
    }
    action
}
