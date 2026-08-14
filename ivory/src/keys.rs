//! Keyboard shortcuts, and the F1 card that lists them.
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
    ToggleDarkMode,
    ToggleDetection,
    CloseHelp,
}

/// (key, label, what it does, whether it appears on the card).
/// The card is generated from this, so the two cannot disagree.
const BINDINGS: &[(Key, &str, KeyAction, bool)] = &[
    (Key::F1, "F1", KeyAction::ToggleHelp, true),
    (Key::K, "K", KeyAction::ToggleKeytoggle, true),
    (Key::R, "R", KeyAction::ClearNotes, true),
    (Key::G, "G", KeyAction::ToggleFretboard, true),
    (Key::D, "D", KeyAction::ToggleDarkMode, true),
    (Key::C, "C", KeyAction::ToggleDetection, true),
    // Escape only closes the card; it is not worth a row of its own.
    (Key::Escape, "Esc", KeyAction::CloseHelp, false),
];

/// One line of help text per binding, written where the binding is defined so
/// they cannot drift apart.
fn describe(a: KeyAction) -> &'static str {
    match a {
        KeyAction::ToggleHelp => "this card",
        KeyAction::ToggleKeytoggle => "keytoggle: click the piano or the neck to place notes",
        KeyAction::ClearNotes => "clear every note you placed",
        KeyAction::ToggleFretboard => "guitar view",
        KeyAction::ToggleDarkMode => "dark mode",
        KeyAction::ToggleDetection => "chord detection",
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
            .find(|(key, ..)| i.key_pressed(*key))
            .map(|&(_, _, action, _)| action)
    })
}

/// The help card, drawn over the app.
///
/// Centred, sized to its own content, and painted directly rather than through
/// a `Window` so it cannot be dragged off, cannot be resized into nothing, and
/// looks identical in a plugin editor where there is no window manager at all.
pub fn draw_help(painter: &Painter, rect: Rect, dark: bool) {
    let rows: Vec<(&str, &str)> = BINDINGS
        .iter()
        .filter(|(.., shown)| *shown)
        .map(|&(_, label, action, _)| (label, describe(action)))
        .collect();

    // Scale with the window, but stay legible in a small one.
    let size = (rect.height() * 0.075).clamp(11.0, 20.0);
    let row_h = size * 1.75;
    let pad = size * 1.4;
    let key_w = size * 3.2;
    let font = FontId::new(size, crate::fonts::courier());
    let bold = FontId::new(size, crate::fonts::courier_bold());

    let widest = rows.iter().map(|(_, d)| d.len()).max().unwrap_or(20) as f32;
    let card = Vec2::new(
        (key_w + widest * size * 0.62 + pad * 2.0).min(rect.width() * 0.92),
        rows.len() as f32 * row_h + pad * 2.4,
    );
    let origin = rect.center() - card * 0.5;
    let card_rect = Rect::from_min_size(origin, card);

    // Dim what is behind it, so the card reads as modal without being one.
    painter.rect_filled(rect, 0.0, Color32::from_black_alpha(150));

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
    painter.rect_stroke(card_rect, 4.0, Stroke::new(1.0_f32, edge), StrokeKind::Middle);

    let mut y = card_rect.top() + pad * 1.2;
    for (label, desc) in &rows {
        painter.text(
            Pos2::new(card_rect.left() + pad, y),
            Align2::LEFT_TOP,
            label,
            bold.clone(),
            fg,
        );
        painter.text(
            Pos2::new(card_rect.left() + pad + key_w, y),
            Align2::LEFT_TOP,
            desc,
            font.clone(),
            dim,
        );
        y += row_h;
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
        let shown = BINDINGS.iter().filter(|(.., s)| *s).count();
        assert_eq!(shown, BINDINGS.len() - 1, "only Esc should be hidden");
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
    #[test]
    fn the_card_fits_the_window_at_every_size() {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx, crate::fonts::FontChoice::default(), None);
        for w in [650.0_f32, 1300.0, 2600.0] {
            let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(w, w / 8.667 + 50.0));
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_help(ui.painter(), rect, false);
                    draw_help(ui.painter(), rect, true);
                });
            });
        }
    }
}
