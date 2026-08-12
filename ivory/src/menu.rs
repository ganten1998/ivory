//! The right-click context menu (spec §6) — the entire UI surface.
//!
//! Qt shows menus as their own top-level popup windows; the fixed 200px-tall
//! main window cannot host a ~460px menu, so the menu is rendered in its own
//! borderless immediate viewport at the global cursor position. The Size
//! submenu is a second sibling viewport to its right (Qt-like placement).
//!
//! Chrome parity (spec §6.1): bold Courier Prime, per-mode colors, item
//! padding 4px 20px, no rounding, 1px separators, toggle items rename
//! themselves (no checkmarks anywhere).

use crate::fonts;
use egui::{
    Button, Color32, CornerRadius, FontId, Margin, Pos2, Stroke, Vec2, ViewportBuilder, ViewportId,
};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorTarget {
    WhiteIdle,
    BlackIdle,
    Active,
    Sustain,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuAction {
    SetSizePercent(i64),
    ToggleBorderless,
    SelectMidiInput,
    PickColor(ColorTarget),
    ToggleDarkMode,
    ToggleKeytoggle,
    ToggleNotePreference,
    ToggleChordDetection,
    /// Cycle the built-in UI typeface (Courier Prime <-> Terminess).
    CycleFont,
    /// Supporter extra: halo under held keys, in the active key color.
    ToggleKeyGlow,
    /// Supporter extra: 16-segment chord readout.
    ToggleSegmentDisplay,
    DetachChordWindow,
    AttachChordWindow,
    TeachChordName,
    ManageTaughtChords,
    /// D-UI-9: correct the current reading, training the learned re-ranker.
    CorrectChordName,
    /// D-UI-9: master switch for the learned re-ranker (weights are kept).
    ToggleChordLearning,
    ShowAbout,
    ResetSettings,
}

/// Everything the menu needs to know to render its labels.
#[derive(Clone, Copy)]
pub struct MenuView {
    pub dark_mode: bool,
    pub borderless: bool,
    pub keytoggle: bool,
    pub prefer_flats: bool,
    pub detection_enabled: bool,
    pub detached: bool,
    /// "Teach Chord Name..." and "Correct Chord Name..." are greyed when no
    /// notes are held — both act on the voicing you are playing.
    pub notes_held: bool,
    /// D-UI-9: whether the learned re-ranker is currently influencing readings.
    pub learning_on: bool,
    /// Label of the typeface that will be active after the next Font click.
    /// `None` hides the row entirely (only Courier Prime is available here).
    pub next_font: Option<&'static str>,
    /// A valid supporter license is installed. Gates the extras block.
    pub supporter: bool,
    pub glow_on: bool,
    pub segment_on: bool,
}

#[derive(Clone, Copy)]
pub struct MenuColors {
    pub bg: Color32,
    pub text: Color32,
    pub sel: Color32,
    pub sep: Color32,
}

pub fn colors(dark_mode: bool) -> MenuColors {
    if dark_mode {
        MenuColors {
            bg: Color32::from_rgb(0x00, 0x00, 0x00),
            text: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            sel: Color32::from_rgb(0x1a, 0x1a, 0x1a),
            sep: Color32::from_rgb(0xE8, 0xDC, 0xC0),
        }
    } else {
        MenuColors {
            bg: Color32::from_rgb(0xE8, 0xDC, 0xC0),
            text: Color32::from_rgb(0x00, 0x00, 0x00),
            sel: Color32::from_rgb(0xd4, 0xc8, 0xb0),
            sep: Color32::from_rgb(0x00, 0x00, 0x00),
        }
    }
}

const MENU_FONT_SIZE: f32 = 13.0;
const PAD_X: f32 = 20.0; // Qt item padding 4px 20px
const PAD_Y: f32 = 4.0;
const SEP_H: f32 = 3.0; // 1px line + 1px margin above/below
const SIZE_PERCENTS: [i64; 7] = [50, 75, 100, 125, 150, 175, 200];
const ARROW: &str = "\u{23F5}"; // ⏵ submenu indicator

enum Entry {
    Separator,
    Item {
        label: String,
        action: MenuAction,
        enabled: bool,
    },
    SizeParent,
}

pub struct MenuState {
    pos: Pos2, // global (monitor points), top-left
    size: Vec2,
    entries: Vec<Entry>,
    row_h: f32,        // uniform item height; buttons are forced to it so the
    // stacked rows exactly fill the computed viewport size
    size_row_top: f32, // y offset of the Size row within the menu
    submenu_open: bool,
    submenu_size: Vec2,
    dark_mode: bool,
    opened_at: Instant,
    saw_focus: bool,
}

fn menu_vp_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-menu")
}

fn submenu_vp_id() -> ViewportId {
    ViewportId::from_hash_of("ivory-menu-sub")
}

fn build_entries(view: MenuView) -> Vec<Entry> {
    let item = |label: &str, action: MenuAction| Entry::Item {
        label: label.to_owned(),
        action,
        enabled: true,
    };
    let mut e = Vec::new();
    // 1. Size submenu
    e.push(Entry::SizeParent);
    e.push(Entry::Separator);
    // 3. Borderless toggle (label shows what you would switch TO the current
    //    state from: "Borderless" while bordered, "Bordered" while borderless)
    e.push(item(
        if view.borderless { "Bordered" } else { "Borderless" },
        MenuAction::ToggleBorderless,
    ));
    e.push(Entry::Separator);
    e.push(item("Select MIDI Input...", MenuAction::SelectMidiInput));
    e.push(Entry::Separator);
    e.push(item(
        "Set White Key Color...",
        MenuAction::PickColor(ColorTarget::WhiteIdle),
    ));
    e.push(item(
        "Set Black Key Color...",
        MenuAction::PickColor(ColorTarget::BlackIdle),
    ));
    e.push(Entry::Separator);
    e.push(item(
        "Set Active Key Color...",
        MenuAction::PickColor(ColorTarget::Active),
    ));
    e.push(item(
        "Set Sustain Color...",
        MenuAction::PickColor(ColorTarget::Sustain),
    ));
    e.push(Entry::Separator);
    e.push(item(
        if view.dark_mode { "Light Mode" } else { "Dark Mode" },
        MenuAction::ToggleDarkMode,
    ));
    // Only offered when a second typeface is actually installed, matching how
    // Detach appears conditionally rather than showing a dead row.
    if let Some(next) = view.next_font {
        e.push(item(next, MenuAction::CycleFont));
    }
    // Supporter extras. Hidden rather than greyed for the free build: a locked
    // row you cannot use is a nag, and the app is meant to feel complete.
    if view.supporter {
        e.push(item(
            if view.glow_on { "Disable Key Glow" } else { "Enable Key Glow" },
            MenuAction::ToggleKeyGlow,
        ));
        e.push(item(
            if view.segment_on { "Disable Segment Display" } else { "Enable Segment Display" },
            MenuAction::ToggleSegmentDisplay,
        ));
    }
    e.push(Entry::Separator);
    e.push(item(
        if view.keytoggle {
            "Disable Keytoggle"
        } else {
            "Enable Keytoggle"
        },
        MenuAction::ToggleKeytoggle,
    ));
    // Chord-detection block (the detector is always available in the Rust build).
    e.push(Entry::Separator);
    e.push(item(
        if view.prefer_flats {
            "Use Sharps (A#)"
        } else {
            "Use Flats (Bb)"
        },
        MenuAction::ToggleNotePreference,
    ));
    e.push(Entry::Separator);
    if view.detached {
        e.push(item("Attach Chord Window", MenuAction::AttachChordWindow));
    } else {
        e.push(item(
            if view.detection_enabled {
                "Disable Chord Detection"
            } else {
                "Enable Chord Detection"
            },
            MenuAction::ToggleChordDetection,
        ));
        if view.detection_enabled {
            e.push(item("Detach Chord Window", MenuAction::DetachChordWindow));
        }
    }
    // D-UI-5: teach items, inside the detection block, right after the
    // Detach/Attach entry, preceded by their own separator. "Teach Chord
    // Name..." is greyed only when no notes are held; "Manage Taught
    // Chords..." is always available.
    e.push(Entry::Separator);
    e.push(Entry::Item {
        label: "Teach Chord Name...".to_owned(),
        action: MenuAction::TeachChordName,
        enabled: view.notes_held,
    });
    e.push(Entry::Item {
        label: "Manage Taught Chords...".to_owned(),
        action: MenuAction::ManageTaughtChords,
        enabled: true,
    });
    // D-UI-9: the learned re-ranker. "Correct Chord Name..." needs a voicing to
    // act on; the toggle renames itself like every other toggle here (Qt parity
    // — no checkmarks anywhere). Forgetting what was learned lives in "Manage
    // Taught Chords...", one step away from the button that trains.
    e.push(Entry::Separator);
    e.push(Entry::Item {
        label: "Correct Chord Name...".to_owned(),
        action: MenuAction::CorrectChordName,
        // Needs both a voicing AND a visible reading: with detection off,
        // detection_tick() nulls current_chord, so the dialog would show
        // "Now reads: (none)" and the result would land somewhere invisible.
        enabled: view.notes_held && view.detection_enabled,
    });
    e.push(item(
        if view.learning_on {
            "Disable Chord Learning"
        } else {
            "Enable Chord Learning"
        },
        MenuAction::ToggleChordLearning,
    ));
    e.push(Entry::Separator);
    e.push(item("About", MenuAction::ShowAbout));
    e.push(item("Reset Settings to Default", MenuAction::ResetSettings));
    e
}

impl MenuState {
    /// Snapshot labels and measure geometry at open time (Qt menus are static
    /// while shown). `global_pos` is monitor-space points.
    pub fn open(
        ctx: &egui::Context,
        view: MenuView,
        global_pos: Pos2,
        monitor_size: Option<Vec2>,
    ) -> Self {
        let entries = build_entries(view);
        let font = FontId::new(MENU_FONT_SIZE, fonts::courier_bold());

        let measure = |ctx: &egui::Context, text: &str| -> Vec2 {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(text.to_owned(), font.clone(), Color32::WHITE)
                    .size()
            })
        };

        let mut text_h: f32 = 0.0;
        let mut max_w: f32 = 0.0;
        let arrow_w = measure(ctx, ARROW).x;
        for entry in &entries {
            match entry {
                Entry::Separator => {}
                Entry::Item { label, .. } => {
                    let sz = measure(ctx, label);
                    text_h = text_h.max(sz.y);
                    max_w = max_w.max(sz.x);
                }
                Entry::SizeParent => {
                    let sz = measure(ctx, "Size");
                    text_h = text_h.max(sz.y);
                    // text + gap + arrow
                    max_w = max_w.max(sz.x + 12.0 + arrow_w);
                }
            }
        }
        let row_h = (text_h + 2.0 * PAD_Y).ceil();
        let width = (max_w + 2.0 * PAD_X).ceil();

        let mut height = 0.0;
        let mut size_row_top = 0.0;
        for entry in &entries {
            match entry {
                Entry::Separator => height += SEP_H,
                Entry::Item { .. } => height += row_h,
                Entry::SizeParent => {
                    size_row_top = height;
                    height += row_h;
                }
            }
        }

        // Submenu geometry.
        let mut sub_w: f32 = 0.0;
        for pct in SIZE_PERCENTS {
            sub_w = sub_w.max(measure(ctx, &format!("{pct}%")).x);
        }
        let submenu_size = Vec2::new(
            (sub_w + 2.0 * PAD_X).ceil(),
            SIZE_PERCENTS.len() as f32 * row_h,
        );

        // Best-effort clamp to the monitor.
        let mut pos = global_pos;
        if let Some(mon) = monitor_size {
            if pos.x + width > mon.x {
                pos.x = (mon.x - width).max(0.0);
            }
            if pos.y + height > mon.y {
                pos.y = (mon.y - height).max(0.0);
            }
        }

        Self {
            pos,
            size: Vec2::new(width, height),
            entries,
            row_h,
            size_row_top,
            submenu_open: false,
            submenu_size,
            dark_mode: view.dark_mode,
            opened_at: Instant::now(),
            saw_focus: false,
        }
    }
}

fn apply_menu_style(style: &mut egui::Style, c: MenuColors) {
    style.spacing.button_padding = egui::vec2(PAD_X, PAD_Y);
    style.spacing.item_spacing = egui::vec2(0.0, 0.0);
    style.spacing.menu_margin = Margin::ZERO;
    style.spacing.window_margin = Margin::ZERO;
    style.spacing.interact_size = egui::vec2(0.0, 0.0);

    let set = |wv: &mut egui::style::WidgetVisuals, bg: Color32, fg: Color32| {
        wv.bg_fill = bg;
        wv.weak_bg_fill = bg;
        wv.bg_stroke = Stroke::NONE;
        wv.fg_stroke = Stroke::new(1.0, fg);
        wv.corner_radius = CornerRadius::ZERO;
        wv.expansion = 0.0;
    };
    set(
        &mut style.visuals.widgets.inactive,
        Color32::TRANSPARENT,
        c.text,
    );
    set(&mut style.visuals.widgets.hovered, c.sel, c.text);
    set(&mut style.visuals.widgets.active, c.sel, c.text);
    set(&mut style.visuals.widgets.open, c.sel, c.text);
    set(
        &mut style.visuals.widgets.noninteractive,
        Color32::TRANSPARENT,
        c.text.gamma_multiply(0.4),
    );
    // Separators draw with noninteractive.bg_stroke.
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, c.sep);
    style.visuals.selection.bg_fill = c.sel;
    style.visuals.selection.stroke = Stroke::new(1.0, c.text);
    style.visuals.window_fill = c.bg;
    style.visuals.window_stroke = Stroke::new(1.0, c.bg);
    style.visuals.popup_shadow = egui::Shadow::NONE;
    style.visuals.window_shadow = egui::Shadow::NONE;
    style.visuals.menu_corner_radius = CornerRadius::ZERO;
    style.visuals.override_text_color = None;
}

fn menu_button(ui: &mut egui::Ui, label: &str, enabled: bool, row_h: f32) -> egui::Response {
    ui.add_enabled(
        enabled,
        Button::new(label)
            .wrap_mode(egui::TextWrapMode::Extend)
            .min_size(egui::vec2(0.0, row_h)),
    )
}

/// Render the open menu (and submenu). Returns a chosen action, if any;
/// `state_opt` is set to None when the menu closes for any reason.
pub fn show(ctx: &egui::Context, state_opt: &mut Option<MenuState>) -> Option<MenuAction> {
    let state = state_opt.as_mut()?;
    let c = colors(state.dark_mode);
    let mut action: Option<MenuAction> = None;
    let mut close = false;

    let mut menu_focused = None;
    let mut submenu_focused = None;

    // ── Main menu viewport ─────────────────────────────────────────────────
    let builder = ViewportBuilder::default()
        .with_title("Ivory")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_position(state.pos)
        .with_inner_size(state.size)
        .with_min_inner_size(state.size)
        .with_max_inner_size(state.size);

    let mut hover_close_submenu = false;
    let mut hover_open_submenu = false;
    ctx.show_viewport_immediate(menu_vp_id(), builder, |ui, _class| {
        apply_menu_style(ui.style_mut(), c);
        let rect = ui.max_rect();
        // Background + 1px border in the background color (visually borderless).
        ui.painter().rect_filled(rect, 0.0, c.bg);
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            0.0,
            Stroke::new(1.0, c.bg),
            egui::StrokeKind::Middle,
        );

        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
            for entry in &state.entries {
                match entry {
                    Entry::Separator => {
                        ui.add(egui::Separator::default().spacing(SEP_H));
                    }
                    Entry::Item {
                        label,
                        action: a,
                        enabled,
                    } => {
                        let r = menu_button(ui, label, *enabled, state.row_h);
                        if r.hovered() {
                            hover_close_submenu = true;
                        }
                        if r.clicked() {
                            action = Some(*a);
                            close = true;
                        }
                    }
                    Entry::SizeParent => {
                        let r = ui.add(
                            Button::new("Size")
                                .right_text(ARROW)
                                .selected(state.submenu_open)
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .min_size(egui::vec2(0.0, state.row_h)),
                        );
                        if r.hovered() || r.clicked() {
                            hover_open_submenu = true;
                        }
                    }
                }
            }
        });

        let (close_req, esc, focused) = ui.input(|i| {
            (
                i.viewport().close_requested(),
                i.key_pressed(egui::Key::Escape),
                i.viewport().focused,
            )
        });
        menu_focused = focused;
        if close_req || esc {
            close = true;
        }
    });

    if hover_open_submenu {
        state.submenu_open = true;
    } else if hover_close_submenu {
        state.submenu_open = false;
    }

    // ── Size submenu viewport (sibling, Qt-style to the right) ─────────────
    if state.submenu_open && !close {
        let sub_pos = Pos2::new(state.pos.x + state.size.x, state.pos.y + state.size_row_top);
        let sub_builder = ViewportBuilder::default()
            .with_title("Ivory")
            .with_decorations(false)
            .with_resizable(false)
            .with_always_on_top()
            .with_active(false) // don't steal key focus from the menu
            .with_position(sub_pos)
            .with_inner_size(state.submenu_size)
            .with_min_inner_size(state.submenu_size)
            .with_max_inner_size(state.submenu_size);

        ctx.show_viewport_immediate(submenu_vp_id(), sub_builder, |ui, _class| {
            apply_menu_style(ui.style_mut(), c);
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, c.bg);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                for pct in SIZE_PERCENTS {
                    if menu_button(ui, &format!("{pct}%"), true, state.row_h).clicked() {
                        action = Some(MenuAction::SetSizePercent(pct));
                        close = true;
                    }
                }
            });
            let (esc, focused) =
                ui.input(|i| (i.key_pressed(egui::Key::Escape), i.viewport().focused));
            submenu_focused = focused;
            if esc {
                close = true;
            }
        });
    }

    // ── Close-on-focus-loss (click elsewhere / other app) ──────────────────
    if menu_focused == Some(true) || submenu_focused == Some(true) {
        state.saw_focus = true;
    }
    let grace = state.opened_at.elapsed() > std::time::Duration::from_millis(250);
    let all_unfocused =
        menu_focused == Some(false) && submenu_focused.is_none_or(|f| !f);
    if state.saw_focus && grace && all_unfocused {
        close = true;
    }

    if close {
        *state_opt = None;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> MenuView {
        MenuView {
            dark_mode: false,
            borderless: false,
            keytoggle: false,
            prefer_flats: true,
            detection_enabled: true,
            detached: false,
            notes_held: false,
            learning_on: false,
            next_font: None,
            supporter: false,
            glow_on: false,
            segment_on: false,
        }
    }

    fn rows(v: MenuView) -> Vec<(String, MenuAction, bool)> {
        build_entries(v)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Item {
                    label,
                    action,
                    enabled,
                } => Some((label, action, enabled)),
                _ => None,
            })
            .collect()
    }

    fn find(v: MenuView, action: MenuAction) -> Option<(String, bool)> {
        rows(v)
            .into_iter()
            .find(|(_, a, _)| *a == action)
            .map(|(l, _, e)| (l, e))
    }

    /// D-UI-9: correcting acts on the held voicing AND needs a visible reading,
    /// so it greys out with no notes down or with detection off.
    #[test]
    fn correct_item_needs_notes_and_detection() {
        let mut v = view();
        let label = "Correct Chord Name...".to_owned();
        assert_eq!(find(v, MenuAction::CorrectChordName), Some((label.clone(), false)));
        v.notes_held = true;
        assert_eq!(find(v, MenuAction::CorrectChordName), Some((label.clone(), true)));
        // Detection off nulls current_chord — nothing to correct against.
        v.detection_enabled = false;
        assert_eq!(find(v, MenuAction::CorrectChordName), Some((label, false)));
        // Teaching still works with detection off: it pins a name outright.
        assert_eq!(
            find(v, MenuAction::TeachChordName).map(|(_, e)| e),
            Some(true)
        );
    }

    /// Qt parity: toggles rename themselves rather than showing a checkmark.
    #[test]
    fn learning_toggle_renames_itself() {
        let mut v = view();
        assert_eq!(
            find(v, MenuAction::ToggleChordLearning),
            Some(("Enable Chord Learning".to_owned(), true))
        );
        v.learning_on = true;
        assert_eq!(
            find(v, MenuAction::ToggleChordLearning),
            Some(("Disable Chord Learning".to_owned(), true))
        );
    }

    /// The learning block sits with the teach block, after it, and the menu
    /// still ends with About / Reset.
    #[test]
    fn learning_block_sits_after_the_teach_block() {
        let r = rows(view());
        let pos = |a: MenuAction| r.iter().position(|(_, x, _)| *x == a).unwrap();
        assert!(pos(MenuAction::TeachChordName) < pos(MenuAction::ManageTaughtChords));
        assert!(pos(MenuAction::ManageTaughtChords) < pos(MenuAction::CorrectChordName));
        assert!(pos(MenuAction::CorrectChordName) < pos(MenuAction::ToggleChordLearning));
        assert!(pos(MenuAction::ToggleChordLearning) < pos(MenuAction::ShowAbout));
        assert_eq!(r.last().map(|(_, a, _)| *a), Some(MenuAction::ResetSettings));
    }
}
