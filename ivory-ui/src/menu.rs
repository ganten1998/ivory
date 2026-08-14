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
use crate::fretboard_panel;
use crate::host::Caps;
use egui::{Button, Color32, CornerRadius, FontId, Margin, Pos2, Stroke, Vec2};
use ivory_core::fretboard;
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorTarget {
    WhiteIdle,
    BlackIdle,
    Active,
    Sustain,
    /// The chord label. Free to change; the bloom around it is the extra.
    ChordText,
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
    DetachChordWindow,
    AttachChordWindow,
    TeachChordName,
    ManageTaughtChords,
    /// D-UI-9: correct the current reading, training the learned re-ranker.
    CorrectChordName,
    /// D-UI-9: master switch for the learned re-ranker (weights are kept).
    ToggleChordLearning,
    /// Open the supporter-key dialog.
    ShowSupporterKey,
    /// Supporter decoration: show/hide the pixel heart.
    ToggleHeart,
    /// D-UI-15: the guitar view.
    ToggleFretboard,
    /// Name from `fretboard::TUNINGS`.
    SetTuning(&'static str),
    SetCapo(u8),
    /// Fingerboard wood key (see `fretboard_panel::Wood`).
    SetWood(&'static str),
    /// D-UI-17: turn one theory diagram on or off. Independent, because any
    /// combination of the three may be showing at once.
    ToggleTheoryView(crate::theory_panel::View),
    /// D-UI-17: whether the theory band follows live playing or stays put.
    ToggleTheoryFollowsMidi,
    /// D-UI-16: pop the guitar view into its own window, and put it back.
    DetachFretboard,
    AttachFretboard,
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
    pub heart_on: bool,
    /// D-UI-15: the guitar view is showing. Its Tuning and Capo submenus are
    /// hidden while it is off, rather than offered and inert.
    pub fretboard_on: bool,
    pub tuning: &'static str,
    pub capo: u8,
    pub wood: &'static str,
    /// D-UI-16: the guitar view is in its own window.
    pub fretboard_detached: bool,
    /// D-UI-17: which theory diagrams are showing.
    pub theory: crate::theory_panel::Views,
    /// D-UI-17: whether the theory band follows live playing.
    pub theory_follows_midi: bool,
    /// What the host allows. Rows whose action needs a window, a device list,
    /// or control of its own size are not shown where they cannot work — an
    /// inert row is worse than an absent one, because the user cannot tell
    /// whether they mis-clicked or the app is broken.
    pub caps: Caps,
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
/// Highest capo offered. Past this it stops being a capo and starts being a
/// different instrument, and the list has to end somewhere.
const CAPO_MAX: u8 = 9;
const ARROW: &str = "\u{23F5}"; // ⏵ submenu indicator

enum Entry {
    Separator,
    Item {
        label: String,
        action: MenuAction,
        enabled: bool,
    },
    /// A parent row that opens a sibling viewport to its right. There used to
    /// be exactly one of these (Size), hard-coded from the entry list all the
    /// way down to the viewport; the guitar view needs two more, so the whole
    /// path is now driven by the list.
    Submenu {
        label: String,
        items: Vec<(String, MenuAction)>,
    },
}

/// Where an open submenu goes and how big it is. Measured at open time with
/// the rest of the menu, because a Qt menu is static once it is showing.
struct SubGeom {
    row_top: f32,
    size: Vec2,
}

pub struct MenuState {
    pos: Pos2, // global (monitor points), top-left
    size: Vec2,
    entries: Vec<Entry>,
    row_h: f32, // uniform item height; buttons are forced to it so the
    // stacked rows exactly fill the computed viewport size
    /// Geometry per submenu, in entry order.
    subs: Vec<SubGeom>,
    /// Which submenu is showing, as an index into `subs`. At most one, which
    /// is why they can all share a single viewport id.
    submenu_open: Option<usize>,
    /// Kept so a submenu can be clamped too, not just the menu. Tuning and
    /// Capo sit near the BOTTOM of a long menu and Capo is ten rows deep, so
    /// unclamped they run off the screen and their lower rows cannot be
    /// clicked. Size never hit this: it is the first row and seven rows tall.
    monitor: Option<Vec2>,
    dark_mode: bool,
    opened_at: Instant,
    saw_focus: bool,
    /// Captured at open time, not read from the app each frame, so a menu can
    /// never be half-drawn as a window and half as a layer.
    caps: Caps,
}

/// Stable surface identities. On the desktop these are viewport ids; in a
/// plugin they are `Area` ids. One string each, so the two paths cannot drift.
const MENU_ID: &str = "ivory-menu";
const SUBMENU_ID: &str = "ivory-menu-sub";

fn build_entries(view: MenuView) -> Vec<Entry> {
    let item = |label: &str, action: MenuAction| Entry::Item {
        label: label.to_owned(),
        action,
        enabled: true,
    };
    let submenu = |label: &str, items: Vec<(String, MenuAction)>| Entry::Submenu {
        label: label.to_owned(),
        items,
    };
    let mut e = Vec::new();
    // 1. Size submenu, and the borderless toggle: both are the app deciding
    //    its own geometry, which a plugin editor does not get to do.
    if view.caps.window_sizing {
        e.push(submenu(
            "Size",
            SIZE_PERCENTS
                .iter()
                .map(|&p| (format!("{p}%"), MenuAction::SetSizePercent(p)))
                .collect(),
        ));
        e.push(Entry::Separator);
        // 3. Borderless toggle (label shows what you would switch TO the
        //    current state from: "Borderless" while bordered, "Bordered"
        //    while borderless)
        e.push(item(
            if view.borderless {
                "Bordered"
            } else {
                "Borderless"
            },
            MenuAction::ToggleBorderless,
        ));
        e.push(Entry::Separator);
    }
    // A plugin is handed its notes by the host and has no device to choose.
    if view.caps.midi_ports {
        e.push(item("Select MIDI Input...", MenuAction::SelectMidiInput));
        e.push(Entry::Separator);
    }
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
    e.push(item(
        "Set Chord Color...",
        MenuAction::PickColor(ColorTarget::ChordText),
    ));
    e.push(Entry::Separator);
    e.push(item(
        if view.dark_mode {
            "Light Mode"
        } else {
            "Dark Mode"
        },
        MenuAction::ToggleDarkMode,
    ));
    // Only offered when a second typeface is actually installed, matching how
    // Detach appears conditionally rather than showing a dead row.
    if let Some(next) = view.next_font {
        e.push(item(next, MenuAction::CycleFont));
    }
    e.push(Entry::Separator);
    e.push(item(
        if view.supporter {
            "Supporter Key..."
        } else {
            "Support Tangent..."
        },
        MenuAction::ShowSupporterKey,
    ));
    if view.supporter {
        e.push(item(
            if view.heart_on {
                "Hide Heart"
            } else {
                "Show Heart"
            },
            MenuAction::ToggleHeart,
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
    if view.detached && view.caps.detachable {
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
        if view.detection_enabled && view.caps.detachable {
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
    // D-UI-17: the theory band. A submenu rather than three top-level rows,
    // because the menu is already long and these three belong together — and
    // each row renames itself the way every other toggle here does, so the
    // submenu says what is showing without a checkmark column.
    e.push(Entry::Separator);
    e.push(submenu(
        "Theory",
        crate::theory_panel::View::ALL
            .iter()
            .map(|v| {
                (
                    if v.is_on(view.theory) {
                        format!("Hide {}", v.label())
                    } else {
                        format!("Show {}", v.label())
                    },
                    MenuAction::ToggleTheoryView(*v),
                )
            })
            // Whether the band tracks your playing sits with the diagrams
            // rather than in the keytoggle block, because it is a property of
            // this display and of nothing else.
            .chain(std::iter::once((
                if view.theory_follows_midi {
                    "Stop Following MIDI".to_owned()
                } else {
                    "Follow MIDI".to_owned()
                },
                MenuAction::ToggleTheoryFollowsMidi,
            )))
            .collect(),
    ));
    // D-UI-15: the guitar view. Its own block, because it is a second
    // instrument rather than another chord-display option, and its two
    // submenus only exist while it is on: a Tuning row on a hidden fretboard
    // is a control for something the user cannot see.
    e.push(Entry::Separator);
    e.push(item(
        if view.fretboard_on {
            "Hide Fretboard"
        } else {
            "Show Fretboard"
        },
        MenuAction::ToggleFretboard,
    ));
    if view.fretboard_on {
        // Mirrors the chord window's Detach/Attach exactly, so there is one
        // set of habits rather than two.
        if view.caps.detachable {
            e.push(item(
                if view.fretboard_detached {
                    "Attach Fretboard"
                } else {
                    "Detach Fretboard"
                },
                if view.fretboard_detached {
                    MenuAction::AttachFretboard
                } else {
                    MenuAction::DetachFretboard
                },
            ));
        }
        e.push(submenu(
            "Wood",
            fretboard_panel::Wood::ALL
                .iter()
                .map(|w| {
                    (
                        if w.key() == view.wood {
                            format!("{}  \u{2022}", w.label())
                        } else {
                            w.label().to_owned()
                        },
                        MenuAction::SetWood(w.key()),
                    )
                })
                .collect(),
        ));
        e.push(submenu(
            "Tuning",
            fretboard::TUNINGS
                .iter()
                .map(|t| {
                    (
                        // The current one is marked rather than hidden: a
                        // submenu that never says what is selected makes you
                        // close it again to find out.
                        if t.name == view.tuning {
                            format!("{}  \u{2022}", t.name)
                        } else {
                            t.name.to_owned()
                        },
                        MenuAction::SetTuning(t.name),
                    )
                })
                .collect(),
        ));
        e.push(submenu(
            "Capo",
            (0..=CAPO_MAX)
                .map(|f| {
                    let label = if f == 0 {
                        "No Capo".to_owned()
                    } else {
                        format!("Fret {f}")
                    };
                    (
                        if f == view.capo {
                            format!("{label}  \u{2022}")
                        } else {
                            label
                        },
                        MenuAction::SetCapo(f),
                    )
                })
                .collect(),
        ));
    }
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
                Entry::Submenu { label, .. } => {
                    let sz = measure(ctx, label);
                    text_h = text_h.max(sz.y);
                    // text + gap + arrow
                    max_w = max_w.max(sz.x + 12.0 + arrow_w);
                }
            }
        }
        let row_h = (text_h + 2.0 * PAD_Y).ceil();
        let width = (max_w + 2.0 * PAD_X).ceil();

        let mut height = 0.0;
        let mut subs: Vec<SubGeom> = Vec::new();
        for entry in &entries {
            match entry {
                Entry::Separator => height += SEP_H,
                Entry::Item { .. } => height += row_h,
                Entry::Submenu { items, .. } => {
                    let w = items
                        .iter()
                        .fold(0.0_f32, |acc, (l, _)| acc.max(measure(ctx, l).x));
                    subs.push(SubGeom {
                        row_top: height,
                        size: Vec2::new((w + 2.0 * PAD_X).ceil(), items.len() as f32 * row_h),
                    });
                    height += row_h;
                }
            }
        }

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
            subs,
            submenu_open: None,
            monitor: monitor_size,
            dark_mode: view.dark_mode,
            opened_at: Instant::now(),
            saw_focus: false,
            caps: view.caps,
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
        wv.fg_stroke = Stroke::new(1.0_f32, fg);
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
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, c.sep);
    style.visuals.selection.bg_fill = c.sel;
    style.visuals.selection.stroke = Stroke::new(1.0_f32, c.text);
    style.visuals.window_fill = c.bg;
    style.visuals.window_stroke = Stroke::new(1.0_f32, c.bg);
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
    let caps = state.caps;
    let row_h = state.row_h;
    let submenu_open = state.submenu_open;
    let mut action: Option<MenuAction> = None;
    let mut close = false;

    let mut hover_close_submenu = false;
    let mut hover_open_submenu: Option<usize> = None;

    // ── Main menu ──────────────────────────────────────────────────────────
    let menu_spec = crate::shell::SurfaceSpec {
        id: MENU_ID,
        size: state.size,
        min_size: state.size,
        pos: Some(state.pos),
        order: egui::Order::Foreground,
        ..Default::default()
    };
    let menu_report = crate::shell::surface(ctx, caps, &menu_spec, &mut |ui, want_close| {
        apply_menu_style(ui.style_mut(), c);
        let rect = ui.max_rect();
        // Background + 1px border in the background color (visually borderless).
        ui.painter().rect_filled(rect, 0.0, c.bg);
        ui.painter().rect_stroke(
            rect.shrink(0.5),
            0.0,
            Stroke::new(1.0_f32, c.bg),
            egui::StrokeKind::Middle,
        );

        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
            let mut sub_idx = 0usize;
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
                        let r = menu_button(ui, label, *enabled, row_h);
                        if r.hovered() {
                            hover_close_submenu = true;
                        }
                        if r.clicked() {
                            action = Some(*a);
                            *want_close = true;
                        }
                    }
                    Entry::Submenu { label, .. } => {
                        let r = ui.add(
                            Button::new(label.as_str())
                                .right_text(ARROW)
                                .selected(submenu_open == Some(sub_idx))
                                .wrap_mode(egui::TextWrapMode::Extend)
                                .min_size(egui::vec2(0.0, row_h)),
                        );
                        if r.hovered() || r.clicked() {
                            hover_open_submenu = Some(sub_idx);
                        }
                        sub_idx += 1;
                    }
                }
            }
        });
    });
    close |= menu_report.close;

    if let Some(i) = hover_open_submenu {
        state.submenu_open = Some(i);
    } else if hover_close_submenu {
        state.submenu_open = None;
    }

    // ── Submenu (sibling, Qt-style to the right) ──────────────────────────
    // Only one submenu can be open at a time, so they all share one surface id
    // and it simply moves and resizes as the pointer travels down the menu.
    let open_sub = state
        .submenu_open
        .filter(|_| !close)
        .and_then(|i| state.subs.get(i).map(|g| (i, g.row_top, g.size)));
    let mut submenu_report = None;
    if let Some((sub_i, row_top, sub_size)) = open_sub {
        let mut sub_pos = Pos2::new(state.pos.x + state.size.x, state.pos.y + row_top);
        if let Some(mon) = state.monitor {
            // Slide up rather than off the bottom, and flip to the menu's LEFT
            // rather than off the right edge, which is what a native menu does.
            if sub_pos.y + sub_size.y > mon.y {
                sub_pos.y = (mon.y - sub_size.y).max(0.0);
            }
            if sub_pos.x + sub_size.x > mon.x {
                sub_pos.x = (state.pos.x - sub_size.x).max(0.0);
            }
        }

        let sub_spec = crate::shell::SurfaceSpec {
            id: SUBMENU_ID,
            size: sub_size,
            min_size: sub_size,
            pos: Some(sub_pos),
            // Don't steal key focus from the menu — and inline, sit above it.
            takes_focus: false,
            order: egui::Order::Tooltip,
            ..Default::default()
        };
        let report = crate::shell::surface(ctx, caps, &sub_spec, &mut |ui, want_close| {
            apply_menu_style(ui.style_mut(), c);
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, c.bg);
            let items = state
                .entries
                .iter()
                .filter_map(|e| match e {
                    Entry::Submenu { items, .. } => Some(items),
                    _ => None,
                })
                .nth(sub_i);
            ui.with_layout(egui::Layout::top_down_justified(egui::Align::Min), |ui| {
                for (label, a) in items.into_iter().flatten() {
                    if menu_button(ui, label, true, row_h).clicked() {
                        action = Some(*a);
                        *want_close = true;
                    }
                }
            });
        });
        close |= report.close;
        submenu_report = Some(report);
    }

    // ── Closing when the user goes elsewhere ───────────────────────────────
    //
    // Two different signals for the same intent, because the two hosts offer
    // different evidence. A window knows it lost focus; a layer in someone
    // else's window has no focus to lose and has to watch the pointer instead.
    if caps.child_windows {
        if menu_report.focused == Some(true)
            || submenu_report.is_some_and(|r| r.focused == Some(true))
        {
            state.saw_focus = true;
        }
        let grace = state.opened_at.elapsed() > std::time::Duration::from_millis(250);
        let all_unfocused = menu_report.focused == Some(false)
            && submenu_report.is_none_or(|r| r.focused != Some(true));
        if state.saw_focus && grace && all_unfocused {
            close = true;
        }
    } else if menu_report.pressed_outside && submenu_report.is_none_or(|r| r.pressed_outside) {
        // The press that OPENED the menu is always inside it — the menu is
        // positioned at the cursor — so this needs no opening grace.
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
            heart_on: true,
            fretboard_on: false,
            tuning: "Standard",
            capo: 0,
            wood: "rosewood",
            fretboard_detached: false,
            theory: crate::theory_panel::Views::default(),
            theory_follows_midi: false,
            caps: Caps::DESKTOP,
        }
    }

    /// Submenu rows, in menu order: (parent label, item labels, item actions).
    fn submenus(v: MenuView) -> Vec<(String, Vec<String>, Vec<MenuAction>)> {
        build_entries(v)
            .into_iter()
            .filter_map(|e| match e {
                Entry::Submenu { label, items } => Some((
                    label,
                    items.iter().map(|(l, _)| l.clone()).collect(),
                    items.into_iter().map(|(_, a)| a).collect(),
                )),
                _ => None,
            })
            .collect()
    }

    /// One submenu by name. Positional indexing broke every time a submenu
    /// was added above another, which is a test failing for the wrong reason.
    fn sub(v: MenuView, name: &str) -> (String, Vec<String>, Vec<MenuAction>) {
        submenus(v)
            .into_iter()
            .find(|(n, ..)| n == name)
            .unwrap_or_else(|| panic!("no {name} submenu"))
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
        assert_eq!(
            find(v, MenuAction::CorrectChordName),
            Some((label.clone(), false))
        );
        v.notes_held = true;
        assert_eq!(
            find(v, MenuAction::CorrectChordName),
            Some((label.clone(), true))
        );
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

    /// The Size submenu is the one that existed before submenus were plural.
    /// It must come out of the generalised path byte-for-byte the same.
    #[test]
    fn size_is_still_the_first_submenu_and_still_lists_the_same_percents() {
        let subs = submenus(view());
        assert_eq!(subs[0].0, "Size");
        assert_eq!(
            subs[0].1,
            vec!["50%", "75%", "100%", "125%", "150%", "175%", "200%"]
        );
        assert_eq!(subs[0].2[2], MenuAction::SetSizePercent(100));
    }

    /// D-UI-15: the guitar view renames itself like every other toggle here,
    /// and its two submenus exist only while it is on. A Tuning row on a
    /// hidden fretboard is a control for something you cannot see.
    #[test]
    fn the_fretboard_toggle_brings_its_submenus_with_it() {
        let mut v = view();
        assert_eq!(
            find(v, MenuAction::ToggleFretboard),
            Some(("Show Fretboard".to_owned(), true))
        );
        assert_eq!(
            submenus(v)
                .iter()
                .map(|(n, ..)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["Size", "Theory"],
            "only Size and Theory while the fretboard is off"
        );

        v.fretboard_on = true;
        assert_eq!(
            find(v, MenuAction::ToggleFretboard),
            Some(("Hide Fretboard".to_owned(), true))
        );
        let subs = submenus(v);
        // Asserted as the whole list, in order: an inserted submenu that
        // silently shifts Wood/Tuning/Capo is exactly what this catches.
        assert_eq!(
            subs.iter().map(|(n, ..)| n.as_str()).collect::<Vec<_>>(),
            vec!["Size", "Theory", "Wood", "Tuning", "Capo"]
        );
        let wood = sub(v, "Wood");
        assert_eq!(wood.1.len(), 3, "three woods");
        assert!(
            wood.1[0].starts_with("Rosewood"),
            "rosewood is the default and comes first"
        );
        assert!(wood.1[0].ends_with('\u{2022}'));
        assert_eq!(wood.2[0], MenuAction::SetWood("rosewood"));
        // Detach mirrors the chord window's toggle, renaming itself.
        assert_eq!(
            find(v, MenuAction::DetachFretboard).map(|(l, _)| l),
            Some("Detach Fretboard".to_owned())
        );
        let d = MenuView {
            fretboard_detached: true,
            ..v
        };
        assert_eq!(
            find(d, MenuAction::AttachFretboard).map(|(l, _)| l),
            Some("Attach Fretboard".to_owned())
        );
        // Every shipped tuning is offered, and the live one is marked rather
        // than hidden: a submenu that never says what is selected makes you
        // close it again to find out.
        let tuning = sub(v, "Tuning");
        assert_eq!(tuning.1.len(), fretboard::TUNINGS.len());
        assert!(tuning.1[0].starts_with("Standard"));
        assert!(
            tuning.1[0].ends_with('\u{2022}'),
            "the current tuning is marked"
        );
        assert!(!tuning.1[1].ends_with('\u{2022}'));
        assert_eq!(tuning.2[0], MenuAction::SetTuning("Standard"));
        assert!(
            tuning.2.iter().all(|a| matches!(a, MenuAction::SetTuning(n)
                if fretboard::Tuning::by_name(n).is_some())),
            "every offered tuning must resolve"
        );

        let capo = sub(v, "Capo");
        assert_eq!(capo.1[0], "No Capo  \u{2022}");
        assert_eq!(capo.1[1], "Fret 1");
        assert_eq!(capo.2[0], MenuAction::SetCapo(0));
        assert_eq!(capo.2.len() as u8, CAPO_MAX + 1);
    }

    #[test]
    fn the_marked_row_follows_the_settings() {
        let v = MenuView {
            fretboard_on: true,
            tuning: "DADGAD",
            capo: 3,
            ..view()
        };
        let tuning = sub(v, "Tuning");
        let marked: Vec<&String> = tuning
            .1
            .iter()
            .filter(|l| l.ends_with('\u{2022}'))
            .collect();
        assert_eq!(marked.len(), 1);
        assert!(marked[0].starts_with("DADGAD"));
        assert_eq!(sub(v, "Capo").1[3], "Fret 3  \u{2022}");
    }

    /// A submenu low in a long menu must slide up rather than run off the
    /// bottom of the screen. Size never needed this — it is the first row and
    /// seven rows tall — but Capo is ten rows and sits near the end.
    #[test]
    fn a_submenu_near_the_bottom_is_pulled_back_onto_the_screen() {
        // The clamp, in the same form `show` applies it.
        let clamp = |pos: Pos2, size: Vec2, menu_x: f32, mon: Vec2| {
            let mut p = pos;
            if p.y + size.y > mon.y {
                p.y = (mon.y - size.y).max(0.0);
            }
            if p.x + size.x > mon.x {
                p.x = (menu_x - size.x).max(0.0);
            }
            p
        };
        let mon = Vec2::new(1440.0, 900.0);
        let size = Vec2::new(120.0, 260.0); // ten rows of Capo
                                            // Opened near the bottom: pulled up so the last row is reachable.
        let p = clamp(Pos2::new(1000.0, 820.0), size, 900.0, mon);
        assert!(p.y + size.y <= mon.y, "bottom row is off-screen at {p:?}");
        // Opened near the right edge: flipped to the menu's other side.
        let p = clamp(Pos2::new(1380.0, 100.0), size, 1260.0, mon);
        assert!(p.x + size.x <= mon.x, "right edge is off-screen at {p:?}");
        assert!(p.x < 1380.0, "it should flip left, not just shrink back");
        // Comfortably inside: untouched.
        let inside = Pos2::new(300.0, 200.0);
        assert_eq!(clamp(inside, size, 200.0, mon), inside);
    }

    /// The desktop menu must be BYTE-IDENTICAL under `Caps::DESKTOP`. This
    /// refactor is only safe if the shipping app cannot tell it happened.
    #[test]
    fn desktop_caps_change_nothing() {
        let mut v = view();
        v.fretboard_on = true;
        v.detection_enabled = true;
        let with = rows(v);
        // The same view, built the way it was before Caps existed, is what
        // `Caps::DESKTOP` has to reproduce: every row present.
        assert!(with
            .iter()
            .any(|(_, a, _)| *a == MenuAction::ToggleBorderless));
        assert!(with
            .iter()
            .any(|(_, a, _)| *a == MenuAction::SelectMidiInput));
        assert!(with
            .iter()
            .any(|(_, a, _)| *a == MenuAction::DetachChordWindow));
        assert!(with
            .iter()
            .any(|(_, a, _)| *a == MenuAction::DetachFretboard));
        assert_eq!(submenus(v)[0].0, "Size");
        assert_eq!(
            with.last().map(|(_, a, _)| *a),
            Some(MenuAction::ResetSettings)
        );
    }

    /// In a plugin, every row that survives must be one the host can actually
    /// honour. An inert row is worse than an absent one: the user cannot tell
    /// whether they mis-clicked or the app is broken.
    #[test]
    fn no_surviving_plugin_row_needs_a_window_or_a_device() {
        let v = MenuView {
            caps: Caps::PLUGIN,
            fretboard_on: true,
            detection_enabled: true,
            detached: true,
            fretboard_detached: true,
            ..view()
        };
        let forbidden = [
            MenuAction::SelectMidiInput,
            MenuAction::ToggleBorderless,
            MenuAction::DetachChordWindow,
            MenuAction::AttachChordWindow,
            MenuAction::DetachFretboard,
            MenuAction::AttachFretboard,
        ];
        for (label, action, _) in rows(v) {
            assert!(
                !forbidden.contains(&action),
                "{label} needs something a plugin editor does not have"
            );
        }
        // Size is the app choosing its own geometry, which the host owns.
        assert!(
            !submenus(v).iter().any(|(name, ..)| name == "Size"),
            "Size must not be offered where the host decides the size"
        );
        // And what SHOULD survive still does: the whole point is a plugin that
        // can still teach a chord, change tuning and pick a colour.
        let kept = rows(v);
        for want in [
            MenuAction::ToggleDarkMode,
            MenuAction::ToggleKeytoggle,
            MenuAction::TeachChordName,
            MenuAction::ManageTaughtChords,
            MenuAction::ToggleFretboard,
            MenuAction::ShowAbout,
        ] {
            assert!(
                kept.iter().any(|(_, a, _)| *a == want),
                "{want:?} went missing"
            );
        }
        // Theory, Wood, Tuning and Capo are pure state and must all remain:
        // none of them needs a window, a device or a size of its own.
        assert_eq!(
            submenus(v)
                .iter()
                .map(|(n, ..)| n.as_str())
                .collect::<Vec<_>>(),
            vec!["Theory", "Wood", "Tuning", "Capo"]
        );
    }

    /// Each theory row renames itself the way every other toggle in this menu
    /// does, and all three are independent — the request was explicitly to be
    /// able to show more than one at once, so turning one on must not turn
    /// another off.
    #[test]
    fn the_theory_rows_rename_themselves_and_stay_independent() {
        use crate::theory_panel::{View, Views};
        let mut v = view();
        assert_eq!(
            sub(v, "Theory").1,
            vec![
                "Show Circle of Fifths",
                "Show Tonnetz",
                "Show Harmonic Triangles",
                "Follow MIDI",
            ]
        );
        let mut want: Vec<MenuAction> = View::ALL
            .iter()
            .map(|x| MenuAction::ToggleTheoryView(*x))
            .collect();
        want.push(MenuAction::ToggleTheoryFollowsMidi);
        assert_eq!(sub(v, "Theory").2, want);

        // The follow row renames itself like every other toggle here, and it
        // is OFF by default: the band is something to look at while playing,
        // and one that redrew on every note could not be read while playing.
        assert!(!view().theory_follows_midi);
        let following = MenuView {
            theory_follows_midi: true,
            ..view()
        };
        assert_eq!(
            sub(following, "Theory").1.last().map(String::as_str),
            Some("Stop Following MIDI")
        );

        v.theory = Views {
            circle: true,
            tonnetz: false,
            triangles: true,
        };
        assert_eq!(
            sub(v, "Theory").1[..3],
            [
                "Hide Circle of Fifths",
                "Show Tonnetz",
                "Hide Harmonic Triangles"
            ],
            "the rows do not each follow their own flag"
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
        assert_eq!(
            r.last().map(|(_, a, _)| *a),
            Some(MenuAction::ResetSettings)
        );
    }
}
