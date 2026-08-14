//! The seam between "something hands us a window" and "we draw into it".
//!
//! Three different hosts call into this app and all three hand over an
//! `egui::Context` rather than a `Ui`:
//!
//!   * `eframe::App::update` for the desktop window,
//!   * `Context::show_viewport_immediate` for each child window, and
//!   * `nih_plug_egui`'s editor callback for the VST3 build.
//!
//! Every drawing surface here paints absolutely into `max_rect` and owns its
//! whole window, so the bridge is the same in all three cases and costs
//! nothing: one central panel with no frame, no margin and no fill.
//!
//! Keeping it in one place matters more than it looks. The desktop and plugin
//! builds have to render identically, and the fastest way to end up with two
//! subtly different GUIs is to let each entry point grow its own idea of what
//! the root panel should be.

/// Run `add` against a `Ui` covering the whole of `ctx`'s viewport.
pub fn viewport_ui<R>(ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, add)
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole point of `Frame::NONE`: the bridge must hand over the entire
    /// viewport with no margin and no inset. A default `CentralPanel` adds
    /// both, which would shift the piano and silently break the fixed-size
    /// geometry the app depends on. Cheaper and more reliable than eyeballing
    /// a screenshot, and it cannot rot.
    #[test]
    fn the_bridge_gives_away_the_whole_viewport() {
        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1300.0, 200.0));
        let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };

        let mut got = None;
        let _ = ctx.run(input, |ctx| {
            viewport_ui(ctx, |ui| got = Some(ui.max_rect()));
        });

        let got = got.expect("the bridge never called its closure");
        assert!(
            (got.min.x - screen.min.x).abs() < 0.5 && (got.min.y - screen.min.y).abs() < 0.5,
            "bridge inset the origin: {got:?} vs {screen:?}"
        );
        assert!(
            (got.width() - screen.width()).abs() < 0.5
                && (got.height() - screen.height()).abs() < 0.5,
            "bridge shrank the drawing area: {got:?} vs {screen:?}"
        );
    }
}
