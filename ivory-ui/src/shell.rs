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

use crate::host::Caps;
use egui::{Color32, Pos2, Rect, Vec2};

/// Run `add` against a `Ui` covering the whole of `ctx`'s viewport.
pub fn viewport_ui<R>(ctx: &egui::Context, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE)
        .show(ctx, add)
        .inner
}

/// A pane that is an OS window on the desktop and a floating layer in a plugin.
///
/// The fields are exactly the union of what the real callers set on a
/// `ViewportBuilder`; nothing is here speculatively. `id` is used for BOTH the
/// viewport id and the inline `Area` id, so the two paths cannot drift apart by
/// one of them being renamed.
pub struct SurfaceSpec<'a> {
    /// Stable identity. Changing one of these strings resets a window's
    /// remembered geometry, so they are written down once and left alone.
    pub id: &'a str,
    pub title: &'a str,
    pub size: Vec2,
    pub min_size: Vec2,
    /// Top-left. Monitor points on the desktop, canvas points inline. None
    /// means centre it: the desktop lets the OS place the window, inline
    /// centres on the canvas.
    pub pos: Option<Pos2>,
    pub decorated: bool,
    pub resizable: bool,
    /// Whether the surface takes key focus. The submenu passes false so it
    /// cannot steal focus from the menu and close it.
    pub takes_focus: bool,
    /// Dim and block everything underneath. Dialogs only, and only inline —
    /// on the desktop modality is enforced by the app dropping input, because
    /// there is nothing to dim.
    pub modal: bool,
    /// Inline stacking. The submenu must sit above the menu.
    pub order: egui::Order,
    /// Whether the surface floats above ordinary windows.
    ///
    /// **True for everything, except while a native file panel is up.** A
    /// modal dialog has to float: this app enforces modality by dropping input,
    /// so a dialog that ends up behind the main window is an app that has
    /// frozen with nothing on screen saying why — that failure has been fixed
    /// twice already. But a floating dialog also floats above the OS's own file
    /// panel, which is parented to the MAIN window, and then "Load .syx..."
    /// appears to do nothing at all. So it is a flag rather than a constant,
    /// and the one moment it goes false is the one moment it must.
    pub on_top: bool,
    /// Come to the front NOW, whatever the window manager decided earlier.
    ///
    /// **Set when the user presses the window this one is modal over.** A
    /// modal dialog behind its parent is an app that has stopped responding
    /// with no visible reason why — `handle_main_interaction` drops every
    /// event while one is open — and `AlwaysOnTop` is a request some window
    /// managers ignore. A press on the parent is the moment the user is
    /// wondering where their click went, so it is the moment to answer.
    pub raise: bool,
    /// The `_NET_WM_WINDOW_TYPE` hint, X11 only, ignored everywhere else.
    ///
    /// This is what tells a Linux window manager what the window IS. It
    /// matters most under tiling WMs (i3, sway): a `Normal` resizable window
    /// gets TILED into the layout — which for a modal dialog means it opens
    /// as a random pane UNDERNEATH the floating main window while the app
    /// ignores all input waiting on it, i.e. the app appears frozen. `Dialog`
    /// and `PopupMenu` windows are floated by every WM that reads the hint.
    pub window_type: Option<egui::X11WindowType>,
}

impl Default for SurfaceSpec<'_> {
    fn default() -> Self {
        Self {
            id: "",
            title: "Tangent",
            size: Vec2::ZERO,
            min_size: Vec2::ZERO,
            pos: None,
            decorated: false,
            resizable: false,
            takes_focus: true,
            modal: false,
            on_top: true,
            raise: false,
            order: egui::Order::Foreground,
            window_type: None,
        }
    }
}

/// What the host says happened to a surface after a frame.
///
/// `close` is deliberately one field fed by four sources — a title-bar close,
/// Escape, the body asking, and (inline) a click outside. Callers that treated
/// those separately were the reason Escape worked on one surface and not
/// another.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct SurfaceReport {
    pub close: bool,
    /// `None` inline: there is no window, so there is no focus to lose. Callers
    /// must not read absence as "unfocused" — that is what would make a plugin
    /// menu close itself on the first frame.
    pub focused: Option<bool>,
    /// A pointer press landed outside this surface's rect this frame. Only ever
    /// true inline; on the desktop, a click elsewhere shows up as focus loss.
    pub pressed_outside: bool,
    /// Whether the pointer is currently over this surface's window. `Some`
    /// only on the desktop, where each viewport has its own pointer stream and
    /// a `PointerGone` marks the cursor leaving. `None` inline.
    ///
    /// This exists because focus is not a safe proxy for attention on Linux:
    /// under i3 a freshly created submenu window takes the focus and the menu
    /// window never gets it back, so "nothing of mine is focused" can be true
    /// while the user's pointer is sitting in the middle of the menu.
    pub pointer_over: Option<bool>,
}

fn viewport_id(id: &str) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(id)
}

/// Draw `add` into a surface, as an OS window or in the canvas.
///
/// **This is the only place in `ivory-ui` that decides between the two.** The
/// plugin path is not a fallback for calls that fail: in a plugin
/// `show_viewport_immediate` still RUNS, because the context is built with
/// `embed_viewports: true`, so it invokes the closure inline and opens a second
/// `CentralPanel` under the same id as the first — painting garbage over the
/// piano rather than erroring. A hard error would have been kinder. That is why
/// the branch sits ABOVE `viewport_ui` rather than inside it.
///
/// `add` receives the `Ui` and a `&mut bool` it sets to ask for the surface to
/// close (a Cancel button, a menu item chosen). Every other reason to close is
/// folded in here.
pub fn surface(
    ctx: &egui::Context,
    caps: Caps,
    spec: &SurfaceSpec<'_>,
    add: &mut dyn FnMut(&mut egui::Ui, &mut bool),
) -> SurfaceReport {
    let mut report = SurfaceReport::default();
    let mut body_wants_close = false;

    if caps.child_windows {
        // **Born hidden, revealed on the second pass.**
        //
        // A window is created VISIBLE unless told otherwise, and on Windows a
        // window that exists before anything has been drawn into it shows one
        // frame of undefined back buffer — white, in practice. eframe's own
        // main window dodges exactly this by starting hidden and revealing
        // itself after the first frame (egui#3631, and the comment there says
        // "to fix white flash on startup"); a CHILD viewport gets no such
        // treatment, and this app's menu, submenu panel and dialogs are all
        // child viewports that appear and disappear while somebody is using
        // it.
        //
        // A surface that was drawn on the PREVIOUS pass already has a window,
        // so it stays visible; one that was not is being created right now.
        // Consecutive calls differ by exactly one pass — not drawing a surface
        // is what destroys it — so the gap is a reliable "this is new".
        let life = egui::Id::new(("surface-life", spec.id));
        let pass = ctx.cumulative_pass_nr();
        // `(the pass it was born on, the last pass it was drawn on)`. Drawn on
        // the previous pass means the window is still there; a gap means this
        // one is brand new, because not drawing a surface is what destroys it.
        let alive = ctx
            .memory(|m| m.data.get_temp::<(u64, u64)>(life))
            .filter(|(_, last)| pass.saturating_sub(*last) <= 1);
        let born = alive.map_or(pass, |(b, _)| b);
        ctx.memory_mut(|m| m.data.insert_temp(life, (born, pass)));
        let existing = alive.is_some();
        // The one pass on which it goes from hidden to shown.
        let revealing = existing && pass == born + 1;
        if !existing {
            // The reveal is a second pass, and a menu sitting still would not
            // otherwise ask for one.
            ctx.request_repaint();
        }
        let mut builder = egui::ViewportBuilder::default()
            .with_title(spec.title)
            .with_decorations(spec.decorated)
            .with_resizable(spec.resizable)
            .with_window_level(if spec.on_top {
                egui::WindowLevel::AlwaysOnTop
            } else {
                egui::WindowLevel::Normal
            })
            .with_active(spec.takes_focus)
            .with_visible(existing)
            .with_inner_size(spec.size)
            .with_min_inner_size(spec.min_size);
        if !spec.resizable {
            builder = builder.with_max_inner_size(spec.size);
        }
        if let Some(p) = spec.pos {
            builder = builder.with_position(p);
        }
        if let Some(t) = spec.window_type {
            builder = builder.with_window_type(t);
        }

        ctx.show_viewport_immediate(viewport_id(spec.id), builder, |vp, _class| {
            viewport_ui(vp, |ui| {
                let (close_req, esc, focused, pointer_over) = ui.input(|i| {
                    (
                        i.viewport().close_requested(),
                        i.key_pressed(egui::Key::Escape),
                        i.viewport().focused,
                        i.pointer.has_pointer(),
                    )
                });
                report.focused = focused;
                report.pointer_over = Some(pointer_over);
                report.close |= close_req || esc;
                // **Asked for on the reveal, because it was born invisible.**
                // `with_active` is a creation-time attribute and an invisible
                // window is not a window anybody can focus, so without this a
                // dialog could come up unable to hear Escape — which is the
                // only way some of them close. Only for surfaces that asked to
                // take focus: the submenu passes `takes_focus: false` because
                // stealing it from the menu is what closes the menu.
                if (revealing && spec.takes_focus) || spec.raise {
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
                    // The level as well as the focus: a window manager that
                    // dropped the request the first time gets asked again, and
                    // on the one that honoured it this changes nothing.
                    if spec.on_top {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                            egui::WindowLevel::AlwaysOnTop,
                        ));
                    }
                }
                add(ui, &mut body_wants_close);
            });
        });
        report.close |= body_wants_close;
        return report;
    }

    // ── In-canvas ──────────────────────────────────────────────────────────
    // An Area needs only a Context, so this works identically in a host-owned
    // window with no window manager anywhere in sight.
    let screen = ctx.content_rect();
    let size = Vec2::new(
        spec.size
            .x
            .min(screen.width() * 0.94)
            .max(spec.min_size.x.min(screen.width())),
        spec.size
            .y
            .min(screen.height() * 0.94)
            .max(spec.min_size.y.min(screen.height())),
    );
    let rect = match spec.pos {
        // Clamped rather than trusted: a right-click near the bottom of a short
        // editor would otherwise put the menu mostly off the canvas, and unlike
        // a window there is no desktop for it to hang over.
        Some(p) => {
            let min = Pos2::new(
                p.x.clamp(screen.min.x, (screen.max.x - size.x).max(screen.min.x)),
                p.y.clamp(screen.min.y, (screen.max.y - size.y).max(screen.min.y)),
            );
            Rect::from_min_size(min, size)
        }
        None => Rect::from_center_size(screen.center(), size),
    };

    if spec.modal {
        // Dim the app behind it. Without this the piano behind a plugin dialog
        // looks live when it is not.
        egui::Area::new(egui::Id::new((spec.id, "scrim")))
            .order(egui::Order::Middle)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(150));
                // Swallow clicks so the app underneath cannot be operated
                // through the scrim, which is what makes it genuinely modal.
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
            });
    }

    egui::Area::new(egui::Id::new(spec.id))
        .order(spec.order)
        .fixed_pos(rect.min)
        .show(ctx, |ui| {
            ui.set_clip_rect(rect);
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    report.close = true;
                }
                // SCROLL rather than clip when the body does not fit.
                //
                // Every body here paints absolutely into `max_rect`, so handing
                // it a rect smaller than it asked for does not make it smaller
                // — it makes it CUT OFF, with the rows past the bottom edge
                // drawn and unreachable. The menu is about 460 points tall and
                // a plugin editor is often shorter than that, so the last third
                // of it, Reset and About included, simply could not be clicked.
                //
                // On the desktop, and inline whenever the surface fits, this
                // branch is not taken and not one pixel moves.
                if spec.size.y > rect.height() + 0.5 || spec.size.x > rect.width() + 0.5 {
                    egui::ScrollArea::both()
                        .auto_shrink([false; 2])
                        .show(ui, |ui| {
                            let natural = Rect::from_min_size(ui.max_rect().min, spec.size);
                            let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(natural));
                            add(&mut inner, &mut body_wants_close);
                            // Claim the full height so the scrollbar knows how
                            // far there is to go.
                            ui.allocate_rect(natural, egui::Sense::hover());
                        });
                } else {
                    add(ui, &mut body_wants_close);
                }
            });
        });

    report.pressed_outside = ctx.input(|i| i.pointer.any_pressed())
        && ctx.pointer_interact_pos().is_none_or(|p| !rect.contains(p));
    report.close |= body_wants_close;
    report
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
        let screen = Rect::from_min_size(Pos2::ZERO, egui::vec2(1300.0, 200.0));
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };

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

    /// Returns (what the body was given, what is actually visible, report).
    fn run_inline(screen: Rect, spec: &SurfaceSpec<'_>) -> (Rect, Rect, SurfaceReport) {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        let mut got = Rect::NOTHING;
        let mut visible = Rect::NOTHING;
        let mut report = SurfaceReport::default();
        let _ = ctx.run(input, |ctx| {
            report = surface(ctx, Caps::PLUGIN, spec, &mut |ui, _close| {
                got = ui.max_rect();
                visible = ui.clip_rect();
            });
        });
        (got, visible, report)
    }

    /// The inline surface must land inside the canvas whatever position it is
    /// asked for. A menu opened by a right-click near the bottom edge of a
    /// short plugin editor is the real case: on the desktop it may hang over
    /// the rest of the screen, inline there is nothing to hang over, and an
    /// unclamped menu is one whose lower rows cannot be clicked.
    #[test]
    fn an_inline_surface_stays_inside_the_canvas() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0));
        for pos in [
            Pos2::new(590.0, 195.0),
            Pos2::new(-40.0, -40.0),
            Pos2::new(300.0, 100.0),
        ] {
            let spec = SurfaceSpec {
                id: "test-clamp",
                size: Vec2::new(200.0, 120.0),
                min_size: Vec2::new(120.0, 60.0),
                pos: Some(pos),
                ..Default::default()
            };
            let (_, visible, _) = run_inline(screen, &spec);
            assert!(
                screen.expand(1.0).contains_rect(visible),
                "asked for {pos:?}, visible area {visible:?} is not inside {screen:?}"
            );
        }
    }

    /// A surface bigger than the editor SCROLLS. It must not be shrunk, and it
    /// must not be clipped.
    ///
    /// This test used to assert the opposite — that the body is handed a rect
    /// that fits — and that assertion was the bug. Every body here paints
    /// absolutely into `max_rect`, so a smaller rect does not make a smaller
    /// menu; it makes one whose bottom rows are drawn past the edge and cannot
    /// be clicked. The menu is ~460pt tall and a plugin editor is often less,
    /// so Reset, About and Theory were all unreachable in a plugin.
    #[test]
    fn an_oversized_inline_surface_scrolls_rather_than_clipping() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0));
        let spec = SurfaceSpec {
            id: "test-big",
            size: Vec2::new(900.0, 460.0),
            min_size: Vec2::new(200.0, 100.0),
            ..Default::default()
        };
        let (got, visible, _) = run_inline(screen, &spec);

        // The body gets what it asked for, so nothing it draws falls off.
        assert!(
            got.width() >= spec.size.x - 0.5 && got.height() >= spec.size.y - 0.5,
            "the body was given {got:?}, less than the {:?} it needs",
            spec.size
        );
        // What is on screen still fits on screen.
        assert!(
            screen.expand(1.0).contains_rect(visible),
            "the visible area {visible:?} escapes {screen:?}"
        );
    }

    /// ...and a surface that DOES fit is handed exactly its rect, with no
    /// scroll area in the way. This is every dialog on the desktop and most of
    /// them in a plugin, so it is the path that must not change.
    #[test]
    fn a_surface_that_fits_is_not_wrapped_in_anything() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 400.0));
        let spec = SurfaceSpec {
            id: "test-fits",
            size: Vec2::new(300.0, 200.0),
            min_size: Vec2::new(100.0, 100.0),
            ..Default::default()
        };
        let (got, _, _) = run_inline(screen, &spec);
        assert!(
            (got.width() - 300.0).abs() < 0.5 && (got.height() - 200.0).abs() < 0.5,
            "a surface that fits was resized to {got:?}"
        );
    }

    /// An oversized inline surface must actually SCROLL when the wheel turns.
    ///
    /// "It has a ScrollArea in it" is not the same as "it scrolls": the area
    /// only scrolls if it knows its content is bigger than its viewport, and
    /// the content size here comes from an explicitly allocated rect rather
    /// than from laying widgets out.
    #[test]
    fn an_oversized_inline_surface_really_scrolls() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0));
        let spec = SurfaceSpec {
            id: "test-scroll",
            size: Vec2::new(400.0, 900.0),
            min_size: Vec2::new(400.0, 900.0),
            pos: Some(Pos2::ZERO),
            ..Default::default()
        };
        let ctx = egui::Context::default();
        let mut body_top = 0.0_f32;
        let mut run = |events: Vec<egui::Event>| {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    surface(ctx, Caps::PLUGIN, &spec, &mut |ui, _| {
                        body_top = ui.max_rect().min.y;
                    });
                },
            );
            body_top
        };

        run(vec![]);
        let before = run(vec![egui::Event::PointerMoved(Pos2::new(200.0, 100.0))]);
        // Three notches down, over the middle of the surface.
        let after = run(vec![
            egui::Event::PointerMoved(Pos2::new(200.0, 100.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -240.0),
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        run(vec![]);
        let settled = run(vec![]);

        assert!(
            settled < before - 50.0,
            "the wheel moved the content from {before} to {settled} \
             (immediately after the event: {after}) - it is not scrolling"
        );
    }

    /// Inline has no window, so it has no focus. Reporting `Some(false)` would
    /// be read by the menu as "the user clicked away" on the very first frame,
    /// and the menu would shut before it could be used.
    #[test]
    fn inline_reports_no_focus_rather_than_unfocused() {
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0));
        let spec = SurfaceSpec {
            id: "test-focus",
            size: Vec2::new(100.0, 100.0),
            ..Default::default()
        };
        let (_, _, report) = run_inline(screen, &spec);
        assert_eq!(report.focused, None, "inline invented a focus state");
        assert!(!report.close, "an untouched surface asked to close");
    }

    /// The body's own close request has to survive the trip back, on both
    /// paths. It is how Cancel works.
    #[test]
    fn the_body_can_ask_to_close() {
        let ctx = egui::Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 200.0));
        let spec = SurfaceSpec {
            id: "test-close",
            size: Vec2::new(100.0, 100.0),
            ..Default::default()
        };
        let mut report = SurfaceReport::default();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                report = surface(ctx, Caps::PLUGIN, &spec, &mut |_ui, close| *close = true);
            },
        );
        assert!(report.close, "the body asked to close and was ignored");
    }
}
