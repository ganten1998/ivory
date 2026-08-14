//! Keyboard shortcuts, and the card that lists them.
//!
//! Two things make this more than a match on `Key`.
//!
//! The bindings live in ONE table, and the help card is rendered FROM that
//! table. A shortcut that works but is not listed, or is listed but does not
//! work, is worse than no shortcut at all, and the only way to guarantee
//! neither happens is to have one source. Adding a binding is one line and the
//! card updates itself.
//!
//! And the card is drawn IN THE CANVAS rather than in a child window. Every
//! other surface in this app is an OS viewport, which a VST3 editor cannot
//! create — so this is the first piece of UI that already works in both hosts,
//! and the shape the rest will move to (docs/PLUGIN-PLAN.md).

use egui::{Align2, Color32, FontId, Key, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

/// What a keypress asks the app to do. Deliberately not the same enum as
/// `MenuAction`: these are verbs a key can mean, and several have no menu row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    ToggleHelp,
    ToggleKeytoggle,
    ClearNotes,
    ToggleFretboard,
    CycleTheory,
    ToggleDarkMode,
    ToggleDetection,
    ToggleBorderless,
    CycleFont,
    ShowAbout,
    ShowSupporterKey,
    CloseHelp,
}

/// (key, label, what it does, whether it appears on the card).
/// The card is generated from this, so the two cannot disagree.
/// `H` as well as `F1`, because macOS takes F1 for screen brightness unless
/// "use F1, F2 etc. as standard function keys" is on, and it is off by default.
/// An app never sees the keypress at all, so a help key that only answers to F1
/// is a help key most Mac users cannot press.
const BINDINGS: &[(Key, &str, KeyAction, bool)] = &[
    (Key::H, "H", KeyAction::ToggleHelp, true),
    (Key::F1, "F1", KeyAction::ToggleHelp, false),
    (Key::K, "K", KeyAction::ToggleKeytoggle, true),
    (Key::R, "R", KeyAction::ClearNotes, true),
    (Key::G, "G", KeyAction::ToggleFretboard, true),
    (Key::T, "T", KeyAction::CycleTheory, true),
    (Key::C, "C", KeyAction::ToggleDetection, true),
    (Key::D, "D", KeyAction::ToggleDarkMode, true),
    (Key::B, "B", KeyAction::ToggleBorderless, true),
    (Key::F, "F", KeyAction::CycleFont, true),
    (Key::A, "A", KeyAction::ShowAbout, true),
    (Key::S, "S", KeyAction::ShowSupporterKey, true),
    // Escape only closes the card; it is not worth a row of its own.
    (Key::Escape, "Esc", KeyAction::CloseHelp, false),
];

/// One line of help text per binding, written where the binding is defined so
/// they cannot drift apart.
fn describe(a: KeyAction) -> &'static str {
    match a {
        KeyAction::ToggleHelp => "hold for this card",
        KeyAction::ToggleKeytoggle => "keytoggle: click the piano or the neck to place notes",
        KeyAction::ClearNotes => "clear every note you placed",
        KeyAction::ToggleFretboard => "guitar view",
        KeyAction::CycleTheory => "cycle the theory band",
        KeyAction::ToggleDarkMode => "dark mode",
        KeyAction::ToggleDetection => "chord detection",
        KeyAction::ToggleBorderless => "window border",
        KeyAction::CycleFont => "cycle the typeface",
        KeyAction::ShowAbout => "about",
        KeyAction::ShowSupporterKey => "enter a supporter key",
        KeyAction::CloseHelp => "close this card",
    }
}

/// Which action a frame's keypresses ask for, if any.
///
/// Returns at most one, because two shortcuts firing on one frame is never
/// what anybody meant. Modifiers are REQUIRED to be absent: Cmd-R and Ctrl-R
/// belong to the OS and the browser habit, and swallowing them would be rude.
pub fn pressed(ctx: &egui::Context) -> Option<KeyAction> {
    ctx.input(|i| {
        if i.modifiers.any() {
            return None;
        }
        BINDINGS
            .iter()
            .filter(|(_, _, a, _)| !matches!(a, KeyAction::ToggleHelp | KeyAction::CloseHelp))
            .find(|(key, ..)| i.key_pressed(*key))
            .map(|&(_, _, action, _)| action)
    })
}

/// How far the card is out, 0.0 (hidden) to 1.0 (fully down).
///
/// The help key is HELD rather than toggled: press and read, let go and it is
/// gone, with no state to get stuck in and nothing to dismiss. That only works
/// if it moves, though — a card that blinks in and out looks like a glitch,
/// while one that slides looks like it was always there waiting.
///
/// `animate_bool_with_time` does the easing and asks for the repaints, but the
/// app's own cadence is 50ms (20fps), which would make this visibly steppy, so
/// an animation in progress asks for the next frame immediately.
/// How far the shortcut card is down, 0 to 1.
///
/// `allowed` is not a nicety. The key is read from `InputState::keys_down`,
/// which is built before any widget runs, so keyboard focus does not filter
/// it — typing an `h` into the supporter-key field slid the card down over
/// the app while the letter also went into the box. The desktop escaped that
/// only because a dialog there is a separate OS window and the unfocused root
/// gets `WindowFocused(false)`, which clears `keys_down`.
///
/// Still animates when not allowed, so a card that is already down slides back
/// up rather than vanishing.
pub fn help_progress(ctx: &egui::Context, allowed: bool) -> f32 {
    let held = allowed
        && ctx.input(|i| {
            !i.modifiers.any()
                && BINDINGS
                    .iter()
                    .filter(|(_, _, a, _)| *a == KeyAction::ToggleHelp)
                    .any(|(key, ..)| i.key_down(*key))
        });
    let t = ctx.animate_bool_with_time(egui::Id::new("tangent-help-card"), held, 0.16);
    if t > 0.0 && t < 1.0 {
        ctx.request_repaint();
    }
    t
}

/// Where the card goes, how big its text is, and how many columns it uses.
///
/// Split out so it can be TESTED, because the bug here is invisible in code and
/// glaring on screen: sized from the window height alone, the card grew taller
/// than the app the moment the list did, and the first and last shortcuts were
/// cut off by the window edges.
///
/// Columns matter as much as type size. This window is very WIDE and very SHORT
/// (1300x332 usually, 650x166 at the smallest with everything on), so ten
/// shortcuts in one column do not fit at any legible size while two columns fit
/// comfortably. Using the shape of the window beats shrinking the type until
/// nobody can read it.
fn layout(rect: Rect, rows: usize, widest: usize) -> (Rect, f32, usize) {
    let rows = rows.max(1);
    for cols in 1..=3usize {
        let per_col = rows.div_ceil(cols);
        let by_height = rect.height() * 0.92 / (per_col as f32 * 1.75 + 3.36);
        let size = by_height.min(rect.height() * 0.075).clamp(8.0, 20.0);

        let row_h = size * 1.75;
        let pad = size * 1.4;
        let col_w = size * 3.2 + widest as f32 * size * 0.62;
        let w = col_w * cols as f32 + pad * 2.0 + pad * (cols - 1) as f32;
        let h = per_col as f32 * row_h + pad * 2.4;

        if (w <= rect.width() * 0.94 && h <= rect.height()) || cols == 3 {
            // Never flush against the window edges: a card touching them reads
            // as clipped even when it is not, and exact-equality containment is
            // a float coin toss.
            let card = Vec2::new(w.min(rect.width() * 0.94), h.min(rect.height() * 0.96));
            return (
                Rect::from_min_size(rect.center() - card * 0.5, card),
                size,
                cols,
            );
        }
    }
    unreachable!("the loop always returns on its last iteration")
}

/// The help card, drawn over the app.
///
/// Centred, sized to its own content, and painted directly rather than through
/// a `Window` so it cannot be dragged off, cannot be resized into nothing, and
/// looks identical in a plugin editor where there is no window manager at all.
pub fn draw_help(painter: &Painter, rect: Rect, dark: bool, t: f32) {
    let rows: Vec<(&str, &str)> = BINDINGS
        .iter()
        .filter(|(.., shown)| *shown)
        .map(|&(_, label, action, _)| (label, describe(action)))
        .collect();
    let widest = rows.iter().map(|(_, d)| d.len()).max().unwrap_or(20);
    let (resting, size, cols) = layout(rect, rows.len(), widest);
    // Slide down from above the top edge. At t = 0 the card sits entirely off
    // the top, so the first frame of the animation is genuinely invisible
    // rather than a flash of a half-drawn card.
    let t = t.clamp(0.0, 1.0);
    let travel = resting.bottom() - rect.top();
    let card_rect = resting.translate(egui::vec2(0.0, -(1.0 - t) * travel));

    let row_h = size * 1.75;
    let pad = size * 1.4;
    let key_w = size * 3.2;
    let per_col = rows.len().div_ceil(cols);
    let col_w = (card_rect.width() - pad * 2.0) / cols as f32;
    let font = FontId::new(size, crate::fonts::courier());
    let bold = FontId::new(size, crate::fonts::courier_bold());

    // Dim what is behind it, so the card reads as modal without being one.
    // The dimming fades with the slide, or the app would go dark before the
    // card had arrived to explain why.
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha((150.0 * t) as u8));

    let (bg, fg, dim, edge) = if dark {
        (
            Color32::from_rgb(0x1a, 0x1a, 0x1a),
            Color32::from_rgb(0xE8, 0xDC, 0xC0),
            Color32::from_rgb(0x99, 0x99, 0x99),
            Color32::from_rgb(0x5A, 0x5A, 0x5A),
        )
    } else {
        (
            Color32::from_rgb(0xE8, 0xDC, 0xC0),
            Color32::from_rgb(0x2a, 0x1e, 0x14),
            Color32::from_rgb(0x6d, 0x5a, 0x46),
            Color32::from_rgb(0x8B, 0x73, 0x55),
        )
    };
    painter.rect_filled(card_rect, 4.0, bg);
    painter.rect_stroke(
        card_rect,
        4.0,
        Stroke::new(1.0_f32, edge),
        StrokeKind::Middle,
    );

    for (i, (label, desc)) in rows.iter().enumerate() {
        let x = card_rect.left() + pad + (i / per_col) as f32 * col_w;
        let y = card_rect.top() + pad * 1.2 + (i % per_col) as f32 * row_h;
        painter.text(Pos2::new(x, y), Align2::LEFT_TOP, label, bold.clone(), fg);
        painter.text(
            Pos2::new(x + key_w, y),
            Align2::LEFT_TOP,
            desc,
            font.clone(),
            dim,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The card is generated from the binding table, so a shortcut that works
    /// but is not listed cannot exist. This asserts the table itself is sane.
    #[test]
    fn every_binding_is_unique_and_described() {
        let mut keys: Vec<Key> = Vec::new();
        for &(key, label, action, _) in BINDINGS {
            assert!(!keys.contains(&key), "{label} is bound twice");
            keys.push(key);
            assert!(!label.is_empty());
            assert!(!describe(action).is_empty(), "{label} has no description");
        }
        // Everything the card shows is a real binding, and every binding the
        // card hides is deliberate.
        // Hidden rows are the ALTERNATES: Esc, and F1 as a second help key.
        // Every hidden one must duplicate an action that is shown, or a
        // shortcut exists that the card never mentions.
        let shown: Vec<KeyAction> = BINDINGS
            .iter()
            .filter(|(.., s)| *s)
            .map(|&(_, _, a, _)| a)
            .collect();
        for &(_, label, action, is_shown) in BINDINGS {
            if !is_shown && action != KeyAction::CloseHelp {
                assert!(shown.contains(&action), "{label} is bound but never listed");
            }
        }
    }

    /// Letters that are ordinary shortcuts must not fire with a modifier held.
    /// Cmd-R and Ctrl-R belong to the OS, and swallowing them would be rude.
    #[test]
    fn a_modifier_suppresses_every_shortcut() {
        let ctx = egui::Context::default();
        for mods in [
            egui::Modifiers::COMMAND,
            egui::Modifiers::CTRL,
            egui::Modifiers::ALT,
            egui::Modifiers::SHIFT,
        ] {
            let mut input = egui::RawInput::default();
            input.modifiers = mods;
            input.events.push(egui::Event::Key {
                key: Key::R,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: mods,
            });
            let mut got = Some(KeyAction::ClearNotes);
            let _ = ctx.run(input, |ctx| got = pressed(ctx));
            assert_eq!(got, None, "R fired with {mods:?} held");
        }
    }

    #[test]
    fn a_bare_key_does_fire() {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.events.push(egui::Event::Key {
            key: Key::K,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        });
        let mut got = None;
        let _ = ctx.run(input, |ctx| got = pressed(ctx));
        assert_eq!(got, Some(KeyAction::ToggleKeytoggle));
    }

    /// The card must fit inside the window it is drawn over, at every size the
    /// app can be, or the shortcut list is the thing you cannot read.
    /// It has to FIT. The first version sized itself from the window height
    /// alone, so the card outgrew the app the moment another shortcut was added
    /// and the top and bottom rows were cut off by the window edges. "It did
    /// not panic" was all the old test checked, which is how that reached a
    /// screenshot.
    #[test]
    fn the_card_fits_the_window_at_every_size_and_row_count() {
        let shown = BINDINGS.iter().filter(|(.., s)| *s).count();
        for w in [650.0_f32, 975.0, 1300.0, 1950.0, 2600.0] {
            for h in [w / 8.667, w / 8.667 + 50.0, w / 8.667 + 182.0, 90.0] {
                let rect = Rect::from_min_size(Pos2::new(7.0, 11.0), Vec2::new(w, h));
                for rows in [1, shown, shown + 4, 20] {
                    let (card, size, cols) = layout(rect, rows, 46);
                    // ALWAYS: nothing is ever painted outside the app.
                    assert!(size >= 8.0, "text shrank below legible at {w}x{h}");
                    assert!(
                        rect.contains_rect(card),
                        "card {card:?} escapes window {rect:?}, {rows} rows"
                    );
                    // AND wherever there is room, every row has somewhere to be.
                    // The exception is real rather than a fudge: the smallest
                    // this app goes is 50% with the chord strip and the guitar
                    // view both off, 650x75, and no number of legible rows fits
                    // in 75 points. It clips there rather than escaping, and the
                    // window is one keypress from being bigger.
                    if h >= 120.0 {
                        let used = rows.div_ceil(cols) as f32 * size * 1.75 + size * 1.4 * 1.2;
                        assert!(
                            used <= card.height() + 0.5,
                            "{rows} rows in {cols} col(s) need {used} of {} at {w}x{h}",
                            card.height()
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_card_paints_without_panicking() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);
        for w in [650.0_f32, 1300.0, 2600.0] {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w / 8.667 + 50.0));
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    for t in [0.0, 0.01, 0.5, 0.999, 1.0] {
                        draw_help(ui.painter(), rect, false, t);
                        draw_help(ui.painter(), rect, true, t);
                    }
                    // Out of range must not escape the window either.
                    draw_help(ui.painter(), rect, false, -1.0);
                    draw_help(ui.painter(), rect, true, 2.0);
                });
            });
        }
    }
}
